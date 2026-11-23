//! IFC review. The round trip uses NanoArch's own export (synthetic model); hand-written files test the faults.

use nnx_core::ifc::{self, IfcOpts};
use nnx_core::ifc_review::{Severity, review_ifc};
use nnx_core::typed::{Door, Slab, Space, Wall, element_of};
use nnx_core::*;

fn exported() -> String {
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
    let wall = Wall { x1: 0, y1: 0, x2: 4000, y2: 0, thickness: 200, height: 3000, length: 4000, is_exterior: true, room_a: "r1".into(), room_b: None, extra: Default::default() };
    let wall_el = element_of(&wall, Source::User);
    let wid = wall_el.id;
    let door = Door { width: 900, height: 2100, offset_center: 1000, is_egress: true, from: "r1".into(), to: "outside".into(), opens_into: "r1".into(), extra: Default::default() };
    doc.commit(
        "t",
        Origin::User,
        vec![Command::Create(element_of(&space, Source::User)), Command::Create(element_of(&slab, Source::User)), Command::Create(wall_el), Command::Create(element_of(&door, Source::User).hosted_by(wid))],
    )
    .unwrap();
    let opts = IfcOpts { project_name: "Villa 'A'".into(), author: "t".into(), timestamp: "2026-11-23T10:00:00".into(), labels: Default::default() };
    ifc::export_ifc(&doc.model, &opts).unwrap().text
}

#[test]
fn our_own_export_reviews_clean_with_the_model_values() {
    let r = review_ifc(exported().as_bytes()).unwrap();
    assert_eq!(r.schema.as_deref(), Some("IFC4"));
    assert_eq!(r.project_name.as_deref(), Some("Villa 'A'"));
    assert_eq!(r.length_unit_m, Some(0.001));
    assert_eq!(r.area_unit_m2, Some(1.0));
    assert_eq!((r.counts["IFCWALL"], r.counts["IFCDOOR"], r.counts["IFCSPACE"]), (1, 1, 1));
    assert_eq!(r.storeys.len(), 1);
    assert_eq!(r.storeys[0].elevation_m, Some(0.0));
    assert_eq!(r.spaces.len(), 1);
    // The exporter writes NetFloorArea = space.area (20 m2); the quantity wins over the 4 x 5 m profile.
    assert_eq!(r.spaces[0].area_m2, Some(20.0));
    assert_eq!(r.spaces[0].area_source.as_deref(), Some("quantity NetFloorArea"));
    assert_eq!(r.spaces[0].storey, r.storeys[0].name);
    assert_eq!(r.spaces_without_area, 0);
    let errors: Vec<_> = r.findings.iter().filter(|f| f.severity == Severity::Error).collect();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_space_without_a_quantity_takes_its_area_from_the_profile_in_the_files_units() {
    let text = exported();
    // Drop the quantity relation and set: only the geometry remains (4000 x 5000 mm = 20 m2).
    let stripped: String = text.lines().filter(|l| !l.contains("IFCELEMENTQUANTITY") && !(l.contains("IFCRELDEFINESBYPROPERTIES") && l.contains("(#") && !l.contains("IFCPROPERTYSET"))).map(|l| format!("{l}\n")).collect();
    let r = review_ifc(stripped.as_bytes()).unwrap();
    let s = &r.spaces[0];
    assert_eq!(s.area_source.as_deref(), Some("rectangle profile"));
    assert_eq!(s.area_m2, Some(20.0));
}

fn file(data: &str) -> Vec<u8> {
    format!("ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC2X3'));\nENDSEC;\nDATA;\n{data}\nENDSEC;\nEND-ISO-10303-21;\n").into_bytes()
}

fn codes(r: &nnx_core::ifc_review::IfcReview) -> Vec<&'static str> {
    r.findings.iter().map(|f| f.code).collect()
}

#[test]
fn a_file_without_units_storeys_or_project_reports_each_gap_and_computes_nothing() {
    let r = review_ifc(&file("#1=IFCWALL('1111111111111111111111',$,'W',$,$,$,$,$);\n#2=IFCSPACE('2222222222222222222222',$,'S',$,$,$,#9,'Room',.ELEMENT.,.INTERNAL.,$);")).unwrap();
    let c = codes(&r);
    for want in ["no_project", "no_length_unit", "no_area_unit", "no_storeys", "dangling_reference", "space_without_area"] {
        assert!(c.contains(&want), "{want} in {c:?}");
    }
    assert_eq!(r.length_unit_m, None);
    assert_eq!(r.spaces[0].area_m2, None);
    assert_eq!(r.findings[0].severity, Severity::Error, "errors sort first");
}

