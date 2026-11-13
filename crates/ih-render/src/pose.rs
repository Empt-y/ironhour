//! Hierarchy poses and animation sampling.
//!
//! Ported from `WW3D2/htree.cpp` (`Base_Update`, `Anim_Update`), `hrawanim.cpp`,
//! `hcanim.cpp` and `motchan.cpp`.
//!
//! A pivot's world transform is `parent * base * T(anim translation) * R(anim
//! rotation)`, where `base` is the pivot's stored translation then rotation. Pivot 0
//! is the root and always takes the object's transform. Pivots beyond the
//! animation's pivot count keep their base pose.

use glam::{Mat4, Quat, Vec3};
use ih_formats::w3d::{Animation, BitChannel, ChannelData, ChannelKind, Hierarchy};

fn quat(q: [f32; 4]) -> Quat {
    Quat::from_xyzw(q[0], q[1], q[2], q[3])
}

fn base_local(h: &Hierarchy, i: usize) -> Mat4 {
    let p = &h.pivots[i];
    Mat4::from_translation(Vec3::from(p.translation)) * Mat4::from_quat(quat(p.rotation))
}

/// World transforms of every pivot in the rest pose.
pub fn base_pose(h: &Hierarchy, root: Mat4) -> Vec<Mat4> {
    let mut out = Vec::with_capacity(h.pivots.len());
    for i in 0..h.pivots.len() {
        let m = match (i, h.pivots[i].parent) {
            (0, _) => root,
            (_, Some(p)) if (p as usize) < i => out[p as usize] * base_local(h, i),
            _ => root * base_local(h, i),
        };
        out.push(m);
    }
    out
}

/// World transforms and visibility of every pivot at `frame` of `anim`.
#[allow(clippy::needless_range_loop)] // indexes several parallel arrays
pub fn animated_pose(h: &Hierarchy, anim: &Animation, frame: f32, root: Mat4) -> (Vec<Mat4>, Vec<bool>) {
    let sampler = Sampler::new(anim, h.pivots.len());
    let mut out: Vec<Mat4> = Vec::with_capacity(h.pivots.len());
    let mut visible = vec![true; h.pivots.len()];
    for i in 0..h.pivots.len() {
        if i == 0 {
            out.push(root);
            continue;
        }
        let parent = match h.pivots[i].parent {
            Some(p) if (p as usize) < i => out[p as usize],
            _ => root,
        };
        let mut m = parent * base_local(h, i);
        if i < sampler.pivots {
            let (t, q) = sampler.transform(i, frame);
            m = m * Mat4::from_translation(t) * Mat4::from_quat(q);
            visible[i] = sampler.visible(i, frame);
        }
        out.push(m);
    }
    (out, visible)
}

/// Per-pivot channel lookup for one animation.
struct Sampler<'a> {
    anim: &'a Animation,
    /// Number of pivots the animation covers (the hierarchy's count; channels for
    /// pivots past it are ignored by the original).
    pivots: usize,
    /// Per pivot: channel index for X, Y, Z, Q.
    chans: Vec<[Option<usize>; 4]>,
    vis: Vec<Option<usize>>,
}

impl<'a> Sampler<'a> {
    fn new(anim: &'a Animation, pivots: usize) -> Self {
        let mut chans = vec![[None; 4]; pivots];
        for (ci, c) in anim.channels.iter().enumerate() {
            let p = c.pivot as usize;
            if p >= pivots {
                continue;
            }
            let slot = match c.kind {
                ChannelKind::X => 0,
                ChannelKind::Y => 1,
                ChannelKind::Z => 2,
                ChannelKind::Q => 3,
                // Euler channels are loaded but never applied (hrawanim.cpp).
                _ => continue,
            };
            chans[p][slot] = Some(ci);
        }
        let mut vis = vec![None; pivots];
        for (bi, b) in anim.bit_channels.iter().enumerate() {
            let p = match b {
                BitChannel::Raw { pivot, .. } | BitChannel::TimeCoded { pivot, .. } => *pivot as usize,
            };
            if p < pivots {
                vis[p] = Some(bi);
            }
        }
        Self { anim, pivots, chans, vis }
    }

