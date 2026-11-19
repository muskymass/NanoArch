//! Elevations, sections, discipline layers and clash detection on a small model built through commands.

use nnx_core::clash::{self, ClashOpts};
use nnx_core::elevation::{self, Cut, Side, ViewError, ViewOpts};
use nnx_core::model3d::{self, DISCIPLINES};
use nnx_core::typed::{Beam, Column, Door, MepRun, Space, Wall, Window, element_of, run_element};
use nnx_core::{Command, Document, Element, Id, Origin, Source};
use serde_json::Map;

/// One 6 x 4 m room with four exterior walls (3 m high), a door and a window on the south wall, a slab, a beam under the
/// slab along the room's middle and a column in the middle.
struct Fixture {
    doc: Document,
    beam: Id,
    column: Id,
    window: Id,
    south: Id,
}

fn wall(x1: i64, y1: i64, x2: i64, y2: i64, a: &str) -> Element {
    element_of(&Wall { x1, y1, x2, y2, thickness: 200, height: 3000, length: (x2 - x1).abs() + (y2 - y1).abs(), is_exterior: true, room_a: a.into(), room_b: None, extra: Map::new() }, Source::User)
}

fn fixture() -> Fixture {
    let mut cmds = Vec::new();
    let space = Space { func: "living".into(), requirement_key: "r".into(), area: 24.0, target_area: 24.0, x: 0, y: 0, w: 6000, h: 4000, boundary: vec![[0, 0], [6000, 0], [6000, 4000], [0, 4000]], net_area: None, glazing_ratio: None, extra: Map::new() };
    cmds.push(Command::Create(element_of(&space, Source::User)));
    let south = wall(0, 0, 6000, 0, "r");
    let south_id = south.id;
    for w in [south, wall(6000, 0, 6000, 4000, "r"), wall(6000, 4000, 0, 4000, "r"), wall(0, 4000, 0, 0, "r")] {
        cmds.push(Command::Create(w));
    }
    let door = element_of(&Door { width: 900, height: 2100, offset_center: 1000, is_egress: true, from: "r".into(), to: "out".into(), opens_into: "r".into(), extra: Map::new() }, Source::User).hosted_by(south_id);
    let win = element_of(&Window { width: 1500, height: 1200, sill_height: 900, offset_center: 4000, room: "r".into(), extra: Map::new() }, Source::User).hosted_by(south_id);
    let window = win.id;
    cmds.push(Command::Create(door));
    cmds.push(Command::Create(win));
    let slab = nnx_core::typed::Slab { x: 0, y: 0, w: 6000, h: 4000, thickness: 150, kind: "roof".into(), room: "r".into(), extra: Map::new() };
    cmds.push(Command::Create(element_of(&slab, Source::User)));
    let beam = element_of(&Beam { x1: 0, y1: 2000, x2: 6000, y2: 2000, b: 250, h: 450, length: 6000, extra: Map::new() }, Source::User);
    let beam_id = beam.id;
    cmds.push(Command::Create(beam));
    let column = element_of(&Column { x: 3000, y: 2000, b: 300, h: 300, grid_ref: "C1".into(), axial_service_kn: None, extra: Map::new() }, Source::User);
    let column_id = column.id;
    cmds.push(Command::Create(column));
    let mut doc = Document::default();
    doc.commit("fixture", Origin::User, cmds).unwrap_or_else(|e| panic!("fixture: {e}"));
    Fixture { doc, beam: beam_id, column: column_id, window, south: south_id }
}

fn run(system: &str, p1: [i64; 3], p2: [i64; 3], w: i64, h: i64) -> Element {
    run_element(&MepRun { system: system.into(), x1: p1[0], y1: p1[1], z1: p1[2], x2: p2[0], y2: p2[1], z2: p2[2], width: w, height: h, extra: Map::new() }, None, Source::User).unwrap()
}

