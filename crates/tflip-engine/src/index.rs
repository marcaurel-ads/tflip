//! Index orders.
//!
//! An index order is an object behind a handle, owned by the table it
//! belongs to: closing the table closes it. Opening an ADT table opens its
//! structural index, the `.adi` file with the table's base name, and hands
//! out a handle for each of its orders, in the order they were created.
//! Other index files open with `AdsOpenIndex`: `.adi` files, and the `.ntx`
//! files of Clipper tables, of one order each.

use std::cmp::Ordering;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tflip_storage::collation::weighed_cmp;
use tflip_storage::order::{self, Cursor, IndexFile, Kind, Tag};
use tflip_storage::value::Value;

use crate::transaction::Illegal;

use crate::handle::{self, Handle, Object};
use crate::seek::{self, KeySource};
use crate::setters::SetValue;
use crate::shared::SharedIndex;
use crate::table::{self, FieldRef, Table};
use crate::{Error, Session, lock};

/// An open index order.
#[derive(Debug)]
pub struct Index {
    /// The table the order belongs to.
    pub table: Handle,
    /// That table.
    owner: Arc<Table>,
    /// The index file holding the order.
    pub path: PathBuf,
    /// The index file's format.
    pub kind: Kind,
    /// The order's properties as the index file holds them (the expression
    /// and condition with fields as `F<n>`).
    pub tag: Tag,
    /// An NTX order's expression and condition as its file has them.
    written: Option<(String, String)>,
    /// How the table is deciphered, to reopen an encrypted index file; `None`
    /// for an unencrypted table. Kept here so a reopen while navigating does not
    /// re-lock the table.
    cipher: Option<tflip_storage::table::RecordCipher>,
    /// The page size of the index file, in bytes.
    pub page_size: u32,
    /// The order's cursor and the file it reads, opened when first needed.
    cursor: Mutex<Option<OrderCursor>>,
    /// An ADI file, shared by the order handles on it in the process, which
    /// writes change (see [`crate::shared`]); NTX and CDX orders, which are
    /// not written, read a file of their own.
    pub(crate) shared: Option<Arc<SharedIndex>>,
    /// The keys the order shows.
    scope: Mutex<Scope>,
    /// Whether the order is walked from its last key to its first
    /// (`AdsSetIndexDirection`).
    reversed: AtomicBool,
    /// The record a raw key is built from (`AdsInitRawKey`).
    raw_key: Mutex<Option<RawKey>>,
    /// What an SQL cursor's temporary index is besides its tag; `None` for
    /// an order of an index file.
    temporary: Option<Temporary>,
    /// A full-text order's rules, read from its file when first needed.
    fts_rules: std::sync::OnceLock<Arc<tflip_sql::fts::Rules>>,
}

/// An SQL cursor's temporary index, which has no file: the cursor shows
/// its records in its order itself, and the index only answers for it.
#[derive(Debug)]
struct Temporary {
    flags: u32,
    key_type: KeyType,
}

/// The record an order's raw key is built from, with the fields set on the
/// order (`AdsInitRawKey`, then `AdsSet*` on the order's handle).
#[derive(Debug, Clone)]
struct RawKey {
    record: Vec<u8>,
    /// The last segment of the key set: its number, and the length in the
    /// key of the text it was set to if it was set to text, with that of its
    /// primary weights (the sort key up to its first separator) for Unicode
    /// text.
    last: Option<(usize, Option<RawText>)>,
}

/// The text a raw key's last segment was set to.
#[derive(Debug, Clone, Copy)]
struct RawText {
    /// Its length in the key: the bytes of 8-bit text, the whole sort key of
    /// Unicode text.
    length: usize,
    /// For Unicode text, the length of the primary weights in the sort key.
    primary: Option<usize>,
}

/// How long a raw key is (`AdsBuildRawKey100`).
pub mod raw_key_length {
    /// Up to the end of the text given to the last segment set, or of the
    /// segment if it was not text (`ADS_GET_DEFAULT_KEY_LENGTH`).
    pub const DEFAULT: u32 = 0;
    /// Up to the end of the last segment set
    /// (`ADS_GET_PARTIAL_FULL_KEY_LENGTH`).
    pub const PARTIAL_FULL: u32 = 1;
    /// The order's key length (`ADS_GET_FULL_KEY_LENGTH`).
    pub const FULL: u32 = 2;
    /// Up to the end of the primary weights of the text given to the first
    /// segment, if it is Unicode text (`ADS_GET_PRIMARY_WEIGHT_LENGTH`).
    pub const PRIMARY_WEIGHT: u32 = 4;
}

/// A full-text order's parsing options, as `AdsGetFTSIndexInfo` reports them:
/// the indexed field's name, its character sets and word-length bounds, and the
/// option flags. The character sets are the stored strings (see
/// [`tflip_storage::adi::FtsOptions`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FtsInfo {
    pub field: String,
    pub delimiters: String,
    pub noise: String,
    pub drop: String,
    pub conditional: String,
    pub min_word: u32,
    pub max_word: u32,
    pub options: u32,
}

/// What does not change about an open index order, for a client to keep.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexInfo {
    /// The table the order belongs to.
    pub table: Handle,
    /// The order's name.
    pub name: String,
    /// The key expression, with fields by the names the table defines.
    pub expression: String,
    /// The `FOR` condition, the same way; empty if the order has none.
    pub condition: String,
    /// The length of the order's keys, in bytes.
    pub key_length: u16,
    pub descending: bool,
    pub unique: bool,
    /// Whether the order is a candidate key: an ADI unique order, or a CDX
    /// candidate order (which is not unique in the xBase sense).
    pub candidate: bool,
    /// The type of the order's keys (see [`Index::key_type`]).
    pub key_type: KeyType,
    /// The page size of the index file, in bytes.
    pub page_size: u32,
    /// The 1-based numbers of the fields of the order's segments (see
    /// [`Index::segments`]).
    pub segments: Vec<usize>,
    /// The order's options as the original engine reports them
    /// (`AdsGetIndexFlags`; see [`Index::flags`]).
    pub flags: u32,
    /// The index file, as the server names it.
    pub path: String,
    /// The name of the order's dynamic collation; empty for the static one.
    pub collation: String,
    /// Whether the order is its table's primary key in a data dictionary.
    pub primary_key: bool,
    /// A full-text order's parsing options (`AdsGetFTSIndexInfo`); `None` for an
    /// order that is not full-text.
    pub fts: Option<FtsInfo>,
}

/// The keys an order shows (`AdsSetScope`): from the first key, in index
/// order, that is not before `top` to the last that is not after `bottom`.
/// A key compares with a bound over the bound's length, so a bottom of `a`
/// takes in `abc`. In a descending order the top is the largest key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub top: Option<Vec<u8>>,
    pub bottom: Option<Vec<u8>>,
}

/// The type of an order's keys, as `AdsGetKeyType` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyType {
    /// `ADS_LOGICAL`.
    Logical,
    /// `ADS_NUMERIC`: the number types, Numeric to RowVersion.
    Numeric,
    /// `ADS_DATE`.
    Date,
    /// `ADS_STRING`: the character types, Unicode ones included.
    String,
    /// `ADS_RAW`: Raw, GUID, Time and TimeStamp, and keys of several fields.
    Raw,
}

impl KeyType {
    /// The key type of an order on an expression whose result type is `ty`, as
    /// the original engine reports it (`AdsGetKeyType`, probed with
    /// `tools/ace/expr/keytype.c`): a number is `Numeric`, a date `Date` (unlike
    /// `Type::ace_type`, which folds dates into numeric), a logical `Logical`,
    /// character text `String`, and anything else `Raw`.
    pub(crate) fn of_expression(ty: tflip_expr::Type) -> KeyType {
        match ty {
            tflip_expr::Type::Logical => KeyType::Logical,
            tflip_expr::Type::Number(_) => KeyType::Numeric,
            tflip_expr::Type::Date => KeyType::Date,
            tflip_expr::Type::String { .. } => KeyType::String,
            tflip_expr::Type::Time
            | tflip_expr::Type::TimeStamp
            | tflip_expr::Type::Binary { .. } => KeyType::Raw,
            tflip_expr::Type::Memo | tflip_expr::Type::Null => KeyType::String,
        }
    }
}

/// Where a key is relative to an order's scope, in index order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    Before,
    Inside,
    After,
}

impl Scope {
    /// Where `key` is relative to the scope of an order that is
    /// `descending` or not, whose keys are ordered by `weights` (see
    /// [`Index::weights`]) or as bytes.
    fn place(&self, key: &[u8], descending: bool, weights: Option<&[u8; 256]>) -> Place {
        let compare = |bound: &[u8]| {
            let length = bound.len().min(key.len());
            let order = weighed_cmp(&key[..length], &bound[..length], weights);
            if descending { order.reverse() } else { order }
        };
        if self
            .top
            .as_deref()
            .is_some_and(|top| compare(top) == Ordering::Less)
        {
            Place::Before
        } else if self
            .bottom
            .as_deref()
            .is_some_and(|bottom| compare(bottom) == Ordering::Greater)
        {
            Place::After
        } else {
            Place::Inside
        }
    }
}

/// A cursor in an order, with the index file it reads.
#[derive(Debug)]
struct OrderCursor {
    /// The file of an order that has no shared one.
    file: Option<IndexFile>,
    cursor: Cursor,
    /// The changes of the shared file the cursor has seen.
    changes: u64,
}

impl Index {
    /// The order as the original engine names it when it refuses a key it
    /// has for another record (see [`Error::UniqueViolation`]):
    /// `<index>:<TAG>`, the index an ADI file's name without its extension,
    /// a CDX or NTX file's with it.
    pub(crate) fn unique_name(&self) -> String {
        let file = match self.kind {
            Kind::Adi => self.path.file_stem(),
            _ => self.path.file_name(),
        };
        let file = file
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        format!("{file}:{}", self.tag.name.to_ascii_uppercase())
    }

    /// What does not change about the order while it is open.
    pub fn info(&self) -> IndexInfo {
        IndexInfo {
            table: self.table,
            name: self.tag.name.clone(),
            expression: self.expression(),
            condition: self.condition(),
            key_length: self.tag.key_length,
            // A live cursor reads an order backwards through its handle.
            descending: self.tag.descending != self.reversed(),
            unique: self.tag.unique(),
            candidate: match self.kind {
                Kind::Adi => self.tag.unique(),
                _ => self.tag.flags & order::flags::CANDIDATE != 0,
            },
            key_type: self.key_type(),
            page_size: self.page_size,
            segments: self.segments(),
            flags: self.flags(),
            path: self.path.to_string_lossy().into_owned(),
            collation: self.tag.collation.clone(),
            primary_key: self
                .owner
                .primary_key
                .as_ref()
                .is_some_and(|key| key.eq_ignore_ascii_case(&self.tag.name)),
            fts: self.fts_info(),
        }
    }

    /// A full-text order's parsing options for [`IndexInfo`]: the indexed field's
    /// name and the sets and bounds read from the `.adi`. `None` for an order
    /// that is not full-text or whose options cannot be read.
    fn fts_info(&self) -> Option<FtsInfo> {
        let options = self.fts_options().ok().flatten()?;
        let field = match self.key_source(&self.owner) {
            KeySource::Field(field) => field.name.clone(),
            _ => options.field.clone(),
        };
        Some(FtsInfo {
            field,
            delimiters: options.delimiters,
            noise: options.noise,
            drop: options.drop,
            conditional: options.conditional,
            min_word: options.min_word,
            max_word: options.max_word,
            options: options.options,
        })
    }

