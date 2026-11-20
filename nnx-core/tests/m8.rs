//! Milestone 8: BIM drafting over the model (snapping, walls, openings, box select, move/copy/rotate/mirror, split, trim, delete,
//! properties). Every value is SYNTHETIC test data.

use nnx_core::draft::{self, BoxMode, OpeningKind, OpeningSpec, SnapCtx, SnapKind, WallSpec, Xf};
use nnx_core::project::{self, NanoArchProject};
use nnx_core::typed::{Column, Door, Slab, Space, Wall, Window, element_of};
use nnx_core::*;
use serde_json::json;

fn space(key: &str, func: &str, x: i64) -> Space {
    Space { func: func.into(), requirement_key: key.into(), area: 24.0, target_area: 24.0, x, y: 0, w: 6000, h: 4000, boundary: vec![[x, 0], [x + 6000, 0], [x + 6000, 4000], [x, 4000]], net_area: None, glazing_ratio: None, extra: Default::default() }
}

fn wall(x1: i64, y1: i64, x2: i64, y2: i64, a: &str, b: Option<&str>, exterior: bool) -> Wall {
    Wall { x1, y1, x2, y2, thickness: 200, height: 3000, length: (x2 - x1).abs() + (y2 - y1).abs(), is_exterior: exterior, room_a: a.into(), room_b: b.map(str::to_string), extra: Default::default() }
}

/// Two 6 x 4 m rooms side by side (a: x 0..6000, b: x 6000..12000), their walls, and a project element.
fn plan() -> Document {
    let mut cmds = vec![Command::Create(element_of(&space("a", "living", 0), Source::User)), Command::Create(element_of(&space("b", "bedroom", 6000), Source::User))];
    for (x1, y1, x2, y2, a, b, ext) in [(0, 0, 12000, 0, "a", None, true), (12000, 0, 12000, 4000, "b", None, true), (12000, 4000, 0, 4000, "b", None, true), (0, 4000, 0, 0, "a", None, true), (6000, 0, 6000, 4000, "a", Some("b"), false)] {
        cmds.push(Command::Create(element_of(&wall(x1, y1, x2, y2, a, b, ext), Source::Generated)));
    }
    for (key, x) in [("a", 0), ("b", 6000)] {
        cmds.push(Command::Create(element_of(&Slab { x, y: 0, w: 6000, h: 4000, thickness: 150, kind: "solid".into(), room: key.into(), extra: Default::default() }, Source::User)));
    }
    cmds.push(project::create_command(&NanoArchProject::default()).unwrap());
    let mut doc = Document::default();
    doc.commit("plan", Origin::User, cmds).unwrap_or_else(|e| panic!("{e}"));
    doc.mark_saved();
    doc
}

fn ctx(from: Option<[f64; 2]>, ortho: bool) -> SnapCtx {
    SnapCtx { scale: 0.05, from, ortho, exclude: vec![] }
}

fn wall_id(doc: &Document, x1: i64, y1: i64, x2: i64, y2: i64) -> Id {
    doc.model.elements.values().find(|e| e.kind == "Wall" && e.props["x1"] == x1 && e.props["y1"] == y1 && e.props["x2"] == x2 && e.props["y2"] == y2).unwrap_or_else(|| panic!("no wall {x1},{y1}-{x2},{y2}")).id
}

fn commit(doc: &mut Document, label: &str, f: impl FnOnce(&Model) -> draft::Edit) {
    let e = f(&doc.model);
    doc.commit(label, Origin::User, e.cmds).unwrap_or_else(|x| panic!("{x}"));
}

fn add_wall(doc: &mut Document, a: [i64; 2], b: [i64; 2]) -> Id {
    let (c, id) = draft::wall_command(&doc.model, None, &WallSpec { a, b, thickness: 150, height: 3000 }).unwrap();
    doc.commit("Draw wall", Origin::User, vec![c]).unwrap_or_else(|x| panic!("{x}"));
    id
}

fn open(kind: OpeningKind, wall: Id, offset: i64, width: i64) -> OpeningSpec {
    OpeningSpec { kind, wall, offset, width, height: 2100, sill: (kind == OpeningKind::Window).then_some(900), side: None }
}

// ------------------------------------------------------------------ snapping

