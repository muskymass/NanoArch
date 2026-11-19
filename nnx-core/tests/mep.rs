//! MEP and insulation. All input values are SYNTHETIC test values, chosen so every result can be checked by hand.

use nnx_core::insulation::{self, InsulationLib};
use nnx_core::mep::{self, MepInputs};
use nnx_core::typed::{Beam, Column, Door, MepPoint, Slab, Space, Typed, Wall, Window, element_of};
use nnx_core::*;

struct Golden {
    doc: Document,
}

/// One 4 m x 5 m room, a 4 m exterior wall with a door and a window, a 4 m partition, one column, one beam.
fn golden() -> Golden {
    let mut doc = Document::default();
    let space = Space {
        func: "bedroom".into(),
        requirement_key: "r1".into(),
        area: 20.0,
        target_area: 20.0,
        x: 0,
        y: 0,
        w: 4000,
        h: 5000,
        boundary: vec![[0, 0], [4000, 0], [4000, 5000], [0, 5000]],
        net_area: Some(18.5),
        glazing_ratio: None,
        extra: Default::default(),
    };
    let slab = Slab { x: 0, y: 0, w: 4000, h: 5000, thickness: 150, kind: "solid".into(), room: "r1".into(), extra: Default::default() };
    let ext = Wall {
        x1: 0,
        y1: 0,
        x2: 4000,
        y2: 0,
        thickness: 200,
        height: 3000,
        length: 4000,
        is_exterior: true,
        room_a: "r1".into(),
        room_b: None,
        extra: Default::default(),
    };
    let int = Wall { y1: 5000, y2: 5000, thickness: 100, is_exterior: false, ..ext.clone() };
    let column = Column { x: 150, y: 150, b: 300, h: 300, grid_ref: "A1".into(), axial_service_kn: None, extra: Default::default() };
    let beam = Beam { x1: 0, y1: 0, x2: 4000, y2: 0, b: 200, h: 500, length: 4000, extra: Default::default() };
    let ext_el = element_of(&ext, Source::User);
    let ext_id = ext_el.id;
    let door = Door { width: 900, height: 2100, offset_center: 1000, is_egress: true, from: "r1".into(), to: "outside".into(), opens_into: "r1".into(), extra: Default::default() };
    let window = Window { width: 1500, height: 1200, sill_height: 900, offset_center: 3000, room: "r1".into(), extra: Default::default() };
    let cmds = vec![
        Command::Create(element_of(&space, Source::User)),
        Command::Create(element_of(&slab, Source::User)),
        Command::Create(ext_el),
        Command::Create(element_of(&int, Source::User)),
        Command::Create(element_of(&door, Source::User).hosted_by(ext_id)),
        Command::Create(element_of(&window, Source::User).hosted_by(ext_id)),
        Command::Create(element_of(&column, Source::User)),
        Command::Create(element_of(&beam, Source::User)),
    ];
    doc.commit("golden", Origin::User, cmds).unwrap();
    Golden { doc }
}

fn lib_json() -> serde_json::Value {
    serde_json::json!({
        "assemblies": {
            "wall": {"rsi": 0.13, "rse": 0.04, "layers": [
                {"material": "plaster", "thickness_mm": 20, "lambda": 0.7},
                {"material": "block", "thickness_mm": 200, "lambda": 1.0},
                {"material": "eps", "thickness_mm": 50, "lambda": 0.04, "kind": "insulation"}]},
            "roof": {"rsi": 0.1, "rse": 0.04, "layers": [{"material": "concrete", "thickness_mm": 150, "lambda": 1.5}]}
        },
        "u_limits": {"wall": 0.5}
    })
}

fn lib() -> InsulationLib {
    serde_json::from_value(lib_json()).unwrap()
}

