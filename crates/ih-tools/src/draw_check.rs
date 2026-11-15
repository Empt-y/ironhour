//! `draw-check`: parse every object template's model condition states (as
//! `W3DModelDraw` does) and report states without an idle match and models or
//! animations that do not resolve.
use anyhow::Result;
use ih_client::condition::{Flags, ModelDrawData};
use ih_ini::definitions::Definitions;
use ih_sim::templates::Templates;
use ih_vfs::Vfs;
use std::collections::BTreeSet;

pub fn run(vfs: &Vfs, language: &str) -> Result<()> {
    let defs = Definitions::load(vfs, language);
    let templates = Templates::from_definitions(&defs);
    let mut assets = ih_render::Assets::new(vfs, language);
    let (mut modules, mut states, mut transitions, mut no_idle) = (0, 0, 0, Vec::new());
    let mut models = BTreeSet::new();
    let mut anims = BTreeSet::new();
    for (name, t) in &templates.objects {
        for m in t.modules.iter().filter(|m| m.kind == "Draw") {
            let d = ModelDrawData::parse(&m.block);
            if d.states.is_empty() {
                continue;
            }
            modules += 1;
            states += d.states.len();
            transitions += d.transitions.len();
            if d.find_best(&Flags::new()).is_none() {
                no_idle.push(format!("{name} {}", m.tag));
            }
            for s in d.states.iter().chain(d.transitions.values()) {
                if !s.model.is_empty() {
                    models.insert(s.model.clone());
                }
                anims.extend(s.animations.iter().map(|a| a.name.clone()));
            }
        }
    }
    let missing_models: Vec<_> = models.iter().filter(|m| assets.w3d(m).is_none()).cloned().collect();
    let missing_anims: Vec<_> = anims.iter().filter(|a| assets.animation(a).is_none()).cloned().collect();
    println!(
        "{} templates; {modules} draw modules with states: {states} states, {transitions} transitions",
        templates.objects.len()
    );
    println!(
        "{} distinct models ({} missing), {} animations ({} missing)",
        models.len(),
        missing_models.len(),
        anims.len(),
        missing_anims.len()
    );
    println!("draw modules without an idle state: {}", no_idle.len());
    for n in no_idle.iter().take(20) {
        println!("  {n}");
    }
    for m in missing_models.iter().take(20) {
        println!("  missing model {m}");
    }
    for a in missing_anims.iter().take(20) {
        println!("  missing animation {a}");
    }
    Ok(())
}
