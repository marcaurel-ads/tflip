//! Replays `fixtures/events/*.json`: event notifications across several
//! connections and threads (`tools/ace/events/`, recorded on the original's
//! remote server), against tflip as the client library does them in one
//! process and, through `tflipd`, as sessions of a server.

use std::collections::HashMap;
use std::ffi::CString;
use std::path::PathBuf;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Map, Value as Json, json};

use crate::fixtures;
use crate::{
    AdsAtEOF, AdsCloseSQLStatement, AdsCloseTable, AdsCreateSQLStatement, AdsDisconnect,
    AdsExecuteSQLDirect, AdsGetField, AdsGetFieldLength, AdsGetFieldName, AdsGetFieldType,
    AdsGetLastError, AdsGetNumFields, AdsGotoTop, AdsHandle, AdsIsNull, AdsSkip, Unsigned8,
};

/// Bytes as the probe records them.
fn raw(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

fn last_error() -> String {
    let (mut code, mut buf, mut len) = (0, [0u8; 1200], 1200u16);
    unsafe { AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
    raw(&buf[..usize::from(len).min(buf.len())])
}

/// Runs `sql` on `connection` as the probe's `sql` command does, and
/// answers its result: the code, the error or the columns and the rows.
fn run(connection: AdsHandle, sql: &str) -> Json {
    run_keeping(connection, sql, None)
}

/// [`run`], leaving the statement and its cursor open in `keep` (the probe's
/// `hold`), as a client does that has not closed them yet.
fn run_keeping(connection: AdsHandle, sql: &str, keep: Option<&mut [AdsHandle; 2]>) -> Json {
    let mut object = Map::new();
    let mut statement = 0;
    let mut cursor = 0;
    // The probe's `\r`, `\n` and `\t` stand for those characters.
    let sql = sql
        .replace("\\r", "\r")
        .replace("\\n", "\n")
        .replace("\\t", "\t");
    let text = CString::new(crate::ansi::from_str_lossy(&sql)).unwrap();
    let rc = unsafe {
        match AdsCreateSQLStatement(connection, &mut statement) {
            0 => AdsExecuteSQLDirect(statement, text.as_ptr().cast_mut().cast(), &mut cursor),
            rc => rc,
        }
    };
    object.insert("rc".into(), json!(rc));
    if rc != 0 {
        object.insert("error".into(), json!(last_error()));
    } else if cursor != 0 {
        unsafe {
            let mut fields = 0;
            AdsGetNumFields(cursor, &mut fields);
            let mut columns = Vec::new();
            for f in 1..=fields {
                let (mut name, mut len) = ([0u8; 260], 260u16);
                AdsGetFieldName(cursor, f, name.as_mut_ptr(), &mut len);
                let (mut ftype, mut flen) = (0, 0);
                let field = usize::from(f) as *mut Unsigned8;
                AdsGetFieldType(cursor, field, &mut ftype);
                AdsGetFieldLength(cursor, field, &mut flen);
                columns.push(json!([raw(&name[..usize::from(len)]), ftype, flen]));
            }
            object.insert("fields".into(), json!(columns));
            let mut rows = Vec::new();
            AdsGotoTop(cursor);
            for _ in 0..100 {
                let mut eof = 0;
                AdsAtEOF(cursor, &mut eof);
                if eof != 0 {
                    break;
                }
                let mut values = Vec::new();
                for f in 1..=fields {
                    let field = usize::from(f) as *mut Unsigned8;
                    let mut null = 0;
                    AdsIsNull(cursor, field, &mut null);
                    if null != 0 {
                        values.push(Json::Null);
                        continue;
                    }
                    let (mut buf, mut len) = (vec![0u8; 70_000], 70_000u32);
                    match AdsGetField(cursor, field, buf.as_mut_ptr(), &mut len, 0) {
                        0 => values.push(json!(raw(&buf[..len as usize]))),
                        rc => values.push(json!(format!("E{rc}"))),
                    }
                }
                rows.push(json!(values));
                AdsSkip(cursor, 1);
            }
            object.insert("rows".into(), json!(rows));
        }
    }
    match keep {
        Some(keep) => *keep = [statement, cursor],
        None => close_both(statement, cursor),
    }
    Json::Object(object)
}

fn close_both(statement: AdsHandle, cursor: AdsHandle) {
    if cursor != 0 {
        AdsCloseTable(cursor);
    }
    if statement != 0 {
        AdsCloseSQLStatement(statement);
    }
}

/// What of a result is compared: not how long it took, and a `ROWID` only
/// by its record number (it names the dictionary and the table by their
/// paths).
fn comparable(value: &mut Json) {
    match value {
        Json::String(s)
            if s.len() == 18
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
                && s.bytes().any(|b| b.is_ascii_uppercase())
                && s[12..].starts_with("AAA") =>
        {
            *s = format!("<rowid>{}", &s[12..]);
        }
        // The file a refusal to open names is in the server's folder as the
        // original names it: by its file name only.
        Json::String(s) if s.contains("Error 7008:") => {
            const AFTER: &str = "opened.  ";
            if let (Some(from), Some(to)) = (s.find(AFTER), s.rfind(" Table name:")) {
                let from = from + AFTER.len();
                let file = s[from..to]
                    .rsplit(['\\', '/'])
                    .next()
                    .unwrap_or("")
                    .to_owned();
                s.replace_range(from..to, &file);
            }
        }
        // The same for the table a refusal of free tables names.
        Json::String(s) if s.contains("Error 7087:") && s.contains(" Table name:") => {
            const AFTER: &str = "granted to the user.  ";
            if let (Some(from), Some(to)) = (s.find(AFTER), s.rfind(" Table name:")) {
                let from = from + AFTER.len();
                let file = s[from..to]
                    .rsplit(['\\', '/'])
                    .next()
                    .unwrap_or("")
                    .to_owned();
                s.replace_range(from..to, &file);
            }
        }
        Json::Array(items) => items.iter_mut().for_each(comparable),
        Json::Object(map) => {
            map.remove("ms");
            map.values_mut().for_each(comparable);
        }
        _ => {}
    }
}

fn replay(file: &str) {
    replay_as(file, false);
}

/// Replays `file`; if `counted`, with the transactions of the original's
/// local server (the fixture is recorded on it).
fn replay_as(file: &str, counted: bool) {
    replay_in(file, counted, None);
}

/// Replays `file` in a copy of the folder `from` (an empty one without),
/// which the probe's script ran in, made by an earlier one.
fn replay_in(file: &str, counted: bool, from: Option<PathBuf>) {
    let _settings = fixtures::settings_lock();
    /// Puts the transactions back to real ones when the replay is over.
    struct Counted;
    impl Drop for Counted {
        fn drop(&mut self) {
            tflip_engine::settings::set_real_transactions(&crate::LOCAL, true);
        }
    }
    let _counted = counted.then(|| {
        tflip_engine::settings::set_real_transactions(&crate::LOCAL, false);
        Counted
    });
    let steps: Vec<Json> = serde_json::from_slice(
        &std::fs::read(fixtures::dir(&format!("../events/{file}"))).unwrap(),
    )
    .unwrap();
    let from = from.unwrap_or_else(fixtures::empty_dir);
    let name = format!("events-{}", file.trim_end_matches(".json"));
    let (source, dir) = fixtures::scratch_from(&name, &from);
    // The folders the dictionaries are made in, which the server makes itself.
    for step in &steps {
        let cmd = step["cmd"].as_str().unwrap();
        if let Some((_, rest)) = cmd.split_once("CREATE DATABASE \"")
            && let Some((folder, _)) = rest.split_once('\\')
        {
            std::fs::create_dir_all(dir.join(folder)).unwrap();
        }
    }
    let mut connections: HashMap<usize, AdsHandle> = HashMap::new();
    let mut waiting: HashMap<usize, JoinHandle<Json>> = HashMap::new();
    let mut held: HashMap<usize, [AdsHandle; 2]> = HashMap::new();
    // The connections that were refused, which the probe goes on to use as
    // the handle 0 they are.
    let mut refused: Vec<usize> = Vec::new();
    let mut failures = Vec::new();
    for step in &steps {
        let cmd = step["cmd"].as_str().unwrap();
        let (verb, rest) = cmd.split_once(' ').unwrap_or((cmd, ""));
        let number = |text: &str| text.parse::<usize>().unwrap();
        let connected = |id: &str| match connections.get(&number(id)) {
            Some(handle) => *handle,
            None if refused.contains(&number(id)) => 0,
            None => panic!(
                "no connection {id} for {cmd}; earlier differences:\n{}",
                failures.join("\n")
            ),
        };
        let mut got = Map::new();
        got.insert("cmd".into(), json!(cmd));
        match verb {
            "free" | "connect" => {
                let mut words = rest.split(' ');
                let id = number(words.next().unwrap());
                let target = match verb {
                    "free" => source.clone(),
                    _ => {
                        let relative = words.next().unwrap();
                        match fixtures::is_remote() {
                            true => format!(r"{source}\{relative}"),
                            false => {
                                let mut path = dir.clone();
                                relative.split(['\\', '/']).for_each(|part| path.push(part));
                                path.display().to_string()
                            }
                        }
                    }
                };
                let (user, password) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
                let user = if verb == "connect" && user.is_empty() {
                    "ADSSYS"
                } else {
                    user
                };
                let (rc, handle) = fixtures::connect_to_as(&target, user, password);
                got.insert("rc".into(), json!(rc));
                if rc == 0 {
                    connections.insert(id, handle);
                    refused.retain(|&refused| refused != id);
                } else {
                    if step.get("error").is_some() {
                        got.insert("error".into(), json!(last_error()));
                    }
                    if !connections.contains_key(&id) {
                        refused.push(id);
                    }
                }
            }
            "sql" => {
                let (id, sql) = rest.split_once(' ').unwrap();
                if let Json::Object(result) = run(connected(id), sql) {
                    got.extend(result);
                }
            }
            "hold" => {
                let (id, sql) = rest.split_once(' ').unwrap();
                let mut keep = [0, 0];
                if let Json::Object(result) = run_keeping(connected(id), sql, Some(&mut keep)) {
                    got.extend(result);
                }
                held.insert(number(id), keep);
            }
            "release" => {
                let [statement, cursor] = held.remove(&number(rest)).unwrap();
                close_both(statement, cursor);
                got.insert("rc".into(), json!(0));
            }
            "cancel" => {
                let id = connected(rest);
                let sql = "SELECT MAX(\"Query Number\") FROM (EXECUTE PROCEDURE sp_GetSQLStatements()) a \
                           WHERE Query LIKE '%WaitFor%Event%'";
                let found = run(id, sql);
                let number = found["rows"][0][0].as_str().unwrap_or("0").to_owned();
                if let Json::Object(result) =
                    run(id, &format!("EXECUTE PROCEDURE sp_CancelQuery({number})"))
                {
                    got.extend(result);
                }
            }
            "async" => {
                let (id, sql) = rest.split_once(' ').unwrap();
                let (connection, sql) = (connected(id), sql.to_owned());
                waiting.insert(number(id), thread::spawn(move || run(connection, &sql)));
                got.insert("started".into(), json!(true));
            }
            "join" => {
                let handle = waiting.remove(&number(rest)).unwrap();
                if let Json::Object(result) = handle.join().unwrap() {
                    got.extend(result);
                }
            }
            // Waits the original's server needed, not behaviour.
            "pace" => {
                got.insert("paced".into(), json!(true));
            }
            "sleep" => {
                thread::sleep(Duration::from_millis(rest.parse().unwrap()));
                got.insert("slept".into(), json!(true));
            }
            "disconnect" => {
                let rc = AdsDisconnect(connected(rest));
                got.insert("rc".into(), json!(rc));
            }
            other => panic!("unknown command {other}"),
        }
        let (mut got, mut want) = (Json::Object(got), step.clone());
        comparable(&mut got);
        comparable(&mut want);
        if got != want {
            failures.push(format!("{want}\n  tflip: {got}"));
        }
    }
    // What the script leaves connected would stay open for the rest of the
    // process, every remote one a session that the exports asking all
    // servers (`AdsShowDeleted`, ...) ask too.
    for (_, connection) in connections {
        AdsDisconnect(connection);
    }
    assert!(
        failures.is_empty(),
        "{} of {} steps differ from the original's:\n{}",
        failures.len(),
        steps.len(),
        failures.join("\n")
    );
}

macro_rules! replays {
    ($($name:ident: $file:literal),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                replay($file);
            }
        )*
        mod remote {
            $(
                #[test]
                fn $name() {
                    crate::fixtures::remotely_with(|| super::replay($file));
                }
            )*
        }
    };
}

