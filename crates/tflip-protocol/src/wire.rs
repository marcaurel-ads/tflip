//! The wire format: frames of postcard-encoded messages over a TLS stream.
//!
//! A frame is the length of its message as 4 little-endian bytes, then the
//! message. A connection starts with the client's [`Hello`], which the
//! server answers with `Result<Welcome, Error>`; after that the client sends
//! one [`crate::Request`] at a time and waits for its [`crate::Reply`].
//!
//! The hello carries the client's [`Compression`] setting and the welcome
//! whether the connection is compressed (see [`compressed`]); if it is, every
//! frame after the welcome goes through [`crate::deflate::Deflated`].

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::Handle;

/// The longest message either side accepts, in bytes.
pub const MAX_MESSAGE: usize = 16 << 20;

/// What a [`Hello`] starts with, so that a server can tell a tflip client
/// from anything else that connects.
pub const MAGIC: [u8; 8] = *b"tflip\r\n\x1a";

/// The client's first message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// [`MAGIC`].
    pub magic: [u8; 8],
    /// The client's [`crate::PROTOCOL_VERSION`].
    pub protocol: u16,
    /// The first handle the server's session for this connection hands out,
    /// so that the handles of the client's sessions do not collide.
    pub handle_base: u64,
    /// The client, for the server's log (such as `tflip-ace 0.2.0`).
    pub client: String,
    /// The client's compression setting.
    pub compression: Compression,
    /// Whether the client connects as to an Internet server
    /// (`ADS_AIS_SERVER`), which [`Compression::Internet`] compresses.
    pub internet: bool,
}

impl Hello {
    /// A hello from `client` asking for handles from `handle_base` on, with
    /// the compression setting `compression` for a connection that is to an
    /// Internet server if `internet`.
    pub fn new(
        handle_base: Handle,
        client: &str,
        compression: Compression,
        internet: bool,
    ) -> Self {
        Hello {
            magic: MAGIC,
            protocol: crate::PROTOCOL_VERSION,
            handle_base: handle_base as u64,
            client: client.to_owned(),
            compression,
            internet,
        }
    }
}

/// A compression setting, of a client or a server, as the original engine
/// has them (`ADS_COMPRESS_*`, the `Compression` connect-string option, the
/// server's `COMPRESSION`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Compression {
    /// Compress connections to an Internet server (the default).
    #[default]
    Internet,
    /// Compress every connection, unless the other side says never.
    Always,
    /// Compress no connection.
    Never,
}

impl Compression {
    /// The setting a connect string's `Compression` value names
    /// (`INTERNET`, `ALWAYS` or `NEVER`, in any case); `None` for anything
    /// else.
    pub fn parse(text: &str) -> Option<Compression> {
        [
            Compression::Internet,
            Compression::Always,
            Compression::Never,
        ]
        .into_iter()
        .find(|setting| text.trim().eq_ignore_ascii_case(setting.name()))
    }

    /// The setting's name, as a connect string gives it.
    pub fn name(self) -> &'static str {
        match self {
            Compression::Internet => "INTERNET",
            Compression::Always => "ALWAYS",
            Compression::Never => "NEVER",
        }
    }

    /// The setting `AdsConnect60` options ask for (`ADS_COMPRESS_ALWAYS` 4,
    /// `ADS_COMPRESS_NEVER` 8, `ADS_COMPRESS_INTERNET` 12); `None` if they
    /// ask for none.
    pub fn from_options(options: u32) -> Option<Compression> {
        match options & 0xC {
            0x4 => Some(Compression::Always),
            0x8 => Some(Compression::Never),
            0xC => Some(Compression::Internet),
            _ => None,
        }
    }

    /// The setting as an `ADS_COMPRESS_*` number.
    pub fn ads(self) -> u16 {
        match self {
            Compression::Always => 0x4,
            Compression::Never => 0x8,
            Compression::Internet => 0xC,
        }
    }
}

/// Whether a connection is compressed, as the original engine decides from
/// the client's setting, the server's, and whether the client connects as to
/// an Internet server: never if either side says never; else always if
/// either says always; else (both say Internet) only for an Internet server.
pub fn compressed(client: Compression, internet: bool, server: Compression) -> bool {
    use Compression::*;
    match (client, server) {
        (Never, _) | (_, Never) => false,
        (Always, _) | (_, Always) => true,
        (Internet, Internet) => internet,
    }
}

