//! Map scripts: `PlayerScriptsList` chunks in maps and `.scb` files.
//!
//! Ported from `GameLogic/ScriptEngine/Scripts.cpp` (`ScriptList::ParseScriptsDataChunk`,
//! `ParseScriptListDataChunk`, `ScriptGroup::ParseGroupDataChunk`,
//! `Script::ParseScriptFromListDataChunk`, `OrCondition::ParseOrConditionDataChunk`,
//! `Condition::ParseConditionDataChunk`, `ScriptAction::ParseActionDataChunk`,
//! `ParseActionFalseDataChunk`, `Parameter::ReadParameter`) and
//! `SidesList::ParseSidesDataChunk`, which nests the player scripts inside the
//! sides.
//!
//! Since condition version 4 and action version 2, each one also stores a name
//! key: an index into the file's name table that gives its internal name
//! (`TEAM_DESTROYED`). The original rematches by that name when the enum
//! moved; here the name is kept as the identity. Older entries keep only the
//! enum index.
//!
//! Not yet: the parameter fix-ups for old versions (`ParameterChangesVer2`, the
//! `Fundamentalist` object renames), and the `.scb` file's player and team
//! chunks (`ScriptsPlayers`, `ScriptTeams`).
use crate::map::{MapError, Reader, Toc, read_toc};

/// `Parameter::COORD3D`, stored as three reals.
pub const PARAMETER_COORD3D: i32 = 16;

/// A `Parameter`.
#[derive(Clone, Debug, PartialEq)]
pub struct Parameter {
    /// `ParameterType`.
    pub kind: i32,
    pub int: i32,
    pub real: f32,
    pub string: String,
    pub coord: Option<[f32; 3]>,
}

/// A `Condition` or a `ScriptAction`.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    /// The enum index as stored.
    pub index: i32,
    /// The internal name, when the chunk version stores it.
    pub name: Option<String>,
    pub params: Vec<Parameter>,
}

/// A `Script`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Script {
    pub name: String,
    pub comment: String,
    pub active: bool,
    pub one_shot: bool,
    /// Difficulties it runs at: easy, normal, hard.
    pub difficulty: [bool; 3],
    pub subroutine: bool,
    pub delay_seconds: i32,
    /// `OrCondition`s, each a list of conditions that must all hold.
    pub conditions: Vec<Vec<Call>>,
    pub actions: Vec<Call>,
    pub false_actions: Vec<Call>,
}

/// A `ScriptGroup`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptGroup {
    pub name: String,
    pub active: bool,
    pub subroutine: bool,
    pub scripts: Vec<Script>,
}

/// A player's `ScriptList`: loose scripts and groups, in file order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptList {
    pub scripts: Vec<Script>,
    pub groups: Vec<ScriptGroup>,
}
impl ScriptList {
    /// Every script, loose ones first then each group's, as the engine walks them.
    pub fn all(&self) -> impl Iterator<Item = &Script> {
        self.scripts.iter().chain(self.groups.iter().flat_map(|g| g.scripts.iter()))
    }
}

fn param(r: &mut Reader) -> Result<Parameter, MapError> {
    let kind = r.i32()?;
    if kind == PARAMETER_COORD3D {
        let c = [r.f32()?, r.f32()?, r.f32()?];
        return Ok(Parameter { kind, int: 0, real: 0.0, string: String::new(), coord: Some(c) });
    }
    Ok(Parameter { kind, int: r.i32()?, real: r.f32()?, string: r.ascii()?, coord: None })
}

/// A condition (`with_name` from version 4) or action (from version 2).
fn call(r: &mut Reader, toc: &Toc, with_name: bool) -> Result<Call, MapError> {
    let index = r.i32()?;
    // readNameKey: the name's table id shifted past a type byte, as dictionary keys.
    let name = if with_name { toc.get(&((r.i32()? >> 8) as u32)).cloned() } else { None };
    let n = r.i32()?;
    let mut params = Vec::with_capacity(n.max(0) as usize);
    for _ in 0..n.max(0) {
        params.push(param(r)?);
    }
    Ok(Call { index, name, params })
}

fn script(r: &mut Reader, toc: &Toc, version: u16) -> Result<Script, MapError> {
    let mut s = Script { name: r.ascii()?, comment: r.ascii()?, ..Default::default() };
    let _condition_comment = r.ascii()?;
    let _action_comment = r.ascii()?;
    s.active = r.u8()? != 0;
    s.one_shot = r.u8()? != 0;
    s.difficulty = [r.u8()? != 0, r.u8()? != 0, r.u8()? != 0];
    s.subroutine = r.u8()? != 0;
    if version >= 2 {
        s.delay_seconds = r.i32()?;
    }
    while r.remaining() >= crate::map::CHUNK_HEADER {
        let chunk = r.open_chunk(toc)?;
        let mut c = chunk.reader;
        match chunk.label.as_str() {
            "OrCondition" => {
                let mut ands = vec![];
                while c.remaining() >= crate::map::CHUNK_HEADER {
                    let cond = c.open_chunk(toc)?;
                    if cond.label == "Condition" {
                        let mut cr = cond.reader;
                        ands.push(call(&mut cr, toc, cond.version >= 4)?);
                    }
                }
                s.conditions.push(ands);
            }
            "ScriptAction" => s.actions.push(call(&mut c, toc, chunk.version >= 2)?),
            "ScriptActionFalse" => s.false_actions.push(call(&mut c, toc, chunk.version >= 2)?),
            _ => {}
        }
    }
    Ok(s)
}

