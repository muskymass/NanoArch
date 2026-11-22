//! Layout generator: turns a room program and a buildable rectangle into several
//! alternative floor layouts.
//!
//! Method: a slicing tree. A genome is a room order plus one "orientation" bit per
//! split. Each split divides the rectangle in proportion to the target areas of the
//! two halves, so every room gets exactly its target area (to 10 mm rounding), and the
//! rooms tile the rectangle with no gaps or overlaps. Simulated annealing searches
//! orders and bits for the lowest cost (aspect ratio, minimum width, adjacency,
//! daylight, entry position). Several seeded runs give distinct alternatives.
//!
//! Scope of this first version: one level, rectangular buildable envelope, no walls
//! or doors yet (those are derived from the rectangles in a later step). Any area
//! left after the requested rooms becomes an explicit `reserve` room (circulation
//! and flexible space), so nothing is stretched silently.

use crate::command::Command;
use crate::model::{Id, Source};
use crate::typed::{Space, element_of};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Public,
    Private,
    Service,
}

#[derive(Debug, Clone)]
pub struct RoomReq {
    pub key: String,
    pub func: String,
    pub target_area_m2: f64,
    pub min_width_mm: i64,
    pub zone: Zone,
    pub needs_light: bool,
}

/// Desired relation between two rooms: +2 must touch ... -2 must not touch.
#[derive(Debug, Clone)]
pub struct Adjacency {
    pub a: String,
    pub b: String,
    pub weight: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

impl Rect {
    pub fn area_m2(&self) -> f64 {
        self.w as f64 * self.h as f64 / 1e6
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    South,
    North,
    East,
    West,
}

#[derive(Debug, Clone)]
pub struct Brief {
    /// Buildable envelope (site minus setbacks), millimetres.
    pub envelope: Rect,
    pub rooms: Vec<RoomReq>,
    pub adjacency: Vec<Adjacency>,
    /// Side of the envelope facing the street entrance.
    pub entry: Side,
    /// Where the user marked the front door (mm, envelope frame), if they did: the entry door goes on the outside wall nearest to
    /// it. `None` keeps the street-side rule.
    pub entry_at: Option<(i64, i64)>,
    /// Wall thickness the drawings use, `(exterior, interior)` in mm. `None` keeps the engine's defaults (300 / 150).
    pub wall_mm: Option<(i64, i64)>,
    /// Wall height = storey height the drawings use (mm). `None` keeps the engine's default (3000), which the sheets may not share.
    pub wall_height_mm: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Weights {
    pub aspect: f64,
    pub min_width: f64,
    pub adjacency: f64,
    pub daylight: f64,
    pub entry: f64,
    /// Penalty when a private room does not touch a circulation room over a door-sized stretch.
    pub access: f64,
    /// Penalty when a bathroom is not attached to a distinct bedroom (when bedrooms are available).
    pub bath: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Self { aspect: 2.0, min_width: 3.0, adjacency: 1.0, daylight: 1.5, entry: 1.0, access: 3.0, bath: 2.0 }
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    pub seed: u64,
    pub restarts: usize,
    pub iterations: usize,
    /// Maximum number of distinct alternatives returned.
    pub count: usize,
    pub weights: Weights,
    /// Room functions that act as circulation hubs for access (the generated `reserve`
    /// room always does). Private rooms should open onto one of these, not onto each other.
    pub hub_funcs: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            seed: 1,
            restarts: 20,
            iterations: 12_000,
            count: 5,
            weights: Weights::default(),
            hub_funcs: ["hall", "corridor", "lobby", "living"].map(String::from).to_vec(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Breakdown {
    pub aspect: f64,
    pub min_width: f64,
    pub adjacency: f64,
    pub daylight: f64,
    pub entry: f64,
    pub access: f64,
    pub bath: f64,
}

impl Breakdown {
    fn total(&self) -> f64 {
        self.aspect + self.min_width + self.adjacency + self.daylight + self.entry + self.access + self.bath
    }
}

#[derive(Debug, Clone)]
pub struct PlacedRoom {
    pub key: String,
    pub func: String,
    pub zone: Zone,
    pub needs_light: bool,
    pub rect: Rect,
    pub area_m2: f64,
    pub target_m2: f64,
    pub deviation_pct: f64,
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub rooms: Vec<PlacedRoom>,
    pub cost: f64,
    pub breakdown: Breakdown,
    /// Area given to the `reserve` room (0 if the program filled the envelope).
    pub reserve_m2: f64,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum LayoutError {
    #[error("no rooms in the program")]
    Empty,
    #[error("envelope has no area")]
    BadEnvelope,
    #[error("duplicate room key '{0}'")]
    DuplicateKey(String),
    #[error("room '{0}' has a non-positive target area")]
    BadArea(String),
    #[error("adjacency refers to unknown room '{0}'")]
    UnknownRoom(String),
    #[error("program needs {required_m2:.1} m2 but the envelope has {available_m2:.1} m2")]
    Infeasible { required_m2: f64, available_m2: f64 },
}

// ---------- internals ----------

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

struct Problem<'a> {
    brief: &'a Brief,
    /// Areas used for splitting (rooms, then optional reserve), mm2.
    areas: Vec<f64>,
    has_reserve: bool,
    pairs: Vec<(usize, usize, i32)>,
    /// Indices of circulation rooms (including the reserve, which is last).
    hubs: Vec<usize>,
}

#[derive(Clone)]
struct Genome {
    order: Vec<usize>,
    bits: Vec<bool>,
}

const GRID: i64 = 10;
const RESERVE_MIN_FRACTION: f64 = 0.005;
/// A private room must share at least this much wall with a hub to count as having access.
const ACCESS_MIN_MM: i64 = 1200;
/// A circulation room needs this much street frontage to take the entrance.
const ENTRY_MIN_MM: i64 = 1600;
/// A bathroom needs this much shared wall with a bedroom to open into it (door plus margins).
const BATH_MIN_MM: i64 = 1100;
const WET_FUNCS: [&str; 2] = ["bathroom", "toilet"];

fn prepare<'a>(brief: &'a Brief, hub_funcs: &[String]) -> Result<Problem<'a>, LayoutError> {
    if brief.rooms.is_empty() {
        return Err(LayoutError::Empty);
    }
    let e = brief.envelope;
    if e.w <= 0 || e.h <= 0 {
        return Err(LayoutError::BadEnvelope);
    }
    let mut seen = std::collections::BTreeSet::new();
    for r in &brief.rooms {
        if !seen.insert(r.key.as_str()) {
            return Err(LayoutError::DuplicateKey(r.key.clone()));
        }
        if r.target_area_m2 <= 0.0 {
            return Err(LayoutError::BadArea(r.key.clone()));
        }
    }
    let index = |k: &str| brief.rooms.iter().position(|r| r.key == k).ok_or_else(|| LayoutError::UnknownRoom(k.to_string()));
    let mut pairs = Vec::new();
    for a in &brief.adjacency {
        pairs.push((index(&a.a)?, index(&a.b)?, a.weight));
    }

    let env_m2 = e.area_m2();
    let required: f64 = brief.rooms.iter().map(|r| r.target_area_m2).sum();
    if required > env_m2 + 1e-9 {
        return Err(LayoutError::Infeasible { required_m2: required, available_m2: env_m2 });
    }
    let leftover = env_m2 - required;
    let has_reserve = leftover >= env_m2 * RESERVE_MIN_FRACTION;
    let mut areas: Vec<f64> = brief.rooms.iter().map(|r| r.target_area_m2 * 1e6).collect();
    if has_reserve {
        areas.push(leftover * 1e6);
    } else {
        // Tiny remainder: spread it proportionally so the rooms still tile the envelope.
        let scale = env_m2 / required;
        areas.iter_mut().for_each(|a| *a *= scale);
    }
    let mut hubs: Vec<usize> =
        brief.rooms.iter().enumerate().filter(|(_, r)| hub_funcs.contains(&r.func)).map(|(i, _)| i).collect();
    if has_reserve {
        hubs.push(brief.rooms.len());
    }
    Ok(Problem { brief, areas, has_reserve, pairs, hubs })
}

fn round_grid(v: f64) -> i64 {
    ((v / GRID as f64).round() as i64) * GRID
}

fn place(order: &[usize], rect: Rect, bits: &[bool], bit: &mut usize, areas: &[f64], out: &mut [Rect]) {
    if order.len() == 1 {
        out[order[0]] = rect;
        return;
    }
    let total: f64 = order.iter().map(|&i| areas[i]).sum();
    let mut cum = 0.0;
    let mut best = (1, f64::INFINITY);
    for k in 1..order.len() {
        cum += areas[order[k - 1]];
        let d = (cum - total / 2.0).abs();
        if d < best.1 {
            best = (k, d);
        }
    }
    let k = best.0;
    let left_area: f64 = order[..k].iter().map(|&i| areas[i]).sum();
    let frac = left_area / total;
    // Bit false: cut across the longer side (keeps rooms compact); true: across the shorter.
    let flip = bits[*bit % bits.len()];
    *bit += 1;
    let cut_width = (rect.w >= rect.h) != flip;
    let (a, b) = if cut_width {
        let s = round_grid(rect.w as f64 * frac).clamp(GRID, rect.w - GRID);
        (Rect { w: s, ..rect }, Rect { x: rect.x + s, w: rect.w - s, ..rect })
    } else {
        let s = round_grid(rect.h as f64 * frac).clamp(GRID, rect.h - GRID);
        (Rect { h: s, ..rect }, Rect { y: rect.y + s, h: rect.h - s, ..rect })
    };
    place(&order[..k], a, bits, bit, areas, out);
    place(&order[k..], b, bits, bit, areas, out);
}

fn realise(p: &Problem, g: &Genome) -> Vec<Rect> {
    let mut out = vec![Rect { x: 0, y: 0, w: 0, h: 0 }; p.areas.len()];
    let mut bit = 0;
    place(&g.order, p.brief.envelope, &g.bits, &mut bit, &p.areas, &mut out);
    out
}

fn shared_len(a: &Rect, b: &Rect) -> i64 {
    let overlap = |a0: i64, a1: i64, b0: i64, b1: i64| (a1.min(b1) - a0.max(b0)).max(0);
    let mut s = 0;
    if a.x + a.w == b.x || b.x + b.w == a.x {
        s += overlap(a.y, a.y + a.h, b.y, b.y + b.h);
    }
    if a.y + a.h == b.y || b.y + b.h == a.y {
        s += overlap(a.x, a.x + a.w, b.x, b.x + b.w);
    }
    s
}

fn touches_boundary(r: &Rect, e: &Rect) -> bool {
    r.x == e.x || r.y == e.y || r.x + r.w == e.x + e.w || r.y + r.h == e.y + e.h
}

/// Normalised distance (0 at the entry side, 1 at the far side) of a rectangle's centre.
fn entry_distance(r: &Rect, e: &Rect, side: Side) -> f64 {
    let cx = (r.x + r.w / 2 - e.x) as f64 / e.w as f64;
    let cy = (r.y + r.h / 2 - e.y) as f64 / e.h as f64;
    match side {
        Side::South => cy,
        Side::North => 1.0 - cy,
        Side::West => cx,
        Side::East => 1.0 - cx,
    }
}

fn evaluate(p: &Problem, rects: &[Rect], w: &Weights) -> Breakdown {
    let b = p.brief;
    let mut bd = Breakdown::default();
    for (i, room) in b.rooms.iter().enumerate() {
        let r = &rects[i];
        let (long, short) = (r.w.max(r.h) as f64, r.w.min(r.h) as f64);
        bd.aspect += (long / short - 2.0).max(0.0) * w.aspect;
        bd.min_width += ((room.min_width_mm as f64 - short).max(0.0) / 1000.0) * w.min_width;
        if room.needs_light && !touches_boundary(r, &b.envelope) {
            bd.daylight += w.daylight;
        }
        let d = entry_distance(r, &b.envelope, b.entry);
        bd.entry += w.entry
            * match room.zone {
                Zone::Public => d,
                Zone::Private => (1.0 - d) * 0.5,
                Zone::Service => 0.0,
            };
    }
    if !p.hubs.is_empty() {
        for (i, room) in b.rooms.iter().enumerate() {
            if room.zone != Zone::Private || WET_FUNCS.contains(&room.func.as_str()) {
                continue;
            }
            let best = p.hubs.iter().filter(|&&h| h != i).map(|&h| shared_len(&rects[i], &rects[h])).max().unwrap_or(0);
            bd.access += w.access * (1.0 - best.min(ACCESS_MIN_MM) as f64 / ACCESS_MIN_MM as f64);
        }
    }
    // The entrance must open onto circulation: some hub room needs a door-sized stretch of the street side.
    if !p.hubs.is_empty() {
        let e = &b.envelope;
        let street = |r: &Rect| match b.entry {
            Side::South => if r.y == e.y { r.w } else { 0 },
            Side::North => if r.y + r.h == e.y + e.h { r.w } else { 0 },
            Side::West => if r.x == e.x { r.h } else { 0 },
            Side::East => if r.x + r.w == e.x + e.w { r.h } else { 0 },
        };
        let best = p.hubs.iter().map(|&h| street(&rects[h])).max().unwrap_or(0);
        bd.entry += w.access * (1.0 - best.min(ENTRY_MIN_MM) as f64 / ENTRY_MIN_MM as f64);
    }
    // Circulation rooms must form one connected spine (living -> hall -> bedrooms), so private
    // rooms are not reached through guest or service rooms.
    if p.hubs.len() > 1 {
        let mut parent: Vec<usize> = (0..p.hubs.len()).collect();
        fn root(parent: &mut [usize], mut x: usize) -> usize {
            while parent[x] != x {
                parent[x] = parent[parent[x]];
                x = parent[x];
            }
            x
        }
        for a in 0..p.hubs.len() {
            for c in a + 1..p.hubs.len() {
                if shared_len(&rects[p.hubs[a]], &rects[p.hubs[c]]) >= ACCESS_MIN_MM {
                    let (ra, rc) = (root(&mut parent, a), root(&mut parent, c));
                    parent[ra] = rc;
                }
            }
        }
        let components = (0..p.hubs.len()).filter(|&x| root(&mut parent, x) == x).count();
        bd.access += w.access * (components - 1) as f64;
    }
    bd.bath = bath_penalty(p, rects, w);
    for &(i, j, weight) in &p.pairs {
        let touch = (shared_len(&rects[i], &rects[j]).min(1000)) as f64 / 1000.0;
        bd.adjacency += w.adjacency
            * if weight > 0 { weight as f64 * (1.0 - touch) } else { weight.unsigned_abs() as f64 * touch };
    }
    bd
}

/// Bathrooms should each open into a *different* bedroom (attached baths spread across
/// bedrooms, found by maximum bipartite matching); a bath with no bedroom to attach to
/// should at least touch a circulation room (a shared bath).
fn bath_penalty(p: &Problem, rects: &[Rect], w: &Weights) -> f64 {
    let rooms = &p.brief.rooms;
    let baths: Vec<usize> = (0..rooms.len()).filter(|&i| WET_FUNCS.contains(&rooms[i].func.as_str())).collect();
    let hosts: Vec<usize> =
        (0..rooms.len()).filter(|&i| rooms[i].zone == Zone::Private && !WET_FUNCS.contains(&rooms[i].func.as_str())).collect();
    if baths.is_empty() || hosts.is_empty() {
        return 0.0;
    }
    let adj: Vec<Vec<usize>> = baths
        .iter()
        .map(|&b| (0..hosts.len()).filter(|&h| shared_len(&rects[b], &rects[hosts[h]]) >= BATH_MIN_MM).collect())
        .collect();

    fn augment(b: usize, adj: &[Vec<usize>], seen: &mut [bool], owner: &mut [Option<usize>]) -> bool {
        for &h in &adj[b] {
            if seen[h] {
                continue;
            }
            seen[h] = true;
            if owner[h].is_none_or(|o| augment(o, adj, seen, owner)) {
                owner[h] = Some(b);
                return true;
            }
        }
        false
    }
    let mut owner = vec![None; hosts.len()];
    let mut matched = 0;
    let mut is_matched = vec![false; baths.len()];
    for b in 0..baths.len() {
        let mut seen = vec![false; hosts.len()];
        if augment(b, &adj, &mut seen, &mut owner) {
            matched += 1;
        }
    }
    for o in owner.iter().flatten() {
        is_matched[*o] = true;
    }
    let mut pen = w.bath * (baths.len().min(hosts.len()) - matched) as f64;
    for (k, &b) in baths.iter().enumerate() {
        let hub_touch = p.hubs.iter().any(|&h| shared_len(&rects[b], &rects[h]) >= BATH_MIN_MM);
        if !is_matched[k] && adj[k].is_empty() && !hub_touch {
            pen += 0.5 * w.bath;
        }
    }
    pen
}

fn anneal(p: &Problem, rng: &mut Rng, iterations: usize, w: &Weights) -> (Genome, Breakdown) {
    let n = p.areas.len();
    let mut order: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        order.swap(i, rng.below(i + 1));
    }
    let bits: Vec<bool> = (0..n).map(|_| rng.below(4) == 0).collect();
    let mut cur = Genome { order, bits };
    let mut cur_bd = evaluate(p, &realise(p, &cur), w);
    let mut best = (cur.clone(), cur_bd.clone());

