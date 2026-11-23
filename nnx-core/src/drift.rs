//! Deflection, storey drift and foundation settlement checks.
//!
//! Three independent checks over inputs the caller supplies. Nothing is assumed: moduli, loads, stiffness reductions and every
//! limit are inputs with a source the caller keeps; a missing required value is refused by name and a missing limit leaves that
//! check `unchecked` (never `ok`).
//!
//! - **Beam deflection** (service UDL `w`, linear elastic, `I = b h^3 / 12`, optionally times a cracked-section factor):
//!   simply supported `5 w L^4 / (384 E I)`, fixed-fixed `w L^4 / (384 E I)`, cantilever `w L^4 / (8 E I)`. An optional
//!   long-term multiplier gives the long-term deflection; absent, only the immediate value is reported and the report says so.
//! - **Storey drift** (shear-building idealization: rigid floors, columns fixed at both ends, `k = sum 12 E I / h^3`): drift
//!   `V / k` from a storey shear the caller took from their lateral analysis (this module performs no seismic or wind analysis),
//!   its ratio to the storey height, and the accumulated top drift.
//! - **Settlement** (immediate elastic, flexible-footing formula `s = q B (1 - nu^2) Is / Es`): per footing, then differential
//!   settlement and angular distortion between named pairs of footings over the distance the caller gives.
//!
//! Output is a preliminary serviceability screen, not a design and not a code check.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct BeamIn {
    pub key: String,
    pub span_mm: f64,
    pub b_mm: f64,
    pub h_mm: f64,
    /// `simple`, `fixed` or `cantilever`.
    pub support: String,
    /// Service uniform line load, kN/m.
    pub w_kn_m: f64,
    /// Ie / Ig for a cracked section, in (0, 1]. Absent: gross section used and the report says so.
    #[serde(default)]
    pub cracked_factor: Option<f64>,
    /// Multiplier on the immediate deflection for the long-term value. Absent: immediate only.
    #[serde(default)]
    pub long_term_factor: Option<f64>,
    /// The deflection must not exceed `span / limit_span_ratio` (for example 360). Absent: `unchecked`.
    #[serde(default)]
    pub limit_span_ratio: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ColumnGroup {
    pub count: u32,
    /// Section size across the loading direction, mm.
    pub width_mm: f64,
    /// Section depth along the loading direction, mm.
    pub depth_mm: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct StoreyIn {
    pub key: String,
    pub height_mm: f64,
    /// Storey shear from the caller's lateral analysis, kN.
    pub shear_kn: f64,
    pub columns: Vec<ColumnGroup>,
    /// Effective-stiffness factor for cracked columns, in (0, 1]. Absent: gross section.
    #[serde(default)]
    pub stiffness_factor: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FootingIn {
    pub key: String,
    pub load_kn: f64,
    pub width_mm: f64,
    pub length_mm: f64,
    /// Soil modulus, MPa.
    pub es_mpa: f64,
    /// Poisson's ratio of the soil, in [0, 0.5).
    pub nu: f64,
    /// Influence factor for the footing shape and rigidity; the caller states its source.
    pub influence: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PairIn {
    pub a: String,
    pub b: String,
    /// Centre-to-centre distance, mm.
    pub distance_mm: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DriftInput {
    /// Concrete modulus, MPa. Required when `beams` or `storeys` are given.
    #[serde(default)]
    pub e_mpa: Option<f64>,
    #[serde(default)]
    pub beams: Vec<BeamIn>,
    #[serde(default)]
    pub storeys: Vec<StoreyIn>,
    /// Maximum storey drift ratio (drift / height), for example 0.005. Absent: `unchecked`.
    #[serde(default)]
    pub max_drift_ratio: Option<f64>,
    #[serde(default)]
    pub footings: Vec<FootingIn>,
    #[serde(default)]
    pub pairs: Vec<PairIn>,
    /// Maximum absolute settlement of a footing, mm. Absent: `unchecked`.
    #[serde(default)]
    pub max_settlement_mm: Option<f64>,
    /// Maximum angular distortion between two footings (differential / distance), for example 0.002. Absent: `unchecked`.
    #[serde(default)]
    pub max_angular_distortion: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BeamRow {
    pub key: String,
    pub support: String,
    pub immediate_mm: f64,
    pub long_term_mm: Option<f64>,
    /// The value compared to the limit: long-term when a multiplier was given, otherwise immediate.
    pub checked_mm: f64,
    pub allowed_mm: Option<f64>,
    pub status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StoreyRow {
    pub key: String,
    pub stiffness_kn_per_mm: f64,
    pub drift_mm: f64,
    pub drift_ratio: f64,
    /// Accumulated drift from the first listed storey up to and including this one, mm.
    pub cumulative_mm: f64,
    pub status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SettlementRow {
    pub key: String,
    pub pressure_kpa: f64,
    pub settlement_mm: f64,
    pub status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DifferentialRow {
    pub a: String,
    pub b: String,
    pub differential_mm: f64,
    pub angular_distortion: f64,
    pub status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub ar: String,
    pub en: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DriftReport {
    pub beams: Vec<BeamRow>,
    pub storeys: Vec<StoreyRow>,
    pub total_drift_mm: Option<f64>,
    pub settlements: Vec<SettlementRow>,
    pub differentials: Vec<DifferentialRow>,
    pub findings: Vec<Finding>,
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn r6(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}

fn positive(v: f64, field: &str, key: &str) -> Result<f64, String> {
    if v.is_finite() && v > 0.0 {
        Ok(v)
    } else {
        Err(format!("'{key}': {field} must be a positive number, got {v}"))
    }
}

fn factor(v: Option<f64>, field: &str, key: &str) -> Result<f64, String> {
    match v {
        None => Ok(1.0),
        Some(f) if f.is_finite() && f > 0.0 && f <= 1.0 => Ok(f),
        Some(f) => Err(format!("'{key}': {field} must be in (0, 1], got {f}")),
    }
}

fn verdict(value: f64, limit: Option<f64>) -> &'static str {
    match limit {
        None => "unchecked",
        Some(l) if value <= l => "ok",
        Some(_) => "fails",
    }
}

fn unique<'a>(keys: impl Iterator<Item = &'a String>, what: &str) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for k in keys {
        if !seen.insert(k.as_str()) {
            return Err(format!("{what} key '{k}' is used twice"));
        }
    }
    Ok(())
}

fn beam_row(b: &BeamIn, e: f64) -> Result<BeamRow, String> {
    let (l, bw, h, w) = (positive(b.span_mm, "span_mm", &b.key)?, positive(b.b_mm, "b_mm", &b.key)?, positive(b.h_mm, "h_mm", &b.key)?, positive(b.w_kn_m, "w_kn_m", &b.key)?);
    let cr = factor(b.cracked_factor, "cracked_factor", &b.key)?;
    let ei = e * (bw * h.powi(3) / 12.0) * cr;
    // kN/m is N/mm, MPa is N/mm2, so the result is in mm.
    let coeff = match b.support.as_str() {
        "simple" => 5.0 / 384.0,
        "fixed" => 1.0 / 384.0,
        "cantilever" => 1.0 / 8.0,
        other => return Err(format!("beam '{}': support '{other}' is not one of simple, fixed, cantilever", b.key)),
    };
    let immediate = coeff * w * l.powi(4) / ei;
    let long_term = match b.long_term_factor {
        None => None,
        Some(f) if f.is_finite() && f >= 1.0 => Some(immediate * f),
        Some(f) => return Err(format!("beam '{}': long_term_factor must be 1 or more, got {f}", b.key)),
    };
    let checked = long_term.unwrap_or(immediate);
    let allowed = match b.limit_span_ratio {
        None => None,
        Some(r) => Some(l / positive(r, "limit_span_ratio", &b.key)?),
    };
    Ok(BeamRow { key: b.key.clone(), support: b.support.clone(), immediate_mm: r3(immediate), long_term_mm: long_term.map(r3), checked_mm: r3(checked), allowed_mm: allowed.map(r3), status: verdict(checked, allowed) })
}

fn storey_rows(storeys: &[StoreyIn], e: f64, max_ratio: Option<f64>) -> Result<(Vec<StoreyRow>, f64), String> {
    let mut rows = Vec::new();
    let mut cumulative = 0.0;
    for s in storeys {
        let h = positive(s.height_mm, "height_mm", &s.key)?;
        let v = positive(s.shear_kn, "shear_kn", &s.key)?;
        let sf = factor(s.stiffness_factor, "stiffness_factor", &s.key)?;
        if s.columns.is_empty() || s.columns.iter().all(|c| c.count == 0) {
            return Err(format!("storey '{}' has no columns: its stiffness cannot be computed", s.key));
        }
        let mut k = 0.0;
        for c in &s.columns {
            let (b, d) = (positive(c.width_mm, "width_mm", &s.key)?, positive(c.depth_mm, "depth_mm", &s.key)?);
            k += f64::from(c.count) * 12.0 * e * sf * (b * d.powi(3) / 12.0) / h.powi(3);
        }
        // k in N/mm; shear in kN so drift (mm) = V * 1000 / k.
        let drift = v * 1000.0 / k;
        cumulative += drift;
        let ratio = drift / h;
        rows.push(StoreyRow { key: s.key.clone(), stiffness_kn_per_mm: r3(k / 1000.0), drift_mm: r3(drift), drift_ratio: r6(ratio), cumulative_mm: r3(cumulative), status: verdict(ratio, max_ratio) });
    }
    Ok((rows, cumulative))
}

fn settlement_row(f: &FootingIn, max: Option<f64>) -> Result<(SettlementRow, f64), String> {
    let (load, b, l, es, is) = (positive(f.load_kn, "load_kn", &f.key)?, positive(f.width_mm, "width_mm", &f.key)?, positive(f.length_mm, "length_mm", &f.key)?, positive(f.es_mpa, "es_mpa", &f.key)?, positive(f.influence, "influence", &f.key)?);
    if !(f.nu.is_finite() && (0.0..0.5).contains(&f.nu)) {
        return Err(format!("footing '{}': nu must be in [0, 0.5), got {}", f.key, f.nu));
    }
    // kN / mm2 is kN/mm2 = 1000 MPa; pressure in kPa = kN/m2.
    let q_mpa = load * 1000.0 / (b * l);
    let s = q_mpa * b * (1.0 - f.nu * f.nu) * is / es;
    Ok((SettlementRow { key: f.key.clone(), pressure_kpa: r3(q_mpa * 1000.0), settlement_mm: r3(s), status: verdict(s, max) }, s))
}

pub fn check(inp: &DriftInput) -> Result<DriftReport, String> {
    if inp.beams.is_empty() && inp.storeys.is_empty() && inp.footings.is_empty() {
        return Err("nothing to check: give beams, storeys or footings".into());
    }
    unique(inp.beams.iter().map(|b| &b.key), "beam")?;
    unique(inp.storeys.iter().map(|s| &s.key), "storey")?;
    unique(inp.footings.iter().map(|f| &f.key), "footing")?;
    let mut findings = Vec::new();
    let e = if inp.beams.is_empty() && inp.storeys.is_empty() {
        0.0
    } else {
        positive(inp.e_mpa.ok_or("e_mpa is required for beams and storeys: the concrete modulus is an input with a stated source and nothing is assumed")?, "e_mpa", "input")?
    };

    let beams = inp.beams.iter().map(|b| beam_row(b, e)).collect::<Result<Vec<_>, _>>()?;
    for (row, b) in beams.iter().zip(&inp.beams) {
        if b.cracked_factor.is_none() {
            findings.push(Finding { ar: format!("الكمرة «{}»: لم يُدخل معامل التشقق فحُسب المقطع الكامل وقد يقلّ الهبوط الحقيقي عن الواقع.", b.key), en: format!("Beam '{}': no cracked_factor given, so the gross section is used and the real deflection may be larger.", b.key) });
        }
        if row.long_term_mm.is_none() {
            findings.push(Finding { ar: format!("الكمرة «{}»: لم يُدخل معامل الأمد الطويل فالمقارنة بالهبوط الفوري فقط.", b.key), en: format!("Beam '{}': no long_term_factor given, so only the immediate deflection is compared.", b.key) });
        }
        if row.status == "unchecked" {
            findings.push(Finding { ar: format!("الكمرة «{}»: لا حد للهبوط (limit_span_ratio) فلم يُحكم بنجاح أو فشل.", b.key), en: format!("Beam '{}': no deflection limit (limit_span_ratio) given, so no pass or fail is declared.", b.key) });
        }
    }

    let (storeys, cumulative) = storey_rows(&inp.storeys, e, inp.max_drift_ratio.map(|r| positive(r, "max_drift_ratio", "input")).transpose()?)?;
    if !storeys.is_empty() {
        findings.push(Finding { ar: "الانحراف: مبنى قص مثالي (أرضيات جاسئة وأعمدة مثبتة الطرفين) وقص الطابق من تحليلك الجانبي؛ لا يُحسب هنا زلزال ولا رياح.".into(), en: "Drift: idealized shear building (rigid floors, columns fixed at both ends) with the storey shear taken from your lateral analysis; no seismic or wind analysis is done here.".into() });
        if inp.max_drift_ratio.is_none() {
            findings.push(Finding { ar: "لم يُدخل حد نسبة الانحراف فلم يُحكم بنجاح أو فشل.".into(), en: "No max_drift_ratio given, so no pass or fail is declared for drift.".into() });
        }
    }

    let max_s = inp.max_settlement_mm.map(|m| positive(m, "max_settlement_mm", "input")).transpose()?;
    let mut settlements = Vec::new();
    let mut values = std::collections::BTreeMap::new();
    for f in &inp.footings {
        let (row, s) = settlement_row(f, max_s)?;
        values.insert(f.key.clone(), s);
        settlements.push(row);
    }
    let max_ang = inp.max_angular_distortion.map(|m| positive(m, "max_angular_distortion", "input")).transpose()?;
    let mut differentials = Vec::new();
    for p in &inp.pairs {
        let d = positive(p.distance_mm, "distance_mm", &format!("{}-{}", p.a, p.b))?;
        let (Some(sa), Some(sb)) = (values.get(&p.a), values.get(&p.b)) else {
            return Err(format!("pair '{}'-'{}' names a footing that is not in footings", p.a, p.b));
        };
        if p.a == p.b {
            return Err(format!("pair '{}'-'{}' names the same footing twice", p.a, p.b));
        }
        let diff = (sa - sb).abs();
        differentials.push(DifferentialRow { a: p.a.clone(), b: p.b.clone(), differential_mm: r3(diff), angular_distortion: r6(diff / d), status: verdict(diff / d, max_ang) });
    }
    if !settlements.is_empty() {
        findings.push(Finding { ar: "الهبوط: مرن فوري فقط بمعادلة q·B·(1−ν²)·Is/Es؛ بلا تماسك ولا زحف ولا تفاعل بين الأساسات. معامل التأثير ومعامل التربة من مدخلاتك.".into(), en: "Settlement: immediate elastic only, s = q B (1 - nu^2) Is / Es; no consolidation, no creep, no interaction between footings. The influence factor and soil modulus are your inputs.".into() });
        if max_s.is_none() {
            findings.push(Finding { ar: "لم يُدخل أقصى هبوط فلم يُحكم بنجاح أو فشل لكل أساس.".into(), en: "No max_settlement_mm given, so no pass or fail is declared per footing.".into() });
        }
        if !inp.pairs.is_empty() && max_ang.is_none() {
            findings.push(Finding { ar: "لم يُدخل أقصى انحناء زاوي فلم يُحكم على الهبوط التفاضلي.".into(), en: "No max_angular_distortion given, so no pass or fail is declared for differential settlement.".into() });
        }
    }
    findings.push(Finding { ar: "فحص خدمة أولي وليس تصميماً؛ يراجعه مهندس إنشائي مرخّص.".into(), en: "Preliminary serviceability screen, not a design; a licensed structural engineer must verify.".into() });
    Ok(DriftReport { beams, total_drift_mm: if storeys.is_empty() { None } else { Some(r3(cumulative)) }, storeys, settlements, differentials, findings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beam(support: &str) -> BeamIn {
        BeamIn { key: "b".into(), span_mm: 5000.0, b_mm: 300.0, h_mm: 500.0, support: support.into(), w_kn_m: 10.0, cracked_factor: None, long_term_factor: None, limit_span_ratio: None }
    }
    fn input_beam(b: BeamIn) -> DriftInput {
        DriftInput { e_mpa: Some(25000.0), beams: vec![b], ..Default::default() }
    }
    fn storey(key: &str, v: f64) -> StoreyIn {
        StoreyIn { key: key.into(), height_mm: 3000.0, shear_kn: v, columns: vec![ColumnGroup { count: 4, width_mm: 400.0, depth_mm: 400.0 }], stiffness_factor: None }
    }
    fn footing(key: &str, load: f64) -> FootingIn {
        FootingIn { key: key.into(), load_kn: load, width_mm: 2000.0, length_mm: 2000.0, es_mpa: 10.0, nu: 0.3, influence: 0.82 }
    }

    #[test]
    fn simple_fixed_and_cantilever_beams_use_their_closed_forms() {
        // EI = 25000 * 300*500^3/12 = 7.8125e13 N mm2.
        let simple = check(&input_beam(beam("simple"))).unwrap().beams[0].immediate_mm;
        assert_eq!(simple, 1.042);
        let fixed = check(&input_beam(beam("fixed"))).unwrap().beams[0].immediate_mm;
        assert_eq!(fixed, 0.208);
        let mut c = beam("cantilever");
        c.span_mm = 2000.0;
        assert_eq!(check(&input_beam(c)).unwrap().beams[0].immediate_mm, 0.256);
    }

    #[test]
    fn long_term_and_cracking_scale_the_deflection_and_the_limit_decides() {
        let mut b = beam("simple");
        b.cracked_factor = Some(0.5);
        b.long_term_factor = Some(2.0);
        b.limit_span_ratio = Some(360.0);
        let r = check(&input_beam(b.clone())).unwrap();
        let row = &r.beams[0];
        assert_eq!(row.immediate_mm, 2.083);
        assert_eq!(row.long_term_mm, Some(4.167));
        assert_eq!(row.allowed_mm, Some(13.889));
        assert_eq!(row.status, "ok");
        b.limit_span_ratio = Some(1500.0);
        assert_eq!(check(&input_beam(b)).unwrap().beams[0].status, "fails");
    }

    #[test]
    fn a_beam_without_a_limit_is_unchecked_and_says_why() {
        let r = check(&input_beam(beam("simple"))).unwrap();
        assert_eq!(r.beams[0].status, "unchecked");
        assert!(r.findings.iter().any(|f| f.en.contains("no pass or fail is declared")));
        assert!(r.findings.iter().any(|f| f.en.contains("gross section")));
    }

    #[test]
    fn storey_drift_comes_from_the_column_stiffness_and_accumulates() {
        // k = 4 * 12 * 25000 * (400^4/12) / 3000^3 = 94814.8 N/mm; V = 100 kN -> 1.055 mm.
        let inp = DriftInput { e_mpa: Some(25000.0), storeys: vec![storey("S1", 100.0), storey("S2", 50.0)], max_drift_ratio: Some(0.005), ..Default::default() };
        let r = check(&inp).unwrap();
        assert_eq!(r.storeys[0].drift_mm, 1.055);
        assert_eq!(r.storeys[0].stiffness_kn_per_mm, 94.815);
        assert_eq!(r.storeys[1].cumulative_mm, 1.582);
        assert_eq!(r.total_drift_mm, Some(1.582));
        assert!(r.storeys.iter().all(|s| s.status == "ok"));
        let tight = DriftInput { max_drift_ratio: Some(0.0001), ..inp };
        assert_eq!(check(&tight).unwrap().storeys[0].status, "fails");
    }

    #[test]
    fn cracked_columns_drift_more() {
        let mut s = storey("S1", 100.0);
        s.stiffness_factor = Some(0.5);
        let r = check(&DriftInput { e_mpa: Some(25000.0), storeys: vec![s], ..Default::default() }).unwrap();
        assert_eq!(r.storeys[0].drift_mm, 2.109);
        assert_eq!(r.storeys[0].status, "unchecked");
    }

    #[test]
    fn settlement_and_differential_settlement_follow_the_elastic_formula() {
        // q = 400/4 = 100 kPa; s = 0.1 * 2000 * 0.91 * 0.82 / 10 = 14.924 mm.
        let inp = DriftInput { footings: vec![footing("F1", 400.0), footing("F2", 200.0)], pairs: vec![PairIn { a: "F1".into(), b: "F2".into(), distance_mm: 5000.0 }], max_settlement_mm: Some(25.0), max_angular_distortion: Some(0.002), ..Default::default() };
        let r = check(&inp).unwrap();
        assert_eq!(r.settlements[0].pressure_kpa, 100.0);
        assert_eq!(r.settlements[0].settlement_mm, 14.924);
        assert_eq!(r.settlements[1].settlement_mm, 7.462);
        assert_eq!(r.differentials[0].differential_mm, 7.462);
        assert_eq!(r.differentials[0].angular_distortion, 0.001492);
        assert!(r.differentials[0].status == "ok" && r.settlements[0].status == "ok");
        let tight = DriftInput { max_angular_distortion: Some(0.001), max_settlement_mm: Some(10.0), ..inp };
        let r = check(&tight).unwrap();
        assert_eq!(r.differentials[0].status, "fails");
        assert_eq!((r.settlements[0].status, r.settlements[1].status), ("fails", "ok"));
    }

    #[test]
    fn missing_or_bad_inputs_are_refused_by_name() {
        let mut no_e = input_beam(beam("simple"));
        no_e.e_mpa = None;
        assert!(check(&no_e).unwrap_err().contains("e_mpa is required"));
        assert!(check(&DriftInput::default()).unwrap_err().contains("nothing to check"));
        assert!(check(&input_beam(beam("pinned"))).unwrap_err().contains("not one of simple, fixed, cantilever"));
        let mut b = beam("simple");
        b.h_mm = 0.0;
        assert!(check(&input_beam(b)).unwrap_err().contains("h_mm must be a positive number"));
        let mut b = beam("simple");
        b.cracked_factor = Some(1.5);
        assert!(check(&input_beam(b)).unwrap_err().contains("cracked_factor must be in (0, 1]"));
        let mut b = beam("simple");
        b.long_term_factor = Some(0.5);
        assert!(check(&input_beam(b)).unwrap_err().contains("long_term_factor must be 1 or more"));
        let mut s = storey("S1", 10.0);
        s.columns.clear();
        assert!(check(&DriftInput { e_mpa: Some(25000.0), storeys: vec![s], ..Default::default() }).unwrap_err().contains("has no columns"));
        let mut f = footing("F1", 100.0);
        f.nu = 0.5;
        assert!(check(&DriftInput { footings: vec![f], ..Default::default() }).unwrap_err().contains("nu must be in [0, 0.5)"));
        let pair = |a: &str, b: &str| PairIn { a: a.into(), b: b.into(), distance_mm: 3000.0 };
        let two = |p| DriftInput { footings: vec![footing("F1", 100.0), footing("F2", 100.0)], pairs: vec![p], ..Default::default() };
        assert!(check(&two(pair("F1", "F9"))).unwrap_err().contains("not in footings"));
        assert!(check(&two(pair("F1", "F1"))).unwrap_err().contains("same footing twice"));
        let dup = DriftInput { footings: vec![footing("F1", 100.0), footing("F1", 100.0)], ..Default::default() };
        assert!(check(&dup).unwrap_err().contains("used twice"));
    }
}
