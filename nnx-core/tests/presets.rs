//! Reference presets (read from the code files, with pages) and the preliminary reinforcement design. Design inputs are
//! SYNTHETIC test values, chosen so every result can be checked by hand.

use nnx_core::presets::{self, Exposure, Member};
use nnx_core::rebar::{self, BeamIn, ColIn, DesignIn, FndIn};
use nnx_core::sizing::{self, SizingInputs};
use nnx_core::structural::{self, Loads, Soil, StructOpts};
use nnx_core::typed::{Space, Wall, element_of};
use nnx_core::*;
use serde_json::json;

fn near(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol, "{a} vs {b}");
}

#[test]
fn the_zone_table_is_what_the_guide_prints_with_its_page() {
    let z = |n| presets::zone(n).unwrap();
    assert_eq!((z(1).wall_u_max, z(1).roof_u_max, z(1).wall_r_min, z(1).roof_r_min), (0.403, 0.272, 2.36, 3.54));
    assert_eq!((z(2).wall_u_max, z(2).roof_u_max, z(2).wall_r_min, z(2).roof_r_min), (0.454, 0.34, 2.06, 2.8));
    assert_eq!((z(3).wall_u_max, z(3).roof_u_max, z(3).wall_r_min, z(3).roof_r_min), (0.511, 0.34, 1.81, 2.38));
    for p in presets::ZONES {
        assert_eq!((p.window_u_max, p.window_shgc_max), (2.668, 0.25));
        assert_eq!(p.source.page, 172);
        assert!(p.source.text().contains("secondary") && p.source.text().contains("p.172"));
    }
    assert!(presets::zone(0).is_none() && presets::zone(4).is_none());
    // The design temperature is not in the files, so it is not in the preset.
    assert!(presets::presets_json()["zones"][0]["outdoor_design_c"].is_null());
    assert!(presets::presets_json()["design_temperature_note"].as_str().unwrap().contains("not in the code files"));
}

#[test]
fn a_zone_is_named_by_number_or_by_a_city_of_the_map_and_jubail_says_it_is_not_named_there() {
    for (text, zone) in [("1", 1), ("zone 2", 2), (" Zone 3 ", 3), ("المنطقة 1", 1), ("منطقة 2", 2), ("Jubail", 1), ("jubail", 1), ("الجبيل", 1), ("Dhahran", 1), ("Riyadh", 1), ("Tabuk", 2), ("Khamis Mushait", 2), ("Abha", 3), ("أبها", 3)] {
        assert_eq!(presets::zone_of(text), Some(zone), "{text}");
    }
    for text in ["", "4", "zone 9", "Atlantis", "zone x", "12"] {
        assert_eq!(presets::zone_of(text), None, "{text}");
    }
    let jubail = presets::CITIES.iter().find(|c| c.en == "Jubail").unwrap();
    assert!(jubail.note.contains("not named on the map") && jubail.note.contains("confirm with the municipality"));
    assert!(presets::CITIES.iter().filter(|c| c.note.starts_with("named on")).count() >= 20, "the other cities were read from the map");
    let mut names: Vec<&str> = presets::CITIES.iter().map(|c| c.en).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), presets::CITIES.len());
}

#[test]
fn concrete_cover_follows_table_4a5_row_by_row() {
    use Exposure::*;
    use Member::*;
    assert_eq!(presets::cover_mm(Ground, Frame, 12.0), Some(75.0));
    assert_eq!(presets::cover_mm(Ground, Slab, 32.0), Some(75.0));
    assert_eq!(presets::cover_mm(Weather, Frame, 16.0), Some(40.0));
    assert_eq!(presets::cover_mm(Weather, Frame, 18.0), Some(50.0));
    assert_eq!(presets::cover_mm(Weather, Slab, 32.0), Some(50.0));
    assert_eq!(presets::cover_mm(Protected, Frame, 25.0), Some(40.0));
    assert_eq!(presets::cover_mm(Protected, Slab, 25.0), Some(20.0));
    assert_eq!(presets::cover_mm(Protected, Slab, 28.0), None, "the table gives none for a slab bar above Dia 25");
    assert_eq!(presets::FC_MIN_MPA, 20.0);
    let c = |n: &str| presets::exposure_class(n).unwrap();
    assert_eq!((c("S0").min_fc_mpa, c("S1").min_fc_mpa, c("S2").min_fc_mpa, c("c4").min_fc_mpa, c("C5").min_fc_mpa), (20.0, 28.0, 31.0, 35.0, 40.0));
    assert_eq!((c("C1").max_wcm, c("C4").max_wcm, c("C0").max_wcm), (Some(0.50), Some(0.40), None));
    assert!(presets::exposure_class("X9").is_none());
    assert_eq!(presets::presets_json()["concrete"]["cover_rows"].as_array().unwrap().len(), 5);
}

