//! Preliminary MEP router: turns the computed loads into runs the model can hold (`MepRun`) and diffusers.
//!
//! - **HVAC.** From the air handler's room, a supply trunk and a return trunk run along one axis, with a branch to the
//!   centre of every room on the same level; a diffuser (supply) and a grille (return) sit at the end of each branch. Ducts are
//!   sized from the room's cooling load and your supply temperature difference and duct velocity, and run hard under the
//!   slab soffit inside the ceiling void (between your false-ceiling height and the soffit), where the beams hang: the
//!   clash check then shows every duct-to-beam intersection.
//! - **Drainage.** From each wet room's centre a drain runs, straight, to the nearest side of the building and on to a stack
//!   or manhole outside it, falling by your slope. A wet room above the lowest level gets a branch to a stack inside the room, at the
//!   point nearest its centre that keeps your clearance from every column; the stack drops through the levels below to the invert.
//! - **Water supply.** From your tank a trunk and a branch to every room with fixtures, at your supply height, sized by the
//!   pipe table; a riser joins the tank to that height.
//! - **Cables.** From each distribution board a cable tray trunk with a branch to the centre of each circuit.
//!
//! Every value is yours (see `mep::Routing`). A system with an input missing is not routed and the missing names are
//! listed. Routes are straight Manhattan legs: no obstacle avoidance, which is exactly what the clash check is for.

use crate::mep::{AIR_CP, AIR_DENSITY, MepInputs, MepReport, PointSpec, RoomG, Routing, centre, round2};
use crate::model::{Discipline, Id, Model};
use crate::typed::{Typed, Wall};
use serde::Serialize;

/// A run to create, in millimetres (heights above the floor top of its level).
#[derive(Debug, Clone, PartialEq)]
pub struct RunSpec {
    pub level: Option<Id>,
    /// `duct`, `pipe`, `drain` or `cable`.
    pub system: String,
    pub role: String,
    pub room: Option<String>,
    pub label: Option<String>,
    pub p1: [i64; 3],
    pub p2: [i64; 3],
    pub width: i64,
    pub height: i64,
}

/// One line of the route table the page shows.
#[derive(Debug, Clone, Serialize)]
pub struct RouteRow {
    pub system: String,
    pub role: String,
    pub room: Option<String>,
    pub label: Option<String>,
    pub from: [i64; 3],
    pub to: [i64; 3],
    pub length_m: f64,
    pub size_mm: String,
    /// Fall of a drain, percent.
    pub slope_pct: Option<f64>,
}

impl RunSpec {
    fn row(&self) -> RouteRow {
        let d: Vec<f64> = (0..3).map(|i| (self.p2[i] - self.p1[i]) as f64).collect();
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() / 1000.0;
        let horizontal = (d[0].abs() + d[1].abs()).max(0.0);
        let slope = (d[2] != 0.0 && horizontal > 0.0).then(|| round2(-d[2] / horizontal * 100.0));
        RouteRow { system: self.system.clone(), role: self.role.clone(), room: self.room.clone(), label: self.label.clone(), from: self.p1, to: self.p2, length_m: round2(len), size_mm: format!("{}x{}", self.width, self.height), slope_pct: slope }
    }
}

/// Height of the storey of `level` in millimetres: its tallest wall.
fn storey_mm(model: &Model, level: Option<Id>) -> Option<i64> {
    model.elements.values().filter(|e| e.kind == Wall::KIND && e.level_id == level).filter_map(|e| e.typed::<Wall>().ok()).map(|w| w.height).max()
}

/// Bounding box `[x0, y0, x1, y1]` of the rooms on `level`.
fn outline(rooms: &[RoomG], level: Option<Id>) -> Option<[i64; 4]> {
    rooms.iter().filter(|r| r.level == level).map(|r| [r.rect.x, r.rect.y, r.rect.x + r.rect.w, r.rect.y + r.rect.h]).reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])
}

/// A trunk along `y = origin.y` and a branch along `x = target.x` to each target. Legs of zero length are left out.
struct Tee {
    trunk: Option<([i64; 2], [i64; 2])>,
    branches: Vec<Option<([i64; 2], [i64; 2])>>,
}

