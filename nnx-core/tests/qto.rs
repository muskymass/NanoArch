//! Dimensions are SYNTHETIC test values. The golden case is small enough to check by hand.

use nnx_core::layout::*;
use nnx_core::qto::{self, QtoOpts};
use nnx_core::structural::{self, Loads, Soil, StructOpts};
use nnx_core::typed::{Beam, Column, Door, Slab, Space, Wall, Window, element_of};
use nnx_core::walls::{self, WallOpts, WindowOpts};
use nnx_core::*;

// ------------------------------------------------------------------ golden model

struct Golden {
    doc: Document,
    door: Id,
    slab: Id,
}

/// One 4 m x 5 m room, a 4 m exterior wall with a door and a window, a 4 m partition,
/// one column and one beam under the slab.
fn golden() -> Golden {
    let mut doc = Document::default(); // strict
    let space = Space {
        func: "bedroom".into(),
        requirement_key: "r1".into(),
        area: 20.0,
        target_area: 20.0,
        x: 0,
        y: 0,
        w: 4000,
        h: 5000,
        boundary: vec![[0, 0], [4000, 0], [4000, 5000], [0, 5000]],
        net_area: Some(18.5),
        glazing_ratio: None,
        extra: Default::default(),
    };
    let slab = Slab { x: 0, y: 0, w: 4000, h: 5000, thickness: 150, kind: "solid".into(), room: "r1".into(), extra: Default::default() };
    let ext = Wall {
        x1: 0,
        y1: 0,
        x2: 4000,
        y2: 0,
        thickness: 200,
        height: 3000,
        length: 4000,
        is_exterior: true,
        room_a: "r1".into(),
        room_b: None,
        extra: Default::default(),
    };
    let int = Wall { y1: 5000, y2: 5000, thickness: 100, is_exterior: false, ..ext.clone() };
    let column = Column { x: 150, y: 150, b: 300, h: 300, grid_ref: "A1".into(), axial_service_kn: None, extra: Default::default() };
    let beam = Beam { x1: 0, y1: 0, x2: 4000, y2: 0, b: 200, h: 500, length: 4000, extra: Default::default() };

    let (space_el, slab_el) = (element_of(&space, Source::User), element_of(&slab, Source::User));
    let slab_id = slab_el.id;
    let ext_el = element_of(&ext, Source::User);
    let ext_id = ext_el.id;
    let door = Door { width: 900, height: 2100, offset_center: 1000, is_egress: true, from: "r1".into(), to: "outside".into(), opens_into: "r1".into(), extra: Default::default() };
    let window = Window { width: 1500, height: 1200, sill_height: 900, offset_center: 3000, room: "r1".into(), extra: Default::default() };
    let door_el = element_of(&door, Source::User).hosted_by(ext_id);
    let door_id = door_el.id;
    let cmds = vec![
        Command::Create(space_el),
        Command::Create(slab_el),
        Command::Create(ext_el),
        Command::Create(element_of(&int, Source::User)),
        Command::Create(door_el),
        Command::Create(element_of(&window, Source::User).hosted_by(ext_id)),
        Command::Create(element_of(&column, Source::User)),
        Command::Create(element_of(&beam, Source::User)),
    ];
    doc.commit("golden", Origin::User, cmds).unwrap();
    Golden { doc, door: door_id, slab: slab_id }
}

fn q(qto: &qto::Qto, code: &str) -> f64 {
    qto.get(code).unwrap_or_else(|| panic!("no item {code}")).quantity
}

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-3, "{a} != {b}");
}

