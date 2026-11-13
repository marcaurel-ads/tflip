//! Writing a table's records: appending, changing fields, writing,
//! cancelling, deleting and recalling, the record and table locks, and
//! keeping the index orders current as records change.

use super::*;

impl Table {
    /// Checks the connection's user holds `right` on the table, else refuses
    /// with [`Error::PermissionDenied`] (5054, "the current user permissions"),
    /// as the original engine does on a dictionary that verifies access rights
    /// (probed, `tools/ace/dd/rights.c`). `access` is `u32::MAX` — every right —
    /// for a free table, the administrator, or a dictionary that does not verify.
    fn check_access(&self, right: u32) -> Result<(), Error> {
        match self.access & right != 0 {
            true => Ok(()),
            false => Err(Error::PermissionDenied),
        }
    }

    /// Why the table takes no writes, if it does not: one opened read-only
    /// ([`Error::TableReadOnly`]), or one tflip cannot write yet
    /// ([`Error::NotImplemented`]: a DBF or encrypted table, a table with an
    /// index order tflip cannot keep current). A static SQL cursor is
    /// read-only, as a live one of a read-only statement is; the others
    /// write their table (`fixtures/write/cursor_writes`).
    pub(super) fn check_writable(&self, state: &State) -> Result<(), Error> {
        if self.built {
            return Err(Error::TableReadOnly);
        }
        self.check_table_writable(state)
    }

    /// The failure of what a cursor handle does not take, the locks of a
    /// table and the rebuilds of its files (`AdsLockTable`, `AdsPackTable`,
    /// ...): [`Error::IsCursorHandle`], live or static.
    pub(crate) fn refuse_cursor(&self) -> Result<(), Error> {
        match self.statement.is_some() {
            true => Err(Error::IsCursorHandle),
            false => Ok(()),
        }
    }

    /// [`Table::check_writable`] for a set.
    fn check_table_writable(&self, state: &State) -> Result<(), Error> {
        if self.settings.options & options::READONLY != 0 {
            return Err(Error::TableReadOnly);
        }
        let file = state.file();
        // A handle without the password of an encrypted table writes nothing
        // to it (`share.json`); an AES record it may change, but not write
        // (see `State::flush`).
        match !self.keyed() && file.structure.header.encrypted() {
            true if !file.is_aes() => return Err(Error::EncryptionNotEnabled),
            true => {}
            false => file.check_writable().map_err(write_error)?,
        }
        drop(file);
        for order in state.orders.iter().filter_map(Weak::upgrade) {
            // Orders tflip cannot keep current: NTX and CDX ones, those on an
            // NChar or NVarChar field (whose keys it builds for the
            // characters it knows the ICU weights of, not for every text),
            // and those whose keys it cannot build; and a full-text one on a
            // Unicode field.
            if order.shared.is_none() {
                return Err(Error::NotImplemented);
            }
            if order.tag.full_text() {
                let unicode = self.names_unicode(&order.tag.expression);
                if unicode {
                    return Err(Error::NotImplemented);
                }
                continue;
            }
            if !order.tag.binary() {
                let row = filter::Record {
                    fields: self.fields(),
                    bytes: self.null_record(),
                    memo: None,
                    number: 0,
                };
                if let Err(Error::NotImplemented) = self.build_key(&order, row, false) {
                    return Err(Error::NotImplemented);
                }
            }
        }
        Ok(())
    }

    /// What a handle without the table's password may do to record `record`,
    /// as in the original engine (`share.json`): an enciphered record of an
    /// RC4 table it may not change or delete ([`Error::RecordEncrypted`]); a
    /// record of an AES table it may change (but not write, see
    /// `State::flush`), and deleting one does nothing, which `true` says.
    fn check_unkeyed_record(&self, state: &State, record: u32) -> Result<bool, Error> {
        if self.keyed() {
            return Ok(false);
        }
        let mut file = state.file();
        if !file.has_password() {
            return Ok(false);
        }
        if file.is_aes() {
            return Ok(file.structure.header.encrypted());
        }
        let changed = state
            .pending
            .as_ref()
            .is_some_and(|pending| pending.slot == record);
        let status = file.status_byte(record).map_err(storage_error)?;
        match !changed && file.enciphered(&[status]) {
            true => {
                Err(Error::RecordEncrypted.detailed("You must call AdsEnableEncryption first."))
            }
            false => Ok(false),
        }
    }

    /// Whether the handle has not given the password of an AES table.
    fn unkeyed_aes(&self, state: &State) -> bool {
        let file = state.file();
        !self.keyed() && file.is_aes() && file.structure.header.encrypted()
    }

    /// The record number an insert would take: the head of the free list,
    /// or a slot after the last.
    pub fn next_insert_slot(&self) -> u32 {
        self.state().file().next_slot()
    }

    /// Appends an empty record (`AdsAppendRecord`), with the next AutoInc
    /// and RowVersion values, and goes to it: the head of the free list, or
    /// a slot after the last. It is locked until it is written.
    pub fn append(&self) -> Result<(), Error> {
        self.check_access(crate::system::right::INSERT)?;
        let mut state = self.state();
        // The refusal of an append names what it needs, where the other writes
        // of a handle without the password do not, and it comes before the
        // record the handle changed is written (`fixtures/aes/api/03_writes`).
        if !self.keyed()
            && state.file().structure.header.encrypted()
            && self.settings.options & options::READONLY == 0
        {
            return Err(Error::EncryptionNotEnabled
                .detailed("Cannot append to an encrypted table unless encryption is enabled."));
        }
        state.flush()?;
        self.check_writable(&state)?;
        let shared = Arc::clone(&state.file);
        let writer = state
            .connection
            .as_ref()
            .and_then(crate::transaction::writer);
        let orders: Vec<Arc<Index>> = state.orders.iter().filter_map(Weak::upgrade).collect();
        let mut file = shared.file();
        file.begin_write().map_err(write_error)?;
        let inserted = match writer.as_ref().filter(|_| !file.transaction_free()) {
            // In a transaction, logged first and written marked.
            Some(writer) => self
                .empty_record(&mut file)
                .and_then(|(record, row_version)| {
                    let slot = super::tx::append_in_transaction(
                        &mut file,
                        &shared,
                        &state.source,
                        &orders,
                        &record,
                        writer,
                        state.owner,
                    )?;
                    Ok((slot, record, row_version))
                }),
            None => self.insert_empty(&mut file),
        };
        let ended = file.end_write().map_err(write_error);
        drop(file);
        let (slot, mut record, row_version) = inserted?;
        ended?;
        let implicit_lock = !self.exclusive() && !shared.holds_record(state.owner, slot);
        let tx = state
            .connection
            .as_deref()
            .and_then(crate::transaction::reader);
        if implicit_lock {
            // A slot off the free list may still be locked for a moment by
            // the handle that deleted the record there (a commit or rollback
            // lets its locks go once it has written); failing that, the
            // empty record goes back.
            let mut locked = shared.lock_record(state.owner, slot, tx);
            for _ in 0..200 {
                if locked.is_ok() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
                locked = shared.lock_record(state.owner, slot, tx);
            }
            if let Err(error) = locked {
                let mut file = shared.file();
                file.begin_write().map_err(write_error)?;
                let deleted = file.delete(slot, Some(&record)).map_err(write_error);
                file.end_write().map_err(write_error)?;
                deleted?;
                return Err(error);
            }
        }
        state.dropped = 0;
        state.stale_count = None;
        state.kept = None;
        state.land_in(Some(slot), true, false)?;
        // Until it is written, an ADT record has no status.
        if self.format() == Format::Adt {
            record[..5].fill(0);
        }
        state.pending = Some(Pending {
            slot,
            bytes: record,
            appended: true,
            row_version,
            changed: false,
            revealed: Vec::new(),
            nulled: Vec::new(),
            binary_memos: Vec::new(),
            deleted: false,
            kept: false,
            redeleted: false,
            implicit_lock,
            memos: Vec::new(),
            refused_by: None,
            old_keys: None,
        });
        // A dictionary's field defaults fill the record at once.
        if let Some(checks) = state.checks.clone() {
            let date_format = self.date_format();
            let dates = setters::Dates {
                format: &date_format,
                epoch: self.session_settings.epoch(),
            };
            if let Some(pending) = state.pending.as_mut() {
                checks.fill_defaults(&mut pending.bytes, pending.slot, dates);
            }
        }
        Ok(())
    }

