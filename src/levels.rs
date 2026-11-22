//! Multi-storey: planning the levels from the user's brief, assembling them into one model, and keeping the
//! structure of every level in step.
//!
//! - Each room says which floor it is on (floor 1 is the ground floor). A level with no rooms of its own repeats the
//!   level below it, and says so. Room keys must be unique across floors (a repeated room gets `@<level>`).
//! - With more than one floor the user must give the stair width, length and side; nothing is defaulted. The stair
//!   block (shaft + hall) is the same on every level, and the generator fills what is left of the envelope.
//! - Only the ground level has an entrance door and footings. Upper columns pass their load to the columns below.
//! - One level is the old single-storey model, with no `Level` element, so existing projects open unchanged.

use crate::backend::{BriefIn, RoomIn, adjacency_of, envelope_and_entry, room_reqs};
use nnx_core::building::{self, Stair};
use nnx_core::layout::{self, Brief, Layout, Options, RoomReq};
use nnx_core::structural::{self, StructOpts};
use nnx_core::typed::{Column, Level, Space, Typed, element_of};
use nnx_core::walls::{self, WindowOpts};
use nnx_core::{Command, Document, Id, Model, Origin, Source};
use std::collections::BTreeMap;

pub struct LevelPlan {
    /// The brief the generator sees for this level: the envelope minus the stair strip.
    pub brief: Brief,
    pub block: Option<building::Block>,
}

pub struct BuildingPlan {
    pub levels: Vec<LevelPlan>,
    /// Every room of every level plus the stair block, over the whole envelope: what plan edits look rooms up in.
    pub merged: Brief,
    pub labels: BTreeMap<String, String>,
    pub notes: Vec<String>,
}

/// One alternative: a layout per level, ground first.
#[derive(Clone)]
pub struct Building {
    pub levels: Vec<Layout>,
}

fn level_name(i: usize) -> String {
    if i == 0 { "Ground floor".into() } else { format!("Floor {i}") }
}

/// The wall thickness the user gave (exterior, interior), when either is given; the engine's default fills the other.
fn wall_mm_of(b: &BriefIn) -> Option<(i64, i64)> {
    if b.wall_exterior_mm.is_none() && b.wall_interior_mm.is_none() {
        return None;
    }
    let d = walls::WallOpts::default();
    Some((b.wall_exterior_mm.unwrap_or(d.exterior_mm), b.wall_interior_mm.unwrap_or(d.interior_mm)))
}

pub fn apply_wall_mm(wo: &mut walls::WallOpts, wall_mm: Option<(i64, i64)>) {
    if let Some((e, i)) = wall_mm {
        wo.exterior_mm = e;
        wo.interior_mm = i;
    }
}

pub fn apply_wall_height(wo: &mut walls::WallOpts, height_mm: Option<i64>) {
    if let Some(h) = height_mm {
        wo.height_mm = h;
    }
}

