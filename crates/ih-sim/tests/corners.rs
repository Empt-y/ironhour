//! Regression cases for long-route stalls at static cell corners.
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{Simulation, templates::Templates, terrain::Waypoint};
use std::{collections::VecDeque, sync::Arc};

fn simulation() -> Simulation {
    let mut defs = Definitions::default();
    defs.add_document(ih_ini::parse(b"Object Tank\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 1\nBehavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\nLocomotor L\nSurfaces = GROUND\nSpeed = 30\nAcceleration = 900\nBraking = 900\nTurnRate = 360\nEnd\n"));
    Simulation::new(
        Arc::new(Map {
            width: 24,
            height: 24,
            border: 2,
            boundaries: vec![[20, 20]],
            heights: vec![0; 576],
            ..Default::default()
        }),
        Arc::new(Templates::from_definitions(&defs)),
        1,
    )
}

#[test]
fn intermediate_corner_is_reached_before_turning_into_the_next_leg() {
    let mut s = simulation();
    s.grid.ground.blocked[6 * 20 + 6] = true;
    s.grid.invalidate_zones();
    let id = s.spawn("Tank", [59.8, 60.2], 0).unwrap();
    let o = s.objects.get_mut(&id).unwrap();
    o.angle = -std::f32::consts::FRAC_PI_2;
    o.target = Some([80.0, 59.8]);
    o.ai.begin(ih_sim::ai::AiState::MoveTo, 0);
    // Paths start with the current segment start node.
    o.path = VecDeque::from([
        Waypoint::ground([59.8, 60.2]),
        Waypoint::ground([59.8, 59.8]),
        Waypoint::ground([80.0, 59.8]),
    ]);
    for _ in 0..180 {
        s.step();
    }
    let o = &s.objects[&id];
    assert!(o.target.is_none(), "{o:?}");
    assert!((o.position[0] - 80.0).abs() < 0.5);
    assert_eq!(o.traffic_repaths, 0);
}

#[test]
fn terrain_blocked_order_repaths_and_finishes_instead_of_waiting_forever() {
    let mut s = simulation();
    s.grid.ground.blocked[6 * 20 + 8] = true;
    s.grid.invalidate_zones();
    let id = s.spawn("Tank", [25.0, 65.0], 0).unwrap();
    // A once-valid corridor may become obstructed by a later static edit.
    let o = s.objects.get_mut(&id).unwrap();
    o.target = Some([125.0, 65.0]);
    o.ai.begin(ih_sim::ai::AiState::MoveTo, 0);
    o.path = VecDeque::from([Waypoint::ground([25.0, 65.0]), Waypoint::ground([125.0, 65.0])]);
    for _ in 0..600 {
        s.step();
    }
    let o = &s.objects[&id];
    assert!(o.target.is_none(), "{o:?}");
    assert!(o.traffic_repaths > 0);
    assert!((o.position[0] - 125.0).abs() < 0.5);
    assert!(s.events.is_empty());
}
