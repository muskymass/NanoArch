//! Plots of any shape: setbacks per edge, the inward offset, and the largest building rectangle. Shapes and distances are
//! synthetic; every expected number is worked out by hand in the comment next to it.

use nnx_core::site::{self, Edge, EdgeKind, RuleSet, SiteInput, contains, signed_area};

fn edge(kind: EdgeKind, w: Option<f64>, sb: Option<f64>) -> Edge {
    Edge { kind, street_width_m: w, setback_m: sb }
}

fn near(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol, "{a} vs {b}");
}

/// Distance from a point to a segment.
fn dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    (p[0] - (a[0] + t * dx)).hypot(p[1] - (a[1] + t * dy))
}

#[test]
fn a_rectangle_under_the_ministry_villa_rule_gets_3_m_in_front_and_1_5_m_on_the_other_sides() {
    // Front street 15 m wide: a fifth is 3 m, not less than 3 m. Neighbours 1.5 m. 20 x 30 -> (20 - 3) wide? No: the sides are
    // east and west (1.5 each) and the front is south (3), the back north (1.5): 17 x 25.5.
    let mut s = site::rectangle(20.0, 30.0);
    s.rule = RuleSet::MinistryVilla;
    s.edges[0].street_width_m = Some(15.0);
    let r = site::analyse(&s).unwrap();
    assert_eq!(r.setbacks_m, vec![3.0, 1.5, 1.5, 1.5]);
    assert!(r.setback_sources[0].contains("p.18") && r.setback_sources[1].contains("neighbours"));
    near(r.area_m2, 600.0, 1e-9);
    near(r.buildable_area_m2, 17.0 * 25.5, 1e-6);
    assert_eq!((r.building.width_m, r.building.length_m, r.building.entry), (17.0, 25.5, "south"));
    near(r.building.angle_deg, 0.0, 1e-9);
    assert_eq!(r.building.corners_m[0], [1.5, 3.0]);
    near(r.coverage, 17.0 * 25.5 / 600.0, 1e-6);
    assert!(r.warnings.is_empty(), "a rectangle is used completely: {:?}", r.warnings);
}

#[test]
fn a_wide_front_street_needs_6_m_and_a_side_street_a_fifth_of_its_width() {
    let mut s = site::rectangle(20.0, 30.0);
    s.rule = RuleSet::MinistryVilla;
    s.edges[0].street_width_m = Some(40.0);
    s.edges[1] = edge(EdgeKind::SideStreet, Some(20.0), None);
    let r = site::analyse(&s).unwrap();
    assert_eq!(r.setbacks_m[0], 6.0, "30 m or more: not less than 6 m");
    assert_eq!(r.setbacks_m[1], 4.0, "20 / 5 = 4 m on a side street");
    // A side street under 10 m is held to the 2 m floor; a plaza is 3 m.
    s.edges[1] = edge(EdgeKind::SideStreet, Some(8.0), None);
    s.edges[2] = edge(EdgeKind::Plaza, None, None);
    let r = site::analyse(&s).unwrap();
    assert_eq!((r.setbacks_m[1], r.setbacks_m[2]), (2.0, 3.0));
}

#[test]
fn what_the_rule_cannot_give_is_named_edge_by_edge_and_nothing_is_guessed() {
    let mut s = site::rectangle(20.0, 30.0);
    s.rule = RuleSet::MinistryVilla;
    // No street width on the front, a 35 m side street (no rule), and an apartment rule without floors.
    s.edges[1] = edge(EdgeKind::SideStreet, Some(35.0), None);
    let errs = site::analyse(&s).unwrap_err();
    assert!(errs.iter().any(|e| e.contains("edge 1") && e.contains("needs the width of its street")), "{errs:?}");
    assert!(errs.iter().any(|e| e.contains("edge 2") && e.contains("30 m or more")), "{errs:?}");
    s.edges[0].street_width_m = Some(12.0);
    s.edges[1].setback_m = Some(5.0);
    assert!(site::analyse(&s).is_ok(), "an entered setback replaces the rule's");
    s.rule = RuleSet::MinistryApartment;
    let errs = site::analyse(&s).unwrap_err();
    assert!(errs.iter().any(|e| e.contains("number of floors")), "{errs:?}");
    s.floors = Some(6);
    let r = site::analyse(&s).unwrap();
    assert_eq!(r.setbacks_m[2], 3.0, "above 5 floors the neighbour setback is 3 m");
    s.floors = Some(5);
    assert_eq!(site::analyse(&s).unwrap().setbacks_m[2], 2.0);
    // Manual: every edge needs its own.
    s.rule = RuleSet::Manual;
    s.edges[1].setback_m = None;
    let errs = site::analyse(&s).unwrap_err();
    assert!(errs.len() >= 3 && errs.iter().all(|e| e.contains("no rule is selected") || e.contains("entered")), "{errs:?}");
}

