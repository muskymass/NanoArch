//! Preliminary MEP engines (electrical, plumbing, HVAC) driven by the model and by YOUR parameters.
//!
//! The mechanics are physics and arithmetic: current from power, voltage drop from resistance, pipe diameter
//! from flow and velocity, cooling load from conduction, solar gain, people, lights and ventilation. The DATA is
//! yours: voltages, demand factors, cable ampacity tables, pipe sizes, fixture flows, design temperatures,
//! unit capacities, safety factors. None is defaulted. A discipline whose inputs are incomplete is skipped and
//! the report names every missing value, exactly as the schedule does with production rates.
//!
//! The only constants in this file are physical: the density and specific heat of air, and the watts in one
//! ton of refrigeration. Results are preliminary: a sketch for planning and early costing, to be checked by a
//! licensed MEP engineer against the project's code.
//!
//! Points (lights, sockets, fixtures, the panel, air-conditioning units) become `MepPoint` elements of the model,
//! so they have ids, appear in the drawing and the quantities, and are undoable like everything else. Routing of
//! cables, pipes and ducts is NOT done: lengths are straight-line estimates (Manhattan distance times your
//! route factor).

use crate::command::Command;
use crate::history::Document;
use crate::insulation::{self, AssemblyReport, InsulationLib};
use crate::layout::Rect;
use crate::model::{Discipline, Id, Model, Source};
use crate::qto::QtoItem;
use crate::typed::{MepPoint, Space, Typed, Wall, Window, element_of};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const AIR_DENSITY_KG_M3: f64 = 1.2;
const AIR_CP_J_KG_K: f64 = 1006.0;
pub(crate) const WATTS_PER_TON_REFRIGERATION: f64 = 3516.85;
/// Btu per hour in one watt.
const BTU_HR_PER_WATT: f64 = 3.412142;
pub(crate) const AIR_DENSITY: f64 = AIR_DENSITY_KG_M3;
pub(crate) const AIR_CP: f64 = AIR_CP_J_KG_K;

// ------------------------------------------------------------------ inputs

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MepInputs {
    #[serde(default)]
    pub electrical: Option<Elec>,
    #[serde(default)]
    pub plumbing: Option<Plumb>,
    #[serde(default)]
    pub hvac: Option<Hvac>,
    #[serde(default)]
    pub insulation: Option<InsulationLib>,
    /// Where the generated ducts, drains, supply pipes and cable trays go. Absent: nothing is routed.
    #[serde(default)]
    pub routing: Option<Routing>,
}

/// Position of the water tank feeding the building, millimetres in the plan (z above the floor top of the level where the
/// supply runs). `kind` is `roof` or `ground`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TankPos {
    pub kind: String,
    pub x_mm: f64,
    pub y_mm: f64,
    pub z_mm: f64,
}

