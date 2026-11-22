//! Crates (`CreateCrateDie`, `CrateCollide`) and radar (`RadarUpgrade`).
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation, templates::Templates};
use std::sync::Arc;

const INI: &[u8] = b"CrateData AlwaysMoney\nCreationChance = 1.0\nCrateObject = MoneyCrate 1.0\nEnd\n\
CrateData EliteOnly\nCreationChance = 1.0\nVeterancyLevel = ELITE\nCrateObject = HealCrate 1.0\nEnd\n\
CrateData TankKillsOnly\nCreationChance = 1.0\nKilledByType = VEHICLE\nCrateObject = UnitCrate 1.0\nEnd\n\
Upgrade Upgrade_Radar\nType = OBJECT\nEnd\n\
Weapon Rifle\nPrimaryDamage = 50\nAttackRange = 100\nDelayBetweenShots = 500\nEnd\n\
Object Soldier\nKindOf = INFANTRY\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\nVisionRange = 150\n\
WeaponSet\nConditions = None\nWeapon = PRIMARY Rifle\nEnd\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nBehavior = DestroyDie Die\nEnd\nEnd\n\
Object Victim\nKindOf = INFANTRY\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\n\
Body = ActiveBody Body\nMaxHealth = 50\nInitialHealth = 50\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\n\
Behavior = CreateCrateDie Crates\nCrateData = AlwaysMoney\nCrateData = EliteOnly\nCrateData = TankKillsOnly\nEnd\n\
Behavior = DestroyDie Die\nEnd\nEnd\n\
Object MoneyCrate\nKindOf = CRATE\nGeometryMajorRadius = 6\nGeometryIsSmall = Yes\n\
Behavior = MoneyCrateCollide Pickup\nForbiddenKindOf = PROJECTILE\nMoneyProvided = 1000\nEnd\n\
Behavior = PhysicsBehavior P\nMass = 75\nEnd\nEnd\n\
Object HealCrate\nKindOf = CRATE\nGeometryMajorRadius = 6\nGeometryIsSmall = Yes\n\
Behavior = HealCrateCollide Pickup\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\n\
Object UnitCrate\nKindOf = CRATE\nGeometryMajorRadius = 6\nGeometryIsSmall = Yes\n\
Behavior = UnitCrateCollide Pickup\nUnitType = Soldier\nUnitCount = 2\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\n\
Object Radar\nKindOf = STRUCTURE IMMOBILE\nGeometryMajorRadius = 10\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = RadarUpgrade Radar\nTriggeredBy = Upgrade_Radar\nEnd\nBehavior = DestroyDie Die\nEnd\nEnd\n\
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
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&defs)), 47)
}
fn run(s: &mut Simulation, n: u32) {
    for _ in 0..n {
        s.step();
    }
}
fn count(s: &Simulation, t: &str) -> usize {
    s.objects.values().filter(|o| o.template == t).count()
}

#[test]
fn a_kill_drops_a_money_crate_that_pays_whoever_drives_over_it() {
    let mut s = sim();
    let a = s.spawn("Soldier", [300.0, 300.0], 1).unwrap();
    let v = s.spawn("Victim", [360.0, 300.0], 2).unwrap();
    s.enqueue(1, Command::Attack { object: a, target: v }).unwrap();
    run(&mut s, 60);
    assert!(!s.objects.contains_key(&v));
    // Only the unconditional crate: the victim was not elite nor killed by a vehicle.
    assert_eq!((count(&s, "MoneyCrate"), count(&s, "HealCrate"), count(&s, "UnitCrate")), (1, 0, 0));
    let c = s.objects.values().find(|o| o.template == "MoneyCrate").unwrap();
    assert_eq!(c.owner, 0, "crates are neutral");
    let at = [c.position[0], c.position[1]];
    let f = s.frame;
    s.enqueue(f, Command::Move { object: a, target: at }).unwrap();
    run(&mut s, 150);
    assert_eq!(count(&s, "MoneyCrate"), 0, "picked up");
    assert_eq!(s.money(1), 1000);
}

#[test]
fn heal_and_unit_crates() {
    let mut s = sim();
    let a = s.spawn("Soldier", [300.0, 300.0], 1).unwrap();
    let hurt = s.spawn("Soldier", [600.0, 600.0], 1).unwrap();
    s.enqueue(
        1,
        Command::Damage {
            object: hurt,
            amount: 60.0,
            damage_type: "EXPLOSION".into(),
            death_type: "NORMAL".into(),
            source: 0,
        },
    )
    .unwrap();
    run(&mut s, 2);
    assert_eq!(s.objects[&hurt].health.as_ref().unwrap().current, 40.0);
    s.spawn("HealCrate", [340.0, 300.0], 0).unwrap();
    s.enqueue(3, Command::Move { object: a, target: [340.0, 300.0] }).unwrap();
    run(&mut s, 80);
    assert_eq!(count(&s, "HealCrate"), 0);
    assert_eq!(s.objects[&hurt].health.as_ref().unwrap().current, 100.0, "every unit of the player heals");
    let before = count(&s, "Soldier");
    s.spawn("UnitCrate", [300.0, 340.0], 0).unwrap();
    let f = s.frame;
    s.enqueue(f, Command::Move { object: a, target: [300.0, 340.0] }).unwrap();
    run(&mut s, 80);
    assert_eq!(count(&s, "UnitCrate"), 0);
    assert_eq!(count(&s, "Soldier"), before + 2);
}

#[test]
fn a_radar_upgrade_gives_the_player_radar_until_the_building_goes() {
    let mut s = sim();
    let r = s.spawn("Radar", [300.0, 300.0], 1).unwrap();
    assert!(!s.has_radar(1));
    s.give_object_upgrade(r, "Upgrade_Radar");
    assert!(s.has_radar(1));
    s.enqueue(
        1,
        Command::Damage {
            object: r,
            amount: 500.0,
            damage_type: "EXPLOSION".into(),
            death_type: "NORMAL".into(),
            source: 0,
        },
    )
    .unwrap();
    run(&mut s, 3);
    assert!(!s.objects.contains_key(&r));
    assert!(!s.has_radar(1));
}

#[test]
fn crates_are_deterministic() {
    let hashes = || {
        let mut s = sim();
        let a = s.spawn("Soldier", [300.0, 300.0], 1).unwrap();
        let v = s.spawn("Victim", [360.0, 300.0], 2).unwrap();
        s.enqueue(1, Command::Attack { object: a, target: v }).unwrap();
        (0..120)
            .map(|_| {
                s.step();
                s.state_hash()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(hashes(), hashes());
}
