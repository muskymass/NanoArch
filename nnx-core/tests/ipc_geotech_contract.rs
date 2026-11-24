use nnx_core::contract::*;
use nnx_core::geotech::*;
use nnx_core::ipc::*;

fn boq() -> Vec<ContractLine> {
    vec![
        ContractLine { code: "C1".into(), description: "Concrete".into(), unit: "m3".into(), quantity: 100.0, rate: Some(10.0) },
        ContractLine { code: "S1".into(), description: "Steel".into(), unit: "t".into(), quantity: 5.0, rate: None },
    ]
}

fn cert(id: &str, date: &str, c1: f64) -> Certificate {
    Certificate { id: id.into(), date: date.into(), lines: vec![QtyLine { code: "C1".into(), cum_qty: c1 }], ..Default::default() }
}

#[test]
fn ipc_certifies_the_difference_with_retention_and_vat_on_the_period() {
    let inp = IpcInputs { currency: Some("SAR".into()), retention_pct: Some(10.0), vat_pct: Some(15.0), certificates: vec![cert("1", "2026-11-01", 40.0), cert("2", "2026-12-01", 70.0)] };
    inp.check().unwrap();
    let r = compute(&inp, &boq(), &[]);
    let (a, b) = (&r.certificates[0], &r.certificates[1]);
    assert_eq!((a.cum_value, a.retention, a.net_before_vat, a.vat, a.payable), (400.0, Some(40.0), Some(360.0), Some(54.0), Some(414.0)));
    // Cumulative 700, retention 70: certified net 630; previous net 360, so this period nets 270.
    assert_eq!((b.cum_value, b.retention, b.net_before_vat, b.payable), (700.0, Some(70.0), Some(270.0), Some(310.5)));
    assert_eq!(b.period_value, 300.0);
    assert!(b.lines[0].period_qty == 30.0 && b.lines[0].previous_qty == 40.0);
    assert!(a.complete);
}

#[test]
fn ipc_never_values_an_unpriced_item_and_never_nets_without_a_retention_percentage() {
    let mut c = cert("1", "2026-11-01", 10.0);
    c.lines.push(QtyLine { code: "S1".into(), cum_qty: 1.0 });
    let inp = IpcInputs { certificates: vec![c], ..Default::default() };
    let r = compute(&inp, &boq(), &[]);
    assert!(!r.certificates[0].complete);
    assert_eq!(r.certificates[0].cum_value, 100.0);
    assert_eq!(r.certificates[0].payable, None);
    assert!(r.issues.iter().any(|i| i.contains("retention percentage not entered")));
    assert!(r.issues.iter().any(|i| i.contains("'S1' has no rate")));
}

#[test]
fn ipc_rejects_a_quantity_that_goes_down_and_flags_over_certification() {
    let down = IpcInputs { certificates: vec![cert("1", "2026-11-01", 40.0), cert("2", "2026-12-01", 30.0)], ..Default::default() };
    assert!(down.check().unwrap_err().contains("cannot decrease"));
    let early = IpcInputs { certificates: vec![cert("1", "2026-12-01", 40.0), cert("2", "2026-11-01", 50.0)], ..Default::default() };
    assert!(early.check().unwrap_err().contains("before the previous"));
    let over = IpcInputs { retention_pct: Some(5.0), certificates: vec![cert("1", "2026-11-01", 120.0)], ..Default::default() };
    over.check().unwrap();
    assert!(compute(&over, &boq(), &[]).issues.iter().any(|i| i.contains("beyond the BOQ quantity")));
}

#[test]
fn ipc_certifies_only_approved_variations_by_percentage() {
    let mut c = cert("1", "2026-11-01", 10.0);
    c.variations = vec![VoLine { id: "VO-1".into(), cum_pct: 50.0 }, VoLine { id: "VO-9".into(), cum_pct: 100.0 }];
    c.deductions = vec![Deduction { label: "advance recovery".into(), amount: 20.0 }];
    let inp = IpcInputs { retention_pct: Some(0.0), certificates: vec![c], ..Default::default() };
    let r = compute(&inp, &boq(), &[ApprovedVo { id: "VO-1".into(), cost: 200.0 }]);
    assert_eq!(r.certificates[0].cum_value, 200.0);
    assert_eq!(r.certificates[0].net_before_vat, Some(180.0));
    assert!(r.issues.iter().any(|i| i.contains("'VO-9' is not approved")));
}

