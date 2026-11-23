//! Milestone 7: the outliner, named snapshots with comparison, and rule overlays.
//! Every value below is SYNTHETIC test data; none is a building-code or municipal value.

use nnx_core::boq::{self, BoqLib, ItemData};
use nnx_core::costtier::{self, Basis, CostInputs, CostLine, Tier};
use nnx_core::outline::{self, OutlineTask};
use nnx_core::overlay::{self, Overlay};
use nnx_core::project::{self, MunicipalLimits, NanoArchProject};
use nnx_core::qto::{self, QtoOpts};
use nnx_core::rules::{self, RuleSet};
use nnx_core::snapshot;
use nnx_core::typed::{Beam, Column, Slab, Space, Wall, element_of};
use nnx_core::*;
use std::collections::BTreeMap;

fn space(key: &str, func: &str, x: i64) -> Space {
    Space { func: func.into(), requirement_key: key.into(), area: 24.0, target_area: 24.0, x, y: 0, w: 6000, h: 4000, boundary: vec![[x, 0], [x + 6000, 0], [x + 6000, 4000], [x, 4000]], net_area: None, glazing_ratio: None, extra: Default::default() }
}

fn wall(x1: i64, y1: i64, x2: i64, y2: i64, a: &str, b: Option<&str>, exterior: bool) -> Wall {
    Wall { x1, y1, x2, y2, thickness: 200, height: 3000, length: (x2 - x1).abs() + (y2 - y1).abs(), is_exterior: exterior, room_a: a.into(), room_b: b.map(str::to_string), extra: Default::default() }
}

fn plan() -> Document {
    let mut cmds = vec![Command::Create(element_of(&space("a", "living", 0), Source::User)), Command::Create(element_of(&space("b", "bathroom", 6000), Source::User))];
    for (x1, y1, x2, y2, a, b, ext) in [(0, 0, 12000, 0, "a", None, true), (12000, 0, 12000, 4000, "b", None, true), (12000, 4000, 0, 4000, "b", None, true), (0, 4000, 0, 0, "a", None, true), (6000, 0, 6000, 4000, "a", Some("b"), false)] {
        cmds.push(Command::Create(element_of(&wall(x1, y1, x2, y2, a, b, ext), Source::User)));
    }
    for (key, x) in [("a", 0), ("b", 6000)] {
        cmds.push(Command::Create(element_of(&Slab { x, y: 0, w: 6000, h: 4000, thickness: 150, kind: "solid".into(), room: key.into(), extra: Default::default() }, Source::User)));
    }
    cmds.push(Command::Create(element_of(&Beam { x1: 6000, y1: 0, x2: 6000, y2: 4000, b: 250, h: 450, length: 4000, extra: Default::default() }, Source::User)));
    cmds.push(Command::Create(element_of(&Column { x: 6000, y: 2000, b: 300, h: 300, grid_ref: "B1".into(), axial_service_kn: None, extra: Default::default() }, Source::User)));
    cmds.push(project::create_command(&NanoArchProject::default()).unwrap());
    let mut doc = Document::default();
    doc.commit("plan", Origin::User, cmds).unwrap_or_else(|e| panic!("{e}"));
    doc.mark_saved();
    doc
}

fn items(doc: &Document) -> Vec<nnx_core::qto::QtoItem> {
    qto::compute(&doc.model, &QtoOpts::default()).items
}

fn take(doc: &Document, id: &str, name: &str, lib: &BoqLib, cost: &CostInputs, at: u64) -> snapshot::Snapshot {
    let all = items(doc);
    snapshot::capture(&doc.model, id, name, Some("note"), at, &BTreeMap::new(), &all, &boq::compute(&all, lib), &costtier::compute(&all, cost))
}

// ------------------------------------------------------------------ Feature 47: outliner

