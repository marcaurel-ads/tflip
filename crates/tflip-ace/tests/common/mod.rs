//! What the external procedure tests share: the client library `cargo`
//! built (`libtflip_ace.so`), loaded as a client loads it; the library
//! `tools/ace/aep/aep.c`, built with `cc`; and a replay of the commands of a
//! `tools/ace/sql/sql.c` fixture that its external procedure scripts use.

#![allow(dead_code)]

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::process::Command;

type Handle = usize;

unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}

pub const RTLD_NOW: c_int = 2;
const RTLD_GLOBAL: c_int = 0x100;

pub fn open(path: &Path, flags: c_int) -> *mut c_void {
    let name = CString::new(path.to_str().unwrap()).unwrap();
    let module = unsafe { dlopen(name.as_ptr(), flags) };
    assert!(!module.is_null(), "{}: {:?}", path.display(), unsafe {
        CStr::from_ptr(dlerror())
    });
    module
}

pub fn symbol<F: Copy>(module: *mut c_void, name: &str) -> F {
    let c = CString::new(name).unwrap();
    let address = unsafe { dlsym(module, c.as_ptr()) };
    assert!(!address.is_null(), "no {name}");
    unsafe { std::mem::transmute_copy(&address) }
}

/// The client library's exports the tests call.
pub struct Ace {
    connect101: unsafe extern "system" fn(*const u8, *mut Handle, *mut Handle) -> u32,
    create_statement: unsafe extern "system" fn(Handle, *mut Handle) -> u32,
    execute_direct: unsafe extern "system" fn(Handle, *const u8, *mut Handle) -> u32,
    record_count: unsafe extern "system" fn(Handle, u16, *mut u32) -> u32,
    close_table: unsafe extern "system" fn(Handle) -> u32,
    disconnect: unsafe extern "system" fn(Handle) -> u32,
    last_error: unsafe extern "system" fn(*mut u32, *mut u8, *mut u16) -> u32,
    num_fields: unsafe extern "system" fn(Handle, *mut u16) -> u32,
    field_name: unsafe extern "system" fn(Handle, u16, *mut u8, *mut u16) -> u32,
    field_type: unsafe extern "system" fn(Handle, *const u8, *mut u16) -> u32,
    field_length: unsafe extern "system" fn(Handle, *const u8, *mut u32) -> u32,
    field_decimals: unsafe extern "system" fn(Handle, *const u8, *mut u16) -> u32,
    goto_top: unsafe extern "system" fn(Handle) -> u32,
    at_eof: unsafe extern "system" fn(Handle, *mut u16) -> u32,
    record_num: unsafe extern "system" fn(Handle, u16, *mut u32) -> u32,
    is_null: unsafe extern "system" fn(Handle, *const u8, *mut u16) -> u32,
    get_field: unsafe extern "system" fn(Handle, *const u8, *mut u8, *mut u32, u16) -> u32,
    skip: unsafe extern "system" fn(Handle, i32) -> u32,
    transaction_count: unsafe extern "system" fn(Handle, *mut u32) -> u32,
    open_table: unsafe extern "system" fn(
        Handle,
        *const u8,
        *const u8,
        u16,
        u16,
        u16,
        u16,
        u32,
        *mut Handle,
    ) -> u32,
}

/// The client library `cargo` built next to the running test.
pub fn ace_library() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let deps = exe.parent().unwrap();
    [
        deps.join("libtflip_ace.so"),
        deps.parent().unwrap().join("libtflip_ace.so"),
    ]
    .into_iter()
    .find(|path| path.exists())
    .expect("libtflip_ace.so")
}

/// Builds `tools/ace/aep/aep.c` as `library`, linking the client library
/// in `ace` if given (else its calls bind to the one the process loaded);
/// `None` if there is no `cc`.
pub fn build_library(library: &Path, ace: Option<&Path>) -> Option<()> {
    build_source("aep.c", library, ace, &[])
}

/// Builds the source `tools/ace/aep/<source>` as `library`, with the `-D`
/// definitions `defines`, as [`build_library`] does `aep.c`.
pub fn build_source(
    source: &str,
    library: &Path,
    ace: Option<&Path>,
    defines: &[&str],
) -> Option<()> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/ace/aep")
        .join(source);
    let mut cc = Command::new("cc");
    cc.args(["-shared", "-fPIC", "-o"]).arg(library).arg(source);
    cc.args(defines.iter().map(|define| format!("-D{define}")));
    // Linked by its path, which the library then needs as it is: a search
    // would go by `LD_LIBRARY_PATH`, which `cargo test` points at
    // `target/debug` too, where an older build may lie.
    if let Some(ace) = ace {
        cc.arg(ace);
    }
    match cc.status() {
        Ok(status) => {
            assert!(status.success(), "cc failed");
            Some(())
        }
        Err(error) => {
            eprintln!("skipped: no cc ({error})");
            None
        }
    }
}

