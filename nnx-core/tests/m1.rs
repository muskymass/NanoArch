//! Site layout, furniture clearance and vertical elements (milestone 1).

use nnx_core::furniture::{self, FurnitureInput};
use nnx_core::model3d::{Box3, LevelInfo, Scene};
use nnx_core::site::{self, Edge, EdgeKind, RuleSet, SiteInput};
use nnx_core::site_layout::{self, Footprint, Frame, SiteLayoutInput};
use nnx_core::vertical::{self, AnnexIn, BasementIn, ElevatorIn, RampIn, VerticalInput};

fn near(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol, "{a} vs {b}");
}

fn fp(w: f64, l: f64, h: Option<f64>) -> Option<Footprint> {
    Some(Footprint { width_m: w, length_m: l, height_m: h })
}

fn full_input() -> SiteLayoutInput {
    SiteLayoutInput { wall_height_m: Some(2.5), wall_thickness_m: Some(0.2), gate_width_m: Some(4.0), guard: fp(2.5, 2.5, Some(2.8)), carport: fp(3.0, 5.5, Some(2.6)), pool: fp(3.5, 6.0, None), courtyard: fp(3.0, 3.0, None) }
}

#[test]
fn the_site_layout_places_every_item_outside_the_building_inside_the_plot_and_the_pool_far_from_the_gate() {
    let r = site::analyse(&site::rectangle_with_setbacks(20.0, 30.0, 6.0, 4.0, 4.0)).unwrap();
    let f = Frame::of(&r);
    // The building frame starts at the building's south-west corner (4, 6) of the plot.
    let q = f.to_building([11.5, 18.0]);
    near(q[0], 7.5, 1e-9);
    near(q[1], 12.0, 1e-9);
    let back = f.to_plot(q);
    near(back[0], 11.5, 1e-9);
    near(back[1], 18.0, 1e-9);
    let l = site_layout::layout(&r, &full_input()).unwrap();
    assert_eq!(l.items.iter().map(|i| i.kind).collect::<Vec<_>>(), vec!["guard", "carport", "pool", "courtyard"]);
    let bld = [0.0, 0.0, f.width_m, f.length_m];
    for (i, a) in l.items.iter().enumerate() {
        let c = a.corners_plot;
        assert!(c.iter().all(|p| p[0] >= -1e-9 && p[0] <= 20.0 + 1e-9 && p[1] >= -1e-9 && p[1] <= 30.0 + 1e-9), "{} inside the plot", a.kind);
        let ov = |x: [f64; 4], y: [f64; 4]| x[0] < y[2] - 1e-9 && y[0] < x[2] - 1e-9 && x[1] < y[3] - 1e-9 && y[1] < x[3] - 1e-9;
        assert!(!ov(a.rect_b, bld), "{} is clear of the building", a.kind);
        for b in &l.items[i + 1..] {
            assert!(!ov(a.rect_b, b.rect_b), "{} and {} overlap", a.kind, b.kind);
        }
    }
    // The gate is in the middle of the front edge (x = 10, y = 0 in the plot frame); the pool is the farthest item from it.
    let gate = l.gate_m.unwrap();
    near((gate[0][0] + gate[1][0]) / 2.0, 10.0, 1e-9);
    near(gate[1][0] - gate[0][0], 4.0, 1e-9);
    let dist = |k: &str| {
        let i = l.items.iter().find(|i| i.kind == k).unwrap();
        let c = [(i.corners_plot[0][0] + i.corners_plot[2][0]) / 2.0, (i.corners_plot[0][1] + i.corners_plot[2][1]) / 2.0];
        (c[0] - 10.0).hypot(c[1])
    };
    assert!(dist("pool") > dist("guard") && dist("pool") > dist("carport"));
    // Walls: the front is split by the gate (two pieces), the other three edges are whole.
    assert_eq!(l.walls.len(), 5);
    let solids = site_layout::solids(&r, &l);
    assert_eq!(solids.iter().filter(|s| s.0 == "wall").count(), 5);
    assert!(solids.iter().filter(|s| s.0 == "wall").all(|s| s.3 == 2.5 && s.2 == 0.0));
    assert!(solids.iter().any(|s| s.0 == "pool" && s.3 == 0.05), "a zone without a height is drawn flat");
    assert!(l.warnings.iter().any(|w| w.contains("setbacks")), "the setback caveat is stated");
    // The drawing shows them.
    let svg = site::site_svg_with(&r, "T", Some(&l));
    for needle in ["guard", "carport", "pool", "courtyard", "gate"] {
        assert!(svg.contains(needle), "{needle}");
    }
}

