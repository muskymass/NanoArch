//! Milestone 5: finishes, as-built deviations, STL export and the standalone HTML tour. Every input is SYNTHETIC test data.

use nnx_core::asbuilt::{self, AsBuilt, Measure};
use nnx_core::finishes::{self, Finishes};
use nnx_core::project::{self, NanoArchProject};
use nnx_core::stl::{self, StlOpts};
use nnx_core::structx::schedule_names;
use nnx_core::tour;
use nnx_core::typed::{Beam, Column, Slab, Space, Wall, element_of};
use nnx_core::*;

fn near(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol, "{a} vs {b}");
}

fn space(key: &str, func: &str, x: i64) -> Space {
    Space { func: func.into(), requirement_key: key.into(), area: 24.0, target_area: 24.0, x, y: 0, w: 6000, h: 4000, boundary: vec![[x, 0], [x + 6000, 0], [x + 6000, 4000], [x, 4000]], net_area: None, glazing_ratio: None, extra: Default::default() }
}

fn wall(x1: i64, y1: i64, x2: i64, y2: i64, a: &str, b: Option<&str>, exterior: bool) -> Wall {
    Wall { x1, y1, x2, y2, thickness: 200, height: 3000, length: (x2 - x1).abs() + (y2 - y1).abs(), is_exterior: exterior, room_a: a.into(), room_b: b.map(str::to_string), extra: Default::default() }
}

/// Two 6 x 4 m rooms (a living, b bathroom), exterior walls, a partition with a beam and a column at (6000, 2000).
fn plan() -> Document {
    let mut cmds = vec![Command::Create(element_of(&space("a", "living", 0), Source::User)), Command::Create(element_of(&space("b", "bathroom", 6000), Source::User))];
    for (x1, y1, x2, y2, a, b, ext) in [(0, 0, 12000, 0, "a", None, true), (12000, 0, 12000, 4000, "b", None, true), (12000, 4000, 0, 4000, "b", None, true), (0, 4000, 0, 0, "a", None, true), (6000, 0, 6000, 4000, "a", Some("b"), false)] {
        cmds.push(Command::Create(element_of(&wall(x1, y1, x2, y2, a, b, ext), Source::User)));
    }
    for (key, x) in [("a", 0), ("b", 6000)] {
        cmds.push(Command::Create(element_of(&Slab { x, y: 0, w: 6000, h: 4000, thickness: 150, kind: "solid".into(), room: key.into(), extra: Default::default() }, Source::User)));
    }
    cmds.push(Command::Create(element_of(&Beam { x1: 6000, y1: 0, x2: 6000, y2: 4000, b: 250, h: 450, length: 4000, extra: Default::default() }, Source::User)));
    cmds.push(Command::Create(element_of(&Column { x: 6000, y: 2000, b: 300, h: 300, grid_ref: "B1".into(), axial_service_kn: None, extra: Default::default() }, Source::User)));
    cmds.push(project::create_command(&NanoArchProject::default()).unwrap());
    let mut doc = Document::default();
    doc.commit("plan", Origin::User, cmds).unwrap_or_else(|e| panic!("{e}"));
    doc
}

// ------------------------------------------------------------------ finishes

#[test]
fn finishes_become_quantities_and_a_priced_boq_row_only_with_your_rate() {
    let doc = plan();
    let f = Finishes { facade: Some("travertine".into()), floor: Some("marble".into()) };
    let items = finishes::items(&doc.model, &f);
    assert_eq!(items.len(), 2);
    let floor = items.iter().find(|i| i.code == "FIN.floor.marble").unwrap();
    near(floor.quantity, 24.0, 1e-6); // the bathroom is excluded
    let facade = items.iter().find(|i| i.code == "FIN.facade.travertine").unwrap();
    assert!(facade.quantity > 0.0);

    // No rate: reported, not priced, and no total.
    let rep = finishes::report(&doc.model, &f, None);
    assert!(rep.total.is_none() && rep.lines.iter().all(|l| l.amount.is_none()));
    assert!(rep.notes.iter().any(|n| n.contains("no rate")));

    // With the user's rates the amounts follow the areas.
    let mut lib = None;
    finishes::set_rate(&mut lib, "FIN.floor.marble", 10.0).unwrap();
    finishes::set_rate(&mut lib, "FIN.facade.travertine", 2.0).unwrap();
    let rep = finishes::report(&doc.model, &f, lib.as_ref());
    near(rep.total.unwrap(), 10.0 * 24.0 + 2.0 * facade.quantity, 0.01);

    // Swapping the finish changes the code and therefore the row that is priced.
    let f2 = Finishes { facade: Some("riyadh_stone".into()), floor: Some("marble".into()) };
    let rep2 = finishes::report(&doc.model, &f2, lib.as_ref());
    assert!(rep2.total.is_none(), "the new facade finish has no rate yet");
    assert!(finishes::set_rate(&mut lib, "FIN.floor.nonsense", 1.0).is_err());
    assert!(finishes::set_rate(&mut lib, "FIN.floor.marble", -1.0).is_err());
}

