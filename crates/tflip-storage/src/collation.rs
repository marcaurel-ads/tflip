//! Collations: how character keys are weighed, from the collation files the
//! original engine ships (`ansi.chr`, `extend.chr`, `adscollate.adt`).
//!
//! - A **static collation** maps each of the 256 bytes to a weight of its
//!   own (and back), and has upper and lower case tables. `ansi.chr` holds
//!   the ANSI ones by language (`Engl(Amer)`, `German`, ...), `extend.chr`
//!   the OEM ones (`USA`, `GERMAN`, ...). Each file starts with a directory
//!   of entries (a byte 1, the language's name, NUL-padded, and the offset of
//!   its tables) and has four 256-byte tables per language: byte to weight,
//!   weight to byte, upper case, lower case.
//! - A **dynamic collation** (`GERMAN_VFP_CI_AS_1252`, ...) is a record of
//!   `adscollate.adt`: per byte an 8-byte collation element (the byte, its
//!   primary weight, its accent weight, whether it has one, and an expansion
//!   into other bytes: `ä` as `ae`), two-byte contractions (Czech `ch`), and
//!   case tables. See [`Dynamic::key`] for the keys it builds.
//!
//! These files are the original engine's data, not tflip's: they are read
//! where a client has them (next to its DLL, as the original engine keeps
//! them), not shipped with tflip.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use crate::table::{Format, TableFile};
use crate::value::Value;

/// How `a` compares with `b` byte by byte, by `weights` (a static
/// collation's) or as bytes.
pub fn weighed_cmp(a: &[u8], b: &[u8], weights: Option<&[u8; 256]>) -> Ordering {
    match weights {
        None => a.cmp(b),
        Some(w) => a
            .iter()
            .map(|&x| w[usize::from(x)])
            .cmp(b.iter().map(|&x| w[usize::from(x)])),
    }
}

/// A static collation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Static {
    /// The language it is named by in its file.
    pub name: String,
    /// Each byte's weight.
    pub weights: [u8; 256],
    /// Each byte in upper case.
    pub upper: [u8; 256],
    /// Each byte in lower case.
    pub lower: [u8; 256],
}

/// The size of a language's tables in a `.chr` file.
const CHR_TABLES: usize = 4 * 256;

impl Static {
    /// The language `name` (regardless of case) of the `.chr` file `bytes`,
    /// whose directory entries are `entry` bytes long (16 in `ansi.chr`, 14
    /// in `extend.chr`); `None` if it has no such language.
    pub fn from_chr(bytes: &[u8], entry: usize, name: &str) -> Option<Static> {
        let table = |at: usize| -> Option<[u8; 256]> { bytes.get(at..at + 256)?.try_into().ok() };
        Static::languages(bytes, entry)
            .into_iter()
            .find_map(|(language, offset)| {
                if !language.eq_ignore_ascii_case(name.trim()) || offset + CHR_TABLES > bytes.len()
                {
                    return None;
                }
                Some(Static {
                    name: language,
                    weights: table(offset)?,
                    upper: table(offset + 512)?,
                    lower: table(offset + 768)?,
                })
            })
    }

    /// The languages of a `.chr` file and the offsets of their tables.
    pub fn languages(bytes: &[u8], entry: usize) -> Vec<(String, usize)> {
        bytes
            .chunks_exact(entry)
            .take_while(|e| e[0] == 1)
            .map(|e| {
                let name = &e[1..entry - 4];
                let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
                let offset = u32::from_le_bytes(e[entry - 4..].try_into().unwrap()) as usize;
                (String::from_utf8_lossy(&name[..end]).into_owned(), offset)
            })
            .collect()
    }

    /// The key of `bytes` under the collation: each byte's weight.
    pub fn key(&self, bytes: &[u8]) -> Vec<u8> {
        bytes
            .iter()
            .map(|&b| self.weights[usize::from(b)])
            .collect()
    }
}

/// A record of `adscollate.adt` as the system procedures that list the
/// repository give it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub name: String,
    pub short_name: String,
    pub description: Option<String>,
    pub version: i64,
    pub code_page: i64,
    pub fox_compat: bool,
    pub unicode_locale: Option<String>,
    /// Whether an index may hold keys of several collations; the file has
    /// the column only once `sp_AllowMultipleCollations` has been run.
    pub allow_multiple: bool,
    pub elements: Vec<u8>,
    pub contractions: Option<Vec<u8>>,
    pub upper: Vec<u8>,
    pub lower: Vec<u8>,
}

