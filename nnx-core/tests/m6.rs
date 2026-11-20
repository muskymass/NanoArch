//! Milestone 6: cost tiers, tender pack, payment schedule, long-lead procurement, QA/QC and apartment ROI.
//! Every number below is SYNTHETIC test data chosen so the arithmetic can be checked by hand; none is a price, rate or lead time.

use nnx_core::costtier::{self, Basis, CostInputs, CostLine, Tier};
use nnx_core::execution::{Activity, ExecutionInputs, Link, ScheduleSetup, Trigger};
use nnx_core::payment::{self, Milestone, PaymentInputs};
use nnx_core::procure::{self, LongLeadItem, ProcureInputs};
use nnx_core::project::{self, BuildingType, NanoArchProject};
use nnx_core::qaqc::{self, CustomItem, QaInputs, QaRecord, Status};
use nnx_core::qto::{self, QtoOpts};
use nnx_core::roi::{self, CapexBasis, OtherCost, RoiContext, RoiInputs, UnitType};
use nnx_core::schedule::Date;
use nnx_core::tender::{self, TenderInputs, TenderNote, TenderParts};
use nnx_core::typed::{Beam, Column, Slab, Space, Wall, element_of};
use nnx_core::*;

fn near(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol, "{a} vs {b}");
}

fn space(key: &str, func: &str, x: i64) -> Space {
    Space { func: func.into(), requirement_key: key.into(), area: 24.0, target_area: 24.0, x, y: 0, w: 6000, h: 4000, boundary: vec![[x, 0], [x + 6000, 0], [x + 6000, 4000], [x, 4000]], net_area: None, glazing_ratio: None, extra: Default::default() }
}

fn wall(x1: i64, y1: i64, x2: i64, y2: i64, a: &str, b: Option<&str>, exterior: bool) -> Wall {
    Wall { x1, y1, x2, y2, thickness: 200, height: 3000, length: (x2 - x1).abs() + (y2 - y1).abs(), is_exterior: exterior, room_a: a.into(), room_b: b.map(str::to_string), extra: Default::default() }
}

/// Two 6 x 4 m rooms (gross floor area 48 m2), walls, slabs, a beam and a column, and a project element.
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
    doc
}

fn items(doc: &Document) -> Vec<nnx_core::qto::QtoItem> {
    qto::compute(&doc.model, &QtoOpts::default()).items
}

fn line(tier: Tier, phase: &str, basis: Basis, rate: f64) -> CostLine {
    CostLine { tier, phase: phase.into(), basis, rate, label: None, source: "synthetic test".into() }
}

fn d(y: i32, m: u32, day: u32) -> Date {
    Date::new(y, m, day).unwrap()
}

fn acts() -> Vec<Activity> {
    vec![
        Activity { key: "excavate", name: "Excavation".into(), phase: "Substructure", start: d(2027, 1, 10), finish: d(2027, 1, 20) },
        Activity { key: "footing_pour", name: "Footing pour".into(), phase: "Substructure", start: d(2027, 2, 1), finish: d(2027, 2, 5) },
        Activity { key: "column_pour", name: "Column pour".into(), phase: "Superstructure", start: d(2027, 3, 1), finish: d(2027, 3, 10) },
        Activity { key: "column_pour", name: "Column pour (zone 2)".into(), phase: "Superstructure", start: d(2027, 3, 12), finish: d(2027, 3, 20) },
        Activity { key: "doors_windows", name: "Doors and windows".into(), phase: "Finishing", start: d(2027, 6, 1), finish: d(2027, 6, 30) },
    ]
}

// ------------------------------------------------------------------ Feature 41: cost tiers

