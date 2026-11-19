//! Greywater recycling: which fixtures discharge greywater (your list), the greywater and soil load of every room in drainage fixture
//! units, the daily volume, the drains that carry it to a treatment unit, the storage and the surplus.
//!
//! - **Separation**: a room's fixtures (from `plumbing.fixtures_by_func`) split into the greywater fixtures you name (lavatory, shower,
//!   washing machine...) and the rest, which stays on the soil (SOIL) drain; each load is sized by your drain table from its own DFU.
//!   The existing drain router sizes ONE drain per room from the total DFU: `soil_drain_mm` is what the soil drain can be once the
//!   greywater leaves it.
//! - **Volume**: litres per day for each fixture name you enter (and the condensate of the air-conditioning units, if you count it,
//!   at your litres per unit). The reuse demand and the storage days are yours; the storage is `days x min(supply, demand)`.
//! - **Runs**: from the centre of each ground-floor room with greywater, along x then y, to the treatment unit you place, falling by the
//!   slope and starting at the invert depth of the routing inputs; system `drain`, role `grey`.

use crate::mep::{self, MepInputs, MepReport};
use crate::mepx::{GreyIn, r2};
use crate::model::Model;
use crate::routing::RunSpec;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct GreyRoom {
    pub room: String,
    pub grey_dfu: f64,
    pub soil_dfu: f64,
    pub grey_drain_mm: Option<f64>,
    pub soil_drain_mm: Option<f64>,
    pub grey_l_day: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct GreyReport {
    pub rooms: Vec<GreyRoom>,
    pub grey_dfu: f64,
    pub soil_dfu: f64,
    pub fixtures_l_day: f64,
    pub condensate_l_day: f64,
    pub supply_l_day: f64,
    pub reuse_demand_l_day: Option<f64>,
    pub surplus_l_day: Option<f64>,
    pub storage_m3: Option<f64>,
    pub unit: Option<[f64; 2]>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
    #[serde(skip)]
    pub runs: Vec<RunSpec>,
}

fn drain_dia(table: &[crate::mep::DrainRow], dfu: f64) -> Option<f64> {
    let mut t: Vec<&crate::mep::DrainRow> = table.iter().collect();
    t.sort_by(|a, b| a.max_dfu.total_cmp(&b.max_dfu));
    t.iter().find(|r| r.max_dfu + 1e-9 >= dfu).map(|r| r.dia_mm)
}

pub fn design(model: &Model, mep_in: &MepInputs, rep: &MepReport, inp: &GreyIn) -> GreyReport {
    let mut out = GreyReport::default();
    let Some(pl) = mep_in.plumbing.as_ref() else {
        out.missing.push("plumbing inputs (the MEP tab): the fixture catalog, the fixtures per room, the drain table".into());
        return out;
    };
    if inp.fixtures.is_empty() {
        out.missing.push("grey.fixtures: which fixtures discharge greywater".into());
        return out;
    }
    if pl.drain_table.is_empty() {
        out.missing.push("plumbing.drain_table".into());
    }
    let rooms = mep::read_rooms(model);
    let lowest = crate::building::levels(model).first().map(|(_, id)| *id);
    let mut missing_l: Vec<String> = Vec::new();
    for r in &rooms {
        let Some(fx) = pl.fixtures_by_func.get(&r.func) else { continue };
        let (mut grey, mut soil, mut litres) = (0.0, 0.0, 0.0);
        for (name, count) in fx {
            let dfu = pl.catalog.get(name).map_or(0.0, |f| f.dfu) * *count as f64;
            if inp.fixtures.contains(name) {
                grey += dfu;
                match inp.fixture_l_day.get(name) {
                    Some(l) => litres += l * *count as f64,
                    None => {
                        if !missing_l.contains(name) {
                            missing_l.push(name.clone());
                        }
                    }
                }
            } else {
                soil += dfu;
            }
        }
        if grey == 0.0 && soil == 0.0 {
            continue;
        }
        out.grey_dfu += grey;
        out.soil_dfu += soil;
        out.fixtures_l_day += litres;
        out.rooms.push(GreyRoom { room: r.key.clone(), grey_dfu: grey, soil_dfu: soil, grey_drain_mm: (grey > 0.0).then(|| drain_dia(&pl.drain_table, grey)).flatten(), soil_drain_mm: (soil > 0.0).then(|| drain_dia(&pl.drain_table, soil)).flatten(), grey_l_day: r2(litres) });
    }
    if !missing_l.is_empty() {
        out.missing.push(format!("grey.fixture_l_day for: {}", missing_l.join(", ")));
    }
    if inp.condensate {
        match (inp.condensate_l_day_per_unit, rep.hvac.as_ref()) {
            (Some(l), Some(h)) => out.condensate_l_day = r2(h.rooms.iter().map(|r| r.units as f64).sum::<f64>() * l),
            (None, _) => out.missing.push("grey.condensate_l_day_per_unit".into()),
            (_, None) => out.missing.push("the HVAC report (units per room) for the condensate".into()),
        }
    }
    out.supply_l_day = r2(out.fixtures_l_day + out.condensate_l_day);
    out.reuse_demand_l_day = inp.reuse_demand_l_day;
    if let Some(d) = inp.reuse_demand_l_day {
        out.surplus_l_day = Some(r2(out.supply_l_day - d));
        match inp.storage_days {
            Some(days) => out.storage_m3 = Some(r2(days * out.supply_l_day.min(d) / 1000.0)),
            None => out.missing.push("grey.storage_days".into()),
        }
    } else {
        out.missing.push("grey.reuse_demand_l_day (what the treated water is used for)".into());
    }
    out.rooms.sort_by(|a, b| a.room.cmp(&b.room));
    // Runs to the treatment unit.
    match (inp.unit_x_mm, inp.unit_y_mm) {
        (Some(ux), Some(uy)) => {
            out.unit = Some([ux, uy]);
            let r = mep_in.routing.as_ref();
            match (r.and_then(|r| r.drain_slope_pct), r.and_then(|r| r.drain_invert_mm)) {
                (Some(slope), Some(invert)) if slope > 0.0 => {
                    for gr in out.rooms.iter().filter(|g| g.grey_dfu > 0.0) {
                        let Some(room) = rooms.iter().find(|x| x.key == gr.room) else { continue };
                        if room.level != lowest {
                            out.notes.push(format!("greywater of room '{}' is on an upper level: a vertical stack is needed and is not generated", room.key));
                            continue;
                        }
                        let Some(dia) = gr.grey_drain_mm else {
                            out.notes.push(format!("room '{}': the drain table has no size for {} DFU of greywater", room.key, gr.grey_dfu));
                            continue;
                        };
                        let (cx, cy) = mep::centre(&room.rect);
                        let (ux, uy) = (ux.round() as i64, uy.round() as i64);
                        let z0 = -(invert.round() as i64);
                        let fall = |len: i64| (len as f64 * slope / 100.0).round() as i64;
                        let (l1, l2) = ((ux - cx).abs(), (uy - cy).abs());
                        let corner = [ux, cy];
                        let z1 = z0 - fall(l1);
                        let z2 = z1 - fall(l2);
                        let d = dia.round() as i64;
                        let mk = |a: [i64; 3], b: [i64; 3]| RunSpec { level: room.level, system: "drain".into(), role: "grey".into(), room: Some(room.key.clone()), label: Some(format!("{d} mm greywater drain to the treatment unit")), p1: a, p2: b, width: d, height: d };
                        if l1 > 0 {
                            out.runs.push(mk([cx, cy, z0], [corner[0], corner[1], z1]));
                        }
                        if l2 > 0 {
                            out.runs.push(mk([corner[0], corner[1], z1], [ux, uy, z2]));
                        }
                    }
                }
                _ => out.missing.push("routing.drain_slope_pct and routing.drain_invert_mm (the MEP routing inputs) for the greywater drains".into()),
            }
        }
        _ => out.missing.push("grey.unit_x_mm and unit_y_mm: where the treatment unit stands".into()),
    }
    out.notes.push("the soil drain sizes above are for the soil fixtures alone; the router's existing drain of a room is sized on its total DFU and can be reduced to them".into());
    out
}
