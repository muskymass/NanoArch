//! Milestone 2: materials catalog, thermal envelope, audits, Malath checklist, revisions and the certificate.

use nnx_core::audits::{self, lightwell_min};
use nnx_core::certificate::{self, CertData};
use nnx_core::compliance::*;
use nnx_core::drawing::{DrawOpts, plan_svg};
use nnx_core::malath;
use nnx_core::materials;
use nnx_core::project::{NanoArchProject, ProjectMeta};
use nnx_core::revisions;
use nnx_core::sbc;
use nnx_core::thermal::{self, assembly_u};
use nnx_core::typed::{Door, Slab, Space, Wall, Window, element_of};
use nnx_core::*;
use serde_json::json;

fn near(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol, "{a} vs {b}");
}

fn space(key: &str, func: &str, x: i64, y: i64, w: i64, h: i64) -> Space {
    Space { func: func.into(), requirement_key: key.into(), area: (w * h) as f64 / 1e6, target_area: (w * h) as f64 / 1e6, x, y, w, h, boundary: vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]], net_area: None, glazing_ratio: None, extra: Default::default() }
}

fn wall(x1: i64, y1: i64, x2: i64, y2: i64, a: &str) -> Wall {
    Wall { x1, y1, x2, y2, thickness: 200, height: 3000, length: (x2 - x1).abs() + (y2 - y1).abs(), is_exterior: true, room_a: a.into(), room_b: None, extra: Default::default() }
}

struct House {
    doc: Document,
    south: Id,
}

/// Two living rooms 6 x 4 m side by side, four exterior walls 3 m high, a 900 mm entrance door and a 1500 x 1200 window on the south
/// wall, and a roof slab on each room.
fn house() -> House {
    let mut cmds = vec![Command::Create(element_of(&space("a", "living", 0, 0, 6000, 4000), Source::User)), Command::Create(element_of(&space("b", "living", 6000, 0, 6000, 4000), Source::User))];
    let s = element_of(&wall(0, 0, 12000, 0, "a"), Source::User);
    let south = s.id;
    cmds.push(Command::Create(s));
    for w in [wall(12000, 0, 12000, 4000, "b"), wall(12000, 4000, 0, 4000, "b"), wall(0, 4000, 0, 0, "a")] {
        cmds.push(Command::Create(element_of(&w, Source::User)));
    }
    let door = Door { width: 900, height: 2100, offset_center: 1000, is_egress: true, from: "a".into(), to: "out".into(), opens_into: "a".into(), extra: Default::default() };
    let win = Window { width: 1500, height: 1200, sill_height: 900, offset_center: 4000, room: "a".into(), extra: Default::default() };
    cmds.push(Command::Create(element_of(&door, Source::User).hosted_by(south)));
    cmds.push(Command::Create(element_of(&win, Source::User).hosted_by(south)));
    for (key, x) in [("a", 0), ("b", 6000)] {
        cmds.push(Command::Create(element_of(&Slab { x, y: 0, w: 6000, h: 4000, thickness: 150, kind: "roof".into(), room: key.into(), extra: Default::default() }, Source::User)));
    }
    let mut doc = Document::default();
    doc.commit("house", Origin::User, cmds).unwrap();
    House { doc, south }
}

fn project(c: ComplianceInputs) -> NanoArchProject {
    NanoArchProject { meta: ProjectMeta { climate_zone: Some("Riyadh".into()), compliance: c, ..Default::default() }, ..Default::default() }
}

fn pick(m: &str, mm: f64) -> LayerPick {
    LayerPick { material: m.into(), thickness_mm: Some(mm), ..Default::default() }
}

// ------------------------------------------------------------------ materials

