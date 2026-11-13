//! Writing in a transaction, as the original's server does it
//! (`docs/TRANSACTIONS.md`): a record the transaction changes or deletes is
//! only marked in the file, its new image held until the commit; one it
//! appends is written blank and marked; memo values are written at once,
//! and the orders get the new keys beside the old ones. The commit writes
//! the images and takes the old keys out; a rollback puts the records back
//! as they were, deletes the appended ones and takes the new keys out.
//! Full-text orders are the exception: they hold no word of the
//! transaction's until the commit puts them in (see
//! `transaction::replay_words`), so a rollback has nothing to take out.

use super::*;
use crate::shared::{Earlier, TxRecord};
use crate::transaction::{self, Writer};

/// The byte after a marked record's status: the transaction's number.
fn number(id: u64) -> u8 {
    (id % 255 + 1) as u8
}

/// [`super::order_keys`] for a transaction's records: a full-text order's
/// words are no key of the transaction's until it commits, as the original's
/// order does not hold them (`fixtures/write/fts_tx_commit`), so they are
/// [`OrderKey::Fixed`], which [`maintain`] leaves alone.
fn order_keys(
    orders: &[Arc<Index>],
    fields: &[Field],
    record: Option<&[u8]>,
    slot: u32,
) -> Result<Vec<OrderKey>, Error> {
    let mut keys = super::order_keys(orders, fields, record, slot)?;
    for key in &mut keys {
        if matches!(key, OrderKey::Words(_)) {
            *key = OrderKey::Fixed;
        }
    }
    Ok(keys)
}

/// The full-text orders among `orders` that are kept current.
fn kept_full_text(orders: &[Arc<Index>]) -> Result<Vec<Arc<Index>>, Error> {
    let mut kept = Vec::new();
    for order in orders {
        if order.tag.full_text() && order.maintained_full_text()? {
            kept.push(order.clone());
        }
    }
    Ok(kept)
}

/// The words of `old` and `new` (records' bytes, the memo values they refer
/// to in `file`) in the full-text orders among `orders`: what a commit puts
/// in them, read before the memo blocks the commit frees are.
fn word_move(
    file: &mut TableFile,
    orders: &[Arc<Index>],
    slot: u32,
    old: Option<&[u8]>,
    new: Option<&[u8]>,
) -> Result<Option<WordMove>, Error> {
    let orders = kept_full_text(orders)?;
    if orders.is_empty() {
        return Ok(None);
    }
    let fields = file.structure.fields.clone();
    let from = order_keys_with(&orders, &fields, old, slot, file.memo(), true)?;
    let to = order_keys_with(&orders, &fields, new, slot, file.memo(), true)?;
    Ok(Some(WordMove { orders, from, to }))
}

/// Puts the change of the record in slot `slot` of `shared` from the bytes
/// `old` to `new` (`None` where it is not in the orders) into the full-text
/// orders among `orders`, if the table is an ADT table (a DBF table's are
/// written as its records are committed).
pub(crate) fn commit_words(
    shared: &Arc<Shared>,
    slot: u32,
    orders: &[Arc<Index>],
    old: Option<&[u8]>,
    new: Option<&[u8]>,
) -> Result<(), Error> {
    let mut file = shared.file();
    if file.structure.header.format != Format::Adt {
        return Ok(());
    }
    file.begin_write().map_err(write_error)?;
    let moved = word_move(&mut file, orders, slot, old, new)
        .and_then(|moved| moved.map_or(Ok(()), |moved| moved.apply(slot)));
    let ended = file.end_write().map_err(write_error);
    moved.and(ended)
}

/// What [`word_move`] reads: applied to the orders once the commit has
/// written the record.
struct WordMove {
    orders: Vec<Arc<Index>>,
    from: Vec<OrderKey>,
    to: Vec<OrderKey>,
}

impl WordMove {
    fn apply(self, slot: u32) -> Result<(), Error> {
        maintain(&self.orders, Some(&self.from), Some(&self.to), slot, true)
    }
}

/// The bytes of a record the transaction `id` changed or deleted, as the
/// file holds them meanwhile: `original`, marked.
fn marked(format: Format, original: &[u8], id: u64) -> Vec<u8> {
    let mut bytes = original.to_vec();
    bytes[0] |= tflip_storage::adt::TX_MARK;
    if format == Format::Adt {
        bytes[1] = number(id);
    }
    bytes
}

