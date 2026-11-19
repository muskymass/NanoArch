use nnx_core::*;
use serde_json::json;

fn wall() -> Element {
    Element::new("Wall", Discipline::Arc, Source::User).with_prop("length", 4000)
}

/// Deterministic xorshift so the fuzz test needs no extra crate.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[test]
fn ten_thousand_random_ops_undo_to_empty_and_redo_to_same_hash() {
    let mut doc = Document::lenient();
    let empty = doc.model.state_hash();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut committed = 0;

    for i in 0..10_000 {
        let ids: Vec<Id> = doc.model.elements.keys().copied().collect();
        let cmd = match (rng.below(4), ids.is_empty()) {
            (_, true) | (0, _) => {
                let mut el = wall();
                if !ids.is_empty() && rng.below(2) == 0 {
                    el = Element::new("Door", Discipline::Arc, Source::Generated).hosted_by(ids[rng.below(ids.len())]);
                }
                Command::Create(el)
            }
            (1, _) => Command::Delete(ids[rng.below(ids.len())]),
            (2, _) => Command::SetProp { id: ids[rng.below(ids.len())], key: "length".into(), value: Some(json!(i)) },
            _ => Command::SetLocked { id: ids[rng.below(ids.len())], locked: rng.below(2) == 0 },
        };
        let origin = if rng.below(3) == 0 { Origin::Generator } else { Origin::User };
        // Failing commands (deleting a host, generator touching a locked element) are expected.
        if doc.commit("op", origin, vec![cmd]).is_ok() {
            committed += 1;
        }
    }
    assert!(committed > 5_000, "too few successful ops: {committed}");
    let full = doc.model.state_hash();

    while doc.undo() {}
    assert_eq!(doc.model.state_hash(), empty);
    assert!(doc.model.is_empty());

    while doc.redo() {}
    assert_eq!(doc.model.state_hash(), full);
}

#[test]
fn failed_transaction_rolls_back_completely() {
    let mut doc = Document::lenient();
    let a = wall();
    let before = doc.model.state_hash();
    let ghost = Element::new("Wall", Discipline::Arc, Source::User).id;
    let err = doc
        .commit(
            "bad",
            Origin::User,
            vec![
                Command::Create(a.clone()),
                Command::SetProp { id: a.id, key: "height".into(), value: Some(json!(3000)) },
                Command::Delete(ghost),
            ],
        )
        .unwrap_err();
    assert_eq!(err.index, 2);
    assert_eq!(doc.model.state_hash(), before);
    assert!(!doc.can_undo());
}

#[test]
fn generator_cannot_touch_locked_element_but_user_can() {
    let mut doc = Document::lenient();
    let w = wall();
    let id = w.id;
    doc.commit("create", Origin::User, vec![Command::Create(w)]).unwrap();
    doc.commit("lock", Origin::User, vec![Command::SetLocked { id, locked: true }]).unwrap();

    let set = |v| Command::SetProp { id, key: "length".into(), value: Some(json!(v)) };
    assert!(matches!(
        doc.commit("regen", Origin::Generator, vec![set(5000)]).unwrap_err().source,
        CommandError::Locked(_)
    ));
    assert!(doc.commit("regen", Origin::Generator, vec![Command::Delete(id)]).is_err());
    doc.commit("edit", Origin::User, vec![set(4500)]).unwrap();
    assert_eq!(doc.model.get(&id).unwrap().props["length"], json!(4500));
}

#[test]
fn host_integrity_is_enforced() {
    let mut doc = Document::lenient();
    let w = wall();
    let door = Element::new("Door", Discipline::Arc, Source::User).hosted_by(w.id);
    // Door before its wall fails.
    assert!(doc.commit("x", Origin::User, vec![Command::Create(door.clone())]).is_err());
    doc.commit("ok", Origin::User, vec![Command::Create(w.clone()), Command::Create(door.clone())]).unwrap();
    // Wall with a door cannot be deleted; deleting door first then wall works.
    assert!(matches!(
        doc.commit("x", Origin::User, vec![Command::Delete(w.id)]).unwrap_err().source,
        CommandError::HasDependents(_, 1)
    ));
    doc.commit("ok", Origin::User, vec![Command::Delete(door.id), Command::Delete(w.id)]).unwrap();
    assert!(doc.model.is_empty());
    doc.undo();
    assert_eq!(doc.model.len(), 2);
}

