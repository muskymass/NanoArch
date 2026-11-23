//! Walls and doors derived from a room layout.
//!
//! Every room edge is cut at all T-junctions so each wall piece borders one room
//! (exterior wall) or two rooms (partition). Consecutive pieces with the same
//! neighbours merge into one wall. Doors then connect rooms: a spanning tree over
//! the non-wet rooms (preferring the circulation reserve as hub), each wet room
//! attached to one neighbour, extra doors for wanted adjacencies, and one entry
//! door on the street side. A BFS from the entry reports unreachable rooms.
//!
//! All dimensions in `WallOpts` are PLACEHOLDER design defaults, not code values.
//! Door widths and wall thicknesses are checked by the rule engine against the
//! official SBC values once those are entered in a rule pack.
//!
//! Known limits: no corner/T-junction trimming (wall pieces overlap by up to one
//! thickness at corners, so wall quantities are slightly over-counted), no windows.

use crate::command::Command;
use crate::layout::{Adjacency, Layout, Rect, Side, Zone};
use crate::model::{Id, Source};
use crate::typed::{Door, Wall, Window, element_of};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct WallOpts {
    pub exterior_mm: i64,
    pub interior_mm: i64,
    pub height_mm: i64,
    pub entry_door_mm: i64,
    pub interior_door_mm: i64,
    pub wet_door_mm: i64,
    pub door_height_mm: i64,
    /// Minimum solid wall on each side of a door.
    pub door_margin_mm: i64,
    /// `func` values treated as wet rooms: one door only, never a transit room.
    pub wet_funcs: Vec<String>,
    /// `func` values that are shafts: exactly one door, onto a neighbour that is not itself a shaft or a wet room, never a transit room
    /// (a lift shaft is entered from a landing and nobody passes through it).
    pub dead_end_funcs: Vec<String>,
    /// `func` values open to the sky (a courtyard, a light well): the room has no walls of its own, the walls of the rooms around it
    /// face it as outside walls (so they take windows), and nobody enters it as a room.
    pub open_funcs: Vec<String>,
    /// Room functions the entry door should open into, most preferred first.
    /// Without a match the public room with the longest street wall is used.
    pub entry_funcs: Vec<String>,
    /// Circulation rooms. Doors into private rooms should come from these (or a bathroom).
    pub hub_funcs: Vec<String>,
    /// Pairs of room keys that should share a door when they share a wall.
    pub wanted: Vec<(String, String)>,
    /// Pairs of room keys that must never share a door.
    pub forbidden: Vec<(String, String)>,
    /// False for an upper level: no door to the street. Reachability is then checked from the first circulation room.
    pub entry: bool,
    /// Where the user marked the front door, in the envelope's own frame (mm). The entry door goes on the outside wall nearest to
    /// it instead of on the street side. Nothing is invented: a point farther than `ENTRY_POINT_REACH_MM` from every outside wall is
    /// reported and the street-side rule is used.
    pub entry_at: Option<(i64, i64)>,
}

/// Farthest the marked entry point may lie from an outside wall and still be taken to mean that wall.
pub const ENTRY_POINT_REACH_MM: i64 = 1500;

impl Default for WallOpts {
    fn default() -> Self {
        Self {
            exterior_mm: 300,
            interior_mm: 150,
            height_mm: 3000,
            entry_door_mm: 1000,
            interior_door_mm: 900,
            wet_door_mm: 800,
            door_height_mm: 2100,
            door_margin_mm: 150,
            wet_funcs: vec!["bathroom".into(), "toilet".into()],
            dead_end_funcs: vec!["lift".into()],
            open_funcs: vec!["courtyard".into()],
            entry_funcs: ["circulation_reserve", "hall", "living", "reception", "majlis"].map(String::from).to_vec(),
            hub_funcs: ["circulation_reserve", "hall", "corridor", "lobby", "living"].map(String::from).to_vec(),
            wanted: vec![],
            forbidden: vec![],
            entry: true,
            entry_at: None,
        }
    }
}

/// Wanted doors from positive adjacency (weight >= 2), forbidden from negative weights.
pub fn opts_from_adjacency(adj: &[Adjacency]) -> WallOpts {
    let mut o = WallOpts::default();
    for a in adj {
        if a.weight >= 2 {
            o.wanted.push((a.a.clone(), a.b.clone()));
        } else if a.weight < 0 {
            o.forbidden.push((a.a.clone(), a.b.clone()));
        }
    }
    o
}

