//! The lockout of users whose logins fail too often, on a server.
//!
//! With the dictionary's `ENFORCE_MAX_FAILED_LOGINS` on, the original's
//! remote server counts a user's failed logins and, at `MAX_FAILED_ATTEMPTS`
//! of them, disables the user's logins (`fixtures/events/lockout_*.json`).
//! The count lives in the server's memory only: a restart forgets it, a good
//! login and an administrator enabling the user again do not reset it, and
//! the next failure locks the user out again. `ADSSYS` is locked out the same
//! way, but only in memory, where no other administrator can undo it.
//!
//! A user whose logins are disabled is told so (7113) before the password is
//! looked at, which the local server does not do.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use tflip_storage::dd;

use crate::dictionary::Database;
use crate::{Error, Session};

/// The failed logins since the server started, by dictionary and user (both
/// in lower case; the administrator has no user object, so it is `None`).
static FAILURES: Mutex<Option<HashMap<(PathBuf, Option<String>), u32>>> = Mutex::new(None);

/// The dictionaries whose administrator is locked out.
static LOCKED_ADMINISTRATORS: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn key(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().to_lowercase())
}

/// [`Database::login`] for a connection to a server's dictionary at `path`,
/// with the lockout of users that fail too often.
pub(crate) fn login(
    session: &Session,
    path: &Path,
    database: &Database,
    user: Option<&str>,
    password: &[u8],
) -> Result<Option<Vec<u8>>, Error> {
    let named = user.filter(|u| !u.is_empty());
    if let Some(user) = named {
        let refusal = match Database::is_administrator_name(user) {
            true => lock(&LOCKED_ADMINISTRATORS)
                .as_ref()
                .is_some_and(|locked| locked.contains(&key(path)))
                .then(Database::logins_disabled_for_user),
            false => database.user_disabled(user),
        };
        if let Some(refusal) = refusal {
            return Err(refusal);
        }
    }
    let result = database.login(user, password);
    if let (Err(Error::LoginFailed), Some(user)) = (&result, named)
        && database.has_user(user)
        && let Some(limit) = database.failed_login_limit()
    {
        failed(session, path, database, user, limit);
    }
    // A server names the call in every refusal, an unknown user's too, which
    // the local server does not (`fixtures/events/lockout_unknown.json`).
    result.map_err(|error| match error {
        Error::LoginFailed => Error::Ace {
            code: 7078,
            detail: "axServerConnect".to_owned(),
        },
        other => other,
    })
}

/// Counts a failed login of `user`, and locks the user out if that makes
/// `limit` of them.
fn failed(session: &Session, path: &Path, database: &Database, user: &str, limit: u16) {
    let administrator = Database::is_administrator_name(user);
    let who = (!administrator).then(|| user.to_lowercase());
    let count = {
        let mut guard = lock(&FAILURES);
        let count = guard
            .get_or_insert_with(HashMap::new)
            .entry((key(path), who))
            .or_insert(0);
        *count += 1;
        *count
    };
    if count < u32::from(limit) {
        return;
    }
    match administrator {
        true => {
            lock(&LOCKED_ADMINISTRATORS)
                .get_or_insert_with(HashSet::new)
                .insert(key(path));
        }
        false => {
            // A write that fails leaves the user able to log in, which is
            // all the server can do about it.
            if let Some(record) = database.user_record(user) {
                let _ = crate::dictionary_write::write_slot_unconnected(
                    session,
                    path,
                    record,
                    dd::user::LOGINS_DISABLED,
                    &1u16.to_le_bytes(),
                );
            }
        }
    }
}
