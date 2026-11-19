//! 3D geometry. The golden model is small and SYNTHETIC; every box is checked against hand values and the quantity take-off.

use nnx_core::model3d::{self, Box3};
use nnx_core::qto::{self, QtoOpts};
use nnx_core::typed::{Beam, Column, Door, Slab, Space, Wall, Window, element_of};
use nnx_core::*;

struct Golden {
    doc: Document,
}

/// One 4 m x 5 m room, a 4 m exterior wall with a door and a window, a 4 m partition, one column, one beam.
fn golden() -> Golden {
    let mut doc = Document::default();
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
    let ext_el = element_of(&ext, Source::User);
    let ext_id = ext_el.id;
    let door = Door { width: 900, height: 2100, offset_center: 1000, is_egress: true, from: "r1".into(), to: "outside".into(), opens_into: "r1".into(), extra: Default::default() };
    let window = Window { width: 1500, height: 1200, sill_height: 900, offset_center: 3000, room: "r1".into(), extra: Default::default() };
    let cmds = vec![
        Command::Create(element_of(&space, Source::User)),
        Command::Create(element_of(&slab, Source::User)),
        Command::Create(ext_el),
        Command::Create(element_of(&int, Source::User)),
        Command::Create(element_of(&door, Source::User).hosted_by(ext_id)),
        Command::Create(element_of(&window, Source::User).hosted_by(ext_id)),
        Command::Create(element_of(&column, Source::User)),
        Command::Create(element_of(&beam, Source::User)),
    ];
    doc.commit("golden", Origin::User, cmds).unwrap();
    Golden { doc }
}

fn near(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-9, "{a} vs {b}");
}

fn of_kind<'a>(s: &'a model3d::Scene, kind: &str) -> Vec<&'a Box3> {
    s.boxes.iter().filter(|b| b.kind == kind).collect()
}

#[test]
fn walls_are_cut_around_openings_and_keep_the_quantity_takeoff_volume() {
    let g = golden();
    let s = model3d::build(&g.doc.model);
    let walls = of_kind(&s, "Wall");
    // exterior: [0,0.55] [1.45,2.25] [3.75,4] solid, the lintel over the door, the sill part and lintel of the window; interior: 1
    assert_eq!(walls.iter().filter(|b| b.class == "ext").count(), 6);
    assert_eq!(walls.iter().filter(|b| b.class == "int").count(), 1);
    let ext_vol: f64 = walls.iter().filter(|b| b.class == "ext").map(|b| b.volume()).sum();
    let q = qto::compute(&g.doc.model, &QtoOpts::default());
    near((ext_vol * 1e6).round() / 1e6, 1.662);
    near((ext_vol * 1e6).round() / 1e6, (q.get("WALL.EXT.VOL").unwrap().quantity * 1e6).round() / 1e6);
    let int_vol: f64 = walls.iter().filter(|b| b.class == "int").map(|b| b.volume()).sum();
    near((int_vol * 1e6).round() / 1e6, (q.get("WALL.INT.VOL").unwrap().quantity * 1e6).round() / 1e6);
}

#[test]
fn doors_windows_slab_column_beam_and_floor_sit_where_the_model_says() {
    let g = golden();
    let s = model3d::build(&g.doc.model);
    let leaf = of_kind(&s, "Door")[0];
    assert_eq!(leaf.class, "leaf");
    for (a, b) in leaf.min.iter().zip([0.55, -0.02, 0.0]) {
        near(*a, b);
    }
    for (a, b) in leaf.max.iter().zip([1.45, 0.02, 2.1]) {
        near(*a, b);
    }
    let pane = of_kind(&s, "Window")[0];
    assert_eq!(pane.class, "glass");
    near(pane.min[2], 0.9);
    near(pane.max[2], 2.1);
    near(pane.min[0], 2.25);
    near(pane.max[0], 3.75);
    let slab = of_kind(&s, "Slab")[0];
    assert_eq!((slab.min, slab.max), ([0.0, 0.0, 3.0], [4.0, 5.0, 3.15]));
    let col = of_kind(&s, "Column")[0];
    assert_eq!((col.min, col.max), ([0.0, 0.0, 0.0], [0.3, 0.3, 3.0]));
    let beam = of_kind(&s, "Beam")[0];
    assert_eq!((beam.min, beam.max), ([0.0, -0.1, 2.5], [4.0, 0.1, 3.0]));
    let floor = of_kind(&s, "Space")[0];
    assert_eq!((floor.class.as_str(), floor.min[2], floor.max[2]), ("bedroom", -0.05, 0.0));
    near(s.storey_height, 3.0);
    // overall bounds: walls are centred on their lines, the slab is the top
    near(s.min[2], -0.05);
    near(s.max[2], 3.15);
    near(s.min[1], -0.1);
    near(s.max[1], 5.05);
}

#[test]
fn every_box_names_an_element_of_the_model_and_the_scene_is_stable() {
    let g = golden();
    let s = model3d::build(&g.doc.model);
    for b in &s.boxes {
        assert!(g.doc.model.elements.keys().any(|k| k.to_string() == b.id), "{} {}", b.kind, b.id);
        assert!((0..3).all(|i| b.max[i] > b.min[i]), "{} has a degenerate box", b.kind);
    }
    let again = model3d::build(&g.doc.model);
    assert_eq!(serde_json::to_string(&s).unwrap(), serde_json::to_string(&again).unwrap());
    // an empty model gives an empty scene, not a crash
    let empty = model3d::build(&Model::default());
    assert!(empty.boxes.is_empty() && empty.min == [0.0; 3] && empty.max == [0.0; 3]);
}