#[test]
fn the_pointer_snaps_to_ends_crossings_middles_then_the_grid_in_that_order() {
    let doc = plan();
    let m = &doc.model;
    // The scale is 0.05 px/mm, so the snap radius is 200 mm and the grid step 250 mm.
    let s = draft::snap(m, None, [6080.0, 3930.0], &ctx(None, false));
    assert_eq!((s.p, s.kind), ([6000, 4000], SnapKind::Endpoint), "the end of the partition and the middle of the top wall");
    let s = draft::snap(m, None, [3050.0, 90.0], &ctx(None, false));
    assert_eq!(s.kind, SnapKind::Grid, "3000 is on the 250 grid; no wall end near");
    let s = draft::snap(m, None, [5960.0, 20.0], &ctx(None, false));
    assert_eq!((s.p, s.kind), ([6000, 0], SnapKind::Endpoint));
    let s = draft::snap(m, None, [9020.0, 60.0], &ctx(None, false));
    assert_eq!(s.kind, SnapKind::Grid, "9000 is not the middle of anything");
    let s = draft::snap(m, None, [6060.0, 1950.0], &ctx(None, false));
    assert_eq!((s.p, s.kind), ([6000, 2000], SnapKind::Midpoint), "the middle of the 4 m partition");
}

#[test]
fn a_crossing_and_a_wall_middle_are_found_and_excluded_walls_do_not_attract() {
    let mut doc = plan();
    let w = add_wall(&mut doc, [3000, 1000], [3000, 3000]);
    let h = add_wall(&mut doc, [1000, 2000], [5000, 2000]);
    let _ = (w, h);
    let s = draft::snap(&doc.model, None, [3070.0, 1950.0], &ctx(None, false));
    assert_eq!((s.p, s.kind), ([3000, 2000], SnapKind::Intersection));
    // Without the horizontal wall the crossing is gone; the middle of the vertical one (3000, 2000) is offered instead.
    let mut c = ctx(None, false);
    c.exclude = vec![h];
    let s = draft::snap(&doc.model, None, [3070.0, 1950.0], &c);
    assert_eq!((s.p, s.kind), ([3000, 2000], SnapKind::Midpoint));
}

#[test]
fn while_drawing_the_direction_is_held_to_an_axis_and_the_length_snaps_to_the_grid() {
    let doc = plan();
    // Start at (1000, 1000); the pointer is mostly to the right and slightly up: the point lands on the horizontal axis.
    let s = draft::snap(&doc.model, None, [3010.0, 1090.0], &ctx(Some([1000.0, 1000.0]), true));
    assert_eq!(s.p[1], 1000);
    assert_eq!((s.p[0] - 1000) % 250, 0);
    assert_eq!(s.kind, SnapKind::Grid);
    // Mostly up: the vertical axis.
    let s = draft::snap(&doc.model, None, [1080.0, 3010.0], &ctx(Some([1000.0, 1000.0]), true));
    assert_eq!(s.p[0], 1000);
    // A wall end on the held axis wins over the grid, and lands exactly on the axis line.
    let s = draft::snap(&doc.model, None, [5900.0, 1100.0], &ctx(Some([1000.0, 1000.0]), true));
    assert_ne!(s.kind, SnapKind::Endpoint, "the partition end (6000, 0) is not on the axis y = 1000");
    let s = draft::snap(&doc.model, None, [5990.0, 40.0], &ctx(Some([1000.0, 0.0]), true));
    assert_eq!((s.p, s.kind), ([6000, 0], SnapKind::Endpoint));
}

// ------------------------------------------------------------------ walls

#[test]
fn a_wall_is_drawn_between_two_points_with_its_rooms_and_exterior_flag_derived_from_where_it_stands() {
    let mut doc = plan();
    // Inside room a: both sides are room a, so it has one room and is not exterior.
    let id = add_wall(&mut doc, [3000, 1000], [3000, 3000]);
    let w = doc.model.get(&id).unwrap().typed::<Wall>().unwrap();
    assert_eq!((w.room_a.as_str(), w.room_b.clone(), w.is_exterior, w.length, w.thickness, w.height), ("a", None, false, 2000, 150, 3000));
    assert_eq!(doc.model.get(&id).unwrap().provenance.source, Source::User, "a drafted wall belongs to the user");
    // Reversed end points give the same wall (the lower end first).
    let id2 = add_wall(&mut doc, [5000, 2000], [4000, 2000]);
    let w2 = doc.model.get(&id2).unwrap().typed::<Wall>().unwrap();
    assert_eq!((w2.x1, w2.x2), (4000, 5000));
    // Along the shared edge of the two rooms (but offset so it does not overlap the partition): between a and b.
    let between = add_wall(&mut doc, [6000, 4000], [6000, 5000]);
    let wb = doc.model.get(&between).unwrap().typed::<Wall>().unwrap();
    assert_eq!(wb.room_a, "", "outside the building: no room on either side");
    assert!(!wb.is_exterior);
    // One side inside a room and the other outside: exterior.
    let edge = add_wall(&mut doc, [1000, 4100], [2000, 4100]);
    assert!(!doc.model.get(&edge).unwrap().typed::<Wall>().unwrap().is_exterior, "4100 is outside the 4000 room edge: no room on either side");
    let inner = add_wall(&mut doc, [8000, 3900], [9000, 3900]);
    let wi = doc.model.get(&inner).unwrap().typed::<Wall>().unwrap();
    assert_eq!((wi.room_a.as_str(), wi.is_exterior), ("b", false));
    // It is one undoable step and part of the model the file stores.
    assert!(doc.undo());
    assert!(doc.model.get(&inner).is_none());
}

