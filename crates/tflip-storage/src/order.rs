//! Index files of any format, and cursors in their orders: Advantage's
//! `.adi` (see [`crate::adi`]), Clipper's `.ntx` (see [`crate::ntx`]) and
//! FoxPro's `.cdx` and `.idx` (see [`crate::cdx`]).
//!
//! An order is described by a [`Tag`] whatever its format, with the
//! options and flags of an ADI tag header; an NTX file's one order has the
//! file's name. NTX and CDX orders have their expression and condition as
//! written (with field names, which the engine turns into `F<n>`).

use std::io;
use std::path::Path;

use crate::adi;
use crate::cdx::{self, CdxFile};
use crate::ntx::{self, NtxFile};

/// Why an index could not be read.
#[derive(Debug)]
pub enum Error {
    /// Reading the file failed.
    Io(io::Error),
    /// The file is not an index of its format, or its pages contradict
    /// each other.
    Corrupt(&'static str),
    /// The index is encrypted.
    Encrypted,
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::UnexpectedEof => Error::Corrupt("file ends inside a page"),
            _ => Error::Io(error),
        }
    }
}

/// Tag option bits (header offset 0x0E).
pub mod options {
    /// Unique keys (`UNIQUE` or `CANDIDATE`).
    pub const UNIQUE: u8 = 0x01;
    /// Keys are added explicitly (`AdsAddCustomKey`).
    pub const CUSTOM: u8 = 0x04;
    /// The order has a `FOR` condition.
    pub const CONDITION: u8 = 0x08;
    /// The flags at header offset 0x18 are in use.
    pub const FLAGS: u8 = 0x10;
}

/// Where an ADI tag header names the locale of a Unicode order (the root
/// collation's `en_US`, the only one the original engine's files have), and
/// the four bytes after it, which every such header has.
pub mod locale {
    pub const OFFSET: usize = 0x5C;
    pub const END: usize = 0x74;
    pub const TAIL: [u8; 4] = [0x31, 0xC0, 0x00, 0x05];
    /// The byte after them: how many bytes a character takes in the order's
    /// keys, as `ADS_UCHAR_KEY_SHORT`, `_LONG` and `_XLONG` ask for (2, 4 and
    /// 5); 0 for the default of 3.
    pub const WIDTH: usize = END + 4;
}

/// Header flags (offset 0x18, in use when option bit [`options::FLAGS`] is
/// set).
pub mod flags {
    /// A full-text order (tag header).
    pub const FULL_TEXT: u32 = 0x100;
    /// The file is encrypted (file header).
    pub const ENCRYPTED: u32 = 0x200;
    /// A binary (bitmap) order (tag header).
    pub const BINARY: u32 = 0x800;
    /// Not in ADI files: a candidate order of a CDX file, whose keys are
    /// unique and which rejects duplicates (an ADI unique order is one).
    pub const CANDIDATE: u32 = 0x4000_0000;
    /// Not in ADI files: a VFP order whose keys can be NULL (CDX option
    /// 0x02): they are one byte longer, led by 0x80, or all zeros for NULL.
    pub const NULLABLE: u32 = 0x2000_0000;
}

/// One order of an index file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    /// The name, in upper case.
    pub name: String,
    /// The page of the tag header.
    pub header_page: u32,
    /// The root page of the order's tree.
    pub root: u32,
    pub key_length: u16,
    /// Option bits ([`options`]).
    pub options: u8,
    /// The byte trailing compression removes from keys.
    pub pad: u8,
    /// Flags ([`flags`]); 0 unless option bit [`options::FLAGS`] is set.
    pub flags: u32,
    /// Keys are walked in reverse; they are stored ascending.
    pub descending: bool,
    /// The key expression, with fields as `F<n>` (1-based field numbers).
    pub expression: String,
    /// The `FOR` condition, with fields as `F<n>`; empty if none.
    pub condition: String,
    /// The name of a dynamic collation; empty for the static collation.
    pub collation: String,
    /// The locale of a Unicode (NChar, NVarChar) order, whose keys are ICU
    /// sort keys (`en_US`); empty for any other.
    pub locale: String,
    /// The key width option of a Unicode order (see [`locale::WIDTH`]): 0, 2,
    /// 4 or 5.
    pub uchar_width: u8,
    /// The decimals of an NTX order's numeric keys; 0 in an ADI file.
    pub decimals: u16,
    /// The page of a full-text order's options block (tag header offset
    /// 0x1C); 0 for any other order.
    pub fts_block: u32,
    /// Whether a full-text order counts each word's occurrences in its leaf
    /// entries (`KEEPSCORE`).
    pub keeps_score: bool,
}

