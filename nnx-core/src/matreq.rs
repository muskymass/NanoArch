//! Material requests ordered by construction stage.
//!
//! The BOQ lists every material of the whole project. A site orders by stage: what the foundations need is not what the finishing
//! needs. This splits the project's material list across the stages of the computed schedule (`schedule::PHASES`, in construction
//! order) by the take-off codes each scheduled task works from, so every request is the BOQ's own material arithmetic over the items
//! of that stage and nothing else.
//!
//! - A take-off code that tasks of two or more stages share cannot be split without inventing a ratio: it is `ambiguous`, named, and
//!   its materials are listed apart (`unassigned`), never put in a stage.
//! - A code no task works from is `unscheduled` and its materials are `unassigned` the same way.
//!   The stage requests plus `unassigned` always add up to the project material list (checked in the tests).
//! - Lead times come from the supplier and are entered by the user per material, each with its source. With a lead time a request has
//!   an order-by date (stage start - lead - buffer, calendar days); without one it is listed with no date. Nothing is defaulted.

use crate::boq::{self, BoqLib, MaterialRow};
use crate::qto::QtoItem;
use crate::schedule::{Date, PHASES, Schedule, Task};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lead {
    pub days: u32,
    /// Where the lead time comes from (a supplier quotation, a data sheet).
    pub source: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MatReqInputs {
    /// The reference date for "already passed", `YYYY-MM-DD`. Unset: the system date is used and said so.
    pub as_of: Option<String>,
    /// Safety margin in days subtracted from every order date; unset counts as zero and is shown as such.
    pub buffer_days: Option<u32>,
    /// Material name (as in the BOQ) -> lead time.
    pub lead: BTreeMap<String, Lead>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Request {
    pub material: String,
    pub unit: String,
    pub quantity: f64,
    pub spec: Option<String>,
    pub used_in: Vec<String>,
    pub lead_days: Option<u32>,
    pub lead_source: Option<String>,
    pub order_by: Option<String>,
    /// Days from `as_of` to the order date; negative once it has passed.
    pub days_left: Option<i64>,
    pub overdue: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct StageRequest {
    pub stage: String,
    pub start: String,
    pub finish: String,
    /// The take-off codes of this stage's tasks that carry a quantity.
    pub codes: Vec<String>,
    pub requests: Vec<Request>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Ambiguous {
    pub code: String,
    pub stages: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MatReqReport {
    pub as_of: Option<String>,
    pub as_of_source: &'static str,
    pub buffer_days: u32,
    pub stages: Vec<StageRequest>,
    pub ambiguous: Vec<Ambiguous>,
    pub unscheduled: Vec<String>,
    pub unassigned: Vec<MaterialRow>,
    pub issues: Vec<String>,
}

pub fn check(inp: &MatReqInputs) -> Result<(), String> {
    if let Some(d) = &inp.as_of
        && Date::parse(d).is_none()
    {
        return Err(format!("as_of '{d}' is not a real date (YYYY-MM-DD)"));
    }
    if inp.buffer_days.is_some_and(|d| d > 3650) {
        return Err("a buffer above 3650 days is not a number of days".into());
    }
    for (m, l) in &inp.lead {
        if m.trim().is_empty() {
            return Err("a lead time needs a material name".into());
        }
        if l.source.trim().is_empty() {
            return Err(format!("material '{m}': the lead time needs its source (supplier quotation, data sheet)"));
        }
        if l.days > 3650 {
            return Err(format!("material '{m}': a lead time above 3650 days is not a number of days"));
        }
    }
    Ok(())
}

/// `items` are the project take-off items (the BOQ input), `lib` the BOQ library with the user's material factors, `sched` the
/// computed schedule, `today` the system date used when `as_of` is unset.
pub fn build(items: &[QtoItem], lib: &BoqLib, sched: &Schedule, inp: &MatReqInputs, today: Option<Date>) -> Result<MatReqReport, String> {
    check(inp)?;
    let (as_of, as_of_source) = match inp.as_of.as_deref().and_then(Date::parse) {
        Some(d) => (Some(d), "entered"),
        None => (today, "system clock"),
    };
    let buffer = inp.buffer_days.unwrap_or(0);

    // code -> the stages whose tasks work from it (only codes that carry a quantity in the take-off)
    let measured: BTreeSet<&str> = items.iter().filter(|i| i.quantity > 0.0).map(|i| i.code.as_str()).collect();
    let mut code_stages: BTreeMap<&str, BTreeSet<&'static str>> = BTreeMap::new();
    for t in &sched.tasks {
        for c in &t.qto_codes {
            if measured.contains(c.as_str()) {
                code_stages.entry(c.as_str()).or_default().insert(t.phase);
            }
        }
    }
    let ambiguous: Vec<Ambiguous> = code_stages.iter().filter(|(_, s)| s.len() > 1).map(|(c, s)| Ambiguous { code: (*c).to_string(), stages: s.iter().map(|x| (*x).to_string()).collect() }).collect();
    let ambiguous_codes: BTreeSet<&str> = ambiguous.iter().map(|a| a.code.as_str()).collect();
    let unscheduled: Vec<String> = measured.iter().filter(|c| !code_stages.contains_key(*c)).map(|c| (*c).to_string()).collect();

    let mut stages = Vec::new();
    let mut seen_materials: BTreeSet<String> = BTreeSet::new();
    for stage in PHASES {
        let tasks: Vec<&Task> = sched.tasks.iter().filter(|t| t.phase == stage).collect();
        let (Some(start), Some(finish)) = (tasks.iter().map(|t| t.start).min(), tasks.iter().map(|t| t.finish).max()) else { continue };
        let codes: Vec<String> = code_stages.iter().filter(|(c, s)| s.len() == 1 && s.contains(stage) && !ambiguous_codes.contains(*c)).map(|(c, _)| (*c).to_string()).collect();
        let stage_items: Vec<QtoItem> = items.iter().filter(|i| codes.contains(&i.code)).cloned().collect();
        let report = boq::compute(&stage_items, lib);
        let requests: Vec<Request> = report
            .materials
            .iter()
            .map(|m| {
                seen_materials.insert(m.material.clone());
                let lead = inp.lead.get(&m.material);
                let order = lead.map(|l| Date::from_days(start.to_days() - i64::from(l.days) - i64::from(buffer)));
                let days_left = order.zip(as_of).map(|(o, a)| o.to_days() - a.to_days());
                Request {
                    material: m.material.clone(),
                    unit: m.unit.clone(),
                    quantity: m.quantity,
                    spec: m.spec.clone(),
                    used_in: m.used_in.clone(),
                    lead_days: lead.map(|l| l.days),
                    lead_source: lead.map(|l| l.source.clone()),
                    order_by: order.map(Date::iso),
                    days_left,
                    overdue: days_left.is_some_and(|d| d < 0),
                }
            })
            .collect();
        if !codes.is_empty() {
            stages.push(StageRequest { stage: stage.to_string(), start: start.iso(), finish: finish.iso(), codes, requests });
        }
    }

    let rest: Vec<QtoItem> = items.iter().filter(|i| unscheduled.contains(&i.code) || ambiguous_codes.contains(i.code.as_str())).cloned().collect();
    let unassigned = boq::compute(&rest, lib).materials;
    seen_materials.extend(unassigned.iter().map(|m| m.material.clone()));

    for m in inp.lead.keys() {
        if !seen_materials.contains(m) {
            return Err(format!("a lead time is given for '{m}', which no item of this project needs (check the material name)"));
        }
    }

    let mut issues = Vec::new();
    if as_of.is_none() && !inp.lead.is_empty() {
        issues.push("no as_of date and no system date: overdue cannot be judged".into());
    }
    if !ambiguous.is_empty() {
        issues.push(format!("{} take-off code(s) are worked on in more than one stage and are not split: {}", ambiguous.len(), ambiguous.iter().map(|a| a.code.as_str()).collect::<Vec<_>>().join(", ")));
    }
    if !unassigned.is_empty() {
        issues.push(format!("{} material(s) belong to items that no scheduled task works from or that span stages: listed under unassigned, in no stage", unassigned.len()));
    }
    for s in &stages {
        let missing: Vec<&str> = s.requests.iter().filter(|r| r.lead_days.is_none()).map(|r| r.material.as_str()).collect();
        if !missing.is_empty() {
            issues.push(format!("{}: no lead time entered for {}", s.stage, missing.join(", ")));
        }
    }
    Ok(MatReqReport { as_of: as_of.map(Date::iso), as_of_source, buffer_days: buffer, stages, ambiguous, unscheduled, unassigned, issues })
}

fn cell(s: &str) -> String {
    if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

impl MatReqReport {
    pub fn csv(&self) -> String {
        let mut o = String::from("\u{feff}stage,stage_start,material,unit,quantity,specification,lead_days,lead_source,order_by,days_left,overdue,used_in\n");
        for s in &self.stages {
            for r in &s.requests {
                o.push_str(&format!(
                    "{},{},{},{},{:.3},{},{},{},{},{},{},{}\n",
                    cell(&s.stage),
                    s.start,
                    cell(&r.material),
                    cell(&r.unit),
                    r.quantity,
                    cell(r.spec.as_deref().unwrap_or("")),
                    r.lead_days.map(|d| d.to_string()).unwrap_or_default(),
                    cell(r.lead_source.as_deref().unwrap_or("")),
                    r.order_by.clone().unwrap_or_default(),
                    r.days_left.map(|d| d.to_string()).unwrap_or_default(),
                    r.overdue,
                    cell(&r.used_in.join(";"))
                ));
            }
        }
        for m in &self.unassigned {
            o.push_str(&format!("unassigned,,{},{},{:.3},{},,,,,,{}\n", cell(&m.material), cell(&m.unit), m.quantity, cell(m.spec.as_deref().unwrap_or("")), cell(&m.used_in.join(";"))));
        }
        o
    }
}
