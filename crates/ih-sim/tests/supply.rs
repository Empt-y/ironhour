//! Supply gathering (`SupplyWarehouseDockUpdate`, `SupplyCenterDockUpdate`,
//! `AIDockMachine`, `SupplyTruckAIUpdate`). No models here, so the docks have
//! no bones and trucks dock where they stand next to the building.
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation, supply::TruckState, templates::Templates};
use std::sync::Arc;

const INI: &[u8] = b"GameData\nBaseValuePerSupplyBox = 75\nMinLowEnergyProductionSpeed = 1.0\nMaxLowEnergyProductionSpeed = 1.0\nEnd\n\
CommandButton Command_Truck\nCommand = UNIT_BUILD\nObject = Truck\nEnd\n\
CommandSet CenterSet\n1 = Command_Truck\nEnd\n\
Object Center\nKindOf = STRUCTURE IMMOBILE CASH_GENERATOR\nCommandSet = CenterSet\nGeometryMajorRadius = 20\n\
Behavior = SupplyCenterDockUpdate Dock\nNumberApproachPositions = -1\nEnd\n\
Behavior = ProductionUpdate PU\nEnd\n\
Behavior = SupplyCenterProductionExitUpdate Exit\nUnitCreatePoint = X:30.0 Y:0.0 Z:0.0\nNaturalRallyPoint = X:60.0 Y:0.0 Z:0.0\nEnd\nEnd\n\
Object Warehouse\nKindOf = STRUCTURE IMMOBILE\nGeometryMajorRadius = 20\n\
Behavior = SupplyWarehouseDockUpdate Dock\nNumberApproachPositions = -1\nStartingBoxes = 3\nDeleteWhenEmpty = Yes\nEnd\nEnd\n\
Object Truck\nKindOf = VEHICLE HARVESTER\nBuildCost = 600\nBuildTime = 1.0\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 5\n\
Behavior = SupplyTruckAIUpdate AI\nMaxBoxes = 2\nSupplyCenterActionDelay = 500\nSupplyWarehouseActionDelay = 500\nSupplyWarehouseScanDistance = 500\nEnd\n\
Behavior = PhysicsBehavior P\nEnd\nEnd\n\
Locomotor L\nSurfaces = GROUND\nSpeed = 40\nAcceleration = 900\nBraking = 900\nTurnRate = 360\nEnd\n";

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
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&defs)), 3)
}

#[test]
fn a_truck_ferries_boxes_until_the_warehouse_is_empty() {
    let mut s = sim();
    s.spawn("Center", [200.0, 200.0], 1).unwrap();
    let w = s.spawn("Warehouse", [450.0, 200.0], 0).unwrap();
    let t = s.spawn("Truck", [300.0, 200.0], 1).unwrap();
    s.enqueue(1, Command::Dock { object: t, target: w }).unwrap();
    let mut money = vec![];
    for _ in 0..3000 {
        s.step();
        if money.last() != Some(&s.money(1)) {
            money.push(s.money(1));
        }
    }
    // Two boxes (150), then the last one (75); the empty warehouse is deleted.
    assert_eq!(money, vec![0, 150, 225], "{:?}", s.events);
    assert!(!s.objects.contains_key(&w));
    let truck = s.objects[&t].truck.as_ref().unwrap();
    assert_eq!(truck.boxes, 0);
    assert!(s.objects[&t].dock.is_none());
}

#[test]
fn a_truck_from_the_supply_center_starts_gathering_by_itself() {
    let mut s = sim();
    let c = s.spawn("Center", [200.0, 200.0], 1).unwrap();
    s.spawn("Warehouse", [200.0, 420.0], 0).unwrap();
    s.enqueue(0, Command::Deposit { player: 1, amount: 600 }).unwrap();
    s.enqueue(1, Command::Produce { factory: c, template: "Truck".into() }).unwrap();
    for _ in 0..2500 {
        s.step();
    }
    // All three boxes, in two trips.
    assert_eq!(s.money(1), 225, "{:?}", s.events);
    // With the warehouse gone the truck waits by the supply centre, still on autopilot.
    let truck = s.objects.values().find(|o| o.template == "Truck").unwrap();
    assert!(
        matches!(truck.truck.as_ref().unwrap().state, TruckState::Regrouping | TruckState::Wanting),
        "{:?}",
        truck.truck
    );
}

#[test]
fn a_stopped_truck_stays_put() {
    let mut s = sim();
    s.spawn("Center", [200.0, 200.0], 1).unwrap();
    let w = s.spawn("Warehouse", [450.0, 200.0], 0).unwrap();
    let t = s.spawn("Truck", [300.0, 200.0], 1).unwrap();
    s.enqueue(1, Command::Dock { object: t, target: w }).unwrap();
    s.enqueue(60, Command::Stop { object: t }).unwrap();
    for _ in 0..1500 {
        s.step();
    }
    assert_eq!(s.money(1), 0);
    assert_eq!(s.objects[&t].truck.as_ref().unwrap().state, TruckState::Idle);
    // The warehouse slot it held was given back.
    assert!(s.objects[&w].dock.as_ref().unwrap().owners.iter().all(Option::is_none));
}

#[test]
fn supply_gathering_is_deterministic() {
    let run = || {
        let mut s = sim();
        let c = s.spawn("Center", [200.0, 200.0], 1).unwrap();
        s.spawn("Warehouse", [450.0, 260.0], 0).unwrap();
        s.enqueue(0, Command::Deposit { player: 1, amount: 1200 }).unwrap();
        for _ in 0..2 {
            s.enqueue(1, Command::Produce { factory: c, template: "Truck".into() }).unwrap();
        }
        (0..2000)
            .map(|_| {
                s.step();
                s.state_hash()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(run(), run());
}
