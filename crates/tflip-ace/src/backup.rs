//! Replays `fixtures/backup/*/result.json`: the online backup and restore
//! procedures on the remote server (`tools/ace/backup/`, recorded on the
//! original's server), through `tflipd` as a client does, and compares the
//! files the scenario leaves with the original's `tree/`.

use std::collections::HashMap;
use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value as Json, json};

use crate::fixtures;
use crate::{
    AdsAtEOF, AdsCloseSQLStatement, AdsCloseTable, AdsCreateSQLStatement, AdsDisconnect,
    AdsExecuteSQLDirect, AdsGetField, AdsGetFieldLength, AdsGetFieldName, AdsGetFieldType,
    AdsGetLastError, AdsGetNumFields, AdsGotoTop, AdsHandle, AdsIsNull, AdsSkip,
    AdsStmtSetTableType, Unsigned8,
};

fn raw(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

fn last_error() -> String {
    let (mut code, mut buf, mut len) = (0, [0u8; 1200], 1200u16);
    unsafe { AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
    raw(&buf[..usize::from(len).min(buf.len())])
}

/// Runs `sql` as the probe's `sql` command does, with the table type
/// `table_type` set on the statement if it is not 0.
fn run(connection: AdsHandle, sql: &str, table_type: u16) -> Json {
    let mut object = Map::new();
    let (mut statement, mut cursor) = (0, 0);
    let text = CString::new(crate::ansi::from_str_lossy(sql)).unwrap();
    let rc = unsafe {
        let mut rc = AdsCreateSQLStatement(connection, &mut statement);
        if rc == 0 && table_type != 0 {
            rc = AdsStmtSetTableType(statement, table_type);
        }
        match rc {
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
                let mut row = Vec::new();
                for f in 1..=fields {
                    let field = usize::from(f) as *mut Unsigned8;
                    let mut null = 0;
                    AdsIsNull(cursor, field, &mut null);
                    if null != 0 {
                        row.push(Json::Null);
                        continue;
                    }
                    let (mut buf, mut len) = (vec![0u8; 70_000], 70_000u32);
                    row.push(
                        match AdsGetField(cursor, field, buf.as_mut_ptr(), &mut len, 0) {
                            0 => json!(raw(&buf[..len as usize])),
                            rc => json!(format!("E{rc}")),
                        },
                    );
                }
                rows.push(Json::Array(row));
                AdsSkip(cursor, 1);
            }
            object.insert("rows".into(), json!(rows));
        }
    }
    if cursor != 0 {
        AdsCloseTable(cursor);
    }
    if statement != 0 {
        AdsCloseSQLStatement(statement);
    }
    Json::Object(object)
}

/// `value` with the folder the original's server ran the scenario in
/// replaced by `root`, and the timing left out.
fn localized(value: &mut Json, original: &str, root: &str) {
    match value {
        Json::String(s) => *s = s.replace(original, root),
        Json::Array(items) => items
            .iter_mut()
            .for_each(|item| localized(item, original, root)),
        Json::Object(map) => {
            map.remove("ms");
            map.values_mut()
                .for_each(|item| localized(item, original, root));
        }
        _ => {}
    }
}

/// What a scenario's files are compared by.
#[derive(Default)]
pub(crate) struct Replay<'a> {
    /// The folders of `tree/` that are not compared (the source's, which the
    /// scenario makes with tflip, and what depends on the time).
    pub skip: &'a [&'a str],
    /// The statements (by their numbers, from 1) whose result is not
    /// compared.
    pub ignore_steps: &'a [usize],
    /// The folder the original's local server ran the scenario in, for a
    /// scenario recorded on it.
    pub local_original: Option<&'a str>,
    /// Folders the original's scenario ends with empty, which `tree/` cannot
    /// hold: the replay must leave them there, empty.
    pub empty: &'a [&'a str],
}

/// `relative` in the folder `source`, as the connection of the replay names it.
fn below(source: &str, relative: &str) -> String {
    match fixtures::is_remote() {
        true => format!(r"{source}\{relative}"),
        false => format!("{source}/{}", relative.replace('\\', "/")),
    }
}

