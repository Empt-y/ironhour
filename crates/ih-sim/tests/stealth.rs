//! Stealth (`StealthUpdate`) and detection (`StealthDetectorUpdate`).
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation, templates::Templates};
use std::sync::Arc;

const INI: &[u8] = b"AIData\nGuardOuterModifierHuman = 1.0\nGuardInnerModifierHuman = 1.0\nEnd\n\
Weapon Rifle\nPrimaryDamage = 10\nAttackRange = 100\nDelayBetweenShots = 1000\nEnd\n\
Object Sneak\nKindOf = INFANTRY\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\nVisionRange = 150\n\
WeaponSet\nConditions = None\nWeapon = PRIMARY Rifle\nEnd\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\n\
Behavior = StealthUpdate Stealth\nStealthDelay = 2000\nStealthForbiddenConditions = MOVING FIRING_PRIMARY\nMoveThresholdSpeed = 3\nEnd\n\
Behavior = DestroyDie Die\nEnd\nEnd\n\
Object Guard\nKindOf = INFANTRY\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\nVisionRange = 150\n\
WeaponSet\nConditions = None\nWeapon = PRIMARY Rifle\nEnd\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nAutoAcquireEnemiesWhenIdle = Yes\nEnd\nBehavior = PhysicsBehavior P\nEnd\n\
Behavior = DestroyDie Die\nEnd\nEnd\n\
Object Radar\nKindOf = STRUCTURE IMMOBILE\nGeometryMajorRadius = 5\nVisionRange = 200\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = StealthDetectorUpdate Detect\nDetectionRate = 500\nEnd\nEnd\n\
Locomotor L\nSurfaces = GROUND\nSpeed = 30\nAcceleration = 900\nBraking = 900\nTurnRate = 360\nEnd\n";

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
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&defs)), 37)
}
fn run(s: &mut Simulation, n: u32) {
    for _ in 0..n {
        s.step();
    }
}
fn bits() -> (u64, u64) {
    let b = ih_sim::stealth::bits();
    (b.stealthed, b.detected)
}
fn stealthed(s: &Simulation, id: u32) -> bool {
    s.objects[&id].status & bits().0 != 0
}
fn detected(s: &Simulation, id: u32) -> bool {
    s.objects[&id].status & bits().1 != 0
}
fn health(s: &Simulation, id: u32) -> f32 {
    s.objects[&id].health.as_ref().unwrap().current
}

#[test]
fn a_stealthed_unit_is_ignored_until_detected() {
    let mut s = sim();
    let sneak = s.spawn("Sneak", [300.0, 300.0], 1).unwrap();
    // StealthDelay: two seconds before it first hides.
    run(&mut s, 50);
    assert!(!stealthed(&s, sneak));
    run(&mut s, 20);
    assert!(stealthed(&s, sneak));
    // A guard in range never picks it.
    let guard = s.spawn("Guard", [360.0, 300.0], 2).unwrap();
    run(&mut s, 300);
    assert_eq!(health(&s, sneak), 100.0);
    assert!(!s.objects[&guard].ai.active());
    // A player's order to attack it is refused too.
    let f = s.frame;
    s.enqueue(f, Command::Attack { object: guard, target: sneak }).unwrap();
    run(&mut s, 60);
    assert_eq!(health(&s, sneak), 100.0);
    // A detector of the guard's side reveals it, and the guard opens fire.
    s.spawn("Radar", [340.0, 340.0], 2).unwrap();
    run(&mut s, 20);
    assert!(detected(&s, sneak) && stealthed(&s, sneak));
    run(&mut s, 120);
    assert!(health(&s, sneak) < 100.0);
}

#[test]
fn moving_and_firing_reveal_and_the_delay_restarts() {
    let mut s = sim();
    let sneak = s.spawn("Sneak", [300.0, 300.0], 1).unwrap();
    run(&mut s, 70);
    assert!(stealthed(&s, sneak));
    let f = s.frame;
    s.enqueue(f, Command::Move { object: sneak, target: [400.0, 300.0] }).unwrap();
    run(&mut s, 10);
    assert!(!stealthed(&s, sneak), "moving");
    // Arrived and still: hidden again two seconds after it stopped.
    run(&mut s, 150);
    assert!(s.objects[&sneak].is_idle());
    let stopped = (0..200).find(|_| {
        s.step();
        stealthed(&s, sneak)
    });
    assert!(stopped.is_some());
    // Firing its primary weapon gives it away.
    let target = s.spawn("Guard", [460.0, 300.0], 2).unwrap();
    let f = s.frame;
    s.enqueue(f, Command::Attack { object: sneak, target }).unwrap();
    let mut seen = false;
    for _ in 0..60 {
        s.step();
        seen |= !stealthed(&s, sneak);
    }
    assert!(seen, "revealed by firing");
    assert!(health(&s, target) < 100.0);
}

#[test]
fn stealth_is_deterministic() {
    let hashes = || {
        let mut s = sim();
        s.spawn("Sneak", [300.0, 300.0], 1).unwrap();
        s.spawn("Guard", [360.0, 300.0], 2).unwrap();
        s.spawn("Radar", [340.0, 340.0], 2).unwrap();
        (0..200)
            .map(|_| {
                s.step();
                s.state_hash()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(hashes(), hashes());
}
