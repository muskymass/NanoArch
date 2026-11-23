//! Progress payment certificates (tracker item 90): payments tied to the completion percentage of each payment milestone.

use nnx_core::execution::{Link, Trigger};
use nnx_core::payment::{self, Milestone, PaymentInputs};
use nnx_core::progress::{CertLine, Certificate, ProgressInputs, compute};
use std::collections::BTreeSet;

fn ms(id: &str, pct: f64) -> Milestone {
    Milestone { id: id.into(), name: id.to_uppercase(), link: Link::Task { key: "excavate".into() }, trigger: Trigger::Finish, pct: Some(pct), amount: None, note: None }
}

fn line(m: &str, c: f64) -> CertLine {
    CertLine { milestone: m.into(), completion_pct: c }
}

fn cert(id: &str, date: &str, lines: Vec<CertLine>) -> Certificate {
    Certificate { id: id.into(), date: date.into(), reference: format!("site report {id}"), lines, note: None }
}

/// Contract 1000: a = 40% (400), b = 60% (600). Retention 10%.
fn base(certs: Vec<Certificate>) -> PaymentInputs {
    PaymentInputs {
        contract_value: Some(1000.0),
        contract_source: Some("synthetic contract".into()),
        milestones: vec![ms("a", 40.0), ms("b", 60.0)],
        progress: ProgressInputs { retention_pct: Some(10.0), retention_source: Some("contract clause 5".into()), certificates: certs },
        ..Default::default()
    }
}

fn run(inp: &PaymentInputs) -> nnx_core::progress::ProgressReport {
    inp.check().unwrap();
    let pay = payment::compute(inp, None, None);
    compute(&inp.progress, &pay)
}

#[test]
fn each_certificate_pays_the_new_net_value_of_the_completion_certified() {
    let inp = base(vec![cert("c1", "2026-01-31", vec![line("a", 50.0)]), cert("c2", "2026-02-28", vec![line("a", 100.0), line("b", 25.0)])]);
    let r = run(&inp);
    let (c1, c2) = (&r.certificates[0], &r.certificates[1]);
    // c1: a 50% of 400 = 200 gross, retention 20, net 180, pays 180.
    assert_eq!(c1.gross_cumulative, Some(200.0));
    assert_eq!(c1.retention_cumulative, Some(20.0));
    assert_eq!(c1.net_cumulative, Some(180.0));
    assert_eq!(c1.payable_now, Some(180.0));
    assert_eq!(c1.contract_completion_pct, Some(20.0));
    // c2: a 400 + b 25% of 600 = 150 -> 550 gross, retention 55, net 495, pays 495 - 180 = 315.
    assert_eq!(c2.gross_cumulative, Some(550.0));
    assert_eq!(c2.net_cumulative, Some(495.0));
    assert_eq!(c2.previous_net, Some(180.0));
    assert_eq!(c2.payable_now, Some(315.0));
    assert_eq!(r.total_paid_net, Some(495.0));
    let b = c2.lines.iter().find(|l| l.milestone == "b").unwrap();
    assert_eq!((b.previous_pct, b.completion_pct, b.earned_this), (0.0, 25.0, Some(150.0)));
    // The payments of every certificate add up to the last net cumulative.
    assert_eq!(c1.payable_now.unwrap() + c2.payable_now.unwrap(), 495.0);
}

#[test]
fn a_milestone_a_certificate_does_not_mention_keeps_its_certified_completion() {
    let inp = base(vec![cert("c1", "2026-01-31", vec![line("a", 100.0)]), cert("c2", "2026-02-28", vec![line("b", 50.0)])]);
    let r = run(&inp);
    // c2 does not mention a: it stays at 100% (400) and b adds 300.
    assert_eq!(r.certificates[1].gross_cumulative, Some(700.0));
    let a = r.certificates[1].lines.iter().find(|l| l.milestone == "a").unwrap();
    assert_eq!((a.previous_pct, a.completion_pct, a.earned_this), (100.0, 100.0, Some(0.0)));
}

#[test]
fn certificates_are_ordered_by_date_not_by_entry() {
    let inp = base(vec![cert("late", "2026-03-31", vec![line("a", 80.0)]), cert("early", "2026-01-31", vec![line("a", 20.0)])]);
    let r = run(&inp);
    assert_eq!(r.certificates[0].id, "early");
    assert_eq!(r.certificates[1].id, "late");
    // 20% of 400 = 80 -> net 72; 80% = 320 -> net 288; the second pays 216.
    assert_eq!(r.certificates[1].payable_now, Some(216.0));
}

