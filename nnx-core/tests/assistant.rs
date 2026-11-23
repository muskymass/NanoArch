//! In-app assistant (tracker item 100). All numeric values are SYNTHETIC test values, not building-code values.

use nnx_core::assistant::{advise, apply};
use nnx_core::precheck::{Issue, Level, Report as Integrity};
use nnx_core::rules::{RuleSet, check};
use nnx_core::*;

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
    "id": "door.nofix", "clause": "T-3",
    "applies_to": { "kind": "Door", "where": "isEgress == true" },
    "check": "width >= param.min",
    "params": { "min": { "value": 1000, "source": "synthetic" } },
    "severity": "warning", "message": "door under the second limit"
  }, {
    "id": "room.area", "clause": "T-2",
    "applies_to": { "kind": "Door" },
    "check": "width >= param.min",
    "params": { "min": { "value": null, "source": "" } },
    "severity": "warning", "message": "unset limit"
  }]
}"#;

fn door(width: i64) -> Element {
    Element::new("Door", Discipline::Arc, Source::User).with_prop("width", width).with_prop("isEgress", true)
}

fn setup() -> (Document, RuleSet, Id) {
    let mut rules = RuleSet::default();
    rules.add_layer(PACK).unwrap();
    let mut doc = Document::lenient();
    let d = door(800);
    let id = d.id;
    doc.commit("seed", Origin::User, vec![Command::Create(d)]).unwrap();
    (doc, rules, id)
}

#[test]
fn every_finding_and_skipped_rule_is_explained_and_only_defined_fixes_are_offered() {
    let (doc, rules, id) = setup();
    let rep = check(&doc.model, &rules);
    let adv = advise(&doc.model, &Integrity::default(), Some(&rep));
    // door.width (fix) + door.nofix (no fix) + room.area skipped.
    assert_eq!(adv.items.len(), 3);
    assert_eq!(adv.fixable, 1);
    assert_eq!(adv.items[0].severity, "error", "errors come first");
    assert_eq!(adv.items.last().unwrap().severity, "not_checked", "skipped rules come last");
    let fixed = adv.items.iter().find(|i| i.code == "door.width").unwrap();
    assert_eq!(fixed.id, format!("rule:door.width:{id}"));
    let ch = &fixed.fix.as_ref().unwrap().changes;
    assert_eq!(ch.len(), 1);
    assert_eq!((ch[0].prop.as_str(), ch[0].from.clone(), ch[0].to.clone()), ("width", Some(800.into()), Some(900.into())));
    assert!(fixed.why_en.contains("T-1") && fixed.why_ar.contains("T-1"), "the clause is named");
    let nofix = adv.items.iter().find(|i| i.code == "door.nofix").unwrap();
    assert!(nofix.fix.is_none() && nofix.how_en.contains("no automatic fix"));
    let skipped = adv.items.iter().find(|i| i.source == "skipped").unwrap();
    assert!(skipped.fix.is_none(), "a missing value is never invented");
    assert!(skipped.why_en.contains("NOT checked") && skipped.how_en.contains("where it comes from"));
}

#[test]
fn applying_a_fix_is_one_undoable_step_and_clears_that_finding_only() {
    let (mut doc, rules, id) = setup();
    let adv = advise(&doc.model, &Integrity::default(), Some(&check(&doc.model, &rules)));
    let applied = apply(&mut doc, &adv, &format!("rule:door.width:{id}")).unwrap();
    assert_eq!(applied.changes.len(), 1);
    assert_eq!(doc.model.get(&id).unwrap().props["width"], 900);
    let after = check(&doc.model, &rules);
    assert!(after.findings.iter().all(|f| f.rule_id != "door.width"), "the fixed finding is gone");
    assert!(after.findings.iter().any(|f| f.rule_id == "door.nofix"), "the other finding is untouched");
    assert!(doc.undo());
    assert_eq!(doc.model.get(&id).unwrap().props["width"], 800, "one undo restores the model");
}

#[test]
fn a_missing_stale_or_fixless_item_is_refused_by_name_and_changes_nothing() {
    let (mut doc, rules, id) = setup();
    let adv = advise(&doc.model, &Integrity::default(), Some(&check(&doc.model, &rules)));
    let before = doc.model.clone();
    let e = apply(&mut doc, &adv, "rule:nope:x").unwrap_err();
    assert!(e.contains("not in the current warning list"), "{e}");
    let e = apply(&mut doc, &adv, &format!("rule:door.nofix:{id}")).unwrap_err();
    assert!(e.contains("no automatic fix"), "{e}");
    let e = apply(&mut doc, &adv, "skipped:room.area").unwrap_err();
    assert!(e.contains("no automatic fix"), "{e}");
    assert_eq!(doc.model, before);
}

#[test]
fn integrity_issues_are_explained_by_code_and_an_unknown_code_says_so() {
    let (doc, _, id) = setup();
    let integ = Integrity {
        issues: vec![
            Issue { level: Level::Warning, code: "ROOM.NO_DOOR", message: "level 0: room 'x' has no door".into(), elements: vec![id] },
            Issue { level: Level::Error, code: "ROOM.OVERLAP", message: "overlap".into(), elements: vec![id] },
            Issue { level: Level::Warning, code: "SOME.NEW_CODE", message: "m".into(), elements: vec![] },
        ],
    };
    let adv = advise(&doc.model, &integ, None);
    assert_eq!(adv.items[0].code, "ROOM.OVERLAP", "the error is listed before the warnings");
    assert!(adv.items.iter().all(|i| i.fix.is_none()) && adv.fixable == 0, "no integrity fix is invented");
    let unknown = adv.items.iter().find(|i| i.code == "SOME.NEW_CODE").unwrap();
    assert!(unknown.why_en.contains("no explanation"), "an unknown code is reported as unexplained, not guessed");
    for i in &adv.items {
        assert!(!i.why_ar.is_empty() && !i.how_ar.is_empty() && !i.why_en.is_empty() && !i.how_en.is_empty());
    }
}
