//! Change log with cost impact (tracker item 92). Every value is SYNTHETIC test data.

use nnx_core::boq::{self, BoqLib, ItemData};
use nnx_core::changelog;
use nnx_core::qto::{self, QtoOpts};
use nnx_core::typed::{Column, Wall, element_of};
use nnx_core::*;

fn col(x: i64) -> Element {
    element_of(&Column { x, y: 0, b: 300, h: 300, grid_ref: "A1".into(), axial_service_kn: None, extra: Default::default() }, Source::User)
}

/// A document whose first step is one wall: columns take their storey height from the tallest wall, so without it they have no quantities.
fn base() -> Document {
    let wall = element_of(&Wall { x1: 0, y1: 0, x2: 6000, y2: 0, thickness: 200, height: 3000, length: 6000, is_exterior: true, room_a: "a".into(), room_b: None, extra: Default::default() }, Source::User);
    let mut doc = Document::default();
    doc.commit("base wall", Origin::User, vec![Command::Create(wall)]).unwrap_or_else(|e| panic!("{e}"));
    doc
}

fn lib(priced: bool) -> BoqLib {
    let mut l = BoqLib::default();
    l.project.currency = Some("SAR".into());
    if priced {
        for code in ["COLUMN.VOL", "COLUMN.COUNT", "COLUMN.AREA"] {
            l.items.insert(code.into(), ItemData { spec: None, spec_source: None, rate: Some(100.0), rate_source: Some("synthetic".into()), rate_date: Some("2026-01-01".into()), waste_pct: None });
        }
    }
    l
}

fn price(l: BoqLib) -> impl Fn(&Model) -> Result<boq::BoqReport, String> {
    move |m| Ok(boq::compute(&qto::compute(m, &QtoOpts::default()).items, &l))
}

#[test]
fn log_lists_steps_in_order_with_origin_touched_elements_and_cost_delta() {
    let (a, b) = (col(0), col(4000));
    let (aid, bid) = (a.id, b.id);
    let mut doc = base();
    doc.commit("add column A", Origin::User, vec![Command::Create(a)]).unwrap_or_else(|e| panic!("{e}"));
    doc.commit("generate column B", Origin::Generator, vec![Command::Create(b)]).unwrap_or_else(|e| panic!("{e}"));
    doc.commit("move A", Origin::User, vec![Command::SetProp { id: aid, key: "x".into(), value: Some(serde_json::json!(500)) }]).unwrap_or_else(|e| panic!("{e}"));
    doc.commit("remove B", Origin::User, vec![Command::Delete(bid)]).unwrap_or_else(|e| panic!("{e}"));

    let log = changelog::build(&doc, None, price(lib(true)));
    assert_eq!(log.entries.iter().map(|e| e.seq).collect::<Vec<_>>(), [1, 2, 3, 4, 5]);
    assert_eq!(log.entries.iter().map(|e| e.origin).collect::<Vec<_>>(), ["user", "user", "generator", "user", "user"]);
    assert_eq!(log.entries[1].touched[0].status, "created");
    assert_eq!(log.entries[3].touched[0].status, "modified");
    assert_eq!(log.entries[3].touched[0].props, ["x"]);
    assert_eq!(log.entries[4].touched[0].status, "deleted");
    assert_eq!(log.entries[4].touched[0].kind, "Column");

    let c: Vec<f64> = log.entries.iter().map(|e| e.cost.as_ref().unwrap().delta).collect();
    // One column is priced the same as the next, so create is +d, create is +d, a move is 0, a delete is -d.
    assert!(c[1] > 0.0 && (c[1] - c[2]).abs() < 1e-9, "{c:?}");
    assert!(c[3].abs() < 1e-9, "{c:?}");
    assert!((c[4] + c[1]).abs() < 1e-9, "{c:?}");
    // Net = what the document costs now minus what it cost before the first step (an empty model).
    let net = log.net.as_ref().unwrap();
    let now = price(lib(true))(&doc.model).unwrap().total;
    assert!((net.total_after - now).abs() < 1e-9 && net.total_before == 0.0);
    // The wall quantities have no rate in this library, so the log says its figures are partial.
    assert!(net.partial);
    // The deltas add up to the net.
    assert!((c.iter().sum::<f64>() - net.delta).abs() < 1e-6);
    // A row-level change names the BOQ item.
    assert!(log.entries[1].cost.as_ref().unwrap().rows.iter().any(|r| r.code == "COLUMN.COUNT" && r.quantity_before == 0.0 && r.quantity_after == 1.0));
}