#[test]
fn the_catalog_has_only_sourced_values_and_everything_else_asks_for_them() {
    let all = &materials::CATALOG;
    assert_eq!(all.iter().filter(|m| m.kind == "insulation" && !m.needs_user_values).count(), 12);
    for m in all.iter() {
        if m.needs_user_values {
            assert!(m.lambda.is_none() && m.density_kg_m3.is_none() && m.source.is_none(), "{} has no value in the files", m.id);
        } else {
            assert!(m.lambda.is_some() && m.density_kg_m3.is_some(), "{}", m.id);
            let s = m.source.unwrap();
            assert!(s.page == 195 || s.page == 199, "{}: page {}", m.id, s.page);
        }
    }
    let x = materials::find("xps_35").unwrap();
    assert_eq!((x.lambda, x.density_kg_m3, x.min_thickness_cm), (Some(0.027), Some(35.0), Some([5.5, 5.0, 4.0])));
    for id in ["aac_block", "volcanic_block_saso", "pir_board", "glass_low_e_double", "glass_low_e_triple", "rebar_sabic_g60", "rebar_sabic_g40"] {
        assert!(materials::find(id).unwrap().needs_user_values, "{id}");
    }
    assert!(materials::find("nope").is_none());
    let j = materials::catalog_json();
    assert_eq!(j["materials"].as_array().unwrap().len(), 22);
    assert!(j["film_source"].as_str().unwrap().contains("p.199"));
}

// ------------------------------------------------------------------ thermal

#[test]
fn the_assembly_u_reproduces_the_guides_own_worked_example() {
    // Page 199: plaster 3 cm, hollow block 10 cm, rock wool 150 kg/m3 10 cm, hollow block 15 cm, plaster 2 cm; Rsi 0.120, Rse 0.03.
    let wall = [pick("plaster_ext_1856", 30.0), pick("block_hollow_1216", 100.0), pick("rockwool_150", 100.0), pick("block_hollow_1216", 150.0), pick("plaster_int_1100", 20.0)];
    let u = assembly_u(&wall, Some(materials::RSI_EXAMPLE), Some(materials::RSE_EXAMPLE), "wall").unwrap();
    near(u, 0.395, 0.0005);
    // A material without a value in the files needs your lambda, and a missing thickness or resistance is named.
    let e = assembly_u(&[pick("aac_block", 200.0)], Some(0.12), Some(0.03), "wall").unwrap_err();
    assert_eq!(e, vec!["wall: conductivity (lambda) of 'aac_block'"]);
    let with = LayerPick { material: "aac_block".into(), thickness_mm: Some(200.0), lambda: Some(0.16), ..Default::default() };
    near(assembly_u(&[with], Some(0.12), Some(0.03), "wall").unwrap(), 1.0 / (0.15 + 1.25), 1e-9);
    let e = assembly_u(&[LayerPick { material: "eps_30".into(), ..Default::default() }], None, None, "roof").unwrap_err();
    assert!(e.iter().any(|m| m.contains("thickness of 'eps_30'")) && e.contains(&"surface resistance Rsi".to_string()));
    assert!(assembly_u(&[], Some(0.1), Some(0.1), "wall").unwrap_err()[0].contains("no layers"));
}

fn thermal_inputs() -> ComplianceInputs {
    ComplianceInputs {
        thermal: ThermalInputs {
            wall_layers: vec![pick("plaster_ext_1856", 30.0), pick("block_hollow_1216", 100.0), pick("rockwool_150", 100.0), pick("block_hollow_1216", 150.0), pick("plaster_int_1100", 20.0)],
            roof_layers: vec![pick("xps_35", 150.0)],
            rsi: Some(0.12),
            rse: Some(0.03),
            window_u: Some(2.0),
            window_shgc: Some(0.2),
            bridge_pct: None,
            band_ratios: Some(vec![0.5, 0.65, 0.8, 1.0, 1.25]),
        },
        ..Default::default()
    }
}

