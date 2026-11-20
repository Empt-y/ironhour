//! Group moves (`AIGroup::groupMoveToPosition`): shared ground paths, lanes and
//! columns, kept shape on short moves, and gathering on a click inside the group.
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation, templates::Templates};
use std::sync::Arc;

const INI: &[u8] = b"Object Tank\nKindOf = VEHICLE\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 4\nBehavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\n\
Object Soldier\nKindOf = INFANTRY\nLocomotor = SET_NORMAL Feet\nGeometryMajorRadius = 3\nBehavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\n\
Locomotor L\nSurfaces = GROUND\nSpeed = 30\nAcceleration = 900\nBraking = 900\nTurnRate = 360\nEnd\n\
Locomotor Feet\nSurfaces = GROUND\nSpeed = 20\nAcceleration = 900\nBraking = 900\nTurnRate = 720\nEnd\n\
AIData\nMinInfantryForGroup = 3\nMinVehiclesForGroup = 3\nMinDistanceForGroup = 100.0\nDistanceRequiresGroup = 500.0\nEnd\n\
GameData\nGroupMoveClickToGatherAreaFactor = 0.5\nEnd\n";

fn sim() -> Simulation {
    let mut defs = Definitions::default();
    defs.add_document(ih_ini::parse(INI));
    let (w, b) = (104, 2);
    let map = Map {
        width: w,
        height: w,
        border: b,
        boundaries: vec![[100, 100]],
        heights: vec![0; (w * w) as usize],
        ..Default::default()
    };
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&defs)), 1)
}

fn run(s: &mut Simulation, frames: u32) {
    for _ in 0..frames {
        s.step();
        assert!(s.overlaps().is_empty(), "overlap at frame {}", s.frame);
    }
}

#[test]
fn group_settings_come_from_aidata_and_gamedata() {
    let s = sim();
    let g = s.templates.group;
    assert_eq!((g.min_infantry, g.min_vehicles, g.min_distance, g.distance_requires_group), (3, 3, 100.0, 500.0));
    assert_eq!(g.gather_factor, 0.5);
}

#[test]
fn vehicles_on_open_ground_move_individually() {
    // A straight shared path has no end node far enough from its start, which the
    // vehicle layout rejects (`endNode == getFirstNode()`): individual moves.
    let mut s = sim();
    let ids: Vec<u32> = (0..6)
        .map(|i| s.spawn("Tank", [100.0 + 20.0 * (i % 3) as f32, 100.0 + 20.0 * (i / 3) as f32], 0).unwrap())
        .collect();
    s.enqueue(1, Command::GroupMove { objects: ids.clone(), target: [800.0, 700.0] }).unwrap();
    s.step();
    s.step();
    assert!(ids.iter().all(|id| s.objects[id].ai.state == ih_sim::ai::AiState::MoveTo));
    run(&mut s, 1500);
    assert!(ids.iter().all(|id| s.objects[id].is_idle()));
}

#[test]
fn vehicles_routed_around_a_wall_share_the_path_in_lanes() {
    let mut s = sim();
    // A wall at x 400 from y 0 to 600: the shared path bends around its end.
    let w = s.grid.width;
    for y in 0..60 {
        for x in 40..42 {
            let i = (y * w + x) as usize;
            s.grid.ground.blocked[i] = true;
            s.grid.obstacles.kind[i] = ih_sim::footprint::CellType::Obstacle;
        }
    }
    s.grid.invalidate_zones();
    let ids: Vec<u32> = (0..6)
        .map(|i| s.spawn("Tank", [100.0 + 20.0 * (i % 3) as f32, 100.0 + 20.0 * (i / 3) as f32], 0).unwrap())
        .collect();
    s.enqueue(1, Command::GroupMove { objects: ids.clone(), target: [700.0, 200.0] }).unwrap();
    s.step();
    s.step();
    // Every vehicle follows a path (the shared route), not a straight move.
    for id in &ids {
        assert_eq!(s.objects[id].ai.state, ih_sim::ai::AiState::FollowPath, "unit {id}");
    }
    run(&mut s, 1500);
    assert!(s.events.is_empty(), "{:?}", s.events);
    let p: Vec<[f32; 3]> = ids.iter().map(|id| s.objects[id].position).collect();
    for (i, a) in p.iter().enumerate() {
        assert!(s.objects[&ids[i]].is_idle(), "unit {} still moving", ids[i]);
        assert!(ih_math::length([a[0] - 700.0, a[1] - 200.0]) < 90.0, "unit {i} far from the goal: {a:?}");
        for b in &p[i + 1..] {
            assert!(ih_math::length([a[0] - b[0], a[1] - b[1]]) >= 8.0, "units on top of each other: {p:?}");
        }
    }
}

#[test]
fn infantry_columns_end_in_separate_spots() {
    let mut s = sim();
    let ids: Vec<u32> = (0..8)
        .map(|i| s.spawn("Soldier", [100.0 + 12.0 * (i % 4) as f32, 100.0 + 12.0 * (i / 4) as f32], 0).unwrap())
        .collect();
    s.enqueue(1, Command::GroupMove { objects: ids.clone(), target: [700.0, 150.0] }).unwrap();
    s.step();
    s.step();
    assert!(ids.iter().all(|id| s.objects[id].ai.state == ih_sim::ai::AiState::FollowPath));
    run(&mut s, 1500);
    let goals: std::collections::BTreeSet<_> = ids.iter().map(|id| s.objects[id].goal_cell).collect();
    assert_eq!(goals.len(), ids.len(), "every soldier holds its own goal cell");
    assert!(ids.iter().all(|id| s.objects[id].is_idle()));
}

#[test]
fn a_short_move_keeps_the_group_shape() {
    let mut s = sim();
    let a = s.spawn("Tank", [300.0, 300.0], 0).unwrap();
    let b = s.spawn("Tank", [340.0, 300.0], 0).unwrap();
    // 80 units away: below MinDistanceForGroup, so each moves by its own offset.
    s.enqueue(1, Command::GroupMove { objects: vec![a, b], target: [320.0, 380.0] }).unwrap();
    s.step();
    s.step();
    assert!([a, b].iter().all(|id| s.objects[id].ai.state == ih_sim::ai::AiState::MoveTo));
    run(&mut s, 600);
    let (pa, pb) = (s.objects[&a].position, s.objects[&b].position);
    // Side by side as before, about 40 apart, around the goal.
    assert!((pb[0] - pa[0]) > 25.0 && (pb[0] - pa[0]) < 55.0, "{pa:?} {pb:?}");
    assert!((pa[1] - 380.0).abs() < 12.0 && (pb[1] - 380.0).abs() < 12.0, "{pa:?} {pb:?}");
}

#[test]
fn clicking_inside_the_group_gathers_it() {
    let mut s = sim();
    let ids: Vec<u32> = (0..4)
        .map(|i| s.spawn("Tank", [200.0 + 100.0 * (i % 2) as f32, 200.0 + 100.0 * (i / 2) as f32], 0).unwrap())
        .collect();
    // The centre of the group's box, inside its middle half: tighten.
    s.enqueue(1, Command::GroupMove { objects: ids.clone(), target: [250.0, 250.0] }).unwrap();
    run(&mut s, 900);
    for id in &ids {
        let p = s.objects[id].position;
        assert!(ih_math::length([p[0] - 250.0, p[1] - 250.0]) < 30.0, "unit {id} not gathered: {p:?}");
    }
}