#[derive(Debug, Clone)]
pub struct WallSpec {
    pub id: Id,
    pub a: (i64, i64),
    pub b: (i64, i64),
    pub thickness: i64,
    pub exterior: bool,
    pub horizontal: bool,
    /// Coordinate of the room edge this wall sits on (before exterior inset).
    pub edge_line: i64,
    pub start: i64,
    pub end: i64,
    /// Room indices into `Layout::rooms`.
    pub rooms: (usize, Option<usize>),
    /// True when the wall faces an open-to-sky room (a courtyard) instead of the street.
    pub court: bool,
}

impl WallSpec {
    pub fn length(&self) -> i64 {
        self.end - self.start
    }
}

#[derive(Debug, Clone)]
pub struct DoorSpec {
    /// Index into `Plan::walls`.
    pub wall: usize,
    /// Distance from the wall start to the door centre.
    pub offset: i64,
    pub width: i64,
    pub height: i64,
    pub entry: bool,
    pub connects: (usize, Option<usize>),
    pub opens_into: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub walls: Vec<WallSpec>,
    pub doors: Vec<DoorSpec>,
    pub room_keys: Vec<String>,
    pub wall_height_mm: i64,
    /// Net floor area per room (after wall thickness), m2.
    pub net_area_m2: Vec<f64>,
    /// Problems the user must resolve (unreachable room, no entry door...).
    pub warnings: Vec<String>,
}

/// A wall piece between two cut points and the `(room, side)` pairs covering it.
type Piece = (i64, i64, Vec<(usize, usize)>);

#[derive(Clone, Copy)]
struct Edge {
    a: i64,
    b: i64,
    room: usize,
    side: usize, // 0 bottom, 1 top, 2 left, 3 right
}

fn key_pair(a: &str, b: &str) -> (String, String) {
    if a <= b { (a.to_string(), b.to_string()) } else { (b.to_string(), a.to_string()) }
}

struct Uf(Vec<usize>);
impl Uf {
    fn find(&mut self, x: usize) -> usize {
        if self.0[x] != x {
            let r = self.find(self.0[x]);
            self.0[x] = r;
        }
        self.0[x]
    }
    fn union(&mut self, a: usize, b: usize) -> bool {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return false;
        }
        self.0[ra] = rb;
        true
    }
}

