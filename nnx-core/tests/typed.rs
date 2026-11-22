//! Dimensions are SYNTHETIC test values, not code requirements.

use nnx_core::layout::*;
use nnx_core::rules::{RuleSet, check};
use nnx_core::typed::{self, Door, Space, Typed, TypedError, Wall, Window, element_of};
use nnx_core::walls::{self, WindowOpts};
use nnx_core::*;
use serde_json::json;

fn wall() -> Wall {
    Wall {
        x1: 0,
        y1: 0,
        x2: 4000,
        y2: 0,
        thickness: 150,
        height: 3000,
        length: 4000,
        is_exterior: false,
        room_a: "a".into(),
        room_b: Some("b".into()),
        extra: Default::default(),
    }
}

fn door(offset: i64) -> Door {
    Door {
        width: 900,
        height: 2100,
        offset_center: offset,
        is_egress: false,
        from: "a".into(),
        to: "b".into(),
        opens_into: "a".into(),
        extra: Default::default(),
    }
}

fn window() -> Window {
    Window { width: 1500, height: 1200, sill_height: 900, offset_center: 2000, room: "a".into(), extra: Default::default() }
}

fn space(key: &str) -> Space {
    Space {
        func: "bedroom".into(),
        requirement_key: key.into(),
        area: 20.0,
        target_area: 20.0,
        x: 0,
        y: 0,
        w: 4000,
        h: 5000,
        boundary: vec![[0, 0], [4000, 0], [4000, 5000], [0, 5000]],
        net_area: None,
        glazing_ratio: None,
        extra: Default::default(),
    }
}

/// A strict document holding one wall; returns the wall id.
fn with_wall() -> (Document, Id) {
    let mut doc = Document::default();
    let w = element_of(&wall(), Source::User);
    let id = w.id;
    doc.commit("wall", Origin::User, vec![Command::Create(w)]).unwrap();
    (doc, id)
}

