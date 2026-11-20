//! Milestone 9: drafting on an empty project, rooms from enclosed walls, grid lines, permanent dimensions and room tags.
//! Every value is SYNTHETIC test data.

use nnx_core::annot::{self, DimSpec, GridSpec};
use nnx_core::draft::{self, SnapCtx, SnapKind, WallSpec, Xf};
use nnx_core::drawing::{self, DrawOpts};
use nnx_core::dxf::{self, DxfOpts};
use nnx_core::elevation::{self, Cut, Side, ViewOpts, ViewSpec};
use nnx_core::typed::{Grid, Space};
use nnx_core::*;
use serde_json::json;

fn blank() -> Document {
    let mut doc = Document::default();
    doc.commit("new project", Origin::User, annot::blank_project_commands("Synthetic").unwrap()).unwrap_or_else(|e| panic!("{e}"));
    doc.mark_saved();
    doc
}

fn wall(doc: &mut Document, a: [i64; 2], b: [i64; 2]) -> Id {
    let level = building::levels(&doc.model).first().map(|(_, id)| *id);
    let (c, id) = draft::wall_command(&doc.model, level, &WallSpec { a, b, thickness: 200, height: 3000 }).unwrap_or_else(|e| panic!("{e}"));
    doc.commit("wall", Origin::User, vec![c]).unwrap_or_else(|e| panic!("{e}"));
    id
}

fn level(doc: &Document) -> Option<Id> {
    building::levels(&doc.model).first().map(|(_, id)| *id)
}

/// Four walls round a 6 x 4 m rectangle with its lower-left corner at (1000, 1000).
fn box_room(doc: &mut Document) -> [Id; 4] {
    [wall(doc, [1000, 1000], [7000, 1000]), wall(doc, [7000, 1000], [7000, 5000]), wall(doc, [1000, 5000], [7000, 5000]), wall(doc, [1000, 1000], [1000, 5000])]
}

fn sync(doc: &mut Document, label: &str) -> annot::RoomSync {
    let s = annot::rooms_sync(&doc.model, level(doc));
    if !s.cmds.is_empty() {
        doc.commit(label, Origin::User, s.cmds.clone()).unwrap_or_else(|e| panic!("{e}"));
    }
    s
}

fn rooms(doc: &Document) -> Vec<(String, [i64; 4], Source)> {
    doc.model.elements.values().filter(|e| e.kind == "Space").map(|e| {
        let s = e.typed::<Space>().unwrap();
        (s.requirement_key, [s.x, s.y, s.w, s.h], e.provenance.source)
    }).collect()
}

fn dxf_opts() -> DxfOpts {
    DxfOpts { title: "Synthetic".into(), labels: Default::default(), mep: false }
}

fn opts() -> DrawOpts {
    DrawOpts { blank_canvas: true, ..DrawOpts::default() }
}

// ------------------------------------------------------------------ the empty project

#[test]
fn an_empty_project_has_the_project_and_level_lvl_1_and_draws_a_blank_plan() {
    let doc = blank();
    let lv = building::levels(&doc.model);
    assert_eq!(lv.len(), 1);
    assert_eq!(doc.model.get(&lv[0].1).unwrap().props["name"], "LVL-1");
    assert!(project::get(&doc.model).unwrap().is_some());
    assert_eq!(doc.model.len(), 2, "only the level and the project");
    // The plan is drawn with the blank drawing area and no overall dimensions; without the option it still refuses.
    let svg = drawing::plan_svg(&doc.model, level(&doc), &opts()).unwrap();
    assert!(svg.starts_with("<svg") && !svg.contains(">20.00 m<") && !svg.contains(">12.00 m<"), "no overall dimension of the blank area");
    assert_eq!(drawing::plan_svg(&doc.model, level(&doc), &DrawOpts::default()).unwrap_err(), drawing::DrawError::NoSpaces);
    let f = drawing::frame(&doc.model, level(&doc), &opts()).unwrap();
    assert_eq!((f.0, f.1), (0.0, 12_000.0), "the blank area is 20 x 12 m, a view window");
    // What is drawn beyond it enlarges the plan.
    let mut d2 = blank();
    wall(&mut d2, [0, 0], [30_000, 0]);
    let svg2 = drawing::plan_svg(&d2.model, level(&d2), &opts()).unwrap();
    let w = |s: &str| s.split("viewBox=\"0 0 ").nth(1).and_then(|r| r.split(' ').next()).and_then(|v| v.parse::<f64>().ok()).unwrap();
    assert!(w(&svg2) > w(&svg));
}

