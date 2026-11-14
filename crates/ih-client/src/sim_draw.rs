//! Client snapshots for the initial M4 movement slice. Logic remains in ih-sim;
//! interpolation, MOVING/idle art and animation are render-only (`W3DModelDraw.cpp`).
use crate::drawable::condition_model;
use glam::{Mat4, Vec3};
use ih_formats::w3d::{Animation, Hierarchy};
use ih_render::{Assets, BuildOptions, PreparedModel, Renderer, Scene};
use ih_sim::{Simulation, templates::Templates};
use std::{collections::BTreeMap, sync::Arc};

struct Art {
    model: Arc<PreparedModel>,
    hierarchy: Option<Hierarchy>,
    animation: Option<Animation>,
    scale: f32,
}
pub struct SimDrawables {
    art: BTreeMap<(String, bool), Vec<Art>>,
    static_instances: usize,
    rings: BTreeMap<String, Arc<PreparedModel>>,
}
impl SimDrawables {
    pub fn new(
        renderer: &mut Renderer,
        assets: &mut Assets,
        templates: &Templates,
        names: impl Iterator<Item = String>,
        flags: &[&str],
        static_instances: usize,
    ) -> Self {
        let mut out = Self { art: BTreeMap::new(), static_instances, rings: BTreeMap::new() };
        for name in names {
            let Some(t) = templates.objects.get(&name) else { continue };
            if !out.rings.contains_key(&name) {
                out.rings.insert(name.clone(), renderer.prepare_model(assets, &ring(t.radius().max(4.0))));
            }
            for moving in [false, true] {
                if out.art.contains_key(&(name.clone(), moving)) {
                    continue;
                }
                let mut wanted = flags.to_vec();
                if moving {
                    wanted.push("MOVING");
                }
                let mut draws = vec![];
                for m in t.modules.iter().filter(|m| m.kind == "Draw") {
                    let dm = condition_model(&m.block, &wanted);
                    let Some(dm) = dm else { continue };
                    let mut hide = dm.hide;
                    hide.extend(["TURRETFX".into(), "MUZZLE".into()]);
                    let options = BuildOptions { house_color: Some(0x1e5aff), hide };
                    match ih_render::build(assets, &dm.model, &options) {
                        Ok(model) => {
                            let animation = dm.animation.as_deref().and_then(|a| assets.animation(a));
                            draws.push(Art {
                                model: renderer.prepare_model(assets, &model),
                                hierarchy: model.hierarchy,
                                animation,
                                scale: t.number("Scale", 1.0),
                            });
                        }
                        Err(e) => assets.warnings.push(format!("{}: {e}", dm.model)),
                    }
                }
                out.art.insert((name.clone(), moving), draws);
            }
        }
        out
    }
    pub fn update(
        &self,
        renderer: &Renderer,
        scene: &mut Scene,
        sim: &Simulation,
        alpha: f32,
        time_ms: f32,
        selected: Option<u32>,
    ) {
        scene.instances.truncate(self.static_instances);
        for o in sim.objects.values() {
            let moving = o.velocity[0] * o.velocity[0] + o.velocity[1] * o.velocity[1] > 0.000001;
            let Some(art) = self.art.get(&(o.template.clone(), moving)) else { continue };
            let previous = Vec3::from(o.previous_position);
            let current = Vec3::from(o.position);
            let p = previous.lerp(current, alpha.clamp(0.0, 1.0));
            for a in art {
                let root =
                    Mat4::from_translation(p) * Mat4::from_rotation_z(o.angle) * Mat4::from_scale(Vec3::splat(a.scale));
                let bones = match (&a.hierarchy, &a.animation) {
                    (Some(h), Some(anim)) => {
                        let frame = (time_ms * anim.frame_rate as f32 / 1000.0) % (anim.num_frames.max(1) as f32);
                        ih_render::pose::animated_pose(h, anim, frame, root).0
                    }
                    (Some(h), None) => ih_render::pose::base_pose(h, root),
                    _ => vec![root],
                };
                scene.instances.push(renderer.instance(&a.model, &bones));
            }
            if selected == Some(o.id)
                && let Some(ring) = self.rings.get(&o.template)
            {
                scene.instances.push(renderer.instance(ring, &[Mat4::from_translation(p + Vec3::Z * 0.2)]));
            }
        }
    }
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
        batches: vec![Batch {
            mesh: "selection".into(),
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