    /// Stores an empty record with the next AutoInc and RowVersion values in
    /// `file`, locked for the write: its slot, its bytes, and its RowVersion.
    pub(super) fn insert_empty(&self, file: &mut TableFile) -> Result<(u32, Vec<u8>, u64), Error> {
        let (record, row_version) = self.empty_record(file)?;
        file.append_only(crate::maintenance::appends_only(self.path()));
        let slot = file.insert(&record).map_err(write_error);
        file.append_only(false);
        let slot = slot?;
        let today = tflip_storage::value::julian_to_ymd(crate::clock::now().local.0);
        file.set_updated(today).map_err(write_error)?;
        Ok((slot, record, row_version))
    }

    /// An empty record with the next AutoInc and RowVersion values of
    /// `file`, locked for the write, and its RowVersion.
    fn empty_record(&self, file: &mut TableFile) -> Result<(Vec<u8>, u64), Error> {
        let mut record = file.structure.null_record();
        let fields = file.structure.fields.clone();
        let mut row_version = 0;
        for field in &fields {
            let value = match field.field_type {
                FieldType::AutoInc => {
                    let next = file.take_autoinc(field).map_err(write_error)?;
                    self.last_autoinc.store(next, Ordering::Relaxed);
                    Value::Integer(i128::from(next))
                }
                FieldType::RowVersion => {
                    row_version = file.take_row_version().map_err(write_error)?;
                    Value::Integer(i128::from(row_version))
                }
                _ => continue,
            };
            encode_into(field, &value, &mut record)?;
        }
        Ok((record, row_version))
    }

