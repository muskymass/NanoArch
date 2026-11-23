//! Drain stacks between levels: a wet room above the lowest level gets a branch, a vertical stack that keeps its distance from
//! the columns, and a drain from the stack's foot to the outside. Every input is SYNTHETIC test data.

use nnx_core::mep::{self, MepInputs};
use nnx_core::typed::{Column, Level, Space, element_of};
use nnx_core::*;
use serde_json::{Value, json};

/// Ground floor: a 6 x 4 m living room. First floor: a 6 x 4 m bathroom above it. A 300 mm column stands beside the bathroom's
/// centre, at (3000, 2200), just beside the bathroom's centre (3000, 2000), on both levels.
fn building() -> Document {
    let mut doc = Document::default();
    let l0 = element_of(&Level { name: "Ground".into(), index: 0, extra: Default::default() }, Source::User);
    let l1 = element_of(&Level { name: "First".into(), index: 1, extra: Default::default() }, Source::User);
    let (i0, i1) = (l0.id, l1.id);
    let mut cmds = vec![Command::Create(l0), Command::Create(l1)];
    for (key, func, level) in [("living", "living", i0), ("wc", "bathroom", i1)] {
        let s = Space { func: func.into(), requirement_key: key.into(), area: 24.0, target_area: 24.0, x: 0, y: 0, w: 6000, h: 4000, boundary: vec![[0, 0], [6000, 0], [6000, 4000], [0, 4000]], net_area: None, glazing_ratio: None, extra: Default::default() };
        let mut el = element_of(&s, Source::User);
        el.level_id = Some(level);
        cmds.push(Command::Create(el));
    }
    for level in [i0, i1] {
        let mut el = element_of(&Column { x: 3000, y: 2200, b: 300, h: 300, grid_ref: "A1".into(), axial_service_kn: None, extra: Default::default() }, Source::User);
        el.level_id = Some(level);
        cmds.push(Command::Create(el));
    }
    doc.commit("building", Origin::User, cmds).unwrap_or_else(|e| panic!("{e}"));
    doc
}

fn inputs() -> Value {
    json!({
        "plumbing": {"catalog": {"wc": {"flow_lps": 0.1, "dfu": 4}}, "fixtures_by_func": {"bathroom": {"wc": 1}},
            "simultaneity": 0.5, "max_velocity_mps": 2,
            "pipes": [{"name": "PPR 20", "inner_mm": 16}, {"name": "PPR 25", "inner_mm": 21}],
            "drain_table": [{"max_dfu": 3, "dia_mm": 40}, {"max_dfu": 8, "dia_mm": 75}]},
        "routing": {"drain_slope_pct": 1.5, "drain_invert_mm": 400, "stack_offset_mm": 1500, "stack_clearance_mm": 50}
    })
}

fn parse(v: &Value) -> MepInputs {
    serde_json::from_value(v.clone()).unwrap()
}

