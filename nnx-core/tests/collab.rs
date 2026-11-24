//! Collaboration merge and LAN exchange (tracker item 94). Every value is SYNTHETIC test data.

use nnx_core::collab::{self, HostState, Snapshot, apply_merged, merge};
use nnx_core::typed::{Column, element_of};
use nnx_core::*;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

fn col(x: i64, grid: &str) -> Element {
    element_of(&Column { x, y: 0, b: 300, h: 300, grid_ref: grid.into(), axial_service_kn: None, extra: Default::default() }, Source::User)
}

fn base_doc() -> (Document, Vec<Id>) {
    let (a, b, c) = (col(0, "A1"), col(3000, "A2"), col(6000, "A3"));
    let ids = vec![a.id, b.id, c.id];
    let mut doc = Document::default();
    doc.commit("base", Origin::User, vec![Command::Create(a), Command::Create(b), Command::Create(c)]).unwrap_or_else(|e| panic!("{e}"));
    (doc, ids)
}

fn set(doc: &mut Document, id: Id, key: &str, v: serde_json::Value) {
    doc.commit("edit", Origin::User, vec![Command::SetProp { id, key: key.into(), value: Some(v) }]).unwrap_or_else(|e| panic!("{e}"));
}

fn no_choices() -> BTreeMap<String, String> {
    BTreeMap::new()
}

#[test]
fn edits_to_different_elements_and_different_properties_merge_without_a_conflict() {
    let (base_doc, ids) = base_doc();
    let base = base_doc.model.clone();
    let (mut a, mut b) = (Document::new(base.clone()), Document::new(base.clone()));
    set(&mut a, ids[0], "x", json!(100));
    set(&mut b, ids[1], "x", json!(3500));
    set(&mut a, ids[2], "x", json!(6100));
    set(&mut b, ids[2], "gridRef", json!("A3b"));
    let r = merge(&base, ("Ali", &a.model), ("Sara", &b.model), &no_choices()).unwrap();
    assert!(r.conflicts.is_empty() && r.issues.is_empty(), "{:?}", r.conflicts);
    let m = r.model.unwrap();
    assert_eq!(m.get(&ids[0]).unwrap().props["x"], json!(100));
    assert_eq!(m.get(&ids[1]).unwrap().props["x"], json!(3500));
    assert_eq!(m.get(&ids[2]).unwrap().props["x"], json!(6100));
    assert_eq!(m.get(&ids[2]).unwrap().props["gridRef"], json!("A3b"));
    assert_eq!(r.stats.merged_both, 1);
    // The result is the same whichever copy is passed first.
    let r2 = merge(&base, ("Sara", &b.model), ("Ali", &a.model), &no_choices()).unwrap();
    assert_eq!(r2.model.unwrap().state_hash(), m.state_hash());
}

#[test]
fn the_same_edit_on_both_sides_is_kept_once_and_a_different_edit_is_a_conflict_that_needs_a_named_winner() {
    let (d, ids) = base_doc();
    let base = d.model.clone();
    let (mut a, mut b) = (Document::new(base.clone()), Document::new(base.clone()));
    set(&mut a, ids[0], "x", json!(500));
    set(&mut b, ids[0], "x", json!(500));
    assert!(merge(&base, ("Ali", &a.model), ("Sara", &b.model), &no_choices()).unwrap().conflicts.is_empty());

    set(&mut b, ids[0], "x", json!(900));
    let r = merge(&base, ("Ali", &a.model), ("Sara", &b.model), &no_choices()).unwrap();
    assert_eq!(r.conflicts.len(), 1);
    assert!(r.model.is_none(), "no model while a conflict is open");
    let c = &r.conflicts[0];
    assert_eq!(c.path, "props.x");
    assert_eq!(c.base, Some(json!(0)));
    assert_eq!(c.values["Ali"], Some(json!(500)));
    assert_eq!(c.values["Sara"], Some(json!(900)));

    let pick = |who: &str| BTreeMap::from([(c.key.clone(), who.to_string())]);
    let ali = merge(&base, ("Ali", &a.model), ("Sara", &b.model), &pick("Ali")).unwrap().model.unwrap();
    let sara = merge(&base, ("Ali", &a.model), ("Sara", &b.model), &pick("Sara")).unwrap().model.unwrap();
    assert_eq!(ali.get(&ids[0]).unwrap().props["x"], json!(500));
    assert_eq!(sara.get(&ids[0]).unwrap().props["x"], json!(900));
    // Each computer computes the same result from the same choice, whichever side it calls "mine".
    let from_sara = merge(&base, ("Sara", &b.model), ("Ali", &a.model), &pick("Sara")).unwrap().model.unwrap();
    assert_eq!(from_sara.state_hash(), sara.state_hash());
}