fn dsn() -> DesignIn {
    serde_json::from_value(json!({
        "fc_mpa": 25, "fy_mpa": 420, "bar_dias_mm": [12, 16, 20], "min_clear_spacing_mm": 25, "load_factor": 1.4,
        "exposure": {"column": "protected", "footing": "ground", "beam": "protected", "tie_beam": "ground"},
        "column": {"ratio_min": 0.01, "ratio_max": 0.04, "min_bars": 4, "phi": 0.65, "alpha": 0.8, "tie_dia_mm": 8, "tie_spacing_mm": 200},
        "footing": {"ratio_min": 0.0018, "phi_flexure": 0.9, "max_spacing_mm": 300},
        "beam": {"ratio_min": 0.004, "min_top_bars": 2, "stirrup_dia_mm": 8, "stirrup_spacing_mm": 150}
    }))
    .unwrap()
}

#[test]
fn a_lightly_loaded_column_gets_the_minimum_ratio_and_a_heavy_one_is_flagged_too_small() {
    let cols = [ColIn { name: "COL-1", id: "id1", b: 300.0, h: 300.0, axial_kn: Some(600.0) }, ColIn { name: "COL-2", id: "id2", b: 300.0, h: 300.0, axial_kn: Some(2500.0) }, ColIn { name: "COL-3", id: "id3", b: 300.0, h: 300.0, axial_kn: None }];
    let out = rebar::design(&dsn(), Some(500.0), &cols, &[], &[]);
    // Nu = 1.4 x 600 = 840 kN; 840000 / (0.65 x 0.8) = 1615385 N, less than 0.85 x 25 x 90000 = 1912500 N: no steel needed for load,
    // so the 1 % minimum governs: 900 mm2. Dia 12 bars (113.1 mm2): 8 of them; centre perimeter 768 mm, gap 768 / 8 - 12 = 84 mm >= 25.
    let r = out.rows.iter().find(|r| r.element == "COL-1").unwrap();
    near(r.required_mm2, 900.0, 0.01);
    assert_eq!((r.bars.as_str(), r.status.as_str(), r.cover_mm), ("8 x Dia 12", "ok", 40.0));
    near(r.provided_mm2, 8.0 * std::f64::consts::PI / 4.0 * 144.0, 0.01);
    near(r.ratio_pct, r.provided_mm2 / 90000.0 * 100.0, 0.01);
    let (bars, cover) = &out.columns["COL-1"];
    assert_eq!((bars.count, bars.bar_dia_mm, bars.tie_dia_mm, bars.tie_spacing_mm, *cover), (8, 12.0, 8.0, 200.0, 40.0));
    // 2500 kN: Nu = 3500 kN needs (6730769 - 1912500) / (420 - 21.25) = 12083 mm2, above the 4 % maximum of 3600 mm2.
    let heavy = out.rows.iter().find(|r| r.element == "COL-2").unwrap();
    assert_eq!(heavy.status, "section too small");
    assert!(heavy.notes.contains("12083") && heavy.notes.contains("enlarge the section"), "{}", heavy.notes);
    // A column with no load is named, not designed.
    assert!(out.rows.iter().all(|r| r.element != "COL-3"));
    assert!(out.missing.iter().any(|m| m.starts_with("COL-3: axial load")));
}

#[test]
fn a_column_that_carries_load_beyond_the_minimum_gets_steel_from_the_capacity_formula() {
    // 1500 kN: Nu = 2100 kN; 2100000 / 0.52 = 4038462 N; (4038462 - 1912500) / 398.75 = 5331 mm2 -> above the 4 % maximum of
    // 3600 mm2 for 300 x 300, so use a 400 x 400 section (Ag 160000, maximum 6400): As = (4038462 - 0.85 x 25 x 160000) / 398.75.
    let col = ColIn { name: "COL-1", id: "i", b: 400.0, h: 400.0, axial_kn: Some(1500.0) };
    let out = rebar::design(&dsn(), None, &[col], &[], &[]);
    let r = &out.rows[0];
    let expect: f64 = (2100.0 * 1000.0 / 0.52 - 0.85 * 25.0 * 160000.0) / (420.0 - 0.85 * 25.0);
    near(r.required_mm2, expect.max(0.01 * 160000.0), 0.01);
    assert!(expect > 1600.0 && r.provided_mm2 >= r.required_mm2 && r.status == "ok", "{r:?}");
    assert!(r.ratio_pct <= 4.0);
}

