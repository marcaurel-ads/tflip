//! Maintenance of a table on a server (`CREATE INDEX`, `ALTER TABLE`) as
//! the other connections see it (`fixtures/events/events_online.json`).
//!
//! The offline forms close the table to every other connection while they
//! run: an open of it fails with 7008. The `ONLINE` forms leave it open: the
//! connections read and write the table as it was while the new index or
//! structure is built beside it, and a final stage then takes the table
//! (an open of it waits) and builds again what the connections changed
//! meanwhile. `ONLINE_WITH_TRANSITION` waits before its final stage for the
//! connections that opened the table meanwhile to disconnect.
//!
//! Only a server's sessions are held to this: a local engine's connections
//! are the one process's.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use crate::connection::Connection;
use crate::handle::Handle;
use crate::{Error, Session, connection};

/// How an operation leaves its table to the other connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Closed to them.
    Offline,
    /// Open to them until the final stage.
    Online,
    /// [`Kind::Online`], with a final stage that waits for the connections
    /// that opened the table meanwhile to disconnect.
    Transition,
}

/// Where an operation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Working,
    /// Opens of the table by other connections wait.
    Final,
}

struct Operation {
    kind: Kind,
    stage: Stage,
    /// The session that runs it ([`Session::id`]).
    owner: usize,
    /// The connections of other sessions that opened the table meanwhile.
    openers: Vec<Weak<Connection>>,
}

static ACTIVE: Mutex<Option<HashMap<String, Operation>>> = Mutex::new(None);

/// How long an open waits for a final stage to end, and a final stage for
/// the table to be free of other handles.
const PATIENCE: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(5);

fn with<T>(f: impl FnOnce(&mut HashMap<String, Operation>) -> T) -> T {
    let mut active = ACTIVE.lock().unwrap_or_else(PoisonError::into_inner);
    f(active.get_or_insert_with(HashMap::new))
}

/// What names a table's file in the registry: its path in any case, as a
/// server's file names are.
fn key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

/// A running operation on a table, which ends when this is dropped.
pub(crate) struct Guard {
    key: String,
    kind: Kind,
    owner: usize,
    /// The event its final stage signals, and the connection that signals it.
    signal: Option<(Arc<Connection>, String)>,
}

/// Starts an operation of `kind` on the table at `path` for `session`:
/// `None` for a session that is not a server's. Another operation on the
/// table at the time is 7008. Its final stage signals `event` (as the
/// original does: `__OCIFinalStage_<table>` for an index, `__OAFinalStage_`
/// for `ALTER TABLE`) to the connections of `connection`'s dictionary or
/// folder.
pub(crate) fn begin(
    session: &Session,
    connection: Handle,
    path: &Path,
    kind: Kind,
    event: String,
) -> Result<Option<Guard>, Error> {
    if session.shares.is_none() {
        return Ok(None);
    }
    let (key, owner) = (key(path), session.id());
    with(|active| {
        if active.contains_key(&key) {
            return Err(Error::FileInUse.detailed(path.display().to_string()));
        }
        active.insert(
            key.clone(),
            Operation {
                kind,
                stage: Stage::Working,
                owner,
                openers: Vec::new(),
            },
        );
        let signal = connection::get(session, connection)
            .ok()
            .map(|connection| (connection, event));
        Ok(Some(Guard {
            key,
            kind,
            owner,
            signal,
        }))
    })
}

impl Guard {
    /// The operation without a signal at its final stage: `sp_ReindexOnline`
    /// signals none (`fixtures/events/events_online_procs.json`).
    pub(crate) fn silent(mut self) -> Guard {
        self.signal = None;
        self
    }

    /// Whether the table is open to other connections meanwhile.
    pub(crate) fn online(&self) -> bool {
        self.kind != Kind::Offline
    }

    /// Enters the final stage: for a transition, once the connections that
    /// opened the table have gone; the opens that come after wait.
    pub(crate) fn final_stage(&self) {
        if self.kind == Kind::Transition {
            loop {
                let alive = with(|active| {
                    let operation = active.get_mut(&self.key).expect("a running operation");
                    operation.openers.retain(|opener| opener.strong_count() > 0);
                    !operation.openers.is_empty()
                });
                if !alive {
                    break;
                }
                std::thread::sleep(POLL);
            }
        }
        with(|active| {
            if let Some(operation) = active.get_mut(&self.key) {
                operation.stage = Stage::Final;
            }
        });
        if let Some((connection, event)) = &self.signal {
            crate::events::announce(connection, event);
        }
    }

    /// Waits for the other handles on the table at `path` to close, the
    /// `own` handles of the operation aside: the final stage's opens wait,
    /// so what is open now is what is running. Writes that are on their way
    /// are then in the table.
    pub(crate) fn drain(&self, path: &Path, own: usize) {
        let start = Instant::now();
        while crate::shared::open_count(path) > own && start.elapsed() < PATIENCE {
            std::thread::sleep(POLL);
        }
    }

    /// Retries `attempt` while it fails because another handle has the
    /// table open, which a final stage lets go of in time.
    pub(crate) fn until_free<T>(
        &self,
        mut attempt: impl FnMut() -> Result<T, Error>,
    ) -> Result<T, Error> {
        let start = Instant::now();
        loop {
            match attempt() {
                Err(Error::LockFailed | Error::FileInUse)
                    if self.online() && start.elapsed() < PATIENCE =>
                {
                    std::thread::sleep(POLL);
                }
                done => return done,
            }
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        with(|active| {
            if active.get(&self.key).is_some_and(|o| o.owner == self.owner) {
                active.remove(&self.key);
            }
        });
    }
}

/// Whether an operation is online on the table at `path`, whose inserts
/// then go after the last record (see [`TableFile::append_only`]).
///
/// [`TableFile::append_only`]: tflip_storage::table::TableFile::append_only
pub(crate) fn appends_only(path: &Path) -> bool {
    let key = key(path);
    with(|active| active.get(&key).is_some_and(|o| o.kind != Kind::Offline))
}

/// Lets `session` open the table at `path` on `connection`: the operation
/// running on it closes it to other sessions (7008), or lets it open, and
/// notes who did, or waits for its final stage.
pub(crate) fn admit(session: &Session, connection: Handle, path: &Path) -> Result<(), Error> {
    if session.shares.is_none() {
        return Ok(());
    }
    let (key, me) = (key(path), session.id());
    // A dictionary's table is refused without its file named.
    let file = match connection::get(session, connection) {
        Ok(owner) if owner.dictionary => String::new(),
        _ => path.display().to_string(),
    };
    let start = Instant::now();
    loop {
        let refused = with(|active| {
            let Some(operation) = active.get_mut(&key) else {
                return Ok(false);
            };
            if operation.owner == me {
                return Ok(false);
            }
            match (operation.kind, operation.stage) {
                (Kind::Offline, _) => Err(Error::FileInUse.detailed(file.clone())),
                (_, Stage::Final) => Ok(true),
                (_, Stage::Working) => {
                    if let Ok(opener) = connection::get(session, connection) {
                        let opener: Weak<Connection> = Arc::downgrade(&opener);
                        if !operation.openers.iter().any(|o| o.ptr_eq(&opener)) {
                            operation.openers.push(opener);
                        }
                    }
                    Ok(false)
                }
            }
        })?;
        if !refused {
            return Ok(());
        }
        if start.elapsed() >= PATIENCE {
            return Err(Error::FileInUse.detailed(file));
        }
        std::thread::sleep(POLL);
    }
}
