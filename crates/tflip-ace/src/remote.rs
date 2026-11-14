//! Connecting to `tflipd`.
//!
//! A connection goes to a server when the client allows a remote server
//! (`ADS_REMOTE_SERVER` or `ADS_AIS_SERVER`) and names its data by a server
//! path, `\\server\share\folder` or `\\server:port\share\folder` (port
//! [`DEFAULT_PORT`] if not given). The server finds the share; the client
//! trusts the server by its certificate's fingerprint, or by its certificate's
//! chain to a trusted root (see [`crate::config`]).

use tflip_engine::connection::LOCAL as LOCAL_SERVER;
use tflip_engine::handle::Handle;
use tflip_engine::{Error, settings};
use tflip_protocol::client::{Client, ConnectOptions};
use tflip_protocol::wire::Compression;
use tflip_protocol::{Api, DEFAULT_PORT};

use crate::LOCAL;
use crate::backend::{self, Remote};
use crate::{config, log};

/// A server path taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServerPath {
    pub host: String,
    pub port: u16,
    /// The share and the folder in it, as the server is to find them
    /// (`share\folder`).
    pub path: String,
}

fn is_separator(c: char) -> bool {
    c == '\\' || c == '/'
}

/// `path` as a server path, or `None` if it is not one. Device paths
/// (`\\?\...`, `\\.\...`) are not server paths.
pub(crate) fn server_path(path: &str) -> Option<ServerPath> {
    let mut chars = path.chars();
    if !(chars.next().is_some_and(is_separator) && chars.next().is_some_and(is_separator)) {
        return None;
    }
    let rest = &path[2..];
    let (server, path) = rest.split_once(is_separator).unwrap_or((rest, ""));
    // `[::1]:6262` or `host:6262`.
    let (host, port) = match server.strip_prefix('[') {
        Some(bracketed) => {
            let (host, after) = bracketed.split_once(']')?;
            (host, after.strip_prefix(':'))
        }
        None => match server.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (server, None),
        },
    };
    if host.is_empty() || host == "?" || host == "." {
        return None;
    }
    let port = match port {
        Some(port) => port.parse().ok()?,
        None => DEFAULT_PORT,
    };
    Some(ServerPath {
        host: host.to_owned(),
        port,
        path: path.to_owned(),
    })
}

/// `name`, if it starts with a drive (`Y:\data\orders.adt`), as the
/// original engine names it (see [`tflip_storage::universal_path`]), so a
/// drive mapped to a share names it by its server path; any other name as it
/// is, as a relative name is the connection's and a server name no path.
pub(crate) fn universal_name(name: &str) -> String {
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic() => {
            tflip_storage::universal_path(name)
        }
        _ => name.to_owned(),
    }
}

/// Checks the communication settings an `AdsConnect101` connect string carries
/// against what `tflipd` provides, for a connection that is about to go to a
/// server, and logs on the `remote` channel the ones it cannot honour.
///
/// `tflipd` always speaks TLS 1.2 or 1.3 and the client trusts a server by
/// its certificate's fingerprint or chain (see [`crate::config`]), so a
/// request to use a
/// non-TLS transport — `CommType=TCP_IP`, `UDP_IP` or `IPX` — is refused with
/// [`Error::InvalidOption`] rather than silently given TLS. The TLS cipher list
/// (the original's are all TLS 1.0), certificate, common name and FIPS mode
/// cannot be applied either, but do not change that the connection is
/// encrypted, so they are logged and accepted. `CommType=TLS`, or none, is what
/// `tflipd` does anyway.
pub(crate) fn check_comm(connect_string: &str) -> Result<(), Error> {
    check_comm_with(connect_string, crate::config::ignore_comm_type())
}

