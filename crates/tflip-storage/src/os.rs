//! The calls into the operating system, the crate's only unsafe code, each
//! behind a safe function: byte-range locks on files ([`crate::lock`]) and,
//! on Windows, the user locale's order and case
//! ([`crate::collation::user_locale_default`]) and how it names a path
//! ([`universal_path`](crate::universal_path)).

pub(crate) mod file_lock {
    pub(crate) use imp::{lock, unlock};

    #[cfg(unix)]
    mod imp {
        use std::fs::File;
        use std::io;
        use std::os::fd::AsRawFd;

        #[cfg(target_os = "linux")]
        const SET: libc::c_int = libc::F_OFD_SETLK;
        #[cfg(target_os = "linux")]
        const SET_WAIT: libc::c_int = libc::F_OFD_SETLKW;
        #[cfg(not(target_os = "linux"))]
        const SET: libc::c_int = libc::F_SETLK;
        #[cfg(not(target_os = "linux"))]
        const SET_WAIT: libc::c_int = libc::F_SETLKW;

        fn set(
            file: &File,
            offset: u64,
            length: u64,
            kind: libc::c_int,
            wait: bool,
        ) -> io::Result<bool> {
            // SAFETY: flock is plain data; all-zero is a valid value for it.
            let mut lock: libc::flock = unsafe { std::mem::zeroed() };
            lock.l_type = kind as _;
            lock.l_whence = libc::SEEK_SET as _;
            lock.l_start = offset as libc::off_t;
            lock.l_len = length as libc::off_t;
            let command = if wait { SET_WAIT } else { SET };
            loop {
                // SAFETY: the descriptor is open for the life of `file`, and
                // `lock` is a valid flock.
                if unsafe { libc::fcntl(file.as_raw_fd(), command, &lock) } == 0 {
                    return Ok(true);
                }
                let error = io::Error::last_os_error();
                match error.raw_os_error() {
                    Some(libc::EINTR) => continue,
                    Some(libc::EAGAIN | libc::EACCES) if !wait => return Ok(false),
                    _ => return Err(error),
                }
            }
        }

        pub(crate) fn lock(
            file: &File,
            offset: u64,
            length: u64,
            exclusive: bool,
            wait: bool,
        ) -> io::Result<bool> {
            let kind = if exclusive {
                libc::F_WRLCK
            } else {
                libc::F_RDLCK
            };
            set(file, offset, length, kind as libc::c_int, wait)
        }

        pub(crate) fn unlock(file: &File, offset: u64, length: u64) -> io::Result<()> {
            set(file, offset, length, libc::F_UNLCK as libc::c_int, false).map(|_| ())
        }
    }

    #[cfg(windows)]
    mod imp {
        use std::fs::File;
        use std::io;
        use std::os::windows::io::AsRawHandle;

        use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
        use windows_sys::Win32::Storage::FileSystem::{
            LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx, UnlockFileEx,
        };
        use windows_sys::Win32::System::IO::OVERLAPPED;

        fn overlapped(offset: u64) -> OVERLAPPED {
            // SAFETY: OVERLAPPED is plain data; all-zero is a valid value for it.
            let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
            overlapped.Anonymous.Anonymous.Offset = offset as u32;
            overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
            overlapped
        }

        pub(crate) fn lock(
            file: &File,
            offset: u64,
            length: u64,
            exclusive: bool,
            wait: bool,
        ) -> io::Result<bool> {
            let mut flags = 0;
            if exclusive {
                flags |= LOCKFILE_EXCLUSIVE_LOCK;
            }
            if !wait {
                flags |= LOCKFILE_FAIL_IMMEDIATELY;
            }
            let mut overlapped = overlapped(offset);
            // SAFETY: the handle is open for the life of `file`, and the
            // OVERLAPPED is valid for the call (the file is not opened for
            // overlapped I/O, so the call completes before it returns).
            let done = unsafe {
                LockFileEx(
                    file.as_raw_handle() as _,
                    flags,
                    0,
                    length as u32,
                    (length >> 32) as u32,
                    &mut overlapped,
                )
            };
            if done != 0 {
                return Ok(true);
            }
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(code) if code as u32 == ERROR_LOCK_VIOLATION && !wait => Ok(false),
                _ => Err(error),
            }
        }

        pub(crate) fn unlock(file: &File, offset: u64, length: u64) -> io::Result<()> {
            let mut overlapped = overlapped(offset);
            // SAFETY: as for lock.
            let done = unsafe {
                UnlockFileEx(
                    file.as_raw_handle() as _,
                    0,
                    length as u32,
                    (length >> 32) as u32,
                    &mut overlapped,
                )
            };
            if done != 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
    }
}