fn invalid(doc: &mut Document, cmds: Vec<Command>) -> String {
    let before = doc.model.state_hash();
    let err = doc.commit("bad", Origin::User, cmds).unwrap_err();
    assert_eq!(doc.model.state_hash(), before, "a rejected commit must leave the model untouched");
    match err.source {
        CommandError::Invalid(_, msg) => msg,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn typed_round_trip_is_lossless_and_keeps_unknown_properties() {
    let mut el = element_of(&door(2000), Source::User);
    el.props.insert("fireRating".into(), json!(60));
    let d: Door = el.typed().unwrap();
    assert_eq!(d.extra["fireRating"], json!(60));
    assert_eq!(d.offset_center, 2000);

    let back = element_of(&d, Source::User);
    assert_eq!(back.props, el.props, "no property gained or lost");
    // Property names are the camelCase names already used in saved files.
    assert!(el.props.contains_key("offsetCenter") && el.props.contains_key("isEgress") && el.props.contains_key("opensInto"));
}

#[test]
fn type_errors_are_specific() {
    let el = element_of(&wall(), Source::User);
    assert_eq!(
        el.typed::<Door>().unwrap_err(),
        TypedError::WrongKind { expected: "Door", found: "Wall".into() }
    );

    let mut missing = element_of(&door(2000), Source::User);
    missing.props.remove("opensInto");
    assert!(missing.typed::<Door>().unwrap_err().to_string().contains("opensInto"));

    let mut frac = element_of(&door(2000), Source::User);
    frac.props.insert("width".into(), json!(1500.5));
    assert!(frac.typed::<Door>().unwrap_err().to_string().contains("whole number of millimetres"));

    // An integral float is accepted: 1500.0 mm is 1500 mm.
    let mut whole = element_of(&door(2000), Source::User);
    whole.props.insert("width".into(), json!(1500.0));
    assert_eq!(whole.typed::<Door>().unwrap().width, 1500);
}

#[test]
fn strict_commit_rejects_invalid_walls() {
    let mut doc = Document::default();
    let mut w = wall();
    w.length = 3999;
    assert!(invalid(&mut doc, vec![Command::Create(element_of(&w, Source::User))]).contains("length"));

    let mut w = wall();
    w.y2 = 500; // diagonal
    w.length = 4500;
    assert!(invalid(&mut doc, vec![Command::Create(element_of(&w, Source::User))]).contains("axis-aligned"));

    let mut w = wall();
    w.thickness = 0;
    assert!(invalid(&mut doc, vec![Command::Create(element_of(&w, Source::User))]).contains("positive"));
    assert!(!doc.can_undo(), "nothing was recorded for rejected commits");
}

#[test]
fn strict_commit_rejects_openings_that_do_not_fit_or_have_no_host() {
    let (mut doc, wall_id) = with_wall();

    let loose = element_of(&door(2000), Source::User); // no host
    assert!(invalid(&mut doc, vec![Command::Create(loose)]).contains("hosted"));

    let too_close = element_of(&door(300), Source::User).hosted_by(wall_id); // 300 - 450 < 0
    assert!(invalid(&mut doc, vec![Command::Create(too_close)]).contains("does not fit"));

    let mut tall = door(2000);
    tall.height = 3500;
    assert!(invalid(&mut doc, vec![Command::Create(element_of(&tall, Source::User).hosted_by(wall_id))]).contains("wall height"));

    let mut w = window();
    w.sill_height = 2000; // 2000 + 1200 > 3000
    assert!(invalid(&mut doc, vec![Command::Create(element_of(&w, Source::User).hosted_by(wall_id))]).contains("sill"));

    // A door hosted by something that is not a wall.
    let sp = element_of(&space("s1"), Source::User);
    let sp_id = sp.id;
    doc.commit("space", Origin::User, vec![Command::Create(sp)]).unwrap();
    assert!(invalid(&mut doc, vec![Command::Create(element_of(&door(2000), Source::User).hosted_by(sp_id))]).contains("not a valid wall"));

    // The valid ones are accepted.
    let ok = element_of(&door(2000), Source::User).hosted_by(wall_id);
    doc.commit("door", Origin::User, vec![Command::Create(ok)]).unwrap();
}

#[test]
fn editing_a_wall_cannot_orphan_the_openings_it_hosts() {
    let (mut doc, wall_id) = with_wall();
    let d = element_of(&door(3500), Source::User).hosted_by(wall_id); // 3050..3950 of 4000
    doc.commit("door", Origin::User, vec![Command::Create(d)]).unwrap();

    let shorten = |len: i64| {
        vec![
            Command::SetProp { id: wall_id, key: "x2".into(), value: Some(json!(len)) },
            Command::SetProp { id: wall_id, key: "length".into(), value: Some(json!(len)) },
        ]
    };
    assert!(invalid(&mut doc, shorten(3000)).contains("does not fit"));
    // Shortening only as far as the door allows is fine.
    doc.commit("shorten", Origin::User, shorten(3960)).unwrap();
    // Changing one of the two linked fields alone is caught.
    let only_x2 = vec![Command::SetProp { id: wall_id, key: "x2".into(), value: Some(json!(3980)) }];
    assert!(invalid(&mut doc, only_x2).contains("length"));
}

#[test]
fn spaces_must_have_consistent_area_unique_keys_and_sane_net_area() {
    let mut doc = Document::default();
    let mut s = space("bed1");
    s.area = 25.0; // 4 m x 5 m is 20 m2
    assert!(invalid(&mut doc, vec![Command::Create(element_of(&s, Source::User))]).contains("does not match"));

    let mut s = space("bed1");
    s.net_area = Some(77.7);
    assert!(invalid(&mut doc, vec![Command::Create(element_of(&s, Source::User))]).contains("netArea"));

    doc.commit("a", Origin::User, vec![Command::Create(element_of(&space("bed1"), Source::User))]).unwrap();
    assert!(invalid(&mut doc, vec![Command::Create(element_of(&space("bed1"), Source::User))]).contains("already used"));
}

#[test]
fn rule_fix_that_would_break_the_schema_is_refused_not_applied() {
    let (mut doc, wall_id) = with_wall();
    let mut d = door(2000);
    d.is_egress = true;
    doc.commit("door", Origin::User, vec![Command::Create(element_of(&d, Source::User).hosted_by(wall_id))]).unwrap();

    // The rule's fix value (1100.5 mm) is not a whole number of millimetres.
    let mut rules = RuleSet::default();
    rules
        .add_layer(
            r#"{"id":"SYN","edition":"synthetic","version":"1","rules":[{
                "id":"w","clause":"S","applies_to":{"kind":"Door","where":"isEgress == true"},
                "check":"width >= param.min","params":{"min":{"value":1100.5,"source":"synthetic"}},
                "severity":"error","message":"narrow","fix":{"prop":"width","value":"param.min"}}]}"#,
        )
        .unwrap();
    let report = check(&doc.model, &rules);
    let fix = report.findings[0].fix.clone().unwrap();
    assert!(invalid(&mut doc, vec![fix]).contains("whole number of millimetres"));
}

