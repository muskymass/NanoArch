//! A room made of several rectangles is checked once, as a whole room. SYNTHETIC values.

use nnx_core::typed::{Space, Window, Wall, element_of};
use nnx_core::*;

fn space(key: &str, r: [i64; 4], part_of: Option<&str>, func: &str) -> Command {
    let [x, y, w, h] = r;
    let area = w as f64 * h as f64 / 1e6;
    let mut extra = serde_json::Map::new();
    if let Some(of) = part_of {
        extra.insert("partOf".into(), serde_json::json!(of));
    }
    let s = Space { func: func.into(), requirement_key: key.into(), area, target_area: area, x, y, w, h, boundary: vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]], net_area: None, glazing_ratio: None, extra };
    Command::Create(element_of(&s, Source::User))
}

/// A bedroom of the given rectangles with one window of `window` mm x 1500 mm (0: none) belonging to the room.
fn bedroom(rects: &[[i64; 4]], window: i64) -> Document {
    let mut doc = Document::default();
    let mut cmds: Vec<Command> = rects.iter().enumerate().map(|(i, r)| if i == 0 { space("R1", *r, None, "bedroom") } else { space(&format!("R1-{}", i + 1), *r, Some("R1"), "bedroom") }).collect();
    if window > 0 {
        let wall = element_of(&Wall { x1: 0, y1: 0, x2: 9000, y2: 0, thickness: 200, height: 3000, length: 9000, is_exterior: true, room_a: "R1".into(), room_b: None, extra: Default::default() }, Source::User);
        let win = Window { width: window, height: 1500, sill_height: 500, offset_center: 3000, room: "R1".into(), extra: Default::default() };
        let el = element_of(&win, Source::User).hosted_by(wall.id);
        cmds.push(Command::Create(wall));
        cmds.push(Command::Create(el));
    }
    cmds.push(Command::Create(element_of(&nnx_core::project::NanoArchProject::default(), Source::User)));
    doc.commit("room", Origin::User, cmds).unwrap();
    doc
}

fn rules_of(doc: &Document) -> nnx_core::rules::Report {
    let (set, _) = nnx_core::overlay::rule_set(Some(&nnx_core::sbc::sourced_pack_json()), &[]).unwrap();
    nnx_core::rules::check(&doc.model, &set)
}

#[test]
fn a_room_in_parts_has_one_area_and_one_daylight_ratio() {
    // 6 x 2.5 m plus 3 x 3 m = 24 m2; a 2000 x 1500 window is 3 m2 = 12.5%.
    let doc = bedroom(&[[0, 0, 6000, 2500], [0, 2500, 3000, 3000]], 2000);
    let r = rules_of(&doc);
    assert!(r.findings.is_empty(), "{:?}", r.findings);
    assert!(r.skipped.iter().all(|s| s.rule_id != "sbc.room_width.bedroom"), "every rectangle is wide enough: {:?}", r.skipped);
    let s = nnx_core::sbc::check(&doc.model);
    assert!(s.violations.iter().all(|v| !v.rule.starts_with("daylight") && !v.rule.starts_with("room_min")), "{:?}", s.violations);
}

#[test]
fn two_small_rectangles_make_one_room_that_is_large_enough() {
    // 2.2 x 2.2 m = 4.84 m2 each: below 6.5 m2 alone, 9.68 m2 together.
    let doc = bedroom(&[[0, 0, 2200, 2200], [2200, 0, 2200, 2200]], 1000);
    assert!(rules_of(&doc).findings.is_empty(), "{:?}", rules_of(&doc).findings);
}

#[test]
fn a_narrow_rectangle_of_a_room_in_parts_is_not_judged_neither_passed_nor_failed() {
    let doc = bedroom(&[[0, 0, 6000, 2000], [0, 2000, 3000, 3000]], 2000);
    let r = rules_of(&doc);
    assert!(r.findings.is_empty(), "{:?}", r.findings);
    let k = r.skipped.iter().find(|s| s.rule_id == "sbc.room_width.bedroom").expect("reported as not run");
    assert_eq!((k.param.as_str(), k.elements.len()), ("", 1));
    assert!(k.reason.contains("several rectangles"), "{}", k.reason);
    let s = nnx_core::sbc::check(&doc.model);
    assert!(s.violations.iter().all(|v| !v.rule.starts_with("room_min_width")), "{:?}", s.violations);
    let k = s.skipped.iter().find(|k| k.rule.starts_with("room_min_width_m.bedroom")).expect("reported as not checked");
    assert!(k.ar.contains("R1") && k.en.contains("several rectangles"), "{} {}", k.ar, k.en);
}