#[test]
fn the_weighted_u_is_area_weighted_compared_with_the_zone_limits_and_banded() {
    let h = house();
    let p = project(thermal_inputs());
    let r = thermal::report(&h.doc.model, &p);
    assert_eq!(r.zone, Some(1));
    // Areas: walls 92.31 m2 net of the door and the window, roof 48 m2, window 1.8 m2, door 1.89 m2 (left out).
    let a = |c: &str| r.rows.iter().find(|x| x.class.starts_with(c)).unwrap();
    near(a("wall").area_m2, 92.31, 0.005);
    near(a("roof").area_m2, 48.0, 1e-9);
    near(a("window").area_m2, 1.8, 1e-9);
    near(r.door_area_m2, 1.89, 1e-9);
    assert_eq!((a("wall").limit, a("roof").limit, a("window").limit), (Some(0.403), Some(0.272), Some(2.668)));
    assert_eq!((a("wall").status.as_str(), a("roof").status.as_str(), a("window").status.as_str()), ("ok", "ok", "ok"));
    let (wu, ru) = (a("wall").u.unwrap(), a("roof").u.unwrap());
    near(ru, 1.0 / (0.15 + 0.15 / 0.027), 0.001);
    let tot = 92.31 + 48.0 + 1.8;
    near(r.weighted_u.unwrap(), (92.31 * wu + 48.0 * ru + 1.8 * 2.0) / tot, 0.002);
    near(r.reference_u.unwrap(), (92.31 * 0.403 + 48.0 * 0.272 + 1.8 * 2.668) / tot, 0.002);
    let ratio = r.weighted_u.unwrap() / r.reference_u.unwrap();
    near(r.ratio.unwrap(), ratio, 0.002);
    assert_eq!(r.band, Some("D"), "ratio {ratio}: above 0.8 and not above 1.0");
    assert!(r.caveat_en.contains("conflict") && r.limits_source.contains("p.172"));
    assert!(r.notes.iter().any(|n| n.contains("not an SBC or SEC rating")) && r.notes.iter().any(|n| n.contains("doors")));
    near(r.glazing_share_pct, 1.9, 0.05);
    // A thermal-bridge allowance raises the weighted U; a worse wall exceeds its limit and the band falls.
    let mut c = thermal_inputs();
    c.thermal.bridge_pct = Some(10.0);
    let r2 = thermal::report(&h.doc.model, &project(c));
    near(r2.weighted_u.unwrap(), r.weighted_u.unwrap() * 1.1, 0.002);
    let mut bad = thermal_inputs();
    bad.thermal.wall_layers = vec![pick("block_hollow_1216", 200.0)];
    let r3 = thermal::report(&h.doc.model, &project(bad));
    assert_eq!(r3.rows[0].status, "exceeds limit");
    assert!(r3.ratio.unwrap() > r.ratio.unwrap());
}

#[test]
fn missing_thermal_inputs_are_named_and_no_band_is_invented() {
    let h = house();
    let r = thermal::report(&h.doc.model, &project(ComplianceInputs::default()));
    assert!(r.weighted_u.is_none() && r.band.is_none());
    for needle in ["wall: no layers", "roof: no layers", "surface resistance Rsi", "window U-value"] {
        assert!(r.missing.iter().any(|m| m.contains(needle)), "{needle} in {:?}", r.missing);
    }
    let mut c = thermal_inputs();
    c.thermal.band_ratios = None;
    let r = thermal::report(&h.doc.model, &project(c));
    assert!(r.weighted_u.is_some() && r.band.is_none() && r.missing.iter().any(|m| m.contains("band limits")));
    let no_zone = NanoArchProject { meta: ProjectMeta { compliance: thermal_inputs(), ..Default::default() }, ..Default::default() };
    let r = thermal::report(&h.doc.model, &no_zone);
    assert!(r.zone.is_none() && r.missing.iter().any(|m| m.contains("climate zone")) && r.rows[0].limit.is_none());
}

// ------------------------------------------------------------------ audits

fn access(a: AccessInputs) -> ComplianceInputs {
    ComplianceInputs { access: a, ..Default::default() }
}