/// The bytes of a record the transaction `id` appended, as the file holds
/// them meanwhile: blank, but an ADT record's status and a DBF record's
/// memo references (its memo values are written at once). An ADT record's
/// fields are empty rather than NULL: text spaces, the rest zeros (probed
/// for Integer, Character and Memo fields, `fixtures/write/tx_basic`). A
/// table open exclusively has the block numbers of the memo values written
/// at once in it (`fixtures/write/tx_memo_exclusive`), without their lengths.
fn placeholder(file: &TableFile, image: &[u8], id: u64, exclusive: bool) -> Vec<u8> {
    let mut bytes = file.structure.null_record();
    match file.structure.header.format {
        Format::Adt => {
            bytes[..5].copy_from_slice(&[tflip_storage::adt::TX_APPENDED, number(id), 0, 0, 0]);
            for field in &file.structure.fields {
                let range = usize::from(field.offset)..usize::from(field.offset + field.size);
                let text = matches!(
                    field.field_type,
                    FieldType::Character | FieldType::CiCharacter | FieldType::Numeric
                );
                bytes[range].fill(if text { b' ' } else { 0 });
                if exclusive && field.is_memo() {
                    let start = usize::from(field.offset);
                    bytes[start..start + 4].copy_from_slice(&image[start..start + 4]);
                }
            }
        }
        Format::Dbf(_) => {
            bytes[0] = tflip_storage::dbf::TX_APPENDED;
            for field in file.structure.fields.iter().filter(|f| f.is_memo()) {
                let range = usize::from(field.offset)..usize::from(field.offset + field.size);
                bytes[range.clone()].copy_from_slice(&image[range]);
            }
        }
    }
    bytes
}

/// Whether setting the memo field `field`, whose reference `image` holds, to
/// `value` needs no new block: it clears the value, or fits the blocks of the
/// one there, which `store_memo` writes in place.
fn needs_no_block(field: &Field, image: &[u8], value: Option<&[u8]>, block_size: u32) -> bool {
    let data = value.unwrap_or_default();
    if data.is_empty() {
        return true;
    }
    let Some(now) = field.memo_ref(image).filter(|now| now.block != 0) else {
        return false;
    };
    !field.compressed()
        && now.stored.div_ceil(block_size).max(1) == (data.len() as u32).div_ceil(block_size).max(1)
}

/// `keys` with those equal to `base`'s in their order left out (as no
/// key), for [`maintain`]: the keys a record's original has stay in the
/// orders until the transaction ends.
fn beyond(keys: &[OrderKey], base: Option<&[OrderKey]>) -> Vec<OrderKey> {
    keys.iter()
        .enumerate()
        .map(|(i, key)| match (key, base.map(|base| &base[i])) {
            (OrderKey::Key(_), Some(original)) if original == key => OrderKey::Key(None),
            _ => key.clone(),
        })
        .collect()
}

/// Moves a record's keys in `orders` from those of `from` to those of `to`
/// (records' bytes), the keys of `original` staying.
fn move_keys(
    orders: &[Arc<Index>],
    fields: &[Field],
    slot: u32,
    original: Option<&[u8]>,
    from: Option<&[u8]>,
    to: Option<&[u8]>,
) -> Result<(), Error> {
    key_move(orders, fields, slot, original, from, to)?.apply(orders, slot)
}

/// A change of a record's keys for [`maintain`]: its keys before and after
/// (see [`key_move`]), built before anything is written.
struct KeyMove {
    from: Option<Vec<OrderKey>>,
    to: Option<Vec<OrderKey>>,
}

impl KeyMove {
    fn apply(self, orders: &[Arc<Index>], slot: u32) -> Result<(), Error> {
        match (&self.from, &self.to) {
            (None, None) => Ok(()),
            (from, to) => maintain(orders, from.as_deref(), to.as_deref(), slot, true),
        }
    }
}

