//! Keeping a table in shape: packing and zapping it, recalling every
//! deleted record, flushing it, adding orders to it, and the raw records
//! (slots) that copying and restructuring read.

use super::*;

impl Table {
    /// Whether the file is an AES table, armed (an ADT or a VFP one).
    pub(crate) fn file_is_aes(&self) -> bool {
        self.state().file().is_aes()
    }

    /// Whether the table has a password this handle has not given.
    pub(crate) fn password_missing(&self) -> bool {
        !self.keyed() && self.state().file().has_password()
    }

    /// Whether the table is a Visual FoxPro one laid out for AES.
    pub(crate) fn dbf_aes_layout(&self) -> bool {
        self.state().file().dbf_aes_layout()
    }

    /// Marks a DBF table's header as having a structural `.cdx` file.
    pub(crate) fn mark_structural_index(&self) -> Result<(), Error> {
        let state = self.state();
        state.file().mark_structural_index().map_err(write_error)
    }

    /// Marks a DBF table's header as having no structural `.cdx` file.
    pub(crate) fn unmark_structural_index(&self) -> Result<(), Error> {
        let state = self.state();
        state.file().unmark_structural_index().map_err(write_error)
    }

    /// The checksum of a blank record (see [`Table::record_crc`]), which a
    /// table with no records gives for any record number (`sp_GetRecordCRC`).
    pub(crate) fn blank_record_crc(&self) -> u32 {
        let blank = self.structure.null_record();
        crc32_unfinished(blank.get(self.record_prefix()..).unwrap_or_default())
    }

    /// Counts `order` as open on the table, to keep current.
    pub(crate) fn add_order(&self, order: &Arc<Index>) {
        let mut state = self.state();
        state.orders.retain(|order| order.strong_count() > 0);
        state.orders.push(Arc::downgrade(order));
    }

    /// Packs the table's file (`AdsPackTable`), or with `zap` empties it
    /// (`AdsZapTable`), as the original engine does (see
    /// [`TableFile::pack`]); the table must be open exclusively
    /// ([`Error::NotExclusive`]). Its orders are rebuilt by the caller.
    pub(crate) fn pack_file(&self, zap: bool, block_size: Option<u16>) -> Result<(), Error> {
        if !self.exclusive() {
            return Err(Error::NotExclusive);
        }
        let mut state = self.state();
        state.flush()?;
        let mut file = state.file();
        // Records move when a table is packed, and an enciphered one is
        // enciphered for its place: without the password they cannot move.
        if !zap && !self.keyed() && file.has_password() {
            return Err(match file.is_aes() {
                true => Error::DictionaryEncrypted,
                false => Error::EncryptionNotEnabled,
            });
        }
        let packed = match zap {
            true => file.zap(),
            false => file.pack_to(block_size),
        };
        packed.map_err(write_error)
    }

