//! The open-table and cursor caches (`AdsCacheOpenTables`,
//! `AdsCacheOpenCursors`, `AdsCloseCachedTables`) through a server, replayed
//! against `fixtures/cache/remote.txt`, and through the local server with
//! its caching on (`tflip.ini`'s `[cache] local`), against
//! `fixtures/cache/local.txt`: what `tools/ace/cache/cache.c` printed with
//! the original engine and the original's server and local server. Every
//! call must answer as there, and the tables listed open at each step
//! (`AdsMgGetOpenTables`) must be the same.
//!
//! Some things differ from the probe, which the comparison allows for: a
//! `tflipd` lists the tables of the asking connection's session only, so
//! the replay lists those of every connection it has open, which together
//! are what the original's server lists (but its error log table,
//! `ads_err.adt`); the local server lists those of the whole test process,
//! of which the replay takes those in its own folder; the probe connected
//! with `AdsConnect60` or `AdsConnect101`, which the comparison takes as
//! one; and `AdsCloseAllTables`, which would close other tests' tables in
//! the test process, is done as what it does: each table closed, and each
//! connection's cached tables.

use std::ffi::CString;
use std::path::Path;

use crate::connection::*;
use crate::fixtures;
use crate::mgmt::AdsMgGetOpenTables;
use crate::record::AdsLockTable;
use crate::settings::{AdsCacheOpenCursors, AdsCacheOpenTables};
use crate::statement::*;
use crate::table::*;
use crate::*;

/// A line of the probe's output as the comparison takes it: a call's label
/// and answer, a listing's label and sorted table names, a record count,
/// or a section's heading.
fn normalized(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    if line.starts_with("==") {
        return Some(line.to_owned());
    }
    if let Some(at) = line.find("rows=") {
        return Some(line[at..].to_owned());
    }
    if let Some((label, rest)) = line.split_once("] MgGetOpenTables rc=0 n=") {
        let names = rest.split_once(':').map_or("", |(_, names)| names);
        let mut names: Vec<&str> = names
            .split_whitespace()
            .filter(|name| *name != "ads_err.adt")
            .collect();
        names.sort_unstable();
        return Some(format!("{label}] {}", names.join(" ")));
    }
    let (label, rest) = line.split_once(" rc=")?;
    let rc = rest.split_whitespace().next().unwrap_or("");
    let label = label.trim();
    let label = if label.starts_with("AdsConnect") {
        "AdsConnect"
    } else {
        label
    };
    Some(format!("{label} rc={rc}"))
}

/// What the replay saw, as [`normalized`] lines.
struct Replay {
    lines: Vec<String>,
    /// The connections open, whose sessions' tables a listing shows.
    connections: Vec<AdsHandle>,
    /// The name of the replay's folder, the only one whose tables a
    /// listing shows (the local server's lists other tests' too).
    folder: String,
}