impl Tag {
    pub fn unique(&self) -> bool {
        self.options & options::UNIQUE != 0
    }

    pub fn custom(&self) -> bool {
        self.options & options::CUSTOM != 0
    }

    pub fn full_text(&self) -> bool {
        self.flags & flags::FULL_TEXT != 0
    }

    pub fn binary(&self) -> bool {
        self.flags & flags::BINARY != 0
    }
}

/// A key and the record it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub bytes: Vec<u8>,
    pub record: u32,
}

/// The format of an index file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Adi,
    Ntx,
    Cdx,
}

impl Kind {
    /// The format of the index file at `path`, by its extension: `.ntx` is
    /// NTX, `.cdx` and `.idx` CDX, anything else ADI.
    pub fn of(path: &Path) -> Kind {
        match path.extension().and_then(|e| e.to_str()) {
            Some(e) if e.eq_ignore_ascii_case("ntx") => Kind::Ntx,
            Some(e) if e.eq_ignore_ascii_case("cdx") || e.eq_ignore_ascii_case("idx") => Kind::Cdx,
            _ => Kind::Adi,
        }
    }
}

/// An open index file.
#[derive(Debug)]
pub enum IndexFile {
    Adi(adi::IndexFile),
    Ntx(NtxFile),
    Cdx(CdxFile),
}

impl IndexFile {
    /// Opens the index file at `path`, of the format its extension says.
    pub fn open(path: &Path) -> Result<IndexFile, Error> {
        Ok(match Kind::of(path) {
            Kind::Adi => IndexFile::Adi(adi::IndexFile::open(path)?),
            Kind::Ntx => IndexFile::Ntx(NtxFile::open(path)?),
            Kind::Cdx => IndexFile::Cdx(CdxFile::open(path)?),
        })
    }

    /// Opens the index file at `path`, deciphering an encrypted ADI the way its
    /// table is (`cipher`); DBF index files are not encrypted here.
    pub fn open_encrypted(
        path: &Path,
        cipher: &crate::table::RecordCipher,
    ) -> Result<IndexFile, Error> {
        Ok(match Kind::of(path) {
            Kind::Adi => IndexFile::Adi(adi::IndexFile::open_encrypted(path, cipher)?),
            Kind::Ntx => IndexFile::Ntx(NtxFile::open(path)?),
            Kind::Cdx => IndexFile::Cdx(CdxFile::open(path)?),
        })
    }

    pub fn kind(&self) -> Kind {
        match self {
            IndexFile::Adi(_) => Kind::Adi,
            IndexFile::Ntx(_) => Kind::Ntx,
            IndexFile::Cdx(_) => Kind::Cdx,
        }
    }

    /// The page size in bytes.
    pub fn page_size(&self) -> u32 {
        match self {
            IndexFile::Adi(file) => file.page_size(),
            IndexFile::Ntx(_) => 1024,
            IndexFile::Cdx(_) => 512,
        }
    }

    /// The options block of a full-text order as stored
    /// ([`adi::IndexFile::fts_block`]); `None` for any other order.
    pub fn fts_block(&mut self, tag: &Tag) -> Result<Option<crate::create::FtsBlock>, Error> {
        match self {
            IndexFile::Adi(file) => file.fts_block(tag),
            IndexFile::Cdx(file) => file.fts_block(tag),
            IndexFile::Ntx(_) => Ok(None),
        }
    }