#[test]
fn redo_stack_clears_on_new_commit() {
    let mut doc = Document::lenient();
    doc.commit("a", Origin::User, vec![Command::Create(wall())]).unwrap();
    doc.undo();
    assert!(doc.can_redo());
    doc.commit("b", Origin::User, vec![Command::Create(wall())]).unwrap();
    assert!(!doc.can_redo());
    assert_eq!(doc.undo_label(), Some("b"));
}

#[test]
fn save_load_roundtrip_preserves_hash() {
    let mut doc = Document::lenient();
    let w = wall();
    let cmds = vec![
        Command::Create(w.clone()),
        Command::Create(Element::new("Window", Discipline::Arc, Source::Generated).hosted_by(w.id).with_prop("width", 1500)),
        Command::Create(Element::new("Column", Discipline::Str, Source::Generated).with_prop("section", json!({"b": 300, "h": 600}))),
    ];
    doc.commit("seed", Origin::User, cmds).unwrap();

    let dir = std::env::temp_dir().join(format!("nnx-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("villa.nnx");
    persist::save(&doc.model, &path).unwrap();
    persist::save(&doc.model, &path).unwrap(); // overwriting an existing file works
    let loaded = persist::load(&path).unwrap();
    assert_eq!(loaded.state_hash(), doc.model.state_hash());
    assert_eq!(loaded, doc.model);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn bulk_5000_elements_save_and_load_under_one_second() {
    let mut doc = Document::lenient();
    let cmds = (0..5_000).map(|_| Command::Create(wall())).collect();
    doc.commit("bulk", Origin::Generator, cmds).unwrap();
    let path = std::env::temp_dir().join(format!("nnx-bulk-{}.nnx", std::process::id()));
    let t = std::time::Instant::now();
    persist::save(&doc.model, &path).unwrap();
    let loaded = persist::load(&path).unwrap();
    let elapsed = t.elapsed();
    std::fs::remove_file(&path).ok();
    assert_eq!(loaded.len(), 5_000);
    eprintln!("save+load 5000 elements: {elapsed:?}");
    assert!(elapsed.as_secs_f64() < 1.0, "took {elapsed:?}");
    // The whole generated batch undoes as one step.
    doc.undo();
    assert!(doc.model.is_empty());
}

#[test]
fn settings_travel_with_the_model_and_old_files_still_load() {
    use nnx_core::persist::{self, Settings};
    let mut doc = Document::lenient();
    doc.commit("seed", Origin::User, vec![Command::Create(wall())]).unwrap();
    let dir = std::env::temp_dir().join(format!("nnx-settings-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let mut settings = Settings::new();
    settings.insert("title".into(), json!("فيلا تجريبية"));
    settings.insert("labels".into(), json!({ "majlis": "المجلس" }));
    settings.insert("structure".into(), json!({ "dead_kpa": 5.0, "bearing_kpa": null }));
    let with = dir.join("with.nnx");
    persist::save_with_settings(&doc.model, &settings, &with).unwrap();
    let (model, back) = persist::load_with_settings(&with).unwrap();
    assert_eq!(back, settings, "every setting comes back exactly, Arabic text and nulls included");
    assert_eq!(model.state_hash(), doc.model.state_hash(), "settings do not touch the model");
    assert_eq!(persist::load(&with).unwrap().state_hash(), doc.model.state_hash(), "plain load ignores settings");

    // A file written without settings (or by an older version) loads with empty settings.
    let plain = dir.join("plain.nnx");
    persist::save(&doc.model, &plain).unwrap();
    assert!(persist::load_with_settings(&plain).unwrap().1.is_empty());
    // Saving again over the same file replaces the settings instead of mixing them.
    persist::save_with_settings(&doc.model, &Settings::new(), &with).unwrap();
    assert!(persist::load_with_settings(&with).unwrap().1.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dirty_follows_the_saved_state_not_a_counter() {
    let mut doc = Document::lenient();
    assert!(!doc.is_dirty());
    doc.commit("a", Origin::User, vec![Command::Create(wall())]).unwrap();
    assert!(doc.is_dirty());
    doc.mark_saved();
    assert!(!doc.is_dirty());
    doc.undo();
    assert!(doc.is_dirty(), "undoing past the saved state is a change");
    doc.redo();
    assert!(!doc.is_dirty(), "back at the saved state the document is clean again");
}
