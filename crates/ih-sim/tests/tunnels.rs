//! Tunnel networks (`TunnelContain`, `TunnelTracker`).
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation, templates::Templates};
use std::sync::Arc;

const INI: &[u8] = b"GameData\nMaxTunnelCapacity = 3\nEnd\n\
Object Tunnel\nKindOf = STRUCTURE IMMOBILE\nGeometryMajorRadius = 15\n\
Body = ActiveBody Body\nMaxHealth = 500\nInitialHealth = 500\nEnd\n\
Behavior = TunnelContain Tunnel\nTimeForFullHeal = 5000\nNumberOfExitPaths = 1\nEnd\n\
Behavior = DestroyDie Die\nEnd\nEnd\n\
Object Rebel\nKindOf = INFANTRY\nTransportSlotCount = 1\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nBehavior = DestroyDie Die\nEnd\nEnd\n\
Object Jet\nKindOf = AIRCRAFT\nTransportSlotCount = 1\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\n\
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
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&defs)), 27)
}
fn run(s: &mut Simulation, n: u32) {
    for _ in 0..n {
        s.step();
    }
}
fn damage(s: &mut Simulation, id: u32, amount: f32) {
    let f = s.frame;
    s.enqueue(
        f,
        Command::Damage { object: id, amount, damage_type: "EXPLOSION".into(), death_type: "NORMAL".into(), source: 0 },
    )
    .unwrap();
    s.step();
}

#[test]
fn units_enter_one_tunnel_and_leave_from_another() {
    let mut s = sim();
    let west = s.spawn("Tunnel", [200.0, 300.0], 1).unwrap();
    let east = s.spawn("Tunnel", [650.0, 300.0], 1).unwrap();
    let a = s.spawn("Rebel", [150.0, 300.0], 1).unwrap();
    let b = s.spawn("Rebel", [150.0, 340.0], 1).unwrap();
    s.enqueue(1, Command::Enter { object: a, target: west }).unwrap();
    s.enqueue(1, Command::Enter { object: b, target: west }).unwrap();
    run(&mut s, 200);
    assert_eq!(s.riders_of(east), vec![a, b], "the network is shared");
    let f = s.frame;
    s.enqueue(f, Command::Evacuate { object: east }).unwrap();
    run(&mut s, 60);
    for id in [a, b] {
        assert!(!s.contained(id));
        let p = s.objects[&id].position;
        assert!((p[0] - 650.0).abs() < 50.0, "came out of the east tunnel: {p:?}");
    }
}

#[test]
fn the_network_has_one_capacity_and_takes_no_aircraft() {
    let mut s = sim();
    let t1 = s.spawn("Tunnel", [200.0, 300.0], 1).unwrap();
    let t2 = s.spawn("Tunnel", [400.0, 300.0], 1).unwrap();
    let mut rebels = vec![];
    for (i, t) in [t1, t1, t2, t2].into_iter().enumerate() {
        let r = s.spawn("Rebel", [150.0 + 300.0 * (i / 2) as f32, 260.0 + 30.0 * i as f32], 1).unwrap();
        s.enqueue(1, Command::Enter { object: r, target: t }).unwrap();
        rebels.push(r);
    }
    let jet = s.spawn("Jet", [300.0, 200.0], 1).unwrap();
    s.enqueue(1, Command::Enter { object: jet, target: t1 }).unwrap();
    run(&mut s, 300);
    assert_eq!(s.riders_of(t1).len(), 3, "MaxTunnelCapacity");
    assert!(!s.contained(jet));
}

#[test]
fn units_heal_inside_and_die_with_the_last_tunnel() {
    let mut s = sim();
    let t1 = s.spawn("Tunnel", [200.0, 300.0], 1).unwrap();
    let t2 = s.spawn("Tunnel", [500.0, 300.0], 1).unwrap();
    let a = s.spawn("Rebel", [150.0, 300.0], 1).unwrap();
    damage(&mut s, a, 60.0);
    s.enqueue(s.frame, Command::Enter { object: a, target: t1 }).unwrap();
    run(&mut s, 150);
    assert!(s.contained(a));
    // Five seconds for a full heal, from two tunnels at once.
    run(&mut s, 160);
    assert_eq!(s.objects[&a].health.as_ref().unwrap().current, 100.0);
    // Losing the tunnel it entered moves it to the other one.
    damage(&mut s, t1, 1000.0);
    run(&mut s, 5);
    assert_eq!(s.objects[&a].contained_by, Some(t2));
    damage(&mut s, t2, 1000.0);
    run(&mut s, 5);
    assert!(!s.objects.contains_key(&a), "gone with the last tunnel");
}

#[test]
fn tunnels_are_deterministic() {
    let run_hashes = || {
        let mut s = sim();
        let t1 = s.spawn("Tunnel", [200.0, 300.0], 1).unwrap();
        let t2 = s.spawn("Tunnel", [500.0, 300.0], 1).unwrap();
        let a = s.spawn("Rebel", [150.0, 300.0], 1).unwrap();
        s.enqueue(1, Command::Enter { object: a, target: t1 }).unwrap();
        s.enqueue(200, Command::Evacuate { object: t2 }).unwrap();
        (0..400)
            .map(|_| {
                s.step();
                s.state_hash()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(run_hashes(), run_hashes());
}
