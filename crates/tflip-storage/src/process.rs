//! Folders named after the process that owns them.

use std::path::Path;

/// Whether the process `pid` has ended: `None` where the system does not
/// tell (outside Linux).
pub fn ended(pid: u32) -> Option<bool> {
    if cfg!(target_os = "linux") {
        Some(!Path::new("/proc").join(pid.to_string()).exists())
    } else {
        None
    }
}

/// Removes the folders in `parent` named after a process other than this
/// one that `gone` says has ended (given its pid and folder).
pub fn sweep(parent: &Path, gone: impl Fn(u32, &Path) -> bool) {
    let own = std::process::id();
    for entry in std::fs::read_dir(parent).into_iter().flatten().flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if pid != own && gone(pid, &entry.path()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}