pub fn build(layout: &Layout, envelope: Rect, entry: Side, opts: &WallOpts) -> Plan {
    let rooms = &layout.rooms;
    let n = rooms.len();
    let mut plan = Plan {
        room_keys: rooms.iter().map(|r| r.key.clone()).collect(),
        wall_height_mm: opts.height_mm,
        ..Default::default()
    };

    // ---- 1. cut room edges into wall pieces ----
    let mut groups: BTreeMap<(bool, i64), Vec<Edge>> = BTreeMap::new();
    let is_open = |i: usize| opts.open_funcs.contains(&rooms[i].func);
    // Edges of the open rooms, to tell which outside walls face one.
    let court_edges: Vec<(bool, i64, i64, i64)> = rooms
        .iter()
        .enumerate()
        .filter(|(i, _)| is_open(*i))
        .flat_map(|(_, r)| {
            let b = r.rect;
            [(true, b.y, b.x, b.x + b.w), (true, b.y + b.h, b.x, b.x + b.w), (false, b.x, b.y, b.y + b.h), (false, b.x + b.w, b.y, b.y + b.h)]
        })
        .collect();
    for (i, r) in rooms.iter().enumerate().filter(|(i, _)| !is_open(*i)) {
        let b = r.rect;
        let mut add = |horiz: bool, line: i64, a: i64, bb: i64, side: usize| {
            groups.entry((horiz, line)).or_default().push(Edge { a, b: bb, room: i, side });
        };
        add(true, b.y, b.x, b.x + b.w, 0);
        add(true, b.y + b.h, b.x, b.x + b.w, 1);
        add(false, b.x, b.y, b.y + b.h, 2);
        add(false, b.x + b.w, b.y, b.y + b.h, 3);
    }

    // inset_sum[room][side] = sum of piece length * inset
    let mut inset_sum = vec![[0f64; 4]; n];
    for (&(horiz, line), edges) in &groups {
        let mut pts: Vec<i64> = edges.iter().flat_map(|e| [e.a, e.b]).collect();
        pts.sort_unstable();
        pts.dedup();
        // Elementary pieces: (start, end, covering (room, side))
        let mut pieces: Vec<Piece> = Vec::new();
        for w in pts.windows(2) {
            let cover: Vec<(usize, usize)> =
                edges.iter().filter(|e| e.a <= w[0] && e.b >= w[1]).map(|e| (e.room, e.side)).collect();
            if cover.is_empty() {
                continue;
            }
            let same = |c: &[(usize, usize)], d: &[(usize, usize)]| {
                c.iter().map(|x| x.0).collect::<Vec<_>>() == d.iter().map(|x| x.0).collect::<Vec<_>>()
            };
            match pieces.last_mut() {
                Some(last) if last.1 == w[0] && same(&last.2, &cover) => last.1 = w[1],
                _ => pieces.push((w[0], w[1], cover)),
            }
        }
        for (start, end, cover) in pieces {
            let exterior = cover.len() == 1;
            let t = if exterior { opts.exterior_mm } else { opts.interior_mm };
            let inset = if exterior { t as f64 } else { t as f64 / 2.0 };
            for &(room, side) in &cover {
                inset_sum[room][side] += (end - start) as f64 * inset;
            }
            // Exterior walls sit fully inside the envelope; partitions are centred on the edge.
            let court = exterior && court_edges.iter().any(|&(h, l, a, b)| h == horiz && l == line && start < b && end > a);
            let cl = if !exterior {
                line
            } else if court {
                // A wall facing a courtyard sits inside its room, like an outside wall at the envelope.
                if cover[0].1 % 2 == 0 { line + t / 2 } else { line - t / 2 }
            } else {
                let (lo, hi) = if horiz { (envelope.y, envelope.y + envelope.h) } else { (envelope.x, envelope.x + envelope.w) };
                if line == lo {
                    line + t / 2
                } else if line == hi {
                    line - t / 2
                } else {
                    line
                }
            };
            let (a, b) = if horiz { ((start, cl), (end, cl)) } else { ((cl, start), (cl, end)) };
            let mut rs = cover.iter().map(|c| c.0);
            let first = rs.next().expect("non-empty");
            plan.walls.push(WallSpec {
                id: Uuid::now_v7(),
                a,
                b,
                thickness: t,
                exterior,
                horizontal: horiz,
                edge_line: line,
                start,
                end,
                rooms: (first, rs.next()),
                court,
            });
        }
    }

    // ---- 2. net areas ----
    for (i, r) in rooms.iter().enumerate() {
        let (w, h) = (r.rect.w as f64, r.rect.h as f64);
        let s = &inset_sum[i];
        let bottom = s[0] / w;
        let top = s[1] / w;
        let left = s[2] / h;
        let right = s[3] / h;
        plan.net_area_m2.push(((w - left - right) * (h - bottom - top)).max(0.0) / 1e6);
    }

    // ---- 3. doors ----
    let keys = plan.room_keys.clone();
    let idx = |k: &str| keys.iter().position(|x| x == k);
    let is_wet = |i: usize| opts.wet_funcs.contains(&rooms[i].func);
    let is_hub = |i: usize| opts.hub_funcs.contains(&rooms[i].func);
    let is_dead = |i: usize| opts.dead_end_funcs.contains(&rooms[i].func);
    let forbidden: BTreeSet<(String, String)> = opts.forbidden.iter().map(|(a, b)| key_pair(a, b)).collect();
    let is_forbidden = |i: usize, j: usize| forbidden.contains(&key_pair(&rooms[i].key, &rooms[j].key));

    let mut wall_of: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    for (wi, w) in plan.walls.iter().enumerate() {
        if let (a, Some(b)) = w.rooms {
            let key = (a.min(b), a.max(b));
            // Two rooms can share more than one wall piece: keep the longest, where a door fits best.
            if wall_of.get(&key).is_none_or(|&old| plan.walls[old].length() < w.length()) {
                wall_of.insert(key, wi);
            }
        }
    }
    let feasible = |plan: &Plan, i: usize, j: usize, width: i64| -> Option<usize> {
        let wi = *wall_of.get(&(i.min(j), i.max(j)))?;
        (plan.walls[wi].length() >= width + 2 * opts.door_margin_mm && !is_forbidden(i, j)).then_some(wi)
    };

    let mut doored: BTreeSet<(usize, usize)> = BTreeSet::new();
    let add_door = |plan: &mut Plan, doored: &mut BTreeSet<(usize, usize)>, i: usize, j: usize, wi: usize, width: i64| {
        let len = plan.walls[wi].length();
        let opens_into = if is_wet(i) {
            i
        } else if is_wet(j) {
            j
        } else if rooms[i].area_m2 <= rooms[j].area_m2 {
            i
        } else {
            j
        };
        plan.doors.push(DoorSpec {
            wall: wi,
            offset: len / 2,
            width,
            height: opts.door_height_mm,
            entry: false,
            connects: (i, Some(j)),
            opens_into,
        });
        doored.insert((i.min(j), i.max(j)));
    };

    // 3a. spanning tree over non-wet rooms; the reserve (circulation) is the preferred hub.
    let mut edges: Vec<(i64, i64, usize, usize, usize)> = Vec::new(); // cost, -len, i, j, wall
    for (&(i, j), &wi) in &wall_of {
        if is_wet(i) || is_wet(j) || is_dead(i) || is_dead(j) {
            continue;
        }
        if feasible(&plan, i, j, opts.interior_door_mm).is_none() {
            continue;
        }
        let (hi, hj) = (is_hub(i), is_hub(j));
        let reserve = rooms[i].key.starts_with("reserve") || rooms[j].key.starts_with("reserve");
        let private = rooms[i].zone == Zone::Private || rooms[j].zone == Zone::Private;
        let cost = if hi && hj {
            10
        } else if hi || hj {
            if reserve { 20 } else { 40 }
        } else if private {
            // Never pass through a bedroom to reach another room.
            400
        } else {
            100
        };
        edges.push((cost, -plan.walls[wi].length(), i, j, wi));
    }
    edges.sort();
    let mut uf = Uf((0..n).collect());
    for (_, _, i, j, wi) in edges {
        if uf.union(i, j) {
            add_door(&mut plan, &mut doored, i, j, wi, opts.interior_door_mm);
        }
    }

    // 3b. each wet room gets exactly one door.
    let mut wet_count = vec![0usize; n];
    for w in (0..n).filter(|&i| is_wet(i)) {
        let mut best: Option<(i32, i64, usize, usize)> = None; // score, len, neighbour, wall
        for j in (0..n).filter(|&j| j != w && !is_wet(j)) {
            let Some(wi) = feasible(&plan, w, j, opts.wet_door_mm) else { continue };
            let wanted = opts.wanted.iter().any(|(a, b)| key_pair(a, b) == key_pair(&rooms[w].key, &rooms[j].key));
            // Wanted pair first, then a bedroom without a bath, then a hub (shared bath), then a
            // bedroom that already has one; so baths spread across bedrooms.
            let score = if wanted {
                20
            } else if rooms[j].zone == Zone::Private {
                if wet_count[j] == 0 { 15 } else { 8 }
            } else if is_hub(j) {
                12
            } else {
                0
            };
            let cand = (score, plan.walls[wi].length(), j, wi);
            if best.is_none_or(|b| (cand.0, cand.1) > (b.0, b.1)) {
                best = Some(cand);
            }
        }
        match best {
            Some((_, _, j, wi)) => {
                wet_count[j] += 1;
                add_door(&mut plan, &mut doored, w, j, wi, opts.wet_door_mm)
            }
            None => plan.warnings.push(format!("wet room '{}' has no neighbour wide enough for a door", rooms[w].key)),
        }
    }

    // 3b'. each shaft gets exactly one door, onto a hub if it touches one, else onto the neighbour with the longest shared wall.
    // The door opens into the neighbour: a lift door is a landing door, not a swing door into the shaft.
    for s in (0..n).filter(|&i| is_dead(i)) {
        let mut best: Option<(i32, i64, usize, usize)> = None; // score, len, neighbour, wall
        for j in (0..n).filter(|&j| j != s && !is_wet(j) && !is_dead(j)) {
            let Some(wi) = feasible(&plan, s, j, opts.interior_door_mm) else { continue };
            let score = if is_hub(j) { 12 } else if rooms[j].zone != Zone::Private { 6 } else { 0 };
            let cand = (score, plan.walls[wi].length(), j, wi);
            if best.is_none_or(|b| (cand.0, cand.1) > (b.0, b.1)) {
                best = Some(cand);
            }
        }
        match best {
            Some((_, _, j, wi)) => {
                add_door(&mut plan, &mut doored, s, j, wi, opts.interior_door_mm);
                if let Some(d) = plan.doors.last_mut() {
                    d.opens_into = j;
                }
            }
            None => plan.warnings.push(format!("shaft '{}' has no neighbour wide enough for a door", rooms[s].key)),
        }
    }

    // 3c. extra doors for wanted adjacencies.
    for (a, b) in &opts.wanted {
        let (Some(i), Some(j)) = (idx(a), idx(b)) else { continue };
        if is_wet(i) || is_wet(j) || is_dead(i) || is_dead(j) || doored.contains(&(i.min(j), i.max(j))) {
            continue;
        }
        if let Some(wi) = feasible(&plan, i, j, opts.interior_door_mm) {
            add_door(&mut plan, &mut doored, i, j, wi, opts.interior_door_mm);
        }
    }

    // 3d. entry door on the street side, in the public room with the longest wall there.
    let on_entry_side = |w: &WallSpec| {
        w.exterior
            && match entry {
                Side::South => w.horizontal && w.edge_line == envelope.y,
                Side::North => w.horizontal && w.edge_line == envelope.y + envelope.h,
                Side::West => !w.horizontal && w.edge_line == envelope.x,
                Side::East => !w.horizontal && w.edge_line == envelope.x + envelope.w,
            }
    };
    let mut entry_pick: Option<(usize, u8, i64, usize)> = None; // preference, public?, len, wall
    for (wi, w) in plan.walls.iter().enumerate().filter(|_| opts.entry) {
        if w.court || !on_entry_side(w) || w.length() < opts.entry_door_mm + 2 * opts.door_margin_mm {
            continue;
        }
        let room = w.rooms.0;
        if is_wet(room) {
            continue;
        }
        let rank = opts.entry_funcs.iter().position(|f| *f == rooms[room].func).map_or(0, |p| opts.entry_funcs.len() - p);
        let cand = (rank, (rooms[room].zone == Zone::Public) as u8, w.length(), wi);
        if entry_pick.is_none_or(|b| (cand.0, cand.1, cand.2) > (b.0, b.1, b.2)) {
            entry_pick = Some(cand);
        }
    }
    // A marked entry point wins over the street-side rule: the outside wall nearest to it, the door centred on its projection.
    let mut marked_entry: Option<(usize, i64)> = None;
    if let (true, Some((px, py))) = (opts.entry, opts.entry_at) {
        let mut best: Option<(i64, usize, i64)> = None;
        for (wi, w) in plan.walls.iter().enumerate() {
            if !w.exterior || w.court || w.length() < opts.entry_door_mm + 2 * opts.door_margin_mm || is_wet(w.rooms.0) {
                continue;
            }
            let (along, across) = if w.horizontal { (px, py) } else { (py, px) };
            let beyond = (w.start - along).max(along - w.end).max(0);
            let d2 = beyond * beyond + (across - w.edge_line) * (across - w.edge_line);
            let lo = opts.door_margin_mm + opts.entry_door_mm / 2;
            let hi = (w.length() - lo).max(lo);
            if best.is_none_or(|b| d2 < b.0) {
                best = Some((d2, wi, (along - w.start).clamp(lo, hi)));
            }
        }
        match best {
            Some((d2, wi, off)) if d2 <= ENTRY_POINT_REACH_MM * ENTRY_POINT_REACH_MM => marked_entry = Some((wi, off)),
            _ => plan.warnings.push("the marked entry point is not within 1.5 m of an outside wall: the street-side rule was used".into()),
        }
    }
    let mut entry_room = None;
    let chosen: Option<(usize, i64)> = marked_entry.or(entry_pick.map(|(_, _, _, wi)| (wi, plan.walls[wi].length() / 2)));
    match chosen {
        Some((wi, off)) => {
            let room = plan.walls[wi].rooms.0;
            plan.doors.push(DoorSpec {
                wall: wi,
                offset: off,
                width: opts.entry_door_mm,
                height: opts.door_height_mm,
                entry: true,
                connects: (room, None),
                opens_into: room,
            });
            entry_room = Some(room);
        }
        None if opts.entry => plan.warnings.push("no wall on the entry side is long enough for an entry door".into()),
        None => entry_room = (0..n).find(|&i| is_hub(i)),
    }

    // ---- 4. reachability from the entry ----
    if let Some(start) = entry_room {
        let mut adj = vec![Vec::new(); n];
        for d in &plan.doors {
            if let (a, Some(b)) = d.connects {
                adj[a].push(b);
                adj[b].push(a);
            }
        }
        let mut seen = vec![false; n];
        seen[start] = true;
        let mut q = VecDeque::from([start]);
        while let Some(x) = q.pop_front() {
            for &y in &adj[x] {
                if !seen[y] {
                    seen[y] = true;
                    q.push_back(y);
                }
            }
        }
        for i in (0..n).filter(|&i| !seen[i] && !is_open(i)) {
            plan.warnings.push(format!("room '{}' cannot be reached from the entry", rooms[i].key));
        }
    }

    // ---- 5. privacy: a private room reached only through other private rooms ----
    for i in (0..n).filter(|&i| !is_wet(i) && rooms[i].zone == Zone::Private) {
        let neighbours: Vec<usize> = plan
            .doors
            .iter()
            .filter_map(|d| match d.connects {
                (a, Some(b)) if a == i => Some(b),
                (a, Some(b)) if b == i => Some(a),
                _ => None,
            })
            .filter(|&j| !is_wet(j))
            .collect();
        if !neighbours.is_empty() && neighbours.iter().all(|&j| rooms[j].zone == Zone::Private) {
            plan.warnings.push(format!(
                "room '{}' is reached only through another private room; change the layout or add a corridor",
                rooms[i].key
            ));
        }
    }
    // Non-private rooms (kitchen, dining...) must not be reached only through a bedroom.
    for i in (0..n).filter(|&i| !is_wet(i) && !is_dead(i) && !is_hub(i) && rooms[i].zone != Zone::Private) {
        let neighbours: Vec<usize> = plan
            .doors
            .iter()
            .filter_map(|d| match d.connects {
                (a, Some(b)) if a == i => Some(b),
                (a, Some(b)) if b == i => Some(a),
                _ => None,
            })
            .filter(|&j| !is_wet(j))
            .collect();
        if !neighbours.is_empty() && neighbours.iter().all(|&j| rooms[j].zone == Zone::Private) {
            plan.warnings.push(format!("room '{}' is reached only through a bedroom", rooms[i].key));
        }
    }
    plan
}

