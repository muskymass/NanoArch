//! Naming a room and stating its use. SYNTHETIC values.

use nnx_core::project::{NanoArchProject, ROOM_USES, name_room};
use nnx_core::typed::{Space, element_of};
use nnx_core::*;

fn space(key: &str, x: i64, part_of: Option<&str>) -> Element {
    let mut extra = serde_json::Map::new();
    if let Some(of) = part_of {
        extra.insert("partOf".into(), serde_json::json!(of));
    }
    let s = Space { func: "room".into(), requirement_key: key.into(), area: 20.0, target_area: 20.0, x, y: 0, w: 4000, h: 5000, boundary: vec![[x, 0], [x + 4000, 0], [x + 4000, 5000], [x, 5000]], net_area: None, glazing_ratio: None, extra };
    element_of(&s, Source::User)
}

fn setup() -> (Document, Id, Id, Id) {
    let mut doc = Document::default();
    let (a, part, b) = (space("R1", 0, None), space("R1-2", 4000, Some("R1")), space("R2", 8000, None));
    let ids = (a.id, part.id, b.id);
    let project = element_of(&NanoArchProject::default(), Source::User);
    doc.commit("rooms", Origin::User, vec![Command::Create(project), Command::Create(a), Command::Create(part), Command::Create(b)]).unwrap();
    (doc, ids.0, ids.1, ids.2)
}

fn func(doc: &Document, id: Id) -> String {
    doc.model.get(&id).unwrap().typed::<Space>().unwrap().func
}

fn label(doc: &Document, key: &str) -> Option<String> {
    nnx_core::project::find(&doc.model).unwrap().typed::<NanoArchProject>().unwrap().labels.get(key).cloned()
}

#[test]
fn a_name_and_a_use_reach_every_part_of_the_room_in_one_undo_step() {
    let (mut doc, a, part, b) = setup();
    assert!(name_room(&mut doc, a, Some("  حمام الضيوف "), Some("bathroom")).unwrap());
    assert_eq!(label(&doc, "R1").as_deref(), Some("حمام الضيوف"));
    assert_eq!((func(&doc, a), func(&doc, part), func(&doc, b)), ("bathroom".into(), "bathroom".into(), "room".into()));
    // Nothing to change: no undo step is added.
    assert!(!name_room(&mut doc, a, Some("حمام الضيوف"), Some("bathroom")).unwrap());
    assert!(doc.undo());
    assert_eq!(label(&doc, "R1"), None);
    assert_eq!((func(&doc, a), func(&doc, part)), ("room".into(), "room".into()));
}

#[test]
fn only_what_is_given_changes_an_empty_name_removes_it_and_a_part_names_its_room() {
    let (mut doc, a, part, _) = setup();
    name_room(&mut doc, part, Some("Hall"), None).unwrap();
    assert_eq!(label(&doc, "R1").as_deref(), Some("Hall"));
    assert_eq!(label(&doc, "R1-2"), None);
    name_room(&mut doc, a, None, Some("hall")).unwrap();
    assert_eq!(label(&doc, "R1").as_deref(), Some("Hall"), "the name stays");
    name_room(&mut doc, a, Some(""), None).unwrap();
    assert_eq!(label(&doc, "R1"), None);
    assert_eq!(func(&doc, a), "hall", "the use stays");
}

#[test]
fn what_is_not_a_room_a_use_or_a_name_is_refused_and_nothing_changes() {
    let (mut doc, a, _, _) = setup();
    let long = "x".repeat(81);
    for (id, name, f, why) in [
        (Id::nil(), Some("x"), None, "naming a room: that element is not a room"),
        (a, None, Some("garage-ish"), "naming a room: 'garage-ish' is not a known room use"),
        (a, Some(long.as_str()), None, "naming a room: a name is at most 80 characters"),
    ] {
        let e = name_room(&mut doc, id, name, f).unwrap_err();
        assert_eq!(e, why);
        assert!(nnx_core::msgs::localize(&e).1, "no Arabic for: {e}");
    }
    assert_eq!(label(&doc, "R1"), None);
    assert_eq!(func(&doc, a), "room");
    assert!(ROOM_USES.contains(&"room") && ROOM_USES.contains(&"bathroom"));
}