fn tee(origin: (i64, i64), targets: &[(i64, i64)]) -> Tee {
    let (x0, x1) = targets.iter().fold((origin.0, origin.0), |(a, b), t| (a.min(t.0), b.max(t.0)));
    Tee {
        trunk: (x0 != x1).then_some(([x0, origin.1], [x1, origin.1])),
        branches: targets.iter().map(|t| (t.1 != origin.1).then_some(([t.0, origin.1], [t.0, t.1]))).collect(),
    }
}

fn push_run(rep: &mut MepReport, run: RunSpec) {
    rep.routes.push(run.row());
    rep.runs.push(run);
}

/// What the duct router needs to go around the structure: the obstacles of the level, the heights a duct may take, the clearance.
struct Avoid {
    obstacles: Vec<crate::ductpath::Obstacle>,
    zs: Vec<i64>,
    cl: i64,
    bounds: [i64; 4],
    half_h: i64,
    rerouted: usize,
    unavoidable: Vec<String>,
}

/// A duct leg: straight, or around the structure when `av` is given.
#[allow(clippy::too_many_arguments)]
fn emit_duct(rep: &mut MepReport, av: &mut Option<Avoid>, level: Option<Id>, role: &str, room: Option<&str>, label: String, a: [i64; 2], b: [i64; 2], z: i64, width: i64, height: i64) {
    let Some(av) = av.as_mut() else {
        push_run(rep, leg(level, "duct", role, room, label, a, b, z, width, height));
        return;
    };
    let path = crate::ductpath::find(a, b, &av.zs, width / 2, av.half_h, av.cl, &av.obstacles, av.bounds);
    if path.rerouted {
        av.rerouted += 1;
    }
    for id in path.unavoidable {
        if !av.unavoidable.contains(&id) {
            av.unavoidable.push(id);
        }
    }
    for w in path.points.windows(2) {
        if w[0] != w[1] {
            push_run(rep, RunSpec { level, system: "duct".into(), role: role.into(), room: room.map(str::to_string), label: Some(label.clone()), p1: w[0], p2: w[1], width, height });
        }
    }
}

/// A leg between two plan points at height `z` as a run.
#[allow(clippy::too_many_arguments)]
fn leg(level: Option<Id>, system: &str, role: &str, room: Option<&str>, label: String, a: [i64; 2], b: [i64; 2], z: i64, width: i64, height: i64) -> RunSpec {
    RunSpec { level, system: system.into(), role: role.into(), room: room.map(str::to_string), label: Some(label), p1: [a[0], a[1], z], p2: [b[0], b[1], z], width, height }
}

fn clamp_into(r: &RoomG, x: i64, y: i64) -> (i64, i64) {
    let inset = 100.min(r.rect.w / 4).min(r.rect.h / 4);
    (x.clamp(r.rect.x + inset, r.rect.x + r.rect.w - inset), y.clamp(r.rect.y + inset, r.rect.y + r.rect.h - inset))
}

pub(crate) fn route(model: &Model, rooms: &[RoomG], inputs: &MepInputs, r: &Routing, rep: &mut MepReport) {
    hvac_routes(model, rooms, r, rep);
    drainage_routes(model, rooms, inputs, r, rep);
    supply_routes(rooms, inputs, r, rep);
    cable_routes(model, rooms, r, rep);
}

// ------------------------------------------------------------------ HVAC

