//! Bodies, armour, damage and death (`ActiveBody`, the body variants,
//! `ArmorTemplate`, `DestroyDie`, `SlowDeathBehavior`, `AIDeadState`).
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{
    Command, Simulation,
    ai::AiState,
    body::{DAMAGE_UNRESISTABLE, DEATH_NORMAL, DamageInfo, DamageState, damage_type, death_type},
    templates::Templates,
};
use std::sync::Arc;

const INI: &[u8] = b"Armor TankArmor\nArmor = Default 50%\nArmor = SMALL_ARMS 10%\nArmor = CRUSH 0%\nEnd\n\
Object Tank\nKindOf = VEHICLE\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 4\n\
ArmorSet\nConditions = None\nArmor = TankArmor\nEnd\n\
Body = ActiveBody Body\nMaxHealth = 200\nInitialHealth = 200\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nBehavior = DestroyDie Die\nDeathTypes = ALL -CRUSHED\nEnd\nEnd\n\
Object Hero\nKindOf = INFANTRY\nBody = HighlanderBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\nEnd\n\
Object Ghost\nBody = ImmortalBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\nEnd\n\
Object Rock\nKindOf = IMMOBILE\nBody = InactiveBody Body\nEnd\nBehavior = DestroyDie Die\nEnd\nEnd\n\
Object Soldier\nKindOf = INFANTRY\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\n\
Behavior = SlowDeathBehavior Death1\nProbabilityModifier = 10\nSinkDelay = 1000\nSinkRate = 3\nDestructionDelay = 3000\nEnd\n\
Behavior = SlowDeathBehavior Death2\nDeathTypes = NONE +EXPLODED\nProbabilityModifier = 50\nDestructionDelay = 500\nEnd\nEnd\n\
Locomotor L\nSurfaces = GROUND\nSpeed = 30\nAcceleration = 900\nBraking = 900\nTurnRate = 360\nEnd\n";

fn sim() -> Simulation {
    let mut defs = Definitions::default();
    defs.add_document(ih_ini::parse(INI));
    let map = Map {
        width: 44,
        height: 44,
        border: 2,
        boundaries: vec![[40, 40]],
        heights: vec![0; 44 * 44],
        ..Default::default()
    };
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&defs)), 1)
}
fn health(s: &Simulation, id: u32) -> f32 {
    s.objects[&id].health.as_ref().unwrap().current
}
fn hit(s: &mut Simulation, id: u32, kind: &str, amount: f32) -> DamageInfo {
    s.attempt_damage(id, DamageInfo::new(0, damage_type(kind).unwrap(), DEATH_NORMAL, amount))
}

#[test]
fn armour_scales_damage_and_damage_states_follow_health() {
    let mut s = sim();
    let tank = s.spawn("Tank", [100.0, 100.0], 1).unwrap();
    // SMALL_ARMS at 10%, everything else at the default 50%, unresistable in full.
    assert_eq!(hit(&mut s, tank, "SMALL_ARMS", 100.0).dealt, 10.0);
    assert_eq!(health(&s, tank), 190.0);
    assert_eq!(s.estimate_damage(tank, damage_type("EXPLOSION").unwrap(), 100.0), 50.0);
    hit(&mut s, tank, "EXPLOSION", 200.0);
    assert_eq!(health(&s, tank), 90.0);
    // 90 / 200 is under the 50% UnitDamagedThreshold.
    assert_eq!(s.objects[&tank].health.as_ref().unwrap().state, DamageState::Damaged);
    s.attempt_healing(tank, 500.0, 0);
    assert_eq!(health(&s, tank), 200.0, "healing caps at the maximum");
    assert_eq!(s.objects[&tank].health.as_ref().unwrap().state, DamageState::Pristine);
    let info = hit(&mut s, tank, "UNRESISTABLE", 150.0);
    assert_eq!((info.dealt, info.clipped), (150.0, 150.0));
    assert_eq!(s.objects[&tank].health.as_ref().unwrap().state, DamageState::Damaged);
    hit(&mut s, tank, "UNRESISTABLE", 31.0);
    assert_eq!(s.objects[&tank].health.as_ref().unwrap().state, DamageState::ReallyDamaged);
}

