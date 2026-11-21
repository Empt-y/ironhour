//! Dumb projectiles (`DumbProjectileBehavior`): the shell is an object that
//! arcs to its target and detonates there, or on something in the way.
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation, templates::Templates};
use std::sync::Arc;

const INI: &[u8] = b"Weapon Cannon\nPrimaryDamage = 40\nPrimaryDamageRadius = 5\nAttackRange = 300\nWeaponSpeed = 300\nProjectileObject = Shell\nDelayBetweenShots = 3000\nEnd\n\
Object Shell\nKindOf = PROJECTILE\nGeometryMajorRadius = 1\n\
Body = ActiveBody Body\nMaxHealth = 1\nInitialHealth = 1\nEnd\n\
Behavior = PhysicsBehavior P\nEnd\n\
Behavior = DumbProjectileBehavior D\nFirstHeight = 20\nSecondHeight = 20\nFirstPercentIndent = 30%\nSecondPercentIndent = 70%\nEnd\n\
Behavior = DestroyDie Die\nEnd\nEnd\n\
Object Tank\nKindOf = VEHICLE\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 5\n\
WeaponSet\nConditions = None\nWeapon = PRIMARY Cannon\nEnd\n\
Body = ActiveBody Body\nMaxHealth = 1000\nInitialHealth = 1000\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\n\
Object Soldier\nKindOf = INFANTRY\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\n\
Object Pillbox\nKindOf = STRUCTURE IMMOBILE\nGeometry = CYLINDER\nGeometryMajorRadius = 12\nGeometryHeight = 60\n\
Body = ActiveBody Body\nMaxHealth = 500\nInitialHealth = 500\nEnd\nEnd\n\
GameData\nPartitionCellSize = 40\nEnd\n\
Locomotor L\nSurfaces = GROUND\nSpeed = 30\nAcceleration = 900\nBraking = 900\nTurnRate = 360\nEnd\n";

fn sim() -> Simulation {
    let mut defs = Definitions::default();
    defs.add_document(ih_ini::parse(INI));
    let map = Map {
        width: 64,
        height: 64,
        border: 2,
        boundaries: vec![[60, 60]],
        heights: vec![0; 64 * 64],
        ..Default::default()
    };
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&defs)), 1)
}
fn health(s: &Simulation, id: u32) -> f32 {
    s.objects.get(&id).and_then(|o| o.health.as_ref()).map_or(0.0, |h| h.current)
}

#[test]
fn a_shell_arcs_to_its_victim_and_detonates_there() {
    let mut a = sim();
    let mut b = sim();
    for s in [&mut a, &mut b] {
        s.spawn("Tank", [100.0, 100.0], 1).unwrap();
        s.spawn("Soldier", [300.0, 100.0], 2).unwrap();
        s.enqueue(1, Command::Attack { object: 1, target: 2 }).unwrap();
    }
    let (mut peak, mut flew, mut hit) = (0.0f32, 0, None);
    for _ in 0..60 {
        a.step();
        b.step();
        assert_eq!(a.state_hash(), b.state_hash());
        if let Some(shell) = a.objects.values().find(|o| o.template == "Shell") {
            peak = peak.max(shell.position[2]);
            flew += 1;
        }
        if hit.is_none() && health(&a, 2) < 100.0 {
            hit = Some(a.frame - 1);
        }
    }
    assert!(peak > 10.0, "the shell arcs above the ground: {peak}");
    // About 200 units at 10 a frame, along a slightly longer arc.
    assert!((20..=24).contains(&flew), "flight frames {flew}");
    assert_eq!(health(&a, 2), 60.0, "one detonation of 40");
    assert!(hit.is_some());
    assert!(!a.objects.values().any(|o| o.template == "Shell"), "the shell is gone");
}

#[test]
fn a_structure_in_the_way_takes_the_hit() {
    let mut s = sim();
    s.spawn("Tank", [100.0, 100.0], 1).unwrap();
    s.spawn("Soldier", [300.0, 100.0], 2).unwrap();
    // Tall enough that the arc cannot clear it.
    let pillbox = s.spawn("Pillbox", [200.0, 100.0], 2).unwrap();
    s.enqueue(1, Command::Attack { object: 1, target: 2 }).unwrap();
    for _ in 0..40 {
        s.step();
    }
    assert_eq!(health(&s, 2), 100.0, "the soldier is untouched");
    assert!(health(&s, pillbox) < 500.0, "the pillbox was hit");
}