/// A dynamic collation, a record of `adscollate.adt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dynamic {
    /// Its name (`GERMAN_VFP_CI_AS_1252`).
    pub name: String,
    /// The ANSI code page it is for.
    pub code_page: u32,
    /// The collation element of each byte: the byte, its primary weight,
    /// its accent weight, whether it has one, a byte not known, the number
    /// of bytes it expands to (0 for none), and those bytes.
    elements: Vec<[u8; 8]>,
    /// Contractions: two bytes, their primary weight and a fourth byte.
    contractions: Vec<[u8; 4]>,
    /// Each byte in upper case.
    pub upper: [u8; 256],
    /// Each byte in lower case.
    pub lower: [u8; 256],
}

impl Dynamic {
    /// The name of the VFP collation that a FoxPro index names `short`
    /// (`GERMAN`), for code page `code_page`, in the `adscollate.adt` at
    /// `path`: the collation with that short name and code page whose name
    /// says it is a VFP one (`GERMAN_VFP_CI_AS_1252`).
    pub fn vfp_name(path: &Path, short: &str, code_page: u32) -> Option<String> {
        let mut table = TableFile::open(path, Format::Adt).ok()?;
        let fields = table.structure.fields.clone();
        let field = |name: &str| {
            fields
                .iter()
                .position(|f| f.name.eq_ignore_ascii_case(name))
        };
        let (name_at, short_at, page_at) =
            (field("Name")?, field("ShortName")?, field("Codepage")?);
        let mut record = vec![0; table.structure.header.record_length as usize];
        for slot in 1..=table.structure.header.record_slots {
            table.read(slot, &mut record).ok()?;
            let mut value = |at: usize| fields[at].value(&record, table.memo()).ok();
            let (Some(Value::Text(name)), Some(Value::Text(found)), Some(Value::Integer(page))) =
                (value(name_at), value(short_at), value(page_at))
            else {
                continue;
            };
            let name = name.trim();
            if found.trim().eq_ignore_ascii_case(short.trim())
                && page == i128::from(code_page)
                && name.to_ascii_uppercase().contains("_VFP_")
            {
                return Some(name.to_owned());
            }
        }
        None
    }

    /// Every record of the `adscollate.adt` at `path`, in the file's order;
    /// none if there is no such file.
    pub fn records(path: &Path) -> Vec<Record> {
        let Ok(mut table) = TableFile::open(path, Format::Adt) else {
            return Vec::new();
        };
        let fields = table.structure.fields.clone();
        let field = |name: &str| {
            fields
                .iter()
                .position(|f| f.name.eq_ignore_ascii_case(name))
        };
        let mut records = Vec::new();
        let mut record = vec![0; table.structure.header.record_length as usize];
        for slot in 1..=table.structure.header.record_slots {
            if table.read(slot, &mut record).is_err() {
                break;
            }
            let mut value = |name: &str| {
                let at = field(name)?;
                fields[at].value(&record, table.memo()).ok()
            };
            let Some(Value::Text(name)) = value("Name") else {
                continue;
            };
            let text = |value: Option<Value>| match value {
                Some(Value::Text(text)) => Some(text.trim_end().to_owned()),
                _ => None,
            };
            let number = |value: Option<Value>| match value {
                Some(Value::Integer(n)) => n as i64,
                _ => 0,
            };
            let binary = |value: Option<Value>| match value {
                Some(Value::Binary(bytes)) => Some(bytes),
                _ => None,
            };
            records.push(Record {
                name: name.trim_end().to_owned(),
                short_name: text(value("ShortName")).unwrap_or_default(),
                description: text(value("Desc")),
                version: number(value("Version")),
                code_page: number(value("Codepage")),
                fox_compat: matches!(value("FoxCompat"), Some(Value::Logical(true))),
                unicode_locale: text(value("UnicodeLocale")),
                allow_multiple: matches!(value("AllowMultiple"), Some(Value::Logical(true))),
                elements: binary(value("CE")).unwrap_or_default(),
                contractions: binary(value("Contractions")),
                upper: binary(value("Upper")).unwrap_or_default(),
                lower: binary(value("Lower")).unwrap_or_default(),
            });
        }
        records
    }