replays!(
    basic: "events_basic.json",
    data: "events_data.json",
    trigger: "events_trigger.json",
    scope: "events_scope.json",
    transactions: "events_tx.json",
);

/// What only a server shows: its list of statements is every client's (each
/// remote connection is a session of its own on `tflipd`), so that a wait one
/// connection cancels from another is found, and a long query is active. The
/// original's local server cancels nothing (see [`local_server`]).
#[test]
fn remote_server() {
    for file in [
        "events_wait.json",
        "events_statements.json",
        "events_trigger_shared.json",
    ] {
        fixtures::remotely_with(|| replay(file));
    }
}

/// A trigger created while another connection's statement holds the table
/// open is not fired by any connection until none has the table open, as the
/// original's one open instance of a table goes on with the triggers it
/// loaded; tflip loads them for each statement (KNOWN_ISSUES).
#[test]
#[ignore = "tflip loads a table's triggers for each statement"]
fn trigger_changes_while_a_table_is_open() {
    fixtures::remotely_with(|| replay("events_trigger_open.json"));
}

/// The online forms of `CREATE INDEX` and `ALTER TABLE` leave the table open
/// to another connection, the offline ones do not (7008), and
/// `ONLINE_WITH_TRANSITION` waits for the connections that opened the table
/// meanwhile. Slow: the table has 131072 records, and the replay needs
/// statements that last as long as the original's (a release build does).
#[test]
fn online_maintenance() {
    fixtures::remotely_with(|| replay("events_online.json"));
}

