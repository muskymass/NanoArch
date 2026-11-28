//! A raft and hourdi slabs declared on a project: their quantities come from the walls, and nothing is assumed.

use nnx_core::project::NanoArchProject;
use nnx_core::qto::{self, QtoOpts};
use nnx_core::substruct::{Declared, FoundingIn, HourdiIn, RaftIn, Soil};
use nnx_core::typed::{Wall, element_of};
use nnx_core::*;

/// A 10 x 8 m box (outer faces) of 250 mm walls on their centre lines, holding the given declaration. SYNTHETIC values.
fn boxed(declared: Declared) -> Document {
    let wall = |x1: i64, y1: i64, x2: i64, y2: i64| {
        let w = Wall { x1, y1, x2, y2, thickness: 250, height: 3000, length: (x2 - x1) + (y2 - y1), is_exterior: true, room_a: String::new(), room_b: None, extra: Default::default() };
        Command::Create(element_of(&w, Source::User))
    };
    let mut project = NanoArchProject::default();
    project.meta.declared = declared;
    let mut doc = Document::default();
    let cmds = vec![wall(125, 125, 9875, 125), wall(125, 7875, 9875, 7875), wall(125, 125, 125, 7875), wall(9875, 125, 9875, 7875), Command::Create(element_of(&project, Source::User))];
    doc.commit("box", Origin::User, cmds).unwrap();
    doc
}

fn quantity(doc: &Document, code: &str) -> Option<f64> {
    qto::compute(&doc.model, &QtoOpts::default()).items.iter().find(|i| i.code == code).map(|i| i.quantity)
}

#[test]
fn a_raft_and_hourdi_slabs_are_measured_from_the_walls() {
    let raft = RaftIn { top_above_ground_mm: Some(600.0), thickness_mm: Some(800.0), projection_mm: Some(500.0) };
    let hourdi = HourdiIn { total_mm: Some(300.0), topping_mm: Some(70.0), rib_width_mm: Some(150.0), rib_spacing_mm: Some(550.0), block_length_mm: Some(200.0) };
    let doc = boxed(Declared { raft: Some(raft), hourdi: Some(hourdi), ..Default::default() });
    // Raft: 11 x 9 = 99 m2, 79.2 m3 at 0.8 m, sides 40 m x 0.8 = 32 m2, 0.2 m of it below the ground = 19.8 m3.
    for (code, want) in [("RAFT.AREA", 99.0), ("RAFT.CONC", 79.2), ("RAFT.FORM", 32.0), ("RAFT.EXCAV", 19.8)] {
        assert_eq!(quantity(&doc, code), Some(want), "{code}");
    }
    // Slab: 80 m2; 70 + 230 x 150 / 550 = 132.73 mm of concrete = 10.62 m3; 80 / (0.55 x 0.2) = 727.3, so 728 blocks.
    for (code, want) in [("SLAB.FORM", 80.0), ("SLAB.VOL", 10.62), ("SLAB.BLOCKS", 728.0)] {
        assert_eq!(quantity(&doc, code), Some(want), "{code}");
    }
}

#[test]
fn what_lacks_an_input_is_not_measured_and_the_input_is_named() {
    let raft = RaftIn { top_above_ground_mm: Some(600.0), ..Default::default() };
    let doc = boxed(Declared { raft: Some(raft), hourdi: Some(HourdiIn::default()), ..Default::default() });
    let q = qto::compute(&doc.model, &QtoOpts::default());
    assert!(q.items.iter().all(|i| !i.code.starts_with("RAFT.") && i.code != "SLAB.VOL" && i.code != "SLAB.BLOCKS"), "{:?}", q.items);
    // The footprint of a floor needs no input.
    assert_eq!(quantity(&doc, "SLAB.FORM"), Some(80.0));
    for name in ["raft thickness", "raft projection", "slab depth", "block length"] {
        let warning = q.warnings.iter().find(|w| w.contains(name)).unwrap_or_else(|| panic!("{name}: {:?}", q.warnings));
        assert!(nnx_core::msgs::localize(warning).1, "no Arabic for: {warning}");
    }
    for note in &q.warnings {
        if note.contains("raft") || note.contains("hourdi") || note.starts_with("reinforcement is not computed") {
            assert!(nnx_core::msgs::localize(note).1, "no Arabic for: {note}");
        }
    }
}

