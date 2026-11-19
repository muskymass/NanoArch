//! Preliminary structural layout derived from the walls: columns, beams, slabs, footings.
//!
//! Geometry comes from the architecture: a column at every wall junction (corners, T and
//! cross junctions), a beam along every wall line between consecutive columns, one slab per
//! room. Extra columns are inserted when a span exceeds `max_span_mm` (if set).
//!
//! Sizing uses only mechanics plus inputs YOU provide. Nothing here is a code value:
//! - Floor loads and soil bearing capacity default to UNSET. Without them no load take-down
//!   or footing size is produced, and a warning says what to enter (from the SBC load
//!   tables and the soil report).
//! - The column size and the span-to-depth ratios are PLACEHOLDERS (rules of thumb) to be
//!   replaced by the SBC requirements.
//!
//! Load path is a tributary take-down: each slab's load is shared among the columns on its perimeter in proportion to
//! the half-spans on either side of each column (so the four corner columns of a plain panel carry a quarter each, and
//! an intermediate column carries the strip between the mid-points to its neighbours). Before, it was split equally among the
//! columns on its perimeter. Wall, beam and column self-weight are NOT included (add them
//! to the dead load). Footing depth, reinforcement, punching shear and everything else
//! needs a licensed structural engineer; this output is for planning and early costing.