#[test]
fn a_tier_costs_only_what_the_user_priced_by_phase_and_says_what_is_missing() {
    let doc = plan();
    let all = items(&doc);
    let gross = all.iter().find(|i| i.code == "SPACE.GROSS.AREA").unwrap().quantity;
    near(gross, 48.0, 1e-6);
    let slab_vol = all.iter().find(|i| i.code == "SLAB.VOL").expect("the model has slabs").quantity;

    // Nothing entered: no tier, no number, and the gap is named.
    let none = costtier::compute(&all, &CostInputs::default());
    assert!(none.tiers.is_empty() && none.notes.iter().any(|n| n.contains("no tier selected")));

    let inp = CostInputs {
        selected: Some(Tier::ShellWithMaterials),
        currency: Some("SAR".into()),
        lines: vec![
            line(Tier::ShellWithMaterials, "Substructure", Basis::GrossArea, 100.0),
            line(Tier::ShellWithMaterials, "Superstructure", Basis::Qto { code: "SLAB.VOL".into() }, 1000.0),
            line(Tier::ShellWithMaterials, "Superstructure", Basis::Lump, 500.0),
            line(Tier::CommercialFinish, "Finishing", Basis::GrossArea, 10.0),
        ],
    };
    inp.check().unwrap();
    let rep = costtier::compute(&all, &inp);
    let shell = rep.selected_result().unwrap();
    // The shell scope is two phases: both priced, so it is complete even though the finishing phases are empty.
    assert!(shell.complete && shell.missing_phases.is_empty());
    near(shell.total, 100.0 * 48.0 + 1000.0 * slab_vol + 500.0, 0.01);
    near(shell.phases[0].amount, 4800.0, 0.01);
    near(shell.per_m2.unwrap(), shell.total / 48.0, 0.01);
    // The commercial tier is expected to price all six phases and priced one: PARTIAL, with the five others named.
    let com = rep.tiers.iter().find(|t| t.tier == Tier::CommercialFinish).unwrap();
    assert!(!com.complete);
    assert_eq!(com.missing_phases.len(), 5);
    assert!(rep.notes.iter().any(|n| n.contains("PARTIAL")));
    // Tiers nobody priced do not appear.
    assert!(rep.tiers.iter().all(|t| t.tier != Tier::DeluxeFinish));

    // A quantity the model lacks is listed and left out of the money, never priced as zero silently.
    let bad = CostInputs { selected: Some(Tier::ShellLaborOnly), lines: vec![line(Tier::ShellLaborOnly, "Substructure", Basis::Qto { code: "NOPE.*".into() }, 5.0)], ..Default::default() };
    let r = costtier::compute(&all, &bad);
    let t = r.selected_result().unwrap();
    assert!(t.lines[0].amount.is_none() && t.lines[0].note.as_deref().unwrap().contains("NOPE"));
    assert!(!t.complete && t.total == 0.0 && t.per_m2.is_none());
}

#[test]
fn a_cost_line_without_a_source_a_bad_phase_or_a_negative_rate_is_refused() {
    let ok = line(Tier::DeluxeFinish, "Finishing", Basis::GrossArea, 1.0);
    assert!(CostInputs { lines: vec![CostLine { source: " ".into(), ..ok.clone() }], ..Default::default() }.check().unwrap_err().contains("source"));
    assert!(CostInputs { lines: vec![CostLine { phase: "Roofing".into(), ..ok.clone() }], ..Default::default() }.check().unwrap_err().contains("WBS phase"));
    assert!(CostInputs { lines: vec![CostLine { rate: -1.0, ..ok.clone() }], ..Default::default() }.check().is_err());
    assert!(CostInputs { lines: vec![CostLine { basis: Basis::Qto { code: "".into() }, ..ok }], ..Default::default() }.check().is_err());
    assert_eq!(Tier::ALL.len(), 5);
    assert!(Tier::ALL.iter().all(|t| !t.label().0.is_empty() && !t.label().1.is_empty()));
}

// ------------------------------------------------------------------ Feature 43: payment schedule

fn ms(id: &str, link: Link, trigger: Trigger, pct: Option<f64>, amount: Option<f64>) -> Milestone {
    Milestone { id: id.into(), name: id.to_uppercase(), link, trigger, pct, amount, note: None }
}

fn task(k: &str) -> Link {
    Link::Task { key: k.into() }
}

#[test]
fn milestones_take_their_dates_from_the_schedule_and_the_allocation_is_validated() {
    let a = acts();
    let mut inp = PaymentInputs {
        contract_value: Some(1000.0),
        contract_source: Some("synthetic contract".into()),
        milestones: vec![
            ms("m2", Link::Phase { name: "Superstructure".into() }, Trigger::Finish, Some(40.0), None),
            ms("m1", task("footing_pour"), Trigger::Finish, Some(30.0), None),
            ms("m3", task("doors_windows"), Trigger::Start, Some(30.0), None),
        ],
        ..Default::default()
    };
    inp.check().unwrap();
    let r = payment::compute(&inp, Some(&a), None);
    assert_eq!(r.status, "balanced");
    assert!(r.balanced && r.issues.is_empty(), "{:?}", r.issues);
    // Ordered by date, not by entry; the phase finish is the LATEST finish of its zones.
    let order: Vec<&str> = r.milestones.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(order, ["m1", "m2", "m3"]);
    assert_eq!(r.milestones[0].date.as_deref(), Some("2027-02-05"));
    assert_eq!(r.milestones[1].date.as_deref(), Some("2027-03-20"));
    near(r.milestones[1].amount.unwrap(), 400.0, 1e-9);
    near(r.milestones[2].cumulative_amount.unwrap(), 1000.0, 1e-9);
    near(r.milestones[2].cumulative_pct.unwrap(), 100.0, 1e-9);
    assert!(r.csv().contains("2027-02-05"));

    // Under: 90% allocated. Over: 110%.
    inp.milestones[2].pct = Some(20.0);
    let under = payment::compute(&inp, Some(&a), None);
    assert_eq!(under.status, "under");
    near(under.remaining_amount.unwrap(), 100.0, 1e-9);
    assert!(under.issues.iter().any(|i| i.contains("under-allocated")));
    inp.milestones[2].pct = Some(40.0);
    let over = payment::compute(&inp, Some(&a), None);
    assert_eq!(over.status, "over");
    assert!(over.issues.iter().any(|i| i.contains("over-allocated")));
}

