//! A hand sketch read into rooms: the scale and origin are the user's, every room is unconfirmed, and only confirmed rooms pass.

use nnx_core::sketchread::{self, Edit, Frame, Scale};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// A 1000 x 800 px image where 100 px are 2000 mm (20 mm per px); the buildable area's south-west corner is at pixel (100, 700).
fn frame(tol: Option<f64>) -> Frame {
    Frame { width_px: 1000, height_px: 800, scale: Scale::from_reference((0.0, 0.0), (100.0, 0.0), 2000.0).unwrap(), origin_px: (100.0, 700.0), tolerance_pct: tol }
}

fn room(label: &str, func: &str, bbox: [f64; 4]) -> Value {
    json!({ "label": label, "func": func, "bbox_px": bbox })
}

fn read(rooms: Vec<Value>, tol: Option<f64>) -> Result<sketchread::Reading, String> {
    sketchread::from_model_json(&json!({ "rooms": rooms }), "test-vl", &frame(tol))
}

#[test]
fn pixels_become_millimetres_only_through_the_users_scale_and_origin() {
    // 250 x 200 px = 5000 x 4000 mm, whose south-west corner is (150, 700): 1000 mm east and 0 north of the origin.
    let r = read(vec![room("مجلس", "majlis", [150.0, 500.0, 400.0, 700.0])], None).unwrap();
    let m = &r.rooms[0];
    assert_eq!((m.x_mm, m.y_mm, m.w_mm, m.h_mm), (1000, 0, 5000, 4000));
    assert_eq!(m.area_m2, 20.0);
    assert_eq!(m.status, "unconfirmed");
    assert!(r.needs_review && r.source == "ollama:test-vl");
    assert_eq!(m.name.as_deref(), Some("مجلس"));
    // A room north of the origin gets a larger y (y is up): its south edge is pixel 400, 300 px above the origin row 700.
    let r = read(vec![room("a", "bedroom", [100.0, 300.0, 300.0, 400.0])], None).unwrap();
    assert_eq!((r.rooms[0].x_mm, r.rooms[0].y_mm), (0, 6000));
}

#[test]
fn a_scale_reference_that_is_not_usable_is_refused() {
    assert!(Scale::from_reference((0.0, 0.0), (0.0, 0.0), 1000.0).unwrap_err().contains("same point"));
    assert!(Scale::from_reference((0.0, 0.0), (5.0, 0.0), 0.0).unwrap_err().contains("above zero"));
    assert!(Scale::from_reference((0.0, f64::NAN), (5.0, 0.0), 10.0).unwrap_err().contains("not a number"));
    assert_eq!(Scale::from_reference((0.0, 0.0), (30.0, 40.0), 5000.0).unwrap().mm_per_px, 100.0);
}

#[test]
fn bad_boxes_are_dropped_and_named_never_repaired() {
    let r = read(
        vec![
            room("ok", "kitchen", [150.0, 500.0, 300.0, 600.0]),
            room("outside", "kitchen", [900.0, 500.0, 1100.0, 600.0]),
            room("flat", "kitchen", [300.0, 500.0, 300.0, 600.0]),
            room("tiny", "kitchen", [300.0, 500.0, 310.0, 600.0]),
            json!({ "label": "nan", "func": "kitchen", "bbox_px": [1, 2, 3] }),
            json!({ "label": "str", "func": "kitchen", "bbox_px": ["a", 2, 3, 4] }),
        ],
        None,
    )
    .unwrap();
    assert_eq!(r.rooms.len(), 1);
    let notes = r.notes.join("\n");
    for expect in ["room 2: dropped, its box lies outside", "room 3: dropped, its box has no area", "room 4: dropped, it measures", "room 5: dropped, its box is not four numbers", "room 6: dropped, its box has a value that is not a number"] {
        assert!(notes.contains(expect), "{expect}\n{notes}");
    }
    let err = read(vec![room("outside", "kitchen", [900.0, 500.0, 1100.0, 600.0])], None).unwrap_err();
    assert!(err.contains("no room could be read") && err.contains("outside"), "{err}");
    assert!(sketchread::from_model_json(&json!({}), "m", &frame(None)).unwrap_err().contains("no 'rooms'"));
    assert!(sketchread::from_model_json(&json!([]), "m", &frame(None)).unwrap_err().contains("not a JSON object"));
}

