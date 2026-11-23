//! Material requests by stage. Synthetic numbers, checkable by hand.

use nnx_core::boq::{BoqLib, MatData, MatFactor};
use nnx_core::matreq::{Lead, MatReqInputs, build};
use nnx_core::qto::{QtoItem, Unit};
use nnx_core::schedule::{Date, Schedule, Task};

fn item(code: &str, unit: Unit, q: f64) -> QtoItem {
    QtoItem { code: code.into(), description: code.into(), unit, quantity: q, waste_pct: None, sources: vec![], parts: vec![] }
}

fn task(key: &'static str, phase: &'static str, start: &str, finish: &str, codes: &[&str]) -> Task {
    Task {
        id: key.into(),
        key,
        name: key.into(),
        phase,
        wbs: "1.1".into(),
        zone: None,
        quantity: None,
        unit: None,
        also: vec![],
        duration: 1,
        preds: vec![],
        es: 0,
        ef: 0,
        ls: 0,
        lf: 0,
        float: 0,
        critical: false,
        start: Date::parse(start).unwrap(),
        finish: Date::parse(finish).unwrap(),
        rate_source: "test".into(),
        qto_codes: codes.iter().map(|c| c.to_string()).collect(),
        elements: vec![],
    }
}

fn sched(tasks: Vec<Task>) -> Schedule {
    Schedule { tasks, project_days: 0, finish: Date::parse("2027-06-01").unwrap(), critical: vec![], warnings: vec![] }
}

fn factor(material: &str, unit: &str, per_unit: f64) -> MatFactor {
    MatFactor { material: material.into(), unit: unit.into(), per_unit, source: Some("test".into()) }
}

fn lib() -> BoqLib {
    let mut l = BoqLib::default();
    l.materials.insert("FOOTING.VOL".into(), vec![factor("cement", "bag", 7.0)]);
    l.materials.insert("SLAB.VOL".into(), vec![factor("cement", "bag", 8.0), factor("steel", "kg", 100.0)]);
    l.materials.insert("PLASTER.AREA".into(), vec![factor("plaster mix", "bag", 0.5)]);
    l.materials.insert("WALL.VOL".into(), vec![factor("block", "nr", 12.0)]);
    l.material_data.insert("cement".into(), MatData { spec: Some("synthetic spec".into()), ..Default::default() });
    l
}

fn items() -> Vec<QtoItem> {
    vec![item("FOOTING.VOL", Unit::M3, 10.0), item("SLAB.VOL", Unit::M3, 5.0), item("PLASTER.AREA", Unit::M2, 200.0), item("WALL.VOL", Unit::M3, 3.0), item("ORPHAN.VOL", Unit::M3, 1.0)]
}

fn schedule() -> Schedule {
    sched(vec![
        task("footing_pour", "Substructure", "2027-01-10", "2027-01-20", &["FOOTING.VOL"]),
        task("slab_pour", "Superstructure", "2027-02-01", "2027-02-20", &["SLAB.VOL"]),
        task("plaster", "Finishing", "2027-04-01", "2027-04-30", &["PLASTER.AREA"]),
        // WALL.VOL is worked on in two stages: it cannot be split without inventing a ratio.
        task("wall_a", "Superstructure", "2027-02-05", "2027-02-25", &["WALL.VOL"]),
        task("wall_b", "Finishing", "2027-04-05", "2027-04-10", &["WALL.VOL"]),
    ])
}

#[test]
fn each_stage_holds_only_its_own_items_materials_in_construction_order() {
    let r = build(&items(), &lib(), &schedule(), &MatReqInputs::default(), Date::parse("2027-01-01")).unwrap();
    let names: Vec<&str> = r.stages.iter().map(|s| s.stage.as_str()).collect();
    assert_eq!(names, ["Substructure", "Superstructure", "Finishing"]);
    let q = |stage: &str, mat: &str| r.stages.iter().find(|s| s.stage == stage).unwrap().requests.iter().find(|x| x.material == mat).map(|x| x.quantity);
    assert_eq!(q("Substructure", "cement"), Some(70.0), "10 m3 x 7");
    assert_eq!(q("Superstructure", "cement"), Some(40.0), "5 m3 x 8");
    assert_eq!(q("Superstructure", "steel"), Some(500.0));
    assert_eq!(q("Finishing", "plaster mix"), Some(100.0), "200 m2 x 0.5");
    assert_eq!(q("Substructure", "steel"), None);
    let sub = &r.stages[0];
    assert_eq!((sub.start.as_str(), sub.finish.as_str()), ("2027-01-10", "2027-01-20"));
    assert_eq!(sub.requests[0].spec.as_deref(), Some("synthetic spec"));
}

