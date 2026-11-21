//! Money, power and unit production (`Money`, `Energy`, `ProductionUpdate`,
//! `DefaultProductionExitUpdate`, `QueueProductionExitUpdate`).
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation, templates::Templates};
use std::sync::Arc;

const INI: &[u8] = b"GameData\nLowEnergyPenaltyModifier = 0.4\nMinLowEnergyProductionSpeed = 0.5\n\
MaxLowEnergyProductionSpeed = 0.8\nMultipleFactory = 0.85\nEnd\n\
CommandButton Command_Soldier\nCommand = UNIT_BUILD\nObject = Soldier\nEnd\n\
CommandButton Command_Elite\nCommand = UNIT_BUILD\nObject = Elite\nEnd\n\
CommandSet BarracksSet\n1 = Command_Soldier\n2 = Command_Elite\nEnd\n\
Object Barracks\nKindOf = STRUCTURE IMMOBILE\nCommandSet = BarracksSet\nEnergyProduction = -1\n\
Behavior = ProductionUpdate PU\nEnd\n\
Behavior = DefaultProductionExitUpdate Exit\nUnitCreatePoint = X:30.0 Y:0.0 Z:0.0\nNaturalRallyPoint = X:60.0 Y:0.0 Z:0.0\nEnd\nEnd\n\
Object Factory\nKindOf = STRUCTURE IMMOBILE\nCommandSet = BarracksSet\n\
Behavior = ProductionUpdate PU\nQuantityModifier = Soldier 2\nEnd\n\
Behavior = QueueProductionExitUpdate Exit\nUnitCreatePoint = X:30.0 Y:0.0 Z:0.0\nNaturalRallyPoint = X:60.0 Y:0.0 Z:0.0\nExitDelay = 1000\nEnd\nEnd\n\
Object DoorFactory\nKindOf = STRUCTURE IMMOBILE\nCommandSet = BarracksSet\n\
Behavior = ProductionUpdate PU\nNumDoorAnimations = 1\nDoorOpeningTime = 500\nDoorWaitOpenTime = 1000\nDoorCloseTime = 500\nEnd\n\
Behavior = DefaultProductionExitUpdate Exit\nUnitCreatePoint = X:30.0 Y:0.0 Z:0.0\nNaturalRallyPoint = X:60.0 Y:0.0 Z:0.0\nEnd\nEnd\n\
Object PowerPlant\nKindOf = STRUCTURE IMMOBILE\nEnergyProduction = 5\nEnd\n\
Object Academy\nKindOf = STRUCTURE IMMOBILE\nEnd\n\
Object Soldier\nKindOf = INFANTRY\nBuildCost = 200\nBuildTime = 2.0\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\n\
Prerequisites\nObject = Barracks Factory DoorFactory\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\n\
Object Elite\nKindOf = INFANTRY\nBuildCost = 500\nBuildTime = 4.0\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\n\
Prerequisites\nObject = Barracks\nObject = Academy\nEnd\n\
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
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&defs)), 1)
}
fn soldiers(s: &Simulation) -> Vec<u32> {
    s.objects.values().filter(|o| o.template == "Soldier").map(|o| o.id).collect()
}
/// Steps until a new soldier appears; returns the frame it was made on.
fn until_soldier(s: &mut Simulation, limit: u32) -> Option<u32> {
    let n = soldiers(s).len();
    for _ in 0..limit {
        s.step();
        if soldiers(s).len() > n {
            return Some(s.frame - 1);
        }
    }
    None
}

#[test]
fn production_costs_money_takes_build_time_and_exits_to_the_rally_point() {
    let mut s = sim();
    let b = s.spawn("Barracks", [300.0, 300.0], 1).unwrap();
    s.spawn("PowerPlant", [500.0, 500.0], 1).unwrap();
    s.enqueue(0, Command::Deposit { player: 1, amount: 1000 }).unwrap();
    s.enqueue(1, Command::Produce { factory: b, template: "Soldier".into() }).unwrap();
    s.step();
    assert_eq!(s.money(1), 1000);
    // 2 s at 30 frames a second with full power: the 60th update, frame 60.
    assert_eq!(until_soldier(&mut s, 200), Some(60));
    assert_eq!(s.money(1), 800);
    let id = soldiers(&s)[0];
    let o = &s.objects[&id];
    // Created at the create point; its AI already took its first step this frame.
    assert!((o.position[0] - 330.0).abs() < 1.5 && (o.position[1] - 300.0).abs() < 0.5, "{:?}", o.position);
    assert!(o.ai.cmd_from_ai);
    for _ in 0..200 {
        s.step();
    }
    let p = s.objects[&id].position;
    assert!((p[0] - 360.0).abs() < 6.0 && (p[1] - 300.0).abs() < 6.0, "at the natural rally point: {p:?}");
    assert!(s.objects[&b].production.as_ref().unwrap().queue.is_empty());
}

