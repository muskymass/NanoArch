//! The project element and the code checks, at the engine level (the application tests cover the whole flow).

use nnx_core::project::{self, BriefIn, NanoArchProject, RoomIn};
use nnx_core::rules::{Param, Severity};
use nnx_core::typed::{Space, element_of};
use nnx_core::{Document, Model, Origin, Source, sbc};
use serde_json::Map;

fn brief() -> BriefIn {
    BriefIn {
        title: Some("t".into()),
        width_m: 18.0,
        length_m: 22.0,
        setback_front_m: 2.0,
        setback_back_m: 2.0,
        setback_side_m: 2.0,
        entry: "south".into(),
        rooms: vec![RoomIn { key: "bed".into(), name: None, func: "bedroom".into(), area: 12.0, zone: "private".into(), light: true, min_width_mm: None, floor: None, phase: None }],
        seed: None,
        stair: None,
        wall_exterior_mm: None,
        wall_interior_mm: None,
        wall_height_mm: None,
    }
}

fn with_project() -> Document {
    let mut doc = Document::default();
    let p = NanoArchProject { brief: Some(brief()), ..NanoArchProject::default() };
    doc.commit("project", Origin::User, vec![project::create_command(&p).unwrap()]).unwrap();
    doc
}

fn space(key: &str, func: &str, w: i64, h: i64) -> nnx_core::Command {
    let s = Space {
        func: func.into(),
        requirement_key: key.into(),
        area: w as f64 * h as f64 / 1e6,
        target_area: 10.0,
        x: 0,
        y: 0,
        w,
        h,
        boundary: vec![[0, 0], [w, 0], [w, h], [0, h]],
        net_area: None,
        glazing_ratio: None,
        extra: Map::new(),
    };
    nnx_core::Command::Create(element_of(&s, Source::User))
}

#[test]
fn a_project_edit_is_one_undoable_step_and_an_unchanged_edit_commits_nothing() {
    let mut doc = with_project();
    doc.mark_saved();
    let out = project::update(&mut doc, "setback", |p| {
        p.brief.as_mut().unwrap().setback_front_m = 3.0;
        Ok(())
    })
    .unwrap();
    assert!(out.changed && doc.is_dirty());
    assert_eq!(project::get(&doc.model).unwrap().unwrap().brief.unwrap().setback_front_m, 3.0);
    assert!(!project::update(&mut doc, "same", |_| Ok(())).unwrap().changed);
    assert_eq!(doc.undo_label(), Some("setback"));
    assert!(doc.undo());
    assert!(!doc.is_dirty(), "undo returns to the saved state");
    assert!(doc.redo());
    assert_eq!(project::get(&doc.model).unwrap().unwrap().brief.unwrap().setback_front_m, 3.0);
}

#[test]
fn invalid_projects_never_reach_the_model() {
    let mut doc = with_project();
    let before = doc.model.state_hash();
    let err = project::update(&mut doc, "x", |p| {
        p.brief.as_mut().unwrap().setback_side_m = 9.0;
        Ok(())
    })
    .unwrap_err();
    assert!(err.contains("no buildable area"), "{err}");
    let err = project::update(&mut doc, "x", |p| {
        p.limits.min_setback_front_m = Some(Param { value: Some(3.0), source: " ".into() });
        Ok(())
    })
    .unwrap_err();
    assert!(err.contains("source"), "{err}");
    assert_eq!(doc.model.state_hash(), before);
    // A second project element is refused by the typed check.
    let dup = project::create_command(&NanoArchProject::default()).unwrap();
    assert!(doc.commit("dup", Origin::User, vec![dup]).is_err());
}

#[test]
fn the_typed_project_round_trips_through_the_stored_properties_unchanged() {
    let mut p = NanoArchProject { brief: Some(brief()), ..NanoArchProject::default() };
    p.meta.building_type = Some(project::BuildingType::Palace);
    p.meta.smart_tech = vec!["access".into()];
    p.limits.max_coverage_ratio = Some(Param { value: Some(0.6), source: "regulation X".into() });
    p.mep = Some(serde_json::from_value(serde_json::json!({ "electrical": { "voltage_v": 230.0 } })).unwrap());
    let mut doc = Document::default();
    doc.commit("p", Origin::User, vec![project::create_command(&p).unwrap()]).unwrap();
    let back = project::get(&doc.model).unwrap().unwrap();
    assert_eq!(serde_json::to_value(&back).unwrap(), serde_json::to_value(&p).unwrap());
}

#[test]
fn room_minimums_come_from_the_code_table_with_their_page_and_unread_values_are_skipped() {
    let mut doc = with_project();
    doc.commit("rooms", Origin::User, vec![space("small", "bedroom", 2000, 2500), space("fine", "bedroom", 3000, 4000), space("odd", "gym", 3000, 3000)]).unwrap();
    let r = sbc::check(&doc.model);
    let small: Vec<_> = r.violations.iter().filter(|v| v.rule.starts_with("room_min") && v.ar.contains("small")).collect();
    assert_eq!(small.len(), 2, "5 m2 is below 6.5 m2 and the width of 2.0 m is below 2.1 m: two rules fire: {small:?}");
    assert!(r.violations.iter().any(|v| v.rule == "room_min_area_m2.bedroom" && v.limit == 6.5 && v.severity == Severity::Error && v.source.contains("p.26")));
    assert!(r.violations.iter().any(|v| v.rule == "room_min_width_m.bedroom" && v.limit == 2.1));
    assert!(!r.violations.iter().any(|v| v.rule.starts_with("room_min") && v.ar.contains("fine")), "a 12 m2 bedroom meets the area and width minimums");
    assert!(r.violations.iter().any(|v| v.rule == "daylight_min_ratio.bedroom" && v.ar.contains("fine")), "and with no window it has 0% glazing, which is reported");
    assert!(r.skipped.iter().any(|s| s.rule == "room_min_area_m2.gym"), "a function with no value in the files is skipped, not passed");
}

#[test]
fn every_code_value_has_a_clause_a_file_and_a_page() {
    assert!(!sbc::CODE_VALUES.is_empty());
    for v in sbc::CODE_VALUES {
        assert!(v.value > 0.0 && !v.clause.is_empty() && !v.file.is_empty() && v.page > 0 && !v.edition.is_empty(), "{v:?}");
    }
    let mut ids: Vec<_> = sbc::CODE_VALUES.iter().map(|v| v.id).collect();
    ids.sort();
    let n = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), n, "no value is listed twice");
}

#[test]
fn a_model_without_a_project_has_nothing_to_check() {
    let model = Model::default();
    let r = sbc::check(&model);
    assert!(r.violations.is_empty() && r.skipped.is_empty() && r.checked == 0);
}
