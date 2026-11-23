use nnx_core::plot_import::import_dxf;

fn dxf(insunits: Option<i32>, entities: &str) -> Vec<u8> {
    let header = match insunits {
        Some(u) => format!("0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n{u}\n0\nENDSEC\n"),
        None => "0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1027\n0\nENDSEC\n".to_string(),
    };
    format!("0\nSECTION\n2\nBLOCKS\n0\nENDSEC\n{header}0\nSECTION\n2\nENTITIES\n{entities}0\nENDSEC\n0\nEOF\n").into_bytes()
}

fn lw(layer: &str, closed: bool, pts: &[(f64, f64)]) -> String {
    let mut s = format!("0\nLWPOLYLINE\n8\n{layer}\n90\n{}\n70\n{}\n", pts.len(), i32::from(closed));
    for (x, y) in pts {
        s += &format!("10\n{x}\n20\n{y}\n");
    }
    s
}

fn line(layer: &str, a: (f64, f64), b: (f64, f64)) -> String {
    format!("0\nLINE\n8\n{layer}\n10\n{}\n20\n{}\n11\n{}\n21\n{}\n", a.0, a.1, b.0, b.1)
}

#[test]
fn a_closed_polyline_in_metres_is_the_plot_and_moves_to_the_origin() {
    // 18 x 22 m plot far from the origin, drawn clockwise.
    let e = lw("PLOT", true, &[(500000.0, 2700000.0), (500000.0, 2700022.0), (500018.0, 2700022.0), (500018.0, 2700000.0)]);
    let r = import_dxf(&dxf(Some(6), &e), None, None).unwrap();
    assert_eq!(r.unit_source, "the drawing's $INSUNITS");
    assert_eq!(r.selected, Some(0));
    let c = &r.candidates[0];
    assert!((c.area_m2 - 396.0).abs() < 1e-6 && (c.perimeter_m - 80.0).abs() < 1e-6);
    assert!(nnx_core::site::signed_area(&c.polygon_m) > 0.0, "turned counter-clockwise");
    assert!(c.polygon_m.iter().all(|p| p[0] >= 0.0 && p[1] >= 0.0) && c.polygon_m.iter().any(|p| p[0] == 0.0));
    assert_eq!(r.origin_offset_m, Some([-500000.0, -2700000.0]));
    // The imported outline goes straight into the site analysis.
    let mut input = nnx_core::site::SiteInput { polygon_m: c.polygon_m.clone(), ..Default::default() };
    input.edges = vec![nnx_core::site::Edge { setback_m: Some(2.0), ..Default::default() }; 4];
    input.edges[0].kind = nnx_core::site::EdgeKind::FrontStreet;
    input.rule = nnx_core::site::RuleSet::Manual;
    let a = nnx_core::site::analyse(&input);
    assert!(a.is_ok(), "{:?}", a.err());
}

#[test]
fn units_are_converted_and_never_assumed() {
    let e = lw("PLOT", true, &[(0.0, 0.0), (18000.0, 0.0), (18000.0, 22000.0), (0.0, 22000.0)]);
    let mm = import_dxf(&dxf(Some(4), &e), None, None).unwrap();
    assert!((mm.candidates[0].area_m2 - 396.0).abs() < 1e-6);
    // No unit in the drawing: refused until stated; a stated unit is used and a conflicting header is reported.
    let err = import_dxf(&dxf(None, &e), None, None).unwrap_err();
    assert!(err.contains("no $INSUNITS"), "{err}");
    assert!(import_dxf(&dxf(Some(0), &e), None, None).unwrap_err().contains("unitless"));
    let ok = import_dxf(&dxf(None, &e), Some("mm"), None).unwrap();
    assert!((ok.candidates[0].area_m2 - 396.0).abs() < 1e-6);
    let clash = import_dxf(&dxf(Some(6), &e), Some("mm"), None).unwrap();
    assert!(clash.warnings.iter().any(|w| w.contains("declares m")));
    assert!(import_dxf(&dxf(None, &e), Some("yards"), None).unwrap_err().contains("unknown unit"));
    let ft = import_dxf(&dxf(Some(2), &lw("P", true, &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)])), None, None).unwrap();
    assert!((ft.candidates[0].area_m2 - 100.0 * 0.3048 * 0.3048).abs() < 1e-9);
}