    /// The order's options as the original engine reports them
    /// (`AdsGetIndexFlags`): the `ADS_*` index options (`ADS_COMPOUND` for
    /// every ADI order, not for NTX ones; `ADS_UNIQUE` with `ADS_CANDIDATE`, `ADS_DESCENDING`,
    /// `ADS_CUSTOM`, `ADS_FTS_INDEX`, `ADS_BINARY_INDEX`), and two bits
    /// `ace.h` does not name for the kind of key (see
    /// [`tflip_expr::KeyKind`]): 0xC000 for a computed key, also for a
    /// conditional, full-text or binary order, and 0x8000 for `DESCEND` of a
    /// plain one.
    pub fn flags(&self) -> u32 {
        if let Some(temporary) = &self.temporary {
            return temporary.flags;
        }
        const COMPOUND: u32 = 0x2;
        const UNIQUE: u32 = 0x1 | 0x800;
        const CUSTOM: u32 = 0x4;
        const DESCENDING: u32 = 0x8;
        const FTS: u32 = 0x20;
        const BINARY: u32 = 0x1000;
        const COMPUTED: u32 = 0xC000;
        const DESCEND: u32 = 0x8000;
        let tag = &self.tag;
        let compound = match self.kind {
            Kind::Adi => true,
            Kind::Cdx => self
                .path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("cdx")),
            Kind::Ntx => false,
        };
        let mut flags = if compound { COMPOUND } else { 0 };
        // A CDX order is xBase unique (`ADS_UNIQUE`) or a candidate
        // (`ADS_CANDIDATE`).
        let (unique, unique_flags) = match self.kind {
            Kind::Cdx if tag.flags & order::flags::CANDIDATE != 0 => (true, 0x800),
            Kind::Cdx | Kind::Ntx => (tag.unique(), 0x1),
            Kind::Adi => (tag.unique(), UNIQUE),
        };
        for (on, flag) in [
            (unique, unique_flags),
            (tag.custom(), CUSTOM),
            (tag.descending, DESCENDING),
            (tag.full_text(), FTS),
            (tag.binary(), BINARY),
        ] {
            if on {
                flags |= flag;
            }
        }
        let special = !tag.condition.is_empty() || tag.full_text() || tag.binary();
        flags
            | match tflip_expr::key_kind(&tag.expression) {
                _ if special => COMPUTED,
                tflip_expr::KeyKind::Computed => COMPUTED,
                tflip_expr::KeyKind::Descend => DESCEND,
                tflip_expr::KeyKind::Plain => 0,
            }
    }

    /// The segments of the order: the 1-based numbers of the distinct fields
    /// its key expression references, in order of first appearance, as the
    /// original engine counts them (`id;UPPER(name);born` has three,
    /// `name+name` one, `RECNO()` none).
    pub fn segments(&self) -> Vec<usize> {
        segments(&self.tag.expression)
    }

    /// The key expression, with fields by the names the table defines; an
    /// NTX order's as its file has it, as the original engine reports it.
    pub fn expression(&self) -> String {
        match &self.written {
            Some((expression, _)) => expression.clone(),
            None => with_field_names(&self.tag.expression, &self.owner),
        }
    }

    /// The `FOR` condition, with fields by the names the table defines (an
    /// NTX order's as written); empty if the order has none.
    pub fn condition(&self) -> String {
        match &self.written {
            Some((_, condition)) => condition.clone(),
            None => with_field_names(&self.tag.condition, &self.owner),
        }
    }

    /// Calls `f` with the order's cursor and its index file, opening the
    /// file the first time. A file that cannot be read is
    /// [`Error::CorruptIndex`].
    fn with_cursor<R>(
        &self,
        f: impl FnOnce(&mut Cursor, &mut IndexFile) -> Result<R, order::Error>,
    ) -> Result<R, Error> {
        if self.temporary.is_some() {
            return Err(Error::NotImplemented);
        }
        let mut cursor = lock(&self.cursor);
        if let Some(shared) = &self.shared {
            let mut file = shared.file();
            let changes = shared.changes();
            let entry = cursor.get_or_insert_with(|| OrderCursor {
                file: None,
                cursor: Cursor::new(&self.tag, &file),
                changes,
            });
            // The pages the cursor read may have changed: it starts again,
            // and the moves find their record anew.
            if entry.changes != changes {
                entry.cursor = Cursor::new(&self.tag, &file);
                entry.changes = changes;
            }
            return f(&mut entry.cursor, &mut file).map_err(|_| Error::CorruptIndex);
        }
        if cursor.is_none() {
            let mut file = open_index_file(&self.path, self.cipher.as_ref())
                .map_err(|_| Error::CorruptIndex)?;
            // NTX keys are the stored bytes, ordered by the static collation
            // (the OEM one for an OEM table).
            let collations = self.owner.session_settings().collations();
            let collation = collations.table_collation(self.owner.is_oem());
            file.set_weights(collation.map(|c| c.weights));
            let cursor_ = Cursor::new(&self.tag, &file);
            *cursor = Some(OrderCursor {
                file: Some(file),
                cursor: cursor_,
                changes: 0,
            });
        }
        let OrderCursor { file, cursor, .. } = cursor.as_mut().unwrap();
        let file = file.as_mut().ok_or(Error::CorruptIndex)?;
        f(cursor, file).map_err(|_| Error::CorruptIndex)
    }

    /// The weights an NTX or CDX order's keys are ordered by, byte by byte:
    /// its table's static collation's (the OEM one for an OEM table), as
    /// its cursor weighs them; `None` where keys are ordered as bytes (ADI
    /// keys are weights already, and CDX keys other than text are bytes).
    pub(crate) fn weights(&self) -> Option<[u8; 256]> {
        if self.kind == Kind::Adi || self.kind == Kind::Cdx && self.tag.pad != b' ' {
            return None;
        }
        let collations = self.owner.session_settings().collations();
        let weights = collations.table_collation(self.owner.is_oem())?.weights;
        Some(weights).filter(|w| w.iter().enumerate().any(|(i, &b)| usize::from(b) != i))
    }

    /// The smallest key after every key that starts with `key` in index
    /// order (`descending` or not), weighed as the order's keys are (see
    /// [`seek::after`] and [`seek::before`], which it does by weight); `None`
    /// if there is none.
    pub(crate) fn past(&self, key: Vec<u8>, descending: bool) -> Option<Vec<u8>> {
        let step = |key| {
            if descending {
                seek::before(key)
            } else {
                seek::after(key)
            }
        };
        let Some(weights) = self.weights() else {
            return step(key);
        };
        // A static collation weighs each byte differently, so its weights
        // map back to bytes.
        let mut bytes = [0u8; 256];
        for (byte, &weight) in weights.iter().enumerate() {
            bytes[usize::from(weight)] = byte as u8;
        }
        let weighed = key.iter().map(|&b| weights[usize::from(b)]).collect();
        Some(
            step(weighed)?
                .iter()
                .map(|&w| bytes[usize::from(w)])
                .collect(),
        )
    }

    /// The order's scope.
    pub fn scope(&self) -> Scope {
        lock(&self.scope).clone()
    }

    /// Replaces the whole scope.
    pub(crate) fn set_scope_whole(&self, scope: Scope) {
        *lock(&self.scope) = scope;
    }

    /// Sets the scope's top (or `bottom`) to `key`.
    pub(crate) fn set_scope(&self, bottom: bool, key: Vec<u8>) {
        let mut scope = lock(&self.scope);
        *if bottom {
            &mut scope.bottom
        } else {
            &mut scope.top
        } = Some(key);
    }

    /// Clears the scope's top (or `bottom`); [`Error::NoScope`] if it is not
    /// set.
    pub fn clear_scope(&self, bottom: bool) -> Result<(), Error> {
        let mut scope = lock(&self.scope);
        let bound = if bottom {
            &mut scope.bottom
        } else {
            &mut scope.top
        };
        bound.take().map(|_| ()).ok_or(Error::NoScope)
    }

    /// The key of the scope's top (or `bottom`); [`Error::NoScope`] if it is
    /// not set.
    pub fn scope_key(&self, bottom: bool) -> Result<Vec<u8>, Error> {
        let scope = lock(&self.scope);
        let bound = if bottom { &scope.bottom } else { &scope.top };
        bound.clone().ok_or(Error::NoScope)
    }

    /// Whether the order is walked from its last key to its first.
    pub fn reversed(&self) -> bool {
        self.reversed.load(AtomicOrdering::Relaxed)
    }

    /// Walks the order from its last key to its first, or not
    /// (`AdsSetIndexDirection`): its first key, last key, steps, places and
    /// key numbers are then those of the reversed order.
    pub fn set_reversed(&self, reversed: bool) {
        self.reversed.store(reversed, AtomicOrdering::Relaxed);
    }

    /// The record of the first key in the scope, in the direction the order
    /// is walked, moving the cursor there; `None` if there is none.
    pub(crate) fn first(&self) -> Result<Option<u32>, Error> {
        if self.reversed() {
            self.index_last()
        } else {
            self.index_first()
        }
    }

    /// The record of the last key in the scope, in the direction the order
    /// is walked, moving the cursor there; `None` if there is none.
    pub(crate) fn last(&self) -> Result<Option<u32>, Error> {
        if self.reversed() {
            self.index_first()
        } else {
            self.index_last()
        }
    }

    /// The record of the next key (`forward`) or the previous one in the
    /// scope, in the direction the order is walked, moving the cursor there;
    /// `None` past the last or before the first.
    pub(crate) fn step(&self, forward: bool) -> Result<Option<u32>, Error> {
        self.index_step(forward != self.reversed())
    }

    /// Where the cursor's key is relative to the scope, in the direction the
    /// order is walked; `None` if it is not on a key.
    pub(crate) fn place(&self) -> Result<Option<Place>, Error> {
        let place = self.index_place()?;
        Ok(match (place, self.reversed()) {
            (Some(Place::Before), true) => Some(Place::After),
            (Some(Place::After), true) => Some(Place::Before),
            (place, _) => place,
        })
    }

    /// The place of the cursor's key in the order, 1-based, in the
    /// direction it is walked; `None` if it is not on a key.
    pub(crate) fn key_number(&self) -> Result<Option<u32>, Error> {
        let number = self.with_cursor(|cursor, file| cursor.key_number(file))?;
        match (number, self.reversed()) {
            (Some(number), true) => Ok(Some(self.tree_keys()? + 1 - number)),
            (number, _) => Ok(number),
        }
    }

    /// The first key of the order (or the `last`), in index order, whatever
    /// the scope; the cursor does not move. `None` if the order has none.
    pub(crate) fn end_key(&self, last: bool) -> Result<Option<Vec<u8>>, Error> {
        self.with_cursor(|cursor, file| {
            let mut end = cursor.clone();
            let key = if last {
                end.last(file)?
            } else {
                end.first(file)?
            };
            Ok(key.map(|key| key.bytes.clone()))
        })
    }

    /// The bytes of the cursor's key; `None` if it is not on a key.
    pub(crate) fn current_key(&self) -> Result<Option<Vec<u8>>, Error> {
        self.with_cursor(|cursor, _| Ok(cursor.key().map(|key| key.bytes.clone())))
    }

    /// The record of the first key in the scope, in index order, moving the
    /// cursor there; `None` if there is none.
    fn index_first(&self) -> Result<Option<u32>, Error> {
        let scope = self.scope();
        let weights = self.weights();
        let weights = weights.as_ref();
        self.with_cursor(|cursor, file| {
            let key = match &scope.top {
                Some(top) => cursor.seek(file, top, None)?,
                None => cursor.first(file)?,
            };
            Ok(key.filter(|key| {
                scope.place(&key.bytes, self.tag.descending, weights) == Place::Inside
            }))
            .map(|key| key.map(|key| key.record))
        })
    }

    /// The record of the last key in the scope, in index order, moving the
    /// cursor there; `None` if there is none.
    fn index_last(&self) -> Result<Option<u32>, Error> {
        let scope = self.scope();
        let weights = self.weights();
        let weights = weights.as_ref();
        let descending = self.tag.descending;
        self.with_cursor(|cursor, file| {
            // The first key past the bottom, in index order, and the one
            // before it.
            let past = scope
                .bottom
                .clone()
                .and_then(|bottom| self.past(bottom, descending));
            let key = match past {
                Some(past) => {
                    cursor.seek(file, &past, None)?;
                    cursor.prev(file)?
                }
                None => cursor.last(file)?,
            };
            Ok(key.filter(|key| scope.place(&key.bytes, descending, weights) == Place::Inside))
                .map(|key| key.map(|key| key.record))
        })
    }

    /// The record of the next key (`forward`) or the previous one in the
    /// scope, in index order, moving the cursor there; `None` past the last
    /// or before the first.
    fn index_step(&self, forward: bool) -> Result<Option<u32>, Error> {
        let scope = self.scope();
        let weights = self.weights();
        let weights = weights.as_ref();
        self.with_cursor(|cursor, file| {
            let key = if forward {
                cursor.next(file)?
            } else {
                cursor.prev(file)?
            };
            let key = key.filter(|key| {
                scope.place(&key.bytes, self.tag.descending, weights) == Place::Inside
            });
            Ok(key.map(|key| key.record))
        })
    }

    /// Seeks the scope's top, moving the cursor: whether a key starts with
    /// it; `None` if the scope has no top.
    pub(crate) fn seek_top(&self) -> Result<Option<bool>, Error> {
        let Some(top) = self.scope().top else {
            return Ok(None);
        };
        Ok(Some(self.seek(&top)?.is_some_and(|(_, found, _)| found)))
    }

    /// Where the cursor's key is relative to the scope, in index order;
    /// `None` if it is not on a key.
    fn index_place(&self) -> Result<Option<Place>, Error> {
        let scope = self.scope();
        let weights = self.weights();
        let weights = weights.as_ref();
        self.with_cursor(|cursor, _| {
            Ok(cursor
                .key()
                .map(|key| scope.place(&key.bytes, self.tag.descending, weights)))
        })
    }

    /// Puts the cursor on `record`'s entry, whose key is `key`, if the order
    /// has it.
    ///
    /// With `strict`, a cursor already on the record's entry counts only if
    /// its key is `key`: for the handle whose record is blank, as it has no
    /// password, the key it reads is not the entry's.
    pub(crate) fn locate_key(&self, record: u32, key: &[u8], strict: bool) -> Result<bool, Error> {
        self.with_cursor(|cursor, file| {
            if cursor
                .key()
                .is_some_and(|found| found.record == record && (!strict || found.bytes == key))
            {
                return Ok(true);
            }
            let found = cursor.seek(file, key, Some(record))?;
            Ok(found.is_some_and(|found| found.record == record && found.bytes == key))
        })
    }

    /// Puts the cursor on the key of `record`, if the order has one for it,
    /// without knowing the key: unless the cursor is already there, this
    /// goes through the keys from the first one, reading every leaf up to
    /// it. [`Table::locate_in`] seeks the record's key instead where it can
    /// build it.
    pub(crate) fn locate(&self, record: u32) -> Result<bool, Error> {
        self.with_cursor(|cursor, file| {
            if cursor.key().is_some_and(|key| key.record == record) {
                return Ok(true);
            }
            let mut key = cursor.first(file)?;
            while let Some(found) = key {
                if found.record == record {
                    return Ok(true);
                }
                key = cursor.next(file)?;
            }
            Ok(false)
        })
    }

    /// Moves the cursor to the first key, in index order, that is not
    /// before `key` (see [`Cursor::seek`]), whatever the scope: the record of
    /// that key, whether it starts with `key`, and where it is relative to
    /// the scope; `None` past the last key.
    pub(crate) fn seek(&self, key: &[u8]) -> Result<Option<(u32, bool, Place)>, Error> {
        let scope = self.scope();
        let weights = self.weights();
        let weights = weights.as_ref();
        self.with_cursor(|cursor, file| {
            let found = cursor.seek(file, key, None)?;
            Ok(found.map(|found| {
                let place = scope.place(&found.bytes, self.tag.descending, weights);
                (found.record, found.bytes.starts_with(key), place)
            }))
        })
    }

    /// Moves the cursor to the key next to where `record`'s entry, whose key
    /// is `key`, would be in the order (it has none: its condition leaves
    /// the record out, or the record is deleted), the next one `forward` in
    /// the direction the order is walked or the one before it, whatever the
    /// scope: that key's record; `None` past the last key or before the
    /// first. Entries of the same key sort by record number.
    pub(crate) fn seek_beside(
        &self,
        key: &[u8],
        record: u32,
        forward: bool,
    ) -> Result<Option<u32>, Error> {
        let after = forward != self.reversed();
        self.with_cursor(|cursor, file| {
            let found = cursor
                .seek(file, key, Some(record))?
                .map(|found| found.record);
            if after {
                return Ok(found);
            }
            // Past the last key, the one before is the last.
            Ok(cursor.prev(file)?.map(|found| found.record))
        })
    }

    /// Moves the cursor to the last key, in index order, that starts with
    /// `key`, whatever the scope: its record and where it is relative to
    /// the scope; if no key starts with `key`, whether any key comes before
    /// it.
    pub(crate) fn seek_last(&self, key: &[u8]) -> Result<Result<(u32, Place), bool>, Error> {
        let scope = self.scope();
        let weights = self.weights();
        let weights = weights.as_ref();
        let descending = self.tag.descending;
        // The first key past those that start with `key`, in index order.
        let past = self.past(key.to_vec(), descending);
        self.with_cursor(|cursor, file| {
            let last = match past {
                Some(past) if cursor.seek(file, &past, None)?.is_some() => cursor.prev(file)?,
                _ => cursor.last(file)?,
            };
            Ok(match last {
                Some(last) if last.bytes.starts_with(key) => {
                    Ok((last.record, scope.place(&last.bytes, descending, weights)))
                }
                last => Err(last.is_some()),
            })
        })
    }

    /// The records of the keys from `low` to `high`, both included and
    /// compared over their whole length, in index order, whatever the
    /// scope; without a bound, from the smallest key or to the largest.
    /// Keys an NTX or CDX order weighs by a static collation compare by
    /// their weights.
    pub(crate) fn records_between(
        &self,
        low: Option<&[u8]>,
        high: Option<&[u8]>,
    ) -> Result<Vec<u32>, Error> {
        let mut records = Vec::new();
        self.walk_between(low, high, |_, record| records.push(record))?;
        Ok(records)
    }

    /// The keys a reader that takes one leaf of the order at a time (an SQL
    /// cursor that reads live) has in hand next: those of the leaf at the
    /// start of the order, or its end `backwards`, or, `after` the entry of
    /// `record` whose key is `key`, those of the leaf next to where that
    /// entry is now, from it on; nearest first. The leaf is what the file
    /// holds as this is called: later changes to the order do not reach it.
    /// With them, whether a leaf follows that one as the file had it (the walk
    /// ends at the last one without looking again). Only an ADI order is read
    /// this way.
    pub(crate) fn walk_leaf(
        &self,
        backwards: bool,
        after: Option<(&[u8], u32)>,
    ) -> Result<(Vec<order::Key>, bool), Error> {
        let Some(shared) = &self.shared else {
            return Err(Error::NotImplemented);
        };
        let mut file = shared.file();
        let mut cursor = Cursor::new(&self.tag, &file);
        let moved = |r: Result<Option<&order::Key>, order::Error>| {
            r.map(|key| key.cloned()).map_err(|_| Error::CorruptIndex)
        };
        match after {
            None if backwards => {
                moved(cursor.last(&mut file))?;
            }
            None => {
                moved(cursor.first(&mut file))?;
            }
            Some((key, record)) => {
                let found = moved(cursor.seek(&mut file, key, Some(record)))?;
                if backwards {
                    // The entry before the one found, or the last if none was.
                    moved(cursor.prev(&mut file))?;
                } else if found.is_some_and(|found| found.record == record && found.bytes == key) {
                    moved(cursor.next(&mut file))?;
                }
            }
        };
        Ok(cursor.leaf_keys(!backwards))
    }

    /// The keys from `low` to `high` and their records, as
    /// [`Index::records_between`] finds them.
    pub(crate) fn keys_between(
        &self,
        low: Option<&[u8]>,
        high: Option<&[u8]>,
    ) -> Result<Vec<(Vec<u8>, u32)>, Error> {
        let mut keys = Vec::new();
        self.walk_between(low, high, |key, record| keys.push((key.to_vec(), record)))?;
        Ok(keys)
    }

    /// Passes each key from `low` to `high` and its record to `visit`, in
    /// index order (see [`Index::records_between`]).
    fn walk_between(
        &self,
        low: Option<&[u8]>,
        high: Option<&[u8]>,
        mut visit: impl FnMut(&[u8], u32),
    ) -> Result<(), Error> {
        let weights = self.weights();
        let compare = |a: &[u8], b: &[u8]| match &weights {
            Some(w) => {
                let weigh = |key: &[u8]| key.iter().map(|&b| w[usize::from(b)]).collect::<Vec<_>>();
                weigh(a).cmp(&weigh(b))
            }
            None => a.cmp(b),
        };
        // A descending order holds the largest key first.
        let (start, stop, past) = match self.tag.descending {
            false => (low, high, Ordering::Greater),
            true => (high, low, Ordering::Less),
        };
        if let (Some(low), Some(high)) = (low, high)
            && compare(low, high) == Ordering::Greater
        {
            return Ok(());
        }
        self.with_cursor(|cursor, file| {
            let mut key = match start {
                Some(start) => cursor.seek(file, start, None)?,
                None => cursor.first(file)?,
            };
            while let Some(found) = key {
                if stop.is_some_and(|stop| compare(&found.bytes, stop) == past) {
                    break;
                }
                visit(&found.bytes, found.record);
                key = cursor.next(file)?;
            }
            Ok(())
        })
    }

    /// What the order's keys are built from: one field of `table`, several
    /// (`F1;F2`), or an expression.
    pub(crate) fn key_source<'t>(&self, table: &'t table::Table) -> KeySource<'t> {
        // A full-text order of a DBF table names its field in the key
        // expression, where an ADT table's says `F<n>`
        // (`fixtures/aes/rc4tp/y12`).
        if self.tag.full_text() && table.dbf_flavour().is_some() {
            let name = self.tag.expression.trim();
            let found = table
                .fields()
                .iter()
                .find(|field| field.name.eq_ignore_ascii_case(name));
            return found.map_or(KeySource::Expression, KeySource::Field);
        }
        let fields: Option<Vec<usize>> = self
            .tag
            .expression
            .split(';')
            .map(|part| part.trim().strip_prefix('F')?.parse().ok())
            .collect();
        let field = |number: usize| table.key_field(number).ok();
        match fields.as_deref() {
            Some(&[number]) => field(number).map_or(KeySource::Expression, KeySource::Field),
            Some(&[number, ..]) => field(number).map_or(KeySource::Expression, KeySource::Fields),
            _ => KeySource::Expression,
        }
    }

    /// The number of keys in the order's tree: for a binary order, those of
    /// the records it does not mark deleted. See [`Index::key_count`].
    pub(crate) fn tree_keys(&self) -> Result<u32, Error> {
        self.with_cursor(|cursor, file| cursor.key_count(file))
    }

    /// The rules a full-text order tokenizes its records' text with, from
    /// its options ([`Index::fts_options`]).
    pub(crate) fn fts_rules(&self) -> Result<Arc<tflip_sql::fts::Rules>, Error> {
        if let Some(rules) = self.fts_rules.get() {
            return Ok(Arc::clone(rules));
        }
        let options = self.fts_options()?.ok_or(Error::CorruptIndex)?;
        let rules = Arc::new(tflip_sql::fts::Rules::from_options(&options));
        Ok(Arc::clone(self.fts_rules.get_or_init(|| rules)))
    }

    /// Whether the order is a full-text one its records' changes keep
    /// current: one not made `NOTMAINTAINED` (`ADS_FTS_FIXED`).
    pub(crate) fn maintained_full_text(&self) -> Result<bool, Error> {
        Ok(self.tag.full_text()
            && self
                .fts_options()?
                .is_some_and(|options| options.options & tflip_sql::fts::FTS_FIXED == 0))
    }

    /// The parsing options of a full-text order, read from its `.adi`
    /// ([`tflip_storage::adi::IndexFile::fts_options`]); `None` for an order
    /// that is not full-text.
    pub fn fts_options(&self) -> Result<Option<tflip_storage::adi::FtsOptions>, Error> {
        if !self.tag.full_text() {
            return Ok(None);
        }
        self.with_cursor(|_, file| file.fts_options(&self.tag))
    }

    /// The options block of a full-text order as it is stored, to make the
    /// order again; `None` for an order that is not full-text.
    pub(crate) fn fts_block(&self) -> Result<Option<tflip_storage::create::FtsBlock>, Error> {
        if !self.tag.full_text() {
            return Ok(None);
        }
        self.with_cursor(|_, file| file.fts_block(&self.tag))
    }

    /// The number of keys in the order, as the original engine counts them:
    /// one per record it holds, but for a binary order (`DELETED()`) the bits
    /// of its bitmaps, which cover records 0 to the last slot, deleted
    /// records included (5,001 for 5,000 slots). With `scoped`, only the
    /// keys in the scope count.
    pub fn key_count(&self, scoped: bool) -> Result<u32, Error> {
        // The records the cursor shows, whether it respects its filter or
        // not (`fixtures/sql/orderby.json`).
        if self.temporary.is_some() {
            return self.owner.record_count();
        }
        if self.tag.binary() {
            return Ok(self.owner.slot_count() + 1);
        }
        if !scoped || self.scope() == Scope::default() {
            return self.tree_keys();
        }
        let Some(_) = self.first()? else { return Ok(0) };
        let first = self.key_number()?.unwrap_or(0);
        let Some(_) = self.last()? else { return Ok(0) };
        let last = self.key_number()?.unwrap_or(0);
        Ok((last + 1).saturating_sub(first))
    }

    /// The type of the order's keys, as the original engine reports it: for
    /// an order on one field, by the field's type; for several segments
    /// (`a;b`, `UPPER(a);b`), [`KeyType::Raw`]. An expression's keys have the
    /// type of its result, found by compiling the expression — so `UPPER()`,
    /// `STR()` and `a + b` are [`KeyType::String`], `n + 1` is
    /// [`KeyType::Numeric`] and `d + 1` (a date) is [`KeyType::Date`]; a binary
    /// order (`DELETED()`) is [`KeyType::Logical`], and an expression that does
    /// not compile falls back to [`KeyType::String`].
    pub fn key_type(&self) -> KeyType {
        use tflip_storage::field::FieldType::*;
        if let Some(temporary) = &self.temporary {
            return temporary.key_type;
        }
        match self.key_source(&self.owner) {
            KeySource::Field(field) => match field.field_type {
                Logical => KeyType::Logical,
                Date => KeyType::Date,
                Numeric | ShortInt | Integer | LongInt | AutoInc | RowVersion | Double
                | CurDouble | Money => KeyType::Numeric,
                Raw | Guid | Time | TimeStamp | ModTime => KeyType::Raw,
                _ => KeyType::String,
            },
            KeySource::Fields(_) => KeyType::Raw,
            KeySource::Expression if self.tag.expression.contains(';') => KeyType::Raw,
            KeySource::Expression if self.tag.binary() => KeyType::Logical,
            KeySource::Expression => self
                .owner
                .compile(&self.expression())
                .map_or(KeyType::String, |expr| KeyType::of_expression(expr.ty())),
        }
    }

    /// The name of the field of segment `segment` (0-based; see
    /// [`Index::segments`]), as the table defines it. A segment past the last
    /// is [`Error::InvalidFieldNumber`].
    pub fn segment_field(&self, segment: usize) -> Result<String, Error> {
        let number = *self
            .segments()
            .get(segment)
            .ok_or(Error::InvalidFieldNumber)?;
        Ok(self.owner.field(FieldRef::Number(number))?.name.clone())
    }
}

/// The segments of an index expression; see [`Index::segments`].
fn segments(expression: &str) -> Vec<usize> {
    let mut fields = Vec::new();
    for (_, number) in field_refs(expression) {
        if !fields.contains(&number) {
            fields.push(number);
        }
    }
    fields
}

/// `expression` (from an index file, fields as `F<n>`) with each field
/// number `n` replaced by `map(n)`; `None` if `map` has no number for one.
pub(crate) fn renumber_fields(
    expression: &str,
    map: impl Fn(usize) -> Option<usize>,
) -> Option<String> {
    let mut out = String::new();
    let mut at = 0;
    for (range, number) in field_refs(expression) {
        out.push_str(&expression[at..range.start]);
        out.push_str(&format!("F{}", map(number)?));
        at = range.end;
    }
    out.push_str(&expression[at..]);
    Some(out)
}

/// The numbers of the fields an expression from an index file references
/// (see [`field_refs`]).
pub(crate) fn field_numbers(expression: &str) -> Vec<usize> {
    field_refs(expression)
        .into_iter()
        .map(|(_, number)| number)
        .collect()
}

/// The fields an expression from an index file references: where each one
/// is (a byte range) and its 1-based field number. Index files store fields
/// as `F<n>`; an `F` followed by digits that makes a word of its own and is
/// not inside quotes is a field.
fn field_refs(expression: &str) -> Vec<(Range<usize>, usize)> {
    let chars: Vec<(usize, char)> = expression.char_indices().collect();
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let byte = |i: usize| chars.get(i).map_or(expression.len(), |&(at, _)| at);
    let mut refs = Vec::new();
    let mut quote = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i].1;
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == 'F' && (i == 0 || !word(chars[i - 1].1)) => {
                let digits = chars[i + 1..]
                    .iter()
                    .take_while(|(_, c)| c.is_ascii_digit())
                    .count();
                let end = i + 1 + digits;
                if digits > 0 && chars.get(end).is_none_or(|&(_, c)| !word(c)) {
                    if let Ok(number) = expression[byte(i + 1)..byte(end)].parse() {
                        refs.push((byte(i)..byte(end), number));
                    }
                    i = end;
                    continue;
                }
            }
            None => {}
        }
        i += 1;
    }
    refs
}

/// `expression` with each `F<n>` replaced by the name of field *n* of
/// `table`. A number the table has no field for is left as it is.
fn with_field_names(expression: &str, table: &Table) -> String {
    let mut text = String::new();
    let mut copied = 0;
    for (range, number) in field_refs(expression) {
        if let Some(field) = number
            .checked_sub(1)
            .and_then(|i| table.table_fields().get(i))
        {
            text += &expression[copied..range.start];
            text += &field.name;
            copied = range.end;
        }
    }
    text += &expression[copied..];
    text
}

