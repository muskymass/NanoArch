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
use crate::typed::{Column, Door, Space, Typed, Wall, Window, element_of};
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
    for e in model.elements.values().filter(|e| e.kind == Column::KIND && on_level(e, level) && !ctx.exclude.contains(&e.id)) {
        if let Ok(c) = e.typed::<Column>() {
            points.push([c.x as f64, c.y as f64]);
        }
    }
    let best = |cands: &[P]| -> Option<P> { cands.iter().filter_map(|p| on_axis(*p)).filter(|p| d(held, *p) <= radius).min_by(|a, b| d(held, *a).total_cmp(&d(held, *b))) };
    if let Some(p) = best(&points) {
        return round(p, SnapKind::Endpoint);
    }
    // Where two wall centre lines cross (a horizontal and a vertical one).
    let mut crossings = Vec::new();
    for (_, h) in walls.iter().filter(|(_, w)| w.horizontal()) {
        for (_, v) in walls.iter().filter(|(_, w)| !w.horizontal()) {
            let (hx0, hx1) = (h.x1.min(h.x2), h.x1.max(h.x2));
            let (vy0, vy1) = (v.y1.min(v.y2), v.y1.max(v.y2));
            if (hx0..=hx1).contains(&v.x1) && (vy0..=vy1).contains(&h.y1) {
                crossings.push([v.x1 as f64, h.y1 as f64]);
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
    let rects = room_rects(model, host.level_id);
    let (rlo, rhi) = rooms_beside(&rects, ax(&wall), wall.thickness);
    let mut el = match spec.kind {
        OpeningKind::Door => {
            let from = if wall.room_a.is_empty() { rlo.clone().or(rhi.clone()).unwrap_or_default() } else { wall.room_a.clone() };
            let to = wall.room_b.clone().unwrap_or_else(|| if wall.is_exterior { "out".into() } else { String::new() });
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
            let room = if wall.room_a.is_empty() { rlo.or(rhi).unwrap_or_default() } else { wall.room_a.clone() };
            element_of(&Window { width: spec.width, height: spec.height, sill_height: sill, offset_center: spec.offset, room, extra: Default::default() }, Source::User)
        }
    };
    el.level_id = host.level_id;
    el.host_id = Some(spec.wall);
    let id = el.id;
    // A position that would stand in a column is refused, not moved behind the user's back.
    let mut probe = crate::Document::new(model.clone());
    probe.commit("probe", crate::Origin::User, vec![Command::Create(el.clone())]).map_err(|e| e.to_string())?;
    let (fix, _) = building::clear_openings(&probe.model, &[host.level_id]);
    if fix.iter().any(|c| matches!(c, Command::SetProp { id: i, .. } | Command::Delete(i) if *i == id)) {
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
    for e in model.elements.values().filter(|e| on_level(e, level)) {
        let r: Option<[f64; 4]> = match e.kind.as_str() {
            "Wall" => e.typed::<Wall>().ok().map(|w| {
                let t = w.thickness as f64 / 2.0;
                [w.x1.min(w.x2) as f64 - if w.horizontal() { 0.0 } else { t }, w.y1.min(w.y2) as f64 - if w.horizontal() { t } else { 0.0 }, w.x1.max(w.x2) as f64 + if w.horizontal() { 0.0 } else { t }, w.y1.max(w.y2) as f64 + if w.horizontal() { t } else { 0.0 }]
            }),
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
            "Space" => return Err("rooms are not moved here: use the partition and room tools, which keep the plan tiled".into()),
            other => return Err(format!("{other} elements cannot be moved this way")),
        }
    }
    if walls.is_empty() && columns.is_empty() {
        return Err("select walls or columns first".into());
    }
    let level = walls.first().map(|w| w.2.level_id).or(columns.first().map(|c| c.2.level_id)).flatten();
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
            "Door" | "Window" => {}
            "Column" if e.provenance.source != Source::Generated => {}
            "Column" => return Err("this column belongs to the generated structure and would be put back: lock it, or edit the plan it comes from".into()),
            "Space" => return Err("rooms are deleted with the room tool (it hands the area to a neighbour)".into()),
            other => return Err(format!("{other} elements are not deleted this way")),
        }
    }
    // Openings first: a wall that hosts one cannot be deleted.
    let all: Vec<Id> = ids.iter().copied().chain(gone).collect();
    let (open, rest): (Vec<Id>, Vec<Id>) = all.into_iter().partition(|i| model.get(i).is_some_and(|e| e.host_id.is_some()));
    edit.cmds.extend(open.into_iter().chain(rest).map(Command::Delete));
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
            f.push(field("roomB", "Room (side 2)", "الغرفة (الجهة 2)", json!(w.room_b.unwrap_or_default()), "text", false));
            f.push(field("isExterior", "Exterior", "خارجي", json!(w.is_exterior), "bool", false));
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
            f.push(field("requirementKey", "Room", "الغرفة", json!(s.requirement_key), "text", false));
            f.push(field("area", "Area (m²)", "المساحة (م²)", json!(s.area), "text", false));
            f.push(field("w", "Width", "العرض", json!(s.w), "mm", false));
            f.push(field("h", "Length", "الطول", json!(s.h), "mm", false));
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