/// Reference case for the flexure branch of the footing design. The engine solves a quadratic in As; this test uses the other
/// textbook form (Rn, then rho = 0.85 f'c / fy x (1 - sqrt(1 - 2 Rn / (0.85 f'c)))), so the two cannot share an error.
/// Inputs are the synthetic test values of `dsn()` (fc 25, fy 420, phi 0.9, factor 1.4), not project values.
#[test]
fn footing_flexure_matches_the_rho_form_when_the_moment_governs() {
    let (fc, fy, phi, gamma) = (25.0_f64, 420.0_f64, 0.9_f64, 1.4_f64);
    let (side, col, load_kn, thick, cover, dia) = (2400.0_f64, 400.0_f64, 2000.0_f64, 500.0_f64, 75.0_f64, 12.0_f64);
    let qu_kpa = gamma * load_kn / (side * side / 1e6);
    let a_m = (side - col) / 2.0 / 1000.0;
    let mu_nmm = qu_kpa * a_m * a_m / 2.0 * 1e6; // N.mm per metre width
    let d = thick - cover - dia / 2.0;
    let rn = mu_nmm / (phi * 1000.0 * d * d);
    let rho = 0.85 * fc / fy * (1.0 - (1.0 - 2.0 * rn / (0.85 * fc)).sqrt());
    let as_ref = rho * 1000.0 * d;
    assert!(as_ref > 0.0018 * 1000.0 * thick, "the case must be governed by flexure, not by the minimum: {as_ref}");
    near(as_ref, 1594.6, 0.5); // hand value: qu 486.11 kPa, Mu 243.06 kN.m/m, Rn 1.538 MPa, rho 0.003806
    let f = FndIn { name: "FND-F", id: "ff", length: side, width: side, load_kn, col_b: col, col_h: col };
    let out = rebar::design(&dsn(), Some(thick), &[], &[f], &[]);
    let r = &out.rows[0];
    near(r.required_mm2, as_ref, 0.05);
    assert_eq!((r.status.as_str(), r.cover_mm), ("ok", 75.0));
    // Dia 12: 113.097 x 1000 / 1594.6 = 70.9 mm, rounded down to 70 mm.
    assert_eq!(r.bars, "Dia 12 @ 70 each way");
}

#[test]
fn a_footing_takes_the_larger_of_the_flexure_and_the_minimum_and_is_checked_for_spacing() {
    let f = FndIn { name: "FND-1", id: "f1", length: 1800.0, width: 1800.0, load_kn: 400.0, col_b: 300.0, col_h: 300.0 };
    let out = rebar::design(&dsn(), Some(500.0), &[], &[f], &[]);
    let r = &out.rows[0];
    // q_u = 1.4 x 400 / 3.24 = 172.8 kPa; cantilever 750 mm; Mu = 172.8 x 0.75^2 / 2 = 48.6 kN.m/m, which needs about 310 mm2/m
    // (d = 500 - 75 - 6); the minimum 0.0018 x 1000 x 500 = 900 mm2/m governs. Dia 12 at 113.1 x 1000 / 900 = 125 -> 120 mm.
    near(r.required_mm2, 900.0, 0.01);
    assert_eq!(r.bars, "Dia 12 @ 120 each way");
    assert_eq!((r.cover_mm, r.status.as_str()), (75.0, "ok"), "cast against the ground");
    assert!(r.notes.contains("punching"));
    let (mesh, cover) = &out.footings["FND-1"];
    assert_eq!((mesh.bar_dia_mm, mesh.spacing_mm, *cover), (12.0, 120.0, 75.0));
    // A thin footing under a big cantilever cannot be saved by bar size: it says so.
    let big = FndIn { name: "FND-2", id: "f2", length: 4000.0, width: 4000.0, load_kn: 3000.0, col_b: 300.0, col_h: 300.0 };
    let thin = rebar::design(&dsn(), Some(250.0), &[], &[big], &[]);
    assert_eq!(thin.rows[0].status, "section too small");
    assert!(thin.rows[0].notes.contains("thicken the footing"));
    // Without a thickness the footing is not designed and the missing name says so.
    assert!(rebar::design(&dsn(), None, &[], &[FndIn { name: "FND-3", id: "f3", length: 1500.0, width: 1500.0, load_kn: 100.0, col_b: 300.0, col_h: 300.0 }], &[]).missing.contains(&"footing_thickness_mm".to_string()));
}

