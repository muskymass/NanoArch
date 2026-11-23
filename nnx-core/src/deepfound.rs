//! Mat (raft) foundation and pile-group screening.
//!
//! Nothing is assumed: soil capacity, pile capacity, spacing and coverage limits are inputs the caller supplies with their own
//! source. A missing required value is refused by name; a missing limit leaves that check `unchecked` (never `ok`).
//!
//! - **Need screen** (`mat_screen`): the plan area of the isolated footings the caller sized (for example from `sizing`)
//!   against the building plan area. If the caller gives a maximum coverage ratio the verdict is `mat_indicated` or
//!   `isolated_ok`; without it the ratio is reported and the verdict is `unchecked`.
//! - **Mat** (`mat`): rigid-mat method. Column loads (and optional base moments) are reduced to the mat centroid; the bearing
//!   pressure is the linear distribution `q = P/A + Mx*y/Ix + My*x/Iy` evaluated at the four corners. A negative corner pressure
//!   is reported as `uplift` (no redistribution is invented). Self-weight is added only when both thickness and unit weight
//!   are given.
//! - **Pile cap** (`piles`): rigid cap, `R_i = P/n + Mx*y_i/sum(y^2) + My*x_i/sum(x^2)` about the group centroid. The layout is
//!   either given as positions, or generated as a centred grid of `ceil(P / capacity)` piles at the caller's spacing (more piles
//!   are never added automatically if the max reaction exceeds the capacity: the check reports `fails`).
//!
//! Sign convention: `mx_knm` is the moment about the x axis that raises pressure (or pile load) toward +y, `my_knm` toward +x.
//! Loads are service loads in kN; lengths are whole millimetres; pressures are kPa.
//!
//! Output is a preliminary screen, not a design: no punching shear, flexure, settlement, pile group efficiency, negative skin
//! friction or lateral load is checked here.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ColumnLoad {
    pub key: String,
    pub x_mm: f64,
    pub y_mm: f64,
    /// Service axial load, kN (compression positive).
    pub load_kn: f64,
    #[serde(default)]
    pub mx_knm: f64,
    #[serde(default)]
    pub my_knm: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MatIn {
    pub x_mm: f64,
    pub y_mm: f64,
    pub w_mm: f64,
    pub l_mm: f64,
    /// With `unit_weight_kn_m3`, adds the mat's self-weight. Either alone is refused.
    #[serde(default)]
    pub thickness_mm: Option<f64>,
    #[serde(default)]
    pub unit_weight_kn_m3: Option<f64>,
    /// Allowable bearing pressure, kPa, from the caller's soil report. Absent: `unchecked`.
    #[serde(default)]
    pub allowable_kpa: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ScreenIn {
    /// Total plan area of the isolated footings, m2.
    pub isolated_area_m2: f64,
    /// Building plan area, m2.
    pub building_area_m2: f64,
    /// Coverage above which a mat is indicated. Absent: `unchecked`.
    #[serde(default)]
    pub max_coverage_ratio: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PileIn {
    pub key: String,
    pub load_kn: f64,
    #[serde(default)]
    pub mx_knm: f64,
    #[serde(default)]
    pub my_knm: f64,
    /// Allowable compression capacity of one pile, kN, from the caller's pile design or test.
    pub capacity_kn: f64,
    /// Allowable uplift capacity of one pile, kN. Absent: any tension is `unchecked`.
    #[serde(default)]
    pub uplift_capacity_kn: Option<f64>,
    /// Centre-to-centre spacing for a generated grid, mm. Required when `positions` is absent.
    #[serde(default)]
    pub spacing_mm: Option<f64>,
    /// Given pile positions `[x, y]` in mm relative to the load point (the cap origin).
    #[serde(default)]
    pub positions: Option<Vec<[f64; 2]>>,
    /// Smallest allowed centre-to-centre spacing, mm. Absent: spacing `unchecked`.
    #[serde(default)]
    pub min_spacing_mm: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DeepInput {
    #[serde(default)]
    pub screen: Option<ScreenIn>,
    #[serde(default)]
    pub mat: Option<MatIn>,
    /// Column loads for the mat.
    #[serde(default)]
    pub columns: Vec<ColumnLoad>,
    #[serde(default)]
    pub piles: Vec<PileIn>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScreenReport {
    pub coverage_ratio: f64,
    pub verdict: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Corner {
    pub x_mm: f64,
    pub y_mm: f64,
    pub pressure_kpa: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MatReport {
    pub area_m2: f64,
    pub column_load_kn: f64,
    pub self_weight_kn: f64,
    pub total_load_kn: f64,
    /// Resultant offset from the mat centre, mm.
    pub ex_mm: f64,
    pub ey_mm: f64,
    pub corners: Vec<Corner>,
    pub max_kpa: f64,
    pub min_kpa: f64,
    pub allowed_kpa: Option<f64>,
    pub status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PileRow {
    pub x_mm: f64,
    pub y_mm: f64,
    pub reaction_kn: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PileReport {
    pub key: String,
    pub required_count: u32,
    pub count: u32,
    pub generated: bool,
    pub piles: Vec<PileRow>,
    pub max_kn: f64,
    pub min_kn: f64,
    pub min_distance_mm: Option<f64>,
    pub spacing_status: &'static str,
    pub status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub ar: String,
    pub en: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DeepReport {
    pub screen: Option<ScreenReport>,
    pub mat: Option<MatReport>,
    pub piles: Vec<PileReport>,
    pub findings: Vec<Finding>,
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn positive(v: f64, field: &str, key: &str) -> Result<f64, String> {
    if v.is_finite() && v > 0.0 {
        Ok(v)
    } else {
        Err(format!("'{key}': {field} must be a positive number, got {v}"))
    }
}

fn finite(v: f64, field: &str, key: &str) -> Result<f64, String> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(format!("'{key}': {field} must be a finite number, got {v}"))
    }
}

fn screen(s: &ScreenIn, findings: &mut Vec<Finding>) -> Result<ScreenReport, String> {
    let iso = positive(s.isolated_area_m2, "isolated_area_m2", "screen")?;
    let bld = positive(s.building_area_m2, "building_area_m2", "screen")?;
    let ratio = iso / bld;
    let verdict = match s.max_coverage_ratio {
        None => {
            findings.push(Finding { ar: "لم يُدخل حد نسبة التغطية فلم يُحكم بالحاجة إلى حصيرة.".into(), en: "No max_coverage_ratio given, so no verdict on whether a mat is needed.".into() });
            "unchecked"
        }
        Some(m) => {
            let m = positive(m, "max_coverage_ratio", "screen")?;
            if ratio > m {
                "mat_indicated"
            } else {
                "isolated_ok"
            }
        }
    };
    Ok(ScreenReport { coverage_ratio: r3(ratio), verdict })
}

fn mat(m: &MatIn, cols: &[ColumnLoad], findings: &mut Vec<Finding>) -> Result<MatReport, String> {
    let (w, l) = (positive(m.w_mm, "w_mm", "mat")?, positive(m.l_mm, "l_mm", "mat")?);
    finite(m.x_mm, "x_mm", "mat")?;
    finite(m.y_mm, "y_mm", "mat")?;
    if cols.is_empty() {
        return Err("mat: columns are required (each with key, x_mm, y_mm, load_kn)".into());
    }
    let (cx, cy) = (m.x_mm + w / 2.0, m.y_mm + l / 2.0);
    let area = w * l / 1e6;
    let mut p = 0.0;
    let (mut mx, mut my) = (0.0, 0.0);
    let mut seen = std::collections::BTreeSet::new();
    for c in cols {
        if !seen.insert(c.key.as_str()) {
            return Err(format!("column key '{}' is used twice", c.key));
        }
        finite(c.x_mm, "x_mm", &c.key)?;
        finite(c.y_mm, "y_mm", &c.key)?;
        finite(c.mx_knm, "mx_knm", &c.key)?;
        finite(c.my_knm, "my_knm", &c.key)?;
        let load = positive(c.load_kn, "load_kn", &c.key)?;
        if c.x_mm < m.x_mm || c.x_mm > m.x_mm + w || c.y_mm < m.y_mm || c.y_mm > m.y_mm + l {
            return Err(format!("column '{}' lies outside the mat", c.key));
        }
        p += load;
        mx += load * (c.y_mm - cy) / 1000.0 + c.mx_knm;
        my += load * (c.x_mm - cx) / 1000.0 + c.my_knm;
    }
    let sw = match (m.thickness_mm, m.unit_weight_kn_m3) {
        (None, None) => {
            findings.push(Finding { ar: "الوزن الذاتي للحصيرة غير محسوب: لم يُدخل السُّمك والوزن الحجمي.".into(), en: "Mat self-weight is not included: thickness_mm and unit_weight_kn_m3 were not given.".into() });
            0.0
        }
        (Some(t), Some(g)) => area * positive(t, "thickness_mm", "mat")? / 1000.0 * positive(g, "unit_weight_kn_m3", "mat")?,
        _ => return Err("mat: thickness_mm and unit_weight_kn_m3 must be given together".into()),
    };
    let total = p + sw;
    // Self-weight acts at the mat centre, so it adds no moment about it.
    let (wm, lm) = (w / 1000.0, l / 1000.0);
    let ix = wm * lm.powi(3) / 12.0;
    let iy = lm * wm.powi(3) / 12.0;
    let q_at = |x_m: f64, y_m: f64| total / area + mx * y_m / ix + my * x_m / iy;
    let mut corners = Vec::new();
    for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        corners.push(Corner { x_mm: cx + sx * w / 2.0, y_mm: cy + sy * l / 2.0, pressure_kpa: r3(q_at(sx * wm / 2.0, sy * lm / 2.0)) });
    }
    let max = corners.iter().map(|c| c.pressure_kpa).fold(f64::MIN, f64::max);
    let min = corners.iter().map(|c| c.pressure_kpa).fold(f64::MAX, f64::min);
    let allowed = m.allowable_kpa.map(|a| positive(a, "allowable_kpa", "mat")).transpose()?;
    let status = if min < 0.0 {
        findings.push(Finding { ar: "ضغط سالب عند ركن الحصيرة: رفع جزئي، والتوزيع الخطي لا ينطبق ويلزم إعادة ترتيب الحصيرة أو الأحمال.".into(), en: "Negative corner pressure on the mat: partial uplift; the linear distribution does not apply, so change the mat extent or the loads.".into() });
        "uplift"
    } else {
        match allowed {
            None => {
                findings.push(Finding { ar: "لم تُدخل قدرة التحمل المسموحة فلم يُحكم بنجاح أو فشل.".into(), en: "No allowable_kpa given, so no pass or fail is declared for bearing.".into() });
                "unchecked"
            }
            Some(a) if max <= a => "ok",
            Some(_) => "fails",
        }
    };
    findings.push(Finding { ar: "الحصيرة جاسئة بتوزيع خطي للضغط؛ لا قص ثاقب ولا انعطاف ولا هبوط هنا.".into(), en: "Rigid mat with linear pressure; no punching shear, flexure or settlement is checked here.".into() });
    Ok(MatReport { area_m2: r3(area), column_load_kn: r3(p), self_weight_kn: r3(sw), total_load_kn: r3(total), ex_mm: r3(my / total * 1000.0), ey_mm: r3(mx / total * 1000.0), corners, max_kpa: max, min_kpa: min, allowed_kpa: allowed, status })
}

fn grid(n: u32, spacing: f64) -> Vec<[f64; 2]> {
    let cols = (n as f64).sqrt().ceil() as u32;
    let rows = n.div_ceil(cols);
    let mut v = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            if v.len() as u32 == n {
                break;
            }
            v.push([c as f64 * spacing, r as f64 * spacing]);
        }
    }
    v
}

fn pile_cap(p: &PileIn, findings: &mut Vec<Finding>) -> Result<PileReport, String> {
    let load = positive(p.load_kn, "load_kn", &p.key)?;
    let cap = positive(p.capacity_kn, "capacity_kn", &p.key)?;
    finite(p.mx_knm, "mx_knm", &p.key)?;
    finite(p.my_knm, "my_knm", &p.key)?;
    let required = (load / cap).ceil() as u32;
    let (pos, generated) = match &p.positions {
        Some(v) if !v.is_empty() => {
            for q in v {
                finite(q[0], "positions", &p.key)?;
                finite(q[1], "positions", &p.key)?;
            }
            (v.clone(), false)
        }
        Some(_) => return Err(format!("'{}': positions is empty", p.key)),
        None => {
            let s = p.spacing_mm.ok_or_else(|| format!("'{}': spacing_mm is required when positions is not given", p.key))?;
            (grid(required, positive(s, "spacing_mm", &p.key)?), true)
        }
    };
    let n = pos.len() as f64;
    let (gx, gy) = (pos.iter().map(|q| q[0]).sum::<f64>() / n, pos.iter().map(|q| q[1]).sum::<f64>() / n);
    let sxx: f64 = pos.iter().map(|q| ((q[0] - gx) / 1000.0).powi(2)).sum();
    let syy: f64 = pos.iter().map(|q| ((q[1] - gy) / 1000.0).powi(2)).sum();
    // Moments about the group centroid: the load acts at the cap origin (0, 0).
    let mx = p.mx_knm + load * (0.0 - gy) / 1000.0;
    let my = p.my_knm + load * (0.0 - gx) / 1000.0;
    let mut piles = Vec::new();
    for q in &pos {
        let mut r = load / n;
        if syy > 0.0 {
            r += mx * ((q[1] - gy) / 1000.0) / syy;
        }
        if sxx > 0.0 {
            r += my * ((q[0] - gx) / 1000.0) / sxx;
        }
        piles.push(PileRow { x_mm: q[0], y_mm: q[1], reaction_kn: r3(r) });
    }
    if (mx.abs() > 1e-9 && syy == 0.0) || (my.abs() > 1e-9 && sxx == 0.0) {
        return Err(format!("'{}': a moment acts about an axis the pile layout cannot resist (all piles are collinear)", p.key));
    }
    let max = piles.iter().map(|q| q.reaction_kn).fold(f64::MIN, f64::max);
    let min = piles.iter().map(|q| q.reaction_kn).fold(f64::MAX, f64::min);
    let mut min_d: Option<f64> = None;
    for i in 0..pos.len() {
        for j in i + 1..pos.len() {
            let d = ((pos[i][0] - pos[j][0]).powi(2) + (pos[i][1] - pos[j][1]).powi(2)).sqrt();
            min_d = Some(min_d.map_or(d, |m: f64| m.min(d)));
        }
    }
    let spacing_status = match (min_d, p.min_spacing_mm) {
        (None, _) => "not_applicable",
        (Some(_), None) => "unchecked",
        (Some(d), Some(m)) => {
            if d + 1e-6 >= positive(m, "min_spacing_mm", &p.key)? {
                "ok"
            } else {
                "fails"
            }
        }
    };
    let status = if min < 0.0 {
        match p.uplift_capacity_kn {
            None => {
                findings.push(Finding { ar: format!("الغطاء «{}»: خازوق في شد ولم تُدخل قدرة الشد فلم يُحكم.", p.key), en: format!("Cap '{}': a pile is in tension and no uplift_capacity_kn was given, so no verdict.", p.key) });
                "unchecked"
            }
            Some(u) if max <= cap && -min <= positive(u, "uplift_capacity_kn", &p.key)? => "ok",
            Some(_) => "fails",
        }
    } else if max <= cap {
        "ok"
    } else {
        "fails"
    };
    if spacing_status == "unchecked" {
        findings.push(Finding { ar: format!("الغطاء «{}»: لم يُدخل أدنى تباعد فلم يُحكم بالتباعد.", p.key), en: format!("Cap '{}': no min_spacing_mm given, so spacing is unchecked.", p.key) });
    }
    Ok(PileReport { key: p.key.clone(), required_count: required, count: pos.len() as u32, generated, piles, max_kn: max, min_kn: min, min_distance_mm: min_d.map(r3), spacing_status, status })
}

pub fn check(inp: &DeepInput) -> Result<DeepReport, String> {
    if inp.screen.is_none() && inp.mat.is_none() && inp.piles.is_empty() {
        return Err("nothing to check: give screen, mat or piles".into());
    }
    let mut findings = Vec::new();
    let screen_r = inp.screen.as_ref().map(|s| screen(s, &mut findings)).transpose()?;
    let mat_r = inp.mat.as_ref().map(|m| mat(m, &inp.columns, &mut findings)).transpose()?;
    let mut seen = std::collections::BTreeSet::new();
    let mut piles = Vec::new();
    for p in &inp.piles {
        if !seen.insert(p.key.as_str()) {
            return Err(format!("pile cap key '{}' is used twice", p.key));
        }
        piles.push(pile_cap(p, &mut findings)?);
    }
    if !piles.is_empty() {
        findings.push(Finding { ar: "الخوازيق: غطاء جاسئ بتوزيع خطي للأحمال؛ لا كفاءة مجموعة ولا احتكاك سالب ولا حمل جانبي.".into(), en: "Piles: rigid cap with linear load distribution; no group efficiency, negative skin friction or lateral load.".into() });
    }
    Ok(DeepReport { screen: screen_r, mat: mat_r, piles, findings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(key: &str, x: f64, y: f64, p: f64) -> ColumnLoad {
        ColumnLoad { key: key.into(), x_mm: x, y_mm: y, load_kn: p, mx_knm: 0.0, my_knm: 0.0 }
    }
    fn mat_in(allowable: Option<f64>) -> MatIn {
        MatIn { x_mm: 0.0, y_mm: 0.0, w_mm: 10000.0, l_mm: 10000.0, thickness_mm: None, unit_weight_kn_m3: None, allowable_kpa: allowable }
    }
    fn four(p: f64) -> Vec<ColumnLoad> {
        vec![col("a", 2000.0, 2000.0, p), col("b", 8000.0, 2000.0, p), col("c", 2000.0, 8000.0, p), col("d", 8000.0, 8000.0, p)]
    }
    fn pile(load: f64) -> PileIn {
        PileIn { key: "C1".into(), load_kn: load, mx_knm: 0.0, my_knm: 0.0, capacity_kn: 500.0, uplift_capacity_kn: None, spacing_mm: Some(1500.0), positions: None, min_spacing_mm: None }
    }

    #[test]
    fn symmetric_mat_has_uniform_pressure() {
        let inp = DeepInput { mat: Some(mat_in(Some(150.0))), columns: four(1000.0), ..Default::default() };
        let r = check(&inp).unwrap().mat.unwrap();
        assert_eq!(r.max_kpa, 40.0);
        assert_eq!(r.min_kpa, 40.0);
        assert_eq!(r.status, "ok");
        let tight = DeepInput { mat: Some(mat_in(Some(30.0))), columns: four(1000.0), ..Default::default() };
        assert_eq!(check(&tight).unwrap().mat.unwrap().status, "fails");
        let none = DeepInput { mat: Some(mat_in(None)), columns: four(1000.0), ..Default::default() };
        assert_eq!(check(&none).unwrap().mat.unwrap().status, "unchecked");
    }

    #[test]
    fn eccentric_load_tilts_pressure_and_can_uplift() {
        // 4000 kN total, extra 1000 kN on the +x side column: shifts the resultant.
        let mut cols = four(1000.0);
        cols[1].load_kn = 2000.0;
        cols[3].load_kn = 2000.0;
        let r = check(&DeepInput { mat: Some(mat_in(Some(200.0))), columns: cols, ..Default::default() }).unwrap().mat.unwrap();
        // P = 6000, A = 100, My = 2*1000*3 kNm... = 6000 kNm, Iy = 833.333 m4, q = 60 +- 6000*5/833.333.
        assert_eq!(r.total_load_kn, 6000.0);
        assert_eq!(r.max_kpa, 96.0);
        assert_eq!(r.min_kpa, 24.0);
        assert_eq!(r.ex_mm, 1000.0);
        let mut big = four(100.0);
        big[0].mx_knm = 5000.0;
        let up = check(&DeepInput { mat: Some(mat_in(Some(200.0))), columns: big, ..Default::default() }).unwrap().mat.unwrap();
        assert_eq!(up.status, "uplift");
    }

    #[test]
    fn self_weight_needs_both_inputs() {
        let mut m = mat_in(None);
        m.thickness_mm = Some(500.0);
        assert!(check(&DeepInput { mat: Some(m.clone()), columns: four(1000.0), ..Default::default() }).unwrap_err().contains("together"));
        m.unit_weight_kn_m3 = Some(25.0);
        let r = check(&DeepInput { mat: Some(m), columns: four(1000.0), ..Default::default() }).unwrap().mat.unwrap();
        assert_eq!(r.self_weight_kn, 1250.0);
        assert_eq!(r.max_kpa, 52.5);
    }

    #[test]
    fn mat_refuses_bad_inputs_by_name() {
        let no_cols = DeepInput { mat: Some(mat_in(None)), ..Default::default() };
        assert!(check(&no_cols).unwrap_err().contains("columns are required"));
        let out = DeepInput { mat: Some(mat_in(None)), columns: vec![col("z", 20000.0, 0.0, 10.0)], ..Default::default() };
        assert!(check(&out).unwrap_err().contains("outside the mat"));
        let dup = DeepInput { mat: Some(mat_in(None)), columns: vec![col("a", 1.0, 1.0, 1.0), col("a", 2.0, 2.0, 1.0)], ..Default::default() };
        assert!(check(&dup).unwrap_err().contains("used twice"));
        assert!(check(&DeepInput::default()).unwrap_err().contains("nothing to check"));
    }

    #[test]
    fn screen_reports_ratio_and_verdict_only_with_a_limit() {
        let s = |m| DeepInput { screen: Some(ScreenIn { isolated_area_m2: 60.0, building_area_m2: 100.0, max_coverage_ratio: m }), ..Default::default() };
        assert_eq!(check(&s(None)).unwrap().screen.unwrap().verdict, "unchecked");
        assert_eq!(check(&s(Some(0.5))).unwrap().screen.unwrap().verdict, "mat_indicated");
        let r = check(&s(Some(0.7))).unwrap().screen.unwrap();
        assert_eq!(r.verdict, "isolated_ok");
        assert_eq!(r.coverage_ratio, 0.6);
    }

    #[test]
    fn pile_cap_generates_a_grid_and_shares_load_equally() {
        let r = check(&DeepInput { piles: vec![pile(1800.0)], ..Default::default() }).unwrap().piles.remove(0);
        // 1800 / 500 = 3.6 -> 4 piles in a 2 x 2 grid; the load is at the origin, so each pile is offset and the cap origin
        // is one corner: the eccentric moment makes the corner nearest the load heavier.
        assert_eq!(r.required_count, 4);
        assert_eq!(r.count, 4);
        assert!(r.generated);
        assert_eq!(r.min_distance_mm, Some(1500.0));
        assert_eq!(r.spacing_status, "unchecked");
        let total: f64 = r.piles.iter().map(|p| p.reaction_kn).sum();
        assert!((total - 1800.0).abs() < 0.01);
    }

    #[test]
    fn pile_positions_centred_on_load_give_equal_reactions_and_spacing_verdict() {
        let mut p = pile(1600.0);
        p.positions = Some(vec![[-750.0, -750.0], [750.0, -750.0], [-750.0, 750.0], [750.0, 750.0]]);
        p.min_spacing_mm = Some(1500.0);
        let r = check(&DeepInput { piles: vec![p.clone()], ..Default::default() }).unwrap().piles.remove(0);
        assert_eq!(r.max_kn, 400.0);
        assert_eq!(r.min_kn, 400.0);
        assert_eq!(r.status, "ok");
        assert_eq!(r.spacing_status, "ok");
        p.min_spacing_mm = Some(2000.0);
        assert_eq!(check(&DeepInput { piles: vec![p.clone()], ..Default::default() }).unwrap().piles[0].spacing_status, "fails");
        p.min_spacing_mm = None;
        p.mx_knm = 600.0;
        let m = check(&DeepInput { piles: vec![p], ..Default::default() }).unwrap().piles.remove(0);
        // sum(y^2) = 4 * 0.75^2 = 2.25; 600 * 0.75 / 2.25 = 200.
        assert_eq!(m.max_kn, 600.0);
        assert_eq!(m.min_kn, 200.0);
        assert_eq!(m.status, "fails");
    }

    #[test]
    fn pile_tension_needs_an_uplift_capacity() {
        let mut p = pile(400.0);
        p.positions = Some(vec![[-1000.0, 0.0], [1000.0, 0.0]]);
        p.my_knm = 600.0;
        let r = check(&DeepInput { piles: vec![p.clone()], ..Default::default() }).unwrap().piles.remove(0);
        // my = 600, sum(x^2) = 2; R = 200 +- 600 * 1 / 2 = 500 / -100.
        assert_eq!(r.max_kn, 500.0);
        assert_eq!(r.min_kn, -100.0);
        assert_eq!(r.status, "unchecked");
        p.uplift_capacity_kn = Some(150.0);
        assert_eq!(check(&DeepInput { piles: vec![p.clone()], ..Default::default() }).unwrap().piles[0].status, "ok");
        p.uplift_capacity_kn = Some(50.0);
        assert_eq!(check(&DeepInput { piles: vec![p], ..Default::default() }).unwrap().piles[0].status, "fails");
    }

    #[test]
    fn pile_refuses_missing_spacing_and_collinear_moment() {
        let mut p = pile(500.0);
        p.spacing_mm = None;
        assert!(check(&DeepInput { piles: vec![p], ..Default::default() }).unwrap_err().contains("spacing_mm is required"));
        let mut c = pile(500.0);
        c.positions = Some(vec![[-1000.0, 0.0], [1000.0, 0.0]]);
        c.mx_knm = 10.0;
        assert!(check(&DeepInput { piles: vec![c], ..Default::default() }).unwrap_err().contains("collinear"));
        let mut z = pile(500.0);
        z.capacity_kn = 0.0;
        assert!(check(&DeepInput { piles: vec![z], ..Default::default() }).unwrap_err().contains("capacity_kn must be a positive"));
    }
}