#[test]
fn a_clockwise_outline_gives_the_same_result_as_the_same_outline_counter_clockwise() {
    let mut a = site::rectangle(20.0, 30.0);
    a.edges = vec![edge(EdgeKind::FrontStreet, None, Some(3.0)), edge(EdgeKind::Neighbour, None, Some(1.0)), edge(EdgeKind::Neighbour, None, Some(2.0)), edge(EdgeKind::SideStreet, Some(10.0), Some(4.0))];
    let ra = site::analyse(&a).unwrap();
    // The same plot written clockwise: reverse the vertices, so edge k of the new list is old edge n-2-k reversed.
    let n = 4;
    let cw = SiteInput { polygon_m: a.polygon_m.iter().rev().copied().collect(), edges: (0..n).map(|k| a.edges[(2 * n - 2 - k) % n].clone()).collect(), ..a.clone() };
    assert!(signed_area(&cw.polygon_m) < 0.0);
    let rb = site::analyse(&cw).unwrap();
    near(rb.buildable_area_m2, ra.buildable_area_m2, 1e-9);
    assert_eq!((rb.building.width_m, rb.building.length_m), (ra.building.width_m, ra.building.length_m));
    // 20 - 1 - 4 = 15 wide (east 1, west 4), 30 - 3 - 2 = 25 long.
    assert_eq!((ra.building.width_m, ra.building.length_m), (15.0, 25.0));
    assert_eq!(rb.building.corners_m[0], ra.building.corners_m[0]);
}

#[test]
fn a_trapezoid_keeps_its_building_inside_the_slanted_sides_with_the_setback_on_every_edge() {
    // Bottom 24 m, top 12 m, height 30 m. Front street at the bottom.
    let s = SiteInput {
        polygon_m: vec![[0.0, 0.0], [24.0, 0.0], [18.0, 30.0], [6.0, 30.0]],
        edges: vec![edge(EdgeKind::FrontStreet, None, Some(3.0)), edge(EdgeKind::Neighbour, None, Some(1.5)), edge(EdgeKind::Neighbour, None, Some(1.5)), edge(EdgeKind::Neighbour, None, Some(1.5))],
        rule: RuleSet::Manual,
        floors: None,
    };
    let r = site::analyse(&s).unwrap();
    assert_eq!(r.buildable_m.len(), 4);
    near(r.area_m2, (24.0 + 12.0) / 2.0 * 30.0, 1e-9);
    // Every buildable vertex is at least the setback away from the edges it is not on, and inside the plot.
    for v in &r.buildable_m {
        assert!(contains(&r.polygon_m, *v));
    }
    // The building rectangle is inside the buildable polygon, so at least the setback from every edge.
    for c in &r.building.corners_m {
        assert!(contains(&r.buildable_m, [c[0], c[1]]), "{c:?}");
        for i in 0..4 {
            let (a, b) = (r.polygon_m[i], r.polygon_m[(i + 1) % 4]);
            assert!(dist(*c, a, b) >= r.setbacks_m[i] - 0.02, "corner {c:?} is {:.3} m from edge {i}, needs {}", dist(*c, a, b), r.setbacks_m[i]);
        }
    }
    // The narrow top limits the width: the building cannot be wider than the buildable polygon at its top edge.
    let top_y = r.building.corners_m[2][1];
    let slant = (24.0 - 12.0) / 2.0 / 30.0; // each side moves in 0.2 m per metre of height
    let inner_half_width = 12.0 - slant * top_y - 1.5 * (1.0 + slant * slant).sqrt();
    assert!(r.building.width_m <= 2.0 * inner_half_width + 0.1, "{} vs {}", r.building.width_m, 2.0 * inner_half_width);
    assert!(r.building_area_m2 > 0.6 * r.buildable_area_m2, "the rectangle uses most of a trapezoid: {} of {}", r.building_area_m2, r.buildable_area_m2);
    assert!(!r.warnings.is_empty() || r.unused_buildable_m2 < 0.05 * r.buildable_area_m2);
}

