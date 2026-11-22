//! Sciences, ranks, skill points and special powers.
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation, templates::Templates};
use std::sync::Arc;

const INI: &[u8] = b"Rank 1\nSkillPointsNeeded = 0\nSciencesGranted = SCIENCE_Rank1\nSciencePurchasePointsGranted = 1\nEnd\n\
Rank 2\nSkillPointsNeeded = 100\nSciencesGranted = SCIENCE_Rank2\nSciencePurchasePointsGranted = 1\nEnd\n\
Rank 3\nSkillPointsNeeded = 300\nSciencesGranted = SCIENCE_Rank3\nSciencePurchasePointsGranted = 3\nEnd\n\
Science SCIENCE_Rank1\nSciencePurchasePointCost = 0\nIsGrantable = No\nEnd\n\
Science SCIENCE_Rank2\nPrerequisiteSciences = SCIENCE_Rank1\nSciencePurchasePointCost = 0\nIsGrantable = No\nEnd\n\
Science SCIENCE_Rank3\nPrerequisiteSciences = SCIENCE_Rank2\nSciencePurchasePointCost = 0\nIsGrantable = No\nEnd\n\
Science SCIENCE_Drop\nPrerequisiteSciences = SCIENCE_Rank1\nSciencePurchasePointCost = 1\nEnd\n\
Science SCIENCE_Bounty\nPrerequisiteSciences = SCIENCE_Rank3\nSciencePurchasePointCost = 3\nEnd\n\
SpecialPower SuperweaponDrop\nReloadTime = 10000\nRequiredScience = SCIENCE_Drop\nEnd\n\
SpecialPower SuperweaponSync\nReloadTime = 20000\nSharedSyncedTimer = Yes\nEnd\n\
SpecialPower SuperweaponHack\nReloadTime = 5000\nEnd\n\
SpecialPower SuperweaponBounty\nReloadTime = 0\nRequiredScience = SCIENCE_Bounty\nEnd\n\
ObjectCreationList OCL_Crate\nCreateObject\nObjectNames = Crate\nDisposition = LIKE_EXISTING\nEnd\nEnd\n\
Weapon Rifle\nPrimaryDamage = 50\nAttackRange = 100\nDelayBetweenShots = 500\nEnd\n\
Object Command\nKindOf = STRUCTURE IMMOBILE\nGeometryMajorRadius = 10\nBuildCost = 2000\n\
Body = ActiveBody Body\nMaxHealth = 1000\nInitialHealth = 1000\nEnd\n\
Behavior = OCLSpecialPower Drop\nSpecialPowerTemplate = SuperweaponDrop\nOCL = OCL_Crate\nCreateLocation = CREATE_AT_LOCATION\nEnd\n\
Behavior = OCLSpecialPower Sync\nSpecialPowerTemplate = SuperweaponSync\nOCL = OCL_Crate\nCreateLocation = CREATE_AT_LOCATION\nEnd\n\
Behavior = CashHackSpecialPower Hack\nSpecialPowerTemplate = SuperweaponHack\nMoneyAmount = 300\nEnd\n\
Behavior = CashBountyPower Bounty\nSpecialPowerTemplate = SuperweaponBounty\nBounty = 50%\nEnd\nEnd\n\
Object Crate\nKindOf = NO_COLLIDE\nEnd\n\
Object Soldier\nKindOf = INFANTRY\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\nVisionRange = 150\n\
WeaponSet\nConditions = None\nWeapon = PRIMARY Rifle\nEnd\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nBehavior = DestroyDie Die\nEnd\nEnd\n\
Object Victim\nKindOf = INFANTRY\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\nBuildCost = 400\n\
SkillPointValue = 150 150 150 150\n\
Body = ActiveBody Body\nMaxHealth = 50\nInitialHealth = 50\nEnd\n\
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
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&defs)), 43)
}
fn run(s: &mut Simulation, n: u32) {
    for _ in 0..n {
        s.step();
    }
}
fn kill(s: &mut Simulation, shooter: u32, at: [f32; 2]) {
    let v = s.spawn("Victim", at, 2).unwrap();
    let f = s.frame;
    s.enqueue(f, Command::Attack { object: shooter, target: v }).unwrap();
    run(s, 60);
    assert!(!s.objects.contains_key(&v));
}
fn crates(s: &Simulation) -> usize {
    s.objects.values().filter(|o| o.template == "Crate").count()
}