    /// The pending change of the current record, started now if there is
    /// none: from the record's bytes, with the record locked for it unless
    /// this handle holds a lock already.
    pub(super) fn change<'s>(&self, state: &'s mut State) -> Result<&'s mut Pending, Error> {
        self.check_writable(state)?;
        self.start_change(state)
    }

    /// [`Table::change`] for a set (see [`Table::check_table_writable`]).
    fn change_for_set<'s>(&self, state: &'s mut State) -> Result<&'s mut Pending, Error> {
        self.check_table_writable(state)?;
        self.start_change(state)
    }

    fn start_change<'s>(&self, state: &'s mut State) -> Result<&'s mut Pending, Error> {
        let record = state.position.record.ok_or(Error::NoCurrentRecord)?;
        if state.pending.as_ref().is_some_and(|p| p.slot != record) {
            state.flush()?;
        }
        if state.pending.is_none() {
            let length = self.structure.header.record_length as usize;
            let bytes = state.read(record, length)?;
            // A deleted ADT record the table was left on (a failed
            // `AdsGotoRecord`): changed, it is written as deleted once more
            // (`fixtures/write/deleted_cursor`).
            let redeleted = self.format() == Format::Adt && state.exists_deleted(record)?;
            // A transaction keeps the lock of a record it deleted until it
            // ends, so it may change or delete it again (`fixtures/write/
            // tx_redelete`).
            if redeleted && !self.exclusive() && !state.file.holds_record(state.owner, record) {
                return Err(Error::LockFailed);
            }
            let shared = &state.file;
            let held = self.exclusive()
                || shared.holds_record(state.owner, record)
                || shared.holds_table(state.owner);
            let tx = state
                .connection
                .as_deref()
                .and_then(crate::transaction::reader);
            if !held {
                shared.lock_record(state.owner, record, tx)?;
            }
            state.pending = Some(Pending {
                slot: record,
                bytes,
                appended: false,
                row_version: 0,
                changed: false,
                revealed: Vec::new(),
                nulled: Vec::new(),
                binary_memos: Vec::new(),
                deleted: redeleted,
                kept: false,
                redeleted,
                implicit_lock: !held,
                memos: Vec::new(),
                refused_by: None,
                old_keys: None,
            });
        }
        Ok(state.pending.as_mut().expect("a pending change"))
    }

    /// Sets `field` of the current record to `value` (`AdsSetString`, ...),
    /// converted as the original engine converts it (see [`setters`]); it is
    /// written with the record. A value cut to fit is set, and then
    /// [`Error::DataTruncated`].
    pub fn set_field(&self, field: FieldRef, value: &SetValue) -> Result<(), Error> {
        let field = self.field(field)?.clone();
        let date_format = self.date_format();
        let dates = setters::Dates {
            format: &date_format,
            epoch: self.session_settings.epoch(),
        };
        let mut state = self.state();
        let record = state.position.record.ok_or(Error::NoCurrentRecord)?;
        // The values of a record the user appended are its insert's, which
        // the right to insert is enough for (`fixtures/sql/ddinsertonly`).
        let appended = state
            .pending
            .as_ref()
            .is_some_and(|pending| pending.slot == record && pending.appended);
        self.check_access(match appended {
            true => crate::system::right::INSERT,
            false => crate::system::right::UPDATE,
        })?;
        if !appended
            && self
                .not_updatable
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&field.name))
        {
            return Err(crate::ace_error::plain(crate::ace_error::sql(
                5171,
                &format!(
                    " Update permission not granted on the column. Field: \"{}\"",
                    field.name
                ),
            )));
        }
        if self.statement.is_some()
            && (self.built || self.settings.options & options::READONLY != 0)
        {
            return self.refuse_set(&mut state, record, &field, value, dates);
        }
        self.check_unkeyed_record(&state, record)?;
        self.check_table_writable(&state)?;
        let dbf = self.format() != Format::Adt;
        // On a deleted ADT record (one a failed AdsGotoRecord left the table
        // on), a table open shared refuses the set as a lock it cannot get
        // (`tools/ace/dd/constraints.c`); an exclusive one takes it, and so
        // does a transaction that deleted the record and holds its lock
        // (`fixtures/write/tx_redelete`); the record is deleted again when it
        // is written (`fixtures/write/deleted_cursor`).
        if !dbf
            && state
                .pending
                .as_ref()
                .is_none_or(|pending| pending.slot != record)
            && state.exists_deleted(record)?
            && !self.exclusive()
            && !state.file.holds_record(state.owner, record)
        {
            return Err(Error::LockFailed);
        }
        // NULL is a DBF memo field's own bytes (blanks, or a VFP NULL bit),
        // where an empty value clears its reference.
        if field.is_memo() && dbf && *value == SetValue::Null {
            if !field.nullable() {
                return Err(Error::NotVfpNullable);
            }
            let pending = self.change_for_set(&mut state)?;
            pending.memos.retain(|(offset, _)| *offset != field.offset);
            field
                .encode(&Value::Null, &mut pending.bytes)
                .map_err(|_| Error::InvalidFieldType)?;
            pending.nulled.push(field.offset);
            pending.changed = true;
            return Ok(());
        }
        // A binary value on a Clipper or FoxPro text Memo is stored as the
        // binary field type it is.
        let binary = setters::binary_memo(&field, value);
        let mut field = field;
        if let Some(field_type) = binary {
            field.field_type = field_type;
        }
        let checks = state.checks.clone();
        let index = self.fields().iter().position(|f| f.offset == field.offset);
        if field.is_memo() {
            let value = match setters::memo_value(&field, value) {
                Ok(value) => value,
                Err(error) => {
                    if setters::begins_update(&field, value, &error) {
                        self.change_for_set(&mut state)?;
                    }
                    return Err(error);
                }
            };
            // A dictionary's NOT NULL refuses an empty memo (which is NULL).
            if let (Some(checks), Some(index)) = (&checks, index) {
                checks.check_set(index, &[], value.is_none(), dates)?;
            }
            let shared = Arc::clone(&state.file);
            let orders: Vec<_> = state.orders.iter().filter_map(Weak::upgrade).collect();
            let pending = self.change_for_set(&mut state)?;
            pending.changed = true;
            pending.nulled.retain(|&offset| offset != field.offset);
            // Text makes the field text again; an empty value does not.
            match binary {
                Some(_) => pending.binary_memos.push(field.offset),
                None if value.is_some() => {
                    pending
                        .binary_memos
                        .retain(|&offset| offset != field.offset);
                }
                None => {}
            }
            // An empty value clears a DBF memo field's reference at once.
            if dbf && value.is_none() {
                pending.memos.retain(|(offset, _)| *offset != field.offset);
                field
                    .encode_blank(&mut pending.bytes)
                    .map_err(|_| Error::InvalidFieldType)?;
                return Ok(());
            }
            // Text, and binary values to compress, are written with the
            // record.
            if matches!(field.field_type, FieldType::Memo | FieldType::NMemo) || field.compressed()
            {
                pending.memos.retain(|(offset, _)| *offset != field.offset);
                pending.memos.push((field.offset, value));
                return Ok(());
            }
            // Other binary values are written at once, their reference into
            // the record too, so that cancelling the change keeps them.
            let start = usize::from(field.offset);
            let end = start + usize::from(field.size);
            let mut file = shared.file();
            // A full-text order's old words are in the value this replaces.
            if !pending.appended
                && pending.old_keys.is_none()
                && orders.iter().any(|order| order.tag.full_text())
            {
                let fields = file.structure.fields.clone();
                let before = file.record(pending.slot).map_err(storage_error)?;
                pending.old_keys = Some(order_keys_with(
                    &orders,
                    &fields,
                    Some(&before),
                    pending.slot,
                    file.memo(),
                    true,
                )?);
            }
            file.begin_write().map_err(write_error)?;
            let written = match file.memo() {
                Some(memo) => {
                    memo.decipher_record(pending.slot);
                    field.store_memo(value.as_deref(), &mut pending.bytes, memo)
                }
                .and_then(|()| file.write_part(pending.slot, start, &pending.bytes[start..end]))
                .map_err(write_error),
                None => Err(Error::CorruptTable),
            };
            let ended = file.end_write().map_err(write_error);
            return written.and(ended);
        }
        // An ADS VarChar's text past its inline part goes to the memo file at
        // once.
        if field.field_type == FieldType::LegacyVarChar
            && let Some(text) = setters::var_char_text(value)
            && text.len() > usize::from(field.size) - 2
        {
            let shared = Arc::clone(&state.file);
            let pending = self.change_for_set(&mut state)?;
            let mut bytes = pending.bytes.clone();
            let mut file = shared.file();
            file.begin_write().map_err(write_error)?;
            let written = field
                .store_var_char(&text, &mut bytes, file.memo())
                .map_err(write_error);
            let ended = file.end_write().map_err(write_error);
            // A remainder that cannot be rewritten leaves the inline part set.
            if matches!(written, Ok(()) | Err(Error::CorruptMemo)) {
                pending.bytes = bytes;
                pending.changed = true;
            }
            return written.and(ended);
        }
        // With AutoInc enforcement off (AdsDisableAutoIncEnforcement) a client
        // sets an AutoInc field in an ADT table to its own value, which is
        // stored in place of the engine's fill; with it on the set is taken and
        // dropped (as apply does), and the engine fills the field in. A NULL,
        // empty or string value keeps apply's handling either way.
        if field.field_type == FieldType::AutoInc
            && !dbf
            && !matches!(
                value,
                SetValue::Null | SetValue::Empty | SetValue::String(_)
            )
            && state
                .connection
                .as_ref()
                .is_some_and(|c| !c.autoinc_enforcement())
        {
            let pending = self.change_for_set(&mut state)?;
            let mut changed = pending.bytes.clone();
            setters::set_autoinc(&field, value, &mut changed)?;
            pending.bytes = changed;
            pending.changed = true;
            pending.revealed.retain(|&offset| offset != field.offset);
            return Ok(());
        }
        let began = state.pending.is_none();
        let pending = self.change_for_set(&mut state)?;
        let mut changed = pending.bytes.clone();
        let result = setters::apply(&field, value, dates, &mut changed);
        // A dictionary's field constraints refuse the value as it is set; the
        // field keeps what it held.
        if let (Some(checks), Some(index)) = (&checks, index)
            && matches!(result, Ok(()) | Err(Error::DataTruncated))
        {
            checks.check_set(index, &changed, field.is_null(&changed), dates)?;
        }
        if matches!(result, Ok(()) | Err(Error::DataTruncated)) {
            pending.bytes = changed;
            pending.changed = true;
            pending.revealed.retain(|&offset| offset != field.offset);
        }
        // The original starts the update before it converts a value, but a
        // refusal of some kinds leaves no update in progress, so that
        // `AdsCancelUpdate` is 5085 (`fixtures/write/set_errors`).
        if began
            && let Err(error) = &result
            && !setters::begins_update(&field, value, error)
        {
            drop(state);
            self.cancel(false)?;
        }
        result
    }

    /// The failure of a set on a cursor that cannot be written, a static one
    /// or a read-only live one: the value is converted for the field first,
    /// so what the field refuses fails as it would on a table, and a value
    /// cut to fit does not; what it takes is refused with
    /// [`Error::TableReadOnly`] (`fixtures/errors/cursor_set.json`).
    fn refuse_set(
        &self,
        state: &mut State,
        record: u32,
        field: &Field,
        value: &SetValue,
        dates: setters::Dates,
    ) -> Result<(), Error> {
        let converted = match field.is_memo() {
            true => setters::memo_value(field, value).map(drop),
            false => {
                let length = self.structure.header.record_length as usize;
                let mut scratch = state.read(record, length)?;
                setters::apply(field, value, dates, &mut scratch)
            }
        };
        match converted {
            Ok(()) | Err(Error::DataTruncated) => Err(Error::TableReadOnly),
            Err(error) => Err(error),
        }
    }

    /// The name of the unique order that refused the pending record's key,
    /// after writing it was [`Error::UniqueViolation`].
    pub fn refused_by(&self) -> Option<String> {
        self.state()
            .pending
            .as_ref()
            .and_then(|p| p.refused_by.clone())
    }

    /// Checks the record pending a write against the dictionary's field
    /// constraints and record validation (see [`crate::constraints`]), as
    /// writing it does; the triggers of a write fire after this.
    pub fn check_pending(&self) -> Result<(), Error> {
        self.state().check_pending()
    }

    /// The first constraint of the dictionary's on the table that the current
    /// record, as it is stored, breaks (see [`crate::constraints`]).
    pub(crate) fn broken_constraint(&self) -> Result<Option<crate::constraints::Broken>, Error> {
        let bytes = self.read_record()?;
        let number = self.record_number();
        Ok(self.state().stored_broken(bytes, number))
    }

    /// Whether the record pending a write was appended (an insert) or is a
    /// change of a record (an update); `None` if no record is pending.
    pub fn pending_append(&self) -> Option<bool> {
        self.state()
            .pending
            .as_ref()
            .map(|pending| pending.appended)
    }

    /// The name the table has in its data dictionary; `None` for a free
    /// table.
    pub fn dictionary_name(&self) -> Option<String> {
        self.index_files
            .as_ref()
            .and_then(|_| self.alias.get().cloned())
    }

    /// Writes the changes of the current record (`AdsWriteRecord`).
    pub fn write_record(&self) -> Result<(), Error> {
        let mut state = self.state();
        let deleted = state
            .pending
            .as_ref()
            .filter(|pending| pending.deleted && !pending.appended)
            .map(|pending| pending.slot);
        state.flush()?;
        // A written delete of an ADT record leaves the table on no record,
        // as a cancelled append does (`fixtures/write/binary_changes`).
        if let (Some(slot), Format::Adt) = (deleted, self.format()) {
            state.land_in(None, false, false)?;
            state.position = Position {
                record: None,
                bof: false,
                eof: false,
            };
            state.stranded = Some(slot);
        }
        Ok(())
    }

    /// Drops the changes of the current record (`AdsCancelUpdate`). An
    /// appended record is deleted, as the original engine does in ADT
    /// tables: it goes on the free list as an empty record, and the table is
    /// on no record.
    pub fn cancel_update(&self) -> Result<(), Error> {
        let state = self.state();
        if state.pending.is_none() {
            // On no record (after a cancelled append, a written delete, or at
            // the ends) the original says so, and that it was cancelling
            // (`fixtures/write/set_errors`).
            return Err(match state.position.record {
                None => Error::NoCurrentRecord.detailed("Cancelling update."),
                Some(_) => Error::NoPendingUpdate,
            });
        }
        drop(state);
        self.cancel(false)
    }

    /// Drops the changes of the current record as [`Table::cancel_update`]
    /// does, but an appended ADT record keeps the values it was given in its
    /// deleted slot, as the original engine leaves a record a dictionary's
    /// rules refused to an `INSERT` (`fixtures/sql/ddtables`).
    pub(crate) fn cancel_refused(&self) -> Result<(), Error> {
        self.cancel(true)
    }

    /// [`Table::cancel_update`]; `keep` says whether an appended ADT record
    /// keeps its values.
    pub(super) fn cancel(&self, keep: bool) -> Result<(), Error> {
        let mut state = self.state();
        let Some(mut pending) = state.pending.take() else {
            return Ok(());
        };
        let shared = Arc::clone(&state.file);
        let adt = self.format() == Format::Adt;
        let reader = state
            .connection
            .as_deref()
            .and_then(crate::transaction::reader);
        let held = shared.transactions().get(&pending.slot).cloned();
        if let Some(held) = held.filter(|held| pending.appended && Some(held.tx) == reader) {
            // A record its transaction appended stays the transaction's until
            // it ends, as it was appended: an ADT record deleted, a DBF record
            // blank and deleted (`fixtures/write/tx_append`).
            let orders: Vec<Arc<Index>> = state.orders.iter().filter_map(Weak::upgrade).collect();
            let writer = state
                .connection
                .as_ref()
                .and_then(crate::transaction::writer);
            let writer = writer.as_ref().map(|writer| (writer, &*state.source));
            pending.bytes = held.image;
            pending.memos.clear();
            pending.changed = true;
            pending.deleted = adt;
            if !adt {
                pending.bytes[0] = b'*';
            }
            write_pending(&shared, &orders, &mut pending, writer, state.owner, true)?;
            if adt {
                state.land_in(None, false, false)?;
                state.position = Position {
                    record: None,
                    bof: false,
                    eof: false,
                };
                state.stranded = Some(pending.slot);
            }
            return Ok(());
        }
        if pending.appended && !adt {
            // A DBF record cannot be taken back: it is left blank and
            // deleted, with its keys (but for a candidate order's, which
            // takes none of it, `fixtures/sql/ddopts`), and stays the
            // current record.
            let candidate = |order: &Arc<Index>| {
                order.kind == tflip_storage::order::Kind::Cdx
                    && order.tag.flags & tflip_storage::order::flags::CANDIDATE != 0
            };
            let orders: Vec<Arc<Index>> = state
                .orders
                .iter()
                .filter_map(Weak::upgrade)
                .filter(|order| !candidate(order))
                .collect();
            let mut file = shared.file();
            file.begin_write().map_err(write_error)?;
            let mut empty = file.structure.null_record();
            empty[0] = b'*';
            let fields = file.structure.fields.clone();
            let deleted = file
                .delete(pending.slot, Some(&empty))
                .map_err(write_error)
                .and_then(|()| {
                    order_keys_with(&orders, &fields, Some(&empty), pending.slot, None, true)
                })
                .and_then(|keys| maintain(&orders, None, Some(&keys), pending.slot, true));
            let ended = file.end_write().map_err(write_error);
            drop(file);
            deleted.and(ended)?;
            if pending.implicit_lock {
                shared.unlock_record(state.owner, pending.slot);
            }
        } else if pending.appended {
            let orders: Vec<Arc<Index>> = state.orders.iter().filter_map(Weak::upgrade).collect();
            let mut file = shared.file();
            file.begin_write().map_err(write_error)?;
            let empty = match keep {
                true => pending.bytes.clone(),
                false => file.structure.null_record(),
            };
            let deleted = file
                .delete(pending.slot, Some(&empty))
                .map_err(write_error)
                .and_then(|()| maintain(&orders, None, None, pending.slot, true));
            let ended = file.end_write().map_err(write_error);
            drop(file);
            deleted.and(ended)?;
            shared.release_record(pending.slot);
            state.land_in(None, false, false)?;
            state.position = Position {
                record: None,
                bof: false,
                eof: false,
            };
            state.stranded = Some(pending.slot);
            state.resequence()?;
        } else if pending.implicit_lock {
            shared.unlock_record(state.owner, pending.slot);
        }
        Ok(())
    }

    /// Deletes the current record (`AdsDeleteRecord`) when it is written;
    /// until then [`Table::recall_record`] undoes it. A DBF record is marked
    /// deleted, and keeps its place, values and keys.
    pub fn delete_record(&self) -> Result<(), Error> {
        let mut state = self.state();
        // A record the user appended and has not written is taken back with
        // the right it was appended with (`fixtures/sql/ddinsertonly2`).
        let appended = state.position.record.is_some_and(|record| {
            (state.pending.as_ref())
                .is_some_and(|pending| pending.slot == record && pending.appended)
        });
        self.check_access(match appended {
            true => crate::system::right::INSERT,
            false => crate::system::right::DELETE,
        })?;
        if let Some(record) = state.position.record {
            self.check_unkeyed_record(&state, record)?;
        }
        let dbf = self.format() != Format::Adt;
        let pending = self.change(&mut state)?;
        match dbf {
            true => {
                pending.bytes[0] = b'*';
                pending.changed = true;
            }
            false => pending.deleted = true,
        }
        Ok(())
    }

    /// [`Table::delete_record`], but an ADT record keeps the values it is
    /// given in its deleted slot, as the original engine writes a record
    /// `WITH DELETE` (`fixtures/sql/merge`).
    pub(crate) fn delete_record_keeping(&self) -> Result<(), Error> {
        self.delete_record()?;
        if let Some(pending) = self.state().pending.as_mut() {
            pending.kept = true;
        }
        Ok(())
    }

    /// Undoes a delete of the current record not written yet
    /// (`AdsRecallRecord`); a written delete of an ADT record cannot be, of
    /// a DBF record it is (the record is marked active again).
    pub fn recall_record(&self) -> Result<(), Error> {
        // A read-only cursor refuses it whatever the record (the table's
        // own takes it when there is nothing to recall).
        if self.statement.is_some()
            && (self.built || self.settings.options & options::READONLY != 0)
        {
            return Err(Error::TableReadOnly);
        }
        let mut state = self.state();
        let record = state.position.record.ok_or(Error::NoCurrentRecord)?;
        if self.format() != Format::Adt {
            let length = self.structure.header.record_length as usize;
            let deleted = state.read(record, length)?.first() == Some(&b'*');
            if deleted {
                let pending = self.change(&mut state)?;
                pending.bytes[0] = b' ';
                pending.changed = true;
            }
            return Ok(());
        }
        if let Some(pending) = state.pending.as_mut() {
            pending.deleted = false;
            return Ok(());
        }
        // A handle without the password changes an AES record by recalling
        // it, though it is not deleted, as it does by deleting it: the
        // change fails when it is written (`fixtures/aes/api/09_pending`).
        if self.unkeyed_aes(&state) {
            let pending = self.change(&mut state)?;
            pending.deleted = false;
            pending.changed = true;
            return Ok(());
        }
        // A record the transaction deleted is the transaction's still: the
        // recall is a change of it, written (and committed) as one
        // (`fixtures/write/tx_free`).
        if matches!(state.seen(record), Some(Seen::Mine(_, true))) {
            let pending = self.change(&mut state)?;
            pending.deleted = false;
            pending.changed = true;
            return Ok(());
        }
        // A deleted record a failed `AdsGotoRecord` left the table on is
        // recalled at once, but only the binary orders take it back: the
        // others keep it out until a reindex (`fixtures/write/append_reuse`).
        if state.exists_deleted(record)? {
            self.check_writable(&state)?;
            // A table open shared needs the record's lock for it, which a
            // deleted record does not give (`fixtures/write/cursor_writes`).
            if !self.exclusive() {
                return Err(Error::LockFailed);
            }
            let orders: Vec<Arc<Index>> = state.orders.iter().filter_map(Weak::upgrade).collect();
            let shared = Arc::clone(&state.file);
            let mut file = shared.file();
            file.begin_write().map_err(write_error)?;
            let fields = file.structure.fields.clone();
            let recalled = file
                .recall(record)
                .map_err(write_error)
                .and_then(|()| file.record(record).map_err(storage_error))
                .and_then(|bytes| order_keys(&orders, &fields, Some(&bytes), record))
                .and_then(|keys| {
                    let (orders, keys): (Vec<_>, Vec<_>) = orders
                        .iter()
                        .cloned()
                        .zip(keys)
                        .filter(|(order, _)| order.tag.binary())
                        .unzip();
                    maintain_recalled(&orders, &keys, record)
                });
            let ended = file.end_write().map_err(write_error);
            drop(file);
            recalled.and(ended)?;
        }
        Ok(())
    }

    /// Locks record `record` (`AdsLockRecord`), 0 for the current one. In a
    /// table opened exclusively it is [`Error::TableNotShared`].
    pub fn lock_record(&self, record: u32) -> Result<(), Error> {
        if self.exclusive() {
            return Err(Error::TableNotShared);
        }
        let mut state = self.state();
        let record = match record {
            0 => state.position.record.ok_or(Error::NoCurrentRecord)?,
            record => record,
        };
        if record == 0 || record > state.slots() {
            return Err(Error::InvalidRecordNumber);
        }
        // A deleted ADT record cannot be locked, which is why an append can
        // take it though another handle tried (`fixtures/write/append_reuse`).
        if self.format() == Format::Adt && state.exists_deleted(record)? {
            return Err(Error::LockFailed);
        }
        let tx = state
            .connection
            .as_deref()
            .and_then(crate::transaction::reader);
        state.file.lock_record(state.owner, record, tx)
    }

    /// Releases the lock on record `record` (`AdsUnlockRecord`), 0 for the
    /// current one, writing its changes first.
    pub fn unlock_record(&self, record: u32) -> Result<(), Error> {
        if self.built {
            return Err(Error::TableNotShared);
        }
        let mut state = self.state();
        let record = match record {
            0 => state.position.record.ok_or(Error::NoCurrentRecord)?,
            record => record,
        };
        if state.pending.as_ref().is_some_and(|p| p.slot == record) {
            state.flush()?;
        }
        // A record a transaction wrote stays locked until it ends; unlocking
        // it succeeds, as in the original engine, but through the handle
        // that holds the lock only (`fixtures/write/tx_handles`).
        if matches!(state.seen(record), Some(Seen::Mine(..)))
            && state.file.holds_record(state.owner, record)
        {
            return Ok(());
        }
        match state.file.unlock_record(state.owner, record) {
            true => Ok(()),
            false => Err(Error::RecordNotLocked),
        }
    }

    /// Locks the table (`AdsLockTable`).
    pub fn lock_table(&self) -> Result<(), Error> {
        self.refuse_cursor()?;
        let state = self.state();
        state.file.lock_table(state.owner)
    }

    /// Releases the table lock (`AdsUnlockTable`), writing the pending
    /// changes first.
    pub fn unlock_table(&self) -> Result<(), Error> {
        self.refuse_cursor()?;
        let mut state = self.state();
        state.flush()?;
        match state.file.unlock_table(state.owner) {
            true => Ok(()),
            false => Err(Error::TableNotLocked),
        }
    }

    /// Whether this handle holds a lock on record `record`, 0 for the
    /// current one (`AdsIsRecordLocked`); the table lock does not count.
    pub fn is_record_locked(&self, record: u32) -> Result<bool, Error> {
        let state = self.state();
        let record = match record {
            0 => state.position.record.ok_or(Error::NoCurrentRecord)?,
            record => record,
        };
        Ok(state.file.holds_record(state.owner, record))
    }

    /// Whether this handle holds the table lock (`AdsIsTableLocked`).
    pub fn is_table_locked(&self) -> bool {
        let state = self.state();
        state.file.holds_table(state.owner)
    }

    /// The records this handle holds locks on (`AdsGetAllLocks`).
    pub fn locks(&self) -> Vec<u32> {
        let state = self.state();
        state.file.records_locked_by(state.owner)
    }

    /// Writes the record the handle has pending, if any.
    pub(crate) fn flush_pending(&self) -> Result<(), Error> {
        self.state().flush()
    }

    /// Replaces the current record's bytes by `record` (`AdsSetRecord`), but
    /// for its first byte; `record` must be the table's record length.
    pub fn set_record(&self, record: &[u8]) -> Result<(), Error> {
        let length = self.structure.header.record_length as usize;
        if record.len() != length || length == 0 {
            return Err(Error::InvalidRecordLength);
        }
        let mut state = self.state();
        state.position.record.ok_or(Error::NoCurrentRecord)?;
        self.check_writable(&state)?;
        let pending = self.change(&mut state)?;
        pending.bytes[1..].copy_from_slice(&record[1..]);
        pending.changed = true;
        Ok(())
    }
}

