//! The MEP router, the load tables behind it, and the structural schedules. Every input is SYNTHETIC test data.

use nnx_core::clash::{self, ClashOpts};
use nnx_core::mep::{self, MepInputs};
use nnx_core::sizing::{self, SizingInputs};
use nnx_core::structural::{self, Loads, Soil, StructOpts};
use nnx_core::typed::{Beam, Column, MepRun, Slab, Space, Wall, element_of};
use nnx_core::*;
use serde_json::{Value, json};

/// Two 6 x 4 m rooms side by side (a: living, the air handler's room; b: bathroom), exterior walls, a partition on x = 6 m
/// with a beam under the slab along it and a column at its middle. Walls are 3 m high.
struct Plan {
    doc: Document,
    beam: Id,
    column: Id,
}

fn space(key: &str, func: &str, x: i64) -> Space {
    Space { func: func.into(), requirement_key: key.into(), area: 24.0, target_area: 24.0, x, y: 0, w: 6000, h: 4000, boundary: vec![[x, 0], [x + 6000, 0], [x + 6000, 4000], [x, 4000]], net_area: None, glazing_ratio: None, extra: Default::default() }
}

fn wall(x1: i64, y1: i64, x2: i64, y2: i64, a: &str, b: Option<&str>, exterior: bool) -> Wall {
    Wall { x1, y1, x2, y2, thickness: 200, height: 3000, length: (x2 - x1).abs() + (y2 - y1).abs(), is_exterior: exterior, room_a: a.into(), room_b: b.map(str::to_string), extra: Default::default() }
}

fn plan() -> Plan {
    let mut cmds = vec![Command::Create(element_of(&space("a", "living", 0), Source::User)), Command::Create(element_of(&space("b", "bathroom", 6000), Source::User))];
    for (x1, y1, x2, y2, a, b, ext) in [
        (0, 0, 12000, 0, "a", None, true),
        (12000, 0, 12000, 4000, "b", None, true),
        (12000, 4000, 0, 4000, "b", None, true),
        (0, 4000, 0, 0, "a", None, true),
        (6000, 0, 6000, 4000, "a", Some("b"), false),
    ] {
        cmds.push(Command::Create(element_of(&wall(x1, y1, x2, y2, a, b, ext), Source::User)));
    }
    for (key, x) in [("a", 0), ("b", 6000)] {
        cmds.push(Command::Create(element_of(&Slab { x, y: 0, w: 6000, h: 4000, thickness: 150, kind: "solid".into(), room: key.into(), extra: Default::default() }, Source::User)));
    }
    let beam = element_of(&Beam { x1: 6000, y1: 0, x2: 6000, y2: 4000, b: 250, h: 450, length: 4000, extra: Default::default() }, Source::User);
    let column = element_of(&Column { x: 6000, y: 2000, b: 300, h: 300, grid_ref: "B1".into(), axial_service_kn: None, extra: Default::default() }, Source::User);
    let ids = (beam.id, column.id);
    cmds.push(Command::Create(beam));
    cmds.push(Command::Create(column));
    let mut doc = Document::default();
    doc.commit("plan", Origin::User, cmds).unwrap_or_else(|e| panic!("{e}"));
    Plan { doc, beam: ids.0, column: ids.1 }
}

fn inputs() -> Value {
    json!({
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
            "pipes": [{"name": "PPR 20", "inner_mm": 16}, {"name": "PPR 25", "inner_mm": 21}],
            "drain_table": [{"max_dfu": 3, "dia_mm": 40}, {"max_dfu": 8, "dia_mm": 75}]},
        "hvac": {"indoor_c": 24, "outdoor_c": 45, "window_u": 3.0, "window_shgc": 0.5, "solar_w_m2": 400,
            "person_w": 100, "area_per_person_m2": 10, "lights_w_m2": 10, "equipment_w_m2": 5,
            "vent_l_s_per_m2": 1, "safety_factor": 1.1, "unit_sizes_w": [2000, 3500, 5000, 9000], "roof_exposed": true, "cop": 3.0},
        "routing": {"ahu_room": "a", "supply_delta_t_c": 10, "duct_velocity_mps": 5, "duct_height_mm": 300, "ceiling_height_mm": 2600, "return_offset_mm": 400,
            "drain_slope_pct": 1.5, "drain_invert_mm": 400, "stack_offset_mm": 1500,
            "tank": {"kind": "ground", "x_mm": -2000, "y_mm": 2000, "z_mm": 0}, "supply_z_mm": 600,
            "tray_z_mm": 2800, "tray_width_mm": 300, "tray_height_mm": 100}
    })
}

