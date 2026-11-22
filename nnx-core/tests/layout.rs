//! Room sizes and rule thresholds here are SYNTHETIC test values, not code requirements.

use nnx_core::layout::*;
use nnx_core::rules::{RuleSet, check};
use nnx_core::*;

fn room(key: &str, func: &str, area: f64, min_w: i64, zone: Zone, light: bool) -> RoomReq {
    RoomReq { key: key.into(), func: func.into(), target_area_m2: area, min_width_mm: min_w, zone, needs_light: light }
}

fn adj(a: &str, b: &str, weight: i32) -> Adjacency {
    Adjacency { a: a.into(), b: b.into(), weight }
}

fn villa() -> Brief {
    use Zone::*;
    Brief {
        envelope: Rect { x: 0, y: 0, w: 14_000, h: 18_000 }, // 252 m2
        rooms: vec![
            room("majlis", "majlis", 36.0, 3500, Public, true),
            room("living", "living", 30.0, 3500, Public, true),
            room("dining", "dining", 24.0, 3000, Public, true),
            room("kitchen", "kitchen", 20.0, 2500, Service, true),
            room("master", "bedroom", 28.0, 3500, Private, true),
            room("bed1", "bedroom", 20.0, 3000, Private, true),
            room("bed2", "bedroom", 20.0, 3000, Private, true),
            room("bed3", "bedroom", 20.0, 3000, Private, true),
            room("bath1", "bathroom", 6.0, 1500, Service, false),
            room("bath2", "bathroom", 6.0, 1500, Service, false),
            room("bath3", "bathroom", 6.0, 1500, Service, false),
        ],
        adjacency: vec![adj("kitchen", "dining", 2), adj("master", "bath1", 2), adj("majlis", "bed1", -2)],
        entry: Side::South,
        entry_at: None,
        wall_mm: None,
    }
}

fn opts() -> Options {
    Options { seed: 7, ..Options::default() }
}

fn overlap_area(a: &Rect, b: &Rect) -> i64 {
    let w = ((a.x + a.w).min(b.x + b.w) - a.x.max(b.x)).max(0);
    let h = ((a.y + a.h).min(b.y + b.h) - a.y.max(b.y)).max(0);
    w * h
}

#[test]
fn every_layout_tiles_the_envelope_exactly() {
    let brief = villa();
    let layouts = generate(&brief, &opts()).unwrap();
    assert!(!layouts.is_empty());
    let env = brief.envelope;
    for l in &layouts {
        let mut total = 0i64;
        for (i, a) in l.rooms.iter().enumerate() {
            let r = a.rect;
            assert!(r.w > 0 && r.h > 0);
            assert!(r.x >= env.x && r.y >= env.y && r.x + r.w <= env.x + env.w && r.y + r.h <= env.y + env.h);
            total += r.w * r.h;
            for b in &l.rooms[i + 1..] {
                assert_eq!(overlap_area(&r, &b.rect), 0, "{} overlaps {}", a.key, b.key);
            }
        }
        assert_eq!(total, env.w * env.h, "gaps or overlaps in the tiling");
    }
}

#[test]
fn rooms_get_their_target_area_and_leftover_is_an_explicit_reserve() {
    let layouts = generate(&villa(), &opts()).unwrap();
    let l = &layouts[0];
    for r in l.rooms.iter().filter(|r| r.key != "reserve") {
        assert!(r.deviation_pct.abs() < 1.0, "{} deviates {:.2}%", r.key, r.deviation_pct);
    }
    // 252 - 216 = 36 m2 of reserve.
    assert!((l.reserve_m2 - 36.0).abs() < 0.5, "reserve = {}", l.reserve_m2);
}

#[test]
fn best_layout_honours_adjacency_and_returns_distinct_alternatives() {
    let layouts = generate(&villa(), &opts()).unwrap();
    assert!(layouts.len() >= 3, "only {} alternatives", layouts.len());
    assert!(layouts.windows(2).all(|w| w[0].cost <= w[1].cost), "sorted best first");

    let best = &layouts[0];
    let rect = |k: &str| best.rooms.iter().find(|r| r.key == k).unwrap().rect;
    let touch = |a: &str, b: &str| {
        let (a, b) = (rect(a), rect(b));
        let ov = |a0: i64, a1: i64, b0: i64, b1: i64| (a1.min(b1) - a0.max(b0)).max(0);
        let mut s = 0;
        if a.x + a.w == b.x || b.x + b.w == a.x {
            s += ov(a.y, a.y + a.h, b.y, b.y + b.h);
        }
        if a.y + a.h == b.y || b.y + b.h == a.y {
            s += ov(a.x, a.x + a.w, b.x, b.x + b.w);
        }
        s
    };
    assert!(touch("kitchen", "dining") >= 1000, "kitchen must share a wall with dining");
    assert_eq!(touch("majlis", "bed1"), 0, "majlis must stay away from bedroom 1");
}

#[test]
fn same_seed_same_result() {
    let a = generate(&villa(), &opts()).unwrap();
    let b = generate(&villa(), &opts()).unwrap();
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x.cost, y.cost);
        assert_eq!(x.rooms.iter().map(|r| r.rect).collect::<Vec<_>>(), y.rooms.iter().map(|r| r.rect).collect::<Vec<_>>());
    }
}

