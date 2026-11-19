//! EV charger circuits, the main distribution board's demand with them, three-phase balancing and voltage drop.
//!
//! All parameters come from the electrical inputs of the MEP pack (`voltage_v`, `power_factor`, `three_phase`, `breaker_sizes_a`,
//! `breaker_margin`, `cables`, `resistivity`, `max_vdrop_pct`, `route_factor`, the boards) and from your EV and balance inputs; nothing is
//! assumed. The mechanics: single-phase power `V I`, three-phase power `sqrt(3) V_ll I` with `V_ll = sqrt(3) V`; voltage drop
//! `2 rho L I / A` (single-phase) and `sqrt(3) rho L I / A` (three-phase) divided by the voltage; a breaker is the smallest size not below
//! the design current; a cable the smallest section whose ampacity is not below the breaker and whose drop is within the limit.
//!
//! Balancing: the single-phase circuits of each board (and the single-phase chargers) are distributed over L1, L2 and L3, the largest
//! first onto the least loaded phase; three-phase chargers load the three phases equally. The imbalance is
//! `(largest phase current - mean) / mean`; the limit is yours. With a single-phase supply there is nothing to balance.

use crate::mep::{self, Elec, MepInputs, MepReport};
use crate::mepx::{BalanceIn, EvIn, r2};
use crate::model::Model;
use serde::Serialize;

const SQRT3: f64 = 1.732_050_807_568_877_2;

#[derive(Debug, Clone, Serialize)]
pub struct EvRow {
    pub name: String,
    pub phases: u32,
    pub current_a: f64,
    pub load_kw: f64,
    pub design_a: f64,
    pub breaker_a: Option<f64>,
    pub cable_mm2: Option<f64>,
    pub length_m: Option<f64>,
    pub vdrop_pct: Option<f64>,
    /// `ok`, `no breaker`, `no cable` or `incomplete`.
    pub status: &'static str,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct EvReport {
    pub chargers: Vec<EvRow>,
    pub connected_kw: f64,
    pub demand_kw: f64,
    pub main_before_kva: Option<f64>,
    pub main_after_kva: Option<f64>,
    pub main_before_a: Option<f64>,
    pub main_after_a: Option<f64>,
    pub breaker_before_a: Option<f64>,
    pub breaker_after_a: Option<f64>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
}

fn need<T: Copy>(missing: &mut Vec<String>, v: Option<T>, name: &str) -> Option<T> {
    if v.is_none() {
        missing.push(name.to_string());
    }
    v
}

fn main_current(w: f64, pf: f64, v: f64, three: bool) -> (f64, f64) {
    let kva = w / pf / 1000.0;
    (kva, kva * 1000.0 / if three { 3.0 * v } else { v })
}

fn breaker_for(sizes: &[f64], i: f64, margin: f64) -> Option<f64> {
    sizes.iter().copied().filter(|b| *b + 1e-9 >= i * margin).fold(None, |a: Option<f64>, b| Some(a.map_or(b, |x| x.min(b))))
}

