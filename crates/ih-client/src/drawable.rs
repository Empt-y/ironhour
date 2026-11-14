//! What an object template draws in its idle state, read from its `Draw` modules.
//!
//! A first cut of `W3DModelDraw` (`W3DModelDraw.cpp`, `parseConditionState`),
//! `W3DTreeDraw` and `W3DPropDraw`: the model of the condition state that matches
//! the wanted flags (none, `NIGHT`, `SNOW`), the sub-objects it hides, and its
//! animation. Full condition-state switching comes with drawables in M4.

use ih_ini::Block;

use crate::defs::Definitions;

/// One model an object draws.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawModel {
    /// The draw module's class, e.g. `W3DModelDraw`.
    pub module: String,
    pub model: String,
    /// Sub-objects hidden in this state (`HideSubObject`, muzzle flashes).
    pub hide: Vec<String>,
    /// Sub-objects explicitly shown (`ShowSubObject`).
    pub show: Vec<String>,
    /// `Animation` or `IdleAnimation` (first name only).
    pub animation: Option<String>,
}

/// How a template draws.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TemplateArt {
    pub models: Vec<DrawModel>,
    /// `Scale`.
    pub scale: f32,
    pub kind_of: Vec<String>,
}

impl TemplateArt {
    pub fn is_kind_of(&self, k: &str) -> bool {
        self.kind_of.iter().any(|x| x.eq_ignore_ascii_case(k))
    }
}

fn field_values<'a>(b: &'a Block, name: &str) -> impl Iterator<Item = Vec<String>> + 'a {
    let name = name.to_string();
    b.fields().filter(move |f| f.name.eq_ignore_ascii_case(&name)).map(|f| f.values())
}

/// The template's own block, following `ObjectReskin` to its base for what the
/// reskin does not override.
fn template_blocks<'a>(defs: &'a Definitions, name: &str) -> Vec<&'a Block> {
    let mut out = Vec::new();
    let mut cur = name.to_string();
    for _ in 0..8 {
        if let Some(b) = defs.get("Object", &cur) {
            out.push(b);
            break;
        }
        let Some(b) = defs.get("ObjectReskin", &cur) else { break };
        out.push(b);
        match b.args().get(1) {
            Some(base) => cur = base.clone(),
            None => break,
        }
    }
    out
}

/// Reads what `name` draws. `wanted` is the condition flags of the idle state
/// (e.g. `["NIGHT"]`).
pub fn template_art(defs: &Definitions, name: &str, wanted: &[&str]) -> Option<TemplateArt> {
    let blocks = template_blocks(defs, name);
    if blocks.is_empty() {
        return None;
    }
    let mut art = TemplateArt { scale: 1.0, ..Default::default() };
    // Fields: the most derived definition that sets them wins.
    let mut scale_set = false;
    let mut kind_set = false;
    for b in &blocks {
        if !scale_set && let Some(v) = field_values(b, "Scale").last() {
            art.scale = v.first().and_then(|s| s.parse().ok()).unwrap_or(1.0);
            scale_set = true;
        }
        if !kind_set && let Some(v) = field_values(b, "KindOf").last() {
            art.kind_of = v;
            kind_set = true;
        }
    }
    // Draw modules: a reskin that declares any replaces the base's.
    let draws = blocks
        .iter()
        .map(|b| b.blocks().filter(|d| d.kind.eq_ignore_ascii_case("Draw")).collect::<Vec<_>>())
        .find(|d| !d.is_empty())
        .unwrap_or_default();
    for d in draws {
        let module = d.args().into_iter().next().unwrap_or_default();
        match module.as_str() {
            "W3DTreeDraw" | "W3DPropDraw" => {
                if let Some(m) = field_values(d, "ModelName").last().and_then(|v| v.into_iter().next()) {
                    art.models.push(DrawModel { module, model: m, hide: vec![], show: vec![], animation: None });
                }
            }
            _ => {
                if let Some(m) = condition_model(d, wanted) {
                    art.models.push(DrawModel { module, ..m });
                }
            }
        }
    }
    Some(art)
}

/// The state block best matching `wanted`: one whose flags equal it, else the
/// default state, else one with no flags, else the first.
pub(crate) fn condition_model(d: &Block, wanted: &[&str]) -> Option<DrawModel> {
    let states: Vec<&Block> = d
        .blocks()
        .filter(|b| {
            b.kind.eq_ignore_ascii_case("ConditionState") || b.kind.eq_ignore_ascii_case("DefaultConditionState")
        })
        .collect();
    let flags = |b: &Block| -> Vec<String> {
        b.args().into_iter().filter(|a| !a.eq_ignore_ascii_case("NONE")).map(|a| a.to_ascii_uppercase()).collect()
    };
    let exact = |b: &&&Block| {
        if b.kind.eq_ignore_ascii_case("DefaultConditionState") {
            return wanted.is_empty();
        }
        let f = flags(b);
        f.len() == wanted.len() && wanted.iter().all(|w| f.iter().any(|x| x.eq_ignore_ascii_case(w)))
    };
    let state = states
        .iter()
        .find(exact)
        .or_else(|| states.iter().find(|b| b.kind.eq_ignore_ascii_case("DefaultConditionState")))
        .or_else(|| states.iter().find(|b| flags(b).is_empty()))
        .or_else(|| states.first())?;
    // A ConditionState starts as a copy of the DefaultConditionState before it.
    let default = states.iter().find(|b| b.kind.eq_ignore_ascii_case("DefaultConditionState"));
    let model = field_values(state, "Model")
        .last()
        .or_else(|| default.and_then(|d| field_values(d, "Model").last()))
        .and_then(|v| v.into_iter().next())?;
    if model.eq_ignore_ascii_case("NONE") {
        return None;
    }
    let mut hide: Vec<String> = field_values(state, "HideSubObject").flatten().collect();
    // Muzzle flashes start hidden (W3DModelDraw::hideAllMuzzleFlashes); the field is
    // `WeaponMuzzleFlash = SLOT BONE`.
    for v in field_values(state, "WeaponMuzzleFlash") {
        if let Some(bone) = v.get(1) {
            hide.push(bone.clone());
        }
    }
    let show = field_values(state, "ShowSubObject").flatten().collect();
    let animation = field_values(state, "IdleAnimation")
        .chain(field_values(state, "Animation"))
        .next()
        .and_then(|v| v.into_iter().next());
    Some(DrawModel { module: String::new(), model, hide, show, animation })
}
