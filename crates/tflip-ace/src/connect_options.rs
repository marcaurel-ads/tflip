//! The options of `AdsConnect101`'s connect string, and what
//! `AdsOpenTable101` takes from them, as probed on the original engine
//! (`fixtures/config/connect101.json` and `connect101_values.json`,
//! `tools/ace/config/run_connect101.sh`).

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use tflip_engine::handle::Handle;
use tflip_engine::table::{TableType, options};

use crate::{ADS_ANSI, ADS_OEM};

/// What a connect string asks of the tables its connection opens and of
/// the connection itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Options {
    /// The type `AdsOpenTable101` opens tables as (`TableType`).
    pub table_type: TableType,
    /// Their character set (`CharType`).
    pub char_type: u16,
    /// Whether it opens them read-only (`ReadOnly`).
    pub read_only: bool,
    /// Whether it opens them exclusively (`Shared=FALSE`).
    pub exclusive: bool,
    /// Whether it opens them with compatible locking (`LockMode`).
    pub compatible_locking: bool,
    /// The connection's date format (`DateFormat`).
    pub date_format: Option<String>,
    /// The connection's "exact" setting (`Exact`).
    pub exact: Option<bool>,
    /// The connection's dynamic collation (`Collation`).
    pub collation: Option<String>,
    /// Whether `AdsGetString` trims the trailing spaces of what it reads
    /// when its caller does not say how (`TrimTrailingSpaces`).
    pub trim: bool,
    /// Whether the connection refuses free tables to users that are no
    /// administrators (`CheckFreeTableAccess`).
    pub check_free_tables: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            table_type: TableType::Adt,
            char_type: ADS_ANSI,
            read_only: false,
            exclusive: false,
            compatible_locking: false,
            date_format: None,
            exact: None,
            collation: None,
            trim: false,
            check_free_tables: false,
        }
    }
}

impl Options {
    /// The open options (`ADS_READONLY`, ...) `AdsOpenTable101` gives the
    /// tables it opens.
    pub fn open_options(&self) -> u32 {
        let mut open = 0;
        if self.read_only {
            open |= options::READONLY;
        }
        if self.exclusive {
            open |= options::EXCLUSIVE;
        }
        // An ADT table is always locked the proprietary way.
        if self.compatible_locking && self.table_type != TableType::Adt {
            open |= options::COMPATIBLE_LOCKING;
        }
        open
    }
}

/// How a key's value is checked. An empty one is refused naming the key,
/// except for `Free` keys, which take anything, and the two that say nothing
/// of it (`Compression` and `CommType`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Anything, empty too.
    Free,
    /// Anything.
    Text,
    /// `TRUE` or `FALSE` in any case.
    Boolean,
    /// A `Boolean`, or `1`, `0`, `YES` or `NO`.
    Switch,
    /// One of the names (each with the other spellings it has), in any case,
    /// refused as given or, if `upper`, in upper case.
    Names(Names, bool),
    /// A number, refused if it does not start with a digit.
    Number,
    /// The connection's date format.
    DateFormat,
    /// The connection's dynamic collation.
    Collation,
}