#[test]
fn hand_checked_quantities_of_the_golden_room() {
    let g = golden();
    let r = qto::compute(&g.doc.model, &QtoOpts::default());

    // Exterior wall: 4 x 3 = 12 m2 minus door 0.9 x 2.1 = 1.89 and window 1.5 x 1.2 = 1.80 -> 8.31 m2.
    close(q(&r, "WALL.EXT.AREA"), 8.31);
    close(q(&r, "WALL.EXT.VOL"), 8.31 * 0.2);
    close(q(&r, "WALL.INT.AREA"), 12.0);
    close(q(&r, "WALL.INT.VOL"), 1.2);
    close(q(&r, "WALL.OPENINGS.AREA"), 3.69);
    close(q(&r, "WALL.FACES.AREA"), 2.0 * (8.31 + 12.0));
    close(q(&r, "DOOR.900x2100"), 1.0);
    close(q(&r, "WINDOW.1500x1200"), 1.0);
    close(q(&r, "WINDOW.AREA"), 1.8);
    // Slab 4 x 5 x 0.15 = 3.0 m3; soffit 20 m2.
    close(q(&r, "SLAB.VOL"), 3.0);
    close(q(&r, "SLAB.FORM"), 20.0);
    // Beam 4 m x 0.2 m, 0.5 deep but 0.15 is the slab: 0.35 below it.
    close(q(&r, "BEAM.VOL"), 4.0 * 0.2 * 0.35);
    close(q(&r, "BEAM.FORM"), 4.0 * (0.2 + 0.7));
    // Column 0.3 x 0.3 x 3.0 m.
    close(q(&r, "COLUMN.VOL"), 0.27);
    close(q(&r, "COLUMN.FORM"), 2.0 * 0.6 * 3.0);
    close(q(&r, "COLUMN.COUNT"), 1.0);
    // Finishes use the net area; skirting is the perimeter (18 m) minus the 0.9 m door.
    close(q(&r, "FLOOR.AREA.bedroom"), 18.5);
    close(q(&r, "CEILING.AREA.bedroom"), 18.5);
    close(q(&r, "SKIRTING.bedroom"), 17.1);
    close(q(&r, "SPACE.GROSS.AREA"), 20.0);
}

#[test]
fn waste_is_applied_only_where_the_user_asked() {
    let g = golden();
    let mut opts = QtoOpts::default();
    opts.waste_pct.insert("SLAB.VOL".into(), 5.0);
    let r = qto::compute(&g.doc.model, &opts);
    let slab = r.get("SLAB.VOL").unwrap();
    assert_eq!(slab.waste_pct, Some(5.0));
    close(slab.ordered(), 3.15);
    for i in r.items.iter().filter(|i| i.code != "SLAB.VOL") {
        assert_eq!(i.waste_pct, None, "{} got a waste allowance nobody asked for", i.code);
        close(i.ordered(), i.quantity);
    }
}

#[test]
fn quantities_follow_edits_to_the_model() {
    let mut g = golden();
    let before = qto::compute(&g.doc.model, &QtoOpts::default());

    // Thicker slab: concrete volume changes by area x delta.
    let edit = vec![Command::SetProp { id: g.slab, key: "thickness".into(), value: Some(serde_json::json!(200)) }];
    g.doc.commit("thicker", Origin::User, edit).unwrap();
    let after = qto::compute(&g.doc.model, &QtoOpts::default());
    close(q(&after, "SLAB.VOL") - q(&before, "SLAB.VOL"), 20.0 * 0.05);
    // The beam below a thicker slab gets shallower.
    close(q(&after, "BEAM.VOL"), 4.0 * 0.2 * 0.30);

    // Deleting the door gives its area back to the wall.
    g.doc.commit("no door", Origin::User, vec![Command::Delete(g.door)]).unwrap();
    let no_door = qto::compute(&g.doc.model, &QtoOpts::default());
    close(q(&no_door, "WALL.EXT.AREA") - q(&after, "WALL.EXT.AREA"), 1.89);
    assert!(no_door.get("DOOR.900x2100").is_none());
    // Undo restores the earlier quantities exactly.
    g.doc.undo();
    g.doc.undo();
    assert_eq!(qto::compute(&g.doc.model, &QtoOpts::default()).items, before.items);
}

#[test]
fn csv_export_has_a_bom_quotes_commas_and_one_row_per_item() {
    let g = golden();
    let r = qto::compute(&g.doc.model, &QtoOpts::default());
    let csv = r.to_csv();
    assert!(csv.starts_with('\u{feff}'));
    assert_eq!(csv.lines().count(), r.items.len() + 1);
    assert!(csv.contains("\"Exterior walls, area net of openings\""));
    assert!(csv.contains("WALL.EXT.AREA,"));
}

