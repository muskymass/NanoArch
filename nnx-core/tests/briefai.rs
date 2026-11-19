//! The brief from a description: keywords (offline) and the checking of a model's answer.

use nnx_core::briefai::{self, FUNCS};
use nnx_core::project::BuildingType;
use serde_json::json;

const CLIENT: &str = "فيلا دورين وملحق في الجبيل على أرض 600م طراز نجدي حديث";

#[test]
fn the_clients_own_example_gives_villa_two_floors_annex_jubail_600_m2_and_najdi() {
    let d = briefai::from_keywords(CLIENT);
    assert_eq!(d.source, "keywords");
    assert_eq!(d.building_type, Some(BuildingType::Villa));
    assert_eq!(d.floors, Some(2));
    assert!(d.annex);
    assert_eq!((d.city.as_deref(), d.climate_zone), (Some("Jubail"), Some(1)));
    assert_eq!(d.plot_area_m2, Some(600.0));
    assert_eq!(d.style.as_deref(), Some("modern_najdi"));
    // Only the area is given: the shape is not assumed, and the user is told what to enter.
    assert_eq!((d.plot_width_m, d.plot_length_m), (None, None));
    let notes = d.notes.join(" | ");
    assert!(notes.contains("only the area (600 m²) is given") && notes.contains("no shape is assumed"), "{notes}");
    assert!(notes.contains("annex") && notes.contains("stair"), "{notes}");
    assert!(d.rooms.is_empty(), "no room is named, none is invented");
}

#[test]
fn english_text_gives_the_same_kind_of_answer_with_size_rooms_and_bedrooms_upstairs_marked_assumed() {
    let d = briefai::from_keywords("Two-storey villa with an annex in Riyadh on a 20x30 m plot, contemporary style, 4 bedrooms, a majlis, a kitchen and two bathrooms");
    assert_eq!((d.building_type, d.floors, d.annex), (Some(BuildingType::Villa), Some(2), true));
    assert_eq!((d.city.as_deref(), d.climate_zone), (Some("Riyadh"), Some(1)));
    assert_eq!((d.plot_width_m, d.plot_length_m, d.plot_area_m2), (Some(20.0), Some(30.0), Some(600.0)));
    assert_eq!(d.style.as_deref(), Some("contemporary"));
    let count = |f: &str| d.rooms.iter().filter(|r| r.func == f).count();
    assert_eq!((count("bedroom"), count("majlis"), count("kitchen"), count("bathroom")), (4, 1, 1, 2));
    // Bedrooms of a two-floor building are put upstairs, and that is flagged as a proposal.
    assert!(d.rooms.iter().filter(|r| r.func == "bedroom").all(|r| r.floor == 2 && r.assumed));
    assert!(d.rooms.iter().filter(|r| r.func != "bedroom").all(|r| r.floor == 1 && !r.assumed));
    assert!(d.rooms.iter().all(|r| r.area_m2.is_none()), "areas are not invented");
    assert!(d.notes.iter().any(|n| n.contains("does not give room areas")));
}

#[test]
fn arabic_indic_digits_number_words_and_building_types_are_read() {
    let d = briefai::from_keywords("عمارة سكنية ٥ أدوار على أرض ٨٠٠ متر مربع في تبوك");
    assert_eq!((d.building_type, d.floors, d.plot_area_m2), (Some(BuildingType::ApartmentBuilding), Some(5), Some(800.0)));
    assert_eq!((d.city.as_deref(), d.climate_zone), (Some("Tabuk"), Some(2)));
    let d = briefai::from_keywords("منزل ثلاث غرف نوم ومجلس وصالة ومطبخ وحمامين");
    assert_eq!(d.building_type, Some(BuildingType::House));
    let count = |f: &str| d.rooms.iter().filter(|r| r.func == f).count();
    assert_eq!((count("bedroom"), count("majlis"), count("living"), count("kitchen"), count("bathroom")), (3, 1, 1, 1, 2));
    assert!(d.rooms.iter().filter(|r| r.func == "bedroom").all(|r| r.floor == 1 && !r.assumed), "one floor: no upstairs");
    assert_eq!(briefai::from_keywords("قصر ثلاثة أدوار في الرياض").floors, Some(3));
    assert_eq!(briefai::from_keywords("villa with 3 floors").floors, Some(3));
    assert_eq!(briefai::from_keywords("فيلا دور واحد").floors, Some(1));
    assert_eq!(briefai::from_keywords("a palace").building_type, Some(BuildingType::Palace));
    assert_eq!(briefai::from_keywords("Jeddah neo-classical house").style.as_deref(), Some("neo_classical"));
}

#[test]
fn a_distance_is_not_mistaken_for_a_plot_area_and_empty_or_unrelated_text_fills_nothing() {
    // "15م" is a street width here, not an area: no land word, no area unit.
    let d = briefai::from_keywords("فيلا على شارع عرض 15م");
    assert_eq!(d.plot_area_m2, None);
    assert_eq!(briefai::from_keywords("land 450 sqm").plot_area_m2, Some(450.0));
    assert_eq!(briefai::from_keywords("on a plot of 30 m2").plot_area_m2, None, "below any plot size");
    for text in ["", "   ", "hello world", "أريد شيئاً جميلاً"] {
        let d = briefai::from_keywords(text);
        assert!(d.building_type.is_none() && d.floors.is_none() && d.plot_area_m2.is_none() && d.city.is_none() && d.style.is_none() && d.rooms.is_empty() && !d.annex, "{text}");
    }
    // The text is matched with its Arabic letters unified: hamza and ta marbuta spellings agree.
    assert_eq!(briefai::norm("أَرْض"), "ارض");
    assert_eq!(briefai::norm("غرفة"), "غرفه");
    assert_eq!(briefai::norm("٦٠٠"), "600");
}

