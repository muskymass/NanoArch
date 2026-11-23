//! Rooftop solar PV sizing: the usable roof, the number of panels, the installed power, the inverter and, when you give the yield and
//! the tariff, the energy and the money saved.
//!
//! - **Roof**: the slabs of the top level, each pulled in by the edge clearance you enter, MINUS the footprint of the roof annex and the
//!   elevator shaft and machine room of the project (`meta.vertical`) and your own obstructions. The union is measured exactly.
//! - **Panels**: `floor(usable m2 x coverage ratio / panel area)`; power `panels x panel W`; the inverter is that power divided by the
//!   DC/AC ratio you enter.
//! - **Energy and savings**: `kW x specific yield (kWh per kW per year)`; the saving is the energy used in the building
//!   (`selfConsumption`) times the tariff. No yield, tariff, panel rating or ratio is built in: a missing one is named and that figure
//!   is not computed. The exported energy is not paid for here (no export tariff exists in the inputs).

use crate::mepx::{Rect4, SolarIn, area_minus_m2, r2};
use crate::model::Model;
use crate::typed::{Slab, Typed};
use crate::vertical;
use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
pub struct SolarReport {
    pub roof_m2: f64,
    pub after_edge_m2: f64,
    pub excluded_m2: f64,
    pub usable_m2: f64,
    pub panels: Option<u64>,
    pub kwp: Option<f64>,
    pub inverter_kw: Option<f64>,
    pub annual_kwh: Option<f64>,
    pub self_used_kwh: Option<f64>,
    pub exported_kwh: Option<f64>,
    pub annual_saving: Option<f64>,
    pub offset_pct: Option<f64>,
    /// Usable roof as a share of the whole roof, percent.
    pub usable_share_pct: Option<f64>,
    /// `suitable`, `unsuitable` or `unchecked` (no criterion entered, or the figure it needs was not computed).
    pub suitability: String,
    pub suitability_reasons: Vec<String>,
    /// The roof rectangles left for panels' footprint reference: `[x0, y0, x1, y1]` mm of the pulled-in slabs, and what is excluded.
    pub roof_rects: Vec<Rect4>,
    pub excluded_rects: Vec<Rect4>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
}

pub fn size(model: &Model, p: &crate::project::NanoArchProject) -> SolarReport {
    let mut rep = size_inner(model, p);
    judge(&p.meta.mep_ext.solar, &mut rep);
    rep
}

/// Applies the user's own suitability criteria. A criterion that was not entered is not judged; one whose figure is missing stays `unchecked`.
fn judge(s: &SolarIn, rep: &mut SolarReport) {
    let mut checked = false;
    let mut failed = false;
    if let Some(min) = s.min_usable_share {
        match rep.usable_share_pct {
            Some(share) => {
                checked = true;
                if share < min * 100.0 {
                    failed = true;
                    rep.suitability_reasons.push(format!("usable share {share:.2}% is below the {:.2}% you require", min * 100.0));
                }
            }
            None => rep.missing.push("a roof, to judge the usable share".into()),
        }
    }
    if let Some(min) = s.min_panels {
        match rep.panels {
            Some(n) => {
                checked = true;
                if n < min {
                    failed = true;
                    rep.suitability_reasons.push(format!("{n} panels fit, fewer than the {min} you require"));
                }
            }
            None => rep.missing.push("panel power, area and coverage ratio, to judge the panel count".into()),
        }
    }
    if failed {
        rep.suitability = "unsuitable".into();
    } else if checked {
        rep.suitability = "suitable".into();
    }
}