fn parse(v: &Value) -> MepInputs {
    serde_json::from_value(v.clone()).unwrap()
}

fn near(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol, "{a} vs {b}");
}

#[test]
fn cooling_loads_come_out_in_watts_btu_and_tons_with_the_supply_airflow() {
    let p = plan();
    let rep = mep::compute(&p.doc.model, &parse(&inputs()));
    assert!(rep.missing.is_empty(), "{:?}", rep.missing);
    let h = rep.hvac.as_ref().unwrap();
    assert_eq!(h.rooms.len(), 2);
    for r in &h.rooms {
        near(r.btu_hr, r.load_w * 3.412142, 0.05);
        near(r.tons, r.load_w / 3516.85, 0.01);
        // 10 K supply difference: L/s = W / (1.2 kg/m3 x 1006 J/kgK x 10 K) x 1000
        near(r.airflow_lps.unwrap(), r.load_w / (1.2 * 1006.0 * 10.0) * 1000.0, 0.01);
    }
    near(h.total_btu_hr, h.rooms.iter().map(|r| r.load_w).sum::<f64>() * 3.412142, 0.1);
    near(h.total_tons, h.total_w / 3516.85, 0.01);
}

#[test]
fn ducts_run_under_the_soffit_with_diffusers_and_the_clash_check_finds_the_beam_and_the_column() {
    let mut p = plan();
    let rep = mep::compute(&p.doc.model, &parse(&inputs()));
    let supply: Vec<_> = rep.runs.iter().filter(|r| r.system == "duct" && r.role == "supply").collect();
    assert_eq!(supply.len(), 1, "a trunk only: both rooms are on the handler's line: {supply:?}");
    let trunk = supply[0];
    assert_eq!((trunk.p1, trunk.p2), ([3000, 2000, 2850], [9000, 2000, 2850]), "centres of both rooms, 300 mm duct hard under the 3000 mm soffit");
    assert_eq!(trunk.height, 300);
    // Width from the airflow: area = L/s / 1000 / 5 m/s, divided by 300 mm, rounded up to 50 mm.
    let q: f64 = rep.hvac.as_ref().unwrap().rooms.iter().map(|r| r.load_w / (1.2 * 1006.0 * 10.0) * 1000.0).sum();
    assert_eq!(trunk.width, ((q / 1000.0 / 5.0 * 1e6 / 300.0 / 50.0).ceil() * 50.0) as i64);
    // Diffusers at the room centres, grilles beside them inside the rooms.
    let pts: Vec<_> = rep.points.iter().filter(|p| p.role == "diffuser").collect();
    assert_eq!(pts.len(), 2);
    assert!(pts.iter().any(|p| (p.x, p.y, p.room.as_str()) == (3000, 2000, "a")) && pts.iter().any(|p| (p.x, p.y, p.room.as_str()) == (9000, 2000, "b")));
    assert!(rep.points.iter().filter(|p| p.role == "grille").all(|g| (g.x >= 0 && g.x <= 12000) && g.discipline == Discipline::Hvac));

    let (created, _) = mep::apply_with_runs(&mut p.doc, &rep.points, &rep.runs).unwrap();
    assert!(created > 10);
    let scene = nnx_core::model3d::build(&p.doc.model);
    let report = clash::detect(&scene, &ClashOpts::default());
    let supply_id = p.doc.model.elements.values().find(|e| e.kind == "MepRun" && e.props["system"] == "duct" && e.props["role"] == "supply").unwrap().id.to_string();
    let hits: Vec<_> = report.clashes.iter().filter(|c| c.run_id == supply_id).map(|c| c.struct_id.as_str()).collect();
    assert!(hits.contains(&p.beam.to_string().as_str()), "the duct crosses the beam under the slab: {hits:?}");
    assert!(hits.contains(&p.column.to_string().as_str()), "and the column at y = 2 m: {hits:?}");
    // Every generated run is of its own discipline and in the scene.
    assert!(scene.boxes.iter().any(|b| b.kind == "MepPoint" && b.class == "diffuser" && b.discipline == "hvac"));
}

