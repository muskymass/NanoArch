//! Integrity check run before a drawing, report or model file leaves the application.
//!
//! It looks only at the model and decides nothing about design or the code: an **error** is a geometric contradiction (two rooms
//! on top of each other, two openings on the same stretch of wall) that would put a wrong drawing in a contractor's hands; a
//! **warning** is something an engineer must look at (a column with nothing under it, an opening that stands in a column, a room
//! nobody can enter, a door or window that names a room that is no longer there: re-tiling a drafted plan leaves such names behind).
//! No number here is a statutory value.

use crate::building;
use crate::model::{Id, Model};
use crate::typed::{Door, RoomSeparator, Space, Typed, Wall, Window};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// How far (mm) to each side of a room separator the room on that side is looked for. A geometric probe, not a design value.
const SEPARATOR_SIDE_MM: i64 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct Issue {
    pub level: Level,
    pub code: &'static str,
    pub message: String,
    pub elements: Vec<Id>,
}

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub issues: Vec<Issue>,
}

impl Report {
    pub fn errors(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| i.level == Level::Error)
    }

    pub fn has_errors(&self) -> bool {
        self.errors().next().is_some()
    }

    /// One line per error, for a refusal message.
    pub fn error_text(&self) -> String {
        self.errors().map(|i| format!("[{}] {}", i.code, i.message)).collect::<Vec<_>>().join("; ")
    }
}

/// Two rooms sharing less than this (mm²) only touch along an edge: rounding, not an overlap.
const OVERLAP_MIN_MM2: i64 = 10_000;

