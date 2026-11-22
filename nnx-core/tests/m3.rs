//! Milestone 3: column grid, slab advisor, punching shear, combined footings and rafts, tanks, rebar cutting, analytical export.

use nnx_core::analytical;
use nnx_core::cutting::{self, Piece};
use nnx_core::gridopt::{self, GridInputs};
use nnx_core::merger::{self, MergeInputs};
use nnx_core::punching;
use nnx_core::slabadv::{self, RibbedGeom, SlabAdvInputs, SystemIn};
use nnx_core::structx::{StructuralInputs, schedule_names};
use nnx_core::tanks::{self, TankDesignIn, TankIn};
use nnx_core::typed::{Beam, Column, Footing, Slab, Space, TieBeam, Wall, element_of};
use nnx_core::*;

fn near(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol, "{a} vs {b}");
}

fn col(x: i64, y: i64, kn: Option<f64>) -> Column {
    Column { x, y, b: 300, h: 300, grid_ref: String::new(), axial_service_kn: kn, extra: Default::default() }
}

fn wall(x1: i64, y1: i64, x2: i64, y2: i64) -> Wall {
    wall_t(x1, y1, x2, y2, 200)
}

fn wall_t(x1: i64, y1: i64, x2: i64, y2: i64, thickness: i64) -> Wall {
    Wall { x1, y1, x2, y2, thickness, height: 3000, length: (x2 - x1).abs() + (y2 - y1).abs(), is_exterior: true, room_a: "a".into(), room_b: None, extra: Default::default() }
}

fn doc_with(cmds: Vec<Command>) -> Document {
    let mut doc = Document::default();
    doc.commit("model", Origin::User, cmds).unwrap();
    doc
}

fn create<T: nnx_core::typed::Typed + serde::Serialize>(t: &T) -> (Id, Command) {
    let e = element_of(t, Source::User);
    (e.id, Command::Create(e))
}

// ------------------------------------------------------------------ column grid

#[test]
fn a_column_off_its_wall_intersection_is_moved_onto_it_when_the_spans_allow() {
    let mut cmds = Vec::new();
    // The south wall is as thick as the columns, so a column on its line stands inside it.
    for w in [wall_t(0, 0, 6000, 0, 300), wall(0, 4000, 6000, 4000), wall(0, 0, 0, 4000), wall(6000, 0, 6000, 4000), wall(3000, 0, 3000, 4000)] {
        cmds.push(create(&w).1);
    }
    let (a, ca) = create(&col(0, 0, None));
    let (b, cb) = create(&col(3000, 150, None));
    let (c, cc) = create(&col(6000, 0, None));
    cmds.extend([ca, cb, cc]);
    let (_, beam1) = create(&Beam { x1: 3000, y1: 150, x2: 3000, y2: 4000, b: 200, h: 400, length: 3850, extra: Default::default() });
    cmds.push(beam1);
    let footing = Footing { x: 3000, y: 150, length: 1500, width: 1500, service_load_kn: 100.0, bearing_kpa: 200.0, column: b.to_string(), extra: Default::default() };
    let (fid, fc) = create(&footing);
    cmds.push(fc);
    let mut doc = doc_with(cmds);
    let _ = (a, c);
    let inp = GridInputs { max_span_mm: Some(7000.0), snap_mm: Some(300.0) };
    let rep = gridopt::analyze(&doc.model, &inp);
    assert_eq!(rep.intersections, 6, "two horizontal walls cross three vertical ones");
    let mv: Vec<_> = rep.proposals.iter().filter(|p| p.status == "move").collect();
    assert_eq!(mv.len(), 1, "{:?}", rep.proposals);
    assert_eq!((mv[0].from, mv[0].to), ([3000, 150], [3000, 0]));
    assert!(mv[0].projection_after_m2 < mv[0].projection_before_m2);
    assert_eq!(rep.proposals.iter().filter(|p| p.status == "kept").count(), 2, "the corner columns have no better place");
    // Applying moves the column, its footing and the end of the beam, as one undoable step; undo puts them back.
    let cmds = gridopt::apply(&doc.model, &rep, 300);
    doc.commit("optimise grid", Origin::User, cmds).unwrap();
    fn pos(doc: &Document, id: Id) -> (i64, i64) {
        let e = doc.model.get(&id).unwrap();
        (e.props["x"].as_i64().unwrap(), e.props["y"].as_i64().unwrap())
    }
    assert_eq!((pos(&doc, b), pos(&doc, fid)), ((3000, 0), (3000, 0)));
    let beam = doc.model.elements.values().find(|e| e.kind == "Beam").unwrap();
    assert_eq!((beam.props["x1"].as_i64(), beam.props["y1"].as_i64(), beam.props["length"].as_i64()), (Some(3000), Some(0), Some(4000)));
    assert!(gridopt::analyze(&doc.model, &inp).proposals.iter().all(|p| p.status != "move"), "nothing left to move");
    doc.undo();
    assert_eq!(pos(&doc, b), (3000, 150));
    // Without the limits nothing moves and they are named.
    let none = gridopt::analyze(&doc.model, &GridInputs::default());
    assert!(none.proposals.iter().all(|p| p.status == "kept") && none.missing.len() == 2 && none.missing[1].contains("maximum span"));
    assert!(GridInputs { max_span_mm: Some(-1.0), snap_mm: None }.check().is_err());
}

#[test]
fn a_column_is_not_moved_where_another_stands() {
    let mut cmds = vec![create(&wall_t(0, 0, 6000, 0, 300)).1, create(&wall(3000, 0, 3000, 4000)).1];
    cmds.push(create(&col(3000, 0, None)).1);
    cmds.push(create(&col(3000, 150, None)).1);
    let doc = doc_with(cmds);
    let rep = gridopt::analyze(&doc.model, &GridInputs { max_span_mm: Some(7000.0), snap_mm: Some(300.0) });
    assert!(rep.proposals.iter().any(|p| p.status == "blocked" && p.reason.contains("already there")), "{:?}", rep.proposals);
}

