//! Lateral load screening: equivalent-lateral-force seismic and velocity-pressure wind, as a parametric local engine.
//!
//! This module contains **no code table**. Every statutory or site value (spectral accelerations, response modification,
//! importance, period coefficients, gust factor, pressure coefficients, exposure coefficients per level, directionality and
//! topography factors, minimum base-shear factors) is an input, and `source` (code, edition, clause/table, page) is mandatory
//! for each analysis: an empty source or a missing value is refused by name and nothing is defaulted. The formulas are the
//! ones every ELF/velocity-pressure code shares; which code clause supplies the inputs is the caller's to cite.
//!
//! Units: kN, mm for geometry as given (`elevation_mm`, widths), kPa for pressure, seconds for period, m/s for speed.
//!
//! **Seismic (ELF).** `Cs = Sds / (R / Ie)`, bounded above by `Sd1 / (T (R / Ie))` for `T <= TL` and
//! `Sd1 TL / (T^2 (R / Ie))` beyond, and below by the minima the caller supplies (absent minima are reported as not applied).
//! `V = Cs W`; distribution `Fx = V w_x h_x^k / sum(w_i h_i^k)` with the exponent `k` supplied. The period is either given or
//! the approximate `Ta = Ct hn^x` from supplied `ct`, `x` with `hn` in metres.
//!
//! **Wind (velocity pressure).** `q = 0.5 rho V^2 Kz Kzt Kd` (kPa with V in m/s and rho in kg/m^3, divided by 1000), with the air
//! density supplied. Net pressure along a direction is `G (q_z Cp_w - q_h Cp_l)` (internal pressure cancels in the net),
//! `q_h` taken at the top level. Each level takes the strip from the mid-height below to the mid-height above (the top level
//! up to its own elevation, no parapet); the strip from the ground to mid-height of the first level goes straight to the
//! foundation and is reported separately, so base shear = sum of level forces + that strip.
//!
//! The storey shears returned are the ones `drift` takes as `shear_kn` for the storey below each level. Output is a
//! preliminary load screen, not a design, not a code check, and does not replace a licensed engineer.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LevelIn {
    pub key: String,
    /// Height of this level above the base, mm.
    pub elevation_mm: f64,
    /// Seismic weight tributary to this level, kN. Required when `seismic` is given.
    #[serde(default)]
    pub weight_kn: Option<f64>,
    /// Exposure coefficient `Kz` at this level. Required when `wind` is given.
    #[serde(default)]
    pub kz: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SeismicIn {
    /// Code, edition, clause/table and page the inputs below came from.
    pub source: String,
    pub sds: Option<f64>,
    pub sd1: Option<f64>,
    pub r: Option<f64>,
    pub ie: Option<f64>,
    /// Long-period transition period `TL`, s.
    pub tl_s: Option<f64>,
    /// Distribution exponent `k`.
    pub k: Option<f64>,
    /// Fundamental period, s. Absent: `ct` and `x` give the approximate period.
    #[serde(default)]
    pub period_s: Option<f64>,
    #[serde(default)]
    pub ct: Option<f64>,
    #[serde(default)]
    pub x: Option<f64>,
    /// Lower bound `Cs >= cs_min_sds_factor * Sds * Ie`. Absent: not applied, and the report says so.
    #[serde(default)]
    pub cs_min_sds_factor: Option<f64>,
    /// Lower bound `Cs >= cs_min_abs`. Absent: not applied, and the report says so.
    #[serde(default)]
    pub cs_min_abs: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DirectionIn {
    pub name: String,
    /// Width of the face the wind strikes, mm.
    pub face_width_mm: f64,
    pub cp_windward: Option<f64>,
    /// Signed leeward coefficient (suction is negative).
    pub cp_leeward: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WindIn {
    pub source: String,
    pub basic_speed_m_s: Option<f64>,
    pub air_density_kg_m3: Option<f64>,
    pub kd: Option<f64>,
    pub kzt: Option<f64>,
    pub gust: Option<f64>,
    pub directions: Vec<DirectionIn>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LateralInput {
    pub levels: Vec<LevelIn>,
    #[serde(default)]
    pub seismic: Option<SeismicIn>,
    #[serde(default)]
    pub wind: Option<WindIn>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub ar: String,
    pub en: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LevelForce {
    pub key: String,
    pub elevation_mm: f64,
    /// Height of the storey below this level, mm (feeds `drift`).
    pub storey_height_mm: f64,
    pub force_kn: f64,
    /// Shear in the storey below this level, kN (feeds `drift` as `shear_kn`).
    pub storey_shear_kn: f64,
    /// Overturning moment at this level's elevation from the forces above it, kN·m.
    pub overturning_knm: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SeismicReport {
    pub source: String,
    pub weight_kn: f64,
    pub period_s: f64,
    pub period_origin: String,
    pub cs_short_period: f64,
    pub cs_upper_limit: f64,
    pub cs: f64,
    pub cs_governed_by: String,
    pub base_shear_kn: f64,
    pub base_overturning_knm: f64,
    pub levels: Vec<LevelForce>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WindDirectionReport {
    pub name: String,
    pub ground_strip_kn: f64,
    pub base_shear_kn: f64,
    pub base_overturning_knm: f64,
    pub levels: Vec<LevelForce>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WindReport {
    pub source: String,
    /// Velocity pressure at each level, kPa (before gust and Cp).
    pub velocity_pressure_kpa: Vec<(String, f64)>,
    pub directions: Vec<WindDirectionReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LateralReport {
    pub seismic: Option<SeismicReport>,
    pub wind: Option<WindReport>,
    pub findings: Vec<Finding>,
}

fn req(v: Option<f64>, name: &str, positive: bool) -> Result<f64, String> {
    let x = v.ok_or_else(|| format!("{name} is required (no default is assumed) / {name} مطلوب ولا قيمة افتراضية"))?;
    if !x.is_finite() || (positive && x <= 0.0) {
        return Err(format!("{name} must be a finite {} number / {name} يجب أن يكون رقماً {}", if positive { "positive" } else { "" }, if positive { "موجباً" } else { "منتهياً" }));
    }
    Ok(x)
}

fn need_source(s: &str, what: &str) -> Result<(), String> {
    if s.trim().is_empty() {
        return Err(format!("{what}.source is required: name the code, edition, clause/table and page of the inputs / {what}.source مطلوب: اذكر الكود والإصدار والبند والصفحة"));
    }
    Ok(())
}

fn sorted_levels(levels: &[LevelIn]) -> Result<Vec<&LevelIn>, String> {
    if levels.is_empty() {
        return Err("levels is empty: nothing to analyse / لا مستويات للتحليل".into());
    }
    let mut v: Vec<&LevelIn> = levels.iter().collect();
    for l in &v {
        if l.key.trim().is_empty() {
            return Err("a level has an empty key / مستوى بلا اسم".into());
        }
        if !l.elevation_mm.is_finite() || l.elevation_mm <= 0.0 {
            return Err(format!("level '{}': elevation_mm must be above the base (positive) / منسوب المستوى يجب أن يكون موجباً", l.key));
        }
    }
    v.sort_by(|a, b| a.elevation_mm.total_cmp(&b.elevation_mm));
    for w in v.windows(2) {
        if (w[1].elevation_mm - w[0].elevation_mm).abs() < 1e-9 {
            return Err(format!("levels '{}' and '{}' share one elevation / مستويان بمنسوب واحد", w[0].key, w[1].key));
        }
    }
    Ok(v)
}

/// Builds per-level forces from forces `f` (kN, ascending elevation): storey shears from the top down and overturning moments.
fn accumulate(levels: &[&LevelIn], f: &[f64]) -> Vec<LevelForce> {
    let n = levels.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let shear: f64 = f[i..].iter().sum();
        let ei = levels[i].elevation_mm;
        let overturning: f64 = (i..n).map(|j| f[j] * (levels[j].elevation_mm - ei) / 1000.0).sum();
        let below = if i == 0 { 0.0 } else { levels[i - 1].elevation_mm };
        out.push(LevelForce { key: levels[i].key.clone(), elevation_mm: ei, storey_height_mm: ei - below, force_kn: f[i], storey_shear_kn: shear, overturning_knm: overturning });
    }
    out
}

fn seismic(levels: &[&LevelIn], s: &SeismicIn, findings: &mut Vec<Finding>) -> Result<SeismicReport, String> {
    need_source(&s.source, "seismic")?;
    let sds = req(s.sds, "seismic.sds", true)?;
    let sd1 = req(s.sd1, "seismic.sd1", true)?;
    let r = req(s.r, "seismic.r", true)?;
    let ie = req(s.ie, "seismic.ie", true)?;
    let tl = req(s.tl_s, "seismic.tl_s", true)?;
    let k = req(s.k, "seismic.k", true)?;
    let mut w = Vec::with_capacity(levels.len());
    for l in levels {
        w.push(req(l.weight_kn, &format!("level '{}' weight_kn", l.key), true)?);
    }
    let total_w: f64 = w.iter().sum();
    let hn_m = levels[levels.len() - 1].elevation_mm / 1000.0;
    let (t, origin) = match s.period_s {
        Some(_) => (req(s.period_s, "seismic.period_s", true)?, "given period_s".to_string()),
        None => {
            let ct = req(s.ct, "seismic.ct (or give seismic.period_s)", true)?;
            let x = req(s.x, "seismic.x (or give seismic.period_s)", true)?;
            (ct * hn_m.powf(x), format!("approximate Ta = Ct*hn^x, hn = {hn_m:.3} m"))
        }
    };
    let ratio = r / ie;
    let cs_short = sds / ratio;
    let cs_upper = if t <= tl { sd1 / (t * ratio) } else { sd1 * tl / (t * t * ratio) };
    let mut cs = cs_short.min(cs_upper);
    let mut governed = if cs_upper < cs_short { "upper limit (Sd1)" } else { "short-period plateau (Sds)" }.to_string();
    match s.cs_min_sds_factor {
        Some(_) => {
            let f = req(s.cs_min_sds_factor, "seismic.cs_min_sds_factor", true)?;
            let m = f * sds * ie;
            if m > cs {
                cs = m;
                governed = "minimum (cs_min_sds_factor)".into();
            }
        }
        None => findings.push(Finding { ar: "الزلزال: لم يُدخل معامل الحد الأدنى بدلالة Sds فلم يُطبَّق، وقد يكون القص القاعدي أقل مما يشترطه الكود.".into(), en: "Seismic: no cs_min_sds_factor given, so that lower bound is not applied and the base shear may be below what the code requires.".into() }),
    }
    match s.cs_min_abs {
        Some(_) => {
            let m = req(s.cs_min_abs, "seismic.cs_min_abs", true)?;
            if m > cs {
                cs = m;
                governed = "minimum (cs_min_abs)".into();
            }
        }
        None => findings.push(Finding { ar: "الزلزال: لم يُدخل الحد الأدنى المطلق لـ Cs فلم يُطبَّق.".into(), en: "Seismic: no cs_min_abs given, so that lower bound is not applied.".into() }),
    }
    let v = cs * total_w;
    let wh: Vec<f64> = levels.iter().zip(&w).map(|(l, wi)| wi * (l.elevation_mm / 1000.0).powf(k)).collect();
    let denom: f64 = wh.iter().sum();
    let f: Vec<f64> = wh.iter().map(|x| v * x / denom).collect();
    let lv = accumulate(levels, &f);
    findings.push(Finding { ar: "الزلزال: قوة جانبية مكافئة بلا فحص انتظام ولا لي ولا تأثير P-Δ ولا تحليل ديناميكي؛ المدخلات كلها من المصدر المذكور.".into(), en: "Seismic: equivalent lateral force only; no irregularity, torsion, P-Delta or dynamic analysis. All inputs come from the cited source.".into() });
    Ok(SeismicReport { source: s.source.clone(), weight_kn: total_w, period_s: t, period_origin: origin, cs_short_period: cs_short, cs_upper_limit: cs_upper, cs, cs_governed_by: governed, base_shear_kn: v, base_overturning_knm: f.iter().zip(levels).map(|(fi, l)| fi * l.elevation_mm / 1000.0).sum(), levels: lv })
}

fn wind(levels: &[&LevelIn], wi: &WindIn, findings: &mut Vec<Finding>) -> Result<WindReport, String> {
    need_source(&wi.source, "wind")?;
    let speed = req(wi.basic_speed_m_s, "wind.basic_speed_m_s", true)?;
    let rho = req(wi.air_density_kg_m3, "wind.air_density_kg_m3", true)?;
    let kd = req(wi.kd, "wind.kd", true)?;
    let kzt = req(wi.kzt, "wind.kzt", true)?;
    let g = req(wi.gust, "wind.gust", true)?;
    if wi.directions.is_empty() {
        return Err("wind.directions is empty: give at least one direction with its face width and Cp values / لا اتجاهات للرياح".into());
    }
    let mut q = Vec::with_capacity(levels.len());
    for l in levels {
        let kz = req(l.kz, &format!("level '{}' kz", l.key), true)?;
        q.push(0.5 * rho * speed * speed * kz * kzt * kd / 1000.0);
    }
    let n = levels.len();
    let e: Vec<f64> = levels.iter().map(|l| l.elevation_mm / 1000.0).collect();
    let qh = q[n - 1];
    let mut dirs = Vec::new();
    for d in &wi.directions {
        if d.name.trim().is_empty() {
            return Err("a wind direction has an empty name / اتجاه رياح بلا اسم".into());
        }
        if !d.face_width_mm.is_finite() || d.face_width_mm <= 0.0 {
            return Err(format!("wind direction '{}': face_width_mm must be positive / عرض الواجهة يجب أن يكون موجباً", d.name));
        }
        let cpw = req(d.cp_windward, &format!("wind direction '{}' cp_windward", d.name), false)?;
        let cpl = req(d.cp_leeward, &format!("wind direction '{}' cp_leeward", d.name), false)?;
        let b = d.face_width_mm / 1000.0;
        let mut f = Vec::with_capacity(n);
        for i in 0..n {
            let lo = if i == 0 { e[0] / 2.0 } else { (e[i - 1] + e[i]) / 2.0 };
            let hi = if i + 1 == n { e[i] } else { (e[i] + e[i + 1]) / 2.0 };
            let p = g * (q[i] * cpw - qh * cpl);
            f.push(p * b * (hi - lo));
        }
        let ground = g * (q[0] * cpw - qh * cpl) * b * (e[0] / 2.0);
        let lv = accumulate(levels, &f);
        let total: f64 = f.iter().sum::<f64>() + ground;
        let ot: f64 = (0..n).map(|j| f[j] * e[j]).sum::<f64>() + ground * (e[0] / 4.0);
        dirs.push(WindDirectionReport { name: d.name.clone(), ground_strip_kn: ground, base_shear_kn: total, base_overturning_knm: ot, levels: lv });
    }
    findings.push(Finding { ar: "الرياح: ضغط سرعة وCp من مدخلاتك على مبنى مغلق مستطيل؛ بلا أحمال أسطح ولا مكوّنات وكسوات ولا تأثير الدوامات ولا فحص رد الفعل الديناميكي.".into(), en: "Wind: velocity pressure and Cp from your inputs on an enclosed rectangular building; no roof, components-and-cladding, vortex-shedding or dynamic-response checks.".into() });
    Ok(WindReport { source: wi.source.clone(), velocity_pressure_kpa: levels.iter().map(|l| l.key.clone()).zip(q).collect(), directions: dirs })
}

pub fn analyse(inp: &LateralInput) -> Result<LateralReport, String> {
    if inp.seismic.is_none() && inp.wind.is_none() {
        return Err("nothing to analyse: give seismic and/or wind / لا شيء للتحليل: أدخل seismic أو wind".into());
    }
    let levels = sorted_levels(&inp.levels)?;
    let mut findings = Vec::new();
    let seismic = match &inp.seismic {
        Some(s) => Some(seismic(&levels, s, &mut findings)?),
        None => None,
    };
    let wind = match &inp.wind {
        Some(w) => Some(wind(&levels, w, &mut findings)?),
        None => None,
    };
    findings.push(Finding { ar: "حمل جانبي أولي وليس تصميماً؛ يعتمده مهندس إنشائي مرخّص ويطابقه مع الكود المعتمد من الجهة.".into(), en: "Preliminary lateral load screen, not a design; a licensed structural engineer must verify it against the code the authority adopts.".into() });
    Ok(LateralReport { seismic, wind, findings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lv(key: &str, e: f64, w: Option<f64>, kz: Option<f64>) -> LevelIn {
        LevelIn { key: key.into(), elevation_mm: e, weight_kn: w, kz }
    }

    fn seis() -> SeismicIn {
        SeismicIn { source: "test code, ed. X, table Y, p.1".into(), sds: Some(0.5), sd1: Some(0.1), r: Some(4.0), ie: Some(1.0), tl_s: Some(8.0), k: Some(1.0), period_s: Some(0.4), ct: None, x: None, cs_min_sds_factor: None, cs_min_abs: None }
    }

    #[test]
    fn seismic_base_shear_distribution_and_overturning_match_hand_values() {
        let inp = LateralInput { levels: vec![lv("L2", 6000.0, Some(1000.0), None), lv("L1", 3000.0, Some(1000.0), None)], seismic: Some(seis()), wind: None };
        let r = analyse(&inp).unwrap().seismic.unwrap();
        assert!((r.cs - 0.0625).abs() < 1e-12, "Sd1 limit governs: {}", r.cs);
        assert!((r.base_shear_kn - 125.0).abs() < 1e-9);
        assert!((r.levels[0].force_kn - 125.0 / 3.0).abs() < 1e-9);
        assert!((r.levels[1].force_kn - 250.0 / 3.0).abs() < 1e-9);
        assert!((r.levels[1].storey_shear_kn - 250.0 / 3.0).abs() < 1e-9);
        assert!((r.levels[0].storey_shear_kn - 125.0).abs() < 1e-9);
        assert!((r.base_overturning_knm - 625.0).abs() < 1e-9, "{}", r.base_overturning_knm);
        assert!((r.levels[1].overturning_knm - 0.0).abs() < 1e-9);
        assert_eq!(r.levels[1].storey_height_mm, 3000.0);
    }

    #[test]
    fn seismic_period_beyond_tl_and_minima_apply() {
        let mut s = seis();
        s.period_s = Some(10.0);
        s.cs_min_abs = Some(0.05);
        let inp = LateralInput { levels: vec![lv("L1", 3000.0, Some(100.0), None)], seismic: Some(s), wind: None };
        let r = analyse(&inp).unwrap().seismic.unwrap();
        // Sd1*TL/(T^2*R) = 0.1*8/(100*4) = 0.002 < 0.05 minimum.
        assert!((r.cs_upper_limit - 0.002).abs() < 1e-12);
        assert!((r.cs - 0.05).abs() < 1e-12);
        assert!(r.cs_governed_by.contains("cs_min_abs"));
    }

    #[test]
    fn seismic_approximate_period_needs_ct_and_x_and_is_reported() {
        let mut s = seis();
        s.period_s = None;
        let inp = LateralInput { levels: vec![lv("L1", 4000.0, Some(100.0), None)], seismic: Some(s.clone()), wind: None };
        assert!(analyse(&inp).unwrap_err().contains("seismic.ct"));
        s.ct = Some(0.1);
        s.x = Some(0.75);
        let inp = LateralInput { levels: vec![lv("L1", 4000.0, Some(100.0), None)], seismic: Some(s), wind: None };
        let r = analyse(&inp).unwrap().seismic.unwrap();
        assert!((r.period_s - 0.1 * 4.0_f64.powf(0.75)).abs() < 1e-12);
        assert!(r.period_origin.contains("Ct*hn^x"));
    }

    #[test]
    fn missing_inputs_and_empty_source_are_refused_by_name() {
        let base = |s: SeismicIn| LateralInput { levels: vec![lv("L1", 3000.0, Some(10.0), None)], seismic: Some(s), wind: None };
        let mut s = seis();
        s.source = "  ".into();
        assert!(analyse(&base(s)).unwrap_err().contains("seismic.source"));
        let mut s = seis();
        s.sds = None;
        assert!(analyse(&base(s)).unwrap_err().contains("seismic.sds"));
        let mut s = seis();
        s.k = None;
        assert!(analyse(&base(s)).unwrap_err().contains("seismic.k"));
        let no_w = LateralInput { levels: vec![lv("L1", 3000.0, None, None)], seismic: Some(seis()), wind: None };
        assert!(analyse(&no_w).unwrap_err().contains("weight_kn"));
        assert!(analyse(&LateralInput { levels: vec![], seismic: Some(seis()), wind: None }).unwrap_err().contains("levels"));
        assert!(analyse(&LateralInput { levels: vec![lv("L1", 3000.0, Some(1.0), None)], seismic: None, wind: None }).unwrap_err().contains("nothing to analyse"));
        let dup = LateralInput { levels: vec![lv("A", 3000.0, Some(1.0), None), lv("B", 3000.0, Some(1.0), None)], seismic: Some(seis()), wind: None };
        assert!(analyse(&dup).unwrap_err().contains("share one elevation"));
    }

    #[test]
    fn missing_minima_are_reported_not_silently_skipped() {
        let inp = LateralInput { levels: vec![lv("L1", 3000.0, Some(10.0), None)], seismic: Some(seis()), wind: None };
        let r = analyse(&inp).unwrap();
        assert!(r.findings.iter().any(|f| f.en.contains("cs_min_sds_factor")));
        assert!(r.findings.iter().any(|f| f.en.contains("cs_min_abs")));
    }

    fn wind_in() -> WindIn {
        WindIn {
            source: "test code, ed. X, ch. Y, p.2".into(),
            basic_speed_m_s: Some(40.0),
            air_density_kg_m3: Some(1.2),
            kd: Some(1.0),
            kzt: Some(1.0),
            gust: Some(1.0),
            directions: vec![DirectionIn { name: "X".into(), face_width_mm: 10_000.0, cp_windward: Some(0.8), cp_leeward: Some(-0.5) }],
        }
    }

    #[test]
    fn wind_single_level_total_equals_net_pressure_times_face_area() {
        let inp = LateralInput { levels: vec![lv("L1", 3000.0, None, Some(1.0))], seismic: None, wind: Some(wind_in()) };
        let r = analyse(&inp).unwrap().wind.unwrap();
        assert!((r.velocity_pressure_kpa[0].1 - 0.96).abs() < 1e-12);
        let d = &r.directions[0];
        // p = 0.96*0.8 + 0.96*0.5 = 1.248 kPa over 10 m x 3 m.
        assert!((d.base_shear_kn - 1.248 * 10.0 * 3.0).abs() < 1e-9, "{}", d.base_shear_kn);
        assert!((d.levels[0].force_kn - 18.72).abs() < 1e-9);
        assert!((d.ground_strip_kn - 18.72).abs() < 1e-9);
        // 18.72*3 (level) + 18.72*0.75 (ground strip centroid) = 70.2 kN·m.
        assert!((d.base_overturning_knm - 70.2).abs() < 1e-9, "{}", d.base_overturning_knm);
    }

    #[test]
    fn wind_two_levels_tributary_strips_sum_to_the_whole_face() {
        let mut w = wind_in();
        w.directions[0].cp_leeward = Some(-0.5);
        let inp = LateralInput { levels: vec![lv("L1", 3000.0, None, Some(1.0)), lv("L2", 6000.0, None, Some(1.0))], seismic: None, wind: Some(w) };
        let d = analyse(&inp).unwrap().wind.unwrap().directions.remove(0);
        assert!((d.base_shear_kn - 1.248 * 10.0 * 6.0).abs() < 1e-9, "{}", d.base_shear_kn);
        assert!((d.levels[0].force_kn - 1.248 * 10.0 * 3.0).abs() < 1e-9);
        assert!((d.levels[1].force_kn - 1.248 * 10.0 * 1.5).abs() < 1e-9);
        assert!((d.levels[0].storey_shear_kn - (d.levels[0].force_kn + d.levels[1].force_kn)).abs() < 1e-9);
    }

    #[test]
    fn wind_refuses_missing_values_by_name() {
        let mk = |w: WindIn, kz: Option<f64>| LateralInput { levels: vec![lv("L1", 3000.0, None, kz)], seismic: None, wind: Some(w) };
        assert!(analyse(&mk(wind_in(), None)).unwrap_err().contains("kz"));
        let mut w = wind_in();
        w.gust = None;
        assert!(analyse(&mk(w, Some(1.0))).unwrap_err().contains("wind.gust"));
        let mut w = wind_in();
        w.air_density_kg_m3 = None;
        assert!(analyse(&mk(w, Some(1.0))).unwrap_err().contains("air_density"));
        let mut w = wind_in();
        w.directions[0].cp_leeward = None;
        assert!(analyse(&mk(w, Some(1.0))).unwrap_err().contains("cp_leeward"));
        let mut w = wind_in();
        w.directions.clear();
        assert!(analyse(&mk(w, Some(1.0))).unwrap_err().contains("directions"));
        let mut w = wind_in();
        w.source.clear();
        assert!(analyse(&mk(w, Some(1.0))).unwrap_err().contains("wind.source"));
    }
}
