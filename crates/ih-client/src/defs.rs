//! Every INI definition the game loads at startup, indexed by kind and name.
//!
//! Files are read in the original's load order (`ih_ini::load_order`). Later
//! definitions of the same name come after earlier ones; callers decide how they
//! combine (most kinds replace, `Object` re-parses over the existing template).

use std::collections::HashMap;

use ih_ini::Block;
use ih_vfs::Vfs;

#[derive(Default)]
pub struct Definitions {
    /// Kind (as written, e.g. `Object`) to name to definitions in load order.
    by_kind: HashMap<String, HashMap<String, Vec<Block>>>,
    pub files: usize,
}

impl Definitions {
    pub fn load(vfs: &Vfs, language: &str) -> Self {
        let (files, _missing) = ih_ini::load_order::plan(vfs, language, ih_ini::load_order::STARTUP);
        let mut d = Definitions::default();
        for f in &files {
            let Ok(Some(bytes)) = vfs.read(&f.path) else { continue };
            d.add_document(ih_ini::parse(&bytes));
            d.files += 1;
        }
        d
    }

    /// Adds a parsed file's blocks (e.g. a map's `map.ini`).
    pub fn add_document(&mut self, doc: ih_ini::Document) {
        for b in doc.blocks {
            let name = b.args().into_iter().next().unwrap_or_default();
            self.by_kind.entry(b.kind.clone()).or_default().entry(name).or_default().push(b);
        }
    }

    /// The last definition of `name`.
    pub fn get(&self, kind: &str, name: &str) -> Option<&Block> {
        self.by_kind.get(kind)?.get(name)?.last()
    }

    /// Every definition of `name`, in load order.
    pub fn all(&self, kind: &str, name: &str) -> &[Block] {
        self.by_kind.get(kind).and_then(|m| m.get(name)).map_or(&[], |v| v.as_slice())
    }

    pub fn names(&self, kind: &str) -> impl Iterator<Item = &String> {
        self.by_kind.get(kind).into_iter().flat_map(|m| m.keys())
    }

    /// `TerrainType` name to its texture file (`Texture = ...`, under `Art\Terrain\`).
    pub fn terrain_textures(&self) -> HashMap<String, String> {
        let mut out = HashMap::new();
        for name in self.names("Terrain") {
            if let Some(b) = self.get("Terrain", name)
                && let Some(f) = b.fields().filter(|f| f.name == "Texture").last()
                && let Some(v) = f.values().into_iter().next()
            {
                out.insert(name.clone(), v);
            }
        }
        out
    }
}

/// Parses an INI colour such as `R:185 G:185 B:185 A:255` (missing parts default to
/// 0, alpha to 255).
pub fn parse_rgba(values: &[String]) -> [u8; 4] {
    let mut c = [0u8, 0, 0, 255];
    for v in values {
        let (k, n) = v.split_once(':').unwrap_or(("", v));
        let n = n.trim().parse::<f32>().unwrap_or(0.0).clamp(0.0, 255.0) as u8;
        match k.trim().to_ascii_uppercase().as_str() {
            "R" => c[0] = n,
            "G" => c[1] = n,
            "B" => c[2] = n,
            "A" => c[3] = n,
            _ => {}
        }
    }
    c
}

impl Definitions {
    /// The last value of `field` in the last definition of `kind name`.
    pub fn field(&self, kind: &str, name: &str, field: &str) -> Option<Vec<String>> {
        self.get(kind, name)?.fields().filter(|f| f.name.eq_ignore_ascii_case(field)).last().map(|f| f.values())
    }

    /// Water settings for a time of day (1 morning .. 4 night): the `WaterSet` and
    /// `WaterTransparency` blocks of `Water.ini` (and a map's `map.ini`).
    pub fn water_settings(&self, time_of_day: i32) -> ih_render::water::WaterSettings {
        let mut s = ih_render::water::WaterSettings::default();
        let tod = ["MORNING", "AFTERNOON", "EVENING", "NIGHT"][(time_of_day - 1).clamp(0, 3) as usize];
        if let Some(v) = self.field("WaterSet", tod, "DiffuseColor") {
            s.diffuse = parse_rgba(&v);
        }
        let num = |f: &str| self.field("WaterTransparency", "", f).and_then(|v| v.first()?.parse::<f32>().ok());
        if let Some(v) = num("TransparentWaterDepth") {
            s.transparent_depth = v;
        }
        if let Some(v) = num("TransparentWaterMinOpacity") {
            s.min_opacity = v;
        }
        if let Some(v) = self.field("WaterTransparency", "", "StandingWaterColor") {
            let c = parse_rgba(&v);
            s.standing_color = [c[0], c[1], c[2]].map(|x| f32::from(x) / 255.0);
        }
        if let Some(v) = self.field("WaterTransparency", "", "StandingWaterTexture").and_then(|v| v.into_iter().next())
        {
            s.texture = v;
        }
        if let Some(v) = self.field("WaterTransparency", "", "AdditiveBlending").and_then(|v| v.into_iter().next()) {
            s.additive = matches!(v.to_ascii_lowercase().as_str(), "yes" | "true" | "1");
        }
        s
    }
}