/// The structural index of the table at `path`: the same name with the
/// extension `.adi` for an ADT table, `.cdx` for a FoxPro or VFP one (in
/// upper case if the table's is).
fn structural_path(path: &Path) -> PathBuf {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let upper = extension.chars().any(|c| c.is_ascii_uppercase());
    let structural = match extension.eq_ignore_ascii_case("dbf") {
        true => "cdx",
        false => "adi",
    };
    path.with_extension(if upper {
        structural.to_ascii_uppercase()
    } else {
        structural.to_owned()
    })
}

/// Opens the structural index of the table `owner` behind the handle
/// `table` and returns the handles of its orders. A table without one has no
/// orders; so, for now, has a table whose index cannot be read (corrupt or
/// encrypted), rather than failing to open.
pub(crate) fn open_structural(session: &Session, table: Handle, owner: &Arc<Table>) -> Vec<Handle> {
    let path = structural_path(&owner.path);
    let cipher = owner.encryption_cipher();
    match open_index_file(&path, cipher.as_ref()) {
        Ok(file) => register(session, table, owner, path, file, cipher),
        Err(_) => Vec::new(),
    }
}

/// Refuses to open a table whose index keys a different static collation
/// weighed than its connection uses (`AE_INDEX_COLLATION_MISMATCH`), as the
/// original engine does. Only ADI files record the collation (a CRC-32 of its
/// byte-to-weight table, at file header 0x14) and keep weighed keys; NTX and
/// CDX keys are raw, so a collation only changes how they navigate. An index
/// that cannot be read (corrupt or encrypted without the key) is left to open
/// as it would otherwise — without its orders, not refused.
///
/// With `ADS_IGNORE_COLLATION_MISMATCH` such a file opens as it is, its keys
/// found by the connection's collation; with `ADS_REINDEX_ON_COLLATION_MISMATCH`
/// it is rebuilt under the connection's, which the original refuses (with
/// its reason added to the error) if the file has a custom order, or another
/// handle has the table open (`fixtures/collation/mismatch`).
pub(crate) fn check_collations(owner: &Arc<Table>) -> Result<(), Error> {
    use crate::table::options::{IGNORE_MISMATCH, REINDEX_ON_MISMATCH};
    let cipher = owner.encryption_cipher();
    let expected = tflip_storage::create::collation_crc(
        owner
            .session_settings()
            .collations()
            .table_collation(owner.is_oem())
            .map(|c| &c.weights),
    );
    let paths = match &owner.index_files {
        Some(files) => files
            .iter()
            .map(|file| crate::shares::actual_case(file.clone()))
            .collect(),
        None => vec![structural_path(&owner.path)],
    };
    let choice = owner.settings.options;
    for path in paths {
        if Kind::of(&path) != Kind::Adi {
            continue;
        }
        if let Ok(IndexFile::Adi(adi)) = open_index_file(&path, cipher.as_ref())
            && adi.collation_crc != expected
        {
            if choice & IGNORE_MISMATCH != 0 {
                continue;
            }
            if choice & REINDEX_ON_MISMATCH == 0 {
                return Err(Error::IndexCollationMismatch);
            }
            let refuse = |why: &str| Error::IndexCollationMismatch.detailed(why);
            if adi.tags.iter().any(|tag| tag.custom()) {
                return Err(refuse(
                    "Index file contains a custom index.  Cannot auto-reindex.",
                ));
            }
            if owner.opened_by_others() {
                return Err(refuse("Cannot open index exclusively for auto-reindex."));
            }
            rebuild_file(owner, &path, None, Some(expected))?;
        }
    }
    Ok(())
}

/// Opens the index file at `path`, deciphering it the way the table is
/// (`cipher`) when the table is encrypted; its index files decipher the same.
fn open_index_file(
    path: &Path,
    cipher: Option<&tflip_storage::table::RecordCipher>,
) -> Result<IndexFile, order::Error> {
    match cipher {
        Some(cipher) => IndexFile::open_encrypted(path, cipher),
        None => IndexFile::open(path),
    }
}

/// Opens the index file at `path` that a data dictionary has for its table
/// `owner` behind the handle `table`, as the structural one is opened (see
/// [`open_structural`]).
pub(crate) fn open_bound(session: &Session, table: Handle, owner: &Arc<Table>, path: &Path) {
    let path = crate::shares::actual_case(path.to_path_buf());
    let cipher = owner.encryption_cipher();
    if let Ok(file) = open_index_file(&path, cipher.as_ref()) {
        register(session, table, owner, path, file, cipher);
    }
}

/// Whether the index file at `path` opens with the table `owner` and
/// stays open: its structural one, or one its data dictionary has for it.
fn opens_with(owner: &Table, path: &Path) -> bool {
    match &owner.index_files {
        Some(files) => files
            .iter()
            .any(|file| crate::shares::actual_case(file.clone()) == path),
        None => path == structural_path(&owner.path),
    }
}

/// The dynamic collations the orders of the structural index of the table
/// at `path` name.
pub(crate) fn dynamic_collations(path: &Path) -> Vec<String> {
    let mut named = index_collations(path);
    named.retain(|collation| !collation.is_empty());
    named
}

/// The collations the orders of the structural index of the table at `path`
/// have, the empty name for an order of none.
pub(crate) fn index_collations(path: &Path) -> Vec<String> {
    // Only ADI files are checked yet: CDX files name VFP collations short.
    let structural = structural_path(path);
    if Kind::of(&structural) != Kind::Adi {
        return Vec::new();
    }
    let Ok(file) = IndexFile::open(&structural) else {
        return Vec::new();
    };
    file.tags("").into_iter().map(|tag| tag.collation).collect()
}

/// Opens the index file `name` of `table` (`AdsOpenIndex`): a name in the
/// table's folder, `.adi` (`.ntx` for a Clipper table, `.cdx` for a FoxPro
/// or VFP one) if it has no extension, found regardless of case, or a full
/// path. Its orders come after the table's open ones. As in
/// the original engine, a file that is open already (the structural one
/// among them) opens nothing: the handles it has are returned, and `true`.
/// A file that does not exist is [`Error::FileNotFound`].
pub fn open_file(
    session: &Session,
    table: Handle,
    name: &str,
) -> Result<(Vec<Handle>, bool), Error> {
    let owner = table::get(session, table)?;
    let folder = owner
        .path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let path = match session.shares {
        Some(_) => crate::shares::inside(&folder, name)?,
        None => folder.join(name),
    };
    let extension = match owner.settings.table_type {
        table::TableType::Ntx => "ntx",
        table::TableType::Cdx | table::TableType::Vfp => "cdx",
        _ => "adi",
    };
    let path = if path.extension().is_none() {
        path.with_extension(extension)
    } else {
        path
    };
    // As the folder spells it, so that a file is the same whatever case it
    // is named in, and is named as it is.
    let path = crate::shares::actual_case(path);
    let same = |other: &Path| match (other.canonicalize(), path.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => other == path,
    };
    let open: Vec<Handle> = of_table(session, table)
        .into_iter()
        .filter(|&order| get(session, order).is_ok_and(|order| same(&order.path)))
        .collect();
    if !open.is_empty() {
        return Ok((open, true));
    }
    // 15 index files are open on a table at most (`fixtures/write/limits`).
    let mut files: Vec<PathBuf> = Vec::new();
    for order in of_table(session, table) {
        if let Ok(order) = get(session, order)
            && !files.contains(&order.path)
        {
            files.push(order.path.clone());
        }
    }
    if files.len() >= 15 {
        return Err(crate::ddl::limit(
            5045,
            "The maximum number of index files that Advantage can have open per table is 15.  \
             Too many indexes open for this work area.",
        ));
    }
    let file = IndexFile::open(&path).map_err(|_| match path.exists() {
        true => Error::CorruptIndex,
        false => Error::FileNotFound,
    })?;
    Ok((register(session, table, &owner, path, file, None), false))
}

/// Closes the index file that holds the order `order` (`AdsCloseIndex`),
/// all its orders; the structural index cannot be closed
/// ([`Error::AutoOpenIndex`]).
pub fn close_file(session: &Session, order: Handle) -> Result<(), Error> {
    let index = get(session, order)?;
    if opens_with(&index.owner, &index.path) {
        return Err(Error::AutoOpenIndex);
    }
    for other in of_table(session, index.table) {
        if get(session, other).is_ok_and(|other| other.path == index.path) {
            session.remove(other);
        }
    }
    Ok(())
}

/// Closes the index files of `table` but its structural one
/// (`AdsCloseAllIndexes`).
pub fn close_files(session: &Session, table: Handle) -> Result<(), Error> {
    let owner = table::get(session, table)?;
    for order in of_table(session, table) {
        if get(session, order).is_ok_and(|order| !opens_with(&owner, &order.path)) {
            session.remove(order);
        }
    }
    Ok(())
}

/// Puts the tags of the index file `file` at `path` behind handles, as
/// orders of `table`.
fn register(
    session: &Session,
    table: Handle,
    owner: &Arc<Table>,
    path: PathBuf,
    file: IndexFile,
    cipher: Option<tflip_storage::table::RecordCipher>,
) -> Vec<Handle> {
    orders(table, owner, path, file, cipher)
        .into_iter()
        .map(|index| {
            index.owner.add_order(&index);
            session.insert(Object::Index(index), Some(table))
        })
        .collect()
}

/// The orders of the index file `file` at `path`, of the table `owner`
/// behind the handle `table`.
fn orders(
    table: Handle,
    owner: &Arc<Table>,
    path: PathBuf,
    file: IndexFile,
    cipher: Option<tflip_storage::table::RecordCipher>,
) -> Vec<Arc<Index>> {
    let page_size = file.page_size();
    let kind = file.kind();
    // An RC4 index has no IV to count.
    let counter = owner.aes_counter().or_else(|| {
        matches!(cipher, Some(tflip_storage::table::RecordCipher::Rc4(_))).then(Default::default)
    });
    let shared = crate::shared::open_index(&path, cipher.as_ref().zip(counter)).ok();
    // NTX and CDX keys are the stored bytes, ordered by the static
    // collation (the OEM one for an OEM table).
    if let (Some(shared), Kind::Ntx | Kind::Cdx) = (&shared, kind) {
        let collations = owner.session_settings().collations();
        let collation = collations.table_collation(owner.is_oem());
        shared.file().set_weights(collation.map(|c| c.weights));
    }
    // An NTX file's one order is named after the file, and has field names
    // where index files have `F<n>`.
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    file.tags(&stem)
        .into_iter()
        .map(|mut tag| {
            let written = (kind != Kind::Adi).then(|| {
                let written = (tag.expression.clone(), tag.condition.clone());
                tag.expression = owner.with_field_numbers(&tag.expression);
                tag.condition = owner.with_field_numbers(&tag.condition);
                written
            });
            if kind == Kind::Cdx {
                cdx_tag(&mut tag, owner, &written.as_ref().unwrap().0);
            }
            Arc::new(Index {
                table,
                owner: Arc::clone(owner),
                path: path.clone(),
                kind,
                tag,
                written,
                cipher: cipher.clone(),
                page_size,
                cursor: Mutex::new(None),
                scope: Mutex::new(Scope::default()),
                reversed: AtomicBool::new(false),
                raw_key: Mutex::new(None),
                shared: shared.clone(),
                temporary: None,
                fts_rules: std::sync::OnceLock::new(),
            })
        })
        .collect()
}

/// Creates an order on the table `table` (`AdsCreateIndex`): in the index
/// file `file` (its structural one for `None`), named `tag`, with the key
/// `expression` and the condition `condition`, `options` and pages of
/// `page_size` bytes; built at once, and opened on the table. An order of
/// that name in the file is replaced, as the original engine replaces it.
#[allow(clippy::too_many_arguments)]
pub fn create(
    session: &Session,
    table: Handle,
    file: Option<&str>,
    tag: &str,
    expression: &str,
    condition: &str,
    options: u32,
    page_size: u32,
) -> Result<Handle, Error> {
    let owner = table::get(session, table)?;
    let connection = owner.connection;
    // Before the table is looked at any further (`fixtures/aes/api/04_maintenance`).
    if owner.lacks_aes_password() {
        return Err(Error::EncryptionNotEnabled.detailed(
            "Cannot create an index on an encrypted table unless encryption is enabled.",
        ));
    }
    match owner.dbf_flavour() {
        Some(tflip_storage::dbf::Flavour::Clipper) => {
            return create_ntx(
                session, table, &owner, file, tag, expression, condition, options,
            );
        }
        Some(_) => {
            return create_cdx(
                session, table, &owner, file, tag, expression, condition, options,
            );
        }
        None => {}
    }
    if options & !crate::ddl::index_options::ALL != 0 {
        return Err(Error::InvalidOption.detailed("Parameter 7"));
    }
    // An ADI file's pages are 512 to 8,192 bytes (0 for the default).
    if page_size != 0 && !(512..=8192).contains(&page_size) {
        return Err(Error::InvalidOption.detailed("Parameter 8.  Page size must be 512..8192"));
    }
    // A user without `ALTER` on a dictionary's table makes a temporary
    // index: its own file, which the table's close deletes
    // (`fixtures/dictionary/freetables.json`).
    let bound = owner.object_id.is_some();
    let temporary = bound && owner.access & crate::system::right::ALTER == 0;
    let structural = file.is_none_or(str::is_empty);
    if temporary && structural {
        return Err(Error::Ace {
            code: 5054,
            detail: "The user do not have permssion to add index into an auto open index file."
                .to_owned(),
        });
    }
    // A dictionary's table is open exclusively to get an order in its own
    // index file.
    if bound && structural && !owner.exclusive() {
        return Err(Error::Ace {
            code: 5075,
            detail: "Index cannot be shared when creating orders.".to_owned(),
        });
    }
    let path = crate::ddl::order_file(session, connection, &owner, file)?;
    // An order of the same name goes first, and its handle with it.
    for handle in of_table(session, table) {
        if let Ok(order) = get(session, handle)
            && order.path == path
            && order.tag.name.eq_ignore_ascii_case(tag)
        {
            session.remove(handle);
        }
    }
    let order = crate::ddl::Order {
        table: "",
        file,
        tag,
        expression,
        condition,
        options,
        page_size: page_size as u16,
    };
    crate::ddl::create_order_on(session, connection, &owner, &order, true)?;
    if temporary {
        let mut files = lock(&owner.temporary_indexes);
        if !files.contains(&path) {
            files.push(path.clone());
        }
    } else if bound && let Some(name) = owner.alias.get() {
        // The order joins the dictionary, for every user of the table
        // (`fixtures/dictionary/freetables.json`).
        crate::dictionary_write::register_created_index(
            session, connection, name, file, tag, None,
        )?;
    }
    let file = IndexFile::open(&path).map_err(|_| Error::CorruptIndex)?;
    let fresh = orders(table, &owner, path.clone(), file, None);
    // A file that grew to bigger pages was built again: the orders it had
    // are new ones.
    rebind_orders(session, table, &owner, &path, &fresh);
    let index = fresh
        .into_iter()
        .find(|index| index.tag.name.eq_ignore_ascii_case(tag))
        .ok_or(Error::CorruptIndex)?;
    owner.add_order(&index);
    Ok(session.insert(Object::Index(index), Some(table)))
}

/// The options block of a full-text order `AdsCreateFTSIndex` makes: words of
/// `min_word` to `max_word` letters, and the delimiters, noise words, drop
/// and conditional characters of `lists`, each whether the defaults go with
/// it and the list given (none for NULL); `options` its `ADS_FTS_*` flags.
pub fn full_text_block(
    min_word: u32,
    max_word: u32,
    lists: &[(bool, Option<String>)],
    options: u32,
) -> Result<tflip_storage::create::FtsBlock, Error> {
    let list = |i: usize| match lists.get(i) {
        Some((true, None)) | None => None,
        Some((true, Some(text))) => Some((false, text.clone())),
        Some((false, text)) => Some((true, text.clone().unwrap_or_default())),
    };
    let full_text = tflip_sql::ast::FullText {
        min_word: Some(u64::from(min_word)),
        max_word: Some(u64::from(max_word)),
        delimiters: list(0),
        noise: list(1),
        drop: list(2),
        conditionals: list(3),
        fixed: options & 0x40 != 0,
        case_sensitive: options & 0x80 != 0,
        keep_score: options & 0x100 != 0,
        protect_numbers: options & 0x200 != 0,
    };
    tflip_sql::fts::index_block(&full_text).map_err(|bad| {
        Error::InvalidOption.detailed(format!("Parameter {}.  {}", bad.parameter, bad.why))
    })
}

/// A full-text order on a FoxPro or VFP table (`CREATE INDEX … CONTENT`,
/// `AdsCreateFTSIndex`), as the original engine makes it
/// (`fixtures/write/fts_cdx`): SQL's is a tag of the structural `.cdx`,
/// named for the order; the API's is a file of its own, a single order named
/// for the file (the table's `.idx` where none is given). Its header names
/// the field as it was given, and its keys are the words of the records, with
/// their counts if it keeps scores. `false` if a tag of that name is in the
/// file already.
pub(crate) fn create_full_text_xbase(
    session: &Session,
    connection: Handle,
    owner: &Arc<Table>,
    order: &crate::ddl::Order,
    field: usize,
    block: tflip_storage::create::FtsBlock,
    compound: bool,
) -> Result<bool, Error> {
    use tflip_storage::create::{CdxSpec, cdx_add_tag, idx_create};
    let visual = owner.dbf_flavour() == Some(tflip_storage::dbf::Flavour::Visual);
    if owner.dbf_flavour() == Some(tflip_storage::dbf::Flavour::Clipper) {
        return Err(Error::NotImplemented);
    }
    let extension = if compound { "cdx" } else { "idx" };
    let path = crate::ddl::order_file_ext(session, connection, owner, order.file, extension)?;
    let key_length = u16::try_from(block.max_word).unwrap_or(u16::MAX);
    if key_length > 240 {
        return Err(Error::InvalidKeyLength.detailed("Key too long."));
    }
    let tag = order.tag;
    if compound
        && let Ok(file) = IndexFile::open(&path)
        && file
            .tags("")
            .iter()
            .any(|t| t.name.eq_ignore_ascii_case(tag))
    {
        return Ok(false);
    }
    // SQL names the field as the table does; the API as it was asked for.
    let expression = match compound {
        true => owner.fields()[field].name.clone(),
        false => order.expression.to_owned(),
    };
    let spec = CdxSpec {
        name: tag.to_ascii_uppercase(),
        expression,
        condition: String::new(),
        key_length,
        unique: false,
        descending: false,
        pad: b' ',
        visual,
        custom: false,
        binary: false,
        nullable: false,
        candidate: false,
        fts: Some(block),
    };
    let structural = path == owner.path().with_extension("cdx");
    match compound {
        true => cdx_add_tag(&path, &spec).map(|_| ()),
        false => idx_create(&path, &spec),
    }
    .map_err(|_| Error::CorruptIndex)?;
    build_full_text(owner, &path, tag)?;
    if structural {
        owner.mark_structural_index()?;
    }
    Ok(true)
}

/// Creates the full-text order `tag` on the field `field` of the table
/// `table` (`AdsCreateFTSIndex`), in the index file `file` (its structural
/// one for `None`), with the options `block`, replacing an order of that
/// name, and opens it on the table.
#[allow(clippy::too_many_arguments)]
pub fn create_full_text(
    session: &Session,
    table: Handle,
    file: Option<&str>,
    tag: &str,
    field: &str,
    page_size: u32,
    block: tflip_storage::create::FtsBlock,
) -> Result<(), Error> {
    let owner = table::get(session, table)?;
    if page_size != 0 && !(512..=8192).contains(&page_size) {
        return Err(Error::InvalidOption.detailed("Parameter 5.  Page size must be 512..8192"));
    }
    let place = owner
        .fields()
        .iter()
        .position(|f| f.name.eq_ignore_ascii_case(field))
        .ok_or_else(|| Error::NoSuchField(field.to_owned()))?;
    // Refused before another order of the file is let go of.
    if !crate::ddl::can_have_full_text(owner.fields()[place].field_type) {
        return Err(Error::InvalidFieldType
            .detailed("FTS indexes can only be created on character and BLOB fields."));
    }
    // A FoxPro or VFP table's order is a file of its own, whatever the name
    // of the order (`fixtures/write/fts_cdx`): the table's `.idx` for none.
    let xbase = owner.dbf_flavour().is_some();
    let path = match xbase {
        true => crate::ddl::order_file_ext(session, owner.connection, &owner, file, "idx")?,
        false => crate::ddl::order_file(session, owner.connection, &owner, file)?,
    };
    for handle in of_table(session, table) {
        if let Ok(order) = get(session, handle)
            && order.path == path
            && (xbase || order.tag.name.eq_ignore_ascii_case(tag))
        {
            session.remove(handle);
        }
    }
    let order = crate::ddl::Order {
        table: "",
        file,
        tag,
        expression: field,
        condition: "",
        options: 0,
        page_size: page_size as u16,
    };
    crate::ddl::create_full_text_on(
        session,
        owner.connection,
        &owner,
        &order,
        place,
        block,
        true,
        false,
    )?;
    let opened = IndexFile::open(&path).map_err(|_| Error::CorruptIndex)?;
    let fresh = orders(table, &owner, path.clone(), opened, None);
    rebind_orders(session, table, &owner, &path, &fresh);
    let index = fresh
        .into_iter()
        .find(|index| xbase || index.tag.name.eq_ignore_ascii_case(tag))
        .ok_or(Error::CorruptIndex)?;
    owner.add_order(&index);
    session.insert(Object::Index(index), Some(table));
    Ok(())
}

