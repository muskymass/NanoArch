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
fn a_drafted_room_has_a_net_area_between_the_wall_faces_and_it_follows_the_walls() {
    let mut doc = blank();
    let walls = box_room(&mut doc);
    sync(&mut doc, "rooms");
    let room = |doc: &Document| doc.model.elements.values().find(|e| e.kind == "Space").unwrap().typed::<Space>().unwrap();
    // 6 x 4 m between the wall centre lines; the walls are 200 mm, so each side comes in by 100 mm: 5.8 x 3.8 m.
    assert_eq!((room(&doc).area, room(&doc).net_area), (24.0, Some(22.04)));
    assert_eq!(annot::net_area_of(&doc.model, level(&doc), &[1000, 1000, 6000, 4000]), Some(22.04));
    // A second pass has nothing to do.
    assert!(sync(&mut doc, "again").cmds.is_empty());
    // One wall made 400 mm thick takes another 100 mm off that side: 5.8 x 3.7 m. The room is the same room, its net area follows.
    doc.commit("thicker", Origin::User, vec![Command::SetProp { id: walls[0], key: "thickness".into(), value: Some(json!(400)) }]).unwrap_or_else(|e| panic!("{e}"));
    let s = sync(&mut doc, "after the thicker wall");
    assert!(s.added.is_empty() && s.removed.is_empty(), "{s:?}");
    assert_eq!((room(&doc).area, room(&doc).net_area), (24.0, Some(21.46)));
    // With no wall on any side there is nothing to take off.
    assert_eq!(annot::net_area_of(&doc.model, level(&doc), &[20_000, 20_000, 3000, 3000]), None);
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
    assert!(svg.contains("class=\"tag\"") && svg.contains(">Majlis</text>") && svg.contains(">22.04 m²</text>") && svg.contains(">±0.00 m</text>"), "{svg}");
    // The tag shows the NET area: 6 x 4 m between the wall centre lines, 5.8 x 3.8 m between the faces of the 200 mm walls.
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

// ------------------------------------------------------------------ room separators across the rooms of a layout

fn layout_room(doc: &mut Document, key: &str, func: &str, r: [i64; 4]) -> Id {
    let sp = Space { func: func.into(), requirement_key: key.into(), area: (r[2] * r[3]) as f64 / 1e6, target_area: 8.0, x: r[0], y: r[1], w: r[2], h: r[3], boundary: vec![[r[0], r[1]], [r[0] + r[2], r[1]], [r[0] + r[2], r[1] + r[3]], [r[0], r[1] + r[3]]], net_area: None, glazing_ratio: None, extra: Default::default() };
    let mut el = typed::element_of(&sp, Source::Generated);
    el.level_id = level(doc);
    let id = el.id;
    doc.commit("layout room", Origin::Generator, vec![Command::Create(el)]).unwrap_or_else(|e| panic!("{e}"));
    id
}

fn separator(doc: &mut Document, a: [i64; 2], b: [i64; 2]) -> (Id, annot::RoomSync) {
    let (cmd, id) = annot::separator_command(&doc.model, level(doc), (a[0], a[1]), (b[0], b[1])).unwrap_or_else(|e| panic!("{e}"));
    let mut scratch = Document::new(doc.model.clone());
    scratch.commit("x", Origin::User, vec![cmd.clone()]).unwrap_or_else(|e| panic!("{e}"));
    let s = annot::rooms_sync(&scratch.model, level(doc));
    let mut cmds = vec![cmd];
    cmds.extend(s.cmds.clone());
    doc.commit("Draw room separator", Origin::User, cmds).unwrap_or_else(|e| panic!("{e}"));
    (id, s)
}

#[test]
fn a_separator_across_a_layout_room_divides_it_keeps_its_tag_and_openings_and_gives_the_room_back_when_it_goes() {
    let mut doc = blank();
    let room = layout_room(&mut doc, "living", "living", [0, 0, 9000, 4000]);
    let (tc, _) = annot::tag_commands(&doc.model, &[room]).unwrap();
    doc.commit("tag", Origin::User, tc).unwrap_or_else(|e| panic!("{e}"));
    let n_before = doc.model.len();
    // Two separators that cross each other cut it into four; one is enough for two. First one.
    let (sep, s) = separator(&mut doc, [3000, 0], [3000, 4000]);
    assert_eq!(s.added.len(), 2);
    let all = rooms(&doc);
    assert_eq!(all.len(), 2);
    let big = all.iter().find(|r| r.0 == "living").unwrap();
    let small = all.iter().find(|r| r.0 == "living-2").unwrap();
    assert_eq!((big.1, small.1), ([3000, 0, 6000, 4000], [0, 0, 3000, 4000]));
    assert!(all.iter().all(|r| r.2 == Source::Generated));
    fn sp(doc: &Document, k: &str) -> Space {
        doc.model.elements.values().find(|e| e.kind == "Space" && e.props["requirementKey"] == k).unwrap().typed::<Space>().unwrap()
    }
    assert_eq!((sp(&doc, "living").func.as_str(), sp(&doc, "living-2").func.as_str()), ("living", "room"));
    assert_eq!(sp(&doc, "living-2").target_area, 12.0, "the part's own area, not the original's target");
    assert_eq!(doc.model.elements.values().filter(|e| e.kind == "Tag").count(), 2, "each part is tagged, as the room was");
    // A second separator crossing the first divides the big part again.
    let (sep2, _) = separator(&mut doc, [0, 2000], [9000, 2000]);
    assert_eq!(rooms(&doc).len(), 4);
    // Deleting the first leaves the horizontal one: the room is two halves, one above the other.
    let mut cmds = draft::delete(&doc.model, &[sep]).unwrap().cmds;
    let mut scratch = Document::new(doc.model.clone());
    scratch.commit("x", Origin::User, cmds.clone()).unwrap_or_else(|e| panic!("{e}"));
    cmds.extend(annot::rooms_sync(&scratch.model, level(&doc)).cmds);
    doc.commit("Delete", Origin::User, cmds).unwrap_or_else(|e| panic!("{e}"));
    let halves: Vec<[i64; 4]> = rooms(&doc).into_iter().map(|r| r.1).collect();
    assert_eq!(halves.len(), 2);
    assert!(halves.contains(&[0, 0, 9000, 2000]) && halves.contains(&[0, 2000, 9000, 2000]));
    // And deleting that one gives the room back as the layout made it.
    let mut cmds = draft::delete(&doc.model, &[sep2]).unwrap().cmds;
    let mut scratch = Document::new(doc.model.clone());
    scratch.commit("x", Origin::User, cmds.clone()).unwrap_or_else(|e| panic!("{e}"));
    let back = annot::rooms_sync(&scratch.model, level(&doc));
    assert!(back.notes.iter().any(|n| n.contains("one room again")));
    cmds.extend(back.cmds);
    doc.commit("Delete", Origin::User, cmds).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(rooms(&doc), vec![("living".to_string(), [0, 0, 9000, 4000], Source::Generated)]);
    assert_eq!(sp(&doc, "living").func, "living");
    assert_eq!(sp(&doc, "living").target_area, 8.0, "the layout's own target is back");
    assert!(sp(&doc, "living").extra.get("splitFrom").is_none());
    assert_eq!(doc.model.elements.values().filter(|e| e.kind == "Tag").count(), 1);
    // Each edit was one step: undoing walks back through the halves, the four parts and the two parts to the whole room.
    for expected in [2, 4, 2, 1] {
        assert!(doc.undo());
        assert_eq!(rooms(&doc).len(), expected);
    }
    assert_eq!(rooms(&doc), vec![("living".to_string(), [0, 0, 9000, 4000], Source::Generated)]);
    assert_eq!(doc.model.len(), n_before);
}

#[test]
fn a_locked_room_the_stair_block_and_a_separator_that_stops_short_are_not_divided_and_say_why() {
    let mut doc = blank();
    let locked = layout_room(&mut doc, "bedroom", "bedroom", [0, 0, 4000, 4000]);
    doc.commit("lock", Origin::User, vec![Command::SetLocked { id: locked, locked: true }]).unwrap_or_else(|e| panic!("{e}"));
    layout_room(&mut doc, "stair", "stair", [4000, 0, 3000, 4000]);
    layout_room(&mut doc, "hall", "hall", [7000, 0, 4000, 4000]);
    let (_, s) = separator(&mut doc, [2000, 0], [2000, 4000]);
    assert!(s.cmds.is_empty() && s.blocked.iter().any(|b| b.contains("'bedroom'") && b.contains("locked")), "{:?}", s.blocked);
    let (_, s) = separator(&mut doc, [5500, 0], [5500, 4000]);
    assert!(s.blocked.iter().any(|b| b.contains("'stair'") && b.contains("stair block")), "{:?}", s.blocked);
    let (_, s) = separator(&mut doc, [9000, 0], [9000, 2000]);
    assert!(s.blocked.iter().any(|b| b.contains("'hall'") && b.contains("does not reach both sides")), "{:?}", s.blocked);
    assert_eq!(rooms(&doc).len(), 3, "nothing was divided");
}

// ------------------------------------------------------------------ a divided layout room: openings, MEP, numbers, exports and the file

fn qto_numbers(doc: &Document) -> Vec<(String, f64)> {
    let q = qto::compute(&doc.model, &qto::QtoOpts::default());
    q.items.iter().map(|i| (i.code.clone(), i.quantity)).collect()
}

fn prop(doc: &Document, id: Id, key: &str) -> serde_json::Value {
    doc.model.get(&id).unwrap().props[key].clone()
}

fn rooms_area(doc: &Document) -> f64 {
    doc.model.elements.values().filter(|e| e.kind == "Space").map(|e| e.typed::<Space>().unwrap().area).sum()
}

/// A 9 x 4 m layout room 'living' with a wall along its south side, a window and a door in the part x < 3000, a window in the rest, and
/// two MEP points, one in each part.
fn living_with_openings() -> (Document, [Id; 3], [Id; 2]) {
    let mut doc = blank();
    layout_room(&mut doc, "living", "living", [0, 0, 9000, 4000]);
    let level = level(&doc);
    let (wc, wall_id) = draft::wall_command(&doc.model, level, &WallSpec { a: [0, 0], b: [9000, 0], thickness: 200, height: 3000 }).unwrap_or_else(|e| panic!("{e}"));
    doc.commit("wall", Origin::User, vec![wc]).unwrap_or_else(|e| panic!("{e}"));
    let mut ids = Vec::new();
    for (kind, offset, width, sill) in [(draft::OpeningKind::Window, 1000, 1200, Some(900)), (draft::OpeningKind::Window, 6000, 1200, Some(900)), (draft::OpeningKind::Door, 2200, 900, None)] {
        let height = if sill.is_some() { 1200 } else { 2100 };
        let (cmds, id) = draft::opening_command(&doc.model, &draft::OpeningSpec { kind, wall: wall_id, offset, width, height, sill, side: Some(1) }).unwrap_or_else(|e| panic!("{e}"));
        doc.commit("opening", Origin::User, cmds).unwrap_or_else(|e| panic!("{e}"));
        // The openings name the room explicitly, as those of a generated layout do.
        let fields: &[&str] = if sill.is_some() { &["room"] } else { &["from", "opensInto"] };
        let sets = fields.iter().map(|f| Command::SetProp { id, key: (*f).into(), value: Some(json!("living")) }).collect();
        doc.commit("name the room", Origin::User, sets).unwrap_or_else(|e| panic!("{e}"));
        ids.push(id);
    }
    let mut pts = Vec::new();
    for (x, y) in [(1500, 2000), (6000, 2000)] {
        let mut el = typed::element_of(&typed::MepPoint { role: "light".into(), x, y, room: "living".into(), watts: None, circuit: None, label: None, extra: Default::default() }, Source::Generated);
        el.level_id = level;
        pts.push(el.id);
        doc.commit("point", Origin::Generator, vec![Command::Create(el)]).unwrap_or_else(|e| panic!("{e}"));
    }
    doc.mark_saved();
    (doc, [ids[0], ids[1], ids[2]], [pts[0], pts[1]])
}

#[test]
fn dividing_a_layout_room_sends_each_opening_and_mep_point_to_the_part_it_stands_in_and_the_numbers_follow() {
    let (mut doc, [w_left, w_right, door], [p_left, p_right]) = living_with_openings();
    let qto_before = qto_numbers(&doc);
    let skipped_before = rules::check(&doc.model, &rules::RuleSet::default()).skipped.len();
    let area_before = rooms_area(&doc);
    assert_eq!(area_before, 36.0);
    let before3d = model3d::build(&doc.model).boxes.iter().filter(|b| b.kind == "Space").count();
    let (sep, s) = separator(&mut doc, [3000, 0], [3000, 4000]);
    assert!(s.notes.iter().any(|n| n.contains("was divided")), "{:?}", s.notes);
    // Left of the separator is 'living-2'; the rest keeps 'living'.
    assert_eq!(prop(&doc, w_left, "room"), "living-2");
    assert_eq!(prop(&doc, w_right, "room"), "living");
    assert_eq!((prop(&doc, door, "from"), prop(&doc, door, "opensInto")), (json!("living-2"), json!("living-2")));
    assert_eq!((prop(&doc, p_left, "room"), prop(&doc, p_right, "room")), (json!("living-2"), json!("living")));
    // Areas add up to the room's, exactly: 12 + 24 = 36 m².
    assert_eq!(rooms_area(&doc), area_before);
    let by_key: std::collections::BTreeMap<String, f64> = doc.model.elements.values().filter(|e| e.kind == "Space").map(|e| { let s = e.typed::<Space>().unwrap(); (s.requirement_key, s.area) }).collect();
    assert_eq!((by_key["living"], by_key["living-2"]), (24.0, 12.0));
    // Take-off: walls, openings and the gross area are what they were. Floor and ceiling split by function (24 m² 'living', 12 m² 'room'),
    // and the skirting is the same length in all: no skirting runs along the separator, which is not a wall.
    let after = qto_numbers(&doc);
    let get = |v: &[(String, f64)], c: &str| v.iter().find(|i| i.0 == c).map(|i| i.1);
    for code in ["WALL.EXT.AREA", "WALL.EXT.VOL", "WALL.OPENINGS.AREA", "WALL.FACES.AREA", "DOOR.900x2100", "WINDOW.1200x1200", "WINDOW.AREA", "SPACE.GROSS.AREA"] {
        assert_eq!(get(&after, code), get(&qto_before, code), "{code}");
    }
    assert_eq!((get(&after, "FLOOR.AREA.living"), get(&after, "FLOOR.AREA.room"), get(&after, "CEILING.AREA.living"), get(&after, "CEILING.AREA.room")), (Some(24.0), Some(12.0), Some(24.0), Some(12.0)));
    let skirting_before = get(&qto_before, "SKIRTING.living").unwrap();
    let skirting_after = get(&after, "SKIRTING.living").unwrap() + get(&after, "SKIRTING.room").unwrap();
    assert!((skirting_before - skirting_after).abs() < 0.011, "{skirting_before} vs {skirting_after}");
    assert!(qto::compute(&doc.model, &qto::QtoOpts::default()).warnings.iter().any(|w| w.contains("room separators")));
    // The audit skips what it skipped before and applies no rule of a function to the new part.
    let report = rules::check(&doc.model, &rules::RuleSet::default());
    assert_eq!(report.skipped.len(), skipped_before);
    assert!(report.findings.iter().all(|f| !format!("{f:?}").contains("living-2")), "no rule applies to the part until it is given a function");
    // 3D: one box per room, now two.
    let after3d = model3d::build(&doc.model).boxes.iter().filter(|b| b.kind == "Space").count();
    assert_eq!((before3d, after3d), (1, 2));
    // DXF: the separator is on its layer and the part is named.
    let out = dxf::export_dxf(&doc.model, &[level(&doc)], &dxf_opts()).unwrap();
    assert_eq!(out.counts.get("A-ROOM-SEP"), Some(&1));
    assert!(out.text.contains("living-2"), "the part's name is in the drawing");
    // One undo gives every reference back to the whole room.
    assert!(doc.undo());
    assert_eq!((prop(&doc, w_left, "room"), prop(&doc, door, "from"), prop(&doc, p_left, "room")), (json!("living"), json!("living"), json!("living")));
    assert!(doc.model.get(&sep).is_none() && rooms(&doc).len() == 1);
}

#[test]
fn a_divided_room_survives_the_file_undo_and_redo_and_is_clean_after_reopening() {
    let (mut doc, [w_left, ..], [p_left, _]) = living_with_openings();
    let (sep, _) = separator(&mut doc, [3000, 0], [3000, 4000]);
    assert!(doc.is_dirty());
    let path = std::env::temp_dir().join(format!("nanoarch-split-{}.nnx", std::process::id()));
    persist::save(&doc.model, &path).unwrap();
    doc.mark_saved();
    assert!(!doc.is_dirty());
    let loaded = persist::load(&path).unwrap();
    std::fs::remove_file(&path).ok();
    assert_eq!(loaded.state_hash(), doc.model.state_hash(), "the file holds exactly the divided model");
    let mut back = Document::new(loaded);
    assert!(!back.is_dirty(), "a reopened project is clean");
    assert!(!back.undo(), "history starts at the file");
    // Both parts, their link to the original and the separator are there.
    assert_eq!(rooms(&back).len(), 2);
    let part = back.model.elements.values().find(|e| e.kind == "Space" && e.props["requirementKey"] == "living-2").unwrap().typed::<Space>().unwrap();
    assert_eq!(part.extra["splitFrom"]["requirementKey"], "living");
    assert_eq!((prop(&back, w_left, "room"), prop(&back, p_left, "room")), (json!("living-2"), json!("living-2")));
    assert!(back.model.get(&sep).is_some());
    // Nothing is left to do: the reopened plan is already divided.
    let again = annot::rooms_sync(&back.model, level(&back));
    assert!(again.cmds.is_empty(), "a second pass changes nothing: {:?}", again.notes);
    // Deleting the separator in the reopened project gives the room back as one step; undo and redo walk it.
    let divided_hash = back.model.state_hash();
    let mut cmds = draft::delete(&back.model, &[sep]).unwrap().cmds;
    let mut scratch = Document::new(back.model.clone());
    scratch.commit("x", Origin::User, cmds.clone()).unwrap_or_else(|e| panic!("{e}"));
    cmds.extend(annot::rooms_sync(&scratch.model, level(&back)).cmds);
    back.commit("Delete", Origin::User, cmds).unwrap_or_else(|e| panic!("{e}"));
    assert!(back.is_dirty());
    assert_eq!(rooms(&back), vec![("living".to_string(), [0, 0, 9000, 4000], Source::Generated)]);
    assert_eq!((prop(&back, w_left, "room"), prop(&back, p_left, "room")), (json!("living"), json!("living")));
    let whole_hash = back.model.state_hash();
    assert!(back.undo());
    assert_eq!(back.model.state_hash(), divided_hash);
    assert!(!back.is_dirty(), "back at the saved state it is clean");
    assert!(back.redo());
    assert_eq!(back.model.state_hash(), whole_hash);
    // The whole room saved and reopened has no trace of the division.
    let path2 = std::env::temp_dir().join(format!("nanoarch-split2-{}.nnx", std::process::id()));
    persist::save(&back.model, &path2).unwrap();
    let whole = persist::load(&path2).unwrap();
    std::fs::remove_file(&path2).ok();
    assert!(whole.elements.values().filter(|e| e.kind == "Space").all(|e| e.props.get("splitFrom").is_none()));
}

#[test]
fn a_room_cut_into_four_parts_keeps_every_part_tag_area_and_link_through_the_file_and_its_history() {
    let mut doc = blank();
    let room = layout_room(&mut doc, "living", "living", [0, 0, 9000, 4000]);
    let (tc, _) = annot::tag_commands(&doc.model, &[room]).unwrap();
    doc.commit("tag", Origin::User, tc).unwrap_or_else(|e| panic!("{e}"));
    let (sep_v, _) = separator(&mut doc, [3000, 0], [3000, 4000]);
    let (sep_h, _) = separator(&mut doc, [0, 2000], [9000, 2000]);
    let keys = |d: &Document| {
        let mut k: Vec<String> = d.model.elements.values().filter(|e| e.kind == "Space").map(|e| e.props["requirementKey"].as_str().unwrap().to_string()).collect();
        k.sort();
        k
    };
    assert_eq!(keys(&doc).len(), 4);
    assert!(keys(&doc).contains(&"living".to_string()) && keys(&doc).contains(&"living-2".to_string()) && keys(&doc).contains(&"living-3".to_string()));
    let path = std::env::temp_dir().join(format!("nanoarch-split4-{}.nnx", std::process::id()));
    persist::save(&doc.model, &path).unwrap();
    doc.mark_saved();
    let loaded = persist::load(&path).unwrap();
    std::fs::remove_file(&path).ok();
    assert_eq!(loaded.state_hash(), doc.model.state_hash(), "the file holds exactly the divided model");
    let mut back = Document::new(loaded);
    assert!(!back.is_dirty() && !back.undo() && !back.redo(), "a reopened project is clean and its history starts at the file");
    assert_eq!(keys(&back), keys(&doc));
    // Every part but the one that kept the original's key remembers the room it came from, and the parts' areas add up to the room's.
    let spaces: Vec<Space> = back.model.elements.values().filter(|e| e.kind == "Space").map(|e| e.typed::<Space>().unwrap()).collect();
    for sp in &spaces {
        if sp.requirement_key != "living" {
            assert_eq!(sp.extra["splitFrom"]["requirementKey"], "living", "{} knows its origin", sp.requirement_key);
        }
        assert_eq!(sp.area, (sp.w * sp.h) as f64 / 1e6, "{} has its own area", sp.requirement_key);
    }
    assert_eq!(spaces.iter().map(|s| s.area).sum::<f64>(), 36.0);
    assert_eq!(back.model.elements.values().filter(|e| e.kind == "Tag").count(), 4, "each part has its tag");
    assert!(back.model.get(&sep_v).is_some() && back.model.get(&sep_h).is_some());
    assert!(annot::rooms_sync(&back.model, level(&back)).cmds.is_empty(), "the reopened plan is already divided");
    // Deleting both separators in the reopened project gives the whole room back, byte for byte, and undo walks back to the file.
    for sep in [sep_v, sep_h] {
        let mut cmds = draft::delete(&back.model, &[sep]).unwrap().cmds;
        let mut scratch = Document::new(back.model.clone());
        scratch.commit("x", Origin::User, cmds.clone()).unwrap_or_else(|e| panic!("{e}"));
        cmds.extend(annot::rooms_sync(&scratch.model, level(&back)).cmds);
        back.commit("Delete", Origin::User, cmds).unwrap_or_else(|e| panic!("{e}"));
    }
    assert_eq!(rooms(&back), vec![("living".to_string(), [0, 0, 9000, 4000], Source::Generated)]);
    assert!(back.model.elements.values().filter(|e| e.kind == "Space").all(|e| e.props.get("splitFrom").is_none()));
    assert_eq!(back.model.elements.values().filter(|e| e.kind == "Tag").count(), 1);
    assert!(back.undo() && back.undo());
    assert_eq!(keys(&back).len(), 4);
    assert!(!back.is_dirty(), "undone to the saved state it is clean again");
    assert!(back.redo() && back.redo());
    assert!(back.is_dirty() && keys(&back).len() == 1);
}

#[test]
fn the_dxf_of_a_divided_room_is_the_same_after_the_file_is_saved_and_reopened() {
    let mut doc = blank();
    layout_room(&mut doc, "living", "living", [0, 0, 9000, 4000]);
    let (sep, _) = separator(&mut doc, [3000, 0], [3000, 4000]);
    let before = dxf::export_dxf(&doc.model, &[level(&doc)], &dxf_opts()).unwrap();
    let path = std::env::temp_dir().join(format!("nanoarch-split-dxf-{}.nnx", std::process::id()));
    persist::save(&doc.model, &path).unwrap();
    let loaded = persist::load(&path).unwrap();
    std::fs::remove_file(&path).ok();
    let back = Document::new(loaded);
    let after = dxf::export_dxf(&back.model, &[level(&back)], &dxf_opts()).unwrap();
    // The separator is on its own layer, once, and is not a wall.
    assert_eq!(after.counts.get("A-ROOM-SEP"), Some(&1));
    assert!(back.model.get(&sep).is_some());
    assert_eq!(after.counts.get("A-ROOM"), Some(&2), "one outline per part");
    // Both parts are named and carry their own areas: 6 m x 4 m and 3 m x 4 m.
    for label in ["living", "living-2", "24.0 m2", "12.0 m2"] {
        assert!(after.text.contains(label), "the drawing has {label}");
    }
    // Reopening changes nothing in the drawing: same layers, same entities, same text.
    assert_eq!(after.counts, before.counts);
    assert_eq!(after.text, before.text);
}

#[test]
fn the_tag_of_a_divided_part_names_it_by_its_whole_key_and_its_own_area_with_no_stray_space() {
    let mut doc = blank();
    layout_room(&mut doc, "reserve", "bedroom", [0, 0, 9000, 4000]);
    separator(&mut doc, [3000, 0], [3000, 4000]);
    let (tc, _) = annot::tag_commands(&doc.model, &doc.model.elements.values().filter(|e| e.kind == "Space").map(|e| e.id).collect::<Vec<_>>()).unwrap();
    doc.commit("tag all", Origin::User, tc).unwrap_or_else(|e| panic!("{e}"));
    let none = std::collections::BTreeMap::new();
    let mut names: Vec<(String, String)> = doc
        .model
        .elements
        .values()
        .filter(|e| e.kind == "Tag")
        .map(|t| {
            let host = doc.model.get(&t.host_id.unwrap()).unwrap();
            let lines = annot::tag_lines(&doc.model, host, &t.typed().unwrap(), &none);
            (lines[0].clone(), lines[1].clone())
        })
        .collect();
    names.sort();
    // The part is named by its key exactly (every letter present, no space), and shows its own area, not the original's.
    assert_eq!(names, vec![("reserve".to_string(), "24.00 m²".to_string()), ("reserve-2".to_string(), "12.00 m²".to_string())]);
}

fn names_of_tags(doc: &Document, labels: &std::collections::BTreeMap<String, String>) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = doc
        .model
        .elements
        .values()
        .filter(|e| e.kind == "Tag")
        .map(|t| {
            let host = doc.model.get(&t.host_id.unwrap()).unwrap();
            let lines = annot::tag_lines(&doc.model, host, &t.typed().unwrap(), labels);
            (lines[0].clone(), lines[1].clone())
        })
        .collect();
    v.sort();
    v
}

