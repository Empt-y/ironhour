//! A loaded map and the scene that draws it.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Mat4, Vec3};
use ih_formats::map::{MAP_XY_FACTOR, Map, MapError};
use ih_formats::w3d::Hierarchy;
use ih_render::terrain::{Terrain, TerrainLights, TerrainOptions};
use ih_render::{Assets, BuildOptions, Lights, PreparedModel, Renderer, Scene};
use ih_vfs::Vfs;

use crate::defs::Definitions;
use crate::drawable::{TemplateArt, template_art};

#[derive(Debug, thiserror::Error)]
pub enum WorldError {
    #[error("{0}: no such map")]
    NotFound(String),
    #[error("{0}: {1}")]
    Map(String, MapError),
    #[error("{0}: no terrain tiles")]
    NoTiles(String),
    #[error(transparent)]
    Vfs(#[from] ih_vfs::VfsError),
}

/// Finds a map by path or by name (`Maps\<name>\<name>.map`), or by a unique part
/// of its path.
pub fn find_map(vfs: &Vfs, name: &str) -> Option<String> {
    if vfs.exists(name) {
        return Some(name.to_string());
    }
    let path = format!("Maps\\{name}\\{name}.map");
    if vfs.exists(&path) {
        return Some(path);
    }
    let lower = name.to_ascii_lowercase();
    vfs.list("Maps", "*.map", true).into_iter().find(|p| p.to_ascii_lowercase().contains(&lower))
}

/// A built model, its hierarchy and idle animation.
type PreparedArt = (Arc<PreparedModel>, Option<Hierarchy>, Option<String>);

pub struct World {
    pub path: String,
    pub map: Map,
    pub warnings: Vec<String>,
}

impl World {
    /// Paired bridge points, as `W3DBridgeBuffer::loadBridges`. Stored object z is
    /// ignored: endpoints sit at terrain height plus `BRIDGE_FLOAT_AMT` (0.25).
    pub fn add_bridges(&mut self, renderer: &mut Renderer, assets: &mut Assets, defs: &Definitions, scene: &mut Scene) {
        let meshes = self.bridge_meshes(assets, defs);
        let night = self.map.lighting.as_ref().is_some_and(|l| l.time_of_day == 4);
        scene.bridges = renderer.prepare_roads(assets, &meshes, (!night).then_some("TSCloudMed.tga"), None);
    }

    /// CPU-only bridge construction, also used by the map verification tool.
    pub fn bridge_meshes(&mut self, assets: &mut Assets, defs: &Definitions) -> Vec<ih_render::roads::RoadMesh> {
        use ih_formats::map::object_flags as of;
        let mut meshes = Vec::new();
        let lights = self.terrain_lights();
        for pair in self.map.objects.windows(2) {
            if pair[0].flags & of::BRIDGE_POINT1 == 0 || pair[1].flags & of::BRIDGE_POINT2 == 0 {
                continue;
            }
            let name = &pair[0].name;
            let field = |f: &str| {
                defs.field("Bridge", name, f)
                    .or_else(|| defs.field("Bridge", "DefaultBridge", f))
                    .and_then(|v| v.into_iter().next())
            };
            let Some(model) = field("BridgeModelName") else {
                self.warnings.push(format!("bridge type {name} has no model"));
                continue;
            };
            let def = ih_render::bridges::BridgeType {
                model,
                texture: field("Texture").unwrap_or_default(),
                scale: field("BridgeScale").and_then(|v| ih_ini::values::scan_real(v.as_bytes()).ok()).unwrap_or(1.0),
            };
            let endpoint = |p: [f32; 3]| Vec3::new(p[0], p[1], self.ground_height(p[0], p[1]) + 0.25);
            match ih_render::bridges::build(
                assets,
                &def,
                endpoint(pair[0].position),
                endpoint(pair[1].position),
                &lights,
                3,
            ) {
                Ok(mesh) => meshes.push(mesh),
                Err(e) => self.warnings.push(format!("bridge {name}: {e}")),
            }
        }
        meshes
    }