/// [`create`] on a Clipper table: the NTX file `file` (named after the
/// order) holds the order, built at once from the keys of every record, as
/// the original engine builds it (see
/// [`tflip_storage::create::build_ntx`]); a unique order's keys go in one
/// by one, as when records are written. As in the original engine, an NTX
/// order cannot be compound (`AE_INVALID_OPTION`).
#[allow(clippy::too_many_arguments)]
fn create_ntx(
    session: &Session,
    table: Handle,
    owner: &Arc<Table>,
    file: Option<&str>,
    tag: &str,
    expression: &str,
    condition: &str,
    options: u32,
) -> Result<Handle, Error> {
    use crate::ddl::index_options;
    use tflip_storage::create::{NtxSpec, ntx_bytes};
    if options & index_options::COMPOUND != 0 {
        return Err(Error::InvalidOption.detailed("NTX tables cannot have compound indexes."));
    }
    if options & (index_options::CUSTOM | index_options::BINARY) != 0 {
        return Err(Error::InvalidOption);
    }
    let file = file.ok_or(Error::InvalidOption)?;
    let path = crate::ddl::order_file_ext(session, owner.connection, owner, Some(file), "ntx")?;
    // The limits of the original engine (`fixtures/write/limits`): a name of
    // 10 characters, expressions of 255, and 15 index files open on a table.
    if tag.len() > 10 {
        return Err(crate::ddl::limit(
            5050,
            "The name given was too long.  Parameter 3",
        ));
    }
    if expression.len() > 255 {
        return Err(Error::NoSuchField("Index key expression too long.".into()));
    }
    if condition.len() > 255 {
        return Err(Error::NoSuchField(
            "Index condition expression too long.".into(),
        ));
    }
    let key = owner.compile(expression).map_err(|error| match error {
        Error::Expression(kind) => Error::KeyExpression(kind),
        error => error,
    })?;
    if !condition.is_empty() {
        owner.compile(condition)?;
    }
    let mut open_files: Vec<std::path::PathBuf> = Vec::new();
    for handle in of_table(session, table) {
        if let Ok(order) = get(session, handle)
            && !open_files.contains(&order.path)
        {
            open_files.push(order.path.clone());
        }
    }
    if open_files.len() >= 15 && !open_files.contains(&path) {
        return Err(crate::ddl::limit(
            5045,
            "The maximum number of index files that Advantage can have open per table is 15.",
        ));
    }
    let (key_length, decimals) = ntx_key_shape(owner, expression, &key)?;
    if key_length > 256 {
        return Err(Error::InvalidKeyLength.detailed("Key too long."));
    }
    let spec = NtxSpec {
        key_length,
        decimals,
        expression: expression.to_owned(),
        condition: condition.to_owned(),
        unique: options & index_options::UNIQUE != 0,
        descending: options & index_options::DESCENDING != 0,
    };
    // Its handles on a file of that name go first.
    for handle in of_table(session, table) {
        if let Ok(order) = get(session, handle)
            && order.path == path
        {
            session.remove(handle);
        }
    }
    std::fs::write(&path, ntx_bytes(&spec)).map_err(|_| Error::FileCreate)?;
    let index = || -> Result<Arc<Index>, Error> {
        let file = IndexFile::open(&path).map_err(|_| Error::CorruptIndex)?;
        orders(table, owner, path.clone(), file, None)
            .into_iter()
            .next()
            .ok_or(Error::CorruptIndex)
    };
    let order = index()?;
    // Deleted records keep their keys in an xBase order.
    let keys = collect_keys(owner, &order, 1..=owner.slot_count())?;
    drop(order);
    write_ntx_keys(owner, &path, &spec, keys)?;
    let index = index()?;
    owner.add_order(&index);
    Ok(session.insert(Object::Index(index), Some(table)))
}

/// The length of an NTX order's keys of `expression` (compiled as `key`) on
/// `owner`, and their decimals: a field's key is its text, an expression's
/// its value's.
pub(crate) fn ntx_key_shape(
    owner: &Table,
    expression: &str,
    key: &tflip_expr::Expr,
) -> Result<(u16, u16), Error> {
    use tflip_storage::field::FieldType;
    let field = owner
        .fields()
        .iter()
        .find(|f| f.name.eq_ignore_ascii_case(expression.trim()));
    Ok(
        match field.map(|f| (f.field_type, f.length(), f.decimals())) {
            Some((FieldType::Numeric, length, decimals)) => (length, decimals),
            Some((FieldType::Character, length, _)) => (length, 0),
            // The digits of any Integer (`fixtures/write/sql_dbf`).
            Some((FieldType::Integer, _, _)) => (11, 0),
            // A Double's, as a restructure of a dictionary's table builds it
            // (`fixtures/sql/ddalterdbf`).
            Some((FieldType::Double, _, _)) => (26, 15),
            _ => match key.ty() {
                tflip_expr::Type::String { width, .. } => (width, 0),
                tflip_expr::Type::Date => (8, 0),
                tflip_expr::Type::Logical => (1, 0),
                _ => return Err(Error::NotImplemented),
            },
        },
    )
}

/// The length of a CDX order's keys of `expression` (compiled as `key`) on
/// `owner`, and whether they are text: a Character field's length, a
/// Logical's byte, a VFP Integer's four bytes, eight for numbers, dates and
/// timestamps (doubles), an expression's width; see `vfp_nullable` for the
/// byte a VFP field that takes NULL adds.
pub(crate) fn cdx_key_shape(
    owner: &Table,
    expression: &str,
    key: &tflip_expr::Expr,
) -> Result<(u16, bool), Error> {
    use tflip_storage::field::FieldType;
    // A key of several fields is theirs one after the other, each as a
    // segment (a VFP Integer's is a double); as one of text it is padded.
    if expression.contains(';') {
        let segments: Option<Vec<_>> = expression
            .split(';')
            .map(|part| {
                owner
                    .fields()
                    .iter()
                    .find(|f| f.name.eq_ignore_ascii_case(part.trim()))
            })
            .collect();
        if let Some(segments) = segments {
            let length = segments
                .iter()
                .map(|f| crate::table::cdx_segment_length(f))
                .sum::<usize>();
            let text = segments.last().is_some_and(|f| {
                matches!(
                    f.field_type,
                    FieldType::Character | FieldType::CiCharacter | FieldType::VarChar
                )
            });
            return Ok((length as u16, text));
        }
    }
    let field = owner
        .fields()
        .iter()
        .find(|f| f.name.eq_ignore_ascii_case(expression.trim()));
    let visual = owner.dbf_flavour() == Some(tflip_storage::dbf::Flavour::Visual);
    Ok(match field.map(|f| (f.field_type, f.length())) {
        Some((FieldType::Character, length)) => (length, true),
        Some((FieldType::Logical, _)) => (1, false),
        Some((FieldType::Integer | FieldType::AutoInc, _)) if visual => (4, false),
        // A VFP timestamp's key is a double (see `key::vfp_timestamp`).
        Some((FieldType::TimeStamp, _)) if visual => (8, false),
        Some((_, _)) | None => match key.ty() {
            tflip_expr::Type::String { width, .. } => (width, true),
            tflip_expr::Type::Logical => (1, false),
            tflip_expr::Type::Number(_) | tflip_expr::Type::Date | tflip_expr::Type::TimeStamp => {
                (8, false)
            }
            _ => return Err(Error::NotImplemented),
        },
    })
}

/// Whether `expression` reads a field of `owner` that takes NULL in a VFP
/// table, whose keys then start with a byte saying whether the key is NULL:
/// a field alone, or one in an expression (`UPPER(c)`, `CTOTS(c)`), as in
/// the original engine (`fixtures/write/cdx_timestamps`).
fn vfp_nullable(owner: &Table, expression: &str) -> bool {
    /// The table's schema, noting the fields an expression names.
    struct Noting<'a> {
        schema: crate::filter::Schema<'a>,
        named: std::cell::RefCell<Vec<usize>>,
    }
    impl tflip_expr::Schema for Noting<'_> {
        fn alias(&self) -> &str {
            self.schema.alias()
        }
        fn field(&self, name: &str) -> Option<tflip_expr::Field> {
            let field = self.schema.field(name)?;
            self.named.borrow_mut().push(field.index);
            Some(field)
        }
    }
    let alias = owner
        .path()
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    let noting = Noting {
        schema: crate::filter::Schema {
            alias: &alias,
            fields: owner.fields(),
        },
        named: Default::default(),
    };
    if tflip_expr::Expr::compile(expression, &noting).is_err() {
        return false;
    }
    let named = noting.named.into_inner();
    named.into_iter().any(|index| {
        matches!(&owner.fields()[index].layout,
            tflip_storage::field::Layout::Dbf(l) if l.null_bit.is_some())
    })
}

/// [`create`] on a FoxPro or VFP table: the order goes into the compound
/// index file `file` (the structural one for `None`, which the table's
/// header then names), built at once from the keys of every record, as the
/// original engine builds it (see [`tflip_storage::create::cdx_add_tag`]);
/// a unique order keeps the first record of each key. Single-order `.idx`
/// files, and candidate, binary and custom orders, are not created yet
/// (`AE_NOT_IMPLEMENTED`).
#[allow(clippy::too_many_arguments)]
fn create_cdx(
    session: &Session,
    table: Handle,
    owner: &Arc<Table>,
    file: Option<&str>,
    tag: &str,
    expression: &str,
    condition: &str,
    options: u32,
) -> Result<Handle, Error> {
    use crate::ddl::index_options;
    use tflip_storage::create::{
        CdxSpec, cdx_add_tag, cdx_build_bitmap, cdx_build_tag, idx_create,
    };
    let visual = owner.dbf_flavour() == Some(tflip_storage::dbf::Flavour::Visual);
    // As in the original engine, a FoxPro table has no candidate orders.
    let candidate = options & index_options::CANDIDATE != 0;
    if candidate && !visual {
        let why = "ADS_CANDIDATE is not a valid option for this table type.";
        return Err(Error::InvalidOption.detailed(why));
    }
    let binary = options & index_options::BINARY != 0;
    let custom = options & index_options::CUSTOM != 0;
    // An order that is not compound has an `.idx` file of its own, named
    // after the table without a file (`fixtures/write/sql_dbf`).
    let single = options & index_options::COMPOUND == 0;
    if candidate && single {
        let why = "ADS_CANDIDATE cannot be specified for non-compound index files.";
        return Err(Error::InvalidOption.detailed(why));
    }
    let extension = if single { "idx" } else { "cdx" };
    let path = crate::ddl::order_file_ext(session, owner.connection, owner, file, extension)?;
    // The limits of the original engine (`fixtures/write/limits`).
    if tag.len() > 10 {
        return Err(crate::ddl::limit(
            5050,
            "The name given was too long.  Parameter 3",
        ));
    }
    if expression.len() + condition.len() > 510 {
        return Err(Error::NoSuchField("Index expressions too long.".into()));
    }
    let key = owner.compile(expression).map_err(|error| match error {
        Error::Expression(kind) => Error::KeyExpression(kind),
        error => error,
    })?;
    if !condition.is_empty() {
        owner.compile(condition)?;
    }
    // A binary order's keys are a byte, `T` or `F`.
    let (key_length, text) = match binary {
        true => (1, false),
        false => cdx_key_shape(owner, expression, &key)?,
    };
    // A field that takes NULL adds a byte to its keys, but in a candidate
    // order, which leaves NULL out.
    let null_field = visual && vfp_nullable(owner, expression);
    let nullable = null_field && !candidate && !binary;
    let key_length = key_length + u16::from(nullable);
    if key_length > 240 {
        return Err(Error::InvalidKeyLength.detailed("Key too long."));
    }
    if !single && let Ok(file) = IndexFile::open(&path) {
        let tags = file.tags("");
        let named = tags.iter().any(|t| t.name.eq_ignore_ascii_case(tag));
        if tags.len() >= 256 && !named {
            return Err(crate::ddl::limit(
                5046,
                "The maximum number of tags that Advantage supports per compound index is 256.",
            ));
        }
    }
    // A candidate order over a NULL is refused before the file is touched
    // (`fixtures/write/cdx_custom_reindex`).
    if null_field && candidate {
        let field = owner
            .fields()
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(expression.trim()));
        for slot in 1..=owner.slot_count() {
            let values = owner.slot_values(slot)?;
            if field.is_some_and(|i| values[i] == tflip_storage::value::Value::Null) {
                return Err(Error::Constraint {
                    code: 7153,
                    text: "The index key cannot be null.".to_owned(),
                });
            }
        }
    }
    let spec = CdxSpec {
        name: tag.to_ascii_uppercase(),
        expression: expression.to_owned(),
        condition: condition.to_owned(),
        key_length,
        unique: options & index_options::UNIQUE != 0,
        descending: options & index_options::DESCENDING != 0,
        pad: if text { b' ' } else { 0 },
        visual,
        custom,
        binary,
        nullable,
        candidate,
        fts: None,
    };
    let structural = path == owner.path().with_extension("cdx");
    let header = match single {
        true => idx_create(&path, &spec).map(|()| 0),
        false => cdx_add_tag(&path, &spec),
    }
    .map_err(|_| Error::CorruptIndex)?;
    let index = || -> Result<Arc<Index>, Error> {
        let file = IndexFile::open(&path).map_err(|_| Error::CorruptIndex)?;
        orders(table, owner, path.clone(), file, None)
            .into_iter()
            .find(|index| single || index.tag.name.eq_ignore_ascii_case(tag))
            .ok_or(Error::CorruptIndex)
    };
    let order = index()?;
    // Deleted records keep their keys in an xBase order; a custom order
    // takes none.
    let slots = (1..=owner.slot_count()).filter(|_| !custom);
    let mut keys = collect_keys(owner, &order, slots)?;
    drop(order);
    if binary {
        cdx_build_bitmap(
            &path,
            header,
            owner.slot_count(),
            &true_records(&keys),
            None,
        )
        .map_err(|_| Error::NotImplemented)?;
    } else {
        sort_cdx_keys(&mut keys, spec.unique);
        let pad = spec.condition.is_empty().then_some(spec.pad);
        cdx_build_tag(&path, header, pad, &keys, None).map_err(|_| Error::CorruptIndex)?;
    }
    if structural {
        owner.mark_structural_index()?;
    }
    let index = index()?;
    owner.add_order(&index);
    Ok(session.insert(Object::Index(index), Some(table)))
}

/// Deletes the order `order` from its index file (`AdsDeleteIndex`) and
/// closes its handle. A compound `.cdx` file left without orders goes, and
/// a structural one's table no longer has one, as in the original engine
/// (`fixtures/write/cdx_drop`). Orders of NTX and IDX files, which hold one
/// order each, are not deleted ([`Error::NotImplemented`]).
pub fn delete(session: &Session, order: Handle) -> Result<(), Error> {
    use tflip_storage::create::{cdx_drop_tag, drop_order};
    let index = get(session, order)?;
    // Not in a transaction (5063, `fixtures/sql/txremote.json`).
    crate::transaction::check_legal(session, index.owner.connection, Illegal::DeleteIndex)?;
    // An order of a shared AES table is not deleted by a handle without its
    // password (`fixtures/aes/api/04_maintenance`); of a table open
    // exclusively it is (`06_exclusive`).
    if index.owner.lacks_aes_password() && !index.owner.exclusive() {
        return Err(Error::Ace {
            code: 5075,
            detail: "Can't delete a shared index.".to_owned(),
        });
    }
    let compound = index
        .path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cdx"));
    let left = match index.kind {
        Kind::Adi => drop_order(&index.path, &index.tag.name).map(|dropped| dropped.then_some(1)),
        Kind::Cdx if compound => cdx_drop_tag(&index.path, &index.tag.name),
        Kind::Cdx | Kind::Ntx => return Err(Error::NotImplemented),
    }
    .map_err(|_| Error::CorruptIndex)?
    .ok_or(Error::InvalidIndexOrderName)?;
    let (owner, path) = (Arc::clone(&index.owner), index.path.clone());
    drop(index);
    session.remove(order);
    if left == 0 {
        let _ = std::fs::remove_file(&path);
        if path == structural_path(&owner.path) {
            owner.unmark_structural_index()?;
        }
    }
    Ok(())
}

/// Rebuilds the index files open on the table `handle`, or the one of the
/// order `handle` (`AdsReindex`), as the original engine does (see
/// [`tflip_storage::create::rebuild_file`]); the table must be open
/// exclusively. The orders' handles stay, with the orders rebuilt.
pub fn reindex(session: &Session, handle: Handle) -> Result<(), Error> {
    reindex_pages(session, handle, None)
}

/// [`reindex`], with ADI files of `page_size`-byte pages if given (`sp_Reindex`;
/// NTX and CDX files have no such thing).
pub fn reindex_pages(
    session: &Session,
    handle: Handle,
    page_size: Option<u16>,
) -> Result<(), Error> {
    let (table, only) = match get(session, handle) {
        Ok(order) => (order.table, Some(order.path.clone())),
        Err(_) => (handle, None),
    };
    let owner = table::get(session, table)?;
    owner.refuse_cursor()?;
    if !owner.exclusive() {
        return Err(Error::NotExclusive);
    }
    crate::transaction::check_legal(session, owner.connection, Illegal::Reindex)?;
    owner.flush_pending()?;
    // Without the key the index files cannot be rebuilt; a zap has emptied
    // the table by now (`fixtures/aes/api/06_exclusive`, `07_zap`).
    if owner.lacks_aes_password() {
        return Err(Error::EncryptionNotEnabled
            .detailed("Cannot reindex an encrypted table unless encryption is enabled."));
    }
    // The files, in the order their orders were opened.
    let handles = of_table(session, table);
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    let mut kinds = Vec::new();
    for &h in &handles {
        if let Ok(order) = get(session, h)
            && !paths.contains(&order.path)
            && only.as_ref().is_none_or(|only| *only == order.path)
        {
            paths.push(order.path.clone());
            kinds.push(order.kind);
        }
    }
    for (path, kind) in paths.into_iter().zip(kinds) {
        match kind {
            Kind::Adi => rebuild(session, table, &owner, &path, page_size)?,
            Kind::Ntx | Kind::Cdx => rebuild_xbase(session, table, &owner, &path, kind)?,
        }
    }
    Ok(())
}

/// Rebuilds the index files of the table `table` that hold only full-text
/// orders (`AdsReindexFTS`), with pages of `page_size` bytes, or of their own
/// for 0 (`fixtures/write/fts_rebuild`). The table must be open exclusively,
/// and not in a transaction.
pub fn reindex_fts(session: &Session, table: Handle, page_size: u32) -> Result<(), Error> {
    let owner = table::get(session, table)?;
    owner.refuse_cursor()?;
    let page_size = match page_size {
        0 => None,
        512..=8192 => Some(page_size as u16),
        _ => return Err(Error::InvalidOption.detailed("Parameter 2.  Page size must be 512..8192")),
    };
    if !owner.exclusive() {
        return Err(Error::NotExclusive);
    }
    crate::transaction::check_legal(session, owner.connection, Illegal::Reindex)?;
    owner.flush_pending()?;
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    for handle in of_table(session, table) {
        if let Ok(order) = get(session, handle)
            && order.kind == Kind::Adi
            && !paths.contains(&order.path)
        {
            paths.push(order.path.clone());
        }
    }
    for path in paths {
        let file = open_deciphering(&owner, &path)?;
        if file.tags("").iter().all(|tag| tag.full_text()) {
            rebuild(session, table, &owner, &path, page_size)?;
        }
    }
    Ok(())
}

