//! Interactive BIM drafting over the model: snapping, drawing walls, placing doors and windows, selecting by box, moving, copying,
//! rotating and mirroring, splitting and trimming walls, deleting, and the property edits of the inspector.
//!
//! Everything here is pure: it reads the model and returns [`Command`]s. The caller commits them as ONE undoable step on the document
//! (`Document::commit`), so undo, redo, the dirty flag and the `.nnx` file follow, and the strict typed check refuses anything invalid.
//!
//! - **Orthogonal walls only.** The model's `Wall` is horizontal or vertical (its typed check says so); a wall at another angle is
//!   refused with that reason rather than approximated.
//! - **Nothing is invented.** A wall's thickness and height, a door's or window's size and sill come from the user (the page offers the
//!   values most common on the level as a starting point and says so). The rooms on either side of a drafted wall, whether it is
//!   exterior, and the rooms a door connects are derived from the rooms' rectangles, never assumed.
//! - **Drafted elements belong to the user** (`Source::User`): generators that rebuild walls, doors and windows leave them alone.
//! - Rooms are not drafted here: they come from the layout and are edited with the partition and room tools.

use crate::building;
use crate::command::Command;
use crate::model::{Element, Id, Model, Source};
use crate::typed::{Column, Dimension, Door, Grid, RoomSeparator, Space, Typed, Wall, Window, element_of};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A point in plan, millimetres. Integers for anything stored; floats while the pointer moves.
pub type P = [f64; 2];

/// The shortest wall that can be drawn: a geometric floor, not a code value.
pub const MIN_WALL_MM: i64 = 100;
/// Clear distance kept between an opening and the end of its wall: a geometric clearance, not a code value.
pub const EDGE_MARGIN_MM: i64 = 50;
/// Door and window positions are rounded to this step.
pub const OFFSET_STEP_MM: i64 = 50;

fn on_level(e: &Element, level: Option<Id>) -> bool {
    level.is_none() || e.level_id == level
}

fn walls_on(model: &Model, level: Option<Id>) -> Vec<(Id, Wall)> {
    model.elements.values().filter(|e| e.kind == Wall::KIND && on_level(e, level)).filter_map(|e| Some((e.id, e.typed::<Wall>().ok()?))).collect()
}

fn wall_label(id: Id) -> String {
    id.to_string().chars().take(8).collect()
}

/// A wall along its own axis: `along` runs from `lo` to `hi`, `line` is the centre line across it.
#[derive(Debug, Clone, Copy)]
struct Ax {
    horiz: bool,
    line: i64,
    lo: i64,
    hi: i64,
}

fn ax(w: &Wall) -> Ax {
    if w.horizontal() { Ax { horiz: true, line: w.y1, lo: w.x1.min(w.x2), hi: w.x1.max(w.x2) } } else { Ax { horiz: false, line: w.x1, lo: w.y1.min(w.y2), hi: w.y1.max(w.y2) } }
}

fn xy(horiz: bool, along: i64, across: i64) -> (i64, i64) {
    if horiz { (along, across) } else { (across, along) }
}

// ------------------------------------------------------------------ snapping

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapKind {
    Endpoint,
    Intersection,
    Midpoint,
    Perpendicular,
    /// A point on a grid line.
    Online,
    Aligned,
    Grid,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snap {
    pub p: [i64; 2],
    pub kind: SnapKind,
}

#[derive(Debug, Clone)]
pub struct SnapCtx {
    /// Screen pixels per millimetre: the snap radius and the grid step follow the zoom.
    pub scale: f64,
    /// The start of what is being drawn.
    pub from: Option<P>,
    /// Hold the direction to a horizontal or vertical axis from `from` (always so while drawing a wall).
    pub ortho: bool,
    pub exclude: Vec<Id>,
}

pub const SNAP_RADIUS_PX: f64 = 10.0;
const GRID_STEPS: [i64; 8] = [10, 25, 50, 100, 250, 500, 1000, 5000];

/// The smallest grid step that is still at least 10 screen pixels wide.
pub fn grid_step(scale: f64) -> i64 {
    GRID_STEPS.iter().copied().find(|s| *s as f64 * scale >= 10.0).unwrap_or(5000)
}

/// Snaps a pointer position (world millimetres) to wall ends, wall crossings, wall middles, columns, the foot of a perpendicular
/// from `from`, then the grid, then alignment with `from`.
pub fn snap(model: &Model, level: Option<Id>, world: P, ctx: &SnapCtx) -> Snap {
    let radius = SNAP_RADIUS_PX / ctx.scale.max(1e-9);
    // While drawing orthogonally the pointer is first held on the axis from the start point.
    let axis_horiz = ctx.from.filter(|_| ctx.ortho).map(|f| (world[0] - f[0]).abs() >= (world[1] - f[1]).abs());
    let held: P = match (ctx.from, axis_horiz) {
        (Some(f), Some(true)) => [world[0], f[1]],
        (Some(f), Some(false)) => [f[0], world[1]],
        _ => world,
    };
    let on_axis = |p: P| -> Option<P> {
        match (ctx.from, axis_horiz) {
            (Some(f), Some(true)) => ((p[1] - f[1]).abs() <= radius).then_some([p[0], f[1]]),
            (Some(f), Some(false)) => ((p[0] - f[0]).abs() <= radius).then_some([f[0], p[1]]),
            _ => Some(p),
        }
    };
    let round = |p: P, kind: SnapKind| Snap { p: [p[0].round() as i64, p[1].round() as i64], kind };
    let d = |a: P, b: P| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();

    let walls: Vec<(Id, Wall)> = walls_on(model, level).into_iter().filter(|(id, _)| !ctx.exclude.contains(id)).collect();
    let mut points: Vec<P> = Vec::new();
    let mut lines: Vec<(P, P)> = Vec::new();
    for (_, w) in &walls {
        let (a, b) = ([w.x1 as f64, w.y1 as f64], [w.x2 as f64, w.y2 as f64]);
        points.push(a);
        points.push(b);
        lines.push((a, b));
    }
    // Grid lines belong to the project, not to a level.
    let grids: Vec<Grid> = model.elements.values().filter(|e| e.kind == Grid::KIND && !ctx.exclude.contains(&e.id)).filter_map(|e| e.typed::<Grid>().ok()).collect();
    for g in &grids {
        let (a, b) = ([g.x1 as f64, g.y1 as f64], [g.x2 as f64, g.y2 as f64]);
        points.push(a);
        points.push(b);
        lines.push((a, b));
    }
    // Room separators are drawn like walls: their ends, crossings and middles attract the pointer.
    let seps: Vec<RoomSeparator> = model.elements.values().filter(|e| e.kind == RoomSeparator::KIND && on_level(e, level) && !ctx.exclude.contains(&e.id)).filter_map(|e| e.typed::<RoomSeparator>().ok()).collect();
    for s in &seps {
        let (a, b) = ([s.x1 as f64, s.y1 as f64], [s.x2 as f64, s.y2 as f64]);
        points.push(a);
        points.push(b);
        lines.push((a, b));
    }
    for e in model.elements.values().filter(|e| e.kind == Column::KIND && on_level(e, level) && !ctx.exclude.contains(&e.id)) {
        if let Ok(c) = e.typed::<Column>() {
            points.push([c.x as f64, c.y as f64]);
        }
    }
    let best = |cands: &[P]| -> Option<P> { cands.iter().filter_map(|p| on_axis(*p)).filter(|p| d(held, *p) <= radius).min_by(|a, b| d(held, *a).total_cmp(&d(held, *b))) };
    if let Some(p) = best(&points) {
        return round(p, SnapKind::Endpoint);
    }
    // Where two centre lines cross (a horizontal and a vertical one), walls and grid lines alike.
    let mut crossings = Vec::new();
    let segs: Vec<(bool, i64, i64, i64)> = walls.iter().map(|(_, w)| (w.horizontal(), if w.horizontal() { w.y1 } else { w.x1 }, if w.horizontal() { w.x1.min(w.x2) } else { w.y1.min(w.y2) }, if w.horizontal() { w.x1.max(w.x2) } else { w.y1.max(w.y2) })).chain(grids.iter().map(|g| (g.horizontal(), g.coord(), if g.horizontal() { g.x1.min(g.x2) } else { g.y1.min(g.y2) }, if g.horizontal() { g.x1.max(g.x2) } else { g.y1.max(g.y2) }))).chain(seps.iter().map(|s| (s.horizontal(), if s.horizontal() { s.y1 } else { s.x1 }, if s.horizontal() { s.x1.min(s.x2) } else { s.y1.min(s.y2) }, if s.horizontal() { s.x1.max(s.x2) } else { s.y1.max(s.y2) }))).collect();
    for (_, y, hx0, hx1) in segs.iter().filter(|s| s.0) {
        for (_, x, vy0, vy1) in segs.iter().filter(|s| !s.0) {
            if (*hx0..=*hx1).contains(x) && (*vy0..=*vy1).contains(y) {
                crossings.push([*x as f64, *y as f64]);
            }
        }
    }
    if let Some(p) = best(&crossings) {
        return round(p, SnapKind::Intersection);
    }
    let mids: Vec<P> = lines.iter().map(|(a, b)| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]).collect();
    if let Some(p) = best(&mids) {
        return round(p, SnapKind::Midpoint);
    }
    // The foot of the perpendicular from the start point on a wall (only when not holding an axis, where it would be a wall end).
    if let (Some(f), None) = (ctx.from, axis_horiz) {
        let feet: Vec<P> = lines
            .iter()
            .filter_map(|(a, b)| if a[1] == b[1] { (f[0] >= a[0].min(b[0]) && f[0] <= a[0].max(b[0])).then_some([f[0], a[1]]) } else { (f[1] >= a[1].min(b[1]) && f[1] <= a[1].max(b[1])).then_some([a[0], f[1]]) })
            .collect();
        if let Some(p) = best(&feet) {
            return round(p, SnapKind::Perpendicular);
        }
    }
    // Along a grid line: the nearest point on it, so a wall can follow a grid.
    let online: Vec<P> = grids
        .iter()
        .filter_map(|g| {
            let (lo, hi) = if g.horizontal() { (g.x1.min(g.x2) as f64, g.x1.max(g.x2) as f64) } else { (g.y1.min(g.y2) as f64, g.y1.max(g.y2) as f64) };
            if g.horizontal() { (held[0] >= lo && held[0] <= hi).then_some([held[0], g.y1 as f64]) } else { (held[1] >= lo && held[1] <= hi).then_some([g.x1 as f64, held[1]]) }
        })
        .collect();
    if let Some(p) = best(&online) {
        return round(p, SnapKind::Online);
    }
    let step = grid_step(ctx.scale) as f64;
    if let (Some(f), Some(h)) = (ctx.from, axis_horiz) {
        let run = if h { held[0] - f[0] } else { held[1] - f[1] };
        let run = (run / step).round() * step;
        return round(if h { [f[0] + run, f[1]] } else { [f[0], f[1] + run] }, SnapKind::Grid);
    }
    let mut p = [(held[0] / step).round() * step, (held[1] / step).round() * step];
    let mut kind = SnapKind::Grid;
    if let Some(f) = ctx.from {
        let tol = 5.0 / ctx.scale.max(1e-9);
        if (held[0] - f[0]).abs() < tol {
            p[0] = f[0];
            kind = SnapKind::Aligned;
        }
        if (held[1] - f[1]).abs() < tol {
            p[1] = f[1];
            kind = SnapKind::Aligned;
        }
    }
    round(p, kind)
}