#[test]
fn fixed_amounts_are_checked_against_the_contract_and_without_one_they_cannot_be() {
    let a = acts();
    let inp = PaymentInputs { contract_value: Some(1000.0), contract_source: Some("s".into()), milestones: vec![ms("a", task("excavate"), Trigger::Start, None, Some(250.0)), ms("b", task("excavate"), Trigger::Finish, Some(75.0), None)], ..Default::default() };
    let r = payment::compute(&inp, Some(&a), None);
    assert_eq!(r.status, "balanced");
    near(r.milestones[0].pct.unwrap(), 25.0, 1e-9);
    // No contract value: percentages only can be validated; amounts say so.
    let amounts_only = PaymentInputs { milestones: vec![ms("a", task("excavate"), Trigger::Start, None, Some(250.0))], ..Default::default() };
    let r = payment::compute(&amounts_only, Some(&a), None);
    assert_eq!(r.status, "unknown");
    assert!(r.issues.iter().any(|i| i.contains("contract value")));
    let pct_only = PaymentInputs { milestones: vec![ms("a", task("excavate"), Trigger::Start, Some(60.0), None), ms("b", task("excavate"), Trigger::Finish, Some(40.0), None)], ..Default::default() };
    let r = payment::compute(&pct_only, Some(&a), None);
    assert_eq!(r.status, "balanced");
    assert!(r.milestones.iter().all(|m| m.amount.is_none()), "no amount without a contract value");
    // The BOQ total is the contract only when it is complete.
    let from_boq = PaymentInputs { contract_from_boq: true, milestones: pct_only.milestones.clone(), ..Default::default() };
    assert!(payment::compute(&from_boq, Some(&a), None).issues.iter().any(|i| i.contains("unpriced")));
    near(payment::compute(&from_boq, Some(&a), Some(2000.0)).milestones[0].amount.unwrap() + payment::compute(&from_boq, Some(&a), Some(2000.0)).milestones[1].amount.unwrap(), 2000.0, 1e-9);
    // An activity the schedule lacks, and no schedule at all, are reported by name rather than guessed.
    let missing = PaymentInputs { milestones: vec![ms("x", task("ceiling"), Trigger::Start, Some(100.0), None)], ..Default::default() };
    let r = payment::compute(&missing, Some(&a), None);
    assert!(r.milestones[0].date.is_none() && r.issues.iter().any(|i| i.contains("ceiling")));
    assert!(payment::compute(&missing, None, None).issues.iter().any(|i| i.contains("not been computed")));
}

#[test]
fn a_milestone_with_both_or_neither_a_percentage_and_an_amount_is_refused() {
    let base = |p: Option<f64>, a: Option<f64>| PaymentInputs { milestones: vec![ms("x", task("excavate"), Trigger::Start, p, a)], ..Default::default() };
    assert!(base(Some(10.0), Some(5.0)).check().unwrap_err().contains("not both"));
    assert!(base(None, None).check().is_err());
    assert!(base(Some(0.0), None).check().is_err() && base(Some(101.0), None).check().is_err());
    assert!(base(None, Some(-1.0)).check().is_err());
    assert!(PaymentInputs { milestones: vec![ms("x", task("nope"), Trigger::Start, Some(1.0), None)], ..Default::default() }.check().is_err());
    assert!(PaymentInputs { milestones: vec![ms("x", Link::Phase { name: "Roof".into() }, Trigger::Start, Some(1.0), None)], ..Default::default() }.check().is_err());
    let twice = PaymentInputs { milestones: vec![ms("x", task("excavate"), Trigger::Start, Some(1.0), None), ms("x", task("excavate"), Trigger::Finish, Some(1.0), None)], ..Default::default() };
    assert!(twice.check().unwrap_err().contains("twice"));
    assert!(PaymentInputs { contract_value: Some(10.0), ..Default::default() }.check().unwrap_err().contains("source"));
}

// ------------------------------------------------------------------ Feature 44: long-lead procurement

fn lead(id: &str, cat: &str, link: Link, lead_days: Option<u32>, buffer: Option<u32>) -> LongLeadItem {
    LongLeadItem { id: id.into(), name: id.into(), category: cat.into(), needed_for: link, needed_at: Trigger::Start, lead_days, buffer_days: buffer, source: lead_days.map(|_| "synthetic quotation".to_string()) }
}

