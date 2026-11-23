use nnx_core::numaudit::AuditFile;
use nnx_core::precheck::{Issue, Level, Report as Integrity};
use nnx_core::quality::{QualityWeights, Verdict, build};
use nnx_core::rules::{Finding, Report as RuleReport, Severity, Skipped};
use nnx_core::typed::{Column, element_of};
use nnx_core::*;

fn model(n: usize) -> (Model, Vec<Id>) {
    let mut doc = Document::default();
    let mut ids = Vec::new();
    let mut cmds = Vec::new();
    for i in 0..n {
        let col = element_of(&Column { x: 1000 * i as i64, y: 0, b: 300, h: 300, grid_ref: format!("A{i}"), axial_service_kn: None, extra: Default::default() }, Source::User);
        ids.push(col.id);
        cmds.push(Command::Create(col));
    }
    doc.commit("m", Origin::User, cmds).unwrap_or_else(|e| panic!("{e}"));
    (doc.model, ids)
}

fn issue(level: Level, ids: &[Id]) -> Issue {
    Issue { level, code: "T.CODE", message: "synthetic".into(), elements: ids.to_vec() }
}

fn finding(sev: Severity, id: Id) -> Finding {
    Finding { rule_id: "R1".into(), clause: "synthetic clause".into(), layer: "L".into(), element: id, severity: sev, message: "synthetic".into(), fix: None }
}

fn audit(sourced: usize, unsourced: usize, undated: usize) -> AuditFile {
    AuditFile {
        entries: vec![],
        total: sourced + unsourced,
        sourced,
        unsourced,
        undated,
        unsourced_paths: (0..unsourced).map(|i| format!("p{i}")).collect(),
        notes_en: vec![],
        notes_ar: vec![],
    }
}

fn crit<'a>(r: &'a nnx_core::quality::QualityReport, id: &str) -> &'a nnx_core::quality::Criterion {
    r.criteria.iter().find(|c| c.id == id).unwrap()
}

#[test]
fn every_criterion_is_a_counted_fraction_checked_by_hand() {
    let (m, ids) = model(4);
    let integ = Integrity { issues: vec![issue(Level::Warning, &[ids[0], ids[1]]), issue(Level::Warning, &[ids[1]])] };
    let rules = RuleReport {
        findings: vec![finding(Severity::Warning, ids[0])],
        skipped: vec![Skipped { rule_id: "R2".into(), param: "p".into(), reason: "unset".into(), elements: vec![ids[0], ids[1]] }],
        layers: vec![],
        checked: 6,
    };
    let r = build(&m, &integ, Some(&rules), Some(&audit(3, 1, 0)), &QualityWeights::default()).unwrap();
    // 2 of 4 elements are named by an issue (ids[1] twice counts once).
    assert_eq!((crit(&r, "integrity").counted, crit(&r, "integrity").total), (2, 4));
    assert!((crit(&r, "integrity").score.unwrap() - 50.0).abs() < 1e-9);
    // 5 of 6 checks clean.
    assert!((crit(&r, "rules").score.unwrap() - 100.0 * 5.0 / 6.0).abs() < 1e-9);
    // 6 ran, 2 could not: 6 of 8.
    assert!((crit(&r, "coverage").score.unwrap() - 75.0).abs() < 1e-9);
    assert!((crit(&r, "provenance").score.unwrap() - 75.0).abs() < 1e-9);
    let expect = (50.0 + 100.0 * 5.0 / 6.0 + 75.0 + 75.0) / 4.0;
    assert!((r.overall.unwrap() - expect).abs() < 1e-9);
    assert_eq!(r.verdict, Verdict::Review, "warnings, a skipped rule and an unsourced number remain");
    assert_eq!((r.integrity_warnings, r.rule_warnings, r.rules_skipped, r.numbers_unsourced), (2, 1, 1, 1));
}

