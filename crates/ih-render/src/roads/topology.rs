//! Road junctions and end trimming, translated from `W3DRoadBuffer.cpp`:
//! `insertTee`, `insertY`, `offset3Way`, `offsetH`, `offsetY`, `insert4Way`,
//! `offset4Way`, `insertCurveSegmentAt`, `miter`, `insertCrossTypeJoins`.
//! Endpoint groups replace the original mutable linked runs; traversal is in map
//! order and no hash iteration affects the geometry or texture stacking.

use super::{Segment, Shape};
use glam::Vec2;
use ih_formats::map::object_flags;
use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, FRAC_PI_6};

#[derive(Clone, Copy)]
struct End {
    segment: usize,
    point: usize,
}

#[derive(Clone, Copy)]
struct Point {
    loc: Vec2,
    top: Vec2,
    bottom: Vec2,
}

fn point(s: &Segment, p: usize) -> Point {
    Point { loc: if p == 0 { s.p1 } else { s.p2 }, top: s.corners[p + 2], bottom: s.corners[p] }
}
fn set(s: &mut Segment, p: usize, value: Point) {
    if p == 0 {
        s.p1 = value.loc;
    } else {
        s.p2 = value.loc;
    }
    s.corners[p + 2] = value.top;
    s.corners[p] = value.bottom;
}
fn rotate(v: Vec2, angle: f32) -> Vec2 {
    Vec2::from_angle(angle).rotate(v)
}
fn sign(a: Vec2, b: Vec2) -> i32 {
    let c = a.perp_dot(b);
    if c < 0.0 {
        -1
    } else if c > 0.0 {
        1
    } else {
        0
    }
}
/// Infinite line intersection, as `LineSegClass::Find_Intersection` (not a
/// bounded segment test; road mitres often lie beyond the stored endpoints).
fn intersection(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> Option<Vec2> {
    let (u, v) = (b - a, d - c);
    let denom = u.perp_dot(v);
    if denom.abs() < 1e-6 { None } else { Some(a + u * ((c - a).perp_dot(v) / denom)) }
}

pub(super) fn connect(roads: &mut Vec<Segment>, num_types: usize) -> Vec<usize> {
    let original = roads.len();
    let mut nodes: Vec<(Vec2, Option<usize>, Vec<End>)> = Vec::new();
    for (i, road) in roads.iter().enumerate() {
        for p in 0..2 {
            let loc = point(road, p).loc;
            let at = nodes.iter().position(|(v, k, _)| *v == loc && *k == road.kind);
            let end = End { segment: i, point: p };
            if let Some(at) = at {
                nodes[at].2.push(end);
            } else {
                nodes.push((loc, road.kind, vec![end]));
            }
        }
    }
    let mut counts = vec![[0usize; 2]; original];
    for (_, _, ends) in &nodes {
        for end in ends {
            counts[end.segment][end.point] = ends.len() - 1;
        }
    }
    // Junctions precede curves and joins, as `loadRoads` does.
    for (loc, _, ends) in &nodes {
        if ends.len() == 3 || ends.len() == 4 {
            junction(roads, *loc, ends);
        }
    }
    for (_, _, ends) in &nodes {
        if let [a, b] = ends.as_slice() {
            curve(roads, *a, *b);
        }
    }
    let mut stacking = vec![0; num_types];
    for i in 0..original {
        // The original handles at most one join per segment, preferring point 2.
        let end = if counts[i][1] == 0 && roads[i].flags[1] & object_flags::ROAD_JOIN != 0 {
            Some(1)
        } else if counts[i][0] == 0 && roads[i].flags[0] & object_flags::ROAD_JOIN != 0 {
            Some(0)
        } else {
            None
        };
        if let Some(p) = end {
            alpha_join(roads, i, p, &mut stacking);
        }
    }
    stacking
}

/// Set the two edge points while preserving which side used to be "top".
fn edges(p: &mut Point, axis: Vec2, normal: Vec2, positive: bool) {
    let yes = axis.perp_dot(p.top - p.loc) > 0.0;
    let n = if yes == positive { normal } else { -normal };
    p.top = p.loc + n;
    p.bottom = p.loc - n;
}

fn junction(roads: &mut Vec<Segment>, loc: Vec2, ends: &[End]) {
    let base = roads[ends[0].segment].clone();
    let mut points: Vec<_> = ends.iter().map(|e| point(&roads[e.segment], e.point)).collect();
    let dirs: Vec<_> =
        ends.iter().map(|e| (point(&roads[e.segment], 1 - e.point).loc - loc).normalize_or_zero()).collect();
    let scale = base.scale;
    let width = base.width_in_texture;
    let mut patch = Segment::new(loc, loc + Vec2::X, scale, width, base.kind);
    if ends.len() == 4 {
        let pairs = [(0, 1, 2, 3), (0, 2, 1, 3), (0, 3, 2, 1), (1, 2, 0, 3), (1, 3, 0, 2), (2, 3, 0, 1)];
        let mut best = pairs[0];
        let mut dot = dirs[0].dot(dirs[1]);
        for pair in &pairs[1..] {
            let d = dirs[pair.0].dot(dirs[pair.1]);
            if d < dot {
                dot = d;
                best = *pair;
            }
        }
        let (a, b, c, d) = best;
        let align = (dirs[b] - dirs[a]).normalize_or_zero() * 0.5 * scale;
        let tee = rotate(align, if sign(align, dirs[c]) < 0 { -FRAC_PI_2 } else { FRAC_PI_2 });
        points[a].loc = loc - align;
        points[b].loc = loc + align;
        points[c].loc = loc + tee;
        points[d].loc = loc - tee;
        let n = align.perp() * width;
        for i in [a, b] {
            edges(&mut points[i], align, n, true);
        }
        for i in [c, d] {
            edges(&mut points[i], n, align * width, false);
        }
        patch.p2 = loc + if align.x < 0.0 { -align } else { align };
        patch.width_in_texture = 1.03;
        patch.shape = Shape::FourWay;
    } else {
        let dots = [dirs[0].dot(dirs[1]), dirs[0].dot(dirs[2]), dirs[2].dot(dirs[1])];
        // Exact candidate order and score/tie rules of `insertY`.
        let mut candidates = [false; 3];
        let mut scores = [2.0f32; 3];
        if dots.iter().all(|d| *d >= -0.866) {
            for (slot, stem, a, b, score) in [
                (0, 2, 0, 1, (dots[1] + 0.707).abs() + (dots[2] + 0.707).abs()),
                (1, 1, 0, 2, (dots[0] + 0.707).abs() + (dots[2] + 0.707).abs()),
                (2, 0, 2, 1, (dots[0] + 0.707).abs() + (dots[1] + 0.707).abs()),
            ] {
                let (s1, s2) = (sign(dirs[stem], dirs[a]), sign(dirs[stem], dirs[b]));
                if s1 != s2
                    && s1 + s2 == 0
                    && sign(dirs[stem].perp(), dirs[a]) == 1
                    && sign(dirs[stem].perp(), dirs[b]) == 1
                {
                    candidates[slot] = true;
                    scores[slot] = score;
                }
            }
        }
        let best = if scores[0] < scores[1] {
            if scores[0] < scores[2] { 0 } else { 2 }
        } else if scores[1] < scores[2] {
            1
        } else {
            2
        };
        if candidates[best] {
            let (mut a, mut b, stem) = [(0, 1, 2), (0, 2, 1), (2, 1, 0)][best];
            if sign(dirs[stem], dirs[a]) != -1 {
                std::mem::swap(&mut a, &mut b);
            }
            let up = dirs[stem] * 0.5 * scale;
            points[stem].loc += up * 0.55;
            points[stem].top += up * 0.55;
            points[stem].bottom += up * 0.55;
            for (i, angle) in [(a, -3.0 * FRAC_PI_4), (b, 3.0 * FRAC_PI_4)] {
                let arm = rotate(up, angle);
                let n = arm.perp() * width;
                points[i].loc += arm * 1.1;
                let positive = sign(arm, points[i].top - loc) == 1;
                points[i].top = points[i].loc + if positive { n } else { -n };
                points[i].bottom = points[i].loc - if positive { n } else { -n };
            }
            patch.p2 = loc + rotate(up, -FRAC_PI_2);
            patch.shape = Shape::Y;
        } else {
            let best = if dots[0] < dots[1] {
                if dots[0] < dots[2] { 0 } else { 2 }
            } else if dots[1] < dots[2] {
                1
            } else {
                2
            };
            let (a, b, c) = [(0, 1, 2), (0, 2, 1), (2, 1, 0)][best];
            let up = (dirs[b] - dirs[a]).normalize_or_zero() * 0.5 * scale;
            let mirror = sign(up, dirs[c]) < 0;
            let tee = rotate(up, if mirror { -FRAC_PI_2 } else { FRAC_PI_2 });
            let slanted = up.normalize_or_zero().dot(dirs[c]).abs() > 0.5;
            let flip = sign(tee, dirs[c]) == 1;
            if slanted {
                let (left, right) = if flip != mirror { (2.05, 0.46) } else { (0.46, 2.05) };
                points[a].loc = loc - up * left;
                points[b].loc = loc + up * right;
            } else {
                points[a].loc = loc - up;
                points[b].loc = loc + up;
                points[c].loc = loc + tee;
            }
            let right = if sign(up, tee) < 0 { -tee } else { tee } * width;
            for i in [a, b] {
                edges(&mut points[i], up, right, true);
            }
            if slanted {
                let arm = rotate(tee, if flip { FRAC_PI_4 } else { -FRAC_PI_4 });
                let n = arm.perp() * width;
                points[c].loc += arm * 2.10;
                let positive = sign(arm, points[c].top - loc) == 1;
                points[c].top = points[c].loc + if positive { n } else { -n };
                points[c].bottom = points[c].loc - if positive { n } else { -n };
                patch.shape = if flip { Shape::HFlip } else { Shape::H };
            } else {
                edges(&mut points[c], right, up * width, false);
                patch.shape = Shape::Tee;
            }
            patch.p2 = loc + tee;
        }
    }
    for (e, p) in ends.iter().zip(points) {
        set(&mut roads[e.segment], e.point, p);
    }
    roads.push(patch);
}

fn oriented(s: &Segment, shared: usize, at_start: bool) -> Segment {
    let mut s = s.clone();
    if (shared == 0) != at_start {
        std::mem::swap(&mut s.p1, &mut s.p2);
        s.corners.reverse();
        s.flags.swap(0, 1);
    }
    s
}

fn curve(roads: &mut Vec<Segment>, a: End, b: End) {
    let mut first = oriented(&roads[a.segment], a.point, true);
    let mut second = oriented(&roads[b.segment], b.point, false);
    let original = first.p1;
    let d1 = (first.p2 - first.p1).normalize_or_zero();
    let d2 = (second.p2 - second.p1).normalize_or_zero();
    let angle = d1.dot(d2).clamp(-1.0, 1.0).acos();
    let count = angle / FRAC_PI_6;
    let r = first.radius * first.scale;
    let mut miter = count < 0.9 || first.flags[0] & object_flags::ROAD_CORNER_ANGLED != 0;
    if !miter {
        let right = d1.perp_dot(d2) > 0.0;
        let (p1, p2, p3, p4) =
            if right { (first.p1, first.p2, second.p1, second.p2) } else { (second.p2, second.p1, first.p2, first.p1) };
        let (v1, v2) = ((p2 - p1).normalize_or_zero(), (p4 - p3).normalize_or_zero());
        let (off1, off2) = (-v1.perp() * r, -v2.perp() * r);
        if let Some(center) = intersection(p1 + off1, p2 + off1, p3 + off2, p4 + off2) {
            let start = center - off2;
            let finish = center - off1;
            if v2.dot(start - p3) < 0.5 || v1.dot(p2 - finish) < 0.5 {
                miter = true;
            } else {
                let mut pt = start;
                let mut dir = p3 - start;
                // The original stamps overlapping 30-degree curve texture pieces.
                let mut add = |p: Vec2, d: Vec2| {
                    let mut patch = Segment::new(p, p + d, first.scale, first.width_in_texture, first.kind);
                    patch.radius = first.radius;
                    patch.shape = Shape::Curve;
                    roads.push(patch);
                };
                pt = center + rotate(pt - center, -FRAC_PI_6);
                dir = rotate(dir, -FRAC_PI_6);
                add(pt, dir);
                let mut i = 2;
                while (i as f32) < count {
                    dir = rotate(dir, -FRAC_PI_6);
                    pt = center + rotate(pt - center, -FRAC_PI_6);
                    add(pt, dir);
                    i += 1;
                }
                if count > 1.0 {
                    add(finish, finish - p2);
                }
                if right {
                    first.p1 = finish;
                    second.p2 = start;
                } else {
                    second.p2 = finish;
                    first.p1 = start;
                }
                let n = (first.p2 - first.p1).normalize_or_zero().perp() * first.scale * first.width_in_texture * 0.5;
                first.corners[0] = first.p1 - n;
                first.corners[2] = first.p1 + n;
                let n =
                    (second.p2 - second.p1).normalize_or_zero().perp() * second.scale * second.width_in_texture * 0.5;
                second.corners[1] = second.p2 - n;
                second.corners[3] = second.p2 + n;
            }
        } else {
            miter = true;
        }
    }
    if miter {
        first.p1 = original;
        second.p2 = original;
        for offset in [0, 2] {
            if let Some(p) = intersection(
                first.corners[offset],
                first.corners[offset + 1],
                second.corners[offset],
                second.corners[offset + 1],
            ) {
                first.corners[offset] = p;
                second.corners[offset + 1] = p;
            }
        }
    }
    let mut pa = point(&first, 0);
    let mut pb = point(&second, 1);
    if a.point == 1 {
        std::mem::swap(&mut pa.top, &mut pa.bottom);
    }
    if b.point == 0 {
        std::mem::swap(&mut pb.top, &mut pb.bottom);
    }
    set(&mut roads[a.segment], a.point, pa);
    set(&mut roads[b.segment], b.point, pb);
}

fn alpha_join(roads: &mut Vec<Segment>, i: usize, p: usize, stacking: &mut [usize]) {
    let source = roads[i].clone();
    let (loc, other) = (point(&source, p).loc, point(&source, 1 - p).loc);
    let dir = (other - loc).normalize_or_zero();
    let mut join = dir * 100.0;
    let mut under = None;
    for road in roads.iter().filter(|r| r.kind != source.kind && r.shape == Shape::Straight) {
        let lo = road.p1.min(road.p2) - Vec2::splat(road.scale * 0.5);
        let hi = road.p1.max(road.p2) + Vec2::splat(road.scale * 0.5);
        if !loc.cmpge(lo).all() || !loc.cmple(hi).all() {
            continue;
        }
        let v = road.p2 - road.p1;
        let closest = road.p1 + v * ((loc - road.p1).dot(v) / v.length_squared()).clamp(0.0, 1.0);
        if closest.distance(loc) < road.scale * 0.55 {
            join = if sign(v, dir) == 1 { v.perp() } else { -v.perp() };
            under = road.kind;
            break;
        }
    }
    let half = dir.perp() * source.scale * source.width_in_texture * 0.5;
    let mut end = point(&source, p);
    let top = intersection(loc, loc + join.perp(), loc + half, other + half);
    let bottom = intersection(loc, loc + join.perp(), loc - half, other - half);
    if let Some(t) = top {
        if p == 0 {
            end.top = t;
        } else {
            end.bottom = t;
        }
    }
    if let Some(b) = bottom {
        if p == 0 {
            end.bottom = b;
        } else {
            end.top = b;
        }
    }
    set(&mut roads[i], p, end);
    let adjustment = end.top.distance(end.bottom) / (source.scale * source.width_in_texture);
    if let (Some(top), Some(bottom)) = (source.kind, under)
        && stacking[top] <= stacking[bottom]
    {
        let level = stacking[bottom];
        for n in stacking.iter_mut() {
            if *n > level {
                *n += 1;
            }
        }
        stacking[top] = level + 1;
    }
    let mut patch = Segment::new(loc, loc + join, source.scale, source.scale * adjustment, source.kind);
    patch.shape = Shape::AlphaJoin;
    roads.push(patch);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn road(a: Vec2, b: Vec2) -> Segment {
        Segment::new(a, b, 20.0, 0.9, Some(0))
    }
    fn close(a: Vec2, b: Vec2) {
        assert!(a.distance(b) < 0.001, "{a:?} != {b:?}");
    }

    #[test]
    fn right_angle_uses_radius_and_curve_texture_stamps() {
        let mut roads =
            vec![road(Vec2::new(0.0, 100.0), Vec2::splat(100.0)), road(Vec2::splat(100.0), Vec2::new(100.0, 200.0))];
        connect(&mut roads, 1);
        close(roads[0].p2, Vec2::new(70.0, 100.0));
        close(roads[1].p1, Vec2::new(100.0, 130.0));
        assert!(roads.iter().filter(|r| r.shape == Shape::Curve).count() >= 2);
        assert!(roads[0].corners[3].y > roads[0].corners[1].y);
        assert!(roads[1].corners[2].x < roads[1].corners[0].x);
    }

    #[test]
    fn angled_corner_mitres_without_shortening_center_lines() {
        let mut a = road(Vec2::new(0.0, 100.0), Vec2::splat(100.0));
        a.flags[1] |= object_flags::ROAD_CORNER_ANGLED;
        let mut roads = vec![a, road(Vec2::splat(100.0), Vec2::new(100.0, 200.0))];
        connect(&mut roads, 1);
        assert_eq!(roads.len(), 2);
        close(roads[0].corners[3], roads[1].corners[2]);
        close(roads[0].corners[1], roads[1].corners[0]);
        close(roads[0].p2, Vec2::splat(100.0));
    }

    #[test]
    fn tee_and_cross_trim_arms_and_use_dedicated_patches() {
        for arms in [3, 4] {
            let dirs = [Vec2::X, Vec2::NEG_X, Vec2::Y, Vec2::NEG_Y];
            let mut roads: Vec<_> = dirs[..arms].iter().map(|d| road(Vec2::ZERO, *d * 100.0)).collect();
            connect(&mut roads, 1);
            assert_eq!(roads.len(), arms + 1);
            assert!(roads[..arms].iter().all(|r| r.p1.length() > 9.9));
            assert!(roads.last().unwrap().shape == if arms == 3 { Shape::Tee } else { Shape::FourWay });
        }
    }

    #[test]
    fn y_and_slanted_tee_are_classified_separately() {
        for (dirs, expected) in [
            ([Vec2::Y, Vec2::new(-1.0, -1.0).normalize(), Vec2::new(1.0, -1.0).normalize()], Shape::Y),
            ([Vec2::X, Vec2::NEG_X, Vec2::new(0.9, 0.3).normalize()], Shape::H),
        ] {
            let mut roads: Vec<_> = dirs.iter().map(|d| road(Vec2::ZERO, *d * 100.0)).collect();
            connect(&mut roads, 1);
            assert!(roads.last().unwrap().shape == expected);
        }
    }

    #[test]
    fn cross_type_alpha_join_draws_above_the_road_it_meets() {
        let mut a = road(Vec2::ZERO, Vec2::Y * 100.0);
        a.flags[0] |= object_flags::ROAD_JOIN;
        let mut b = road(Vec2::NEG_X * 100.0, Vec2::X * 100.0);
        b.kind = Some(1);
        let mut roads = vec![a, b];
        let stacking = connect(&mut roads, 2);
        assert!(stacking[0] > stacking[1]);
        assert_eq!(roads.len(), 3);
        assert!(roads[2].shape == Shape::AlphaJoin);
        assert!((roads[2].width_in_texture - 20.0).abs() < 0.001);
    }
}