use crate::command::Command;
use crate::layout::{Layout, Rect};
use crate::history::{Document, TxError};
use crate::model::{Id, Model, Source};
use crate::typed::{Beam, Column, Door, Footing, Slab, Space, Typed, Wall, Window, element_of};
use crate::walls::{Plan, WindowPlan};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Loads {
    pub dead_kpa: f64,
    pub live_kpa: f64,
    /// Where the values come from (SBC clause, project criteria...).
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct Soil {
    /// Allowable bearing pressure from the soil investigation report.
    pub bearing_kpa: f64,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct StructOpts {
    /// PLACEHOLDER square column size.
    pub column_mm: i64,
    /// PLACEHOLDER beam span-to-depth ratio.
    pub beam_span_depth: f64,
    /// PLACEHOLDER slab short-span-to-thickness ratio.
    pub slab_span_depth: f64,
    /// Longest allowed span between columns; `None` means no extra columns are inserted.
    pub max_span_mm: Option<i64>,
    /// Number of identical floors carried by each column.
    pub floors: u32,
    pub loads: Option<Loads>,
    pub soil: Option<Soil>,
    /// Footings are made only for the lowest level; upper levels pass their load down through columns.
    pub footings: bool,
    /// Service loads (kN) arriving from the level above, as `(x, y, kN)` at column positions. Each is added to
    /// the column at that position; one with no column below it is reported, never dropped silently.
    pub extra_loads: Vec<(i64, i64, f64)>,
    /// Rooms left open above (a stair shaft): they get no slab on this level.
    pub open_rooms: Vec<String>,
    /// Positions where a column stands on the level above. A column must continue down to the foundations, so this
    /// level gets one at each of them unless one is already within half a column width.
    pub forced_columns: Vec<(i64, i64)>,
}

impl Default for StructOpts {
    fn default() -> Self {
        Self { column_mm: 300, beam_span_depth: 12.0, slab_span_depth: 30.0, max_span_mm: None, floors: 1, loads: None, soil: None, footings: true, extra_loads: Vec::new(), open_rooms: Vec::new(), forced_columns: Vec::new() }
    }
}

#[derive(Debug, Clone)]
pub struct ColumnSpec {
    pub id: Id,
    pub x: i64,
    pub y: i64,
    pub grid_ref: String,
    pub load_kn: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct BeamSpec {
    pub a: (i64, i64),
    pub b: (i64, i64),
    pub width: i64,
    pub depth: i64,
    pub length: i64,
}

#[derive(Debug, Clone)]
pub struct SlabSpec {
    pub rect: Rect,
    pub room: String,
    pub thickness: i64,
}

#[derive(Debug, Clone)]
pub struct FootingSpec {
    /// Index into `StructPlan::columns`.
    pub column: usize,
    pub size_mm: i64,
    pub load_kn: f64,
    pub bearing_kpa: f64,
}

#[derive(Debug, Clone, Default)]
pub struct StructPlan {
    pub columns: Vec<ColumnSpec>,
    pub beams: Vec<BeamSpec>,
    pub slabs: Vec<SlabSpec>,
    pub footings: Vec<FootingSpec>,
    pub warnings: Vec<String>,
}

fn round_up(v: f64, step: f64) -> i64 {
    ((v / step).ceil() * step) as i64
}

fn grid_letter(i: usize) -> String {
    if i < 26 { ((b'A' + i as u8) as char).to_string() } else { format!("X{i}") }
}

/// Thickness of a wall whose centre line is `cl` (horizontal if `horiz`) covering `[a, b]`.
fn covering_wall(walls: &[WallIn], horiz: bool, cl: i64, a: i64, b: i64) -> Option<i64> {
    walls.iter().find(|w| w.horizontal == horiz && w.cl == cl && w.start <= a && w.end >= b).map(|w| w.thickness)
}

/// Positions strictly between `a` and `b` where extra columns keep every span within `max`,
/// avoiding the blocked intervals (door and window openings). Empty if none are needed.
fn split_span(a: i64, b: i64, max: i64, blocked: &[(i64, i64)]) -> Vec<i64> {
    let mut pts = vec![a, b];
    for _ in 0..8 {
        let mut next = vec![a];
        let mut changed = false;
        for w in pts.windows(2) {
            let (l, r) = (w[0], w[1]);
            if r - l > max {
                changed = true;
                let n = ((r - l) as f64 / max as f64).ceil() as i64;
                for k in 1..n {
                    let mut p = l + ((r - l) * k / n) / 10 * 10;
                    for _ in 0..4 {
                        if let Some(&(lo, hi)) = blocked.iter().find(|bl| p > bl.0 && p < bl.1) {
                            p = if p - lo < hi - p { lo } else { hi };
                        }
                    }
                    // A candidate still inside an opening (two openings too close for a column) is dropped.
                    let clear_of_openings = !blocked.iter().any(|bl| p > bl.0 && p < bl.1);
                    if clear_of_openings && p > l + 100 && p < r - 100 {
                        next.push(p);
                    }
                }
            }
            next.push(r);
        }
        next.sort_unstable();
        next.dedup();
        pts = next;
        if !changed {
            break;
        }
    }
    pts[1..pts.len() - 1].to_vec()
}

pub fn build(layout: &Layout, plan: &Plan, envelope: Rect, opts: &StructOpts) -> StructPlan {
    build_with_openings(layout, plan, &WindowPlan::default(), envelope, opts)
}

/// Like [`build`], but extra columns also keep clear of the windows in `windows` (doors come from `plan`).
pub fn build_with_openings(layout: &Layout, plan: &Plan, windows: &WindowPlan, envelope: Rect, opts: &StructOpts) -> StructPlan {
    build_input(&Input::from_plan(layout, plan, windows, envelope), opts)
}

/// A wall as the structure sees it: a centre line, an extent along it and a thickness.
#[derive(Debug, Clone, PartialEq)]
pub struct WallIn {
    pub horizontal: bool,
    /// Coordinate of the centre line across the wall.
    pub cl: i64,
    pub start: i64,
    pub end: i64,
    pub thickness: i64,
    pub exterior: bool,
}

/// Everything the structure is derived from. Built either from the generator's plan or
/// (the source of truth) from the model's typed elements.
#[derive(Debug, Clone)]
pub struct Input {
    /// Room key and rectangle.
    pub rooms: Vec<(String, Rect)>,
    pub walls: Vec<WallIn>,
    /// Door and window openings: `(wall index, centre offset from the wall start, width)`.
    pub openings: Vec<(usize, i64, i64)>,
    pub envelope: Rect,
}

impl Input {
    pub fn from_plan(layout: &Layout, plan: &Plan, windows: &WindowPlan, envelope: Rect) -> Input {
        let walls = plan
            .walls
            .iter()
            .map(|w| WallIn {
                horizontal: w.horizontal,
                cl: if w.horizontal { w.a.1 } else { w.a.0 },
                start: w.start,
                end: w.end,
                thickness: w.thickness,
                exterior: w.exterior,
            })
            .collect();
        let mut openings: Vec<(usize, i64, i64)> = plan.doors.iter().map(|d| (d.wall, d.offset, d.width)).collect();
        openings.extend(windows.windows.iter().map(|o| (o.wall, o.offset, o.width)));
        Input { rooms: layout.rooms.iter().map(|r| (r.key.clone(), r.rect)).collect(), walls, openings, envelope }
    }

    /// Reads the typed Space, Wall, Door and Window elements. `Ok(None)` means the model has no
    /// spaces. An element that cannot be read is an `Err` naming it: a damaged element must never
    /// look like "nothing is needed", or a sync would delete the whole structure.
    pub fn from_model(model: &Model, level: Option<Id>) -> Result<Option<Input>, String> {
        let on = |e: &crate::model::Element| level.is_none() || e.level_id == level;
        let bad = |e: &crate::model::Element, err: crate::typed::TypedError| format!("{} {} cannot be read: {err}", e.kind, e.id);
        let mut rooms = Vec::new();
        for e in model.elements.values().filter(|e| e.kind == Space::KIND && on(e)) {
            let s = e.typed::<Space>().map_err(|err| bad(e, err))?;
            rooms.push((s.requirement_key, Rect { x: s.x, y: s.y, w: s.w, h: s.h }));
        }
        if rooms.is_empty() {
            return Ok(None);
        }
        let x0 = rooms.iter().map(|r| r.1.x).min().unwrap_or(0);
        let y0 = rooms.iter().map(|r| r.1.y).min().unwrap_or(0);
        let x1 = rooms.iter().map(|r| r.1.x + r.1.w).max().unwrap_or(0);
        let y1 = rooms.iter().map(|r| r.1.y + r.1.h).max().unwrap_or(0);
        let mut walls = Vec::new();
        let mut index: BTreeMap<Id, usize> = BTreeMap::new();
        for e in model.elements.values().filter(|e| e.kind == Wall::KIND && on(e)) {
            let w = e.typed::<Wall>().map_err(|err| bad(e, err))?;
            index.insert(e.id, walls.len());
            walls.push(WallIn {
                horizontal: w.horizontal(),
                cl: w.centre_line(),
                start: w.start(),
                end: w.start() + w.length,
                thickness: w.thickness,
                exterior: w.is_exterior,
            });
        }
        let mut openings = Vec::new();
        for e in model.elements.values().filter(|e| (e.kind == Door::KIND || e.kind == Window::KIND) && on(e)) {
            // An opening hosted by a wall on another level is simply not on this level.
            let Some(host) = e.host_id.and_then(|h| index.get(&h).copied()) else { continue };
            let (Some(off), Some(width)) = (e.props.get("offsetCenter").and_then(|v| v.as_i64()), e.props.get("width").and_then(|v| v.as_i64())) else {
                return Err(format!("{} {} has no readable offsetCenter and width", e.kind, e.id));
            };
            openings.push((host, off, width));
        }
        Ok(Some(Input { rooms, walls, openings, envelope: Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 } }))
    }
}

/// The structure derived from an [`Input`].
pub fn build_input(input: &Input, opts: &StructOpts) -> StructPlan {
    let envelope = input.envelope;
    let mut sp = StructPlan::default();
    let te = input.walls.iter().find(|w| w.exterior).map_or(0, |w| w.thickness);

    // ---- columns at every wall junction (room-rectangle corners) ----
    let mut vertices: BTreeSet<(i64, i64)> = BTreeSet::new();
    for (_, b) in &input.rooms {
        for p in [(b.x, b.y), (b.x + b.w, b.y), (b.x, b.y + b.h), (b.x + b.w, b.y + b.h)] {
            vertices.insert(p);
        }
    }
    // Corner columns stand where the exterior walls' real centre lines cross, read from the walls
    // themselves (not assumed from their thickness), so moving a wall moves its columns.
    let cl_range = |horiz: bool| {
        let v: Vec<i64> = input.walls.iter().filter(|w| w.exterior && w.horizontal == horiz).map(|w| w.cl).collect();
        (v.iter().copied().min(), v.iter().copied().max())
    };
    let ((y_lo, y_hi), (x_lo, x_hi)) = (cl_range(true), cl_range(false));
    let off = |cl: Option<i64>, edge: i64| cl.map_or(0, |c| c - edge);
    let (ox_lo, ox_hi) = (off(x_lo, envelope.x), off(x_hi, envelope.x + envelope.w));
    let (oy_lo, oy_hi) = (off(y_lo, envelope.y), off(y_hi, envelope.y + envelope.h));
    let inset = |v: i64, lo: i64, hi: i64, olo: i64, ohi: i64| if v == lo { v + olo } else if v == hi { v + ohi } else { v };
    let mut points: BTreeSet<(i64, i64)> = vertices
        .iter()
        .map(|&(vx, vy)| {
            (
                inset(vx, envelope.x, envelope.x + envelope.w, ox_lo, ox_hi),
                inset(vy, envelope.y, envelope.y + envelope.h, oy_lo, oy_hi),
            )
        })
        .collect();

    // ---- insert extra columns where a wall span is too long ----
    if let Some(max) = opts.max_span_mm.filter(|&m| m > 0) {
        let mut extra: BTreeSet<(i64, i64)> = BTreeSet::new();
        for horiz in [true, false] {
            let mut lines: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
            for &(x, y) in &points {
                if horiz { lines.entry(y).or_default().push(x) } else { lines.entry(x).or_default().push(y) }
            }
            for (cl, mut pos) in lines {
                pos.sort_unstable();
                for pair in pos.windows(2) {
                    let (a, b) = (pair[0], pair[1]);
                    if covering_wall(&input.walls, horiz, cl, a, b).is_none() || b - a <= max {
                        continue;
                    }
                    // Openings on this wall line that a column must not stand in.
                    let clear = opts.column_mm / 2 + 50;
                    let mut blocked: Vec<(i64, i64)> = Vec::new();
                    for &(wi, off, width) in &input.openings {
                        let w = &input.walls[wi];
                        if w.horizontal != horiz || w.cl != cl || w.start > a || w.end < b {
                            continue;
                        }
                        blocked.push((w.start + off - width / 2 - clear, w.start + off + width / 2 + clear));
                    }
                    let cuts = split_span(a, b, max, &blocked);
                    let mut edges = vec![a];
                    edges.extend(&cuts);
                    edges.push(b);
                    if edges.windows(2).any(|e| e[1] - e[0] > max) {
                        sp.warnings.push(format!(
                            "a {} mm span at {} cannot be reduced to {max} mm because openings block extra columns",
                            b - a,
                            cl
                        ));
                    }
                    for p in cuts {
                        extra.insert(if horiz { (p, cl) } else { (cl, p) });
                    }
                }
            }
        }
        points.extend(extra);
    } else {
        sp.warnings.push("max span not set: no extra columns were inserted for long spans".into());
    }

    // ---- columns that continue down from the level above ----
    if !opts.forced_columns.is_empty() {
        let half = opts.column_mm / 2;
        let clear = half + 50;
        let on_wall = |x: i64, y: i64| input.walls.iter().any(|w| if w.horizontal { w.cl == y && w.start <= x && x <= w.end } else { w.cl == x && w.start <= y && y <= w.end });
        let in_opening = |x: i64, y: i64| {
            input.openings.iter().any(|&(wi, off, width)| {
                let w = &input.walls[wi];
                let (along, across) = if w.horizontal { (x, y) } else { (y, x) };
                w.cl == across && (along - (w.start + off)).abs() < width / 2 + clear
            })
        };
        let (mut added, mut free, mut doorway) = (0usize, 0usize, 0usize);
        for &(fx, fy) in &opts.forced_columns {
            if points.iter().any(|p| (p.0 - fx).abs() <= half && (p.1 - fy).abs() <= half) {
                continue;
            }
            points.insert((fx, fy));
            added += 1;
            free += usize::from(!on_wall(fx, fy));
            doorway += usize::from(in_opening(fx, fy));
        }
        if added > 0 {
            sp.warnings.push(format!("{added} column(s) continue down from the level above where this level has no junction"));
        }
        if free > 0 {
            sp.warnings.push(format!("{free} of them stand inside a room with no wall and no beam: the slab panel they support needs a structural engineer's check"));
        }
        if doorway > 0 {
            sp.warnings.push(format!("{doorway} of them stand in a door or window opening: move the opening or the room, or have an engineer provide a transfer"));
        }
    }

    // ---- grid references ----
    let xs: Vec<i64> = points.iter().map(|p| p.0).collect::<BTreeSet<_>>().into_iter().collect();
    let ys: Vec<i64> = points.iter().map(|p| p.1).collect::<BTreeSet<_>>().into_iter().collect();
    for &(x, y) in &points {
        let xi = xs.binary_search(&x).unwrap_or(0);
        let yi = ys.binary_search(&y).unwrap_or(0);
        sp.columns.push(ColumnSpec { id: Uuid::now_v7(), x, y, grid_ref: format!("{}{}", grid_letter(xi), yi + 1), load_kn: None });
    }

    // ---- beams along wall lines between consecutive columns ----
    for horiz in [true, false] {
        let mut lines: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
        for &(x, y) in &points {
            if horiz { lines.entry(y).or_default().push(x) } else { lines.entry(x).or_default().push(y) }
        }
        for (cl, mut pos) in lines {
            pos.sort_unstable();
            for pair in pos.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                let Some(width) = covering_wall(&input.walls, horiz, cl, a, b) else { continue };
                let length = b - a;
                sp.beams.push(BeamSpec {
                    a: if horiz { (a, cl) } else { (cl, a) },
                    b: if horiz { (b, cl) } else { (cl, b) },
                    width,
                    depth: round_up(length as f64 / opts.beam_span_depth, 50.0),
                    length,
                });
            }
        }
    }

    // ---- one slab per room ----
    for (key, rect) in input.rooms.iter().filter(|(k, _)| !opts.open_rooms.contains(k)) {
        let short = rect.w.min(rect.h) as f64;
        sp.slabs.push(SlabSpec { rect: *rect, room: key.clone(), thickness: round_up(short / opts.slab_span_depth, 10.0) });
    }

    // ---- load take-down and footings (only with user-supplied loads and soil) ----
    let Some(loads) = &opts.loads else {
        sp.warnings.push("floor loads not set: column loads and footings were not computed (enter dead and live loads from the SBC tables)".into());
        return sp;
    };
    sp.warnings.push("wall, beam and column self-weight are not included in the loads; add them to the dead load".into());
    let tol = te.max(1);
    let per_area = (loads.dead_kpa + loads.live_kpa) * opts.floors as f64;
    let mut load = vec![0.0f64; sp.columns.len()];
    for s in &sp.slabs {
        let r = s.rect;
        let on_edge = |c: &ColumnSpec| {
            let inside_band = c.x >= r.x - tol && c.x <= r.x + r.w + tol && c.y >= r.y - tol && c.y <= r.y + r.h + tol;
            let near = |v: i64, e: i64| (v - e).abs() <= tol;
            inside_band && (near(c.x, r.x) || near(c.x, r.x + r.w) || near(c.y, r.y) || near(c.y, r.y + r.h))
        };
        let carriers: Vec<usize> = sp.columns.iter().enumerate().filter(|(_, c)| on_edge(c)).map(|(i, _)| i).collect();
        if carriers.is_empty() {
            sp.warnings.push(format!("slab of room '{}' has no supporting column", s.room));
            continue;
        }
        let total = r.area_m2() * per_area;
        for (i, share) in tributary_shares(&sp.columns, &carriers, &r, tol) {
            load[i] += total * share;
        }
    }
    for &(x, y, kn) in &opts.extra_loads {
        match sp.columns.iter().position(|c| (c.x - x).abs() <= tol && (c.y - y).abs() <= tol) {
            Some(i) => load[i] += kn,
            None => sp.warnings.push(format!("a column above at ({x}, {y}) has no column below it: its {kn:.1} kN is not carried to the foundations")),
        }
    }
    for (c, l) in sp.columns.iter_mut().zip(&load) {
        c.load_kn = Some(*l);
    }

    if !opts.footings {
        return sp;
    }
    let Some(soil) = &opts.soil else {
        sp.warnings.push("soil bearing capacity not set: footings were not sized (enter it from the soil investigation report)".into());
        return sp;
    };
    for (i, l) in load.iter().enumerate().filter(|(_, l)| **l > 0.0) {
        // Size from the load as stored (0.1 kN), so the stored footing always carries the stored load.
        let l = (l * 10.0).round() / 10.0;
        let area_m2 = l / soil.bearing_kpa;
        sp.footings.push(FootingSpec { column: i, size_mm: round_up(area_m2.sqrt() * 1000.0, 50.0), load_kn: l, bearing_kpa: soil.bearing_kpa });
    }
    sp.warnings.push("footing depth and reinforcement are not designed: a licensed structural engineer must complete them".into());
    sp
}

/// Share (0..1, summing to 1) of a slab's load that each carrier column takes: its half-spans along the slab's perimeter
/// divided by the perimeter. `carriers` are the columns on the slab's edge.
fn tributary_shares(cols: &[ColumnSpec], carriers: &[usize], r: &Rect, tol: i64) -> Vec<(usize, f64)> {
    let (w, h) = (r.w as f64, r.h as f64);
    let near = |v: i64, e: i64| (v - e).abs() <= tol;
    // Position along the perimeter, going east along the lowest-y edge, then north, west and south.
    let pos = |c: &ColumnSpec| -> f64 {
        if near(c.y, r.y) {
            (c.x - r.x) as f64
        } else if near(c.x, r.x + r.w) {
            w + (c.y - r.y) as f64
        } else if near(c.y, r.y + r.h) {
            w + h + (r.x + r.w - c.x) as f64
        } else {
            2.0 * w + h + (r.y + r.h - c.y) as f64
        }
    };
    let mut at: Vec<(usize, f64)> = carriers.iter().map(|&i| (i, pos(&cols[i]).clamp(0.0, 2.0 * (w + h)))).collect();
    at.sort_by(|a, b| a.1.total_cmp(&b.1));
    let perimeter = 2.0 * (w + h);
    let n = at.len();
    if n == 1 {
        return vec![(at[0].0, 1.0)];
    }
    (0..n)
        .map(|k| {
            let prev = at[(k + n - 1) % n].1;
            let next = at[(k + 1) % n].1;
            let gap = |from: f64, to: f64| (to - from).rem_euclid(perimeter);
            let half = (gap(prev, at[k].1) + gap(at[k].1, next)) / 2.0;
            (at[k].0, half / perimeter)
        })
        .collect()
}

/// Tributary floor area of every column in m2, per column element: each slab's area shared by the same half-span rule the
/// load take-down uses, for the slabs of the column's own level. Independent of any load: a column that no slab edge
/// touches gets 0.
pub fn column_tributary(model: &Model) -> BTreeMap<Id, f64> {
    let tol = 200;
    let mut out: BTreeMap<Id, f64> = BTreeMap::new();
    let levels: BTreeSet<Option<Id>> = model.elements.values().filter(|e| e.kind == Column::KIND).map(|e| e.level_id).collect();
    for level in levels {
        let cols: Vec<ColumnSpec> = model
            .elements
            .values()
            .filter(|e| e.kind == Column::KIND && e.level_id == level)
            .filter_map(|e| Some(ColumnSpec { id: e.id, x: e.typed::<Column>().ok()?.x, y: e.typed::<Column>().ok()?.y, grid_ref: String::new(), load_kn: None }))
            .collect();
        for c in &cols {
            out.entry(c.id).or_insert(0.0);
        }
        for s in model.elements.values().filter(|e| e.kind == Slab::KIND && e.level_id == level).filter_map(|e| e.typed::<Slab>().ok()) {
            let r = Rect { x: s.x, y: s.y, w: s.w, h: s.h };
            let near = |v: i64, e: i64| (v - e).abs() <= tol;
            let carriers: Vec<usize> = cols
                .iter()
                .enumerate()
                .filter(|(_, c)| c.x >= r.x - tol && c.x <= r.x + r.w + tol && c.y >= r.y - tol && c.y <= r.y + r.h + tol && (near(c.x, r.x) || near(c.x, r.x + r.w) || near(c.y, r.y) || near(c.y, r.y + r.h)))
                .map(|(i, _)| i)
                .collect();
            if carriers.is_empty() {
                continue;
            }
            for (i, share) in tributary_shares(&cols, &carriers, &r, tol) {
                *out.entry(cols[i].id).or_insert(0.0) += r.area_m2() * share;
            }
        }
    }
    out
}

/// Creation commands: columns first (footings refer to them), then beams, slabs, footings.
pub fn to_commands(sp: &StructPlan, opts: &StructOpts, level: Option<Id>) -> Vec<Command> {
    let mut cmds = Vec::new();
    let mut put = |mut el: crate::model::Element, id: Option<Id>| {
        if let Some(id) = id {
            el.id = id;
        }
        el.level_id = level;
        cmds.push(Command::Create(el));
    };
    for c in &sp.columns {
        let col = Column {
            x: c.x,
            y: c.y,
            b: opts.column_mm,
            h: opts.column_mm,
            grid_ref: c.grid_ref.clone(),
            axial_service_kn: c.load_kn.map(|l| (l * 10.0).round() / 10.0),
            extra: Default::default(),
        };
        put(element_of(&col, Source::Generated), Some(c.id));
    }
    for b in &sp.beams {
        let beam = Beam { x1: b.a.0, y1: b.a.1, x2: b.b.0, y2: b.b.1, b: b.width, h: b.depth, length: b.length, extra: Default::default() };
        put(element_of(&beam, Source::Generated), None);
    }
    for s in &sp.slabs {
        let slab = Slab {
            x: s.rect.x,
            y: s.rect.y,
            w: s.rect.w,
            h: s.rect.h,
            thickness: s.thickness,
            kind: "solid".into(),
            room: s.room.clone(),
            extra: Default::default(),
        };
        put(element_of(&slab, Source::Generated), None);
    }
    for f in &sp.footings {
        let c = &sp.columns[f.column];
        let footing = Footing {
            x: c.x,
            y: c.y,
            length: f.size_mm,
            width: f.size_mm,
            service_load_kn: (f.load_kn * 10.0).round() / 10.0,
            bearing_kpa: f.bearing_kpa,
            column: c.id.to_string(),
            extra: Default::default(),
        };
        put(element_of(&footing, Source::Generated), None);
    }
    cmds
}

// ------------------------------------------------------------------ keeping the structure in sync

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub created: usize,
    pub removed: usize,
    pub unchanged: usize,
    /// Locked generated elements that no longer match the model but were left alone.
    pub kept_locked: usize,
    /// Notes from the structural design (missing inputs, unsupported slabs, unsupported upper columns).
    pub warnings: Vec<String>,
}