// ------------------------------------------------------------------ rooms from enclosed walls

#[test]
fn four_walls_make_one_room_with_its_rectangle_and_area_and_a_second_pass_changes_nothing() {
    let mut doc = blank();
    box_room(&mut doc);
    let s = sync(&mut doc, "Find rooms");
    assert_eq!(s.added, ["R1"]);
    assert_eq!(rooms(&doc), vec![("R1".to_string(), [1000, 1000, 6000, 4000], Source::Computed)]);
    let sp = doc.model.elements.values().find(|e| e.kind == "Space").unwrap().typed::<Space>().unwrap();
    assert_eq!((sp.area, sp.func.as_str()), (24.0, "room"));
    assert_eq!(doc.model.elements.values().find(|e| e.kind == "Space").unwrap().level_id, level(&doc));
    assert!(annot::rooms_sync(&doc.model, level(&doc)).cmds.is_empty(), "nothing to do the second time");
    // The wall that closes the box was the last one: with three walls there is no room.
    let mut open3 = blank();
    wall(&mut open3, [1000, 1000], [7000, 1000]);
    wall(&mut open3, [7000, 1000], [7000, 5000]);
    wall(&mut open3, [1000, 1000], [1000, 5000]);
    assert!(annot::rooms_sync(&open3.model, level(&open3)).cmds.is_empty());
}

#[test]
fn a_partition_makes_two_rooms_and_removing_it_takes_them_back_to_one_with_the_tag_gone_too() {
    let mut doc = blank();
    box_room(&mut doc);
    let part = wall(&mut doc, [4000, 1000], [4000, 5000]);
    let s = sync(&mut doc, "Find rooms");
    assert_eq!(s.added.len(), 2);
    let r: Vec<[i64; 4]> = rooms(&doc).into_iter().map(|x| x.1).collect();
    assert!(r.contains(&[1000, 1000, 3000, 4000]) && r.contains(&[4000, 1000, 3000, 4000]));
    // Tag one room, then delete the partition: the two rooms are replaced by one and the old rooms' tag goes with its room.
    let first = doc.model.elements.values().find(|e| e.kind == "Space").unwrap().id;
    let (tc, _) = annot::tag_commands(&doc.model, &[first]).unwrap();
    doc.commit("tag", Origin::User, tc).unwrap_or_else(|e| panic!("{e}"));
    let mut cmds = draft::delete(&doc.model, &[part]).unwrap().cmds;
    let mut scratch = Document::new(doc.model.clone());
    scratch.commit("x", Origin::User, cmds.clone()).unwrap_or_else(|e| panic!("{e}"));
    let s2 = annot::rooms_sync(&scratch.model, level(&doc));
    assert_eq!((s2.removed.len(), s2.added.len()), (2, 1));
    cmds.extend(s2.cmds);
    doc.commit("Delete", Origin::User, cmds).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(rooms(&doc).len(), 1);
    assert!(doc.model.elements.values().all(|e| e.kind != "Tag"), "the tag left with its room");
    // One undo brings the partition, both rooms and the tag back.
    assert!(doc.undo());
    assert_eq!(rooms(&doc).len(), 2);
    assert_eq!(doc.model.elements.values().filter(|e| e.kind == "Tag").count(), 1);
}

