//! The client's configuration: which servers it trusts, whether its
//! transactions are real ones, and what its relative names are relative to.
//!
//! A server is trusted by the SHA-256 fingerprint of its certificate, which
//! `tflipd` prints when it starts. Fingerprints come from:
//!
//! - `TFLIP_SERVER_FINGERPRINT`: one or more, separated by commas, for any
//!   server;
//! - the `[servers]` section of `tflip.ini` next to the DLL, or of the file
//!   `TFLIP_CONFIG` names, by server (`host:port` or `host`, regardless of
//!   case) or for any server (`default`):
//!
//! ```ini
//! [servers]
//! db1 = sha256:3f1c...
//! db2:6263 = sha256:9a0b...
//! default = sha256:77d2...
//! ```
//!
//! The file is read on every connect, so changes need no restart.
//!
//! A server whose certificate has none of these fingerprints is trusted if
//! its certificate chains to a root the operating system trusts (on Windows,
//! its certificate store) and names the server as the client named it — for a
//! `tflipd` given a certificate from a CA. The `[trust]` section changes that:
//! `system = no` trusts by fingerprint alone, and `roots` names a PEM file of
//! more roots to trust that way (a private CA's). `TFLIP_TRUST_SYSTEM` and
//! `TFLIP_TRUST_ROOTS` override them.
//!
//! ```ini
//! [trust]
//! roots = C:\certs\our-ca.pem
//! ```
//!
//! `mode` in the `[transactions]` section chooses the transactions of the
//! local server: `real` (the default), as the original's remote server has
//! them, or `count`, as the original's local server keeps them (a count of
//! levels, with every write reaching the files at once), for clients that
//! rely on that. It is read when the DLL starts.
//!
//! ```ini
//! [transactions]
//! mode = count
//! ```
//!
//! `log` in the same section is the folder of the transactions' logs, from
//! which transactions a crash interrupted are recovered (by default
//! `TFLIP_TX_LOG`, else `tflip/log` in the user's state folder: see
//! `tflip_engine::txlog`).
//!
//! The `[connection]` section bounds talking to a server, in seconds:
//! `connect_timeout` (default 5) how long making the TCP connection may take,
//! and `request_timeout` (default none) how long a single read or write may
//! block before the connection counts as lost — so a stalled server does not
//! block a client forever, as the original engine's would. A zero (or
//! unparseable) `request_timeout` means no bound; `connect_timeout` keeps its
//! default instead, since connecting is always bounded. The environment
//! variables `TFLIP_CONNECT_TIMEOUT` and `TFLIP_REQUEST_TIMEOUT` override them.
//! Both are read on every connect.
//!
//! ```ini
//! [connection]
//! request_timeout = 30
//! ```
//!
//! `compression` in the same section (or `TFLIP_COMPRESSION`) is the client's
//! compression setting for connections whose connect call does not give one —
//! `INTERNET` (the default), `ALWAYS` or `NEVER` — where the original engine
//! reads `ads.ini`'s `COMPRESSION` (see
//! `tflip_protocol::wire::compressed`). It is read on every connect.
//!
//! `commtype` in the same section (or `TFLIP_COMMTYPE`), as `ignore`, lets a
//! connect string ask for a transport without TLS (`CommType=TCP_IP`,
//! `UDP_IP`, `IPX`), as connect strings written for the original often do:
//! the setting is logged and ignored, and the connection is made over TLS
//! as every connection to `tflipd` is. By default such a connect string is
//! refused (`AE_INVALID_OPTION`). It is read on every connect.
//!
//! ```ini
//! [connection]
//! commtype = ignore
//! ```
//!
//! `version` in the `[compat]` section is the description `AdsGetVersion`
//! gives, for clients that check it: `original` for the original's
//! (`Advantage 32-bit Client Engine for Windows`, which its 64-bit DLL gives
//! too), or any other text as it is; tflip's own
//! (`tflip 0.27.0 i686-pc-windows-gnu`, its release and the target it is
//! built for) by default.
//! It is read once.
//!
//! ```ini
//! [compat]
//! version = original
//! ```
//!
//! `local` in the `[cache]` section (or `TFLIP_LOCAL_CACHE`) chooses whether
//! the local server's connections keep closed tables open for reuse
//! (`AdsCacheOpenTables`, `AdsCacheOpenCursors`) as the original's local
//! server and `tflipd` do: `yes` (or `true`, `on`, `1`), or `no`, the
//! default, which keeps none, so that a table a client closed is closed —
//! another program can then open it exclusively (or replace its files) at
//! once, where with caching on that waits until the client closes its
//! cached tables (`AdsCloseCachedTables`) or its connection; within the
//! process, an exclusive open makes the caches let go of the file, as in the
//! original engine. It is read when the DLL starts.
//!
//! ```ini
//! [cache]
//! local = yes
//! ```
//!
//! `relative` in the `[paths]` section chooses what relative table names,
//! table listing masks and index file names with a folder in them are
//! relative to on the local server: `connection` (the default), the
//! connection's folder, as on a server, or `cwd`, the process's current
//! directory, as the original's local server takes them, for clients that
//! rely on that (`docs/DIFFERENCES.md`). It is read when the DLL starts.
//!
//! ```ini
//! [paths]
//! relative = cwd
//! ```

