//! Design decision log (tracker item 98). Every value is SYNTHETIC test data.

use nnx_core::decisions::{Decision, DecisionLog, report, validate_links};
use nnx_core::typed::Column;
use nnx_core::typed::element_of;
use nnx_core::*;

fn model() -> (Model, String) {
    let col = element_of(&Column { x: 1000, y: 0, b: 300, h: 300, grid_ref: "A1".into(), axial_service_kn: None, extra: Default::default() }, Source::User);
    let id = col.id.to_string();
    let mut doc = Document::default();
    doc.commit("m", Origin::User, vec![Command::Create(col)]).unwrap_or_else(|e| panic!("{e}"));
    (doc.model, id)
}

fn dec(id: &str, date: &str, by: &str) -> Decision {
    Decision { id: id.into(), date: date.into(), title: format!("title {id}"), decision: format!("text {id}"), decided_by: by.into(), ..Default::default() }
}

#[test]
fn an_entry_without_author_date_title_or_text_is_refused_by_name() {
    let ok = dec("a", "2026-01-05", "Owner");
    for (bad, msg) in [
        (Decision { id: " ".into(), ..ok.clone() }, "needs an id"),
        (Decision { decided_by: "  ".into(), ..ok.clone() }, "who made it"),
        (Decision { title: "".into(), ..ok.clone() }, "needs a title"),
        (Decision { decision: "".into(), ..ok.clone() }, "what was decided"),
        (Decision { date: "2026-13-01".into(), ..ok.clone() }, "not a date"),
        (Decision { date: "2026-02-29".into(), ..ok.clone() }, "not a date"),
        (Decision { elements: vec!["x".into(), "x".into()], ..ok.clone() }, "twice"),
        (Decision { elements: vec![" ".into()], ..ok.clone() }, "empty element"),
    ] {
        let e = DecisionLog { decisions: vec![bad] }.check().unwrap_err();
        assert!(e.contains(msg), "{e}");
    }
    let dup = DecisionLog { decisions: vec![ok.clone(), ok.clone()] }.check().unwrap_err();
    assert!(dup.contains("used twice"), "{dup}");
    assert!(DecisionLog { decisions: vec![ok] }.check().is_ok());
    assert!(DecisionLog::default().check().is_ok());
}

#[test]
fn supersede_links_must_point_at_an_earlier_distinct_entry_without_loops_or_forks() {
    let a = dec("a", "2026-01-05", "Owner");
    let sup = |id: &str, date: &str, t: &str| Decision { supersedes: Some(t.into()), ..dec(id, date, "Architect") };
    let chk = |v: Vec<Decision>| DecisionLog { decisions: v }.check();
    assert!(chk(vec![a.clone(), sup("b", "2026-02-01", "a")]).is_ok());
    // Same day is allowed.
    assert!(chk(vec![a.clone(), sup("b", "2026-01-05", "a")]).is_ok());
    assert!(chk(vec![a.clone(), sup("b", "2026-02-01", "zz")]).unwrap_err().contains("not in the log"));
    assert!(chk(vec![sup("a", "2026-01-05", "a")]).unwrap_err().contains("itself"));
    assert!(chk(vec![a.clone(), sup("b", "2026-01-01", "a")]).unwrap_err().contains("dated later"));
    let fork = chk(vec![a.clone(), sup("b", "2026-02-01", "a"), sup("c", "2026-03-01", "a")]).unwrap_err();
    assert!(fork.contains("both"), "{fork}");
    // A two-entry ring on the same day: a replaces b and b replaces a.
    let ring = chk(vec![sup("a", "2026-01-05", "b"), sup("b", "2026-01-05", "a")]).unwrap_err();
    assert!(ring.contains("loop"), "{ring}");
    // A longer chain is fine.
    assert!(chk(vec![a, sup("b", "2026-02-01", "a"), sup("c", "2026-03-01", "b")]).is_ok());
}

#[test]
fn links_must_match_an_element_and_a_deleted_element_is_reported_not_dropped() {
    let (m, id) = model();
    let mut d = dec("a", "2026-01-05", "Owner");
    d.elements = vec!["COL-1".into(), id.clone()];
    let log = DecisionLog { decisions: vec![d.clone()] };
    assert!(validate_links(&m, &log).is_ok());
    let rep = report(&m, &log);
    assert_eq!(rep.decisions[0].element_ids, vec![id]);
    assert!(rep.decisions[0].unresolved.is_empty());
    let mut bad = d.clone();
    bad.elements = vec!["NOPE-9".into()];
    assert!(validate_links(&m, &DecisionLog { decisions: vec![bad.clone()] }).unwrap_err().contains("NOPE-9"));
    // The element is gone from the model afterwards: the reference stays, flagged.
    let rep = report(&Model::default(), &DecisionLog { decisions: vec![bad] });
    assert_eq!(rep.decisions[0].unresolved, vec!["NOPE-9".to_string()]);
    assert!(rep.notes.iter().any(|n| n.contains("NOPE-9")));
}

#[test]
fn the_report_orders_by_date_marks_superseded_entries_and_counts_per_decider() {
    let (m, _) = model();
    let log = DecisionLog {
        decisions: vec![
            dec("c", "2026-03-01", "Owner"),
            Decision { supersedes: Some("a".into()), ..dec("b", "2026-02-01", "Architect") },
            dec("a", "2026-01-05", "Owner"),
        ],
    };
    log.check().unwrap();
    let r = report(&m, &log);
    assert_eq!(r.decisions.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
    assert_eq!((r.total, r.active, r.superseded), (3, 2, 1));
    assert_eq!(r.decisions[0].status, "superseded");
    assert_eq!(r.decisions[0].superseded_by.as_deref(), Some("b"));
    assert_eq!(r.decisions[1].status, "active");
    assert_eq!(r.first_date.as_deref(), Some("2026-01-05"));
    assert_eq!(r.last_date.as_deref(), Some("2026-03-01"));
    let owner = r.by_decider.iter().find(|c| c.decided_by == "Owner").unwrap();
    assert_eq!((owner.total, owner.active), (2, 1));
    let arch = r.by_decider.iter().find(|c| c.decided_by == "Architect").unwrap();
    assert_eq!((arch.total, arch.active), (1, 1));
    assert!(report(&m, &DecisionLog::default()).notes.iter().any(|n| n.contains("no design decisions")));
}

#[test]
fn csv_quotes_commas_and_html_escapes_markup() {
    let (m, _) = model();
    let mut d = dec("a", "2026-01-05", "Owner, Ltd");
    d.title = "Use <b>RC</b>".into();
    d.decision = "Slab \"S1\", 200 mm".into();
    let r = report(&m, &DecisionLog { decisions: vec![d] });
    let csv = r.csv();
    assert_eq!(csv.lines().count(), 2);
    assert!(csv.contains("\"Owner, Ltd\""));
    assert!(csv.contains("\"Slab \"\"S1\"\", 200 mm\""));
    let html = r.html();
    assert!(html.contains("&lt;b&gt;RC&lt;/b&gt;"));
    assert!(!html.contains("<b>RC</b>"));
}
