//! Project quality report: one declared score, with the counted sets behind it, before a project is handed over.
//!
//! The report reads what the engine already computes (the integrity check, the rule check and the number audit) and adds
//! nothing of its own to the engineering. No number here is a code value and no pass mark is invented: every criterion is a
//! plain fraction of a counted set, and the set is returned with it (`counted` of `total`) so the user sees what the
//! percentage stands for.
//!
//! Criteria (each 0-100, or `None` when there is nothing to count; `None` is left out of the overall score, never defaulted):
//! - **integrity**: share of model elements that no integrity issue (error or warning) points at.
//! - **rules**: share of the rule checks that ran and produced no finding.
//! - **coverage**: share of the rule checks that could run at all. A rule with an unset parameter is skipped, and a skipped
//!   check is not a pass, so it lowers coverage instead of raising the rules score.
//! - **provenance**: share of the project's numbers that carry a source.
//!
//! The overall score is the mean of the measured criteria weighted by [`QualityWeights`], which the user can set; the
//! weights used are returned with the result. The verdict does not depend on any score threshold:
//! - `blocked`: an integrity error or a rule error exists. The score is still shown, but the project is not ready.
//! - `review`: no error, but warnings, skipped rules, unsourced numbers or an unmeasured criterion remain.
//! - `clean`: every criterion was measured and nothing is left to review.
//!
//! The result is a preliminary self-check. It does not replace a licensed engineer or the approving authority.

use crate::model::{Id, Model};
use crate::numaudit::AuditFile;
use crate::precheck::{self, Level};
use crate::rules::{self, Severity};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Relative importance of each criterion in the overall score. Zero removes a criterion from the ranking.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct QualityWeights {
    pub integrity: f64,
    pub rules: f64,
    pub coverage: f64,
    pub provenance: f64,
}

impl Default for QualityWeights {
    /// Equal weight: the user states a preference by changing it.
    fn default() -> Self {
        Self { integrity: 1.0, rules: 1.0, coverage: 1.0, provenance: 1.0 }
    }
}

impl QualityWeights {
    pub fn validate(&self) -> Result<(), String> {
        let all = [("integrity", self.integrity), ("rules", self.rules), ("coverage", self.coverage), ("provenance", self.provenance)];
        for (name, v) in all {
            if !v.is_finite() || v < 0.0 {
                return Err(format!("quality weight '{name}' must be zero or more, not {v}"));
            }
        }
        if all.iter().map(|(_, v)| v).sum::<f64>() <= 0.0 {
            return Err("at least one quality weight must be above zero".into());
        }
        Ok(())
    }
}

/// One criterion: its score and the counted set it is a fraction of.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Criterion {
    pub id: &'static str,
    pub title_en: &'static str,
    pub title_ar: &'static str,
    /// 0-100, or `None` when nothing could be counted.
    pub score: Option<f64>,
    pub counted: usize,
    pub total: usize,
    pub weight: f64,
    /// What the counted set is, or why the criterion could not be measured.
    pub basis_en: String,
    pub basis_ar: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Blocked,
    Review,
    Clean,
}

/// What stands between the project and a clean handover, most serious first.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Item {
    pub severity: &'static str,
    pub source: &'static str,
    pub message_en: String,
    pub message_ar: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct QualityReport {
    pub overall: Option<f64>,
    pub verdict: Verdict,
    pub criteria: Vec<Criterion>,
    pub weights: QualityWeights,
    pub items: Vec<Item>,
    /// Counts the verdict was drawn from.
    pub integrity_errors: usize,
    pub integrity_warnings: usize,
    pub rule_errors: usize,
    pub rule_warnings: usize,
    pub rules_skipped: usize,
    pub numbers_unsourced: usize,
    pub numbers_undated: usize,
    pub notes_en: Vec<String>,
    pub notes_ar: Vec<String>,
}

fn pct(num: usize, den: usize) -> Option<f64> {
    (den > 0).then(|| 100.0 * num as f64 / den as f64)
}