#[test]
fn level_filter_warnings_and_determinism() {
    let g = golden();
    let none = qto::compute(&g.doc.model, &QtoOpts { level: Some(Id::now_v7()), ..QtoOpts::default() });
    assert!(none.items.is_empty());

    let a = qto::compute(&g.doc.model, &QtoOpts::default());
    let b = qto::compute(&g.doc.model, &QtoOpts::default());
    assert_eq!(a.items, b.items);
    assert!(a.warnings.iter().any(|w| w.contains("reinforcement")));

    // A column with no wall anywhere cannot get a height: skipped with a warning.
    let mut doc = Document::default();
    let col = Column { x: 0, y: 0, b: 300, h: 300, grid_ref: "A1".into(), axial_service_kn: None, extra: Default::default() };
    doc.commit("c", Origin::User, vec![Command::Create(element_of(&col, Source::User))]).unwrap();
    let r = qto::compute(&doc.model, &QtoOpts::default());
    assert!(r.get("COLUMN.VOL").is_none());
    assert!(r.warnings.iter().any(|w| w.contains("storey height")));
}

// ------------------------------------------------------------------ whole house

fn room(key: &str, func: &str, area: f64, zone: Zone, light: bool) -> RoomReq {
    RoomReq { key: key.into(), func: func.into(), target_area_m2: area, min_width_mm: 2500, zone, needs_light: light }
}

fn house() -> (Document, Brief, walls::Plan, walls::WindowPlan) {
    use Zone::*;
    let b = Brief {
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
        wall_mm: None,
        wall_height_mm: None,
    };
    let layout = generate(&b, &Options { seed: 7, ..Options::default() }).unwrap().remove(0);
    let plan = walls::build(&layout, b.envelope, b.entry, &WallOpts::default());
    let wp = walls::place_windows(&layout, &plan, &WallOpts::default(), &WindowOpts::default());
    let so = StructOpts {
        loads: Some(Loads { dead_kpa: 5.0, live_kpa: 2.0, source: "synthetic".into() }),
        soil: Some(Soil { bearing_kpa: 150.0, source: "synthetic".into() }),
        max_span_mm: Some(5000),
        ..StructOpts::default()
    };
    let sp = structural::build_with_openings(&layout, &plan, &wp, b.envelope, &so);

    let space_cmds = to_commands(&layout, None);
    let ids = walls::space_ids(&space_cmds);
    let mut all = space_cmds;
    all.extend(walls::to_commands(&plan, &ids, None));
    all.extend(walls::window_commands(&plan, &wp, &ids, None));
    all.extend(structural::to_commands(&sp, &so, None));
    let mut doc = Document::default();
    doc.commit("house", Origin::Generator, all).unwrap();
    (doc, b, plan, wp)
}

#[test]
fn house_quantities_reconcile_with_independent_calculations() {
    let (doc, b, plan, wp) = house();
    let r = qto::compute(&doc.model, &QtoOpts::default());

    // Areas tile the envelope exactly.
    close(q(&r, "SLAB.FORM"), 14.0 * 18.0);
    close(q(&r, "SPACE.GROSS.AREA"), 14.0 * 18.0);

    // Openings: counts match the plans, and the area deducted matches door + window areas.
    let count = |prefix: &str| r.items.iter().filter(|i| i.code.starts_with(prefix) && i.code != "WINDOW.AREA").map(|i| i.quantity).sum::<f64>();
    assert_eq!(count("DOOR.") as usize, plan.doors.len());
    assert_eq!(count("WINDOW.") as usize, wp.windows.len());
    let open_mm2: f64 = plan.doors.iter().map(|d| (d.width * d.height) as f64).sum::<f64>()
        + wp.windows.iter().map(|w| (w.width * w.height) as f64).sum::<f64>();
    close(q(&r, "WALL.OPENINGS.AREA"), open_mm2 / 1e6);

    // Wall volume recomputed from the wall plan, independent of the model reader.
    let wall_vol = |ext: bool| -> f64 {
        plan.walls
            .iter()
            .enumerate()
            .filter(|(_, w)| w.exterior == ext)
            .map(|(wi, w)| {
                let open: i64 = plan.doors.iter().filter(|d| d.wall == wi).map(|d| d.width * d.height).sum::<i64>()
                    + wp.windows.iter().filter(|v| v.wall == wi).map(|v| v.width * v.height).sum::<i64>();
                (w.length() * plan.wall_height_mm - open) as f64 * w.thickness as f64 / 1e9
            })
            .sum()
    };
    close(q(&r, "WALL.EXT.VOL"), wall_vol(true));
    close(q(&r, "WALL.INT.VOL"), wall_vol(false));
    let _ = b;
}