    pub fn load(vfs: &Vfs, name: &str) -> Result<World, WorldError> {
        let path = find_map(vfs, name).ok_or_else(|| WorldError::NotFound(name.into()))?;
        let bytes = vfs.read(&path)?.ok_or_else(|| WorldError::NotFound(path.clone()))?;
        let map = Map::parse(&bytes).map_err(|e| WorldError::Map(path.clone(), e))?;
        let warnings = map.warnings.clone();
        Ok(World { path, map, warnings })
    }

    /// The playable area in world units: (min x, min y), (max x, max y).
    pub fn playable_bounds(&self) -> ([f32; 2], [f32; 2]) {
        let b = self
            .map
            .boundaries
            .first()
            .copied()
            .unwrap_or([self.map.width - 2 * self.map.border, self.map.height - 2 * self.map.border]);
        ([0.0, 0.0], [b[0] as f32 * MAP_XY_FACTOR, b[1] as f32 * MAP_XY_FACTOR])
    }

    /// Terrain height at world (x, y), using the original's triangle interpolation.
    pub fn ground_height(&self, x: f32, y: f32) -> f32 {
        self.map.ground_height(x, y)
    }

    /// The map's object lights, or GameData's afternoon lights.
    pub fn object_lights(&self) -> Lights {
        Lights::from_map(&self.map).unwrap_or_else(Lights::afternoon)
    }

    /// Builds the terrain into `scene`.
    pub fn add_terrain(
        &mut self,
        renderer: &mut Renderer,
        assets: &mut Assets,
        vfs: &Vfs,
        defs: &Definitions,
        scene: &mut Scene,
    ) -> Result<(), WorldError> {
        let textures = defs.terrain_textures();
        let terrain = Terrain::new(&self.map, TerrainOptions::default(), &textures, |p| vfs.read(p).ok().flatten())
            .ok_or_else(|| WorldError::NoTiles(self.path.clone()))?;
        self.warnings.extend(terrain.warnings.iter().cloned());
        let lights = self.terrain_lights();
        let mesh = terrain.mesh(&lights);
        let atlas = terrain.atlas();
        // UseCloudMap and UseLightMap are on in stock GameData; clouds are off at night.
        let night = self.map.lighting.as_ref().is_some_and(|l| l.time_of_day == 4);
        let cloud = (!night).then_some("TSCloudMed.tga");
        scene.terrain = Some(renderer.prepare_terrain(assets, &atlas, &mesh, cloud, Some("TSNoiseUrb.tga")));
        Ok(())
    }
}

impl World {
    /// Idle condition flags for this map: `NIGHT` on night maps, `SNOW` when the
    /// world's `weather` is snowy (`Weather` 1).
    pub fn condition_flags(&self) -> Vec<&'static str> {
        let mut f = Vec::new();
        if self.map.lighting.as_ref().is_some_and(|l| l.time_of_day == 4) {
            f.push("NIGHT");
        }
        if self.map.world.int("weather") == Some(1) {
            f.push("SNOW");
        }
        f
    }

