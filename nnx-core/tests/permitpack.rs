//! The permit package. SYNTHETIC test values, checked by hand against the document.

use nnx_core::permitpack::{Context, Engineer, Input, Letterhead, build, sheet_number};
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
    Input { engineer: Engineer { name: "Eng One".into(), licence_no: "L-1".into(), licensing_body: "Council".into(), discipline: String::new() }, project_no: "P-1".into(), sheet_prefix: "S".into(), revision: "Rev 0".into(), issued_for: String::new(), paper: String::new(), letterhead: Default::default() }
}

fn one_room_doc() -> Document {
    let mut doc = Document::default();
    doc.commit("plan", Origin::User, vec![room("a", 4000, None)]).unwrap();
    doc
}

#[test]
fn paper_size_sets_the_page_and_scales_type_and_margins_from_a3() {
    let doc = one_room_doc();
    let a3 = build(&ctx(&doc.model, "2026-11-23", false), &input()).unwrap();
    assert_eq!(a3.paper, "A3");
    assert!(a3.html.contains("@page{size:A3 portrait;margin:12.0mm}") && a3.html.contains("h1{font-size:20.0px;"), "empty means A3 at the old values");
    for (p, size, margin, h1) in [("A1", "A1", "24.0mm", "40.0px"), ("a0", "A0", "34.0mm", "56.6px"), (" A4 ", "A4", "8.5mm", "14.1px")] {
        let mut i = input();
        i.paper = p.into();
        let r = build(&ctx(&doc.model, "2026-11-23", false), &i).unwrap();
        assert_eq!(r.paper, size);
        assert!(r.html.contains(&format!("@page{{size:{size} portrait;margin:{margin}}}")), "{size}: page rule");
        assert!(r.html.contains(&format!("h1{{font-size:{h1};")), "{size}: heading scales with the sheet");
        assert_eq!(r.sheets.len(), 2, "sheet numbering does not depend on the paper");
    }
    let mut i = input();
    i.paper = "A1".into();
    let a1 = build(&ctx(&doc.model, "2026-11-23", false), &i).unwrap();
    assert!(a1.html.contains("max-height:380.5mm"), "plan cap is the A3 share (190/420) of the A1 height 841");
}

#[test]
fn an_unknown_paper_size_is_refused_by_name() {
    let doc = one_room_doc();
    for p in ["A5", "Letter", "B1"] {
        let mut i = input();
        i.paper = p.into();
        let e = build(&ctx(&doc.model, "2026-11-23", false), &i).unwrap_err();
        assert!(e.contains("unknown paper size") && e.contains(&p.to_ascii_uppercase()), "{e}");
    }
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

#[test]
fn bilingual_pack_pairs_arabic_and_english_with_the_same_numbers_and_measurements() {
    let mut doc = Document::default();
    let l0 = element_of(&Level { name: "L0".into(), index: 0, extra: Default::default() }, Source::User);
    let l1 = element_of(&Level { name: "L1".into(), index: 1, extra: Default::default() }, Source::User);
    let (i0, i1) = (l0.id, l1.id);
    doc.commit("levels", Origin::User, vec![Command::Create(l0), Command::Create(l1), room("a", 4000, Some(i0)), room("b", 6000, Some(i1))]).unwrap();
    // `ctx.arabic` is ignored: both are built whichever way it is set.
    for flag in [true, false] {
        let b = nnx_core::permitpack::build_bilingual(&ctx(&doc.model, "2026-11-23", flag), &input()).unwrap();
        assert!(b.ar.html.contains("dir=\"rtl\"") && b.ar.html.contains("الدور الأرضي") && !b.ar.html.contains("Ground floor"));
        assert!(b.en.html.contains("dir=\"ltr\"") && b.en.html.contains("Ground floor") && !b.en.html.contains("الدور الأرضي"));
        let nums = |p: &nnx_core::permitpack::Pack| p.sheets.iter().map(|s| (s.number.clone(), s.kind)).collect::<Vec<_>>();
        assert_eq!(nums(&b.ar), nums(&b.en));
        assert_eq!(b.sheets.iter().map(|s| s.number.as_str()).collect::<Vec<_>>(), ["S-01", "S-02", "S-03"]);
        for h in [&b.ar.html, &b.en.html] {
            assert_eq!(h.matches("<section class=\"sheet\">").count(), 3);
            assert!(h.contains("<td class=\"n\">20.0</td></tr></table>") && h.contains("<td class=\"n\">30.0</td></tr></table>"), "same measured totals");
        }
        assert!(b.notes.iter().any(|n| n.contains("as entered")) && b.notes.iter().any(|n| n.contains("كما أُدخل")));
    }
}

#[test]
fn bilingual_pack_refuses_like_the_single_build() {
    let doc = one_room_doc();
    let e = nnx_core::permitpack::build_bilingual(&ctx(&doc.model, "2026-11-23", false), &Input::default()).unwrap_err();
    assert!(e.contains("project_no"), "{e}");
    assert!(nnx_core::permitpack::build_bilingual(&ctx(&Model::default(), "2026-11-23", true), &input()).unwrap_err().contains("no rooms"));
}

const PNG_1PX: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg==";

#[test]
fn letterhead_prints_on_every_sheet_escaped_and_nothing_when_empty() {
    let doc = one_room_doc();
    let plain = build(&ctx(&doc.model, "2026-11-23", false), &input()).unwrap();
    assert!(!plain.html.contains("class=\"lh\"") && !plain.html.contains("class=\"lf\"") && !plain.html.contains("<img"), "no letterhead entered: none printed");
    let mut i = input();
    i.letterhead = Letterhead { office_name: "Studio <X>".into(), address: "1 Main St".into(), contact: "a@b.test".into(), footer: "CR 123 & Co".into(), logo_data_uri: PNG_1PX.into() };
    let r = build(&ctx(&doc.model, "2026-11-23", false), &i).unwrap();
    assert_eq!(r.html.matches("class=\"lh\"").count(), r.sheets.len(), "one band per sheet");
    assert_eq!(r.html.matches("class=\"lf\"").count(), r.sheets.len(), "one footer per sheet");
    assert_eq!(r.html.matches("<img class=\"logo\"").count(), r.sheets.len());
    assert!(r.html.contains("Studio &lt;X&gt;") && !r.html.contains("Studio <X>") && r.html.contains("CR 123 &amp; Co") && r.html.contains(PNG_1PX));
    let mut a1 = i.clone();
    a1.paper = "A1".into();
    assert!(build(&ctx(&doc.model, "2026-11-23", true), &a1).unwrap().html.contains("max-height:50.0mm"), "logo cap scales with the paper");
    let mut text_only = i.clone();
    text_only.letterhead.logo_data_uri.clear();
    assert!(!build(&ctx(&doc.model, "2026-11-23", false), &text_only).unwrap().html.contains("<img"));
}

#[test]
fn a_bad_logo_is_refused_by_name() {
    let doc = one_room_doc();
    let long = format!("data:image/png;base64,{}", "A".repeat(2_000_000));
    for bad in ["https://example.test/logo.png", "C:\\logo.png", "data:text/html;base64,AAAA", "data:image/png;base64,", "data:image/png;base64,AB*D", "data:image/png;base64,ABC", "data:image/png;base64,A===", long.as_str()] {
        let mut i = input();
        i.letterhead.logo_data_uri = bad.into();
        let e = build(&ctx(&doc.model, "2026-11-23", false), &i).unwrap_err();
        assert!(e.contains("letterhead.logo_data_uri"), "{bad}: {e}");
    }
}
