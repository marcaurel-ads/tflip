//! What the writes of a table leave in the source of a differential backup
//! (`fixtures/backup/s56_diff_writes` to `s59_diff_dd`): each scenario's
//! source folder after `PrepareDiff` is copied, the original's steps are
//! run on it through the exports, and the files must equal the original's
//! snapshots after each step.

use std::path::Path;

use super::fixtures::{self, remote_twins};
use super::writes::{compare, replay};

fn hex(text: &str) -> String {
    text.bytes().map(|byte| format!("{byte:02x}")).collect()
}

/// Bytes of a file in a snapshot that are not compared: the snapshot, the
/// file, the offset and the length.
type Mask = (&'static str, &'static str, usize, usize);

/// Runs `script` on a copy of the scenario's snapshot `start` (with the
/// dictionary's index and memo files of `src`, which the snapshots leave
/// out) and compares the files of each snapshot `snap_<name>` the script
/// takes with the original's. On the remote server (where a backup can be
/// made) the source is first given back the marks `PrepareDiff` cleared, the
/// script's `{img}` is the folder of an image that `PrepareDiff` makes before
/// it, and the folders `snapdir` takes are compared with the original's
/// folder of that name.
fn replay_scenario(
    scenario: &str,
    start: &str,
    test: &str,
    script: &str,
    admin: bool,
    masks: &[Mask],
) -> Snapshots {
    let tree = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/backup")
        .join(scenario)
        .join("tree");
    let from = tflip_storage::testing::temp_root().join(format!("tflip-diff-from-{test}"));
    let _ = std::fs::remove_dir_all(&from);
    std::fs::create_dir_all(&from).unwrap();
    for name in ["d.ai", "d.am"] {
        std::fs::copy(tree.join("src").join(name), from.join(name)).unwrap();
    }
    fixtures::copy_files(&tree.join(start), &from);
    let diff = fixtures::is_remote();
    let image = tflip_storage::testing::temp_root().join(format!("tflip-diff-image-{test}"));
    let _ = std::fs::remove_dir_all(&image);
    let mut script = script.to_owned();
    if diff {
        unprepare(&from);
        script =
            format!("sql EXECUTE PROCEDURE sp_BackupDatabase('{{img}}', 'PrepareDiff')\n{script}")
                .replace("{img}", &image.display().to_string());
    }
    let (source, work) = fixtures::scratch_from(&format!("diff-{test}"), &from);
    tflip_storage::undefined::take(&work);
    // The restructure and the procedures want the administrator, who only
    // connects to a dictionary in the process.
    let (source, script) = match (admin, fixtures::is_remote()) {
        (true, false) => (source, format!("ddconnect d.add ADSSYS -\n{script}")),
        (true, true) => (
            format!(r"{source}\d.add"),
            format!("reconnect ADSSYS -\n{script}"),
        ),
        (false, true) => (format!(r"{source}\d.add"), script),
        (false, false) => (format!("{source}/d.add"), script),
    };
    let (answers, snapshots) = replay(&source, &work, &script);
    for answer in &answers {
        let cmd = answer["cmd"].as_str().unwrap();
        // The original's answer to a goto to a deleted record.
        let allowed = [0, 5022];
        let rc = answer.get("rc").and_then(|rc| rc.as_u64()).unwrap_or(0);
        assert!(allowed.contains(&rc), "{cmd}: {rc} {answer}");
    }
    let mut problems = Vec::new();
    for (name, files) in &snapshots {
        let images = name.starts_with("img");
        let folder = match images {
            true => tree.join(name),
            false => tree.join(format!("snap_{name}")),
        };
        if !folder.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            let file = path.file_name().unwrap();
            // An image's index and memo of the dictionary are made by the
            // server and read by nobody.
            if images && matches!(path.extension().and_then(|e| e.to_str()), Some("ai" | "am")) {
                continue;
            }
            let Some((_, written)) = files.iter().find(|(file_name, _)| file_name == file) else {
                problems.push(format!("{name}: {} not there", file.to_string_lossy()));
                continue;
            };
            let original = std::fs::read(&path).unwrap();
            // The original's memo files in the snapshots lack what its server
            // held in memory (free runs and their counters), as in the write
            // scenarios' snapshots; an image's are written whole.
            if !images && path.extension().is_some_and(|e| e == "adm") {
                continue;
            }
            let mut masked: Vec<(usize, usize)> = masks
                .iter()
                .filter(|mask| mask.0 == name && file == mask.1)
                .map(|mask| (mask.2, mask.3))
                .collect();
            // The time the backup was made at.
            if images || diff {
                masked.push((0x174, 4));
            }
            if let Some(problem) = compare(&path, original, written.clone(), masked) {
                problems.push(format!("{name}: {}", problem.trim_start()));
            }
        }
    }
    assert!(problems.is_empty(), "{scenario}:\n{}", problems.join("\n"));
    snapshots
}

