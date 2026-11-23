//! Sun exposure of sleeping rooms: which bedrooms take the afternoon sun on a west outside wall.
//!
//! In the northern hemisphere the sun sets in the west, so a west-facing outside wall takes the hottest, lowest afternoon
//! sun. This module reports, for one placed level, every sleeping room whose outside wall lies on the west side of the
//! envelope, with the length of that wall, and gives the share of sleeping-room area that is clear of the west wall. It
//! changes nothing.
//!
//! The sides of the envelope are the compass sides of the layout (the same frame `privacy` and `entrances` use). Inputs
//! that are never invented: the **sleeping-room functions** (the backend passes the user's list; no default), and the
//! rooms and envelope of the level. The result is a geometric comparison between layouts, not a heat-gain calculation.

use crate::layout::{Layout, Rect, Side};
use crate::scoring::on_street_side;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SunStatus {
    /// The level has no sleeping room.
    NotApplicable,
    /// No sleeping room has an outside wall on the west.
    Clear,
    /// At least one sleeping room has an outside wall on the west.
    WestExposed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WestRoom {
    pub room: String,
    /// Length (mm) of the room's outside wall on the west side.
    pub wall_mm: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub ar: String,
    pub en: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SunReport {
    pub status: SunStatus,
    pub west_rooms: Vec<WestRoom>,
    /// Number of sleeping rooms checked.
    pub sleeping_rooms: usize,
    /// Percent (0-100) of the sleeping-room area that is not on the west wall; `None` when there is no sleeping room.
    pub clear_area_percent: Option<f64>,
    pub findings: Vec<Finding>,
}

/// Checks one level. `sleeping_funcs` lists the room functions that count as sleeping rooms and must not be empty.
pub fn check_sun(layout: &Layout, envelope: &Rect, sleeping_funcs: &[String]) -> Result<SunReport, String> {
    if sleeping_funcs.is_empty() {
        return Err("name at least one sleeping-room function, otherwise there is nothing to check".to_string());
    }
    if envelope.w <= 0 || envelope.h <= 0 {
        return Err(format!("the envelope must have a positive width and depth, not {} x {} mm", envelope.w, envelope.h));
    }
    let sleeping: Vec<_> = layout.rooms.iter().filter(|r| sleeping_funcs.iter().any(|f| f == &r.func)).collect();
    let say = |ar: String, en: String| Finding { ar, en };
    if sleeping.is_empty() {
        return Ok(SunReport {
            status: SunStatus::NotApplicable,
            west_rooms: Vec::new(),
            sleeping_rooms: 0,
            clear_area_percent: None,
            findings: vec![say("لا توجد غرفة نوم في هذا الدور فلا حاجة للفحص.".to_string(), "This level has no sleeping room, so no check is needed.".to_string())],
        });
    }
    let west_rooms: Vec<WestRoom> =
        sleeping.iter().filter(|r| on_street_side(&r.rect, envelope, Side::West)).map(|r| WestRoom { room: r.key.clone(), wall_mm: r.rect.h }).collect();
    let total: f64 = sleeping.iter().map(|r| r.rect.area_m2()).sum();
    let exposed: f64 = sleeping.iter().filter(|r| west_rooms.iter().any(|w| w.room == r.key)).map(|r| r.rect.area_m2()).sum();
    let clear_area_percent = if total > 0.0 { Some((total - exposed) / total * 100.0) } else { None };
    let (status, findings) = if west_rooms.is_empty() {
        (
            SunStatus::Clear,
            vec![say(
                "لا غرفة نوم بجدار خارجي على الجهة الغربية.".to_string(),
                "No sleeping room has an outside wall on the west side.".to_string(),
            )],
        )
    } else {
        let findings = west_rooms
            .iter()
            .map(|w| {
                say(
                    format!("غرفة النوم «{}» لها جدار خارجي غربي بطول {} مم يتلقى شمس العصر؛ انقلها إلى جهة أخرى أو اجعل الغربية لغرفة خدمة.", w.room, w.wall_mm),
                    format!("Sleeping room '{}' has a {} mm west outside wall that takes the afternoon sun; move it to another side or give the west to a service room.", w.room, w.wall_mm),
                )
            })
            .collect();
        (SunStatus::WestExposed, findings)
    };
    Ok(SunReport { status, west_rooms, sleeping_rooms: sleeping.len(), clear_area_percent, findings })
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

    #[test]
    fn a_bedroom_on_the_west_wall_is_reported_with_its_wall_length() {
        let l = layout(vec![room("bed1", "bedroom", [0, 0, 6000, 10_000]), room("bed2", "bedroom", [6000, 0, 6000, 10_000])]);
        let r = check_sun(&l, &ENV, &beds()).unwrap();
        assert_eq!(r.status, SunStatus::WestExposed);
        assert_eq!(r.west_rooms, vec![WestRoom { room: "bed1".into(), wall_mm: 10_000 }]);
        assert!((r.clear_area_percent.unwrap() - 50.0).abs() < 1e-9);
        assert!(r.findings[0].en.contains("afternoon sun") && r.findings[0].ar.contains("الغربي"));
    }

    #[test]
    fn bedrooms_away_from_the_west_are_clear() {
        let l = layout(vec![room("hall", "hall", [0, 0, 6000, 10_000]), room("bed1", "bedroom", [6000, 0, 6000, 10_000])]);
        let r = check_sun(&l, &ENV, &beds()).unwrap();
        assert_eq!(r.status, SunStatus::Clear);
        assert_eq!(r.clear_area_percent, Some(100.0));
    }

    #[test]
    fn a_level_without_a_bedroom_is_not_applicable() {
        let l = layout(vec![room("hall", "hall", [0, 0, 12_000, 10_000])]);
        let r = check_sun(&l, &ENV, &beds()).unwrap();
        assert_eq!(r.status, SunStatus::NotApplicable);
        assert_eq!(r.clear_area_percent, None);
        assert_eq!(r.sleeping_rooms, 0);
    }

    #[test]
    fn bad_inputs_are_rejected_by_name() {
        let l = layout(vec![room("bed1", "bedroom", [0, 0, 6000, 10_000])]);
        assert!(check_sun(&l, &ENV, &[]).unwrap_err().contains("sleeping-room"));
        assert!(check_sun(&l, &Rect { x: 0, y: 0, w: 0, h: 10 }, &beds()).unwrap_err().contains("positive"));
    }
}
