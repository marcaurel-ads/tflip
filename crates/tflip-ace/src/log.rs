//! Diagnostic output in the style of Wine's `WINEDEBUG`.
//!
//! `TFLIP_DEBUG` is a comma-separated list of channels, each optionally
//! prefixed with `+` (enable) or `-` (disable). The channels, on by
//! default:
//!
//! - `fixme` reports the first call of each stub;
//! - `remote` reports connections to servers, and why they failed;
//! - `sql` (off by default) reports the SQL statements clients prepare and
//!   run, and how running them ended;
//! - `err` (off by default) reports every export that fails, and its error
//!   code.
//!
//! ```text
//! fixme:ace32:AdsCacheOpenCursors(25) stub
//! remote:ace32:connected to tflipd 0.2.0 at db1:6262 for \\db1\data
//! sql:ace32:execute 0x1002 "SELECT * FROM items": Some(4099)
//! err:ace32:AdsGetFilter: 5037: No filter
//! ```
//!
//! Output goes to stderr, or appended to the file named by `TFLIP_LOG_FILE`.
//! Logging never fails the caller: write errors are ignored.

use std::fmt::{self, Debug, Write as _};
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::sync::{Mutex, OnceLock};

struct Config {
    fixme: bool,
    remote: bool,
    sql: bool,
    err: bool,
    file: Option<Mutex<File>>,
}

fn config() -> &'static Config {
    static CONFIG: OnceLock<Config> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let file = std::env::var_os("TFLIP_LOG_FILE")
            .and_then(|path| OpenOptions::new().create(true).append(true).open(path).ok())
            .map(Mutex::new);
        let spec = std::env::var("TFLIP_DEBUG").ok();
        Config {
            fixme: channel_enabled(spec.as_deref(), "fixme", true),
            remote: channel_enabled(spec.as_deref(), "remote", true),
            sql: channel_enabled(spec.as_deref(), "sql", false),
            err: channel_enabled(spec.as_deref(), "err", false),
            file,
        }
    })
}

/// Whether `channel` is on according to `spec`; the last mention wins.
fn channel_enabled(spec: Option<&str>, channel: &str, default: bool) -> bool {
    let mut on = default;
    for item in spec.unwrap_or("").split(',').map(str::trim) {
        let (value, name) = match item.as_bytes().first() {
            Some(b'-') => (false, &item[1..]),
            Some(b'+') => (true, &item[1..]),
            _ => (true, item),
        };
        if name == channel {
            on = value;
        }
    }
    on
}

/// A flags argument, logged in hex (`0x3`).
pub(crate) struct Hex<T>(pub T);

impl<T: fmt::LowerHex> Debug for Hex<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:#x}", self.0)
    }
}

/// `name(arg, arg, ...)`, with each argument in its `Debug` form.
fn format_call(name: &str, args: &[&dyn Debug]) -> String {
    let mut s = format!("{name}(");
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        let _ = write!(s, "{arg:?}");
    }
    s.push(')');
    s
}

fn write_line(line: fmt::Arguments) {
    let config = config();
    match &config.file {
        Some(file) => {
            if let Ok(mut file) = file.lock() {
                let _ = writeln!(file, "{line}");
            }
        }
        None => {
            let _ = writeln!(std::io::stderr(), "{line}");
        }
    }
}

/// Reports something about a connection to a server on the `remote`
/// channel.
pub(crate) fn remote(message: fmt::Arguments) {
    if config().remote {
        write_line(format_args!("remote:ace32:{message}"));
    }
}

/// Reports an SQL statement and how it ended on the `sql` channel.
pub(crate) fn sql(message: fmt::Arguments) {
    if config().sql {
        write_line(format_args!("sql:ace32:{message}"));
    }
}

/// Reports a failed export on the `err` channel.
pub(crate) fn err(message: fmt::Arguments) {
    if config().err {
        write_line(format_args!("err:ace32:{message}"));
    }
}

#[doc(hidden)]
pub fn fixme_stub(name: &str, args: &[&dyn Debug]) {
    if config().fixme {
        write_line(format_args!("fixme:ace32:{} stub", format_call(name, args)));
    }
}

/// Marks an export as a stub and reports its first call on the `fixme`
/// channel. `grep -rn 'stub!(' crates/tflip-ace` lists all stubs.
macro_rules! stub {
    ($name:literal $(, $arg:expr)* $(,)?) => {{
        static SEEN: ::std::sync::atomic::AtomicBool = ::std::sync::atomic::AtomicBool::new(false);
        if !SEEN.swap(true, ::std::sync::atomic::Ordering::Relaxed) {
            $crate::log::fixme_stub($name, &[$(&$arg),*]);
        }
    }};
}
pub(crate) use stub;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels() {
        assert!(channel_enabled(None, "fixme", true));
        assert!(!channel_enabled(Some("-fixme"), "fixme", true));
        assert!(channel_enabled(Some("-fixme,+fixme"), "fixme", true));
        assert!(channel_enabled(Some("trace, fixme"), "fixme", false));
        assert!(!channel_enabled(Some("+trace"), "fixme", false));
    }

    #[test]
    fn call_format() {
        assert_eq!(
            format_call("AdsCacheOpenCursors", &[&25u16]),
            "AdsCacheOpenCursors(25)"
        );
        assert_eq!(
            format_call("F", &[&1u32, &std::ptr::null::<u8>()]),
            "F(1, 0x0)"
        );
        assert_eq!(format_call("F", &[]), "F()");
        assert_eq!(format_call("F", &[&Hex(3u16)]), "F(0x3)");
    }
}