#[test]
fn accessibility_uses_only_the_limits_you_enter_and_names_the_missing_ones() {
    let h = house();
    let none = audits::accessibility(&h.doc.model, &project(ComplianceInputs::default()));
    assert!(none.rows.iter().all(|r| r.status == "skipped") && none.rows[0].en.contains("clear width is missing"));
    let ok = audits::accessibility(&h.doc.model, &project(access(AccessInputs { door_min_clear_mm: Some(900.0), source: Some("my circular".into()), ..Default::default() })));
    let d = ok.rows.iter().find(|r| r.rule == "access.door_width").unwrap();
    assert_eq!((d.status, d.value, d.limit), ("ok", Some(900.0), Some(900.0)));
    assert!(d.source.contains("my circular") && ok.rows.iter().any(|r| r.en.contains("nominal door width")));
    let bad = audits::accessibility(&h.doc.model, &project(access(AccessInputs { door_min_clear_mm: Some(950.0), door_deduction_mm: Some(0.0), ..Default::default() })));
    assert_eq!(bad.rows.iter().find(|r| r.rule == "access.door_width").unwrap().status, "violation");
    assert!(bad.rows[0].en.contains("entrance"));
    // Ramp 1:N: rise 500, run 6000 is 1:12.
    let ramp = |rise: f64, run: f64, n: Option<f64>| audits::accessibility(&h.doc.model, &project(access(AccessInputs { entrance_ramp_rise_mm: Some(rise), entrance_ramp_run_mm: Some(run), ramp_max_ratio_n: n, ..Default::default() })));
    let st = |r: audits::AuditReport| r.rows.iter().find(|x| x.rule == "access.ramp").unwrap().status;
    assert_eq!(st(ramp(500.0, 6000.0, Some(12.0))), "ok");
    assert_eq!(st(ramp(500.0, 5000.0, Some(12.0))), "violation");
    assert_eq!(st(ramp(500.0, 6000.0, None)), "skipped");
    // Turning circle in the accessible rooms: rooms a and b are 4 m on the short side.
    let tc = |d: Option<f64>| audits::accessibility(&h.doc.model, &project(access(AccessInputs { turning_circle_mm: d, accessible_rooms: vec!["a".into(), "ghost".into()], ..Default::default() })));
    let r = tc(Some(1500.0));
    assert_eq!(r.rows.iter().find(|x| x.rule == "access.turning_circle").unwrap().status, "ok");
    assert!(r.rows.iter().any(|x| x.rule == "access.room" && x.status == "warn"));
    assert_eq!(tc(Some(4500.0)).rows.iter().find(|x| x.rule == "access.turning_circle").unwrap().status, "violation");
    assert!(tc(None).rows.iter().any(|x| x.rule == "access.turning_circle" && x.status == "skipped"));
}

#[test]
fn lightwell_sizes_follow_the_table_of_clause_5_1_6() {
    assert_eq!(lightwell_min(1, false), Some((0.90, 1.8)));
    assert_eq!(lightwell_min(1, true), Some((1.5, 1.8)));
    assert_eq!(lightwell_min(2, false), Some((1.5, 1.8)));
    assert_eq!(lightwell_min(2, true), Some((1.8, 1.8)));
    assert_eq!(lightwell_min(3, false), Some((1.8, 2.4)));
    assert_eq!(lightwell_min(3, true), Some((2.1, 2.4)));
    assert_eq!(lightwell_min(4, false), Some((2.1, 3.0)));
    assert_eq!(lightwell_min(4, true), Some((2.4, 3.0)));
    // Above four floors: 30 cm more width and 60 cm more length per floor.
    assert_eq!(lightwell_min(5, false), Some((2.4, 3.6)));
    assert_eq!(lightwell_min(6, true), Some((3.0, 4.2)));
    assert_eq!(lightwell_min(15, false), None);
    assert_eq!(lightwell_min(0, false), None);
}

fn with_wet(h: &mut House) {
    let cmds = vec![Command::Create(element_of(&space("k", "kitchen", 0, 4000, 3000, 3000), Source::User)), Command::Create(element_of(&space("w", "bathroom", 3000, 4000, 2000, 2000), Source::User))];
    h.doc.commit("wet rooms", Origin::User, cmds).unwrap();
}