use std::path::PathBuf;
use std::time::Duration;

use tflip_protocol::tls::{self, Fingerprint, Trust};
use tflip_protocol::wire::Compression;

/// The fingerprints of the certificates to trust for the server at
/// `host`:`port`.
pub(crate) fn fingerprints(host: &str, port: u16) -> Vec<Fingerprint> {
    let mut found: Vec<Fingerprint> = Vec::new();
    #[cfg(test)]
    found.extend(
        TEST_FINGERPRINTS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .copied(),
    );
    if let Ok(list) = std::env::var("TFLIP_SERVER_FINGERPRINT") {
        found.extend(list.split(',').filter_map(tls::parse_fingerprint));
    }
    if let Some(text) = config_file().and_then(|path| std::fs::read_to_string(path).ok()) {
        found.extend(from_ini(&text, host, port));
    }
    found
}

/// Which servers to trust at `host`:`port`: those with one of the
/// [`fingerprints`], and those whose certificates chain to the system's roots
/// (unless `[trust] system = no` / `TFLIP_TRUST_SYSTEM=no`) or to those in the
/// PEM file `[trust] roots` / `TFLIP_TRUST_ROOTS` names. A roots file that
/// cannot be read is logged and left out.
pub(crate) fn trust(host: &str, port: u16) -> Trust {
    let text = config_file().and_then(|path| std::fs::read_to_string(path).ok());
    let setting = |env: &str, key: &str| {
        std::env::var(env)
            .ok()
            .or_else(|| ini_value(text.as_deref()?, "trust", key))
    };
    let system = setting("TFLIP_TRUST_SYSTEM", "system").is_none_or(|value| {
        !["no", "false", "0", "off"]
            .iter()
            .any(|no| value.trim().eq_ignore_ascii_case(no))
    });
    let mut roots = Vec::new();
    if let Some(file) = setting("TFLIP_TRUST_ROOTS", "roots").filter(|file| !file.is_empty()) {
        let read = std::fs::read(&file).map_err(|error| error.to_string());
        match read.and_then(|pem| tls::certificates_from_pem(&pem)) {
            Ok(found) => roots = found,
            Err(error) => crate::log::remote(format_args!("trusted roots {file}: {error}")),
        }
    }
    Trust {
        fingerprints: fingerprints(host, port),
        system,
        roots,
    }
}

/// Fingerprints the tests trust, for the server they start.
#[cfg(test)]
pub(crate) static TEST_FINGERPRINTS: std::sync::Mutex<Vec<Fingerprint>> =
    std::sync::Mutex::new(Vec::new());

/// The fingerprints `[servers]` of an ini file lists for `host`:`port`.
fn from_ini(text: &str, host: &str, port: u16) -> Vec<Fingerprint> {
    let names = [
        format!("{host}:{port}"),
        host.to_owned(),
        "default".to_owned(),
    ];
    let mut section = String::new();
    let mut found = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with([';', '#']) {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            section = name.trim().to_ascii_lowercase();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if section == "servers"
            && names
                .iter()
                .any(|name| key.trim().eq_ignore_ascii_case(name))
        {
            found.extend(value.split(',').filter_map(tls::parse_fingerprint));
        }
    }
    found
}

