//! Virtual file system for game data.
//!
//! Reproduces the lookup rules of the original engine:
//!
//! 1. Loose files under the game folder win over everything
//!    (`FileSystem::openFile` asks the local file system first).
//! 2. Archives are mounted in batches. A batch is every archive under a folder,
//!    searched recursively, sorted case-insensitively by path
//!    (`Win32BIGFileSystem::loadBigFilesFromDirectory`).
//! 3. Normal batches use first-registered-wins, which is why `!`-prefixed archives
//!    override stock ones. Mod batches (`-mod`) use overwrite, so they beat
//!    everything mounted before them, and within a mod folder the *last* archive
//!    wins (`ArchiveFileSystem::loadIntoDirectoryTree`, `loadMods`).
//!
//! Directory listings are the union of loose files and every archive's entries,
//! deduplicated and sorted case-insensitively, as the original's `FilenameList` is.

pub mod big;
pub mod install;
mod loose;
pub mod path;
pub mod profile;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use big::{BigArchive, BigEntry};
pub use install::Installs;
pub use loose::LooseRoot;
pub use profile::ModProfile;

#[derive(Debug, thiserror::Error)]
pub enum VfsError {
    #[error("{0}: {1}")]
    Io(PathBuf, #[source] std::io::Error),
    #[error("{0}: not a BIG archive")]
    NotBig(PathBuf),
    #[error("{0}: corrupt archive: {1}")]
    Corrupt(PathBuf, String),
    #[error("{0}: invalid mod profile: {1}")]
    Profile(PathBuf, String),
}

/// Where a game path resolved to.
#[derive(Debug, Clone, Copy)]
pub enum Source<'a> {
    Loose(&'a Path),
    Archive { archive: &'a BigArchive, entry: &'a BigEntry },
}

impl Source<'_> {
    pub fn describe(&self) -> String {
        match self {
            Source::Loose(p) => format!("loose {}", p.display()),
            Source::Archive { archive, entry } => {
                format!("{} :: {}", archive.path().display(), entry.name)
            }
        }
    }
}

/// How to mount a batch of archives.
#[derive(Debug, Clone)]
pub struct MountOptions {
    /// File extensions (without dot, case-insensitive) that count as archives.
    pub extensions: Vec<String>,
    /// File names (wildcards allowed, case-insensitive) to skip.
    pub exclude: Vec<String>,
    /// Later archives replace earlier entries (mod behaviour).
    pub overwrite: bool,
    /// Search subfolders, as the original always does for archive folders.
    pub recursive: bool,
}

impl Default for MountOptions {
    fn default() -> Self {
        Self { extensions: vec!["big".into()], exclude: Vec::new(), overwrite: false, recursive: true }
    }
}

/// A mounted archive and why it was mounted.
#[derive(Debug)]
pub struct Mounted {
    pub archive: Arc<BigArchive>,
    pub overwrite: bool,
}

/// A problem met while mounting that the original would also have skipped past.
#[derive(Debug)]
pub struct MountWarning(pub String);

#[derive(Debug, Clone, Copy)]
struct EntryRef {
    archive: u32,
    entry: u32,
}

/// Loose files plus mounted archives, resolved with the original priority rules.
#[derive(Debug, Default)]
pub struct Vfs {
    loose: Option<LooseRoot>,
    archives: Vec<Mounted>,
    /// Archive key -> winning entry.
    index: BTreeMap<String, EntryRef>,
    /// Every archive entry path (plain key -> display name), for listings.
    names: BTreeMap<String, String>,
    warnings: Vec<MountWarning>,
}