#[test]
fn every_kitchen_and_bathroom_needs_a_window_a_compliant_lightwell_or_an_exhaust() {
    let mut h = house();
    with_wet(&mut h);
    let find = |rep: &audits::AuditReport, key: &str| rep.rows.iter().find(|r| r.rule == "vent.room" && (r.en.contains(&format!("'{key}'")))).map(|r| r.status);
    let r = audits::ventilation(&h.doc.model, &project(ComplianceInputs::default()));
    assert_eq!((find(&r, "k"), find(&r, "w")), (Some("violation"), Some("violation")));
    assert!(r.rows.iter().all(|x| x.clause == "5-1-6 (1), (2)" && x.source.contains("p.27")));
    // A window of the kitchen on the exterior south wall.
    let win = Window { width: 1000, height: 1000, sill_height: 1000, offset_center: 2000, room: "k".into(), extra: Default::default() };
    h.doc.commit("kitchen window", Origin::User, vec![Command::Create(element_of(&win, Source::User).hosted_by(h.south))]).unwrap();
    let r = audits::ventilation(&h.doc.model, &project(ComplianceInputs::default()));
    assert_eq!((find(&r, "k"), find(&r, "w")), (Some("ok"), Some("violation")));
    // The bathroom: an exhaust is allowed; the same exhaust is only a warning for a kitchen without a window.
    let c = ComplianceInputs { vent: VentInputs { exhaust_rooms: vec!["w".into()], lightwells: vec![] }, ..Default::default() };
    assert_eq!(find(&audits::ventilation(&h.doc.model, &project(c)), "w"), Some("ok"));
    let mut g = house();
    with_wet(&mut g);
    let c = ComplianceInputs { vent: VentInputs { exhaust_rooms: vec!["k".into(), "w".into()], lightwells: vec![] }, ..Default::default() };
    let r = audits::ventilation(&g.doc.model, &project(c));
    assert_eq!((find(&r, "k"), find(&r, "w")), (Some("warn"), Some("ok")));
    // Lightwells: 2 floors with windows on two opposite sides need 1.8 x 1.8; 1.5 x 1.8 is too small.
    let wells = |w: f64, l: f64| ComplianceInputs { vent: VentInputs { lightwells: vec![Lightwell { name: "L1".into(), width_m: w, length_m: l, floors: 2, opposite_or_all: true, rooms: vec!["k".into(), "w".into()] }], exhaust_rooms: vec![] }, ..Default::default() };
    let r = audits::ventilation(&g.doc.model, &project(wells(1.8, 1.8)));
    assert_eq!((find(&r, "k"), find(&r, "w")), (Some("ok"), Some("ok")));
    assert!(r.rows.iter().any(|x| x.rule == "vent.lightwell" && x.status == "ok"));
    let r = audits::ventilation(&g.doc.model, &project(wells(1.5, 1.8)));
    assert_eq!((find(&r, "k"), find(&r, "w")), (Some("violation"), Some("violation")));
    assert!(r.rows.iter().any(|x| x.rule == "vent.lightwell" && x.status == "violation" && x.en.contains("need at least 1.80 x 1.80")));
    // A room named in the inputs that is not in the plan is flagged.
    let c = ComplianceInputs { vent: VentInputs { exhaust_rooms: vec!["ghost".into()], lightwells: vec![] }, ..Default::default() };
    assert!(audits::ventilation(&g.doc.model, &project(c)).rows.iter().any(|x| x.rule == "vent.room.unknown"));
    // Rooms that are not wet are not audited.
    assert!(audits::ventilation(&house().doc.model, &project(ComplianceInputs::default())).rows.is_empty());
}

#[test]
fn projections_are_judged_by_clause_5_1_11_and_the_street_width() {
    let h = house();
    let pr = |side: &str, depth: f64, kind: &str, guard: Option<f64>| Projection { name: "P".into(), level: 2, side: side.into(), depth_m: depth, kind: kind.into(), guard_height_m: guard };
    let run = |street: Option<f64>, ps: Vec<Projection>| audits::cantilever(&h.doc.model, &project(ComplianceInputs { street_width_m: street, projections: ps, ..Default::default() }));
    // The front is the entry side (south by default).
    assert_eq!(run(Some(12.0), vec![pr("south", 1.0, "bay", None)]).rows[0].status, "ok");
    assert_eq!(run(Some(12.0), vec![pr("south", 1.5, "bay", None)]).rows[0].status, "violation");
    let narrow = run(Some(8.0), vec![pr("south", 0.5, "bay", None)]);
    assert_eq!(narrow.rows[0].status, "violation");
    assert!(narrow.rows[0].en.contains("under 10 m") && narrow.rows[0].clause == "5-1-11" && narrow.rows[0].source.contains("p.30"));
    assert_eq!(run(None, vec![pr("south", 0.5, "bay", None)]).rows[0].status, "skipped");
    // Side and rear: only a decorative cornice up to 25 cm.
    assert_eq!(run(None, vec![pr("north", 0.5, "bay", None)]).rows[0].status, "violation");
    assert_eq!(run(None, vec![pr("east", 0.2, "cornice", None)]).rows[0].status, "warn");
    assert_eq!(run(None, vec![pr("west", 0.0, "bay", None)]).rows[0].status, "ok");
    // Balcony barrier at least 0.90 m.
    let g = |guard: Option<f64>| run(Some(15.0), vec![pr("south", 1.0, "balcony", guard)]).rows.iter().find(|r| r.rule == "projection.guard").unwrap().status;
    assert_eq!((g(Some(1.0)), g(Some(0.8)), g(None)), ("ok", "violation", "skipped"));
    assert!(run(None, vec![]).rows.is_empty());
    assert!(ComplianceInputs { projections: vec![pr("up", 1.0, "bay", None)], ..Default::default() }.check().is_err());
}