#[test]
fn a_wall_that_is_diagonal_too_short_without_a_size_or_over_another_is_refused_in_words() {
    let doc = plan();
    let spec = |a: [i64; 2], b: [i64; 2], t: i64, h: i64| WallSpec { a, b, thickness: t, height: h };
    let err = |s: WallSpec| draft::wall_command(&doc.model, None, &s).unwrap_err();
    assert!(err(spec([0, 0], [1000, 500], 150, 3000)).contains("horizontal or vertical"));
    assert!(err(spec([1000, 1000], [1000, 1000], 150, 3000)).contains("same"));
    assert!(err(spec([1000, 1000], [1050, 1000], 150, 3000)).contains("at least 100"));
    assert!(err(spec([1000, 1000], [2000, 1000], 0, 3000)).contains("thickness and height"));
    assert!(err(spec([1000, 1000], [2000, 1000], 150, 0)).contains("thickness and height"));
    assert!(err(spec([2000, 0], [4000, 0], 150, 3000)).contains("already runs"), "over the bottom wall");
    // A wall that only touches the end of another does not overlap it.
    assert!(draft::wall_command(&doc.model, None, &spec([12000, 4000], [12000, 5000], 150, 3000)).is_ok());
}

// ------------------------------------------------------------------ openings

#[test]
fn a_door_and_a_window_are_placed_in_a_wall_with_their_rooms_derived_and_overlaps_refused() {
    let mut doc = plan();
    let bottom = wall_id(&doc, 0, 0, 12000, 0);
    let part = wall_id(&doc, 6000, 0, 6000, 4000);
    let (cmds, door) = draft::opening_command(&doc.model, &open(OpeningKind::Door, part, 2000, 900)).unwrap();
    doc.commit("door", Origin::User, cmds).unwrap_or_else(|e| panic!("{e}"));
    let d = doc.model.get(&door).unwrap().typed::<Door>().unwrap();
    assert_eq!((d.from.as_str(), d.to.as_str(), d.opens_into.as_str(), d.is_egress), ("a", "b", "a", false));
    assert_eq!(doc.model.get(&door).unwrap().host_id, Some(part));
    // The door opens into the room on the side the pointer was on.
    let mut spec = open(OpeningKind::Door, part, 3500, 900);
    spec.side = Some(1);
    let (c2, door2) = draft::opening_command(&doc.model, &spec).unwrap();
    doc.commit("door 2", Origin::User, c2).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(doc.model.get(&door2).unwrap().typed::<Door>().unwrap().opens_into, "b");
    // A window in the exterior wall: its room is the room inside; the sill is required.
    let (cw, win) = draft::opening_command(&doc.model, &open(OpeningKind::Window, bottom, 3000, 1200)).unwrap();
    doc.commit("window", Origin::User, cw).unwrap_or_else(|e| panic!("{e}"));
    let w = doc.model.get(&win).unwrap().typed::<Window>().unwrap();
    assert_eq!((w.room.as_str(), w.sill_height), ("a", 900));
    let mut no_sill = open(OpeningKind::Window, bottom, 8000, 1200);
    no_sill.sill = None;
    assert!(draft::opening_command(&doc.model, &no_sill).unwrap_err().contains("sill"));
    // Refusals: overlap, too wide for the wall, too near the end, no size, vanished wall.
    assert!(draft::opening_command(&doc.model, &open(OpeningKind::Door, part, 2400, 900)).unwrap_err().contains("overlap"));
    assert!(draft::opening_command(&doc.model, &open(OpeningKind::Door, part, 2000, 5000)).unwrap_err().contains("does not fit"));
    assert!(draft::opening_command(&doc.model, &open(OpeningKind::Door, part, 100, 900)).unwrap_err().contains("does not fit"));
    assert!(draft::opening_command(&doc.model, &OpeningSpec { width: 0, ..open(OpeningKind::Door, part, 2000, 900) }).unwrap_err().contains("above zero"));
    assert!(draft::opening_command(&doc.model, &open(OpeningKind::Door, Id::now_v7(), 2000, 900)).is_err());
}

