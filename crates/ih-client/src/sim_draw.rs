//! Drawables for simulated objects. Logic stays in ih-sim; interpolation, model
//! condition states, animation and sub-object visibility are render-only
//! (`W3DModelDraw.cpp`, see [`crate::condition`]).
use crate::condition::{ArtSource, Flags, ModelDrawData, ModelDrawState, SubObjects};
use glam::{Mat4, Vec3};
use ih_formats::w3d::{Animation, Hierarchy};
use ih_render::{Assets, BuildOptions, PreparedModel, Renderer, Scene};
use ih_sim::{Simulation, templates::Templates};
use std::{collections::BTreeMap, sync::Arc};

/// A loaded model: GPU geometry, skeleton and sub-objects.
struct ModelArt {
    prepared: Arc<PreparedModel>,
    hierarchy: Option<Hierarchy>,
    subs: Vec<(String, u32)>,
    parents: Vec<Option<u32>>,
}

/// Every model and animation the spawned templates' states can use, loaded up
/// front so states can switch while the viewer runs.
#[derive(Default)]
struct ArtCache {
    models: BTreeMap<String, ModelArt>,
    animations: BTreeMap<String, Arc<Animation>>,
}

impl ArtSource for ArtCache {
    fn animation_info(&mut self, name: &str) -> Option<(u32, f32)> {
        self.animations.get(name).map(|a| (a.num_frames, a.frame_rate as f32))
    }
    fn sub_objects(&mut self, model: &str) -> Option<SubObjects> {
        self.models.get(model).map(|m| (m.subs.clone(), m.parents.clone()))
    }
}

/// A template's draw modules.
struct TemplateDraws {
    modules: Vec<ModelDrawData>,
    /// Per module: truck wheel data (`W3DTruckDraw`, `W3DTankTruckDraw`).
    trucks: Vec<Option<crate::truck::TruckDraw>>,
    scale: f32,
}

struct ObjectDraw {
    states: Vec<ModelDrawState>,
    flags: Option<Flags>,
    last_ms: f32,
    /// Chassis motion: state, the last two logic frames' results, and the frame stepped.
    loco: crate::chassis::LocoInfo,
    xform: [crate::chassis::Xform; 2],
    /// Per module: truck wheel state.
    trucks: Vec<crate::truck::TruckState>,
    stepped: Option<u32>,
}

pub struct SimDrawables {
    templates: BTreeMap<String, TemplateDraws>,
    art: ArtCache,
    objects: BTreeMap<u32, ObjectDraw>,
    base_flags: Flags,
    static_instances: usize,
    rings: BTreeMap<String, Arc<PreparedModel>>,
    /// `GameClientRandomValue` for chassis bounces.
    rng: crate::condition::ClientRng,
}

impl SimDrawables {
    /// `flags` are the map-wide condition flags (`NIGHT`, `SNOW`).
    pub fn new(
        renderer: &mut Renderer,
        assets: &mut Assets,
        templates: &Templates,
        names: impl Iterator<Item = String>,
        flags: &[&str],
        static_instances: usize,
    ) -> Self {
        let mut out = Self {
            templates: BTreeMap::new(),
            art: ArtCache::default(),
            objects: BTreeMap::new(),
            base_flags: flags.iter().map(|f| f.to_string()).collect(),
            static_instances,
            rings: BTreeMap::new(),
            rng: crate::condition::ClientRng::new(0x5eed),
        };
        for name in names {
            if out.templates.contains_key(&name) {
                continue;
            }
            let Some(t) = templates.objects.get(&name) else { continue };
            out.rings.insert(name.clone(), renderer.prepare_model(assets, &ring(t.radius().max(4.0))));
            let (modules, trucks): (Vec<ModelDrawData>, Vec<_>) = t
                .modules
                .iter()
                .filter(|m| m.kind == "Draw")
                .map(|m| (ModelDrawData::parse(&m.block), crate::truck::TruckDraw::parse(&m.class, &m.block)))
                .filter(|(d, _)| !d.states.is_empty())
                .unzip();
            for d in &modules {
                for s in d.states.iter().chain(d.transitions.values()) {
                    out.load_model(renderer, assets, &s.model);
                    for a in &s.animations {
                        if !out.art.animations.contains_key(&a.name) {
                            match assets.animation(&a.name) {
                                Some(anim) => {
                                    out.art.animations.insert(a.name.clone(), Arc::new(anim));
                                }
                                None => assets.warnings.push(format!("{name}: animation {} not found", a.name)),
                            }
                        }
                    }
                }
            }
            out.templates.insert(name.clone(), TemplateDraws { modules, trucks, scale: t.number("Scale", 1.0) });
        }
        out
    }