pub fn plan_building(b: &BriefIn, n: u32) -> Result<BuildingPlan, String> {
    if let Some(h) = b.wall_height_mm.filter(|h| *h <= 0) {
        return Err(format!("wall height (storey height) must be positive, not {h} mm"));
    }
    if let Some(r) = b.rooms.iter().find(|r| r.phase.as_deref().is_some_and(|p| !p.is_empty() && !nnx_core::qto::PHASES.contains(&p))) {
        return Err(format!("room '{}' has the phase '{}': use one of {}", r.key, r.phase.as_deref().unwrap_or_default(), nnx_core::qto::PHASES.join(", ")));
    }
    let n = n.max(1) as usize;
    let (envelope, entry) = envelope_and_entry(b)?;
    if let Some(r) = b.rooms.iter().find(|r| r.floor.unwrap_or(1) == 0 || r.floor.unwrap_or(1) as usize > n) {
        return Err(format!("room '{}' is on floor {} but the building has {n} floor(s)", r.key, r.floor.unwrap_or(1)));
    }
    let own: Vec<Vec<&RoomIn>> = (1..=n).map(|f| b.rooms.iter().filter(|r| r.floor.unwrap_or(1) as usize == f).collect()).collect();
    if own[0].is_empty() {
        return Err("the ground floor has no rooms".into());
    }
    let mut notes = Vec::new();
    let mut labels: BTreeMap<String, String> = BTreeMap::new();
    let stair = if n > 1 {
        let s = b.stair.as_ref().ok_or("the building has several floors: enter the stair width, length and side")?;
        let west = match s.side.as_str() {
            "west" => true,
            "east" => false,
            other => return Err(format!("unknown stair side '{other}' (west or east)")),
        };
        Some(Stair { width_mm: (s.width_m * 1000.0).round() as i64, length_mm: (s.length_m * 1000.0).round() as i64, west })
    } else {
        None
    };

    let mut levels = Vec::new();
    let mut merged_rooms: Vec<RoomReq> = Vec::new();
    let mut merged_adj = Vec::new();
    let mut seen_keys: BTreeMap<String, usize> = BTreeMap::new();
    let mut source: Vec<&RoomIn> = Vec::new();
    for (l, rooms) in own.iter().enumerate() {
        let repeats = rooms.is_empty();
        if repeats {
            notes.push(format!("floor {} has no rooms of its own: it repeats floor {}", l + 1, l));
        } else {
            source = rooms.clone();
        }
        let suffix = if l == 0 { String::new() } else { format!("@{l}") };
        let (mut reqs, lab) = room_reqs(&source)?;
        if repeats {
            for r in &mut reqs {
                r.key = format!("{}@{l}", r.key);
            }
            for (k, v) in lab {
                labels.insert(format!("{k}@{l}"), v);
            }
        } else {
            labels.extend(lab);
        }
        for r in &reqs {
            if let Some(prev) = seen_keys.insert(r.key.clone(), l + 1) {
                return Err(format!("room key '{}' is used on floor {prev} and floor {}: keys must be unique", r.key, l + 1));
            }
        }
        let adjacency = adjacency_of(&reqs);
        let (main_env, block) = match &stair {
            Some(s) => {
                let blk = building::stair_block(envelope, entry, s, &suffix)?;
                (blk.main, Some(blk))
            }
            None => (envelope, None),
        };
        merged_rooms.extend(reqs.clone());
        merged_adj.extend(adjacency.clone());
        if let Some(blk) = &block {
            for pr in &blk.rooms {
                merged_rooms.push(RoomReq { key: pr.key.clone(), func: pr.func.clone(), target_area_m2: pr.area_m2, min_width_mm: 1000, zone: pr.zone, needs_light: pr.needs_light });
                let name = if pr.func == "stair" { "Stair" } else { "Stair hall" };
                labels.insert(pr.key.clone(), name.into());
            }
        }
        levels.push(LevelPlan { brief: Brief { envelope: main_env, rooms: reqs, adjacency, entry, entry_at: None, wall_mm: wall_mm_of(b), wall_height_mm: b.wall_height_mm }, block });
    }
    labels.insert("reserve".into(), "Hall".into());
    for l in 1..n {
        labels.insert(format!("reserve@{l}"), "Hall".into());
    }
    // Filler rooms of the extra pieces when rooms are locked.
    for i in 1..4 {
        labels.insert(format!("reserve{i}"), "Hall".into());
        for l in 1..n {
            labels.insert(format!("reserve{i}@{l}"), "Hall".into());
        }
    }
    Ok(BuildingPlan { levels, merged: Brief { envelope, rooms: merged_rooms, adjacency: merged_adj, entry, entry_at: None, wall_mm: wall_mm_of(b), wall_height_mm: b.wall_height_mm }, labels, notes })
}