/// The names a key takes, each with its spellings.
type Names = &'static [&'static [&'static str]];

const TABLE_TYPES: Names = &[
    &["ADT", "ADS_ADT"],
    &["CDX", "ADS_CDX"],
    &["NTX", "ADS_NTX"],
    &["VFP", "ADS_VFP"],
];
const CHAR_TYPES: Names = &[&["ANSI", "ADS_ANSI"], &["OEM", "ADS_OEM"]];
const LOCK_MODES: Names = &[
    &["COMPATIBLE", "ADS_COMPATIBLE_LOCKING"],
    &["PROPRIETARY", "ADS_PROPRIETARY_LOCKING"],
];
const SECURITY_MODES: Names = &[
    &["CHECKRIGHTS", "ADS_CHECKRIGHTS"],
    &["IGNORERIGHTS", "ADS_IGNORERIGHTS"],
];
const COMM_TYPES: Names = &[&["TCP_IP"], &["UDP_IP"], &["IPX"], &["TLS"]];
const ENCRYPTIONS: Names = &[&["RC4"], &["AES128"], &["AES256"]];
const COMPRESSIONS: Names = &[&["INTERNET"], &["ALWAYS"], &["NEVER"]];

/// The key `name` (in any case), if the original engine knows it: how it
/// spells it, and how its value is checked.
fn key(name: &str) -> Option<(&'static str, Kind)> {
    use Kind::*;
    Some(match name.to_ascii_lowercase().as_str() {
        "datasource" | "data source" => ("data source", Free),
        "servertype" => ("servertype", Text),
        "userid" | "user id" => ("user id", Free),
        "password" => ("password", Free),
        "description" => ("description", Free),
        "compression" => ("compression", Names(COMPRESSIONS, false)),
        "commtype" => ("commtype", Names(COMM_TYPES, false)),
        "tlsciphers" => ("tlsciphers", Text),
        "tlscertificate" => ("tlscertificate", Text),
        "tlscommonname" => ("tlscommonname", Text),
        "fips" => ("fips", Switch),
        "encryptionpassword" => ("encryptionpassword", Text),
        "encryptiontype" => ("encryptiontype", Names(ENCRYPTIONS, false)),
        "ddpassword" => ("ddpassword", Text),
        "tabletype" => ("tabletype", Names(TABLE_TYPES, true)),
        "chartype" => ("chartype", Names(CHAR_TYPES, true)),
        "readonly" => ("readonly", Boolean),
        "shared" => ("shared", Boolean),
        "lockmode" => ("lockmode", Names(LOCK_MODES, true)),
        "securitymode" => ("securitymode", Names(SECURITY_MODES, true)),
        "showdeleted" => ("showdeleted", Boolean),
        "trimtrailingspaces" => ("trimtrailingspaces", Switch),
        "epoch" => ("epoch", Number),
        "exact" => ("exact", Boolean),
        "dateformat" => ("dateformat", DateFormat),
        "decimals" => ("decimals", Number),
        "sqltimeout" => ("sqltimeout", Number),
        "collation" => ("collation", Collation),
        "checkfreetableaccess" => ("checkfreetableaccess", Boolean),
        "incrementusercount" => ("incrementusercount", Boolean),
        "pooling" => ("pooling", Boolean),
        "max pool size" => ("max pool size", Text),
        "connection lifetime" => ("connection lifetime", Text),
        "connect timeout" => ("connect timeout", Text),
        _ => return None,
    })
}

/// Which of `names` `value` is, in any case.
fn name_in(value: &str, names: Names) -> Option<usize> {
    names.iter().position(|spellings| {
        spellings
            .iter()
            .any(|name| name.eq_ignore_ascii_case(value))
    })
}

/// The `key=value` items of `connect_string` as the original reads them, or
/// the text it adds to the `AE_INVALID_OPTION` for a key it does not know.
///
/// The original reads a key up to the next `=`, so a `;` does not end it:
/// `Foo;Bar=1` is the key `Foo;Bar`, and an item with no `=` takes the rest
/// of the string, `;`s included. Blanks and `;`s before a key are skipped.
/// A `=` that follows the previous `;` at once, with a value, fails with
/// that value up to the next `=` (`=a;Foo=1;` gives `a;Foo`); behind a blank
/// or a `;` it names no item and its value is dropped. A few strings with
/// several of these in them differ in the original (`KNOWN_ISSUES`).
fn scan(connect_string: &str) -> Result<Vec<(String, String)>, String> {
    let mut items = Vec::new();
    let mut at = 0;
    while at < connect_string.len() {
        let rest = &connect_string[at..];
        let Some(equals) = rest.find('=') else {
            let key = rest.trim_start_matches([' ', ';']).trim_end_matches(' ');
            match key.is_empty() {
                true => break,
                false if self::key(key).is_none() => return Err(key.to_owned()),
                false => items.push((key.to_owned(), String::new())),
            }
            break;
        };
        let raw = &rest[..equals];
        let name = raw.trim_start_matches([' ', ';']).trim_end_matches(' ');
        let after = &rest[equals + 1..];
        let value = after.trim_start_matches(' ');
        if raw.is_empty() {
            if !value.is_empty()
                && !value.starts_with(';')
                && !value.trim_matches([' ', ';']).is_empty()
            {
                let end = value.find('=').unwrap_or(value.len());
                return Err(value[..end].to_owned());
            }
        } else if !name.is_empty() && key(name).is_none() {
            return Err(name.to_owned());
        }
        let end = value.find(';').unwrap_or(value.len());
        items.push((
            name.to_owned(),
            value[..end].trim_end_matches(' ').to_owned(),
        ));
        at += equals + 1 + (after.len() - value.len()) + (end + 1).min(value.len());
    }
    Ok(items)
}

/// The options of `connect_string` (`key=value` items between `;`), or what
/// the original engine adds to the `AE_INVALID_OPTION` it fails with: the
/// value that is wrong (a name in upper case for some keys), the key itself
/// if its value is empty or the key is not one it knows, which is found
/// before any value is checked, nothing for an empty `Compression` or
/// `CommType`. A key given twice takes the last value.
pub(crate) fn parse(connect_string: &str) -> Result<Options, String> {
    let items = scan(connect_string)?;
    let mut parsed = Options::default();
    for (name, value) in items {
        let Some((canonical, kind)) = key(&name) else {
            continue;
        };
        let (name, value) = (name.as_str(), value.as_str());
        if value.is_empty() && kind != Kind::Free {
            return Err(match canonical {
                "compression" | "commtype" => String::new(),
                _ => name.to_owned(),
            });
        }
        match kind {
            Kind::Boolean | Kind::Switch => {
                let truth = match value.to_ascii_lowercase().as_str() {
                    "true" => true,
                    "false" => false,
                    "1" | "yes" if kind == Kind::Switch => true,
                    "0" | "no" if kind == Kind::Switch => false,
                    _ => return Err(value.to_owned()),
                };
                match canonical {
                    "readonly" => parsed.read_only = truth,
                    "shared" => parsed.exclusive = !truth,
                    "exact" => parsed.exact = Some(truth),
                    "trimtrailingspaces" => parsed.trim = truth,
                    "checkfreetableaccess" => parsed.check_free_tables = truth,
                    _ => {}
                }
            }
            Kind::Names(names, upper) => {
                let Some(found) = name_in(value, names) else {
                    return Err(match upper {
                        true => value.to_ascii_uppercase(),
                        false => value.to_owned(),
                    });
                };
                match canonical {
                    "tabletype" => {
                        parsed.table_type = match found {
                            1 => TableType::Cdx,
                            2 => TableType::Ntx,
                            3 => TableType::Vfp,
                            _ => TableType::Adt,
                        }
                    }
                    "chartype" => parsed.char_type = [ADS_ANSI, ADS_OEM][found],
                    "lockmode" => parsed.compatible_locking = found == 0,
                    _ => {}
                }
            }
            Kind::Number if !value.starts_with(|c: char| c.is_ascii_digit()) => {
                return Err(value.to_owned());
            }
            Kind::DateFormat => parsed.date_format = Some(value.to_owned()),
            Kind::Collation => {
                parsed.collation = Some(match value.eq_ignore_ascii_case("ansi") {
                    true => String::new(),
                    false => value.to_owned(),
                })
            }
            Kind::Free | Kind::Text | Kind::Number => {}
        }
    }
    Ok(parsed)
}

/// What the connect strings of the connections open say, by connection.
static OPTIONS: Mutex<BTreeMap<Handle, Options>> = Mutex::new(BTreeMap::new());

fn options_of() -> MutexGuard<'static, BTreeMap<Handle, Options>> {
    OPTIONS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Keeps the options connection `handle` was opened with.
pub(crate) fn remember(handle: Handle, options: Options) {
    options_of().insert(handle, options);
}

/// The options connection `handle` was opened with; the defaults for one
/// opened some other way.
pub(crate) fn of(handle: Handle) -> Options {
    options_of().get(&handle).cloned().unwrap_or_default()
}

/// Whether the connection `handle` trims the trailing spaces of the text
/// `AdsGetString` reads (`TrimTrailingSpaces`).
pub(crate) fn trims(handle: Handle) -> bool {
    options_of()
        .get(&handle)
        .is_some_and(|options| options.trim)
}

/// Forgets the options of connection `handle`, or of every connection
/// (handle 0).
pub(crate) fn forget(handle: Handle) {
    match handle {
        0 => options_of().clear(),
        _ => drop(options_of().remove(&handle)),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};

    use crate::{AE_SUCCESS, AdsHandle, Unsigned16, Unsigned32, fixtures};

    fn fixtures_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/config")
    }

    fn cases(file: &str) -> Vec<Value> {
        let text = std::fs::read_to_string(fixtures_dir().join(file)).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn last_error() -> String {
        let (mut code, mut buf, mut len) = (0, [0u8; 600], 600);
        unsafe { crate::AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
        String::from_utf8_lossy(&buf[..usize::from(len)]).into_owned()
    }

    fn text_of(read: impl FnOnce(*mut u8, *mut Unsigned16) -> Unsigned32) -> String {
        let (mut buf, mut len) = ([0u8; 300], 300);
        match read(buf.as_mut_ptr(), &mut len) {
            AE_SUCCESS => String::from_utf8_lossy(&buf[..usize::from(len)]).into_owned(),
            _ => String::new(),
        }
    }

    /// Connects with `Data Source=<dir>;ServerType=LOCAL;<options>`.
    fn connect(dir: &Path, options: &str) -> (Unsigned32, AdsHandle) {
        let mut string = format!("Data Source={};ServerType=LOCAL;{options}", dir.display());
        string.push('\0');
        let mut connection = 0;
        let rc = unsafe {
            crate::AdsConnect101(string.as_mut_ptr(), std::ptr::null_mut(), &mut connection)
        };
        (rc, connection)
    }

    /// What the probe printed for a table of a connection, run by tflip.
    fn table(dir: &Path, connection: AdsHandle, name: &str) -> Value {
        let mut name_c = format!("{name}\0");
        let mut table = 0;
        let rc = unsafe { crate::AdsOpenTable101(connection, name_c.as_mut_ptr(), &mut table) };
        let mut got = json!({ "open": rc });
        if rc != AE_SUCCESS {
            return got;
        }
        let (mut kind, mut chars, mut lock, mut opened, mut count) = (99, 99, 99, 0, 0);
        unsafe {
            crate::AdsGetTableType(table, &mut kind);
            crate::AdsGetTableCharType(table, &mut chars);
            crate::AdsGetTableLockType(table, &mut lock);
            crate::AdsGetTableOpenOptions(table, &mut opened);
            crate::AdsGetRecordCount(table, 2, &mut count);
        }
        got["type"] = json!(kind);
        got["char_type"] = json!(chars);
        got["lock_type"] = json!(lock);
        got["open_options"] = json!(opened);
        got["count"] = json!(count);
        got["goto_deleted"] = json!(crate::AdsGotoRecord(table, 2));
        crate::AdsGotoRecord(table, 1);
        // A text field set and read back (the connection's trimming).
        let mut field = if name.ends_with(".adt") {
            "name\0"
        } else {
            "C_CHAR\0"
        }
        .to_owned();
        let mut value = *b"ab  ";
        let set = unsafe {
            crate::AdsSetString(
                table,
                field.as_mut_ptr(),
                value.as_mut_ptr(),
                value.len() as u32,
            )
        };
        let (mut buf, mut len) = ([0u8; 100], 100);
        let read = unsafe {
            crate::AdsGetString(table, field.as_mut_ptr(), buf.as_mut_ptr(), &mut len, 0)
        };
        let text = String::from_utf8_lossy(&buf[..len as usize]).into_owned();
        got["text"] = json!([set, read, text]);
        got["append"] = json!(crate::AdsAppendRecord(table));
        // Whether another connection can open it exclusively meanwhile.
        let other = fixtures::connect_to(&dir.display().to_string());
        let mut path = format!("{}\0", dir.join(name).display());
        let mut held = 0;
        let exclusive = if matches!(kind, 1..=4) { kind } else { 3 };
        got["exclusive_elsewhere"] = json!(unsafe {
            crate::AdsOpenTable(
                other,
                path.as_mut_ptr(),
                std::ptr::null_mut(),
                exclusive,
                if chars == 0 { 1 } else { chars },
                1,
                2,
                1,
                &mut held,
            )
        });
        crate::AdsDisconnect(other);
        crate::AdsCloseTable(table);
        got
    }

    /// The options of a connect string do as the original engine's did
    /// (`fixtures/config/connect101.json`): which it refuses and with what
    /// text, the connection's settings, and how it opens each table.
    #[test]
    fn connect_strings_act_as_in_the_original_engine() {
        let _locked = fixtures::settings_lock();
        let (_, dir) = fixtures::scratch_from("connect101", &fixtures_dir().join("connect101"));
        for case in cases("connect101.json") {
            let string = case["string"].as_str().unwrap();
            let expected = &case["result"];
            // The tables are fresh copies, as the probe's.
            fixtures::copy_files(&fixtures_dir().join("connect101"), &dir);
            let (rc, connection) = connect(&dir, string);
            // The original's text for a collation it does not know is garbled.
            if expected["connect"] == 7147 {
                assert!(
                    rc == 7147 || crate::config::collation_dir().is_none(),
                    "{string}"
                );
                continue;
            }
            assert_eq!(rc, expected["connect"], "{string}");
            if rc != AE_SUCCESS {
                let text = expected["text"].as_str().map(str::to_owned);
                assert_eq!(Some(last_error()), text, "{string}");
                continue;
            }
            let collation =
                text_of(|buf, len| unsafe { crate::AdsGetCollation(connection, buf, len) });
            assert_eq!(
                collation,
                expected["collation"].as_str().unwrap(),
                "{string}"
            );
            let format =
                text_of(|buf, len| unsafe { crate::AdsGetDateFormat60(connection, buf, len) });
            assert_eq!(
                format,
                expected["date_format"].as_str().unwrap(),
                "{string}"
            );
            let mut exact = 99;
            let rc = unsafe { crate::AdsGetExact22(connection, &mut exact) };
            assert_eq!(json!([rc, exact]), expected["exact"], "{string}");
            for (name, wanted) in expected["tables"].as_object().unwrap() {
                let got = table(&dir, connection, name);
                // A DBF opened as ADT (or an ADT as DBF) fails with 7010, 7014 or 7016 in the original,
                // which tflip reports as 5081 (see KNOWN_ISSUES).
                if matches!(wanted["open"].as_u64(), Some(7010 | 7014 | 7016)) {
                    assert_ne!(got["open"], 0, "{string}: {name}");
                    continue;
                }
                // The original opens a Visual FoxPro table as a Clipper one
                // too, which tflip refuses (5025).
                if name == "v.dbf" && wanted["open"] == 0 && got["open"] == 5025 {
                    continue;
                }
                if wanted["open"] != 0 {
                    assert_eq!(got["open"], wanted["open"], "{string}: {name}");
                    continue;
                }
                for key in [
                    "open",
                    "type",
                    "char_type",
                    "lock_type",
                    "open_options",
                    "count",
                    "goto_deleted",
                    "text",
                    "append",
                    "exclusive_elsewhere",
                ] {
                    // Whether the exclusive open fails is what matters: the
                    // original says 7008, or 7016 for a table of another kind
                    // opened as a Visual FoxPro one.
                    if key == "exclusive_elsewhere" {
                        let failed = |value: &Value| value != 0;
                        assert_eq!(failed(&got[key]), failed(&wanted[key]), "{string}: {name}");
                        continue;
                    }
                    assert_eq!(got[key], wanted[key], "{string}: {name}: {key}");
                }
            }
            crate::AdsDisconnect(connection);
        }
    }

    /// The values each option takes (`fixtures/config/connect101_values.json`).
    #[test]
    fn option_values_are_checked_as_in_the_original_engine() {
        let _locked = fixtures::settings_lock();
        let (_, dir) =
            fixtures::scratch_from("connect101-values", &fixtures_dir().join("connect101"));
        for case in cases("connect101_values.json") {
            let string = case["string"].as_str().unwrap();
            let expected = &case["result"];
            let want = expected["connect"].as_u64().unwrap();
            // The environment's errors: no OpenSSL (a TLS transport or FIPS), no
            // remote server.
            if [5214, 6420, 5033].contains(&want) {
                continue;
            }
            let (rc, connection) = connect(&dir, string);
            // The original's text for a collation it does not know is garbled.
            if want == 7147 {
                assert!(
                    rc == 7147 || crate::config::collation_dir().is_none(),
                    "{string}"
                );
                continue;
            }
            assert_eq!(u64::from(rc), want, "{string}");
            match rc {
                AE_SUCCESS => {
                    let format = text_of(|buf, len| unsafe {
                        crate::AdsGetDateFormat60(connection, buf, len)
                    });
                    assert_eq!(
                        format,
                        expected["date_format"].as_str().unwrap(),
                        "{string}"
                    );
                    crate::AdsDisconnect(connection);
                }
                _ => {
                    let text = expected["text"].as_str().map(str::to_owned);
                    assert_eq!(Some(last_error()), text, "{string}");
                }
            }
        }
    }
}
