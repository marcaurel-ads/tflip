//! External procedures in the local server, end to end: the library
//! `tools/ace/aep/aep.c`, built here with `cc`, binds to the client library
//! `cargo` built (`libtflip_ace.so`), which this test loads as a client
//! would and through which it replays what the original's server did with
//! the same library (`fixtures/sql/aep.json`, `tools/ace/aep/`).

#![cfg(target_os = "linux")]

mod common;

use std::path::Path;

#[test]
fn external_procedures_match_the_original_server() {
    let dir = tflip_storage::testing::temp_root().join(format!("tflip-aep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ae")).unwrap();
    // Its calls bind to the client library loaded first.
    if common::build_library(&dir.join("ae/aeptest.aep"), None).is_none() {
        return;
    }
    let ace = common::Ace::load(&common::ace_library());
    let folder = dir.to_str().unwrap().to_owned();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sql/aep.json");
    let differ = common::replay(
        &ace,
        &fixture,
        &folder,
        "ServerType=LOCAL",
        |folder, path| format!("{folder}/{}", path.replace('\\', "/")),
        |text| text.replace(&format!("{folder}/ae/"), r"C:\ads\data\aep\data\ae\"),
    );
    assert!(differ.is_empty(), "{}", differ.join("\n"));
    let _ = std::fs::remove_dir_all(&dir);
}
