//! Which engine a handle belongs to: the local session in the process, or
//! the session of a remote connection on a `tflipd` server.
//!
//! Every connection to a server is a TLS connection of its own with a
//! session of its own there, which hands out handles from a range the
//! client chose ([`REMOTE_BASE`] and up, [`REMOTE_RANGE`] handles each). So
//! a handle names its engine by its value, and the exports pass handles on
//! unchanged, whichever engine they belong to.

use std::collections::BTreeMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tflip_engine::Error;
use tflip_engine::handle::Handle;
use tflip_protocol::Api;
use tflip_protocol::client::Client;

use crate::LOCAL;

/// The first handle of the first remote connection. Local handles stay
/// below it.
pub(crate) const REMOTE_BASE: Handle = 0x4000_0000;

/// How many handles each remote connection has.
pub(crate) const REMOTE_RANGE: Handle = 0x10_0000;

/// How many remote connections fit above [`REMOTE_BASE`] in a 32-bit
/// handle.
const REMOTE_SLOTS: usize = (u32::MAX as usize - REMOTE_BASE) / REMOTE_RANGE;

/// A connection to a server.
pub(crate) struct Remote {
    /// The connection's session on the server.
    pub(crate) client: Client,
    /// The ACE connection handle, from the server's session.
    pub(crate) connection: Handle,
    /// The path the client connected to, as it gave it
    /// (`\\server\share\folder`).
    pub(crate) path: String,
}

struct Remotes {
    /// The open remote connections, by the first handle of their range.
    open: BTreeMap<Handle, Arc<Remote>>,
    /// The slot the next connection gets. Slots are not reused until all
    /// were used, so that handles of a closed connection do not reach the
    /// objects of a later one.
    next: usize,
}

static REMOTES: Mutex<Remotes> = Mutex::new(Remotes {
    open: BTreeMap::new(),
    next: 0,
});

fn remotes() -> MutexGuard<'static, Remotes> {
    REMOTES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The engine a handle belongs to.
pub(crate) enum Backend {
    Local,
    Remote(Arc<Remote>),
    /// A handle an external procedure may not use: its connection's is
    /// another's.
    Foreign,
}

/// Every call on a handle of another connection's session (5202).
static FOREIGN: tflip_protocol::Refusing = tflip_protocol::Refusing(Error::FullText(5202));

impl Deref for Backend {
    type Target = dyn Api;

    fn deref(&self) -> &(dyn Api + 'static) {
        match self {
            Backend::Local => &*LOCAL,
            Backend::Remote(remote) => &remote.client,
            Backend::Foreign => &FOREIGN,
        }
    }
}

impl Backend {
    /// Whether the handle belongs to a remote connection.
    pub(crate) fn is_remote(&self) -> bool {
        matches!(self, Backend::Remote(_))
    }
}

/// The engine `handle` belongs to. Handles of no open remote connection
/// belong to the local session, which does not know them; an external
/// procedure that uses the handle of another connection's session gets
/// 5202 (`INVALID_USE_OF_HANDLE_IN_AEP`), however the call goes.
pub(crate) fn api(handle: Handle) -> Backend {
    if handle < REMOTE_BASE {
        return Backend::Local;
    }
    let base = handle - (handle - REMOTE_BASE) % REMOTE_RANGE;
    let hosted = HOSTED.with(|hosted| {
        hosted
            .borrow()
            .as_ref()
            .map(|(of, remote)| (*of, Arc::clone(remote)))
    });
    if let Some((of, remote)) = &hosted
        && *of == base
    {
        return Backend::Remote(Arc::clone(remote));
    }
    match remotes().open.get(&base) {
        Some(remote) => Backend::Remote(Arc::clone(remote)),
        None if hosted.is_some() => Backend::Foreign,
        None => Backend::Local,
    }
}

/// Whether this thread is an external procedure's in `tflipd`.
pub(crate) fn hosted() -> bool {
    HOSTED.with(|hosted| hosted.borrow().is_some())
}

/// The local connections a library opened in `tflipd` for itself: the
/// original's server makes those its own kind (`ADS_AIS_SERVER`, what
/// `AdsGetConnectionType` says of them).
fn inside() -> std::sync::MutexGuard<'static, std::collections::BTreeSet<Handle>> {
    static INSIDE: Mutex<std::collections::BTreeSet<Handle>> =
        Mutex::new(std::collections::BTreeSet::new());
    INSIDE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Notes `handle` as a connection a library opened for itself.
