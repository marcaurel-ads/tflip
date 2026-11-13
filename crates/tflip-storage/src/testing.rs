//! The folder the workspace's tests keep their files in.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The folder of this process's test files, `tflip-tests/<pid>` in the
/// temporary folder. A test process never removes it itself, since folders
/// held by statics outlive every test and a process ends without dropping
/// them; instead the first call removes the folders of the processes that
/// have ended, so a run leaves no more behind than its own.
pub fn temp_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let parent = std::env::temp_dir().join("tflip-tests");
        crate::process::sweep(&parent, |pid, folder| {
            crate::process::ended(pid).unwrap_or_else(|| untouched_for_a_day(folder))
        });
        let root = parent.join(std::process::id().to_string());
        std::fs::create_dir_all(&root).unwrap();
        root
    })
    .clone()
}

/// No test run lasts a day, so such a folder is an ended process's.
fn untouched_for_a_day(folder: &Path) -> bool {
    let day = std::time::Duration::from_secs(24 * 3600);
    folder
        .metadata()
        .and_then(|m| m.modified())
        .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age > day))
}
