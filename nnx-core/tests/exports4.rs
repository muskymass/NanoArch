//! Phase 4 exports: DXF with MEP runs, tie beams, elevations and sections; the engineering report as HTML and XLSX.
//! Inputs are SYNTHETIC test data.

use nnx_core::dxf::{self, DxfOpts};
use nnx_core::elevation::{Cut, Side, ViewOpts, ViewSpec};
use nnx_core::mep::{self, MepInputs};
use nnx_core::report::{self, ReportData};
use nnx_core::sizing::{self, SizingInputs};
use nnx_core::structural::{self, Loads, Soil, StructOpts};
use nnx_core::typed::{Space, Wall, element_of};
use nnx_core::*;
use serde_json::json;
use std::collections::BTreeMap;

fn space(key: &str, func: &str, x: i64) -> Space {
    Space { func: func.into(), requirement_key: key.into(), area: 24.0, target_area: 24.0, x, y: 0, w: 6000, h: 4000, boundary: vec![[x, 0], [x + 6000, 0], [x + 6000, 4000], [x, 4000]], net_area: None, glazing_ratio: None, extra: Default::default() }
}

fn wall(x1: i64, y1: i64, x2: i64, y2: i64, a: &str, b: Option<&str>, exterior: bool) -> Wall {
    Wall { x1, y1, x2, y2, thickness: 200, height: 3000, length: (x2 - x1).abs() + (y2 - y1).abs(), is_exterior: exterior, room_a: a.into(), room_b: b.map(str::to_string), extra: Default::default() }
}

fn inputs() -> MepInputs {
    serde_json::from_value(json!({
        "insulation": {"assemblies": {
            "wall": {"rsi": 0.13, "rse": 0.04, "layers": [{"material": "block", "thickness_mm": 200, "lambda": 1.0}, {"material": "eps", "thickness_mm": 50, "lambda": 0.04, "kind": "insulation"}]},
            "roof": {"rsi": 0.1, "rse": 0.04, "layers": [{"material": "concrete", "thickness_mm": 150, "lambda": 1.5}]}}},
        "electrical": {"voltage_v": 230, "power_factor": 0.9, "three_phase": false, "lights_per_m2": 0.5, "light_w": 10,
            "sockets_per_m": 0.5, "socket_w": 200, "demand_factor": 0.5, "max_light_points": 12, "max_socket_points": 6,
            "breaker_margin": 1.25, "breaker_sizes_a": [6, 10, 16, 32, 63],
            "cables": [{"amps": 10, "mm2": 1.5}, {"amps": 16, "mm2": 2.5}, {"amps": 32, "mm2": 6}, {"amps": 63, "mm2": 16}],
            "resistivity": 0.0175, "max_vdrop_pct": 3, "boards": ["a", "b"], "route_factor": 1.2},
        "plumbing": {"catalog": {"wc": {"flow_lps": 0.1, "dfu": 4}}, "fixtures_by_func": {"bathroom": {"wc": 1}},
            "simultaneity": 0.5, "max_velocity_mps": 2,
            "pipes": [{"name": "PPR 20", "inner_mm": 16}], "drain_table": [{"max_dfu": 8, "dia_mm": 75}]},
        "hvac": {"indoor_c": 24, "outdoor_c": 45, "window_u": 3.0, "window_shgc": 0.5, "solar_w_m2": 400,
            "person_w": 100, "area_per_person_m2": 10, "lights_w_m2": 10, "equipment_w_m2": 5,
            "vent_l_s_per_m2": 1, "safety_factor": 1.1, "unit_sizes_w": [2000, 3500, 5000, 9000], "roof_exposed": true, "cop": 3.0},
        "routing": {"ahu_room": "a", "supply_delta_t_c": 10, "duct_velocity_mps": 5, "duct_height_mm": 300, "ceiling_height_mm": 2600, "return_offset_mm": 400,
            "drain_slope_pct": 1.5, "drain_invert_mm": 400, "stack_offset_mm": 1500,
            "tank": {"kind": "ground", "x_mm": -2000, "y_mm": 2000, "z_mm": 0}, "supply_z_mm": 600,
            "tray_z_mm": 2800, "tray_width_mm": 300, "tray_height_mm": 100}
    }))
    .unwrap()
}

