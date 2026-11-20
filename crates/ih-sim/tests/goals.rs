//! Goal cells (`Pathfinder::updateGoal`, `adjustDestination`): units ordered to
//! the same point claim different cells and stop apart.
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation, templates::Templates};
use std::sync::Arc;

const INI: &[u8] = b"Object Tank\nKindOf = VEHICLE\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 8\nBehavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\n\
Locomotor L\nSurfaces = GROUND\nSpeed = 30\nAcceleration = 900\nBraking = 900\nTurnRate = 360\nEnd\n";

fn sim() -> Simulation {
    let mut defs = Definitions::default();
    defs.add_document(ih_ini::parse(INI));
    let map = Map {
        width: 44,
        height: 44,
        border: 2,
        boundaries: vec![[40, 40]],
        heights: vec![0; 44 * 44],
        ..Default::default()
    };
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&defs)), 1)
}

#[test]
fn units_sent_to_one_point_take_separate_goal_cells() {
    let mut s = sim();
    let ids: Vec<u32> = (0..3).map(|i| s.spawn("Tank", [50.0, 100.0 + 40.0 * i as f32], 0).unwrap()).collect();
    for &id in &ids {
        s.enqueue(1, Command::Move { object: id, target: [300.0, 140.0] }).unwrap();
    }
    s.step();
    s.step();
    let goals: Vec<_> = ids.iter().map(|id| s.objects[id].goal_cell.unwrap()).collect();
    // A 16-wide tank covers two cells each way: goals at least two cells apart.
    for (i, a) in goals.iter().enumerate() {
        for b in &goals[i + 1..] {
            assert!((a.0 - b.0).abs() >= 2 || (a.1 - b.1).abs() >= 2, "goals overlap: {goals:?}");
        }
    }
    // Every goal cell is owned by exactly one unit.
    for &id in &ids {
        let owned = s.unit_cells.cells.values().filter(|c| c.goal_unit == id).count();
        assert_eq!(owned, 4, "unit {id} owns {owned} goal cells");
    }
    for _ in 0..900 {
        s.step();
        assert!(s.overlaps().is_empty());
    }
    assert!(s.objects.values().all(|o| o.is_idle()), "{:?}", s.events);
    let p: Vec<[f32; 3]> = ids.iter().map(|id| s.objects[id].position).collect();
    for (i, a) in p.iter().enumerate() {
        assert!(ih_math::length([a[0] - 300.0, a[1] - 140.0]) < 40.0, "unit {i} ended far away at {a:?}");
        for b in &p[i + 1..] {
            assert!(ih_math::length([a[0] - b[0], a[1] - b[1]]) >= 16.0, "units stopped on top of each other: {p:?}");
        }
    }
    // Idle units' goals are where they stand.
    for &id in &ids {
        let o = &s.objects[&id];
        assert_eq!(o.goal_cell, o.cur_cell);
    }
    // Destroying a unit releases its cells.
    s.enqueue(s.frame, Command::Destroy { object: ids[0] }).unwrap();
    s.step();
    assert!(s.unit_cells.cells.values().all(|c| c.goal_unit != ids[0] && c.pos_unit != ids[0]));
}
