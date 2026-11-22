//! Dimensions and thresholds here are SYNTHETIC test values, not code requirements.

use nnx_core::layout::*;
use nnx_core::rules::{RuleSet, Severity, check};
use nnx_core::walls::{self, Plan, WallOpts};
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
        envelope: Rect { x: 0, y: 0, w: 14_000, h: 18_000 },
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
        wall_height_mm: None,
    }
}

fn opts() -> Options {
    Options { seed: 7, ..Options::default() }
}

fn setup() -> (Brief, Layout, Plan) {
    let brief = villa();
    let layout = generate(&brief, &opts()).unwrap().remove(0);
    let plan = walls::build(&layout, brief.envelope, brief.entry, &walls::opts_from_adjacency(&brief.adjacency));
    (brief, layout, plan)
}

#[test]
fn walls_cover_every_room_edge_exactly_once_per_side() {
    let (brief, layout, plan) = setup();
    let perimeter: i64 = layout.rooms.iter().map(|r| 2 * (r.rect.w + r.rect.h)).sum();
    let ext: i64 = plan.walls.iter().filter(|w| w.exterior).map(|w| w.length()).sum();
    let int: i64 = plan.walls.iter().filter(|w| !w.exterior).map(|w| w.length()).sum();
    let env = brief.envelope;
    assert_eq!(ext, 2 * (env.w + env.h), "exterior walls run the whole envelope");
    assert_eq!(ext + 2 * int, perimeter, "each partition is the edge of exactly two rooms");
}

#[test]
fn exterior_walls_stay_inside_the_envelope() {
    let (brief, _, plan) = setup();
    let e = brief.envelope;
    for w in plan.walls.iter().filter(|w| w.exterior) {
        let t = w.thickness;
        let (lo_x, hi_x) = (w.a.0.min(w.b.0), w.a.0.max(w.b.0));
        let (lo_y, hi_y) = (w.a.1.min(w.b.1), w.a.1.max(w.b.1));
        let (fx0, fx1, fy0, fy1) = if w.horizontal {
            (lo_x, hi_x, lo_y - t / 2, hi_y + t / 2)
        } else {
            (lo_x - t / 2, hi_x + t / 2, lo_y, hi_y)
        };
        assert!(fx0 >= e.x && fy0 >= e.y && fx1 <= e.x + e.w && fy1 <= e.y + e.h, "wall pokes outside: {w:?}");
    }
}

#[test]
fn door_graph_is_sound() {
    let (brief, layout, plan) = setup();
    // Structural warnings (unreachable room, wet room without a door) must not occur.
    // The privacy warning is allowed here: it flags a layout weakness for the user to see.
    assert!(
        !plan.warnings.iter().any(|w| w.contains("cannot be reached") || w.contains("no neighbour") || w.contains("no wall")),
        "{:?}",
        plan.warnings
    );

    // Exactly one entry door, on the street (south) side.
    let entries: Vec<_> = plan.doors.iter().filter(|d| d.entry).collect();
    assert_eq!(entries.len(), 1);
    let ew = &plan.walls[entries[0].wall];
    assert!(ew.exterior && ew.horizontal && ew.edge_line == brief.envelope.y);

    // Every wet room has exactly one door; none is a transit room.
    for (i, r) in layout.rooms.iter().enumerate() {
        if r.func == "bathroom" {
            let n = plan.doors.iter().filter(|d| d.connects.0 == i || d.connects.1 == Some(i)).count();
            assert_eq!(n, 1, "{} has {n} doors", r.key);
        }
    }

    // The majlis/bed1 pair is forbidden to share a door.
    let key = |i: usize| layout.rooms[i].key.as_str();
    assert!(!plan.doors.iter().any(|d| {
        let pair = (key(d.connects.0), d.connects.1.map(key));
        pair == ("majlis", Some("bed1")) || pair == ("bed1", Some("majlis"))
    }));

    // The wanted kitchen-dining door exists when the two rooms share enough wall.
    let ki = layout.rooms.iter().position(|r| r.key == "kitchen").unwrap();
    let di = layout.rooms.iter().position(|r| r.key == "dining").unwrap();
    let has = plan.doors.iter().any(|d| {
        (d.connects.0 == ki && d.connects.1 == Some(di)) || (d.connects.0 == di && d.connects.1 == Some(ki))
    });
    assert!(has, "kitchen-dining door missing");
}

