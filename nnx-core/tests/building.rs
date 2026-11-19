//! Multi-storey core pieces. Every input is a SYNTHETIC test value; every expectation is checked by hand.

use nnx_core::building::{self, Stair};
use nnx_core::layout::{Rect, Side};
use nnx_core::structural::{self, Loads, Soil, StructOpts};
use nnx_core::typed::{Column, Level, Slab, Space, Typed, Wall, element_of};
use nnx_core::*;

const ENV: Rect = Rect { x: 0, y: 0, w: 14000, h: 18000 };

fn stair(west: bool) -> Stair {
    Stair { width_mm: 2500, length_mm: 5000, west }
}

#[test]
fn the_stair_block_tiles_its_strip_and_leaves_the_rest_for_the_rooms() {
    let b = building::stair_block(ENV, Side::South, &stair(true), "").unwrap();
    assert_eq!(b.main, Rect { x: 2500, y: 0, w: 11500, h: 18000 });
    let hall = b.rooms.iter().find(|r| r.key == "stairhall").unwrap();
    let shaft = b.rooms.iter().find(|r| r.key == "stair").unwrap();
    // entrance on the south: hall at the south end, stair at the far (north) end
    assert_eq!(hall.rect, Rect { x: 0, y: 0, w: 2500, h: 13000 });
    assert_eq!(shaft.rect, Rect { x: 0, y: 13000, w: 2500, h: 5000 });
    assert_eq!(hall.func, "hall");
    assert_eq!(shaft.func, "stair");
    let area: f64 = b.rooms.iter().map(|r| r.area_m2).sum::<f64>() + b.main.area_m2();
    assert!((area - ENV.area_m2()).abs() < 1e-9, "strip + main = envelope");

    // entrance on the north, strip on the east, level 2 suffix
    let n = building::stair_block(ENV, Side::North, &stair(false), "@1").unwrap();
    assert_eq!(n.main, Rect { x: 0, y: 0, w: 11500, h: 18000 });
    let hall = n.rooms.iter().find(|r| r.key == "stairhall@1").unwrap();
    let shaft = n.rooms.iter().find(|r| r.key == "stair@1").unwrap();
    assert_eq!(hall.rect, Rect { x: 11500, y: 5000, w: 2500, h: 13000 });
    assert_eq!(shaft.rect, Rect { x: 11500, y: 0, w: 2500, h: 5000 });
    assert!(building::is_shaft("stair@1") && building::is_shaft("stair") && !building::is_shaft("stairhall"));
    assert!(building::is_stair_block("stairhall@1") && !building::is_stair_block("bedroom"));
}

#[test]
fn a_stair_that_does_not_fit_is_refused_with_the_reason() {
    let long = Stair { length_mm: 17500, ..stair(true) };
    assert!(building::stair_block(ENV, Side::South, &long, "").unwrap_err().contains("no room is left for a hall"));
    let wide = Stair { width_mm: 11500, ..stair(true) };
    assert!(building::stair_block(ENV, Side::South, &wide, "").unwrap_err().contains("too little is left"));
    let zero = Stair { width_mm: 0, ..stair(true) };
    assert!(building::stair_block(ENV, Side::South, &zero, "").is_err());
}

fn two_level_model() -> (Document, Id, Id) {
    let mut doc = Document::default();
    let l0 = element_of(&Level { name: "Ground floor".into(), index: 0, extra: Default::default() }, Source::User);
    let l1 = element_of(&Level { name: "Floor 1".into(), index: 1, extra: Default::default() }, Source::User);
    let (i0, i1) = (l0.id, l1.id);
    doc.commit("levels", Origin::User, vec![Command::Create(l0), Command::Create(l1)]).unwrap();
    (doc, i0, i1)
}

#[test]
fn levels_are_ordered_unique_and_validated() {
    let (mut doc, i0, i1) = two_level_model();
    assert_eq!(building::levels(&doc.model), vec![(0, i0), (1, i1)]);
    let dup = element_of(&Level { name: "again".into(), index: 1, extra: Default::default() }, Source::User);
    assert!(doc.commit("dup", Origin::User, vec![Command::Create(dup)]).is_err(), "two levels cannot share an index");
    let neg = element_of(&Level { name: "basement".into(), index: -1, extra: Default::default() }, Source::User);
    assert!(doc.commit("neg", Origin::User, vec![Command::Create(neg)]).is_err());
}

