//! Plan import: coloured raster plans, one per level, into a project with its levels, walls, doors, windows and rooms.
//!
//! This is the whole path the program takes from pictures to a model, so that nothing about it lives outside the program:
//!
//! 1. **Axes.** The axis bubbles of each drawing are found (`planaxes`) and laid over the axes the owner typed from the drawing's
//!    dimension chain. That gives the scale of each drawing and where it stands, in the same coordinates for every drawing, whatever
//!    each one's crop. No label is read; the program says how many bubbles fixed each side and asks for confirmation when only two did.
//! 2. **Walls and openings** (`rasterwalls`) in the colours the owner named as walls.
//! 3. **The model**: a project, a level per drawing, and every wall and opening drawn with the same commands as the drafting tools, so
//!    undo, the room finder, the checks and the exports see them as drafted by hand. Rooms are found from the walls, one step per wall.
//!
//! Nothing is invented. The heights, the wall thickness and the opening sizes are the owner's and are required: a missing one is an error
//! that names it. A gap the drawing does not show as a door or a window is not built, and is counted with the reason. No structure is
//! generated here: the model carries what the drawings carry.

use crate::annot;
use crate::building;
use crate::draft::{self, OpeningKind as BuildKind, OpeningSpec, WallSpec};
use crate::planaxes::{self, Axis, SheetFit};
use crate::project::{self, NanoArchProject, ProjectMeta};
use crate::rasterwalls::{self, OpeningKind, WallParams};
use crate::typed::{Level, Typed, Wall, element_of};
use crate::Model;
use crate::{Command, Document, Id, Origin, Source};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What the owner gives for the whole project. Every field is required: there is no default for a height or a thickness.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Spec {
    pub title: String,
    /// Floor to floor, mm: the wall height of every level.
    pub storey_mm: Option<i64>,
    pub wall_thickness_mm: Option<i64>,
    pub door_height_mm: Option<i64>,
    pub window_height_mm: Option<i64>,
    /// Sill height of the windows, mm.
    pub window_sill_mm: Option<i64>,
    /// The axes along the top of the sheets, left to right, each with its distance from the first, mm.
    pub x_axes: Vec<Axis>,
    /// The axes down the side of the sheets, top to bottom, each with its distance from the first, mm.
    pub y_axes: Vec<Axis>,
    /// Bubble radius range in pixels. When absent it is derived from the image width and said in the notes.
    pub bubble_radius_px: Option<[u32; 2]>,
    /// The column's two sides, mm, short then long. Optional: with it a column is placed where two axes cross on a wall; without it
    /// no column is placed. The drawings carry no column size, so there is no default.
    pub column_mm: Option<[i64; 2]>,
    /// The door widths the owner builds with, mm. Optional: with it a door read within two pixels of one of them takes that width;
    /// without it every door keeps the width read from the picture. There is no default list.
    pub door_widths_mm: Option<Vec<i64>>,
    /// The same for windows.
    pub window_widths_mm: Option<Vec<i64>>,
}

/// The width of the owner's list that a width read from a picture stands for: the nearest one within `reach` mm, or none. `reach` is
/// two pixels of the drawing (one at each end of the opening): what the picture cannot tell apart.
pub fn standard_width(read: i64, list: &[i64], reach: i64) -> Option<i64> {
    list.iter().copied().filter(|w| (w - read).abs() <= reach).min_by_key(|w| ((w - read).abs(), *w))
}

pub struct Drawing {
    /// The drawing's own name, for the report.
    pub name: String,
    /// The level it becomes. Levels are made in the order the drawings come.
    pub level: String,
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
    /// The colours this drawing's walls are in (the owner's choice among `rasterwalls::wall_colour_candidates`).
    pub wall_colours: Vec<[u8; 3]>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LevelReport {
    pub level: String,
    pub drawing: String,
    pub mm_per_px: f64,
    /// Where this drawing's pixels stand in the model: `x_mm = origin_x_mm + mm_per_px * px_x`, `y_mm = origin_y_mm - mm_per_px * px_y`.
    /// With it any position read off the picture can be found in the model, and the other way round.
    pub origin_x_mm: f64,
    pub origin_y_mm: f64,
    pub bubbles_found: usize,
    pub column_bubbles_fitted: usize,
    pub row_bubbles_fitted: usize,
    pub walls_read: usize,
    pub walls_built: usize,
    pub walls_refused: BTreeMap<String, usize>,
    pub openings_read: BTreeMap<String, usize>,
    pub doors_built: usize,
    pub windows_built: usize,
    /// Passages, each made a room separator between the two walls beside it.
    pub passages_built: usize,
    /// Empty gaps where nothing was built (see the reader's rule).
    pub gaps_left_open: usize,
    /// Those gaps, each where it stands in the model and in the picture, so that the owner can close the ones that are room boundaries.
    pub open_gaps: Vec<OpenGap>,
    /// Gaps that were not built, by reason.
    pub openings_not_built: BTreeMap<String, usize>,
    /// Columns placed where two of the drawings' axes cross on a wall of this level.
    pub columns_built: usize,
    /// Axis crossings of this level with no wall there, by the names of the two axes: no column was placed on them.
    pub crossings_without_wall: Vec<String>,
    /// Rooms, a room made of several rectangles counted once.
    pub rooms: usize,
    /// Rooms that are not rectangles (made of several parts).
    pub composite_rooms: usize,
    pub notes: Vec<String>,
}

/// An empty gap the import left open.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OpenGap {
    /// Its number on the level, from 1.
    pub n: usize,
    /// Its two ends in the model, mm.
    pub a: [i64; 2],
    pub b: [i64; 2],
    pub width_mm: i64,
    /// `passage` (bare) or `unclear` (something is drawn across it).
    pub kind: &'static str,
    /// Its two ends in the picture: x1, y1, x2, y2 (pixels).
    pub px: [f64; 4],
    /// The row of equal gaps it stands in (from 1), when it is one of several side by side: see `rows_of_equal_gaps`.
    pub row: Option<usize>,
}

/// A row of equal gaps: at least this many, each spanning the same stretch (within the tolerance) and no farther from the next than
/// the pitch. What a stair's treads leave when they are drawn in the wall colour. These only decide how the report lists the gaps:
/// nothing is built or left out because of them.
pub const GAP_ROW_MIN: usize = 4;
pub const GAP_ROW_SPAN_TOL_MM: i64 = 100;
pub const GAP_ROW_PITCH_MAX_MM: i64 = 1000;

/// Marks the gaps that stand in a row of equal gaps with the row's number, and returns how many rows there are.
pub fn rows_of_equal_gaps(gaps: &mut [OpenGap]) -> usize {
    // (horizontal, where it starts along its line, where it ends, its line's position, index)
    let mut keyed: Vec<(bool, i64, i64, i64, usize)> = gaps.iter().enumerate().map(|(i, g)| if g.a[1] == g.b[1] { (true, g.a[0].min(g.b[0]), g.a[0].max(g.b[0]), g.a[1], i) } else { (false, g.a[1].min(g.b[1]), g.a[1].max(g.b[1]), g.a[0], i) }).collect();
    keyed.sort_by_key(|k| (k.0, k.3, k.4));
    let mut rows = 0;
    let mut used = vec![false; gaps.len()];
    for s in 0..keyed.len() {
        if used[keyed[s].4] {
            continue;
        }
        // The gaps after this one, in order across the row, that span the same stretch, each within the pitch of the one before.
        let mut run = vec![s];
        for t in s + 1..keyed.len() {
            let (first, last, k) = (keyed[s], keyed[run[run.len() - 1]], keyed[t]);
            if k.0 != first.0 || k.3 - last.3 > GAP_ROW_PITCH_MAX_MM {
                break;
            }
            if !used[k.4] && (k.1 - first.1).abs() <= GAP_ROW_SPAN_TOL_MM && (k.2 - first.2).abs() <= GAP_ROW_SPAN_TOL_MM {
                run.push(t);
            }
        }
        if run.len() >= GAP_ROW_MIN {
            rows += 1;
            for t in run {
                used[keyed[t].4] = true;
                gaps[keyed[t].4].row = Some(rows);
            }
        }
    }
    rows
}