// ------------------------------------------------------------------ a small structure

struct Frame {
    doc: Document,
    footings: Vec<Id>,
    beams: Vec<Id>,
    slab: Id,
}

/// Four 300 mm columns at the corners of a 6 x 4 m bay with service loads 200, 300, 400 and 100 kN, four beams 300 x 500, a 150 mm
/// slab and a 1500 x 1500 mm footing under each column.
fn frame() -> Frame {
    let pts = [(0, 0, 200.0), (6000, 0, 300.0), (6000, 4000, 400.0), (0, 4000, 100.0)];
    let mut cmds = vec![create(&wall(0, 0, 6000, 0)).1, create(&Space { func: "living".into(), requirement_key: "a".into(), area: 24.0, target_area: 24.0, x: 0, y: 0, w: 6000, h: 4000, boundary: vec![[0, 0], [6000, 0], [6000, 4000], [0, 4000]], net_area: None, glazing_ratio: None, extra: Default::default() }).1];
    let mut cols = Vec::new();
    for (x, y, kn) in pts {
        let (id, c) = create(&col(x, y, Some(kn)));
        cols.push(id);
        cmds.push(c);
    }
    let mut footings = Vec::new();
    for (i, (x, y, kn)) in pts.iter().enumerate() {
        let (id, c) = create(&Footing { x: *x, y: *y, length: 1500, width: 1500, service_load_kn: *kn, bearing_kpa: 200.0, column: cols[i].to_string(), extra: Default::default() });
        footings.push(id);
        cmds.push(c);
    }
    let mut beams = Vec::new();
    for (i, j) in [(0, 1), (1, 2), (2, 3), (3, 0)] {
        let (p, q) = (pts[i], pts[j]);
        let (id, c) = create(&Beam { x1: p.0, y1: p.1, x2: q.0, y2: q.1, b: 300, h: 500, length: (q.0 - p.0).abs() + (q.1 - p.1).abs(), extra: Default::default() });
        beams.push(id);
        cmds.push(c);
    }
    let (slab, c) = create(&Slab { x: 0, y: 0, w: 6000, h: 4000, thickness: 150, kind: "solid".into(), room: "a".into(), extra: Default::default() });
    cmds.push(c);
    Frame { doc: doc_with(cmds), footings, beams, slab }
}

// ------------------------------------------------------------------ slab advisor

fn slab_inputs() -> SlabAdvInputs {
    SlabAdvInputs {
        hourdi: SystemIn { span_depth: Some(20.0), max_span_mm: Some(6000.0), rebar_kg_m3: Some(60.0), extra_rate_m2: Some(40.0) },
        solid: SystemIn { span_depth: Some(25.0), max_span_mm: Some(5000.0), rebar_kg_m3: Some(100.0), extra_rate_m2: Some(25.0) },
        flat: SystemIn { span_depth: Some(28.0), max_span_mm: Some(6000.0), rebar_kg_m3: Some(120.0), extra_rate_m2: Some(30.0) },
        ribbed: RibbedGeom { topping_mm: Some(50.0), rib_width_mm: Some(120.0), rib_spacing_mm: Some(500.0) },
        concrete_rate: Some(300.0),
        steel_rate: Some(3.0),
    }
}

#[test]
fn the_slab_advisor_compares_the_three_systems_on_the_levels_slabs() {
    let f = frame();
    let rep = slabadv::advise(&f.doc.model, &slab_inputs());
    assert_eq!(rep.levels.len(), 1);
    let l = &rep.levels[0];
    // Support width: the beams' 300 mm. Short span 4000 - 300, long span 6000 - 300, area 24 m2.
    assert_eq!((l.short_span_mm, l.long_span_mm, l.support_mm, l.area_m2), (3700.0, 5700.0, 300.0, 24.0));
    let s = |n: &str| l.systems.iter().find(|x| x.system == n).unwrap();
    // Hourdi: 3700 / 20 = 185 -> 200 mm; 50 + 150 * 120 / 500 = 86 mm of concrete per m2 -> 2.06 m3.
    assert_eq!(s("hourdi").depth_mm, Some(200.0));
    near(s("hourdi").concrete_m3.unwrap(), 2.06, 0.005);
    near(s("hourdi").rebar_kg.unwrap(), 2.06 * 60.0, 0.5);
    // Solid: 3700 / 25 = 148 -> 150 mm -> 3.6 m3. Flat: 5700 / 28 = 203.6 -> 210 mm -> 5.04 m3.
    assert_eq!((s("solid").depth_mm, s("flat").depth_mm), (Some(150.0), Some(210.0)));
    near(s("solid").concrete_m3.unwrap(), 3.6, 1e-9);
    near(s("flat").concrete_m3.unwrap(), 5.04, 1e-9);
    near(s("solid").cost.unwrap(), 3.6 * 300.0 + 3.6 * 100.0 * 3.0 + 24.0 * 25.0, 0.05);
    // The span limits: the solid slab is limited to 5000, the 3700 span is within it; the flat slab's 5700 is within 6000.
    assert_eq!((s("solid").within_max_span, s("flat").within_max_span, s("hourdi").within_max_span), (Some(true), Some(true), Some(true)));
    assert_eq!(l.cheapest, Some("hourdi"));
    assert_eq!((l.current_thickness_mm, l.current_m3), (Some(150), 3.6));
    // A system past its own span limit is not chosen even if cheaper.
    let mut tight = slab_inputs();
    tight.hourdi.max_span_mm = Some(3000.0);
    assert_eq!(slabadv::advise(&f.doc.model, &tight).levels[0].cheapest, Some("solid"));
    // Missing inputs are named and that system has no cost.
    let mut part = slab_inputs();
    part.flat.rebar_kg_m3 = None;
    part.concrete_rate = None;
    let r = &slabadv::advise(&f.doc.model, &part).levels[0];
    let flat = r.systems.iter().find(|x| x.system == "flat").unwrap();
    assert!(flat.cost.is_none() && flat.missing.iter().any(|m| m.contains("flat: reinforcement")) && flat.missing.iter().any(|m| m.contains("concrete rate")));
    let none = &slabadv::advise(&f.doc.model, &SlabAdvInputs::default()).levels[0];
    assert!(none.systems.iter().all(|x| x.depth_mm.is_none() && x.cost.is_none() && !x.missing.is_empty()) && none.cheapest.is_none());
    assert!(SlabAdvInputs { ribbed: RibbedGeom { rib_width_mm: Some(500.0), rib_spacing_mm: Some(400.0), topping_mm: None }, ..Default::default() }.check().is_err());
    assert!(slabadv::advise(&Document::default().model, &slab_inputs()).notes[0].contains("no slabs"));
}