#[test]
fn lines_that_meet_end_to_end_form_a_boundary_and_loose_lines_are_reported() {
    let e = [line("P", (0.0, 0.0), (10.0, 0.0)), line("P", (10.0, 10.0), (10.0, 0.0)), line("P", (10.0, 10.0), (0.0, 10.0)), line("P", (0.0, 10.0), (0.0, 0.0)), line("NOTE", (1.0, 1.0), (2.0, 2.0))].concat();
    let r = import_dxf(&dxf(Some(6), &e), None, None).unwrap();
    assert_eq!(r.candidates.len(), 1);
    assert_eq!(r.candidates[0].source, "LINE chain");
    assert!((r.candidates[0].area_m2 - 100.0).abs() < 1e-9);
    assert!(r.warnings.iter().any(|w| w.contains("ignored")));
    // Three lines do not close.
    let open = [line("P", (0.0, 0.0), (10.0, 0.0)), line("P", (10.0, 0.0), (10.0, 10.0)), line("P", (10.0, 10.0), (0.0, 0.0))].concat();
    assert!(import_dxf(&dxf(Some(6), &open), None, None).is_ok(), "a triangle of three lines closes");
    let broken = [line("P", (0.0, 0.0), (10.0, 0.0)), line("P", (10.0, 0.0), (10.0, 10.0))].concat();
    assert!(import_dxf(&dxf(Some(6), &broken), None, None).unwrap_err().contains("no closed boundary"));
}

#[test]
fn several_boundaries_are_never_picked_silently() {
    let e = [lw("PLOT", true, &[(0.0, 0.0), (20.0, 0.0), (20.0, 20.0), (0.0, 20.0)]), lw("BLDG", true, &[(2.0, 2.0), (12.0, 2.0), (12.0, 12.0), (2.0, 12.0)]), lw("OPEN", false, &[(0.0, 0.0), (5.0, 5.0)])].concat();
    let r = import_dxf(&dxf(Some(6), &e), None, None).unwrap();
    assert_eq!(r.candidates.len(), 2);
    assert_eq!(r.selected, None);
    assert!(r.warnings.iter().any(|w| w.contains("choose one")));
    let pick = import_dxf(&dxf(Some(6), &e), None, Some(1)).unwrap();
    assert_eq!(pick.candidates[pick.selected.unwrap()].layer, "BLDG");
    assert!(import_dxf(&dxf(Some(6), &e), None, Some(7)).unwrap_err().contains("no candidate 7"));
}

#[test]
fn a_self_crossing_outline_is_listed_but_cannot_be_chosen() {
    let e = lw("BOW", true, &[(0.0, 0.0), (10.0, 5.0), (10.0, 0.0), (0.0, 8.0)]);
    let err = import_dxf(&dxf(Some(6), &e), None, None).unwrap_err();
    assert!(err.contains("unusable"), "{err}");
    assert!(import_dxf(&dxf(Some(6), &e), None, Some(0)).unwrap_err().contains("crosses itself"));
}

#[test]
fn bulge_arcs_are_tessellated() {
    // A half disc: chord 10 m (bulge 1 = semicircle) closed by the straight side. Area = pi * 5^2 / 2.
    let e = "0\nLWPOLYLINE\n8\nP\n90\n2\n70\n1\n10\n0\n20\n0\n42\n1\n10\n10\n20\n0\n".to_string();
    let r = import_dxf(&dxf(Some(6), &e), None, None).unwrap();
    let want = std::f64::consts::PI * 25.0 / 2.0;
    assert!((r.candidates[0].area_m2 - want).abs() / want < 0.01, "{} vs {want}", r.candidates[0].area_m2);
    // The arc bulges to the right of travel (counter-clockwise bulge > 0): below the chord here.
    assert!(r.candidates[0].polygon_m.iter().all(|p| p[1] >= -1e-9));
}

#[test]
fn old_style_polylines_are_read() {
    let mut e = String::from("0\nPOLYLINE\n8\nOLD\n66\n1\n70\n1\n");
    for (x, y) in [(0.0, 0.0), (8.0, 0.0), (8.0, 5.0), (0.0, 5.0)] {
        e += &format!("0\nVERTEX\n8\nOLD\n10\n{x}\n20\n{y}\n");
    }
    e += "0\nSEQEND\n";
    let r = import_dxf(&dxf(Some(6), &e), None, None).unwrap();
    assert_eq!(r.candidates[0].source, "POLYLINE");
    assert!((r.candidates[0].area_m2 - 40.0).abs() < 1e-9);
}

#[test]
fn dwg_and_garbage_are_refused_with_a_reason() {
    assert!(import_dxf(b"AC1032\0\0\0", None, None).unwrap_err().contains("DWG"));
    assert!(import_dxf(b"AutoCAD Binary DXF\r\n", None, None).unwrap_err().contains("binary DXF"));
    assert!(import_dxf(b"hello world\nfoo\n", None, None).unwrap_err().contains("not a DXF"));
    assert!(import_dxf(b"0\nSECTION\n2\nHEADER\n0\nENDSEC\n", Some("m"), None).unwrap_err().contains("ENTITIES"));
}
