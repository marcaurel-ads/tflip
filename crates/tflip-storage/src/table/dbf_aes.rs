//! Visual FoxPro tables laid out for AES: the ones an AES dictionary makes
//! and encrypts, and those a connection with `EncryptionType=AES128` or
//! `AES256` makes. The layout is in [`crate::dbf`]; what the original does
//! with them is in `tools/ace/crypto/FINDINGS.md` (`fixtures/aes/rc4tp`, `v1`
//! to `v12`).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{Error, RecordCipher, TableFile, flags};
use crate::create::AesMaterial;
use crate::dbf;
use crate::memo::MemoFile;

/// The IVs the original reserves for a table's writer past the counter it
/// stores (`fixtures/aes/rc4tp`, `v1`: the header holds the counter 1,000
/// on from the first IV a record has).
const RESERVED: u64 = 1000;

/// A memo value's block header and IV.
const VALUE_HEAD: usize = 16;

impl TableFile {
    /// Whether the table is a Visual FoxPro one laid out for AES.
    pub fn dbf_aes_layout(&self) -> bool {
        self.dbf.as_ref().is_some_and(|info| info.aes_layout)
    }

    /// The key and nonce of a Visual FoxPro table enciphered with AES, its
    /// password given.
    fn dbf_aes_cipher(&self) -> Option<(Vec<u8>, [u8; 4])> {
        match (&self.dbf, self.records.cipher()) {
            (Some(_), RecordCipher::Aes { key, nonce }) => Some((key, nonce)),
            _ => None,
        }
    }

    /// Whether the table is a VFP one armed for AES (its header names the
    /// type), whether or not its password is given.
    pub fn dbf_aes_armed(&self) -> bool {
        self.dbf
            .as_ref()
            .is_some_and(|info| dbf::aes_type(info.password_check[0]))
    }

    fn backlink(&self) -> u64 {
        u64::from(self.structure.header.data_offset) - dbf::BACKLINK as u64
    }

    /// Arms the table, laid out for AES, with `material` (its type, nonce,
    /// salt and the IV counter), `key` and the first 9 bytes of the verifier:
    /// the version byte of an encrypted table, the type and verifier in the
    /// header, and the crypto block in the back link; later writes encipher
    /// (`fixtures/aes/rc4tp`, `v1`, `v12`).
    fn arm_dbf_aes(
        &mut self,
        material: &AesMaterial,
        key: Vec<u8>,
        verifier: &[u8],
    ) -> Result<(), Error> {
        if !self.write {
            return Err(Error::ReadOnly);
        }
        if !self.dbf_aes_layout() {
            return Err(Error::NotWritable);
        }
        let mut check = [0u8; 10];
        check[0] = material.enc_type;
        check[1..].copy_from_slice(&verifier[..9]);
        let mut block = [0u8; super::DBF_AES_BLOCK];
        block[0] = 1;
        block[4..8].copy_from_slice(&material.nonce);
        block[8..16].copy_from_slice(&(material.iv_counter + RESERVED).to_le_bytes());
        block[16..48].copy_from_slice(&material.salt);
        let at = self.backlink();
        self.records.file.write_at(0, &[dbf::VFP_ENCRYPTED])?;
        self.records.file.write_at(0x0C, &check)?;
        self.records.file.write_at(at, &block)?;
        if let Some(info) = &mut self.dbf {
            info.version = dbf::VFP_ENCRYPTED;
            info.password_check = check;
        }
        self.structure.header.flags |= flags::ENCRYPTED;
        self.changes += 1;
        let cipher = RecordCipher::Aes {
            key,
            nonce: material.nonce,
        };
        let counter = Arc::new(AtomicU64::new(material.iv_counter));
        self.records.set_cipher(cipher.clone());
        self.records.set_aes_counter(Arc::clone(&counter));
        if let Some(memo) = self.memo.as_mut() {
            memo.set_cipher(cipher);
            memo.set_aes_counter(counter);
        }
        Ok(())
    }