/// [`check_comm`], a transport without TLS ignored (logged, the connection
/// made over TLS) if `ignore` (`[connection] commtype = ignore`).
fn check_comm_with(connect_string: &str, ignore: bool) -> Result<(), Error> {
    use tflip_engine::connection::connect_string_value as value;
    if let Some(comm) = value(connect_string, "commtype")
        && !comm.eq_ignore_ascii_case("tls")
    {
        if ignore {
            log::remote(format_args!(
                "CommType={comm} asks for a connection without TLS; connecting over TLS"
            ));
        } else {
            log::remote(format_args!(
                "CommType={comm} asks for a connection without TLS, which tflipd does not offer"
            ));
            return Err(Error::InvalidOption);
        }
    }
    let ignored = [
        ("TLSCiphers", value(connect_string, "tlsciphers")),
        ("TLSCertificate", value(connect_string, "tlscertificate")),
        ("TLSCommonName", value(connect_string, "tlscommonname")),
        // TDataset always sends FIPS; only an on request means anything.
        (
            "FIPS",
            value(connect_string, "fips").filter(|v| v.eq_ignore_ascii_case("true")),
        ),
    ];
    for (name, set) in ignored
        .into_iter()
        .filter_map(|(name, v)| v.map(|v| (name, v)))
    {
        log::remote(format_args!(
            "{name}={set} is accepted but not applied; tflipd uses its own TLS and trusts by \
             fingerprint or by tflip.ini's [trust]"
        ));
    }
    Ok(())
}

/// How a client connects to a server: as which user, and how it asks for
/// compression.
pub(crate) struct How<'a> {
    pub user: Option<&'a str>,
    pub password: Option<&'a str>,
    /// The dictionary password of an AES dictionary (`DDPassword`).
    pub dd_password: Option<&'a str>,
    /// The connect string's `EncryptionType` (0 for none), which the server's
    /// connection takes for the tables it creates.
    pub encryption_type: u8,
    /// The client's compression setting.
    pub compression: Compression,
    /// Whether the client connects as to an Internet server.
    pub internet: bool,
}

/// Opens a connection to `server` on a server, for the path the client gave
/// (`given`). The server's session gets the client's settings first.
pub(crate) fn connect(server: &ServerPath, given: &str, how: &How) -> Result<Handle, Error> {
    let How {
        user,
        password,
        dd_password,
        encryption_type,
        compression,
        internet,
    } = *how;
    let ServerPath { host, port, path } = server;
    let base = backend::reserve_base();
    let trust = crate::config::trust(host, *port);
    if trust.is_empty() {
        log::remote(format_args!(
            "nothing trusts {host}:{port}; set TFLIP_SERVER_FINGERPRINT or add it to tflip.ini"
        ));
        return Err(Error::RemoteNotFound);
    }
    let pinned = !trust.fingerprints.is_empty();
    let name = concat!("tflip-ace ", env!("CARGO_PKG_VERSION"));
    let options = ConnectOptions {
        request_timeout: config::request_timeout(),
        compression,
        internet,
        ..ConnectOptions::new(trust, config::connect_timeout())
    };
    let client = Client::connect(host, *port, base, name, &options).map_err(|error| {
        let hint = if pinned || !error.reason.contains("TLS") {
            ""
        } else {
            " (no fingerprint is configured for it: set TFLIP_SERVER_FINGERPRINT or add it to \
             tflip.ini)"
        };
        log::remote(format_args!(
            "cannot connect to {host}:{port}: {}{hint}",
            error.reason
        ));
        error.error
    })?;
    client.set_date_format(0, settings::date_format(&LOCAL))?;
    client.set_epoch(settings::epoch(&LOCAL))?;
    client.set_show_deleted(settings::show_deleted(&LOCAL))?;
    client.set_exact(0, settings::exact(&LOCAL))?;
    let connection = client
        .connect_path(
            path.clone(),
            LOCAL_SERVER,
            user.map(str::to_owned),
            password.map(str::to_owned),
            dd_password.map(str::to_owned),
        )
        .inspect_err(|error| {
            log::remote(format_args!("{host}:{port} refused {path:?}: {error:?}"));
        })?;
    if encryption_type != 0 {
        client.set_encryption_type(connection, encryption_type)?;
    }
    // The connection keeps closed tables open as the client asked (see
    // `AdsCacheOpenTables`).
    for (cursors, size) in [false, true]
        .into_iter()
        .zip(crate::settings::cache_sizes())
    {
        if let Some(size) = size {
            client.set_cache_size(connection, cursors, size)?;
        }
    }
    let compressed = if client.compressed() {
        ", compressed"
    } else {
        ""
    };
    log::remote(format_args!(
        "connected to {} at {host}:{port} for {given}{compressed}",
        client.server()
    ));
    backend::register(
        base,
        Remote {
            client,
            connection,
            path: given.to_owned(),
        },
    );
    Ok(connection)
}

