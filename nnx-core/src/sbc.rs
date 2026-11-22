//! Saudi Building Code checks on the project and the model.
//!
//! Two kinds of limit feed the checks:
//! - **Code values** (`CODE_VALUES`): copied from the code text in `C:\Projects\البناء\الكود السعودي\`, each with the
//!   code, edition, clause and the PDF page it was read from. A value that was not read from the files is not in the
//!   table, and the check that needs it is reported as skipped.
//! - **Municipal limits** (`MunicipalLimits` in the project): setbacks and plot coverage are set by each municipality,
//!   so the user enters them with their source.
//!
//! A check never passes silently when its limit is missing: it appears in `SbcReport::skipped` with the reason.
//! The result is a preliminary check; it does not replace the approving authority.

use crate::building;
use crate::model::{Id, Model};
use crate::presets;
use crate::project::{self, NanoArchProject};
use crate::rules::Severity;
use crate::typed::{Space, Window};
use serde::Serialize;
use std::collections::BTreeSet;

/// One value read from the code text.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CodeValue {
    /// Key the checks look the value up by, for example `room_min_area_m2.bedroom`.
    pub id: &'static str,
    pub value: f64,
    pub unit: &'static str,
    /// For example `SBC 201`.
    pub code: &'static str,
    pub edition: &'static str,
    /// Clause or table number.
    pub clause: &'static str,
    /// PDF file the value was read from.
    pub file: &'static str,
    /// Page of that PDF.
    pub page: u32,
}

/// The Ministry of Municipal and Rural Affairs residential guidelines (اشتراطات إنشاء المباني السكنية), decision of 1446 AH.
/// It also cites SBC 1101 for several of these values; the SBC 1101 text itself is not among the files.
const MIN: &str = "Ministry residential guidelines";
const ED: &str = "1446 AH (2024)";
const FILE: &str = "ashtratat a'nsha almbany alsknyt almnshwrt ly mwq alwzart.pdf";

/// Every value read from the files so far (page numbers are physical PDF pages). Room areas and widths: clause 5-1-4,
/// page 26, checked on the rendered page. The 8% daylight ratio: clause 5-1-15 (2), page 31, read from extracted text
/// only; the contractor guide gives the same 8% for SBC 1101 section 303.1 (page 87). Not included because they could not
/// be taken from the code text itself: thermal U-limits (only second-hand in the contractor guide, page 172, whose
/// conditioned/unconditioned column labels conflict), ceiling heights (SBC 1101 sections 304/305, second-hand), and any
/// SBC 201 number (the Arabic edition has none). See THERMAL_NOTE_EN.
pub const CODE_VALUES: &[CodeValue] = &[
    CodeValue { id: "room_min_area_m2.majlis", value: 11.2, unit: "m2", code: MIN, edition: ED, clause: "5-1-4", file: FILE, page: 26 },
    CodeValue { id: "room_min_width_m.majlis", value: 2.8, unit: "m", code: MIN, edition: ED, clause: "5-1-4", file: FILE, page: 26 },
    CodeValue { id: "room_min_area_m2.dining", value: 11.2, unit: "m2", code: MIN, edition: ED, clause: "5-1-4", file: FILE, page: 26 },
    CodeValue { id: "room_min_width_m.dining", value: 2.8, unit: "m", code: MIN, edition: ED, clause: "5-1-4", file: FILE, page: 26 },
    CodeValue { id: "room_min_area_m2.living", value: 11.2, unit: "m2", code: MIN, edition: ED, clause: "5-1-4", file: FILE, page: 26 },
    CodeValue { id: "room_min_width_m.living", value: 2.8, unit: "m", code: MIN, edition: ED, clause: "5-1-4", file: FILE, page: 26 },
    CodeValue { id: "room_min_area_m2.bedroom", value: 6.5, unit: "m2", code: MIN, edition: ED, clause: "5-1-4", file: FILE, page: 26 },
    CodeValue { id: "room_min_width_m.bedroom", value: 2.1, unit: "m", code: MIN, edition: ED, clause: "5-1-4", file: FILE, page: 26 },
    CodeValue { id: "room_min_area_m2.kitchen", value: 5.0, unit: "m2", code: MIN, edition: ED, clause: "5-1-4", file: FILE, page: 26 },
    CodeValue { id: "room_min_width_m.kitchen", value: 1.8, unit: "m", code: MIN, edition: ED, clause: "5-1-4", file: FILE, page: 26 },
    CodeValue { id: "room_min_area_m2.bathroom", value: 2.8, unit: "m2", code: MIN, edition: ED, clause: "5-1-4", file: FILE, page: 26 },
    CodeValue { id: "room_min_width_m.bathroom", value: 1.4, unit: "m", code: MIN, edition: ED, clause: "5-1-4", file: FILE, page: 26 },
    CodeValue { id: "daylight_min_ratio.majlis", value: 0.08, unit: "ratio of floor area", code: MIN, edition: ED, clause: "5-1-15 (2)", file: FILE, page: 31 },
    CodeValue { id: "daylight_min_ratio.dining", value: 0.08, unit: "ratio of floor area", code: MIN, edition: ED, clause: "5-1-15 (2)", file: FILE, page: 31 },
    CodeValue { id: "daylight_min_ratio.living", value: 0.08, unit: "ratio of floor area", code: MIN, edition: ED, clause: "5-1-15 (2)", file: FILE, page: 31 },
    CodeValue { id: "daylight_min_ratio.bedroom", value: 0.08, unit: "ratio of floor area", code: MIN, edition: ED, clause: "5-1-15 (2)", file: FILE, page: 31 },
    CodeValue { id: "daylight_min_ratio.kitchen", value: 0.08, unit: "ratio of floor area", code: MIN, edition: ED, clause: "5-1-15 (2)", file: FILE, page: 31 },
];

