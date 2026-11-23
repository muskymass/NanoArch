//! Design decision log: every design decision with who made it (tracker item 98).
//!
//! The log is entered by the user and nothing is defaulted or inferred:
//! - `decided_by` is a person or role the user names; an entry without one is refused (the log never guesses an author);
//! - `date` is a real `YYYY-MM-DD` day, `title` and `decision` are not blank, and `id` is unique;
//! - `elements` (optional) names what the decision concerns, as in the as-built check: a schedule name (`COL-3`), an element id or
//!   `room:<requirement key>`. A reference that matches no element in the model is refused by name when the log is stored; if the
//!   element is deleted later the reference is reported as unresolved, never dropped;
//! - `supersedes` (optional) names an earlier decision of the log that this one replaces. The replaced entry stays in the log and is
//!   reported as superseded by it. A decision cannot replace itself, cannot be replaced by two entries, cannot replace a decision
//!   dated after it, and the chain cannot loop.
//!
//! This is a record of what was decided, by whom and when. It does not judge whether a decision was right, and it is not a legal or
//! engineering approval. It is separate from the undo history (`changelog`), which records edits, not reasons.

use crate::asbuilt::resolve_ref;
use crate::execution::{esc, parse_date};
use crate::model::Model;
use crate::structx::schedule_names;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Decision {
    pub id: String,
    /// `YYYY-MM-DD`.
    pub date: String,
    pub title: String,
    /// What was decided.
    pub decision: String,
    /// Who made the decision (a person or a role), as the user names them.
    pub decided_by: String,
    pub rationale: Option<String>,
    pub category: Option<String>,
    /// What the decision concerns (schedule name, element id or `room:<key>`).
    pub elements: Vec<String>,
    /// The id of the earlier decision this one replaces.
    pub supersedes: Option<String>,
    /// A document or meeting the decision comes from (minutes, letter, drawing number).
    pub reference: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DecisionLog {
    pub decisions: Vec<Decision>,
}

impl DecisionLog {
    /// Checks what can be checked without the model.
    pub fn check(&self) -> Result<(), String> {
        let mut ids: BTreeMap<&str, &Decision> = BTreeMap::new();
        for d in &self.decisions {
            if d.id.trim().is_empty() {
                return Err("a decision needs an id".into());
            }
            if ids.insert(d.id.as_str(), d).is_some() {
                return Err(format!("the decision id '{}' is used twice", d.id));
            }
            if d.title.trim().is_empty() {
                return Err(format!("decision '{}' needs a title", d.id));
            }
            if d.decision.trim().is_empty() {
                return Err(format!("decision '{}' needs the text of what was decided", d.id));
            }
            if d.decided_by.trim().is_empty() {
                return Err(format!("decision '{}' needs the person or role who made it (decidedBy)", d.id));
            }
            parse_date(&format!("decision '{}' date", d.id), &d.date)?;
            let mut seen = BTreeSet::new();
            for e in &d.elements {
                if e.trim().is_empty() {
                    return Err(format!("decision '{}' has an empty element name", d.id));
                }
                if !seen.insert(e.trim()) {
                    return Err(format!("decision '{}' lists '{e}' twice", d.id));
                }
            }
        }
        let mut replaced_by: BTreeMap<&str, &str> = BTreeMap::new();
        for d in &self.decisions {
            let Some(target) = d.supersedes.as_deref() else { continue };
            if target == d.id {
                return Err(format!("decision '{}' cannot supersede itself", d.id));
            }
            let Some(old) = ids.get(target) else {
                return Err(format!("decision '{}' supersedes '{target}', which is not in the log", d.id));
            };
            if let Some(other) = replaced_by.insert(target, d.id.as_str()) {
                return Err(format!("decision '{target}' is superseded by both '{other}' and '{}'", d.id));
            }
            let new_day = parse_date("decision date", &d.date)?.to_days();
            let old_day = parse_date("decision date", &old.date)?.to_days();
            if new_day < old_day {
                return Err(format!("decision '{}' ({}) cannot supersede '{target}', which is dated later ({})", d.id, d.date, old.date));
            }
        }
        // Every supersedes edge points at a different entry and each entry is replaced at most once, so a loop is a closed ring.
        for d in &self.decisions {
            let mut at = d.id.as_str();
            for _ in 0..=self.decisions.len() {
                let Some(next) = ids.get(at).and_then(|x| x.supersedes.as_deref()) else { break };
                if next == d.id {
                    return Err(format!("decision '{}' is part of a supersede loop", d.id));
                }
                at = next;
            }
        }
        Ok(())
    }
}

/// Refuses, by decision and name, every element reference that matches nothing in the model.
pub fn validate_links(model: &Model, log: &DecisionLog) -> Result<(), String> {
    let names = schedule_names(model, None);
    for d in &log.decisions {
        for e in &d.elements {
            if resolve_ref(model, &names, e.trim()).is_none() {
                return Err(format!("decision '{}': '{e}' matches no element in the model", d.id));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct DecisionRow {
    pub id: String,
    pub date: String,
    pub title: String,
    pub decision: String,
    pub decided_by: String,
    pub rationale: Option<String>,
    pub category: Option<String>,
    pub reference: Option<String>,
    pub supersedes: Option<String>,
    /// `active`, or `superseded` when a later entry replaces it.
    pub status: String,
    pub superseded_by: Option<String>,
    pub element_ids: Vec<String>,
    /// References that no longer match an element (deleted after the decision was recorded).
    pub unresolved: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeciderCount {
    pub decided_by: String,
    pub total: usize,
    pub active: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DecisionReport {
    /// Oldest first; entries of the same day keep their entry order.
    pub decisions: Vec<DecisionRow>,
    pub total: usize,
    pub active: usize,
    pub superseded: usize,
    pub by_decider: Vec<DeciderCount>,
    pub first_date: Option<String>,
    pub last_date: Option<String>,
    pub notes: Vec<String>,
}

pub fn report(model: &Model, log: &DecisionLog) -> DecisionReport {
    let names = schedule_names(model, None);
    let mut rep = DecisionReport::default();
    let mut order: Vec<(i64, usize)> = Vec::new();
    for (i, d) in log.decisions.iter().enumerate() {
        match parse_date("decision date", &d.date) {
            Ok(day) => order.push((day.to_days(), i)),
            Err(e) => rep.notes.push(format!("decision '{}': {e}", d.id)),
        }
    }
    order.sort();
    let replaced_by: BTreeMap<&str, &str> = log.decisions.iter().filter_map(|d| d.supersedes.as_deref().map(|t| (t, d.id.as_str()))).collect();
    let mut by: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (_, i) in &order {
        let d = &log.decisions[*i];
        let sup = replaced_by.get(d.id.as_str()).map(|s| s.to_string());
        let mut row = DecisionRow {
            id: d.id.clone(),
            date: d.date.clone(),
            title: d.title.clone(),
            decision: d.decision.clone(),
            decided_by: d.decided_by.clone(),
            rationale: d.rationale.clone(),
            category: d.category.clone(),
            reference: d.reference.clone(),
            supersedes: d.supersedes.clone(),
            status: if sup.is_some() { "superseded".into() } else { "active".into() },
            superseded_by: sup,
            element_ids: Vec::new(),
            unresolved: Vec::new(),
        };
        for e in &d.elements {
            match resolve_ref(model, &names, e.trim()) {
                Some(id) => {
                    let id = id.to_string();
                    if !row.element_ids.contains(&id) {
                        row.element_ids.push(id);
                    }
                }
                None => row.unresolved.push(e.clone()),
            }
        }
        if !row.unresolved.is_empty() {
            rep.notes.push(format!("decision '{}': {} no longer match an element", d.id, row.unresolved.join(", ")));
        }
        let c = by.entry(d.decided_by.trim().to_string()).or_default();
        c.0 += 1;
        if row.status == "active" {
            c.1 += 1;
        }
        rep.decisions.push(row);
    }
    rep.total = rep.decisions.len();
    rep.superseded = rep.decisions.iter().filter(|r| r.status == "superseded").count();
    rep.active = rep.total - rep.superseded;
    rep.by_decider = by.into_iter().map(|(decided_by, (total, active))| DeciderCount { decided_by, total, active }).collect();
    rep.first_date = order.first().map(|(_, i)| log.decisions[*i].date.clone());
    rep.last_date = order.last().map(|(_, i)| log.decisions[*i].date.clone());
    if log.decisions.is_empty() {
        rep.notes.push("no design decisions have been recorded".into());
    }
    rep
}

impl DecisionReport {
    /// One row per decision.
    pub fn csv(&self) -> String {
        let cell = |s: &str| if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
        let mut o = String::from("id,date,decided_by,status,superseded_by,category,title,decision,rationale,reference,element_ids\n");
        for r in &self.decisions {
            let _ = writeln!(
                o,
                "{},{},{},{},{},{},{},{},{},{},{}",
                cell(&r.id),
                r.date,
                cell(&r.decided_by),
                r.status,
                cell(r.superseded_by.as_deref().unwrap_or("")),
                cell(r.category.as_deref().unwrap_or("")),
                cell(&r.title),
                cell(&r.decision),
                cell(r.rationale.as_deref().unwrap_or("")),
                cell(r.reference.as_deref().unwrap_or("")),
                cell(&r.element_ids.join(" "))
            );
        }
        o
    }

    /// A printable page: the decisions by date with who made each one, and the count per decider.
    pub fn html(&self) -> String {
        let mut o = String::from("<!doctype html><meta charset=\"utf-8\"><title>Design decisions</title><h1>Design decisions</h1>");
        let _ = write!(o, "<p>{} decisions ({} active, {} superseded)", self.total, self.active, self.superseded);
        if let (Some(a), Some(b)) = (&self.first_date, &self.last_date) {
            let _ = write!(o, ", {a} to {b}");
        }
        o.push_str("</p><table border=\"1\" cellpadding=\"4\"><tr><th>Date</th><th>Decided by</th><th>Decision</th><th>Reason</th><th>Status</th></tr>");
        for r in &self.decisions {
            let status = match &r.superseded_by {
                Some(s) => format!("superseded by {}", esc(s)),
                None => "active".to_string(),
            };
            let _ = write!(
                o,
                "<tr><td>{}</td><td>{}</td><td><b>{}</b><br>{}</td><td>{}</td><td>{}</td></tr>",
                esc(&r.date),
                esc(&r.decided_by),
                esc(&r.title),
                esc(&r.decision),
                esc(r.rationale.as_deref().unwrap_or("")),
                status
            );
        }
        o.push_str("</table>");
        if !self.by_decider.is_empty() {
            o.push_str("<h2>By decider</h2><ul>");
            for c in &self.by_decider {
                let _ = write!(o, "<li>{}: {} ({} active)</li>", esc(&c.decided_by), c.total, c.active);
            }
            o.push_str("</ul>");
        }
        o
    }
}