#[test]
fn an_unknown_finish_is_refused_and_the_choice_is_one_undoable_edit() {
    let mut doc = plan();
    assert!(Finishes { facade: Some("gold".into()), floor: None }.check().is_err());
    project::update(&mut doc, "Project", |p| {
        p.meta.finishes = Finishes { facade: Some("wood_cladding".into()), floor: Some("parquet".into()) };
        Ok(())
    })
    .unwrap();
    assert_eq!(project::get(&doc.model).unwrap().unwrap().meta.finishes.floor.as_deref(), Some("parquet"));
    assert!(doc.is_dirty());
    assert!(doc.undo());
    assert!(project::get(&doc.model).unwrap().is_none_or(|p| p.meta.finishes.floor.is_none()));
    assert!(doc.redo());
    assert_eq!(project::get(&doc.model).unwrap().unwrap().meta.finishes.facade.as_deref(), Some("wood_cladding"));
    let bad = project::update(&mut doc, "Project", |p: &mut NanoArchProject| {
        p.meta.finishes.floor = Some("lava".into());
        Ok(())
    });
    assert!(bad.is_err(), "an unknown finish must not be stored");
}

// ------------------------------------------------------------------ as-built

#[test]
fn as_built_deviations_are_signed_in_mm_and_judged_only_against_your_tolerance() {
    let doc = plan();
    let names = schedule_names(&doc.model, None);
    let col = names.values().find(|n| n.starts_with("COL-")).expect("a column name").clone();
    let rows = asbuilt::parse_csv(&format!("name,x_mm,y_mm,width_mm\n{col},6012,1995,305\nCOL-99,1,1,1\n")).unwrap();
    assert_eq!(rows.len(), 2);

    let mut ab = AsBuilt { tolerance_mm: Some(10.0), source: Some("total station".into()), rows };
    let rep = asbuilt::compare(&doc.model, &ab);
    assert_eq!(rep.matched, 1);
    assert_eq!(rep.unmatched, vec!["COL-99".to_string()]);
    let get = |what: &str| rep.deviations.iter().find(|d| d.what == what).unwrap();
    near(get("x").deviation_mm, 12.0, 1e-9);
    near(get("y").deviation_mm, -5.0, 1e-9);
    near(get("width").deviation_mm, 5.0, 1e-9);
    assert_eq!(get("x").status, "over");
    assert_eq!(get("y").status, "ok");
    assert_eq!(rep.over, 1);
    near(rep.max_abs_mm, 12.0, 1e-9);
    assert_eq!(rep.points.len(), 1);
    assert_eq!(rep.points[0].3, "over");

    // No tolerance: nothing is judged.
    ab.tolerance_mm = None;
    let rep = asbuilt::compare(&doc.model, &ab);
    assert!(rep.deviations.iter().all(|d| d.status == "no tolerance") && rep.over == 0);
    assert!(rep.notes.iter().any(|n| n.contains("no tolerance")));
}

#[test]
fn as_built_csv_errors_name_the_line_and_a_missing_design_value_is_reported() {
    let e = asbuilt::parse_csv("name,x_mm\nCOL-1,abc\n").unwrap_err();
    assert!(e.contains("line 2"), "{e}");
    assert!(asbuilt::parse_csv("name,bogus\nCOL-1,1\n").unwrap_err().contains("unknown column"));
    assert!(asbuilt::parse_csv("x_mm\n1\n").unwrap_err().contains("name"));
    assert!(asbuilt::parse_csv("").is_err());
    assert!(AsBuilt { tolerance_mm: Some(-1.0), ..Default::default() }.check().is_err());

    // A beam has no x: the value is reported, not skipped.
    let doc = plan();
    let beam = schedule_names(&doc.model, None).values().find(|n| n.starts_with("BM-")).cloned();
    if let Some(beam) = beam {
        let ab = AsBuilt { tolerance_mm: Some(5.0), source: None, rows: vec![Measure { name: beam, x_mm: Some(1.0), length_mm: Some(4010.0), ..Default::default() }] };
        let rep = asbuilt::compare(&doc.model, &ab);
        assert!(rep.notes.iter().any(|n| n.contains("no design x")), "{:?}", rep.notes);
        near(rep.deviations.iter().find(|d| d.what == "length").unwrap().deviation_mm, 10.0, 1e-9);
    }
}