#[test]
fn a_slab_covers_the_floor_above_too_and_the_raft_reports_what_overhangs_it() {
    // A 10 x 8 m ground floor under an upper floor that reaches 2 m further: SYNTHETIC rooms on two levels.
    let level = |name: &str, i: i64| element_of(&nnx_core::typed::Level { name: name.into(), index: i, extra: Default::default() }, Source::User);
    let room = |key: &str, w: i64, on: nnx_core::model::Id| {
        let area = w as f64 * 8.0 / 1000.0;
        let s = nnx_core::typed::Space { func: "other".into(), requirement_key: key.into(), area, target_area: area, x: 0, y: 0, w, h: 8000, boundary: vec![[0, 0], [w, 0], [w, 8000], [0, 8000]], net_area: None, glazing_ratio: None, extra: Default::default() };
        let mut e = element_of(&s, Source::User);
        e.level_id = Some(on);
        Command::Create(e)
    };
    let (ground, first) = (level("Ground", 0), level("First", 1));
    let raft = RaftIn { top_above_ground_mm: Some(600.0), thickness_mm: Some(500.0), projection_mm: Some(0.0) };
    let mut project = NanoArchProject::default();
    project.meta.declared = Declared { raft: Some(raft), hourdi: Some(HourdiIn::default()), ..Default::default() };
    let (g, f) = (ground.id, first.id);
    let mut doc = Document::default();
    doc.commit("two floors", Origin::User, vec![Command::Create(ground), Command::Create(first), room("R1", 10_000, g), room("R2", 12_000, f), Command::Create(element_of(&project, Source::User))]).unwrap();
    let rep = nnx_core::substruct::compute(&doc.model, &project.meta.declared);
    assert_eq!(rep.slabs.iter().map(|s| s.area_m2).collect::<Vec<_>>(), [96.0, 96.0]);
    assert_eq!(rep.raft.unwrap().area_m2, Some(80.0));
    let note = rep.notes.iter().find(|n| n.contains("outside the ground footprint")).expect("the overhang is reported");
    assert!(note.contains("16.00") && nnx_core::msgs::localize(note).1, "{note}");
    // All of the overhang on an existing roof: the ground slab loses it, the slab above does not, and no overhang is left to report.
    let on = |m2: f64| nnx_core::substruct::compute(&doc.model, &Declared { existing_roof_m2: Some(m2), ..project.meta.declared.clone() });
    let rep = on(16.0);
    assert_eq!(rep.slabs.iter().map(|s| s.area_m2).collect::<Vec<_>>(), [80.0, 96.0]);
    assert!(rep.notes.iter().all(|n| !n.contains("outside the ground footprint: the raft")), "{:?}", rep.notes);
    let warning = rep.notes.iter().find(|n| n.contains("structural engineer")).expect("the existing building is flagged");
    assert!(nnx_core::msgs::localize(warning).1, "{warning}");
    // Part of it: the rest is still reported.
    assert!(on(10.0).notes.iter().any(|n| n.contains("covers 6.00 m2 outside")));
    // More than there is outside: nothing is quantified, and it says why in both languages.
    let rep = on(20.0);
    assert!(rep.slabs.is_empty(), "{:?}", rep.slabs);
    let why = rep.notes.iter().find(|n| n.contains("until it is corrected")).expect("named");
    assert!(nnx_core::msgs::localize(why).1, "{why}");
}

#[test]
fn nothing_declared_adds_nothing_and_a_raft_wholly_above_ground_needs_no_excavation() {
    let doc = boxed(Declared::default());
    assert!(qto::compute(&doc.model, &QtoOpts::default()).items.iter().all(|i| !i.code.starts_with("RAFT.") && !i.code.starts_with("SLAB.")));
    let raft = RaftIn { top_above_ground_mm: Some(600.0), thickness_mm: Some(500.0), projection_mm: Some(0.0) };
    let doc = boxed(Declared { raft: Some(raft), hourdi: None, ..Default::default() });
    assert_eq!((quantity(&doc, "RAFT.AREA"), quantity(&doc, "RAFT.EXCAV")), (Some(80.0), None));
}

