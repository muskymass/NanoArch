//! Sloping ground, multiple finished levels and ramps between them.
//!
//! The user enters spot levels from a survey (x, y, level in millimetres on the survey datum), the finished level of each
//! platform (a rectangle of the plan built at one level) and optionally ramps between two platforms. Nothing is assumed:
//! - the ground is the least-squares plane through the spot levels; at least three points not on one line are required and
//!   the largest residual is reported, so a ground the plane does not describe is visible rather than hidden;
//! - every limit (steepest site slope, ground variation across a platform, deepest cut, deepest fill, largest single step,
//!   steepest ramp, narrowest ramp, plane residual) is a user input from the code, the authority or the geotechnical report;
//!   without one the matching check is `unchecked`, never passed. Nothing is read from the Saudi Building Code here.
//!
//! Cut and fill volumes are exact for the fitted plane: the platform rectangle is clipped to the part where the ground is
//! above (cut) or below (fill) the finished level, and a linear function integrates to area times its value at the centroid.
//! This is a geometric earthworks estimate to the finished level; it allows for no slab thickness, bulking, shrinkage,
//! retaining-wall footings or side slopes, and it is not a grading design.

use crate::courtyard::Finding;
use crate::layout::Rect;
use crate::rooftop::Check;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// One surveyed ground level: plan position and level, all in millimetres.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct SpotLevel {
    pub x: i64,
    pub y: i64,
    pub z_mm: i64,
}

/// A rectangle of the plan built at one finished level (millimetres, same datum as the spot levels).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Platform {
    pub key: String,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    pub level_mm: i64,
}

/// A ramp joining two platforms. Its rise is the level difference of the platforms; the run and width are entered.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Ramp {
    pub key: String,
    pub from: String,
    pub to: String,
    pub run_mm: i64,
    pub width_mm: i64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TerrainSpec {
    pub spots: Vec<SpotLevel>,
    #[serde(default)]
    pub platforms: Vec<Platform>,
    #[serde(default)]
    pub ramps: Vec<Ramp>,
    #[serde(default)]
    pub max_site_slope_pct: Option<f64>,
    #[serde(default)]
    pub max_plane_residual_mm: Option<i64>,
    #[serde(default)]
    pub max_platform_ground_range_mm: Option<i64>,
    #[serde(default)]
    pub max_cut_mm: Option<i64>,
    #[serde(default)]
    pub max_fill_mm: Option<i64>,
    #[serde(default)]
    pub max_step_mm: Option<i64>,
    #[serde(default)]
    pub max_ramp_slope_pct: Option<f64>,
    #[serde(default)]
    pub min_ramp_width_mm: Option<i64>,
}

