//! Loading the libraries of external procedures: a Windows DLL with
//! `LoadLibraryW`, a shared object with `dlopen`, as the original's servers
//! load them (see [`tflip_engine::aep`]). Only the version 2 interface is
//! called for an external procedure: the library's `GetInterfaceVersion`
//! says 2. A trigger's library has neither that nor a `Startup` to call
//! ([`Libraries::trigger`](tflip_engine::aep::Libraries::trigger)). A library stays
//! loaded until the process ends; the original's DLL caching (a copy loaded
//! again when the file changes) is not done.
//!
//! The library links tflip's ACE, and calls it with the connection handle
//! it is given. In the local server ([`Loader::local`]) that ACE is the
//! client library itself, whose session the handle is one of. In `tflipd`
//! ([`Loader::hosting`]) it is a copy of the client library that `tflipd`
//! does not otherwise use: before each call into the library, `tflipd` tells
//! that copy ([`HOST_EXPORT`]) to send what the library asks on this thread
//! to [`serve`], which answers it with the session of the client whose
//! statement runs the procedure, as `tflipd` answers its requests over the
//! network.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use tflip_engine::Session;
use tflip_engine::aep::{Failure, TriggerCall};
use tflip_engine::handle::Handle;

type GetInterfaceVersion = unsafe extern "system" fn() -> u32;
type Startup = unsafe extern "system" fn(u32, usize) -> u32;
type Procedure = unsafe extern "system" fn(u32, usize, *mut u32) -> u32;
type TriggerFunction =
    unsafe extern "system" fn(u32, usize, *const c_char, *const c_char, u32, u32, u32) -> u32;

/// What [`serve`] hands a reply to: `out` as it was given, and the reply.
pub type Deliver = unsafe extern "C" fn(out: *mut c_void, reply: *const u8, length: usize);

/// [`serve`]'s type: the session behind `host` answers the request of
/// `length` bytes at `request`, and hands its reply to `deliver` with `out`.
pub type Serve = unsafe extern "C" fn(
    host: *const c_void,
    request: *const u8,
    length: usize,
    deliver: Deliver,
    out: *mut c_void,
);

/// The type of the client library's [`HOST_EXPORT`]: the handles of
/// `connection`'s session, on the calling thread, go to `serve` with
/// `host`; a `serve` of `None` ends that.
pub type Host = unsafe extern "C" fn(connection: usize, serve: Option<Serve>, host: *const c_void);

/// The unpublished export of tflip's client library that a [`Host`] is.
pub const HOST_EXPORT: &str = "TflipHostSession";

/// A loaded library.
struct Library {
    /// What `dlopen` or `LoadLibraryW` returned, as an address.
    module: usize,
    /// What `GetInterfaceVersion` says, if the library has one.
    version: Option<u32>,
    /// In `tflipd`, the library's ACE's [`HOST_EXPORT`], if it links the ACE.
    host: Option<Host>,
    /// The connections whose `Startup` ran and whose `Shutdown` is due, by
    /// their session (its address) and handle.
    started: BTreeSet<(usize, Handle)>,
}

/// The libraries loaded, by their path, and the connection ids handed out.
struct Loaded {
    libraries: BTreeMap<PathBuf, Library>,
    ids: BTreeMap<(usize, Handle), u32>,
}

static LOADED: Mutex<Loaded> = Mutex::new(Loaded {
    libraries: BTreeMap::new(),
    ids: BTreeMap::new(),
});

fn loaded() -> std::sync::MutexGuard<'static, Loaded> {
    LOADED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A session's address, which tells it from the others while it lives.
fn address(session: &Session) -> usize {
    &**session as *const _ as usize
}

/// What loads the libraries for [`tflip_engine::aep`].
pub struct Loader {
    /// Whether the library's ACE is told whose session to call ([`HOST_EXPORT`]).
    hosting: bool,
    /// What each call into a library runs inside.
    around: fn(&mut dyn FnMut()),
}

impl Loader {
    /// The local server's: the library's ACE is the one that runs the
    /// session. `around` runs each call into the library (the client
    /// library keeps its last error apart from the library's).
    pub fn local(around: fn(&mut dyn FnMut())) -> Loader {
        Loader {
            hosting: false,
            around,
        }
    }

    /// `tflipd`'s: the library's ACE sends its calls to the session.
    pub fn hosting() -> Loader {
        Loader {
            hosting: true,
            around: |f| f(),
        }
    }
}

/// Hosts `session` in a library's ACE for as long as it lives.
struct Hosted {
    host: Option<(Host, usize)>,
}

impl Hosted {
    fn new(host: Option<Host>, session: &Session, connection: Handle) -> Hosted {
        let host = host.map(|host| {
            // SAFETY: the client library's `TflipHostSession`; the session
            // outlives the call it is hosted for.
            unsafe {
                host(
                    connection,
                    Some(serve),
                    session as *const Session as *const c_void,
                )
            };
            (host, connection)
        });
        Hosted { host }
    }
}

