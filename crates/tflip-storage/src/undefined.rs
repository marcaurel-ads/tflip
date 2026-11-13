//! The bytes tflip writes where the original engine leaves bytes undefined
//! (whatever its memory held), which a comparison with the original's files
//! must not count: listed with the feature `undefined`, for tests; without
//! it, nothing is kept.

use std::path::{Path, PathBuf};

/// Notes that the byte at `offset` of the file at `path` is one the original
/// engine leaves undefined.
#[cfg(feature = "undefined")]
pub(crate) fn note(path: Option<&Path>, offset: u64) {
    if let Some(path) = path {
        list().lock().unwrap().push((path.to_path_buf(), offset));
    }
}

#[cfg(not(feature = "undefined"))]
pub(crate) fn note(_: Option<&Path>, _: u64) {}

/// The bytes noted in files under the folder `dir` since the last call for
/// it, as (file, offset) pairs.
#[cfg(feature = "undefined")]
pub fn take(dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut list = list().lock().unwrap();
    let (taken, kept) = std::mem::take(&mut *list)
        .into_iter()
        .partition(|(p, _)| p.starts_with(dir));
    *list = kept;
    taken
}

/// The bytes noted under `dir`: none without the feature `undefined`.
#[cfg(not(feature = "undefined"))]
pub fn take(_: &Path) -> Vec<(PathBuf, u64)> {
    Vec::new()
}

#[cfg(feature = "undefined")]
fn list() -> &'static std::sync::Mutex<Vec<(PathBuf, u64)>> {
    static LIST: std::sync::OnceLock<std::sync::Mutex<Vec<(PathBuf, u64)>>> =
        std::sync::OnceLock::new();
    LIST.get_or_init(Default::default)
}