#[test]
fn a_duct_that_does_not_fit_the_ceiling_void_is_not_routed_and_says_so() {
    let p = plan();
    let mut i = inputs();
    i["routing"]["duct_height_mm"] = json!(500);
    let rep = mep::compute(&p.doc.model, &parse(&i));
    assert!(rep.runs.iter().all(|r| r.system != "duct"));
    assert!(rep.warnings.iter().any(|w| w.contains("does not fit the ceiling void")), "{:?}", rep.warnings);
}

#[test]
fn drains_fall_by_the_given_slope_from_each_wet_room_to_a_stack_outside_the_building() {
    let p = plan();
    let rep = mep::compute(&p.doc.model, &parse(&inputs()));
    let drains: Vec<_> = rep.runs.iter().filter(|r| r.system == "drain").collect();
    assert_eq!(drains.len(), 1, "only the bathroom has fixtures");
    let d = drains[0];
    // Room b's centre is (9000, 2000): the nearest sides are south and north (2 m); the first is taken. Stack 1.5 m outside.
    assert_eq!((d.p1, d.room.as_deref()), ([9000, 2000, -400], Some("b")));
    assert_eq!((d.p2[0], d.p2[1]), (9000, -1500));
    let fall = (d.p1[2] - d.p2[2]) as f64;
    near(fall / 3500.0 * 100.0, 1.5, 0.05);
    assert_eq!((d.width, d.height), (75, 75), "4 fixture units need the 75 mm row of the drain table");
    let row = rep.routes.iter().find(|r| r.system == "drain").unwrap();
    near(row.slope_pct.unwrap(), 1.5, 0.05);
    near(row.length_m, (3500f64.powi(2) + fall.powi(2)).sqrt() / 1000.0, 0.01);
}

#[test]
fn the_water_supply_comes_from_the_tank_through_a_riser_a_trunk_and_branches_sized_by_the_pipe_table() {
    let p = plan();
    let rep = mep::compute(&p.doc.model, &parse(&inputs()));
    let riser = rep.runs.iter().find(|r| r.system == "pipe" && r.role == "riser").expect("the tank is at z = 0, the supply at 600");
    assert_eq!((riser.p1, riser.p2), ([-2000, 2000, 0], [-2000, 2000, 600]));
    let trunk = rep.runs.iter().find(|r| r.system == "pipe" && r.role == "trunk").unwrap();
    assert_eq!((trunk.p1, trunk.p2), ([-2000, 2000, 600], [9000, 2000, 600]));
    // 0.1 L/s x 0.5 simultaneity = 0.05 L/s; at 2 m/s the smallest inner diameter is 5.6 mm, so the first row (16 mm) serves.
    assert_eq!((trunk.width, trunk.label.as_deref()), (16, Some("PPR 20 supply trunk")));
}

