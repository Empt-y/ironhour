//! Loose files on disk.
//!
//! Stands in for `Win32LocalFileSystem.cpp`. The original relies on the working
//! directory being the game folder and on Windows' case-insensitive file system.
//! Here the root is indexed once with lowercased keys so lookups behave the same on
//! case-sensitive platforms.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::VfsError;
use crate::path::{key, wildcard_matches_nocase};

/// An indexed directory tree of loose files.
#[derive(Debug, Default)]
pub struct LooseRoot {
    root: PathBuf,
    /// Lowercased relative key -> (relative path with original case, absolute path).
    files: BTreeMap<String, (String, PathBuf)>,
}

impl LooseRoot {
    pub fn index(root: impl AsRef<Path>) -> Result<Self, VfsError> {
        let root = root.as_ref().to_path_buf();
        let mut files = BTreeMap::new();
        walk(&root, "", &mut files)?;
        Ok(Self { root, files })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn find(&self, path: &str) -> Option<&Path> {
        self.files.get(&key(path)).map(|(_, abs)| abs.as_path())
    }

    /// Files under `dir` whose name matches `mask`, as relative paths in original case.
    ///
    /// Mirrors `Win32LocalFileSystem::getFileListInDirectory`: files directly in
    /// `dir`, then (if `recursive`) everything below it.
    pub fn list<'a>(&'a self, dir: &str, mask: &'a str, recursive: bool) -> impl Iterator<Item = &'a str> + 'a {
        let dir_key = key(dir);
        let prefix = if dir_key.is_empty() { String::new() } else { format!("{dir_key}\\") };
        let bound = prefix.clone();
        self.files.range(prefix.clone()..).take_while(move |(k, _)| k.starts_with(&bound)).filter_map(
            move |(k, (rel, _))| {
                let sub = &k[prefix.len()..];
                if !recursive && sub.contains('\\') {
                    return None;
                }
                let name = rel.rsplit('\\').next().unwrap_or(rel);
                wildcard_matches_nocase(name, mask).then_some(rel.as_str())
            },
        )
    }
}

fn walk(dir: &Path, rel: &str, out: &mut BTreeMap<String, (String, PathBuf)>) -> Result<(), VfsError> {
    let rd = std::fs::read_dir(dir).map_err(|e| VfsError::Io(dir.to_path_buf(), e))?;
    for entry in rd {
        let entry = entry.map_err(|e| VfsError::Io(dir.to_path_buf(), e))?;
        let ft = entry.file_type().map_err(|e| VfsError::Io(entry.path(), e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let child_rel = if rel.is_empty() { name } else { format!("{rel}\\{name}") };
        if ft.is_dir() {
            walk(&entry.path(), &child_rel, out)?;
        } else if ft.is_file() {
            // On a case-insensitive file system there are no collisions. On others the
            // first name in directory order wins, which is as good as any.
            out.entry(key(&child_rel)).or_insert((child_rel, entry.path()));
        }
    }
    Ok(())
}