#[test]
fn every_door_fits_its_wall_with_margins() {
    let (_, layout, plan) = setup();
    let margin = WallOpts::default().door_margin_mm;
    assert!(plan.doors.len() >= layout.rooms.len(), "{} doors for {} rooms: a spanning tree plus the entry needs at least one per room", plan.doors.len(), layout.rooms.len());
    for d in &plan.doors {
        let len = plan.walls[d.wall].length();
        assert!(d.offset - d.width / 2 >= margin, "left jamb too close: {d:?}");
        assert!(d.offset + d.width / 2 <= len - margin, "right jamb too close: {d:?}");
    }
}

#[test]
fn net_area_is_smaller_than_gross_by_a_plausible_wall_allowance() {
    let (_, layout, plan) = setup();
    for (i, r) in layout.rooms.iter().enumerate() {
        let net = plan.net_area_m2[i];
        assert!(net < r.area_m2, "{}: net {net} >= gross {}", r.key, r.area_m2);
        assert!(net > r.area_m2 * 0.6, "{}: net {net} implausibly small vs {}", r.key, r.area_m2);
    }
}

#[test]
fn unreachable_and_oversize_door_cases_are_reported() {
    let (brief, layout, _) = setup();
    // Doors wider than any wall: nothing can connect, so warnings must appear.
    let opts = WallOpts { interior_door_mm: 50_000, wet_door_mm: 50_000, entry_door_mm: 50_000, ..WallOpts::default() };
    let plan = walls::build(&layout, brief.envelope, brief.entry, &opts);
    assert!(plan.doors.is_empty());
    assert!(plan.warnings.iter().any(|w| w.contains("entry")));
}

#[test]
fn whole_building_is_one_undoable_step_and_walls_with_doors_are_protected() {
    let (_, layout, plan) = setup();
    let space_cmds = to_commands(&layout, None);
    let ids = walls::space_ids(&space_cmds);
    assert_eq!(ids.len(), layout.rooms.len());
    let mut all = space_cmds;
    all.extend(walls::to_commands(&plan, &ids, None));

    let mut doc = Document::default();
    doc.commit("generate house", Origin::Generator, all).unwrap();
    let kinds = |k: &str| doc.model.elements.values().filter(|e| e.kind == k).count();
    assert_eq!(kinds("Wall"), plan.walls.len());
    assert_eq!(kinds("Door"), plan.doors.len());
    assert_eq!(kinds("Space"), layout.rooms.len());

    // netArea landed on the spaces.
    let bed = doc.model.get(&ids["bed1"]).unwrap();
    assert!(bed.props["netArea"].as_f64().unwrap() < bed.props["area"].as_f64().unwrap());

    // A wall that hosts a door cannot be deleted.
    let hosted_wall = doc.model.elements.values().find(|e| e.kind == "Door").unwrap().host_id.unwrap();
    assert!(doc.commit("x", Origin::User, vec![Command::Delete(hosted_wall)]).is_err());

    assert!(doc.undo());
    assert!(doc.model.is_empty());
}

#[test]
fn rule_engine_flags_the_entry_door_and_the_fix_works() {
    let (_, layout, plan) = setup();
    let space_cmds = to_commands(&layout, None);
    let ids = walls::space_ids(&space_cmds);
    let mut all = space_cmds;
    all.extend(walls::to_commands(&plan, &ids, None));
    let mut doc = Document::default();
    doc.commit("generate house", Origin::Generator, all).unwrap();

    let mut rules = RuleSet::default();
    rules
        .add_layer(
            r#"{"id":"SYN","edition":"synthetic","version":"1","rules":[{
                "id":"egress.width","clause":"S-9",
                "applies_to":{"kind":"Door","where":"isEgress == true"},
                "check":"width >= param.min",
                "params":{"min":{"value":1100,"source":"synthetic"}},
                "severity":"error","message":"egress door too narrow",
                "fix":{"prop":"width","value":"param.min"}}]}"#,
        )
        .unwrap();
    let report = check(&doc.model, &rules);
    assert_eq!(report.checked, 1);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].severity, Severity::Error);
    doc.commit("fix", Origin::User, vec![report.findings[0].fix.clone().unwrap()]).unwrap();
    assert!(check(&doc.model, &rules).findings.is_empty());
}

