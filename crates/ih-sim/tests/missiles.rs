//! Missiles (`MissileAIUpdate` with a `THRUST` locomotor).
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation, missiles::MissileState, templates::Templates};
use std::sync::Arc;

const INI: &[u8] = b"Weapon Launcher\nPrimaryDamage = 50\nPrimaryDamageRadius = 5\nAttackRange = 300\nWeaponSpeed = 600\nProjectileObject = Rocket\nProjectileCollidesWith = STRUCTURES\nDelayBetweenShots = 5000\nEnd\n\
Object Rocket\nKindOf = PROJECTILE SMALL_MISSILE\nGeometryMajorRadius = 1\nLocomotor = SET_NORMAL RocketLoco\n\
Body = ActiveBody Body\nMaxHealth = 1\nInitialHealth = 1\nEnd\n\
Behavior = PhysicsBehavior P\nMass = 1\nEnd\n\
Behavior = MissileAIUpdate AI\nTryToFollowTarget = Yes\nIgnitionDelay = 100\nInitialVelocity = 60\nDistanceToTravelBeforeTurning = 20\nFuelLifetime = 4000\nDistanceToTargetForLock = 30\nEnd\n\
Behavior = DestroyDie Die\nEnd\nEnd\n\
Object Truck\nKindOf = VEHICLE\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 5\nGeometryHeight = 10\n\
WeaponSet\nConditions = None\nWeapon = PRIMARY Launcher\nEnd\n\
Body = ActiveBody Body\nMaxHealth = 300\nInitialHealth = 300\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\n\
GameData\nPartitionCellSize = 40\nEnd\n\
Locomotor RocketLoco\nSurfaces = AIR\nSpeed = 300\nAcceleration = 1200\nTurnRate = 360\nAppearance = THRUST\nMaxThrustAngle = 30\nEnd\n\
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
fn a_missile_ignites_steers_to_its_target_and_detonates() {
    let mut a = sim();
    let mut b = sim();
    for s in [&mut a, &mut b] {
        s.spawn("Truck", [100.0, 100.0], 1).unwrap();
        // Off to the side: the missile must turn once it may.
        s.spawn("Truck", [300.0, 200.0], 2).unwrap();
        s.enqueue(1, Command::Attack { object: 1, target: 2 }).unwrap();
    }
    let mut states = Vec::new();
    let mut hit = None;
    for _ in 0..120 {
        a.step();
        b.step();
        assert_eq!(a.state_hash(), b.state_hash());
        if let Some(m) = a.objects.values().find_map(|o| o.missile.as_ref())
            && states.last() != Some(&m.state)
        {
            states.push(m.state);
        }
        if hit.is_none() && health(&a, 2) < 300.0 {
            hit = Some(a.frame - 1);
        }
    }
    // Launched, coasting through the ignition delay (3 frames), then flying
    // straight, then steering, then locked on.
    assert_eq!(
        &states[..4],
        &[MissileState::Launch, MissileState::AttackNoTurn, MissileState::Attack, MissileState::Kill],
        "{states:?}"
    );
    let hit = hit.expect("the missile never hit");
    assert!(hit < 60, "hit on frame {hit}");
    assert_eq!(health(&a, 2), 250.0, "one detonation of 50");
    assert!(!a.objects.values().any(|o| o.template == "Rocket"), "the missile is gone");
}

#[test]
fn a_missile_whose_target_dies_destroys_itself() {
    let mut s = sim();
    s.spawn("Truck", [100.0, 100.0], 1).unwrap();
    let v = s.spawn("Truck", [380.0, 100.0], 2).unwrap();
    s.enqueue(1, Command::Attack { object: 1, target: v }).unwrap();
    for _ in 0..4 {
        s.step();
    }
    assert!(s.objects.values().any(|o| o.template == "Rocket"), "in flight");
    s.enqueue(s.frame, Command::Destroy { object: v }).unwrap();
    for _ in 0..20 {
        s.step();
    }
    assert!(!s.objects.values().any(|o| o.template == "Rocket"), "the missile destroyed itself");
}
