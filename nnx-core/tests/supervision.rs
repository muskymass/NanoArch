use nnx_core::schedule::Date;
use nnx_core::supervision::*;

fn hp(text: &str, signed: bool) -> HoldPoint {
    HoldPoint { text: text.into(), signed_by: signed.then(|| "Eng. A".into()), signed_date: signed.then(|| "2026-11-02".into()) }
}

fn approved_mar() -> Submittal {
    Submittal { id: "MAR-1".into(), category: "rebar".into(), description: "Rebar B500".into(), status: Review::Approved, reviewed_by: Some("Eng. A".into()), reviewed_date: Some("2026-11-01".into()), ..Default::default() }
}

fn wir(status: Review, points: Vec<HoldPoint>) -> Wir {
    Wir { id: "WIR-1".into(), stage: "foundation_rebar".into(), element: "FND-1".into(), status, inspector: status.ne(&Review::Submitted).then(|| "Eng. A".into()), decided_date: status.ne(&Review::Submitted).then(|| "2026-11-02".into()), hold_points: points, ..Default::default() }
}

#[test]
fn a_decision_needs_its_reviewer_and_date_and_a_rejection_needs_a_comment() {
    let mut s = approved_mar();
    s.reviewed_by = None;
    let inp = SupervisionInputs { mar: vec![s], ..Default::default() };
    assert!(inp.check().unwrap_err().contains("who reviewed"));
    let mut s = approved_mar();
    s.status = Review::Rejected;
    let inp = SupervisionInputs { mar: vec![s], ..Default::default() };
    assert!(inp.check().unwrap_err().contains("comment"));
    let mut s = approved_mar();
    s.category = "gold".into();
    assert!(SupervisionInputs { mar: vec![s], ..Default::default() }.check().unwrap_err().contains("not a material category"));
}

#[test]
fn a_wir_cannot_be_approved_with_an_unsigned_or_missing_hold_point() {
    let none = SupervisionInputs { wir: vec![wir(Review::Approved, vec![])], ..Default::default() };
    assert!(none.check().unwrap_err().contains("no hold point"));
    let open = SupervisionInputs { wir: vec![wir(Review::Approved, vec![hp("cover checked", true), hp("pour approved", false)])], ..Default::default() };
    assert!(open.check().unwrap_err().contains("pour approved"));
    let ok = SupervisionInputs { wir: vec![wir(Review::Approved, vec![hp("cover checked", true), hp("pour approved", true)])], ..Default::default() };
    ok.check().unwrap();
    // A pending request may carry unsigned points.
    SupervisionInputs { wir: vec![wir(Review::Submitted, vec![hp("cover checked", false)])], ..Default::default() }.check().unwrap();
}

#[test]
fn every_stage_has_hold_point_templates_without_a_figure() {
    for (id, _, _) in WIR_STAGES {
        let t = hold_templates(id);
        assert!(!t.is_empty(), "{id}");
        for (en, ar) in t {
            assert!(!ar.is_empty() && !en.chars().any(|c| c.is_ascii_digit()), "{id}: {en}");
        }
    }
    assert!(hold_templates("nope").is_empty());
}

#[test]
fn a_mir_must_name_an_approved_mar() {
    let mir = |status| Mir { id: "MIR-1".into(), mar: "MAR-1".into(), status, checked_by: Some("Eng. A".into()), quantity: Some(10.0), unit: Some("t".into()), ..Default::default() };
    let mut pending = approved_mar();
    pending.status = Review::Submitted;
    pending.reviewed_by = None;
    pending.reviewed_date = None;
    let bad = SupervisionInputs { mar: vec![pending], mir: vec![mir(Review::Approved)], ..Default::default() };
    assert!(bad.check().unwrap_err().contains("not approved"));
    let missing = SupervisionInputs { mir: vec![mir(Review::Submitted)], ..Default::default() };
    assert!(missing.check().unwrap_err().contains("not a MAR"));
    SupervisionInputs { mar: vec![approved_mar()], mir: vec![mir(Review::Approved)], ..Default::default() }.check().unwrap();
}