/// One level with some rooms locked at exact rectangles (the envelope's frame, millimetres). The locked rooms are kept as they
/// are; the other rooms are generated in the largest free rectangle that the locked rooms leave. Whatever else is left of the
/// envelope stays empty: nothing is invented to fill it.
fn generate_level(lp: &LevelPlan, level: usize, seed: u64, locked: &BTreeMap<String, layout::Rect>) -> Result<Vec<Layout>, String> {
    let fixed: Vec<(&RoomReq, layout::Rect)> = lp.brief.rooms.iter().filter_map(|r| locked.get(&r.key).map(|rc| (r, *rc))).collect();
    if fixed.is_empty() {
        return layout::generate(&lp.brief, &Options { seed, ..Options::default() }).map_err(|e| e.to_string());
    }
    let env = lp.brief.envelope;
    let overlap = |a: &layout::Rect, b: &layout::Rect| a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
    for (i, (req, rc)) in fixed.iter().enumerate() {
        if rc.x < env.x || rc.y < env.y || rc.x + rc.w > env.x + env.w || rc.y + rc.h > env.y + env.h {
            return Err(format!("the locked room '{}' lies outside the buildable area", req.key));
        }
        for (other, oc) in fixed.iter().skip(i + 1) {
            if overlap(rc, oc) {
                return Err(format!("the locked rooms '{}' and '{}' overlap", req.key, other.key));
            }
        }
    }
    let placed = || -> Vec<layout::PlacedRoom> {
        fixed.iter().map(|(req, rc)| layout::PlacedRoom { key: req.key.clone(), func: req.func.clone(), zone: req.zone, needs_light: req.needs_light, rect: *rc, area_m2: rc.area_m2(), target_m2: req.target_area_m2, deviation_pct: (rc.area_m2() - req.target_area_m2) / req.target_area_m2 * 100.0 }).collect()
    };
    let free: Vec<&RoomReq> = lp.brief.rooms.iter().filter(|r| !locked.contains_key(&r.key)).collect();
    if free.is_empty() {
        return Ok(vec![Layout { rooms: placed(), cost: 0.0, breakdown: layout::Breakdown::default(), reserve_m2: 0.0 }]);
    }
    // The free space is cut into up to four rectangles: repeatedly the largest rectangle (sides on the envelope's or a blocked
    // room's edges) that touches nothing locked and nothing already taken. The other rooms are shared among them by area.
    let mut blocked: Vec<layout::Rect> = fixed.iter().map(|(_, rc)| *rc).collect();
    let mut regions: Vec<layout::Rect> = Vec::new();
    let total: f64 = free.iter().map(|q| q.target_area_m2).sum();
    while regions.len() < 4 {
        let mut xs = vec![env.x, env.x + env.w];
        let mut ys = vec![env.y, env.y + env.h];
        for rc in &blocked {
            xs.extend([rc.x, rc.x + rc.w]);
            ys.extend([rc.y, rc.y + rc.h]);
        }
        xs.sort_unstable();
        xs.dedup();
        ys.sort_unstable();
        ys.dedup();
        let mut best: Option<layout::Rect> = None;
        for (i, &x0) in xs.iter().enumerate() {
            for &x1 in &xs[i + 1..] {
                for (j, &y0) in ys.iter().enumerate() {
                    for &y1 in &ys[j + 1..] {
                        let c = layout::Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
                        if c.w >= 1500 && c.h >= 1500 && !blocked.iter().any(|b| overlap(&c, b)) && best.is_none_or(|b| c.w * c.h > b.w * b.h) {
                            best = Some(c);
                        }
                    }
                }
            }
        }
        let Some(c) = best else { break };
        blocked.push(c);
        regions.push(c);
        if regions.iter().map(|g| g.area_m2()).sum::<f64>() >= total * 1.15 {
            break;
        }
    }
    let space: f64 = regions.iter().map(|g| g.area_m2()).sum();
    if regions.is_empty() {
        return Err("the locked rooms leave no free space for the other rooms".into());
    }
    // Biggest room first, into the region with the most space left.
    let mut left: Vec<f64> = regions.iter().map(|g| g.area_m2()).collect();
    let mut share: Vec<Vec<&RoomReq>> = vec![Vec::new(); regions.len()];
    let mut order = free.clone();
    order.sort_by(|a, b| b.target_area_m2.total_cmp(&a.target_area_m2));
    for room in order {
        let (i, _) = left.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).ok_or("no region")?;
        if left[i] + 1e-9 < room.target_area_m2 {
            return Err(format!("the space left by the locked rooms ({space:.1} m2 in {} piece(s)) cannot hold the other rooms ({total:.1} m2)", regions.len()));
        }
        left[i] -= room.target_area_m2;
        share[i].push(room);
    }
    let mut per_region: Vec<Vec<Layout>> = Vec::new();
    for (idx, (region, rooms)) in regions.iter().zip(&share).enumerate() {
        if rooms.is_empty() {
            continue;
        }
        let names: std::collections::BTreeSet<&str> = rooms.iter().map(|q| q.key.as_str()).collect();
        let sub = Brief { envelope: *region, rooms: rooms.iter().map(|q| (*q).clone()).collect(), adjacency: lp.brief.adjacency.iter().filter(|a| names.contains(a.a.as_str()) && names.contains(a.b.as_str())).cloned().collect(), entry: lp.brief.entry, entry_at: None, wall_mm: lp.brief.wall_mm, wall_height_mm: lp.brief.wall_height_mm };
        let mut ls = layout::generate(&sub, &Options { seed: seed.wrapping_add(idx as u64 * 977), ..Options::default() }).map_err(|e| format!("the space left by the locked rooms ({:.1} m2): {e}", region.area_m2()))?;
        // Circulation filler rooms of the second and later pieces get their own keys.
        if idx > 0 {
            for l in &mut ls {
                for q in l.rooms.iter_mut().filter(|q| q.key == "reserve") {
                    q.key = if level == 0 { format!("reserve{idx}") } else { format!("reserve{idx}@{level}") };
                }
            }
        }
        per_region.push(ls);
    }
    let count = per_region.iter().map(Vec::len).min().unwrap_or(1).max(1);
    Ok((0..count)
        .map(|k| {
            let mut rooms = placed();
            let (mut cost, mut reserve) = (0.0, 0.0);
            for v in &per_region {
                let l = &v[k.min(v.len() - 1)];
                rooms.extend(l.rooms.iter().cloned());
                cost += l.cost;
                reserve += l.reserve_m2;
            }
            Layout { rooms, cost, breakdown: layout::Breakdown::default(), reserve_m2: reserve }
        })
        .collect())
}
/// Alternatives for the whole building: alternative `k` takes the `k`-th layout of every level. Some rooms may be locked at exact\n/// rectangles (room key -> rectangle); with none locked this is the plain generator.
pub fn generate_building_locked(plan: &BuildingPlan, seed: u64, locked: &BTreeMap<String, layout::Rect>) -> Result<Vec<Building>, String> {
    let mut per_level: Vec<Vec<Layout>> = Vec::new();
    for (l, lp) in plan.levels.iter().enumerate() {
        let layouts = generate_level(lp, l, seed.wrapping_add(l as u64), locked).map_err(|e| format!("floor {}: {e}", l + 1))?;
        per_level.push(
            layouts
                .into_iter()
                .map(|mut lay| {
                    // Space keys are unique across the building: the reserve (circulation) room of an upper level
                    // is `reserve@<level>`.
                    if l > 0 {
                        for r in lay.rooms.iter_mut().filter(|r| r.key == "reserve") {
                            r.key = format!("reserve@{l}");
                        }
                    }
                    if let Some(blk) = &lp.block {
                        lay.rooms.extend(blk.rooms.iter().cloned());
                    }
                    lay
                })
                .collect(),
        );
    }
    let count = per_level.iter().map(Vec::len).min().unwrap_or(0).max(1);
    Ok((0..count).map(|k| Building { levels: per_level.iter().map(|v| v[k.min(v.len() - 1)].clone()).collect() }).collect())
}