/// A record's key in an order, as a write needs it.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum OrderKey {
    /// A binary order's: a bit per record, for whether it is deleted.
    Bit,
    /// A CDX binary order's: a bit per record, set when its key is `T`.
    Flag(bool),
    /// A custom order's, which holds only the keys clients add.
    Custom(Option<Vec<u8>>),
    /// Any other order's; `None` if its condition leaves the record out.
    Key(Option<Vec<u8>>),
    /// A full-text order's: each distinct word of the record with its count
    /// (see [`super::keys::words`]); `None` where the record's memo values
    /// were not at hand to read them, which [`maintain`] refuses.
    Words(Option<Vec<(Vec<u8>, u16)>>),
    /// A full-text order that is not kept current (`NOTMAINTAINED`).
    Fixed,
}

/// The keys `record` (slot `slot`) has in `orders`; none for no record. A
/// full-text order's words are not read (see [`order_keys_with`]).
pub(super) fn order_keys(
    orders: &[Arc<Index>],
    fields: &[Field],
    record: Option<&[u8]>,
    slot: u32,
) -> Result<Vec<OrderKey>, Error> {
    order_keys_with(orders, fields, record, slot, None, false)
}

/// [`order_keys`], with the words of `record` in the full-text orders read
/// with its memo values in `memo` if `words`.
pub(super) fn order_keys_with(
    orders: &[Arc<Index>],
    fields: &[Field],
    record: Option<&[u8]>,
    slot: u32,
    mut memo: Option<&mut MemoFile>,
    words: bool,
) -> Result<Vec<OrderKey>, Error> {
    let mut keys = Vec::with_capacity(orders.len());
    for order in orders {
        if order.tag.full_text() {
            if !order.maintained_full_text()? {
                keys.push(OrderKey::Fixed);
                continue;
            }
            let found = match (record, words) {
                (Some(bytes), true) => {
                    let mut row = filter::Record {
                        fields,
                        bytes: bytes.to_vec(),
                        memo: memo.as_deref_mut(),
                        number: slot,
                    };
                    let rules = order.fts_rules()?;
                    Some(super::keys::words(&order.tag, &rules, &mut row)?)
                }
                (None, _) => Some(Vec::new()),
                (Some(_), false) => None,
            };
            keys.push(OrderKey::Words(found));
            continue;
        }
        if order.tag.binary() && order.kind == tflip_storage::order::Kind::Cdx {
            let key = match record {
                Some(bytes) => {
                    let row = filter::Record {
                        fields,
                        bytes: bytes.to_vec(),
                        memo: None,
                        number: slot,
                    };
                    order.owner().build_key(order, row, true)?
                }
                None => None,
            };
            keys.push(OrderKey::Flag(key.as_deref() == Some(b"T")));
            continue;
        }
        if order.tag.binary() {
            keys.push(OrderKey::Bit);
            continue;
        }
        let key = match record {
            Some(bytes) => {
                let row = filter::Record {
                    fields,
                    bytes: bytes.to_vec(),
                    memo: None,
                    number: slot,
                };
                order.owner().build_key(order, row, true)?
            }
            None => None,
        };
        keys.push(match order.tag.custom() {
            true => OrderKey::Custom(key),
            false => OrderKey::Key(key),
        });
    }
    Ok(keys)
}

