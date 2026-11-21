use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{
    Command, Simulation,
    ai::{AiState, Outcome},
    disabled::DisabledReason as D,
    templates::Templates,
};
use std::sync::Arc;
fn sim() -> Simulation {
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
fn until(s: &mut Simulation, frame: u32) {
    while s.frame < frame {
        s.step();
    }
}

#[test]
fn follow_path_visits_each_leg_and_completes_one_order() {
    let mut s = sim();
    s.spawn("Tank", [25.0, 25.0], 0).unwrap();
    let points = vec![[125.0, 25.0], [125.0, 125.0], [25.0, 125.0]];
    s.enqueue(1, Command::FollowPath { object: 1, points: points.clone() }).unwrap();
    let mut visited = [false; 3];
    for _ in 0..600 {
        s.step();
        let o = &s.objects[&1];
        for (i, p) in points.iter().enumerate() {
            visited[i] |= ih_math::length([p[0] - o.position[0], p[1] - o.position[1]]) < 0.6;
        }
    }
    assert!(visited.into_iter().all(|v| v));
    let o = &s.objects[&1];
    assert!(o.is_idle());
    assert_eq!(o.ai.completed_orders, 1);
    assert_eq!(o.ai.outcome, Outcome::Success);
    assert!(s.events.is_empty());
    assert_eq!(s.failed_orders, 0);
}
#[test]
fn emp_preserves_order_and_resumes_after_end_of_frame_expiry() {
    let mut s = sim();
    s.spawn("Tank", [25.0, 25.0], 0).unwrap();
    s.enqueue(1, Command::Move { object: 1, target: [175.0, 25.0] }).unwrap();
    s.enqueue(20, Command::Disable { object: 1, reason: D::Emp, frames: Some(30) }).unwrap();
    until(&mut s, 20);
    let p = s.objects[&1].position;
    let ai = s.objects[&1].modules[0].updates;
    let physics = s.objects[&1].modules[1].updates;
    until(&mut s, 51);
    let o = &s.objects[&1];
    assert_eq!(o.position, p);
    assert_eq!(o.ai.state, AiState::MoveTo);
    assert!(!o.disabled.any());
    assert_eq!(o.modules[0].updates, ai);
    // Physics runs while disabled, then sleeps once the stopped body rests and its
    // last motive force has expired (SLEEPY_PHYSICS).
    let ran = o.modules[1].updates - physics;
    assert!(ran > 0 && ran < 31, "physics ran {ran} times during the EMP");
    // The AI's first force wakes the sleeping body for the frame after.
    s.step();
    assert_eq!(s.objects[&1].position, p);
    s.step();
    assert!(s.objects[&1].position[0] > p[0]);
    until(&mut s, 300);
    assert!(s.objects[&1].is_idle());
    assert!(s.events.is_empty());
}
#[test]
fn held_exception_runs_ai_with_emp_but_drive_remains_frozen() {
    let mut s = sim();
    s.spawn("Tank", [25.0, 25.0], 0).unwrap();
    s.enqueue(1, Command::Move { object: 1, target: [175.0, 25.0] }).unwrap();
    s.enqueue(10, Command::Disable { object: 1, reason: D::Emp, frames: None }).unwrap();
    s.enqueue(11, Command::Disable { object: 1, reason: D::Held, frames: Some(10) }).unwrap();
    s.enqueue(25, Command::Enable { object: 1, reason: D::Emp }).unwrap();
    until(&mut s, 11);
    let n = s.objects[&1].modules[0].updates;
    let p = s.objects[&1].position;
    until(&mut s, 22);
    assert_eq!(s.objects[&1].modules[0].updates, n + 11);
    assert_eq!(s.objects[&1].position, p);
    until(&mut s, 25);
    assert_eq!(s.objects[&1].modules[0].updates, n + 11);
    until(&mut s, 300);
    assert!(s.objects[&1].is_idle());
    assert!(s.events.is_empty());
}
#[test]
fn stop_while_disabled_cancels_the_old_move_permanently() {
    let mut s = sim();
    s.spawn("Tank", [25.0, 25.0], 0).unwrap();
    s.enqueue(1, Command::Move { object: 1, target: [175.0, 25.0] }).unwrap();
    s.enqueue(10, Command::Disable { object: 1, reason: D::Emp, frames: Some(30) }).unwrap();
    s.enqueue(15, Command::Stop { object: 1 }).unwrap();
    until(&mut s, 11);
    let p = s.objects[&1].position;
    until(&mut s, 200);
    let o = &s.objects[&1];
    assert_eq!(o.position, p);
    assert!(o.is_idle());
    assert_eq!(o.ai.interrupted_orders, 1);
    assert_eq!(o.ai.outcome, Outcome::Interrupted);
}
#[test]
fn facing_interrupts_motion_without_changing_position() {
    let mut s = sim();
    s.spawn("Tank", [25.0, 25.0], 0).unwrap();
    s.enqueue(1, Command::Move { object: 1, target: [175.0, 25.0] }).unwrap();
    s.enqueue(10, Command::Face { object: 1, target: [25.0, 125.0] }).unwrap();
    until(&mut s, 10);
    let p = s.objects[&1].position;
    until(&mut s, 100);
    let o = &s.objects[&1];
    assert_eq!(o.position, p);
    assert!(o.is_idle());
    assert!(ih_math::angle_delta(o.angle, ih_math::atan2(125.0 - p[1], 25.0 - p[0])).abs() < 0.035);
    assert_eq!(o.ai.interrupted_orders, 1);
    assert_eq!(o.ai.completed_orders, 1);
}
#[test]
fn facing_a_live_object_tracks_it_and_missing_target_fails_to_idle() {
    let mut s = sim();
    s.spawn("Tank", [25.0, 25.0], 0).unwrap();
    s.spawn("Tank", [125.0, 25.0], 0).unwrap();
    s.objects.get_mut(&1).unwrap().angle = std::f32::consts::PI;
    s.enqueue(1, Command::FaceObject { object: 1, target: 2 }).unwrap();
    s.enqueue(1, Command::Move { object: 2, target: [125.0, 175.0] }).unwrap();
    until(&mut s, 3);
    while s.objects[&1].ai.active() {
        s.step();
        assert!(s.frame < 100);
    }
    let (a, b) = (&s.objects[&1], &s.objects[&2]);
    assert!(
        ih_math::angle_delta(a.angle, ih_math::atan2(b.position[1] - a.position[1], b.position[0] - a.position[0]))
            .abs()
            < 0.04
    );
    s.objects.get_mut(&1).unwrap().angle = std::f32::consts::PI;
    s.enqueue(s.frame, Command::FaceObject { object: 1, target: 2 }).unwrap();
    s.enqueue(s.frame + 1, Command::Destroy { object: 2 }).unwrap();
    let end = s.frame + 10;
    until(&mut s, end);
    assert!(s.objects[&1].is_idle());
    assert_eq!(s.objects[&1].ai.outcome, Outcome::Failure);
    assert_eq!(s.failed_orders, 1);
    assert!(s.events.is_empty());
}
#[test]
fn unreachable_later_leg_fails_the_action_instead_of_reporting_success() {
    let mut s = sim();
    s.spawn("Tank", [25.0, 25.0], 0).unwrap();
    s.enqueue(1, Command::FollowPath { object: 1, points: vec![[125.0, 25.0], [5000.0, 25.0]] }).unwrap();
    until(&mut s, 300);
    let o = &s.objects[&1];
    assert!(o.is_idle());
    assert_eq!(o.ai.outcome, Outcome::Failure);
    assert_eq!(o.ai.completed_orders, 0);
    assert_eq!(s.failed_orders, 1);
}
#[test]
fn disabled_units_reject_new_motion_and_replacement_deadlines_are_hashed() {
    let mut a = sim();
    a.spawn("Tank", [25.0, 25.0], 0).unwrap();
    a.enqueue(1, Command::Disable { object: 1, reason: D::Emp, frames: None }).unwrap();
    a.enqueue(2, Command::Move { object: 1, target: [125.0, 25.0] }).unwrap();
    until(&mut a, 20);
    assert!(a.objects[&1].is_idle());
    assert_eq!(a.objects[&1].position[..2], [25.0, 25.0]);
    assert_eq!(a.events.len(), 1);
    let before = a.state_hash();
    a.objects.get_mut(&1).unwrap().disabled.set(D::Emp, a.frame, Some(30));
    assert_ne!(before, a.state_hash());
}
#[test]
fn status_and_order_log_replays_match_each_tick() {
    let mut a = sim();
    let mut b = sim();
    for s in [&mut a, &mut b] {
        s.spawn("Tank", [25.0, 25.0], 0).unwrap();
        s.enqueue(1, Command::FollowPath { object: 1, points: vec![[125.0, 25.0], [125.0, 125.0]] }).unwrap();
        s.enqueue(20, Command::Disable { object: 1, reason: D::Emp, frames: Some(30) }).unwrap();
        s.enqueue(30, Command::Disable { object: 1, reason: D::Held, frames: Some(40) }).unwrap();
        s.enqueue(400, Command::Face { object: 1, target: [25.0, 125.0] }).unwrap();
    }
    for _ in 0..600 {
        a.step();
        b.step();
        assert_eq!(a.state_hash(), b.state_hash());
        assert!(a.overlaps().is_empty());
    }
    assert!(a.objects[&1].is_idle());
    assert_eq!(a.objects[&1].ai.completed_orders, 2);
    assert!(a.events.is_empty());
    assert_eq!(a.state_hash(), 0x73ae2a0f752820ea, "portable AI/disabled fixture changed");
}