#[test]
fn a_divided_part_is_shown_by_the_display_name_of_its_room_plus_its_number_everywhere_and_keeps_its_key() {
    let named: std::collections::BTreeMap<String, String> = [("living".to_string(), "Hall".to_string())].into();
    let mut doc = blank();
    layout_room(&mut doc, "living", "living", [0, 0, 9000, 4000]);
    let (sep_v, _) = separator(&mut doc, [3000, 0], [3000, 4000]);
    let part = |d: &Document, k: &str| d.model.elements.values().find(|e| e.kind == "Space" && e.props["requirementKey"] == k).unwrap().typed::<Space>().unwrap();
    // The name is derived, never stored: the part keeps its own key and the link to the room it came from.
    let p2 = part(&doc, "living-2");
    assert_eq!(p2.extra["splitFrom"]["requirementKey"], "living");
    assert_eq!((annot::room_name(&p2, &named).as_deref(), annot::room_name(&part(&doc, "living"), &named).as_deref()), (Some("Hall-2"), Some("Hall")));
    assert_eq!(annot::room_name(&p2, &Default::default()), None, "no display name anywhere: callers show the key");
    // Dividing a part again numbers the new part from the room the layout made (living-3), which it remembers as its origin: Hall-3.
    separator(&mut doc, [0, 2000], [3000, 2000]);
    let mut keys: Vec<String> = doc.model.elements.values().filter(|e| e.kind == "Space").map(|e| e.props["requirementKey"].as_str().unwrap().to_string()).collect();
    keys.sort();
    assert_eq!(keys, ["living", "living-2", "living-3"]);
    assert_eq!(part(&doc, "living-3").extra["splitFrom"]["requirementKey"], "living");
    assert_eq!(annot::room_name(&part(&doc, "living-3"), &named), Some("Hall-3".to_string()));
    // A display name the user gives a part wins over the derived one.
    let mut own = named.clone();
    own.insert("living-2".into(), "Study".into());
    assert_eq!(annot::room_name(&part(&doc, "living-2"), &own).as_deref(), Some("Study"));
    // Tags show the derived name and each part's own area.
    let all: Vec<Id> = doc.model.elements.values().filter(|e| e.kind == "Space").map(|e| e.id).collect();
    let (tc, _) = annot::tag_commands(&doc.model, &all).unwrap();
    doc.commit("tag", Origin::User, tc).unwrap_or_else(|e| panic!("{e}"));
    let tags = names_of_tags(&doc, &named);
    assert!(tags.iter().any(|t| t.0 == "Hall" && t.1 == "24.00 m²") && tags.iter().any(|t| t.0 == "Hall-3") && tags.iter().any(|t| t.0 == "Hall-2"), "{tags:?}");
    // DXF: tags carry the derived names; undo takes the parts and their names away, redo brings them back.
    let mut o = dxf_opts();
    o.labels = named.clone();
    let out = dxf::export_dxf(&doc.model, &[level(&doc)], &o).unwrap();
    assert!(out.text.contains("Hall-3") && out.text.contains("Hall-2") && !out.text.contains("living-2"), "the drawing uses the name, not the key");
    // The file keeps the parts; reopened, the same names appear and the DXF is the same drawing.
    let path = std::env::temp_dir().join(format!("nanoarch-names-{}.nnx", std::process::id()));
    persist::save(&doc.model, &path).unwrap();
    let back = Document::new(persist::load(&path).unwrap());
    std::fs::remove_file(&path).ok();
    assert_eq!(names_of_tags(&back, &named), tags);
    assert_eq!(dxf::export_dxf(&back.model, &[level(&back)], &o).unwrap().text, out.text);
    // Without a display name the key is shown, as before.
    assert!(names_of_tags(&back, &Default::default()).iter().any(|t| t.0 == "living-2"));
    // Undo and redo walk the division and its names: the room is whole and named `Hall` again, then divided again.
    for _ in 0..3 {
        assert!(doc.undo());
    }
    assert_eq!(doc.model.elements.values().filter(|e| e.kind == "Space").count(), 1);
    assert!(names_of_tags(&doc, &named).is_empty() || names_of_tags(&doc, &named).iter().all(|t| t.0 == "Hall"));
    for _ in 0..3 {
        assert!(doc.redo());
    }
    assert_eq!(names_of_tags(&doc, &named), tags);
    assert!(doc.model.get(&sep_v).is_some());
}

