//! Zoning check: the model against the limits of one municipality entry (setbacks, coverage, height, floors, floor-area ratio).
//!
//! The limits are the user's own, from the municipality table (`municipality`), each with its source. This check reads the entry
//! directly, so it works without `municipality_apply` and does not touch the project's `limits`. A limit the entry leaves unset, or a
//! quantity the model cannot supply, gives `unchecked` and names what is missing; success is never declared for it. A failure
//! outranks an unchecked row in the overall verdict.
//!
//! What is measured, and the assumptions that come with it (all repeated in `notes`):
//! - Setbacks: the front, rear and side distances the user entered in the brief (rectangular plots only).
//! - Coverage: the bounding rectangle of the ground level's rooms over the plot area (the same footprint `sbc` uses).
//! - Height: the highest top of a wall, slab, column or beam, above the top of the ground-level floor (not above the site grade;
//!   parapets, tanks and lift overruns are included only if they are modelled).
//! - Floors: the number of levels that hold at least one room.
//! - Floor-area ratio: the sum over those levels of the bounding rectangle of their rooms, over the plot area.
//!
//! A preliminary check, not an approval by the municipality.

use crate::model::{Id, Model};
use crate::municipality::MunicipalityEntry;
use crate::project::{self, NanoArchProject};
use crate::rules::Param;
use crate::typed::{Space, Typed};
use serde::Serialize;

const EPS: f64 = 1e-9;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Passes,
    Fails,
    Unchecked,
}

/// What the model supplies. `None` is a quantity that could not be measured.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Measured {
    pub setback_front_m: Option<f64>,
    pub setback_back_m: Option<f64>,
    pub setback_side_m: Option<f64>,
    pub plot_area_m2: Option<f64>,
    pub footprint_m2: Option<f64>,
    pub height_m: Option<f64>,
    pub floors: Option<u32>,
    pub floor_area_m2: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    pub rule: &'static str,
    pub name_en: &'static str,
    pub name_ar: &'static str,
    pub status: Status,
    /// The measured value, in the unit of the limit.
    pub value: Option<f64>,
    pub limit: Option<f64>,
    /// The source the user gave for the limit.
    pub source: Option<String>,
    pub en: String,
    pub ar: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ZoningReport {
    pub entry_id: String,
    pub municipality: String,
    pub zone: String,
    pub document: String,
    pub rows: Vec<Row>,
    pub overall: Status,
    pub measured: Measured,
    pub notes: Vec<String>,
}

fn limit_of(p: &Option<Param>) -> Option<(f64, String)> {
    p.as_ref().and_then(|p| p.value.map(|v| (v, p.source.clone())))
}

/// Which way a limit binds.
#[derive(Clone, Copy)]
enum Bound {
    /// The measured value must be at least the limit.
    Min,
    /// The measured value must not exceed the limit.
    Max,
}

struct Spec {
    rule: &'static str,
    name_en: &'static str,
    name_ar: &'static str,
    unit: &'static str,
    bound: Bound,
    limit: Option<(f64, String)>,
    value: Option<f64>,
    /// Why the value is missing (English, Arabic).
    missing: (&'static str, &'static str),
}

fn row(s: Spec) -> Row {
    let Spec { rule, name_en, name_ar, unit, bound, limit, value, missing } = s;
    let Some((lim, source)) = limit else {
        return Row {
            rule,
            name_en,
            name_ar,
            status: Status::Unchecked,
            value,
            limit: None,
            source: None,
            en: format!("{name_en}: the municipality entry sets no limit, so it was not checked"),
            ar: format!("{name_ar}: لا يحدد إدخال البلدية حداً فلم يُفحص"),
        };
    };
    let Some(v) = value else {
        return Row {
            rule,
            name_en,
            name_ar,
            status: Status::Unchecked,
            value: None,
            limit: Some(lim),
            source: Some(source),
            en: format!("{name_en}: {}", missing.0),
            ar: format!("{name_ar}: {}", missing.1),
        };
    };
    let ok = match bound {
        Bound::Min => v + EPS >= lim,
        Bound::Max => v <= lim + EPS,
    };
    let (word_en, word_ar) = match bound {
        Bound::Min => ("minimum", "الحد الأدنى"),
        Bound::Max => ("maximum", "الحد الأقصى"),
    };
    let with_unit = |x: f64| format!("{x:.2} {unit}").trim_end().to_string();
    let (value_text, limit_text) = (with_unit(v), with_unit(lim));
    let (en, ar) = if ok {
        (format!("{name_en} {value_text} meets the {word_en} {limit_text}"), format!("{name_ar} {value_text} يحقق {word_ar} {limit_text}"))
    } else {
        (format!("{name_en} {value_text} breaks the {word_en} {limit_text}"), format!("{name_ar} {value_text} يخالف {word_ar} {limit_text}"))
    };
    Row { rule, name_en, name_ar, status: if ok { Status::Passes } else { Status::Fails }, value: Some(v), limit: Some(lim), source: Some(source), en, ar }
}