/// The notes about one clause, each of which must have its Arabic.
fn clause(raft: RaftIn, founding: FoundingIn, clause: &str) -> String {
    let declared = Declared { raft: Some(raft), founding: Some(founding), ..Default::default() };
    let rep = nnx_core::substruct::compute(&boxed(declared.clone()).model, &declared);
    for n in &rep.notes {
        assert!(nnx_core::msgs::localize(n).1, "no Arabic for: {n}");
    }
    let found: Vec<&String> = rep.notes.iter().filter(|n| n.starts_with(&format!("SBC 303 (2007) {clause} "))).collect();
    assert_eq!(found.len(), 1, "{clause}: {:?}", rep.notes);
    found[0].clone()
}

fn raft_of(thickness: Option<f64>) -> RaftIn {
    RaftIn { top_above_ground_mm: Some(600.0), thickness_mm: thickness, projection_mm: Some(500.0) }
}

#[test]
fn the_founding_depth_is_checked_against_sbc_303_for_the_declared_soil() {
    let on = |soil| FoundingIn { soil, ..Default::default() };
    let c = |t, soil| clause(raft_of(t), on(soil), "5.2.0");
    // Top 0.60 m above ground, 0.80 m thick: the underside is 0.20 m below ground.
    let fail = c(Some(800.0), Some(Soil::Cohesionless));
    assert!(fail.contains("not met") && fail.contains("0.20 m below") && fail.contains("1.20 m minimum") && fail.contains("1.80 m thick"), "{fail}");
    assert!(c(Some(2_000.0), Some(Soil::Cohesionless)).contains(" met:"));
    // 1.40 m below ground passes cohesionless soil but not silty or clay soil (1.50 m).
    assert!(c(Some(2_000.0), Some(Soil::SiltyClay)).contains("not met"));
    // Wholly above the ground.
    let above = c(Some(400.0), Some(Soil::Rock));
    assert!(above.contains("not met") && above.contains("0.20 m above ground"), "{above}");
    // Rock between 0.60 and 1.20 m is the engineer's call; from 1.20 m it passes.
    assert!(c(Some(1_300.0), Some(Soil::Rock)).contains("needs your engineer"));
    assert!(c(Some(1_800.0), Some(Soil::Rock)).contains(" met:"));
    // What is missing is named, and nothing passes without it.
    let thin = c(None, Some(Soil::SiltyClay));
    assert!(thin.contains("not checked") && thin.contains("at least 2.10 m"), "{thin}");
    assert!(c(Some(800.0), None).contains("not checked: choose the founding soil"));
}

#[test]
fn the_neighbouring_footing_is_checked_against_the_30_degree_line() {
    let with = |soil, depth, gap| FoundingIn { soil, existing_depth_mm: depth, existing_gap_mm: gap };
    let c = |f| clause(raft_of(Some(800.0)), f, "5.2.1");
    // Raft underside 0.20 m, existing 1.20 m: 1.00 m of difference needs 1.73 m of distance.
    let steep = c(with(Some(Soil::Cohesionless), Some(1_200.0), Some(1_000.0)));
    assert!(steep.contains("not met") && steep.contains("45.0 degrees") && steep.contains("at least 1.73 m"), "{steep}");
    assert!(c(with(Some(Soil::Cohesionless), Some(1_200.0), Some(2_000.0))).contains(" met:"));
    assert!(c(with(Some(Soil::Cohesionless), Some(1_200.0), Some(0.0))).contains("not met"));
    assert!(c(with(Some(Soil::Rock), Some(1_200.0), Some(2_000.0))).contains("not applied"));
    assert!(c(with(Some(Soil::Cohesionless), None, Some(2_000.0))).contains("not checked"));
    assert!(c(with(None, Some(1_200.0), Some(2_000.0))).contains("not checked"));
    assert!(Declared { founding: Some(with(None, Some(-1.0), None)), ..Default::default() }.check().is_err());
}