    for it in 0..iterations {
        let t = 2.0 * (0.005f64 / 2.0).powf(it as f64 / iterations as f64);
        let mut cand = cur.clone();
        match rng.below(3) {
            0 if n > 1 => {
                let (a, b) = (rng.below(n), rng.below(n));
                cand.order.swap(a, b);
            }
            1 if n > 1 => {
                let from = rng.below(n);
                let v = cand.order.remove(from);
                cand.order.insert(rng.below(n), v);
            }
            _ => {
                let k = rng.below(cand.bits.len());
                cand.bits[k] = !cand.bits[k];
            }
        }
        let bd = evaluate(p, &realise(p, &cand), w);
        let delta = bd.total() - cur_bd.total();
        if delta <= 0.0 || rng.unit() < (-delta / t).exp() {
            cur = cand;
            cur_bd = bd;
            if cur_bd.total() < best.1.total() {
                best = (cur.clone(), cur_bd.clone());
            }
        }
    }
    best
}

fn to_layout(p: &Problem, g: &Genome, bd: Breakdown) -> Layout {
    let rects = realise(p, g);
    let mut rooms: Vec<PlacedRoom> = p
        .brief
        .rooms
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let area = rects[i].area_m2();
            PlacedRoom {
                key: r.key.clone(),
                func: r.func.clone(),
                zone: r.zone,
                needs_light: r.needs_light,
                rect: rects[i],
                area_m2: area,
                target_m2: r.target_area_m2,
                deviation_pct: (area / r.target_area_m2 - 1.0) * 100.0,
            }
        })
        .collect();
    let mut reserve_m2 = 0.0;
    if p.has_reserve {
        let r = rects[p.brief.rooms.len()];
        reserve_m2 = r.area_m2();
        rooms.push(PlacedRoom {
            key: "reserve".into(),
            func: "circulation_reserve".into(),
            zone: Zone::Service,
            needs_light: false,
            rect: r,
            area_m2: reserve_m2,
            target_m2: reserve_m2,
            deviation_pct: 0.0,
        });
    }
    Layout { rooms, cost: bd.total(), breakdown: bd, reserve_m2 }
}

