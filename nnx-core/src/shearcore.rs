//! Shear-wall core around lifts and stairs: enclosure check, section properties and a cantilever lateral screen.
//!
//! Everything is an input the caller supplies and keeps the source of: wall footprints, shaft clear openings, floor forces from
//! their own lateral analysis (this module performs no seismic or wind analysis), the concrete modulus, and every limit. A missing
//! required value is refused by name; a missing limit leaves that check `unchecked` (never `ok`).
//!
//! - **Geometry**: walls are axis-aligned plan rectangles (mm). Overlapping walls, overlapping shafts and a wall cutting into a
//!   shaft are refused, because they would double count area or squeeze the clear opening.
//! - **Enclosure**: for each side of each shaft, the length covered by walls that abut that side. A side the caller declares open
//!   (a lift door, a stair entry) is reported but not flagged; any other side not fully covered is a `gap`.
//! - **Section**: area, centroid and the second moments `Ixx`, `Iyy`, `Ixy` of the wall union about its centroid (exact, by the
//!   parallel-axis theorem over the rectangles).
//! - **Lateral screen**: the core is a fixed-base cantilever with the floor forces as point loads. Tip and level deflections are
//!   exact for point loads (`F h^2 (3 h' - h) / 6EI`), including the coupling a non-symmetric section (`Ixy != 0`) produces.
//!   Base bending stress is `-(x a + y b)` with `[a b] = S^-1 * sum(F h)` at every wall corner; mean shear stress is the base shear
//!   over the area of the walls that are long in the load direction.
//!
//! Not computed, and said so in the report: torsion (loads are taken through the shear centre), openings in a wall's height,
//! coupling beams, slenderness and the foundation. Output is a preliminary screen, not a design and not a code check.

use serde::{Deserialize, Serialize};

