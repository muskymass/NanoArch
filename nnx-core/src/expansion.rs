//! Future expansion kept in the design (`meta.expansion`): vertical (extra storeys on top of the building) and/or horizontal (a
//! reserved wing beside it).
//!
//! The expansion is a stored input of the project, so it is saved in the `.nnx` file, edited through the project's command
//! stack and read by every view. Nothing is assumed:
//! - the height of an added storey, the number of storeys and the reserved rectangle are entered by the user;
//! - the largest number of storeys, the largest height, the buildable bounds and the largest coverage ratio are limits taken from the
//!   municipality or the code (with their source) and are user inputs: without one the matching check is `unchecked`, never passed;
//! - the structural provisions (columns, foundations, stair and lift shaft carried up) are declared by the user as made or not;
//!   each one not made is listed as a finding, because adding a floor later without it means strengthening or demolishing.
//!
//! This is a geometric and bookkeeping check of what the user entered, not a structural design of the future load, and it does not
//! replace the licensed engineer or the approving authority.

use crate::courtyard::Finding;
use crate::model3d::Scene;
use serde::{Deserialize, Serialize};

/// The most added storeys the application accepts in one input (a typing guard, not a code limit).
pub const MAX_EXTRA_STOREYS: u32 = 10;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VerticalExpansion {
    /// How many storeys will be added on top of the existing top storey.
    pub extra_storeys: u32,
    /// Floor-to-floor height of an added storey, millimetres.
    pub storey_height_mm: Option<f64>,
    /// The columns are designed (sized and reinforced) for the future storeys.
    pub columns_designed: bool,
    /// The foundations are designed for the future storeys.
    pub foundation_designed: bool,
    /// The stair is built so that it can continue up through the roof slab.
    pub stair_continues: bool,
    /// The lift shaft is built to the height of the future top stop.
    pub lift_shaft_continues: bool,
    /// Largest number of storeys the authority allows, with `limits_source`.
    pub max_storeys: Option<u32>,
    /// Largest building height, millimetres, from the ground-floor top to the top of the roof slab, with `limits_source`.
    pub max_height_mm: Option<f64>,
    pub limits_source: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HorizontalExpansion {
    /// South-west corner and size of the reserved wing, millimetres, in the model frame.
    pub x_mm: Option<f64>,
    pub y_mm: Option<f64>,
    pub width_mm: Option<f64>,
    pub length_mm: Option<f64>,
    /// The buildable rectangle `[x0, y0, x1, y1]` in millimetres (the plot inside its setbacks), with `limits_source`.
    pub bounds_mm: Option<[f64; 4]>,
    /// Largest built footprint divided by the plot area (0..1), with `limits_source`.
    pub max_coverage_ratio: Option<f64>,
    pub limits_source: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExpansionInput {
    pub vertical: Option<VerticalExpansion>,
    pub horizontal: Option<HorizontalExpansion>,
    /// Why the expansion is reserved and what was agreed (free text).
    pub note: Option<String>,
}

fn positive(name: &str, v: Option<f64>) -> Result<(), String> {
    match v {
        Some(x) if !x.is_finite() || x <= 0.0 => Err(format!("{name} must be a positive number")),
        _ => Ok(()),
    }
}

impl ExpansionInput {
    /// Everything wrong with the entered data, by name.
    pub fn check(&self) -> Result<(), String> {
        if let Some(v) = &self.vertical {
            if v.extra_storeys > MAX_EXTRA_STOREYS {
                return Err(format!("more than {MAX_EXTRA_STOREYS} future storeys is not supported"));
            }
            positive("the future storey height", v.storey_height_mm)?;
            positive("the maximum building height", v.max_height_mm)?;
            if v.max_storeys == Some(0) {
                return Err("the maximum number of storeys must be at least 1".into());
            }
        }
        if let Some(h) = &self.horizontal {
            for (n, x) in [("the wing width", h.width_mm), ("the wing length", h.length_mm)] {
                positive(n, x)?;
            }
            for (n, x) in [("the wing x", h.x_mm), ("the wing y", h.y_mm)] {
                if x.is_some_and(|x| !x.is_finite()) {
                    return Err(format!("{n} must be a number"));
                }
            }
            if let Some(b) = h.bounds_mm
                && (b.iter().any(|v| !v.is_finite()) || b[2] <= b[0] || b[3] <= b[1])
            {
                return Err("the buildable bounds must be [x0, y0, x1, y1] with x1 > x0 and y1 > y0".into());
            }
            if h.max_coverage_ratio.is_some_and(|r| !r.is_finite() || r <= 0.0 || r > 1.0) {
                return Err("the maximum coverage ratio must be above 0 and at most 1".into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    Pass,
    Fail,
    /// No limit (or no value to compare) was entered: nothing was compared.
    Unchecked,
}

fn judge(value: f64, limit: Option<f64>) -> Check {
    match limit {
        None => Check::Unchecked,
        Some(l) if value <= l + 1e-9 => Check::Pass,
        Some(_) => Check::Fail,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VerticalReport {
    pub existing_storeys: usize,
    pub extra_storeys: u32,
    pub total_storeys: usize,
    /// Ground-floor top to the top of the existing roof slab, millimetres.
    pub existing_height_mm: f64,
    pub added_height_mm: Option<f64>,
    pub total_height_mm: Option<f64>,
    pub storeys_check: Check,
    pub height_check: Check,
    /// The structural provisions that are not made, each with its consequence.
    pub missing_provisions: Vec<Finding>,
    pub limits_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HorizontalReport {
    pub area_m2: f64,
    /// Ground-floor area of the existing rooms, square metres.
    pub existing_footprint_m2: f64,
    pub footprint_after_m2: f64,
    pub plot_area_m2: Option<f64>,
    pub coverage_after: Option<f64>,
    pub coverage_check: Check,
    pub bounds_check: Check,
    /// Length of the wing's edge that touches the existing rooms, millimetres (0 when it floats free of the building).
    pub shared_edge_mm: f64,
    pub limits_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExpansionReport {
    pub vertical: Option<VerticalReport>,
    pub horizontal: Option<HorizontalReport>,
    pub findings: Vec<Finding>,
    pub note: Option<String>,
}

fn finding(ar: &str, en: &str) -> Finding {
    Finding { ar: ar.into(), en: en.into() }
}

/// A rectangle in metres: `[x0, y0, x1, y1]`.
type R = [f64; 4];

fn overlap_area(a: R, b: R) -> f64 {
    let w = a[2].min(b[2]) - a[0].max(b[0]);
    let h = a[3].min(b[3]) - a[1].max(b[1]);
    if w > 1e-9 && h > 1e-9 { w * h } else { 0.0 }
}

/// The length shared by the edges of two rectangles that touch without overlapping, metres.
fn shared_edge(a: R, b: R) -> f64 {
    let t = 1e-6;
    let mut len = 0.0;
    if (a[2] - b[0]).abs() < t || (b[2] - a[0]).abs() < t {
        len += (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    }
    if (a[3] - b[1]).abs() < t || (b[3] - a[1]).abs() < t {
        len += (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    }
    len
}

/// Reports the saved expansion against the built model. `plot_area_m2` is the project's plot area when known.
pub fn report(scene: &Scene, input: &ExpansionInput, plot_area_m2: Option<f64>) -> Result<ExpansionReport, String> {
    input.check()?;
    if input.vertical.is_none() && input.horizontal.is_none() {
        return Err("no expansion is saved: enter a vertical or a horizontal expansion".into());
    }
    let spaces: Vec<_> = scene.boxes.iter().filter(|b| b.kind == "Space").collect();
    if spaces.is_empty() {
        return Err("the model has no rooms".into());
    }
    let mut findings = Vec::new();

    let vertical = match &input.vertical {
        Some(v) if v.extra_storeys > 0 => {
            let existing = scene.levels.len().max(1);
            let roof_top = scene.boxes.iter().filter(|b| b.kind == "Slab").map(|b| b.max[2]).fold(f64::MIN, f64::max);
            if !roof_top.is_finite() {
                return Err("the model has no slabs".into());
            }
            let storey = v.storey_height_mm.ok_or("the future storey height is missing: the added height cannot be computed")?;
            let existing_mm = roof_top * 1000.0;
            let added = storey * f64::from(v.extra_storeys);
            let total_mm = existing_mm + added;
            let total_storeys = existing + v.extra_storeys as usize;
            let storeys_check = judge(total_storeys as f64, v.max_storeys.map(f64::from));
            let height_check = judge(total_mm, v.max_height_mm);
            let mut missing = Vec::new();
            for (made, ar, en) in [
                (v.columns_designed, "الأعمدة غير مصمَّمة للأدوار المستقبلية: إضافة دور تتطلب تقوية الأعمدة", "the columns are not designed for the future storeys: adding a storey will need column strengthening"),
                (v.foundation_designed, "الأساسات غير مصمَّمة للأدوار المستقبلية: إضافة دور تتطلب تقوية الأساسات", "the foundations are not designed for the future storeys: adding a storey will need foundation strengthening"),
                (v.stair_continues, "الدرج لا يستمر إلى الأدوار المستقبلية: يلزم فتح في سقف الدور الأخير", "the stair does not continue to the future storeys: an opening in the top slab will be needed"),
                (v.lift_shaft_continues, "بئر المصعد لا يصل إلى الدور المستقبلي: يلزم تمديده", "the lift shaft does not reach the future top stop: it will need extending"),
            ] {
                if !made {
                    missing.push(finding(ar, en));
                }
            }
            if storeys_check == Check::Fail {
                findings.push(finding("عدد الأدوار بعد التوسّع يتجاوز الحد المُدخل", "the storeys after expansion exceed the entered limit"));
            }
            if height_check == Check::Fail {
                findings.push(finding("ارتفاع المبنى بعد التوسّع يتجاوز الحد المُدخل", "the building height after expansion exceeds the entered limit"));
            }
            if storeys_check == Check::Unchecked || height_check == Check::Unchecked {
                findings.push(finding(
                    "حد الأدوار أو الارتفاع لم يُدخل: لم يُقارَن ولا يُعدّ ناجحاً",
                    "the storey or height limit was not entered: it was not compared and is not reported as passed",
                ));
            }
            Some(VerticalReport {
                existing_storeys: existing,
                extra_storeys: v.extra_storeys,
                total_storeys,
                existing_height_mm: existing_mm,
                added_height_mm: Some(added),
                total_height_mm: Some(total_mm),
                storeys_check,
                height_check,
                missing_provisions: missing,
                limits_source: v.limits_source.clone(),
            })
        }
        Some(_) => {
            findings.push(finding("عدد الأدوار المستقبلية صفر: لا توسّع رأسي محفوظ", "the number of future storeys is zero: no vertical expansion is saved"));
            None
        }
        None => None,
    };

    let horizontal = match &input.horizontal {
        Some(h) => {
            let need = |n: &str, v: Option<f64>| v.ok_or_else(|| format!("{n} is missing: the wing cannot be placed"));
            let (x, y, w, l) = (need("the wing x", h.x_mm)?, need("the wing y", h.y_mm)?, need("the wing width", h.width_mm)?, need("the wing length", h.length_mm)?);
            let wing: R = [x / 1000.0, y / 1000.0, (x + w) / 1000.0, (y + l) / 1000.0];
            let ground_z = spaces.iter().map(|b| b.min[2]).fold(f64::MAX, f64::min);
            let ground: Vec<R> = spaces.iter().filter(|b| (b.min[2] - ground_z).abs() < 1e-6).map(|b| [b.min[0], b.min[1], b.max[0], b.max[1]]).collect();
            if let Some(r) = ground.iter().find(|r| overlap_area(**r, wing) > 0.0) {
                return Err(format!(
                    "the reserved wing overlaps an existing room (x {:.0}..{:.0} mm, y {:.0}..{:.0} mm): move it clear of the building",
                    r[0] * 1000.0, r[2] * 1000.0, r[1] * 1000.0, r[3] * 1000.0
                ));
            }
            let existing: f64 = ground.iter().map(|r| (r[2] - r[0]) * (r[3] - r[1])).sum();
            let area = (w / 1000.0) * (l / 1000.0);
            let after = existing + area;
            let shared: f64 = ground.iter().map(|r| shared_edge(*r, wing)).sum::<f64>() * 1000.0;
            let bounds_check = match h.bounds_mm {
                None => Check::Unchecked,
                Some(b) => {
                    if x >= b[0] - 1e-6 && y >= b[1] - 1e-6 && x + w <= b[2] + 1e-6 && y + l <= b[3] + 1e-6 { Check::Pass } else { Check::Fail }
                }
            };
            let coverage_after = plot_area_m2.filter(|a| *a > 0.0).map(|a| after / a);
            let coverage_check = match (coverage_after, h.max_coverage_ratio) {
                (Some(c), limit) => judge(c, limit),
                (None, _) => Check::Unchecked,
            };
            if bounds_check == Check::Fail {
                findings.push(finding("الجناح المحجوز يخرج عن حدود البناء المُدخلة", "the reserved wing lies outside the entered buildable bounds"));
            }
            if coverage_check == Check::Fail {
                findings.push(finding("نسبة البناء بعد الجناح تتجاوز الحد المُدخل", "the coverage after the wing exceeds the entered limit"));
            }
            if bounds_check == Check::Unchecked || coverage_check == Check::Unchecked {
                findings.push(finding(
                    "حدود البناء أو نسبة التغطية أو مساحة الأرض غير مُدخلة: لم تُقارَن ولا تُعدّ ناجحة",
                    "the buildable bounds, the coverage limit or the plot area is missing: not compared and not reported as passed",
                ));
            }
            if shared <= 0.0 {
                findings.push(finding("الجناح المحجوز لا يلامس المبنى: لا يمكن وصله به دون ممر", "the reserved wing does not touch the building: it cannot be joined without a link"));
            }
            Some(HorizontalReport {
                area_m2: area,
                existing_footprint_m2: existing,
                footprint_after_m2: after,
                plot_area_m2,
                coverage_after,
                coverage_check,
                bounds_check,
                shared_edge_mm: shared,
                limits_source: h.limits_source.clone(),
            })
        }
        None => None,
    };

    Ok(ExpansionReport { vertical, horizontal, findings, note: input.note.clone() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model3d::{Box3, LevelInfo};
    use std::collections::BTreeMap;

    fn b(kind: &'static str, min: [f64; 3], max: [f64; 3]) -> Box3 {
        Box3 { id: format!("{kind}-{}", min[0]), kind, class: String::new(), min, max, visual: false, level: Some(0), discipline: "architectural" }
    }

    /// One 10 x 8 m room on the ground floor (plan 0..10 x 0..8), roof slab top at 3.2 m.
    fn scene() -> Scene {
        Scene {
            levels: vec![LevelInfo { id: "L1".into(), name: "L1".into(), index: 0, z0: 0.0, storey: 3.0 }],
            boxes: vec![b("Space", [0.0, 0.0, 0.0], [10.0, 8.0, 3.0]), b("Slab", [0.0, 0.0, 3.0], [10.0, 8.0, 3.2])],
            min: [0.0; 3],
            max: [10.0, 8.0, 3.2],
            storey_height: 3.0,
            assemblies: BTreeMap::new(),
        }
    }

    fn vert(extra: u32) -> VerticalExpansion {
        VerticalExpansion { extra_storeys: extra, storey_height_mm: Some(3200.0), ..Default::default() }
    }
    fn input_v(v: VerticalExpansion) -> ExpansionInput {
        ExpansionInput { vertical: Some(v), ..Default::default() }
    }
    fn wing(x: f64, y: f64, w: f64, l: f64) -> HorizontalExpansion {
        HorizontalExpansion { x_mm: Some(x), y_mm: Some(y), width_mm: Some(w), length_mm: Some(l), ..Default::default() }
    }
    fn input_h(h: HorizontalExpansion) -> ExpansionInput {
        ExpansionInput { horizontal: Some(h), ..Default::default() }
    }

    #[test]
    fn vertical_totals_are_exact_and_unset_limits_are_unchecked() {
        let r = report(&scene(), &input_v(vert(2)), None).unwrap();
        let v = r.vertical.unwrap();
        assert_eq!((v.existing_storeys, v.total_storeys), (1, 3));
        assert!((v.existing_height_mm - 3200.0).abs() < 1e-6);
        assert!((v.added_height_mm.unwrap() - 6400.0).abs() < 1e-9);
        assert!((v.total_height_mm.unwrap() - 9600.0).abs() < 1e-6);
        assert_eq!((v.storeys_check, v.height_check), (Check::Unchecked, Check::Unchecked));
        assert!(r.findings.iter().any(|f| f.en.contains("not reported as passed")));
    }

    #[test]
    fn vertical_limits_pass_and_fail_and_missing_provisions_are_listed() {
        let mut v = vert(2);
        v.max_storeys = Some(3);
        v.max_height_mm = Some(9000.0);
        v.columns_designed = true;
        v.stair_continues = true;
        let r = report(&scene(), &input_v(v), None).unwrap();
        let vr = r.vertical.unwrap();
        assert_eq!((vr.storeys_check, vr.height_check), (Check::Pass, Check::Fail));
        assert_eq!(vr.missing_provisions.len(), 2);
        assert!(vr.missing_provisions.iter().any(|f| f.en.contains("foundations")));
        assert!(vr.missing_provisions.iter().any(|f| f.en.contains("lift shaft")));
        assert!(r.findings.iter().any(|f| f.en.contains("height after expansion")));
    }

    #[test]
    fn a_missing_storey_height_is_named_not_defaulted() {
        let v = VerticalExpansion { extra_storeys: 1, ..Default::default() };
        let e = report(&scene(), &input_v(v), None).unwrap_err();
        assert!(e.contains("future storey height is missing"), "{e}");
    }

    #[test]
    fn horizontal_wing_area_coverage_bounds_and_contact_are_computed() {
        let mut h = wing(10000.0, 0.0, 4000.0, 8000.0);
        h.bounds_mm = Some([0.0, 0.0, 15000.0, 20000.0]);
        h.max_coverage_ratio = Some(0.6);
        let r = report(&scene(), &input_h(h), Some(200.0)).unwrap();
        let hr = r.horizontal.unwrap();
        assert!((hr.area_m2 - 32.0).abs() < 1e-9);
        assert!((hr.existing_footprint_m2 - 80.0).abs() < 1e-9);
        assert!((hr.footprint_after_m2 - 112.0).abs() < 1e-9);
        assert!((hr.coverage_after.unwrap() - 0.56).abs() < 1e-12);
        assert_eq!((hr.coverage_check, hr.bounds_check), (Check::Pass, Check::Pass));
        assert!((hr.shared_edge_mm - 8000.0).abs() < 1e-6);
    }

    #[test]
    fn horizontal_limits_fail_and_missing_ones_stay_unchecked() {
        let mut h = wing(10000.0, 0.0, 6000.0, 8000.0);
        h.bounds_mm = Some([0.0, 0.0, 15000.0, 20000.0]);
        h.max_coverage_ratio = Some(0.5);
        let r = report(&scene(), &input_h(h), Some(200.0)).unwrap();
        let hr = r.horizontal.unwrap();
        assert_eq!((hr.coverage_check, hr.bounds_check), (Check::Fail, Check::Fail));
        let none = report(&scene(), &input_h(wing(10000.0, 0.0, 4000.0, 8000.0)), None).unwrap();
        let hn = none.horizontal.unwrap();
        assert_eq!((hn.coverage_check, hn.bounds_check), (Check::Unchecked, Check::Unchecked));
    }

    #[test]
    fn a_wing_over_a_room_or_floating_free_is_handled_by_name() {
        let e = report(&scene(), &input_h(wing(9000.0, 0.0, 4000.0, 8000.0)), None).unwrap_err();
        assert!(e.contains("overlaps an existing room"), "{e}");
        let r = report(&scene(), &input_h(wing(12000.0, 0.0, 4000.0, 8000.0)), None).unwrap();
        assert_eq!(r.horizontal.unwrap().shared_edge_mm, 0.0);
        assert!(r.findings.iter().any(|f| f.en.contains("does not touch the building")));
    }

    #[test]
    fn bad_input_is_refused_with_the_reason() {
        assert!(report(&scene(), &ExpansionInput::default(), None).unwrap_err().contains("no expansion is saved"));
        assert!(input_v(vert(11)).check().unwrap_err().contains("more than 10"));
        let mut h = wing(0.0, 0.0, -1.0, 5.0);
        assert!(input_h(h.clone()).check().unwrap_err().contains("wing width"));
        h.width_mm = Some(1000.0);
        h.max_coverage_ratio = Some(1.5);
        assert!(input_h(h.clone()).check().unwrap_err().contains("coverage ratio"));
        h.max_coverage_ratio = None;
        h.bounds_mm = Some([5.0, 0.0, 1.0, 9.0]);
        assert!(input_h(h).check().unwrap_err().contains("buildable bounds"));
        let zero = report(&scene(), &input_v(vert(0)), None).unwrap();
        assert!(zero.vertical.is_none() && zero.findings.iter().any(|f| f.en.contains("zero")));
    }

    #[test]
    fn the_input_round_trips_through_json_for_saving() {
        let mut v = vert(1);
        v.limits_source = Some("municipality letter".into());
        let i = ExpansionInput { vertical: Some(v), horizontal: Some(wing(1.0, 2.0, 3.0, 4.0)), note: Some("phase 2".into()) };
        let back: ExpansionInput = serde_json::from_value(serde_json::to_value(&i).unwrap()).unwrap();
        assert_eq!(i, back);
    }
}
