//! Construction (`BuildAssistant::buildObjectNow`, `DozerAIUpdate`).
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation, footprint::CellType, templates::Templates};
use std::sync::Arc;

const INI: &[u8] = b"GameData\nSellPercentage = 50%\nMinLowEnergyProductionSpeed = 1.0\nMaxLowEnergyProductionSpeed = 1.0\nEnd\n\
CommandButton Command_Barracks\nCommand = DOZER_CONSTRUCT\nObject = Barracks\nEnd\n\
CommandButton Command_Plant\nCommand = DOZER_CONSTRUCT\nObject = PowerPlant\nEnd\n\
CommandSet DozerSet\n1 = Command_Barracks\n2 = Command_Plant\nEnd\n\
Object Dozer\nKindOf = VEHICLE DOZER\nCommandSet = DozerSet\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 6\n\
Body = ActiveBody Body\nMaxHealth = 250\nInitialHealth = 250\nEnd\n\
Behavior = DozerAIUpdate AI\nRepairHealthPercentPerSecond = 2%\nBoredTime = 5000\nBoredRange = 150\nEnd\n\
Behavior = PhysicsBehavior P\nEnd\nEnd\n\
Object Barracks\nKindOf = STRUCTURE IMMOBILE\nBuildCost = 500\nBuildTime = 2.0\nBuildCompletion = PLACED_BY_PLAYER\n\
Geometry = BOX\nGeometryMajorRadius = 30\nGeometryMinorRadius = 30\nGeometryHeight = 20\nEnergyProduction = -1\n\
Body = ActiveBody Body\nMaxHealth = 1000\nInitialHealth = 1000\nEnd\nEnd\n\
Object PowerPlant\nKindOf = STRUCTURE IMMOBILE\nBuildCost = 300\nBuildTime = 1.0\nBuildCompletion = PLACED_BY_PLAYER\n\
Geometry = BOX\nGeometryMajorRadius = 20\nGeometryMinorRadius = 20\nGeometryHeight = 20\nEnergyProduction = 5\n\
Body = ActiveBody Body\nMaxHealth = 800\nInitialHealth = 800\nEnd\nEnd\n\
CommandButton Command_Worker\nCommand = UNIT_BUILD\nObject = Worker\nEnd\n\
CommandSet StashSet\n1 = Command_Worker\nEnd\n\
CommandSet WorkerSet\n1 = Command_Barracks\nEnd\n\
Object Stash\nKindOf = STRUCTURE IMMOBILE CASH_GENERATOR\nCommandSet = StashSet\nGeometryMajorRadius = 20\n\
Behavior = SupplyCenterDockUpdate Dock\nNumberApproachPositions = -1\nEnd\nBehavior = ProductionUpdate PU\nEnd\n\
Behavior = SupplyCenterProductionExitUpdate Exit\nUnitCreatePoint = X:30.0 Y:0.0 Z:0.0\nNaturalRallyPoint = X:60.0 Y:0.0 Z:0.0\nEnd\nEnd\n\
Object Pile\nKindOf = IMMOBILE\nGeometryMajorRadius = 15\n\
Behavior = SupplyWarehouseDockUpdate Dock\nNumberApproachPositions = -1\nStartingBoxes = 20\nEnd\nEnd\n\
Object Worker\nKindOf = INFANTRY DOZER HARVESTER\nCommandSet = WorkerSet\nBuildCost = 200\nBuildTime = 1.0\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\n\
Behavior = WorkerAIUpdate AI\nMaxBoxes = 1\nSupplyCenterActionDelay = 300\nSupplyWarehouseActionDelay = 300\nSupplyWarehouseScanDistance = 500\nBoredTime = 5000\nBoredRange = 100\nEnd\n\
Behavior = PhysicsBehavior P\nEnd\nEnd\n\
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
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&defs)), 9)
}
fn building(s: &Simulation, name: &str) -> Option<u32> {
    s.objects.values().find(|o| o.template == name).map(|o| o.id)
}
fn run(s: &mut Simulation, n: u32) {
    for _ in 0..n {
        s.step();
    }
}