#[test]
fn the_order_date_is_the_activity_start_less_lead_time_and_buffer_and_a_passed_date_is_flagged() {
    let a = acts();
    // doors_windows starts 2027-06-01; 100 days lead + 11 days buffer = 111 days before: order by 2027-02-10.
    let inp = ProcureInputs {
        as_of: Some("2027-01-05".into()),
        items: vec![lead("win", "windows", task("doors_windows"), Some(100), Some(11)), lead("lift", "elevator", Link::Phase { name: "Substructure".into() }, Some(30), None), lead("hvac", "hvac", task("column_pour"), None, None)],
    };
    inp.check().unwrap();
    let r = procure::compute(&inp, Some(&a), None, false);
    let win = r.items.iter().find(|i| i.id == "win").unwrap();
    assert_eq!(win.needed_on.as_deref(), Some("2027-06-01"));
    assert_eq!(win.order_by.as_deref(), Some("2027-02-10"));
    assert_eq!(win.status, "ok");
    assert_eq!(win.days_to_order, Some(36));
    assert!(!win.before_project_start);
    // The substructure phase starts 2027-01-10; 30 days earlier is 2026-12-11: before today AND before the project start.
    let lift = r.items.iter().find(|i| i.id == "lift").unwrap();
    assert_eq!(lift.order_by.as_deref(), Some("2026-12-11"));
    assert_eq!(lift.status, "overdue");
    assert!(lift.before_project_start);
    assert_eq!(r.overdue, 1);
    assert_eq!(r.before_project_start, 1);
    assert_eq!(r.project_start.as_deref(), Some("2027-01-10"));
    // No lead time: no date, named.
    let hvac = r.items.iter().find(|i| i.id == "hvac").unwrap();
    assert!(hvac.order_by.is_none() && hvac.status == "unknown");
    assert_eq!(r.unknown, 1);
    // Earliest order date first, undated last.
    assert_eq!(r.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["lift", "win", "hvac"]);
    assert_eq!(r.as_of_source, "entered");
    assert!(r.csv().contains("2027-02-10"));
}

#[test]
fn the_system_date_is_used_only_when_none_is_entered_and_the_elevator_gap_is_reported() {
    let a = acts();
    let inp = ProcureInputs { as_of: None, items: vec![lead("win", "windows", task("doors_windows"), Some(10), None)] };
    let r = procure::compute(&inp, Some(&a), Some(d(2027, 5, 22)), true);
    assert_eq!(r.as_of_source, "system clock");
    assert_eq!(r.items[0].order_by.as_deref(), Some("2027-05-22"));
    assert_eq!(r.items[0].status, "due");
    assert!(r.issues.iter().any(|i| i.contains("elevator")));
    // Without a schedule there is no date and the reason is stated.
    let none = procure::compute(&inp, None, Some(d(2027, 5, 22)), false);
    assert!(none.items[0].order_by.is_none() && none.issues.iter().any(|i| i.contains("not been computed")));
}

#[test]
fn a_lead_time_needs_a_source_and_a_link_must_exist() {
    let mut it = lead("a", "custom", task("excavate"), Some(5), None);
    it.source = None;
    assert!(ProcureInputs { as_of: None, items: vec![it] }.check().unwrap_err().contains("source"));
    assert!(ProcureInputs { as_of: Some("2027-13-40".into()), items: vec![] }.check().is_err());
    assert!(ProcureInputs { as_of: None, items: vec![lead("a", "custom", task("nope"), Some(5), None)] }.check().is_err());
    assert!(ProcureInputs { as_of: None, items: vec![lead("a", "custom", task("excavate"), Some(5), None), lead("a", "custom", task("excavate"), Some(5), None)] }.check().is_err());
}

// ------------------------------------------------------------------ Feature 45: QA/QC

fn rec(item: &str, status: Status, by: Option<&str>, value: Option<&str>) -> QaRecord {
    QaRecord { item: item.into(), status, by: by.map(str::to_string), date: Some("2027-02-01".into()), value: value.map(str::to_string), note: None }
}

