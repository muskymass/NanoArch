//! All numeric values below are SYNTHETIC test values. They are not building-code values.

use nnx_core::expr::{self, Scope, Val, parse};
use nnx_core::rules::{PackError, RuleSet, Severity, check};
use nnx_core::*;

struct Vars;
impl Scope for Vars {
    fn lookup(&self, n: &str) -> Option<Val> {
        match n {
            "x" => Some(Val::Num(10.0)),
            "flag" => Some(Val::Bool(true)),
            "s" => Some(Val::Str("a".into())),
            _ => None,
        }
    }
}

fn ev(src: &str) -> Result<Val, expr::ExprError> {
    parse(src)?.eval(&Vars)
}

#[test]
fn expression_precedence_and_functions() {
    assert_eq!(ev("1 + 2 * 3").unwrap(), Val::Num(7.0));
    assert_eq!(ev("(1 + 2) * 3").unwrap(), Val::Num(9.0));
    assert_eq!(ev("-x + 4").unwrap(), Val::Num(-6.0));
    assert_eq!(ev("max(1, x, 3) - min(4, 2)").unwrap(), Val::Num(8.0));
    assert_eq!(ev("x >= 10 && flag && !false").unwrap(), Val::Bool(true));
    assert_eq!(ev("s == \"a\" || y > 1").unwrap(), Val::Bool(true), "|| short-circuits past the missing y");
    assert_eq!(ev("s != \"a\" || y > 1").unwrap_err(), expr::ExprError::Missing("y".into()));
}

#[test]
fn expression_errors_are_reported_not_panics() {
    assert!(matches!(parse("1 +"), Err(expr::ExprError::Syntax { .. })));
    assert!(matches!(parse("(1"), Err(expr::ExprError::Syntax { .. })));
    assert!(matches!(parse("1 2"), Err(expr::ExprError::Syntax { .. })));
    assert!(matches!(parse("sqrt(4)"), Err(expr::ExprError::Syntax { .. })));
    assert!(matches!(parse("a $ b"), Err(expr::ExprError::Syntax { .. })));
    assert!(matches!(ev("1 / 0"), Err(expr::ExprError::Type(_))));
    assert!(matches!(ev("flag + 1"), Err(expr::ExprError::Type(_))));
}

const PACK: &str = r#"{
  "id": "TEST", "edition": "synthetic", "version": "1",
  "rules": [{
    "id": "door.width", "clause": "T-1",
    "applies_to": { "kind": "Door", "where": "isEgress == true" },
    "check": "width >= param.min",
    "params": { "min": { "value": 900, "source": "synthetic" } },
    "severity": "error", "message": "door too narrow",
    "fix": { "prop": "width", "value": "param.min" }
  }, {
    "id": "room.area", "clause": "T-2",
    "applies_to": { "kind": "Space" },
    "check": "area >= param.min",
    "params": { "min": { "value": null, "source": "" } },
    "severity": "warning", "message": "room too small"
  }]
}"#;

fn door(width: i64, egress: bool) -> Element {
    Element::new("Door", Discipline::Arc, Source::User).with_prop("width", width).with_prop("isEgress", egress)
}

#[test]
fn finds_violations_respects_filter_and_offers_working_fix() {
    let mut rules = RuleSet::default();
    rules.add_layer(PACK).unwrap();

    let mut doc = Document::lenient();
    let bad = door(800, true);
    let ok = door(950, true);
    let not_egress = door(600, false);
    let bad_id = bad.id;
    doc.commit("seed", Origin::User, vec![Command::Create(bad), Command::Create(ok), Command::Create(not_egress)]).unwrap();

    let report = check(&doc.model, &rules);
    assert_eq!(report.checked, 2, "non-egress door is out of scope");
    assert_eq!(report.findings.len(), 1);
    let f = &report.findings[0];
    assert_eq!((f.element, f.severity, f.clause.as_str()), (bad_id, Severity::Error, "T-1"));
    assert_eq!(f.layer, "TEST@synthetic/1");

    // Applying the suggested fix clears the finding, and it is undoable.
    doc.commit("fix", Origin::User, vec![f.fix.clone().unwrap()]).unwrap();
    assert!(check(&doc.model, &rules).findings.is_empty());
    doc.undo();
    assert_eq!(check(&doc.model, &rules).findings.len(), 1);
}