pub fn run(model: &Model) -> Report {
    let mut issues = Vec::new();
    let level_name: BTreeMap<Id, String> = building::levels(model).into_iter().map(|(n, id)| (id, format!("level {n}"))).collect();
    let at = |l: Option<Id>| l.and_then(|id| level_name.get(&id).cloned()).unwrap_or_else(|| "model".to_string());

    // Rooms per level.
    let mut spaces: BTreeMap<Option<Id>, Vec<(Id, Space)>> = BTreeMap::new();
    for e in model.elements.values().filter(|e| e.kind == Space::KIND) {
        if let Ok(s) = e.typed::<Space>() {
            spaces.entry(e.level_id).or_default().push((e.id, s));
        }
    }
    for (lvl, list) in &spaces {
        for (i, (ia, a)) in list.iter().enumerate() {
            for (ib, b) in &list[i + 1..] {
                let (ox, oy) = ((a.x + a.w).min(b.x + b.w) - a.x.max(b.x), (a.y + a.h).min(b.y + b.h) - a.y.max(b.y));
                if ox > 0 && oy > 0 && ox * oy >= OVERLAP_MIN_MM2 {
                    issues.push(Issue {
                        level: Level::Error,
                        code: "ROOM.OVERLAP",
                        message: format!("{}: rooms '{}' and '{}' overlap by {:.2} m2", at(*lvl), a.requirement_key, b.requirement_key, (ox * oy) as f64 / 1e6),
                        elements: vec![*ia, *ib],
                    });
                }
            }
        }
    }

    // Openings: unknown rooms, and two openings on the same stretch of one wall.
    // A drafted project has rooms only as the names its walls carry, so those count as rooms too.
    let mut wall_rooms: BTreeMap<Option<Id>, BTreeSet<String>> = BTreeMap::new();
    for e in model.elements.values().filter(|e| e.kind == Wall::KIND) {
        if let Ok(w) = e.typed::<Wall>() {
            let set = wall_rooms.entry(e.level_id).or_default();
            set.insert(w.room_a);
            set.extend(w.room_b);
        }
    }
    let keys_of = |lvl: Option<Id>| -> BTreeSet<String> {
        // A level with no room of its own (a drafted project keeps its rooms at the model level) answers to every room there is.
        let own = spaces.get(&lvl).filter(|l| !l.is_empty());
        let mut k: BTreeSet<String> = match own {
            Some(l) => l.iter().map(|(_, s)| s.requirement_key.clone()).collect(),
            None => spaces.values().flatten().map(|(_, s)| s.requirement_key.clone()).collect(),
        };
        k.extend(wall_rooms.get(&lvl).cloned().unwrap_or_default());
        k
    };
    // A room made of several rectangles is one room: a door or a wall that names one of its parts counts for the whole room.
    let whole: BTreeMap<(Option<Id>, String), String> = spaces.iter().flat_map(|(lvl, l)| l.iter().filter_map(|(_, s)| Some(((*lvl, s.requirement_key.clone()), crate::annot::part_of(s)?.to_string())))).collect();
    let whole_of = |lvl: Option<Id>, k: String| whole.get(&(lvl, k.clone())).cloned().unwrap_or(k);
    let mut spans: BTreeMap<Id, Vec<(i64, i64, Id, &'static str)>> = BTreeMap::new();
    let mut entered: BTreeSet<(Option<Id>, String)> = BTreeSet::new();
    for e in model.elements.values() {
        let known = keys_of(e.level_id);
        if e.kind == Door::KIND {
            let Ok(d) = e.typed::<Door>() else { continue };
            for k in [&d.from, &d.to] {
                if !k.is_empty() && k != "outside" && k != "out" {
                    entered.insert((e.level_id, whole_of(e.level_id, k.clone())));
                    if !known.contains(k) {
                        issues.push(Issue { level: Level::Warning, code: "DOOR.UNKNOWN_ROOM", message: format!("{}: a door names room '{k}', which is not on the level", at(e.level_id)), elements: vec![e.id] });
                    }
                }
            }
            if let Some(h) = e.host_id {
                spans.entry(h).or_default().push((d.offset_center - d.width / 2, d.offset_center + d.width / 2, e.id, "door"));
            }
        } else if e.kind == Window::KIND {
            let Ok(w) = e.typed::<Window>() else { continue };
            if !known.contains(&w.room) {
                issues.push(Issue { level: Level::Warning, code: "WINDOW.UNKNOWN_ROOM", message: format!("{}: a window names room '{}', which is not on the level", at(e.level_id), w.room), elements: vec![e.id] });
            }
            if let Some(h) = e.host_id {
                spans.entry(h).or_default().push((w.offset_center - w.width / 2, w.offset_center + w.width / 2, e.id, "window"));
            }
        }
    }
    for (wall, list) in &spans {
        for (i, a) in list.iter().enumerate() {
            for b in &list[i + 1..] {
                if a.0 < b.1 && b.0 < a.1 {
                    let lvl = model.get(wall).and_then(|w| w.level_id);
                    issues.push(Issue { level: Level::Error, code: "OPENING.OVERLAP", message: format!("{}: a {} and a {} overlap on the same wall", at(lvl), a.3, b.3), elements: vec![a.2, b.2] });
                }
            }
        }
    }

    // Warnings: engineer's attention.
    for (lvl, id, c) in building::unsupported_column_elements(model) {
        // A column that stands on named axes is called by them: "AX4/AY3" is found on the drawing, a pair of coordinates is not.
        let message = if c.grid_ref.is_empty() { format!("level {lvl}: the column at ({}, {}) mm has no column below it", c.x, c.y) } else { format!("level {lvl}: the column at {} ({}, {}) mm has no column below it", c.grid_ref, c.x, c.y) };
        issues.push(Issue { level: Level::Warning, code: "COLUMN.UNSUPPORTED", message, elements: vec![id] });
    }
    let level_ids: Vec<Option<Id>> = if level_name.is_empty() { vec![None] } else { level_name.keys().map(|id| Some(*id)).collect() };
    let (moves, stuck) = building::clear_openings(model, &level_ids);
    if !moves.is_empty() || !stuck.is_empty() {
        // Only the counts that are not zero are said: "0 stand in a column and 7 cannot be moved" reads as if none did.
        let message = match (moves.len(), stuck.len()) {
            (m, 0) => format!("{m} door(s) or window(s) stand in a column and can be moved clear of it"),
            (0, s) => format!("{s} door(s) or window(s) stand in a column and are not moved clear of it: check each on the drawing"),
            (m, s) => format!("{} door(s) or window(s) stand in a column: {m} can be moved clear of it and {s} are not moved: check each on the drawing", m + s),
        };
        // The doors and windows themselves, so that whoever reads the warning can be shown each one.
        let elements: Vec<Id> = model.elements.values().filter(|e| e.kind == Door::KIND || e.kind == Window::KIND).filter(|e| building::stands_in_column(model, &e.id)).map(|e| e.id).collect();
        issues.push(Issue { level: Level::Warning, code: "OPENING.IN_COLUMN", message, elements });
    }
    // A room separator is an open boundary, a passage with no wall: the rooms on its two sides are entered through it, door or no door.
    for e in model.elements.values().filter(|e| e.kind == RoomSeparator::KIND) {
        let Ok(sep) = e.typed::<RoomSeparator>() else { continue };
        let Some(list) = spaces.get(&e.level_id) else { continue };
        let vertical = sep.x1 == sep.x2;
        // Asked at three places along it, a little to each side: a long separator can pass more than one rectangle.
        for quarter in 1..=3 {
            let (x, y) = (sep.x1 + (sep.x2 - sep.x1) * quarter / 4, sep.y1 + (sep.y2 - sep.y1) * quarter / 4);
            for off in [-SEPARATOR_SIDE_MM, SEPARATOR_SIDE_MM] {
                let (px, py) = if vertical { (x + off, y) } else { (x, y + off) };
                if let Some((_, s)) = list.iter().find(|(_, s)| px > s.x && px < s.x + s.w && py > s.y && py < s.y + s.h) {
                    entered.insert((e.level_id, whole_of(e.level_id, s.requirement_key.clone())));
                }
            }
        }
    }
    let walled: BTreeSet<(Option<Id>, String)> = model
        .elements
        .values()
        .filter(|e| e.kind == Wall::KIND)
        .filter_map(|e| e.typed::<Wall>().ok().map(|w| (e.level_id, whole_of(e.level_id, w.room_a))))
        .collect();
    for (lvl, list) in &spaces {
        for (id, s) in list {
            if s.func == "courtyard" || crate::annot::part_of(s).is_some() || !walled.contains(&(*lvl, s.requirement_key.clone())) {
                continue;
            }
            if !entered.contains(&(*lvl, s.requirement_key.clone())) {
                issues.push(Issue { level: Level::Warning, code: "ROOM.NO_DOOR", message: format!("{}: room '{}' has no door", at(*lvl), s.requirement_key), elements: vec![*id] });
            }
        }
    }
    issues.sort_by(|a, b| (std::cmp::Reverse(a.level), a.code, &a.message).cmp(&(std::cmp::Reverse(b.level), b.code, &b.message)));
    Report { issues }
}
