//! Solar geometry, sightlines, efficiency, stairs and ramps.

use nnx_core::analysis::*;

fn near(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol, "{a} vs {b}");
}

#[test]
fn the_sun_follows_known_geometry() {
    // Equinox noon at latitude 27: altitude 90 - 27 = 63 degrees, due south. (Declination is 0.4 degrees on day 80.)
    let s = sun(27.0, 80, 12.0).unwrap();
    near(s.altitude_deg, 63.0, 0.8);
    near(s.azimuth_deg, 180.0, 0.5);
    // Summer solstice noon: 90 - 27 + 23.45 = 86.45; winter: 90 - 27 - 23.45 = 39.55.
    near(sun(27.0, 172, 12.0).unwrap().altitude_deg, 86.45, 0.3);
    near(sun(27.0, 355, 12.0).unwrap().altitude_deg, 39.55, 0.3);
    // Morning sun is in the east half, afternoon sun in the west half; midnight is dark.
    assert!(sun(27.0, 80, 9.0).unwrap().azimuth_deg < 180.0 && sun(27.0, 80, 15.0).unwrap().azimuth_deg > 180.0);
    assert!(sun(27.0, 80, 0.0).is_none());
    near(declination_deg(80), 0.0, 0.6);
    near(declination_deg(172), 23.45, 0.1);
}

#[test]
fn the_solar_report_ranks_facades_and_sizes_the_overhang() {
    let r = solar(27.0, Some(1.5)).unwrap();
    assert_eq!(r.table.len(), 4);
    assert!(r.table[0].daylight_hours > 13.0 && r.table[1].daylight_hours < 11.0, "{:?}", r.table);
    let f = |n: &str| r.facades.iter().find(|f| f.facade == n).unwrap();
    // In a northern summer the south wall has little sun and in winter plenty; east and west bake in summer.
    assert!(f("south").summer_sun_hours < f("south").winter_sun_hours);
    assert!(f("south").summer_to_winter.unwrap() < 1.0);
    assert!(f("west").summer_sun_hours > f("south").summer_sun_hours && f("east").summer_sun_hours > f("south").summer_sun_hours);
    assert_eq!(r.best_for_glazing.len(), 8);
    assert!(r.best_for_glazing[0] == "north" || r.best_for_glazing[0] == "south" || r.best_for_glazing[0].starts_with("north"));
    // Overhang h / tan(noon altitude 86.45 deg) = 1.5 / 16.1 = about 0.09 m.
    near(r.overhang_m.unwrap(), 0.09, 0.02);
    assert!(r.notes.iter().any(|n| n.contains("wind")), "wind is reported as not computed");
    assert!(solar(27.0, None).unwrap().overhang_m.is_none());
    assert!(solar(80.0, None).is_err() && solar(f64::NAN, None).is_err());
    // Southern hemisphere mirrors it: the sunny winter wall is the north one.
    let s = solar(-27.0, None).unwrap();
    let n = s.facades.iter().find(|f| f.facade == "north").unwrap();
    assert!(n.summer_sun_hours < n.winter_sun_hours);
}

#[test]
fn a_screen_blocks_the_line_from_the_eye_to_the_target() {
    // Eye 1.7 m, 3 m from the screen; a seated person's head (1.2 m) 6 m behind it: 1.7 + (1.2 - 1.7) * 3/9 = 1.5333.
    near(screen_height(1.7, 3.0, 6.0, 1.2).unwrap().required_height_m, 1.53, 1e-9);
    // Target as tall as the eye: the screen must be as tall as the eye line.
    near(screen_height(1.7, 3.0, 6.0, 1.7).unwrap().required_height_m, 1.7, 1e-9);
    // Viewer above the target looks down: the screen can be lower; higher viewer (a floor up) raises the requirement the other way.
    assert!(screen_height(4.5, 3.0, 6.0, 1.2).unwrap().required_height_m > 1.7);
    assert!(screen_height(1.7, 0.0, 6.0, 1.2).is_err() && screen_height(-1.0, 3.0, 6.0, 1.2).is_err());
}

fn rooms(v: &[(&str, f64)]) -> Vec<(String, f64)> {
    v.iter().map(|(f, a)| (f.to_string(), *a)).collect()
}

#[test]
fn efficiency_flags_corridor_waste_and_compare_lines_alternatives_up() {
    let a = rooms(&[("majlis", 40.0), ("bedroom", 40.0), ("kitchen", 15.0), ("hall", 10.0), ("stair", 5.0)]);
    let e = efficiency(&a, 15.0).unwrap();
    assert_eq!((e.gross_m2, e.net_m2, e.circulation_m2, e.circulation_pct, e.net_to_gross), (110.0, 95.0, 15.0, 13.6, 0.86));
    assert!(!e.flagged && e.hints.is_empty());
    let b = rooms(&[("majlis", 40.0), ("bedroom", 30.0), ("hall", 30.0)]);
    let e = efficiency(&b, 15.0).unwrap();
    assert!(e.flagged && e.hints[0].contains("above the 15% limit"));
    assert!(efficiency(&a, 0.0).unwrap_err().contains("limit") && efficiency(&[], 15.0).is_err());
    let c = compare(&[(120.0, a.clone()), (100.0, b.clone()), (110.0, a.clone()), (90.0, b)], 15.0).unwrap();
    assert_eq!(c.rows.len(), 3, "at most three alternatives");
    assert_eq!((c.lowest_cost, c.least_circulation), (1, 0));
    assert!(c.not_available.contains(&"cooling load"));
    assert!(compare(&[], 15.0).is_err());
}

#[test]
fn stairs_and_ramps_are_checked_against_limits_the_caller_gives() {
    // 3000 mm in 17 risers: R = 176.5; with a 270 mm tread 2R + T = 623: within 20 mm of 620.
    let s = stair(3000.0, 17, 270.0, 620.0, 20.0).unwrap();
    near(s.riser_mm, 176.5, 0.05);
    near(s.two_r_plus_t_mm, 623.0, 0.2);
    assert!(s.comfortable);
    assert_eq!(s.run_mm, 270.0 * 16.0);
    assert!(!stair(3000.0, 14, 250.0, 620.0, 20.0).unwrap().comfortable, "R = 214: 678");
    assert!(stair(0.0, 17, 270.0, 620.0, 20.0).is_err() && stair(3000.0, 0, 270.0, 620.0, 20.0).is_err());
    let r = ramp(1500.0, 12500.0, Some(12.0)).unwrap();
    assert_eq!((r.slope_pct, r.ok, r.min_run_mm), (12.0, true, 12500.0));
    let r = ramp(1500.0, 10000.0, Some(12.0)).unwrap();
    assert!(!r.ok && r.min_run_mm == 12500.0);
    assert!(ramp(1500.0, 10000.0, None).unwrap_err().contains("missing"));
    assert!(ramp(0.0, 10000.0, Some(12.0)).is_err());
}
