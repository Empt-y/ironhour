//! The script engine core.
//!
//! Ported from `ScriptEngine.cpp`: `update` (countdown timers, then each side's
//! loose scripts and active non-subroutine groups in order), `executeScripts`,
//! `executeScript` (active, difficulty, evaluation delay, one-shot, actions or
//! false actions), `evaluateConditions` (OR of ANDs, short-circuit),
//! `executeActions`, `newMap`/`checkConditionsForTeamNames` (scripts with a
//! delay start at a random frame in the first two seconds), the flag, counter
//! and timer conditions and actions, `enableScript`, `disableScript` and
//! `callSubroutine`. Skirmish script assignment follows
//! `SidesList::prepareForMP_or_Skirmish` and `Player::initFromDict`.
//!
//! Conditions and actions not ported yet evaluate false and do nothing; each is
//! counted in [`ScriptEngine::unhandled`] so coverage can be followed in play.
//! Not yet: condition teams (`conditionTeamName`, iterating team instances),
//! UI-interaction flags, sequential scripts, camera fades and the rest of
//! `ScriptActions`/`ScriptConditions`.
use crate::Simulation;
use ih_formats::scripts::{Call, Script, ScriptList};
use std::collections::{BTreeMap, BTreeSet};
pub mod building;
pub mod teams;
mod world;

/// `GameDifficulty`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Difficulty {
    Easy,
    #[default]
    Normal,
    Hard,
}

/// A script with its run-time state.
#[derive(Clone, Debug)]
pub struct LiveScript {
    pub script: Script,
    pub active: bool,
    /// `m_frameToEvaluate`.
    pub next_frame: u32,
}

/// A group with its run-time state.
#[derive(Clone, Debug)]
pub struct LiveGroup {
    pub name: String,
    pub active: bool,
    pub subroutine: bool,
    pub scripts: Vec<LiveScript>,
}

/// One side's scripts: its player, difficulty, loose scripts and groups.
#[derive(Clone, Debug)]
pub struct Side {
    pub player: u32,
    pub difficulty: Difficulty,
    pub scripts: Vec<LiveScript>,
    pub groups: Vec<LiveGroup>,
}

/// A counter or countdown timer (`TCounter`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counter {
    pub value: i32,
    pub timer: bool,
}

/// Where a script lives: (side, group or `None` for loose, index).
type ScriptRef = (usize, Option<usize>, usize);

/// `TheScriptEngine`.
#[derive(Clone, Debug, Default)]
pub struct ScriptEngine {
    pub sides: Vec<Side>,
    /// By name, as the original allocates them on first use.
    pub counters: BTreeMap<String, Counter>,
    pub flags: BTreeMap<String, bool>,
    /// Unported conditions and actions met, with how often.
    pub unhandled: BTreeMap<String, u64>,
    pub players: Vec<SkirmishPlayer>,
    pub player_names: BTreeMap<String, u32>,
    pub object_lists: BTreeMap<String, BTreeSet<String>>,
    pub object_counts: BTreeMap<(u32, String), i32>,
    pub bases: BTreeMap<u32, building::Base>,
    pub prototypes: BTreeMap<String, teams::Prototype>,
    pub teams: BTreeMap<u32, teams::Team>,
    pub next_team: u32,
    pub current_team: Option<u32>,
    pub attack_priorities: BTreeMap<String, BTreeMap<String, i32>>,
    /// The side whose scripts are running (`m_currentPlayer`).
    current: Option<usize>,
}

fn live(s: &Script) -> LiveScript {
    LiveScript { script: s.clone(), active: s.active, next_frame: 0 }
}

impl Side {
    pub fn new(player: u32, difficulty: Difficulty, list: &ScriptList) -> Self {
        Side {
            player,
            difficulty,
            scripts: list.scripts.iter().map(live).collect(),
            groups: list
                .groups
                .iter()
                .map(|g| LiveGroup {
                    name: g.name.clone(),
                    active: g.active,
                    subroutine: g.subroutine,
                    scripts: g.scripts.iter().map(live).collect(),
                })
                .collect(),
        }
    }
}

fn name_of(c: &Call) -> &str {
    c.name.as_deref().unwrap_or("")
}
fn param_str(c: &Call, i: usize) -> &str {
    c.params.get(i).map_or("", |p| p.string.as_str())
}
fn param_int(c: &Call, i: usize) -> i32 {
    c.params.get(i).map_or(0, |p| p.int)
}
fn param_real(c: &Call, i: usize) -> f32 {
    c.params.get(i).map_or(0.0, |p| p.real)
}
/// `ConvertDurationFromMsecsToFrames`, rounded up (`REAL_TO_INT_CEIL`).
fn secs_to_frames(seconds: f32) -> i32 {
    (seconds * 1000.0 * 30.0 / 1000.0).ceil() as i32
}

