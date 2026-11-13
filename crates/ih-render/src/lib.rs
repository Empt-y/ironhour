//! Renderer for the original game's assets on wgpu.
//!
//! The first goal is the original look: W3D meshes drawn with the fixed-function
//! lighting, texture combining and blending WW3D2 set up on Direct3D 8. Each module
//! names the C++ it follows.

pub mod assets;
pub mod gpu;
pub mod house_color;
pub mod model;
pub mod pose;

use glam::{Mat4, Vec3};

pub use assets::Assets;
pub use gpu::{Camera, Lights, Renderer};
pub use model::{BuildOptions, Model, build};

/// World-space bounds of a posed model's vertices.
pub fn bounds(model: &model::Model, bones: &[Mat4]) -> Option<(Vec3, Vec3)> {
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for b in &model.batches {
        for &i in &b.indices {
            let v = &b.vertices[i as usize];
            let m = bones.get(v.bone as usize).copied().unwrap_or(Mat4::IDENTITY);
            let p = m.transform_point3(Vec3::from(v.position));
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    (lo.x <= hi.x).then_some((lo, hi))
}

/// A camera looking at the bounds from the given yaw and pitch (degrees), far enough
/// back to fit them.
pub fn framing_camera(lo: Vec3, hi: Vec3, yaw_deg: f32, pitch_deg: f32) -> Camera {
    let center = (lo + hi) * 0.5;
    let radius = ((hi - lo).length() * 0.5).max(1.0);
    let fov_y = 30f32.to_radians();
    let dist = radius / (fov_y * 0.5).sin() * 1.05;
    let (yaw, pitch) = (yaw_deg.to_radians(), pitch_deg.to_radians());
    let dir = Vec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin());
    Camera { eye: center + dir * dist, target: center, fov_y, near: dist * 0.05, far: dist * 4.0 }
}
