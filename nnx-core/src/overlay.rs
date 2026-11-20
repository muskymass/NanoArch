//! Municipal and authority rule overlays: JSON definitions the user loads on top of their rule pack.
//!
//! An overlay is an ordinary rule pack (`rules::PackDef`: added rules, parameter overrides) plus, optionally, a `limits` object that
//! carries municipal limits (`minSetbackFrontM`, `minSetbackBackM`, `minSetbackSideM`, `maxCoverageRatio`), each a `{ value, source }`.
//! The overlay entry itself names its authority and the document it was taken from.
//!
//! Loading is never silent and never touches the built-in checks:
//! - the pack is parsed and compiled, and every value it carries must have its source (a value without one is refused), every rule its
//!   clause, and the overlay its authority and document;
//! - a **review** states what the overlay would do before anything is stored: the rules it adds, the rules it replaces (and the layer
//!   they came from), every parameter it overrides (old value and source, new value and source) and every limit it would change;
//!   anything that replaces or overrides needs the user's explicit confirmation;
//! - the built-in code checks (`sbc`) are not in the rule set and an overlay cannot reach them. Overlay limits reach the project only
//!   when the user applies them, as one undoable step that shows old and new.
//!
//! Overlays are stored in the project (`overlays`), so undo, redo and the `.nnx` file follow, and they are applied in the order listed,
//! only while enabled.

use crate::execution::parse_date;
use crate::project::{MunicipalLimits, NanoArchProject};
use crate::rules::{LayerReport, Overridden, PackDef, Param, RuleSet};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