#[test]
fn circuits_go_to_the_nearest_board_with_kva_breakers_cables_and_dedicated_ac_circuits() {
    let mut p = plan();
    let rep = mep::compute(&p.doc.model, &parse(&inputs()));
    let el = rep.electrical.as_ref().unwrap();
    assert_eq!(el.boards.len(), 2, "both boards are used");
    // Every circuit is on the board of the room it sits in (rooms a and b have a board each).
    for c in el.circuits.iter().filter(|c| c.kind != "hvac") {
        assert_eq!(c.board, c.rooms[0], "{c:?}");
    }
    let ac: Vec<_> = el.circuits.iter().filter(|c| c.kind == "hvac").collect();
    assert_eq!(ac.len(), 2, "one dedicated circuit per air-conditioned room");
    let h = rep.hvac.as_ref().unwrap();
    for (c, r) in ac.iter().zip(&h.rooms) {
        near(c.watts, r.load_w / 3.0, 0.01);
        assert!(c.breaker_a >= c.current_a * 1.25 - 1e-9 && c.cable_mm2 > 0.0, "{c:?}");
    }
    // The AC units point at their circuits.
    assert!(rep.points.iter().filter(|p| p.role == "ac_indoor").all(|p| p.circuit.as_deref().is_some_and(|c| c.starts_with("AC"))));
    // kVA = W / power factor / 1000, lights and sockets at the demand factor, air conditioning at 100 %.
    let (lights_sockets, ac_w): (f64, f64) = (el.circuits.iter().filter(|c| c.kind != "hvac").map(|c| c.watts).sum(), ac.iter().map(|c| c.watts).sum());
    near(el.connected_w, lights_sockets + ac_w, 0.05);
    near(el.demand_w, lights_sockets * 0.5 + ac_w, 0.05);
    near(el.demand_kva, el.demand_w / 0.9 / 1000.0, 0.01);
    near(el.boards.iter().map(|b| b.demand_w).sum::<f64>(), el.demand_w, 0.05);
    assert!(rep.warnings.iter().any(|w| w.contains("100 % demand")));
    // Cable trays go from each board along y = board and out to each circuit.
    let trays: Vec<_> = rep.runs.iter().filter(|r| r.system == "cable").collect();
    assert!(!trays.is_empty() && trays.iter().all(|t| t.p1[2] == 2800 && t.width == 300 && t.height == 100));
    mep::apply_with_runs(&mut p.doc, &rep.points, &rep.runs).unwrap();
    let scene = nnx_core::model3d::build(&p.doc.model);
    assert!(scene.boxes.iter().any(|b| b.kind == "MepRun" && b.class == "cable" && b.discipline == "electrical"));
}

#[test]
fn generated_runs_are_replaced_in_one_undoable_step_and_user_runs_are_never_touched() {
    let mut p = plan();
    // A run the user entered.
    let user = nnx_core::typed::run_element(&MepRun { system: "pipe".into(), x1: 100, y1: 100, z1: 500, x2: 900, y2: 100, z2: 500, width: 20, height: 20, room: None, role: None, label: None, extra: Default::default() }, None, Source::User).unwrap();
    let uid = user.id;
    p.doc.commit("user run", Origin::User, vec![Command::Create(user)]).unwrap();
    let rep = mep::compute(&p.doc.model, &parse(&inputs()));
    let before = p.doc.model.state_hash();
    let (created, removed) = mep::apply_with_runs(&mut p.doc, &rep.points, &rep.runs).unwrap();
    assert!(created > 0 && removed == 0);
    assert!(p.doc.model.get(&uid).is_some());
    // The same result again changes nothing.
    assert_eq!(mep::apply_with_runs(&mut p.doc, &rep.points, &rep.runs).unwrap(), (0, 0));
    // One undo removes every generated point and run.
    assert!(p.doc.undo());
    assert_eq!(p.doc.model.state_hash(), before);
    // Without routing the generated runs are removed again.
    assert!(p.doc.redo());
    let mut no_route = inputs();
    no_route.as_object_mut().unwrap().remove("routing");
    let rep2 = mep::compute(&p.doc.model, &parse(&no_route));
    assert!(rep2.runs.is_empty());
    mep::apply_with_runs(&mut p.doc, &rep2.points, &rep2.runs).unwrap();
    assert_eq!(p.doc.model.elements.values().filter(|e| e.kind == "MepRun").count(), 1, "only the user's run is left");
}