pub(crate) fn mark_inside(handle: Handle) {
    inside().insert(handle);
}

/// Whether `handle` is such a connection.
pub(crate) fn is_inside(handle: Handle) -> bool {
    inside().contains(&handle)
}

/// Forgets `handle`, which was disconnected.
pub(crate) fn forget_inside(handle: Handle) {
    inside().remove(&handle);
}

/// A free range of handles for a new remote connection: its first handle.
pub(crate) fn reserve_base() -> Handle {
    let mut remotes = remotes();
    loop {
        let slot = remotes.next % REMOTE_SLOTS;
        remotes.next = slot + 1;
        let base = REMOTE_BASE + slot * REMOTE_RANGE;
        if !remotes.open.contains_key(&base) {
            return base;
        }
    }
}

thread_local! {
    /// The session of `tflipd` this thread calls for an external procedure
    /// (see [`TflipHostSession`]), with the first handle of its range.
    static HOSTED: std::cell::RefCell<Option<(Handle, Arc<Remote>)>> =
        const { std::cell::RefCell::new(None) };
}

/// `TflipHostSession` (tflip's own, unpublished): in `tflipd`, which loads
/// this library only for the external procedures that link it, the handles
/// of `connection`'s session go, on this thread, to `serve` with `host`;
/// `serve` of `None` ends that (see [`tflip_aep::Host`]).
///
/// # Safety
///
/// `serve` must answer requests as [`tflip_aep::serve`] does with `host`,
/// until this is called again with `None`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn TflipHostSession(
    connection: usize,
    serve: Option<tflip_aep::Serve>,
    host: *const std::ffi::c_void,
) {
    let base = connection.saturating_sub((connection.saturating_sub(REMOTE_BASE)) % REMOTE_RANGE);
    let remote = serve.and_then(|serve| {
        let host = host as usize;
        let exchange = move |request: &[u8]| {
            let mut reply = Vec::new();
            unsafe extern "C" fn deliver(
                out: *mut std::ffi::c_void,
                bytes: *const u8,
                length: usize,
            ) {
                // SAFETY: `out` is the `reply` below, and `bytes` valid for
                // `length` bytes, as `serve` vouches.
                unsafe {
                    (*(out as *mut Vec<u8>))
                        .extend_from_slice(std::slice::from_raw_parts(bytes, length))
                };
            }
            // SAFETY: as the caller vouches; `reply` outlives the call.
            unsafe {
                serve(
                    host as *const std::ffi::c_void,
                    request.as_ptr(),
                    request.len(),
                    deliver,
                    &mut reply as *mut Vec<u8> as *mut std::ffi::c_void,
                )
            };
            reply
        };
        let client = Client::in_process(base, Box::new(exchange)).ok()?;
        Some((
            base,
            Arc::new(Remote {
                client,
                connection,
                path: String::new(),
            }),
        ))
    });
    HOSTED.with(|hosted| *hosted.borrow_mut() = remote);
}

/// Registers a remote connection whose session hands out handles from
/// `base` on.
pub(crate) fn register(base: Handle, remote: Remote) {
    remotes().open.insert(base, Arc::new(remote));
}

/// Closes a remote connection: disconnects its session and drops the TLS
/// connection.
pub(crate) fn disconnect_remote(connection: Handle) -> Result<(), Error> {
    let Backend::Remote(remote) = api(connection) else {
        return Err(Error::InvalidConnection);
    };
    if remote.connection != connection {
        return Err(Error::InvalidConnection);
    }
    let result = remote.client.disconnect(connection);
    remotes().open.retain(|_, open| !Arc::ptr_eq(open, &remote));
    // A lost connection is closed all the same.
    result.or_else(|error| {
        if error == Error::ConnectionLost {
            Ok(())
        } else {
            Err(error)
        }
    })
}

/// Every open remote connection.
pub(crate) fn all_remotes() -> Vec<Arc<Remote>> {
    remotes().open.values().cloned().collect()
}

/// Runs `f` on the local engine and on every remote connection's; the first
/// failure is the result.
pub(crate) fn each(f: impl Fn(&dyn Api) -> Result<(), Error>) -> Result<(), Error> {
    let mut result = f(&*LOCAL);
    for remote in all_remotes() {
        let done = f(&remote.client);
        if result.is_ok() {
            result = done;
        }
    }
    result
}

/// Closes every remote connection.
pub(crate) fn disconnect_all_remotes() {
    for remote in all_remotes() {
        let _ = disconnect_remote(remote.connection);
    }
}