fn inputs() -> MepInputs {
    serde_json::from_value(serde_json::json!({
        "insulation": lib_json(),
        "electrical": {"voltage_v": 230, "power_factor": 1, "three_phase": false, "lights_per_m2": 0.5, "light_w": 10,
            "sockets_per_m": 0.5, "socket_w": 200, "demand_factor": 0.5, "max_light_points": 12, "max_socket_points": 6,
            "breaker_margin": 1.25, "breaker_sizes_a": [6, 10, 16, 32],
            "cables": [{"amps": 10, "mm2": 1.5}, {"amps": 16, "mm2": 2.5}, {"amps": 32, "mm2": 6}],
            "resistivity": 0.0175, "max_vdrop_pct": 3, "panel_room": "r1", "route_factor": 1.2},
        "plumbing": {"catalog": {"wc": {"flow_lps": 0.1, "dfu": 4}, "basin": {"flow_lps": 0.1, "dfu": 1}},
            "fixtures_by_func": {"bedroom": {"wc": 1, "basin": 2}}, "simultaneity": 0.5, "max_velocity_mps": 2,
            "pipes": [{"name": "15", "inner_mm": 12}, {"name": "20", "inner_mm": 16}, {"name": "25", "inner_mm": 21}],
            "drain_table": [{"max_dfu": 3, "dia_mm": 40}, {"max_dfu": 8, "dia_mm": 75}],
            "persons": 5, "litres_per_person_day": 200, "storage_days": 1.5},
        "hvac": {"indoor_c": 24, "outdoor_c": 45, "window_u": 3.0, "window_shgc": 0.5, "solar_w_m2": 400,
            "person_w": 100, "area_per_person_m2": 10, "lights_w_m2": 10, "equipment_w_m2": 5,
            "vent_l_s_per_m2": 1, "safety_factor": 1.1, "unit_sizes_w": [2000, 3500, 5000], "roof_exposed": true}
    }))
    .unwrap()
}

fn near(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol, "{a} vs {b}");
}

#[test]
fn u_value_by_hand() {
    // R = 0.13 + 0.04 + 0.02/0.7 + 0.2/1.0 + 0.05/0.04 = 1.648571 ; U = 0.606584
    let u = lib().assemblies["wall"].u_value().unwrap();
    near(u, 1.0 / (0.17 + 0.02 / 0.7 + 0.2 + 1.25), 1e-12);
    near(u, 0.6066, 1e-4);
}

#[test]
fn incomplete_assembly_names_what_is_missing() {
    let mut l = lib();
    l.assemblies.get_mut("wall").unwrap().rse = None;
    l.assemblies.get_mut("wall").unwrap().layers[1].lambda = None;
    let e = l.assemblies["wall"].u_value().unwrap_err();
    assert!(e.contains(&"rse".to_string()) && e.contains(&"lambda of 'block'".to_string()), "{e:?}");
    let rep = insulation::analyze(&golden().doc.model, &l);
    assert_eq!(rep.assemblies.iter().find(|a| a.scope == "wall").unwrap().status, "incomplete");
}

#[test]
fn insulation_areas_and_status() {
    let g = golden();
    let rep = insulation::analyze(&g.doc.model, &lib());
    let wall = rep.assemblies.iter().find(|a| a.scope == "wall").unwrap();
    // 4 m x 3 m exterior wall minus a 0.9 x 2.1 door and a 1.5 x 1.2 window; the partition is not exterior.
    near(wall.area_m2, 12.0 - 1.89 - 1.8, 1e-9);
    assert_eq!(wall.status, "exceeds limit");
    let roof = rep.assemblies.iter().find(|a| a.scope == "roof").unwrap();
    near(roof.area_m2, 20.0, 1e-9);
    assert_eq!(roof.status, "no limit given");
    let eps = rep.items.iter().find(|i| i.code == "INS.wall.eps").unwrap();
    near(eps.quantity, 8.31 * 0.05, 1e-4);
    assert!(!eps.sources.is_empty());
}