#[test]
fn duplicate_guids_and_unfilled_doors_are_found() {
    let g = "'3333333333333333333333'";
    let r = review_ifc(&file(&format!("#1=IFCDOOR({g},$,'D1',$,$,$,$,$,$,$);\n#2=IFCWINDOW({g},$,'W1',$,$,$,$,$,$,$);"))).unwrap();
    let dup = r.findings.iter().find(|f| f.code == "duplicate_guid").unwrap();
    assert_eq!((dup.count, dup.examples.clone()), (2, vec!["#1".to_string(), "#2".to_string()]));
    let un = r.findings.iter().find(|f| f.code == "door_window_without_opening").unwrap();
    assert_eq!(un.count, 2);
}

#[test]
fn strings_escapes_and_a_polyline_profile_are_decoded() {
    let data = "#1=IFCPROJECT('4444444444444444444444',$,'\\X2\\0645\\X0\\\\X2\\0634\\X0\\ it''s',$,$,$,$,(#3),#5);\n\
        #3=IFCGEOMETRICREPRESENTATIONCONTEXT($,'Model',3,1.E-05,#4,$);\n#4=IFCAXIS2PLACEMENT3D(#6,$,$);\n#6=IFCCARTESIANPOINT((0.,0.,0.));\n\
        #5=IFCUNITASSIGNMENT((#7,#8));\n#7=IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.);\n#8=IFCSIUNIT(*,.AREAUNIT.,$,.SQUARE_METRE.);\n\
        #10=IFCCARTESIANPOINT((0.,0.));\n#11=IFCCARTESIANPOINT((3.,0.));\n#12=IFCCARTESIANPOINT((3.,2.));\n#13=IFCCARTESIANPOINT((0.,2.));\n\
        #14=IFCPOLYLINE((#10,#11,#12,#13,#10));\n#15=IFCARBITRARYCLOSEDPROFILEDEF(.AREA.,$,#14);\n\
        #16=IFCEXTRUDEDAREASOLID(#15,#4,#17,2.7);\n#17=IFCDIRECTION((0.,0.,1.));\n#18=IFCSHAPEREPRESENTATION(#3,'Body','SweptSolid',(#16));\n#19=IFCPRODUCTDEFINITIONSHAPE($,$,(#18));\n\
        #20=IFCSPACE('5555555555555555555555',$,'R1',$,$,$,#19,$,.ELEMENT.,.INTERNAL.,$);";
    let r = review_ifc(&file(data)).unwrap();
    assert_eq!(r.project_name.as_deref(), Some("مش it's"));
    assert_eq!(r.length_unit_m, Some(1.0));
    assert_eq!(r.spaces[0].area_m2, Some(6.0));
    assert_eq!(r.spaces[0].area_source.as_deref(), Some("closed polyline profile"));
}

#[test]
fn a_prefixed_area_unit_is_refused_not_guessed() {
    let data = "#1=IFCPROJECT('6666666666666666666666',$,'P',$,$,$,$,$,#2);\n#2=IFCUNITASSIGNMENT((#3,#4));\n#3=IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.);\n#4=IFCSIUNIT(*,.AREAUNIT.,.MILLI.,.SQUARE_METRE.);";
    let r = review_ifc(&file(data)).unwrap();
    assert_eq!((r.length_unit_m, r.area_unit_m2), (Some(0.001), None));
    assert!(codes(&r).contains(&"no_area_unit"));
}

#[test]
fn revit_zip_xml_and_non_ifc_input_are_refused_with_the_reason() {
    let revit = [&[0xD0u8, 0xCF, 0x11, 0xE0][..], &[0u8; 64]].concat();
    let e = review_ifc(&revit).unwrap_err();
    assert!(e.contains("Revit") && e.contains("Export > IFC"), "{e}");
    assert!(review_ifc(b"PK\x03\x04rest").unwrap_err().contains("ifcZIP"));
    assert!(review_ifc(b"<?xml version=\"1.0\"?><ifcXML/>").unwrap_err().contains("ifcXML"));
    assert!(review_ifc(b"hello").unwrap_err().contains("ISO-10303-21"));
    assert!(review_ifc(&file("")).unwrap_err().contains("no readable entities"));
}
