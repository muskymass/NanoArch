//! Preliminary MEP router: turns the computed loads into runs the model can hold (`MepRun`) and diffusers.
//!
//! - **HVAC.** From the air handler's room, a supply trunk and a return trunk run along one axis, with a branch to the
//!   centre of every room on the same level; a diffuser (supply) and a grille (return) sit at the end of each branch. Ducts are
//!   sized from the room's cooling load and your supply temperature difference and duct velocity, and run hard under the
//!   slab soffit inside the ceiling void (between your false-ceiling height and the soffit), where the beams hang: the
//!   clash check then shows every duct-to-beam intersection.
//! - **Drainage.** From each wet room's centre a drain runs, straight, to the nearest side of the building and on to a stack
//!   or manhole outside it, falling by your slope. Only rooms on the lowest level are routed (an upper floor needs a
//!   vertical stack, which is not generated).
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
    for (role, shift) in [("supply", 0), ("return", off)] {
        let t = tee((ax, ay + shift), &centres.iter().map(|c| (c.0 + shift, c.1 + shift)).collect::<Vec<_>>());
        if let Some((a, b)) = t.trunk {
            push_run(rep, leg(ahu.level, "duct", role, None, format!("{role} trunk {total:.0} L/s"), a, b, z, width_for(total), dh_i));
        }
        for (((room, lps), c), br) in targets.iter().zip(&centres).zip(&t.branches) {
            if let Some((a, b)) = br {
                push_run(rep, leg(ahu.level, "duct", role, Some(&room.key), format!("{role} branch {lps:.0} L/s"), *a, *b, z, width_for(*lps), dh_i));
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
    let lowest = crate::building::levels(model).first().map(|(_, id)| *id);
    let _ = inputs;
    for pr in &pl.rooms {
        let Some(room) = rooms.iter().find(|x| x.key == pr.room) else { continue };
        if room.level != lowest {
            rep.warnings.push(format!("drain of room '{}' is on an upper level: a vertical stack is needed and is not generated", room.key));
            continue;
        }
        let Some(o) = outline(rooms, room.level) else { continue };
        let (cx, cy) = centre(&room.rect);
        // Nearest side of the building: west, east, south, north.
        let sides = [(cx - o[0], [o[0] - stack as i64, cy]), (o[2] - cx, [o[2] + stack as i64, cy]), (cy - o[1], [cx, o[1] - stack as i64]), (o[3] - cy, [cx, o[3] + stack as i64])];
        let (_, end) = sides.iter().min_by_key(|(d, _)| *d).copied().expect("four sides");
        let length = ((end[0] - cx).abs() + (end[1] - cy).abs()) as f64;
        let z1 = -(invert.round() as i64);
        let z2 = z1 - (length * slope / 100.0).round() as i64;
        let d = pr.drain_mm.round() as i64;
        push_run(rep, RunSpec { level: room.level, system: "drain".into(), role: "drain".into(), room: Some(room.key.clone()), label: Some(format!("{d} mm drain to the stack at ({}, {})", end[0], end[1])), p1: [cx, cy, z1], p2: [end[0], end[1], z2], width: d, height: d });
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
