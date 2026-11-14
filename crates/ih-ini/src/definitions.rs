//! Startup definitions in `GameEngine::init` load order. Shared by the client
//! and headless logic; storage never depends on rendering types.
use crate::{Block, Document};
use ih_vfs::Vfs;
use std::collections::BTreeMap;

#[derive(Clone, Default)]
pub struct Definitions {
    by_kind: BTreeMap<String, BTreeMap<String, Vec<Block>>>,
    order: Vec<(String, String, usize)>,
    named_order: BTreeMap<String, Vec<(String, usize)>>,
    pub files: usize,
}
impl Definitions {
    pub fn load(vfs: &Vfs, language: &str) -> Self {
        let (files, _) = crate::load_order::plan(vfs, language, crate::load_order::STARTUP);
        let mut d = Self::default();
        for f in files {
            if let Ok(Some(bytes)) = vfs.read(&f.path) {
                d.add_document(crate::parse(&bytes));
                d.files += 1;
            }
        }
        d
    }
    pub fn load_map_overrides(&mut self, vfs: &Vfs, map_path: &str) -> Result<(), ih_vfs::VfsError> {
        let path = map_path.replace('/', "\\");
        if let Some((folder, _)) = path.rsplit_once('\\')
            && let Some(bytes) = vfs.read(&format!("{folder}\\map.ini"))?
        {
            self.add_document(crate::parse(&bytes));
            self.files += 1;
        }
        Ok(())
    }
    pub fn add_document(&mut self, doc: Document) {
        for b in doc.blocks {
            let name = b.args().into_iter().next().unwrap_or_default();
            let index = self.all(&b.kind, &name).len();
            self.order.push((b.kind.clone(), name.clone(), index));
            self.named_order.entry(name.clone()).or_default().push((b.kind.clone(), index));
            self.by_kind.entry(b.kind.clone()).or_default().entry(name).or_default().push(b);
        }
    }
    pub fn get(&self, kind: &str, name: &str) -> Option<&Block> {
        self.all(kind, name).last()
    }
    /// All definition kinds for one name in original file order (e.g. a reskin
    /// followed later by an Object override with the same name).
    pub fn layers<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Block> {
        self.named_order.get(name).into_iter().flatten().map(move |(kind, index)| &self.all(kind, name)[*index])
    }
    pub fn all(&self, kind: &str, name: &str) -> &[Block] {
        self.by_kind.get(kind).and_then(|m| m.get(name)).map_or(&[], |v| v.as_slice())
    }
    pub fn names(&self, kind: &str) -> impl Iterator<Item = &String> {
        self.by_kind.get(kind).into_iter().flat_map(|m| m.keys())
    }
    pub fn ordered<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Block> {
        self.order.iter().filter(move |(k, _, _)| k == kind).map(|(k, n, i)| &self.all(k, n)[*i])
    }
    pub fn field(&self, kind: &str, name: &str, field: &str) -> Option<Vec<String>> {
        self.get(kind, name)?.fields().filter(|f| f.name.eq_ignore_ascii_case(field)).last().map(|f| f.values())
    }
}
