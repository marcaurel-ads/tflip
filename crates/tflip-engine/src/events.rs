//! Event notifications (`sp_CreateEvent`, `sp_SignalEvent`,
//! `sp_WaitForEvent`, `sp_WaitForAnyEvent`), as the original's server keeps
//! them (`fixtures/events/`, `tools/ace/events/ev.c`): every connection that
//! registers an event gets its own notifications of it, kept until it waits
//! for them, whoever signals. They are shared by all connections of the
//! process, the client library's and every client's of `tflipd`, to the
//! same dictionary (or the same folder of free tables), so that a signal on
//! one connection reaches the others.

use std::collections::HashMap;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::connection::Connection;
use crate::{Error, ace_error};

/// Where a procedure of events is called from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Context {
    /// A statement of the client.
    Direct,
    /// A script the client sent.
    Script,
    /// A stored procedure.
    Procedure,
    /// A trigger.
    Trigger,
}

/// An event a connection registered, and what was signalled to it since it
/// last waited.
struct Registered {
    /// The name as it was registered.
    name: String,
    /// Whether it takes the data of a signal (`ADS_EVENT_WITH_DATA`).
    data: bool,
    /// The signals, with their data, in the order they came.
    signals: Vec<Option<String>>,
}

/// A connection's events, in the order it registered them.
struct Listener {
    /// What its events are shared within (see [`scope`]).
    scope: String,
    events: Vec<Registered>,
}

/// A signal that waits for its transaction to commit.
struct Deferred {
    scope: String,
    name: String,
    data: Option<String>,
}

#[derive(Default)]
struct Registry {
    listeners: HashMap<usize, Listener>,
    deferred: HashMap<usize, Vec<Deferred>>,
    /// The numbers of the statements that wait, and whether one was told to
    /// stop (`sp_CancelQuery`).
    waiting: HashMap<u32, bool>,
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);
static CHANGED: Condvar = Condvar::new();

fn registry() -> MutexGuard<'static, Option<Registry>> {
    let mut guard = REGISTRY.lock().unwrap_or_else(|p| p.into_inner());
    guard.get_or_insert_with(Registry::default);
    guard
}

fn with<R>(f: impl FnOnce(&mut Registry) -> R) -> R {
    let mut guard = registry();
    f(guard.as_mut().expect("created above"))
}

/// A connection's number: the process's address of it, which no other
/// connection has while it is open.
fn id(connection: &Connection) -> usize {
    std::ptr::from_ref(connection) as usize
}

/// What events are shared within: the dictionary or folder a connection is
/// to, by its path (as the original's, regardless of case and of the way the
/// path is written).
fn scope(connection: &Connection) -> String {
    let path = connection.path.to_string_lossy().replace('\\', "/");
    let mut parts: Vec<String> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part.to_lowercase()),
        }
    }
    parts.join("/")
}

/// An event's name as the original compares it: regardless of case (of ASCII
/// letters) and of the spaces after it.
fn key(name: &str) -> String {
    name.trim_end_matches(' ').to_ascii_lowercase()
}

fn not_found() -> Error {
    ace_error::sql(5041, "The specified event was not found.")
}

/// `sp_CreateEvent`: `connection` takes the notifications of `name`.
pub(crate) fn create(connection: &Connection, name: &str, options: i64) -> Result<(), Error> {
    // 1 is taken and has no effect, 2 is `ADS_EVENT_WITH_DATA`.
    if !(0..=3).contains(&options) {
        return Err(ace_error::sql(5019, ""));
    }
    let scope = scope(connection);
    with(|registry| {
        let listener = registry
            .listeners
            .entry(id(connection))
            .or_insert(Listener {
                scope,
                events: Vec::new(),
            });
        if listener.events.iter().any(|e| key(&e.name) == key(name)) {
            return Err(ace_error::sql(
                5051,
                "An event with this name already exists.",
            ));
        }
        listener.events.push(Registered {
            name: name.to_owned(),
            data: options & 2 != 0,
            signals: Vec::new(),
        });
        Ok(())
    })
}