// ------------------------------------------------------------------ punching shear and deflection

fn punch_inputs() -> StructuralInputs {
    StructuralInputs { fc_mpa: Some(25.0), load_factor: Some(1.4), phi_shear: Some(0.75), punching_factor: Some(0.33), cover_mm: Some(50.0), bar_dia_mm: Some(16.0), beam_span_depth_max: Some(16.0), slab_span_depth_max: Some(30.0), ..Default::default() }
}

#[test]
fn punching_shear_is_checked_around_each_column_with_a_safety_factor_per_footing() {
    let f = frame();
    let rep = punching::check(&f.doc.model, &punch_inputs(), Some(500.0), None, None);
    assert!(rep.missing.is_empty(), "{:?}", rep.missing);
    assert_eq!(rep.punching.len(), 4);
    // The 400 kN column: d = 500 - 50 - 16 = 434; critical square 734 mm; Pu = 560 kN; Vu = Pu (1 - 0.734^2 / 1.5^2).
    let r = rep.punching.iter().find(|p| p.vu_kn > 400.0 * 1.4 * 0.7 && p.vu_kn < 400.0 * 1.4 * 0.8).expect("the 400 kN footing");
    assert_eq!(r.kind, "footing");
    near(r.d_mm, 434.0, 1e-9);
    near(r.bo_mm, 4.0 * 734.0, 1e-9);
    let vu = 560.0 * (1.0 - 0.734 * 0.734 / 2.25);
    near(r.vu_kn, vu, 0.01);
    near(r.stress_mpa, vu * 1000.0 / (4.0 * 734.0 * 434.0), 0.001);
    near(r.capacity_mpa, 0.33 * 5.0 * 0.75, 0.001);
    near(r.safety, r.capacity_mpa / r.stress_mpa, 0.02);
    assert_eq!(r.status, "ok");
    assert!(r.name.starts_with("FND-") && r.column.starts_with("COL-"));
    // A thin footing under the same load fails; with a higher factor on the load it fails more.
    let thin = punching::check(&f.doc.model, &punch_inputs(), Some(250.0), None, None);
    assert!(thin.punching.iter().all(|p| p.safety < r.safety || p.safety > 0.0) && thin.punching.iter().any(|p| p.status == "fails"), "{:?}", thin.punching.iter().map(|p| p.safety).collect::<Vec<_>>());
    // No phi: the capacity is not reduced, and it is said.
    let mut nophi = punch_inputs();
    nophi.phi_shear = None;
    let r2 = punching::check(&f.doc.model, &nophi, Some(500.0), None, None);
    near(r2.punching[0].capacity_mpa, 0.33 * 5.0, 0.001);
    assert!(r2.notes.iter().any(|n| n.contains("phi")));
    // Missing inputs are named and nothing is computed from them.
    let m = punching::check(&f.doc.model, &StructuralInputs::default(), None, None, None);
    for n in ["f'c", "load factor", "punching shear factor", "cover", "bar diameter", "beam span-to-depth limit", "slab span-to-depth limit"] {
        assert!(m.missing.iter().any(|x| x.contains(n)), "{n} in {:?}", m.missing);
    }
    assert!(m.punching.is_empty() && m.deflection.is_empty());
    let nothick = punching::check(&f.doc.model, &punch_inputs(), None, None, None);
    assert!(nothick.missing.iter().any(|x| x.contains("footing thickness")));
}

#[test]
fn punching_around_a_rectangular_column_matches_the_hand_calculation() {
    // Hand case, not read from the engine: column 300 x 500 on a 2000 x 1600 footing, 600 kN service, thickness 600 mm,
    // cover 50, bar 20 -> d = 530; critical rectangle (300 + 530) x (500 + 530) = 830 x 1030; bo = 2 (830 + 1030) = 3720 mm;
    // Pu = 1.4 x 600 = 840 kN; Vu = 840 (1 - 830 x 1030 / (2000 x 1600)) = 615.58 kN; stress = Vu / (bo d) = 0.3122 MPa;
    // capacity = 0.33 sqrt(25) 0.75 = 1.2375 MPa; safety = 3.963.
    let column = Column { x: 0, y: 0, b: 300, h: 500, grid_ref: String::new(), axial_service_kn: Some(600.0), extra: Default::default() };
    let (cid, cc) = create(&column);
    let footing = Footing { x: 0, y: 0, length: 2000, width: 1600, service_load_kn: 600.0, bearing_kpa: 200.0, column: cid.to_string(), extra: Default::default() };
    let (_, cf) = create(&footing);
    let doc = doc_with(vec![cc, cf]);
    let mut inp = punch_inputs();
    inp.bar_dia_mm = Some(20.0);
    let rep = punching::check(&doc.model, &inp, Some(600.0), None, None);
    assert_eq!(rep.punching.len(), 1, "{:?}", rep.missing);
    let r = &rep.punching[0];
    near(r.d_mm, 530.0, 1e-9);
    near(r.bo_mm, 3720.0, 1e-9);
    near(r.vu_kn, 615.58, 0.01);
    near(r.stress_mpa, 0.3122, 0.001);
    near(r.capacity_mpa, 1.2375, 0.0006);
    near(r.safety, 3.963, 0.01);
    assert_eq!(r.status, "ok");
}