#[test]
fn the_models_answer_is_checked_field_by_field_and_bad_values_are_dropped_with_a_note() {
    let v = json!({
        "title": "فيلا الجبيل", "building_type": "villa", "style": "contemporary", "city": "Jubail", "floors": 2, "annex": true,
        "plot_area_m2": 600, "plot_width_m": null, "smart_tech": ["lighting"],
        "rooms": [
            { "func": "majlis", "name": "المجلس", "area_m2": 40, "floor": 1, "assumed": false },
            { "func": "bedroom", "name": "نوم", "area_m2": 20, "floor": 2, "assumed": true },
            { "func": "sauna", "floor": 1, "assumed": false },
            { "func": "kitchen", "area_m2": -5, "floor": 1, "assumed": false },
            { "func": "bathroom", "area_m2": 6, "floor": 3, "assumed": false }
        ]
    });
    let d = briefai::from_model_json(&v, "qwen").unwrap();
    assert_eq!((d.source.as_str(), d.floors, d.plot_area_m2, d.climate_zone), ("ollama:qwen", Some(2), Some(600.0), Some(1)));
    assert_eq!(d.rooms.len(), 3, "the unknown function and the third-floor room are dropped");
    assert!(d.rooms.iter().any(|r| r.func == "kitchen" && r.area_m2.is_none()), "a negative area is dropped");
    let notes = d.notes.join(" | ");
    assert!(notes.contains("unknown function 'sauna'") && notes.contains("floor 3"), "{notes}");
    let bad = briefai::from_model_json(&json!({ "building_type": "castle", "style": "baroque", "city": "Atlantis", "floors": 99, "rooms": [] }), "m").unwrap();
    assert_eq!((bad.building_type, bad.style, bad.city, bad.floors), (None, None, None, Some(20)));
    let n = bad.notes.join(" | ");
    assert!(n.contains("building type 'castle'") && n.contains("style 'baroque'") && n.contains("'Atlantis'"), "{n}");
    assert!(briefai::from_model_json(&json!([1, 2]), "m").is_err());
}

#[test]
fn where_the_text_and_the_model_disagree_the_text_wins_and_says_so() {
    let model = briefai::from_model_json(
        &json!({ "building_type": "palace", "style": "neo_classical", "floors": 3, "annex": false, "plot_area_m2": 500, "city": "Riyadh",
                 "rooms": [{ "func": "bedroom", "floor": 2, "assumed": true }, { "func": "bedroom", "floor": 2, "assumed": true }] }),
        "m",
    )
    .unwrap();
    let text = "فيلا دورين وملحق في الجبيل على أرض 600م طراز نجدي حديث وأربع غرف نوم";
    let d = briefai::reconcile(model, text);
    assert_eq!((d.building_type, d.floors, d.plot_area_m2), (Some(BuildingType::Villa), Some(2), Some(600.0)));
    assert_eq!(d.style.as_deref(), Some("modern_najdi"));
    assert_eq!((d.city.as_deref(), d.climate_zone), (Some("Jubail"), Some(1)));
    assert!(d.annex);
    assert_eq!(d.rooms.iter().filter(|r| r.func == "bedroom").count(), 4, "the text counts four bedrooms");
    let n = d.notes.join(" | ");
    for needle in ["the text says", "kept the text", "bedrooms: the text says 4", "only the plot area is known", "assumed", "stair"] {
        assert!(n.contains(needle), "{needle} in {n}");
    }
    // What the model got right and the text does not contradict is kept; what only the text says is added.
    let only_model = briefai::reconcile(briefai::from_model_json(&json!({ "building_type": "villa", "floors": 2, "annex": false, "rooms": [] }), "m").unwrap(), "any text");
    assert_eq!((only_model.building_type, only_model.floors), (Some(BuildingType::Villa), Some(2)));
    let filled = briefai::reconcile(briefai::from_model_json(&json!({ "annex": false, "rooms": [] }), "m").unwrap(), CLIENT);
    assert_eq!((filled.building_type, filled.floors), (Some(BuildingType::Villa), Some(2)), "facts the model missed come from the text");
}

#[test]
fn an_inconsistent_plot_is_flagged_and_the_schema_lists_the_planners_room_functions() {
    let model = briefai::from_model_json(&json!({ "building_type": "villa", "floors": 1, "annex": false, "plot_area_m2": 600, "plot_width_m": 20, "plot_length_m": 20, "rooms": [] }), "m").unwrap();
    let d = briefai::reconcile(model, "villa");
    assert!(d.notes.iter().any(|n| n.contains("does not equal")), "{:?}", d.notes);
    let schema = briefai::schema();
    assert_eq!(schema["properties"]["rooms"]["items"]["properties"]["func"]["enum"], json!(FUNCS));
    assert!(schema["required"].as_array().unwrap().iter().any(|r| r == "floors"));
    assert!(briefai::system_prompt().contains("null for anything the text does not state"));
}