#[test]
fn shared_and_unscheduled_codes_are_named_and_kept_out_of_every_stage() {
    let r = build(&items(), &lib(), &schedule(), &MatReqInputs::default(), None).unwrap();
    assert_eq!(r.ambiguous.len(), 1);
    assert_eq!(r.ambiguous[0].code, "WALL.VOL");
    assert_eq!(r.ambiguous[0].stages, ["Finishing", "Superstructure"]);
    assert_eq!(r.unscheduled, ["ORPHAN.VOL"]);
    assert!(r.stages.iter().all(|s| s.requests.iter().all(|x| x.material != "block")));
    let block = r.unassigned.iter().find(|m| m.material == "block").unwrap();
    assert_eq!(block.quantity, 36.0, "3 m3 x 12, listed apart");
    assert!(r.issues.iter().any(|i| i.contains("WALL.VOL")));
}

#[test]
fn stages_plus_unassigned_add_up_to_the_project_material_list() {
    let whole = nnx_core::boq::compute(&items(), &lib());
    let r = build(&items(), &lib(), &schedule(), &MatReqInputs::default(), None).unwrap();
    for m in &whole.materials {
        let staged: f64 = r.stages.iter().flat_map(|s| &s.requests).filter(|x| x.material == m.material && x.unit == m.unit).map(|x| x.quantity).sum();
        let apart: f64 = r.unassigned.iter().filter(|x| x.material == m.material && x.unit == m.unit).map(|x| x.quantity).sum();
        assert!((staged + apart - m.quantity).abs() < 1e-6, "{}: {} + {} != {}", m.material, staged, apart, m.quantity);
    }
}

#[test]
fn order_by_is_stage_start_minus_lead_minus_buffer_and_overdue_is_judged_against_as_of() {
    let mut inp = MatReqInputs { as_of: Some("2027-01-20".into()), buffer_days: Some(5), ..Default::default() };
    inp.lead.insert("cement".into(), Lead { days: 30, source: "supplier quotation".into() });
    let r = build(&items(), &lib(), &schedule(), &inp, Date::parse("2020-01-01")).unwrap();
    assert_eq!((r.as_of.as_deref(), r.as_of_source), (Some("2027-01-20"), "entered"));
    let cement = |stage: &str| r.stages.iter().find(|s| s.stage == stage).unwrap().requests.iter().find(|x| x.material == "cement").unwrap();
    // Substructure starts 2027-01-10: minus 30 days minus 5 = 2026-12-06, 45 days before as_of.
    assert_eq!(cement("Substructure").order_by.as_deref(), Some("2026-12-06"));
    assert_eq!(cement("Substructure").days_left, Some(-45));
    assert!(cement("Substructure").overdue);
    // Superstructure starts 2027-02-01: 2026-12-28, 23 days before as_of.
    assert_eq!(cement("Superstructure").order_by.as_deref(), Some("2026-12-28"));
    // A material with no lead time gets no date and is named.
    let steel = r.stages[1].requests.iter().find(|x| x.material == "steel").unwrap();
    assert!(steel.order_by.is_none() && !steel.overdue);
    assert!(r.issues.iter().any(|i| i.contains("no lead time entered for steel")));
    assert!(r.csv().contains("Substructure,2027-01-10,cement,bag,70.000,synthetic spec,30,supplier quotation,2026-12-06,-45,true"));
}

#[test]
fn the_system_date_is_used_only_when_as_of_is_unset_and_is_said_so() {
    let mut inp = MatReqInputs::default();
    inp.lead.insert("cement".into(), Lead { days: 0, source: "s".into() });
    let r = build(&items(), &lib(), &schedule(), &inp, Date::parse("2027-01-12")).unwrap();
    assert_eq!(r.as_of_source, "system clock");
    assert_eq!(r.buffer_days, 0);
    assert_eq!(r.stages[0].requests[0].days_left, Some(-2), "order date 2027-01-10 against 2027-01-12");
}

#[test]
fn bad_inputs_are_rejected_by_name() {
    let run = |inp: MatReqInputs| build(&items(), &lib(), &schedule(), &inp, None).unwrap_err();
    let lead = |m: &str, days: u32, src: &str| {
        let mut i = MatReqInputs::default();
        i.lead.insert(m.into(), Lead { days, source: src.into() });
        i
    };
    assert!(run(lead("cement", 10, " ")).contains("needs its source"));
    assert!(run(lead("cement", 4000, "s")).contains("3650"));
    assert!(run(lead("cemnt", 10, "s")).contains("'cemnt'"), "a misspelt material is not silently ignored");
    assert!(run(MatReqInputs { as_of: Some("2027-02-30".into()), ..Default::default() }).contains("not a real date"));
    assert!(run(MatReqInputs { buffer_days: Some(9999), ..Default::default() }).contains("buffer"));
    assert!(serde_json::from_str::<MatReqInputs>(r#"{"lead":{},"extra":1}"#).is_err());
}

#[test]
fn a_schedule_with_no_matching_codes_yields_no_stage_instead_of_a_guess() {
    let r = build(&items(), &lib(), &sched(vec![]), &MatReqInputs::default(), None).unwrap();
    assert!(r.stages.is_empty());
    assert_eq!(r.unscheduled.len(), 5);
    assert!(r.unassigned.iter().any(|m| m.material == "cement"));
}
