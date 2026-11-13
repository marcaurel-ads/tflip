//! The `Parameter N` that `AdsGetLastError`'s text carries after a call was
//! given a NULL pointer or an option it refuses, and the order the original
//! engine checks its arguments in (`fixtures/errors/param_n.json`, from
//! `tools/ace/errors/param_n.c`).

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::Value as Json;

    use crate::fixtures;

    macro_rules! exports {
        ($($name:ident),* $(,)?) => {
            /// The address of the export `name`.
            fn export(name: &str) -> Option<usize> {
                match name {
                    $(stringify!($name) => Some(crate::$name as *const () as usize),)*
                    _ => None,
                }
            }
        };
    }

    exports!(
        AdsGetNumFields,
        AdsGetRecordLength,
        AdsGetFieldName,
        AdsGetFieldType,
        AdsGetFieldLength,
        AdsGetFieldDecimals,
        AdsGetFieldOffset,
        AdsGetTableFilename,
        AdsGetTableAlias,
        AdsGetTableOpenOptions,
        AdsGetTableType,
        AdsGetTableCharType,
        AdsGetTableLockType,
        AdsGetTableRights,
        AdsGetMemoBlockSize,
        AdsGetLastTableUpdate,
        AdsGetNumIndexes,
        AdsGetAllIndexes,
        AdsGetIndexHandle,
        AdsGetIndexHandleByOrder,
        AdsGetLastAutoinc,
        AdsGetRecordNum,
        AdsGetRecord,
        AdsGetRecordCRC,
        AdsAtBOF,
        AdsAtEOF,
        AdsIsFound,
        AdsIsRecordDeleted,
        AdsIsRecordEncrypted,
        AdsIsTableEncrypted,
        AdsIsTableLocked,
        AdsIsRecordLocked,
        AdsGetNumLocks,
        AdsGetAllLocks,
        AdsGetFilter,
        AdsGetAOF,
        AdsGetAOFOptLevel,
        AdsIsRecordInAOF,
        AdsGetTableCollation,
        AdsGetHandleType,
        AdsIsEmpty,
        AdsIsNull,
        AdsGetField,
        AdsGetString,
        AdsGetLogical,
        AdsGetLong,
        AdsGetShort,
        AdsGetDouble,
        AdsGetDate,
        AdsGetTime,
        AdsGetBinary,
        AdsGetBinaryLength,
        AdsGetMemoLength,
        AdsGetMemoDataType,
        AdsGetJulian,
        AdsGetMilliseconds,
        AdsGetMoney,
        AdsIsExprValid,
        AdsEvalLogicalExpr,
        AdsEvalStringExpr,
        AdsEvalNumericExpr,
        AdsGetIndexName,
        AdsGetIndexFilename,
        AdsGetIndexExpr,
        AdsGetIndexCondition,
        AdsGetIndexCollation,
        AdsGetKeyLength,
        AdsGetKeyType,
        AdsGetKeyNum,
        AdsGetKeyCount,
        AdsGetRelKeyPos,
        AdsExtractKey,
        AdsIsIndexCompound,
        AdsIsIndexDescending,
        AdsIsIndexCustom,
        AdsIsIndexUnique,
        AdsGetIndexPageSize,
        AdsGetCollation,
        AdsGetNumOpenTables,
        AdsGetNumActiveLinks,
        AdsGetConnectionType,
        AdsGetServerName,
        AdsGetServerTime,
        AdsGetConnectionPath,
        AdsGetDateFormat,
        AdsGetEpoch,
        AdsGetDecimals,
        AdsGetExact,
        AdsGetDeleted,
        AdsGetVersion,
        AdsGetSQLStatement,
        AdsGetNumParams,
        AdsSetString,
        AdsSetField,
        AdsSetBinary,
        AdsSetDate,
        AdsSetTime,
        AdsSetTimeStamp,
        AdsSetLogical,
        AdsSetLong,
        AdsSetShort,
        AdsSetDouble,
        AdsSetMoney,
        AdsSetTimeStampRaw,
        AdsSetFieldRaw,
        AdsClearSQLParams,
        AdsSetLongLong,
        AdsSetJulian,
        AdsSetMilliseconds,
        AdsSetEmpty,
        AdsSetNull,
        AdsSetRecord,
        AdsSetStringFromCodePage,
        AdsGetStringW,
        AdsGetFieldW,
        AdsSetStringW,
        AdsSetFieldW,
        AdsEvalLogicalExprW,
        AdsPrepareSQLW,
        AdsVerifySQLW,
        AdsBinaryToFileW,
        AdsFileToBinaryW,
        AdsCachePrepareSQLW,
        AdsExecuteSQLDirectW,
        AdsSetFilter,
        AdsSetFilter100,
        AdsSetAOF100,
        AdsSetScope,
        AdsSetRelation,
        AdsSetScopedRelation,
        AdsSetDateFormat,
        AdsSetDateFormat60,
        AdsSetCollation,
        AdsSetDefault,
        AdsSetSearchPath,
        AdsSetCollationLang,
        AdsSeek,
        AdsSetAOF,
        AdsEvalAOF,
        AdsCustomizeAOF,
        AdsPrepareSQL,
        AdsVerifySQL,
        AdsCachePrepareSQL,
        AdsExecuteSQLDirect,
        AdsCreateSQLStatement,
        AdsDDGetDatabaseProperty,
        AdsDDGetTableProperty,
        AdsDDGetFieldProperty,
        AdsDDGetIndexProperty,
        AdsDDGetIndexFileProperty,
        AdsDDGetUserProperty,
        AdsDDGetUserGroupProperty,
        AdsDDGetLinkProperty,
        AdsDDGetViewProperty,
        AdsDDGetTriggerProperty,
        AdsDDGetProcedureProperty,
        AdsDDGetRefIntegrityProperty,
        AdsDDGetPublicationProperty,
        AdsDDGetArticleProperty,
        AdsDDGetSubscriptionProperty,
        AdsDDGetPermissions,
        AdsDDFindFirstObject,
        AdsDDFindNextObject,
        AdsDDFindClose,
    );

    /// Calls the export at `address` with `args`, all as machine words, the
    /// way the probe does.
    fn call(address: usize, args: &[usize]) -> u32 {
        // SAFETY: each export takes as many words as the case gives it
        // (integers and pointers, which the C ABI passes alike), and
        // returns a `u32`; the pointers are the test's buffers or null.
        unsafe {
            match *args {
                [] => std::mem::transmute::<usize, unsafe extern "system" fn() -> u32>(address)(),
                [a] => std::mem::transmute::<usize, unsafe extern "system" fn(usize) -> u32>(
                    address,
                )(a),
                [a, b] => std::mem::transmute::<
                    usize,
                    unsafe extern "system" fn(usize, usize) -> u32,
                >(address)(a, b),
                [a, b, c] => std::mem::transmute::<
                    usize,
                    unsafe extern "system" fn(usize, usize, usize) -> u32,
                >(address)(a, b, c),
                [a, b, c, d] => std::mem::transmute::<
                    usize,
                    unsafe extern "system" fn(usize, usize, usize, usize) -> u32,
                >(address)(a, b, c, d),
                [a, b, c, d, e] => std::mem::transmute::<
                    usize,
                    unsafe extern "system" fn(usize, usize, usize, usize, usize) -> u32,
                >(address)(a, b, c, d, e),
                [a, b, c, d, e, f] => std::mem::transmute::<
                    usize,
                    unsafe extern "system" fn(usize, usize, usize, usize, usize, usize) -> u32,
                >(address)(a, b, c, d, e, f),
                [a, b, c, d, e, f, g] => std::mem::transmute::<
                    usize,
                    unsafe extern "system" fn(
                        usize,
                        usize,
                        usize,
                        usize,
                        usize,
                        usize,
                        usize,
                    ) -> u32,
                >(address)(a, b, c, d, e, f, g),
                _ => panic!("{} arguments", args.len()),
            }
        }
    }

    fn last_error() -> (u32, String) {
        let (mut code, mut buf, mut len) = (0, [0u8; 1200], 1200);
        unsafe { crate::AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
        let text = String::from_utf8_lossy(&buf[..usize::from(len)]).into_owned();
        (code, text.trim_end_matches('\0').to_owned())
    }

    /// Replays `fixtures/errors/param_n.json`.
    #[test]
    fn null_pointers_and_options_are_reported_as_the_original_does() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../");
        let rows: Vec<Json> = serde_json::from_slice(
            &std::fs::read(root.join("fixtures/errors/param_n.json")).unwrap(),
        )
        .unwrap();
        // The probe opens `t` read-only and `w` for writing, and `k` is a cursor.
        let _settings = fixtures::settings_lock();
        let (connect, writable) = fixtures::open("indexes", c"indexed.adt");
        let mut table = 0;
        assert_eq!(
            unsafe {
                crate::AdsOpenTable(
                    connect,
                    c"indexed.adt".as_ptr().cast_mut().cast(),
                    c"t".as_ptr().cast_mut().cast(),
                    crate::ADS_ADT,
                    1,
                    1,
                    1,
                    2,
                    &mut table,
                )
            },
            0
        );
        // `a` has a field of each type.
        let (_, alltypes) = fixtures::open("all_types", c"all_types.adt");
        // `x`, `y` and `v` are DBF tables of FoxPro, Clipper and VFP type with
        // a record to set.
        // They are made in a folder of their own, as the fixtures are not
        // written to.
        let empty = tflip_storage::testing::temp_root().join("param_n_empty");
        std::fs::create_dir_all(&empty).unwrap();
        let (folder, _) = fixtures::scratch_from("param_n-dbf", &empty);
        let dbf_connect = fixtures::connect_to(&folder);
        let mut dbf = [0usize; 3];
        for (handle, (name, table_type)) in dbf.iter_mut().zip([
            (c"xcdx.dbf", crate::ADS_CDX),
            (c"yntx.dbf", crate::ADS_NTX),
            (c"vvfp.dbf", crate::ADS_VFP),
        ]) {
            let mut fields = *b"n,Numeric,8,2;d,Date;c,Character,10;l,Logical\0";
            assert_eq!(
                unsafe {
                    crate::AdsCreateTable71(
                        dbf_connect,
                        name.as_ptr().cast_mut().cast(),
                        std::ptr::null_mut(),
                        table_type,
                        1,
                        crate::ADS_PROPRIETARY_LOCKING,
                        1,
                        0,
                        fields.as_mut_ptr(),
                        0,
                        handle,
                    )
                },
                0
            );
            crate::AdsAppendRecord(*handle);
            crate::AdsWriteRecord(*handle);
        }
        let dictionary_path = root
            .join("fixtures/errors/param_n/indexed_table.add")
            .canonicalize()
            .unwrap()
            .display()
            .to_string();
        let (rc, dictionary) = fixtures::connect_to_as(&dictionary_path, "", "");
        assert_eq!(rc, 0);
        let (rc, admin) = fixtures::connect_to_as(&dictionary_path, "ADSSYS", "");
        assert_eq!(rc, 0);
        let mut direct = 0;
        let mut cursor = 0;
        assert_eq!(
            unsafe { crate::AdsCreateSQLStatement(connect, &mut direct) },
            0
        );
        let select = c"SELECT * FROM indexed".as_ptr().cast_mut().cast();
        assert_eq!(
            unsafe { crate::AdsExecuteSQLDirect(direct, select, &mut cursor) },
            0
        );
        let mut order = 0;
        let name = c"BY_NAME".as_ptr().cast_mut().cast();
        assert_eq!(
            unsafe { crate::AdsGetIndexHandle(table, name, &mut order) },
            0
        );
        let mut statement = 0;
        assert_eq!(
            unsafe { crate::AdsCreateSQLStatement(connect, &mut statement) },
            0
        );
        let sql = c"SELECT * FROM indexed".as_ptr().cast_mut().cast();
        assert_eq!(unsafe { crate::AdsPrepareSQL(statement, sql) }, 0);
        let mut params = 0;
        assert_eq!(
            unsafe { crate::AdsCreateSQLStatement(connect, &mut params) },
            0
        );
        let sql = c"SELECT * FROM indexed WHERE name = :p"
            .as_ptr()
            .cast_mut()
            .cast();
        assert_eq!(unsafe { crate::AdsPrepareSQL(params, sql) }, 0);
        let mut differ = Vec::new();
        for row in rows {
            let line = row["call"].as_str().unwrap();
            let words: Vec<&str> = line.split(' ').collect();
            let raw_dump = row.get("rec").is_some();
            let null = usize::try_from(row["null"].as_u64().unwrap()).unwrap();
            let (mut buf, mut buf2) = ([0u8; 400], [0u8; 400]);
            let (mut l16, mut l16b, mut v16) = (200u16, 200u16, 0u16);
            let (mut l32, mut v32) = (200u32, 0u32);
            let mut v64 = [0u32; 2];
            let mut handles = [0usize; 8];
            let text = |word: &str| -> String {
                let text = ["str:", "pstr:", "pwstr:"]
                    .iter()
                    .find_map(|prefix| word.strip_prefix(prefix));
                text.unwrap_or("").replace('~', " ")
            };
            let strings: Vec<std::ffi::CString> = words
                .iter()
                .map(|word| std::ffi::CString::new(text(word)).unwrap())
                .collect();
            let mut wide: Vec<Vec<u16>> = words
                .iter()
                .map(|word| text(word).encode_utf16().chain([0]).collect())
                .collect();
            let mut args = Vec::new();
            for (i, word) in words.iter().enumerate().skip(1) {
                let value = match *word {
                    "t" => table,
                    "w" => writable,
                    "a" => alltypes,
                    "x" => dbf[0],
                    "y" => dbf[1],
                    "v" => dbf[2],
                    "k" => cursor,
                    "d" => dictionary,
                    "da" => admin,
                    "n" => {
                        let mut fresh = 0;
                        unsafe { crate::AdsCreateSQLStatement(connect, &mut fresh) };
                        fresh
                    }
                    "i" => order,
                    "c" => connect,
                    "s" => statement,
                    "q" => params,
                    "pbuf" => buf.as_mut_ptr() as usize,
                    "pbuf2" => buf2.as_mut_ptr() as usize,
                    // The buffer with bytes at its start.
                    word if word.starts_with("pbuft:") => {
                        let text = word["pbuft:".len()..].replace('~', " ");
                        buf[..text.len()].copy_from_slice(text.as_bytes());
                        buf.as_mut_ptr() as usize
                    }
                    word if word.starts_with("pbufh:") => {
                        let hex = &word["pbufh:".len()..];
                        for (at, pair) in hex.as_bytes().chunks(2).enumerate() {
                            let pair = std::str::from_utf8(pair).unwrap();
                            buf[at] = u8::from_str_radix(pair, 16).unwrap();
                        }
                        buf.as_mut_ptr() as usize
                    }
                    "pl16" => &mut l16 as *mut u16 as usize,
                    "pl16b" => &mut l16b as *mut u16 as usize,
                    "pl32" => &mut l32 as *mut u32 as usize,
                    "p16" => &mut v16 as *mut u16 as usize,
                    "p32" => &mut v32 as *mut u32 as usize,
                    "p64" => v64.as_mut_ptr() as usize,
                    "ph" => handles.as_mut_ptr() as usize,
                    word if word.starts_with("pwstr:") => wide[i].as_mut_ptr() as usize,
                    word if word.starts_with("str:") || word.starts_with("pstr:") => {
                        strings[i].as_ptr() as usize
                    }
                    // The double of `AdsSetDouble`, which is passed apart.
                    number if number.contains('.') => 0,
                    number => number.parse().unwrap(),
                };
                args.push(if i == null { 0 } else { value });
            }
            crate::AdsGotoTop(table);
            crate::AdsGotoTop(writable);
            crate::AdsGotoTop(alltypes);
            for handle in dbf {
                crate::AdsGotoTop(handle);
            }
            crate::AdsGotoTop(cursor);
            let mut field_type = 0u16;
            let nosuch = c"nosuch".as_ptr().cast_mut().cast();
            unsafe { crate::AdsGetFieldType(table, nosuch, &mut field_type) };
            let address = export(words[0]).unwrap_or_else(|| panic!("{}", words[0]));
            let rc = if words[0] == "AdsSetDouble" {
                // A double goes in a floating point register, not with the
                // integer words `call` passes.
                let value: f64 = words[3].parse().unwrap();
                // SAFETY: `AdsSetDouble` takes two handles or pointers and a
                // double, and returns a `u32`.
                unsafe {
                    std::mem::transmute::<usize, unsafe extern "system" fn(usize, usize, f64) -> u32>(
                        address,
                    )(args[0], args[1], value)
                }
            } else {
                call(address, &args)
            };
            let (code, text) = last_error();
            // The record the call left, of the rows that record it.
            let record = raw_dump.then(|| {
                let (mut record, mut length) = ([0u8; 1024], 1024u32);
                let rc = unsafe { crate::AdsGetRecord(args[0], record.as_mut_ptr(), &mut length) };
                let hex: String = record[..length as usize]
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                (u64::from(rc), hex)
            });
            // A setter's change is not kept for the next call.
            crate::AdsCancelUpdate(writable);
            crate::AdsCancelUpdate(alltypes);
            for handle in dbf {
                crate::AdsCancelUpdate(handle);
            }
            let want = (
                row["rc"].as_u64().unwrap(),
                row["last"].as_u64().unwrap(),
                row["text"].as_str().unwrap(),
            );
            // The out-buffers of the rows that record them.
            let out = |len: u64, v32: u64, buf: &[u8]| {
                let hex: String = buf[..32].iter().map(|b| format!("{b:02x}")).collect();
                (len, v32, hex)
            };
            let wanted_out = row.get("len").map(|len| {
                (
                    len.as_u64().unwrap(),
                    row["v32"].as_u64().unwrap(),
                    row["buf"].as_str().unwrap().to_owned(),
                )
            });
            let got_out = out(u64::from(l16), u64::from(v32), &buf);
            let wanted_record = row.get("rec").map(|hex| {
                (
                    row["rec_rc"].as_u64().unwrap(),
                    hex.as_str().unwrap().to_owned(),
                )
            });
            if (u64::from(rc), u64::from(code), text.as_str()) != want
                || wanted_out.as_ref().is_some_and(|wanted| *wanted != got_out)
                || record != wanted_record
            {
                differ.push(format!(
                    "{line} (null {null}):\n    got  {rc} {code} {text:?} {got_out:?} {record:?}\n    want {} {} {:?} {wanted_out:?} {wanted_record:?}",
                    want.0, want.1, want.2
                ));
            }
        }
        crate::AdsDisconnect(dbf_connect);
        crate::AdsDisconnect(admin);
        crate::AdsDisconnect(dictionary);
        crate::AdsDisconnect(connect);
        assert!(
            differ.is_empty(),
            "{} differ:\n{}",
            differ.len(),
            differ.join("\n")
        );
    }
}
