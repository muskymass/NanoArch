//! IFC export. The golden model is small and SYNTHETIC; every value is checked by hand against the written file.

use nnx_core::ifc::{self, IfcOpts, ifc_guid, step_str};
use nnx_core::typed::{Beam, Column, Door, Level, Slab, Space, Wall, Window, element_of};
use nnx_core::*;
use std::collections::BTreeSet;

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

fn opts() -> IfcOpts {
    let mut labels = std::collections::BTreeMap::new();
    labels.insert("r1".to_string(), "غرفة النوم".to_string());
    IfcOpts { project_name: "Ali's \\ villa".into(), author: "tester".into(), timestamp: "2026-11-17T10:00:00".into(), labels }
}

fn export(doc: &Document) -> String {
    ifc::export_ifc(&doc.model, &opts()).unwrap().text
}

/// Entity bodies by id, parsed back from the written file.
fn entities(text: &str) -> Vec<(usize, String)> {
    let data = text.split("DATA;\n").nth(1).unwrap().split("ENDSEC;").next().unwrap();
    data.lines()
        .filter(|l| l.starts_with('#'))
        .map(|l| {
            let (id, body) = l.split_once('=').unwrap();
            (id[1..].parse().unwrap(), body.trim_end_matches(';').to_string())
        })
        .collect()
}

fn count(text: &str, class: &str) -> usize {
    entities(text).iter().filter(|(_, b)| b.starts_with(&format!("{class}("))).count()
}

#[test]
fn the_file_is_well_formed_step_with_every_reference_defined() {
    let g = golden();
    let text = export(&g.doc);
    assert!(text.starts_with("ISO-10303-21;\nHEADER;"));
    assert!(text.contains("FILE_SCHEMA(('IFC4'));") && text.trim_end().ends_with("END-ISO-10303-21;"));
    assert!(text.is_ascii(), "everything is escaped to ASCII");
    let ents = entities(&text);
    let ids: Vec<usize> = ents.iter().map(|e| e.0).collect();
    assert_eq!(ids, (1..=ids.len()).collect::<Vec<_>>(), "ids are 1..n with no gaps");
    let defined: BTreeSet<usize> = ids.iter().copied().collect();
    for (id, body) in &ents {
        // references outside string literals only
        let plain: String = body.split('\'').enumerate().filter(|(i, _)| i % 2 == 0).map(|(_, s)| s).collect();
        let mut rest = plain.as_str();
        while let Some(i) = rest.find('#') {
            rest = &rest[i + 1..];
            let n: usize = rest.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap();
            assert!(defined.contains(&n), "#{id} refers to missing #{n}: {body}");
        }
        assert_eq!(plain.matches('(').count(), plain.matches(')').count(), "unbalanced: {body}");
    }
}

#[test]
fn products_openings_and_relations_match_the_model() {
    let g = golden();
    let text = export(&g.doc);
    for (class, n) in [("IFCPROJECT", 1), ("IFCSITE", 1), ("IFCBUILDING", 1), ("IFCBUILDINGSTOREY", 1), ("IFCWALL", 2), ("IFCDOOR", 1), ("IFCWINDOW", 1), ("IFCOPENINGELEMENT", 2), ("IFCRELVOIDSELEMENT", 2), ("IFCRELFILLSELEMENT", 2), ("IFCSLAB", 1), ("IFCCOLUMN", 1), ("IFCBEAM", 1), ("IFCSPACE", 1)] {
        assert_eq!(count(&text, class), n, "{class}");
    }
    let out = ifc::export_ifc(&g.doc.model, &opts()).unwrap();
    assert_eq!((out.counts["IfcWall"], out.counts["IfcDoor"], out.counts["IfcSpace"]), (2, 1, 1));
    // one storey at elevation 0 holding the two walls, slab, column, beam, door and window: 7 products
    let contain = entities(&text).into_iter().find(|(_, b)| b.starts_with("IFCRELCONTAINEDINSPATIALSTRUCTURE(")).unwrap().1;
    assert_eq!(contain.split("),#").next().unwrap().matches('#').count(), 7, "{contain}");
    assert!(text.contains(",.ELEMENT.,0.);"), "storey elevation");
}