fn column(x: i64, y: i64, level: Id) -> Command {
    let mut el = element_of(&Column { x, y, b: 300, h: 300, grid_ref: "A1".into(), axial_service_kn: None, extra: Default::default() }, Source::Generated);
    el.level_id = Some(level);
    Command::Create(el)
}

#[test]
fn an_upper_column_with_nothing_below_it_is_reported() {
    let (mut doc, i0, i1) = two_level_model();
    doc.commit("columns", Origin::User, vec![column(0, 0, i0), column(4000, 0, i0), column(0, 0, i1), column(4100, 0, i1), column(9000, 9000, i1)]).unwrap();
    // (4100, 0) is 100 mm from (4000, 0): within half a column (150 mm), so it is supported; (9000, 9000) is not.
    assert_eq!(building::unsupported_columns(&doc.model), vec![(1, 9000, 9000)]);
}

fn room(key: &str, func: &str, r: Rect, level: Id) -> Command {
    let s = Space {
        func: func.into(),
        requirement_key: key.into(),
        area: r.area_m2(),
        target_area: r.area_m2(),
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
        boundary: vec![[r.x, r.y], [r.x + r.w, r.y], [r.x + r.w, r.y + r.h], [r.x, r.y + r.h]],
        net_area: None,
        glazing_ratio: None,
        extra: Default::default(),
    };
    let mut el = element_of(&s, Source::Generated);
    el.level_id = Some(level);
    Command::Create(el)
}

fn wall(x1: i64, y1: i64, x2: i64, y2: i64, ext: bool, a: &str, level: Id) -> Command {
    let w = Wall { x1, y1, x2, y2, thickness: 200, height: 3000, length: (x2 - x1).abs() + (y2 - y1).abs(), is_exterior: ext, room_a: a.into(), room_b: None, extra: Default::default() };
    let mut el = element_of(&w, Source::Generated);
    el.level_id = Some(level);
    Command::Create(el)
}

/// One 4 x 4 m room per level, with a stair shaft room 2 x 4 m beside it, walls round each.
fn building_with_shaft() -> (Document, Id, Id) {
    let (mut doc, i0, i1) = two_level_model();
    let mut cmds = Vec::new();
    for (lvl, suf) in [(i0, ""), (i1, "@1")] {
        cmds.push(room(&format!("r{suf}"), "bedroom", Rect { x: 0, y: 0, w: 4000, h: 4000 }, lvl));
        cmds.push(room(&format!("stair{suf}"), "stair", Rect { x: 4000, y: 0, w: 2000, h: 4000 }, lvl));
        for (a, b, c, d) in [(0, 0, 6000, 0), (0, 4000, 6000, 4000), (0, 0, 0, 4000), (6000, 0, 6000, 4000)] {
            cmds.push(wall(a, b, c, d, true, &format!("r{suf}"), lvl));
        }
    }
    doc.commit("building", Origin::User, cmds).unwrap();
    (doc, i0, i1)
}

fn opts() -> StructOpts {
    StructOpts { loads: Some(Loads { dead_kpa: 5.0, live_kpa: 2.0, source: "synthetic".into() }), soil: Some(Soil { bearing_kpa: 150.0, source: "synthetic".into() }), ..StructOpts::default() }
}

fn slabs_of(model: &Model, level: Id) -> Vec<String> {
    model.elements.values().filter(|e| e.kind == Slab::KIND && e.level_id == Some(level)).filter_map(|e| e.typed::<Slab>().ok()).map(|s| s.room).collect()
}