/// Edges of two marked rooms closer than this (a wall's thickness, however the user measured it) are taken to be one shared wall.
pub const SNAP_MM: i64 = 500;

/// Marked rectangles are only as exact as the clicks that made them, and a user marks the inside of each room, so two
/// neighbours end up a wall apart (or a hair overlapping). Left like that the engine would give every room its own outer
/// walls and no door could join them. Facing edges of rooms that share at least 0.5 m of side, and edges lying within
/// `tol` of the buildable envelope, are therefore moved onto one line (the midpoint for two rooms, the envelope edge for
/// the envelope). Edges farther apart than `tol` are left exactly as marked: a real gap stays a gap.
pub fn snap_rects(r: &mut [layout::Rect], env: &layout::Rect, tol: i64) {
    // Every edge is a node; edges that face each other within `tol` (and share a side of at least 0.5 m) are united.
    // Each group then takes ONE value, applied to all its members at once, so nothing is moved twice.
    // x nodes: left of room i = 2i, right = 2i + 1; the envelope's left and right follow them. y nodes likewise.
    fn find(p: &mut [usize], mut a: usize) -> usize {
        while p[a] != a {
            p[a] = p[p[a]];
            a = p[a];
        }
        a
    }
    fn union(p: &mut [usize], a: usize, b: usize) {
        let (a, b) = (find(p, a), find(p, b));
        if a != b {
            p[b] = a;
        }
    }
    let n = r.len();
    let min_shared = 500;
    let mut px: Vec<usize> = (0..2 * n + 2).collect();
    let mut py: Vec<usize> = (0..2 * n + 2).collect();
    let (env_l, env_r) = (2 * n, 2 * n + 1);
    let base: Vec<layout::Rect> = r.to_vec();
    let xv = |i: usize, node: usize| if node == 2 * i { base[i].x } else { base[i].x + base[i].w };
    let yv = |i: usize, node: usize| if node == 2 * i { base[i].y } else { base[i].y + base[i].h };
    for i in 0..n {
        for j in 0..n {
            if i == j {
                continue;
            }
            let (a, b) = (r[i], r[j]);
            if (a.y + a.h).min(b.y + b.h) - a.y.max(b.y) >= min_shared && (a.x + a.w - b.x).abs() <= tol {
                union(&mut px, 2 * i + 1, 2 * j);
            }
            if (a.x + a.w).min(b.x + b.w) - a.x.max(b.x) >= min_shared && (a.y + a.h - b.y).abs() <= tol {
                union(&mut py, 2 * i + 1, 2 * j);
            }
        }
        if (r[i].x - env.x).abs() <= tol {
            union(&mut px, 2 * i, env_l);
        }
        if (r[i].x + r[i].w - (env.x + env.w)).abs() <= tol {
            union(&mut px, 2 * i + 1, env_r);
        }
        if (r[i].y - env.y).abs() <= tol {
            union(&mut py, 2 * i, env_l);
        }
        if (r[i].y + r[i].h - (env.y + env.h)).abs() <= tol {
            union(&mut py, 2 * i + 1, env_r);
        }
    }
    // The value of a group: the envelope's own coordinate when it holds an envelope edge, else the mean of its edges.
    let group_values = |p: &mut Vec<usize>, val: &dyn Fn(usize) -> i64, lo: i64, hi: i64| -> Vec<i64> {
        let total = p.len();
        let mut sum = vec![0i64; total];
        let mut cnt = vec![0i64; total];
        for node in 0..2 * n {
            let g = find(p, node);
            sum[g] += val(node);
            cnt[g] += 1;
        }
        let (gl, gr) = (find(p, env_l), find(p, env_r));
        (0..total).map(|node| {
            let g = find(p, node);
            if g == gl && g != gr { lo } else if g == gr && g != gl { hi } else if cnt[g] > 0 { (((sum[g] as f64 / cnt[g] as f64) / 10.0).round() as i64) * 10 } else { 0 }
        }).collect()
    };
    let marked: Vec<layout::Rect> = r.to_vec();
    let overlap_pair = |a: &layout::Rect, b: &layout::Rect| a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
    // Two groups that settle a few millimetres apart can push a corner of one room into another. When that happens the
    // two facing edges are joined into one group and the values are worked out again, so the neighbours stay joined.
    for _ in 0..(4 * n + 4) {
        let xs = group_values(&mut px, &|node| xv(node / 2, node), env.x, env.x + env.w);
        let ys = group_values(&mut py, &|node| yv(node / 2, node), env.y, env.y + env.h);
        for (i, q) in r.iter_mut().enumerate() {
            let (l, rt, b, t) = (xs[2 * i], xs[2 * i + 1], ys[2 * i], ys[2 * i + 1]);
            // A group so large that a room would collapse leaves that room as it was marked.
            *q = if rt - l >= 100 && t - b >= 100 { layout::Rect { x: l, y: b, w: rt - l, h: t - b } } else { marked[i] };
        }
        let mut joined = false;
        'scan: for i in 0..n {
            for j in i + 1..n {
                if !overlap_pair(&r[i], &r[j]) || overlap_pair(&marked[i], &marked[j]) {
                    continue;
                }
                let pen_x = (r[i].x + r[i].w).min(r[j].x + r[j].w) - r[i].x.max(r[j].x);
                let pen_y = (r[i].y + r[i].h).min(r[j].y + r[j].h) - r[i].y.max(r[j].y);
                let (p, a, b) = if pen_x <= pen_y {
                    if marked[i].x < marked[j].x { (&mut px, 2 * i + 1, 2 * j) } else { (&mut px, 2 * j + 1, 2 * i) }
                } else if marked[i].y < marked[j].y {
                    (&mut py, 2 * i + 1, 2 * j)
                } else {
                    (&mut py, 2 * j + 1, 2 * i)
                };
                if find(p, a) != find(p, b) {
                    union(p, a, b);
                    joined = true;
                    break 'scan;
                }
            }
        }
        if !joined {
            break;
        }
    }
    // Snapping must never turn a valid marking into an overlap: two rooms that overlap afterwards go back to what the user marked.
    let overlap = |a: &layout::Rect, b: &layout::Rect| a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
    for _ in 0..n {
        let mut reverted = false;
        for i in 0..n {
            for j in i + 1..n {
                if overlap(&r[i], &r[j]) && !overlap(&marked[i], &marked[j]) && (r[i] != marked[i] || r[j] != marked[j]) {
                    r[i] = marked[i];
                    r[j] = marked[j];
                    reverted = true;
                }
            }
        }
        if !reverted {
            break;
        }
    }
}

