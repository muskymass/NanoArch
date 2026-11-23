//! Privacy between the openings of private rooms and the neighbouring plots.
//!
//! A window in an outside wall that faces a neighbour looks across the boundary. This module finds, on a placed level,
//! every privacy-sensitive room whose outside wall lies on a side that faces a neighbour, and compares the distance from
//! that wall to the boundary with the minimum clear distance the user enters. It changes nothing.
//!
//! Inputs that are never invented:
//! - the **sides facing a neighbour**, each with the distance (mm) from the building envelope to that boundary (the
//!   setback the municipality or the plot survey gives; the caller states its source);
//! - the **privacy-sensitive room functions** (the backend passes the user's list);
//! - the **minimum clear distance** for an opening that faces a neighbour. It is a municipal or design value, so without
//!   one the distances are reported and the pass/fail check is skipped, never passed.
//!
//! The result is a geometric preliminary check of walls, not of individual windows (the layout holds rooms, not openings).

use crate::layout::{Layout, Rect, Side};
use crate::scoring::on_street_side;
use serde::Serialize;

/// One side of the envelope that faces a neighbour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NeighbourSide {
    pub side: Side,
    /// Distance (mm) from the envelope wall on this side to the neighbour's boundary.
    pub distance_mm: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyStatus {
    /// No privacy-sensitive room has an outside wall on a neighbour side.
    NotApplicable,
    /// Distances are reported but no minimum was entered, so the check did not run.
    Unchecked,
    /// Every exposed wall is at least the minimum from its boundary.
    Clear,
    /// At least one exposed wall is closer than the minimum.
    Exposed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExposureStatus {
    Unchecked,
    Clear,
    TooClose,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Exposure {
    pub room: String,
    /// `south`, `north`, `east` or `west`.
    pub side: String,
    pub distance_mm: i64,
    pub status: ExposureStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub ar: String,
    pub en: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PrivacyReport {
    pub status: PrivacyStatus,
    pub exposures: Vec<Exposure>,
    pub min_distance_mm: Option<i64>,
    pub findings: Vec<Finding>,
}

fn side_name(s: Side) -> &'static str {
    match s {
        Side::South => "south",
        Side::North => "north",
        Side::East => "east",
        Side::West => "west",
    }
}

fn side_ar(s: Side) -> &'static str {
    match s {
        Side::South => "الجنوبية",
        Side::North => "الشمالية",
        Side::East => "الشرقية",
        Side::West => "الغربية",
    }
}

/// Checks one level. `neighbours` lists only the sides that face a neighbour (the street side is left out); a side may
/// appear once and its distance must not be negative. `min_distance_mm` must be positive when given.
pub fn check_privacy(
    layout: &Layout,
    envelope: &Rect,
    neighbours: &[NeighbourSide],
    private_funcs: &[String],
    min_distance_mm: Option<i64>,
) -> Result<PrivacyReport, String> {
    if let Some(m) = min_distance_mm.filter(|&m| m <= 0) {
        return Err(format!("the minimum clear distance for an opening facing a neighbour must be above zero, not {m} mm"));
    }
    for (i, n) in neighbours.iter().enumerate() {
        if n.distance_mm < 0 {
            return Err(format!("the distance to the {} boundary must not be negative, not {} mm", side_name(n.side), n.distance_mm));
        }
        if neighbours[..i].iter().any(|p| p.side == n.side) {
            return Err(format!("the {} side is listed twice", side_name(n.side)));
        }
    }
    if private_funcs.is_empty() {
        return Err("name at least one privacy-sensitive room function, otherwise there is nothing to check".to_string());
    }
    let mut exposures = Vec::new();
    for room in layout.rooms.iter().filter(|r| private_funcs.iter().any(|f| f == &r.func)) {
        for n in neighbours.iter().filter(|n| on_street_side(&room.rect, envelope, n.side)) {
            let status = match min_distance_mm {
                None => ExposureStatus::Unchecked,
                Some(m) if n.distance_mm >= m => ExposureStatus::Clear,
                Some(_) => ExposureStatus::TooClose,
            };
            exposures.push(Exposure { room: room.key.clone(), side: side_name(n.side).to_string(), distance_mm: n.distance_mm, status });
        }
    }
    let say = |ar: String, en: String| Finding { ar, en };
    let mut report = PrivacyReport { status: PrivacyStatus::NotApplicable, exposures, min_distance_mm, findings: Vec::new() };
    if report.exposures.is_empty() {
        report.findings.push(say(
            "لا توجد غرفة حساسة للخصوصية بجدار خارجي على جهة جار، فلا حاجة للفحص.".to_string(),
            "No privacy-sensitive room has an outside wall on a side that faces a neighbour, so no check is needed.".to_string(),
        ));
        return Ok(report);
    }
    let Some(min) = min_distance_mm else {
        report.status = PrivacyStatus::Unchecked;
        report.findings.push(say(
            format!("{} غرفة حساسة للخصوصية تواجه جاراً؛ لم يُدخل حد أدنى للمسافة فلم يجرِ الفحص.", report.exposures.len()),
            format!("{} privacy-sensitive wall(s) face a neighbour; no minimum clear distance was entered, so the check did not run.", report.exposures.len()),
        ));
        return Ok(report);
    };
    let close: Vec<&Exposure> = report.exposures.iter().filter(|e| e.status == ExposureStatus::TooClose).collect();
    let mut findings = Vec::new();
    for e in &close {
        let side = [Side::South, Side::North, Side::East, Side::West].into_iter().find(|s| side_name(*s) == e.side).map(side_ar).unwrap_or("");
        findings.push(say(
            format!("الغرفة «{}» على الجهة {} تبعد {} مم عن حد الجار وهي دون الحد الأدنى المُدخل {} مم؛ أبعد الفتحات أو أغلقها أو ارفع أسفلها.", e.room, side, e.distance_mm, min),
            format!("Room '{}' on the {} side is {} mm from the neighbour boundary, below the entered minimum {} mm; move the openings, close them or raise their sills.", e.room, e.side, e.distance_mm, min),
        ));
    }
    if close.is_empty() {
        report.status = PrivacyStatus::Clear;
        findings.push(say(
            format!("كل الجدران الحساسة المواجهة لجار تبعد عن حده {min} مم فأكثر (الحد الأدنى المُدخل)."),
            format!("Every sensitive wall facing a neighbour is at least {min} mm from the boundary (entered minimum)."),
        ));
    } else {
        report.status = PrivacyStatus::Exposed;
    }
    report.findings = findings;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Breakdown, PlacedRoom, Zone};

    fn room(key: &str, func: &str, at: [i64; 4]) -> PlacedRoom {
        let rect = Rect { x: at[0], y: at[1], w: at[2], h: at[3] };
        PlacedRoom { key: key.into(), func: func.into(), zone: Zone::Private, needs_light: true, rect, area_m2: rect.area_m2(), target_m2: rect.area_m2(), deviation_pct: 0.0 }
    }
    fn layout(rooms: Vec<PlacedRoom>) -> Layout {
        Layout { rooms, cost: 0.0, breakdown: Breakdown::default(), reserve_m2: 0.0 }
    }
    const ENV: Rect = Rect { x: 0, y: 0, w: 12_000, h: 10_000 };
    fn beds() -> Vec<String> {
        vec!["bedroom".into()]
    }
    fn l() -> Layout {
        layout(vec![room("bed1", "bedroom", [0, 0, 6000, 10_000]), room("hall", "hall", [6000, 0, 6000, 10_000])])
    }

    #[test]
    fn a_bedroom_wall_closer_than_the_minimum_is_exposed() {
        let n = [NeighbourSide { side: Side::West, distance_mm: 1500 }];
        let r = check_privacy(&l(), &ENV, &n, &beds(), Some(3000)).unwrap();
        assert_eq!(r.status, PrivacyStatus::Exposed);
        assert_eq!(r.exposures, vec![Exposure { room: "bed1".into(), side: "west".into(), distance_mm: 1500, status: ExposureStatus::TooClose }]);
        assert!(r.findings[0].en.contains("below the entered minimum") && r.findings[0].ar.contains("الغربية"));
    }

    #[test]
    fn a_wall_at_or_beyond_the_minimum_is_clear() {
        let n = [NeighbourSide { side: Side::West, distance_mm: 3000 }];
        let r = check_privacy(&l(), &ENV, &n, &beds(), Some(3000)).unwrap();
        assert_eq!(r.status, PrivacyStatus::Clear);
    }

    #[test]
    fn without_a_minimum_nothing_is_passed() {
        let n = [NeighbourSide { side: Side::West, distance_mm: 9000 }];
        let r = check_privacy(&l(), &ENV, &n, &beds(), None).unwrap();
        assert_eq!(r.status, PrivacyStatus::Unchecked);
        assert_eq!(r.exposures[0].status, ExposureStatus::Unchecked);
    }

    #[test]
    fn rooms_off_a_neighbour_side_or_not_sensitive_are_not_applicable() {
        // The bedroom is on the west; only the east faces a neighbour, and the hall there is not sensitive.
        let n = [NeighbourSide { side: Side::East, distance_mm: 500 }];
        let r = check_privacy(&l(), &ENV, &n, &beds(), Some(3000)).unwrap();
        assert_eq!(r.status, PrivacyStatus::NotApplicable);
        assert!(r.exposures.is_empty());
    }

    #[test]
    fn a_corner_room_is_checked_on_each_neighbour_side() {
        let n = [NeighbourSide { side: Side::West, distance_mm: 4000 }, NeighbourSide { side: Side::South, distance_mm: 1000 }];
        let r = check_privacy(&l(), &ENV, &n, &beds(), Some(3000)).unwrap();
        assert_eq!(r.exposures.len(), 2);
        assert_eq!(r.status, PrivacyStatus::Exposed);
        assert_eq!(r.exposures.iter().filter(|e| e.status == ExposureStatus::TooClose).count(), 1);
    }

    #[test]
    fn bad_inputs_are_rejected_by_name() {
        let n = [NeighbourSide { side: Side::West, distance_mm: 1000 }];
        assert!(check_privacy(&l(), &ENV, &n, &beds(), Some(0)).unwrap_err().contains("above zero"));
        assert!(check_privacy(&l(), &ENV, &[NeighbourSide { side: Side::West, distance_mm: -1 }], &beds(), None).unwrap_err().contains("negative"));
        let dup = [n[0], n[0]];
        assert!(check_privacy(&l(), &ENV, &dup, &beds(), None).unwrap_err().contains("twice"));
        assert!(check_privacy(&l(), &ENV, &n, &[], None).unwrap_err().contains("privacy-sensitive"));
    }
}
