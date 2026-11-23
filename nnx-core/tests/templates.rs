//! Starter templates and suggested room areas.

use nnx_core::briefai::{self, suggest_areas, suggested_area_m2};
use nnx_core::sbc::code_value;
use nnx_core::site::{self, RuleSet};
use nnx_core::templates;

#[test]
fn every_suggested_and_template_area_is_above_the_sbc_minimum_for_its_type() {
    for f in ["majlis", "living", "dining", "kitchen", "bedroom", "bathroom"] {
        let min = code_value(&format!("room_min_area_m2.{f}")).unwrap_or_else(|| panic!("no SBC minimum for {f}")).value;
        assert!(suggested_area_m2(f) > min, "{f}: suggested {} vs minimum {min}", suggested_area_m2(f));
    }
    for t in templates::all() {
        assert!(!t.rooms.is_empty());
        for r in &t.rooms {
            if let Some(cv) = code_value(&format!("room_min_area_m2.{}", r.func)) {
                assert!(r.area_m2 >= cv.value, "{} / {}: {} < {}", t.id, r.key, r.area_m2, cv.value);
            }
            assert!(r.floor >= 1 && r.floor <= t.floors, "{} / {}", t.id, r.key);
        }
        let mut keys: Vec<&String> = t.rooms.iter().map(|r| &r.key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), t.rooms.len(), "{}: unique room keys", t.id);
    }
}

#[test]
fn rooms_without_an_area_get_a_marked_suggestion_and_given_areas_are_left_alone() {
    let mut rooms = vec![
        briefai::DraftRoom { func: "bedroom".into(), name: None, area_m2: None, area_suggested: false, floor: 1, assumed: false },
        briefai::DraftRoom { func: "majlis".into(), name: None, area_m2: Some(55.0), area_suggested: false, floor: 1, assumed: false },
    ];
    assert_eq!(suggest_areas(&mut rooms), 1);
    assert_eq!((rooms[0].area_m2, rooms[0].area_suggested), (Some(16.0), true));
    assert_eq!((rooms[1].area_m2, rooms[1].area_suggested), (Some(55.0), false));
    assert_eq!(suggest_areas(&mut rooms), 0);
}

#[test]
fn keyword_and_model_rooms_never_come_back_with_an_empty_area() {
    let d = briefai::from_keywords("villa two floors 3 bedrooms, a majlis, a kitchen and two bathrooms");
    assert!(!d.rooms.is_empty() && d.rooms.iter().all(|r| r.area_m2.is_some_and(|a| a > 0.0) && r.area_suggested));
    assert!(d.notes.iter().any(|n| n.contains("suggested target area")), "{:?}", d.notes);
    let m = briefai::from_model_json(&serde_json::json!({ "building_type": "villa", "floors": 1, "annex": false, "rooms": [{ "func": "kitchen", "floor": 1, "assumed": false, "area_m2": -4 }, { "func": "majlis", "floor": 1, "assumed": false, "area_m2": 40 }] }), "m").unwrap();
    assert_eq!(m.rooms.iter().map(|r| (r.area_m2, r.area_suggested)).collect::<Vec<_>>(), vec![(Some(15.0), true), (Some(40.0), false)]);
    let r = briefai::reconcile(m, "villa");
    assert!(r.notes.iter().any(|n| n.contains("marked suggested")));
}