/// Maps requirement key -> element id from the `Space` creation commands of a layout.
pub fn space_ids(cmds: &[Command]) -> BTreeMap<String, Id> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::Create(el) if el.kind == "Space" => {
                let key = el.props.get("requirementKey")?.as_str()?.to_string();
                Some((key, el.id))
            }
            _ => None,
        })
        .collect()
}

/// Creation commands for walls, then doors (hosted by their wall), then `netArea` on each space.
/// Apply after (or in the same transaction as) the space commands.
pub fn to_commands(plan: &Plan, spaces: &BTreeMap<String, Id>, level: Option<Id>) -> Vec<Command> {
    let mut cmds = Vec::new();
    for w in &plan.walls {
        let wall = Wall {
            x1: w.a.0,
            y1: w.a.1,
            x2: w.b.0,
            y2: w.b.1,
            thickness: w.thickness,
            height: plan.wall_height_mm,
            length: w.length(),
            is_exterior: w.exterior,
            room_a: plan.room_keys[w.rooms.0].clone(),
            room_b: w.rooms.1.map(|b| plan.room_keys[b].clone()),
            extra: Default::default(),
        };
        let mut el = element_of(&wall, Source::Generated);
        el.id = w.id;
        el.level_id = level;
        cmds.push(Command::Create(el));
    }
    for d in &plan.doors {
        let wall = &plan.walls[d.wall];
        let to = d.connects.1.map_or_else(|| "outside".to_string(), |j| plan.room_keys[j].clone());
        let door = Door {
            width: d.width,
            height: d.height,
            offset_center: d.offset,
            is_egress: d.entry,
            from: plan.room_keys[d.connects.0].clone(),
            to,
            opens_into: plan.room_keys[d.opens_into].clone(),
            extra: Default::default(),
        };
        let mut el = element_of(&door, Source::Generated).hosted_by(wall.id);
        el.level_id = level;
        cmds.push(Command::Create(el));
    }
    for (i, key) in plan.room_keys.iter().enumerate() {
        if let Some(id) = spaces.get(key) {
            cmds.push(Command::SetProp {
                id: *id,
                key: "netArea".into(),
                value: Some(json!((plan.net_area_m2[i] * 100.0).round() / 100.0)),
            });
        }
    }
    cmds
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

/// PLACEHOLDER design defaults, not code values. `target_ratio` is where a code value enters: for each room function
/// listed there, the windows of a daylight room are enlarged until glazed area / floor area reaches the ratio (the
/// caller takes the ratio from the code table, see `sbc::daylight_targets`). Empty means no target, so the sizes
/// below are used as they are.
#[derive(Debug, Clone)]
pub struct WindowOpts {
    pub width_mm: i64,
    pub wet_width_mm: i64,
    pub height_mm: i64,
    pub sill_mm: i64,
    pub wet_sill_mm: i64,
    pub min_width_mm: i64,
    /// Solid wall kept between a window and a door, a corner or another window.
    pub margin_mm: i64,
    /// Room function -> minimum glazed area as a fraction of floor area.
    pub target_ratio: BTreeMap<String, f64>,
}

impl Default for WindowOpts {
    fn default() -> Self {
        Self { width_mm: 1500, wet_width_mm: 600, height_mm: 1200, sill_mm: 900, wet_sill_mm: 1500, min_width_mm: 600, margin_mm: 300, target_ratio: BTreeMap::new() }
    }
}

/// Solid wall kept above a window (lintel), when a window has to grow taller to reach a glazing target.
const LINTEL_MM: i64 = 300;

#[derive(Debug, Clone)]
pub struct WindowSpec {
    /// Index into `Plan::walls`.
    pub wall: usize,
    /// Distance from the wall start to the window centre.
    pub offset: i64,
    pub width: i64,
    pub height: i64,
    pub sill: i64,
    pub room: usize,
}

#[derive(Debug, Clone, Default)]
pub struct WindowPlan {
    pub windows: Vec<WindowSpec>,
    /// Glazed area divided by floor area, per room.
    pub glazing_ratio: Vec<f64>,
    pub warnings: Vec<String>,
}

/// The windows of one room on its exterior walls, in the free stretches left between doors and corners, each window
/// `target` wide at most and `height` tall.
fn windows_of_room(plan: &Plan, room: usize, sill: i64, target: i64, height: i64, opts: &WindowOpts) -> Vec<WindowSpec> {
    let mut out = Vec::new();
    for (wi, w) in plan.walls.iter().enumerate().filter(|(_, w)| w.exterior && w.rooms.0 == room) {
        let mut free = vec![(opts.margin_mm, w.length() - opts.margin_mm)];
        let subtract = |free: Vec<(i64, i64)>, lo: i64, hi: i64| -> Vec<(i64, i64)> {
            let mut out = Vec::new();
            for (a, b) in free {
                if b.min(lo) > a {
                    out.push((a, b.min(lo)));
                }
                if b > a.max(hi) {
                    out.push((a.max(hi), b));
                }
            }
            out
        };
        for d in plan.doors.iter().filter(|d| d.wall == wi) {
            free = subtract(free, d.offset - d.width / 2 - opts.margin_mm, d.offset + d.width / 2 + opts.margin_mm);
        }
        let count = ((w.length() + 3999) / 4000).clamp(1, 3);
        for _ in 0..count {
            let Some(&(a, b)) = free.iter().max_by_key(|(a, b)| b - a) else { break };
            let width = target.min(b - a);
            if width < opts.min_width_mm {
                break;
            }
            let centre = (a + b) / 2;
            out.push(WindowSpec { wall: wi, offset: centre, width, height, sill, room });
            free = subtract(free, centre - width / 2 - opts.margin_mm, centre + width / 2 + opts.margin_mm);
        }
    }
    out
}

/// Places windows on exterior walls of rooms that need daylight (and a small high window
/// in wet rooms), in the free stretches of wall left between doors and corners. A room whose function has a
/// `target_ratio` gets the smallest windows (by area) that reach it; if its walls are too short for that, the largest
/// that fit are used and the shortfall is reported.
pub fn place_windows(layout: &Layout, plan: &Plan, wall_opts: &WallOpts, opts: &WindowOpts) -> WindowPlan {
    let rooms = &layout.rooms;
    let mut wp = WindowPlan { glazing_ratio: vec![0.0; rooms.len()], ..Default::default() };
    let glazed = |ws: &[WindowSpec]| ws.iter().map(|w| (w.width * w.height) as f64 / 1e6).sum::<f64>();

    for (room, r) in rooms.iter().enumerate() {
        let wet = wall_opts.wet_funcs.contains(&r.func);
        if !(r.needs_light || wet) || !plan.walls.iter().any(|w| w.exterior && w.rooms.0 == room) {
            continue;
        }
        let (target, sill) = if wet { (opts.wet_width_mm, opts.wet_sill_mm) } else { (opts.width_mm, opts.sill_mm) };
        let mut chosen = windows_of_room(plan, room, sill, target, opts.height_mm, opts);
        if let Some(&ratio) = opts.target_ratio.get(&r.func).filter(|_| !wet) {
            // A hair above the target, so rounding to whole millimetres cannot leave it just short.
            let need = ratio * r.area_m2 * 1.002;
            if glazed(&chosen) < need {
                let max_h = (wall_opts.height_mm - sill - LINTEL_MM).max(opts.height_mm);
                let mut sizes: Vec<(i64, i64)> = (opts.height_mm..=max_h).step_by(100).flat_map(|h| (opts.width_mm..=4000).step_by(100).map(move |w| (w, h))).collect();
                sizes.sort_by_key(|(w, h)| (w * h, *h));
                let mut best: Option<Vec<WindowSpec>> = None;
                let mut largest: Vec<WindowSpec> = chosen.clone();
                for (w, h) in sizes {
                    let ws = windows_of_room(plan, room, sill, w, h, opts);
                    if glazed(&ws) >= need {
                        best = Some(ws);
                        break;
                    }
                    if glazed(&ws) > glazed(&largest) {
                        largest = ws;
                    }
                }
                chosen = best.unwrap_or_else(|| {
                    wp.warnings.push(format!("room '{}' cannot reach {:.0}% glazing: its exterior walls are too short for the windows it needs", r.key, ratio * 100.0));
                    largest
                });
            }
        }
        wp.glazing_ratio[room] = glazed(&chosen) / r.area_m2;
        wp.windows.extend(chosen);
    }
    for (i, r) in rooms.iter().enumerate() {
        if r.needs_light && wp.glazing_ratio[i] == 0.0 {
            wp.warnings.push(format!("room '{}' needs daylight but has no window (no exterior wall space)", r.key));
        }
    }
    wp
}
/// Window creation commands (hosted by their wall) and `glazingRatio` on each space.
/// Apply after the wall commands.
pub fn window_commands(plan: &Plan, wp: &WindowPlan, spaces: &BTreeMap<String, Id>, level: Option<Id>) -> Vec<Command> {
    let mut cmds = Vec::new();
    for w in &wp.windows {
        let window = Window {
            width: w.width,
            height: w.height,
            sill_height: w.sill,
            offset_center: w.offset,
            room: plan.room_keys[w.room].clone(),
            extra: Default::default(),
        };
        let mut el = element_of(&window, Source::Generated).hosted_by(plan.walls[w.wall].id);
        el.level_id = level;
        cmds.push(Command::Create(el));
    }
    for (i, key) in plan.room_keys.iter().enumerate() {
        if let Some(id) = spaces.get(key) {
            cmds.push(Command::SetProp {
                id: *id,
                key: "glazingRatio".into(),
                value: Some(json!((wp.glazing_ratio[i] * 1000.0).round() / 1000.0)),
            });
        }
    }
    cmds
}