#[test]
fn a_system_with_an_input_missing_is_not_routed_and_the_names_are_listed() {
    let p = plan();
    let mut i = inputs();
    i["routing"].as_object_mut().unwrap().remove("drain_slope_pct");
    i["routing"].as_object_mut().unwrap().remove("tank");
    i["routing"].as_object_mut().unwrap().remove("tray_height_mm");
    i["routing"].as_object_mut().unwrap().remove("duct_velocity_mps");
    i["hvac"].as_object_mut().unwrap().remove("cop");
    let rep = mep::compute(&p.doc.model, &parse(&i));
    assert!(rep.missing["routing.drainage"].contains(&"routing.drain_slope_pct".to_string()));
    assert!(rep.missing["routing.supply"].contains(&"routing.tank".to_string()));
    assert!(rep.missing["routing.cables"].contains(&"routing.tray_height_mm".to_string()));
    assert!(rep.missing["routing.hvac"].contains(&"routing.duct_velocity_mps".to_string()));
    assert!(rep.runs.is_empty(), "{:?}", rep.routes);
    assert!(rep.warnings.iter().any(|w| w.contains("hvac.cop")), "no dedicated AC circuits without a cop: {:?}", rep.warnings);
    // The airflow is still reported: it needs only the supply temperature difference.
    assert!(rep.hvac.unwrap().rooms[0].airflow_lps.is_some());
}

#[test]
fn a_falling_pipe_is_valid_but_a_falling_duct_or_a_diagonal_is_not() {
    let mut p = plan();
    let mk = |system: &str, p2: [i64; 3]| nnx_core::typed::run_element(&MepRun { system: system.into(), x1: 0, y1: 0, z1: 0, x2: p2[0], y2: p2[1], z2: p2[2], width: 100, height: 100, room: None, role: None, label: None, extra: Default::default() }, None, Source::User).unwrap();
    assert!(p.doc.commit("a", Origin::User, vec![Command::Create(mk("drain", [1000, 0, -15]))]).is_ok(), "a drain falls along its run");
    assert!(p.doc.commit("b", Origin::User, vec![Command::Create(mk("duct", [1000, 0, -15]))]).unwrap_err().to_string().contains("only a pipe or a drain"));
    assert!(p.doc.commit("c", Origin::User, vec![Command::Create(mk("pipe", [1000, 1000, 0]))]).is_err());
    assert!(p.doc.commit("d", Origin::User, vec![Command::Create(mk("cable", [1000, 0, 0]))]).is_ok());
}

// ------------------------------------------------------------------ structural schedules

/// The plan with its structure made by the engine: loads 5 + 2 kPa and a soil value, so columns carry loads and footings exist.
fn structured() -> Document {
    let mut p = plan();
    // Remove the hand-made column, beam and slabs so the engine's own structure is the only one.
    let ids: Vec<Id> = p.doc.model.elements.values().filter(|e| matches!(e.kind.as_str(), "Beam" | "Column" | "Slab")).map(|e| e.id).collect();
    p.doc.commit("clear", Origin::User, ids.into_iter().map(Command::Delete).collect()).unwrap();
    let opts = StructOpts { loads: Some(Loads { dead_kpa: 5.0, live_kpa: 2.0, source: "test".into() }), soil: Some(Soil { bearing_kpa: 150.0, source: "test".into() }), ..StructOpts::default() };
    structural::sync(&mut p.doc, &opts, None).unwrap();
    p.doc
}

fn sizing_inputs() -> SizingInputs {
    serde_json::from_value(json!({
        "footing_thickness_mm": 500,
        "tie_beam": {"width_mm": 250, "span_depth_ratio": 12},
        "rebar": {"cover_mm": 40, "anchorage_mm": 300, "hook_mm": 100,
            "column": {"bar_dia_mm": 16, "count": 8, "tie_dia_mm": 8, "tie_spacing_mm": 200},
            "footing": {"bar_dia_mm": 12, "spacing_mm": 150},
            "beam": {"top_dia_mm": 16, "top_count": 2, "bottom_dia_mm": 16, "bottom_count": 3, "stirrup_dia_mm": 8, "stirrup_spacing_mm": 150}}
    }))
    .unwrap()
}