// ------------------------------------------------------------------ Malath

#[test]
fn the_checklist_keeps_its_sources_judges_two_items_from_inputs_and_gates_the_stages_in_order() {
    let mut c = ComplianceInputs::default();
    let r = malath::report(&c);
    assert_eq!(r.items.len(), 11);
    assert!(r.items.iter().all(|i| i.status == "pending"));
    assert!(r.items.iter().filter(|i| i.stage == "joints").all(|i| i.source.contains("added by this application")));
    assert!(r.items.iter().find(|i| i.id == "wet.floor").unwrap().source.contains("item 13") && r.items.iter().find(|i| i.id == "wet.floor").unwrap().source.contains("p.45"));
    assert_eq!(r.stages.iter().map(|s| s.open).collect::<Vec<_>>(), vec![true, false, false, false]);
    assert!(r.disclaimer_en.contains("not the insurer's list"));
    // Soil borings are judged from the soil report: fewer than 3 is a defect (inspectors' item 1, p.15).
    c.soil = SoilReport { status: "received".into(), borings: Some(2), reference: None };
    assert_eq!(malath::report(&c).items[0].status, "rejected");
    c.soil.borings = Some(3);
    assert_eq!(malath::report(&c).items[0].status, "accepted");
    assert!(malath::set_status(&mut c, "soil.borings", "accepted", "").unwrap_err().contains("judged from the inputs"));
    for id in ["soil.level", "soil.compaction", "soil.groundwater"] {
        malath::set_status(&mut c, id, "accepted", "ok").unwrap();
    }
    let r = malath::report(&c);
    assert!(r.stages[0].complete && r.stages[1].open && !r.stages[2].open);
    malath::set_status(&mut c, "soil.groundwater", "na", "dry site").unwrap();
    assert!(malath::report(&c).stages[0].complete);
    malath::set_status(&mut c, "soil.level", "rejected", "").unwrap();
    assert!(!malath::report(&c).stages[1].open);
    malath::set_status(&mut c, "soil.level", "pending", "").unwrap();
    assert!(!c.malath.contains_key("soil.level"));
    assert!(malath::set_status(&mut c, "nope", "accepted", "").is_err() && malath::set_status(&mut c, "soil.level", "done", "").is_err());
    assert!(c.check().is_ok());
    c.malath.insert("x".into(), MalathState { status: "bad".into(), note: String::new() });
    assert!(c.check().is_err());
}

#[test]
fn the_concrete_cover_is_checked_against_table_4a5() {
    let cover = |e: &str, m: &str, d: f64, s: f64| malath::cover_result(&CoverInput { exposure: Some(e.into()), member: Some(m.into()), bar_dia_mm: Some(d), specified_mm: Some(s) });
    let r = cover("ground", "slab", 12.0, 50.0);
    assert_eq!((r.required_mm, r.ok), (Some(75.0), Some(false)));
    assert!(r.source.contains("p.134"));
    assert_eq!(cover("weather", "frame", 20.0, 50.0).ok, Some(true));
    assert_eq!(cover("weather", "frame", 12.0, 40.0).required_mm, Some(40.0));
    assert_eq!(cover("protected", "slab", 12.0, 20.0).ok, Some(true));
    let none = cover("protected", "slab", 28.0, 40.0);
    assert!(none.ok.is_none() && none.missing[0].contains("Table 4A5 gives no cover"));
    let part = malath::cover_result(&CoverInput { exposure: Some("ground".into()), ..Default::default() });
    assert!(part.ok.is_none() && part.missing.iter().any(|m| m.contains("member")) && part.missing.iter().any(|m| m.contains("bar diameter")));
    // It drives the checklist item.
    let mut c = ComplianceInputs { cover: Some(CoverInput { exposure: Some("ground".into()), member: Some("frame".into()), bar_dia_mm: Some(16.0), specified_mm: Some(75.0) }), ..Default::default() };
    let r = malath::report(&c);
    let it = r.items.iter().find(|i| i.id == "found.cover").unwrap();
    assert_eq!(it.status, "accepted");
    assert!(it.note.contains("75") && malath::set_status(&mut c, "found.cover", "rejected", "").is_err());
}

