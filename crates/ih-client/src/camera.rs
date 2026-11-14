//! Tactical map inspection camera. Defaults follow `W3DView.cpp::init` and
//! GameData's height/pitch/yaw; controls are frame-rate independent and allow
//! zooming out to inspect a whole map. Scripted cameras come with the simulation.
use crate::Definitions;
use glam::{Vec2, Vec3};
use ih_formats::map::Map;
use ih_render::Camera;

#[derive(Clone, Debug)]
pub struct TacticalCamera {
    pub center: Vec2,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub fov_y: f32,
    bounds: (Vec2, Vec2),
}

impl TacticalCamera {
    /// Ray/terrain pick for move orders. Screen coordinates are physical pixels.
    pub fn ground_at_pixel(&self, map: &Map, size: [u32; 2], pixel: Vec2) -> Option<Vec2> {
        let (a, dir) = self.ray_at_pixel(map, size, pixel)?;
        let camera = self.camera(map);
        if dir.z >= 0.0 {
            return None;
        }
        let mut previous = 0.0;
        for step in 1..=512 {
            let distance = camera.far * step as f32 / 512.0;
            let p = a + dir * distance;
            if p.z <= map.ground_height(p.x, p.y) {
                let (mut lo, mut hi) = (previous, distance);
                for _ in 0..20 {
                    let mid = (lo + hi) * 0.5;
                    let p = a + dir * mid;
                    if p.z > map.ground_height(p.x, p.y) {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                let p = (a + dir * ((lo + hi) * 0.5)).truncate();
                return (p.cmpge(self.bounds.0).all() && p.cmple(self.bounds.1).all()).then_some(p);
            }
            previous = distance;
        }
        None
    }
    /// Pick the nearest visible terrain/deck surface for a move order.
    pub fn surface_at_pixel(
        &self,
        map: &Map,
        terrain: &ih_sim::terrain::Terrain,
        size: [u32; 2],
        pixel: Vec2,
    ) -> Option<Vec2> {
        let ground = self.ground_at_pixel(map, size, pixel);
        let (origin, dir) = self.ray_at_pixel(map, size, pixel)?;
        let mut distance = ground.map_or(f32::INFINITY, |p| (p.extend(map.ground_height(p.x, p.y)) - origin).dot(dir));
        let mut result = ground;
        for layer in &terrain.layers {
            if let Some((t, p)) = layer.bridge.ray_hit(origin.to_array(), dir.to_array())
                && t < distance
            {
                distance = t;
                result = Some(Vec2::from(p));
            }
        }
        result
    }
    pub fn ray_at_pixel(&self, map: &Map, size: [u32; 2], pixel: Vec2) -> Option<(Vec3, Vec3)> {
        if size[0] == 0 || size[1] == 0 {
            return None;
        }
        let camera = self.camera(map);
        let inv = camera.view_proj(size[0] as f32 / size[1] as f32).inverse();
        let xy = Vec2::new(pixel.x / size[0] as f32 * 2.0 - 1.0, 1.0 - pixel.y / size[1] as f32 * 2.0);
        let a = inv * xy.extend(0.0).extend(1.0);
        let b = inv * xy.extend(1.0).extend(1.0);
        let a = a.truncate() / a.w;
        let b = b.truncate() / b.w;
        let dir = (b - a).normalize_or_zero();
        (a.is_finite() && dir.is_finite() && dir.length_squared() > 0.0).then_some((a, dir))
    }
    pub fn new(bounds: ([f32; 2], [f32; 2]), defs: &Definitions) -> Self {
        let num = |field: &str, default: f32| {
            defs.field("GameData", "", field)
                .and_then(|v| ih_ini::values::scan_real(v.first()?.as_bytes()).ok())
                .unwrap_or(default)
        };
        let pitch = num("CameraPitch", 37.5).clamp(15.0, 85.0).to_radians();
        let (lo, hi) = (Vec2::from(bounds.0), Vec2::from(bounds.1));
        Self {
            center: (lo + hi) * 0.5,
            yaw: num("CameraYaw", 0.0).to_radians(),
            pitch,
            distance: num("CameraHeight", 232.0) / pitch.sin(),
            fov_y: 50.0f32.to_radians(),
            bounds: (lo, hi),
        }
    }
    pub fn camera(&self, map: &Map) -> Camera {
        let target = self.center.extend(map.ground_height(self.center.x, self.center.y));
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let eye = target + Vec3::new(sy * cp, -cy * cp, sp) * self.distance;
        Camera {
            eye,
            target,
            fov_y: self.fov_y,
            near: 1.0,
            far: (self.bounds.1 - self.bounds.0).length() * 2.0 + self.distance * 4.0 + 500.0,
        }
    }
    /// Local x = screen right; local y = screen up.
    pub fn pan(&mut self, local: Vec2, seconds: f32, fast: bool) {
        let local = local.clamp_length_max(1.0);
        let (s, c) = self.yaw.sin_cos();
        let delta = Vec2::new(c, s) * local.x + Vec2::new(-s, c) * local.y;
        self.center = (self.center + delta * self.distance * seconds * if fast { 2.0 } else { 0.8 })
            .clamp(self.bounds.0, self.bounds.1);
    }
    pub fn drag_pan(&mut self, delta: Vec2, viewport_height: f32) {
        let scale = 2.0 * self.distance * (self.fov_y * 0.5).tan() / viewport_height.max(1.0);
        let (s, c) = self.yaw.sin_cos();
        self.center = (self.center + scale * (Vec2::new(c, s) * -delta.x + Vec2::new(-s, c) * delta.y))
            .clamp(self.bounds.0, self.bounds.1);
    }
    pub fn zoom(&mut self, steps: f32) {
        if steps.is_finite() {
            self.distance = (self.distance * (-steps * 0.15).exp()).clamp(80.0, 15000.0);
        }
    }
    pub fn orbit(&mut self, yaw_delta: f32, pitch_delta: f32) {
        self.yaw = (self.yaw + yaw_delta).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + pitch_delta).clamp(15.0f32.to_radians(), 85.0f32.to_radians());
    }
    pub fn overview(&mut self) {
        self.center = (self.bounds.0 + self.bounds.1) * 0.5;
        self.distance = (self.bounds.1 - self.bounds.0).length().max(100.0);
        self.pitch = 60.0f32.to_radians();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn panning_is_frame_rate_independent_and_rotates_with_the_view() {
        let mut a = TacticalCamera::new(([0.0; 2], [10000.0; 2]), &Definitions::default());
        let mut b = a.clone();
        for _ in 0..30 {
            a.pan(Vec2::Y, 1.0 / 30.0, false);
        }
        for _ in 0..120 {
            b.pan(Vec2::Y, 1.0 / 120.0, false);
        }
        assert!(a.center.distance(b.center) < 0.1);
        let start = b.center;
        b.yaw = std::f32::consts::FRAC_PI_2;
        b.pan(Vec2::Y, 1.0, false);
        assert!(b.center.x < start.x);
        assert!((b.center.y - start.y).abs() < 0.001);
    }
    #[test]
    fn zoom_pitch_and_bounds_stay_valid() {
        let mut c = TacticalCamera::new(([0.0; 2], [100.0; 2]), &Definitions::default());
        c.pan(Vec2::splat(1.0), 100.0, true);
        assert_eq!(c.center, Vec2::splat(100.0));
        c.zoom(1000.0);
        assert_eq!(c.distance, 80.0);
        c.zoom(-1000.0);
        assert_eq!(c.distance, 15000.0);
        c.orbit(1000.0, 1000.0);
        let camera = c.camera(&Map::default());
        assert!(camera.view_proj(1.5).is_finite());
        assert!(camera.far > c.distance);
    }
    #[test]
    fn centre_pixel_picks_the_terrain_under_the_camera_target() {
        let map = Map {
            width: 14,
            height: 14,
            border: 2,
            boundaries: vec![[10, 10]],
            heights: vec![16; 196],
            ..Default::default()
        };
        let c = TacticalCamera::new(([0.0; 2], [100.0; 2]), &Definitions::default());
        let p = c.ground_at_pixel(&map, [800, 600], Vec2::new(400.0, 300.0)).unwrap();
        assert!(p.distance(c.center) < 0.02, "{p:?}");
    }
    #[test]
    fn surface_pick_hits_an_elevated_deck_before_the_ground() {
        use ih_sim::{bridges::Bridge, navigation::Grid, terrain::Terrain};
        let map = Map { width: 24, height: 24, border: 2, heights: vec![0; 576], ..Default::default() };
        let c = TacticalCamera::new(([0.0; 2], [200.0; 2]), &Definitions::default());
        let bridge =
            Bridge { name: "B".into(), from: [50.0, 100.0, 20.0], to: [150.0, 100.0, 20.0], min_y: -50.0, max_y: 50.0 };
        let terrain = Terrain::new(Grid { width: 20, height: 20, blocked: vec![false; 400] }, vec![bridge], &map);
        let p = c.surface_at_pixel(&map, &terrain, [800, 600], Vec2::new(400.0, 300.0)).unwrap();
        assert!((p.x - 100.0).abs() < 0.01);
        assert!(p.y < 90.0 && p.y > 50.0, "{p:?}");
    }
}