#[test]
fn low_power_slows_production_and_more_factories_speed_it_up() {
    let mut s = sim();
    s.spawn("Barracks", [300.0, 300.0], 1).unwrap();
    let t = s.templates.objects["Soldier"].clone();
    // No power plant: the barracks' 1 unit of consumption is unmet.
    let rate: f32 = (1.0f32 - 1.0 * 0.4).clamp(0.5, 0.8);
    assert_eq!(s.time_to_build(&t, 1), (60.0 / rate) as i32);
    s.spawn("PowerPlant", [500.0, 500.0], 1).unwrap();
    assert_eq!(s.time_to_build(&t, 1), 60);
    s.spawn("Barracks", [300.0, 500.0], 1).unwrap();
    assert_eq!(s.time_to_build(&t, 1), (60.0f32 * 0.85) as i32);
}

#[test]
fn production_needs_money_prerequisites_and_queue_space() {
    let mut s = sim();
    let b = s.spawn("Barracks", [300.0, 300.0], 1).unwrap();
    s.enqueue(0, Command::Deposit { player: 1, amount: 2000 }).unwrap();
    // No academy yet.
    s.enqueue(1, Command::Produce { factory: b, template: "Elite".into() }).unwrap();
    for _ in 0..9 {
        s.enqueue(1, Command::Produce { factory: b, template: "Soldier".into() }).unwrap();
    }
    s.step();
    s.step();
    let p = s.objects[&b].production.as_ref().unwrap();
    // 2000 buys ten soldiers, but the queue holds nine.
    assert_eq!(p.queue.len(), 9);
    assert_eq!(s.money(1), 2000 - 9 * 200);
    assert_eq!(s.events.len(), 1, "{:?}", s.events);
    // Cancelling refunds in full.
    let last = p.queue.back().unwrap().id;
    s.enqueue(2, Command::CancelProduction { factory: b, production: last }).unwrap();
    s.step();
    assert_eq!(s.money(1), 2000 - 8 * 200);
    // Not enough money for an elite (500), even with the academy.
    s.spawn("Academy", [500.0, 300.0], 1).unwrap();
    s.enqueue(3, Command::Produce { factory: b, template: "Elite".into() }).unwrap();
    s.step();
    assert_eq!(s.objects[&b].production.as_ref().unwrap().queue.len(), 8);
    assert!(s.events.last().unwrap().contains("NoMoney"), "{:?}", s.events);
    s.enqueue(4, Command::Deposit { player: 1, amount: 100 }).unwrap();
    s.enqueue(4, Command::Produce { factory: b, template: "Elite".into() }).unwrap();
    s.step();
    assert_eq!(s.objects[&b].production.as_ref().unwrap().queue.len(), 9);
    assert_eq!(s.money(1), 0);
}

#[test]
fn a_queue_exit_spaces_out_a_batch_and_a_door_delays_the_exit() {
    let mut s = sim();
    let f = s.spawn("Factory", [300.0, 300.0], 1).unwrap();
    s.spawn("PowerPlant", [500.0, 500.0], 1).unwrap();
    s.enqueue(0, Command::Deposit { player: 1, amount: 1000 }).unwrap();
    s.enqueue(1, Command::Produce { factory: f, template: "Soldier".into() }).unwrap();
    s.enqueue(1, Command::SetRallyPoint { object: f, position: [300.0, 500.0] }).unwrap();
    // One build makes two soldiers; the second waits out the one-second exit delay.
    let first = until_soldier(&mut s, 200).unwrap();
    assert_eq!(first, 60);
    let second = until_soldier(&mut s, 200).unwrap();
    assert_eq!(second, first + 31);
    assert!(s.objects[&f].production.as_ref().unwrap().queue.is_empty());
    for _ in 0..400 {
        s.step();
    }
    for id in soldiers(&s) {
        let p = s.objects[&id].position;
        assert!(((p[0] - 300.0).powi(2) + (p[1] - 500.0).powi(2)).sqrt() < 20.0, "at the rally point: {p:?}");
    }

    let mut s = sim();
    let d = s.spawn("DoorFactory", [300.0, 300.0], 1).unwrap();
    s.spawn("PowerPlant", [500.0, 500.0], 1).unwrap();
    s.enqueue(0, Command::Deposit { player: 1, amount: 1000 }).unwrap();
    s.enqueue(1, Command::Produce { factory: d, template: "Soldier".into() }).unwrap();
    // Built on frame 60; the door opens for 15 frames and the unit leaves on the 16th.
    assert_eq!(until_soldier(&mut s, 200), Some(76));
}

#[test]
fn production_is_deterministic() {
    let run = || {
        let mut s = sim();
        let f = s.spawn("Factory", [300.0, 300.0], 1).unwrap();
        s.enqueue(0, Command::Deposit { player: 1, amount: 1000 }).unwrap();
        for _ in 0..3 {
            s.enqueue(1, Command::Produce { factory: f, template: "Soldier".into() }).unwrap();
        }
        let mut hashes = vec![];
        for _ in 0..600 {
            s.step();
            hashes.push(s.state_hash());
        }
        (hashes, soldiers(&s).len())
    };
    let (a, n) = run();
    assert_eq!(a, run().0);
    assert_eq!(n, 6);
}