    /// The parsing options of a full-text order ([`adi::IndexFile::fts_options`]);
    /// `None` for an order that is not full-text, and for NTX files, which
    /// have none.
    pub fn fts_options(&mut self, tag: &Tag) -> Result<Option<adi::FtsOptions>, Error> {
        match self {
            IndexFile::Adi(file) => file.fts_options(tag),
            IndexFile::Cdx(file) => Ok(file
                .fts_block(tag)?
                .map(|block| adi::fts_options_of(tag, block))),
            IndexFile::Ntx(_) => Ok(None),
        }
    }

    /// The keys of a full-text order ([`adi::IndexFile::keys`]): a word (padded
    /// with blanks to the key length) and its record, for each distinct word of
    /// each record the order holds. None for NTX files.
    pub fn fts_keys(&mut self, tag: &Tag) -> Result<Vec<Key>, Error> {
        match self {
            IndexFile::Adi(file) if tag.full_text() => file.keys(tag),
            IndexFile::Cdx(file) if tag.full_text() => {
                let mut cursor = cdx::Cursor::new(tag);
                let mut keys = Vec::new();
                let mut key = cursor.first(file)?.cloned();
                while let Some(found) = key {
                    keys.push(found);
                    key = cursor.next(file)?.cloned();
                }
                Ok(keys)
            }
            _ => Ok(Vec::new()),
        }
    }

    /// The file's orders, in the order they were created; `name` names an
    /// NTX file's one order.
    pub fn tags(&self, name: &str) -> Vec<Tag> {
        match self {
            IndexFile::Adi(file) => file.tags.clone(),
            IndexFile::Cdx(file) => file.tags.clone(),
            IndexFile::Ntx(file) => {
                let header = &file.header;
                let mut options = if header.unique { options::UNIQUE } else { 0 };
                if !header.condition.is_empty() {
                    options |= options::CONDITION;
                }
                vec![Tag {
                    name: name.to_owned(),
                    header_page: 0,
                    root: header.root,
                    key_length: header.key_length,
                    options,
                    pad: b' ',
                    flags: 0,
                    descending: header.descending,
                    expression: header.expression.clone(),
                    condition: header.condition.clone(),
                    collation: String::new(),
                    locale: String::new(),
                    uchar_width: 0,
                    decimals: header.decimals,
                    fts_block: 0,
                    keeps_score: false,
                }]
            }
        }
    }

    /// Orders an NTX or CDX file's keys by `weights` (its table's static
    /// collation's); ADI files keep weighed keys.
    pub fn set_weights(&mut self, weights: Option<[u8; 256]>) {
        match self {
            IndexFile::Ntx(file) => file.set_weights(weights),
            IndexFile::Cdx(file) => file.set_weights(weights),
            IndexFile::Adi(_) => {}
        }
    }
}

/// A position in an order of an index file, moving through its keys in
/// index order (see [`adi::Cursor`]).
#[derive(Debug, Clone)]
pub enum Cursor {
    Adi(adi::Cursor),
    Ntx(ntx::Cursor),
    Cdx(cdx::Cursor),
}

/// A cursor of one format given the file of another.
const MISMATCH: Error = Error::Corrupt("cursor and index file of different formats");

impl Cursor {
    /// A cursor before the first key of `tag`, an order of `file`.
    pub fn new(tag: &Tag, file: &IndexFile) -> Cursor {
        match file {
            IndexFile::Adi(_) => Cursor::Adi(adi::Cursor::new(tag)),
            IndexFile::Ntx(_) => Cursor::Ntx(ntx::Cursor::new()),
            IndexFile::Cdx(_) => Cursor::Cdx(cdx::Cursor::new(tag)),
        }
    }

    /// The key the cursor is on, if any.
    pub fn key(&self) -> Option<&Key> {
        match self {
            Cursor::Adi(cursor) => cursor.key(),
            Cursor::Ntx(cursor) => cursor.key(),
            Cursor::Cdx(cursor) => cursor.key(),
        }
    }

    pub fn before_first(&self) -> bool {
        match self {
            Cursor::Adi(cursor) => cursor.before_first(),
            Cursor::Ntx(cursor) => cursor.before_first(),
            Cursor::Cdx(cursor) => cursor.before_first(),
        }
    }

    pub fn after_last(&self) -> bool {
        match self {
            Cursor::Adi(cursor) => cursor.after_last(),
            Cursor::Ntx(cursor) => cursor.after_last(),
            Cursor::Cdx(cursor) => cursor.after_last(),
        }
    }