/// What [`move_keys`] changes: the keys are built here, so that one that
/// cannot be is an error before anything is written.
fn key_move(
    orders: &[Arc<Index>],
    fields: &[Field],
    slot: u32,
    original: Option<&[u8]>,
    from: Option<&[u8]>,
    to: Option<&[u8]>,
) -> Result<KeyMove, Error> {
    let keys = |bytes: Option<&[u8]>| -> Result<Option<Vec<OrderKey>>, Error> {
        bytes
            .map(|b| order_keys(orders, fields, Some(b), slot))
            .transpose()
    };
    let (original, from, to) = (keys(original)?, keys(from)?, keys(to)?);
    let from = from.map(|from| beyond(&from, original.as_deref()));
    let to = to.map(|to| beyond(&to, original.as_deref()));
    let nothing = KeyMove {
        from: None,
        to: None,
    };
    if from == to {
        return Ok(nothing);
    }
    // A record with no keys beyond its original's is in the orders as it is.
    let none = |keys: &Option<Vec<OrderKey>>| {
        keys.as_ref().is_none_or(|keys| {
            keys.iter()
                .all(|key| matches!(key, OrderKey::Key(None) | OrderKey::Fixed))
        })
    };
    Ok(match (none(&from), none(&to)) {
        (true, true) => nothing,
        (true, false) => KeyMove {
            from: from.filter(|_| original.is_some()),
            to,
        },
        (false, _) => {
            let to = to.or_else(|| Some(empty(&from)));
            KeyMove { from, to }
        }
    })
}

/// No key in any order of `like`'s shape.
fn empty(like: &Option<Vec<OrderKey>>) -> Vec<OrderKey> {
    like.as_ref().map_or_else(Vec::new, |keys| {
        keys.iter().map(|_| OrderKey::Key(None)).collect()
    })
}

