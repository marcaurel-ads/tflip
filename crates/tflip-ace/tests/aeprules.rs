//! What an external procedure may do with the connection handle it is given
//! and with connections of its own, in the local server: the library
//! `tools/ace/aep/aeprules.c`, built here with `cc`, binds to the client
//! library `cargo` built (`libtflip_ace.so`), which this test loads as a
//! client would and through which it replays what the original's local
//! server did with the same library (`fixtures/sql/aeprules.json`,
//! `tools/ace/aep/aeprules.sh`).

#![cfg(target_os = "linux")]

mod common;

use std::path::Path;

#[test]
fn rules_for_external_procedures_match_the_original_local_server() {
    let dir =
        tflip_storage::testing::temp_root().join(format!("tflip-aeprules-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ae")).unwrap();
    // Its calls bind to the client library loaded first.
    if common::build_source("aeprules.c", &dir.join("ae/aeprules.aep"), None, &[]).is_none() {
        return;
    }
    // The original's local server counts transactions, and so does this one
    // when it reads the setting, which it does as the first connection opens.
    let ini = dir.join("tflip.ini");
    std::fs::write(&ini, "[transactions]\nmode = count\n").unwrap();
    // SAFETY: before the client library reads it, and no other thread
    // reads the environment.
    unsafe { std::env::set_var("TFLIP_CONFIG", &ini) };
    let ace = common::Ace::load(&common::ace_library());
    let folder = dir.to_str().unwrap().to_owned();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sql/aeprules.json");
    let differ = common::replay(
        &ace,
        &fixture,
        &folder,
        "ServerType=LOCAL",
        |folder, path| format!("{folder}/{}", path.replace('\\', "/")),
        |text| text.to_owned(),
    );
    assert!(differ.is_empty(), "{}", differ.join("\n"));
    let _ = std::fs::remove_dir_all(&dir);
}