fn brief() -> Brief {
    let r = |k: &str, f: &str, a: f64, z: Zone, light: bool| RoomReq {
        key: k.into(),
        func: f.into(),
        target_area_m2: a,
        min_width_mm: 2000,
        zone: z,
        needs_light: light,
    };
    Brief {
        envelope: Rect { x: 0, y: 0, w: 10_000, h: 9_000 },
        rooms: vec![
            r("living", "living", 40.0, Zone::Public, true),
            r("bed", "bedroom", 28.0, Zone::Private, true),
            r("bath", "bathroom", 8.0, Zone::Service, false),
        ],
        adjacency: vec![],
        entry: Side::South,
        entry_at: None,
        wall_mm: None,
        wall_height_mm: None,
    }
}

#[test]
fn generated_houses_are_valid_and_survive_save_and_load() {
    let b = brief();
    let layout = generate(&b, &Options { restarts: 4, iterations: 3_000, ..Options::default() }).unwrap().remove(0);
    let plan = walls::build(&layout, b.envelope, b.entry, &walls::WallOpts::default());
    let wp = walls::place_windows(&layout, &plan, &walls::WallOpts::default(), &WindowOpts::default());
    let space_cmds = to_commands(&layout, None);
    let ids = walls::space_ids(&space_cmds);
    let mut all = space_cmds;
    all.extend(walls::to_commands(&plan, &ids, None));
    all.extend(walls::window_commands(&plan, &wp, &ids, None));

    let mut doc = Document::default(); // strict: the commit itself proves validity
    doc.commit("house", Origin::Generator, all).unwrap();
    assert!(typed::validate_model(&doc.model).is_empty());

    let path = std::env::temp_dir().join(format!("nnx-typed-{}.nnx", std::process::id()));
    persist::save(&doc.model, &path).unwrap();
    let loaded = persist::load(&path).unwrap();
    std::fs::remove_file(&path).ok();
    assert_eq!(loaded.state_hash(), doc.model.state_hash());
    assert!(typed::validate_model(&loaded).is_empty(), "loaded model is valid");
    // Typed reads work on loaded data.
    let n = loaded.elements.values().filter(|e| e.kind == Wall::KIND).filter_map(|e| e.typed::<Wall>().ok()).count();
    assert_eq!(n, plan.walls.len());
}

#[test]
fn lenient_documents_accept_partial_data_and_validate_model_reports_it() {
    let mut doc = Document::lenient();
    let partial = Element::new("Door", Discipline::Arc, Source::Imported).with_prop("width", 900);
    doc.commit("import", Origin::User, vec![Command::Create(partial)]).unwrap();
    let issues = typed::validate_model(&doc.model);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].kind, "Door");

    // Unknown kinds are never rejected, even by a strict document.
    let mut strict = Document::default();
    let custom = Element::new("Sculpture", Discipline::Site, Source::User).with_prop("anything", json!([1, 2]));
    strict.commit("custom", Origin::User, vec![Command::Create(custom)]).unwrap();
}