/// [`write_locked`] in the transaction `writer`: the record's image held,
/// the file's record marked (or written blank if appended), its memo values
/// written, its new keys put in the orders beside its old ones.
#[allow(clippy::too_many_arguments)]
pub(super) fn write_in_transaction(
    file: &mut TableFile,
    shared: &Arc<Shared>,
    source: &crate::txlog::Source,
    orders: &[Arc<Index>],
    pending: &mut Pending,
    writer: &Writer,
    owner: Owner,
    enforce_unique: bool,
) -> Result<(), Error> {
    let slot = pending.slot;
    let fields = file.structure.fields.clone();
    let format = file.structure.header.format;
    let held = shared.transactions().get(&slot).cloned();
    let mut memos = held
        .as_ref()
        .map(|record| record.memos.clone())
        .unwrap_or_default();
    let original = match &held {
        Some(record) => record.original.clone(),
        None if pending.appended => None,
        None => Some(file.record(slot).map_err(storage_error)?),
    };
    // An appended record the transaction has not written has no keys.
    let current = held
        .as_ref()
        .filter(|record| !record.unwritten)
        .map(|record| record.image.clone())
        .or_else(|| original.clone());
    // The record as the file holds it meanwhile: marked, with the memo
    // references of values written at once.
    let mut in_file = match (&original, &held) {
        (Some(_), Some(_)) => file.record(slot).map_err(storage_error)?,
        (Some(original), None) => marked(format, original, writer.id),
        (None, _) => Vec::new(),
    };
    let deleted = pending.deleted && format == Format::Adt;
    if !deleted {
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
        check_unique(
            shared,
            writer.id,
            orders,
            &fields,
            slot,
            current.as_deref(),
            pending,
            enforce_unique,
        )?;
        if format != Format::Adt {
            pending.memos.sort_by_key(|(offset, _)| *offset);
        }
        // A record's own memo values wait for the commit where the original's
        // server holds them back: a DBF record's, but where it has no memo
        // block yet (an appended record's are written now, their references
        // in the marked record too: `fixtures/write/tx_dbf_memo`); an ADT
        // table's open exclusively, but those that need no new block, as
        // `store_memo` would write them in place or clear them
        // (`fixtures/write/tx_memo_exclusive`).
        // (A full-text order reads the values from their blocks as the writes
        // are replayed, so it has them written at once.)
        let in_place = format == Format::Adt
            && shared.opened_exclusively()
            && orders.iter().all(|order| !order.tag.full_text());
        let mut at_once = Vec::new();
        if original.is_some() && (format != Format::Adt || in_place) {
            for (offset, value) in pending.memos.drain(..) {
                let Some(field) = fields.iter().find(|f| f.offset == offset) else {
                    continue;
                };
                let waits = match format {
                    Format::Adt => {
                        let memo = file.memo().ok_or(Error::CorruptTable)?;
                        needs_no_block(field, &pending.bytes, value.as_deref(), memo.block_size())
                    }
                    _ => {
                        let blockless = field.memo_ref(&in_file).is_none_or(|memo| memo.block == 0);
                        !blockless || memos.iter().any(|(at, _)| *at == offset)
                    }
                };
                memos.retain(|(at, _)| *at != offset);
                match waits {
                    true => memos.push((offset, value)),
                    false => at_once.push((offset, value)),
                }
            }
            pending.memos = at_once;
        }
        // The memo values of the original, and of the image the transaction
        // had (which a rollback to a savepoint goes back to), stay for them:
        // a value written now goes to blocks of its own. (The DBF values
        // that wait are written as the commit writes the record.)
        let earlier: Vec<&[u8]> = original.iter().chain(&current).map(Vec::as_slice).collect();
        for (offset, _) in &pending.memos {
            let Some(field) = fields.iter().find(|f| f.offset == *offset) else {
                continue;
            };
            let range = usize::from(field.offset)..usize::from(field.offset + field.size);
            if earlier
                .iter()
                .any(|bytes| pending.bytes[range.clone()] == bytes[range.clone()])
            {
                pending.bytes[range].copy_from_slice(&field.null_bytes());
            }
        }
        for (offset, value) in &pending.memos {
            let Some(field) = fields.iter().find(|f| f.offset == *offset) else {
                continue;
            };
            let memo = file.memo().ok_or(Error::CorruptTable)?;
            memo.decipher_record(slot);
            memo.set_counting(false);
            let stored = field.store_memo(value.as_deref(), &mut pending.bytes, memo);
            memo.set_counting(true);
            stored.map_err(write_error)?;
            if original.is_some() && format != Format::Adt {
                let range = usize::from(field.offset)..usize::from(field.offset + field.size);
                in_file[range.clone()].copy_from_slice(&pending.bytes[range]);
            } else if original.is_some() && in_place {
                // Only the block number: the length waits for the commit.
                let start = usize::from(field.offset);
                in_file[start..start + 4].copy_from_slice(&pending.bytes[start..start + 4]);
            }
        }
        pending.memos.clear();
    }
    // As a delete outside a transaction does, the record keeps the values
    // set before or after it, in a table without memo fields
    // (`fixtures/write/tx_redelete`).
    let set = pending.kept || pending.changed && fields.iter().all(|field| !field.is_memo());
    let image = match deleted && !set {
        true => current.clone().unwrap_or_else(|| pending.bytes.clone()),
        false => pending.bytes.clone(),
    };
    let record = TxRecord {
        tx: writer.id,
        original: original.clone(),
        image,
        deleted,
        orders: orders.to_vec(),
        owner,
        memos,
        unwritten: false,
        by_ri: crate::ri::active(),
        sealed: crate::shared::sealed(),
    };
    let to = match deleted {
        true => original.clone(),
        false => Some(pending.bytes.clone()),
    };
    let keys = key_move(
        orders,
        &fields,
        slot,
        original.as_deref(),
        current.as_deref(),
        to.as_deref(),
    )?;
    // Logged, on disk too, before the files change.
    let before = held.as_ref().map(Earlier::of);
    let adt = file.structure.header.format == Format::Adt;
    transaction::logged(
        &writer.connection,
        shared,
        source,
        slot,
        before,
        &record,
        adt,
    )?;
    if original.is_none() {
        in_file = placeholder(file, &pending.bytes, writer.id, shared.opened_exclusively());
    }
    file.write_raw(slot, &in_file).map_err(write_error)?;
    // An appended DBF record's end-of-file byte waits for the table to close.
    if original.is_none() && format != Format::Adt {
        file.defer_end_of_file().map_err(write_error)?;
    }
    // The record is the transaction's before its keys move: if the orders
    // cannot be written, the rollback takes it back.
    shared.transactions().insert(slot, record);
    keys.apply(orders, slot)
}

/// Appends `record` (an empty record) in the transaction `writer`, for the
/// handle `owner` with `orders`: logged, then written marked and blank, as
/// the original's server does at `AdsAppendRecord` (`fixtures/write/
/// tx_append`); the record is the transaction's, without keys until it is
/// written. Returns its slot.
pub(super) fn append_in_transaction(
    file: &mut TableFile,
    shared: &Arc<Shared>,
    source: &crate::txlog::Source,
    orders: &[Arc<Index>],
    record: &[u8],
    writer: &Writer,
    owner: Owner,
) -> Result<u32, Error> {
    let slot = file.next_slot();
    let held = TxRecord {
        tx: writer.id,
        original: None,
        image: record.to_vec(),
        deleted: false,
        orders: orders.to_vec(),
        owner,
        memos: Vec::new(),
        unwritten: true,
        by_ri: crate::ri::active(),
        sealed: crate::shared::sealed(),
    };
    let adt = file.structure.header.format == Format::Adt;
    transaction::logged(&writer.connection, shared, source, slot, None, &held, adt)?;
    let blank = placeholder(file, record, writer.id, false);
    let inserted = file.insert_raw(&blank).map_err(write_error)?;
    if inserted != slot {
        return Err(Error::CorruptTable);
    }
    file.defer_end_of_file().map_err(write_error)?;
    shared.transactions().insert(slot, held);
    Ok(slot)
}

