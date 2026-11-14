//! Inspect the simulation's bridge graph without creating GPU resources.
use anyhow::{Result, bail};
use ih_ini::definitions::Definitions;
use ih_sim::{
    navigation::CELL,
    templates::Templates,
    terrain::{Terrain, Waypoint},
};
use ih_vfs::Vfs;
pub fn run(vfs: &Vfs, language: &str, name: &str, radius: f32, query: Option<([f32; 2], [f32; 2])>) -> Result<()> {
    if !radius.is_finite() || radius < 0.0 {
        bail!("radius must be finite and non-negative");
    }
    let world = ih_client::World::load(vfs, name)?;
    let mut defs = Definitions::load(vfs, language);
    defs.load_map_overrides(vfs, &world.path)?;
    let templates = Templates::for_map(&defs, &world.map, vfs, language)?;
    let terrain = Terrain::from_map(&world.map, &templates);
    let zones = terrain.zone_summary(radius);
    println!(
        "{}: {} bridge layers, radius {radius}; {} zones, {} connections, {} components",
        world.path,
        terrain.layers.len(),
        zones.zones,
        zones.connections,
        zones.components
    );
    for (i, layer) in terrain.layers.iter().enumerate() {
        let b = &layer.bridge;
        let entries = layer.cells.values().filter(|c| c.entry).count();
        let usable = layer
            .cells
            .iter()
            .filter(|((x, y), c)| {
                c.entry && terrain.passable_on(i as u32 + 1, *x, *y, radius) && terrain.passable_on(0, *x, *y, radius)
            })
            .count();
        let bank = |end: bool| {
            for distance in [20.0, 40.0, 60.0, 80.0, 100.0] {
                for offset in [0.0, -10.0, 10.0, -20.0, 20.0] {
                    let p = b
                        .world(if end { b.length() + distance } else { -distance }, (b.min_y + b.max_y) * 0.5 + offset);
                    if terrain.ground.passable((p[0] / CELL).floor() as i32, (p[1] / CELL).floor() as i32, radius) {
                        return Some(p);
                    }
                }
            }
            None
        };
        let banks = bank(false).zip(bank(true));
        let report = banks.map(|(a, b)| terrain.path_report(Waypoint::ground(a), Waypoint::ground(b), radius));
        let path = report.as_ref().and_then(|r| r.path.as_ref());
        println!(
            "  layer {} {}: width {:.2}, {} deck cells, {usable}/{entries} usable entries; banks {banks:?}; route {}",
            i + 1,
            b.name,
            b.max_y - b.min_y,
            layer.cells.len(),
            path.map_or("unreachable".into(), |p| format!(
                "{} waypoints, layers {:?}; search {:?}",
                p.len(),
                p.iter().map(|p| p.layer).collect::<Vec<_>>(),
                report.as_ref().unwrap().stats
            ))
        );
    }
    if let Some((from, to)) = query {
        let report =
            terrain.path_report(terrain.destination(&world.map, from), terrain.destination(&world.map, to), radius);
        println!(
            "query {from:?} -> {to:?}: {}; {:?}",
            report.path.as_ref().map_or("unreachable".into(), |p| format!("{} waypoints", p.len())),
            report.stats
        );
    }
    Ok(())
}
