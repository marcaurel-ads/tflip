//! The server's side of a connection: accepting TLS, greeting the client,
//! and answering its requests with a session of its own.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

use rustls::{ServerConfig, ServerConnection, StreamOwned};
use tflip_engine::Error;

use crate::wire::{self, Compression, Hello, MAGIC, Welcome};
use crate::{Api, Handle, Request, dispatch};

/// A TLS stream to a client.
pub type TlsStream = StreamOwned<ServerConnection, TcpStream>;

/// Completes the TLS handshake with the client at the other end of
/// `socket`.
pub fn accept_tls(config: Arc<ServerConfig>, socket: TcpStream) -> io::Result<TlsStream> {
    let connection = ServerConnection::new(config).map_err(io::Error::other)?;
    let mut stream = StreamOwned::new(connection, socket);
    while stream.conn.is_handshaking() {
        stream.conn.complete_io(&mut stream.sock)?;
    }
    Ok(stream)
}

/// What a client said about itself in its [`Hello`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Greeting {
    /// The first handle its session is to hand out.
    pub handle_base: Handle,
    /// The client, as it names itself.
    pub client: String,
    /// Whether the rest of the connection is compressed: if so, the server
    /// talks to the client through [`crate::deflate::Deflated`].
    pub compressed: bool,
}

/// Reads the client's [`Hello`] and welcomes it as `server`, whose
/// compression setting is `compression` (see [`wire::compressed`]). A client
/// that is not a tflip client, or speaks another protocol version, is refused
/// with [`Error::ProtocolMismatch`] and [`io::ErrorKind::InvalidData`].
pub fn greet(
    stream: &mut (impl Read + Write),
    server: &str,
    compression: Compression,
) -> io::Result<Greeting> {
    let hello: Hello = wire::read_message(stream)?;
    let handle_base = Handle::try_from(hello.handle_base).ok();
    let handle_base = match handle_base {
        Some(base) if hello.magic == MAGIC && hello.protocol == crate::PROTOCOL_VERSION => base,
        _ => {
            let refusal: Result<Welcome, Error> = Err(Error::ProtocolMismatch);
            wire::write_message(stream, &refusal)?;
            let reason = format!("{:?} speaks protocol {}", hello.client, hello.protocol);
            return Err(io::Error::new(io::ErrorKind::InvalidData, reason));
        }
    };
    let compressed = wire::compressed(hello.compression, hello.internet, compression);
    let welcome: Result<Welcome, Error> = Ok(Welcome {
        protocol: crate::PROTOCOL_VERSION,
        server: server.to_owned(),
        platform: wire::Platform::this(),
        compression,
        compressed,
    });
    wire::write_message(stream, &welcome)?;
    Ok(Greeting {
        handle_base,
        client: hello.client,
        compressed,
    })
}

/// Answers the client's requests with `api` until it closes the connection,
/// compressed if `compressed` (as [`greet`] agreed). `log` is called with each
/// request before it is answered.
pub fn serve(
    stream: &mut (impl Read + Write),
    compressed: bool,
    api: &(impl Api + ?Sized),
    log: impl FnMut(&Request),
) -> io::Result<()> {
    if compressed {
        run(&mut crate::deflate::Deflated::new(stream), api, log)
    } else {
        run(stream, api, log)
    }
}

/// Answers the client's requests with `api` until it closes the connection.
/// `log` is called with each request before it is answered.
pub fn run(
    stream: &mut (impl Read + Write),
    api: &(impl Api + ?Sized),
    mut log: impl FnMut(&Request),
) -> io::Result<()> {
    loop {
        let request: Request = match wire::read_message(stream) {
            Ok(request) => request,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        };
        log(&request);
        wire::write_message(stream, &dispatch(api, request))?;
    }
}
