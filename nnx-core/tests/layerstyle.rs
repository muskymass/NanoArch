//! Office layer standard applied to the DXF export. The model is small and SYNTHETIC.

use nnx_core::dxf::{self, DxfOpts};
use nnx_core::layerstyle::{self, LayerRule, LayerStyle};
use nnx_core::typed::{Column, Wall, element_of};
use nnx_core::*;

fn model() -> Document {
    let mut doc = Document::default();
    let wall = Wall { x1: 0, y1: 0, x2: 4000, y2: 0, thickness: 200, height: 3000, length: 4000, is_exterior: true, room_a: "r1".into(), room_b: None, extra: Default::default() };
    let column = Column { x: 150, y: 150, b: 300, h: 300, grid_ref: "A1".into(), axial_service_kn: None, extra: Default::default() };
    doc.commit("m", Origin::User, vec![Command::Create(element_of(&wall, Source::User)), Command::Create(element_of(&column, Source::User))]).unwrap();
    doc
}

fn export(doc: &Document) -> dxf::DxfOut {
    dxf::export_dxf(&doc.model, &[None], &DxfOpts { title: "T".into(), labels: Default::default(), mep: true }).unwrap()
}

fn rule(layer: &str, name: Option<&str>, color: Option<i32>, linetype: Option<&str>) -> LayerRule {
    LayerRule { layer: layer.into(), name: name.map(Into::into), color, linetype: linetype.map(Into::into) }
}

fn style(layers: Vec<LayerRule>) -> LayerStyle {
    LayerStyle { name: "Office standard".into(), source: "Office CAD manual, section 4".into(), layers }
}

/// (name, colour, line type) of every entry of the LAYER table.
fn layer_entries(text: &str) -> Vec<(String, String, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let pairs: Vec<(&str, &str)> = lines.chunks(2).map(|c| (c[0].trim(), c[1])).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < pairs.len() {
        if pairs[i] == ("0", "LAYER") {
            let get = |code: &str| pairs[i..i + 5].iter().find(|p| p.0 == code).unwrap().1.to_string();
            out.push((get("2"), get("62"), get("6")));
        }
        i += 1;
    }
    out
}

fn entity_layers(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.chunks(2).position(|c| c[0].trim() == "2" && c[1] == "ENTITIES").unwrap();
    lines.chunks(2).skip(start).filter(|c| c[0].trim() == "8").map(|c| c[1].to_string()).collect()
}

#[test]
fn rename_recolour_and_linetype_reach_the_table_the_entities_and_the_counts() {
    let doc = model();
    let before = export(&doc);
    let s = style(vec![rule("S-COLS", Some("STR-COLUMN"), Some(11), Some("dashed")), rule("A-WALL-EXT", None, Some(40), None)]);
    let r = layerstyle::apply(&before, &s).unwrap();

    let table = layer_entries(&r.out.text);
    assert!(table.contains(&("STR-COLUMN".into(), "11".into(), "DASHED".into())));
    assert!(table.contains(&("A-WALL-EXT".into(), "40".into(), "CONTINUOUS".into())), "colour only: name and line type stay");
    assert!(!table.iter().any(|t| t.0 == "S-COLS"));
    // untouched layers keep the engine's own values
    assert!(table.contains(&("A-DOOR".into(), "3".into(), "CONTINUOUS".into())));
    assert_eq!(table.len(), layer_entries(&before.text).len());

    let layers = entity_layers(&r.out.text);
    assert!(layers.contains(&"STR-COLUMN".to_string()) && !layers.contains(&"S-COLS".to_string()));
    assert_eq!(r.out.counts.get("STR-COLUMN"), before.counts.get("S-COLS"));
    assert!(!r.out.counts.contains_key("S-COLS"));
    assert_eq!(r.out.counts.get("A-WALL-EXT"), before.counts.get("A-WALL-EXT"));
    // same entities, same line count: only names, colours and line types moved
    assert_eq!(r.out.text.lines().count(), before.text.lines().count());
    let used: Vec<(&str, bool)> = r.applied.iter().map(|a| (a.layer.as_str(), a.used)).collect();
    assert_eq!(used, vec![("A-WALL-EXT", true), ("S-COLS", true)]);
}

#[test]
fn a_rule_for_a_layer_that_is_not_drawn_is_reported_unused() {
    let doc = model();
    let r = layerstyle::apply(&export(&doc), &style(vec![rule("M-DUCT", Some("MEC-DUCT"), None, None)])).unwrap();
    assert_eq!(r.applied.len(), 1);
    assert!(!r.applied[0].used);
    assert!(layer_entries(&r.out.text).iter().any(|t| t.0 == "MEC-DUCT"));
}

#[test]
fn phase_layers_follow_their_base_layer() {
    let mut doc = model();
    let col = doc.model.elements.values().find(|e| e.kind == "Column").map(|e| e.id).unwrap();
    doc.commit("phase", Origin::User, vec![Command::SetProp { id: col, key: "phase".into(), value: Some(serde_json::json!("NEW")) }]).unwrap();
    let before = export(&doc);
    assert!(before.counts.contains_key("S-COLS_NEW"));
    let r = layerstyle::apply(&before, &style(vec![rule("S-COLS", Some("STR-COLUMN"), Some(11), None)])).unwrap();
    let table = layer_entries(&r.out.text);
    assert!(table.contains(&("STR-COLUMN_NEW".into(), "11".into(), "CONTINUOUS".into())));
    assert!(entity_layers(&r.out.text).contains(&"STR-COLUMN_NEW".to_string()));
}

#[test]
fn a_bad_style_is_rejected_by_name_all_at_once() {
    let doc = model();
    let out = export(&doc);
    let bad = LayerStyle {
        name: " ".into(),
        source: " ".into(),
        layers: vec![
            rule("NOT-A-LAYER", Some("X"), None, None),
            rule("S-COLS", Some("bad:name"), Some(0), Some("DASHDOT")),
            rule("S-COLS", Some("Y"), None, None),
            rule("A-DOOR", None, None, None),
            rule("A-GLAZ", Some("A-WALL-INT"), None, None),
            rule("S-BEAM", Some(&"x".repeat(32)), None, None),
        ],
    };
    let e = layerstyle::apply(&out, &bad).unwrap_err();
    for needle in ["style name is empty", "source", "'NOT-A-LAYER' is not drawn", "'bad:name'", "colour 0", "'DASHDOT'", "listed twice", "changes nothing", "would all be named 'A-WALL-INT'", "longer than 31"] {
        assert!(e.contains(needle), "missing {needle:?} in {e}");
    }
    let empty = layerstyle::apply(&out, &style(vec![])).unwrap_err();
    assert!(empty.contains("no layer rules"));
}

#[test]
fn a_swap_of_two_names_is_allowed_because_final_names_stay_unique() {
    let doc = model();
    let s = style(vec![rule("A-WALL-EXT", Some("A-WALL-INT"), None, None), rule("A-WALL-INT", Some("A-WALL-EXT"), None, None)]);
    let r = layerstyle::apply(&export(&doc), &s).unwrap();
    let before = export(&doc);
    assert_eq!(r.out.counts.get("A-WALL-INT"), before.counts.get("A-WALL-EXT"));
}

#[test]
fn unknown_fields_in_the_json_are_refused() {
    let v = serde_json::json!({ "name": "n", "source": "s", "layers": [{ "layer": "S-COLS", "colour": 3 }] });
    assert!(serde_json::from_value::<LayerStyle>(v).is_err());
}
