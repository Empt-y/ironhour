//! Weapons and attacks (`Weapon`, `WeaponSet`, `AIAttackState`).
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation, ai::Outcome, templates::Templates};
use std::sync::Arc;

const INI: &[u8] = b"Weapon Gun\nPrimaryDamage = 10\nAttackRange = 100\nDamageType = SMALL_ARMS\nDelayBetweenShots = 500\nEnd\n\
Weapon Burst\nPrimaryDamage = 20\nAttackRange = 100\nClipSize = 3\nClipReloadTime = 1000\nDelayBetweenShots = 100\nEnd\n\
Weapon Bomb\nPrimaryDamage = 50\nPrimaryDamageRadius = 30\nAttackRange = 150\nRadiusDamageAffects = ENEMIES\nDelayBetweenShots = 1000\nEnd\n\
Weapon Cannon\nPrimaryDamage = 40\nAttackRange = 200\nWeaponSpeed = 300\nProjectileObject = Shell\nDelayBetweenShots = 2000\nEnd\n\
Object Soldier\nKindOf = INFANTRY\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\n\
WeaponSet\nConditions = None\nWeapon = PRIMARY Gun\nEnd\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nBehavior = DestroyDie Die\nEnd\nEnd\n\
Object Gunner\nKindOf = INFANTRY\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\n\
WeaponSet\nConditions = None\nWeapon = PRIMARY Burst\nEnd\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\n\
Object Bomber\nKindOf = VEHICLE\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 5\n\
WeaponSet\nConditions = None\nWeapon = PRIMARY Bomb\nEnd\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\n\
Object Tank\nKindOf = VEHICLE\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 5\n\
WeaponSet\nConditions = None\nWeapon = PRIMARY Cannon\nEnd\n\
Body = ActiveBody Body\nMaxHealth = 1000\nInitialHealth = 1000\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\n\
Locomotor L\nSurfaces = GROUND\nSpeed = 30\nAcceleration = 900\nBraking = 900\nTurnRate = 360\nEnd\n";

fn sim() -> Simulation {
    let mut defs = Definitions::default();
    defs.add_document(ih_ini::parse(INI));
    let map = Map {
        width: 64,
        height: 64,
        border: 2,
        boundaries: vec![[60, 60]],
        heights: vec![0; 64 * 64],
        ..Default::default()
    };
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&defs)), 1)
}
fn health(s: &Simulation, id: u32) -> f32 {
    s.objects.get(&id).and_then(|o| o.health.as_ref()).map_or(0.0, |h| h.current)
}

#[test]
fn an_attack_fires_at_the_weapon_cadence_until_the_victim_dies() {
    let mut s = sim();
    let a = s.spawn("Soldier", [100.0, 100.0], 1).unwrap();
    let v = s.spawn("Soldier", [150.0, 100.0], 2).unwrap();
    s.enqueue(1, Command::Attack { object: a, target: v }).unwrap();
    let mut hits = Vec::new();
    let mut last = 100.0;
    for _ in 0..400 {
        s.step();
        let h = health(&s, v);
        if h < last {
            hits.push(s.frame - 1);
            last = h;
        }
    }
    // Ten shots of 10. The 15-frame delay (500 ms) becomes 16: after each shot the
    // machine aims (one frame), then fires on the next, so shots land on every
    // other frame. The victim is removed by DestroyDie.
    assert_eq!(hits.len(), 10, "{hits:?}");
    assert_eq!(hits[0], 2, "aim on the order's frame, fire on the next");
    assert!(hits.windows(2).all(|w| w[1] - w[0] == 16), "{hits:?}");
    assert!(!s.objects.contains_key(&v));
    assert_eq!(s.objects[&a].ai.outcome, Outcome::Success);
    assert!(s.objects[&a].is_idle());
}

#[test]
fn clips_reload_after_their_last_shot() {
    let mut s = sim();
    let a = s.spawn("Gunner", [100.0, 100.0], 1).unwrap();
    let v = s.spawn("Tank", [150.0, 100.0], 2).unwrap();
    s.enqueue(1, Command::Attack { object: a, target: v }).unwrap();
    let mut hits = Vec::new();
    let mut last = 1000.0;
    for _ in 0..120 {
        s.step();
        let h = health(&s, v);
        if h < last {
            hits.push(s.frame - 1);
            last = h;
        }
    }
    // Three shots 3 frames (100 ms, rounded up to even frames) apart, then a
    // 30-frame (1 s) reload.
    let gaps: Vec<u32> = hits.windows(2).map(|w| w[1] - w[0]).collect();
    assert_eq!(&gaps[..5], &[4, 4, 30, 4, 4], "{hits:?}");
}

#[test]
fn an_attacker_out_of_range_closes_in_before_firing() {
    let mut s = sim();
    let a = s.spawn("Soldier", [100.0, 100.0], 1).unwrap();
    let v = s.spawn("Tank", [400.0, 100.0], 2).unwrap();
    s.enqueue(1, Command::Attack { object: a, target: v }).unwrap();
    let mut first_hit = None;
    for _ in 0..300 {
        s.step();
        if first_hit.is_none() && health(&s, v) < 1000.0 {
            first_hit = Some(s.objects[&a].position[0]);
        }
    }
    let x = first_hit.expect("the attacker never fired");
    // In range (97.5 plus both radii) of the tank at x = 400.
    assert!(400.0 - x <= 97.5 + 8.0 + 0.5 && x > 250.0, "fired from {x}");
}

#[test]
fn radius_damage_spares_allies_when_it_affects_only_enemies() {
    let mut s = sim();
    let a = s.spawn("Bomber", [100.0, 100.0], 1).unwrap();
    let v = s.spawn("Soldier", [200.0, 100.0], 2).unwrap();
    let near_enemy = s.spawn("Soldier", [210.0, 110.0], 2).unwrap();
    let near_ally = s.spawn("Soldier", [205.0, 95.0], 1).unwrap();
    s.enqueue(1, Command::Attack { object: a, target: v }).unwrap();
    for _ in 0..10 {
        s.step();
    }
    assert_eq!(health(&s, v), 50.0);
    assert_eq!(health(&s, near_enemy), 50.0);
    assert_eq!(health(&s, near_ally), 100.0);
}

#[test]
fn projectile_damage_lands_after_the_flight_and_replays() {
    let mut a = sim();
    let mut b = sim();
    for s in [&mut a, &mut b] {
        let t = s.spawn("Tank", [100.0, 100.0], 1).unwrap();
        let v = s.spawn("Soldier", [300.0, 100.0], 2).unwrap();
        s.enqueue(1, Command::Attack { object: t, target: v }).unwrap();
    }
    let mut hit = None;
    for _ in 0..60 {
        a.step();
        b.step();
        assert_eq!(a.state_hash(), b.state_hash());
        if hit.is_none() && health(&a, 2) < 100.0 {
            hit = Some(a.frame - 1);
        }
    }
    // 200 units at 10 per frame: twenty frames after the shot on frame 2.
    assert_eq!(hit, Some(2 + 20));
}