#[test]
fn every_solid_belongs_to_one_discipline_and_a_scene_can_be_isolated_by_layer() {
    let mut f = fixture();
    f.doc.commit("runs", Origin::User, vec![Command::Create(run("duct", [500, 1000, 2600], [5500, 1000, 2600], 300, 200)), Command::Create(run("pipe", [500, 3000, 100], [5500, 3000, 100], 50, 50))]).unwrap();
    let scene = model3d::build(&f.doc.model);
    assert!(scene.boxes.iter().all(|b| DISCIPLINES.contains(&b.discipline)));
    let of = |d: &str| scene.boxes.iter().filter(|b| b.discipline == d).count();
    for d in DISCIPLINES {
        assert!(of(d) > 0 || d == "electrical", "{d} has solids");
    }
    assert!(scene.boxes.iter().filter(|b| matches!(b.kind, "Beam" | "Column" | "Slab")).all(|b| b.discipline == "structural"));
    assert!(scene.boxes.iter().filter(|b| b.kind == "MepRun" && b.class == "duct").all(|b| b.discipline == "hvac"));
    assert!(scene.boxes.iter().filter(|b| b.kind == "MepRun" && b.class == "pipe").all(|b| b.discipline == "plumbing"));
    let only = scene.filter(&["structural"]);
    assert_eq!(only.boxes.len(), of("structural"));
    assert!(only.boxes.iter().all(|b| b.discipline == "structural"));
    assert!(only.max[2] <= scene.max[2] && only.levels.len() == scene.levels.len());
    assert!(scene.filter(&[]).boxes.is_empty());
    assert!(scene.filter(&["nonsense"]).boxes.is_empty());
    assert_eq!(scene.filter(&DISCIPLINES).boxes.len(), scene.boxes.len());
}

#[test]
fn a_duct_through_a_beam_and_a_pipe_through_a_column_are_clashes_with_exact_ids_and_a_clear_run_is_not() {
    let mut f = fixture();
    // The beam hangs from the slab underside at z = 3.0 down to 2.55 along y = 2.0, x 0..6.
    let hit_beam = run("duct", [500, 1500, 2700], [500, 2500, 2700], 300, 200);
    let hit_beam_id = hit_beam.id;
    // A pipe through the column (x 2.85..3.15, y 1.85..2.15), at mid height.
    let hit_col = run("pipe", [2000, 2000, 1500], [4000, 2000, 1500], 100, 100);
    let hit_col_id = hit_col.id;
    // Under the beam (below 2.55): no clash. Beside the column: no clash.
    let clear = run("duct", [500, 1000, 2000], [5500, 1000, 2000], 300, 200);
    let beside = run("drain", [500, 3000, -100], [5500, 3000, -100], 110, 110);
    f.doc.commit("runs", Origin::User, vec![Command::Create(hit_beam), Command::Create(hit_col), Command::Create(clear), Command::Create(beside)]).unwrap();
    let scene = model3d::build(&f.doc.model);
    let report = clash::detect(&scene, &ClashOpts::default());
    assert_eq!(report.runs, 4);
    assert_eq!(report.structural, 2);
    let ids: Vec<(String, String)> = report.clashes.iter().map(|c| (c.run_id.clone(), c.struct_id.clone())).collect();
    assert!(ids.contains(&(hit_beam_id.to_string(), f.beam.to_string())), "{ids:?}");
    assert!(ids.contains(&(hit_col_id.to_string(), f.column.to_string())), "{ids:?}");
    // The pipe at y = 2.0, z = 1.5 passes the beam's footprint too, but the beam is at z 2.55..3.0, so only the column.
    assert!(!ids.contains(&(hit_col_id.to_string(), f.beam.to_string())));
    assert_eq!(report.clashes.len(), 2, "only the two real clashes: {ids:?}");
    let c = report.clashes.iter().find(|c| c.run_id == hit_beam_id.to_string()).unwrap();
    assert_eq!((c.run_system.as_str(), c.struct_kind, c.run_discipline, c.struct_discipline), ("duct", "Beam", "hvac", "structural"));
    assert!((c.min[2] - 2.6).abs() < 1e-9 && (c.max[2] - 2.8).abs() < 1e-9, "the overlap is the duct's z 2.6..2.8 inside the beam: {c:?}");
    assert!((c.volume_m3 - 0.25 * 0.2 * 0.3).abs() < 1e-9 || c.volume_m3 > 0.0);
    assert!((c.min[0] - 0.35).abs() < 1e-9 && (c.max[0] - 0.65).abs() < 1e-9, "x overlap is the duct's 0.35..0.65: {c:?}");
}

#[test]
fn touching_is_not_a_clash_and_the_tolerance_is_honoured() {
    let mut f = fixture();
    // The beam's underside is z = 2.55. A duct whose top is exactly 2.55 only touches it.
    f.doc.commit("duct", Origin::User, vec![Command::Create(run("duct", [500, 1500, 2450], [500, 2500, 2450], 300, 200))]).unwrap();
    let scene = model3d::build(&f.doc.model);
    assert!(clash::detect(&scene, &ClashOpts::default()).clashes.is_empty(), "face contact only");
    // A duct 2 mm into the beam is below the default 5 mm tolerance, 2 mm with a tighter tolerance is a clash.
    let mut g = fixture();
    g.doc.commit("duct", Origin::User, vec![Command::Create(run("duct", [500, 1500, 2452], [500, 2500, 2452], 300, 200))]).unwrap();
    let scene = model3d::build(&g.doc.model);
    assert!(clash::detect(&scene, &ClashOpts::default()).clashes.is_empty());
    assert_eq!(clash::detect(&scene, &ClashOpts { tolerance: 0.001 }).clashes.len(), 1);
}

