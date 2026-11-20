//! The weighted thermal envelope: the area-weighted U-value of the exterior walls (net of openings), the roof and the windows, set
//! against the limits of the project's climate zone, and an efficiency band.
//!
//! - **Limits**: the zone limits of `presets::ZONES` (SBC 602 Table 5.2 as reproduced in the contractor's guide, pp.172-174), WITH
//!   the guide's own conflict about its column labels, repeated in every report (`presets::ZONE_CAVEAT_EN`): confirm against SBC 602.
//! - **Reference U**: the same areas at the zone's limits. The weighted U of the building divided by it is the ratio the band uses.
//! - **Bands A to F**: NOT an SBC or SEC rating. They are this application's relative scale on that ratio; the five upper bounds of
//!   A to E are an input (`thermalInputs.bandRatios`) and without them no band is given.
//! - **Doors** are left out of the weighted U (no limit is recorded for them); their area is reported.
//! - Layers come from the materials catalog (`materials`) or from a conductivity you enter; a missing value is named.

use crate::compliance::{LayerPick, ThermalInputs};
use crate::materials;
use crate::model::Model;
use crate::presets;
use crate::project::NanoArchProject;
use crate::typed::Wall;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ClassRow {
    pub class: &'static str,
    pub area_m2: f64,
    pub u: Option<f64>,
    pub limit: Option<f64>,
    /// `ok`, `exceeds limit`, `incomplete` or `no limit`.
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ThermalReport {
    pub zone: Option<u8>,
    pub rows: Vec<ClassRow>,
    pub weighted_u: Option<f64>,
    pub reference_u: Option<f64>,
    pub ratio: Option<f64>,
    pub band: Option<&'static str>,
    pub glazing_share_pct: f64,
    pub door_area_m2: f64,
    pub shgc: Option<f64>,
    pub shgc_limit: Option<f64>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
    pub caveat_en: &'static str,
    pub caveat_ar: &'static str,
    pub limits_source: String,
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// U of a layered assembly: `1 / (Rsi + sum(t / lambda) + Rse)`. `Err` names everything missing.
pub fn assembly_u(layers: &[LayerPick], rsi: Option<f64>, rse: Option<f64>, what: &str) -> Result<f64, Vec<String>> {
    let mut missing = Vec::new();
    if layers.is_empty() {
        missing.push(format!("{what}: no layers"));
    }
    if rsi.is_none() {
        missing.push("surface resistance Rsi".into());
    }
    if rse.is_none() {
        missing.push("surface resistance Rse".into());
    }
    let mut r = rsi.unwrap_or(0.0) + rse.unwrap_or(0.0);
    for l in layers {
        let lam = l.lambda.or_else(|| materials::find(&l.material).and_then(|m| m.lambda));
        match (l.thickness_mm, lam) {
            (Some(t), Some(lam)) if lam > 0.0 => r += t / 1000.0 / lam,
            (None, _) => missing.push(format!("{what}: thickness of '{}'", l.material)),
            (_, None) => missing.push(format!("{what}: conductivity (lambda) of '{}'", l.material)),
            _ => missing.push(format!("{what}: conductivity (lambda) of '{}'", l.material)),
        }
    }
    if missing.is_empty() { Ok(1.0 / r) } else { Err(missing) }
}

fn opening_area(model: &Model, kind: &str) -> f64 {
    model
        .elements
        .values()
        .filter(|e| e.kind == kind)
        .filter(|e| e.host_id.and_then(|h| model.get(&h)).and_then(|h| h.typed::<Wall>().ok()).is_some_and(|w| w.is_exterior))
        .filter_map(|e| Some(e.props.get("width")?.as_f64()? * e.props.get("height")?.as_f64()? / 1e6))
        .sum()
}

/// The U-value of the exterior walls. With no wall assembly in use this is the project's single wall build-up. Otherwise the exterior
/// walls are grouped by the assembly they use (walls with none keep the single build-up) and the groups are weighted by net area.
fn wall_envelope_u(model: &Model, t: &ThermalInputs, missing: &mut Vec<String>, notes: &mut Vec<String>) -> Option<f64> {
    let resolver = crate::assembly::Resolver::of(model);
    let walls = crate::insulation::exterior_wall_areas(model);
    // (assembly id or none) -> area
    let mut groups: Vec<(Option<&crate::assembly::WallAssembly>, f64)> = Vec::new();
    for (id, area) in &walls {
        let a = model.get(id).and_then(|e| Some(resolver.of_wall(e, &e.typed::<Wall>().ok()?))).flatten();
        match groups.iter_mut().find(|(g, _)| g.map(|x| &x.id) == a.map(|x| &x.id)) {
            Some(g) => g.1 += area,
            None => groups.push((a, *area)),
        }
    }
    if groups.iter().all(|(a, _)| a.is_none()) {
        return assembly_u(&t.wall_layers, t.rsi, t.rse, "wall").map_err(|m| missing.extend(m)).ok();
    }
    let (mut num, mut den, mut ok) = (0.0, 0.0, true);
    for (a, area) in &groups {
        let u = match a {
            Some(a) => a.u_value(t),
            None => assembly_u(&t.wall_layers, t.rsi, t.rse, "wall (no assembly)"),
        };
        match u {
            Ok(u) => {
                num += u * area;
                den += area;
                notes.push(format!("wall {}: U {:.3} W/(m2 K) over {:.2} m2", a.map_or("without an assembly".to_string(), |a| format!("assembly '{}'", a.name)), u, area));
            }
            Err(m) => {
                for x in m {
                    if !missing.contains(&x) {
                        missing.push(x);
                    }
                }
                ok = false;
            }
        }
    }
    (ok && den > 0.0).then(|| num / den)
}

pub fn report(model: &Model, p: &NanoArchProject) -> ThermalReport {
    let t: &ThermalInputs = &p.meta.compliance.thermal;
    let mut missing: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let zone = p.meta.climate_zone.as_deref().and_then(presets::zone_of);
    let zp = zone.and_then(presets::zone);
    if zp.is_none() {
        missing.push("climate zone (set it in the project details; a city or zone number)".into());
    }
    let (wall_net, _) = crate::insulation::scope_area(model, "wall", None);
    let (roof, _) = crate::insulation::scope_area(model, "roof", None);
    let windows = opening_area(model, "Window");
    let doors = opening_area(model, "Door");
    let gross_wall = wall_net + windows + doors;
    let share = if gross_wall > 0.0 { windows / gross_wall * 100.0 } else { 0.0 };

    let wall_u = wall_envelope_u(model, t, &mut missing, &mut notes);
    let roof_u = assembly_u(&t.roof_layers, t.rsi, t.rse, "roof").map_err(|m| missing.extend(m)).ok();
    let win_u = t.window_u;
    if win_u.is_none() && windows > 0.0 {
        missing.push("window U-value".into());
    }
    let win_limit = zp.map(|_| presets::GLAZING_BANDS.iter().zip([25.0, 50.0, 100.0]).find(|(_, hi)| share <= *hi + 1e-9).map_or(presets::GLAZING_BANDS[2].1, |(b, _)| b.1));
    let mk = |class: &'static str, area: f64, u: Option<f64>, limit: Option<f64>| {
        let status = match (u, limit) {
            (None, _) if area > 0.0 => "incomplete",
            (None, _) => "no area",
            (_, None) => "no limit",
            (Some(u), Some(l)) if u <= l + 1e-9 => "ok",
            _ => "exceeds limit",
        };
        ClassRow { class, area_m2: r3(area), u: u.map(r3), limit, status: status.into() }
    };
    let rows = vec![mk("wall (net of openings)", wall_net, wall_u, zp.map(|z| z.wall_u_max)), mk("roof", roof, roof_u, zp.map(|z| z.roof_u_max)), mk("window", windows, win_u, win_limit)];
    // Weighted U and the reference at the limits: only when every class that has an area has a U and a limit.
    let complete = rows.iter().filter(|r| r.area_m2 > 0.0).all(|r| r.u.is_some() && r.limit.is_some()) && rows.iter().any(|r| r.area_m2 > 0.0);
    let (mut num, mut refn, mut den) = (0.0, 0.0, 0.0);
    if complete {
        for r in rows.iter().filter(|r| r.area_m2 > 0.0) {
            num += r.area_m2 * r.u.unwrap_or(0.0);
            refn += r.area_m2 * r.limit.unwrap_or(0.0);
            den += r.area_m2;
        }
    }
    let bridge = t.bridge_pct.unwrap_or(0.0);
    if t.bridge_pct.is_none() && complete {
        notes.push("no thermal-bridge allowance entered: the weighted U has none".into());
    }
    let weighted = complete.then(|| r3(num / den * (1.0 + bridge / 100.0)));
    let reference = complete.then(|| r3(refn / den));
    let ratio = match (weighted, reference) {
        (Some(w), Some(r)) if r > 0.0 => Some(r3(w / r)),
        _ => None,
    };
    let band = match (ratio, &t.band_ratios) {
        (Some(x), Some(b)) => Some(["A", "B", "C", "D", "E"].iter().zip(b).find(|(_, hi)| x <= **hi + 1e-9).map_or("F", |(n, _)| *n)),
        (Some(_), None) => {
            missing.push("the five band limits (A to E) for the efficiency band".into());
            None
        }
        _ => None,
    };
    if doors > 0.0 {
        notes.push(format!("exterior doors ({:.2} m2) are not in the weighted U: no limit is recorded for them", doors));
    }
    if let (Some(s), Some(z)) = (t.window_shgc, zp)
        && s > z.window_shgc_max + 1e-9
    {
        notes.push(format!("window SHGC {s} is above the guide's maximum {} (0-25 % glazing band)", z.window_shgc_max));
    }
    notes.push("bands A to F are this application's relative scale (weighted U / reference U), not an SBC or SEC rating".into());
    ThermalReport {
        zone,
        rows,
        weighted_u: weighted,
        reference_u: reference,
        ratio,
        band,
        glazing_share_pct: (share * 10.0).round() / 10.0,
        door_area_m2: r3(doors),
        shgc: t.window_shgc,
        shgc_limit: zp.map(|z| z.window_shgc_max),
        missing,
        notes,
        caveat_en: presets::ZONE_CAVEAT_EN,
        caveat_ar: presets::ZONE_CAVEAT_AR,
        limits_source: zp.map_or_else(|| presets::GUIDE_NOTE.to_string(), |z| z.source.text()),
    }
}
