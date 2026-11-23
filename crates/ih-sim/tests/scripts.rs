//! The script engine core (`ScriptEngine.cpp`).
use ih_formats::map::{Dict, DictValue, Map};
use ih_formats::scripts::{Call, Parameter, Script, ScriptGroup, ScriptList};
use ih_ini::definitions::Definitions;
use ih_sim::script_engine::{Difficulty, ScriptEngine, Side, SkirmishPlayer, skirmish_sides};
use ih_sim::{Simulation, templates::Templates};
use std::sync::Arc;

fn sim() -> Simulation {
    let map = Map {
        width: 44,
        height: 44,
        border: 2,
        boundaries: vec![[40, 40]],
        heights: vec![0; 44 * 44],
        ..Default::default()
    };
    Simulation::new(Arc::new(map), Arc::new(Templates::from_definitions(&Definitions::default())), 7)
}
fn s(text: &str) -> Parameter {
    Parameter { kind: 0, int: 0, real: 0.0, string: text.into(), coord: None }
}
fn i(v: i32) -> Parameter {
    Parameter { kind: 0, int: v, real: 0.0, string: String::new(), coord: None }
}
fn r(v: f32) -> Parameter {
    Parameter { kind: 1, int: 0, real: v, string: String::new(), coord: None }
}
fn call(name: &str, params: Vec<Parameter>) -> Call {
    Call { index: 0, name: Some(name.into()), params }
}
fn script(name: &str, conditions: Vec<Call>, actions: Vec<Call>) -> Script {
    Script {
        name: name.into(),
        active: true,
        difficulty: [true, true, true],
        conditions: vec![conditions],
        actions,
        ..Default::default()
    }
}
fn run(sim: &mut Simulation, n: u32) {
    for _ in 0..n {
        sim.step();
    }
}
fn engine(sim: &Simulation) -> &ScriptEngine {
    sim.script_engine.as_ref().unwrap()
}

#[test]
fn flags_counters_timers_one_shots_and_disabling() {
    let mut start = script(
        "Start",
        vec![call("CONDITION_TRUE", vec![])],
        vec![
            call("SET_COUNTER", vec![s("Waves"), i(3)]),
            call("SET_MILLISECOND_TIMER", vec![s("Clock"), r(1.0)]),
            call("SET_FLAG", vec![s("Ready"), i(1)]),
        ],
    );
    start.one_shot = true;
    let tick = script(
        "Tick",
        vec![call("TIMER_EXPIRED", vec![s("Clock")]), call("FLAG", vec![s("Ready"), i(1)])],
        vec![call("INCREMENT_COUNTER", vec![i(1), s("Waves")]), call("DISABLE_SCRIPT", vec![s("Tick")])],
    );
    let list = ScriptList { scripts: vec![start, tick], groups: vec![] };
    let mut sim = sim();
    sim.start_scripts(ScriptEngine::new(vec![Side::new(1, Difficulty::Normal, &list)]));
    run(&mut sim, 2);
    let e = engine(&sim);
    assert_eq!(e.counters["Waves"].value, 3);
    assert!(e.flags["Ready"] && !e.sides[0].scripts[0].active, "the one-shot ran once");
    // A one-second timer (30 frames) expires, then Tick runs once and turns itself off.
    run(&mut sim, 40);
    let e = engine(&sim);
    assert_eq!(e.counters["Waves"].value, 4);
    assert!(!e.sides[0].scripts[1].active);
}

#[test]
fn difficulty_delays_subroutines_and_unported_entries() {
    let mut hard_only =
        script("HardOnly", vec![call("CONDITION_TRUE", vec![])], vec![call("SET_FLAG", vec![s("Hard"), i(1)])]);
    hard_only.difficulty = [false, false, true];
    let mut slow =
        script("Slow", vec![call("CONDITION_TRUE", vec![])], vec![call("INCREMENT_COUNTER", vec![i(1), s("Slow")])]);
    slow.delay_seconds = 2;
    let caller = script(
        "Caller",
        vec![call("COUNTER", vec![s("Slow"), i(3), i(1)])],
        vec![call("CALL_SUBROUTINE", vec![s("Sub")]), call("BUILD_SOMETHING_UNPORTED", vec![])],
    );
    let sub = ScriptGroup {
        name: "Sub".into(),
        active: true,
        subroutine: true,
        scripts: vec![script(
            "InSub",
            vec![call("CONDITION_TRUE", vec![])],
            vec![call("INCREMENT_COUNTER", vec![i(1), s("Called")])],
        )],
    };
    let list = ScriptList { scripts: vec![hard_only, slow, caller], groups: vec![sub] };
    let mut sim = sim();
    sim.start_scripts(ScriptEngine::new(vec![Side::new(1, Difficulty::Easy, &list)]));
    run(&mut sim, 300);
    let e = engine(&sim);
    assert!(!e.flags.contains_key("Hard"), "not at easy");
    // Every two seconds after a random start in the first two: four or five times in ten seconds.
    let slow = e.counters["Slow"].value;
    assert!((4..=5).contains(&slow), "{slow}");
    // The subroutine group only runs when called (Caller fires each frame once Slow >= 1).
    assert!(e.counters["Called"].value > 0);
    assert_eq!(e.counters["Called"].value as u64, e.unhandled["action BUILD_SOMETHING_UNPORTED"]);
}

#[test]
fn skirmish_players_get_qualified_copies_of_their_sides_scripts() {
    let side = |name: &str, faction: &str| ih_formats::map::Side {
        dict: Dict(vec![
            ("playerName".into(), DictValue::Ascii(name.into())),
            ("playerFaction".into(), DictValue::Ascii(faction.into())),
        ]),
        build_list: vec![],
    };
    let mut map = Map::default();
    map.sides.sides =
        vec![side("", ""), side("PlyrCivilian", "FactionCivilian"), side("SkirmishAmerica", "FactionAmerica")];
    let template = ScriptList {
        scripts: vec![script(
            "Build",
            vec![call("FLAG", vec![Parameter { kind: 5, ..s("Go") }, i(1)])],
            vec![call("SET_FLAG", vec![Parameter { kind: 5, ..s("Go") }, i(0)])],
        )],
        groups: vec![ScriptGroup { name: "Economy".into(), active: true, subroutine: false, scripts: vec![] }],
    };
    let scb = ih_formats::scripts::ScbFile {
        lists: vec![ScriptList::default(), template],
        players: vec![
            ih_formats::scripts::ScbPlayer { name: "PlyrCivilian".into(), dict: Dict::default() },
            ih_formats::scripts::ScbPlayer { name: "SkirmishAmerica".into(), dict: Dict::default() },
        ],
    };
    let players = [SkirmishPlayer {
        player: 14,
        side: "America".into(),
        start: 2,
        ai: Some(Difficulty::Hard),
        name: "player2".into(),
    }];
    let faction_side = |f: &str| f.strip_prefix("Faction").map(str::to_string);
    let sides = skirmish_sides(&map, Some(&scb), &players, &faction_side, 1);
    let ai = sides.iter().find(|s| s.player == 14).expect("the AI got scripts");
    assert_eq!(ai.difficulty, Difficulty::Hard);
    assert_eq!(ai.scripts[0].script.name, "Build2");
    assert_eq!(ai.groups[0].name, "Economy2");
    assert_eq!(ai.scripts[0].script.conditions[0][0].params[0].string, "Go2", "flags are qualified");
}
