//! Basement and underground parking: one basement level below the building, with parking bays, storage, service rooms,
//! vehicle ramps and stair/lift cores placed inside its outline.
//!
//! The basement outline is a rectangle in millimetres in the same frame as the bounds it must stay inside (the plot, or the
//! set-back boundary the user chooses; the lower-left corner of the bounds is the origin). Nothing is assumed:
//! - a space outside the basement, overlapping another, with a non-positive size or a duplicate key is refused with the
//!   reason; a parking space needs its declared bay count, a ramp needs the axis it runs along, and nothing else takes either;
//! - the storey height, the slab thickness and the depth of the basement floor below ground are required inputs;
//! - every limit (smallest clear height, steepest ramp, narrowest ramp, smallest gross area per bay, fewest exits, longest
//!   straight-line distance to an exit) and the depth of the water table are user inputs taken from the code, the approving
//!   authority or the soil report; without one the matching check is `unchecked`, never passed.
//!
//! The report gives the basement area, the retaining-wall length and face area, the excavation volume down to the finished
//! floor (the raft or slab thickness below it is not included), the clear height, each space's area, each ramp's slope (the
//! rise is the depth of the floor below ground) and width, each bay space's gross area per bay, and each space's straight-line
//! distance to the nearest stair/lift core. The slope check on a ramp is a geometric check of the rectangle given, not a design
//! of the vehicle turning curves. A passed groundwater check only means the floor lies above the water table entered; it is
//! not a waterproofing or dewatering design.