/// The events the online forms signal when their final stage begins, and the
/// offline ones do not; the table of a dictionary is closed to other
/// connections by the offline forms as a free table is (the original names no
/// file in the error then).
#[test]
fn online_final_stage_events() {
    fixtures::remotely_with(|| replay("events_online_final.json"));
}

/// The arguments, options and errors of the online pack and reindex
/// procedures (`tools/ace/events/events_online_args.txt`).
#[test]
fn online_procedure_arguments() {
    fixtures::remotely_with(|| replay("events_online_args.json"));
}

/// What `sp_PackTableOnline`, `sp_ReindexOnline` and `sp_PackAllTablesOnline`
/// leave open to other connections, their events, and how a transaction on
/// the table and the `Timeout` option hold them back. Slow, and timing
/// sensitive like [`online_maintenance`].
#[test]
fn online_procedures() {
    fixtures::remotely_with(|| replay("events_online_procs.json"));
}

/// The configuration `sp_mgGetConfigInfo` reports: the local server's (the
/// client library's own) and the remote server's (a `tflipd` session's), as
/// probed on both.
fn configuration(item: &str) -> String {
    let (source, _) = fixtures::scratch_from("events-config", &fixtures::empty_dir());
    let connection = fixtures::connect_to(&source);
    let result = run(connection, "EXECUTE PROCEDURE sp_mgGetConfigInfo()");
    AdsDisconnect(connection);
    let rows = result["rows"].as_array().unwrap();
    let row = rows
        .iter()
        .find(|row| row[0].as_str().unwrap().trim_end() == item)
        .unwrap();
    row[1].as_str().unwrap().trim_end().to_owned()
}

