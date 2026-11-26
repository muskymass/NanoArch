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
}

/// How far (mm) an end of a gap may stand from the wall end or the crossing wall it is closed onto.
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
/// that crosses there, within `GAP_SNAP_MM`. The line itself moves onto the axis of the wall in line. A line that stopped a few
/// millimetres short of a wall would bound nothing, so an end with no wall to stand on is an error, not a guess.
pub fn snap_gap(model: &Model, level: Option<Id>, a: [i64; 2], b: [i64; 2]) -> Result<([i64; 2], [i64; 2]), String> {
    if (a[0] == b[0]) == (a[1] == b[1]) {
        return Err("closing a gap: its two ends must share an x or a y, and differ".into());
    }
    let horizontal = a[1] == b[1];
    let (c0, lo, hi) = if horizontal { (a[1], a[0].min(b[0]), a[0].max(b[0])) } else { (a[0], a[1].min(b[1]), a[1].max(b[1])) };
    // (horizontal, axis, lo, hi) of every wall of the level.
    let walls: Vec<(bool, i64, i64, i64)> = model
        .elements
        .values()
        .filter(|e| e.kind == Wall::KIND && e.level_id == level)
        .filter_map(|e| e.typed::<Wall>().ok())
        .filter(|w| (w.x1 == w.x2) != (w.y1 == w.y2))
        .map(|w| if w.y1 == w.y2 { (true, w.y1, w.x1.min(w.x2), w.x1.max(w.x2)) } else { (false, w.x1, w.y1.min(w.y2), w.y1.max(w.y2)) })
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
    let crossing = |edge: i64| walls.iter().filter(|w| w.0 != horizontal && w.2 <= c && c <= w.3 && (w.1 - edge).abs() <= GAP_SNAP_MM).map(|w| w.1).min_by_key(|x| (x - edge).abs());
    let end = |found: Option<(i64, i64)>, edge: i64, what: &str| found.map(|x| x.0).or_else(|| crossing(edge)).ok_or_else(|| format!("closing a gap: no wall ends or crosses within {GAP_SNAP_MM} mm of its {what}: draw it by hand"));
    let (p, q) = (end(l, lo, "start")?, end(h, hi, "end")?);
    Ok(if horizontal { ([p, c], [q, c]) } else { ([c, p], [c, q]) })
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
    let mut cmds = vec![project::create_command(&NanoArchProject { meta: ProjectMeta { title: spec.title.trim().to_string(), ..Default::default() }, ..Default::default() })?];
    for (i, r) in reads.iter().enumerate() {
        cmds.push(Command::Create(element_of(&Level { name: r.level.clone(), index: i as i64, extra: Default::default() }, Source::User)));
    }
    doc.commit("plan import: project and levels", Origin::User, cmds).map_err(|e| e.to_string())?;
    let level_ids: Vec<Id> = building::levels(&doc.model).into_iter().map(|(_, id)| id).collect();
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
            let width = (((o.hi_px - o.lo_px) * s) / 10.0).round() as i64 * 10;
            let spec_o = OpeningSpec { kind, wall: host, offset: along - start, width, height: if label == "door" { door_h } else { win_h }, sill: (label == "window").then_some(sill), side: (label == "door").then_some(1) };
            let outcome = draft::opening_command(&doc.model, &spec_o).and_then(|(cmds, _)| doc.commit("plan import: opening", Origin::User, cmds).map_err(|e| e.to_string()));
            match outcome {
                Ok(()) if label == "door" => lr.doors_built += 1,
                Ok(()) => lr.windows_built += 1,
                Err(e) => *lr.openings_not_built.entry(reason(&e)).or_default() += 1,
            }
        }
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
        lr.gaps_left_open = r.walls.left_open.len();
        for (n, g) in r.walls.left_open.iter().enumerate() {
            let (a, b, px) = if g.horizontal { ([wx(g.lo_px), wy(g.axis_px)], [wx(g.hi_px), wy(g.axis_px)], [g.lo_px, g.axis_px, g.hi_px, g.axis_px]) } else { ([wx(g.axis_px), wy(g.hi_px)], [wx(g.axis_px), wy(g.lo_px)], [g.axis_px, g.lo_px, g.axis_px, g.hi_px]) };
            lr.open_gaps.push(OpenGap { n: n + 1, a, b, width_mm: (((g.hi_px - g.lo_px) * s) / 10.0).round() as i64 * 10, kind: if g.kind == OpeningKind::Passage { "passage" } else { "unclear" }, px });
        }
        lr.rooms = doc.model.elements.values().filter(|e| e.kind == "Space" && e.level_id == level && !e.props.contains_key("partOf")).count();
        lr.composite_rooms = composite;
        report.levels.push(lr);
    }
    doc.mark_saved();
    report.notes.push("no structure was generated: the model holds what the drawings hold (walls, doors, windows, rooms). Structure comes from a design, not from the architecture drawings".into());
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
    fn a_gap_end_with_no_wall_to_stand_on_is_refused_and_a_crossing_wall_serves_as_an_end() {
        let (doc, level) = box_with_a_gap();
        // From the partition's lower piece up to the top wall: its far end stands on the wall that crosses there.
        let (mut d2, level2) = box_with_a_gap();
        let low = d2.model.elements.values().find(|e| e.kind == "Wall" && e.props.get("y1").and_then(|v| v.as_i64()) == Some(3000) && e.props.get("x1").and_then(|v| v.as_i64()) == Some(3000)).map(|e| e.id).unwrap();
        d2.commit("drop", Origin::User, vec![Command::Delete(low)]).unwrap();
        assert_eq!(snap_gap(&d2.model, level2, [3000, 1100], [3000, 3900]).unwrap(), ([3000, 1000], [3000, 4000]));
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
        }
    }

    fn drawing(name: &str, level: &str, g: Vec<u8>) -> Drawing {
        Drawing { name: name.into(), level: level.into(), width: W as u32, height: H as u32, rgb: g, wall_colours: vec![WALL] }
    }

    fn walls_of(doc: &Document, level: Id) -> Vec<crate::typed::Wall> {
        doc.model.elements.values().filter(|e| e.kind == "Wall" && e.level_id == Some(level)).filter_map(|e| e.typed::<crate::typed::Wall>().ok()).collect()
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
