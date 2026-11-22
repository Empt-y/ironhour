//! FX lists as logic events.
//!
//! The original plays an `FXList` (sounds, particles, tracers, scorch marks,
//! camera shake) straight from logic code through `FXList::doFXPos` and
//! `doFXObj`. Everything an FX list does is client-side and draws only the
//! client's random numbers, so here the simulation records which list
//! played, where and for whom, and the client plays it. Events are not part of
//! the state hash and are cleared at the start of every frame.
//!
//! Sources of events so far: `WeaponTemplate::fireWeaponTemplate` (`FireFX`,
//! `ProjectileDetonationFX`, their veterancy variants and `SuspendFXDelay`,
//! hidden when the shooter is stealthed and undetected unless the weapon has
//! `PlayFXWhenStealthed` or the shooter is a mine), `FXListDie` (with
//! `OrientToObject`, `InitiallyActive`, `TriggeredBy` and `ConflictsWith`) and
//! `SlowDeathBehavior`'s phase FX.
//!
//! Not yet: body `DamageFX`, transition damage FX, missile ignition and
//! exhaust, crate, special power and construction FX, the drawable's own
//! weapon fire FX (`handleWeaponFireFX`: muzzle flashes at barrel bones), and
//! the FX lists themselves (client).
use crate::Simulation;

/// One `FXList` played.
#[derive(Clone, Debug, PartialEq)]
pub struct FxEvent {
    pub fx: String,
    pub position: [f32; 3],
    /// The object it plays for (`doFXObj`, or the shooter).
    pub object: Option<u32>,
    /// Where it points (a weapon's target, the damage dealer).
    pub target: Option<[f32; 3]>,
    /// `Some(player)`: only that player and its allies see it (a stealthed shooter's FX).
    pub allies_only: Option<u32>,
}

/// `FXListDie`.
#[derive(Clone, Debug, PartialEq)]
pub struct FxDie {
    pub fx: Option<String>,
    pub orient_to_object: bool,
    pub initially_active: bool,
    pub triggered_by: Vec<String>,
    pub conflicts_with: Vec<String>,
}
impl FxDie {
    pub fn parse(b: &ih_ini::Block) -> Self {
        let mut d = Self {
            fx: None,
            orient_to_object: false,
            initially_active: true,
            triggered_by: vec![],
            conflicts_with: vec![],
        };
        for f in b.fields() {
            let v = f.values();
            let first = v.first().cloned().unwrap_or_default();
            match f.name.as_str() {
                "DeathFX" => d.fx = (!first.is_empty() && !first.eq_ignore_ascii_case("None")).then_some(first),
                "OrientToObject" => d.orient_to_object = first.eq_ignore_ascii_case("yes"),
                "InitiallyActive" => d.initially_active = first.eq_ignore_ascii_case("yes"),
                "TriggeredBy" => d.triggered_by = v,
                "ConflictsWith" => d.conflicts_with = v,
                _ => {}
            }
        }
        d
    }
}

impl Simulation {
    pub(crate) fn play_fx(&mut self, fx: &str, position: [f32; 3], object: Option<u32>, target: Option<[f32; 3]>) {
        self.fx_events.push(FxEvent { fx: fx.to_string(), position, object, target, allies_only: None });
    }

    /// `FXListDie::onDie` (after its die filters).
    pub(crate) fn fx_die(&mut self, id: u32, d: &FxDie, dealer: u32) {
        let Some(fx) = &d.fx else { return };
        let o = &self.objects[&id];
        let owner = o.owner;
        let has = |sim: &Self, u: &String| {
            o.upgrades.completed.contains(u)
                || sim.players.get(&owner).is_some_and(|p| p.upgrades.completed.contains(u))
        };
        let active = d.initially_active || d.triggered_by.iter().any(|u| has(self, u));
        if !active || d.conflicts_with.iter().any(|u| has(self, u)) {
            return;
        }
        let pos = o.position;
        if d.orient_to_object {
            let at = self.objects.get(&dealer).map(|o| o.position);
            self.play_fx(fx, pos, Some(id), at);
        } else {
            self.play_fx(fx, pos, None, None);
        }
    }

    /// The fire or detonation FX of `fireWeaponTemplate`.
    pub(crate) fn weapon_fx(
        &mut self,
        id: u32,
        wt: &crate::weapons::WeaponTemplate,
        slot: Option<usize>,
        target: [f32; 3],
        detonation: bool,
    ) {
        let o = &self.objects[&id];
        let level = o.experience.level as usize;
        let Some(fx) = (if detonation { &wt.detonation_fx[level] } else { &wt.fire_fx[level] }) else { return };
        let suspended = slot
            .and_then(|s| o.weapon_set.weapons.get(s).and_then(|w| w.as_ref()))
            .is_some_and(|w| self.frame < w.suspend_fx_frame);
        if suspended {
            return;
        }
        let hidden = self.stealthed_and_undetected(id)
            && !self.templates.objects[&o.template].has_kind("MINE")
            && !wt.play_fx_when_stealthed;
        let position = if wt.is_contact() { target } else { o.position };
        let owner = o.owner;
        self.fx_events.push(FxEvent {
            fx: fx.clone(),
            position,
            object: Some(id),
            target: Some(target),
            allies_only: hidden.then_some(owner),
        });
    }
}