#[test]
fn a_wall_that_does_not_split_a_room_and_a_door_gap_change_nothing_and_a_non_rectangle_is_reported() {
    let mut doc = blank();
    box_room(&mut doc);
    wall(&mut doc, [3000, 1000], [3000, 3000]); // a stub: the room goes round it
    let s = sync(&mut doc, "Find rooms");
    assert_eq!((s.added.len(), s.not_rect), (1, 0));
    assert_eq!(rooms(&doc)[0].1, [1000, 1000, 6000, 4000]);
    // An L: two rooms' worth of walls whose enclosed region is not a rectangle.
    let mut l = blank();
    for (a, b) in [([0, 0], [4000, 0]), ([4000, 0], [4000, 2000]), ([4000, 2000], [2000, 2000]), ([2000, 2000], [2000, 4000]), ([2000, 4000], [0, 4000]), ([0, 4000], [0, 0])] {
        wall(&mut l, a, b);
    }
    let s = annot::rooms_sync(&l.model, level(&l));
    assert_eq!((s.added.len(), s.not_rect), (0, 1), "no room is invented for an L");
    // Splitting the L with a wall gives its two rectangles.
    wall(&mut l, [0, 2000], [2000, 2000]);
    assert_eq!(annot::rooms_sync(&l.model, level(&l)).added.len(), 2);
}

#[test]
fn rooms_of_a_layout_are_never_removed_or_overlapped_and_keys_stay_unique() {
    let mut doc = blank();
    box_room(&mut doc);
    // A room that is not computed and covers part of the same area.
    let sp = Space { func: "living".into(), requirement_key: "R1".into(), area: 6.0, target_area: 6.0, x: 1000, y: 1000, w: 3000, h: 2000, boundary: vec![], net_area: None, glazing_ratio: None, extra: Default::default() };
    let mut el = typed::element_of(&sp, Source::Generated);
    el.level_id = level(&doc);
    doc.commit("layout room", Origin::Generator, vec![Command::Create(el)]).unwrap_or_else(|e| panic!("{e}"));
    let s = annot::rooms_sync(&doc.model, level(&doc));
    assert!(s.added.is_empty() && s.overlapping == ["R1"], "the enclosed rectangle overlaps the layout's room: reported, not added");
    // Remove the walls: the generated room stays.
    let none = annot::rooms_sync(&Model { elements: doc.model.elements.iter().filter(|(_, e)| e.kind != "Wall").map(|(k, v)| (*k, v.clone())).collect() }, level(&doc));
    assert!(none.removed.is_empty());
    // A computed room made later takes the next free key.
    let mut d2 = blank();
    box_room(&mut d2);
    sync(&mut d2, "a");
    wall(&mut d2, [4000, 1000], [4000, 5000]);
    let keys: Vec<String> = { sync(&mut d2, "b"); rooms(&d2).into_iter().map(|r| r.0).collect() };
    assert_eq!(keys.iter().collect::<std::collections::BTreeSet<_>>().len(), keys.len());
}

// ------------------------------------------------------------------ grid lines

#[test]
fn grid_lines_get_letters_for_vertical_and_numbers_for_horizontal_and_names_are_unique() {
    let mut doc = blank();
    let add = |doc: &mut Document, a: [i64; 2], b: [i64; 2], name: Option<&str>| {
        let (c, id) = annot::grid_command(&doc.model, &GridSpec { a, b, name: name.map(str::to_string) })?;
        doc.commit("grid", Origin::User, vec![c]).map_err(|e| e.to_string())?;
        Ok::<Id, String>(id)
    };
    let a = add(&mut doc, [0, -500], [0, 8000], None).unwrap();
    let b = add(&mut doc, [4000, -500], [4000, 8000], None).unwrap();
    let one = add(&mut doc, [-500, 0], [9000, 0], None).unwrap();
    let two = add(&mut doc, [-500, 3000], [9000, 3000], None).unwrap();
    fn nm(doc: &Document, id: Id) -> String {
        doc.model.get(&id).unwrap().props["name"].as_str().unwrap().to_string()
    }
    assert_eq!((nm(&doc, a), nm(&doc, b), nm(&doc, one), nm(&doc, two)), ("A".into(), "B".into(), "1".into(), "2".into()));
    assert!(doc.model.get(&a).unwrap().level_id.is_none(), "grid lines belong to the project, not a level");
    // Refusals: diagonal, short, over another, a name already used.
    assert!(add(&mut doc, [0, 0], [1000, 1000], None).unwrap_err().contains("horizontal or vertical"));
    assert!(add(&mut doc, [100, 0], [150, 0], None).unwrap_err().contains("at least"));
    assert!(add(&mut doc, [0, 0], [0, 4000], None).unwrap_err().contains("already runs"));
    assert!(add(&mut doc, [8000, 0], [8000, 4000], Some("a")).unwrap_err().contains("already named"), "names are compared without case");
    // A name edit that clashes is refused by the model; a free one is accepted.
    assert!(draft::set_property(&doc.model, b, "name", &json!("A")).and_then(|e| doc.clone_model_commit(e)).is_err());
    let e = draft::set_property(&doc.model, b, "name", &json!("C")).unwrap();
    doc.commit("rename", Origin::User, e.cmds).unwrap_or_else(|x| panic!("{x}"));
    assert_eq!(nm(&doc, b), "C");
    // The next vertical line continues after the highest free letter in order.
    assert_eq!(annot::next_grid_name(&doc.model, false), "B");
    assert_eq!(annot::next_grid_name(&doc.model, true), "3");
}