#[test]
fn a_chamfered_corner_sakhta_is_cut_off_and_its_edge_keeps_its_own_setback() {
    // 20 x 30 with a 4 x 4 chamfer at the north-east corner (shatfa), the chamfer facing a side street.
    let s = SiteInput {
        polygon_m: vec![[0.0, 0.0], [20.0, 0.0], [20.0, 26.0], [16.0, 30.0], [0.0, 30.0]],
        edges: vec![edge(EdgeKind::FrontStreet, Some(15.0), None), edge(EdgeKind::Neighbour, None, None), edge(EdgeKind::SideStreet, Some(10.0), None), edge(EdgeKind::Neighbour, None, None), edge(EdgeKind::Neighbour, None, None)],
        rule: RuleSet::MinistryVilla,
        floors: None,
    };
    let r = site::analyse(&s).unwrap();
    assert_eq!(r.setbacks_m, vec![3.0, 1.5, 2.0, 1.5, 1.5], "the chamfer edge is a side street 10 m wide: 10 / 5 = 2 m");
    near(r.area_m2, 600.0 - 8.0, 1e-9);
    assert_eq!(r.buildable_m.len(), 5, "the chamfer stays an edge of the buildable polygon");
    for c in &r.building.corners_m {
        assert!(dist(*c, [20.0, 26.0], [16.0, 30.0]) >= 2.0 - 0.02, "{c:?} is too close to the chamfer");
        assert!(contains(&r.buildable_m, *c));
    }
    // The chamfer takes the building's north-east corner in: the footprint is smaller than the plain rectangle's 17 x 25.5.
    assert!(r.building_area_m2 <= 17.0 * 25.5 + 1e-6);
}

#[test]
fn an_l_shaped_plot_offsets_round_its_inner_corner_and_the_building_takes_the_long_arm() {
    // Bottom arm 20 x 12, left arm 10 x 30 (north). Setbacks 3 m front, 1.5 m elsewhere.
    let s = SiteInput {
        polygon_m: vec![[0.0, 0.0], [20.0, 0.0], [20.0, 12.0], [10.0, 12.0], [10.0, 30.0], [0.0, 30.0]],
        edges: vec![edge(EdgeKind::FrontStreet, None, Some(3.0)), edge(EdgeKind::Neighbour, None, Some(1.5)), edge(EdgeKind::Neighbour, None, Some(1.5)), edge(EdgeKind::Neighbour, None, Some(1.5)), edge(EdgeKind::Neighbour, None, Some(1.5)), edge(EdgeKind::Neighbour, None, Some(1.5))],
        rule: RuleSet::Manual,
        floors: None,
    };
    let r = site::analyse(&s).unwrap();
    assert_eq!(r.buildable_m.len(), 6);
    // Buildable: bottom arm x 1.5..18.5, y 3..10.5 (17 x 7.5); left arm x 1.5..8.5, y 3..28.5 (7 x 25.5, minus the shared 7 x 7.5).
    near(r.buildable_area_m2, 17.0 * 7.5 + 7.0 * (28.5 - 10.5), 1e-6);
    // The largest rectangle: the whole left arm, 7 x 25.5 = 178.5, against 17 x 7.5 = 127.5 in the bottom arm.
    near(r.building.width_m, 7.0, 0.11);
    near(r.building.length_m, 25.5, 0.11);
    assert!(r.warnings.iter().any(|w| w.contains("not used")), "the bottom arm is left over: {:?}", r.warnings);
    assert!(r.unused_buildable_m2 > 60.0);
    near(r.unused_buildable_m2, r.buildable_area_m2 - r.building_area_m2, 0.02);
}

