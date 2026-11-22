//! Shroud and fog (`PartitionCell` shroud levels, `Object::look`).
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation, shroud::Visibility, templates::Templates};
use std::sync::Arc;

const INI: &[u8] = b"GameData\nPartitionCellSize = 40\nUnlookPersistDuration = 1000\nEnd\n\
AIData\nGuardOuterModifierHuman = 1.0\nGuardInnerModifierHuman = 1.0\nEnd\n\
Weapon Rifle\nPrimaryDamage = 10\nAttackRange = 100\nDelayBetweenShots = 1000\nEnd\n\
Object Scout\nKindOf = INFANTRY\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\nVisionRange = 150\nShroudClearingRange = 120\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nBehavior = DestroyDie Die\nEnd\nEnd\n\
Object Nearsighted\nKindOf = INFANTRY\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\nVisionRange = 150\nShroudClearingRange = 20\n\
WeaponSet\nConditions = None\nWeapon = PRIMARY Rifle\nEnd\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nAutoAcquireEnemiesWhenIdle = Yes\nEnd\nBehavior = PhysicsBehavior P\nEnd\nBehavior = DestroyDie Die\nEnd\nEnd\n\
Object Target\nKindOf = INFANTRY\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nBehavior = DestroyDie Die\nEnd\nEnd\n\
Locomotor L\nSurfaces = GROUND\nSpeed = 30\nAcceleration = 900\nBraking = 900\nTurnRate = 360\nEnd\n";

fn sim() -> Simulation {
    let mut defs = Definitions::default();
    defs.add_document(ih_ini::parse(INI));
    let map = Map {
        width: 104,
        height: 104,
        border: 2,
        boundaries: vec![[100, 100]],
        heights: vec![0; 104 * 104],
        ..Default::default()
    };
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&defs)), 41)
}
fn run(s: &mut Simulation, n: u32) {
    for _ in 0..n {
        s.step();
    }
}

#[test]
fn the_discrete_circle_is_symmetric_and_covers_its_radius() {
    let lines = ih_sim::shroud::discrete_circle(10, 10, 3);
    let rows: Vec<i32> = lines.iter().map(|l| l.2).collect();
    for y in 7..=13 {
        assert_eq!(rows.iter().filter(|r| **r == y).count(), 1, "row {y}");
    }
    let centre = lines.iter().find(|l| l.2 == 10).unwrap();
    assert_eq!((centre.0, centre.1), (7, 13));
    for l in &lines {
        let mirror = lines.iter().find(|m| m.2 == 20 - l.2).unwrap();
        assert_eq!((l.0, l.1), (mirror.0, mirror.1));
    }
}

#[test]
fn a_unit_clears_the_shroud_for_its_side_and_fog_follows_it() {
    let mut s = sim();
    let scout = s.spawn("Scout", [300.0, 300.0], 1).unwrap();
    s.spawn("Target", [900.0, 900.0], 2).unwrap();
    run(&mut s, 2);
    assert_eq!(s.shroud.status_at(1, [300.0, 300.0]), Visibility::Clear);
    assert_eq!(s.shroud.status_at(1, [380.0, 300.0]), Visibility::Clear);
    assert_eq!(s.shroud.status_at(1, [700.0, 300.0]), Visibility::Shrouded);
    assert_eq!(s.shroud.status_at(2, [300.0, 300.0]), Visibility::Shrouded, "not for the enemy");
    // Walk away: the old ground stays clear for UnlookPersistDuration, then fogs.
    s.enqueue(2, Command::Move { object: scout, target: [700.0, 300.0] }).unwrap();
    run(&mut s, 20);
    assert_eq!(s.shroud.status_at(1, [220.0, 300.0]), Visibility::Clear, "still remembered");
    run(&mut s, 450);
    assert_eq!(s.shroud.status_at(1, [220.0, 300.0]), Visibility::Fogged);
    assert_eq!(s.shroud.status_at(1, [700.0, 300.0]), Visibility::Clear);
}

#[test]
fn units_only_pick_targets_their_side_can_see() {
    let mut s = sim();
    // Can see 20 around itself, so an enemy 60 away is in range but fogged.
    let g = s.spawn("Nearsighted", [300.0, 300.0], 1).unwrap();
    let t = s.spawn("Target", [360.0, 300.0], 2).unwrap();
    run(&mut s, 300);
    assert_eq!(s.shrouded_status(t, 1), Visibility::Shrouded);
    assert_eq!(s.objects[&t].health.as_ref().unwrap().current, 100.0);
    assert!(!s.objects[&g].ai.active());
    // An allied scout lights it up.
    s.spawn("Scout", [320.0, 340.0], 1).unwrap();
    run(&mut s, 200);
    assert!(s.objects[&t].health.as_ref().unwrap().current < 100.0);
}

#[test]
fn shroud_is_deterministic() {
    let hashes = || {
        let mut s = sim();
        let a = s.spawn("Scout", [300.0, 300.0], 1).unwrap();
        s.spawn("Nearsighted", [500.0, 300.0], 2).unwrap();
        s.enqueue(5, Command::Move { object: a, target: [600.0, 500.0] }).unwrap();
        (0..300)
            .map(|_| {
                s.step();
                s.state_hash()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(hashes(), hashes());
}
