//! The change log of the open document: every committed command transaction (tracker item 92), who made it (the user or a
//! generator), the elements it created, deleted or modified, and what it did to the bill of quantities.
//!
//! Nothing is stored per commit. The steps come from the document's own undo stack ([`Document::steps`]): the model before and after
//! each step is rebuilt by applying the stored inverses backwards, so the log can never disagree with `undo()`. A step that was
//! undone is not in the log (it is not part of the document any more); a step that was redone is.
//!
//! Cost impact is the difference between two BOQ reports computed by the caller's pricing function on the model before and after the
//! step, with the rates the project holds NOW (rates are not versioned per commit). A step's impact is therefore "what this change
//! would cost at today's rates". Rules:
//! - an item without a rate contributes nothing to a total, so a step whose before or after has unpriced items is flagged `partial`
//!   and its delta covers only the priced items; it is never presented as the full cost change;
//! - when the pricing function fails for a model, the step carries the reason in `cost_error` and no number is invented;
//! - the log is a record of edits and their arithmetic effect, not an approval or a change-order valuation.

use crate::boq::{BoqReport, BoqRow};
use crate::command::{Command, Origin};
use crate::execution::esc;
use crate::history::{Document, Step};
use crate::model::{Id, Model};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

fn r3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Touched {
    pub id: String,
    pub kind: String,
    /// `created`, `deleted` or `modified` (net effect of the step on that element).
    pub status: &'static str,
    /// Property names set or removed by the step's commands (empty for created and deleted elements).
    pub props: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RowChange {
    pub code: String,
    pub description: String,
    pub unit: String,
    pub quantity_before: f64,
    pub quantity_after: f64,
    pub amount_before: Option<f64>,
    pub amount_after: Option<f64>,
    /// `amount_after - amount_before` when both are priced; `None` when either side has no rate.
    pub amount_delta: Option<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CostImpact {
    pub total_before: f64,
    pub total_after: f64,
    pub delta: f64,
    pub unpriced_before: usize,
    pub unpriced_after: usize,
    /// True when either side has items without a rate: `delta` covers only the priced items.
    pub partial: bool,
    pub rows: Vec<RowChange>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Entry {
    pub seq: usize,
    pub label: String,
    /// `user` or `generator`.
    pub origin: &'static str,
    pub commands: usize,
    pub touched: Vec<Touched>,
    pub cost: Option<CostImpact>,
    pub cost_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Net {
    pub total_before: f64,
    pub total_after: f64,
    pub delta: f64,
    pub partial: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangeLog {
    pub entries: Vec<Entry>,
    /// Steps still on the undo stack, including the ones older than `entries` when a limit cut the log short.
    pub undo_depth: usize,
    /// Whether the log was cut to the newest steps.
    pub truncated: bool,
    /// First-to-last effect over `entries`; `None` when there are no steps or any step's cost could not be computed.
    pub net: Option<Net>,
    pub warnings: Vec<String>,
}

fn origin_name(o: Origin) -> &'static str {
    match o {
        Origin::User => "user",
        Origin::Generator => "generator",
    }
}

fn touched(step: &Step) -> Vec<Touched> {
    let mut props: BTreeMap<Id, BTreeSet<String>> = BTreeMap::new();
    let mut order: Vec<Id> = Vec::new();
    for c in &step.commands {
        let id = match c {
            Command::Create(e) | Command::Restore(e) => e.id,
            Command::Delete(id) => *id,
            Command::SetProp { id, key, .. } => {
                props.entry(*id).or_default().insert(key.clone());
                *id
            }
            Command::SetLocked { id, .. } => *id,
        };
        if !order.contains(&id) {
            order.push(id);
        }
    }
    let mut out = Vec::new();
    for id in order {
        let (b, a) = (step.before.get(&id), step.after.get(&id));
        let (status, kind) = match (b, a) {
            (None, Some(e)) => ("created", e.kind.clone()),
            (Some(e), None) => ("deleted", e.kind.clone()),
            (Some(_), Some(e)) => {
                if b == a {
                    continue; // set and set back inside the same step: no net change
                }
                ("modified", e.kind.clone())
            }
            (None, None) => continue, // created and deleted inside the same step
        };
        let p = if status == "modified" { props.remove(&id).map(|s| s.into_iter().collect()).unwrap_or_default() } else { Vec::new() };
        out.push(Touched { id: id.to_string(), kind, status, props: p });
    }
    out
}

fn row_map(r: &BoqReport) -> BTreeMap<&str, &BoqRow> {
    r.rows.iter().map(|x| (x.code.as_str(), x)).collect()
}

fn impact(before: &BoqReport, after: &BoqReport) -> CostImpact {
    let (bm, am) = (row_map(before), row_map(after));
    let codes: BTreeSet<&str> = bm.keys().chain(am.keys()).copied().collect();
    let mut rows = Vec::new();
    for code in codes {
        let (b, a) = (bm.get(code), am.get(code));
        let (qb, qa) = (b.map_or(0.0, |r| r.quantity), a.map_or(0.0, |r| r.quantity));
        let (ab, aa) = (b.and_then(|r| r.amount), a.and_then(|r| r.amount));
        if (qb - qa).abs() < 5e-4 && ab == aa {
            continue;
        }
        let src = a.or(b).expect("code comes from one of the two maps");
        rows.push(RowChange {
            code: code.to_string(),
            description: src.description.clone(),
            unit: src.unit.clone(),
            quantity_before: r3(qb),
            quantity_after: r3(qa),
            amount_before: ab,
            amount_after: aa,
            amount_delta: match (ab, aa) {
                (Some(x), Some(y)) => Some(r3(y - x)),
                // An item that appears or disappears with a rate is a real change of that amount.
                (None, Some(y)) if b.is_none() => Some(r3(y)),
                (Some(x), None) if a.is_none() => Some(r3(-x)),
                _ => None,
            },
        });
    }
    CostImpact {
        total_before: before.total,
        total_after: after.total,
        delta: r3(after.total - before.total),
        unpriced_before: before.unpriced,
        unpriced_after: after.unpriced,
        partial: before.unpriced > 0 || after.unpriced > 0,
        rows,
    }
}

/// Builds the log of the newest `limit` steps (`None` = all). `price` returns the BOQ report of a model at the project's current rates.
pub fn build(doc: &Document, limit: Option<usize>, price: impl Fn(&Model) -> Result<BoqReport, String>) -> ChangeLog {
    if limit == Some(0) {
        return ChangeLog { entries: vec![], undo_depth: doc.undo_depth(), truncated: doc.undo_depth() > 0, net: None, warnings: vec!["limit 0 shows no steps".into()] };
    }
    let steps = doc.steps(limit);
    let mut entries = Vec::with_capacity(steps.len());
    // Consecutive steps share a model (the after of one is the before of the next): price each model once.
    let mut carried: Option<Result<BoqReport, String>> = None;
    let (mut first_before, mut last_after): (Option<f64>, Option<f64>) = (None, None);
    let (mut all_priced, mut any_partial) = (true, false);
    for step in &steps {
        let before = carried.take().unwrap_or_else(|| price(&step.before));
        let after = price(&step.after);
        let (cost, cost_error) = match (&before, &after) {
            (Ok(b), Ok(a)) => (Some(impact(b, a)), None),
            (Err(e), _) | (_, Err(e)) => (None, Some(e.clone())),
        };
        match &cost {
            Some(c) => {
                first_before.get_or_insert(c.total_before);
                last_after = Some(c.total_after);
                any_partial |= c.partial;
            }
            None => all_priced = false,
        }
        entries.push(Entry { seq: step.seq, label: step.label.clone(), origin: origin_name(step.origin), commands: step.commands.len(), touched: touched(step), cost, cost_error });
        carried = Some(after);
    }
    let net = match (all_priced, first_before, last_after) {
        (true, Some(b), Some(a)) => Some(Net { total_before: b, total_after: a, delta: r3(a - b), partial: any_partial }),
        _ => None,
    };
    let mut warnings = Vec::new();
    if any_partial {
        warnings.push("some steps involve items without a rate: their cost deltas cover only the priced items and are PARTIAL".into());
    }
    if entries.iter().any(|e| e.cost_error.is_some()) {
        warnings.push("the cost of some steps could not be computed (see cost_error); no number was substituted".into());
    }
    if entries.is_empty() {
        warnings.push("no committed steps in this document yet".into());
    }
    warnings.push("cost impact uses the rates the project holds now (rates are not versioned per step)".into());
    let truncated = entries.len() < doc.undo_depth();
    ChangeLog { entries, undo_depth: doc.undo_depth(), truncated, net, warnings }
}

fn num(x: Option<f64>) -> String {
    x.map_or(String::new(), |v| format!("{v:.3}"))
}

fn cell(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

impl ChangeLog {
    /// One row per step and BOQ item that changed (a step with no BOQ effect still gets one row, with empty item columns).
    pub fn csv(&self) -> String {
        let mut o = String::from("seq,label,origin,commands,elements_created,elements_deleted,elements_modified,total_delta,partial,code,unit,quantity_before,quantity_after,amount_delta,cost_error\n");
        for e in &self.entries {
            let n = |s: &str| e.touched.iter().filter(|t| t.status == s).count();
            let head = format!("{},{},{},{},{},{},{}", e.seq, cell(&e.label), e.origin, e.commands, n("created"), n("deleted"), n("modified"));
            let (delta, partial) = e.cost.as_ref().map_or((String::new(), String::new()), |c| (format!("{:.3}", c.delta), c.partial.to_string()));
            let err = cell(e.cost_error.as_deref().unwrap_or(""));
            match e.cost.as_ref().filter(|c| !c.rows.is_empty()) {
                Some(c) => {
                    for r in &c.rows {
                        let _ = writeln!(o, "{head},{delta},{partial},{},{},{:.3},{:.3},{},{err}", cell(&r.code), cell(&r.unit), r.quantity_before, r.quantity_after, num(r.amount_delta));
                    }
                }
                None => {
                    let _ = writeln!(o, "{head},{delta},{partial},,,,,,{err}");
                }
            }
        }
        o
    }

    pub fn html(&self) -> String {
        let mut o = String::from("<!doctype html><meta charset=\"utf-8\"><title>Change log</title><style>body{font-family:sans-serif}table{border-collapse:collapse}td,th{border:1px solid #888;padding:3px 8px;text-align:start}.warn{color:#a05a00}</style><h1>Change log</h1>");
        for w in &self.warnings {
            let _ = write!(o, "<p class=\"warn\">{}</p>", esc(w));
        }
        if let Some(n) = &self.net {
            let _ = write!(o, "<p>Net effect over the listed steps: {:.3} → {:.3} (Δ {:+.3}){}</p>", n.total_before, n.total_after, n.delta, if n.partial { " — PARTIAL" } else { "" });
        }
        o.push_str("<table><tr><th>#</th><th>Step</th><th>By</th><th>Elements</th><th>Δ cost</th><th>BOQ items changed</th></tr>");
        for e in &self.entries {
            let els = e.touched.iter().map(|t| format!("{} {}", t.status, esc(&t.kind))).collect::<Vec<_>>().join(", ");
            let (d, items) = match (&e.cost, &e.cost_error) {
                (Some(c), _) => (format!("{:+.3}{}", c.delta, if c.partial { " (partial)" } else { "" }), c.rows.iter().map(|r| esc(&r.code)).collect::<Vec<_>>().join(", ")),
                (None, Some(err)) => (format!("not computed: {}", esc(err)), String::new()),
                (None, None) => (String::new(), String::new()),
            };
            let _ = write!(o, "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>", e.seq, esc(&e.label), e.origin, els, d, items);
        }
        o.push_str("</table>");
        o
    }
}
