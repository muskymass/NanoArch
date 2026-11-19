//! Column-grid optimizer: moves columns onto wall intersections so that they stand inside the walls instead of sticking out into the
//! rooms, while every span between neighbouring columns stays within the limit YOU enter.
//!
//! - **Projection** of a column is the part of its section that is not covered by a wall (walls are rectangles of their thickness,
//!   running to the far face of a corner). A column buried in a wall has none; one in the middle of a room projects its whole section.
//! - **Candidates** are the wall intersections (and corners) within `snapMm` of the column. The one with the least projection wins,
//!   if it is better than where the column stands now.
//! - **Spans**: after the move no span to the nearest column in the same row or column of the grid may exceed `maxSpanMm`. Without that
//!   limit nothing is moved: the report names it as missing. The limit is a design input, not a code value.
//! - `apply` moves the column, its footing and the ends of the beams that met it, as one undoable step. Tie beams that were already
//!   applied to the model keep their old ends: apply them again.

use crate::building;
use crate::command::Command;
use crate::model::{Id, Model};
use crate::typed::{Beam, Column, Typed, Wall};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GridInputs {
    /// Longest span between neighbouring columns that is acceptable, mm.
    pub max_span_mm: Option<f64>,
    /// How far a column may move to reach a wall intersection, mm; also the tolerance for "same row or column".
    pub snap_mm: Option<f64>,
}