/// `sp_DropEvent`.
pub(crate) fn drop_event(connection: &Connection, name: &str) -> Result<(), Error> {
    with(|registry| {
        let events = registry
            .listeners
            .get_mut(&id(connection))
            .map(|l| &mut l.events);
        match events.and_then(|e| {
            e.iter()
                .position(|e| key(&e.name) == key(name))
                .map(|n| (e, n))
        }) {
            Some((events, at)) => {
                events.remove(at);
                Ok(())
            }
            None => Err(not_found()),
        }
    })
}

/// `sp_DropAllEvents`, and what a connection that closes leaves.
pub(crate) fn drop_all(connection: &Connection) {
    with(|registry| {
        registry.listeners.remove(&id(connection));
        registry.deferred.remove(&id(connection));
    });
}

/// Delivers a signal of `name` to the connections of `scope` that registered
/// it.
fn deliver(registry: &mut Registry, scope: &str, name: &str, data: Option<&str>) {
    for listener in registry.listeners.values_mut() {
        if listener.scope != scope {
            continue;
        }
        if let Some(event) = listener
            .events
            .iter_mut()
            .find(|e| key(&e.name) == key(name))
        {
            event.signals.push(data.map(str::to_owned));
        }
    }
    CHANGED.notify_all();
}

/// The signal of an event the server itself sends, to the connections of
/// `connection`'s dictionary or folder that registered `name` (the final
/// stage of online maintenance, `__OCIFinalStage_<table>`).
pub(crate) fn announce(connection: &Connection, name: &str) {
    let scope = scope(connection);
    with(|registry| deliver(registry, &scope, name, None));
}

/// `sp_SignalEvent`. A dictionary's connection may signal from a stored
/// procedure or a trigger only. With `wait_for_commit`, in a transaction, the
/// signal is sent when the transaction commits, and not if it rolls back.
pub(crate) fn signal(
    connection: &Connection,
    context: Context,
    name: &str,
    wait_for_commit: bool,
    data: Option<String>,
) -> Result<(), Error> {
    if connection.dictionary && matches!(context, Context::Direct | Context::Script) {
        return Err(ace_error::sql(
            5054,
            "For security reasons, event signals are only allowed inside stored procedures \
             and triggers.",
        ));
    }
    let scope = scope(connection);
    let deferring = wait_for_commit
        && connection.settings.real_transactions_on()
        && crate::lock(&connection.transaction).in_transaction();
    with(|registry| match deferring {
        true => registry
            .deferred
            .entry(id(connection))
            .or_default()
            .push(Deferred {
                scope,
                name: name.to_owned(),
                data,
            }),
        false => deliver(registry, &scope, name, data.as_deref()),
    });
    Ok(())
}

/// The transaction of `connection` committed: the signals it held are sent.
pub(crate) fn committed(connection: &Connection) {
    with(|registry| {
        for held in registry
            .deferred
            .remove(&id(connection))
            .unwrap_or_default()
        {
            deliver(registry, &held.scope, &held.name, held.data.as_deref());
        }
    });
}

/// The transaction of `connection` rolled back: its signals are not sent.
pub(crate) fn rolled_back(connection: &Connection) {
    with(|registry| {
        registry.deferred.remove(&id(connection));
    });
}

/// A signal that waits for an event: its name, and for an event with data
/// each signal's number and data, or for one without the number of them.
pub(crate) struct Notice {
    pub name: String,
    pub count: u32,
    pub data: Option<String>,
}

/// What a wait found.
pub(crate) struct Found {
    /// The notices, none if it timed out.
    pub notices: Vec<Notice>,
}

/// Takes the signals `connection` has for `name`, or for the first event of
/// it that has any if `name` is none.
fn take(registry: &mut Registry, connection: &Connection, name: Option<&str>) -> Vec<Notice> {
    let Some(listener) = registry.listeners.get_mut(&id(connection)) else {
        return Vec::new();
    };
    let wanted = name.map(key);
    for event in &mut listener.events {
        if wanted.as_ref().is_some_and(|w| *w != key(&event.name)) || event.signals.is_empty() {
            continue;
        }
        let signals = std::mem::take(&mut event.signals);
        let shown = name.unwrap_or(&event.name).to_owned();
        return match event.data {
            true => signals
                .into_iter()
                .enumerate()
                .map(|(n, data)| Notice {
                    name: shown.clone(),
                    count: n as u32 + 1,
                    data,
                })
                .collect(),
            false => vec![Notice {
                name: shown,
                count: signals.len() as u32,
                data: None,
            }],
        };
    }
    Vec::new()
}