    /// The collation `name` (regardless of case) of the `adscollate.adt` at
    /// `path`; `None` if there is no such file or collation.
    pub fn open(path: &Path, name: &str) -> Option<Dynamic> {
        let mut table = TableFile::open(path, Format::Adt).ok()?;
        let fields = table.structure.fields.clone();
        let field = |name: &str| {
            fields
                .iter()
                .position(|f| f.name.eq_ignore_ascii_case(name))
        };
        let (name_at, page_at, ce_at, contractions_at, upper_at, lower_at) = (
            field("Name")?,
            field("Codepage")?,
            field("CE")?,
            field("Contractions")?,
            field("Upper")?,
            field("Lower")?,
        );
        let mut record = vec![0; table.structure.header.record_length as usize];
        for slot in 1..=table.structure.header.record_slots {
            table.read(slot, &mut record).ok()?;
            let mut value = |at: usize| fields[at].value(&record, table.memo()).ok();
            let Some(Value::Text(found)) = value(name_at) else {
                continue;
            };
            if !found.trim().eq_ignore_ascii_case(name.trim()) {
                continue;
            }
            let bytes = |value: Option<Value>| match value {
                Some(Value::Binary(bytes)) => bytes,
                _ => Vec::new(),
            };
            let code_page = match value(page_at) {
                Some(Value::Integer(page)) => page as u32,
                _ => 0,
            };
            let elements = bytes(value(ce_at));
            let contractions = bytes(value(contractions_at));
            let case = |bytes: Vec<u8>| -> Option<[u8; 256]> { bytes.try_into().ok() };
            let (upper, lower) = (case(bytes(value(upper_at)))?, case(bytes(value(lower_at)))?);
            if elements.len() != 256 * 8 {
                return None;
            }
            return Some(Dynamic {
                name: found.trim().to_owned(),
                code_page,
                elements: elements.as_chunks::<8>().0.to_vec(),
                contractions: contractions.as_chunks::<4>().0.to_vec(),
                upper,
                lower,
            });
        }
        None
    }

    /// The canonical name of the collation `name` (regardless of case) in the
    /// repository `adscollate.adt` under `dir`, as the engine stores it
    /// (`GERMAN_VFP_CI_AS_1252`); `None` if there is no such file or no such
    /// collation. The engine refuses a name it cannot find here (`AdsSetCollation`
    /// and `AdsStmtSetTableCollation`, 7147).
    pub fn canonical(dir: &Path, name: &str) -> Option<String> {
        Dynamic::open(&dir.join("adscollate.adt"), name).map(|c| c.name)
    }

    /// Whether the engine's ICU opens a collator for `locale` (`de_DE`,
    /// `de_ADS_CI`, `deu`; regardless of case, and of spaces around it), the
    /// names `AdsStmtSetTableCollation` takes after a colon. The set is the
    /// one the original engine answered to, probed name by name
    /// (`tools/ace/collation/setcollation_dd.c`): locales with collation
    /// data of their own (an alias such as `zh_CN` or `in` has none), the
    /// ADS variants of `adscollate.adt`, and some ISO 639-2 spellings.
    pub fn is_icu_locale(locale: &str) -> bool {
        static LOCALES: OnceLock<HashSet<&'static str>> = OnceLock::new();
        LOCALES
            .get_or_init(|| include_str!("icu_collators.txt").lines().collect())
            .contains(locale.trim_matches(' ').to_ascii_lowercase().as_str())
    }

    /// Whether the collation sorts by the bytes themselves (`MACHINE_..._BIN`):
    /// no byte has an accent weight.
    pub fn is_binary(&self) -> bool {
        self.elements.iter().all(|e| e[3] == 0)
    }

    /// Whether the collation weighs a letter's cases alike (`..._CI_...`).
    pub fn ignores_case(&self) -> bool {
        self.weights(b"A") == self.weights(b"a")
    }

    /// The key of `value`, `key_length` bytes, as the original engine builds
    /// it: the primary weights of its bytes, then the accent weights of
    /// those that have one, then zeros. Trailing spaces are dropped (but an
    /// all-blank value keeps them); a contraction weighs its two bytes as
    /// one; a byte that expands (`ä` as `ae`, `ß` as `ss`) weighs as the
    /// bytes it expands to. A binary collation weighs every byte, trailing
    /// spaces among them, and has no accent weights.
    pub fn key(&self, value: &[u8], key_length: usize) -> Vec<u8> {
        let mut key = self.weights(value);
        key.resize(key_length, 0);
        key
    }

