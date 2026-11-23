//! Office plugins (tracker item 97). Every value is SYNTHETIC test data.

use nnx_core::overlay::Overlay;
use nnx_core::plugins::{Plugin, PluginRoom, PluginTemplate, check_all, effective_overlays, find_template, review};

const RULES: &str = r#"{ "id": "OFFICE", "edition": "synthetic", "version": "1", "rules": [ { "id": "r.one", "clause": "OS-1", "applies_to": { "kind": "Space" }, "check": "area >= param.min", "params": { "min": { "value": 9, "source": "synthetic" } }, "severity": "warning", "message": "m" } ] }"#;

fn room(key: &str, func: &str, area: f64, floor: u32) -> PluginRoom {
    PluginRoom { key: key.into(), name: key.into(), func: func.into(), area_m2: area, floor }
}

fn template() -> PluginTemplate {
    PluginTemplate { id: "t1".into(), title_ar: "ت".into(), title_en: "T".into(), building_type: "villa".into(), plot_width_m: 10.0, plot_length_m: 20.0, floors: 2, rooms: vec![room("a", "bedroom", 16.0, 1), room("b", "bathroom", 2.0, 2)], ..Default::default() }
}

fn plugin() -> Plugin {
    Plugin { id: "office".into(), name: "Office".into(), version: "1".into(), publisher: "Synthetic".into(), source: "Standard 2027".into(), enabled: true, rules: Some(RULES.into()), templates: vec![template()], ..Default::default() }
}

fn with_rooms(rooms: Vec<PluginRoom>) -> Plugin {
    Plugin { templates: vec![PluginTemplate { rooms, ..template() }], ..plugin() }
}

#[test]
fn a_valid_plugin_has_no_problems_and_its_pack_is_an_overlay_layer() {
    let p = plugin();
    assert!(p.problems().is_empty(), "{:?}", p.problems());
    let o = p.as_overlay().unwrap();
    assert_eq!((o.id.as_str(), o.authority.as_str(), o.document.as_str()), ("plugin:office", "Synthetic", "Standard 2027"));
    let own = Overlay { id: "ov1".into(), ..o.clone() };
    let eff = effective_overlays(&[own], &[p.clone(), Plugin { id: "off".into(), enabled: false, ..p }]);
    assert_eq!(eff.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(), ["ov1", "plugin:office"], "a disabled plugin adds no layer");
}

#[test]
fn what_is_missing_or_impossible_is_refused_by_name() {
    let cases: Vec<(Plugin, &str)> = vec![
        (Plugin { id: "Bad Id".into(), ..plugin() }, "lower-case"),
        (Plugin { publisher: " ".into(), ..plugin() }, "publisher"),
        (Plugin { source: "".into(), ..plugin() }, "source"),
        (Plugin { version: "".into(), ..plugin() }, "version"),
        (Plugin { date: Some("2027-02-30".into()), ..plugin() }, "not a date"),
        (Plugin { rules: None, templates: vec![], ..plugin() }, "neither rules nor templates"),
        (Plugin { rules: Some(RULES.replace("\"synthetic\" }", "\"\" }")), ..plugin() }, "no source"),
        (Plugin { rules: Some("{".into()), ..plugin() }, "json"),
        (Plugin { templates: vec![template(), template()], ..plugin() }, "used twice"),
        (Plugin { templates: vec![PluginTemplate { plot_width_m: 0.0, ..template() }], ..plugin() }, "plot width"),
        (Plugin { templates: vec![PluginTemplate { floors: 0, ..template() }], ..plugin() }, "at least one floor"),
        (with_rooms(vec![room("a", "bedroom", 16.0, 3)]), "floor 3"),
        (with_rooms(vec![room("a", "Bed", 16.0, 1)]), "lower-case"),
        (with_rooms(vec![room("a", "bedroom", f64::NAN, 1)]), "positive"),
        (with_rooms(vec![room("a", "bedroom", 5.0, 1), room("a", "bedroom", 5.0, 1)]), "twice"),
        (with_rooms(vec![room("a", "hall", 300.0, 1)]), "more than the whole plot"),
        (with_rooms(vec![]), "room program"),
    ];
    for (p, msg) in cases {
        let all = p.problems().join(" | ");
        assert!(all.contains(msg), "expected '{msg}' in: {all}");
    }
}