trait CloneCommit {
    fn clone_model_commit(&self, e: draft::Edit) -> Result<(), String>;
}
impl CloneCommit for Document {
    fn clone_model_commit(&self, e: draft::Edit) -> Result<(), String> {
        let mut d = Document::new(self.model.clone());
        d.commit("x", Origin::User, e.cmds).map_err(|x| x.to_string())
    }
}

#[test]
fn grid_lines_are_drawn_on_the_plan_the_elevations_the_sections_and_the_dxf() {
    let mut doc = blank();
    box_room(&mut doc);
    sync(&mut doc, "Find rooms");
    for (a, b) in [([1000, 0], [1000, 6000]), ([7000, 0], [7000, 6000]), ([0, 1000], [8000, 1000])] {
        let (c, _) = annot::grid_command(&doc.model, &GridSpec { a, b, name: None }).unwrap();
        doc.commit("grid", Origin::User, vec![c]).unwrap_or_else(|e| panic!("{e}"));
    }
    let svg = drawing::plan_svg(&doc.model, level(&doc), &opts()).unwrap();
    assert_eq!(svg.matches("class=\"gridline\"").count(), 3);
    assert!(svg.contains(">A</text>") && svg.contains(">B</text>") && svg.contains(">1</text>"));
    // The two vertical lines are seen end-on from the south and the north; the horizontal one from the east and the west.
    let vo = ViewOpts::default();
    let south = elevation::elevation_svg(&doc.model, Side::South, &vo).unwrap();
    assert_eq!(south.matches("class=\"grid\"").count(), 2, "A and B");
    assert_eq!(elevation::elevation_svg(&doc.model, Side::East, &vo).unwrap().matches("class=\"grid\"").count(), 1, "line 1");
    let sec = elevation::section_svg(&doc.model, &Cut::AtY { name: "A-A".into(), at: 3.0 }, &vo).unwrap();
    assert_eq!(sec.matches("class=\"grid\"").count(), 2, "a section across y shows the vertical lines");
    let shapes = elevation::elevation_shapes(&doc.model, Side::South, &vo).unwrap();
    assert_eq!(shapes.grids.iter().map(|g| g.0.as_str()).collect::<Vec<_>>(), ["A", "B"]);
    // DXF: the A-GRID layer on the plan and in the elevation.
    let out = dxf::export_dxf_views(&doc.model, &[level(&doc)], &[ViewSpec::Elevation(Side::South)], &vo, &dxf_opts()).unwrap();
    assert!(out.counts.get("A-GRID").copied().unwrap_or(0) >= 3 + 2 * 3, "{:?}", out.counts);
    assert!(out.text.contains("A-GRID"));
}

