//! Automatic parking layout and vehicle turning envelope.
//!
//! Given a rectangular parking area (millimetres, in the frame of the bounds it must stay inside) and the stall and aisle
//! sizes the user chose, the layout packs 90-degree stalls into the area and returns every stall and aisle rectangle.
//! Nothing is assumed:
//! - the stall width, stall length and aisle width are required inputs; there is no default stall size;
//! - the limits (smallest stall width, smallest stall length, smallest aisle, stalls required) and the vehicle used for the
//!   turning check are user inputs from the code, the approving authority or the vehicle's data sheet; without one the
//!   matching check is `unchecked`, never passed.
//!
//! The packer tries rows running along x and rows running along y and keeps the one that holds more stalls (x on a tie). In
//! the depth direction it chooses the largest number of stall rows `r` that fits with `ceil(r / 2)` aisles, so each aisle
//! serves two rows back to back except an odd last row that has its own aisle. The leftover strip is reported, not filled.
//!
//! The turning envelope is plane kinematics of a vehicle on a circle: with the vehicle width `W`, wheelbase `L`, front
//! overhang `F` and the radius `R` of the rear-axle centre, the inner path is `R - W/2` and the outer path (the front outer
//! corner) is `sqrt((R + W/2)^2 + (L + F)^2)`; the lane width a curve needs is their difference. This is a geometric check of
//! the dimensions entered, not a swept-path simulation with steering limits or articulated vehicles.

use crate::courtyard::Finding;
use crate::layout::Rect;
use crate::rooftop::Check;
use serde::{Deserialize, Serialize};

/// The vehicle and the curve it must negotiate, all lengths in millimetres.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct TurnSpec {
    pub vehicle_width_mm: i64,
    pub wheelbase_mm: i64,
    pub front_overhang_mm: i64,
    /// Radius of the rear-axle centre path around the curve centre.
    pub centre_radius_mm: i64,
    /// Width of the curved lane provided; `None` leaves the lane check `unchecked`.
    #[serde(default)]
    pub lane_width_mm: Option<i64>,
}