/// A `PlayerScriptsList` chunk's body: one `ScriptList` per player.
pub(crate) fn player_scripts(r: &mut Reader, toc: &Toc) -> Result<Vec<ScriptList>, MapError> {
    let mut lists = vec![];
    while r.remaining() >= crate::map::CHUNK_HEADER {
        let chunk = r.open_chunk(toc)?;
        if chunk.label != "ScriptList" {
            continue;
        }
        let mut c = chunk.reader;
        let mut list = ScriptList::default();
        while c.remaining() >= crate::map::CHUNK_HEADER {
            let inner = c.open_chunk(toc)?;
            let mut ic = inner.reader;
            match inner.label.as_str() {
                "Script" => list.scripts.push(script(&mut ic, toc, inner.version)?),
                "ScriptGroup" => {
                    let mut g = ScriptGroup { name: ic.ascii()?, active: ic.u8()? != 0, ..Default::default() };
                    if inner.version >= 2 {
                        g.subroutine = ic.u8()? != 0;
                    }
                    while ic.remaining() >= crate::map::CHUNK_HEADER {
                        let sc = ic.open_chunk(toc)?;
                        if sc.label == "Script" {
                            let mut sr = sc.reader;
                            g.scripts.push(script(&mut sr, toc, sc.version)?);
                        }
                    }
                    list.groups.push(g);
                }
                _ => {}
            }
        }
        lists.push(list);
    }
    Ok(lists)
}

/// A player of a `.scb` file (`ScriptsPlayers`): the name its script list is for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScbPlayer {
    /// `SkirmishAmerica`, matched against the skirmish sides' player names.
    pub name: String,
    /// The side dictionary (`playerFaction` and others), when stored.
    pub dict: crate::map::Dict,
}

/// A `.scb` file: its script lists and, in the same order, their players.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScbFile {
    pub lists: Vec<ScriptList>,
    pub players: Vec<ScbPlayer>,
}

