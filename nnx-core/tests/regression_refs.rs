//! Regression suite over several reference projects. Dimensions are SYNTHETIC test values chosen so every expected
//! quantity is computed by hand from the table below, independently of the engine. Each reference is built, quantified,
//! saved to a `.nnx` file, loaded back and unwound with undo; any drift in a quantity or in persistence fails here.

use nnx_core::persist;
use nnx_core::qto::{self, QtoOpts};
use nnx_core::typed::{Typed, Door, Slab, Space, Wall, Window, element_of};
use nnx_core::*;

/// One reference project: a single rectangular room with one exterior wall (door and window in it) and a slab.
struct Reference {
    name: &'static str,
    w: i64,
    h: i64,
    wall_t: i64,
    wall_h: i64,
    slab_t: i64,
    door: (i64, i64),
    window: (i64, i64),
}

const REFERENCES: [Reference; 4] = [
    Reference { name: "small-studio", w: 3000, h: 4000, wall_t: 200, wall_h: 3000, slab_t: 150, door: (900, 2100), window: (1000, 1000) },
    Reference { name: "bedroom", w: 4000, h: 5000, wall_t: 200, wall_h: 3000, slab_t: 150, door: (900, 2100), window: (1500, 1200) },
    Reference { name: "majlis-hall", w: 8000, h: 6000, wall_t: 250, wall_h: 3500, slab_t: 200, door: (1200, 2400), window: (2000, 1500) },
    Reference { name: "long-corridor", w: 12000, h: 2000, wall_t: 200, wall_h: 3200, slab_t: 180, door: (1000, 2100), window: (1200, 1200) },
];

fn build(r: &Reference) -> Document {
    let mut doc = Document::default();
    let area = r.w as f64 * r.h as f64 / 1e6;
    let space = Space {
        func: "bedroom".into(),
        requirement_key: "r1".into(),
        area,
        target_area: area,
        x: 0,
        y: 0,
        w: r.w,
        h: r.h,
        boundary: vec![[0, 0], [r.w, 0], [r.w, r.h], [0, r.h]],
        net_area: None,
        glazing_ratio: None,
        extra: Default::default(),
    };
    let slab = Slab { x: 0, y: 0, w: r.w, h: r.h, thickness: r.slab_t, kind: "solid".into(), room: "r1".into(), extra: Default::default() };
    let wall = Wall {
        x1: 0,
        y1: 0,
        x2: r.w,
        y2: 0,
        thickness: r.wall_t,
        height: r.wall_h,
        length: r.w,
        is_exterior: true,
        room_a: "r1".into(),
        room_b: None,
        extra: Default::default(),
    };
    let door = Door { width: r.door.0, height: r.door.1, offset_center: r.door.0, is_egress: true, from: "r1".into(), to: "outside".into(), opens_into: "r1".into(), extra: Default::default() };
    let window = Window { width: r.window.0, height: r.window.1, sill_height: 900, offset_center: r.w - r.window.0, room: "r1".into(), extra: Default::default() };
    let wall_el = element_of(&wall, Source::User);
    let wall_id = wall_el.id;
    let cmds = vec![
        Command::Create(element_of(&space, Source::User)),
        Command::Create(element_of(&slab, Source::User)),
        Command::Create(wall_el),
        Command::Create(element_of(&door, Source::User).hosted_by(wall_id)),
        Command::Create(element_of(&window, Source::User).hosted_by(wall_id)),
    ];
    doc.commit(r.name, Origin::User, cmds).unwrap_or_else(|e| panic!("{}: {e:?}", r.name));
    doc
}

fn q(r: &qto::Qto, name: &str, code: &str) -> f64 {
    r.get(code).unwrap_or_else(|| panic!("{name}: no item {code}")).quantity
}

fn close(name: &str, code: &str, got: f64, want: f64) {
    assert!((got - want).abs() < 1e-3, "{name} {code}: engine {got} vs hand {want}");
}

