//! BOQ engine. Every number here is a SYNTHETIC test value chosen so the result can be checked by hand.

use nnx_core::boq::{self, BoqLib};
use nnx_core::qto::{QtoItem, Unit};

fn item(code: &str, unit: Unit, q: f64) -> QtoItem {
    QtoItem { code: code.into(), description: format!("desc {code}"), unit, quantity: q, waste_pct: None, sources: Vec::new(), parts: Vec::new() }
}

fn items() -> Vec<QtoItem> {
    vec![
        item("SLAB.VOL", Unit::M3, 3.0),
        item("COLUMN.VOL", Unit::M3, 0.5),
        item("FLOOR.AREA.bedroom", Unit::M2, 20.0),
        item("WALL.EXT.AREA", Unit::M2, 8.31),
        item("WALL.EXT.VOL", Unit::M3, 1.662),
        item("DOOR.900x2100", Unit::Nr, 1.0),
        item("SPACE.GROSS.AREA", Unit::M2, 20.0),
    ]
}

fn lib() -> BoqLib {
    serde_json::from_value(serde_json::json!({
        "project": {"name": "Test <villa>", "currency": "XXX"},
        "items": {
            "SLAB.VOL": {"spec": "spec S", "spec_source": "src", "rate": 100, "rate_source": "src", "waste_pct": 5},
            "COLUMN.VOL": {"rate": 200},
            "FLOOR.AREA.*": {"spec": "floor spec", "spec_source": "src", "rate": 10, "rate_source": "src"},
            "WALL.EXT.*": {"rate": 5}
        },
        "materials": {
            "SLAB.*": [{"material": "cement", "unit": "bag", "per_unit": 7, "source": "src"}, {"material": "steel", "unit": "kg", "per_unit": 100, "source": "src"}],
            "COLUMN.*": [{"material": "cement", "unit": "bag", "per_unit": 7}, {"material": "steel", "unit": "kg", "per_unit": 100}]
        },
        "material_data": {"cement": {"rate": 2, "rate_source": "src", "spec": "cement spec"}, "steel": {"rate": 3}},
        "markups": [{"name": "Overheads", "pct": 10}, {"name": "VAT", "pct": 15, "source": "src"}]
    }))
    .unwrap()
}

fn near(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "{a} vs {b}");
}

#[test]
fn amounts_totals_and_markups_by_hand() {
    let r = boq::compute(&items(), &lib());
    let amount = |c: &str| r.rows.iter().find(|x| x.code == c).unwrap().amount;
    assert_eq!(amount("SLAB.VOL"), Some(300.0)); // 3 x 100, net quantity
    assert_eq!(amount("COLUMN.VOL"), Some(100.0)); // 0.5 x 200
    assert_eq!(amount("FLOOR.AREA.bedroom"), Some(200.0)); // prefix rule, 20 x 10
    assert_eq!(amount("WALL.EXT.AREA"), Some(41.55)); // 8.31 x 5
    assert_eq!(amount("WALL.EXT.VOL"), Some(8.31)); // 1.662 x 5
    assert_eq!(amount("DOOR.900x2100"), None);
    near(r.subtotal, 649.86);
    // overheads 10 % of 649.86 = 64.99 (rounded) -> 714.85; VAT 15 % of that = 107.23 -> 822.08
    assert_eq!((r.markups[0].amount, r.markups[1].amount), (64.99, 107.23));
    near(r.total, 822.08);
    assert_eq!((r.priced, r.unpriced, r.complete), (5, 1, false));
    // the reference quantity is shown but never priced or counted
    let reference = r.rows.iter().find(|x| x.code == "SPACE.GROSS.AREA").unwrap();
    assert!(reference.reference_only && reference.amount.is_none());
    assert!(r.warnings.iter().any(|w| w.contains("PARTIAL")));
    assert!(r.warnings.iter().any(|w| w.contains("WALL.EXT.AREA") && w.contains("WALL.EXT.VOL") && w.contains("twice")));
}

#[test]
fn waste_drives_the_order_quantity_and_the_materials_not_the_amount() {
    let r = boq::compute(&items(), &lib());
    let slab = r.rows.iter().find(|x| x.code == "SLAB.VOL").unwrap();
    near(slab.quantity, 3.0);
    near(slab.ordered, 3.15);
    let m = |name: &str| r.materials.iter().find(|x| x.material == name).unwrap();
    // cement: 3.15 x 7 + 0.5 x 7 = 25.55 bags; steel: 3.15 x 100 + 0.5 x 100 = 365 kg
    near(m("cement").quantity, 25.55);
    near(m("steel").quantity, 365.0);
    assert_eq!(m("cement").used_in, vec!["COLUMN.VOL".to_string(), "SLAB.VOL".to_string()]);
    near(m("cement").cost.unwrap(), 51.1); // 25.55 x 2
    assert_eq!(m("cement").spec.as_deref(), Some("cement spec"));
    near(r.material_cost.unwrap(), 51.1 + 1095.0); // steel 365 x 3
}

