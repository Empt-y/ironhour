//! Veterancy (`ExperienceTracker`) and weapon bonuses (`WeaponBonusSet`).
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation, templates::Templates};
use std::sync::Arc;

const INI: &[u8] = b"GameData\nWeaponBonus = VETERAN DAMAGE 150%\nWeaponBonus = ELITE RATE_OF_FIRE 200%\nHealthBonus_Veteran = 120%\nEnd\n\
Weapon Rifle\nPrimaryDamage = 10\nAttackRange = 100\nDelayBetweenShots = 1000\nEnd\n\
Weapon Cannon\nPrimaryDamage = 40\nAttackRange = 100\nDelayBetweenShots = 1000\nEnd\n\
Object Soldier\nKindOf = INFANTRY\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\n\
IsTrainable = Yes\nExperienceValue = 20 40 60 80\nExperienceRequired = 0 50 100 200\n\
WeaponSet\nConditions = None\nWeapon = PRIMARY Rifle\nEnd\nWeaponSet\nConditions = HERO\nWeapon = PRIMARY Cannon\nEnd\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nBehavior = DestroyDie Die\nEnd\nEnd\n\
Object Target\nKindOf = INFANTRY\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\nExperienceValue = 25 25 25 25\n\
Body = ActiveBody Body\nMaxHealth = 10\nInitialHealth = 10\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nBehavior = DestroyDie Die\nEnd\nEnd\n\
Object Wall\nKindOf = STRUCTURE IMMOBILE\nGeometryMajorRadius = 10\n\
Body = ActiveBody Body\nMaxHealth = 1000\nInitialHealth = 1000\nEnd\nEnd\n\
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
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&defs)), 33)
}
fn run(s: &mut Simulation, n: u32) {
    for _ in 0..n {
        s.step();
    }
}
fn health(s: &Simulation, id: u32) -> (f32, f32) {
    let h = s.objects[&id].health.as_ref().unwrap();
    (h.current, h.max)
}

#[test]
fn kills_earn_experience_and_promotion_brings_its_bonuses() {
    let mut s = sim();
    let a = s.spawn("Soldier", [200.0, 200.0], 1).unwrap();
    for i in 0..2 {
        let t = s.spawn("Target", [260.0, 180.0 + 40.0 * i as f32], 2).unwrap();
        s.enqueue(1 + 60 * i, Command::Attack { object: a, target: t }).unwrap();
    }
    run(&mut s, 200);
    let e = s.objects[&a].experience;
    assert_eq!((e.points, e.level), (50, 1), "two kills of 25: veteran");
    // HealthBonus_Veteran: the maximum grows by a fifth, keeping the ratio.
    let (hp, max) = health(&s, a);
    assert!((max - 120.0).abs() < 0.001 && hp == max, "{hp} of {max}");
    // VETERAN DAMAGE 150%: the wall takes 15 a shot.
    let w = s.spawn("Wall", [260.0, 260.0], 2).unwrap();
    let f = s.frame;
    s.enqueue(f, Command::Attack { object: a, target: w }).unwrap();
    run(&mut s, 31);
    let (hp, _) = health(&s, w);
    assert_eq!((1000.0 - hp) % 15.0, 0.0, "{hp}");
    assert!(hp < 1000.0);
}

#[test]
fn only_enemies_of_a_playable_side_give_experience() {
    let mut s = sim();
    let a = s.spawn("Soldier", [200.0, 200.0], 1).unwrap();
    let neutral = s.spawn("Target", [260.0, 200.0], 0).unwrap();
    s.enqueue(1, Command::Attack { object: a, target: neutral }).unwrap();
    run(&mut s, 100);
    assert!(!s.objects.contains_key(&neutral));
    assert_eq!(s.objects[&a].experience.points, 0);
}

#[test]
fn levels_set_weapon_sets_and_rate_of_fire() {
    let mut s = sim();
    let a = s.spawn("Soldier", [200.0, 200.0], 1).unwrap();
    let w = s.spawn("Wall", [260.0, 200.0], 2).unwrap();
    s.enqueue(1, Command::SetVeterancy { object: a, level: 2 }).unwrap();
    s.enqueue(2, Command::Attack { object: a, target: w }).unwrap();
    // ELITE RATE_OF_FIRE 200%: a shot every half second, not every second.
    run(&mut s, 2 + 91);
    let shots = (1000.0 - health(&s, w).0) / 10.0;
    assert!(shots >= 6.0, "{shots} shots in three seconds");
    s.enqueue(s.frame, Command::SetVeterancy { object: a, level: 3 }).unwrap();
    run(&mut s, 1);
    let weapon = s.objects[&a].weapon_set.weapons[0].as_ref().unwrap().template.clone();
    assert_eq!(weapon, "Cannon", "the HERO weapon set");
}