#[test]
fn missing_or_impossible_site_inputs_are_named_and_nothing_is_invented() {
    let r = site::analyse(&site::rectangle_with_setbacks(20.0, 30.0, 3.0, 1.5, 1.5)).unwrap();
    let only_height = SiteLayoutInput { wall_height_m: Some(2.5), ..Default::default() };
    let l = site_layout::layout(&r, &only_height).unwrap();
    assert!(l.walls.is_empty() && l.warnings.iter().any(|w| w.contains("wall thickness")), "{:?}", l.warnings);
    let nothing = site_layout::layout(&r, &SiteLayoutInput::default()).unwrap();
    assert!(nothing.items.is_empty() && nothing.walls.is_empty() && nothing.warnings.iter().any(|w| w.contains("no boundary wall")));
    let no_gate = SiteLayoutInput { wall_height_m: Some(2.5), wall_thickness_m: Some(0.2), ..Default::default() };
    let l = site_layout::layout(&r, &no_gate).unwrap();
    assert_eq!(l.walls.len(), 4);
    assert!(l.warnings.iter().any(|w| w.contains("gate width is missing")));
    let wide = SiteLayoutInput { gate_width_m: Some(25.0), ..no_gate.clone() };
    assert!(site_layout::layout(&r, &wide).unwrap_err().contains("as wide as the front edge"));
    assert!(SiteLayoutInput { wall_height_m: Some(-1.0), ..Default::default() }.check().is_err());
    assert!(SiteLayoutInput { pool: fp(0.0, 5.0, None), ..Default::default() }.check().unwrap_err().contains("pool width"));
    // A pool that cannot fit is reported by name, not squeezed in.
    let huge = SiteLayoutInput { pool: fp(19.0, 29.0, None), ..Default::default() };
    assert!(site_layout::layout(&r, &huge).unwrap().warnings.iter().any(|w| w.contains("no room for the pool")));
}

#[test]
fn a_chamfered_plot_keeps_every_item_inside_its_outline_and_walls_follow_the_slanted_edge_in_steps() {
    let edge = |kind| Edge { kind, street_width_m: Some(15.0), setback_m: None };
    let input = SiteInput { polygon_m: vec![[0.0, 0.0], [18.0, 0.0], [18.0, 18.0], [14.0, 22.0], [0.0, 22.0]], edges: vec![edge(EdgeKind::FrontStreet), edge(EdgeKind::Neighbour), edge(EdgeKind::Neighbour), edge(EdgeKind::SideStreet), edge(EdgeKind::Neighbour)], rule: RuleSet::MinistryVilla, floors: Some(2) };
    let r = site::analyse(&input).unwrap();
    let l = site_layout::layout(&r, &SiteLayoutInput { wall_height_m: Some(2.0), wall_thickness_m: Some(0.2), gate_width_m: Some(3.0), carport: fp(3.0, 5.0, Some(2.5)), ..Default::default() }).unwrap();
    assert_eq!(l.items.len(), 1);
    assert!(l.items[0].corners_plot.iter().all(|p| site::contains(&r.polygon_m, *p)));
    let solids = site_layout::solids(&r, &l);
    // The slanted edge (4 m, 4 m) is about 5.66 m long: 29 steps of 0.2 m.
    assert!(solids.iter().filter(|s| s.0 == "wall").count() > 20);
}

fn space(x0: f64, y0: f64, x1: f64, y1: f64, func: &str) -> Box3 {
    Box3 { id: format!("{func}{x0}{y0}"), kind: "Space", class: func.into(), min: [x0, y0, 0.0], max: [x1, y1, 0.0], visual: false, level: Some(0), discipline: "architectural" }
}

fn scene(boxes: Vec<Box3>) -> Scene {
    Scene { levels: vec![LevelInfo { id: "l".into(), name: "G".into(), index: 0, z0: 0.0, storey: 3.0 }], boxes, min: [0.0; 3], max: [12.0, 4.0, 3.15], storey_height: 3.0, assemblies: Default::default() }
}

fn slab(top: f64) -> Box3 {
    Box3 { id: "s".into(), kind: "Slab", class: String::new(), min: [0.0, 0.0, top - 0.15], max: [12.0, 4.0, top], visual: false, level: Some(0), discipline: "architectural" }
}