#[test]
fn an_unknown_function_becomes_other_and_is_reported() {
    let r = read(vec![room("x", "garage", [150.0, 500.0, 300.0, 600.0]), json!({ "label": null, "func": null, "bbox_px": [400.0, 500.0, 550.0, 600.0] })], None).unwrap();
    assert_eq!((r.rooms[0].func.as_str(), r.rooms[1].func.as_str()), ("other", "other"));
    assert!(r.rooms[1].name.is_none());
    let n = r.notes.join("\n");
    assert!(n.contains("'garage' is not one the planner knows") && n.contains("room 2: no function read"), "{n}");
}

#[test]
fn written_dimensions_are_compared_only_with_a_tolerance() {
    let boxed = |w: f64, l: f64| json!({ "label": "r", "func": "bedroom", "bbox_px": [150.0, 500.0, 400.0, 700.0], "written_width_mm": w, "written_length_mm": l });
    // The box measures 5000 x 4000 mm.
    assert_eq!(read(vec![boxed(5000.0, 4000.0)], None).unwrap().rooms[0].dimension_check, "unchecked");
    assert!(read(vec![boxed(5000.0, 4000.0)], None).unwrap().notes.join("\n").contains("no tolerance was given"));
    assert_eq!(read(vec![boxed(5100.0, 3900.0)], Some(5.0)).unwrap().rooms[0].dimension_check, "ok");
    assert_eq!(read(vec![boxed(4000.0, 5000.0)], Some(1.0)).unwrap().rooms[0].dimension_check, "ok", "order of the pair does not matter");
    let bad = read(vec![boxed(7000.0, 4000.0)], Some(5.0)).unwrap();
    assert_eq!(bad.rooms[0].dimension_check, "conflict");
    assert!(bad.rooms[0].issues[0].contains("differs"), "{:?}", bad.rooms[0].issues);
    assert_eq!(read(vec![room("r", "bedroom", [150.0, 500.0, 400.0, 700.0])], Some(5.0)).unwrap().rooms[0].dimension_check, "no_written_dimension");
    assert!(sketchread::from_model_json(&json!({ "rooms": [] }), "m", &Frame { tolerance_pct: Some(-1.0), ..frame(None) }).unwrap_err().contains("tolerance"));
}

#[test]
fn overlapping_boxes_are_both_flagged_not_merged() {
    let r = read(vec![room("a", "bedroom", [150.0, 500.0, 400.0, 700.0]), room("b", "bedroom", [300.0, 500.0, 500.0, 700.0]), room("c", "bedroom", [600.0, 500.0, 800.0, 700.0])], None).unwrap();
    assert_eq!(r.rooms[0].overlaps, ["s2"]);
    assert_eq!(r.rooms[1].overlaps, ["s1"]);
    assert!(r.rooms[2].overlaps.is_empty() && r.rooms[2].issues.is_empty());
    assert_eq!(r.rooms.len(), 3);
}

#[test]
fn only_confirmed_rooms_pass_and_they_become_brief_rooms_and_a_placement() {
    let r = read(vec![room("مجلس", "majlis", [150.0, 500.0, 400.0, 700.0]), room("مطبخ", "kitchen", [400.0, 500.0, 600.0, 700.0]), room("حمام", "bathroom", [650.0, 500.0, 750.0, 700.0])], None).unwrap();
    let a = sketchread::accept(&r, &["s1".into(), "s3".into()], 1, &BTreeMap::new()).unwrap();
    assert_eq!(a.keys, ["s1", "s3"]);
    assert_eq!(a.rooms.len(), 2);
    assert_eq!((a.rooms[0].func.as_str(), a.rooms[0].area_m2, a.rooms[0].floor, a.rooms[0].assumed), ("majlis", Some(20.0), 1, false));
    assert_eq!(a.placement["s1"], json!({ "x": 1000, "y": 0, "w": 5000, "h": 4000 }));
    assert!(a.placement.get("s2").is_none(), "the unconfirmed room is not placed");
    assert!(a.notes[0].contains("2 room(s) confirmed"));
}

