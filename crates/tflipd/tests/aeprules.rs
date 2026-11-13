//! What an external procedure may do with the connection handle it is given
//! and with connections of its own, in `tflipd`: the library
//! `tools/ace/aep/aeprules.c`, built here with `cc` and linking the client
//! library `cargo` built (`libtflip_ace.so`), runs in the `tflipd` this test
//! starts, and calls its client's session through its own copy of that
//! library. The test, a client of that `tflipd` through the same client
//! library, replays what the original's remote server did with the same
//! library (`fixtures/sql/aeprulesremote.json`,
//! `tools/ace/aep/aeprulesremote.txt`).

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

/// Starts a `tflipd` serving `dir/data` as the share `data`, with real
/// transactions, and returns it with its address; sets the fingerprint the
/// client library trusts.
fn serve(dir: &Path) -> (Server, String) {
    // The library's own connections are local sessions of `tflipd`'s process:
    // they count as the setting of this `tflip.ini` says.
    let ini = dir.join("tflip.ini");
    std::fs::write(&ini, "[transactions]\nmode = real\n").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tflipd"))
        .env("TFLIP_CONFIG", &ini)
        .arg("--share")
        .arg(format!("data={}", dir.join("data").display()))
        .args(["--listen", "127.0.0.1:0", "--state"])
        .arg(dir.join("state"))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let server = Server(child);
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
    // SAFETY: before the client library reads it, and no other thread
    // reads the environment.
    unsafe { std::env::set_var("TFLIP_SERVER_FINGERPRINT", fingerprint.unwrap()) };
    (server, address.expect("tflipd's address"))
}

#[test]
fn rules_for_external_procedures_match_the_original_remote_server() {
    let dir = std::env::temp_dir().join(format!("tflipd-aeprules-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("data/ae")).unwrap();
    let ace_library = common::ace_library();
    if common::build_source(
        "aeprules.c",
        &dir.join("data/ae/aeprules.aep"),
        Some(&ace_library),
        &[],
    )
    .is_none()
    {
        return;
    }
    let (_server, address) = serve(&dir);
    let ace = common::Ace::load(&ace_library);
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sql/aeprulesremote.json");
    // The recording masked the path of the connection, which is the
    // original's; the rest is replayed.
    let mut steps: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    steps.retain(|step| step["cmd"] != "query EXECUTE PROCEDURE path_of()");
    let fixture = dir.join("aeprulesremote.json");
    std::fs::write(&fixture, serde_json::to_string(&steps).unwrap()).unwrap();
    let differ = common::replay(
        &ace,
        &fixture,
        &format!(r"\\{address}\data"),
        "ServerType=REMOTE",
        |folder, path| format!(r"{folder}\{path}"),
        |text| text.to_owned(),
    );
    assert!(differ.is_empty(), "{}", differ.join("\n"));
    let _ = std::fs::remove_dir_all(&dir);
}