#[test]
fn beams_and_tie_beams_get_minimum_steel_only_and_say_so() {
    let beams = [BeamIn { name: "BM-1", id: Some("b1"), b: 250.0, depth: 450.0, tie: false, span_mm: None, slab_load_kn_m: None }, BeamIn { name: "TB-1", id: None, b: 250.0, depth: 500.0, tie: true, span_mm: None, slab_load_kn_m: None }];
    let out = rebar::design(&dsn(), None, &[], &[], &beams);
    // BM-1: cover 40, stirrup 8; Dia 12: d = 450 - 40 - 8 - 6 = 396; As_min = 0.004 x 250 x 396 = 396 mm2 -> 4 bars (452 mm2).
    let b = out.rows.iter().find(|r| r.element == "BM-1").unwrap();
    near(b.required_mm2, 396.0, 0.01);
    assert_eq!(b.bars, "4 x Dia 12 bottom, 2 x Dia 12 top");
    assert!(b.status == "minimum steel only" && b.notes.contains("bending is not designed"));
    // The tie beam is in the ground: 75 mm cover.
    let t = out.rows.iter().find(|r| r.element == "TB-1").unwrap();
    assert_eq!((t.cover_mm, t.kind.as_str()), (75.0, "TieBeam"));
    let (bars, _) = &out.beams["BM-1"];
    assert_eq!((bars.bottom_count, bars.top_count, bars.stirrup_dia_mm), (4, 2, 8.0));
}

#[test]
fn every_missing_parameter_is_named_and_nothing_is_designed_from_nothing() {
    let cols = [ColIn { name: "COL-1", id: "i", b: 300.0, h: 300.0, axial_kn: Some(600.0) }];
    let out = rebar::design(&DesignIn::default(), Some(500.0), &cols, &[], &[]);
    for name in ["design.fc_mpa", "design.fy_mpa", "design.bar_dias_mm", "design.min_clear_spacing_mm", "design.load_factor"] {
        assert!(out.missing.contains(&name.to_string()), "{name}: {:?}", out.missing);
    }
    assert!(out.rows.is_empty() && out.columns.is_empty());
    let mut d = dsn();
    d.column.as_mut().unwrap().phi = None;
    d.exposure.as_mut().unwrap().column = None;
    let out = rebar::design(&d, None, &cols, &[], &[]);
    assert!(out.missing.contains(&"design.column.phi".to_string()) && out.missing.contains(&"design.exposure.column".to_string()), "{:?}", out.missing);
    assert!(out.rows.is_empty());
}

#[test]
fn a_concrete_strength_below_the_code_minimum_or_the_exposure_class_is_warned_with_the_page() {
    let mut d = dsn();
    d.fc_mpa = Some(18.0);
    let w = rebar::design(&d, None, &[], &[], &[]).warnings;
    assert!(w.iter().any(|x| x.contains("below the least value 20") && x.contains("p.131")), "{w:?}");
    d.fc_mpa = Some(30.0);
    d.exposure_class = Some("C4".into());
    let w = rebar::design(&d, None, &[], &[], &[]).warnings;
    assert!(w.iter().any(|x| x.contains("35 MPa") && x.contains("C4") && x.contains("p.132")), "{w:?}");
    d.exposure_class = Some("Z1".into());
    assert!(rebar::design(&d, None, &[], &[], &[]).warnings.iter().any(|x| x.contains("unknown exposure class")));
}

// ------------------------------------------------------------------ through the schedules

