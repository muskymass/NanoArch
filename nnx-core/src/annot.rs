//! Annotations and the empty-project start: structural grid lines, permanent dimensions, room tags, rooms found from enclosed walls, and
//! the drawing of all of them on the plan.
//!
//! - **Blank project**: `blank_project_commands` makes a project element and the level `LVL-1`; nothing else. Drafting then works with no
//!   generated layout.
//! - **Rooms from walls**: `rooms_sync` finds the regions the wall centre lines enclose. A region that is a rectangle becomes a room
//!   (`Source::Computed`); one that is not a rectangle is reported, not guessed. Rooms it made are removed again when their walls go.
//!   Rooms from a layout, or ones the user made, are never touched.
//! - **Grid lines** have unique names (letters for vertical lines, numbers for horizontal ones), belong to the project and are drawn on every
//!   plan, elevation and section.
//! - **Permanent dimensions** measure along x or y between two positions. Taken from walls, grid lines, openings or columns, they keep
//!   a reference to them and follow them when they move.
//! - **Room tags** show a room's name, area and the level's elevation, computed when drawn.

use crate::command::Command;
use crate::model::{Element, Id, Model, Source};
use crate::typed::{Column, Dimension, Door, Footing, Grid, Level, RoomSeparator, Space, Tag, Typed, Wall, Window, element_of};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

/// Width in pixels of the invisible line laid over each thin plan element (wall, grid line, dimension line, separator) so a click on it hits
/// the first time. Eight is the least that selects reliably; a grid bubble is a circle of radius 13 and needs no widening.
pub const HIT_PX: u32 = 12;

/// The area of an empty plan that is shown to draw on `[x0, y0, x1, y1]` in millimetres: a view window, not a design value. Anything
/// drawn beyond it enlarges the plan.
pub const BLANK_CANVAS_MM: [i64; 4] = [0, 0, 20_000, 12_000];

fn on_level(e: &Element, level: Option<Id>) -> bool {
    level.is_none() || e.level_id == level
}

// ------------------------------------------------------------------ the empty project

/// The commands of a new empty project: its project element and the level `LVL-1`.
pub fn blank_project_commands(title: &str) -> Result<Vec<Command>, String> {
    let p = crate::project::NanoArchProject { meta: crate::project::ProjectMeta { title: title.trim().to_string(), ..Default::default() }, ..Default::default() };
    let level = element_of(&Level { name: "LVL-1".into(), index: 0, extra: Default::default() }, Source::User);
    Ok(vec![Command::Create(level), crate::project::create_command(&p)?])
}

// ------------------------------------------------------------------ grid lines

fn grid_names(model: &Model) -> BTreeSet<String> {
    model.elements.values().filter(|e| e.kind == Grid::KIND).filter_map(|e| e.props.get("name").and_then(|v| v.as_str()).map(|n| n.trim().to_uppercase())).collect()
}

fn letters(mut n: usize) -> String {
    let mut s = String::new();
    loop {
        s.insert(0, (b'A' + (n % 26) as u8) as char);
        if n < 26 {
            return s;
        }
        n = n / 26 - 1;
    }
}

/// The next unused name: `A`, `B` ... for a vertical line, `1`, `2` ... for a horizontal one.
pub fn next_grid_name(model: &Model, horizontal: bool) -> String {
    next_grid_name_excluding(model, horizontal, &BTreeSet::new())
}

