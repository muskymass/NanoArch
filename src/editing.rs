//! Editing the plan. Every edit is a set of model commands committed as ONE undoable step, validated by the
//! document's strict typed check, followed by a structure sync in the same step. Nothing is edited on the drawing:
//! the drawing is redrawn from the model afterwards.
//!
//! - Moving a partition changes the rectangles of the rooms on both sides (the whole connected stretch of
//!   that wall line moves together, so the plan keeps tiling with no gaps or overlaps), then walls, doors and
//!   windows are regenerated from the new rooms. Manual door and window edits on the plan are therefore
//!   reset by a partition move; the interface says so.
//! - A door or window can be resized, moved along its wall, or deleted without touching anything else.

use nnx_core::layout::{Brief, Layout, PlacedRoom, Rect, Zone};
use nnx_core::model::Element;
use crate::levels;
use nnx_core::building;
use nnx_core::structural::StructOpts;
use nnx_core::typed::{Door, MepPoint, Space, Typed, Wall, Window};
use nnx_core::walls::{self, WindowOpts};
use nnx_core::{Command, Document, Id, Origin, Source};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Smallest room dimension an edit may produce. A geometric floor, not a code value; the rule engine
/// checks real minimum sizes once the SBC values are entered.
pub const MIN_ROOM_MM: i64 = 1000;

struct Room {
    id: Id,
    key: String,
    func: String,
    rect: Rect,
    target: f64,
}

/// The rooms of one level (`None`: the model has no levels, so every room).
fn read_rooms(doc: &Document, level: Option<Id>) -> Result<Vec<Room>, String> {
    let mut rooms = Vec::new();
    for e in doc.model.elements.values().filter(|e| e.kind == Space::KIND && (level.is_none() || e.level_id == level)) {
        let s = e.typed::<Space>().map_err(|err| format!("space {}: {err}", e.id))?;
        rooms.push(Room { id: e.id, key: s.requirement_key, func: s.func, rect: Rect { x: s.x, y: s.y, w: s.w, h: s.h }, target: s.target_area });
    }
    rooms.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(rooms)
}

/// Commits `cmds` and the structure sync that follows from them as one transaction. The sync is computed
/// on a copy first, so a failing edit changes nothing.
fn commit_with_sync(doc: &mut Document, so: &StructOpts, label: &str, cmds: Vec<Command>, level: Option<Id>) -> Result<(), String> {
    let mut scratch = Document::new(doc.model.clone());
    scratch.commit(label, Origin::User, cmds.clone()).map_err(|e| e.to_string())?;
    // The edited level and every level below it: loads come down from above, so footings can change too.
    let from = level.and_then(|l| building::levels(&doc.model).iter().position(|(_, id)| *id == l));
    let (sync_cmds, _) = levels::settle(&mut scratch, so, from)?;
    let mut all = cmds;
    all.extend(sync_cmds);
    doc.commit(label, Origin::User, all).map_err(|e| e.to_string())
}