#[test]
fn the_log_agrees_with_undo_and_redo_and_does_not_touch_the_document() {
    let mut doc = base();
    doc.commit("one", Origin::User, vec![Command::Create(col(0))]).unwrap_or_else(|e| panic!("{e}"));
    doc.commit("two", Origin::User, vec![Command::Create(col(3000))]).unwrap_or_else(|e| panic!("{e}"));
    let hash = doc.model.state_hash();
    assert_eq!(changelog::build(&doc, None, price(lib(true))).entries.len(), 3);
    assert_eq!(doc.model.state_hash(), hash);
    assert!(doc.undo());
    let log = changelog::build(&doc, None, price(lib(true)));
    assert_eq!(log.entries.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), ["base wall", "one"]);
    assert!(doc.redo());
    assert_eq!(changelog::build(&doc, None, price(lib(true))).entries.len(), 3);
    assert_eq!(doc.model.state_hash(), hash);
}

#[test]
fn a_limit_keeps_the_newest_steps_and_says_so() {
    let mut doc = base();
    for i in 0..5 {
        doc.commit(&format!("s{i}"), Origin::User, vec![Command::Create(col(i * 2000))]).unwrap_or_else(|e| panic!("{e}"));
    }
    let log = changelog::build(&doc, Some(2), price(lib(true)));
    assert_eq!(log.entries.iter().map(|e| (e.seq, e.label.as_str())).collect::<Vec<_>>(), [(5, "s3"), (6, "s4")]);
    assert!(log.truncated && log.undo_depth == 6);
    // The window's net starts from the model before step 4, not from empty.
    assert!(log.net.as_ref().unwrap().total_before > 0.0);
    assert!(!changelog::build(&doc, None, price(lib(true))).truncated);
}

#[test]
fn without_rates_the_delta_is_partial_and_never_presented_as_complete() {
    let mut doc = base();
    doc.commit("add", Origin::User, vec![Command::Create(col(0))]).unwrap_or_else(|e| panic!("{e}"));
    let log = changelog::build(&doc, None, price(lib(false)));
    let c = log.entries[0].cost.as_ref().unwrap();
    assert!(c.partial && c.delta == 0.0 && c.unpriced_after > 0);
    assert!(log.net.as_ref().unwrap().partial);
    assert!(log.warnings.iter().any(|w| w.contains("PARTIAL")));
}

#[test]
fn a_pricing_failure_is_reported_on_the_step_and_no_number_is_invented() {
    let mut doc = base();
    doc.commit("add", Origin::User, vec![Command::Create(col(0))]).unwrap_or_else(|e| panic!("{e}"));
    let log = changelog::build(&doc, None, |_| Err("no rates entered".to_string()));
    assert!(log.entries[0].cost.is_none());
    assert_eq!(log.entries[0].cost_error.as_deref(), Some("no rates entered"));
    assert!(log.net.is_none());
    assert!(log.csv().contains("no rates entered"));
}

#[test]
fn an_empty_document_has_an_empty_log_and_csv_html_render() {
    let doc = Document::default();
    let log = changelog::build(&doc, None, price(lib(true)));
    assert!(log.entries.is_empty() && log.net.is_none());
    assert!(log.warnings.iter().any(|w| w.contains("no committed steps")));
    assert_eq!(log.csv().lines().count(), 1);
    let mut doc = base();
    doc.commit("a \"quoted\" <step>", Origin::User, vec![Command::Create(col(0))]).unwrap_or_else(|e| panic!("{e}"));
    let log = changelog::build(&doc, None, price(lib(true)));
    assert!(log.csv().contains("\"a \"\"quoted\"\" <step>\""));
    assert!(log.html().contains("a &quot;quoted&quot; &lt;step&gt;"));
}