/// Puts back in a DBF table's record in slot `slot` the memo references of
/// the image `to` where the file has those of the image `from`: values a
/// transaction wrote at once, of a record it appended or where the
/// record's `original` has no block; the blocks of `from`'s values go back.
/// An ADT table open exclusively has the block numbers of its values put
/// back alike (the blocks are freed by the caller).
fn memo_refs_back(
    file: &mut TableFile,
    slot: u32,
    original: Option<&[u8]>,
    from: &[u8],
    to: &[u8],
    exclusive: bool,
) -> Result<(), Error> {
    if file.structure.header.format == Format::Adt {
        if !exclusive {
            return Ok(());
        }
        let mut in_file = file.record(slot).map_err(storage_error)?;
        let mut changed = false;
        for field in file.structure.fields.iter().filter(|f| f.is_memo()) {
            let start = usize::from(field.offset);
            if from[start..start + 4] != to[start..start + 4] {
                in_file[start..start + 4].copy_from_slice(&to[start..start + 4]);
                changed = true;
            }
        }
        if changed {
            file.write_raw(slot, &in_file).map_err(write_error)?;
        }
        return Ok(());
    }
    let fields: Vec<Field> = file
        .structure
        .fields
        .iter()
        .filter(|f| f.is_memo())
        .cloned()
        .collect();
    let mut in_file = file.record(slot).map_err(storage_error)?;
    let mut changed = false;
    for field in &fields {
        let range = usize::from(field.offset)..usize::from(field.offset + field.size);
        let waits = original
            .is_some_and(|original| field.memo_ref(original).is_some_and(|memo| memo.block != 0));
        if waits || from[range.clone()] == to[range.clone()] {
            continue;
        }
        in_file[range.clone()].copy_from_slice(&to[range]);
        changed = true;
        if let (Some(memo_ref), Some(memo)) = (field.memo_ref(from), file.memo()) {
            memo.release_at_end(memo_ref.block, memo_ref.stored)
                .map_err(write_error)?;
        }
    }
    if changed {
        file.write_raw(slot, &in_file).map_err(write_error)?;
    }
    Ok(())
}

/// [`Error::UniqueViolation`] if a unique order has the key `pending`
/// gives it for another record (one that the record's `current` bytes give
/// it does not count, nor one only a record of the transaction `tx` that
/// no longer has it holds).
#[allow(clippy::too_many_arguments)]
fn check_unique(
    shared: &Shared,
    tx: u64,
    orders: &[Arc<Index>],
    fields: &[Field],
    slot: u32,
    current: Option<&[u8]>,
    pending: &mut Pending,
    enforce_unique: bool,
) -> Result<(), Error> {
    let old = current
        .map(|bytes| order_keys(orders, fields, Some(bytes), slot))
        .transpose()?;
    let new = order_keys(orders, fields, Some(&pending.bytes), slot)?;
    for (i, order) in orders.iter().enumerate() {
        let candidate = order.tag.flags & tflip_storage::order::flags::CANDIDATE != 0;
        // With unique enforcement off, a write adds a key an order already has
        // instead of refusing it (AdsDisableUniqueEnforcement).
        let refuses = enforce_unique
            && match order.kind {
                tflip_storage::order::Kind::Adi => order.tag.unique(),
                tflip_storage::order::Kind::Cdx => candidate,
                tflip_storage::order::Kind::Ntx => false,
            };
        let (OrderKey::Key(Some(key)), true) = (&new[i], refuses) else {
            continue;
        };
        if matches!(old.as_ref().map(|old| &old[i]), Some(OrderKey::Key(Some(old))) if old == key) {
            continue;
        }
        let (_, mut index) = order_file(order)?;
        let holders = match &mut *index {
            OrderFile::Adi(index) => index.holders(&order.tag, key),
            OrderFile::Cdx(index) => index.holders(&order.tag, key),
            OrderFile::Ntx(_) => continue,
        };
        let holders = holders.map_err(|_| Error::CorruptIndex)?;
        drop(index);
        // A key the order has only for one of the transaction's records that
        // no longer has it is free for the transaction.
        let mut taken = false;
        for holder in holders.into_iter().filter(|&holder| holder != slot) {
            let mine = shared
                .transactions()
                .get(&holder)
                .filter(|r| r.tx == tx)
                .cloned();
            let stale = match mine {
                Some(record) if record.deleted => true,
                Some(record) => {
                    let keys = order_keys(orders, fields, Some(&record.image), holder)?;
                    !matches!(&keys[i], OrderKey::Key(Some(k)) if k == key)
                }
                None => false,
            };
            taken |= !stale;
        }
        if taken {
            pending.refused_by = Some(order.tag.name.clone());
            return Err(Error::UniqueViolation(order.unique_name()));
        }
    }
    Ok(())
}

