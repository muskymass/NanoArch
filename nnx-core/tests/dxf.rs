//! DXF export. The golden model is small and SYNTHETIC; every value is checked by hand against the written file.

use nnx_core::dxf::{self, DxfOpts, dxf_text};
use nnx_core::typed::{Beam, Column, Door, Level, Slab, Space, Wall, Window, element_of};
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
    let ext = Wall { x1: 0, y1: 0, x2: 4000, y2: 0, thickness: 200, height: 3000, length: 4000, is_exterior: true, room_a: "r1".into(), room_b: None, extra: Default::default() };
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

fn opts() -> DxfOpts {
    let mut labels = std::collections::BTreeMap::new();
    labels.insert("r1".to_string(), "غرفة".to_string());
    DxfOpts { title: "Test villa".into(), labels, mep: true }
}

/// The file as (group code, value) pairs.
fn pairs(text: &str) -> Vec<(i32, String)> {
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len() % 2, 0, "group codes and values come in pairs");
    lines.chunks(2).map(|c| (c[0].trim().parse::<i32>().expect("a numeric group code"), c[1].to_string())).collect()
}

/// Entities of the ENTITIES section: (type, layer, its pairs including vertices).
type Entity = (String, String, Vec<(i32, String)>);

fn entities(text: &str) -> Vec<Entity> {
    let p = pairs(text);
    let start = p.iter().position(|(c, v)| *c == 2 && v == "ENTITIES").unwrap() + 1;
    let end = p.iter().rposition(|(c, v)| *c == 0 && v == "ENDSEC").unwrap();
    let mut out: Vec<Entity> = Vec::new();
    for (c, v) in &p[start..end] {
        if *c == 0 && v != "VERTEX" && v != "SEQEND" {
            out.push((v.clone(), String::new(), Vec::new()));
        } else if let Some(last) = out.last_mut() {
            if *c == 8 && last.1.is_empty() {
                last.1 = v.clone();
            }
            last.2.push((*c, v.clone()));
        }
    }
    out
}

fn on(text: &str, layer: &str) -> Vec<(String, Vec<(i32, String)>)> {
    entities(text).into_iter().filter(|e| e.1 == layer).map(|e| (e.0, e.2)).collect()
}

fn num(v: &[(i32, String)], code: i32) -> f64 {
    v.iter().find(|p| p.0 == code).unwrap().1.parse().unwrap()
}

fn texts(text: &str, layer: &str) -> Vec<String> {
    on(text, layer).iter().filter(|e| e.0 == "TEXT").map(|e| e.1.iter().find(|p| p.0 == 1).unwrap().1.clone()).collect()
}

#[test]
fn the_file_has_the_r12_structure_with_every_layer_defined() {
    let g = golden();
    let out = dxf::export_dxf(&g.doc.model, &[None], &opts()).unwrap();
    let p = pairs(&out.text);
    assert!(out.text.is_ascii());
    assert!(p.contains(&(1, "AC1009".to_string())) && p.contains(&(9, "$INSUNITS".to_string())));
    assert_eq!(p.iter().filter(|(c, v)| *c == 0 && v == "SECTION").count(), p.iter().filter(|(c, v)| *c == 0 && v == "ENDSEC").count());
    assert_eq!(p.iter().filter(|(c, v)| *c == 0 && v == "TABLE").count(), p.iter().filter(|(c, v)| *c == 0 && v == "ENDTAB").count());
    assert_eq!(p.last().unwrap(), &(0, "EOF".to_string()));
    // every layer used by an entity is defined in the LAYER table
    let defined: Vec<&String> = p.windows(2).filter(|w| w[0] == (0, "LAYER".to_string()) && w[1].0 == 2).map(|w| &w[1].1).collect();
    for (_, layer, _) in entities(&out.text) {
        assert!(defined.iter().any(|d| **d == layer), "layer {layer} is not defined");
    }
    // every polyline is closed by a SEQEND
    let start = p.iter().position(|(c, v)| *c == 2 && v == "ENTITIES").unwrap();
    let polys = p[start..].iter().filter(|(c, v)| *c == 0 && v == "POLYLINE").count();
    assert_eq!(polys, p[start..].iter().filter(|(c, v)| *c == 0 && v == "SEQEND").count());
}

