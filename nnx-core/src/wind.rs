//! Cross ventilation against the prevailing wind: which habitable rooms can take the wind on one outside wall and let it
//! out through the opposite one.
//!
//! This application holds no wind data for any city (see `analysis`). The **prevailing wind side** (the side the wind comes
//! from) and its **source** are therefore inputs the user must supply; an empty source or an unnamed side is refused, and
//! nothing is defaulted. The **habitable room functions** have no default either.
//!
//! For one placed level, each habitable room is classed by the outside walls it has on the envelope:
//! - `cross`: outside walls on the windward side and on the opposite (leeward) side, so wind can pass through the room;
//! - `corner`: a windward wall plus an outside wall on a perpendicular side (partial cross flow);
//! - `windward_only`: only a windward wall, the air has no exit through the room;
//! - `sheltered`: no windward wall.
//!
//! The classification is geometric (it reads walls, not windows) and compares layouts; it is not an airflow simulation.
//! The report also repeats the cross-ventilated share for each of the four possible wind sides, so a layout can be judged
//! against the wind direction being uncertain.

use crate::layout::{Layout, Rect, Side};
use crate::scoring::on_street_side;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Flow {
    Cross,
    Corner,
    WindwardOnly,
    Sheltered,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RoomFlow {
    pub room: String,
    pub flow: Flow,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DirectionShare {
    /// The side the wind comes from.
    pub from: String,
    /// Percent (0-100) of the habitable area that is `cross`.
    pub cross_area_percent: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub ar: String,
    pub en: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WindReport {
    pub wind_from: String,
    pub source: String,
    pub habitable_rooms: usize,
    pub rooms: Vec<RoomFlow>,
    /// Percent (0-100) of the habitable area that is `cross`; `None` when the level has no habitable room.
    pub cross_area_percent: Option<f64>,
    /// The same share for each of the four wind sides.
    pub by_direction: Vec<DirectionShare>,
    pub findings: Vec<Finding>,
}

fn name(s: Side) -> &'static str {
    match s {
        Side::South => "south",
        Side::North => "north",
        Side::East => "east",
        Side::West => "west",
    }
}

fn opposite(s: Side) -> Side {
    match s {
        Side::South => Side::North,
        Side::North => Side::South,
        Side::East => Side::West,
        Side::West => Side::East,
    }
}

fn perpendicular(s: Side) -> [Side; 2] {
    match s {
        Side::South | Side::North => [Side::East, Side::West],
        Side::East | Side::West => [Side::South, Side::North],
    }
}

fn classify(rect: &Rect, env: &Rect, from: Side) -> Flow {
    let on = |s: Side| on_street_side(rect, env, s);
    if !on(from) {
        Flow::Sheltered
    } else if on(opposite(from)) {
        Flow::Cross
    } else if perpendicular(from).into_iter().any(on) {
        Flow::Corner
    } else {
        Flow::WindwardOnly
    }
}

fn cross_share(rooms: &[&crate::layout::PlacedRoom], env: &Rect, from: Side) -> f64 {
    let total: f64 = rooms.iter().map(|r| r.rect.area_m2()).sum();
    if total <= 0.0 {
        return 0.0;
    }
    let cross: f64 = rooms.iter().filter(|r| classify(&r.rect, env, from) == Flow::Cross).map(|r| r.rect.area_m2()).sum();
    cross / total * 100.0
}

/// Checks one level. `wind_from` is the side the prevailing wind comes from, `source` says where that came from (both
/// required), and `habitable_funcs` lists the room functions that need ventilation and must not be empty.
pub fn check_wind(layout: &Layout, envelope: &Rect, wind_from: Option<Side>, source: &str, habitable_funcs: &[String]) -> Result<WindReport, String> {
    let from = wind_from.ok_or("wind: the side the prevailing wind comes from is missing; enter it from a source you trust, none is assumed")?;
    if source.trim().is_empty() {
        return Err("wind: the source of the prevailing-wind direction is missing; state where it was read".to_string());
    }
    if habitable_funcs.is_empty() {
        return Err("wind: name at least one habitable-room function, otherwise there is nothing to check".to_string());
    }
    if envelope.w <= 0 || envelope.h <= 0 {
        return Err(format!("wind: the envelope must have a positive width and depth, not {} x {} mm", envelope.w, envelope.h));
    }
    let habitable: Vec<_> = layout.rooms.iter().filter(|r| habitable_funcs.iter().any(|f| f == &r.func)).collect();
    let say = |ar: String, en: String| Finding { ar, en };
    if habitable.is_empty() {
        return Ok(WindReport {
            wind_from: name(from).to_string(),
            source: source.trim().to_string(),
            habitable_rooms: 0,
            rooms: Vec::new(),
            cross_area_percent: None,
            by_direction: Vec::new(),
            findings: vec![say("لا توجد غرفة تحتاج تهوية في هذا الدور فلا حاجة للفحص.".to_string(), "This level has no habitable room, so no check is needed.".to_string())],
        });
    }
    let rooms: Vec<RoomFlow> = habitable.iter().map(|r| RoomFlow { room: r.key.clone(), flow: classify(&r.rect, envelope, from) }).collect();
    let by_direction: Vec<DirectionShare> =
        [Side::South, Side::North, Side::East, Side::West].into_iter().map(|s| DirectionShare { from: name(s).to_string(), cross_area_percent: cross_share(&habitable, envelope, s) }).collect();
    let cross_area_percent = Some(cross_share(&habitable, envelope, from));
    let mut findings = Vec::new();
    for r in &rooms {
        match r.flow {
            Flow::Cross => {}
            Flow::Corner => findings.push(say(
                format!("الغرفة «{}» تأخذ الريح من جدار وتخرجها من جدار جانبي فقط (تهوية ركنية جزئية).", r.room),
                format!("Room '{}' gets the wind on one wall and a side wall only (partial corner ventilation).", r.room),
            )),
            Flow::WindwardOnly => findings.push(say(
                format!("الغرفة «{}» تواجه الريح بلا جدار خارجي مقابل لخروج الهواء؛ افتح لها منفذاً مقابلاً أو ممراً نافذاً.", r.room),
                format!("Room '{}' faces the wind but has no opposite outside wall for the air to leave; give it an opposite opening or a through corridor.", r.room),
            )),
            Flow::Sheltered => findings.push(say(
                format!("الغرفة «{}» محجوبة عن الريح السائدة ({}).", r.room, name(from)),
                format!("Room '{}' is sheltered from the prevailing wind ({}).", r.room, name(from)),
            )),
        }
    }
    if findings.is_empty() {
        findings.push(say("كل الغرف المطلوب تهويتها تتهوى عرضياً مع الريح السائدة.".to_string(), "Every habitable room cross-ventilates with the prevailing wind.".to_string()));
    }
    Ok(WindReport { wind_from: name(from).to_string(), source: source.trim().to_string(), habitable_rooms: habitable.len(), rooms, cross_area_percent, by_direction, findings })
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
    fn funcs() -> Vec<String> {
        vec!["bedroom".into(), "living".into()]
    }

    #[test]
    fn a_room_spanning_the_depth_cross_ventilates_with_a_north_wind() {
        // Both rooms span south to north: wind from the north meets north and south walls.
        let l = layout(vec![room("a", "bedroom", [0, 0, 6000, 10_000]), room("b", "living", [6000, 0, 6000, 10_000])]);
        let r = check_wind(&l, &ENV, Some(Side::North), "city met station 2020", &funcs()).unwrap();
        assert!(r.rooms.iter().all(|x| x.flow == Flow::Cross));
        assert_eq!(r.cross_area_percent, Some(100.0));
        assert!(r.findings[0].en.contains("cross-ventilates"));
    }

    #[test]
    fn rooms_are_classed_corner_windward_only_and_sheltered() {
        // 2x2 grid: wind from the south.
        let l = layout(vec![
            room("sw", "bedroom", [0, 0, 6000, 5000]),
            room("se", "bedroom", [6000, 0, 6000, 5000]),
            room("nw", "bedroom", [0, 5000, 6000, 5000]),
        ]);
        let r = check_wind(&l, &ENV, Some(Side::South), "survey", &funcs()).unwrap();
        let get = |k: &str| r.rooms.iter().find(|x| x.room == k).unwrap().flow;
        assert_eq!(get("sw"), Flow::Corner);
        assert_eq!(get("se"), Flow::Corner);
        assert_eq!(get("nw"), Flow::Sheltered);
        // A room touching only the windward side (inner stretch of the south wall).
        let inner = layout(vec![room("mid", "bedroom", [3000, 0, 4000, 5000])]);
        let r = check_wind(&inner, &ENV, Some(Side::South), "survey", &funcs()).unwrap();
        assert_eq!(r.rooms[0].flow, Flow::WindwardOnly);
        assert_eq!(r.cross_area_percent, Some(0.0));
    }

    #[test]
    fn by_direction_reports_all_four_sides() {
        let l = layout(vec![room("a", "bedroom", [0, 0, 12_000, 4000]), room("b", "bedroom", [0, 4000, 12_000, 6000])]);
        let r = check_wind(&l, &ENV, Some(Side::East), "survey", &funcs()).unwrap();
        assert_eq!(r.by_direction.len(), 4);
        let east = r.by_direction.iter().find(|d| d.from == "east").unwrap();
        assert!((east.cross_area_percent - 100.0).abs() < 1e-9);
        let north = r.by_direction.iter().find(|d| d.from == "north").unwrap();
        assert!((north.cross_area_percent - 0.0).abs() < 1e-9);
    }

    #[test]
    fn a_level_without_habitable_rooms_is_not_checked() {
        let l = layout(vec![room("w", "wc", [0, 0, 3000, 3000])]);
        let r = check_wind(&l, &ENV, Some(Side::North), "survey", &funcs()).unwrap();
        assert_eq!(r.habitable_rooms, 0);
        assert_eq!(r.cross_area_percent, None);
    }

    #[test]
    fn missing_direction_source_or_functions_are_refused_by_name() {
        let l = layout(vec![room("a", "bedroom", [0, 0, 6000, 10_000])]);
        assert!(check_wind(&l, &ENV, None, "s", &funcs()).unwrap_err().contains("prevailing wind"));
        assert!(check_wind(&l, &ENV, Some(Side::North), "  ", &funcs()).unwrap_err().contains("source"));
        assert!(check_wind(&l, &ENV, Some(Side::North), "s", &[]).unwrap_err().contains("habitable-room"));
        assert!(check_wind(&l, &Rect { x: 0, y: 0, w: 0, h: 10 }, Some(Side::North), "s", &funcs()).unwrap_err().contains("positive"));
    }
}
