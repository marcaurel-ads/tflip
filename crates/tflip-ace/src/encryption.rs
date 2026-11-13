//! Table encryption: the Advantage proprietary RC4 scheme, enabled and applied
//! through `AdsEnableEncryption`/`AdsEncryptTable` and the record-level
//! `AdsEncryptRecord`, with the matching decrypt and disable calls, and the
//! queries that report a table's and a record's encryption. (AES encryption is
//! applied only when a table is created; these calls are RC4.)

use crate::backend::api;
use crate::ffi::text;
use crate::last_error::{Call, fail};
use crate::{
    ADS_FALSE, ADS_TRUE, AE_INVALID_POINTER, AE_SUCCESS, AdsHandle, Unsigned8, Unsigned16,
    Unsigned32, code, error_code,
};

/// Writes `value` (`ADS_TRUE`/`ADS_FALSE`) to `out`, or fails with
/// `AE_INVALID_POINTER` if `out` is null.
///
/// # Safety
///
/// `out` must be null or valid for writes.
unsafe fn write_bool(out: *mut Unsigned16, value: bool) -> Unsigned32 {
    // SAFETY: as the caller promises.
    unsafe { crate::field::write_at(Ok(if value { ADS_TRUE } else { ADS_FALSE }), out, 2) }
}

/// `AdsIsEncryptionEnabled`: whether the session holds the table's encryption
/// password (set by [`AdsEnableEncryption`] or by opening it with the password).
///
/// # Safety
///
/// `enabled` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsEncryptionEnabled(
    table: AdsHandle,
    enabled: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsIsEncryptionEnabled");
    match api(table).encryption_enabled(table) {
        // SAFETY: `enabled` is null or writable, as # Safety says.
        Ok(value) => unsafe { write_bool(enabled, value) },
        Err(error) => error_code(error),
    }
}

/// `AdsIsRecordEncrypted`: whether the current record is enciphered.
///
/// # Safety
///
/// `encrypted` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsRecordEncrypted(
    table: AdsHandle,
    encrypted: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsIsRecordEncrypted");
    match api(table).record_encrypted(table) {
        // SAFETY: `encrypted` is null or writable, as # Safety says.
        Ok(value) => unsafe { write_bool(encrypted, value) },
        Err(error) => error_code(error),
    }
}

/// `AdsVerifyPassword`: obsolete alias of [`AdsIsEncryptionEnabled`] — whether
/// the session holds the table's password.
///
/// # Safety
///
/// `enabled` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsVerifyPassword(
    table: AdsHandle,
    enabled: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsVerifyPassword");
    match api(table).encryption_enabled(table) {
        // SAFETY: `enabled` is null or writable, as # Safety says.
        Ok(value) => unsafe { write_bool(enabled, value) },
        Err(error) => error_code(error),
    }
}

/// `AdsIsTableEncrypted`: whether a table is encrypted (`ADS_TRUE`) or not.
///
/// A table reports its header's encryption flag.
///
/// # Safety
///
/// `encrypted` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsTableEncrypted(
    table: AdsHandle,
    encrypted: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsIsTableEncrypted");
    let opened = match api(table).table_info(table) {
        Ok(opened) => opened,
        Err(error) => return error_code(error),
    };
    if encrypted.is_null() {
        return crate::last_error::null_param(2);
    }
    let value = opened.structure.header.encrypted();
    // SAFETY: not null, so writable, as # Safety says.
    unsafe { *encrypted = if value { ADS_TRUE } else { ADS_FALSE } };
    AE_SUCCESS
}

/// `AdsEnableEncryption`: set the table's RC4 encryption password, so later
/// writes encipher and already-enciphered records read deciphered. It writes the
/// header's password check and type but does not encrypt existing records.
///
/// # Safety
///
/// `password` must be null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsEnableEncryption(
    table: AdsHandle,
    password: *mut Unsigned8,
) -> Unsigned32 {
    let _call = Call::begin("AdsEnableEncryption");
    // SAFETY: `password` is null or NUL-terminated, as # Safety says.
    let Some(password) = (unsafe { text(password) }) else {
        return fail(AE_INVALID_POINTER);
    };
    code(api(table).enable_encryption(table, password))
}

/// `AdsDisableEncryption`: drop the session's password, so later writes are in
/// the clear and reads stop deciphering. The file is left as it is.
#[unsafe(no_mangle)]
pub extern "system" fn AdsDisableEncryption(table: AdsHandle) -> Unsigned32 {
    let _call = Call::begin("AdsDisableEncryption");
    code(api(table).disable_encryption(table))
}

/// `AdsEncryptTable`: encipher every record and memo value and mark the table
/// encrypted. The table must be open exclusively with its password enabled.
#[unsafe(no_mangle)]
pub extern "system" fn AdsEncryptTable(table: AdsHandle) -> Unsigned32 {
    let _call = Call::begin("AdsEncryptTable");
    code(api(table).encrypt_table(table))
}