/// Geometric tolerance for "touching" and "overlapping" tests on mm coordinates. It is a numerical tolerance, not an engineering value.
const TOL_MM: f64 = 0.5;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WallIn {
    pub key: String,
    pub x_mm: f64,
    pub y_mm: f64,
    /// Footprint extent along x.
    pub w_mm: f64,
    /// Footprint extent along y.
    pub h_mm: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ShaftIn {
    pub key: String,
    /// `lift` or `stair`.
    pub kind: String,
    /// Clear opening, mm: lower-left corner and extents.
    pub x_mm: f64,
    pub y_mm: f64,
    pub w_mm: f64,
    pub h_mm: f64,
    /// Sides left open on purpose (`south`, `north`, `east`, `west`): a lift door, a stair entry.
    #[serde(default)]
    pub open_sides: Vec<String>,
    /// Smallest clear width along x the equipment or stair needs, from the supplier or the designer. Absent: `unchecked`.
    #[serde(default)]
    pub min_clear_x_mm: Option<f64>,
    /// Smallest clear depth along y. Absent: `unchecked`.
    #[serde(default)]
    pub min_clear_y_mm: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LevelLoad {
    /// Height of the load above the core base, mm.
    pub height_mm: f64,
    /// Lateral force at that level in +x and +y from the caller's lateral analysis, kN.
    #[serde(default)]
    pub fx_kn: f64,
    #[serde(default)]
    pub fy_kn: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CoreInput {
    pub walls: Vec<WallIn>,
    #[serde(default)]
    pub shafts: Vec<ShaftIn>,
    /// Concrete modulus, MPa. Required when `levels` are given.
    #[serde(default)]
    pub e_mpa: Option<f64>,
    /// Effective-stiffness factor for a cracked core, in (0, 1]. Absent: gross section.
    #[serde(default)]
    pub stiffness_factor: Option<f64>,
    #[serde(default)]
    pub levels: Vec<LevelLoad>,
    /// Gravity load carried at the core base, kN (compression). Absent: stresses are bending only and the report says so.
    #[serde(default)]
    pub axial_kn: Option<f64>,
    /// Smallest wall thickness acceptable. Absent: `unchecked`.
    #[serde(default)]
    pub min_wall_thickness_mm: Option<f64>,
    /// Largest storey drift ratio of the core alone (drift / storey height). Absent: `unchecked`.
    #[serde(default)]
    pub max_drift_ratio: Option<f64>,
    /// Largest top deflection ratio (top deflection / height of the highest load). Absent: `unchecked`.
    #[serde(default)]
    pub max_top_ratio: Option<f64>,
    /// Largest mean shear stress, MPa. Absent: `unchecked`.
    #[serde(default)]
    pub max_shear_mpa: Option<f64>,
    /// Largest base compressive stress, MPa. Absent: `unchecked`.
    #[serde(default)]
    pub max_compression_mpa: Option<f64>,
    /// Largest base tensile stress, MPa. Absent: `unchecked`.
    #[serde(default)]
    pub max_tension_mpa: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WallRow {
    pub key: String,
    pub thickness_mm: f64,
    /// `x` when the wall is at least as long along x as along y, else `y`: the direction it resists shear in.
    pub long_axis: &'static str,
    pub area_mm2: f64,
    pub status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SideRow {
    pub side: &'static str,
    pub length_mm: f64,
    pub covered_mm: f64,
    pub open_mm: f64,
    /// `closed`, `gap` or `open_declared`.
    pub status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ShaftRow {
    pub key: String,
    pub kind: String,
    pub sides: Vec<SideRow>,
    /// `enclosed` when every side not declared open is fully covered, else `gaps`.
    pub enclosure: &'static str,
    pub clear_status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Section {
    pub area_mm2: f64,
    pub cx_mm: f64,
    pub cy_mm: f64,
    pub ixx_mm4: f64,
    pub iyy_mm4: f64,
    pub ixy_mm4: f64,
    /// Area of the walls long in x (they carry `Vx`) and in y (they carry `Vy`).
    pub shear_area_x_mm2: f64,
    pub shear_area_y_mm2: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LevelRow {
    pub height_mm: f64,
    pub dx_mm: f64,
    pub dy_mm: f64,
    /// Drift of this level relative to the one below (the base for the first), mm, resultant of x and y.
    pub drift_mm: f64,
    pub drift_ratio: f64,
    pub status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Lateral {
    pub base_shear_x_kn: f64,
    pub base_shear_y_kn: f64,
    pub base_moment_about_y_knm: f64,
    pub base_moment_about_x_knm: f64,
    pub levels: Vec<LevelRow>,
    pub top_deflection_mm: f64,
    pub top_ratio: f64,
    pub top_status: &'static str,
    pub shear_x_mpa: Option<f64>,
    pub shear_y_mpa: Option<f64>,
    pub shear_status: &'static str,
    pub max_compression_mpa: f64,
    pub max_tension_mpa: f64,
    pub compression_status: &'static str,
    pub tension_status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub ar: String,
    pub en: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CoreReport {
    pub walls: Vec<WallRow>,
    pub shafts: Vec<ShaftRow>,
    pub section: Section,
    pub lateral: Option<Lateral>,
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

fn finite(v: f64, field: &str, key: &str) -> Result<f64, String> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(format!("'{key}': {field} must be a finite number, got {v}"))
    }
}

fn upper(value: f64, limit: Option<f64>) -> &'static str {
    match limit {
        None => "unchecked",
        Some(l) if value <= l => "ok",
        Some(_) => "fails",
    }
}

fn lower(value: f64, limit: Option<f64>) -> &'static str {
    match limit {
        None => "unchecked",
        Some(l) if value >= l => "ok",
        Some(_) => "fails",
    }
}

fn limit(v: Option<f64>, field: &str) -> Result<Option<f64>, String> {
    v.map(|x| positive(x, field, "input")).transpose()
}

/// Axis-aligned rectangle `[x0, x1] x [y0, y1]`.
#[derive(Clone, Copy)]
struct Rect {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Rect {
    fn new(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x0: x, y0: y, x1: x + w, y1: y + h }
    }
    fn overlaps(&self, o: &Rect) -> bool {
        self.x1.min(o.x1) - self.x0.max(o.x0) > TOL_MM && self.y1.min(o.y1) - self.y0.max(o.y0) > TOL_MM
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

fn side_ar(s: &str) -> &'static str {
    match s {
        "south" => "الجنوبية",
        "north" => "الشمالية",
        "east" => "الشرقية",
        _ => "الغربية",
    }
}

/// Union length of intervals clipped to `[lo, hi]`.
fn covered_length(mut spans: Vec<(f64, f64)>, lo: f64, hi: f64) -> f64 {
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (mut total, mut reach) = (0.0, lo);
    for (a, b) in spans {
        let (a, b) = (a.max(reach), b.min(hi));
        if b > a {
            total += b - a;
            reach = b;
        }
    }
    total
}

fn shaft_row(s: &ShaftIn, rect: &Rect, walls: &[(Rect, &WallIn)], findings: &mut Vec<Finding>) -> Result<ShaftRow, String> {
    if s.kind != "lift" && s.kind != "stair" {
        return Err(format!("shaft '{}': kind '{}' is not one of lift, stair", s.key, s.kind));
    }
    let mut declared = Vec::new();
    for o in &s.open_sides {
        if !["south", "north", "east", "west"].contains(&o.as_str()) {
            return Err(format!("shaft '{}': open side '{o}' is not one of south, north, east, west", s.key));
        }
        if declared.contains(&o.as_str()) {
            return Err(format!("shaft '{}': open side '{o}' is listed twice", s.key));
        }
        declared.push(o.as_str());
    }
    // Plan frame as everywhere else: south is the low-y side, north the high-y side.
    let mut sides = Vec::new();
    for (name, along_x, line, lo, hi) in [("south", true, rect.y0, rect.x0, rect.x1), ("north", true, rect.y1, rect.x0, rect.x1), ("west", false, rect.x0, rect.y0, rect.y1), ("east", false, rect.x1, rect.y0, rect.y1)] {
        let spans: Vec<(f64, f64)> = walls
            .iter()
            .filter(|(w, _)| {
                // The wall must sit outside the shaft and share this side's line with its inner face.
                let face = match name {
                    "south" => w.y1,
                    "north" => w.y0,
                    "west" => w.x1,
                    _ => w.x0,
                };
                (face - line).abs() <= TOL_MM
            })
            .map(|(w, _)| if along_x { (w.x0, w.x1) } else { (w.y0, w.y1) })
            .collect();
        let length = hi - lo;
        let covered = covered_length(spans, lo, hi);
        let open = (length - covered).max(0.0);
        let is_open = declared.contains(&name);
        let status = if is_open {
            "open_declared"
        } else if open <= TOL_MM {
            "closed"
        } else {
            "gap"
        };
        if status == "gap" {
            findings.push(Finding {
                ar: format!("المنور «{}»: الجهة {} فيها {} مم بلا جدار ولم تُعلن مفتوحة؛ أعلنها في open_sides إن كانت باباً أو أغلقها.", s.key, side_ar(name), r3(open)),
                en: format!("Shaft '{}': the {name} side has {} mm with no wall and is not declared open; list it in open_sides if it is the door, or close it.", s.key, r3(open)),
            });
        }
        if is_open && open <= TOL_MM {
            findings.push(Finding {
                ar: format!("المنور «{}»: الجهة {} أُعلنت مفتوحة لكن الجدران تغطيها كاملة؛ لا فتحة للباب فيها.", s.key, side_ar(name)),
                en: format!("Shaft '{}': the {name} side is declared open but walls cover all of it, so there is no door opening.", s.key),
            });
        }
        sides.push(SideRow { side: name, length_mm: r3(length), covered_mm: r3(covered), open_mm: r3(open), status });
    }
    let enclosure = if sides.iter().any(|r| r.status == "gap") { "gaps" } else { "enclosed" };
    let (cw, ch) = (rect.x1 - rect.x0, rect.y1 - rect.y0);
    let clear_status = match (s.min_clear_x_mm, s.min_clear_y_mm) {
        (None, None) => "unchecked",
        (mx, my) => {
            let a = mx.map(|m| positive(m, "min_clear_x_mm", &s.key)).transpose()?.map(|m| lower(cw, Some(m)));
            let b = my.map(|m| positive(m, "min_clear_y_mm", &s.key)).transpose()?.map(|m| lower(ch, Some(m)));
            match (a, b) {
                (Some("fails"), _) | (_, Some("fails")) => "fails",
                (Some(_), Some(_)) => "ok",
                // Only one direction was given: the other is not judged, so the shaft is not declared ok.
                _ => "partial",
            }
        }
    };
    if clear_status == "unchecked" || clear_status == "partial" {
        findings.push(Finding {
            ar: format!("المنور «{}»: لم تُدخل كل الأبعاد الصافية الدنيا (min_clear_x_mm وmin_clear_y_mm) من المورّد أو المصمم فلم يُحكم على صلاحية الفتحة.", s.key),
            en: format!("Shaft '{}': min_clear_x_mm and min_clear_y_mm were not both given by the supplier or designer, so the clear opening is not judged.", s.key),
        });
    }
    Ok(ShaftRow { key: s.key.clone(), kind: s.kind.clone(), sides, enclosure, clear_status })
}

fn section(walls: &[(Rect, &WallIn)]) -> Result<Section, String> {
    let (mut a, mut sx, mut sy, mut ax, mut ay) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (r, _) in walls {
        let (w, h) = (r.x1 - r.x0, r.y1 - r.y0);
        let area = w * h;
        a += area;
        sx += area * (r.x0 + r.x1) / 2.0;
        sy += area * (r.y0 + r.y1) / 2.0;
        if w >= h {
            ax += area;
        } else {
            ay += area;
        }
    }
    let (cx, cy) = (sx / a, sy / a);
    let (mut ixx, mut iyy, mut ixy) = (0.0, 0.0, 0.0);
    for (r, _) in walls {
        let (w, h) = (r.x1 - r.x0, r.y1 - r.y0);
        let area = w * h;
        let (dx, dy) = ((r.x0 + r.x1) / 2.0 - cx, (r.y0 + r.y1) / 2.0 - cy);
        ixx += w * h.powi(3) / 12.0 + area * dy * dy;
        iyy += h * w.powi(3) / 12.0 + area * dx * dx;
        ixy += area * dx * dy;
    }
    if (ixx * iyy - ixy * ixy) <= 1e-9 * ixx * iyy {
        return Err("the wall section is degenerate: its bending stiffness matrix cannot be inverted".into());
    }
    Ok(Section { area_mm2: r3(a), cx_mm: r3(cx), cy_mm: r3(cy), ixx_mm4: r3(ixx), iyy_mm4: r3(iyy), ixy_mm4: r3(ixy), shear_area_x_mm2: r3(ax), shear_area_y_mm2: r3(ay) })
}

/// Deflection geometry factor at height `hj` of a fixed-base cantilever for a unit point load at `hi`, mm^3 (divide by 6EI after).
fn g(hj: f64, hi: f64) -> f64 {
    if hj <= hi {
        hj * hj * (3.0 * hi - hj)
    } else {
        hi * hi * (3.0 * hj - hi)
    }
}

struct Props {
    cx: f64,
    cy: f64,
    ixx: f64,
    iyy: f64,
    ixy: f64,
}

impl Props {
    /// `S^-1 * [fx, fy]` with `S = [[Iyy, Ixy], [Ixy, Ixx]]`.
    fn solve(&self, fx: f64, fy: f64) -> (f64, f64) {
        let det = self.ixx * self.iyy - self.ixy * self.ixy;
        ((self.ixx * fx - self.ixy * fy) / det, (self.iyy * fy - self.ixy * fx) / det)
    }
}

fn lateral(inp: &CoreInput, walls: &[(Rect, &WallIn)], sec: &Section, findings: &mut Vec<Finding>) -> Result<Lateral, String> {
    let e = positive(inp.e_mpa.ok_or("e_mpa is required when levels are given: the concrete modulus is an input with a stated source and nothing is assumed")?, "e_mpa", "input")?;
    let sf = match inp.stiffness_factor {
        None => 1.0,
        Some(f) if f.is_finite() && f > 0.0 && f <= 1.0 => f,
        Some(f) => return Err(format!("stiffness_factor must be in (0, 1], got {f}")),
    };
    let mut prev = 0.0;
    for (i, l) in inp.levels.iter().enumerate() {
        let key = format!("level {}", i + 1);
        positive(l.height_mm, "height_mm", &key)?;
        finite(l.fx_kn, "fx_kn", &key)?;
        finite(l.fy_kn, "fy_kn", &key)?;
        if l.height_mm <= prev {
            return Err(format!("{key}: heights must increase strictly, {} mm is not above {prev} mm", l.height_mm));
        }
        prev = l.height_mm;
    }
    let (vx, vy): (f64, f64) = (inp.levels.iter().map(|l| l.fx_kn).sum(), inp.levels.iter().map(|l| l.fy_kn).sum());
    if inp.levels.iter().all(|l| l.fx_kn == 0.0 && l.fy_kn == 0.0) {
        return Err("every level force is zero: nothing to check".into());
    }
    let p = Props { cx: sec.cx_mm, cy: sec.cy_mm, ixx: sec.ixx_mm4, iyy: sec.iyy_mm4, ixy: sec.ixy_mm4 };
    let n = |kn: f64| kn * 1000.0;

    // Deflection of every level: d_j = S^-1 (sum_i g(j, i) F_i) / (6 E sf).
    let mut levels = Vec::new();
    let max_ratio = limit(inp.max_drift_ratio, "max_drift_ratio")?;
    let (mut below, mut below_h) = ((0.0, 0.0), 0.0);
    for l in &inp.levels {
        let (gx, gy) = inp.levels.iter().fold((0.0, 0.0), |(ax, ay), f| (ax + g(l.height_mm, f.height_mm) * n(f.fx_kn), ay + g(l.height_mm, f.height_mm) * n(f.fy_kn)));
        let (sx, sy) = p.solve(gx, gy);
        let k = 6.0 * e * sf;
        let (dx, dy) = (sx / k, sy / k);
        let drift = ((dx - below.0).powi(2) + (dy - below.1).powi(2)).sqrt();
        let ratio = drift / (l.height_mm - below_h);
        levels.push(LevelRow { height_mm: r3(l.height_mm), dx_mm: r3(dx), dy_mm: r3(dy), drift_mm: r3(drift), drift_ratio: r6(ratio), status: upper(ratio, max_ratio) });
        below = (dx, dy);
        below_h = l.height_mm;
    }
    let top_h = below_h;
    let top = (below.0.powi(2) + below.1.powi(2)).sqrt();
    let top_ratio = top / top_h;
    let top_status = upper(top_ratio, limit(inp.max_top_ratio, "max_top_ratio")?);

    // Base bending stress: sigma = -(x a + y b), [a b] = S^-1 * sum(F h), evaluated at every wall corner; axial adds -N/A.
    let (mx, my) = inp.levels.iter().fold((0.0, 0.0), |(ax, ay), l| (ax + n(l.fx_kn) * l.height_mm, ay + n(l.fy_kn) * l.height_mm));
    let (a, b) = p.solve(mx, my);
    let axial = match inp.axial_kn {
        None => 0.0,
        Some(v) => finite(v, "axial_kn", "input")?,
    };
    let axial_stress = -n(axial) / sec.area_mm2;
    let (mut s_min, mut s_max) = (f64::INFINITY, f64::NEG_INFINITY);
    for (r, _) in walls {
        for (x, y) in [(r.x0, r.y0), (r.x1, r.y0), (r.x0, r.y1), (r.x1, r.y1)] {
            let s = -((x - p.cx) * a + (y - p.cy) * b) + axial_stress;
            s_min = s_min.min(s);
            s_max = s_max.max(s);
        }
    }
    let compression = (-s_min).max(0.0);
    let tension = s_max.max(0.0);

    let max_shear = limit(inp.max_shear_mpa, "max_shear_mpa")?;
    let shear = |v: f64, area: f64, dir: &str| -> Result<Option<f64>, String> {
        if v == 0.0 {
            Ok(None)
        } else if area <= 0.0 {
            Err(format!("a shear force acts along {dir} but no wall is long in {dir}: the shear stress cannot be computed"))
        } else {
            Ok(Some(n(v.abs()) / area))
        }
    };
    let (tx, ty) = (shear(vx, sec.shear_area_x_mm2, "x")?, shear(vy, sec.shear_area_y_mm2, "y")?);
    let worst = tx.unwrap_or(0.0).max(ty.unwrap_or(0.0));
    let shear_status = upper(worst, max_shear);

    findings.push(Finding {
        ar: "الفحص الجانبي: كابولي مثبت القاعدة بأحمال نقطية من تحليلك الجانبي (لا زلزال ولا رياح هنا). الالتواء والفتحات داخل ارتفاع الجدار وكمرات الربط والنحافة والأساس غير محسوبة، والأحمال تُعدّ مارّة بمركز القص.".into(),
        en: "Lateral screen: fixed-base cantilever with point loads from your own lateral analysis (no seismic or wind here). Torsion, openings within a wall's height, coupling beams, slenderness and the foundation are not computed; loads are taken through the shear centre.".into(),
    });
    if sec.ixy_mm4.abs() > 1e-3 * (sec.ixx_mm4 * sec.iyy_mm4).sqrt() {
        findings.push(Finding {
            ar: "المقطع غير متماثل (Ixy ≠ 0): حمل في اتجاه ينتج إزاحة في الاتجاه الآخر، وهي محسوبة في dx وdy؛ لكن مركز القص لا يطابق مركز الثقل فالالتواء الفعلي أكبر مما يظهر.".into(),
            en: "The section is not symmetric (Ixy != 0): a load in one direction moves the core in the other too, which dx and dy include; the shear centre is also off the centroid, so the real twist is larger than shown.".into(),
        });
    }
    if inp.stiffness_factor.is_none() {
        findings.push(Finding { ar: "لم يُدخل معامل التشقق فحُسب المقطع الكامل وقد تزيد الإزاحة الحقيقية.".into(), en: "No stiffness_factor given, so the gross section is used and the real deflection may be larger.".into() });
    }
    if inp.axial_kn.is_none() {
        findings.push(Finding { ar: "لم يُدخل الحمل المحوري فالإجهادات انعطاف فقط ولا ضغط ناتج عن الأحمال الرأسية.".into(), en: "No axial_kn given, so the stresses are bending only with no gravity compression.".into() });
    }
    for (given, ar, en) in [
        (inp.max_drift_ratio, "حد نسبة الانحراف max_drift_ratio", "max_drift_ratio"),
        (inp.max_top_ratio, "حد نسبة الإزاحة العلوية max_top_ratio", "max_top_ratio"),
        (inp.max_shear_mpa, "حد إجهاد القص max_shear_mpa", "max_shear_mpa"),
        (inp.max_compression_mpa, "حد إجهاد الضغط max_compression_mpa", "max_compression_mpa"),
        (inp.max_tension_mpa, "حد إجهاد الشد max_tension_mpa", "max_tension_mpa"),
    ] {
        if given.is_none() {
            findings.push(Finding { ar: format!("لم يُدخل {ar} فلم يُحكم بنجاح أو فشل لهذا الفحص."), en: format!("No {en} given, so no pass or fail is declared for that check.") });
        }
    }
    Ok(Lateral {
        base_shear_x_kn: r3(vx),
        base_shear_y_kn: r3(vy),
        base_moment_about_y_knm: r3(mx / 1e6),
        base_moment_about_x_knm: r3(my / 1e6),
        levels,
        top_deflection_mm: r3(top),
        top_ratio: r6(top_ratio),
        top_status,
        shear_x_mpa: tx.map(r6),
        shear_y_mpa: ty.map(r6),
        shear_status,
        max_compression_mpa: r6(compression),
        max_tension_mpa: r6(tension),
        compression_status: upper(compression, limit(inp.max_compression_mpa, "max_compression_mpa")?),
        tension_status: upper(tension, limit(inp.max_tension_mpa, "max_tension_mpa")?),
    })
}

pub fn check(inp: &CoreInput) -> Result<CoreReport, String> {
    if inp.walls.is_empty() {
        return Err("no walls given: the core cannot be checked without wall footprints".into());
    }
    unique(inp.walls.iter().map(|w| &w.key), "wall")?;
    unique(inp.shafts.iter().map(|s| &s.key), "shaft")?;
    let min_t = limit(inp.min_wall_thickness_mm, "min_wall_thickness_mm")?;
    let mut findings = Vec::new();

    let mut walls: Vec<(Rect, &WallIn)> = Vec::new();
    for w in &inp.walls {
        finite(w.x_mm, "x_mm", &w.key)?;
        finite(w.y_mm, "y_mm", &w.key)?;
        positive(w.w_mm, "w_mm", &w.key)?;
        positive(w.h_mm, "h_mm", &w.key)?;
        walls.push((Rect::new(w.x_mm, w.y_mm, w.w_mm, w.h_mm), w));
    }
    for (i, (a, wa)) in walls.iter().enumerate() {
        for (b, wb) in &walls[i + 1..] {
            if a.overlaps(b) {
                return Err(format!("walls '{}' and '{}' overlap: give each wall as its own non-overlapping footprint so the area is not counted twice", wa.key, wb.key));
            }
        }
    }
    let mut shafts: Vec<(Rect, &ShaftIn)> = Vec::new();
    for s in &inp.shafts {
        finite(s.x_mm, "x_mm", &s.key)?;
        finite(s.y_mm, "y_mm", &s.key)?;
        positive(s.w_mm, "w_mm", &s.key)?;
        positive(s.h_mm, "h_mm", &s.key)?;
        let r = Rect::new(s.x_mm, s.y_mm, s.w_mm, s.h_mm);
        if let Some((_, w)) = walls.iter().find(|(w, _)| w.overlaps(&r)) {
            return Err(format!("wall '{}' cuts into shaft '{}': the shaft is the clear opening and must not overlap a wall", w.key, s.key));
        }
        shafts.push((r, s));
    }
    for (i, (a, sa)) in shafts.iter().enumerate() {
        for (b, sb) in &shafts[i + 1..] {
            if a.overlaps(b) {
                return Err(format!("shafts '{}' and '{}' overlap", sa.key, sb.key));
            }
        }
    }

    let wall_rows: Vec<WallRow> = walls
        .iter()
        .map(|(r, w)| {
            let (lw, lh) = (r.x1 - r.x0, r.y1 - r.y0);
            let t = lw.min(lh);
            WallRow { key: w.key.clone(), thickness_mm: r3(t), long_axis: if lw >= lh { "x" } else { "y" }, area_mm2: r3(lw * lh), status: lower(t, min_t) }
        })
        .collect();
    if min_t.is_none() {
        findings.push(Finding { ar: "لم يُدخل أدنى سماكة للجدار (min_wall_thickness_mm) فلم يُحكم على السماكات.".into(), en: "No min_wall_thickness_mm given, so wall thicknesses are not judged.".into() });
    }
    let shaft_rows = shafts.iter().map(|(r, s)| shaft_row(s, r, &walls, &mut findings)).collect::<Result<Vec<_>, _>>()?;
    let sec = section(&walls)?;
    let lateral = if inp.levels.is_empty() {
        findings.push(Finding { ar: "لا أحمال طوابق: حُسبت الهندسة وخصائص المقطع فقط، ولم يُفحص الانحراف ولا الإجهاد.".into(), en: "No level loads given: only the geometry and section properties are computed, with no deflection or stress check.".into() });
        None
    } else {
        Some(lateral(inp, &walls, &sec, &mut findings)?)
    };
    findings.push(Finding { ar: "فحص أولي وليس تصميماً؛ يراجعه مهندس إنشائي مرخّص.".into(), en: "Preliminary screen, not a design; a licensed structural engineer must verify.".into() });
    Ok(CoreReport { walls: wall_rows, shafts: shaft_rows, section: sec, lateral, findings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall(key: &str, x: f64, y: f64, w: f64, h: f64) -> WallIn {
        WallIn { key: key.into(), x_mm: x, y_mm: y, w_mm: w, h_mm: h }
    }

    /// 3000 x 3000 closed box, 200 mm walls, as four non-overlapping footprints.
    fn box_walls() -> Vec<WallIn> {
        vec![wall("S", 0.0, 0.0, 3000.0, 200.0), wall("N", 0.0, 2800.0, 3000.0, 200.0), wall("W", 0.0, 200.0, 200.0, 2600.0), wall("E", 2800.0, 200.0, 200.0, 2600.0)]
    }

    fn shaft(key: &str, open: &[&str]) -> ShaftIn {
        ShaftIn { key: key.into(), kind: "lift".into(), x_mm: 200.0, y_mm: 200.0, w_mm: 2600.0, h_mm: 2600.0, open_sides: open.iter().map(|s| s.to_string()).collect(), min_clear_x_mm: None, min_clear_y_mm: None }
    }

    fn loaded(fx: f64, fy: f64) -> CoreInput {
        CoreInput { walls: box_walls(), e_mpa: Some(25000.0), levels: vec![LevelLoad { height_mm: 12000.0, fx_kn: fx, fy_kn: fy }], ..Default::default() }
    }

    fn approx(a: f64, b: f64, tol: f64) {
        assert!((a - b).abs() <= tol, "{a} vs {b}");
    }

    #[test]
    fn box_core_section_matches_outer_minus_inner() {
        let r = check(&CoreInput { walls: box_walls(), ..Default::default() }).unwrap();
        let s = &r.section;
        assert_eq!(s.area_mm2, 3000.0 * 3000.0 - 2600.0 * 2600.0);
        assert_eq!((s.cx_mm, s.cy_mm), (1500.0, 1500.0));
        let i = (3000f64.powi(4) - 2600f64.powi(4)) / 12.0;
        approx(s.ixx_mm4, i, 1.0);
        approx(s.iyy_mm4, i, 1.0);
        assert_eq!(s.ixy_mm4, 0.0);
        assert_eq!(s.shear_area_x_mm2, 2.0 * 3000.0 * 200.0);
        assert_eq!(s.shear_area_y_mm2, 2.0 * 200.0 * 2600.0);
        assert!(r.lateral.is_none());
        assert!(r.findings.iter().any(|f| f.en.contains("No level loads")));
    }

    #[test]
    fn tip_deflection_stress_and_shear_follow_the_cantilever_formulas() {
        let r = check(&loaded(100.0, 0.0)).unwrap();
        let l = r.lateral.unwrap();
        let i = (3000f64.powi(4) - 2600f64.powi(4)) / 12.0;
        let (f, h): (f64, f64) = (100_000.0, 12000.0);
        approx(l.top_deflection_mm, f * h.powi(3) / (3.0 * 25000.0 * i), 1e-3);
        approx(l.levels[0].dx_mm, l.top_deflection_mm, 1e-9);
        assert_eq!(l.levels[0].dy_mm, 0.0);
        // M = F h, sigma = M c / I with c = 1500.
        let sigma = f * h * 1500.0 / i;
        approx(l.max_compression_mpa, sigma, 1e-5);
        approx(l.max_tension_mpa, sigma, 1e-5);
        assert_eq!(l.base_moment_about_y_knm, 1200.0);
        // Vx is carried by the two walls long in x: 2 * 3000 * 200 mm2.
        approx(l.shear_x_mpa.unwrap(), f / 1_200_000.0, 1e-6);
        assert_eq!(l.shear_y_mpa, None);
        assert_eq!(l.shear_status, "unchecked");
        assert_eq!(l.top_status, "unchecked");
    }

    #[test]
    fn axial_load_adds_compression_and_limits_decide() {
        let mut inp = loaded(100.0, 0.0);
        inp.axial_kn = Some(1000.0);
        inp.max_compression_mpa = Some(1.5);
        inp.max_tension_mpa = Some(1.0);
        inp.max_shear_mpa = Some(0.1);
        inp.max_drift_ratio = Some(0.01);
        inp.max_top_ratio = Some(0.00001);
        let a = 3000.0 * 3000.0 - 2600.0 * 2600.0;
        let i = (3000f64.powi(4) - 2600f64.powi(4)) / 12.0;
        let sigma = 100_000.0 * 12000.0 * 1500.0 / i;
        let l = check(&inp).unwrap().lateral.unwrap();
        approx(l.max_compression_mpa, sigma + 1_000_000.0 / a, 1e-5);
        approx(l.max_tension_mpa, (sigma - 1_000_000.0 / a).max(0.0), 1e-5);
        assert_eq!(l.compression_status, "ok");
        assert_eq!(l.tension_status, "ok");
        assert_eq!(l.shear_status, "ok");
        assert_eq!(l.levels[0].status, "ok");
        assert_eq!(l.top_status, "fails");
        inp.max_shear_mpa = Some(0.01);
        assert_eq!(check(&inp).unwrap().lateral.unwrap().shear_status, "fails");
    }

    #[test]
    fn cracking_scales_deflection_and_floors_accumulate_drift() {
        let mut inp = loaded(100.0, 0.0);
        let gross = check(&inp).unwrap().lateral.unwrap().top_deflection_mm;
        inp.stiffness_factor = Some(0.5);
        approx(check(&inp).unwrap().lateral.unwrap().top_deflection_mm, 2.0 * gross, 2e-3);
        inp.levels = vec![LevelLoad { height_mm: 4000.0, fx_kn: 50.0, fy_kn: 0.0 }, LevelLoad { height_mm: 8000.0, fx_kn: 50.0, fy_kn: 0.0 }];
        let l = check(&inp).unwrap().lateral.unwrap();
        // Top level drift relative to the one below is the difference of the two displacements.
        approx(l.levels[1].drift_mm, l.levels[1].dx_mm - l.levels[0].dx_mm, 2e-3);
        approx(l.levels[1].drift_ratio, l.levels[1].drift_mm / 4000.0, 1e-6);
        assert_eq!(l.base_shear_x_kn, 100.0);
        assert_eq!(l.base_moment_about_y_knm, 50.0 * 4.0 + 50.0 * 8.0);
    }

    #[test]
    fn an_l_core_couples_the_two_directions_by_maxwell_reciprocity() {
        let l_walls = vec![wall("A", 0.0, 0.0, 3000.0, 200.0), wall("B", 0.0, 200.0, 200.0, 2800.0)];
        let run = |fx: f64, fy: f64| {
            let inp = CoreInput { walls: l_walls.clone(), e_mpa: Some(25000.0), levels: vec![LevelLoad { height_mm: 9000.0, fx_kn: fx, fy_kn: fy }], ..Default::default() };
            check(&inp).unwrap()
        };
        let by_x = run(100.0, 0.0);
        let by_y = run(0.0, 100.0);
        assert!(by_x.section.ixy_mm4.abs() > 1.0);
        let dy_from_fx = by_x.lateral.as_ref().unwrap().levels[0].dy_mm;
        let dx_from_fy = by_y.lateral.as_ref().unwrap().levels[0].dx_mm;
        assert!(dy_from_fx.abs() > 0.01);
        approx(dy_from_fx, dx_from_fy, 1e-6);
        assert!(by_x.findings.iter().any(|f| f.en.contains("not symmetric")));
    }

    #[test]
    fn a_closed_shaft_is_enclosed_and_an_unwalled_side_is_a_gap_unless_declared_open() {
        let mut inp = CoreInput { walls: box_walls(), shafts: vec![shaft("L1", &[])], ..Default::default() };
        let r = check(&inp).unwrap();
        assert_eq!(r.shafts[0].enclosure, "enclosed");
        assert!(r.shafts[0].sides.iter().all(|s| s.status == "closed" && s.open_mm == 0.0));
        // Door in the south wall: split it, leaving a 1000 mm gap.
        inp.walls[0] = wall("S1", 0.0, 0.0, 1000.0, 200.0);
        inp.walls.push(wall("S2", 2000.0, 0.0, 1000.0, 200.0));
        let r = check(&inp).unwrap();
        let south = r.shafts[0].sides.iter().find(|s| s.side == "south").unwrap();
        // The shaft's south side spans x 200..2800, the walls cover 200..1000 and 2000..2800.
        assert_eq!((south.covered_mm, south.open_mm, south.status), (1600.0, 1000.0, "gap"));
        assert_eq!(r.shafts[0].enclosure, "gaps");
        assert!(r.findings.iter().any(|f| f.en.contains("not declared open")));
        inp.shafts[0].open_sides = vec!["south".into()];
        let r = check(&inp).unwrap();
        assert_eq!(r.shafts[0].enclosure, "enclosed");
        assert_eq!(r.shafts[0].sides.iter().find(|s| s.side == "south").unwrap().status, "open_declared");
    }

    #[test]
    fn clear_size_and_thickness_are_judged_only_against_supplied_minimums() {
        let mut inp = CoreInput { walls: box_walls(), shafts: vec![shaft("L1", &[])], min_wall_thickness_mm: Some(250.0), ..Default::default() };
        let r = check(&inp).unwrap();
        assert_eq!(r.shafts[0].clear_status, "unchecked");
        assert!(r.walls.iter().all(|w| w.status == "fails" && w.thickness_mm == 200.0));
        inp.shafts[0].min_clear_x_mm = Some(2000.0);
        assert_eq!(check(&inp).unwrap().shafts[0].clear_status, "partial");
        inp.shafts[0].min_clear_y_mm = Some(2000.0);
        assert_eq!(check(&inp).unwrap().shafts[0].clear_status, "ok");
        inp.shafts[0].min_clear_y_mm = Some(2700.0);
        assert_eq!(check(&inp).unwrap().shafts[0].clear_status, "fails");
        inp.min_wall_thickness_mm = Some(150.0);
        assert!(check(&inp).unwrap().walls.iter().all(|w| w.status == "ok"));
    }

    #[test]
    fn missing_or_bad_inputs_are_refused_by_name() {
        assert!(check(&CoreInput::default()).unwrap_err().contains("no walls"));
        let mut no_e = loaded(10.0, 0.0);
        no_e.e_mpa = None;
        assert!(check(&no_e).unwrap_err().contains("e_mpa is required"));
        let mut zero = loaded(0.0, 0.0);
        assert!(check(&zero).unwrap_err().contains("every level force is zero"));
        zero.levels[0].fx_kn = 5.0;
        zero.stiffness_factor = Some(1.2);
        assert!(check(&zero).unwrap_err().contains("stiffness_factor must be in (0, 1]"));
        let mut order = loaded(10.0, 0.0);
        order.levels.push(LevelLoad { height_mm: 6000.0, fx_kn: 1.0, fy_kn: 0.0 });
        assert!(check(&order).unwrap_err().contains("must increase strictly"));
        let mut overlap = CoreInput { walls: box_walls(), ..Default::default() };
        overlap.walls.push(wall("X", 100.0, 100.0, 500.0, 500.0));
        assert!(check(&overlap).unwrap_err().contains("overlap"));
        let mut dup = CoreInput { walls: box_walls(), ..Default::default() };
        dup.walls[1].key = "S".into();
        assert!(check(&dup).unwrap_err().contains("used twice"));
        let mut thin = CoreInput { walls: box_walls(), ..Default::default() };
        thin.walls[0].h_mm = 0.0;
        assert!(check(&thin).unwrap_err().contains("h_mm must be a positive number"));
        let mut cut = CoreInput { walls: box_walls(), shafts: vec![shaft("L1", &[])], ..Default::default() };
        cut.shafts[0].x_mm = 100.0;
        assert!(check(&cut).unwrap_err().contains("cuts into shaft"));
        let mut kind = CoreInput { walls: box_walls(), shafts: vec![shaft("L1", &[])], ..Default::default() };
        kind.shafts[0].kind = "ramp".into();
        assert!(check(&kind).unwrap_err().contains("not one of lift, stair"));
        let bad_side = CoreInput { walls: box_walls(), shafts: vec![shaft("L1", &["up"])], ..Default::default() };
        assert!(check(&bad_side).unwrap_err().contains("open side 'up'"));
        // Shear along y with no wall long in y: two x-walls only.
        let x_only = CoreInput { walls: vec![wall("A", 0.0, 0.0, 3000.0, 200.0), wall("B", 0.0, 2800.0, 3000.0, 200.0)], e_mpa: Some(25000.0), levels: vec![LevelLoad { height_mm: 3000.0, fx_kn: 0.0, fy_kn: 10.0 }], ..Default::default() };
        assert!(check(&x_only).unwrap_err().contains("no wall is long in y"));
    }
}
