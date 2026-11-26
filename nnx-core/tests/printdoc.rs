//! The printable plan set. SYNTHETIC test values, checked by hand against the document.

use nnx_core::printdoc::{PrintOpts, print_html};
use nnx_core::typed::{Door, Level, Space, Wall, element_of};
use nnx_core::*;

/// A room `w` x 5000 mm at the origin; its area is the rectangle's.
fn room(key: &str, w: i64, level: Option<Id>) -> Command {
    let area = w as f64 * 5000.0 / 1e6;
    let s = Space { func: "bedroom".into(), requirement_key: key.into(), area, target_area: area, x: 0, y: 0, w, h: 5000, boundary: vec![[0, 0], [w, 0], [w, 5000], [0, 5000]], net_area: None, glazing_ratio: None, extra: Default::default() };
    let mut el = element_of(&s, Source::User);
    el.level_id = level;
    Command::Create(el)
}

fn wall_with_door(level: Option<Id>) -> Vec<Command> {
    let w = Wall { x1: 0, y1: 0, x2: 4000, y2: 0, thickness: 200, height: 3000, length: 4000, is_exterior: true, room_a: "r1".into(), room_b: None, extra: Default::default() };
    let mut wel = element_of(&w, Source::User);
    wel.level_id = level;
    let id = wel.id;
    let d = Door { width: 900, height: 2100, offset_center: 1000, is_egress: true, from: "r1".into(), to: "outside".into(), opens_into: "r1".into(), extra: Default::default() };
    let mut del = element_of(&d, Source::User).hosted_by(id);
    del.level_id = level;
    vec![Command::Create(wel), Command::Create(del)]
}

fn opts(arabic: bool) -> PrintOpts {
    let mut labels = std::collections::BTreeMap::new();
    labels.insert("r1".to_string(), "غرفة <نوم>".to_string());
    PrintOpts { title: "Villa <A>".into(), labels, arabic, show_structure: false, show_mep: false, date: "2026-11-17".into() }
}

#[test]
fn one_level_prints_one_a3_sheet_with_the_plan_and_its_schedule() {
    let mut doc = Document::default();
    let mut cmds = vec![room("r1", 4000, None)];
    cmds.extend(wall_with_door(None));
    doc.commit("plan", Origin::User, cmds).unwrap();
    let html = print_html(&doc.model, &opts(false)).unwrap();
    assert!(html.contains("@page{size:A3 portrait;margin:12mm;") && html.contains("@bottom-center{content:counter(page) \" / \" counter(pages);direction:ltr;"), "A3 sheets with page numbers");
    assert_eq!(html.matches("<section class=\"sheet\">").count(), 1);
    assert_eq!(html.matches("<svg").count(), 1, "the same drawing as on screen");
    assert!(html.contains("dir=\"ltr\"") && html.contains("Room schedule") && html.contains("Sheet <bdi dir=\"ltr\">1/1</bdi>"));
    // the schedule: the room, its function and area, and the total as their sum
    assert!(html.contains("<td>bedroom</td><td class=\"n\">20.0</td>"));
    assert!(html.contains("<td class=\"n\">20.0</td></tr></table>"), "total");
    assert!(html.contains("Preliminary") && html.contains("<bdi dir=\"ltr\">2026-11-17</bdi>"), "the date keeps its order in right-to-left text");
    assert!(html.contains(".plan{direction:ltr}"), "the drawing's own text anchoring is not mirrored");
    // text from the user is escaped everywhere
    assert!(html.contains("Villa &lt;A&gt;") && !html.contains("Villa <A>"));
    assert!(html.contains("&lt;") && !html.contains("<نوم>"));
}

#[test]
fn arabic_headings_are_right_to_left_and_each_level_gets_its_own_sheet() {
    let mut doc = Document::default();
    let l0 = element_of(&Level { name: "Ground floor".into(), index: 0, extra: Default::default() }, Source::User);
    let l1 = element_of(&Level { name: "Floor 1".into(), index: 1, extra: Default::default() }, Source::User);
    let (i0, i1) = (l0.id, l1.id);
    let mut cmds = vec![Command::Create(l0), Command::Create(l1), room("r1", 4000, Some(i0)), room("r2", 3700, Some(i1))];
    cmds.extend(wall_with_door(Some(i0)));
    doc.commit("two levels", Origin::User, cmds).unwrap();
    let html = print_html(&doc.model, &opts(true)).unwrap();
    assert!(html.contains("dir=\"rtl\"") && html.contains("lang=\"ar\""));
    assert_eq!(html.matches("<section class=\"sheet\">").count(), 2);
    assert!(html.contains("الدور الأرضي") && html.contains("الدور 1"));
    assert!(html.contains("ورقة <bdi dir=\"ltr\">1/2</bdi>") && html.contains("ورقة <bdi dir=\"ltr\">2/2</bdi>"));
    // each sheet lists only its own level's rooms
    let second = html.split("<section class=\"sheet\">").nth(2).unwrap();
    assert!(second.contains("r2") && second.contains("18.5") && !second.contains("20.0"));
    assert!(html.contains("<td>نوم</td>"), "functions are translated");
}