/// Inputs of the router. Every value is yours (design criteria, the project's drawings): nothing is defaulted, and a
/// system with an input missing is not routed, the missing names are listed under `routing.<system>`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Routing {
    /// HVAC: requirement key of the room that holds the air handler (the ducts start at its centre).
    pub ahu_room: Option<String>,
    /// Supply air temperature difference, K.
    pub supply_delta_t_c: Option<f64>,
    pub duct_velocity_mps: Option<f64>,
    pub duct_height_mm: Option<f64>,
    /// Underside of the false ceiling above the floor, mm. The ducts run between it and the slab soffit.
    pub ceiling_height_mm: Option<f64>,
    /// Distance between a supply run and its return run, mm.
    pub return_offset_mm: Option<f64>,
    /// Drainage: fall of the drains, percent (for example 1.5), the depth of the start of a drain below the floor top, and
    /// how far outside the building outline the stack or manhole is, mm.
    pub drain_slope_pct: Option<f64>,
    pub drain_invert_mm: Option<f64>,
    pub stack_offset_mm: Option<f64>,
    /// Water supply: the tank and the height of the supply runs above the floor.
    pub tank: Option<TankPos>,
    pub supply_z_mm: Option<f64>,
    /// Cables: height and section of the cable trays in the ceiling void.
    pub tray_z_mm: Option<f64>,
    pub tray_width_mm: Option<f64>,
    pub tray_height_mm: Option<f64>,
    /// Route the HVAC ducts around beams and columns (under a beam where the ceiling void is deep enough). Off when absent.
    pub avoid_structure: Option<bool>,
    /// Gap kept between a duct and the structure it passes, mm. Required when `avoid_structure` is on.
    pub structure_clearance_mm: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CableRow {
    pub amps: f64,
    pub mm2: f64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Elec {
    /// Voltage of a branch circuit (phase to neutral), V.
    pub voltage_v: Option<f64>,
    pub power_factor: Option<f64>,
    /// Whether the main supply is three-phase (loads are shared by three equal phases).
    pub three_phase: Option<bool>,
    pub lights_per_m2: Option<f64>,
    pub light_w: Option<f64>,
    /// Sockets per metre of room perimeter.
    pub sockets_per_m: Option<f64>,
    pub socket_w: Option<f64>,
    pub demand_factor: Option<f64>,
    pub max_light_points: Option<f64>,
    pub max_socket_points: Option<f64>,
    /// Multiplier on the circuit current before choosing a breaker.
    pub breaker_margin: Option<f64>,
    #[serde(default)]
    pub breaker_sizes_a: Vec<f64>,
    #[serde(default)]
    pub cables: Vec<CableRow>,
    /// Conductor resistivity, ohm mm2 / m, at the temperature you design for.
    pub resistivity: Option<f64>,
    pub max_vdrop_pct: Option<f64>,
    /// Requirement key of the room that holds the distribution board.
    pub panel_room: Option<String>,
    /// Several distribution boards (room keys): each circuit is assigned to the nearest one. Overrides `panel_room`.
    #[serde(default)]
    pub boards: Vec<String>,
    /// Multiplier from straight-line to routed length.
    pub route_factor: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Fixture {
    pub flow_lps: f64,
    pub dfu: f64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PipeRow {
    pub name: String,
    pub inner_mm: f64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DrainRow {
    pub max_dfu: f64,
    pub dia_mm: f64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Plumb {
    #[serde(default)]
    pub catalog: BTreeMap<String, Fixture>,
    /// Room function -> fixture name -> count.
    #[serde(default)]
    pub fixtures_by_func: BTreeMap<String, BTreeMap<String, u32>>,
    /// Share of the connected flow that runs at the same time.
    pub simultaneity: Option<f64>,
    pub max_velocity_mps: Option<f64>,
    #[serde(default)]
    pub pipes: Vec<PipeRow>,
    #[serde(default)]
    pub drain_table: Vec<DrainRow>,
    pub persons: Option<f64>,
    pub litres_per_person_day: Option<f64>,
    pub storage_days: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Hvac {
    pub indoor_c: Option<f64>,
    pub outdoor_c: Option<f64>,
    pub window_u: Option<f64>,
    pub window_shgc: Option<f64>,
    /// Solar irradiance on the glazing, W/m2.
    pub solar_w_m2: Option<f64>,
    pub person_w: Option<f64>,
    pub area_per_person_m2: Option<f64>,
    pub lights_w_m2: Option<f64>,
    pub equipment_w_m2: Option<f64>,
    pub vent_l_s_per_m2: Option<f64>,
    pub safety_factor: Option<f64>,
    #[serde(default)]
    pub unit_sizes_w: Vec<f64>,
    /// Whether the roof is exposed over every room (single storey: yes).
    pub roof_exposed: Option<bool>,
    /// Coefficient of performance of the units: electrical input = cooling load / cop. With it, each room gets a dedicated
    /// circuit for its air-conditioning unit.
    pub cop: Option<f64>,
}

// ------------------------------------------------------------------ reports

#[derive(Debug, Clone)]
pub struct PointSpec {
    pub level: Option<Id>,
    pub role: String,
    pub discipline: Discipline,
    pub x: i64,
    pub y: i64,
    pub room: String,
    pub watts: Option<f64>,
    pub circuit: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Circuit {
    pub id: String,
    /// The distribution board (room key) that feeds it.
    pub board: String,
    /// Centre of its points, mm.
    pub x_mm: i64,
    pub y_mm: i64,
    pub kind: String,
    pub points: usize,
    pub rooms: Vec<String>,
    pub watts: f64,
    pub current_a: f64,
    pub breaker_a: f64,
    pub cable_mm2: f64,
    pub length_m: f64,
    pub vdrop_pct: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BoardLoad {
    pub room: String,
    pub circuits: usize,
    pub connected_w: f64,
    pub demand_w: f64,
    pub demand_kva: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ElecReport {
    pub circuits: Vec<Circuit>,
    pub boards: Vec<BoardLoad>,
    pub connected_kva: f64,
    pub demand_kva: f64,
    pub connected_w: f64,
    pub demand_w: f64,
    pub main_current_a: f64,
    pub main_breaker_a: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlumbRoom {
    pub room: String,
    pub flow_lps: f64,
    pub pipe: String,
    pub dfu: f64,
    pub drain_mm: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlumbReport {
    pub rooms: Vec<PlumbRoom>,
    pub connected_flow_lps: f64,
    pub design_flow_lps: f64,
    pub main_pipe: String,
    pub total_dfu: f64,
    pub main_drain_mm: f64,
    pub tank_m3: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HvacRoom {
    pub room: String,
    pub conduction_w: f64,
    pub glass_w: f64,
    pub internal_w: f64,
    pub ventilation_w: f64,
    pub load_w: f64,
    pub btu_hr: f64,
    pub tons: f64,
    /// Supply airflow, L/s, when the routing inputs give the supply temperature difference.
    pub airflow_lps: Option<f64>,
    pub units: u32,
    pub unit_w: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct HvacReport {
    pub rooms: Vec<HvacRoom>,
    pub total_w: f64,
    pub total_btu_hr: f64,
    pub total_tons: f64,
    pub delta_t: f64,
    pub wall_u: f64,
    pub roof_u: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct MepReport {
    pub electrical: Option<ElecReport>,
    pub plumbing: Option<PlumbReport>,
    pub hvac: Option<HvacReport>,
    pub insulation: Vec<AssemblyReport>,
    /// Discipline -> the inputs that are missing, so it was not computed.
    pub missing: BTreeMap<String, Vec<String>>,
    pub warnings: Vec<String>,
    /// One row per generated duct, drain, pipe or cable run.
    pub routes: Vec<crate::routing::RouteRow>,
    #[serde(skip)]
    pub runs: Vec<crate::routing::RunSpec>,
    #[serde(skip)]
    pub points: Vec<PointSpec>,
    #[serde(skip)]
    pub insulation_items: Vec<QtoItem>,
}

// ------------------------------------------------------------------ helpers

pub(crate) struct RoomG {
    pub(crate) level: Option<Id>,
    pub(crate) key: String,
    pub(crate) func: String,
    pub(crate) rect: Rect,
    pub(crate) area: f64,
}

pub(crate) fn read_rooms(model: &Model) -> Vec<RoomG> {
    let mut v: Vec<RoomG> = model
        .elements
        .values()
        .filter(|e| e.kind == Space::KIND)
        .filter_map(|e| Some((e.level_id, e.typed::<Space>().ok()?)))
        .map(|(level, s)| RoomG { level, key: s.requirement_key, func: s.func, rect: Rect { x: s.x, y: s.y, w: s.w, h: s.h }, area: s.area })
        .collect();
    v.sort_by(|a, b| a.key.cmp(&b.key));
    v
}

struct Need {
    missing: Vec<String>,
}

impl Need {
    fn f(&mut self, name: &str, v: Option<f64>) -> f64 {
        v.unwrap_or_else(|| {
            self.missing.push(name.to_string());
            0.0
        })
    }
    fn list(&mut self, name: &str, empty: bool) {
        if empty {
            self.missing.push(name.to_string());
        }
    }
}

pub(crate) fn centre(r: &Rect) -> (i64, i64) {
    (r.x + r.w / 2, r.y + r.h / 2)
}

/// `n` points spread over a room in a grid, row by row.
fn grid(r: &Rect, n: usize) -> Vec<(i64, i64)> {
    if n == 0 {
        return Vec::new();
    }
    let cols = (((n as f64) * r.w as f64 / r.h.max(1) as f64).sqrt().ceil() as usize).max(1);
    let rows = n.div_ceil(cols);
    let mut v = Vec::with_capacity(n);
    for j in 0..rows {
        for i in 0..cols {
            if v.len() < n {
                v.push((r.x + ((i as f64 + 0.5) / cols as f64 * r.w as f64) as i64, r.y + ((j as f64 + 0.5) / rows as f64 * r.h as f64) as i64));
            }
        }
    }
    v
}

/// `n` points evenly along the room's perimeter, inset from the walls.
fn along_walls(r: &Rect, n: usize) -> Vec<(i64, i64)> {
    const INSET: i64 = 200;
    let (x0, y0, x1, y1) = (r.x + INSET.min(r.w / 2 - 1), r.y + INSET.min(r.h / 2 - 1), r.x + r.w - INSET.min(r.w / 2 - 1), r.y + r.h - INSET.min(r.h / 2 - 1));
    let (iw, ih) = ((x1 - x0) as f64, (y1 - y0) as f64);
    let p = 2.0 * (iw + ih);
    (0..n)
        .map(|k| {
            let t = (k as f64 + 0.5) / n as f64 * p;
            if t < iw {
                (x0 + t as i64, y0)
            } else if t < iw + ih {
                (x1, y0 + (t - iw) as i64)
            } else if t < 2.0 * iw + ih {
                (x1 - (t - iw - ih) as i64, y1)
            } else {
                (x0, y1 - (t - 2.0 * iw - ih) as i64)
            }
        })
        .collect()
}

pub(crate) fn pick_ge(sorted: &[f64], need: f64) -> Option<f64> {
    sorted.iter().copied().find(|&x| x >= need - 1e-9)
}

pub(crate) fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

// ------------------------------------------------------------------ electrical

/// The values that size one circuit, read from the user's inputs once.
struct ElecCtx {
    v: f64,
    pf: f64,
    margin: f64,
    rho: f64,
    maxvd: f64,
    route: f64,
    demand_factor: f64,
    phases: f64,
    breakers: Vec<f64>,
    cables: Vec<CableRow>,
}

impl ElecCtx {
    /// Breaker and cable for a circuit of `watts` whose cable is `length_m` long: the smallest breaker above the design
    /// current, then the smallest cable that carries the breaker and keeps the voltage drop within the limit.
    /// Returns `(current_a, breaker_a, cable_mm2, vdrop_pct)`.
    fn size(&self, id: &str, watts: f64, length_m: f64, rep: &mut MepReport) -> (f64, f64, f64, f64) {
        let current = watts / (self.v * self.pf);
        let design = current * self.margin;
        let breaker = pick_ge(&self.breakers, design).unwrap_or_else(|| {
            rep.warnings.push(format!("circuit {id} needs {design:.1} A, above the largest breaker you listed"));
            *self.breakers.last().expect("non-empty")
        });
        let mut row = self.cables.iter().find(|c| c.amps >= breaker - 1e-9).unwrap_or_else(|| self.cables.last().expect("non-empty"));
        let vd = |c: &CableRow| 2.0 * current * length_m * self.rho / (c.mm2 * self.v) * 100.0;
        while vd(row) > self.maxvd {
            match self.cables.iter().find(|c| c.mm2 > row.mm2) {
                Some(next) => row = next,
                None => {
                    rep.warnings.push(format!("circuit {id}: even the largest cable gives {:.2} % voltage drop (limit {} %)", vd(row), self.maxvd));
                    break;
                }
            }
        }
        (current, breaker, row.mm2, vd(row))
    }
}

/// The board nearest (Manhattan) to `(x, y)`: `(index, distance_mm)`.
fn nearest_board(boards: &[(&RoomG, (i64, i64))], x: i64, y: i64) -> (usize, i64) {
    boards.iter().enumerate().map(|(i, (_, (bx, by)))| (i, (bx - x).abs() + (by - y).abs())).min_by_key(|(_, d)| *d).expect("at least one board")
}

fn electrical(rooms: &[RoomG], e: &Elec, rep: &mut MepReport) -> Option<ElecCtx> {
    let mut n = Need { missing: Vec::new() };
    let (v, pf, lpm, lw, spm, sw) = (
        n.f("electrical.voltage_v", e.voltage_v),
        n.f("electrical.power_factor", e.power_factor),
        n.f("electrical.lights_per_m2", e.lights_per_m2),
        n.f("electrical.light_w", e.light_w),
        n.f("electrical.sockets_per_m", e.sockets_per_m),
        n.f("electrical.socket_w", e.socket_w),
    );
    let (df, maxl, maxs, margin, rho, maxvd, route) = (
        n.f("electrical.demand_factor", e.demand_factor),
        n.f("electrical.max_light_points", e.max_light_points),
        n.f("electrical.max_socket_points", e.max_socket_points),
        n.f("electrical.breaker_margin", e.breaker_margin),
        n.f("electrical.resistivity", e.resistivity),
        n.f("electrical.max_vdrop_pct", e.max_vdrop_pct),
        n.f("electrical.route_factor", e.route_factor),
    );
    n.list("electrical.breaker_sizes_a", e.breaker_sizes_a.is_empty());
    n.list("electrical.cables", e.cables.is_empty());
    if e.three_phase.is_none() {
        n.missing.push("electrical.three_phase".into());
    }
    let board_keys: Vec<String> = if e.boards.is_empty() { e.panel_room.iter().cloned().collect() } else { e.boards.clone() };
    let mut board_rooms: Vec<&RoomG> = Vec::new();
    for k in &board_keys {
        match rooms.iter().find(|r| &r.key == k) {
            Some(r) => board_rooms.push(r),
            None => n.missing.push(format!("electrical.panel_room / boards: no room '{k}' in the plan")),
        }
    }
    if board_keys.is_empty() {
        n.missing.push("electrical.panel_room (the key of a room in the plan)".into());
    }
    if !n.missing.is_empty() {
        rep.missing.insert("electrical".into(), n.missing);
        return None;
    }
    let boards: Vec<(&RoomG, (i64, i64))> = board_rooms.iter().map(|r| (*r, centre(&r.rect))).collect();
    let mut breakers = e.breaker_sizes_a.clone();
    breakers.sort_by(f64::total_cmp);
    let mut cables = e.cables.clone();
    cables.sort_by(|a, b| a.amps.total_cmp(&b.amps));
    let phases = if e.three_phase == Some(true) { 3.0 } else { 1.0 };
    let ctx = ElecCtx { v, pf, margin, rho, maxvd, route, demand_factor: df, phases, breakers, cables };

    // Points: lights on a grid, sockets along the walls; rooms ordered west to east so circuits stay local.
    let mut ordered: Vec<&RoomG> = rooms.iter().collect();
    ordered.sort_by_key(|r| (r.rect.x, r.rect.y));
    let (mut lights, mut sockets): (Vec<PointSpec>, Vec<PointSpec>) = (Vec::new(), Vec::new());
    for r in &ordered {
        let nl = if lpm > 0.0 { ((r.area * lpm).ceil() as usize).max(1) } else { 0 };
        let perimeter_m = 2.0 * (r.rect.w + r.rect.h) as f64 / 1000.0;
        let ns = if spm > 0.0 { ((perimeter_m * spm).ceil() as usize).max(1) } else { 0 };
        for (x, y) in grid(&r.rect, nl) {
            lights.push(PointSpec { level: r.level, role: "light".into(), discipline: Discipline::Ele, x, y, room: r.key.clone(), watts: Some(lw), circuit: None, label: None });
        }
        for (x, y) in along_walls(&r.rect, ns) {
            sockets.push(PointSpec { level: r.level, role: "socket".into(), discipline: Discipline::Ele, x, y, room: r.key.clone(), watts: Some(sw), circuit: None, label: None });
        }
    }

    let mut circuits: Vec<Circuit> = Vec::new();
    for (prefix, kind, pts, max) in [("L", "lighting", &mut lights, maxl), ("S", "sockets", &mut sockets, maxs)] {
        let chunk = (max.floor() as usize).max(1);
        let ranges: Vec<(usize, usize)> = (0..pts.len()).step_by(chunk).map(|a| (a, (a + chunk).min(pts.len()))).collect();
        for (k, (a, b)) in ranges.into_iter().enumerate() {
            let id = format!("{prefix}{}", k + 1);
            let slice = &mut pts[a..b];
            slice.iter_mut().for_each(|p| p.circuit = Some(id.clone()));
            let watts: f64 = slice.iter().map(|p| p.watts.unwrap_or(0.0)).sum();
            let (cx, cy) = (slice.iter().map(|p| p.x).sum::<i64>() / slice.len() as i64, slice.iter().map(|p| p.y).sum::<i64>() / slice.len() as i64);
            // Length: from the nearest board to the centre of the circuit's points, times the route factor.
            let (bi, dist) = nearest_board(&boards, cx, cy);
            let length_m = dist as f64 / 1000.0 * route;
            let (current, breaker, mm2, vd) = ctx.size(&id, watts, length_m, rep);
            let mut rooms_of: Vec<String> = slice.iter().map(|p| p.room.clone()).collect();
            rooms_of.dedup();
            circuits.push(Circuit {
                id,
                board: boards[bi].0.key.clone(),
                x_mm: cx,
                y_mm: cy,
                kind: kind.into(),
                points: slice.len(),
                rooms: rooms_of,
                watts: round2(watts),
                current_a: round2(current),
                breaker_a: breaker,
                cable_mm2: mm2,
                length_m: round2(length_m),
                vdrop_pct: round2(vd),
            });
        }
    }
    for (room, (px, py)) in &boards {
        rep.points.push(PointSpec { level: room.level, role: "panel".into(), discipline: Discipline::Ele, x: *px, y: *py, room: room.key.clone(), watts: None, circuit: None, label: Some(if boards.len() > 1 { format!("DB {}", room.key) } else { "distribution board".into() }) });
    }
    rep.points.extend(lights);
    rep.points.extend(sockets);
    rep.electrical = Some(ElecReport { circuits, boards: Vec::new(), connected_kva: 0.0, demand_kva: 0.0, connected_w: 0.0, demand_w: 0.0, main_current_a: 0.0, main_breaker_a: 0.0 });
    finish_electrical(&ctx, rep);
    Some(ctx)
}

/// Totals of the circuits: per board and for the supply. Lighting and socket circuits take the demand factor you gave;
/// dedicated air-conditioning circuits (`kind` `hvac`) are counted at 100 %, since no diversity value is given for them.
fn finish_electrical(ctx: &ElecCtx, rep: &mut MepReport) {
    let Some(el) = rep.electrical.take() else { return };
    let mut keys: Vec<String> = el.circuits.iter().map(|c| c.board.clone()).collect();
    keys.sort();
    keys.dedup();
    let demand = |cs: &[&Circuit]| cs.iter().map(|c| c.watts * if c.kind == "hvac" { 1.0 } else { ctx.demand_factor }).sum::<f64>();
    let boards: Vec<BoardLoad> = keys
        .iter()
        .map(|k| {
            let cs: Vec<&Circuit> = el.circuits.iter().filter(|c| &c.board == k).collect();
            let (connected, dem) = (cs.iter().map(|c| c.watts).sum::<f64>(), demand(&cs));
            BoardLoad { room: k.clone(), circuits: cs.len(), connected_w: round2(connected), demand_w: round2(dem), demand_kva: round2(dem / ctx.pf / 1000.0) }
        })
        .collect();
    let all: Vec<&Circuit> = el.circuits.iter().collect();
    let (connected, dem) = (all.iter().map(|c| c.watts).sum::<f64>(), demand(&all));
    let main_current = dem / (ctx.phases * ctx.v * ctx.pf);
    let main_breaker = pick_ge(&ctx.breakers, main_current * ctx.margin).unwrap_or_else(|| {
        rep.warnings.push(format!("the main supply needs {:.1} A per phase, above the largest breaker you listed", main_current * ctx.margin));
        *ctx.breakers.last().expect("non-empty")
    });
    rep.electrical = Some(ElecReport {
        circuits: el.circuits,
        boards,
        connected_kva: round2(connected / ctx.pf / 1000.0),
        demand_kva: round2(dem / ctx.pf / 1000.0),
        connected_w: round2(connected),
        demand_w: round2(dem),
        main_current_a: round2(main_current),
        main_breaker_a: main_breaker,
    });
}

/// A dedicated circuit for the air-conditioning unit of every room, once the loads are known: electrical input is the
/// cooling load divided by your `hvac.cop`. Without it the circuits are not made and the report says so.
fn ac_circuits(rooms: &[RoomG], e: &Elec, ctx: &ElecCtx, h: &Hvac, rep: &mut MepReport) {
    let (Some(_), Some(hv)) = (&rep.electrical, rep.hvac.clone()) else { return };
    let Some(cop) = h.cop.filter(|c| *c > 0.0) else {
        rep.warnings.push("dedicated air-conditioning circuits not made: enter hvac.cop (cooling load / electrical input)".into());
        return;
    };
    let keys: Vec<String> = if e.boards.is_empty() { e.panel_room.iter().cloned().collect() } else { e.boards.clone() };
    let boards: Vec<(&RoomG, (i64, i64))> = keys.iter().filter_map(|k| rooms.iter().find(|r| &r.key == k)).map(|r| (r, centre(&r.rect))).collect();
    if boards.is_empty() {
        return;
    }
    let mut circuits = rep.electrical.take().map(|el| el.circuits).unwrap_or_default();
    for (i, hr) in hv.rooms.iter().enumerate() {
        let Some(room) = rooms.iter().find(|r| r.key == hr.room) else { continue };
        let (cx, cy) = centre(&room.rect);
        let id = format!("AC{}", i + 1);
        let watts = hr.load_w / cop;
        let (bi, dist) = nearest_board(&boards, cx, cy);
        let length_m = dist as f64 / 1000.0 * ctx.route;
        let (current, breaker, mm2, vd) = ctx.size(&id, watts, length_m, rep);
        circuits.push(Circuit { id: id.clone(), board: boards[bi].0.key.clone(), x_mm: cx, y_mm: cy, kind: "hvac".into(), points: hr.units as usize, rooms: vec![room.key.clone()], watts: round2(watts), current_a: round2(current), breaker_a: breaker, cable_mm2: mm2, length_m: round2(length_m), vdrop_pct: round2(vd) });
        for p in rep.points.iter_mut().filter(|p| p.role == "ac_indoor" && p.room == room.key) {
            p.circuit = Some(id.clone());
        }
    }
    rep.electrical = Some(ElecReport { circuits, boards: Vec::new(), connected_kva: 0.0, demand_kva: 0.0, connected_w: 0.0, demand_w: 0.0, main_current_a: 0.0, main_breaker_a: 0.0 });
    finish_electrical(ctx, rep);
    rep.warnings.push("dedicated air-conditioning circuits are counted at 100 % demand: no diversity value was given for them".into());
}
// ------------------------------------------------------------------ plumbing

fn plumbing(rooms: &[RoomG], p: &Plumb, rep: &mut MepReport) {
    let mut n = Need { missing: Vec::new() };
    let (sim, vmax) = (n.f("plumbing.simultaneity", p.simultaneity), n.f("plumbing.max_velocity_mps", p.max_velocity_mps));
    n.list("plumbing.catalog", p.catalog.is_empty());
    n.list("plumbing.fixtures_by_func", p.fixtures_by_func.is_empty());
    n.list("plumbing.pipes", p.pipes.is_empty());
    n.list("plumbing.drain_table", p.drain_table.is_empty());
    for fx in p.fixtures_by_func.values().flat_map(|m| m.keys()) {
        if !p.catalog.contains_key(fx) {
            n.missing.push(format!("plumbing.catalog.{fx}"));
        }
    }
    if !n.missing.is_empty() {
        rep.missing.insert("plumbing".into(), n.missing);
        return;
    }
    let mut pipes = p.pipes.clone();
    pipes.sort_by(|a, b| a.inner_mm.total_cmp(&b.inner_mm));
    let mut drains = p.drain_table.clone();
    drains.sort_by(|a, b| a.max_dfu.total_cmp(&b.max_dfu));
    let size_pipe = |q_lps: f64, rep: &mut MepReport, what: &str| -> String {
        let d_min_mm = (4.0 * (q_lps / 1000.0) / (std::f64::consts::PI * vmax)).sqrt() * 1000.0;
        match pipes.iter().find(|x| x.inner_mm >= d_min_mm - 1e-9) {
            Some(x) => x.name.clone(),
            None => {
                rep.warnings.push(format!("{what} needs an inner diameter of {d_min_mm:.1} mm, above the largest pipe you listed"));
                pipes.last().expect("non-empty").name.clone()
            }
        }
    };
    let size_drain = |dfu: f64, rep: &mut MepReport, what: &str| -> f64 {
        match drains.iter().find(|d| d.max_dfu >= dfu - 1e-9) {
            Some(d) => d.dia_mm,
            None => {
                rep.warnings.push(format!("{what} has {dfu} fixture units, above the largest row of your drain table"));
                drains.last().expect("non-empty").dia_mm
            }
        }
    };

    let (mut connected, mut total_dfu) = (0.0, 0.0);
    let mut out_rooms = Vec::new();
    for r in rooms {
        let Some(list) = p.fixtures_by_func.get(&r.func) else { continue };
        let (mut flow, mut dfu) = (0.0, 0.0);
        let total_n: usize = list.values().map(|&c| c as usize).sum();
        let spots = grid(&r.rect, total_n);
        let mut it = spots.into_iter();
        for (name, &count) in list {
            let fx = &p.catalog[name];
            for _ in 0..count {
                flow += fx.flow_lps;
                dfu += fx.dfu;
                if let Some((x, y)) = it.next() {
                    rep.points.push(PointSpec { level: r.level, role: "fixture".into(), discipline: Discipline::Plb, x, y, room: r.key.clone(), watts: None, circuit: None, label: Some(name.clone()) });
                }
            }
        }
        connected += flow;
        total_dfu += dfu;
        let pipe = size_pipe(flow * sim, rep, &format!("room '{}'", r.key));
        let drain_mm = size_drain(dfu, rep, &format!("room '{}'", r.key));
        out_rooms.push(PlumbRoom { room: r.key.clone(), flow_lps: round2(flow), pipe, dfu: round2(dfu), drain_mm });
    }
    let design = connected * sim;
    let main_pipe = size_pipe(design, rep, "the main supply");
    let main_drain_mm = size_drain(total_dfu, rep, "the building drain");
    let tank = match (p.persons, p.litres_per_person_day, p.storage_days) {
        (Some(a), Some(b), Some(c)) => Some(round2(a * b * c / 1000.0)),
        _ => {
            rep.warnings.push("storage tank not sized: persons, litres_per_person_day and storage_days are all needed".into());
            None
        }
    };
    rep.plumbing = Some(PlumbReport { rooms: out_rooms, connected_flow_lps: round2(connected), design_flow_lps: round2(design), main_pipe, total_dfu: round2(total_dfu), main_drain_mm, tank_m3: tank });
}

// ------------------------------------------------------------------ HVAC

fn hvac(model: &Model, rooms: &[RoomG], h: &Hvac, ins: &Option<InsulationLib>, rep: &mut MepReport) {
    let mut n = Need { missing: Vec::new() };
    let (tin, tout) = (n.f("hvac.indoor_c", h.indoor_c), n.f("hvac.outdoor_c", h.outdoor_c));
    let (uw, shgc, sol) = (n.f("hvac.window_u", h.window_u), n.f("hvac.window_shgc", h.window_shgc), n.f("hvac.solar_w_m2", h.solar_w_m2));
    let (pw, app, lw, ew) = (n.f("hvac.person_w", h.person_w), n.f("hvac.area_per_person_m2", h.area_per_person_m2), n.f("hvac.lights_w_m2", h.lights_w_m2), n.f("hvac.equipment_w_m2", h.equipment_w_m2));
    let (vent, safety) = (n.f("hvac.vent_l_s_per_m2", h.vent_l_s_per_m2), n.f("hvac.safety_factor", h.safety_factor));
    n.list("hvac.unit_sizes_w", h.unit_sizes_w.is_empty());
    if h.roof_exposed.is_none() {
        n.missing.push("hvac.roof_exposed".into());
    }
    let u_of = |scope: &str| ins.as_ref().and_then(|l| l.assemblies.get(scope)).and_then(|a| a.u_value().ok());
    let (wall_u, roof_u) = (u_of("wall"), u_of("roof"));
    if wall_u.is_none() {
        n.missing.push("insulation.assemblies.wall (complete, so a U-value can be computed)".into());
    }
    if roof_u.is_none() && h.roof_exposed == Some(true) {
        n.missing.push("insulation.assemblies.roof (complete, so a U-value can be computed)".into());
    }
    if !n.missing.is_empty() {
        rep.missing.insert("hvac".into(), n.missing);
        return;
    }
    let (wall_u, roof_u) = (wall_u.expect("checked"), roof_u.unwrap_or(0.0));
    let dt = tout - tin;
    let mut sizes = h.unit_sizes_w.clone();
    sizes.sort_by(f64::total_cmp);

    // Opening area per host wall, exterior wall pieces and windows per room.
    let mut open: BTreeMap<Id, f64> = BTreeMap::new();
    for e in model.elements.values().filter(|e| e.kind == "Door" || e.kind == "Window") {
        if let (Some(host), Some(w), Some(hh)) = (e.host_id, e.props.get("width").and_then(|v| v.as_f64()), e.props.get("height").and_then(|v| v.as_f64())) {
            *open.entry(host).or_default() += w * hh;
        }
    }
    let mut wall_area: BTreeMap<String, f64> = BTreeMap::new();
    for e in model.elements.values().filter(|e| e.kind == Wall::KIND) {
        if let Ok(w) = e.typed::<Wall>()
            && w.is_exterior
        {
            *wall_area.entry(w.room_a).or_default() += ((w.length * w.height) as f64 - open.get(&e.id).copied().unwrap_or(0.0)).max(0.0) / 1e6;
        }
    }
    let mut glass: BTreeMap<String, f64> = BTreeMap::new();
    for e in model.elements.values().filter(|e| e.kind == Window::KIND) {
        if let Ok(w) = e.typed::<Window>() {
            *glass.entry(w.room).or_default() += (w.width * w.height) as f64 / 1e6;
        }
    }

    let top_level = crate::building::levels(model).last().map(|(_, id)| *id);
    let mut out = Vec::new();
    let mut total = 0.0;
    for r in rooms {
        let a_wall = wall_area.get(&r.key).copied().unwrap_or(0.0);
        let a_win = glass.get(&r.key).copied().unwrap_or(0.0);
        let under_roof = top_level.is_none() || r.level == top_level;
        let conduction = a_wall * wall_u * dt + if h.roof_exposed == Some(true) && under_roof { r.area * roof_u * dt } else { 0.0 };
        let glass_w = a_win * (uw * dt + shgc * sol);
        let internal = r.area / app * pw + (lw + ew) * r.area;
        let ventilation = AIR_DENSITY_KG_M3 * AIR_CP_J_KG_K * (vent * r.area / 1000.0) * dt;
        let load = (conduction + glass_w + internal + ventilation).max(0.0) * safety;
        let (unit_w, units) = match pick_ge(&sizes, load) {
            Some(s) => (s, 1),
            None => {
                let big = *sizes.last().expect("non-empty");
                (big, (load / big).ceil() as u32)
            }
        };
        total += load;
        let (cx, cy) = centre(&r.rect);
        rep.points.push(PointSpec { level: r.level, role: "ac_indoor".into(), discipline: Discipline::Hvac, x: cx, y: cy, room: r.key.clone(), watts: Some(round2(load)), circuit: None, label: Some(format!("{units} x {unit_w} W")) });
        out.push(HvacRoom { room: r.key.clone(), conduction_w: round2(conduction), glass_w: round2(glass_w), internal_w: round2(internal), ventilation_w: round2(ventilation), load_w: round2(load), btu_hr: round2(load * BTU_HR_PER_WATT), tons: round2(load / WATTS_PER_TON_REFRIGERATION), airflow_lps: None, units, unit_w });
    }
    rep.hvac = Some(HvacReport { rooms: out, total_w: round2(total), total_btu_hr: round2(total * BTU_HR_PER_WATT), total_tons: round2(total / WATTS_PER_TON_REFRIGERATION), delta_t: dt, wall_u: round2(wall_u * 1e2) / 1.0, roof_u });
}

// ------------------------------------------------------------------ entry points

/// Computes every discipline whose inputs are complete and lists what is missing for the others.
pub fn compute(model: &Model, inputs: &MepInputs) -> MepReport {
    let mut rep = MepReport::default();
    let rooms = read_rooms(model);
    if rooms.is_empty() {
        rep.warnings.push("the model has no rooms".into());
        return rep;
    }
    if let Some(lib) = &inputs.insulation {
        let r = insulation::analyze(model, lib);
        rep.insulation = r.assemblies;
        rep.insulation_items = r.items;
        rep.warnings.extend(r.warnings);
    }
    let ctx = inputs.electrical.as_ref().and_then(|e| electrical(&rooms, e, &mut rep));
    if let Some(p) = &inputs.plumbing {
        plumbing(&rooms, p, &mut rep);
    }
    if let Some(h) = &inputs.hvac {
        hvac(model, &rooms, h, &inputs.insulation, &mut rep);
        if let (Some(e), Some(ctx)) = (&inputs.electrical, &ctx) {
            ac_circuits(&rooms, e, ctx, h, &mut rep);
        }
    }
    if let Some(r) = &inputs.routing {
        crate::routing::route(model, &rooms, inputs, r, &mut rep);
    }
    if inputs.routing.is_none() {
        rep.warnings.push("preliminary: cables, pipes and ducts are not routed; lengths are straight-line estimates".into());
    }
    if crate::building::levels(model).len() > 1 {
        rep.warnings.push("several levels: lengths are measured in plan only, so vertical risers between levels are NOT counted".into());
    }
    rep
}

fn signature(el: &crate::model::Element) -> String {
    format!("{}|{:?}|{}", el.kind, el.level_id, serde_json::Value::Object(el.props.clone()))
}

/// Replaces the model's generated MEP points with `points`, as one undoable step. Identical points are kept (and
/// keep their ids); locked points are never deleted. Returns `(created, removed)`.
pub fn apply(doc: &mut Document, points: &[PointSpec]) -> Result<(usize, usize), String> {
    apply_with_runs(doc, points, &[])
}

/// Like `apply`, and the generated ducts, drains, pipes and cable trays (`MepRun`, `Source::Generated`) are replaced by
/// `runs` in the same undoable step. Runs the user entered (`Source::User`) are never touched.
pub fn apply_with_runs(doc: &mut Document, points: &[PointSpec], runs: &[crate::routing::RunSpec]) -> Result<(usize, usize), String> {
    let mut existing: BTreeMap<String, Vec<Id>> = BTreeMap::new();
    let mut locked = 0;
    let mut all: Vec<Id> = Vec::new();
    for e in doc.model.elements.values().filter(|e| (e.kind == MepPoint::KIND || e.kind == crate::typed::MepRun::KIND) && e.provenance.source == Source::Generated) {
        if e.provenance.locked {
            locked += 1;
            continue;
        }
        existing.entry(signature(e)).or_default().push(e.id);
        all.push(e.id);
    }
    let _ = locked;
    let mut keep: Vec<Id> = Vec::new();
    let mut creates: Vec<Command> = Vec::new();
    for p in points {
        let mut el = element_of(
            &MepPoint { role: p.role.clone(), x: p.x, y: p.y, room: p.room.clone(), watts: p.watts, circuit: p.circuit.clone(), label: p.label.clone(), extra: Default::default() },
            Source::Generated,
        );
        el.discipline = p.discipline;
        el.level_id = p.level;
        match existing.get_mut(&signature(&el)).and_then(|v| v.pop()) {
            Some(id) => keep.push(id),
            None => creates.push(Command::Create(el)),
        }
    }
    for run in runs {
        let spec = crate::typed::MepRun {
            system: run.system.clone(),
            x1: run.p1[0],
            y1: run.p1[1],
            z1: run.p1[2],
            x2: run.p2[0],
            y2: run.p2[1],
            z2: run.p2[2],
            width: run.width,
            height: run.height,
            room: run.room.clone(),
            role: Some(run.role.clone()),
            label: run.label.clone(),
            extra: Default::default(),
        };
        let el = crate::typed::run_element(&spec, run.level, Source::Generated)?;
        match existing.get_mut(&signature(&el)).and_then(|v| v.pop()) {
            Some(id) => keep.push(id),
            None => creates.push(Command::Create(el)),
        }
    }
    let mut cmds: Vec<Command> = all.iter().filter(|id| !keep.contains(id)).map(|id| Command::Delete(*id)).collect();
    let removed = cmds.len();
    let created = creates.len();
    cmds.extend(creates);
    if !cmds.is_empty() {
        doc.commit("MEP points", crate::command::Origin::User, cmds).map_err(|e| e.to_string())?;
    }
    Ok((created, removed))
}

/// Counts of MEP points by role and label, as quantity items that cite the points.
pub fn point_items(model: &Model) -> Vec<QtoItem> {
    let mut groups: BTreeMap<String, Vec<Id>> = BTreeMap::new();
    for e in model.elements.values().filter(|e| e.kind == MepPoint::KIND) {
        if let Ok(p) = e.typed::<MepPoint>() {
            let code = match (&p.role[..], &p.label) {
                ("fixture", Some(l)) => format!("MEP.fixture.{l}"),
                (role, _) => format!("MEP.{role}"),
            };
            groups.entry(code).or_default().push(e.id);
        }
    }
    let mut items: Vec<QtoItem> = groups
        .into_iter()
        .map(|(code, ids)| QtoItem { description: format!("{code} points"), code, unit: crate::qto::Unit::Nr, quantity: ids.len() as f64, waste_pct: None, sources: ids, parts: Vec::new() })
        .collect();
    // Runs by system and section, as lengths.
    let mut runs: BTreeMap<String, (f64, Vec<Id>)> = BTreeMap::new();
    for e in model.elements.values().filter(|e| e.kind == crate::typed::MepRun::KIND) {
        if let Ok(r) = e.typed::<crate::typed::MepRun>() {
            let d = [(r.x2 - r.x1) as f64, (r.y2 - r.y1) as f64, (r.z2 - r.z1) as f64];
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() / 1000.0;
            let slot = runs.entry(format!("MEP.run.{}.{}x{}", r.system, r.width, r.height)).or_default();
            slot.0 += len;
            slot.1.push(e.id);
        }
    }
    items.extend(runs.into_iter().map(|(code, (len, ids))| QtoItem { description: format!("{code} runs"), code, unit: crate::qto::Unit::M, quantity: (len * 100.0).round() / 100.0, waste_pct: None, sources: ids, parts: Vec::new() }));
    items
}