#[test]
fn column_loads_follow_the_tributary_area_and_the_schedule_names_every_element_by_id() {
    let doc = structured();
    let rep = sizing::compute(&doc.model, Some(150.0), &sizing_inputs());
    assert!(rep.missing.is_empty(), "{:?}", rep.missing);
    assert_eq!(rep.columns.len(), 6, "a column at each of the six wall junctions");
    // Two 24 m2 slabs: corner columns carry a quarter of one slab, the two middle ones half of each.
    let areas: Vec<f64> = rep.columns.iter().map(|c| c.tributary_m2).collect();
    near(areas.iter().sum::<f64>(), 48.0, 1e-6);
    for c in &rep.columns {
        let expect = if c.x == 6000 { 12.0 } else { 6.0 };
        near(c.tributary_m2, expect, 1e-6);
        near(c.axial_kn.unwrap(), expect * 7.0, 0.1);
        assert!(c.name.starts_with("COL-") && !c.id.is_empty());
    }
    // Footings: one per column, named and checked against the soil value; none is undersized.
    assert_eq!(rep.footings.len(), 6);
    for f in &rep.footings {
        assert!(f.name.starts_with("FND-") && f.column.starts_with("COL-") && f.status == "ok", "{f:?}");
        near(f.required_m2.unwrap(), f.load_kn / 150.0, 1e-3);
    }
    // A bearing value too low for the footings the model holds flags them.
    let weak = sizing::compute(&doc.model, Some(500.0), &sizing_inputs());
    assert!(weak.footings.iter().all(|f| f.status == "ok"), "a stronger soil needs less");
    let tight = sizing::compute(&doc.model, Some(10.0), &sizing_inputs());
    assert!(tight.footings.iter().all(|f| f.status == "undersized"), "a weaker soil than the one used: all undersized");
}

#[test]
fn tie_beams_join_neighbouring_footings_and_concrete_and_steel_add_up_by_hand() {
    let doc = structured();
    let rep = sizing::compute(&doc.model, Some(150.0), &sizing_inputs());
    // 6 footings on a 3 x 2 grid: 2 per row x 2 rows, plus 1 per column line x 3 = 7 ties.
    assert_eq!(rep.tie_beams.len(), 7);
    let long = rep.tie_beams.iter().find(|t| t.span_mm == 6000).unwrap();
    assert_eq!((long.width_mm, long.depth_mm), (250, 500), "6000 / 12 = 500");
    let short = rep.tie_beams.iter().find(|t| t.span_mm == 4000).unwrap();
    assert_eq!(short.depth_mm, 350, "4000 / 12 = 333 rounded up to 350");
    assert!(rep.tie_beams.iter().all(|t| t.clear_mm < t.span_mm && !t.from_id.is_empty() && !t.to_id.is_empty()));

    // Column concrete: 6 x (0.3 x 0.3 x 3.0) m3.
    let col: f64 = rep.concrete.iter().filter(|c| c.kind == "Column").map(|c| c.volume_m3).sum();
    near(col, 6.0 * 0.3 * 0.3 * 3.0, 0.01);
    let fnd: f64 = rep.concrete.iter().filter(|c| c.kind == "Footing").map(|c| c.volume_m3).sum();
    assert!(fnd > 0.0);
    near(rep.concrete_total_m3, rep.concrete.iter().map(|c| c.volume_m3).sum::<f64>(), 0.01);
    // Column bars: 8 x (3000 + 300 anchorage) mm of 16 mm bar = 26.4 m, weighing pi/4 x 0.016^2 x 26.4 x 7850 kg.
    let v = rep.bars.iter().find(|b| b.mark == "COL-1/V").unwrap();
    assert_eq!((v.number, v.cut_length_mm, v.dia_mm), (8, 3300.0, 16.0));
    near(v.kg, std::f64::consts::PI / 4.0 * 0.016f64.powi(2) * 26.4 * 7850.0, 0.01);
    // Ties: perimeter 2 x (220 + 220) + 2 x 100 = 1080 mm, floor(3000 / 200) + 1 = 16 of them.
    let t = rep.bars.iter().find(|b| b.mark == "COL-1/T").unwrap();
    assert_eq!((t.number, t.cut_length_mm), (16, 1080.0));
    near(rep.steel_total_kg, rep.bars.iter().map(|b| b.kg).sum::<f64>(), 0.01);
    // Every row is tied to an element id (tie beams have none: they are not model elements).
    assert!(rep.bars.iter().filter(|b| !b.element.starts_with("TB-")).all(|b| b.id.is_some()));
    assert!(rep.concrete.iter().filter(|c| c.kind != "TieBeam").all(|c| c.id.is_some()));
    // The exports carry the ids and a total.
    assert!(rep.bbs_csv().contains("COL-1/V") && rep.bbs_csv().contains("TOTAL"));
    assert!(rep.concrete_csv().lines().count() > 6 && rep.members_csv().contains("FND-1"));
}

