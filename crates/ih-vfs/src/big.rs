//! `.big` archive reader.
//!
//! Ported from `GameEngineDevice/Source/Win32Device/Common/Win32BIGFileSystem.cpp`
//! (`openArchiveFile`) and `Win32BIGFile.cpp`.
//!
//! Layout:
//! - `"BIGF"` magic
//! - archive size, u32 (ignored by the game)
//! - entry count, big-endian u32
//! - directory starts at offset 0x10
//! - each entry: offset (BE u32), size (BE u32), NUL-terminated path
//!
//! Entry paths are stored as raw bytes. They are decoded byte-for-byte (Latin-1)
//! so that every name round-trips and sorts in the original byte order.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::VfsError;

const MAGIC: &[u8; 4] = b"BIGF";
const DIRECTORY_START: u64 = 0x10;
/// The original reads names into a `_MAX_PATH` buffer.
const MAX_NAME: usize = 260;

/// One file stored inside a `.big` archive.
#[derive(Debug, Clone)]
pub struct BigEntry {
    /// Path as stored in the archive (original case and separators).
    pub name: String,
    pub offset: u32,
    pub size: u32,
}

/// An opened `.big` archive. Reads are serialised through a mutex.
#[derive(Debug)]
pub struct BigArchive {
    path: PathBuf,
    len: u64,
    entries: Vec<BigEntry>,
    file: Mutex<File>,
}

/// Decodes raw bytes as Latin-1.
pub(crate) fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

impl BigArchive {
    /// Opens and indexes an archive. Fails with [`VfsError::NotBig`] if the file does
    /// not start with `BIGF`, which the original also rejects.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, VfsError> {
        let path = path.as_ref().to_path_buf();
        let io = |e| VfsError::Io(path.clone(), e);
        let file = File::open(&path).map_err(io)?;
        let len = file.metadata().map_err(io)?.len();
        let mut r = BufReader::new(file.try_clone().map_err(io)?);

        let mut magic = [0u8; 4];
        if r.read_exact(&mut magic).is_err() || &magic != MAGIC {
            return Err(VfsError::NotBig(path));
        }
        let mut word = [0u8; 4];
        r.read_exact(&mut word).map_err(io)?; // archive size, unused
        r.read_exact(&mut word).map_err(io)?;
        let count = u32::from_be_bytes(word);

        r.seek(SeekFrom::Start(DIRECTORY_START)).map_err(io)?;
        let corrupt = |why: &str| VfsError::Corrupt(path.clone(), why.to_string());
        // A directory entry is at least 9 bytes, which bounds a sane count.
        if u64::from(count) > len / 9 {
            return Err(corrupt("entry count larger than the file allows"));
        }

        let mut entries = Vec::with_capacity(count as usize);
        let mut name = Vec::with_capacity(64);
        for _ in 0..count {
            r.read_exact(&mut word).map_err(|_| corrupt("truncated directory"))?;
            let offset = u32::from_be_bytes(word);
            r.read_exact(&mut word).map_err(|_| corrupt("truncated directory"))?;
            let size = u32::from_be_bytes(word);
            name.clear();
            loop {
                let mut b = [0u8; 1];
                r.read_exact(&mut b).map_err(|_| corrupt("truncated entry name"))?;
                if b[0] == 0 {
                    break;
                }
                if name.len() >= MAX_NAME {
                    return Err(corrupt("entry name longer than MAX_PATH"));
                }
                name.push(b[0]);
            }
            entries.push(BigEntry { name: latin1(&name), offset, size });
        }

        Ok(Self { path, len, entries, file: Mutex::new(file) })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn entries(&self) -> &[BigEntry] {
        &self.entries
    }

    /// Reads one entry's bytes.
    pub fn read(&self, entry: &BigEntry) -> Result<Vec<u8>, VfsError> {
        let end = u64::from(entry.offset) + u64::from(entry.size);
        if end > self.len {
            return Err(VfsError::Corrupt(
                self.path.clone(),
                format!("entry '{}' runs past the end of the archive", entry.name),
            ));
        }
        let io = |e| VfsError::Io(self.path.clone(), e);
        let mut buf = vec![0u8; entry.size as usize];
        let mut f = self.file.lock().unwrap_or_else(|p| p.into_inner());
        f.seek(SeekFrom::Start(u64::from(entry.offset))).map_err(io)?;
        f.read_exact(&mut buf).map_err(io)?;
        Ok(buf)
    }
}

/// Builds a `.big` archive in memory. Used by tests and tools.
pub fn write_big(files: &[(&str, &[u8])]) -> Vec<u8> {
    let dir_len: usize = files.iter().map(|(n, _)| 8 + n.len() + 1).sum();
    let data_start = DIRECTORY_START as usize + dir_len;
    let total = data_start + files.iter().map(|(_, d)| d.len()).sum::<usize>();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(files.len() as u32).to_be_bytes());
    out.extend_from_slice(&(data_start as u32).to_be_bytes());
    let mut offset = data_start;
    for (name, data) in files {
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        offset += data.len();
    }
    for (_, data) in files {
        out.extend_from_slice(data);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.big");
        std::fs::write(&p, write_big(&[("Data\\INI\\A.ini", b"hello"), ("art/x.w3d", b"w3d")])).unwrap();
        let a = BigArchive::open(&p).unwrap();
        assert_eq!(a.entries().len(), 2);
        assert_eq!(a.entries()[0].name, "Data\\INI\\A.ini");
        assert_eq!(a.read(&a.entries()[0]).unwrap(), b"hello");
        assert_eq!(a.read(&a.entries()[1]).unwrap(), b"w3d");
    }

    #[test]
    fn rejects_non_big() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("generals.ctr");
        std::fs::write(&p, b"MZ\x90\x00 not an archive").unwrap();
        assert!(matches!(BigArchive::open(&p), Err(VfsError::NotBig(_))));
    }
}
