//! Rates and durations here are SYNTHETIC test values, not productivity data.

use nnx_core::layout::*;
use nnx_core::qto::{self, Qto, QtoOpts};
use nnx_core::schedule::{self, Calendar, Date, RateLib, ScheduleError};
use nnx_core::structural::{self, Loads, Soil, StructOpts};
use nnx_core::walls::{self, WallOpts, WindowOpts};
use nnx_core::*;

// ------------------------------------------------------------------ critical path method

#[test]
fn cpm_matches_a_hand_solved_network() {
    // A(3) -> B(2), A -> C(4), B and C -> D(2), D -> E(1).
    let d = [3, 2, 4, 2, 1];
    let p = vec![vec![], vec![(0, 0)], vec![(0, 0)], vec![(1, 0), (2, 0)], vec![(3, 0)]];
    let c = schedule::cpm(&d, &p);
    assert_eq!(c.es, [0, 3, 3, 7, 9]);
    assert_eq!(c.ef, [3, 5, 7, 9, 10]);
    assert_eq!(c.finish, 10);
    assert_eq!(c.ls, [0, 5, 3, 7, 9]);
    assert_eq!(c.float, [0, 2, 0, 0, 0], "B has 2 days of float, everything else is critical");
}

#[test]
fn cpm_respects_lags_and_zero_duration_tasks() {
    // A(2) -> (lag 3) -> B(1); a zero-duration milestone M after B.
    let c = schedule::cpm(&[2, 1, 0], &[vec![], vec![(0, 3)], vec![(1, 0)]]);
    assert_eq!(c.es, [0, 5, 6]);
    assert_eq!(c.finish, 6);
    assert_eq!(c.float, [0, 0, 0]);
}

// ------------------------------------------------------------------ dates and calendar

#[test]
fn dates_round_trip_and_weekdays_are_right() {
    assert_eq!(Date::new(1970, 1, 1).unwrap().weekday(), 3, "1970-01-01 was a Thursday");
    assert_eq!(Date::parse("2026-11-17").unwrap().weekday(), 1, "2026-11-17 is a Tuesday");
    for z in (-800..40_000).step_by(7) {
        assert_eq!(Date::from_days(z).to_days(), z);
    }
    let d = Date::parse("2028-02-29").unwrap();
    assert_eq!(Date::from_days(d.to_days() + 1).iso(), "2028-03-01");
    assert!(Date::parse("2026-02-30").is_none() && Date::parse("2027-02-29").is_none() && Date::parse("x").is_none());
}

// Monday = 0 ... Sunday = 6. A project setting: here Sunday to Thursday are working days.
const SUN_THU: [bool; 7] = [true, true, true, true, false, false, true];

fn calendar(start: &str) -> Calendar {
    Calendar::new(Date::parse(start).unwrap(), SUN_THU).unwrap()
}

#[test]
fn working_days_skip_the_weekend_and_holidays() {
    let mut c = calendar("2026-11-15"); // a Sunday
    assert_eq!(c.nth_workday(0).iso(), "2026-11-15");
    assert_eq!(c.nth_workday(4).iso(), "2026-11-19", "Thursday");
    assert_eq!(c.nth_workday(5).iso(), "2026-11-22", "skips Friday 20th and Saturday 21st");
    c.holidays.insert(Date::parse("2026-11-16").unwrap());
    assert_eq!(c.nth_workday(1).iso(), "2026-11-17");
    // A start on a rest day moves to the next working day.
    assert_eq!(calendar("2026-11-20").nth_workday(0).iso(), "2026-11-22");
    assert!(Calendar::new(Date::parse("2026-11-15").unwrap(), [false; 7]).is_none());
}

// ------------------------------------------------------------------ rates