#[test]
fn the_outline_follows_site_levels_spaces_structural_mep_then_execution_and_names_its_elements() {
    let doc = plan();
    let p = project::get(&doc.model).unwrap().unwrap();
    let tasks = vec![OutlineTask { key: "footing_pour".into(), name: "Footing pour".into(), phase: "Substructure".into(), start: "2027-02-01".into(), finish: "2027-02-05".into(), elements: vec![doc.model.elements.values().find(|e| e.kind == "Column").unwrap().id.to_string()] }];
    let root = outline::build(&doc.model, Some(&p), &BTreeMap::new(), Some(&tasks));
    assert_eq!(root.kind, "Site");
    let names: Vec<&str> = root.children.iter().map(|c| c.kind.as_str()).collect();
    assert_eq!(names, ["Level", "Execution"], "one implicit level, then the schedule and execution");
    let level = &root.children[0];
    let groups: Vec<&str> = level.children.iter().map(|c| c.kind.as_str()).collect();
    assert_eq!(groups, ["Spaces", "Envelope", "Structural"], "no MEP elements, so no MEP group");
    assert_eq!(level.children[0].children.len(), 2);
    assert!(level.children[0].children[0].en.contains("24.0 m²"));
    // Every leaf is one element of the model; a group names everything below it; the level names every element.
    let model_ids: std::collections::BTreeSet<String> = doc.model.elements.values().filter(|e| e.kind != "Project").map(|e| e.id.to_string()).collect();
    let level_ids: std::collections::BTreeSet<String> = level.ids.iter().cloned().collect();
    assert_eq!(level_ids, model_ids);
    let leaf = &level.children[2].children[0];
    assert_eq!(leaf.element.as_deref(), Some(leaf.ids[0].as_str()));
    // The schedule lists phases and tasks with the elements each builds, and a tab to open.
    let exe = &root.children[1];
    let sched = exe.children.iter().find(|c| c.key == "sched").unwrap();
    assert_eq!(sched.goto.as_ref().unwrap().tab, "sched");
    assert_eq!(sched.children[0].en, "Substructure");
    assert_eq!(sched.children[0].children[0].ids, tasks[0].elements);
    // QA/QC has its six stages and goes to the Execution tab's QA section.
    let qa = exe.children.iter().find(|c| c.key == "qa").unwrap();
    assert_eq!(qa.children.len(), 6);
    assert_eq!(qa.goto.as_ref().unwrap().section, Some("qa"));
    // Without a computed schedule the node says so.
    let none = outline::build(&doc.model, Some(&p), &BTreeMap::new(), None);
    assert!(none.children[1].children[0].en.contains("not computed"));
}

#[test]
fn execution_records_and_versions_appear_in_the_outline() {
    let mut doc = plan();
    project::update(&mut doc, "inputs", |p| {
        p.meta.execution.cost = CostInputs { selected: Some(Tier::DeluxeFinish), currency: None, lines: vec![CostLine { tier: Tier::DeluxeFinish, phase: "Finishing".into(), basis: Basis::GrossArea, rate: 1.0, label: None, source: "synthetic".into() }] };
        p.meta.execution.roi.enabled = true;
        Ok(())
    })
    .unwrap();
    let p = project::get(&doc.model).unwrap().unwrap();
    let root = outline::build(&doc.model, Some(&p), &BTreeMap::new(), None);
    let exe = &root.children[1];
    let cost = exe.children.iter().find(|c| c.key == "cost").unwrap();
    assert_eq!(cost.children.len(), 1);
    assert!(cost.children[0].en.starts_with("Deluxe finish"));
    assert!(exe.children.iter().any(|c| c.key == "roi"));
    assert!(exe.children.iter().any(|c| c.key == "versions") && exe.children.iter().any(|c| c.key == "overlays"));
}

// ------------------------------------------------------------------ Feature 49: snapshots and diff

fn lib_with(rate: f64) -> BoqLib {
    let mut lib = BoqLib::default();
    lib.items.insert("SLAB.VOL".into(), ItemData { spec: Some("s".into()), spec_source: Some("synthetic".into()), rate: Some(rate), rate_source: Some("synthetic".into()), rate_date: None, waste_pct: None });
    lib
}