/// `name`, a table or a table listing's mask on the connection `connection`,
/// as the server takes it: relative to the connection's folder. Clients
/// such as ARC name them by full path (`\\server\share\folder\*.adt`, or
/// through a mapped drive); one inside the folder of a remote connection
/// loses that folder. Any other name is passed on as it is, for the server
/// to check.
pub(crate) fn relative_name(connection: Handle, name: &str) -> String {
    let backend::Backend::Remote(remote) = backend::api(connection) else {
        return name.to_owned();
    };
    let universal = universal_name(name);
    match (server_path(&remote.path), server_path(&universal)) {
        (Some(folder), Some(full)) if same_host(&folder, &full) => {
            within(&folder.path, &full.path).unwrap_or(name).to_owned()
        }
        _ => name.to_owned(),
    }
}

fn same_host(a: &ServerPath, b: &ServerPath) -> bool {
    a.host.eq_ignore_ascii_case(&b.host) && a.port == b.port
}

/// The rest of `path` after the folder `folder`, both share paths, compared
/// regardless of case and of the separators used; `None` if `path` is not
/// inside `folder`.
fn within<'p>(folder: &str, path: &'p str) -> Option<&'p str> {
    let parts = |text: &str| -> Vec<String> {
        text.split(is_separator)
            .filter(|part| !part.is_empty())
            .map(str::to_lowercase)
            .collect()
    };
    let folder = parts(folder);
    let mut rest = path;
    for part in &folder {
        rest = rest.trim_start_matches(is_separator);
        let end = rest.find(is_separator).unwrap_or(rest.len());
        if rest[..end].to_lowercase() != *part {
            return None;
        }
        rest = &rest[end..];
    }
    Some(rest.trim_start_matches(is_separator))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(host: &str, port: u16, path: &str) -> Option<ServerPath> {
        Some(ServerPath {
            host: host.into(),
            port,
            path: path.into(),
        })
    }

    #[test]
    fn server_paths() {
        assert_eq!(
            server_path(r"\\db1\data\sales"),
            parsed("db1", 6262, r"data\sales")
        );
        assert_eq!(server_path(r"\\db1:6263\data"), parsed("db1", 6263, "data"));
        assert_eq!(server_path("//db1/data/"), parsed("db1", 6262, "data/"));
        assert_eq!(
            server_path(r"\\[::1]:7000\data"),
            parsed("::1", 7000, "data")
        );
        assert_eq!(server_path(r"\\[::1]\data"), parsed("::1", 6262, "data"));
        assert_eq!(server_path(r"\\db1"), parsed("db1", 6262, ""));
        for local in [
            r"C:\data",
            r"data\x",
            r"\data",
            r"\\?\C:\data",
            r"\\.\pipe",
            r"\\",
            "",
        ] {
            assert_eq!(server_path(local), None, "{local}");
        }
        assert_eq!(server_path(r"\\db1:port\data"), None);
    }

    #[test]
    fn comm_settings_refuse_only_a_non_tls_transport() {
        // No communication settings, or TLS: the connection goes ahead.
        assert!(check_comm("Data Source=\\\\db1\\data;ServerType=REMOTE;").is_ok());
        assert!(check_comm("CommType=TLS;FIPS=FALSE;").is_ok());
        // TLS settings tflipd cannot apply are accepted (logged), not refused.
        assert!(
            check_comm("CommType=tls;TLSCiphers=AES256SHA;TLSCommonName=db1;FIPS=TRUE;").is_ok()
        );
        // A non-TLS transport is refused, unless told to ignore it.
        for comm in ["TCP_IP", "UDP_IP", "IPX", "tcp_ip"] {
            assert_eq!(
                check_comm_with(&format!("CommType={comm};"), false),
                Err(Error::InvalidOption),
                "{comm}"
            );
            assert_eq!(
                check_comm_with(&format!("CommType={comm};"), true),
                Ok(()),
                "{comm}"
            );
        }
    }

    #[test]
    fn names_within_a_folder() {
        assert_eq!(within(r"data\sales", r"data\sales\*.adt"), Some("*.adt"));
        assert_eq!(
            within(r"data\sales\", r"DATA/Sales/sub\x.adt"),
            Some(r"sub\x.adt")
        );
        assert_eq!(within(r"data\sales", r"data\sales"), Some(""));
        assert_eq!(within("data", r"data\x.adt"), Some("x.adt"));
        assert_eq!(within(r"data\sales", r"data\salesx\a.adt"), None);
        assert_eq!(within(r"data\sales", r"other\sales\a.adt"), None);
    }
}