impl ScriptEngine {
    pub fn new(sides: Vec<Side>) -> Self {
        Self { sides, ..Default::default() }
    }

    pub fn with_players(mut self, players: Vec<SkirmishPlayer>) -> Self {
        self.player_names.extend(players.iter().map(|p| (p.name.clone(), p.player)));
        self.players = players;
        self
    }

    fn player_named(&self, name: &str) -> Option<u32> {
        match name {
            "<This Player>" => self.current.map(|s| self.sides[s].player),
            "<Local Player>" => self.players.iter().find(|p| p.ai.is_none()).map(|p| p.player),
            "<This Player's Enemy>" => {
                let current = self.current.map(|s| self.sides[s].player)?;
                self.players.iter().find(|p| p.player != current).map(|p| p.player)
            }
            _ => self.player_names.get(name).copied(),
        }
    }

    fn scripts(&self) -> Vec<ScriptRef> {
        let mut out = vec![];
        for (si, side) in self.sides.iter().enumerate() {
            out.extend((0..side.scripts.len()).map(|i| (si, None, i)));
            for (gi, g) in side.groups.iter().enumerate() {
                out.extend((0..g.scripts.len()).map(|i| (si, Some(gi), i)));
            }
        }
        out
    }
    fn get(&self, r: ScriptRef) -> &LiveScript {
        let side = &self.sides[r.0];
        match r.1 {
            Some(g) => &side.groups[g].scripts[r.2],
            None => &side.scripts[r.2],
        }
    }
    fn get_mut(&mut self, r: ScriptRef) -> &mut LiveScript {
        let side = &mut self.sides[r.0];
        match r.1 {
            Some(g) => &mut side.groups[g].scripts[r.2],
            None => &mut side.scripts[r.2],
        }
    }
    /// `findScript`: the first script of that name on any side.
    fn find_script(&self, name: &str) -> Option<ScriptRef> {
        self.scripts().into_iter().find(|&r| self.get(r).script.name == name)
    }
    /// `findGroup`.
    fn find_group(&self, name: &str) -> Option<(usize, usize)> {
        self.sides
            .iter()
            .enumerate()
            .find_map(|(si, s)| s.groups.iter().position(|g| g.name == name).map(|gi| (si, gi)))
    }
}

impl Simulation {
    /// `ScriptEngine::newMap`: scripts with an evaluation delay start at a random
    /// frame in the first two seconds, drawn in side and script order.
    pub fn start_scripts(&mut self, engine: ScriptEngine) {
        let mut engine = engine;
        for (id, name) in crate::map_objects::players(&self.map).into_iter().enumerate() {
            engine.player_names.insert(name, id as u32);
        }
        for r in engine.scripts() {
            let delay = engine.get(r).script.delay_seconds;
            engine.get_mut(r).next_frame = if delay > 0 { self.rng.integer(0, 2 * 30) as u32 } else { 0 };
        }
        self.script_engine = Some(engine);
    }

    /// `ScriptEngine::update`.
    pub(crate) fn update_scripts(&mut self) {
        let Some(mut engine) = self.script_engine.take() else { return };
        for c in engine.counters.values_mut() {
            if c.timer && c.value >= 0 {
                c.value -= 1;
            }
        }
        for si in 0..engine.sides.len() {
            engine.current = Some(si);
            for i in 0..engine.sides[si].scripts.len() {
                if !engine.sides[si].scripts[i].script.subroutine {
                    self.execute_script(&mut engine, (si, None, i));
                }
            }
            for gi in 0..engine.sides[si].groups.len() {
                let g = &engine.sides[si].groups[gi];
                if !g.active || g.subroutine {
                    continue;
                }
                for i in 0..engine.sides[si].groups[gi].scripts.len() {
                    if !engine.sides[si].groups[gi].scripts[i].script.subroutine {
                        self.execute_script(&mut engine, (si, Some(gi), i));
                    }
                }
            }
            engine.current = None;
        }
        self.update_script_buildings(&mut engine);
        self.update_script_gatherers(&engine);
        self.update_script_teams(&mut engine);
        self.script_engine = Some(engine);
    }