#[test]
fn the_starter_templates_are_the_ones_asked_for_and_their_setbacks_come_from_the_ministry_rule() {
    let all = templates::all();
    assert_eq!(all.iter().map(|t| t.id).collect::<Vec<_>>(), vec!["villa_jubail", "palace", "apartments_4", "duplex", "annex"]);
    assert_eq!(all[0].title_ar, "فيلا سكنية - الجبيل (600 م² - طراز نجدي حديث)");
    assert_eq!(all[1].title_ar, "قصر سكني (1,500 م² - كلاسيكي حديث)");
    assert_eq!(all[2].title_ar, "عمارة سكنية 4 أدوار (900 م² - معاصر)");
    assert_eq!([all[0].plot_width_m * all[0].plot_length_m, all[1].plot_width_m * all[1].plot_length_m, all[2].plot_width_m * all[2].plot_length_m], [600.0, 1500.0, 900.0]);
    assert_eq!(all.iter().map(|t| t.style).collect::<Vec<_>>(), vec!["modern_najdi", "neo_classical", "contemporary", "contemporary", "contemporary"]);
    assert_eq!(all[2].floors, 4);
    // Villa: street 15 m -> front 3 m (a fifth, not less than 3 m); neighbours 1.5 m.
    let (sb, src) = templates::setbacks(&all[0]).unwrap();
    assert_eq!(sb, [3.0, 1.5, 1.5]);
    assert!(src.iter().all(|s| s.contains("clause 4-")), "{src:?}");
    // Palace: street 20 m -> 4 m. Apartments (4 floors, not more than 5): neighbours 2 m.
    assert_eq!(templates::setbacks(&all[1]).unwrap().0[0], 4.0);
    let (apt, src) = templates::setbacks(&all[2]).unwrap();
    assert_eq!((apt[1], apt[2]), (2.0, 2.0));
    assert!(src.iter().any(|s| s.contains("4-2")));
    assert_eq!(all[2].rule, RuleSet::MinistryApartment);
    assert!(templates::find("nope").is_none());
    // The Ministry setbacks leave a building on every template plot.
    for t in &all {
        let (s, _) = templates::setbacks(t).unwrap();
        let mut i = site::rectangle_with_setbacks(t.plot_width_m, t.plot_length_m, s[0], s[2], s[1]);
        i.floors = Some(t.floors);
        let r = site::analyse(&i).unwrap();
        assert!((r.building.width_m - (t.plot_width_m - 2.0 * s[1])).abs() < 0.01 && (r.building.length_m - (t.plot_length_m - s[0] - s[2])).abs() < 0.01, "{}: {:?}", t.id, r.building);
    }
}

#[test]
fn a_rectangular_plot_with_its_setbacks_draws_the_same_site_plan_as_a_polygon() {
    let r = site::analyse(&site::rectangle_with_setbacks(18.0, 22.0, 3.0, 2.0, 1.5)).unwrap();
    assert_eq!(r.setbacks_m, vec![3.0, 1.5, 2.0, 1.5]);
    assert_eq!((r.building.width_m, r.building.length_m), (15.0, 17.0));
    let svg = site::site_svg(&r, "T");
    // North arrow, scale bar, the setbacks, the street edge and the dimensions of the building rectangle.
    for needle in [">N</text>", "10 m</text>", "15 m</text>", "17 m</text>", "front street", "3 m</text>", "1.5 m</text>", "class=\"building\"", "class=\"buildable\""] {
        assert!(svg.contains(needle), "{needle}");
    }
    assert!(svg.starts_with("<svg") && svg.trim_end().ends_with("</svg>"));
}

#[test]
fn the_duplex_and_the_annex_are_complete_starting_points_with_ministry_setbacks() {
    let duplex = templates::find("duplex").unwrap();
    assert_eq!((duplex.floors, duplex.plot_width_m * duplex.plot_length_m), (2, 400.0));
    for f in 1..=2 {
        assert!(duplex.rooms.iter().any(|r| r.floor == f && r.func == "kitchen") && duplex.rooms.iter().filter(|r| r.floor == f && r.func == "bedroom").count() == 3);
    }
    assert_eq!(templates::setbacks(&duplex).unwrap().0, [3.0, 1.5, 1.5]);
    let annex = templates::find("annex").unwrap();
    assert_eq!((annex.floors, annex.stair, annex.plot_width_m * annex.plot_length_m), (1, None, 180.0));
    let funcs: Vec<&str> = annex.rooms.iter().map(|r| r.func).collect();
    assert_eq!(funcs, vec!["majlis", "kitchen", "bedroom", "bathroom"]);
    assert_eq!(templates::setbacks(&annex).unwrap().0, [3.0, 1.5, 1.5]);
    for t in [&duplex, &annex] {
        assert!(t.assumptions.iter().any(|a| a.contains("clause 4-1")), "{}", t.id);
    }
}
