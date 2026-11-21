//! Auto-targeting when idle (`AIIdleState`, `getNextMoodTarget`) and guarding
//! (`AIGuardMachine`).
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation, ai::AiState, templates::Templates};
use std::sync::Arc;

const INI: &[u8] = b"AIData\nGuardOuterModifierHuman = 1.0\nGuardInnerModifierHuman = 1.0\nGuardChaseUnitsDuration = 5000\nEnd\n\
Weapon Rifle\nPrimaryDamage = 10\nAttackRange = 100\nDelayBetweenShots = 500\nEnd\n\
Object Soldier\nKindOf = INFANTRY\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\nVisionRange = 150\n\
WeaponSet\nConditions = None\nWeapon = PRIMARY Rifle\nEnd\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nAutoAcquireEnemiesWhenIdle = Yes\nEnd\nBehavior = PhysicsBehavior P\nEnd\nBehavior = DestroyDie Die\nEnd\nEnd\n\
Object Dummy\nKindOf = INFANTRY\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nBehavior = DestroyDie Die\nEnd\nEnd\n\
Locomotor L\nSurfaces = GROUND\nSpeed = 20\nAcceleration = 900\nBraking = 900\nTurnRate = 360\nEnd\n";

fn sim() -> Simulation {
    let mut defs = Definitions::default();
    defs.add_document(ih_ini::parse(INI));
    let map = Map {
        width: 84,
        height: 84,
        border: 2,
        boundaries: vec![[80, 80]],
        heights: vec![0; 84 * 84],
        ..Default::default()
    };
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&defs)), 1)
}
fn health(s: &Simulation, id: u32) -> f32 {
    s.objects.get(&id).and_then(|o| o.health.as_ref()).map_or(0.0, |h| h.current)
}

#[test]
fn an_idle_unit_attacks_an_enemy_that_comes_within_range() {
    let mut a = sim();
    let mut b = sim();
    for s in [&mut a, &mut b] {
        s.spawn("Soldier", [200.0, 200.0], 1).unwrap();
        s.spawn("Dummy", [500.0, 200.0], 2).unwrap();
        // The dummy walks past, about 80 units away at its closest.
        s.enqueue(1, Command::Move { object: 2, target: [200.0, 280.0] }).unwrap();
    }
    let mut first_hit = None;
    for _ in 0..900 {
        a.step();
        b.step();
        assert_eq!(a.state_hash(), b.state_hash());
        if first_hit.is_none() && health(&a, 2) < 100.0 {
            first_hit = Some(a.objects.get(&2).map(|o| o.position));
        }
    }
    let p = first_hit.expect("never fired").expect("victim gone at first hit");
    let d = ((p[0] - 200.0).powi(2) + (p[1] - 200.0).powi(2)).sqrt();
    assert!(d <= 97.5 + 6.0 + 1.0, "fired from beyond range: {d}");
    assert!(!a.objects.contains_key(&2), "the dummy died");
    // The soldier never moved: it attacked from where it stood.
    assert_eq!(a.objects[&1].position[..2], [200.0, 200.0]);
}

#[test]
fn an_auto_acquired_target_is_not_chased() {
    let mut s = sim();
    s.spawn("Soldier", [200.0, 200.0], 1).unwrap();
    let d = s.spawn("Dummy", [280.0, 200.0], 2).unwrap();
    // The dummy runs away after a few shots.
    s.enqueue(40, Command::Move { object: d, target: [700.0, 200.0] }).unwrap();
    for _ in 0..400 {
        s.step();
    }
    assert!(health(&s, d) < 100.0 && health(&s, d) > 0.0, "hit, then escaped: {}", health(&s, d));
    assert_eq!(s.objects[&1].position[..2], [200.0, 200.0], "no chase");
    assert_eq!(s.objects[&1].ai.state, AiState::Idle);
}

#[test]
fn a_guard_engages_intruders_and_returns_to_its_post() {
    let mut s = sim();
    let g = s.spawn("Soldier", [150.0, 150.0], 1).unwrap();
    s.enqueue(1, Command::Guard { object: g, position: [300.0, 300.0] }).unwrap();
    for _ in 0..300 {
        s.step();
    }
    let p = s.objects[&g].position;
    assert!(((p[0] - 300.0).powi(2) + (p[1] - 300.0).powi(2)).sqrt() < 10.0, "at its post: {p:?}");
    // An intruder inside the 150-unit guard range, beyond the rifle's reach:
    // the guard goes after it.
    let d = s.spawn("Dummy", [430.0, 300.0], 2).unwrap();
    for _ in 0..600 {
        s.step();
    }
    assert!(!s.objects.contains_key(&d), "the intruder died");
    for _ in 0..600 {
        s.step();
    }
    let p = s.objects[&g].position;
    assert!(((p[0] - 300.0).powi(2) + (p[1] - 300.0).powi(2)).sqrt() < 10.0, "back at its post: {p:?}");
    assert_eq!(s.objects[&g].ai.state, AiState::Guard);
}
