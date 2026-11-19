//! Obstacle-aware path finding for duct legs: a shortest path in a 3D grid (plan cells of 100 mm and the heights a duct may take)
//! that goes around columns and beams, or passes UNDER a beam when the ceiling void is deep enough.
//!
//! - **Obstacles** are the structural beams and columns of the level (boxes of the 3D scene) inflated by half the duct's width and the
//!   clearance you enter. A duct at a given height is blocked by an obstacle when their height ranges, with the duct's own half height
//!   and the clearance, overlap.
//! - **Heights**: the duct normally runs hard under the soffit; if the beams hang lower than the duct's top, a lower height (under the
//!   beams) is offered when it still clears the false ceiling. Changing height costs more than going further in plan, and each turn
//!   costs a little, so the path is straight and high wherever it can be.
//! - **No path** (a beam line with no room under it, or an endpoint inside an obstacle): the straight leg is returned unchanged and the
//!   obstacles it crosses are listed, so the clash check shows the unavoidable intersection and the report names it.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

#[derive(Debug, Clone)]
pub struct Obstacle {
    pub id: String,
    /// `[x0, y0, x1, y1]`, mm.
    pub rect: [i64; 4],
    /// Height range above the floor top of the level, mm.
    pub z0: i64,
    pub z1: i64,
}

#[derive(Debug, Clone)]
pub struct Path {
    /// Waypoints `[x, y, z]`, mm; the first and last are the requested end points.
    pub points: Vec<[i64; 3]>,
    /// The path differs from the straight leg.
    pub rerouted: bool,
    /// Obstacles the straight leg crosses that the path could not avoid (empty when the path is clear).
    pub unavoidable: Vec<String>,
}

const CELL: i64 = 100;

fn blocked(o: &Obstacle, x: i64, y: i64, z: i64, half_w: i64, half_h: i64, cl: i64) -> bool {
    let inflate = half_w + cl;
    x > o.rect[0] - inflate && x < o.rect[2] + inflate && y > o.rect[1] - inflate && y < o.rect[3] + inflate && z + half_h + cl > o.z0 && z - half_h - cl < o.z1
}

fn crosses(obs: &[Obstacle], a: [i64; 2], b: [i64; 2], z: i64, half_w: i64, half_h: i64, cl: i64) -> Vec<String> {
    let steps = (((b[0] - a[0]).abs() + (b[1] - a[1]).abs()) / 25).max(1);
    let mut hit: Vec<String> = Vec::new();
    for s in 0..=steps {
        let (x, y) = (a[0] + (b[0] - a[0]) * s / steps, a[1] + (b[1] - a[1]) * s / steps);
        for o in obs {
            if blocked(o, x, y, z, half_w, half_h, cl) && !hit.contains(&o.id) {
                hit.push(o.id.clone());
            }
        }
    }
    hit
}