    /// Places the map's objects into `scene`; returns how many were drawn.
    ///
    /// As `GameLogic::startNewGame` does: roads and bridge points are skipped (the
    /// terrain draws them), z is the stored height above the ground, the angle is
    /// the stored facing.
    pub fn add_objects(
        &mut self,
        renderer: &mut Renderer,
        assets: &mut Assets,
        defs: &Definitions,
        scene: &mut Scene,
    ) -> usize {
        use ih_formats::map::object_flags as of;
        let wanted = self.condition_flags();
        let mut prepared: HashMap<(String, Vec<String>), Option<PreparedArt>> = HashMap::new();
        let mut arts: HashMap<String, Option<TemplateArt>> = HashMap::new();
        let mut missing: std::collections::BTreeSet<String> = Default::default();
        let mut placed = 0;
        let objects = self.map.objects.clone();
        for o in &objects {
            if o.flags & (of::ROAD_POINT1 | of::ROAD_POINT2 | of::BRIDGE_POINT1 | of::BRIDGE_POINT2) != 0 {
                continue;
            }
            let art = arts.entry(o.name.clone()).or_insert_with(|| template_art(defs, &o.name, &wanted));
            let Some(art) = art.clone() else {
                if !o.name.starts_with('*') {
                    missing.insert(o.name.clone());
                }
                continue;
            };
            let z = o.position[2] + self.ground_height(o.position[0], o.position[1]);
            let root = Mat4::from_translation(Vec3::new(o.position[0], o.position[1], z))
                * Mat4::from_rotation_z(o.angle)
                * Mat4::from_scale(Vec3::splat(art.scale));
            for dm in &art.models {
                let key = (dm.model.to_ascii_lowercase(), dm.hide.clone());
                let entry = prepared.entry(key).or_insert_with(|| {
                    let opts = BuildOptions { house_color: None, hide: dm.hide.clone() };
                    match ih_render::build(assets, &dm.model, &opts) {
                        Ok(model) => {
                            let p = renderer.prepare_model(assets, &model);
                            Some((p, model.hierarchy.clone(), dm.animation.clone()))
                        }
                        Err(e) => {
                            self.warnings.push(format!("{}: {e}", dm.model));
                            None
                        }
                    }
                });
                let Some((model, hierarchy, anim)) = entry else { continue };
                let bones = match hierarchy {
                    Some(h) => {
                        let a = anim.as_deref().and_then(|a| assets.animation(a));
                        match a {
                            Some(a) => ih_render::pose::animated_pose(h, &a, 0.0, root).0,
                            None => ih_render::pose::base_pose(h, root),
                        }
                    }
                    None => vec![root],
                };
                scene.instances.push(renderer.instance(model, &bones));
            }
            placed += 1;
        }
        for m in missing {
            self.warnings.push(format!("object template {m} is not defined"));
        }
        placed
    }
}

impl World {
    /// The map's terrain lights, or GameData's afternoon terrain lights.
    pub fn terrain_lights(&self) -> TerrainLights {
        TerrainLights::from_map(&self.map).unwrap_or(TerrainLights {
            ambient: [56.0 / 255.0, 52.0 / 255.0, 44.0 / 255.0],
            diffuse: [
                [1.0; 3],
                [60.0 / 255.0, 60.0 / 255.0, 120.0 / 255.0],
                [30.0 / 255.0, 30.0 / 255.0, 20.0 / 255.0],
            ],
            position: [[-0.81, 0.38, -0.45], [0.79, 0.62, 0.0], [0.81, -0.48, -0.34]],
        })
    }

    /// Adds the map's standing water to `scene`.
    pub fn add_water(&self, renderer: &mut Renderer, assets: &mut Assets, defs: &Definitions, scene: &mut Scene) {
        let tod = self.map.lighting.as_ref().map_or(2, |l| l.time_of_day);
        let settings = defs.water_settings(tod);
        let mesh = ih_render::water::water_mesh(&self.map);
        let lights = self.terrain_lights();
        let pairs: Vec<([f32; 3], [f32; 3])> = (0..3).map(|i| (lights.position[i], lights.diffuse[i])).collect();
        let color = settings.vertex_color(lights.ambient, &pairs);
        let (w, h, data) = ih_render::water::height_image(&self.map);
        scene.water = renderer.prepare_water(assets, &mesh, &settings, color, (w, h, &data), self.map.border);
    }

    /// Adds the road ribbons (`W3DRoadBuffer::addMapObjects` / `drawRoads`).
    /// Includes junctions, curves, mitres and cross-type alpha joins.
    pub fn add_roads(&mut self, renderer: &mut Renderer, assets: &mut Assets, defs: &Definitions, scene: &mut Scene) {
        let types = defs.road_types();
        let lights = self.terrain_lights();
        let meshes =
            ih_render::roads::road_meshes(&self.map, &types, &lights, TerrainOptions::default().num_global_lights);
        let night = self.map.lighting.as_ref().is_some_and(|l| l.time_of_day == 4);
        scene.roads =
            renderer.prepare_roads(assets, &meshes, (!night).then_some("TSCloudMed.tga"), Some("TSNoiseUrb.tga"));
        let missing: std::collections::BTreeSet<_> = self
            .map
            .objects
            .iter()
            .filter(|o| o.flags & ih_formats::map::object_flags::ROAD_POINT1 != 0)
            .filter(|o| !types.iter().any(|t| t.name == o.name))
            .map(|o| o.name.as_str())
            .collect();
        for name in missing {
            self.warnings.push(format!("road type {name} is not defined; segment skipped"));
        }
    }
}