#[test]
fn the_checklists_cover_the_stages_state_no_acceptance_limit_and_follow_the_schedule() {
    assert_eq!(qaqc::STAGES.len(), 6);
    for id in ["excavation", "rebar", "waterproofing", "mep_pressure", "mep_continuity", "handover"] {
        assert!(qaqc::stage(id).is_some(), "{id}");
    }
    // Every figure-dependent check asks for the figure to be recorded; no template text carries a digit that could be a limit.
    for s in qaqc::STAGES {
        for i in s.items {
            assert!(!i.en.chars().any(|c| c.is_ascii_digit()), "{} states a number: {}", i.id, i.en);
            assert!(!i.ar.is_empty());
        }
        assert!(nnx_core::schedule::template_keys().iter().any(|(k, _)| *k == s.due_key), "{} due key", s.id);
    }
    let r = qaqc::report(&QaInputs::default(), Some(&acts()));
    assert!(r.stages.iter().all(|s| s.state == "not_started" && s.progress_pct == 0.0));
    // The rebar hold point is the start of the footing pour in the schedule.
    assert_eq!(r.stages.iter().find(|s| s.id == "rebar").unwrap().due.as_deref(), Some("2027-02-01"));
    // An activity absent from the schedule leaves the stage without a due date rather than a made-up one.
    assert!(r.stages.iter().find(|s| s.id == "waterproofing").unwrap().due.is_none());
    assert!(qaqc::report(&QaInputs::default(), None).issues.iter().any(|i| i.contains("not been computed")));
}

#[test]
fn a_stage_passes_only_when_every_check_passed_with_its_recorded_figure_and_any_fail_fails_it() {
    let stage = qaqc::stage("excavation").unwrap();
    let mut records: Vec<QaRecord> = stage.items.iter().map(|i| rec(i.id, Status::Pass, Some("Inspector A"), None)).collect();
    // Passing a check that asks for a figure without recording it leaves the stage open and says why.
    let r = qaqc::report(&QaInputs { records: records.clone(), custom_items: vec![] }, None);
    let s = r.stages.iter().find(|s| s.id == "excavation").unwrap();
    assert_eq!(s.state, "in_progress");
    assert!(r.issues.iter().any(|i| i.starts_with("excavation.level") && i.contains("record the")));
    for rc in &mut records {
        if qaqc::stage("excavation").unwrap().items.iter().any(|i| i.id == rc.item && i.record.is_some()) {
            rc.value = Some("synthetic reading".into());
        }
    }
    let ok = qaqc::report(&QaInputs { records: records.clone(), custom_items: vec![] }, None);
    let s = ok.stages.iter().find(|s| s.id == "excavation").unwrap();
    assert_eq!(s.state, "passed");
    assert_eq!(s.progress_pct, 100.0);
    assert_eq!(ok.passed_stages, 1);
    // One not-applicable check does not block; one fail fails the stage.
    records[0].status = Status::Na;
    let na = qaqc::report(&QaInputs { records: records.clone(), custom_items: vec![] }, None);
    assert_eq!(na.stages[0].state, "passed");
    records[1].status = Status::Fail;
    let failed = qaqc::report(&QaInputs { records, custom_items: vec![] }, None);
    assert_eq!(failed.stages[0].state, "failed");
    assert_eq!(failed.failed_stages, 1);
}

#[test]
fn qa_records_are_validated_custom_checks_join_their_stage_and_the_exports_carry_everything() {
    let bad_item = QaInputs { records: vec![rec("nope.item", Status::Pass, Some("x"), None)], custom_items: vec![] };
    assert!(bad_item.check().is_err());
    assert!(QaInputs { records: vec![rec("rebar.marks", Status::Pass, None, None)], custom_items: vec![] }.check().unwrap_err().contains("who inspected"));
    assert!(QaInputs { records: vec![rec("rebar.marks", Status::Pending, None, None)], custom_items: vec![] }.check().is_ok());
    assert!(QaInputs { records: vec![rec("rebar.marks", Status::Pass, Some("x"), None), rec("rebar.marks", Status::Fail, Some("x"), None)], custom_items: vec![] }.check().unwrap_err().contains("two records"));
    let mut r = rec("rebar.marks", Status::Pass, Some("x"), None);
    r.date = Some("tomorrow".into());
    assert!(QaInputs { records: vec![r], custom_items: vec![] }.check().is_err());
    let dup = CustomItem { id: "rebar.marks".into(), stage: "rebar".into(), text: "t".into(), record: None };
    assert!(QaInputs { records: vec![], custom_items: vec![dup] }.check().is_err());
    assert!(QaInputs { records: vec![], custom_items: vec![CustomItem { id: "c1".into(), stage: "roof".into(), text: "t".into(), record: None }] }.check().is_err());

    let inp = QaInputs { records: vec![rec("c1", Status::Fail, Some("Eng. Test"), Some("12 mm")), rec("rebar.marks", Status::Pass, Some("Eng. Test"), None)], custom_items: vec![CustomItem { id: "c1".into(), stage: "rebar".into(), text: "Check the starter bars".into(), record: Some("starter length".into()) }] };
    inp.check().unwrap();
    let r = qaqc::report(&inp, Some(&acts()));
    let rebar = r.stages.iter().find(|s| s.id == "rebar").unwrap();
    assert!(rebar.items.iter().any(|i| i.id == "c1" && i.custom && i.status == Status::Fail));
    assert_eq!(rebar.state, "failed");
    let html = r.html("Test villa", false);
    assert!(html.contains("Check the starter bars") && html.contains("Eng. Test") && html.contains("12 mm") && html.contains("FAILED") && html.contains("states no acceptance limit"));
    let ar = r.html("فيلا", true);
    assert!(ar.contains("dir=\"rtl\"") && ar.contains("الحفر والأساسات"));
    let wb = r.workbook(false);
    assert_eq!(wb.sheets.len(), 2);
    assert!(wb.sheets[1].rows.len() > 30);
    assert!(wb.to_bytes().starts_with(b"PK"));
}