#[test]
fn flat_slab_punching_matches_the_hand_calculation() {
    // Hand case, not read from the engine: a 6 x 4 m slab of 150 mm on four equal corner columns of 300 x 300 mm, so by symmetry each
    // column carries a quarter: 24 / 4 = 6.0 m2. cover 50, bar 16 -> d = 84 mm; critical square 300 + 84 = 384 mm; bo = 4 x 384 = 1536 mm;
    // w = 1.4 (5 + 2) = 9.8 kPa; Vu = 9.8 x 6.0 - 9.8 x 0.384^2 = 58.800 - 1.445 = 57.355 kN; stress = 57355 / (1536 x 84) = 0.4445 MPa;
    // capacity = 0.33 sqrt(25) 0.75 = 1.2375 MPa; safety = 1.2375 / 0.4445 = 2.784.
    let f = frame();
    let mut inp = punch_inputs();
    inp.treat_slabs_as_flat = true;
    let rep = punching::check(&f.doc.model, &inp, Some(500.0), Some(5.0), Some(2.0));
    let slabs: Vec<_> = rep.punching.iter().filter(|p| p.kind == "slab").collect();
    assert_eq!(slabs.len(), 4);
    for r in slabs {
        near(r.d_mm, 84.0, 1e-9);
        near(r.bo_mm, 1536.0, 1e-9);
        near(r.vu_kn, 57.355, 0.01);
        near(r.stress_mpa, 0.4445, 0.001);
        near(r.capacity_mpa, 1.2375, 0.0006);
        near(r.safety, 2.784, 0.01);
        assert_eq!(r.status, "ok");
    }
}

#[test]
fn flat_slab_punching_uses_the_slab_thickness_and_the_floor_loads() {
    let f = frame();
    let mut inp = punch_inputs();
    inp.treat_slabs_as_flat = true;
    let rep = punching::check(&f.doc.model, &inp, Some(500.0), Some(5.0), Some(2.0));
    let s: Vec<_> = rep.punching.iter().filter(|p| p.kind == "slab").collect();
    assert_eq!(s.len(), 4);
    assert!(s.iter().all(|p| p.d_mm == 84.0 && p.name.ends_with(" slab")), "d = 150 - 50 - 16");
    assert!(s.iter().all(|p| p.vu_kn >= 0.0 && p.safety > 0.0));
    let no_loads = punching::check(&f.doc.model, &inp, Some(500.0), None, None);
    assert!(no_loads.missing.iter().any(|m| m.contains("floor loads")));
}

#[test]
fn span_to_depth_ratios_give_a_safety_factor_per_beam_and_slab() {
    let f = frame();
    let rep = punching::check(&f.doc.model, &punch_inputs(), Some(500.0), None, None);
    let beam = rep.deflection.iter().find(|d| d.kind == "beam" && d.span_mm == 6000.0).unwrap();
    near(beam.ratio, 12.0, 1e-9);
    near(beam.safety, 16.0 / 12.0, 0.001);
    assert_eq!(beam.status, "ok");
    let slab = rep.deflection.iter().find(|d| d.kind == "slab").unwrap();
    near(slab.ratio, 4000.0 / 150.0, 0.001);
    assert_eq!(slab.status, "ok");
    let mut tight = punch_inputs();
    tight.beam_span_depth_max = Some(10.0);
    tight.slab_span_depth_max = Some(20.0);
    let t = punching::check(&f.doc.model, &tight, Some(500.0), None, None);
    // The 6 m beams (ratio 12) and the slab (26.7) fail the tighter limits; the 4 m beams (ratio 8) still pass.
    assert!(t.deflection.iter().filter(|d| d.status == "fails").all(|d| d.safety < 1.0) && t.deflection.iter().filter(|d| d.status == "fails").count() >= 3 && t.deflection.iter().any(|d| d.status == "ok"));
    let names: Vec<&str> = rep.deflection.iter().map(|d| d.name.as_str()).collect();
    assert!(names.iter().any(|n| n.starts_with("BM-")) && names.iter().any(|n| n.starts_with("SL-")));
    let _ = (f.slab, &f.beams);
}

// ------------------------------------------------------------------ combined footings and raft

