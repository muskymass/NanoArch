//! Plumbing head loss and booster pump sizing for the supply network.
//!
//! The critical path runs from the tank to the outlet of the farthest room: a plan run (the Manhattan distance from the tank to the room
//! centre, the last leg on the room's branch pipe, the rest on the main pipe) and a vertical riser. The pipes are those the plumbing
//! inputs chose (`plumbing.pipes`, inner diameters) for the main design flow and for the room's flow.
//!
//! - **Friction**: Hazen-Williams, `hf = 10.67 L Q^1.852 / (C^1.852 D^4.87)` (SI: metres, m3/s): a textbook empirical formula whose
//!   roughness coefficient `C` is YOURS. Fittings add your percentage to the length.
//! - **Head**: `TDH = static + friction + residual - inlet`, all in metres of water; the static head is from the source level to the
//!   outlet level (derived from the tank and the top level when you do not give them). A TDH of zero or less needs no pump.
//! - **Pump**: hydraulic power `rho g Q H` (rho = 1000 kg/m3, g = 9.81 m/s2: physics), motor power = that / your efficiency times
//!   (1 + your margin); a standby pump is counted if you ask.
//! - The velocity in the main pipe is compared with the maximum of the plumbing inputs.

use crate::mep::{self, MepInputs, MepReport};
use crate::mepx::{HydraIn, r2};
use crate::model::Model;
use serde::Serialize;
use std::f64::consts::PI;

const RHO: f64 = 1000.0;
const G: f64 = 9.81;