impl GridInputs {
    pub fn check(&self) -> Result<(), String> {
        for (n, v) in [("maximum span", self.max_span_mm), ("snap distance", self.snap_mm)] {
            if v.is_some_and(|x| !x.is_finite() || x <= 0.0) {
                return Err(format!("{n} must be a positive number of millimetres"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Proposal {
    pub column: String,
    pub id: String,
    pub level: Option<i64>,
    pub from: [i64; 2],
    pub to: [i64; 2],
    pub projection_before_m2: f64,
    pub projection_after_m2: f64,
    pub span_before_mm: Option<i64>,
    pub span_after_mm: Option<i64>,
    /// `move`, `kept` (already as good as any intersection) or `blocked`.
    pub status: &'static str,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct GridReport {
    pub proposals: Vec<Proposal>,
    pub intersections: usize,
    pub grid_x: Vec<i64>,
    pub grid_y: Vec<i64>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Clone, Copy)]
struct WallRect {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

fn wall_rects(model: &Model, level: Option<Id>) -> (Vec<WallRect>, Vec<(i64, i64)>) {
    let mut rects = Vec::new();
    // Wall centre lines: (centre line, start, end, thickness).
    type Seg = (i64, i64, i64, i64);
    let (mut hs, mut vs): (Vec<Seg>, Vec<Seg>) = (Vec::new(), Vec::new());
    for e in model.elements.values().filter(|e| e.kind == Wall::KIND && (level.is_none() || e.level_id == level)) {
        let Ok(w) = e.typed::<Wall>() else { continue };
        let t = w.thickness;
        if w.y1 == w.y2 {
            let (a, b) = (w.x1.min(w.x2), w.x1.max(w.x2));
            hs.push((w.y1, a, b, t));
            rects.push(WallRect { x0: a - t / 2, y0: w.y1 - t / 2, x1: b + t / 2, y1: w.y1 + t / 2 });
        } else if w.x1 == w.x2 {
            let (a, b) = (w.y1.min(w.y2), w.y1.max(w.y2));
            vs.push((w.x1, a, b, t));
            rects.push(WallRect { x0: w.x1 - t / 2, y0: a - t / 2, x1: w.x1 + t / 2, y1: b + t / 2 });
        }
    }
    let mut pts = Vec::new();
    for &(y, xa, xb, _) in &hs {
        for &(x, ya, yb, _) in &vs {
            if x >= xa && x <= xb && y >= ya && y <= yb {
                pts.push((x, y));
            }
        }
    }
    pts.sort_unstable();
    pts.dedup();
    (rects, pts)
}

/// Area of the column's section not covered by any wall, m2 (sampled every 10 mm).
fn projection_m2(c: (i64, i64, i64, i64), walls: &[WallRect]) -> f64 {
    let (cx, cy, b, h) = c;
    let (x0, y0) = (cx - b / 2, cy - h / 2);
    let step = 10;
    let (mut free, mut total) = (0u64, 0u64);
    let mut y = y0 + step / 2;
    while y < y0 + h {
        let mut x = x0 + step / 2;
        while x < x0 + b {
            total += 1;
            if !walls.iter().any(|w| x >= w.x0 && x <= w.x1 && y >= w.y0 && y <= w.y1) {
                free += 1;
            }
            x += step;
        }
        y += step;
    }
    if total == 0 { 0.0 } else { free as f64 / total as f64 * (b * h) as f64 / 1e6 }
}

/// Longest span from a column to its nearest neighbour in its row and in its column of the grid.
fn spans(i: usize, pos: &[(i64, i64)], tol: i64) -> Option<i64> {
    let (x, y) = pos[i];
    let mut worst: Option<i64> = None;
    let mut consider = |d: Option<i64>| {
        if let Some(d) = d {
            worst = Some(worst.map_or(d, |w| w.max(d)));
        }
    };
    consider(pos.iter().enumerate().filter(|(j, p)| *j != i && (p.1 - y).abs() <= tol && p.0 > x).map(|(_, p)| p.0 - x).min());
    consider(pos.iter().enumerate().filter(|(j, p)| *j != i && (p.1 - y).abs() <= tol && p.0 < x).map(|(_, p)| x - p.0).min());
    consider(pos.iter().enumerate().filter(|(j, p)| *j != i && (p.0 - x).abs() <= tol && p.1 > y).map(|(_, p)| p.1 - y).min());
    consider(pos.iter().enumerate().filter(|(j, p)| *j != i && (p.0 - x).abs() <= tol && p.1 < y).map(|(_, p)| y - p.1).min());
    worst
}

pub fn analyze(model: &Model, inp: &GridInputs) -> GridReport {
    let mut rep = GridReport::default();
    let tol = inp.snap_mm.map(|s| s.round() as i64);
    let max_span = inp.max_span_mm.map(|s| s.round() as i64);
    if tol.is_none() {
        rep.missing.push("snap distance (mm): how far a column may move to reach a wall intersection".into());
    }
    if max_span.is_none() {
        rep.missing.push("maximum span (mm): the longest span between neighbouring columns you accept".into());
    }
    let levels = building::levels(model);
    let level_ids: Vec<Option<Id>> = if levels.is_empty() { vec![None] } else { levels.iter().map(|(_, id)| Some(*id)).collect() };
    let mut names: BTreeMap<Id, String> = BTreeMap::new();
    let mut all: Vec<(Id, Column, Option<Id>)> = model.elements.values().filter(|e| e.kind == Column::KIND).filter_map(|e| Some((e.id, e.typed::<Column>().ok()?, e.level_id))).collect();
    all.sort_by_key(|(_, c, l)| (levels.iter().position(|x| Some(x.1) == *l), c.y, c.x));
    for (i, (id, _, _)) in all.iter().enumerate() {
        names.insert(*id, format!("COL-{}", i + 1));
    }
    let (mut gx, mut gy) = (Vec::new(), Vec::new());
    for lv in level_ids {
        let (walls, inter) = wall_rects(model, lv);
        rep.intersections += inter.len();
        let cols: Vec<&(Id, Column, Option<Id>)> = all.iter().filter(|(_, _, l)| *l == lv).collect();
        let mut pos: Vec<(i64, i64)> = cols.iter().map(|(_, c, _)| (c.x, c.y)).collect();
        for (i, (id, c, _)) in cols.iter().enumerate() {
            let before = projection_m2((c.x, c.y, c.b, c.h), &walls);
            let sb = tol.and_then(|t| spans(i, &pos, t));
            let mut p = Proposal { column: names[id].clone(), id: id.to_string(), level: levels.iter().position(|x| Some(x.1) == lv).map(|k| levels[k].0), from: [c.x, c.y], to: [c.x, c.y], projection_before_m2: (before * 1e4).round() / 1e4, projection_after_m2: (before * 1e4).round() / 1e4, span_before_mm: sb, span_after_mm: sb, status: "kept", reason: String::new() };
            let (Some(t), Some(ms)) = (tol, max_span) else {
                p.reason = "inputs missing: no column is moved".into();
                rep.proposals.push(p);
                continue;
            };
            if before < 1e-4 {
                p.reason = "already inside the walls".into();
                rep.proposals.push(p);
                continue;
            }
            // Best candidate within the snap distance.
            let mut best: Option<((i64, i64), f64)> = None;
            for &(x, y) in inter.iter().filter(|(x, y)| (x - c.x).abs() <= t && (y - c.y).abs() <= t) {
                let pr = projection_m2((x, y, c.b, c.h), &walls);
                if best.is_none_or(|b| pr < b.1 - 1e-9 || (pr <= b.1 + 1e-9 && (x - c.x).abs() + (y - c.y).abs() < (b.0.0 - c.x).abs() + (b.0.1 - c.y).abs())) {
                    best = Some(((x, y), pr));
                }
            }
            let Some(((nx, ny), after)) = best.filter(|b| b.1 < before - 1e-6) else {
                p.reason = if inter.iter().any(|(x, y)| (x - c.x).abs() <= t && (y - c.y).abs() <= t) { "no intersection in reach projects less".into() } else { "no wall intersection within the snap distance".into() };
                rep.proposals.push(p);
                continue;
            };
            if (nx, ny) == (c.x, c.y) {
                rep.proposals.push(p);
                continue;
            }
            // Another column too close to the target, or a span over the limit after the move.
            if pos.iter().enumerate().any(|(j, q)| j != i && (q.0 - nx).abs() < c.b.max(c.h) && (q.1 - ny).abs() < c.b.max(c.h)) {
                p.status = "blocked";
                p.reason = "another column is already there".into();
                rep.proposals.push(p);
                continue;
            }
            let old = pos[i];
            let before_spans: Vec<Option<i64>> = (0..pos.len()).map(|j| spans(j, &pos, t)).collect();
            pos[i] = (nx, ny);
            let sa = spans(i, &pos, t);
            // Every span must stay within the limit, or at least not get worse where it was already over it.
            let neighbours_ok = (0..pos.len()).all(|j| match (spans(j, &pos, t), before_spans[j]) {
                (Some(s), Some(b)) => s <= ms || s <= b,
                (Some(s), None) => s <= ms,
                _ => true,
            });
            if !neighbours_ok {
                pos[i] = old;
                p.status = "blocked";
                p.reason = format!("a span would exceed the {ms} mm limit");
                p.span_after_mm = sa;
                rep.proposals.push(p);
                continue;
            }
            p.status = "move";
            p.to = [nx, ny];
            p.projection_after_m2 = (after * 1e4).round() / 1e4;
            p.span_after_mm = sa;
            p.reason = format!("projection {:.4} m2 to {:.4} m2", before, after);
            rep.proposals.push(p);
        }
        for q in &pos {
            gx.push(q.0);
            gy.push(q.1);
        }
    }
    gx.sort_unstable();
    gx.dedup();
    gy.sort_unstable();
    gy.dedup();
    rep.grid_x = gx;
    rep.grid_y = gy;
    if rep.proposals.iter().any(|p| p.status == "move") {
        rep.notes.push("tie beams already applied to the model keep their old ends: apply them again after moving columns".into());
    }
    rep
}

/// The commands that carry out the `move` proposals: the column, its footings, and the beam ends that met it.
pub fn apply(model: &Model, rep: &GridReport, tol_mm: i64) -> Vec<Command> {
    let mut cmds = Vec::new();
    for p in rep.proposals.iter().filter(|p| p.status == "move") {
        let Ok(id) = p.id.parse::<Id>() else { continue };
        let set = |id: Id, k: &str, v: i64| Command::SetProp { id, key: k.into(), value: Some(json!(v)) };
        cmds.push(set(id, "x", p.to[0]));
        cmds.push(set(id, "y", p.to[1]));
        for e in model.elements.values().filter(|e| e.kind == "Footing" && e.props.get("column").and_then(|v| v.as_str()) == Some(&p.id)) {
            cmds.push(set(e.id, "x", p.to[0]));
            cmds.push(set(e.id, "y", p.to[1]));
        }
        for e in model.elements.values().filter(|e| e.kind == Beam::KIND && e.level_id == model.get(&id).and_then(|c| c.level_id)) {
            let Ok(b) = e.typed::<Beam>() else { continue };
            let near = |x: i64, y: i64| (x - p.from[0]).abs() <= tol_mm && (y - p.from[1]).abs() <= tol_mm;
            let (mut x1, mut y1, mut x2, mut y2) = (b.x1, b.y1, b.x2, b.y2);
            let mut changed = false;
            if near(b.x1, b.y1) {
                (x1, y1) = (p.to[0], p.to[1]);
                changed = true;
            }
            if near(b.x2, b.y2) {
                (x2, y2) = (p.to[0], p.to[1]);
                changed = true;
            }
            if changed {
                for (k, v) in [("x1", x1), ("y1", y1), ("x2", x2), ("y2", y2), ("length", ((x2 - x1) as f64).hypot((y2 - y1) as f64).round() as i64)] {
                    cmds.push(set(e.id, k, v));
                }
            }
        }
    }
    cmds
}