// ---------------------------------------------------------------------------
// Windows and 2D drawing
// ---------------------------------------------------------------------------

use nnx_core::drawing::{DrawError, DrawOpts, plan_svg};
use nnx_core::walls::{WindowOpts, place_windows, window_commands};

fn full_model() -> (Layout, Plan, walls::WindowPlan, Document, std::collections::BTreeMap<String, Id>) {
    let (_, layout, plan) = setup();
    let wopts = walls::opts_from_adjacency(&villa().adjacency);
    let wp = place_windows(&layout, &plan, &wopts, &WindowOpts::default());
    let space_cmds = to_commands(&layout, None);
    let ids = walls::space_ids(&space_cmds);
    let mut all = space_cmds;
    all.extend(walls::to_commands(&plan, &ids, None));
    all.extend(window_commands(&plan, &wp, &ids, None));
    let mut doc = Document::default();
    doc.commit("generate house", Origin::Generator, all).unwrap();
    (layout, plan, wp, doc, ids)
}

#[test]
fn windows_sit_on_exterior_walls_clear_of_doors_corners_and_each_other() {
    let (layout, plan, wp, _, _) = full_model();
    let o = WindowOpts::default();
    assert!(!wp.windows.is_empty());
    for w in &wp.windows {
        let wall = &plan.walls[w.wall];
        assert!(wall.exterior, "window on an interior wall");
        assert!(w.offset - w.width / 2 >= o.margin_mm && w.offset + w.width / 2 <= wall.length() - o.margin_mm);
        for d in plan.doors.iter().filter(|d| d.wall == w.wall) {
            let gap = (w.offset - d.offset).abs() - w.width / 2 - d.width / 2;
            assert!(gap >= o.margin_mm, "window {gap} mm from a door");
        }
        for v in wp.windows.iter().filter(|v| v.wall == w.wall && !std::ptr::eq(*v, w)) {
            let gap = (w.offset - v.offset).abs() - w.width / 2 - v.width / 2;
            assert!(gap >= o.margin_mm, "two windows {gap} mm apart");
        }
    }
    // Wet rooms get small high windows only.
    for w in wp.windows.iter().filter(|w| layout.rooms[w.room].func == "bathroom") {
        assert_eq!((w.width <= o.wet_width_mm, w.sill), (true, o.wet_sill_mm));
    }
}

#[test]
fn rooms_without_exterior_space_get_a_daylight_warning_not_silence() {
    let (layout, plan, wp, _, _) = full_model();
    for (i, r) in layout.rooms.iter().enumerate() {
        let has = wp.windows.iter().any(|w| w.room == i);
        let warned = wp.warnings.iter().any(|m| m.contains(&format!("'{}'", r.key)));
        if r.needs_light {
            assert!(has != warned, "{}: has window {has}, warned {warned}", r.key);
        }
    }
    // The ratio is area-consistent.
    for (i, r) in layout.rooms.iter().enumerate() {
        let area: f64 = wp.windows.iter().filter(|w| w.room == i).map(|w| (w.width * w.height) as f64 / 1e6).sum();
        assert!((wp.glazing_ratio[i] - area / r.area_m2).abs() < 1e-9);
    }
    let _ = plan;
}

#[test]
fn windows_join_the_same_undoable_transaction_and_hosts_are_protected() {
    let (_, _, wp, mut doc, ids) = full_model();
    let n = doc.model.elements.values().filter(|e| e.kind == "Window").count();
    assert_eq!(n, wp.windows.len());
    assert!(doc.model.get(&ids["majlis"]).unwrap().props.contains_key("glazingRatio"));
    assert!(doc.undo());
    assert!(doc.model.is_empty());
}

fn parse(svg: &str) -> roxmltree::Document<'_> {
    roxmltree::Document::parse(svg).expect("svg is well-formed XML")
}

fn count(doc: &roxmltree::Document, class: &str) -> usize {
    doc.descendants().filter(|n| n.attribute("class").is_some_and(|c| c.split(' ').any(|x| x == class))).count()
}