// ------------------------------------------------------------------ Feature 46: apartment ROI

fn types() -> Vec<UnitType> {
    vec![
        UnitType { name: "1BR".into(), count: 8, annual_rent: Some(30_000.0), rent_source: Some("synthetic survey".into()) },
        UnitType { name: "2BR".into(), count: 12, annual_rent: Some(40_000.0), rent_source: Some("synthetic survey".into()) },
    ]
}

fn apartment_ctx() -> RoiContext {
    RoiContext { is_apartment_building: true, tier_total: None, boq_total: None, floors: Some(4) }
}

#[test]
fn units_capex_rent_yield_and_payback_follow_the_entered_figures() {
    let inp = RoiInputs {
        unit_types: types(),
        capex_basis: CapexBasis::Manual,
        capex_manual: Some(7_200_000.0),
        capex_manual_source: Some("synthetic estimate".into()),
        other_costs: vec![OtherCost { name: "Land".into(), amount: 800_000.0, source: "synthetic deed".into() }],
        ..Default::default()
    };
    inp.check().unwrap();
    let r = roi::compute(&inp, &apartment_ctx());
    assert!(r.active);
    assert_eq!(r.total_units, 20);
    near(r.units_per_floor.unwrap(), 5.0, 1e-9);
    near(r.gross_rent.unwrap(), 8.0 * 30_000.0 + 12.0 * 40_000.0, 1e-6); // 720 000
    near(r.capex.unwrap(), 8_000_000.0, 1e-6);
    near(r.gross_yield_pct.unwrap(), 9.0, 1e-9);
    near(r.payback_years.unwrap(), 8_000_000.0 / 720_000.0, 0.01);
    assert_eq!(r.payback_basis, "gross rent");
    near(r.capex_per_unit.unwrap(), 400_000.0, 1e-6);
    assert!(r.missing.is_empty());

    // Vacancy and operating cost turn the payback to the net income; the gross yield stays gross.
    let net = RoiInputs { vacancy_pct: Some(5.0), vacancy_source: Some("synthetic".into()), annual_opex: Some(100_000.0), opex_source: Some("synthetic".into()), ..inp.clone() };
    let r = roi::compute(&net, &apartment_ctx());
    near(r.net_income.unwrap(), 720_000.0 - 36_000.0 - 100_000.0, 1e-6);
    assert_eq!(r.payback_basis, "net operating income");
    near(r.gross_yield_pct.unwrap(), 9.0, 1e-9);
    near(r.payback_years.unwrap(), 8_000_000.0 / 584_000.0, 0.01);
}

#[test]
fn a_missing_rent_or_an_incomplete_capex_names_the_gap_and_produces_no_yield() {
    let mut inp = RoiInputs { unit_types: types(), capex_basis: CapexBasis::Manual, capex_manual: Some(1_000_000.0), capex_manual_source: Some("s".into()), ..Default::default() };
    inp.unit_types[1].annual_rent = None;
    inp.unit_types[1].rent_source = None;
    let r = roi::compute(&inp, &apartment_ctx());
    assert!(r.gross_rent.is_none() && r.gross_yield_pct.is_none() && r.payback_years.is_none());
    assert!(r.missing.iter().any(|m| m.contains("2BR")));
    // Nothing entered at all.
    let r = roi::compute(&RoiInputs::default(), &apartment_ctx());
    assert!(r.missing.iter().any(|m| m.contains("unit types")) && r.missing.iter().any(|m| m.contains("cost")));
    assert_eq!(r.total_units, 0);
    // The cost-tier basis refuses a PARTIAL tier total and the BOQ basis a partial BOQ.
    let tier = RoiInputs { unit_types: types(), ..Default::default() };
    let r = roi::compute(&tier, &RoiContext { tier_total: Some((500.0, false)), ..apartment_ctx() });
    assert!(r.capex.is_none() && r.missing.iter().any(|m| m.contains("PARTIAL")));
    let r = roi::compute(&tier, &RoiContext { tier_total: Some((500_000.0, true)), ..apartment_ctx() });
    near(r.capex.unwrap(), 500_000.0, 1e-9);
    assert!(r.gross_yield_pct.is_some());
    let boq = RoiInputs { capex_basis: CapexBasis::Boq, ..tier.clone() };
    assert!(roi::compute(&boq, &RoiContext { boq_total: Some((9.0, false)), ..apartment_ctx() }).capex.is_none());
    // Costs above the rent leave no payback.
    let loss = RoiInputs { annual_opex: Some(900_000.0), opex_source: Some("s".into()), ..tier };
    let r = roi::compute(&loss, &RoiContext { tier_total: Some((500_000.0, true)), ..apartment_ctx() });
    assert!(r.payback_years.is_none() && r.missing.iter().any(|m| m.contains("positive annual income")));
}