pub fn ev(model: &Model, mep_in: &MepInputs, rep: &MepReport, inp: &EvIn) -> EvReport {
    let mut out = EvReport::default();
    let Some(el) = mep_in.electrical.as_ref() else {
        out.missing.push("electrical inputs (the MEP tab): voltage, power factor, breakers, cables, resistivity".into());
        return out;
    };
    let mut miss = Vec::new();
    let v = need(&mut miss, el.voltage_v, "electrical.voltage_v");
    let pf = need(&mut miss, el.power_factor, "electrical.power_factor");
    let rho = need(&mut miss, el.resistivity, "electrical.resistivity");
    let maxvd = need(&mut miss, el.max_vdrop_pct, "electrical.max_vdrop_pct");
    let margin = need(&mut miss, el.breaker_margin, "electrical.breaker_margin");
    let rf = need(&mut miss, el.route_factor, "electrical.route_factor");
    if el.breaker_sizes_a.is_empty() {
        miss.push("electrical.breaker_sizes_a".into());
    }
    if el.cables.is_empty() {
        miss.push("electrical.cables".into());
    }
    let cont = need(&mut miss, inp.continuous_factor, "ev.continuous_factor");
    let dem = need(&mut miss, inp.demand_factor, "ev.demand_factor");
    out.missing = miss;
    let (Some(v), Some(pf), Some(rho), Some(maxvd), Some(margin), Some(rf), Some(cont), Some(dem)) = (v, pf, rho, maxvd, margin, rf, cont, dem) else { return out };
    if el.breaker_sizes_a.is_empty() || el.cables.is_empty() {
        return out;
    }
    let three_supply = el.three_phase == Some(true);
    let rooms = mep::read_rooms(model);
    let board_key = inp.board_room.clone().or_else(|| el.boards.first().cloned()).or_else(|| el.panel_room.clone());
    let board = board_key.as_deref().and_then(|k| rooms.iter().find(|r| r.key == k)).map(|r| mep::centre(&r.rect));
    let mut cables = el.cables.clone();
    cables.sort_by(|a, b| a.mm2.total_cmp(&b.mm2));
    let mut total_w = 0.0;
    for c in &inp.chargers {
        let (power_w, vll) = if c.phases == 3 { (SQRT3 * SQRT3 * v * c.current_a, SQRT3 * v) } else { (v * c.current_a, v) };
        total_w += power_w;
        let design = c.current_a * cont;
        let length = c.length_m.or_else(|| match (board, c.x_mm, c.y_mm) {
            (Some(b), Some(x), Some(y)) => Some(((x - b.0 as f64).abs() + (y - b.1 as f64).abs()) / 1000.0 * rf),
            _ => None,
        });
        let breaker = breaker_for(&el.breaker_sizes_a, design, 1.0);
        let mut row = EvRow { name: c.name.clone(), phases: c.phases, current_a: c.current_a, load_kw: r2(power_w / 1000.0), design_a: r2(design), breaker_a: breaker, cable_mm2: None, length_m: length.map(r2), vdrop_pct: None, status: "incomplete" };
        match (breaker, length) {
            (None, _) => row.status = "no breaker",
            (Some(b), Some(l)) => {
                let vd = |a: f64| (if c.phases == 3 { SQRT3 } else { 2.0 }) * rho * l * c.current_a / a / vll * 100.0;
                match cables.iter().find(|k| k.amps + 1e-9 >= b && vd(k.mm2) <= maxvd + 1e-9) {
                    Some(k) => {
                        row.cable_mm2 = Some(k.mm2);
                        row.vdrop_pct = Some(r2(vd(k.mm2)));
                        row.status = "ok";
                    }
                    None => row.status = "no cable",
                }
            }
            (Some(_), None) => out.missing.push(format!("charger '{}': a position (x, y) or a cable length, and the board's room", c.name)),
        }
        out.chargers.push(row);
    }
    out.connected_kw = r2(total_w / 1000.0);
    let ev_demand = total_w * dem;
    out.demand_kw = r2(ev_demand / 1000.0);
    if let Some(e) = &rep.electrical {
        let before_w = e.demand_w;
        let (k0, i0) = main_current(before_w, pf, v, three_supply);
        let (k1, i1) = main_current(before_w + ev_demand, pf, v, three_supply);
        out.main_before_kva = Some(r2(k0));
        out.main_after_kva = Some(r2(k1));
        out.main_before_a = Some(r2(i0));
        out.main_after_a = Some(r2(i1));
        out.breaker_before_a = breaker_for(&el.breaker_sizes_a, i0, margin);
        out.breaker_after_a = breaker_for(&el.breaker_sizes_a, i1, margin);
        if out.breaker_after_a.is_none() {
            out.notes.push("no breaker in your list is large enough for the main with the chargers".into());
        }
    } else {
        out.missing.push("the electrical report (compute the MEP loads first) to add the chargers to the main board's demand".into());
    }
    out.notes.push("chargers are added to the main demand at your demand factor; the supply's capacity and the utility's rules are not checked".into());
    out
}

// ------------------------------------------------------------------ balancing and voltage drop

