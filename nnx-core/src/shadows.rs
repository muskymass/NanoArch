//! Ground shadows of the building through the year.
//!
//! For a latitude, a list of days of the year and a list of solar hours, this projects every shadow-casting solid of the 3D
//! scene (walls, slabs, columns, beams) onto the ground plane (z = 0, the floor top of the lowest level) along the sun's
//! direction. The sun's position comes from `analysis::sun` (declination and hour-angle formulas, about 1 degree; solar time,
//! so the clock time depends on the longitude and the time zone, which this module does not take).
//!
//! Inputs that are never invented: the **latitude**, the **days**, the **solar hours**, and, when the plan is not drawn with
//! +y at true north, the **north offset** (degrees the plan's +y axis is turned clockwise from true north). The caller passes
//! all of them; an empty or out-of-range one is reported by name and nothing is computed.
//!
//! Method: the shadow of a convex solid on the plane is the convex hull of its eight corners projected along the sun ray, so
//! each box gives one convex polygon. The shaded ground area (outside the building's own footprint) is measured on a raster
//! of the union of those polygons. A ground probe (a point the user names: a garden, a neighbour's boundary) gets its hours of
//! direct sun per day by sampling the day every 15 minutes. Trees, neighbouring buildings and terrain are not in the scene,
//! so they cast nothing here. The result is a geometric study, not a daylight or heat-gain simulation.

use crate::analysis::{Sun, sun};
use crate::model3d::Scene;
use serde::{Deserialize, Serialize};

/// Day of the year of the 21st of each month in a non-leap year (fixed calendar approximation).
pub const MONTH_21ST: [u32; 12] = [21, 52, 80, 111, 141, 172, 202, 233, 264, 294, 325, 355];

/// Below this sun altitude the shadow is longer than about 29 times the height; it is reported as grazing, not drawn.
const MIN_ALTITUDE_DEG: f64 = 2.0;
/// Sampling step of the sun-hours count, in hours.
const SAMPLE_STEP_H: f64 = 0.25;
/// Largest raster, in cells, used to measure a shadow area.
const MAX_CELLS: f64 = 90_000.0;
/// Element kinds that cast a shadow.
const CASTERS: [&str; 4] = ["Wall", "Slab", "Column", "Beam"];

