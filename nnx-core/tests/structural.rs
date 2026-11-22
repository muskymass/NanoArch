//! Loads, soil pressure and ratios here are SYNTHETIC test values, not code or site data.

use nnx_core::drawing::{DrawOpts, plan_svg};
use nnx_core::layout::*;
use nnx_core::structural::{self, Loads, Soil, StructOpts, StructPlan};
use nnx_core::typed::{self, Column, Footing, Typed, element_of};
use nnx_core::walls::{self, Plan, WallOpts, WindowOpts};
use nnx_core::*;

fn room(key: &str, func: &str, area: f64, zone: Zone, light: bool) -> RoomReq {
    RoomReq { key: key.into(), func: func.into(), target_area_m2: area, min_width_mm: 2500, zone, needs_light: light }
}

fn brief() -> Brief {
    use Zone::*;
    Brief {
        envelope: Rect { x: 0, y: 0, w: 14_000, h: 18_000 },
        rooms: vec![
            room("majlis", "majlis", 36.0, Public, true),
            room("living", "living", 30.0, Public, true),
            room("dining", "dining", 24.0, Public, true),
            room("kitchen", "kitchen", 20.0, Service, true),
            room("master", "bedroom", 28.0, Private, true),
            room("bed1", "bedroom", 20.0, Private, true),
            room("bed2", "bedroom", 20.0, Private, true),
            room("bed3", "bedroom", 20.0, Private, true),
            room("bath1", "bathroom", 6.0, Service, false),
            room("bath2", "bathroom", 6.0, Service, false),
            room("bath3", "bathroom", 6.0, Service, false),
        ],
        adjacency: vec![],
        entry: Side::South,
        entry_at: None,
    }
}

fn setup() -> (Brief, Layout, Plan) {
    let b = brief();
    let layout = generate(&b, &Options { seed: 7, ..Options::default() }).unwrap().remove(0);
    let plan = walls::build(&layout, b.envelope, b.entry, &WallOpts::default());
    (b, layout, plan)
}

fn with_inputs() -> StructOpts {
    StructOpts {
        loads: Some(Loads { dead_kpa: 5.0, live_kpa: 2.0, source: "synthetic".into() }),
        soil: Some(Soil { bearing_kpa: 150.0, source: "synthetic".into() }),
        floors: 2,
        ..StructOpts::default()
    }
}

fn plan_with(opts: &StructOpts) -> (Brief, Layout, Plan, StructPlan) {
    let (b, layout, plan) = setup();
    let sp = structural::build(&layout, &plan, b.envelope, opts);
    (b, layout, plan, sp)
}

#[test]
fn a_column_stands_at_every_wall_junction() {
    let (b, layout, plan, sp) = plan_with(&StructOpts::default());
    let te = plan.walls.iter().find(|w| w.exterior).unwrap().thickness;
    let e = b.envelope;
    let inset = |v: i64, lo: i64, hi: i64| if v == lo { v + te / 2 } else if v == hi { v - te / 2 } else { v };
    for r in &layout.rooms {
        let c = r.rect;
        for (vx, vy) in [(c.x, c.y), (c.x + c.w, c.y), (c.x, c.y + c.h), (c.x + c.w, c.y + c.h)] {
            let want = (inset(vx, e.x, e.x + e.w), inset(vy, e.y, e.y + e.h));
            assert!(sp.columns.iter().any(|k| (k.x, k.y) == want), "no column at {want:?} (corner of {})", r.key);
        }
    }
    // Columns never sit on top of each other.
    for (i, a) in sp.columns.iter().enumerate() {
        for c in &sp.columns[i + 1..] {
            assert!((a.x - c.x).abs() + (a.y - c.y).abs() > 100, "columns {} and {} coincide", a.grid_ref, c.grid_ref);
        }
    }
    // Grid references are unique per position.
    let refs: std::collections::BTreeSet<_> = sp.columns.iter().map(|c| c.grid_ref.clone()).collect();
    assert_eq!(refs.len(), sp.columns.len());
}

