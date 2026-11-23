//! Milestone contractor payment schedule: the payment milestones tied to the 4D schedule.
//!
//! Each milestone is tied to an activity or a phase of the schedule (at its start or its finish) and carries EITHER a percentage of the
//! contract value OR a fixed amount, both entered by the user. The date of a milestone is the schedule's date, so it moves when the
//! schedule does. The allocation is validated and reported, never adjusted:
//! - with a contract value (entered, or the project's complete BOQ total when the user chose that), the milestone amounts must add up to
//!   it: `balanced`, `under` (part of the contract is not allocated) or `over`;
//! - without one, milestones that are all percentages must add up to 100; amounts cannot be validated and are reported as such.
//!
//! The contract value needs its source. Nothing is defaulted: a milestone with no schedule date is listed without one, with the reason.

use crate::execution::{Activity, Link, Trigger, blank, link_date, round};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Milestone {
    pub id: String,
    pub name: String,
    pub link: Link,
    #[serde(default)]
    pub trigger: Trigger,
    /// Share of the contract value, 0 to 100 (not both with `amount`).
    pub pct: Option<f64>,
    /// A fixed amount (not both with `pct`).
    pub amount: Option<f64>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PaymentInputs {
    pub contract_value: Option<f64>,
    pub contract_source: Option<String>,
    /// Use the project's BOQ total as the contract value (only when every BOQ item is priced).
    pub contract_from_boq: bool,
    pub currency: Option<String>,
    pub milestones: Vec<Milestone>,
    /// Progress certificates: how far each milestone is completed, and what each certificate pays.
    pub progress: crate::progress::ProgressInputs,
}

impl PaymentInputs {
    pub fn check(&self) -> Result<(), String> {
        if let Some(v) = self.contract_value {
            if !v.is_finite() || v <= 0.0 {
                return Err("the contract value must be a positive number".into());
            }
            if blank(&self.contract_source) {
                return Err("the contract value needs its source (the contract, the accepted tender)".into());
            }
        }
        let mut seen = BTreeSet::new();
        for m in &self.milestones {
            if m.id.trim().is_empty() || m.name.trim().is_empty() {
                return Err("a payment milestone needs an id and a name".into());
            }
            if !seen.insert(m.id.as_str()) {
                return Err(format!("the milestone id '{}' is used twice", m.id));
            }
            m.link.check(&format!("milestone '{}'", m.id))?;
            match (m.pct, m.amount) {
                (Some(_), Some(_)) => return Err(format!("milestone '{}': give a percentage or an amount, not both", m.id)),
                (None, None) => return Err(format!("milestone '{}': give a percentage or an amount", m.id)),
                (Some(p), None) if !p.is_finite() || p <= 0.0 || p > 100.0 => return Err(format!("milestone '{}': the percentage must be above 0 and at most 100", m.id)),
                (None, Some(a)) if !a.is_finite() || a < 0.0 => return Err(format!("milestone '{}': the amount must be zero or more", m.id)),
                _ => {}
            }
        }
        let ids: BTreeSet<&str> = self.milestones.iter().map(|m| m.id.as_str()).collect();
        self.progress.check(&ids)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MilestoneResult {
    pub id: String,
    pub name: String,
    pub link: Link,
    pub trigger: Trigger,
    pub date: Option<String>,
    pub pct: Option<f64>,
    pub amount: Option<f64>,
    pub cumulative_amount: Option<f64>,
    pub cumulative_pct: Option<f64>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaymentReport {
    pub currency: Option<String>,
    pub contract_value: Option<f64>,
    pub contract_source: Option<String>,
    pub milestones: Vec<MilestoneResult>,
    /// Sum of the milestone percentages (given, or derived from amounts when the contract value is known).
    pub allocated_pct: f64,
    pub allocated_amount: Option<f64>,
    pub remaining_pct: Option<f64>,
    pub remaining_amount: Option<f64>,
    /// `balanced`, `under`, `over` or `unknown`.
    pub status: &'static str,
    pub balanced: bool,
    pub issues: Vec<String>,
}

/// `acts` is the computed schedule (none when it has not been computed). `boq_total` is the BOQ total when every item is priced.
pub fn compute(inp: &PaymentInputs, acts: Option<&[Activity]>, boq_total: Option<f64>) -> PaymentReport {
    let mut issues = Vec::new();
    let (contract, source) = if inp.contract_from_boq {
        match boq_total {
            Some(t) => (Some(t), Some("BOQ total of this project".to_string())),
            None => {
                issues.push("the contract value is set to the BOQ total, but the BOQ has unpriced items: enter their rates or enter the contract value".into());
                (None, None)
            }
        }
    } else {
        (inp.contract_value, inp.contract_source.clone())
    };
    if contract.is_none() && !inp.contract_from_boq {
        issues.push("no contract value entered: percentages have no amount and amounts cannot be checked against the contract".into());
    }
    if acts.is_none() && !inp.milestones.is_empty() {
        issues.push("the 4D schedule has not been computed (Schedule tab): milestones have no dates".into());
    }

    let mut rows: Vec<(Option<crate::schedule::Date>, usize, MilestoneResult)> = Vec::new();
    for (i, m) in inp.milestones.iter().enumerate() {
        let mut note = m.note.clone();
        let date = match acts {
            Some(a) => match link_date(a, &m.link, m.trigger) {
                Ok(d) => Some(d),
                Err(why) => {
                    issues.push(format!("milestone '{}': {why}", m.id));
                    note = Some(why);
                    None
                }
            },
            None => None,
        };
        let (pct, amount) = match (m.pct, m.amount) {
            (Some(p), _) => (Some(p), contract.map(|c| round(c * p / 100.0, 2))),
            (_, Some(a)) => (contract.map(|c| round(a / c * 100.0, 4)), Some(round(a, 2))),
            _ => (None, None),
        };
        rows.push((date, i, MilestoneResult { id: m.id.clone(), name: m.name.clone(), link: m.link.clone(), trigger: m.trigger, date: date.map(|d| d.iso()), pct, amount, cumulative_amount: None, cumulative_pct: None, note }));
    }
    // By date; undated milestones keep their entry order after the dated ones.
    rows.sort_by_key(|(d, i, _)| (d.is_none(), d.map(|d| d.to_days()), *i));
    let mut milestones: Vec<MilestoneResult> = rows.into_iter().map(|r| r.2).collect();
    let (mut cum_a, mut cum_p) = (0.0, 0.0);
    for m in &mut milestones {
        if let Some(a) = m.amount {
            cum_a += a;
            m.cumulative_amount = Some(round(cum_a, 2));
        }
        if let Some(p) = m.pct {
            cum_p += p;
            m.cumulative_pct = Some(round(cum_p, 4));
        }
    }

    let all_pct = !inp.milestones.is_empty() && inp.milestones.iter().all(|m| m.pct.is_some());
    let allocated_pct = round(milestones.iter().filter_map(|m| m.pct).sum(), 4);
    let amounts_known = !milestones.is_empty() && milestones.iter().all(|m| m.amount.is_some());
    let allocated_amount = amounts_known.then(|| round(milestones.iter().filter_map(|m| m.amount).sum(), 2));
    let (status, remaining_pct, remaining_amount) = if milestones.is_empty() {
        issues.push("no payment milestones entered".into());
        ("unknown", None, None)
    } else if let (Some(c), Some(a)) = (contract, allocated_amount) {
        let diff = round(c - a, 2);
        let s = if diff.abs() < 0.005 { "balanced" } else if diff > 0.0 { "under" } else { "over" };
        (s, Some(round(100.0 - allocated_pct, 4)), Some(diff))
    } else if all_pct {
        let diff = round(100.0 - allocated_pct, 4);
        let s = if diff.abs() < 1e-6 { "balanced" } else if diff > 0.0 { "under" } else { "over" };
        (s, Some(diff), None)
    } else {
        issues.push("amounts cannot be checked without a contract value".into());
        ("unknown", None, None)
    };
    match status {
        "under" => issues.push(match remaining_amount {
            Some(r) => format!("under-allocated: {r:.2} of the contract ({:.4}%) is not tied to any milestone", 100.0 - allocated_pct),
            None => format!("under-allocated: the percentages add up to {allocated_pct:.4}%, {:.4}% is not tied to any milestone", 100.0 - allocated_pct),
        }),
        "over" => issues.push(match remaining_amount {
            Some(r) => format!("over-allocated: the milestones exceed the contract by {:.2}", -r),
            None => format!("over-allocated: the percentages add up to {allocated_pct:.4}%, above 100%"),
        }),
        _ => {}
    }
    PaymentReport { currency: inp.currency.clone(), contract_value: contract, contract_source: source, milestones, allocated_pct, allocated_amount, remaining_pct, remaining_amount, status, balanced: status == "balanced", issues }
}

impl PaymentReport {
    pub fn csv(&self) -> String {
        let cell = |s: &str| if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
        let f = |v: Option<f64>, d: usize| v.map_or(String::new(), |x| format!("{x:.d$}"));
        let mut o = String::from("\u{feff}no,id,milestone,linked_to,trigger,date,percent,amount,cumulative_percent,cumulative_amount,currency\n");
        for (i, m) in self.milestones.iter().enumerate() {
            o.push_str(&format!(
                "{},{},{},{},{},{},{},{},{},{},{}\n",
                i + 1,
                cell(&m.id),
                cell(&m.name),
                cell(&m.link.label()),
                if m.trigger == Trigger::Start { "start" } else { "finish" },
                m.date.as_deref().unwrap_or(""),
                f(m.pct, 4),
                f(m.amount, 2),
                f(m.cumulative_pct, 4),
                f(m.cumulative_amount, 2),
                cell(self.currency.as_deref().unwrap_or(""))
            ));
        }
        o.push_str(&format!(",,Allocation: {},,,,{:.4},{},,,\n", self.status, self.allocated_pct, f(self.allocated_amount, 2)));
        o
    }
}