/// A `.scb` file (`SkirmishScripts.scb`), as `SidesList::prepareForMP_or_Skirmish`
/// reads it: `PlayerScriptsList` and `ScriptsPlayers` (`ParsePlayersDataChunk`).
pub fn parse_scb(raw: &[u8]) -> Result<ScbFile, MapError> {
    let data = crate::compression::decompress(raw)?;
    let (toc, start) = read_toc(&data)?;
    let mut r = Reader::new(&data, start);
    let mut out = ScbFile::default();
    while r.remaining() >= crate::map::CHUNK_HEADER {
        let chunk = r.open_chunk(&toc)?;
        let mut c = chunk.reader;
        match chunk.label.as_str() {
            "PlayerScriptsList" => out.lists = player_scripts(&mut c, &toc)?,
            "ScriptsPlayers" => {
                let dicts = chunk.version >= 2 && c.i32()? != 0;
                let n = c.i32()?;
                for _ in 0..n.max(0) {
                    let name = c.ascii()?;
                    let dict = if dicts { c.dict(&toc)? } else { Default::default() };
                    out.players.push(ScbPlayer { name, dict });
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// `THIS_TEAM`, which qualifying leaves alone.
pub const THIS_TEAM: &str = "<This Team>";

impl Parameter {
    /// `Parameter::qualify`.
    fn qualify(&mut self, qualifier: &str, template_player: &str, new_player: &str) {
        const SCRIPT: i32 = 2;
        const TEAM: i32 = 3;
        const COUNTER: i32 = 4;
        const FLAG: i32 = 5;
        const SIDE: i32 = 11;
        const SCRIPT_SUBROUTINE: i32 = 13;
        match self.kind {
            SIDE => {
                if format!("{}{qualifier}", self.string) == template_player {
                    self.string = new_player.to_string();
                }
            }
            TEAM if self.string == THIS_TEAM => {}
            TEAM | SCRIPT | COUNTER | FLAG | SCRIPT_SUBROUTINE => self.string.push_str(qualifier),
            _ => {}
        }
    }
}

impl ScriptList {
    /// `ScriptList::duplicateAndQualify`: script and group names, and the script,
    /// team, counter, flag and subroutine parameters, get the qualifier (a
    /// player's start index); side parameters naming the template player become
    /// the new player.
    pub fn qualified(&self, qualifier: &str, template_player: &str, new_player: &str) -> ScriptList {
        let q = |s: &Script| {
            let mut s = s.clone();
            s.name.push_str(qualifier);
            for c in s.conditions.iter_mut().flatten().chain(s.actions.iter_mut()).chain(s.false_actions.iter_mut()) {
                for p in &mut c.params {
                    p.qualify(qualifier, template_player, new_player);
                }
            }
            s
        };
        ScriptList {
            scripts: self.scripts.iter().map(q).collect(),
            groups: self
                .groups
                .iter()
                .map(|g| ScriptGroup {
                    name: format!("{}{qualifier}", g.name),
                    active: g.active,
                    subroutine: g.subroutine,
                    scripts: g.scripts.iter().map(q).collect(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal chunk writer: a name table plus nested chunks.
    struct W {
        names: Vec<String>,
    }
    impl W {
        fn id(&mut self, n: &str) -> i32 {
            if let Some(i) = self.names.iter().position(|x| x == n) {
                return i as i32 + 1;
            }
            self.names.push(n.into());
            self.names.len() as i32
        }
        fn chunk(&mut self, label: &str, version: u16, body: &[u8]) -> Vec<u8> {
            let mut out = self.id(label).to_le_bytes().to_vec();
            out.extend(version.to_le_bytes());
            out.extend((body.len() as i32).to_le_bytes());
            out.extend(body);
            out
        }
        fn file(&self, body: &[u8]) -> Vec<u8> {
            let mut out = b"CkMp".to_vec();
            out.extend((self.names.len() as i32).to_le_bytes());
            for (i, n) in self.names.iter().enumerate() {
                out.push(n.len() as u8);
                out.extend(n.as_bytes());
                out.extend((i as i32 + 1).to_le_bytes());
            }
            out.extend(body);
            out
        }
    }
    fn ascii(s: &str) -> Vec<u8> {
        let mut v = (s.len() as u16).to_le_bytes().to_vec();
        v.extend(s.as_bytes());
        v
    }

    #[test]
    fn reads_groups_conditions_actions_and_named_keys() {
        let mut w = W { names: vec![] };
        // Condition v4: type, name key (id << 8 | string type), one int parameter.
        let mut cond = 2i32.to_le_bytes().to_vec();
        cond.extend(((w.id("COUNTER") << 8) | 3).to_le_bytes());
        cond.extend(1i32.to_le_bytes());
        cond.extend(0i32.to_le_bytes()); // INT parameter
        cond.extend(7i32.to_le_bytes());
        cond.extend(0.5f32.to_le_bytes());
        cond.extend(ascii("Counter1"));
        let cond = w.chunk("Condition", 4, &cond);
        let or = w.chunk("OrCondition", 1, &cond);
        // Action v2 with a coordinate parameter.
        let mut act = 20i32.to_le_bytes().to_vec();
        act.extend(((w.id("SET_FLAG") << 8) | 3).to_le_bytes());
        act.extend(1i32.to_le_bytes());
        act.extend(PARAMETER_COORD3D.to_le_bytes());
        for v in [1.0f32, 2.0, 3.0] {
            act.extend(v.to_le_bytes());
        }
        let act = w.chunk("ScriptAction", 2, &act);
        // Script v2 header.
        let mut script = vec![];
        for s in ["Build Base", "comment", "", ""] {
            script.extend(ascii(s));
        }
        script.extend([1, 0, 1, 1, 0, 0]);
        script.extend(5i32.to_le_bytes());
        script.extend(or);
        script.extend(act);
        let script = w.chunk("Script", 2, &script);
        let mut group = ascii("Economy");
        group.extend([1, 0]);
        group.extend(script);
        let group = w.chunk("ScriptGroup", 2, &group);
        let list = w.chunk("ScriptList", 1, &group);
        let psl = w.chunk("PlayerScriptsList", 5, &list);
        let lists = parse_scb(&w.file(&psl)).unwrap().lists;
        assert_eq!(lists.len(), 1);
        let g = &lists[0].groups[0];
        assert_eq!((g.name.as_str(), g.active), ("Economy", true));
        let s = &g.scripts[0];
        assert_eq!((s.name.as_str(), s.difficulty, s.delay_seconds), ("Build Base", [true, true, false], 5));
        let c = &s.conditions[0][0];
        assert_eq!((c.index, c.name.as_deref()), (2, Some("COUNTER")));
        assert_eq!((c.params[0].int, c.params[0].string.as_str()), (7, "Counter1"));
        let a = &s.actions[0];
        assert_eq!((a.name.as_deref(), a.params[0].coord), (Some("SET_FLAG"), Some([1.0, 2.0, 3.0])));
        assert_eq!(lists[0].all().count(), 1);
    }
}