#[test]
fn the_calculator_runs_for_an_apartment_building_or_when_switched_on() {
    let inp = RoiInputs { unit_types: types(), ..Default::default() };
    let off = roi::compute(&inp, &RoiContext::default());
    assert!(!off.active && off.notes.iter().any(|n| n.contains("apartment")) && off.capex.is_none());
    assert!(roi::compute(&RoiInputs { enabled: true, ..inp }, &RoiContext::default()).active);
    // Inputs that cannot be right are refused: no source, zero count, vacancy above 100.
    let mut t = types();
    t[0].rent_source = None;
    assert!(RoiInputs { unit_types: t, ..Default::default() }.check().unwrap_err().contains("source"));
    let mut t = types();
    t[0].count = 0;
    assert!(RoiInputs { unit_types: t, ..Default::default() }.check().is_err());
    assert!(RoiInputs { vacancy_pct: Some(101.0), vacancy_source: Some("s".into()), ..Default::default() }.check().is_err());
    assert!(RoiInputs { capex_manual: Some(5.0), ..Default::default() }.check().unwrap_err().contains("source"));
    assert!(RoiInputs { other_costs: vec![OtherCost { name: "x".into(), amount: 1.0, source: "".into() }], ..Default::default() }.check().is_err());
}

// ------------------------------------------------------------------ Feature 42: tender pack

#[test]
fn the_tender_pack_combines_the_project_the_priced_boq_and_the_tier_and_lists_every_gap() {
    let mut doc = plan();
    project::update(&mut doc, "inputs", |p| {
        p.meta.title = "Synthetic villa".into();
        p.meta.building_type = Some(BuildingType::Villa);
        p.structure.floors = Some(1);
        p.structure.dead_kpa = Some(2.5);
        p.meta.execution.tender = TenderInputs { scope: Some("Build the shell.".into()), notes: vec![TenderNote { title: "Concrete".into(), text: "As the approved drawings.".into() }], bid_deadline: Some("2027-01-31".into()), validity_days: Some(60), contact: Some("synthetic@example.invalid".into()) };
        p.meta.execution.cost = CostInputs { selected: Some(Tier::ShellWithMaterials), currency: Some("SAR".into()), lines: vec![line(Tier::ShellWithMaterials, "Substructure", Basis::GrossArea, 100.0), line(Tier::ShellWithMaterials, "Superstructure", Basis::Lump, 1000.0)] };
        Ok(())
    })
    .unwrap();
    let p = project::get(&doc.model).unwrap().unwrap();
    let all = items(&doc);
    let mut lib = boq::BoqLib::default();
    lib.project.currency = Some("SAR".into());
    lib.project.name = Some("Synthetic villa".into());
    lib.items.insert("SLAB.VOL".into(), boq::ItemData { spec: Some("Grade per the drawings".into()), spec_source: Some("synthetic".into()), rate: Some(10.0), rate_source: Some("synthetic".into()), waste_pct: None });
    lib.markups.push(boq::Markup { name: "Contingency".into(), pct: 10.0, source: Some("synthetic".into()) });
    let boq_report = boq::compute(&all, &lib);
    let cost = costtier::compute(&all, &p.meta.execution.cost);
    let finishes = finishes::report(&doc.model, &p.meta.finishes, Some(&lib));
    let pack = tender::pack(TenderParts { project: &p, boq: boq_report.clone(), cost, finishes, payment: None });

    assert!(pack.facts.iter().any(|f| f.en == "Dead load (kPa)" && f.source.as_deref() == Some("entered by the user")));
    // Loads that were not entered are listed as missing, never filled in; so are the unpriced BOQ items.
    assert!(pack.missing.iter().any(|m| m.contains("Live load")) && pack.missing.iter().any(|m| m.contains("Soil bearing")));
    assert!(pack.missing.iter().any(|m| m.starts_with("rate of ")));
    assert!(!boq_report.complete);

    let html = pack.html(false);
    for needle in ["Synthetic villa", "Build the shell.", "As the approved drawings.", "2027-01-31", "Grade per the drawings", "Contingency", "PARTIAL", "Shell with materials", "Gaps to complete"] {
        assert!(html.contains(needle), "missing {needle}");
    }
    assert!(pack.html(true).contains("dir=\"rtl\"") && pack.html(true).contains("حزمة المناقصة"));

    // The workbook: summary first, live formulas for the totals, the BOQ rows and the tier.
    let wb = pack.workbook(false);
    let names: Vec<&str> = wb.sheets.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names[0], "Tender");
    for n in ["BOQ", "Cost", "Technical notes"] {
        assert!(names.contains(&n), "{names:?}");
    }
    let bytes = wb.to_bytes();
    assert!(bytes.starts_with(b"PK"));
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("SUM(BOQ!K2:K") && text.contains("ROUND(B") && text.contains("SUM(F2:F"), "formulas present");
    near(pack.boq.subtotal, boq_report.subtotal, 1e-9);
    near(pack.boq.total, boq_report.subtotal * 1.1, 0.01);
}