/// The server's answer to a [`Hello`] it accepts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    /// The server's [`crate::PROTOCOL_VERSION`].
    pub protocol: u16,
    /// The server, for the client's log (such as `tflipd 0.2.0`).
    pub server: String,
    /// The server's platform, for `AdsMgGetServerType`.
    pub platform: Platform,
    /// The server's compression setting, for `AdsMgGetConfigInfo`.
    pub compression: Compression,
    /// Whether the rest of the connection is compressed (see [`compressed`]).
    pub compressed: bool,
}

/// The platform a program runs on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Platform {
    /// The operating system, as Rust names it (`linux`, `windows`, ...).
    pub os: String,
    /// The width of a pointer, in bits (32 or 64).
    pub bits: u32,
}

impl Platform {
    /// The platform this program runs on.
    pub fn this() -> Self {
        Platform {
            os: std::env::consts::OS.to_owned(),
            bits: usize::BITS,
        }
    }
}

/// Writes `message` as one frame and flushes it.
pub fn write_message(stream: &mut impl Write, message: &impl Serialize) -> io::Result<()> {
    let bytes = postcard::to_stdvec(message).map_err(invalid)?;
    if bytes.len() > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message too long",
        ));
    }
    let mut frame = Vec::with_capacity(4 + bytes.len());
    frame.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    frame.extend_from_slice(&bytes);
    stream.write_all(&frame)?;
    stream.flush()
}

/// Reads one frame and decodes its message. The end of the stream before a
/// frame starts is [`io::ErrorKind::UnexpectedEof`].
pub fn read_message<T: DeserializeOwned>(stream: &mut impl Read) -> io::Result<T> {
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message too long",
        ));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    postcard::from_bytes(&bytes).map_err(invalid)
}

fn invalid(error: postcard::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use tflip_engine::Error;

    use super::*;
    use crate::{Key, Reply, Request};

    #[test]
    fn messages_round_trip() {
        let mut buf = Vec::new();
        let request = Request::seek {
            order: 0x4000_0001,
            key: Key::Text("abc".into()),
            how: tflip_engine::seek::SeekType::Hard,
        };
        write_message(&mut buf, &request).unwrap();
        write_message(&mut buf, &Reply::seek(Err(Error::InvalidIndex))).unwrap();
        let length = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
        assert!(buf.len() > 4 + length, "two frames");
        let mut stream = Cursor::new(buf);
        assert_eq!(read_message::<Request>(&mut stream).unwrap(), request);
        let reply = read_message::<Reply>(&mut stream).unwrap();
        assert_eq!(reply, Reply::seek(Err(Error::InvalidIndex)));
        let end = read_message::<Reply>(&mut stream).unwrap_err();
        assert_eq!(end.kind(), io::ErrorKind::UnexpectedEof);
    }

    /// The original's table of client and server settings (the help's
    /// "Communications Compression").
    #[test]
    fn compression_is_agreed_as_the_original_does() {
        use Compression::*;
        for internet in [false, true] {
            assert_eq!(compressed(Internet, internet, Internet), internet);
            assert!(compressed(Internet, internet, Always));
            assert!(compressed(Always, internet, Internet));
            assert!(compressed(Always, internet, Always));
            for other in [Internet, Always, Never] {
                assert!(!compressed(Never, internet, other));
                assert!(!compressed(other, internet, Never));
            }
        }
        assert_eq!(Compression::parse(" always "), Some(Always));
        assert_eq!(Compression::parse("Never"), Some(Never));
        assert_eq!(Compression::parse("INTERNET"), Some(Internet));
        for bad in ["", "1", "TRUE", "ON", "OFF", "NONE", "BOGUS"] {
            assert_eq!(Compression::parse(bad), None, "{bad}");
        }
        assert_eq!(Compression::from_options(0x1004), Some(Always));
        assert_eq!(Compression::from_options(0x8), Some(Never));
        assert_eq!(Compression::from_options(0xC), Some(Internet));
        assert_eq!(Compression::from_options(0x3), None);
    }

    #[test]
    fn oversized_and_garbled_frames_are_refused() {
        let mut huge = Cursor::new(((MAX_MESSAGE + 1) as u32).to_le_bytes().to_vec());
        let error = read_message::<Request>(&mut huge).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let mut garbled = Cursor::new(vec![2, 0, 0, 0, 0xff, 0xff]);
        let error = read_message::<Request>(&mut garbled).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