// ------------------------------------------------------------------ rooms beside a wall

struct RoomRect {
    key: String,
    x: i64,
    y: i64,
    w: i64,
    h: i64,
}

fn room_rects(model: &Model, level: Option<Id>) -> Vec<RoomRect> {
    model.elements.values().filter(|e| e.kind == Space::KIND && on_level(e, level)).filter_map(|e| e.typed::<Space>().ok()).map(|s| RoomRect { key: s.requirement_key, x: s.x, y: s.y, w: s.w, h: s.h }).collect()
}

fn room_at(rects: &[RoomRect], x: i64, y: i64) -> Option<String> {
    rects.iter().find(|r| x > r.x && x < r.x + r.w && y > r.y && y < r.y + r.h).map(|r| r.key.clone())
}

/// The room on each side of a wall (the lower and the higher side across its axis), from the rooms' rectangles.
fn rooms_beside(rects: &[RoomRect], a: Ax, thickness: i64) -> (Option<String>, Option<String>) {
    let mid = (a.lo + a.hi) / 2;
    let off = thickness / 2 + 20;
    let (x0, y0) = xy(a.horiz, mid, a.line - off);
    let (x1, y1) = xy(a.horiz, mid, a.line + off);
    (room_at(rects, x0, y0), room_at(rects, x1, y1))
}

/// The room on each side of a wall at `along` (a position on its axis), by the name of the whole room: a rectangle that is one part of a
/// room answers with that room. A long wall passes several rooms, so an opening asks at its own position, not at the wall's middle.
fn rooms_at(model: &Model, level: Option<Id>, a: Ax, thickness: i64, along: i64) -> (Option<String>, Option<String>) {
    let spaces: Vec<Space> = model.elements.values().filter(|e| e.kind == Space::KIND && on_level(e, level)).filter_map(|e| e.typed::<Space>().ok()).collect();
    let whole = |x: i64, y: i64| spaces.iter().find(|s| x > s.x && x < s.x + s.w && y > s.y && y < s.y + s.h).map(|s| crate::annot::part_of(s).unwrap_or(&s.requirement_key).to_string());
    let off = thickness / 2 + 20;
    let (x0, y0) = xy(a.horiz, along, a.line - off);
    let (x1, y1) = xy(a.horiz, along, a.line + off);
    (whole(x0, y0), whole(x1, y1))
}

/// `(from, to)` of a door from the rooms either side of it: two rooms, or one room and the outside. Where the rooms' rectangles say
/// nothing (no room found on either side), the host wall's own rooms answer.
fn door_rooms(wall: &Wall, lo: &Option<String>, hi: &Option<String>) -> (String, String) {
    match (lo, hi) {
        (Some(a), Some(b)) if a != b => (a.clone(), b.clone()),
        (Some(a), Some(_)) => (a.clone(), String::new()),
        (Some(a), None) | (None, Some(a)) => (a.clone(), "out".into()),
        (None, None) => (wall.room_a.clone(), wall.room_b.clone().unwrap_or_else(|| if wall.is_exterior { "out".into() } else { String::new() })),
    }
}

/// After the level's rooms were found or changed: the rooms that the doors and windows the user drew (or that were read from a drawing)
/// name, derived again from where each one stands. A door placed before its room existed, or whose room was renumbered, names the
/// right rooms here. Openings of a generated layout are left as they are.
pub fn refresh_openings(model: &Model, level: Option<Id>) -> Vec<Command> {
    let mut cmds = Vec::new();
    let mut set = |id: Id, key: &str, old: &str, new: &str| {
        if old != new {
            cmds.push(Command::SetProp { id, key: key.into(), value: Some(json!(new)) });
        }
    };
    for e in model.elements.values().filter(|e| (e.kind == Door::KIND || e.kind == Window::KIND) && on_level(e, level) && e.provenance.source == Source::User) {
        let Some(wall) = e.host_id.and_then(|h| model.get(&h)).and_then(|h| h.typed::<Wall>().ok()) else { continue };
        let a = ax(&wall);
        if let Ok(d) = e.typed::<Door>()
            && e.kind == Door::KIND
        {
            let (lo, hi) = rooms_at(model, level, a, wall.thickness, a.lo + d.offset_center);
            let (from, to) = door_rooms(&wall, &lo, &hi);
            // The side it opens into stays the side it was on: the room that was `to` is now the new `to`.
            let into = if !d.to.is_empty() && d.opens_into == d.to && !to.is_empty() && to != "out" { to.clone() } else { from.clone() };
            set(e.id, "from", &d.from, &from);
            set(e.id, "to", &d.to, &to);
            set(e.id, "opensInto", &d.opens_into, &into);
        } else if let Ok(w) = e.typed::<Window>() {
            let (lo, hi) = rooms_at(model, level, a, wall.thickness, a.lo + w.offset_center);
            if let Some(room) = lo.or(hi) {
                set(e.id, "room", &w.room, &room);
            }
        }
    }
    cmds
}

/// `(room_a, room_b, is_exterior)` of a wall: its rooms from the geometry; exterior when exactly one side is inside a room.
fn wall_rooms(rects: &[RoomRect], a: Ax, thickness: i64) -> (String, Option<String>, bool) {
    let (lo, hi) = rooms_beside(rects, a, thickness);
    let exterior = lo.is_some() != hi.is_some();
    match (lo, hi) {
        (Some(x), Some(y)) if x != y => (x, Some(y), false),
        (Some(x), Some(_)) => (x, None, false),
        (Some(x), None) | (None, Some(x)) => (x, None, exterior),
        (None, None) => (String::new(), None, false),
    }
}

// ------------------------------------------------------------------ walls

#[derive(Debug, Clone, Deserialize)]
pub struct WallSpec {
    pub a: [i64; 2],
    pub b: [i64; 2],
    pub thickness: i64,
    pub height: i64,
}