#[test]
fn without_a_retention_nothing_is_held_back_and_the_report_says_so() {
    let mut inp = base(vec![cert("c1", "2026-01-31", vec![line("a", 100.0)])]);
    inp.progress.retention_pct = None;
    inp.progress.retention_source = None;
    let r = run(&inp);
    assert_eq!(r.certificates[0].retention_cumulative, Some(0.0));
    assert_eq!(r.certificates[0].payable_now, Some(400.0));
    assert!(r.issues.iter().any(|i| i.contains("no retention")));
}

#[test]
fn an_amount_that_cannot_be_known_is_reported_unknown_not_zero() {
    // No contract value: the percentage milestones have no amount.
    let mut inp = base(vec![cert("c1", "2026-01-31", vec![line("a", 50.0)])]);
    inp.contract_value = None;
    inp.contract_source = None;
    let r = run(&inp);
    let c = &r.certificates[0];
    assert_eq!((c.gross_cumulative, c.net_cumulative, c.payable_now), (None, None, None));
    assert!(c.lines[0].earned_cumulative.is_none());
    assert!(r.issues.iter().any(|i| i.contains("no amount")));
    assert_eq!(r.total_paid_net, None);
}

#[test]
fn a_fixed_amount_milestone_is_priced_without_a_contract_value() {
    let mut inp = base(vec![cert("c1", "2026-01-31", vec![line("fixed", 50.0)])]);
    inp.contract_value = None;
    inp.contract_source = None;
    inp.milestones = vec![Milestone { amount: Some(200.0), pct: None, ..ms("fixed", 1.0) }];
    let r = run(&inp);
    assert_eq!(r.certificates[0].gross_cumulative, Some(100.0));
    assert_eq!(r.certificates[0].payable_now, Some(90.0));
}

#[test]
fn bad_entries_are_refused_by_name() {
    let ok = |c: Vec<Certificate>| base(c).check();
    assert!(ok(vec![cert("c1", "2026-01-31", vec![line("zz", 10.0)])]).unwrap_err().contains("not a payment milestone"));
    assert!(ok(vec![cert("c1", "2026-01-31", vec![line("a", 101.0)])]).unwrap_err().contains("between 0 and 100"));
    assert!(ok(vec![cert("c1", "2026-01-31", vec![line("a", -1.0)])]).is_err());
    assert!(ok(vec![cert("c1", "2026-01-31", vec![line("a", f64::NAN)])]).is_err());
    assert!(ok(vec![cert("c1", "2026-01-31", vec![line("a", 5.0), line("a", 6.0)])]).unwrap_err().contains("twice"));
    assert!(ok(vec![cert("c1", "2026-13-01", vec![])]).unwrap_err().contains("not a date"));
    assert!(ok(vec![cert("c1", "2026-01-31", vec![]), cert("c1", "2026-02-28", vec![])]).unwrap_err().contains("used twice"));
    assert!(ok(vec![cert("", "2026-01-31", vec![])]).is_err());
    let mut no_ref = cert("c1", "2026-01-31", vec![]);
    no_ref.reference = " ".into();
    assert!(ok(vec![no_ref]).unwrap_err().contains("reference"));
    // Completion going down (judged by date, not entry order).
    let down = ok(vec![cert("c2", "2026-02-28", vec![line("a", 30.0)]), cert("c1", "2026-01-31", vec![line("a", 60.0)])]);
    assert!(down.unwrap_err().contains("cannot go down"));
    // Same completion twice is allowed.
    assert!(ok(vec![cert("c1", "2026-01-31", vec![line("a", 60.0)]), cert("c2", "2026-02-28", vec![line("a", 60.0)])]).is_ok());
}

#[test]
fn the_retention_needs_a_valid_percentage_and_its_source() {
    let mut inp = base(vec![]);
    inp.progress.retention_source = None;
    assert!(inp.check().unwrap_err().contains("source"));
    inp.progress.retention_source = Some("clause".into());
    inp.progress.retention_pct = Some(100.5);
    assert!(inp.check().is_err());
    inp.progress.retention_pct = Some(-1.0);
    assert!(inp.check().is_err());
    let ids: BTreeSet<&str> = BTreeSet::new();
    assert!(ProgressInputs::default().check(&ids).is_ok());
}

#[test]
fn the_csv_has_a_total_row_per_certificate() {
    let inp = base(vec![cert("c1", "2026-01-31", vec![line("a", 50.0)])]);
    let csv = run(&inp).csv();
    assert!(csv.starts_with('\u{feff}'));
    assert!(csv.contains("c1,2026-01-31,site report c1,a,50.0000,0.0000,400.00,200.00,200.00"));
    assert!(csv.contains("TOTAL,,,,,,200.00,20.00,180.00,180.00"));
}
