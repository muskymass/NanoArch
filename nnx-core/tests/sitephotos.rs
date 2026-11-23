//! Site progress photos linked to elements (tracker item 91). Every value is SYNTHETIC test data.

use nnx_core::sitephotos::{SitePhoto, SitePhotos, report, validate_links};
use nnx_core::typed::{Column, Space, element_of};
use nnx_core::*;

fn model() -> (Model, String) {
    let col = element_of(&Column { x: 1000, y: 0, b: 300, h: 300, grid_ref: "A1".into(), axial_service_kn: None, extra: Default::default() }, Source::User);
    let room = element_of(&Space { func: "living".into(), requirement_key: "living1".into(), area: 24.0, target_area: 24.0, x: 0, y: 0, w: 6000, h: 4000, boundary: vec![[0, 0], [6000, 0], [6000, 4000], [0, 4000]], net_area: None, glazing_ratio: None, extra: Default::default() }, Source::User);
    let id = col.id.to_string();
    let mut doc = Document::default();
    doc.commit("m", Origin::User, vec![Command::Create(col), Command::Create(room)]).unwrap_or_else(|e| panic!("{e}"));
    (doc.model, id)
}

fn photo(id: &str, taken: &str, els: &[&str], pct: Option<f64>) -> SitePhoto {
    SitePhoto { id: id.into(), file: format!("{id}.jpg"), taken: taken.into(), elements: els.iter().map(|s| s.to_string()).collect(), observed_pct: pct, ..Default::default() }
}

#[test]
fn a_photo_without_id_file_date_or_element_is_refused_by_name() {
    let ok = photo("a", "2026-01-05", &["x"], None);
    for (bad, msg) in [
        (SitePhoto { id: " ".into(), ..ok.clone() }, "needs an id"),
        (SitePhoto { file: "".into(), ..ok.clone() }, "needs its file"),
        (SitePhoto { taken: "2026-13-01".into(), ..ok.clone() }, "not a date"),
        (SitePhoto { taken: "2026-02-29".into(), ..ok.clone() }, "not a date"),
        (SitePhoto { elements: vec![], ..ok.clone() }, "not linked"),
        (SitePhoto { elements: vec!["x".into(), "x".into()], ..ok.clone() }, "twice"),
        (SitePhoto { observed_pct: Some(101.0), ..ok.clone() }, "between 0 and 100"),
        (SitePhoto { observed_pct: Some(f64::NAN), ..ok.clone() }, "between 0 and 100"),
    ] {
        let e = SitePhotos { photos: vec![bad] }.check().unwrap_err();
        assert!(e.contains(msg), "{e}");
    }
    let dup = SitePhotos { photos: vec![ok.clone(), ok.clone()] }.check().unwrap_err();
    assert!(dup.contains("used twice"), "{dup}");
    assert!(SitePhotos { photos: vec![ok] }.check().is_ok());
}

#[test]
fn links_resolve_by_schedule_name_id_or_room_key_and_unknown_ones_are_refused() {
    let (m, col_id) = model();
    let sp = SitePhotos { photos: vec![photo("a", "2026-01-05", &["COL-1", &col_id, "room:living1"], None)] };
    assert!(validate_links(&m, &sp).is_ok());
    let bad = SitePhotos { photos: vec![photo("z", "2026-01-05", &["COL-99"], None)] };
    let e = validate_links(&m, &bad).unwrap_err();
    assert!(e.contains("'z'") && e.contains("COL-99"), "{e}");
}

#[test]
fn the_report_orders_by_date_finds_the_latest_photo_and_names_unphotographed_elements() {
    let (m, col_id) = model();
    // Entered newest first: the report still orders by date.
    let sp = SitePhotos { photos: vec![photo("late", "2026-03-10", &["COL-1"], Some(80.0)), photo("early", "2026-03-01", &["COL-1", "room:living1"], Some(20.0)), photo("noreading", "2026-03-20", &["COL-1"], None)] };
    let r = report(&m, &sp);
    assert_eq!(r.photos.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["early", "late", "noreading"]);
    assert_eq!(r.first_taken.as_deref(), Some("2026-03-01"));
    assert_eq!(r.last_taken.as_deref(), Some("2026-03-20"));
    let col = r.elements.iter().find(|e| e.element_id == col_id).unwrap();
    assert_eq!(col.photo_ids, ["early", "late", "noreading"]);
    assert_eq!(col.latest_photo, "noreading");
    // The newest photo has no reading, so the latest reading is the one before it, not an invented value.
    assert_eq!(col.latest_observed_pct, Some(80.0));
    // The room has a photo but no schedule name; the only schedule-named element (the column) is photographed.
    assert!(r.unphotographed.is_empty());
    // With no photos every schedule-named element is listed and the report says so.
    let none = report(&m, &SitePhotos::default());
    assert_eq!(none.unphotographed.iter().map(|u| u.name.as_str()).collect::<Vec<_>>(), ["COL-1"]);
    assert!(none.notes.iter().any(|n| n.contains("no site photos")));
}

#[test]
fn a_deleted_element_is_reported_as_unresolved_not_dropped() {
    let (m, _) = model();
    let sp = SitePhotos { photos: vec![photo("a", "2026-01-05", &["COL-1", "GONE-1"], None)] };
    let r = report(&m, &sp);
    assert_eq!(r.photos[0].unresolved, ["GONE-1"]);
    assert_eq!(r.photos[0].element_ids.len(), 1);
    assert!(r.notes.iter().any(|n| n.contains("GONE-1")));
}

#[test]
fn csv_has_one_row_per_photo_and_element_and_quotes_commas() {
    let (m, _) = model();
    let mut p = photo("a", "2026-01-05", &["COL-1", "room:living1"], Some(50.0));
    p.caption = Some("rebar, tied".into());
    let r = report(&m, &SitePhotos { photos: vec![p] });
    let csv = r.csv();
    assert_eq!(csv.lines().count(), 3);
    assert!(csv.contains("\"rebar, tied\""));
    assert!(r.html().contains("<table"));
}