#[test]
fn a_lab_result_is_judged_only_against_an_acceptance_value_with_a_source() {
    let cube = |measured: Option<f64>, required: Option<f64>| LabTest {
        id: "CUBE-1".into(),
        kind: LabKind::Cube,
        subject: "Slab L1".into(),
        sampled: Some("2026-10-01".into()),
        tested: Some("2026-10-29".into()),
        measured,
        unit: Some("MPa".into()),
        required,
        required_source: required.map(|_| "Spec 03300 cl. 5".into()),
        design_fc_mpa: Some(30.0),
        ..Default::default()
    };
    assert_eq!(cube(None, Some(30.0)).verdict(), Verdict::Pending);
    assert_eq!(cube(Some(33.0), None).verdict(), Verdict::Unchecked);
    assert_eq!(cube(Some(33.0), Some(30.0)).verdict(), Verdict::Pass);
    assert_eq!(cube(Some(27.0), Some(30.0)).verdict(), Verdict::Fail);
    assert_eq!(cube(Some(27.0), Some(30.0)).age_days(), Some(28));
    let mut no_source = cube(Some(33.0), Some(30.0));
    no_source.required_source = None;
    assert!(SupervisionInputs { lab: vec![no_source], ..Default::default() }.check().unwrap_err().contains("source"));
    let mut backwards = cube(Some(33.0), Some(30.0));
    backwards.tested = Some("2026-09-01".into());
    assert!(SupervisionInputs { lab: vec![backwards], ..Default::default() }.check().unwrap_err().contains("before it was sampled"));
    let at_most = LabTest { rule: Rule::AtMost, ..cube(Some(0.2), Some(0.5)) };
    assert_eq!(at_most.verdict(), Verdict::Pass);
}

#[test]
fn ncr_rfi_variation_progress_and_snag_rules() {
    let ncr = Ncr { id: "NCR-1".into(), date: Some("2026-11-01".into()), description: "honeycomb".into(), closed_date: Some("2026-11-05".into()), ..Default::default() };
    assert!(SupervisionInputs { ncr: vec![ncr], ..Default::default() }.check().unwrap_err().contains("corrective action"));
    let rfi = Rfi { id: "RFI-1".into(), subject: "beam depth".into(), answered: Some("2026-11-05".into()), ..Default::default() };
    assert!(SupervisionInputs { rfi: vec![rfi], ..Default::default() }.check().unwrap_err().contains("answer"));
    let vo = Variation { id: "VO-1".into(), description: "extra".into(), status: VoStatus::Approved, cost_delta: Some(1000.0), days_delta: Some(3), ..Default::default() };
    assert!(SupervisionInputs { variations: vec![vo], ..Default::default() }.check().unwrap_err().contains("owner's reference"));
    let p = |d: &str, a: f64| ProgressEntry { period_end: d.into(), planned_pct: Some(a), actual_pct: Some(a), ..Default::default() };
    assert!(SupervisionInputs { progress: vec![p("2026-11-07", 20.0), p("2026-11-14", 10.0)], ..Default::default() }.check().unwrap_err().contains("cannot go down"));
    assert!(SupervisionInputs { progress: vec![p("2026-11-07", 120.0)], ..Default::default() }.check().unwrap_err().contains("between 0 and 100"));
    assert!(SupervisionInputs { progress: vec![p("2026-11-14", 10.0), p("2026-11-07", 20.0)], ..Default::default() }.check().unwrap_err().contains("increasing"));
    let snag = Snag { id: "S-1".into(), room: "majlis".into(), description: "crack".into(), status: SnagStatus::Verified, fixed_date: Some("2026-11-20".into()), ..Default::default() };
    assert!(SupervisionInputs { snags: vec![snag], ..Default::default() }.check().unwrap_err().contains("verified"));
}