    /// `AdsEnableEncryption` on a free VFP table laid out for AES (made on a
    /// connection whose `EncryptionType` is AES): arms it under a key of
    /// `password` and the table's own salt, leaving its records as they are
    /// until `AdsEncryptTable` (`fixtures/aes/rc4tp`, `v12`).
    pub fn enable_encryption_dbf_aes(
        &mut self,
        material: &AesMaterial,
        password: &[u8],
    ) -> Result<(), Error> {
        let key_len = material.key_len();
        let derived = crate::crypto::aes_derive(password, &material.salt, key_len);
        self.arm_dbf_aes(
            material,
            derived[..key_len].to_vec(),
            &derived[key_len..key_len + 9],
        )
    }

    /// Encrypts a table laid out for AES that is bound to the AES dictionary
    /// at `add_path`: armed under the dictionary's key, salt and verifier,
    /// then enciphered ([`TableFile::encrypt_dbf_aes`]).
    pub(super) fn encrypt_dbf_for_dictionary(&mut self, add_path: &Path) -> Result<(), Error> {
        use std::io::Read;
        let Some(RecordCipher::Aes { key, .. }) = crate::dd::keyring::cipher(add_path) else {
            return Err(Error::Encrypted);
        };
        let mut dictionary = [0u8; 0x1C0];
        std::fs::File::open(add_path)?.read_exact(&mut dictionary)?;
        let mut material = AesMaterial::generate(dictionary[0x190])?;
        material.salt = dictionary[0x1A0..0x1C0].try_into().unwrap();
        self.arm_dbf_aes(&material, key, &dictionary[0x38..0x41])?;
        self.encrypt_dbf_aes()
    }

    /// Lays the Visual FoxPro table at `path` out for AES as the original does
    /// when a dictionary encrypts it (`AdsDDAddTable` with `ENCRYPT_NEW_TABLE`,
    /// `sp_EncryptTable`; `fixtures/aes/rc4tp`, `v5`, `v11`): its records get
    /// the 8 bytes of an IV (in `_NullFlags`, a field that is made if the table
    /// had none), its memo values are stored anew behind an IV, one record
    /// after the other, and the version byte is [`dbf::AES_LAYOUT`]. Nothing
    /// is enciphered. A table laid out for AES is left as it is.
    pub fn lay_out_dbf_for_aes(path: &Path) -> Result<(), Error> {
        use crate::dbf::Flavour;
        let format = super::Format::Dbf(Flavour::Visual);
        let mut old = TableFile::open(path, format)?;
        if old.dbf_aes_layout() {
            return Ok(());
        }
        let memo_fields = old.memo_fields();
        let mut rows = Vec::new();
        for slot in 1..=old.structure.header.record_slots {
            let record = old.record(slot)?;
            let mut values = Vec::with_capacity(memo_fields.len());
            for field in &memo_fields {
                values.push(
                    field
                        .memo_data(&record, old.memo.as_mut())?
                        .unwrap_or_default(),
                );
            }
            rows.push((record, values));
        }
        let block_size = old.memo.as_ref().map(|memo| memo.block_size() as u16);
        drop(old);

        let bytes = std::fs::read(path)?;
        let header_length = usize::from(u16::from_le_bytes([bytes[8], bytes[9]]));
        let record_length = u16::from_le_bytes([bytes[10], bytes[11]]);
        let mut head = bytes[..header_length].to_vec();
        let end = (32..header_length)
            .step_by(32)
            .find(|&at| head[at] == 0x0D)
            .ok_or(Error::Corrupt("field descriptors not terminated"))?;
        let flags_at = (32..end)
            .step_by(32)
            .find(|&at| head[at + 11] == b'0' && head[at + 18] & 1 != 0);
        match flags_at {
            Some(at) => head[at + 16] += 8,
            None => {
                let mut descriptor = [0u8; 32];
                descriptor[..10].copy_from_slice(b"_NullFlags");
                descriptor[11] = b'0';
                descriptor[12..16].copy_from_slice(&u32::from(record_length).to_le_bytes());
                descriptor[16] = 8;
                descriptor[18] = 0x05;
                head.splice(end..end, descriptor);
            }
        }
        head[0] = dbf::AES_LAYOUT;
        let length = head.len() as u16;
        head[8..10].copy_from_slice(&length.to_le_bytes());
        head[10..12].copy_from_slice(&(record_length + 8).to_le_bytes());
        let mut out = head;
        for (record, _) in &rows {
            out.extend_from_slice(record);
            out.extend_from_slice(&[0; 8]);
        }
        out.push(dbf::END_OF_FILE);
        std::fs::write(path, &out)?;
        let Some(block_size) = block_size else {
            return Ok(());
        };
        let memo_path = dbf::memo_path(path, crate::memo::MemoKind::Fpt);
        std::fs::write(&memo_path, crate::create::fpt_bytes(block_size))?;
        let mut file = TableFile::open_for_writing(path, format)?;
        for (slot, (mut record, values)) in (1..).zip(rows) {
            record.extend_from_slice(&[0; 8]);
            let memo = file.memo.as_mut().ok_or(Error::NotWritable)?;
            for (field, value) in memo_fields.iter().zip(values) {
                field.encode_blank(&mut record)?;
                if !value.is_empty() {
                    field.store_memo(Some(&value), &mut record, memo)?;
                }
            }
            file.records.write(slot, &record)?;
        }
        Ok(())
    }