#[test]
fn a_stair_shaft_is_open_in_the_slab_when_it_is_listed_and_footings_follow_the_flag() {
    let (mut doc, i0, i1) = building_with_shaft();
    // Top level first: it has no open shaft and no footings; its columns' loads are then handed down.
    let top = StructOpts { footings: false, ..opts() };
    structural::sync(&mut doc, &top, Some(i1)).unwrap();
    assert!(slabs_of(&doc.model, i1).contains(&"stair@1".to_string()), "the roof closes the shaft");
    assert!(doc.model.elements.values().all(|e| e.kind != "Footing"), "no footings on an upper level");
    let top_loads: Vec<(i64, i64, f64)> = doc.model.elements.values().filter(|e| e.kind == Column::KIND && e.level_id == Some(i1)).filter_map(|e| e.typed::<Column>().ok()).filter_map(|c| Some((c.x, c.y, c.axial_service_kn?))).collect();
    assert!(!top_loads.is_empty() && top_loads.iter().all(|l| l.2 > 0.0));

    let ground = StructOpts { open_rooms: vec!["stair".into()], extra_loads: top_loads.clone(), ..opts() };
    structural::sync(&mut doc, &ground, Some(i0)).unwrap();
    assert!(!slabs_of(&doc.model, i0).contains(&"stair".to_string()), "the shaft is open above the ground level");
    assert!(slabs_of(&doc.model, i0).contains(&"r".to_string()));
    let footings = doc.model.elements.values().filter(|e| e.kind == "Footing").count();
    let ground_columns = doc.model.elements.values().filter(|e| e.kind == Column::KIND && e.level_id == Some(i0)).count();
    assert!(footings > 0 && footings <= ground_columns, "{footings} footings for {ground_columns} ground columns");

    // Each ground column carries its own slab share plus what stands on it: more than the top column alone.
    let col = |lvl: Id, x: i64, y: i64| -> f64 { doc.model.elements.values().filter(|e| e.kind == Column::KIND && e.level_id == Some(lvl)).filter_map(|e| e.typed::<Column>().ok()).find(|c| c.x == x && c.y == y).unwrap().axial_service_kn.unwrap() };
    let corners = |lvl: Id| -> Vec<(i64, i64)> { doc.model.elements.values().filter(|e| e.kind == Column::KIND && e.level_id == Some(lvl)).filter_map(|e| e.typed::<Column>().ok()).map(|c| (c.x, c.y)).collect() };
    let (x, y) = corners(i1)[0];
    let (above, below) = (col(i1, x, y), col(i0, x, y));
    assert!(below > above, "{below} should exceed {above}");
    // The ground column has its own share (same plan, same slab) plus the whole load of the column above.
    assert!((below - 2.0 * above).abs() < 0.2, "ground {below} = own {above} + above {above}");
}

#[test]
fn an_upper_column_without_a_column_below_is_named_in_the_structural_notes() {
    let (doc, i0, i1) = building_with_shaft();
    let ground = StructOpts { extra_loads: vec![(12345, 12345, 40.0)], ..opts() };
    let (_, rep) = structural::sync_commands(&doc.model, &ground, Some(i0)).unwrap();
    assert!(rep.warnings.iter().any(|w| w.contains("(12345, 12345)") && w.contains("not carried")), "{:?}", rep.warnings);
    // and without any input the notes still say what is missing
    let (_, none) = structural::sync_commands(&doc.model, &StructOpts::default(), Some(i1)).unwrap();
    assert!(none.warnings.iter().any(|w| w.contains("floor loads not set")));
}

/// Ground: one 6 x 4 m room. First floor: four 3 x 2 m rooms. The upper level has junctions the ground floor lacks.
fn stepped_building() -> (Document, Id, Id) {
    let (mut doc, i0, i1) = two_level_model();
    let mut cmds = vec![room("g", "living", Rect { x: 0, y: 0, w: 6000, h: 4000 }, i0)];
    for (a, b, c, d) in [(0, 0, 6000, 0), (0, 4000, 6000, 4000), (0, 0, 0, 4000), (6000, 0, 6000, 4000)] {
        cmds.push(wall(a, b, c, d, true, "g", i0));
    }
    for (k, (x, y)) in [("q1", (0, 0)), ("q2", (3000, 0)), ("q3", (0, 2000)), ("q4", (3000, 2000))] {
        cmds.push(room(k, "bedroom", Rect { x, y, w: 3000, h: 2000 }, i1));
    }
    for (a, b, c, d) in [(0, 0, 6000, 0), (0, 4000, 6000, 4000), (0, 0, 0, 4000), (6000, 0, 6000, 4000)] {
        cmds.push(wall(a, b, c, d, true, "q1", i1));
    }
    cmds.push(wall(3000, 0, 3000, 4000, false, "q1", i1));
    cmds.push(wall(0, 2000, 6000, 2000, false, "q1", i1));
    doc.commit("stepped", Origin::User, cmds).unwrap();
    (doc, i0, i1)
}