#[test]
fn footings_that_overlap_or_stand_close_are_merged_into_a_combined_footing_with_the_summed_load() {
    let mut f = frame();
    // Under 6000 mm of spacing the footings (1500 wide) are 4500 apart: far. Bring two of them within 100 mm.
    let near_id = f.footings[3];
    f.doc.commit("move footing", Origin::User, vec![Command::SetProp { id: near_id, key: "x".into(), value: Some(serde_json::json!(1600)) }, Command::SetProp { id: near_id, key: "y".into(), value: Some(serde_json::json!(0)) }]).unwrap();
    // Footing 0 is at (0,0) 1500 wide: x in [-750, 750]; the moved one at x 1600: [850, 2350]: a 100 mm gap.
    let inp = MergeInputs { min_gap_mm: Some(200.0), raft_threshold_pct: Some(90.0), raft_edge_mm: Some(300.0) };
    let plan = merger::plan(&f.doc.model, &inp, Some(500.0));
    assert!(!plan.raft && plan.missing.is_empty(), "{:?}", plan.missing);
    assert_eq!(plan.groups.len(), 1);
    let g = &plan.groups[0];
    assert_eq!((g.label.as_str(), g.kind, g.footings.len()), ("CFND-1", "combined", 2));
    assert_eq!(g.rect, [-750, -750, 2350, 750]);
    near(g.load_kn, 200.0 + 100.0, 1e-9);
    assert!(g.provided_m2 + 1e-9 >= g.required_m2);
    // Coverage: four 2.25 m2 footings on a 24 m2 footprint.
    near(plan.coverage_pct.unwrap(), 37.5, 0.05);
    // Applying replaces the two footings by one element that carries both columns, as one undoable step.
    let before = f.doc.model.elements.values().filter(|e| e.kind == "Footing").count();
    let cmds = merger::apply(&f.doc.model, &plan);
    f.doc.commit("merge footings", Origin::User, cmds).unwrap();
    let fs: Vec<_> = f.doc.model.elements.values().filter(|e| e.kind == "Footing").collect();
    assert_eq!(fs.len(), before - 1);
    let m = fs.iter().find(|e| e.props.get("label").and_then(|v| v.as_str()) == Some("CFND-1")).unwrap();
    assert_eq!(m.props["columns"].as_array().unwrap().len(), 2);
    assert_eq!(m.props["mergedFrom"].as_array().unwrap().len(), 2);
    near(m.props["serviceLoadKn"].as_f64().unwrap(), 300.0, 1e-9);
    // The schedules name it by its label and the others in order.
    let names = schedule_names(&f.doc.model, Some(500.0));
    assert_eq!(names[&m.id.to_string()], "CFND-1");
    assert!(names.values().any(|n| n == "FND-1") && names.values().any(|n| n == "FND-2") && !names.values().any(|n| n == "FND-3"));
    let sz = nnx_core::sizing::compute(&f.doc.model, Some(200.0), &nnx_core::sizing::SizingInputs { footing_thickness_mm: Some(500.0), ..Default::default() });
    assert!(sz.footings.iter().any(|r| r.name == "CFND-1" && r.status == "ok"));
    assert!(sz.concrete.iter().any(|c| c.element == "CFND-1"));
    f.doc.undo();
    assert_eq!(f.doc.model.elements.values().filter(|e| e.kind == "Footing").count(), before);
}

#[test]
fn a_raft_replaces_every_footing_when_the_coverage_passes_the_threshold_and_ties_go_with_them() {
    let mut f = frame();
    // A tie beam between footings 0 and 1 refers to both by id: it must go when they merge.
    let (_, tb) = create(&TieBeam { x1: 750, y1: 0, x2: 5250, y2: 0, width: 300, depth: 500, from: f.footings[0].to_string(), to: f.footings[1].to_string(), extra: Default::default() });
    f.doc.commit("tie", Origin::User, vec![tb]).unwrap();
    let inp = MergeInputs { min_gap_mm: Some(10.0), raft_threshold_pct: Some(30.0), raft_edge_mm: Some(300.0) };
    let plan = merger::plan(&f.doc.model, &inp, None);
    assert!(plan.raft);
    assert_eq!(plan.groups.len(), 1);
    let g = &plan.groups[0];
    assert_eq!((g.label.as_str(), g.kind, g.footings.len()), ("RAFT-1", "raft", 4));
    // The raft reaches 300 mm beyond the outermost footings: x -750..6750 and y -750..4750, each side 300 more.
    assert_eq!(g.rect, [-1050, -1050, 7050, 5050]);
    near(g.load_kn, 1000.0, 1e-9);
    let cmds = merger::apply(&f.doc.model, &plan);
    f.doc.commit("raft", Origin::User, cmds).unwrap();
    let fs: Vec<_> = f.doc.model.elements.values().filter(|e| e.kind == "Footing").collect();
    assert_eq!(fs.len(), 1);
    assert_eq!(fs[0].props["label"], "RAFT-1");
    assert!(!f.doc.model.elements.values().any(|e| e.kind == "TieBeam"), "the tie beam went with its footings");
    // Not past the threshold: combined groups only, and with nothing close there is nothing to do.
    let g2 = frame();
    let calm = merger::plan(&g2.doc.model, &MergeInputs { min_gap_mm: Some(100.0), raft_threshold_pct: Some(90.0), raft_edge_mm: Some(300.0) }, None);
    assert!(calm.groups.is_empty() && calm.notes[0].contains("nothing to merge"));
    // Missing inputs are named and nothing is merged.
    let none = merger::plan(&g2.doc.model, &MergeInputs::default(), None);
    assert!(none.groups.is_empty() && none.missing.len() == 3);
    assert!(MergeInputs { raft_threshold_pct: Some(150.0), ..Default::default() }.check().is_err());
}

// ------------------------------------------------------------------ tanks

fn design_in() -> TankDesignIn {
    TankDesignIn { fc_mpa: Some(25.0), fy_mpa: Some(420.0), phi_flexure: Some(0.9), gamma_soil: Some(1.6), gamma_water: Some(1.4), gamma_liquid: Some(1.4), gamma_dead: Some(1.2), concrete_unit_weight_kn_m3: Some(25.0), rho_min: Some(0.0018), rho_max: Some(0.02), cover_mm: Some(50.0), bar_dias_mm: vec![12.0, 16.0, 20.0], min_spacing_mm: Some(100.0), max_spacing_mm: Some(250.0), uplift_fs_min: Some(1.1), min_thickness_mm: Some(200.0), hook_mm: Some(150.0) }
}

fn tank() -> TankIn {
    TankIn {
        name: "T1".into(),
        kind: "water".into(),
        inner_length_mm: Some(4000.0),
        inner_width_mm: Some(3000.0),
        liquid_depth_mm: Some(2000.0),
        freeboard_mm: Some(300.0),
        soil_cover_mm: Some(500.0),
        groundwater_depth_mm: Some(1500.0),
        soil_unit_weight_kn_m3: Some(18.0),
        k0: Some(0.5),
        surcharge_kpa: Some(10.0),
        liquid_unit_weight_kn_m3: Some(9.81),
        compartments: 1,
        wall_mm: Some(250.0),
        base_mm: Some(300.0),
        top_mm: Some(250.0),
        ..Default::default()
    }
}