/// Frees the memo blocks `bytes` refers to that `kept` does not (an ADT
/// table's; a DBF memo file keeps them).
pub(super) fn free_memos(
    file: &mut TableFile,
    bytes: &[u8],
    kept: Option<&[u8]>,
) -> Result<(), Error> {
    if file.structure.header.format != Format::Adt {
        return Ok(());
    }
    let fields: Vec<Field> = file
        .structure
        .fields
        .iter()
        .filter(|f| f.is_memo())
        .cloned()
        .collect();
    let Some(memo) = file.memo() else {
        return Ok(());
    };
    for field in fields {
        let range = usize::from(field.offset)..usize::from(field.offset + field.size);
        if kept.is_some_and(|kept| kept[range.clone()] == bytes[range.clone()]) {
            continue;
        }
        tflip_storage::adt::free_memo(&field, &bytes[range], memo).map_err(write_error)?;
    }
    Ok(())
}

/// Gives back the blocks of a DBF record's memo values in `image` that its
/// original `original` had none for (values a transaction wrote at once),
/// if they are a `.fpt`'s last.
fn release_dbf_memos(file: &mut TableFile, original: &[u8], image: &[u8]) -> Result<(), Error> {
    if file.structure.header.format == Format::Adt {
        return Ok(());
    }
    let fields: Vec<Field> = file
        .structure
        .fields
        .iter()
        .filter(|f| f.is_memo())
        .cloned()
        .collect();
    for field in fields {
        if field.memo_ref(original).is_some_and(|memo| memo.block != 0) {
            continue;
        }
        if let (Some(memo_ref), Some(memo)) = (field.memo_ref(image), file.memo()) {
            memo.release_at_end(memo_ref.block, memo_ref.stored)
                .map_err(write_error)?;
        }
    }
    Ok(())
}

/// Commits the record a transaction wrote in slot `slot` of `shared`: its
/// image written, the keys only its original had taken out, its lock
/// released.
pub(crate) fn commit_record(
    shared: &Arc<Shared>,
    slot: u32,
    between: &[Vec<u8>],
) -> Result<(), Error> {
    let mut file = shared.file();
    // Taken out under the file's lock, so that no reader finds neither it
    // nor the record written.
    let Some(record) = shared.transactions().remove(&slot) else {
        return Ok(());
    };
    if record.sealed {
        file.seal();
    }
    file.begin_write().map_err(write_error)?;
    // A recovery (transaction 0) has no other handle's image to keep: the
    // handles it opens itself stand on the first record.
    let moving =
        record.tx != 0 && !record.memos.is_empty() && shared.outdate_others(record.owner, slot);
    if let Some(memo) = file.memo() {
        memo.set_moving(moving);
    }
    let result = commit_locked(&mut file, &record, slot)
        .and_then(|()| free_between(&mut file, &record, between));
    if let Some(memo) = file.memo() {
        memo.set_moving(false);
    }
    let ended = file.end_write().map_err(write_error);
    drop(file);
    shared.unlock_record(record.owner, slot);
    if record.deleted {
        shared.release_record(slot);
    }
    shared.settle();
    result.and(ended)
}

/// Frees the memo blocks of the images the transaction gave the record
/// between its original and its end (`between`, which the log has) that none
/// of those has, as the commit of the original's server does
/// (`fixtures/write/fts_tx_memo_commit`).
fn free_between(file: &mut TableFile, record: &TxRecord, between: &[Vec<u8>]) -> Result<(), Error> {
    if file.structure.header.format != Format::Adt || between.is_empty() {
        return Ok(());
    }
    let mut kept: Vec<&[u8]> = vec![&record.image];
    kept.extend(record.original.as_deref());
    for image in between {
        for field in file.structure.fields.clone().iter().filter(|f| f.is_memo()) {
            let range = usize::from(field.offset)..usize::from(field.offset + field.size);
            if kept
                .iter()
                .any(|bytes| bytes[range.clone()] == image[range.clone()])
            {
                continue;
            }
            let memo = file.memo().ok_or(Error::CorruptTable)?;
            tflip_storage::adt::free_memo(field, &image[range], memo).map_err(write_error)?;
        }
        kept.push(image);
    }
    Ok(())
}