#[test]
fn beams_join_columns_and_run_inside_walls() {
    let (_, _, plan, sp) = plan_with(&StructOpts::default());
    let cols: std::collections::BTreeSet<(i64, i64)> = sp.columns.iter().map(|c| (c.x, c.y)).collect();
    for c in &sp.columns {
        assert!(sp.beams.iter().any(|b| b.a == (c.x, c.y) || b.b == (c.x, c.y)), "column {} has no beam", c.grid_ref);
    }
    for b in &sp.beams {
        assert!(cols.contains(&b.a) && cols.contains(&b.b), "beam {:?}-{:?} does not end on columns", b.a, b.b);
        assert_eq!(b.length, (b.b.0 - b.a.0).abs() + (b.b.1 - b.a.1).abs());
        let horiz = b.a.1 == b.b.1;
        let cl = if horiz { b.a.1 } else { b.a.0 };
        let (lo, hi) = if horiz { (b.a.0, b.b.0) } else { (b.a.1, b.b.1) };
        assert!(
            plan.walls.iter().any(|w| w.horizontal == horiz && (if horiz { w.a.1 } else { w.a.0 }) == cl && w.start <= lo && w.end >= hi),
            "beam {:?}-{:?} is not under a wall",
            b.a,
            b.b
        );
    }
    // Every wall carries at least one beam.
    for w in &plan.walls {
        let cl = if w.horizontal { w.a.1 } else { w.a.0 };
        let has = sp.beams.iter().any(|b| {
            let horiz = b.a.1 == b.b.1;
            let (bcl, lo, hi) = if horiz { (b.a.1, b.a.0, b.b.0) } else { (b.a.0, b.a.1, b.b.1) };
            horiz == w.horizontal && bcl == cl && lo >= w.start && hi <= w.end
        });
        assert!(has, "wall {:?}-{:?} has no beam", w.a, w.b);
    }
}

#[test]
fn one_slab_per_room_covering_the_whole_plan() {
    let (b, layout, _, sp) = plan_with(&StructOpts::default());
    assert_eq!(sp.slabs.len(), layout.rooms.len());
    let area: i64 = sp.slabs.iter().map(|s| s.rect.w * s.rect.h).sum();
    assert_eq!(area, b.envelope.w * b.envelope.h);
    let o = StructOpts::default();
    for s in &sp.slabs {
        let want = (((s.rect.w.min(s.rect.h)) as f64 / o.slab_span_depth / 10.0).ceil() * 10.0) as i64;
        assert_eq!(s.thickness, want, "slab of {}", s.room);
    }
}

#[test]
fn without_loads_or_soil_nothing_is_sized_and_the_plan_says_why() {
    let (_, _, _, none) = plan_with(&StructOpts::default());
    assert!(none.footings.is_empty() && none.columns.iter().all(|c| c.load_kn.is_none()));
    assert!(none.warnings.iter().any(|w| w.contains("floor loads not set")));

    let loads_only = StructOpts { loads: with_inputs().loads, ..StructOpts::default() };
    let (_, _, _, sp) = plan_with(&loads_only);
    assert!(sp.columns.iter().all(|c| c.load_kn.is_some()));
    assert!(sp.footings.is_empty());
    assert!(sp.warnings.iter().any(|w| w.contains("soil bearing capacity not set")));
}

#[test]
fn column_loads_balance_the_slab_loads_and_footings_carry_them() {
    let o = with_inputs();
    let (_, layout, _, sp) = plan_with(&o);
    let loads = o.loads.as_ref().unwrap();
    let expected: f64 = layout.rooms.iter().map(|r| r.rect.area_m2() * (loads.dead_kpa + loads.live_kpa) * o.floors as f64).sum();
    let got: f64 = sp.columns.iter().map(|c| c.load_kn.unwrap()).sum();
    assert!((got - expected).abs() < 1e-6 * expected, "columns carry {got} kN but slabs impose {expected} kN");

    assert_eq!(sp.footings.len(), sp.columns.iter().filter(|c| c.load_kn.unwrap() > 0.0).count(), "one footing per loaded column");
    let q = o.soil.as_ref().unwrap().bearing_kpa;
    for f in &sp.footings {
        let area = (f.size_mm as f64 / 1000.0).powi(2);
        assert!(area * q >= f.load_kn - 1e-9, "footing {} m2 is too small for {} kN", area, f.load_kn);
        // Not wildly oversized: the next smaller 50 mm step would not carry the load.
        let smaller = ((f.size_mm - 50) as f64 / 1000.0).powi(2);
        assert!(smaller * q < f.load_kn + 1e-9, "footing is more than one step oversized");
    }
    assert!(sp.warnings.iter().any(|w| w.contains("self-weight")));
}