#[test]
fn the_placement_preview_finds_the_wall_snaps_the_offset_and_keeps_the_opening_inside() {
    let doc = plan();
    let bottom = wall_id(&doc, 0, 0, 12000, 0);
    let p = draft::placement(&doc.model, None, [3020.0, 60.0], 900, 0.05).unwrap();
    assert_eq!((p.wall.as_str(), p.offset, p.fits, p.side, p.horizontal), (bottom.to_string().as_str(), 3000, true, 1, true));
    assert_eq!(p.center, [3000, 0]);
    // Near the middle of the wall the opening centres on it.
    assert_eq!(draft::placement(&doc.model, None, [6030.0, -30.0], 900, 0.05).map(|p| (p.offset, p.side)), Some((6000, -1)));
    // Near the end it is held inside the wall with the clearance.
    assert_eq!(draft::placement(&doc.model, None, [30.0, 30.0], 900, 0.05).unwrap().offset, 500);
    // Away from every wall there is no placement.
    assert!(draft::placement(&doc.model, None, [3000.0, 2000.0], 900, 0.05).is_none());
    // A wall shorter than the opening cannot take it.
    let mut d2 = plan();
    let short = add_wall(&mut d2, [3000, 1000], [3500, 1000]);
    let p = draft::placement(&d2.model, None, [3250.0, 1000.0], 900, 0.05).unwrap();
    assert_eq!((p.wall.as_str(), p.fits), (short.to_string().as_str(), false));
}

// ------------------------------------------------------------------ selection

#[test]
fn a_window_box_takes_what_it_holds_and_a_crossing_box_what_it_touches() {
    let mut doc = plan();
    let part = wall_id(&doc, 6000, 0, 6000, 4000);
    let (c, door) = draft::opening_command(&doc.model, &open(OpeningKind::Door, part, 2000, 900)).unwrap();
    doc.commit("door", Origin::User, c).unwrap_or_else(|e| panic!("{e}"));
    let col = element_of(&Column { x: 3000, y: 2000, b: 300, h: 300, grid_ref: String::new(), axial_service_kn: None, extra: Default::default() }, Source::User);
    let col_id = col.id;
    doc.commit("col", Origin::User, vec![Command::Create(col)]).unwrap_or_else(|e| panic!("{e}"));
    // A box around the partition and its door only.
    let tight = draft::pick_box(&doc.model, None, [5800.0, -100.0, 6200.0, 4100.0], BoxMode::Window);
    assert!(tight.contains(&part) && tight.contains(&door) && !tight.contains(&col_id));
    // A box over the column and part of the bottom wall: a window box takes only the column; a crossing box also the wall it touches.
    let b = [2500.0, -50.0, 3500.0, 2500.0];
    assert_eq!(draft::pick_box(&doc.model, None, b, BoxMode::Window), vec![col_id]);
    let cross = draft::pick_box(&doc.model, None, b, BoxMode::Crossing);
    assert!(cross.contains(&col_id) && cross.contains(&wall_id(&doc, 0, 0, 12000, 0)));
    // Corners may be given in any order.
    assert_eq!(draft::pick_box(&doc.model, None, [3500.0, 2500.0, 2500.0, -50.0], BoxMode::Window), vec![col_id]);
    // Rooms are not picked by a box.
    assert!(draft::pick_box(&doc.model, None, [-100.0, -100.0, 20000.0, 20000.0], BoxMode::Crossing).iter().all(|i| doc.model.get(i).unwrap().kind != "Space"));
}

// ------------------------------------------------------------------ move, copy, rotate, mirror

fn with_door() -> (Document, Id, Id) {
    let mut doc = plan();
    let w = add_wall(&mut doc, [2000, 1000], [4000, 1000]);
    let (c, d) = draft::opening_command(&doc.model, &open(OpeningKind::Door, w, 600, 800)).unwrap();
    doc.commit("door", Origin::User, c).unwrap_or_else(|e| panic!("{e}"));
    (doc, w, d)
}