/// Judges `m` against the limits of `e`.
pub fn check(e: &MunicipalityEntry, m: &Measured) -> ZoningReport {
    let setback_missing = ("the plot is not a rectangle or the project has no brief, so the entered setback is not available", "القطعة ليست مستطيلة أو لا برنامج معماري للمشروع فلا ارتداد مُدخل");
    let area_missing = ("the project has no plot size or the model has no rooms", "لا مساحة قطعة للمشروع أو لا غرف في النموذج");
    let ratio = |a: Option<f64>| match (a, m.plot_area_m2) {
        (Some(a), Some(plot)) if plot > 0.0 => Some(a / plot),
        _ => None,
    };
    let rows = vec![
        row(Spec { rule: "front_setback", name_en: "Front setback", name_ar: "الارتداد الأمامي", unit: "m", bound: Bound::Min, limit: limit_of(&e.min_setback_front_m), value: m.setback_front_m, missing: setback_missing }),
        row(Spec { rule: "rear_setback", name_en: "Rear setback", name_ar: "الارتداد الخلفي", unit: "m", bound: Bound::Min, limit: limit_of(&e.min_setback_back_m), value: m.setback_back_m, missing: setback_missing }),
        row(Spec { rule: "side_setback", name_en: "Side setback", name_ar: "الارتداد الجانبي", unit: "m", bound: Bound::Min, limit: limit_of(&e.min_setback_side_m), value: m.setback_side_m, missing: setback_missing }),
        row(Spec { rule: "coverage", name_en: "Plot coverage", name_ar: "نسبة البناء", unit: "", bound: Bound::Max, limit: limit_of(&e.max_coverage_ratio), value: ratio(m.footprint_m2), missing: area_missing }),
        row(Spec { rule: "height", name_en: "Building height", name_ar: "ارتفاع المبنى", unit: "m", bound: Bound::Max, limit: limit_of(&e.max_height_m), value: m.height_m, missing: ("the model has no walls, slabs, columns or beams to measure", "لا جدران أو بلاطات أو أعمدة أو كمرات في النموذج لقياس الارتفاع") }),
        row(Spec { rule: "floors", name_en: "Number of floors", name_ar: "عدد الأدوار", unit: "", bound: Bound::Max, limit: limit_of(&e.max_floors), value: m.floors.map(f64::from), missing: ("the model has no rooms to count floors from", "لا غرف في النموذج لعدّ الأدوار") }),
        row(Spec { rule: "far", name_en: "Floor-area ratio", name_ar: "نسبة المساحة الإجمالية", unit: "", bound: Bound::Max, limit: limit_of(&e.max_far), value: ratio(m.floor_area_m2), missing: area_missing }),
    ];
    let overall = if rows.iter().any(|r| r.status == Status::Fails) {
        Status::Fails
    } else if rows.iter().any(|r| r.status == Status::Unchecked) {
        Status::Unchecked
    } else {
        Status::Passes
    };
    ZoningReport {
        entry_id: e.id.clone(),
        municipality: e.municipality.clone(),
        zone: e.zone.clone(),
        document: e.document.clone(),
        rows,
        overall,
        measured: m.clone(),
        notes: vec![
            "Setbacks are the values entered in the brief; coverage and floor-area ratio use the bounding rectangle of each level's rooms (wall thickness and projections outside the rooms are not added).".into(),
            "Height is measured from the top of the ground-level floor to the highest wall, slab, column or beam in the model, not from the site grade.".into(),
            "Floors are the levels that hold at least one room. Preliminary check: it does not replace the municipality's approval.".into(),
        ],
    }
}

/// Measures the model of project `p`.
pub fn measure(p: &NanoArchProject, model: &Model) -> Measured {
    let mut m = Measured { plot_area_m2: p.plot_area_m2(), ..Default::default() };
    if let Some(b) = p.brief.as_ref().filter(|_| !p.plot_is_irregular()) {
        m.setback_front_m = Some(b.setback_front_m);
        m.setback_back_m = Some(b.setback_back_m);
        m.setback_side_m = Some(b.setback_side_m);
    }
    let levels = crate::building::levels(model);
    let has_room = |level: Option<Id>| model.elements.values().any(|e| e.kind == Space::KIND && (level.is_none() || e.level_id == level));
    // A level counts when it holds a room. A model without levels is one floor.
    let floors: Vec<Option<Id>> = if levels.is_empty() { if has_room(None) { vec![None] } else { Vec::new() } } else { levels.iter().map(|(_, id)| Some(*id)).filter(|l| has_room(*l)).collect() };
    if !floors.is_empty() {
        m.floors = u32::try_from(floors.len()).ok();
        m.footprint_m2 = crate::sbc::level_footprint_m2(model, levels.first().map(|(_, id)| *id));
        let areas: Option<Vec<f64>> = floors.iter().map(|l| crate::sbc::level_footprint_m2(model, *l)).collect();
        m.floor_area_m2 = areas.map(|a| a.iter().sum());
    }
    let scene = crate::model3d::build(model);
    m.height_m = scene.boxes.iter().filter(|b| matches!(b.kind, "Wall" | "Slab" | "Column" | "Beam")).map(|b| b.max[2]).reduce(f64::max);
    m
}