/// Rebuilds the NTX or CDX file `path` of the table `owner` (behind
/// `table`) as its orders are created (see [`create_ntx`] and
/// [`create_cdx`]): the file written anew, each order built from the
/// records' keys; its orders' handles get the rebuilt orders.
fn rebuild_xbase(
    session: &Session,
    table: Handle,
    owner: &Arc<Table>,
    path: &Path,
    kind: Kind,
) -> Result<(), Error> {
    use tflip_storage::create::{
        CdxSpec, NtxSpec, cdx_add_tag, cdx_build_bitmap, cdx_build_tag_scored, idx_create,
        ntx_bytes,
    };
    let file = IndexFile::open(path).map_err(|_| Error::CorruptIndex)?;
    let current = orders(table, owner, path.to_path_buf(), file, None);
    let visual = owner.dbf_flavour() == Some(tflip_storage::dbf::Flavour::Visual);
    let keys_of = |order: &Index| collect_keys(owner, order, 1..=owner.slot_count());
    let reopened = || -> Result<Vec<Arc<Index>>, Error> {
        let file = IndexFile::open(path).map_err(|_| Error::CorruptIndex)?;
        Ok(orders(table, owner, path.to_path_buf(), file, None))
    };
    match kind {
        Kind::Ntx => {
            let order = current.first().ok_or(Error::CorruptIndex)?;
            let (expression, condition) = order.written.clone().unwrap_or_default();
            let spec = NtxSpec {
                key_length: order.tag.key_length,
                decimals: order.tag.decimals,
                expression,
                condition,
                unique: order.tag.unique(),
                descending: order.tag.descending,
            };
            let keys = keys_of(order)?;
            std::fs::write(path, ntx_bytes(&spec)).map_err(|_| Error::FileCreate)?;
            write_ntx_keys(owner, path, &spec, keys)?;
        }
        _ => {
            let mut built = Vec::with_capacity(current.len());
            for order in &current {
                let (expression, condition) = order.written.clone().unwrap_or_default();
                let mut spec = CdxSpec::from_tag(&order.tag, expression, condition, visual);
                // A full-text order's keys are the words of the records.
                if order.tag.full_text() {
                    let block = order.fts_block()?.ok_or(Error::CorruptIndex)?;
                    let (keys, scores) =
                        full_text_words(owner, &order.tag, order.fts_rules()?.as_ref())?;
                    spec.fts = Some(block);
                    built.push((spec, keys, scores));
                    continue;
                }
                // A reindex empties a custom order, whose keys are only those
                // clients add.
                let mut keys = match order.tag.custom() {
                    true => Vec::new(),
                    false => keys_of(order)?,
                };
                sort_cdx_keys(&mut keys, order.tag.unique());
                built.push((spec, keys, Vec::new()));
            }
            // Emptied in place: the file stays the one its handles share. As
            // the original engine rewrites it, every tag's header and root
            // come first, then each tree, one unused node after each. An
            // `.idx` file is its one order's header and tree.
            let single = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("idx"));
            std::fs::write(path, b"").map_err(|_| Error::FileInUse)?;
            let mut headers = Vec::with_capacity(built.len());
            for (spec, _, _) in &built {
                headers.push(
                    match single {
                        true => idx_create(path, spec).map(|()| 0),
                        false => cdx_add_tag(path, spec),
                    }
                    .map_err(|_| Error::CorruptIndex)?,
                );
            }
            let length = std::fs::metadata(path)
                .map_err(|_| Error::CorruptIndex)?
                .len();
            let mut next = (length / tflip_storage::cdx::NODE_SIZE as u64) as u32;
            for ((spec, keys, scores), header) in built.iter().zip(headers) {
                if spec.binary {
                    let set = true_records(keys);
                    cdx_build_bitmap(path, header, owner.slot_count(), &set, Some(&mut next))
                        .map_err(|_| Error::NotImplemented)?;
                    continue;
                }
                let pad = spec.condition.is_empty().then_some(spec.pad);
                let counts: std::collections::HashMap<_, _> = keys
                    .iter()
                    .zip(scores)
                    .map(|(key, &score)| ((key.record, key.bytes.as_slice()), score))
                    .collect();
                let score = |key: &tflip_storage::order::Key| {
                    counts
                        .get(&(key.record, key.bytes.as_slice()))
                        .copied()
                        .unwrap_or(0)
                };
                let keeps = spec
                    .fts
                    .as_ref()
                    .is_some_and(|block| block.options & tflip_storage::adi::FTS_KEEP_SCORE != 0);
                let score: Option<&dyn Fn(&tflip_storage::order::Key) -> u16> =
                    keeps.then_some(&score);
                cdx_build_tag_scored(path, header, pad, keys, Some(&mut next), score)
                    .map_err(|_| Error::CorruptIndex)?;
            }
        }
    }
    drop(current);
    let fresh = reopened()?;
    rebind_orders(session, table, owner, path, &fresh);
    Ok(())
}

/// The keys `order` has for the records `slots` of `owner`, which have
/// one (an ADI order walks the active records, an xBase one every slot).
fn collect_keys(
    owner: &Table,
    order: &Index,
    slots: impl Iterator<Item = u32>,
) -> Result<Vec<tflip_storage::order::Key>, Error> {
    let mut keys = Vec::new();
    for slot in slots {
        if let Some(bytes) = owner.record_key(order, slot)? {
            keys.push(tflip_storage::order::Key {
                bytes,
                record: slot,
            });
        }
    }
    Ok(keys)
}

/// Writes the keys of an NTX order whose file `path` is new, as the original
/// engine writes them: a unique order's one by one, any other's built from
/// the keys in the order of their weights.
fn write_ntx_keys(
    owner: &Table,
    path: &Path,
    spec: &tflip_storage::create::NtxSpec,
    mut keys: Vec<tflip_storage::order::Key>,
) -> Result<(), Error> {
    let weights = owner
        .session_settings()
        .collations()
        .table_collation(owner.is_oem())
        .map(|c| c.weights);
    if spec.unique {
        let mut ntx =
            tflip_storage::ntx::NtxFile::open_for_writing(path).map_err(|_| Error::CorruptIndex)?;
        ntx.set_weights(weights);
        for key in &keys {
            ntx.insert_key(&key.bytes, key.record)
                .map_err(|_| Error::CorruptIndex)?;
        }
        return Ok(());
    }
    keys.sort_by(|a, b| {
        let by_key = weighed_cmp(&a.bytes, &b.bytes, weights.as_ref());
        let by_key = if spec.descending {
            by_key.reverse()
        } else {
            by_key
        };
        by_key.then(a.record.cmp(&b.record))
    });
    tflip_storage::create::build_ntx(path, &keys).map_err(|_| Error::CorruptIndex)
}

/// Puts a CDX order's keys in its order (by their bytes, then records); a
/// unique one keeps the first record of each key.
fn sort_cdx_keys(keys: &mut Vec<tflip_storage::order::Key>, unique: bool) {
    keys.sort_by(|a, b| a.bytes.cmp(&b.bytes).then(a.record.cmp(&b.record)));
    if unique {
        keys.dedup_by(|b, a| a.bytes == b.bytes);
    }
}

/// Builds the keys of the tag `spec` of the CDX file `path`, whose header is
/// at `header` and which was just added empty, as a `CREATE INDEX` builds it.
pub(crate) fn build_cdx_tag(
    owner: &Arc<Table>,
    path: &Path,
    header: u32,
    spec: &tflip_storage::create::CdxSpec,
) -> Result<(), Error> {
    use tflip_storage::create::{cdx_build_bitmap, cdx_build_tag};
    if spec.fts.is_some() {
        return build_full_text(owner, path, &spec.name);
    }
    let file = IndexFile::open(path).map_err(|_| Error::CorruptIndex)?;
    let order = orders(0, owner, path.to_path_buf(), file, None)
        .into_iter()
        .find(|order| order.tag.name.eq_ignore_ascii_case(&spec.name))
        .ok_or(Error::CorruptIndex)?;
    let slots = (1..=owner.slot_count()).filter(|_| !spec.custom);
    let mut keys = collect_keys(owner, &order, slots)?;
    drop(order);
    if spec.binary {
        let set = true_records(&keys);
        return cdx_build_bitmap(path, header, owner.slot_count(), &set, None)
            .map_err(|_| Error::NotImplemented);
    }
    sort_cdx_keys(&mut keys, spec.unique);
    let pad = spec.condition.is_empty().then_some(spec.pad);
    cdx_build_tag(path, header, pad, &keys, None).map_err(|_| Error::CorruptIndex)
}

/// The records a binary CDX order's `keys` set: those whose key is `T`.
fn true_records(keys: &[tflip_storage::order::Key]) -> Vec<u32> {
    keys.iter()
        .filter(|k| k.bytes == b"T")
        .map(|k| k.record)
        .collect()
}

/// Gives the handles of `table` on the index file `path` the orders `fresh`
/// of the rebuilt file, by name.
fn rebind_orders(
    session: &Session,
    table: Handle,
    owner: &Arc<Table>,
    path: &Path,
    fresh: &[Arc<Index>],
) {
    for handle in of_table(session, table) {
        let Ok(old) = get(session, handle) else {
            continue;
        };
        if old.path != path {
            continue;
        }
        let Some(new) = fresh
            .iter()
            .find(|f| f.tag.name.eq_ignore_ascii_case(&old.tag.name))
        else {
            continue;
        };
        owner.add_order(new);
        session.with(handle, |object| *object = Object::Index(Arc::clone(new)));
    }
}

/// The record slots from 1 to `slots` that are not among the `active` ones.
fn inactive_slots(active: &[u32], slots: u32) -> Vec<u32> {
    // A table of hundreds of thousands of records is too many for a scan
    // of `active` per slot.
    let mut is_active = vec![false; slots as usize + 1];
    for &slot in active {
        is_active[slot as usize] = true;
    }
    (1..=slots)
        .filter(|&slot| !is_active[slot as usize])
        .collect()
}

/// The index file `path` of the table `owner`, deciphered if the table is
/// AES-encrypted and the file enciphered.
pub(crate) fn open_deciphering(owner: &Arc<Table>, path: &Path) -> Result<IndexFile, Error> {
    match owner.encryption_cipher() {
        Some(
            cipher @ (tflip_storage::table::RecordCipher::Aes { .. }
            | tflip_storage::table::RecordCipher::Rc4(_)),
        ) if tflip_storage::adi::is_aes(path) => IndexFile::open_encrypted(path, &cipher),
        _ => IndexFile::open(path),
    }
    .map_err(|_| Error::CorruptIndex)
}

/// Rebuilds the ADI file `path` of the table `owner` (behind `table`) and
/// gives its orders' handles the rebuilt orders.
fn rebuild(
    session: &Session,
    table: Handle,
    owner: &Arc<Table>,
    path: &Path,
    page_size: Option<u16>,
) -> Result<(), Error> {
    rebuild_file(owner, path, page_size, None)?;
    // The handles get the rebuilt orders.
    let file = open_deciphering(owner, path)?;
    let fresh = orders(table, owner, path.to_path_buf(), file, None);
    rebind_orders(session, table, owner, path, &fresh);
    Ok(())
}

/// Writes the ADI file `path` of the table `owner` anew, with pages of
/// `page_size` bytes if given: every order built again from the records,
/// and the collation the file records (see
/// [`tflip_storage::create::rebuild_file_collated`]) changed if given.
pub(crate) fn rebuild_file(
    owner: &Arc<Table>,
    path: &Path,
    page_size: Option<u16>,
    collation: Option<u32>,
) -> Result<(), Error> {
    // An AES table's index keeps 8 bytes of each page free for its IV.
    let reserve = match tflip_storage::adi::is_aes(path)
        && matches!(
            owner.encryption_cipher(),
            Some(tflip_storage::table::RecordCipher::Aes { .. })
        ) {
        true => 8,
        false => 0,
    };
    crate::ddl::with_clear_index(owner, path, || {
        rebuild_clear_file(owner, path, page_size, collation, reserve)
    })
}

/// [`rebuild_file`] on an index file in the clear.
fn rebuild_clear_file(
    owner: &Arc<Table>,
    path: &Path,
    page_size: Option<u16>,
    collation: Option<u32>,
    reserve: usize,
) -> Result<(), Error> {
    use tflip_storage::create::{OrderSpec, Rebuilt};
    let mut file = IndexFile::open(path).map_err(|_| Error::CorruptIndex)?;
    let blocks = file
        .tags("")
        .iter()
        .map(|tag| file.fts_block(tag))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| Error::CorruptIndex)?;
    let current = orders(0, owner, path.to_path_buf(), file, None);
    let active = owner.active_slots()?;
    let slots = owner.slot_count();
    let deleted = inactive_slots(&active, slots);
    let mut rebuilt = Vec::with_capacity(current.len());
    for (order, block) in current.iter().zip(blocks) {
        let tag = &order.tag;
        let mut keys = Vec::new();
        let mut scores = Vec::new();
        if tag.full_text() {
            let rules = order.fts_rules()?;
            let mut words = Vec::new();
            for &slot in &active {
                for (bytes, count) in owner.record_words(tag, &rules, slot)? {
                    words.push((
                        tflip_storage::order::Key {
                            bytes,
                            record: slot,
                        },
                        count,
                    ));
                }
            }
            words.sort_by(|(a, _), (b, _)| a.bytes.cmp(&b.bytes).then(a.record.cmp(&b.record)));
            (keys, scores) = words.into_iter().unzip();
        } else if !tag.binary() && !tag.custom() {
            keys = collect_keys(owner, order, active.iter().copied())?;
            keys.sort_by(|a, b| a.bytes.cmp(&b.bytes).then(a.record.cmp(&b.record)));
        }
        let mut spec = OrderSpec::from_tag(tag);
        spec.fts = block;
        // The blanks that pad text keys weigh what the new collation says.
        if collation.is_some() && spec.pad != 0 && !tag.binary() {
            spec.pad = owner
                .session_settings()
                .collations()
                .table_collation(owner.is_oem())
                .map_or(b' ', |c| c.weights[usize::from(b' ')]);
        }
        let changes = order
            .shared
            .as_deref()
            .map_or(0, |shared| match &*shared.file() {
                IndexFile::Adi(file) => file.changes_of(tag),
                _ => 0,
            });
        rebuilt.push(Rebuilt {
            spec,
            keys,
            slots,
            deleted: deleted.clone(),
            changes,
            scores,
        });
    }
    tflip_storage::create::rebuild_file_collated(path, &rebuilt, page_size, collation, reserve)
        .and_then(|()| crate::shared::reopen_index(path))
        .map_err(|_| Error::CorruptIndex)
}

/// Packs the table `handle` (`AdsPackTable`), or with `zap` empties it
/// (`AdsZapTable`), and rebuilds its open index files; it goes to its first
/// record.
pub fn pack(session: &Session, handle: Handle, zap: bool) -> Result<(), Error> {
    pack_to(session, handle, zap, 0)
}

/// [`pack`], with memo blocks of `block_size` bytes (0 for the table's own,
/// else 8 to 1,024, as `AdsPackTable120` takes it).
pub fn pack_to(session: &Session, handle: Handle, zap: bool, block_size: u32) -> Result<(), Error> {
    table::get(session, handle)?.refuse_cursor()?;
    let block_size = match block_size {
        0 => None,
        8..=1024 => Some(block_size as u16),
        _ => return Err(Error::InvalidMemoBlockSize),
    };
    pack_blocks(session, handle, zap, block_size)
}

/// [`pack_to`] with memo blocks of `block_size` bytes if given, whatever the
/// size (`sp_PackTable` checks its own).
pub(crate) fn pack_blocks(
    session: &Session,
    handle: Handle,
    zap: bool,
    block_size: Option<u16>,
) -> Result<(), Error> {
    let owner = table::get(session, handle)?;
    if owner.exclusive() {
        let command = if zap { Illegal::Zap } else { Illegal::Pack };
        crate::transaction::check_legal(session, owner.connection, command)?;
    }
    owner.pack_file(zap, block_size)?;
    let reindexed = reindex(session, handle);
    match zap {
        // A zapped table is on no record, whether or not its files could be
        // rebuilt after (`fixtures/aes/api/07_zap`).
        true => {
            owner.unposition();
            reindexed
        }
        false => {
            reindexed?;
            owner.goto_top()
        }
    }
}

/// Builds the order `tag` of the ADI file `path`, just created, from the
/// keys of `table`'s records at once, as the original engine builds it
/// (see [`tflip_storage::create::build_order`]); a custom order stays
/// empty, and a unique one with duplicate keys is [`Error::Sql`] 7059.
pub(crate) fn build_keys(table: &Arc<Table>, path: &Path, tag: &str) -> Result<(), Error> {
    finish_keys(path, tag, prepare_keys(table, path, tag)?)
}

/// The keys of an order, collected and sorted, with the table's changes at
/// the time (see [`prepare_keys`]).
pub(crate) struct Prepared {
    /// What the process had changed of the table's file when the keys were
    /// collected: the keys are current while it still is.
    pub(crate) changes: u64,
    /// `None` where there is nothing to build: no records, or a custom order.
    work: Option<Work>,
}

struct Work {
    keys: Vec<tflip_storage::order::Key>,
    active: Vec<u32>,
    slots: u32,
    unique: bool,
    /// The bytes each page keeps free: an AES-encrypted table's index keeps
    /// 8 for the page's IV.
    reserve: usize,
}

impl Prepared {
    /// Whether the order is unique and its keys are not.
    pub(crate) fn duplicates(&self) -> bool {
        self.work
            .as_ref()
            .is_some_and(|w| w.unique && w.keys.windows(2).any(|k| k[0].bytes == k[1].bytes))
    }
}

/// The 7059 of a unique order whose keys are not.
pub(crate) fn not_unique() -> Error {
    Error::Sql(tflip_sql::SqlError::ace(
        7059,
        "An index with the UNIQUE property was not able to be created because data in the \
         table produced non-unique key values.",
        "Check the Advantage error log for record numbers that caused the failures.",
    ))
}

/// Collects the keys of the order `tag` of the ADI file `path` from the
/// records of `table`, in the order's order, for [`finish_keys`].
pub(crate) fn prepare_keys(table: &Arc<Table>, path: &Path, tag: &str) -> Result<Prepared, Error> {
    let changes = table.file_changes();
    let file = IndexFile::open(path).map_err(|_| Error::CorruptIndex)?;
    let order = orders(0, table, path.to_path_buf(), file, None)
        .into_iter()
        .find(|order| order.tag.name.eq_ignore_ascii_case(tag))
        .ok_or(Error::CorruptIndex)?;
    let active = table.active_slots()?;
    let slots = table.slot_count();
    if active.is_empty() && slots == 0 || order.tag.custom() {
        return Ok(Prepared {
            changes,
            work: None,
        });
    }
    let mut keys = Vec::new();
    if !order.tag.binary() {
        keys = collect_keys(table, &order, active.iter().copied())?;
        keys.sort_by(|a, b| a.bytes.cmp(&b.bytes).then(a.record.cmp(&b.record)));
    }
    Ok(Prepared {
        changes,
        work: Some(Work {
            keys,
            active,
            slots,
            unique: order.tag.unique() && !order.tag.binary(),
            reserve: match table.structure.header.encrypted()
                && matches!(table.structure.header.encryption_type, 5 | 6)
            {
                true => 8,
                false => 0,
            },
        }),
    })
}

/// Builds the full-text order `tag` of the ADI file `path`, just added and
/// empty, from the words of the records of `table`, as the original engine
/// builds one (`fixtures/write/fts_build`): a key for each distinct word of each
/// record not deleted, in the order of their bytes and records, with the
/// word's count in the record if the order keeps scores.
pub(crate) fn build_full_text(table: &Arc<Table>, path: &Path, tag: &str) -> Result<(), Error> {
    let corrupt = |_| Error::CorruptIndex;
    let mut file = IndexFile::open(path).map_err(corrupt)?;
    let kind = file.kind();
    let mut tags = file.tags("");
    // A single-order `.idx` file's tag is named for the file.
    let single = (tags.len() == 1 && kind == Kind::Cdx).then(|| tags[0].clone());
    let tag = tags
        .iter()
        .position(|t| t.name.eq_ignore_ascii_case(tag))
        .map(|at| tags.swap_remove(at))
        .or(single)
        .ok_or(Error::CorruptIndex)?;
    let options = file
        .fts_options(&tag)
        .map_err(corrupt)?
        .ok_or(Error::CorruptIndex)?;
    drop(file);
    let rules = tflip_sql::fts::Rules::from_options(&options);
    let (keys, scores) = full_text_words(table, &tag, &rules)?;
    if kind == Kind::Cdx {
        let counts: std::collections::HashMap<_, _> = keys
            .iter()
            .zip(&scores)
            .map(|(key, &score)| ((key.record, key.bytes.as_slice()), score))
            .collect();
        let score = |key: &tflip_storage::order::Key| {
            counts
                .get(&(key.record, key.bytes.as_slice()))
                .copied()
                .unwrap_or(0)
        };
        let score: Option<&dyn Fn(&tflip_storage::order::Key) -> u16> =
            tag.keeps_score.then_some(&score);
        let pad = Some(tag.pad);
        return tflip_storage::create::cdx_build_tag_scored(
            path,
            tag.header_page,
            pad,
            &keys,
            None,
            score,
        )
        .map_err(corrupt);
    }
    match tag.keeps_score {
        true => tflip_storage::create::build_scored_order(path, &tag.name, &keys, &scores),
        false => tflip_storage::create::build_order(path, &tag.name, &keys, 0, &[]),
    }
    .map_err(corrupt)
}

/// The keys a full-text order `tag` has over the active records of `table`
/// with their word counts, in index order: by the bytes of the word, then
/// the record.
fn full_text_words(
    table: &Arc<Table>,
    tag: &tflip_storage::order::Tag,
    rules: &tflip_sql::fts::Rules,
) -> Result<(Vec<tflip_storage::order::Key>, Vec<u16>), Error> {
    let mut words = Vec::new();
    for slot in table.active_slots()? {
        for (bytes, count) in table.record_words(tag, rules, slot)? {
            words.push((
                tflip_storage::order::Key {
                    bytes,
                    record: slot,
                },
                count,
            ));
        }
    }
    words.sort_by(|(a, _), (b, _)| a.bytes.cmp(&b.bytes).then(a.record.cmp(&b.record)));
    Ok(words.into_iter().unzip())
}

