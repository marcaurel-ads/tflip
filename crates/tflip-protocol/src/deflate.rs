//! Compression of a connection's messages, when client and server agree on it
//! (see [`crate::wire::Compression`]).
//!
//! Each direction is one raw deflate stream that lives as long as the
//! connection, so that a message is compressed against those before it. A
//! flush — which [`crate::wire::write_message`] does after every message —
//! ends the bytes written so far with a sync flush, so the other side can
//! decompress the whole message as soon as it arrives. The frames are written
//! into the stream as they are without compression; the length a frame starts
//! with is the length of its message uncompressed, and is checked against
//! [`crate::wire::MAX_MESSAGE`] as usual.

use std::io::{self, Read, Write};

use flate2::{Compress, Decompress, FlushCompress, FlushDecompress, Status};

/// The compression level: the fastest, as a message is compressed while its
/// caller waits.
const LEVEL: u32 = 1;

/// The most compressed bytes read from the stream at a time.
const READ_CHUNK: usize = 16 << 10;

/// A stream whose bytes are compressed on the way out and decompressed on the
/// way in.
pub struct Deflated<S> {
    inner: S,
    compress: Compress,
    decompress: Decompress,
    /// What was written since the last flush, uncompressed.
    pending: Vec<u8>,
    /// Compressed bytes read and not yet decompressed: `input[start..end]`.
    input: Box<[u8]>,
    start: usize,
    end: usize,
}

impl<S> Deflated<S> {
    /// Compresses what is written to `inner` and decompresses what is read
    /// from it.
    pub fn new(inner: S) -> Self {
        Deflated {
            inner,
            compress: Compress::new(flate2::Compression::new(LEVEL), false),
            decompress: Decompress::new(false),
            pending: Vec::new(),
            input: vec![0; READ_CHUNK].into_boxed_slice(),
            start: 0,
            end: 0,
        }
    }

    /// The stream underneath.
    pub fn get_ref(&self) -> &S {
        &self.inner
    }
}

fn invalid(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

impl<S: Write> Write for Deflated<S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        Ok(buf.len())
    }

    /// Compresses what was written, ends it with a sync flush, and writes and
    /// flushes it to the stream underneath.
    fn flush(&mut self) -> io::Result<()> {
        let mut out = Vec::with_capacity(self.pending.len() / 2 + 64);
        let mut done = 0;
        loop {
            let before = self.compress.total_in();
            self.compress
                .compress_vec(&self.pending[done..], &mut out, FlushCompress::Sync)
                .map_err(invalid)?;
            done += (self.compress.total_in() - before) as usize;
            // The flush is complete once all input is taken and the output
            // was not filled up.
            if done == self.pending.len() && out.len() < out.capacity() {
                break;
            }
            out.reserve(out.capacity().max(64));
        }
        self.pending.clear();
        self.inner.write_all(&out)?;
        self.inner.flush()
    }
}

impl<S: Read> Read for Deflated<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            let (before_in, before_out) = (self.decompress.total_in(), self.decompress.total_out());
            let status = self
                .decompress
                .decompress(
                    &self.input[self.start..self.end],
                    buf,
                    FlushDecompress::None,
                )
                .map_err(invalid)?;
            let consumed = (self.decompress.total_in() - before_in) as usize;
            self.start += consumed;
            let produced = (self.decompress.total_out() - before_out) as usize;
            if produced > 0 {
                return Ok(produced);
            }
            if status == Status::StreamEnd {
                return Ok(0);
            }
            if self.start < self.end {
                if consumed > 0 {
                    continue;
                }
                // Input left that gives nothing: the stream is broken.
                return Err(invalid("compressed stream makes no progress"));
            }
            let n = self.inner.read(&mut self.input)?;
            if n == 0 {
                return Ok(0);
            }
            (self.start, self.end) = (0, n);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::wire;

    #[test]
    fn messages_round_trip_compressed() {
        let mut sent = Deflated::new(Vec::new());
        let long = "compressible text ".repeat(2000);
        let messages = [String::new(), "short".to_owned(), long.clone(), long];
        for message in &messages {
            wire::write_message(&mut sent, message).unwrap();
        }
        let bytes = sent.get_ref().clone();
        assert!(
            bytes.len() < 2 * 18 * 2000 / 10,
            "compressed: {} bytes",
            bytes.len()
        );
        let mut received = Deflated::new(Cursor::new(bytes));
        for message in &messages {
            assert_eq!(
                &wire::read_message::<String>(&mut received).unwrap(),
                message
            );
        }
        let end = wire::read_message::<String>(&mut received).unwrap_err();
        assert_eq!(end.kind(), io::ErrorKind::UnexpectedEof);
    }

    /// Each flushed message can be read before the next one is written.
    #[test]
    fn each_message_is_readable_at_once() {
        let mut sent = Deflated::new(Vec::new());
        let mut offset = 0;
        let mut received_bytes = Vec::new();
        for i in 0..20u32 {
            let message = format!("message {i} {}", "x".repeat(i as usize * 100));
            wire::write_message(&mut sent, &message).unwrap();
            received_bytes.extend_from_slice(&sent.get_ref()[offset..]);
            offset = sent.get_ref().len();
            let mut received = Deflated::new(Cursor::new(received_bytes.clone()));
            let mut last = String::new();
            for _ in 0..=i {
                last = wire::read_message(&mut received).unwrap();
            }
            assert_eq!(last, message);
        }
    }

    #[test]
    fn garbage_is_refused() {
        let mut received = Deflated::new(Cursor::new(vec![0xff; 64]));
        let error = wire::read_message::<String>(&mut received).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