fn columns_at(model: &Model, level: Id) -> Vec<(i64, i64)> {
    let mut v: Vec<(i64, i64)> = model.elements.values().filter(|e| e.kind == Column::KIND && e.level_id == Some(level)).filter_map(|e| e.typed::<Column>().ok()).map(|c| (c.x, c.y)).collect();
    v.sort();
    v
}

#[test]
fn columns_continue_down_to_the_ground_and_what_that_costs_is_reported() {
    let (mut doc, i0, i1) = stepped_building();
    // Without continuation the upper junctions have nothing below them.
    let top = StructOpts { footings: false, ..opts() };
    structural::sync(&mut doc, &top, Some(i1)).unwrap();
    let upper = columns_at(&doc.model, i1);
    assert_eq!(upper, vec![(0, 0), (0, 2000), (0, 4000), (3000, 0), (3000, 2000), (3000, 4000), (6000, 0), (6000, 2000), (6000, 4000)]);
    let alone = StructOpts { floors: 1, ..opts() };
    let (_, plain) = structural::sync_commands(&doc.model, &alone, Some(i0)).unwrap();
    assert!(plain.warnings.iter().all(|w| !w.contains("continue down")));

    // With the forced positions the ground level carries all nine, and says what is unusual about them.
    let loads: Vec<(i64, i64, f64)> = doc.model.elements.values().filter(|e| e.kind == Column::KIND && e.level_id == Some(i1)).filter_map(|e| e.typed::<Column>().ok()).filter_map(|c| Some((c.x, c.y, c.axial_service_kn?))).collect();
    let ground = StructOpts { forced_columns: upper.clone(), extra_loads: loads, ..opts() };
    let (cmds, rep) = structural::sync_commands(&doc.model, &ground, Some(i0)).unwrap();
    doc.commit("ground", Origin::Generator, cmds).unwrap();
    assert_eq!(columns_at(&doc.model, i0), upper, "every upper column has one directly below it");
    assert!(building::unsupported_columns(&doc.model).is_empty());
    let w = rep.warnings.join(" | ");
    // (3000, 2000) is the only one inside the room: all the others lie on a ground wall line.
    assert!(w.contains("5 column(s) continue down") && w.contains("1 of them stand inside a room"), "{w}");
    assert!(!w.contains("not carried"), "{w}");
    // The free-standing column carries the whole load of the one above it and none of its own slab.
    let load = |lvl: Id, x: i64, y: i64| doc.model.elements.values().filter(|e| e.kind == Column::KIND && e.level_id == Some(lvl)).filter_map(|e| e.typed::<Column>().ok()).find(|c| c.x == x && c.y == y).and_then(|c| c.axial_service_kn).unwrap();
    assert!((load(i0, 3000, 2000) - load(i1, 3000, 2000)).abs() < 0.11, "{} vs {}", load(i0, 3000, 2000), load(i1, 3000, 2000));
    // A column that is already there within half a column width is not doubled.
    let near = StructOpts { forced_columns: vec![(100, 100)], ..opts() };
    let (c2, _) = structural::sync_commands(&doc.model, &near, Some(i0)).unwrap();
    let mut probe = Document::new(doc.model.clone());
    probe.commit("p", Origin::Generator, c2).unwrap();
    assert_eq!(columns_at(&probe.model, i0).len(), 4, "only the four corners remain once the forced list is the corner alone");
}

// ------------------------------------------------------------------ openings that stand in columns

use nnx_core::typed::{Door, Window};