    fn transform(&self, pivot: usize, frame: f32) -> (Vec3, Quat) {
        let c = &self.chans[pivot];
        let comp = |slot: usize| c[slot].map_or(0.0, |ci| self.scalar(ci, frame));
        let t = Vec3::new(comp(0), comp(1), comp(2));
        let q = c[3].map_or(Quat::IDENTITY, |ci| self.quat(ci, frame));
        (t, q)
    }

    /// Frames to blend for raw animations (`HRawAnimClass::Get_Translation`):
    /// `floor(frame)` and the next, wrapping to 0 past the end.
    fn raw_frames(&self, frame: f32) -> (u32, u32, f32) {
        let f0 = (frame - 0.499_999).round().max(0.0) as u32;
        let mut f1 = f0 + 1;
        if f1 >= self.anim.num_frames {
            f1 = 0;
        }
        (f0, f1, frame - f0 as f32)
    }

    fn scalar(&self, ci: usize, frame: f32) -> f32 {
        let ch = &self.anim.channels[ci];
        match &ch.data {
            ChannelData::Raw { first, last, values } => {
                let get = |f: u32| if f < *first || f > *last { 0.0 } else { values[(f - first) as usize] };
                let (f0, f1, r) = self.raw_frames(frame);
                let a = get(f0);
                if r == 0.0 { a } else { a + (get(f1) - a) * r }
            }
            ChannelData::TimeCoded { keys } => timecoded(keys, frame, 1)[0],
            ChannelData::Dense { values } => {
                // AdaptiveDeltaMotionChannelClass::Get_Vector: lerp between floor and
                // floor + 1, clamped to the last frame.
                let n = values.len();
                if n == 0 {
                    return 0.0;
                }
                let f0 = (frame.max(0.0) as usize).min(n - 1);
                let f1 = (f0 + 1).min(n - 1);
                let r = frame - frame.floor();
                values[f0] + (values[f1] - values[f0]) * r
            }
        }
    }

    fn quat(&self, ci: usize, frame: f32) -> Quat {
        let ch = &self.anim.channels[ci];
        match &ch.data {
            ChannelData::Raw { first, last, values } => {
                let get = |f: u32| {
                    if f < *first || f > *last {
                        Quat::IDENTITY
                    } else {
                        let i = (f - first) as usize * 4;
                        Quat::from_xyzw(values[i], values[i + 1], values[i + 2], values[i + 3])
                    }
                };
                let (f0, f1, r) = self.raw_frames(frame);
                if r == 0.0 { get(f0) } else { get(f0).slerp(get(f1), r) }
            }
            ChannelData::TimeCoded { keys } => {
                let v = timecoded_quat(keys, frame);
                Quat::from_xyzw(v[0], v[1], v[2], v[3])
            }
            ChannelData::Dense { values } => {
                let n = values.len() / 4;
                if n == 0 {
                    return Quat::IDENTITY;
                }
                let f0 = (frame.max(0.0) as usize).min(n - 1);
                let f1 = (f0 + 1).min(n - 1);
                let q =
                    |f: usize| Quat::from_xyzw(values[f * 4], values[f * 4 + 1], values[f * 4 + 2], values[f * 4 + 3]);
                q(f0).slerp(q(f1), frame - frame.floor())
            }
        }
    }

    fn visible(&self, pivot: usize, frame: f32) -> bool {
        let Some(bi) = self.vis[pivot] else { return true };
        match &self.anim.bit_channels[bi] {
            BitChannel::Raw { first, last, default, bits, .. } => {
                let f = frame as u32;
                if f < *first || f > *last {
                    *default
                } else {
                    let b = (f - first) as usize;
                    bits.get(b / 8).is_some_and(|v| v & (1 << (b % 8)) != 0)
                }
            }
            // TimeCodedBitChannelClass::Get_Bit: the value of the last key at or before
            // the frame (the first key if none).
            BitChannel::TimeCoded { keys, default, .. } => {
                if keys.is_empty() {
                    return *default;
                }
                let f = frame as u32;
                let idx = keys.iter().rposition(|(t, _)| *t <= f).unwrap_or(0);
                keys[idx].1
            }
        }
    }
}

