//! Named snapshots (versions) of the project and the comparison of two of them.
//!
//! A snapshot keeps, at the moment it is taken, everything needed to say later what changed: every element (kind, level, host and ALL of
//! its properties, plus its name in the schedules), the quantity of every take-off code and the cost figures of the BOQ and of the
//! selected cost tier. It is stored in the project (`snapshots`), so undo, redo and the `.nnx` file follow; the project element itself
//! and the snapshots are not part of a snapshot.
//!
//! A comparison of A with B reports, from the stored data only:
//! - **elements** added, removed or modified, each modified one with its property changes (`x: 6000 -> 6200`);
//! - **rooms** (by requirement key) added, removed or with a changed area;
//! - **quantities** that differ, by take-off code;
//! - **cost**: the amount of every priced BOQ row that differs, and the BOQ and tier totals when both are complete. A total that is
//!   PARTIAL in either snapshot is shown but its delta is not computed: it would compare different things.

use crate::boq::BoqReport;
use crate::costtier::CostReport;
use crate::execution::round;
use crate::model::Model;
use crate::qto::QtoItem;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// The most snapshots a project keeps.
pub const MAX_SNAPSHOTS: usize = 20;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SnapElement {
    pub kind: String,
    /// The element's name in the schedules (`COL-1`), or the room key, or the kind.
    pub label: String,
    pub level: Option<String>,
    pub host: Option<String>,
    pub props: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CostSnap {
    pub currency: Option<String>,
    /// The BOQ total when at least one item is priced.
    pub boq_total: Option<f64>,
    pub boq_complete: bool,
    /// Amount of every priced BOQ row, by take-off code.
    pub rows: BTreeMap<String, f64>,
    pub tier: Option<String>,
    pub tier_total: Option<f64>,
    pub tier_complete: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Snapshot {
    pub id: String,
    pub name: String,
    pub note: Option<String>,
    pub created_unix: u64,
    pub elements: BTreeMap<String, SnapElement>,
    /// Take-off code -> (unit, quantity).
    pub quantities: BTreeMap<String, (String, f64)>,
    pub costs: CostSnap,
}

pub fn check_all(list: &[Snapshot]) -> Result<(), String> {
    if list.len() > MAX_SNAPSHOTS {
        return Err(format!("a project keeps at most {MAX_SNAPSHOTS} snapshots: delete one first"));
    }
    for (i, s) in list.iter().enumerate() {
        if s.id.trim().is_empty() || s.name.trim().is_empty() {
            return Err("a snapshot needs an id and a name".into());
        }
        if list[..i].iter().any(|o| o.id == s.id || o.name.trim().eq_ignore_ascii_case(s.name.trim())) {
            return Err(format!("the snapshot name '{}' is already used", s.name));
        }
    }
    Ok(())
}

pub fn next_id(list: &[Snapshot]) -> String {
    (1..).map(|n| format!("S{n}")).find(|id| !list.iter().any(|s| &s.id == id)).unwrap_or_default()
}

/// The display name of every room (`Hall`, and `Hall-2` for a divided part) keyed by element id, for a snapshot's labels. Rooms with no display name
/// anywhere are left out, so `capture` falls back to the key as before. Snapshots stay keyed by element id; only the shown label is derived.
pub fn room_labels(model: &Model, names: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    model.elements.values().filter_map(|e| Some((e.id.to_string(), crate::annot::room_name(&e.typed::<crate::typed::Space>().ok().filter(|_| e.kind == "Space")?, names)?))).collect()
}

/// Takes a snapshot of the model as it is. `labels` maps element ids to their names in the schedules.
#[allow(clippy::too_many_arguments)]
pub fn capture(model: &Model, id: &str, name: &str, note: Option<&str>, created_unix: u64, labels: &BTreeMap<String, String>, items: &[QtoItem], boq: &BoqReport, cost: &CostReport) -> Snapshot {
    let elements = model
        .elements
        .values()
        .filter(|e| e.kind != "Project")
        .map(|e| {
            let key = e.id.to_string();
            let label = labels.get(&key).cloned().or_else(|| e.props.get("requirementKey").and_then(Value::as_str).map(str::to_string)).unwrap_or_else(|| e.kind.clone());
            (key, SnapElement { kind: e.kind.clone(), label, level: e.level_id.map(|l| l.to_string()), host: e.host_id.map(|h| h.to_string()), props: e.props.clone() })
        })
        .collect();
    let quantities = items.iter().filter(|i| i.quantity > 0.0).map(|i| (i.code.clone(), (i.unit.label().to_string(), round(i.quantity, 3)))).collect();
    let tier = cost.selected_result();
    let costs = CostSnap {
        currency: boq.project.currency.clone().or_else(|| cost.currency.clone()),
        boq_total: (boq.priced > 0).then_some(boq.total),
        boq_complete: boq.complete,
        rows: boq.rows.iter().filter_map(|r| r.amount.map(|a| (r.code.clone(), a))).collect(),
        tier: tier.map(|t| t.id.to_string()),
        tier_total: tier.map(|t| t.total),
        tier_complete: tier.is_some_and(|t| t.complete),
    };
    Snapshot { id: id.into(), name: name.trim().into(), note: note.map(str::trim).filter(|n| !n.is_empty()).map(str::to_string), created_unix, elements, quantities, costs }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PropChange {
    pub key: String,
    pub from: Option<Value>,
    pub to: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ElementDiff {
    pub id: String,
    pub kind: String,
    pub label: String,
    /// `added`, `removed` or `modified`.
    pub status: &'static str,
    pub changes: Vec<PropChange>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoomDiff {
    pub key: String,
    pub status: &'static str,
    pub area_from: Option<f64>,
    pub area_to: Option<f64>,
    pub delta: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct QtyDiff {
    pub code: String,
    pub unit: String,
    pub from: Option<f64>,
    pub to: Option<f64>,
    pub delta: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CostDiff {
    pub code: String,
    pub from: Option<f64>,
    pub to: Option<f64>,
    pub delta: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Totals {
    pub boq_from: Option<f64>,
    pub boq_to: Option<f64>,
    /// Only when both BOQ totals are complete.
    pub boq_delta: Option<f64>,
    pub tier_from: Option<f64>,
    pub tier_to: Option<f64>,
    /// Only when both snapshots selected the same tier and both totals are complete.
    pub tier_delta: Option<f64>,
    pub currency: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Side {
    pub id: String,
    pub name: String,
    pub created_unix: u64,
    pub elements: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diff {
    pub a: Side,
    pub b: Side,
    pub added: usize,
    pub removed: usize,
    pub modified: usize,
    pub elements: Vec<ElementDiff>,
    pub rooms: Vec<RoomDiff>,
    pub quantities: Vec<QtyDiff>,
    pub costs: Vec<CostDiff>,
    pub totals: Totals,
    pub notes: Vec<String>,
}

fn side(s: &Snapshot) -> Side {
    Side { id: s.id.clone(), name: s.name.clone(), created_unix: s.created_unix, elements: s.elements.len() }
}

fn prop_changes(a: &SnapElement, b: &SnapElement) -> Vec<PropChange> {
    let mut out = Vec::new();
    if a.level != b.level {
        out.push(PropChange { key: "level".into(), from: a.level.clone().map(Value::String), to: b.level.clone().map(Value::String) });
    }
    if a.host != b.host {
        out.push(PropChange { key: "host".into(), from: a.host.clone().map(Value::String), to: b.host.clone().map(Value::String) });
    }
    let keys: std::collections::BTreeSet<&String> = a.props.keys().chain(b.props.keys()).collect();
    for k in keys {
        let (x, y) = (a.props.get(k), b.props.get(k));
        if x != y {
            out.push(PropChange { key: k.clone(), from: x.cloned(), to: y.cloned() });
        }
    }
    out
}

fn rooms(s: &Snapshot) -> BTreeMap<String, f64> {
    s.elements.values().filter(|e| e.kind == "Space").filter_map(|e| Some((e.props.get("requirementKey")?.as_str()?.to_string(), e.props.get("area").and_then(Value::as_f64).unwrap_or(0.0)))).collect()
}

/// What changed from `a` to `b`.
pub fn compare(a: &Snapshot, b: &Snapshot) -> Diff {
    let mut elements = Vec::new();
    for (id, eb) in &b.elements {
        match a.elements.get(id) {
            None => elements.push(ElementDiff { id: id.clone(), kind: eb.kind.clone(), label: eb.label.clone(), status: "added", changes: Vec::new() }),
            Some(ea) => {
                let changes = prop_changes(ea, eb);
                if !changes.is_empty() || ea.kind != eb.kind {
                    elements.push(ElementDiff { id: id.clone(), kind: eb.kind.clone(), label: eb.label.clone(), status: "modified", changes });
                }
            }
        }
    }
    for (id, ea) in &a.elements {
        if !b.elements.contains_key(id) {
            elements.push(ElementDiff { id: id.clone(), kind: ea.kind.clone(), label: ea.label.clone(), status: "removed", changes: Vec::new() });
        }
    }
    let count = |s: &str| elements.iter().filter(|e| e.status == s).count();
    let (added, removed, modified) = (count("added"), count("removed"), count("modified"));

    let (ra, rb) = (rooms(a), rooms(b));
    let mut room_diffs = Vec::new();
    for (k, &to) in &rb {
        match ra.get(k) {
            None => room_diffs.push(RoomDiff { key: k.clone(), status: "added", area_from: None, area_to: Some(to), delta: round(to, 3) }),
            Some(&from) if (from - to).abs() > 1e-9 => room_diffs.push(RoomDiff { key: k.clone(), status: "modified", area_from: Some(from), area_to: Some(to), delta: round(to - from, 3) }),
            _ => {}
        }
    }
    for (k, &from) in &ra {
        if !rb.contains_key(k) {
            room_diffs.push(RoomDiff { key: k.clone(), status: "removed", area_from: Some(from), area_to: None, delta: round(-from, 3) });
        }
    }

    let mut quantities = Vec::new();
    let codes: std::collections::BTreeSet<&String> = a.quantities.keys().chain(b.quantities.keys()).collect();
    for c in codes {
        let (qa, qb) = (a.quantities.get(c), b.quantities.get(c));
        let (from, to) = (qa.map(|q| q.1), qb.map(|q| q.1));
        let delta = round(to.unwrap_or(0.0) - from.unwrap_or(0.0), 3);
        if delta.abs() > 0.0005 {
            quantities.push(QtyDiff { code: c.clone(), unit: qb.or(qa).map(|q| q.0.clone()).unwrap_or_default(), from, to, delta });
        }
    }

    let mut costs = Vec::new();
    let codes: std::collections::BTreeSet<&String> = a.costs.rows.keys().chain(b.costs.rows.keys()).collect();
    for c in codes {
        let (from, to) = (a.costs.rows.get(c).copied(), b.costs.rows.get(c).copied());
        let delta = round(to.unwrap_or(0.0) - from.unwrap_or(0.0), 2);
        if delta.abs() >= 0.005 {
            costs.push(CostDiff { code: c.clone(), from, to, delta });
        }
    }

    let mut notes = Vec::new();
    let boq_delta = match (a.costs.boq_total, b.costs.boq_total) {
        (Some(x), Some(y)) if a.costs.boq_complete && b.costs.boq_complete => Some(round(y - x, 2)),
        (Some(_), Some(_)) => {
            notes.push("a BOQ total is PARTIAL in one of the snapshots: its change is not computed (they price different things)".into());
            None
        }
        _ => {
            notes.push("a snapshot has no priced BOQ item: no BOQ total change".into());
            None
        }
    };
    let tier_delta = match (&a.costs.tier, &b.costs.tier, a.costs.tier_total, b.costs.tier_total) {
        (Some(ta), Some(tb), Some(x), Some(y)) if ta == tb && a.costs.tier_complete && b.costs.tier_complete => Some(round(y - x, 2)),
        (Some(ta), Some(tb), Some(_), Some(_)) if ta != tb => {
            notes.push("the two snapshots selected different cost tiers: no tier change is computed".into());
            None
        }
        (Some(_), Some(_), Some(_), Some(_)) => {
            notes.push("a cost tier total is PARTIAL in one of the snapshots: its change is not computed".into());
            None
        }
        _ => None,
    };
    let totals = Totals { boq_from: a.costs.boq_total, boq_to: b.costs.boq_total, boq_delta, tier_from: a.costs.tier_total, tier_to: b.costs.tier_total, tier_delta, currency: b.costs.currency.clone().or_else(|| a.costs.currency.clone()) };
    Diff { a: side(a), b: side(b), added, removed, modified, elements, rooms: room_diffs, quantities, costs, totals, notes }
}