/// A 4 m wall with a 300 mm column on its centre line at x = 1000, a 900 mm door centred at 1000 and, optionally, a
/// 1500 mm window centred at 1900. Offsets are measured from the wall start.
fn wall_with_column(window: bool) -> (Document, Id, Option<Id>) {
    let mut doc = Document::default();
    let w = Wall { x1: 0, y1: 0, x2: 4000, y2: 0, thickness: 200, height: 3000, length: 4000, is_exterior: true, room_a: "r".into(), room_b: None, extra: Default::default() };
    let wel = element_of(&w, Source::User);
    let wid = wel.id;
    let col = element_of(&Column { x: 1000, y: 0, b: 300, h: 300, grid_ref: "A1".into(), axial_service_kn: None, extra: Default::default() }, Source::Generated);
    let door = element_of(&Door { width: 900, height: 2100, offset_center: 1000, is_egress: false, from: "r".into(), to: "o".into(), opens_into: "r".into(), extra: Default::default() }, Source::Generated).hosted_by(wid);
    let did = door.id;
    let mut cmds = vec![Command::Create(wel), Command::Create(col), Command::Create(door)];
    let mut win_id = None;
    if window {
        let win = element_of(&Window { width: 1500, height: 1200, sill_height: 900, offset_center: 1900, room: "r".into(), extra: Default::default() }, Source::Generated).hosted_by(wid);
        win_id = Some(win.id);
        cmds.push(Command::Create(win));
    }
    doc.commit("wall", Origin::User, cmds).unwrap();
    (doc, did, win_id)
}

fn offset_of(model: &Model, id: Id) -> i64 {
    model.get(&id).unwrap().props["offsetCenter"].as_i64().unwrap()
}

#[test]
fn a_door_in_a_column_moves_to_the_nearest_clear_position_and_stays_clear() {
    let (mut doc, door, _) = wall_with_column(false);
    let (cmds, notes) = building::clear_openings(&doc.model, &[None]);
    assert!(notes.is_empty());
    assert_eq!(cmds.len(), 1);
    // The column face is at 1150; with 50 mm clearance the blocked stretch ends at 1200; a 900 mm door centred at
    // 1200 + 450 = 1650 starts exactly there. The other side (350) would leave only 100 mm to the wall end (< 150).
    doc.commit("clear", Origin::Generator, cmds).unwrap();
    assert_eq!(offset_of(&doc.model, door), 1650);
    // Idempotent: nothing more to do.
    assert!(building::clear_openings(&doc.model, &[None]).0.is_empty());
}

#[test]
fn an_opening_that_cannot_be_moved_clear_is_left_alone_and_named() {
    let (mut doc, door, win) = wall_with_column(true);
    let (cmds, notes) = building::clear_openings(&doc.model, &[None]);
    // The door takes the only gap (1200..2100); the 1500 mm window then has nowhere to go on this wall.
    assert_eq!(cmds.len(), 1, "only the door moves");
    assert!(notes.len() == 1 && notes[0].contains("window") && notes[0].contains("1500"), "{notes:?}");
    doc.commit("clear", Origin::Generator, cmds).unwrap();
    assert_eq!(offset_of(&doc.model, door), 1650);
    assert_eq!(offset_of(&doc.model, win.unwrap()), 1900, "the window is exactly where it was");
}

#[test]
fn openings_clear_of_every_column_are_never_touched() {
    let (mut doc, door, _) = wall_with_column(false);
    doc.commit("far", Origin::User, vec![Command::SetProp { id: door, key: "offsetCenter".into(), value: Some(serde_json::json!(3000)) }]).unwrap();
    let (cmds, notes) = building::clear_openings(&doc.model, &[None]);
    assert!(cmds.is_empty() && notes.is_empty());
    // a column that is not on the wall's line does not block it
    let (mut off, _, _) = wall_with_column(false);
    let col = off.model.elements.values().find(|e| e.kind == Column::KIND).unwrap().id;
    off.commit("move", Origin::User, vec![Command::SetProp { id: col, key: "y".into(), value: Some(serde_json::json!(2000)) }]).unwrap();
    assert!(building::clear_openings(&off.model, &[None]).0.is_empty());
}