    /// `executeScript`.
    fn execute_script(&mut self, engine: &mut ScriptEngine, r: ScriptRef) {
        let s = engine.get(r);
        if !s.active {
            return;
        }
        // The running side's difficulty (the side the script belongs to when called).
        let side = engine.current.unwrap_or(r.0);
        let d = engine.sides[side].difficulty;
        let allowed = match d {
            Difficulty::Easy => s.script.difficulty[0],
            Difficulty::Normal => s.script.difficulty[1],
            Difficulty::Hard => s.script.difficulty[2],
        };
        if !allowed || self.frame < s.next_frame {
            return;
        }
        if s.script.delay_seconds > 0 {
            let next = self.frame + s.script.delay_seconds as u32 * 30;
            engine.get_mut(r).next_frame = next;
        }
        let script = engine.get(r).script.clone();
        let one_shot = script.one_shot;
        if self.evaluate_conditions(engine, &script) {
            self.execute_actions(engine, &script.actions);
            if one_shot {
                engine.get_mut(r).active = false;
            }
        } else if !script.false_actions.is_empty() {
            self.execute_actions(engine, &script.false_actions);
            if one_shot {
                engine.get_mut(r).active = false;
            }
        }
    }

    /// `evaluateConditions`: any OR term whose conditions all hold.
    fn evaluate_conditions(&mut self, engine: &mut ScriptEngine, script: &Script) -> bool {
        script
            .conditions
            .iter()
            .filter(|ands| !ands.is_empty())
            .any(|ands| ands.iter().all(|c| self.evaluate_condition(engine, c)))
    }

    fn evaluate_condition(&mut self, engine: &mut ScriptEngine, c: &Call) -> bool {
        if let Some(result) = self.script_world_condition(engine, c) {
            return result;
        }
        match name_of(c) {
            "CONDITION_TRUE" => true,
            "CONDITION_FALSE" => false,
            "FLAG" => {
                let flag = *engine.flags.entry(param_str(c, 0).to_string()).or_default();
                flag == (param_int(c, 1) != 0)
            }
            "COUNTER" => {
                let v = engine.counters.entry(param_str(c, 0).to_string()).or_default().value;
                compare(v, param_int(c, 1), param_int(c, 2))
            }
            "TIMER_EXPIRED" => {
                let t = *engine.counters.entry(param_str(c, 0).to_string()).or_default();
                t.timer && t.value < 1
            }
            other => {
                *engine
                    .unhandled
                    .entry(format!("condition {}", if other.is_empty() { "#?" } else { other }))
                    .or_default() += 1;
                false
            }
        }
    }

    /// `executeActions`.
    fn execute_actions(&mut self, engine: &mut ScriptEngine, actions: &[Call]) {
        for a in actions {
            if self.script_world_action(engine, a) {
                continue;
            }
            match name_of(a) {
                "NO_OP" => {}
                "SET_FLAG" => {
                    engine.flags.insert(param_str(a, 0).to_string(), param_int(a, 1) != 0);
                }
                "SET_COUNTER" => {
                    engine.counters.entry(param_str(a, 0).to_string()).or_default().value = param_int(a, 1)
                }
                "INCREMENT_COUNTER" => {
                    engine.counters.entry(param_str(a, 1).to_string()).or_default().value += param_int(a, 0)
                }
                "DECREMENT_COUNTER" => {
                    engine.counters.entry(param_str(a, 1).to_string()).or_default().value -= param_int(a, 0)
                }
                "SET_TIMER" | "SET_RANDOM_TIMER" | "SET_MILLISECOND_TIMER" | "SET_RANDOM_MSEC_TIMER" => {
                    let name = name_of(a);
                    let value = if name.contains("MSEC") || name == "SET_MILLISECOND_TIMER" {
                        let mut v = param_real(a, 1);
                        if name == "SET_RANDOM_MSEC_TIMER" {
                            v = self.rng.real(v, param_real(a, 2));
                        }
                        secs_to_frames(v)
                    } else {
                        let mut v = param_int(a, 1);
                        if name == "SET_RANDOM_TIMER" {
                            v = self.rng.integer(v, param_int(a, 2));
                        }
                        v
                    };
                    *engine.counters.entry(param_str(a, 0).to_string()).or_default() = Counter { value, timer: true };
                }
                "STOP_TIMER" => engine.counters.entry(param_str(a, 0).to_string()).or_default().timer = false,
                "RESTART_TIMER" => {
                    let c = engine.counters.entry(param_str(a, 0).to_string()).or_default();
                    if c.value > 0 {
                        c.timer = true;
                    }
                }
                "ADD_TO_MSEC_TIMER" | "SUB_FROM_MSEC_TIMER" => {
                    let mut v = secs_to_frames(param_real(a, 0));
                    if name_of(a) == "SUB_FROM_MSEC_TIMER" {
                        v = -secs_to_frames(param_real(a, 0));
                    }
                    engine.counters.entry(param_str(a, 1).to_string()).or_default().value += v;
                }
                "ENABLE_SCRIPT" | "DISABLE_SCRIPT" => {
                    let on = name_of(a) == "ENABLE_SCRIPT";
                    let n = param_str(a, 0).to_string();
                    if let Some((si, gi)) = engine.find_group(&n) {
                        engine.sides[si].groups[gi].active = on;
                    }
                    if let Some(r) = engine.find_script(&n) {
                        engine.get_mut(r).active = on;
                    }
                }
                "CALL_SUBROUTINE" => {
                    let n = param_str(a, 0).to_string();
                    if let Some((si, gi)) = engine.find_group(&n) {
                        let g = &engine.sides[si].groups[gi];
                        if g.subroutine && g.active {
                            for i in 0..g.scripts.len() {
                                if !engine.sides[si].groups[gi].scripts[i].script.subroutine {
                                    self.execute_script(engine, (si, Some(gi), i));
                                }
                            }
                        }
                    } else if let Some(r) = engine.find_script(&n)
                        && engine.get(r).script.subroutine
                    {
                        self.execute_script(engine, r);
                    }
                }
                other => {
                    *engine
                        .unhandled
                        .entry(format!("action {}", if other.is_empty() { "#?" } else { other }))
                        .or_default() += 1;
                }
            }
        }
    }
}