#[test]
fn a_room_of_one_rectangle_is_still_judged_and_no_window_is_no_glazing() {
    let doc = bedroom(&[[0, 0, 4000, 2000]], 0);
    let ids: Vec<String> = rules_of(&doc).findings.iter().map(|f| f.rule_id.clone()).collect();
    assert_eq!(ids, ["sbc.daylight.bedroom", "sbc.room_width.bedroom"]);
    assert!(rules_of(&doc).findings.iter().all(|f| !f.message.contains("could not be evaluated")));
    let s = nnx_core::sbc::check(&doc.model);
    let glazing = s.violations.iter().find(|v| v.rule.starts_with("daylight")).unwrap();
    assert!(glazing.en.contains("is 0.0%"), "{}", glazing.en);
}

#[test]
fn every_rule_of_the_sourced_pack_has_an_arabic_sentence_and_a_replaced_one_does_not() {
    let pack: serde_json::Value = serde_json::from_str(&nnx_core::sbc::sourced_pack_json()).unwrap();
    let rules = pack["rules"].as_array().unwrap();
    for r in rules {
        let (id, message) = (r["id"].as_str().unwrap(), r["message"].as_str().unwrap());
        let ar = nnx_core::sbc::rule_message_ar(id, message).unwrap_or_else(|| panic!("no Arabic for {id}"));
        assert!(!ar.chars().any(|c| c.is_ascii_alphabetic()), "{ar}");
        assert_eq!(nnx_core::sbc::rule_message_ar(id, "another sentence"), None);
    }
    assert_eq!(nnx_core::sbc::rule_message_ar("sbc.daylight.bedroom", "bedroom: glazing is below 0.08 of the floor area").unwrap(), "نوم: الزجاج أقل من 8% من مساحة الأرضية");
}

#[test]
fn the_smallest_area_is_compared_with_the_net_area() {
    // 3 x 2.4 m = 7.2 m2 between the wall axes, above 6.5; 6.16 m2 inside the walls, below it.
    let mut doc = bedroom(&[[0, 0, 3000, 2400]], 1000);
    let id = nnx_core::annot::whole_rooms(&doc.model)[0].element;
    assert!(rules_of(&doc).findings.iter().all(|f| f.rule_id != "sbc.room_area.bedroom"), "no walls: gross is net");
    doc.commit("net", Origin::User, vec![Command::SetProp { id, key: "netArea".into(), value: Some(serde_json::json!(6.16)) }]).unwrap();
    let f = rules_of(&doc).findings;
    assert!(f.iter().any(|f| f.rule_id == "sbc.room_area.bedroom" && f.message == "bedroom: net floor area is below the minimum of 6.5 m2"), "{f:?}");
    let s = nnx_core::sbc::check(&doc.model);
    let v = s.violations.iter().find(|v| v.rule == "room_min_area_m2.bedroom").expect("the built-in check agrees");
    assert!((v.value - 6.16).abs() < 1e-9 && v.ar.contains("المساحة الصافية 6.16"), "{} {}", v.value, v.ar);
}

#[test]
fn the_permit_pack_lists_a_room_in_parts_once_like_the_printout() {
    use nnx_core::permitpack::{Context, Engineer, Input, build};
    let doc = bedroom(&[[0, 0, 6000, 2500], [0, 2500, 3000, 3000]], 2000);
    let input = Input { engineer: Engineer { name: "E".into(), licence_no: "L".into(), licensing_body: "B".into(), discipline: String::new() }, project_no: "P".into(), sheet_prefix: "S".into(), revision: "0".into(), issued_for: String::new(), paper: String::new(), letterhead: Default::default() };
    let pack = build(&Context { model: &doc.model, title: "T", labels: Default::default(), arabic: false, show_structure: false, show_mep: false, date: "2026-11-27" }, &input).unwrap();
    let table = pack.html.split("<table class=\"rooms\">").last().unwrap().split("</table>").next().unwrap();
    assert!(table.contains("<td>R1</td><td>bedroom</td><td class=\"n\">24.0</td>"), "{table}");
    assert!(!table.contains("R1-2"), "{table}");
}
