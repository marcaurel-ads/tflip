//! The client's side of a connection to `tflipd`.
//!
//! A [`Client`] is one TLS connection, and on the server one session. It
//! implements [`crate::Api`] by sending each call and waiting for its
//! answer, one at a time. Once the connection breaks, every call fails with
//! [`Error::ConnectionLost`].
//!
//! What does not change about open tables and index orders ([`TableInfo`],
//! [`IndexInfo`]) is kept after it was first asked for, until a table is
//! closed or a connection disconnected.
//!
//! [`TableInfo`]: tflip_engine::table::TableInfo
//! [`IndexInfo`]: tflip_engine::index::IndexInfo

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Mutex;
use std::time::Duration;

use rustls::pki_types::ServerName;
use rustls::{ClientConnection, StreamOwned};
use tflip_engine::Error;
use tflip_engine::getters::{FieldId, Scalar, Want, row_cacheable};
use tflip_engine::table::{FieldRef, find_field};

use crate::deflate::Deflated;
use crate::tls::{self, Trust};
use crate::wire::{self, Compression, Hello, Platform, Welcome};
use crate::{Api, Handle, Reply, Request};

/// A stream a client talks to its server over.
pub trait Stream: Read + Write + Send {}

impl<T: Read + Write + Send> Stream for T {}

/// A connection to a server.
pub struct Client {
    /// The stream; `None` once it broke.
    stream: Mutex<Option<Box<dyn Stream>>>,
    /// The server, as its [`Welcome`] names it.
    server: String,
    /// The server's platform, as its [`Welcome`] names it.
    platform: Platform,
    /// The TLS cipher suite the connection uses (`TLS13_AES_256_GCM_SHA384`),
    /// if it is a TLS connection.
    cipher_suite: Option<String>,
    /// The server's compression setting, as its [`Welcome`] names it.
    server_compression: Compression,
    /// Whether the connection is compressed.
    compressed: bool,
    /// The replies the client keeps (see [`Client::call`]), by their
    /// requests as sent.
    kept: Mutex<HashMap<Vec<u8>, Reply>>,
    /// The fields of the current record, cached a whole row at a time (see
    /// [`Client::field_as_cached`]): each `AdsGet*` of a field after a move
    /// fetches every field of the row in one call, and the rest are then
    /// read from here. Keyed by table handle, 1-based field number and the
    /// form it was read as; cleared whenever a request might move the cursor
    /// or change a record or a formatting setting (see [`keeps_row`]).
    rows: Mutex<Rows>,
}

/// A cache of records' fields by table handle, 1-based field number and the
/// form the field was read as (see [`Client::field_as_cached`]).
type Rows = HashMap<(Handle, usize, Want), Result<Scalar, Error>>;

/// Why connecting to a server failed: the engine error the client reports,
/// and what went wrong, for its log.
#[derive(Debug)]
pub struct ConnectError {
    pub error: Error,
    pub reason: String,
}