#[test]
fn a_span_limit_adds_columns_so_no_beam_is_longer_than_the_limit() {
    let base = plan_with(&StructOpts::default()).3;
    let limited = plan_with(&StructOpts { max_span_mm: Some(3500), ..StructOpts::default() }).3;
    assert!(limited.columns.len() > base.columns.len());
    assert!(limited.beams.iter().all(|b| b.length <= 3500 + 10), "longest beam {}", limited.beams.iter().map(|b| b.length).max().unwrap());
    assert!(!limited.warnings.iter().any(|w| w.contains("max span not set")));
}

#[test]
fn whole_building_with_structure_commits_valid_and_undoes_as_one_step() {
    let o = with_inputs();
    let (b, layout, plan) = setup();
    let wp = walls::place_windows(&layout, &plan, &WallOpts::default(), &WindowOpts::default());
    let sp = structural::build(&layout, &plan, b.envelope, &o);

    let space_cmds = to_commands(&layout, None);
    let ids = walls::space_ids(&space_cmds);
    let mut all = space_cmds;
    all.extend(walls::to_commands(&plan, &ids, None));
    all.extend(walls::window_commands(&plan, &wp, &ids, None));
    all.extend(structural::to_commands(&sp, &o, None));

    let mut doc = Document::default(); // strict
    doc.commit("house", Origin::Generator, all).unwrap();
    assert!(typed::validate_model(&doc.model).is_empty());
    let count = |k: &str| doc.model.elements.values().filter(|e| e.kind == k).count();
    assert_eq!((count("Column"), count("Beam"), count("Slab"), count("Footing")), (sp.columns.len(), sp.beams.len(), sp.slabs.len(), sp.footings.len()));

    // The model refuses an edit that would leave a footing too small for its load.
    let footing = doc.model.elements.values().find(|e| e.kind == "Footing").unwrap().id;
    let tiny = vec![Command::SetProp { id: footing, key: "length".into(), value: Some(serde_json::json!(100)) }];
    assert!(matches!(doc.commit("shrink", Origin::User, tiny).unwrap_err().source, CommandError::Invalid(..)));

    assert!(doc.undo());
    assert!(doc.model.is_empty());
}

#[test]
fn invalid_footings_are_rejected() {
    let mut doc = Document::default();
    let col = Column { x: 0, y: 0, b: 300, h: 300, grid_ref: "A1".into(), axial_service_kn: Some(100.0), extra: Default::default() };
    let col_el = element_of(&col, Source::User);
    let col_id = col_el.id;
    doc.commit("col", Origin::User, vec![Command::Create(col_el)]).unwrap();

    let mk = |len: i64, col: String| Footing {
        x: 0,
        y: 0,
        length: len,
        width: len,
        service_load_kn: 100.0,
        bearing_kpa: 100.0,
        column: col,
        extra: Default::default(),
    };
    // 1.0 m x 1.0 m at 100 kPa carries exactly 100 kN: accepted. 900 mm is not.
    doc.commit("ok", Origin::User, vec![Command::Create(element_of(&mk(1000, col_id.to_string()), Source::User))]).unwrap();
    let small = doc.commit("small", Origin::User, vec![Command::Create(element_of(&mk(900, col_id.to_string()), Source::User))]).unwrap_err();
    assert!(small.to_string().contains("less than"), "{small}");
    let ghost = doc.commit("ghost", Origin::User, vec![Command::Create(element_of(&mk(1000, Id::nil().to_string()), Source::User))]).unwrap_err();
    assert!(ghost.to_string().contains("does not exist"), "{ghost}");
    assert_eq!(Column::KIND, "Column");
}