#[test]
fn an_error_blocks_whatever_the_score() {
    let (m, ids) = model(50);
    let integ = Integrity { issues: vec![issue(Level::Error, &[ids[0]])] };
    let rules = RuleReport { checked: 100, ..Default::default() };
    let r = build(&m, &integ, Some(&rules), Some(&audit(10, 0, 0)), &QualityWeights::default()).unwrap();
    assert!(r.overall.unwrap() > 95.0, "a high score is shown, not hidden");
    assert_eq!(r.verdict, Verdict::Blocked);
    assert_eq!(r.items[0].severity, "error", "the most serious item comes first");

    let rule_err = RuleReport { findings: vec![finding(Severity::Error, ids[0])], checked: 100, ..Default::default() };
    let r2 = build(&m, &Integrity::default(), Some(&rule_err), Some(&audit(10, 0, 0)), &QualityWeights::default()).unwrap();
    assert_eq!(r2.verdict, Verdict::Blocked);
}

#[test]
fn a_check_that_was_not_run_is_unmeasured_never_a_pass() {
    let (m, _) = model(2);
    let r = build(&m, &Integrity::default(), None, None, &QualityWeights::default()).unwrap();
    for id in ["rules", "coverage", "provenance"] {
        assert!(crit(&r, id).score.is_none(), "{id} must be None");
    }
    assert!((r.overall.unwrap() - 100.0).abs() < 1e-9, "only integrity was measured");
    assert_eq!(r.verdict, Verdict::Review, "an unmeasured criterion keeps the project out of 'clean'");
    assert!(r.notes_en.iter().any(|n| n.contains("rule check was not run")));

    let (empty, _) = model(0);
    let none = build(&empty, &Integrity::default(), None, None, &QualityWeights::default()).unwrap();
    assert!(none.overall.is_none(), "nothing measurable gives no score, not a default");
}

#[test]
fn a_fully_measured_project_with_nothing_left_is_clean() {
    let (m, _) = model(3);
    let rules = RuleReport { checked: 9, ..Default::default() };
    let r = build(&m, &Integrity::default(), Some(&rules), Some(&audit(5, 0, 0)), &QualityWeights::default()).unwrap();
    assert_eq!(r.verdict, Verdict::Clean);
    assert!((r.overall.unwrap() - 100.0).abs() < 1e-9);
}

#[test]
fn weights_change_the_overall_and_bad_weights_are_refused_by_name() {
    let (m, ids) = model(4);
    let integ = Integrity { issues: vec![issue(Level::Warning, &[ids[0], ids[1]])] };
    let a = audit(4, 0, 0);
    let rules = RuleReport { checked: 4, ..Default::default() };
    let only_integrity = QualityWeights { integrity: 1.0, rules: 0.0, coverage: 0.0, provenance: 0.0 };
    let r = build(&m, &integ, Some(&rules), Some(&a), &only_integrity).unwrap();
    assert!((r.overall.unwrap() - 50.0).abs() < 1e-9);
    assert_eq!(r.weights, only_integrity, "the weights used are returned");
    let neg = QualityWeights { rules: -1.0, ..Default::default() };
    assert!(build(&m, &integ, None, None, &neg).unwrap_err().contains("rules"));
    let zero = QualityWeights { integrity: 0.0, rules: 0.0, coverage: 0.0, provenance: 0.0 };
    assert!(build(&m, &integ, None, None, &zero).is_err());
}

#[test]
fn csv_and_html_carry_the_score_and_escape_text() {
    let (m, ids) = model(2);
    let mut i = issue(Level::Warning, &[ids[0]]);
    i.message = "a <b> & \"c\"".into();
    let r = build(&m, &Integrity { issues: vec![i] }, None, None, &QualityWeights::default()).unwrap();
    let csv = r.csv();
    assert!(csv.starts_with("criterion,score,counted,total,weight,basis\n"));
    assert!(csv.contains("overall,50.0,"));
    assert!(csv.contains("\"\"c\"\""), "quotes are doubled");
    let html = r.html();
    assert!(html.contains("&lt;b&gt; &amp;") && !html.contains("<b> &"));
    assert!(html.contains("50.0 / 100"));
}