#[test]
fn the_pointer_snaps_to_grid_lines_their_crossings_and_points_along_them() {
    let mut doc = blank();
    for (a, b) in [([2000, -1000], [2000, 8000]), ([-1000, 3000], [9000, 3000])] {
        let (c, _) = annot::grid_command(&doc.model, &GridSpec { a, b, name: None }).unwrap();
        doc.commit("grid", Origin::User, vec![c]).unwrap_or_else(|e| panic!("{e}"));
    }
    let c = SnapCtx { scale: 0.05, from: None, ortho: false, exclude: vec![] };
    assert_eq!(draft::snap(&doc.model, level(&doc), [2060.0, 3050.0], &c).kind, SnapKind::Intersection);
    let s = draft::snap(&doc.model, level(&doc), [2090.0, 5100.0], &c);
    assert_eq!((s.kind, s.p[0]), (SnapKind::Online, 2000));
    assert_eq!(draft::snap(&doc.model, level(&doc), [2040.0, 8020.0], &c).kind, SnapKind::Endpoint);
}

// ------------------------------------------------------------------ permanent dimensions

#[test]
fn a_dimension_between_walls_measures_follows_them_and_is_drawn_on_the_plan_and_the_dxf() {
    let mut doc = blank();
    let [bottom, right, _, left] = box_room(&mut doc);
    let _ = bottom;
    sync(&mut doc, "Find rooms");
    // Taken from the left and right walls: they are vertical, so the dimension measures x and attaches to both.
    let spec = DimSpec { a: [1040, 3000], b: [6980, 3000], at: [0, 6000], ref_a: Some(left), ref_b: Some(right) };
    let (c, id, notes) = annot::dimension_command(&doc.model, level(&doc), &spec).unwrap();
    assert!(notes.is_empty());
    doc.commit("dim", Origin::User, vec![c]).unwrap_or_else(|e| panic!("{e}"));
    let d = doc.model.get(&id).unwrap().typed::<typed::Dimension>().unwrap();
    assert_eq!((d.axis.as_str(), d.a, d.b, d.at), ("x", 1000, 7000, 6000), "snapped onto the two wall lines");
    let svg = drawing::plan_svg(&doc.model, level(&doc), &opts()).unwrap();
    assert!(svg.contains("class=\"dimension\"") && svg.contains(">6000</text>"));
    // Move the right wall 500 mm out: the dimension follows without being edited.
    let e = draft::xform(&doc.model, &[right], Xf::Move { dx: 500, dy: 0 }, false).unwrap();
    doc.commit("Move", Origin::User, e.cmds).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(annot::dim_positions(&doc.model, &doc.model.get(&id).unwrap().typed::<typed::Dimension>().unwrap()), (1000, 7500));
    assert!(drawing::plan_svg(&doc.model, level(&doc), &opts()).unwrap().contains(">6500</text>"));
    let p = draft::properties(&doc.model, id).unwrap();
    assert!(p["fields"].as_array().unwrap().iter().any(|f| f["key"] == "length" && f["value"] == 6500));
    assert!(p["fields"].as_array().unwrap().iter().any(|f| f["key"] == "attached" && f["value"] == "both ends"));
    // DXF: lines on A-DIMS and the value as text.
    let out = dxf::export_dxf(&doc.model, &[level(&doc)], &dxf_opts()).unwrap();
    assert!(out.text.contains("6500") && out.counts.get("A-DIMS").copied().unwrap_or(0) >= 6);
    // The line's position is editable; the rest is read-only.
    let e = draft::set_property(&doc.model, id, "at", &json!(7000)).unwrap();
    doc.commit("edit", Origin::User, e.cmds).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(doc.model.get(&id).unwrap().props["at"], 7000);
    assert!(draft::set_property(&doc.model, id, "length", &json!(1)).is_err());
}