fn lib_for(fixed: f64, per_day: f64) -> RateLib {
    let mut lib = RateLib::default();
    for (key, is_fixed) in schedule::template_keys() {
        let r = if is_fixed {
            schedule::Rate { fixed_days: Some(fixed), crews: 1, source: "synthetic".into(), ..Default::default() }
        } else {
            schedule::Rate { per_crew_day: Some(per_day), crews: 1, source: "synthetic".into(), ..Default::default() }
        };
        lib.rates.insert(key.to_string(), r);
    }
    lib
}

#[test]
fn missing_rates_are_an_error_that_names_them() {
    let q = Qto::default();
    let cal = calendar("2026-11-15");
    let all = schedule::template_keys().len();
    // Tasks that exist only when the take-off has the work (insulation, runs, devices) need no rate for an empty take-off.
    let required = all - schedule::optional_keys().len();

    let ScheduleError::MissingRates(keys) = schedule::build(&q, &RateLib::default(), &cal, 1).unwrap_err() else { panic!() };
    assert_eq!(keys.len(), required);

    // The shipped template has every key but no values: still an error, same keys.
    let tpl = RateLib::from_json(include_str!("../packs/schedule-rates-template.json")).unwrap();
    assert_eq!(tpl.rates.len(), all);
    let ScheduleError::MissingRates(keys) = schedule::build(&q, &tpl, &cal, 1).unwrap_err() else { panic!() };
    assert_eq!(keys.len(), required);

    // One missing rate is reported alone.
    let mut lib = lib_for(3.0, 40.0);
    lib.rates.get_mut("excavate").unwrap().per_crew_day = None;
    assert_eq!(schedule::build(&q, &lib, &cal, 1).unwrap_err(), ScheduleError::MissingRates(vec!["excavate".into()]));
    assert_eq!(schedule::build(&q, &lib_for(3.0, 40.0), &cal, 0).unwrap_err(), ScheduleError::BadZones);
}

// ------------------------------------------------------------------ whole house

fn room(key: &str, func: &str, area: f64, zone: Zone, light: bool) -> RoomReq {
    RoomReq { key: key.into(), func: func.into(), target_area_m2: area, min_width_mm: 2500, zone, needs_light: light }
}

fn house() -> Document {
    use Zone::*;
    let b = Brief {
        envelope: Rect { x: 0, y: 0, w: 14_000, h: 18_000 },
        rooms: vec![
            room("majlis", "majlis", 36.0, Public, true),
            room("living", "living", 30.0, Public, true),
            room("dining", "dining", 24.0, Public, true),
            room("kitchen", "kitchen", 20.0, Service, true),
            room("master", "bedroom", 28.0, Private, true),
            room("bed1", "bedroom", 20.0, Private, true),
            room("bed2", "bedroom", 20.0, Private, true),
            room("bed3", "bedroom", 20.0, Private, true),
            room("bath1", "bathroom", 6.0, Service, false),
            room("bath2", "bathroom", 6.0, Service, false),
            room("bath3", "bathroom", 6.0, Service, false),
        ],
        adjacency: vec![],
        entry: Side::South,
        entry_at: None,
    };
    let layout = generate(&b, &Options { seed: 7, ..Options::default() }).unwrap().remove(0);
    let plan = walls::build(&layout, b.envelope, b.entry, &WallOpts::default());
    let wp = walls::place_windows(&layout, &plan, &WallOpts::default(), &WindowOpts::default());
    let so = StructOpts {
        loads: Some(Loads { dead_kpa: 5.0, live_kpa: 2.0, source: "synthetic".into() }),
        soil: Some(Soil { bearing_kpa: 150.0, source: "synthetic".into() }),
        max_span_mm: Some(5000),
        ..StructOpts::default()
    };
    let sp = structural::build_with_openings(&layout, &plan, &wp, b.envelope, &so);
    let space_cmds = to_commands(&layout, None);
    let ids = walls::space_ids(&space_cmds);
    let mut all = space_cmds;
    all.extend(walls::to_commands(&plan, &ids, None));
    all.extend(walls::window_commands(&plan, &wp, &ids, None));
    all.extend(structural::to_commands(&sp, &so, None));
    let mut doc = Document::default();
    doc.commit("house", Origin::Generator, all).unwrap();
    doc
}

