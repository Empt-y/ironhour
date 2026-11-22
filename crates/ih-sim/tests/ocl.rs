//! Object creation lists (`ObjectCreationList`) and `LifetimeUpdate`.
use ih_formats::map::Map;
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation, templates::Templates};
use std::sync::Arc;

const INI: &[u8] = b"ObjectCreationList OCL_Survivors\nCreateObject\nObjectNames = Pilot Unused\nCount = 2\nDisposition = SEND_IT_OUT\nDispositionIntensity = 1.0\nEnd\nEnd\n\
ObjectCreationList OCL_Debris\nCreateDebris\nModelNames = ChunkA ChunkB\nCount = 3\nDisposition = SEND_IT_FLYING\nDispositionIntensity = 2.0\nMinLifetime = 1000\nMaxLifetime = 2000\nMass = 5\nEnd\nEnd\n\
ObjectCreationList OCL_Flare\nCreateObject\nObjectNames = Flare\nDisposition = LIKE_EXISTING\nOffset = X:10.0 Y:0.0 Z:0.0\nEnd\nEnd\n\
Weapon FlareGun\nPrimaryDamage = 5\nAttackRange = 100\nDelayBetweenShots = 1000\nFireOCL = OCL_Flare\nEnd\n\
Object Tank\nKindOf = VEHICLE\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 8\n\
WeaponSet\nConditions = None\nWeapon = PRIMARY FlareGun\nEnd\n\
Body = ActiveBody Body\nMaxHealth = 100\nInitialHealth = 100\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\n\
Behavior = CreateObjectDie Survivors\nCreationList = OCL_Survivors\nEnd\n\
Behavior = CreateObjectDie Debris\nCreationList = OCL_Debris\nEnd\n\
Behavior = DestroyDie Die\nEnd\nEnd\n\
Object Pilot\nKindOf = INFANTRY\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 3\n\
Body = ActiveBody Body\nMaxHealth = 50\nInitialHealth = 50\nEnd\n\
Behavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\n\
Object Unused\nKindOf = INFANTRY\nEnd\n\
Object GenericDebris\nKindOf = NO_COLLIDE\nBody = InactiveBody Body\nEnd\n\
Behavior = PhysicsBehavior P\nMass = 1\nEnd\nBehavior = LifetimeUpdate Life\nMinLifetime = 99000\nMaxLifetime = 99000\nEnd\n\
Behavior = DestroyDie Die\nEnd\nEnd\n\
Object Flare\nKindOf = NO_COLLIDE\nBody = InactiveBody Body\nEnd\nBehavior = LifetimeUpdate Life\nMinLifetime = 500\nMaxLifetime = 500\nEnd\nBehavior = DestroyDie Die\nEnd\nEnd\n\
Object Wall\nKindOf = STRUCTURE IMMOBILE\nGeometryMajorRadius = 10\nBody = ActiveBody Body\nMaxHealth = 1000\nInitialHealth = 1000\nEnd\nEnd\n\
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
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&defs)), 35)
}
fn count(s: &Simulation, t: &str) -> usize {
    s.objects.values().filter(|o| o.template == t).count()
}
fn run(s: &mut Simulation, n: u32) {
    for _ in 0..n {
        s.step();
    }
}

#[test]
fn a_dying_tank_leaves_survivors_and_debris_that_expires() {
    let mut s = sim();
    let t = s.spawn("Tank", [300.0, 300.0], 1).unwrap();
    s.enqueue(
        1,
        Command::Damage {
            object: t,
            amount: 500.0,
            damage_type: "EXPLOSION".into(),
            death_type: "NORMAL".into(),
            source: 0,
        },
    )
    .unwrap();
    run(&mut s, 3);
    assert!(!s.objects.contains_key(&t));
    // Only every other name counts: two pilots, no "Unused".
    assert_eq!(count(&s, "Pilot"), 2);
    assert_eq!(count(&s, "Unused"), 0);
    assert_eq!(count(&s, "GenericDebris"), 3);
    for o in s.objects.values().filter(|o| o.template == "Pilot") {
        assert_eq!(o.owner, 1, "survivors keep the owner");
    }
    // Debris lives one to two seconds (the OCL's range replaces the template's).
    for o in s.objects.values().filter(|o| o.template == "GenericDebris") {
        let left = o.die_frame.unwrap() - s.frame;
        assert!((25..=62).contains(&left), "{left}");
    }
    run(&mut s, 70);
    assert_eq!(count(&s, "GenericDebris"), 0, "expired");
}

#[test]
fn a_weapon_fire_ocl_spawns_at_the_shooter() {
    let mut s = sim();
    let t = s.spawn("Tank", [300.0, 300.0], 1).unwrap();
    let w = s.spawn("Wall", [360.0, 300.0], 2).unwrap();
    s.enqueue(1, Command::Attack { object: t, target: w }).unwrap();
    run(&mut s, 10);
    let flare = s.objects.values().find(|o| o.template == "Flare").expect("a flare");
    // LIKE_EXISTING with a 10-unit offset along the tank's facing.
    let tank = &s.objects[&t];
    let d = ((flare.position[0] - tank.position[0]).powi(2) + (flare.position[1] - tank.position[1]).powi(2)).sqrt();
    assert!((d - 10.0).abs() < 1.0, "{d}");
    run(&mut s, 20);
    assert!(count(&s, "Flare") <= 1, "flares last half a second");
}

#[test]
fn ocls_are_deterministic() {
    let run_hashes = || {
        let mut s = sim();
        let t = s.spawn("Tank", [300.0, 300.0], 1).unwrap();
        s.enqueue(
            5,
            Command::Damage {
                object: t,
                amount: 500.0,
                damage_type: "EXPLOSION".into(),
                death_type: "NORMAL".into(),
                source: 0,
            },
        )
        .unwrap();
        (0..120)
            .map(|_| {
                s.step();
                s.state_hash()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(run_hashes(), run_hashes());
}