fn sizing_inputs() -> SizingInputs {
    serde_json::from_value(json!({
        "footing_thickness_mm": 500, "tie_beam": {"width_mm": 250, "span_depth_ratio": 12},
        "rebar": {"cover_mm": 40, "anchorage_mm": 300, "hook_mm": 100,
            "column": {"bar_dia_mm": 16, "count": 8, "tie_dia_mm": 8, "tie_spacing_mm": 200},
            "footing": {"bar_dia_mm": 12, "spacing_mm": 150},
            "beam": {"top_dia_mm": 16, "top_count": 2, "bottom_dia_mm": 16, "bottom_count": 3, "stirrup_dia_mm": 8, "stirrup_spacing_mm": 150}}
    }))
    .unwrap()
}

fn house() -> Document {
    let mut cmds = vec![Command::Create(element_of(&space("a", "living", 0), Source::User)), Command::Create(element_of(&space("b", "bathroom", 6000), Source::User))];
    let mut south = None;
    for (x1, y1, x2, y2, a, b, ext) in [(0, 0, 12000, 0, "a", None, true), (12000, 0, 12000, 4000, "b", None, true), (12000, 4000, 0, 4000, "b", None, true), (0, 4000, 0, 0, "a", None, true), (6000, 0, 6000, 4000, "a", Some("b"), false)] {
        let el = element_of(&wall(x1, y1, x2, y2, a, b, ext), Source::User);
        if south.is_none() {
            south = Some(el.id);
        }
        cmds.push(Command::Create(el));
    }
    let south = south.unwrap();
    let door = nnx_core::typed::Door { width: 900, height: 2100, offset_center: 1500, is_egress: true, from: "a".into(), to: "out".into(), opens_into: "a".into(), extra: Default::default() };
    let win = nnx_core::typed::Window { width: 1500, height: 1200, sill_height: 900, offset_center: 4000, room: "a".into(), extra: Default::default() };
    cmds.push(Command::Create(element_of(&door, Source::User).hosted_by(south)));
    cmds.push(Command::Create(element_of(&win, Source::User).hosted_by(south)));
    let mut doc = Document::default();
    doc.commit("plan", Origin::User, cmds).unwrap();
    let opts = StructOpts { loads: Some(Loads { dead_kpa: 5.0, live_kpa: 2.0, source: "test".into() }), soil: Some(Soil { bearing_kpa: 150.0, source: "test".into() }), ..StructOpts::default() };
    structural::sync(&mut doc, &opts, None).unwrap();
    let rep = sizing::compute(&doc.model, Some(150.0), &sizing_inputs());
    sizing::apply_tie_beams(&mut doc, &rep).unwrap();
    let m = mep::compute(&doc.model, &inputs());
    mep::apply_with_runs(&mut doc, &m.points, &m.runs).unwrap();
    doc
}

fn dxf_opts() -> DxfOpts {
    DxfOpts { title: "Test".into(), labels: BTreeMap::new(), mep: true }
}

