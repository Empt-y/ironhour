//! The map's own objects, from `GameLogic::startNewGame` (GameLogic.cpp),
//! `PlayerList::newGame`/`validateTeam` (PlayerList.cpp) and
//! `Pathfinder::classifyObjectFootprint` (AIPathfind.cpp).
//!
//! Which map objects become logic objects follows the original's object pass:
//! road and bridge points belong to the terrain, `OPTIMIZED_TREE`s and `PROP`s
//! are client-only, and so is other `CLEARED_BY_BUILD` fluff without a fence
//! width, because ironhour always takes the multiplayer setting
//! (`forceFluffToProp`, `useTrees`) for self-determinism. Bridge-like templates
//! (`IsBridge`, `WALK_ON_TOP_OF_WALL`) are created earlier in the original;
//! landmark bridges and wall layers are not ported yet, so they are skipped.
use crate::{
    footprint::{Footprint, Shape},
    templates::{Template, Templates},
};
use ih_formats::map::{Map, MapObject, object_flags as of};

/// What `startNewGame` does with a map object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fate {
    /// A road or bridge end point: terrain.
    TerrainPoint,
    /// No thing template (lights, test objects, removed units).
    NoTemplate,
    /// Landmark bridges and wall pieces (pending).
    BridgeLike,
    /// `OPTIMIZED_TREE`: a client drawable only.
    ClientTree,
    /// `PROP`, or `CLEARED_BY_BUILD` fluff without a fence: terrain-visual props.
    Prop,
    /// A logic object.
    Logic,
}

/// `handleNameChange`: renamed templates that maps still use.
pub fn template_name(name: &str) -> &str {
    match name {
        "AmericaTankLeopard" => "AmericaTankCrusader",
        "AmericaVehicleHumVee" => "AmericaVehicleHumvee",
        other => other,
    }
}

fn yes(t: &Template, key: &str) -> bool {
    t.values(key).first().is_some_and(|v| v.eq_ignore_ascii_case("yes") || v.eq_ignore_ascii_case("true"))
}

pub fn fate(o: &MapObject, templates: &Templates) -> Fate {
    if o.flags & (of::BRIDGE_POINT1 | of::BRIDGE_POINT2 | of::ROAD_POINT1 | of::ROAD_POINT2) != 0 {
        return Fate::TerrainPoint;
    }
    let Some(t) = templates.objects.get(template_name(&o.name)) else {
        return Fate::NoTemplate;
    };
    if yes(t, "IsBridge") || t.has_kind("WALK_ON_TOP_OF_WALL") {
        return Fate::BridgeLike;
    }
    // useTrees is always on: SHRUBBERY stays.
    if t.has_kind("OPTIMIZED_TREE") {
        return Fate::ClientTree;
    }
    let fluff = t.has_kind("CLEARED_BY_BUILD") && t.number("FenceWidth", 0.0) == 0.0;
    if t.has_kind("PROP") || fluff {
        return Fate::Prop;
    }
    Fate::Logic
}

/// The player list (`PlayerList::newGame`): the neutral player first, then
/// each side with a player name, in side order. Returns the player names.
pub fn players(map: &Map) -> Vec<String> {
    let mut out = vec![String::new()];
    for side in &map.sides.sides {
        let name = side.dict.ascii("playerName").unwrap_or_default();
        if !name.is_empty() && out.len() < 16 {
            out.push(name.to_string());
        }
    }
    out
}

/// `validateTeam(originalOwner)` followed by the team's owning player: teams are
/// the map's teams plus each player's default team (`team<playerName>`, and
/// `team` for the neutral player). Unknown owners go to the neutral player.
pub fn owner(map: &Map, players: &[String], original_owner: &str) -> u32 {
    let player = |name: &str| players.iter().position(|p| p == name).map_or(0, |i| i as u32);
    if let Some(team) = map.sides.teams.iter().find(|t| t.ascii("teamName") == Some(original_owner)) {
        return player(team.ascii("teamOwner").unwrap_or_default());
    }
    players.iter().position(|p| format!("team{p}") == original_owner).map_or(0, |i| i as u32)
}

/// `classifyObjectFootprint` filters, then the template's footprint. `height` is
/// the object's height above the terrain.
pub fn footprint(t: &Template, position: [f32; 2], angle: f32, height: f32, key: u32) -> Option<Footprint> {
    if t.has_kind("MINE") || t.has_kind("PROJECTILE") || t.has_kind("BRIDGE_TOWER") {
        return None;
    }
    let fence = t.number("FenceWidth", 0.0);
    if fence > 0.0 && !t.has_kind("DEFENSIVE_WALL") {
        let x_offset = t.number("FenceXOffset", 0.0);
        return Some(Footprint { position, angle, shape: Shape::Fence { width: fence, x_offset }, key });
    }
    // Only immobile structures, not small ones, not up in the air.
    if !t.has_kind("STRUCTURE") || !t.has_kind("IMMOBILE") || yes(t, "GeometryIsSmall") {
        return None;
    }
    if height > crate::navigation::CELL && !t.has_kind("BLAST_CRATER") {
        return None;
    }
    // GeometryInfo defaults: a sphere of radius 1.
    let major = t.number("GeometryMajorRadius", 1.0);
    let shape = match t.values("Geometry").first().map(|s| s.to_ascii_uppercase()) {
        Some(g) if g == "BOX" => Shape::Box { major, minor: t.number("GeometryMinorRadius", 1.0) },
        _ => Shape::Circle { radius: major },
    };
    Some(Footprint { position, angle, shape, key })
}

/// A map object that becomes a logic object.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    /// Index into `Map::objects`.
    pub index: usize,
    pub template: String,
    /// x, y and the stored height above the ground.
    pub position: [f32; 3],
    /// Normalised (`normalizeAngle`).
    pub angle: f32,
    pub owner: u32,
}

/// The logic objects of the map, in map order.
pub fn placements(map: &Map, templates: &Templates) -> Vec<Placement> {
    let players = players(map);
    map.objects
        .iter()
        .enumerate()
        .filter(|(_, o)| fate(o, templates) == Fate::Logic)
        .map(|(index, o)| Placement {
            index,
            template: template_name(&o.name).to_string(),
            position: o.position,
            angle: ih_math::normalize_angle(o.angle),
            owner: owner(map, &players, o.properties.ascii("originalOwner").unwrap_or_default()),
        })
        .collect()
}