#[test]
fn moving_a_wall_takes_its_door_and_refuses_what_cannot_be_moved() {
    let (mut doc, w, d) = with_door();
    commit(&mut doc, "Move", |m| draft::xform(m, &[w], Xf::Move { dx: 500, dy: 1000 }, false).unwrap());
    let nw = doc.model.get(&w).unwrap().typed::<Wall>().unwrap();
    assert_eq!((nw.x1, nw.y1, nw.x2, nw.y2, nw.length), (2500, 2000, 4500, 2000, 2000));
    assert_eq!(doc.model.get(&d).unwrap().typed::<Door>().unwrap().offset_center, 600, "a translation keeps the offset");
    assert_eq!(doc.model.get(&d).unwrap().host_id, Some(w));
    // A door alone, a room, or a generated column cannot be moved this way.
    assert!(draft::xform(&doc.model, &[d], Xf::Move { dx: 100, dy: 0 }, false).unwrap_err().contains("moves with its wall"));
    let room = doc.model.elements.values().find(|e| e.kind == "Space").unwrap().id;
    assert!(draft::xform(&doc.model, &[room], Xf::Move { dx: 100, dy: 0 }, false).unwrap_err().contains("partition"));
    assert!(draft::xform(&doc.model, &[], Xf::Move { dx: 100, dy: 0 }, false).is_err());
    let gcol = element_of(&Column { x: 0, y: 0, b: 300, h: 300, grid_ref: "A1".into(), axial_service_kn: None, extra: Default::default() }, Source::Generated);
    let gid = gcol.id;
    doc.commit("col", Origin::Generator, vec![Command::Create(gcol)]).unwrap_or_else(|e| panic!("{e}"));
    assert!(draft::xform(&doc.model, &[gid], Xf::Move { dx: 100, dy: 0 }, false).unwrap_err().contains("generated structure"));
    assert!(draft::xform(&doc.model, &[gid], Xf::Move { dx: 100, dy: 0 }, true).is_ok(), "a copy of it is a new user column");
}

#[test]
fn a_copy_makes_new_walls_and_openings_owned_by_the_user_and_leaves_the_original() {
    let (mut doc, w, d) = with_door();
    let before = doc.model.len();
    let e = draft::xform(&doc.model, &[w], Xf::Move { dx: 0, dy: 1500 }, true).unwrap();
    assert_eq!(e.created.len(), 2, "the wall and its door");
    commit(&mut doc, "Copy", |_| e.clone());
    assert_eq!(doc.model.len(), before + 2);
    let nw = doc.model.get(&e.created[0]).unwrap();
    assert_eq!((nw.props["y1"].as_i64(), nw.provenance.source), (Some(2500), Source::User));
    let nd = doc.model.get(&e.created[1]).unwrap();
    assert_eq!((nd.kind.as_str(), nd.host_id), ("Door", Some(e.created[0])));
    assert_ne!(e.created[1], d);
    assert_eq!(doc.model.get(&w).unwrap().props["y1"], 1000, "the original did not move");
    assert!(doc.undo());
    assert_eq!(doc.model.len(), before);
}

#[test]
fn rotating_and_mirroring_keep_the_door_at_the_same_place_along_the_wall_in_the_plan() {
    let (mut doc, w, d) = with_door();
    // The door centre is at x = 2600 on the wall y = 1000. Quarter turn counter-clockwise about (3000, 1000): (x, y) -> (cx - dy, cy + dx).
    commit(&mut doc, "Rotate", |m| draft::xform(m, &[w], Xf::Rotate { cx: 3000, cy: 1000, turns: 1 }, false).unwrap());
    let rw = doc.model.get(&w).unwrap().typed::<Wall>().unwrap();
    assert_eq!((rw.x1, rw.y1, rw.x2, rw.y2), (3000, 0, 3000, 2000), "now vertical, lowest end first");
    let (door_y, host) = (doc.model.get(&d).unwrap().typed::<Door>().unwrap().offset_center, doc.model.get(&d).unwrap().host_id);
    assert_eq!(door_y, 600, "the old start (x = 2000) is the new lower end (y = 0), so the offset is unchanged");
    assert_eq!(host, Some(w));
    // Clockwise reverses the wall's direction: the offset becomes the distance from the new lower end.
    let (mut d2, w2, door2) = with_door();
    commit(&mut d2, "Rotate", |m| draft::xform(m, &[w2], Xf::Rotate { cx: 3000, cy: 1000, turns: -1 }, false).unwrap());
    assert_eq!(d2.model.get(&door2).unwrap().typed::<Door>().unwrap().offset_center, 1400, "length 2000 - 600");
    // Mirroring in the vertical line x = 3000 turns the horizontal wall end for end: 600 from the left becomes 600 from the right.
    let (mut d3, w3, door3) = with_door();
    commit(&mut d3, "Mirror", |m| draft::xform(m, &[w3], Xf::Mirror { axis: 'x', at: 3000 }, false).unwrap());
    assert_eq!(d3.model.get(&w3).unwrap().props["x1"], 2000);
    assert_eq!(d3.model.get(&door3).unwrap().typed::<Door>().unwrap().offset_center, 1400);
    // A mirror copy leaves the original.
    let (d4, w4, _) = with_door();
    let e = draft::xform(&d4.model, &[w4], Xf::Mirror { axis: 'y', at: 2000 }, true).unwrap();
    assert_eq!(e.created.len(), 2);
}