const STRUCT_KINDS: [&str; 4] = ["Column", "Beam", "Slab", "Footing"];

fn signature(el: &crate::model::Element) -> String {
    format!("{}|{}", el.kind, serde_json::Value::Object(el.props.clone()))
}

/// Commands that bring the model's generated structure in line with the model's walls and spaces:
/// delete generated elements that no longer match, create the missing ones, leave matching ones
/// (and their ids) untouched. Locked elements are never deleted. Empty when nothing differs.
pub fn sync_commands(model: &Model, opts: &StructOpts, level: Option<Id>) -> Result<(Vec<Command>, SyncReport), String> {
    // Only a model that really has no spaces asks for no structure; an unreadable element is an error.
    let mut notes: Vec<String> = Vec::new();
    let desired: Vec<Command> = match Input::from_model(model, level)? {
        Some(input) => {
            let sp = build_input(&input, opts);
            notes = sp.warnings.clone();
            to_commands(&sp, opts, level)
        }
        None => Vec::new(),
    };
    // Generated structural elements already in the model, by content.
    let mut existing: BTreeMap<String, Vec<Id>> = BTreeMap::new();
    let mut all_existing: Vec<&crate::model::Element> = Vec::new();
    for e in model.elements.values() {
        if STRUCT_KINDS.contains(&e.kind.as_str()) && e.provenance.source == Source::Generated && (level.is_none() || e.level_id == level) {
            existing.entry(signature(e)).or_default().push(e.id);
            all_existing.push(e);
        }
    }
    // Columns carried by a combined footing or a raft: their single footings are not made again.
    let merged_cols: BTreeSet<String> = model.elements.values().filter(|e| e.kind == "Footing").filter_map(|e| e.props.get("columns")?.as_array().cloned()).flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
    let mut report = SyncReport { warnings: notes, ..SyncReport::default() };
    let mut matched: BTreeSet<Id> = BTreeSet::new();
    let mut creates: Vec<Command> = Vec::new();
    // New column id -> the id actually used (reused or new), so footings point at the right column.
    let mut column_id: BTreeMap<String, String> = BTreeMap::new();
    for cmd in desired {
        let Command::Create(mut el) = cmd else { continue };
        let new_id = el.id;
        if el.kind == "Footing"
            && let Some(c) = el.props.get("column").and_then(|v| v.as_str()).and_then(|c| column_id.get(c)) {
                el.props.insert("column".into(), serde_json::json!(c));
            }
        if el.kind == "Footing" && el.props.get("column").and_then(|v| v.as_str()).is_some_and(|c| merged_cols.contains(c)) {
            continue;
        }
        match existing.get_mut(&signature(&el)).and_then(|v| v.pop()) {
            Some(old) => {
                matched.insert(old);
                report.unchanged += 1;
                if el.kind == "Column" {
                    column_id.insert(new_id.to_string(), old.to_string());
                }
            }
            None => {
                report.created += 1;
                if el.kind == "Column" {
                    column_id.insert(new_id.to_string(), new_id.to_string());
                }
                creates.push(Command::Create(el));
            }
        }
    }
    // Deletes: footings first, then beams and slabs, then columns.
    let mut cmds: Vec<Command> = Vec::new();
    for kind in ["Footing", "Beam", "Slab", "Column"] {
        for e in all_existing.iter().filter(|e| e.kind == kind && !matched.contains(&e.id)) {
            if e.provenance.locked {
                report.kept_locked += 1;
            } else {
                report.removed += 1;
                cmds.push(Command::Delete(e.id));
            }
        }
    }
    cmds.extend(creates);
    Ok((cmds, report))
}

/// True when the generated structure no longer matches the model's walls and spaces.
/// `Err` when the model cannot be read (nothing is changed or judged in that case).
pub fn needs_sync(model: &Model, opts: &StructOpts, level: Option<Id>) -> Result<bool, String> {
    Ok(!sync_commands(model, opts, level)?.0.is_empty())
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// The model has an element the structure cannot be derived from. Nothing was changed.
    #[error("the structure was not touched: {0}")]
    Unreadable(String),
    #[error(transparent)]
    Commit(#[from] TxError),
}

/// Regenerates the structure from the model as one undoable step. A no-op when it is already in step,
/// and a refusal (with nothing deleted) when the model has an element that cannot be read.
pub fn sync(doc: &mut Document, opts: &StructOpts, level: Option<Id>) -> Result<SyncReport, SyncError> {
    let (cmds, report) = sync_commands(&doc.model, opts, level).map_err(SyncError::Unreadable)?;
    if !cmds.is_empty() {
        doc.commit("sync structure with the model", crate::command::Origin::Generator, cmds)?;
    }
    Ok(report)
}