    fn load_model(&mut self, renderer: &mut Renderer, assets: &mut Assets, model: &str) {
        if model.is_empty() || self.art.models.contains_key(model) {
            return;
        }
        let options = BuildOptions { house_color: Some(0x1e5aff), hide: vec![] };
        match ih_render::build(assets, model, &options) {
            Ok(m) => {
                let parents =
                    m.hierarchy.as_ref().map(|h| h.pivots.iter().map(|p| p.parent).collect()).unwrap_or_default();
                let subs = m.sub_objects.iter().map(|s| (s.name.clone(), s.bone)).collect();
                let prepared = renderer.prepare_model(assets, &m);
                self.art.models.insert(model.to_string(), ModelArt { prepared, hierarchy: m.hierarchy, subs, parents });
            }
            Err(e) => assets.warnings.push(format!("{model}: {e}")),
        }
    }

    /// The object's condition flags: map-wide ones plus `MOVING` while a move order
    /// is underway and not blocked for more than a quarter second
    /// (`AIInternalMoveToState`).
    fn flags_for(&self, o: &ih_sim::simulation::Object) -> Flags {
        let mut f = self.base_flags.clone();
        if o.target.is_some() && o.blocked_frames <= 7 && !o.disabled.any() {
            f.insert("MOVING".into());
        }
        f
    }

    pub fn update(
        &mut self,
        renderer: &Renderer,
        scene: &mut Scene,
        sim: &Simulation,
        alpha: f32,
        time_ms: f32,
        selected: &[u32],
    ) {
        scene.instances.truncate(self.static_instances);
        self.objects.retain(|id, _| sim.objects.contains_key(id));
        // The map's own objects are still drawn by the static scene (they cannot
        // change until M5), so only objects created by commands or scenarios draw here.
        for o in sim.objects.values().filter(|o| o.map_index.is_none()) {
            let Some(td) = self.templates.get(&o.template) else { continue };
            let flags = self.flags_for(o);
            let entry = self.objects.entry(o.id).or_insert_with(|| ObjectDraw {
                states: (0..td.modules.len()).map(|i| ModelDrawState::new(u64::from(o.id) << 8 | i as u64)).collect(),
                flags: None,
                last_ms: time_ms,
                loco: Default::default(),
                xform: Default::default(),
                trucks: vec![Default::default(); td.modules.len()],
                stepped: None,
            });
            let dt = (time_ms - entry.last_ms).max(0.0);
            entry.last_ms = time_ms;
            let changed = entry.flags.as_ref() != Some(&flags);
            let speed = Vec3::new(o.physics.vel[0], o.physics.vel[1], 0.0).length();
            let previous = Vec3::from(o.previous_position);
            let current = Vec3::from(o.position);
            let p = previous.lerp(current, alpha.clamp(0.0, 1.0));
            // applyPhysicsXform, stepped once per logic frame and interpolated.
            if entry.stepped != Some(sim.frame) {
                let x = chassis_step(sim, o, &mut entry.loco, &mut self.rng);
                // W3DTruckDraw::doDrawModule's wheel part, once per logic frame too.
                let motion = truck_motion(sim, o);
                for (state, data) in entry.trucks.iter_mut().zip(&td.trucks) {
                    if let Some(d) = data {
                        crate::truck::step(d, state, &entry.loco.wheels, &motion);
                    }
                }
                entry.xform = if entry.stepped.is_some() { [entry.xform[1], x] } else { [x, x] };
                entry.stepped = Some(sim.frame);
            }
            let x = entry.xform[0].lerp(entry.xform[1], alpha.clamp(0.0, 1.0));
            let root = Mat4::from_translation(p)
                * Mat4::from_rotation_z(o.angle)
                * Mat4::from_scale(Vec3::splat(td.scale))
                * x.matrix();
            for (index, (state, data)) in entry.states.iter_mut().zip(&td.modules).enumerate() {
                if changed {
                    state.set_conditions(data, &flags, &mut self.art);
                }
                state.update(data, dt, speed, &mut self.art);
                let Some(art) = self.art.models.get(&state.model) else { continue };
                let anim = state.playback.anim.as_ref().and_then(|a| self.art.animations.get(a));
                let bones = match &art.hierarchy {
                    Some(h) => {
                        let controls = match &td.trucks[index] {
                            Some(d) => crate::truck::controls(d, &entry.trucks[index], &entry.loco.wheels, h),
                            None => Vec::new(),
                        };
                        ih_render::pose::controlled_pose(h, anim.map(|a| &**a), state.playback.frame, root, &controls).0
                    }
                    None => vec![root],
                };
                let hidden: Arc<[bool]> = state.hidden.clone().into();
                scene.instances.push(renderer.instance_hiding(&art.prepared, &bones, hidden));
            }
            entry.flags = Some(flags);
            if selected.contains(&o.id)
                && let Some(ring) = self.rings.get(&o.template)
            {
                scene.instances.push(renderer.instance(ring, &[Mat4::from_translation(p + Vec3::Z * 0.2)]));
            }
        }
    }
}