/// Whether the local server's transactions are real ones: unless
/// `[transactions]` says `mode = count`.
pub(crate) fn real_transactions() -> bool {
    let text = config_file().and_then(|path| std::fs::read_to_string(path).ok());
    text.and_then(|text| ini_value(&text, "transactions", "mode"))
        .is_none_or(|mode| !mode.eq_ignore_ascii_case("count"))
}

/// Whether relative names on the local server are relative to the
/// process's current directory, as in the original: `[paths]` says
/// `relative = cwd`; else (the default) to the connection's folder.
pub(crate) fn cwd_relative() -> bool {
    let text = config_file().and_then(|path| std::fs::read_to_string(path).ok());
    text.is_some_and(|text| cwd_relative_in(&text))
}

/// Whether the ini file `text` says `[paths] relative = cwd`.
fn cwd_relative_in(text: &str) -> bool {
    ini_value(text, "paths", "relative").is_some_and(|base| base.trim().eq_ignore_ascii_case("cwd"))
}

/// Whether the local server's connections keep closed tables open for
/// reuse: `TFLIP_LOCAL_CACHE`, else `[cache] local`, as `yes`, `true`, `on`
/// or `1` (in any case); anything else, or nothing, keeps none.
pub(crate) fn local_caching() -> bool {
    let value = std::env::var("TFLIP_LOCAL_CACHE").ok().or_else(|| {
        let text = config_file().and_then(|path| std::fs::read_to_string(path).ok())?;
        ini_value(&text, "cache", "local")
    });
    value.is_some_and(|value| is_yes(&value))
}

/// Whether a setting's `value` says yes.
fn is_yes(value: &str) -> bool {
    ["yes", "true", "on", "1"]
        .iter()
        .any(|yes| value.trim().eq_ignore_ascii_case(yes))
}

/// How long making the TCP connection to a server may take before it counts
/// as not found: `TFLIP_CONNECT_TIMEOUT`, else `[connection] connect_timeout`
/// (both in seconds), else [`DEFAULT_CONNECT_TIMEOUT`]. A zero, missing or
/// unparseable value keeps the default; connecting is always bounded (the
/// socket API cannot wait forever for a connection anyway).
pub(crate) fn connect_timeout() -> Duration {
    timeout_setting("TFLIP_CONNECT_TIMEOUT", "connect_timeout")
        .flatten()
        .filter(|timeout| !timeout.is_zero())
        .unwrap_or(DEFAULT_CONNECT_TIMEOUT)
}

/// How long a single read or write to a server may block before the
/// connection counts as lost: `TFLIP_REQUEST_TIMEOUT`, else
/// `[connection] request_timeout` (both in seconds), else none (block as long
/// as the original engine's client does). Zero also means no bound.
pub(crate) fn request_timeout() -> Option<Duration> {
    timeout_setting("TFLIP_REQUEST_TIMEOUT", "request_timeout")
        .flatten()
        .filter(|timeout| !timeout.is_zero())
}

/// The client's compression setting for a connection whose connect call
/// Whether a connect string's `CommType` that asks for a transport without
/// TLS (`TCP_IP`, `UDP_IP`, `IPX`) is ignored, the connection going ahead
/// over TLS as every connection to `tflipd` does, rather than refused:
/// `TFLIP_COMMTYPE`, else `[connection] commtype`, as `ignore`.
pub(crate) fn ignore_comm_type() -> bool {
    let value = std::env::var("TFLIP_COMMTYPE").ok().or_else(|| {
        let text = config_file().and_then(|path| std::fs::read_to_string(path).ok())?;
        ini_value(&text, "connection", "commtype")
    });
    value.is_some_and(|value| value.trim().eq_ignore_ascii_case("ignore"))
}

/// The original's `AdsGetVersion` description, which both its `ace32.dll`
/// and its `ace64.dll` give.
pub(crate) const ORIGINAL_DESCRIPTION: &str = "Advantage 32-bit Client Engine for Windows";

/// The description `AdsGetVersion` gives: `[compat] version` (`original`
/// for the original's, any other text as it is), else `None`, tflip's own.
/// Read once.
pub(crate) fn version_description() -> Option<&'static str> {
    static DESCRIPTION: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    DESCRIPTION
        .get_or_init(|| {
            let text = config_file().and_then(|path| std::fs::read_to_string(path).ok())?;
            description_in(&text)
        })
        .as_deref()
}