/// The largest overlay accepted, in bytes.
pub const MAX_BYTES: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Overlay {
    pub id: String,
    pub name: String,
    /// The authority that issued the values (municipality, ministry, office standard).
    pub authority: String,
    /// The document they come from (circular, regulation, edition).
    pub document: String,
    pub date: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    /// The definition exactly as loaded (JSON text).
    pub pack: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OverlayLimits {
    pub min_setback_front_m: Option<Param>,
    pub min_setback_back_m: Option<Param>,
    pub min_setback_side_m: Option<Param>,
    pub max_coverage_ratio: Option<Param>,
}

impl OverlayLimits {
    fn entries(&self) -> [(&'static str, &Option<Param>, f64); 4] {
        [("front setback", &self.min_setback_front_m, f64::INFINITY), ("rear setback", &self.min_setback_back_m, f64::INFINITY), ("side setback", &self.min_setback_side_m, f64::INFINITY), ("coverage ratio", &self.max_coverage_ratio, 1.0)]
    }
}

/// Reads the optional `limits` of an overlay.
pub fn limits_of(pack: &str) -> Result<Option<OverlayLimits>, String> {
    let v: Value = serde_json::from_str(pack).map_err(|e| format!("json: {e}"))?;
    match v.get("limits") {
        None | Some(Value::Null) => Ok(None),
        Some(l) => serde_json::from_value(l.clone()).map(Some).map_err(|e| format!("limits: {e}")),
    }
}

/// Everything wrong with one overlay on its own (without the layers below it). Empty when it is acceptable.
pub fn problems(o: &Overlay) -> Vec<String> {
    let mut v = Vec::new();
    if o.id.trim().is_empty() || o.name.trim().is_empty() {
        v.push("an overlay needs an id and a name".into());
    }
    if o.authority.trim().is_empty() {
        v.push("the overlay needs its authority (who issued these values)".into());
    }
    if o.document.trim().is_empty() {
        v.push("the overlay needs the document its values come from (circular, regulation, edition)".into());
    }
    if let Some(d) = &o.date
        && let Err(e) = parse_date("overlay date", d)
    {
        v.push(e);
    }
    if o.pack.len() > MAX_BYTES {
        v.push(format!("the definition is larger than {MAX_BYTES} bytes"));
        return v;
    }
    let pack: PackDef = match serde_json::from_str(&o.pack) {
        Ok(p) => p,
        Err(e) => {
            v.push(format!("json: {e}"));
            return v;
        }
    };
    if pack.id.trim().is_empty() || pack.edition.trim().is_empty() || pack.version.trim().is_empty() {
        v.push("the pack needs an id, an edition and a version".into());
    }
    fn param(v: &mut Vec<String>, what: String, p: &Param) {
        if p.value.is_some_and(|x| !x.is_finite()) {
            v.push(format!("{what}: the value is not a number"));
        }
        if p.value.is_some() && p.source.trim().is_empty() {
            v.push(format!("{what}: has a value but no source"));
        }
    }
    for r in &pack.rules {
        if r.clause.trim().is_empty() {
            v.push(format!("rule '{}': no clause reference", r.id));
        }
        for (n, p) in &r.params {
            param(&mut v, format!("rule '{}' parameter '{n}'", r.id), p);
        }
    }
    for (rule, ps) in &pack.overrides {
        for (n, p) in ps {
            param(&mut v, format!("override of '{rule}' parameter '{n}'"), p);
        }
    }
    match limits_of(&o.pack) {
        Err(e) => v.push(e),
        Ok(Some(l)) => {
            for (name, p, max) in l.entries() {
                let Some(p) = p else { continue };
                param(&mut v, format!("limit '{name}'"), p);
                if let Some(x) = p.value
                    && (x < 0.0 || x > max)
                {
                    v.push(format!("limit '{name}': {x} is out of range"));
                }
            }
        }
        Ok(None) => {}
    }
    v
}

/// What the project's list of overlays must satisfy to be stored.
pub fn check_all(list: &[Overlay]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for o in list {
        if !seen.insert(o.id.as_str()) {
            return Err(format!("the overlay id '{}' is used twice", o.id));
        }
        if let Some(first) = problems(o).into_iter().next() {
            return Err(format!("overlay '{}': {first}", o.id));
        }
    }
    if list.len() > 20 {
        return Err("at most 20 overlays are kept in a project".into());
    }
    Ok(())
}

/// The rule set: the optional base pack, then the enabled overlays in order. Also what each overlay did.
pub fn rule_set(base: Option<&str>, list: &[Overlay]) -> Result<(RuleSet, Vec<LayerReport>), String> {
    let mut rs = RuleSet::default();
    let mut reports = Vec::new();
    if let Some(b) = base.filter(|b| !b.trim().is_empty()) {
        reports.push(rs.add_layer_reporting(b).map_err(|e| format!("rule pack: {e}"))?);
    }
    for o in list.iter().filter(|o| o.enabled) {
        reports.push(rs.add_layer_reporting(&o.pack).map_err(|e| format!("overlay '{}': {e}", o.name))?);
    }
    Ok((rs, reports))
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LimitChange {
    pub name: &'static str,
    pub from: Option<Param>,
    pub to: Param,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Review {
    pub label: Option<String>,
    pub errors: Vec<String>,
    pub added: Vec<String>,
    /// `(rule id, layer whose rule is replaced)`.
    pub replaced: Vec<(String, String)>,
    pub overridden: Vec<Overridden>,
    /// Parameters left unset (null) by the overlay: their rules are skipped, never defaulted.
    pub unset: Vec<String>,
    pub limit_changes: Vec<LimitChange>,
    /// The overlay replaces or overrides something already in force: the user must confirm.
    pub needs_confirm: bool,
    pub notes: Vec<String>,
}

fn same(a: &Option<Param>, b: &Param) -> bool {
    a.as_ref().is_some_and(|a| a == b)
}

/// The review of `o` on top of `base` (the layers already in force) and the project's current limits.
pub fn review(base: &RuleSet, o: &Overlay, current: &MunicipalLimits) -> Review {
    let mut r = Review { errors: problems(o), ..Default::default() };
    if !r.errors.is_empty() {
        return r;
    }
    let mut rs = base.clone();
    match rs.add_layer_reporting(&o.pack) {
        Err(e) => r.errors.push(e.to_string()),
        Ok(rep) => {
            r.label = Some(rep.label);
            r.added = rep.added;
            r.replaced = rep.replaced;
            r.overridden = rep.overridden;
        }
    }
    if let Ok(pack) = serde_json::from_str::<PackDef>(&o.pack) {
        for rule in &pack.rules {
            for (n, _) in rule.params.iter().filter(|(_, p)| p.value.is_none()) {
                r.unset.push(format!("{}.{n}", rule.id));
            }
        }
        for (rule, ps) in &pack.overrides {
            for (n, _) in ps.iter().filter(|(_, p)| p.value.is_none()) {
                r.unset.push(format!("{rule}.{n}"));
            }
        }
    }
    if let Ok(Some(l)) = limits_of(&o.pack) {
        let cur = [&current.min_setback_front_m, &current.min_setback_back_m, &current.min_setback_side_m, &current.max_coverage_ratio];
        for ((name, p, _), c) in l.entries().into_iter().zip(cur) {
            match p {
                Some(p) if p.value.is_some() => {
                    if !same(c, p) {
                        r.limit_changes.push(LimitChange { name, from: c.clone(), to: p.clone() });
                    }
                }
                Some(_) => r.notes.push(format!("the {name} limit is left unset by this overlay: it will not change the project's limit")),
                None => {}
            }
        }
    }
    r.needs_confirm = !r.replaced.is_empty() || !r.overridden.is_empty() || !r.limit_changes.is_empty();
    r.notes.push("the built-in code checks are not changed by overlays".into());
    r
}

/// Puts the limits of overlay `o` into the project's limits (only the values it sets). Returns what changed.
pub fn apply_limits(p: &mut NanoArchProject, o: &Overlay) -> Result<Vec<LimitChange>, String> {
    let l = limits_of(&o.pack)?.ok_or("this overlay carries no limits")?;
    let mut changed = Vec::new();
    let targets: [&mut Option<Param>; 4] = [&mut p.limits.min_setback_front_m, &mut p.limits.min_setback_back_m, &mut p.limits.min_setback_side_m, &mut p.limits.max_coverage_ratio];
    for ((name, new, _), slot) in l.entries().into_iter().zip(targets) {
        let Some(new) = new.as_ref().filter(|n| n.value.is_some()) else { continue };
        if !same(slot, new) {
            changed.push(LimitChange { name, from: slot.clone(), to: new.clone() });
            *slot = Some(new.clone());
        }
    }
    if changed.is_empty() {
        return Err("the project already carries these limits (or the overlay sets none)".into());
    }
    Ok(changed)
}

/// Where a project's overlay list is edited from: the unused id `ov1`, `ov2` ...
pub fn next_id(list: &[Overlay]) -> String {
    (1..).map(|n| format!("ov{n}")).find(|id| !list.iter().any(|o| &o.id == id)).unwrap_or_default()
}