/// A building whose levels follow rooms the user marked on a sketch. `placement` maps a room key to its rectangle
/// `{x, y, w, h}` in millimetres, in the buildable envelope's own frame (origin at its south-west corner, y up).
/// A level is taken from the sketch only when every one of its rooms is marked; the other levels come from `other`.
/// Rooms must lie inside the envelope and must not overlap each other or the stair block. The rest is left empty:
/// nothing is invented to fill the gaps.
pub fn placed_building(plan: &BuildingPlan, placement: &serde_json::Value, other: &Building) -> Result<Building, String> {
    let get = |key: &str| -> Option<layout::Rect> {
        let r = placement.get(key)?;
        let f = |k: &str| r.get(k).and_then(serde_json::Value::as_f64);
        let round10 = |v: f64| ((v / 10.0).round() as i64) * 10;
        Some(layout::Rect { x: round10(f("x")?), y: round10(f("y")?), w: round10(f("w")?), h: round10(f("h")?) })
    };
    let overlap = |a: &layout::Rect, b: &layout::Rect| a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
    // A stair marked on the sketch (`_stair`) replaces the generator's strip along the west or east side: the shaft sits where it
    // was marked, on every level, and the rooms around it are the ones the user marked (the hall beside it is just another room).
    let stair_free = get("_stair");
    let mut used = 0;
    let mut out = Vec::new();
    for (l, lp) in plan.levels.iter().enumerate() {
        let rects: Vec<Option<layout::Rect>> = lp.brief.rooms.iter().map(|r| get(&r.key)).collect();
        if rects.is_empty() || rects.iter().any(Option::is_none) {
            if stair_free.is_some() {
                return Err(format!("the stair is placed on the sketch, so every room of every level must be marked: level {} is not", l + 1));
            }
            out.push(other.levels[l.min(other.levels.len() - 1)].clone());
            continue;
        }
        let env = if stair_free.is_some() { plan.merged.envelope } else { lp.brief.envelope };
        let mut marked: Vec<layout::Rect> = rects.into_iter().flatten().collect();
        let n_rooms = marked.len();
        marked.extend(stair_free);
        snap_rects(&mut marked, &env, SNAP_MM);
        let shaft = if stair_free.is_some() { marked.pop() } else { None };
        debug_assert_eq!(marked.len(), n_rooms);
        let mut rooms: Vec<layout::PlacedRoom> = Vec::new();
        for (req, rect) in lp.brief.rooms.iter().zip(marked) {
            if rect.w < 500 || rect.h < 500 {
                return Err(format!("room '{}' marked on the sketch is smaller than 0.5 m in one side", req.key));
            }
            if rect.x < env.x || rect.y < env.y || rect.x + rect.w > env.x + env.w || rect.y + rect.h > env.y + env.h {
                return Err(format!("room '{}' marked on the sketch lies outside the buildable area (plot minus setbacks)", req.key));
            }
            let area = rect.area_m2();
            rooms.push(layout::PlacedRoom { key: req.key.clone(), func: req.func.clone(), zone: req.zone, needs_light: req.needs_light, rect, area_m2: area, target_m2: req.target_area_m2, deviation_pct: (area - req.target_area_m2) / req.target_area_m2 * 100.0 });
        }
        if let Some(rect) = shaft {
            if rect.w < 500 || rect.h < 500 {
                return Err("the stair marked on the sketch is smaller than 0.5 m in one side".into());
            }
            if rect.x < env.x || rect.y < env.y || rect.x + rect.w > env.x + env.w || rect.y + rect.h > env.y + env.h {
                return Err("the stair marked on the sketch lies outside the buildable area".into());
            }
            let suffix = if l == 0 { String::new() } else { format!("@{l}") };
            rooms.push(layout::PlacedRoom { key: format!("stair{suffix}"), func: "stair".into(), zone: layout::Zone::Service, needs_light: false, rect, area_m2: rect.area_m2(), target_m2: rect.area_m2(), deviation_pct: 0.0 });
        } else if let Some(blk) = &lp.block {
            rooms.extend(blk.rooms.iter().cloned());
        }
        for i in 0..rooms.len() {
            for j in i + 1..rooms.len() {
                if overlap(&rooms[i].rect, &rooms[j].rect) {
                    return Err(format!("rooms '{}' and '{}' overlap on the sketch", rooms[i].key, rooms[j].key));
                }
            }
        }
        used += 1;
        out.push(layout::Layout { rooms, cost: 0.0, breakdown: layout::Breakdown::default(), reserve_m2: 0.0 });
    }
    if used == 0 {
        return Err("no level has all its rooms marked on the sketch".into());
    }
    Ok(Building { levels: out })
}

