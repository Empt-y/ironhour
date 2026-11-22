//! Garrisons (`GarrisonContain`) and passengers firing from transports.
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation, templates::Templates};
use std::sync::Arc;

const INI: &[u8] = b"AIData\nGuardOuterModifierHuman = 1.0\nGuardInnerModifierHuman = 1.0\nEnd\n\
Weapon Rifle\nPrimaryDamage = 25\nAttackRange = 150\nDelayBetweenShots = 500\nEnd\n\
Object House\nKindOf = STRUCTURE IMMOBILE\nGeometry = BOX\nGeometryMajorRadius = 30\nGeometryMinorRadius = 30\nGeometryHeight = 30\n\
Body = ActiveBody Body\nMaxHealth = 1000\nInitialHealth = 1000\nEnd\n\
Behavior = GarrisonContain Garrison\nContainMax = 2\nDamagePercentToUnits = 10%\nEnd\n\
Behavior = DestroyDie Die\nEnd\nEnd\n\
Object Humvee\nKindOf = VEHICLE TRANSPORT\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 10\nVisionRange = 200\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\n\
Behavior = TransportContain Contain\nSlots = 5\nPassengersAllowedToFire = Yes\nEnd\nEnd\n\
Object Soldier\nKindOf = INFANTRY\nTransportSlotCount = 1\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\nVisionRange = 200\n\
WeaponSet\nConditions = None\nWeapon = PRIMARY Rifle\nEnd\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nAutoAcquireEnemiesWhenIdle = Yes\nEnd\nBehavior = PhysicsBehavior P\nEnd\nBehavior = DestroyDie Die\nEnd\nEnd\n\
Object Pilot\nKindOf = INFANTRY NO_GARRISON\nTransportSlotCount = 1\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\n\
Object Dummy\nKindOf = INFANTRY\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nBehavior = DestroyDie Die\nEnd\nEnd\n\
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
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&defs)), 25)
}
fn run(s: &mut Simulation, n: u32) {
    for _ in 0..n {
        s.step();
    }
}
fn riders(s: &Simulation, t: u32) -> Vec<u32> {
    s.objects[&t].contain.as_ref().unwrap().riders.clone()
}

#[test]
fn soldiers_garrison_a_building_take_it_over_and_shoot_out() {
    let mut s = sim();
    let h = s.spawn("House", [300.0, 300.0], 0).unwrap();
    let a = s.spawn("Soldier", [200.0, 300.0], 1).unwrap();
    let b = s.spawn("Soldier", [200.0, 340.0], 1).unwrap();
    let c = s.spawn("Soldier", [200.0, 260.0], 1).unwrap();
    for o in [a, b, c] {
        s.enqueue(1, Command::Enter { object: o, target: h }).unwrap();
    }
    run(&mut s, 300);
    // ContainMax 2: the third stays out.
    assert_eq!(riders(&s, h).len(), 2);
    assert_eq!(s.objects[&h].owner, 1, "the building is the occupants' now");
    // An enemy walks up; the garrison shoots it from inside.
    let d = s.spawn("Dummy", [430.0, 300.0], 2).unwrap();
    run(&mut s, 300);
    assert!(!s.objects.contains_key(&d), "shot from the building");
    assert!(riders(&s, h).iter().all(|r| s.contained(*r)));
    let f = s.frame;
    s.enqueue(f, Command::Evacuate { object: h }).unwrap();
    run(&mut s, 60);
    assert!(riders(&s, h).is_empty());
    assert_eq!(s.objects[&h].owner, 0, "back to neutral");
}

#[test]
fn some_units_cannot_garrison_and_enemies_cannot_share() {
    let mut s = sim();
    let h = s.spawn("House", [300.0, 300.0], 0).unwrap();
    let a = s.spawn("Soldier", [200.0, 300.0], 1).unwrap();
    let p = s.spawn("Pilot", [200.0, 340.0], 1).unwrap();
    let e = s.spawn("Soldier", [420.0, 300.0], 2).unwrap();
    s.enqueue(1, Command::Enter { object: a, target: h }).unwrap();
    s.enqueue(1, Command::Enter { object: p, target: h }).unwrap();
    run(&mut s, 200);
    assert_eq!(riders(&s, h), vec![a]);
    s.enqueue(200, Command::Enter { object: e, target: h }).unwrap();
    run(&mut s, 200);
    assert_eq!(riders(&s, h), vec![a], "an enemy cannot join");
}

#[test]
fn a_wrecked_building_is_abandoned_and_refuses_occupants() {
    let mut s = sim();
    let h = s.spawn("House", [300.0, 300.0], 0).unwrap();
    let a = s.spawn("Soldier", [200.0, 300.0], 1).unwrap();
    s.enqueue(1, Command::Enter { object: a, target: h }).unwrap();
    run(&mut s, 200);
    assert!(s.contained(a));
    // Down to really damaged (10%): everyone gets out.
    let f = s.frame;
    s.enqueue(
        f,
        Command::Damage {
            object: h,
            amount: 905.0,
            damage_type: "EXPLOSION".into(),
            death_type: "NORMAL".into(),
            source: 0,
        },
    )
    .unwrap();
    run(&mut s, 30);
    assert!(!s.contained(a), "evacuated");
    // Back in when it is whole is refused while really damaged.
    let f = s.frame;
    s.enqueue(f, Command::Enter { object: a, target: h }).unwrap();
    run(&mut s, 200);
    assert!(!s.contained(a));
}

#[test]
fn infantry_fire_from_a_transport_that_allows_it() {
    let mut s = sim();
    let t = s.spawn("Humvee", [300.0, 300.0], 1).unwrap();
    let a = s.spawn("Soldier", [250.0, 300.0], 1).unwrap();
    s.enqueue(1, Command::Enter { object: a, target: t }).unwrap();
    run(&mut s, 200);
    assert!(s.contained(a));
    let d = s.spawn("Dummy", [420.0, 300.0], 2).unwrap();
    run(&mut s, 400);
    assert!(!s.objects.contains_key(&d), "shot from the transport");
    assert!(s.contained(a));
}

#[test]
fn garrisons_are_deterministic() {
    let run_hashes = || {
        let mut s = sim();
        let h = s.spawn("House", [300.0, 300.0], 0).unwrap();
        let a = s.spawn("Soldier", [200.0, 300.0], 1).unwrap();
        s.spawn("Dummy", [440.0, 300.0], 2).unwrap();
        s.enqueue(1, Command::Enter { object: a, target: h }).unwrap();
        (0..500)
            .map(|_| {
                s.step();
                s.state_hash()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(run_hashes(), run_hashes());
}