#[test]
fn comparing_two_snapshots_reports_added_removed_modified_rooms_quantities_and_cost() {
    let mut doc = plan();
    let cost = CostInputs::default();
    let a = take(&doc, "S1", "Rev A", &lib_with(100.0), &cost, 1_000);
    // Edit: the room 'a' grows (area and size), a second column is added, the beam is deleted.
    let sp = doc.model.elements.values().find(|e| e.kind == "Space" && e.props["requirementKey"] == "a").unwrap().id;
    let beam = doc.model.elements.values().find(|e| e.kind == "Beam").unwrap().id;
    let col = Column { x: 9000, y: 2000, b: 300, h: 300, grid_ref: "C1".into(), axial_service_kn: None, extra: Default::default() };
    doc.commit("edit", Origin::User, vec![Command::SetProp { id: sp, key: "w".into(), value: Some(serde_json::json!(7500)) }, Command::SetProp { id: sp, key: "boundary".into(), value: Some(serde_json::json!([[0, 0], [7500, 0], [7500, 4000], [0, 4000]])) }, Command::SetProp { id: sp, key: "area".into(), value: Some(serde_json::json!(30.0)) }, Command::Create(element_of(&col, Source::User)), Command::Delete(beam)]).unwrap_or_else(|e| panic!("{e}"));
    let b = take(&doc, "S2", "Rev B", &lib_with(120.0), &cost, 2_000);
    let d = snapshot::compare(&a, &b);
    assert_eq!((d.added, d.removed, d.modified), (1, 1, 1));
    let modified = d.elements.iter().find(|e| e.status == "modified").unwrap();
    assert_eq!(modified.id, sp.to_string());
    assert!(modified.changes.iter().any(|c| c.key == "area" && c.from == Some(serde_json::json!(24.0)) && c.to == Some(serde_json::json!(30.0))));
    assert!(d.elements.iter().any(|e| e.status == "added" && e.kind == "Column"));
    assert!(d.elements.iter().any(|e| e.status == "removed" && e.kind == "Beam"));
    let room = d.rooms.iter().find(|r| r.key == "a").unwrap();
    assert_eq!((room.status, room.delta), ("modified", 6.0));
    assert!(d.quantities.iter().any(|q| q.code == "COLUMN.COUNT" && q.delta == 1.0), "{:?}", d.quantities.iter().map(|q| &q.code).collect::<Vec<_>>());
    // Cost: the slab volume is unchanged but its rate went from 100 to 120, so the row amount moved by 20% of itself.
    let row = d.costs.iter().find(|c| c.code == "SLAB.VOL").unwrap();
    assert!((row.delta - row.from.unwrap() * 0.2).abs() < 0.01);
    assert!(d.totals.boq_delta.is_none() && d.notes.iter().any(|n| n.contains("PARTIAL")), "other BOQ items are unpriced, so no total delta is claimed");
    // Identical snapshots: nothing changed.
    let same = snapshot::compare(&b, &b.clone());
    assert!(same.elements.is_empty() && same.rooms.is_empty() && same.quantities.is_empty() && same.costs.is_empty());
}

#[test]
fn a_complete_boq_gives_a_total_delta_and_a_tier_delta_needs_the_same_complete_tier() {
    let mut a = snapshot::Snapshot { id: "S1".into(), name: "A".into(), ..Default::default() };
    let mut b = snapshot::Snapshot { id: "S2".into(), name: "B".into(), ..Default::default() };
    a.costs = snapshot::CostSnap { boq_total: Some(1000.0), boq_complete: true, tier: Some("deluxe_finish".into()), tier_total: Some(500.0), tier_complete: true, ..Default::default() };
    b.costs = snapshot::CostSnap { boq_total: Some(1250.5), boq_complete: true, tier: Some("deluxe_finish".into()), tier_total: Some(450.0), tier_complete: true, ..Default::default() };
    let d = snapshot::compare(&a, &b);
    assert_eq!(d.totals.boq_delta, Some(250.5));
    assert_eq!(d.totals.tier_delta, Some(-50.0));
    b.costs.tier = Some("shell_labor_only".into());
    let d = snapshot::compare(&a, &b);
    assert!(d.totals.tier_delta.is_none() && d.notes.iter().any(|n| n.contains("different cost tiers")));
    b.costs.boq_complete = false;
    assert!(snapshot::compare(&a, &b).totals.boq_delta.is_none());
}

