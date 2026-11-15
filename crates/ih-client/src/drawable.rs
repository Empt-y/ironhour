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

/// The best state for `wanted` (`W3DModelDraw::findBestInfo`) as a static model:
/// its hidden sub-objects (plus muzzle flashes) and first animation.
pub(crate) fn condition_model(d: &Block, wanted: &[&str]) -> Option<DrawModel> {
    use crate::condition::{Flags, ModelDrawData};
    let data = ModelDrawData::parse(d);
    let flags: Flags = wanted.iter().map(|w| w.to_ascii_uppercase()).collect();
    let state = &data.states[data.find_best(&flags)?];
    if state.model.is_empty() {
        return None;
    }
    let mut hide: Vec<String> = state.hide_show.iter().filter(|(_, h)| *h).map(|(n, _)| n.clone()).collect();
    hide.extend(state.muzzle_flashes.iter().cloned());
    let show = state.hide_show.iter().filter(|(_, h)| !*h).map(|(n, _)| n.clone()).collect();
    let animation = state.animations.first().map(|a| a.name.clone());
    Some(DrawModel { module: String::new(), model: state.model.clone(), hide, show, animation })
}