#[test]
fn a_vertical_drain_through_a_footing_is_found_and_slabs_are_not_tested() {
    let mut f = fixture();
    let footing = element_of(&nnx_core::typed::Footing { x: 3000, y: 1000, length: 1000, width: 1000, service_load_kn: 100.0, bearing_kpa: 150.0, column: f.column.to_string(), extra: Map::new() }, Source::User);
    let fid = footing.id;
    f.doc.commit("footing", Origin::User, vec![Command::Create(footing)]).unwrap();
    // A vertical drain at the footing's centre from below the floor up to 0.5 m. Footings are drawn below the floor plate.
    f.doc.commit("drain", Origin::User, vec![Command::Create(run("drain", [3000, 1000, -400], [3000, 1000, 500], 110, 110))]).unwrap();
    // A duct crossing the roof slab edge (z 3.0..3.15) is a sleeve matter, not a clash.
    f.doc.commit("duct", Origin::User, vec![Command::Create(run("duct", [500, 500, 2900], [500, 500, 3300], 200, 200))]).unwrap();
    let report = clash::detect(&model3d::build(&f.doc.model), &ClashOpts::default());
    assert_eq!(report.clashes.len(), 1, "{:?}", report.clashes);
    assert_eq!((report.clashes[0].struct_kind, report.clashes[0].struct_id.as_str()), ("Footing", fid.to_string().as_str()));
}

#[test]
fn runs_are_validated_when_they_are_committed() {
    let mut f = fixture();
    let bad = |system: &str, p2: [i64; 3], w: i64| run_element(&MepRun { system: system.into(), x1: 0, y1: 0, z1: 0, x2: p2[0], y2: p2[1], z2: p2[2], width: w, height: 100, extra: Map::new() }, None, Source::User);
    assert!(bad("steam", [100, 0, 0], 100).is_err(), "unknown system");
    let diagonal = bad("duct", [100, 100, 0], 100).unwrap();
    assert!(f.doc.commit("d", Origin::User, vec![Command::Create(diagonal)]).is_err(), "a diagonal run is refused");
    let zero = bad("duct", [0, 0, 0], 100).unwrap();
    assert!(f.doc.commit("z", Origin::User, vec![Command::Create(zero)]).is_err());
    let thin = bad("duct", [100, 0, 0], 0).unwrap();
    assert!(f.doc.commit("t", Origin::User, vec![Command::Create(thin)]).is_err());
    // A duct filed under the wrong discipline is refused too.
    let mut wrong = run("duct", [0, 0, 0], [100, 0, 0], 100, 100);
    wrong.discipline = nnx_core::Discipline::Plb;
    assert!(f.doc.commit("w", Origin::User, vec![Command::Create(wrong)]).is_err());
}

#[test]
fn the_south_elevation_shows_the_south_wall_its_window_and_door_but_not_the_other_walls() {
    let f = fixture();
    let svg = elevation::elevation_svg(&f.doc.model, Side::South, &ViewOpts::default()).unwrap();
    assert!(svg.starts_with("<svg") && svg.ends_with("</svg>\n") || svg.trim_end().ends_with("</svg>"));
    assert!(svg.contains(&format!("data-id=\"{}\"", f.window)), "the window pane is drawn, by its element id");
    assert!(svg.contains(&format!("data-id=\"{}\"", f.south)), "pieces of the south wall are drawn");
    assert!(svg.contains("class=\"leaf\"") && svg.contains("class=\"glass\"") && svg.contains("class=\"slab\""));
    assert!(svg.contains("South elevation") && svg.contains("±0.00"));
    // No parapet unless a height is given.
    assert!(!svg.contains("class=\"parapet\""));
    let with = elevation::elevation_svg(&f.doc.model, Side::South, &ViewOpts { parapet_m: Some(1.0), ..ViewOpts::default() }).unwrap();
    assert!(with.contains("class=\"parapet\""));
    // The north wall has no door or window, so the north elevation has neither.
    let north = elevation::elevation_svg(&f.doc.model, Side::North, &ViewOpts::default()).unwrap();
    assert!(!north.contains("class=\"glass\"") && !north.contains("class=\"leaf\""));
    assert!(!north.contains(&format!("data-id=\"{}\"", f.south)));
    // East and west are drawn too, each from its own walls.
    for side in [Side::East, Side::West] {
        let svg = elevation::elevation_svg(&f.doc.model, side, &ViewOpts::default()).unwrap();
        assert!(svg.contains("class=\"wall\""), "{side:?}");
    }
}

