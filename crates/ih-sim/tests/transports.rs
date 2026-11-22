//! Transports (`TransportContain`, `AIEnterState`, `AIExitState`).
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation, templates::Templates};
use std::sync::Arc;

const INI: &[u8] = b"Object Truck\nKindOf = VEHICLE TRANSPORT\nLocomotor = SET_NORMAL L\nGeometry = BOX\nGeometryMajorRadius = 12\nGeometryMinorRadius = 6\n\
Body = ActiveBody Body\nMaxHealth = 300\nInitialHealth = 300\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\n\
Behavior = TransportContain Contain\nSlots = 3\nDamagePercentToUnits = 50%\nExitDelay = 250\nNumberOfExitPaths = 1\nEnd\n\
Behavior = DestroyDie Die\nEnd\nEnd\n\
Object Bus\nKindOf = VEHICLE TRANSPORT\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 12\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\n\
Behavior = TransportContain Contain\nSlots = 4\nInitialPayload = Soldier 2\nEnd\nEnd\n\
Object Soldier\nKindOf = INFANTRY\nTransportSlotCount = 1\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nBehavior = DestroyDie Die\nEnd\nEnd\n\
Object Ogre\nKindOf = INFANTRY\nTransportSlotCount = 2\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 4\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\n\
Object Tank\nKindOf = VEHICLE\nTransportSlotCount = 3\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 8\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\n\
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
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&defs)), 21)
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
fn infantry_get_in_ride_along_and_get_out() {
    let mut s = sim();
    let t = s.spawn("Truck", [300.0, 300.0], 1).unwrap();
    let a = s.spawn("Soldier", [200.0, 300.0], 1).unwrap();
    let b = s.spawn("Soldier", [200.0, 340.0], 1).unwrap();
    s.enqueue(1, Command::Enter { object: a, target: t }).unwrap();
    s.enqueue(1, Command::Enter { object: b, target: t }).unwrap();
    run(&mut s, 300);
    assert_eq!(riders(&s, t), vec![a, b]);
    assert!(s.contained(a) && s.contained(b));
    // The truck drives off with them.
    let f = s.frame;
    s.enqueue(f, Command::Move { object: t, target: [600.0, 300.0] }).unwrap();
    run(&mut s, 400);
    let p = s.objects[&t].position;
    assert!((p[0] - 600.0).abs() < 10.0, "{p:?}");
    let f = s.frame;
    s.enqueue(f, Command::Evacuate { object: t }).unwrap();
    // ExitDelay spaces the two out by eight frames.
    run(&mut s, 2);
    assert_eq!(riders(&s, t).len(), 1);
    run(&mut s, 8);
    assert!(riders(&s, t).is_empty());
    run(&mut s, 100);
    for id in [a, b] {
        let o = &s.objects[&id];
        assert!(!s.contained(id));
        assert!(((o.position[0] - p[0]).powi(2) + (o.position[1] - p[1]).powi(2)).sqrt() < 40.0, "{:?}", o.position);
    }
}

#[test]
fn only_what_fits_and_is_allowed_gets_in() {
    let mut s = sim();
    let t = s.spawn("Truck", [300.0, 300.0], 1).unwrap();
    let ogre = s.spawn("Ogre", [250.0, 300.0], 1).unwrap();
    let a = s.spawn("Soldier", [250.0, 340.0], 1).unwrap();
    let b = s.spawn("Soldier", [250.0, 260.0], 1).unwrap();
    let tank = s.spawn("Tank", [350.0, 360.0], 1).unwrap();
    let enemy = s.spawn("Soldier", [350.0, 250.0], 2).unwrap();
    for (f, o) in [(1, ogre), (60, a), (60, b), (60, tank), (60, enemy)] {
        s.enqueue(f, Command::Enter { object: o, target: t }).unwrap();
    }
    run(&mut s, 400);
    // Three slots: the two-slot ogre and one soldier. Vehicles and enemies never.
    let r = riders(&s, t);
    assert_eq!(r.len(), 2, "{r:?}");
    assert_eq!(r[0], ogre);
    assert!(!s.contained(tank) && !s.contained(enemy));
}

#[test]
fn a_dying_transport_hurts_its_passengers_and_lets_them_out() {
    let mut s = sim();
    let t = s.spawn("Truck", [300.0, 300.0], 1).unwrap();
    let a = s.spawn("Soldier", [250.0, 300.0], 1).unwrap();
    s.enqueue(1, Command::Enter { object: a, target: t }).unwrap();
    run(&mut s, 200);
    assert!(s.contained(a));
    let f = s.frame;
    s.enqueue(
        f,
        Command::Damage {
            object: t,
            amount: 1000.0,
            damage_type: "EXPLOSION".into(),
            death_type: "NORMAL".into(),
            source: 0,
        },
    )
    .unwrap();
    run(&mut s, 60);
    assert!(!s.objects.contains_key(&t));
    let o = &s.objects[&a];
    assert!(!s.contained(a));
    assert_eq!(o.health.as_ref().unwrap().current, 50.0, "DamagePercentToUnits");
}

#[test]
fn an_initial_payload_rides_in_from_the_start_and_transports_are_deterministic() {
    let mut s = sim();
    let bus = s.spawn("Bus", [300.0, 300.0], 1).unwrap();
    run(&mut s, 2);
    assert_eq!(riders(&s, bus).len(), 2);
    let run_hashes = || {
        let mut s = sim();
        let t = s.spawn("Truck", [300.0, 300.0], 1).unwrap();
        let a = s.spawn("Soldier", [200.0, 300.0], 1).unwrap();
        s.enqueue(1, Command::Enter { object: a, target: t }).unwrap();
        s.enqueue(200, Command::Exit { object: a }).unwrap();
        (0..400)
            .map(|_| {
                s.step();
                s.state_hash()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(run_hashes(), run_hashes());
}