/// How long a wait of `timeout` milliseconds lasts: forever for -1 (and any
/// other negative number, as the original's local server does).
fn limit(timeout: i64) -> Option<Duration> {
    u64::try_from(timeout).ok().map(Duration::from_millis)
}

/// `sp_WaitForEvent` (for `name`) and `sp_WaitForAnyEvent` (without): the
/// notices of the event, waiting up to `timeout` milliseconds for one if
/// there is none. `statement` is the number of the statement that waits, which
/// `sp_CancelQuery` can end ([`cancel`]).
///
/// The original has a timeout wait in whole polls of 300 ms (and one more
/// on a remote server): this one waits as long as it is told.
pub(crate) fn wait(
    connection: &Connection,
    context: Context,
    statement: u32,
    name: Option<&str>,
    timeout: i64,
) -> Result<Found, Error> {
    if let Some(name) = name
        && !with(|registry| {
            registry
                .listeners
                .get(&id(connection))
                .is_some_and(|l| l.events.iter().any(|e| key(&e.name) == key(name)))
        })
    {
        return Err(not_found());
    }
    if context != Context::Direct && timeout != 0 {
        let scripts = match name {
            Some(_) => "SQL scripts, stored procedures, or triggers",
            None => "SQL scripts",
        };
        return Err(ace_error::sql(
            5207,
            &format!("sp_WaitForEvent calls with a timeout are not allowed inside {scripts}."),
        ));
    }
    let until = limit(timeout).map(|t| Instant::now() + t);
    crate::statement::idle(statement);
    let mut guard = registry();
    guard
        .as_mut()
        .expect("created above")
        .waiting
        .insert(statement, false);
    let outcome = loop {
        let registry = guard.as_mut().expect("created above");
        // A cancelled wait ends whatever came since.
        if registry.waiting.get(&statement) == Some(&true) {
            break Err(Error::Constraint {
                code: 7209,
                text: "SQL query aborted by user.".to_owned(),
            });
        }
        let notices = take(registry, connection, name);
        if !notices.is_empty() {
            break Ok(notices);
        }
        let left = match until {
            None => None,
            Some(until) => match until.checked_duration_since(Instant::now()) {
                Some(left) if !left.is_zero() => Some(left),
                _ => break Ok(Vec::new()),
            },
        };
        guard = match left {
            Some(left) => {
                CHANGED
                    .wait_timeout(guard, left)
                    .unwrap_or_else(|p| p.into_inner())
                    .0
            }
            None => CHANGED.wait(guard).unwrap_or_else(|p| p.into_inner()),
        };
    };
    guard
        .as_mut()
        .expect("created above")
        .waiting
        .remove(&statement);
    outcome.map(|notices| Found { notices })
}

/// `sp_CancelQuery` of the statement numbered `statement`, if it waits for an
/// event: the wait ends with 7209. False for another.
pub(crate) fn cancel(statement: u32) -> bool {
    with(|registry| match registry.waiting.get_mut(&statement) {
        Some(stop) => {
            *stop = true;
            CHANGED.notify_all();
            true
        }
        None => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Session, connection};

    /// A wait that `sp_CancelQuery` cancels ends in 7209 and leaves the
    /// signals that came after it for the next wait (`fixtures/events/`
    /// replays it on the original's server, through the statement list).
    #[test]
    fn a_cancelled_wait_ends_and_keeps_its_signals() {
        let session = Session::new();
        let handle = connection::connect(&session, "Data Source=cancelled;ServerType=LOCAL;");
        let waiter = connection::get(&session, handle.unwrap()).unwrap();
        create(&waiter, "x", 0).unwrap();
        let thread = std::thread::scope(|scope| {
            let started = scope.spawn(|| wait(&waiter, Context::Direct, 4_000_001, Some("x"), -1));
            while !cancel(4_000_001) {
                std::thread::sleep(Duration::from_millis(5));
            }
            signal(&waiter, Context::Procedure, "x", false, None).unwrap();
            started.join().unwrap()
        });
        assert!(matches!(thread, Err(Error::Constraint { code: 7209, .. })));
        let found = wait(&waiter, Context::Direct, 4_000_002, Some("x"), 0).unwrap();
        assert_eq!(found.notices[0].count, 1);
        assert!(!cancel(4_000_001));
    }
}
