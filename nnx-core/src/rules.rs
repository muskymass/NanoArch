//! Rule packs and the checker.
//!
//! Rules are data (JSON), not code. A pack carries an id, edition and version.
//! Packs are layered (national code, then municipality, then office standards);
//! a later layer may replace a rule or override its parameters. A parameter whose
//! value is `null` is *unset*: the rule is disabled and reported as such, never
//! silently given a default. Statutory values must come from the official text.

use crate::command::Command;
use crate::expr::{Expr, ExprError, Scope, Val, parse};
use crate::model::{Element, Id, Model};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Param {
    /// `null` means unset: the rule using it is disabled.
    pub value: Option<f64>,
    /// Where the value comes from (clause, edition, municipality circular...).
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Applies {
    pub kind: String,
    /// Optional filter expression over the element, e.g. `isEgress == true`.
    #[serde(default, rename = "where")]
    pub filter: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixSpec {
    pub prop: String,
    /// Expression giving the corrected value.
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleDef {
    pub id: String,
    /// Clause reference in the code; entered from the official text.
    pub clause: String,
    pub applies_to: Applies,
    /// Expression that must be true for the element to comply.
    pub check: String,
    #[serde(default)]
    pub params: BTreeMap<String, Param>,
    pub severity: Severity,
    pub message: String,
    #[serde(default)]
    pub fix: Option<FixSpec>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PackDef {
    pub id: String,
    pub edition: String,
    pub version: String,
    #[serde(default)]
    pub rules: Vec<RuleDef>,
    /// rule id -> param name -> new value (layer overrides).
    #[serde(default)]
    pub overrides: BTreeMap<String, BTreeMap<String, Param>>,
}

#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("rule '{rule}': {source}")]
    Expr { rule: String, source: ExprError },
    #[error("rule '{rule}' references undeclared param '{param}'")]
    UndeclaredParam { rule: String, param: String },
    #[error("duplicate rule id '{0}' in one pack")]
    Duplicate(String),
    #[error("override targets unknown rule '{0}'")]
    UnknownOverride(String),
}

/// A parameter a layer overrode.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Overridden {
    pub rule: String,
    pub param: String,
    /// The value before the layer; none when the rule did not declare the parameter.
    pub from: Option<Param>,
    pub to: Param,
    /// The layer the rule belongs to.
    pub layer: String,
}

/// What one layer did to the rule set.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LayerReport {
    pub label: String,
    pub added: Vec<String>,
    /// `(rule id, label of the layer whose rule was replaced)`.
    pub replaced: Vec<(String, String)>,
    pub overridden: Vec<Overridden>,
}

/// A rule with its expressions parsed ready to run.
#[derive(Debug, Clone)]
struct Compiled {
    def: RuleDef,
    filter: Option<Expr>,
    check: Expr,
    fix: Option<Expr>,
    layer: String,
}

#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    rules: BTreeMap<String, Compiled>,
    /// `pack id@edition/version` of every layer applied, in order.
    pub layers: Vec<String>,
}

impl RuleSet {
    /// Adds a pack on top of the current ones. Rules with an existing id are replaced;
    /// `overrides` patch parameters of rules already loaded.
    pub fn add_layer(&mut self, json: &str) -> Result<(), PackError> {
        self.add_layer_reporting(json).map(|_| ())
    }

    /// Like `add_layer`, and says exactly what the layer did to the rules below it: the rules it added, the rules it replaced (and the
    /// layer each came from) and every parameter it overrode (old and new value, with sources). Nothing changes on an error.
    pub fn add_layer_reporting(&mut self, json: &str) -> Result<LayerReport, PackError> {
        let pack: PackDef = serde_json::from_str(json)?;
        let label = format!("{}@{}/{}", pack.id, pack.edition, pack.version);

        let mut seen = std::collections::BTreeSet::new();
        let mut compiled = Vec::new();
        for def in pack.rules {
            if !seen.insert(def.id.clone()) {
                return Err(PackError::Duplicate(def.id));
            }
            compiled.push(compile(def, &label)?);
        }
        for rule_id in pack.overrides.keys() {
            let in_pack = compiled.iter().any(|c| &c.def.id == rule_id);
            if !in_pack && !self.rules.contains_key(rule_id) {
                return Err(PackError::UnknownOverride(rule_id.clone()));
            }
        }
        let mut report = LayerReport { label: label.clone(), ..Default::default() };
        for c in compiled {
            match self.rules.get(&c.def.id) {
                Some(prev) => report.replaced.push((c.def.id.clone(), prev.layer.clone())),
                None => report.added.push(c.def.id.clone()),
            }
            self.rules.insert(c.def.id.clone(), c);
        }
        for (rule_id, params) in pack.overrides {
            let c = self.rules.get_mut(&rule_id).expect("checked");
            for (name, p) in params {
                let from = c.def.params.insert(name.clone(), p.clone());
                report.overridden.push(Overridden { rule: rule_id.clone(), param: name, from, to: p, layer: c.layer.clone() });
            }
        }
        self.layers.push(label);
        Ok(report)
    }