#[test]
fn missing_inputs_are_named_and_nothing_is_invented() {
    let doc = structured();
    let rep = sizing::compute(&doc.model, None, &SizingInputs::default());
    assert!(rep.missing["footing check"][0].contains("soil bearing"));
    assert!(rep.missing["tie beams"].contains(&"tie_beam.width_mm".to_string()) && rep.missing["tie beams"].contains(&"tie_beam.span_depth_ratio".to_string()));
    assert!(rep.missing["footing concrete"][0].contains("footing_thickness_mm"));
    assert!(rep.missing["reinforcement"][0].contains("rebar"));
    assert!(rep.tie_beams.is_empty() && rep.bars.is_empty());
    assert!(rep.footings.iter().all(|f| f.status == "no soil value"));
    let partial = SizingInputs { rebar: Some(Default::default()), ..SizingInputs::default() };
    let r2 = sizing::compute(&doc.model, Some(150.0), &partial);
    assert_eq!(r2.missing["reinforcement"], vec!["rebar.cover_mm", "rebar.anchorage_mm", "rebar.hook_mm"]);
    // A model with no structure says what to do.
    let bare = plan();
    let r3 = sizing::compute(&bare.doc.model, None, &SizingInputs::default());
    assert!(r3.columns.len() == 1, "the hand-made column is read as it is");
    assert!(sizing::compute(&Model::default(), None, &SizingInputs::default()).warnings.iter().any(|w| w.contains("no columns")));
}

#[test]
fn the_take_down_now_shares_a_slab_by_half_spans_and_still_balances() {
    // One slab 6 x 4 m with columns at the four corners and one at mid-span of a long side.
    let mut doc = Document::lenient();
    let slab = Slab { x: 0, y: 0, w: 6000, h: 4000, thickness: 150, kind: "solid".into(), room: "r".into(), extra: Default::default() };
    let mut cmds = vec![Command::Create(element_of(&slab, Source::User))];
    let mut ids = Vec::new();
    for (x, y) in [(0, 0), (6000, 0), (6000, 4000), (0, 4000), (3000, 0)] {
        let el = element_of(&Column { x, y, b: 300, h: 300, grid_ref: String::new(), axial_service_kn: None, extra: Default::default() }, Source::User);
        ids.push(el.id);
        cmds.push(Command::Create(el));
    }
    doc.commit("c", Origin::User, cmds).unwrap();
    let t = structural::column_tributary(&doc.model);
    // Perimeter 20 m. Corner (0,0): half of 3 m and half of 4 m = 3.5 m; (3000,0): 3 m; (6000,0): 3.5 m; (6000,4000): 5 m; (0,4000): 5 m.
    near(t[&ids[0]], 24.0 * 3.5 / 20.0, 1e-6);
    near(t[&ids[4]], 24.0 * 3.0 / 20.0, 1e-6);
    near(t[&ids[1]], 24.0 * 3.5 / 20.0, 1e-6);
    near(t[&ids[2]], 24.0 * 5.0 / 20.0, 1e-6);
    near(t[&ids[3]], 24.0 * 5.0 / 20.0, 1e-6);
    near(t.values().sum::<f64>(), 24.0, 1e-9);
}