#[test]
fn the_source_of_the_declared_values_and_an_estimate_follow_the_quantities_into_the_exports() {
    let hourdi = HourdiIn { total_mm: Some(300.0), topping_mm: Some(70.0), rib_width_mm: Some(150.0), rib_spacing_mm: Some(550.0), block_length_mm: Some(200.0) };
    let with = |estimate: bool, source: &str| boxed(Declared { hourdi: Some(hourdi.clone()), estimate, source: source.into(), ..Default::default() });
    let of = |doc: &Document| qto::compute(&doc.model, &QtoOpts::default());
    let check = |q: &qto::Qto, label: &str| {
        for code in ["SLAB.FORM", "SLAB.VOL", "SLAB.BLOCKS"] {
            let d = &q.get(code).unwrap().description;
            assert!(d.ends_with(label), "{code}: {d}");
        }
        // Wall quantities are measured from the drawing and carry no label.
        assert!(q.items.iter().filter(|i| i.code.starts_with("WALL.")).all(|i| !i.description.contains('[')));
        for n in &q.warnings {
            if n.contains("declared structural values") {
                assert!(nnx_core::msgs::localize(n).1, "no Arabic for: {n}");
            }
        }
    };
    let q = of(&with(true, "guide example, page 153"));
    check(&q, " [ESTIMATE, not for ordering or construction; source: guide example, page 153]");
    assert!(q.to_csv().contains("ESTIMATE"));
    assert!(q.warnings.iter().any(|w| w.contains("are an estimate")) && q.warnings.iter().any(|w| w.ends_with("guide example, page 153")), "{:?}", q.warnings);
    let boq = nnx_core::boq::compute(&q.items, &Default::default());
    assert!(boq.rows.iter().any(|r| r.description.contains("ESTIMATE")));
    assert!(boq.warnings.iter().any(|w| w.contains("estimate") && nnx_core::msgs::localize(w).1), "{:?}", boq.warnings);
    check(&of(&with(true, "  ")), " [ESTIMATE, not for ordering or construction; source not stated]");
    check(&of(&with(false, "structural drawing S-02")), " [source: structural drawing S-02]");
    let q = of(&with(false, ""));
    check(&q, " [source of the declared values not stated]");
    assert!(q.warnings.iter().any(|w| w.contains("is not stated")));
    assert!(nnx_core::boq::compute(&q.items, &Default::default()).warnings.iter().all(|w| !w.contains("estimate")));
}

#[test]
fn a_material_that_comes_from_an_estimated_quantity_is_marked_in_every_material_output() {
    let hourdi = HourdiIn { total_mm: Some(300.0), topping_mm: Some(70.0), rib_width_mm: Some(150.0), rib_spacing_mm: Some(550.0), block_length_mm: Some(200.0) };
    // SYNTHETIC factors: one material from an estimated item, one from a measured wall.
    let lib: nnx_core::boq::BoqLib = serde_json::from_value(serde_json::json!({ "materials": {
        "SLAB.VOL": [{ "material": "cement", "unit": "kg", "per_unit": 300.0, "source": "test" }],
        "WALL.EXT.AREA": [{ "material": "mortar", "unit": "m3", "per_unit": 0.02, "source": "test" }] } }))
    .unwrap();
    for estimate in [true, false] {
        let doc = boxed(Declared { hourdi: Some(hourdi.clone()), estimate, ..Default::default() });
        let r = nnx_core::boq::compute(&qto::compute(&doc.model, &QtoOpts::default()).items, &lib);
        let flag = |name: &str| r.materials.iter().find(|m| m.material == name).unwrap().estimate;
        assert_eq!((flag("cement"), flag("mortar")), (estimate, false));
        let csv = r.materials_csv();
        let line = |name: &str| csv.lines().find(|l| l.starts_with(name)).unwrap().to_string();
        assert!(line("cement").ends_with(if estimate { ",estimate" } else { ",preliminary" }), "{csv}");
        assert!(line("mortar").ends_with(",preliminary"));
        for arabic in [true, false] {
            let mark = if arabic { "cement (تقديري)" } else { "cement (estimate)" };
            assert_eq!(r.html(arabic).contains(mark), estimate);
            let book = nnx_core::xlsx::boq_workbook(&r, arabic);
            assert_eq!(format!("{:?}", book.sheets[1].rows).contains(mark), estimate);
        }
    }
}