#[derive(Debug, Clone, Serialize)]
pub struct Segment {
    pub what: String,
    pub pipe: String,
    pub inner_mm: f64,
    pub length_m: f64,
    pub flow_lps: f64,
    pub velocity_mps: f64,
    pub head_loss_m: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct HydraReport {
    pub room: Option<String>,
    pub segments: Vec<Segment>,
    pub friction_m: f64,
    pub fittings_m: f64,
    pub static_m: f64,
    pub residual_m: f64,
    pub inlet_m: f64,
    pub tdh_m: Option<f64>,
    pub needs_pump: Option<bool>,
    pub flow_lps: f64,
    pub hydraulic_kw: Option<f64>,
    pub motor_kw: Option<f64>,
    pub pumps: Option<u32>,
    pub velocity_ok: Option<bool>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
}

/// Head loss in metres of `l` metres of pipe of `d` mm carrying `q` L/s (Hazen-Williams).
pub fn hazen_williams_m(l_m: f64, q_lps: f64, d_mm: f64, c: f64) -> f64 {
    let (q, d) = (q_lps / 1000.0, d_mm / 1000.0);
    10.67 * l_m * q.powf(1.852) / (c.powf(1.852) * d.powf(4.87))
}

pub fn size(model: &Model, mep_in: &MepInputs, rep: &MepReport, inp: &HydraIn) -> HydraReport {
    let mut out = HydraReport::default();
    let (Some(pl), Some(pr), Some(rt)) = (mep_in.plumbing.as_ref(), rep.plumbing.as_ref(), mep_in.routing.as_ref()) else {
        out.missing.push("plumbing inputs and report, and the routing tank position (the MEP tab)".into());
        return out;
    };
    let Some(tank) = rt.tank.as_ref() else {
        out.missing.push("routing.tank: where the water tank stands".into());
        return out;
    };
    for (n, ok) in [("hydra.hazen_c", inp.hazen_c.is_some()), ("hydra.fittings_pct", inp.fittings_pct.is_some()), ("hydra.residual_head_m", inp.residual_head_m.is_some()), ("hydra.pump_efficiency", inp.pump_efficiency.is_some()), ("hydra.motor_margin", inp.motor_margin.is_some())] {
        if !ok {
            out.missing.push(n.to_string());
        }
    }
    let rooms = mep::read_rooms(model);
    // The farthest room with a supply.
    let cand: Vec<&crate::mep::PlumbRoom> = pr.rooms.iter().filter(|r| r.flow_lps > 0.0).collect();
    let dist = |k: &str| rooms.iter().find(|r| r.key == k).map(|r| {
        let c = mep::centre(&r.rect);
        (c.0 as f64 - tank.x_mm).abs() + (c.1 as f64 - tank.y_mm).abs()
    });
    let far = match inp.farthest_room.as_deref() {
        Some(k) => cand.iter().find(|r| r.room == k).copied(),
        None => cand.iter().copied().max_by(|a, b| dist(&a.room).unwrap_or(0.0).total_cmp(&dist(&b.room).unwrap_or(0.0))),
    };
    let Some(far) = far else {
        out.missing.push("a room with fixtures to supply".into());
        return out;
    };
    out.room = Some(far.room.clone());
    let Some(room) = rooms.iter().find(|r| r.key == far.room) else { return out };
    let (cx, cy) = mep::centre(&room.rect);
    let (dx, dy) = ((cx as f64 - tank.x_mm).abs() / 1000.0, (cy as f64 - tank.y_mm).abs() / 1000.0);
    // Levels: heights from the 3D scene.
    let scene = crate::model3d::build(model);
    let top_z = scene.levels.iter().map(|l| l.z0).fold(0.0, f64::max) * 1000.0;
    let supply_z = rt.supply_z_mm.unwrap_or(0.0);
    let source_z = inp.source_z_mm.unwrap_or(if tank.kind == "ground" { tank.z_mm } else { 0.0 });
    let outlet_z = match (inp.outlet_z_mm, tank.kind.as_str()) {
        (Some(z), _) => Some(z),
        (None, "ground") => Some(top_z + supply_z),
        _ => None,
    };
    let Some(outlet_z) = outlet_z else {
        out.missing.push("hydra.outlet_z_mm: the height the pump must reach (the roof tank's inlet)".into());
        return out;
    };
    out.static_m = r2((outlet_z - source_z) / 1000.0);
    let pipe = |name: &str| pl.pipes.iter().find(|p| p.name == name).map(|p| p.inner_mm);
    let (Some(c), Some(fit)) = (inp.hazen_c, inp.fittings_pct) else { return out };
    let main_d = pipe(&pr.main_pipe);
    let branch_d = pipe(&far.pipe);
    let (Some(main_d), Some(branch_d)) = (main_d, branch_d) else {
        out.missing.push("plumbing.pipes: the main and the branch pipe sizes the plumbing report chose".into());
        return out;
    };
    let vel = |q: f64, d: f64| (q / 1000.0) / (PI / 4.0 * (d / 1000.0).powi(2));
    let rise = ((outlet_z - source_z) / 1000.0).abs();
    let main_plan = dx.max(0.0);
    let segs = [("main (plan)", pr.main_pipe.as_str(), main_d, main_plan, pr.design_flow_lps), ("riser", pr.main_pipe.as_str(), main_d, rise, pr.design_flow_lps), ("branch to the room", far.pipe.as_str(), branch_d, dy, far.flow_lps)];
    let mut fric = 0.0;
    for (what, p, d, l, q) in segs {
        let hf = hazen_williams_m(l, q, d, c);
        fric += hf;
        out.segments.push(Segment { what: what.into(), pipe: p.into(), inner_mm: d, length_m: r2(l), flow_lps: r2(q), velocity_mps: r2(vel(q, d)), head_loss_m: r2(hf) });
    }
    out.friction_m = r2(fric);
    out.fittings_m = r2(fric * fit / 100.0);
    out.flow_lps = r2(pr.design_flow_lps);
    out.velocity_ok = pl.max_velocity_mps.map(|m| out.segments.iter().all(|s| s.velocity_mps <= m + 1e-9));
    if out.velocity_ok == Some(false) {
        out.notes.push("a segment's velocity exceeds the plumbing maximum: choose a larger pipe".into());
    }
    out.residual_m = inp.residual_head_m.unwrap_or(0.0);
    out.inlet_m = inp.inlet_head_m.unwrap_or(0.0);
    if inp.inlet_head_m.is_none() {
        out.notes.push("no inlet pressure entered: 0 m at the pump inlet".into());
    }
    if inp.residual_head_m.is_none() {
        return out;
    }
    let tdh = out.static_m + fric * (1.0 + fit / 100.0) + out.residual_m - out.inlet_m;
    out.tdh_m = Some(r2(tdh));
    out.needs_pump = Some(tdh > 0.0);
    if let (Some(eff), Some(margin)) = (inp.pump_efficiency, inp.motor_margin)
        && tdh > 0.0
    {
        let hyd = RHO * G * (pr.design_flow_lps / 1000.0) * tdh / 1000.0;
        out.hydraulic_kw = Some(r2(hyd));
        out.motor_kw = Some(r2(hyd / eff * (1.0 + margin)));
        out.pumps = Some(1 + u32::from(inp.standby));
    }
    out.notes.push("a preliminary hydraulic estimate on the critical path: select the pump from the manufacturer's curve at this flow and head".into());
    out
}