/// Coarse fingerprint (3x3 cell of each room's centre) used to drop near-duplicate alternatives.
fn signature(l: &Layout, e: &Rect) -> String {
    l.rooms
        .iter()
        .map(|r| {
            let cx = ((r.rect.x + r.rect.w / 2 - e.x) * 3 / e.w).clamp(0, 2);
            let cy = ((r.rect.y + r.rect.h / 2 - e.y) * 3 / e.h).clamp(0, 2);
            format!("{}{}", cx, cy)
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Generates up to `opts.count` distinct layouts, best first. Deterministic for a given seed.
pub fn generate(brief: &Brief, opts: &Options) -> Result<Vec<Layout>, LayoutError> {
    let p = prepare(brief, &opts.hub_funcs)?;
    let mut found: Vec<Layout> = Vec::new();
    for r in 0..opts.restarts.max(1) {
        let mut rng = Rng(opts.seed.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(r as u64 + 1) | 1);
        let (g, bd) = anneal(&p, &mut rng, opts.iterations, &opts.weights);
        found.push(to_layout(&p, &g, bd));
    }
    found.sort_by(|a, b| a.cost.total_cmp(&b.cost));
    let mut seen = std::collections::BTreeSet::new();
    found.retain(|l| seen.insert(signature(l, &brief.envelope)));
    found.truncate(opts.count.max(1));
    Ok(found)
}

/// Turns a chosen layout into `Space` creation commands (one undoable generator transaction).
pub fn to_commands(layout: &Layout, level: Option<Id>) -> Vec<Command> {
    layout
        .rooms
        .iter()
        .map(|r| {
            let b = r.rect;
            let space = Space {
                func: r.func.clone(),
                requirement_key: r.key.clone(),
                area: (r.area_m2 * 100.0).round() / 100.0,
                target_area: r.target_m2,
                x: b.x,
                y: b.y,
                w: b.w,
                h: b.h,
                boundary: vec![[b.x, b.y], [b.x + b.w, b.y], [b.x + b.w, b.y + b.h], [b.x, b.y + b.h]],
                net_area: None,
                glazing_ratio: None,
                extra: Default::default(),
            };
            let mut el = element_of(&space, Source::Generated);
            el.level_id = level;
            Command::Create(el)
        })
        .collect()
}
