//! Punching shear around columns (isolated footings and, if you say the slabs are flat slabs, the slabs) and span-to-depth checks of
//! beams and slabs, with a safety factor per element id.
//!
//! - **Punching**: critical perimeter at `d / 2` from the column faces, `bo = 2 (b + d) + 2 (h + d)`; the shear is the factored column
//!   load less the factored pressure on the area inside the perimeter; the stress is `Vu / (bo d)`; the capacity is
//!   `punchingFactor * sqrt(f'c) * phi` in MPa. The safety factor is capacity / stress. `d` is the thickness less your cover and bar
//!   diameter.
//! - **Span to depth**: beams `length / h`, slabs `short side / thickness`, against the limits you enter; the factor is limit / actual.
//! - The factor `0.33` the page offers is the one in the feature brief; it is NOT read from the code files (SBC 304 is not in them), and
//!   neither are the load factor, phi or the span-to-depth limits. Every one is your input and a missing one is named.
//!
//! Names are those of the schedules (`COL-n`, `FND-n` or the footing's own label, `BM-n`, `SL-n`).

use crate::model::{Id, Model};
use crate::structx::StructuralInputs;
use crate::typed::{Beam, Column, Footing, Slab, Typed};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct PunchRow {
    pub name: String,
    pub id: String,
    pub column: String,
    /// `footing` or `slab`.
    pub kind: &'static str,
    pub d_mm: f64,
    pub bo_mm: f64,
    pub vu_kn: f64,
    pub stress_mpa: f64,
    pub capacity_mpa: f64,
    pub safety: f64,
    pub status: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeflRow {
    pub name: String,
    pub id: String,
    pub kind: &'static str,
    pub span_mm: f64,
    pub depth_mm: f64,
    pub ratio: f64,
    pub limit: f64,
    pub safety: f64,
    pub status: &'static str,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PunchReport {
    pub punching: Vec<PunchRow>,
    pub deflection: Vec<DeflRow>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

pub fn check(model: &Model, inp: &StructuralInputs, footing_thickness_mm: Option<f64>, dead_kpa: Option<f64>, live_kpa: Option<f64>) -> PunchReport {
    let mut rep = PunchReport::default();
    let nm = crate::structx::schedule_names(model, footing_thickness_mm);
    let name_of = |id: &Id| nm.get(&id.to_string()).cloned().unwrap_or_else(|| id.to_string());
    fn need(missing: &mut Vec<String>, n: &str, ok: bool) {
        if !ok && !missing.iter().any(|m| m == n) {
            missing.push(n.to_string());
        }
    }
    let (fc, lf, k, cover, bar) = (inp.fc_mpa, inp.load_factor, inp.punching_factor, inp.cover_mm, inp.bar_dia_mm);
    let punch_ready = fc.is_some() && lf.is_some() && k.is_some() && cover.is_some() && bar.is_some();
    need(&mut rep.missing, "concrete strength f'c (MPa)", fc.is_some());
    need(&mut rep.missing, "load factor (service to factored)", lf.is_some());
    need(&mut rep.missing, "punching shear factor (stress = factor x sqrt(f'c))", k.is_some());
    need(&mut rep.missing, "cover (mm)", cover.is_some());
    need(&mut rep.missing, "bar diameter (mm)", bar.is_some());
    let mut punching = Vec::new();
    if inp.phi_shear.is_none() {
        rep.notes.push("no strength reduction factor (phi) entered: the capacity is not reduced".into());
    }
    if punch_ready {
        let (fc, lf, k, cover, bar) = (fc.unwrap_or(0.0), lf.unwrap_or(0.0), k.unwrap_or(0.0), cover.unwrap_or(0.0), bar.unwrap_or(0.0));
        let cap = k * fc.sqrt() * inp.phi_shear.unwrap_or(1.0);
        let status = |safety: f64| if safety >= 1.0 { "ok" } else { "fails" };
        // Footings.
        match footing_thickness_mm {
            None => {
                rep.missing.push("footing thickness (mm): in the structural schedule inputs".into());
            }
            Some(t) => {
                for e in model.elements.values().filter(|e| e.kind == Footing::KIND) {
                    let Ok(f) = e.typed::<Footing>() else { continue };
                    let col_id = f.column.parse::<Id>().ok();
                    let Some(c) = col_id.and_then(|c| model.get(&c)).and_then(|c| c.typed::<Column>().ok()) else { continue };
                    let d = t - cover - bar;
                    if d <= 0.0 {
                        rep.notes.push(format!("{}: thickness {t} mm leaves no effective depth after cover and bar", name_of(&e.id)));
                        continue;
                    }
                    let (b, h) = (c.b as f64 + d, c.h as f64 + d);
                    let bo = 2.0 * (b + h);
                    let pu = f.service_load_kn * lf;
                    let a = f.length as f64 * f.width as f64;
                    let vu = (pu * (1.0 - b * h / a)).max(0.0);
                    let stress = vu * 1000.0 / (bo * d);
                    let safety = if stress > 0.0 { cap / stress } else { f64::INFINITY };
                    punching.push(PunchRow { name: name_of(&e.id), id: e.id.to_string(), column: col_id.map(|c| name_of(&c)).unwrap_or_default(), kind: "footing", d_mm: r3(d), bo_mm: r3(bo), vu_kn: r3(vu), stress_mpa: r3(stress), capacity_mpa: r3(cap), safety: r3(safety.min(9999.0)), status: status(safety) });
                }
            }
        }
        // Slabs treated as flat slabs.
        if inp.treat_slabs_as_flat {
            match (dead_kpa, live_kpa) {
                (Some(dl), Some(ll)) => {
                    let trib = crate::structural::column_tributary(model);
                    for e in model.elements.values().filter(|e| e.kind == Column::KIND) {
                        let Ok(c) = e.typed::<Column>() else { continue };
                        let Some(sl) = model.elements.values().filter(|s| s.kind == Slab::KIND && s.level_id == e.level_id).filter_map(|s| s.typed::<Slab>().ok()).filter(|s| c.x >= s.x - 200 && c.x <= s.x + s.w + 200 && c.y >= s.y - 200 && c.y <= s.y + s.h + 200).max_by_key(|s| s.thickness) else { continue };
                        let d = sl.thickness as f64 - cover - bar;
                        if d <= 0.0 {
                            continue;
                        }
                        let (b, h) = (c.b as f64 + d, c.h as f64 + d);
                        let bo = 2.0 * (b + h);
                        let w = lf * (dl + ll);
                        let vu = (w * trib.get(&e.id).copied().unwrap_or(0.0) - w * b * h / 1e6).max(0.0);
                        let stress = vu * 1000.0 / (bo * d);
                        let safety = if stress > 0.0 { cap / stress } else { f64::INFINITY };
                        punching.push(PunchRow { name: format!("{} slab", name_of(&e.id)), id: e.id.to_string(), column: name_of(&e.id), kind: "slab", d_mm: r3(d), bo_mm: r3(bo), vu_kn: r3(vu), stress_mpa: r3(stress), capacity_mpa: r3(cap), safety: r3(safety.min(9999.0)), status: status(safety) });
                    }
                }
                _ => rep.missing.push("dead and live floor loads (kPa) for the flat-slab punching check".into()),
            }
        }
    }
    rep.punching = punching;
    // Span to depth.
    need(&mut rep.missing, "beam span-to-depth limit", inp.beam_span_depth_max.is_some());
    need(&mut rep.missing, "slab span-to-depth limit", inp.slab_span_depth_max.is_some());
    if let Some(lim) = inp.beam_span_depth_max {
        for e in model.elements.values().filter(|e| e.kind == Beam::KIND) {
            let Ok(b) = e.typed::<Beam>() else { continue };
            let ratio = b.length as f64 / b.h as f64;
            let safety = lim / ratio;
            rep.deflection.push(DeflRow { name: name_of(&e.id), id: e.id.to_string(), kind: "beam", span_mm: b.length as f64, depth_mm: b.h as f64, ratio: r3(ratio), limit: lim, safety: r3(safety), status: if safety >= 1.0 { "ok" } else { "fails" } });
        }
    }
    if let Some(lim) = inp.slab_span_depth_max {
        for e in model.elements.values().filter(|e| e.kind == Slab::KIND) {
            let Ok(s) = e.typed::<Slab>() else { continue };
            let span = s.w.min(s.h) as f64;
            let ratio = span / s.thickness as f64;
            let safety = lim / ratio;
            rep.deflection.push(DeflRow { name: name_of(&e.id), id: e.id.to_string(), kind: "slab", span_mm: span, depth_mm: s.thickness as f64, ratio: r3(ratio), limit: lim, safety: r3(safety), status: if safety >= 1.0 { "ok" } else { "fails" } });
        }
    }
    rep.deflection.sort_by(|a, b| a.name.cmp(&b.name));
    rep.punching.sort_by(|a, b| a.name.cmp(&b.name));
    rep.notes.push("preliminary: every factor and limit is your input (none is read from the code files); a licensed structural engineer must verify".into());
    rep
}