// ------------------------------------------------------------------ STL

#[test]
fn every_solid_is_closed_and_the_scale_sets_the_size() {
    let doc = plan();
    let scene = model3d::build(&doc.model);
    assert!(stl::each_solid_closed(&stl::box_triangles([0.0; 3], [1.0, 2.0, 3.0])));
    let (tris, rep) = stl::triangles(&scene, &StlOpts { scale: Some(100.0), level: None, min_feature_mm: None }).unwrap();
    assert!(stl::each_solid_closed(&tris), "every solid must be a closed, consistently oriented mesh");
    assert_eq!(tris.len(), rep.solids * 12);
    // 12.2 m of building at 1:100 is about 122 mm.
    assert!(rep.size_mm[0] > 118.0 && rep.size_mm[0] < 126.0, "{:?}", rep.size_mm);
    let bytes = stl::to_binary(&tris, "NanoArch");
    assert_eq!(bytes.len(), 84 + 50 * tris.len());
    assert_eq!(u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize, tris.len());
    // The model sits on the bed: the lowest z is 0.
    let min_z = tris.iter().flatten().map(|v| v[2]).fold(f32::MAX, f32::min);
    near(min_z as f64, 0.0, 1e-4);

    // Half the scale denominator doubles the size.
    let (_, r50) = stl::triangles(&scene, &StlOpts { scale: Some(50.0), ..Default::default() }).unwrap();
    near(r50.size_mm[0], rep.size_mm[0] * 2.0, 0.05);
}

#[test]
fn stl_needs_a_scale_names_thin_parts_and_refuses_an_unknown_level() {
    let doc = plan();
    let scene = model3d::build(&doc.model);
    assert!(stl::triangles(&scene, &StlOpts::default()).unwrap_err().contains("scale"));
    assert!(stl::triangles(&scene, &StlOpts { scale: Some(0.0), ..Default::default() }).is_err());
    assert!(stl::triangles(&scene, &StlOpts { scale: Some(100.0), level: Some(42), min_feature_mm: None }).is_err());
    let (_, unchecked) = stl::triangles(&scene, &StlOpts { scale: Some(100.0), ..Default::default() }).unwrap();
    assert!(unchecked.notes.iter().any(|n| n.contains("not checked")));
    // A 200 mm wall at 1:100 is 2 mm: it is thinner than a 3 mm printer limit and is named, but still exported.
    let (tris, thin) = stl::triangles(&scene, &StlOpts { scale: Some(100.0), level: None, min_feature_mm: Some(3.0) }).unwrap();
    assert!(!thin.thin.is_empty() && thin.thin.iter().any(|t| t.starts_with("Wall")));
    assert_eq!(tris.len(), thin.solids * 12);
}

// ------------------------------------------------------------------ the standalone tour

#[test]
fn the_tour_is_one_self_contained_file_with_the_scene_inside() {
    let doc = plan();
    let scene = model3d::build(&doc.model);
    let html = tour::tour_html(&scene, "Villa </script> test", false, Some([0.8, 0.7, 0.5]), Some([0.9, 0.9, 0.9])).unwrap();
    assert!(html.starts_with("<!doctype html>") && html.contains("<canvas id=\"gl\">"));
    assert!(html.contains("const D={"), "the scene data is embedded");
    assert!(html.contains("\"k\":\"Wall\"") && html.contains("\"k\":\"Column\""));
    // Offline: no external script, stylesheet or fetch.
    for banned in ["src=\"http", "href=\"http", "fetch(", "XMLHttpRequest", "import("] {
        assert!(!html.contains(banned), "found {banned}");
    }
    // A title cannot break out of the page and the data cannot close the script.
    assert!(!html.contains("Villa </script>"));
    let data_end = html.find("(function(){").unwrap();
    assert!(!html[html.find("const D=").unwrap()..data_end].contains("</"), "the data block must not contain a closing tag");
    // Arabic switches direction.
    assert!(tour::tour_html(&scene, "x", true, None, None).unwrap().contains("dir=\"rtl\""));
}

#[test]
fn an_empty_scene_has_no_tour() {
    let doc = Document::default();
    let scene = model3d::build(&doc.model);
    assert!(tour::tour_html(&scene, "x", false, None, None).is_err());
}