fn quantities(doc: &Document) -> Qto {
    qto::compute(&doc.model, &QtoOpts::default())
}

#[test]
fn durations_follow_quantities_and_rates() {
    let doc = house();
    let q = quantities(&doc);
    let s = schedule::build(&q, &lib_for(3.0, 40.0), &calendar("2026-11-15"), 1).unwrap();

    let task = |name: &str| s.tasks.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no task {name}"));
    let slab_vol = q.get("SLAB.VOL").unwrap().quantity + q.get("BEAM.VOL").unwrap().quantity;
    assert_eq!(task("Slab and beam concrete pour").duration, (slab_vol / 40.0).ceil() as i64);
    assert_eq!(task("Slab curing").duration, 3, "fixed duration comes straight from the rate");
    let blocks = q.get("WALL.EXT.AREA").unwrap().quantity + q.get("WALL.INT.AREA").unwrap().quantity;
    assert_eq!(task("Blockwork").duration, (blocks / 40.0).ceil() as i64);

    // Two crews halve the time (rounded up).
    let mut lib = lib_for(3.0, 40.0);
    lib.rates.get_mut("blockwork").unwrap().crews = 2;
    let s2 = schedule::build(&q, &lib, &calendar("2026-11-15"), 1).unwrap();
    let d2 = s2.tasks.iter().find(|t| t.name == "Blockwork").unwrap().duration;
    assert_eq!(d2, (blocks / 80.0).ceil() as i64);
}

#[test]
fn the_schedule_obeys_its_own_logic() {
    let doc = house();
    let cal = calendar("2026-11-15");
    let s = schedule::build(&quantities(&doc), &lib_for(3.0, 40.0), &cal, 1).unwrap();

    for t in &s.tasks {
        assert!(t.float >= 0, "{} has negative float", t.name);
        for &(p, lag) in &t.preds {
            assert!(t.es >= s.tasks[p].ef + lag, "{} starts before {} ends", t.name, s.tasks[p].name);
            assert!(t.start >= s.tasks[p].finish || t.duration == 0 || s.tasks[p].duration == 0, "dates out of order");
        }
        // Every date falls on a working day of the calendar.
        for d in [t.start, t.finish] {
            assert!(SUN_THU[d.weekday()], "{} is scheduled on a rest day {}", t.name, d.iso());
        }
    }

    // The critical path is continuous from day 0 to the project finish.
    assert!(s.tasks.first().unwrap().critical && s.tasks.last().unwrap().critical, "the first and last tasks are always on the critical path");
    let mut cur = *s.critical.iter().find(|&&i| s.tasks[i].ef == s.project_days).expect("a critical task ends the project");
    let mut sum = 0;
    loop {
        sum += s.tasks[cur].duration;
        let prev = s.tasks[cur].preds.iter().map(|&(p, _)| p).find(|&p| s.tasks[p].critical && s.tasks[p].ef == s.tasks[cur].es);
        match prev {
            Some(p) => cur = p,
            None => break,
        }
    }
    assert_eq!(s.tasks[cur].es, 0, "the chain reaches the start");
    assert_eq!(sum, s.project_days, "critical durations add up to the project length");
    assert_eq!(s.finish, cal.nth_workday(s.project_days - 1));
}

#[test]
fn every_quantity_task_cites_real_elements() {
    let doc = house();
    let s = schedule::build(&quantities(&doc), &lib_for(3.0, 40.0), &calendar("2026-11-15"), 1).unwrap();
    let with_qty: Vec<_> = s.tasks.iter().filter(|t| t.quantity.is_some()).collect();
    assert!(with_qty.len() >= 15);
    for t in with_qty {
        assert!(!t.elements.is_empty(), "{} has no elements", t.name);
        for id in &t.elements {
            assert!(doc.model.get(id).is_some(), "{} cites a missing element", t.name);
        }
    }
}