/// Moves an interior wall by `delta_mm` perpendicular to itself. Returns how many rooms changed size.
pub fn move_partition(doc: &mut Document, brief: &Brief, so: &StructOpts, wall_id: Id, delta_mm: i64) -> Result<usize, String> {
    let d = (delta_mm / 10) * 10;
    if d == 0 {
        return Err("the move is smaller than 10 mm".into());
    }
    let wall_el = doc.model.get(&wall_id).ok_or("that wall no longer exists")?;
    let wall = wall_el.typed::<Wall>().map_err(|_| "that element is not a wall")?;
    if wall.is_exterior || wall.room_b.is_none() {
        return Err("an exterior wall sets the building outline and cannot be moved".into());
    }
    let level = wall_el.level_id;
    if building::is_stair_block(&wall.room_a) || wall.room_b.as_deref().is_some_and(building::is_stair_block) {
        return Err("the stair block is the same on every level and cannot be moved".into());
    }
    let horizontal = wall.horizontal();
    let line = wall.centre_line();
    let (s, e) = (wall.start(), wall.start() + wall.length);

    let mut rooms = read_rooms(doc, level)?;
    // Axis helpers: `cross` is the direction the wall moves in, `along` is the wall's own direction.
    let cross = |r: &Rect| if horizontal { (r.y, r.y + r.h) } else { (r.x, r.x + r.w) };
    let along = |r: &Rect| if horizontal { (r.x, r.x + r.w) } else { (r.y, r.y + r.h) };

    // Grow the set of rooms that must move together: any room on the line whose edge overlaps the stretch.
    let mut stretch = (s, e);
    let mut moving: Vec<usize> = Vec::new();
    loop {
        let mut changed = false;
        for (i, r) in rooms.iter().enumerate() {
            if moving.contains(&i) {
                continue;
            }
            let (c0, c1) = cross(&r.rect);
            let (a0, a1) = along(&r.rect);
            let on_line = c1 == line || c0 == line;
            if on_line && a0.max(stretch.0) < a1.min(stretch.1) {
                moving.push(i);
                stretch = (stretch.0.min(a0), stretch.1.max(a1));
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    if moving.iter().any(|&i| building::is_stair_block(&rooms[i].key)) {
        return Err("this partition is joined to the stair block, which is the same on every level and cannot be moved".into());
    }
    // Both sides must cover the whole stretch exactly, or the wall ends against a room that would be cut.
    let covered = |before: bool| -> i64 {
        moving
            .iter()
            .filter(|&&i| (cross(&rooms[i].rect).1 == line) == before && (cross(&rooms[i].rect).0 == line) != before)
            .map(|&i| along(&rooms[i].rect).1 - along(&rooms[i].rect).0)
            .sum()
    };
    let len = stretch.1 - stretch.0;
    if covered(true) != len || covered(false) != len {
        return Err("this partition ends against another room and cannot be moved on its own".into());
    }

    let mut moved_rects: BTreeMap<Id, Rect> = BTreeMap::new();
    for &i in &moving {
        let mut r = rooms[i].rect;
        let before = cross(&r).1 == line;
        let (pos, size) = if horizontal { (&mut r.y, &mut r.h) } else { (&mut r.x, &mut r.w) };
        if before {
            *size += d;
        } else {
            *pos += d;
            *size -= d;
        }
        if *size < MIN_ROOM_MM {
            return Err(format!("the room '{}' would become {} mm wide; the minimum for an edit is {MIN_ROOM_MM} mm", rooms[i].key, *size));
        }
        moved_rects.insert(rooms[i].id, r);
    }
    for r in rooms.iter_mut() {
        if let Some(n) = moved_rects.get(&r.id) {
            r.rect = *n;
        }
    }

    // Rebuild the architecture from the new rooms, exactly as generation does.
    let lookup = |key: &str| brief.rooms.iter().find(|q| q.key == key);
    let placed: Vec<PlacedRoom> = rooms
        .iter()
        .map(|r| {
            let (zone, light) = lookup(&r.key).map_or((Zone::Service, false), |q| (q.zone, q.needs_light));
            let area = r.rect.area_m2();
            PlacedRoom {
                key: r.key.clone(),
                func: r.func.clone(),
                zone,
                needs_light: light,
                rect: r.rect,
                area_m2: area,
                target_m2: r.target,
                deviation_pct: if r.target > 0.0 { (area / r.target - 1.0) * 100.0 } else { 0.0 },
            }
        })
        .collect();
    let layout = Layout { rooms: placed, cost: 0.0, breakdown: Default::default(), reserve_m2: 0.0 };
    let mut wall_opts = walls::opts_from_adjacency(&brief.adjacency);
    // Only the lowest level has the street entrance.
    wall_opts.entry = level.is_none_or(|l| building::levels(&doc.model).first().is_some_and(|(_, id)| *id == l));
    let plan = walls::build(&layout, brief.envelope, brief.entry, &wall_opts);
    let wp = walls::place_windows(&layout, &plan, &wall_opts, &WindowOpts::default());
    let ids: BTreeMap<String, Id> = rooms.iter().map(|r| (r.key.clone(), r.id)).collect();

    let mut cmds: Vec<Command> = Vec::new();
    for (id, r) in &moved_rects {
        let set = |key: &str, v: Value| Command::SetProp { id: *id, key: key.into(), value: Some(v) };
        cmds.push(set("x", json!(r.x)));
        cmds.push(set("y", json!(r.y)));
        cmds.push(set("w", json!(r.w)));
        cmds.push(set("h", json!(r.h)));
        cmds.push(set("area", json!((r.area_m2() * 100.0).round() / 100.0)));
        cmds.push(set("boundary", json!([[r.x, r.y], [r.x + r.w, r.y], [r.x + r.w, r.y + r.h], [r.x, r.y + r.h]])));
    }
    // Remove the old openings and walls (openings first: a wall that hosts one cannot be deleted).
    for kind in [Door::KIND, Window::KIND, Wall::KIND] {
        cmds.extend(doc.model.elements.values().filter(|e| e.kind == kind && (level.is_none() || e.level_id == level)).map(|e| Command::Delete(e.id)));
    }
    cmds.extend(walls::to_commands(&plan, &ids, level));
    cmds.extend(walls::window_commands(&plan, &wp, &ids, level));
    // MEP points sit inside rooms that this edit resizes. Generated ones are dropped in the same transaction
    // (one undo brings them back; recompute places them again); one the user placed or locked stays, so it
    // must still lie inside its room or the edit is refused.
    for e in doc.model.elements.values().filter(|e| e.kind == MepPoint::KIND && (level.is_none() || e.level_id == level)) {
        let Ok(p) = e.typed::<MepPoint>() else { continue };
        if e.provenance.source == Source::Generated && !e.provenance.locked {
            cmds.push(Command::Delete(e.id));
        } else if let Some(r) = rooms.iter().find(|r| r.key == p.room).map(|r| r.rect)
            && !(p.x >= r.x && p.x <= r.x + r.w && p.y >= r.y && p.y <= r.y + r.h)
        {
            return Err(format!("the {} point at ({}, {}) in '{}' would end up outside its room; move or delete it first", p.role, p.x, p.y, p.room));
        }
    }
    commit_with_sync(doc, so, "move partition", cmds, level)?;
    Ok(moved_rects.len())
}

/// Changes a door or window's width and/or its position along the wall (centre offset from the wall start).
pub fn set_opening(doc: &mut Document, so: &StructOpts, id: Id, width: Option<i64>, offset: Option<i64>) -> Result<(), String> {
    let el = doc.model.get(&id).ok_or("that element no longer exists")?;
    if el.kind != Door::KIND && el.kind != Window::KIND {
        return Err("only doors and windows can be edited this way".into());
    }
    let level = el.level_id;
    let mut cmds = Vec::new();
    if let Some(w) = width {
        cmds.push(Command::SetProp { id, key: "width".into(), value: Some(json!(w)) });
    }
    if let Some(o) = offset {
        cmds.push(Command::SetProp { id, key: "offsetCenter".into(), value: Some(json!(o)) });
    }
    if cmds.is_empty() {
        return Err("nothing to change".into());
    }
    // A position typed by the user is never moved behind their back: one that would stand in a column is refused.
    let mut probe = Document::new(doc.model.clone());
    probe.commit("probe", Origin::User, cmds.clone()).map_err(|e| e.to_string())?;
    let (fix, _) = building::clear_openings(&probe.model, &[level]);
    if fix.iter().any(|c| matches!(c, Command::SetProp { id: i, .. } if *i == id)) {
        return Err("that position or width would put the opening in a column: choose another".into());
    }
    commit_with_sync(doc, so, "edit opening", cmds, level)
}

pub fn delete_opening(doc: &mut Document, so: &StructOpts, id: Id) -> Result<(), String> {
    let el = doc.model.get(&id).ok_or("that element no longer exists")?;
    if el.kind != Door::KIND && el.kind != Window::KIND {
        return Err("only doors and windows can be deleted from the plan".into());
    }
    let level = el.level_id;
    commit_with_sync(doc, so, "delete opening", vec![Command::Delete(id)], level)
}

/// What the inspector shows for a clicked element.
pub fn describe(doc: &Document, id: Id) -> Result<Value, String> {
    let el: &Element = doc.model.get(&id).ok_or("that element no longer exists")?;
    Ok(match el.kind.as_str() {
        "Wall" => {
            let w = el.typed::<Wall>().map_err(|e| e.to_string())?;
            json!({ "kind": "Wall", "id": id.to_string(), "exterior": w.is_exterior, "movable": !w.is_exterior && w.room_b.is_some(), "horizontal": w.horizontal(),
                    "length": w.length, "thickness": w.thickness, "rooms": [w.room_a, w.room_b] })
        }
        "Door" | "Window" => {
            let (width, height, offset) = if el.kind == "Door" {
                let d = el.typed::<Door>().map_err(|e| e.to_string())?;
                (d.width, d.height, d.offset_center)
            } else {
                let w = el.typed::<Window>().map_err(|e| e.to_string())?;
                (w.width, w.height, w.offset_center)
            };
            let wall_len = el.host_id.and_then(|h| doc.model.get(&h)).and_then(|h| h.typed::<Wall>().ok()).map_or(0, |w| w.length);
            json!({ "kind": el.kind, "id": id.to_string(), "width": width, "height": height, "offset": offset, "wall_length": wall_len })
        }
        "Space" => {
            let s = el.typed::<Space>().map_err(|e| e.to_string())?;
            json!({ "kind": "Space", "id": id.to_string(), "key": s.requirement_key, "func": s.func, "area": s.area, "target": s.target_area, "w": s.w, "h": s.h })
        }
        other => json!({ "kind": other, "id": id.to_string() }),
    })
}