/// Writes the keys [`prepare_keys`] collected as the order `tag` of the ADI
/// file `path`.
pub(crate) fn finish_keys(path: &Path, tag: &str, prepared: Prepared) -> Result<(), Error> {
    let duplicates = prepared.duplicates();
    let Some(Work {
        keys,
        active,
        slots,
        reserve,
        ..
    }) = prepared.work
    else {
        return Ok(());
    };
    if duplicates {
        // The order goes again, its pages left behind, its root emptied.
        tflip_storage::create::clear_order(path, tag).map_err(|_| Error::CorruptIndex)?;
        tflip_storage::create::drop_order(path, tag).map_err(|_| Error::CorruptIndex)?;
        return Err(not_unique());
    }
    let deleted = inactive_slots(&active, slots);
    tflip_storage::create::build_order_reserving(path, tag, &keys, slots, &deleted, reserve)
        .map_err(|_| Error::CorruptIndex)
}

/// The size of a dictionary index's pages.
pub(crate) const DICTIONARY_PAGE_SIZE: u16 = 1024;

/// The seven orders a data dictionary's index (`.ai`) holds over its objects,
/// as the original engine builds them: by id, by type, by name, two that
/// combine an info field with the type and parent, a unique one over parent,
/// type and name, and the names of the database's own objects (those whose
/// parent is the database, record 1). Their expressions are in the
/// field-number form index files store: F1 Object ID, F2 Parent ID, F3 Object
/// Type, F4 Object Name, F7 Info1, F8 Info2.
pub(crate) fn dictionary_orders() -> [tflip_storage::create::OrderSpec; 7] {
    use tflip_storage::create::OrderSpec;
    use tflip_storage::order::options::{CONDITION, UNIQUE};
    let order =
        |name: &str, expression: &str, condition: &str, key_length: u16, pad: u8, options: u8| {
            OrderSpec {
                name: name.to_owned(),
                expression: expression.to_owned(),
                condition: condition.to_owned(),
                key_length,
                options,
                flags: 0,
                pad,
                descending: false,
                collation: String::new(),
                locale: String::new(),
                uchar_width: 0,
                fts: None,
            }
        };
    [
        order("OBJECTID", "F1", "", 8, 0, 0),
        order("OBJECTTYPE", "F3", "", 10, b' ', 0),
        order("OBJECTNAME", "UPPER(F4)", "", 200, b' ', 0),
        order("INFO1", "F7;F3;F2", "", 26, 0, 0),
        order("INFO2", "F8;F3;F2", "", 26, 0, 0),
        order("UNIQUETAG", "F2;F3;UPPER(F4)", "", 218, b' ', UNIQUE),
        order(
            "DBOBJECTNAMES",
            "UPPER(F4)",
            "F2 = 1",
            200,
            b' ',
            UNIQUE | CONDITION,
        ),
    ]
}

/// Builds a data dictionary's index file (`.ai`) at `path`, over the objects
/// of the dictionary `table` (opened with [`table::opening::dictionary`]):
/// the seven system orders and their keys, as the original engine writes
/// them. `collation` is the file's collation CRC (the ANSI one).
pub(crate) fn build_dictionary_index(
    table: &Arc<Table>,
    path: &Path,
    collation: u32,
) -> Result<(), Error> {
    let orders = dictionary_orders();
    // Each order is filled before the next one is made, so that the pages a
    // large dictionary's orders split into follow their order's, as the
    // original lays them (`fixtures/aes/convert`, letter V).
    for spec in &orders {
        tflip_storage::create::add_order(path, spec, DICTIONARY_PAGE_SIZE, collation)
            .map_err(|_| Error::CorruptIndex)?;
        build_keys(table, path, &spec.name)?;
    }
    // A dictionary that encrypts its records with AES enciphers its index.
    tflip_storage::dd::finish_index(&table.path, path).map_err(|_| Error::CorruptIndex)
}

/// Brings a data dictionary's index file up to date with the objects of
/// `table` key by key, as the original engine does after every write: the
/// keys the records no longer have leave their pages and the new ones are
/// inserted, so the pages split and empty as they did through the
/// dictionary's history. `false` when the file is not one to keep up to date
/// (it is missing or enciphered under RC4), which the caller builds anew. An
/// AES dictionary's is put in the clear for the update and enciphered again.
pub(crate) fn update_dictionary_index(table: &Arc<Table>, path: &Path) -> Result<bool, Error> {
    if !path.exists() {
        return Ok(false);
    }
    if tflip_storage::dd::aes_index(&table.path) {
        let mut done = Ok(false);
        tflip_storage::dd::with_clear_index(&table.path, path, || {
            done = update_clear_index(table, path);
            Ok(())
        })
        .map_err(|_| Error::CorruptIndex)?;
        return done;
    }
    if tflip_storage::dd::encrypts_index(&table.path) {
        return Ok(false);
    }
    update_clear_index(table, path)
}

fn update_clear_index(table: &Arc<Table>, path: &Path) -> Result<bool, Error> {
    let Ok(file) = IndexFile::open(path) else {
        return Ok(false);
    };
    let wanted = dictionary_orders();
    let have: Vec<_> = orders(0, table, path.to_path_buf(), file, None);
    if have.len() != wanted.len()
        || !wanted
            .iter()
            .zip(&have)
            .all(|(spec, order)| order.tag.name.eq_ignore_ascii_case(&spec.name))
    {
        return Ok(false);
    }
    let active = table.active_slots()?;
    let mut changes = Vec::new();
    let mut file =
        tflip_storage::adi::IndexFile::open_for_writing(path).map_err(|_| Error::CorruptIndex)?;
    for (at, order) in have.iter().enumerate() {
        let mut old = file.keys(&order.tag).map_err(|_| Error::CorruptIndex)?;
        let mut new = collect_keys(table, order, active.iter().copied())?;
        let ordering = |a: &tflip_storage::order::Key, b: &tflip_storage::order::Key| {
            a.bytes.cmp(&b.bytes).then(a.record.cmp(&b.record))
        };
        old.sort_by(ordering);
        new.sort_by(ordering);
        let (mut i, mut j) = (0, 0);
        while i < old.len() || j < new.len() {
            let step = match (old.get(i), new.get(j)) {
                (Some(o), Some(n)) => ordering(o, n),
                (Some(_), None) => Ordering::Less,
                _ => Ordering::Greater,
            };
            match step {
                Ordering::Equal => {
                    i += 1;
                    j += 1;
                }
                Ordering::Less => {
                    changes.push((old[i].record, at, false, old[i].bytes.clone()));
                    i += 1;
                }
                Ordering::Greater => {
                    changes.push((new[j].record, at, true, new[j].bytes.clone()));
                    j += 1;
                }
            }
        }
    }
    // A record's keys leave its orders before the new ones enter, record by
    // record.
    changes.sort_by_key(|(record, at, insert, _)| (*insert, *record, *at));
    for (record, at, insert, bytes) in changes {
        let tag = &have[at].tag;
        match insert {
            true => file.insert_key(tag, &bytes, record),
            false => file.delete_key(tag, &bytes, record),
        }
        .map_err(|_| Error::CorruptIndex)?;
    }
    Ok(true)
}

/// The orders `owner` opens with (its structural index's, or those of the
/// index files its data dictionary has for it), for reading it without a
/// handle: an SQL statement's lookups. They are behind no handle, and the
/// table does not keep them current, as it only reads.
/// The full-text parsing rules of each of `table`'s fields, by its place: `Some`
/// for a field with a full-text index, `None` otherwise. Read from the table's
/// index files without opening any handle (as [`private_orders`] reads orders),
/// for `CONTAINS`/`SCORE` in SQL.
pub(crate) fn fts_rules(table: &Table) -> Vec<Option<tflip_sql::fts::Rules>> {
    let mut rules: Vec<Option<tflip_sql::fts::Rules>> = vec![None; table.fields().len()];
    let in_transaction = table.has_transaction_records();
    let paths = match &table.index_files {
        Some(files) => files
            .iter()
            .map(|f| crate::shares::actual_case(f.clone()))
            .collect(),
        None => vec![structural_path(&table.path)],
    };
    for path in paths {
        let Ok(mut file) = IndexFile::open(&path) else {
            continue;
        };
        for tag in file.tags("") {
            if !tag.full_text() {
                continue;
            }
            // A full-text order's key is one field, `F<n>`; resolve `n` to the
            // field and its place as the engine does (not a bare `n - 1`).
            // (A DBF table's order names the field.)
            let field = match table.dbf_flavour() {
                Some(_) => {
                    let name = tag.expression.trim();
                    table
                        .fields()
                        .iter()
                        .find(|field| field.name.eq_ignore_ascii_case(name))
                }
                None => tag
                    .expression
                    .trim()
                    .strip_prefix('F')
                    .and_then(|n| n.parse::<usize>().ok())
                    .and_then(|number| table.field(FieldRef::Number(number)).ok()),
            };
            let Some(field) = field else {
                continue;
            };
            let Some(index) = table.fields().iter().position(|f| std::ptr::eq(f, field)) else {
                continue;
            };
            if let Ok(Some(options)) = file.fts_options(&tag) {
                let mut field_rules = tflip_sql::fts::Rules::from_options(&options);
                // A FIXED order answers from its keys, which may no longer be
                // its records' text: read them. So does any order while a
                // transaction holds records of the table: it takes their words
                // in at its commit (`fixtures/write/fts_tx_commit`).
                if (options.options & tflip_sql::fts::FTS_FIXED != 0 || in_transaction)
                    && let Ok(keys) = file.fts_keys(&tag)
                {
                    field_rules.postings = Some(Arc::new(fts_postings(&keys, &tag)));
                }
                rules[index] = Some(field_rules);
            }
        }
    }
    rules
}

/// The postings of a full-text order `tag` whose keys are `keys`: each a word
/// (in the table's code page, padded to the key length) and its record.
fn fts_postings(keys: &[tflip_storage::order::Key], tag: &Tag) -> tflip_sql::fts::Postings {
    let words = keys
        .iter()
        .map(|key| (tflip_storage::cp1252::decode(&key.bytes), key.record));
    tflip_sql::fts::Postings::new(words, usize::from(tag.key_length))
}

impl Index {
    /// The postings of this full-text order (see [`tflip_sql::fts::Postings`]):
    /// what its keys hold for each record.
    pub(crate) fn fts_postings(&self) -> Result<tflip_sql::fts::Postings, Error> {
        let keys = self.with_cursor(|_, file| file.fts_keys(&self.tag))?;
        Ok(fts_postings(&keys, &self.tag))
    }
}

pub(crate) fn private_orders(owner: &Arc<Table>, structural: bool) -> Vec<Arc<Index>> {
    let paths = match &owner.index_files {
        Some(files) => files
            .iter()
            .map(|f| crate::shares::actual_case(f.clone()))
            .collect(),
        None if structural => vec![structural_path(&owner.path)],
        None => Vec::new(),
    };
    let mut out = Vec::new();
    for path in paths {
        if let Ok(file) = IndexFile::open(&path) {
            out.extend(orders(0, owner, path, file, None));
        }
    }
    out
}

/// Completes a CDX order's tag from its table: a key that is not text is
/// padded with zeros, and a VFP collation's short name becomes its name
/// (`GERMAN_VFP_CI_AS_1252`, by the table's code page).
///
/// But a timestamp key of a FoxPro table is padded with spaces: the
/// original engine keeps such an order current, and reads it, as if its
/// keys were text, though it builds it with zeros left out
/// (`fixtures/write/cdx_timestamps`, see docs/ADS_BUGS.md).
fn cdx_tag(tag: &mut Tag, owner: &Table, expression: &str) {
    let visual = owner.dbf_flavour() == Some(tflip_storage::dbf::Flavour::Visual);
    let spaces = owner
        .compile(expression)
        .map_or(true, |expr| match expr.ty() {
            tflip_expr::Type::String { .. } => true,
            tflip_expr::Type::TimeStamp => !visual,
            _ => false,
        });
    if !spaces {
        tag.pad = 0;
    }
    // The header's option 0x04 says candidate on a VFP table, where a custom
    // order is refused, and custom on a FoxPro one, which has no candidate
    // orders (`fixtures/write/cdx_custom_reindex`).
    if !visual && tag.flags & tflip_storage::order::flags::CANDIDATE != 0 {
        tag.flags &= !tflip_storage::order::flags::CANDIDATE;
        tag.options |= tflip_storage::order::options::CUSTOM;
    }
    if !tag.collation.is_empty() {
        let code_page = match owner.dbf.as_ref().map_or(0x03, |info| info.code_page) {
            0xC8 => 1250,
            0xC9 => 1251,
            0xCB => 1253,
            0xCA => 1254,
            _ => 1252,
        };
        let collations = owner.session_settings().collations();
        if let Some(name) = collations.vfp_name(&tag.collation, code_page) {
            tag.collation = name;
        }
    }
}

/// The order behind `handle`.
pub fn get(session: &Session, handle: Handle) -> Result<Arc<Index>, Error> {
    session
        .with(handle, |object| match object {
            Object::Index(index) => Some(Arc::clone(index)),
            _ => None,
        })
        .flatten()
        .ok_or(Error::InvalidIndex)
}

impl Index {
    /// The table the order belongs to.
    pub fn owner(&self) -> &Arc<Table> {
        &self.owner
    }

    /// Starts a raw key (`AdsInitRawKey`): from a record of NULL fields.
    pub fn init_raw_key(&self) {
        *lock(&self.raw_key) = Some(RawKey {
            record: self.owner.null_record(),
            last: None,
        });
    }

    /// Sets `field` of the record the raw key is built from to `value`, as
    /// `AdsSet*` on the order's handle does; a field that is not in the key
    /// changes nothing the key shows.
    pub fn set_raw_key_field(&self, field: FieldRef, value: &SetValue) -> Result<(), Error> {
        let mut raw = lock(&self.raw_key);
        let raw = raw.get_or_insert_with(|| RawKey {
            record: self.owner.null_record(),
            last: None,
        });
        let (field, number) = self.owner.set_in_bytes(field, value, &mut raw.record)?;
        let Some(segment) = self.segments().iter().position(|&n| n == number) else {
            return Ok(());
        };
        let text = match value {
            SetValue::String(text) | SetValue::Field(text) if is_text_field(&field) => {
                Some(RawText {
                    length: tflip_storage::cp1252::encode(text).len(),
                    primary: None,
                })
            }
            SetValue::String(text) | SetValue::Field(text) if is_unicode_field(&field) => {
                tflip_storage::icu::sort_key(tflip_storage::icu::nchar_text(text)).map(|key| {
                    RawText {
                        length: key.len(),
                        primary: Some(key.iter().position(|&b| b == 1).unwrap_or(key.len())),
                    }
                })
            }
            _ => None,
        };
        raw.last = match raw.last {
            Some((last, _)) if last > segment => raw.last,
            _ => Some((segment, text)),
        };
        Ok(())
    }

    /// Sets the whole record the raw key is built from (`AdsSetRecord` on
    /// the order's handle), which must be the table's record length.
    pub fn set_raw_key_record(&self, record: &[u8]) -> Result<(), Error> {
        if record.len() != self.owner.null_record().len() {
            return Err(Error::InvalidRecordLength);
        }
        let last = self
            .segments()
            .len()
            .checked_sub(1)
            .map(|segment| (segment, None));
        *lock(&self.raw_key) = Some(RawKey {
            record: record.to_vec(),
            last,
        });
        Ok(())
    }

    /// The raw key built from the fields set since [`Index::init_raw_key`]
    /// (`AdsBuildRawKey100`), as long as `options` says (see
    /// [`raw_key_length`]); empty if no field of the key was set.
    pub fn build_raw_key(&self, options: u32) -> Result<Vec<u8>, Error> {
        if !matches!(options, 0..=2 | raw_key_length::PRIMARY_WEIGHT) {
            return Err(Error::InvalidOption);
        }
        let raw = lock(&self.raw_key).clone();
        let Some(RawKey { record, last }) = raw else {
            return Ok(Vec::new());
        };
        let mut key = self
            .owner
            .key_of_bytes(self, record)?
            .ok_or(Error::NotImplemented)?;
        // Where each segment of the key starts: a key of several fields in
        // an ADI order is their keys one after the other; any other key is
        // one segment.
        let lengths: Vec<usize> = match (self.kind, self.key_source(&self.owner)) {
            (Kind::Adi, KeySource::Fields(_)) => self
                .segments()
                .iter()
                .map(|&n| {
                    self.owner.field(FieldRef::Number(n)).map_or(0, |field| {
                        tflip_storage::key::field_key_length_wide(field, self.tag.uchar_width)
                    })
                })
                .collect(),
            _ => vec![key.len()],
        };
        // With no segment set the key is empty, unless it is asked for whole
        // (`ADS_GET_FULL_KEY_LENGTH`; the same for `ADS_GET_PARTIAL_FULL_KEY_LENGTH`
        // on a key of one segment): then it is the key of NULLs.
        let Some((segment, text)) = last else {
            return Ok(match (options, lengths.len()) {
                (raw_key_length::FULL, _) | (raw_key_length::PARTIAL_FULL, 1) => key,
                _ => Vec::new(),
            });
        };
        if options == raw_key_length::FULL {
            return Ok(key);
        }
        let segment = segment.min(lengths.len() - 1);
        let start: usize = lengths[..segment].iter().sum();
        // Primary weights are only built for the first segment of an order
        // (`fixtures/write/uchar_keys`); a later Unicode one is not
        // implemented in the original engine.
        let unicode = self
            .segments()
            .get(segment)
            .and_then(|&n| self.owner.field(FieldRef::Number(n)).ok())
            .is_some_and(is_unicode_field);
        if options == raw_key_length::PRIMARY_WEIGHT && unicode && segment > 0 {
            return Err(Error::NotImplemented);
        }
        // A number that is not the key's last segment ends two bytes after
        // the field's own length (Integer and Short probed).
        let number = self
            .segments()
            .get(segment)
            .and_then(|&n| self.owner.field(FieldRef::Number(n)).ok())
            .filter(|field| !is_text_field(field) && !is_unicode_field(field))
            .filter(|_| segment + 1 < lengths.len())
            .map(|field| usize::from(field.length()) + 2);
        let length = match (options, text) {
            (
                raw_key_length::PRIMARY_WEIGHT,
                Some(RawText {
                    primary: Some(n), ..
                }),
            ) => n.min(lengths[segment]),
            (raw_key_length::DEFAULT | raw_key_length::PRIMARY_WEIGHT, Some(text)) => {
                text.length.min(lengths[segment])
            }
            (raw_key_length::DEFAULT, None) => {
                number.map_or(lengths[segment], |n| n.min(lengths[segment]))
            }
            _ => lengths[segment],
        };
        key.truncate(start + length);
        Ok(key)
    }
}

/// Whether `field` holds Unicode text, whose key is an ICU sort key.
fn is_unicode_field(field: &tflip_storage::field::Field) -> bool {
    use tflip_storage::field::FieldType::*;
    matches!(field.field_type, NChar | NVarChar)
}

/// Whether `field` holds text whose key is its bytes, so that a raw key may
/// end after the text given.
fn is_text_field(field: &tflip_storage::field::Field) -> bool {
    use tflip_storage::field::FieldType::*;
    matches!(
        field.field_type,
        Character | CiCharacter | VarChar | LegacyVarChar
    )
}

/// The handles of the open orders of the table `table`, in the order they
/// were opened.
pub fn of_table(session: &Session, table: Handle) -> Vec<Handle> {
    session.owned(
        table,
        |object| matches!(object, Object::Index(index) if index.temporary.is_none()),
    )
}

/// The handles of the table's index orders as clients see them: those of
/// [`of_table`], and the temporary index of an SQL cursor.
fn listed(session: &Session, table: Handle) -> Vec<Handle> {
    session.owned(table, |object| matches!(object, Object::Index(_)))
}

impl Index {
    /// Whether the order is an SQL cursor's temporary index.
    pub fn is_temporary(&self) -> bool {
        self.temporary.is_some()
    }
}