#[cfg(windows)]
pub(crate) mod locale {
    const LOCALE_USER_DEFAULT: u32 = 0x0400;
    const LOCALE_SABBREVLANGNAME: u32 = 0x0003;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocaleInfoA(locale: u32, kind: u32, data: *mut u8, len: i32) -> i32;
        fn CompareStringA(
            locale: u32,
            flags: u32,
            a: *const u8,
            a_len: i32,
            b: *const u8,
            b_len: i32,
        ) -> i32;
        fn LCMapStringA(
            locale: u32,
            flags: u32,
            src: *const u8,
            src_len: i32,
            dest: *mut u8,
            dest_len: i32,
        ) -> i32;
    }

    /// The user locale's language abbreviation (`ENU`, `DEU`, ...), empty if
    /// Windows gives none.
    pub(crate) fn language_abbreviation() -> Vec<u8> {
        let mut abbreviation = [0u8; 16];
        // SAFETY: the buffer holds the 16 bytes the call is told it has.
        let len = unsafe {
            GetLocaleInfoA(
                LOCALE_USER_DEFAULT,
                LOCALE_SABBREVLANGNAME,
                abbreviation.as_mut_ptr(),
                16,
            )
        };
        abbreviation[..usize::try_from(len).unwrap_or(0).saturating_sub(1)].to_vec()
    }

    /// `CompareStringA` of the bytes `a` and `b` under the user locale: 0 on
    /// failure, else `CSTR_LESS_THAN`, `CSTR_EQUAL` or `CSTR_GREATER_THAN`.
    pub(crate) fn compare(a: u8, b: u8) -> i32 {
        // SAFETY: both pointers are to one live byte, as the lengths say.
        unsafe { CompareStringA(LOCALE_USER_DEFAULT, 0, &a, 1, &b, 1) }
    }

    /// `LCMapStringA` of the byte `source` under the user locale with
    /// `flags`; `source` itself if the call fails.
    pub(crate) fn map(flags: u32, source: u8) -> u8 {
        let mut mapped = source;
        // SAFETY: both pointers are to one live byte, as the lengths say.
        unsafe { LCMapStringA(LOCALE_USER_DEFAULT, flags, &source, 1, &mut mapped, 1) };
        mapped
    }
}

pub mod path {
    /// `path` as the original engine on Windows makes a connection's path
    /// (probed, `tools/ace/mapped`): full, through a drive mapped to a share
    /// the share's UNC path (`Y:\data` is `\\server\share\data`), and without
    /// a trailing separator. A drive's root keeps its separator, though the
    /// original reports `C:`, as `C:` alone names that drive's current
    /// folder. Elsewhere, or if Windows cannot make it full, `path` as it is.
    pub fn universal_path(path: &str) -> String {
        imp::universal(path).unwrap_or_else(|| path.to_owned())
    }

    #[cfg(not(windows))]
    mod imp {
        pub(super) fn universal(_path: &str) -> Option<String> {
            None
        }
    }

    #[cfg(windows)]
    mod imp {
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use std::path::Path;

        use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, NO_ERROR};
        use windows_sys::Win32::NetworkManagement::WNet::{
            UNIVERSAL_NAME_INFO_LEVEL, UNIVERSAL_NAME_INFOW, WNetGetUniversalNameW,
        };

        pub(super) fn universal(path: &str) -> Option<String> {
            let full = std::path::absolute(path).ok()?;
            let mut path = match network_name(&full) {
                Some(name) => name,
                None => full.to_string_lossy().into_owned(),
            };
            let drive_root = |path: &str| path.len() == 3 && path.as_bytes()[1] == b':';
            while path.ends_with(['\\', '/']) && !drive_root(&path) {
                path.pop();
            }
            Some(path)
        }

        /// The UNC path of `full` if it is on a drive mapped to a share.
        fn network_name(full: &Path) -> Option<String> {
            let wide: Vec<u16> = full.as_os_str().encode_wide().chain([0]).collect();
            // In u64s, so the UNIVERSAL_NAME_INFOW at its start is aligned.
            let mut buffer = vec![0u64; 64];
            loop {
                let mut size = (buffer.len() * 8) as u32;
                // SAFETY: `wide` is NUL-terminated and `buffer` holds the
                // `size` bytes the call is told it has.
                let rc = unsafe {
                    WNetGetUniversalNameW(
                        wide.as_ptr(),
                        UNIVERSAL_NAME_INFO_LEVEL,
                        buffer.as_mut_ptr().cast(),
                        &mut size,
                    )
                };
                match rc {
                    NO_ERROR => break,
                    ERROR_MORE_DATA => buffer.resize((size as usize).div_ceil(8), 0),
                    _ => return None,
                }
            }
            // SAFETY: on success the buffer starts with a UNIVERSAL_NAME_INFOW
            // whose name is a NUL-terminated string within the buffer.
            let name = unsafe {
                let name = (*buffer.as_ptr().cast::<UNIVERSAL_NAME_INFOW>()).lpUniversalName;
                let len = (0..).take_while(|&i| *name.add(i) != 0).count();
                std::slice::from_raw_parts(name, len)
            };
            Some(OsString::from_wide(name).to_string_lossy().into_owned())
        }
    }
}