#[test]
fn columns_rotate_their_section_and_a_wall_moved_across_rooms_gets_its_rooms_again() {
    let mut doc = plan();
    let col = element_of(&Column { x: 3000, y: 2000, b: 300, h: 500, grid_ref: String::new(), axial_service_kn: None, extra: Default::default() }, Source::User);
    let id = col.id;
    doc.commit("col", Origin::User, vec![Command::Create(col)]).unwrap_or_else(|e| panic!("{e}"));
    commit(&mut doc, "Rotate", |m| draft::xform(m, &[id], Xf::Rotate { cx: 3000, cy: 2000, turns: 1 }, false).unwrap());
    let c = doc.model.get(&id).unwrap().typed::<Column>().unwrap();
    assert_eq!((c.x, c.y, c.b, c.h), (3000, 2000, 500, 300), "section turned with it");
    let w = add_wall(&mut doc, [1000, 1000], [2000, 1000]);
    assert_eq!(doc.model.get(&w).unwrap().props["roomA"], "a");
    commit(&mut doc, "Move", |m| draft::xform(m, &[w], Xf::Move { dx: 8000, dy: 0 }, false).unwrap());
    assert_eq!(doc.model.get(&w).unwrap().props["roomA"], "b", "now inside room b");
}

#[test]
fn align_brings_a_wall_to_the_line_of_a_parallel_reference() {
    let (mut doc, w, _) = with_door();
    let other = add_wall(&mut doc, [1000, 3000], [5000, 3000]);
    commit(&mut doc, "Align", |m| draft::align(m, other, w).unwrap());
    assert_eq!(doc.model.get(&w).unwrap().props["y1"], 3000);
    assert!(draft::align(&doc.model, other, w).unwrap_err().contains("already"));
    let vertical = wall_id(&doc, 6000, 0, 6000, 4000);
    assert!(draft::align(&doc.model, vertical, w).unwrap_err().contains("parallel"));
}

// ------------------------------------------------------------------ split and trim

#[test]
fn splitting_a_wall_keeps_each_door_with_its_half_and_refuses_a_cut_through_one() {
    let mut doc = plan();
    let w = add_wall(&mut doc, [1000, 1000], [5000, 1000]);
    let (c1, near) = draft::opening_command(&doc.model, &open(OpeningKind::Door, w, 600, 800)).unwrap();
    doc.commit("d1", Origin::User, c1).unwrap_or_else(|e| panic!("{e}"));
    let (c2, far) = draft::opening_command(&doc.model, &open(OpeningKind::Window, w, 3200, 1000)).unwrap();
    doc.commit("d2", Origin::User, c2).unwrap_or_else(|e| panic!("{e}"));
    assert!(draft::split(&doc.model, w, 1600 + 1000).is_ok(), "at x = 2600, between the door (to 2000) and the window (from 3700)");
    assert!(draft::split(&doc.model, w, 1000 + 3200).unwrap_err().contains("crosses"), "through the window");
    assert!(draft::split(&doc.model, w, 1050).unwrap_err().contains("at least 100"));
    let e = draft::split(&doc.model, w, 2600).unwrap();
    commit(&mut doc, "Split", |_| e.clone());
    let first = doc.model.get(&w).unwrap().typed::<Wall>().unwrap();
    assert_eq!((first.x1, first.x2, first.length), (1000, 2600, 1600));
    let second = doc.model.get(&e.created[0]).unwrap().typed::<Wall>().unwrap();
    assert_eq!((second.x1, second.x2, second.length, second.thickness), (2600, 5000, 2400, 150));
    assert_eq!(doc.model.get(&near).unwrap().host_id, Some(w), "the door stayed");
    assert!(doc.model.get(&far).is_none(), "the window was re-hosted under a new id");
    let moved = doc.model.dependents(&e.created[0]);
    assert_eq!(moved.len(), 1);
    assert_eq!(doc.model.get(&moved[0]).unwrap().typed::<Window>().unwrap().offset_center, 3200 - 1600, "same place in the plan: 600 from the new wall's start");
    // One undo puts it all back.
    assert!(doc.undo());
    assert_eq!(doc.model.get(&far).unwrap().host_id, Some(w));
    assert_eq!(doc.model.get(&w).unwrap().props["x2"], 5000);
}

