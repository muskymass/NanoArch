//! Progress payment certificates: the contractor's interim payments tied to the percentage of each payment milestone completed.
//!
//! The milestones (and their amounts) come from [`crate::payment`]. A certificate is a dated, referenced statement of how far each
//! milestone has been completed, CUMULATIVELY (0 to 100). Everything is entered by the user and nothing is defaulted:
//! - a milestone a certificate does not mention keeps the completion certified by the previous certificate (0 before the first one);
//! - completion can never go down from one certificate to the next, and never exceeds 100;
//! - the retention percentage is the user's, with its source (the contract clause); without one no retention is deducted and the
//!   report says so;
//! - an amount that cannot be known (a milestone with no amount because the contract value is missing) is reported as unknown, never
//!   as zero, and the totals that depend on it are left out.
//!
//! Payable now = (cumulative earned - cumulative retention) of this certificate - the same figure of the previous one.
//! The result is an arithmetic statement of the user's own entries; it is not a substitute for the engineer's certification.

use crate::execution::{blank, parse_date, round};
use crate::payment::PaymentReport;
use crate::schedule::Date;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Cumulative completion of one payment milestone at a certificate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CertLine {
    /// The id of a payment milestone.
    pub milestone: String,
    /// Cumulative completion, 0 to 100.
    pub completion_pct: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Certificate {
    pub id: String,
    /// `YYYY-MM-DD`.
    pub date: String,
    /// Where the measurement comes from (the engineer's site report, the joint measurement sheet number).
    pub reference: String,
    pub lines: Vec<CertLine>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProgressInputs {
    /// Retention held back from each certificate, 0 to 100, with its source.
    pub retention_pct: Option<f64>,
    pub retention_source: Option<String>,
    pub certificates: Vec<Certificate>,
}

impl ProgressInputs {
    /// `milestones` are the ids of the payment milestones the certificates may refer to.
    pub fn check(&self, milestones: &BTreeSet<&str>) -> Result<(), String> {
        if let Some(r) = self.retention_pct {
            if !r.is_finite() || !(0.0..=100.0).contains(&r) {
                return Err("the retention percentage must be between 0 and 100".into());
            }
            if blank(&self.retention_source) {
                return Err("the retention percentage needs its source (the contract clause)".into());
            }
        }
        let mut ids = BTreeSet::new();
        let mut ordered: Vec<(i64, usize, &Certificate)> = Vec::new();
        for (i, c) in self.certificates.iter().enumerate() {
            if c.id.trim().is_empty() {
                return Err("a certificate needs an id".into());
            }
            if !ids.insert(c.id.as_str()) {
                return Err(format!("the certificate id '{}' is used twice", c.id));
            }
            let date = parse_date(&format!("certificate '{}' date", c.id), &c.date)?;
            if c.reference.trim().is_empty() {
                return Err(format!("certificate '{}' needs its reference (the measurement or site report it comes from)", c.id));
            }
            let mut seen = BTreeSet::new();
            for l in &c.lines {
                if !milestones.contains(l.milestone.as_str()) {
                    return Err(format!("certificate '{}': '{}' is not a payment milestone", c.id, l.milestone));
                }
                if !seen.insert(l.milestone.as_str()) {
                    return Err(format!("certificate '{}': milestone '{}' appears twice", c.id, l.milestone));
                }
                if !l.completion_pct.is_finite() || !(0.0..=100.0).contains(&l.completion_pct) {
                    return Err(format!("certificate '{}': the completion of '{}' must be between 0 and 100", c.id, l.milestone));
                }
            }
            ordered.push((date.to_days(), i, c));
        }
        ordered.sort_by_key(|(d, i, _)| (*d, *i));
        let mut last: BTreeMap<&str, (f64, &str)> = BTreeMap::new();
        for (_, _, c) in &ordered {
            for l in &c.lines {
                if let Some((prev, by)) = last.get(l.milestone.as_str())
                    && l.completion_pct < *prev
                {
                    return Err(format!("certificate '{}': milestone '{}' is at {}% but certificate '{by}' already certified {prev}%: completion cannot go down", c.id, l.milestone, l.completion_pct));
                }
                last.insert(l.milestone.as_str(), (l.completion_pct, c.id.as_str()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LineResult {
    pub milestone: String,
    pub name: String,
    /// Cumulative completion at this certificate (carried forward when the certificate does not mention the milestone).
    pub completion_pct: f64,
    pub previous_pct: f64,
    pub milestone_amount: Option<f64>,
    pub earned_cumulative: Option<f64>,
    pub earned_this: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CertificateResult {
    pub id: String,
    pub date: String,
    pub reference: String,
    pub note: Option<String>,
    pub lines: Vec<LineResult>,
    /// Cumulative value of the work certified so far, before retention.
    pub gross_cumulative: Option<f64>,
    pub retention_cumulative: Option<f64>,
    pub net_cumulative: Option<f64>,
    /// Net cumulative of the previous certificate (0 for the first).
    pub previous_net: Option<f64>,
    /// What this certificate pays: net cumulative minus the previous net cumulative.
    pub payable_now: Option<f64>,
    /// Gross cumulative as a share of the contract value.
    pub contract_completion_pct: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProgressReport {
    pub currency: Option<String>,
    pub contract_value: Option<f64>,
    pub retention_pct: Option<f64>,
    pub retention_source: Option<String>,
    pub certificates: Vec<CertificateResult>,
    pub total_paid_net: Option<f64>,
    pub issues: Vec<String>,
}

/// `payment` is the computed payment report (milestone amounts and the contract value). `inp` must have passed [`ProgressInputs::check`].
pub fn compute(inp: &ProgressInputs, payment: &PaymentReport) -> ProgressReport {
    let mut issues = Vec::new();
    if inp.certificates.is_empty() {
        issues.push("no payment certificates entered".into());
    } else if inp.retention_pct.is_none() {
        issues.push("no retention percentage entered: nothing is held back from the certificates (enter it with its contract clause if the contract has one)".into());
    }
    let info: BTreeMap<&str, (&str, Option<f64>)> = payment.milestones.iter().map(|m| (m.id.as_str(), (m.name.as_str(), m.amount))).collect();

    let mut ordered: Vec<(i64, usize, &Certificate)> = inp.certificates.iter().enumerate().filter_map(|(i, c)| Date::parse(c.date.trim()).map(|d| (d.to_days(), i, c))).collect();
    ordered.sort_by_key(|(d, i, _)| (*d, *i));

    let mut completion: BTreeMap<&str, f64> = BTreeMap::new();
    let mut previous_net: Option<f64> = Some(0.0);
    let mut certificates = Vec::new();
    for (_, _, c) in ordered {
        let before = completion.clone();
        for l in &c.lines {
            completion.insert(l.milestone.as_str(), l.completion_pct);
        }
        let mut lines = Vec::new();
        let mut gross = Some(0.0);
        for (id, (name, amount)) in &info {
            let now = completion.get(id).copied().unwrap_or(0.0);
            let was = before.get(id).copied().unwrap_or(0.0);
            // A milestone with no completion needs no amount; one with work done and no amount makes the totals unknown.
            let earned_cum = match amount {
                Some(a) => Some(round(a * now / 100.0, 2)),
                None if now == 0.0 => Some(0.0),
                None => None,
            };
            let earned_prev = match amount {
                Some(a) => Some(round(a * was / 100.0, 2)),
                None if was == 0.0 => Some(0.0),
                None => None,
            };
            if earned_cum.is_none() {
                issues.push(format!("certificate '{}': milestone '{id}' has {now}% completed but no amount (enter the contract value or the milestone amount): its value and the totals are left out", c.id));
            }
            gross = match (gross, earned_cum) {
                (Some(g), Some(e)) => Some(g + e),
                _ => None,
            };
            if now > 0.0 || was > 0.0 || c.lines.iter().any(|l| l.milestone == *id) {
                lines.push(LineResult { milestone: (*id).to_string(), name: (*name).to_string(), completion_pct: now, previous_pct: was, milestone_amount: *amount, earned_cumulative: earned_cum, earned_this: earned_cum.zip(earned_prev).map(|(a, b)| round(a - b, 2)) });
            }
        }
        let gross = gross.map(|g| round(g, 2));
        let retention = gross.map(|g| round(g * inp.retention_pct.unwrap_or(0.0) / 100.0, 2));
        let net = gross.zip(retention).map(|(g, r)| round(g - r, 2));
        let payable = net.zip(previous_net).map(|(n, p)| round(n - p, 2));
        let share = gross.zip(payment.contract_value).map(|(g, cv)| round(g / cv * 100.0, 4));
        certificates.push(CertificateResult { id: c.id.clone(), date: c.date.trim().to_string(), reference: c.reference.clone(), note: c.note.clone(), lines, gross_cumulative: gross, retention_cumulative: retention, net_cumulative: net, previous_net, payable_now: payable, contract_completion_pct: share });
        previous_net = net;
    }
    let total_paid_net = certificates.last().and_then(|c| c.net_cumulative);
    ProgressReport { currency: payment.currency.clone(), contract_value: payment.contract_value, retention_pct: inp.retention_pct, retention_source: inp.retention_source.clone(), certificates, total_paid_net, issues }
}

impl ProgressReport {
    pub fn csv(&self) -> String {
        let cell = |s: &str| if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
        let f = |v: Option<f64>, d: usize| v.map_or(String::new(), |x| format!("{x:.d$}"));
        let cur = cell(self.currency.as_deref().unwrap_or(""));
        let mut o = String::from("\u{feff}certificate,date,reference,milestone,completion_percent,previous_percent,milestone_amount,earned_cumulative,earned_this,gross_cumulative,retention_cumulative,net_cumulative,payable_now,currency\n");
        for c in &self.certificates {
            for l in &c.lines {
                o.push_str(&format!(
                    "{},{},{},{},{:.4},{:.4},{},{},{},,,,,{cur}\n",
                    cell(&c.id),
                    c.date,
                    cell(&c.reference),
                    cell(&l.milestone),
                    l.completion_pct,
                    l.previous_pct,
                    f(l.milestone_amount, 2),
                    f(l.earned_cumulative, 2),
                    f(l.earned_this, 2)
                ));
            }
            o.push_str(&format!("{},{},{},TOTAL,,,,,,{},{},{},{},{cur}\n", cell(&c.id), c.date, cell(&c.reference), f(c.gross_cumulative, 2), f(c.retention_cumulative, 2), f(c.net_cumulative, 2), f(c.payable_now, 2)));
        }
        o
    }
}
