//! Contractor bid comparison. Synthetic numbers, checkable by hand.

use nnx_core::bids::{Bid, BidMarkup, compare};
use nnx_core::boq::{self, BoqLib, ItemData};
use nnx_core::qto::{QtoItem, Unit};

fn item(code: &str, unit: Unit, q: f64) -> QtoItem {
    QtoItem { code: code.into(), description: code.into(), unit, quantity: q, waste_pct: None, sources: vec![], parts: vec![] }
}

fn report(currency: Option<&str>) -> boq::BoqReport {
    let mut l = BoqLib::default();
    l.project.currency = currency.map(Into::into);
    l.items.insert("SLAB.VOL".into(), ItemData { rate: Some(9.0), rate_source: Some("own".into()), ..Default::default() });
    boq::compute(&[item("SLAB.VOL", Unit::M3, 2.0), item("COLUMN.VOL", Unit::M3, 1.0), item("BEAM.VOL", Unit::M3, 4.0)], &l)
}

fn bid(name: &str, rates: &[(&str, f64)], markups: &[(&str, f64)]) -> Bid {
    Bid {
        contractor: name.into(),
        source: format!("{name} quotation"),
        date: "2026-11-01".into(),
        currency: "SAR".into(),
        rates: rates.iter().map(|(c, r)| (c.to_string(), *r)).collect(),
        markups: markups.iter().map(|(n, p)| BidMarkup { name: n.to_string(), pct: *p }).collect(),
    }
}

fn full(name: &str, s: f64, c: f64, b: f64, mk: &[(&str, f64)]) -> Bid {
    bid(name, &[("SLAB.VOL", s), ("COLUMN.VOL", c), ("BEAM.VOL", b)], mk)
}

#[test]
fn totals_ranks_and_gaps_match_a_hand_calculation() {
    let r = report(Some("SAR"));
    // A: 2x10 + 1x20 + 4x30 = 160. B: 2x11 + 1x18 + 4x28 = 152, +10% = 167.2. C ties A at 160.
    let cmp = compare(&r, &[full("A", 10.0, 20.0, 30.0, &[]), full("B", 11.0, 18.0, 28.0, &[("Profit", 10.0)]), full("C", 10.0, 20.0, 30.0, &[])]).unwrap();
    let t = |n: &str| cmp.totals.iter().find(|t| t.contractor == n).unwrap();
    assert_eq!(t("B").subtotal, 152.0);
    assert_eq!(t("B").total, 167.2);
    assert_eq!((t("A").rank, t("C").rank, t("B").rank), (Some(1), Some(1), Some(3)));
    assert_eq!(t("B").above_lowest, Some(7.2));
    assert_eq!(t("B").above_lowest_pct, Some(4.5));
    assert_eq!(cmp.lowest_complete.as_deref(), Some("A"));
    let col = cmp.lines.iter().find(|l| l.code == "COLUMN.VOL").unwrap();
    assert_eq!(col.lowest, vec!["B".to_string()]);
    assert_eq!(col.spread, Some(2.0));
    let slab = cmp.lines.iter().find(|l| l.code == "SLAB.VOL").unwrap();
    assert_eq!(slab.own_rate, Some(9.0));
    assert_eq!(slab.lowest, vec!["A".to_string(), "C".to_string()]);
}

#[test]
fn a_bidder_with_unpriced_items_is_not_ranked_and_nothing_is_filled_in() {
    let r = report(None);
    let cmp = compare(&r, &[full("A", 10.0, 20.0, 30.0, &[]), bid("B", &[("SLAB.VOL", 1.0), ("COLUMN.VOL", 1.0)], &[])]).unwrap();
    let b = cmp.totals.iter().find(|t| t.contractor == "B").unwrap();
    assert!(!b.complete);
    assert_eq!(b.unpriced, vec!["BEAM.VOL".to_string()]);
    assert_eq!(b.rank, None);
    assert_eq!(b.subtotal, 3.0);
    assert_eq!(cmp.common_lines, 2);
    // Like for like on the two common lines: A = 2x10 + 20 = 40, B = 2 + 1 = 3.
    assert_eq!(cmp.totals[0].common_subtotal, 40.0);
    assert_eq!(b.common_subtotal, 3.0);
    assert_eq!(cmp.lowest_complete.as_deref(), Some("A"));
    assert!(cmp.notes.iter().any(|n| n.contains("Not ranked") && n.contains('B')));
}