#[test]
fn acceptance_refuses_unknown_repeated_and_flawed_rooms_by_name() {
    let r = read(vec![room("a", "bedroom", [150.0, 500.0, 400.0, 700.0]), room("b", "bedroom", [300.0, 500.0, 500.0, 700.0])], None).unwrap();
    let none = BTreeMap::new();
    assert!(sketchread::accept(&r, &[], 1, &none).unwrap_err().contains("at least one"));
    assert!(sketchread::accept(&r, &["s1".into()], 0, &none).unwrap_err().contains("floor"));
    assert!(sketchread::accept(&r, &["s9".into()], 1, &none).unwrap_err().contains("'s9' is not in the reading"));
    assert!(sketchread::accept(&r, &["s1".into(), "s1".into()], 1, &none).unwrap_err().contains("twice"));
    // s1 overlaps s2: confirming both is refused; confirming only s1 is allowed (s2 is not placed).
    assert!(sketchread::accept(&r, &["s1".into(), "s2".into()], 1, &none).unwrap_err().contains("overlap"));
    assert!(sketchread::accept(&r, &["s1".into()], 1, &none).is_ok());
}

#[test]
fn a_conflicting_dimension_blocks_acceptance_until_the_box_is_corrected() {
    let item = json!({ "label": "r", "func": "bedroom", "bbox_px": [150.0, 500.0, 400.0, 700.0], "written_width_mm": 9000.0, "written_length_mm": 4000.0 });
    let r = read(vec![item], Some(5.0)).unwrap();
    let none = BTreeMap::new();
    assert!(sketchread::accept(&r, &["s1".into()], 1, &none).unwrap_err().contains("Correct its box"));
    let mut edits = BTreeMap::new();
    edits.insert("s1".to_string(), Edit { w_mm: Some(9000), name: Some("  غرفة النوم ".into()), func: Some("bedroom".into()), ..Default::default() });
    let a = sketchread::accept(&r, &["s1".into()], 2, &edits).unwrap();
    assert_eq!(a.rooms[0].area_m2, Some(36.0));
    assert_eq!(a.rooms[0].name.as_deref(), Some("غرفة النوم"));
    assert_eq!(a.rooms[0].floor, 2);
}

#[test]
fn a_bad_correction_is_refused() {
    let r = read(vec![room("a", "bedroom", [150.0, 500.0, 400.0, 700.0])], None).unwrap();
    let with = |e: Edit| BTreeMap::from([("s1".to_string(), e)]);
    assert!(sketchread::accept(&r, &["s1".into()], 1, &with(Edit { func: Some("garage".into()), ..Default::default() })).unwrap_err().contains("not one the planner knows"));
    assert!(sketchread::accept(&r, &["s1".into()], 1, &with(Edit { w_mm: Some(300), ..Default::default() })).unwrap_err().contains("under 0.5 m"));
    assert!(sketchread::accept(&r, &["s1".into()], 1, &with(Edit { x_mm: Some(-10), ..Default::default() })).unwrap_err().contains("west or south"));
}

#[test]
fn image_sizes_come_from_the_header() {
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
    png.extend_from_slice(&640u32.to_be_bytes());
    png.extend_from_slice(&480u32.to_be_bytes());
    assert_eq!(sketchread::image_size(&png).unwrap(), (640, 480));
    // JPEG: SOI, an APP0 segment of length 4, then SOF0 (height 300, width 400).
    let jpg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0, 0, 0xFF, 0xC0, 0x00, 0x11, 8, 0x01, 0x2C, 0x01, 0x90, 3, 0, 0];
    assert_eq!(sketchread::image_size(&jpg).unwrap(), (400, 300));
    let mut bmp = vec![b'B', b'M'];
    bmp.resize(18, 0);
    bmp.extend_from_slice(&50i32.to_le_bytes());
    bmp.extend_from_slice(&(-30i32).to_le_bytes());
    assert_eq!(sketchread::image_size(&bmp).unwrap(), (50, 30));
    assert!(sketchread::image_size(b"%PDF-1.7 ....").unwrap_err().contains("render the page"));
    assert!(sketchread::image_size(b"GIF89a").unwrap_err().contains("not recognised"));
    assert!(sketchread::image_size(&png[..20]).unwrap_err().contains("no header"));
}

#[test]
fn the_model_is_told_the_image_size_and_the_schema_demands_a_box() {
    let p = sketchread::system_prompt(1000, 800);
    assert!(p.contains("1000 x 800 pixels") && p.contains("Never estimate real sizes"));
    assert_eq!(sketchread::schema()["properties"]["rooms"]["items"]["required"], json!(["label", "func", "bbox_px"]));
}