#[test]
fn structure_overlay_is_optional_and_traces_to_model_elements() {
    let o = with_inputs();
    let (b, layout, plan) = setup();
    let sp = structural::build(&layout, &plan, b.envelope, &o);
    let space_cmds = to_commands(&layout, None);
    let ids = walls::space_ids(&space_cmds);
    let mut all = space_cmds;
    all.extend(walls::to_commands(&plan, &ids, None));
    all.extend(structural::to_commands(&sp, &o, None));
    let mut doc = Document::default();
    doc.commit("house", Origin::Generator, all).unwrap();

    let off = plan_svg(&doc.model, None, &DrawOpts::default()).unwrap();
    assert!(!off.contains(r#"class="column""#) && !off.contains(r#"class="beam""#));

    let on = plan_svg(&doc.model, None, &DrawOpts { show_structure: true, ..DrawOpts::default() }).unwrap();
    let x = roxmltree::Document::parse(&on).expect("valid XML");
    let n = |class: &str| x.descendants().filter(|e| e.attribute("class") == Some(class)).count();
    assert_eq!(n("column"), sp.columns.len());
    assert_eq!(n("beam"), sp.beams.len());
    for e in x.descendants().filter(|e| matches!(e.attribute("class"), Some("column" | "beam"))) {
        let id: Id = e.attribute("data-id").unwrap().parse().unwrap();
        assert!(doc.model.get(&id).is_some());
    }
}

/// Set NNX_STRUCT_SVG to a file path to keep the structural drawing for a visual check.
#[test]
fn write_structural_drawing_when_requested() {
    let Ok(path) = std::env::var("NNX_STRUCT_SVG") else { return };
    let o = StructOpts { max_span_mm: Some(5000), ..with_inputs() };
    let (b, layout, plan) = setup();
    let wp = walls::place_windows(&layout, &plan, &WallOpts::default(), &WindowOpts::default());
    let sp = structural::build_with_openings(&layout, &plan, &wp, b.envelope, &o);
    let space_cmds = to_commands(&layout, None);
    let ids = walls::space_ids(&space_cmds);
    let mut all = space_cmds;
    all.extend(walls::to_commands(&plan, &ids, None));
    all.extend(walls::window_commands(&plan, &wp, &ids, None));
    all.extend(structural::to_commands(&sp, &o, None));
    let mut doc = Document::default();
    doc.commit("house", Origin::Generator, all).unwrap();
    let opts = DrawOpts { title: "Villa - structural layout (preliminary, synthetic inputs)".into(), show_structure: true, ..DrawOpts::default() };
    std::fs::write(path, plan_svg(&doc.model, None, &opts).unwrap()).unwrap();
}

#[test]
fn extra_columns_stay_clear_of_door_and_window_openings() {
    let (b, layout, plan) = setup();
    let wp = walls::place_windows(&layout, &plan, &WallOpts::default(), &WindowOpts::default());
    let o = StructOpts { max_span_mm: Some(3000), ..StructOpts::default() };
    let sp = structural::build_with_openings(&layout, &plan, &wp, b.envelope, &o);
    assert!(sp.columns.len() > structural::build(&layout, &plan, b.envelope, &StructOpts::default()).columns.len());

    let mut spans: Vec<(bool, i64, i64, i64)> = Vec::new(); // horizontal, centre line, from, to
    for d in &plan.doors {
        let w = &plan.walls[d.wall];
        let (c, line) = (w.start + d.offset, if w.horizontal { w.a.1 } else { w.a.0 });
        spans.push((w.horizontal, line, c - d.width / 2, c + d.width / 2));
    }
    for v in &wp.windows {
        let w = &plan.walls[v.wall];
        let (c, line) = (w.start + v.offset, if w.horizontal { w.a.1 } else { w.a.0 });
        spans.push((w.horizontal, line, c - v.width / 2, c + v.width / 2));
    }
    let half = o.column_mm / 2;
    for c in &sp.columns {
        for &(horiz, line, lo, hi) in &spans {
            let (along, across) = if horiz { (c.x, c.y) } else { (c.y, c.x) };
            if across == line {
                assert!(along + half <= lo || along - half >= hi, "column {} overlaps an opening ({lo}..{hi} on line {line})", c.grid_ref);
            }
        }
    }
    // Every beam is still within the limit, or the plan says why not.
    let long = sp.beams.iter().filter(|b| b.length > 3000 + 10).count();
    assert!(long == 0 || sp.warnings.iter().any(|w| w.contains("openings block")), "{long} beams exceed the span limit without a warning");
}

// ------------------------------------------------------------------ structure follows the model

fn house_doc() -> (Document, StructOpts) {
    let (b, layout, plan) = setup();
    let wp = walls::place_windows(&layout, &plan, &WallOpts::default(), &WindowOpts::default());
    let o = StructOpts { max_span_mm: Some(5000), ..with_inputs() };
    let sp = structural::build_with_openings(&layout, &plan, &wp, b.envelope, &o);
    let space_cmds = to_commands(&layout, None);
    let ids = walls::space_ids(&space_cmds);
    let mut all = space_cmds;
    all.extend(walls::to_commands(&plan, &ids, None));
    all.extend(walls::window_commands(&plan, &wp, &ids, None));
    all.extend(structural::to_commands(&sp, &o, None));
    let mut doc = Document::default();
    doc.commit("house", Origin::Generator, all).unwrap();
    (doc, o)
}

fn count(doc: &Document, kind: &str) -> usize {
    doc.model.elements.values().filter(|e| e.kind == kind).count()
}

#[test]
fn structure_read_from_the_model_equals_the_one_the_generator_made() {
    // Two independent derivations (from the layout and plan, and from the model's elements)
    // must agree exactly, or the model is not the single source of truth.
    let (doc, o) = house_doc();
    assert!(!structural::needs_sync(&doc.model, &o, None).unwrap());
    let r = structural::sync(&mut { doc }, &o, None).unwrap();
    assert_eq!((r.created, r.removed), (0, 0));
    assert!(r.unchanged > 50);
}

#[test]
fn editing_walls_in_the_model_updates_beams_and_undo_restores_them() {
    let (mut doc, o) = house_doc();
    let exterior: Vec<Id> = doc.model.elements.values().filter(|e| e.kind == "Wall" && e.props["isExterior"] == serde_json::json!(true)).map(|e| e.id).collect();
    let edit: Vec<Command> = exterior
        .iter()
        .map(|&id| Command::SetProp { id, key: "thickness".into(), value: Some(serde_json::json!(400)) })
        .collect();
    doc.commit("thicker exterior walls", Origin::User, edit).unwrap();
    assert!(structural::needs_sync(&doc.model, &o, None).unwrap(), "beams under exterior walls take the wall thickness");
    let edited = doc.model.state_hash();

    let r = structural::sync(&mut doc, &o, None).unwrap();
    assert!(r.created > 0 && r.removed > 0 && r.created == r.removed, "{r:?}");
    assert!(!structural::needs_sync(&doc.model, &o, None).unwrap());
    let widths: std::collections::BTreeSet<i64> = doc
        .model
        .elements
        .values()
        .filter(|e| e.kind == "Beam")
        .map(|e| e.props["b"].as_i64().unwrap())
        .collect();
    assert!(widths.contains(&400), "exterior beams are now 400 mm wide: {widths:?}");

    // The whole regeneration is one undoable step.
    assert!(doc.undo());
    assert_eq!(doc.model.state_hash(), edited);
    // A second sync after the first is a no-op.
    structural::sync(&mut doc, &o, None).unwrap();
    let again = structural::sync(&mut doc, &o, None).unwrap();
    assert_eq!((again.created, again.removed), (0, 0));
}

#[test]
fn removing_a_partition_removes_its_beams_and_the_rest_stays() {
    let (mut doc, o) = house_doc();
    let beams_before = count(&doc, "Beam");
    let columns_before = count(&doc, "Column");
    // The longest interior wall, with the doors it hosts removed first.
    let wall = doc
        .model
        .elements
        .values()
        .filter(|e| e.kind == "Wall" && e.props["isExterior"] == serde_json::json!(false))
        .max_by_key(|e| e.props["length"].as_i64().unwrap())
        .unwrap()
        .id;
    let mut cmds: Vec<Command> = doc.model.dependents(&wall).into_iter().map(Command::Delete).collect();
    cmds.push(Command::Delete(wall));
    doc.commit("remove partition", Origin::User, cmds).unwrap();

    let r = structural::sync(&mut doc, &o, None).unwrap();
    assert!(r.removed > 0, "{r:?}");
    assert!(count(&doc, "Beam") < beams_before, "the beam under the removed wall is gone");
    // Every junction column (room corners) stays; only span-limit columns that stood on the removed wall go.
    let input = structural::Input::from_model(&doc.model, None).unwrap().unwrap();
    let junctions = structural::build_input(&input, &StructOpts::default());
    let at: std::collections::BTreeSet<(i64, i64)> = doc
        .model
        .elements
        .values()
        .filter(|e| e.kind == "Column")
        .map(|e| (e.props["x"].as_i64().unwrap(), e.props["y"].as_i64().unwrap()))
        .collect();
    for c in &junctions.columns {
        assert!(at.contains(&(c.x, c.y)), "junction column {} is missing", c.grid_ref);
    }
    assert!(count(&doc, "Column") <= columns_before);
    assert!(typed::validate_model(&doc.model).is_empty());
}

#[test]
fn locked_structure_survives_a_sync_and_an_empty_model_clears_the_rest() {
    let (mut doc, o) = house_doc();
    let col = doc.model.elements.values().find(|e| e.kind == "Column").unwrap().id;
    doc.commit("lock", Origin::User, vec![Command::SetLocked { id: col, locked: true }]).unwrap();

    // Remove every wall and space: everything generated and unlocked must go, the locked column stays.
    let mut cmds: Vec<Command> = Vec::new();
    for kind in ["Door", "Window", "Wall", "Space"] {
        cmds.extend(doc.model.elements.values().filter(|e| e.kind == kind).map(|e| Command::Delete(e.id)));
    }
    doc.commit("clear architecture", Origin::User, cmds).unwrap();
    let r = structural::sync(&mut doc, &o, None).unwrap();
    assert_eq!(r.kept_locked, 1);
    assert!(doc.model.get(&col).is_some(), "a locked element is never deleted by a sync");
    assert_eq!(count(&doc, "Beam") + count(&doc, "Slab") + count(&doc, "Footing"), 0);
    assert_eq!(count(&doc, "Column"), 1);
}

/// A damaged element (imported file, older version) must stop a sync, never be mistaken for
/// "no structure needed". Each case adds one unreadable element to a full house.
#[test]
fn an_unreadable_element_stops_sync_and_deletes_nothing() {
    use nnx_core::structural::SyncError;
    let broken: Vec<(&str, Element)> = vec![
        ("Wall", Element::new("Wall", Discipline::Arc, Source::Imported).with_prop("x1", 0)),
        ("Space", Element::new("Space", Discipline::Arc, Source::Imported).with_prop("func", "bedroom")),
    ];
    for (kind, bad) in broken {
        let (mut doc, o) = house_doc();
        doc.strict = false; // an import path that does not validate
        doc.commit("import a damaged element", Origin::User, vec![Command::Create(bad)]).unwrap();
        let before = doc.model.state_hash();
        let structure = count(&doc, "Column") + count(&doc, "Beam") + count(&doc, "Slab") + count(&doc, "Footing");
        assert!(structure > 50);

        let err = structural::sync(&mut doc, &o, None).unwrap_err();
        assert!(matches!(err, SyncError::Unreadable(_)), "{kind}: {err}");
        assert!(err.to_string().contains(kind), "the message names the damaged element: {err}");
        assert_eq!(doc.model.state_hash(), before, "{kind}: nothing changed");
        assert!(structural::needs_sync(&doc.model, &o, None).is_err(), "{kind}: needs_sync says it cannot judge");
    }
}

#[test]
fn a_door_without_readable_geometry_also_stops_sync() {
    let (mut doc, o) = house_doc();
    let door = doc.model.elements.values().find(|e| e.kind == "Door").unwrap().id;
    doc.strict = false;
    doc.commit("damage a door", Origin::User, vec![Command::SetProp { id: door, key: "offsetCenter".into(), value: None }]).unwrap();
    let before = doc.model.state_hash();
    assert!(structural::sync(&mut doc, &o, None).is_err());
    assert_eq!(doc.model.state_hash(), before);
}