#[test]
fn a_servers_session_reports_the_remote_servers_configuration() {
    let _settings = fixtures::settings_lock();
    let items = [
        "Burst Packets",
        "Sort Buffer",
        "Receive IP Port",
        "Use Internet",
    ];
    let local = items.map(configuration);
    assert_eq!(local, ["0", "32768", "0", "0"]);
    let remote = items.map(|item| {
        let mut value = String::new();
        fixtures::remotely_with(|| value = configuration(item));
        value
    });
    assert_eq!(remote, ["16", "8192", "6262", "1"]);
}

/// What the original's local server does, which has no transactions: a signal
/// that waits for the commit is sent at once, and nothing cancels a wait
/// (`sp_CancelQuery` is not implemented there).
#[test]
fn local_server() {
    replay_as("events_tx_local.json", true);
    replay_as("events_wait_local.json", true);
}

/// The original's local server counts no failed logins, whatever
/// `ENFORCE_MAX_FAILED_LOGINS` says (`tools/ace/events/lockout_local.txt`),
/// nor does tflip's in-process engine.
#[test]
fn local_server_locks_no_one_out() {
    replay("lockout_local.json");
}

/// A user whose failed logins reach `MAX_FAILED_ATTEMPTS` is locked out when
/// the dictionary enforces it (`ENFORCE_MAX_FAILED_LOGINS`), as a server
/// does it (`tools/ace/events/lockout_*.txt`): the failure that makes the
/// limit is refused as the others (7078), the later ones with 7113, whatever
/// the password. Neither a good login nor an administrator enabling the user
/// again resets the count; unknown users have none; `ADSSYS` is locked out
/// too, with no way back but a restart.
macro_rules! lockouts {
    ($($name:ident: $file:literal),* $(,)?) => {
        mod lockout {
            use super::*;
            $(
                #[test]
                fn $name() {
                    fixtures::remotely_with(|| replay($file));
                }
            )*
        }
    };
}