#[test]
fn part_names_in_the_ifc_follow_the_room_and_a_part_whose_key_no_longer_extends_its_origin_falls_back_to_its_key() {
    let mut named: std::collections::BTreeMap<String, String> = [("living".to_string(), "Hall".to_string())].into();
    let mut doc = blank();
    layout_room(&mut doc, "living", "living", [0, 0, 9000, 4000]);
    separator(&mut doc, [3000, 0], [3000, 4000]);
    let part = |d: &Document, k: &str| d.model.elements.values().find(|e| e.kind == "Space" && e.props["requirementKey"] == k).unwrap().typed::<Space>().unwrap();
    // IFC: the key is the space's Name, the derived display name its LongName; renaming the room renames its parts, nothing is stored.
    let ifc_of = |labels: &std::collections::BTreeMap<String, String>| {
        let o = nnx_core::ifc::IfcOpts { project_name: "P".into(), author: "A".into(), timestamp: "2026-01-01T00:00:00".into(), labels: labels.clone() };
        nnx_core::ifc::export_ifc(&doc.model, &o).unwrap().text
    };
    let t = ifc_of(&named);
    assert!(t.contains("'living-2'") && t.contains("'Hall-2'") && t.contains("'Hall'"), "Name is the key, LongName the display name");
    named.insert("living".into(), "Lounge".into());
    let t = ifc_of(&named);
    assert!(t.contains("'Lounge-2'") && !t.contains("'Hall-2'"));
    // No display name anywhere: no LongName, the key alone, as before.
    assert!(!ifc_of(&Default::default()).contains("Lounge"));
    // A part whose key does not extend the key of the room it came from (edited by hand) is shown by its own key, never a guessed name.
    let mut odd = part(&doc, "living-2");
    odd.requirement_key = "study".into();
    assert_eq!(annot::room_name(&odd, &named), None);
    // A splitFrom that is not a room is ignored, not an error.
    let mut broken = part(&doc, "living-2");
    broken.extra.insert("splitFrom".into(), json!("not a room"));
    assert_eq!(annot::room_name(&broken, &named), None);
    // A display name set for the part's own key wins, and clearing it gives the derived name back.
    named.insert("living-2".into(), "Study".into());
    assert_eq!(annot::room_name(&part(&doc, "living-2"), &named).as_deref(), Some("Study"));
    named.remove("living-2");
    assert_eq!(annot::room_name(&part(&doc, "living-2"), &named).as_deref(), Some("Lounge-2"));
}

