//! What handles do on AES tables, replayed from the scripts of
//! `fixtures/aes/api/`: the Rust twin of `tools/ace/crypto/aesapi.c`, which
//! recorded what the original engine answers (`<script>.json`). Every
//! command must give the keys, return codes and values the original did.

use std::collections::HashMap;

use serde_json::{Map, Value as Json};

use crate::connection::*;
use crate::fixtures;
use crate::*;

fn word<'a>(rest: &mut &'a str) -> &'a str {
    let (word, after) = rest.split_once(' ').unwrap_or((rest, ""));
    *rest = after;
    word
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn c_string(text: &str) -> Vec<u8> {
    let mut bytes = tflip_storage::cp1252::encode(text);
    bytes.push(0);
    bytes
}

/// `AdsGetLastError`'s code and text.
fn last_error() -> (Unsigned32, String) {
    let (mut code, mut buf, mut len) = (0, [0u8; 1100], 1100u16);
    unsafe { AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
    (
        code,
        buf[..usize::from(len)]
            .iter()
            .map(|&b| char::from(b))
            .collect(),
    )
}

struct Replay {
    /// What this step recorded, in the probe's keys.
    out: Map<String, Json>,
    step: usize,
    objects: HashMap<String, (AdsHandle, AdsHandle)>,
}

impl Replay {
    fn put(&mut self, what: &str, value: impl Into<Json>) {
        let key = format!("{:03}_{what}", self.step);
        self.out.insert(key, value.into());
    }

    fn result(&mut self, rc: Unsigned32) {
        self.put("rc", rc);
        if rc != 0 {
            let (code, text) = last_error();
            self.put("err", code);
            self.put("text", text);
        }
    }

    fn handle(&self, name: &str) -> AdsHandle {
        self.objects.get(name).map_or(0, |object| object.0)
    }

    fn table(&self, name: &str) -> AdsHandle {
        self.objects.get(name).map_or(0, |object| object.1)
    }

    fn dump(&mut self, t: AdsHandle) {
        let mut count = 0;
        unsafe { AdsGetNumFields(t, &mut count) };
        let mut flag = 0x55;
        unsafe { AdsIsEncryptionEnabled(t, &mut flag) };
        self.put("enabled", flag);
        flag = 0x55;
        unsafe { AdsAtEOF(t, &mut flag) };
        self.put("eof", flag);
        let mut record = 0;
        unsafe { AdsGetRecordNum(t, 1, &mut record) };
        self.put("recno", record);
        flag = 0x55;
        unsafe { AdsIsRecordDeleted(t, &mut flag) };
        self.put("deleted", flag);
        for f in 1..=count {
            let (mut name, mut name_len, mut kind) = ([0u8; 128], 128u16, 0u16);
            unsafe {
                AdsGetFieldName(t, f, name.as_mut_ptr(), &mut name_len);
                name[usize::from(name_len)] = 0;
                AdsGetFieldType(t, name.as_mut_ptr(), &mut kind);
            }
            let field = String::from_utf8_lossy(&name[..usize::from(name_len)]).into_owned();
            let mut buf = vec![0u8; 4096];
            let mut len = 4096;
            let rc = unsafe { AdsGetField(t, name.as_mut_ptr(), buf.as_mut_ptr(), &mut len, 0) };
            self.put(&format!("{field}_type"), kind);
            self.put(&format!("{field}_rc"), rc);
            if rc == 0 {
                self.put(&field, hex(&buf[..len as usize]));
            } else {
                self.put(&format!("{field}_err"), last_error().0);
            }
            flag = 0x55;
            unsafe { AdsIsEmpty(t, name.as_mut_ptr(), &mut flag) };
            self.put(&format!("{field}_empty"), flag);
            let mut length = 0xdead;
            let rc = unsafe { AdsGetFieldLength(t, name.as_mut_ptr(), &mut length) };
            self.put(
                &format!("{field}_len"),
                if rc != 0 { -1 } else { i64::from(length) },
            );
            length = 0xdead;
            let rc = unsafe { AdsGetFieldOffset(t, name.as_mut_ptr(), &mut length) };
            self.put(
                &format!("{field}_off"),
                if rc != 0 { -1 } else { i64::from(length) },
            );
            if [5, 6, 7, 28].contains(&kind) {
                length = 0xdead;
                let rc = unsafe { AdsGetMemoLength(t, name.as_mut_ptr(), &mut length) };
                self.put(
                    &format!("{field}_memolen"),
                    if rc != 0 { -1 } else { i64::from(length) },
                );
                let mut big = vec![0u8; 4096];
                length = 4096;
                let rc =
                    unsafe { AdsGetString(t, name.as_mut_ptr(), big.as_mut_ptr(), &mut length, 1) };
                self.put(&format!("{field}_string_rc"), rc);
                if rc == 0 {
                    self.put(&format!("{field}_string"), hex(&big[..length as usize]));
                }
            }
        }
        let mut buf = vec![0u8; 4096];
        let mut len = 4096;
        let rc = unsafe { AdsGetRecord(t, buf.as_mut_ptr(), &mut len) };
        self.put("getrecord_rc", rc);
        if rc == 0 {
            self.put("record", hex(&buf[..len as usize]));
        }
    }

    fn rows(&mut self, cursor: AdsHandle) {
        let mut kind = 0;
        unsafe { AdsGetTableType(cursor, &mut kind) };
        self.put("tabletype", kind);
        let mut fields = 0;
        unsafe { AdsGetNumFields(cursor, &mut fields) };
        let mut rows = String::new();
        unsafe { AdsGotoTop(cursor) };
        loop {
            let mut eof = 0;
            unsafe { AdsAtEOF(cursor, &mut eof) };
            if eof != 0 {
                break;
            }
            for f in 1..=fields {
                let (mut name, mut name_len) = ([0u8; 128], 128u16);
                let (mut value, mut len) = ([0u8; 512], 511);
                unsafe { AdsGetFieldName(cursor, f, name.as_mut_ptr(), &mut name_len) };
                name[usize::from(name_len)] = 0;
                let rc = unsafe {
                    AdsGetString(cursor, name.as_mut_ptr(), value.as_mut_ptr(), &mut len, 1)
                };
                let text =
                    String::from_utf8_lossy(&value[..if rc != 0 { 0 } else { len as usize }])
                        .into_owned();
                let name = String::from_utf8_lossy(&name[..usize::from(name_len)]).into_owned();
                rows += &format!("{name}={text};");
            }
            rows.push('|');
            AdsSkip(cursor, 1);
        }
        self.put("rows", rows);
        let mut flag = 0x55;
        unsafe { AdsIsTableEncrypted(cursor, &mut flag) };
        self.put("encrypted", flag);
        let mut length = 0;
        unsafe { AdsGetRecordLength(cursor, &mut length) };
        self.put("length", length);
    }

    fn step(&mut self, line: &str, folder: &str) {
        let line = line.replace("%D", folder);
        let line = if fixtures::is_remote() {
            line
        } else {
            line.replace('\\', "/")
        };
        let mut rest = line.as_str();
        let cmd = word(&mut rest);
        let name = word(&mut rest);
        let (h, t) = (self.handle(name), self.table(name));
        let null = std::ptr::null_mut();
        match cmd {
            "connect" => {
                let mut connect = c_string(rest);
                let (mut options, mut handle) = (0, 0);
                let rc = unsafe { AdsConnect101(connect.as_mut_ptr(), &mut options, &mut handle) };
                self.result(rc);
                self.objects.insert(name.to_owned(), (handle, 0));
            }
            "disconnect" => {
                let rc = AdsDisconnect(h);
                self.result(rc);
            }
            "create" => {
                let (conn, file) = (word(&mut rest), word(&mut rest));
                let (mut file, mut fields) = (c_string(file), c_string(rest));
                let mut table = 0;
                let rc = unsafe {
                    AdsCreateTable71(
                        self.handle(conn),
                        file.as_mut_ptr(),
                        null,
                        3,
                        1,
                        1,
                        1,
                        0,
                        fields.as_mut_ptr(),
                        0,
                        &mut table,
                    )
                };
                self.result(rc);
                self.objects.insert(name.to_owned(), (table, table));
            }
            "open" | "openn" => {
                let (conn, file) = (word(&mut rest), word(&mut rest));
                let options: u32 = rest.trim().parse().unwrap_or(0);
                let mut file = c_string(file);
                let mut table = 0;
                let rc = unsafe {
                    AdsOpenTable(
                        self.handle(conn),
                        file.as_mut_ptr(),
                        null,
                        if cmd == "open" { 3 } else { 0 },
                        1,
                        1,
                        1,
                        options,
                        &mut table,
                    )
                };
                self.result(rc);
                self.objects.insert(name.to_owned(), (table, table));
            }
            "close" => {
                let rc = AdsCloseTable(h);
                self.result(rc);
            }
            "enable" => {
                let mut password = c_string(rest);
                let rc = unsafe { AdsEnableEncryption(h, password.as_mut_ptr()) };
                self.result(rc);
            }
            "encrypt" => {
                let rc = AdsEncryptTable(h);
                self.result(rc);
            }
            "decrypt" => {
                let rc = AdsDecryptTable(h);
                self.result(rc);
            }
            "index" => {
                let (table, file, tag) = (word(&mut rest), word(&mut rest), word(&mut rest));
                let options: u32 = word(&mut rest).parse().unwrap();
                let (mut file, mut tag, mut expr) = (c_string(file), c_string(tag), c_string(rest));
                let mut order = 0;
                let rc = unsafe {
                    AdsCreateIndex(
                        self.handle(table),
                        file.as_mut_ptr(),
                        tag.as_mut_ptr(),
                        expr.as_mut_ptr(),
                        null,
                        null,
                        options,
                        &mut order,
                    )
                };
                self.result(rc);
                let table = self.handle(table);
                self.objects.insert(name.to_owned(), (order, table));
            }
            "order" => {
                let table = word(&mut rest);
                let mut tag = c_string(rest);
                let mut order = 0;
                let rc =
                    unsafe { AdsGetIndexHandle(self.handle(table), tag.as_mut_ptr(), &mut order) };
                self.result(rc);
                let table = self.handle(table);
                self.objects.insert(name.to_owned(), (order, table));
            }
            "sql" => {
                let (mut statement, mut cursor) = (0, 0);
                unsafe { AdsCreateSQLStatement(h, &mut statement) };
                let mut sql = c_string(rest);
                let rc = unsafe { AdsExecuteSQLDirect(statement, sql.as_mut_ptr(), &mut cursor) };
                self.result(rc);
                if rc == 0 && cursor != 0 {
                    self.rows(cursor);
                    AdsCloseTable(cursor);
                }
                AdsCloseSQLStatement(statement);
            }
            "sqlc" => {
                let (mut statement, mut cursor) = (0, 0);
                unsafe { AdsCreateSQLStatement(h, &mut statement) };
                let mut sql = c_string(rest);
                let rc = unsafe { AdsExecuteSQLDirect(statement, sql.as_mut_ptr(), &mut cursor) };
                self.result(rc);
                if rc == 0 && cursor != 0 {
                    let mut count = 0;
                    unsafe { AdsGetRecordCount(cursor, 2, &mut count) };
                    self.put("count", count);
                    AdsCloseTable(cursor);
                }
                AdsCloseSQLStatement(statement);
            }
            "append" | "write" | "cancel" | "delete" | "recall" | "top" | "bottom" | "pack"
            | "zap" | "reindex" | "dropindex" => {
                let call: extern "system" fn(AdsHandle) -> Unsigned32 = match cmd {
                    "append" => AdsAppendRecord,
                    "write" => AdsWriteRecord,
                    "cancel" => AdsCancelUpdate,
                    "delete" => AdsDeleteRecord,
                    "recall" => AdsRecallRecord,
                    "top" => AdsGotoTop,
                    "bottom" => AdsGotoBottom,
                    "pack" => AdsPackTable,
                    "zap" => AdsZapTable,
                    "reindex" => AdsReindex,
                    _ => AdsDeleteIndex,
                };
                let rc = call(h);
                self.result(rc);
            }
            "goto" => {
                let rc = AdsGotoRecord(t, rest.trim().parse().unwrap());
                self.result(rc);
            }
            "skip" => {
                let rc = AdsSkip(h, rest.trim().parse().unwrap());
                self.result(rc);
            }
            "set" => {
                let field = word(&mut rest);
                let (mut field, mut value) = (c_string(field), c_string(rest));
                let rc = unsafe {
                    AdsSetField(t, field.as_mut_ptr(), value.as_mut_ptr(), rest.len() as u32)
                };
                self.result(rc);
            }
            "setempty" => {
                let mut field = c_string(rest);
                let rc = unsafe { AdsSetEmpty(t, field.as_mut_ptr()) };
                self.result(rc);
            }
            "seek" | "seekd" => {
                let key = word(&mut rest);
                let soft: u16 = rest.trim().parse().unwrap();
                let (mut bytes, data_type) = match cmd {
                    "seek" => (tflip_storage::cp1252::encode(key), 2),
                    _ => (key.parse::<f64>().unwrap().to_le_bytes().to_vec(), 4),
                };
                let mut found = 0x55;
                let len = bytes.len() as u16;
                let rc = unsafe {
                    AdsSeek(
                        h,
                        bytes.as_mut_ptr(),
                        len,
                        data_type,
                        if soft != 0 { 1 } else { 2 },
                        &mut found,
                    )
                };
                self.result(rc);
                self.put("found", found);
                let mut record = 0;
                unsafe { AdsGetRecordNum(t, 1, &mut record) };
                self.put("recno", record);
            }
            "keycount" | "keynum" => {
                let mut n = 0;
                let rc = unsafe {
                    match cmd {
                        "keycount" => AdsGetKeyCount(h, 1, &mut n),
                        _ => AdsGetKeyNum(h, 1, &mut n),
                    }
                };
                self.result(rc);
                self.put(if cmd == "keycount" { "count" } else { "num" }, n);
            }
            "keylen" => {
                let mut n = 0;
                let rc = unsafe { AdsGetKeyLength(h, &mut n) };
                self.result(rc);
                self.put("len", n);
            }
            "extract" => {
                let (mut buf, mut len) = ([0u8; 512], 512u16);
                let rc = unsafe { AdsExtractKey(h, buf.as_mut_ptr(), &mut len) };
                self.result(rc);
                if rc == 0 {
                    self.put("key", hex(&buf[..usize::from(len)]));
                }
            }
            "recno" => {
                let mut n = 0;
                let rc = unsafe { AdsGetRecordNum(t, 1, &mut n) };
                self.result(rc);
                self.put("recno", n);
                let mut eof = 0x55;
                unsafe { AdsAtEOF(t, &mut eof) };
                self.put("eof", eof);
            }
            "count" => {
                let mut n = 0;
                let rc = unsafe { AdsGetRecordCount(t, 2, &mut n) };
                self.result(rc);
                self.put("count", n);
            }
            "dump" => self.dump(t),
            "reclen" => {
                let mut length = 0;
                let rc = unsafe { AdsGetRecordLength(t, &mut length) };
                self.result(rc);
                self.put("length", length);
            }
            "opts" => {
                let mut options = 0;
                let rc = unsafe { AdsGetTableOpenOptions(t, &mut options) };
                self.result(rc);
                self.put("options", options);
            }
            "tablecheck" => {
                let mut flag = 0x55;
                unsafe { AdsIsTableEncrypted(t, &mut flag) };
                self.put("encrypted", flag);
                flag = 0x55;
                unsafe { AdsIsEncryptionEnabled(t, &mut flag) };
                self.put("enabled", flag);
            }
            other => panic!("unknown command {other}"),
        }
    }
}

/// Replays `script` of `fixtures/aes/api/` in a folder of its own and
/// compares every step with what the original recorded.
fn replay(script: &str) {
    let dir = fixtures::dir("../aes/api");
    let text = std::fs::read_to_string(dir.join(format!("{script}.txt"))).unwrap();
    let golden: Map<String, Json> = std::fs::read_to_string(dir.join(format!("{script}.json")))
        .map(|text| serde_json::from_str(&text).unwrap())
        .unwrap_or_default();
    let (source, _) = fixtures::scratch_from(&format!("aes_api_{script}"), &fixtures::empty_dir());
    let mut replay = Replay {
        out: Map::new(),
        step: 0,
        objects: HashMap::new(),
    };
    for line in text
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        replay.step += 1;
        let before = replay.out.len();
        replay.step(line, &source);
        let step = replay.step;
        let theirs: std::collections::BTreeMap<&String, &Json> = golden
            .iter()
            .filter(|(k, _)| k.starts_with(&format!("{step:03}_")) && !k.ends_with("_cmd"))
            .collect();
        let ours: std::collections::BTreeMap<&String, &Json> =
            replay.out.iter().skip(before).collect();
        if !golden.is_empty() && ours != theirs {
            let differing =
                |a: &std::collections::BTreeMap<&String, &Json>,
                 b: &std::collections::BTreeMap<&String, &Json>| {
                    a.iter()
                        .filter(|(k, v)| b.get(**k) != Some(v))
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                };
            panic!(
                "{script}: {line}\n  ours:   {:?}\n  theirs: {:?}",
                differing(&ours, &theirs),
                differing(&theirs, &ours)
            );
        }
    }
}

macro_rules! scripts {
    ($($name:ident: $script:literal),* $(,)?) => {
        $(#[test] fn $name() { replay($script); })*
    };
}

scripts! {
    probe: "00_probe",
    types: "01_types",
    orders: "02_orders",
    writes: "03_writes",
    maintenance: "04_maintenance",
    delete: "05_delete",
    exclusive: "06_exclusive",
    zap: "07_zap",
    pending: "09_pending",
    into_temp: "10_into_temp",
    query_log: "11_querylog",
    query_log_aes: "12_querylog_aes",
    query_log_dictionary: "13_querylog_dict",
    query_log_plain: "14_querylog_plain",
    index_files: "08_idxorder",
}