#[test]
fn a_dozer_builds_a_barracks() {
    let mut s = sim();
    let d = s.spawn("Dozer", [200.0, 200.0], 1).unwrap();
    s.enqueue(0, Command::Deposit { player: 1, amount: 1000 }).unwrap();
    s.enqueue(1, Command::Construct { dozer: d, template: "Barracks".into(), position: [400.0, 400.0], angle: 0.0 })
        .unwrap();
    run(&mut s, 2);
    assert_eq!(s.money(1), 500);
    let b = building(&s, "Barracks").expect("placed");
    assert!(s.under_construction(b));
    assert_eq!(s.objects[&b].health.as_ref().unwrap().current, 1.0);
    // The site blocks pathing straight away.
    assert_eq!(s.grid.ground_kind(40, 40), Some(CellType::Obstacle));
    let mut started = None;
    for _ in 0..600 {
        s.step();
        if started.is_none() && s.objects[&b].construction.is_some_and(|p| p > 0.0) {
            started = Some(s.frame);
        }
        if !s.under_construction(b) {
            break;
        }
    }
    let started = started.expect("the dozer started building");
    assert!(!s.under_construction(b), "finished");
    // Two seconds of building at full power: 60 frames of progress.
    assert!((s.frame - started) <= 61, "{} frames", s.frame - started);
    assert_eq!(s.objects[&b].health.as_ref().unwrap().current, 1000.0);
    assert!(s.objects[&b].construction.is_none());
    run(&mut s, 200);
    let p = s.objects[&d].position;
    let dist = ((p[0] - 400.0).powi(2) + (p[1] - 400.0).powi(2)).sqrt();
    assert!(dist > 40.0, "backed away from the building: {p:?}");
    assert!(s.objects[&d].dozer.as_ref().unwrap().tasks.iter().all(|t| t.target.is_none()));
}

#[test]
fn a_site_must_be_clear_and_affordable() {
    let mut s = sim();
    let d = s.spawn("Dozer", [200.0, 200.0], 1).unwrap();
    s.spawn("Barracks", [400.0, 400.0], 2).unwrap();
    s.enqueue(0, Command::Deposit { player: 1, amount: 600 }).unwrap();
    // On top of another building.
    s.enqueue(1, Command::Construct { dozer: d, template: "Barracks".into(), position: [420.0, 420.0], angle: 0.0 })
        .unwrap();
    // Off the map.
    s.enqueue(1, Command::Construct { dozer: d, template: "Barracks".into(), position: [-50.0, 100.0], angle: 0.0 })
        .unwrap();
    run(&mut s, 3);
    assert_eq!(s.money(1), 600);
    assert_eq!(s.events.len(), 2, "{:?}", s.events);
    s.enqueue(3, Command::Construct { dozer: d, template: "Barracks".into(), position: [200.0, 500.0], angle: 0.0 })
        .unwrap();
    s.enqueue(3, Command::Construct { dozer: d, template: "PowerPlant".into(), position: [500.0, 200.0], angle: 0.0 })
        .unwrap();
    run(&mut s, 2);
    // The second order replaces the first task but the first site stays; 100 left is too little for a plant.
    assert_eq!(s.money(1), 100);
    assert!(s.events.last().unwrap().contains("NoMoney"), "{:?}", s.events);
}

#[test]
fn power_counts_once_built_and_construction_resumes() {
    let mut s = sim();
    let d = s.spawn("Dozer", [200.0, 200.0], 1).unwrap();
    s.enqueue(0, Command::Deposit { player: 1, amount: 1000 }).unwrap();
    s.enqueue(1, Command::Construct { dozer: d, template: "PowerPlant".into(), position: [300.0, 200.0], angle: 0.0 })
        .unwrap();
    run(&mut s, 2);
    let p = building(&s, "PowerPlant").unwrap();
    assert_eq!(s.energy(1), (0, 0), "nothing while under construction");
    // Wait until half built, then order the dozer away.
    while s.objects[&p].construction.unwrap_or(0.0) < 50.0 {
        s.step();
    }
    let f = s.frame;
    s.enqueue(f, Command::Move { object: d, target: [100.0, 100.0] }).unwrap();
    run(&mut s, 60);
    let half = s.objects[&p].construction.unwrap();
    assert!((50.0..60.0).contains(&half), "stopped at {half}");
    let f = s.frame;
    s.enqueue(f, Command::ResumeConstruction { dozer: d, target: p }).unwrap();
    for _ in 0..600 {
        s.step();
        if !s.under_construction(p) {
            break;
        }
    }
    assert!(!s.under_construction(p));
    assert_eq!(s.energy(1), (5, 0));
}

