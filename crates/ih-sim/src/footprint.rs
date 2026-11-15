//! Obstacle cells under structures, from `AIPathfind.cpp`:
//! `Pathfinder::classifyObjectFootprint`, `internal_classifyObjectFootprint`,
//! `classifyFence` and `PathfindCell::setTypeAsObstacle`. Objects are added one
//! at a time, in map order, as `GameLogic::startNewGame` does; each insertion
//! closes off pinched cells around it and marks the border cells as pinched.
use crate::navigation::{CELL, Grid};

/// The cell types the footprint code distinguishes (`PathfindCell::CellType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellType {
    Clear,
    /// Cliff, water or a bridge's underside: never turned into an obstacle.
    Terrain,
    Obstacle,
    /// `CELL_IMPASSABLE`: a clear cell pinched shut between obstacles.
    Impassable,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// `GEOMETRY_BOX`: half sizes along and across the facing.
    Box { major: f32, minor: f32 },
    /// `GEOMETRY_SPHERE` and `GEOMETRY_CYLINDER`.
    Circle { radius: f32 },
    /// Templates with `FenceWidth` (and not `DEFENSIVE_WALL`).
    Fence { width: f32, x_offset: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Footprint {
    pub position: [f32; 2],
    pub angle: f32,
    pub shape: Shape,
    /// Nonzero key of the obstacle (map object index + 1 for map objects).
    pub key: u32,
}

/// Per-cell obstacle state kept beside the grid's `blocked` flags.
#[derive(Clone, Debug, Default)]
pub struct Obstacles {
    pub kind: Vec<CellType>,
    /// `m_obstacleID` (the footprint key), 0 for none.
    pub owner: Vec<u32>,
    pub fence: Vec<bool>,
    /// `getPinched`: clear cells orthogonally next to an obstacle.
    pub pinched: Vec<bool>,
}

fn floor_cell(v: f32) -> i32 {
    ((v + 0.5) / CELL).floor() as i32
}

impl Obstacles {
    pub fn new(grid: &Grid) -> Self {
        let n = grid.blocked.len();
        Self {
            kind: grid.blocked.iter().map(|b| if *b { CellType::Terrain } else { CellType::Clear }).collect(),
            owner: vec![0; n],
            fence: vec![false; n],
            pinched: vec![false; n],
        }
    }
    pub fn count(&self, kind: CellType) -> usize {
        self.kind.iter().filter(|k| **k == kind).count()
    }

    /// `setTypeAsObstacle` (rubble needs body damage states, M5).
    fn set_obstacle(&mut self, i: usize, key: u32, fence: bool) -> bool {
        if !matches!(self.kind[i], CellType::Clear | CellType::Impassable) {
            return false;
        }
        self.kind[i] = CellType::Obstacle;
        self.owner[i] = key;
        self.fence[i] = fence;
        true
    }

    /// Adds one footprint and writes the result back into `grid.blocked`.
    pub fn insert(&mut self, grid: &mut Grid, f: &Footprint) {
        let (w, h) = (grid.width, grid.height);
        let index = |x: i32, y: i32| (x >= 0 && y >= 0 && x < w && y < h).then(|| (y * w + x) as usize);
        let [px, py] = f.position;
        let mut lo = (floor_cell(px), floor_cell(py));
        let mut hi = lo;
        let mut grow = |x: i32, y: i32| {
            lo = (lo.0.min(x), lo.1.min(y));
            hi = (hi.0.max(x), hi.1.max(y));
        };
        match f.shape {
            Shape::Box { .. } | Shape::Fence { .. } => {
                let (half_x, half_y, fence) = match f.shape {
                    Shape::Box { major, minor } => (major, minor, false),
                    Shape::Fence { width, .. } => (width / 2.0, CELL / 10.0, true),
                    Shape::Circle { .. } => unreachable!(),
                };
                let (c, s) = (ih_math::cos(f.angle), ih_math::sin(f.angle));
                // Half a cell, "to avoid aliasing problems".
                let step = CELL * 0.5;
                let (ydx, ydy, xdx, xdy) = (s * step, -c * step, c * step, s * step);
                let steps_x = (2.0 * half_x / step).ceil() as i32;
                let steps_y = (2.0 * half_y / step).ceil() as i32;
                // A box starts at its corner; a fence at its x offset along the facing.
                let (mut tl_x, mut tl_y) = match f.shape {
                    Shape::Fence { x_offset, .. } => (px - x_offset * c - half_y * s, py + half_y * c - x_offset * s),
                    _ => (px - half_x * c - half_y * s, py + half_y * c - half_x * s),
                };
                for _ in 0..steps_y {
                    let (mut x, mut y) = (tl_x, tl_y);
                    for _ in 0..steps_x {
                        let (cx, cy) = (floor_cell(x), floor_cell(y));
                        if let Some(i) = index(cx, cy) {
                            self.set_obstacle(i, f.key, fence);
                            grow(cx, cy);
                        }
                        x += xdx;
                        y += xdy;
                    }
                    tl_x += ydx;
                    tl_y += ydy;
                }
                if fence {
                    // classifyFence ends here: no pinch processing.
                    self.write_back(grid);
                    return;
                }
            }
            Shape::Circle { radius } => {
                let top_left =
                    ((0.5 + (px - radius) / CELL).floor() as i32 - 1, (0.5 + (py - radius) / CELL).floor() as i32 - 1);
                let size = radius / CELL + 0.4;
                let center = (px / CELL, py / CELL);
                let r2 = size * size;
                // `Int = Int + Real` truncates.
                let bottom_right =
                    ((top_left.0 as f32 + 2.0 * size + 2.0) as i32, (top_left.1 as f32 + 2.0 * size + 2.0) as i32);
                for j in top_left.1..bottom_right.1 {
                    for i in top_left.0..bottom_right.0 {
                        let d = (i as f32 + 0.5 - center.0, j as f32 + 0.5 - center.1);
                        if d.0 * d.0 + d.1 * d.1 <= r2
                            && let Some(n) = index(i, j)
                        {
                            self.set_obstacle(n, f.key, false);
                            grow(i, j);
                        }
                    }
                }
            }
        }
        // Bounds grown by two cells and clamped to the map.
        let lo = ((lo.0 - 2).max(0), (lo.1 - 2).max(0));
        let hi = ((hi.0 + 2).min(w - 1), (hi.1 + 2).min(h - 1));
        let clear = |s: &Self, x: i32, y: i32| index(x, y).is_some_and(|i| s.kind[i] == CellType::Clear);
        // "Check for pinched cells, and close them off."
        for j in lo.1..=hi.1 {
            for i in lo.0..=hi.0 {
                let n = index(i, j).unwrap();
                self.pinched[n] = false;
                if self.kind[n] != CellType::Clear {
                    continue;
                }
                let (mut total, mut orthogonal) = (0, 0);
                for k in i - 1..i + 2 {
                    for l in j - 1..j + 2 {
                        if (k, l) != (i, j) && clear(self, k, l) {
                            total += 1;
                            if k == i || l == j {
                                orthogonal += 1;
                            }
                        }
                    }
                }
                // Fewer than two open orthogonal or four open neighbours: pinched.
                if orthogonal < 2 || total < 4 {
                    self.pinched[n] = true;
                }
            }
        }
        for j in lo.1..=hi.1 {
            for i in lo.0..=hi.0 {
                let n = index(i, j).unwrap();
                if self.pinched[n] && self.kind[n] == CellType::Clear {
                    self.kind[n] = CellType::Impassable;
                    self.pinched[n] = false;
                }
            }
        }
        // MARK_BORDER_PINCHED
        for j in lo.1..=hi.1 {
            for i in lo.0..=hi.0 {
                let n = index(i, j).unwrap();
                if self.kind[n] != CellType::Clear {
                    continue;
                }
                let adjacent = [(i - 1, j), (i + 1, j), (i, j - 1), (i, j + 1)]
                    .into_iter()
                    .any(|(k, l)| index(k, l).is_some_and(|m| self.kind[m] == CellType::Obstacle));
                if adjacent {
                    self.pinched[n] = true;
                }
            }
        }
        self.write_back(grid);
    }

    fn write_back(&self, grid: &mut Grid) {
        for (b, k) in grid.blocked.iter_mut().zip(&self.kind) {
            *b = *k != CellType::Clear;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(n: i32) -> Grid {
        Grid { width: n, height: n, blocked: vec![false; (n * n) as usize] }
    }
    fn cells(o: &Obstacles, n: i32, key: u32) -> Vec<(i32, i32)> {
        (0..(n * n) as usize).filter(|i| o.owner[*i] == key).map(|i| (i as i32 % n, i as i32 / n)).collect()
    }

    #[test]
    fn box_footprint_covers_its_extent() {
        let mut g = grid(20);
        let mut o = Obstacles::new(&g);
        o.insert(
            &mut g,
            &Footprint { position: [100.0, 100.0], angle: 0.0, shape: Shape::Box { major: 15.0, minor: 10.0 }, key: 1 },
        );
        // Samples from x 85 to 110 and y 110 down to 95, every 5, with the +0.5 offset.
        let c = cells(&o, 20, 1);
        let xs = c.iter().map(|c| c.0);
        let ys = c.iter().map(|c| c.1);
        assert_eq!((xs.clone().min(), xs.max()), (Some(8), Some(11)), "{c:?}");
        assert_eq!((ys.clone().min(), ys.max()), (Some(9), Some(11)), "{c:?}");
        assert!(c.iter().all(|&(x, y)| o.kind[(y * 20 + x) as usize] == CellType::Obstacle));
        assert!(g.blocked[10 * 20 + 10]);
        // Border cells are pinched but still clear.
        assert!(o.pinched[10 * 20 + 7] && !g.blocked[10 * 20 + 7]);
    }

    #[test]
    fn a_one_cell_gap_between_two_obstacles_is_closed_off() {
        let mut g = grid(20);
        let mut o = Obstacles::new(&g);
        for (key, x) in [(1, 80.0), (2, 130.0)] {
            o.insert(
                &mut g,
                &Footprint { position: [x, 100.0], angle: 0.0, shape: Shape::Box { major: 15.0, minor: 30.0 }, key },
            );
        }
        let a = cells(&o, 20, 1).iter().map(|c| c.0).max().unwrap();
        let b = cells(&o, 20, 2).iter().map(|c| c.0).min().unwrap();
        assert_eq!(b - a, 2, "a one-cell gap");
        let gap = (10 * 20 + a + 1) as usize;
        assert_eq!(o.kind[gap], CellType::Impassable);
        assert!(g.blocked[gap]);
    }

    #[test]
    fn circles_and_fences_rasterize_and_terrain_is_kept() {
        let mut g = grid(30);
        g.blocked[15 * 30 + 15] = true;
        let mut o = Obstacles::new(&g);
        o.insert(
            &mut g,
            &Footprint { position: [150.0, 150.0], angle: 0.0, shape: Shape::Circle { radius: 20.0 }, key: 3 },
        );
        assert_eq!(o.kind[15 * 30 + 15], CellType::Terrain);
        assert_eq!(o.kind[15 * 30 + 13], CellType::Obstacle);
        assert_eq!(o.kind[15 * 30 + 19], CellType::Clear);
        let before = o.count(CellType::Impassable);
        o.insert(
            &mut g,
            &Footprint {
                position: [50.0, 250.0],
                angle: 0.0,
                shape: Shape::Fence { width: 40.0, x_offset: 20.0 },
                key: 4,
            },
        );
        let fence: Vec<usize> = (0..900).filter(|i| o.owner[*i] == 4).collect();
        assert!(fence.len() >= 4 && fence.iter().all(|i| o.fence[*i]), "{fence:?}");
        assert_eq!(o.count(CellType::Impassable), before, "fences are not pinch-processed");
    }
}