#[test]
fn ipc_workbook_has_live_formulas_and_html_renders() {
    let inp = IpcInputs { currency: Some("SAR".into()), retention_pct: Some(10.0), vat_pct: Some(15.0), certificates: vec![cert("1", "2026-11-01", 40.0)] };
    let r = compute(&inp, &boq(), &[]);
    let wb = r.workbook(false);
    assert_eq!(wb.sheets.len(), 1);
    assert!(wb.to_bytes().starts_with(b"PK"));
    assert!(r.html("Villa", false).contains("Net payable"));
    assert!(r.html("فيلا", true).contains("dir=\"rtl\""));
    assert_eq!(compute(&IpcInputs::default(), &boq(), &[]).workbook(true).sheets.len(), 1);
}

#[test]
fn geotech_requires_the_report_reference_and_ordered_layers() {
    let layer = |a: f64, b: f64| SoilLayer { from_m: a, to_m: b, description: "sand".into(), notes: None };
    let g = GeotechInputs { bearing_kpa: Some(150.0), ..Default::default() };
    assert!(g.check().unwrap_err().contains("report reference"));
    let base = GeotechInputs { report_ref: Some("GR-1".into()), report_date: Some("2026-05-01".into()), ..Default::default() };
    GeotechInputs { bearing_kpa: Some(150.0), layers: vec![layer(0.0, 2.0), layer(2.0, 6.0)], ..base.clone() }.check().unwrap();
    assert!(GeotechInputs { layers: vec![layer(0.0, 3.0), layer(2.0, 6.0)], ..base.clone() }.check().unwrap_err().contains("overlaps"));
    assert!(GeotechInputs { layers: vec![layer(2.0, 1.0)], ..base.clone() }.check().unwrap_err().contains("0 <= from < to"));
    assert!(GeotechInputs { bearing_kpa: Some(0.0), ..base }.check().is_err());
}

#[test]
fn geotech_reports_missing_figures_gaps_and_a_conflict_with_the_structural_input() {
    let g = GeotechInputs { report_ref: Some("GR-1".into()), report_date: Some("2026-05-01".into()), bearing_kpa: Some(150.0), layers: vec![SoilLayer { from_m: 0.0, to_m: 1.0, description: "fill".into(), notes: None }, SoilLayer { from_m: 2.0, to_m: 5.0, description: "sand".into(), notes: None }], ..Default::default() };
    let r = report(&g, Some(200.0), false);
    assert!(r.bearing_conflict);
    for needle in ["groundwater depth not entered", "gap between 1 m and 2 m", "conflict", "recommends is not entered"] {
        assert!(r.issues.iter().any(|i| i.contains(needle)), "{needle}: {:?}", r.issues);
    }
    assert!(!report(&g, Some(150.0), false).bearing_conflict);
    assert!(report(&g, None, false).issues.iter().any(|i| i.contains("not in the structural inputs")));
    assert!(report(&GeotechInputs::default(), None, true).issues[0].contains("q_all"));
    assert!(r.html("Villa", false).contains("150") && r.workbook(false).sheets.len() == 2);
}

#[test]
fn contract_draft_states_only_entered_terms_and_lists_what_is_missing() {
    let inp = ContractInputs { parties: vec![Party { role: "Owner".into(), name: "A".into(), ..Default::default() }, Party { role: "Contractor".into(), name: "B".into(), ..Default::default() }], contract_value: Some(1_000_000.0), currency: Some("SAR".into()), retention_pct: Some(10.0), clauses: vec![Clause { title: "Scope".into(), text: "As in the BOQ.\nSecond line".into() }], ..Default::default() };
    inp.check().unwrap();
    let ms = vec![MilestoneLine { name: "Foundations".into(), when: Some("2026-12-01".into()), percent: Some(20.0), amount: Some(200_000.0) }];
    let d = draft("Villa", &inp, &ms, false);
    assert!(d.html.contains("DRAFT") && d.html.contains("1000000.00 SAR") && d.html.contains("Foundations") && d.html.contains("Second line"));
    for m in ["Start date", "Duration", "Delay penalty", "Warranty", "Governing law"] {
        assert!(d.missing.iter().any(|x| x == m), "{m}");
    }
    assert!(!d.missing.iter().any(|x| x == "Contract value" || x == "Retention" || x == "Currency"));
    assert!(draft("فيلا", &inp, &[], true).html.contains("مسودة"));
    assert!(ContractInputs { retention_pct: Some(120.0), ..Default::default() }.check().is_err());
    assert!(ContractInputs { parties: vec![Party::default()], ..Default::default() }.check().is_err());
}