#[test]
fn drawing_is_valid_xml_with_one_symbol_per_model_element() {
    let (layout, plan, wp, doc, _) = full_model();
    let svg = plan_svg(&doc.model, None, &DrawOpts::default()).unwrap();
    let x = parse(&svg);
    assert_eq!(x.root_element().tag_name().name(), "svg");
    assert_eq!(count(&x, "wall"), plan.walls.len());
    assert_eq!(count(&x, "door"), plan.doors.len());
    assert_eq!(count(&x, "window"), wp.windows.len());
    assert_eq!(count(&x, "room"), layout.rooms.len());
    assert_eq!(count(&x, "name"), layout.rooms.len());

    // Every drawn symbol traces back to a real model element.
    for n in x.descendants().filter(|n| n.has_attribute("data-id")) {
        let id: Id = n.attribute("data-id").unwrap().parse().unwrap();
        assert!(doc.model.get(&id).is_some(), "data-id {id} not in model");
    }
    // A swing arc and leaf for each door.
    assert_eq!(count(&x, "swing"), plan.doors.len());
    assert_eq!(count(&x, "leaf"), plan.doors.len());
}

#[test]
fn drawing_follows_the_model_and_is_deterministic() {
    let (_, _, _, mut doc, ids) = full_model();
    let a = plan_svg(&doc.model, None, &DrawOpts::default()).unwrap();
    let b = plan_svg(&doc.model, None, &DrawOpts::default()).unwrap();
    assert_eq!(a, b);

    // Editing the model changes the drawing; nothing else holds the plan.
    doc.commit(
        "edit",
        Origin::User,
        vec![Command::SetProp { id: ids["bed1"], key: "netArea".into(), value: Some(serde_json::json!(15.3)) }],
    )
    .unwrap();
    let c = plan_svg(&doc.model, None, &DrawOpts::default()).unwrap();
    assert!(c.contains("15.3 m²") && !a.contains("15.3 m²"));

    // Display names (for example Arabic) replace keys and are XML-escaped.
    let mut opts = DrawOpts::default();
    opts.labels.insert("majlis".into(), "المجلس & الضيافة".into());
    let d = plan_svg(&doc.model, None, &opts).unwrap();
    assert!(d.contains("المجلس &amp; الضيافة"));
    parse(&d);
}

#[test]
fn empty_model_cannot_be_drawn() {
    assert_eq!(plan_svg(&Model::default(), None, &DrawOpts::default()).unwrap_err(), DrawError::NoSpaces);
}

/// Set NNX_SVG_OUT to a file path to keep the drawing for a visual check.
#[test]
fn write_sample_drawing_when_requested() {
    let Ok(path) = std::env::var("NNX_SVG_OUT") else { return };
    let (_, _, _, doc, _) = full_model();
    let mut opts = DrawOpts { title: "Villa - ground floor plan (synthetic test program)".into(), ..DrawOpts::default() };
    for (k, v) in [("majlis", "Majlis"), ("living", "Living"), ("dining", "Dining"), ("kitchen", "Kitchen"), ("master", "Master bed"), ("bed1", "Bedroom 1"), ("bed2", "Bedroom 2"), ("bed3", "Bedroom 3"), ("bath1", "Bath 1"), ("bath2", "Bath 2"), ("bath3", "Bath 3"), ("reserve", "Hall / circulation")] {
        opts.labels.insert(k.into(), v.into());
    }
    std::fs::write(path, plan_svg(&doc.model, None, &opts).unwrap()).unwrap();
}

#[test]
fn entry_door_opens_into_a_preferred_room_not_the_dining_room() {
    let (_, layout, plan) = setup();
    let entry = plan.doors.iter().find(|d| d.entry).unwrap();
    let func = layout.rooms[entry.opens_into].func.as_str();
    assert!(["circulation_reserve", "hall", "living", "reception", "majlis"].contains(&func), "entry opens into {func}");
}

