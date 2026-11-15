//! Model condition states: which model, animation and sub-objects a draw module
//! shows for the object's current condition flags.
//!
//! Ported from `W3DDevice/GameClient/Drawable/Draw/W3DModelDraw.cpp`
//! (`parseConditionState`, `parseAnimation`, `parseShowHideSubObject`,
//! `findBestInfo`, `setModelState`, `adjustAnimation`, `doDrawModule`,
//! `doHideShowSubObjs`, `hideAllHeadlights`, `adjustAnimSpeedToMovementSpeed`) and
//! `Common/SparseMatchFinder.h`.
//!
//! Not ported here: turrets, recoil, weapon bones and particle-system bones,
//! terrain decals, shadows and track marks.

use std::collections::{BTreeMap, BTreeSet};

use ih_ini::{Block, Item};

/// A set of model condition flag names (upper case).
pub type Flags = BTreeSet<String>;

/// `RenderObjClass::AnimMode` (`TheAnimModeNames`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnimMode {
    Manual,
    Loop,
    #[default]
    Once,
    LoopPingPong,
    LoopBackwards,
    OnceBackwards,
}

impl AnimMode {
    fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_uppercase().as_str() {
            "MANUAL" => Self::Manual,
            "LOOP" => Self::Loop,
            "ONCE" => Self::Once,
            "LOOP_PINGPONG" => Self::LoopPingPong,
            "LOOP_BACKWARDS" => Self::LoopBackwards,
            "ONCE_BACKWARDS" => Self::OnceBackwards,
            _ => return None,
        })
    }
    fn backwards(self) -> bool {
        matches!(self, Self::LoopBackwards | Self::OnceBackwards)
    }
}

/// `ACBitsNames`.
pub mod ac_flags {
    pub const RANDOMSTART: u32 = 1 << 0;
    pub const START_FRAME_FIRST: u32 = 1 << 1;
    pub const START_FRAME_LAST: u32 = 1 << 2;
    pub const MAINTAIN_FRAME_ACROSS_STATES: u32 = 1 << 5;
    pub const RESTART_ANIM_WHEN_COMPLETE: u32 = 1 << 6;
    pub const MAINTAIN_FRAME_ACROSS_STATES2: u32 = 1 << 7;
    pub const MAINTAIN_FRAME_ACROSS_STATES3: u32 = 1 << 8;
    pub const MAINTAIN_FRAME_ACROSS_STATES4: u32 = 1 << 9;
    pub const MAINTAIN_ANY: u32 = MAINTAIN_FRAME_ACROSS_STATES
        | MAINTAIN_FRAME_ACROSS_STATES2
        | MAINTAIN_FRAME_ACROSS_STATES3
        | MAINTAIN_FRAME_ACROSS_STATES4;
    pub const NAMES: [&str; 10] = [
        "RANDOMSTART",
        "START_FRAME_FIRST",
        "START_FRAME_LAST",
        "ADJUST_HEIGHT_BY_CONSTRUCTION_PERCENT",
        "PRISTINE_BONE_POS_IN_FINAL_FRAME",
        "MAINTAIN_FRAME_ACROSS_STATES",
        "RESTART_ANIM_WHEN_COMPLETE",
        "MAINTAIN_FRAME_ACROSS_STATES2",
        "MAINTAIN_FRAME_ACROSS_STATES3",
        "MAINTAIN_FRAME_ACROSS_STATES4",
    ];
}

/// `W3DAnimationInfo`.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimInfo {
    /// Lower case, e.g. `aiheru_skl.aiheru_sta`.
    pub name: String,
    pub idle: bool,
    /// `DistanceCovered`: the walk cycle's length, to sync with movement speed.
    pub distance: f32,
}

/// `ModelConditionInfo`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConditionInfo {
    pub yes: Vec<Flags>,
    /// Empty for `Model = NONE`.
    pub model: String,
    /// (lower-case sub-object name, hide).
    pub hide_show: Vec<(String, bool)>,
    pub animations: Vec<AnimInfo>,
    pub mode: AnimMode,
    pub transition_key: Option<String>,
    pub allow_to_finish_key: Option<String>,
    pub flags: u32,
    pub speed_range: (f32, f32),
    /// `WeaponMuzzleFlash` names, all slots (lower case).
    pub muzzle_flashes: Vec<String>,
    anims_from_default: bool,
}