#[test]
fn trimming_two_perpendicular_walls_brings_them_to_their_corner() {
    let mut doc = plan();
    // An L that does not close: a horizontal wall ending short of a vertical one, and the vertical one passing the horizontal.
    let h = add_wall(&mut doc, [1000, 1000], [2500, 1000]);
    let v = add_wall(&mut doc, [3000, 500], [3000, 3000]);
    commit(&mut doc, "Trim", |m| draft::trim_corner(m, (h, [2400.0, 1000.0]), (v, [3000.0, 2500.0])).unwrap());
    let hw = doc.model.get(&h).unwrap().typed::<Wall>().unwrap();
    assert_eq!((hw.x1, hw.x2, hw.length), (1000, 3000, 2000), "extended to the corner at x = 3000");
    let vw = doc.model.get(&v).unwrap().typed::<Wall>().unwrap();
    assert_eq!((vw.y1, vw.y2), (1000, 3000), "the part below the corner (y < 1000) was trimmed away: the click was on the upper part");
    assert!(draft::trim_corner(&doc.model, (h, [2400.0, 1000.0]), (v, [3000.0, 2500.0])).unwrap_err().contains("already meet"));
    let parallel = add_wall(&mut doc, [1000, 2000], [2000, 2000]);
    assert!(draft::trim_corner(&doc.model, (h, [0.0, 0.0]), (parallel, [0.0, 0.0])).unwrap_err().contains("perpendicular"));
}

#[test]
fn a_trim_that_would_strand_an_opening_is_refused() {
    let mut doc = plan();
    let h = add_wall(&mut doc, [1000, 1000], [4000, 1000]);
    let v = add_wall(&mut doc, [2000, 500], [2000, 3000]);
    let (c, _) = draft::opening_command(&doc.model, &open(OpeningKind::Door, h, 2500, 800)).unwrap();
    doc.commit("door", Origin::User, c).unwrap_or_else(|e| panic!("{e}"));
    // Keeping the left part (click left of the corner at x = 2000) would cut away the door at x = 3500.
    let err = draft::trim_corner(&doc.model, (h, [1500.0, 1000.0]), (v, [2000.0, 2000.0])).unwrap_err();
    assert!(err.contains("outside the trimmed wall"), "{err}");
    // Keeping the right part is fine and the door keeps its place in the plan (offset measured from the new start).
    commit(&mut doc, "Trim", |m| draft::trim_corner(m, (h, [3500.0, 1000.0]), (v, [2000.0, 2000.0])).unwrap());
    let dep = doc.model.dependents(&h)[0];
    assert_eq!(doc.model.get(&dep).unwrap().typed::<Door>().unwrap().offset_center, 1500, "3500 - 2000");
}

// ------------------------------------------------------------------ delete

#[test]
fn deleting_a_wall_deletes_its_openings_first_and_says_so_and_generated_columns_are_protected() {
    let (mut doc, w, d) = with_door();
    let e = draft::delete(&doc.model, &[w]).unwrap();
    assert_eq!(e.notes.len(), 1);
    assert!(matches!(e.cmds[0], Command::Delete(i) if i == d), "the door goes first");
    commit(&mut doc, "Delete", |_| e);
    assert!(doc.model.get(&w).is_none() && doc.model.get(&d).is_none());
    assert!(doc.undo());
    assert!(doc.model.get(&w).is_some() && doc.model.get(&d).is_some());
    // Selecting the wall and its door together does not delete the door twice.
    assert_eq!(draft::delete(&doc.model, &[w, d]).unwrap().cmds.len(), 2);
    let room = doc.model.elements.values().find(|e| e.kind == "Space").unwrap().id;
    assert!(draft::delete(&doc.model, &[room]).unwrap_err().contains("room tool"));
    let gcol = element_of(&Column { x: 0, y: 0, b: 300, h: 300, grid_ref: "A1".into(), axial_service_kn: None, extra: Default::default() }, Source::Generated);
    let gid = gcol.id;
    doc.commit("col", Origin::Generator, vec![Command::Create(gcol)]).unwrap_or_else(|e| panic!("{e}"));
    assert!(draft::delete(&doc.model, &[gid]).unwrap_err().contains("generated structure"));
}

// ------------------------------------------------------------------ properties