#[test]
fn nested_objects_merge_by_key_and_only_the_same_key_conflicts() {
    let (mut d, ids) = base_doc();
    set(&mut d, ids[0], "meta", json!({ "title": "T", "owner": "O", "tags": [1] }));
    let base = d.model.clone();
    let (mut a, mut b) = (Document::new(base.clone()), Document::new(base.clone()));
    set(&mut a, ids[0], "meta", json!({ "title": "T2", "owner": "O", "tags": [1] }));
    set(&mut b, ids[0], "meta", json!({ "title": "T", "owner": "O2", "tags": [1, 2] }));
    let r = merge(&base, ("Ali", &a.model), ("Sara", &b.model), &no_choices()).unwrap();
    assert!(r.conflicts.is_empty());
    assert_eq!(r.model.unwrap().get(&ids[0]).unwrap().props["meta"], json!({ "title": "T2", "owner": "O2", "tags": [1, 2] }));

    set(&mut b, ids[0], "meta", json!({ "title": "T3", "owner": "O2", "tags": [1, 3] }));
    let r = merge(&base, ("Ali", &a.model), ("Sara", &b.model), &no_choices()).unwrap();
    let paths: Vec<&str> = r.conflicts.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, ["props.meta.title"]);
    assert!(paths.contains(&"props.meta.title"), "{paths:?}");
    // Tags: Ali left them alone, so Sara's edit wins there without a conflict.
    assert!(!paths.contains(&"props.meta.tags"), "{paths:?}");
}

#[test]
fn deleting_on_one_side_and_editing_on_the_other_is_a_conflict_and_a_clean_delete_is_not() {
    let (d, ids) = base_doc();
    let base = d.model.clone();
    let (mut a, mut b) = (Document::new(base.clone()), Document::new(base.clone()));
    a.commit("del", Origin::User, vec![Command::Delete(ids[1]), Command::Delete(ids[2])]).unwrap();
    set(&mut b, ids[1], "x", json!(3300));
    let r = merge(&base, ("Ali", &a.model), ("Sara", &b.model), &no_choices()).unwrap();
    assert_eq!(r.conflicts.len(), 1);
    assert_eq!(r.conflicts[0].path, "element");
    assert_eq!(r.conflicts[0].values["Ali"], None);
    let key = r.conflicts[0].key.clone();
    let keep = merge(&base, ("Ali", &a.model), ("Sara", &b.model), &BTreeMap::from([(key.clone(), "Sara".to_string())])).unwrap().model.unwrap();
    assert_eq!(keep.get(&ids[1]).unwrap().props["x"], json!(3300));
    assert!(keep.get(&ids[2]).is_none(), "the untouched column Ali deleted is gone");
    let drop = merge(&base, ("Ali", &a.model), ("Sara", &b.model), &BTreeMap::from([(key, "Ali".to_string())])).unwrap().model.unwrap();
    assert!(drop.get(&ids[1]).is_none());
    assert_eq!(drop.len(), 1);
}

#[test]
fn elements_added_by_either_side_are_kept_and_a_hostless_dependent_is_reported_not_merged() {
    let (d, ids) = base_doc();
    let base = d.model.clone();
    let (mut a, mut b) = (Document::new(base.clone()), Document::new(base.clone()));
    let new_a = col(9000, "A4");
    let new_b = col(12000, "A5");
    a.commit("add", Origin::User, vec![Command::Create(new_a.clone())]).unwrap();
    b.commit("add", Origin::User, vec![Command::Create(new_b.clone())]).unwrap();
    let r = merge(&base, ("Ali", &a.model), ("Sara", &b.model), &no_choices()).unwrap();
    assert_eq!((r.stats.added_by_first, r.stats.added_by_second), (1, 1));
    assert_eq!(r.model.unwrap().len(), 5);

    // Ali deletes a column; Sara hangs a note on it.
    let mut note = Element::new("Note", Discipline::Arc, Source::User);
    note.host_id = Some(ids[0]);
    let mut a2 = Document::new(base.clone());
    let mut b2 = Document::new(base.clone());
    a2.commit("del", Origin::User, vec![Command::Delete(ids[0])]).unwrap();
    b2.commit("note", Origin::User, vec![Command::Create(note.clone())]).unwrap();
    let r = merge(&base, ("Ali", &a2.model), ("Sara", &b2.model), &no_choices()).unwrap();
    assert!(r.model.is_none());
    assert_eq!(r.issues.len(), 1);
    assert!(r.issues[0].contains("hosted by"), "{:?}", r.issues);
}

#[test]
fn bad_input_is_refused_by_name() {
    let (d, ids) = base_doc();
    let m = &d.model;
    assert!(merge(m, ("", m), ("B", m), &no_choices()).unwrap_err().contains("replica name"));
    assert!(merge(m, ("A", m), (" A ", m), &no_choices()).unwrap_err().contains("same name"));
    let stale = BTreeMap::from([(format!("{}|props.x", ids[0]), "A".to_string())]);
    assert!(merge(m, ("A", m), ("B", m), &stale).unwrap_err().contains("matches no conflict"));
    let stranger = BTreeMap::from([("k".to_string(), "Zed".to_string())]);
    assert!(merge(m, ("A", m), ("B", m), &stranger).unwrap_err().contains("not one of the replicas"));
}