fn field_arg(f: u16) -> *const u8 {
    f as usize as *const u8
}

impl Ace {
    /// Loads the client library at `path`, globally, so that a library the
    /// process loads after it binds to it.
    pub fn load(path: &Path) -> Ace {
        let module = open(path, RTLD_NOW | RTLD_GLOBAL);
        Ace {
            connect101: symbol(module, "AdsConnect101"),
            create_statement: symbol(module, "AdsCreateSQLStatement"),
            execute_direct: symbol(module, "AdsExecuteSQLDirect"),
            record_count: symbol(module, "AdsGetRecordCount"),
            close_table: symbol(module, "AdsCloseTable"),
            disconnect: symbol(module, "AdsDisconnect"),
            last_error: symbol(module, "AdsGetLastError"),
            num_fields: symbol(module, "AdsGetNumFields"),
            field_name: symbol(module, "AdsGetFieldName"),
            field_type: symbol(module, "AdsGetFieldType"),
            field_length: symbol(module, "AdsGetFieldLength"),
            field_decimals: symbol(module, "AdsGetFieldDecimals"),
            goto_top: symbol(module, "AdsGotoTop"),
            at_eof: symbol(module, "AdsAtEOF"),
            record_num: symbol(module, "AdsGetRecordNum"),
            is_null: symbol(module, "AdsIsNull"),
            get_field: symbol(module, "AdsGetField"),
            skip: symbol(module, "AdsSkip"),
            transaction_count: symbol(module, "AdsGetTransactionCount"),
            open_table: symbol(module, "AdsOpenTable"),
        }
    }

    fn connect(&self, source: &str) -> Result<Handle, String> {
        let source = CString::new(source).unwrap();
        let mut connection = 0;
        match unsafe {
            (self.connect101)(
                source.as_ptr().cast(),
                std::ptr::null_mut(),
                &mut connection,
            )
        } {
            0 => Ok(connection),
            rc => Err(self.error(rc)),
        }
    }

    fn error(&self, rc: u32) -> String {
        let (mut code, mut buf, mut len) = (0, [0u8; 1200], 1200u16);
        unsafe { (self.last_error)(&mut code, buf.as_mut_ptr(), &mut len) };
        format!("{rc}: {}", String::from_utf8_lossy(&buf[..len as usize]))
    }

    /// A cursor's fields and rows, as `sql.c` records them.
    fn cursor(&self, cursor: Handle) -> serde_json::Value {
        let mut count = 0;
        unsafe { (self.num_fields)(cursor, &mut count) };
        let mut fields = Vec::new();
        for f in 1..=count {
            let (mut name, mut len) = ([0u8; 260], 260u16);
            let (mut ty, mut length, mut decimals) = (0u16, 0u32, 0u16);
            unsafe {
                (self.field_name)(cursor, f, name.as_mut_ptr(), &mut len);
                (self.field_type)(cursor, field_arg(f), &mut ty);
                (self.field_length)(cursor, field_arg(f), &mut length);
                (self.field_decimals)(cursor, field_arg(f), &mut decimals);
            }
            let name = String::from_utf8_lossy(&name[..len as usize]).into_owned();
            fields.push(serde_json::json!([name, ty, length, decimals]));
        }
        let mut rows = Vec::new();
        unsafe { (self.goto_top)(cursor) };
        loop {
            let mut eof = 0;
            unsafe { (self.at_eof)(cursor, &mut eof) };
            if eof != 0 {
                break;
            }
            let mut record = 0;
            unsafe { (self.record_num)(cursor, 2, &mut record) };
            let mut values = Vec::new();
            for f in 1..=count {
                let mut null = 0;
                unsafe { (self.is_null)(cursor, field_arg(f), &mut null) };
                if null != 0 {
                    values.push(serde_json::Value::Null);
                    continue;
                }
                let (mut buf, mut len) = (vec![0u8; 70000], 70000u32);
                let rc = unsafe {
                    (self.get_field)(cursor, field_arg(f), buf.as_mut_ptr(), &mut len, 0)
                };
                values.push(match rc {
                    0 => String::from_utf8_lossy(&buf[..len as usize])
                        .into_owned()
                        .into(),
                    rc => format!("E{rc}").into(),
                });
            }
            rows.push(serde_json::json!([record, values]));
            unsafe { (self.skip)(cursor, 1) };
        }
        serde_json::json!({"fields": fields, "rows": rows})
    }
}