/// The ADI file of `order`, to change.
pub(super) fn order_file(
    order: &Index,
) -> Result<(&SharedIndex, MutexGuard<'_, OrderFile>), Error> {
    let shared = order.shared.as_deref().ok_or(Error::NotImplemented)?;
    Ok((shared, shared.file()))
}

/// [`maintain`] for a record `AdsRecallRecord` or `AdsRecallAllRecords` put
/// back, whose keys go into the orders: an ADI order that is not binary does
/// not count the write in its tag header, as the original engine leaves it
/// (`fixtures/write/binary_changes`).
pub(super) fn maintain_recalled(
    orders: &[Arc<Index>],
    keys: &[OrderKey],
    slot: u32,
) -> Result<(), Error> {
    let counted = |order: &Arc<Index>| -> Result<Option<u32>, Error> {
        let (_, file) = order_file(order)?;
        Ok(match &*file {
            OrderFile::Adi(file) if !order.tag.binary() => Some(file.changes_of(&order.tag)),
            _ => None,
        })
    };
    let before = orders.iter().map(counted).collect::<Result<Vec<_>, _>>()?;
    maintain(orders, None, Some(keys), slot, true)?;
    for (order, count) in orders.iter().zip(before) {
        if let (Some(count), (_, mut file)) = (count, order_file(order)?)
            && let OrderFile::Adi(file) = &mut *file
        {
            file.set_changes_of(&order.tag, count);
        }
    }
    Ok(())
}