    /// The memo fields' fields of the table.
    fn memo_fields(&self) -> Vec<crate::field::Field> {
        self.structure
            .fields
            .iter()
            .filter(|field| field.is_memo())
            .cloned()
            .collect()
    }

    /// Enciphers every record that is not, and each memo value that is not,
    /// in place under an IV of its own: a value to the end of its blocks.
    pub(super) fn encrypt_dbf_aes(&mut self) -> Result<(), Error> {
        let Some((key, nonce)) = self.dbf_aes_cipher() else {
            return Err(Error::Encrypted);
        };
        let memo_fields = self.memo_fields();
        for slot in 1..=self.structure.header.record_slots {
            let raw = self.records.status_byte(slot)?;
            if self.enciphered(&[raw]) {
                continue;
            }
            let record = self.record(slot)?;
            if let Some(memo) = self.memo.as_mut() {
                for field in &memo_fields {
                    let block = field.memo_ref(&record).map_or(0, |r| r.block);
                    if block != 0 {
                        encipher_value(memo, block, &key, &nonce)?;
                    }
                }
            }
            let mut bytes = record;
            self.encipher_dbf_record(slot, &mut bytes)?;
            self.records.write(slot, &bytes)?;
        }
        self.changes += 1;
        self.persist_aes_counter()
    }

    /// Puts the table in the clear, as `sp_DecryptTable` and
    /// `AdsDecryptTable` do: records and memo values deciphered in place (the
    /// bytes after a value zeroed), the version byte of a table laid out for
    /// AES that is not encrypted, the type zeroed in the header, and the
    /// verifier and crypto block left (`fixtures/aes/rc4tp`, `v6`).
    pub(super) fn decrypt_dbf_aes(&mut self) -> Result<(), Error> {
        let Some((key, nonce)) = self.dbf_aes_cipher() else {
            return Err(Error::Encrypted);
        };
        let memo_fields = self.memo_fields();
        for slot in 1..=self.structure.header.record_slots {
            let record = self.record(slot)?;
            if let Some(memo) = self.memo.as_mut() {
                for field in &memo_fields {
                    let block = field.memo_ref(&record).map_or(0, |r| r.block);
                    if block != 0 {
                        decipher_value(memo, block, &key, &nonce)?;
                    }
                }
            }
            self.records.write(slot, &record)?;
        }
        self.records.file.write_at(0, &[dbf::AES_LAYOUT])?;
        self.records.file.write_at(0x0C, &[0])?;
        if let Some(info) = &mut self.dbf {
            info.version = dbf::AES_LAYOUT;
            info.password_check[0] = 0;
        }
        self.structure.header.flags &= !flags::ENCRYPTED;
        self.changes += 1;
        self.disable_encryption();
        Ok(())
    }
}

/// How long the value at `block` is, and how many bytes its blocks have after
/// its block header and IV.
fn value_room(memo: &mut MemoFile, block: u32) -> Result<(usize, usize), Error> {
    let size = memo.block_size() as usize;
    let head = memo.read_raw(block, 8)?;
    let length = u32::from_be_bytes(head[4..8].try_into().unwrap()) as usize;
    let room = (VALUE_HEAD + length).div_ceil(size) * size - VALUE_HEAD;
    Ok((length, room))
}

