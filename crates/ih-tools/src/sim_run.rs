//! Headless command runner and per-tick determinism comparison.
use anyhow::{Context, Result, bail};
use ih_ini::definitions::Definitions;
use ih_sim::{scenario::Scenario, templates::Templates};
use ih_vfs::Vfs;
use std::{collections::BTreeMap, io::Write, path::Path, sync::Arc};
/// `watch`: print one object's movement state every N frames (default 15) (diagnostics).
pub fn run(vfs: &Vfs, language: &str, path: &Path, trace: Option<&Path>, watch: Option<(u32, u32)>) -> Result<()> {
    let scenario: Scenario = toml::from_str(&std::fs::read_to_string(path)?).context("invalid simulation scenario")?;
    let world = ih_client::World::load(vfs, &scenario.map)?;
    let mut defs = Definitions::load(vfs, language);
    defs.load_map_overrides(vfs, &world.path)?;
    let templates = Arc::new(Templates::for_map(&defs, &world.map, vfs, language)?);
    for w in &templates.warnings {
        eprintln!("template: {w}");
    }
    let map = Arc::new(world.map);
    let mut a = scenario.instantiate(map.clone(), templates.clone())?;
    let mut b = scenario.instantiate(map, templates)?;
    eprintln!(
        "navigation: {}x{} cells; {} blocked; {} bridge layers",
        a.grid.width,
        a.grid.height,
        a.grid.blocked.iter().filter(|b| **b).count(),
        a.grid.layers.len()
    );
    {
        use ih_sim::footprint::CellType;
        let o = &a.grid.obstacles;
        let mut fates = std::collections::BTreeMap::new();
        for m in &a.map.objects {
            *fates.entry(ih_sim::map_objects::fate(m, &a.templates)).or_insert(0usize) += 1;
        }
        eprintln!(
            "map objects: {fates:?}; obstacle cells {}, pinched shut {}, border pinched {}",
            o.count(CellType::Obstacle),
            o.count(CellType::Impassable),
            o.pinched.iter().filter(|p| **p).count()
        );
    }
    let mut trace = trace
        .map(|p| -> Result<_> {
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            Ok(std::io::BufWriter::new(std::fs::File::create(p)?))
        })
        .transpose()?;
    if let Some(f) = &mut trace {
        writeln!(f, "# ironhour state-hash v4; map={}; seed={}", scenario.map, scenario.seed)?;
    }
    let mut entered = BTreeMap::new();
    let mut traversals = 0u32;
    for frame in 0..=scenario.frames {
        scenario.verify_frame(&a)?;
        if scenario.require_no_overlaps {
            let overlaps = a.overlaps();
            if !overlaps.is_empty() {
                bail!("object overlap at frame {frame}: {overlaps:?}");
            }
        }
        let (ha, hb) = (a.state_hash(), b.state_hash());
        if ha != hb {
            bail!("determinism mismatch at frame {frame}: {ha:016x} != {hb:016x}");
        }
        if let Some(f) = &mut trace {
            writeln!(f, "{frame:08} {ha:016x}")?;
        }
        if let Some((o, _)) = watch
            .and_then(|(id, every)| a.objects.get(&id).map(|o| (o, every)))
            .filter(|(_, every)| frame % (*every).max(1) == 0)
        {
            println!(
                "{frame:5} layer {} next {:?} goal {:?} pos {:8.2} {:8.2} {:6.2} angle {:6.3} vel {:6.3} {:6.3} speed {:6.3} braking {} {:?} path {} blocked {} by {:?}",
                o.layer,
                o.path.get(1).map(|w| (w.position, w.layer)),
                o.target,
                o.position[0],
                o.position[1],
                o.position[2],
                o.angle,
                o.physics.vel[0],
                o.physics.vel[1],
                o.speed,
                o.loco.is_braking,
                o.ai.state,
                o.path.len(),
                o.blocked_frames,
                o.blocked_by
            );
        }
        if frame < scenario.frames {
            let previous: BTreeMap<_, _> = a.objects.values().map(|o| (o.id, o.layer)).collect();
            a.step();
            b.step();
            for o in a.objects.values() {
                let old = previous.get(&o.id).copied().unwrap_or(o.layer);
                if old == 0 && o.layer != 0 {
                    let bridge = &a.grid.layers[o.layer as usize - 1].bridge;
                    let end = bridge.local([o.position[0], o.position[1]])[0] > bridge.length() * 0.5;
                    entered.insert(o.id, (o.layer, end));
                } else if old != 0
                    && o.layer == 0
                    && let Some((layer, end)) = entered.remove(&o.id)
                {
                    let bridge = &a.grid.layers[old as usize - 1].bridge;
                    let exit = bridge.local([o.position[0], o.position[1]])[0] > bridge.length() * 0.5;
                    if layer == old && end != exit {
                        traversals += 1;
                    }
                }
            }
            entered.retain(|id, _| a.objects.contains_key(id));
        }
    }
    for event in &a.events {
        eprintln!("rejected: {event}");
    }
    println!(
        "{}: {} frames, {} objects, every-tick hashes matched; final {:016x}",
        scenario.map,
        scenario.frames,
        a.objects.len(),
        a.state_hash()
    );
    for o in a.objects.values() {
        println!(
            "  {} {} at {:.3},{:.3},{:.3}; layer {}; {}; {} traffic repaths; blocked {:?} for {} ticks",
            o.id,
            o.template,
            o.position[0],
            o.position[1],
            o.position[2],
            o.layer,
            if o.target.is_some() { "moving" } else { "idle" },
            o.traffic_repaths,
            o.blocked_by,
            o.blocked_frames
        );
        println!(
            "    AI {:?}, outcome {:?}; completed {}, interrupted {}, failed {}; disabled {:?}",
            o.ai.state,
            o.ai.outcome,
            o.ai.completed_orders,
            o.ai.interrupted_orders,
            o.ai.failed_orders,
            o.disabled.names()
        );
    }
    println!("stub module instances (loaded, gameplay not implemented): {:?}", a.stub_counts);
    println!("crushed (frame, victim, crusher): {:?}; move-asides: {}", a.crushed, a.move_asides);
    let money: Vec<(u32, u32)> = a.players.iter().map(|(id, p)| (*id, p.money)).collect();
    println!("money (player, amount): {money:?}");
    for o in a.objects.values().filter(|o| o.experience.points > 0) {
        println!("  {} {} experience {} (level {})", o.id, o.template, o.experience.points, o.experience.level);
    }
    for (id, p) in a.players.iter().filter(|(_, p)| !p.tunnels.tunnels.is_empty()) {
        println!("  player {id} tunnels {:?} hold {:?}", p.tunnels.tunnels, p.tunnels.riders);
    }
    for o in a.objects.values().filter(|o| o.contain.as_ref().is_some_and(|c| !c.riders.is_empty())) {
        println!("  {} {} (owner {}) carries {:?}", o.id, o.template, o.owner, o.contain.as_ref().unwrap().riders);
    }
    for o in a.objects.values().filter(|o| o.owner != 0) {
        if let Some(h) = o.health.as_ref().filter(|h| h.current < h.max || o.sole_healer.is_some()) {
            println!("  {} {} health {:.1} of {:.1}", o.id, o.template, h.current, h.max);
        }
    }
    if !a.sell_list.is_empty() {
        println!("  being sold: {:?}", a.sell_list);
    }
    for o in a.objects.values().filter(|o| o.builder.is_some()) {
        match o.construction {
            Some(p) => println!("  {} {} under construction, {p:.1}% (builder {:?})", o.id, o.template, o.builder),
            None => println!("  {} {} built by {:?}", o.id, o.template, o.builder),
        }
    }
    for (id, p) in &a.players {
        if !p.upgrades.completed.is_empty() || !p.upgrades.in_progress.is_empty() {
            println!(
                "  player {id} upgrades {:?}, in research {:?}, power {:?}",
                p.upgrades.completed,
                p.upgrades.in_progress,
                a.energy(*id)
            );
        }
    }
    for o in a.objects.values().filter(|o| !o.upgrades.completed.is_empty()) {
        println!("  object {} {} upgrades {:?}", o.id, o.template, o.upgrades.completed);
    }
    for o in a.objects.values().filter(|o| o.upgrades.executed.iter().any(|e| *e)) {
        let ran: Vec<&str> = o
            .upgrades
            .modules
            .iter()
            .zip(&o.upgrades.executed)
            .filter(|(_, e)| **e)
            .map(|(m, _)| m.class.as_str())
            .collect();
        let weapons: Vec<&str> = o.weapon_set.weapons.iter().flatten().map(|w| w.template.as_str()).collect();
        println!("  object {} {} ran {ran:?}; weapons {weapons:?}", o.id, o.template);
    }
    for o in a.objects.values() {
        if let Some(t) = &o.truck {
            println!("  truck {} {:?}: {} boxes, AI {:?}", o.id, t.state, t.boxes, o.ai.state);
        }
        if let Some(d) =
            o.dock.as_ref().filter(|d| d.kind == ih_sim::supply::DockKind::Warehouse && d.boxes != d.starting_boxes)
        {
            println!("  warehouse {} {}: {} of {} boxes left", o.id, o.template, d.boxes, d.starting_boxes);
        }
    }
    println!("completed bridge traversals: {traversals}");
    let repaths: u32 = a.objects.values().map(|o| o.traffic_repaths).sum();
    if !a.events.is_empty() {
        bail!("{} commands were rejected", a.events.len());
    }
    if scenario.require_idle_at_end && a.objects.values().any(|o| !o.is_idle()) {
        bail!("scenario ended before all move orders completed");
    }
    if scenario.require_no_failed_orders && a.failed_orders != 0 {
        bail!("{} AI orders failed", a.failed_orders);
    }
    if traversals < scenario.require_bridge_traversals {
        bail!(
            "expected at least {} completed bridge traversals, observed {traversals}",
            scenario.require_bridge_traversals
        );
    }
    if repaths < scenario.require_traffic_repaths {
        bail!("expected at least {} traffic repaths, observed {repaths}", scenario.require_traffic_repaths);
    }
    Ok(())
}
