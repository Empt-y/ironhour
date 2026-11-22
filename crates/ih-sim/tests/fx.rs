//! FX lists as logic events (`fx`).
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation, templates::Templates};
use std::sync::Arc;

const INI: &[u8] = b"Weapon Rifle\nPrimaryDamage = 50\nAttackRange = 100\nDelayBetweenShots = 500\n\
FireFX = FX_Rifle\nVeterancyFireFX = HEROIC FX_RifleHero\nSuspendFXDelay = 2000\nEnd\n\
Object Soldier\nKindOf = INFANTRY\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\nVisionRange = 150\n\
IsTrainable = Yes\nExperienceRequired = 0 50 100 200\n\
WeaponSet\nConditions = None\nWeapon = PRIMARY Rifle\nEnd\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nBehavior = DestroyDie Die\nEnd\nEnd\n\
Object Victim\nKindOf = INFANTRY\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\n\
Body = ActiveBody Body\nMaxHealth = 500\nInitialHealth = 500\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\n\
Behavior = FXListDie Boom\nDeathFX = FX_Boom\nOrientToObject = Yes\nEnd\n\
Behavior = FXListDie Gated\nDeathFX = FX_Never\nInitiallyActive = No\nEnd\n\
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
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&defs)), 53)
}
/// Steps, gathering every FX event (events last one frame).
fn run(s: &mut Simulation, n: u32) -> Vec<ih_sim::fx::FxEvent> {
    let mut out = vec![];
    for _ in 0..n {
        s.step();
        out.extend(s.fx_events.iter().cloned());
    }
    out
}

#[test]
fn weapons_play_their_fire_fx_after_the_suspend_delay() {
    let mut s = sim();
    let a = s.spawn("Soldier", [300.0, 300.0], 1).unwrap();
    let v = s.spawn("Victim", [360.0, 300.0], 2).unwrap();
    s.enqueue(1, Command::Attack { object: a, target: v }).unwrap();
    // SuspendFXDelay: no FX for the first two seconds of the weapon's life.
    let early = run(&mut s, 55);
    assert!(early.iter().all(|e| e.fx != "FX_Rifle"), "{early:?}");
    assert!(s.objects[&v].health.as_ref().unwrap().current < 500.0, "but it fires");
    let later = run(&mut s, 60);
    let shot = later.iter().find(|e| e.fx == "FX_Rifle").expect("fire FX");
    assert_eq!(shot.object, Some(a));
    assert_eq!(shot.allies_only, None);
    assert!((shot.position[0] - 300.0).abs() < 1.0, "played at the shooter");
    assert!((shot.target.unwrap()[0] - 360.0).abs() < 1.0);
    // A hero uses its own FX.
    let f = s.frame;
    s.enqueue(f, Command::SetVeterancy { object: a, level: 3 }).unwrap();
    s.enqueue(f, Command::Attack { object: a, target: v }).unwrap();
    let hero = run(&mut s, 90);
    assert!(hero.iter().any(|e| e.fx == "FX_RifleHero"));
}

#[test]
fn death_fx_plays_unless_its_upgrade_is_missing() {
    let mut s = sim();
    let killer = s.spawn("Soldier", [200.0, 300.0], 1).unwrap();
    let v = s.spawn("Victim", [360.0, 300.0], 2).unwrap();
    s.enqueue(
        1,
        Command::Damage {
            object: v,
            amount: 900.0,
            damage_type: "EXPLOSION".into(),
            death_type: "NORMAL".into(),
            source: killer,
        },
    )
    .unwrap();
    let events = run(&mut s, 3);
    let boom = events.iter().find(|e| e.fx == "FX_Boom").expect("death FX");
    assert_eq!(boom.object, Some(v));
    assert!((boom.target.unwrap()[0] - 200.0).abs() < 1.0, "pointed at the killer");
    assert!(events.iter().all(|e| e.fx != "FX_Never"));
}