impl ConnectError {
    fn new(error: Error, reason: impl ToString) -> Self {
        ConnectError {
            error,
            reason: reason.to_string(),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// How a client connects to a server (see [`Client::connect`]).
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    /// The servers to trust.
    pub trust: Trust,
    /// How long making the TCP connection may take.
    pub timeout: Duration,
    /// How long a single read or write may block once the connection is up;
    /// `None` for no bound.
    pub request_timeout: Option<Duration>,
    /// The client's compression setting.
    pub compression: Compression,
    /// Whether the client connects as to an Internet server
    /// (`ADS_AIS_SERVER`), which [`Compression::Internet`] compresses.
    pub internet: bool,
}

impl ConnectOptions {
    /// Trusting the servers `trust` says, connecting within `timeout`, with no
    /// request timeout and the default compression.
    pub fn new(trust: Trust, timeout: Duration) -> Self {
        ConnectOptions {
            trust,
            timeout,
            request_timeout: None,
            compression: Compression::default(),
            internet: false,
        }
    }
}

impl Client {
    /// Starts a session on the server at the other end of `stream`, with
    /// handles from `handle_base` on, asking for compression as
    /// `compression` and `internet` say (see [`wire::compressed`]). `name`
    /// names the client in the server's log.
    pub fn start(
        mut stream: Box<dyn Stream>,
        handle_base: Handle,
        name: &str,
        compression: Compression,
        internet: bool,
    ) -> Result<Client, ConnectError> {
        let lost = |error: io::Error| ConnectError::new(Error::RemoteNotFound, error);
        let hello = Hello::new(handle_base, name, compression, internet);
        wire::write_message(&mut stream, &hello).map_err(lost)?;
        let welcome: Result<Welcome, Error> = wire::read_message(&mut stream).map_err(lost)?;
        let welcome = welcome.map_err(|error| ConnectError::new(error, "refused by the server"))?;
        if welcome.protocol != crate::PROTOCOL_VERSION {
            let reason = format!("the server speaks protocol {}", welcome.protocol);
            return Err(ConnectError::new(Error::ProtocolMismatch, reason));
        }
        if welcome.compressed {
            stream = Box::new(Deflated::new(stream));
        }
        Ok(Client {
            stream: Mutex::new(Some(stream)),
            server: welcome.server,
            platform: welcome.platform,
            cipher_suite: None,
            server_compression: welcome.compression,
            compressed: welcome.compressed,
            kept: Mutex::new(HashMap::new()),
            rows: Mutex::new(HashMap::new()),
        })
    }

    /// Connects to the server at `host`:`port` over TLS, trusting it as the
    /// `options`' [`Trust`] says, and starts a session (see
    /// [`Client::start`]).
    ///
    /// The `options`' `timeout` bounds making the TCP connection; their
    /// `request_timeout`, if given, bounds how long a single read or write on
    /// the connection may block once it is up, so that a stalled server does
    /// not block a call forever (the original engine's client blocks
    /// indefinitely). A read or write that times out breaks the connection,
    /// and the call fails with [`Error::ConnectionLost`].
    pub fn connect(
        host: &str,
        port: u16,
        handle_base: Handle,
        name: &str,
        options: &ConnectOptions,
    ) -> Result<Client, ConnectError> {
        let ConnectOptions {
            timeout,
            request_timeout,
            ..
        } = *options;
        let not_found = |reason: String| ConnectError::new(Error::RemoteNotFound, reason);
        let addresses = (host, port)
            .to_socket_addrs()
            .map_err(|error| not_found(format!("cannot resolve {host}: {error}")))?;
        let mut last = format!("{host} has no address");
        let mut socket = None;
        for address in addresses {
            match TcpStream::connect_timeout(&address, timeout) {
                Ok(connected) => {
                    socket = Some(connected);
                    break;
                }
                Err(error) => last = format!("cannot connect to {address}: {error}"),
            }
        }
        let socket = socket.ok_or_else(|| not_found(last))?;
        let _ = socket.set_nodelay(true);
        let name_of_server = ServerName::try_from(host.to_owned())
            .map_err(|error| not_found(format!("{host}: {error}")))?;
        let config = tls::client_config(&options.trust).map_err(not_found)?;
        let connection = ClientConnection::new(config, name_of_server)
            .map_err(|error| not_found(error.to_string()))?;
        let mut stream = StreamOwned::new(connection, socket);
        // Finish the handshake now, so that an untrusted server fails here.
        while stream.conn.is_handshaking() {
            stream
                .conn
                .complete_io(&mut stream.sock)
                .map_err(|error| not_found(format!("TLS with {host}:{port}: {error}")))?;
        }
        let suite = stream
            .conn
            .negotiated_cipher_suite()
            .map(|suite| format!("{:?}", suite.suite()));
        // Bound each later read and write, now that the handshake is done, so
        // a stalled server cannot block a call forever.
        if request_timeout.is_some() {
            let _ = stream.sock.set_read_timeout(request_timeout);
            let _ = stream.sock.set_write_timeout(request_timeout);
        }
        let (compression, internet) = (options.compression, options.internet);
        let mut client = Client::start(Box::new(stream), handle_base, name, compression, internet)?;
        client.cipher_suite = suite;
        Ok(client)
    }

    /// Starts a session with a server in the process, which `exchange`
    /// answers: it is given each request as one frame and returns its reply
    /// as one (see [`crate::local::answer`]). The session is the server's
    /// already, so the client greets no one; its handles are the server
    /// session's, from `handle_base` on. An external procedure's library in
    /// `tflipd` calls the server's session of its client this way.
    pub fn in_process(handle_base: Handle, exchange: Exchange) -> Result<Client, ConnectError> {
        let stream = InProcess {
            exchange,
            request: Vec::new(),
            reply: io::Cursor::new(Vec::new()),
            greeted: false,
        };
        Client::start(
            Box::new(stream),
            handle_base,
            "external procedure",
            Compression::Never,
            false,
        )
    }

    /// The server, as it names itself.
    pub fn server(&self) -> &str {
        &self.server
    }

    /// The server's platform.
    pub fn platform(&self) -> &Platform {
        &self.platform
    }

    /// The TLS cipher suite the connection uses, if it is a TLS connection.
    pub fn cipher_suite(&self) -> Option<&str> {
        self.cipher_suite.as_deref()
    }

    /// The server's compression setting.
    pub fn server_compression(&self) -> Compression {
        self.server_compression
    }

    /// Whether the connection is compressed.
    pub fn compressed(&self) -> bool {
        self.compressed
    }

    /// Whether the connection still works, as far as the client knows.
    pub fn is_connected(&self) -> bool {
        lock(&self.stream).is_some()
    }

    /// Sends `request` and waits for its reply, or answers it from what the
    /// client keeps: the successful replies of the requests whose answers do
    /// not change while their handles stay open (see [`kept`]). A request
    /// that closes objects, or opens an index file, makes the client forget
    /// them all rather than work out which.
    pub(crate) fn call(&self, request: Request) -> Result<Reply, Error> {
        // A field read is answered from the whole row cached on the first
        // read after a move, fetching the row if it is not cached yet.
        if let Request::field_as { table, field, want } = &request {
            return self.field_as_cached(*table, field, *want);
        }
        // Anything that might move the cursor, change a record or change a
        // formatting setting invalidates the cached rows.
        if !keeps_row(&request) {
            lock(&self.rows).clear();
        }
        let key = kept(&request)
            .then(|| postcard::to_stdvec(&request).ok())
            .flatten();
        if let Some(key) = &key
            && let Some(reply) = lock(&self.kept).get(key)
        {
            return Ok(reply.clone());
        }
        let reply = self.exchange(&request)?;
        if forgets(&request) {
            lock(&self.kept).clear();
        } else if let Some(key) = key
            && succeeded(&reply)
        {
            lock(&self.kept).insert(key, reply.clone());
        }
        Ok(reply)
    }

    /// Answers a `field_as` of `field` read as `want` from the cached row,
    /// fetching the whole current record ([`Request::read_fields`]) on a
    /// miss and caching every field of it. A memo field, or a field a
    /// [`crate::Api::table_info`] does not describe, is read on its own and
    /// not cached (so memo values stay fetched on demand).
    fn field_as_cached(&self, table: Handle, field: &FieldId, want: Want) -> Result<Reply, Error> {
        let Some(number) = self.cacheable_number(table, field)? else {
            return self.exchange(&Request::field_as {
                table,
                field: field.clone(),
                want,
            });
        };
        if let Some(result) = lock(&self.rows).get(&(table, number, want)) {
            return Ok(Reply::field_as(result.clone()));
        }
        let row = match self.call(Request::read_fields { table, want })? {
            Reply::read_fields(Ok(row)) => row,
            Reply::read_fields(Err(error)) => return Ok(Reply::field_as(Err(error))),
            _ => return Err(Error::ProtocolMismatch),
        };
        let mut rows = lock(&self.rows);
        for (i, entry) in row.iter().enumerate() {
            if let Some(result) = entry {
                rows.insert((table, i + 1, want), result.clone());
            }
        }
        match row.get(number - 1).and_then(|entry| entry.clone()) {
            Some(result) => Ok(Reply::field_as(result)),
            // The server did not cache this field after all; read it alone.
            None => {
                drop(rows);
                self.exchange(&Request::field_as {
                    table,
                    field: field.clone(),
                    want,
                })
            }
        }
    }

    /// The 1-based number of `field` of `table` if its value can be read from
    /// a cached row (see [`row_cacheable`]), resolved from the table info the
    /// client keeps; `None` for a memo field or an unknown field, which are
    /// read on their own instead.
    fn cacheable_number(&self, table: Handle, field: &FieldId) -> Result<Option<usize>, Error> {
        let info = self.table_info(table)?;
        let structure = &info.structure;
        let reference = match field {
            FieldId::Number(number) => FieldRef::Number(*number),
            FieldId::Name(name) => FieldRef::Name(name),
        };
        Ok(match find_field(&structure.fields, reference) {
            Ok((number, found)) if row_cacheable(found) => Some(number),
            _ => None,
        })
    }

    /// Sends `request` and reads its reply. A failure breaks the connection.
    fn exchange(&self, request: &Request) -> Result<Reply, Error> {
        let mut stream = lock(&self.stream);
        let open = stream.as_mut().ok_or(Error::ConnectionLost)?;
        let result = wire::write_message(open, request).and_then(|()| wire::read_message(open));
        result.map_err(|_| {
            *stream = None;
            Error::ConnectionLost
        })
    }
}

/// What answers a request frame with a reply frame (see [`Client::in_process`]).
pub type Exchange = Box<dyn FnMut(&[u8]) -> Vec<u8> + Send>;

/// The stream of [`Client::in_process`]: what is written is a request,
/// whose reply is read once `exchange` gave it. The [`Hello`] is answered
/// here, as the session needs no greeting.
struct InProcess {
    exchange: Exchange,
    request: Vec<u8>,
    reply: io::Cursor<Vec<u8>>,
    greeted: bool,
}

impl Write for InProcess {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.request.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for InProcess {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.reply.position() as usize == self.reply.get_ref().len() && !self.request.is_empty()
        {
            let reply = match self.greeted {
                true => (self.exchange)(&self.request),
                false => {
                    self.greeted = true;
                    let welcome: Result<Welcome, Error> = Ok(Welcome {
                        protocol: crate::PROTOCOL_VERSION,
                        server: "tflipd".to_owned(),
                        platform: Platform::this(),
                        compression: Compression::Never,
                        compressed: false,
                    });
                    let mut frame = Vec::new();
                    wire::write_message(&mut frame, &welcome)?;
                    frame
                }
            };
            self.request.clear();
            self.reply = io::Cursor::new(reply);
        }
        self.reply.read(buf)
    }
}

/// Whether the answer to `request` stays the same while the handles it
/// names stay open, so that the client may keep it.
fn kept(request: &Request) -> bool {
    matches!(
        request,
        Request::handle_kind { .. }
            | Request::connection_info { .. }
            | Request::table_info { .. }
            | Request::index_info { .. }
            | Request::indexes { .. }
            | Request::full_text_indexes { .. }
            | Request::numbered_indexes { .. }
            | Request::cursor_statement { .. }
            | Request::cursor_is_live { .. }
    )
}

/// Whether `request` cannot change which record a table is on, the contents
/// of a record, or a setting that changes how a field reads (the date
/// format, "exact", the epoch, shown deleted records, a collation), so that
/// the cached rows (see [`Client::field_as_cached`]) stay good across it.
/// Anything not listed here clears them, so a move, a write or a setting
/// change is never missed; over-clearing only costs a re-read.
fn keeps_row(request: &Request) -> bool {
    matches!(
        request,
        // Reading the current record.
        Request::read_fields { .. }
            | Request::null { .. }
            | Request::read_record { .. }
            | Request::value { .. }
            | Request::is_null { .. }
            | Request::is_empty { .. }
            | Request::memo_length { .. }
            | Request::read_memo { .. }
            | Request::data_length { .. }
            | Request::record_crc { .. }
            // Where the table and its orders are.
            | Request::record_number { .. }
            | Request::found { .. }
            | Request::is_visible { .. }
            | Request::is_deleted { .. }
            | Request::position { .. }
            | Request::key_number { .. }
            | Request::key_count { .. }
            | Request::relative_position { .. }
            | Request::extract_key { .. }
            | Request::bookmark_key { .. }
            | Request::scope_key { .. }
            // Locks and the last AutoInc.
            | Request::is_record_locked { .. }
            | Request::is_table_locked { .. }
            | Request::locks { .. }
            | Request::last_autoinc { .. }
            // Reading filters and evaluating on the current record.
            | Request::aof { .. }
            | Request::aof_level { .. }
            | Request::eval_aof { .. }
            | Request::is_in_aof { .. }
            | Request::filter { .. }
            | Request::check_expression { .. }
            | Request::eval { .. }
            | Request::key_filter { .. }
            // What does not change about open objects.
            | Request::handle_kind { .. }
            | Request::connection_info { .. }
            | Request::table_info { .. }
            | Request::index_info { .. }
            | Request::indexes { .. }
            | Request::full_text_indexes { .. }
            | Request::numbered_indexes { .. }
            | Request::index_by_expression { .. }
            | Request::record_count { .. }
            | Request::last_update { .. }
            | Request::handle_long { .. }
            // Session and setting reads.
            | Request::activity { .. }
            | Request::users { .. }
            | Request::open_tables { .. }
            | Request::open_indexes { .. }
            | Request::date_format { .. }
            | Request::exact { .. }
            | Request::epoch { .. }
            | Request::collation { .. }
            | Request::transaction_count { .. }
            | Request::in_transaction { .. }
            // Statement and cursor metadata.
            | Request::statement_text { .. }
            | Request::statement_params { .. }
            | Request::affected { .. }
            | Request::cursor_statement { .. }
            | Request::open_cursor { .. }
            | Request::cursor_is_live { .. }
            | Request::dd_property { .. }
    )
}

/// Whether `request` closes objects (`delete_table` closes its table),
/// changes a table's orders, or changes what a kept answer describes, so
/// the client forgets its kept answers.
/// `set_transaction_free` changes a flag the table info reports, which is
/// otherwise taken not to change.
fn forgets(request: &Request) -> bool {
    matches!(
        request,
        Request::disconnect { .. }
            | Request::close_table { .. }
            | Request::delete_table { .. }
            | Request::open_index { .. }
            | Request::close_index { .. }
            | Request::close_all_indexes { .. }
            | Request::close_statement { .. }
            | Request::set_transaction_free { .. }
    )
}

/// Whether `reply` is a success worth keeping: a known handle's kind, or
/// another answer that is not an error.
fn succeeded(reply: &Reply) -> bool {
    match reply {
        Reply::handle_kind(result) => matches!(result, Ok(Some(_))),
        Reply::connection_info(result) => result.is_ok(),
        Reply::table_info(result) => result.is_ok(),
        Reply::index_info(result) => result.is_ok(),
        Reply::indexes(result) | Reply::full_text_indexes(result) => result.is_ok(),
        Reply::numbered_indexes(result) => result.is_ok(),
        Reply::cursor_statement(result) => result.is_ok(),
        Reply::cursor_is_live(result) => result.is_ok(),
        _ => false,
    }
}
