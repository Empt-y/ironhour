//! Cell classification (`Pathfinder::classifyMap`) and locomotor surfaces
//! (`validLocomotorSurfacesForCellType`): water, cliffs, and who may go where.
use ih_formats::map::{Map, PolygonTrigger};
use ih_ini::definitions::Definitions;
use ih_sim::{
    Command, Simulation,
    footprint::CellType,
    navigation::{classify_map, point_in_trigger},
    templates::Templates,
};
use std::sync::Arc;

const INI: &[u8] = b"Object Tank\nKindOf = VEHICLE\nLocomotor = SET_NORMAL Tracks\nGeometryMajorRadius = 3\nBehavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\n\
Object Boat\nKindOf = VEHICLE\nLocomotor = SET_NORMAL Hull\nGeometryMajorRadius = 3\nBehavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\n\
Locomotor Tracks\nSurfaces = GROUND\nSpeed = 30\nAcceleration = 900\nBraking = 900\nTurnRate = 360\nEnd\n\
Locomotor Hull\nSurfaces = WATER\nSpeed = 30\nAcceleration = 900\nBraking = 900\nTurnRate = 360\nEnd\n";

/// 40 x 40 cells: a lake over cells x 0..=10 (z 5 over flat ground) and a
/// plateau 62.5 high on heightmap samples 22..=26 (cells 20..=24).
fn map() -> Map {
    let (w, border) = (44, 2);
    let mut heights = vec![0u8; (w * w) as usize];
    for y in 22..=26 {
        for x in 22..=26 {
            heights[(y * w + x) as usize] = 100;
        }
    }
    Map {
        width: w,
        height: w,
        border,
        boundaries: vec![[40, 40]],
        heights,
        triggers: vec![PolygonTrigger {
            name: "Lake".into(),
            layer: String::new(),
            id: 1,
            is_water: true,
            is_river: false,
            river_start: 0,
            points: vec![[0, 0, 5], [100, 0, 5], [100, 400, 5], [0, 400, 5]],
        }],
        ..Default::default()
    }
}

#[test]
fn point_in_trigger_matches_the_original_integer_test() {
    let square = [[0, 0, 0], [100, 0, 0], [100, 100, 0], [0, 100, 0]];
    assert!(point_in_trigger(&square, [50, 50]));
    // The right edge counts (intersection x >= point x); the bottom edge counts
    // (pt.y < point.y on one end only); the top edge does not.
    assert!(point_in_trigger(&square, [100, 50]));
    assert!(point_in_trigger(&square, [50, 100]));
    assert!(!point_in_trigger(&square, [50, 0]));
    assert!(!point_in_trigger(&square, [101, 50]));
}

#[test]
fn classify_map_finds_water_corners_and_grows_cliffs() {
    let c = classify_map(&map());
    let at = |x: i32, y: i32| c.kind[(y * c.width + x) as usize];
    // Any underwater corner makes a water cell: cell 10's left corners lie on x 100.
    assert_eq!(at(0, 5), CellType::Water);
    assert_eq!(at(10, 5), CellType::Water);
    assert_eq!(at(11, 5), CellType::Clear);
    // The plateau's slopes are cliffs (heightmap cells 19..=24 have a 62.5 drop),
    // grown one cell into clear ground; its flat top stays clear.
    assert_eq!(at(19, 22), CellType::Cliff);
    assert_eq!(at(18, 22), CellType::Cliff, "grown by one cell");
    assert_eq!(at(17, 22), CellType::Clear);
    assert!(c.pinched[(22 * c.width + 17) as usize], "pinched border");
    assert_eq!(at(22, 22), CellType::Clear, "flat top");
    // Surfaces: water for boats, cliffs for climbers, ground for both of neither.
    use ih_sim::footprint::surface::*;
    assert!(CellType::Water.allows(WATER) && !CellType::Water.allows(GROUND));
    assert!(CellType::Cliff.allows(CLIFF) && !CellType::Cliff.allows(GROUND));
    assert!(CellType::Obstacle.allows(AIR) && !CellType::Obstacle.allows(GROUND | WATER | CLIFF));
}

#[test]
fn boats_stay_on_water_and_tanks_on_land() {
    let mut defs = Definitions::default();
    defs.add_document(ih_ini::parse(INI));
    let mut sim = Simulation::new(Arc::new(map()), Arc::new(Templates::from_definitions(&defs)), 1);
    let boat = sim.spawn("Boat", [40.0, 50.0], 0).unwrap();
    let tank = sim.spawn("Tank", [200.0, 50.0], 0).unwrap();
    assert_eq!(sim.views.len(), 1, "one water view");
    // Each can move within its own surface...
    sim.enqueue(1, Command::Move { object: boat, target: [40.0, 350.0] }).unwrap();
    sim.enqueue(1, Command::Move { object: tank, target: [300.0, 50.0] }).unwrap();
    // ...but not onto the other's.
    sim.enqueue(2, Command::Move { object: boat, target: [300.0, 300.0] }).unwrap();
    sim.enqueue(3, Command::Move { object: tank, target: [40.0, 200.0] }).unwrap();
    sim.step();
    sim.step();
    assert_eq!(sim.events.len(), 0, "{:?}", sim.events);
    for _ in 0..4 {
        sim.step();
    }
    assert_eq!(sim.events.len(), 2, "both cross-surface orders are rejected: {:?}", sim.events);
    sim.enqueue(sim.frame, Command::Move { object: boat, target: [40.0, 350.0] }).unwrap();
    for _ in 0..600 {
        sim.step();
    }
    let p = sim.objects[&boat].position;
    // The goal snaps to the centre of its cell (an odd, one-cell footprint).
    assert!(ih_math::length([p[0] - 45.0, p[1] - 355.0]) < 0.5, "boat at {p:?}");
    let p = sim.objects[&tank].position;
    assert!(p[0] > 110.0, "tank stayed out of the lake: {p:?}");
}