#[test]
fn guids_are_valid_unique_stable_and_derived_from_the_element_ids() {
    assert_eq!(ifc_guid([0; 16]), "0000000000000000000000");
    assert_eq!(ifc_guid([0xff; 16]), format!("3{}", "$".repeat(21)));
    let g = golden();
    let (a, b) = (export(&g.doc), export(&g.doc));
    assert_eq!(a, b, "the same model exports byte for byte the same file");
    let mut seen = BTreeSet::new();
    for (_, body) in entities(&a) {
        let Some((class, rest)) = body.split_once("('") else { continue };
        let guid = rest.split('\'').next().unwrap();
        let rooted = ["IFCPROJECT", "IFCSITE", "IFCBUILDING", "IFCBUILDINGSTOREY", "IFCWALL", "IFCDOOR", "IFCWINDOW", "IFCSLAB", "IFCCOLUMN", "IFCBEAM", "IFCSPACE", "IFCOPENINGELEMENT", "IFCREL", "IFCPROPERTYSET", "IFCELEMENTQUANTITY"];
        if rooted.iter().any(|r| class.starts_with(r)) {
            assert_eq!(guid.len(), 22, "{body}");
            assert!(guid.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$'), "{guid}");
            assert!(seen.insert(guid.to_string()), "duplicate GUID {guid}");
        }
    }
    assert!(seen.len() > 20, "{}", seen.len());
    // an element's GUID is its model id compressed
    let door = g.doc.model.elements.values().find(|e| e.kind == "Door").unwrap();
    let bytes = *uuid::Uuid::parse_str(&door.id.to_string()).unwrap().as_bytes();
    assert!(a.contains(&format!("IFCDOOR('{}'", ifc_guid(bytes))));
}

#[test]
fn geometry_is_the_models_in_millimetres() {
    let g = golden();
    let text = export(&g.doc);
    // the exterior wall: a 4000 x 200 profile extruded 3000 high, its lower corner at (0, -100) (centre line y = 0)
    assert!(text.contains(",4000.,200.)"));
    assert!(text.contains("IFCCARTESIANPOINT((0.,-100.,0.))"));
    assert!(text.contains(",3000.)"), "extrusion depth of the wall");
    // the door opening: 900 wide, centred at 1000 -> starts at 550, 2100 high, through the 200 mm wall
    assert!(text.contains("IFCCARTESIANPOINT((550.,-100.,0.))") && text.contains(",900.,200.)") && text.contains(",2100.)"));
    // the window opening: 1500 wide centred at 3000 -> starts at 2250, sill 900, 1200 high
    assert!(text.contains("IFCCARTESIANPOINT((2250.,-100.,900.))") && text.contains(",1500.,200.)") && text.contains(",1200.)"));
    // door and window sizes as attributes
    assert!(text.contains(",2100.,900.,.DOOR.,") && text.contains(",1200.,1500.,.WINDOW.,"));
    // slab on top of the walls: placed at z = 3000 relative to the storey; the only level, so a roof
    assert!(text.contains("IFCCARTESIANPOINT((0.,0.,3000.))") && text.contains(".ROOF."));
}

#[test]
fn names_are_escaped_and_properties_trace_back_to_the_model() {
    let g = golden();
    let text = export(&g.doc);
    assert!(text.contains("'Ali''s \\\\ villa'"), "apostrophe and backslash are escaped");
    // each Arabic letter is one UTF-16 unit of the \X2\ ... \X0\ escape
    assert_eq!(step_str("غ"), "'\\X2\\063A\\X0\\'");
    assert_eq!(step_str("a'b"), "'a''b'");
    assert!(text.contains("\\X2\\063A063106410629\\X0\\ \\X2\\0627064406460648"), "the room's long name is in the file, escaped");
    // the property set carries the element id, source and lock state
    let wall = g.doc.model.elements.values().find(|e| e.kind == "Wall").unwrap();
    assert!(text.contains(&format!("IFCIDENTIFIER('{}')", wall.id)));
    assert!(text.contains("'Pset_WallCommon'") && text.contains("'IsExternal'") && text.contains("'ModelId'"));
    assert!(text.contains("IFCBOOLEAN(.T.)") && text.contains("IFCLABEL('User')"));
    assert!(text.contains("IFCSPACE(") && text.contains(",'r1',"), "the room's key is its Name");
}

#[test]
fn levels_become_storeys_at_their_elevations_and_an_empty_model_is_refused() {
    let mut doc = Document::default();
    let l0 = element_of(&Level { name: "Ground floor".into(), index: 0, extra: Default::default() }, Source::User);
    let l1 = element_of(&Level { name: "Floor 1".into(), index: 1, extra: Default::default() }, Source::User);
    let (i0, i1) = (l0.id, l1.id);
    let wall = |lvl: Id| {
        let w = Wall { x1: 0, y1: 0, x2: 4000, y2: 0, thickness: 200, height: 3000, length: 4000, is_exterior: true, room_a: "r".into(), room_b: None, extra: Default::default() };
        let mut el = element_of(&w, Source::User);
        el.level_id = Some(lvl);
        Command::Create(el)
    };
    doc.commit("two levels", Origin::User, vec![Command::Create(l0), Command::Create(l1), wall(i0), wall(i1)]).unwrap();
    let text = export(&doc);
    assert_eq!(count(&text, "IFCBUILDINGSTOREY"), 2);
    // no slabs: the second storey starts one storey height (3000 mm) above the first
    assert!(text.contains(",.ELEMENT.,0.);") && text.contains(",.ELEMENT.,3000.);"), "storey elevations");
    assert_eq!(count(&text, "IFCWALL"), 2);
    let contained = entities(&text).into_iter().filter(|(_, b)| b.starts_with("IFCRELCONTAINEDINSPATIALSTRUCTURE(")).count();
    assert_eq!(contained, 2, "one containment per storey");
    assert!(ifc::export_ifc(&Model::default(), &opts()).unwrap_err().contains("nothing to export"));
}

#[test]
fn the_construction_phase_is_a_property_of_the_products_that_have_one() {
    let mut g = golden();
    let wall = g.doc.model.elements.values().find(|e| e.kind == "Wall" && e.typed::<Wall>().unwrap().is_exterior).unwrap().id;
    g.doc.commit("phase", Origin::User, vec![Command::SetProp { id: wall, key: "phase".into(), value: Some(serde_json::json!("TO_BE_DEMOLISHED")) }]).unwrap();
    let text = export(&g.doc);
    assert_eq!(text.matches("IFCPROPERTYSINGLEVALUE('Phase',$,IFCLABEL('TO_BE_DEMOLISHED'),$)").count(), 1, "one product has a phase");
    assert_eq!(text.matches("'Phase'").count(), 1, "the others get none: an unset phase is not written");
}