/// Like `next_grid_name`, also avoiding the (upper-case) names in `taken`: for several lines made in one edit.
pub fn next_grid_name_excluding(model: &Model, horizontal: bool, taken: &BTreeSet<String>) -> String {
    let used: BTreeSet<String> = grid_names(model).union(taken).cloned().collect();
    if horizontal { (1..).map(|n: usize| n.to_string()).find(|n| !used.contains(n)).unwrap_or_default() } else { (0..).map(letters).find(|n| !used.contains(n)).unwrap_or_default() }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct GridSpec {
    pub a: [i64; 2],
    pub b: [i64; 2],
    pub name: Option<String>,
}

/// A new grid line between two points.
pub fn grid_command(model: &Model, spec: &GridSpec) -> Result<(Command, Id), String> {
    let (a, b) = (spec.a, spec.b);
    if (a[0] == b[0]) == (a[1] == b[1]) {
        return Err(if a == b { "the two points are the same".into() } else { "a grid line is horizontal or vertical: the points must share an x or a y".into() });
    }
    let horizontal = a[1] == b[1];
    let (lo, hi) = if horizontal { (a[0].min(b[0]), a[0].max(b[0])) } else { (a[1].min(b[1]), a[1].max(b[1])) };
    if hi - lo < crate::draft::MIN_WALL_MM {
        return Err(format!("a grid line must be at least {} mm long", crate::draft::MIN_WALL_MM));
    }
    let line = if horizontal { a[1] } else { a[0] };
    for e in model.elements.values().filter(|e| e.kind == Grid::KIND) {
        let Ok(g) = e.typed::<Grid>() else { continue };
        let (glo, ghi) = if g.horizontal() { (g.x1.min(g.x2), g.x1.max(g.x2)) } else { (g.y1.min(g.y2), g.y1.max(g.y2)) };
        if g.horizontal() == horizontal && g.coord() == line && glo.max(lo) < ghi.min(hi) {
            return Err(format!("grid line {} already runs along that line there", g.name));
        }
    }
    let name = spec.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| next_grid_name(model, horizontal));
    let (p, q) = if horizontal { ([lo, line], [hi, line]) } else { ([line, lo], [line, hi]) };
    let g = Grid { x1: p[0], y1: p[1], x2: q[0], y2: q[1], name, extra: Default::default() };
    let el = element_of(&g, Source::User);
    let id = el.id;
    Ok((Command::Create(el), id))
}

// ------------------------------------------------------------------ dimensions

/// The coordinate of an element along `axis` (`'x'` or `'y'`) that a dimension can be taken from: the line of a perpendicular wall or
/// grid, the centre of a door, window or column. `None` where it has no single position along that axis.
pub fn feature_coord(model: &Model, id: Id, axis: char) -> Option<i64> {
    let e = model.get(&id)?;
    match e.kind.as_str() {
        "Wall" => {
            let w = e.typed::<Wall>().ok()?;
            (w.horizontal() == (axis == 'y')).then_some(if axis == 'x' { w.x1 } else { w.y1 })
        }
        "Grid" => {
            let g = e.typed::<Grid>().ok()?;
            (g.horizontal() == (axis == 'y')).then_some(g.coord())
        }
        "Column" => {
            let c = e.typed::<Column>().ok()?;
            Some(if axis == 'x' { c.x } else { c.y })
        }
        "Door" | "Window" => {
            let host = model.get(&e.host_id?)?.typed::<Wall>().ok()?;
            let off = e.props.get("offsetCenter")?.as_i64()?;
            let start = if host.horizontal() { host.x1.min(host.x2) } else { host.y1.min(host.y2) };
            Some(if host.horizontal() == (axis == 'x') { start + off } else if axis == 'x' { host.x1 } else { host.y1 })
        }
        _ => None,
    }
}