#[test]
fn the_dxf_has_the_runs_tie_beams_diffusers_and_the_views_on_their_own_layers() {
    let doc = house();
    let views = [ViewSpec::Elevation(Side::South), ViewSpec::Elevation(Side::East), ViewSpec::Section(Cut::AtY { name: "A-A".into(), at: 2.0 }), ViewSpec::Section(Cut::AtX { name: "B-B".into(), at: 3.0 })];
    let out = dxf::export_dxf_views(&doc.model, &[None], &views, &ViewOpts::default(), &dxf_opts()).unwrap();
    let n = |layer: &str| out.counts.get(layer).copied().unwrap_or(0);
    let runs = |system: &str| doc.model.elements.values().filter(|e| e.kind == "MepRun" && e.props["system"] == system).count();
    // Each run draws its outline and its centre line (plus a label): at least two entities per run.
    for (layer, system) in [("M-DUCT", "duct"), ("P-PIPE", "pipe"), ("P-DRAN", "drain"), ("E-TRAY", "cable")] {
        assert!(runs(system) > 0, "{system} runs exist");
        assert!(n(layer) >= 3 * runs(system), "{layer}: {} entities for {} runs", n(layer), runs(system));
    }
    assert_eq!(n("S-TIEB"), doc.model.elements.values().filter(|e| e.kind == "TieBeam").count(), "one outline per tie beam");
    assert!(n("S-TIEB") > 0);
    for layer in ["V-WALL", "V-OPEN", "V-SLAB", "V-LEVEL", "V-TEXT", "V-CUT", "V-FAR", "V-STRUC"] {
        assert!(n(layer) > 0, "{layer} has entities");
    }
    // Every layer used is declared in the LAYER table.
    for layer in out.counts.keys() {
        assert!(out.text.contains(&format!("2\n{layer}\n70\n0\n62\n")), "layer {layer} is not defined");
    }
    assert!(out.text.starts_with("0\nSECTION") && out.text.trim_end().ends_with("EOF"));
    // The elevations and sections lie to the right of the plan: the extent covers them.
    let plan_only = dxf::export_dxf(&doc.model, &[None], &dxf_opts()).unwrap();
    let ext = |text: &str| -> f64 {
        let at = text.find("$EXTMAX\n10\n").unwrap() + "$EXTMAX\n10\n".len();
        text[at..].lines().next().unwrap().parse().unwrap()
    };
    assert!(ext(&out.text) > ext(&plan_only.text) + 10_000.0, "the extent grows to the right: {} vs {}", ext(&out.text), ext(&plan_only.text));
    // Plans alone are unchanged by the views feature (no V- layers).
    assert!(plan_only.counts.keys().all(|k| !k.starts_with("V-")));
}

#[test]
fn views_alone_are_exportable_and_a_bad_view_is_refused_before_anything_is_written() {
    let doc = house();
    let only = dxf::export_dxf_views(&doc.model, &[], &[ViewSpec::Elevation(Side::North)], &ViewOpts::default(), &dxf_opts()).unwrap();
    assert!(only.counts.keys().all(|k| k.starts_with("V-")));
    let err = dxf::export_dxf_views(&doc.model, &[None], &[ViewSpec::Section(Cut::AtY { name: "A-A".into(), at: 99.0 })], &ViewOpts::default(), &dxf_opts()).unwrap_err();
    assert!(err.contains("outside the building"), "{err}");
    assert!(dxf::export_dxf_views(&doc.model, &[], &[], &ViewOpts::default(), &dxf_opts()).is_err());
}

fn data<'a>(m: &'a mep::MepReport, s: &'a sizing::SizingReport) -> ReportData<'a> {
    ReportData { title: "Engineering report <test>".into(), date: "2026-11-19".into(), mep: Some(m), sizing: Some(s) }
}