/// Replays the fixture `fixture` (the commands `stmt`, `direct`, `query`,
/// `connect` (with a user and password), `disconnect`, `switch` and
/// `txcount` of `sql.c`) on the data folder `folder`,
/// connected to with the connect string's `options` (`ServerType=LOCAL`, or
/// `ServerType=REMOTE` for `tflipd`, whose connections are TLS), with `join` making
/// a path in it; `shown` turns tflip's error texts into the fixture's (its
/// paths). Returns what differs, as one line each.
pub fn replay(
    ace: &Ace,
    fixture: &Path,
    folder: &str,
    options: &str,
    join: impl Fn(&str, &str) -> String,
    shown: impl Fn(&str) -> String,
) -> Vec<String> {
    let recorded: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(fixture).unwrap()).unwrap();
    let mut connection = ace
        .connect(&format!("Data Source={folder};{options}"))
        .unwrap();
    let mut statement = 0;
    // `switch`'s other connection and statement, none the first time.
    let mut other = (0, 0);
    let mut differ = Vec::new();
    for (step, entry) in recorded.iter().enumerate() {
        let line = entry["cmd"].as_str().unwrap();
        let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
        let mut got = serde_json::Map::new();
        let rc = match cmd {
            "stmt" => unsafe { (ace.create_statement)(connection, &mut statement) },
            "direct" | "query" => {
                let sql = CString::new(rest).unwrap();
                let mut cursor = 0;
                let rc =
                    unsafe { (ace.execute_direct)(statement, sql.as_ptr().cast(), &mut cursor) };
                got.insert("has_cursor".into(), (cursor != 0 && rc == 0).into());
                if rc == 0 {
                    let mut affected = 0;
                    let crc = unsafe { (ace.record_count)(statement, 2, &mut affected) };
                    got.insert(
                        "affected".into(),
                        (if crc == 0 { affected } else { 0 }).into(),
                    );
                }
                if rc == 0 && cursor != 0 {
                    got.insert("cursor".into(), ace.cursor(cursor));
                    if cmd == "query" {
                        unsafe { (ace.close_table)(cursor) };
                    }
                }
                rc
            }
            "connect" => {
                let mut words = rest.split(' ');
                let path = join(folder, words.next().unwrap());
                let user = words.next().unwrap_or("");
                let password = words.next().unwrap_or("");
                let source =
                    format!("Data Source={path};{options};User ID={user};Password={password}");
                match ace.connect(&source) {
                    Ok(fresh) => {
                        connection = fresh;
                        0
                    }
                    Err(error) => error.split(':').next().unwrap().parse().unwrap(),
                }
            }
            "disconnect" => unsafe { (ace.disconnect)(connection) },
            "switch" => {
                (connection, other.0) = (other.0, connection);
                (statement, other.1) = (other.1, statement);
                0
            }
            "txcount" => {
                let mut count = 0xAAAA;
                let rc = unsafe { (ace.transaction_count)(connection, &mut count) };
                got.insert("count".into(), count.into());
                rc
            }
            // `sql.c`'s `open <name>` and `otype <type> <name>`: only the
            // result is compared.
            "open" | "otype" => {
                let (kind, name) = match cmd {
                    "open" => ("0", rest),
                    _ => rest.split_once(' ').unwrap(),
                };
                let name = CString::new(name).unwrap();
                let mut table = 0;
                let rc = unsafe {
                    (ace.open_table)(
                        connection,
                        name.as_ptr().cast(),
                        std::ptr::null(),
                        kind.parse().unwrap(),
                        1,
                        0,
                        1,
                        0,
                        &mut table,
                    )
                };
                if rc == 0 {
                    unsafe { (ace.close_table)(table) };
                }
                rc
            }
            other => panic!("{other} is not replayed"),
        };
        got.insert("rc".into(), rc.into());
        if rc != 0 {
            let error = ace.error(rc);
            let text = error.split_once(": ").map_or("", |(_, text)| text);
            got.insert("error".into(), shown(text).into());
        }
        let mut wanted = entry.as_object().unwrap().clone();
        wanted.remove("cmd");
        // What a failed statement leaves as its rows affected is not compared.
        if rc != 0 || wanted["rc"] != 0 {
            wanted.remove("affected");
        }
        if let Some(cursor) = wanted.get_mut("cursor") {
            *cursor = serde_json::json!({"fields": cursor["fields"], "rows": cursor["rows"]});
        }
        let got = serde_json::Value::Object(got);
        let wanted = serde_json::Value::Object(wanted);
        if got != wanted {
            differ.push(format!(
                "step {step}: {line}\n  wanted {wanted}\n  got    {got}"
            ));
        }
    }
    differ
}
