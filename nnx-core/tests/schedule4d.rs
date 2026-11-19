//! The 4D schedule: structural elements and MEP runs as tasks with their own quantities, the phase chain, and the
//! element ids a 4D viewer reveals. Rates and inputs are SYNTHETIC test data.

use nnx_core::mep::{self, MepInputs};
use nnx_core::qto::{self, Qto, QtoOpts, Unit};
use nnx_core::schedule::{self, Calendar, Date, PHASES, Rate, RateLib};
use nnx_core::sizing::{self, SizingInputs};
use nnx_core::structural::{self, Loads, Soil, StructOpts};
use nnx_core::typed::{Slab, Space, Wall, element_of};
use nnx_core::*;
use serde_json::json;

fn space(key: &str, func: &str, x: i64) -> Space {
    Space { func: func.into(), requirement_key: key.into(), area: 24.0, target_area: 24.0, x, y: 0, w: 6000, h: 4000, boundary: vec![[x, 0], [x + 6000, 0], [x + 6000, 4000], [x, 4000]], net_area: None, glazing_ratio: None, extra: Default::default() }
}

fn wall(x1: i64, y1: i64, x2: i64, y2: i64, a: &str, b: Option<&str>, exterior: bool) -> Wall {
    Wall { x1, y1, x2, y2, thickness: 200, height: 3000, length: (x2 - x1).abs() + (y2 - y1).abs(), is_exterior: exterior, room_a: a.into(), room_b: b.map(str::to_string), extra: Default::default() }
}

/// Two rooms, structure from the engine, tie beams from the schedules, MEP points and runs from the router.
struct House {
    doc: Document,
    qto: Qto,
}