#[test]
fn construction_is_deterministic() {
    let run = || {
        let mut s = sim();
        let d = s.spawn("Dozer", [200.0, 200.0], 1).unwrap();
        s.spawn("Dozer", [390.0, 400.0], 1).unwrap();
        s.enqueue(0, Command::Deposit { player: 1, amount: 1000 }).unwrap();
        s.enqueue(
            1,
            Command::Construct { dozer: d, template: "Barracks".into(), position: [400.0, 400.0], angle: 0.5 },
        )
        .unwrap();
        (0..500)
            .map(|_| {
                s.step();
                s.state_hash()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(run(), run());
}

#[test]
fn a_worker_gathers_and_builds() {
    let mut s = sim();
    let stash = s.spawn("Stash", [200.0, 200.0], 1).unwrap();
    s.spawn("Pile", [200.0, 420.0], 0).unwrap();
    s.enqueue(0, Command::Deposit { player: 1, amount: 1000 }).unwrap();
    s.enqueue(1, Command::Produce { factory: stash, template: "Worker".into() }).unwrap();
    run(&mut s, 1500);
    // A worker from the supply centre gathers on its own (one box, 100, a trip).
    assert!(s.money(1) > 800 + 200, "{} {:?}", s.money(1), s.events);
    let w = building(&s, "Worker").unwrap();
    let f = s.frame;
    s.enqueue(f, Command::Construct { dozer: w, template: "Barracks".into(), position: [450.0, 250.0], angle: 0.0 })
        .unwrap();
    for _ in 0..1200 {
        s.step();
        if building(&s, "Barracks").is_some_and(|b| !s.under_construction(b)) {
            break;
        }
    }
    let b = building(&s, "Barracks").expect("placed");
    assert!(!s.under_construction(b), "{:?}", s.objects[&b].construction);
    assert_eq!(s.objects[&w].dozer.as_ref().unwrap().worker, Some(false), "acting as a dozer");
    // Sent back to the pile, it gathers again.
    let money = s.money(1);
    let pile = building(&s, "Pile").unwrap();
    let f = s.frame;
    s.enqueue(f, Command::Dock { object: w, target: pile }).unwrap();
    run(&mut s, 1200);
    assert!(s.money(1) > money, "{} {}", s.money(1), money);
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
fn health(s: &Simulation, id: u32) -> f32 {
    s.objects[&id].health.as_ref().unwrap().current
}

#[test]
fn a_dozer_repairs_a_building_two_percent_a_second() {
    let mut s = sim();
    let d = s.spawn("Dozer", [200.0, 200.0], 1).unwrap();
    let b = s.spawn("Barracks", [400.0, 400.0], 1).unwrap();
    damage(&mut s, b, 500.0);
    assert_eq!(health(&s, b), 500.0);
    let f = s.frame;
    s.enqueue(f, Command::Repair { dozer: d, target: b }).unwrap();
    let mut started = None;
    for _ in 0..2400 {
        s.step();
        if started.is_none() && health(&s, b) > 500.0 {
            started = Some(s.frame);
        }
        if health(&s, b) == 1000.0 {
            break;
        }
    }
    let started = started.expect("repairs began");
    assert_eq!(health(&s, b), 1000.0);
    // 20 health a second: 25 seconds for 500.
    let took = s.frame - started;
    assert!((745..=755).contains(&took), "{took} frames");
    // The dozer notices the building is whole on its next update.
    run(&mut s, 1);
    assert!(s.objects[&d].dozer.as_ref().unwrap().tasks.iter().all(|t| t.target.is_none()));
}

#[test]
fn a_bored_dozer_repairs_nearby_buildings_by_itself() {
    let mut s = sim();
    s.spawn("Dozer", [330.0, 400.0], 1).unwrap();
    let b = s.spawn("Barracks", [400.0, 400.0], 1).unwrap();
    let far = s.spawn("PowerPlant", [700.0, 700.0], 1).unwrap();
    damage(&mut s, b, 100.0);
    damage(&mut s, far, 100.0);
    // Bored after five seconds.
    run(&mut s, 140);
    assert_eq!(health(&s, b), 900.0);
    run(&mut s, 600);
    assert_eq!(health(&s, b), 1000.0);
    assert_eq!(health(&s, far), 700.0, "out of the bored range");
}

#[test]
fn selling_takes_four_and_a_half_seconds_and_refunds_half() {
    let mut s = sim();
    let b = s.spawn("Barracks", [400.0, 400.0], 1).unwrap();
    s.enqueue(1, Command::Sell { object: b }).unwrap();
    s.step();
    let sold = s.frame;
    let mut gone = None;
    for _ in 0..400 {
        s.step();
        if !s.objects.contains_key(&b) {
            gone = Some(s.frame);
            break;
        }
    }
    // 1.5 s of scaffold, then 99.9% down to -50% at 100% per 3 s.
    let took = gone.expect("sold") - sold;
    assert!((178..=182).contains(&took), "{took} frames");
    assert_eq!(s.money(1), 250);
    assert_eq!(s.grid.ground_kind(40, 40), Some(CellType::Clear), "its cells are free again");
}