fn size_inner(model: &Model, p: &crate::project::NanoArchProject) -> SolarReport {
    let s: &SolarIn = &p.meta.mep_ext.solar;
    let mut rep = SolarReport { suitability: "unchecked".into(), ..Default::default() };
    let top = crate::building::levels(model).last().map(|(_, id)| *id);
    let slabs: Vec<Slab> = model.elements.values().filter(|e| e.kind == Slab::KIND && (top.is_none() || e.level_id == top)).filter_map(|e| e.typed::<Slab>().ok()).collect();
    if slabs.is_empty() {
        rep.missing.push("a roof: the model has no slabs".into());
        return rep;
    }
    let raw: Vec<Rect4> = slabs.iter().map(|s| [s.x as f64, s.y as f64, (s.x + s.w) as f64, (s.y + s.h) as f64]).collect();
    rep.roof_m2 = r2(area_minus_m2(&raw, &[]));
    let edge = s.edge_clearance_mm;
    if edge.is_none() {
        rep.missing.push("edge clearance (mm): the strip kept free along the roof edge".into());
    }
    let e = edge.unwrap_or(0.0);
    let inset: Vec<Rect4> = raw.iter().filter_map(|r| (r[2] - r[0] > 2.0 * e && r[3] - r[1] > 2.0 * e).then_some([r[0] + e, r[1] + e, r[2] - e, r[3] - e])).collect();
    // The edge clearance applies to the outline of the roof; panels are kept off the joints between slabs only by your obstructions.
    rep.after_edge_m2 = r2(area_minus_m2(&inset, &[]));
    // What stands on or passes through the roof.
    let mut excluded: Vec<Rect4> = s.obstructions.clone();
    if let Some(v) = &p.meta.vertical {
        let scene = crate::model3d::build(model);
        if let Ok(r) = vertical::report(&scene, v)
            && let Some(sh) = &r.shaft
        {
            excluded.push([sh.rect_m[0] * 1000.0, sh.rect_m[1] * 1000.0, sh.rect_m[2] * 1000.0, sh.rect_m[3] * 1000.0]);
        }
        if let Some(a) = &v.roof_annex
            && let (Some(x), Some(y), Some(w), Some(l)) = (a.x_mm, a.y_mm, a.width_mm, a.length_mm)
        {
            excluded.push([x, y, x + w, y + l]);
        }
    }
    rep.usable_m2 = r2(area_minus_m2(&inset, &excluded));
    rep.excluded_m2 = r2(rep.after_edge_m2 - rep.usable_m2);
    if rep.roof_m2 > 0.0 {
        rep.usable_share_pct = Some(r2(rep.usable_m2 / rep.roof_m2 * 100.0));
    }
    rep.roof_rects = inset;
    rep.excluded_rects = excluded;
    for (n, ok) in [("panel power (W)", s.panel_w.is_some()), ("panel area (m2)", s.panel_area_m2.is_some()), ("coverage ratio (0 to 1)", s.coverage_ratio.is_some())] {
        if !ok {
            rep.missing.push(n.to_string());
        }
    }
    let (Some(w), Some(a), Some(cov)) = (s.panel_w, s.panel_area_m2, s.coverage_ratio) else { return rep };
    let panels = (rep.usable_m2 * cov / a).floor() as u64;
    let kwp = panels as f64 * w / 1000.0;
    rep.panels = Some(panels);
    rep.kwp = Some(r2(kwp));
    match s.dc_ac_ratio {
        Some(r) => rep.inverter_kw = Some(r2(kwp / r)),
        None => rep.missing.push("DC/AC ratio (to size the inverter)".into()),
    }
    match s.specific_yield_kwh_kw {
        Some(y) => {
            let e = kwp * y;
            rep.annual_kwh = Some(r2(e));
            match s.self_consumption {
                Some(sc) => {
                    rep.self_used_kwh = Some(r2(e * sc));
                    rep.exported_kwh = Some(r2(e * (1.0 - sc)));
                    match s.tariff_per_kwh {
                        Some(t) => rep.annual_saving = Some(r2(e * sc * t)),
                        None => rep.missing.push("tariff per kWh (to price the saving)".into()),
                    }
                    if let Some(c) = s.annual_consumption_kwh {
                        rep.offset_pct = Some(r2(e * sc / c * 100.0));
                    }
                }
                None => rep.missing.push("self-consumption share (0 to 1): the saving counts only the energy used in the building".into()),
            }
        }
        None => rep.missing.push("annual yield, kWh per kW per year (from your own source for the site)".into()),
    }
    rep.notes.push("no yield, tariff or panel rating is built in; shading by neighbours and the roof's tilt are not modelled; the exported energy is not paid for here".into());
    rep
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Source;
    use crate::typed::element_of;
    use serde_json::Map;

    /// A 10 m x 10 m roof slab.
    fn project(solar: SolarIn) -> (Model, crate::project::NanoArchProject) {
        let mut model = Model::default();
        let slab = Slab { x: 0, y: 0, w: 10_000, h: 10_000, thickness: 200, kind: "roof".into(), room: String::new(), extra: Map::new() };
        let el = element_of(&slab, Source::User);
        model.elements.insert(el.id, el);
        let mut p = crate::project::NanoArchProject::default();
        p.meta.mep_ext.solar = solar;
        (model, p)
    }

    fn full() -> SolarIn {
        SolarIn { panel_w: Some(500.0), panel_area_m2: Some(2.0), edge_clearance_mm: Some(1000.0), coverage_ratio: Some(0.5), ..Default::default() }
    }

    #[test]
    fn no_criterion_means_unchecked_and_nothing_is_invented() {
        let (m, p) = project(full());
        let r = size(&m, &p);
        assert_eq!(r.suitability, "unchecked");
        assert!(r.suitability_reasons.is_empty());
        // 8 m x 8 m after the 1 m edge strip = 64 m2 of 100 m2.
        assert_eq!(r.usable_share_pct, Some(64.0));
        assert_eq!(r.panels, Some(16));
    }

    #[test]
    fn the_users_own_criteria_decide_suitable_or_not() {
        let (m, p) = project(SolarIn { min_usable_share: Some(0.6), min_panels: Some(16), ..full() });
        assert_eq!(size(&m, &p).suitability, "suitable");
        let (m, p) = project(SolarIn { min_usable_share: Some(0.7), ..full() });
        let r = size(&m, &p);
        assert_eq!(r.suitability, "unsuitable");
        assert!(r.suitability_reasons[0].contains("64.00%") && r.suitability_reasons[0].contains("70.00%"));
        let (m, p) = project(SolarIn { min_panels: Some(17), ..full() });
        let r = size(&m, &p);
        assert_eq!(r.suitability, "unsuitable");
        assert!(r.suitability_reasons[0].contains("16 panels"));
    }

    #[test]
    fn a_criterion_without_its_figure_stays_unchecked_and_is_named() {
        let (m, p) = project(SolarIn { min_panels: Some(5), edge_clearance_mm: Some(1000.0), ..Default::default() });
        let r = size(&m, &p);
        assert_eq!(r.suitability, "unchecked");
        assert!(r.missing.iter().any(|x| x.contains("judge the panel count")));
        let (m, p) = (Model::default(), p);
        let r = size(&m, &p);
        assert_eq!(r.suitability, "unchecked");
        assert!(r.missing.iter().any(|x| x.contains("no slabs")));
    }

    #[test]
    fn an_impossible_share_is_refused() {
        let mut e = crate::mepx::MepExtInputs::default();
        e.solar.min_usable_share = Some(1.5);
        assert!(e.check().is_err());
    }
}