lockouts!(
    off: "lockout_off.json",
    basic: "lockout_basic.json",
    good_login_between: "lockout_good.json",
    enabled_again: "lockout_enable.json",
    administrator: "lockout_adssys.json",
    administrator_and_another: "lockout_admin.json",
    unknown_users: "lockout_unknown.json",
    limit_one: "lockout_limit1.json",
    limit_zero: "lockout_limit0.json",
    enforcement_switched: "lockout_toggle.json",
    limit_lowered: "lockout_lower.json",
    limit_raised: "lockout_raise.json",
    open_connection: "lockout_open.json",
    disabled_by_an_administrator: "lockout_manual.json",
    disabled_for_the_database: "lockout_dbdisabled.json",
);

/// The server settings `Check_Free_Table_Access` and
/// `Disable_Free_Connections` (`tools/ace/events/freeconf_*.txt`, run in the
/// folder `freeconf_setup.txt` made, `fixtures/events/freeconf/`): the first
/// refuses free tables (7087) to every connection to a dictionary, ADSSYS's
/// and the views' and procedures' too, and not to free connections; the
/// second refuses free connections (7083), ADSSYS's too.
mod server_settings {
    use super::*;

    fn folder() -> PathBuf {
        fixtures::dir("../events/freeconf")
    }

    #[test]
    fn default() {
        fixtures::remotely_with_settings(false, false, || {
            replay_in("freeconf_default.json", false, Some(folder()))
        });
    }

    #[test]
    fn check_free_table_access() {
        fixtures::remotely_with_settings(true, false, || {
            replay_in("freeconf_check.json", false, Some(folder()))
        });
    }

    /// A procedure that reads a free table fails with 7087 too, but the end
    /// of tflip's text names where in its script (KNOWN_ISSUES).
    #[test]
    #[ignore = "the end of the text of a failed statement of a procedure's body"]
    fn check_free_table_access_in_a_procedure() {
        fixtures::remotely_with_settings(true, false, || {
            replay_in("freeconf_check_proc.json", false, Some(folder()))
        });
    }

    #[test]
    fn disable_free_connections() {
        fixtures::remotely_with_settings(false, true, || {
            replay_in("freeconf_disable.json", false, Some(folder()))
        });
    }
}