/// What the truck wheels read from the logic object this frame.
fn truck_motion(sim: &Simulation, o: &ih_sim::simulation::Object) -> crate::truck::TruckMotion {
    let v = o.physics.vel;
    let at = ih_sim::terrain::Waypoint { position: [o.position[0], o.position[1]], layer: o.layer };
    let (height, _) = sim.grid.height_and_normal(&sim.map, at);
    crate::truck::TruckMotion {
        speed: (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt(),
        backwards: o.loco.moving_backwards,
        motive: o.physics.motive_expires > sim.frame,
        airborne: o.position[2] - height > -9.0 * ih_sim::locomotion::DEFAULT_GRAVITY,
        turning: o.physics.turning,
        angle_to_path: o.path.get(1).map(|p| {
            ih_math::angle_delta(o.angle, ih_math::atan2(p.position[1] - o.position[1], p.position[0] - o.position[0]))
        }),
    }
}

/// One `calcPhysicsXform` step for a logic object (identity without a chassis model,
/// an AI locomotor and physics, or while held).
fn chassis_step(
    sim: &Simulation,
    o: &ih_sim::simulation::Object,
    info: &mut crate::chassis::LocoInfo,
    rng: &mut crate::condition::ClientRng,
) -> crate::chassis::Xform {
    use ih_sim::simulation::ModuleKind;
    let t = &sim.templates.objects[&o.template];
    let Some(l) = t.locomotor().and_then(|n| sim.templates.locomotors.get(n)) else { return Default::default() };
    let held = o.disabled.mask & (1 << ih_sim::disabled::DisabledReason::Held as u32) != 0;
    let has = |k| o.modules.iter().any(|m| m.kind == k);
    if held || !has(ModuleKind::Steering) || !has(ModuleKind::Physics) {
        return Default::default();
    }
    let at = ih_sim::terrain::Waypoint { position: [o.position[0], o.position[1]], layer: o.layer };
    let (height, normal) = sim.grid.height_and_normal(&sim.map, at);
    let height_above = o.position[2] - height;
    let m = crate::chassis::Motion {
        dir: [ih_math::cos(o.angle), ih_math::sin(o.angle)],
        vel: o.physics.vel,
        accel: o.physics.prev_accel,
        motive: o.physics.motive_expires > sim.frame,
        height_above,
        normal,
        airborne: height_above > -9.0 * ih_sim::locomotion::DEFAULT_GRAVITY,
        max_speed: l.speed,
        length: t.number("GeometryMajorRadius", 1.0),
        width: t.number("GeometryMinorRadius", 1.0),
        position: [o.position[0], o.position[1]],
        radius: o.body.radius,
        // Shrubbery is smashed through without a bump.
        overlap: sim.objects.get(&o.physics.current_overlap).filter(|v| v.id != 0).and_then(|v| {
            let vt = &sim.templates.objects[&v.template];
            (!vt.has_kind("SHRUBBERY")).then(|| crate::chassis::Overlap {
                position: [v.position[0], v.position[1]],
                radius: v.body.radius,
                height: v.body.top,
                // Crushed front and back (body damage) comes with M5.
                flat: vt.has_kind("LOW_OVERLAPPABLE") || vt.has_kind("INFANTRY"),
            })
        }),
        previous_overlap: o.physics.previous_overlap != 0,
        turning: o.physics.turning,
        backwards: o.loco.moving_backwards,
    };
    crate::chassis::step(&l.chassis, info, &m, rng).unwrap_or_default()
}

fn ring(radius: f32) -> ih_render::Model {
    use ih_render::model::{Batch, Mapper, Model, Vertex};
    let mut vertices = vec![];
    let mut indices = vec![];
    for i in 0..48 {
        let base = vertices.len() as u32;
        for (angle, r) in [(i, radius - 0.6), (i, radius + 0.6), (i + 1, radius + 0.6), (i + 1, radius - 0.6)] {
            let angle = angle as f32 * std::f32::consts::TAU / 48.0;
            vertices.push(Vertex {
                position: [angle.cos() * r, angle.sin() * r, 0.0],
                normal: [0.0, 0.0, 1.0],
                uv0: [0.0; 2],
                uv1: [0.0; 2],
                diffuse: [0.2, 1.0, 0.25, 1.0],
                ambient: [0.0; 3],
                emissive: [0.0; 3],
                bone: 0,
            });
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    Model {
        name: "selection".into(),
        hierarchy: None,
        house_color: None,
        sub_objects: vec![ih_render::model::SubObject { name: "selection".into(), bone: 0 }],
        batches: vec![Batch {
            mesh: "selection".into(),
            sub_object: 0,
            pass: 0,
            vertices,
            indices,
            shader: ih_render::model::default_shader(),
            lighting: false,
            textures: [None, None],
            mappers: [Mapper::Uv, Mapper::Uv],
            two_sided: true,
            sort_level: 0,
        }],
    }
}
