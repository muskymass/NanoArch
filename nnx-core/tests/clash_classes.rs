//! Conflict classes of the clash check, on a hand-built scene (metres) so every overlap can be checked by hand.

use nnx_core::clash::{self, ClashOpts};
use nnx_core::model3d::{Box3, Scene};
use std::collections::BTreeMap;

fn bx(id: &str, kind: &'static str, class: &str, min: [f64; 3], max: [f64; 3]) -> Box3 {
    Box3 { id: id.into(), kind, class: class.into(), min, max, visual: false, level: Some(0), discipline: "structural" }
}

fn scene(boxes: Vec<Box3>) -> Scene {
    Scene { levels: vec![], boxes, min: [0.0; 3], max: [10.0; 3], storey_height: 3.0, assemblies: BTreeMap::new() }
}

#[test]
fn each_class_is_found_and_counted_apart() {
    let s = scene(vec![
        // beam 4 m long, section y 0..0.3, z 2.5..3.0
        bx("beam", "Beam", "", [0.0, 0.0, 2.5], [4.0, 0.3, 3.0]),
        // duct inside the beam: hard clash. Overlap x 1..2, y 0..0.2, z 2.5..2.7 = 1 x 0.2 x 0.2 = 0.04 m3
        bx("duct", "MepRun", "duct", [1.0, 0.0, 2.5], [2.0, 0.2, 2.7]),
        // cable tray 0.1 m beside the beam (y 0.4..0.6), same height: a clearance case for 0.2 m, not for 0.05 m
        bx("tray", "MepRun", "cable", [3.0, 0.4, 2.6], [3.5, 0.6, 2.8]),
        // pipe crossing the duct: routing conflict. Overlap x 1.5..2.0, y 0..0.2, z 2.5..2.7 = 0.5 x 0.2 x 0.2 = 0.02 m3
        bx("pipe", "MepRun", "pipe", [1.5, 0.0, 2.5], [2.5, 0.2, 2.7]),
        // slab crossed by a stack: penetration
        bx("slab", "Slab", "", [0.0, 5.0, 3.0], [10.0, 9.0, 3.2]),
        bx("stack", "MepRun", "drain", [6.0, 6.0, 2.0], [6.1, 6.1, 3.5]),
    ]);
    let r = clash::classify(&s, &ClashOpts::default(), Some(0.2));
    assert_eq!(r.hard.len(), 2, "duct and pipe both lie inside the beam: {:?}", r.hard.iter().map(|c| &c.run_id).collect::<Vec<_>>());
    let duct = r.hard.iter().find(|c| c.run_id == "duct").unwrap();
    assert!((duct.volume_m3 - 0.04).abs() < 1e-12, "{}", duct.volume_m3);
    assert_eq!(r.routing.len(), 1);
    assert!((r.routing[0].volume_m3 - 0.02).abs() < 1e-12, "{}", r.routing[0].volume_m3);
    assert_eq!(r.clearance.len(), 1, "only the tray, which is 0.1 m from the beam");
    assert_eq!(r.clearance[0].a_id, "tray");
    assert_eq!(r.penetrations.len(), 1);
    assert_eq!((r.penetrations[0].a_id.as_str(), r.penetrations[0].b_kind), ("stack", "Slab"));
    assert!(r.not_assessed.iter().any(|n| n.starts_with("maintenance access")));
    assert_eq!(r.hard, clash::detect(&s, &ClashOpts::default()).clashes, "the hard class is exactly the old check");
}

#[test]
fn without_an_entered_gap_clearance_is_not_assessed_and_nothing_is_assumed() {
    let s = scene(vec![bx("beam", "Beam", "", [0.0, 0.0, 2.5], [4.0, 0.3, 3.0]), bx("tray", "MepRun", "cable", [3.0, 0.4, 2.6], [3.5, 0.6, 2.8])]);
    let none = clash::classify(&s, &ClashOpts::default(), None);
    assert!(none.clearance.is_empty() && none.not_assessed.iter().any(|n| n.starts_with("clearance")));
    let small = clash::classify(&s, &ClashOpts::default(), Some(0.05));
    assert!(small.clearance.is_empty(), "0.1 m away is outside a 0.05 m gap");
    let zero = clash::classify(&s, &ClashOpts::default(), Some(0.0));
    assert!(zero.clearance.is_empty() && zero.not_assessed.iter().any(|n| n.starts_with("clearance")), "a zero gap is no gap");
}

#[test]
fn runs_of_the_same_system_that_meet_at_a_tee_are_not_a_routing_conflict() {
    let s = scene(vec![bx("trunk", "MepRun", "duct", [0.0, 0.0, 2.5], [4.0, 0.3, 2.8]), bx("branch", "MepRun", "duct", [1.0, 0.0, 2.5], [1.3, 2.0, 2.8])]);
    assert!(clash::classify(&s, &ClashOpts::default(), None).routing.is_empty());
}