    /// The weights of `value` that its [`Dynamic::key`] starts with, without
    /// the zeros that end it: by these the expression engine compares text
    /// under the collation.
    pub fn weights(&self, value: &[u8]) -> Vec<u8> {
        let binary = self.is_binary();
        let trimmed = value
            .iter()
            .rposition(|&b| b != b' ')
            .map(|end| &value[..=end]);
        let value = if binary {
            value
        } else {
            trimmed.unwrap_or(value)
        };
        let (mut primary, mut accent) = (Vec::new(), Vec::new());
        let weigh = |element: &[u8; 8], primary: &mut Vec<u8>, accent: &mut Vec<u8>| {
            primary.push(element[1]);
            if element[3] != 0 {
                accent.push(element[2]);
            }
        };
        let mut i = 0;
        while i < value.len() {
            let byte = value[i];
            let next = value.get(i + 1).copied();
            if let Some(c) = self
                .contractions
                .iter()
                .find(|c| c[0] == byte && Some(c[1]) == next)
            {
                primary.push(c[2]);
                i += 2;
                continue;
            }
            let element = &self.elements[usize::from(byte)];
            match usize::from(element[5]) {
                0 => weigh(element, &mut primary, &mut accent),
                count => {
                    for &part in element[6..].iter().take(count) {
                        weigh(&self.elements[usize::from(part)], &mut primary, &mut accent);
                    }
                }
            }
            i += 1;
        }
        primary.extend(accent);
        // Trailing zero weights (accents of weight zero) are as the zeros
        // that fill a key.
        while primary.last() == Some(&0) {
            primary.pop();
        }
        primary
    }

    /// The weights by which the original engine's SQL groups text under the
    /// collation (`GROUP BY`, `DISTINCT`, `UNION`): the [`Dynamic::weights`]
    /// of the text without its trailing spaces, an all-blank one's too, so
    /// that a blank sorts before a character that weighs least (probed,
    /// `fixtures/sql/collation_dynamic.json`).
    pub fn grouping_weights(&self, value: &[u8]) -> Vec<u8> {
        let end = value.iter().rposition(|&b| b != b' ').map_or(0, |i| i + 1);
        match end {
            0 => Vec::new(),
            _ => self.weights(&value[..end]),
        }
    }

    /// Compares `a` and `b` as the original engine's SQL compares text of
    /// columns under the collation (`=`, `<`, `MIN`, ...): the shorter
    /// padded with spaces, by their [`Dynamic::weights`] padded with zeros,
    /// then by which bytes expand, so that `ß` sorts after `ss` and `ä` after
    /// `ae` (probed, `fixtures/sql/collation_dynamic.json`).
    pub fn compare(&self, a: &[u8], b: &[u8]) -> Ordering {
        let width = a.len().max(b.len());
        let padded = |s: &[u8]| {
            let mut s = s.to_vec();
            s.resize(width, b' ');
            s
        };
        let (a, b) = (padded(a), padded(b));
        let (mut x, mut y) = (self.weights(&a), self.weights(&b));
        let weights = x.len().max(y.len());
        x.resize(weights, 0);
        y.resize(weights, 0);
        x.cmp(&y)
            .then_with(|| self.expansions(&a).cmp(&self.expansions(&b)))
    }

    /// What equal text compares equal by under [`Dynamic::compare`]: its
    /// weights, then which of its bytes expand.
    pub fn comparison_key(&self, value: &[u8]) -> Vec<u8> {
        let end = value.iter().rposition(|&b| b != b' ').map_or(0, |i| i + 1);
        let value = &value[..end];
        let weights = self.weights(value);
        let mut key = (weights.len() as u32).to_le_bytes().to_vec();
        key.extend(weights);
        let mut expansions = self.expansions(value);
        while expansions.last() == Some(&0) {
            expansions.pop();
        }
        key.extend(expansions);
        key
    }

    /// Whether each byte of `value` expands to others (1) or not (0).
    fn expansions(&self, value: &[u8]) -> Vec<u8> {
        value
            .iter()
            .map(|&b| u8::from(self.elements[usize::from(b)][5] != 0))
            .collect()
    }
}

#[cfg(windows)]
mod windows {
    use super::Static;

    use crate::os::locale;

    const LCMAP_LOWERCASE: u32 = 0x0100;
    const LCMAP_UPPERCASE: u32 = 0x0200;
    const CSTR_EQUAL: i32 = 2;