/// `W3DModelDrawModuleData`'s condition states.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelDrawData {
    pub states: Vec<ConditionInfo>,
    /// Keyed by (from, to) transition keys.
    pub transitions: BTreeMap<(String, String), ConditionInfo>,
    pub default: Option<usize>,
    pub ignore: Flags,
}

fn flags_of(values: &[String]) -> Flags {
    values.iter().filter(|v| !v.eq_ignore_ascii_case("NONE")).map(|v| v.to_ascii_uppercase()).collect()
}

impl ModelDrawData {
    /// Parses a `Draw = W3D...Draw` block's states, in file order.
    pub fn parse(draw: &Block) -> Self {
        let mut d = Self::default();
        for item in &draw.items {
            match item {
                Item::Field(f) if f.name == "IgnoreConditionStates" => d.ignore = flags_of(&f.values()),
                Item::Field(f) if f.name == "AliasConditionState" => {
                    // Adds another flag set to the previous state.
                    if let Some(prev) = d.states.last_mut() {
                        prev.yes.push(flags_of(&f.values()));
                    }
                }
                Item::Block(b) if b.kind == "DefaultConditionState" => {
                    if d.default.is_some() || !d.states.is_empty() {
                        continue; // asset error in the original
                    }
                    let mut info =
                        ConditionInfo { yes: vec![Flags::new()], speed_range: (1.0, 1.0), ..Default::default() };
                    read_state(&mut info, b);
                    d.default = Some(d.states.len());
                    d.states.push(info);
                }
                Item::Block(b) if b.kind == "ConditionState" => {
                    let mut info = d.copy_of_default();
                    info.transition_key = info.transition_key.clone();
                    info.yes = vec![flags_of(&b.args())];
                    read_state(&mut info, b);
                    d.states.push(info);
                }
                Item::Block(b) if b.kind == "TransitionState" => {
                    let args = b.args();
                    let (Some(from), Some(to)) = (args.first(), args.get(1)) else { continue };
                    let mut info = d.copy_of_default();
                    info.yes.clear();
                    info.transition_key = None;
                    info.allow_to_finish_key = None;
                    read_state(&mut info, b);
                    d.transitions.insert((from.to_ascii_lowercase(), to.to_ascii_lowercase()), info);
                }
                _ => {}
            }
        }
        d
    }

    /// A new state starts as a copy of the default state (its animations replaced
    /// as soon as the new state names any).
    fn copy_of_default(&self) -> ConditionInfo {
        match self.default {
            Some(i) => ConditionInfo { anims_from_default: true, ..self.states[i].clone() },
            None => ConditionInfo { speed_range: (1.0, 1.0), ..Default::default() },
        }
    }

    /// `findBestInfo`: the state sharing the most flags with `current`, then the one
    /// with the fewest flags `current` lacks; the first wins a tie.
    pub fn find_best(&self, current: &Flags) -> Option<usize> {
        let bits: Flags = current.difference(&self.ignore).cloned().collect();
        let mut best = None;
        let (mut best_yes, mut best_extra) = (0usize, 999usize);
        for (i, s) in self.states.iter().enumerate() {
            for yes in s.yes.iter().rev() {
                let matched = bits.intersection(yes).count();
                let extra = yes.difference(&bits).count();
                if matched > best_yes || (matched >= best_yes && extra < best_extra) {
                    best = Some(i);
                    best_yes = matched;
                    best_extra = extra;
                }
            }
        }
        best
    }
}