#[test]
fn destroy_die_removes_the_object_at_the_end_of_the_frame_for_its_death_types() {
    let mut s = sim();
    let tank = s.spawn("Tank", [100.0, 100.0], 1).unwrap();
    let other = s.spawn("Tank", [200.0, 100.0], 1).unwrap();
    s.kill(tank, DAMAGE_UNRESISTABLE, DEATH_NORMAL);
    assert!(s.objects[&tank].dead, "dead at once");
    assert!(s.objects.contains_key(&tank), "removed only at the end of the frame");
    s.step();
    assert!(!s.objects.contains_key(&tank));
    // CRUSHED is not among this DestroyDie's death types: dead, but not destroyed.
    s.kill(other, DAMAGE_UNRESISTABLE, death_type("CRUSHED").unwrap());
    s.step();
    assert!(s.objects.contains_key(&other) && s.objects[&other].dead);
    // The dead take no orders.
    s.enqueue(s.frame, Command::Move { object: other, target: [300.0, 100.0] }).unwrap();
    s.step();
    assert_eq!(s.events.len(), 1, "{:?}", s.events);
}

#[test]
fn highlander_immortal_and_inactive_bodies() {
    let mut s = sim();
    let hero = s.spawn("Hero", [100.0, 100.0], 1).unwrap();
    let ghost = s.spawn("Ghost", [150.0, 100.0], 1).unwrap();
    let rock = s.spawn("Rock", [200.0, 100.0], 1).unwrap();
    hit(&mut s, hero, "EXPLOSION", 1000.0);
    hit(&mut s, ghost, "EXPLOSION", 1000.0);
    assert_eq!((health(&s, hero), health(&s, ghost)), (1.0, 1.0));
    // Unresistable damage still kills a highlander, never an immortal.
    hit(&mut s, hero, "UNRESISTABLE", 1000.0);
    hit(&mut s, ghost, "UNRESISTABLE", 1000.0);
    assert!(s.objects[&hero].dead && !s.objects[&ghost].dead);
    // An inactive body ignores damage until it is unresistable, which kills it.
    assert!(hit(&mut s, rock, "EXPLOSION", 1000.0).no_effect);
    s.step();
    assert!(s.objects.contains_key(&rock));
    hit(&mut s, rock, "UNRESISTABLE", 1.0);
    s.step();
    assert!(!s.objects.contains_key(&rock));
}

#[test]
fn slow_death_rolls_a_module_sinks_the_body_and_destroys_it() {
    let mut a = sim();
    let mut b = sim();
    for s in [&mut a, &mut b] {
        let soldier = s.spawn("Soldier", [100.0, 100.0], 1).unwrap();
        s.enqueue(1, Command::Move { object: soldier, target: [300.0, 100.0] }).unwrap();
        s.enqueue(
            20,
            Command::Damage {
                object: soldier,
                amount: 500.0,
                damage_type: "EXPLOSION".into(),
                death_type: "NORMAL".into(),
                source: 0,
            },
        )
        .unwrap();
    }
    let mut sank = false;
    let mut gone = None;
    for _ in 0..200 {
        a.step();
        b.step();
        assert_eq!(a.state_hash(), b.state_hash());
        match a.objects.get(&1) {
            Some(o) if a.frame > 21 => {
                assert_eq!(o.ai.state, AiState::Dead);
                assert!(o.target.is_none());
                sank |= o.position[2] < -1.0;
            }
            None if gone.is_none() => gone = Some(a.frame),
            _ => {}
        }
    }
    // Only Death1 applies to a NORMAL death: it sinks after 30 frames and is
    // destroyed 90 frames after death.
    assert!(sank, "the body sank");
    assert_eq!(gone, Some(21 + 90));
}