fn hvac_routes(model: &Model, rooms: &[RoomG], r: &Routing, rep: &mut MepReport) {
    let requested = r.ahu_room.is_some() || r.supply_delta_t_c.is_some() || r.duct_velocity_mps.is_some() || r.duct_height_mm.is_some() || r.ceiling_height_mm.is_some() || r.return_offset_mm.is_some();
    if !requested {
        return;
    }
    let mut missing: Vec<String> = Vec::new();
    for (name, v) in [("supply_delta_t_c", r.supply_delta_t_c), ("duct_velocity_mps", r.duct_velocity_mps), ("duct_height_mm", r.duct_height_mm), ("ceiling_height_mm", r.ceiling_height_mm), ("return_offset_mm", r.return_offset_mm)] {
        if v.is_none() {
            missing.push(format!("routing.{name}"));
        }
    }
    let ahu = r.ahu_room.as_deref().and_then(|k| rooms.iter().find(|x| x.key == k));
    if ahu.is_none() {
        missing.push("routing.ahu_room (the key of a room in the plan)".into());
    }
    if rep.hvac.is_none() {
        missing.push("hvac loads (the hvac inputs must be complete first)".into());
    }
    if let Some(dt) = r.supply_delta_t_c.filter(|d| *d > 0.0)
        && let Some(h) = rep.hvac.as_mut()
    {
        for hr in h.rooms.iter_mut() {
            hr.airflow_lps = Some(round2(hr.load_w / (AIR_DENSITY * AIR_CP * dt) * 1000.0));
        }
    }
    if !missing.is_empty() {
        rep.missing.insert("routing.hvac".into(), missing);
        return;
    }
    let (ahu, hv) = (ahu.expect("checked"), rep.hvac.clone().expect("checked"));
    let (dt, vel, dh, ceil, off) = (r.supply_delta_t_c.expect("checked"), r.duct_velocity_mps.expect("checked"), r.duct_height_mm.expect("checked"), r.ceiling_height_mm.expect("checked"), r.return_offset_mm.expect("checked"));
    if dt <= 0.0 || vel <= 0.0 || dh <= 0.0 {
        rep.warnings.push("routing: the supply temperature difference, duct velocity and duct height must be positive".into());
        return;
    }
    let Some(storey) = storey_mm(model, ahu.level) else {
        rep.warnings.push("routing: the air handler's level has no walls, so the slab soffit is unknown".into());
        return;
    };
    let plenum = storey as f64 - ceil;
    if dh > plenum {
        rep.warnings.push(format!("ducts not routed: the {dh:.0} mm duct does not fit the ceiling void of {plenum:.0} mm (storey {storey} mm, false ceiling at {ceil:.0} mm)"));
        return;
    }
    let z = (storey as f64 - dh / 2.0).round() as i64;
    let width_for = |lps: f64| -> i64 {
        let area_mm2 = lps / 1000.0 / vel * 1e6;
        (((area_mm2 / dh) / 50.0).ceil() * 50.0).max(100.0) as i64
    };
    let off = off.round() as i64;
    let (ax, ay) = centre(&ahu.rect);
    let mut targets: Vec<(&RoomG, f64)> = Vec::new();
    for hr in &hv.rooms {
        let Some(room) = rooms.iter().find(|x| x.key == hr.room) else { continue };
        if room.level != ahu.level {
            rep.warnings.push(format!("room '{}' is on another level than the air handler: no vertical duct is routed to it", room.key));
            continue;
        }
        targets.push((room, hr.load_w / (AIR_DENSITY * AIR_CP * dt) * 1000.0));
    }
    let total: f64 = targets.iter().map(|t| t.1).sum();
    let centres: Vec<(i64, i64)> = targets.iter().map(|(room, _)| centre(&room.rect)).collect();
    let dh_i = dh.round() as i64;
    // Obstacle-aware routing: the beams and columns of the handler's level, and the heights a duct may take.
    let mut av: Option<Avoid> = None;
    if r.avoid_structure == Some(true) {
        let Some(cl) = r.structure_clearance_mm.filter(|c| *c >= 0.0) else {
            rep.missing.insert("routing.structure".into(), vec!["routing.structure_clearance_mm (the gap kept between a duct and the structure)".into()]);
            return;
        };
        let scene = crate::model3d::build(model);
        let idx = crate::building::levels(model).iter().find(|(_, id)| Some(*id) == ahu.level).map(|(i, _)| *i);
        let base = idx.and_then(|i| scene.levels.iter().find(|l| l.index == i)).map_or(0.0, |l| l.z0 * 1000.0);
        let obstacles: Vec<crate::ductpath::Obstacle> = scene
            .boxes
            .iter()
            .filter(|b| matches!(b.kind, "Beam" | "Column") && b.level == idx)
            .map(|b| crate::ductpath::Obstacle { id: b.id.clone(), rect: [(b.min[0] * 1000.0).round() as i64, (b.min[1] * 1000.0).round() as i64, (b.max[0] * 1000.0).round() as i64, (b.max[1] * 1000.0).round() as i64], z0: (b.min[2] * 1000.0 - base).round() as i64, z1: (b.max[2] * 1000.0 - base).round() as i64 })
            .collect();
        let cl = cl.round() as i64;
        let half_h = dh_i / 2;
        let mut zs = vec![z];
        // Under the lowest beam: the duct centre this far below its soffit, if the duct still clears the false ceiling.
        if let Some(low) = obstacles.iter().filter(|o| o.id.is_empty() || scene.boxes.iter().any(|b| b.id == o.id && b.kind == "Beam")).map(|o| o.z0).min() {
            let z_low = low - cl - half_h;
            if z_low < z && (z_low - half_h) as f64 >= ceil {
                zs.push(z_low);
            }
        }
        let bounds = outline(rooms, ahu.level).unwrap_or([0, 0, 0, 0]);
        av = Some(Avoid { obstacles, zs, cl, bounds, half_h, rerouted: 0, unavoidable: Vec::new() });
    }
    for (role, shift) in [("supply", 0), ("return", off)] {
        let t = tee((ax, ay + shift), &centres.iter().map(|c| (c.0 + shift, c.1 + shift)).collect::<Vec<_>>());
        if let Some((a, b)) = t.trunk {
            emit_duct(rep, &mut av, ahu.level, role, None, format!("{role} trunk {total:.0} L/s"), a, b, z, width_for(total), dh_i);
        }
        for (((room, lps), c), br) in targets.iter().zip(&centres).zip(&t.branches) {
            if let Some((a, b)) = br {
                emit_duct(rep, &mut av, ahu.level, role, Some(&room.key), format!("{role} branch {lps:.0} L/s"), *a, *b, z, width_for(*lps), dh_i);
            }
            // The diffuser at the room's centre, the return grille beside it (kept inside the room).
            let (px, py) = if shift == 0 { *c } else { clamp_into(room, c.0 + shift, c.1 + shift) };
            rep.points.push(PointSpec {
                level: room.level,
                role: if shift == 0 { "diffuser".into() } else { "grille".into() },
                discipline: Discipline::Hvac,
                x: px,
                y: py,
                room: room.key.clone(),
                watts: None,
                circuit: None,
                label: Some(format!("{role} {lps:.0} L/s")),
            });
        }
    }

    if let Some(av) = av {
        if av.rerouted > 0 {
            rep.warnings.push(format!("{} duct leg(s) were routed around the structure (under a beam or around a column)", av.rerouted));
        }
        if !av.unavoidable.is_empty() {
            rep.warnings.push(format!("duct(s) cross {} structural element(s) that cannot be avoided at this ceiling height (no room under the beam or no way around): {}; the clash check lists each one", av.unavoidable.len(), av.unavoidable.join(", ")));
        }
    }
}