/// The `room` bytes after the IV of the value at `block`: what the file has,
/// zeros after.
fn value_bytes(memo: &mut MemoFile, block: u32, room: usize) -> Result<Vec<u8>, Error> {
    let at = u64::from(block) * u64::from(memo.block_size()) + VALUE_HEAD as u64;
    let have = memo.source().len()?.saturating_sub(at) as usize;
    let mut data = vec![0u8; room];
    let n = room.min(have);
    memo.source().read_at(at, &mut data[..n])?;
    Ok(data)
}

/// Enciphers the clear value at `block` in place, behind an IV of its own, to
/// the end of its blocks. One that has an IV is enciphered already.
fn encipher_value(
    memo: &mut MemoFile,
    block: u32,
    key: &[u8],
    nonce: &[u8; 4],
) -> Result<(), Error> {
    let (_, room) = value_room(memo, block)?;
    let head = memo.read_raw(block, VALUE_HEAD)?;
    if head[8..] != [0; 8] {
        return Ok(());
    }
    let mut data = value_bytes(memo, block, room)?;
    let iv = memo
        .aes_counter()
        .fetch_add(room.div_ceil(16) as u64, Ordering::Relaxed)
        .to_le_bytes();
    crate::crypto::aes_ctr_xor(key, &iv, nonce, &mut data);
    let at = u64::from(block) * u64::from(memo.block_size()) + 8;
    memo.source().write_at(at, &iv)?;
    memo.source().write_at(at + 8, &data)?;
    Ok(())
}

/// Deciphers the value at `block` in place if it has an IV: the IV zeroed,
/// the value in the clear and zeros after it.
fn decipher_value(
    memo: &mut MemoFile,
    block: u32,
    key: &[u8],
    nonce: &[u8; 4],
) -> Result<(), Error> {
    let (length, room) = value_room(memo, block)?;
    let head = memo.read_raw(block, VALUE_HEAD)?;
    let iv: [u8; 8] = head[8..].try_into().unwrap();
    if iv == [0; 8] {
        return Ok(());
    }
    let mut data = value_bytes(memo, block, room)?;
    crate::crypto::aes_ctr_xor(key, &iv, nonce, &mut data);
    data[length..].fill(0);
    let at = u64::from(block) * u64::from(memo.block_size()) + 8;
    memo.source().write_at(at, &[0; 8])?;
    memo.source().write_at(at + 8, &data)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::dbf::Flavour;
    use crate::table::{Format, TableFile};

    fn open(table: &str) -> TableFile {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/aes/rc4tp")
            .join(table);
        TableFile::open_deciphering(&path, Format::Dbf(Flavour::Visual), false, Some(b"ddpw"))
            .unwrap()
    }

    #[test]
    fn reads_the_records_and_memo_values_of_aes_tables() {
        let mut file = open("v2/tv.dbf");
        assert!(file.has_password());
        let first = file.record(1).unwrap();
        assert_eq!(&first[1..8], b"changed");
        let at = file
            .structure
            .fields
            .iter()
            .find(|f| f.name == "m")
            .unwrap()
            .offset as usize;
        let block = u32::from_le_bytes(first[at..at + 4].try_into().unwrap());
        let memo = file.memo().unwrap();
        memo.decipher_record(1);
        assert!(
            memo.read_dbf(block, false)
                .unwrap()
                .starts_with(b"a longer memo value")
        );
        // The deleted record reads as deleted, and its text deciphers.
        assert_eq!(file.record(3).unwrap()[0], b'*');
        assert_eq!(&file.record(3).unwrap()[1..2], b"x");
        // A wrong password is refused.
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/aes/rc4tp/v2/tv.dbf");
        let mut other = TableFile::open(&path, Format::Dbf(Flavour::Visual)).unwrap();
        assert!(other.supply_password(b"wrong").is_err());
    }

    #[test]
    fn reads_an_aes_256_table_and_one_in_the_clear() {
        let mut file = open("v3/ta.dbf");
        assert_eq!(&file.record(2).unwrap()[5..15], b"aaaaaaaaaa");
        let mut clear = open("v6/ta.dbf");
        assert!(!clear.has_password());
        assert_eq!(&clear.record(2).unwrap()[5..15], b"bbbbbbbbbb");
        let block = u32::from_le_bytes(clear.record(2).unwrap()[15..19].try_into().unwrap());
        assert_eq!(
            clear.memo().unwrap().read_dbf(block, false).unwrap(),
            b"two memo"
        );
    }
}