/// Gives the ADT files in `folder` the state they had before `PrepareDiff`
/// cleared them: every record marked changed, the header flag set and no stamp.
fn unprepare(folder: &Path) {
    for entry in std::fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        let name = path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_ascii_lowercase();
        if !(name.ends_with(".adt") || name == "d.add") {
            continue;
        }
        let mut bytes = std::fs::read(&path).unwrap();
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let slots = word(0x18);
        let record = usize::from(u16::from_le_bytes([bytes[0x24], bytes[0x25]]));
        let first = word(0x20) & 0xffff;
        for slot in 0..slots {
            bytes[first + slot * record] |= 0x04;
        }
        bytes[0x58] |= 0x10;
        bytes[0x174..0x178].fill(0);
        std::fs::write(&path, bytes).unwrap();
    }
}

/// What a script does on the remote server only: the `Diff`s of the original's
/// scenario and the snapshots of what they left.
fn on_remote(steps: &str) -> &str {
    match fixtures::is_remote() {
        true => steps,
        false => "",
    }
}

type Snapshots = Vec<(String, Vec<(std::ffi::OsString, Vec<u8>)>)>;

/// The header flags, free list (head, count), stamp and status bytes of the
/// ADT file `bytes`.
fn header(bytes: &[u8]) -> (u32, (u32, u32), u32, Vec<u8>) {
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let slots = word(0x18) as usize;
    let record = usize::from(u16::from_le_bytes([bytes[0x24], bytes[0x25]]));
    let first = word(0x20) as usize & 0xffff;
    let stats = (0..slots).map(|i| bytes[first + i * record]).collect();
    (word(0x58), (word(0x28), word(0x30)), word(0x174), stats)
}

fn file<'a>(snapshots: &'a Snapshots, name: &str, file: &str) -> &'a [u8] {
    let (_, files) = snapshots.iter().find(|(snap, _)| snap == name).unwrap();
    &files.iter().find(|(f, _)| f == file).unwrap().1
}

#[test]
fn writes() {
    let _settings = fixtures::settings_lock();
    let grown = "grown memo value that does not fit in the old place at all, ".repeat(5);
    let grown = grown.trim_end_matches([' ', ',']);
    let script = format!(
        "
sql UPDATE x_edit SET name = 'edited' WHERE id = 1
snapshot edit1
sql UPDATE x_edit SET name = 'edit2' WHERE id = 1
snapshot edit2
sql UPDATE x_edit SET name = 'edit3' WHERE id = 4
snapshot edit3
sql INSERT INTO x_app VALUES (6, 'n6', 'memo 6', NULL)
snapshot app
sql DELETE FROM x_del WHERE id = 3
snapshot del
showdeleted 1
open x_recall exclusive
goto 2
recall
close
snapshot recall
sql UPDATE x_memo SET note = '{grown}' WHERE id = 2
snapshot memo
open x_part exclusive
goto 3
setbin bin 6 20 0 {}
close
snapshot part1
open x_part exclusive
goto 3
setbin bin 6 20 10 {}
close
snapshot part2
snapshot all
",
        hex("0123456789"),
        hex("abcdefghij"),
    );
    let diff = "sql EXECUTE PROCEDURE sp_BackupDatabase('{img}', 'Diff')
snapshot after
snapdir img {img}
";
    let script = format!("{script}{}", on_remote(diff));
    replay_scenario("s56_diff_writes", "snap0", "writes", &script, false, &[]);
    crate::AdsShowDeleted(crate::ADS_TRUE);
}