// ------------------------------------------------------------------ drainage

fn drainage_routes(model: &Model, rooms: &[RoomG], inputs: &MepInputs, r: &Routing, rep: &mut MepReport) {
    let requested = r.drain_slope_pct.is_some() || r.drain_invert_mm.is_some() || r.stack_offset_mm.is_some();
    if !requested {
        return;
    }
    let mut missing: Vec<String> = Vec::new();
    for (name, v) in [("drain_slope_pct", r.drain_slope_pct), ("drain_invert_mm", r.drain_invert_mm), ("stack_offset_mm", r.stack_offset_mm)] {
        if v.is_none() {
            missing.push(format!("routing.{name}"));
        }
    }
    let Some(pl) = rep.plumbing.clone() else {
        missing.push("plumbing (the plumbing inputs must be complete first)".into());
        rep.missing.insert("routing.drainage".into(), missing);
        return;
    };
    if !missing.is_empty() {
        rep.missing.insert("routing.drainage".into(), missing);
        return;
    }
    let (slope, invert, stack) = (r.drain_slope_pct.expect("checked"), r.drain_invert_mm.expect("checked"), r.stack_offset_mm.expect("checked"));
    if slope <= 0.0 || invert < 0.0 || stack < 0.0 {
        rep.warnings.push("routing: the drain slope must be positive, the invert depth and stack offset zero or more".into());
        return;
    }
    let levels = crate::building::levels(model);
    let lowest = levels.first().map(|(_, id)| *id);
    let _ = inputs;
    let drop = DrainFall { slope, invert: invert.round() as i64, stack: stack.round() as i64 };
    let mut env: Option<StackEnv> = None;
    let mut upper_without_clearance: Vec<String> = Vec::new();
    for pr in &pl.rooms {
        let Some(room) = rooms.iter().find(|x| x.key == pr.room) else { continue };
        let d = pr.drain_mm.round() as i64;
        if room.level == lowest {
            let Some(o) = outline(rooms, room.level) else { continue };
            outfall(rep, room.level, &room.key, centre(&room.rect), d, o, &drop);
            continue;
        }
        let Some(pos) = levels.iter().position(|(_, id)| Some(*id) == room.level) else {
            rep.warnings.push(format!("drain of room '{}' is not routed: the room is on no level of the model", room.key));
            continue;
        };
        let Some(clearance) = r.stack_clearance_mm else {
            upper_without_clearance.push(room.key.clone());
            continue;
        };
        if clearance < 0.0 {
            rep.warnings.push("routing: the stack clearance must be zero or more".into());
            return;
        }
        let env = env.get_or_insert_with(|| StackEnv::new(model, &levels));
        route_stack(rep, rooms, &levels, env, room, pos, d, clearance.round() as i64, &drop);
    }
    if !upper_without_clearance.is_empty() {
        rep.missing.insert(
            "routing.drainage.stacks".into(),
            vec![format!("routing.stack_clearance_mm (the gap kept between a vertical stack and a column; needed by the wet rooms above the lowest level: {})", upper_without_clearance.join(", "))],
        );
    }
}

