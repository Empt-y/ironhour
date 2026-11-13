//! Finding models and textures in the game's file system.
//!
//! Follows `W3DDevice/GameClient/W3DFileSystem.cpp` (`GameFileClass::Set_Name`):
//! a file is looked up in `Data\<language>\Art\W3D\` (or `...\Art\Textures\`), then
//! `Art\W3D\` (`Art\Textures\`). Render objects are loaded from the file named after
//! them (`W3DAssetManager`): `AVCrusader` from `avcrusader.w3d`, a mesh
//! `CONTAINER.MESH` from `container.w3d`, a hierarchy from `<name>.w3d`, and an
//! animation `SKELETON.ANIM` from `anim.w3d`. Textures named `.tga` are loaded from the
//! `.dds` of the same name when it exists (`textureloader.cpp`).

use std::collections::HashMap;
use std::sync::Arc;

use ih_formats::texture::{self, Image};
use ih_formats::w3d::{self, Animation, Hierarchy, W3dFile};
use ih_vfs::Vfs;

pub struct Assets<'v> {
    vfs: &'v Vfs,
    language: String,
    files: HashMap<String, Option<Arc<W3dFile>>>,
    textures: HashMap<String, Option<Arc<Image>>>,
    /// Problems met while loading, for reporting.
    pub warnings: Vec<String>,
}

impl<'v> Assets<'v> {
    pub fn new(vfs: &'v Vfs, language: &str) -> Self {
        Self {
            vfs,
            language: language.to_string(),
            files: HashMap::new(),
            textures: HashMap::new(),
            warnings: Vec::new(),
        }
    }

    fn read(&self, dir: &str, file: &str) -> Option<Vec<u8>> {
        for d in [format!("Data\\{}\\Art\\{dir}\\", self.language), format!("Art\\{dir}\\")] {
            if let Ok(Some(b)) = self.vfs.read(&format!("{d}{file}")) {
                return Some(b);
            }
        }
        None
    }

    /// A `.w3d` file by stem (`avcrusader`), cached.
    pub fn w3d(&mut self, stem: &str) -> Option<Arc<W3dFile>> {
        let key = stem.to_ascii_lowercase();
        if let Some(f) = self.files.get(&key) {
            return f.clone();
        }
        let parsed = self.read("W3D", &format!("{key}.w3d")).and_then(|b| match w3d::parse(&b) {
            Ok(f) => {
                for e in &f.errors {
                    self.warnings.push(format!("{key}.w3d: {e}"));
                }
                Some(Arc::new(f))
            }
            Err(e) => {
                self.warnings.push(format!("{key}.w3d: {e}"));
                None
            }
        });
        self.files.insert(key, parsed.clone());
        parsed
    }

    /// A mesh by render object name (`CONTAINER.MESH`), from the container's file or,
    /// failing that, `hint` (the file being assembled).
    pub fn mesh(&mut self, full_name: &str, hint: &Arc<W3dFile>) -> Option<(Arc<W3dFile>, usize)> {
        let find = |f: &Arc<W3dFile>| f.meshes.iter().position(|m| m.full_name().eq_ignore_ascii_case(full_name));
        if let Some(i) = find(hint) {
            return Some((hint.clone(), i));
        }
        let container = full_name.split('.').next().unwrap_or(full_name);
        let f = self.w3d(container)?;
        find(&f).map(|i| (f, i))
    }

    pub fn hierarchy(&mut self, name: &str, hint: Option<&Arc<W3dFile>>) -> Option<Hierarchy> {
        let find = |f: &W3dFile| f.hierarchies.iter().find(|h| h.name.eq_ignore_ascii_case(name)).cloned();
        if let Some(h) = hint.and_then(|f| find(f)) {
            return Some(h);
        }
        find(&*self.w3d(name)?)
    }

    /// An animation by `SKELETON.ANIM` (or just `ANIM`), loaded from `anim.w3d`.
    pub fn animation(&mut self, name: &str) -> Option<Animation> {
        let file = name.rsplit('.').next().unwrap_or(name);
        let f = self.w3d(file)?;
        let want = |a: &&Animation| a.name.eq_ignore_ascii_case(name) || a.name.eq_ignore_ascii_case(file);
        f.animations.iter().find(want).or_else(|| f.animations.first()).cloned()
    }

    /// A texture by the name a model uses, cached. Tries the `.dds` first.
    pub fn texture(&mut self, name: &str) -> Option<Arc<Image>> {
        let key = name.to_ascii_lowercase();
        if let Some(t) = self.textures.get(&key) {
            return t.clone();
        }
        let base = key.rsplit_once('.').map_or(key.as_str(), |(b, _)| b).to_string();
        let mut found = None;
        for ext in ["dds", "tga"] {
            if let Some(b) = self.read("Textures", &format!("{base}.{ext}")) {
                match texture::decode(&b) {
                    Ok(img) => {
                        found = Some(Arc::new(img));
                        break;
                    }
                    Err(e) => self.warnings.push(format!("{base}.{ext}: {e}")),
                }
            }
        }
        if found.is_none() {
            self.warnings.push(format!("texture {name} not found"));
        }
        self.textures.insert(key, found.clone());
        found
    }
}