fn wall_of(spec: &WallSpec, a: Ax, rects: &[RoomRect]) -> Wall {
    let (room_a, room_b, is_exterior) = wall_rooms(rects, a, spec.thickness);
    let (a2, b2) = if spec.a[0] < spec.b[0] || spec.a[1] < spec.b[1] { (spec.a, spec.b) } else { (spec.b, spec.a) };
    Wall { x1: a2[0], y1: a2[1], x2: b2[0], y2: b2[1], thickness: spec.thickness, height: spec.height, length: a.hi - a.lo, is_exterior, room_a, room_b, extra: Default::default() }
}

/// A new wall between two points. Refused when it is not horizontal or vertical, is shorter than 100 mm, or lies over a wall already
/// there.
pub fn wall_command(model: &Model, level: Option<Id>, spec: &WallSpec) -> Result<(Command, Id), String> {
    let probe = Wall { x1: spec.a[0], y1: spec.a[1], x2: spec.b[0], y2: spec.b[1], thickness: spec.thickness, height: spec.height, length: 0, is_exterior: false, room_a: String::new(), room_b: None, extra: Default::default() };
    if (probe.x1 == probe.x2) == (probe.y1 == probe.y2) {
        return Err(if probe.x1 == probe.x2 { "the two points are the same".into() } else { "walls are drawn horizontal or vertical: the points must share an x or a y".into() });
    }
    let a = ax(&probe);
    if a.hi - a.lo < MIN_WALL_MM {
        return Err(format!("a wall must be at least {MIN_WALL_MM} mm long"));
    }
    if spec.thickness <= 0 || spec.height <= 0 {
        return Err("enter the wall's thickness and height (both above zero)".into());
    }
    for (id, w) in walls_on(model, level) {
        let o = ax(&w);
        if o.horiz == a.horiz && o.line == a.line && o.lo.max(a.lo) < o.hi.min(a.hi) {
            return Err(format!("a wall ({}) already runs along that line there: edit or split it instead", wall_label(id)));
        }
    }
    let rects = room_rects(model, level);
    let wall = wall_of(spec, a, &rects);
    let mut el = element_of(&wall, Source::User);
    el.level_id = level;
    let id = el.id;
    Ok((Command::Create(el), id))
}