use crate::courtyard::Finding;
use crate::layout::Rect;
use crate::rooftop::{rect_gap, Check};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpaceKind {
    /// Car bays and their aisles; needs `stalls`.
    Parking,
    Storage,
    /// Plant, pump, tank or electrical room.
    Service,
    /// A vehicle ramp up to ground level; needs `run_along`.
    Ramp,
    /// A stair or lift core that leads up and out.
    Core,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    X,
    Y,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct BasementSpace {
    pub key: String,
    pub kind: SpaceKind,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    /// Parking only: the number of bays the space holds.
    #[serde(default)]
    pub stalls: Option<u32>,
    /// Ramp only: the axis along which the ramp climbs; its length is the side on that axis.
    #[serde(default)]
    pub run_along: Option<Axis>,
}

/// The basement, its spaces and the optional limits, all lengths in millimetres.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct BasementSpec {
    /// The basement outline in the frame of the bounds.
    pub outline: OutlineRect,
    /// Height from the basement floor to the underside of the slab above it, finished floor to finished floor.
    pub storey_mm: i64,
    /// Thickness of the slab above the basement.
    pub slab_mm: i64,
    /// Depth of the basement finished floor below the ground level at its edge.
    pub depth_mm: i64,
    #[serde(default)]
    pub spaces: Vec<BasementSpace>,
    /// Depth of the water table below ground from the soil report; `None` leaves the groundwater check `unchecked`.
    #[serde(default)]
    pub water_table_depth_mm: Option<i64>,
    #[serde(default)]
    pub min_clear_mm: Option<i64>,
    /// Steepest ramp, in percent (rise over run times 100).
    #[serde(default)]
    pub max_ramp_slope_pct: Option<f64>,
    #[serde(default)]
    pub min_ramp_width_mm: Option<i64>,
    /// Smallest gross area (the whole parking space divided by its bays), in square metres.
    #[serde(default)]
    pub min_area_per_stall_m2: Option<f64>,
    /// Fewest stair/lift cores.
    #[serde(default)]
    pub min_exits: Option<u32>,
    /// Longest straight-line distance from a space to its nearest core.
    #[serde(default)]
    pub max_travel_mm: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct OutlineRect {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpaceReport {
    pub key: String,
    pub kind: SpaceKind,
    pub area_m2: f64,
    /// Parking only.
    pub stalls: Option<u32>,
    pub area_per_stall_m2: Option<f64>,
    /// Ramp only: rise over run in percent, the rise being the depth of the floor below ground.
    pub slope_pct: Option<f64>,
    /// Ramp only: the side across the climb.
    pub width_mm: Option<i64>,
    /// Straight-line distance to the nearest core; `None` for a core itself or when there is no core.
    pub nearest_core_mm: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BasementReport {
    pub basement_m2: f64,
    pub perimeter_mm: i64,
    /// Perimeter times depth: the face of retaining wall.
    pub retaining_wall_m2: f64,
    /// Basement area times depth, down to the finished floor.
    pub excavation_m3: f64,
    pub clear_mm: i64,
    pub spaces: Vec<SpaceReport>,
    pub parking_stalls: u32,
    pub parking_m2: f64,
    /// Basement area not taken by any space.
    pub unallocated_m2: f64,
    pub core_count: u32,
    /// Distance from the floor up to the water table entered (negative when the floor lies below it).
    pub floor_above_water_mm: Option<i64>,
    pub clear_check: Check,
    pub ramp_slope_check: Check,
    pub ramp_width_check: Check,
    pub stall_check: Check,
    pub exits_check: Check,
    pub travel_check: Check,
    pub groundwater_check: Check,
    pub findings: Vec<Finding>,
}

fn at_least<T: PartialOrd>(value: Option<T>, min: Option<T>) -> Check {
    match (value, min) {
        (Some(v), Some(m)) if v >= m => Check::Pass,
        (Some(_), Some(_)) => Check::Fail,
        _ => Check::Unchecked,
    }
}

fn at_most<T: PartialOrd>(value: Option<T>, max: Option<T>) -> Check {
    match (value, max) {
        (Some(v), Some(m)) if v <= m => Check::Pass,
        (Some(_), Some(_)) => Check::Fail,
        _ => Check::Unchecked,
    }
}

fn rect_of(s: &BasementSpace) -> Rect {
    Rect { x: s.x, y: s.y, w: s.w, h: s.h }
}

fn validate(bounds: &Rect, spec: &BasementSpec) -> Result<(), String> {
    let o = &spec.outline;
    if bounds.w <= 0 || bounds.h <= 0 {
        return Err(format!("basement: the bounds must have a positive size (got {} x {} mm)", bounds.w, bounds.h));
    }
    if o.w <= 0 || o.h <= 0 {
        return Err(format!("basement: the outline must have a positive size (got {} x {} mm)", o.w, o.h));
    }
    if o.x < 0 || o.y < 0 || o.x + o.w > bounds.w || o.y + o.h > bounds.h {
        return Err(format!("basement: the outline lies outside the bounds ({} x {} mm)", bounds.w, bounds.h));
    }
    for (name, v) in [("storey_mm", spec.storey_mm), ("slab_mm", spec.slab_mm), ("depth_mm", spec.depth_mm)] {
        if v <= 0 {
            return Err(format!("basement: {name} is required and must be positive, not {v} mm"));
        }
    }
    if spec.slab_mm >= spec.storey_mm {
        return Err(format!("basement: slab_mm ({} mm) must be thinner than storey_mm ({} mm)", spec.slab_mm, spec.storey_mm));
    }
    for (name, v) in [
        ("water_table_depth_mm", spec.water_table_depth_mm),
        ("min_clear_mm", spec.min_clear_mm),
        ("min_ramp_width_mm", spec.min_ramp_width_mm),
        ("max_travel_mm", spec.max_travel_mm),
    ] {
        if let Some(m) = v.filter(|m| *m <= 0) {
            return Err(format!("basement: {name} must be positive, not {m} mm"));
        }
    }
    for (name, v) in [("max_ramp_slope_pct", spec.max_ramp_slope_pct), ("min_area_per_stall_m2", spec.min_area_per_stall_m2)] {
        if let Some(m) = v.filter(|m| !m.is_finite() || *m <= 0.0) {
            return Err(format!("basement: {name} must be a positive number, not {m}"));
        }
    }
    if spec.min_exits == Some(0) {
        return Err("basement: min_exits must be at least 1".to_string());
    }
    let mut keys = BTreeSet::new();
    for s in &spec.spaces {
        if s.key.trim().is_empty() {
            return Err("basement: every space needs a key".to_string());
        }
        if !keys.insert(s.key.as_str()) {
            return Err(format!("basement: the key '{}' is used by more than one space", s.key));
        }
        if s.w <= 0 || s.h <= 0 {
            return Err(format!("basement: space '{}' must have a positive width and depth (got {} x {} mm)", s.key, s.w, s.h));
        }
        if s.x < o.x || s.y < o.y || s.x + s.w > o.x + o.w || s.y + s.h > o.y + o.h {
            return Err(format!("basement: space '{}' lies outside the basement outline", s.key));
        }
        match (s.kind, s.stalls) {
            (SpaceKind::Parking, None | Some(0)) => return Err(format!("basement: parking space '{}' needs stalls of at least 1", s.key)),
            (SpaceKind::Parking, Some(_)) => {}
            (_, Some(_)) => return Err(format!("basement: only a parking space has stalls; remove stalls from '{}'", s.key)),
            _ => {}
        }
        match (s.kind, s.run_along) {
            (SpaceKind::Ramp, None) => return Err(format!("basement: ramp '{}' needs run_along (x or y)", s.key)),
            (SpaceKind::Ramp, Some(_)) => {}
            (_, Some(_)) => return Err(format!("basement: only a ramp has run_along; remove it from '{}'", s.key)),
            _ => {}
        }
    }
    for (i, a) in spec.spaces.iter().enumerate() {
        for b in &spec.spaces[i + 1..] {
            if a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h {
                return Err(format!("basement: spaces '{}' and '{}' overlap", a.key, b.key));
            }
        }
    }
    Ok(())
}

/// Checks a basement level placed inside `bounds` against the limits the user entered.
pub fn check_basement(bounds: &Rect, spec: &BasementSpec) -> Result<BasementReport, String> {
    validate(bounds, spec)?;
    let o = &spec.outline;
    let area_mm2 = o.w as i128 * o.h as i128;
    let perimeter_mm = 2 * (o.w + o.h);
    let clear_mm = spec.storey_mm - spec.slab_mm;
    let depth = spec.depth_mm as f64;

    let cores: Vec<Rect> = spec.spaces.iter().filter(|s| s.kind == SpaceKind::Core).map(rect_of).collect();
    let mut spaces = Vec::with_capacity(spec.spaces.len());
    let (mut taken_mm2, mut parking_mm2, mut parking_stalls) = (0i128, 0i128, 0u32);
    for s in &spec.spaces {
        let r = rect_of(s);
        let area = s.w as i128 * s.h as i128;
        taken_mm2 += area;
        let (stalls, per_stall) = match (s.kind, s.stalls) {
            (SpaceKind::Parking, Some(n)) => {
                parking_mm2 += area;
                parking_stalls += n;
                (Some(n), Some(area as f64 / 1e6 / n as f64))
            }
            _ => (None, None),
        };
        let (slope_pct, width_mm) = match s.run_along {
            Some(Axis::X) => (Some(depth / s.w as f64 * 100.0), Some(s.h)),
            Some(Axis::Y) => (Some(depth / s.h as f64 * 100.0), Some(s.w)),
            None => (None, None),
        };
        let nearest_core_mm = match s.kind {
            SpaceKind::Core => None,
            _ => cores.iter().map(|c| rect_gap(&r, c)).min(),
        };
        spaces.push(SpaceReport { key: s.key.clone(), kind: s.kind, area_m2: area as f64 / 1e6, stalls, area_per_stall_m2: per_stall, slope_pct, width_mm, nearest_core_mm });
    }

    let ramps: Vec<&SpaceReport> = spaces.iter().filter(|s| s.kind == SpaceKind::Ramp).collect();
    let steepest = ramps.iter().filter_map(|r| r.slope_pct).max_by(f64::total_cmp);
    let narrowest = ramps.iter().filter_map(|r| r.width_mm).min();
    let tightest_bay = spaces.iter().filter_map(|s| s.area_per_stall_m2).min_by(f64::total_cmp);
    let farthest = spaces.iter().filter_map(|s| s.nearest_core_mm).max();
    let core_count = cores.len() as u32;
    let floor_above_water_mm = spec.water_table_depth_mm.map(|w| w - spec.depth_mm);

    let clear_check = at_least(Some(clear_mm), spec.min_clear_mm);
    let ramp_slope_check = at_most(steepest, spec.max_ramp_slope_pct);
    let ramp_width_check = at_least(narrowest, spec.min_ramp_width_mm);
    let stall_check = at_least(tightest_bay, spec.min_area_per_stall_m2);
    let exits_check = at_least(spec.min_exits.map(|_| core_count), spec.min_exits);
    let travel_check = at_most(farthest, spec.max_travel_mm);
    let groundwater_check = match floor_above_water_mm {
        Some(m) if m >= 0 => Check::Pass,
        Some(_) => Check::Fail,
        None => Check::Unchecked,
    };

    let mut findings = Vec::new();
    if let (Check::Fail, Some(m)) = (clear_check, spec.min_clear_mm) {
        findings.push(Finding {
            ar: format!("الارتفاع الصافي في القبو {clear_mm} مم وهو أقل من الحد الأدنى {m} مم."),
            en: format!("The basement clear height is {clear_mm} mm, below the {m} mm minimum."),
        });
    }
    if let (Check::Fail, Some(m), Some(v)) = (ramp_slope_check, spec.max_ramp_slope_pct, steepest) {
        let k = ramps.iter().find(|r| r.slope_pct == Some(v)).map_or("", |r| r.key.as_str());
        findings.push(Finding {
            ar: format!("ميل المنحدر «{k}» {v:.1}% وهو أشد من الحد الأقصى {m:.1}%."),
            en: format!("Ramp '{k}' climbs at {v:.1}%, steeper than the {m:.1}% maximum."),
        });
    }
    if let (Check::Fail, Some(m), Some(v)) = (ramp_width_check, spec.min_ramp_width_mm, narrowest) {
        let k = ramps.iter().find(|r| r.width_mm == Some(v)).map_or("", |r| r.key.as_str());
        findings.push(Finding {
            ar: format!("عرض المنحدر «{k}» {v} مم وهو أقل من الحد الأدنى {m} مم."),
            en: format!("Ramp '{k}' is {v} mm wide, below the {m} mm minimum."),
        });
    }
    if let (Check::Fail, Some(m), Some(v)) = (stall_check, spec.min_area_per_stall_m2, tightest_bay) {
        let k = spaces.iter().find(|s| s.area_per_stall_m2 == Some(v)).map_or("", |s| s.key.as_str());
        findings.push(Finding {
            ar: format!("نصيب الموقف الواحد في «{k}» {v:.1} م² وهو أقل من الحد الأدنى {m:.1} م²."),
            en: format!("'{k}' gives {v:.1} m² per bay, below the {m:.1} m² minimum."),
        });
    }
    if let (Check::Fail, Some(m)) = (exits_check, spec.min_exits) {
        findings.push(Finding {
            ar: format!("عدد نواة الدرج أو المصعد {core_count} وهو أقل من الحد الأدنى {m}."),
            en: format!("The basement has {core_count} stair/lift core(s), fewer than the {m} required."),
        });
    }
    if let (Check::Fail, Some(m), Some(v)) = (travel_check, spec.max_travel_mm, farthest) {
        let k = spaces.iter().find(|s| s.nearest_core_mm == Some(v)).map_or("", |s| s.key.as_str());
        findings.push(Finding {
            ar: format!("«{k}» يبعد {v} مم عن أقرب نواة وهو أكثر من الحد الأقصى {m} مم."),
            en: format!("'{k}' is {v} mm from the nearest core, more than the {m} mm maximum."),
        });
    }
    if let (Check::Fail, Some(m)) = (groundwater_check, floor_above_water_mm) {
        findings.push(Finding {
            ar: format!("أرضية القبو أخفض من منسوب الماء الجوفي المُدخَل بمقدار {} مم: يلزم تصميم عزل وتصريف يعتمده مهندس التربة والإنشاء.", -m),
            en: format!("The basement floor is {} mm below the water table entered: a waterproofing and dewatering design from the geotechnical and structural engineer is needed.", -m),
        });
    }
    if parking_stalls > 0 && ramps.is_empty() {
        findings.push(Finding {
            ar: "في القبو مواقف ولم يُدخَل منحدر مركبات: لم يُفحص الوصول إليها.".to_string(),
            en: "The basement has parking but no vehicle ramp was entered: access to it was not checked.".to_string(),
        });
    }
    for (check, ar, en) in [
        (clear_check, "الارتفاع الصافي", "the clear height"),
        (ramp_slope_check, "ميل المنحدر (يلزم منحدر وحد أقصى معاً)", "the ramp slope (both a ramp and a maximum are needed)"),
        (ramp_width_check, "عرض المنحدر (يلزم منحدر وحد أدنى معاً)", "the ramp width (both a ramp and a minimum are needed)"),
        (stall_check, "مساحة الموقف (يلزم موقف وحد أدنى معاً)", "the area per bay (both a parking space and a minimum are needed)"),
        (exits_check, "عدد المخارج", "the number of exits"),
        (travel_check, "مسافة الوصول إلى أقرب نواة (يلزم وجود نواة وحد أقصى معاً)", "the distance to the nearest core (both a core and a maximum are needed)"),
        (groundwater_check, "منسوب الماء الجوفي", "the groundwater level"),
    ] {
        if check == Check::Unchecked {
            findings.push(Finding { ar: format!("لم يُدخَل ما يلزم لفحص {ar}: لم يُفحص."), en: format!("Nothing was entered to check {en}: not checked.") });
        }
    }
    Ok(BasementReport {
        basement_m2: area_mm2 as f64 / 1e6,
        perimeter_mm,
        retaining_wall_m2: perimeter_mm as f64 * depth / 1e6,
        excavation_m3: area_mm2 as f64 * depth / 1e9,
        clear_mm,
        spaces,
        parking_stalls,
        parking_m2: parking_mm2 as f64 / 1e6,
        unallocated_m2: (area_mm2 - taken_mm2) as f64 / 1e6,
        core_count,
        floor_above_water_mm,
        clear_check,
        ramp_slope_check,
        ramp_width_check,
        stall_check,
        exits_check,
        travel_check,
        groundwater_check,
        findings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: Rect = Rect { x: 0, y: 0, w: 20000, h: 30000 };

    fn space(key: &str, kind: SpaceKind, x: i64, y: i64, w: i64, h: i64) -> BasementSpace {
        BasementSpace { key: key.into(), kind, x, y, w, h, stalls: None, run_along: None }
    }
    fn parking(key: &str, x: i64, y: i64, w: i64, h: i64, stalls: u32) -> BasementSpace {
        BasementSpace { stalls: Some(stalls), ..space(key, SpaceKind::Parking, x, y, w, h) }
    }
    fn ramp(key: &str, x: i64, y: i64, w: i64, h: i64, axis: Axis) -> BasementSpace {
        BasementSpace { run_along: Some(axis), ..space(key, SpaceKind::Ramp, x, y, w, h) }
    }
    fn spec(spaces: Vec<BasementSpace>) -> BasementSpec {
        BasementSpec {
            outline: OutlineRect { x: 2000, y: 2000, w: 16000, h: 26000 },
            storey_mm: 3500,
            slab_mm: 300,
            depth_mm: 3000,
            spaces,
            water_table_depth_mm: None,
            min_clear_mm: None,
            max_ramp_slope_pct: None,
            min_ramp_width_mm: None,
            min_area_per_stall_m2: None,
            min_exits: None,
            max_travel_mm: None,
        }
    }

    fn layout() -> Vec<BasementSpace> {
        vec![
            parking("bays", 2000, 2000, 16000, 10000, 12),
            ramp("ramp", 2000, 13000, 4000, 15000, Axis::Y),
            space("core", SpaceKind::Core, 14000, 14000, 3000, 3000),
            space("store", SpaceKind::Storage, 8000, 14000, 4000, 4000),
        ]
    }

    #[test]
    fn areas_excavation_and_walls_are_exact() {
        let r = check_basement(&BOUNDS, &spec(layout())).unwrap();
        assert!((r.basement_m2 - 416.0).abs() < 1e-9);
        assert_eq!(r.perimeter_mm, 84000);
        assert!((r.retaining_wall_m2 - 252.0).abs() < 1e-9);
        assert!((r.excavation_m3 - 1248.0).abs() < 1e-9);
        assert_eq!(r.clear_mm, 3200);
        assert_eq!(r.parking_stalls, 12);
        assert!((r.parking_m2 - 160.0).abs() < 1e-9);
        // 416 - 160 (bays) - 60 (ramp) - 9 (core) - 16 (store).
        assert!((r.unallocated_m2 - 171.0).abs() < 1e-9);
        assert_eq!(r.core_count, 1);
        let bays = &r.spaces[0];
        assert!((bays.area_per_stall_m2.unwrap() - 160.0 / 12.0).abs() < 1e-9);
    }

    #[test]
    fn ramp_slope_width_and_distance_to_core_are_measured() {
        let r = check_basement(&BOUNDS, &spec(layout())).unwrap();
        let ramp = r.spaces.iter().find(|s| s.key == "ramp").unwrap();
        // Rise 3000 over a 15000 run.
        assert!((ramp.slope_pct.unwrap() - 20.0).abs() < 1e-9);
        assert_eq!(ramp.width_mm, Some(4000));
        // Ramp spans x 2000..6000, core starts at x 14000 with the same y range overlap: gap 8000.
        assert_eq!(ramp.nearest_core_mm, Some(8000));
        let core = r.spaces.iter().find(|s| s.key == "core").unwrap();
        assert_eq!(core.nearest_core_mm, None);
        let x_run = spec(vec![ramp_x()]);
        let rx = check_basement(&BOUNDS, &x_run).unwrap();
        assert!((rx.spaces[0].slope_pct.unwrap() - 18.75).abs() < 1e-9);
        assert_eq!(rx.spaces[0].width_mm, Some(3000));
    }

    fn ramp_x() -> BasementSpace {
        ramp("rx", 2000, 2000, 16000, 3000, Axis::X)
    }

    #[test]
    fn limits_not_met_fail_and_limits_not_entered_are_unchecked() {
        let none = check_basement(&BOUNDS, &spec(layout())).unwrap();
        assert_eq!(
            (none.clear_check, none.ramp_slope_check, none.ramp_width_check, none.stall_check, none.exits_check, none.travel_check, none.groundwater_check),
            (Check::Unchecked, Check::Unchecked, Check::Unchecked, Check::Unchecked, Check::Unchecked, Check::Unchecked, Check::Unchecked)
        );
        assert_eq!(none.findings.iter().filter(|f| f.en.contains("not checked")).count(), 7);
        let tight = BasementSpec {
            min_clear_mm: Some(3500),
            max_ramp_slope_pct: Some(15.0),
            min_ramp_width_mm: Some(5000),
            min_area_per_stall_m2: Some(25.0),
            min_exits: Some(2),
            max_travel_mm: Some(5000),
            water_table_depth_mm: Some(2000),
            ..spec(layout())
        };
        let r = check_basement(&BOUNDS, &tight).unwrap();
        assert_eq!(
            (r.clear_check, r.ramp_slope_check, r.ramp_width_check, r.stall_check, r.exits_check, r.travel_check, r.groundwater_check),
            (Check::Fail, Check::Fail, Check::Fail, Check::Fail, Check::Fail, Check::Fail, Check::Fail)
        );
        assert_eq!(r.floor_above_water_mm, Some(-1000));
        assert!(r.findings.iter().any(|f| f.en.contains("clear height is 3200 mm, below the 3500 mm")));
        assert!(r.findings.iter().any(|f| f.en.contains("Ramp 'ramp' climbs at 20.0%, steeper than the 15.0%")));
        assert!(r.findings.iter().any(|f| f.en.contains("Ramp 'ramp' is 4000 mm wide")));
        assert!(r.findings.iter().any(|f| f.en.contains("'bays' gives 13.3 m² per bay")));
        assert!(r.findings.iter().any(|f| f.en.contains("1 stair/lift core(s), fewer than the 2")));
        assert!(r.findings.iter().any(|f| f.en.contains("from the nearest core, more than the 5000 mm")));
        assert!(r.findings.iter().any(|f| f.en.contains("1000 mm below the water table")));
        let ok = BasementSpec {
            min_clear_mm: Some(2300),
            max_ramp_slope_pct: Some(20.0),
            min_ramp_width_mm: Some(3500),
            min_area_per_stall_m2: Some(13.0),
            min_exits: Some(1),
            max_travel_mm: Some(8000),
            water_table_depth_mm: Some(3000),
            ..spec(layout())
        };
        let r = check_basement(&BOUNDS, &ok).unwrap();
        assert_eq!(
            (r.clear_check, r.ramp_slope_check, r.ramp_width_check, r.stall_check, r.exits_check, r.travel_check, r.groundwater_check),
            (Check::Pass, Check::Pass, Check::Pass, Check::Pass, Check::Pass, Check::Pass, Check::Pass)
        );
    }

    #[test]
    fn a_limit_with_nothing_to_measure_is_unchecked_and_parking_without_a_ramp_is_flagged() {
        let s = BasementSpec { max_ramp_slope_pct: Some(12.0), max_travel_mm: Some(5000), ..spec(vec![parking("bays", 2000, 2000, 8000, 5000, 4)]) };
        let r = check_basement(&BOUNDS, &s).unwrap();
        assert_eq!((r.ramp_slope_check, r.travel_check), (Check::Unchecked, Check::Unchecked));
        assert!(r.findings.iter().any(|f| f.en.contains("no vehicle ramp was entered")));
        // A minimum number of exits with no core at all is a failure, not an unchecked item.
        let none = BasementSpec { min_exits: Some(1), ..s };
        assert_eq!(check_basement(&BOUNDS, &none).unwrap().exits_check, Check::Fail);
    }

    #[test]
    fn invalid_input_is_refused_with_the_reason() {
        let e = |s: BasementSpec| check_basement(&BOUNDS, &s).unwrap_err();
        assert!(e(BasementSpec { outline: OutlineRect { x: 0, y: 0, w: 0, h: 100 }, ..spec(vec![]) }).contains("outline must have a positive size"));
        assert!(e(BasementSpec { outline: OutlineRect { x: 5000, y: 0, w: 16000, h: 1000 }, ..spec(vec![]) }).contains("outside the bounds"));
        assert!(e(BasementSpec { depth_mm: 0, ..spec(vec![]) }).contains("depth_mm is required"));
        assert!(e(BasementSpec { slab_mm: 3500, ..spec(vec![]) }).contains("thinner than storey_mm"));
        assert!(e(spec(vec![parking("p", 0, 0, 1000, 1000, 2)])).contains("outside the basement outline"));
        assert!(e(spec(vec![parking("p", 2000, 2000, 1000, 1000, 0)])).contains("needs stalls of at least 1"));
        assert!(e(spec(vec![BasementSpace { stalls: Some(3), ..space("s", SpaceKind::Storage, 2000, 2000, 1000, 1000) }])).contains("only a parking space has stalls"));
        assert!(e(spec(vec![space("r", SpaceKind::Ramp, 2000, 2000, 1000, 1000)])).contains("needs run_along"));
        assert!(e(spec(vec![BasementSpace { run_along: Some(Axis::X), ..space("s", SpaceKind::Storage, 2000, 2000, 1000, 1000) }])).contains("only a ramp has run_along"));
        assert!(e(spec(vec![space("a", SpaceKind::Storage, 2000, 2000, 3000, 3000), space("b", SpaceKind::Core, 4000, 4000, 3000, 3000)])).contains("'a' and 'b' overlap"));
        assert!(e(spec(vec![space("a", SpaceKind::Storage, 2000, 2000, 1000, 1000), space("a", SpaceKind::Core, 5000, 5000, 1000, 1000)])).contains("more than one space"));
        assert!(e(BasementSpec { max_ramp_slope_pct: Some(f64::NAN), ..spec(vec![]) }).contains("max_ramp_slope_pct"));
        assert!(e(BasementSpec { min_exits: Some(0), ..spec(vec![]) }).contains("min_exits"));
        assert!(e(BasementSpec { min_clear_mm: Some(-5), ..spec(vec![]) }).contains("min_clear_mm must be positive"));
    }
}
