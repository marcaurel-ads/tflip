//! `tflipd`, the TableflipDB server.
//!
//! ```text
//! tflipd --share NAME=DIR [--share NAME=DIR]... [--listen ADDRESS]
//!        [--cert FILE --key FILE] [--state DIR] [--collations DIR]
//!        [--transactions real|count] [--compression internet|always|never]
//!        [--check-free-table-access] [--disable-free-connections]
//! ```
//!
//! Serves the folders it is given as shares to clients that load tflip's
//! `ace32.dll` or `ace64.dll`: a client names its data
//! `\\server\NAME\folder` (or `\\server:port\NAME\folder`). Every connection
//! uses TLS, with the certificate from `--cert` and `--key`, or else one
//! `tflipd` makes itself and keeps in its state folder; clients trust it by
//! the fingerprint it prints when it starts. A certificate from a CA (its
//! file may hold the intermediate certificates after it, which are presented
//! with it) is trusted by clients whose system trusts the CA, without a
//! fingerprint.
//!
//! `--collations` names the folder of the original engine's collation files
//! (`ansi.chr`, `adscollate.adt`, and `adslocal.cfg` for `ANSI_CHAR_SET`),
//! which index orders with a collation need. With `ANSI_CHAR_SET` empty, or
//! without the folder, the static ANSI collation is the one the host's
//! locale derives, as in the original engine: on Linux `LC_ALL`, else
//! `LC_COLLATE`, else `LANG` (`de_DE.UTF-8` weighs `ä` after `a`; US, UK and
//! Canadian English, `C` and none keep the bytes themselves; see
//! `tflip_engine::system_collation`).
//!
//! `--transactions count` keeps transactions as the original's local server
//! does, only counting their levels, for clients that rely on that; they are
//! real ones by default (see `tflip_engine::transaction`). Their logs are
//! kept in `log` in the state folder; transactions a crash interrupted are
//! recovered from them when `tflipd` starts and when tables are opened (see
//! `tflip_engine::txlog`).
//!
//! `--compression` is the server's side of compressing a connection, as the
//! original server's `COMPRESSION` setting: `internet` (the default)
//! compresses a client's connection if the client says `ALWAYS`, or says
//! `INTERNET` (its default) and connects as to an Internet server
//! (`ADS_AIS_SERVER`); `always` compresses every client's that does not say
//! `NEVER`; `never` none (see `tflip_protocol::wire::compressed`).
//!
//! `--check-free-table-access` and `--disable-free-connections` are the
//! original server's settings `Check_Free_Table_Access` (no connection to a
//! dictionary opens a free table, ADSSYS's too) and `Disable_Free_Connections`
//! (every connection must be to a dictionary, ADSSYS's too).
//!
//! With a dictionary's `ENFORCE_MAX_FAILED_LOGINS` on, a user whose logins
//! fail `MAX_FAILED_ATTEMPTS` times is locked out (7113) until an
//! administrator enables the logins again; ADSSYS is locked out until
//! `tflipd` restarts, and the counts are forgotten then.
//!
//! A dictionary's external procedures (`.aep`, a shared object on Linux and a
//! DLL on Windows) run in `tflipd`'s process when they link tflip's client
//! library, `libtflip_ace.so` or `ace64.dll`, through which they call the
//! session of the client that runs them (see `tflip_aep`).
//!
//! There are no users or passwords yet: anyone who can reach the port can
//! read the shares, so listen only where that is acceptable.
//!
//! `TFLIPD_DEBUG=requests` logs every request.

#![forbid(unsafe_code)]

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use std::{env, fs, thread};

use anyhow::{Context, anyhow, bail};
use tflip_engine::Session;
use tflip_engine::collations::Collations;
use tflip_engine::shares::Shares;
use tflip_protocol::rustls::ServerConfig;
use tflip_protocol::server::{self, Greeting};
use tflip_protocol::tls::fingerprint_text;
use tflip_protocol::tls::server::Identity;
use tflip_protocol::wire::Compression;

const USAGE: &str = "usage: tflipd --share NAME=DIR [--share NAME=DIR]... [--listen ADDRESS] \
                     [--cert FILE --key FILE] [--state DIR] [--collations DIR] \
                     [--transactions real|count] [--compression internet|always|never] \
                     [--check-free-table-access] [--disable-free-connections]";

/// The server's name, as clients see it.
const SERVER: &str = concat!("tflipd ", env!("CARGO_PKG_VERSION"));

/// What `tflipd` prints first when it starts, as the README's banner.
const BANNER: &str = "(╯°□°)╯︵ ┻━┻\n\nTableflipDB\n";

