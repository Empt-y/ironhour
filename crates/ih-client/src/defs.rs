//! Every INI definition the game loads at startup, indexed by kind and name.
//!
//! Files are read in the original's load order (`ih_ini::load_order`). Later
//! definitions of the same name come after earlier ones; callers decide how they
//! combine (most kinds replace, `Object` re-parses over the existing template).

use std::collections::HashMap;

use ih_vfs::Vfs;

#[derive(Clone, Default)]
pub struct Definitions {
    pub data: ih_ini::definitions::Definitions,
}

impl std::ops::Deref for Definitions {
    type Target = ih_ini::definitions::Definitions;
    fn deref(&self) -> &Self::Target {
        &self.data
    }
}
impl std::ops::DerefMut for Definitions {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

impl Definitions {
    pub fn load(vfs: &Vfs, language: &str) -> Self {
        Self { data: ih_ini::definitions::Definitions::load(vfs, language) }
    }

    /// Road types in the linked-list order of `TerrainRoadCollection::newRoad`
    /// (`TerrainRoads.cpp`), newest first. New roads inherit `DefaultRoad` at
    /// creation time; later definitions reparse fields over the existing entry.
    pub fn road_types(&self) -> Vec<ih_render::roads::RoadType> {
        use ih_render::roads::RoadType;
        let mut roads: Vec<RoadType> = Vec::new();
        for block in self.data.ordered("Road") {
            let name = block.args().into_iter().next().unwrap_or_default();
            let at = if let Some(at) = roads.iter().position(|r| r.name == name) {
                at
            } else {
                let mut road = roads.iter().find(|r| r.name == "DefaultRoad").cloned().unwrap_or(RoadType {
                    name: String::new(),
                    texture: String::new(),
                    width: 0.0,
                    width_in_texture: 0.0,
                });
                road.name = name.clone();
                roads.push(road);
                roads.len() - 1
            };
            let road = &mut roads[at];
            for f in block.fields() {
                let values = f.values();
                let Some(value) = values.first() else { continue };
                match f.name.as_str() {
                    "Texture" => road.texture = value.clone(),
                    "RoadWidth" => {
                        if let Ok(v) = ih_ini::values::scan_real(value.as_bytes()) {
                            road.width = v;
                        }
                    }
                    "RoadWidthInTexture" => {
                        if let Ok(v) = ih_ini::values::scan_real(value.as_bytes()) {
                            road.width_in_texture = v;
                        }
                    }
                    _ => {}
                }
            }
        }
        roads.reverse();
        roads
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roads_inherit_defaults_and_keep_reverse_creation_order() {
        let mut defs = Definitions::default();
        defs.add_document(ih_ini::parse(b"Road DefaultRoad\nTexture = default.tga\nRoadWidth = 10\nRoadWidthInTexture = 0.9\nEnd\nRoad First\nTexture = first.tga\nEnd\nRoad Second\nRoadWidth = 25\nEnd\n"));
        defs.add_document(ih_ini::parse(b"Road First\nRoadWidth = 35.5suffix\nEnd\n"));
        let roads = defs.road_types();
        assert_eq!(roads.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Second", "First", "DefaultRoad"]);
        assert_eq!(roads[0].texture, "default.tga");
        assert_eq!(roads[0].width, 25.0);
        assert_eq!(roads[1].texture, "first.tga");
        assert_eq!(roads[1].width, 35.5);
        assert_eq!(roads[1].width_in_texture, 0.9);
    }

    #[test]
    fn changing_defaults_only_affects_roads_created_afterwards() {
        let mut defs = Definitions::default();
        defs.add_document(ih_ini::parse(b"Road DefaultRoad\nRoadWidth = 10\nEnd\nRoad First\nEnd\nRoad DefaultRoad\nRoadWidth = 20\nEnd\nRoad Second\nEnd\n"));
        let roads = defs.road_types();
        assert_eq!(roads[0].width, 20.0);
        assert_eq!(roads[1].width, 10.0);
        assert_eq!(roads[2].width, 20.0);
    }
}