fn mep_inputs() -> MepInputs {
    serde_json::from_value(json!({
        "insulation": {"assemblies": {
            "wall": {"rsi": 0.13, "rse": 0.04, "layers": [{"material": "block", "thickness_mm": 200, "lambda": 1.0}, {"material": "eps", "thickness_mm": 50, "lambda": 0.04, "kind": "insulation"}]},
            "roof": {"rsi": 0.1, "rse": 0.04, "layers": [{"material": "concrete", "thickness_mm": 150, "lambda": 1.5}, {"material": "eps", "thickness_mm": 60, "lambda": 0.04, "kind": "insulation"}]}}},
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

fn house(with_extras: bool) -> House {
    let mut cmds = vec![Command::Create(element_of(&space("a", "living", 0), Source::User)), Command::Create(element_of(&space("b", "bathroom", 6000), Source::User))];
    for (x1, y1, x2, y2, a, b, ext) in [(0, 0, 12000, 0, "a", None, true), (12000, 0, 12000, 4000, "b", None, true), (12000, 4000, 0, 4000, "b", None, true), (0, 4000, 0, 0, "a", None, true), (6000, 0, 6000, 4000, "a", Some("b"), false)] {
        cmds.push(Command::Create(element_of(&wall(x1, y1, x2, y2, a, b, ext), Source::User)));
    }
    let mut doc = Document::default();
    doc.commit("plan", Origin::User, cmds).unwrap();
    let opts = StructOpts { loads: Some(Loads { dead_kpa: 5.0, live_kpa: 2.0, source: "test".into() }), soil: Some(Soil { bearing_kpa: 150.0, source: "test".into() }), ..StructOpts::default() };
    structural::sync(&mut doc, &opts, None).unwrap();
    let _ = Slab { x: 0, y: 0, w: 1, h: 1, thickness: 1, kind: String::new(), room: String::new(), extra: Default::default() };
    let mut items: Vec<qto::QtoItem> = qto::compute(&doc.model, &QtoOpts::default()).items;
    if with_extras {
        let rep = sizing::compute(&doc.model, Some(150.0), &sizing_inputs());
        sizing::apply_tie_beams(&mut doc, &rep).unwrap();
        let rep = sizing::compute(&doc.model, Some(150.0), &sizing_inputs());
        let m = mep::compute(&doc.model, &mep_inputs());
        mep::apply_with_runs(&mut doc, &m.points, &m.runs).unwrap();
        items = qto::compute(&doc.model, &QtoOpts::default()).items;
        items.extend(mep::point_items(&doc.model));
        items.extend(m.insulation_items.clone());
        items.extend(sizing::schedule_items(&rep));
    }
    House { doc, qto: Qto { items, warnings: Vec::new() } }
}

fn lib() -> RateLib {
    let mut lib = RateLib::default();
    for (key, is_fixed) in schedule::template_keys() {
        let r = if is_fixed { Rate { fixed_days: Some(2.0), crews: 1, source: "synthetic".into(), ..Default::default() } } else { Rate { per_crew_day: Some(4.0), crews: 1, source: "synthetic".into(), ..Default::default() } };
        lib.rates.insert(key.to_string(), r);
    }
    lib
}

fn cal() -> Calendar {
    Calendar::new(Date::parse("2026-11-15").unwrap(), [true, true, true, true, false, false, true]).unwrap()
}

fn task<'a>(s: &'a schedule::Schedule, name: &str) -> &'a schedule::Task {
    s.tasks.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no task '{name}' in {:?}", s.tasks.iter().map(|t| &t.name).collect::<Vec<_>>()))
}

#[test]
fn structural_elements_and_mep_runs_become_tasks_with_their_own_quantities_and_ids() {
    let h = house(true);
    let s = schedule::build_with_model(&h.doc.model, &h.qto, &lib(), &cal(), 1).unwrap();
    // Footings: concrete m3 and steel kg of the same work, with the footing element ids.
    let pour = task(&s, "Footing concrete pour");
    let conc = pour.also.iter().find(|m| m.label == "concrete").unwrap();
    assert_eq!(conc.unit, Unit::M3);
    let rep = sizing::compute(&h.doc.model, Some(150.0), &sizing_inputs());
    let fnd_m3: f64 = rep.concrete.iter().filter(|c| c.kind == "Footing").map(|c| c.volume_m3).sum();
    assert!((conc.quantity - fnd_m3).abs() < 1e-3, "{} vs {fnd_m3}", conc.quantity);
    let steel = task(&s, "Footing reinforcement").also.iter().find(|m| m.label == "steel").unwrap();
    assert_eq!(steel.unit, Unit::Kg);
    let fnd_kg: f64 = rep.bars.iter().filter(|b| b.element.starts_with("FND-")).map(|b| b.kg).sum();
    assert!((steel.quantity - fnd_kg).abs() < 1e-2);
    for f in h.doc.model.elements.values().filter(|e| e.kind == "Footing") {
        assert!(pour.elements.contains(&f.id), "every footing is in the pour");
    }
    // Columns likewise, in cubic metres and kilograms.
    let col = task(&s, "Column concrete pour");
    assert!(col.also.iter().any(|m| m.label == "concrete" && m.unit == Unit::M3 && m.quantity > 0.0));
    assert!(task(&s, "Column reinforcement").also.iter().any(|m| m.label == "steel" && m.unit == Unit::Kg));
    // Tie beams are model elements now, so they have ids and a task in the substructure.
    let tb = task(&s, "Tie beams");
    assert_eq!(tb.phase, "Substructure");
    assert_eq!(tb.unit, Some(Unit::M3));
    assert_eq!(tb.elements.len(), rep.tie_beams.len(), "one element per tie beam");
    assert!(tb.elements.iter().all(|id| h.doc.model.get(id).is_some_and(|e| e.kind == "TieBeam")));
    assert!(tb.also.iter().any(|m| m.label == "steel" && m.unit == Unit::Kg));
    // MEP runs in linear metres, each with its run ids; devices as a count of points.
    for (name, system) in [("HVAC ductwork", "duct"), ("Below-slab drainage", "drain"), ("Water supply pipework", "pipe"), ("Cable trays", "cable")] {
        let t = task(&s, name);
        assert_eq!(t.unit, Some(Unit::M), "{name}");
        assert!(t.quantity.unwrap() > 0.0 && !t.elements.is_empty());
        let total: f64 = t.elements.iter().filter_map(|id| h.doc.model.get(id)).filter_map(|e| e.typed::<nnx_core::typed::MepRun>().ok()).map(|r| ((r.x2 - r.x1).pow(2) as f64 + (r.y2 - r.y1).pow(2) as f64 + (r.z2 - r.z1).pow(2) as f64).sqrt() / 1000.0).sum();
        assert!((t.quantity.unwrap() - total).abs() < 0.02, "{name}: {} vs {total}", t.quantity.unwrap());
        assert!(t.elements.iter().all(|id| h.doc.model.get(id).is_some_and(|e| e.kind == "MepRun" && e.props["system"] == system)), "{name}");
    }
    let points = task(&s, "Devices, diffusers and fixtures installed");
    assert_eq!(points.unit, Some(Unit::Nr));
    assert!(points.quantity.unwrap() > 10.0);
    assert!(points.elements.iter().all(|id| h.doc.model.get(id).is_some_and(|e| e.kind == "MepPoint")));
    // Insulation has its own task.
    assert_eq!(task(&s, "Thermal insulation and waterproofing").phase, "Insulation");
}

#[test]
fn the_phases_follow_each_other_and_every_task_has_a_wbs_number() {
    let h = house(true);
    let s = schedule::build_with_model(&h.doc.model, &h.qto, &lib(), &cal(), 1).unwrap();
    let rank = |p: &str| PHASES.iter().position(|x| *x == p).unwrap();
    // Every predecessor is in the same phase or an earlier one: work never waits for a later phase.
    for t in &s.tasks {
        for &(p, _) in &t.preds {
            assert!(rank(s.tasks[p].phase) <= rank(t.phase), "{} ({}) waits for {} ({})", t.name, t.phase, s.tasks[p].name, s.tasks[p].phase);
        }
    }
    for phase in PHASES {
        assert!(s.tasks.iter().any(|t| t.phase == phase), "{phase} has tasks");
    }
    // The chain Substructure -> Superstructure -> Insulation -> MEP rough-in -> Finishing -> Commissioning is on the
    // critical path: following critical predecessors back from the end passes through every phase.
    let mut cur = *s.critical.iter().find(|&&i| s.tasks[i].ef == s.project_days).unwrap();
    let mut seen = std::collections::BTreeSet::new();
    loop {
        seen.insert(s.tasks[cur].phase);
        let Some(prev) = s.tasks[cur].preds.iter().map(|&(p, _)| p).find(|&p| s.tasks[p].critical && s.tasks[p].ef == s.tasks[cur].es) else { break };
        cur = prev;
    }
    for phase in PHASES {
        assert!(seen.contains(phase), "the critical path passes through {phase}: {seen:?}");
    }
    // WBS numbers: phase.sequence, unique.
    let mut wbs: Vec<&str> = s.tasks.iter().map(|t| t.wbs.as_str()).collect();
    wbs.sort();
    wbs.dedup();
    assert_eq!(wbs.len(), s.tasks.len());
    assert!(s.tasks[0].wbs == "1.1" && s.tasks.last().unwrap().wbs.starts_with("6."));
    // The rough-in runs wait for the insulation, the finishes wait for the runs.
    let ins = s.tasks.iter().position(|t| t.name == "Thermal insulation and waterproofing").unwrap();
    let ff = s.tasks.iter().position(|t| t.name == "First-fix electrical and plumbing").unwrap();
    assert!(s.tasks[ff].preds.iter().any(|&(p, _)| p == ins));
    let plaster = task(&s, "Plastering");
    for name in ["HVAC ductwork", "Water supply pipework", "Cable trays"] {
        let i = s.tasks.iter().position(|t| t.name == name).unwrap();
        assert!(plaster.preds.iter().any(|&(p, _)| p == i), "plastering waits for {name}");
    }
}

#[test]
fn a_model_without_the_optional_work_has_none_of_those_tasks_and_needs_no_rates_for_them() {
    let h = house(false);
    let mut l = lib();
    for k in schedule::optional_keys() {
        l.rates.remove(k);
    }
    let s = schedule::build_with_model(&h.doc.model, &h.qto, &l, &cal(), 1).unwrap();
    for name in ["Tie beams", "HVAC ductwork", "Cable trays", "Thermal insulation and waterproofing", "Devices, diffusers and fixtures installed"] {
        assert!(!s.tasks.iter().any(|t| t.name == name), "{name}");
    }
    // The chain closes over the absent insulation task: first fix still follows blockwork.
    let bw = s.tasks.iter().position(|t| t.name == "Blockwork").unwrap();
    let ff = task(&s, "First-fix electrical and plumbing");
    assert!(ff.preds.iter().any(|&(p, _)| p == bw));
    // With the work present but no rate, the missing rate is named.
    let h2 = house(true);
    let err = schedule::build_with_model(&h2.doc.model, &h2.qto, &l, &cal(), 1).unwrap_err();
    let schedule::ScheduleError::MissingRates(keys) = err else { panic!() };
    assert!(keys.contains(&"tie_beam".to_string()) && keys.contains(&"duct_runs".to_string()), "{keys:?}");
}

#[test]
fn zones_split_the_new_quantities_by_element_and_every_task_element_exists() {
    let h = house(true);
    let s = schedule::build_with_model(&h.doc.model, &h.qto, &lib(), &cal(), 2).unwrap();
    let pours: Vec<&schedule::Task> = s.tasks.iter().filter(|t| t.name.starts_with("Column concrete pour")).collect();
    assert_eq!(pours.len(), 2);
    let (a, b) = (&pours[0], &pours[1]);
    assert!(a.elements.iter().all(|id| !b.elements.contains(id)), "zones take disjoint columns");
    let total: f64 = pours.iter().map(|t| t.also.iter().find(|m| m.label == "concrete").unwrap().quantity).sum();
    let one = schedule::build_with_model(&h.doc.model, &h.qto, &lib(), &cal(), 1).unwrap();
    let whole = task(&one, "Column concrete pour").also.iter().find(|m| m.label == "concrete").unwrap().quantity;
    assert!((total - whole).abs() < 1e-3, "{total} vs {whole}");
    for t in &s.tasks {
        assert!(t.elements.iter().all(|id| h.doc.model.get(id).is_some()), "{} cites an element that does not exist", t.name);
    }
}