/// Builds the report. `rules` and `audit` are `None` when that check was not run: the criterion is then unmeasured and named
/// as such, never scored as if it had passed.
pub fn build(model: &Model, integrity: &precheck::Report, rules: Option<&rules::Report>, audit: Option<&AuditFile>, w: &QualityWeights) -> Result<QualityReport, String> {
    w.validate()?;
    let mut items: Vec<Item> = Vec::new();
    let mut notes_en = Vec::new();
    let mut notes_ar = Vec::new();

    // Integrity.
    let integrity_errors = integrity.issues.iter().filter(|i| i.level == Level::Error).count();
    let integrity_warnings = integrity.issues.iter().filter(|i| i.level == Level::Warning).count();
    let total_el = model.elements.len();
    let touched: BTreeSet<Id> = integrity.issues.iter().flat_map(|i| i.elements.iter().copied()).filter(|id| model.elements.contains_key(id)).collect();
    let integrity_c = Criterion {
        id: "integrity",
        title_en: "Model integrity",
        title_ar: "سلامة النموذج",
        score: pct(total_el.saturating_sub(touched.len()), total_el),
        counted: total_el.saturating_sub(touched.len()),
        total: total_el,
        weight: w.integrity,
        basis_en: if total_el == 0 {
            "the model has no elements to check".into()
        } else {
            format!("{} of {} elements are not named by any integrity issue", total_el - touched.len(), total_el)
        },
        basis_ar: if total_el == 0 {
            "لا عناصر في النموذج لفحصها".into()
        } else {
            format!("{} من {} عنصراً لا يذكره أي إشكال في سلامة النموذج", total_el - touched.len(), total_el)
        },
    };
    for i in &integrity.issues {
        let sev = if i.level == Level::Error { "error" } else { "warning" };
        items.push(Item { severity: sev, source: "integrity", message_en: format!("[{}] {}", i.code, i.message), message_ar: format!("[{}] {}", i.code, i.message) });
    }

    // Rules and coverage.
    let (mut rule_errors, mut rule_warnings, mut rules_skipped) = (0, 0, 0);
    let (rules_c, coverage_c) = match rules {
        Some(r) => {
            rule_errors = r.findings.iter().filter(|f| f.severity == Severity::Error).count();
            rule_warnings = r.findings.iter().filter(|f| f.severity == Severity::Warning).count();
            rules_skipped = r.skipped.len();
            let failed = r.findings.len().min(r.checked);
            let not_run: usize = r.skipped.iter().map(|s| s.elements.len()).sum();
            for f in &r.findings {
                let sev = match f.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                    Severity::Info => "info",
                };
                items.push(Item { severity: sev, source: "rules", message_en: format!("{} ({}): {}", f.rule_id, f.clause, f.message), message_ar: format!("{} ({}): {}", f.rule_id, f.clause, f.message) });
            }
            for s in &r.skipped {
                items.push(Item {
                    severity: "skipped",
                    source: "rules",
                    message_en: format!("rule {} did not run on {} element(s): {}", s.rule_id, s.elements.len(), s.reason),
                    message_ar: format!("لم تعمل القاعدة {} على {} عنصراً: المعامل «{}» غير مُدخل", s.rule_id, s.elements.len(), s.param),
                });
            }
            (
                Criterion {
                    id: "rules",
                    title_en: "Rule check",
                    title_ar: "فحص القواعد",
                    score: pct(r.checked - failed, r.checked),
                    counted: r.checked - failed,
                    total: r.checked,
                    weight: w.rules,
                    basis_en: if r.checked == 0 { "no rule check ran".into() } else { format!("{} of {} rule checks produced no finding", r.checked - failed, r.checked) },
                    basis_ar: if r.checked == 0 { "لم يعمل أي فحص للقواعد".into() } else { format!("{} من {} فحصاً للقواعد لم ينتج عنها أي ملاحظة", r.checked - failed, r.checked) },
                },
                Criterion {
                    id: "coverage",
                    title_en: "Rule coverage",
                    title_ar: "تغطية القواعد",
                    score: pct(r.checked, r.checked + not_run),
                    counted: r.checked,
                    total: r.checked + not_run,
                    weight: w.coverage,
                    basis_en: if r.checked + not_run == 0 { "no rule applies to any element".into() } else { format!("{} of {} rule checks could run; the rest wait for an unset limit", r.checked, r.checked + not_run) },
                    basis_ar: if r.checked + not_run == 0 { "لا قاعدة تنطبق على أي عنصر".into() } else { format!("{} من {} فحصاً أمكن تشغيله؛ والباقي ينتظر حدّاً غير مُدخل", r.checked, r.checked + not_run) },
                },
            )
        }
        None => {
            notes_en.push("The rule check was not run: rules and coverage are unmeasured.".to_string());
            notes_ar.push("لم يُشغَّل فحص القواعد: معيارا القواعد والتغطية غير مقيسين.".to_string());
            let unmeasured = |id: &'static str, en: &'static str, ar: &'static str, weight: f64| Criterion {
                id,
                title_en: en,
                title_ar: ar,
                score: None,
                counted: 0,
                total: 0,
                weight,
                basis_en: "the rule check was not run".into(),
                basis_ar: "لم يُشغَّل فحص القواعد".into(),
            };
            (unmeasured("rules", "Rule check", "فحص القواعد", w.rules), unmeasured("coverage", "Rule coverage", "تغطية القواعد", w.coverage))
        }
    };

    // Provenance.
    let (mut unsourced, mut undated) = (0, 0);
    let provenance_c = match audit {
        Some(a) => {
            unsourced = a.unsourced;
            undated = a.undated;
            if a.unsourced > 0 {
                let shown: Vec<&str> = a.unsourced_paths.iter().take(10).map(String::as_str).collect();
                let more = a.unsourced.saturating_sub(shown.len());
                let tail = if more > 0 { format!(" (+{more} more)") } else { String::new() };
                items.push(Item {
                    severity: "warning",
                    source: "provenance",
                    message_en: format!("{} number(s) carry no source: {}{}", a.unsourced, shown.join(", "), tail),
                    message_ar: format!("{} رقماً بلا مصدر: {}{}", a.unsourced, shown.join(", "), tail),
                });
            }
            if a.undated > 0 {
                items.push(Item {
                    severity: "info",
                    source: "provenance",
                    message_en: format!("{} sourced number(s) carry no date", a.undated),
                    message_ar: format!("{} رقماً له مصدر بلا تاريخ", a.undated),
                });
            }
            Criterion {
                id: "provenance",
                title_en: "Number provenance",
                title_ar: "مصادر الأرقام",
                score: pct(a.sourced, a.total),
                counted: a.sourced,
                total: a.total,
                weight: w.provenance,
                basis_en: if a.total == 0 { "the project holds no numbers to audit".into() } else { format!("{} of {} numbers carry a source", a.sourced, a.total) },
                basis_ar: if a.total == 0 { "لا أرقام في المشروع لتدقيقها".into() } else { format!("{} من {} رقماً له مصدر", a.sourced, a.total) },
            }
        }
        None => {
            notes_en.push("The number audit was not run: provenance is unmeasured.".to_string());
            notes_ar.push("لم يُشغَّل تدقيق الأرقام: معيار المصادر غير مقيس.".to_string());
            Criterion {
                id: "provenance",
                title_en: "Number provenance",
                title_ar: "مصادر الأرقام",
                score: None,
                counted: 0,
                total: 0,
                weight: w.provenance,
                basis_en: "the number audit was not run".into(),
                basis_ar: "لم يُشغَّل تدقيق الأرقام".into(),
            }
        }
    };

    let criteria = vec![integrity_c, rules_c, coverage_c, provenance_c];
    let weight_sum: f64 = criteria.iter().filter(|c| c.score.is_some()).map(|c| c.weight).sum();
    let overall = (weight_sum > 0.0).then(|| criteria.iter().filter_map(|c| c.score.map(|s| s * c.weight)).sum::<f64>() / weight_sum);
    for c in criteria.iter().filter(|c| c.score.is_none()) {
        notes_en.push(format!("{}: not measured ({}).", c.title_en, c.basis_en));
        notes_ar.push(format!("{}: غير مقيس ({}).", c.title_ar, c.basis_ar));
    }
    notes_en.push("A preliminary self-check; it does not replace a licensed engineer or the approving authority.".into());
    notes_ar.push("فحص ذاتي أولي لا يغني عن مهندس مرخّص ولا عن الجهة المعتمدة.".into());

    // Most serious first; the sort is stable so each source keeps its own order.
    let rank = |s: &str| match s {
        "error" => 0,
        "warning" => 1,
        "skipped" => 2,
        _ => 3,
    };
    items.sort_by_key(|i| rank(i.severity));

    let verdict = if integrity_errors > 0 || rule_errors > 0 {
        Verdict::Blocked
    } else if integrity_warnings > 0 || rule_warnings > 0 || rules_skipped > 0 || unsourced > 0 || criteria.iter().any(|c| c.score.is_none()) {
        Verdict::Review
    } else {
        Verdict::Clean
    };
    Ok(QualityReport {
        overall,
        verdict,
        criteria,
        weights: *w,
        items,
        integrity_errors,
        integrity_warnings,
        rule_errors,
        rule_warnings,
        rules_skipped,
        numbers_unsourced: unsourced,
        numbers_undated: undated,
        notes_en,
        notes_ar,
    })
}