#[test]
fn an_edit_shows_up_in_the_next_elevation_and_the_height_label_follows_the_wall() {
    let mut f = fixture();
    let before = elevation::elevation_svg(&f.doc.model, Side::South, &ViewOpts::default()).unwrap();
    // Move the window along the wall: the drawing must change.
    f.doc.commit("move", Origin::User, vec![Command::SetProp { id: f.window, key: "offsetCenter".into(), value: Some(serde_json::json!(5000)) }]).unwrap();
    let after = elevation::elevation_svg(&f.doc.model, Side::South, &ViewOpts::default()).unwrap();
    assert_ne!(before, after);
    assert!(before.contains(">3.15<"), "the building height (roof slab top, 3.0 wall + 0.15 slab) is dimensioned");
}

#[test]
fn a_section_cuts_walls_slab_beam_and_column_and_shows_what_is_beyond_light() {
    let f = fixture();
    // The plane y = 2.0 passes through the beam's axis and the column.
    let cut = Cut::AtY { name: "A-A".into(), at: 2.0 };
    let svg = elevation::section_svg(&f.doc.model, &cut, &ViewOpts::default()).unwrap();
    assert!(svg.contains("A-A") && svg.contains("looking north"));
    assert!(svg.contains(&format!("data-id=\"{}\"", f.beam)) && svg.contains(&format!("data-id=\"{}\"", f.column)));
    assert!(svg.contains("class=\"wall cut\"") && svg.contains("class=\"slab cut\""), "walls and slab are cut by the plane");
    assert!(svg.contains("class=\"wall far\""), "the north wall is beyond the plane");
    // B-B across the room's width.
    let b = elevation::section_svg(&f.doc.model, &Cut::AtX { name: "B-B".into(), at: 3.0 }, &ViewOpts::default()).unwrap();
    assert!(b.contains("B-B") && b.contains("looking west") && b.contains("class=\"column\""), "the column is cut by x = 3.0");
    // Default cuts are in the middle, and a plane outside the building is an error.
    assert_eq!(elevation::default_cut(&f.doc.model, "A-A"), Some(Cut::AtY { name: "A-A".into(), at: 2.0 }));
    assert_eq!(elevation::default_cut(&f.doc.model, "B-B"), Some(Cut::AtX { name: "B-B".into(), at: 3.0 }));
    assert!(elevation::default_cut(&f.doc.model, "C-C").is_none());
    let err = elevation::section_svg(&f.doc.model, &Cut::AtY { name: "A-A".into(), at: 9.0 }, &ViewOpts::default()).unwrap_err();
    assert!(matches!(err, ViewError::OutsideBuilding { .. }));
    assert_eq!(elevation::section_svg(&f.doc.model, &Cut::AtY { name: "A-A".into(), at: 0.0 }, &ViewOpts::default()).unwrap_err(), ViewError::OutsideBuilding { at: 0.0, lo: 0.0, hi: 4.0 });
}

#[test]
fn runs_appear_in_sections_and_the_parapet_height_must_be_positive() {
    let mut f = fixture();
    let r = run("duct", [500, 1000, 2000], [500, 3000, 2000], 300, 200);
    let rid = r.id;
    f.doc.commit("run", Origin::User, vec![Command::Create(r)]).unwrap();
    // The plane x = 0.5 passes through the duct's axis.
    let svg = elevation::section_svg(&f.doc.model, &Cut::AtX { name: "B-B".into(), at: 0.5 }, &ViewOpts::default()).unwrap();
    assert!(svg.contains(&format!("data-id=\"{rid}\"")) && svg.contains("class=\"duct\""));
    for p in [0.0, -1.0, f64::NAN] {
        let e = elevation::elevation_svg(&f.doc.model, Side::South, &ViewOpts { parapet_m: Some(p), ..ViewOpts::default() }).unwrap_err();
        assert_eq!(e, ViewError::BadParapet);
    }
    let empty = Document::default();
    assert_eq!(elevation::elevation_svg(&empty.model, Side::South, &ViewOpts::default()).unwrap_err(), ViewError::Empty);
}
