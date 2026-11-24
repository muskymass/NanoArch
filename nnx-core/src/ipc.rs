//! Interim payment certificates (IPC, «المستخلصات») measured on the BOQ quantities.
//!
//! Each certificate states, CUMULATIVELY, the quantity completed of every BOQ item (an item a certificate does not mention keeps the
//! quantity of the previous certificate) and the percentage completed of every approved variation order. The value of a line is the
//! cumulative quantity times the BOQ rate; the amount of the period is the cumulative value less what the previous certificates had
//! already certified. Retention is a percentage of the cumulative value and the VAT percentage applies to the net of the period. Both
//! percentages, the deductions (advance recovery, penalties) and the currency are the user's: none is defaulted, and without the
//! retention percentage no net is computed. An item with no BOQ rate is listed and makes the certificate incomplete; its value is
//! never guessed.

use crate::boq::BoqReport;
use crate::execution::{blank, esc, parse_date, round};
use crate::xlsx::{Cell, Sheet, Workbook, style};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One BOQ item as a contract line.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContractLine {
    pub code: String,
    pub description: String,
    pub unit: String,
    pub quantity: f64,
    pub rate: Option<f64>,
}

/// The measured items of a BOQ report (reference-only rows are not payable).
pub fn lines_from_boq(r: &BoqReport) -> Vec<ContractLine> {
    r.rows.iter().filter(|x| !x.reference_only).map(|x| ContractLine { code: x.code.clone(), description: x.description.clone(), unit: x.unit.clone(), quantity: x.quantity, rate: x.rate }).collect()
}