impl Vfs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mounts the game the way the original starts up: loose files and archives from
    /// the Zero Hour folder, then archives from the base Generals folder, then the
    /// profile's mod archives on top.
    pub fn for_game(installs: &Installs, profile: &ModProfile) -> Result<Self, VfsError> {
        let mut vfs = Self::new();
        let zh = installs.zero_hour.as_ref();
        if let Some(zh) = zh {
            vfs.set_loose_root(zh)?;
            vfs.mount_dir(
                zh,
                &MountOptions {
                    extensions: profile.archive_extensions(),
                    exclude: profile.exclude.clone(),
                    ..MountOptions::default()
                },
            )?;
        }
        if let Some(g) = installs.generals.as_ref() {
            vfs.mount_dir(g, &MountOptions::default())?;
        }
        for big in &profile.mod_big {
            vfs.mount_archive(big, true)?;
        }
        for dir in &profile.mod_dir {
            vfs.mount_dir(dir, &MountOptions { overwrite: true, ..MountOptions::default() })?;
        }
        Ok(vfs)
    }

    pub fn set_loose_root(&mut self, root: impl AsRef<Path>) -> Result<(), VfsError> {
        self.loose = Some(LooseRoot::index(root)?);
        Ok(())
    }

    pub fn loose_root(&self) -> Option<&LooseRoot> {
        self.loose.as_ref()
    }

    /// Mounts every archive under `dir` in case-insensitive path order.
    /// Files that are not BIG archives are skipped with a warning, as the original does.
    pub fn mount_dir(&mut self, dir: impl AsRef<Path>, opts: &MountOptions) -> Result<usize, VfsError> {
        let dir = dir.as_ref();
        let mut found = Vec::new();
        collect_archives(dir, "", opts, &mut found)?;
        found.sort_by(|(a, _), (b, _)| path::cmp_nocase(a, b));
        let mut n = 0;
        for (_, abs) in found {
            match self.mount_archive(&abs, opts.overwrite) {
                Ok(()) => n += 1,
                Err(e @ (VfsError::NotBig(_) | VfsError::Corrupt(..))) => {
                    self.warnings.push(MountWarning(format!("skipped {e}")));
                }
                Err(e) => return Err(e),
            }
        }
        Ok(n)
    }

    /// Mounts a single archive.
    pub fn mount_archive(&mut self, path: impl AsRef<Path>, overwrite: bool) -> Result<(), VfsError> {
        let archive = Arc::new(BigArchive::open(path)?);
        let id = self.archives.len() as u32;

        // Within one archive a later duplicate replaces an earlier one
        // (ArchiveFile::addFile assigns into a map).
        let mut own: BTreeMap<String, u32> = BTreeMap::new();
        for (i, e) in archive.entries().iter().enumerate() {
            let plain = path::key(&e.name);
            self.names.entry(plain).or_insert_with(|| e.name.replace('/', "\\").to_ascii_lowercase());
            if let Some(k) = path::archive_key(&e.name) {
                own.insert(k, i as u32);
            }
        }
        for (k, entry) in own {
            let r = EntryRef { archive: id, entry };
            if overwrite {
                self.index.insert(k, r);
            } else {
                self.index.entry(k).or_insert(r);
            }
        }
        self.archives.push(Mounted { archive, overwrite });
        Ok(())
    }

    pub fn archives(&self) -> &[Mounted] {
        &self.archives
    }

    pub fn warnings(&self) -> &[MountWarning] {
        &self.warnings
    }

    /// Resolves a game path to the file that would be opened.
    pub fn resolve(&self, path: &str) -> Option<Source<'_>> {
        if let Some(p) = self.loose.as_ref().and_then(|l| l.find(path)) {
            return Some(Source::Loose(p));
        }
        self.resolve_archive(path)
    }

    fn resolve_archive(&self, path: &str) -> Option<Source<'_>> {
        let r = self.index.get(&path::archive_key(path)?)?;
        let archive = &self.archives[r.archive as usize].archive;
        Some(Source::Archive { archive, entry: &archive.entries()[r.entry as usize] })
    }

    /// Every source that provides `path`, highest priority first. Useful for seeing
    /// what a mod overrides.
    pub fn all_sources(&self, path: &str) -> Vec<Source<'_>> {
        let mut out = Vec::new();
        if let Some(p) = self.loose.as_ref().and_then(|l| l.find(path)) {
            out.push(Source::Loose(p));
        }
        let Some(want) = path::archive_key(path) else { return out };
        let winner = self.resolve_archive(path);
        if let Some(w) = winner {
            out.push(w);
        }
        for m in &self.archives {
            for e in m.archive.entries() {
                if path::archive_key(&e.name).as_deref() == Some(want.as_str()) {
                    let is_winner = matches!(winner, Some(Source::Archive { entry, .. }) if std::ptr::eq(entry, e));
                    if !is_winner {
                        out.push(Source::Archive { archive: &m.archive, entry: e });
                    }
                }
            }
        }
        out
    }

    pub fn exists(&self, path: &str) -> bool {
        self.resolve(path).is_some()
    }

    /// Reads a file, or `Ok(None)` if no source provides it.
    pub fn read(&self, path: &str) -> Result<Option<Vec<u8>>, VfsError> {
        match self.resolve(path) {
            None => Ok(None),
            Some(Source::Loose(p)) => std::fs::read(p).map(Some).map_err(|e| VfsError::Io(p.to_path_buf(), e)),
            Some(Source::Archive { archive, entry }) => archive.read(entry).map(Some),
        }
    }

    /// Lists files under `dir` matching `mask`, sorted and deduplicated
    /// case-insensitively. Results start with `dir` as given.
    ///
    /// Mirrors `FileSystem::getFileListInDirectory`: loose files honour `recursive`,
    /// but archive listings always include subfolders
    /// (`ArchiveFile::getFileListInDirectory` recurses unconditionally). Archive
    /// names are matched case-sensitively against their lowercase names.
    pub fn list(&self, dir: &str, mask: &str, recursive: bool) -> Vec<String> {
        let dir_key = path::key(dir);
        let prefix = if dir_key.is_empty() { String::new() } else { format!("{dir_key}\\") };
        let mut shown = dir.to_string();
        if !shown.is_empty() && !shown.ends_with(['\\', '/']) {
            shown.push('\\');
        }

        // Lowercased key -> display name; the first insertion wins, as in std::set.
        let mut out: BTreeMap<String, String> = BTreeMap::new();
        if let Some(l) = &self.loose {
            for rel in l.list(dir, mask, recursive) {
                let display = format!("{shown}{}", &rel[prefix.len()..]);
                out.entry(path::lower(&display)).or_insert(display);
            }
        }
        for (k, name) in self.names.range(prefix.clone()..) {
            if !k.starts_with(&prefix) {
                break;
            }
            let file = name.rsplit('\\').next().unwrap_or(name);
            if path::search_string_matches(file, mask) {
                let display = format!("{shown}{}", &name[prefix.len()..]);
                out.entry(path::lower(&display)).or_insert(display);
            }
        }
        // BTreeMap order over lowercase keys equals the original's _stricmp order.
        out.into_values().collect()
    }

    /// Number of distinct files the archives make reachable.
    pub fn archive_file_count(&self) -> usize {
        self.index.len()
    }
}