fn full_project() -> SupervisionInputs {
    SupervisionInputs {
        handover: Some(Handover { date: Some("2026-10-01".into()), easting_m: Some(500000.0), northing_m: Some(2900000.0), crs: Some("as on the survey".into()), benchmark_level_m: Some(12.5), benchmark_datum: Some("as on the survey".into()), signatories: vec![Signatory { role: "Contractor".into(), name: "C".into(), signed_date: Some("2026-10-01".into()) }], ..Default::default() }),
        mar: vec![approved_mar()],
        wir: vec![wir(Review::Approved, vec![hp("a", true)])],
        lab: vec![LabTest { id: "T1".into(), kind: LabKind::Cube, subject: "col".into(), sampled: Some("2026-10-01".into()), tested: Some("2026-10-08".into()), measured: Some(22.0), unit: Some("MPa".into()), required: Some(20.0), required_source: Some("spec".into()), ..Default::default() }],
        ncr: vec![Ncr { id: "N1".into(), date: Some("2026-10-02".into()), description: "x".into(), corrective_action: Some("re-pour".into()), closed_by: Some("Eng".into()), closed_date: Some("2026-10-10".into()), ..Default::default() }],
        variations: vec![
            Variation { id: "VO-1".into(), description: "a".into(), cost_delta: Some(1000.0), days_delta: Some(3), status: VoStatus::Approved, approved_date: Some("2026-10-05".into()), source: Some("letter 7".into()) },
            Variation { id: "VO-2".into(), description: "b".into(), cost_delta: Some(-200.0), days_delta: Some(-1), status: VoStatus::Approved, approved_date: Some("2026-10-06".into()), source: Some("letter 8".into()) },
            Variation { id: "VO-3".into(), description: "c".into(), cost_delta: Some(9999.0), days_delta: Some(9), status: VoStatus::Rejected, ..Default::default() },
        ],
        progress: vec![
            ProgressEntry { period_end: "2026-10-07".into(), planned_pct: Some(10.0), actual_pct: Some(8.0), ..Default::default() },
            ProgressEntry { period_end: "2026-10-14".into(), planned_pct: Some(25.0), actual_pct: Some(20.0), ..Default::default() },
        ],
        closeout: Closeout {
            records: CLOSEOUT_ITEMS.iter().map(|c| CloseRecord { item: c.0.into(), done: true, by: Some("Eng".into()), date: Some("2026-11-20".into()), note: None }).collect(),
            warranties: vec![Warranty { item: "Lift".into(), start: Some("2026-01-31".into()), months: Some(13), ..Default::default() }],
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn report_totals_the_approved_variations_and_the_s_curve_only() {
    let inp = full_project();
    inp.check().unwrap();
    let r = report(&inp, &Context { baseline_finish: Date::parse("2027-03-01"), ..Context::default() }, false);
    let kpi = |id: &str| r.kpis.iter().find(|k| k.id == id).unwrap().value.clone();
    assert_eq!(kpi("vo_cost"), "800");
    assert_eq!(kpi("vo_days"), "2");
    let sched = r.tables.iter().find(|t| t.id == "vo_schedule").expect("vo_schedule table");
    assert_eq!(sched.rows[2][1], "2027-03-03");
    assert_eq!(kpi("actual_pct"), "20");
    assert_eq!(kpi("lab_fail"), "0");
    assert_eq!(r.s_curve.len(), 2);
    assert_eq!(r.s_curve[1].variance_pct, Some(-5.0));
    assert!(r.closeout_ready, "{:?}", r.blockers);
    // Jan 31 + 13 months clamps to the end of the shorter month.
    let w = r.tables.iter().find(|t| t.id == "warranties").unwrap();
    assert_eq!(w.rows[0][4], "2027-02-28");
}

#[test]
fn close_out_is_blocked_by_every_open_item() {
    let mut inp = full_project();
    inp.closeout.records[0].done = false;
    inp.ncr[0].closed_date = None;
    inp.lab[0].measured = Some(10.0);
    inp.snags.push(Snag { id: "S-1".into(), room: "kitchen".into(), description: "tile".into(), ..Default::default() });
    inp.wir.push(Wir { id: "WIR-2".into(), stage: "slabs".into(), element: "SL-1".into(), ..Default::default() });
    inp.check().unwrap();
    let r = report(&inp, &Context::default(), false);
    assert!(!r.closeout_ready);
    let all = r.blockers.join("|");
    for needle in ["Taking Over", "NCR N1", "test T1", "Snag S-1", "WIR WIR-2"] {
        assert!(all.contains(needle), "{needle} missing in {all}");
    }
}

#[test]
fn overdue_needs_today_and_unknown_rooms_are_flagged() {
    let mut inp = SupervisionInputs::default();
    inp.ncr.push(Ncr { id: "N1".into(), date: Some("2026-10-01".into()), description: "x".into(), due: Some("2026-10-10".into()), ..Default::default() });
    inp.snags.push(Snag { id: "S-1".into(), room: "ghost".into(), description: "x".into(), ..Default::default() });
    let none = report(&inp, &Context::default(), false);
    assert!(!none.issues.iter().any(|i| i.contains("past its due date")));
    let rooms = vec!["majlis".to_string()];
    let ctx = Context { today: Date::parse("2026-11-24"), room_keys: Some(&rooms), baseline_finish: Date::parse("2027-03-01") };
    let r = report(&inp, &ctx, false);
    assert!(r.issues.iter().any(|i| i.contains("NCR N1") && i.contains("past its due date")));
    assert!(r.issues.iter().any(|i| i.contains("room 'ghost' is not in the plan")));
    let ar = report(&inp, &ctx, true);
    assert!(ar.issues.iter().any(|i| i.contains("ghost")));
}

#[test]
fn html_and_workbook_render_in_both_languages() {
    let inp = full_project();
    let en = report(&inp, &Context::default(), false);
    let ar = report(&inp, &Context::default(), true);
    let h = en.html("Villa <A>", false);
    assert!(h.contains("dir=\"ltr\"") && h.contains("Villa &lt;A&gt;") && h.contains("<svg"));
    let h = ar.html("فيلا", true);
    assert!(h.contains("dir=\"rtl\"") && h.contains("سجلات الإشراف"));
    let wb = ar.workbook(true);
    assert!(wb.sheets.len() >= 12);
    assert!(wb.sheets.iter().all(|s| s.name.chars().count() <= 31));
    let names: std::collections::BTreeSet<_> = wb.sheets.iter().map(|s| s.name.clone()).collect();
    assert_eq!(names.len(), wb.sheets.len());
    assert!(wb.to_bytes().starts_with(b"PK"));
}

#[test]
fn inputs_round_trip_through_json_and_old_projects_load_without_the_field() {
    let inp = full_project();
    let v = serde_json::to_value(&inp).unwrap();
    let back: SupervisionInputs = serde_json::from_value(v).unwrap();
    assert_eq!(inp, back);
    let empty: SupervisionInputs = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(empty, SupervisionInputs::default());
    empty.check().unwrap();
}

#[test]
fn the_issues_plan_marks_rooms_and_lists_what_it_cannot_place() {
    use nnx_core::supervision::{PlanLevel, PlanRoom, Rfi, Snag, SnagStatus, issues_plan_svg};
    let level = PlanLevel { name: "L1".into(), rooms: vec![PlanRoom { key: "bed".into(), x: 0, y: 0, w: 4000, h: 3000 }, PlanRoom { key: "kit".into(), x: 4000, y: 0, w: 3000, h: 3000 }] };
    let mut inp = SupervisionInputs { snags: vec![
        Snag { id: "S1".into(), room: "bed".into(), description: "crack".into(), ..Default::default() },
        Snag { id: "S2".into(), room: "bed".into(), description: "paint".into(), status: SnagStatus::Verified, ..Default::default() },
        Snag { id: "S3".into(), room: "roof".into(), description: "leak".into(), ..Default::default() },
    ], ..Default::default() };
    inp.rfi = vec![
        Rfi { id: "R1".into(), subject: "tile".into(), bim_ref: Some("kit".into()), ..Default::default() },
        Rfi { id: "R2".into(), subject: "beam".into(), bim_ref: Some("COL-3".into()), ..Default::default() },
        Rfi { id: "R3".into(), subject: "done".into(), bim_ref: Some("kit".into()), answered: Some("2026-11-01".into()), ..Default::default() },
    ];
    let svg = issues_plan_svg(std::slice::from_ref(&level), &inp, false).unwrap();
    assert_eq!(svg.matches("<circle").count(), 1, "only the unverified snag of a room on the plan is marked");
    assert!(svg.contains("S3 (roof)") && svg.contains("R2 (COL-3)"), "what cannot be placed is listed");
    assert!(!svg.contains("R3"), "an answered RFI is not shown");
    assert!(issues_plan_svg(&[PlanLevel { name: "x".into(), rooms: vec![] }], &inp, true).is_none());
}