#[test]
fn furniture_is_placed_per_room_type_and_narrow_walkways_are_flagged_against_the_input() {
    assert!(furniture::plan(&scene(vec![]), &FurnitureInput::default()).unwrap_err().contains("missing"));
    let s = scene(vec![space(0.0, 0.0, 6.0, 4.0, "living"), space(6.0, 0.0, 9.0, 4.0, "bedroom"), space(9.0, 0.0, 11.2, 2.2, "majlis"), space(0.0, 4.0, 6.0, 7.0, "kitchen"), space(0.0, 7.0, 4.0, 11.0, "dining")]);
    let at = |m: f64| furniture::plan(&s, &FurnitureInput { min_clear_mm: Some(m) }).unwrap();
    let r = at(900.0);
    let kinds = |room: &str| r.pieces.iter().filter(|p| p.func == room).map(|p| p.kind).collect::<Vec<_>>();
    assert_eq!(kinds("living"), vec!["sofa", "coffee table"]);
    assert_eq!(kinds("bedroom"), vec!["bed"]);
    assert_eq!(kinds("majlis"), vec!["seating", "seating", "seating"]);
    assert_eq!(kinds("kitchen"), vec!["counter"]);
    assert_eq!(kinds("dining"), vec!["dining table"]);
    // The living room is fine at 900 mm (sofa to table 1.0 m); the 3 m wide bedroom leaves 0.7 m beside the bed on both sides;
    // the 2.2 m wide majlis leaves 0.6 m between its two side seats.
    let flagged = |room: &str| r.flags.iter().filter(|f| f.func == room).count();
    assert_eq!(flagged("living"), 0, "{:?}", r.flags);
    assert_eq!(flagged("bedroom"), 2, "{:?}", r.flags);
    assert!(flagged("majlis") >= 1);
    assert!(r.flags.iter().all(|f| f.min_mm == 900.0 && f.gap_mm > 0.0 && f.gap_mm < 900.0));
    // At 1200 mm the living room's 1.0 m between sofa and table is narrow too; at 600 mm the bedroom and the majlis are fine.
    assert!(at(1200.0).flags.iter().any(|f| f.func == "living" && f.what.contains("between two pieces")));
    let lax = at(600.0);
    assert!(lax.flags.iter().all(|f| f.func != "bedroom" && f.func != "majlis"), "{:?}", lax.flags);
    // A bed that does not fit is said so.
    let tiny = scene(vec![space(0.0, 0.0, 1.5, 1.5, "bedroom")]);
    assert!(furniture::plan(&tiny, &FurnitureInput { min_clear_mm: Some(900.0) }).unwrap().flags.iter().any(|f| f.what.contains("does not fit")));
    // Boxes and a drawing.
    let b = furniture::boxes(&r);
    assert_eq!(b.len(), r.pieces.len());
    assert!(b.iter().all(|x| x.kind == "Furniture" && x.max[2] > x.min[2]));
    let svg = furniture::svg(&r, Some(0));
    assert!(svg.contains("class=\"furniture\"") && svg.contains("class=\"narrow\"") && svg.contains("minimum clear walkway 900 mm"));
    assert!(furniture::plan(&s, &FurnitureInput { min_clear_mm: Some(-5.0) }).is_err());
}

fn base() -> Scene {
    scene(vec![space(0.0, 0.0, 12.0, 4.0, "living"), slab(3.15)])
}

fn elevator() -> ElevatorIn {
    ElevatorIn { car_width_mm: Some(1100.0), car_depth_mm: Some(1400.0), clear_side_mm: Some(150.0), clear_back_mm: Some(200.0), clear_front_mm: Some(100.0), wall_mm: Some(200.0), pit_mm: Some(1200.0), overhead_mm: Some(3800.0), x_mm: Some(500.0), y_mm: Some(500.0), ..Default::default() }
}

#[test]
fn the_elevator_shaft_is_the_car_plus_clearances_plus_walls_and_is_checked_against_the_building() {
    let v = VerticalInput { elevator: Some(elevator()), ..Default::default() };
    let r = vertical::report(&base(), &v).unwrap();
    let s = r.shaft.unwrap();
    assert_eq!((s.inner_mm, s.outer_mm), ([1400.0, 1700.0], [1800.0, 2100.0]));
    near(s.outer_area_m2, 3.78, 1e-9);
    near(s.total_height_m, 3.15 + 5.0, 1e-9);
    assert_eq!((s.stops, s.machine_room), (1, false));
    assert!(r.problems.is_empty());
    // Outside the building, and a machine room without its height.
    let out = VerticalInput { elevator: Some(ElevatorIn { x_mm: Some(11500.0), ..elevator() }), ..Default::default() };
    assert!(vertical::report(&base(), &out).unwrap().problems.iter().any(|p| p.contains("not inside the building")));
    let mr = VerticalInput { elevator: Some(ElevatorIn { machine_room: true, ..elevator() }), ..Default::default() };
    assert!(vertical::report(&base(), &mr).unwrap().problems.iter().any(|p| p.contains("machine room height is missing")));
    // Missing inputs are named, none defaulted.
    let miss = VerticalInput { elevator: Some(ElevatorIn { car_width_mm: None, pit_mm: None, ..elevator() }), ..Default::default() };
    let p = vertical::report(&base(), &miss).unwrap();
    assert!(p.shaft.is_none() && p.problems.iter().any(|x| x.contains("car width")) && p.problems.iter().any(|x| x.contains("pit depth")));
    assert!(vertical::report(&base(), &VerticalInput { elevator: Some(ElevatorIn { wall_mm: Some(-1.0), ..elevator() }), ..Default::default() }).is_err());
    // 3D: the shaft runs from the pit to above the roof.
    let boxes = vertical::boxes(&base(), &v);
    let e = boxes.iter().find(|b| b.class == "elevator").unwrap();
    near(e.min[2], -1.2, 1e-9);
    near(e.max[2], 3.15 + 3.8, 1e-9);
    let with_mr = VerticalInput { elevator: Some(ElevatorIn { machine_room: true, machine_room_height_mm: Some(2500.0), ..elevator() }), ..Default::default() };
    assert!(vertical::boxes(&base(), &with_mr).iter().any(|b| b.class == "machine"));
}