    /// `AdsEnableEncryption`: set the table's RC4 encryption password, writing
    /// the header's password check and type and arming the cipher so later writes
    /// encipher and enciphered records read deciphered (see
    /// [`TableFile::enable_encryption`]). This alone does not encrypt the table.
    pub fn enable_encryption(&self, password: &[u8]) -> Result<(), Error> {
        // The connection's EncryptionType chooses the scheme: AES when the table
        // was created on an AES connection (so its file already has the AES
        // layout), otherwise the default RC4. For AES this is the step that
        // encrypts the table (fills the crypto block, sets the ENCRYPTED flag and
        // arms the cipher); RC4 only sets the password check and arms the cipher.
        let enc_type = match crate::ddl::plain_layout() {
            true => 0,
            false => self.owner().map(|c| c.encryption_type()).unwrap_or(0),
        };
        let mut state = self.state();
        state.flush()?;
        // A record kept after the password went is read again, as the file
        // has it, when the password is given again.
        if let Some((record, _)) = state.image.take() {
            let length = self.structure.header.record_length as usize;
            let bytes = state.read(record, length)?;
            state.image = Some((record, bytes));
        }
        let mut file = state.file();
        file.unsealed(|file| {
            // On a table that has its password already (an RC4 one enabled
            // before, or an AES-encrypted one), this supplies it: checked, a
            // wrong one refused (7060, `share.json`) and the file left as it
            // is; the same call reads an encrypted table as encrypts a fresh
            // one, as the engine does.
            if file.has_password() {
                return match file.supply_password(password) {
                    Err(StorageError::Encrypted) => Err(Error::InvalidPassword),
                    result => result.map(drop).map_err(write_error),
                };
            }
            if enc_type != 0 {
                let material =
                    tflip_storage::create::AesMaterial::generate(enc_type).map_err(write_error)?;
                // A Visual FoxPro table laid out for AES is armed in its
                // back link (`fixtures/aes/rc4tp`, `v12`).
                if file.dbf_aes_layout() {
                    return file
                        .enable_encryption_dbf_aes(&material, password)
                        .map_err(write_error);
                }
                return file
                    .enable_encryption_aes(&material, password)
                    .map_err(write_error);
            }
            let key = &password[..password.len().min(tflip_storage::crypto::KEY_MAX)];
            file.enable_encryption(key, tflip_storage::adt::ENCRYPTION_RC4)
                .map_err(write_error)
        })?;
        self.keyed.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// `AdsDisableEncryption`: the handle lets go of the table's password (no
    /// file change); other handles that gave it keep theirs.
    pub fn disable_encryption(&self) -> Result<(), Error> {
        let mut state = self.state();
        state.flush()?;
        // The record an exclusive handle stands on stays as it was read,
        // deciphered, though the password is gone; a shared handle reads
        // the file again (`fixtures/write/encryption`).
        if self.keyed()
            && self.exclusive()
            && let Some(record) = state.position.record
            && state.file().has_password()
        {
            let length = self.structure.header.record_length as usize;
            let bytes = state.read(record, length)?;
            state.image = Some((record, bytes));
        }
        self.keyed.store(false, Ordering::Relaxed);
        Ok(())
    }

    /// Why a handle cannot (de)cipher the table: it has not given its password
    /// ([`Error::EncryptionNotEnabled`], `share.json`).
    fn check_keyed(&self) -> Result<(), Error> {
        match self.keyed() {
            true => Ok(()),
            false => Err(Error::EncryptionNotEnabled),
        }
    }

    /// `AdsEncryptTable`: encipher every record and memo value and set the
    /// table's ENCRYPTED flag (see [`TableFile::encrypt_table`]). The table must
    /// be open exclusively, and its password set ([`Table::enable_encryption`]).
    pub fn encrypt_table(&self) -> Result<(), Error> {
        if self.lacks_aes_password() {
            return Err(Error::EncryptionNotEnabled.detailed("Cannot encrypt table."));
        }
        if !self.exclusive() {
            return Err(Error::NotExclusive);
        }
        // A dictionary's table is encrypted by its property
        // (`fixtures/aes/rc4tp`, `x5`).
        if self.object_id.is_some() {
            return Err(match self.structure.header.encrypted() {
                true => Error::TableEncrypted.detailed("The table is encrypted."),
                false => Error::EncryptionNotEnabled.detailed("Cannot encrypt table."),
            });
        }
        self.check_keyed()
            .map_err(|_| Error::EncryptionNotEnabled.detailed("Cannot encrypt table."))?;
        let mut state = self.state();
        state.flush()?;
        // A Clipper or FoxPro table that is encrypted says so (an AES one is
        // armed by `AdsEnableEncryption`, and has its records left).
        if self.format() != Format::Adt
            && !state.file().dbf_aes_armed()
            && state.file().structure.header.encrypted()
        {
            return Err(Error::TableEncrypted);
        }
        state.file().encrypt_table().map_err(write_error)
    }

    /// Encrypts the table, which is bound to the AES dictionary at
    /// `add_path` (see [`TableFile::encrypt_for_dictionary`]); open
    /// exclusively.
    pub fn encrypt_for_dictionary(
        &self,
        add_path: &std::path::Path,
        indexes: &[std::path::PathBuf],
    ) -> Result<(), Error> {
        if !self.exclusive() {
            return Err(Error::NotExclusive);
        }
        let mut state = self.state();
        state.flush()?;
        state
            .file()
            .encrypt_for_dictionary(add_path, indexes)
            .map_err(write_error)
    }

    /// Decrypts the table, which is bound to an AES dictionary (see
    /// [`TableFile::decrypt_for_dictionary`]); open exclusively.
    pub fn decrypt_for_dictionary(&self, indexes: &[std::path::PathBuf]) -> Result<(), Error> {
        if !self.exclusive() {
            return Err(Error::NotExclusive);
        }
        let mut state = self.state();
        state.flush()?;
        state
            .file()
            .decrypt_for_dictionary(indexes)
            .map_err(write_error)
    }

    /// Encrypts the table, which is bound to a dictionary of RC4 tables whose
    /// table password is `key` (see [`TableFile::encrypt_for_dictionary_rc4`]);
    /// open exclusively.
    pub fn encrypt_for_dictionary_rc4(
        &self,
        key: &[u8],
        indexes: &[std::path::PathBuf],
    ) -> Result<(), Error> {
        if !self.exclusive() {
            return Err(Error::NotExclusive);
        }
        let mut state = self.state();
        state.flush()?;
        state
            .file()
            .encrypt_for_dictionary_rc4(key, indexes)
            .map_err(write_error)
    }

    /// Decrypts the table, which is bound to a dictionary of RC4 tables (see
    /// [`TableFile::decrypt_for_dictionary_rc4`]); open exclusively.
    pub fn decrypt_for_dictionary_rc4(&self, indexes: &[std::path::PathBuf]) -> Result<(), Error> {
        if !self.exclusive() {
            return Err(Error::NotExclusive);
        }
        let mut state = self.state();
        state.flush()?;
        state
            .file()
            .decrypt_for_dictionary_rc4(indexes)
            .map_err(write_error)
    }

    /// `AdsDecryptTable`: decipher every record and memo value and clear the
    /// ENCRYPTED flag (see [`TableFile::decrypt_table`]); open exclusively.
    pub fn decrypt_table(&self) -> Result<(), Error> {
        // An AES table's missing password comes before the open mode
        // (`fixtures/aes/api/04_maintenance`).
        if self.lacks_aes_password() {
            return Err(Error::EncryptionNotEnabled.detailed("Cannot decrypt table."));
        }
        if !self.exclusive() {
            return Err(Error::NotExclusive);
        }
        if self.object_id.is_some() {
            return Err(Error::InvalidOption.detailed(
                "Database table can only be decrypted by setting the table property in the \
                 dictionary.",
            ));
        }
        self.check_keyed()
            .map_err(|_| Error::EncryptionNotEnabled.detailed("Cannot decrypt table."))?;
        let mut state = self.state();
        state.flush()?;
        state.file().decrypt_table().map_err(write_error)?;
        // The password is the table's no more.
        self.keyed.store(false, Ordering::Relaxed);
        Ok(())
    }

    /// `AdsEncryptRecord`: encipher the current record in place (record-level,
    /// the table flag untouched; see [`TableFile::encrypt_record`]).
    pub fn encrypt_record(&self) -> Result<(), Error> {
        // A Visual FoxPro table takes the whole table or none, with or
        // without the password (`fixtures/write/dbf_encryption_vfp`).
        if self.format() == Format::Dbf(tflip_storage::dbf::Flavour::Visual) {
            return Err(Error::EncryptionNotEnabled.detailed(
                "Can not encrypt single records with ADS_VFP table type. Use AdsEncryptTable \
                 instead.",
            ));
        }
        self.check_keyed()?;
        let slot = self.record_number();
        if slot == 0 {
            return Err(Error::NoCurrentRecord);
        }
        let mut state = self.state();
        state.flush()?;
        state.file().encrypt_record(slot).map_err(write_error)
    }

    /// `AdsDecryptRecord`: decipher the current record in place (see
    /// [`TableFile::decrypt_record`]).
    pub fn decrypt_record(&self) -> Result<(), Error> {
        self.check_keyed()?;
        let slot = self.record_number();
        if slot == 0 {
            return Err(Error::NoCurrentRecord);
        }
        let mut state = self.state();
        state.flush()?;
        state.file().decrypt_record(slot).map_err(write_error)
    }

    /// Whether the session has the table's encryption password (so
    /// `AdsIsEncryptionEnabled`/`AdsVerifyPassword` are true).
    pub fn encryption_enabled(&self) -> bool {
        matches!(
            self.encryption_cipher(),
            Some(
                tflip_storage::table::RecordCipher::Rc4(_)
                    | tflip_storage::table::RecordCipher::Aes { .. }
            )
        )
    }

    /// Whether the current record is enciphered (`AdsIsRecordEncrypted`): its
    /// status byte is 0x06 (active) or 0x07 (deleted), a DBF record's `E` or
    /// `D`. No current record, or an
    /// unencrypted one, is false.
    pub fn record_encrypted(&self) -> Result<bool, Error> {
        let slot = self.record_number();
        if slot == 0 {
            return Ok(false);
        }
        let state = self.state();
        let mut file = state.file();
        if slot > file.slots() {
            return Err(Error::NoCurrentRecord);
        }
        // A DBF record changed and not written yet is enciphered when it is,
        // as far as the original says (`fixtures/write/dbf_encryption_cdx`).
        if file.structure.header.format != Format::Adt
            && self.keyed()
            && file.has_password()
            && state
                .pending
                .as_ref()
                .is_some_and(|pending| pending.slot == slot && pending.changed)
        {
            return Ok(true);
        }
        let status = file.status_byte(slot).map_err(storage_error)?;
        // The record this handle appended in a transaction counts as one that
        // is, though the file holds its blank placeholder until the commit.
        if file.structure.header.format != Format::Adt
            && self.keyed()
            && file.has_password()
            && status == tflip_storage::dbf::TX_APPENDED
        {
            return Ok(true);
        }
        // An AES record does not count as one, though it is enciphered
        // (`share.json`).
        Ok(!file.is_aes() && file.enciphered(&[status & !tflip_storage::adt::TX_MARK]))
    }

    /// Recalls every deleted record (`AdsRecallAllRecords`), which needs the
    /// table open exclusively, adding their keys to the open orders (see
    /// [`TableFile::recall_all`]); the table then goes to its first record.
    pub fn recall_all(&self) -> Result<(), Error> {
        if !self.exclusive() {
            return Err(Error::NotExclusive);
        }
        let mut state = self.state();
        self.check_writable(&state)?;
        state.flush()?;
        let orders: Vec<Arc<Index>> = state.orders.iter().filter_map(Weak::upgrade).collect();
        let shared = Arc::clone(&state.file);
        let mut file = shared.file();
        // A DBF record loses its deletion flag, its keys following it.
        if let Format::Dbf(_) = file.structure.header.format {
            // As in the original engine, the index files' change counters do
            // not count these changes.
            let counters: Vec<(std::path::PathBuf, u64, Vec<u8>)> = {
                let mut seen = Vec::new();
                for order in &orders {
                    let at = match order.kind {
                        tflip_storage::order::Kind::Ntx => 0x02,
                        tflip_storage::order::Kind::Cdx => 0x0A,
                        tflip_storage::order::Kind::Adi => continue,
                    };
                    if seen
                        .iter()
                        .any(|(p, _, _): &(std::path::PathBuf, u64, Vec<u8>)| *p == order.path)
                    {
                        continue;
                    }
                    let bytes = std::fs::read(&order.path).map_err(|_| Error::CorruptIndex)?;
                    let counter = bytes
                        .get(at as usize..at as usize + 2)
                        .unwrap_or_default()
                        .to_vec();
                    // An encrypted table's recall leaves the counter at 1, as a
                    // rebuild does (`fixtures/write/dbf_encryption_cdx`).
                    let counter = match file.structure.header.encrypted() && at == 0x0A {
                        true => vec![0, 1],
                        false => counter,
                    };
                    seen.push((order.path.clone(), at, counter));
                }
                seen
            };
            let fields = file.structure.fields.clone();
            for slot in 1..=file.structure.header.record_slots {
                let mut bytes = file.record(slot).map_err(storage_error)?;
                if bytes.first() != Some(&b'*') {
                    continue;
                }
                let old = order_keys_with(&orders, &fields, Some(&bytes), slot, file.memo(), true)?;
                bytes[0] = b' ';
                let new = order_keys_with(&orders, &fields, Some(&bytes), slot, file.memo(), true)?;
                file.write(slot, &bytes).map_err(write_error)?;
                maintain(&orders, Some(&old), Some(&new), slot, true)?;
            }
            for (path, at, counter) in counters {
                use std::io::{Seek, SeekFrom, Write};
                let mut index = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .map_err(|_| Error::CorruptIndex)?;
                index
                    .seek(SeekFrom::Start(at))
                    .map_err(|_| Error::CorruptIndex)?;
                index.write_all(&counter).map_err(|_| Error::CorruptIndex)?;
            }
            drop((file, state));
            return self.goto_top();
        }
        let slots = file.recall_all().map_err(write_error)?;
        let fields = file.structure.fields.clone();
        for slot in slots {
            let bytes = file.record(slot).map_err(storage_error)?;
            let keys = order_keys_with(&orders, &fields, Some(&bytes), slot, file.memo(), true)?;
            maintain_recalled(&orders, &keys, slot)?;
        }
        // The orders' change counts reach their tag headers now
        // (`fixtures/write/append_reuse`).
        for order in &orders {
            let (_, mut index) = order_file(order)?;
            if let OrderFile::Adi(index) = &mut *index {
                index
                    .write_changes(&order.tag)
                    .map_err(|_| Error::CorruptIndex)?;
            }
        }
        drop((file, state));
        // As the original engine does, the table goes to its first record.
        self.goto_top()
    }

    /// Makes the table transaction-free or not
    /// (`AdsSetTableTransactionFree`), which needs it open exclusively and,
    /// as in the original engine, no transaction on its connection
    /// ([`Error::IllegalInTransaction`], checked after the open mode).
    pub fn set_transaction_free(&self, session: &Session, free: bool) -> Result<(), Error> {
        if !self.exclusive() {
            return Err(Error::NotExclusive);
        }
        let other = crate::transaction::Illegal::Other;
        crate::transaction::check_legal(session, self.connection, other)?;
        let state = self.state();
        self.check_writable(&state)?;
        state.file().set_transaction_free(free).map_err(write_error)
    }

    /// Writes the changes of the current record (`AdsFlushFileBuffers`); a
    /// table opened read-only is [`Error::TableReadOnly`]. On a server
    /// (`session` serves shares) the call does nothing else, as the help says of the
    /// original's, nor in a transaction, where the original's server leaves
    /// a pending append as it is (`fixtures/write/tx_append`).
    pub fn flush_buffers(&self, session: &Session) -> Result<(), Error> {
        if self.settings.options & options::READONLY != 0 {
            return Err(Error::TableReadOnly);
        }
        let mut state = self.state();
        let in_transaction = state
            .connection
            .as_ref()
            .and_then(crate::transaction::writer);
        if session.shares.is_some() || in_transaction.is_some() {
            return Ok(());
        }
        state.flush()
    }

    /// The slots of the table's records that are not deleted, in order.
    pub(crate) fn active_slots(&self) -> Result<Vec<u32>, Error> {
        let mut state = self.state();
        let mut out = Vec::new();
        for slot in 1..=state.slots() {
            if state.active(slot)? {
                out.push(slot);
            }
        }
        Ok(out)
    }

    /// A VFP AutoInc field's next value and step (see
    /// [`TableFile::autoinc_state`]).
    pub(crate) fn autoinc_state(&self, field: &Field) -> Result<Option<(u32, u8)>, Error> {
        let state = self.state();
        state.file().autoinc_state(field).map_err(storage_error)
    }

    /// The values of the fields of the record in slot `slot`, memo values
    /// read.
    pub(crate) fn slot_values(&self, slot: u32) -> Result<Vec<Value>, Error> {
        let length = self.structure.header.record_length as usize;
        let mut state = self.state();
        let bytes = state.read(slot, length)?;
        state.with_memo(|mut memo| {
            if let Some(memo) = memo.as_mut() {
                memo.decipher_record(slot);
            }
            self.fields()
                .iter()
                .map(|field| {
                    field
                        .value(&bytes, memo.as_deref_mut())
                        .map_err(|_| Error::CorruptTable)
                })
                .collect()
        })
    }

    /// The bytes of the record in slot `slot`.
    pub(crate) fn slot_record(&self, slot: u32) -> Result<Vec<u8>, Error> {
        let length = self.structure.header.record_length as usize;
        self.state().read(slot, length)
    }

    /// The value of field `index` of the record in slot `slot` when it is a
    /// General or Picture field, as its memo file keeps it (see
    /// [`Field::object_data`]); `None` without one.
    pub(crate) fn slot_object(&self, slot: u32, index: usize) -> Result<Option<Vec<u8>>, Error> {
        let length = self.structure.header.record_length as usize;
        let mut state = self.state();
        let bytes = state.read(slot, length)?;
        state.with_memo(|mut memo| {
            if let Some(memo) = memo.as_mut() {
                memo.decipher_record(slot);
            }
            self.fields()[index]
                .object_data(&bytes, memo)
                .map_err(|_| Error::CorruptMemo)
        })
    }

    /// The record slots a copy of the table takes (`AdsCopyTable`), in
    /// record order or in `order`'s: with `filter` 2 (`ADS_IGNOREFILTERS`)
    /// every record that is not hidden as deleted, the order's scope
    /// ignored; with 3 (`ADS_RESPECTSCOPES`) those in the scope; with 1
    /// (`ADS_RESPECTFILTERS`) those in the scope that the filters show.
    pub(crate) fn copy_slots(&self, order: Option<&Index>, filter: u16) -> Result<Vec<u32>, Error> {
        let sieve = self.sieve();
        let mut state = self.state();
        let mut slots = Vec::new();
        let mut take = |state: &mut State, slot: u32| -> Result<(), Error> {
            let shown = match filter {
                1 => state.visible(slot, &sieve)?,
                _ => state.active(slot)?,
            };
            if shown {
                slots.push(slot);
            }
            Ok(())
        };
        match order {
            None => {
                for slot in 1..=state.slots() {
                    take(&mut state, slot)?;
                }
            }
            Some(order) => {
                let scope = order.scope();
                if filter == 2 {
                    order.set_scope_whole(crate::index::Scope::default());
                }
                let mut next = order.first();
                while let Ok(Some(slot)) = next {
                    take(&mut state, slot)?;
                    next = order.step(true);
                }
                order.set_scope_whole(scope);
                next?;
            }
        }
        Ok(slots)
    }

    /// The raw bytes of the record in slot `slot`, and its memo fields'
    /// values (empty for none).
    pub(crate) fn slot_bytes(&self, slot: u32) -> Result<(Vec<u8>, Vec<Vec<u8>>), Error> {
        let length = self.structure.header.record_length as usize;
        let mut state = self.state();
        let bytes = state.read(slot, length)?;
        let memos = state.with_memo(|mut memo| {
            self.fields()
                .iter()
                .filter(|f| f.is_memo())
                .map(|f| {
                    f.memo_data(&bytes, memo.as_deref_mut())
                        .map(Option::unwrap_or_default)
                        .map_err(|_| Error::CorruptMemo)
                })
                .collect::<Result<Vec<_>, Error>>()
        })?;
        Ok((bytes, memos))
    }

    /// Whether the record in slot `slot` is marked deleted (a DBF record's
    /// `*`).
    pub(crate) fn slot_deleted(&self, slot: u32) -> Result<bool, Error> {
        let length = self.structure.header.record_length as usize;
        let bytes = self.state().read(slot, length)?;
        Ok(self.format() != Format::Adt && bytes.first() == Some(&b'*'))
    }
}