#[derive(Debug, Clone, Serialize)]
pub struct Assign {
    pub circuit: String,
    pub current_a: f64,
    pub phase: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct BoardBalance {
    pub board: String,
    pub phase_a: [f64; 3],
    pub imbalance_pct: f64,
    pub ok: Option<bool>,
    pub assignments: Vec<Assign>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DropRow {
    pub circuit: String,
    pub board: String,
    pub branch_pct: f64,
    pub feeder_pct: Option<f64>,
    pub total_pct: Option<f64>,
    /// `ok`, `branch over limit`, `total over limit` or `incomplete`.
    pub status: &'static str,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct BalanceReport {
    pub boards: Vec<BoardBalance>,
    pub drops: Vec<DropRow>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
}

pub fn balance(mep_in: &MepInputs, rep: &MepReport, ev_in: &EvIn, inp: &BalanceIn) -> BalanceReport {
    let mut out = BalanceReport::default();
    let (Some(el), Some(er)) = (mep_in.electrical.as_ref(), rep.electrical.as_ref()) else {
        out.missing.push("the electrical report (the MEP tab): circuits with their currents".into());
        return out;
    };
    let three = el.three_phase == Some(true);
    if !three {
        out.notes.push("the supply is single-phase (electrical.three_phase is not true): nothing to balance".into());
    } else {
        if inp.max_imbalance_pct.is_none() {
            out.missing.push("balance.max_imbalance_pct".into());
        }
        let ev_board = ev_in.board_room.clone().or_else(|| el.boards.first().cloned()).or_else(|| el.panel_room.clone()).unwrap_or_default();
        let mut boards: Vec<String> = er.circuits.iter().map(|c| c.board.clone()).collect();
        boards.sort();
        boards.dedup();
        for b in boards {
            let mut items: Vec<(String, f64)> = er.circuits.iter().filter(|c| c.board == b).map(|c| (c.id.clone(), c.current_a)).collect();
            let mut equal = 0.0;
            if b == ev_board {
                for c in &ev_in.chargers {
                    if c.phases == 3 {
                        equal += c.current_a;
                    } else {
                        items.push((format!("EV {}", c.name), c.current_a));
                    }
                }
            }
            items.sort_by(|a, c| c.1.total_cmp(&a.1).then(a.0.cmp(&c.0)));
            let mut ph = [equal; 3];
            let mut assignments = Vec::new();
            for (id, i) in items {
                let k = (0..3).min_by(|a, c| ph[*a].total_cmp(&ph[*c])).unwrap_or(0);
                ph[k] += i;
                assignments.push(Assign { circuit: id, current_a: r2(i), phase: ["L1", "L2", "L3"][k] });
            }
            let mean = ph.iter().sum::<f64>() / 3.0;
            let imb = if mean > 0.0 { (ph.iter().cloned().fold(0.0, f64::max) - mean) / mean * 100.0 } else { 0.0 };
            out.boards.push(BoardBalance { board: b, phase_a: [r2(ph[0]), r2(ph[1]), r2(ph[2])], imbalance_pct: r2(imb), ok: inp.max_imbalance_pct.map(|m| imb <= m + 1e-9), assignments });
        }
        out.notes.push("balanced on the connected currents of the circuits (not the demand), the largest first onto the least loaded phase".into());
    }
    // Voltage drop: the branch circuit plus the feeder to its board.
    let (Some(v), Some(pf), Some(rho)) = (el.voltage_v, el.power_factor, el.resistivity) else {
        out.missing.push("electrical.voltage_v, power_factor and resistivity for the voltage drop".into());
        return out;
    };
    for (n, ok) in [("balance.feeder_length_m", inp.feeder_length_m.is_some()), ("balance.feeder_mm2", inp.feeder_mm2.is_some()), ("balance.total_vdrop_max_pct", inp.total_vdrop_max_pct.is_some())] {
        if !ok {
            out.missing.push(n.to_string());
        }
    }
    let feeder = |board: &str| -> Option<f64> {
        let (l, a) = (inp.feeder_length_m?, inp.feeder_mm2?);
        let w = er.boards.iter().find(|b| b.room == board)?.demand_w;
        let (_, i) = main_current(w, pf, v, three);
        Some(if three { rho * l * i / a / v * 100.0 } else { 2.0 * rho * l * i / a / v * 100.0 })
    };
    for c in &er.circuits {
        let f = feeder(&c.board);
        let total = f.map(|f| f + c.vdrop_pct);
        let status = match (total, inp.total_vdrop_max_pct, el.max_vdrop_pct) {
            (_, _, Some(m)) if c.vdrop_pct > m + 1e-9 => "branch over limit",
            (Some(t), Some(m), _) if t > m + 1e-9 => "total over limit",
            (Some(_), Some(_), _) => "ok",
            _ => "incomplete",
        };
        out.drops.push(DropRow { circuit: c.id.clone(), board: c.board.clone(), branch_pct: r2(c.vdrop_pct), feeder_pct: f.map(r2), total_pct: total.map(r2), status });
    }
    out
}

/// Elec is re-exported for the page's field list.
pub type ElecInputs = Elec;