    /// The rule with this id as it stands now, and the label of the layer it came from.
    pub fn rule(&self, id: &str) -> Option<(&RuleDef, &str)> {
        self.rules.get(id).map(|c| (&c.def, c.layer.as_str()))
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

fn compile(def: RuleDef, layer: &str) -> Result<Compiled, PackError> {
    let wrap = |source| PackError::Expr { rule: def.id.clone(), source };
    let check = parse(&def.check).map_err(wrap)?;
    let filter = def.applies_to.filter.as_deref().map(parse).transpose().map_err(wrap)?;
    let fix = def.fix.as_ref().map(|f| parse(&f.value)).transpose().map_err(wrap)?;

    let mut used = Vec::new();
    check.params(&mut used);
    if let Some(f) = &filter {
        f.params(&mut used);
    }
    if let Some(f) = &fix {
        f.params(&mut used);
    }
    for p in used {
        if !def.params.contains_key(&p) {
            return Err(PackError::UndeclaredParam { rule: def.id.clone(), param: p });
        }
    }
    Ok(Compiled { def, filter, check, fix, layer: layer.to_string() })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub rule_id: String,
    pub clause: String,
    pub layer: String,
    pub element: Id,
    pub severity: Severity,
    pub message: String,
    /// One-click correction, when the rule defines one.
    pub fix: Option<Command>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    pub rule_id: String,
    /// The rule's clause reference and message: what the library is searched with to suggest a source.
    pub clause: String,
    pub message: String,
    /// The first unset parameter: the one a user must enter, with its source, to make the rule run.
    pub param: String,
    pub reason: String,
    /// Elements the rule would have been applied to: they were not checked, which is not the same as passing.
    pub elements: Vec<Id>,
}

#[derive(Debug, Default)]
pub struct Report {
    pub findings: Vec<Finding>,
    /// Rules that did not run (unset parameters), so silence is never mistaken for compliance.
    pub skipped: Vec<Skipped>,
    /// Layers used, recorded in every compliance report.
    pub layers: Vec<String>,
    pub checked: usize,
}

struct ElScope<'a> {
    el: &'a Element,
    params: &'a BTreeMap<String, Param>,
}

fn to_val(v: &Value) -> Option<Val> {
    match v {
        Value::Number(n) => n.as_f64().map(Val::Num),
        Value::Bool(b) => Some(Val::Bool(*b)),
        Value::String(s) => Some(Val::Str(s.clone())),
        _ => None,
    }
}

impl Scope for ElScope<'_> {
    fn lookup(&self, name: &str) -> Option<Val> {
        if let Some(p) = name.strip_prefix("param.") {
            return self.params.get(p)?.value.map(Val::Num);
        }
        if name == "kind" {
            return Some(Val::Str(self.el.kind.clone()));
        }
        let mut parts = name.split('.');
        let mut cur = self.el.props.get(parts.next()?)?;
        for part in parts {
            cur = cur.get(part)?;
        }
        to_val(cur)
    }
}

/// What the rules judge: every element, except that a room made of several rectangles is judged once. Its element carries the whole
/// room's areas and no `w` or `h`; the rectangles' [w, h] come beside it (empty for every other element). A room with no stored
/// `glazingRatio` gets the one measured from its windows.
fn judged_elements(model: &Model) -> Vec<(Element, Vec<[i64; 2]>)> {
    let mut rooms: BTreeMap<Id, crate::annot::WholeRoom> = crate::annot::whole_rooms(model).into_iter().map(|r| (r.element, r)).collect();
    model
        .elements
        .values()
        .filter_map(|e| {
            if e.kind != "Space" {
                return Some((e.clone(), Vec::new()));
            }
            let Some(room) = rooms.remove(&e.id) else {
                // A part is judged with its room. A space that does not read as a room is left as it is.
                let part = e.typed::<crate::typed::Space>().is_ok_and(|s| crate::annot::part_of(&s).is_some());
                return (!part).then(|| (e.clone(), Vec::new()));
            };
            let mut el = e.clone();
            if !el.props.contains_key("glazingRatio") && room.area > 0.0 {
                el.props.insert("glazingRatio".into(), Value::from(room.glazed_m2 / room.area));
            }
            if room.rects.len() < 2 {
                return Some((el, Vec::new()));
            }
            el.props.insert("area".into(), Value::from(room.area));
            if let Some(n) = room.net_area {
                el.props.insert("netArea".into(), Value::from(n));
            }
            el.props.remove("w");
            el.props.remove("h");
            Some((el, room.rects))
        })
        .collect()
}