// ------------------------------------------------------------------ doors and windows

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpeningKind {
    Door,
    Window,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OpeningSpec {
    pub kind: OpeningKind,
    pub wall: Id,
    /// Distance from the wall's start (its lower end) to the centre of the opening.
    pub offset: i64,
    pub width: i64,
    pub height: i64,
    /// Windows only.
    pub sill: Option<i64>,
    /// Which side of the wall the pointer was on (-1 lower, 1 higher): the door opens into the room on that side.
    pub side: Option<i64>,
}

fn opening_interval(model: &Model, id: &Id) -> Option<(i64, i64)> {
    let e = model.get(id)?;
    match e.kind.as_str() {
        "Door" => e.typed::<Door>().ok().map(|d| (d.offset_center - d.width / 2, d.offset_center + d.width / 2)),
        "Window" => e.typed::<Window>().ok().map(|w| (w.offset_center - w.width / 2, w.offset_center + w.width / 2)),
        _ => None,
    }
}

/// A door or window in a wall. Refused when it does not fit, overlaps another opening in that wall, or would stand in a column.
pub fn opening_command(model: &Model, spec: &OpeningSpec) -> Result<(Vec<Command>, Id), String> {
    let host = model.get(&spec.wall).ok_or("that wall no longer exists")?;
    let wall = host.typed::<Wall>().map_err(|_| "the host is not a wall")?;
    if spec.width <= 0 || spec.height <= 0 {
        return Err("enter the width and the height (both above zero)".into());
    }
    let (lo, hi) = (spec.offset - spec.width / 2, spec.offset + spec.width / 2);
    if lo < EDGE_MARGIN_MM || hi > wall.length - EDGE_MARGIN_MM {
        return Err(format!("a {} mm opening does not fit a {} mm wall with {EDGE_MARGIN_MM} mm left at each end", spec.width, wall.length));
    }
    for dep in model.dependents(&spec.wall) {
        if let Some((a, b)) = opening_interval(model, &dep)
            && lo < b
            && hi > a
        {
            return Err(format!("it would overlap another opening ({}) in that wall", wall_label(dep)));
        }
    }
    let a = ax(&wall);
    let (rlo, rhi) = rooms_at(model, host.level_id, a, wall.thickness, a.lo + spec.offset);
    let mut el = match spec.kind {
        OpeningKind::Door => {
            let (from, to) = door_rooms(&wall, &rlo, &rhi);
            let opens_into = match spec.side {
                Some(s) if s < 0 => rlo.clone(),
                Some(_) => rhi.clone(),
                None => None,
            }
            .unwrap_or_else(|| from.clone());
            element_of(&Door { width: spec.width, height: spec.height, offset_center: spec.offset, is_egress: false, from, to, opens_into, extra: Default::default() }, Source::User)
        }
        OpeningKind::Window => {
            let sill = spec.sill.ok_or("enter the window's sill height")?;
            if sill < 0 {
                return Err("the sill height must be zero or more".into());
            }
            let room = rlo.or(rhi).unwrap_or_else(|| wall.room_a.clone());
            element_of(&Window { width: spec.width, height: spec.height, sill_height: sill, offset_center: spec.offset, room, extra: Default::default() }, Source::User)
        }
    };
    el.level_id = host.level_id;
    el.host_id = Some(spec.wall);
    let id = el.id;
    // A position that would stand in a column is refused, not moved behind the user's back.
    let mut probe = crate::Document::new(model.clone());
    probe.commit("probe", crate::Origin::User, vec![Command::Create(el.clone())]).map_err(|e| e.to_string())?;
    if building::stands_in_column(&probe.model, &id) {
        return Err("that position would put the opening in a column: choose another".into());
    }
    Ok((vec![Command::Create(el)], id))
}

#[derive(Debug, Clone, Serialize)]
pub struct Placement {
    pub wall: String,
    pub offset: i64,
    pub fits: bool,
    pub distance: f64,
    /// -1 when the pointer is on the lower side of the wall, 1 on the higher side.
    pub side: i64,
    pub room: Option<String>,
    pub length: i64,
    /// Centre of the opening in plan, and the direction along the wall (for the preview).
    pub center: [i64; 2],
    pub horizontal: bool,
}

/// The wall a door or window of `width` would go in for a pointer position, and how far along it. The offset snaps to 50 mm, and to the
/// wall's middle when the pointer is near it.
pub fn placement(model: &Model, level: Option<Id>, world: P, width: i64, scale: f64) -> Option<Placement> {
    let reach = 12.0 / scale.max(1e-9);
    let mut best: Option<(f64, Id, Wall)> = None;
    for (id, w) in walls_on(model, level) {
        let a = ax(&w);
        let (al, ac) = if a.horiz { (world[0], world[1]) } else { (world[1], world[0]) };
        if al < a.lo as f64 - reach || al > a.hi as f64 + reach {
            continue;
        }
        let dist = (ac - a.line as f64).abs();
        if dist <= w.thickness as f64 / 2.0 + reach && best.as_ref().is_none_or(|b| dist < b.0) {
            best = Some((dist, id, w));
        }
    }
    let (distance, id, w) = best?;
    let a = ax(&w);
    let al = if a.horiz { world[0] } else { world[1] } - a.lo as f64;
    let mut offset = ((al / OFFSET_STEP_MM as f64).round() as i64) * OFFSET_STEP_MM;
    if (al - w.length as f64 / 2.0).abs() < 8.0 / scale.max(1e-9) {
        offset = w.length / 2;
    }
    let (min, max) = (width / 2 + EDGE_MARGIN_MM, w.length - width / 2 - EDGE_MARGIN_MM);
    let fits = max >= min;
    offset = if fits { offset.clamp(min, max) } else { w.length / 2 };
    let across = if a.horiz { world[1] } else { world[0] };
    let side = if across < a.line as f64 { -1 } else { 1 };
    let rects = room_rects(model, level);
    let (lo, hi) = rooms_beside(&rects, a, w.thickness);
    let c = xy(a.horiz, a.lo + offset, a.line);
    Some(Placement { wall: id.to_string(), offset, fits, distance, side, room: if side < 0 { lo } else { hi }, length: w.length, center: [c.0, c.1], horizontal: a.horiz })
}

// ------------------------------------------------------------------ selection by box

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoxMode {
    /// Takes what the box fully contains (dragged left to right).
    Window,
    /// Takes what the box touches (dragged right to left).
    Crossing,
}

fn rect_hit(mode: BoxMode, b: [f64; 4], r: [f64; 4]) -> bool {
    match mode {
        BoxMode::Window => r[0] >= b[0] && r[1] >= b[1] && r[2] <= b[2] && r[3] <= b[3],
        BoxMode::Crossing => r[0] <= b[2] && r[2] >= b[0] && r[1] <= b[3] && r[3] >= b[1],
    }
}

/// The walls, doors, windows and columns of the level inside or touching the box `[x0, y0, x1, y1]`.
pub fn pick_box(model: &Model, level: Option<Id>, b: [f64; 4], mode: BoxMode) -> Vec<Id> {
    let b = [b[0].min(b[2]), b[1].min(b[3]), b[0].max(b[2]), b[1].max(b[3])];
    let mut out = Vec::new();
    for e in model.elements.values().filter(|e| e.kind == Grid::KIND || on_level(e, level)) {
        let r: Option<[f64; 4]> = match e.kind.as_str() {
            "Grid" => e.typed::<Grid>().ok().map(|g| [g.x1.min(g.x2) as f64, g.y1.min(g.y2) as f64, g.x1.max(g.x2) as f64, g.y1.max(g.y2) as f64]),
            "Dimension" => e.typed::<Dimension>().ok().map(|d| {
                let (a, b) = crate::annot::dim_positions(model, &d);
                let (lo, hi) = (a.min(b) as f64, a.max(b) as f64);
                let (c0, c1) = (d.oa.min(d.ob).min(d.at) as f64, d.oa.max(d.ob).max(d.at) as f64);
                if d.axis == "x" { [lo, c0, hi, c1] } else { [c0, lo, c1, hi] }
            }),
            "Wall" => e.typed::<Wall>().ok().map(|w| {
                let t = w.thickness as f64 / 2.0;
                [w.x1.min(w.x2) as f64 - if w.horizontal() { 0.0 } else { t }, w.y1.min(w.y2) as f64 - if w.horizontal() { t } else { 0.0 }, w.x1.max(w.x2) as f64 + if w.horizontal() { 0.0 } else { t }, w.y1.max(w.y2) as f64 + if w.horizontal() { t } else { 0.0 }]
            }),
            "RoomSeparator" => e.typed::<RoomSeparator>().ok().map(|s| [s.x1.min(s.x2) as f64, s.y1.min(s.y2) as f64, s.x1.max(s.x2) as f64, s.y1.max(s.y2) as f64]),
            "Column" => e.typed::<Column>().ok().map(|c| [(c.x - c.b / 2) as f64, (c.y - c.h / 2) as f64, (c.x + c.b / 2) as f64, (c.y + c.h / 2) as f64]),
            "Door" | "Window" => {
                let host = e.host_id.and_then(|h| model.get(&h)).and_then(|h| h.typed::<Wall>().ok());
                let (off, width) = match e.kind.as_str() {
                    "Door" => e.typed::<Door>().ok().map(|d| (d.offset_center, d.width)),
                    _ => e.typed::<Window>().ok().map(|w| (w.offset_center, w.width)),
                }
                .unzip();
                match (host, off, width) {
                    (Some(w), Some(o), Some(wd)) => {
                        let a = ax(&w);
                        let (x0, y0) = xy(a.horiz, a.lo + o - wd / 2, a.line - w.thickness / 2);
                        let (x1, y1) = xy(a.horiz, a.lo + o + wd / 2, a.line + w.thickness / 2);
                        Some([x0 as f64, y0 as f64, x1 as f64, y1 as f64])
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        if r.is_some_and(|r| rect_hit(mode, b, r)) {
            out.push(e.id);
        }
    }
    out
}

// ------------------------------------------------------------------ move, copy, rotate, mirror

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Xf {
    Move { dx: i64, dy: i64 },
    /// Quarter turns counter-clockwise about a point (negative: clockwise).
    Rotate { cx: i64, cy: i64, turns: i64 },
    /// Reflection in the vertical line `x = at` (`axis: "x"`) or the horizontal line `y = at` (`axis: "y"`).
    Mirror { axis: char, at: i64 },
}

fn tp(xf: Xf, p: (i64, i64)) -> (i64, i64) {
    match xf {
        Xf::Move { dx, dy } => (p.0 + dx, p.1 + dy),
        Xf::Rotate { cx, cy, turns } => {
            let (mut x, mut y) = (p.0 - cx, p.1 - cy);
            for _ in 0..turns.rem_euclid(4) {
                (x, y) = (-y, x);
            }
            (x + cx, y + cy)
        }
        Xf::Mirror { axis: 'x', at } => (2 * at - p.0, p.1),
        Xf::Mirror { at, .. } => (p.0, 2 * at - p.1),
    }
}

fn swaps_axes(xf: Xf) -> bool {
    matches!(xf, Xf::Rotate { turns, .. } if turns.rem_euclid(2) == 1)
}

fn start_point(w: &Wall) -> (i64, i64) {
    let a = ax(w);
    xy(a.horiz, a.lo, a.line)
}

fn along(w: &Wall, p: (i64, i64)) -> i64 {
    if w.horizontal() { p.0 - w.x1.min(w.x2) } else { p.1 - w.y1.min(w.y2) }
}

fn point_on(w: &Wall, off: i64) -> (i64, i64) {
    let s = start_point(w);
    if w.horizontal() { (s.0 + off, s.1) } else { (s.0, s.1 + off) }
}

fn moved_wall(w: &Wall, xf: Xf) -> Wall {
    let (a, b) = (tp(xf, (w.x1, w.y1)), tp(xf, (w.x2, w.y2)));
    let (a, b) = if a.0 < b.0 || a.1 < b.1 { (a, b) } else { (b, a) };
    Wall { x1: a.0, y1: a.1, x2: b.0, y2: b.1, ..w.clone() }
}

fn props_cmds(id: Id, old: &Element, new: &Value) -> Vec<Command> {
    let Value::Object(new) = new else { return Vec::new() };
    let mut cmds: Vec<Command> = new.iter().filter(|(k, v)| old.props.get(*k) != Some(*v)).map(|(k, v)| Command::SetProp { id, key: k.clone(), value: Some(v.clone()) }).collect();
    cmds.extend(old.props.keys().filter(|k| !new.contains_key(*k)).map(|k| Command::SetProp { id, key: k.clone(), value: None }));
    cmds
}

/// What a draft edit does.
#[derive(Debug, Clone, Default)]
pub struct Edit {
    pub cmds: Vec<Command>,
    /// Ids of the elements a copy created.
    pub created: Vec<Id>,
    pub notes: Vec<String>,
}

/// Moves, copies, rotates (by quarter turns) or mirrors the selected walls and user columns. Doors and windows of a selected wall go
/// with it. A copy's walls get their rooms and exterior flag from where they land; a moved user wall gets them again, a moved
/// generated wall keeps its exterior flag.
pub fn xform(model: &Model, ids: &[Id], xf: Xf, copy: bool) -> Result<Edit, String> {
    let mut edit = Edit::default();
    let mut walls: Vec<(Id, Wall, Element)> = Vec::new();
    let mut columns: Vec<(Id, Column, Element)> = Vec::new();
    let mut grids: Vec<(Id, Grid, Element)> = Vec::new();
    let mut seps: Vec<(Id, RoomSeparator, Element)> = Vec::new();
    for id in ids {
        let e = model.get(id).ok_or("an element in the selection no longer exists")?;
        match e.kind.as_str() {
            "Wall" => walls.push((*id, e.typed::<Wall>().map_err(|x| x.to_string())?, e.clone())),
            "Column" => {
                if e.provenance.source == Source::Generated && !copy {
                    return Err("this column belongs to the generated structure and would be put back: copy it, or lock it first".into());
                }
                columns.push((*id, e.typed::<Column>().map_err(|x| x.to_string())?, e.clone()));
            }
            "Door" | "Window" => {
                if !e.host_id.is_some_and(|h| ids.contains(&h)) {
                    return Err("a door or window moves with its wall: select the wall too, or change its distance along the wall in the properties".into());
                }
            }
            "Grid" => grids.push((*id, e.typed::<Grid>().map_err(|x| x.to_string())?, e.clone())),
            "RoomSeparator" => seps.push((*id, e.typed::<RoomSeparator>().map_err(|x| x.to_string())?, e.clone())),
            "Dimension" | "Tag" => edit.notes.push("dimensions and room tags follow what they measure or tag: they were left as they are".into()),
            "Space" => return Err("rooms are not moved here: use the partition and room tools, which keep the plan tiled".into()),
            other => return Err(format!("{other} elements cannot be moved this way")),
        }
    }
    if walls.is_empty() && columns.is_empty() && grids.is_empty() && seps.is_empty() {
        return Err("select walls, room separators, columns or grid lines first".into());
    }
    let level = walls.first().map(|w| w.2.level_id).or(columns.first().map(|c| c.2.level_id)).or(seps.first().map(|s| s.2.level_id)).flatten();
    let rects = room_rects(model, level);
    for (id, w, el) in &walls {
        let nw = moved_wall(w, xf);
        let a = ax(&nw);
        let (ra, rb, ext) = wall_rooms(&rects, a, nw.thickness);
        let hosted: Vec<Id> = model.dependents(id);
        if copy {
            let mut c = nw.clone();
            (c.room_a, c.room_b, c.is_exterior) = (ra, rb, ext);
            let mut new_el = element_of(&c, Source::User);
            new_el.level_id = el.level_id;
            let new_id = new_el.id;
            edit.created.push(new_id);
            edit.cmds.push(Command::Create(new_el));
            for h in hosted {
                let Some(oe) = model.get(&h) else { continue };
                let off = oe.props.get("offsetCenter").and_then(Value::as_i64).unwrap_or(0);
                let c2 = tp(xf, point_on(w, off));
                let noff = along(&nw, c2);
                let mut ne = oe.clone();
                ne.id = Element::new(&oe.kind, oe.discipline, Source::User).id;
                ne.provenance = crate::model::Provenance { source: Source::User, locked: false };
                ne.rev = 0;
                ne.host_id = Some(new_id);
                ne.props.insert("offsetCenter".into(), json!(noff));
                edit.created.push(ne.id);
                edit.cmds.push(Command::Create(ne));
            }
        } else {
            let mut c = nw.clone();
            if el.provenance.source != Source::Generated {
                (c.room_a, c.room_b, c.is_exterior) = (ra, rb, ext);
            } else {
                c.room_a = ra;
                c.room_b = rb;
            }
            edit.cmds.extend(props_cmds(*id, el, &serde_json::to_value(&c).map_err(|e| e.to_string())?));
            for h in hosted {
                let Some(oe) = model.get(&h) else { continue };
                let off = oe.props.get("offsetCenter").and_then(Value::as_i64).unwrap_or(0);
                let noff = along(&nw, tp(xf, point_on(w, off)));
                if noff != off {
                    edit.cmds.push(Command::SetProp { id: h, key: "offsetCenter".into(), value: Some(json!(noff)) });
                }
            }
        }
    }
    let mut taken: std::collections::BTreeSet<String> = Default::default();
    for (id, g, el) in &grids {
        let (a, b) = (tp(xf, (g.x1, g.y1)), tp(xf, (g.x2, g.y2)));
        let (a, b) = if a.0 < b.0 || a.1 < b.1 { (a, b) } else { (b, a) };
        let mut n = g.clone();
        (n.x1, n.y1, n.x2, n.y2) = (a.0, a.1, b.0, b.1);
        if copy {
            n.name = crate::annot::next_grid_name_excluding(model, n.horizontal(), &taken);
            taken.insert(n.name.to_uppercase());
            let ne = element_of(&n, Source::User);
            edit.created.push(ne.id);
            edit.cmds.push(Command::Create(ne));
        } else {
            edit.cmds.extend(props_cmds(*id, el, &serde_json::to_value(&n).map_err(|e| e.to_string())?));
        }
    }
    for (id, s, el) in &seps {
        let (a, b) = (tp(xf, (s.x1, s.y1)), tp(xf, (s.x2, s.y2)));
        let (a, b) = if a.0 < b.0 || a.1 < b.1 { (a, b) } else { (b, a) };
        let n = RoomSeparator { x1: a.0, y1: a.1, x2: b.0, y2: b.1, extra: s.extra.clone() };
        if copy {
            let mut ne = element_of(&n, Source::User);
            ne.level_id = el.level_id;
            edit.created.push(ne.id);
            edit.cmds.push(Command::Create(ne));
        } else {
            edit.cmds.extend(props_cmds(*id, el, &serde_json::to_value(&n).map_err(|e| e.to_string())?));
        }
    }
    for (id, c, el) in &columns {
        let (x, y) = tp(xf, (c.x, c.y));
        let (b, h) = if swaps_axes(xf) { (c.h, c.b) } else { (c.b, c.h) };
        if copy {
            let mut n = c.clone();
            (n.x, n.y, n.b, n.h, n.grid_ref) = (x, y, b, h, String::new());
            let mut ne = element_of(&n, Source::User);
            ne.level_id = el.level_id;
            edit.created.push(ne.id);
            edit.cmds.push(Command::Create(ne));
        } else {
            let mut n = c.clone();
            (n.x, n.y, n.b, n.h) = (x, y, b, h);
            edit.cmds.extend(props_cmds(*id, el, &serde_json::to_value(&n).map_err(|e| e.to_string())?));
        }
    }
    Ok(edit)
}

/// Brings a wall's centre line to the line of a reference wall (they must be parallel); its doors and windows go with it.
pub fn align(model: &Model, reference: Id, target: Id) -> Result<Edit, String> {
    let r = model.get(&reference).and_then(|e| e.typed::<Wall>().ok()).ok_or("the reference is not a wall")?;
    let t = model.get(&target).and_then(|e| e.typed::<Wall>().ok()).ok_or("the wall to move is not a wall")?;
    if r.horizontal() != t.horizontal() {
        return Err("the two walls must be parallel".into());
    }
    let d = ax(&r).line - ax(&t).line;
    if d == 0 {
        return Err("the wall is already on that line".into());
    }
    let xf = if t.horizontal() { Xf::Move { dx: 0, dy: d } } else { Xf::Move { dx: d, dy: 0 } };
    xform(model, &[target], xf, false)
}

/// A parallel copy of a wall, room separator or grid line, `distance` mm away on the side of the point `toward` (the pointer). A wall's
/// doors and windows are copied with it; a copied grid line is named like any new one.
pub fn offset(model: &Model, id: Id, distance: i64, toward: P) -> Result<Edit, String> {
    if distance <= 0 {
        return Err("enter an offset distance above zero (mm)".into());
    }
    let e = model.get(&id).ok_or("that element no longer exists")?;
    let (horiz, line) = match e.kind.as_str() {
        "Wall" => {
            let a = ax(&e.typed::<Wall>().map_err(|x| x.to_string())?);
            (a.horiz, a.line)
        }
        "RoomSeparator" => {
            let s = e.typed::<RoomSeparator>().map_err(|x| x.to_string())?;
            (s.horizontal(), if s.horizontal() { s.y1 } else { s.x1 })
        }
        "Grid" => {
            let g = e.typed::<Grid>().map_err(|x| x.to_string())?;
            (g.horizontal(), g.coord())
        }
        _ => return Err("only walls, room separators and grid lines can be offset".into()),
    };
    let side = if horiz { toward[1] - line as f64 } else { toward[0] - line as f64 };
    let d = if side >= 0.0 { distance } else { -distance };
    xform(model, &[id], if horiz { Xf::Move { dx: 0, dy: d } } else { Xf::Move { dx: d, dy: 0 } }, true)
}

// ------------------------------------------------------------------ split and trim

fn rebuilt(id: Id, el: &Element, w: &Wall) -> Result<Vec<Command>, String> {
    Ok(props_cmds(id, el, &serde_json::to_value(w).map_err(|e| e.to_string())?))
}

fn with_ends(w: &Wall, lo: i64, hi: i64) -> Wall {
    let a = ax(w);
    let (p, q) = (xy(a.horiz, lo, a.line), xy(a.horiz, hi, a.line));
    Wall { x1: p.0, y1: p.1, x2: q.0, y2: q.1, length: hi - lo, ..w.clone() }
}

/// Cuts a wall in two at the position `at` (a coordinate along the wall's axis: x for a horizontal wall, y for a vertical one). Doors
/// and windows stay with their half; one that straddles the cut refuses it.
pub fn split(model: &Model, wall: Id, at: i64) -> Result<Edit, String> {
    let el = model.get(&wall).ok_or("that wall no longer exists")?;
    let w = el.typed::<Wall>().map_err(|_| "that is not a wall")?;
    let a = ax(&w);
    if at - a.lo < MIN_WALL_MM || a.hi - at < MIN_WALL_MM {
        return Err(format!("each part must be at least {MIN_WALL_MM} mm long"));
    }
    let mut edit = Edit::default();
    let mut second = element_of(&with_ends(&w, at, a.hi), Source::User);
    second.level_id = el.level_id;
    let second_id = second.id;
    let mut moved = Vec::new();
    for h in model.dependents(&wall) {
        let (lo, hi) = opening_interval(model, &h).ok_or("a hosted element is not a door or window")?;
        let (lo, hi) = (a.lo + lo, a.lo + hi);
        if lo < at && hi > at {
            return Err("a door or window crosses that point: move it or split elsewhere".into());
        }
        if lo >= at {
            moved.push(h);
        }
    }
    edit.cmds.extend(rebuilt(wall, el, &with_ends(&w, a.lo, at))?);
    edit.cmds.push(Command::Create(second));
    edit.created.push(second_id);
    for h in moved {
        let oe = model.get(&h).ok_or("a hosted element disappeared")?;
        let off = oe.props.get("offsetCenter").and_then(Value::as_i64).unwrap_or(0);
        let mut ne = oe.clone();
        ne.id = Element::new(&oe.kind, oe.discipline, oe.provenance.source).id;
        ne.rev = 0;
        ne.host_id = Some(second_id);
        ne.props.insert("offsetCenter".into(), json!(off - (at - a.lo)));
        edit.cmds.push(Command::Delete(h));
        edit.cmds.push(Command::Create(ne));
    }
    Ok(edit)
}

/// One wall across the stretch from `p` to `q` (two points on one horizontal or vertical line), made of what is already there: a wall
/// in that line that ends at `p` and one that starts at `q` are joined across the stretch into a single wall; with only one of them,
/// that wall is extended over the stretch; with neither, a new wall of the given thickness and height is drawn. Doors and windows of
/// the walls keep their place in the plan. Returns the edit, the wall that now covers the stretch, and where that wall starts along
/// the line. Two walls that differ in thickness or height are not joined: which of the two the stretch takes is not the program's to say.
pub fn bridge(model: &Model, level: Option<Id>, p: [i64; 2], q: [i64; 2], thickness: i64, height: i64) -> Result<(Edit, Id, i64), String> {
    if (p[0] == q[0]) == (p[1] == q[1]) {
        return Err("bridging a gap: its two ends must share an x or a y, and differ".into());
    }
    let horiz = p[1] == q[1];
    let (line, lo, hi) = if horiz { (p[1], p[0].min(q[0]), p[0].max(q[0])) } else { (p[0], p[1].min(q[1]), p[1].max(q[1])) };
    let in_line: Vec<(Id, Wall)> = walls_on(model, level).into_iter().filter(|(_, w)| { let a = ax(w); a.horiz == horiz && a.line == line }).collect();
    let before = in_line.iter().find(|(_, w)| ax(w).hi == lo);
    let after = in_line.iter().find(|(_, w)| ax(w).lo == hi);
    let mut edit = Edit::default();
    // Every door and window of `from`, as it must stand on a wall that starts at `start`: its distance from that start.
    let shifted = |from: Id, from_lo: i64, start: i64| -> Result<Vec<(Id, i64)>, String> {
        model.dependents(&from).into_iter().map(|h| { let (a, b) = opening_interval(model, &h).ok_or("a hosted element is not a door or window")?; Ok((h, from_lo + (a + b) / 2 - start)) }).collect()
    };
    match (before, after) {
        (None, None) => {
            let (cmd, id) = wall_command(model, level, &WallSpec { a: p, b: q, thickness, height })?;
            edit.cmds.push(cmd);
            edit.created.push(id);
            Ok((edit, id, lo))
        }
        (Some((id, w)), None) => {
            let el = model.get(id).ok_or("that wall no longer exists")?;
            edit.cmds.extend(rebuilt(*id, el, &with_ends(w, ax(w).lo, hi))?);
            Ok((edit, *id, ax(w).lo))
        }
        (None, Some((id, w))) => {
            let el = model.get(id).ok_or("that wall no longer exists")?;
            edit.cmds.extend(rebuilt(*id, el, &with_ends(w, lo, ax(w).hi))?);
            for (h, off) in shifted(*id, ax(w).lo, lo)? {
                edit.cmds.push(Command::SetProp { id: h, key: "offsetCenter".into(), value: Some(json!(off)) });
            }
            Ok((edit, *id, lo))
        }
        (Some((ia, wa)), Some((ib, wb))) => {
            if wa.thickness != wb.thickness || wa.height != wb.height {
                return Err(format!("bridging a gap: the walls at its two ends differ ({} x {} mm and {} x {} mm, thickness x height): close it with a wall and place the door by hand", wa.thickness, wa.height, wb.thickness, wb.height));
            }
            let el = model.get(ia).ok_or("that wall no longer exists")?;
            let start = ax(wa).lo;
            edit.cmds.extend(rebuilt(*ia, el, &with_ends(wa, start, ax(wb).hi))?);
            for (h, off) in shifted(*ib, ax(wb).lo, start)? {
                let oe = model.get(&h).ok_or("a hosted element disappeared")?;
                let mut ne = oe.clone();
                ne.id = Element::new(&oe.kind, oe.discipline, oe.provenance.source).id;
                ne.rev = 0;
                ne.host_id = Some(*ia);
                ne.props.insert("offsetCenter".into(), json!(off));
                edit.cmds.push(Command::Delete(h));
                edit.cmds.push(Command::Create(ne));
            }
            edit.cmds.push(Command::Delete(*ib));
            Ok((edit, *ia, start))
        }
    }
}

/// Brings two perpendicular walls to their common corner. Where the corner lies inside a wall, the part on the side of the click is
/// kept; where it lies outside, the nearer end is extended to it. Doors and windows keep their place in the plan; one that would end
/// up outside its wall refuses the edit.
pub fn trim_corner(model: &Model, a: (Id, P), b: (Id, P)) -> Result<Edit, String> {
    let get = |id: Id| -> Result<(Element, Wall), String> {
        let e = model.get(&id).ok_or("a wall no longer exists")?.clone();
        let w = e.typed::<Wall>().map_err(|_| "that is not a wall")?;
        Ok((e, w))
    };
    let ((ea, wa), (eb, wb)) = (get(a.0)?, get(b.0)?);
    if wa.horizontal() == wb.horizontal() {
        return Err("the two walls must be perpendicular (one horizontal, one vertical)".into());
    }
    let (h, v) = if wa.horizontal() { (&wa, &wb) } else { (&wb, &wa) };
    let corner = (ax(v).line, ax(h).line);
    let mut edit = Edit::default();
    for (id, el, w, pick) in [(a.0, &ea, &wa, a.1), (b.0, &eb, &wb, b.1)] {
        let x = ax(w);
        let c = if x.horiz { corner.0 } else { corner.1 };
        let p = if x.horiz { pick[0] } else { pick[1] };
        let (lo, hi) = if c > x.lo && c < x.hi {
            if p < c as f64 { (x.lo, c) } else { (c, x.hi) }
        } else if c <= x.lo {
            (c, x.hi)
        } else {
            (x.lo, c)
        };
        if hi - lo < MIN_WALL_MM {
            return Err(format!("the wall would be shorter than {MIN_WALL_MM} mm"));
        }
        if (lo, hi) == (x.lo, x.hi) {
            continue;
        }
        for h in model.dependents(&id) {
            let (ilo, ihi) = opening_interval(model, &h).ok_or("a hosted element is not a door or window")?;
            let (wlo, whi) = (x.lo + ilo, x.lo + ihi);
            if wlo < lo + EDGE_MARGIN_MM || whi > hi - EDGE_MARGIN_MM {
                return Err("a door or window would be left outside the trimmed wall: move or delete it first".into());
            }
            let off = x.lo + (ilo + ihi) / 2 - lo;
            edit.cmds.push(Command::SetProp { id: h, key: "offsetCenter".into(), value: Some(json!(off)) });
        }
        edit.cmds.extend(rebuilt(id, el, &with_ends(w, lo, hi))?);
    }
    if edit.cmds.is_empty() {
        return Err("the two walls already meet at their corner".into());
    }
    Ok(edit)
}

/// After the level's rooms were found or changed: the rooms either side and the exterior flag of the walls the user drew, derived again
/// from the rooms' rectangles. A wall drawn before its room existed (the walls of a blank project) learns that it is an exterior wall here.
/// Walls of a generated layout are left as they are.
pub fn refresh_walls(model: &Model, level: Option<Id>) -> Vec<Command> {
    let rects = room_rects(model, level);
    let mut cmds = Vec::new();
    for e in model.elements.values().filter(|e| e.kind == Wall::KIND && on_level(e, level) && e.provenance.source == Source::User) {
        let Ok(w) = e.typed::<Wall>() else { continue };
        let (ra, rb, ext) = wall_rooms(&rects, ax(&w), w.thickness);
        if ra != w.room_a {
            cmds.push(Command::SetProp { id: e.id, key: "roomA".into(), value: Some(json!(ra)) });
        }
        if rb != w.room_b {
            cmds.push(Command::SetProp { id: e.id, key: "roomB".into(), value: rb.map(|r| json!(r)) });
        }
        if ext != w.is_exterior {
            cmds.push(Command::SetProp { id: e.id, key: "isExterior".into(), value: Some(json!(ext)) });
        }
    }
    cmds
}

// ------------------------------------------------------------------ delete

/// Deletes walls (with their doors and windows), doors, windows and user columns.
pub fn delete(model: &Model, ids: &[Id]) -> Result<Edit, String> {
    let mut edit = Edit::default();
    let mut gone: Vec<Id> = Vec::new();
    for id in ids {
        let e = model.get(id).ok_or("an element in the selection no longer exists")?;
        match e.kind.as_str() {
            "Wall" => {
                for h in model.dependents(id) {
                    if !ids.contains(&h) && !gone.contains(&h) {
                        gone.push(h);
                        edit.notes.push(format!("its {} {} was deleted with the wall", model.get(&h).map_or("opening", |x| x.kind.as_str()).to_lowercase(), wall_label(h)));
                    }
                }
            }
            "Door" | "Window" | "Grid" | "Dimension" | "Tag" | "RoomSeparator" => {}
            "Column" if e.provenance.source != Source::Generated => {}
            "Column" => return Err("this column belongs to the generated structure and would be put back: lock it, or edit the plan it comes from".into()),
            "Space" => return Err("rooms are deleted with the room tool (it hands the area to a neighbour)".into()),
            other => return Err(format!("{other} elements are not deleted this way")),
        }
    }
    // What hangs on a deleted element (the tag of a door, window, column or footing) goes with it, and goes first: an element that hosts
    // another cannot be deleted before it.
    let mut all: Vec<Id> = ids.iter().copied().chain(gone).collect();
    let mut i = 0;
    while i < all.len() {
        for d in model.dependents(&all[i]) {
            if !all.contains(&d) {
                all.push(d);
            }
        }
        i += 1;
    }
    let depth = |id: &Id| {
        let (mut n, mut cur) = (0, *id);
        while let Some(h) = model.get(&cur).and_then(|e| e.host_id) {
            n += 1;
            cur = h;
        }
        n
    };
    all.sort_by_key(|id| std::cmp::Reverse(depth(id)));
    edit.cmds.extend(all.into_iter().map(Command::Delete));
    Ok(edit)
}

// ------------------------------------------------------------------ properties

#[derive(Debug, Clone, Serialize)]
pub struct Field {
    pub key: &'static str,
    pub en: &'static str,
    pub ar: &'static str,
    pub value: Value,
    /// `mm`, `bool` or `text`.
    pub unit: &'static str,
    pub editable: bool,
}

fn field(key: &'static str, en: &'static str, ar: &'static str, value: Value, unit: &'static str, editable: bool) -> Field {
    Field { key, en, ar, value, unit, editable }
}

/// The properties the inspector shows for an element, with the ones that can be edited.
pub fn properties(model: &Model, id: Id) -> Result<Value, String> {
    let e = model.get(&id).ok_or("that element no longer exists")?;
    let src = serde_json::to_value(e.provenance.source).unwrap_or(Value::Null);
    let mut f: Vec<Field> = Vec::new();
    match e.kind.as_str() {
        "Wall" => {
            let w = e.typed::<Wall>().map_err(|x| x.to_string())?;
            f.push(field("length", "Length", "الطول", json!(w.length), "mm", true));
            f.push(field("thickness", "Thickness", "السماكة", json!(w.thickness), "mm", true));
            f.push(field("height", "Height", "الارتفاع", json!(w.height), "mm", true));
            f.push(field("roomA", "Room (side 1)", "الغرفة (الجهة 1)", json!(w.room_a), "text", false));
            f.push(field("roomB", "Room (side 2)", "الغرفة (الجهة 2)", json!(w.room_b.clone().unwrap_or_default()), "text", false));
            f.push(field("isExterior", "Exterior", "خارجي", json!(w.is_exterior), "bool", false));
            let r = crate::assembly::Resolver::of(model);
            let explicit = e.props.get("assembly").and_then(Value::as_str).is_some();
            let name = r.of_wall(e, &w).map(|a| format!("{}{}", a.name, if explicit { "" } else { " (project default)" })).unwrap_or_default();
            f.push(field("assembly", "Wall assembly", "تركيب الجدار", json!(name), "text", false));
        }
        "Door" => {
            let d = e.typed::<Door>().map_err(|x| x.to_string())?;
            let len = host_length(model, e);
            f.push(field("width", "Width", "العرض", json!(d.width), "mm", true));
            f.push(field("height", "Height", "الارتفاع", json!(d.height), "mm", true));
            f.push(field("offsetCenter", "Distance along the wall (to centre)", "المسافة على الجدار (للمركز)", json!(d.offset_center), "mm", true));
            f.push(field("offsetFromEnd", "Distance from the far end (to centre)", "المسافة من الطرف الآخر (للمركز)", json!(len.map(|l| l - d.offset_center)), "mm", true));
            f.push(field("isEgress", "Egress door", "باب مخرج", json!(d.is_egress), "bool", true));
            f.push(field("from", "From room", "من غرفة", json!(d.from), "text", false));
            f.push(field("to", "To room", "إلى غرفة", json!(d.to), "text", false));
        }
        "Window" => {
            let w = e.typed::<Window>().map_err(|x| x.to_string())?;
            let len = host_length(model, e);
            f.push(field("width", "Width", "العرض", json!(w.width), "mm", true));
            f.push(field("height", "Height", "الارتفاع", json!(w.height), "mm", true));
            f.push(field("sillHeight", "Sill height", "ارتفاع الحافة", json!(w.sill_height), "mm", true));
            f.push(field("offsetCenter", "Distance along the wall (to centre)", "المسافة على الجدار (للمركز)", json!(w.offset_center), "mm", true));
            f.push(field("offsetFromEnd", "Distance from the far end (to centre)", "المسافة من الطرف الآخر (للمركز)", json!(len.map(|l| l - w.offset_center)), "mm", true));
            f.push(field("room", "Room", "الغرفة", json!(w.room), "text", false));
        }
        "Column" => {
            let c = e.typed::<Column>().map_err(|x| x.to_string())?;
            f.push(field("x", "X", "س", json!(c.x), "mm", true));
            f.push(field("y", "Y", "ص", json!(c.y), "mm", true));
            f.push(field("b", "Width", "العرض", json!(c.b), "mm", true));
            f.push(field("h", "Depth", "العمق", json!(c.h), "mm", true));
        }
        "Space" => {
            let s = e.typed::<Space>().map_err(|x| x.to_string())?;
            let labels: std::collections::BTreeMap<String, String> = crate::project::find(model).and_then(|p| p.props.get("labels").and_then(|l| l.as_object()).map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|t| (k.clone(), t.to_string()))).collect())).unwrap_or_default();
            let label = crate::annot::room_name(&s, &labels).unwrap_or_default();
            f.push(field("requirementKey", "Room key", "رمز الغرفة", json!(s.requirement_key), "text", false));
            f.push(field("name", "Name", "الاسم", json!(label), "text", true));
            f.push(field("func", "Function", "الوظيفة", json!(s.func), "text", true));
            f.push(field("area", "Area (m²)", "المساحة (م²)", json!(s.area), "text", false));
            f.push(field("w", "Width", "العرض", json!(s.w), "mm", false));
            f.push(field("h", "Length", "الطول", json!(s.h), "mm", false));
        }
        "RoomSeparator" => {
            let s = e.typed::<RoomSeparator>().map_err(|x| x.to_string())?;
            f.push(field("length", "Length", "الطول", json!((s.x2 - s.x1).abs() + (s.y2 - s.y1).abs()), "mm", false));
            f.push(field("direction", "Direction", "الاتجاه", json!(if s.horizontal() { "horizontal" } else { "vertical" }), "text", false));
            f.push(field("position", if s.horizontal() { "At y" } else { "At x" }, if s.horizontal() { "عند ص" } else { "عند س" }, json!(if s.horizontal() { s.y1 } else { s.x1 }), "mm", false));
        }
        "Grid" => {
            let g = e.typed::<Grid>().map_err(|x| x.to_string())?;
            f.push(field("name", "Name", "الاسم", json!(g.name), "text", true));
            f.push(field("direction", "Direction", "الاتجاه", json!(if g.horizontal() { "horizontal" } else { "vertical" }), "text", false));
            f.push(field("position", if g.horizontal() { "At y" } else { "At x" }, if g.horizontal() { "عند ص" } else { "عند س" }, json!(g.coord()), "mm", false));
        }
        "Dimension" => {
            let d = e.typed::<Dimension>().map_err(|x| x.to_string())?;
            let (a, b) = crate::annot::dim_positions(model, &d);
            f.push(field("length", "Length", "الطول", json!((b - a).abs()), "mm", false));
            f.push(field("at", if d.axis == "x" { "Line at y" } else { "Line at x" }, if d.axis == "x" { "الخط عند ص" } else { "الخط عند س" }, json!(d.at), "mm", true));
            f.push(field("attached", "Follows", "يتبع", json!(match (d.ref_a.is_some(), d.ref_b.is_some()) { (true, true) => "both ends", (false, false) => "nothing (fixed)", _ => "one end" }), "text", false));
        }
        "Tag" => {
            let t = e.typed::<crate::typed::Tag>().map_err(|x| x.to_string())?;
            f.push(field("showName", "Show name", "إظهار الاسم", json!(t.show_name), "bool", true));
            f.push(field("showArea", "Show area", "إظهار المساحة", json!(t.show_area), "bool", true));
            f.push(field("showElevation", "Show elevation", "إظهار المنسوب", json!(t.show_elevation), "bool", true));
            f.push(field("dx", "Offset x", "إزاحة س", json!(t.dx), "mm", true));
            f.push(field("dy", "Offset y", "إزاحة ص", json!(t.dy), "mm", true));
        }
        other => f.push(field("kind", "Kind", "النوع", json!(other), "text", false)),
    }
    Ok(json!({ "id": id.to_string(), "kind": e.kind, "source": src, "locked": e.provenance.locked, "level": e.level_id.map(|l| l.to_string()), "fields": f }))
}

fn host_length(model: &Model, e: &Element) -> Option<i64> {
    e.host_id.and_then(|h| model.get(&h)).and_then(|h| h.typed::<Wall>().ok()).map(|w| w.length)
}

/// Changes one property. The strict typed check at commit refuses what is invalid; what depends on other elements is checked here.
pub fn set_property(model: &Model, id: Id, key: &str, value: &Value) -> Result<Edit, String> {
    let e = model.get(&id).ok_or("that element no longer exists")?;
    let int = |what: &str| value.as_i64().ok_or_else(|| format!("{what}: enter a whole number of millimetres"));
    let mut edit = Edit::default();
    match (e.kind.as_str(), key) {
        ("Wall", "length") => {
            let w = e.typed::<Wall>().map_err(|x| x.to_string())?;
            let len = int("length")?;
            if len < MIN_WALL_MM {
                return Err(format!("a wall must be at least {MIN_WALL_MM} mm long"));
            }
            let a = ax(&w);
            for h in model.dependents(&id) {
                let (_, ihi) = opening_interval(model, &h).ok_or("a hosted element is not a door or window")?;
                if ihi > len - EDGE_MARGIN_MM {
                    return Err("a door or window would be left outside the shorter wall: move or delete it first".into());
                }
            }
            edit.cmds = rebuilt(id, e, &with_ends(&w, a.lo, a.lo + len))?;
        }
        ("Wall", "thickness") if e.props.get("assembly").and_then(Value::as_str).is_some() => {
            return Err("the thickness comes from the wall's assembly: change the assembly's layers, or clear the assembly from the wall first".into());
        }
        ("Wall", "thickness" | "height") | ("Door" | "Window", "width" | "height") | ("Window", "sillHeight") | ("Column", "x" | "y" | "b" | "h") => {
            let v = int(key)?;
            if matches!(key, "thickness" | "height" | "width" | "b" | "h") && v <= 0 {
                return Err(format!("{key} must be above zero"));
            }
            if key == "sillHeight" && v < 0 {
                return Err("the sill height must be zero or more".into());
            }
            edit.cmds.push(Command::SetProp { id, key: key.into(), value: Some(json!(v)) });
            if e.kind != "Wall" && e.kind != "Column" && key == "width" {
                let off = e.props.get("offsetCenter").and_then(Value::as_i64).unwrap_or(0);
                let len = host_length(model, e).unwrap_or(0);
                if off - v / 2 < EDGE_MARGIN_MM || off + v / 2 > len - EDGE_MARGIN_MM {
                    return Err(format!("a {v} mm opening does not fit its wall at that position ({EDGE_MARGIN_MM} mm is kept clear at each end)"));
                }
                for dep in model.dependents(&e.host_id.ok_or("no host")?).into_iter().filter(|d| *d != id) {
                    if let Some((a, b)) = opening_interval(model, &dep)
                        && off - v / 2 < b
                        && off + v / 2 > a
                    {
                        return Err(format!("it would overlap another opening ({}) in that wall", wall_label(dep)));
                    }
                }
            }
        }
        ("Door" | "Window", "offsetCenter" | "offsetFromEnd") => {
            let v = int(key)?;
            let len = host_length(model, e).ok_or("the opening has no wall")?;
            let off = if key == "offsetCenter" { v } else { len - v };
            let width = e.props.get("width").and_then(Value::as_i64).unwrap_or(0);
            if off - width / 2 < EDGE_MARGIN_MM || off + width / 2 > len - EDGE_MARGIN_MM {
                return Err(format!("the opening does not fit the {len} mm wall there ({EDGE_MARGIN_MM} mm is kept clear at each end)"));
            }
            for dep in model.dependents(&e.host_id.ok_or("no host")?).into_iter().filter(|d| *d != id) {
                if let Some((a, b)) = opening_interval(model, &dep)
                    && off - width / 2 < b
                    && off + width / 2 > a
                {
                    return Err(format!("it would overlap another opening ({}) in that wall", wall_label(dep)));
                }
            }
            edit.cmds.push(Command::SetProp { id, key: "offsetCenter".into(), value: Some(json!(off)) });
        }
        ("Grid", "name") | ("Space", "func") => {
            let t = value.as_str().map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| format!("{key}: enter a text"))?;
            edit.cmds.push(Command::SetProp { id, key: key.into(), value: Some(json!(t)) });
        }
        ("Space", "name") => {
            let key_room = e.props.get("requirementKey").and_then(Value::as_str).ok_or("the room has no key")?.to_string();
            let project = crate::project::find(model).ok_or("this model has no project")?;
            let mut labels = project.props.get("labels").and_then(Value::as_object).cloned().unwrap_or_default();
            match value.as_str().map(str::trim).filter(|t| !t.is_empty()) {
                Some(t) => labels.insert(key_room, json!(t)),
                None => labels.remove(&key_room),
            };
            edit.cmds.push(Command::SetProp { id: project.id, key: "labels".into(), value: Some(Value::Object(labels)) });
        }
        ("Dimension", "at") | ("Tag", "dx" | "dy") => {
            edit.cmds.push(Command::SetProp { id, key: key.into(), value: Some(json!(int(key)?)) });
        }
        ("Tag", "showName" | "showArea" | "showElevation") => {
            let v = value.as_bool().ok_or_else(|| format!("{key} is true or false"))?;
            edit.cmds.push(Command::SetProp { id, key: key.into(), value: Some(json!(v)) });
        }
        ("Door", "isEgress") => {
            let v = value.as_bool().ok_or("isEgress is true or false")?;
            edit.cmds.push(Command::SetProp { id, key: "isEgress".into(), value: Some(json!(v)) });
        }
        _ => return Err(format!("'{key}' of a {} is not edited here", e.kind.to_lowercase())),
    }
    Ok(edit)
}

// ------------------------------------------------------------------ starting values

/// The values most common among the level's walls, doors and windows: offered as the starting point of the drawing tools, never applied
/// silently. `None` where the level has none to learn from.
pub fn defaults(model: &Model, level: Option<Id>) -> Value {
    fn mode<T: Ord + Clone>(v: Vec<T>) -> Option<T> {
        let mut m: std::collections::BTreeMap<T, usize> = Default::default();
        for x in v {
            *m.entry(x).or_default() += 1;
        }
        m.into_iter().max_by_key(|(_, n)| *n).map(|(k, _)| k)
    }
    let walls = walls_on(model, level);
    let interior: Vec<&Wall> = walls.iter().map(|(_, w)| w).filter(|w| !w.is_exterior).collect();
    let pool: Vec<&Wall> = if interior.is_empty() { walls.iter().map(|(_, w)| w).collect() } else { interior };
    let wall = mode(pool.iter().map(|w| (w.thickness, w.height)).collect());
    let doors: Vec<(i64, i64)> = model.elements.values().filter(|e| e.kind == Door::KIND && on_level(e, level)).filter_map(|e| e.typed::<Door>().ok()).map(|d| (d.width, d.height)).collect();
    let windows: Vec<(i64, i64, i64)> = model.elements.values().filter(|e| e.kind == Window::KIND && on_level(e, level)).filter_map(|e| e.typed::<Window>().ok()).map(|w| (w.width, w.height, w.sill_height)).collect();
    json!({
        "wall": wall.map(|(t, h)| json!({ "thickness": t, "height": h })),
        "door": mode(doors).map(|(w, h)| json!({ "width": w, "height": h })),
        "window": mode(windows).map(|(w, h, s)| json!({ "width": w, "height": h, "sill": s })),
        "note": "the values most common on this level; change them as needed",
    })
}