/// How long a client may take to finish the TLS handshake and say hello.
const GREETING_TIMEOUT: Duration = Duration::from_secs(10);

/// What the command line asks for.
struct Options {
    listen: String,
    shares: Shares,
    share_list: Vec<(String, PathBuf)>,
    cert: Option<PathBuf>,
    key: Option<PathBuf>,
    state: Option<PathBuf>,
    collations: Option<PathBuf>,
    /// Whether transactions are real ones (`--transactions`).
    real_transactions: bool,
    /// The server's compression setting (`--compression`).
    compression: Compression,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tflipd: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    let options = parse(env::args().skip(1))?;
    println!("{BANNER}");
    let identity = identity(&options)?;
    let fingerprint = identity.fingerprint().map_err(anyhow::Error::msg)?;
    let config = identity.server_config().map_err(anyhow::Error::msg)?;
    let listener = TcpListener::bind(&options.listen)
        .with_context(|| format!("cannot listen on {}", options.listen))?;
    let address = listener
        .local_addr()
        .context("cannot get the listening address")?;
    println!("{SERVER} listening on {address}");
    println!(
        "certificate fingerprint: {}",
        fingerprint_text(&fingerprint)
    );
    for (name, folder) in &options.share_list {
        println!("share {name}: {}", folder.display());
    }
    // The transactions' logs, from which those a crash interrupted are
    // recovered now and when tables are opened.
    if let Some(state) = options.state.clone().or_else(state_folder) {
        tflip_engine::txlog::set_folder(state.join("log"));
    }
    tflip_engine::aep::set_libraries(Box::new(tflip_aep::Loader::hosting()));
    let recovering = Session::new();
    tflip_engine::settings::set_collations(&recovering, collations(options.collations.as_deref()));
    match tflip_engine::txlog::recover(&recovering) {
        Ok(0) => {}
        Ok(n) => println!("recovered {n} transaction(s) a crash interrupted"),
        Err(_) => eprintln!("tflipd: a transaction a crash interrupted could not be recovered"),
    }
    drop(recovering);
    let shares = Arc::new(options.shares);
    let log_requests = env::var("TFLIPD_DEBUG").is_ok_and(|spec| {
        spec.split(',')
            .any(|channel| channel.trim().trim_start_matches('+') == "requests")
    });
    for socket in listener.incoming() {
        let socket = match socket {
            Ok(socket) => socket,
            Err(error) => {
                eprintln!("tflipd: accepting a connection: {error}");
                continue;
            }
        };
        let (config, shares) = (Arc::clone(&config), Arc::clone(&shares));
        let collations = options.collations.clone();
        let real = options.real_transactions;
        let compression = options.compression;
        thread::spawn(move || {
            serve(
                socket,
                config,
                shares,
                collations,
                real,
                compression,
                log_requests,
            )
        });
    }
    Ok(())
}

/// The collations of the original engine's files in `dir` (with
/// `ANSI_CHAR_SET` empty, or without the files, the static ANSI collation is
/// the one the host's locale derives, as in the original engine).
fn collations(dir: Option<&std::path::Path>) -> Collations {
    let system = tflip_engine::system_collation::host();
    match dir {
        Some(dir) => Collations::from_folder_for(dir, system),
        None => Collations::system(system),
    }
}

/// Serves one client until it disconnects.
fn serve(
    socket: TcpStream,
    config: Arc<ServerConfig>,
    shares: Arc<Shares>,
    collations: Option<PathBuf>,
    real_transactions: bool,
    compression: Compression,
    log_requests: bool,
) {
    let peer = socket
        .peer_addr()
        .map_or_else(|_| "?".to_owned(), |peer| peer.to_string());
    let _ = socket.set_nodelay(true);
    let _ = socket.set_read_timeout(Some(GREETING_TIMEOUT));
    let mut stream = match server::accept_tls(config, socket) {
        Ok(stream) => stream,
        Err(error) => return eprintln!("tflipd: {peer}: TLS: {error}"),
    };
    let greeting = server::greet(&mut stream, SERVER, compression);
    let Greeting {
        handle_base,
        client,
        compressed,
    } = match greeting {
        Ok(greeting) => greeting,
        Err(error) => return eprintln!("tflipd: {peer}: {error}"),
    };
    let _ = stream.sock.set_read_timeout(None);
    let how = if compressed { ", compressed" } else { "" };
    eprintln!("tflipd: {peer}: {client} connected{how}");
    let session = Session::serving(handle_base, shares);
    if let Ok(address) = stream.sock.peer_addr() {
        session.set_client_address(address.ip().to_string());
    }
    tflip_engine::settings::set_real_transactions(&session, real_transactions);
    tflip_engine::settings::set_collations(&session, self::collations(collations.as_deref()));
    let result = server::serve(&mut stream, compressed, &session, |request| {
        if log_requests {
            eprintln!("tflipd: {peer}: {request:?}");
        }
    });
    match result {
        Ok(()) => eprintln!("tflipd: {peer}: disconnected"),
        Err(error) => eprintln!("tflipd: {peer}: connection lost: {error}"),
    }
}