#[test]
fn privacy_warning_matches_the_door_graph() {
    let (_, layout, plan) = setup();
    for (i, r) in layout.rooms.iter().enumerate().filter(|(_, r)| r.zone == Zone::Private && r.func != "bathroom") {
        let nbrs: Vec<usize> = plan
            .doors
            .iter()
            .filter_map(|d| match d.connects {
                (a, Some(b)) if a == i => Some(b),
                (a, Some(b)) if b == i => Some(a),
                _ => None,
            })
            .filter(|&j| layout.rooms[j].func != "bathroom")
            .collect();
        let expect = !nbrs.is_empty() && nbrs.iter().all(|&j| layout.rooms[j].zone == Zone::Private);
        let warned = plan.warnings.iter().any(|w| w.contains(&format!("'{}' is reached only through", r.key)));
        assert_eq!(expect, warned, "{}", r.key);
    }
}

#[test]
fn no_private_room_is_reached_through_another_private_room() {
    let (_, _, plan) = setup();
    let bad: Vec<_> = plan.warnings.iter().filter(|w| w.contains("reached only through")).collect();
    assert!(bad.is_empty(), "{bad:?}");
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
}

#[test]
fn bedrooms_are_reached_through_circulation_rooms_only() {
    let (_, layout, plan) = setup();
    let hub = |f: &str| ["living", "hall", "corridor", "lobby", "circulation_reserve"].contains(&f);
    let n = layout.rooms.len();
    let entry = plan.doors.iter().find(|d| d.entry).unwrap().connects.0;
    assert!(hub(&layout.rooms[entry].func), "entry opens into a circulation room");

    // Walk from the entry through hub rooms only.
    let mut seen = vec![false; n];
    seen[entry] = true;
    let mut stack = vec![entry];
    while let Some(x) = stack.pop() {
        for d in &plan.doors {
            let (a, Some(b)) = d.connects else { continue };
            let next = if a == x { b } else if b == x { a } else { continue };
            if !seen[next] && hub(&layout.rooms[next].func) {
                seen[next] = true;
                stack.push(next);
            }
        }
    }
    for (i, r) in layout.rooms.iter().enumerate().filter(|(_, r)| r.zone == Zone::Private && r.func != "bathroom") {
        let door_to_spine = plan.doors.iter().any(|d| {
            let (a, Some(b)) = d.connects else { return false };
            (a == i && seen[b]) || (b == i && seen[a])
        });
        assert!(door_to_spine, "{} has no door onto the circulation spine", r.key);
    }
}

#[test]
fn bedroom_doors_lead_only_to_circulation_or_a_bathroom() {
    let (_, layout, plan) = setup();
    let hub = |f: &str| ["living", "hall", "corridor", "lobby", "circulation_reserve"].contains(&f);
    for d in &plan.doors {
        let (a, Some(b)) = d.connects else { continue };
        for (me, other) in [(a, b), (b, a)] {
            let (m, o) = (&layout.rooms[me], &layout.rooms[other]);
            if m.zone == Zone::Private && m.func != "bathroom" {
                assert!(hub(&o.func) || o.func == "bathroom", "{} has a door to {} ({})", m.key, o.key, o.func);
            }
        }
    }
}

#[test]
fn baths_are_spread_across_bedrooms_not_piled_into_one() {
    let (_, layout, plan) = setup();
    for (i, r) in layout.rooms.iter().enumerate().filter(|(_, r)| r.zone == Zone::Private && r.func != "bathroom") {
        let baths = plan
            .doors
            .iter()
            .filter(|d| {
                let (a, Some(b)) = d.connects else { return false };
                let other = if a == i { b } else if b == i { a } else { return false };
                layout.rooms[other].func == "bathroom"
            })
            .count();
        assert!(baths <= 2, "{} has {baths} baths opening into it", r.key);
    }
}

#[test]
fn every_bathroom_door_opens_into_its_own_bedroom() {
    let (_, layout, plan) = setup();
    let mut hosts: Vec<usize> = Vec::new();
    for (i, r) in layout.rooms.iter().enumerate().filter(|(_, r)| r.func == "bathroom") {
        let host = plan
            .doors
            .iter()
            .find_map(|d| match d.connects {
                (a, Some(b)) if a == i => Some(b),
                (a, Some(b)) if b == i => Some(a),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{} has no door", r.key));
        assert_eq!(layout.rooms[host].zone, Zone::Private, "{} opens into {}", r.key, layout.rooms[host].key);
        assert!(!hosts.contains(&host), "two baths open into {}", layout.rooms[host].key);
        hosts.push(host);
    }
}