#[test]
fn an_empty_model_is_refused() {
    assert!(print_html(&Model::default(), &opts(false)).unwrap_err().contains("nothing to print"));
}

#[test]
fn a_long_room_schedule_is_split_in_two_so_the_level_stays_on_one_sheet() {
    // 20 rooms of 4 x 5 m = 20.0 m2 each: above 16 rows the schedule becomes two tables of 10 rows, the total only in the last.
    let mut doc = Document::default();
    let cmds: Vec<Command> = (0..20).map(|i| room(&format!("r{i}"), 4000, None)).collect();
    doc.commit("many rooms", Origin::User, cmds).unwrap();
    let html = print_html(&doc.model, &opts(false)).unwrap();
    assert_eq!(html.matches("<section class=\"sheet\">").count(), 1);
    assert_eq!(html.matches("<table>").count(), 2);
    assert_eq!(html.matches("<td>bedroom</td>").count(), 20, "every room appears once");
    assert_eq!(html.matches("class=\"tot\"").count(), 1);
    assert!(html.contains("<td class=\"n\">400.0</td></tr></table>"), "the total is the sum of all 20 rooms, in the last table");
    // 16 rooms or fewer stay in a single table.
    let mut small = Document::default();
    small.commit("few", Origin::User, (0..16).map(|i| room(&format!("r{i}"), 4000, None)).collect()).unwrap();
    assert_eq!(print_html(&small.model, &opts(false)).unwrap().matches("<table>").count(), 1);
}

#[test]
fn a_room_in_parts_is_one_row_with_its_net_area_and_a_door_size_has_one_mark_on_every_sheet() {
    let mut doc = Document::default();
    let l0 = element_of(&Level { name: "G".into(), index: 0, extra: Default::default() }, Source::User);
    let l1 = element_of(&Level { name: "F".into(), index: 1, extra: Default::default() }, Source::User);
    let (i0, i1) = (l0.id, l1.id);
    // A room of two rectangles: 20.0 m2 gross (18.0 net) and 10.0 m2 gross (9.0 net).
    let part = |key: &str, x: i64, w: i64, net: f64, of: Option<&str>| {
        let area = w as f64 * 5000.0 / 1e6;
        let mut extra = serde_json::Map::new();
        if let Some(of) = of {
            extra.insert("partOf".into(), serde_json::json!(of));
        }
        let s = Space { func: "room".into(), requirement_key: key.into(), area, target_area: area, x, y: 0, w, h: 5000, boundary: vec![[x, 0], [x + w, 0], [x + w, 5000], [x, 5000]], net_area: Some(net), glazing_ratio: None, extra };
        let mut el = element_of(&s, Source::User);
        el.level_id = Some(i0);
        Command::Create(el)
    };
    // The ground level has a 900 mm door; the upper one an 800 mm door and a 900 mm door.
    let door = |level: Id, width: i64| {
        let w = Wall { x1: 0, y1: 9000, x2: 4000, y2: 9000, thickness: 200, height: 3000, length: 4000, is_exterior: true, room_a: String::new(), room_b: None, extra: Default::default() };
        let mut wel = element_of(&w, Source::User);
        wel.level_id = Some(level);
        let id = wel.id;
        let d = Door { width, height: 2100, offset_center: 1000, is_egress: false, from: String::new(), to: String::new(), opens_into: String::new(), extra: Default::default() };
        let mut del = element_of(&d, Source::User).hosted_by(id);
        del.level_id = Some(level);
        vec![Command::Create(wel), Command::Create(del)]
    };
    let mut cmds = vec![Command::Create(l0), Command::Create(l1), part("a", 0, 4000, 18.0, None), part("a-2", 4000, 2000, 9.0, Some("a")), room("b", 3700, Some(i1))];
    cmds.extend(door(i0, 900));
    cmds.extend(door(i1, 800));
    cmds.extend(door(i1, 900));
    doc.commit("two levels", Origin::User, cmds).unwrap();
    let html = print_html(&doc.model, &opts(true)).unwrap();
    let sheets: Vec<&str> = html.split("<section class=\"sheet\">").skip(1).collect();
    assert!(sheets[0].contains("<td>غرفة</td><td class=\"n\">30.0</td><td class=\"n\">27.0</td></tr>"), "one room, both rectangles, gross and net");
    assert!(!sheets[0].contains("<td>a-2</td>"), "a part is not a room of its own");
    assert!(sheets[0].contains("الصافية (م²)") && sheets[0].contains("<td class=\"n\">30.0</td><td class=\"n\">27.0</td></tr></table>"), "the totals");
    // 800 mm is D1 and 900 mm is D2 in the whole set: the ground sheet shows D2 only, with one on the level and two in the building.
    assert!(sheets[0].contains("<td>D2</td><td class=\"n\">900 × 2100</td><td>داخلي</td><td class=\"n\">1</td><td class=\"n\">2</td>") && !sheets[0].contains("<td>D1</td>"));
    assert!(sheets[1].contains("<td>D1</td><td class=\"n\">800 × 2100</td>") && sheets[1].contains("<td>D2</td><td class=\"n\">900 × 2100</td>"));
}