    /// The static ANSI collation the original engine derives from the user
    /// locale with `ANSI_CHAR_SET` empty (see `tflip_engine::system_collation`):
    /// `None` (the default) under US, UK or Canadian English (by the language
    /// abbreviation), else each byte weighed by its rank in `CompareStringA`'s
    /// order, with the locale's case.
    pub fn user_locale_default() -> Option<Static> {
        let abbreviation = locale::language_abbreviation();
        if [&b"ENU"[..], b"ENG", b"ENC"]
            .iter()
            .any(|a| a.eq_ignore_ascii_case(&abbreviation))
        {
            return None;
        }
        // Whether `a` sorts after `b`: by CompareStringA, equal ones by byte.
        let after = |a: u8, b: u8| {
            let result = locale::compare(a, b);
            if result == 0 || result == CSTR_EQUAL {
                a > b
            } else {
                result > CSTR_EQUAL
            }
        };
        // An insertion sort, as the tables of `system_collations.bin` were
        // made (Windows' sorting need not be transitive across ignorable
        // characters, so the kind of sort could matter).
        let mut order: Vec<u8> = (0..=255).collect();
        for i in 1..order.len() {
            let mut j = i;
            while j > 0 && after(order[j - 1], order[j]) {
                order.swap(j - 1, j);
                j -= 1;
            }
        }
        let mut weights = [0u8; 256];
        for (rank, &byte) in order.iter().enumerate() {
            weights[usize::from(byte)] = rank as u8;
        }
        // Byte by byte: a double-byte code page's lead byte stays itself.
        let map = |flags: u32| -> [u8; 256] {
            std::array::from_fn(|b| {
                let source = b as u8;
                if b == 0 {
                    source
                } else {
                    locale::map(flags, source)
                }
            })
        };
        let (upper, lower) = (map(LCMAP_UPPERCASE), map(LCMAP_LOWERCASE));
        let weighs_itself = weights
            .iter()
            .enumerate()
            .all(|(i, &w)| usize::from(w) == i);
        (!weighs_itself).then(|| Static {
            name: String::new(),
            weights,
            upper,
            lower,
        })
    }
}