impl Drop for Hosted {
    fn drop(&mut self) {
        if let Some((host, connection)) = self.host {
            // SAFETY: as in `Hosted::new`.
            unsafe { host(connection, None, std::ptr::null()) };
        }
    }
}

impl Loader {
    /// The library at `path` loaded (once), its function `function` as a `F`,
    /// the `Startup` still due for `key` if `procedure`, the host and the
    /// connection's id. The lock is not held once this returns: the library
    /// may run another external procedure.
    fn resolve<F: Copy>(
        &self,
        path: &Path,
        function: &str,
        key: (usize, Handle),
        procedure: bool,
    ) -> Result<(F, Option<Startup>, Option<Host>, u32), Failure> {
        let mut loaded = loaded();
        let next = loaded.ids.len() as u32 + 1;
        let id = *loaded.ids.entry(key).or_insert(next);
        let library = match loaded.libraries.get_mut(path) {
            Some(library) => library,
            None => {
                let library = self.load(path)?;
                loaded.libraries.entry(path.to_owned()).or_insert(library)
            }
        };
        if procedure {
            if library.version != Some(2) {
                return Err(Failure::Refused(format!(
                    "The AEP named \"{}\" is not one of version 2, which alone tflip calls.",
                    path.display()
                )));
            }
            if self.hosting && library.host.is_none() {
                return Err(Failure::Refused(format!(
                    "The AEP named \"{}\" does not link tflip's ACE, which tflipd needs.",
                    path.display()
                )));
            }
        }
        // SAFETY: the function's prototype, as the help gives it.
        let found = unsafe { symbol::<F>(library.module, function) }.ok_or(Failure::NoFunction)?;
        // SAFETY: as `Shutdown`'s, in `closing`.
        let startup = unsafe { symbol::<Startup>(library.module, "Startup") };
        let startup = procedure
            .then(|| library.started.insert(key).then_some(startup).flatten())
            .flatten();
        Ok((found, startup, library.host, id))
    }
}

impl tflip_engine::aep::Libraries for Loader {
    fn call(
        &self,
        session: &Session,
        path: &Path,
        function: &str,
        connection: Handle,
        rows: &mut u32,
    ) -> Result<u32, Failure> {
        let key = (address(session), connection);
        let (procedure, startup, host, id) =
            self.resolve::<Procedure>(path, function, key, true)?;
        let _hosted = Hosted::new(host, session, connection);
        let mut returned = 0;
        (self.around)(&mut || {
            // SAFETY: the version 2 interface's prototypes; the library runs
            // on this thread and calls its ACE.
            unsafe {
                if let Some(startup) = startup {
                    startup(id, connection);
                }
                returned = procedure(id, connection, rows);
            }
        });
        Ok(returned)
    }

    fn trigger(
        &self,
        session: &Session,
        path: &Path,
        function: &str,
        connection: Handle,
        call: &TriggerCall<'_>,
    ) -> Result<u32, Failure> {
        let key = (address(session), connection);
        let (trigger, _, host, id) = self.resolve::<TriggerFunction>(path, function, key, false)?;
        let text = |text: &str| CString::new(text.replace('\0', "")).unwrap_or_default();
        let (name, table) = (text(call.name), text(call.table));
        let _hosted = Hosted::new(host, session, connection);
        let mut returned = 0;
        (self.around)(&mut || {
            // SAFETY: the trigger prototype of the help's "Trigger
            // Containers"; the library runs on this thread and calls its ACE.
            returned = unsafe {
                trigger(
                    id,
                    connection,
                    name.as_ptr(),
                    table.as_ptr(),
                    call.event,
                    call.kind,
                    call.recno,
                )
            };
        });
        Ok(returned)
    }

    fn closing(&self, session: &Session, connection: Handle) {
        let key = (address(session), connection);
        let (due, id) = {
            let mut loaded = loaded();
            let id = loaded.ids.remove(&key);
            let due: Vec<(usize, Option<Host>)> = loaded
                .libraries
                .values_mut()
                .filter_map(|library| {
                    library
                        .started
                        .remove(&key)
                        .then_some((library.module, library.host))
                })
                .collect();
            (due, id)
        };
        let Some(id) = id else {
            return;
        };
        for (module, host) in due {
            // SAFETY: the version 2 interface's `Shutdown`, which takes what
            // `Startup` took.
            let Some(shutdown) = (unsafe { symbol::<Startup>(module, "Shutdown") }) else {
                continue;
            };
            let _hosted = Hosted::new(host, session, connection);
            // SAFETY: as above.
            (self.around)(&mut || unsafe {
                shutdown(id, connection);
            });
        }
    }
}