/// `initFromINI` with the condition-state field table.
fn read_state(info: &mut ConditionInfo, b: &Block) {
    for f in b.fields() {
        let v = f.values();
        let first = || v.first().cloned().unwrap_or_default();
        match f.name.as_str() {
            "Model" => {
                let m = first().to_ascii_lowercase();
                info.model = if m == "none" { String::new() } else { m };
            }
            "ShowSubObject" | "HideSubObject" => {
                let hide = f.name == "HideSubObject";
                for name in &v {
                    let name = name.to_ascii_lowercase();
                    if name == "none" {
                        info.hide_show.clear();
                        break;
                    }
                    match info.hide_show.iter_mut().find(|(n, _)| *n == name) {
                        Some(e) => e.1 = hide,
                        None => info.hide_show.push((name, hide)),
                    }
                }
            }
            "Animation" | "IdleAnimation" => {
                let idle = f.name == "IdleAnimation";
                if info.anims_from_default {
                    info.anims_from_default = false;
                    info.animations.clear();
                }
                let name = first().to_ascii_lowercase();
                let distance = v.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let times = v.get(2).and_then(|s| s.parse::<i32>().ok()).unwrap_or(1).max(1);
                if !name.is_empty() && name != "none" {
                    for _ in 0..times {
                        info.animations.push(AnimInfo { name: name.clone(), idle, distance });
                    }
                }
            }
            "AnimationMode" => {
                if let Some(m) = AnimMode::parse(&first()) {
                    info.mode = m;
                }
            }
            "TransitionKey" => info.transition_key = Some(first().to_ascii_lowercase()),
            "WaitForStateToFinishIfPossible" => info.allow_to_finish_key = Some(first().to_ascii_lowercase()),
            "Flags" => {
                info.flags = 0;
                for name in &v {
                    if let Some(i) = ac_flags::NAMES.iter().position(|n| n.eq_ignore_ascii_case(name)) {
                        info.flags |= 1 << i;
                    }
                }
            }
            "AnimationSpeedFactorRange" => {
                let lo = v.first().and_then(|s| s.parse().ok()).unwrap_or(1.0);
                let hi = v.get(1).and_then(|s| s.parse().ok()).unwrap_or(lo);
                info.speed_range = (lo, hi);
            }
            "WeaponMuzzleFlash" => {
                if let Some(n) = v.get(1) {
                    let n = n.to_ascii_lowercase();
                    if n != "none" && !info.muzzle_flashes.contains(&n) {
                        info.muzzle_flashes.push(n);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Which state is showing: a normal state or a transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateRef {
    Normal(usize),
    Transition(String, String),
}

/// Animation playback on an HLOD (`HLodClass` animation state).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Playback {
    pub anim: Option<String>,
    pub frame: f32,
    pub frames: u32,
    /// Native frames per second.
    pub fps: f32,
    pub mode: AnimMode,
    /// `Set_Animation_Frame_Rate_Multiplier`.
    pub multiplier: f32,
    pub complete: bool,
    pingpong_back: bool,
}

impl Playback {
    fn advance(&mut self, dt_ms: f32) {
        if self.anim.is_none() || self.frames == 0 || self.complete {
            return;
        }
        let last = (self.frames - 1) as f32;
        let step = self.fps * self.multiplier * dt_ms / 1000.0;
        match self.mode {
            AnimMode::Manual => {}
            AnimMode::Loop => self.frame = (self.frame + step).rem_euclid(self.frames as f32),
            AnimMode::LoopBackwards => self.frame = (self.frame - step).rem_euclid(self.frames as f32),
            AnimMode::Once => {
                self.frame += step;
                if self.frame >= last {
                    self.frame = last;
                    self.complete = true;
                }
            }
            AnimMode::OnceBackwards => {
                self.frame -= step;
                if self.frame <= 0.0 {
                    self.frame = 0.0;
                    self.complete = true;
                }
            }
            AnimMode::LoopPingPong => {
                if self.pingpong_back {
                    self.frame -= step;
                    if self.frame <= 0.0 {
                        self.frame = -self.frame;
                        self.pingpong_back = false;
                    }
                } else {
                    self.frame += step;
                    if self.frame >= last {
                        self.frame = 2.0 * last - self.frame;
                        self.pingpong_back = true;
                    }
                }
            }
        }
    }

    /// `isAnimationComplete`: true without an animation.
    pub fn is_complete(&self) -> bool {
        self.anim.is_none() || self.complete
    }

    fn fraction(&self) -> f32 {
        if self.frames <= 1 {
            return 0.0;
        }
        (self.frame / (self.frames - 1) as f32).clamp(0.0, 1.0)
    }
}

/// A small client-side random source (`GameClientRandomValue`): not part of the
/// deterministic simulation.
#[derive(Clone, Debug)]
pub struct ClientRng(u64);

impl ClientRng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// Inclusive range.
    pub fn int(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next() % (hi - lo + 1) as u64) as i32
    }
    pub fn real(&mut self, lo: f32, hi: f32) -> f32 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next() >> 40) as f32 / (1u64 << 24) as f32 * (hi - lo)
    }
}

/// One draw module's live state on a drawable (`W3DModelDraw`).
#[derive(Clone, Debug)]
pub struct ModelDrawState {
    pub cur: Option<StateRef>,
    pub next: Option<usize>,
    pub which_anim: Option<usize>,
    pub playback: Playback,
    /// The current render object's model (empty: none).
    pub model: String,
    /// Per sub-object of `model`: hidden.
    pub hidden: Vec<bool>,
    pub hide_headlights: bool,
    pub rng: ClientRng,
}

/// A model's sub-objects (full name, bone) and each bone's parent.
pub type SubObjects = (Vec<(String, u32)>, Vec<Option<u32>>);

/// What the state needs from loaded assets.
pub trait ArtSource {
    /// (frames, frames per second) of an animation.
    fn animation_info(&mut self, name: &str) -> Option<(u32, f32)>;
    /// Full sub-object names of a model and each one's bone, plus each bone's parent.
    fn sub_objects(&mut self, model: &str) -> Option<SubObjects>;
}

impl ModelDrawState {
    pub fn new(seed: u64) -> Self {
        Self {
            cur: None,
            next: None,
            which_anim: None,
            playback: Playback::default(),
            model: String::new(),
            hidden: Vec::new(),
            hide_headlights: true,
            rng: ClientRng::new(seed),
        }
    }

    pub fn info<'a>(&self, data: &'a ModelDrawData) -> Option<&'a ConditionInfo> {
        Self::resolve(data, self.cur.as_ref()?)
    }

    fn resolve<'a>(data: &'a ModelDrawData, r: &StateRef) -> Option<&'a ConditionInfo> {
        match r {
            StateRef::Normal(i) => data.states.get(*i),
            StateRef::Transition(a, b) => data.transitions.get(&(a.clone(), b.clone())),
        }
    }

    /// `replaceModelConditionState`.
    pub fn set_conditions(&mut self, data: &ModelDrawData, flags: &Flags, art: &mut dyn ArtSource) {
        self.hide_headlights = !flags.contains("NIGHT");
        if let Some(best) = data.find_best(flags) {
            self.set_model_state(data, best, art);
        }
        self.apply_headlights(art);
    }

    /// `setModelState`.
    pub fn set_model_state(&mut self, data: &ModelDrawData, wanted: usize, art: &mut dyn ArtSource) {
        let mut new_ref = StateRef::Normal(wanted);
        let mut next_state = None;
        if let Some(cur) = self.cur.clone() {
            if (cur == new_ref && self.next.is_none()) || self.next == Some(wanted) {
                return;
            }
            let cur_info = Self::resolve(data, &cur);
            let new_info = &data.states[wanted];
            let cur_key = cur_info.and_then(|c| c.transition_key.clone());
            if cur != new_ref
                && new_info.allow_to_finish_key.is_some()
                && new_info.allow_to_finish_key == cur_key
                && !self.playback.is_complete()
            {
                self.next = Some(wanted);
                return;
            }
            if cur != new_ref
                && let (Some(a), Some(b)) = (cur_key, new_info.transition_key.clone())
                && data.transitions.contains_key(&(a.clone(), b.clone()))
            {
                next_state = Some(wanted);
                new_ref = StateRef::Transition(a, b);
            }
        }
        let prev_fraction = self.current_anim_fraction(data);
        let Some(new_info) = Self::resolve(data, &new_ref) else { return };
        if self.cur.is_none() || self.model != new_info.model {
            self.model = new_info.model.clone();
            let count = art.sub_objects(&self.model).map_or(0, |s| s.0.len());
            self.hidden = vec![false; count];
        }
        self.hide_muzzle_flashes(new_info, art);
        self.apply_hide_show(&new_info.hide_show.clone(), art);
        let prev = self.cur.replace(new_ref);
        self.next = next_state;
        self.adjust_animation(data, prev, prev_fraction, art);
    }

    /// `getCurrentAnimFraction`: only for states that keep their frame.
    fn current_anim_fraction(&self, data: &ModelDrawData) -> f32 {
        match self.info(data) {
            Some(c) if c.flags & ac_flags::MAINTAIN_ANY != 0 && self.playback.anim.is_some() => {
                self.playback.fraction()
            }
            _ => -1.0,
        }
    }

    /// `adjustAnimation`.
    fn adjust_animation(
        &mut self,
        data: &ModelDrawData,
        prev: Option<StateRef>,
        prev_fraction: f32,
        art: &mut dyn ArtSource,
    ) {
        let Some(cur_ref) = self.cur.clone() else { return };
        let Some(cur) = Self::resolve(data, &cur_ref) else { return };
        let n = cur.animations.len();
        if n == 0 {
            self.which_anim = None;
            self.playback = Playback::default();
            return;
        }
        let which = if n == 1 {
            0
        } else if prev.as_ref() == Some(&cur_ref) {
            let avoid = self.which_anim;
            let mut w = self.rng.int(0, n as i32 - 1) as usize;
            while Some(w) == avoid {
                w = self.rng.int(0, n as i32 - 1) as usize;
            }
            w
        } else {
            self.rng.int(0, n as i32 - 1) as usize
        };
        self.which_anim = Some(which);
        let name = cur.animations[which].name.clone();
        let Some((frames, fps)) = art.animation_info(&name) else {
            self.playback = Playback::default();
            return;
        };
        let last = frames.saturating_sub(1) as f32;
        let mut start = if cur.mode.backwards() { last } else { 0.0 };
        let prev_info = prev.as_ref().and_then(|p| Self::resolve(data, p));
        if cur.flags & ac_flags::RANDOMSTART != 0 {
            start = self.rng.int(0, frames as i32 - 1) as f32;
        } else if cur.flags & ac_flags::START_FRAME_FIRST != 0 {
            start = 0.0;
        } else if cur.flags & ac_flags::START_FRAME_LAST != 0 {
            start = last;
        } else if cur.flags & ac_flags::MAINTAIN_ANY != 0
            && let Some(p) = prev_info
            && prev.as_ref() != Some(&cur_ref)
            && p.flags & ac_flags::MAINTAIN_ANY != 0
            && p.flags & cur.flags & ac_flags::MAINTAIN_ANY != 0
            && prev_fraction >= 0.0
        {
            // REAL_TO_INT(fraction * numFrames - 1)
            start = (prev_fraction * frames as f32 - 1.0).trunc().max(0.0);
        }
        let multiplier = self.rng.real(cur.speed_range.0, cur.speed_range.1);
        self.playback = Playback {
            anim: Some(name),
            frame: start,
            frames,
            fps,
            mode: cur.mode,
            multiplier,
            complete: false,
            pingpong_back: false,
        };
    }

    /// `doDrawModule`'s animation part: advance, switch to the pending state when a
    /// transition finishes, pick another idle animation, sync walk cycles to speed.
    pub fn update(&mut self, data: &ModelDrawData, dt_ms: f32, speed_per_frame: f32, art: &mut dyn ArtSource) {
        if self.playback.is_complete() {
            if let Some(next) = self.next.take() {
                self.set_model_state(data, next, art);
            }
            if let (Some(cur), Some(w)) = (self.info(data), self.which_anim) {
                let restart = cur.animations.get(w).is_some_and(|a| a.idle)
                    || cur.flags & ac_flags::RESTART_ANIM_WHEN_COMPLETE != 0;
                if restart && self.playback.anim.is_some() {
                    let same = self.cur.clone();
                    self.adjust_animation(data, same, -1.0, art);
                }
            }
        }
        // adjustAnimSpeedToMovementSpeed
        if let (Some(cur), Some(w)) = (self.info(data), self.which_anim)
            && let Some(a) = cur.animations.get(w)
            && a.distance > 0.0
            && speed_per_frame > 0.0
            && self.playback.fps > 0.0
        {
            let desired_ms = a.distance / speed_per_frame * (1000.0 / 30.0);
            let natural_ms = self.playback.frames as f32 * 1000.0 / self.playback.fps;
            if natural_ms > 0.0 && desired_ms > 0.0 {
                self.playback.multiplier = natural_ms / desired_ms;
            }
        }
        self.playback.advance(dt_ms);
    }

    /// `doHideShowSubObjs` with `doHideShowBoneSubObjs`.
    fn apply_hide_show(&mut self, list: &[(String, bool)], art: &mut dyn ArtSource) {
        let Some((subs, parents)) = art.sub_objects(&self.model) else { return };
        for (name, hide) in list {
            let Some(i) = find_sub_object(&subs, name) else { continue };
            self.hidden[i] = *hide;
            let bone = subs[i].1;
            for (j, (_, b)) in subs.iter().enumerate() {
                let mut p = *b;
                while p != 0 {
                    p = parents.get(p as usize).copied().flatten().unwrap_or(0);
                    if p == bone {
                        self.hidden[j] = *hide;
                        break;
                    }
                }
            }
        }
    }

    /// `hideAllMuzzleFlashes`: muzzle flash sub-objects (`NAME` or `NAMEnn`) start hidden.
    fn hide_muzzle_flashes(&mut self, info: &ConditionInfo, art: &mut dyn ArtSource) {
        let Some((subs, _)) = art.sub_objects(&self.model) else { return };
        for (i, (full, _)) in subs.iter().enumerate() {
            let short = full.rsplit_once('.').map_or(full.as_str(), |(_, s)| s).to_ascii_lowercase();
            if info.muzzle_flashes.iter().any(|m| {
                short == *m || (short.starts_with(m.as_str()) && short[m.len()..].chars().all(|c| c.is_ascii_digit()))
            }) {
                self.hidden[i] = true;
            }
        }
    }

    /// `hideAllHeadlights`: sub-objects named `...HEADLIGHT...` show only at night.
    fn apply_headlights(&mut self, art: &mut dyn ArtSource) {
        let Some((subs, _)) = art.sub_objects(&self.model) else { return };
        for (i, (full, _)) in subs.iter().enumerate() {
            if full.contains("HEADLIGHT") {
                self.hidden[i] = self.hide_headlights;
            }
        }
    }
}