// ------------------------------------------------------------------ revisions

#[test]
fn the_baseline_tracks_added_modified_and_removed_elements_by_id() {
    let mut h = house();
    let mut p = project(ComplianceInputs::default());
    assert!(p.meta.baseline.is_none());
    revisions::set_baseline(&h.doc.model, &mut p, "Issued for permit", 1_700_000_000);
    let base = p.meta.baseline.clone().unwrap();
    assert!(base.elements.len() == 10 && !base.elements.values().any(|(k, _)| k == "Project"));
    assert!(revisions::changes(&h.doc.model, &base).is_empty());
    // Modify a wall, add a room, delete a slab.
    let slab = h.doc.model.elements.values().find(|e| e.kind == "Slab").unwrap().id;
    h.doc.commit("edit", Origin::User, vec![Command::SetProp { id: h.south, key: "height".into(), value: Some(json!(3200)) }]).unwrap();
    let added = element_of(&space("n", "bedroom", 0, 4000, 3000, 3000), Source::User);
    let added_id = added.id;
    h.doc.commit("add", Origin::User, vec![Command::Create(added), Command::Delete(slab)]).unwrap();
    let ch = revisions::changes(&h.doc.model, &base);
    assert_eq!(revisions::counts(&ch), (1, 1, 1));
    let st = |id: Id| ch.iter().find(|c| c.id == id.to_string()).map(|c| c.status);
    assert_eq!((st(h.south), st(added_id), st(slab)), (Some("modified"), Some("added"), Some("removed")));
    // Undo brings the model back to the baseline.
    h.doc.undo();
    h.doc.undo();
    assert!(revisions::changes(&h.doc.model, &base).is_empty());
}

#[test]
fn revision_clouds_surround_the_changed_geometry_and_a_new_baseline_closes_the_old_one_into_the_history() {
    let mut h = house();
    let mut p = project(ComplianceInputs::default());
    revisions::set_baseline(&h.doc.model, &mut p, "Rev A", 1_700_000_000);
    h.doc.commit("edit", Origin::User, vec![Command::SetProp { id: h.south, key: "height".into(), value: Some(json!(3200)) }]).unwrap();
    let base = p.meta.baseline.clone().unwrap();
    let ch = revisions::changes(&h.doc.model, &base);
    let opts = DrawOpts::default();
    let svg = plan_svg(&h.doc.model, None, &opts).unwrap();
    assert!(!svg.contains("rev-cloud"));
    let with = revisions::add_clouds(&svg, &h.doc.model, None, &opts, &ch);
    assert!(with.contains("class=\"rev-cloud\"") && with.contains("data-status=\"modified\"") && with.contains(&h.south.to_string()));
    assert!(with.contains(" A") && with.trim_end().ends_with("</svg>"), "scalloped arcs and a closed svg");
    assert_eq!(revisions::add_clouds(&svg, &h.doc.model, None, &opts, &[]), svg);
    // The history table: the open revision, then after a new baseline the closed one with its counts.
    let html = revisions::history_html(&p, &ch, false);
    assert!(html.contains("Revision history") && html.contains("Rev A") && html.contains("2023-11-14") && html.contains("current"));
    revisions::set_baseline(&h.doc.model, &mut p, "Rev B", 1_700_100_000);
    assert_eq!(p.meta.revision_history.len(), 1);
    let closed = &p.meta.revision_history[0];
    assert_eq!((closed.rev, closed.name.as_str(), closed.added, closed.modified, closed.removed), (1, "Rev A", 0, 1, 0));
    let ar = revisions::history_html(&p, &[], true);
    assert!(ar.contains("سجل المراجعات") && ar.contains("Rev B") && ar.contains("Rev A"));
    assert_eq!(revisions::date(0), "1970-01-01");
    assert_eq!(revisions::date(1_700_000_000), "2023-11-14");
    assert_eq!(revisions::date(951_782_400), "2000-02-29");
}