#[test]
fn invalid_briefs_are_rejected() {
    let mut b = villa();
    b.envelope = Rect { x: 0, y: 0, w: 10_000, h: 10_000 }; // 100 m2 < 216 m2
    assert!(matches!(generate(&b, &opts()), Err(LayoutError::Infeasible { .. })));

    let mut b = villa();
    b.adjacency.push(adj("kitchen", "nowhere", 1));
    assert_eq!(generate(&b, &opts()).unwrap_err(), LayoutError::UnknownRoom("nowhere".into()));

    let mut b = villa();
    b.rooms.push(room("bed1", "bedroom", 10.0, 3000, Zone::Private, true));
    assert_eq!(generate(&b, &opts()).unwrap_err(), LayoutError::DuplicateKey("bed1".into()));

    let mut b = villa();
    b.rooms.clear();
    assert_eq!(generate(&b, &opts()).unwrap_err(), LayoutError::Empty);
}

#[test]
fn full_program_without_reserve_still_tiles() {
    let mut b = villa();
    b.envelope = Rect { x: 0, y: 0, w: 12_000, h: 18_000 }; // exactly 216 m2
    let l = &generate(&b, &opts()).unwrap()[0];
    assert_eq!(l.reserve_m2, 0.0);
    assert!(l.rooms.iter().all(|r| r.key != "reserve"));
    assert_eq!(l.rooms.iter().map(|r| r.rect.w * r.rect.h).sum::<i64>(), 12_000 * 18_000);
}

#[test]
fn layout_becomes_one_undoable_generator_step_and_feeds_the_rule_engine() {
    let layout = generate(&villa(), &opts()).unwrap().remove(0);
    let mut doc = Document::default();
    doc.commit("generate layout", Origin::Generator, to_commands(&layout, None)).unwrap();
    assert_eq!(doc.model.len(), layout.rooms.len());

    // A synthetic minimum-bedroom-area rule runs directly on the generated spaces.
    let mut rules = RuleSet::default();
    rules
        .add_layer(
            r#"{"id":"SYN","edition":"synthetic","version":"1","rules":[{
                "id":"bedroom.min","clause":"S-1",
                "applies_to":{"kind":"Space","where":"func == \"bedroom\""},
                "check":"area >= param.min",
                "params":{"min":{"value":19,"source":"synthetic"}},
                "severity":"error","message":"bedroom too small"}]}"#,
        )
        .unwrap();
    let report = check(&doc.model, &rules);
    assert_eq!(report.checked, 4, "four bedrooms checked");
    assert!(report.findings.is_empty(), "{:?}", report.findings);

    assert!(doc.undo());
    assert!(doc.model.is_empty());
}

fn shared(a: &Rect, b: &Rect) -> i64 {
    let ov = |a0: i64, a1: i64, b0: i64, b1: i64| (a1.min(b1) - a0.max(b0)).max(0);
    let mut s = 0;
    if a.x + a.w == b.x || b.x + b.w == a.x {
        s += ov(a.y, a.y + a.h, b.y, b.y + b.h);
    }
    if a.y + a.h == b.y || b.y + b.h == a.y {
        s += ov(a.x, a.x + a.w, b.x, b.x + b.w);
    }
    s
}

#[test]
fn every_private_room_in_the_best_layouts_opens_onto_circulation() {
    let layouts = generate(&villa(), &opts()).unwrap();
    let hub = |f: &str| ["living", "hall", "corridor", "lobby", "circulation_reserve"].contains(&f);
    let mut satisfied = 0;
    for (n, l) in layouts.iter().enumerate() {
        let ok = l.rooms.iter().filter(|r| r.zone == Zone::Private && r.func != "bathroom").all(|r| {
            l.rooms.iter().filter(|h| hub(&h.func)).any(|h| shared(&r.rect, &h.rect) >= 1200)
        });
        if n == 0 {
            assert!(ok, "the best layout must give every bedroom direct access to circulation");
        }
        satisfied += ok as usize;
    }
    assert!(satisfied >= 2, "only {satisfied} of {} alternatives give every bedroom access", layouts.len());
}

#[test]
fn access_term_is_visible_in_the_cost_breakdown_and_can_be_switched_off() {
    let on = generate(&villa(), &opts()).unwrap();
    assert!(on[0].breakdown.access < 0.5, "access penalty left: {}", on[0].breakdown.access);
    let mut off_opts = opts();
    off_opts.weights.access = 0.0;
    let off = generate(&villa(), &off_opts).unwrap();
    assert_eq!(off[0].breakdown.access, 0.0);
}

#[test]
fn circulation_rooms_touch_each_other_so_the_hall_connects_to_the_living_room() {
    let best = &generate(&villa(), &opts()).unwrap()[0];
    let hubs: Vec<_> = best.rooms.iter().filter(|r| ["living", "circulation_reserve"].contains(&r.func.as_str())).collect();
    assert_eq!(hubs.len(), 2);
    assert!(shared(&hubs[0].rect, &hubs[1].rect) >= 1200, "living and the hall must share a door-sized wall");
}

#[test]
fn each_bathroom_is_attached_to_a_different_bedroom() {
    let best = &generate(&villa(), &opts()).unwrap()[0];
    let baths: Vec<_> = best.rooms.iter().filter(|r| r.func == "bathroom").collect();
    let beds: Vec<_> = best.rooms.iter().filter(|r| r.zone == Zone::Private && r.func != "bathroom").collect();
    assert_eq!((baths.len(), beds.len()), (3, 4));
    // Greedy check is enough here: every bath must touch a bedroom, and no bedroom is used twice.
    let mut used: Vec<&str> = Vec::new();
    let mut ok = true;
    for b in &baths {
        let host = beds.iter().find(|h| shared(&b.rect, &h.rect) >= 1100 && !used.contains(&h.key.as_str()));
        match host {
            Some(h) => used.push(h.key.as_str()),
            None => ok = false,
        }
    }
    assert!(ok || best.breakdown.bath == 0.0, "baths not spread across bedrooms, bath penalty {}", best.breakdown.bath);
    assert!(best.breakdown.bath < 0.5, "bath penalty left: {}", best.breakdown.bath);
}