/// The two measured positions of a dimension: those of the elements it was taken from where they still exist, else the stored ones.
pub fn dim_positions(model: &Model, d: &Dimension) -> (i64, i64) {
    let axis = d.axis.chars().next().unwrap_or('x');
    let follow = |r: &Option<String>, stored: i64| r.as_ref().and_then(|s| s.parse::<Id>().ok()).and_then(|id| feature_coord(model, id, axis)).unwrap_or(stored);
    (follow(&d.ref_a, d.a), follow(&d.ref_b, d.b))
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct DimSpec {
    pub a: [i64; 2],
    pub b: [i64; 2],
    /// A point on the line the dimension is drawn on.
    pub at: [i64; 2],
    pub ref_a: Option<Id>,
    pub ref_b: Option<Id>,
}

/// A permanent dimension. It measures along x when the two points are further apart in x, else along y. A reference is kept only when
/// the element's position along that axis is within 250 mm of the point (the point is then moved onto it).
pub fn dimension_command(model: &Model, level: Option<Id>, spec: &DimSpec) -> Result<(Command, Id, Vec<String>), String> {
    let (dx, dy) = ((spec.b[0] - spec.a[0]).abs(), (spec.b[1] - spec.a[1]).abs());
    let axis = if dx >= dy { 'x' } else { 'y' };
    let ix = usize::from(axis == 'y');
    let (mut a, mut b) = (spec.a[ix], spec.b[ix]);
    let mut notes = Vec::new();
    let mut take = |r: Option<Id>, v: &mut i64, which: &str| -> Option<String> {
        let id = r?;
        match feature_coord(model, id, axis) {
            Some(c) if (c - *v).abs() <= 250 => {
                *v = c;
                Some(id.to_string())
            }
            _ => {
                notes.push(format!("the {which} point is not attached to an element: it has no single position along {axis}"));
                None
            }
        }
    };
    let ref_a = take(spec.ref_a, &mut a, "first");
    let ref_b = take(spec.ref_b, &mut b, "second");
    if a == b {
        return Err(format!("the two points are at the same position along {axis}"));
    }
    let d = Dimension { axis: axis.to_string(), a, b, oa: spec.a[1 - ix], ob: spec.b[1 - ix], at: spec.at[1 - ix], ref_a, ref_b, extra: Default::default() };
    let mut el = element_of(&d, Source::User);
    el.level_id = level;
    let id = el.id;
    Ok((Command::Create(el), id, notes))
}

// ------------------------------------------------------------------ tags

// ------------------------------------------------------------------ room separators

/// The shortest separator that can be drawn: a geometric floor, not a code value.
pub const MIN_SEPARATOR_MM: i64 = 100;

/// A room separator from `a` to `b` on `level`. It must be horizontal or vertical and cannot lie on top of another separator.
pub fn separator_command(model: &Model, level: Option<Id>, a: (i64, i64), b: (i64, i64)) -> Result<(Command, Id), String> {
    if a.0 != b.0 && a.1 != b.1 {
        return Err("a room separator must be horizontal or vertical".into());
    }
    if (a.0 - b.0).abs().max((a.1 - b.1).abs()) < MIN_SEPARATOR_MM {
        return Err(format!("a room separator is at least {MIN_SEPARATOR_MM} mm long"));
    }
    let (p, q) = if a.0 < b.0 || a.1 < b.1 { (a, b) } else { (b, a) };
    let dup = model.elements.values().filter(|e| e.kind == RoomSeparator::KIND && e.level_id == level).filter_map(|e| e.typed::<RoomSeparator>().ok()).any(|s| (s.x1, s.y1, s.x2, s.y2) == (p.0, p.1, q.0, q.1));
    if dup {
        return Err("a room separator is already drawn there".into());
    }
    let mut el = element_of(&RoomSeparator { x1: p.0, y1: p.1, x2: q.0, y2: q.1, extra: Default::default() }, Source::User);
    el.level_id = level;
    let id = el.id;
    Ok((Command::Create(el), id))
}

/// The point a tag is anchored at (before its own offset): a room's centre, a column's or footing's centre, an opening's centre in its wall.
pub fn host_centre(model: &Model, host: &Element) -> Option<(i64, i64)> {
    match host.kind.as_str() {
        "Space" => host.typed::<Space>().ok().map(|s| (s.x + s.w / 2, s.y + s.h / 2)),
        "Column" => host.typed::<Column>().ok().map(|c| (c.x, c.y)),
        "Footing" => host.typed::<Footing>().ok().map(|f| (f.x, f.y)),
        "Door" | "Window" => {
            let off = host.props.get("offsetCenter").and_then(|v| v.as_i64())?;
            let w = host.host_id.and_then(|h| model.get(&h))?.typed::<Wall>().ok()?;
            Some(if w.horizontal() { (w.x1.min(w.x2) + off, w.y1) } else { (w.x1, w.y1.min(w.y2) + off) })
        }
        _ => None,
    }
}

/// The rooms of the level that have no tag yet.
pub fn untagged_rooms(model: &Model, level: Option<Id>) -> Vec<Id> {
    let tagged: BTreeSet<Id> = model.elements.values().filter(|e| e.kind == Tag::KIND).filter_map(|e| e.host_id).collect();
    model.elements.values().filter(|e| e.kind == Space::KIND && on_level(e, level) && !tagged.contains(&e.id)).map(|e| e.id).collect()
}

/// Tags for the given elements (rooms, doors, windows, columns or footings): every field shown, at the element's centre. An element that
/// has one already is skipped.
pub fn tag_commands(model: &Model, hosts: &[Id]) -> Result<(Vec<Command>, Vec<Id>), String> {
    let tagged: BTreeSet<Id> = model.elements.values().filter(|e| e.kind == Tag::KIND).filter_map(|e| e.host_id).collect();
    let (mut cmds, mut ids) = (Vec::new(), Vec::new());
    for r in hosts {
        let host = model.get(r).ok_or("an element no longer exists")?;
        if ![Space::KIND, Door::KIND, Window::KIND, Column::KIND, Footing::KIND].contains(&host.kind.as_str()) {
            return Err("only rooms, doors, windows, columns and footings can be tagged".into());
        }
        if tagged.contains(r) {
            continue;
        }
        let mut el = element_of(&Tag { dx: 0, dy: 0, show_name: true, show_area: true, show_elevation: true, extra: Default::default() }, Source::User);
        el.level_id = host.level_id.or_else(|| host.host_id.and_then(|h| model.get(&h)).and_then(|w| w.level_id));
        el.host_id = Some(*r);
        ids.push(el.id);
        cmds.push(Command::Create(el));
    }
    Ok((cmds, ids))
}

/// The lines a tag shows. A room: its display name, its area in m2 and the elevation of its level. A door, window, column or footing:
/// its kind (and the grid reference of a column), its size in mm and the elevation of its level.
pub fn tag_lines(model: &Model, host: &Element, tag: &Tag, labels: &BTreeMap<String, String>) -> Vec<String> {
    let mut v = Vec::new();
    let level = host.level_id.or_else(|| host.host_id.and_then(|h| model.get(&h)).and_then(|w| w.level_id));
    let (name, size): (String, String) = match host.kind.as_str() {
        "Space" => {
            let Ok(sp) = host.typed::<Space>() else { return Vec::new() };
            (labels.get(&sp.requirement_key).or_else(|| labels.get(&sp.func)).cloned().unwrap_or_else(|| sp.requirement_key.clone()), format!("{:.2} m²", sp.net_area.unwrap_or(sp.area)))
        }
        "Door" => match host.typed::<Door>() {
            Ok(d) => ("Door".into(), format!("{} × {}", d.width, d.height)),
            Err(_) => return Vec::new(),
        },
        "Window" => match host.typed::<Window>() {
            Ok(w) => ("Window".into(), format!("{} × {}", w.width, w.height)),
            Err(_) => return Vec::new(),
        },
        "Column" => match host.typed::<Column>() {
            Ok(c) => (if c.grid_ref.is_empty() { "Column".into() } else { format!("Column {}", c.grid_ref) }, format!("{} × {}", c.b, c.h)),
            Err(_) => return Vec::new(),
        },
        "Footing" => match host.typed::<Footing>() {
            Ok(f) => ("Footing".into(), format!("{} × {}", f.length, f.width)),
            Err(_) => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    if tag.show_name {
        v.push(name);
    }
    if tag.show_area {
        v.push(size);
    }
    if tag.show_elevation {
        let z = crate::model3d::level_elevation_m(model, level);
        v.push(if z.abs() < 0.005 { "±0.00 m".to_string() } else { format!("{z:+.2} m") });
    }
    v
}

// ------------------------------------------------------------------ rooms from enclosed walls

#[derive(Debug, Clone, Default)]
pub struct RoomSync {
    pub cmds: Vec<Command>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// Enclosed regions that are not rectangles: they get no room.
    pub not_rect: usize,
    /// Regions that were not made rooms because a room of a layout already covers part of them.
    pub overlapping: Vec<String>,
}

/// The rectangles (x, y, w, h) the wall centre lines of the level enclose, and how many enclosed regions are not rectangles.
pub fn enclosed_rects(model: &Model, level: Option<Id>) -> (Vec<[i64; 4]>, usize) {
    // Wall centre lines and room separators bound a region alike: (x1, y1, x2, y2).
    let mut lines: Vec<(i64, i64, i64, i64)> = model.elements.values().filter(|e| e.kind == Wall::KIND && on_level(e, level)).filter_map(|e| e.typed::<Wall>().ok()).map(|w| (w.x1, w.y1, w.x2, w.y2)).collect();
    if lines.is_empty() {
        return (Vec::new(), 0);
    }
    lines.extend(model.elements.values().filter(|e| e.kind == RoomSeparator::KIND && on_level(e, level)).filter_map(|e| e.typed::<RoomSeparator>().ok()).map(|s| (s.x1, s.y1, s.x2, s.y2)));
    let mut xs: Vec<i64> = lines.iter().flat_map(|w| [w.0, w.2]).collect();
    let mut ys: Vec<i64> = lines.iter().flat_map(|w| [w.1, w.3]).collect();
    xs.sort_unstable();
    xs.dedup();
    ys.sort_unstable();
    ys.dedup();
    let (cols, rows) = (xs.len() + 1, ys.len() + 1);
    // Vertical walls as (x, y0, y1) and horizontal ones as (y, x0, x1).
    let vert: Vec<(i64, i64, i64)> = lines.iter().filter(|w| w.0 == w.2).map(|w| (w.0, w.1.min(w.3), w.1.max(w.3))).collect();
    let horiz: Vec<(i64, i64, i64)> = lines.iter().filter(|w| w.1 == w.3).map(|w| (w.1, w.0.min(w.2), w.0.max(w.2))).collect();
    // Moving from cell (c, r) to (c + 1, r) crosses the vertical line xs[c] along the row r; to (c, r + 1) the horizontal line ys[r] along column c.
    let blocked_x = |c: usize, r: usize| r > 0 && r < rows - 1 && vert.iter().any(|(x, a, b)| *x == xs[c] && *a <= ys[r - 1] && *b >= ys[r]);
    let blocked_y = |c: usize, r: usize| c > 0 && c < cols - 1 && horiz.iter().any(|(y, a, b)| *y == ys[r] && *a <= xs[c - 1] && *b >= xs[c]);
    let mut comp = vec![usize::MAX; cols * rows];
    let mut n = 0;
    for start in 0..cols * rows {
        if comp[start] != usize::MAX {
            continue;
        }
        let mut stack = vec![start];
        comp[start] = n;
        while let Some(i) = stack.pop() {
            let (c, r) = (i % cols, i / cols);
            let mut go = |j: usize, blocked: bool| {
                if !blocked && comp[j] == usize::MAX {
                    comp[j] = n;
                    stack.push(j);
                }
            };
            if c + 1 < cols {
                go(i + 1, blocked_x(c, r));
            }
            if c > 0 {
                go(i - 1, blocked_x(c - 1, r));
            }
            if r + 1 < rows {
                go(i + cols, blocked_y(c, r));
            }
            if r > 0 {
                go(i - cols, blocked_y(c, r - 1));
            }
        }
        n += 1;
    }
    let outside = comp[0];
    let (mut rects, mut not_rect) = (Vec::new(), 0);
    for k in (0..n).filter(|k| *k != outside) {
        let cells: Vec<usize> = (0..cols * rows).filter(|i| comp[*i] == k).collect();
        let (cmin, cmax) = (cells.iter().map(|i| i % cols).min().unwrap_or(0), cells.iter().map(|i| i % cols).max().unwrap_or(0));
        let (rmin, rmax) = (cells.iter().map(|i| i / cols).min().unwrap_or(0), cells.iter().map(|i| i / cols).max().unwrap_or(0));
        let area: i128 = cells.iter().map(|i| i128::from(xs[i % cols] - xs[i % cols - 1]) * i128::from(ys[i / cols] - ys[i / cols - 1])).sum();
        let (x0, x1, y0, y1) = (xs[cmin - 1], xs[cmax], ys[rmin - 1], ys[rmax]);
        if area == i128::from(x1 - x0) * i128::from(y1 - y0) { rects.push([x0, y0, x1 - x0, y1 - y0]) } else { not_rect += 1 }
    }
    (rects, not_rect)
}

/// What to add and remove so the level's computed rooms match the regions its walls enclose.
pub fn rooms_sync(model: &Model, level: Option<Id>) -> RoomSync {
    let (rects, not_rect) = enclosed_rects(model, level);
    let mut out = RoomSync { not_rect, ..Default::default() };
    let existing: Vec<(Id, Source, String, [i64; 4])> = model.elements.values().filter(|e| e.kind == Space::KIND && on_level(e, level)).filter_map(|e| Some((e.id, e.provenance.source, e.typed::<Space>().ok()?.requirement_key, {
        let s = e.typed::<Space>().ok()?;
        [s.x, s.y, s.w, s.h]
    }))).collect();
    // Computed rooms whose region is gone go, with their tags (a room that hosts a tag cannot be deleted before it).
    for (id, src, key, r) in &existing {
        if *src == Source::Computed && !rects.contains(r) {
            out.cmds.extend(model.dependents(id).into_iter().map(Command::Delete));
            out.cmds.push(Command::Delete(*id));
            out.removed.push(key.clone());
        }
    }
    let mut keys: BTreeSet<String> = model.elements.values().filter(|e| e.kind == Space::KIND).filter_map(|e| e.props.get("requirementKey").and_then(|v| v.as_str()).map(str::to_string)).collect();
    let overlaps = |a: &[i64; 4], b: &[i64; 4]| a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3];
    for r in &rects {
        if existing.iter().any(|(_, _, _, x)| x == r) {
            continue;
        }
        if let Some((_, _, key, _)) = existing.iter().find(|(_, src, _, x)| *src != Source::Computed && overlaps(x, r)) {
            out.overlapping.push(key.clone());
            continue;
        }
        let key = (1..).map(|i| format!("R{i}")).find(|k| !keys.contains(k)).unwrap_or_default();
        keys.insert(key.clone());
        let area = ((r[2] as f64 * r[3] as f64) / 1e6 * 100.0).round() / 100.0;
        let sp = Space { func: "room".into(), requirement_key: key.clone(), area, target_area: area, x: r[0], y: r[1], w: r[2], h: r[3], boundary: vec![[r[0], r[1]], [r[0] + r[2], r[1]], [r[0] + r[2], r[1] + r[3]], [r[0], r[1] + r[3]]], net_area: None, glazing_ratio: None, extra: Default::default() };
        let mut el = element_of(&sp, Source::Computed);
        el.level_id = level;
        out.cmds.push(Command::Create(el));
        out.added.push(key);
    }
    out
}

// ------------------------------------------------------------------ the plan

/// The mapping from millimetres to the plan's pixels.
#[derive(Debug, Clone, Copy)]
pub struct Map {
    pub minx: f64,
    pub maxy: f64,
    pub s: f64,
    pub m: f64,
}

impl Map {
    pub fn tx(&self, x: f64) -> f64 {
        self.m + (x - self.minx) * self.s
    }
    pub fn ty(&self, y: f64) -> f64 {
        self.m + (self.maxy - y) * self.s
    }
}

/// Reports every position the annotations occupy, so the plan is big enough to show them.
pub fn grow_extents(model: &Model, level: Option<Id>, grow: &mut dyn FnMut(i64, i64)) {
    for e in model.elements.values() {
        match e.kind.as_str() {
            "Grid" => {
                if let Ok(g) = e.typed::<Grid>() {
                    // The bubbles sit just beyond the ends of the line.
                    let r = 700;
                    let (dx, dy) = if g.horizontal() { (r, 0) } else { (0, r) };
                    for (x, y) in [(g.x1.min(g.x2), g.y1.min(g.y2)), (g.x1.max(g.x2), g.y1.max(g.y2))] {
                        grow(x, y);
                    }
                    grow(g.x1.min(g.x2) - dx, g.y1.min(g.y2) - dy);
                    grow(g.x1.max(g.x2) + dx, g.y1.max(g.y2) + dy);
                }
            }
            "Dimension" if on_level(e, level) => {
                if let Ok(d) = e.typed::<Dimension>() {
                    let (a, b) = dim_positions(model, &d);
                    if d.axis == "x" {
                        for (x, y) in [(a, d.oa), (b, d.ob), (a, d.at), (b, d.at)] {
                            grow(x, y);
                        }
                    } else {
                        for (y, x) in [(a, d.oa), (b, d.ob), (a, d.at), (b, d.at)] {
                            grow(x, y);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The rooms that have a tag: the plan leaves their default label out.
pub fn tagged_rooms(model: &Model, level: Option<Id>) -> BTreeSet<Id> {
    model.elements.values().filter(|e| e.kind == Tag::KIND && on_level(e, level)).filter_map(|e| e.host_id).collect()
}

/// The grid lines, permanent dimensions and room tags of the level as SVG, appended to `o`.
pub fn plan_svg(model: &Model, level: Option<Id>, m: &Map, labels: &BTreeMap<String, String>, o: &mut String) {
    for e in model.elements.values().filter(|e| e.kind == Grid::KIND) {
        let Ok(g) = e.typed::<Grid>() else { continue };
        let (p, q) = ((m.tx(g.x1 as f64), m.ty(g.y1 as f64)), (m.tx(g.x2 as f64), m.ty(g.y2 as f64)));
        let len = ((q.0 - p.0).powi(2) + (q.1 - p.1).powi(2)).sqrt().max(1e-9);
        let u = ((q.0 - p.0) / len, (q.1 - p.1) / len);
        let _ = writeln!(o, r#"<g class="gridline" data-id="{}"><line class="grid" x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}"/><line class="gridhit" x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="transparent" stroke-width="{HIT_PX}"/>"#, e.id, p.0, p.1, q.0, q.1, p.0, p.1, q.0, q.1);
        let r = 13.0;
        for (end, out) in [(p, (-u.0, -u.1)), (q, u)] {
            let c = (end.0 + out.0 * r, end.1 + out.1 * r);
            let _ = writeln!(o, r#"<circle class="gbub" cx="{:.1}" cy="{:.1}" r="{r}"/><text class="gtxt" x="{:.1}" y="{:.1}">{}</text>"#, c.0, c.1, c.0, c.1 + 4.0, esc(&g.name));
        }
        let _ = writeln!(o, "</g>");
    }
    for e in model.elements.values().filter(|e| e.kind == RoomSeparator::KIND && on_level(e, level)) {
        let Ok(s) = e.typed::<RoomSeparator>() else { continue };
        let (p, q) = ((m.tx(s.x1 as f64), m.ty(s.y1 as f64)), (m.tx(s.x2 as f64), m.ty(s.y2 as f64)));
        let _ = writeln!(o, r#"<g class="separator" data-id="{}"><line class="sepline" x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}"/><line class="sephit" x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="transparent" stroke-width="{HIT_PX}"/></g>"#, e.id, p.0, p.1, q.0, q.1, p.0, p.1, q.0, q.1);
    }
    for e in model.elements.values().filter(|e| e.kind == Dimension::KIND && on_level(e, level)) {
        let Ok(d) = e.typed::<Dimension>() else { continue };
        let (a, b) = dim_positions(model, &d);
        let x_axis = d.axis == "x";
        // (position along the axis, position across it) to pixels.
        let px = |along: i64, across: i64| if x_axis { (m.tx(along as f64), m.ty(across as f64)) } else { (m.tx(across as f64), m.ty(along as f64)) };
        let _ = writeln!(o, r#"<g class="dimension" data-id="{}">"#, e.id);
        for (along, from) in [(a, d.oa), (b, d.ob)] {
            let dir = if d.at >= from { 1 } else { -1 };
            let (s0, s1) = (px(along, from + dir * 50), px(along, d.at + dir * 150));
            let _ = writeln!(o, r#"<line class="dimext" x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}"/>"#, s0.0, s0.1, s1.0, s1.1);
            let t = px(along, d.at);
            let _ = writeln!(o, r#"<line class="dimtick" x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}"/>"#, t.0 - 4.0, t.1 + 4.0, t.0 + 4.0, t.1 - 4.0);
        }
        let (l0, l1) = (px(a, d.at), px(b, d.at));
        let _ = writeln!(o, r#"<line class="dimline" x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}"/><line class="dimhit" x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="transparent" stroke-width="{HIT_PX}"/>"#, l0.0, l0.1, l1.0, l1.1, l0.0, l0.1, l1.0, l1.1);
        let mid = ((l0.0 + l1.0) / 2.0, (l0.1 + l1.1) / 2.0);
        let text = format!("{}", (b - a).abs());
        if x_axis {
            let _ = writeln!(o, r#"<text class="dimtxt2" x="{:.1}" y="{:.1}">{text}</text>"#, mid.0, mid.1 - 4.0);
        } else {
            let _ = writeln!(o, r#"<text class="dimtxt2" x="{:.1}" y="{:.1}" transform="rotate(-90 {:.1} {:.1})">{text}</text>"#, mid.0 - 4.0, mid.1, mid.0 - 4.0, mid.1);
        }
        let _ = writeln!(o, "</g>");
    }
    for e in model.elements.values().filter(|e| e.kind == Tag::KIND && on_level(e, level)) {
        let (Ok(t), Some(host)) = (e.typed::<Tag>(), e.host_id.and_then(|h| model.get(&h))) else { continue };
        let Some(centre) = host_centre(model, host) else { continue };
        let lines = tag_lines(model, host, &t, labels);
        if lines.is_empty() {
            continue;
        }
        let (cx, cy) = (m.tx((centre.0 + t.dx) as f64), m.ty((centre.1 + t.dy) as f64));
        let w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(1) as f64 * 6.4 + 12.0;
        let h = lines.len() as f64 * 13.0 + 6.0;
        let _ = writeln!(o, r#"<g class="tag" data-id="{}" data-for="{}"><rect class="tagbox" x="{:.1}" y="{:.1}" width="{w:.1}" height="{h:.1}" rx="3"/>"#, e.id, host.id, cx - w / 2.0, cy - h / 2.0);
        for (i, l) in lines.iter().enumerate() {
            let _ = writeln!(o, r#"<text class="{}" x="{:.1}" y="{:.1}">{}</text>"#, if i == 0 && t.show_name { "tagname" } else { "tagtxt" }, cx, cy - h / 2.0 + 13.0 * (i as f64 + 1.0) - 1.0, esc(l));
        }
        let _ = writeln!(o, "</g>");
    }
}
