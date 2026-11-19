//! Multi-storey helpers: the stair block that is identical on every level, and the check that upper columns
//! stand on columns below.
//!
//! The stair block is a full-depth strip along the west or east side of the buildable envelope. It holds the
//! stair shaft (the stair width x the stair length, both entered by the user) and a hall in the rest of the
//! strip, which is the circulation hub and the entrance on the ground level. The strip is the same rectangle
//! on every level, so the shafts stack. Whatever is left of the envelope is laid out per level by the generator.
//!
//! The flight itself (treads, risers, landings) is NOT modelled: only the shaft, which is left open in the
//! slabs of every level except the top one.

use crate::layout::{PlacedRoom, Rect, Side, Zone};
use crate::model::Model;
use crate::typed::{Column, Level, Typed};

/// What the user enters about the stair. Nothing here is defaulted.
#[derive(Debug, Clone, Copy)]
pub struct Stair {
    pub width_mm: i64,
    pub length_mm: i64,
    /// True: the strip is on the west side of the envelope; false: on the east side.
    pub west: bool,
}

/// Smallest hall left beside the stair, and smallest main area left beside the strip. Geometric floors that keep
/// the generator solvable, not code values; the code check belongs to the rule packs.
pub const MIN_HALL_MM: i64 = 1000;
pub const MIN_MAIN_MM: i64 = 3000;

#[derive(Debug, Clone)]
pub struct Block {
    /// What is left of the envelope for the generated rooms.
    pub main: Rect,
    /// The stair and the hall, as placed rooms (`stair` and `stairhall`, with `suffix` appended).
    pub rooms: Vec<PlacedRoom>,
}

fn placed(key: String, func: &str, zone: Zone, light: bool, rect: Rect) -> PlacedRoom {
    PlacedRoom { key, func: func.into(), zone, needs_light: light, area_m2: rect.area_m2(), target_m2: rect.area_m2(), deviation_pct: 0.0, rect }
}

/// The stair strip inside `envelope`. The hall sits at the street end of the strip (the north end when the
/// entrance is on the north side, otherwise the south end) and the stair at the far end.
pub fn stair_block(envelope: Rect, entry: Side, stair: &Stair, suffix: &str) -> Result<Block, String> {
    let round10 = |v: i64| (v / 10) * 10;
    let (w, l) = (round10(stair.width_mm), round10(stair.length_mm));
    if w <= 0 || l <= 0 {
        return Err("the stair width and length must be positive".into());
    }
    if envelope.h - l < MIN_HALL_MM {
        return Err(format!("the stair is {l} mm long but the plot is {} mm deep: no room is left for a hall beside it", envelope.h));
    }
    if envelope.w - w < MIN_MAIN_MM {
        return Err(format!("the stair is {w} mm wide but the plot is {} mm wide: too little is left for the rooms", envelope.w));
    }
    let strip_x = if stair.west { envelope.x } else { envelope.x + envelope.w - w };
    let main = Rect { x: if stair.west { envelope.x + w } else { envelope.x }, y: envelope.y, w: envelope.w - w, h: envelope.h };
    let hall_h = envelope.h - l;
    let (hall_y, stair_y) = if entry == Side::North { (envelope.y + l, envelope.y) } else { (envelope.y, envelope.y + hall_h) };
    let hall = Rect { x: strip_x, y: hall_y, w, h: hall_h };
    let shaft = Rect { x: strip_x, y: stair_y, w, h: l };
    Ok(Block { main, rooms: vec![placed(format!("stairhall{suffix}"), "hall", Zone::Public, true, hall), placed(format!("stair{suffix}"), "stair", Zone::Service, false, shaft)] })
}

/// True for the rooms of the stair block (they are fixed on every level and never moved by an edit).
pub fn is_stair_block(key: &str) -> bool {
    key.starts_with("stair")
}

/// True for the stair shaft only.
pub fn is_shaft(key: &str) -> bool {
    key == "stair" || key.starts_with("stair@")
}

/// Levels of the model as `(index, id)`, lowest first.
pub fn levels(model: &Model) -> Vec<(i64, crate::model::Id)> {
    let mut v: Vec<(i64, crate::model::Id)> = model.elements.values().filter(|e| e.kind == Level::KIND).filter_map(|e| Some((e.typed::<Level>().ok()?.index, e.id))).collect();
    v.sort();
    v
}

/// Columns of an upper level that have no column of the level below at the same position: their load has nowhere
/// to go. Returned as `(level index, x, y)`.
pub fn unsupported_columns(model: &Model) -> Vec<(i64, i64, i64)> {
    let lv = levels(model);
    let cols = |id: crate::model::Id| -> Vec<Column> { model.elements.values().filter(|e| e.kind == Column::KIND && e.level_id == Some(id)).filter_map(|e| e.typed::<Column>().ok()).collect() };
    let mut out = Vec::new();
    for pair in lv.windows(2) {
        let (below, above) = (cols(pair[0].1), cols(pair[1].1));
        for c in &above {
            let tol = c.b / 2;
            if !below.iter().any(|d| (d.x - c.x).abs() <= tol && (d.y - c.y).abs() <= tol) {
                out.push((pair[1].0, c.x, c.y));
            }
        }
    }
    out
}