/// `Parameter::COMPARISON`: less, less or equal, equal, greater or equal, greater, not equal.
fn compare(a: i32, op: i32, b: i32) -> bool {
    match op {
        0 => a < b,
        1 => a <= b,
        2 => a == b,
        3 => a >= b,
        4 => a > b,
        5 => a != b,
        _ => false,
    }
}

/// A skirmish player for script assignment.
#[derive(Clone, Debug)]
pub struct SkirmishPlayer {
    pub player: u32,
    /// The `PlayerTemplate`'s `Side` (`AmericaAirForceGeneral`).
    pub side: String,
    /// The start position index (`multiplayerStartIndex`), the qualifier.
    pub start: u32,
    /// `None` for a human player.
    pub ai: Option<Difficulty>,
    /// The player's name, which side parameters are renamed to.
    pub name: String,
}

/// `SidesList::prepareForMP_or_Skirmish` and `Player::initFromDict`: the map's
/// civilian side keeps its scripts. When no other map side has scripts, the
/// skirmish scripts file supplies each skirmish side's list (by player name);
/// each AI player gets a qualified copy of the list of the first skirmish side
/// whose faction has its `Side`, and so does each human player for the civilian
/// side's list (`Civilian`).
///
/// `faction_side` gives a `PlayerTemplate`'s `Side`; `civilian` is the map's
/// civilian player id.
pub fn skirmish_sides(
    map: &ih_formats::map::Map,
    scb: Option<&ih_formats::scripts::ScbFile>,
    players: &[SkirmishPlayer],
    faction_side: &dyn Fn(&str) -> Option<String>,
    civilian: u32,
) -> Vec<Side> {
    // The map's sides (in file order) with their names, factions and scripts.
    let mut skirmish: Vec<(String, String, Option<ScriptList>)> = map
        .sides
        .sides
        .iter()
        .enumerate()
        .map(|(i, s)| {
            (
                s.dict.ascii("playerName").unwrap_or_default().to_string(),
                s.dict.ascii("playerFaction").unwrap_or_default().to_string(),
                map.scripts.get(i).cloned(),
            )
        })
        .collect();
    let has_scripts =
        |l: &Option<ScriptList>| l.as_ref().is_some_and(|l| !l.scripts.is_empty() || !l.groups.is_empty());
    let got = skirmish.iter().any(|(_, f, l)| f != "FactionCivilian" && has_scripts(l));
    if !got && let Some(scb) = scb {
        for (i, list) in scb.lists.iter().enumerate() {
            let Some(p) = scb.players.get(i) else { continue };
            if let Some(side) = skirmish.iter_mut().find(|(n, _, _)| *n == p.name) {
                side.2 = Some(list.clone());
            }
        }
    }
    let mut out = vec![];
    if let Some((_, _, Some(list))) = skirmish.iter().find(|(_, f, _)| f == "FactionCivilian") {
        out.push(Side::new(civilian, Difficulty::Normal, list));
    }
    for p in players {
        let wanted = if p.ai.is_some() { p.side.clone() } else { "Civilian".to_string() };
        let Some((name, _, Some(list))) =
            skirmish.iter().find(|(_, f, l)| l.is_some() && faction_side(f).as_deref() == Some(wanted.as_str()))
        else {
            continue;
        };
        let q = p.start.to_string();
        let template_player = format!("{name}{q}");
        let list = list.qualified(&q, &template_player, &p.name);
        out.push(Side::new(p.player, p.ai.unwrap_or_default(), &list));
    }
    out
}