#[test]
fn walls_are_cut_around_openings_and_doors_swing_into_their_room() {
    let g = golden();
    let text = dxf::export_dxf(&g.doc.model, &[None], &opts()).unwrap().text;
    // the exterior wall (4000 long, door 550..1450, window 2250..3750): three pieces; the partition: one
    assert_eq!(on(&text, "A-WALL-EXT").len(), 3);
    assert_eq!(on(&text, "A-WALL-INT").len(), 1);
    let first = on(&text, "A-WALL-EXT").remove(0);
    let flat: Vec<&(i32, String)> = first.1.iter().filter(|(c, _)| *c == 10 || *c == 20).collect();
    let pts: Vec<(f64, f64)> = flat.chunks(2).map(|w| (w[0].1.parse().unwrap(), w[1].1.parse().unwrap())).collect();
    assert_eq!(pts, vec![(0.0, -100.0), (550.0, -100.0), (550.0, 100.0), (0.0, 100.0)]);
    // the door: two jambs, the open leaf and the swing arc = 4 entities; the room is to the north, so it swings north
    let door = on(&text, "A-DOOR");
    assert_eq!(door.len(), 4);
    let arc = door.iter().find(|d| d.0 == "ARC").unwrap();
    assert_eq!((num(&arc.1, 10), num(&arc.1, 20), num(&arc.1, 40), num(&arc.1, 50), num(&arc.1, 51)), (550.0, 100.0, 900.0, 0.0, 90.0));
    let leaf: Vec<&(String, Vec<(i32, String)>)> = door.iter().filter(|d| d.0 == "LINE" && num(&d.1, 10) == 550.0 && num(&d.1, 20) == 100.0).collect();
    assert_eq!(leaf.len(), 1);
    assert_eq!((num(&leaf[0].1, 11), num(&leaf[0].1, 21)), (550.0, 1000.0), "the leaf is 900 mm long, perpendicular to the wall");
    // the window: frame and glass line
    assert_eq!(on(&text, "A-GLAZ").len(), 2);
}

#[test]
fn structure_rooms_text_and_dimensions_are_where_the_model_says() {
    let g = golden();
    let out = dxf::export_dxf(&g.doc.model, &[None], &opts()).unwrap();
    let text = &out.text;
    assert_eq!(on(text, "S-COLS").len(), 2, "a fill and an outline");
    let solid = on(text, "S-COLS").into_iter().find(|e| e.0 == "SOLID").unwrap();
    assert_eq!((num(&solid.1, 10), num(&solid.1, 20), num(&solid.1, 12), num(&solid.1, 22)), (0.0, 0.0, 0.0, 300.0), "a 300 mm column centred on (150, 150)");
    let beam = on(text, "S-BEAM");
    assert_eq!((beam.len(), num(&beam[0].1, 10), num(&beam[0].1, 11)), (1, 0.0, 4000.0));
    assert_eq!(on(text, "A-ROOM").len(), 1);
    let names = texts(text, "A-TEXT");
    assert!(names.contains(&"\\U+063A\\U+0631\\U+0641\\U+0629".to_string()), "{names:?}");
    assert!(names.contains(&"20.0 m2".to_string()));
    // overall dimensions: 4.00 m by 5.00 m, the title and the preliminary note
    let dims = texts(text, "A-DIMS");
    for want in ["4.00 m", "5.00 m", "Test villa", "PRELIMINARY - NOT FOR CONSTRUCTION"] {
        assert!(dims.contains(&want.to_string()), "{want} in {dims:?}");
    }
    assert_eq!(out.counts["A-WALL-EXT"], 3);
    assert_eq!(dxf_text("a\\b é"), "a\\b \\U+00E9");
}

#[test]
fn levels_are_laid_side_by_side_and_an_empty_request_is_refused() {
    let mut doc = Document::default();
    let l0 = element_of(&Level { name: "Ground floor".into(), index: 0, extra: Default::default() }, Source::User);
    let l1 = element_of(&Level { name: "Floor 1".into(), index: 1, extra: Default::default() }, Source::User);
    let (i0, i1) = (l0.id, l1.id);
    let room = |lvl: Id, key: &str| {
        let s = Space { func: "bedroom".into(), requirement_key: key.into(), area: 20.0, target_area: 20.0, x: 0, y: 0, w: 4000, h: 5000, boundary: vec![[0, 0], [4000, 0], [4000, 5000], [0, 5000]], net_area: None, glazing_ratio: None, extra: Default::default() };
        let mut el = element_of(&s, Source::User);
        el.level_id = Some(lvl);
        Command::Create(el)
    };
    doc.commit("two levels", Origin::User, vec![Command::Create(l0), Command::Create(l1), room(i0, "a"), room(i1, "b")]).unwrap();
    let both = dxf::export_dxf(&doc.model, &[Some(i0), Some(i1)], &opts()).unwrap().text;
    // 4000 wide + 6000 gap: the second sheet starts at x = 10000
    let xs: Vec<f64> = on(&both, "A-ROOM").iter().flat_map(|e| e.1.iter().filter(|p| p.0 == 10).map(|p| p.1.parse::<f64>().unwrap()).collect::<Vec<_>>()).collect();
    assert!(xs.contains(&0.0) && xs.contains(&10000.0) && xs.contains(&14000.0), "{xs:?}");
    let titles = texts(&both, "A-DIMS");
    assert!(titles.contains(&"Test villa - Ground floor".to_string()) && titles.contains(&"Test villa - Floor 1".to_string()));
    // one level alone sits at the origin
    let one = dxf::export_dxf(&doc.model, &[Some(i1)], &opts()).unwrap().text;
    assert_eq!(on(&one, "A-ROOM").len(), 1);
    assert!(dxf::export_dxf(&Model::default(), &[None], &opts()).unwrap_err().contains("nothing to export"));
}