#[test]
fn pour_zones_split_the_frame_and_follow_each_other() {
    let doc = house();
    let q = quantities(&doc);
    let lib = lib_for(3.0, 40.0);
    let cal = calendar("2026-11-15");
    let one = schedule::build(&q, &lib, &cal, 1).unwrap();
    let three = schedule::build(&q, &lib, &cal, 3).unwrap();

    assert_eq!(three.tasks.len(), one.tasks.len() + 6 * 2, "six zoned activities gain two tasks each");
    let zones: Vec<_> = three.tasks.iter().filter(|t| t.name.starts_with("Slab and beam concrete pour")).collect();
    assert_eq!(zones.len(), 3);
    let total: f64 = zones.iter().map(|t| t.quantity.unwrap()).sum();
    let whole = one.tasks.iter().find(|t| t.name == "Slab and beam concrete pour").unwrap().quantity.unwrap();
    assert!((total - whole).abs() < 1e-2, "zones sum to the whole quantity: {total} vs {whole}");
    for w in zones.windows(2) {
        assert!(w[1].es >= w[0].ef, "the crew finishes one zone before starting the next");
    }
    assert!(zones.iter().all(|t| t.zone.is_some()));
}

#[test]
fn quantity_changes_reach_the_schedule() {
    let mut doc = house();
    let lib = lib_for(3.0, 20.0);
    let cal = calendar("2026-11-15");
    let before = schedule::build(&quantities(&doc), &lib, &cal, 1).unwrap();

    // Thicken every slab by 100 mm: more concrete, so a longer pour.
    let cmds: Vec<Command> = doc
        .model
        .elements
        .values()
        .filter(|e| e.kind == "Slab")
        .map(|e| {
            let t = e.props["thickness"].as_i64().unwrap();
            Command::SetProp { id: e.id, key: "thickness".into(), value: Some(serde_json::json!(t + 100)) }
        })
        .collect();
    doc.commit("thicker slabs", Origin::User, cmds).unwrap();
    let after = schedule::build(&quantities(&doc), &lib, &cal, 1).unwrap();
    let pour = |s: &schedule::Schedule| s.tasks.iter().find(|t| t.name == "Slab and beam concrete pour").unwrap().clone();
    assert!(pour(&after).quantity > pour(&before).quantity);
    assert!(pour(&after).duration >= pour(&before).duration);
    assert!(after.project_days >= before.project_days);
}

#[test]
fn csv_and_gantt_exports_cover_every_task() {
    let doc = house();
    let cal = calendar("2026-11-15");
    let s = schedule::build(&quantities(&doc), &lib_for(3.0, 40.0), &cal, 2).unwrap();

    let csv = s.to_csv();
    assert!(csv.starts_with('\u{feff}'));
    assert_eq!(csv.lines().count(), s.tasks.len() + 1);

    let svg = s.gantt_svg(&cal, "Villa - construction schedule (synthetic rates)");
    let x = roxmltree::Document::parse(&svg).expect("valid XML");
    let bars = x.descendants().filter(|n| n.has_attribute("data-task")).count();
    assert_eq!(bars, s.tasks.len(), "one bar or milestone per task");
    let red = x.descendants().filter(|n| n.has_attribute("data-task") && n.attribute("class").is_some_and(|c| c.contains("critical"))).count();
    assert_eq!(red, s.critical.len());

    if let Ok(dir) = std::env::var("NNX_SCHED_OUT") {
        std::fs::write(format!("{dir}/villa-schedule.svg"), &svg).unwrap();
        std::fs::write(format!("{dir}/villa-schedule.csv"), &csv).unwrap();
    }
}

// ------------------------------------------------------------------ zones tied to real elements