#[test]
fn waterproofing_is_counted_without_a_thermal_requirement() {
    // A bathroom gets a membrane: floor 6 m2 + a 300 mm turn-up along its 10 m perimeter = 6 + 3 = 9 m2.
    let mut g = golden();
    let bath = Space { func: "bathroom".into(), requirement_key: "b1".into(), area: 6.0, target_area: 6.0, x: 4000, y: 0, w: 2000, h: 3000, boundary: vec![[4000, 0], [6000, 0], [6000, 3000], [4000, 3000]], net_area: None, glazing_ratio: None, extra: Default::default() };
    g.doc.commit("bath", Origin::User, vec![Command::Create(element_of(&bath, Source::User))]).unwrap();
    let l: InsulationLib = serde_json::from_value(serde_json::json!({
        "assemblies": {"wet_floor": {"layers": [{"material": "membrane", "thickness_mm": 2, "kind": "membrane"}]}},
        "wet_upturn_mm": 300
    }))
    .unwrap();
    let rep = insulation::analyze(&g.doc.model, &l);
    let a = &rep.assemblies[0];
    assert_eq!((a.status.as_str(), a.u_value, a.missing.len()), ("quantities only", None, 0));
    near(a.area_m2, 9.0, 1e-9);
    near(rep.items[0].quantity, 9.0, 1e-9);
}

#[test]
fn electrical_by_hand() {
    let g = golden();
    let rep = mep::compute(&g.doc.model, &inputs());
    assert!(rep.missing.is_empty(), "{:?}", rep.missing);
    let e = rep.electrical.unwrap();
    // 10 lights of 10 W (one circuit); 18 m perimeter x 0.5 = 9 sockets of 200 W (6 + 3).
    let l: Vec<_> = e.circuits.iter().filter(|c| c.kind == "lighting").collect();
    let s: Vec<_> = e.circuits.iter().filter(|c| c.kind == "sockets").collect();
    assert_eq!((l.len(), s.len()), (1, 2));
    assert_eq!((l[0].points, s[0].points, s[1].points), (10, 6, 3));
    near(l[0].watts, 100.0, 1e-9);
    near(s[0].current_a, 1200.0 / 230.0, 0.01);
    assert_eq!(s[0].breaker_a, 10.0); // 5.217 A x 1.25 = 6.52 A
    assert_eq!(l[0].breaker_a, 6.0); // 0.435 A x 1.25 = 0.54 A
    near(e.connected_w, 1900.0, 1e-9);
    near(e.demand_w, 950.0, 1e-9);
    assert_eq!(e.main_breaker_a, 6.0); // 950 / 230 = 4.13 A x 1.25 = 5.16 A
    for c in &e.circuits {
        assert!(c.vdrop_pct <= 3.0 + 1e-9);
    }
    assert!(rep.points.iter().any(|p| p.role == "panel"));
    assert_eq!(rep.points.iter().filter(|p| p.role == "light").count(), 10);
    assert_eq!(rep.points.iter().filter(|p| p.role == "socket").count(), 9);
}

#[test]
fn voltage_drop_forces_a_bigger_cable() {
    let g = golden();
    let mut i = inputs();
    i.electrical.as_mut().unwrap().route_factor = Some(60.0);
    let e = mep::compute(&g.doc.model, &i).electrical.unwrap();
    let s1 = e.circuits.iter().find(|c| c.id == "S1").unwrap();
    assert!(s1.cable_mm2 > 1.5, "{s1:?}");
    // dV% = 2 I L rho / (A V) x 100
    near(s1.vdrop_pct, 2.0 * s1.current_a * s1.length_m * 0.0175 / (s1.cable_mm2 * 230.0) * 100.0, 0.02);
}

#[test]
fn plumbing_by_hand() {
    let g = golden();
    let p = mep::compute(&g.doc.model, &inputs()).plumbing.unwrap();
    // 0.3 L/s connected x 0.5 = 0.15 L/s; d = sqrt(4Q/(pi v)) = 9.77 mm -> pipe "15" (12 mm).
    near(p.connected_flow_lps, 0.3, 1e-9);
    near(p.design_flow_lps, 0.15, 1e-9);
    assert_eq!(p.main_pipe, "15");
    near(p.total_dfu, 6.0, 1e-9);
    assert_eq!(p.main_drain_mm, 75.0);
    near(p.tank_m3.unwrap(), 1.5, 1e-9);
}