/// Reads the command line.
fn parse(mut args: impl Iterator<Item = String>) -> anyhow::Result<Options> {
    let default = SocketAddr::from(([0, 0, 0, 0], tflip_protocol::DEFAULT_PORT));
    let mut options = Options {
        listen: default.to_string(),
        shares: Shares::new(),
        share_list: Vec::new(),
        cert: None,
        key: None,
        state: None,
        collations: None,
        real_transactions: true,
        compression: Compression::Internet,
    };
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| anyhow!("{arg} needs a value\n{USAGE}"))
        };
        match arg.as_str() {
            "--listen" => options.listen = value()?,
            "--share" => {
                let share = value()?;
                let (name, dir) = share
                    .split_once('=')
                    .with_context(|| format!("{share}: not NAME=DIR"))?;
                let folder = fs::canonicalize(dir).with_context(|| dir.to_owned())?;
                if !folder.is_dir() {
                    bail!("{dir}: not a folder");
                }
                options.shares.add(name, &folder);
                options.share_list.push((name.to_owned(), folder));
            }
            "--cert" => options.cert = Some(value()?.into()),
            "--key" => options.key = Some(value()?.into()),
            "--state" => options.state = Some(value()?.into()),
            "--collations" => options.collations = Some(value()?.into()),
            "--transactions" => {
                options.real_transactions = match value()?.to_ascii_lowercase().as_str() {
                    "real" => true,
                    "count" => false,
                    other => bail!("--transactions {other}: not real or count"),
                };
            }
            "--compression" => {
                let setting = value()?;
                options.compression = Compression::parse(&setting).with_context(|| {
                    format!("--compression {setting}: not internet, always or never")
                })?;
            }
            "--check-free-table-access" => options.shares.set_check_free_table_access(true),
            "--disable-free-connections" => options.shares.set_disable_free_connections(true),
            "--help" | "-h" => bail!(USAGE),
            _ => bail!("unknown argument {arg}\n{USAGE}"),
        }
    }
    if options.share_list.is_empty() {
        bail!("no shares\n{USAGE}");
    }
    Ok(options)
}

/// The server's certificate: from `--cert` and `--key`, or the one in the
/// state folder, made there on the first start.
fn identity(options: &Options) -> anyhow::Result<Identity> {
    let read =
        |path: &PathBuf| fs::read_to_string(path).with_context(|| path.display().to_string());
    match (&options.cert, &options.key) {
        (Some(cert), Some(key)) => {
            return Ok(Identity {
                cert_pem: read(cert)?,
                key_pem: read(key)?,
            });
        }
        (None, None) => {}
        _ => bail!("--cert and --key go together"),
    }
    let state = match &options.state {
        Some(state) => state.clone(),
        None => state_folder().context("no state folder; give --state or --cert and --key")?,
    };
    let (cert, key) = (state.join("tflipd.crt"), state.join("tflipd.key"));
    if cert.exists() && key.exists() {
        return Ok(Identity {
            cert_pem: read(&cert)?,
            key_pem: read(&key)?,
        });
    }
    let identity = Identity::generate(&["tflipd", "localhost"]).map_err(anyhow::Error::msg)?;
    fs::create_dir_all(&state).with_context(|| state.display().to_string())?;
    write_private(&key, &identity.key_pem)?;
    fs::write(&cert, &identity.cert_pem).with_context(|| cert.display().to_string())?;
    println!("made a new certificate in {}", state.display());
    Ok(identity)
}

/// Where `tflipd` keeps its certificate unless told otherwise:
/// `$XDG_STATE_HOME/tflipd` (`~/.local/state/tflipd`), or
/// `%LOCALAPPDATA%\tflipd` on Windows.
fn state_folder() -> Option<PathBuf> {
    if cfg!(windows) {
        return env::var_os("LOCALAPPDATA").map(|folder| PathBuf::from(folder).join("tflipd"));
    }
    let state = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))?;
    Some(state.join("tflipd"))
}

/// Writes a file only its owner may read, such as the private key.
fn write_private(path: &PathBuf, text: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(path)
        .with_context(|| path.display().to_string())?;
    file.write_all(text.as_bytes())
        .with_context(|| path.display().to_string())
}