#[test]
fn transactions() {
    let _settings = fixtures::settings_shared();
    let script = "sql BEGIN TRANSACTION
sql UPDATE x_commit SET name = 'txedit' WHERE id = 1
sql INSERT INTO x_commit VALUES (6, 'n6', 'memo 6', NULL)
sql DELETE FROM x_commit WHERE id = 2
snapshot commit_pending
sql COMMIT
snapshot commit_done
sql BEGIN TRANSACTION
sql UPDATE x_rb SET name = 'txedit', note = 'rolled back memo that is rather long to need a new block, rolled back memo that is rather long to need a new block, rolled back memo that is rather long to need a new block' WHERE id = 1
sql INSERT INTO x_rb VALUES (6, 'n6', 'memo 6', NULL)
sql DELETE FROM x_rb WHERE id = 2
snapshot rb_pending
sql ROLLBACK
snapshot rb_done
sql BEGIN TRANSACTION
sql UPDATE x_pend SET name = 'txedit' WHERE id = 1
sql INSERT INTO x_pend VALUES (6, 'n6', 'memo 6', NULL)
sql DELETE FROM x_pend WHERE id = 2
snapshot pend_pending
";
    // The `Diff`s are the original's, the first with the transaction still
    // pending.
    let during = "sql EXECUTE PROCEDURE sp_BackupDatabase('{img}', 'Diff')
snapshot diff_during
snapdir img_during {img}
";
    let after = "sql EXECUTE PROCEDURE sp_BackupDatabase('{img}', 'Diff')
snapshot diff_after
snapdir img_after {img}
";
    let script = format!(
        "{script}{}sql COMMIT\nsnapshot pend_done\n{}",
        on_remote(during),
        on_remote(after)
    );
    // Without the `Diff`, which gave the table its stamp, the original's
    // snapshot differs in it.
    let masks = match fixtures::is_remote() {
        true => vec![],
        false => vec![("pend_done", "x_pend.adt", 0x174, 4)],
    };
    replay_scenario(
        "s57_diff_tx",
        "snap0",
        "transactions",
        &script,
        false,
        &masks,
    );
}

#[test]
fn rewrites() {
    let _settings = fixtures::settings_shared();
    let script = "sql UPDATE x_pack SET name = 'edit' WHERE id = 1
sql DELETE FROM x_pack WHERE id = 2
snapshot pack_before
sql EXECUTE PROCEDURE sp_PackTable('x_pack')
snapshot pack
sql EXECUTE PROCEDURE sp_ZapTable('x_zap')
snapshot zap
sql ALTER TABLE x_alter ADD COLUMN extra CHAR(4)
snapshot alter
sql EXECUTE PROCEDURE sp_EncryptTable('x_enc')
snapshot enc
sql UPDATE x_reidx SET name = 'chg' WHERE id = 3
open x_reidx exclusive
reindex
close
snapshot reidx
";
    let diff = "sql EXECUTE PROCEDURE sp_BackupDatabase('{img}', 'Diff')
snapshot diff
snapdir img_diff {img}
sql EXECUTE PROCEDURE sp_BackupDatabase('{img}', 'Diff')
snapdir img_diff2 {img}
";
    let script = format!("{script}{}", on_remote(diff));
    replay_scenario(
        "s58_diff_reinit_hdr",
        "snap0",
        "rewrites",
        &script,
        true,
        &[],
    );
}

