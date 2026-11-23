//! BOQ rate dates: every rate carries the date the user quoted it. Synthetic numbers, checkable by hand.

use nnx_core::boq::{self, BoqLib, ItemData, MatData, MatFactor};
use nnx_core::qto::{QtoItem, Unit};

fn item(code: &str, unit: Unit, q: f64) -> QtoItem {
    QtoItem { code: code.into(), description: code.into(), unit, quantity: q, waste_pct: None, sources: vec![], parts: vec![] }
}

fn data(rate: Option<f64>, date: Option<&str>) -> ItemData {
    ItemData { spec: Some("spec".into()), spec_source: Some("synthetic".into()), rate, rate_source: Some("synthetic".into()), rate_date: date.map(Into::into), waste_pct: None }
}

fn lib() -> BoqLib {
    let mut l = BoqLib::default();
    l.project.currency = Some("SAR".into());
    l.items.insert("SLAB.VOL".into(), data(Some(10.0), Some("2026-03-15")));
    l.items.insert("COLUMN.VOL".into(), data(Some(20.0), Some("2026-11-02")));
    l.items.insert("BEAM.VOL".into(), data(Some(30.0), None));
    l
}

fn items() -> Vec<QtoItem> {
    vec![item("SLAB.VOL", Unit::M3, 2.0), item("COLUMN.VOL", Unit::M3, 1.0), item("BEAM.VOL", Unit::M3, 1.0)]
}

#[test]
fn each_row_carries_its_rate_date_and_the_report_gives_the_range() {
    let r = boq::compute(&items(), &lib());
    let row = |c: &str| r.rows.iter().find(|x| x.code == c).unwrap();
    assert_eq!(row("SLAB.VOL").rate_date.as_deref(), Some("2026-03-15"));
    assert_eq!(row("COLUMN.VOL").rate_date.as_deref(), Some("2026-11-02"));
    assert_eq!(row("BEAM.VOL").rate_date, None);
    let d = r.rate_dates.expect("two dated rates");
    assert_eq!((d.oldest.as_str(), d.newest.as_str(), d.dated), ("2026-03-15", "2026-11-02", 2));
    // 2 x 10 + 1 x 20 + 1 x 30 = 70: dates do not change money.
    assert!((r.subtotal - 70.0).abs() < 1e-9);
}

#[test]
fn an_undated_rate_is_named_and_warned_never_defaulted() {
    let r = boq::compute(&items(), &lib());
    assert_eq!(r.missing.no_date, vec!["rate of BEAM.VOL".to_string()]);
    assert!(r.warnings.iter().any(|w| w.contains("no date") && w.contains("BEAM.VOL")));
    let none = boq::compute(&items(), &BoqLib::default());
    assert!(none.rate_dates.is_none() && none.missing.no_date.is_empty());
}

#[test]
fn material_rates_carry_dates_and_undated_ones_are_named() {
    let mut l = lib();
    l.materials.insert("SLAB.VOL".into(), vec![MatFactor { material: "Cement".into(), unit: "bag".into(), per_unit: 7.0, source: Some("synthetic".into()) }]);
    l.material_data.insert("Cement".into(), MatData { spec: None, spec_source: None, rate: Some(2.0), rate_source: Some("synthetic".into()), rate_date: Some("2026-10-01".into()) });
    let r = boq::compute(&items(), &l);
    assert_eq!(r.materials[0].rate_date.as_deref(), Some("2026-10-01"));
    assert!(r.materials_csv().contains("2026-10-01"));
    l.material_data.get_mut("Cement").unwrap().rate_date = None;
    let r = boq::compute(&items(), &l);
    assert!(r.missing.no_date.contains(&"rate of material Cement".to_string()));
}

#[test]
fn the_date_reaches_the_csv_and_the_html() {
    let r = boq::compute(&items(), &lib());
    let csv = r.boq_csv();
    assert!(csv.lines().next().unwrap().contains("rate,rate_date,amount"));
    let slab = csv.lines().find(|l| l.contains("SLAB.VOL")).unwrap();
    assert!(slab.contains(",10.00,2026-03-15,20.00,SAR,priced"), "{slab}");
    assert!(r.html(false).contains("<small>2026-03-15</small>"));
}

#[test]
fn bad_dates_are_rejected_by_name() {
    assert!(boq::validate(&lib()).is_ok());
    for bad in ["15/03/2026", "2026-13-01", "2026-02-29", "2026-3-5", "2026-04-31", "abc", "", "1800-01-01"] {
        let mut l = lib();
        l.items.insert("SLAB.VOL".into(), data(Some(10.0), Some(bad)));
        let e = boq::validate(&l).unwrap_err();
        assert!(e.contains("SLAB.VOL") && e.contains("YYYY-MM-DD"), "{bad}: {e}");
    }
    let mut l = lib();
    l.items.insert("SLAB.VOL".into(), data(Some(10.0), Some("2028-02-29")));
    assert!(boq::validate(&l).is_ok(), "2028 is a leap year");
    let mut l = lib();
    l.items.insert("SLAB.VOL".into(), data(None, Some("2026-03-15")));
    assert!(boq::validate(&l).unwrap_err().contains("no rate"));
    let mut l = lib();
    l.material_data.insert("Cement".into(), MatData { rate: Some(1.0), rate_date: Some("bad".into()), ..Default::default() });
    assert!(boq::validate(&l).unwrap_err().contains("material 'Cement'"));
}