#[test]
fn the_stress_block_gives_the_steel_for_a_moment() {
    // Mu 50 kN.m/m, d 200, f'c 25, fy 420, phi 0.9: As = 684 mm2/m by the stress block.
    near(tanks::as_for(50.0, 200.0, 25.0, 420.0, 0.9).unwrap(), 684.0, 1.0);
    assert!(tanks::as_for(500.0, 200.0, 25.0, 420.0, 0.9).is_none(), "section too small");
}

#[test]
fn a_tank_is_designed_for_soil_groundwater_and_liquid_with_uplift_checked() {
    let r = tanks::design(&tank(), &design_in());
    assert!(r.missing.is_empty(), "{:?}", r.missing);
    // Full tank: the wall cantilevers from the base under 1.4 * 9.81 * z: M = 1.4 * 9.81 * 2^3 / 6.
    near(r.wall_moment_full_kn_m, 1.4 * 9.81 * 8.0 / 6.0, 0.05);
    assert!(r.wall_moment_empty_kn_m > 0.0);
    // Geometry: outer 4.5 x 3.5, height 2.3 + 0.3 + 0.25; the base is 3.35 m under the ground.
    assert_eq!((r.outer_mm[0], r.outer_mm[1]), (4500.0, 3500.0));
    near(r.depth_below_ground_m, 0.5 + 0.25 + 2.3 + 0.3, 1e-9);
    // Uplift: head 3.35 - 1.5 = 1.85 m on 15.75 m2; resisting: concrete 17.2875 m3 * 25 + soil over the top 15.75 * 0.5 * 18.
    near(r.uplift_kn, 9.81 * 1.85 * 15.75, 0.1);
    near(r.resisting_kn, 17.2875 * 25.0 + 15.75 * 0.5 * 18.0, 0.1);
    near(r.uplift_fs.unwrap(), r.resisting_kn / r.uplift_kn, 0.01);
    assert_eq!(r.status, "ok");
    assert!(r.elements.len() == 3 && r.elements.iter().all(|e| e.status == "ok" && e.as_provided_mm2_m >= e.as_required_mm2_m.max(e.as_min_mm2_m) - 1e-6 && e.spacing_mm >= 100.0 && e.spacing_mm <= 250.0 && e.sized_by == "your thickness"));
    near(r.concrete_m3, 17.2875, 0.01);
    // The schedule: positive counts and lengths, and the total steel is the sum of the rows.
    assert!(r.bars.len() >= 6 && r.bars.iter().all(|b| b.number > 0 && b.cut_length_mm > 0.0 && b.kg > 0.0));
    near(r.steel_kg, r.bars.iter().map(|b| b.kg).sum::<f64>(), 0.05);
    assert!(r.notes.iter().any(|n| n.contains("preliminary")) && r.notes.iter().any(|n| n.contains("fresh water")));
}

#[test]
fn a_base_too_light_for_the_uplift_is_reported_or_thickened_by_the_design() {
    let mut d = design_in();
    d.uplift_fs_min = Some(3.0);
    let r = tanks::design(&tank(), &d);
    // Concrete (25 kN/m3) weighs less than 3 x the water: no thickness reaches 3, and the report says so.
    assert_eq!(r.status, "uplift");
    assert!(r.extra_base_mm == 0.0 && r.notes.iter().any(|n| n.contains("cannot reach it")));
    // A reachable factor: the report names the thickening, and leaving the thickness to the design reaches it.
    d.uplift_fs_min = Some(2.1);
    let r1 = tanks::design(&tank(), &d);
    assert_eq!(r1.status, "uplift");
    assert!(r1.extra_base_mm > 0.0 && r1.notes.iter().any(|n| n.contains("thicken the base")));
    let mut t = tank();
    t.base_mm = None;
    let r2 = tanks::design(&t, &d);
    assert_eq!(r2.status, "ok");
    assert!(r2.uplift_fs.unwrap() >= 2.1 && r2.elements[1].thickness_mm >= 300.0 + r1.extra_base_mm - 100.0 && r2.elements[1].sized_by == "design");
    // No groundwater: no uplift.
    let mut dry = tank();
    dry.groundwater_depth_mm = None;
    let r3 = tanks::design(&dry, &design_in());
    assert!((r3.uplift_kn, r3.uplift_fs) == (0.0, None) && r3.notes.iter().any(|n| n.contains("no groundwater")));
    // Soil pressure on the empty tank without groundwater is smaller than with it.
    assert!(r3.wall_moment_empty_kn_m < r.wall_moment_empty_kn_m);
}

#[test]
fn a_septic_tank_gets_partitions_and_missing_inputs_are_named() {
    let mut t = tank();
    t.kind = "septic".into();
    t.compartments = 3;
    let r = tanks::design(&t, &design_in());
    assert!(r.bars.iter().any(|b| b.mark == "P1") && r.bars.iter().any(|b| b.mark == "P2"));
    assert!(r.concrete_m3 > tanks::design(&tank(), &design_in()).concrete_m3);
    let m = tanks::design(&TankIn { name: "x".into(), ..Default::default() }, &TankDesignIn::default());
    for n in ["inner length", "soil unit weight", "k0", "design: f'c", "design: fy", "design: phi", "design: uplift safety factor", "design: bar diameters"] {
        assert!(m.missing.iter().any(|x| x.contains(n)), "{n} in {:?}", m.missing);
    }
    assert!(m.bars.is_empty() && m.elements.is_empty());
    // A thickness left out needs the smallest one to try.
    let mut d = design_in();
    d.min_thickness_mm = None;
    let mut auto = tank();
    auto.wall_mm = None;
    assert!(tanks::design(&auto, &d).missing.iter().any(|x| x.contains("smallest thickness")));
    assert!(TankIn { kind: "pool".into(), ..Default::default() }.check().is_err());
    assert!(TankDesignIn { rho_min: Some(0.03), rho_max: Some(0.02), ..Default::default() }.check().is_err());
}

