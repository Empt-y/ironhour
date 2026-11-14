//! Headless M3 geometry verification on mounted map data.
use anyhow::{Result, bail};
use ih_client::{Definitions, World, find_map};
use ih_render::{Assets, roads, water};
use ih_vfs::Vfs;

pub fn run(vfs: &Vfs, language: &str, name: Option<&str>) -> Result<()> {
    let base = Definitions::load(vfs, language);
    let mut assets = Assets::new(vfs, language);
    let files = if let Some(name) = name {
        vec![find_map(vfs, name).ok_or_else(|| anyhow::anyhow!("map {name} not found"))?]
    } else {
        vfs.list("Maps", "*.map", true)
    };
    let (mut failed, mut road_triangles, mut water_triangles, mut bridges, mut rivers) = (0, 0, 0, 0, 0);
    for path in &files {
        let mut world = match World::load(vfs, path) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("{path}: {e}");
                failed += 1;
                continue;
            }
        };
        let mut defs = base.clone();
        defs.load_map_overrides(vfs, path)?;
        let lights = world.terrain_lights();
        let road = roads::road_meshes(&world.map, &defs.road_types(), &lights, 3);
        let water = water::water_mesh(&world.map);
        let bridge = world.bridge_meshes(&mut assets, &defs);
        let nr = world.map.triggers.iter().filter(|t| t.is_water && t.is_river).count();
        rivers += nr;
        bridges += bridge.len();
        if nr > 0 || !bridge.is_empty() {
            println!("{path}: {} bridges, {nr} rivers", bridge.len());
        }
        road_triangles += road.iter().map(|m| m.indices.len() / 3).sum::<usize>();
        water_triangles += water.indices.len() / 3;
        for m in road.iter().chain(&bridge) {
            if m.vertices.iter().any(|v| !v.position.iter().chain(&v.uv0).chain(&v.color).all(|f| f.is_finite()))
                || m.indices.iter().any(|i| *i as usize >= m.vertices.len())
            {
                world.warnings.push(format!("{}: invalid geometry", m.texture));
            }
            assets.texture(&m.texture);
        }
        if water.vertices.iter().any(|v| !v.position.iter().chain(&v.river_uv).all(|f| f.is_finite()))
            || water.indices.iter().any(|i| *i as usize >= water.vertices.len())
        {
            world.warnings.push("invalid water geometry".into());
        }
        if !world.warnings.is_empty() {
            failed += 1;
            for w in world.warnings {
                eprintln!("{path}: {w}");
            }
        }
    }
    for w in &assets.warnings {
        eprintln!("asset: {w}");
    }
    println!(
        "{} maps: {failed} failed, {} asset warnings; {road_triangles} road triangles, {water_triangles} water triangles, {bridges} bridges, {rivers} rivers",
        files.len(),
        assets.warnings.len()
    );
    if failed > 0 || !assets.warnings.is_empty() {
        bail!("map geometry verification failed");
    }
    Ok(())
}