/// The side of the plan frame the ground falls towards (the frame of the entered coordinates, not true north).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FallSide {
    XMax,
    XMin,
    YMax,
    YMin,
    /// The fitted plane is level to within 1e-9: there is no fall direction.
    Level,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct GroundPlane {
    pub spot_count: usize,
    /// Rise of the ground per metre in plan x and y (mm per mm).
    pub gradient_x: f64,
    pub gradient_y: f64,
    pub slope_pct: f64,
    pub slope_deg: f64,
    /// Unit vector pointing downhill in the plan frame; `[0, 0]` when level.
    pub fall_dx: f64,
    pub fall_dy: f64,
    pub fall_side: FallSide,
    pub max_residual_mm: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlatformReport {
    pub key: String,
    pub level_mm: i64,
    pub area_m2: f64,
    pub ground_min_mm: f64,
    pub ground_max_mm: f64,
    pub ground_range_mm: f64,
    pub ground_mean_mm: f64,
    /// Deepest cut (ground above the finished level) at any point of the platform; 0 when none.
    pub max_cut_depth_mm: f64,
    /// Deepest fill (ground below the finished level) at any point of the platform; 0 when none.
    pub max_fill_depth_mm: f64,
    pub cut_m3: f64,
    pub fill_m3: f64,
    pub ground_range_check: Check,
    pub cut_check: Check,
    pub fill_check: Check,
}

/// Two platforms sharing a boundary of positive length.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepReport {
    pub a: String,
    pub b: String,
    pub contact_length_mm: i64,
    /// Level of `b` minus level of `a`.
    pub rise_mm: i64,
    pub check: Check,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RampReport {
    pub key: String,
    pub from: String,
    pub to: String,
    pub rise_mm: i64,
    pub run_mm: i64,
    pub width_mm: i64,
    pub slope_pct: f64,
    /// Run needed at the steepest slope entered; `None` without a limit.
    pub min_run_mm: Option<f64>,
    pub slope_check: Check,
    pub width_check: Check,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TerrainReport {
    pub ground: GroundPlane,
    pub platforms: Vec<PlatformReport>,
    pub steps: Vec<StepReport>,
    pub ramps: Vec<RampReport>,
    pub total_cut_m3: f64,
    pub total_fill_m3: f64,
    /// Cut minus fill: positive means soil to take off site, negative means soil to bring in.
    pub net_cut_m3: f64,
    pub site_slope_check: Check,
    pub plane_fit_check: Check,
    pub ground_range_check: Check,
    pub cut_check: Check,
    pub fill_check: Check,
    pub step_check: Check,
    pub ramp_slope_check: Check,
    pub ramp_width_check: Check,
    pub findings: Vec<Finding>,
}

fn at_most(value: f64, max: Option<f64>) -> Check {
    match max {
        Some(m) if value <= m => Check::Pass,
        Some(_) => Check::Fail,
        None => Check::Unchecked,
    }
}

fn at_least(value: f64, min: Option<f64>) -> Check {
    match min {
        Some(m) if value >= m => Check::Pass,
        Some(_) => Check::Fail,
        None => Check::Unchecked,
    }
}

/// Fails if any check fails, passes if all pass and at least one exists, otherwise `unchecked`.
fn combine(checks: impl IntoIterator<Item = Check>) -> Check {
    let mut result = Check::Unchecked;
    for c in checks {
        match c {
            Check::Fail => return Check::Fail,
            Check::Pass => result = Check::Pass,
            Check::Unchecked => {}
        }
    }
    result
}

fn positive(name: &str, value: Option<i64>) -> Result<(), String> {
    match value {
        Some(v) if v <= 0 => Err(format!("terrain: {name} must be positive, not {v} mm")),
        _ => Ok(()),
    }
}

fn validate(bounds: &Rect, spec: &TerrainSpec) -> Result<(), String> {
    if bounds.w <= 0 || bounds.h <= 0 {
        return Err(format!("terrain: the bounds must have a positive size (got {} x {} mm)", bounds.w, bounds.h));
    }
    if spec.spots.len() < 3 {
        return Err(format!("terrain: at least 3 spot levels are required to describe a sloping ground, got {}", spec.spots.len()));
    }
    for (i, s) in spec.spots.iter().enumerate() {
        if spec.spots[..i].iter().any(|p| p.x == s.x && p.y == s.y) {
            return Err(format!("terrain: two spot levels share the position ({}, {}) mm", s.x, s.y));
        }
    }
    let p0 = spec.spots[0];
    let collinear = spec.spots.iter().skip(1).all(|p| {
        let p1 = spec.spots[1];
        (p1.x - p0.x) as i128 * (p.y - p0.y) as i128 - (p1.y - p0.y) as i128 * (p.x - p0.x) as i128 == 0
    });
    if collinear {
        return Err("terrain: the spot levels lie on one line, so the slope across it cannot be known; add a point off the line".to_string());
    }
    if let Some(m) = spec.max_site_slope_pct.filter(|m| !(m.is_finite() && *m >= 0.0)) {
        return Err(format!("terrain: max_site_slope_pct must be a number not below 0, not {m}"));
    }
    if let Some(m) = spec.max_ramp_slope_pct.filter(|m| !(m.is_finite() && *m > 0.0)) {
        return Err(format!("terrain: max_ramp_slope_pct must be a number above 0, not {m}"));
    }
    for (name, v) in [("max_cut_mm", spec.max_cut_mm), ("max_fill_mm", spec.max_fill_mm), ("max_step_mm", spec.max_step_mm)] {
        if let Some(m) = v.filter(|m| *m < 0) {
            return Err(format!("terrain: {name} must not be negative, not {m} mm"));
        }
    }
    positive("min_ramp_width_mm", spec.min_ramp_width_mm)?;
    if let Some(m) = spec.max_plane_residual_mm.filter(|m| *m < 0) {
        return Err(format!("terrain: max_plane_residual_mm must not be negative, not {m} mm"));
    }
    if let Some(m) = spec.max_platform_ground_range_mm.filter(|m| *m < 0) {
        return Err(format!("terrain: max_platform_ground_range_mm must not be negative, not {m} mm"));
    }

    let mut keys = BTreeSet::new();
    for p in &spec.platforms {
        if p.key.trim().is_empty() {
            return Err("terrain: every platform needs a key".to_string());
        }
        if !keys.insert(p.key.as_str()) {
            return Err(format!("terrain: the platform key '{}' is used twice", p.key));
        }
        if p.w <= 0 || p.h <= 0 {
            return Err(format!("terrain: platform '{}' must have a positive size (got {} x {} mm)", p.key, p.w, p.h));
        }
        if p.x < 0 || p.y < 0 || p.x + p.w > bounds.w || p.y + p.h > bounds.h {
            return Err(format!("terrain: platform '{}' lies outside the bounds ({} x {} mm)", p.key, bounds.w, bounds.h));
        }
    }
    for (i, a) in spec.platforms.iter().enumerate() {
        for b in &spec.platforms[i + 1..] {
            if a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h {
                return Err(format!("terrain: platforms '{}' and '{}' overlap, so one plan area would have two finished levels", a.key, b.key));
            }
        }
    }
    let mut ramp_keys = BTreeSet::new();
    for r in &spec.ramps {
        if r.key.trim().is_empty() {
            return Err("terrain: every ramp needs a key".to_string());
        }
        if !ramp_keys.insert(r.key.as_str()) {
            return Err(format!("terrain: the ramp key '{}' is used twice", r.key));
        }
        for (role, k) in [("from", &r.from), ("to", &r.to)] {
            if !keys.contains(k.as_str()) {
                return Err(format!("terrain: ramp '{}' {role} names the platform '{k}', which does not exist", r.key));
            }
        }
        if r.from == r.to {
            return Err(format!("terrain: ramp '{}' joins the platform '{}' to itself", r.key, r.from));
        }
        if r.run_mm <= 0 {
            return Err(format!("terrain: ramp '{}' run_mm is required and must be positive, not {} mm", r.key, r.run_mm));
        }
        if r.width_mm <= 0 {
            return Err(format!("terrain: ramp '{}' width_mm is required and must be positive, not {} mm", r.key, r.width_mm));
        }
    }
    Ok(())
}

/// The fitted plane `z = zm + a (x - xm) + b (y - ym)`.
struct Plane {
    xm: f64,
    ym: f64,
    zm: f64,
    a: f64,
    b: f64,
}

impl Plane {
    fn at(&self, x: f64, y: f64) -> f64 {
        self.zm + self.a * (x - self.xm) + self.b * (y - self.ym)
    }
}

fn fit(spots: &[SpotLevel]) -> Result<Plane, String> {
    let n = spots.len() as f64;
    let xm = spots.iter().map(|s| s.x as f64).sum::<f64>() / n;
    let ym = spots.iter().map(|s| s.y as f64).sum::<f64>() / n;
    let zm = spots.iter().map(|s| s.z_mm as f64).sum::<f64>() / n;
    let (mut sxx, mut sxy, mut syy, mut sxz, mut syz) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for s in spots {
        let (dx, dy, dz) = (s.x as f64 - xm, s.y as f64 - ym, s.z_mm as f64 - zm);
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
        sxz += dx * dz;
        syz += dy * dz;
    }
    let det = sxx * syy - sxy * sxy;
    if det.abs() <= f64::EPSILON * sxx * syy {
        return Err("terrain: the spot levels are too close to one line to fit a plane; add a point well off the line".to_string());
    }
    Ok(Plane { xm, ym, zm, a: (sxz * syy - syz * sxy) / det, b: (syz * sxx - sxz * sxy) / det })
}

/// Clips the polygon to the half plane where `plane - level >= 0` (Sutherland-Hodgman).
fn clip_above(poly: &[(f64, f64)], plane: &Plane, level: f64) -> Vec<(f64, f64)> {
    let f = |p: (f64, f64)| plane.at(p.0, p.1) - level;
    let mut out = Vec::new();
    for i in 0..poly.len() {
        let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
        let (fp, fq) = (f(p), f(q));
        if fp >= 0.0 {
            out.push(p);
        }
        if (fp >= 0.0) != (fq >= 0.0) {
            let t = fp / (fp - fq);
            out.push((p.0 + t * (q.0 - p.0), p.1 + t * (q.1 - p.1)));
        }
    }
    out
}

/// Area and centroid of a simple polygon; `None` when its area is zero.
fn area_centroid(poly: &[(f64, f64)]) -> Option<(f64, (f64, f64))> {
    if poly.len() < 3 {
        return None;
    }
    let (mut a2, mut cx, mut cy) = (0.0, 0.0, 0.0);
    for i in 0..poly.len() {
        let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
        let cross = p.0 * q.1 - q.0 * p.1;
        a2 += cross;
        cx += (p.0 + q.0) * cross;
        cy += (p.1 + q.1) * cross;
    }
    if a2.abs() < 1e-9 {
        return None;
    }
    Some((a2.abs() / 2.0, (cx / (3.0 * a2), cy / (3.0 * a2))))
}

fn platform_report(p: &Platform, plane: &Plane, spec: &TerrainSpec) -> PlatformReport {
    let (x0, y0, x1, y1) = (p.x as f64, p.y as f64, (p.x + p.w) as f64, (p.y + p.h) as f64);
    let rect = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
    let corners: Vec<f64> = rect.iter().map(|c| plane.at(c.0, c.1)).collect();
    let (gmin, gmax) = corners.iter().fold((f64::MAX, f64::MIN), |(lo, hi), g| (lo.min(*g), hi.max(*g)));
    let mean = plane.at((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let level = p.level_mm as f64;
    let area_mm2 = p.w as f64 * p.h as f64;
    let cut_mm3 = area_centroid(&clip_above(&rect, plane, level)).map(|(a, c)| a * (plane.at(c.0, c.1) - level)).unwrap_or(0.0);
    // The signed volume (ground minus level) over the whole rectangle is area x mean; fill is what cut leaves unbalanced.
    let fill_mm3 = (cut_mm3 - area_mm2 * (mean - level)).max(0.0);
    let cut_mm3 = cut_mm3.max(0.0);
    let max_cut = (gmax - level).max(0.0);
    let max_fill = (level - gmin).max(0.0);
    let range = gmax - gmin;
    PlatformReport {
        key: p.key.clone(),
        level_mm: p.level_mm,
        area_m2: area_mm2 / 1e6,
        ground_min_mm: gmin,
        ground_max_mm: gmax,
        ground_range_mm: range,
        ground_mean_mm: mean,
        max_cut_depth_mm: max_cut,
        max_fill_depth_mm: max_fill,
        cut_m3: cut_mm3 / 1e9,
        fill_m3: fill_mm3 / 1e9,
        ground_range_check: at_most(range, spec.max_platform_ground_range_mm.map(|v| v as f64)),
        cut_check: at_most(max_cut, spec.max_cut_mm.map(|v| v as f64)),
        fill_check: at_most(max_fill, spec.max_fill_mm.map(|v| v as f64)),
    }
}

/// Length of the shared boundary of two non-overlapping rectangles (0 when they only touch at a corner or not at all).
fn contact_length(a: &Platform, b: &Platform) -> i64 {
    let overlap = |a0: i64, a1: i64, b0: i64, b1: i64| (a1.min(b1) - a0.max(b0)).max(0);
    if a.x + a.w == b.x || b.x + b.w == a.x {
        overlap(a.y, a.y + a.h, b.y, b.y + b.h)
    } else if a.y + a.h == b.y || b.y + b.h == a.y {
        overlap(a.x, a.x + a.w, b.x, b.x + b.w)
    } else {
        0
    }
}

/// Fits the ground to the surveyed spot levels and checks platforms, steps and ramps against the limits the user entered.
pub fn check_terrain(bounds: &Rect, spec: &TerrainSpec) -> Result<TerrainReport, String> {
    validate(bounds, spec)?;
    let plane = fit(&spec.spots)?;
    let max_residual = spec.spots.iter().map(|s| (s.z_mm as f64 - plane.at(s.x as f64, s.y as f64)).abs()).fold(0.0, f64::max);
    let grad = (plane.a * plane.a + plane.b * plane.b).sqrt();
    let (fall_dx, fall_dy, fall_side) = if grad < 1e-9 {
        (0.0, 0.0, FallSide::Level)
    } else {
        let (dx, dy) = (-plane.a / grad, -plane.b / grad);
        let side = if dx.abs() >= dy.abs() {
            if dx > 0.0 { FallSide::XMax } else { FallSide::XMin }
        } else if dy > 0.0 {
            FallSide::YMax
        } else {
            FallSide::YMin
        };
        (dx, dy, side)
    };
    let ground = GroundPlane {
        spot_count: spec.spots.len(),
        gradient_x: plane.a,
        gradient_y: plane.b,
        slope_pct: grad * 100.0,
        slope_deg: grad.atan().to_degrees(),
        fall_dx,
        fall_dy,
        fall_side,
        max_residual_mm: max_residual,
    };

    let platforms: Vec<PlatformReport> = spec.platforms.iter().map(|p| platform_report(p, &plane, spec)).collect();
    let total_cut_m3: f64 = platforms.iter().map(|p| p.cut_m3).sum();
    let total_fill_m3: f64 = platforms.iter().map(|p| p.fill_m3).sum();

    let mut steps = Vec::new();
    for (i, a) in spec.platforms.iter().enumerate() {
        for b in &spec.platforms[i + 1..] {
            let len = contact_length(a, b);
            if len > 0 {
                let rise = b.level_mm - a.level_mm;
                steps.push(StepReport {
                    a: a.key.clone(),
                    b: b.key.clone(),
                    contact_length_mm: len,
                    rise_mm: rise,
                    check: at_most(rise.abs() as f64, spec.max_step_mm.map(|v| v as f64)),
                });
            }
        }
    }

    let level_of = |k: &str| spec.platforms.iter().find(|p| p.key == k).map(|p| p.level_mm).unwrap_or_default();
    let ramps: Vec<RampReport> = spec
        .ramps
        .iter()
        .map(|r| {
            let rise = (level_of(&r.to) - level_of(&r.from)).abs();
            RampReport {
                key: r.key.clone(),
                from: r.from.clone(),
                to: r.to.clone(),
                rise_mm: rise,
                run_mm: r.run_mm,
                width_mm: r.width_mm,
                slope_pct: rise as f64 / r.run_mm as f64 * 100.0,
                min_run_mm: spec.max_ramp_slope_pct.map(|m| rise as f64 / (m / 100.0)),
                slope_check: at_most(rise as f64 / r.run_mm as f64 * 100.0, spec.max_ramp_slope_pct),
                width_check: at_least(r.width_mm as f64, spec.min_ramp_width_mm.map(|v| v as f64)),
            }
        })
        .collect();

    let site_slope_check = at_most(ground.slope_pct, spec.max_site_slope_pct);
    let plane_fit_check = at_most(max_residual, spec.max_plane_residual_mm.map(|v| v as f64));
    let ground_range_check = combine(platforms.iter().map(|p| p.ground_range_check));
    let cut_check = combine(platforms.iter().map(|p| p.cut_check));
    let fill_check = combine(platforms.iter().map(|p| p.fill_check));
    let step_check = combine(steps.iter().map(|s| s.check));
    let ramp_slope_check = combine(ramps.iter().map(|r| r.slope_check));
    let ramp_width_check = combine(ramps.iter().map(|r| r.width_check));

    let mut findings = Vec::new();
    if let (Check::Fail, Some(m)) = (site_slope_check, spec.max_site_slope_pct) {
        findings.push(Finding {
            ar: format!("ميل الأرض {:.1}% وهو أشد من الحد المُدخل {m}%.", ground.slope_pct),
            en: format!("The ground slopes at {:.1}%, steeper than the {m}% limit entered.", ground.slope_pct),
        });
    }
    if let (Check::Fail, Some(m)) = (plane_fit_check, spec.max_plane_residual_mm) {
        findings.push(Finding {
            ar: format!("أكبر انحراف عن المستوى المطابق {max_residual:.0} مم وهو فوق {m} مم: الأرض ليست مستوية الميل فأضف نقاط منسوب أو قسّمها."),
            en: format!("The largest deviation from the fitted plane is {max_residual:.0} mm, above {m} mm: the ground is not a single slope, so add spot levels or split the site."),
        });
    }
    for p in &platforms {
        if let (Check::Fail, Some(m)) = (p.ground_range_check, spec.max_platform_ground_range_mm) {
            findings.push(Finding {
                ar: format!("الأرض تحت المنصة {} تتفاوت {:.0} مم وهو فوق الحد {m} مم.", p.key, p.ground_range_mm),
                en: format!("The ground under platform {} varies by {:.0} mm, above the {m} mm limit.", p.key, p.ground_range_mm),
            });
        }
        if let (Check::Fail, Some(m)) = (p.cut_check, spec.max_cut_mm) {
            findings.push(Finding {
                ar: format!("أعمق قطع تحت المنصة {} هو {:.0} مم وهو فوق {m} مم: يلزم جدار ساند أو منسوب آخر.", p.key, p.max_cut_depth_mm),
                en: format!("The deepest cut at platform {} is {:.0} mm, above {m} mm: a retaining wall or another level is needed.", p.key, p.max_cut_depth_mm),
            });
        }
        if let (Check::Fail, Some(m)) = (p.fill_check, spec.max_fill_mm) {
            findings.push(Finding {
                ar: format!("أعمق ردم تحت المنصة {} هو {:.0} مم وهو فوق {m} مم.", p.key, p.max_fill_depth_mm),
                en: format!("The deepest fill at platform {} is {:.0} mm, above {m} mm.", p.key, p.max_fill_depth_mm),
            });
        }
    }
    for s in &steps {
        if let (Check::Fail, Some(m)) = (s.check, spec.max_step_mm) {
            findings.push(Finding {
                ar: format!("الفرق بين المنصتين {} و{} هو {} مم وهو فوق أكبر درجة مفردة {m} مم: يلزم درج أو منحدر.", s.a, s.b, s.rise_mm.abs()),
                en: format!("The level change between platforms {} and {} is {} mm, above the {m} mm single step: stairs or a ramp are needed.", s.a, s.b, s.rise_mm.abs()),
            });
        }
    }
    for r in &ramps {
        if let (Check::Fail, Some(m), Some(run)) = (r.slope_check, spec.max_ramp_slope_pct, r.min_run_mm) {
            findings.push(Finding {
                ar: format!("ميل المنحدر {} هو {:.1}% وهو أشد من {m}%: يلزمه طول أفقي {run:.0} مم على الأقل.", r.key, r.slope_pct),
                en: format!("Ramp {} slopes at {:.1}%, steeper than {m}%: it needs a run of at least {run:.0} mm.", r.key, r.slope_pct),
            });
        }
        if let (Check::Fail, Some(m)) = (r.width_check, spec.min_ramp_width_mm) {
            findings.push(Finding {
                ar: format!("عرض المنحدر {} هو {} مم وهو أقل من الحد {m} مم.", r.key, r.width_mm),
                en: format!("Ramp {} is {} mm wide, below the {m} mm minimum.", r.key, r.width_mm),
            });
        }
    }
    if ground.slope_pct > 0.0 && platforms.is_empty() {
        findings.push(Finding {
            ar: "لم تُدخل منصات بمناسيب نهائية: لم تُحسب كميات القطع والردم.".to_string(),
            en: "No platforms with finished levels were entered: cut and fill were not computed.".to_string(),
        });
    }
    let level_pairs_without_ramp = steps.iter().filter(|s| s.rise_mm != 0).count();
    if level_pairs_without_ramp > 0 && ramps.is_empty() {
        findings.push(Finding {
            ar: format!("{level_pairs_without_ramp} من المنصات المتجاورة بمنسوبين مختلفين ولم يُدخل منحدر أو درج بينها."),
            en: format!("{level_pairs_without_ramp} adjoining platform pair(s) sit at different levels and no ramp joins them."),
        });
    }
    for (check, ar, en) in [
        (site_slope_check, "ميل الأرض (يلزم حد أقصى)", "the site slope (a maximum is needed)"),
        (plane_fit_check, "جودة مطابقة المستوى (يلزم حد أقصى للانحراف)", "the plane fit (a maximum residual is needed)"),
        (ground_range_check, "تفاوت الأرض تحت المنصة (يلزم حد أقصى)", "the ground variation under a platform (a maximum is needed)"),
        (cut_check, "عمق القطع (يلزم حد أقصى)", "the cut depth (a maximum is needed)"),
        (fill_check, "عمق الردم (يلزم حد أقصى)", "the fill depth (a maximum is needed)"),
        (step_check, "الدرجة المفردة بين المنصات (يلزم حد أقصى)", "the single step between platforms (a maximum is needed)"),
        (ramp_slope_check, "ميل المنحدر (يلزم حد أقصى)", "the ramp slope (a maximum is needed)"),
        (ramp_width_check, "عرض المنحدر (يلزم حد أدنى)", "the ramp width (a minimum is needed)"),
    ] {
        if check == Check::Unchecked {
            findings.push(Finding { ar: format!("لم يُفحص: {ar}."), en: format!("Not checked: {en}.") });
        }
    }

    Ok(TerrainReport {
        ground,
        platforms,
        steps,
        ramps,
        total_cut_m3,
        total_fill_m3,
        net_cut_m3: total_cut_m3 - total_fill_m3,
        site_slope_check,
        plane_fit_check,
        ground_range_check,
        cut_check,
        fill_check,
        step_check,
        ramp_slope_check,
        ramp_width_check,
        findings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Rect {
        Rect { x: 0, y: 0, w: 20000, h: 20000 }
    }

    fn spot(x: i64, y: i64, z_mm: i64) -> SpotLevel {
        SpotLevel { x, y, z_mm }
    }

    fn plat(key: &str, x: i64, y: i64, w: i64, h: i64, level_mm: i64) -> Platform {
        Platform { key: key.to_string(), x, y, w, h, level_mm }
    }

    /// Ground rising 5% towards +x from 100 000 mm at x = 0: z = 100000 + 0.05 x.
    fn five_pct(platforms: Vec<Platform>) -> TerrainSpec {
        TerrainSpec {
            spots: vec![spot(0, 0, 100_000), spot(20000, 0, 101_000), spot(0, 20000, 100_000), spot(20000, 20000, 101_000)],
            platforms,
            ramps: vec![],
            max_site_slope_pct: None,
            max_plane_residual_mm: None,
            max_platform_ground_range_mm: None,
            max_cut_mm: None,
            max_fill_mm: None,
            max_step_mm: None,
            max_ramp_slope_pct: None,
            min_ramp_width_mm: None,
        }
    }

    #[test]
    fn fits_the_plane_and_reports_slope_and_fall_direction() {
        let r = check_terrain(&bounds(), &five_pct(vec![])).unwrap();
        assert!((r.ground.slope_pct - 5.0).abs() < 1e-9);
        assert!((r.ground.gradient_x - 0.05).abs() < 1e-12);
        assert!(r.ground.gradient_y.abs() < 1e-12);
        assert_eq!(r.ground.fall_side, FallSide::XMin);
        assert!(r.ground.max_residual_mm < 1e-6);
        assert_eq!(r.site_slope_check, Check::Unchecked);
        assert!(r.findings.iter().any(|f| f.en.contains("cut and fill were not computed")));
    }

    #[test]
    fn a_level_ground_has_no_fall_direction() {
        let mut s = five_pct(vec![]);
        s.spots = vec![spot(0, 0, 5000), spot(1000, 0, 5000), spot(0, 1000, 5000)];
        let r = check_terrain(&bounds(), &s).unwrap();
        assert_eq!(r.ground.fall_side, FallSide::Level);
        assert_eq!(r.ground.slope_pct, 0.0);
    }

    #[test]
    fn cut_and_fill_are_exact_for_a_platform_straddling_the_level() {
        // Platform x 0..10000 at the mean ground (100 250 mm): ground runs 100 000..100 500, so half the plan cuts and half fills.
        let s = five_pct(vec![plat("A", 0, 0, 10000, 10000, 100_250)]);
        let r = check_terrain(&bounds(), &s).unwrap();
        let p = &r.platforms[0];
        // Each side is a triangular prism: 0.5 * 5000 mm * 250 mm * 10000 mm = 6.25e9 mm3 = 6.25 m3.
        assert!((p.cut_m3 - 6.25).abs() < 1e-9, "cut {}", p.cut_m3);
        assert!((p.fill_m3 - 6.25).abs() < 1e-9, "fill {}", p.fill_m3);
        assert!((p.max_cut_depth_mm - 250.0).abs() < 1e-9);
        assert!((p.max_fill_depth_mm - 250.0).abs() < 1e-9);
        assert!((p.ground_range_mm - 500.0).abs() < 1e-9);
        assert!((r.net_cut_m3).abs() < 1e-9);
    }

    #[test]
    fn a_platform_wholly_below_the_ground_is_all_cut() {
        let s = five_pct(vec![plat("A", 0, 0, 10000, 10000, 99_000)]);
        let p = &check_terrain(&bounds(), &s).unwrap().platforms[0];
        // Mean ground 100 250, level 99 000: 1250 mm over 100 m2.
        assert!((p.cut_m3 - 125.0).abs() < 1e-9);
        assert_eq!(p.fill_m3, 0.0);
        assert_eq!(p.max_fill_depth_mm, 0.0);
    }

    #[test]
    fn limits_pass_fail_and_stay_unchecked_without_input() {
        let mut s = five_pct(vec![plat("A", 0, 0, 10000, 10000, 100_250)]);
        s.max_site_slope_pct = Some(4.0);
        s.max_cut_mm = Some(300);
        s.max_fill_mm = Some(200);
        let r = check_terrain(&bounds(), &s).unwrap();
        assert_eq!(r.site_slope_check, Check::Fail);
        assert_eq!(r.cut_check, Check::Pass);
        assert_eq!(r.fill_check, Check::Fail);
        assert_eq!(r.ground_range_check, Check::Unchecked);
        assert!(r.findings.iter().any(|f| f.en.contains("steeper than the 4% limit")));
        assert!(r.findings.iter().any(|f| f.ar.contains("ردم")));
        assert!(r.findings.iter().any(|f| f.en.starts_with("Not checked: the single step")));
    }

    #[test]
    fn adjoining_levels_report_the_step_and_a_ramp_between_them() {
        let mut s = five_pct(vec![plat("low", 0, 0, 10000, 10000, 100_000), plat("high", 10000, 0, 10000, 10000, 100_900)]);
        s.max_step_mm = Some(180);
        s.ramps = vec![Ramp { key: "R1".into(), from: "low".into(), to: "high".into(), run_mm: 10800, width_mm: 1000 }];
        s.max_ramp_slope_pct = Some(8.34);
        s.min_ramp_width_mm = Some(1200);
        let r = check_terrain(&bounds(), &s).unwrap();
        assert_eq!(r.steps.len(), 1);
        assert_eq!(r.steps[0].contact_length_mm, 10000);
        assert_eq!(r.steps[0].rise_mm, 900);
        assert_eq!(r.step_check, Check::Fail);
        let ramp = &r.ramps[0];
        assert_eq!(ramp.rise_mm, 900);
        assert!((ramp.slope_pct - 900.0 / 10800.0 * 100.0).abs() < 1e-9);
        assert_eq!(ramp.slope_check, Check::Pass);
        assert_eq!(ramp.width_check, Check::Fail);
        assert!((ramp.min_run_mm.unwrap() - 900.0 / 0.0834).abs() < 1e-6);
        assert_eq!(r.ramp_width_check, Check::Fail);
        assert!(r.findings.iter().any(|f| f.en.contains("Ramp R1 is 1000 mm wide")));
        // A steeper ramp than the limit must fail and state the run it needs.
        s.ramps[0].run_mm = 5000;
        let r = check_terrain(&bounds(), &s).unwrap();
        assert_eq!(r.ramps[0].slope_check, Check::Fail);
        assert!(r.findings.iter().any(|f| f.en.contains("needs a run of at least")));
    }

    #[test]
    fn platforms_touching_only_at_a_corner_have_no_step() {
        let s = five_pct(vec![plat("a", 0, 0, 5000, 5000, 100_000), plat("b", 5000, 5000, 5000, 5000, 100_500)]);
        assert!(check_terrain(&bounds(), &s).unwrap().steps.is_empty());
    }

    #[test]
    fn a_warped_ground_shows_its_residual() {
        let mut s = five_pct(vec![]);
        s.spots = vec![spot(0, 0, 100_000), spot(20000, 0, 100_000), spot(0, 20000, 100_000), spot(20000, 20000, 101_000)];
        s.max_plane_residual_mm = Some(100);
        let r = check_terrain(&bounds(), &s).unwrap();
        assert!((r.ground.max_residual_mm - 250.0).abs() < 1e-6);
        assert_eq!(r.plane_fit_check, Check::Fail);
    }

    #[test]
    fn bad_input_is_named_and_rejected() {
        let ok = five_pct(vec![plat("A", 0, 0, 5000, 5000, 100_000)]);
        let err = |s: &TerrainSpec| check_terrain(&bounds(), s).unwrap_err();

        let mut s = ok.clone();
        s.spots.truncate(2);
        assert!(err(&s).contains("at least 3 spot levels"));

        let mut s = ok.clone();
        s.spots = vec![spot(0, 0, 1), spot(1000, 1000, 2), spot(2000, 2000, 3)];
        assert!(err(&s).contains("one line"));

        let mut s = ok.clone();
        s.spots.push(spot(0, 0, 7));
        assert!(err(&s).contains("share the position"));

        let mut s = ok.clone();
        s.platforms.push(plat("B", 4000, 4000, 5000, 5000, 100_000));
        assert!(err(&s).contains("overlap"));

        let mut s = ok.clone();
        s.platforms.push(plat("A", 10000, 10000, 1000, 1000, 100_000));
        assert!(err(&s).contains("used twice"));

        let mut s = ok.clone();
        s.platforms[0].w = 30000;
        assert!(err(&s).contains("outside the bounds"));

        let mut s = ok.clone();
        s.ramps = vec![Ramp { key: "R".into(), from: "A".into(), to: "Z".into(), run_mm: 1000, width_mm: 1000 }];
        assert!(err(&s).contains("'Z', which does not exist"));

        let mut s = ok.clone();
        s.platforms.push(plat("B", 10000, 0, 1000, 1000, 100_100));
        s.ramps = vec![Ramp { key: "R".into(), from: "A".into(), to: "B".into(), run_mm: 0, width_mm: 1000 }];
        assert!(err(&s).contains("run_mm is required"));

        let mut s = ok.clone();
        s.max_ramp_slope_pct = Some(0.0);
        assert!(err(&s).contains("max_ramp_slope_pct"));

        let mut s = ok;
        s.max_cut_mm = Some(-1);
        assert!(err(&s).contains("max_cut_mm"));
    }
}