fn structured() -> Document {
    let space = |key: &str, func: &str, x: i64| Space { func: func.into(), requirement_key: key.into(), area: 24.0, target_area: 24.0, x, y: 0, w: 6000, h: 4000, boundary: vec![[x, 0], [x + 6000, 0], [x + 6000, 4000], [x, 4000]], net_area: None, glazing_ratio: None, extra: Default::default() };
    let wall = |x1: i64, y1: i64, x2: i64, y2: i64, a: &str, b: Option<&str>, ext: bool| Wall { x1, y1, x2, y2, thickness: 200, height: 3000, length: (x2 - x1).abs() + (y2 - y1).abs(), is_exterior: ext, room_a: a.into(), room_b: b.map(str::to_string), extra: Default::default() };
    let mut cmds = vec![Command::Create(element_of(&space("a", "living", 0), Source::User)), Command::Create(element_of(&space("b", "bedroom", 6000), Source::User))];
    for w in [wall(0, 0, 12000, 0, "a", None, true), wall(12000, 0, 12000, 4000, "b", None, true), wall(12000, 4000, 0, 4000, "b", None, true), wall(0, 4000, 0, 0, "a", None, true), wall(6000, 0, 6000, 4000, "a", Some("b"), false)] {
        cmds.push(Command::Create(element_of(&w, Source::User)));
    }
    let mut doc = Document::default();
    doc.commit("plan", Origin::User, cmds).unwrap();
    let opts = StructOpts { loads: Some(Loads { dead_kpa: 5.0, live_kpa: 2.0, source: "test".into() }), soil: Some(Soil { bearing_kpa: 150.0, source: "test".into() }), ..StructOpts::default() };
    structural::sync(&mut doc, &opts, None).unwrap();
    doc
}

fn inputs(with_templates: bool, with_design: bool) -> SizingInputs {
    let mut rebar = json!({ "anchorage_mm": 300, "hook_mm": 100 });
    if with_templates {
        rebar["cover_mm"] = json!(40);
        rebar["column"] = json!({"bar_dia_mm": 25, "count": 12, "tie_dia_mm": 10, "tie_spacing_mm": 150});
    }
    if with_design {
        rebar["design"] = serde_json::to_value(dsn()).unwrap();
    }
    serde_json::from_value(json!({ "footing_thickness_mm": 500, "tie_beam": {"width_mm": 250, "span_depth_ratio": 12}, "rebar": rebar })).unwrap()
}

#[test]
fn the_schedule_uses_the_design_where_there_is_no_template_and_your_template_where_there_is_one() {
    let doc = structured();
    // Design only: every column, footing, beam and tie beam gets bars, marked as designed, with a design row each.
    let rep = sizing::compute(&doc.model, Some(150.0), &inputs(false, true));
    assert!(rep.missing.is_empty(), "{:?}", rep.missing);
    assert_eq!(rep.designs.len(), rep.columns.len() + rep.footings.len() + doc.model.elements.values().filter(|e| e.kind == "Beam").count() + rep.tie_beams.len());
    assert!(rep.bars.iter().all(|b| b.source == "design") && !rep.bars.is_empty());
    for c in &rep.columns {
        assert!(rep.bars.iter().any(|b| b.mark == format!("{}/V", c.name)), "{} has vertical bars", c.name);
    }
    assert!(rep.designs.iter().filter(|r| r.kind == "Column").all(|r| r.status == "ok"), "{:?}", rep.designs.iter().map(|r| (&r.element, &r.status, &r.notes)).collect::<Vec<_>>());
    // A footing cast against the ground is covered by 75 mm: its cut lengths use it (Dia 12 mesh along x: 1800 - 150 + 200).
    let x = rep.bars.iter().find(|b| b.mark.ends_with("/X") && b.mark.starts_with("FND-")).unwrap();
    let f = rep.footings.iter().find(|f| f.name == x.element).unwrap();
    let side: f64 = f.provided_mm.split('x').next().unwrap().parse().unwrap();
    near(x.cut_length_mm, side - 150.0 + 200.0, 0.1);
    // Template for columns (12 Dia 25), design for the rest.
    let both = sizing::compute(&doc.model, Some(150.0), &inputs(true, true));
    let col_v = both.bars.iter().find(|b| b.mark == "COL-1/V").unwrap();
    assert_eq!((col_v.number, col_v.dia_mm, col_v.source.as_str()), (12, 25.0, "template"));
    assert!(both.bars.iter().filter(|b| b.mark.starts_with("FND-")).all(|b| b.source == "design"));
    // Templates only: no design rows, as before.
    let tmpl = sizing::compute(&doc.model, Some(150.0), &inputs(true, false));
    assert!(tmpl.designs.is_empty() && tmpl.bars.iter().all(|b| b.source == "template") || tmpl.bars.is_empty());
    // The CSV and the totals carry the source.
    assert!(rep.bbs_csv().lines().next().unwrap().ends_with(",source") && rep.bbs_csv().contains(",design"));
    near(rep.steel_total_kg, rep.bars.iter().map(|b| b.kg).sum::<f64>(), 0.01);
}