fn frame_x(doc: &Document) -> std::collections::BTreeMap<Id, i64> {
    use nnx_core::typed::{Beam, Column, Footing, Slab, Typed};
    let mut m = std::collections::BTreeMap::new();
    for e in doc.model.elements.values() {
        let x = match e.kind.as_str() {
            Column::KIND => e.typed::<Column>().ok().map(|c| c.x),
            Slab::KIND => e.typed::<Slab>().ok().map(|s| s.x + s.w / 2),
            Beam::KIND => e.typed::<Beam>().ok().map(|b| (b.x1 + b.x2) / 2),
            Footing::KIND => e.typed::<Footing>().ok().map(|f| f.x),
            _ => None,
        };
        if let Some(x) = x {
            m.insert(e.id, x);
        }
    }
    m
}

#[test]
fn quantity_parts_add_up_to_their_items() {
    let doc = house();
    let q = quantities(&doc);
    let with_parts: Vec<_> = q.items.iter().filter(|i| !i.parts.is_empty()).collect();
    assert!(with_parts.len() >= 7, "the frame items (column, beam, slab) carry parts");
    for i in with_parts {
        let sum: f64 = i.parts.iter().map(|p| p.1).sum();
        assert!((sum - i.quantity).abs() < 1e-3, "{}: parts {sum} vs {}", i.code, i.quantity);
        let ids: Vec<Id> = i.parts.iter().map(|p| p.0).collect();
        assert_eq!(ids, i.sources, "{}: parts and sources are the same elements", i.code);
    }
}

#[test]
fn zones_tied_to_the_model_take_disjoint_elements_from_their_own_band() {
    let doc = house();
    let q = quantities(&doc);
    let lib = lib_for(3.0, 40.0);
    let cal = calendar("2026-11-15");
    let zones = 3usize;
    let whole = schedule::build(&q, &lib, &cal, 1).unwrap();
    let s = schedule::build_with_model(&doc.model, &q, &lib, &cal, zones).unwrap();
    assert!(!s.warnings.iter().any(|w| w.contains("equal split")));
    assert!(schedule::build(&q, &lib, &cal, zones).unwrap().warnings.iter().any(|w| w.contains("equal split")), "the model-free build says it is an equal split");

    let xs = frame_x(&doc);
    let (lo, hi) = (*xs.values().min().unwrap(), *xs.values().max().unwrap());
    let band = |x: i64| (((x - lo) as i128 * zones as i128) / (hi - lo + 1) as i128) as usize;

    let mut unequal = false;
    for base in ["Column concrete pour", "Slab and beam concrete pour", "Column formwork"] {
        let tasks: Vec<_> = s.tasks.iter().filter(|t| t.name.starts_with(base)).collect();
        assert_eq!(tasks.len(), zones);
        let mut seen = std::collections::BTreeSet::new();
        for (z, t) in tasks.iter().enumerate() {
            assert_eq!(t.zone, Some(z + 1));
            for id in &t.elements {
                assert!(seen.insert(*id), "{base}: an element is in two zones");
                assert_eq!(band(xs[id]), z, "{base}: element in zone {} stands in band {}", z + 1, band(xs[id]));
            }
        }
        // Together the zones hold exactly the elements and the quantity of the unzoned task.
        let all = whole.tasks.iter().find(|t| t.name == base).unwrap();
        let mut union: Vec<Id> = seen.into_iter().collect();
        union.sort();
        assert_eq!(union, all.elements, "{base}: zones cover the same elements as the whole task");
        let total: f64 = tasks.iter().map(|t| t.quantity.unwrap()).sum();
        assert!((total - all.quantity.unwrap()).abs() < 1e-2, "{base}: {total} vs {}", all.quantity.unwrap());
        let first = tasks[0].quantity.unwrap();
        unequal |= tasks.iter().any(|t| (t.quantity.unwrap() - first).abs() > 1e-3);
    }
    assert!(unequal, "zone quantities come from the elements, so they are not all equal shares");
}
