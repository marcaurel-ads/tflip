//! The paths of a backup or restore call: server paths as the original's
//! server on Windows takes them (`C:\data\img`), reached on any host.

use std::fs;
use std::path::{Path, PathBuf};

/// Whether the call's path names a place by its full path, as the original
/// requires: a drive's, or a rooted one. A UNC path and a relative one are
/// not.
pub(crate) fn is_absolute(given: &str) -> bool {
    let bytes = given.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    drive || (given.starts_with('/') && !given.starts_with("//")) || is_rooted(given)
}

/// A path that starts at the root of the drive it is on (`\data`), which
/// needs a Windows host to mean anything.
fn is_rooted(given: &str) -> bool {
    cfg!(windows) && given.starts_with('\\') && !given.starts_with("\\\\")
}

/// The path on this host, or `None` where the call names a drive this host
/// does not have (a Windows path on another host).
pub(crate) fn local(given: &str) -> Option<PathBuf> {
    if cfg!(windows) {
        return Some(PathBuf::from(given));
    }
    given
        .starts_with('/')
        .then(|| PathBuf::from(given.replace('\\', "/")))
}

/// The folder of the path `given`, as it was written, without its last
/// separator.
pub(crate) fn folder_of(given: &str) -> &str {
    match given.rfind(['\\', '/']) {
        Some(at) => &given[..at],
        None => "",
    }
}

/// `name` in the folder `folder`, as the original's rows write a path: with a
/// backslash between them.
pub(crate) fn join(folder: &str, name: &str) -> String {
    format!("{}\\{name}", folder.trim_end_matches(['\\', '/']))
}

/// The file called `name` in `folder`, in whatever case the folder has it.
pub(crate) fn find(folder: &Path, name: &str) -> Option<PathBuf> {
    let exact = folder.join(name);
    if exact.exists() {
        return Some(exact);
    }
    fs::read_dir(folder)
        .ok()?
        .flatten()
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(name)
        })
        .map(|entry| entry.path())
}

/// Whether `a` and `b` name the same file, which need not exist.
pub(crate) fn same(a: &Path, b: &Path) -> bool {
    let full = |path: &Path| {
        let parent = path.parent().and_then(|p| fs::canonicalize(p).ok());
        match (parent, path.file_name()) {
            (Some(parent), Some(name)) => parent.join(name),
            _ => path.to_owned(),
        }
    };
    let (a, b) = (full(a), full(b));
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_absolute() {
        assert!(is_absolute(r"C:\ads\data"));
        assert!(is_absolute("/tmp/x\\img"));
        assert!(!is_absolute("relimg"));
        assert!(!is_absolute(r"\\127.0.0.1\c$\ads"));
    }

    #[test]
    fn rows_write_paths_with_a_backslash() {
        assert_eq!(join(r"C:\img\", "t_adt"), r"C:\img\t_adt");
        assert_eq!(folder_of(r"C:\img\d.add"), r"C:\img");
    }
}