fn commit_locked(file: &mut TableFile, record: &TxRecord, slot: u32) -> Result<(), Error> {
    let fields = file.structure.fields.clone();
    let orders = &record.orders;
    let original = record.original.as_deref();
    let adt = file.structure.header.format == Format::Adt;
    // Each record is written before the memo blocks it no longer needs are
    // freed: a recovery after a crash in between finds it unmarked and
    // leaves it (the blocks are lost, not freed twice). An appended record
    // never written is deleted (only a recovery meets one).
    if record.deleted || record.unwritten && adt {
        // An ADT record deleted: onto the free list, its memo values freed
        // (the original's and any the transaction wrote), its keys taken out.
        // The slot keeps the last image the transaction gave the record
        // (`fixtures/write/fts_tx_big_commit`).
        let deleted = &record.image;
        file.delete(slot, Some(deleted)).map_err(write_error)?;
        free_memos(file, &record.image, original)?;
        // An appended record's keys went when it was deleted.
        let Some(original) = original else {
            return Ok(());
        };
        free_memos(file, original, None)?;
        let keys = order_keys(orders, &fields, Some(original), slot)?;
        return maintain(orders, Some(&keys), None, slot, true);
    }
    // A DBF record's words in its full-text orders before the commit writes
    // the memo values that waited, which may go over the original's blocks.
    let before = match record.unwritten || adt {
        true => None,
        false => {
            let kept = kept_full_text(orders)?;
            let from = order_keys_with(&kept, &fields, original, slot, file.memo(), true)?;
            Some((kept, from))
        }
    };
    // A DBF record's memo values the transaction set.
    let mut image = record.image.clone();
    if record.unwritten {
        image[0] = b'*';
    }
    for (offset, value) in &record.memos {
        let Some(field) = fields.iter().find(|f| f.offset == *offset) else {
            continue;
        };
        let memo = file.memo().ok_or(Error::CorruptTable)?;
        memo.decipher_record(slot);
        field
            .store_memo(value.as_deref(), &mut image, memo)
            .map_err(write_error)?;
    }
    match file.structure.header.format {
        Format::Adt => file.write(slot, &image).map_err(write_error)?,
        Format::Dbf(_) => file.write_raw(slot, &image).map_err(write_error)?,
    }
    // A DBF record's words in its full-text orders: the original's values
    // and the new ones, which the commit has written where they waited (an
    // ADT table's were put in as the transaction's writes were replayed).
    let words = match before {
        Some((kept, from)) if !kept.is_empty() => {
            let to = order_keys_with(&kept, &fields, Some(&image), slot, file.memo(), true)?;
            Some(WordMove {
                orders: kept,
                from,
                to,
            })
        }
        _ => None,
    };
    if let Some(original) = original {
        free_memos(file, original, Some(&record.image))?;
    }
    // The keys of the original that the image does not have go.
    let Some(original) = original else {
        return words.map_or(Ok(()), |words| words.apply(slot));
    };
    let old = order_keys(orders, &fields, Some(original), slot)?;
    let new = order_keys(orders, &fields, Some(&record.image), slot)?;
    let gone: Vec<OrderKey> = old
        .iter()
        .zip(&new)
        .map(|(old, new)| match old == new {
            true => OrderKey::Key(None),
            false => old.clone(),
        })
        .collect();
    let none: Vec<OrderKey> = gone.iter().map(|_| OrderKey::Key(None)).collect();
    if gone.iter().any(|key| !matches!(key, OrderKey::Key(None))) {
        maintain(orders, Some(&gone), Some(&none), slot, true)?;
    }
    words.map_or(Ok(()), |words| words.apply(slot))
}