#[test]
fn kills_raise_the_generals_rank_and_buy_sciences() {
    let mut s = sim();
    let a = s.spawn("Soldier", [300.0, 300.0], 1).unwrap();
    let g = s.player(1).generals.clone();
    assert_eq!((g.rank, g.purchase_points), (1, 1));
    assert!(s.player(1).sciences.contains("SCIENCE_Rank1"));
    kill(&mut s, a, [360.0, 300.0]);
    let g = s.player(1).generals.clone();
    assert_eq!((g.rank, g.skill_points, g.purchase_points), (2, 150, 2), "150 points: rank 2");
    kill(&mut s, a, [360.0, 320.0]);
    let g = s.player(1).generals.clone();
    assert_eq!((g.rank, g.skill_points, g.purchase_points), (3, 300, 5));
    assert!(s.player(1).sciences.contains("SCIENCE_Rank3"));
    // Buying: prerequisites and points.
    let f = s.frame;
    s.enqueue(f, Command::PurchaseScience { player: 1, science: "SCIENCE_Drop".into() }).unwrap();
    s.enqueue(f, Command::PurchaseScience { player: 1, science: "SCIENCE_Bounty".into() }).unwrap();
    s.enqueue(f, Command::PurchaseScience { player: 1, science: "SCIENCE_Bounty".into() }).unwrap();
    run(&mut s, 1);
    assert!(s.player(1).sciences.contains("SCIENCE_Drop") && s.player(1).sciences.contains("SCIENCE_Bounty"));
    assert_eq!(s.player(1).generals.purchase_points, 1);
    assert_eq!(s.events.len(), 1, "the second purchase is refused: {:?}", s.events);
}

#[test]
fn a_special_power_needs_its_science_and_reloads() {
    let mut s = sim();
    let c = s.spawn("Command", [300.0, 300.0], 1).unwrap();
    let drop = |s: &mut Simulation| {
        let f = s.frame;
        s.enqueue(
            f,
            Command::SpecialPower {
                object: c,
                power: "SuperweaponDrop".into(),
                target: None,
                location: Some([500.0, 500.0]),
            },
        )
        .unwrap();
        run(s, 1);
    };
    drop(&mut s);
    assert_eq!(crates(&s), 0, "no science yet");
    s.player(1).generals.purchase_points = 1;
    let f = s.frame;
    s.enqueue(f, Command::PurchaseScience { player: 1, science: "SCIENCE_Drop".into() }).unwrap();
    run(&mut s, 1);
    // Gaining the science makes the power ready at once.
    drop(&mut s);
    assert_eq!(crates(&s), 1);
    let crate_pos = s.objects.values().find(|o| o.template == "Crate").unwrap().position;
    assert!((crate_pos[0] - 500.0).abs() < 1.0 && (crate_pos[1] - 500.0).abs() < 1.0);
    // ReloadTime: ten seconds.
    drop(&mut s);
    assert_eq!(crates(&s), 1);
    run(&mut s, 300);
    drop(&mut s);
    assert_eq!(crates(&s), 2);
}

#[test]
fn shared_timers_span_a_players_buildings() {
    let mut s = sim();
    let a = s.spawn("Command", [300.0, 300.0], 1).unwrap();
    let b = s.spawn("Command", [500.0, 300.0], 1).unwrap();
    let sync = |s: &mut Simulation, o: u32| {
        let f = s.frame;
        s.enqueue(
            f,
            Command::SpecialPower {
                object: o,
                power: "SuperweaponSync".into(),
                target: None,
                location: Some([400.0, 600.0]),
            },
        )
        .unwrap();
        run(s, 1);
    };
    // A shared timer starts ready.
    sync(&mut s, a);
    assert_eq!(crates(&s), 1);
    sync(&mut s, b);
    assert_eq!(crates(&s), 1, "the other building shares the reload");
    run(&mut s, 600);
    sync(&mut s, b);
    assert_eq!(crates(&s), 2);
}

#[test]
fn cash_hacks_steal_and_bounties_pay() {
    let mut s = sim();
    let c = s.spawn("Command", [300.0, 300.0], 1).unwrap();
    let enemy = s.spawn("Command", [700.0, 700.0], 2).unwrap();
    s.player(2).money = 1000;
    run(&mut s, 160);
    let f = s.frame;
    s.enqueue(
        f,
        Command::SpecialPower { object: c, power: "SuperweaponHack".into(), target: Some(enemy), location: None },
    )
    .unwrap();
    run(&mut s, 1);
    assert_eq!((s.money(1), s.money(2)), (300, 700));
    // CashBountyPower: half the victim's cost once the science is there.
    s.player(1).generals.purchase_points = 10;
    s.player(1).sciences.insert("SCIENCE_Rank3".into());
    let f = s.frame;
    s.enqueue(f, Command::PurchaseScience { player: 1, science: "SCIENCE_Bounty".into() }).unwrap();
    run(&mut s, 1);
    assert_eq!(s.player(1).generals.cash_bounty, 0.5);
    let a = s.spawn("Soldier", [300.0, 360.0], 1).unwrap();
    kill(&mut s, a, [360.0, 360.0]);
    assert_eq!(s.money(1), 500);
}

#[test]
fn sciences_are_deterministic() {
    let hashes = || {
        let mut s = sim();
        let a = s.spawn("Soldier", [300.0, 300.0], 1).unwrap();
        let v = s.spawn("Victim", [360.0, 300.0], 2).unwrap();
        s.enqueue(1, Command::Attack { object: a, target: v }).unwrap();
        s.enqueue(80, Command::PurchaseScience { player: 1, science: "SCIENCE_Drop".into() }).unwrap();
        (0..120)
            .map(|_| {
                s.step();
                s.state_hash()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(hashes(), hashes());
}