/// How far (mm) an end of a gap may stand from the wall end, or from the face of the crossing wall, that it is closed onto.
pub const GAP_SNAP_MM: i64 = 300;

/// What a gap is closed with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CloseHow {
    /// A room separator: two rooms, no wall.
    Separator,
    /// A wall, of the owner's thickness and height (mm).
    Wall { thickness: i64, height: i64 },
}

/// The two ends of a gap moved onto the walls it lies between: at each end the end of a wall in the same line, else the axis of a wall
/// that crosses there, within `GAP_SNAP_MM`. A gap read from a picture ends at the crossing wall's face, or at a drawn stub too short
/// to be kept as a wall, so the distance to a crossing wall is measured to its face (half its thickness off its axis), not to its axis.
/// The line itself moves onto the axis of the wall in line. A line that stopped a few
/// millimetres short of a wall would bound nothing, so an end with no wall to stand on is an error, not a guess.
pub fn snap_gap(model: &Model, level: Option<Id>, a: [i64; 2], b: [i64; 2]) -> Result<([i64; 2], [i64; 2]), String> {
    if (a[0] == b[0]) == (a[1] == b[1]) {
        return Err("closing a gap: its two ends must share an x or a y, and differ".into());
    }
    let horizontal = a[1] == b[1];
    let (c0, lo, hi) = if horizontal { (a[1], a[0].min(b[0]), a[0].max(b[0])) } else { (a[0], a[1].min(b[1]), a[1].max(b[1])) };
    // (horizontal, axis, lo, hi, thickness) of every wall of the level.
    let walls: Vec<(bool, i64, i64, i64, i64)> = model
        .elements
        .values()
        .filter(|e| e.kind == Wall::KIND && e.level_id == level)
        .filter_map(|e| e.typed::<Wall>().ok())
        .filter(|w| (w.x1 == w.x2) != (w.y1 == w.y2))
        .map(|w| if w.y1 == w.y2 { (true, w.y1, w.x1.min(w.x2), w.x1.max(w.x2), w.thickness) } else { (false, w.x1, w.y1.min(w.y2), w.y1.max(w.y2), w.thickness) })
        .collect();
    // The wall in line whose end is nearest to this edge of the gap: (its end, its axis).
    let in_line = |edge: i64, before: bool| {
        walls.iter().filter(|w| w.0 == horizontal && (w.1 - c0).abs() <= GAP_SNAP_MM).map(|w| (if before { w.3 } else { w.2 }, w.1)).filter(|(end, _)| (end - edge).abs() <= GAP_SNAP_MM).min_by_key(|(end, axis)| ((end - edge).abs(), (axis - c0).abs()))
    };
    let (l, h) = (in_line(lo, true), in_line(hi, false));
    if let (Some(l), Some(h)) = (l, h)
        && l.1 != h.1
    {
        return Err(format!("closing a gap: the walls at its two ends are not in one line ({} mm apart): draw it by hand", (l.1 - h.1).abs()));
    }
    let c = l.or(h).map_or(c0, |x| x.1);
    let crossing = |edge: i64| walls.iter().filter(|w| w.0 != horizontal && w.2 <= c && c <= w.3 && (w.1 - edge).abs() <= GAP_SNAP_MM + w.4 / 2).map(|w| w.1).min_by_key(|x| (x - edge).abs());
    let end = |found: Option<(i64, i64)>, edge: i64, what: &str| found.map(|x| x.0).or_else(|| crossing(edge)).ok_or_else(|| format!("closing a gap: no wall ends or crosses within {GAP_SNAP_MM} mm of its {what}: draw it by hand"));
    let (p, q) = (end(l, lo, "start")?, end(h, hi, "end")?);
    Ok(if horizontal { ([p, c], [q, c]) } else { ([c, p], [c, q]) })
}

/// The rooms of a level, each one rectangle, that are narrower (between the axes of their walls) than the narrowest door of that level:
/// `(that door's width, the rooms' names)`. No door could stand in the short side of such a room: a duct, or the space between two walls
/// drawn close together, most likely. The measure comes from the level's own doors, so a level without a door has none. The rooms are
/// only named: whether one is a room is the owner's to say.
pub fn narrow_rooms(model: &Model, level: Option<Id>) -> Option<(i64, Vec<String>)> {
    let door = model.elements.values().filter(|e| e.kind == "Door" && e.level_id == level).filter_map(|e| e.props.get("width").and_then(|v| v.as_i64())).min()?;
    let spaces: Vec<crate::typed::Space> = model.elements.values().filter(|e| e.kind == "Space" && e.level_id == level).filter_map(|e| e.typed::<crate::typed::Space>().ok()).collect();
    // A room made of several rectangles is not judged by one of its parts.
    let in_parts: std::collections::BTreeSet<&str> = spaces.iter().filter_map(annot::part_of).collect();
    let mut names: Vec<String> = spaces.iter().filter(|s| annot::part_of(s).is_none() && !in_parts.contains(s.requirement_key.as_str()) && s.w.min(s.h) < door).map(|s| s.requirement_key.clone()).collect();
    names.sort();
    (!names.is_empty()).then_some((door, names))
}

/// Closes a gap with a door: the walls beside the gap become one wall across it (`draft::bridge`), and a door as wide as the gap, of
/// the owner's door height, stands where the gap was. `thickness` and `height` are the owner's wall values, used only where no wall in
/// line stands at either end. Returns the commands of the whole step and the door's id. Refused, with the reason, where the door does
/// not fit its wall or would stand in a column.
pub fn close_gap_with_door(model: &Model, level: Option<Id>, a: [i64; 2], b: [i64; 2], thickness: i64, height: i64, door_height: i64) -> Result<(Vec<Command>, Id), String> {
    let (p, q) = snap_gap(model, level, a, b)?;
    let (edit, host, start) = draft::bridge(model, level, p, q, thickness, height)?;
    // The door is placed on the wall as it will be, so it is checked against that wall: its ends, its other openings, its columns.
    let mut scratch = Document::new(model.clone());
    scratch.commit("bridge", Origin::User, edit.cmds.clone()).map_err(|e| e.to_string())?;
    let horizontal = a[1] == b[1];
    let (lo, hi) = if horizontal { (a[0].min(b[0]), a[0].max(b[0])) } else { (a[1].min(b[1]), a[1].max(b[1])) };
    let width = (hi - lo + 5) / 10 * 10;
    let (door, id) = draft::opening_command(&scratch.model, &draft::OpeningSpec { kind: draft::OpeningKind::Door, wall: host, offset: (lo + hi) / 2 - start, width, height: door_height, sill: None, side: None })?;
    let mut cmds = edit.cmds;
    cmds.extend(door);
    Ok((cmds, id))
}