// ------------------------------------------------------------------ certificate

#[test]
fn the_certificate_cites_clauses_and_pages_and_says_what_it_is_not() {
    let mut h = house();
    with_wet(&mut h);
    let mut c = thermal_inputs();
    c.access.door_min_clear_mm = Some(900.0);
    c.access.source = Some("my circular".into());
    c.cover = Some(CoverInput { exposure: Some("ground".into()), member: Some("slab".into()), bar_dia_mm: Some(12.0), specified_mm: Some(75.0) });
    c.projections = vec![Projection { name: "Balcony".into(), level: 2, side: "north".into(), depth_m: 1.0, kind: "balcony".into(), guard_height_m: Some(0.8) }];
    let mut p = project(c);
    p.meta.title = "Test villa".into();
    revisions::set_baseline(&h.doc.model, &mut p, "Rev A", 1_700_000_000);
    let ch = revisions::changes(&h.doc.model, p.meta.baseline.as_ref().unwrap());
    let d = CertData { model: &h.doc.model, project: &p, changes: &ch, date: "2026-11-19".into() };
    let en = certificate::certificate_html(&d, false);
    for needle in [
        "NOT an official certificate of Balady", "Test villa", "5-1-4", "5-1-15 (2)", "p.26", "p.31", "ashtratat", "room_min_area_m2.bedroom", "Thermal envelope (SBC 602)",
        "Weighted U", "5-1-6", "p.27", "5-1-11", "p.30", "violation", "my circular", "Latent-defects (Malath)", "inspectors' checklist, item 13", "Revision history", "Rev A",
        "Licensed engineer", "p.134", "Audit trail summary", "conflict",
    ] {
        assert!(en.contains(needle), "missing {needle:?}");
    }
    assert!(en.starts_with("<!doctype html>") && en.contains("dir=\"ltr\"") && en.trim_end().ends_with("</html>"));
    let ar = certificate::certificate_html(&d, true);
    for needle in ["dir=\"rtl\"", "ليست شهادة رسمية", "سجل المراجعات", "غلاف المبنى الحراري", "مراجعة المهندس المرخّص"] {
        assert!(ar.contains(needle) || ar.contains(&needle.replace("ليست", "ليس")), "missing {needle:?}");
    }
    // No limit is printed that was not read from the files or entered: the accessibility limit is the user's own input.
    assert!(!en.contains("0.90 m") || en.contains("5-1-11"));
    // The model's own code report is part of it.
    assert_eq!(sbc::check(&h.doc.model).checked, sbc::check(&h.doc.model).checked);
}

#[test]
fn the_compliance_inputs_refuse_impossible_numbers() {
    let ok = ComplianceInputs::default();
    assert!(ok.check().is_ok());
    for bad in [
        ComplianceInputs { street_width_m: Some(-1.0), ..Default::default() },
        ComplianceInputs { thermal: ThermalInputs { band_ratios: Some(vec![1.0, 0.5, 0.8, 1.0, 1.2]), ..Default::default() }, ..Default::default() },
        ComplianceInputs { thermal: ThermalInputs { band_ratios: Some(vec![0.5, 0.6]), ..Default::default() }, ..Default::default() },
        ComplianceInputs { thermal: ThermalInputs { wall_layers: vec![LayerPick { material: "x".into(), thickness_mm: Some(0.0), ..Default::default() }], ..Default::default() }, ..Default::default() },
        ComplianceInputs { access: AccessInputs { door_min_clear_mm: Some(0.0), ..Default::default() }, ..Default::default() },
        ComplianceInputs { vent: VentInputs { lightwells: vec![Lightwell { name: "L".into(), width_m: 1.0, length_m: 1.0, floors: 0, ..Default::default() }], exhaust_rooms: vec![] }, ..Default::default() },
        ComplianceInputs { soil: SoilReport { status: "maybe".into(), ..Default::default() }, ..Default::default() },
        ComplianceInputs { cover: Some(CoverInput { exposure: Some("sea".into()), ..Default::default() }), ..Default::default() },
    ] {
        assert!(bad.check().is_err(), "{bad:?}");
    }
}
