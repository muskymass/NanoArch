//! Slab system advisor: for each level, the hourdi (ribbed with blocks), the solid slab and the flat slab side by side, on the
//! level's own slabs: depth, concrete (m3), estimated reinforcement (kg) and cost.
//!
//! Every number that decides a result is an input you enter in `slabs`: the span-to-depth ratio of each system, the greatest span
//! each system is allowed, the reinforcement ratio (kg per m3 of concrete), the rib geometry, the rates. The application holds no
//! code value for any of them (SBC 304 is not among the code files), so a missing one is named and that system is not priced.
//!
//! Spans: a slab panel is spanned across its short side by the hourdi and the solid slab, and across its long side by the flat slab
//! (which has no beams, so the longer direction governs). The clear span subtracts one support width: the level's mean beam width, or
//! its mean column width when it has no beams.

use crate::building;
use crate::model::{Id, Model};
use crate::typed::{Beam, Column, Slab, Typed};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SystemIn {
    /// Span divided by depth.
    pub span_depth: Option<f64>,
    /// Longest span this system may be used for, mm.
    pub max_span_mm: Option<f64>,
    /// Estimated reinforcement, kg per m3 of concrete.
    pub rebar_kg_m3: Option<f64>,
    /// Everything else per m2 of slab (blocks, formwork, finishing), in the currency of the rates.
    pub extra_rate_m2: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RibbedGeom {
    pub topping_mm: Option<f64>,
    pub rib_width_mm: Option<f64>,
    /// Centre to centre of the ribs.
    pub rib_spacing_mm: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SlabAdvInputs {
    pub hourdi: SystemIn,
    pub solid: SystemIn,
    pub flat: SystemIn,
    pub ribbed: RibbedGeom,
    /// Per m3 of concrete.
    pub concrete_rate: Option<f64>,
    /// Per kg of reinforcement.
    pub steel_rate: Option<f64>,
}

impl SlabAdvInputs {
    pub fn check(&self) -> Result<(), String> {
        let pos = |n: &str, v: Option<f64>| match v {
            Some(x) if !x.is_finite() || x <= 0.0 => Err(format!("{n} must be a positive number")),
            _ => Ok(()),
        };
        for (name, s) in [("hourdi", &self.hourdi), ("solid slab", &self.solid), ("flat slab", &self.flat)] {
            pos(&format!("{name} span-to-depth ratio"), s.span_depth)?;
            pos(&format!("{name} maximum span"), s.max_span_mm)?;
            pos(&format!("{name} reinforcement ratio"), s.rebar_kg_m3)?;
            if s.extra_rate_m2.is_some_and(|x| !x.is_finite() || x < 0.0) {
                return Err(format!("{name} extra rate must be zero or more"));
            }
        }
        for (n, v) in [("topping", self.ribbed.topping_mm), ("rib width", self.ribbed.rib_width_mm), ("rib spacing", self.ribbed.rib_spacing_mm), ("concrete rate", self.concrete_rate), ("steel rate", self.steel_rate)] {
            pos(n, v)?;
        }
        if let (Some(w), Some(s)) = (self.ribbed.rib_width_mm, self.ribbed.rib_spacing_mm)
            && w >= s
        {
            return Err("the rib width must be less than the rib spacing".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemRow {
    pub system: &'static str,
    pub span_mm: f64,
    pub depth_mm: Option<f64>,
    pub concrete_m3: Option<f64>,
    pub rebar_kg: Option<f64>,
    pub cost: Option<f64>,
    /// The span is within the system's own maximum (None when no maximum was entered).
    pub within_max_span: Option<bool>,
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LevelRow {
    pub level: Option<i64>,
    pub slabs: usize,
    pub area_m2: f64,
    pub short_span_mm: f64,
    pub long_span_mm: f64,
    pub support_mm: f64,
    pub systems: Vec<SystemRow>,
    /// The cheapest system among those that are priced and within their maximum span.
    pub cheapest: Option<&'static str>,
    /// What the model holds now: the slabs' thickness and volume.
    pub current_thickness_mm: Option<i64>,
    pub current_m3: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SlabReport {
    pub levels: Vec<LevelRow>,
    pub notes: Vec<String>,
}

fn ceil_to(v: f64, step: f64) -> f64 {
    (v / step).ceil() * step
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn row(system: &'static str, span: f64, area_m2: f64, s: &SystemIn, g: &RibbedGeom, concrete_rate: Option<f64>, steel_rate: Option<f64>) -> SystemRow {
    let mut missing = Vec::new();
    let ribbed = system == "hourdi";
    if s.span_depth.is_none() {
        missing.push(format!("{system}: span-to-depth ratio"));
    }
    if ribbed {
        for (n, v) in [("topping", g.topping_mm), ("rib width", g.rib_width_mm), ("rib spacing", g.rib_spacing_mm)] {
            if v.is_none() {
                missing.push(format!("{system}: {n}"));
            }
        }
    }
    let depth = s.span_depth.map(|r| ceil_to(span / r, if ribbed { 50.0 } else { 10.0 }));
    let per_m2_mm = match (depth, ribbed, g.topping_mm, g.rib_width_mm, g.rib_spacing_mm) {
        (Some(d), false, _, _, _) => Some(d),
        (Some(d), true, Some(t), Some(w), Some(sp)) if d > t => Some(t + (d - t) * w / sp),
        (Some(_), true, Some(_), Some(_), Some(_)) => {
            missing.push("hourdi: the depth for this span is not more than the topping".into());
            None
        }
        _ => None,
    };
    let m3 = per_m2_mm.map(|mm| r2(area_m2 * mm / 1000.0));
    let kg = match (m3, s.rebar_kg_m3) {
        (Some(v), Some(k)) => Some(r2(v * k)),
        (Some(_), None) => {
            missing.push(format!("{system}: reinforcement kg/m3"));
            None
        }
        _ => None,
    };
    let cost = match (m3, kg, concrete_rate, steel_rate, s.extra_rate_m2) {
        (Some(v), Some(k), Some(cr), Some(sr), Some(ex)) => Some(r2(v * cr + k * sr + area_m2 * ex)),
        (Some(_), _, cr, sr, ex) => {
            for (n, v) in [("concrete rate", cr.is_some()), ("steel rate", sr.is_some()), (&format!("{system}: extra rate per m2") as &str, ex.is_some())] {
                if !v {
                    missing.push(n.to_string());
                }
            }
            None
        }
        _ => None,
    };
    SystemRow { system, span_mm: r2(span), depth_mm: depth, concrete_m3: m3, rebar_kg: kg, cost, within_max_span: s.max_span_mm.map(|m| span <= m + 1e-9), missing }
}

pub fn advise(model: &Model, inp: &SlabAdvInputs) -> SlabReport {
    let mut rep = SlabReport::default();
    let levels = building::levels(model);
    let level_ids: Vec<Option<Id>> = if levels.is_empty() { vec![None] } else { levels.iter().map(|(_, id)| Some(*id)).collect() };
    for lv in level_ids {
        let slabs: Vec<Slab> = model.elements.values().filter(|e| e.kind == Slab::KIND && e.level_id == lv).filter_map(|e| e.typed::<Slab>().ok()).collect();
        if slabs.is_empty() {
            continue;
        }
        let beams: Vec<i64> = model.elements.values().filter(|e| e.kind == Beam::KIND && e.level_id == lv).filter_map(|e| e.typed::<Beam>().ok()).map(|b| b.b).collect();
        let cols: Vec<i64> = model.elements.values().filter(|e| e.kind == Column::KIND && e.level_id == lv).filter_map(|e| e.typed::<Column>().ok()).map(|c| c.b.min(c.h)).collect();
        let sup = |v: &[i64]| if v.is_empty() { None } else { Some(v.iter().sum::<i64>() as f64 / v.len() as f64) };
        let support = sup(&beams).or_else(|| sup(&cols)).unwrap_or(0.0);
        let area: f64 = slabs.iter().map(|s| s.w as f64 * s.h as f64 / 1e6).sum();
        let short = slabs.iter().map(|s| s.w.min(s.h) as f64 - support).fold(0.0, f64::max);
        let long = slabs.iter().map(|s| s.w.max(s.h) as f64 - support).fold(0.0, f64::max);
        let systems = vec![row("hourdi", short, area, &inp.hourdi, &inp.ribbed, inp.concrete_rate, inp.steel_rate), row("solid", short, area, &inp.solid, &inp.ribbed, inp.concrete_rate, inp.steel_rate), row("flat", long, area, &inp.flat, &inp.ribbed, inp.concrete_rate, inp.steel_rate)];
        let cheapest = systems.iter().filter(|s| s.cost.is_some() && s.within_max_span != Some(false)).min_by(|a, b| a.cost.unwrap_or(f64::MAX).total_cmp(&b.cost.unwrap_or(f64::MAX))).map(|s| s.system);
        rep.levels.push(LevelRow {
            level: lv.and_then(|l| levels.iter().find(|x| x.1 == l).map(|x| x.0)),
            slabs: slabs.len(),
            area_m2: r2(area),
            short_span_mm: r2(short),
            long_span_mm: r2(long),
            support_mm: r2(support),
            systems,
            cheapest,
            current_thickness_mm: slabs.iter().map(|s| s.thickness).max(),
            current_m3: r2(slabs.iter().map(|s| s.w as f64 * s.h as f64 * s.thickness as f64 / 1e9).sum()),
        });
    }
    if rep.levels.is_empty() {
        rep.notes.push("the model has no slabs".into());
    }
    rep.notes.push("every ratio, rate and limit comes from your inputs: no code value is built in; the comparison is preliminary and needs a licensed structural engineer".into());
    rep
}
