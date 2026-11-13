//! External procedures in the local server, end to end: the library
//! `tools/ace/aep/aeptables.c`, built here with `cc`, binds to the client library
//! `cargo` built (`libtflip_ace.so`), which this test loads as a client
//! would and through which it replays what the original's server did with
//! the same library (`fixtures/sql/aeptables.json`, `tools/ace/aep/`).

#![cfg(target_os = "linux")]

mod common;

use std::path::Path;

#[test]
fn external_procedure_tables_match_the_original_server() {
    let dir =
        tflip_storage::testing::temp_root().join(format!("tflip-aeptables-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ae")).unwrap();
    // Its calls bind to the client library loaded first.
    if common::build_source("aeptables.c", &dir.join("ae/aeptables.aep"), None, &[]).is_none() {
        return;
    }
    let ace = common::Ace::load(&common::ace_library());
    let folder = dir.to_str().unwrap().to_owned();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sql/aeptables.json");
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

/// `__output` through the table API, which the original's servers cannot open
/// (7112), and `__input` and `__error` by their plain names, recorded on the
/// remote one (`fixtures/sql/aeptablesremote.json`): the local server does the
/// same with everything the two have in common.
#[test]
fn a_library_cannot_open_output_through_the_table_api() {
    let dir =
        tflip_storage::testing::temp_root().join(format!("tflip-aepout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ae")).unwrap();
    // The library's traces have the codes only, not the original's texts.
    if common::build_source(
        "aeptables.c",
        &dir.join("ae/plain_aeptables.aep"),
        None,
        &["PLAIN"],
    )
    .is_none()
    {
        return;
    }
    let ace = common::Ace::load(&common::ace_library());
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sql/aeptablesremote.json");
    let folder = dir.to_str().unwrap().to_owned();
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