/// The fall, the depth and the stack offset of the drains, millimetres and percent.
struct DrainFall {
    slope: f64,
    invert: i64,
    stack: i64,
}

/// A drain from `from` (plan point, at the invert depth) to the nearest side of the building outline `o` and on to the stack or
/// manhole `drop.stack` outside it, falling by the slope. Returns the point reached and the level of the bottom of the fall.
fn outfall(rep: &mut MepReport, level: Option<Id>, room: &str, from: (i64, i64), d: i64, o: [i64; 4], drop: &DrainFall) {
    let (cx, cy) = from;
    // Nearest side of the building: west, east, south, north.
    let sides = [(cx - o[0], [o[0] - drop.stack, cy]), (o[2] - cx, [o[2] + drop.stack, cy]), (cy - o[1], [cx, o[1] - drop.stack]), (o[3] - cy, [cx, o[3] + drop.stack])];
    let (_, end) = sides.iter().min_by_key(|(d, _)| *d).copied().expect("four sides");
    let length = ((end[0] - cx).abs() + (end[1] - cy).abs()) as f64;
    let z1 = -drop.invert;
    let z2 = z1 - (length * drop.slope / 100.0).round() as i64;
    push_run(rep, RunSpec { level, system: "drain".into(), role: "drain".into(), room: Some(room.to_string()), label: Some(format!("{d} mm drain to the stack at ({}, {})", end[0], end[1])), p1: [cx, cy, z1], p2: [end[0], end[1], z2], width: d, height: d });
}

// ------------------------------------------------------------------ vertical stacks

/// What a stack must keep clear of: the columns of the levels it passes (plan rectangles in millimetres, with the level
/// index they belong to), the elevation of each level's floor top, and the stacks already placed.
struct StackEnv {
    columns: Vec<(i64, [i64; 4])>,
    /// Elevation of the floor top of each level above the ground floor, millimetres, in the order of `building::levels`.
    elevations: Vec<i64>,
    /// Level index of each level, in the same order.
    indices: Vec<i64>,
    placed: Vec<[i64; 4]>,
}