#[test]
fn an_element_with_a_phase_goes_on_its_phase_layer_and_one_without_stays_on_the_base_layer() {
    let mut g = golden();
    let set = |g: &mut Golden, kind: &str, interior: Option<bool>, phase: &str| {
        let id = g.doc.model.elements.values().find(|e| e.kind == kind && interior.is_none_or(|i| e.typed::<Wall>().map(|w| w.is_exterior != i).unwrap_or(false))).unwrap().id;
        g.doc.commit("phase", Origin::User, vec![Command::SetProp { id, key: "phase".into(), value: Some(serde_json::json!(phase)) }]).unwrap();
    };
    set(&mut g, "Wall", Some(false), "NEW"); // the exterior wall
    set(&mut g, "Column", None, "EXISTING");
    let out = dxf::export_dxf(&g.doc.model, &[None], &opts()).unwrap();
    let layers: Vec<String> = entities(&out.text).iter().map(|e| e.1.clone()).collect();
    assert!(layers.iter().any(|l| l == "A-WALL-EXT_NEW"), "the exterior wall is on its phase layer");
    assert!(!layers.iter().any(|l| l == "A-WALL-EXT"), "none of it is left on the base layer");
    assert!(layers.iter().any(|l| l == "A-DOOR_NEW") && layers.iter().any(|l| l == "A-GLAZ_NEW"), "its door and window follow the wall");
    assert!(layers.iter().any(|l| l == "S-COLS_EXISTING"));
    assert!(layers.iter().any(|l| l == "A-WALL-INT"), "the partition has no phase and stays on the base layer: {layers:?}");
    assert!(!layers.iter().any(|l| l.ends_with("_UNSET")), "no phase is written for an element without one");
    // Each phase layer is declared in the layer table with its base layer's colour and line type.
    let p = pairs(&out.text);
    let table = |name: &str| -> Option<(String, String)> {
        let i = p.iter().position(|(c, v)| *c == 2 && v == name)?;
        let colour = p[i..].iter().find(|(c, _)| *c == 62)?.1.clone();
        let ltype = p[i..].iter().find(|(c, _)| *c == 6)?.1.clone();
        Some((colour, ltype))
    };
    assert_eq!(table("A-WALL-EXT_NEW"), table("A-WALL-EXT"));
    assert_eq!(table("S-COLS_EXISTING"), table("S-COLS"));
    assert!(table("S-COLS_EXISTING").is_some());
    // The layer count in the table matches the layers written.
    let table_layers = p.iter().zip(p.iter().skip(1)).filter(|((c, v), _)| *c == 0 && v == "LAYER").count();
    let declared: usize = p.iter().skip_while(|(c, v)| !(*c == 2 && v == "LAYER")).nth(1).unwrap().1.parse().unwrap();
    assert_eq!(declared, table_layers);
}

#[test]
fn named_features_are_drawn_on_the_feature_layer_with_their_label_and_phase() {
    use nnx_core::typed::Feature;
    let mut g = golden();
    let feat = |role: &str, shape: &str, x1, y1, x2, y2, width, name: &str| Feature { role: role.into(), shape: shape.into(), x1, y1, x2, y2, width, name: Some(name.into()), note: None, room: None, risers: None, riser_mm: None, extra: Default::default() };
    let mut joint = element_of(&feat("EXPANSION_JOINT", "line", 2000, 0, 2000, 5000, 50, "EJ"), Source::User);
    joint.props.insert("phase".into(), serde_json::json!("NEW"));
    let shaft = element_of(&feat("SHAFT", "rect", 500, 500, 1050, 1050, 0, "SH1"), Source::User);
    let drain = element_of(&feat("FLOOR_DRAIN", "point", 3000, 3000, 0, 0, 0, "FD"), Source::User);
    g.doc.commit("features", Origin::User, vec![Command::Create(joint), Command::Create(shaft), Command::Create(drain)]).unwrap();
    let out = dxf::export_dxf(&g.doc.model, &[None], &opts()).unwrap();
    let ents = entities(&out.text);
    let on = |layer: &str, kind: &str| ents.iter().filter(|e| e.1 == layer && e.0 == kind).count();
    // The joint: a 50 mm band (closed polyline) and its centre line, on the NEW layer; the shaft: a rectangle and two diagonals; the drain: a circle.
    assert_eq!((on("A-FEAT_NEW", "POLYLINE"), on("A-FEAT_NEW", "LINE")), (1, 1));
    assert_eq!((on("A-FEAT", "POLYLINE"), on("A-FEAT", "LINE"), on("A-FEAT", "CIRCLE")), (1, 2, 1));
    assert_eq!(on("A-FEAT_NEW", "TEXT") + on("A-FEAT", "TEXT"), 3, "each carries its label");
    assert!(out.text.contains("\nEJ\n") && out.text.contains("\nSH1\n") && out.text.contains("\nFD\n"));
}