#[test]
fn a_plot_turned_on_the_ground_gives_the_same_building_turned_with_it_and_parallel_to_the_front_street() {
    let flat = {
        let mut s = site::rectangle(20.0, 30.0);
        s.edges[0].setback_m = Some(3.0);
        for e in s.edges.iter_mut().skip(1) {
            e.setback_m = Some(1.5);
        }
        s
    };
    let a = site::analyse(&flat).unwrap();
    let theta = 30f64.to_radians();
    let turned = SiteInput { polygon_m: flat.polygon_m.iter().map(|p| [p[0] * theta.cos() - p[1] * theta.sin() + 100.0, p[0] * theta.sin() + p[1] * theta.cos() + 50.0]).collect(), ..flat.clone() };
    let b = site::analyse(&turned).unwrap();
    near(b.building.angle_deg, 30.0, 0.01);
    assert_eq!((b.building.width_m, b.building.length_m), (a.building.width_m, a.building.length_m));
    assert_eq!(b.building.entry, "south");
    // The first side of the building lies along the front street: its direction is the street's.
    let (c0, c1) = (b.building.corners_m[0], b.building.corners_m[1]);
    near((c1[1] - c0[1]).atan2(c1[0] - c0[0]).to_degrees(), 30.0, 0.05);
    near(dist(c0, turned.polygon_m[0], turned.polygon_m[1]), 3.0, 0.02);
}

#[test]
fn bad_shapes_and_setbacks_that_swallow_the_plot_are_refused_with_the_reason() {
    let ok = site::rectangle(20.0, 30.0);
    let with = |f: &dyn Fn(&mut SiteInput)| {
        let mut s = ok.clone();
        for e in s.edges.iter_mut() {
            e.setback_m = Some(1.0);
        }
        f(&mut s);
        site::analyse(&s).unwrap_err().join("; ")
    };
    assert!(with(&|s| s.polygon_m.truncate(2)).contains("at least 3 vertices"));
    assert!(with(&|s| s.edges.pop().map(|_| ()).unwrap_or(())).contains("edge descriptions"));
    assert!(with(&|s| s.polygon_m[1] = [0.0, 0.0]).contains("coincide"));
    assert!(with(&|s| s.polygon_m = vec![[0.0, 0.0], [20.0, 30.0], [20.0, 0.0], [0.0, 30.0]]).contains("crosses itself"));
    assert!(with(&|s| s.polygon_m[0][0] = f64::NAN).contains("numbers"));
    assert!(with(&|s| s.edges[0].kind = EdgeKind::Neighbour).contains("front street"));
    assert!(with(&|s| s.edges[1].kind = EdgeKind::FrontStreet).contains("only one edge"));
    assert!(with(&|s| s.edges[1].setback_m = Some(-1.0)).contains("zero or more"));
    // Setbacks that leave nothing.
    let small = with(&|s| {
        s.polygon_m = vec![[0.0, 0.0], [8.0, 0.0], [8.0, 8.0], [0.0, 8.0]];
        s.edges.iter_mut().for_each(|e| e.setback_m = Some(5.0));
    });
    assert!(small.contains("no buildable area") || small.contains("disappear") || small.contains("cross"), "{small}");
    // Adjacent collinear edges must carry the same setback.
    let line = SiteInput {
        polygon_m: vec![[0.0, 0.0], [10.0, 0.0], [20.0, 0.0], [20.0, 30.0], [0.0, 30.0]],
        edges: vec![edge(EdgeKind::FrontStreet, None, Some(3.0)), edge(EdgeKind::FrontStreet, None, Some(2.0)), edge(EdgeKind::Neighbour, None, Some(1.5)), edge(EdgeKind::Neighbour, None, Some(1.5)), edge(EdgeKind::Neighbour, None, Some(1.5))],
        rule: RuleSet::Manual,
        floors: None,
    };
    assert!(site::analyse(&line).unwrap_err().join(";").contains("only one edge") || site::analyse(&line).is_err());
}

#[test]
fn the_site_plan_drawing_has_the_plot_the_buildable_outline_the_building_and_the_labels() {
    let mut s = site::rectangle(20.0, 30.0);
    s.rule = RuleSet::MinistryVilla;
    s.edges[0].street_width_m = Some(15.0);
    let r = site::analyse(&s).unwrap();
    let svg = site::site_svg(&r, "Test <plot>");
    assert!(svg.starts_with("<svg") && svg.trim_end().ends_with("</svg>"));
    for needle in ["class=\"plot\"", "class=\"buildable\"", "class=\"building\"", "class=\"front\"", "front street 15 m", "neighbour", "Test &lt;plot&gt;", "600.0 m²", "17 x 25.5 m"] {
        assert!(svg.contains(needle), "{needle}");
    }
}
