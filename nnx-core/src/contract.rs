//! A DRAFT construction contract assembled from what the user entered: the parties, the value, the duration, the payment milestones
//! (from [`crate::payment`]), the retention, the delay penalty, the warranty and the clauses the user wrote.
//!
//! This module writes no legal text of its own. Every sentence states a figure the user entered, or says it is not entered. The page
//! says on its first line that it is a draft that a licensed lawyer must review, because a contract is a legal instrument.

use crate::execution::{blank, esc, parse_date};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Party {
    pub role: String,
    pub name: String,
    pub id_ref: Option<String>,
    pub address: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Clause {
    pub title: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContractInputs {
    pub parties: Vec<Party>,
    pub contract_value: Option<f64>,
    pub currency: Option<String>,
    pub start_date: Option<String>,
    pub duration_days: Option<u32>,
    pub retention_pct: Option<f64>,
    /// Amount per day of delay, and the cap as a percentage of the contract value.
    pub delay_penalty_per_day: Option<f64>,
    pub penalty_cap_pct: Option<f64>,
    pub defects_liability_months: Option<u32>,
    pub warranty_months: Option<u32>,
    pub governing_law: Option<String>,
    pub clauses: Vec<Clause>,
}

impl ContractInputs {
    pub fn check(&self) -> Result<(), String> {
        if let Some(d) = &self.start_date {
            parse_date("contract start date", d)?;
        }
        for (what, v) in [("contract value", self.contract_value), ("delay penalty per day", self.delay_penalty_per_day)] {
            if v.is_some_and(|x| !x.is_finite() || x < 0.0) {
                return Err(format!("{what} must be a number of zero or more"));
            }
        }
        for (what, v) in [("retention", self.retention_pct), ("penalty cap", self.penalty_cap_pct)] {
            if v.is_some_and(|x| !(0.0..=100.0).contains(&x)) {
                return Err(format!("{what} percentage must be between 0 and 100"));
            }
        }
        if self.parties.iter().any(|p| p.role.trim().is_empty() || p.name.trim().is_empty()) {
            return Err("every party needs a role and a name".into());
        }
        if self.clauses.iter().any(|c| c.title.trim().is_empty() || c.text.trim().is_empty()) {
            return Err("every clause needs a title and a text".into());
        }
        Ok(())
    }
}

/// A payment milestone as the contract lists it.
#[derive(Debug, Clone, Serialize)]
pub struct MilestoneLine {
    pub name: String,
    pub when: Option<String>,
    pub percent: Option<f64>,
    pub amount: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Draft {
    pub html: String,
    /// What the draft still lacks, by name.
    pub missing: Vec<String>,
}

pub fn draft(title: &str, inp: &ContractInputs, milestones: &[MilestoneLine], arabic: bool) -> Draft {
    let t = |ar: &'static str, en: &'static str| if arabic { ar.to_string() } else { en.to_string() };
    let mut missing = Vec::new();
    let na = t("[غير مدخل]", "[not entered]");
    let mut val = |label: String, v: Option<String>| match v {
        Some(s) => s,
        None => {
            missing.push(label);
            na.clone()
        }
    };
    let cur = inp.currency.clone().unwrap_or_default();
    let value = val(t("قيمة العقد", "Contract value"), inp.contract_value.map(|v| format!("{v:.2} {cur}")));
    let start = val(t("تاريخ البدء", "Start date"), inp.start_date.clone().filter(|s| !s.trim().is_empty()));
    let dur = val(t("مدة التنفيذ", "Duration"), inp.duration_days.map(|d| format!("{d} {}", t("يوماً", "days"))));
    let ret = val(t("نسبة الاستقطاع", "Retention"), inp.retention_pct.map(|p| format!("{p}%")));
    let pen = val(t("غرامة التأخير", "Delay penalty"), inp.delay_penalty_per_day.map(|p| format!("{p:.2} {cur} {}", t("لكل يوم تأخير", "per day of delay"))));
    let cap = val(t("سقف الغرامة", "Penalty cap"), inp.penalty_cap_pct.map(|p| format!("{p}% {}", t("من قيمة العقد", "of the contract value"))));
    let dlp = val(t("فترة الصيانة", "Defects liability period"), inp.defects_liability_months.map(|m| format!("{m} {}", t("شهراً", "months"))));
    let war = val(t("مدة الضمان", "Warranty"), inp.warranty_months.map(|m| format!("{m} {}", t("شهراً", "months"))));
    let law = val(t("النظام الحاكم", "Governing law"), inp.governing_law.clone().filter(|s| !s.trim().is_empty()));
    if inp.parties.is_empty() {
        missing.push(t("أطراف العقد", "Parties"));
    }
    if milestones.is_empty() {
        missing.push(t("دفعات العقد", "Payment milestones"));
    }
    if blank(&inp.currency) {
        missing.push(t("العملة", "Currency"));
    }
    let mut h = format!(
        "<!doctype html><html lang=\"{}\" dir=\"{}\"><head><meta charset=\"utf-8\"><title>{}</title><style>body{{font-family:'Segoe UI',Tahoma,sans-serif;margin:30px;color:#111;line-height:1.55}} h1{{font-size:21px}} h2{{font-size:15px;margin:18px 0 6px}} table{{border-collapse:collapse;width:100%;font-size:12px}} th,td{{border:1px solid #999;padding:4px 6px;text-align:start}} th{{background:#eee}} .draft{{border:2px solid #a33;color:#a33;padding:8px;font-weight:700}} .sign{{margin-top:28px;display:flex;gap:40px}} .sign div{{flex:1;border-top:1px solid #333;padding-top:4px;font-size:11px}} @page{{size:A4;margin:14mm}}</style></head><body>",
        if arabic { "ar" } else { "en" },
        if arabic { "rtl" } else { "ltr" },
        esc(title)
    );
    h.push_str(&format!(
        "<p class=\"draft\">{}</p><h1>{} — {}</h1>",
        t("مسودة: ليست عقداً نافذاً ولا استشارة قانونية؛ يجب أن يراجعها محامٍ مرخّص قبل التوقيع.", "DRAFT: not an executed contract and not legal advice; a licensed lawyer must review it before signing."),
        t("عقد مقاولة", "Construction contract"),
        esc(title)
    ));
    h.push_str(&format!("<h2>1. {}</h2><table>", t("الأطراف", "Parties")));
    for p in &inp.parties {
        h.push_str(&format!("<tr><th>{}</th><td>{} {} {}</td></tr>", esc(&p.role), esc(&p.name), esc(p.id_ref.as_deref().unwrap_or("")), esc(p.address.as_deref().unwrap_or(""))));
    }
    h.push_str(&format!("</table><h2>2. {}</h2><table>", t("الشروط التجارية", "Commercial terms")));
    for (k, v) in [
        (t("قيمة العقد", "Contract value"), &value),
        (t("تاريخ البدء", "Start date"), &start),
        (t("مدة التنفيذ", "Duration"), &dur),
        (t("الاستقطاع من كل مستخلص", "Retention from each certificate"), &ret),
        (t("غرامة التأخير", "Delay penalty"), &pen),
        (t("سقف الغرامة", "Penalty cap"), &cap),
        (t("فترة الصيانة", "Defects liability period"), &dlp),
        (t("الضمان", "Warranty"), &war),
        (t("النظام الحاكم", "Governing law"), &law),
    ] {
        h.push_str(&format!("<tr><th>{}</th><td>{}</td></tr>", esc(&k), esc(v)));
    }
    h.push_str(&format!("</table><h2>3. {}</h2><table><tr><th>{}</th><th>{}</th><th>%</th><th>{}</th></tr>", t("جدول الدفعات", "Payment milestones"), t("الدفعة", "Milestone"), t("الموعد", "Date"), t("المبلغ", "Amount")));
    for m in milestones {
        h.push_str(&format!("<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>", esc(&m.name), esc(m.when.as_deref().unwrap_or("—")), m.percent.map_or("—".into(), |p| p.to_string()), m.amount.map_or("—".into(), |a| format!("{a:.2}"))));
    }
    h.push_str("</table>");
    for (i, c) in inp.clauses.iter().enumerate() {
        h.push_str(&format!("<h2>{}. {}</h2><p>{}</p>", i + 4, esc(&c.title), esc(&c.text).replace('\n', "<br>")));
    }
    h.push_str(&format!("<div class=\"sign\">{}</div></body></html>", inp.parties.iter().map(|p| format!("<div>{}</div>", esc(&p.role))).collect::<String>()));
    Draft { html: h, missing }
}