/// Applies the change of a record's keys to `orders`: from `old` (the
/// record's keys before, if it was in the orders) to `new` (its keys now, or
/// `None` if it was deleted), as the original engine does it order by order:
/// a binary order's bit set or cleared, a custom order's key changed only if
/// the record had one, any other order's key deleted and inserted if it
/// changed.
pub(super) fn maintain(
    orders: &[Arc<Index>],
    old: Option<&[OrderKey]>,
    new: Option<&[OrderKey]>,
    slot: u32,
    enforce_unique: bool,
) -> Result<(), Error> {
    let index_error = |_| Error::CorruptIndex;
    for (i, order) in orders.iter().enumerate() {
        let (shared, mut file) = order_file(order)?;
        let file = match &mut *file {
            OrderFile::Adi(file) => file,
            // An NTX order has a key per record, or none by its condition.
            OrderFile::Ntx(file) => {
                let key = |keys: Option<&[OrderKey]>| match keys.map(|keys| &keys[i]) {
                    Some(OrderKey::Key(key)) => key.clone(),
                    _ => None,
                };
                let (old_key, new_key) = (key(old), key(new));
                if old_key != new_key {
                    if let Some(key) = &old_key {
                        file.delete_key(key, slot).map_err(index_error)?;
                    }
                    if let Some(key) = &new_key {
                        file.insert_key(key, slot).map_err(index_error)?;
                    }
                    shared.changed();
                }
                continue;
            }
            OrderFile::Cdx(file) if order.tag.full_text() => {
                fn words(key: Option<&OrderKey>) -> Result<&[(Vec<u8>, u16)], Error> {
                    match key {
                        None => Ok(&[]),
                        Some(OrderKey::Words(Some(words))) => Ok(words),
                        Some(OrderKey::Fixed) => Ok(&[]),
                        Some(_) => Err(Error::NotImplemented),
                    }
                }
                let (before, after) = (old.map(|k| &k[i]), new.map(|k| &k[i]));
                if matches!(before, Some(OrderKey::Fixed)) || matches!(after, Some(OrderKey::Fixed))
                {
                    continue;
                }
                let changes = maintain_words(file, &order.tag, words(before)?, words(after)?, slot)
                    .map_err(index_error)?;
                if changes > 0 {
                    file.count_changes(changes as u16).map_err(index_error)?;
                    shared.changed();
                }
                continue;
            }
            OrderFile::Cdx(file) => {
                let changes = maintain_cdx(
                    file,
                    order,
                    old.map(|k| &k[i]),
                    new.map(|k| &k[i]),
                    slot,
                    enforce_unique,
                )
                .map_err(index_error)?;
                if changes > 0 {
                    file.count_changes(changes).map_err(index_error)?;
                    shared.changed();
                }
                continue;
            }
        };
        let tag = &order.tag;
        let before = old.map(|keys| &keys[i]);
        let after = new.map(|keys| &keys[i]);
        let mut changed = true;
        match (before, after) {
            (Some(OrderKey::Fixed), _) | (_, Some(OrderKey::Fixed)) => continue,
            (before, after) if tag.full_text() => {
                fn words(key: Option<&OrderKey>) -> Result<&[(Vec<u8>, u16)], Error> {
                    match key {
                        None => Ok(&[]),
                        Some(OrderKey::Words(Some(words))) => Ok(words),
                        Some(_) => Err(Error::NotImplemented),
                    }
                }
                // Each word a change adds or takes away counts in the tag
                // header (`fixtures/write/fts_rebuild`).
                let count = maintain_words(file, tag, words(before)?, words(after)?, slot)
                    .map_err(index_error)?;
                for _ in 0..count {
                    file.count_change(tag);
                }
                shared.changed();
                continue;
            }
            (_, Some(OrderKey::Bit)) if old.is_none() => {
                file.mark_deleted(tag, slot, false).map_err(index_error)?;
            }
            (Some(OrderKey::Bit), None) | (None, None) if tag.binary() => {
                file.mark_deleted(tag, slot, true).map_err(index_error)?;
            }
            (Some(OrderKey::Custom(Some(key))), after) => {
                let new_key = match after {
                    Some(OrderKey::Custom(key)) => key.as_deref(),
                    _ => None,
                };
                let has = file.has_key(tag, key, Some(slot)).map_err(index_error)?;
                changed = has && new_key != Some(key.as_slice());
                if changed {
                    file.delete_key(tag, key, slot).map_err(index_error)?;
                    if let Some(new_key) = new_key {
                        file.insert_key(tag, new_key, slot).map_err(index_error)?;
                    }
                }
            }
            (before, after) => {
                let old_key = match before {
                    Some(OrderKey::Key(key)) => key.as_deref(),
                    _ => None,
                };
                let new_key = match after {
                    Some(OrderKey::Key(key)) => key.as_deref(),
                    _ => None,
                };
                if old_key == new_key {
                    continue;
                }
                // A delete of a record an order took no key back for (one
                // recalled where the table was left, `fixtures/write/deleted_cursor`)
                // has none to remove.
                if let Some(key) = old_key
                    && (new_key.is_some()
                        || file.has_key(tag, key, Some(slot)).map_err(index_error)?)
                {
                    file.delete_key(tag, key, slot).map_err(index_error)?;
                }
                if let Some(key) = new_key {
                    file.insert_key(tag, key, slot).map_err(index_error)?;
                }
            }
        }
        if changed {
            file.count_change(tag);
        }
        shared.changed();
    }
    Ok(())
}