impl StackEnv {
    fn new(model: &Model, levels: &[(i64, Id)]) -> StackEnv {
        let scene = crate::model3d::build(model);
        let elevations = levels.iter().map(|(i, _)| scene.levels.iter().find(|l| l.index == *i).map_or(0, |l| (l.z0 * 1000.0).round() as i64)).collect();
        let columns = scene
            .boxes
            .iter()
            .filter(|b| b.kind == "Column")
            .filter_map(|b| Some((b.level?, [(b.min[0] * 1000.0).round() as i64, (b.min[1] * 1000.0).round() as i64, (b.max[0] * 1000.0).round() as i64, (b.max[1] * 1000.0).round() as i64])))
            .collect();
        StackEnv { columns, elevations, indices: levels.iter().map(|(i, _)| *i).collect(), placed: Vec::new() }
    }
}

fn overlaps(a: [i64; 4], b: [i64; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// The plan footprint of a pipe along `a` to `b`, grown by `margin` on every side.
fn sweep(a: (i64, i64), b: (i64, i64), margin: i64) -> [i64; 4] {
    [a.0.min(b.0) - margin, a.1.min(b.1) - margin, a.0.max(b.0) + margin, a.1.max(b.1) + margin]
}

/// The corner of an L-shaped branch from `from` to `to`: along x first (`x_first`) or along y first.
fn corner(from: (i64, i64), to: (i64, i64), x_first: bool) -> (i64, i64) {
    if x_first { (to.0, from.1) } else { (from.0, to.1) }
}

/// A wet room above the lowest level: a branch drain from the room's centre to a stack, the stack down through the levels
/// below, and the drain from its foot to the outside. The stack stands inside the room, at the point nearest the room's centre
/// where the stack keeps `clearance` from every column (of this level or of those below) and from other stacks, and its branch
/// does not run through a column.
#[allow(clippy::too_many_arguments)]
fn route_stack(rep: &mut MepReport, rooms: &[RoomG], levels: &[(i64, Id)], env: &mut StackEnv, room: &RoomG, pos: usize, d: i64, clearance: i64, drop: &DrainFall) {
    let level_idx = env.indices[pos];
    let margin = (d + 1) / 2 + clearance;
    let (cx, cy) = centre(&room.rect);
    let blocked = |r: [i64; 4]| env.columns.iter().any(|(l, c)| *l <= level_idx && overlaps(r, *c));
    let free = |s: (i64, i64), x_first: bool| -> bool {
        let stack_box = sweep(s, s, margin);
        if blocked(stack_box) || env.placed.iter().any(|p| overlaps(stack_box, *p)) {
            return false;
        }
        let k = corner((cx, cy), s, x_first);
        // The branch is a horizontal pipe under the floor: it must not run through a column, but only the stack keeps the clearance.
        let body = (d + 1) / 2;
        !blocked(sweep((cx, cy), k, body)) && !blocked(sweep(k, s, body))
    };
    // Candidates inside the room: the centre first, then a 50 mm grid by Manhattan distance from the centre.
    let inset = (d + 1) / 2 + clearance;
    let mut cand: Vec<(i64, i64, i64)> = vec![(0, cx, cy)];
    let (x0, x1, y0, y1) = (room.rect.x + inset, room.rect.x + room.rect.w - inset, room.rect.y + inset, room.rect.y + room.rect.h - inset);
    let mut x = x0;
    while x <= x1 {
        let mut y = y0;
        while y <= y1 {
            cand.push(((x - cx).abs() + (y - cy).abs(), x, y));
            y += 50;
        }
        x += 50;
    }
    cand.sort();
    let mut chosen: Option<((i64, i64), bool)> = None;
    for (_, sx, sy) in cand {
        if let Some(x_first) = [true, false].into_iter().find(|xf| free((sx, sy), *xf)) {
            chosen = Some(((sx, sy), x_first));
            break;
        }
    }
    let Some((s, x_first)) = chosen else {
        rep.warnings.push(format!("stack of room '{}' is not routed: no point of the room keeps {} mm from every column with the {} mm stack", room.key, clearance, d));
        return;
    };
    // Branch drain at the invert depth, falling by the slope along each leg.
    let k = corner((cx, cy), s, x_first);
    let mut z = -drop.invert;
    for (a, b) in [((cx, cy), k), (k, s)] {
        let len = ((b.0 - a.0).abs() + (b.1 - a.1).abs()) as f64;
        if len == 0.0 {
            continue;
        }
        let z2 = z - (len * drop.slope / 100.0).round() as i64;
        push_run(rep, RunSpec { level: room.level, system: "drain".into(), role: "branch".into(), room: Some(room.key.clone()), label: Some(format!("{d} mm branch drain to the stack at ({}, {})", s.0, s.1)), p1: [a.0, a.1, z], p2: [b.0, b.1, z2], width: d, height: d });
        z = z2;
    }
    // The stack, from the end of the branch down to the invert at the lowest level, one run per level it crosses.
    let top = env.elevations[pos] + z;
    let bottom = -drop.invert;
    if top <= bottom {
        rep.warnings.push(format!("stack of room '{}' is not routed: its branch ends at {top} mm, not above the lowest invert at {bottom} mm", room.key));
        return;
    }
    for (j, (_, level_id)) in levels.iter().enumerate().take(pos) {
        let lo = if j == 0 { bottom } else { env.elevations[j] };
        let hi = top.min(env.elevations[j + 1]);
        if hi <= lo {
            continue;
        }
        let base = env.elevations[j];
        push_run(rep, RunSpec { level: Some(*level_id), system: "drain".into(), role: "stack".into(), room: Some(room.key.clone()), label: Some(format!("{d} mm stack of room {} at ({}, {})", room.key, s.0, s.1)), p1: [s.0, s.1, hi - base], p2: [s.0, s.1, lo - base], width: d, height: d });
    }
    env.placed.push(sweep(s, s, (d + 1) / 2));
    // Rooms the stack passes through on the levels below: reported, since a stack in a living room is a design decision.
    let through: Vec<&str> = rooms.iter().filter(|x| x.level != room.level && levels[..pos].iter().any(|(_, id)| Some(*id) == x.level) && s.0 > x.rect.x && s.0 < x.rect.x + x.rect.w && s.1 > x.rect.y && s.1 < x.rect.y + x.rect.h).map(|x| x.key.as_str()).collect();
    if !through.is_empty() {
        rep.warnings.push(format!("the stack of room '{}' at ({}, {}) passes through room(s) on the levels below: {}", room.key, s.0, s.1, through.join(", ")));
    }
    // Its foot drains to the outside from the lowest level's invert, like a drain of a room on that level.
    let (ground, foot) = (Some(levels[0].1), (s.0, s.1));
    if let Some(o) = outline(rooms, ground) {
        outfall(rep, ground, &room.key, foot, d, o, drop);
    } else {
        rep.warnings.push(format!("the stack of room '{}' has no drain at its foot: the lowest level has no rooms, so the building outline is unknown", room.key));
    }
}

// ------------------------------------------------------------------ water supply

fn supply_routes(rooms: &[RoomG], inputs: &MepInputs, r: &Routing, rep: &mut MepReport) {
    let requested = r.tank.is_some() || r.supply_z_mm.is_some();
    if !requested {
        return;
    }
    let mut missing: Vec<String> = Vec::new();
    if r.tank.is_none() {
        missing.push("routing.tank".into());
    }
    if r.supply_z_mm.is_none() {
        missing.push("routing.supply_z_mm".into());
    }
    let (Some(pl), Some(pin)) = (rep.plumbing.clone(), inputs.plumbing.as_ref()) else {
        missing.push("plumbing (the plumbing inputs must be complete first)".into());
        rep.missing.insert("routing.supply".into(), missing);
        return;
    };
    if !missing.is_empty() {
        rep.missing.insert("routing.supply".into(), missing);
        return;
    }
    let (tank, z) = (r.tank.clone().expect("checked"), r.supply_z_mm.expect("checked").round() as i64);
    if !matches!(tank.kind.as_str(), "roof" | "ground") {
        rep.warnings.push(format!("routing: the tank kind '{}' must be roof or ground", tank.kind));
        return;
    }
    let inner = |name: &str| pin.pipes.iter().find(|p| p.name == name).map(|p| p.inner_mm.round() as i64);
    let Some(main_d) = inner(&pl.main_pipe) else { return };
    let (tx, ty, tz) = (tank.x_mm.round() as i64, tank.y_mm.round() as i64, tank.z_mm.round() as i64);
    // Rooms with fixtures, on the level of the first of them (the supply runs at one height above that floor).
    let served: Vec<(&RoomG, String, i64)> = pl.rooms.iter().filter_map(|pr| Some((rooms.iter().find(|x| x.key == pr.room)?, pr.pipe.clone(), inner(&pr.pipe)?))).collect();
    let Some(level) = served.first().map(|s| s.0.level) else { return };
    let mut on_level: Vec<(&RoomG, String, i64)> = Vec::new();
    for s in served {
        if s.0.level == level {
            on_level.push(s);
        } else {
            rep.warnings.push(format!("room '{}' is on another level than the supply: no riser between levels is generated", s.0.key));
        }
    }
    if tz != z {
        push_run(rep, RunSpec { level, system: "pipe".into(), role: "riser".into(), room: None, label: Some(format!("{} riser from the {} tank", pl.main_pipe, tank.kind)), p1: [tx, ty, tz.min(z)], p2: [tx, ty, tz.max(z)], width: main_d, height: main_d });
    }
    let centres: Vec<(i64, i64)> = on_level.iter().map(|(room, _, _)| centre(&room.rect)).collect();
    let t = tee((tx, ty), &centres);
    if let Some((a, b)) = t.trunk {
        push_run(rep, leg(level, "pipe", "trunk", None, format!("{} supply trunk", pl.main_pipe), a, b, z, main_d, main_d));
    }
    for (((room, name, d), _), br) in on_level.iter().zip(&centres).zip(&t.branches) {
        if let Some((a, b)) = br {
            push_run(rep, leg(level, "pipe", "branch", Some(&room.key), format!("{name} branch"), *a, *b, z, *d, *d));
        }
    }
}

// ------------------------------------------------------------------ cables

fn cable_routes(model: &Model, rooms: &[RoomG], r: &Routing, rep: &mut MepReport) {
    let requested = r.tray_z_mm.is_some() || r.tray_width_mm.is_some() || r.tray_height_mm.is_some();
    if !requested {
        return;
    }
    let mut missing: Vec<String> = Vec::new();
    for (name, v) in [("tray_z_mm", r.tray_z_mm), ("tray_width_mm", r.tray_width_mm), ("tray_height_mm", r.tray_height_mm)] {
        if v.is_none() {
            missing.push(format!("routing.{name}"));
        }
    }
    let Some(el) = rep.electrical.clone() else {
        missing.push("electrical (the electrical inputs must be complete first)".into());
        rep.missing.insert("routing.cables".into(), missing);
        return;
    };
    if !missing.is_empty() {
        rep.missing.insert("routing.cables".into(), missing);
        return;
    }
    let (z, w, h) = (r.tray_z_mm.expect("checked").round() as i64, r.tray_width_mm.expect("checked").round() as i64, r.tray_height_mm.expect("checked").round() as i64);
    if crate::building::levels(model).len() > 1 {
        rep.warnings.push("cable trays are drawn on each board's own level only: risers between levels are not generated".into());
    }
    for b in &el.boards {
        let Some(room) = rooms.iter().find(|x| x.key == b.room) else { continue };
        let circuits: Vec<&crate::mep::Circuit> = el.circuits.iter().filter(|c| c.board == b.room).collect();
        let targets: Vec<(i64, i64)> = circuits.iter().map(|c| (c.x_mm, c.y_mm)).collect();
        let origin = centre(&room.rect);
        let t = tee(origin, &targets);
        if let Some((a, e)) = t.trunk {
            push_run(rep, leg(room.level, "cable", "trunk", Some(&room.key), format!("tray from {}", b.room), a, e, z, w, h));
        }
        for (c, br) in circuits.iter().zip(&t.branches) {
            if let Some((a, e)) = br {
                push_run(rep, leg(room.level, "cable", "feed", None, format!("{} {} mm2", c.id, c.cable_mm2), *a, *e, z, w, h));
            }
        }
    }
}