/// Checks the open model against entry `id` of the project's municipality table.
pub fn run(model: &Model, id: &str) -> Result<ZoningReport, String> {
    let p = project::get(model)?.ok_or("the model has no project")?;
    let e = p.municipalities.iter().find(|e| e.id == id).ok_or_else(|| format!("no municipality entry '{id}'"))?;
    Ok(check(e, &measure(&p, model)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn param(v: f64) -> Option<Param> {
        Some(Param { value: Some(v), source: "synthetic regulation, art. 2".into() })
    }

    fn entry() -> MunicipalityEntry {
        MunicipalityEntry {
            id: "mn1".into(),
            municipality: "Test City".into(),
            zone: "R1".into(),
            document: "synthetic circular".into(),
            min_setback_front_m: param(3.0),
            max_coverage_ratio: param(0.6),
            max_height_m: param(12.0),
            max_floors: param(3.0),
            max_far: param(1.2),
            ..Default::default()
        }
    }

    fn good() -> Measured {
        Measured { setback_front_m: Some(4.0), setback_back_m: Some(2.0), setback_side_m: Some(1.5), plot_area_m2: Some(400.0), footprint_m2: Some(200.0), height_m: Some(9.0), floors: Some(2), floor_area_m2: Some(400.0) }
    }

    fn status<'a>(r: &'a ZoningReport, rule: &str) -> &'a Row {
        r.rows.iter().find(|x| x.rule == rule).unwrap()
    }

    #[test]
    fn limits_that_hold_pass_and_unset_limits_stay_unchecked() {
        let r = check(&entry(), &good());
        for rule in ["front_setback", "coverage", "height", "floors", "far"] {
            assert_eq!(status(&r, rule).status, Status::Passes, "{rule}");
        }
        assert_eq!(status(&r, "rear_setback").status, Status::Unchecked);
        assert_eq!(status(&r, "side_setback").status, Status::Unchecked);
        assert!(status(&r, "rear_setback").limit.is_none() && status(&r, "rear_setback").source.is_none());
        assert_eq!(r.overall, Status::Unchecked, "a skipped limit must keep the overall verdict from reading as a pass");
        assert_eq!(status(&r, "front_setback").source.as_deref(), Some("synthetic regulation, art. 2"));
    }

    #[test]
    fn every_breach_is_reported_and_a_failure_outranks_unchecked() {
        let m = Measured { setback_front_m: Some(2.0), footprint_m2: Some(300.0), height_m: Some(13.0), floors: Some(4), floor_area_m2: Some(600.0), ..good() };
        let r = check(&entry(), &m);
        for rule in ["front_setback", "coverage", "height", "floors", "far"] {
            assert_eq!(status(&r, rule).status, Status::Fails, "{rule}");
        }
        assert_eq!(r.overall, Status::Fails);
        assert!(status(&r, "coverage").en.contains("breaks") && status(&r, "coverage").ar.contains("يخالف"));
    }

    #[test]
    fn a_value_exactly_at_the_limit_passes() {
        let m = Measured { setback_front_m: Some(3.0), footprint_m2: Some(240.0), height_m: Some(12.0), floors: Some(3), floor_area_m2: Some(480.0), ..good() };
        let r = check(&entry(), &m);
        for rule in ["front_setback", "coverage", "height", "floors", "far"] {
            assert_eq!(status(&r, rule).status, Status::Passes, "{rule}");
        }
    }

    #[test]
    fn a_limit_with_nothing_to_measure_is_unchecked_not_passed() {
        let r = check(&entry(), &Measured::default());
        assert!(r.rows.iter().all(|x| x.status == Status::Unchecked));
        assert!(status(&r, "height").limit == Some(12.0), "the limit is still reported");
        assert!(status(&r, "height").en.contains("no walls"));
        // A zero plot area is not a divisor.
        let z = Measured { plot_area_m2: Some(0.0), footprint_m2: Some(10.0), ..Default::default() };
        assert_eq!(status(&check(&entry(), &z), "coverage").status, Status::Unchecked);
    }

    #[test]
    fn run_refuses_an_unknown_entry_and_a_model_without_a_project() {
        let model = Model::default();
        assert!(run(&model, "mn1").is_err());
    }
}