/// What a full-text order's file does with a record's words: the same in an
/// ADI and a CDX file.
pub(super) trait WordTree {
    fn delete_word(&mut self, tag: &Tag, word: &[u8], slot: u32) -> Result<(), StorageOrderError>;
    fn insert_word(&mut self, tag: &Tag, word: &[u8], slot: u32) -> Result<(), StorageOrderError>;
    fn insert_scored_word(
        &mut self,
        tag: &Tag,
        word: &[u8],
        slot: u32,
        score: u16,
    ) -> Result<(), StorageOrderError>;
    fn set_word_score(
        &mut self,
        tag: &Tag,
        word: &[u8],
        slot: u32,
        score: u16,
    ) -> Result<(), StorageOrderError>;
}

type Tag = tflip_storage::order::Tag;
type StorageOrderError = tflip_storage::order::Error;

impl WordTree for tflip_storage::adi::IndexFile {
    fn delete_word(&mut self, tag: &Tag, word: &[u8], slot: u32) -> Result<(), StorageOrderError> {
        self.delete_key(tag, word, slot)
    }
    fn insert_word(&mut self, tag: &Tag, word: &[u8], slot: u32) -> Result<(), StorageOrderError> {
        self.insert_key(tag, word, slot)
    }
    fn insert_scored_word(
        &mut self,
        tag: &Tag,
        word: &[u8],
        slot: u32,
        score: u16,
    ) -> Result<(), StorageOrderError> {
        self.insert_scored(tag, word, slot, score)
    }
    fn set_word_score(
        &mut self,
        tag: &Tag,
        word: &[u8],
        slot: u32,
        score: u16,
    ) -> Result<(), StorageOrderError> {
        self.set_score(tag, word, slot, score)
    }
}

impl WordTree for tflip_storage::cdx::CdxFile {
    fn delete_word(&mut self, tag: &Tag, word: &[u8], slot: u32) -> Result<(), StorageOrderError> {
        self.delete_key(tag, word, slot).map(|_| ())
    }
    fn insert_word(&mut self, tag: &Tag, word: &[u8], slot: u32) -> Result<(), StorageOrderError> {
        self.insert_key(tag, word, slot)
    }
    fn insert_scored_word(
        &mut self,
        tag: &Tag,
        word: &[u8],
        slot: u32,
        score: u16,
    ) -> Result<(), StorageOrderError> {
        self.insert_scored(tag, word, slot, score)
    }
    fn set_word_score(
        &mut self,
        tag: &Tag,
        word: &[u8],
        slot: u32,
        score: u16,
    ) -> Result<(), StorageOrderError> {
        self.set_score(tag, word, slot, score)
    }
}

/// Applies the change of a record's words in the full-text order `tag` of
/// `file`, from `old` to `new` (each sorted by its bytes): the words that
/// went deleted, the new ones inserted, and the counts that changed set, if
/// the order keeps scores. Returns how many keys went or came.
fn maintain_words(
    file: &mut impl WordTree,
    tag: &tflip_storage::order::Tag,
    old: &[(Vec<u8>, u16)],
    new: &[(Vec<u8>, u16)],
    slot: u32,
) -> Result<u32, tflip_storage::order::Error> {
    let mut changes = 0;
    for (word, _) in old {
        if !new.iter().any(|(w, _)| w == word) {
            file.delete_word(tag, word, slot)?;
            changes += 1;
        }
    }
    for (word, count) in new {
        match old.iter().find(|(w, _)| w == word) {
            None if tag.keeps_score => file.insert_scored_word(tag, word, slot, *count)?,
            None => file.insert_word(tag, word, slot)?,
            Some((_, before)) if before != count && tag.keeps_score => {
                file.set_word_score(tag, word, slot, *count)?;
                continue;
            }
            Some(_) => continue,
        }
        changes += 1;
    }
    Ok(changes)
}

/// Applies the change of a record's key in the CDX order `order` from
/// `before` (none for a new record) to `after`, as the original engine does:
/// a binary order's bit set or cleared (a new record's added); any other
/// order's old key deleted and the new one inserted, but an xBase UNIQUE
/// order's only if no record has it. Returns what the file's change counter
/// counts: a key inserted, or a new record's bit, or an order whose key or
/// bit a change changed (also when xBase UNIQUE leaves it).
pub(super) fn maintain_cdx(
    file: &mut tflip_storage::cdx::CdxFile,
    order: &Index,
    before: Option<&OrderKey>,
    after: Option<&OrderKey>,
    slot: u32,
    enforce_unique: bool,
) -> Result<u16, tflip_storage::order::Error> {
    let tag = &order.tag;
    if tag.binary() {
        let flag = |key: Option<&OrderKey>| matches!(key, Some(OrderKey::Flag(true)));
        let (old, new) = (flag(before), flag(after));
        if before.is_none() {
            file.set_bit(tag, slot, new)?;
            return Ok(1);
        }
        if old == new {
            return Ok(0);
        }
        file.set_bit(tag, slot, new)?;
        return Ok(1);
    }
    let key = |key: Option<&OrderKey>| match key {
        Some(OrderKey::Key(key)) => key.clone(),
        _ => None,
    };
    let (old, new) = (key(before), key(after));
    if before.is_some() && old == new {
        return Ok(0);
    }
    if let Some(old) = &old {
        file.delete_key(tag, old, slot)?;
    }
    let Some(new) = &new else {
        return Ok(u16::from(before.is_some()));
    };
    // An xBase UNIQUE order keeps the first key of a value (first wins, always);
    // a CANDIDATE order is a true unique constraint, which refuses a duplicate —
    // unless unique enforcement is off, when the duplicate key is added.
    let candidate = tag.flags & tflip_storage::order::flags::CANDIDATE != 0;
    let first_wins = tag.unique() && !candidate;
    let refuse_dup = candidate && enforce_unique;
    if !((first_wins || refuse_dup) && file.has_key(tag, new, None)?) {
        file.insert_key(tag, new, slot)?;
    }
    Ok(1)
}