#[test]
fn snapshots_live_in_the_project_with_undo_and_the_file_and_are_validated() {
    let mut doc = plan();
    let a = take(&doc, "S1", "Rev A", &lib_with(100.0), &CostInputs::default(), 5);
    project::update(&mut doc, "Snapshot", |p| {
        p.snapshots.push(a.clone());
        Ok(())
    })
    .unwrap();
    assert!(doc.is_dirty());
    // A second one with the same name (any case) or beyond the limit is refused and nothing changes.
    let dup = snapshot::Snapshot { id: "S2".into(), name: " rev a ".into(), ..a.clone() };
    assert!(project::update(&mut doc, "x", |p| {
        p.snapshots.push(dup.clone());
        Ok(())
    })
    .unwrap_err()
    .contains("already used"));
    assert_eq!(project::get(&doc.model).unwrap().unwrap().snapshots.len(), 1);
    assert_eq!(snapshot::next_id(std::slice::from_ref(&a)), "S2");
    // A snapshot never contains the project element or other snapshots.
    assert!(a.elements.values().all(|e| e.kind != "Project"));
    // The file keeps it, byte for byte.
    let path = std::env::temp_dir().join(format!("nanoarch-m7-{}.nnx", std::process::id()));
    persist::save(&doc.model, &path).unwrap();
    let back = persist::load(&path).unwrap();
    std::fs::remove_file(&path).ok();
    assert_eq!(project::get(&back).unwrap().unwrap().snapshots, vec![a]);
    // Undo removes it, redo brings it back.
    assert!(doc.undo());
    assert!(project::get(&doc.model).unwrap().unwrap().snapshots.is_empty());
    assert!(doc.redo());
    assert_eq!(project::get(&doc.model).unwrap().unwrap().snapshots.len(), 1);
}

#[test]
fn editing_elsewhere_does_not_copy_the_snapshots_into_the_undo_history() {
    let mut doc = plan();
    let a = take(&doc, "S1", "Rev A", &lib_with(100.0), &CostInputs::default(), 5);
    project::update(&mut doc, "Snapshot", |p| {
        p.snapshots.push(a.clone());
        Ok(())
    })
    .unwrap();
    let out = project::update(&mut doc, "execution", |p| {
        p.meta.execution.roi.enabled = true;
        Ok(())
    })
    .unwrap();
    assert!(out.changed);
    // The step the last edit pushed is a SetProp of `meta` only.
    let json = serde_json::to_string(&doc.model.elements.values().find(|e| e.kind == "Project").unwrap().props).unwrap();
    assert!(json.contains("snapshots"));
    assert_eq!(doc.undo_label(), Some("execution"));
}

// ------------------------------------------------------------------ Feature 50: overlays

const BASE: &str = r#"{ "id": "BASE", "edition": "synthetic", "version": "1", "rules": [
  { "id": "door.width", "clause": "T-1", "applies_to": { "kind": "Door" }, "check": "width >= param.min",
    "params": { "min": { "value": 900, "source": "synthetic base" } }, "severity": "error", "message": "door too narrow" } ] }"#;

fn ov(pack: &str) -> Overlay {
    Overlay { id: "ov1".into(), name: "Synthetic municipality".into(), authority: "Synthetic Municipality".into(), document: "Circular 1".into(), date: Some("2027-01-01".into()), enabled: true, pack: pack.into() }
}

const OVERRIDE: &str = r#"{ "id": "MUNI", "edition": "synthetic", "version": "2",
  "overrides": { "door.width": { "min": { "value": 1000, "source": "Circular 1, art. 5" } } },
  "rules": [ { "id": "room.area", "clause": "T-9", "applies_to": { "kind": "Space" }, "check": "area >= param.min",
    "params": { "min": { "value": null, "source": "" } }, "severity": "warning", "message": "small room" } ],
  "limits": { "minSetbackFrontM": { "value": 4, "source": "Circular 1, art. 2" }, "maxCoverageRatio": { "value": 0.6, "source": "Circular 1, art. 3" } } }"#;