#[test]
fn what_is_missing_is_named_and_nothing_is_invented() {
    let r = boq::compute(&items(), &lib());
    assert_eq!(r.missing.no_rate, vec!["DOOR.900x2100".to_string()]);
    for c in ["COLUMN.VOL", "WALL.EXT.AREA", "WALL.EXT.VOL", "DOOR.900x2100"] {
        assert!(r.missing.no_spec.contains(&c.to_string()), "{c}");
    }
    assert!(!r.missing.no_spec.contains(&"SLAB.VOL".to_string()));
    for c in ["FLOOR.AREA.bedroom", "WALL.EXT.AREA", "WALL.EXT.VOL", "DOOR.900x2100"] {
        assert!(r.missing.no_material_factors.contains(&c.to_string()), "{c}");
    }
    let src = r.missing.no_source.join(" | ");
    for s in ["rate of COLUMN.VOL", "rate of WALL.EXT.AREA", "mark-up 'Overheads'", "consumption of cement in COLUMN.VOL"] {
        assert!(src.contains(s), "{s} in {src}");
    }
    assert!(!src.contains("VAT") && !src.contains("rate of SLAB.VOL"));
    // an empty library prices nothing, says so, and reports no total as complete
    let empty = boq::compute(&items(), &BoqLib::default());
    assert_eq!((empty.priced, empty.complete, empty.total), (0, false, 0.0));
    assert!(empty.materials.is_empty() && empty.material_cost.is_none());
    assert_eq!(empty.missing.no_rate.len(), 6);
}

#[test]
fn the_exact_code_beats_the_prefix_and_the_longer_prefix_beats_the_shorter() {
    let l: BoqLib = serde_json::from_value(serde_json::json!({"items": {
        "FLOOR.*": {"rate": 1}, "FLOOR.AREA.*": {"rate": 2}, "FLOOR.AREA.bedroom": {"rate": 3}}}))
    .unwrap();
    let it = vec![item("FLOOR.AREA.bedroom", Unit::M2, 1.0), item("FLOOR.AREA.hall", Unit::M2, 1.0), item("FLOOR.OTHER", Unit::M2, 1.0)];
    let r = boq::compute(&it, &l);
    let rate = |c: &str| r.rows.iter().find(|x| x.code == c).unwrap().rate;
    assert_eq!((rate("FLOOR.AREA.bedroom"), rate("FLOOR.AREA.hall"), rate("FLOOR.OTHER")), (Some(3.0), Some(2.0), Some(1.0)));
}

#[test]
fn rows_are_numbered_by_section_and_complete_when_every_item_is_priced() {
    let l: BoqLib = serde_json::from_value(serde_json::json!({"items": {"*": {"rate": 1}}})).unwrap();
    let r = boq::compute(&items(), &l);
    assert!(r.complete && r.unpriced == 0);
    let no = |c: &str| r.rows.iter().find(|x| x.code == c).unwrap().no.clone();
    assert_eq!(no("SPACE.GROSS.AREA"), "00.01");
    assert_eq!(no("COLUMN.VOL"), "02.01");
    assert_eq!(no("SLAB.VOL"), "02.02");
    assert_eq!(no("WALL.EXT.AREA"), "03.01");
    assert_eq!(no("DOOR.900x2100"), "04.01");
    assert_eq!(no("FLOOR.AREA.bedroom"), "05.01");
    assert!(r.sections.iter().all(|s| s.priced == s.items));
}

#[test]
fn exports_are_safe_and_say_when_partial() {
    let r = boq::compute(&items(), &lib());
    let csv = r.boq_csv();
    assert!(csv.starts_with('\u{feff}') && csv.contains("PARTIAL") && csv.contains("822.08"));
    assert!(r.materials_csv().contains("cement,bag,25.550"));
    assert!(r.specs_csv().contains("SLAB.VOL"));
    let ar = r.html(true);
    assert!(ar.contains("dir=\"rtl\"") && ar.contains("جدول الكميات") && ar.contains("جزئية"));
    assert!(ar.contains("Test &lt;villa&gt;") && !ar.contains("<villa>"), "text is escaped");
    // printable: A4 pages, and each of the three tables repeats its header row on every page
    assert!(ar.contains("@page{size:A4 portrait;margin:12mm;") && ar.contains("@bottom-center{content:counter(page) \" / \" counter(pages);direction:ltr;") && ar.contains("thead{display:table-header-group}"), "page numbers, left to right even in Arabic");
    assert!(ar.contains(">الهيكل الخرساني<") && ar.contains(">الجدران<") && ar.contains(">كميات مرجعية (غير مسعّرة)<"), "section headings follow the language");
    assert_eq!(ar.matches("<thead>").count(), 3);
    assert_eq!(ar.matches("<tbody>").count(), 3);
    assert_eq!(ar.matches("</tbody>").count(), 3);
    let en = r.html(false);
    assert!(en.contains("dir=\"ltr\"") && en.contains("Bill of Quantities") && en.contains("PARTIAL"));
}