/// Opens the temporary index `spec` (see [`tflip_sql::order_index`]) on
/// the SQL cursor behind `table`, called `name` (`TIM_<n>`), whose file
/// `tim_<n>.tmp` is in the temporary folder, as the original engine's is.
pub(crate) fn open_temporary(
    session: &Session,
    table: Handle,
    name: &str,
    spec: &tflip_sql::TempIndex,
) -> Result<(), Error> {
    use tflip_sql::KeyKind;
    let owner = table::get(session, table)?;
    // A key of fields alone is built as an order of those fields has it,
    // by the numbers of the fields the cursor shows.
    let numbers: Option<Vec<String>> = spec
        .expression
        .split(';')
        .map(|part| {
            let field = owner
                .table_fields()
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case(part.trim()))?;
            let shown = owner.fields().iter().position(|f| f.offset == field.offset);
            Some(format!("F{}", shown? + 1))
        })
        .collect();
    let tag = Tag {
        name: name.to_owned(),
        header_page: 0,
        root: 0,
        key_length: spec.key_length,
        options: 0,
        pad: 0,
        flags: 0,
        descending: spec.descending,
        expression: numbers.map(|n| n.join(";")).unwrap_or_default(),
        condition: String::new(),
        collation: String::new(),
        locale: String::new(),
        uchar_width: 0,
        decimals: 0,
        fts_block: 0,
        keeps_score: false,
    };
    let index = Index {
        table,
        owner,
        path: std::env::temp_dir().join(format!("{}.tmp", name.to_ascii_lowercase())),
        kind: Kind::Adi,
        tag,
        written: Some((spec.expression.clone(), String::new())),
        cipher: None,
        page_size: 512,
        cursor: Mutex::new(None),
        shared: None,
        scope: Mutex::new(Scope::default()),
        reversed: AtomicBool::new(false),
        raw_key: Mutex::new(None),
        fts_rules: std::sync::OnceLock::new(),
        temporary: Some(Temporary {
            flags: spec.flags,
            key_type: match spec.key_kind {
                KeyKind::Logical => KeyType::Logical,
                KeyKind::Numeric => KeyType::Numeric,
                KeyKind::Date => KeyType::Date,
                KeyKind::String => KeyType::String,
                KeyKind::Raw => KeyType::Raw,
            },
        }),
    };
    session.insert(Object::Index(Arc::new(index)), Some(table));
    Ok(())
}

/// The index orders open on the table `table`, full-text ones among them.
pub fn open_on(session: &Session, table: Handle) -> Vec<Arc<Index>> {
    of_table(session, table)
        .into_iter()
        .filter_map(|order| get(session, order).ok())
        .collect()
}

/// The handles of the table's index orders, in the order they were opened:
/// its tags but the full-text (FTS) ones, which are no orders to move in
/// (`AdsGetNumIndexes`); a handle that is not a table is
/// [`Error::InvalidTable`].
pub fn orders_of(session: &Session, table: Handle) -> Result<Vec<Handle>, Error> {
    tags_of(session, table, false)
}

/// The handles of the table's full-text (FTS) indexes
/// (`AdsGetNumFTSIndexes`).
pub fn full_text_of(session: &Session, table: Handle) -> Result<Vec<Handle>, Error> {
    tags_of(session, table, true)
}

fn tags_of(session: &Session, table: Handle, full_text: bool) -> Result<Vec<Handle>, Error> {
    table::get(session, table)?;
    let tags = listed(session, table);
    Ok(tags
        .into_iter()
        .filter(|&tag| get(session, tag).is_ok_and(|index| index.tag.full_text() == full_text))
        .collect())
}

/// Clears the scopes of every order of the table `table`
/// (`AdsClearAllScopes`); a handle that is not a table is
/// [`Error::InvalidTable`].
pub fn clear_all_scopes(session: &Session, table: Handle) -> Result<(), Error> {
    if !matches!(
        handle::kind(session, table),
        Some(handle::HandleKind::Table | handle::HandleKind::Cursor)
    ) {
        return Err(Error::InvalidTable);
    }
    for order in of_table(session, table) {
        if let Ok(order) = get(session, order) {
            *lock(&order.scope) = Scope::default();
        }
    }
    Ok(())
}

/// The order behind `handle` if it is an index order; `None` for a table.
pub fn order_of(session: &Session, handle: Handle) -> Option<Arc<Index>> {
    get(session, handle).ok()
}

/// The table's index orders as the original engine numbers them
/// (`AdsGetIndexHandleByOrder`): in the order they were opened, full-text
/// indexes among them.
pub fn numbered(session: &Session, table: Handle) -> Result<Vec<Handle>, Error> {
    table::get(session, table)?;
    Ok(listed(session, table))
}

/// The open order of the table `table` whose key expression is
/// `expression` (`AdsGetIndexHandleByExpr`): regardless of case and spaces
/// around it, without a condition and ascending, the last opened of
/// several, as in the original engine; full-text indexes are not orders.
pub fn by_expression(
    session: &Session,
    table: Handle,
    expression: &str,
) -> Result<Option<Handle>, Error> {
    let wanted = expression.trim();
    Ok(orders_of(session, table)?.into_iter().rev().find(|&order| {
        get(session, order).is_ok_and(|order| {
            order.expression().trim().eq_ignore_ascii_case(wanted)
                && order.condition().is_empty()
                && order.tag.descending == order.reversed()
                && order.temporary.is_none()
        })
    }))
}

/// The open order named `name` of the table `table`. Names match regardless
/// of case; a table without such an order is [`Error::InvalidIndexOrderName`].
pub fn by_name(session: &Session, table: Handle, name: &str) -> Result<Handle, Error> {
    let opened = table::get(session, table)?;
    // An SQL cursor's own order is the one it reads, if it reads one.
    if name.is_empty() && opened.is_cursor() {
        return match opened.cursor_order() {
            Some(order) => by_name(session, table, order),
            None => Ok(0),
        };
    }
    listed(session, table)
        .into_iter()
        .find(|&order| {
            get(session, order).is_ok_and(|order| order.tag.name.eq_ignore_ascii_case(name))
        })
        .ok_or(Error::InvalidIndexOrderName)
}