impl QualityReport {
    pub fn csv(&self) -> String {
        let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        let mut out = String::from("criterion,score,counted,total,weight,basis\n");
        for c in &self.criteria {
            let score = c.score.map(|s| format!("{s:.1}")).unwrap_or_default();
            out.push_str(&format!("{},{},{},{},{},{}\n", c.id, score, c.counted, c.total, c.weight, q(&c.basis_en)));
        }
        out.push_str(&format!("overall,{},,,,{}\n", self.overall.map(|s| format!("{s:.1}")).unwrap_or_default(), q(&format!("{:?}", self.verdict).to_lowercase())));
        out.push_str("\nseverity,source,message\n");
        for i in &self.items {
            out.push_str(&format!("{},{},{}\n", i.severity, i.source, q(&i.message_en)));
        }
        out
    }

    pub fn html(&self) -> String {
        let e = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let fmt = |s: Option<f64>| s.map(|v| format!("{v:.1}")).unwrap_or_else(|| "—".into());
        let mut h = String::from("<!doctype html><html lang=\"en\" dir=\"ltr\"><meta charset=\"utf-8\"><title>Project quality report</title><body>");
        h.push_str(&format!("<h1>Project quality report / تقرير جودة المشروع</h1><h2>{} / 100 — {}</h2>", fmt(self.overall), e(&format!("{:?}", self.verdict).to_lowercase())));
        h.push_str("<table border=\"1\" cellpadding=\"4\"><tr><th>Criterion</th><th>Score</th><th>Counted</th><th>Weight</th><th>Basis</th></tr>");
        for c in &self.criteria {
            h.push_str(&format!("<tr><td>{} / {}</td><td>{}</td><td>{} / {}</td><td>{}</td><td>{}<br>{}</td></tr>", e(c.title_en), e(c.title_ar), fmt(c.score), c.counted, c.total, c.weight, e(&c.basis_en), e(&c.basis_ar)));
        }
        h.push_str("</table><h3>Items</h3><ul>");
        for i in &self.items {
            h.push_str(&format!("<li><b>{}</b> [{}] {}<br>{}</li>", i.severity, i.source, e(&i.message_en), e(&i.message_ar)));
        }
        h.push_str("</ul>");
        for (n, a) in self.notes_en.iter().zip(&self.notes_ar) {
            h.push_str(&format!("<p>{}<br>{}</p>", e(n), e(a)));
        }
        h.push_str("</body></html>");
        h
    }
}