#[test]
fn a_wet_room_upstairs_gets_a_branch_a_stack_clear_of_the_column_and_a_drain_to_the_outside() {
    let doc = building();
    let rep = mep::compute(&doc.model, &parse(&inputs()));
    assert!(rep.missing.is_empty(), "{:?}", rep.missing);
    assert!(rep.warnings.iter().all(|w| !w.contains("not routed")), "{:?}", rep.warnings);

    let stack: Vec<_> = rep.runs.iter().filter(|r| r.role == "stack").collect();
    assert_eq!(stack.len(), 1, "one level below the bathroom, so one vertical run: {stack:?}");
    let s = stack[0];
    assert_eq!((s.system.as_str(), s.room.as_deref(), s.width), ("drain", Some("wc"), 75));
    assert_eq!((s.p1[0], s.p1[1]), (s.p2[0], s.p2[1]), "a stack is vertical");
    let ids: Vec<_> = nnx_core::building::levels(&doc.model).into_iter().map(|(_, id)| id).collect();
    assert_eq!(s.level, Some(ids[0]), "the run belongs to the level it crosses");
    assert_eq!(s.p2[2], -400, "it ends at the invert");

    // The stack (75 mm pipe) keeps 50 mm from the 300 mm column at (3000, 2200): its centre is outside the 150 + 38 + 50 box.
    let (sx, sy) = (s.p1[0], s.p1[1]);
    let clear = |x: i64, y: i64| (x - 3000).abs() >= 150 + 38 + 50 || (y - 2200).abs() >= 150 + 38 + 50;
    assert!(clear(sx, sy), "stack at ({sx}, {sy}) is too close to the column");
    assert!((0..=6000).contains(&sx) && (0..=4000).contains(&sy), "the stack stands inside the bathroom");

    // The branch starts at the room's centre, at the invert depth, and falls by the slope along its legs to the stack.
    let branch: Vec<_> = rep.runs.iter().filter(|r| r.role == "branch").collect();
    assert!(!branch.is_empty(), "the centre is within the clearance of the column, so the stack is elsewhere and a branch is needed");
    assert_eq!(branch[0].p1, [3000, 2000, -400]);
    assert_eq!(branch[0].level, Some(ids[1]));
    let last = branch.last().unwrap();
    assert_eq!((last.p2[0], last.p2[1]), (sx, sy));
    let len: i64 = branch.iter().map(|b| (b.p2[0] - b.p1[0]).abs() + (b.p2[1] - b.p1[1]).abs()).sum();
    let fall = branch.iter().map(|b| b.p1[2] - b.p2[2]).sum::<i64>();
    assert!((fall as f64 - len as f64 * 1.5 / 100.0).abs() <= branch.len() as f64, "fall {fall} for {len} mm");
    // The branch legs do not run through the column either.
    for b in &branch {
        let (x0, x1, y0, y1) = (b.p1[0].min(b.p2[0]), b.p1[0].max(b.p2[0]), b.p1[1].min(b.p2[1]), b.p1[1].max(b.p2[1]));
        let hit = x0 - 38 < 3150 && 2850 < x1 + 38 && y0 - 38 < 2350 && 2050 < y1 + 38;
        assert!(!hit, "branch {b:?} runs through the column");
    }
    // The top of the stack is the end of the branch: 3000 mm (the storey) above the ground floor, less the fall.
    assert_eq!(s.p1[2], 3000 + last.p2[2]);

    // The foot of the stack drains to a stack outside the building on the ground floor.
    let foot: Vec<_> = rep.runs.iter().filter(|r| r.role == "drain" && r.room.as_deref() == Some("wc")).collect();
    assert_eq!(foot.len(), 1);
    assert_eq!((foot[0].level, foot[0].p1), (Some(ids[0]), [sx, sy, -400]));
    assert!(foot[0].p2[2] < -400, "it falls");
    assert!(rep.warnings.iter().any(|w| w.contains("passes through room(s) on the levels below: living")), "{:?}", rep.warnings);
}

#[test]
fn without_a_stack_clearance_the_upper_room_is_not_routed_and_the_missing_name_is_listed() {
    let doc = building();
    let mut i = inputs();
    i["routing"].as_object_mut().unwrap().remove("stack_clearance_mm");
    let rep = mep::compute(&doc.model, &parse(&i));
    assert!(rep.runs.iter().all(|r| r.room.as_deref() != Some("wc")), "nothing is invented for the bathroom");
    let m = &rep.missing["routing.drainage.stacks"];
    assert!(m[0].contains("routing.stack_clearance_mm") && m[0].contains("wc"), "{m:?}");
}

#[test]
fn a_room_where_every_point_is_blocked_is_reported_and_not_routed() {
    let doc = building();
    let mut i = inputs();
    // A clearance so large that no point of a 6 x 4 m room keeps it from the column.
    i["routing"]["stack_clearance_mm"] = json!(5000);
    let rep = mep::compute(&doc.model, &parse(&i));
    assert!(rep.runs.iter().all(|r| r.room.as_deref() != Some("wc")));
    assert!(rep.warnings.iter().any(|w| w.contains("stack of room 'wc' is not routed")), "{:?}", rep.warnings);
}

#[test]
fn the_runs_are_valid_model_elements_and_apply_in_one_undoable_step() {
    let mut doc = building();
    let rep = mep::compute(&doc.model, &parse(&inputs()));
    let (created, _) = mep::apply_with_runs(&mut doc, &rep.points, &rep.runs).unwrap();
    assert!(created >= rep.runs.len(), "every stack run is accepted by the model's validation");
    assert!(doc.undo(), "the whole step is undone");
    assert!(doc.model.elements.values().all(|e| e.kind != "MepRun"));
}