#[test]
fn basement_levels_a_ramp_checked_against_the_given_limit_and_a_roof_annex() {
    let ramp = |run: f64, limit: Option<f64>| RampIn { width_mm: Some(3000.0), run_mm: Some(run), x_mm: Some(2000.0), y_mm: Some(1000.0), max_slope_pct: limit, source: Some("test input".into()) };
    let b = |run: f64, limit: Option<f64>| VerticalInput { basement: Some(BasementIn { levels: 1, height_mm: Some(2800.0), ramp: Some(ramp(run, limit)) }), ..Default::default() };
    let r = vertical::report(&base(), &b(24000.0, Some(12.0))).unwrap();
    assert_eq!(r.basement_levels, vec!["LVL-B1"]);
    near(r.basement_area_m2, 48.0, 1e-9);
    near(r.basement_volume_m3, 134.4, 1e-9);
    let k = r.ramp.unwrap();
    assert_eq!((k.rise_mm, k.slope_pct, k.ok, k.min_run_mm), (2800.0, 11.67, Some(true), Some(23334.0)));
    assert_eq!(vertical::report(&base(), &b(15000.0, Some(12.0))).unwrap().ramp.unwrap().ok, Some(false));
    let unjudged = vertical::report(&base(), &b(15000.0, None)).unwrap();
    assert_eq!(unjudged.ramp.unwrap().ok, None);
    assert!(unjudged.problems.iter().any(|p| p.contains("slope limit is missing")));
    // Two levels.
    let two = VerticalInput { basement: Some(BasementIn { levels: 2, height_mm: Some(3000.0), ramp: None }), ..Default::default() };
    assert_eq!(vertical::report(&base(), &two).unwrap().basement_levels, vec!["LVL-B1", "LVL-B2"]);
    assert_eq!(vertical::boxes(&base(), &two).iter().filter(|x| x.class == "basement").count(), 2);
    assert!(vertical::report(&base(), &VerticalInput { basement: Some(BasementIn { levels: 9, height_mm: Some(3000.0), ramp: None }), ..Default::default() }).is_err());
    assert!(vertical::report(&base(), &VerticalInput { basement: Some(BasementIn { levels: 1, height_mm: None, ramp: None }), ..Default::default() }).unwrap().problems.iter().any(|p| p.contains("basement height")));
    assert_eq!(vertical::boxes(&base(), &b(24000.0, Some(12.0))).iter().filter(|x| x.class == "ramp").count(), 12);
    // Annex: area, volume and a box on the roof.
    let a = VerticalInput { roof_annex: Some(AnnexIn { x_mm: Some(1000.0), y_mm: Some(500.0), width_mm: Some(6000.0), length_mm: Some(3000.0), height_mm: Some(2800.0) }), ..Default::default() };
    let r = vertical::report(&base(), &a).unwrap();
    near(r.annex_area_m2, 18.0, 1e-9);
    near(r.annex_volume_m3, 50.4, 1e-9);
    let bx = vertical::boxes(&base(), &a);
    near(bx[0].min[2], 3.15, 1e-9);
    near(bx[0].max[2], 5.95, 1e-9);
    let out = VerticalInput { roof_annex: Some(AnnexIn { x_mm: Some(9000.0), ..a.roof_annex.clone().unwrap() }), ..Default::default() };
    assert!(vertical::report(&base(), &out).unwrap().problems.iter().any(|p| p.contains("annex is not inside")));
    assert!(vertical::boxes(&base(), &out).is_empty(), "an annex outside the building is not drawn");
}
