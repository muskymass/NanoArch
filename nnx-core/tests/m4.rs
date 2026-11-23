//! Milestone 4: solar, smart home, Wi-Fi and CCTV, EV and balancing, obstacle-aware ducts, greywater and head loss.
//! Every input is SYNTHETIC test data.

use nnx_core::clash::{self, ClashOpts};
use nnx_core::evload;
use nnx_core::greywater;
use nnx_core::hydraulics::{self, hazen_williams_m};
use nnx_core::mep::{self, MepInputs};
use nnx_core::mepx::*;
use nnx_core::project::{NanoArchProject, ProjectMeta};
use nnx_core::smarthome;
use nnx_core::solarpv;
use nnx_core::typed::{Beam, Column, Slab, Space, Wall, element_of};
use nnx_core::vertical::{AnnexIn, VerticalInput};
use nnx_core::wireless;
use nnx_core::*;
use serde_json::{Value, json};

fn near(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol, "{a} vs {b}");
}

/// Two 6 x 4 m rooms (a: living, the air handler's room; b: bathroom), exterior walls, a partition on x = 6 m with a beam
/// under the slab along it and a column at its middle. Walls are 3 m high.
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
    for (x1, y1, x2, y2, a, b, ext) in [(0, 0, 12000, 0, "a", None, true), (12000, 0, 12000, 4000, "b", None, true), (12000, 4000, 0, 4000, "b", None, true), (0, 4000, 0, 0, "a", None, true), (6000, 0, 6000, 4000, "a", Some("b"), false)] {
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
        "plumbing": {"catalog": {"wc": {"flow_lps": 0.1, "dfu": 4}, "basin": {"flow_lps": 0.05, "dfu": 1}, "shower": {"flow_lps": 0.15, "dfu": 2}},
            "fixtures_by_func": {"bathroom": {"wc": 1, "basin": 1, "shower": 1}},
            "simultaneity": 0.5, "max_velocity_mps": 2,
            "pipes": [{"name": "PPR 20", "inner_mm": 16}, {"name": "PPR 25", "inner_mm": 21}, {"name": "PPR 32", "inner_mm": 27}],
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

fn project(ext: MepExtInputs) -> NanoArchProject {
    NanoArchProject { meta: ProjectMeta { mep_ext: ext, ..Default::default() }, ..Default::default() }
}

// ------------------------------------------------------------------ solar

fn solar_in() -> SolarIn {
    SolarIn { panel_w: Some(550.0), panel_area_m2: Some(2.6), edge_clearance_mm: Some(500.0), coverage_ratio: Some(0.6), obstructions: vec![[1000.0, 1000.0, 2000.0, 2000.0]], specific_yield_kwh_kw: Some(1700.0), tariff_per_kwh: Some(0.18), self_consumption: Some(0.7), dc_ac_ratio: Some(1.2), annual_consumption_kwh: Some(8000.0), min_usable_share: None, min_panels: None }
}

#[test]
fn the_usable_roof_excludes_the_edge_the_annex_the_shaft_and_obstructions_and_prices_the_energy_from_your_inputs() {
    let p = plan();
    let r = solarpv::size(&p.doc.model, &project(MepExtInputs { solar: solar_in(), ..Default::default() }));
    // Roof 48 m2; each slab pulled in 500 mm leaves 5 x 3 = 15 m2, so 30 m2; the 1 m2 obstruction leaves 29.
    assert_eq!((r.roof_m2, r.after_edge_m2, r.usable_m2, r.excluded_m2), (48.0, 30.0, 29.0, 1.0));
    // floor(29 * 0.6 / 2.6) = 6 panels of 550 W = 3.3 kW; inverter 3.3 / 1.2; energy 3.3 * 1700; saving at 70 % self-consumption.
    assert_eq!((r.panels, r.kwp, r.inverter_kw), (Some(6), Some(3.3), Some(2.75)));
    near(r.annual_kwh.unwrap(), 5610.0, 1e-6);
    near(r.self_used_kwh.unwrap(), 3927.0, 1e-6);
    near(r.exported_kwh.unwrap(), 1683.0, 1e-6);
    near(r.annual_saving.unwrap(), 3927.0 * 0.18, 0.01);
    near(r.offset_pct.unwrap(), 3927.0 / 8000.0 * 100.0, 0.01);
    assert!(r.missing.is_empty() && r.notes.iter().any(|n| n.contains("no yield, tariff")));
    // The roof annex and the elevator shaft of the project come off the roof too.
    let mut ext = MepExtInputs { solar: solar_in(), ..Default::default() };
    let mut p2 = project(ext.clone());
    p2.meta.vertical = Some(VerticalInput { roof_annex: Some(AnnexIn { x_mm: Some(500.0), y_mm: Some(500.0), width_mm: Some(2000.0), length_mm: Some(2000.0), height_mm: Some(2800.0) }), ..Default::default() });
    let r2 = solarpv::size(&p.doc.model, &p2);
    // The 1 m2 obstruction lies inside the 4 m2 annex: the union is excluded once, so 30 - 4.
    near(r2.usable_m2, 26.0, 1e-6);
    // Obstructions overlapping each other and the annex are counted once.
    ext.solar.obstructions.push([500.0, 500.0, 1500.0, 1500.0]);
    let mut p3 = project(ext);
    p3.meta.vertical = p2.meta.vertical.clone();
    near(solarpv::size(&p.doc.model, &p3).usable_m2, 26.0, 1e-6);
    // Nothing is assumed: without yield, tariff, ratio or panel data the figures are not computed and are named.
    let none = solarpv::size(&p.doc.model, &project(MepExtInputs::default()));
    for n in ["edge clearance", "panel power", "panel area", "coverage ratio"] {
        assert!(none.missing.iter().any(|m| m.contains(n)), "{n}");
    }
    assert!(none.panels.is_none() && none.annual_kwh.is_none());
    let mut part = solar_in();
    part.specific_yield_kwh_kw = None;
    part.tariff_per_kwh = None;
    part.dc_ac_ratio = None;
    let r = solarpv::size(&p.doc.model, &project(MepExtInputs { solar: part, ..Default::default() }));
    assert!(r.panels == Some(6) && r.annual_kwh.is_none() && r.inverter_kw.is_none() && r.missing.iter().any(|m| m.contains("annual yield")) && r.missing.iter().any(|m| m.contains("DC/AC")));
    assert!(solarpv::size(&Document::default().model, &project(MepExtInputs::default())).missing[0].contains("no slabs"));
    assert!(MepExtInputs { solar: SolarIn { coverage_ratio: Some(1.5), ..Default::default() }, ..Default::default() }.check().is_err());
}

#[test]
fn area_minus_is_exact_for_overlapping_rectangles() {
    near(area_minus_m2(&[[0.0, 0.0, 2000.0, 2000.0], [1000.0, 1000.0, 3000.0, 3000.0]], &[]), 7.0, 1e-9);
    near(area_minus_m2(&[[0.0, 0.0, 2000.0, 2000.0]], &[[1000.0, 0.0, 5000.0, 5000.0]]), 2.0, 1e-9);
    near(area_minus_m2(&[], &[[0.0, 0.0, 1.0, 1.0]]), 0.0, 1e-12);
}

// ------------------------------------------------------------------ smart home

fn smart_in() -> SmartIn {
    SmartIn { protocol: "knx".into(), panel_room: Some("a".into()), max_devices_per_line: Some(2.0), max_line_m: Some(100.0), route_factor: Some(1.2), bus_z_mm: Some(2800.0), bus_mm: Some(20.0), devices: vec![] }
}

#[test]
fn the_smart_home_suggestion_follows_the_rooms_and_the_bus_is_split_by_your_line_limits() {
    let p = plan();
    let mut inp = smart_in();
    inp.devices = smarthome::suggest(&p.doc.model);
    // Room a (living): light, sensor, thermostat; room b (bathroom): light, sensor, no thermostat.
    let kinds = |k: &str| inp.devices.iter().filter(|d| d.kind == k).count();
    assert_eq!((kinds("lighting"), kinds("sensor"), kinds("hvac"), inp.devices.len()), (2, 2, 1, 5));
    let r = smarthome::topology(&p.doc.model, &inp);
    assert!(r.wired && r.missing.is_empty(), "{:?}", r.missing);
    assert_eq!(r.panel, Some([3000, 2000]));
    // Five devices, two per line: three lines; every device in exactly one line.
    assert_eq!(r.lines.len(), 3);
    let mut all: Vec<&String> = r.lines.iter().flat_map(|l| &l.devices).collect();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 5);
    assert!(r.lines.iter().all(|l| l.devices.len() <= 2 && l.length_m <= 100.0));
    near(r.total_bus_m, r.lines.iter().map(|l| l.length_m).sum::<f64>(), 0.02);
    // The devices in room b are 6 m (x 6000 apart): 7.2 m with the route factor, once per line that reaches them.
    assert!(r.lines.iter().any(|l| l.length_m >= 7.2 - 1e-9));
    assert!(!r.runs.is_empty() && r.runs.iter().all(|u| u.system == "cable" && u.role == "smart-bus" && u.p1[2] == 2800 && (u.p1[0] != u.p2[0]) != (u.p1[1] != u.p2[1])));
    assert_eq!(r.by_kind["lighting"], 2);
    // The runs are valid model elements and undo with the rest.
    let mut p = p;
    mep::apply_with_runs(&mut p.doc, &[], &r.runs).unwrap();
    assert!(p.doc.model.elements.values().any(|e| e.kind == "MepRun" && e.props["role"] == "smart-bus"));
    // A line limit of one device gives a line per device; a short line length splits by distance.
    let mut one = inp.clone();
    one.max_devices_per_line = Some(1.0);
    assert_eq!(smarthome::topology(&plan().doc.model, &one).lines.len(), 5);
    let mut short = inp.clone();
    short.max_line_m = Some(5.0);
    short.max_devices_per_line = Some(10.0);
    let s = smarthome::topology(&plan().doc.model, &short);
    assert!(s.lines.len() >= 2 && s.lines.iter().all(|l| l.length_m <= 5.0 || l.devices.len() == 1));
    // Wireless protocols route nothing and say so; missing inputs are named.
    let mut matter = inp.clone();
    matter.protocol = "matter".into();
    let m = smarthome::topology(&plan().doc.model, &matter);
    assert!(!m.wired && m.lines.is_empty() && m.runs.is_empty() && m.notes.iter().any(|n| n.contains("wireless")));
    let mut bare = smart_in();
    bare.bus_mm = None;
    bare.max_line_m = None;
    let b = smarthome::topology(&plan().doc.model, &SmartIn { devices: inp.devices.clone(), ..bare });
    assert!(b.missing.iter().any(|x| x.contains("line length")) && b.missing.iter().any(|x| x.contains("bus section")) && b.lines.is_empty());
    let nopanel = smarthome::topology(&plan().doc.model, &SmartIn { panel_room: Some("zz".into()), ..inp.clone() });
    assert!(nopanel.missing[0].contains("panel room"));
    let mut ghost = inp;
    ghost.devices.push(SmartDevice { name: "Ghost".into(), kind: "lighting".into(), room: "nope".into(), x_mm: None, y_mm: None });
    assert!(smarthome::topology(&plan().doc.model, &ghost).unplaced == vec!["Ghost".to_string()]);
    assert!(MepExtInputs { smart: SmartIn { protocol: "x10".into(), ..Default::default() }, ..Default::default() }.check().is_err());
}

// ------------------------------------------------------------------ Wi-Fi and CCTV

fn wifi_in() -> WifiIn {
    WifiIn { band_ghz: Some(2.4), tx_power_dbm: Some(20.0), antenna_gain_dbi: None, exponent: Some(3.0), interior_wall_db: Some(5.0), exterior_wall_db: Some(10.0), threshold_dbm: Some(-60.0), target_coverage_pct: Some(90.0), max_aps: Some(3.0), floor: Some(1), aps: vec![] }
}

#[test]
fn wifi_signal_follows_the_log_distance_model_with_the_walls_between_and_aps_are_suggested_to_the_target() {
    let p = plan();
    let mut inp = wifi_in();
    inp.aps = vec![AccessPoint { name: "AP-A".into(), x_mm: 3000.0, y_mm: 2000.0, floor: None }];
    let r = wireless::wifi(&p.doc.model, &inp);
    near(r.path_loss_1m_db.unwrap(), 20.0 * 2400f64.log10() - 27.56, 0.01);
    let cell = |x: f64, y: f64| r.cells.iter().find(|c| c.x == x && c.y == y).unwrap().dbm;
    // Beside the access point d is under 1 m (taken as 1 m): 20 dBm - the 1 m loss; across the partition: distance and one interior wall.
    near(cell(3250.0, 2250.0), 20.0 - r.path_loss_1m_db.unwrap(), 0.02);
    let d = ((9250.0f64 - 3000.0).powi(2) + (2250.0f64 - 2000.0).powi(2)).sqrt() / 1000.0;
    near(cell(9250.0, 2250.0), 20.0 - r.path_loss_1m_db.unwrap() - 30.0 * d.log10() - 5.0, 0.02);
    assert!(r.notes.iter().any(|n| n.contains("no antenna gain")));
    // The access point's own room is better covered than the far room.
    let cov = |k: &str| r.rooms.iter().find(|x| x.room == k).unwrap().covered_pct;
    assert!(cov("a") >= cov("b"));
    // With a strict threshold the far room is not covered: access points are suggested at room centres until the target is reached.
    let mut strict = inp.clone();
    strict.threshold_dbm = Some(-45.0);
    let s = wireless::wifi(&p.doc.model, &strict);
    assert!(s.aps.len() >= 2 && s.aps.iter().any(|a| a.suggested) && s.aps[0].name == "AP-A" && !s.aps[0].suggested);
    assert_eq!(s.target_met, Some(true), "{:?}", s.coverage_pct);
    assert!(s.coverage_pct.unwrap() >= r.coverage_pct.unwrap().min(90.0));
    // Impossible target within the access-point cap: not met, and it is said.
    let mut cap = strict.clone();
    cap.max_aps = Some(1.0);
    cap.target_coverage_pct = Some(100.0);
    let c = wireless::wifi(&p.doc.model, &cap);
    assert_eq!(c.target_met, Some(false));
    assert!(c.notes.iter().any(|n| n.contains("is not reached")));
    // Missing parameters are named and nothing is computed.
    let none = wireless::wifi(&p.doc.model, &WifiIn::default());
    assert!(none.missing.len() == 8 && none.coverage_pct.is_none() && none.cells.is_empty());
    let mut bad = inp.clone();
    bad.floor = Some(3);
    assert!(wireless::wifi(&p.doc.model, &bad).notes[0].contains("no floor 3"));
}

fn cams() -> CctvIn {
    CctvIn { cameras: vec![Camera { name: "C1".into(), x_mm: 6000.0, y_mm: 0.0, mount_height_mm: 2800.0, azimuth_deg: 180.0, fov_deg: 90.0, range_m: 10.0 }], perimeter_offset_mm: Some(1000.0) }
}

#[test]
fn a_camera_sees_its_sector_outside_the_building_and_not_behind_walls() {
    let p = plan();
    let r = wireless::cctv(&p.doc.model, &cams());
    // A 90 degree sector of 10 m radius is a quarter of a circle: 78.5 m2, all outside the building (south of the south wall).
    near(r.covered_m2, std::f64::consts::PI * 100.0 / 4.0, 6.0);
    assert!(r.cells.iter().all(|c| c.y < 0.0));
    assert!(r.perimeter_pct.unwrap() > 0.0 && r.perimeter_pct.unwrap() < 100.0 && !r.blind_spots.is_empty());
    assert_eq!(r.cameras[0].name, "C1");
    // Looking into the building from its south wall the camera sees nothing outside: the walls hide the far side.
    let mut inward = cams();
    inward.cameras[0].azimuth_deg = 0.0;
    let i = wireless::cctv(&p.doc.model, &inward);
    assert!(i.cells.iter().all(|c| c.y < 0.0 || c.y > 4000.0) && i.covered_m2 < 1.0, "{} m2", i.covered_m2);
    // The 3D solids and the drawing.
    let b = wireless::cctv_boxes(&p.doc.model, &cams());
    assert_eq!(b.len(), r.cells.len() + 1);
    assert!(b.iter().any(|x| x.class == "camera") && b.iter().all(|x| x.kind == "Coverage"));
    let svg = wireless::network_svg(&p.doc.model, 1, None, Some((&cams(), &r)), None, None).unwrap();
    assert!(svg.contains("cctv-cone") && svg.contains("class=\"cctv\"") && svg.contains("CCTV perimeter"));
    // Without an offset the perimeter is not judged and the missing input is named.
    let mut off = cams();
    off.perimeter_offset_mm = None;
    assert!(wireless::cctv(&p.doc.model, &off).missing[0].contains("perimeter offset"));
    assert!(wireless::cctv(&p.doc.model, &CctvIn::default()).notes[0].contains("no camera"));
    assert!(MepExtInputs { cctv: CctvIn { cameras: vec![Camera { fov_deg: 0.0, range_m: 5.0, ..Default::default() }], perimeter_offset_mm: None }, ..Default::default() }.check().is_err());
}

#[test]
fn the_network_drawing_shows_signal_cells_access_points_bus_and_panel() {
    let p = plan();
    let w = wireless::wifi(&p.doc.model, &WifiIn { aps: vec![AccessPoint { name: "AP-A".into(), x_mm: 3000.0, y_mm: 2000.0, floor: None }], ..wifi_in() });
    let mut sm = smart_in();
    sm.devices = smarthome::suggest(&p.doc.model);
    let t = smarthome::topology(&p.doc.model, &sm);
    let svg = wireless::network_svg(&p.doc.model, 1, Some(&w), None, Some((&sm, &t)), Some(-60.0)).unwrap();
    assert!(svg.contains("class=\"wifi\"") && svg.contains("class=\"ap\"") && svg.contains("class=\"bus\"") && svg.contains("panel") && svg.contains("Wi-Fi coverage"));
    assert!(svg.starts_with("<svg") && svg.trim_end().ends_with("</svg>"));
    assert!(wireless::network_svg(&p.doc.model, 4, None, None, None, None).is_err());
}

// ------------------------------------------------------------------ EV, balancing, voltage drop

fn ev_in() -> EvIn {
    EvIn { chargers: vec![Charger { name: "EV1".into(), current_a: 32.0, phases: 1, x_mm: Some(-3000.0), y_mm: Some(2000.0), length_m: None }], continuous_factor: Some(1.25), demand_factor: Some(1.0), board_room: Some("a".into()) }
}

#[test]
fn an_ev_charger_gets_its_own_breaker_and_cable_and_adds_to_the_main_demand() {
    let p = plan();
    let i = parse(&inputs());
    let rep = mep::compute(&p.doc.model, &i);
    let r = evload::ev(&p.doc.model, &i, &rep, &ev_in());
    assert!(r.missing.is_empty(), "{:?}", r.missing);
    let c = &r.chargers[0];
    // 32 A x 1.25 = 40 A: the smallest breaker not below it is 63 A; the cable for 63 A is 16 mm2.
    assert_eq!((c.status, c.breaker_a, c.cable_mm2), ("ok", Some(63.0), Some(16.0)));
    near(c.design_a, 40.0, 1e-9);
    near(c.load_kw, 7.36, 1e-9);
    // From the board in room a (3000, 2000) to (-3000, 2000): 6 m x 1.2 route factor; drop 2 rho L I / A / V.
    near(c.length_m.unwrap(), 7.2, 1e-9);
    near(c.vdrop_pct.unwrap(), 2.0 * 0.0175 * 7.2 * 32.0 / 16.0 / 230.0 * 100.0, 0.01);
    // The main board: the chargers' demand is added at the demand factor.
    near(r.demand_kw, 7.36, 1e-9);
    let before = rep.electrical.as_ref().unwrap().demand_w;
    near(r.main_after_kva.unwrap() - r.main_before_kva.unwrap(), 7360.0 / 0.9 / 1000.0, 0.02);
    near(r.main_before_kva.unwrap(), before / 0.9 / 1000.0, 0.01);
    assert!(r.main_after_a.unwrap() > r.main_before_a.unwrap());
    // The main breaker with the chargers is never smaller than without them; when no listed size is large enough that is said.
    assert!(r.breaker_after_a.is_none_or(|a| a >= r.breaker_before_a.unwrap_or(0.0)));
    assert!(r.breaker_after_a.is_some() || r.notes.iter().any(|n| n.contains("no breaker in your list")));
    // A three-phase charger: 3 x 230 x 32 W and the line-to-line drop.
    let mut three = ev_in();
    three.chargers[0].phases = 3;
    let t = evload::ev(&p.doc.model, &i, &rep, &three);
    near(t.chargers[0].load_kw, 3.0 * 230.0 * 32.0 / 1000.0, 0.01);
    near(t.chargers[0].vdrop_pct.unwrap(), 1.732_050_8 * 0.0175 * 7.2 * 32.0 / 16.0 / (1.732_050_8 * 230.0) * 100.0, 0.01);
    // A cable run too long for the drop limit: no cable in the table does.
    let mut far = ev_in();
    far.chargers[0].length_m = Some(400.0);
    assert_eq!(evload::ev(&p.doc.model, &i, &rep, &far).chargers[0].status, "no cable");
    // A current above every breaker.
    let mut big = ev_in();
    big.chargers[0].current_a = 100.0;
    assert_eq!(evload::ev(&p.doc.model, &i, &rep, &big).chargers[0].status, "no breaker");
    // Missing inputs are named.
    let m = evload::ev(&p.doc.model, &i, &rep, &EvIn { chargers: ev_in().chargers, ..Default::default() });
    assert!(m.missing.iter().any(|x| x.contains("continuous_factor")) && m.missing.iter().any(|x| x.contains("demand_factor")) && m.chargers.is_empty());
    assert!(evload::ev(&p.doc.model, &MepInputs::default(), &rep, &ev_in()).missing[0].contains("electrical inputs"));
    assert!(MepExtInputs { ev: EvIn { chargers: vec![Charger { current_a: 32.0, phases: 2, ..Default::default() }], ..Default::default() }, ..Default::default() }.check().is_err());
}

#[test]
fn circuits_are_balanced_over_three_phases_and_the_voltage_drop_is_feeder_plus_branch() {
    let p = plan();
    let mut v = inputs();
    v["electrical"]["three_phase"] = json!(true);
    let i = parse(&v);
    let rep = mep::compute(&p.doc.model, &i);
    let er = rep.electrical.as_ref().unwrap();
    let ev = ev_in();
    let evr = evload::ev(&p.doc.model, &i, &rep, &ev);
    let bal = BalanceIn { max_imbalance_pct: Some(25.0), feeder_length_m: Some(20.0), feeder_mm2: Some(16.0), total_vdrop_max_pct: Some(5.0) };
    let r = evload::balance(&i, &rep, &ev, &bal);
    assert!(r.missing.is_empty(), "{:?}", r.missing);
    // Every circuit is on exactly one phase; the phase currents add up to the circuits (and the single-phase charger on its board).
    for b in &r.boards {
        let mine: f64 = er.circuits.iter().filter(|c| c.board == b.board).map(|c| c.current_a).sum::<f64>() + if b.board == "a" { 32.0 } else { 0.0 };
        near(b.phase_a.iter().sum::<f64>(), mine, 0.05);
        assert_eq!(b.assignments.len(), er.circuits.iter().filter(|c| c.board == b.board).count() + usize::from(b.board == "a"));
        let mean = mine / 3.0;
        near(b.imbalance_pct, (b.phase_a.iter().cloned().fold(0.0, f64::max) - mean) / mean * 100.0, 0.1);
    }
    // The largest circuits go on different phases: the largest-first rule.
    let a = r.boards.iter().find(|b| b.board == "a").unwrap();
    let mut top: Vec<&evload::Assign> = a.assignments.iter().collect();
    top.sort_by(|x, y| y.current_a.total_cmp(&x.current_a));
    assert!(top.len() >= 3 && top[0].phase != top[1].phase && top[1].phase != top[2].phase || top[0].phase != top[1].phase);
    // A tight limit is reported as not met on at least the unbalanced boards; a loose one is met.
    let tight = evload::balance(&i, &rep, &ev, &BalanceIn { max_imbalance_pct: Some(0.01), ..bal.clone() });
    assert!(tight.boards.iter().any(|b| b.ok == Some(false)));
    assert!(evload::balance(&i, &rep, &ev, &BalanceIn { max_imbalance_pct: Some(500.0), ..bal.clone() }).boards.iter().all(|b| b.ok == Some(true)));
    // Three-phase chargers load the three phases equally.
    let mut tp = ev_in();
    tp.chargers[0].phases = 3;
    let t = evload::balance(&i, &rep, &tp, &bal);
    let ta = t.boards.iter().find(|b| b.board == "a").unwrap();
    near(ta.phase_a.iter().sum::<f64>(), er.circuits.iter().filter(|c| c.board == "a").map(|c| c.current_a).sum::<f64>() + 96.0, 0.05);
    // Voltage drop: the feeder to the board from its demand, plus each circuit's own drop.
    assert_eq!(r.drops.len(), er.circuits.len());
    for d in &r.drops {
        let c = er.circuits.iter().find(|c| c.id == d.circuit).unwrap();
        near(d.branch_pct, c.vdrop_pct, 0.01);
        let w = er.boards.iter().find(|b| b.room == d.board).unwrap().demand_w;
        let feeder = 0.0175 * 20.0 * (w / 0.9 / (3.0 * 230.0)) / 16.0 / 230.0 * 100.0;
        near(d.feeder_pct.unwrap(), feeder, 0.01);
        near(d.total_pct.unwrap(), feeder + c.vdrop_pct, 0.02);
        assert_eq!(d.status, "ok");
    }
    // A limit lower than the total fails the circuit.
    let strict = evload::balance(&i, &rep, &ev, &BalanceIn { total_vdrop_max_pct: Some(0.001), ..bal.clone() });
    assert!(strict.drops.iter().all(|d| d.status == "total over limit"));
    // A single-phase supply has nothing to balance; missing feeder inputs are named.
    let single = parse(&inputs());
    let rs = mep::compute(&p.doc.model, &single);
    let s = evload::balance(&single, &rs, &ev, &bal);
    assert!(s.boards.is_empty() && s.notes.iter().any(|n| n.contains("single-phase")));
    let m = evload::balance(&i, &rep, &ev, &BalanceIn::default());
    assert!(m.missing.iter().any(|x| x.contains("max_imbalance_pct")) && m.missing.iter().any(|x| x.contains("feeder_length_m")));
    let _ = evr;
}

// ------------------------------------------------------------------ ducts around the structure

fn duct_inputs(ceiling: i64, avoid: bool) -> MepInputs {
    let mut v = inputs();
    v["routing"]["ceiling_height_mm"] = json!(ceiling);
    if avoid {
        v["routing"]["avoid_structure"] = json!(true);
        v["routing"]["structure_clearance_mm"] = json!(50);
    }
    parse(&v)
}

#[test]
fn a_duct_goes_around_the_column_and_under_the_beam_when_the_ceiling_void_allows() {
    let mut p = plan();
    let rep = mep::compute(&p.doc.model, &duct_inputs(2100, true));
    let supply: Vec<_> = rep.runs.iter().filter(|r| r.system == "duct" && r.role == "supply").collect();
    assert!(supply.len() > 1, "the trunk is split into legs: {supply:?}");
    assert!(supply.iter().any(|r| r.p1[2] != r.p2[2]), "a leg changes height to pass under the beam");
    assert!(supply.iter().map(|r| r.p1[2]).any(|z| z < 2850), "and part of the duct runs below the soffit duct height");
    assert!(supply.iter().all(|r| [r.p1[0] != r.p2[0], r.p1[1] != r.p2[1], r.p1[2] != r.p2[2]].iter().filter(|b| **b).count() == 1), "every leg is along one axis");
    // The ends are the room centres, as before.
    assert!(supply.iter().any(|r| (r.p1[0], r.p1[1]) == (3000, 2000)) && supply.iter().any(|r| (r.p2[0], r.p2[1]) == (9000, 2000)));
    assert!(rep.warnings.iter().any(|w| w.contains("routed around the structure")), "{:?}", rep.warnings);
    // Applied to the model, the duct clashes with neither the beam nor the column.
    mep::apply_with_runs(&mut p.doc, &rep.points, &rep.runs).unwrap();
    let scene = nnx_core::model3d::build(&p.doc.model);
    let report = clash::detect(&scene, &ClashOpts::default());
    let ids: Vec<String> = p.doc.model.elements.values().filter(|e| e.kind == "MepRun" && e.props["system"] == "duct" && e.props["role"] == "supply").map(|e| e.id.to_string()).collect();
    let hits: Vec<_> = report.clashes.iter().filter(|c| ids.contains(&c.run_id) && (c.struct_id == p.beam.to_string() || c.struct_id == p.column.to_string())).collect();
    assert!(hits.is_empty(), "{hits:?}");
}

#[test]
fn where_the_ceiling_void_has_no_room_under_the_beam_the_crossing_is_reported_and_the_clash_check_finds_it() {
    let mut p = plan();
    let rep = mep::compute(&p.doc.model, &duct_inputs(2600, true));
    assert!(rep.warnings.iter().any(|w| w.contains("cannot be avoided")), "{:?}", rep.warnings);
    let supply: Vec<_> = rep.runs.iter().filter(|r| r.system == "duct" && r.role == "supply").collect();
    assert_eq!(supply.len(), 1, "the straight leg is kept");
    mep::apply_with_runs(&mut p.doc, &rep.points, &rep.runs).unwrap();
    let scene = nnx_core::model3d::build(&p.doc.model);
    let report = clash::detect(&scene, &ClashOpts::default());
    assert!(report.clashes.iter().any(|c| c.struct_id == p.beam.to_string()), "the unavoidable crossing is a clash");
    // Without the option nothing changes; the clearance is required with it.
    let plain = mep::compute(&p.doc.model, &duct_inputs(2600, false));
    assert!(plain.warnings.iter().all(|w| !w.contains("around the structure")));
    let mut v = inputs();
    v["routing"]["avoid_structure"] = json!(true);
    let m = mep::compute(&p.doc.model, &parse(&v));
    assert!(m.missing.get("routing.structure").is_some_and(|x| x[0].contains("structure_clearance_mm")));
}

// ------------------------------------------------------------------ greywater

fn grey_in() -> GreyIn {
    GreyIn {
        fixtures: vec!["basin".into(), "shower".into()],
        fixture_l_day: [("basin".to_string(), 60.0), ("shower".to_string(), 150.0)].into_iter().collect(),
        condensate: false,
        condensate_l_day_per_unit: None,
        unit_x_mm: Some(11000.0),
        unit_y_mm: Some(-1500.0),
        reuse_demand_l_day: Some(150.0),
        storage_days: Some(2.0),
    }
}

#[test]
fn greywater_is_separated_from_the_soil_load_volume_storage_and_drains_are_given() {
    let mut p = plan();
    let i = parse(&inputs());
    let rep = mep::compute(&p.doc.model, &i);
    let r = greywater::design(&p.doc.model, &i, &rep, &grey_in());
    assert!(r.missing.is_empty(), "{:?}", r.missing);
    let b = r.rooms.iter().find(|x| x.room == "b").unwrap();
    // Basin 1 + shower 2 = 3 DFU of greywater (40 mm); the wc is 4 DFU of soil (75 mm).
    assert_eq!((b.grey_dfu, b.soil_dfu, b.grey_drain_mm, b.soil_drain_mm), (3.0, 4.0, Some(40.0), Some(75.0)));
    near(b.grey_l_day, 210.0, 1e-9);
    assert_eq!((r.supply_l_day, r.surplus_l_day, r.storage_m3), (210.0, Some(60.0), Some(0.3)));
    // Two legs from the room centre (9000, 2000) to the unit (11000, -1500), falling 1.5 % from 400 mm below the floor.
    assert_eq!(r.runs.len(), 2);
    assert_eq!((r.runs[0].p1, r.runs[0].p2), ([9000, 2000, -400], [11000, 2000, -430]));
    assert_eq!((r.runs[1].p1, r.runs[1].p2), ([11000, 2000, -430], [11000, -1500, -483]));
    assert!(r.runs.iter().all(|u| u.system == "drain" && u.role == "grey" && u.width == 40));
    mep::apply_with_runs(&mut p.doc, &rep.points, &r.runs).unwrap();
    assert!(p.doc.model.elements.values().any(|e| e.kind == "MepRun" && e.props["role"] == "grey"));
    // Condensate counts only when you ask and give its litres.
    let mut c = grey_in();
    c.condensate = true;
    assert!(greywater::design(&plan().doc.model, &i, &rep, &c).missing.iter().any(|m| m.contains("condensate_l_day_per_unit")));
    c.condensate_l_day_per_unit = Some(10.0);
    let with = greywater::design(&plan().doc.model, &i, &rep, &c);
    let units: f64 = rep.hvac.as_ref().unwrap().rooms.iter().map(|r| r.units as f64).sum();
    near(with.condensate_l_day, units * 10.0, 1e-9);
    near(with.supply_l_day, 210.0 + units * 10.0, 1e-9);
    // Missing inputs are named.
    let mut m = grey_in();
    m.fixture_l_day.remove("shower");
    m.unit_x_mm = None;
    m.reuse_demand_l_day = None;
    let r = greywater::design(&plan().doc.model, &i, &rep, &m);
    assert!(r.missing.iter().any(|x| x.contains("fixture_l_day for: shower")) && r.missing.iter().any(|x| x.contains("unit_x_mm")) && r.missing.iter().any(|x| x.contains("reuse_demand")) && r.runs.is_empty());
    assert!(greywater::design(&plan().doc.model, &i, &rep, &GreyIn::default()).missing[0].contains("grey.fixtures"));
    assert!(greywater::design(&plan().doc.model, &MepInputs::default(), &rep, &grey_in()).missing[0].contains("plumbing inputs"));
}

// ------------------------------------------------------------------ head loss and pumps

fn hydra_in() -> HydraIn {
    HydraIn { hazen_c: Some(140.0), fittings_pct: Some(20.0), source_z_mm: None, outlet_z_mm: None, residual_head_m: Some(10.0), inlet_head_m: Some(0.0), pump_efficiency: Some(0.6), motor_margin: Some(0.15), standby: false, farthest_room: None }
}

#[test]
fn hazen_williams_gives_the_textbook_loss_and_the_pump_follows_the_head() {
    // 100 m of 100 mm pipe at 10 L/s, C = 130: about 1.9 m (the usual 1.9 m per 100 m).
    near(hazen_williams_m(100.0, 10.0, 100.0, 130.0), 1.9, 0.05);
    assert!(hazen_williams_m(100.0, 20.0, 100.0, 130.0) > 3.5 * hazen_williams_m(100.0, 10.0, 100.0, 130.0), "Q^1.852");
    assert!(hazen_williams_m(100.0, 10.0, 50.0, 130.0) > 25.0 * hazen_williams_m(100.0, 10.0, 100.0, 130.0), "D^-4.87");
    let p = plan();
    let i = parse(&inputs());
    let rep = mep::compute(&p.doc.model, &i);
    let r = hydraulics::size(&p.doc.model, &i, &rep, &hydra_in());
    assert!(r.missing.is_empty(), "{:?}", r.missing);
    assert_eq!(r.room.as_deref(), Some("b"));
    // Ground tank at (-2000, 2000), z 0; the room's centre (9000, 2000): 11 m of main pipe, no branch; the riser to 600 mm.
    near(r.static_m, 0.6, 1e-9);
    assert_eq!(r.segments.len(), 3);
    near(r.segments[0].length_m, 11.0, 1e-9);
    let fric: f64 = r.segments.iter().map(|s| s.head_loss_m).sum();
    near(r.friction_m, fric, 0.02);
    near(r.fittings_m, r.friction_m * 0.2, 0.01);
    let tdh = 0.6 + r.friction_m * 1.2 + 10.0;
    near(r.tdh_m.unwrap(), tdh, 0.05);
    assert_eq!(r.needs_pump, Some(true));
    let hyd = 1000.0 * 9.81 * rep.plumbing.as_ref().unwrap().design_flow_lps / 1000.0 * r.tdh_m.unwrap() / 1000.0;
    near(r.hydraulic_kw.unwrap(), hyd, 0.01);
    near(r.motor_kw.unwrap(), hyd / 0.6 * 1.15, 0.02);
    assert_eq!(r.pumps, Some(1));
    // Enough pressure at the inlet: no pump. A standby pump doubles the count.
    let mut high = hydra_in();
    high.inlet_head_m = Some(30.0);
    let h = hydraulics::size(&p.doc.model, &i, &rep, &high);
    assert_eq!((h.needs_pump, h.motor_kw), (Some(false), None));
    let mut sb = hydra_in();
    sb.standby = true;
    assert_eq!(hydraulics::size(&p.doc.model, &i, &rep, &sb).pumps, Some(2));
    // Missing inputs are named; a roof tank needs the height to reach.
    let m = hydraulics::size(&p.doc.model, &i, &rep, &HydraIn::default());
    for n in ["hazen_c", "fittings_pct", "residual_head_m", "pump_efficiency", "motor_margin"] {
        assert!(m.missing.iter().any(|x| x.contains(n)), "{n}");
    }
    let mut roof = inputs();
    roof["routing"]["tank"]["kind"] = json!("roof");
    let ri = parse(&roof);
    let rr = mep::compute(&p.doc.model, &ri);
    assert!(hydraulics::size(&p.doc.model, &ri, &rr, &hydra_in()).missing.iter().any(|x| x.contains("outlet_z_mm")));
    assert!(MepExtInputs { hydra: HydraIn { pump_efficiency: Some(1.5), ..Default::default() }, ..Default::default() }.check().is_err());
}
