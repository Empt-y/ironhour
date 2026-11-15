//! The map's own objects: which become logic objects, who owns them, and the
//! obstacle cells their structures leave in the terrain.
use ih_formats::map::{Dict, DictValue, Map, MapObject, Side, SidesList, object_flags};
use ih_ini::definitions::Definitions;
use ih_sim::{
    Command, Simulation,
    footprint::CellType,
    map_objects::{Fate, fate},
    templates::Templates,
};
use std::sync::Arc;

const INI: &[u8] = b"Object Tank\nKindOf = VEHICLE\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 4\nBehavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\n\
Object Barracks\nKindOf = STRUCTURE IMMOBILE\nGeometry = BOX\nGeometryMajorRadius = 30\nGeometryMinorRadius = 20\nEnd\n\
Object Tree\nKindOf = SHRUBBERY OPTIMIZED_TREE\nEnd\n\
Object Rock\nKindOf = IMMOBILE PROP\nEnd\n\
Object Barrel\nKindOf = IMMOBILE CLEARED_BY_BUILD\nEnd\n\
Object Fence\nKindOf = IMMOBILE CLEARED_BY_BUILD\nFenceWidth = 40\nEnd\n\
Object Shed\nKindOf = STRUCTURE IMMOBILE\nGeometryMajorRadius = 10\nGeometryIsSmall = Yes\nEnd\n\
Locomotor L\nSurfaces = GROUND\nSpeed = 30\nAcceleration = 900\nBraking = 900\nTurnRate = 360\nEnd\n";

fn dict(pairs: &[(&str, &str)]) -> Dict {
    Dict(pairs.iter().map(|(k, v)| (k.to_string(), DictValue::Ascii(v.to_string()))).collect())
}
fn object(name: &str, x: f32, y: f32, owner: &str) -> MapObject {
    MapObject {
        position: [x, y, 0.0],
        angle: 0.0,
        flags: 0,
        name: name.into(),
        properties: dict(&[("originalOwner", owner)]),
    }
}

fn setup() -> (Arc<Map>, Arc<Templates>) {
    let mut defs = Definitions::default();
    defs.add_document(ih_ini::parse(INI));
    let mut road = object("Barracks", 10.0, 10.0, "team");
    road.flags = object_flags::ROAD_POINT1;
    let map = Map {
        width: 44,
        height: 44,
        border: 2,
        boundaries: vec![[40, 40]],
        heights: vec![0; 44 * 44],
        objects: vec![
            object("Barracks", 200.0, 200.0, "teamCivilian"),
            object("Tree", 50.0, 50.0, "team"),
            object("Rock", 60.0, 50.0, "team"),
            object("Barrel", 70.0, 50.0, "team"),
            object("Fence", 300.0, 100.0, "team"),
            object("Shed", 100.0, 300.0, "teamplayer1"),
            road,
            object("Missing", 0.0, 0.0, "team"),
            object("Tank", 350.0, 350.0, "nobody"),
        ],
        sides: SidesList {
            sides: vec![
                Side { dict: dict(&[("playerName", "")]), build_list: vec![] },
                Side { dict: dict(&[("playerName", "player1")]), build_list: vec![] },
                Side { dict: dict(&[("playerName", "civilian")]), build_list: vec![] },
            ],
            teams: vec![dict(&[("teamName", "teamCivilian"), ("teamOwner", "civilian")])],
        },
        ..Default::default()
    };
    (Arc::new(map), Arc::new(Templates::from_definitions(&defs)))
}

#[test]
fn start_new_game_creates_logic_objects_with_owners() {
    let (map, templates) = setup();
    let fates: Vec<Fate> = map.objects.iter().map(|o| fate(o, &templates)).collect();
    assert_eq!(
        fates,
        [
            Fate::Logic,
            Fate::ClientTree,
            Fate::Prop,
            Fate::Prop,
            Fate::Logic,
            Fate::Logic,
            Fate::TerrainPoint,
            Fate::NoTemplate,
            Fate::Logic
        ]
    );
    let mut sim = Simulation::new(map, templates, 1);
    let tank = sim.spawn("Tank", [20.0, 20.0], 1).unwrap();
    assert_eq!(sim.load_map_objects().unwrap(), 4);
    let owners: Vec<(Option<usize>, &str, u32)> =
        sim.objects.values().map(|o| (o.map_index, o.template.as_str(), o.owner)).collect();
    assert_eq!(
        owners,
        [
            (None, "Tank", 1),
            // teamCivilian is owned by "civilian", the third player.
            (Some(0), "Barracks", 2),
            (Some(4), "Fence", 0),
            // A player's default team.
            (Some(5), "Shed", 1),
            // Unknown owners go to the neutral player.
            (Some(8), "Tank", 0),
        ]
    );
    assert_eq!(sim.objects[&tank].id, 1);
}

#[test]
fn structures_leave_obstacle_cells_and_units_route_around_them() {
    let (map, templates) = setup();
    let mut sim = Simulation::new(map, templates, 1);
    let tank = sim.spawn("Tank", [200.0, 150.0], 1).unwrap();
    sim.load_map_objects().unwrap();
    let o = sim.grid.obstacles.clone();
    let at = |x: i32, y: i32| o.kind[(y * 40 + x) as usize];
    // The 60 x 40 barracks box covers cells 17..=22 by 18..=21 (with the +0.5 offset).
    assert_eq!(at(20, 20), CellType::Obstacle);
    assert_eq!(at(17, 18), CellType::Obstacle);
    assert_eq!(at(23, 20), CellType::Clear);
    assert!(o.pinched[(20 * 40 + 23) as usize], "border cells are pinched");
    // The fence is an obstacle; the small shed is not.
    assert!(o.fence[(10 * 40 + 30) as usize]);
    assert_eq!(at(10, 30), CellType::Clear);
    // A tank ordered through the barracks goes around it and arrives.
    sim.enqueue(0, Command::Move { object: tank, target: [200.0, 250.0] }).unwrap();
    for _ in 0..600 {
        sim.step();
        let p = sim.objects[&tank].position;
        let (cx, cy) = ((p[0] / 10.0).floor() as i32, (p[1] / 10.0).floor() as i32);
        assert_ne!(at(cx, cy), CellType::Obstacle, "tank entered the barracks at {p:?}");
    }
    let p = sim.objects[&tank].position;
    assert!(ih_math::length([p[0] - 200.0, p[1] - 250.0]) < 5.0, "tank at {p:?}");
    assert!(sim.overlaps().is_empty());
}
