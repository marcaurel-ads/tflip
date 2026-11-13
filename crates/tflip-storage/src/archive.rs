//! The tar and tar.gz archives of the backup procedures, byte for byte as the
//! original writes them (`fixtures/backup/README.md`, "Archives").

use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const BLOCK: usize = 512;

/// The longest member name; the header's name field holds 100 bytes with the
/// terminating NUL.
pub const MAX_NAME: usize = 99;

/// The gzip header the original writes: deflate, no flags, no mtime, XFL 4,
/// OS 11.
const GZIP_HEADER: [u8; 10] = [0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 4, 11];

#[derive(Debug)]
pub enum ArchiveError {
    /// A member name over 99 characters (the original's 5050).
    NameTooLong(String),
    /// A bad header checksum, a truncated archive or a gzip trailer that does
    /// not match (the original's 5198).
    Corrupt,
    Io(io::Error),
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NameTooLong(n) => write!(f, "archive member name too long: {n}"),
            Self::Corrupt => write!(f, "the archive is not valid"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ArchiveError {}

impl From<io::Error> for ArchiveError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Where a member's bytes come from when an archive is written.
pub enum Source<'a> {
    File(&'a Path),
    Bytes(&'a [u8]),
}

/// A member read back from an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub mtime: u64,
    pub data: Vec<u8>,
}

/// Writes `members` in order to `dest` as a tar, or as a tar in gzip when
/// `compressed`. `mtime` is stamped on every member. The names are checked
/// before `dest` is created, so a too-long name leaves no file behind.
pub fn write_archive(
    dest: &Path,
    members: &[(&str, Source<'_>)],
    mtime: u64,
    compressed: bool,
) -> Result<(), ArchiveError> {
    if let Some((name, _)) = members.iter().find(|(n, _)| n.len() > MAX_NAME) {
        return Err(ArchiveError::NameTooLong((*name).to_string()));
    }
    let file = io::BufWriter::new(std::fs::File::create(dest)?);
    let result = if compressed {
        write_gzip(file, members, mtime)
    } else {
        let mut file = file;
        write_tar(&mut file, members, mtime).and_then(|()| Ok(file.flush()?))
    };
    if result.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    result
}

fn write_gzip<W: Write>(
    mut out: W,
    members: &[(&str, Source<'_>)],
    mtime: u64,
) -> Result<(), ArchiveError> {
    out.write_all(&GZIP_HEADER)?;
    // The raw deflate stream: zlib level 1, windowBits -15, memLevel 8.
    let mut sink = Checked {
        inner: flate2::write::DeflateEncoder::new(out, flate2::Compression::new(1)),
        crc: flate2::Crc::new(),
    };
    write_tar(&mut sink, members, mtime)?;
    let Checked { inner, crc } = sink;
    let mut out = inner.finish()?;
    out.write_all(&crc.sum().to_le_bytes())?;
    out.write_all(&crc.amount().to_le_bytes())?;
    out.flush()?;
    Ok(())
}

/// Feeds the gzip trailer's CRC-32 and size with what the tar writer writes.
struct Checked<W> {
    inner: W,
    crc: flate2::Crc,
}

impl<W: Write> Write for Checked<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.crc.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn write_tar<W: Write>(
    out: &mut W,
    members: &[(&str, Source<'_>)],
    mtime: u64,
) -> Result<(), ArchiveError> {
    for (name, source) in members {
        // The header carries the size, so a file is read whole.
        let owned;
        let data: &[u8] = match source {
            Source::File(path) => {
                owned = std::fs::read(path)?;
                &owned
            }
            Source::Bytes(bytes) => bytes,
        };
        out.write_all(&header(name, data.len() as u64, mtime))?;
        out.write_all(data)?;
        out.write_all(&[0; BLOCK][..data.len().wrapping_neg() % BLOCK])?;
    }
    out.write_all(&[0; 2 * BLOCK])?;
    Ok(())
}

fn header(name: &str, size: u64, mtime: u64) -> [u8; BLOCK] {
    let mut h = [0u8; BLOCK];
    h[..name.len()].copy_from_slice(name.as_bytes());
    h[100..107].copy_from_slice(b"0000666");
    octal(&mut h[124..136], size);
    octal(&mut h[136..148], mtime);
    h[156] = b'0';
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    let sum = checksum(&h);
    h[148..154].copy_from_slice(format!("{sum:06o}").as_bytes());
    h[155] = b' ';
    h
}

/// Zero-padded octal filling the field but its last byte, which stays NUL.
fn octal(field: &mut [u8], value: u64) {
    let digits = field.len() - 1;
    field[..digits].copy_from_slice(format!("{value:0digits$o}").as_bytes());
}

/// The checksum counts its own field as spaces.
fn checksum(h: &[u8; BLOCK]) -> u32 {
    h.iter()
        .enumerate()
        .map(|(i, &b)| {
            if (148..156).contains(&i) {
                32
            } else {
                b as u32
            }
        })
        .sum()
}

/// Reads every member of the tar or tar.gz at `path`. With `compressed`, a
/// plain tar is read as well, as the original does; a gzip file read as plain
/// is `Corrupt`, since it is not a tar.
pub fn read_archive(path: &Path, compressed: bool) -> Result<Vec<Member>, ArchiveError> {
    let raw = std::fs::read(path)?;
    let tar = if compressed && raw.starts_with(&[0x1f, 0x8b]) {
        gunzip(&raw)?
    } else {
        raw
    };
    parse_tar(&tar)
}

fn gunzip(raw: &[u8]) -> Result<Vec<u8>, ArchiveError> {
    if raw.len() < 18 || raw[2] != 8 {
        return Err(ArchiveError::Corrupt);
    }
    let flags = raw[3];
    let mut pos = 10;
    if flags & 4 != 0 {
        let len = raw.get(pos..pos + 2).ok_or(ArchiveError::Corrupt)?;
        pos += 2 + u16::from_le_bytes([len[0], len[1]]) as usize;
    }
    for bit in [8, 16] {
        if flags & bit != 0 {
            let end = raw
                .get(pos..)
                .and_then(|r| r.iter().position(|&b| b == 0))
                .ok_or(ArchiveError::Corrupt)?;
            pos += end + 1;
        }
    }
    if flags & 2 != 0 {
        pos += 2;
    }
    let body = raw.get(pos..raw.len() - 8).ok_or(ArchiveError::Corrupt)?;
    let mut tar = Vec::new();
    flate2::read::DeflateDecoder::new(body)
        .read_to_end(&mut tar)
        .map_err(|_| ArchiveError::Corrupt)?;
    let trailer = &raw[raw.len() - 8..];
    let mut crc = flate2::Crc::new();
    crc.update(&tar);
    if trailer[..4] != crc.sum().to_le_bytes() || trailer[4..] != (tar.len() as u32).to_le_bytes() {
        return Err(ArchiveError::Corrupt);
    }
    Ok(tar)
}

fn parse_tar(tar: &[u8]) -> Result<Vec<Member>, ArchiveError> {
    let mut members = Vec::new();
    let mut pos = 0;
    loop {
        let Some(block) = tar.get(pos..pos + BLOCK) else {
            // The end marker is mandatory.
            return Err(ArchiveError::Corrupt);
        };
        if block.iter().all(|&b| b == 0) {
            return Ok(members);
        }
        let h: &[u8; BLOCK] = block.try_into().unwrap();
        let field = |a: usize, b: usize| {
            let f = &h[a..b];
            &f[..f.iter().position(|&c| c == 0).unwrap_or(f.len())]
        };
        let number = |a: usize, b: usize| -> Result<u64, ArchiveError> {
            let text = std::str::from_utf8(field(a, b)).map_err(|_| ArchiveError::Corrupt)?;
            u64::from_str_radix(text.trim(), 8).map_err(|_| ArchiveError::Corrupt)
        };
        if number(148, 156)? != checksum(h) as u64 {
            return Err(ArchiveError::Corrupt);
        }
        let size = number(124, 136)? as usize;
        let start = pos + BLOCK;
        let data = start
            .checked_add(size)
            .and_then(|end| tar.get(start..end))
            .ok_or(ArchiveError::Corrupt)?;
        members.push(Member {
            name: String::from_utf8_lossy(field(0, 100)).into_owned(),
            mtime: number(136, 148)?,
            data: data.to_vec(),
        });
        pos = start + size.div_ceil(BLOCK) * BLOCK;
    }
}

/// Unpacks the members of the archive into `dir` (created when missing) that
/// `keep` accepts, and returns the paths written in archive order.
pub fn extract_archive(
    path: &Path,
    compressed: bool,
    dir: &Path,
    keep: impl Fn(&str) -> bool,
) -> Result<Vec<PathBuf>, ArchiveError> {
    let members = read_archive(path, compressed)?;
    std::fs::create_dir_all(dir)?;
    let mut written = Vec::new();
    for m in members.into_iter().filter(|m| keep(&m.name)) {
        // A name is a plain file name; one that climbs out of `dir` is not
        // an archive the original wrote.
        if m.name.contains(['/', '\\']) || m.name.is_empty() {
            return Err(ArchiveError::Corrupt);
        }
        let target = dir.join(&m.name);
        std::fs::write(&target, &m.data)?;
        written.push(target);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_root;

    fn fixture(rel: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/backup")
            .join(rel)
    }

    /// Rebuilds the archive from its own members and the recorded mtime: the
    /// images before archiving were not kept, and the mtime is the one thing
    /// that differs per run.
    fn rebuild(rel: &str, compressed: bool) {
        let recorded = fixture(rel);
        let members = read_archive(&recorded, compressed).unwrap();
        assert!(!members.is_empty());
        let mtime = members[0].mtime;
        assert!(members.iter().all(|m| m.mtime == mtime));
        let list: Vec<(&str, Source)> = members
            .iter()
            .map(|m| (m.name.as_str(), Source::Bytes(&m.data)))
            .collect();
        let dest = temp_root().join(format!("archive-{}", rel.replace('/', "_")));
        write_archive(&dest, &list, mtime, compressed).unwrap();
        assert!(
            std::fs::read(&dest).unwrap() == std::fs::read(&recorded).unwrap(),
            "{rel} differs"
        );
    }

    #[test]
    fn recorded_tars_are_reproduced() {
        for rel in [
            "s70_archive/tree/ar1/x.tar",
            "s70_archive/tree/ar3/plain",
            "s70_archive/tree/ar4/x.tar",
            "s70_archive/tree/ar7/x.tar",
            "s72_archive_free/tree/ar1/x.tar",
        ] {
            rebuild(rel, false);
        }
    }

    #[test]
    fn recorded_gzips_are_reproduced() {
        for rel in [
            "s70_archive/tree/ar2/x.tar.gz",
            "s70_archive/tree/ar5/y.tgz",
            "s72_archive_free/tree/ar2/x.tar.gz",
        ] {
            rebuild(rel, true);
        }
    }

    #[test]
    fn member_order_and_contents() {
        let members = read_archive(&fixture("s72_archive_free/tree/ar1/x.tar"), false).unwrap();
        let names: Vec<_> = members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["f_adt.adt", "f_adt.adm", "f_adt.adi"]);
    }

    #[test]
    fn round_trip_and_extract() {
        let dir = temp_root().join("archive-roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let big: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let big_path = dir.join("src.bin");
        std::fs::write(&big_path, &big).unwrap();
        for compressed in [false, true] {
            let dest = dir.join(format!("a{compressed}"));
            let list = [
                ("one", Source::Bytes(b"hello")),
                ("empty", Source::Bytes(b"")),
                ("two.bin", Source::File(&big_path)),
                ("exact", Source::Bytes(&[7; 512])),
            ];
            write_archive(&dest, &list, 12345, compressed).unwrap();
            let got = read_archive(&dest, compressed).unwrap();
            assert_eq!(got.len(), 4);
            assert_eq!(got[0].data, b"hello");
            assert_eq!(got[1].data, b"");
            assert_eq!(got[2].data, big);
            assert_eq!(got[3].data, [7; 512]);
            assert_eq!(got[0].mtime, 12345);
            let out = dir.join(format!("out{compressed}"));
            let written = extract_archive(&dest, compressed, &out, |n| n != "one").unwrap();
            assert_eq!(written.len(), 3);
            assert_eq!(std::fs::read(out.join("two.bin")).unwrap(), big);
            assert!(!out.join("one").exists());
        }
    }

    #[test]
    fn gzip_given_as_plain_and_plain_given_as_compressed() {
        let gz = fixture("s70_archive/tree/ar2/x.tar.gz");
        assert!(matches!(
            read_archive(&gz, false),
            Err(ArchiveError::Corrupt)
        ));
        let plain = fixture("s70_archive/tree/ar1/x.tar");
        assert_eq!(read_archive(&plain, true).unwrap().len(), 10);
    }

    #[test]
    fn long_names_and_corrupt_input() {
        let dir = temp_root().join("archive-bad");
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("long.tar");
        let name = "a".repeat(100);
        let r = write_archive(&dest, &[(&name, Source::Bytes(b"x"))], 0, false);
        assert!(matches!(r, Err(ArchiveError::NameTooLong(_))));
        assert!(!dest.exists());
        let name = "a".repeat(99);
        write_archive(&dest, &[(&name, Source::Bytes(b"x"))], 0, false).unwrap();

        let good = std::fs::read(&dest).unwrap();
        let bad = |bytes: Vec<u8>, compressed: bool| {
            let p = dir.join("bad");
            std::fs::write(&p, bytes).unwrap();
            matches!(read_archive(&p, compressed), Err(ArchiveError::Corrupt))
        };
        let mut flipped = good.clone();
        flipped[0] ^= 1;
        assert!(bad(flipped, false));
        assert!(bad(good[..600].to_vec(), false));
        assert!(bad(good[..512].to_vec(), false));
        assert!(bad(Vec::new(), false));

        let gz = std::fs::read(fixture("s72_archive_free/tree/ar2/x.tar.gz")).unwrap();
        let n = gz.len();
        let mut crc = gz.clone();
        crc[n - 8] ^= 1;
        assert!(bad(crc, true));
        let mut size = gz.clone();
        size[n - 1] ^= 1;
        assert!(bad(size, true));
        assert!(bad(gz[..n / 2].to_vec(), true));
    }
}