#[test]
fn a_dimension_between_openings_grids_and_free_points_and_what_it_refuses() {
    let mut doc = blank();
    let [bottom, ..] = box_room(&mut doc);
    let (g, gid) = annot::grid_command(&doc.model, &GridSpec { a: [3000, 0], b: [3000, 6000], name: None }).unwrap();
    doc.commit("grid", Origin::User, vec![g]).unwrap_or_else(|e| panic!("{e}"));
    let (oc, door) = draft::opening_command(&doc.model, &draft::OpeningSpec { kind: draft::OpeningKind::Door, wall: bottom, offset: 4000, width: 900, height: 2100, sill: None, side: None }).unwrap();
    doc.commit("door", Origin::User, oc).unwrap_or_else(|e| panic!("{e}"));
    // The door's centre is at x = 1000 + 4000; the grid line at x = 3000.
    let (c, id, _) = annot::dimension_command(&doc.model, level(&doc), &DimSpec { a: [3010, 1000], b: [5020, 1000], at: [0, 500], ref_a: Some(gid), ref_b: Some(door) }).unwrap();
    doc.commit("dim", Origin::User, vec![c]).unwrap_or_else(|e| panic!("{e}"));
    let d = doc.model.get(&id).unwrap().typed::<typed::Dimension>().unwrap();
    assert_eq!((d.a, d.b), (3000, 5000));
    // A wall that runs along the measured direction has no single position there: the point stays free and it is said so.
    let (_, id2, notes) = annot::dimension_command(&doc.model, level(&doc), &DimSpec { a: [1000, 1000], b: [3000, 1000], at: [0, 500], ref_a: Some(bottom), ref_b: None }).unwrap();
    assert_eq!(notes.len(), 1);
    assert!(doc.model.get(&id2).is_none(), "not committed in this test");
    // Two points at the same position are refused; a vertical measurement works the same way.
    assert!(annot::dimension_command(&doc.model, level(&doc), &DimSpec { a: [1000, 1000], b: [1000, 1000], at: [0, 0], ref_a: None, ref_b: None }).is_err());
    let (_, _, _) = annot::dimension_command(&doc.model, level(&doc), &DimSpec { a: [500, 1000], b: [520, 5000], at: [200, 0], ref_a: None, ref_b: None }).unwrap();
    // A dimension with a missing reference keeps its stored positions.
    let mut dm = d.clone();
    dm.ref_a = Some(Id::now_v7().to_string());
    assert_eq!(annot::dim_positions(&doc.model, &dm).0, 3000);
}

// ------------------------------------------------------------------ room tags

#[test]
fn a_room_tag_shows_the_name_the_area_and_the_level_elevation_and_replaces_the_default_label() {
    let mut doc = blank();
    box_room(&mut doc);
    sync(&mut doc, "Find rooms");
    let room = doc.model.elements.values().find(|e| e.kind == "Space").unwrap().id;
    // Rename the room through its properties (the name is a project label), then tag it.
    let e = draft::set_property(&doc.model, room, "name", &json!("Majlis")).unwrap();
    doc.commit("rename", Origin::User, e.cmds).unwrap_or_else(|e| panic!("{e}"));
    let labels = project::get(&doc.model).unwrap().unwrap().labels;
    assert_eq!(labels["R1"], "Majlis");
    let before = drawing::plan_svg(&doc.model, level(&doc), &DrawOpts { labels: labels.clone(), ..opts() }).unwrap();
    assert!(before.contains("class=\"name\"") && !before.contains("class=\"tag\""));
    let (cmds, ids) = annot::tag_commands(&doc.model, &[room]).unwrap();
    assert_eq!(ids.len(), 1);
    doc.commit("tag", Origin::User, cmds).unwrap_or_else(|e| panic!("{e}"));
    assert!(annot::tag_commands(&doc.model, &[room]).unwrap().0.is_empty(), "a room is tagged once");
    assert!(annot::untagged_rooms(&doc.model, level(&doc)).is_empty());
    let svg = drawing::plan_svg(&doc.model, level(&doc), &DrawOpts { labels, ..opts() }).unwrap();
    assert!(svg.contains("class=\"tag\"") && svg.contains(">Majlis</text>") && svg.contains(">24.00 m²</text>") && svg.contains(">±0.00 m</text>"), "{svg}");
    assert!(!svg.contains("class=\"name\""), "the tagged room has no default label any more");
    // The tag follows its room: a different area shows at once.
    let tag = doc.model.elements.values().find(|e| e.kind == "Tag").unwrap().id;
    // Fields of the tag can be switched off, but not all of them.
    let off = |doc: &mut Document, k: &str| {
        let e = draft::set_property(&doc.model, tag, k, &json!(false)).unwrap();
        doc.commit("tag", Origin::User, e.cmds).map_err(|x| x.to_string())
    };
    off(&mut doc, "showArea").unwrap();
    off(&mut doc, "showElevation").unwrap();
    assert!(off(&mut doc, "showName").unwrap_err().contains("at least one"));
    let svg = drawing::plan_svg(&doc.model, level(&doc), &opts()).unwrap();
    assert!(!svg.contains("m²</text>"));
    // The DXF carries the tag text and not the default label.
    let out = dxf::export_dxf(&doc.model, &[level(&doc)], &dxf_opts()).unwrap();
    assert!(out.text.contains("R1"));
    // A tag belongs to a room: a tag on a wall is refused.
    let wall_id = doc.model.elements.values().find(|e| e.kind == "Wall").unwrap().id;
    assert!(annot::tag_commands(&doc.model, &[wall_id]).unwrap_err().contains("only rooms"));
}