/// `AdsDecryptTable`: decipher every record and memo value and mark the table
/// unencrypted. The table must be open exclusively with its password enabled.
#[unsafe(no_mangle)]
pub extern "system" fn AdsDecryptTable(table: AdsHandle) -> Unsigned32 {
    let _call = Call::begin("AdsDecryptTable");
    code(api(table).decrypt_table(table))
}

/// `AdsEncryptRecord`: encipher the current record (record-level, the table
/// flag untouched); the table's password must be enabled.
#[unsafe(no_mangle)]
pub extern "system" fn AdsEncryptRecord(table: AdsHandle) -> Unsigned32 {
    let _call = Call::begin("AdsEncryptRecord");
    code(api(table).encrypt_record(table))
}

/// `AdsDecryptRecord`: decipher the current record.
#[unsafe(no_mangle)]
pub extern "system" fn AdsDecryptRecord(table: AdsHandle) -> Unsigned32 {
    let _call = Call::begin("AdsDecryptRecord");
    code(api(table).decrypt_record(table))
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;

    use super::*;
    use crate::fixtures;

    /// What the original engine did (`tools/ace/crypto/share.c`): a table's
    /// encryption password belongs to the handle that gave it, not to the
    /// file other handles on it share.
    fn golden() -> serde_json::Value {
        let path = fixtures::dir("").join("../write/encryption/share.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// The golden's number `key`.
    fn number(golden: &serde_json::Value, key: &str) -> u32 {
        golden[key].as_u64().unwrap_or_else(|| panic!("{key}")) as u32
    }

    /// The golden's text `key`, as the bytes the engine returned.
    fn bytes(golden: &serde_json::Value, key: &str) -> Vec<u8> {
        let text = golden[key].as_str().unwrap_or_else(|| panic!("{key}"));
        text.chars()
            .map(|c| u8::try_from(u32::from(c)).unwrap())
            .collect()
    }

    /// Opens the ADT table `name` on `connect` with `options`.
    fn open(connect: AdsHandle, name: &CStr, options: Unsigned32) -> (Unsigned32, AdsHandle) {
        let mut name = name.to_bytes_with_nul().to_vec();
        let mut table = 0;
        let null = std::ptr::null_mut();
        let rc = unsafe {
            crate::AdsOpenTable(
                connect,
                name.as_mut_ptr(),
                null,
                crate::ADS_ADT,
                1,
                1,
                1,
                options,
                &mut table,
            )
        };
        (rc, table)
    }

    fn enable(table: AdsHandle, password: &CStr) -> Unsigned32 {
        let mut password = password.to_bytes_with_nul().to_vec();
        unsafe { AdsEnableEncryption(table, password.as_mut_ptr()) }
    }

    /// The text of field `field` of the first record, as `AdsGetField` gives
    /// it.
    fn field(table: AdsHandle, field: &CStr) -> (Unsigned32, Vec<u8>) {
        crate::AdsGotoTop(table);
        let mut name = field.to_bytes_with_nul().to_vec();
        let mut buf = vec![0u8; 256];
        let mut len = buf.len() as Unsigned32;
        let rc =
            unsafe { crate::AdsGetField(table, name.as_mut_ptr(), buf.as_mut_ptr(), &mut len, 0) };
        buf.truncate(if rc == AE_SUCCESS { len as usize } else { 0 });
        (rc, buf)
    }

    type Query = unsafe extern "system" fn(AdsHandle, *mut Unsigned16) -> Unsigned32;

    fn flag(query: Query, table: AdsHandle) -> u32 {
        let mut value = 0x55;
        unsafe { query(table, &mut value) };
        u32::from(value)
    }

    /// Checks what handle `table` reads against the golden's `tag` entries.
    fn reads_as(golden: &serde_json::Value, tag: &str, table: AdsHandle) {
        let key = |what: &str| format!("{tag}_{what}");
        assert_eq!(
            flag(AdsIsEncryptionEnabled, table),
            number(golden, &key("enabled")),
            "{tag}"
        );
        let mut count = 0;
        assert_eq!(
            unsafe { crate::AdsGetRecordCount(table, 2, &mut count) },
            AE_SUCCESS
        );
        assert_eq!(count, number(golden, &key("count")), "{tag}");
        assert_eq!(
            field(table, c"D1"),
            (0, bytes(golden, &key("field"))),
            "{tag} D1"
        );
        assert_eq!(
            field(table, c"M1"),
            (0, bytes(golden, &key("memo"))),
            "{tag} M1"
        );
        let encrypted = number(golden, &key("record_encrypted"));
        crate::AdsGotoTop(table);
        assert_eq!(flag(AdsIsRecordEncrypted, table), encrypted, "{tag}");
        // The index's keys are not encrypted: a seek finds its record.
        if golden.get(key("seek_record")).is_some() {
            let mut order = 0;
            let mut name = b"IDX1\0".to_vec();
            let rc = unsafe { crate::AdsGetIndexHandle(table, name.as_mut_ptr(), &mut order) };
            assert_eq!(rc, AE_SUCCESS);
            let (mut text, mut found) = (b"bravo".to_vec(), 0);
            let rc = unsafe { crate::AdsSeek(order, text.as_mut_ptr(), 5, 2, 2, &mut found) };
            assert_eq!(rc, AE_SUCCESS);
            assert_eq!(
                u32::from(found),
                number(golden, &key("seek_found")),
                "{tag}"
            );
            let mut record = 0;
            assert_eq!(
                unsafe { crate::AdsGetRecordNum(table, 2, &mut record) },
                AE_SUCCESS
            );
            assert_eq!(record, number(golden, &key("seek_record")), "{tag}");
        }
    }

    /// Checks what handle `table`'s writes do against the golden's `tag`
    /// entries.
    fn writes_as(golden: &serde_json::Value, tag: &str, table: AdsHandle) {
        let key = |what: &str| format!("{tag}_{what}");
        crate::AdsGotoTop(table);
        let mut name = b"D1\0".to_vec();
        let mut value = b"zulu".to_vec();
        let rc = unsafe { crate::AdsSetField(table, name.as_mut_ptr(), value.as_mut_ptr(), 4) };
        assert_eq!(rc, number(golden, &key("setfield_rc")), "{tag} set");
        crate::AdsCancelUpdate(table);
        let rc = crate::AdsAppendRecord(table);
        assert_eq!(rc, number(golden, &key("append_rc")), "{tag} append");
        crate::AdsCancelUpdate(table);
        crate::AdsGotoTop(table);
        let rc = crate::AdsDeleteRecord(table);
        assert_eq!(rc, number(golden, &key("delete_rc")), "{tag} delete");
        crate::AdsGotoTop(table);
        let rc = AdsDecryptRecord(table);
        assert_eq!(
            rc,
            number(golden, &key("decryptrecord_rc")),
            "{tag} decrypt record"
        );
        let rc = AdsEncryptRecord(table);
        assert_eq!(
            rc,
            number(golden, &key("encryptrecord_rc")),
            "{tag} encrypt record"
        );
    }

    /// As in the original engine: a handle that has not given an encrypted
    /// table's password reads its records enciphered and its memos empty, and
    /// may not write, though another handle on the same file (of the same
    /// connection, of another one, or of another client of a server) gave it.
    /// A wrong password is refused, and one handle's `AdsDisableEncryption`
    /// leaves the others as they are.
    #[test]
    fn the_password_belongs_to_the_handle() {
        let golden = golden();
        let fixture = fixtures::dir("").join("../write/encryption");
        let (source, dir) = fixtures::scratch_from("encryption_share", &fixture);
        // The probe's own table, as it made it.
        for extension in ["adt", "adm", "adi"] {
            let from = dir.join("share").with_extension(extension);
            std::fs::copy(from, dir.join("enc").with_extension(extension)).unwrap();
        }
        let before = std::fs::read(dir.join("enc.adt")).unwrap();
        let (c1, c2) = (fixtures::connect_to(&source), fixtures::connect_to(&source));

        let (rc, h1) = open(c1, c"enc.adt", crate::ADS_SHARED);
        assert_eq!(rc, AE_SUCCESS);
        assert_eq!(enable(h1, c"secret"), AE_SUCCESS);
        assert_eq!(field(h1, c"D1"), (0, bytes(&golden, "h1_field")));
        assert_eq!(field(h1, c"M1"), (0, bytes(&golden, "h1_memo")));
        // Another handle of the same connection, and one of another.
        for (tag, connect) in [("h2", c1), ("h3", c2)] {
            let (rc, table) = open(connect, c"enc.adt", crate::ADS_SHARED);
            assert_eq!(rc, AE_SUCCESS);
            reads_as(&golden, tag, table);
            writes_as(&golden, tag, table);
            assert_eq!(crate::AdsCloseTable(table), AE_SUCCESS);
        }
        // A wrong password.
        let (_, h4) = open(c2, c"enc.adt", crate::ADS_SHARED);
        assert_eq!(enable(h4, c"wrong"), number(&golden, "h4_enable_rc"));
        reads_as(&golden, "h4", h4);
        // One handle letting go of the password leaves another's.
        let (_, h5) = open(c2, c"enc.adt", crate::ADS_SHARED);
        assert_eq!(enable(h5, c"secret"), AE_SUCCESS);
        assert_eq!(AdsDisableEncryption(h1), AE_SUCCESS);
        reads_as(&golden, "h1_disabled", h1);
        reads_as(&golden, "h5", h5);
        for table in [h1, h4, h5] {
            assert_eq!(crate::AdsCloseTable(table), AE_SUCCESS);
        }
        // The handle without the password opened first.
        let (_, h6) = open(c1, c"enc.adt", crate::ADS_SHARED);
        let (_, h7) = open(c2, c"enc.adt", crate::ADS_SHARED);
        assert_eq!(enable(h7, c"secret"), AE_SUCCESS);
        reads_as(&golden, "h6", h6);
        reads_as(&golden, "h7", h7);
        assert_eq!(crate::AdsCloseTable(h6), AE_SUCCESS);
        assert_eq!(crate::AdsCloseTable(h7), AE_SUCCESS);
        // Exclusively, without the password.
        let (_, h8) = open(c1, c"enc.adt", crate::ADS_EXCLUSIVE);
        assert_eq!(AdsDecryptTable(h8), number(&golden, "h8_decrypttable_rc"));
        assert_eq!(AdsEncryptTable(h8), number(&golden, "h8_encrypttable_rc"));
        reads_as(&golden, "h8", h8);
        assert_eq!(crate::AdsCloseTable(h8), AE_SUCCESS);
        crate::AdsDisconnect(c1);
        crate::AdsDisconnect(c2);
        assert_eq!(
            std::fs::read(dir.join("enc.adt")).unwrap(),
            before,
            "the file is unchanged"
        );
    }

    /// What the original engine did (`tools/ace/crypto/freeopen.c`) opening
    /// a dictionary's tables as free tables; its folder is `<data>`.
    fn free_golden() -> serde_json::Value {
        let path = fixtures::dir("").join("../write/encryption/freeopen.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// What `AdsGetLastError` says.
    fn last_error() -> (Unsigned32, String) {
        let (mut code, mut buf, mut len) = (0, [0u8; 1200], 1200u16);
        unsafe { crate::AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
        (
            code,
            buf[..usize::from(len)]
                .iter()
                .map(|&b| char::from(b))
                .collect(),
        )
    }

    /// Runs `sql` on `connect`: its result code, and what `AdsGetLastError`
    /// said then.
    fn execute(connect: AdsHandle, sql: &str) -> (Unsigned32, (Unsigned32, String)) {
        let mut statement = 0;
        let rc = unsafe { crate::AdsCreateSQLStatement(connect, &mut statement) };
        assert_eq!(rc, AE_SUCCESS);
        let mut sql = format!("{sql}\0").into_bytes();
        let mut cursor = 0;
        let rc = unsafe { crate::AdsExecuteSQLDirect(statement, sql.as_mut_ptr(), &mut cursor) };
        let error = last_error();
        if cursor != 0 {
            crate::AdsCloseTable(cursor);
        }
        crate::AdsCloseSQLStatement(statement);
        (rc, error)
    }

    /// As in the original engine: a table of a dictionary that encrypts its
    /// structure does not open as a free table. Without the table's password
    /// (none can be given) the password is wrong (7060, naming the file);
    /// while its dictionary has it open, it is a dictionary's table (5159);
    /// through SQL, either is the AQE error 7200.
    #[test]
    fn a_dictionarys_encrypted_table_does_not_open_free() {
        let golden = free_golden();
        let text = |key: &str| {
            golden[key]
                .as_str()
                .unwrap_or_else(|| panic!("{key}"))
                .to_owned()
        };
        let (source, dir) = fixtures::scratch_from(
            "encryption_freeopen",
            &fixtures::dir("dictionary_encrypted"),
        );
        let free = fixtures::connect_to(&source);
        for tag in ["rc4_secret", "rc4_memos", "rc4_plain"] {
            let name = format!("{tag}.adt");
            let file = dir.join(&name).display().to_string();
            let theirs = format!(r"<data>\rc4\{name}");
            let c_name = std::ffi::CString::new(name.clone()).unwrap();
            for (how, options) in [
                ("shared", crate::ADS_SHARED),
                ("exclusive", crate::ADS_EXCLUSIVE),
            ] {
                let key = format!("{tag}_{how}");
                let (rc, _) = open(free, &c_name, options);
                assert_eq!(rc, number(&golden, &format!("{key}_rc")), "{key}");
                let expected = text(&format!("{key}_text")).replace(&theirs, &file);
                assert_eq!(last_error(), (rc, expected), "{key}");
            }
            let sql = format!("SELECT * FROM \"{name}\"");
            let (rc, error) = execute(free, &sql);
            assert_eq!(rc, number(&golden, &format!("{tag}_sql_rc")), "{tag} SQL");
            let expected = text(&format!("{tag}_sql_text"))
                .replace(
                    &format!("Table name: {theirs}"),
                    &format!("Table name: {name}"),
                )
                .replace(&theirs, &file);
            assert_eq!(error, (rc, expected), "{tag} SQL");
        }

        // While the dictionary has it open.
        let (add, types) = match fixtures::is_remote() {
            true => (
                format!("{source}\\rc4.add\0").into_bytes(),
                crate::ADS_REMOTE_SERVER,
            ),
            false => (
                format!("{source}/rc4.add\0").into_bytes(),
                crate::ADS_LOCAL_SERVER,
            ),
        };
        let mut add = add;
        let (mut user, mut password) = (b"ADSSYS\0".to_vec(), b"adssyspw\0".to_vec());
        let mut dictionary = 0;
        let rc = unsafe {
            crate::AdsConnect60(
                add.as_mut_ptr(),
                types,
                user.as_mut_ptr(),
                password.as_mut_ptr(),
                0,
                &mut dictionary,
            )
        };
        assert_eq!(rc, AE_SUCCESS);
        let (rc, bound) = open(dictionary, c"rc4_secret", crate::ADS_SHARED);
        assert_eq!(rc, AE_SUCCESS);
        let (rc, _) = open(free, c"rc4_secret.adt", crate::ADS_SHARED);
        assert_eq!(rc, number(&golden, "rc4_secret_whilebound_rc"));
        assert_eq!(last_error(), (rc, text("rc4_secret_whilebound_text")));
        let (rc, error) = execute(free, "SELECT * FROM \"rc4_secret.adt\"");
        assert_eq!(rc, number(&golden, "rc4_secret_whilebound_sql_rc"));
        let expected = text("rc4_secret_whilebound_sql_text").replace(
            r"Table name: <data>\rc4\rc4_secret.adt",
            "Table name: rc4_secret.adt",
        );
        assert_eq!(error, (rc, expected));
        // Closed again, the dictionary still connected.
        assert_eq!(crate::AdsCloseTable(bound), AE_SUCCESS);
        let (rc, _) = open(free, c"rc4_secret.adt", crate::ADS_SHARED);
        assert_eq!(rc, number(&golden, "rc4_secret_afterbound_rc"));
        crate::AdsDisconnect(dictionary);
        crate::AdsDisconnect(free);
    }

    /// Connects to `source` (a folder) with the connect string's extras.
    fn connect_string(source: &str, extra: &str) -> AdsHandle {
        let server = if fixtures::is_remote() {
            "REMOTE"
        } else {
            "LOCAL"
        };
        let string =
            std::ffi::CString::new(format!("Data Source={source};ServerType={server};{extra}"))
                .unwrap();
        let (mut options, mut handle) = (0, 0);
        let rc = unsafe {
            crate::AdsConnect101(string.as_ptr().cast_mut().cast(), &mut options, &mut handle)
        };
        assert_eq!(rc, AE_SUCCESS, "{extra}");
        handle
    }

    /// As in the original engine: a table of an AES dictionary, with its
    /// structure enciphered, does not open as a free table (7068, whatever
    /// the connection) unless its dictionary has it open, when it is a
    /// dictionary's table (5159)
    /// (`fixtures/write/encryption/freeopen.json`, `aes_t_*`, `aesc_t_*`).
    #[test]
    fn an_aes_dictionarys_table_does_not_open_free() {
        let golden = free_golden();
        let text = |key: &str| golden[key].as_str().unwrap().to_owned();
        let (source, dir) =
            fixtures::scratch_from("encryption_freeopen_aes", &fixtures::dir("../aes/aes256"));
        let aes = "EncryptionType=AES256;FIPS=FALSE;DDPassword=ddsecret;";
        let plain = connect_string(&source, "");
        let aes_free = connect_string(&source, aes);
        for (tag, free, name) in [
            ("aes_t", plain, "t"),
            ("aesc_t", aes_free, "t"),
            ("aesc_m", aes_free, "m"),
        ] {
            let file = dir.join(format!("{name}.adt")).display().to_string();
            let theirs = format!(r"<data>\aes256\{name}.adt");
            let c_name = std::ffi::CString::new(format!("{name}.adt")).unwrap();
            let check = |step: &str, (rc, error): (Unsigned32, (Unsigned32, String))| {
                let key = format!("{tag}_{step}");
                assert_eq!(rc, number(&golden, &format!("{key}_rc")), "{key}");
                if rc != AE_SUCCESS {
                    let expected = text(&format!("{key}_text"))
                        .replace(
                            &format!("Table name: {theirs}"),
                            &format!("Table name: {name}.adt"),
                        )
                        .replace(&theirs, &file);
                    assert_eq!(error, (rc, expected), "{key}");
                }
            };
            let (rc, _) = open(free, &c_name, crate::ADS_SHARED);
            check("shared", (rc, last_error()));
            let (rc, _) = open(free, &c_name, crate::ADS_EXCLUSIVE);
            check("exclusive", (rc, last_error()));
            let (rc, error) = execute(free, &format!("SELECT * FROM \"{name}.adt\""));
            check("sql", (rc, error));
            let (rc, error) = execute(free, &format!("DELETE FROM \"{name}.adt\" WHERE 1 = 0"));
            check("sqldelete", (rc, error));
            let separator = if fixtures::is_remote() { '\\' } else { '/' };
            let dictionary = connect_string(
                &format!("{source}{separator}aes256.add"),
                &format!("User ID=ADSSYS;{aes}"),
            );
            let (rc, _) = open(free, &c_name, crate::ADS_SHARED);
            check("dictonly", (rc, last_error()));
            let bound_name = std::ffi::CString::new(name).unwrap();
            let (rc, bound) = open(dictionary, &bound_name, crate::ADS_SHARED);
            assert_eq!(rc, AE_SUCCESS, "{tag} bound");
            let (rc, _) = open(free, &c_name, crate::ADS_SHARED);
            check("whilebound", (rc, last_error()));
            let (rc, error) = execute(free, &format!("SELECT * FROM \"{name}.adt\""));
            check("whilebound_sql", (rc, error));
            assert_eq!(crate::AdsCloseTable(bound), AE_SUCCESS);
            let (rc, _) = open(free, &c_name, crate::ADS_SHARED);
            check("afterbound", (rc, last_error()));
            crate::AdsDisconnect(dictionary);
        }
        crate::AdsDisconnect(plain);
        crate::AdsDisconnect(aes_free);
    }

    /// A dictionary's encrypted table opens through the dictionary with its
    /// login password and is read there, locally and through
    /// `tflipd` (whose client sends no password with the open: the server
    /// has the login's).
    #[test]
    fn a_dictionarys_encrypted_table_is_read_through_the_dictionary() {
        let (source, _dir) = fixtures::scratch_from(
            "encryption_dictwrite",
            &fixtures::dir("dictionary_encrypted"),
        );
        let (mut add, types) = match fixtures::is_remote() {
            true => (format!("{source}\\rc4.add\0"), crate::ADS_REMOTE_SERVER),
            false => (format!("{source}/rc4.add\0"), crate::ADS_LOCAL_SERVER),
        };
        let (mut user, mut password) = (b"ADSSYS\0".to_vec(), b"adssyspw\0".to_vec());
        let mut connect = 0;
        let rc = unsafe {
            crate::AdsConnect60(
                add.as_mut_ptr(),
                types,
                user.as_mut_ptr(),
                password.as_mut_ptr(),
                0,
                &mut connect,
            )
        };
        assert_eq!(rc, AE_SUCCESS);
        let (rc, table) = open(connect, c"rc4_secret", crate::ADS_SHARED);
        assert_eq!(rc, AE_SUCCESS);
        assert_eq!(field(table, c"name").1, b"hidden              ");
        let mut count = 0;
        unsafe { crate::AdsGetRecordCount(table, 2, &mut count) };
        assert_eq!(count, 2);
        assert_eq!(crate::AdsCloseTable(table), AE_SUCCESS);
        // A statement of the dictionary connection reads it too.
        let (rc, _) = execute(connect, "SELECT * FROM rc4_secret");
        assert_eq!(rc, AE_SUCCESS);
        crate::AdsDisconnect(connect);
    }

    /// An AES dictionary connected to with its dictionary password
    /// (`DDPassword`, which a connection to `tflipd` sends along with the
    /// login) reads and writes its encrypted tables, on either server:
    /// 7068 without it, 7060 with a wrong one, as the original does
    /// (`fixtures/aes/dict/aesdict3.json`).
    #[test]
    fn an_aes_dictionary_opens_with_its_dictionary_password() {
        let (source, _dir) =
            fixtures::scratch_from("encryption_aesdict", &fixtures::dir("../aes/dict/e3"));
        let (separator, server) = match fixtures::is_remote() {
            true => ('\\', "REMOTE"),
            false => ('/', "LOCAL"),
        };
        let connect = |user: &str, password: &str, dd: &str| {
            let string = std::ffi::CString::new(format!(
                "Data Source={source}{separator}e3.add;ServerType={server};User ID={user};\
                 Password={password};EncryptionType=AES128;FIPS=FALSE;{dd}"
            ))
            .unwrap();
            let (mut options, mut handle) = (0, 0);
            let rc = unsafe {
                crate::AdsConnect101(string.as_ptr().cast_mut().cast(), &mut options, &mut handle)
            };
            (rc, handle)
        };
        // Processes remember a dictionary's key, so the failures come first,
        // as `aesdict3.json` has them.
        assert_eq!(connect("ADSSYS", "admpw", "").0, 7068);
        assert_eq!(connect("ADSSYS", "admpw", "DDPassword=wrong;").0, 7060);
        assert_eq!(connect("u1", "wrongpw", "DDPassword=ddpw;").0, 7078);
        for (user, password) in [("ADSSYS", "admpw"), ("u1", "userpw")] {
            let (rc, handle) = connect(user, password, "DDPassword=ddpw;");
            assert_eq!(rc, AE_SUCCESS, "{user}");
            assert_eq!(execute(handle, "SELECT * FROM t").0, AE_SUCCESS);
            let (rc, table) = open(handle, c"t", crate::ADS_SHARED);
            assert_eq!(rc, AE_SUCCESS, "{user}");
            let mut count = 0;
            unsafe { crate::AdsGetRecordCount(table, 2, &mut count) };
            if user == "ADSSYS" {
                let before = count;
                assert_eq!(
                    execute(handle, "INSERT INTO t (id, name) VALUES (7, 'aes')").0,
                    0
                );
                unsafe { crate::AdsGetRecordCount(table, 2, &mut count) };
                assert_eq!(count, before + 1);
            }
            assert_eq!(crate::AdsCloseTable(table), AE_SUCCESS);
            crate::AdsDisconnect(handle);
        }
    }

    /// What the original engine did (`tools/ace/crypto/stmtpw.sh`) with the
    /// table passwords of a statement: its script, `tools/ace/crypto/stmtpw.txt`,
    /// replayed step by step, each step's result code, `AdsGetLastError` text,
    /// and the record count and first value of its cursor compared.
    #[test]
    fn a_statement_opens_an_encrypted_table_with_the_password_it_is_given() {
        /// The step `tag`'s result against the golden's; a cursor is read and closed.
        fn check(
            golden: &serde_json::Value,
            tag: &str,
            rc: Unsigned32,
            cursor: AdsHandle,
            (ours, theirs): (&str, &str),
        ) {
            let error = last_error();
            let number = |what: &str| golden[format!("{tag}_{what}")].as_u64();
            assert_eq!(u64::from(rc), number("rc").unwrap(), "{tag} rc");
            if rc != AE_SUCCESS {
                let expected = golden[format!("{tag}_text")]
                    .as_str()
                    .unwrap()
                    .replace(theirs, ours);
                assert_eq!(error, (number("err").unwrap() as u32, expected), "{tag}");
            }
            if cursor != 0 {
                let mut count = 0;
                unsafe { crate::AdsGetRecordCount(cursor, 1, &mut count) };
                assert_eq!(u64::from(count), number("count").unwrap(), "{tag} count");
                if count > 0 {
                    assert_eq!(
                        golden[format!("{tag}_first")].as_str().map(str::as_bytes),
                        Some(field(cursor, c"id").1.as_slice()),
                        "{tag} first"
                    );
                }
                crate::AdsCloseTable(cursor);
            }
        }

        let golden: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(fixtures::dir("").join("../write/encryption/stmtpw.json"))
                .unwrap(),
        )
        .unwrap();
        let script = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tools/ace/crypto/stmtpw.txt"),
        )
        .unwrap();
        let (source, dir) = fixtures::scratch_from("encryption_stmtpw", &fixtures::empty_dir());
        // The error texts name the table's file, which is in the folder.
        let folder = format!("{}/", dir.display());
        let names = (folder.as_str(), r"<data>\");
        let bytes_of = |text: &str| format!("{text}\0").into_bytes();
        let pointer = |bytes: &mut Option<Vec<u8>>| {
            bytes
                .as_mut()
                .map_or(std::ptr::null_mut(), |bytes| bytes.as_mut_ptr())
        };
        let (mut connect, mut statement, mut table) = (0, 0, 0);
        let lines = script
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'));
        for (step, line) in lines.enumerate() {
            let tag = format!("{:03}", step + 1);
            assert_eq!(golden[format!("{tag}_cmd")].as_str(), Some(line), "{tag}");
            let line = line
                .replace("%D", &dir.display().to_string())
                .replace('\\', "/");
            let (op, arg) = line.split_once(' ').unwrap_or((&line, ""));
            let mut cursor = 0;
            let rc = match op {
                "connect" => {
                    connect = fixtures::connect_to(&source);
                    AE_SUCCESS
                }
                "disconnect" => {
                    crate::AdsCloseSQLStatement(statement);
                    crate::AdsDisconnect(connect)
                }
                "sql" => {
                    let (mut once, mut sql) = (0, bytes_of(arg));
                    unsafe { crate::AdsCreateSQLStatement(connect, &mut once) };
                    let rc =
                        unsafe { crate::AdsExecuteSQLDirect(once, sql.as_mut_ptr(), &mut cursor) };
                    check(&golden, &tag, rc, cursor, names);
                    crate::AdsCloseSQLStatement(once);
                    continue;
                }
                "stmt" => {
                    crate::AdsCloseSQLStatement(statement);
                    unsafe { crate::AdsCreateSQLStatement(connect, &mut statement) }
                }
                // No statement, or the connection's handle in its place.
                "stmt0" => {
                    crate::AdsCloseSQLStatement(statement);
                    statement = if arg == "conn" { connect } else { 0 };
                    AE_SUCCESS
                }
                "s.exec" => {
                    let mut sql = bytes_of(arg);
                    unsafe { crate::AdsExecuteSQLDirect(statement, sql.as_mut_ptr(), &mut cursor) }
                }
                "s.prepare" => {
                    let mut sql = bytes_of(arg);
                    unsafe { crate::AdsPrepareSQL(statement, sql.as_mut_ptr()) }
                }
                "s.run" => unsafe { crate::AdsExecuteSQL(statement, &mut cursor) },
                "s.setpw" => {
                    let (name, password) = arg.split_once(' ').unwrap_or((arg, ""));
                    let mut name = (name != "NULL").then(|| bytes_of(name));
                    let mut password = (password != "NULL").then(|| bytes_of(password));
                    let (name, password) = (pointer(&mut name), pointer(&mut password));
                    unsafe { crate::AdsStmtSetTablePassword(statement, name, password) }
                }
                "s.clear" => crate::AdsStmtClearTablePasswords(statement),
                "s.enable" => {
                    let mut password = (arg != "NULL").then(|| bytes_of(arg));
                    unsafe { crate::AdsStmtEnableEncryption(statement, pointer(&mut password)) }
                }
                "s.disable" => crate::AdsStmtDisableEncryption(statement),
                "topen" => {
                    // The original resolves a table of the probe's folder
                    // by its full path only; tflip's connection finds it.
                    let file = arg.rsplit('/').next().unwrap();
                    let (rc, opened) = open(connect, &std::ffi::CString::new(file).unwrap(), 1);
                    table = opened;
                    rc
                }
                "t.enable" => enable(table, &std::ffi::CString::new(arg).unwrap()),
                "tencrypt" => crate::AdsEncryptTable(table),
                "tclose" => crate::AdsCloseTable(table),
                _ => panic!("{tag}: {line}"),
            };
            check(&golden, &tag, rc, cursor, names);
        }
    }

    /// A connection whose `EncryptionType` is AES lays out the tables it
    /// creates for AES, on either server (a client of `tflipd` sends the type
    /// after connecting): a record is 8 bytes longer, and the table takes a
    /// password and is encrypted.
    #[test]
    fn an_aes_connection_lays_out_the_tables_it_creates() {
        let (source, _) = fixtures::scratch_from("encryption_aes_create", &fixtures::empty_dir());
        let length_of = |extra: &str, file: &str| {
            let connect = connect_string(&source, extra);
            let mut name = format!("{file}.adt\0").into_bytes();
            let mut fields = b"id,Integer;name,Character,10\0".to_vec();
            let mut table = 0;
            let rc = unsafe {
                crate::AdsCreateTable71(
                    connect,
                    name.as_mut_ptr(),
                    std::ptr::null_mut(),
                    crate::ADS_ADT,
                    1,
                    1,
                    1,
                    0,
                    fields.as_mut_ptr(),
                    0,
                    &mut table,
                )
            };
            assert_eq!(rc, AE_SUCCESS, "{extra}");
            let mut length = 0;
            unsafe { crate::AdsGetRecordLength(table, &mut length) };
            let armed = (enable(table, c"secret"), crate::AdsEncryptTable(table));
            assert_eq!(crate::AdsCloseTable(table), AE_SUCCESS);
            crate::AdsDisconnect(connect);
            (length, armed)
        };
        assert_eq!(length_of("", "plain"), (19, (AE_SUCCESS, AE_SUCCESS)));
        assert_eq!(
            length_of("EncryptionType=AES128;FIPS=FALSE;", "aes128"),
            (27, (AE_SUCCESS, AE_SUCCESS))
        );
        assert_eq!(
            length_of("EncryptionType=AES256;FIPS=FALSE;", "aes256"),
            (27, (AE_SUCCESS, AE_SUCCESS))
        );
    }

    fixtures::remote_twins!(
        an_aes_connection_lays_out_the_tables_it_creates,
        the_password_belongs_to_the_handle,
        a_dictionarys_encrypted_table_does_not_open_free,
        a_dictionarys_encrypted_table_is_read_through_the_dictionary,
        an_aes_dictionary_opens_with_its_dictionary_password,
        a_statement_opens_an_encrypted_table_with_the_password_it_is_given
    );
}