/// `RenderObjClass::Get_Sub_Object_By_Name`: the full name first, then the part
/// after the container name.
fn find_sub_object(subs: &[(String, u32)], name: &str) -> Option<usize> {
    subs.iter().position(|(n, _)| n.eq_ignore_ascii_case(name)).or_else(|| {
        subs.iter().position(|(n, _)| n.split_once('.').map_or(n.as_str(), |(_, s)| s).eq_ignore_ascii_case(name))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(text: &str) -> ModelDrawData {
        let doc = ih_ini::parse(text.as_bytes());
        let obj = &doc.blocks[0];
        let draw = obj.blocks().find(|b| b.kind == "Draw").unwrap();
        ModelDrawData::parse(draw)
    }

    fn flags(names: &[&str]) -> Flags {
        names.iter().map(|s| s.to_string()).collect()
    }

    struct Art;
    impl ArtSource for Art {
        fn animation_info(&mut self, name: &str) -> Option<(u32, f32)> {
            Some(if name.contains("trans") { (10, 30.0) } else { (30, 30.0) })
        }
        fn sub_objects(&mut self, _: &str) -> Option<SubObjects> {
            Some((
                vec![
                    ("M.BODY".into(), 0),
                    ("M.TURRET".into(), 1),
                    ("M.BARREL".into(), 2),
                    ("M.MUZZLEFX01".into(), 2),
                    ("M.HEADLIGHT".into(), 0),
                ],
                vec![None, Some(0), Some(1)],
            ))
        }
    }

    const TANK: &str = "Object T\n  Draw = W3DModelDraw D\n    DefaultConditionState\n      Model = TankA\n      HideSubObject = Turret\n      WeaponMuzzleFlash = PRIMARY MuzzleFX\n    End\n    ConditionState = MOVING\n      Animation = TankA.Move\n      AnimationMode = LOOP\n    End\n    ConditionState = REALLYDAMAGED\n      Model = TankA_D\n    End\n    AliasConditionState = RUBBLE\n    ConditionState = MOVING REALLYDAMAGED\n      Model = TankA_D\n      TransitionKey = trans_moving\n    End\n  End\nEnd\n";

    #[test]
    fn states_copy_the_default_and_best_match_prefers_fewer_extra_flags() {
        let d = data(TANK);
        assert_eq!(d.default, Some(0));
        assert_eq!(d.states.len(), 4);
        // Copied from the default.
        assert_eq!(d.states[1].model, "tanka");
        assert_eq!(d.states[1].hide_show, [("turret".to_string(), true)]);
        assert_eq!(d.states[1].mode, AnimMode::Loop);
        assert_eq!(d.find_best(&Flags::new()), Some(0));
        assert_eq!(d.find_best(&flags(&["MOVING"])), Some(1));
        assert_eq!(d.find_best(&flags(&["RUBBLE"])), Some(2)); // alias
        assert_eq!(d.find_best(&flags(&["MOVING", "REALLYDAMAGED"])), Some(3));
        // An unknown flag alone still falls back to the default.
        assert_eq!(d.find_best(&flags(&["FIRING_A"])), Some(0));
    }

    #[test]
    fn hide_show_hides_bone_children_and_muzzle_flashes() {
        let d = data(TANK);
        let mut s = ModelDrawState::new(1);
        s.set_conditions(&d, &Flags::new(), &mut Art);
        // Turret hidden, with the barrel and flash on its child bone.
        assert_eq!(s.hidden, [false, true, true, true, true]);
        s.set_conditions(&d, &flags(&["NIGHT"]), &mut Art);
        assert!(!s.hidden[4], "headlights show at night");
    }

    #[test]
    fn transitions_play_before_the_requested_state() {
        let text = "Object T\n  Draw = W3DModelDraw D\n    ConditionState = NONE\n      Model = A\n      TransitionKey = up\n    End\n    ConditionState = DEPLOYED\n      Model = A\n      TransitionKey = down\n    End\n    TransitionState = up down\n      Model = A\n      Animation = A.trans\n      AnimationMode = ONCE\n    End\n  End\nEnd\n";
        let d = data(text);
        let mut s = ModelDrawState::new(1);
        s.set_conditions(&d, &Flags::new(), &mut Art);
        s.set_conditions(&d, &flags(&["DEPLOYED"]), &mut Art);
        assert_eq!(s.cur, Some(StateRef::Transition("up".into(), "down".into())));
        assert_eq!(s.next, Some(1));
        // 10 frames at 30 fps: done after a third of a second.
        for _ in 0..12 {
            s.update(&d, 33.3, 0.0, &mut Art);
        }
        assert_eq!(s.cur, Some(StateRef::Normal(1)));
    }

    #[test]
    fn loop_and_once_playback() {
        let mut p = Playback {
            anim: Some("a".into()),
            frames: 10,
            fps: 10.0,
            mode: AnimMode::Once,
            multiplier: 1.0,
            ..Default::default()
        };
        p.advance(2000.0);
        assert!(p.complete && p.frame == 9.0);
        let mut p = Playback {
            anim: Some("a".into()),
            frames: 10,
            fps: 10.0,
            mode: AnimMode::Loop,
            multiplier: 1.0,
            ..Default::default()
        };
        p.advance(1500.0);
        assert!(!p.complete && (p.frame - 5.0).abs() < 1e-4);
    }
}