fn collect_archives(
    dir: &Path,
    rel: &str,
    opts: &MountOptions,
    out: &mut Vec<(String, PathBuf)>,
) -> Result<(), VfsError> {
    let rd = std::fs::read_dir(dir).map_err(|e| VfsError::Io(dir.to_path_buf(), e))?;
    let mut subdirs = Vec::new();
    for entry in rd {
        let entry = entry.map_err(|e| VfsError::Io(dir.to_path_buf(), e))?;
        let ft = entry.file_type().map_err(|e| VfsError::Io(entry.path(), e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let child_rel = if rel.is_empty() { name.clone() } else { format!("{rel}\\{name}") };
        if ft.is_dir() {
            if opts.recursive {
                subdirs.push((entry.path(), child_rel));
            }
        } else if ft.is_file() {
            let ext_ok = name
                .rsplit_once('.')
                .is_some_and(|(_, ext)| opts.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)));
            let excluded = opts.exclude.iter().any(|pat| path::wildcard_matches_nocase(&name, pat));
            if ext_ok && !excluded {
                out.push((child_rel, entry.path()));
            }
        }
    }
    for (p, r) in subdirs {
        collect_archives(&p, &r, opts, out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::big::write_big;

    fn put(dir: &Path, rel: &str, bytes: &[u8]) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    fn read(v: &Vfs, p: &str) -> Option<String> {
        v.read(p).unwrap().map(|b| String::from_utf8(b).unwrap())
    }

    #[test]
    fn priority_rules() {
        let t = tempfile::tempdir().unwrap();
        let zh = t.path().join("zh");
        let base = t.path().join("gen");
        let modd = t.path().join("mod");
        put(
            &zh,
            "INIZH.big",
            &write_big(&[("Data\\INI\\Weapon.ini", b"stock"), ("Data\\INI\\Armor.ini", b"stock armor")]),
        );
        put(&zh, "!Patch.big", &write_big(&[("data/ini/weapon.ini", b"bang")]));
        put(&zh, "sub\\Early.big", &write_big(&[("data\\ini\\armor.ini", b"subfolder")]));
        put(&zh, "generals.ctr", b"MZ not an archive");
        put(&zh, "Data\\INI\\Loose.ini", b"loose");
        put(&base, "INI.big", &write_big(&[("Data\\INI\\Weapon.ini", b"generals"), ("Data\\INI\\Only.ini", b"base")]));
        put(&modd, "a.big", &write_big(&[("Data\\INI\\Only.ini", b"mod a")]));
        put(&modd, "b.big", &write_big(&[("Data\\INI\\Only.ini", b"mod b")]));

        let installs = Installs { zero_hour: Some(zh.clone()), generals: Some(base) };
        let mut profile = ModProfile::default();
        let v = Vfs::for_game(&installs, &profile).unwrap();
        assert_eq!(read(&v, "DATA\\INI\\WEAPON.INI").as_deref(), Some("bang"), "! sorts first and first wins");
        assert_eq!(read(&v, "Data/INI/Armor.ini").as_deref(), Some("stock armor"), "INIZH sorts before sub\\");
        assert_eq!(read(&v, "Data\\INI\\Only.ini").as_deref(), Some("base"), "Generals archives are the fallback");
        assert_eq!(read(&v, "data\\ini\\loose.ini").as_deref(), Some("loose"), "loose files win");
        assert!(v.read("Data\\INI\\Missing.ini").unwrap().is_none());

        profile.extensions = vec!["ctr".into()];
        profile.mod_dir = vec![modd];
        let v = Vfs::for_game(&installs, &profile).unwrap();
        assert_eq!(read(&v, "Data\\INI\\Only.ini").as_deref(), Some("mod b"), "mod folder: last archive wins");
        assert_eq!(v.warnings().len(), 1, "generals.ctr is skipped, not fatal");
    }

    #[test]
    fn listings_merge_and_sort() {
        let t = tempfile::tempdir().unwrap();
        let zh = t.path().join("zh");
        put(
            &zh,
            "INIZH.big",
            &write_big(&[
                ("Data\\INI\\Object\\B.ini", b""),
                ("Data\\INI\\Object\\Sub\\C.ini", b""),
                ("Data\\INI\\Object\\x.txt", b""),
            ]),
        );
        put(&zh, "Data\\INI\\Object\\A.ini", b"");
        put(&zh, "Data\\INI\\Object\\Deep\\D.ini", b"");
        let v = Vfs::for_game(&Installs { zero_hour: Some(zh), generals: None }, &ModProfile::default()).unwrap();
        let got = v.list("Data\\INI\\Object", "*.ini", false);
        // Loose honours `recursive = false`; archive listings always recurse.
        assert_eq!(got, ["Data\\INI\\Object\\A.ini", "Data\\INI\\Object\\b.ini", "Data\\INI\\Object\\sub\\c.ini"]);
        let got = v.list("Data\\INI\\Object\\", "*.ini", true);
        assert_eq!(
            got,
            [
                "Data\\INI\\Object\\A.ini",
                "Data\\INI\\Object\\b.ini",
                "Data\\INI\\Object\\Deep\\D.ini",
                "Data\\INI\\Object\\sub\\c.ini"
            ]
        );
    }
}