#[test]
fn a_merge_is_applied_as_one_undoable_step_and_refused_when_the_document_moved_on() {
    let (d, ids) = base_doc();
    let base = d.model.clone();
    let mut mine = Document::new(base.clone());
    let mut theirs = Document::new(base.clone());
    set(&mut mine, ids[0], "x", json!(10));
    mine.commit("add", Origin::User, vec![Command::Create(col(9000, "A4"))]).unwrap();
    set(&mut theirs, ids[1], "x", json!(3010));
    theirs.commit("del", Origin::User, vec![Command::Delete(ids[2])]).unwrap();
    let exchanged = mine.model.state_hash();
    let merged = merge(&base, ("Ali", &mine.model), ("Sara", &theirs.model), &no_choices()).unwrap().model.unwrap();
    let depth = mine.undo_depth();

    let mut moved = Document::new(mine.model.clone());
    set(&mut moved, ids[0], "x", json!(11));
    assert!(apply_merged(&mut moved, &merged, exchanged, "Merge").unwrap_err().contains("changed after the exchange"));

    let n = apply_merged(&mut mine, &merged, exchanged, "Merge with Sara").unwrap();
    // Only what Sara changed is new to this copy: her edit of one column and her deletion of another.
    assert_eq!(n, 2);
    assert_eq!(mine.undo_depth(), depth + 1);
    assert_eq!(mine.model, merged);
    assert!(mine.undo());
    assert_eq!(mine.model.state_hash(), exchanged);
    assert!(mine.redo());
    assert_eq!(mine.model, merged);
}

fn snap(who: &str, base: &Model, m: &Model) -> Snapshot {
    Snapshot::new(who, base, m).unwrap()
}

#[test]
fn two_computers_swap_copies_over_a_local_connection_and_reach_the_same_model() {
    let (d, ids) = base_doc();
    let base = d.model.clone();
    let (mut a, mut b) = (Document::new(base.clone()), Document::new(base.clone()));
    set(&mut a, ids[0], "x", json!(1));
    set(&mut b, ids[1], "x", json!(3001));
    let mut host = collab::Host::start("session-77", true, 0, Duration::from_secs(20), snap("Ali", &base, &a.model)).unwrap();
    let theirs = collab::connect(&host.addr().to_string(), "session-77", &snap("Sara", &base, &b.model), Duration::from_secs(10)).unwrap();
    assert_eq!(theirs.replica, "Ali");
    let mut got = HostState::Waiting;
    for _ in 0..200 {
        got = host.state();
        if got != HostState::Waiting {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let HostState::Done(sara) = got else { panic!("{got:?}") };
    assert_eq!(sara.replica, "Sara");
    let on_host = merge(&base, ("Ali", &a.model), (&sara.replica, &sara.model), &no_choices()).unwrap().model.unwrap();
    let on_guest = merge(&base, ("Sara", &b.model), (&theirs.replica, &theirs.model), &no_choices()).unwrap().model.unwrap();
    assert_eq!(on_host.state_hash(), on_guest.state_hash());
    host.stop();
}

#[test]
fn a_wrong_code_a_different_starting_file_the_same_name_and_a_short_code_are_all_refused() {
    let (d, ids) = base_doc();
    let base = d.model.clone();
    let host = collab::Host::start("session-77", true, 0, Duration::from_secs(20), snap("Ali", &base, &base)).unwrap();
    let addr = host.addr().to_string();
    let w = Duration::from_secs(10);
    assert!(collab::connect(&addr, "wrong-code", &snap("Sara", &base, &base), w).unwrap_err().contains("wrong session code"));
    // The host is still waiting after a wrong code.
    assert_eq!(host.state(), HostState::Waiting);
    let mut other = Document::new(base.clone());
    set(&mut other, ids[0], "x", json!(5));
    assert!(collab::connect(&addr, "session-77", &snap("Sara", &other.model, &other.model), w).unwrap_err().contains("did not start from the same saved state"));
    drop(host);

    let host2 = collab::Host::start("session-77", true, 0, Duration::from_secs(20), snap("Ali", &base, &base)).unwrap();
    assert!(collab::connect(&host2.addr().to_string(), "session-77", &snap("Ali", &base, &base), w).unwrap_err().contains("each person needs a different name"));
    drop(host2);

    assert!(collab::Host::start("short", true, 0, w, snap("Ali", &base, &base)).err().unwrap().contains("at least 6"));
    assert!(collab::Host::start("session-77", true, 0, Duration::ZERO, snap("Ali", &base, &base)).err().unwrap().contains("between 1 second"));
    assert!(collab::connect("not-an-address", "session-77", &snap("Sara", &base, &base), w).unwrap_err().contains("not an address"));
}

#[test]
fn a_host_nobody_joins_reports_it_after_the_wait() {
    let (d, _) = base_doc();
    let host = collab::Host::start("session-77", true, 0, Duration::from_secs(1), snap("Ali", &d.model, &d.model)).unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    assert!(matches!(host.state(), HostState::Failed(m) if m.contains("no partner joined")));
}