#[test]
fn a_tender_with_a_bad_deadline_or_an_empty_note_is_refused() {
    assert!(TenderInputs { bid_deadline: Some("31/01/2027".into()), ..Default::default() }.check().is_err());
    assert!(TenderInputs { validity_days: Some(0), ..Default::default() }.check().is_err());
    assert!(TenderInputs { notes: vec![TenderNote { title: "t".into(), text: " ".into() }], ..Default::default() }.check().is_err());
    assert!(TenderInputs::default().check().is_ok());
}

// ------------------------------------------------------------------ the project owns it all

#[test]
fn every_m6_input_lives_in_the_project_so_undo_redo_and_the_file_follow() {
    let mut doc = plan();
    doc.mark_saved();
    assert!(!doc.is_dirty());
    let exec = ExecutionInputs {
        schedule: Some(ScheduleSetup { start: "2027-01-10".into(), workdays: vec![true, true, true, true, false, false, true], holidays: vec!["2027-02-22".into()], zones: 2, rates: Default::default() }),
        cost: CostInputs { selected: Some(Tier::DeluxeFinish), currency: Some("SAR".into()), lines: vec![line(Tier::DeluxeFinish, "Finishing", Basis::GrossArea, 10.0)] },
        payment: PaymentInputs { contract_value: Some(100.0), contract_source: Some("s".into()), milestones: vec![ms("m", task("excavate"), Trigger::Start, Some(100.0), None)], ..Default::default() },
        procurement: ProcureInputs { as_of: Some("2027-01-01".into()), items: vec![lead("w", "windows", task("doors_windows"), Some(60), Some(5))] },
        qaqc: QaInputs { records: vec![rec("rebar.marks", Status::Pass, Some("x"), None)], custom_items: vec![] },
        roi: RoiInputs { enabled: true, unit_types: types(), ..Default::default() },
        tender: TenderInputs { scope: Some("s".into()), ..Default::default() },
    };
    let out = project::update(&mut doc, "Execution", |p| {
        p.meta.execution = exec.clone();
        Ok(())
    })
    .unwrap();
    assert!(out.changed && doc.is_dirty());
    assert_eq!(project::get(&doc.model).unwrap().unwrap().meta.execution, exec);
    // Setting the same values again commits nothing.
    assert!(!project::update(&mut doc, "Execution", |p| {
        p.meta.execution = exec.clone();
        Ok(())
    })
    .unwrap()
    .changed);
    // A refused value changes nothing at all.
    assert!(project::update(&mut doc, "bad", |p| {
        p.meta.execution.payment.milestones[0].pct = Some(500.0);
        Ok(())
    })
    .is_err());
    assert_eq!(project::get(&doc.model).unwrap().unwrap().meta.execution, exec);

    // The file keeps it; undo and redo move through it.
    let path = std::env::temp_dir().join(format!("nanoarch-m6-{}.nnx", std::process::id()));
    persist::save(&doc.model, &path).unwrap();
    let back = persist::load(&path).unwrap();
    std::fs::remove_file(&path).ok();
    assert_eq!(project::get(&back).unwrap().unwrap().meta.execution, exec);
    assert!(doc.undo());
    assert!(!doc.is_dirty());
    assert_eq!(project::get(&doc.model).unwrap().unwrap().meta.execution, ExecutionInputs::default());
    assert!(doc.redo());
    assert_eq!(project::get(&doc.model).unwrap().unwrap().meta.execution.roi.unit_types.len(), 2);
}

#[test]
fn a_project_saved_before_m6_opens_with_empty_execution_inputs() {
    let doc = plan();
    let p = project::get(&doc.model).unwrap().unwrap();
    assert_eq!(p.meta.execution, ExecutionInputs::default());
    assert!(ScheduleSetup { start: "x".into(), ..Default::default() }.check().is_err());
    assert!(ScheduleSetup { start: "2027-01-10".into(), workdays: vec![false; 7], ..Default::default() }.check().is_err());
}
