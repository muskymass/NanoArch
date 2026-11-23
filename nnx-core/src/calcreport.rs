//! Reviewable structural calculation report: the assumptions first, then the results, then what was not computed.
//!
//! The report is assembled from what the project already owns: the structural inputs (`StructIn`), the sizing inputs
//! (`SizingInputs`, with the design parameters of `rebar`) and the `SizingReport` computed from them. It computes nothing of its
//! own and adds no value: an assumption row is either the number the user entered (source "user input") or `not set`, and a
//! result row is copied from the sizing report. A reviewer can therefore read every figure back to the input that produced it.
//!
//! A report whose inputs are incomplete is still produced, but each missing input is listed by name under "Not computed" and the
//! overall `status` is `incomplete`; it is `complete` only when no input is missing and no row of the report failed a check.
//! The report is a preliminary calculation record. It does not replace a licensed structural engineer or the approving authority.

use crate::project::StructIn;
use crate::sizing::{SizingInputs, SizingReport};
use serde::Serialize;
use std::fmt::Write;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Assumption {
    /// Dotted input name, as the user enters it (`structure.bearing_kpa`).
    pub key: String,
    pub label_ar: String,
    pub label_en: String,
    pub unit: String,
    /// The entered value as text; `None` when the input was not set.
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResultRow {
    pub section: String,
    /// Element mark (`COL-1`, `FND-2`, ...).
    pub element: String,
    pub detail: String,
    /// `ok`, `undersized`, `no soil value`, `section too small`, ... exactly as the sizing report states it; empty when none.
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Totals {
    pub concrete_m3: f64,
    pub steel_kg: f64,
    pub columns: usize,
    pub footings: usize,
    pub tie_beams: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CalcReport {
    pub title: String,
    pub assumptions: Vec<Assumption>,
    pub results: Vec<ResultRow>,
    pub totals: Totals,
    /// Section -> the inputs whose absence stopped it (copied from the sizing report).
    pub not_computed: Vec<(String, Vec<String>)>,
    pub warnings: Vec<String>,
    /// Inputs of the assumptions table that are not set.
    pub missing_assumptions: Vec<String>,
    /// Rows whose status is a failed check.
    pub failed: Vec<String>,
    /// `complete` or `incomplete`.
    pub status: &'static str,
    /// What the calculation does not cover, in both languages.
    pub limitations_ar: Vec<String>,
    pub limitations_en: Vec<String>,
}

fn num(v: Option<f64>) -> Option<String> {
    v.filter(|x| x.is_finite()).map(|x| format!("{x}"))
}

struct Rows(Vec<Assumption>);

impl Rows {
    fn add(&mut self, key: &str, ar: &str, en: &str, unit: &str, value: Option<String>) {
        self.0.push(Assumption { key: key.into(), label_ar: ar.into(), label_en: en.into(), unit: unit.into(), value });
    }
    fn f(&mut self, key: &str, ar: &str, en: &str, unit: &str, v: Option<f64>) {
        self.add(key, ar, en, unit, num(v));
    }
}

fn exposure(e: Option<crate::presets::Exposure>) -> Option<String> {
    e.and_then(|x| serde_json::to_value(x).ok()).and_then(|v| v.as_str().map(str::to_string))
}

fn failed_status(s: &str) -> bool {
    matches!(s, "undersized" | "section too small" | "no arrangement fits")
}

/// Builds the report. `inputs` are the sizing inputs as stored in the project (the floor load of `structure` is applied here, the
/// same way the `sizing` command applies it); `report` is the sizing report computed from them.
pub fn build(title: &str, structure: &StructIn, inputs: &SizingInputs, report: &SizingReport) -> CalcReport {
    let loaded = inputs.with_floor_load(structure);
    let mut a = Rows(Vec::new());
    a.f("structure.dead_kpa", "الحمل الميت", "Dead load", "kPa", structure.dead_kpa);
    a.f("structure.live_kpa", "الحمل الحي", "Live load", "kPa", structure.live_kpa);
    a.f("sizing.floor_service_kpa", "حمل الخدمة للبلاطة (ميت + حي)", "Floor service load (dead + live)", "kPa", loaded.floor_service_kpa);
    a.f("structure.bearing_kpa", "قدرة تحمل التربة", "Soil allowable bearing", "kPa", structure.bearing_kpa);
    a.add("structure.floors", "عدد الأدوار", "Number of floors", "", structure.floors.map(|v| v.to_string()));
    a.add("structure.max_span_mm", "أقصى بحر", "Maximum span", "mm", structure.max_span_mm.map(|v| v.to_string()));
    a.f("sizing.footing_thickness_mm", "سماكة الأساس", "Footing thickness", "mm", inputs.footing_thickness_mm);
    let tb = inputs.tie_beam.clone().unwrap_or_default();
    a.f("sizing.tie_beam.width_mm", "عرض كمرة الربط", "Tie beam width", "mm", tb.width_mm);
    a.f("sizing.tie_beam.span_depth_ratio", "نسبة البحر إلى العمق لكمرة الربط", "Tie beam span/depth ratio", "", tb.span_depth_ratio);
    let rb = inputs.rebar.clone().unwrap_or_default();
    a.f("sizing.rebar.cover_mm", "الغطاء الخرساني", "Concrete cover", "mm", rb.cover_mm);
    a.f("sizing.rebar.anchorage_mm", "طول تثبيت أسياخ العمود في الأساس", "Column bar anchorage", "mm", rb.anchorage_mm);
    a.f("sizing.rebar.hook_mm", "سماحية الخطاف", "Hook allowance", "mm", rb.hook_mm);
    let d = rb.design.clone().unwrap_or_default();
    a.f("design.fc_mpa", "مقاومة الخرسانة f'c", "Concrete strength f'c", "MPa", d.fc_mpa);
    a.add("design.exposure_class", "فئة تعرض الخرسانة", "Concrete exposure class", "", d.exposure_class.clone());
    a.f("design.fy_mpa", "إجهاد خضوع الحديد fy", "Steel yield strength fy", "MPa", d.fy_mpa);
    a.add(
        "design.bar_dias_mm",
        "أقطار الأسياخ المتاحة",
        "Available bar diameters",
        "mm",
        (!d.bar_dias_mm.is_empty()).then(|| d.bar_dias_mm.iter().map(|v| format!("{v}")).collect::<Vec<_>>().join(", ")),
    );
    a.f("design.min_clear_spacing_mm", "أدنى مسافة صافية بين الأسياخ", "Minimum clear bar spacing", "mm", d.min_clear_spacing_mm);
    a.f("design.load_factor", "معامل تضخيم الحمل", "Load factor", "", d.load_factor);
    let ex = d.exposure.clone().unwrap_or_default();
    a.add("design.exposure.column", "تعرض العمود", "Column exposure", "", exposure(ex.column));
    a.add("design.exposure.footing", "تعرض الأساس", "Footing exposure", "", exposure(ex.footing));
    a.add("design.exposure.beam", "تعرض الكمرة", "Beam exposure", "", exposure(ex.beam));
    a.add("design.exposure.tie_beam", "تعرض كمرة الربط", "Tie beam exposure", "", exposure(ex.tie_beam));
    let col = d.column.clone().unwrap_or_default();
    a.f("design.column.phi", "معامل خفض المقاومة للعمود", "Column strength-reduction factor", "", col.phi);
    a.f("design.column.alpha", "معامل أقصى سعة محورية للعمود", "Column maximum axial capacity factor", "", col.alpha);
    a.f("design.column.ratio_min", "أدنى نسبة تسليح للعمود", "Column minimum steel ratio", "", col.ratio_min);
    a.f("design.column.ratio_max", "أعلى نسبة تسليح للعمود", "Column maximum steel ratio", "", col.ratio_max);
    let fnd = d.footing.clone().unwrap_or_default();
    a.f("design.footing.phi_flexure", "معامل خفض مقاومة الانعطاف للأساس", "Footing flexure reduction factor", "", fnd.phi_flexure);
    a.f("design.footing.ratio_min", "أدنى نسبة تسليح للأساس", "Footing minimum steel ratio", "", fnd.ratio_min);
    a.f("design.footing.max_spacing_mm", "أقصى تباعد لأسياخ الأساس", "Footing maximum bar spacing", "mm", fnd.max_spacing_mm);
    let bm = d.beam.clone().unwrap_or_default();
    a.f("design.beam.phi_flexure", "معامل خفض مقاومة الانعطاف للكمرة", "Beam flexure reduction factor", "", bm.phi_flexure);
    a.f("design.beam.concrete_unit_weight_kn_m3", "الوزن الحجمي للخرسانة", "Concrete unit weight", "kN/m3", bm.concrete_unit_weight_kn_m3);
    a.f("design.beam.phi_shear", "معامل خفض مقاومة القص للكمرة", "Beam shear reduction factor", "", bm.phi_shear);
    a.f("design.beam.concrete_shear_factor", "معامل مقاومة الخرسانة للقص", "Concrete shear factor", "", bm.concrete_shear_factor);
    a.f("design.beam.ratio_min", "أدنى نسبة تسليح للكمرة", "Beam minimum steel ratio", "", bm.ratio_min);
    let assumptions = a.0;
    let missing_assumptions: Vec<String> = assumptions.iter().filter(|x| x.value.is_none()).map(|x| x.key.clone()).collect();

    let mut results: Vec<ResultRow> = Vec::new();
    for c in &report.columns {
        let load = c.axial_kn.map_or("axial load not available".to_string(), |v| format!("N = {v} kN"));
        results.push(ResultRow {
            section: "columns".into(),
            element: c.name.clone(),
            detail: format!("{} at {}, tributary {} m2, {load}", c.section, c.grid_ref, c.tributary_m2),
            status: String::new(),
        });
    }
    for f in &report.footings {
        let need = match (f.required_m2, f.required_side_mm) {
            (Some(m), Some(s)) => format!(", required {m} m2 (square side {s} mm)"),
            _ => String::new(),
        };
        results.push(ResultRow {
            section: "footings".into(),
            element: f.name.clone(),
            detail: format!("{} under {}, load {} kN at {} kPa{need}", f.provided_mm, f.column, f.load_kn, f.bearing_kpa),
            status: f.status.clone(),
        });
    }
    for t in &report.tie_beams {
        results.push(ResultRow {
            section: "tie_beams".into(),
            element: t.name.clone(),
            detail: format!("{} x {} mm, clear {} mm, {} to {}", t.width_mm, t.depth_mm, t.clear_mm, t.from, t.to),
            status: String::new(),
        });
    }
    for r in &report.designs {
        let notes = if r.notes.is_empty() { String::new() } else { format!("; {}", r.notes) };
        let detail = format!("{}: required {} mm2, provided {} mm2 ({} %), {}, cover {} mm{notes}", r.kind, r.required_mm2, r.provided_mm2, r.ratio_pct, r.bars, r.cover_mm);
        results.push(ResultRow { section: "reinforcement".into(), element: r.element.clone(), detail, status: r.status.clone() });
    }
    let failed: Vec<String> = results.iter().filter(|r| failed_status(&r.status)).map(|r| format!("{} ({})", r.element, r.status)).collect();
    let not_computed: Vec<(String, Vec<String>)> = report.missing.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let status = if missing_assumptions.is_empty() && not_computed.is_empty() && failed.is_empty() { "complete" } else { "incomplete" };

    CalcReport {
        title: title.to_string(),
        assumptions,
        results,
        totals: Totals {
            concrete_m3: report.concrete_total_m3,
            steel_kg: report.steel_total_kg,
            columns: report.columns.len(),
            footings: report.footings.len(),
            tie_beams: report.tie_beams.len(),
        },
        not_computed,
        warnings: report.warnings.clone(),
        missing_assumptions,
        failed,
        status,
        limitations_ar: vec![
            "حساب أولي لا يغني عن مهندس إنشائي مرخّص ولا عن جهة الاعتماد.".into(),
            "لا تحليل زلزالي ولا رياح، ولا قص ثاقب ولا استقرار ولا انحراف إلا ما صدر من أوامره المستقلة.".into(),
            "كل معامل ومقاومة وحد في جدول الفرضيات مدخل من المستخدم ولم يُقرأ من الكود في هذا التقرير.".into(),
        ],
        limitations_en: vec![
            "Preliminary calculation: it does not replace a licensed structural engineer or the approving authority.".into(),
            "No seismic or wind analysis, and no punching shear, stability or drift unless their own commands were run.".into(),
            "Every factor, strength and limit in the assumptions table is the user's input; none was read from a code by this report.".into(),
        ],
    }
}

impl CalcReport {
    /// The report as Markdown, `ar` or `en`: assumptions, then results, then what was not computed, then the limits.
    pub fn markdown(&self, lang: &str) -> Result<String, String> {
        let ar = match lang {
            "ar" => true,
            "en" => false,
            other => return Err(format!("lang must be 'ar' or 'en', got '{other}'")),
        };
        let t = |a: &'static str, e: &'static str| if ar { a } else { e };
        let mut o = String::new();
        let _ = writeln!(o, "# {} - {}\n", t("تقرير حسابي إنشائي", "Structural calculation report"), self.title);
        let status_text = match (self.status, ar) {
            ("complete", true) => "مكتمل",
            ("complete", false) => "complete",
            (_, true) => "غير مكتمل",
            (_, false) => "incomplete",
        };
        let _ = writeln!(o, "{}: **{status_text}**\n", t("الحالة", "Status"));
        let _ = writeln!(o, "## 1. {}\n", t("الفرضيات والمدخلات", "Assumptions and inputs"));
        let _ = writeln!(o, "| {} | {} | {} | {} |\n|---|---|---|---|", t("المدخل", "Input"), t("القيمة", "Value"), t("الوحدة", "Unit"), t("المصدر", "Source"));
        for x in &self.assumptions {
            let label = if ar { &x.label_ar } else { &x.label_en };
            let (val, src) = match &x.value {
                Some(v) => (v.as_str(), t("مدخل المستخدم", "user input")),
                None => (t("غير مُدخل", "not set"), "-"),
            };
            let _ = writeln!(o, "| {label} (`{}`) | {val} | {} | {src} |", x.key, x.unit);
        }
        let _ = writeln!(o, "\n## 2. {}\n", t("النتائج", "Results"));
        if self.results.is_empty() {
            let _ = writeln!(o, "{}\n", t("لا نتائج: لا عناصر إنشائية في النموذج.", "No results: the model holds no structural elements."));
        } else {
            let _ = writeln!(o, "| {} | {} | {} | {} |\n|---|---|---|---|", t("القسم", "Section"), t("العنصر", "Element"), t("التفصيل", "Detail"), t("الحالة", "Status"));
            for r in &self.results {
                let status = if r.status.is_empty() { "-" } else { r.status.as_str() };
                let _ = writeln!(o, "| {} | {} | {} | {status} |", r.section, r.element, r.detail.replace('|', "/"));
            }
        }
        let _ = writeln!(
            o,
            "\n{}: {} m3 / {} kg / {} {} / {} {} / {} {}",
            t("الإجمالي", "Totals"),
            self.totals.concrete_m3,
            self.totals.steel_kg,
            self.totals.columns,
            t("أعمدة", "columns"),
            self.totals.footings,
            t("أساسات", "footings"),
            self.totals.tie_beams,
            t("كمرات ربط", "tie beams")
        );
        let _ = writeln!(o, "\n## 3. {}\n", t("ما لم يُحسب", "Not computed"));
        if self.missing_assumptions.is_empty() && self.not_computed.is_empty() && self.failed.is_empty() && self.warnings.is_empty() {
            let _ = writeln!(o, "{}", t("لا شيء.", "Nothing."));
        }
        if !self.missing_assumptions.is_empty() {
            let _ = writeln!(o, "- {}: {}", t("مدخلات غير مُدخلة", "Inputs not set"), self.missing_assumptions.join(", "));
        }
        for (section, names) in &self.not_computed {
            let _ = writeln!(o, "- {section}: {}", names.join(", "));
        }
        if !self.failed.is_empty() {
            let _ = writeln!(o, "- {}: {}", t("عناصر لم تجتز الفحص", "Elements that failed a check"), self.failed.join(", "));
        }
        for w in &self.warnings {
            let _ = writeln!(o, "- {}: {w}", t("تنبيه", "Warning"));
        }
        let _ = writeln!(o, "\n## 4. {}\n", t("حدود التقرير", "Limits of this report"));
        for l in if ar { &self.limitations_ar } else { &self.limitations_en } {
            let _ = writeln!(o, "- {l}");
        }
        Ok(o)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sizing::{FootingRow, TieBeamIn};

    fn report_with_footing(status: &str) -> SizingReport {
        let mut r = SizingReport::default();
        r.footings.push(FootingRow {
            name: "FND-1".into(),
            id: "7".into(),
            column: "COL-1".into(),
            provided_mm: "1200x1200".into(),
            load_kn: 500.0,
            bearing_kpa: 150.0,
            required_m2: Some(3.33),
            required_side_mm: Some(1850),
            status: status.into(),
        });
        r.concrete_total_m3 = 2.5;
        r.steel_total_kg = 120.0;
        r
    }

    #[test]
    fn unset_inputs_are_listed_by_name_and_never_filled() {
        let c = build("Villa", &StructIn::default(), &SizingInputs::default(), &SizingReport::default());
        assert_eq!(c.status, "incomplete");
        assert!(c.missing_assumptions.contains(&"structure.bearing_kpa".to_string()));
        assert!(c.assumptions.iter().all(|a| a.value.is_none()));
        assert!(c.markdown("en").unwrap().contains("not set"));
    }

    #[test]
    fn entered_values_are_echoed_and_floor_load_is_the_sum() {
        let s = StructIn { dead_kpa: Some(5.0), live_kpa: Some(2.0), bearing_kpa: Some(150.0), floors: Some(2), max_span_mm: Some(6000), show_structure: false };
        let inp = SizingInputs { footing_thickness_mm: Some(500.0), tie_beam: Some(TieBeamIn { width_mm: Some(300.0), span_depth_ratio: Some(12.0) }), ..Default::default() };
        let c = build("Villa", &s, &inp, &report_with_footing("ok"));
        let get = |k: &str| c.assumptions.iter().find(|a| a.key == k).unwrap().value.clone();
        assert_eq!(get("sizing.floor_service_kpa").as_deref(), Some("7"));
        assert_eq!(get("structure.bearing_kpa").as_deref(), Some("150"));
        assert_eq!(get("sizing.tie_beam.width_mm").as_deref(), Some("300"));
        assert_eq!(c.totals.concrete_m3, 2.5);
        assert_eq!(c.results[0].element, "FND-1");
    }

    #[test]
    fn a_failed_row_blocks_complete_and_is_named() {
        let c = build("Villa", &StructIn::default(), &SizingInputs::default(), &report_with_footing("undersized"));
        assert_eq!(c.failed, vec!["FND-1 (undersized)".to_string()]);
        let md = c.markdown("ar").unwrap();
        assert!(md.contains("FND-1 (undersized)"));
        assert!(md.contains("غير مكتمل"));
    }

    #[test]
    fn sections_come_in_order_assumptions_then_results_and_bad_lang_is_rejected() {
        let c = build("V", &StructIn::default(), &SizingInputs::default(), &SizingReport::default());
        let md = c.markdown("en").unwrap();
        let (a, r, n) = (md.find("## 1.").unwrap(), md.find("## 2.").unwrap(), md.find("## 3.").unwrap());
        assert!(a < r && r < n);
        assert!(c.markdown("fr").unwrap_err().contains("lang"));
    }
}