// ------------------------------------------------------------------ rebar cutting

fn piece(dia: f64, len: f64, n: u64, tag: &str) -> Piece {
    Piece { dia_mm: dia, length_mm: len, number: n, tag: tag.into() }
}

#[test]
fn bars_are_nested_into_stock_lengths_with_the_waste_reported() {
    let rep = cutting::optimize(&[piece(16.0, 5000.0, 4, "A"), piece(16.0, 3000.0, 6, "B"), piece(16.0, 7000.0, 2, "C")], Some(12000.0), None);
    assert_eq!(rep.per_dia.len(), 1);
    let d = &rep.per_dia[0];
    // 52 m of bars: at least 5 stock bars of 12 m; the packing reaches it.
    assert_eq!((d.pieces, d.lower_bound, d.stock_bars), (12, 5, 5));
    near(d.cut_m, 52.0, 1e-9);
    near(d.stock_m, 60.0, 1e-9);
    near(d.waste_m, 8.0, 1e-9);
    near(d.waste_pct, 13.33, 0.01);
    assert!(d.patterns.iter().all(|p| p.cuts.iter().sum::<f64>() <= 12000.0 + 1e-9 && p.offcut_mm >= 0.0));
    assert_eq!(d.patterns.iter().map(|p| p.cuts.len()).sum::<usize>(), 12);
    near(d.cut_kg, std::f64::consts::PI / 4.0 * 0.016 * 0.016 * 52.0 * 7850.0, 0.05);
    assert!(rep.missing.is_empty() && rep.notes.iter().any(|n| n.contains("offcuts are not reused")));
    // Two diameters are packed separately and totals add up.
    let two = cutting::optimize(&[piece(16.0, 6000.0, 4, "A"), piece(12.0, 4000.0, 3, "B")], Some(12000.0), None);
    assert_eq!(two.per_dia.iter().map(|d| d.dia_mm).collect::<Vec<_>>(), vec![12.0, 16.0]);
    assert_eq!(two.stock_bars, 2 + 1);
}

#[test]
fn a_bar_longer_than_the_stock_is_spliced_with_the_lap_and_without_one_it_is_named() {
    let long = [piece(20.0, 15000.0, 2, "L")];
    let r = cutting::optimize(&long, Some(12000.0), Some(600.0));
    let d = &r.per_dia[0];
    // 15000 + 600 lap = 15600 per bar: 12000 + 3600; two bars -> 2 splices.
    assert_eq!((d.splices, d.pieces), (2, 4));
    near(d.cut_m, 2.0 * 15.6, 1e-9);
    assert!(d.patterns.iter().all(|p| p.tags.iter().all(|t| t.contains("spliced"))));
    let none = cutting::optimize(&long, Some(12000.0), None);
    assert!(none.per_dia.is_empty() && none.missing.iter().any(|m| m.contains("lap length")));
    assert!(cutting::optimize(&long, None, Some(600.0)).missing.iter().any(|m| m.contains("stock bar length")));
    assert!(cutting::optimize(&[], Some(12000.0), None).per_dia.is_empty());
}

#[test]
fn the_schedule_and_the_tank_bars_feed_the_optimizer() {
    let f = frame();
    let sz = nnx_core::sizing::compute(&f.doc.model, Some(200.0), &nnx_core::sizing::SizingInputs { footing_thickness_mm: Some(500.0), rebar: Some(nnx_core::sizing::RebarIn { cover_mm: Some(50.0), anchorage_mm: Some(300.0), hook_mm: Some(100.0), column: Some(nnx_core::sizing::ColumnBars { bar_dia_mm: 16.0, count: 8, tie_dia_mm: 8.0, tie_spacing_mm: 150.0 }), ..Default::default() }), ..Default::default() });
    assert!(!sz.bars.is_empty());
    let mut ps = cutting::pieces_from_schedule(&sz.bars);
    let t = tanks::design(&tank(), &design_in());
    ps.extend(cutting::pieces_from_tank("T1", &t.bars));
    let rep = cutting::optimize(&ps, Some(12000.0), Some(600.0));
    assert!(rep.stock_bars >= rep.per_dia.iter().map(|d| d.lower_bound).sum::<usize>() && rep.stock_kg >= rep.cut_kg && rep.waste_pct >= 0.0);
    assert!(rep.per_dia.iter().all(|d| d.stock_bars >= d.lower_bound));
}

// ------------------------------------------------------------------ analytical export

#[test]
fn the_analytical_model_has_nodes_members_slabs_and_loads_and_exports_to_dxf() {
    let f = frame();
    let a = analytical::build(&f.doc.model, Some(1.4)).unwrap();
    // Four columns: a base and a top node each (ground-level bases are supports); four beams between the top nodes.
    assert_eq!(a.nodes.len(), 8);
    assert_eq!(a.nodes.iter().filter(|n| n.support).count(), 4);
    assert_eq!(a.members.iter().filter(|m| m.kind == "column").count(), 4);
    assert_eq!(a.members.iter().filter(|m| m.kind == "beam").count(), 4);
    assert_eq!(a.surfaces.len(), 1);
    assert_eq!(a.loads.len(), 4);
    let l = a.loads.iter().find(|l| l.service_kn == 400.0).unwrap();
    assert_eq!(l.ultimate_kn, Some(560.0));
    assert_eq!((a.grid_x.len(), a.grid_y.len()), (2, 2));
    let dxf = analytical::dxf(&a, "Test");
    for needle in ["S-GRID", "S-NODE", "S-COLUMN", "S-BEAM", "S-SLAB", "S-LOAD", "S-SUPPORT", "COL-1", "BM-1", "SL-1", "Ps=400 kN Pu=560 kN", "\nEOF\n"] {
        assert!(dxf.contains(needle), "missing {needle:?}");
    }
    assert_eq!(dxf.matches("\nLINE\n").count(), 4 + 4 + a.grid_x.len() + a.grid_y.len());
    assert!(dxf.matches("\nPOLYLINE\n").count() == 1 && dxf.matches("\nSEQEND\n").count() == 1);
    // Without a load factor only the service loads are exported.
    let s = analytical::build(&f.doc.model, None).unwrap();
    assert!(s.loads.iter().all(|l| l.ultimate_kn.is_none()) && s.notes.iter().any(|n| n.contains("no load factor")));
    assert!(analytical::build(&Document::default().model, None).is_err());
}