#[test]
fn nobody_complete_means_no_ranking() {
    let r = report(None);
    let cmp = compare(&r, &[bid("A", &[("SLAB.VOL", 1.0)], &[]), bid("B", &[("COLUMN.VOL", 1.0)], &[])]).unwrap();
    assert_eq!(cmp.lowest_complete, None);
    assert_eq!(cmp.priced_by_nobody, vec!["BEAM.VOL".to_string()]);
    assert!(cmp.totals.iter().all(|t| t.rank.is_none()));
}

#[test]
fn unsound_input_is_rejected_by_name() {
    let r = report(Some("SAR"));
    let ok = || full("A", 1.0, 1.0, 1.0, &[]);
    let two = |b: Bid| compare(&r, &[ok(), b]).map(|_| ());
    assert!(compare(&r, &[ok()]).unwrap_err().contains("at least two"));
    assert!(two(bid("", &[("SLAB.VOL", 1.0)], &[])).unwrap_err().contains("contractor name"));
    assert!(two(full("a", 1.0, 1.0, 1.0, &[])).unwrap_err().contains("twice"));
    assert!(two(bid("B", &[("NOPE.VOL", 1.0)], &[])).unwrap_err().contains("NOPE.VOL"));
    assert!(two(bid("B", &[("SLAB.VOL", 0.0)], &[])).unwrap_err().contains("above zero"));
    assert!(two(bid("B", &[("SLAB.VOL", f64::NAN)], &[])).unwrap_err().contains("above zero"));
    assert!(two(bid("B", &[], &[])).unwrap_err().contains("no rates"));
    let mut b = bid("B", &[("SLAB.VOL", 1.0)], &[]);
    b.date = "2026-02-30".into();
    assert!(two(b).unwrap_err().contains("YYYY-MM-DD"));
    let mut b = bid("B", &[("SLAB.VOL", 1.0)], &[]);
    b.source = " ".into();
    assert!(two(b).unwrap_err().contains("source"));
    let mut b = bid("B", &[("SLAB.VOL", 1.0)], &[]);
    b.currency = "USD".into();
    assert!(two(b).unwrap_err().contains("currency"));
    assert!(two(bid("B", &[("SLAB.VOL", 1.0)], &[("X", -100.0)])).unwrap_err().contains("above -100"));
    assert!(two(bid("B", &[("SLAB.VOL", 1.0)], &[("X", 1.0), ("x", 2.0)])).unwrap_err().contains("twice"));
    // Currency must match the BOQ's when the BOQ has one.
    let other = report(Some("EUR"));
    assert!(compare(&other, &[ok(), full("B", 1.0, 1.0, 1.0, &[])]).map(|_| ()).unwrap_err().contains("BOQ currency"));
    // A BOQ with nothing measured has nothing to price.
    let empty = boq::compute(&[], &BoqLib::default());
    assert!(compare(&empty, &[ok(), full("B", 1.0, 1.0, 1.0, &[])]).map(|_| ()).unwrap_err().contains("no measured items"));
}

#[test]
fn csv_has_one_row_per_item_then_the_totals_and_escapes_names() {
    let r = report(None);
    let cmp = compare(&r, &[full("A, Ltd", 10.0, 20.0, 30.0, &[]), full("B", 11.0, 18.0, 28.0, &[])]).unwrap();
    let csv = cmp.csv();
    assert!(csv.contains("\"A, Ltd rate\""));
    assert!(csv.contains("\ncontractor,source,date,priced_lines"));
    assert!(csv.contains("152.00"));
}