#[test]
fn a_review_states_every_override_replacement_and_limit_change_before_anything_is_stored() {
    let (base, _) = overlay::rule_set(Some(BASE), &[]).unwrap();
    let o = ov(OVERRIDE);
    assert!(overlay::problems(&o).is_empty(), "{:?}", overlay::problems(&o));
    let r = overlay::review(&base, &o, &MunicipalLimits::default());
    assert!(r.errors.is_empty());
    assert_eq!(r.label.as_deref(), Some("MUNI@synthetic/2"));
    assert_eq!(r.added, ["room.area"]);
    assert!(r.replaced.is_empty());
    let ch = &r.overridden[0];
    assert_eq!((ch.rule.as_str(), ch.param.as_str(), ch.layer.as_str()), ("door.width", "min", "BASE@synthetic/1"));
    assert_eq!(ch.from.as_ref().unwrap().value, Some(900.0));
    assert_eq!(ch.to.value, Some(1000.0));
    assert_eq!(ch.to.source, "Circular 1, art. 5");
    assert_eq!(r.unset, ["room.area.min"], "a null parameter stays unset: the rule is skipped, not defaulted");
    assert_eq!(r.limit_changes.len(), 2);
    assert!(r.needs_confirm, "an override must be confirmed by the user");
    assert!(r.notes.iter().any(|n| n.contains("built-in code checks are not changed")));
    // The base set was only reviewed against, never changed.
    assert_eq!(base.rule("door.width").unwrap().0.params["min"].value, Some(900.0));
    // An overlay that replaces a rule says so.
    let replace = ov(r#"{ "id": "R", "edition": "e", "version": "1", "rules": [ { "id": "door.width", "clause": "T-2", "applies_to": { "kind": "Door" }, "check": "width >= param.min", "params": { "min": { "value": 800, "source": "s" } }, "severity": "error", "message": "m" } ] }"#);
    let rr = overlay::review(&base, &replace, &MunicipalLimits::default());
    assert_eq!(rr.replaced, [("door.width".to_string(), "BASE@synthetic/1".to_string())]);
    assert!(rr.needs_confirm);
    // A purely additive overlay needs no confirmation.
    let add = ov(r#"{ "id": "A", "edition": "e", "version": "1", "rules": [ { "id": "new.rule", "clause": "T-3", "applies_to": { "kind": "Space" }, "check": "area >= param.min", "params": { "min": { "value": 5, "source": "s" } }, "severity": "info", "message": "m" } ] }"#);
    assert!(!overlay::review(&base, &add, &MunicipalLimits::default()).needs_confirm);
}

#[test]
fn an_overlay_without_a_source_a_clause_or_attribution_is_refused_by_name() {
    let (base, _) = overlay::rule_set(Some(BASE), &[]).unwrap();
    let unsourced = ov(r#"{ "id": "X", "edition": "e", "version": "1", "overrides": { "door.width": { "min": { "value": 1000, "source": "" } } } }"#);
    assert!(overlay::problems(&unsourced).iter().any(|p| p.contains("door.width") && p.contains("no source")));
    assert!(!overlay::review(&base, &unsourced, &MunicipalLimits::default()).errors.is_empty());
    let noclause = ov(r#"{ "id": "X", "edition": "e", "version": "1", "rules": [ { "id": "r", "clause": " ", "applies_to": { "kind": "Door" }, "check": "true", "severity": "info", "message": "m" } ] }"#);
    assert!(overlay::problems(&noclause).iter().any(|p| p.contains("no clause")));
    let mut a = ov(OVERRIDE);
    a.authority = " ".into();
    a.document = "".into();
    assert_eq!(overlay::problems(&a).len(), 2);
    let mut d = ov(OVERRIDE);
    d.date = Some("1/1/2027".into());
    assert!(!overlay::problems(&d).is_empty());
    // Unknown override target and bad JSON come back from the engine, in words.
    let unknown = ov(r#"{ "id": "X", "edition": "e", "version": "1", "overrides": { "nope": { "min": { "value": 1, "source": "s" } } } }"#);
    assert!(overlay::review(&base, &unknown, &MunicipalLimits::default()).errors.iter().any(|e| e.contains("nope")));
    assert!(!overlay::problems(&ov("{ not json")).is_empty());
    // Limits out of range.
    let bad = ov(r#"{ "id": "X", "edition": "e", "version": "1", "limits": { "maxCoverageRatio": { "value": 1.5, "source": "s" } } }"#);
    assert!(overlay::problems(&bad).iter().any(|p| p.contains("out of range")));
    assert!(overlay::check_all(&[ov(OVERRIDE), ov(OVERRIDE)]).unwrap_err().contains("used twice"));
}

#[test]
fn enabled_overlays_layer_on_top_in_order_and_the_check_names_the_layer_of_every_finding() {
    let o = ov(OVERRIDE);
    let mut off = ov(OVERRIDE);
    off.id = "ov2".into();
    off.enabled = false;
    let (rs, reports) = overlay::rule_set(Some(BASE), &[o.clone(), off]).unwrap();
    assert_eq!(rs.layers, ["BASE@synthetic/1", "MUNI@synthetic/2"], "the disabled overlay is not applied");
    assert_eq!(reports.len(), 2);
    assert_eq!(rs.rule("door.width").unwrap().0.params["min"].value, Some(1000.0));
    // Run on a model with a 950 mm door: it passes the base (900) and fails the overlay (1000).
    let mut doc = plan();
    let wall_id = doc.model.elements.values().find(|e| e.kind == "Wall").unwrap().id;
    let door = nnx_core::typed::Door { width: 950, height: 2100, offset_center: 6000, is_egress: false, from: "a".into(), to: "outside".into(), opens_into: "a".into(), extra: Default::default() };
    let mut el = element_of(&door, Source::User);
    el.host_id = Some(wall_id);
    doc.commit("door", Origin::User, vec![Command::Create(el)]).unwrap_or_else(|e| panic!("{e}"));
    let (base_only, _) = overlay::rule_set(Some(BASE), &[]).unwrap();
    assert!(rules::check(&doc.model, &base_only).findings.is_empty());
    let r = rules::check(&doc.model, &rs);
    assert_eq!(r.findings.len(), 1);
    assert_eq!(r.findings[0].layer, "BASE@synthetic/1", "the rule belongs to the base layer; its value came from the overlay");
    // The rule the overlay added has an unset parameter: it is skipped and reported, never defaulted.
    assert!(r.skipped.iter().any(|s| s.rule_id == "room.area"));
    // A broken base pack is an error that names the pack.
    assert!(overlay::rule_set(Some("{"), &[]).unwrap_err().starts_with("rule pack"));
}

#[test]
fn overlay_limits_reach_the_project_only_when_applied_and_are_undoable() {
    let mut doc = plan();
    let o = ov(OVERRIDE);
    project::update(&mut doc, "Overlay", |p| {
        p.overlays.push(o.clone());
        Ok(())
    })
    .unwrap();
    let p = project::get(&doc.model).unwrap().unwrap();
    assert!(p.limits.min_setback_front_m.is_none(), "storing an overlay does not change the limits");
    let mut changed = Vec::new();
    project::update(&mut doc, "Apply overlay limits", |p| {
        changed = overlay::apply_limits(p, &o)?;
        Ok(())
    })
    .unwrap();
    assert_eq!(changed.len(), 2);
    let p = project::get(&doc.model).unwrap().unwrap();
    let f = p.limits.min_setback_front_m.unwrap();
    assert_eq!((f.value, f.source.as_str()), (Some(4.0), "Circular 1, art. 2"));
    assert_eq!(p.limits.max_coverage_ratio.unwrap().value, Some(0.6));
    // Applying again changes nothing and says so.
    let again = project::update(&mut doc, "again", |p| overlay::apply_limits(p, &o).map(|_| ()));
    assert!(again.unwrap_err().contains("already carries"));
    assert!(doc.undo());
    assert!(project::get(&doc.model).unwrap().unwrap().limits.min_setback_front_m.is_none());
    // An invalid overlay never gets stored.
    let mut bad = ov(OVERRIDE);
    bad.authority = "".into();
    assert!(project::update(&mut doc, "x", |p| {
        p.overlays.push(bad.clone());
        Ok(())
    })
    .is_err());
    assert_eq!(overlay::next_id(&[o]), "ov2");
}

#[test]
fn rule_layer_reports_describe_additions_and_overrides_without_changing_the_old_api() {
    let mut rs = RuleSet::default();
    let r = rs.add_layer_reporting(BASE).unwrap();
    assert_eq!(r.added, ["door.width"]);
    assert!(r.replaced.is_empty() && r.overridden.is_empty());
    rs.add_layer(OVERRIDE).unwrap();
    assert_eq!(rs.len(), 2);
    assert_eq!(rs.rule("door.width").unwrap().1, "BASE@synthetic/1");
}