/// The structural options for one level, derived from the model itself so a sync after opening a file agrees with
/// the one made at generation: no footings above the ground, columns above pass their loads down, and a stair
/// shaft is left open in the slab unless the level is the top one.
pub fn level_opts(model: &Model, base: &StructOpts, level: Option<Id>) -> StructOpts {
    let Some(lid) = level else { return base.clone() };
    let lv = building::levels(model);
    let idx = lv.iter().position(|(_, id)| *id == lid).unwrap_or(0);
    let mut o = base.clone();
    o.floors = 1;
    o.footings = idx == 0;
    if idx + 1 < lv.len() {
        o.open_rooms = model.elements.values().filter(|e| e.kind == Space::KIND && e.level_id == Some(lid)).filter_map(|e| e.typed::<Space>().ok()).filter(|s| s.func == "stair").map(|s| s.requirement_key).collect();
        let above = lv[idx + 1].1;
        let cols: Vec<Column> = model.elements.values().filter(|e| e.kind == Column::KIND && e.level_id == Some(above)).filter_map(|e| e.typed::<Column>().ok()).collect();
        o.forced_columns = cols.iter().map(|c| (c.x, c.y)).collect();
        o.extra_loads = cols.iter().filter_map(|c| Some((c.x, c.y, c.axial_service_kn?))).collect();
    }
    o
}

#[derive(Debug, Default)]
pub struct ChainReport {
    pub created: usize,
    pub removed: usize,
    pub unchanged: usize,
    /// Doors and windows moved along their wall to clear a column.
    pub shifted: usize,
    pub warnings: Vec<String>,
}

/// Brings the structure of the levels `from` and below in line with the walls, top down (a level needs the loads of
/// the one above). Each step is applied to `scratch`; the commands of all steps are returned in order. `from` is a
/// level index; `None` means every level.
pub fn structure_chain(scratch: &mut Document, base: &StructOpts, from: Option<usize>) -> Result<(Vec<Command>, ChainReport), String> {
    let lv = building::levels(&scratch.model);
    let mut all = Vec::new();
    let mut rep = ChainReport::default();
    let steps: Vec<Option<Id>> = if lv.is_empty() { vec![None] } else { lv.iter().enumerate().rev().filter(|(i, _)| from.is_none_or(|f| *i <= f)).map(|(_, (_, id))| Some(*id)).collect() };
    let mut orphans = 0usize;
    for lid in steps {
        let opts = level_opts(&scratch.model, base, lid);
        let (cmds, r) = structural::sync_commands(&scratch.model, &opts, lid)?;
        rep.created += r.created;
        rep.removed += r.removed;
        rep.unchanged += r.unchanged;
        for w in r.warnings {
            if w.starts_with("a column above at") {
                orphans += 1;
            } else if !rep.warnings.contains(&w) {
                rep.warnings.push(w);
            }
        }
        if !cmds.is_empty() {
            scratch.commit("structure", Origin::Generator, cmds.clone()).map_err(|e| e.to_string())?;
            all.extend(cmds);
        }
    }
    if orphans > 0 {
        rep.warnings.push(format!("{orphans} column(s) on upper levels have no column below them: their load is not carried to the foundations (align the rooms or add transfer beams; an engineer must decide)"));
    }
    Ok((all, rep))
}