/// A path from `a` to `b` at height `zs[0]`, allowed to change to the other heights in `zs`. `bounds` limits the search area.
#[allow(clippy::too_many_arguments)]
pub fn find(a: [i64; 2], b: [i64; 2], zs: &[i64], half_w: i64, half_h: i64, cl: i64, obs: &[Obstacle], bounds: [i64; 4]) -> Path {
    let z0 = zs[0];
    let straight = Path { points: vec![[a[0], a[1], z0], [b[0], b[1], z0]], rerouted: false, unavoidable: Vec::new() };
    let hit = crosses(obs, a, b, z0, half_w, half_h, cl);
    if hit.is_empty() {
        return straight;
    }
    let fail = Path { unavoidable: hit, ..straight };
    // The grid is aligned with the start point, so the first leg is exactly axis-aligned.
    let (lo_x, lo_y) = (bounds[0].min(a[0]).min(b[0]) - CELL, bounds[1].min(a[1]).min(b[1]) - CELL);
    let (ox, oy) = (a[0] - ((a[0] - lo_x) / CELL + 1) * CELL, a[1] - ((a[1] - lo_y) / CELL + 1) * CELL);
    let (nx, ny) = (((bounds[2].max(a[0]).max(b[0]) - ox) / CELL + 2) as usize, ((bounds[3].max(a[1]).max(b[1]) - oy) / CELL + 2) as usize);
    let nz = zs.len();
    let cell_of = |p: [i64; 2]| ((p[0] - ox + CELL / 2) / CELL, (p[1] - oy + CELL / 2) / CELL);
    let free = |ix: i64, iy: i64, iz: usize| -> bool {
        if ix < 0 || iy < 0 || ix >= nx as i64 || iy >= ny as i64 {
            return false;
        }
        let (x, y) = (ox + ix * CELL, oy + iy * CELL);
        // A duct stays inside the search area (the building outline): it does not go round the end of a beam outside the walls.
        if x < bounds[0] || x > bounds[2] || y < bounds[1] || y > bounds[3] {
            return false;
        }
        !obs.iter().any(|o| blocked(o, x, y, zs[iz], half_w, half_h, cl))
    };
    let (sa, sb) = (cell_of(a), cell_of(b));
    if !free(sa.0, sa.1, 0) || !free(sb.0, sb.1, 0) {
        return fail;
    }
    // Dijkstra over (x, y, z index, heading): heading 0..3 or 4 at the start.
    type State = (i64, i64, usize, u8);
    let mut dist: HashMap<State, i64> = HashMap::new();
    let mut prev: HashMap<State, State> = HashMap::new();
    let mut heap: BinaryHeap<Reverse<(i64, State)>> = BinaryHeap::new();
    let start: State = (sa.0, sa.1, 0, 4);
    dist.insert(start, 0);
    heap.push(Reverse((0, start)));
    let dirs = [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)];
    let mut goal: Option<State> = None;
    while let Some(Reverse((d, s))) = heap.pop() {
        if dist.get(&s).is_some_and(|&b| b < d) {
            continue;
        }
        if (s.0, s.1, s.2) == (sb.0, sb.1, 0) {
            goal = Some(s);
            break;
        }
        let mut next: Vec<(State, i64)> = Vec::new();
        for (h, (dx, dy)) in dirs.iter().enumerate() {
            let (x, y) = (s.0 + dx, s.1 + dy);
            if free(x, y, s.2) {
                let turn = if s.3 == 4 || s.3 as usize == h { 0 } else { 3 };
                next.push(((x, y, s.2, h as u8), 1 + turn));
            }
        }
        for z in 0..nz {
            if z != s.2 && free(s.0, s.1, z) {
                next.push(((s.0, s.1, z, 4), 30 + 3));
            }
        }
        for (n, c) in next {
            let nd = d + c;
            if dist.get(&n).is_none_or(|&b| nd < b) {
                dist.insert(n, nd);
                prev.insert(n, s);
                heap.push(Reverse((nd, n)));
            }
        }
    }
    let Some(mut cur) = goal else { return fail };
    let mut cells: Vec<(i64, i64, usize)> = vec![(cur.0, cur.1, cur.2)];
    while let Some(p) = prev.get(&cur) {
        cur = *p;
        cells.push((cur.0, cur.1, cur.2));
    }
    cells.reverse();
    cells.dedup();
    let mut pts: Vec<[i64; 3]> = cells.iter().map(|&(ix, iy, iz)| [ox + ix * CELL, oy + iy * CELL, zs[iz]]).collect();
    pts[0] = [a[0], a[1], z0];
    // The last cell centre is within half a cell of the end point: a jog of at most one cell reaches it along the axes.
    let end = *pts.last().unwrap_or(&[b[0], b[1], z0]);
    if end[0] != b[0] && end[1] != b[1] {
        pts.push([b[0], end[1], end[2]]);
    }
    pts.push([b[0], b[1], z0]);
    pts.dedup();
    // Keep the corner points only (every leg runs along one axis).
    let axis = |p: [i64; 3], q: [i64; 3]| -> (usize, i64) {
        let d = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
        let k = (0..3).find(|&k| d[k] != 0).unwrap_or(0);
        (k, d[k].signum())
    };
    let mut out: Vec<[i64; 3]> = vec![pts[0]];
    for w in 1..pts.len() - 1 {
        if axis(pts[w - 1], pts[w]) != axis(pts[w], pts[w + 1]) {
            out.push(pts[w]);
        }
    }
    out.push(pts[pts.len() - 1]);    Path { points: out, rerouted: true, unavoidable: Vec::new() }
}