#[test]
fn dictionary() {
    let _settings = fixtures::settings_shared();
    let script = "sql CREATE VIEW v_one AS SELECT id, name FROM x_one
snapshot view
sql EXECUTE PROCEDURE sp_CreateUser('bkuser', 'pw', 'user')
snapshot user
sql EXECUTE PROCEDURE sp_ModifyUserProperty('bkuser', 'USER_PASSWORD', 'pw2')
snapshot userprop
sql EXECUTE PROCEDURE sp_ModifyTableProperty('x_one', 'COMMENT', 'cmt', 'NO_VALIDATE', NULL)
snapshot tblprop
";
    let diff = "sql EXECUTE PROCEDURE sp_BackupDatabase('{img}', 'Diff')
snapshot diff
snapdir img_diff {img}
sql EXECUTE PROCEDURE sp_BackupDatabase('{img}', 'Diff')
snapshot diff2
sql DROP VIEW v_one
sql UPDATE x_one SET name = 'edit' WHERE id = 1
sql EXECUTE PROCEDURE sp_BackupDatabase('{img}', 'Diff')
snapshot diff3
snapdir img_diff3 {img}
";
    let script = format!("{script}{}", on_remote(diff));
    // The rows' bytes the original makes at random: a user's secret (slot 17
    // from byte 223), and the one in the database's row (slot 0), which it
    // makes again when a user's password changes and tflip does not. Slot 1's
    // comment block is where the memo file the snapshots leave out has room.
    let user = |snap| (snap, "d.add", 2200 + 17 * 524 + 223, 524 - 223);
    let database = |snap| (snap, "d.add", 2200, 524);
    let comment = |snap| (snap, "d.add", 2200 + 524 + 515, 4);
    let mut masks = vec![
        user("user"),
        user("userprop"),
        user("tblprop"),
        database("userprop"),
        database("tblprop"),
        comment("tblprop"),
    ];
    for snap in ["diff", "diff2", "diff3", "img_diff", "img_diff3"] {
        masks.extend([user(snap), database(snap), comment(snap)]);
    }
    replay_scenario("s59_diff_dd", "snap0", "dictionary", &script, true, &masks);
}

/// A dropped object's row: deleted and changed, on the free list. The
/// original's snapshot of it is after a `Diff`, which cleared the bit.
#[test]
fn dropped_view() {
    let _settings = fixtures::settings_shared();
    let script = "sql DROP VIEW v_one
snapshot dropped
";
    let snapshots = replay_scenario(
        "s59_diff_dd",
        "snap_diff2",
        "dropped-view",
        script,
        true,
        &[],
    );
    let (flags, free, stamp, stats) = header(file(&snapshots, "dropped", "d.add"));
    assert_eq!((flags, free, stamp), (0x13, (17, 1), 0x6aedee21));
    assert_eq!(stats[16], 0x05);
    assert!(stats.iter().enumerate().all(|(i, &s)| i == 16 || s == 0));
}

/// A table made after `PrepareDiff` (`s59b_diff_newtable`): flag 0x10 set,
/// no stamp and every record changed. The dictionary it joins is that of
/// `s59`, so the object number in its header, which it holds, is not compared.
#[test]
fn new_table() {
    let _settings = fixtures::settings_shared();
    let mut script =
        "sql CREATE TABLE x_new (id INTEGER, name CHAR(10), note MEMO, bin BLOB)\n".to_owned();
    for id in 1..=2 {
        script += &format!("sql INSERT INTO x_new VALUES ({id}, 'n{id}', 'memo {id}', NULL)\n");
    }
    script += "snapshot new\n";
    script += on_remote(
        "sql EXECUTE PROCEDURE sp_BackupDatabase('{img}', 'Diff')
snapdir imgb_diff {img}
sql UPDATE x_new SET name = 'edit' WHERE id = 1
sql EXECUTE PROCEDURE sp_BackupDatabase('{img}', 'Diff')
snapdir imgb_diff2 {img}
",
    );
    let snapshots = replay_scenario("s59_diff_dd", "snap0", "new-table", &script, true, &[]);
    let written = file(&snapshots, "new", "x_new.adt");
    let tree =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/backup/s59b_diff_newtable");
    let original = std::fs::read(tree.join("tree/snap_before/x_new.adt")).unwrap();
    let path = Path::new("x_new.adt");
    let problem = compare(path, original, written.to_vec(), vec![(0x160, 4)]);
    assert!(problem.is_none(), "{problem:?}");
    let (flags, _, stamp, stats) = header(written);
    assert_eq!((flags, stamp, stats), (0x11, 0, vec![4, 4]));
    if !fixtures::is_remote() {
        return;
    }
    // The image the first `Diff` writes for it, and the second's, in which
    // an edit is copied.
    for (name, original) in [("imgb_diff", "img_diff"), ("imgb_diff2", "img_diff2")] {
        for table in ["x_new.adt", "x_new.adm"] {
            let want = std::fs::read(tree.join("tree").join(original).join(table)).unwrap();
            let got = file(&snapshots, name, table).to_vec();
            let problem = compare(Path::new(table), want, got, vec![(0x174, 4)]);
            assert!(problem.is_none(), "{name}: {problem:?}");
        }
    }
}

remote_twins!(writes, transactions, rewrites, dictionary, new_table);
