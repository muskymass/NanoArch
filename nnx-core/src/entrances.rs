//! Guest entrance versus family entrance on the street side of a level.
//!
//! In a Saudi house the guests (majlis) come in without crossing the family space, and the family door is apart from
//! the guest door. This module checks that on a placed layout and reports what it finds; it changes nothing.
//!
//! What is measured, all on the street side of the envelope:
//! - A **guest room** is a room whose function is in the caller's `guest_funcs` list (the backend passes `majlis` unless
//!   the user names others). A door can go on its street wall when that wall stretch is at least [`DOOR_STRETCH_MM`] long.
//! - A **family entrance room** is any other room with such a street stretch whose zone is not `Service`.
//! - The **separation** is the largest distance along the street wall that two doors can be given, one in a guest stretch
//!   and one in a family stretch, each kept half a door stretch from the ends of its own stretch.
//!
//! The minimum separation is a design or municipal value, so it is an input with a stated source. Without one the
//! distance is reported and the pass/fail check is skipped, never passed. The result is a geometric preliminary check.

use crate::layout::{Layout, PlacedRoom, Rect, Side, Zone};
use crate::scoring::{ACCESS_MIN_MM, on_street_side};
use serde::Serialize;

/// A door plus its margins: the shortest street wall stretch a door can go on (the figure the scores use for access).
pub const DOOR_STRETCH_MM: i64 = ACCESS_MIN_MM;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntranceStatus {
    /// The program has no guest room, so there is nothing to separate.
    NotApplicable,
    /// No guest room has a street wall stretch long enough for a door: guests would have to cross the house.
    GuestThroughHouse,
    /// No non-service, non-guest room has a street stretch long enough for the family door.
    NoFamilyEntrance,
    /// The doors can be kept at least the minimum apart.
    Separated,
    /// The best placement of the two doors is still closer than the minimum.
    TooClose,
    /// The distance is reported but no minimum was entered, so the check did not run.
    Unchecked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    pub room: String,
    /// Stretch of the street wall (mm along the wall, envelope frame) the room occupies.
    pub from_mm: i64,
    pub to_mm: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub ar: String,
    pub en: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EntranceReport {
    pub status: EntranceStatus,
    pub guest_rooms: Vec<String>,
    pub guest_candidates: Vec<Candidate>,
    pub family_candidates: Vec<Candidate>,
    /// Largest achievable distance between the two doors (mm); `None` when either door cannot be placed.
    pub separation_mm: Option<i64>,
    pub min_separation_mm: Option<i64>,
    /// The room pair (guest, family) that gives `separation_mm`.
    pub best_pair: Option<(String, String)>,
    pub findings: Vec<Finding>,
}

fn stretch(r: &Rect, side: Side) -> (i64, i64) {
    match side {
        Side::South | Side::North => (r.x, r.x + r.w),
        Side::East | Side::West => (r.y, r.y + r.h),
    }
}

fn candidates<'a>(rooms: impl Iterator<Item = &'a PlacedRoom>, envelope: &Rect, side: Side) -> Vec<Candidate> {
    rooms
        .filter(|r| on_street_side(&r.rect, envelope, side))
        .map(|r| {
            let (a, b) = stretch(&r.rect, side);
            Candidate { room: r.key.clone(), from_mm: a, to_mm: b }
        })
        .filter(|c| c.to_mm - c.from_mm >= DOOR_STRETCH_MM)
        .collect()
}

/// Largest distance between a door in `g` and a door in `f`, each door centre kept half a door stretch inside its stretch.
fn best_gap(g: &Candidate, f: &Candidate) -> i64 {
    let half = DOOR_STRETCH_MM / 2;
    let (glo, ghi) = (g.from_mm + half, g.to_mm - half);
    let (flo, fhi) = (f.from_mm + half, f.to_mm - half);
    (glo - fhi).abs().max((ghi - flo).abs())
}