/// [`version_description`] of the ini file `text`.
fn description_in(text: &str) -> Option<String> {
    let value = ini_value(text, "compat", "version")?;
    let value = value.trim();
    match value {
        "" => None,
        _ if value.eq_ignore_ascii_case("original") => Some(ORIGINAL_DESCRIPTION.to_owned()),
        _ => Some(value.to_owned()),
    }
}

/// gives none: `TFLIP_COMPRESSION`, else `[connection] compression`, else
/// [`Compression::Internet`] (the original's default). A value that is not
/// `INTERNET`, `ALWAYS` or `NEVER` is logged and the default kept.
pub(crate) fn compression() -> Compression {
    let value = std::env::var("TFLIP_COMPRESSION").ok().or_else(|| {
        let text = config_file().and_then(|path| std::fs::read_to_string(path).ok())?;
        ini_value(&text, "connection", "compression")
    });
    let Some(value) = value else {
        return Compression::default();
    };
    Compression::parse(&value).unwrap_or_else(|| {
        crate::log::remote(format_args!(
            "compression = {value} is not INTERNET, ALWAYS or NEVER; using INTERNET"
        ));
        Compression::default()
    })
}

/// The default [`connect_timeout`].
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// A timeout in seconds from the environment variable `env`, else the
/// `[connection]` key `key` of the ini file. The outer `Option` is whether a
/// setting was found at all (so a default can apply); the inner one is `None`
/// for a non-numeric value, else the duration (which may be zero). A fractional
/// value is allowed (`0.5`).
fn timeout_setting(env: &str, key: &str) -> Option<Option<Duration>> {
    let value = match std::env::var(env) {
        Ok(value) => value,
        Err(_) => {
            let text = config_file().and_then(|path| std::fs::read_to_string(path).ok())?;
            ini_value(&text, "connection", key)?
        }
    };
    Some(parse_timeout(&value))
}

/// A timeout in (possibly fractional) seconds, or `None` if `value` is not a
/// non-negative finite number.
fn parse_timeout(value: &str) -> Option<Duration> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|secs| secs.is_finite() && *secs >= 0.0)
        .map(Duration::from_secs_f64)
}

/// The folder of the transactions' logs, if `[transactions]` names one.
pub(crate) fn transaction_log() -> Option<PathBuf> {
    let text = config_file().and_then(|path| std::fs::read_to_string(path).ok())?;
    ini_value(&text, "transactions", "log")
        .filter(|log| !log.is_empty())
        .map(PathBuf::from)
}

/// The value of `key` in the section `section` of an ini file's `text`.
fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut current = String::new();
    for line in text.lines().map(str::trim) {
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            current = name.trim().to_ascii_lowercase();
            continue;
        }
        let Some((k, value)) = line.split_once('=') else {
            continue;
        };
        if current == section && k.trim().eq_ignore_ascii_case(key) {
            return Some(value.trim().to_owned());
        }
    }
    None
}

/// The folder of the original engine's collation files (`ansi.chr`,
/// `adscollate.adt`) and its `adslocal.cfg`: `TFLIP_COLLATION_DIR`, or the
/// DLL's folder, where the original engine keeps them.
pub(crate) fn collation_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("TFLIP_COLLATION_DIR") {
        return Some(PathBuf::from(dir));
    }
    // The tests take the Advantage SDK's under Wine, where this machine has
    // it.
    #[cfg(test)]
    if let Some(home) = std::env::var_os("HOME") {
        let sdk = PathBuf::from(home)
            .join("src/ace-sdk/drive_c/Program Files (x86)/Advantage 12.0/acesdk");
        if sdk.join("adscollate.adt").exists() {
            return Some(sdk);
        }
    }
    Some(sys::module_path()?.parent()?.to_owned())
}

/// The configuration file: `TFLIP_CONFIG`, or `tflip.ini` next to the DLL.
fn config_file() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("TFLIP_CONFIG") {
        return Some(PathBuf::from(path));
    }
    Some(sys::module_path()?.with_file_name("tflip.ini"))
}

#[cfg(windows)]
mod sys {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::path::PathBuf;