impl Replay {
    fn say(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    fn call(&mut self, label: &str, rc: u32) -> u32 {
        self.say(format!("{label} rc={rc}"));
        rc
    }

    fn connect(&mut self, source: &str) -> AdsHandle {
        let connect = fixtures::connect_to(source);
        self.say("AdsConnect rc=0");
        self.connections.push(connect);
        connect
    }

    fn disconnect(&mut self, connect: AdsHandle, label: Option<&str>) {
        let rc = AdsDisconnect(connect);
        if let Some(label) = label {
            self.call(label, rc);
        }
        self.connections.retain(|&c| c != connect);
    }

    fn sql(&mut self, connect: AdsHandle, text: &str) {
        let (rc, cursor, statement) = execute(connect, text);
        if cursor != 0 {
            AdsCloseTable(cursor);
        }
        AdsCloseSQLStatement(statement);
        self.call(&format!("sql {text}"), rc);
    }

    /// Runs a `SELECT`, printing its record count; its cursor and its
    /// statement.
    fn query(&mut self, connect: AdsHandle, text: &str) -> (AdsHandle, AdsHandle) {
        let (rc, cursor, statement) = execute(connect, text);
        self.call(&format!("query {text}"), rc);
        if cursor != 0 {
            self.say(format!("rows={}", count(cursor)));
        }
        (cursor, statement)
    }

    /// Runs a `SELECT` again on `statement`, printing its record count; its
    /// cursor.
    fn requery(&mut self, statement: AdsHandle, text: &str) -> AdsHandle {
        let sql = CString::new(text).unwrap();
        let mut cursor = 0;
        let rc =
            unsafe { AdsExecuteSQLDirect(statement, sql.as_ptr().cast_mut().cast(), &mut cursor) };
        self.call(&format!("requery {text}"), rc);
        if cursor != 0 {
            self.say(format!("rows={}", count(cursor)));
        }
        cursor
    }

    fn open(&mut self, connect: AdsHandle, name: &str, exclusive: bool) -> AdsHandle {
        let mut table = 0;
        let c_name = CString::new(name).unwrap();
        let options = if exclusive { ADS_EXCLUSIVE } else { ADS_SHARED };
        let rc = unsafe {
            AdsOpenTable(
                connect,
                c_name.as_ptr().cast_mut().cast(),
                std::ptr::null_mut(),
                ADS_ADT,
                1,
                1,
                2,
                options,
                &mut table,
            )
        };
        let mode = if exclusive { "excl" } else { "shared" };
        self.call(&format!("open {name} {mode}"), rc);
        table
    }

    fn close(&mut self, table: AdsHandle, label: &str) {
        let rc = AdsCloseTable(table);
        self.call(&format!("close {label}"), rc);
    }

    fn excl(&mut self, connect: AdsHandle) {
        let table = self.open(connect, "t.adt", true);
        if table != 0 {
            self.close(table, "t (excl)");
        }
    }

    /// The tables open on the server, as the original's listing shows them.
    fn list(&mut self, label: &str) {
        let mut names: Vec<String> = Vec::new();
        for &connect in &self.connections {
            let mut buf = vec![0u8; 262 * 4096];
            let (mut len, mut size) = (4096u16, 262u16);
            let null = std::ptr::null_mut();
            let rc = unsafe {
                AdsMgGetOpenTables(connect, null, 0, buf.as_mut_ptr(), &mut len, &mut size)
            };
            assert_eq!(rc, AE_SUCCESS, "listing for {label}");
            for entry in buf.chunks(usize::from(size)).take(usize::from(len)) {
                let name = entry.split(|&b| b == 0).next().unwrap();
                let name = String::from_utf8_lossy(name).into_owned();
                if name.rsplit(['\\', '/']).nth(1) != Some(self.folder.as_str()) {
                    continue;
                }
                let base = name.rsplit(['\\', '/']).next().unwrap_or(&name).to_owned();
                if !names.contains(&base) {
                    names.push(base);
                }
            }
        }
        names.sort_unstable();
        self.say(format!("[{label}] {}", names.join(" ")));
    }
}

/// Runs `text` on a statement of `connect`: the answer, the cursor (0 for
/// none) and the statement.
fn execute(connect: AdsHandle, text: &str) -> (u32, AdsHandle, AdsHandle) {
    let mut statement = 0;
    assert_eq!(
        unsafe { AdsCreateSQLStatement(connect, &mut statement) },
        AE_SUCCESS
    );
    let sql = CString::new(text).unwrap();
    let mut cursor = 0;
    let rc = unsafe { AdsExecuteSQLDirect(statement, sql.as_ptr().cast_mut().cast(), &mut cursor) };
    (rc, cursor, statement)
}

fn count(table: AdsHandle) -> u32 {
    let mut n = 0;
    unsafe { AdsGetRecordCount(table, ADS_IGNOREFILTERS, &mut n) };
    n
}

/// `tools/ace/cache/cache.c`, through tflip's exports and a server.
fn replay(source: &str, folder: &Path) -> Vec<String> {
    let folder = folder.file_name().unwrap().to_string_lossy().into_owned();
    let mut r = Replay {
        lines: Vec::new(),
        connections: Vec::new(),
        folder,
    };

    r.say("== defaults (nothing set)");
    let (d1, d2) = (r.connect(source), r.connect(source));
    let dm = r.connect(source);
    r.sql(d1, "CREATE TABLE d (a INTEGER)");
    let d = r.open(d1, "d.adt", false);
    r.close(d, "d");
    r.list("d closed");
    let (cursor, statement) = r.query(d1, "SELECT * FROM d WHERE a > 0");
    r.close(cursor, "cursor");
    r.call("AdsCloseSQLStatement", AdsCloseSQLStatement(statement));
    r.list("statement closed");
    let d = r.open(d2, "d.adt", true);
    if d != 0 {
        r.close(d, "d (excl)");
    }
    r.call("DROP via d2", 0);
    r.sql(d2, "DROP TABLE d");
    for connect in [d1, d2, dm] {
        r.disconnect(connect, None);
    }

    r.say("== answers before any connection");
    for value in [0u16, 1, 5, 250, 65535] {
        r.call(
            &format!("AdsCacheOpenTables({value})"),
            AdsCacheOpenTables(value),
        );
        r.call(
            &format!("AdsCacheOpenCursors({value})"),
            AdsCacheOpenCursors(value),
        );
    }
    r.call("AdsCloseCachedTables(0)", AdsCloseCachedTables(0));
    AdsCacheOpenTables(0);
    AdsCacheOpenCursors(0);

    r.say("== setup");
    let (mut c1, c2) = (r.connect(source), r.connect(source));
    r.call("AdsCloseCachedTables(c1)", AdsCloseCachedTables(c1));
    let _mg = r.connect(source);
    r.sql(c1, "CREATE TABLE t (a INTEGER)");
    r.sql(c1, "INSERT INTO t VALUES (1)");
    r.sql(c1, "CREATE TABLE u (a INTEGER)");
    r.sql(c1, "CREATE TABLE v (a INTEGER)");
    r.sql(c1, "CREATE TABLE w (a INTEGER)");
    r.list("after setup");

    r.say("== tables, cache 0");
    let t = r.open(c1, "t.adt", false);
    r.list("t open");
    r.close(t, "t");
    r.list("t closed");
    r.excl(c2);

    r.say("== tables, cache 2");
    r.call("AdsCacheOpenTables(2)", AdsCacheOpenTables(2));
    let t = r.open(c1, "t.adt", false);
    r.close(t, "t");
    r.list("t closed, cache 2");
    r.excl(c2);
    r.excl(c1);
    r.list("after excl");
    let t = r.open(c1, "t.adt", false);
    r.close(t, "t");
    let u = r.open(c1, "u.adt", false);
    r.close(u, "u");
    let v = r.open(c1, "v.adt", false);
    r.close(v, "v");
    r.list("t,u,v closed, cache 2");
    let w = r.open(c1, "w.adt", false);
    r.list("w open");
    r.close(w, "w");
    r.list("w closed");
    r.sql(c2, "INSERT INTO t VALUES (2)");
    let t = r.open(c1, "t.adt", false);
    r.say(format!("rows={}", count(t)));
    r.close(t, "t");
    r.sql(c2, "DROP TABLE w");
    r.list("after drop w");
    r.call("AdsCloseCachedTables(c2)", AdsCloseCachedTables(c2));
    r.list("after close cached c2");
    r.call("AdsCloseCachedTables(c1)", AdsCloseCachedTables(c1));
    r.list("after close cached c1");
    r.sql(c2, "DROP TABLE w");
    let t = r.open(c1, "t.adt", false);
    r.close(t, "t");
    r.list("t closed again");
    r.call("AdsCacheOpenTables(0)", AdsCacheOpenTables(0));
    r.list("after cache 0");
    let u = r.open(c1, "u.adt", false);
    r.close(u, "u");
    r.list("u closed, cache 0");
    r.disconnect(c1, Some("AdsDisconnect(c1)"));
    r.list("c1 disconnected");
    c1 = r.connect(source);
    AdsCloseCachedTables(c1);
    r.list("after close cached new c1");

    r.say("== table cache scope, cache 2");
    r.call("AdsCacheOpenTables(2)", AdsCacheOpenTables(2));
    let t = r.open(c1, "t.adt", false);
    r.close(t, "t (c1)");
    let u = r.open(c2, "u.adt", false);
    r.close(u, "u (c2)");
    r.list("t by c1, u by c2 closed");
    let v = r.open(c2, "v.adt", false);
    r.close(v, "v (c2)");
    r.list("then v by c2 closed");
    r.call("AdsCloseCachedTables(c2)", AdsCloseCachedTables(c2));
    r.list("after close cached c2");
    let t = r.open(c2, "t.adt", false);
    r.close(t, "t (c2)");
    r.list("t by c2 closed");
    let t1 = r.open(c1, "t.adt", false);
    let t2 = r.open(c1, "t.adt", false);
    r.list("t twice open on c1");
    r.call(
        "AdsCloseCachedTables(c1) while open",
        AdsCloseCachedTables(c1),
    );
    r.list("after close cached c1");
    // AdsCloseAllTables, for this test's tables and connections only.
    AdsCloseTable(t1);
    AdsCloseTable(t2);
    for connect in r.connections.clone() {
        AdsCloseCachedTables(connect);
    }
    r.call("AdsCloseAllTables", AE_SUCCESS);
    r.list("after close all tables");
    let t = r.open(c1, "t.adt", false);
    r.call("AdsLockTable", AdsLockTable(t));
    r.close(t, "t (locked)");
    r.list("locked t closed");
    let t = r.open(c2, "t.adt", false);
    r.call("AdsLockTable(c2)", AdsLockTable(t));
    r.close(t, "t (c2)");
    r.disconnect(c1, Some("AdsDisconnect(c1)"));
    r.list("c1 disconnected");
    r.call("AdsCacheOpenTables(0)", AdsCacheOpenTables(0));
    r.list("tables 0");

    r.say("== cursors, cache 0");
    r.call("AdsCacheOpenCursors(0)", AdsCacheOpenCursors(0));
    let c3 = r.connect(source);
    let (cursor, statement) = r.query(c3, "SELECT * FROM t WHERE a > 0");
    r.list("cursor open");
    r.close(cursor, "cursor");
    r.list("cursor closed");
    r.call("AdsCloseSQLStatement", AdsCloseSQLStatement(statement));
    r.list("statement closed");
    r.excl(c2);

    r.say("== cursors, cache 5");
    r.call("AdsCacheOpenCursors(5)", AdsCacheOpenCursors(5));
    let (cursor, statement) = r.query(c3, "SELECT * FROM t WHERE a > 0");
    r.close(cursor, "cursor");
    r.call("AdsCloseSQLStatement", AdsCloseSQLStatement(statement));
    r.list("statement closed, cache 5");
    r.excl(c2);
    r.sql(c2, "INSERT INTO t VALUES (3)");
    let (cursor, statement) = r.query(c3, "SELECT * FROM t WHERE a > 0");
    r.close(cursor, "cursor");
    r.call("AdsCloseSQLStatement", AdsCloseSQLStatement(statement));
    r.list("statement closed");
    r.call("AdsCloseCachedTables(c3)", AdsCloseCachedTables(c3));
    r.list("after close cached c3");
    r.call("AdsCacheOpenCursors(0)", AdsCacheOpenCursors(0));
    r.list("after cursors 0");
    r.excl(c2);
    r.disconnect(c3, Some("AdsDisconnect(c3)"));
    r.list("c3 disconnected");
    r.excl(c2);

    r.say("== statements hold tables");
    r.call("AdsCacheOpenCursors(0)", AdsCacheOpenCursors(0));
    let c4 = r.connect(source);
    r.sql(c2, "CREATE TABLE x (a INTEGER)");
    let (cursor, statement) = r.query(c4, "SELECT * FROM t WHERE a > 0");
    r.close(cursor, "cursor");
    r.list("cursor closed, statement open");
    r.excl(c2);
    r.list("after excl");
    r.call("AdsCloseCachedTables(c4)", AdsCloseCachedTables(c4));
    r.list("after close cached c4");
    let cursor = r.requery(statement, "SELECT * FROM u WHERE a > 0");
    r.list("ran again on u, cursor open");
    r.close(cursor, "cursor");
    r.list("ran again on u, cursor closed");
    r.excl(c2);
    r.call("AdsCloseSQLStatement", AdsCloseSQLStatement(statement));
    r.list("statement closed");
    r.excl(c2);
    let (cursor, statement) = r.query(c4, "SELECT * FROM x WHERE a > 0");
    r.close(cursor, "cursor");
    r.sql(c2, "DROP TABLE x");
    r.list("x dropped, statement open");
    r.call("AdsCloseSQLStatement", AdsCloseSQLStatement(statement));
    r.list("statement closed");
    let (_cursor, statement) = r.query(c4, "SELECT * FROM t WHERE a > 0");
    r.list("cursor open");
    r.call(
        "AdsCloseSQLStatement (cursor open)",
        AdsCloseSQLStatement(statement),
    );
    r.list("statement closed with its cursor");
    r.call("AdsCacheOpenCursors(5)", AdsCacheOpenCursors(5));
    let (cursor, statement) = r.query(c4, "SELECT * FROM t WHERE a > 0");
    r.close(cursor, "cursor");
    r.list("cursor closed, cache 5");
    r.excl(c2);
    r.list("after excl, cache 5");
    r.call("AdsCloseSQLStatement", AdsCloseSQLStatement(statement));
    r.list("statement closed, cache 5");
    r.excl(c2);
    r.disconnect(c4, Some("AdsDisconnect(c4)"));
    r.list("c4 disconnected");

    for connect in r.connections.clone() {
        r.disconnect(connect, None);
    }
    r.lines
}

/// The caches answer and keep tables open as the original server does.
#[test]
fn open_cache_matches_the_original_server() {
    // The cache sizes are the process's; other tests keep theirs meanwhile.
    let _settings = fixtures::settings_lock();
    let empty = tflip_storage::testing::temp_root()
        .join(format!("tflip-open-cache-{}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    let mut got = Vec::new();
    fixtures::remotely_with(|| {
        let (source, dir) = fixtures::scratch_from("open_cache", &empty);
        got = replay(&source, &dir);
    });
    crate::settings::forget_cache_sizes();
    compare(&got, "remote.txt");
}

/// With its caching on (`tflip.ini`'s `[cache] local`), the local server's
/// caches answer and keep tables open as the original's local server does.
#[test]
fn local_open_cache_matches_the_original_local_server() {
    let _settings = fixtures::settings_lock();
    let empty = tflip_storage::testing::temp_root()
        .join(format!("tflip-open-cache-{}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    tflip_engine::settings::set_local_caching(&crate::LOCAL, true);
    let (source, dir) = fixtures::scratch_from("open_cache_local", &empty);
    let got = replay(&source, &dir);
    crate::settings::forget_cache_sizes();
    compare(&got, "local.txt");
}

/// Compares what a replay saw, `got`, with the probe's output `golden`.
fn compare(got: &[String], golden: &str) {
    let golden = fixtures::dir("").join("../cache").join(golden);
    let want: Vec<String> = std::fs::read_to_string(golden)
        .unwrap()
        .lines()
        .filter_map(normalized)
        .collect();
    let mut differences = Vec::new();
    for at in 0..got.len().max(want.len()) {
        let (g, w) = (got.get(at), want.get(at));
        if g != w {
            differences.push(format!("  #{at} got {g:?}\n      want {w:?}"));
        }
    }
    assert!(
        differences.is_empty(),
        "{} differ:\n{}",
        differences.len(),
        differences.join("\n")
    );
}