#[test]
fn hvac_by_hand() {
    let g = golden();
    let h = mep::compute(&g.doc.model, &inputs()).hvac.unwrap();
    let (uw, ur) = (1.0 / (0.17 + 0.02 / 0.7 + 0.2 + 1.25), 1.0 / (0.14 + 0.1));
    let dt = 21.0;
    let conduction = 8.31 * uw * dt + 20.0 * ur * dt;
    let glass = 1.8 * (3.0 * dt + 0.5 * 400.0);
    let internal = 20.0 / 10.0 * 100.0 + 15.0 * 20.0;
    let vent = 1.2 * 1006.0 * (20.0 / 1000.0) * dt;
    let load = (conduction + glass + internal + vent) * 1.1;
    let r = &h.rooms[0];
    near(r.load_w, load, 0.05);
    near(h.total_w, load, 0.05);
    near(h.total_tons, load / 3516.85, 0.01);
    assert!(r.units >= 1);
    assert!(f64::from(r.units) * r.unit_w >= load - 1e-6);
}

#[test]
fn missing_inputs_are_named_and_nothing_is_guessed() {
    let g = golden();
    let mut i = inputs();
    i.electrical.as_mut().unwrap().voltage_v = None;
    i.electrical.as_mut().unwrap().panel_room = Some("nowhere".into());
    i.hvac.as_mut().unwrap().solar_w_m2 = None;
    i.plumbing.as_mut().unwrap().drain_table.clear();
    let r = mep::compute(&g.doc.model, &i);
    assert!(r.electrical.is_none() && r.hvac.is_none() && r.plumbing.is_none());
    assert!(r.missing["electrical"].iter().any(|m| m == "electrical.voltage_v"));
    assert!(r.missing["electrical"].iter().any(|m| m.starts_with("electrical.panel_room")));
    assert!(r.missing["hvac"].iter().any(|m| m == "hvac.solar_w_m2"));
    assert!(r.missing["plumbing"].iter().any(|m| m == "plumbing.drain_table"));
    let none = mep::compute(&g.doc.model, &MepInputs::default());
    assert!(none.electrical.is_none() && none.missing.is_empty());
}

#[test]
fn apply_is_idempotent_undoable_and_refreshes() {
    let mut g = golden();
    let rep = mep::compute(&g.doc.model, &inputs());
    let (created, removed) = mep::apply(&mut g.doc, &rep.points).unwrap();
    assert_eq!(removed, 0);
    assert_eq!(created, rep.points.len());
    let count = |d: &Document| d.model.elements.values().filter(|e| e.kind == MepPoint::KIND).count();
    assert_eq!(count(&g.doc), rep.points.len());
    let before: Vec<_> = g.doc.model.elements.keys().copied().collect();
    assert_eq!(mep::apply(&mut g.doc, &rep.points).unwrap(), (0, 0));
    assert_eq!(g.doc.model.elements.keys().copied().collect::<Vec<_>>(), before);
    let items = mep::point_items(&g.doc.model);
    let sockets = items.iter().find(|i| i.code == "MEP.socket").unwrap();
    assert_eq!(sockets.quantity as usize, 9);
    assert_eq!(sockets.sources.len(), 9);
    let mut i = inputs();
    i.electrical.as_mut().unwrap().sockets_per_m = Some(0.25);
    let rep2 = mep::compute(&g.doc.model, &i);
    let (c2, r2) = mep::apply(&mut g.doc, &rep2.points).unwrap();
    assert!(r2 > 0 || c2 > 0);
    assert_eq!(count(&g.doc), rep2.points.len());
    assert!(g.doc.undo());
    assert_eq!(g.doc.model.elements.keys().copied().collect::<Vec<_>>(), before);
}

#[test]
fn a_point_outside_its_room_is_refused() {
    let mut g = golden();
    let bad = MepPoint { role: "socket".into(), x: 9000, y: 9000, room: "r1".into(), watts: Some(100.0), circuit: None, label: None, extra: Default::default() };
    let r = g.doc.commit("bad", Origin::User, vec![Command::Create(element_of(&bad, Source::User))]);
    assert!(r.is_err());
}