#[cfg(windows)]
pub use windows::user_locale_default;

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;

    use serde_json::Value as Json;

    use super::*;

    /// Where the original engine's collation files are, if this machine has
    /// them: `TFLIP_COLLATION_DIR`, or the Advantage SDK under Wine.
    pub(crate) fn collation_dir() -> Option<PathBuf> {
        let dir = std::env::var_os("TFLIP_COLLATION_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                let home = std::env::var_os("HOME")?;
                Some(
                    PathBuf::from(home)
                        .join("src/ace-sdk/drive_c/Program Files (x86)/Advantage 12.0/acesdk"),
                )
            })?;
        dir.join("ansi.chr").exists().then_some(dir)
    }

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt")
    }

    fn dump(name: &str) -> Json {
        serde_json::from_str(&std::fs::read_to_string(fixtures().join(name)).unwrap()).unwrap()
    }

    /// The bytes of a value as the dump records its text (one character a
    /// byte), `None` for NULL.
    fn bytes(value: &Json) -> Option<Vec<u8>> {
        let text = value["text"].as_str()?;
        Some(text.chars().map(|c| c as u32 as u8).collect())
    }

    /// The keys of the `collation_dynamic` fixtures, as the original engine's
    /// `AdsExtractKey` returned them.
    #[test]
    fn vfp_names() {
        let Some(dir) = collation_dir() else { return };
        let path = dir.join("adscollate.adt");
        for (short, page, name) in [
            ("GERMAN", 1252, "GERMAN_VFP_CI_AS_1252"),
            ("GENERAL", 1252, "GENERAL_VFP_CI_AS_1252"),
            ("MACHINE", 1252, "MACHINE_VFP_BIN_1252"),
            ("UNIQWT", 1252, "UNIQWT_VFP_CS_AS_1252"),
            ("CZECH", 1250, "CZECH_VFP_CI_AS_1250"),
        ] {
            assert_eq!(
                Dynamic::vfp_name(&path, short, page).as_deref(),
                Some(name),
                "{short}"
            );
        }
        assert_eq!(Dynamic::vfp_name(&path, "NOSUCH", 1252), None);
    }

    #[test]
    fn canonical_names_match_the_repository() {
        let Some(dir) = collation_dir() else { return };
        // A name is found regardless of case, and comes back in the engine's
        // stored (upper-case) spelling.
        assert_eq!(
            Dynamic::canonical(&dir, "german_vfp_ci_as_1252").as_deref(),
            Some("GERMAN_VFP_CI_AS_1252")
        );
        assert_eq!(
            Dynamic::canonical(&dir, "GENERAL_VFP_CI_AS_1252").as_deref(),
            Some("GENERAL_VFP_CI_AS_1252")
        );
        // A short alias, a wrong table type and an unknown name are not found
        // (the engine returns 7147 for these).
        assert_eq!(Dynamic::canonical(&dir, "GERMAN"), None);
        assert_eq!(Dynamic::canonical(&dir, "GERMAN_ADT_CI_AS_1252"), None);
        assert_eq!(Dynamic::canonical(&dir, "Bogus"), None);
    }

    #[test]
    fn icu_locales_match_the_engine() {
        for known in [
            "en_US",
            "de_de",
            " root ",
            "de_ADS_CI",
            "deu",
            "zh_Hant_TW",
            "kok",
        ] {
            assert!(Dynamic::is_icu_locale(known), "{known}");
        }
        // Aliases without data, bibliographic codes, other separators and
        // tabs are refused.
        for unknown in [
            "zh_CN", "in", "ger", "en-US", "xx_YY", "\ten_US", "", "ANSI",
        ] {
            assert!(!Dynamic::is_icu_locale(unknown), "{unknown}");
        }
    }

    #[test]
    fn dynamic_keys_match_the_engine() {
        let Some(dir) = collation_dir() else { return };
        let mut checked = 0;
        for name in ["cze", "gen", "ger", "mac", "uni"] {
            let dump = dump(&format!("collation_dynamic/{name}.json"));
            let records = dump["records"].as_array().unwrap();
            for index in dump["indexes"].as_array().unwrap() {
                let collation = index["collation"].as_str().unwrap();
                let dynamic = Dynamic::open(&dir.join("adscollate.adt"), collation).unwrap();
                let field = index["expression"].as_str().unwrap();
                let key_length = index["key_length"].as_u64().unwrap() as usize;
                for key in index["keys"].as_array().unwrap() {
                    let record = &records.iter().find(|r| r["recno"] == key["recno"]).unwrap();
                    let Some(value) = bytes(&record["values"][field]) else {
                        continue;
                    };
                    let got: String = dynamic
                        .key(&value, key_length)
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect();
                    assert_eq!(
                        got,
                        key["key"].as_str().unwrap(),
                        "{name} {collation} {value:?}"
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 2000, "{checked}");
    }

    /// The keys of the `charset_german` fixture, built under the German ANSI
    /// collation (`ANSI_CHAR_SET=German`).
    #[test]
    fn static_keys_match_the_engine() {
        let Some(dir) = collation_dir() else { return };
        let chr = std::fs::read(dir.join("ansi.chr")).unwrap();
        let german = Static::from_chr(&chr, 16, "german").unwrap();
        assert_eq!(german.name, "German");
        let default = Static::from_chr(&chr, 16, "Engl(Amer)").unwrap();
        assert!(
            default
                .weights
                .iter()
                .enumerate()
                .all(|(i, &w)| usize::from(w) == i)
        );
        let mut checked = 0;
        let dump = dump("charset_german/ca.json");
        let records = dump["records"].as_array().unwrap();
        for index in dump["indexes"].as_array().unwrap() {
            let field = index["expression"].as_str().unwrap();
            let key_length = index["key_length"].as_u64().unwrap() as usize;
            for key in index["keys"].as_array().unwrap() {
                let record = &records.iter().find(|r| r["recno"] == key["recno"]).unwrap();
                let Some(mut value) = bytes(&record["values"][field]) else {
                    continue;
                };
                value.resize(key_length, b' ');
                let got: String = german
                    .key(&value)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                assert_eq!(got, key["key"].as_str().unwrap(), "{field} {value:?}");
                checked += 1;
            }
        }
        assert!(checked > 200, "{checked}");
        let extend = std::fs::read(dir.join("extend.chr")).unwrap();
        assert!(Static::from_chr(&extend, 14, "GERMAN").is_some());
        assert!(Static::from_chr(&chr, 16, "Klingon").is_none());
    }
}