#[test]
fn the_list_is_checked_for_duplicates_and_the_limit() {
    assert!(check_all(&[plugin()]).is_ok());
    assert!(check_all(&[plugin(), plugin()]).unwrap_err().contains("used twice"));
    let many: Vec<Plugin> = (0..21).map(|i| Plugin { id: format!("p{i}"), ..plugin() }).collect();
    assert!(check_all(&many).unwrap_err().contains("at most 20"));
    assert!(check_all(&[Plugin { publisher: "".into(), ..plugin() }]).unwrap_err().contains("plugin 'office'"));
}

#[test]
fn notes_report_rooms_below_the_sbc_minimum_and_types_without_a_value() {
    let t = PluginTemplate { rooms: vec![room("a", "bedroom", 5.0, 1), room("b", "bedroom", 16.0, 1), room("c", "hall", 10.0, 1)], ..template() };
    let n = t.notes();
    assert_eq!(n.len(), 2, "{n:?}");
    assert!(n[0].contains("room 'a'") && n[0].contains("below the SBC minimum") && n[0].contains("6.5"), "{}", n[0]);
    assert!(n[1].contains("room 'c'") && n[1].contains("not checked"), "{}", n[1]);
}

#[test]
fn templates_are_found_only_in_enabled_plugins_by_address() {
    let list = vec![plugin(), Plugin { id: "off".into(), enabled: false, ..plugin() }];
    assert_eq!(find_template(&list, "office/t1").unwrap().id, "t1");
    assert!(find_template(&list, "off/t1").unwrap_err().contains("disabled"));
    assert!(find_template(&list, "office/zz").unwrap_err().contains("no template"));
    assert!(find_template(&list, "nope/t1").unwrap_err().contains("no plugin"));
    assert!(find_template(&list, "t1").unwrap_err().contains("not a plugin template address"));
}

#[test]
fn review_says_what_is_added_replaced_and_needs_confirmation() {
    let first = review(&plugin(), &[], &[]);
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    assert_eq!(first.rules_added, ["r.one"]);
    assert!(!first.needs_confirm);
    assert_eq!(first.templates, ["t1"]);
    // A second plugin with the same rule id replaces it: confirmation.
    let other = Plugin { id: "second".into(), ..plugin() };
    let r = review(&other, &[], &[plugin()]);
    assert_eq!(r.rules_replaced.len(), 1);
    assert_eq!(r.rules_replaced[0].0, "r.one");
    assert!(r.needs_confirm);
    // The same id again replaces the plugin itself (its own rules are not counted as a clash).
    let same = review(&plugin(), &[], &[plugin()]);
    assert!(same.needs_confirm && same.rules_replaced.is_empty(), "{same:?}");
    assert!(same.notes.iter().any(|n| n.contains("will be replaced")));
    // An invalid plugin comes back with errors and nothing else.
    let bad = review(&Plugin { publisher: "".into(), ..plugin() }, &[], &[]);
    assert!(!bad.errors.is_empty() && bad.rules_added.is_empty());
}

#[test]
fn plugins_survive_the_project_round_trip_and_an_invalid_one_never_reaches_the_project() {
    let mut p = nnx_core::project::NanoArchProject::default();
    p.plugins.push(plugin());
    assert!(p.check().is_ok());
    let v = serde_json::to_value(&p).unwrap();
    let back: nnx_core::project::NanoArchProject = serde_json::from_value(v).unwrap();
    assert_eq!(back.plugins, p.plugins);
    p.plugins.push(Plugin { publisher: "".into(), id: "x".into(), ..plugin() });
    assert!(p.check().unwrap_err().contains("plugin 'x'"));
}
