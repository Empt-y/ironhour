//! Crushers and fences (`Pathfinder::validMovementPosition`: crushers may enter
//! fence obstacle cells; `m_crusherZones`): a tank drives through a fence line
//! that a jeep has to go round.
use ih_formats::map::{Dict, DictValue, Map, MapObject};
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation, templates::Templates};
use std::sync::Arc;

const INI: &[u8] = b"Object Tank\nKindOf = VEHICLE\nCrusherLevel = 2\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 4\nBehavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\n\
Object Jeep\nKindOf = VEHICLE\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 4\nBehavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\n\
Object Fence\nKindOf = IMMOBILE CLEARED_BY_BUILD\nCrushableLevel = 1\nFenceWidth = 40\nFenceXOffset = 20\nEnd\n\
Locomotor L\nSurfaces = GROUND\nSpeed = 30\nAcceleration = 900\nBraking = 900\nTurnRate = 360\nEnd\n";

/// A fence wall at x 200 from y 0 to 320 (eight 40-wide pieces facing north).
fn sim() -> Simulation {
    let mut defs = Definitions::default();
    defs.add_document(ih_ini::parse(INI));
    let fence = |y: f32| MapObject {
        position: [200.0, y, 0.0],
        angle: std::f32::consts::FRAC_PI_2,
        flags: 0,
        name: "Fence".into(),
        properties: Dict(vec![("originalOwner".into(), DictValue::Ascii("team".into()))]),
    };
    let map = Map {
        width: 44,
        height: 44,
        border: 2,
        boundaries: vec![[40, 40]],
        heights: vec![0; 44 * 44],
        objects: (0..8).map(|i| fence(20.0 + 40.0 * i as f32)).collect(),
        ..Default::default()
    };
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&defs)), 1)
}

#[test]
fn a_crusher_drives_through_a_fence_that_others_go_round() {
    let mut s = sim();
    assert!(s.grid.obstacles.fence.iter().filter(|f| **f).count() >= 30, "fence cells");
    let tank = s.spawn("Tank", [100.0, 100.0], 1).unwrap();
    let jeep = s.spawn("Jeep", [100.0, 200.0], 1).unwrap();
    s.load_map_objects().unwrap();
    assert_eq!(s.views.len(), 1, "a crusher view");
    s.enqueue(1, Command::Move { object: tank, target: [300.0, 100.0] }).unwrap();
    s.enqueue(1, Command::Move { object: jeep, target: [300.0, 200.0] }).unwrap();
    let mut jeep_top = 0.0f32;
    let mut tank_top = 0.0f32;
    for _ in 0..900 {
        s.step();
        jeep_top = jeep_top.max(s.objects[&jeep].position[1]);
        tank_top = tank_top.max(s.objects[&tank].position[1]);
    }
    assert!(s.objects[&tank].is_idle() && s.objects[&jeep].is_idle(), "{:?}", s.events);
    assert!(s.objects[&tank].position[0] > 290.0 && s.objects[&jeep].position[0] > 290.0);
    // The tank keeps to its row; the jeep has to go round the end of the fence.
    assert!(tank_top < 130.0, "tank went round: {tank_top}");
    assert!(jeep_top > 320.0, "jeep went through: {jeep_top}");
}