/// Index of the key to interpolate from (`TimeCodedMotionChannelClass::get_index`).
fn key_index(keys: &[(u32, bool, Vec<f32>)], frame: f32) -> usize {
    let tc = frame.max(0.0) as u32;
    keys.iter().rposition(|(t, _, _)| *t <= tc).unwrap_or(0)
}

/// `TimeCodedMotionChannelClass::Get_Vector`: linear between keys unless the next
/// key is flagged as a step.
fn timecoded(keys: &[(u32, bool, Vec<f32>)], frame: f32, len: usize) -> Vec<f32> {
    if keys.is_empty() {
        return vec![0.0; len];
    }
    let i = key_index(keys, frame);
    let (t0, _, v0) = &keys[i];
    let Some((t1, step, v1)) = keys.get(i + 1) else { return v0.clone() };
    if *step {
        return v0.clone();
    }
    let r = (frame - *t0 as f32) / (*t1 as f32 - *t0 as f32);
    v0.iter().zip(v1).map(|(a, b)| a + (b - a) * r).collect()
}

fn timecoded_quat(keys: &[(u32, bool, Vec<f32>)], frame: f32) -> [f32; 4] {
    if keys.is_empty() {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let i = key_index(keys, frame);
    let (t0, _, v0) = &keys[i];
    let q0 = Quat::from_xyzw(v0[0], v0[1], v0[2], v0[3]);
    let Some((t1, step, v1)) = keys.get(i + 1) else { return q0.to_array() };
    if *step {
        return q0.to_array();
    }
    let q1 = Quat::from_xyzw(v1[0], v1[1], v1[2], v1[3]);
    let r = (frame - *t0 as f32) / (*t1 as f32 - *t0 as f32);
    q0.slerp(q1, r).to_array()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ih_formats::w3d::{Channel, Pivot};

    fn hier() -> Hierarchy {
        let piv = |name: &str, parent: Option<u32>, t: [f32; 3]| Pivot {
            name: name.into(),
            parent,
            translation: t,
            euler: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
        };
        Hierarchy {
            name: "H".into(),
            version: 0,
            center: [0.0; 3],
            pivots: vec![
                piv("ROOT", None, [9.0, 9.0, 9.0]),
                piv("A", Some(0), [1.0, 0.0, 0.0]),
                piv("B", Some(1), [0.0, 2.0, 0.0]),
            ],
        }
    }

    #[test]
    fn base_pose_chains_and_root_ignores_its_base() {
        let p = base_pose(&hier(), Mat4::IDENTITY);
        assert_eq!(p[0], Mat4::IDENTITY);
        assert_eq!(p[2].w_axis.truncate(), Vec3::new(1.0, 2.0, 0.0));
    }

    #[test]
    fn raw_translation_interpolates() {
        let anim = Animation {
            name: "A".into(),
            hierarchy: "H".into(),
            num_frames: 3,
            frame_rate: 30,
            compressed: false,
            channels: vec![Channel {
                pivot: 1,
                kind: ChannelKind::Z,
                vector_len: 1,
                data: ChannelData::Raw { first: 0, last: 2, values: vec![0.0, 10.0, 20.0] },
            }],
            bit_channels: vec![],
        };
        let (p, vis) = animated_pose(&hier(), &anim, 1.5, Mat4::IDENTITY);
        assert!((p[1].w_axis.z - 15.0).abs() < 1e-5);
        assert!(vis[1]);
        // B inherits A's animated offset.
        assert!((p[2].w_axis.z - 15.0).abs() < 1e-5);
    }
}