#[test]
fn unset_parameter_disables_rule_visibly() {
    let mut rules = RuleSet::default();
    rules.add_layer(PACK).unwrap();
    let mut doc = Document::lenient();
    let space = Element::new("Space", Discipline::Arc, Source::User).with_prop("area", 1);
    doc.commit("s", Origin::User, vec![Command::Create(space)]).unwrap();
    let report = check(&doc.model, &rules);
    assert!(report.findings.is_empty(), "no silent default value is applied");
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.skipped[0].rule_id, "room.area");
}

#[test]
fn layer_override_changes_threshold_and_records_layer() {
    let mut rules = RuleSet::default();
    rules.add_layer(PACK).unwrap();
    rules
        .add_layer(
            r#"{"id":"MUNI","edition":"synthetic","version":"2",
                "overrides":{"door.width":{"min":{"value":1000,"source":"synthetic circular"}}}}"#,
        )
        .unwrap();
    let mut doc = Document::lenient();
    doc.commit("d", Origin::User, vec![Command::Create(door(950, true))]).unwrap();
    let report = check(&doc.model, &rules);
    assert_eq!(report.findings.len(), 1, "950 passes the base pack but fails the stricter layer");
    assert_eq!(report.layers.len(), 2);
}

#[test]
fn missing_property_is_info_not_false_pass() {
    let mut rules = RuleSet::default();
    rules.add_layer(PACK).unwrap();
    let mut doc = Document::lenient();
    let no_width = Element::new("Door", Discipline::Arc, Source::User).with_prop("isEgress", true);
    doc.commit("d", Origin::User, vec![Command::Create(no_width)]).unwrap();
    let report = check(&doc.model, &rules);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].severity, Severity::Info);
    assert!(report.findings[0].fix.is_none());
}

#[test]
fn bad_packs_are_rejected_at_load() {
    let mut rules = RuleSet::default();
    let undeclared = PACK.replace(r#""params": { "min": { "value": 900, "source": "synthetic" } },"#, "");
    assert!(matches!(rules.add_layer(&undeclared), Err(PackError::UndeclaredParam { .. })));
    assert!(matches!(
        rules.add_layer(r#"{"id":"X","edition":"e","version":"1","overrides":{"nope":{}}}"#),
        Err(PackError::UnknownOverride(_))
    ));
    assert!(matches!(rules.add_layer(&PACK.replace("width >=", "width >>")), Err(PackError::Expr { .. })));
    assert!(matches!(rules.add_layer("{"), Err(PackError::Json(_))));
}

#[test]
fn shipped_template_loads_and_everything_is_disabled_until_filled_in() {
    let mut rules = RuleSet::default();
    rules.add_layer(include_str!("../packs/sbc-template.json")).unwrap();
    assert_eq!(rules.len(), 3);
    let mut doc = Document::lenient();
    doc.commit("d", Origin::User, vec![Command::Create(door(1, true))]).unwrap();
    let report = check(&doc.model, &rules);
    assert!(report.findings.is_empty());
    assert_eq!(report.skipped.len(), 3);
}

#[test]
fn hostile_expressions_are_rejected_not_fatal() {
    use nnx_core::expr::{ExprError, parse};
    let deep = format!("{}1{}", "(".repeat(5_000), ")".repeat(5_000));
    assert!(matches!(parse(&deep), Err(ExprError::Syntax { .. })));
    assert!(matches!(parse(&format!("{}1", "-".repeat(5_000))), Err(ExprError::Syntax { .. })));
    assert!(matches!(parse(&vec!["1"; 5_000].join("+")), Err(ExprError::Syntax { .. })));
    // Reasonable nesting still works.
    let ok = format!("{}1{}", "(".repeat(60), ")".repeat(60));
    assert!(parse(&ok).is_ok());
}