/// Margin kept between an opening and the end of its wall, and between two openings: geometric clearances, not code values.
const END_MARGIN_MM: i64 = 150;
const OPENING_GAP_MM: i64 = 100;
/// Clear space kept between an opening and a column face.
const COLUMN_CLEAR_MM: i64 = 50;

/// Doors and windows that stand in a column, moved along their wall to the nearest position that clears every column
/// and every other opening and keeps the wall's end margins. `levels` are the levels to check (`None` for a model
/// without levels). Returns the `SetProp` commands and one note per opening that cannot be moved clear (the wall is
/// too short, or the columns leave no gap wide enough): those are left exactly where they are.
pub fn clear_openings(model: &Model, levels: &[Option<crate::model::Id>]) -> (Vec<crate::command::Command>, Vec<String>) {
    use crate::command::Command;
    use crate::typed::{Door, Wall, Window};
    let mut cmds = Vec::new();
    let mut notes = Vec::new();
    let round10 = |v: i64| (v + 5).div_euclid(10) * 10;
    for lvl in levels {
        let cols: Vec<Column> = model.elements.values().filter(|e| e.kind == Column::KIND && (lvl.is_none() || e.level_id == *lvl)).filter_map(|e| e.typed::<Column>().ok()).collect();
        if cols.is_empty() {
            continue;
        }
        for (wid, w) in model.elements.values().filter(|e| e.kind == Wall::KIND && (lvl.is_none() || e.level_id == *lvl)).filter_map(|e| Some((e.id, e.typed::<Wall>().ok()?))) {
            let (s, end, cl, half_t) = (w.start(), w.start() + w.length, w.centre_line(), w.thickness / 2);
            // Blocked stretches along this wall: each column face plus its clearance.
            let blocked: Vec<(i64, i64)> = cols
                .iter()
                .filter(|c| {
                    let (along, across) = if w.horizontal() { (c.x, c.y) } else { (c.y, c.x) };
                    (across - cl).abs() <= half_t.max(c.b / 2) && along >= s - c.b / 2 && along <= end + c.b / 2
                })
                .map(|c| {
                    let along = if w.horizontal() { c.x } else { c.y };
                    (along - c.b / 2 - COLUMN_CLEAR_MM, along + c.b / 2 + COLUMN_CLEAR_MM)
                })
                .collect();
            if blocked.is_empty() {
                continue;
            }
            // (element id, centre along the wall, width, is a door)
            let mut ops: Vec<(crate::model::Id, i64, i64, bool)> = Vec::new();
            for e in model.elements.values().filter(|e| e.host_id == Some(wid)) {
                if let Ok(d) = e.typed::<Door>() {
                    ops.push((e.id, s + d.offset_center, d.width, true));
                } else if let Ok(win) = e.typed::<Window>() {
                    ops.push((e.id, s + win.offset_center, win.width, false));
                }
            }
            ops.sort_by_key(|o| (o.1, o.0));
            let hits = |c: i64, wd: i64| blocked.iter().any(|&(lo, hi)| c - wd / 2 < hi && c + wd / 2 > lo);
            // Openings that already clear every column stay where they are and constrain the others.
            let mut placed: Vec<(i64, i64)> = ops.iter().filter(|o| !hits(o.1, o.2)).map(|o| (o.1 - o.2 / 2, o.1 + o.2 / 2)).collect();
            for (id, c0, wd, door) in ops.iter().filter(|o| hits(o.1, o.2)) {
                let fits = |c: i64| c - wd / 2 >= s + END_MARGIN_MM && c + wd / 2 <= end - END_MARGIN_MM && !hits(c, *wd) && placed.iter().all(|&(lo, hi)| c - wd / 2 >= hi + OPENING_GAP_MM || c + wd / 2 <= lo - OPENING_GAP_MM);
                let mut candidates: Vec<i64> = blocked.iter().flat_map(|&(lo, hi)| [round10(hi + wd / 2), round10(lo - wd / 2)]).collect();
                candidates.sort_by_key(|c| ((c - c0).abs(), *c));
                match candidates.into_iter().find(|&c| fits(c)) {
                    Some(c) => {
                        placed.push((c - wd / 2, c + wd / 2));
                        cmds.push(Command::SetProp { id: *id, key: "offsetCenter".into(), value: Some(serde_json::json!(c - s)) });
                    }
                    None => notes.push(format!("a {} {} mm wide stands in a column on a {} mm wall and cannot be moved clear of it", if *door { "door" } else { "window" }, wd, w.length)),
                }
            }
        }
    }
    (cmds, notes)
}