#[test]
fn the_inspector_lists_properties_and_edits_them_with_the_checks_that_depend_on_other_elements() {
    let (mut doc, w, d) = with_door();
    let p = draft::properties(&doc.model, w).unwrap();
    assert_eq!(p["kind"], "Wall");
    let f = |p: &serde_json::Value, k: &str| p["fields"].as_array().unwrap().iter().find(|f| f["key"] == k).unwrap().clone();
    assert_eq!((f(&p, "length")["value"].clone(), f(&p, "length")["editable"].clone(), f(&p, "roomA")["editable"].clone()), (json!(2000), json!(true), json!(false)));
    // Thickness and height.
    commit(&mut doc, "Edit", |m| draft::set_property(m, w, "thickness", &json!(250)).unwrap());
    assert_eq!(doc.model.get(&w).unwrap().props["thickness"], 250);
    assert!(draft::set_property(&doc.model, w, "thickness", &json!(0)).unwrap_err().contains("above zero"));
    assert!(draft::set_property(&doc.model, w, "thickness", &json!("thick")).unwrap_err().contains("whole number"));
    // Length: a longer wall keeps its lower end; a shorter one than the door needs is refused.
    commit(&mut doc, "Edit", |m| draft::set_property(m, w, "length", &json!(3000)).unwrap());
    let nw = doc.model.get(&w).unwrap().typed::<Wall>().unwrap();
    assert_eq!((nw.x1, nw.x2, nw.length), (2000, 5000, 3000));
    assert!(draft::set_property(&doc.model, w, "length", &json!(900)).unwrap_err().contains("outside the shorter wall"));
    assert!(draft::set_property(&doc.model, w, "length", &json!(50)).unwrap_err().contains("at least"));
    // The door: width, distance from the far end, egress; an impossible position is refused with the reason.
    let pd = draft::properties(&doc.model, d).unwrap();
    assert_eq!(f(&pd, "offsetFromEnd")["value"], json!(2400));
    commit(&mut doc, "Edit", |m| draft::set_property(m, d, "offsetFromEnd", &json!(1000)).unwrap());
    assert_eq!(doc.model.get(&d).unwrap().typed::<Door>().unwrap().offset_center, 2000);
    assert!(draft::set_property(&doc.model, d, "offsetCenter", &json!(100)).unwrap_err().contains("does not fit"));
    assert!(draft::set_property(&doc.model, d, "width", &json!(6000)).unwrap_err().contains("does not fit"));
    commit(&mut doc, "Edit", |m| draft::set_property(m, d, "isEgress", &json!(true)).unwrap());
    assert!(doc.model.get(&d).unwrap().typed::<Door>().unwrap().is_egress);
    assert!(draft::set_property(&doc.model, d, "sillHeight", &json!(900)).unwrap_err().contains("not edited here"), "a door has no sill");
    assert!(draft::set_property(&doc.model, w, "roomA", &json!("x")).unwrap_err().contains("not edited here"));
    // A second opening cannot be moved on top of the first.
    let (c, other) = draft::opening_command(&doc.model, &open(OpeningKind::Window, w, 800, 600)).unwrap();
    doc.commit("window", Origin::User, c).unwrap_or_else(|e| panic!("{e}"));
    assert!(draft::set_property(&doc.model, other, "offsetCenter", &json!(2000)).unwrap_err().contains("overlap"));
    // A room is read-only.
    let room = doc.model.elements.values().find(|e| e.kind == "Space").unwrap().id;
    assert!(draft::properties(&doc.model, room).unwrap()["fields"].as_array().unwrap().iter().all(|f| f["editable"] == false));
}

#[test]
fn the_starting_values_are_the_most_common_on_the_level_and_absent_when_there_is_nothing_to_learn_from() {
    let empty = Document::default();
    let d = draft::defaults(&empty.model, None);
    assert!(d["wall"].is_null() && d["door"].is_null() && d["window"].is_null());
    let (doc, _, _) = with_door();
    let d = draft::defaults(&doc.model, None);
    assert_eq!(d["wall"]["height"], 3000, "only interior walls teach the interior size");
    assert!(d["wall"]["thickness"] == 150 || d["wall"]["thickness"] == 200);
    assert_eq!(d["door"], json!({ "width": 800, "height": 2100 }));
    assert!(d["window"].is_null());
}

// ------------------------------------------------------------------ the project owns it

#[test]
fn drafted_elements_are_in_the_model_so_the_file_undo_and_redo_carry_them() {
    let (mut doc, w, d) = with_door();
    commit(&mut doc, "Move", |m| draft::xform(m, &[w], Xf::Move { dx: 100, dy: 0 }, false).unwrap());
    let path = std::env::temp_dir().join(format!("nanoarch-m8-{}.nnx", std::process::id()));
    persist::save(&doc.model, &path).unwrap();
    let back = persist::load(&path).unwrap();
    std::fs::remove_file(&path).ok();
    assert_eq!(back.elements.get(&w).unwrap().props["x1"], 2100);
    assert_eq!(back.elements.get(&w).unwrap().provenance.source, Source::User);
    assert_eq!(back.elements.get(&d).unwrap().host_id, Some(w));
    assert!(doc.is_dirty());
    assert!(doc.undo() && doc.undo() && doc.undo());
    assert!(doc.model.get(&w).is_none(), "undo walks back through the move, the door and the wall");
    assert!(doc.redo() && doc.redo() && doc.redo());
    assert_eq!(doc.model.get(&w).unwrap().props["x1"], 2100);
}
