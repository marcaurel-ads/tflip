//! What a dictionary connection gets from a free table with and without the
//! connection option `ADS_CHECK_FREE_TABLE_ACCESS`, from the rights
//! properties of a user, from `AdsCreateIndex` by a user without `ALTER`, and
//! from a dictionary table whose files are missing
//! (`fixtures/dictionary/freetables.json`, from `tools/ace/dd/freetables.c`
//! and `freetables.txt`).

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::ffi::{CString, c_void};
    use std::path::Path;

    use serde_json::Value as Json;

    use crate::fixtures;
    use crate::{
        AdsCloseSQLStatement, AdsCloseTable, AdsConnect60, AdsConnect101, AdsCreateIndex,
        AdsCreateSQLStatement, AdsDDGetUserGroupProperty, AdsDDGetUserProperty,
        AdsDDSetUserGroupProperty, AdsDDSetUserProperty, AdsDisconnect, AdsExecuteSQLDirect,
        AdsGetLastError, AdsGetRecordCount, AdsHandle, AdsOpenTable,
    };

    fn c(text: &str) -> CString {
        CString::new(text).unwrap()
    }

    fn pointer(text: &CString) -> *mut u8 {
        text.as_ptr().cast_mut().cast()
    }

    /// A number as the steps write it: decimal, or hexadecimal with `0x`.
    fn number(text: &str) -> u32 {
        match text.strip_prefix("0x") {
            Some(hex) => u32::from_str_radix(hex, 16).unwrap(),
            None => text.parse().unwrap(),
        }
    }

    /// The error the last call left, as the probe records it.
    fn last_error() -> (u32, String) {
        let (mut code, mut buf, mut len) = (0, [0u8; 1200], 1200);
        unsafe { AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
        let text = String::from_utf8_lossy(&buf[..usize::from(len)]).into_owned();
        (code, text.trim_end_matches('\0').to_owned())
    }

    type Error = Option<(u32, String)>;

    /// Compares one step's outcome with the probe's.
    fn check(
        failures: &mut Vec<String>,
        step: &Json,
        rc: u32,
        error: Error,
        rows: Option<u64>,
        folder: &str,
    ) {
        let line = step["op"].as_str().unwrap();
        let want = step["rc"].as_u64().unwrap() as u32;
        if rc != want {
            failures.push(format!("{line}: rc {rc}, want {step}"));
            return;
        }
        if let Some(rows) = rows
            && step["rows"].as_u64() != Some(rows)
        {
            failures.push(format!("{line}: {rows} rows, want {step}"));
        }
        let Some((code, text)) = error else { return };
        // The probe names its folder `<folder>` and writes Windows paths.
        let text = text.replace(folder, "<folder>").replace('/', "\\");
        if u64::from(code) != step["code"].as_u64().unwrap()
            || text != step["text"].as_str().unwrap()
        {
            failures.push(format!("{line}: error {code} {text:?}, want {step}"));
        }
    }

    /// The probe's steps, replayed on a folder of its own with the fixture
    /// dictionary and the free tables, comparing each return code and error.
    #[test]
    fn free_table_access_and_missing_files_are_as_the_original_does() {
        let _settings = fixtures::settings_shared();
        let steps: Vec<Json> = serde_json::from_slice(
            &std::fs::read(fixtures::dir("../dictionary/freetables.json")).unwrap(),
        )
        .unwrap();
        let folder = fixtures::scratch("freetables", "dictionary_props");
        let folder = Path::new(&folder);
        // The probe's free tables, and the dictionary that does not verify
        // access rights, beside the dictionary.
        for (from, names) in [
            ("indexes", &["indexed.adt", "indexed.adi"][..]),
            ("cdx/dbf_types", &["dbf_types.dbf", "dbf_types.fpt"][..]),
            (
                "dictionary",
                &[
                    "dict.add",
                    "dict.ai",
                    "dict.am",
                    "customers.adt",
                    "customers.adi",
                    "customers.adm",
                    "orders.adt",
                    "orders.adi",
                    "orders_extra.adi",
                ][..],
            ),
        ] {
            for name in names {
                std::fs::copy(fixtures::dir(from).join(name), folder.join(name)).unwrap();
            }
        }
        let folder_text = folder.display().to_string();
        let mut handles: HashMap<String, AdsHandle> = HashMap::new();
        let mut connections = Vec::new();
        let mut failures = Vec::new();
        for step in &steps {
            let line = step["op"].as_str().unwrap();
            let (op, rest) = line.split_once(' ').unwrap();
            let words: Vec<&str> = rest.split(' ').collect();
            let mut rows = None;
            let mut error_read = None;
            let rc = match op {
                "conn" => {
                    let [name, kind, user, pw, options] = words[..] else {
                        panic!("{line}")
                    };
                    let path = match kind.strip_prefix("dict") {
                        Some("") => format!("{folder_text}/props.add"),
                        Some(file) => format!("{folder_text}/{}", &file[1..]),
                        None => folder_text.clone(),
                    };
                    let (path, user_c, pw_c) = (c(&path), c(user), c(pw));
                    let mut h = 0;
                    let rc = unsafe {
                        AdsConnect60(
                            pointer(&path),
                            crate::ADS_LOCAL_SERVER,
                            if user == "-" {
                                std::ptr::null_mut()
                            } else {
                                pointer(&user_c)
                            },
                            if pw == "-" {
                                std::ptr::null_mut()
                            } else {
                                pointer(&pw_c)
                            },
                            number(options),
                            &mut h,
                        )
                    };
                    handles.insert(name.to_owned(), h);
                    connections.push(h);
                    rc
                }
                "conn101" => {
                    let (name, string) = rest.split_once(' ').unwrap();
                    let string = string
                        .replace("%D", &format!("{folder_text}/props.add"))
                        .replace("%F", &folder_text);
                    let (string, mut options, mut h) = (c(&string), 0, 0);
                    let rc = unsafe { AdsConnect101(pointer(&string), &mut options, &mut h) };
                    handles.insert(name.to_owned(), h);
                    connections.push(h);
                    rc
                }
                "disc" => AdsDisconnect(handles[words[0]]),
                "open" => {
                    let [name, table, kind, options] = words[..] else {
                        panic!("{line}")
                    };
                    let kind: u16 = kind.parse().unwrap();
                    let full = match kind {
                        0 => table.to_owned(),
                        _ => format!("{folder_text}/{table}"),
                    };
                    let (full, mut t) = (c(&full), 0);
                    let rc = unsafe {
                        AdsOpenTable(
                            handles[name],
                            pointer(&full),
                            std::ptr::null_mut(),
                            kind,
                            1,
                            0,
                            1,
                            number(options),
                            &mut t,
                        )
                    };
                    handles.insert(table.to_owned(), t);
                    rc
                }
                "close" => AdsCloseTable(handles[words[0]]),
                "sql" | "rows" => {
                    let (name, sql) = rest.split_once(' ').unwrap();
                    let (sql, mut statement, mut cursor) = (c(sql), 0, 0);
                    unsafe { AdsCreateSQLStatement(handles[name], &mut statement) };
                    let rc = unsafe { AdsExecuteSQLDirect(statement, pointer(&sql), &mut cursor) };
                    if op == "rows" {
                        let mut n = 0;
                        if rc == 0 && cursor != 0 {
                            unsafe { AdsGetRecordCount(cursor, 1, &mut n) };
                        }
                        rows = Some(u64::from(n));
                    }
                    // Read before the statement's closing replaces it.
                    error_read = Some((rc != 0).then(last_error));
                    if cursor != 0 {
                        AdsCloseTable(cursor);
                    }
                    AdsCloseSQLStatement(statement);
                    rc
                }
                "setuser" | "setgroup" => {
                    let (name, who, id) = (words[0], c(words[1]), words[2].parse().unwrap());
                    let mut v = number(words[3]);
                    let len: u16 = words.get(4).map_or(4, |len| len.parse().unwrap());
                    let property = (&raw mut v).cast::<c_void>();
                    unsafe {
                        match op {
                            "setuser" => AdsDDSetUserProperty(
                                handles[name],
                                pointer(&who),
                                id,
                                property,
                                len,
                            ),
                            _ => AdsDDSetUserGroupProperty(
                                handles[name],
                                pointer(&who),
                                id,
                                property,
                                len,
                            ),
                        }
                    }
                }
                "getuser" | "getgroup" => {
                    let (name, who, id) = (words[0], c(words[1]), words[2].parse().unwrap());
                    let (mut v, mut len) = (0u32, 4u16);
                    let property = (&raw mut v).cast::<c_void>();
                    let rc = unsafe {
                        match op {
                            "getuser" => AdsDDGetUserProperty(
                                handles[name],
                                pointer(&who),
                                id,
                                property,
                                &mut len,
                            ),
                            _ => AdsDDGetUserGroupProperty(
                                handles[name],
                                pointer(&who),
                                id,
                                property,
                                &mut len,
                            ),
                        }
                    };
                    error_read = Some((rc != 0).then(last_error));
                    if step["value"].as_u64() != Some(u64::from(v))
                        || step["len"].as_u64() != Some(u64::from(len))
                    {
                        failures.push(format!("{line}: value {v} len {len}, want {step}"));
                    }
                    rc
                }
                "index" => {
                    let [table, file, tag, expr, options] = words[..] else {
                        panic!("{line}")
                    };
                    let (file_c, tag, expr) = (c(file), c(tag), c(expr));
                    let mut ix = 0;
                    unsafe {
                        AdsCreateIndex(
                            handles[table],
                            if file == "-" {
                                std::ptr::null_mut()
                            } else {
                                pointer(&file_c)
                            },
                            pointer(&tag),
                            pointer(&expr),
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                            number(options),
                            &mut ix,
                        )
                    }
                }
                "copy" => {
                    // The files the original made are the ones tflip makes.
                    let made = std::fs::read(folder.join(words[0])).unwrap_or_default();
                    let golden = fixtures::dir("../dictionary/freetables").join(words[1]);
                    if made != std::fs::read(golden).unwrap() {
                        let _ = std::fs::write(folder.join(format!("made_{}", words[1])), &made);
                        failures.push(format!("{line}: the file differs from the original's"));
                    }
                    continue;
                }
                "exists" | "remove" => {
                    let path = folder.join(words[0]);
                    if op == "remove" {
                        let _ = std::fs::remove_file(&path);
                    }
                    let exists = u64::from(path.exists());
                    if step["exists"].as_u64() != Some(exists) {
                        failures.push(format!("{line}: exists {exists}, want {step}"));
                    }
                    continue;
                }
                _ => panic!("{line}"),
            };
            let error = error_read.unwrap_or_else(|| (rc != 0).then(last_error));
            check(&mut failures, step, rc, error, rows, &folder_text);
        }
        for handle in connections {
            // What the steps left open; one disconnected already is refused.
            AdsDisconnect(handle);
        }
        assert!(
            failures.is_empty(),
            "{} differ:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    /// `CheckFreeTableAccess`, as the connect option 0x2000 and as the
    /// connect string key, on the local server and through `tflipd` (whose
    /// client sends it with the connection): a user that is neither `ADSSYS`
    /// nor of `DB:Admin` is refused a free table (7087), to open and in SQL;
    /// a connection without it, and an administrator, are not.
    #[test]
    fn free_table_access_is_checked_on_either_server() {
        let _settings = fixtures::settings_shared();
        let (source, dir) =
            fixtures::scratch_from("freetables_remote", &fixtures::dir("dictionary_props"));
        for name in ["indexed.adt", "indexed.adi"] {
            std::fs::copy(fixtures::dir("indexes").join(name), dir.join(name)).unwrap();
        }
        let (separator, types) = match fixtures::is_remote() {
            true => ('\\', crate::ADS_REMOTE_SERVER),
            false => ('/', crate::ADS_LOCAL_SERVER),
        };
        let add = format!("{source}{separator}props.add");
        let connect60 = |user: &str, password: &str, options: u32| {
            let (add, user, password) = (c(&add), c(user), c(password));
            let mut handle = 0;
            let rc = unsafe {
                AdsConnect60(
                    pointer(&add),
                    types,
                    pointer(&user),
                    pointer(&password),
                    options,
                    &mut handle,
                )
            };
            assert_eq!(rc, 0);
            handle
        };
        let server = match types {
            crate::ADS_REMOTE_SERVER => "REMOTE",
            _ => "LOCAL",
        };
        let connect101 = |user: &str, password: &str, check: &str| {
            let string = c(&format!(
                "Data Source={add};ServerType={server};User ID={user};Password={password};\
                 CheckFreeTableAccess={check}"
            ));
            let (mut options, mut handle) = (0, 0);
            let rc = unsafe { AdsConnect101(pointer(&string), &mut options, &mut handle) };
            assert_eq!(rc, 0);
            handle
        };
        let open = |connect: AdsHandle| {
            let name = c("indexed.adt");
            let mut table = 0;
            let rc = unsafe {
                AdsOpenTable(
                    connect,
                    pointer(&name),
                    std::ptr::null_mut(),
                    3,
                    1,
                    0,
                    1,
                    0,
                    &mut table,
                )
            };
            if rc == 0 {
                AdsCloseTable(table);
            }
            (rc, last_error().0)
        };
        let select = |connect: AdsHandle| {
            let (sql, mut statement, mut cursor) = (c("SELECT * FROM indexed"), 0, 0);
            unsafe { AdsCreateSQLStatement(connect, &mut statement) };
            let rc = unsafe { AdsExecuteSQLDirect(statement, pointer(&sql), &mut cursor) };
            let error = last_error().0;
            if cursor != 0 {
                AdsCloseTable(cursor);
            }
            AdsCloseSQLStatement(statement);
            (rc, error)
        };
        let refused = (7087, 7087);
        let sql_refused = (7200, 7200);
        let connections = [
            (connect60("bob", "bobpw", 0), false),
            (connect60("bob", "bobpw", 0x2000), true),
            (connect60("ADSSYS", "adssyspw", 0x2000), false),
            (connect101("bob", "bobpw", "TRUE"), true),
            (connect101("bob", "bobpw", "FALSE"), false),
            (connect101("ADSSYS", "adssyspw", "TRUE"), false),
        ];
        for (connect, checked) in connections {
            assert_eq!(open(connect), if checked { refused } else { (0, 0) });
            assert_eq!(select(connect), if checked { sql_refused } else { (0, 0) });
            AdsDisconnect(connect);
        }
    }

    crate::fixtures::remote_twins!(free_table_access_is_checked_on_either_server);
}
