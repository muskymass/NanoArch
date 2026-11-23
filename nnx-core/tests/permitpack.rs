//! The permit package. SYNTHETIC test values, checked by hand against the document.

use nnx_core::permitpack::{Context, Engineer, Input, build, sheet_number};
use nnx_core::typed::{Level, Space, element_of};
use nnx_core::*;

fn room(key: &str, w: i64, level: Option<Id>) -> Command {
    let area = w as f64 * 5000.0 / 1e6;
    let s = Space { func: "bedroom".into(), requirement_key: key.into(), area, target_area: area, x: 0, y: 0, w, h: 5000, boundary: vec![[0, 0], [w, 0], [w, 5000], [0, 5000]], net_area: None, glazing_ratio: None, extra: Default::default() };
    let mut el = element_of(&s, Source::User);
    el.level_id = level;
    Command::Create(el)
}

fn input() -> Input {
    Input { engineer: Engineer { name: "Eng One".into(), licence_no: "L-1".into(), licensing_body: "Council".into(), discipline: String::new() }, project_no: "P-1".into(), sheet_prefix: "S".into(), revision: "Rev 0".into(), issued_for: String::new() }
}

fn ctx<'a>(m: &'a Model, date: &'a str, arabic: bool) -> Context<'a> {
    Context { model: m, title: "Villa", labels: Default::default(), arabic, show_structure: false, show_mep: false, date }
}

#[test]
fn two_levels_give_an_index_and_two_numbered_plan_sheets() {
    let mut doc = Document::default();
    let l0 = element_of(&Level { name: "L0".into(), index: 0, extra: Default::default() }, Source::User);
    let l1 = element_of(&Level { name: "L1".into(), index: 1, extra: Default::default() }, Source::User);
    let (i0, i1) = (l0.id, l1.id);
    doc.commit("levels", Origin::User, vec![Command::Create(l0), Command::Create(l1), room("a", 4000, Some(i0)), room("b", 6000, Some(i1))]).unwrap();
    let p = build(&ctx(&doc.model, "2026-11-23", false), &input()).unwrap();
    let nums: Vec<&str> = p.sheets.iter().map(|s| s.number.as_str()).collect();
    assert_eq!(nums, ["S-01", "S-02", "S-03"]);
    assert_eq!(p.sheets.iter().map(|s| s.kind).collect::<Vec<_>>(), ["index", "plan", "plan"]);
    assert_eq!(p.html.matches("<section class=\"sheet\">").count(), 3);
    assert_eq!(p.html.matches("<svg").count(), 2);
    assert!(p.html.contains("3 / 3") && p.html.contains("Ground floor") && p.html.contains("Floor 1"));
    assert!(p.html.contains("<td class=\"n\">20.0</td></tr></table>") && p.html.contains("<td class=\"n\">30.0</td></tr></table>"), "room totals");
    assert!(!p.html.contains("Discipline"), "an empty discipline is not printed");
    assert_eq!(sheet_number(" S ", 12), "S-12");
}

#[test]
fn a_level_without_rooms_gets_no_sheet_and_no_number() {
    let mut doc = Document::default();
    let l0 = element_of(&Level { name: "L0".into(), index: 0, extra: Default::default() }, Source::User);
    let l1 = element_of(&Level { name: "L1".into(), index: 1, extra: Default::default() }, Source::User);
    let i0 = l0.id;
    doc.commit("levels", Origin::User, vec![Command::Create(l0), Command::Create(l1), room("a", 4000, Some(i0))]).unwrap();
    let p = build(&ctx(&doc.model, "2026-11-23", true), &input()).unwrap();
    assert_eq!(p.sheets.len(), 2);
    assert!(p.notes.iter().any(|n| n.contains("لا فراغات")));
    assert!(p.html.contains("dir=\"rtl\"") && p.html.contains("غير نافذة حتى يختمها"));
}

#[test]
fn refuses_missing_inputs_bad_dates_and_an_empty_model() {
    let mut doc = Document::default();
    doc.commit("plan", Origin::User, vec![room("a", 4000, None)]).unwrap();
    let e = build(&ctx(&doc.model, "2026-11-23", false), &Input::default()).unwrap_err();
    for k in ["engineer.name", "engineer.licence_no", "engineer.licensing_body", "project_no", "sheet_prefix", "revision"] {
        assert!(e.contains(k), "{e}");
    }
    for d in ["", "23-11-2026", "2026-13-01", "2026-11-00", "2026/11/23"] {
        assert!(build(&ctx(&doc.model, d, false), &input()).unwrap_err().contains("YYYY-MM-DD"), "{d}");
    }
    assert!(build(&ctx(&Model::default(), "2026-11-23", false), &input()).unwrap_err().contains("no rooms"));
}
