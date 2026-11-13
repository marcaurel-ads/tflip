//! Triggers that are functions of a library in `tflipd`, end to end: the
//! library `tools/ace/aep/aeptrig.c`, built here with `cc` and linking the
//! client library `cargo` built (`libtflip_ace.so`), runs in the `tflipd`
//! this test starts, and calls its client's session through its own copy of
//! that library. The test, a client of that `tflipd` through the same client
//! library, replays what the original's server did with the same library
//! (`fixtures/sql/aeptrig.json`, `tools/ace/aep/`).

#![cfg(target_os = "linux")]

#[path = "../../tflip-ace/tests/common/mod.rs"]
mod common;

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};

/// Stops the `tflipd` when the test ends, however it ends.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn trigger_libraries_match_the_original_server() {
    let dir = std::env::temp_dir().join(format!("tflipd-aeptrig-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let data = dir.join("data");
    std::fs::create_dir_all(data.join("ae")).unwrap();
    let ace_library = common::ace_library();
    let built = |library: &str, defines: &[&str]| {
        common::build_source(
            "aeptrig.c",
            &data.join("ae").join(library),
            Some(&ace_library),
            defines,
        )
    };
    if built("aeptrig.aep", &[]).is_none() || built("plain_aeptrig.aep", &["PLAIN"]).is_none() {
        return;
    }

    let mut child = Command::new(env!("CARGO_BIN_EXE_tflipd"))
        .arg("--share")
        .arg(format!("data={}", data.display()))
        .args(["--listen", "127.0.0.1:0", "--state"])
        .arg(dir.join("state"))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let _server = Server(child);
    let (mut address, mut fingerprint) = (None, None);
    let mut lines = BufReader::new(stdout).lines();
    for line in lines.by_ref() {
        let line = line.unwrap();
        if let Some((_, at)) = line.split_once(" listening on ") {
            address = Some(at.to_owned());
        }
        if let Some(print) = line.strip_prefix("certificate fingerprint: ") {
            fingerprint = Some(print.to_owned());
            break;
        }
    }
    // What it prints later is read, so that it does not fail to print.
    std::thread::spawn(move || lines.for_each(drop));
    let address = address.expect("tflipd's address");
    // SAFETY: before the client library reads it, and no other thread
    // reads the environment.
    unsafe { std::env::set_var("TFLIP_SERVER_FINGERPRINT", fingerprint.unwrap()) };

    let ace = common::Ace::load(&ace_library);
    let share = format!(r"\\{address}\data");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sql/aeptrig.json");
    let local = format!("{}/ae/", data.display());
    let differ = common::replay(
        &ace,
        &fixture,
        &share,
        "ServerType=REMOTE",
        |folder, path| format!(r"{folder}\{path}"),
        |text| text.replace(&local, r"C:\ads\data\aeptrig\data\ae\"),
    );
    assert!(differ.is_empty(), "{}", differ.join("\n"));
    let _ = std::fs::remove_dir_all(&dir);
}