#[test]
fn hand_computed_quantities_hold_for_every_reference() {
    for r in &REFERENCES {
        let doc = build(r);
        let res = qto::compute(&doc.model, &QtoOpts::default());
        let (w, h) = (r.w as f64 / 1000.0, r.h as f64 / 1000.0);
        let (wt, wh, st) = (r.wall_t as f64 / 1000.0, r.wall_h as f64 / 1000.0, r.slab_t as f64 / 1000.0);
        let door = r.door.0 as f64 * r.door.1 as f64 / 1e6;
        let win = r.window.0 as f64 * r.window.1 as f64 / 1e6;
        let net = w * wh - door - win;
        close(r.name, "WALL.EXT.AREA", q(&res, r.name, "WALL.EXT.AREA"), net);
        close(r.name, "WALL.EXT.VOL", q(&res, r.name, "WALL.EXT.VOL"), net * wt);
        close(r.name, "WALL.OPENINGS.AREA", q(&res, r.name, "WALL.OPENINGS.AREA"), door + win);
        close(r.name, "WINDOW.AREA", q(&res, r.name, "WINDOW.AREA"), win);
        close(r.name, "SLAB.VOL", q(&res, r.name, "SLAB.VOL"), w * h * st);
        close(r.name, "SLAB.FORM", q(&res, r.name, "SLAB.FORM"), w * h);
        close(r.name, "SPACE.GROSS.AREA", q(&res, r.name, "SPACE.GROSS.AREA"), w * h);
        close(r.name, "DOOR", q(&res, r.name, &format!("DOOR.{}x{}", r.door.0, r.door.1)), 1.0);
        close(r.name, "WINDOW", q(&res, r.name, &format!("WINDOW.{}x{}", r.window.0, r.window.1)), 1.0);
    }
}

#[test]
fn quantities_are_deterministic_and_scale_with_the_reference() {
    let mut slab_volumes = Vec::new();
    for r in &REFERENCES {
        let doc = build(r);
        let a = qto::compute(&doc.model, &QtoOpts::default());
        let b = qto::compute(&doc.model, &QtoOpts::default());
        assert_eq!(a.items, b.items, "{}: the same model gave two answers", r.name);
        assert!(a.items.iter().all(|i| i.quantity.is_finite() && i.quantity >= 0.0), "{}: a quantity is negative or not finite", r.name);
        slab_volumes.push((r.w * r.h * r.slab_t, q(&a, r.name, "SLAB.VOL")));
    }
    // Ordering by hand-computed volume must be the ordering of the engine's volumes.
    let mut by_hand: Vec<usize> = (0..slab_volumes.len()).collect();
    let mut by_engine = by_hand.clone();
    by_hand.sort_by_key(|&i| slab_volumes[i].0);
    by_engine.sort_by(|&a, &b| slab_volumes[a].1.total_cmp(&slab_volumes[b].1));
    assert_eq!(by_hand, by_engine);
}

#[test]
fn every_reference_survives_save_and_load_with_identical_quantities() {
    let dir = std::env::temp_dir().join(format!("nnx-regression-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for r in &REFERENCES {
        let doc = build(r);
        let path = dir.join(format!("{}.nnx", r.name));
        persist::save(&doc.model, &path).unwrap_or_else(|e| panic!("{}: save failed: {e}", r.name));
        let loaded = persist::load(&path).unwrap_or_else(|e| panic!("{}: load failed: {e}", r.name));
        let before = qto::compute(&doc.model, &QtoOpts::default());
        let after = qto::compute(&loaded, &QtoOpts::default());
        assert_eq!(before.items, after.items, "{}: quantities changed across save/load", r.name);
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn undo_returns_every_reference_to_an_empty_model_and_redo_to_the_same_quantities() {
    for r in &REFERENCES {
        let mut doc = build(r);
        let built = qto::compute(&doc.model, &QtoOpts::default());
        assert!(!built.items.is_empty(), "{}: nothing quantified", r.name);
        doc.undo();
        assert!(qto::compute(&doc.model, &QtoOpts::default()).items.is_empty(), "{}: undo left quantities behind", r.name);
        doc.redo();
        assert_eq!(qto::compute(&doc.model, &QtoOpts::default()).items, built.items, "{}: redo changed quantities", r.name);
    }
}

#[test]
fn a_thicker_slab_changes_only_slab_concrete_in_every_reference() {
    for r in &REFERENCES {
        let mut doc = build(r);
        let before = qto::compute(&doc.model, &QtoOpts::default());
        let slab = doc.model.elements.values().find(|e| e.kind == Slab::KIND).map(|e| e.id).unwrap_or_else(|| panic!("{}: no slab", r.name));
        let new_t = r.slab_t + 50;
        doc.commit("thicker", Origin::User, vec![Command::SetProp { id: slab, key: "thickness".into(), value: Some(serde_json::json!(new_t)) }]).unwrap();
        let after = qto::compute(&doc.model, &QtoOpts::default());
        close(r.name, "SLAB.VOL delta", q(&after, r.name, "SLAB.VOL") - q(&before, r.name, "SLAB.VOL"), r.w as f64 * r.h as f64 / 1e6 * 0.05);
        close(r.name, "WALL.EXT.AREA", q(&after, r.name, "WALL.EXT.AREA"), q(&before, r.name, "WALL.EXT.AREA"));
    }
}