#[test]
fn design_parameters_that_are_missing_are_reported_under_design_and_the_other_bars_still_schedule() {
    let doc = structured();
    let mut inp = inputs(true, false);
    inp.rebar.as_mut().unwrap().design = Some(DesignIn::default());
    let rep = sizing::compute(&doc.model, Some(150.0), &inp);
    assert!(rep.missing["design"].contains(&"design.fy_mpa".to_string()));
    assert!(rep.designs.is_empty());
    assert!(rep.bars.iter().any(|b| b.mark == "COL-1/V" && b.source == "template"), "the template columns still schedule");
}

// ------------------------------------------------------------------ beam bending

fn bending_dsn() -> DesignIn {
    let mut d = dsn();
    let beam = d.beam.as_mut().unwrap();
    beam.phi_flexure = Some(0.9);
    beam.concrete_unit_weight_kn_m3 = Some(25.0);
    d
}

/// Inputs are the synthetic test values of `dsn()` (fc 25, fy 420, factor 1.4, phi 0.9, concrete 25 kN/m3), not project values.
#[test]
fn beam_flexure_matches_the_rho_form_for_a_simply_supported_span() {
    // Hand case: b 300, depth 500, span 6000 mm, slab strip 20 kN/m service. Self weight 25 x 0.3 x 0.5 = 3.75 kN/m;
    // wu = 1.4 (20 + 3.75) = 33.25 kN/m; Mu = 33.25 x 6^2 / 8 = 149.625 kN.m.
    // Dia 12 (cover 40, stirrup 8): d 446, As 954.9 -> 9 bars: clear gap 12 mm < 25, no fit.
    // Dia 16: d = 500 - 40 - 8 - 8 = 444; Rn = 149.625e6 / (0.9 x 300 x 444^2) = 2.8111 MPa; rho = 0.85 x 25 / 420 (1 - sqrt(1 - 2 Rn / 21.25))
    // = 0.007206; As = 959.9 mm2 -> 5 bars (1005 mm2); clear gap (300 - 96 - 80) / 4 = 31 mm >= 25: fits.
    let beam = BeamIn { name: "BM-1", id: Some("b1"), b: 300.0, depth: 500.0, tie: false, span_mm: Some(6000.0), slab_load_kn_m: Some(20.0) };
    let out = rebar::design(&bending_dsn(), None, &[], &[], &[beam]);
    assert!(out.missing.is_empty(), "{:?}", out.missing);
    let r = &out.rows[0];
    near(r.required_mm2, 959.87, 0.05);
    assert_eq!((r.status.as_str(), r.bars.as_str(), r.cover_mm), ("ok", "5 x Dia 16 bottom, 2 x Dia 16 top", 40.0));
    assert!(r.notes.contains("simply supported") && r.notes.contains("shear"));
    let (bars, _) = &out.beams["BM-1"];
    assert_eq!((bars.bottom_count, bars.bottom_dia_mm), (5, 16.0));
    // A light load leaves the minimum steel in charge (0.004 x 300 x 446 = 535 mm2, Dia 12: 5 bars) and the row still says "ok"
    // because bending was designed.
    let light = BeamIn { name: "BM-2", id: None, b: 300.0, depth: 500.0, tie: false, span_mm: Some(2000.0), slab_load_kn_m: Some(2.0) };
    let l = rebar::design(&bending_dsn(), None, &[], &[], &[light]);
    near(l.rows[0].required_mm2, 0.004 * 300.0 * 446.0, 0.05);
    assert_eq!(l.rows[0].status, "ok");
}

