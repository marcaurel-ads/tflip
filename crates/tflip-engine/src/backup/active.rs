//! The files a running backup has taken: while it copies a table or the
//! dictionary file no other backup may (the 7131 rows of the result set), and
//! no other connection opens the table (`fixtures/backup/README.md`, "Table
//! in use").

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use crate::{Error, Session};

static TAKEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut HashSet<String>) -> T) -> T {
    let mut taken = TAKEN.lock().unwrap_or_else(PoisonError::into_inner);
    f(taken.get_or_insert_with(HashSet::new))
}

/// A server's file names are the same in any case.
fn key(path: &Path) -> String {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    path.to_string_lossy().to_lowercase()
}

/// A file a backup has taken, which the backup gives back when this is
/// dropped.
#[derive(Debug)]
pub(crate) struct Hold(String);

impl Hold {
    /// Takes the file at `path`; `None` if another backup has it.
    pub(crate) fn take(path: &Path) -> Option<Hold> {
        let key = key(path);
        // Lazily: a refused take must not drop a hold, which gives the file back.
        with(|taken| taken.insert(key.clone())).then(|| Hold(key))
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        with(|taken| taken.remove(&self.0));
    }
}

/// Lets a connection of `session` open the table at `path`: a table a backup
/// is copying is refused with 7131. A local engine has no backups.
pub(crate) fn admit(session: &Session, path: &Path) -> Result<(), Error> {
    if session.shares.is_none() || with(|taken| taken.is_empty()) {
        return Ok(());
    }
    match with(|taken| taken.contains(&key(path))) {
        true => Err(Error::Ace {
            code: 7131,
            detail: String::new(),
        }),
        false => Ok(()),
    }
}

/// What a test runs once, in the backup that takes the dictionary file at
/// `add`, when it has taken its files and before it copies them: a backup
/// that is under way for as long as the test wants.
#[cfg(feature = "test-hooks")]
type Pause = Box<dyn FnOnce() + Send>;

#[cfg(feature = "test-hooks")]
static PAUSES: Mutex<Vec<(String, Pause)>> = Mutex::new(Vec::new());

/// Sets the pause of the next backup of the dictionary at `add`.
#[cfg(feature = "test-hooks")]
pub fn pause_next_backup(add: &Path, pause: impl FnOnce() + Send + 'static) {
    let mut pauses = PAUSES.lock().unwrap_or_else(PoisonError::into_inner);
    pauses.push((key(add), Box::new(pause)));
}

/// Runs the pause set for `add`, if any.
#[cfg(feature = "test-hooks")]
pub(crate) fn paused(add: &Path) {
    let key = key(add);
    let pause = {
        let mut pauses = PAUSES.lock().unwrap_or_else(PoisonError::into_inner);
        let at = pauses.iter().position(|(k, _)| *k == key);
        at.map(|at| pauses.remove(at).1)
    };
    if let Some(pause) = pause {
        pause();
    }
}

#[cfg(not(feature = "test-hooks"))]
pub(crate) fn paused(_: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_taken_by_one_backup_at_a_time() {
        let path = std::env::temp_dir().join("tflip-active-hold-test.adt");
        let first = Hold::take(&path).unwrap();
        assert!(Hold::take(&path.with_extension("ADT")).is_none());
        assert!(
            Hold::take(&path).is_none(),
            "a refused take gives nothing back"
        );
        drop(first);
        assert!(Hold::take(&path).is_some());
    }
}