    pub fn first(&mut self, file: &mut IndexFile) -> Result<Option<&Key>, Error> {
        match (self, file) {
            (Cursor::Adi(cursor), IndexFile::Adi(file)) => cursor.first(file),
            (Cursor::Ntx(cursor), IndexFile::Ntx(file)) => cursor.first(file),
            (Cursor::Cdx(cursor), IndexFile::Cdx(file)) => cursor.first(file),
            _ => Err(MISMATCH),
        }
    }

    pub fn last(&mut self, file: &mut IndexFile) -> Result<Option<&Key>, Error> {
        match (self, file) {
            (Cursor::Adi(cursor), IndexFile::Adi(file)) => cursor.last(file),
            (Cursor::Ntx(cursor), IndexFile::Ntx(file)) => cursor.last(file),
            (Cursor::Cdx(cursor), IndexFile::Cdx(file)) => cursor.last(file),
            _ => Err(MISMATCH),
        }
    }

    pub fn next(&mut self, file: &mut IndexFile) -> Result<Option<&Key>, Error> {
        match (self, file) {
            (Cursor::Adi(cursor), IndexFile::Adi(file)) => cursor.next(file),
            (Cursor::Ntx(cursor), IndexFile::Ntx(file)) => cursor.next(file),
            (Cursor::Cdx(cursor), IndexFile::Cdx(file)) => cursor.next(file),
            _ => Err(MISMATCH),
        }
    }

    pub fn prev(&mut self, file: &mut IndexFile) -> Result<Option<&Key>, Error> {
        match (self, file) {
            (Cursor::Adi(cursor), IndexFile::Adi(file)) => cursor.prev(file),
            (Cursor::Ntx(cursor), IndexFile::Ntx(file)) => cursor.prev(file),
            (Cursor::Cdx(cursor), IndexFile::Cdx(file)) => cursor.prev(file),
            _ => Err(MISMATCH),
        }
    }

    pub fn seek(
        &mut self,
        file: &mut IndexFile,
        key: &[u8],
        record: Option<u32>,
    ) -> Result<Option<&Key>, Error> {
        match (self, file) {
            (Cursor::Adi(cursor), IndexFile::Adi(file)) => cursor.seek(file, key, record),
            (Cursor::Ntx(cursor), IndexFile::Ntx(file)) => cursor.seek(file, key, record),
            (Cursor::Cdx(cursor), IndexFile::Cdx(file)) => cursor.seek(file, key, record),
            _ => Err(MISMATCH),
        }
    }

    /// The rest of the leaf the cursor is on and whether a leaf follows it
    /// (see [`adi::Cursor::leaf_keys`]); of an NTX or CDX order, the key it
    /// is on alone.
    pub fn leaf_keys(&self, forward: bool) -> (Vec<Key>, bool) {
        match self {
            Cursor::Adi(cursor) => cursor.leaf_keys(forward),
            Cursor::Ntx(cursor) => (cursor.key().cloned().into_iter().collect(), false),
            Cursor::Cdx(cursor) => (cursor.key().cloned().into_iter().collect(), false),
        }
    }

    pub fn key_number(&self, file: &mut IndexFile) -> Result<Option<u32>, Error> {
        match (self, file) {
            (Cursor::Adi(cursor), IndexFile::Adi(file)) => cursor.key_number(file),
            (Cursor::Ntx(cursor), IndexFile::Ntx(file)) => cursor.key_number(file),
            (Cursor::Cdx(cursor), IndexFile::Cdx(file)) => cursor.key_number(file),
            _ => Err(MISMATCH),
        }
    }

    pub fn key_count(&self, file: &mut IndexFile) -> Result<u32, Error> {
        match (self, file) {
            (Cursor::Adi(cursor), IndexFile::Adi(file)) => cursor.key_count(file),
            (Cursor::Ntx(cursor), IndexFile::Ntx(file)) => cursor.key_count(file),
            (Cursor::Cdx(cursor), IndexFile::Cdx(file)) => cursor.key_count(file),
            _ => Err(MISMATCH),
        }
    }
}