/// The command that closes a gap between the walls beside it (see `snap_gap`), and the id of what it creates.
pub fn close_gap(model: &Model, level: Option<Id>, a: [i64; 2], b: [i64; 2], how: CloseHow) -> Result<(Command, Id), String> {
    let (p, q) = snap_gap(model, level, a, b)?;
    match how {
        CloseHow::Separator => annot::separator_command(model, level, (p[0], p[1]), (q[0], q[1])),
        CloseHow::Wall { thickness, height } => draft::wall_command(model, level, &WallSpec { a: p, b: q, thickness, height }),
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub levels: Vec<LevelReport>,
    pub notes: Vec<String>,
}

fn need(v: Option<i64>, what: &str) -> Result<i64, String> {
    v.filter(|n| *n > 0).ok_or_else(|| format!("plan import: {what} is missing or not above zero: it is the owner's to give, there is no default"))
}

/// A reason with its numbers and ids taken out, so that equal reasons are counted together.
fn reason(e: &str) -> String {
    let mut out = String::new();
    let mut in_digits = false;
    let mut depth = 0;
    for ch in e.chars() {
        match ch {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            _ if depth > 0 => {}
            d if d.is_ascii_digit() => {
                if !in_digits {
                    out.push('N');
                }
                in_digits = true;
                continue;
            }
            c => out.push(c),
        }
        in_digits = false;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

struct Read {
    level: String,
    name: String,
    fit: SheetFit,
    found: usize,
    walls: rasterwalls::FoundWalls,
}

pub fn import(spec: &Spec, drawings: &[Drawing]) -> Result<(Document, Report), String> {
    let storey = need(spec.storey_mm, "the storey height (storey_mm)")?;
    let thick = need(spec.wall_thickness_mm, "the wall thickness (wall_thickness_mm)")?;
    let door_h = need(spec.door_height_mm, "the door height (door_height_mm)")?;
    let win_h = need(spec.window_height_mm, "the window height (window_height_mm)")?;
    let sill = spec.window_sill_mm.filter(|s| *s >= 0).ok_or("plan import: the window sill height (window_sill_mm) is missing: it is the owner's to give, there is no default")?;
    if drawings.is_empty() {
        return Err("plan import: give at least one drawing".into());
    }
    {
        let mut names: Vec<String> = drawings.iter().map(|d| d.level.trim().to_lowercase()).collect();
        names.sort();
        if names.iter().any(String::is_empty) || names.windows(2).any(|p| p[0] == p[1]) {
            return Err("plan import: every drawing needs its own level name".into());
        }
    }
    if let Some([short, long]) = spec.column_mm
        && (short <= 0 || long < short)
    {
        return Err("plan import: the column's sides (column_mm) are the short side then the long side, both above zero".into());
    }
    for (list, what) in [(&spec.door_widths_mm, "door_widths_mm"), (&spec.window_widths_mm, "window_widths_mm")] {
        if list.as_ref().is_some_and(|l| l.is_empty() || l.iter().any(|w| *w <= 0)) {
            return Err(format!("plan import: the owner's widths ({what}) must be a list of widths above zero, or left out"));
        }
    }
    let mut report = Report::default();
    // 1 and 2. Each drawing: its bubbles, its scale and place, its walls.
    let mut reads: Vec<Read> = Vec::new();
    for d in drawings {
        let (lo, hi) = match spec.bubble_radius_px {
            Some([a, b]) => (a, b),
            None => {
                let r = ((d.width / 60).max(4), (d.width / 25).max(6));
                report.notes.push(format!("{}: the bubble radius was taken as {} to {} px (a 60th to a 25th of the image width); give it if the drawing's bubbles are other", d.name, r.0, r.1));
                r
            }
        };
        let bubbles = planaxes::find_bubbles(&d.rgb, d.width, d.height, lo, hi).map_err(|e| format!("{}: {e}", d.name))?;
        let sheet = planaxes::fit_sheet(&bubbles, &spec.x_axes, &spec.y_axes).map_err(|e| format!("{}: {e}", d.name))?;
        let fit = sheet.fit;
        let walls = rasterwalls::read_walls(&d.rgb, d.width, d.height, &WallParams { mm_per_px: fit.mm_per_px, wall_colours: d.wall_colours.clone(), ..Default::default() }).map_err(|e| format!("{}: {e}", d.name))?;
        reads.push(Read { level: d.level.trim().to_string(), name: d.name.clone(), fit, found: bubbles.len(), walls });
    }
    // World coordinates: x as the axes give it, y up. Shifted so every coordinate is positive, by the same amount for every level.
    let (mut min_x, mut max_y) = (f64::MAX, f64::MIN);
    for r in &reads {
        let (ox, oy, s) = (r.fit.x_offset_mm, r.fit.y_offset_mm, r.fit.mm_per_px);
        for w in &r.walls.walls {
            let (a, b, c) = (w.lo_px, w.hi_px, w.axis_px);
            let (xlo, xhi, ytop, ybot) = if w.horizontal { (ox + s * a, ox + s * b, oy + s * c, oy + s * c) } else { (ox + s * c, ox + s * c, oy + s * a, oy + s * b) };
            min_x = min_x.min(xlo.min(xhi));
            max_y = max_y.max(ytop.max(ybot));
        }
    }
    let (shift_x, top_y) = (2000.0 - min_x, max_y + 2000.0);
    // 3. The model.
    let mut doc = Document::default();
    let mut cmds = vec![project::create_command(&NanoArchProject { meta: ProjectMeta { title: spec.title.trim().to_string(), structure_from_drawings: true, ..Default::default() }, ..Default::default() })?];
    for (i, r) in reads.iter().enumerate() {
        cmds.push(Command::Create(element_of(&Level { name: r.level.clone(), index: i as i64, extra: Default::default() }, Source::User)));
    }
    doc.commit("plan import: project and levels", Origin::User, cmds).map_err(|e| e.to_string())?;
    let level_ids: Vec<Id> = building::levels(&doc.model).into_iter().map(|(_, id)| id).collect();
    // The axes of the drawings, as grid lines of the model under the names the owner typed. They run past the outermost axes by 1.5 m.
    let axes_x: Vec<(String, i64)> = spec.x_axes.iter().map(|a| (a.name.clone(), (a.pos_mm + shift_x).round() as i64)).collect();
    let axes_y: Vec<(String, i64)> = spec.y_axes.iter().map(|a| (a.name.clone(), (top_y - a.pos_mm).round() as i64)).collect();
    {
        let span = |v: &[(String, i64)]| (v.iter().map(|a| a.1).min().unwrap_or(0) - 1500, v.iter().map(|a| a.1).max().unwrap_or(0) + 1500);
        let ((x0, x1), (y0, y1)) = (span(&axes_x), span(&axes_y));
        let lines = axes_x.iter().map(|(n, x)| (n, [*x, y0], [*x, y1])).chain(axes_y.iter().map(|(n, y)| (n, [x0, *y], [x1, *y])));
        for (name, a, b) in lines {
            match annot::grid_command(&doc.model, &annot::GridSpec { a, b, name: Some(name.clone()) }) {
                Ok((cmd, _)) => doc.commit("plan import: axis", Origin::User, vec![cmd]).map_err(|e| e.to_string())?,
                Err(e) => report.notes.push(format!("axis {name} was not drawn: {e}")),
            }
        }
    }
    for (r, level_id) in reads.iter().zip(level_ids) {
        let level = Some(level_id);
        let (ox, oy, s) = (r.fit.x_offset_mm, r.fit.y_offset_mm, r.fit.mm_per_px);
        let wx = |px: f64| (ox + s * px + shift_x).round() as i64;
        let wy = |py: f64| (top_y - (oy + s * py)).round() as i64;
        let mut lr = LevelReport {
            level: r.level.clone(),
            drawing: r.name.clone(),
            mm_per_px: s,
            origin_x_mm: ox + shift_x,
            origin_y_mm: top_y - oy,
            bubbles_found: r.found,
            column_bubbles_fitted: r.fit.x.matched.len(),
            row_bubbles_fitted: r.fit.y.matched.len(),
            walls_read: r.walls.walls.len(),
            notes: r.fit.notes.iter().chain(r.walls.notes.iter()).cloned().collect(),
            ..Default::default()
        };
        let kinds = r.walls.openings.iter().map(|o| o.kind).chain(r.walls.passages.iter().chain(&r.walls.left_open).map(|g| g.kind));
        for kind in kinds {
            let k = match kind {
                OpeningKind::Window => "window",
                OpeningKind::Door => "door",
                OpeningKind::Passage => "passage",
                OpeningKind::Unclear => "unclear",
            };
            *lr.openings_read.entry(k.to_string()).or_default() += 1;
        }
        let mut ids: Vec<Option<Id>> = Vec::new();
        let mut composite = 0;
        for w in &r.walls.walls {
            let (a, b) = if w.horizontal { ([wx(w.lo_px), wy(w.axis_px)], [wx(w.hi_px), wy(w.axis_px)]) } else { ([wx(w.axis_px), wy(w.hi_px)], [wx(w.axis_px), wy(w.lo_px)]) };
            match draft::wall_command(&doc.model, level, &WallSpec { a, b, thickness: thick, height: storey }) {
                Ok((cmd, id)) => {
                    doc.commit("plan import: wall", Origin::User, vec![cmd]).map_err(|e| format!("{}: {e}", r.level))?;
                    ids.push(Some(id));
                    lr.walls_built += 1;
                    let sync = annot::rooms_sync(&doc.model, level);
                    composite = sync.composite;
                    if !sync.cmds.is_empty() {
                        doc.commit("plan import: rooms", Origin::User, sync.cmds).map_err(|e| format!("{}: {e}", r.level))?;
                    }
                }
                Err(e) => {
                    ids.push(None);
                    *lr.walls_refused.entry(reason(&e)).or_default() += 1;
                }
            }
        }
        // The passages first: a separator can split a room, and the doors and windows below must name the rooms as they end up.
        for g in &r.walls.passages {
            let (a, b) = if g.horizontal { ([wx(g.lo_px), wy(g.axis_px)], [wx(g.hi_px), wy(g.axis_px)]) } else { ([wx(g.axis_px), wy(g.hi_px)], [wx(g.axis_px), wy(g.lo_px)]) };
            match annot::separator_command(&doc.model, level, (a[0], a[1]), (b[0], b[1])) {
                Ok((cmd, _)) => {
                    doc.commit("plan import: passage", Origin::User, vec![cmd]).map_err(|e| format!("{}: {e}", r.level))?;
                    lr.passages_built += 1;
                    let sync = annot::rooms_sync(&doc.model, level);
                    composite = sync.composite;
                    if !sync.cmds.is_empty() {
                        doc.commit("plan import: rooms", Origin::User, sync.cmds).map_err(|e| format!("{}: {e}", r.level))?;
                    }
                }
                Err(e) => *lr.openings_not_built.entry(reason(&e)).or_default() += 1,
            }
        }
        // Every wall was drawn before the rooms it bounds existed: now that they do, each learns its rooms and whether it is exterior.
        let learnt = draft::refresh_walls(&doc.model, level);
        if !learnt.is_empty() {
            doc.commit("plan import: the walls' rooms", Origin::User, learnt).map_err(|e| format!("{}: {e}", r.level))?;
        }
        // (took a width of the owner's list, kept the width read) for doors, then for windows.
        let mut typed = [[0usize; 2]; 2];
        let reach = (2.0 * s).round() as i64;
        for o in &r.walls.openings {
            let (kind, label) = match o.kind {
                OpeningKind::Door => (BuildKind::Door, "door"),
                OpeningKind::Window => (BuildKind::Window, "window"),
                // The reader puts only windows and doors among the openings; the empty gaps are its passages and open gaps.
                OpeningKind::Passage | OpeningKind::Unclear => continue,
            };
            let w = &r.walls.walls[o.wall];
            let Some(host) = ids[o.wall] else {
                *lr.openings_not_built.entry("its wall was not built".into()).or_default() += 1;
                continue;
            };
            let centre = (o.lo_px + o.hi_px) / 2.0;
            let (start, along) = if w.horizontal { (wx(w.lo_px), wx(centre)) } else { (wy(w.hi_px), wy(centre)) };
            let read = (((o.hi_px - o.lo_px) * s) / 10.0).round() as i64 * 10;
            let (list, row) = if label == "door" { (&spec.door_widths_mm, 0) } else { (&spec.window_widths_mm, 1) };
            let width = match list {
                Some(l) => match standard_width(read, l, reach) {
                    Some(w) => {
                        typed[row][0] += 1;
                        w
                    }
                    None => {
                        typed[row][1] += 1;
                        read
                    }
                },
                None => read,
            };
            let spec_o = OpeningSpec { kind, wall: host, offset: along - start, width, height: if label == "door" { door_h } else { win_h }, sill: (label == "window").then_some(sill), side: (label == "door").then_some(1) };
            let outcome = draft::opening_command(&doc.model, &spec_o).and_then(|(cmds, _)| doc.commit("plan import: opening", Origin::User, cmds).map_err(|e| e.to_string()));
            match outcome {
                Ok(()) if label == "door" => lr.doors_built += 1,
                Ok(()) => lr.windows_built += 1,
                Err(e) => *lr.openings_not_built.entry(reason(&e)).or_default() += 1,
            }
        }
        for (row, what) in [(0, "door"), (1, "window")] {
            let [took, kept] = typed[row];
            if took + kept > 0 {
                lr.notes.push(format!("{took} {what}(s) took a width of the owner's list (the width read was within {reach} mm of it, two pixels of this drawing); {kept} kept the width read, which is farther than that from every width of the list"));
            }
        }
        // Columns: where two axes cross on a wall of this level. The long side runs along the wall the column stands in.
        if let Some([short, long]) = spec.column_mm {
            let walls: Vec<crate::typed::Wall> = doc.model.elements.values().filter(|e| e.kind == "Wall" && e.level_id == level).filter_map(|e| e.typed().ok()).collect();
            // The longest wall of each direction that passes through the point, within one wall thickness of its centre line.
            let through = |x: i64, y: i64, horizontal: bool| {
                walls.iter().filter(|w| w.horizontal() == horizontal).filter(|w| {
                    let (along, across) = if horizontal { (x, y) } else { (y, x) };
                    (across - w.centre_line()).abs() <= thick && along >= w.start() - thick && along <= w.start() + w.length + thick
                }).map(|w| w.length).max()
            };
            let mut cmds = Vec::new();
            let mut by_longer = 0;
            for (xn, x) in &axes_x {
                for (yn, y) in &axes_y {
                    let (b, h) = match (through(*x, *y, true), through(*x, *y, false)) {
                        (None, None) => {
                            lr.crossings_without_wall.push(format!("{xn}/{yn}"));
                            continue;
                        }
                        (Some(_), None) => (long, short),
                        (None, Some(_)) => (short, long),
                        (Some(lh), Some(lv)) => {
                            by_longer += usize::from(long != short);
                            if lh >= lv { (long, short) } else { (short, long) }
                        }
                    };
                    let mut el = element_of(&crate::typed::Column { x: *x, y: *y, b, h, grid_ref: format!("{xn}/{yn}"), axial_service_kn: None, extra: Default::default() }, Source::User);
                    el.level_id = level;
                    cmds.push(Command::Create(el));
                }
            }
            lr.columns_built = cmds.len();
            if !cmds.is_empty() {
                doc.commit("plan import: columns on the axes", Origin::User, cmds).map_err(|e| format!("{}: {e}", r.level))?;
            }
            if by_longer > 0 {
                lr.notes.push(format!("{by_longer} column(s) stand where two walls cross: their long side was laid along the longer wall; turn the ones the drawing shows otherwise"));
            }
            let (_, in_columns) = building::clear_openings(&doc.model, &[level]);
            if !in_columns.is_empty() {
                lr.notes.push(format!("{} door(s) or window(s) stand where a column was placed on the axes: check the drawing there", in_columns.len()));
            }
        }
        lr.gaps_left_open = r.walls.left_open.len();
        for (n, g) in r.walls.left_open.iter().enumerate() {
            let (a, b, px) = if g.horizontal { ([wx(g.lo_px), wy(g.axis_px)], [wx(g.hi_px), wy(g.axis_px)], [g.lo_px, g.axis_px, g.hi_px, g.axis_px]) } else { ([wx(g.axis_px), wy(g.hi_px)], [wx(g.axis_px), wy(g.lo_px)], [g.axis_px, g.lo_px, g.axis_px, g.hi_px]) };
            lr.open_gaps.push(OpenGap { n: n + 1, a, b, width_mm: (((g.hi_px - g.lo_px) * s) / 10.0).round() as i64 * 10, kind: if g.kind == OpeningKind::Passage { "passage" } else { "unclear" }, px, row: None });
        }
        let in_rows = rows_of_equal_gaps(&mut lr.open_gaps);
        if in_rows > 0 {
            lr.notes.push(format!("{} of the gaps left open stand in {in_rows} row(s) of equal gaps side by side (a stair's treads, most likely): they are listed together", lr.open_gaps.iter().filter(|g| g.row.is_some()).count()));
        }
        lr.rooms = doc.model.elements.values().filter(|e| e.kind == "Space" && e.level_id == level && !e.props.contains_key("partOf")).count();
        if let Some((door, names)) = narrow_rooms(&doc.model, level) {
            lr.notes.push(format!("{} room(s) are narrower than the narrowest door read on this level ({door} mm): {}. A duct or the space between two walls, most likely: delete the ones that are not rooms", names.len(), names.join(", ")));
        }
        lr.composite_rooms = composite;
        report.levels.push(lr);
    }
    doc.mark_saved();
    // Without the owner's widths every opening keeps the width read: say how many different ones there are, and how far a picture can be trusted.
    for (kind, list, what, field) in [("Door", &spec.door_widths_mm, "door", "door widths"), ("Window", &spec.window_widths_mm, "window", "window widths")] {
        let widths: std::collections::BTreeSet<i64> = doc.model.elements.values().filter(|e| e.kind == kind).filter_map(|e| e.props.get("width").and_then(|v| v.as_i64())).collect();
        let coarsest = report.levels.iter().map(|l| l.mm_per_px).fold(0.0_f64, f64::max);
        if list.is_none() && widths.len() > 1 {
            report.notes.push(format!("{} different {what} widths were read. A width read from these pictures is good to about {} mm (two pixels): give the {field} you build with, and each {what} within that of one of them takes it", widths.len(), (2.0 * coarsest).round() as i64));
        }
    }
    report.notes.push(match spec.column_mm {
        Some([short, long]) => format!("the columns stand where the drawings' axes cross on a wall, {short} x {long} mm as the owner gave: a proposal from the axes, not a structural design. No beam, slab or footing was generated, and later edits generate none"),
        None => "no structure was generated: no column size was given, so no column was placed (the axes are drawn), and later edits generate none. Structure comes from a design, not from the architecture drawings".into(),
    });
    report.notes.push("every height and the wall thickness are the owner's values, applied to every wall and opening; the drawings carry none of them".into());
    Ok((doc, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 6 x 4 m box with a partition at x = 3000 that has a 2 m gap in its middle (y 1000..3000).
    fn box_with_a_gap() -> (Document, Option<Id>) {
        let mut doc = Document::default();
        doc.commit("level", Origin::User, vec![Command::Create(element_of(&Level { name: "G".into(), index: 0, extra: Default::default() }, Source::User))]).unwrap();
        let level = building::levels(&doc.model).first().map(|l| l.1);
        for (a, b) in [([0, 0], [6000, 0]), ([6000, 0], [6000, 4000]), ([6000, 4000], [0, 4000]), ([0, 4000], [0, 0]), ([3000, 0], [3000, 1000]), ([3000, 3000], [3000, 4000])] {
            let (cmd, _) = draft::wall_command(&doc.model, level, &WallSpec { a, b, thickness: 200, height: 3000 }).unwrap();
            doc.commit("wall", Origin::User, vec![cmd]).unwrap();
        }
        (doc, level)
    }

    fn rooms_after(doc: &mut Document, level: Option<Id>, cmd: Command) -> usize {
        doc.commit("close", Origin::User, vec![cmd]).unwrap();
        let sync = annot::rooms_sync(&doc.model, level);
        doc.commit("rooms", Origin::User, sync.cmds).unwrap();
        doc.model.elements.values().filter(|e| e.kind == "Space" && !e.props.contains_key("partOf")).count()
    }

    #[test]
    fn equal_gaps_side_by_side_are_one_row_and_a_gap_that_differs_or_stands_apart_is_not() {
        let gap = |n: usize, a: [i64; 2], b: [i64; 2]| OpenGap { n, a, b, width_mm: (a[0] - b[0]).abs().max((a[1] - b[1]).abs()), kind: "unclear", px: [0.0; 4], row: None };
        // Six treads 1180 mm wide, 400 mm apart; a wider gap among them; one equal gap 3 m away; three equal gaps of another kind.
        let mut gaps: Vec<OpenGap> = (0..6).map(|i| gap(i + 1, [8000, 5000 + 400 * i as i64], [9180, 5000 + 400 * i as i64])).collect();
        gaps.push(gap(7, [8000, 5200], [10010, 5200]));
        gaps.push(gap(8, [8000, 10000], [9180, 10000]));
        gaps.extend((0..3).map(|i| gap(9 + i, [2000 + 600 * i as i64, 1000], [2000 + 600 * i as i64, 2270])));
        assert_eq!(rows_of_equal_gaps(&mut gaps), 1);
        assert_eq!(gaps.iter().map(|g| g.row).collect::<Vec<_>>(), [vec![Some(1); 6], vec![None; 5]].concat(), "the treads, and nothing else");
        let note = "6 of the gaps left open stand in 1 row(s) of equal gaps side by side (a stair's treads, most likely): they are listed together";
        assert!(crate::msgs::localize(note).1, "the note has its Arabic");
    }

    #[test]
    fn a_gap_is_closed_onto_the_wall_ends_beside_it_by_a_separator_or_a_wall() {
        // The gap as a picture gives it: a little off the line and short at both ends.
        let (off_a, off_b) = ([3040, 1090], [3040, 2920]);
        let (mut doc, level) = box_with_a_gap();
        assert_eq!(snap_gap(&doc.model, level, off_a, off_b).unwrap(), ([3000, 1000], [3000, 3000]));
        let (cmd, _) = close_gap(&doc.model, level, off_a, off_b, CloseHow::Separator).unwrap();
        assert_eq!(rooms_after(&mut doc, level, cmd), 2, "the separator, standing on both wall ends, makes two rooms");
        let (mut doc, level) = box_with_a_gap();
        let (cmd, _) = close_gap(&doc.model, level, off_a, off_b, CloseHow::Wall { thickness: 200, height: 3000 }).unwrap();
        assert_eq!(rooms_after(&mut doc, level, cmd), 2);
        assert_eq!(doc.model.elements.values().filter(|e| e.kind == "Wall").count(), 7);
    }

    #[test]
    fn a_gap_closed_with_a_door_joins_the_walls_beside_it_and_keeps_their_openings_in_place() {
        // A window on the partition's upper piece, 500 mm from that piece's start (y 3500 in the plan).
        let (mut doc, level) = box_with_a_gap();
        let wall_at = |doc: &Document, y1: i64| doc.model.elements.values().find(|e| e.kind == "Wall" && e.props["x1"] == 3000 && e.props["x2"] == 3000 && e.props["y1"] == y1).map(|e| e.id).unwrap();
        let upper = wall_at(&doc, 3000);
        let (cmds, window) = draft::opening_command(&doc.model, &draft::OpeningSpec { kind: draft::OpeningKind::Window, wall: upper, offset: 500, width: 400, height: 1000, sill: Some(900), side: None }).unwrap();
        doc.commit("window", Origin::User, cmds).unwrap();
        let _ = window;
        // The gap as a picture gives it (y 1090..2910), closed with a door 2 m high.
        let (cmds, door) = close_gap_with_door(&doc.model, level, [3040, 1090], [3040, 2910], 200, 3000, 2000).unwrap();
        doc.commit("door", Origin::User, cmds).unwrap();
        let walls: Vec<Wall> = doc.model.elements.values().filter(|e| e.kind == "Wall").filter_map(|e| e.typed::<Wall>().ok()).filter(|w| w.x1 == 3000 && w.x2 == 3000).collect();
        assert_eq!(walls.len(), 1, "the two pieces are one wall now");
        assert_eq!((walls[0].y1, walls[0].y2, walls[0].length), (0, 4000, 4000));
        let host = wall_at(&doc, 0);
        let d = doc.model.get(&door).unwrap();
        assert_eq!((d.host_id, d.props["offsetCenter"].as_i64(), d.props["width"].as_i64(), d.props["height"].as_i64()), (Some(host), Some(2000), Some(1820), Some(2000)), "the door stands where the gap was, as wide as the gap");
        let w = doc.model.elements.values().find(|e| e.kind == "Window").unwrap();
        assert_eq!((w.host_id, w.props["offsetCenter"].as_i64()), (Some(host), Some(3500)), "the window is on the joined wall, at the same place in the plan");
        let sync = annot::rooms_sync(&doc.model, level);
        doc.commit("rooms", Origin::User, sync.cmds).unwrap();
        assert_eq!(doc.model.elements.values().filter(|e| e.kind == "Space" && !e.props.contains_key("partOf")).count(), 2, "the joined wall divides the box");
        // One undo takes the door and the join away together.
        assert!(doc.undo() && doc.undo());
        assert_eq!(doc.model.elements.values().filter(|e| e.kind == "Wall").count(), 6);
        assert!(doc.model.elements.values().all(|e| e.kind != "Door"));
        // Walls that differ are not joined: the refusal says so and has its Arabic.
        let (mut d2, level2) = box_with_a_gap();
        let up = wall_at(&d2, 3000);
        d2.commit("thicker", Origin::User, vec![Command::SetProp { id: up, key: "thickness".into(), value: Some(serde_json::json!(300)) }]).unwrap();
        let e = close_gap_with_door(&d2.model, level2, [3000, 1090], [3000, 2910], 200, 3000, 2000).unwrap_err();
        assert!(e.contains("differ") && crate::msgs::localize(&e).1, "{e}");
    }

    #[test]
    fn a_room_narrower_than_the_narrowest_door_of_its_level_is_named_and_a_level_without_a_door_names_none() {
        // A 6 x 4 m box with two partitions 400 mm apart: three rooms, the middle one 400 mm between axes.
        let mut doc = Document::default();
        doc.commit("level", Origin::User, vec![Command::Create(element_of(&Level { name: "G".into(), index: 0, extra: Default::default() }, Source::User))]).unwrap();
        let level = building::levels(&doc.model).first().map(|l| l.1);
        let mut south = None;
        for (a, b) in [([0, 0], [6000, 0]), ([6000, 0], [6000, 4000]), ([6000, 4000], [0, 4000]), ([0, 4000], [0, 0]), ([3000, 0], [3000, 4000]), ([3400, 0], [3400, 4000])] {
            let (cmd, id) = draft::wall_command(&doc.model, level, &WallSpec { a, b, thickness: 200, height: 3000 }).unwrap();
            doc.commit("wall", Origin::User, vec![cmd]).unwrap();
            south = south.or(Some(id));
        }
        let sync = annot::rooms_sync(&doc.model, level);
        doc.commit("rooms", Origin::User, sync.cmds).unwrap();
        assert_eq!(narrow_rooms(&doc.model, level), None, "no door on the level: nothing to measure against");
        let (cmds, _) = draft::opening_command(&doc.model, &draft::OpeningSpec { kind: draft::OpeningKind::Door, wall: south.unwrap(), offset: 1500, width: 900, height: 2000, sill: None, side: None }).unwrap();
        doc.commit("door", Origin::User, cmds).unwrap();
        let (door, names) = narrow_rooms(&doc.model, level).unwrap();
        assert_eq!((door, names.len()), (900, 1));
        let narrow = doc.model.elements.values().filter(|e| e.kind == "Space").filter_map(|e| e.typed::<crate::typed::Space>().ok()).find(|s| s.requirement_key == names[0]).unwrap();
        assert_eq!(narrow.w, 400);
        let note = format!("1 room(s) are narrower than the narrowest door read on this level (900 mm): {}. A duct or the space between two walls, most likely: delete the ones that are not rooms", names[0]);
        assert!(crate::msgs::localize(&note).1, "the note has its Arabic");
    }

    #[test]
    fn a_width_read_takes_the_owners_width_only_within_two_pixels_of_it() {
        // 43.6 mm per pixel: two pixels are 87 mm.
        let list = [800, 900, 1000];
        assert_eq!(standard_width(920, &list, 87), Some(900));
        assert_eq!(standard_width(960, &list, 87), Some(1000));
        assert_eq!(standard_width(950, &list, 87), Some(900), "midway: the smaller, always the same one");
        assert_eq!(standard_width(1180, &list, 87), None, "farther than the picture's reach from every width: kept as read");
        assert_eq!(standard_width(740, &list, 87), Some(800));
        assert_eq!(standard_width(700, &list, 87), None);
        for note in [
            "9 door(s) took a width of the owner's list (the width read was within 87 mm of it, two pixels of this drawing); 2 kept the width read, which is farther than that from every width of the list",
            "0 window(s) took a width of the owner's list (the width read was within 87 mm of it, two pixels of this drawing); 9 kept the width read, which is farther than that from every width of the list",
            "6 different door widths were read. A width read from these pictures is good to about 87 mm (two pixels): give the door widths you build with, and each door within that of one of them takes it",
            "18 different window widths were read. A width read from these pictures is good to about 87 mm (two pixels): give the window widths you build with, and each window within that of one of them takes it",
            "plan import: the owner's widths (door_widths_mm) must be a list of widths above zero, or left out",
        ] {
            assert!(crate::msgs::localize(note).1, "no Arabic for: {note}");
        }
    }

    #[test]
    fn a_gap_end_with_no_wall_to_stand_on_is_refused_and_a_crossing_wall_serves_as_an_end() {
        let (doc, level) = box_with_a_gap();
        // From the partition's lower piece up to the top wall: its far end stands on the wall that crosses there.
        let (mut d2, level2) = box_with_a_gap();
        let low = d2.model.elements.values().find(|e| e.kind == "Wall" && e.props.get("y1").and_then(|v| v.as_i64()) == Some(3000) && e.props.get("x1").and_then(|v| v.as_i64()) == Some(3000)).map(|e| e.id).unwrap();
        d2.commit("drop", Origin::User, vec![Command::Delete(low)]).unwrap();
        assert_eq!(snap_gap(&d2.model, level2, [3000, 1100], [3000, 3900]).unwrap(), ([3000, 1000], [3000, 4000]));
        // A gap that stops short of the crossing wall's face (a drawn stub too short to be a wall stood there): 280 mm from the face of
        // the 200 mm top wall, 380 mm from its axis. It is closed onto that wall; 320 mm from the face is out of reach.
        assert_eq!(snap_gap(&d2.model, level2, [3000, 1100], [3000, 3620]).unwrap(), ([3000, 1000], [3000, 4000]));
        assert!(snap_gap(&d2.model, level2, [3000, 1100], [3000, 3580]).unwrap_err().contains("end"));
        // An end in the middle of the room has nothing within reach.
        let e = snap_gap(&doc.model, level, [1500, 2000], [2900, 2000]).unwrap_err();
        assert!(e.contains("start") && e.contains("300 mm"), "{e}");
        assert!(snap_gap(&doc.model, level, [10, 10], [900, 800]).unwrap_err().contains("share an x or a y"));
    }

    const W: usize = 320;
    const H: usize = 240;
    const WALL: [u8; 3] = [28, 124, 108];

    fn paint(g: &mut [u8], x0: usize, y0: usize, x1: usize, y1: usize, c: [u8; 3]) {
        for y in y0..y1 {
            for x in x0..x1 {
                g[(y * W + x) * 3..(y * W + x) * 3 + 3].copy_from_slice(&c);
            }
        }
    }

    fn ring(g: &mut [u8], cx: f64, cy: f64) {
        for y in 0..H {
            for x in 0..W {
                if ((x as f64 - cx).hypot(y as f64 - cy) - 13.0).abs() <= 1.0 {
                    g[(y * W + x) * 3..(y * W + x) * 3 + 3].copy_from_slice(&[150, 160, 160]);
                }
            }
        }
    }

    /// A sheet at 50 mm/px whose first axis A (x = 0) is at pixel `ox` and whose first row axis 1 (y = 0) is at pixel `oy`: axes A, B, C at
    /// 0, 5000, 10000 mm across and 1, 2 at 0, 4000 mm down. Walls round the box A..C x 1..2 (6 px = 300 mm), a partition on B with a 1 m door gap
    /// (a swing line beside it) when `partition`, and a 1.5 m window in the top wall.
    fn sheet(ox: usize, oy: usize, partition: bool) -> Vec<u8> {
        let mut g = vec![240u8; W * H * 3];
        for k in 0..3 {
            ring(&mut g, (ox + k * 100) as f64, 26.0);
        }
        for k in 0..2 {
            ring(&mut g, (W - 26) as f64, (oy + k * 80) as f64);
        }
        let (x0, x1, y0, y1) = (ox, ox + 200, oy, oy + 80);
        paint(&mut g, x0 - 3, y0 - 3, x1 + 3, y0 + 3, WALL);
        paint(&mut g, x0 - 3, y1 - 3, x1 + 3, y1 + 3, WALL);
        paint(&mut g, x0 - 3, y0 - 3, x0 + 3, y1 + 3, WALL);
        paint(&mut g, x1 - 3, y0 - 3, x1 + 3, y1 + 3, WALL);
        paint(&mut g, x0 + 20, y0 - 3, x0 + 50, y0 + 3, [240, 240, 240]);
        paint(&mut g, x0 + 20, y0 - 3, x0 + 50, y0 - 2, [90, 100, 110]);
        paint(&mut g, x0 + 20, y0 - 1, x0 + 50, y0, [90, 100, 110]);
        paint(&mut g, x0 + 20, y0 + 2, x0 + 50, y0 + 3, [90, 100, 110]);
        if partition {
            paint(&mut g, ox + 97, y0 - 3, ox + 103, y1 + 3, WALL);
            paint(&mut g, ox + 97, oy + 30, ox + 103, oy + 50, [240, 240, 240]);
            paint(&mut g, ox + 105, oy + 30, ox + 125, oy + 31, WALL);
        }
        g
    }

    fn spec() -> Spec {
        Spec {
            title: "Synthetic".into(),
            storey_mm: Some(3000),
            wall_thickness_mm: Some(300),
            door_height_mm: Some(2000),
            window_height_mm: Some(1500),
            window_sill_mm: Some(500),
            x_axes: ["A", "B", "C"].iter().zip([0.0, 5000.0, 10000.0]).map(|(n, p)| Axis { name: (*n).into(), pos_mm: p }).collect(),
            y_axes: ["1", "2"].iter().zip([0.0, 4000.0]).map(|(n, p)| Axis { name: (*n).into(), pos_mm: p }).collect(),
            bubble_radius_px: None,
            column_mm: None,
            door_widths_mm: None,
            window_widths_mm: None,
        }
    }

    fn drawing(name: &str, level: &str, g: Vec<u8>) -> Drawing {
        Drawing { name: name.into(), level: level.into(), width: W as u32, height: H as u32, rgb: g, wall_colours: vec![WALL] }
    }

    fn walls_of(doc: &Document, level: Id) -> Vec<crate::typed::Wall> {
        doc.model.elements.values().filter(|e| e.kind == "Wall" && e.level_id == Some(level)).filter_map(|e| e.typed::<crate::typed::Wall>().ok()).collect()
    }

    #[test]
    fn the_axes_become_grid_lines_and_a_column_stands_where_two_cross_on_a_wall_with_its_long_side_along_the_wall() {
        let with_columns = Spec { column_mm: Some([200, 500]), ..spec() };
        let (doc, rep) = import(&with_columns, &[drawing("lower.png", "Ground", sheet(60, 70, true)), drawing("upper.png", "First", sheet(73, 79, false))]).unwrap_or_else(|e| panic!("{e}"));
        let mut names: Vec<String> = doc.model.elements.values().filter_map(|e| e.typed::<crate::typed::Grid>().ok().filter(|_| e.kind == "Grid")).map(|g| g.name).collect();
        names.sort();
        assert_eq!(names, ["1", "2", "A", "B", "C"], "the axes under the names the owner typed");
        // Six crossings on each level, each on a wall: A, B and C on the long walls along 1 and 2.
        assert_eq!((rep.levels[0].columns_built, rep.levels[1].columns_built), (6, 6), "{rep:#?}");
        assert!(rep.levels.iter().all(|l| l.crossings_without_wall.is_empty()));
        let cols: Vec<crate::typed::Column> = doc.model.elements.values().filter(|e| e.kind == "Column").filter_map(|e| e.typed().ok()).collect();
        assert_eq!(cols.len(), 12);
        // Every column stands in a 10 m horizontal wall (the corners too: it is longer than the 4 m side walls), so 500 runs along x.
        assert!(cols.iter().all(|c| (c.b, c.h) == (500, 200)), "{cols:?}");
        assert!(cols.iter().any(|c| c.grid_ref == "B/2"));
        // The same axis crossing is at the same place on both levels, and it is where the grid lines cross.
        let lv = building::levels(&doc.model);
        let at = |level: Id, r: &str| doc.model.elements.values().filter(|e| e.kind == "Column" && e.level_id == Some(level)).filter_map(|e| e.typed::<crate::typed::Column>().ok()).find(|c| c.grid_ref == r).map(|c| (c.x, c.y)).unwrap();
        assert_eq!(at(lv[0].1, "C/1"), at(lv[1].1, "C/1"));
        // Nothing else structural, the project says where its structure comes from, and the note says what the columns are.
        assert!(!doc.model.elements.values().any(|e| ["Beam", "Slab", "Footing"].contains(&e.kind.as_str())));
        assert!(project::get(&doc.model).unwrap().unwrap().meta.structure_from_drawings);
        assert!(rep.notes.iter().any(|n| n.contains("a proposal from the axes")), "{:?}", rep.notes);
        // Every note about the axes and the columns has its Arabic, with and without a column size, and so has the refusal of a sync.
        let said = [
            "2 column(s) stand where two walls cross: their long side was laid along the longer wall; turn the ones the drawing shows otherwise",
            "1 door(s) or window(s) stand where a column was placed on the axes: check the drawing there",
            "axis AX3 was not drawn: grid line AX3 already runs along that line there",
            "this project was read from drawings: its columns stand on the drawings' axes and no structure is derived from its rooms",
            "a window 1500 mm wide stands in a column on a 4000 mm wall: it was drawn there and is not moved (move it, or the column)",
        ];
        let plain_notes = import(&spec(), &[drawing("lower.png", "Ground", sheet(60, 70, true))]).unwrap().1.notes;
        for n in rep.notes.iter().chain(rep.levels.iter().flat_map(|l| &l.notes)).chain(&plain_notes).map(String::as_str).chain(said) {
            assert!(crate::msgs::localize(n).1, "no Arabic for: {n}");
        }
        // Without a size there is no column, and a size that is not short-then-long is refused.
        let (plain, _) = import(&spec(), &[drawing("lower.png", "Ground", sheet(60, 70, true))]).unwrap();
        assert!(!plain.model.elements.values().any(|e| e.kind == "Column"));
        assert!(import(&Spec { column_mm: Some([500, 200]), ..spec() }, &[drawing("lower.png", "Ground", sheet(60, 70, true))]).unwrap_err().contains("column_mm"));
    }

    #[test]
    fn two_drawings_with_different_crops_become_two_aligned_levels_with_their_walls_openings_and_rooms() {
        // The upper drawing is cropped differently: the same axes stand 13 px to the right and 9 px lower.
        let (doc, rep) = import(&spec(), &[drawing("lower.png", "Ground", sheet(60, 70, true)), drawing("upper.png", "First", sheet(73, 79, false))]).unwrap_or_else(|e| panic!("{e}"));
        let lv = building::levels(&doc.model);
        assert_eq!(lv.len(), 2);
        let (low, up) = (&rep.levels[0], &rep.levels[1]);
        assert!((low.mm_per_px - 50.0).abs() < 0.5 && (up.mm_per_px - 50.0).abs() < 0.5, "{low:?} {up:?}");
        assert_eq!((low.column_bubbles_fitted, low.row_bubbles_fitted), (3, 2));
        assert_eq!((low.walls_built, up.walls_built), (5, 4), "{rep:#?}");
        assert_eq!((low.rooms, up.rooms), (2, 1), "{rep:#?}");
        assert_eq!((low.doors_built, low.windows_built, up.doors_built, up.windows_built), (1, 1, 0, 1), "{rep:#?}");
        // The same wall is at the same place in both levels, whatever the crop: the west wall's axis (x = 0 on axis A).
        let x_of = |id: Id| {
            let mut xs: Vec<i64> = walls_of(&doc, id).iter().filter(|w| w.x1 == w.x2).map(|w| w.x1).collect();
            xs.sort_unstable();
            xs
        };
        let (a, b) = (x_of(lv[0].1), x_of(lv[1].1));
        assert_eq!(a.first(), b.first(), "{a:?} {b:?}");
        assert!((a.last().unwrap() - b.last().unwrap()).abs() <= 50, "{a:?} {b:?}");
        // The owner's values are what was drawn.
        assert!(walls_of(&doc, lv[0].1).iter().all(|w| w.height == 3000 && w.thickness == 300));
        assert!(rep.notes.iter().any(|n| n.contains("no structure")));
    }

    #[test]
    fn a_missing_owner_value_or_axis_or_bad_drawing_is_an_error_that_says_which() {
        let d = |g| vec![drawing("a.png", "Ground", g)];
        let mut s = spec();
        s.storey_mm = None;
        assert!(import(&s, &d(sheet(60, 70, true))).unwrap_err().contains("storey_mm"));
        let mut s = spec();
        s.window_sill_mm = None;
        assert!(import(&s, &d(sheet(60, 70, true))).unwrap_err().contains("window_sill_mm"));
        let mut s = spec();
        s.x_axes.truncate(1);
        assert!(import(&s, &d(sheet(60, 70, true))).unwrap_err().contains("at least two"));
        assert!(import(&spec(), &[]).unwrap_err().contains("at least one"));
        assert!(import(&spec(), &[drawing("a", "G", sheet(60, 70, true)), drawing("b", "g", sheet(60, 70, true))]).unwrap_err().contains("own level name"));
        // A sheet with no bubbles cannot be scaled: the answer says so and does not make up a scale.
        let blank = vec![240u8; W * H * 3];
        let e = import(&spec(), &d(blank)).unwrap_err();
        assert!(e.contains("bubble"), "{e}");
    }
}