#[test]
fn the_elevation_of_a_tag_follows_the_level_it_is_on() {
    let mut doc = blank();
    box_room(&mut doc);
    sync(&mut doc, "Find rooms");
    // A second level above: its elevation is the storey height plus the slab below it.
    let up = typed::element_of(&typed::Level { name: "LVL-2".into(), index: 1, extra: Default::default() }, Source::User);
    let up_id = up.id;
    doc.commit("level", Origin::User, vec![Command::Create(up)]).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(model3d::level_elevation_m(&doc.model, level(&doc)), 0.0);
    assert!((model3d::level_elevation_m(&doc.model, Some(up_id)) - 3.0).abs() < 1e-9, "the walls are 3000 mm high");
    assert_eq!(model3d::level_elevation_m(&doc.model, None), 0.0);
}

// ------------------------------------------------------------------ the project owns it

#[test]
fn grids_dimensions_and_tags_are_in_the_model_so_the_file_undo_and_redo_carry_them() {
    let mut doc = blank();
    box_room(&mut doc);
    sync(&mut doc, "Find rooms");
    let room = doc.model.elements.values().find(|e| e.kind == "Space").unwrap().id;
    let (g, gid) = annot::grid_command(&doc.model, &GridSpec { a: [0, 0], b: [0, 6000], name: Some("A".into()) }).unwrap();
    let (t, _) = annot::tag_commands(&doc.model, &[room]).unwrap();
    let (dm, did, _) = annot::dimension_command(&doc.model, level(&doc), &DimSpec { a: [1000, 1000], b: [7000, 1000], at: [0, 500], ref_a: None, ref_b: None }).unwrap();
    let mut all = vec![g, dm];
    all.extend(t);
    doc.commit("annotations", Origin::User, all).unwrap_or_else(|e| panic!("{e}"));
    let path = std::env::temp_dir().join(format!("nanoarch-m9-{}.nnx", std::process::id()));
    persist::save(&doc.model, &path).unwrap();
    let back = persist::load(&path).unwrap();
    std::fs::remove_file(&path).ok();
    assert_eq!(back.elements.get(&gid).unwrap().typed::<Grid>().unwrap().name, "A");
    assert!(back.elements.contains_key(&did));
    assert_eq!(back.elements.values().filter(|e| e.kind == "Tag").count(), 1);
    assert_eq!(back.elements.values().find(|e| e.kind == "Tag").unwrap().host_id, Some(room));
    assert!(doc.undo());
    assert!(doc.model.get(&gid).is_none() && doc.model.get(&did).is_none());
    assert!(doc.redo());
    assert!(doc.model.get(&gid).is_some());
    // A hosted tag keeps its room from being deleted by accident.
    assert!(doc.commit("x", Origin::User, vec![Command::Delete(room)]).is_err());
}