/// An approved variation order that can be certified: its id and signed cost.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ApprovedVo {
    pub id: String,
    pub cost: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QtyLine {
    pub code: String,
    pub cum_qty: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VoLine {
    pub id: String,
    pub cum_pct: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Deduction {
    pub label: String,
    /// Amount deducted in THIS certificate.
    pub amount: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Certificate {
    pub id: String,
    pub date: String,
    pub lines: Vec<QtyLine>,
    pub variations: Vec<VoLine>,
    pub deductions: Vec<Deduction>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IpcInputs {
    pub currency: Option<String>,
    pub retention_pct: Option<f64>,
    pub vat_pct: Option<f64>,
    pub certificates: Vec<Certificate>,
}

impl IpcInputs {
    pub fn check(&self) -> Result<(), String> {
        for (what, v) in [("retention", self.retention_pct), ("VAT", self.vat_pct)] {
            if let Some(x) = v
                && !(0.0..=100.0).contains(&x)
            {
                return Err(format!("{what} percentage must be between 0 and 100"));
            }
        }
        let mut ids = BTreeSet::new();
        let mut last: Option<i64> = None;
        let mut qty: BTreeMap<&str, f64> = BTreeMap::new();
        let mut vo: BTreeMap<&str, f64> = BTreeMap::new();
        for c in &self.certificates {
            if blank(&Some(c.id.clone())) || !ids.insert(c.id.as_str()) {
                return Err(format!("certificate '{}': the id is empty or used twice", c.id));
            }
            let d = parse_date(&format!("certificate '{}'", c.id), &c.date)?.to_days();
            if last.is_some_and(|l| d < l) {
                return Err(format!("certificate '{}': dated before the previous certificate", c.id));
            }
            last = Some(d);
            let mut seen = BTreeSet::new();
            for l in &c.lines {
                if !seen.insert(l.code.as_str()) {
                    return Err(format!("certificate '{}': item '{}' appears twice", c.id, l.code));
                }
                if !l.cum_qty.is_finite() || l.cum_qty < 0.0 {
                    return Err(format!("certificate '{}': item '{}' has a negative or invalid quantity", c.id, l.code));
                }
                if qty.get(l.code.as_str()).is_some_and(|p| l.cum_qty < *p) {
                    return Err(format!("certificate '{}': item '{}' went down from the previous certificate (cumulative quantities cannot decrease)", c.id, l.code));
                }
                qty.insert(&l.code, l.cum_qty);
            }
            for v in &c.variations {
                if !(0.0..=100.0).contains(&v.cum_pct) {
                    return Err(format!("certificate '{}': variation '{}' must be between 0 and 100", c.id, v.id));
                }
                if vo.get(v.id.as_str()).is_some_and(|p| v.cum_pct < *p) {
                    return Err(format!("certificate '{}': variation '{}' went down from the previous certificate", c.id, v.id));
                }
                vo.insert(&v.id, v.cum_pct);
            }
            for x in &c.deductions {
                if !x.amount.is_finite() || x.label.trim().is_empty() {
                    return Err(format!("certificate '{}': a deduction needs a label and a valid amount", c.id));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LineResult {
    pub code: String,
    pub description: String,
    pub unit: String,
    pub contract_qty: f64,
    pub rate: Option<f64>,
    pub previous_qty: f64,
    pub cum_qty: f64,
    pub period_qty: f64,
    pub cum_value: Option<f64>,
    pub period_value: Option<f64>,
    pub exceeds_contract: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct VoResult {
    pub id: String,
    pub cost: f64,
    pub cum_pct: f64,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CertResult {
    pub id: String,
    pub date: String,
    pub lines: Vec<LineResult>,
    pub variations: Vec<VoResult>,
    pub cum_value: f64,
    pub previous_cum_value: f64,
    pub previous_retention: f64,
    pub period_value: f64,
    pub retention: Option<f64>,
    pub deductions: f64,
    pub net_before_vat: Option<f64>,
    pub vat: Option<f64>,
    pub payable: Option<f64>,
    /// Every item with work done has a rate.
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct IpcReport {
    pub currency: Option<String>,
    pub retention_pct: Option<f64>,
    pub vat_pct: Option<f64>,
    pub certificates: Vec<CertResult>,
    pub issues: Vec<String>,
}

pub fn compute(inp: &IpcInputs, boq: &[ContractLine], vos: &[ApprovedVo]) -> IpcReport {
    let mut issues = Vec::new();
    if inp.retention_pct.is_none() && !inp.certificates.is_empty() {
        issues.push("retention percentage not entered: the retention and net payable are not computed".to_string());
    }
    let by_code: BTreeMap<&str, &ContractLine> = boq.iter().map(|l| (l.code.as_str(), l)).collect();
    let mut cum_qty: BTreeMap<String, f64> = BTreeMap::new();
    let mut cum_vo: BTreeMap<String, f64> = BTreeMap::new();
    let (mut prev_value, mut prev_retention) = (0.0, 0.0);
    let mut out = Vec::new();
    for c in &inp.certificates {
        let prev_qty = cum_qty.clone();
        for l in &c.lines {
            if by_code.contains_key(l.code.as_str()) {
                cum_qty.insert(l.code.clone(), l.cum_qty);
            } else {
                issues.push(format!("certificate {}: '{}' is not an item of the BOQ and was not counted", c.id, l.code));
            }
        }
        let mut lines = Vec::new();
        let (mut cum_value, mut complete) = (0.0, true);
        for b in boq {
            let q = cum_qty.get(&b.code).copied().unwrap_or(0.0);
            let p = prev_qty.get(&b.code).copied().unwrap_or(0.0);
            if q == 0.0 && p == 0.0 {
                continue;
            }
            let cv = b.rate.map(|r| q * r);
            if let Some(v) = cv {
                cum_value += v;
            } else {
                complete = false;
                issues.push(format!("certificate {}: item '{}' has no rate in the BOQ and was not valued", c.id, b.code));
            }
            let exceeds = q > b.quantity;
            if exceeds {
                issues.push(format!("certificate {}: item '{}' is certified beyond the BOQ quantity ({} > {}): agree a variation or a remeasurement", c.id, b.code, round(q, 3), round(b.quantity, 3)));
            }
            lines.push(LineResult { code: b.code.clone(), description: b.description.clone(), unit: b.unit.clone(), contract_qty: b.quantity, rate: b.rate, previous_qty: p, cum_qty: q, period_qty: q - p, cum_value: cv, period_value: b.rate.map(|r| (q - p) * r), exceeds_contract: exceeds });
        }
        for v in &c.variations {
            cum_vo.insert(v.id.clone(), v.cum_pct);
        }
        let mut vo_rows = Vec::new();
        for (id, pct) in &cum_vo {
            match vos.iter().find(|v| v.id == *id) {
                Some(v) => {
                    let value = v.cost * pct / 100.0;
                    cum_value += value;
                    vo_rows.push(VoResult { id: id.clone(), cost: v.cost, cum_pct: *pct, value });
                }
                None => issues.push(format!("certificate {}: variation '{id}' is not approved (or does not exist) and was not counted", c.id)),
            }
        }
        let deductions: f64 = c.deductions.iter().map(|d| d.amount).sum();
        let retention = inp.retention_pct.map(|p| cum_value * p / 100.0);
        let net = retention.map(|r| (cum_value - r) - (prev_value - prev_retention) - deductions);
        let vat = net.zip(inp.vat_pct).map(|(n, p)| n * p / 100.0);
        let payable = net.map(|n| n + vat.unwrap_or(0.0));
        if net.is_some() && inp.vat_pct.is_none() {
            issues.push(format!("certificate {}: VAT percentage not entered: the payable excludes VAT", c.id));
        }
        out.push(CertResult { id: c.id.clone(), date: c.date.clone(), lines, variations: vo_rows, cum_value: round(cum_value, 2), previous_cum_value: round(prev_value, 2), previous_retention: round(prev_retention, 2), period_value: round(cum_value - prev_value, 2), retention: retention.map(|v| round(v, 2)), deductions: round(deductions, 2), net_before_vat: net.map(|v| round(v, 2)), vat: vat.map(|v| round(v, 2)), payable: payable.map(|v| round(v, 2)), complete });
        prev_value = cum_value;
        prev_retention = retention.unwrap_or(0.0);
    }
    IpcReport { currency: inp.currency.clone(), retention_pct: inp.retention_pct, vat_pct: inp.vat_pct, certificates: out, issues }
}

fn money(v: Option<f64>) -> String {
    v.map_or("—".into(), |x| format!("{x:.2}"))
}

impl IpcReport {
    pub fn html(&self, title: &str, arabic: bool) -> String {
        let t = |ar: &'static str, en: &'static str| if arabic { ar } else { en };
        let cur = self.currency.as_deref().unwrap_or("");
        let mut h = format!(
            "<!doctype html><html lang=\"{}\" dir=\"{}\"><head><meta charset=\"utf-8\"><title>{}</title><style>body{{font-family:'Segoe UI',Tahoma,sans-serif;margin:28px;color:#111}} h1{{font-size:21px}} h2{{font-size:15px;margin:22px 0 6px;border-bottom:2px solid #333}}\
             table{{border-collapse:collapse;width:100%;font-size:11.5px}} th,td{{border:1px solid #999;padding:4px 6px;text-align:start}} th{{background:#eee}} .tot td{{font-weight:700;background:#f5f5f5}} .warn{{color:#a33}} @page{{size:A4 landscape;margin:12mm}}</style></head><body>",
            if arabic { "ar" } else { "en" },
            if arabic { "rtl" } else { "ltr" },
            esc(title)
        );
        h.push_str(&format!("<h1>{} — {}</h1>", t("المستخلصات المالية", "Interim payment certificates"), esc(title)));
        for c in &self.certificates {
            h.push_str(&format!("<h2>{} {} · {}</h2><table><thead><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr></thead><tbody>", t("مستخلص", "Certificate"), esc(&c.id), esc(&c.date), t("البند", "Item"), t("الوصف", "Description"), t("الوحدة", "Unit"), t("سعر الوحدة", "Rate"), t("السابق", "Previous"), t("التراكمي", "Cumulative"), t("هذه الفترة", "This period"), t("قيمة الفترة", "Period value")));
            for l in &c.lines {
                h.push_str(&format!("<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>", esc(&l.code), esc(&l.description), esc(&l.unit), money(l.rate), round(l.previous_qty, 3), round(l.cum_qty, 3), round(l.period_qty, 3), money(l.period_value)));
            }
            for v in &c.variations {
                h.push_str(&format!("<tr><td>{}</td><td>{}</td><td>%</td><td></td><td></td><td>{}</td><td></td><td>{:.2}</td></tr>", esc(&v.id), t("أمر تغيير", "Variation order"), v.cum_pct, v.value));
            }
            let row = |k: &str, v: String| format!("<tr class=\"tot\"><td colspan=\"7\">{k}</td><td>{v}</td></tr>");
            h.push_str(&row(t("القيمة التراكمية", "Cumulative value"), format!("{:.2}", c.cum_value)));
            h.push_str(&row(t("المعتمد سابقاً", "Previously certified"), format!("{:.2}", c.previous_cum_value)));
            h.push_str(&row(&format!("{} {}%", t("الاستقطاع (ضمان حسن التنفيذ)", "Retention"), self.retention_pct.map_or("—".into(), |p| p.to_string())), money(c.retention)));
            h.push_str(&row(t("الخصومات", "Deductions"), format!("{:.2}", c.deductions)));
            h.push_str(&row(t("الصافي قبل الضريبة", "Net before VAT"), money(c.net_before_vat)));
            h.push_str(&row(&format!("{} {}%", t("ضريبة القيمة المضافة", "VAT"), self.vat_pct.map_or("—".into(), |p| p.to_string())), money(c.vat)));
            h.push_str(&row(&format!("{} {cur}", t("المستحق الدفع", "Net payable")), money(c.payable)));
            h.push_str("</tbody></table>");
            if !c.complete {
                h.push_str(&format!("<p class=\"warn\">{}</p>", t("المستخلص غير مكتمل: بنود بلا سعر لم تُقيَّم.", "Incomplete: items with no rate were not valued.")));
            }
        }
        if !self.issues.is_empty() {
            h.push_str("<h2>!</h2><ul class=\"warn\">");
            for i in &self.issues {
                h.push_str(&format!("<li>{}</li>", esc(i)));
            }
            h.push_str("</ul>");
        }
        h.push_str("</body></html>");
        h
    }

    /// One sheet per certificate with live formulas (line value = quantity x rate, totals = SUM, retention, net and VAT from the
    /// percentages in the sheet), so the contractor can audit every figure.
    pub fn workbook(&self, arabic: bool) -> Workbook {
        let t = |ar: &'static str, en: &'static str| if arabic { ar } else { en };
        let text = |s: &str, st: u8| (Cell::Text(s.to_string()), st);
        let mut sheets = Vec::new();
        for c in &self.certificates {
            let head: Vec<(Cell, u8)> = [t("البند", "Item"), t("الوصف", "Description"), t("الوحدة", "Unit"), t("كمية العقد", "Contract qty"), t("السعر", "Rate"), t("السابق", "Previous qty"), t("التراكمي", "Cumulative qty"), t("هذه الفترة", "This period qty"), t("قيمة الفترة", "Period value"), t("القيمة التراكمية", "Cumulative value")].iter().map(|l| text(l, style::HEADER)).collect();
            let mut rows = vec![head];
            for l in &c.lines {
                let n = rows.len() + 1;
                rows.push(vec![
                    text(&l.code, style::TEXT),
                    text(&l.description, style::TEXT),
                    text(&l.unit, style::TEXT),
                    (Cell::Num(l.contract_qty), style::QTY),
                    l.rate.map_or((Cell::Empty, style::MONEY), |r| (Cell::Num(r), style::MONEY)),
                    (Cell::Num(l.previous_qty), style::QTY),
                    (Cell::Num(l.cum_qty), style::QTY),
                    (Cell::FormulaNum(format!("G{n}-F{n}"), l.period_qty), style::QTY),
                    (Cell::FormulaNum(format!("H{n}*N(E{n})"), l.period_value.unwrap_or(0.0)), style::MONEY),
                    (Cell::FormulaNum(format!("G{n}*N(E{n})"), l.cum_value.unwrap_or(0.0)), style::MONEY),
                ]);
            }
            for v in &c.variations {
                let n = rows.len() + 1;
                rows.push(vec![text(&v.id, style::TEXT), text(t("أمر تغيير", "Variation order"), style::TEXT), text("%", style::TEXT), (Cell::Num(v.cost), style::MONEY), Cell::Empty.into_styled(), Cell::Empty.into_styled(), (Cell::Num(v.cum_pct), style::PCT), Cell::Empty.into_styled(), Cell::Empty.into_styled(), (Cell::FormulaNum(format!("D{n}*G{n}/100"), v.value), style::MONEY)]);
            }
            let end = rows.len();
            let row_n = |k: &str, f: Cell| {
                let mut r = vec![text(k, style::TOTAL_TEXT)];
                r.extend((0..8).map(|_| Cell::Empty.into_styled()));
                r.push((f, style::TOTAL_MONEY));
                r
            };
            let base = rows.len() + 1;
            rows.push(row_n(t("القيمة التراكمية", "Cumulative value"), Cell::FormulaNum(format!("SUM(J2:J{end})"), c.cum_value)));
            rows.push(row_n(t("نسبة الاستقطاع %", "Retention %"), Cell::Num(self.retention_pct.unwrap_or(0.0))));
            rows.push(row_n(t("الاستقطاع", "Retention"), Cell::FormulaNum(format!("J{base}*J{}/100", base + 1), c.retention.unwrap_or(0.0))));
            rows.push(row_n(t("المعتمد سابقاً صافياً من الاستقطاع", "Previously certified, net of retention"), Cell::Num(round(c.previous_cum_value - c.previous_retention, 2))));
            rows.push(row_n(t("الخصومات", "Deductions"), Cell::Num(c.deductions)));
            rows.push(row_n(t("الصافي قبل الضريبة", "Net before VAT"), Cell::FormulaNum(format!("J{base}-J{}-J{}-J{}", base + 2, base + 3, base + 4), c.net_before_vat.unwrap_or(0.0))));
            rows.push(row_n(t("نسبة الضريبة %", "VAT %"), Cell::Num(self.vat_pct.unwrap_or(0.0))));
            rows.push(row_n(t("المستحق الدفع", "Net payable"), Cell::FormulaNum(format!("J{}*(1+J{}/100)", base + 5, base + 6), c.payable.unwrap_or(0.0))));
            let name: String = format!("{} {}", t("مستخلص", "IPC"), c.id).chars().filter(|ch| !matches!(ch, '[' | ']' | ':' | '*' | '?' | '/' | '\\')).take(31).collect();
            sheets.push(Sheet { name, rows, widths: vec![14.0, 44.0, 8.0, 12.0, 12.0, 12.0, 12.0, 12.0, 14.0, 16.0], rtl: arabic, header: true, filter_to: 0 });
        }
        if sheets.is_empty() {
            sheets.push(Sheet { name: t("مستخلصات", "IPC").into(), rows: vec![vec![text(t("لا مستخلصات", "No certificates"), style::TEXT)]], widths: vec![30.0], rtl: arabic, header: false, filter_to: 0 });
        }
        Workbook { sheets }
    }
}

trait IntoStyled {
    fn into_styled(self) -> (Cell, u8);
}

impl IntoStyled for Cell {
    fn into_styled(self) -> (Cell, u8) {
        (self, style::TEXT)
    }
}