/// The area, the stall and aisle sizes and the optional limits, all lengths in millimetres.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ParkingSpec {
    /// The parking area in the frame of the bounds.
    pub area: AreaRect,
    pub stall_width_mm: i64,
    pub stall_length_mm: i64,
    pub aisle_width_mm: i64,
    #[serde(default)]
    pub min_stall_width_mm: Option<i64>,
    #[serde(default)]
    pub min_stall_length_mm: Option<i64>,
    #[serde(default)]
    pub min_aisle_width_mm: Option<i64>,
    /// Stalls the project needs (from the municipality or the brief); `None` leaves the count check `unchecked`.
    #[serde(default)]
    pub required_stalls: Option<u32>,
    #[serde(default)]
    pub turn: Option<TurnSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct AreaRect {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RowAxis {
    X,
    Y,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stall {
    pub key: String,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    pub row: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Aisle {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TurnReport {
    /// Path of the inner rear wheel.
    pub inner_radius_mm: f64,
    /// Path of the front outer corner.
    pub outer_radius_mm: f64,
    /// Lane width the curve needs: outer minus inner.
    pub swept_width_mm: f64,
    /// Lane width provided minus the width needed; negative when the lane is too narrow.
    pub lane_margin_mm: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParkingReport {
    /// The direction the stall rows run in.
    pub row_axis: RowAxis,
    pub stalls: Vec<Stall>,
    pub aisles: Vec<Aisle>,
    pub stall_count: u32,
    pub rows: u32,
    pub stalls_per_row: u32,
    pub area_m2: f64,
    pub stall_m2: f64,
    pub aisle_m2: f64,
    /// Area not taken by a stall or an aisle (the strips the packer could not fill).
    pub leftover_m2: f64,
    /// Whole area divided by the stalls, a gross figure that includes the aisles.
    pub gross_m2_per_stall: Option<f64>,
    /// Stalls the other row direction would have held.
    pub alternative_count: u32,
    pub turn: Option<TurnReport>,
    pub stall_width_check: Check,
    pub stall_length_check: Check,
    pub aisle_check: Check,
    pub count_check: Check,
    pub turn_check: Check,
    pub findings: Vec<Finding>,
}

fn at_least<T: PartialOrd>(value: Option<T>, min: Option<T>) -> Check {
    match (value, min) {
        (Some(v), Some(m)) if v >= m => Check::Pass,
        (Some(_), Some(_)) => Check::Fail,
        _ => Check::Unchecked,
    }
}

fn validate(bounds: &Rect, spec: &ParkingSpec) -> Result<(), String> {
    let a = &spec.area;
    if bounds.w <= 0 || bounds.h <= 0 {
        return Err(format!("parking: the bounds must have a positive size (got {} x {} mm)", bounds.w, bounds.h));
    }
    if a.w <= 0 || a.h <= 0 {
        return Err(format!("parking: the area must have a positive size (got {} x {} mm)", a.w, a.h));
    }
    if a.x < 0 || a.y < 0 || a.x + a.w > bounds.w || a.y + a.h > bounds.h {
        return Err(format!("parking: the area lies outside the bounds ({} x {} mm)", bounds.w, bounds.h));
    }
    for (name, v) in [("stall_width_mm", spec.stall_width_mm), ("stall_length_mm", spec.stall_length_mm), ("aisle_width_mm", spec.aisle_width_mm)] {
        if v <= 0 {
            return Err(format!("parking: {name} is required and must be positive, not {v} mm"));
        }
    }
    for (name, v) in [
        ("min_stall_width_mm", spec.min_stall_width_mm),
        ("min_stall_length_mm", spec.min_stall_length_mm),
        ("min_aisle_width_mm", spec.min_aisle_width_mm),
    ] {
        if let Some(m) = v.filter(|m| *m <= 0) {
            return Err(format!("parking: {name} must be positive, not {m} mm"));
        }
    }
    if spec.required_stalls == Some(0) {
        return Err("parking: required_stalls must be at least 1".to_string());
    }
    if let Some(t) = &spec.turn {
        for (name, v) in [
            ("vehicle_width_mm", t.vehicle_width_mm),
            ("wheelbase_mm", t.wheelbase_mm),
            ("centre_radius_mm", t.centre_radius_mm),
        ] {
            if v <= 0 {
                return Err(format!("parking: turn {name} is required and must be positive, not {v} mm"));
            }
        }
        if t.front_overhang_mm < 0 {
            return Err(format!("parking: turn front_overhang_mm must not be negative, not {} mm", t.front_overhang_mm));
        }
        if let Some(l) = t.lane_width_mm.filter(|l| *l <= 0) {
            return Err(format!("parking: turn lane_width_mm must be positive, not {l} mm"));
        }
        if 2 * t.centre_radius_mm <= t.vehicle_width_mm {
            return Err(format!(
                "parking: the centre radius ({} mm) must exceed half the vehicle width ({} mm): the inner wheel would pass through the curve centre",
                t.centre_radius_mm,
                t.vehicle_width_mm / 2
            ));
        }
    }
    Ok(())
}

/// The most stall rows that fit in `depth` with one aisle per two rows, and the stalls that fit along `across`.
fn rows_that_fit(depth: i64, across: i64, spec: &ParkingSpec) -> (u32, u32) {
    let per_row = (across / spec.stall_width_mm) as u32;
    if per_row == 0 {
        return (0, 0);
    }
    let mut rows = 0u32;
    loop {
        let next = rows as i64 + 1;
        let aisles = (next + 1) / 2;
        if next * spec.stall_length_mm + aisles * spec.aisle_width_mm > depth {
            break;
        }
        rows += 1;
    }
    (rows, per_row)
}

/// Lays out stalls and aisles in the stripe order `stall, aisle, stall` repeating, the odd last row taking its own aisle.
fn pack(spec: &ParkingSpec, axis: RowAxis, rows: u32, per_row: u32) -> (Vec<Stall>, Vec<Aisle>) {
    let a = &spec.area;
    let (sw, sl, aw) = (spec.stall_width_mm, spec.stall_length_mm, spec.aisle_width_mm);
    let mut stalls = Vec::with_capacity((rows * per_row) as usize);
    let mut aisles = Vec::new();
    let mut offset = 0i64;
    let mut row = 0u32;
    // `across` is the position along the row, `offset` the position in the depth direction.
    let put_row =|offset: i64, row: u32, stalls: &mut Vec<Stall>| {
        for i in 0..per_row {
            let across = i as i64 * sw;
            let (x, y, w, h) = match axis {
                RowAxis::X => (a.x + across, a.y + offset, sw, sl),
                RowAxis::Y => (a.x + offset, a.y + across, sl, sw),
            };
            stalls.push(Stall { key: format!("P{}", stalls.len() + 1), x, y, w, h, row });
        }
    };
    let row_len = per_row as i64 * sw;
    while row < rows {
        put_row(offset, row, &mut stalls);
        offset += sl;
        row += 1;
        let aisle = match axis {
            RowAxis::X => Aisle { x: a.x, y: a.y + offset, w: row_len, h: aw },
            RowAxis::Y => Aisle { x: a.x + offset, y: a.y, w: aw, h: row_len },
        };
        aisles.push(aisle);
        offset += aw;
        if row < rows {
            put_row(offset, row, &mut stalls);
            offset += sl;
            row += 1;
        }
    }
    (stalls, aisles)
}

fn turn_report(t: &TurnSpec) -> TurnReport {
    let (w, l, f, r) = (t.vehicle_width_mm as f64, t.wheelbase_mm as f64, t.front_overhang_mm as f64, t.centre_radius_mm as f64);
    let inner = r - w / 2.0;
    let outer = ((r + w / 2.0).powi(2) + (l + f).powi(2)).sqrt();
    let swept = outer - inner;
    TurnReport { inner_radius_mm: inner, outer_radius_mm: outer, swept_width_mm: swept, lane_margin_mm: t.lane_width_mm.map(|lane| lane as f64 - swept) }
}

/// Packs stalls into the parking area and checks the result against the limits the user entered.
pub fn plan_parking(bounds: &Rect, spec: &ParkingSpec) -> Result<ParkingReport, String> {
    validate(bounds, spec)?;
    let a = &spec.area;
    let (rx, px) = rows_that_fit(a.h, a.w, spec);
    let (ry, py) = rows_that_fit(a.w, a.h, spec);
    let (count_x, count_y) = (rx * px, ry * py);
    let (axis, rows, per_row, alternative_count) = if count_y > count_x { (RowAxis::Y, ry, py, count_x) } else { (RowAxis::X, rx, px, count_y) };
    let (stalls, aisles) = pack(spec, axis, rows, per_row);
    let stall_count = stalls.len() as u32;

    let area_mm2 = a.w as i128 * a.h as i128;
    let stall_mm2 = stall_count as i128 * spec.stall_width_mm as i128 * spec.stall_length_mm as i128;
    let aisle_mm2: i128 = aisles.iter().map(|s| s.w as i128 * s.h as i128).sum();
    let turn = spec.turn.as_ref().map(turn_report);

    let stall_width_check = at_least(Some(spec.stall_width_mm), spec.min_stall_width_mm);
    let stall_length_check = at_least(Some(spec.stall_length_mm), spec.min_stall_length_mm);
    let aisle_check = at_least(Some(spec.aisle_width_mm), spec.min_aisle_width_mm);
    let count_check = at_least(Some(stall_count), spec.required_stalls);
    let turn_check = match turn.and_then(|t| t.lane_margin_mm) {
        Some(m) if m >= 0.0 => Check::Pass,
        Some(_) => Check::Fail,
        None => Check::Unchecked,
    };

    let mut findings = Vec::new();
    if let (Check::Fail, Some(m)) = (stall_width_check, spec.min_stall_width_mm) {
        findings.push(Finding {
            ar: format!("عرض الموقف {} مم وهو أقل من الحد الأدنى {m} مم.", spec.stall_width_mm),
            en: format!("The stall width is {} mm, below the {m} mm minimum.", spec.stall_width_mm),
        });
    }
    if let (Check::Fail, Some(m)) = (stall_length_check, spec.min_stall_length_mm) {
        findings.push(Finding {
            ar: format!("طول الموقف {} مم وهو أقل من الحد الأدنى {m} مم.", spec.stall_length_mm),
            en: format!("The stall length is {} mm, below the {m} mm minimum.", spec.stall_length_mm),
        });
    }
    if let (Check::Fail, Some(m)) = (aisle_check, spec.min_aisle_width_mm) {
        findings.push(Finding {
            ar: format!("عرض الممر {} مم وهو أقل من الحد الأدنى {m} مم.", spec.aisle_width_mm),
            en: format!("The aisle is {} mm wide, below the {m} mm minimum.", spec.aisle_width_mm),
        });
    }
    if let (Check::Fail, Some(m)) = (count_check, spec.required_stalls) {
        findings.push(Finding {
            ar: format!("المساحة تتسع لـ {stall_count} موقفاً فقط والمطلوب {m}."),
            en: format!("The area holds {stall_count} stall(s), fewer than the {m} required."),
        });
    }
    if let (Check::Fail, Some(t)) = (turn_check, turn) {
        let lane = spec.turn.and_then(|s| s.lane_width_mm).unwrap_or_default();
        findings.push(Finding {
            ar: format!("المنعطف يحتاج حارة بعرض {:.0} مم والمتاح {lane} مم.", t.swept_width_mm),
            en: format!("The curve needs a {:.0} mm lane, but {lane} mm is provided.", t.swept_width_mm),
        });
    }
    if stall_count == 0 {
        findings.push(Finding {
            ar: "لا يتسع الموقف الواحد مع ممره في هذه المساحة: لم يوضع أي موقف.".to_string(),
            en: "Not even one stall with its aisle fits in this area: no stall was placed.".to_string(),
        });
    }
    for (check, ar, en) in [
        (stall_width_check, "عرض الموقف (يلزم حد أدنى)", "the stall width (a minimum is needed)"),
        (stall_length_check, "طول الموقف (يلزم حد أدنى)", "the stall length (a minimum is needed)"),
        (aisle_check, "عرض الممر (يلزم حد أدنى)", "the aisle width (a minimum is needed)"),
        (count_check, "عدد المواقف (يلزم العدد المطلوب)", "the stall count (the required number is needed)"),
        (turn_check, "المنعطف (يلزم مركبة وعرض حارة معاً)", "the curve (both a vehicle and a lane width are needed)"),
    ] {
        if check == Check::Unchecked {
            findings.push(Finding { ar: format!("لم يُدخَل ما يلزم لفحص {ar}: لم يُفحص."), en: format!("Nothing was entered to check {en}: not checked.") });
        }
    }
    Ok(ParkingReport {
        row_axis: axis,
        stalls,
        aisles,
        stall_count,
        rows,
        stalls_per_row: per_row,
        area_m2: area_mm2 as f64 / 1e6,
        stall_m2: stall_mm2 as f64 / 1e6,
        aisle_m2: aisle_mm2 as f64 / 1e6,
        leftover_m2: (area_mm2 - stall_mm2 - aisle_mm2) as f64 / 1e6,
        gross_m2_per_stall: (stall_count > 0).then(|| area_mm2 as f64 / 1e6 / stall_count as f64),
        alternative_count,
        turn,
        stall_width_check,
        stall_length_check,
        aisle_check,
        count_check,
        turn_check,
        findings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: Rect = Rect { x: 0, y: 0, w: 40000, h: 40000 };

    fn spec(w: i64, h: i64) -> ParkingSpec {
        ParkingSpec {
            area: AreaRect { x: 1000, y: 2000, w, h },
            stall_width_mm: 2500,
            stall_length_mm: 5000,
            aisle_width_mm: 6000,
            min_stall_width_mm: None,
            min_stall_length_mm: None,
            min_aisle_width_mm: None,
            required_stalls: None,
            turn: None,
        }
    }

    fn overlap(a: (i64, i64, i64, i64), b: (i64, i64, i64, i64)) -> bool {
        a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
    }

    #[test]
    fn a_double_loaded_module_packs_the_exact_count() {
        // Depth 16000 = 5000 + 6000 + 5000: two rows and one aisle; 15000 / 2500 = 6 stalls a row.
        let r = plan_parking(&BOUNDS, &spec(15000, 16000)).unwrap();
        assert_eq!((r.row_axis, r.rows, r.stalls_per_row, r.stall_count), (RowAxis::X, 2, 6, 12));
        assert_eq!(r.aisles.len(), 1);
        assert_eq!(r.aisles[0], Aisle { x: 1000, y: 7000, w: 15000, h: 6000 });
        assert!((r.stall_m2 - 150.0).abs() < 1e-9);
        assert!((r.aisle_m2 - 90.0).abs() < 1e-9);
        assert!((r.leftover_m2 - 0.0).abs() < 1e-9);
        assert!((r.gross_m2_per_stall.unwrap() - 20.0).abs() < 1e-9);
        assert_eq!(r.stalls[0], Stall { key: "P1".into(), x: 1000, y: 2000, w: 2500, h: 5000, row: 0 });
        assert_eq!(r.stalls[6].row, 1);
        // Second row: below the first row (5000) and the aisle (6000).
        assert_eq!(r.stalls[6].y, 2000 + 5000 + 6000);
    }

    #[test]
    fn an_odd_last_row_takes_its_own_aisle_and_rows_never_overlap() {
        // Depth 27000: three rows need 3*5000 + 2*6000 = 27000 exactly.
        let r = plan_parking(&BOUNDS, &spec(10000, 27000)).unwrap();
        assert_eq!((r.rows, r.stalls_per_row, r.stall_count, r.aisles.len()), (3, 4, 12, 2));
        let mut boxes: Vec<(i64, i64, i64, i64)> = r.stalls.iter().map(|s| (s.x, s.y, s.w, s.h)).collect();
        boxes.extend(r.aisles.iter().map(|a| (a.x, a.y, a.w, a.h)));
        for (i, a) in boxes.iter().enumerate() {
            assert!(a.0 >= 1000 && a.1 >= 2000 && a.0 + a.2 <= 11000 && a.1 + a.3 <= 29000, "inside the area: {a:?}");
            for b in &boxes[i + 1..] {
                assert!(!overlap(*a, *b), "{a:?} overlaps {b:?}");
            }
        }
        // 26999 fits only two rows.
        assert_eq!(plan_parking(&BOUNDS, &spec(10000, 26999)).unwrap().rows, 2);
    }

    #[test]
    fn the_better_row_direction_wins_and_the_other_is_reported() {
        // Wide and shallow: rows along x need 11000 of depth and 8000 is not enough; rows along y give two rows in 16000.
        let r = plan_parking(&BOUNDS, &spec(16000, 8000)).unwrap();
        assert_eq!(r.row_axis, RowAxis::Y);
        assert_eq!((r.rows, r.stalls_per_row, r.stall_count), (2, 3, 6));
        assert_eq!(r.alternative_count, 0);
        // Y rows: stalls are 5000 wide (x) and 2500 tall (y).
        assert_eq!((r.stalls[0].w, r.stalls[0].h), (5000, 2500));
        assert_eq!(r.aisles[0], Aisle { x: 6000, y: 2000, w: 6000, h: 7500 });
        // A tie keeps x.
        assert_eq!(plan_parking(&BOUNDS, &spec(16000, 16000)).unwrap().row_axis, RowAxis::X);
    }

    #[test]
    fn an_area_too_small_for_one_stall_and_aisle_places_nothing() {
        // One row plus its aisle needs 11000: at 11000 rows run along x (6 stalls), at 10999 only along y (4 stalls).
        let fits = plan_parking(&BOUNDS, &spec(15000, 11000)).unwrap();
        assert_eq!((fits.row_axis, fits.stall_count), (RowAxis::X, 6));
        let short = plan_parking(&BOUNDS, &spec(15000, 10999)).unwrap();
        assert_eq!((short.row_axis, short.stall_count, short.alternative_count), (RowAxis::Y, 4, 0));
        let none = plan_parking(&BOUNDS, &spec(2000, 2000)).unwrap();
        assert_eq!((none.stall_count, none.gross_m2_per_stall), (0, None));
        assert!(none.findings.iter().any(|f| f.en.contains("no stall was placed")));
        assert!((none.leftover_m2 - 4.0).abs() < 1e-9);
    }

    #[test]
    fn limits_pass_fail_or_stay_unchecked() {
        let none = plan_parking(&BOUNDS, &spec(15000, 16000)).unwrap();
        assert_eq!(
            (none.stall_width_check, none.stall_length_check, none.aisle_check, none.count_check, none.turn_check),
            (Check::Unchecked, Check::Unchecked, Check::Unchecked, Check::Unchecked, Check::Unchecked)
        );
        assert_eq!(none.findings.iter().filter(|f| f.en.contains("not checked")).count(), 5);
        let tight = ParkingSpec {
            min_stall_width_mm: Some(2700),
            min_stall_length_mm: Some(5500),
            min_aisle_width_mm: Some(6500),
            required_stalls: Some(20),
            ..spec(15000, 16000)
        };
        let r = plan_parking(&BOUNDS, &tight).unwrap();
        assert_eq!((r.stall_width_check, r.stall_length_check, r.aisle_check, r.count_check), (Check::Fail, Check::Fail, Check::Fail, Check::Fail));
        assert!(r.findings.iter().any(|f| f.en.contains("stall width is 2500 mm, below the 2700 mm")));
        assert!(r.findings.iter().any(|f| f.en.contains("stall length is 5000 mm, below the 5500 mm")));
        assert!(r.findings.iter().any(|f| f.en.contains("aisle is 6000 mm wide, below the 6500 mm")));
        assert!(r.findings.iter().any(|f| f.en.contains("12 stall(s), fewer than the 20")));
        let ok = ParkingSpec {
            min_stall_width_mm: Some(2500),
            min_stall_length_mm: Some(5000),
            min_aisle_width_mm: Some(6000),
            required_stalls: Some(12),
            ..spec(15000, 16000)
        };
        let r = plan_parking(&BOUNDS, &ok).unwrap();
        assert_eq!((r.stall_width_check, r.stall_length_check, r.aisle_check, r.count_check), (Check::Pass, Check::Pass, Check::Pass, Check::Pass));
    }

    fn car(lane: Option<i64>) -> TurnSpec {
        TurnSpec { vehicle_width_mm: 1800, wheelbase_mm: 2700, front_overhang_mm: 900, centre_radius_mm: 5000, lane_width_mm: lane }
    }

    #[test]
    fn the_turning_envelope_follows_the_kinematic_formula() {
        let r = plan_parking(&BOUNDS, &ParkingSpec { turn: Some(car(Some(4000))), ..spec(15000, 16000) }).unwrap();
        let t = r.turn.unwrap();
        // Inner = 5000 - 900; outer = sqrt(5900^2 + 3600^2).
        assert!((t.inner_radius_mm - 4100.0).abs() < 1e-9);
        let outer = (5900.0f64 * 5900.0 + 3600.0 * 3600.0).sqrt();
        assert!((t.outer_radius_mm - outer).abs() < 1e-9);
        assert!((t.swept_width_mm - (outer - 4100.0)).abs() < 1e-9);
        // About 2.81 m of swept width: a 4 m lane passes, a 2 m lane fails.
        assert!(t.swept_width_mm > 2800.0 && t.swept_width_mm < 2830.0, "{}", t.swept_width_mm);
        assert_eq!(r.turn_check, Check::Pass);
        assert!((t.lane_margin_mm.unwrap() - (4000.0 - t.swept_width_mm)).abs() < 1e-9);
        let narrow = plan_parking(&BOUNDS, &ParkingSpec { turn: Some(car(Some(2000))), ..spec(15000, 16000) }).unwrap();
        assert_eq!(narrow.turn_check, Check::Fail);
        assert!(narrow.findings.iter().any(|f| f.en.contains("The curve needs a 2") && f.en.contains("but 2000 mm is provided")));
        // A vehicle with no lane width reports the envelope but leaves the lane check unchecked.
        let open = plan_parking(&BOUNDS, &ParkingSpec { turn: Some(car(None)), ..spec(15000, 16000) }).unwrap();
        assert!(open.turn.is_some());
        assert_eq!(open.turn_check, Check::Unchecked);
    }

    #[test]
    fn invalid_input_is_refused_with_the_reason() {
        let e = |s: ParkingSpec| plan_parking(&BOUNDS, &s).unwrap_err();
        assert!(e(ParkingSpec { area: AreaRect { x: 0, y: 0, w: 0, h: 10 }, ..spec(1, 1) }).contains("area must have a positive size"));
        assert!(e(ParkingSpec { area: AreaRect { x: 30000, y: 0, w: 15000, h: 1000 }, ..spec(1, 1) }).contains("outside the bounds"));
        assert!(e(ParkingSpec { stall_width_mm: 0, ..spec(15000, 16000) }).contains("stall_width_mm is required"));
        assert!(e(ParkingSpec { aisle_width_mm: -1, ..spec(15000, 16000) }).contains("aisle_width_mm is required"));
        assert!(e(ParkingSpec { min_aisle_width_mm: Some(0), ..spec(15000, 16000) }).contains("min_aisle_width_mm must be positive"));
        assert!(e(ParkingSpec { required_stalls: Some(0), ..spec(15000, 16000) }).contains("required_stalls"));
        let bad = |t: TurnSpec| e(ParkingSpec { turn: Some(t), ..spec(15000, 16000) });
        assert!(bad(TurnSpec { centre_radius_mm: 900, ..car(None) }).contains("must exceed half the vehicle width"));
        assert!(bad(TurnSpec { wheelbase_mm: 0, ..car(None) }).contains("wheelbase_mm is required"));
        assert!(bad(TurnSpec { front_overhang_mm: -1, ..car(None) }).contains("front_overhang_mm"));
        assert!(bad(car(Some(0))).contains("lane_width_mm"));
    }
}