/// Why the thermal limits are not checked.
pub const THERMAL_NOTE_EN: &str = "thermal U-limits: no value was read from SBC 602 itself (the file is not in the folder); the contractor guide quotes some with conflicting column labels. Enter the limits you rely on in the insulation inputs with their source.";
pub const THERMAL_NOTE_AR: &str = "حدود العزل الحراري: لم تُقرأ قيمة من SBC 602 نفسه (الملف غير موجود في المجلد)، والدليل الاسترشادي يذكر بعضها بتسميات أعمدة متعارضة. أدخل الحدود التي تعتمد عليها في مدخلات العزل مع مصدرها.";

/// Daylight targets for the window generator: room function -> glazed area as a fraction of floor area.
pub fn daylight_targets() -> std::collections::BTreeMap<String, f64> {
    CODE_VALUES.iter().filter_map(|v| v.id.strip_prefix("daylight_min_ratio.").map(|f| (f.to_string(), v.value))).collect()
}

pub fn code_value(id: &str) -> Option<&'static CodeValue> {
    CODE_VALUES.iter().find(|v| v.id == id)
}

/// The values in `CODE_VALUES` as a rule pack for the generic rule engine (the Code tab), so the tab runs without a pasted pack.
/// Every parameter carries its source (code, edition, clause, file, page). Nothing is added that is not in `CODE_VALUES`: a
/// function with no value gets no rule. Rooms without a measured glazing ratio are reported by the engine as not evaluable.
pub fn sourced_pack_json() -> String {
    use serde_json::json;
    let src = |v: &CodeValue| format!("{} {}, clause {}, {}, p.{}", v.code, v.edition, v.clause, v.file, v.page);
    let mut rules = Vec::new();
    for area in CODE_VALUES.iter().filter(|v| v.id.starts_with("room_min_area_m2.")) {
        let func = &area.id["room_min_area_m2.".len()..];
        let applies = json!({ "kind": "Space", "where": format!("func == \"{func}\"") });
        rules.push(json!({
            "id": format!("sbc.room_area.{func}"), "clause": area.clause, "applies_to": applies,
            "check": "area >= param.min", "params": { "min": { "value": area.value, "source": src(area) } },
            "severity": "error", "message": format!("{func}: floor area is below the minimum of {} m2", area.value),
        }));
        if let Some(w) = code_value(&format!("room_min_width_m.{func}")) {
            rules.push(json!({
                "id": format!("sbc.room_width.{func}"), "clause": w.clause, "applies_to": applies,
                "check": "min(w, h) >= param.min * 1000", "params": { "min": { "value": w.value, "source": src(w) } },
                "severity": "error", "message": format!("{func}: the narrower side is below the minimum of {} m", w.value),
            }));
        }
        if let Some(d) = code_value(&format!("daylight_min_ratio.{func}")) {
            rules.push(json!({
                "id": format!("sbc.daylight.{func}"), "clause": d.clause, "applies_to": applies,
                "check": "glazingRatio >= param.min", "params": { "min": { "value": d.value, "source": src(d) } },
                "severity": "warning", "message": format!("{func}: glazing is below {} of the floor area", d.value),
            }));
        }
    }
    // Spaces the guidelines give no daylight ratio for stay in the check with an unset limit: the engine reports the rule as
    // not run until the user enters a value with its source. No ratio is assumed.
    for func in ["other", "stair"] {
        rules.push(json!({
            "id": format!("sbc.daylight.{func}"), "clause": "none read", "applies_to": { "kind": "Space", "where": format!("func == \"{func}\"") },
            "check": "glazingRatio >= param.min",
            "params": { "min": { "value": null, "source": "no daylight ratio for this function was read from the code files; enter it with its source" } },
            "severity": "warning", "message": format!("{func}: glazing is below the limit you entered"),
        }));
    }
    json!({ "id": "SBC-SOURCED", "edition": "values read from the files in the code folder", "version": "1", "rules": rules }).to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Violation {
    /// Stable key of the rule, for example `setback.front`.
    pub rule: String,
    pub code: String,
    pub clause: String,
    /// Where the limit comes from, in words (file and page, or the municipal source the user entered).
    pub source: String,
    pub severity: Severity,
    pub element: Option<Id>,
    pub en: String,
    pub ar: String,
    pub value: f64,
    pub limit: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Skipped {
    pub rule: String,
    pub en: String,
    pub ar: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SbcReport {
    pub violations: Vec<Violation>,
    pub skipped: Vec<Skipped>,
    /// Number of individual comparisons made.
    pub checked: usize,
}

impl SbcReport {
    fn skip(&mut self, rule: &str, en: impl Into<String>, ar: impl Into<String>) {
        if !self.skipped.iter().any(|s| s.rule == rule) {
            self.skipped.push(Skipped { rule: rule.to_string(), en: en.into(), ar: ar.into() });
        }
    }
}

fn cite(v: &CodeValue) -> String {
    format!("{} {} {}, {} p.{}", v.code, v.edition, v.clause, v.file, v.page)
}

/// Runs every check on the model's project and drawing. A model without a project has nothing to check.
pub fn check(model: &Model) -> SbcReport {
    let mut r = SbcReport::default();
    let Some(pe) = project::find(model) else { return r };
    let Ok(p) = project::get(model).map(|p| p.unwrap_or_default()) else { return r };
    setbacks(&p, pe.id, &mut r);
    coverage(&p, model, pe.id, &mut r);
    thermal(&p, pe.id, &mut r);
    rooms(model, &mut r);
    daylight(model, &mut r);
    r.violations.sort_by(|a, b| (&a.rule, a.element).cmp(&(&b.rule, b.element)));
    r
}

fn setbacks(p: &NanoArchProject, pid: Id, r: &mut SbcReport) {
    let Some(b) = &p.brief else {
        r.skip("setback", "setbacks: the project has no brief", "الارتدادات: المشروع بلا برنامج معماري");
        return;
    };
    if p.plot_is_irregular() {
        r.skip("setback", "setbacks: the plot is not a rectangle, so every edge has its own setback from the site analysis (see the site plan); the front/rear/side limits do not apply", "الارتدادات: القطعة ليست مستطيلة، فلكل ضلع ارتداده من تحليل الموقع (انظر مخطط الموقع)؛ فلا تنطبق حدود الأمام والخلف والجانب");
        return;
    }
    let l = &p.limits;
    for (key, name_en, name_ar, entered, limit) in [
        ("setback.front", "front setback", "الارتداد الأمامي", b.setback_front_m, &l.min_setback_front_m),
        ("setback.back", "rear setback", "الارتداد الخلفي", b.setback_back_m, &l.min_setback_back_m),
        ("setback.side", "side setback", "الارتداد الجانبي", b.setback_side_m, &l.min_setback_side_m),
    ] {
        let Some((min, source)) = limit.as_ref().and_then(|p| p.value.map(|v| (v, p.source.clone()))) else {
            r.skip(key, format!("{name_en}: enter the municipality's minimum and its source"), format!("{name_ar}: أدخل الحد الأدنى لدى البلدية ومصدره"));
            continue;
        };
        r.checked += 1;
        if entered + 1e-9 < min {
            r.violations.push(Violation {
                rule: key.into(),
                code: "Municipal".into(),
                clause: String::new(),
                source,
                severity: Severity::Error,
                element: Some(pid),
                en: format!("The {name_en} is {entered} m; the minimum is {min} m"),
                ar: format!("{name_ar} {entered} م، والحد الأدنى {min} م"),
                value: entered,
                limit: min,
            });
        }
    }
}

/// Built footprint in m2: the bounding rectangle of the ground level's rooms (the building outline is a rectangle).
fn footprint_m2(model: &Model) -> Option<f64> {
    let ground = building::levels(model).first().map(|(_, id)| *id);
    let mut b: Option<(i64, i64, i64, i64)> = None;
    for e in model.elements.values().filter(|e| e.kind == "Space" && (ground.is_none() || e.level_id == ground)) {
        let s = e.typed::<Space>().ok()?;
        let (x0, y0, x1, y1) = (s.x, s.y, s.x + s.w, s.y + s.h);
        b = Some(b.map_or((x0, y0, x1, y1), |(a, c, d, f)| (a.min(x0), c.min(y0), d.max(x1), f.max(y1))));
    }
    b.map(|(x0, y0, x1, y1)| (x1 - x0) as f64 * (y1 - y0) as f64 / 1e6)
}

fn coverage(p: &NanoArchProject, model: &Model, pid: Id, r: &mut SbcReport) {
    let Some((max, source)) = p.limits.max_coverage_ratio.as_ref().and_then(|q| q.value.map(|v| (v, q.source.clone()))) else {
        r.skip("coverage", "plot coverage: enter the municipality's maximum ratio and its source", "نسبة البناء: أدخل الحد الأقصى لدى البلدية ومصدره");
        return;
    };
    let (Some(plot), Some(built)) = (p.plot_area_m2(), footprint_m2(model)) else {
        r.skip("coverage", "plot coverage: the project has no plot size or no rooms", "نسبة البناء: لا مساحة قطعة أو لا غرف");
        return;
    };
    r.checked += 1;
    let ratio = built / plot;
    if ratio > max + 1e-9 {
        r.violations.push(Violation {
            rule: "coverage".into(),
            code: "Municipal".into(),
            clause: String::new(),
            source,
            severity: Severity::Error,
            element: Some(pid),
            en: format!("The built footprint is {:.1}% of the plot; the maximum is {:.1}%", ratio * 100.0, max * 100.0),
            ar: format!("مساحة البناء {:.1}% من القطعة، والحد الأقصى {:.1}%", ratio * 100.0, max * 100.0),
            value: ratio,
            limit: max,
        });
    }
}

/// Thermal limits of the climate zone the project names, against the U-values of its insulation assemblies and the glazing U it
/// designs with. The limits are the guide's reproduction of SBC 602 Table 5.2 (secondary, with a caveat), so a breach is a
/// warning that cites the page, never an error that could refuse an edit. Without a zone, or with incomplete assemblies, the
/// check is skipped and says why.
fn thermal(p: &NanoArchProject, pid: Id, r: &mut SbcReport) {
    let Some(z) = p.meta.climate_zone.as_deref().and_then(presets::zone_of).and_then(presets::zone) else {
        r.skip("thermal", format!("{} Set the climate zone (1, 2, 3 or a city) in the project details.", THERMAL_NOTE_EN), format!("{} حدّد المنطقة المناخية (1 أو 2 أو 3 أو مدينة) في تفاصيل المشروع.", THERMAL_NOTE_AR));
        return;
    };
    let lib = p.mep.as_ref().and_then(|m| m.insulation.as_ref());
    let mut compare = |rule: &str, name_en: &str, name_ar: &str, u: f64, max: f64| {
        r.checked += 1;
        if u > max + 1e-9 {
            r.violations.push(Violation {
                rule: rule.into(),
                code: "SBC 602 (via the SBC 1101 guide)".into(),
                clause: format!("zone {}", z.zone),
                source: format!("{} {}", z.source.text(), presets::ZONE_CAVEAT_EN),
                severity: Severity::Warning,
                element: Some(pid),
                en: format!("The {name_en} U-value is {u:.3} W/m2.K; the zone {} maximum is {max} W/m2.K", z.zone),
                ar: format!("معامل انتقال حرارة {name_ar} {u:.3} واط/م².ك، والحد الأقصى للمنطقة {} هو {max} واط/م².ك", z.zone),
                value: u,
                limit: max,
            });
        }
    };
    let mut missing: Vec<String> = Vec::new();
    for (scope, en, ar, max) in [("wall", "wall", "الجدار", z.wall_u_max), ("roof", "roof", "السقف", z.roof_u_max)] {
        match lib.and_then(|l| l.assemblies.get(scope)).map(|a| a.u_value()) {
            Some(Ok(u)) => compare(&format!("thermal.{scope}.u"), en, ar, u, max),
            _ => missing.push(scope.to_string()),
        }
    }
    if let Some(wu) = p.mep.as_ref().and_then(|m| m.hvac.as_ref()).and_then(|h| h.window_u) {
        compare("thermal.window.u", "glazing", "الزجاج", wu, z.window_u_max);
    }
    if !missing.is_empty() {
        r.skip("thermal.assemblies", format!("thermal limits of zone {}: the {} insulation assembl{} must be complete (layers with thickness and lambda) to get a U-value", z.zone, missing.join(" and "), if missing.len() > 1 { "ies" } else { "y" }), format!("حدود المنطقة {}: يجب إكمال تجميعة العزل ({}) لحساب معامل الانتقال الحراري", z.zone, missing.join("، ")));
    }
}

/// Minimum area and width of each room function, where the code text gives one.
fn rooms(model: &Model, r: &mut SbcReport) {
    let mut missing: BTreeSet<String> = BTreeSet::new();
    for e in model.elements.values().filter(|e| e.kind == "Space") {
        let Ok(s) = e.typed::<Space>() else { continue };
        for (kind, en, ar, value, unit) in [
            ("room_min_area_m2", "area", "المساحة", s.area, "m2"),
            ("room_min_width_m", "width", "العرض", s.w.min(s.h) as f64 / 1000.0, "m"),
        ] {
            let id = format!("{kind}.{}", s.func);
            let Some(v) = code_value(&id) else {
                missing.insert(id);
                continue;
            };
            r.checked += 1;
            if value + 1e-9 < v.value {
                r.violations.push(Violation {
                    rule: id,
                    code: v.code.into(),
                    clause: v.clause.into(),
                    source: cite(v),
                    severity: Severity::Error,
                    element: Some(e.id),
                    en: format!("{} ({}): {en} {value:.2} {unit} is below the minimum {} {unit}", s.requirement_key, s.func, v.value),
                    ar: format!("{} ({}): {ar} {value:.2} {unit} أقل من الحد الأدنى {} {unit}", s.requirement_key, s.func, v.value),
                    value,
                    limit: v.value,
                });
            }
        }
    }
    for id in missing {
        r.skip(&id, format!("{id}: no value was read from the code files, so it is not checked"), format!("{id}: لم تُقرأ قيمة من ملفات الكود، فلا يُفحص"));
    }
}

/// Glazed area over floor area of rooms that need daylight, where the code text gives a ratio.
fn daylight(model: &Model, r: &mut SbcReport) {
    let mut missing: BTreeSet<String> = BTreeSet::new();
    for e in model.elements.values().filter(|e| e.kind == "Space") {
        let Ok(s) = e.typed::<Space>() else { continue };
        let id = format!("daylight_min_ratio.{}", s.func);
        let Some(v) = code_value(&id) else {
            missing.insert(id);
            continue;
        };
        let glazed: f64 = model
            .elements
            .values()
            .filter(|w| w.kind == "Window")
            .filter_map(|w| w.typed::<Window>().ok())
            .filter(|w| w.room == s.requirement_key)
            .map(|w| w.width as f64 * w.height as f64 / 1e6)
            .sum();
        r.checked += 1;
        let ratio = glazed / s.area;
        if ratio + 1e-9 < v.value {
            r.violations.push(Violation {
                rule: id,
                code: v.code.into(),
                clause: v.clause.into(),
                source: cite(v),
                severity: Severity::Error,
                element: Some(e.id),
                en: format!("{} ({}): glazing is {:.1}% of the floor area; the minimum is {:.1}%", s.requirement_key, s.func, ratio * 100.0, v.value * 100.0),
                ar: format!("{} ({}): الزجاج {:.1}% من مساحة الأرضية، والحد الأدنى {:.1}%", s.requirement_key, s.func, ratio * 100.0, v.value * 100.0),
                value: ratio,
                limit: v.value,
            });
        }
    }
    for id in missing {
        r.skip(&id, format!("{id}: no value was read from the code files, so it is not checked"), format!("{id}: لم تُقرأ قيمة من ملفات الكود، فلا يُفحص"));
    }
}

fn key(v: &Violation) -> (String, Option<Id>) {
    (v.rule.clone(), v.element)
}

/// Violations present in `after` that were not in `before`: what an edit introduced.
pub fn introduced(before: &Model, after: &Model) -> Vec<Violation> {
    let old: BTreeSet<_> = check(before).violations.iter().map(key).collect();
    check(after).violations.into_iter().filter(|v| !old.contains(&key(v))).collect()
}

/// Whether the project in `model` refuses edits that add code violations.
pub fn enforced(model: &Model) -> bool {
    project::get(model).ok().flatten().is_some_and(|p| p.limits.enforce)
}

/// The refusal message for an edit that turns `before` into `after`, or `None` if it is allowed: the project does
/// not enforce the code, or the edit introduces no error.
pub fn refusal(before: &Model, after: &Model) -> Option<String> {
    if !enforced(after) {
        return None;
    }
    introduced(before, after).into_iter().find(|v| v.severity == Severity::Error).map(|v| format!("{} / {} [{} {}]", v.en, v.ar, v.code, v.clause))
}
