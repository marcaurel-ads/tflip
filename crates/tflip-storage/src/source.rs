//! Where a table's or memo file's bytes come from: a file, or memory for
//! tables built on the fly (SQL result sets, see [`crate::build`]). Both
//! take writes; a file only if it was opened for writing.

use std::fs::File;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};

/// The bytes of an open table or memo file.
#[derive(Debug)]
pub enum Source {
    File(File),
    Memory(Cursor<Vec<u8>>),
}

impl From<File> for Source {
    fn from(file: File) -> Self {
        Source::File(file)
    }
}

impl From<Vec<u8>> for Source {
    fn from(bytes: Vec<u8>) -> Self {
        Source::Memory(Cursor::new(bytes))
    }
}

impl Read for Source {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Source::File(file) => file.read(buf),
            Source::Memory(bytes) => bytes.read(buf),
        }
    }
}

impl Seek for Source {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match self {
            Source::File(file) => file.seek(pos),
            Source::Memory(bytes) => bytes.seek(pos),
        }
    }
}

impl Write for Source {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Source::File(file) => file.write(buf),
            Source::Memory(bytes) => bytes.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Source::File(file) => file.flush(),
            Source::Memory(bytes) => bytes.flush(),
        }
    }
}

impl Source {
    /// Reads `buf.len()` bytes at `offset`.
    pub fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.seek(SeekFrom::Start(offset))?;
        self.read_exact(buf)
    }

    /// Writes `bytes` at `offset`, extending the file if it ends before.
    pub fn write_at(&mut self, offset: u64, bytes: &[u8]) -> io::Result<()> {
        self.seek(SeekFrom::Start(offset))?;
        self.write_all(bytes)
    }
}

impl Source {
    /// The length in bytes.
    pub fn len(&mut self) -> io::Result<u64> {
        match self {
            Source::File(file) => Ok(file.metadata()?.len()),
            Source::Memory(bytes) => Ok(bytes.get_ref().len() as u64),
        }
    }

    /// Whether the source holds no bytes.
    pub fn is_empty(&mut self) -> io::Result<bool> {
        Ok(self.len()? == 0)
    }

    /// Cuts or extends (with zeros) the source to `length` bytes.
    pub fn set_len(&mut self, length: u64) -> io::Result<()> {
        match self {
            Source::File(file) => file.set_len(length),
            Source::Memory(bytes) => {
                bytes.get_mut().resize(length as usize, 0);
                Ok(())
            }
        }
    }
}
