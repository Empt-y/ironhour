//! Upgrades (`UpgradeTemplate`, `UpgradeMux`, `ProductionUpdate::queueUpgrade`)
//! and the ported upgrade modules.
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation, templates::Templates};
use std::sync::Arc;

const INI: &[u8] = b"GameData\nMinLowEnergyProductionSpeed = 1.0\nMaxLowEnergyProductionSpeed = 1.0\nEnd\n\
Upgrade DefaultUpgrade\nBuildTime = 1.0\nEnd\n\
Upgrade Upgrade_Armor\nType = PLAYER\nBuildCost = 300\nBuildTime = 2.0\nEnd\n\
Upgrade Upgrade_Gun\nType = PLAYER\nBuildCost = 100\nEnd\n\
Upgrade Upgrade_Walls\nType = OBJECT\nBuildCost = 400\nEnd\n\
Upgrade Upgrade_Cheap\nType = OBJECT\nBuildCost = 0\nEnd\n\
CommandButton Command_Soldier\nCommand = UNIT_BUILD\nObject = Soldier\nEnd\n\
CommandButton Command_Armor\nCommand = PLAYER_UPGRADE\nUpgrade = Upgrade_Armor\nEnd\n\
CommandButton Command_Gun\nCommand = PLAYER_UPGRADE\nUpgrade = Upgrade_Gun\nEnd\n\
CommandButton Command_Walls\nCommand = OBJECT_UPGRADE\nUpgrade = Upgrade_Walls\nEnd\n\
CommandButton Command_Cheap\nCommand = OBJECT_UPGRADE\nUpgrade = Upgrade_Cheap\nEnd\n\
CommandSet BarracksSet\n1 = Command_Soldier\n2 = Command_Armor\n3 = Command_Gun\n4 = Command_Walls\n5 = Command_Cheap\nEnd\n\
Armor Plain\nArmor = DEFAULT 100%\nEnd\nArmor Plated\nArmor = DEFAULT 50%\nEnd\n\
Weapon Rifle\nPrimaryDamage = 10\nAttackRange = 100\nEnd\nWeapon BigRifle\nPrimaryDamage = 20\nAttackRange = 100\nEnd\n\
Object Barracks\nKindOf = STRUCTURE IMMOBILE\nCommandSet = BarracksSet\n\
Body = ActiveBody Body\nMaxHealth = 1000\nInitialHealth = 1000\nEnd\n\
Behavior = ProductionUpdate PU\nEnd\n\
Behavior = DefaultProductionExitUpdate Exit\nUnitCreatePoint = X:30.0 Y:0.0 Z:0.0\nNaturalRallyPoint = X:60.0 Y:0.0 Z:0.0\nEnd\n\
Behavior = MaxHealthUpgrade Walls\nTriggeredBy = Upgrade_Walls\nAddMaxHealth = 500\nChangeType = ADD_CURRENT_HEALTH_TOO\nEnd\n\
Behavior = CostModifierUpgrade Cheap\nTriggeredBy = Upgrade_Cheap\nEffectKindOf = INFANTRY\nPercentage = -25%\nEnd\n\
Behavior = DestroyDie Die\nEnd\nEnd\n\
Object Soldier\nKindOf = INFANTRY\nBuildCost = 200\nBuildTime = 2.0\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\n\
ArmorSet\nConditions = None\nArmor = Plain\nEnd\nArmorSet\nConditions = PLAYER_UPGRADE\nArmor = Plated\nEnd\n\
WeaponSet\nConditions = None\nWeapon = PRIMARY Rifle\nEnd\nWeaponSet\nConditions = PLAYER_UPGRADE\nWeapon = PRIMARY BigRifle\nEnd\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\n\
Behavior = ArmorUpgrade Armor\nTriggeredBy = Upgrade_Armor\nEnd\n\
Behavior = WeaponSetUpgrade Gun\nTriggeredBy = Upgrade_Gun\nEnd\n\
Behavior = DestroyDie Die\nEnd\nEnd\n\
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
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&defs)), 5)
}
fn health(s: &Simulation, id: u32) -> (f32, f32) {
    let h = s.objects[&id].health.as_ref().unwrap();
    (h.current, h.max)
}
fn run(s: &mut Simulation, frames: u32) {
    for _ in 0..frames {
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
fn a_player_upgrade_reaches_every_unit_including_later_ones() {
    let mut s = sim();
    let b = s.spawn("Barracks", [300.0, 300.0], 1).unwrap();
    let old = s.spawn("Soldier", [500.0, 300.0], 1).unwrap();
    let enemy = s.spawn("Soldier", [500.0, 500.0], 2).unwrap();
    s.enqueue(0, Command::Deposit { player: 1, amount: 1000 }).unwrap();
    s.enqueue(1, Command::Research { factory: b, upgrade: "Upgrade_Armor".into() }).unwrap();
    run(&mut s, 2);
    assert_eq!(s.money(1), 700);
    assert!(s.players[&1].upgrades.in_progress.contains("Upgrade_Armor"));
    // Researching it twice at once is refused.
    s.enqueue(s.frame, Command::Research { factory: b, upgrade: "Upgrade_Armor".into() }).unwrap();
    run(&mut s, 1);
    assert_eq!(s.money(1), 700);
    run(&mut s, 60);
    assert!(s.players[&1].upgrades.completed.contains("Upgrade_Armor"));
    damage(&mut s, old, 10.0);
    damage(&mut s, enemy, 10.0);
    assert_eq!(health(&s, old).0, 95.0, "plated");
    assert_eq!(health(&s, enemy).0, 90.0, "the other player has no upgrade");
    // A soldier made afterwards has the armour from the start.
    s.enqueue(s.frame, Command::Produce { factory: b, template: "Soldier".into() }).unwrap();
    run(&mut s, 70);
    let new = s.objects.values().find(|o| o.template == "Soldier" && o.id > enemy).unwrap().id;
    damage(&mut s, new, 10.0);
    assert_eq!(health(&s, new).0, 95.0);
}

#[test]
fn a_weapon_set_upgrade_switches_weapons() {
    let mut s = sim();
    let b = s.spawn("Barracks", [300.0, 300.0], 1).unwrap();
    let u = s.spawn("Soldier", [500.0, 300.0], 1).unwrap();
    let weapon = |s: &Simulation| s.objects[&u].weapon_set.weapons[0].as_ref().unwrap().template.clone();
    assert_eq!(weapon(&s), "Rifle");
    s.enqueue(0, Command::Deposit { player: 1, amount: 100 }).unwrap();
    s.enqueue(1, Command::Research { factory: b, upgrade: "Upgrade_Gun".into() }).unwrap();
    // DefaultUpgrade's one-second build time: queued on frame 1, done on frame 30.
    run(&mut s, 30);
    assert_eq!(weapon(&s), "Rifle");
    run(&mut s, 1);
    assert_eq!(weapon(&s), "BigRifle");
}

#[test]
fn an_object_upgrade_changes_only_its_building_and_cancelling_refunds() {
    let mut s = sim();
    let a = s.spawn("Barracks", [300.0, 300.0], 1).unwrap();
    let b = s.spawn("Barracks", [300.0, 600.0], 1).unwrap();
    s.enqueue(0, Command::Deposit { player: 1, amount: 1000 }).unwrap();
    s.enqueue(1, Command::Research { factory: a, upgrade: "Upgrade_Walls".into() }).unwrap();
    s.enqueue(1, Command::Research { factory: b, upgrade: "Upgrade_Walls".into() }).unwrap();
    run(&mut s, 2);
    assert_eq!(s.money(1), 200);
    s.enqueue(s.frame, Command::CancelResearch { factory: b, upgrade: "Upgrade_Walls".into() }).unwrap();
    run(&mut s, 40);
    assert_eq!(s.money(1), 600, "refunded");
    assert_eq!(health(&s, a), (1500.0, 1500.0));
    assert_eq!(health(&s, b), (1000.0, 1000.0));
    // Once done it cannot be researched there again.
    s.enqueue(s.frame, Command::Research { factory: a, upgrade: "Upgrade_Walls".into() }).unwrap();
    run(&mut s, 1);
    assert_eq!(s.money(1), 600);
}

#[test]
fn a_cost_modifier_lasts_as_long_as_its_building() {
    let mut s = sim();
    let a = s.spawn("Barracks", [300.0, 300.0], 1).unwrap();
    let soldier = s.templates.objects["Soldier"].clone();
    assert_eq!(s.cost_to_build(&soldier, 1), 200);
    s.enqueue(1, Command::Research { factory: a, upgrade: "Upgrade_Cheap".into() }).unwrap();
    run(&mut s, 40);
    assert_eq!(s.cost_to_build(&soldier, 1), 150);
    assert_eq!(s.cost_to_build(&soldier, 2), 200);
    s.enqueue(s.frame, Command::Destroy { object: a }).unwrap();
    run(&mut s, 1);
    assert_eq!(s.cost_to_build(&soldier, 1), 200);
}

#[test]
fn upgrades_are_deterministic() {
    let run = || {
        let mut s = sim();
        let b = s.spawn("Barracks", [300.0, 300.0], 1).unwrap();
        s.enqueue(0, Command::Deposit { player: 1, amount: 2000 }).unwrap();
        for u in ["Upgrade_Armor", "Upgrade_Gun", "Upgrade_Walls"] {
            s.enqueue(1, Command::Research { factory: b, upgrade: u.into() }).unwrap();
        }
        s.enqueue(1, Command::Produce { factory: b, template: "Soldier".into() }).unwrap();
        (0..400)
            .map(|_| {
                s.step();
                s.state_hash()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(run(), run());
}