/// `AdsGetKeyFilter`: a filter expression that identifies the current record by
/// a unique index — `field=value` for each key field, joined by ` AND ` in key
/// order, as the original engine builds it. The index chosen is the unique one
/// whose tag name sorts alphabetically first; with no unique index the result is
/// [`Error::NotFound`]. `options` bit 1 (`ADS_FILTER_FORMAT_ODATA`) gives the
/// OData form (a single key as a bare value, a composite as `field=value,...`).
/// A values-table file (`pucValuesTable`) is not supported.
pub fn key_filter(
    session: &Session,
    table: Handle,
    values_table: Option<&str>,
    options: u32,
) -> Result<String, Error> {
    use tflip_storage::field::FieldType;
    if values_table.is_some_and(|v| !v.is_empty()) {
        return Err(Error::NotImplemented);
    }
    // Only the OData (0x1) and UTF-8 (0x2) option bits are valid.
    if options & !0x3 != 0 {
        return Err(Error::InvalidOption);
    }
    let owner = table::get(session, table)?;
    // The key fields: the unique index whose tag name sorts first, or — with no
    // unique index — the table's AutoInc field, else [`Error::NotFound`].
    let mut unique: Vec<Arc<Index>> = orders_of(session, table)?
        .into_iter()
        .filter_map(|handle| order_of(session, handle))
        .filter(|order| order.tag.unique())
        .collect();
    unique.sort_by_key(|order| order.tag.name.to_ascii_uppercase());
    let fields: Vec<usize> = match unique.first() {
        Some(order) => order
            .tag
            .expression
            .split(';')
            // A plain field reference `F<n>`; an expression key is not a filter.
            .map(|part| {
                part.trim()
                    .strip_prefix(['F', 'f'])
                    .and_then(|digits| digits.parse().ok())
                    .ok_or(Error::NotImplemented)
            })
            .collect::<Result<_, _>>()?,
        None => {
            let mut autoinc = None;
            for number in 1.. {
                match owner.field(FieldRef::Number(number)) {
                    Ok(field) if field.field_type == FieldType::AutoInc => {
                        autoinc = Some(number);
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            vec![autoinc.ok_or(Error::NotFound)?]
        }
    };
    let odata = options & 0x1 != 0;
    let bare = odata && fields.len() == 1;
    // Off a record (BOF/EOF) the engine writes each key field as `field=null`.
    let no_record = owner.position().record.is_none();
    let mut out = String::new();
    for (position, &number) in fields.iter().enumerate() {
        let field = owner.field(FieldRef::Number(number))?;
        let (name, field_type) = (field.name.clone(), field.field_type);
        if position > 0 {
            out.push_str(if odata { "," } else { " AND " });
        }
        if !bare {
            out.push_str(&name);
            out.push('=');
        }
        if no_record {
            out.push_str("null");
        } else {
            let value = owner.value(FieldRef::Number(number))?;
            out.push_str(&key_filter_value(field_type, &value, odata));
        }
    }
    Ok(out)
}

/// One key field's value, as [`key_filter`] writes it (verified against the
/// engine, see the `getkeyfilter` note): text single-quoted and right-trimmed,
/// numbers bare, a Double in C `%E` form, Money with four decimals, a Logical as
/// `true`/`false`, a Date/TimeStamp as `datetime'YYYY-MM-DD HH:MM:SS'`. A NULL
/// field takes the type's zero value, as the engine emits it.
fn key_filter_value(
    field_type: tflip_storage::field::FieldType,
    value: &Value,
    odata: bool,
) -> String {
    use tflip_storage::field::FieldType;
    // In the OData form a Money/Numeric value takes an `M` suffix and an AutoInc
    // value an `L`; the date/timestamp separator is `T` instead of a space.
    let money_suffix = if odata { "M" } else { "" };
    match value {
        Value::Null => match field_type {
            FieldType::Double | FieldType::CurDouble => msvc_scientific(0.0),
            FieldType::Logical => "false".to_owned(),
            FieldType::Character
            | FieldType::CiCharacter
            | FieldType::VarChar
            | FieldType::NChar
            | FieldType::NVarChar => "''".to_owned(),
            FieldType::Date | FieldType::TimeStamp | FieldType::ModTime => datetime(0, 0, odata),
            // Integer, ShortInt, Money, Numeric, Time, ... emit a bare zero.
            _ => "0".to_owned(),
        },
        Value::Text(text) => format!("'{}'", text.trim_end_matches(' ')),
        Value::Logical(flag) => if *flag { "true" } else { "false" }.to_owned(),
        Value::Integer(number) => {
            let suffix = if odata && field_type == FieldType::AutoInc {
                "L"
            } else {
                ""
            };
            format!("{number}{suffix}")
        }
        Value::Numeric(digits) => format!("{}{money_suffix}", digits.trim()),
        Value::Double(number) => msvc_scientific(*number),
        Value::Money(amount) => {
            let sign = if *amount < 0 { "-" } else { "" };
            let magnitude = amount.unsigned_abs();
            format!(
                "{sign}{}.{:04}{money_suffix}",
                magnitude / 10_000,
                magnitude % 10_000
            )
        }
        Value::Date(julian) => datetime(*julian, 0, odata),
        Value::TimeStamp(julian, millis) => datetime(*julian, *millis, odata),
        Value::Time(millis) => datetime(0, *millis, odata),
        // Binary, GUID and blanks are not plain key values.
        _ => String::new(),
    }
}

/// A Double as C's `printf("%E")` writes it on Windows: six fraction digits, an
/// uppercase `E`, a sign, and at least three exponent digits.
fn msvc_scientific(value: f64) -> String {
    let formatted = format!("{value:.6E}");
    let (mantissa, exponent) = formatted.split_once('E').unwrap_or((&formatted, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let sign = if exponent < 0 { '-' } else { '+' };
    format!("{mantissa}E{sign}{:03}", exponent.unsigned_abs())
}

/// A Julian day and milliseconds since midnight as `datetime'YYYY-MM-DD HH:MM:SS'`,
/// the date and time joined by a space, or by `T` in the OData form.
fn datetime(julian: i32, millis: u32, odata: bool) -> String {
    let date = tflip_storage::value::julian_to_ymd(julian);
    let seconds = millis / 1000;
    let separator = if odata { 'T' } else { ' ' };
    format!(
        "datetime'{:04}-{:02}-{:02}{separator}{:02}:{:02}:{:02}'",
        date.year,
        date.month,
        date.day,
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60,
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::collations::Collations;
    use crate::connection;
    use crate::table::{self, TableSettings, TableType};

    /// The fixtures' folder, named as a connection names it.
    fn fixtures() -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt");
        PathBuf::from(tflip_storage::universal_path(&dir.to_string_lossy()))
    }

    fn adt() -> TableSettings {
        TableSettings {
            table_type: TableType::Adt,
            char_type: 1,
            collation: String::new(),
            options: 0,
            ..TableSettings::default()
        }
    }

    #[test]
    fn structural_indexes_open_with_their_table() {
        let session = Session::new();
        let dir = fixtures().join("indexes");
        let connect = connection::connect(
            &session,
            &format!("Data Source={};ServerType=LOCAL;", dir.display()),
        )
        .unwrap();
        let table = table::open(&session, connect, "indexed.adt", adt()).unwrap();
        let orders = of_table(&session, table);
        let names: Vec<String> = orders
            .iter()
            .map(|&h| get(&session, h).unwrap().tag.name.clone())
            .collect();
        assert_eq!(
            names,
            [
                "PK_INDEXED",
                "BY_NAME",
                "BY_AMOUNT_DESC",
                "BY_BORN_NAME",
                "BY_CINAME"
            ]
        );
        assert_eq!(by_name(&session, table, "by_name"), Ok(orders[1]));
        assert_eq!(by_name(&session, table, "BY_NAME"), Ok(orders[1]));
        assert_eq!(
            by_name(&session, table, "nope"),
            Err(Error::InvalidIndexOrderName)
        );
        assert_eq!(
            by_name(&session, connect, "BY_NAME"),
            Err(Error::InvalidTable)
        );

        // BY_BORN_NAME is `born;name`.
        let born_name = get(&session, orders[3]).unwrap();
        assert_eq!(born_name.expression(), "born;name");
        assert_eq!(born_name.condition(), "");
        assert_eq!(born_name.segment_field(0).unwrap(), "born");
        assert_eq!(born_name.segment_field(1).unwrap(), "name");
        assert_eq!(born_name.segment_field(2), Err(Error::InvalidFieldNumber));
        let info = born_name.info();
        assert_eq!((info.table, info.name.as_str()), (table, "BY_BORN_NAME"));
        assert_eq!(
            (info.expression.as_str(), info.condition.as_str()),
            ("born;name", "")
        );
        assert_eq!((info.key_type, info.segments.len()), (KeyType::Raw, 2));
        let first = get(&session, orders[0]).unwrap();
        assert_eq!(first.table, table);
        assert_eq!(first.path, dir.join("indexed.adi"));
        assert_eq!(first.page_size, 512);
        assert_eq!(get(&session, table).unwrap_err(), Error::InvalidIndex);

        let big = table::open(&session, connect, "pages8k.adt", adt()).unwrap();
        assert_eq!(
            get(&session, of_table(&session, big)[0]).unwrap().page_size,
            8192
        );

        // An order's handle stands for its table in table functions.
        assert!(Arc::ptr_eq(
            &table::get(&session, orders[0]).unwrap(),
            &table::get(&session, table).unwrap()
        ));

        // Closing the table closes its orders, and so does disconnecting.
        table::close(&session, table).unwrap();
        assert_eq!(get(&session, orders[0]).unwrap_err(), Error::InvalidIndex);
        let big_order = of_table(&session, big)[0];
        connection::disconnect(&session, connect).unwrap();
        assert_eq!(get(&session, big_order).unwrap_err(), Error::InvalidIndex);
    }

    /// An index whose keys a different static collation weighed makes the table
    /// fail to open (`AE_INDEX_COLLATION_MISMATCH`): the `charset_german` `ca`
    /// table, whose structural ADI was built under `ANSI_CHAR_SET=German`, is
    /// refused under the default collation and opens under German. Confirmed
    /// against the engine with `tools/ace/expr/openmismatch.c` (5175 vs 0).
    #[test]
    fn an_index_of_another_static_collation_is_refused() {
        let open_ca = |session: &Session, folder: &str| {
            let dir = fixtures().join(folder);
            let connect = connection::connect(
                session,
                &format!("Data Source={};ServerType=LOCAL;", dir.display()),
            )
            .unwrap();
            table::open(session, connect, "ca.adt", adt()).map(|_| ())
        };

        // The default collation is identity and needs no collation files, so
        // the German-built index does not match it: the open is refused, while
        // a default-built table opens.
        assert_eq!(
            open_ca(&Session::new(), "charset_german"),
            Err(Error::IndexCollationMismatch),
        );
        assert_eq!(open_ca(&Session::new(), "charset_default"), Ok(()));

        // Under German (where this machine has the collation files) the German
        // table opens and a default-built one is refused.
        let collation_dir = std::env::var_os("TFLIP_COLLATION_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                let home = PathBuf::from(std::env::var_os("HOME")?);
                Some(home.join("src/ace-sdk/drive_c/Program Files (x86)/Advantage 12.0/acesdk"))
            })
            .filter(|dir| dir.join("ansi.chr").exists());
        if let Some(dir) = collation_dir {
            let session = Session::new();
            crate::settings::set_collations(&session, Collations::new(&dir, "German", ""));
            assert_eq!(open_ca(&session, "charset_german"), Ok(()));
            assert_eq!(
                open_ca(&session, "charset_default"),
                Err(Error::IndexCollationMismatch),
            );
        }
    }

    /// The records from the current position on, moving one at a time in
    /// `order` until EOF or BOF.
    fn walk(table: &table::Table, order: &Index, forward: bool) -> Vec<u32> {
        let mut records = Vec::new();
        while let Some(record) = table.position().record {
            records.push(record);
            table.skip_in(order, if forward { 1 } else { -1 }).unwrap();
        }
        records
    }

    /// Every order's key type and length, as the original engine reports
    /// them in the fixtures.
    #[test]
    fn key_types_match_the_engine() {
        let session = Session::new();
        let mut checked = 0;
        for dir in std::fs::read_dir(fixtures()).unwrap() {
            for entry in std::fs::read_dir(dir.unwrap().path()).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().is_none_or(|e| e != "adt") {
                    continue;
                }
                let json = std::fs::read_to_string(path.with_extension("json")).unwrap();
                let dump: serde_json::Value = serde_json::from_str(&json).unwrap();
                let Some(expected) = dump["indexes"].as_array() else {
                    continue;
                };
                let Ok(table) = table::open(&session, 0, path.to_str().unwrap(), adt()) else {
                    continue;
                };
                for order in of_table(&session, table)
                    .into_iter()
                    .map(|h| get(&session, h).unwrap())
                {
                    let Some(want) = expected
                        .iter()
                        .find(|o| o["name"] == order.tag.name.as_str())
                    else {
                        continue;
                    };
                    let at = format!("{} {}", path.display(), order.tag.name);
                    let key_type = match order.key_type() {
                        KeyType::Logical => 1,
                        KeyType::Numeric => 2,
                        KeyType::Date => 3,
                        KeyType::String => 4,
                        KeyType::Raw => 16,
                    };
                    assert_eq!(key_type, want["key_type"], "{at}");
                    assert_eq!(u64::from(order.tag.key_length), want["key_length"], "{at}");
                    checked += 1;
                }
                table::close(&session, table).unwrap();
            }
        }
        assert!(checked >= 50, "checked only {checked} orders");
    }

    #[test]
    fn moves_in_key_order_match_the_engine() {
        let session = Session::new();
        let mut checked = 0;
        for name in [
            "indexes/indexed.adt",
            "adi_tree/grown.adt",
            "adi_options/opts.adt",
        ] {
            let path = fixtures().join(name);
            let json = std::fs::read_to_string(path.with_extension("json")).unwrap();
            let dump: serde_json::Value = serde_json::from_str(&json).unwrap();
            let table = table::open(&session, 0, path.to_str().unwrap(), adt()).unwrap();
            let opened = table::get(&session, table).unwrap();
            for (order, expected) in of_table(&session, table)
                .iter()
                .zip(dump["indexes"].as_array().unwrap())
            {
                let index = get(&session, *order).unwrap();
                let Some(keys) = expected["keys"].as_array() else {
                    continue;
                };
                let want: Vec<u32> = keys
                    .iter()
                    .map(|k| k["recno"].as_u64().unwrap() as u32)
                    .collect();
                let at = format!("{name} {}", index.tag.name);
                let order = &index;
                opened.goto_top_in(order).unwrap();
                assert_eq!(walk(&opened, order, true), want, "{at}");
                assert!(opened.position().eof, "{at}");
                opened.skip_in(order, -1).unwrap();
                let mut back = walk(&opened, order, false);
                back.reverse();
                assert_eq!(back, want, "{at}");
                assert!(opened.position().bof, "{at}");
                opened.goto_bottom_in(order).unwrap();
                assert_eq!(opened.position().record, want.last().copied(), "{at}");
                if want.len() > 3 {
                    opened.goto_top_in(order).unwrap();
                    opened.skip_in(order, 3).unwrap();
                    assert_eq!(opened.position().record, Some(want[3]), "{at}");
                    assert_eq!(opened.key_number_in(order, true), Ok(4), "{at}");
                    // The same record, reached in record order: the order
                    // finds it again.
                    opened.goto_record(want[1]).unwrap();
                    assert_eq!(opened.key_number_in(order, true), Ok(2), "{at}");
                    opened.skip_in(order, 1).unwrap();
                    assert_eq!(opened.position().record, Some(want[2]), "{at}");
                }
                opened.goto_bottom_in(order).unwrap();
                opened.skip_in(order, 1).unwrap();
                // As in the original engine, an order without keys too: a
                // move leaves it at EOF only, numbered 1.
                assert_eq!(
                    opened.key_number_in(order, true),
                    Ok(want.len() as u32 + 1),
                    "{at}: EOF"
                );
                opened.goto_top_in(order).unwrap();
                opened.skip_in(order, -1).unwrap();
                assert_eq!(opened.key_number_in(order, true), Ok(0), "{at}: BOF");
                checked += 1;
            }
            table::close(&session, table).unwrap();
        }
        assert!(checked >= 10, "checked only {checked} orders");
    }

    /// An OEM Clipper or FoxPro table (opened with `ADS_OEM`) weighs its NTX
    /// and CDX keys by the OEM static collation (`OEM_CHAR_SET`, `extend.chr`)
    /// and an ANSI one by the ANSI collation (`ANSI_CHAR_SET`, `ansi.chr`), so
    /// each navigates in the order the original engine's index records. The
    /// `charset_german` fixtures hold both: `ca` is ANSI (`ANSI_CHAR_SET=German`)
    /// and `co` OEM (`OEM_CHAR_SET=GERMAN`), with the same words in each. Were
    /// an OEM table to weigh by the ANSI collation (or the other way around),
    /// the walked order would not be the golden one.
    #[test]
    fn oem_tables_navigate_by_the_oem_collation() {
        let collation_dir = std::env::var_os("TFLIP_COLLATION_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                let home = PathBuf::from(std::env::var_os("HOME")?);
                Some(home.join("src/ace-sdk/drive_c/Program Files (x86)/Advantage 12.0/acesdk"))
            });
        let Some(dir) = collation_dir.filter(|dir| dir.join("extend.chr").exists()) else {
            return;
        };
        let session = Session::new();
        // German in both: ANSI_CHAR_SET=German, OEM_CHAR_SET=GERMAN.
        crate::settings::set_collations(&session, Collations::new(&dir, "German", "GERMAN"));
        let root = fixtures().join("..");
        let mut checked = 0;
        for (kind, table_type) in [("ntx", TableType::Ntx), ("cdx", TableType::Cdx)] {
            for (stem, char_type) in [("ca", 1), ("co", 2)] {
                let path = root
                    .join(kind)
                    .join("charset_german")
                    .join(format!("{stem}.dbf"));
                let json = std::fs::read_to_string(path.with_extension("json")).unwrap();
                let dump: serde_json::Value = serde_json::from_str(&json).unwrap();
                let settings = TableSettings {
                    table_type,
                    char_type,
                    ..adt()
                };
                let table = table::open(&session, 0, path.to_str().unwrap(), settings).unwrap();
                let indexes = dump["indexes"].as_array().unwrap();
                let mut files: Vec<&str> = indexes
                    .iter()
                    .map(|i| i["file"].as_str().unwrap())
                    .collect();
                files.dedup();
                let structural = path.with_extension("cdx");
                for file in files {
                    if path.parent().unwrap().join(file) != structural {
                        open_file(&session, table, file).unwrap();
                    }
                }
                let opened = table::get(&session, table).unwrap();
                for expected in indexes {
                    let name = expected["name"].as_str().unwrap();
                    let at = format!("{} {name}", path.display());
                    let order = &get(&session, by_name(&session, table, name).unwrap()).unwrap();
                    let want: Vec<u32> = expected["keys"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|k| k["recno"].as_u64().unwrap() as u32)
                        .collect();
                    opened.goto_top_in(order).unwrap();
                    assert_eq!(walk(&opened, order, true), want, "{at} forward");
                    opened.goto_bottom_in(order).unwrap();
                    let mut back = walk(&opened, order, false);
                    back.reverse();
                    assert_eq!(back, want, "{at} backward");
                    checked += 1;
                }
                table::close(&session, table).unwrap();
            }
        }
        assert_eq!(checked, 8, "checked {checked} orders");
    }

    /// The keys tflip builds from records are the ones the original
    /// engine's `AdsExtractKey` returned, in every order of every fixture
    /// but those whose keys are collation weights (and full-text, custom
    /// and binary ones, which have no such keys); records an order's
    /// condition leaves out have none.
    #[test]
    fn record_keys_match_the_engine() {
        let session = Session::new();
        // The original engine's collation files, where this machine has them.
        let collation_dir = std::env::var_os("TFLIP_COLLATION_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                let home = PathBuf::from(std::env::var_os("HOME")?);
                Some(home.join("src/ace-sdk/drive_c/Program Files (x86)/Advantage 12.0/acesdk"))
            });
        let collation_dir = collation_dir.filter(|dir| dir.join("adscollate.adt").exists());
        if let Some(dir) = &collation_dir {
            crate::settings::set_collations(&session, Collations::new(dir, "", ""));
        }
        let (mut matched, mut skipped) = (0, Vec::new());
        let mut dumps: Vec<PathBuf> = std::fs::read_dir(fixtures())
            .unwrap()
            .flat_map(|dir| std::fs::read_dir(dir.unwrap().path()).unwrap())
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|e| e == "json"))
            .filter(|path| path.with_extension("adt").exists())
            .collect();
        dumps.sort();
        for json in dumps {
            let dump: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&json).unwrap()).unwrap();
            // A static collation (ANSI_CHAR_SET) is the server's setting,
            // which the files do not record: tflip builds the default
            // collation's keys there.
            if dump["ansi_char_set"] != "" && dump["ansi_char_set"].is_string() {
                continue;
            }
            let path = json.with_extension("adt");
            // Opened with the dynamic collation its orders have, as the
            // original engine asks.
            let collation = dump["indexes"]
                .as_array()
                .unwrap()
                .iter()
                .find_map(|i| i["collation"].as_str().filter(|c| !c.is_empty()))
                .unwrap_or_default();
            let settings = TableSettings {
                collation: collation.to_owned(),
                ..adt()
            };
            let Ok(table) = table::open(&session, 0, path.to_str().unwrap(), settings) else {
                continue;
            };
            let opened = table::get(&session, table).unwrap();
            for (order, expected) in of_table(&session, table)
                .iter()
                .zip(dump["indexes"].as_array().unwrap())
            {
                let index = get(&session, *order).unwrap();
                let at = format!("{} {}", path.display(), index.tag.name);
                let Some(keys) = expected["keys"].as_array() else {
                    continue;
                };
                if index.tag.binary() || expected["custom"] == true || expected["fts"] == true {
                    continue;
                }
                let mut listed = BTreeSet::new();
                for key in keys {
                    let record = key["recno"].as_u64().unwrap() as u32;
                    listed.insert(record);
                    let want = key["key"].as_str().unwrap();
                    match opened.record_key(&index, record) {
                        Ok(Some(got)) => {
                            let got: String = got.iter().map(|b| format!("{b:02x}")).collect();
                            assert_eq!(got, want, "{at} record {record}");
                            matched += 1;
                        }
                        Ok(None) => panic!("{at} record {record}: no key"),
                        Err(Error::NotImplemented) => {
                            skipped.push(at.clone());
                            break;
                        }
                        Err(error) => panic!("{at} record {record}: {error:?}"),
                    }
                }
                if !index.condition().is_empty() {
                    for record in dump["records"].as_array().unwrap() {
                        let number = record["recno"].as_u64().unwrap() as u32;
                        if record["deleted"] == false && !listed.contains(&number) {
                            assert_eq!(opened.record_key(&index, number), Ok(None), "{at}");
                        }
                    }
                }
            }
            table::close(&session, table).unwrap();
        }
        skipped.dedup();
        eprintln!("{matched} keys match; skipped: {skipped:?}");
        assert!(matched >= 500, "only {matched} keys matched");
    }

    /// The keys tflip builds from the records of DBF tables are the ones in
    /// the original engine's NTX, CDX and IDX orders (the dumps of
    /// `fixtures/{ntx,cdx,vfp}`), in every order but binary ones and those
    /// under a static collation the files do not record; records an
    /// order's condition leaves out have none.
    #[test]
    fn dbf_record_keys_match_the_engine() {
        let session = Session::new();
        let collation_dir = std::env::var_os("TFLIP_COLLATION_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                let home = PathBuf::from(std::env::var_os("HOME")?);
                Some(home.join("src/ace-sdk/drive_c/Program Files (x86)/Advantage 12.0/acesdk"))
            });
        let collation_dir = collation_dir.filter(|dir| dir.join("adscollate.adt").exists());
        if let Some(dir) = &collation_dir {
            crate::settings::set_collations(&session, Collations::new(dir, "", ""));
        }
        let (mut matched, mut skipped, mut problems) = (0, Vec::new(), Vec::new());
        let root = fixtures().join("..");
        let mut dumps: Vec<PathBuf> = ["ntx", "cdx", "vfp"]
            .iter()
            .flat_map(|kind| std::fs::read_dir(root.join(kind)).unwrap())
            .flat_map(|dir| std::fs::read_dir(dir.unwrap().path()).unwrap())
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|e| e == "json"))
            .filter(|path| path.with_extension("dbf").exists())
            .collect();
        dumps.sort();
        for json in dumps {
            let dump: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&json).unwrap()).unwrap();
            if dump["ansi_char_set"] != "" && dump["ansi_char_set"].is_string() {
                continue;
            }
            let table_type = match dump["table_type"].as_str().unwrap() {
                "ntx" => TableType::Ntx,
                "cdx" => TableType::Cdx,
                _ => TableType::Vfp,
            };
            let path = json.with_extension("dbf");
            let indexes = dump["indexes"].as_array().unwrap();
            let collation = indexes
                .iter()
                .find_map(|i| i["collation"].as_str().filter(|c| !c.is_empty()))
                .unwrap_or_default();
            let settings = TableSettings {
                table_type,
                collation: collation.to_owned(),
                char_type: if dump["char_type"] == "oem" { 2 } else { 1 },
                ..adt()
            };
            let table = table::open(&session, 0, path.to_str().unwrap(), settings).unwrap();
            let structural = path.with_extension("cdx");
            let mut files: Vec<&str> = indexes
                .iter()
                .map(|i| i["file"].as_str().unwrap())
                .collect();
            files.dedup();
            for file in files {
                if path.parent().unwrap().join(file) != structural {
                    open_file(&session, table, file).unwrap();
                }
            }
            let opened = table::get(&session, table).unwrap();
            for expected in indexes {
                let name = expected["name"].as_str().unwrap();
                let at = format!("{} {name}", path.display());
                let order = by_name(&session, table, name).unwrap();
                let index = get(&session, order).unwrap();
                let Some(keys) = expected["keys"].as_array() else {
                    continue;
                };
                if index.tag.binary() {
                    continue;
                }
                let mut listed = BTreeSet::new();
                for key in keys {
                    let record = key["recno"].as_u64().unwrap() as u32;
                    listed.insert(record);
                    let want = key["key"].as_str().unwrap();
                    match opened.record_key(&index, record) {
                        Ok(Some(got)) => {
                            let got: String = got.iter().map(|b| format!("{b:02x}")).collect();
                            match got == want {
                                true => matched += 1,
                                false => problems.push(format!(
                                    "{at} record {record}: got {got}, expected {want}"
                                )),
                            }
                        }
                        Ok(None) if expected["unique"] == true => {}
                        Ok(None) => problems.push(format!("{at} record {record}: no key")),
                        Err(Error::NotImplemented) => {
                            skipped.push(at.clone());
                            break;
                        }
                        Err(error) => problems.push(format!("{at} record {record}: {error:?}")),
                    }
                }
                if !index.condition().is_empty() {
                    for record in dump["records"].as_array().unwrap() {
                        let number = record["recno"].as_u64().unwrap() as u32;
                        if !listed.contains(&number)
                            && opened.record_key(&index, number) != Ok(None)
                        {
                            problems.push(format!("{at} record {number}: a key"));
                        }
                    }
                }
            }
            table::close(&session, table).unwrap();
        }
        skipped.dedup();
        eprintln!("{matched} keys match; skipped: {skipped:?}");
        problems.truncate(60);
        assert!(problems.is_empty(), "{}", problems.join("\n"));
        assert!(matched >= 10_000, "only {matched} keys matched");
    }

    #[test]
    fn conditions_name_their_fields() {
        let session = Session::new();
        let path = fixtures().join("adi_options/opts.adt");
        let table = table::open(&session, 0, path.to_str().unwrap(), adt()).unwrap();
        let cond = get(&session, by_name(&session, table, "COND").unwrap()).unwrap();
        assert_eq!(cond.expression(), "id");
        assert_eq!(cond.condition(), "id > 30");
        table::close(&session, table).unwrap();
    }

    #[test]
    fn key_counts_match_the_engine() {
        let session = Session::new();
        for (name, order, count) in [
            ("adi_options/bits.adt", "DELETED", 5001),
            ("adi_options/opts.adt", "COND", 8),
            ("adi_options/opts.adt", "CUST", 0),
            ("adi_tree/grown.adt", "BY_WORD", 2700),
        ] {
            let path = fixtures().join(name);
            let table = table::open(&session, 0, path.to_str().unwrap(), adt()).unwrap();
            let index = get(&session, by_name(&session, table, order).unwrap()).unwrap();
            assert_eq!(index.key_count(true), Ok(count), "{name} {order}");
            table::close(&session, table).unwrap();
        }
    }

    #[test]
    fn empty_orders_are_at_both_ends() {
        let session = Session::new();
        // CUST is a custom order without keys in a table with records.
        let path = fixtures().join("adi_options/opts.adt");
        let table = table::open(&session, 0, path.to_str().unwrap(), adt()).unwrap();
        let custom = by_name(&session, table, "CUST").unwrap();
        let order = get(&session, custom).unwrap();
        let opened = table::get(&session, table).unwrap();
        let both = table::Position {
            record: None,
            bof: true,
            eof: true,
        };
        opened.goto_top_in(&order).unwrap();
        assert_eq!(opened.position(), both);
        opened.goto_bottom_in(&order).unwrap();
        assert_eq!(opened.position(), both);
        table::close(&session, table).unwrap();
    }

    #[test]
    fn tables_without_an_index_have_no_orders() {
        let session = Session::new();
        let path = fixtures().join("all_types/all_types.adt");
        let table = table::open(&session, 0, path.to_str().unwrap(), adt()).unwrap();
        assert!(of_table(&session, table).is_empty());
        table::close(&session, table).unwrap();
    }

    #[test]
    fn segments_of_expressions() {
        // As the original engine counts them: distinct fields, in order of
        // first appearance.
        assert_eq!(segments("F2"), [2]);
        assert_eq!(segments("UPPER(F2)"), [2]);
        assert_eq!(segments("F1;UPPER(F2);F5"), [1, 2, 5]);
        assert_eq!(segments("STR(F1,5)+F2"), [1, 2]);
        assert_eq!(segments("F2+F2"), [2]);
        assert_eq!(segments("RECNO()"), [] as [usize; 0]);
        assert_eq!(segments("F1;RECNO()"), [1]);
        // Not fields: inside quotes, or part of a longer word.
        assert_eq!(segments("IIF(F12 > 1, 'F3', F2)"), [12, 2]);
        assert_eq!(segments("IF1(F3) + DF4 + F5X"), [3]);
    }

    #[test]
    fn structural_paths() {
        assert_eq!(
            structural_path(Path::new("/d/t.adt")),
            Path::new("/d/t.adi")
        );
        assert_eq!(
            structural_path(Path::new("/d/T.ADT")),
            Path::new("/d/T.ADI")
        );
    }

    #[test]
    fn a_new_dictionary_index_matches_the_engine() {
        use tflip_storage::dd::create::{NewDictRandom, new_dictionary_add, new_dictionary_memo};
        let golden = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/dictionary/new/empty.ai"),
        )
        .unwrap();

        let dir =
            tflip_storage::testing::temp_root().join(format!("tflip-dd-ai-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let add_path = dir.join("empty.add");
        let random = NewDictRandom {
            secret_first: [0x5a; 20],
            secret_last: [0xa5; 20],
            slot0: [0x11; 20],
            slot4: [0x22; 20],
        };
        std::fs::write(&add_path, new_dictionary_add(&random).unwrap()).unwrap();
        std::fs::write(dir.join("empty.am"), new_dictionary_memo()).unwrap();

        let session = Session::new();
        let table = Arc::new(table::dictionary(&session, &add_path).unwrap());
        let weights = table
            .session_settings()
            .collations()
            .ansi()
            .map(|c| c.weights);
        let collation = tflip_storage::create::collation_crc(weights.as_ref());
        let ai_path = dir.join("empty.ai");
        build_dictionary_index(&table, &ai_path, collation).unwrap();

        let ours = std::fs::read(&ai_path).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(ours.len(), golden.len(), "index length");
        let diffs = ours.iter().zip(&golden).filter(|(a, b)| a != b).count();
        assert_eq!(
            diffs, 0,
            "the dictionary index differs from the engine in {diffs} bytes"
        );
    }

    #[test]
    fn key_filter_values_format_like_the_engine() {
        use tflip_storage::field::FieldType;
        use tflip_storage::value::{Value, Ymd, ymd_to_julian};
        // The scientific form is MSVC printf("%E"): six fraction digits, a sign,
        // three exponent digits.
        assert_eq!(msvc_scientific(3.25), "3.250000E+000");
        assert_eq!(msvc_scientific(0.0), "0.000000E+000");
        assert_eq!(msvc_scientific(-1.5), "-1.500000E+000");
        let v = |t, value: &Value| key_filter_value(t, value, false);
        let od = |t, value: &Value| key_filter_value(t, value, true);
        assert_eq!(v(FieldType::Double, &Value::Double(3.25)), "3.250000E+000");
        assert_eq!(v(FieldType::Money, &Value::Money(125_000)), "12.5000");
        assert_eq!(v(FieldType::Logical, &Value::Logical(true)), "true");
        assert_eq!(v(FieldType::Logical, &Value::Logical(false)), "false");
        assert_eq!(v(FieldType::Integer, &Value::Integer(-7)), "-7");
        assert_eq!(
            v(FieldType::Character, &Value::Text("BBB     ".to_owned())),
            "'BBB'"
        );
        let julian = ymd_to_julian(Ymd {
            year: 2024,
            month: 1,
            day: 15,
        })
        .unwrap();
        assert_eq!(
            v(FieldType::Date, &Value::Date(julian)),
            "datetime'2024-01-15 00:00:00'"
        );
        assert_eq!(
            v(
                FieldType::TimeStamp,
                &Value::TimeStamp(julian, 13 * 3_600_000 + 45 * 60_000 + 30_000)
            ),
            "datetime'2024-01-15 13:45:30'",
        );
        // A NULL key field takes the type's zero value, as the engine emits it.
        assert_eq!(v(FieldType::Integer, &Value::Null), "0");
        assert_eq!(v(FieldType::Money, &Value::Null), "0");
        assert_eq!(v(FieldType::Double, &Value::Null), "0.000000E+000");
        assert_eq!(v(FieldType::Logical, &Value::Null), "false");
        assert_eq!(v(FieldType::Character, &Value::Null), "''");
        // The OData form: Money/Numeric take `M`, AutoInc `L`, dates a `T`.
        assert_eq!(od(FieldType::Money, &Value::Money(125_000)), "12.5000M");
        assert_eq!(od(FieldType::AutoInc, &Value::Integer(2)), "2L");
        assert_eq!(od(FieldType::Integer, &Value::Integer(2)), "2");
        assert_eq!(
            od(FieldType::Date, &Value::Date(julian)),
            "datetime'2024-01-15T00:00:00'"
        );
    }

    #[test]
    fn key_filter_matches_the_engine() {
        let session = Session::new();
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/keyfilter");
        let connect = connection::connect(
            &session,
            &format!("Data Source={};ServerType=LOCAL;", dir.display()),
        )
        .unwrap();
        // Each table's record 2 is id=2, code=BBB, name=bravo.
        let filter = |file: &str, options: u32| -> Result<String, Error> {
            let handle = table::open(&session, connect, file, adt()).unwrap();
            table::get(&session, handle)
                .unwrap()
                .goto_record(2)
                .unwrap();
            key_filter(&session, handle, None, options)
        };
        // The unique index whose tag sorts first: BYCODE over BYID/NAMEID.
        assert_eq!(filter("kf.adt", 0).unwrap(), "code='BBB'");
        assert_eq!(filter("kf.adt", 0x1).unwrap(), "'BBB'"); // OData single key
        // A composite unique key joins its fields.
        assert_eq!(filter("kf3.adt", 0).unwrap(), "code='BBB' AND id=2");
        assert_eq!(filter("kf3.adt", 0x1).unwrap(), "code='BBB',id=2"); // OData composite
        // AID sorts before ZCODE, so the id index wins.
        assert_eq!(filter("kf6.adt", 0).unwrap(), "id=2");
        // No unique index: not found.
        assert_eq!(filter("kf4.adt", 0), Err(Error::NotFound));
        // With no unique index but an AutoInc field, it falls back to that
        // (record 2's AutoInc is 2); in OData it takes the `L` suffix.
        assert_eq!(filter("ai.adt", 0).unwrap(), "ac=2");
        assert_eq!(filter("ai.adt", 0x1).unwrap(), "2L");
        // Options beyond OData and UTF-8 are invalid.
        assert_eq!(filter("kf.adt", 4), Err(Error::InvalidOption));
        connection::disconnect(&session, connect).ok();
    }
}