/// The structure of the levels `from` and below, then every door or window standing in a column moved clear of it,
/// repeated until nothing changes (moving an opening can change where an extra column goes). All commands are
/// applied to `scratch` and returned in order. Openings that cannot be moved are named in the report.
pub fn settle(scratch: &mut Document, base: &StructOpts, from: Option<usize>) -> Result<(Vec<Command>, ChainReport), String> {
    let mut all = Vec::new();
    let mut total = ChainReport::default();
    for round in 0..4 {
        let (cmds, rep) = structure_chain(scratch, base, from)?;
        all.extend(cmds);
        total.created += rep.created;
        total.removed += rep.removed;
        if round == 0 {
            total.unchanged = rep.unchanged;
        }
        let lv = building::levels(&scratch.model);
        let ids: Vec<Option<Id>> = if lv.is_empty() { vec![None] } else { lv.iter().enumerate().filter(|(i, _)| from.is_none_or(|f| *i <= f)).map(|(_, (_, id))| Some(*id)).collect() };
        let (fix, notes) = building::clear_openings(&scratch.model, &ids);
        if fix.is_empty() {
            total.warnings = rep.warnings;
            total.warnings.extend(notes);
            if total.shifted > 0 {
                total.warnings.insert(0, format!("{} door/window position(s) were moved along their wall to clear columns", total.shifted));
            }
            return Ok((all, total));
        }
        total.shifted += fix.len();
        scratch.commit("move openings clear of columns", Origin::Generator, fix.clone()).map_err(|e| e.to_string())?;
        all.extend(fix);
    }
    total.warnings.push("door and window positions did not settle after 4 passes: check the openings next to columns".into());
    Ok((all, total))
}

/// Commits the structure of every level as one undoable step on `doc`.
pub fn sync_all(doc: &mut Document, base: &StructOpts) -> Result<ChainReport, String> {
    let mut scratch = Document::new(doc.model.clone());
    let (cmds, rep) = settle(&mut scratch, base, None)?;
    if !cmds.is_empty() {
        doc.commit("sync structure with the model", Origin::Generator, cmds).map_err(|e| e.to_string())?;
    }
    Ok(rep)
}

/// Spaces, walls, doors, windows and structure of the whole building, committed as one step.
/// `project`, when given, becomes the model's project element inside the same step, so one undo of the generation
/// removes the plan and its project together.
pub fn assemble(b: &Building, plan: &BuildingPlan, base: &StructOpts, project: Option<&nnx_core::project::NanoArchProject>) -> Result<(Document, Vec<String>), String> {
    let n = b.levels.len();
    let multi = n > 1;
    let mut arch: Vec<Command> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for (l, layout) in b.levels.iter().enumerate() {
        let lid = if multi {
            let el = element_of(&Level { name: level_name(l), index: l as i64, extra: Default::default() }, Source::Generated);
            let id = el.id;
            arch.push(Command::Create(el));
            Some(id)
        } else {
            None
        };
        let mut wo = walls::opts_from_adjacency(&plan.levels[l].brief.adjacency);
        wo.entry = l == 0;
        wo.entry_at = plan.merged.entry_at;
        apply_wall_mm(&mut wo, plan.merged.wall_mm);
        apply_wall_height(&mut wo, plan.merged.wall_height_mm);
        let wplan = walls::build(layout, plan.merged.envelope, plan.merged.entry, &wo);
        let wp = walls::place_windows(layout, &wplan, &wo, &WindowOpts { target_ratio: nnx_core::sbc::daylight_targets(), ..WindowOpts::default() });
        let space_cmds = layout::to_commands(layout, lid);
        let ids = walls::space_ids(&space_cmds);
        arch.extend(space_cmds);
        arch.extend(walls::to_commands(&wplan, &ids, lid));
        arch.extend(walls::window_commands(&wplan, &wp, &ids, lid));
        let tag = if multi { format!("{}: ", level_name(l)) } else { String::new() };
        warnings.extend(wplan.warnings.iter().chain(&wp.warnings).map(|w| format!("{tag}{w}")));
    }
    // The structure is derived from the architecture as the MODEL holds it, exactly as a later sync does.
    let mut scratch = Document::default();
    scratch.commit("architecture", Origin::Generator, arch.clone()).map_err(|e| e.to_string())?;
    let (cmds, rep) = settle(&mut scratch, base, None)?;
    warnings.extend(rep.warnings);
    let mut all = arch;
    all.extend(cmds);
    if let Some(p) = project {
        all.push(nnx_core::project::create_command(p)?);
    }
    let mut doc = Document::default();
    doc.commit("generate house", Origin::Generator, all).map_err(|e| e.to_string())?;
    Ok((doc, warnings))
}