    const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x4;
    const GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT: u32 = 0x2;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut isize) -> i32;
        fn GetModuleFileNameW(module: isize, name: *mut u16, size: u32) -> u32;
    }

    /// The path of this DLL.
    pub(super) fn module_path() -> Option<PathBuf> {
        let mut module = 0;
        let flags =
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
        let address = module_path as *const u16;
        // SAFETY: with FROM_ADDRESS, `name` is an address inside this module.
        if unsafe { GetModuleHandleExW(flags, address, &mut module) } == 0 {
            return None;
        }
        let mut name = vec![0u16; 32768];
        // SAFETY: `name` holds `name.len()` units.
        let len = unsafe { GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) };
        if len == 0 || len as usize >= name.len() {
            return None;
        }
        Some(PathBuf::from(OsString::from_wide(&name[..len as usize])))
    }
}

#[cfg(not(windows))]
mod sys {
    use std::path::PathBuf;

    /// Elsewhere (the host tests), there is no DLL: the executable.
    pub(super) fn module_path() -> Option<PathBuf> {
        std::env::current_exe().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_by_server() {
        let a = format!("sha256:{}", "a1".repeat(32));
        let b = format!("sha256:{}", "b2".repeat(32));
        let c = "cc".repeat(32);
        let ini = format!(
            "; servers\n[other]\ndb1 = {a}\n[Servers]\nDB1 = {a}\ndb1:6263 = {b}\n\
             default = {c}, nonsense\n"
        );
        let found = |host, port| from_ini(&ini, host, port);
        let (a, b, c) = (
            tls::parse_fingerprint(&a).unwrap(),
            tls::parse_fingerprint(&b).unwrap(),
            tls::parse_fingerprint(&c).unwrap(),
        );
        assert_eq!(found("db1", 6262), [a, c]);
        assert_eq!(found("db1", 6263), [a, b, c]);
        assert_eq!(found("db2", 6262), [c]);
        assert!(from_ini("db1 = x", "db1", 6262).is_empty());
    }

    #[test]
    fn timeouts_parse() {
        assert_eq!(parse_timeout("30"), Some(Duration::from_secs(30)));
        assert_eq!(parse_timeout(" 0.5 "), Some(Duration::from_millis(500)));
        assert_eq!(parse_timeout("0"), Some(Duration::ZERO));
        assert_eq!(parse_timeout(""), None);
        assert_eq!(parse_timeout("forever"), None);
        assert_eq!(parse_timeout("-1"), None);
        assert_eq!(parse_timeout("inf"), None);
    }

    #[test]
    fn version_description_from_ini() {
        assert_eq!(
            description_in("[compat]\nversion = Original\n").as_deref(),
            Some(ORIGINAL_DESCRIPTION)
        );
        assert_eq!(
            description_in("[compat]\nversion = Advantage 64-bit Client Engine\n").as_deref(),
            Some("Advantage 64-bit Client Engine")
        );
        assert_eq!(description_in("[compat]\nversion =\n"), None);
        assert_eq!(description_in("[cache]\nversion = original\n"), None);
    }

    #[test]
    fn cwd_relative_from_ini() {
        assert!(cwd_relative_in("[paths]\nrelative = CWD\n"));
        assert!(!cwd_relative_in("[paths]\nrelative = connection\n"));
        assert!(!cwd_relative_in("[cache]\nrelative = cwd\n"));
        assert!(!cwd_relative_in(""));
    }

    #[test]
    fn local_caching_from_ini() {
        let ini = "[cache]\nlocal = Yes\n";
        assert!(ini_value(ini, "cache", "local").is_some_and(|value| is_yes(&value)));
        assert!(
            ["yes", " ON ", "true", "1"]
                .iter()
                .all(|value| is_yes(value))
        );
        assert!(
            !["no", "off", "0", "", "maybe"]
                .iter()
                .any(|value| is_yes(value))
        );
    }

    #[test]
    fn connection_timeout_from_ini() {
        let ini = "[connection]\nconnect_timeout = 12\nrequest_timeout = 0\n";
        assert_eq!(
            ini_value(ini, "connection", "connect_timeout").as_deref(),
            Some("12")
        );
        assert_eq!(
            ini_value(ini, "connection", "request_timeout").as_deref(),
            Some("0")
        );
        assert_eq!(ini_value(ini, "connection", "missing"), None);
    }
}