#[derive(Debug, Clone, Deserialize)]
pub struct ShadowInput {
    pub latitude_deg: f64,
    /// Days of the year (1-365).
    pub days: Vec<u32>,
    /// Solar hours (0-24; 12 is solar noon).
    pub hours: Vec<f64>,
    /// Degrees the plan's +y axis is turned clockwise from true north (0: plan up is north).
    pub north_offset_deg: f64,
    /// Ground points (x, y in metres, plan frame) whose hours of direct sun are counted.
    #[serde(default)]
    pub probes: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HourStatus {
    /// The sun is above the grazing limit and the shadow is computed.
    Shadow,
    /// The sun is below the horizon.
    Night,
    /// The sun is above the horizon but lower than the grazing limit; the shadow is not drawn.
    Grazing,
}

#[derive(Debug, Clone, Serialize)]
pub struct HourShadow {
    pub hour: f64,
    pub status: HourStatus,
    pub sun: Option<Sun>,
    /// One convex polygon (plan x, y in metres) per shadow-casting box, empty unless `status` is `shadow`.
    pub polygons: Vec<Vec<[f64; 2]>>,
    /// Shaded ground outside the building's footprint, m2 (`None` unless `status` is `shadow`).
    pub ground_shadow_m2: Option<f64>,
    /// Distance from the tallest point's foot to the tip of its shadow, metres (`None` unless `status` is `shadow`).
    pub longest_shadow_m: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeSun {
    pub point: [f64; 2],
    /// Hours of direct sun on the day (the sun above the grazing limit and the point outside every shadow).
    pub sun_hours: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DayShadows {
    pub day: u32,
    pub declination_deg: f64,
    /// Hours the sun is above the horizon, sampled every 15 minutes.
    pub daylight_hours: f64,
    pub hours: Vec<HourShadow>,
    pub probes: Vec<ProbeSun>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShadowReport {
    pub latitude_deg: f64,
    pub north_offset_deg: f64,
    /// Height of the tallest shadow-casting point above the ground plane, metres.
    pub height_m: f64,
    pub casters: usize,
    pub days: Vec<DayShadows>,
    pub notes: Vec<String>,
}

/// Convex hull (counter-clockwise, no repeated first point) of a point set; fewer than three distinct points are returned as is.
fn hull(mut pts: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    pts.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9);
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut lower: Vec<[f64; 2]> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 1e-12 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<[f64; 2]> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 1e-12 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// True when `p` is inside or on the counter-clockwise convex polygon `poly`.
fn inside(poly: &[[f64; 2]], p: [f64; 2]) -> bool {
    if poly.len() < 3 {
        return false;
    }
    (0..poly.len()).all(|i| {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]) >= -1e-9
    })
}

/// The shadow polygon on the ground of one box `min..max` for a sun at `s`, with the plan's +y turned `north_offset_deg` from north.
/// The part of the box below the ground plane does not cast.
pub fn box_shadow(min: [f64; 3], max: [f64; 3], s: Sun, north_offset_deg: f64) -> Vec<[f64; 2]> {
    let bearing = (s.azimuth_deg - north_offset_deg).to_radians();
    // Unit plan vector pointing at the sun; a point at height z throws its shadow z / tan(altitude) the other way.
    let to_sun = [bearing.sin(), bearing.cos()];
    let k = 1.0 / s.altitude_deg.to_radians().tan();
    let mut pts = Vec::with_capacity(8);
    for &x in &[min[0], max[0]] {
        for &y in &[min[1], max[1]] {
            for &z in &[min[2].max(0.0), max[2].max(0.0)] {
                pts.push([x - z * k * to_sun[0], y - z * k * to_sun[1]]);
            }
        }
    }
    hull(pts)
}

fn validate(scene: &Scene, input: &ShadowInput) -> Result<(), String> {
    if !input.latitude_deg.is_finite() || !(-90.0..=90.0).contains(&input.latitude_deg) {
        return Err(format!("latitude_deg must be between -90 and 90, not {}", input.latitude_deg));
    }
    if !input.north_offset_deg.is_finite() {
        return Err("north_offset_deg must be a number".to_string());
    }
    if input.days.is_empty() {
        return Err("name at least one day of the year (days)".to_string());
    }
    if let Some(d) = input.days.iter().find(|d| !(1..=365).contains(*d)) {
        return Err(format!("day {d} is not a day of the year (1-365)"));
    }
    if input.hours.is_empty() {
        return Err("name at least one solar hour (hours)".to_string());
    }
    if let Some(h) = input.hours.iter().find(|h| !h.is_finite() || !(0.0..=24.0).contains(*h)) {
        return Err(format!("hour {h} is not a solar hour (0-24)"));
    }
    if input.probes.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
        return Err("a probe point has a value that is not a number".to_string());
    }
    if !scene.boxes.iter().any(|b| CASTERS.contains(&b.kind)) {
        return Err("the model has no wall, slab, column or beam to cast a shadow".to_string());
    }
    Ok(())
}

/// Area (m2) of the union of `polys` that is not under any of the `footprints` rectangles (x0, y0, x1, y1), measured on a raster.
fn shaded_outside(polys: &[Vec<[f64; 2]>], footprints: &[[f64; 4]]) -> f64 {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in polys.iter().flatten() {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    if x1 <= x0 || y1 <= y0 {
        return 0.0;
    }
    let cell = ((x1 - x0) * (y1 - y0) / MAX_CELLS).sqrt().max(0.1);
    let (nx, ny) = (((x1 - x0) / cell).ceil() as usize, ((y1 - y0) / cell).ceil() as usize);
    let mut count = 0usize;
    for i in 0..nx {
        for j in 0..ny {
            let p = [x0 + (i as f64 + 0.5) * cell, y0 + (j as f64 + 0.5) * cell];
            if polys.iter().any(|q| inside(q, p)) && !footprints.iter().any(|f| p[0] >= f[0] && p[0] <= f[2] && p[1] >= f[1] && p[1] <= f[3]) {
                count += 1;
            }
        }
    }
    count as f64 * cell * cell
}

fn polygons_at(scene: &Scene, s: Sun, north_offset_deg: f64) -> Vec<Vec<[f64; 2]>> {
    scene.boxes.iter().filter(|b| CASTERS.contains(&b.kind) && b.max[2] > 0.0).map(|b| box_shadow(b.min, b.max, s, north_offset_deg)).filter(|p| p.len() >= 3).collect()
}

/// Studies the shadows of `scene` for the days and hours of `input`.
pub fn study(scene: &Scene, input: &ShadowInput) -> Result<ShadowReport, String> {
    validate(scene, input)?;
    let casters: Vec<&crate::model3d::Box3> = scene.boxes.iter().filter(|b| CASTERS.contains(&b.kind) && b.max[2] > 0.0).collect();
    let footprints: Vec<[f64; 4]> = casters.iter().filter(|b| b.min[2] <= 0.5).map(|b| [b.min[0], b.min[1], b.max[0], b.max[1]]).collect();
    let height_m = casters.iter().map(|b| b.max[2]).fold(0.0, f64::max);
    let lat = input.latitude_deg;

    let mut days = Vec::with_capacity(input.days.len());
    for &day in &input.days {
        let mut hours = Vec::with_capacity(input.hours.len());
        for &hour in &input.hours {
            let above = sun(lat, day, hour);
            match above {
                None => hours.push(HourShadow { hour, status: HourStatus::Night, sun: None, polygons: Vec::new(), ground_shadow_m2: None, longest_shadow_m: None }),
                Some(s) if s.altitude_deg < MIN_ALTITUDE_DEG => {
                    hours.push(HourShadow { hour, status: HourStatus::Grazing, sun: Some(s), polygons: Vec::new(), ground_shadow_m2: None, longest_shadow_m: None })
                }
                Some(s) => {
                    let polygons = polygons_at(scene, s, input.north_offset_deg);
                    let area = shaded_outside(&polygons, &footprints);
                    let longest = height_m / s.altitude_deg.to_radians().tan();
                    hours.push(HourShadow { hour, status: HourStatus::Shadow, sun: Some(s), polygons, ground_shadow_m2: Some(area), longest_shadow_m: Some(longest) });
                }
            }
        }

        // Daylight and probe sun-hours: one pass over the day every 15 minutes.
        let steps = (24.0 / SAMPLE_STEP_H) as usize;
        let mut daylight = 0.0;
        let mut probe_hours = vec![0.0; input.probes.len()];
        for n in 0..steps {
            let t = (n as f64 + 0.5) * SAMPLE_STEP_H;
            let Some(s) = sun(lat, day, t) else { continue };
            daylight += SAMPLE_STEP_H;
            if input.probes.is_empty() || s.altitude_deg < MIN_ALTITUDE_DEG {
                continue;
            }
            let polys = polygons_at(scene, s, input.north_offset_deg);
            for (k, p) in input.probes.iter().enumerate() {
                if !polys.iter().any(|q| inside(q, *p)) {
                    probe_hours[k] += SAMPLE_STEP_H;
                }
            }
        }
        let probes = input.probes.iter().zip(probe_hours).map(|(p, h)| ProbeSun { point: *p, sun_hours: h }).collect();
        days.push(DayShadows { day, declination_deg: crate::analysis::declination_deg(day), daylight_hours: daylight, hours, probes });
    }

    let notes = vec![
        "Solar time: the hours are measured from solar noon, so the clock time depends on the longitude and the time zone.".to_string(),
        "Walls, slabs, columns and beams cast the shadows; trees, neighbouring buildings and terrain are not in the model.".to_string(),
        format!("A sun lower than {MIN_ALTITUDE_DEG} degrees is reported as grazing and its shadow is not drawn."),
        "Sun positions use approximate astronomical formulas (about 1 degree).".to_string(),
    ];
    Ok(ShadowReport { latitude_deg: lat, north_offset_deg: input.north_offset_deg, height_m, casters: casters.len(), days, notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model3d::Box3;
    use std::collections::BTreeMap;

    fn boxed(kind: &'static str, min: [f64; 3], max: [f64; 3]) -> Box3 {
        Box3 { id: "b".into(), kind, class: String::new(), min, max, visual: false, level: None, discipline: "architectural" }
    }
    fn scene(boxes: Vec<Box3>) -> Scene {
        Scene { levels: Vec::new(), boxes, min: [0.0; 3], max: [0.0; 3], storey_height: 3.0, assemblies: BTreeMap::new() }
    }
    fn bounds(poly: &[[f64; 2]]) -> (f64, f64, f64, f64) {
        let x: Vec<f64> = poly.iter().map(|p| p[0]).collect();
        let y: Vec<f64> = poly.iter().map(|p| p[1]).collect();
        (x.iter().cloned().fold(f64::MAX, f64::min), y.iter().cloned().fold(f64::MAX, f64::min), x.iter().cloned().fold(f64::MIN, f64::max), y.iter().cloned().fold(f64::MIN, f64::max))
    }
    fn input(days: Vec<u32>, hours: Vec<f64>) -> ShadowInput {
        ShadowInput { latitude_deg: 24.0, days, hours, north_offset_deg: 0.0, probes: Vec::new() }
    }

    #[test]
    fn a_southern_sun_at_45_degrees_throws_the_shadow_north_by_the_height() {
        let s = Sun { altitude_deg: 45.0, azimuth_deg: 180.0 };
        let (x0, y0, x1, y1) = bounds(&box_shadow([0.0, 0.0, 0.0], [4.0, 4.0, 10.0], s, 0.0));
        assert!((x0 - 0.0).abs() < 1e-9 && (x1 - 4.0).abs() < 1e-9, "no sideways shift: {x0} {x1}");
        assert!((y0 - 0.0).abs() < 1e-9 && (y1 - 14.0).abs() < 1e-9, "tip at 4 + 10 m: {y0} {y1}");
    }

    #[test]
    fn a_western_sun_throws_the_shadow_east_and_a_turned_plan_turns_it() {
        let west = Sun { altitude_deg: 45.0, azimuth_deg: 270.0 };
        let (x0, _, x1, _) = bounds(&box_shadow([0.0, 0.0, 0.0], [4.0, 4.0, 10.0], west, 0.0));
        assert!((x0 - 0.0).abs() < 1e-9 && (x1 - 14.0).abs() < 1e-9);
        // The plan's +y is turned 90 degrees clockwise (plan up is true east): a southern sun then lies on plan +x and throws the shadow to plan -x.
        let south = Sun { altitude_deg: 45.0, azimuth_deg: 180.0 };
        let (a, _, b, _) = bounds(&box_shadow([0.0, 0.0, 0.0], [4.0, 4.0, 10.0], south, 90.0));
        assert!((b - 4.0).abs() < 1e-9 && (a + 10.0).abs() < 1e-9, "shadow goes to plan -x: {a} {b}");
    }

    #[test]
    fn the_shadow_of_a_flat_box_on_the_ground_is_its_footprint() {
        let s = Sun { altitude_deg: 30.0, azimuth_deg: 100.0 };
        let poly = box_shadow([1.0, 2.0, 0.0], [3.0, 5.0, 0.0], s, 0.0);
        let (x0, y0, x1, y1) = bounds(&poly);
        assert_eq!((x0, y0, x1, y1), (1.0, 2.0, 3.0, 5.0));
    }

    #[test]
    fn the_summer_noon_shadow_is_shorter_than_the_winter_noon_shadow() {
        let sc = scene(vec![boxed("Wall", [0.0, 0.0, 0.0], [10.0, 10.0, 6.0])]);
        let r = study(&sc, &input(vec![172, 355], vec![12.0])).unwrap();
        let summer = r.days[0].hours[0].longest_shadow_m.unwrap();
        let winter = r.days[1].hours[0].longest_shadow_m.unwrap();
        assert!(winter > 2.0 * summer, "winter {winter} vs summer {summer}");
        // Latitude 24: the noon altitude on the summer solstice is 90 - 24 + 23.45 = 89.45, so the shadow of 6 m is about 0.06 m.
        assert!((summer - 6.0 / 89.45_f64.to_radians().tan()).abs() < 0.05, "{summer}");
        assert!(r.days[0].hours[0].ground_shadow_m2.unwrap() >= 0.0);
    }

    #[test]
    fn night_and_a_grazing_sun_are_reported_not_drawn() {
        let sc = scene(vec![boxed("Wall", [0.0, 0.0, 0.0], [10.0, 10.0, 6.0])]);
        let r = study(&sc, &input(vec![80], vec![0.0, 12.0])).unwrap();
        assert_eq!(r.days[0].hours[0].status, HourStatus::Night);
        assert!(r.days[0].hours[0].polygons.is_empty() && r.days[0].hours[0].ground_shadow_m2.is_none());
        assert_eq!(r.days[0].hours[1].status, HourStatus::Shadow);
        assert!(!r.days[0].hours[1].polygons.is_empty());
    }

    #[test]
    fn a_probe_behind_the_building_gets_fewer_sun_hours_than_one_in_the_open() {
        // A long 10 m wall on the plan's x axis; at 24 N on the winter solstice the sun is in the south, so the shadow falls north.
        let sc = scene(vec![boxed("Wall", [0.0, 0.0, 0.0], [20.0, 0.2, 10.0])]);
        let mut inp = input(vec![355], vec![12.0]);
        inp.probes = vec![[10.0, 3.0], [10.0, -30.0]];
        let r = study(&sc, &inp).unwrap();
        let p = &r.days[0].probes;
        assert!(p[0].sun_hours < p[1].sun_hours, "behind {} open {}", p[0].sun_hours, p[1].sun_hours);
        assert!(p[1].sun_hours > 9.0 && p[1].sun_hours <= r.days[0].daylight_hours + 1e-9, "{}", p[1].sun_hours);
        assert!(r.days[0].daylight_hours > 10.0 && r.days[0].daylight_hours < 11.5, "winter day at 24 N: {}", r.days[0].daylight_hours);
    }

    #[test]
    fn the_shaded_area_excludes_the_footprint() {
        // Sun due south at noon on the winter solstice: a 10 x 10 x 5 box throws a strip north of its footprint.
        let sc = scene(vec![boxed("Wall", [0.0, 0.0, 0.0], [10.0, 10.0, 5.0])]);
        let r = study(&sc, &input(vec![355], vec![12.0])).unwrap();
        let h = &r.days[0].hours[0];
        let len = h.longest_shadow_m.unwrap();
        let expected = 10.0 * len;
        assert!((h.ground_shadow_m2.unwrap() - expected).abs() < 0.03 * expected, "{} vs {expected}", h.ground_shadow_m2.unwrap());
    }

    #[test]
    fn bad_inputs_are_rejected_by_name() {
        let sc = scene(vec![boxed("Wall", [0.0, 0.0, 0.0], [4.0, 4.0, 3.0])]);
        assert!(study(&sc, &ShadowInput { latitude_deg: 91.0, ..input(vec![1], vec![12.0]) }).unwrap_err().contains("latitude_deg"));
        assert!(study(&sc, &input(vec![], vec![12.0])).unwrap_err().contains("days"));
        assert!(study(&sc, &input(vec![366], vec![12.0])).unwrap_err().contains("366"));
        assert!(study(&sc, &input(vec![1], vec![])).unwrap_err().contains("hours"));
        assert!(study(&sc, &input(vec![1], vec![25.0])).unwrap_err().contains("25"));
        assert!(study(&scene(vec![boxed("Door", [0.0; 3], [1.0; 3])]), &input(vec![1], vec![12.0])).unwrap_err().contains("no wall"));
    }
}