/// Top-level argument count of a STEP entity `NAME(args)`.
fn arity(args: &str) -> usize {
    let (mut depth, mut inq, mut n) = (0i32, false, 1usize);
    let chars: Vec<char> = args.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\'' if inq && chars.get(i + 1) == Some(&'\'') => i += 1,
            '\'' => inq = !inq,
            '(' if !inq => depth += 1,
            ')' if !inq => depth -= 1,
            ',' if !inq && depth == 0 => n += 1,
            _ => {}
        }
        i += 1;
    }
    n
}

#[test]
fn the_ifc_analytical_view_is_well_formed_with_the_right_attribute_counts_and_valid_references() {
    let f = frame();
    let a = analytical::build(&f.doc.model, Some(1.4)).unwrap();
    let text = analytical::ifc(&a, "Test", "2026-11-19T00:00:00");
    assert!(text.starts_with("ISO-10303-21;") && text.trim_end().ends_with("END-ISO-10303-21;") && text.contains("FILE_SCHEMA(('IFC4'))") && text.contains("StructuralAnalysisView"));
    let expected: std::collections::BTreeMap<&str, usize> = [
        ("IFCSTRUCTURALANALYSISMODEL", 10), ("IFCSTRUCTURALPOINTCONNECTION", 9), ("IFCSTRUCTURALCURVEMEMBER", 9), ("IFCSTRUCTURALSURFACEMEMBER", 9), ("IFCSTRUCTURALPOINTACTION", 10),
        ("IFCSTRUCTURALLOADGROUP", 10), ("IFCSTRUCTURALLOADSINGLEFORCE", 7), ("IFCRELCONNECTSSTRUCTURALMEMBER", 10), ("IFCRELCONNECTSSTRUCTURALACTIVITY", 6), ("IFCRELASSIGNSTOGROUP", 7),
        ("IFCEDGE", 2), ("IFCVERTEXPOINT", 1), ("IFCFACE", 1), ("IFCFACEOUTERBOUND", 2), ("IFCPOLYLOOP", 1), ("IFCTOPOLOGYREPRESENTATION", 4), ("IFCPRODUCTDEFINITIONSHAPE", 3), ("IFCLOCALPLACEMENT", 2),
        ("IFCAXIS2PLACEMENT3D", 3), ("IFCCARTESIANPOINT", 1), ("IFCDIRECTION", 1), ("IFCPROJECT", 9), ("IFCUNITASSIGNMENT", 1), ("IFCSIUNIT", 4),
    ]
    .into_iter()
    .collect();
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let lines: Vec<&str> = text.lines().filter(|l| l.starts_with('#')).collect();
    for (i, line) in lines.iter().enumerate() {
        let (id, rest) = line.split_once('=').unwrap();
        assert_eq!(id, format!("#{}", i + 1), "ids are sequential");
        let (name, args) = rest.split_once('(').unwrap();
        let args = args.strip_suffix(");").unwrap_or_else(|| panic!("{line}"));
        if let Some(n) = expected.get(name) {
            assert_eq!(arity(args), *n, "{name}: {line}");
        }
        *counts.entry(name.to_string()).or_default() += 1;
        // Every reference points at an entity that exists.
        let mut rest = args;
        while let Some(p) = rest.find('#') {
            let num: String = rest[p + 1..].chars().take_while(|c| c.is_ascii_digit()).collect();
            let n: usize = num.parse().unwrap();
            assert!(n >= 1 && n <= lines.len(), "{line}: #{n} does not exist");
            rest = &rest[p + 1..];
        }
    }
    assert_eq!(counts["IFCSTRUCTURALPOINTCONNECTION"], 8);
    assert_eq!(counts["IFCSTRUCTURALCURVEMEMBER"], 8);
    assert_eq!(counts["IFCSTRUCTURALSURFACEMEMBER"], 1);
    assert_eq!(counts["IFCSTRUCTURALPOINTACTION"], 4);
    assert_eq!(counts["IFCRELCONNECTSSTRUCTURALMEMBER"], 16);
    assert!(text.contains("IFCSTRUCTURALLOADSINGLEFORCE('COL-") && text.contains(",-560.,"), "the ultimate load is a downward force in kN");
    let guids: Vec<&str> = text.split('\'').filter(|s| s.len() == 22 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')).collect();
    let mut sorted = guids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), guids.len(), "GUIDs are unique");
}

#[test]
fn the_structural_inputs_refuse_impossible_numbers() {
    assert!(StructuralInputs::default().check().is_ok());
    for bad in [
        StructuralInputs { fc_mpa: Some(0.0), ..Default::default() },
        StructuralInputs { load_factor: Some(-1.0), ..Default::default() },
        StructuralInputs { cutting: nnx_core::structx::CuttingIn { stock_mm: Some(0.0), lap_mm: None }, ..Default::default() },
        StructuralInputs { cutting: nnx_core::structx::CuttingIn { stock_mm: None, lap_mm: Some(-5.0) }, ..Default::default() },
        StructuralInputs { tanks: vec![TankIn { kind: "x".into(), ..Default::default() }], ..Default::default() },
    ] {
        assert!(bad.check().is_err(), "{bad:?}");
    }
}