fn files_of(folder: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    if let Ok(entries) = fs::read_dir(folder) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_file()) {
                found.push((
                    entry.file_name().to_string_lossy().into_owned(),
                    entry.path(),
                ));
            }
        }
    }
    found.sort();
    found
}

fn is_dictionary_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".add", ".am", ".ai"].iter().any(|e| lower.ends_with(e))
}

/// Replays the scenario `scenario`, and compares the folders it leaves with
/// the original's.
pub(crate) fn replay(scenario: &str, replay: &Replay) {
    let _settings = fixtures::settings_shared();
    let folder = fixtures::dir(&format!("../backup/{scenario}"));
    let steps: Vec<Json> =
        serde_json::from_slice(&fs::read(folder.join("result.json")).unwrap()).unwrap();
    let (source, dir) =
        fixtures::scratch_from(&format!("backup-{scenario}"), &fixtures::empty_dir());
    let original = match replay.local_original {
        Some(original) => original.to_owned(),
        None => format!(r"C:\ads\data\bk\{scenario}"),
    };
    let root = dir.display().to_string();
    for step in &steps {
        let cmd = step["cmd"].as_str().unwrap();
        if let Some((_, rest)) = cmd.split_once("CREATE DATABASE \"")
            && let Some((name, _)) = rest.split_once('\\')
        {
            fs::create_dir_all(dir.join(name)).unwrap();
        }
    }
    let mut connections: HashMap<usize, AdsHandle> = HashMap::new();
    // The original's server keeps the table type a connection's last
    // statement was given for the statements that follow with none: a table
    // made after the Clipper ones is a Clipper table (`s03_dd_objects`).
    let mut table_type = 0u16;
    let mut last_type: HashMap<usize, u16> = HashMap::new();
    let mut tables: HashMap<usize, AdsHandle> = HashMap::new();
    // What `connopt` adds to the connect strings of the connections that
    // follow.
    let mut connect_extra = String::new();
    let mut failures = Vec::new();
    for (n, step) in steps.iter().enumerate() {
        let cmd = step["cmd"].as_str().unwrap().replace(&original, &root);
        let (verb, rest) = cmd.split_once(' ').unwrap_or((&cmd, ""));
        let number = |text: &str| text.parse::<usize>().unwrap();
        let mut got = Map::new();
        got.insert("cmd".into(), json!(cmd));
        match verb {
            "free" | "connect" => {
                let mut words = rest.split(' ');
                let id = number(words.next().unwrap());
                let target = match verb {
                    "free" => match words.next() {
                        Some(sub) => below(&source, sub),
                        None => source.clone(),
                    },
                    _ => below(&source, words.next().unwrap()),
                };
                let (user, password) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
                let user = if verb == "connect" && user.is_empty() {
                    "ADSSYS"
                } else {
                    user
                };
                let (rc, handle) = match connect_extra.is_empty() {
                    true => fixtures::connect_to_as(&target, user, password),
                    false => {
                        let server = if fixtures::is_remote() {
                            "REMOTE"
                        } else {
                            "LOCAL"
                        };
                        let user = if verb == "free" { "" } else { user };
                        let mut string = format!(
                            "Data Source={target};ServerType={server};{connect_extra}\
                             User ID={user};Password={password}\0"
                        )
                        .into_bytes();
                        let mut handle = 0;
                        let rc = unsafe {
                            crate::AdsConnect101(
                                string.as_mut_ptr(),
                                std::ptr::null_mut(),
                                &mut handle,
                            )
                        };
                        (rc, handle)
                    }
                };
                got.insert("rc".into(), json!(rc));
                if rc == 0 {
                    connections.insert(id, handle);
                }
            }
            "sql" => {
                let (id, sql) = rest.split_once(' ').unwrap();
                let connection = connections[&number(id)];
                if table_type != 0 {
                    last_type.insert(number(id), table_type);
                }
                let given = last_type.get(&number(id)).copied().unwrap_or(0);
                if let Json::Object(result) = run(connection, sql, given) {
                    got.extend(result);
                }
            }
            "open" => {
                let mut words = rest.split(' ');
                let id = number(words.next().unwrap());
                // The recorded path names the original's server; the replay's
                // is the scratch folder's.
                let recorded = format!(r"\\127.0.0.1:6262\data\bk\{scenario}");
                let name = words.next().unwrap().replace(&recorded, &source);
                let name = CString::new(name).unwrap();
                let mode = words.next().unwrap();
                let table_type: u16 = words.next().map_or(0, |t| t.parse().unwrap());
                let mut handle = 0;
                let options = if mode == "x" {
                    crate::ADS_EXCLUSIVE
                } else {
                    crate::ADS_SHARED
                };
                let rc = unsafe {
                    crate::AdsOpenTable(
                        connections[&id],
                        name.as_ptr().cast_mut().cast(),
                        std::ptr::null_mut(),
                        table_type,
                        1,
                        1,
                        1,
                        options,
                        &mut handle,
                    )
                };
                got.insert("rc".into(), json!(rc));
                if rc != 0 {
                    got.insert("error".into(), json!(last_error()));
                } else {
                    tables.insert(id, handle);
                }
            }
            "closetable" => {
                let handle = tables.remove(&number(rest)).unwrap();
                got.insert("rc".into(), json!(AdsCloseTable(handle)));
            }
            "connopt" => {
                connect_extra = rest.to_owned();
                got.insert("connopt".into(), json!(true));
            }
            "ttype" => {
                table_type = rest.parse().unwrap();
                got.insert("ttype".into(), json!(table_type));
            }
            // Makes the folder the original's script made by hand.
            "shell" => {
                if let Some(path) = rest.strip_prefix("mkdir ") {
                    // The recorded path is that of the machine that recorded
                    // it, below the scenario's folder.
                    let recorded = format!(r"\bk\{scenario}");
                    let path = match path.split_once(&recorded) {
                        Some((_, below)) => format!("{root}{below}"),
                        None => path.to_owned(),
                    };
                    fs::create_dir_all(path.replace('\\', "/")).unwrap();
                }
                // The original copies a folder to keep what it held then.
                if let Some(paths) = rest.strip_prefix("xcopy ") {
                    let mut words = paths.split(' ');
                    let marker = format!("\\bk\\{scenario}\\");
                    let local = |path: Option<&str>| {
                        let (_, relative) = path.unwrap().rsplit_once(&marker).unwrap();
                        dir.join(relative.trim_end_matches('\\').replace('\\', "/"))
                    };
                    let (from, to) = (local(words.next()), local(words.next()));
                    fs::create_dir_all(&to).unwrap();
                    for (name, path) in files_of(&from) {
                        fs::copy(path, to.join(name)).unwrap();
                    }
                }
                got.insert("exit".into(), json!(0));
            }
            "disconnect" => {
                let handle = connections.remove(&number(rest)).unwrap();
                got.insert("rc".into(), json!(AdsDisconnect(handle)));
            }
            other => panic!("unknown command {other}"),
        }
        let mut want = step.clone();
        localized(&mut want, &original, &root);
        let mut got = Json::Object(got);
        localized(&mut got, &original, &root);
        if got != want && !replay.ignore_steps.contains(&(n + 1)) {
            failures.push(format!("step {}: {want}\n  tflip: {got}", n + 1));
        }
    }
    for (_, handle) in connections {
        AdsDisconnect(handle);
    }
    failures.extend(compare_trees(&folder.join("tree"), &dir, replay));
    for name in replay.empty {
        let empty = fs::read_dir(dir.join(name)).is_ok_and(|mut entries| entries.next().is_none());
        if !empty {
            failures.push(format!("{name}: not an empty folder, as the original's is"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} differences from the original's:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Compares two archives (`None` if the original's is not one): the members
/// by name and bytes, in order, and the kind of file. The time every member
/// carries is the run's, and the dictionary's files are tflip's own (see
/// [`compare_trees`]), so those are compared by size.
fn same_archive(want: &Path, got: &Path) -> Option<Result<(), String>> {
    use tflip_storage::archive::read_archive;
    let (want_bytes, got_bytes) = (fs::read(want).ok()?, fs::read(got).ok()?);
    let gzip = |bytes: &[u8]| bytes.starts_with(&[0x1f, 0x8b]);
    let wanted = read_archive(want, true)
        .ok()
        .filter(|members| !members.is_empty())?;
    Some((|| {
        if gzip(&want_bytes) != gzip(&got_bytes) {
            return Err("compressed and plain differ".to_owned());
        }
        let ours = read_archive(got, true).map_err(|error| format!("unreadable: {error}"))?;
        let names = |members: &[tflip_storage::archive::Member]| {
            members.iter().map(|m| m.name.clone()).collect::<Vec<_>>()
        };
        if names(&wanted) != names(&ours) {
            return Err(format!(
                "members {:?}, the original's {:?}",
                names(&ours),
                names(&wanted)
            ));
        }
        if ours.iter().any(|m| m.mtime != ours[0].mtime) {
            return Err("the members have different times".to_owned());
        }
        for (want, got) in wanted.iter().zip(&ours) {
            let mut want_data = want.data.clone();
            tflip_engine::clock::adopt_todays_dbf_date(&want.name, &got.data, &mut want_data);
            let same = match is_dictionary_file(&want.name) {
                true => want_data.len() == got.data.len(),
                false => want_data == got.data,
            };
            if !same {
                return Err(format!("member {} differs", want.name));
            }
        }
        Ok(())
    })())
}

/// The files of the original's `tree` that `dir` has not, or has otherwise.
/// The dictionary's own files are the source's, which tflip made: they are
/// compared with the source's of `dir`, the tables' bytes with the original's.
fn compare_trees(tree: &Path, dir: &Path, replay: &Replay) -> Vec<String> {
    let mut failures = Vec::new();
    let mut folders: Vec<_> = fs::read_dir(tree)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    folders.sort();
    let ours = |folder: &str| dir.join(folder);
    for folder in folders
        .iter()
        .filter(|f| !replay.skip.contains(&f.as_str()))
    {
        let want = files_of(&tree.join(folder));
        let got = files_of(&ours(folder));
        let names =
            |files: &[(String, PathBuf)]| files.iter().map(|f| f.0.clone()).collect::<Vec<_>>();
        if names(&want) != names(&got) {
            failures.push(format!(
                "{folder}: files {:?}, the original's {:?}",
                names(&got),
                names(&want)
            ));
            continue;
        }
        for ((name, want_path), (_, got_path)) in want.iter().zip(&got) {
            let got_bytes = fs::read(got_path).unwrap();
            // A dictionary's files are the source's, which tflip made as it
            // made it (the objects get their own random numbers): only their
            // sizes are the original's.
            if is_dictionary_file(name) {
                let size = fs::metadata(want_path).unwrap().len();
                if size != got_bytes.len() as u64 {
                    failures.push(format!(
                        "{folder}/{name}: {} bytes (the original's {size})",
                        got_bytes.len()
                    ));
                }
                continue;
            }
            if let Some(result) = same_archive(want_path, got_path) {
                if let Err(difference) = result {
                    failures.push(format!("{folder}/{name}: {difference}"));
                }
                continue;
            }
            let mut want_bytes = fs::read(want_path).unwrap();
            // A differential backup's stamp is the time it was made at.
            let stamp = 0x174..0x178;
            if name.ends_with(".adt")
                && want_bytes.len() > stamp.end
                && got_bytes.len() > stamp.end
                && want_bytes[stamp.clone()] != [0; 4]
                && got_bytes[stamp.clone()] != [0; 4]
            {
                want_bytes[stamp.clone()].copy_from_slice(&got_bytes[stamp]);
            }
            // A DBF header carries the date it was last written on.
            tflip_engine::clock::adopt_todays_dbf_date(name, &got_bytes, &mut want_bytes);
            if want_bytes != got_bytes {
                let first = want_bytes.iter().zip(&got_bytes).position(|(a, b)| a != b);
                failures.push(format!(
                    "{folder}/{name}: {} bytes (the original's {}), first difference at {first:?}",
                    got_bytes.len(),
                    want_bytes.len()
                ));
            }
        }
    }
    failures
}

/// Declares a test replaying each scenario on the remote server: the folders
/// of its `tree/` that are not compared (`skip`; the source's always) and the
/// statements whose result is not (`ignore`, from 1).
macro_rules! scenarios {
    ($($name:ident: $scenario:literal $(, skip $skip:expr)? $(, ignore $ignore:expr)?;)*) => {
        $(
            #[test]
            fn $name() {
                fixtures::remotely_with(|| {
                    #[allow(unused_mut)]
                    let mut skip: Vec<&str> = vec!["src"];
                    $(skip.extend($skip);)?
                    #[allow(unused_mut)]
                    let mut ignore: Vec<usize> = Vec::new();
                    $(ignore.extend($ignore);)?
                    replay($scenario, &Replay { skip: &skip, ignore_steps: &ignore, ..Replay::default() });
                });
            }
        )*
    };
}

scenarios! {
    full_backup: "s01_full";
    restore_into_an_empty_folder: "s02_restore_empty";
    restore_over_a_dictionary: "s03_restore_over";
    restore_dictionary_objects: "s03_dd_objects";
    // The original makes the image's index when a connection is made to it.
    open_an_image: "s04_open_image", skip ["img"];
    restore_password: "s05_restore_pw";
    restore_password_not_needed: "s05b_restore_pw_unneeded";
    restore_as_a_user: "s06_restore_user";
    restore_to_another_name: "s07_restore_renamed";
    // A free table missing in a dictionary's folder is 7041 with its server path
    // in the original, 5004 in tflip.
    restore_extra_tables: "s08_restore_extra", ignore [91];
    dont_overwrite: "s09_dontoverwrite";
    no_warnings: "s10_nowarnings";
    include_and_exclude: "s11_include_exclude";
    meta_only: "s12_metaonly";
    option_matrix: "s22_option_matrix";
    encrypted_tables: "s60_dd_encrypted";
    differential: "s50_diff", skip ["srcA", "srcB", "srcC"];
    differential_without_prepare: "s51_diff_noprep";
    differential_reinitialised: "s52_diff_reinit";
    differential_restore: "s53_diff_restore";
    differential_dbf: "s54_diff_dbf";
    differential_memo_slots: "s92_diff_memo_slot";
    differential_memo_passes: "s93_diff_memo_passes";
    differential_memo_multi: "s94_diff_memo_multi";
    differential_memo_sizes: "s95_diff_memo_size";
    differential_memo_more: "s96_diff_memo_more";
    differential_memo_first: "s97_diff_memo_first";
    uncommitted_transaction: "s81_uncommitted_tx";
    exclusive_table: "s82_exclusive";
    exclusive_table_in_restore: "s84_exclusive_restore";
    archives: "s70_archive";
    archive_names_too_long: "s71_archive_long";
    backup_errors: "s20_backup_errors";
    restore_errors: "s21_restore_errors";
}

/// The free-table scenarios: their source folder `fsrc` is made with tflip,
/// which stamps its tables with other times than the original did, so it is
/// compared only where the scenario restores into it.
fn free_scenario(scenario: &str, skip: &[&str], ignore: &[usize], empty: &[&str]) {
    fixtures::remotely_with(|| {
        let mut skipped = vec!["src"];
        skipped.extend(skip);
        replay(
            scenario,
            &Replay {
                skip: &skipped,
                ignore_steps: ignore,
                empty,
                ..Replay::default()
            },
        );
    });
}

// Step 112 connects to a dictionary that is not there, which the original's
// server words with its own path.
#[test]
fn free_table_backup() {
    free_scenario("s40_free_backup", &["fsrc"], &[112], &[]);
}

#[test]
fn free_table_restore() {
    free_scenario("s41_free_restore", &[], &[], &["dst7", "dst8", "dst10"]);
}

#[test]
fn free_table_passwords() {
    free_scenario("s61_free_encrypted", &["fsrc"], &[], &[]);
}

#[test]
fn free_table_archives() {
    free_scenario("s72_archive_free", &["fsrc"], &[], &[]);
}

#[test]
fn free_table_held_exclusively() {
    free_scenario("s83_exclusive_free", &["fsrc"], &[], &[]);
}

#[test]
fn free_table_option_matrix() {
    free_scenario(
        "s23_option_matrix_free",
        &["fsrc"],
        &[],
        &["qf6", "qf15", "qf17", "qf18"],
    );
}

#[test]
fn free_table_differential() {
    free_scenario("s55_free_diff", &["fsrc"], &[], &[]);
}

/// The folder the original's local server ran `s30_local_local` in.
const LOCAL_ORIGINAL: &str =
    r"Z:\home\tflip\work\ads\build\adsserver\pfx\drive_c\ads\data\bk_local\s30_local";

/// The original's local server answers every call with 5042.
#[test]
fn local_server_refuses() {
    replay(
        "s30_local_local",
        &Replay {
            skip: &["src", "fsrc"],
            local_original: Some(LOCAL_ORIGINAL),
            ..Replay::default()
        },
    );
}

/// A backup that is under way (held by the engine's test hook) has taken the
/// dictionary file and its tables: a second backup gets the 7131 rows, an
/// open of a table is 7131, and everything is free again when the first is
/// over. The original's own recording (`s85_concurrent`) depends on timing.
#[test]
fn a_backup_under_way_holds_its_tables() {
    fixtures::remotely_with(|| {
        let _settings = fixtures::settings_shared();
        let (source, dir) =
            fixtures::scratch_from("backup-concurrent-held", &fixtures::empty_dir());
        fs::create_dir_all(dir.join("src")).unwrap();
        let root = dir.display().to_string();
        let (rc, free) = fixtures::connect_to_as(&source, "", "");
        assert_eq!(rc, 0);
        run(free, r#"CREATE DATABASE "src\d.add""#, 0);
        let dictionary = below(&source, r"src\d.add");
        let connect = move || {
            let (rc, handle) = fixtures::connect_to_as(&dictionary, "ADSSYS", "");
            assert_eq!(rc, 0);
            handle
        };
        let main = connect.clone()();
        for sql in [
            "CREATE TABLE t_one (id INTEGER)",
            "CREATE TABLE t_two (id INTEGER)",
        ] {
            assert_eq!(run(main, sql, 0)["rc"], 0);
        }
        let backup = move |connection: AdsHandle, image: &str| {
            let sql = format!("EXECUTE PROCEDURE sp_BackupDatabase('{root}/{image}', NULL)");
            run(connection, &sql, 0)
        };
        let open = |name: &str| {
            let name = CString::new(name).unwrap();
            let mut handle = 0;
            let rc = unsafe {
                crate::AdsOpenTable(
                    main,
                    name.as_ptr().cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    1,
                    1,
                    1,
                    crate::ADS_SHARED,
                    &mut handle,
                )
            };
            if rc == 0 {
                AdsCloseTable(handle);
            }
            rc
        };
        let (started, wait) = std::sync::mpsc::channel();
        let (release, held) = std::sync::mpsc::channel::<()>();
        tflip_engine::pause_next_backup(&dir.join("src/d.add"), move || {
            started.send(()).unwrap();
            let _ = held.recv();
        });
        let first = std::thread::spawn({
            let backup = backup.clone();
            // Whether a test is remote belongs to its thread.
            move || {
                let mut result = Json::Null;
                fixtures::remotely_with(|| {
                    let connection = connect();
                    result = backup(connection, "img1");
                    AdsDisconnect(connection);
                });
                result
            }
        });
        wait.recv_timeout(std::time::Duration::from_secs(30))
            .expect("the first backup is under way");

        let second = backup(main, "img2");
        assert_eq!(second["rc"], 0, "{second}");
        let rows: Vec<(&str, &str, &str, &str)> = second["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                let text = |at: usize| row[at].as_str().unwrap();
                (text(0), text(1), text(3), text(6))
            })
            .collect();
        let add = dir
            .join("src/d.add")
            .display()
            .to_string()
            .replace('/', "\\");
        assert_eq!(
            rows,
            vec![
                ("10", "7131", "t_one", "8009"),
                ("10", "7131", "t_two", "8009"),
                ("7", "7131", add.as_str(), "8048"),
            ]
        );
        assert_eq!(open("t_one"), 7131);
        assert!(!dir.join("img2/t_one.adt").exists());

        release.send(()).unwrap();
        let first = first.join().unwrap();
        assert_eq!(first["rows"], json!([]), "{first}");
        assert!(dir.join("img1/t_one.adt").exists());
        assert_eq!(open("t_one"), 0);
        assert_eq!(backup(main, "img3")["rows"], json!([]));
        AdsDisconnect(main);
        AdsDisconnect(free);
    });
}