#[cfg(test)]
mod snap_tests {
    use super::*;
    fn r(x: i64, y: i64, w: i64, h: i64) -> layout::Rect {
        layout::Rect { x, y, w, h }
    }
    #[test]
    fn neighbours_a_wall_apart_share_one_edge() {
        let env = r(0, 0, 14000, 12000);
        let mut v = vec![r(0, 0, 4800, 4000), r(5120, 0, 4000, 4000), r(0, 4300, 4800, 3000)];
        snap_rects(&mut v, &env, SNAP_MM);
        assert_eq!(v[0].x + v[0].w, v[1].x, "left and right rooms meet on one line");
        assert_eq!(v[0].y + v[0].h, v[2].y, "the room above meets the one below");
    }
    #[test]
    fn a_real_gap_is_left_alone() {
        let env = r(0, 0, 14000, 12000);
        let mut v = vec![r(0, 0, 4000, 4000), r(6000, 0, 4000, 4000)];
        let before = v.clone();
        snap_rects(&mut v, &env, SNAP_MM);
        assert_eq!(v, before, "a 2 m gap is not a wall");
    }
    #[test]
    fn rooms_that_only_touch_at_a_corner_are_not_joined() {
        let env = r(0, 0, 14000, 12000);
        let mut v = vec![r(1000, 1000, 3000, 3000), r(4200, 4200, 3000, 3000)];
        let before = v.clone();
        snap_rects(&mut v, &env, SNAP_MM);
        assert_eq!(v, before, "no shared side of 0.5 m, no snap");
    }
    #[test]
    fn a_chain_of_near_edges_never_ends_in_an_overlap() {
        // Rooms of a real ground floor, marked by hand: a study, a wardrobe, a bathroom, a master bedroom and a sitting room,
        // each a different small distance from its neighbours.
        let env = r(0, 0, 40000, 30000);
        let mut v = vec![
            r(20694, 23056, 5556, 3889),
            r(26614, 22270, 3356, 1816),
            r(26614, 24245, 3356, 2488),
            r(30168, 22270, 6634, 4462),
            r(31629, 17730, 5173, 4383),
            r(26614, 17750, 3356, 4350),
        ];
        snap_rects(&mut v, &env, SNAP_MM);
        for i in 0..v.len() {
            for j in i + 1..v.len() {
                let (a, b) = (v[i], v[j]);
                assert!(!(a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h), "rooms {i} and {j} overlap: {a:?} {b:?}");
            }
        }
        assert_eq!(v[1].x + v[1].w, v[3].x, "the wardrobe and the master bedroom share one wall");
        assert_eq!(v[1].y + v[1].h, v[2].y, "the wardrobe and the bathroom share one wall");
    }
    #[test]
    fn edges_near_the_envelope_move_onto_it() {
        let env = r(0, 0, 10000, 8000);
        let mut v = vec![r(150, 200, 4000, 3000), r(5000, 4000, 4850, 3900)];
        snap_rects(&mut v, &env, SNAP_MM);
        assert_eq!((v[0].x, v[0].y), (0, 0));
        assert_eq!((v[1].x + v[1].w, v[1].y + v[1].h), (10000, 8000));
    }
    #[test]
    fn groups_that_settle_apart_are_joined_instead_of_reverting_a_neighbour() {
        // Marked from the Al-Andalus ground floor: the corner of the dressing room fell into the narrow corridor after snapping,
        // which used to put the corridor back to its marked position and leave a gap that no door could cross.
        let mut v = vec![
            layout::Rect { x: 33330, y: 12380, w: 2870, h: 2150 },
            layout::Rect { x: 36300, y: 12380, w: 5610, h: 3800 },
            layout::Rect { x: 33220, y: 14520, w: 3020, h: 1710 },
            layout::Rect { x: 33330, y: 16340, w: 2810, h: 1750 },
            layout::Rect { x: 36270, y: 16240, w: 1220, h: 6300 },
            layout::Rect { x: 33140, y: 18150, w: 3070, h: 4400 },
            layout::Rect { x: 37520, y: 16240, w: 4390, h: 3730 },
            layout::Rect { x: 37570, y: 21970, w: 4300, h: 4620 },
            layout::Rect { x: 35740, y: 22610, w: 1720, h: 4060 },
        ];
        let env = layout::Rect { x: 0, y: 0, w: 60000, h: 60000 };
        snap_rects(&mut v, &env, SNAP_MM);
        for i in 0..v.len() {
            for j in i + 1..v.len() {
                let (a, b) = (v[i], v[j]);
                assert!(!(a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h), "rooms {i} and {j} overlap: {a:?} {b:?}");
            }
        }
        assert_eq!(v[5].x + v[5].w, v[4].x, "the corridor shares the wall of the room beside it");
    }
}