#[test]
fn the_html_report_carries_every_schedule_with_the_same_numbers_as_the_calculations() {
    let doc = house();
    let m = mep::compute(&doc.model, &inputs());
    let s = sizing::compute(&doc.model, Some(150.0), &sizing_inputs());
    let en = report::engineering_html(&data(&m, &s), false);
    assert!(en.contains("dir=\"ltr\"") && en.contains("Engineering report &lt;test&gt;"), "titles are escaped");
    for needle in ["HVAC load schedule", "Btu/hr", "Electrical load schedule", "kVA", "Plumbing and drainage", "Generated routes", "Structural schedules", "COL-1", "FND-1", "TB-1", "Bar bending schedule", "COL-1/V"] {
        assert!(en.contains(needle), "the report has '{needle}'");
    }
    let h = m.hvac.as_ref().unwrap();
    assert!(en.contains(&format!("{:.0} W · {:.0} Btu/hr · {:.2} tons", h.total_w, h.total_btu_hr, h.total_tons)));
    let e = m.electrical.as_ref().unwrap();
    assert!(en.contains(&format!("{:.2} kVA", e.demand_kva)));
    assert!(en.contains(&format!("{:.2}", s.steel_total_kg)) && en.contains(&format!("{:.3}", s.concrete_total_m3)));
    // Element ids are printed for traceability.
    assert!(en.contains(&s.columns[0].id) && en.contains(&s.footings[0].id));
    assert!(s.tie_beams[0].id.as_ref().is_some_and(|id| en.contains(id)), "tie beams are model elements and carry their ids");
    // Arabic: right to left with Arabic headings.
    let ar = report::engineering_html(&data(&m, &s), true);
    assert!(ar.contains("dir=\"rtl\"") && ar.contains("أحمال التكييف") && ar.contains("جدول ثني الحديد"));
    // Missing inputs and warnings are in the report, not dropped.
    let bare = sizing::compute(&doc.model, None, &SizingInputs::default());
    let page = report::engineering_html(&ReportData { title: "t".into(), date: "d".into(), mep: None, sizing: Some(&bare) }, false);
    assert!(page.contains("Missing inputs") && page.contains("tie_beam.width_mm") && page.contains("Warnings"));
}

#[test]
fn the_workbook_has_a_sheet_per_schedule_with_live_totals_and_reads_back_as_a_valid_package() {
    let doc = house();
    let m = mep::compute(&doc.model, &inputs());
    let s = sizing::compute(&doc.model, Some(150.0), &sizing_inputs());
    let wb = report::engineering_workbook(&data(&m, &s), false);
    let names: Vec<&str> = wb.sheets.iter().map(|x| x.name.as_str()).collect();
    for n in ["HVAC", "Electrical", "Boards kVA", "Plumbing", "Routes", "COL", "FND", "TB", "Concrete", "BBS"] {
        assert!(names.contains(&n), "{n} in {names:?}");
    }
    let bytes = wb.to_bytes();
    assert_eq!(&bytes[..2], b"PK");
    // The BBS total row is a SUM over the kg column and its cached value is the report's.
    let bbs = wb.sheets.iter().find(|x| x.name == "BBS").unwrap();
    let last = bbs.rows.last().unwrap();
    match &last[8].0 {
        nnx_core::xlsx::Cell::FormulaNum(f, v) => {
            assert_eq!(f, &format!("SUM(I2:I{})", bbs.rows.len() - 1));
            assert!((v - s.steel_total_kg).abs() < 1e-9);
        }
        other => panic!("{other:?}"),
    }
    // The HVAC sheet totals watts, Btu/hr and tons.
    let hv = wb.sheets.iter().find(|x| x.name == "HVAC").unwrap();
    let tot = hv.rows.last().unwrap();
    assert!(matches!(&tot[1].0, nnx_core::xlsx::Cell::FormulaNum(f, _) if f.starts_with("SUM(B2:B")));
    assert!(matches!(&tot[2].0, nnx_core::xlsx::Cell::FormulaNum(f, _) if f.starts_with("SUM(C2:C")));
    assert!(matches!(&tot[3].0, nnx_core::xlsx::Cell::FormulaNum(f, _) if f.starts_with("SUM(D2:D")));
    // Arabic workbooks are right to left; an empty report still opens with one explanatory sheet.
    assert!(report::engineering_workbook(&data(&m, &s), true).sheets.iter().all(|x| x.rtl));
    let empty = report::engineering_workbook(&ReportData { title: String::new(), date: String::new(), mep: None, sizing: None }, false);
    assert_eq!(empty.sheets.len(), 1);
}