/// Writes `pending` to the table's file `shared`, keeping `orders` current:
/// with a new RowVersion if it was changed and not appended, the time of the
/// write in its ModTime fields, its memo values; or deletes it. A key a
/// unique order has for another record is [`Error::UniqueViolation`], and
/// nothing is written.
///
/// In the transaction `writer` (the handle `owner`'s, on the table its log
/// knows as the source given with it), unless the table is
/// transaction-free, the record is written as the transaction holds it (see
/// the module `tx`), and its lock is kept until the transaction ends:
/// returns whether it was.
pub(super) fn write_pending(
    shared: &Arc<Shared>,
    orders: &[Arc<Index>],
    pending: &mut Pending,
    writer: Option<(&crate::transaction::Writer, &crate::txlog::Source)>,
    owner: Owner,
    enforce_unique: bool,
) -> Result<bool, Error> {
    if !pending.deleted && !pending.changed && !pending.appended {
        return Ok(false);
    }
    let mut file = shared.file();
    let writer = writer.filter(|_| !file.transaction_free());
    file.begin_write().map_err(write_error)?;
    // In a transaction the values are written at the commit, which moves them.
    let moving =
        writer.is_none() && !pending.memos.is_empty() && shared.outdate_others(owner, pending.slot);
    if let Some(memo) = file.memo() {
        memo.set_moving(moving);
    }
    let result = match writer {
        Some((writer, source)) => super::tx::write_in_transaction(
            &mut file,
            shared,
            source,
            orders,
            pending,
            writer,
            owner,
            enforce_unique,
        ),
        None => write_locked(&mut file, orders, pending, enforce_unique),
    };
    if let Some(memo) = file.memo() {
        memo.set_moving(false);
    }
    let ended = file.end_write().map_err(write_error);
    drop(file);
    if result.is_ok() && pending.deleted && writer.is_none() {
        shared.release_record(pending.slot);
    }
    result.and(ended).map(|()| writer.is_some())
}

/// [`write_pending`], with the table's file locked for the write.
pub(super) fn write_locked(
    file: &mut TableFile,
    orders: &[Arc<Index>],
    pending: &mut Pending,
    enforce_unique: bool,
) -> Result<(), Error> {
    let slot = pending.slot;
    let fields = file.structure.fields.clone();
    let before = match pending.appended {
        true => None,
        false => Some(file.record(slot).map_err(storage_error)?),
    };
    let old = match pending.old_keys.take() {
        Some(old) => old,
        None => order_keys_with(orders, &fields, before.as_deref(), slot, file.memo(), true)?,
    };
    // The keys of a record deleted already are not in the orders: a binary
    // order's bit is set again, and the others have nothing to delete.
    let old = (before.is_some() && !pending.redeleted).then_some(&old[..]);
    if pending.deleted {
        // The values a client set before or after the delete stay in the
        // deleted record (`fixtures/write/binary_changes`); a table with memo
        // fields frees the memo values it had, and keeps the others' old
        // values (`fixtures/write/memos`).
        let set = pending.changed && fields.iter().all(|field| !field.is_memo());
        let mut kept = (pending.kept || set).then(|| pending.bytes.clone());
        // `WITH DELETE` keeps the memo values' places in the record, as a
        // delete does, but frees their blocks too; the values it was given
        // are not written. A memo set to NULL or empty has its place cleared
        // instead, and its block is not freed (`fixtures/sql/updfrom`).
        if let (true, Some(before), Some(kept)) = (pending.kept, &before, kept.as_mut()) {
            let cleared = |field: &Field| {
                pending
                    .memos
                    .iter()
                    .rfind(|(offset, _)| *offset == field.offset)
                    .is_some_and(|(_, value)| value.as_ref().is_none_or(|v| v.is_empty()))
            };
            let mut freed = before.clone();
            for field in fields.iter().filter(|f| f.is_memo()) {
                let range = usize::from(field.offset)..usize::from(field.offset + field.size);
                if cleared(field) {
                    kept[range.clone()].fill(0);
                    freed[range].fill(0);
                }
            }
            super::tx::free_memos(file, &freed, None)?;
        }
        file.delete(slot, kept.as_deref()).map_err(write_error)?;
        return maintain(orders, old, None, slot, enforce_unique);
    }
    let now = crate::clock::now().local;
    file.set_updated(tflip_storage::value::julian_to_ymd(now.0))
        .map_err(write_error)?;
    for field in &fields {
        let value = match field.field_type {
            FieldType::RowVersion if pending.appended => {
                Value::Integer(i128::from(pending.row_version))
            }
            FieldType::RowVersion => {
                let version = file.take_row_version().map_err(write_error)?;
                pending.row_version = version;
                Value::Integer(i128::from(version))
            }
            FieldType::ModTime => Value::TimeStamp(now.0, now.1),
            _ => continue,
        };
        encode_into(field, &value, &mut pending.bytes)?;
    }
    let new = order_keys(orders, &fields, Some(&pending.bytes), slot)?;
    // With unique enforcement off (AdsDisableUniqueEnforcement) a write adds the
    // key an order already has instead of refusing it, so the refusal is skipped
    // and the duplicate falls through to `maintain`.
    for (i, order) in orders.iter().enumerate().filter(|_| enforce_unique) {
        // An ADI unique order and a CDX candidate refuse a key another record
        // has; an xBase UNIQUE order keeps the first key of a value, refusing
        // none.
        let candidate = order.tag.flags & tflip_storage::order::flags::CANDIDATE != 0;
        let refuses = match order.kind {
            tflip_storage::order::Kind::Adi => order.tag.unique(),
            tflip_storage::order::Kind::Cdx => candidate,
            tflip_storage::order::Kind::Ntx => false,
        };
        let (OrderKey::Key(Some(key)), true) = (&new[i], refuses) else {
            continue;
        };
        if matches!(old.map(|old| &old[i]), Some(OrderKey::Key(Some(old))) if old == key) {
            continue;
        }
        let (_, mut index) = order_file(order)?;
        let has = match &mut *index {
            OrderFile::Adi(index) => index.has_key(&order.tag, key, None),
            OrderFile::Cdx(index) => index.has_key(&order.tag, key, None),
            OrderFile::Ntx(_) => continue,
        };
        if has.map_err(|_| Error::CorruptIndex)? {
            pending.refused_by = Some(order.tag.name.clone());
            return Err(Error::UniqueViolation(order.unique_name()));
        }
    }
    // A DBF table's text memos are written in field order, an ADT table's
    // in the order they were set.
    if file.structure.header.format != Format::Adt {
        pending.memos.sort_by_key(|(offset, _)| *offset);
    }
    for (offset, value) in &pending.memos {
        let Some(field) = fields.iter().find(|f| f.offset == *offset) else {
            continue;
        };
        let memo = file.memo().ok_or(Error::CorruptTable)?;
        // An encrypted table's memo value is enciphered at the record number.
        memo.decipher_record(slot);
        field
            .store_memo(value.as_deref(), &mut pending.bytes, memo)
            .map_err(write_error)?;
    }
    file.write(slot, &pending.bytes).map_err(write_error)?;
    // A full-text order's words are read once the memo values are written.
    let mut new = new;
    if new.iter().any(|key| matches!(key, OrderKey::Words(None))) {
        let words = order_keys_with(
            orders,
            &fields,
            Some(&pending.bytes),
            slot,
            file.memo(),
            true,
        )?;
        for (key, words) in new.iter_mut().zip(words) {
            if matches!(key, OrderKey::Words(None)) {
                *key = words;
            }
        }
    }
    // Keep the table's next AutoInc value past any a client set itself with
    // enforcement off, so the engine's fills do not collide with it.
    for field in fields.iter().filter(|f| f.field_type == FieldType::AutoInc) {
        let offset = usize::from(field.offset);
        if let Some(bytes) = pending.bytes.get(offset..offset + 4) {
            let value = u32::from_le_bytes(bytes.try_into().unwrap());
            file.raise_next_autoinc(value.saturating_add(1))
                .map_err(write_error)?;
        }
    }
    maintain(orders, old, Some(&new), slot, enforce_unique)
}

/// What a failed write is for the client.
pub(super) fn write_error(error: StorageError) -> Error {
    match error {
        StorageError::ReadOnly => Error::TableReadOnly,
        StorageError::CorruptMemo => Error::CorruptMemo,
        StorageError::NotWritable => Error::NotImplemented,
        error => storage_error(error),
    }
}

/// Writes `value` into `field`'s bytes in `record`.
pub(super) fn encode_into(field: &Field, value: &Value, record: &mut [u8]) -> Result<(), Error> {
    field
        .encode(value, record)
        .map_err(|_| Error::InvalidFieldType)
}