/// Runs every rule over the model. Deterministic: findings are ordered by rule id, then element id.
pub fn check(model: &Model, rules: &RuleSet) -> Report {
    let mut report = Report { layers: rules.layers.clone(), ..Default::default() };
    let judged = judged_elements(model);
    for c in rules.rules.values() {
        if let Some((name, _)) = c.def.params.iter().find(|(_, p)| p.value.is_none()) {
            // The filter is evaluated without parameters: a filter that needs one counts as "not applicable", as in a run.
            let elements = judged
                .iter()
                .map(|(e, _)| e)
                .filter(|e| e.kind == c.def.applies_to.kind)
                .filter(|e| {
                    let scope = ElScope { el: e, params: &c.def.params };
                    c.filter.as_ref().is_none_or(|f| matches!(f.eval(&scope), Ok(Val::Bool(true))))
                })
                .map(|e| e.id)
                .collect();
            report.skipped.push(Skipped {
                rule_id: c.def.id.clone(),
                clause: c.def.clause.clone(),
                message: c.def.message.clone(),
                param: name.clone(),
                reason: format!("parameter '{name}' is unset; enter it from the official text"),
                elements,
            });
            continue;
        }
        let mut not_judged = Vec::new();
        for (el, rects) in judged.iter().filter(|(e, _)| e.kind == c.def.applies_to.kind) {
            let scope = ElScope { el, params: &c.def.params };
            if let Some(f) = &c.filter {
                // A filter that cannot be evaluated (missing property) means "not applicable".
                if !matches!(f.eval(&scope), Ok(Val::Bool(true))) {
                    continue;
                }
            }
            let mut outcome = c.check.eval(&scope);
            // A room in parts has no one width. A check that asks for it passes when every rectangle passes; otherwise it is not judged.
            if outcome.is_err() && !rects.is_empty() {
                let all_pass = rects.iter().all(|[w, h]| {
                    let mut one = el.clone();
                    one.props.insert("w".into(), Value::from(*w));
                    one.props.insert("h".into(), Value::from(*h));
                    matches!(c.check.eval(&ElScope { el: &one, params: &c.def.params }), Ok(Val::Bool(true)))
                });
                if !all_pass {
                    not_judged.push(el.id);
                    continue;
                }
                outcome = Ok(Val::Bool(true));
            }
            report.checked += 1;
            match outcome {
                Ok(Val::Bool(true)) => {}
                Ok(Val::Bool(false)) => report.findings.push(finding(c, el, &scope, c.def.severity, c.def.message.clone())),
                Ok(other) => report.findings.push(finding(
                    c,
                    el,
                    &scope,
                    Severity::Info,
                    format!("check returned {other}, expected true/false"),
                )),
                Err(e) => report.findings.push(Finding {
                    fix: None,
                    ..finding(c, el, &scope, Severity::Info, format!("could not be evaluated: {e}"))
                }),
            }
        }
        if !not_judged.is_empty() {
            report.skipped.push(Skipped {
                rule_id: c.def.id.clone(),
                clause: c.def.clause.clone(),
                message: c.def.message.clone(),
                param: String::new(),
                reason: format!("{} room(s) of several rectangles: not judged, because one rectangle alone does not pass", not_judged.len()),
                elements: not_judged,
            });
        }
    }
    report.findings.sort_by(|a, b| (&a.rule_id, a.element).cmp(&(&b.rule_id, b.element)));
    report
}

fn finding(c: &Compiled, el: &Element, scope: &ElScope, severity: Severity, message: String) -> Finding {
    let fix = match (&c.def.fix, &c.fix) {
        (Some(spec), Some(expr)) if severity != Severity::Info => match expr.eval(scope) {
            Ok(Val::Num(n)) => Some(Command::SetProp {
                id: el.id,
                key: spec.prop.clone(),
                // Whole millimetres are stored as integers.
                value: Some(if n.fract() == 0.0 { Value::from(n as i64) } else { Value::from(n) }),
            }),
            _ => None,
        },
        _ => None,
    };
    Finding {
        rule_id: c.def.id.clone(),
        clause: c.def.clause.clone(),
        layer: c.layer.clone(),
        element: el.id,
        severity,
        message,
        fix,
    }
}