#[test]
fn a_beam_that_cannot_carry_its_moment_is_too_small_and_missing_inputs_are_named() {
    // 400 kN/m on a 6 m span: Rn is far above 0.85 f'c / 2, so no steel makes the section work.
    let heavy = BeamIn { name: "BM-1", id: None, b: 250.0, depth: 450.0, tie: false, span_mm: Some(6000.0), slab_load_kn_m: Some(400.0) };
    let h = rebar::design(&bending_dsn(), None, &[], &[], &[heavy]);
    assert_eq!(h.rows[0].status, "section too small");
    // Bending needs the span and the floor load: without them the beam keeps its minimum steel and the missing input is named.
    let bare = BeamIn { name: "BM-1", id: None, b: 250.0, depth: 450.0, tie: false, span_mm: Some(6000.0), slab_load_kn_m: None };
    let m = rebar::design(&bending_dsn(), None, &[], &[], &[bare]);
    assert_eq!(m.rows[0].status, "minimum steel only");
    assert!(m.missing.iter().any(|x| x.contains("beam bending") && x.contains("floor service load")), "{:?}", m.missing);
    // Giving phi without the unit weight (or the other way round) names the one that is missing and designs no bending.
    let mut half = dsn();
    half.beam.as_mut().unwrap().phi_flexure = Some(0.9);
    let beam = BeamIn { name: "BM-1", id: None, b: 250.0, depth: 450.0, tie: false, span_mm: Some(6000.0), slab_load_kn_m: Some(20.0) };
    let o = rebar::design(&half, None, &[], &[], &[beam]);
    assert!(o.missing.contains(&"design.beam.concrete_unit_weight_kn_m3".to_string()), "{:?}", o.missing);
    assert_eq!(o.rows[0].status, "minimum steel only");
    // Tie beams are never designed for bending, whatever is given.
    let tie = BeamIn { name: "TB-1", id: None, b: 250.0, depth: 500.0, tie: true, span_mm: Some(6000.0), slab_load_kn_m: Some(20.0) };
    let t = rebar::design(&bending_dsn(), None, &[], &[], &[tie]);
    assert_eq!(t.rows[0].status, "minimum steel only");
}

#[test]
fn the_schedule_designs_a_beam_for_the_slab_strip_it_carries_from_the_projects_floor_load() {
    use nnx_core::typed::{Beam, Slab};
    // A 6 x 4 m slab with one beam along its south edge: that beam takes a strip of half the short side (4 m / 2 = 2 m) of the load.
    let slab = Slab { x: 0, y: 0, w: 6000, h: 4000, thickness: 150, kind: "solid".into(), room: "a".into(), extra: Default::default() };
    let beam = Beam { x1: 0, y1: 0, x2: 6000, y2: 0, b: 300, h: 500, length: 6000, extra: Default::default() };
    let mut doc = Document::default();
    doc.commit("slab and beam", Origin::User, vec![Command::Create(element_of(&slab, Source::User)), Command::Create(element_of(&beam, Source::User))]).unwrap();
    let mut inp = inputs(false, true);
    inp.rebar.as_mut().unwrap().design = Some(bending_dsn());
    // Without a floor load: minimum steel only.
    let without = sizing::compute(&doc.model, None, &inp);
    assert!(without.designs.iter().any(|r| r.element == "BM-1" && r.status == "minimum steel only"), "{:?}", without.designs);
    // 10 kPa over the 2 m strip is the 20 kN/m of the hand case above: the same 959.9 mm2 and 5 x Dia 16.
    let structure = nnx_core::project::StructIn { dead_kpa: Some(8.0), live_kpa: Some(2.0), ..Default::default() };
    let with = sizing::compute(&doc.model, None, &inp.with_floor_load(&structure));
    let r = with.designs.iter().find(|r| r.element == "BM-1").unwrap();
    near(r.required_mm2, 959.87, 0.05);
    assert_eq!((r.status.as_str(), r.bars.as_str()), ("ok", "5 x Dia 16 bottom, 2 x Dia 16 top"));
    // Only one of the two loads given: no floor load is set.
    let half = nnx_core::project::StructIn { dead_kpa: Some(8.0), ..Default::default() };
    assert_eq!(inp.with_floor_load(&half).floor_service_kpa, None);
}

// ------------------------------------------------------------------ beam shear

fn shear_dsn(stirrup_spacing_mm: f64) -> DesignIn {
    let mut d = bending_dsn();
    d.bar_dias_mm.push(25.0); // case 2 of the shear test needs Dia 25
    let beam = d.beam.as_mut().unwrap();
    beam.phi_shear = Some(0.75);
    beam.concrete_shear_factor = Some(0.17);
    beam.stirrup_spacing_mm = Some(stirrup_spacing_mm);
    d
}