#[test]
fn a_snapshot_labels_a_divided_part_by_its_display_name_without_changing_what_a_comparison_sees() {
    let named: std::collections::BTreeMap<String, String> = [("living".to_string(), "Hall".to_string())].into();
    let mut doc = blank();
    layout_room(&mut doc, "living", "living", [0, 0, 9000, 4000]);
    separator(&mut doc, [3000, 0], [3000, 4000]);
    let take = |d: &Document, labels: &std::collections::BTreeMap<String, String>| {
        let lib = nnx_core::boq::BoqLib::default();
        let cost = nnx_core::costtier::CostInputs::default();
        snapshot::capture(&d.model, "S1", "A", None, 1, labels, &[], &boq::compute(&[], &lib), &costtier::compute(&[], &cost))
    };
    let label_of = |s: &snapshot::Snapshot, key: &str| s.elements.values().find(|e| e.kind == "Space" && e.props["requirementKey"] == key).unwrap().label.clone();
    let shown = snapshot::room_labels(&doc.model, &named);
    let with_names = take(&doc, &shown);
    assert_eq!((label_of(&with_names, "living").as_str(), label_of(&with_names, "living-2").as_str()), ("Hall", "Hall-2"));
    // No display name anywhere: the key is the label, exactly as before this change.
    assert!(snapshot::room_labels(&doc.model, &Default::default()).is_empty());
    let plain = take(&doc, &Default::default());
    assert_eq!(label_of(&plain, "living-2"), "living-2");
    // The label is display only: two snapshots of the same model compare as identical whatever their labels, and the rooms stay keyed by requirement key.
    let d = snapshot::compare(&plain, &with_names);
    assert_eq!((d.added, d.removed, d.modified), (0, 0, 0));
    assert!(d.rooms.is_empty());
    // A snapshot saved before this change (labels were keys) still loads and compares.
    let old: snapshot::Snapshot = serde_json::from_value(serde_json::to_value(&plain).unwrap()).unwrap();
    assert_eq!(label_of(&old, "living"), "living");
}