impl Loader {
    /// Loads the library at `path`. A procedure's must be a version 2 one,
    /// and in `tflipd` link tflip's ACE, which [`Loader::resolve`] asks.
    fn load(&self, path: &Path) -> Result<Library, Failure> {
        let module = sys::load(path);
        if module.is_null() {
            return Err(Failure::NotLoaded);
        }
        let module = module as usize;
        // SAFETY: the version 2 interface's `GetInterfaceVersion`, which
        // takes nothing.
        let version = unsafe { symbol::<GetInterfaceVersion>(module, "GetInterfaceVersion") }
            .map(|f| unsafe { f() });
        let host = match self.hosting {
            false => None,
            // SAFETY: tflip's client library's export is a `Host`.
            true => unsafe { sys::ace_symbol::<Host>(module, HOST_EXPORT) },
        };
        Ok(Library {
            module,
            version,
            host,
            started: BTreeSet::new(),
        })
    }
}

/// Answers a request of a library's ACE with the session it hosts (see
/// [`Host`]).
///
/// # Safety
///
/// `host` must be the `&Session` [`Hosted`] gave, `request` valid for
/// `length` bytes, and `deliver` must take `out`.
pub unsafe extern "C" fn serve(
    host: *const c_void,
    request: *const u8,
    length: usize,
    deliver: Deliver,
    out: *mut c_void,
) {
    // SAFETY: as the caller vouches.
    let (session, request) = unsafe {
        (
            &*(host as *const Session),
            std::slice::from_raw_parts(request, length),
        )
    };
    let reply = tflip_protocol::local::answer(session, request);
    // SAFETY: as the caller vouches.
    unsafe { deliver(out, reply.as_ptr(), reply.len()) };
}

/// The function `name` of the loaded library `module`, as a `F`.
///
/// # Safety
///
/// `F` must be a function pointer type the symbol has.
unsafe fn symbol<F: Copy>(module: usize, name: &str) -> Option<F> {
    let name = CString::new(name).ok()?;
    // SAFETY: `module` is a loaded library, `name` a C string.
    let address = unsafe { sys::symbol(module as *mut c_void, name.as_ptr()) };
    // SAFETY: the caller vouches that the symbol is a `F`, which is a
    // function pointer as large as an address.
    (!address.is_null()).then(|| unsafe { std::mem::transmute_copy::<*mut c_void, F>(&address) })
}

#[cfg(unix)]
mod sys {
    use std::ffi::{CString, c_char, c_int, c_void};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    const RTLD_NOW: c_int = 2;

    #[cfg_attr(target_os = "linux", link(name = "dl"))]
    unsafe extern "C" {
        fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }

    pub(super) fn load(path: &Path) -> *mut c_void {
        let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
            return std::ptr::null_mut();
        };
        // SAFETY: a C string; the library's initialisers run.
        unsafe { dlopen(path.as_ptr(), RTLD_NOW) }
    }

    /// # Safety
    ///
    /// `module` must be a loaded library and `name` a C string.
    pub(super) unsafe fn symbol(module: *mut c_void, name: *const c_char) -> *mut c_void {
        // SAFETY: as the caller vouches.
        unsafe { dlsym(module, name) }
    }

    /// The export `name` of the ACE the library `module` links: `dlsym`
    /// looks through the libraries it needs too.
    ///
    /// # Safety
    ///
    /// `module` must be a loaded library, and `F` the export's type.
    pub(super) unsafe fn ace_symbol<F: Copy>(module: usize, name: &str) -> Option<F> {
        // SAFETY: as the caller vouches.
        unsafe { super::symbol(module, name) }
    }
}

#[cfg(windows)]
mod sys {
    use std::ffi::{c_char, c_void};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    }

    fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
        text.encode_wide().chain([0]).collect()
    }

    pub(super) fn load(path: &Path) -> *mut c_void {
        let name = wide(path.as_os_str());
        // SAFETY: a NUL-terminated wide string; the library's `DllMain` runs.
        unsafe { LoadLibraryW(name.as_ptr()) }
    }

    /// # Safety
    ///
    /// `module` must be a loaded library and `name` a C string.
    pub(super) unsafe fn symbol(module: *mut c_void, name: *const c_char) -> *mut c_void {
        // SAFETY: as the caller vouches.
        unsafe { GetProcAddress(module, name) }
    }

    /// The export `name` of the ACE the library links, which is loaded by
    /// now: `ace64.dll` or `ace32.dll`, as wide as this process.
    ///
    /// # Safety
    ///
    /// `F` must be the export's type.
    pub(super) unsafe fn ace_symbol<F: Copy>(_module: usize, name: &str) -> Option<F> {
        let ace = match usize::BITS {
            64 => "ace64.dll",
            _ => "ace32.dll",
        };
        // SAFETY: a NUL-terminated wide string.
        let ace = unsafe { GetModuleHandleW(wide(ace.as_ref()).as_ptr()) };
        if ace.is_null() {
            return None;
        }
        // SAFETY: as the caller vouches.
        unsafe { super::symbol(ace as usize, name) }
    }
}