/// Inputs are the synthetic test values of `dsn()` plus phi_shear 0.75 and factor 0.17, not project values and not code values.
#[test]
fn beam_stirrups_are_checked_against_the_shear_at_d_from_the_support() {
    // Case 1: the bending case (b 300, depth 500, 6 m, 20 kN/m): 5 x Dia 16, d 444; wu 33.25 kN/m.
    // Vu = 33.25 (3 - 0.444) = 84.99 kN; Vc = 0.17 x 5 x 300 x 444 = 113.22 kN; Av = 2 x 50.27 = 100.53 mm2 (Dia 8, two legs);
    // Vs = 100.53 x 420 x 444 / 150 = 124.98 kN; phi (Vc + Vs) = 0.75 x 238.20 = 178.65 kN >= 84.99: sufficient.
    let beam = BeamIn { name: "BM-1", id: None, b: 300.0, depth: 500.0, tie: false, span_mm: Some(6000.0), slab_load_kn_m: Some(20.0) };
    let ok = rebar::design(&shear_dsn(150.0), None, &[], &[], &[beam]);
    assert!(ok.missing.is_empty(), "{:?}", ok.missing);
    let r = &ok.rows[0];
    assert_eq!(r.status, "ok");
    assert!(r.notes.contains("Vu 85.0 kN") && r.notes.contains("178.7 kN") && r.notes.contains("stirrups sufficient"), "{}", r.notes);
    // Case 2: b 300, depth 600, 4 m, 100 kN/m: bending gives 4 x Dia 25 (d 539.5, As 1588.96); wu 146.3 kN/m; Vu = 146.3 (2 - 0.5395) = 213.67 kN;
    // Vc = 137.57 kN; Dia 8 at 150: Vs = 151.86 kN, phi (Vc + Vs) = 217.08 kN >= 213.67: sufficient (a narrow margin).
    let b2 = || BeamIn { name: "BM-2", id: None, b: 300.0, depth: 600.0, tie: false, span_mm: Some(4000.0), slab_load_kn_m: Some(100.0) };
    let tight = rebar::design(&shear_dsn(150.0), None, &[], &[], &[b2()]);
    near(tight.rows[0].required_mm2, 1588.96, 0.05);
    assert_eq!((tight.rows[0].status.as_str(), tight.rows[0].bars.as_str()), ("ok", "4 x Dia 25 bottom, 2 x Dia 25 top"));
    assert!(tight.rows[0].notes.contains("Vu 213.7 kN") && tight.rows[0].notes.contains("217.1 kN"), "{}", tight.rows[0].notes);
    // The same beam with the stirrups at 300: Vs = 75.93 kN, phi (Vc + Vs) = 160.13 kN < 213.67: insufficient.
    let wide = rebar::design(&shear_dsn(300.0), None, &[], &[], &[b2()]);
    assert_eq!(wide.rows[0].status, "stirrups insufficient");
    assert!(wide.rows[0].notes.contains("160.1 kN") && wide.rows[0].notes.contains("stirrups insufficient"), "{}", wide.rows[0].notes);
}

#[test]
fn the_stirrup_check_names_what_it_needs() {
    // Shear parameters without the bending ones: the load is missing, so the check is named, not run.
    let mut d = dsn();
    let beam = d.beam.as_mut().unwrap();
    beam.phi_shear = Some(0.75);
    beam.concrete_shear_factor = Some(0.17);
    let b = BeamIn { name: "BM-1", id: None, b: 300.0, depth: 500.0, tie: false, span_mm: Some(6000.0), slab_load_kn_m: Some(20.0) };
    let out = rebar::design(&d, None, &[], &[], &[b]);
    assert!(out.missing.iter().any(|m| m.contains("the stirrup check needs the beam load")), "{:?}", out.missing);
    assert_eq!(out.rows[0].status, "minimum steel only");
    // Only one of the two shear parameters: the other is named.
    let mut half = bending_dsn();
    half.beam.as_mut().unwrap().phi_shear = Some(0.75);
    let b = BeamIn { name: "BM-1", id: None, b: 300.0, depth: 500.0, tie: false, span_mm: Some(6000.0), slab_load_kn_m: Some(20.0) };
    let o = rebar::design(&half, None, &[], &[], &[b]);
    assert!(o.missing.contains(&"design.beam.concrete_shear_factor".to_string()), "{:?}", o.missing);
    assert_eq!(o.rows[0].status, "ok", "bending is still designed");
    assert!(!o.rows[0].notes.contains("shear at d"), "no shear check without both factors");
}