/// Takes back what the transaction wrote to slot `slot` of `shared` since
/// it had the image `to` (and whether it had deleted the record), or all of
/// it for `None`: the record back as it was (an appended one deleted), the
/// keys and memo values it gave the record gone, its lock released.
pub(crate) fn roll_back_record(
    shared: &Arc<Shared>,
    slot: u32,
    to: Option<Earlier>,
) -> Result<(), Error> {
    let Some(record) = shared.transactions().get(&slot).cloned() else {
        return Ok(());
    };
    let mut file = shared.file();
    if record.sealed {
        file.seal();
    }
    file.begin_write().map_err(write_error)?;
    let result = roll_back_locked(&mut file, shared, &record, slot, to.clone());
    let ended = file.end_write().map_err(write_error);
    drop(file);
    if to.is_none() {
        shared.unlock_record(record.owner, slot);
        if record.original.is_none() {
            shared.release_record(slot);
        }
        shared.settle();
    }
    result.and(ended)
}

fn roll_back_locked(
    file: &mut TableFile,
    shared: &Arc<Shared>,
    record: &TxRecord,
    slot: u32,
    to: Option<Earlier>,
) -> Result<(), Error> {
    let fields = file.structure.fields.clone();
    let orders = &record.orders;
    let original = record.original.as_deref();
    let image = record.image.as_slice();
    // The keys the orders have of the record beyond its original's.
    let keyed = !record.deleted && !record.unwritten;
    if let Some(earlier) = to {
        // Back to an earlier image of the transaction's.
        free_memos(file, image, Some(&earlier.image))?;
        memo_refs_back(
            file,
            slot,
            original,
            image,
            &earlier.image,
            shared.opened_exclusively(),
        )?;
        let from = keyed.then_some(image);
        let target = match (earlier.deleted, earlier.unwritten) {
            (true, _) => original,
            (_, true) => None,
            _ => Some(earlier.image.as_slice()),
        };
        move_keys(orders, &fields, slot, original, from.or(original), target)?;
        let mut records = shared.transactions();
        if let Some(held) = records.get_mut(&slot) {
            held.image = earlier.image;
            held.deleted = earlier.deleted;
            held.unwritten = earlier.unwritten;
            held.memos = earlier.memos;
        }
        return Ok(());
    }
    shared.transactions().remove(&slot);
    // As in a commit, the record is written before its memo blocks are
    // freed.
    let from = keyed.then_some(image);
    match original {
        Some(original) => {
            move_keys(
                orders,
                &fields,
                slot,
                Some(original),
                from.or(Some(original)),
                Some(original),
            )?;
            file.write_raw(slot, original).map_err(write_error)?;
            free_memos(file, image, Some(original))?;
            release_dbf_memos(file, original, image)?;
        }
        None => {
            let mut blank = file.record(slot).map_err(storage_error)?;
            // An ADT record's keys go; a DBF record, deleted and blank, keeps
            // the keys of its blank values, as deleted DBF records keep theirs.
            let keys = from
                .map(|from| order_keys(orders, &fields, Some(from), slot))
                .transpose()?;
            match file.structure.header.format {
                Format::Adt => {
                    if let Some(keys) = keys {
                        maintain(orders, Some(&keys), None, slot, true)?;
                    }
                }
                // Taken out, then put in: the orders' change counters count
                // both (`fixtures/write/tx_dbf`).
                Format::Dbf(_) if keys.is_some() || record.unwritten => {
                    let blank_keys = order_keys(orders, &fields, Some(&blank), slot)?;
                    if let Some(keys) = keys {
                        maintain(orders, Some(&keys), None, slot, true)?;
                    }
                    maintain(orders, None, Some(&blank_keys), slot, true)?;
                }
                Format::Dbf(_) => {}
            }
            match file.structure.header.format {
                // Onto the free list, as a deleted record.
                Format::Adt => {
                    file.delete_unnoted(slot, Some(&blank))
                        .map_err(write_error)?;
                    free_memos(file, image, None)?;
                }
                // Deleted, without its memo values, whose blocks an `.fpt` takes
                // back if they are its last.
                Format::Dbf(_) => {
                    let memos: Vec<&Field> = fields.iter().filter(|f| f.is_memo()).collect();
                    for field in &memos {
                        let range =
                            usize::from(field.offset)..usize::from(field.offset + field.size);
                        blank[range].copy_from_slice(&field.null_bytes());
                    }
                    blank[0] = b'*';
                    file.write_clear(slot, &blank).map_err(write_error)?;
                    for field in memos {
                        if let (Some(memo_ref), Some(memo)) = (field.memo_ref(image), file.memo()) {
                            memo.release_at_end(memo_ref.block, memo_ref.stored)
                                .map_err(write_error)?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
