//! Geotechnical report data: the figures the foundation design depends on, exactly as the soil investigation report states them.
//!
//! Stored as `meta.supervision.geotech`. Nothing is defaulted: the allowable bearing pressure, the groundwater depth and the soil
//! layers are the report's numbers entered by the user with the report reference. The structural sizing reads its own
//! `structure.bearing_kpa`; this module only reports when the two disagree, it never overwrites either.

use crate::execution::{blank, esc, parse_date};
use crate::xlsx::{Cell, Sheet, Workbook, style};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Foundation {
    Isolated,
    Strip,
    Raft,
    Piles,
}

impl Foundation {
    fn label(self, ar: bool) -> &'static str {
        match (self, ar) {
            (Foundation::Isolated, false) => "Isolated footings",
            (Foundation::Strip, false) => "Strip footings",
            (Foundation::Raft, false) => "Raft",
            (Foundation::Piles, false) => "Piles",
            (Foundation::Isolated, true) => "قواعد منفصلة",
            (Foundation::Strip, true) => "قواعد شريطية",
            (Foundation::Raft, true) => "لبشة",
            (Foundation::Piles, true) => "خوازيق",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SoilLayer {
    pub from_m: f64,
    pub to_m: f64,
    pub description: String,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GeotechInputs {
    /// The report's reference number and date. Required with any figure.
    pub report_ref: Option<String>,
    pub report_date: Option<String>,
    pub firm: Option<String>,
    /// Allowable bearing pressure q_all in kPa and the depth below ground it applies to.
    pub bearing_kpa: Option<f64>,
    pub bearing_depth_m: Option<f64>,
    pub groundwater_depth_m: Option<f64>,
    pub layers: Vec<SoilLayer>,
    pub recommended: Vec<Foundation>,
    pub notes: Option<String>,
}

impl GeotechInputs {
    pub fn is_empty(&self) -> bool {
        *self == GeotechInputs::default()
    }

    pub fn check(&self) -> Result<(), String> {
        if !self.is_empty() && (blank(&self.report_ref) || blank(&self.report_date)) {
            return Err("geotechnical report: enter the report reference and date with its figures".into());
        }
        if let Some(d) = &self.report_date {
            parse_date("geotechnical report date", d)?;
        }
        for (what, v) in [("allowable bearing pressure", self.bearing_kpa), ("bearing depth", self.bearing_depth_m), ("groundwater depth", self.groundwater_depth_m)] {
            if let Some(x) = v
                && (!x.is_finite() || x < 0.0 || (what == "allowable bearing pressure" && x == 0.0))
            {
                return Err(format!("geotechnical report: {what} must be a positive number"));
            }
        }
        let mut last = 0.0;
        for (i, l) in self.layers.iter().enumerate() {
            if !(l.from_m.is_finite() && l.to_m.is_finite()) || l.from_m < 0.0 || l.to_m <= l.from_m {
                return Err(format!("soil layer {}: the depths must satisfy 0 <= from < to", i + 1));
            }
            if l.from_m < last {
                return Err(format!("soil layer {}: overlaps the layer above it (layers must be listed top to bottom)", i + 1));
            }
            if l.description.trim().is_empty() {
                return Err(format!("soil layer {}: enter the soil description", i + 1));
            }
            last = l.to_m;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GeotechReport {
    pub rows: Vec<(String, String)>,
    pub layers: Vec<Vec<String>>,
    pub issues: Vec<String>,
    /// The report's bearing pressure differs from the one the structural sizing uses.
    pub bearing_conflict: bool,
}

fn f(v: Option<f64>) -> String {
    v.map_or("—".into(), |x| format!("{x}"))
}

/// `structural_bearing_kpa` is `structure.bearing_kpa` of the project.
pub fn report(inp: &GeotechInputs, structural_bearing_kpa: Option<f64>, arabic: bool) -> GeotechReport {
    let t = |ar: &'static str, en: &'static str| if arabic { ar.to_string() } else { en.to_string() };
    let mut issues = Vec::new();
    for (ok, ar, en) in [
        (inp.bearing_kpa.is_some(), "ضغط التحميل المسموح (q_all) غير مدخل: لا تُصمَّم القواعد", "allowable bearing pressure (q_all) not entered: footings cannot be sized"),
        (inp.groundwater_depth_m.is_some(), "عمق الماء الجوفي غير مدخل", "groundwater depth not entered"),
        (!inp.layers.is_empty(), "طبقات التربة غير مدخلة", "soil layers not entered"),
        (!inp.recommended.is_empty(), "نوع الأساس الموصى به في التقرير غير مدخل", "the foundation type the report recommends is not entered"),
    ] {
        if !ok {
            issues.push(t(ar, en));
        }
    }
    for w in inp.layers.windows(2) {
        if w[1].from_m > w[0].to_m {
            issues.push(if arabic { format!("فجوة بين العمقين {} و{} م في طبقات التربة", w[0].to_m, w[1].from_m) } else { format!("gap between {} m and {} m in the soil layers", w[0].to_m, w[1].from_m) });
        }
    }
    let conflict = matches!((inp.bearing_kpa, structural_bearing_kpa), (Some(a), Some(b)) if (a - b).abs() > 1e-9);
    if conflict {
        issues.push(if arabic {
            format!("تعارض: تقرير التربة {} كيلوباسكال والتصميم الإنشائي يستعمل {} كيلوباسكال", f(inp.bearing_kpa), f(structural_bearing_kpa))
        } else {
            format!("conflict: the soil report gives {} kPa but the structural sizing uses {} kPa", f(inp.bearing_kpa), f(structural_bearing_kpa))
        });
    }
    if inp.bearing_kpa.is_some() && structural_bearing_kpa.is_none() {
        issues.push(t("الضغط المسموح مدخل هنا ولم يُدخل في المدخلات الإنشائية: القواعد غير مصمَّمة", "the bearing pressure is entered here but not in the structural inputs: footings are not sized"));
    }
    let rows = vec![
        (t("مرجع التقرير", "Report reference"), format!("{} / {}", inp.report_ref.as_deref().unwrap_or("—"), inp.report_date.as_deref().unwrap_or("—"))),
        (t("الجهة المعدّة", "Issued by"), inp.firm.clone().unwrap_or_else(|| "—".into())),
        (t("الضغط المسموح q_all (كيلوباسكال)", "Allowable bearing q_all (kPa)"), f(inp.bearing_kpa)),
        (t("عمق ينطبق عنده (م)", "Applies at depth (m)"), f(inp.bearing_depth_m)),
        (t("عمق الماء الجوفي (م)", "Groundwater depth (m)"), f(inp.groundwater_depth_m)),
        (t("الأساس الموصى به", "Recommended foundation"), if inp.recommended.is_empty() { "—".into() } else { inp.recommended.iter().map(|x| x.label(arabic)).collect::<Vec<_>>().join(", ") }),
    ];
    let layers = inp.layers.iter().map(|l| vec![format!("{} – {}", l.from_m, l.to_m), l.description.clone(), l.notes.clone().unwrap_or_default()]).collect();
    GeotechReport { rows, layers, issues, bearing_conflict: conflict }
}

impl GeotechReport {
    pub fn html(&self, title: &str, arabic: bool) -> String {
        let t = |ar: &'static str, en: &'static str| if arabic { ar } else { en };
        let mut h = format!(
            "<!doctype html><html lang=\"{}\" dir=\"{}\"><head><meta charset=\"utf-8\"><title>{}</title><style>body{{font-family:'Segoe UI',Tahoma,sans-serif;margin:28px}} table{{border-collapse:collapse;width:100%;font-size:12px;margin-bottom:14px}} th,td{{border:1px solid #999;padding:4px 6px;text-align:start}} th{{background:#eee}} .warn{{color:#a33}}</style></head><body><h1>{} — {}</h1><table>",
            if arabic { "ar" } else { "en" },
            if arabic { "rtl" } else { "ltr" },
            esc(title),
            t("بيانات التربة (الجيوتقني)", "Geotechnical data"),
            esc(title)
        );
        for (k, v) in &self.rows {
            h.push_str(&format!("<tr><th>{}</th><td>{}</td></tr>", esc(k), esc(v)));
        }
        h.push_str(&format!("</table><table><tr><th>{}</th><th>{}</th><th>{}</th></tr>", t("العمق (م)", "Depth (m)"), t("التربة", "Soil"), t("ملاحظات", "Notes")));
        for l in &self.layers {
            h.push_str(&format!("<tr><td>{}</td><td>{}</td><td>{}</td></tr>", esc(&l[0]), esc(&l[1]), esc(&l[2])));
        }
        h.push_str("</table>");
        if !self.issues.is_empty() {
            h.push_str("<ul class=\"warn\">");
            for i in &self.issues {
                h.push_str(&format!("<li>{}</li>", esc(i)));
            }
            h.push_str("</ul>");
        }
        h.push_str("</body></html>");
        h
    }

    pub fn workbook(&self, arabic: bool) -> Workbook {
        let t = |ar: &'static str, en: &'static str| if arabic { ar } else { en };
        let text = |s: &str, st: u8| (Cell::Text(s.to_string()), st);
        let mut rows = vec![vec![text(t("البند", "Item"), style::HEADER), text(t("القيمة", "Value"), style::HEADER)]];
        for (k, v) in &self.rows {
            rows.push(vec![text(k, style::TEXT), text(v, style::TEXT)]);
        }
        let mut layer_rows = vec![vec![text(t("العمق (م)", "Depth (m)"), style::HEADER), text(t("التربة", "Soil"), style::HEADER), text(t("ملاحظات", "Notes"), style::HEADER)]];
        for l in &self.layers {
            layer_rows.push(l.iter().map(|c| text(c, style::TEXT)).collect());
        }
        Workbook {
            sheets: vec![
                Sheet { name: t("التقرير", "Report").into(), rows, widths: vec![40.0, 40.0], rtl: arabic, header: true, filter_to: 0 },
                Sheet { name: t("الطبقات", "Layers").into(), rows: layer_rows, widths: vec![16.0, 50.0, 40.0], rtl: arabic, header: true, filter_to: 0 },
            ],
        }
    }
}