#[test]
fn every_structural_and_architectural_element_is_counted_and_every_source_exists() {
    let (doc, _, _, _) = house();
    let r = qto::compute(&doc.model, &QtoOpts::default());
    let counted: std::collections::BTreeSet<Id> = r.items.iter().flat_map(|i| i.sources.iter().copied()).collect();
    for e in doc.model.elements.values() {
        assert!(counted.contains(&e.id), "{} {} is in no quantity", e.kind, e.id);
    }
    for i in &r.items {
        for s in &i.sources {
            assert!(doc.model.get(s).is_some(), "{} cites a missing element", i.code);
        }
    }
    assert!(r.warnings.iter().any(|w| w.contains("footing depth")));
    assert!(r.warnings.iter().any(|w| w.contains("reinforcement")));
    assert!(r.get("FOOTING.AREA").is_some() && r.get("COLUMN.VOL").is_some() && r.get("BEAM.VOL").is_some());
}

/// Set NNX_QTO_CSV to a file path to keep the villa quantities for inspection.
#[test]
fn write_villa_quantities_when_requested() {
    let Ok(path) = std::env::var("NNX_QTO_CSV") else { return };
    let (doc, _, _, _) = house();
    std::fs::write(path, qto::compute(&doc.model, &QtoOpts::default()).to_csv()).unwrap();
}

// Regression: tie-beam quantities used to appear in *every* phase export,
// making the sum of the phase CSVs five times the unfiltered total.
#[test]
fn tie_beams_are_counted_once_across_construction_phases() {
    use nnx_core::typed::TieBeam;
    let mut g = golden();
    for phase in [Some("NEW"), None] {
        let tie = TieBeam {
            x1: 0, y1: 0, x2: 4000, y2: 0,
            width: 250, depth: 400,
            from: "unused".into(), to: "unused".into(),
            extra: Default::default(),
        };
        // QTO reads geometry only; no fictitious footing design is asserted.
        let mut el = element_of(&tie, Source::Generated);
        if let Some(p) = phase {
            el.props.insert("phase".into(), serde_json::json!(p));
        }
        g.doc.model.elements.insert(el.id, el);
    }
    let total = qto::compute(&g.doc.model, &QtoOpts::default());
    close(q(&total, "TIEBEAM.COUNT"), 2.0);
    close(q(&total, "TIEBEAM.VOL"), 0.8);
    let mut count = 0.0;
    let mut vol = 0.0;
    for phase in ["NEW", "EXISTING", "TO_BE_DEMOLISHED", "TO_BE_MODIFIED", "TEMPORARY", "UNVERIFIED", "UNSET"] {
        let takeoff = qto::compute(&g.doc.model, &QtoOpts { phase: Some(phase.into()), ..QtoOpts::default() });
        count += takeoff.get("TIEBEAM.COUNT").map_or(0.0, |i| i.quantity);
        vol += takeoff.get("TIEBEAM.VOL").map_or(0.0, |i| i.quantity);
    }
    close(count, q(&total, "TIEBEAM.COUNT"));
    close(vol, q(&total, "TIEBEAM.VOL"));
}

// Regression: a phase-filtered space must still deduct a door of a different
// construction status; the door does not cease to exist geometrically.
#[test]
fn phased_skirting_keeps_cross_phase_door_deductions() {
    let mut g = golden();
    for element in g.doc.model.elements.values_mut() {
        match element.kind.as_str() {
            "Space" => { element.props.insert("phase".into(), serde_json::json!("NEW")); }
            "Door" => { element.props.insert("phase".into(), serde_json::json!("EXISTING")); }
            _ => (),
        }
    }
    let new = qto::compute(&g.doc.model, &QtoOpts { phase: Some("NEW".into()), ..QtoOpts::default() });
    close(q(&new, "SKIRTING.bedroom"), 17.1);
    let total = qto::compute(&g.doc.model, &QtoOpts::default());
    close(q(&new, "SKIRTING.bedroom"), q(&total, "SKIRTING.bedroom"));
}