/// Checks one level. `min_separation_mm` must be positive when given.
pub fn check_entrances(layout: &Layout, envelope: &Rect, street: Side, guest_funcs: &[String], min_separation_mm: Option<i64>) -> Result<EntranceReport, String> {
    if let Some(m) = min_separation_mm.filter(|&m| m <= 0) {
        return Err(format!("the minimum separation between the guest door and the family door must be above zero, not {m} mm"));
    }
    let is_guest = |r: &PlacedRoom| guest_funcs.iter().any(|f| f == &r.func);
    let guest_rooms: Vec<String> = layout.rooms.iter().filter(|r| is_guest(r)).map(|r| r.key.clone()).collect();
    let guest_candidates = candidates(layout.rooms.iter().filter(|r| is_guest(r)), envelope, street);
    let family_candidates = candidates(layout.rooms.iter().filter(|r| !is_guest(r) && r.zone != Zone::Service), envelope, street);
    let mut report = EntranceReport {
        status: EntranceStatus::NotApplicable,
        guest_rooms,
        guest_candidates,
        family_candidates,
        separation_mm: None,
        min_separation_mm,
        best_pair: None,
        findings: Vec::new(),
    };
    let say = |ar: &str, en: &str| Finding { ar: ar.to_string(), en: en.to_string() };
    if report.guest_rooms.is_empty() {
        report.findings.push(say("لا توجد غرفة ضيوف في البرنامج، فلا حاجة لفصل المداخل.", "The program has no guest room, so no entrance separation is needed."));
        return Ok(report);
    }
    if report.guest_candidates.is_empty() {
        report.status = EntranceStatus::GuestThroughHouse;
        report.findings.push(say(
            "لا تملك أي غرفة ضيوف جداراً على جهة الشارع بطول يتسع لباب، فسيمر الضيوف عبر مساحات الأسرة.",
            "No guest room has a street wall long enough for a door, so guests would have to pass through the family space.",
        ));
        return Ok(report);
    }
    if report.family_candidates.is_empty() {
        report.status = EntranceStatus::NoFamilyEntrance;
        report.findings.push(say(
            "لا توجد غرفة غير غرف الضيوف على جهة الشارع بطول يتسع لباب الأسرة.",
            "No room other than the guest rooms has a street wall long enough for the family door.",
        ));
        return Ok(report);
    }
    let best = report
        .guest_candidates
        .iter()
        .flat_map(|g| report.family_candidates.iter().map(move |f| (best_gap(g, f), g, f)))
        .max_by_key(|(gap, ..)| *gap)
        .map(|(gap, g, f)| (gap, g.room.clone(), f.room.clone()));
    let Some((gap, g, f)) = best else {
        return Ok(report);
    };
    report.separation_mm = Some(gap);
    report.best_pair = Some((g, f));
    match min_separation_mm {
        None => {
            report.status = EntranceStatus::Unchecked;
            report.findings.push(say(
                &format!("أبعد مسافة ممكنة بين بابي الضيوف والأسرة {gap} مم؛ لم يُدخل حد أدنى للفصل فلم يجرِ الفحص."),
                &format!("The largest possible distance between the guest door and the family door is {gap} mm; no minimum separation was entered, so the check did not run."),
            ));
        }
        Some(min) if gap >= min => {
            report.status = EntranceStatus::Separated;
            report.findings.push(say(
                &format!("يمكن فصل بابي الضيوف والأسرة بمسافة {gap} مم (الحد الأدنى المُدخل {min} مم)."),
                &format!("The guest door and the family door can be {gap} mm apart (entered minimum {min} mm)."),
            ));
        }
        Some(min) => {
            report.status = EntranceStatus::TooClose;
            report.findings.push(say(
                &format!("أبعد مسافة ممكنة بين البابين {gap} مم وهي دون الحد الأدنى المُدخل {min} مم؛ غيّر موضع غرفة الضيوف أو غرفة مدخل الأسرة."),
                &format!("The largest possible distance between the doors is {gap} mm, below the entered minimum {min} mm; move the guest room or the family entrance room."),
            ));
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Breakdown;

    fn room(key: &str, func: &str, zone: Zone, at: [i64; 4]) -> PlacedRoom {
        let rect = Rect { x: at[0], y: at[1], w: at[2], h: at[3] };
        PlacedRoom { key: key.into(), func: func.into(), zone, needs_light: false, rect, area_m2: rect.area_m2(), target_m2: rect.area_m2(), deviation_pct: 0.0 }
    }
    fn layout(rooms: Vec<PlacedRoom>) -> Layout {
        Layout { rooms, cost: 0.0, breakdown: Breakdown::default(), reserve_m2: 0.0 }
    }
    const ENV: Rect = Rect { x: 0, y: 0, w: 12_000, h: 10_000 };
    fn guests() -> Vec<String> {
        vec!["majlis".into()]
    }

    #[test]
    fn doors_at_opposite_ends_of_the_street_wall_are_separated() {
        let l = layout(vec![
            room("majlis", "majlis", Zone::Public, [0, 0, 5000, 4000]),
            room("hall", "hall", Zone::Public, [5000, 0, 7000, 4000]),
            room("bed", "bedroom", Zone::Private, [0, 4000, 12_000, 6000]),
        ]);
        let r = check_entrances(&l, &ENV, Side::South, &guests(), Some(6000)).unwrap();
        // Guest door centre in [600, 4400], family door centre in [5600, 11400]: farthest apart 11400 - 600.
        assert_eq!(r.separation_mm, Some(10_800));
        assert_eq!(r.status, EntranceStatus::Separated);
        assert_eq!(r.best_pair, Some(("majlis".to_string(), "hall".to_string())));
    }

    #[test]
    fn a_minimum_that_cannot_be_met_is_reported_as_too_close() {
        let l = layout(vec![
            room("majlis", "majlis", Zone::Public, [0, 0, 2000, 4000]),
            room("hall", "hall", Zone::Public, [2000, 0, 2000, 4000]),
            room("back", "kitchen", Zone::Service, [0, 4000, 4000, 6000]),
        ]);
        let r = check_entrances(&l, &ENV, Side::South, &guests(), Some(5000)).unwrap();
        // Guest centre in [600, 1400], family centre in [2600, 3400]: farthest apart 3400 - 600.
        assert_eq!(r.separation_mm, Some(2800));
        assert_eq!(r.status, EntranceStatus::TooClose);
        assert!(r.findings[0].en.contains("below the entered minimum") && r.findings[0].ar.contains("دون"));
    }

    #[test]
    fn without_a_minimum_the_distance_is_reported_but_nothing_is_passed() {
        let l = layout(vec![room("majlis", "majlis", Zone::Public, [0, 0, 3000, 4000]), room("hall", "hall", Zone::Public, [3000, 0, 9000, 4000])]);
        let r = check_entrances(&l, &ENV, Side::South, &guests(), None).unwrap();
        assert_eq!(r.status, EntranceStatus::Unchecked);
        assert!(r.separation_mm.is_some());
    }

    #[test]
    fn a_guest_room_off_the_street_means_guests_cross_the_house() {
        let l = layout(vec![room("hall", "hall", Zone::Public, [0, 0, 12_000, 4000]), room("majlis", "majlis", Zone::Public, [0, 4000, 6000, 6000])]);
        let r = check_entrances(&l, &ENV, Side::South, &guests(), Some(3000)).unwrap();
        assert_eq!(r.status, EntranceStatus::GuestThroughHouse);
        assert!(r.separation_mm.is_none());
    }

    #[test]
    fn a_stretch_too_short_for_a_door_is_not_a_candidate_and_service_rooms_do_not_count() {
        let l = layout(vec![room("majlis", "majlis", Zone::Public, [0, 0, 1000, 4000]), room("kitchen", "kitchen", Zone::Service, [1000, 0, 11_000, 4000])]);
        assert_eq!(check_entrances(&l, &ENV, Side::South, &guests(), None).unwrap().status, EntranceStatus::GuestThroughHouse);
        let l = layout(vec![room("majlis", "majlis", Zone::Public, [0, 0, 6000, 4000]), room("kitchen", "kitchen", Zone::Service, [6000, 0, 6000, 4000])]);
        assert_eq!(check_entrances(&l, &ENV, Side::South, &guests(), None).unwrap().status, EntranceStatus::NoFamilyEntrance);
    }

    #[test]
    fn no_guest_room_is_not_applicable_and_a_bad_minimum_is_rejected() {
        let l = layout(vec![room("hall", "hall", Zone::Public, [0, 0, 12_000, 10_000])]);
        assert_eq!(check_entrances(&l, &ENV, Side::South, &guests(), None).unwrap().status, EntranceStatus::NotApplicable);
        assert!(check_entrances(&l, &ENV, Side::South, &guests(), Some(0)).unwrap_err().contains("above zero"));
    }

    #[test]
    fn the_street_side_decides_the_axis() {
        let l = layout(vec![room("majlis", "majlis", Zone::Public, [0, 0, 4000, 3000]), room("hall", "hall", Zone::Public, [0, 3000, 4000, 7000])]);
        let r = check_entrances(&l, &ENV, Side::West, &guests(), Some(5000)).unwrap();
        // Along y: guest centre in [600, 2400], family in [3600, 9400]: 9400 - 600.
        assert_eq!(r.separation_mm, Some(8800));
        assert_eq!(r.status, EntranceStatus::Separated);
    }
}
