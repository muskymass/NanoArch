//! Axis (grid-line) and detailed-dimension plan: the structural axes of one level with their bubbles and dimension chains, and
//! where every column and wall of the level stands against them.
//!
//! The axes are the user's. Either they are entered (`axes_x` and `axes_y`, each with a label and a position in model
//! millimetres) or they are read from the columns of the level (`from_columns` with `cluster_mm`, the distance within which two
//! column centres count as one axis). Giving both, or neither, is refused. Derived axes take the mean of their cluster rounded to
//! the millimetre and are labelled `1, 2, 3...` along x (west to east) and `A, B, C...` along y (south to north); those labels are
//! names, not engineering values.
//!
//! For each column the plan reports the nearest x axis and y axis and the signed offset to each (positive to the east / north).
//! `tolerance_mm` with its `tolerance_source` is the user's design tolerance: with it a column is `on_axis` or `off_axis`, without
//! it the column check is `unchecked` and is never reported as passed. Orthogonal walls report their offset from the nearest axis
//! that runs parallel to them; a wall is not expected to sit on an axis, so walls carry no verdict. Skew walls are skipped and
//! counted in `notes`.
//!
//! The drawing is an SVG with the axes as dash-dot lines, numbered bubbles, the bay-by-bay dimension chain with the overall
//! dimension along each side, the walls and the columns (off-axis columns in red). No scale bar is drawn. The result is a
//! preliminary set-out drawing from the user's own entries, not a surveyor's set-out and not a code check.

use crate::model::{Id, Model};
use crate::permitpack::esc;
use crate::typed::{Column, Level, Typed, Wall};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

/// The most axes accepted in one direction.
pub const MAX_AXES: usize = 200;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AxisInput {
    pub label: String,
    /// Position in model millimetres: an x value for `axes_x`, a y value for `axes_y`.
    pub pos_mm: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Input {
    /// The level by name; required when the model has more than one level.
    pub level: String,
    pub title: String,
    pub axes_x: Vec<AxisInput>,
    pub axes_y: Vec<AxisInput>,
    /// Read the axes from the column centres of the level instead of entering them.
    pub from_columns: bool,
    /// With `from_columns`: column centres closer than this along an axis direction share one axis, mm.
    pub cluster_mm: Option<f64>,
    /// How far a column centre may stand from its axis and still be `on_axis`, mm. Needs `tolerance_source`.
    pub tolerance_mm: Option<f64>,
    pub tolerance_source: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AxisRow {
    pub label: String,
    pub pos_mm: f64,
    /// Distance to the previous axis (west or south of it); `None` for the first.
    pub spacing_mm: Option<f64>,
    /// `true` when read from the columns, `false` when the user entered it.
    pub derived: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ColumnRow {
    pub column: String,
    pub x_mm: f64,
    pub y_mm: f64,
    pub x_axis: String,
    pub x_offset_mm: f64,
    pub y_axis: String,
    pub y_offset_mm: f64,
    /// `on_axis`, `off_axis` or `unchecked` (no tolerance entered).
    pub status: &'static str,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WallRow {
    pub id: String,
    /// `x` for a wall running north-south (it is measured against the x axes), `y` for one running east-west.
    pub measured_against: &'static str,
    pub axis: String,
    /// Signed offset of the wall centre line from that axis, mm (positive to the east / north).
    pub offset_mm: f64,
    pub thickness_mm: f64,
    pub length_mm: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CheckRow {
    pub check: &'static str,
    /// `passes`, `fails` or `unchecked`.
    pub status: &'static str,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub level: String,
    pub x_axes: Vec<AxisRow>,
    pub y_axes: Vec<AxisRow>,
    /// First axis to last axis along x and along y, mm; `None` with a single axis.
    pub overall_x_mm: Option<f64>,
    pub overall_y_mm: Option<f64>,
    pub columns: Vec<ColumnRow>,
    pub walls: Vec<WallRow>,
    pub checks: Vec<CheckRow>,
    /// `fails` when any check fails, else `passes` when every check ran, else `incomplete`.
    pub overall: &'static str,
    pub svg: String,
    pub notes: Vec<String>,
}

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn alpha(mut i: usize) -> String {
    let mut s = Vec::new();
    loop {
        s.push((b'A' + (i % 26) as u8) as char);
        if i < 26 {
            break;
        }
        i = i / 26 - 1;
    }
    s.iter().rev().collect()
}

/// One axis per cluster of values lying within `cluster` of the cluster's first value.
fn cluster_axes(mut values: Vec<f64>, cluster: f64) -> Vec<f64> {
    values.sort_by(f64::total_cmp);
    let mut out: Vec<f64> = Vec::new();
    let mut group: Vec<f64> = Vec::new();
    let flush = |group: &mut Vec<f64>, out: &mut Vec<f64>| {
        if !group.is_empty() {
            out.push((group.iter().sum::<f64>() / group.len() as f64).round());
            group.clear();
        }
    };
    for v in values {
        if group.first().is_some_and(|f| v - f > cluster) {
            flush(&mut group, &mut out);
        }
        group.push(v);
    }
    flush(&mut group, &mut out);
    out
}

fn rows_of(axes: Vec<(String, f64)>, derived: bool) -> Vec<AxisRow> {
    let mut axes = axes;
    axes.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut prev: Option<f64> = None;
    axes.into_iter()
        .map(|(label, pos)| {
            let row = AxisRow { label, pos_mm: pos, spacing_mm: prev.map(|p| r1(pos - p)), derived };
            prev = Some(pos);
            row
        })
        .collect()
}

/// The axis nearest to `v` and the signed offset `v - axis`.
fn nearest(axes: &[AxisRow], v: f64) -> Option<(&AxisRow, f64)> {
    axes.iter().map(|a| (a, v - a.pos_mm)).min_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
}

fn check_axes(name: &str, axes: &[AxisInput], errs: &mut Vec<String>) {
    if axes.len() > MAX_AXES {
        errs.push(format!("at most {MAX_AXES} axes are accepted in {name}"));
    }
    let mut labels: HashSet<&str> = HashSet::new();
    let mut positions: Vec<f64> = Vec::new();
    for (i, a) in axes.iter().enumerate() {
        let l = a.label.trim();
        if l.is_empty() {
            errs.push(format!("{name}[{i}].label"));
        } else if !labels.insert(l) {
            errs.push(format!("{name}[{i}].label `{l}` is duplicated"));
        }
        if !a.pos_mm.is_finite() {
            errs.push(format!("{name}[{i}].pos_mm must be a number"));
        } else if positions.contains(&a.pos_mm) {
            errs.push(format!("{name}[{i}].pos_mm {} repeats another axis", a.pos_mm));
        } else {
            positions.push(a.pos_mm);
        }
    }
}

/// Builds the plan. Refuses, naming each problem, when an input is missing or malformed.
pub fn build(model: &Model, input: &Input) -> Result<Plan, String> {
    let mut errs: Vec<String> = Vec::new();
    let entered = !input.axes_x.is_empty() || !input.axes_y.is_empty();
    if input.from_columns && entered {
        errs.push("from_columns cannot be combined with axes_x / axes_y: enter the axes or read them from the columns, not both".into());
    }
    if !input.from_columns && !entered {
        errs.push("axes_x and axes_y (each axis with label and pos_mm), or from_columns with cluster_mm".into());
    }
    if !input.from_columns && entered && (input.axes_x.is_empty() || input.axes_y.is_empty()) {
        errs.push("axes_x and axes_y must both be given".into());
    }
    if input.from_columns {
        match input.cluster_mm {
            None => errs.push("cluster_mm (the distance within which column centres share one axis)".into()),
            Some(c) if !(c.is_finite() && c >= 0.0) => errs.push(format!("cluster_mm must be zero or more, got {c}")),
            _ => {}
        }
    } else if input.cluster_mm.is_some() {
        errs.push("cluster_mm only applies with from_columns".into());
    }
    check_axes("axes_x", &input.axes_x, &mut errs);
    check_axes("axes_y", &input.axes_y, &mut errs);
    match (input.tolerance_mm, input.tolerance_source.trim().is_empty()) {
        (Some(t), _) if !(t.is_finite() && t >= 0.0) => errs.push(format!("tolerance_mm must be zero or more, got {t}")),
        (Some(_), true) => errs.push("tolerance_source (the specification or office standard of tolerance_mm)".into()),
        (None, false) => errs.push("tolerance_mm (a tolerance_source was given without a tolerance)".into()),
        _ => {}
    }

    let level_names: BTreeMap<Id, String> = model.elements.values().filter(|e| e.kind == Level::KIND).filter_map(|e| Some((e.id, e.typed::<Level>().ok()?.name))).collect();
    let wanted = input.level.trim();
    let (level_id, level_name): (Option<Id>, String) = if wanted.is_empty() {
        match level_names.len() {
            0 => (None, String::new()),
            1 => level_names.iter().next().map(|(id, n)| (Some(*id), n.clone())).unwrap_or_default(),
            _ => {
                errs.push(format!("level (the model has {} levels: {})", level_names.len(), level_names.values().cloned().collect::<Vec<_>>().join(", ")));
                (None, String::new())
            }
        }
    } else {
        match level_names.iter().find(|(_, n)| n.as_str() == wanted) {
            Some((id, n)) => (Some(*id), n.clone()),
            None => {
                errs.push(format!("level `{wanted}` is not a level of the model"));
                (None, String::new())
            }
        }
    };
    if !errs.is_empty() {
        return Err(format!("axes plan: {}. Nothing is defaulted and no axis is built in; enter them.", errs.join("; ")));
    }

    let mut cols: Vec<Column> = model.elements.values().filter(|e| e.kind == Column::KIND && e.level_id == level_id).filter_map(|e| e.typed::<Column>().ok()).collect();
    cols.sort_by_key(|c| (c.y, c.x));
    let walls: Vec<(Id, Wall)> = model.elements.values().filter(|e| e.kind == Wall::KIND && e.level_id == level_id).filter_map(|e| Some((e.id, e.typed::<Wall>().ok()?))).collect();

    let (x_axes, y_axes) = if input.from_columns {
        if cols.is_empty() {
            return Err(format!("axes plan: level `{level_name}` has no columns to read the axes from; enter axes_x and axes_y"));
        }
        let cl = input.cluster_mm.unwrap_or_default();
        let xs = cluster_axes(cols.iter().map(|c| c.x as f64).collect(), cl);
        let ys = cluster_axes(cols.iter().map(|c| c.y as f64).collect(), cl);
        (rows_of(xs.into_iter().enumerate().map(|(i, p)| ((i + 1).to_string(), p)).collect(), true), rows_of(ys.into_iter().enumerate().map(|(i, p)| (alpha(i), p)).collect(), true))
    } else {
        (rows_of(input.axes_x.iter().map(|a| (a.label.trim().to_string(), a.pos_mm)).collect(), false), rows_of(input.axes_y.iter().map(|a| (a.label.trim().to_string(), a.pos_mm)).collect(), false))
    };

    let mut col_rows: Vec<ColumnRow> = Vec::new();
    for (i, c) in cols.iter().enumerate() {
        let (cx, cy) = (c.x as f64, c.y as f64);
        let (Some((ax, dx)), Some((ay, dy))) = (nearest(&x_axes, cx), nearest(&y_axes, cy)) else { continue };
        let status = match input.tolerance_mm {
            None => "unchecked",
            Some(t) if dx.abs() <= t && dy.abs() <= t => "on_axis",
            Some(_) => "off_axis",
        };
        let name = if c.grid_ref.trim().is_empty() { format!("COL-{}", i + 1) } else { c.grid_ref.trim().to_string() };
        col_rows.push(ColumnRow { column: name, x_mm: cx, y_mm: cy, x_axis: ax.label.clone(), x_offset_mm: r1(dx), y_axis: ay.label.clone(), y_offset_mm: r1(dy), status });
    }

    let mut wall_rows: Vec<WallRow> = Vec::new();
    let mut skew = 0usize;
    for (id, w) in &walls {
        let (against, axes, v) = if w.y1 == w.y2 {
            ("y", &y_axes, w.y1 as f64)
        } else if w.x1 == w.x2 {
            ("x", &x_axes, w.x1 as f64)
        } else {
            skew += 1;
            continue;
        };
        let Some((a, d)) = nearest(axes, v) else { continue };
        wall_rows.push(WallRow { id: id.to_string(), measured_against: against, axis: a.label.clone(), offset_mm: r1(d), thickness_mm: w.thickness as f64, length_mm: w.length as f64 });
    }
    wall_rows.sort_by(|a, b| a.measured_against.cmp(b.measured_against).then_with(|| a.axis.cmp(&b.axis)).then_with(|| a.offset_mm.total_cmp(&b.offset_mm)));

    let off = col_rows.iter().filter(|r| r.status == "off_axis").count();
    let mut checks: Vec<CheckRow> = Vec::new();
    match (input.tolerance_mm, col_rows.is_empty()) {
        (None, _) => checks.push(CheckRow { check: "columns_on_axes", status: "unchecked", reason: "no tolerance_mm with its tolerance_source was entered".into() }),
        (Some(_), true) => checks.push(CheckRow { check: "columns_on_axes", status: "unchecked", reason: "the level has no columns".into() }),
        (Some(t), false) if off > 0 => checks.push(CheckRow { check: "columns_on_axes", status: "fails", reason: format!("{off} of {} column(s) stand more than {t} mm from an axis", col_rows.len()) }),
        (Some(t), false) => checks.push(CheckRow { check: "columns_on_axes", status: "passes", reason: format!("all {} column(s) are within {t} mm of their axes ({})", col_rows.len(), input.tolerance_source.trim()) }),
    }
    let overall = if checks.iter().any(|c| c.status == "fails") {
        "fails"
    } else if checks.iter().any(|c| c.status == "unchecked") {
        "incomplete"
    } else {
        "passes"
    };

    let span = |a: &[AxisRow]| if a.len() > 1 { Some(r1(a[a.len() - 1].pos_mm - a[0].pos_mm)) } else { None };
    let title = if input.title.trim().is_empty() { if level_name.is_empty() { "Axes and dimensions".to_string() } else { format!("{level_name} - axes and dimensions") } } else { input.title.trim().to_string() };
    let svg = draw(&title, &x_axes, &y_axes, &cols, &walls, &col_rows);
    let mut notes = vec!["Axis positions, labels, the tolerance and its source are the user's entries (or the column centres when read from the columns); the application holds no axis, tolerance or code figure. Offsets are signed, positive to the east / north, measured from the wall centre line or the column centre. Walls carry offsets only: a wall is not expected to sit on an axis. No scale bar is drawn. Preliminary set-out drawing, not a surveyor's set-out and not a code check / الأرقام من إدخال المستخدم؛ الجدران تُقاس إزاحتها فقط؛ لا مقياس رسم؛ رسم أولي لا مسح ميداني ولا فحص كود.".to_string()];
    if input.from_columns {
        notes.push("Axes were read from the column centres (mean of each cluster, rounded to the millimetre); x axes are numbered west to east and y axes lettered south to north / المحاور مأخوذة من مراكز الأعمدة.".into());
    }
    if skew > 0 {
        notes.push(format!("{skew} skew wall(s) were not measured against the axes / {skew} جدار مائل لم يُقَس."));
    }
    if overall == "incomplete" {
        notes.push("Not checked (never reported as passed): columns_on_axes / فحص لم يُجرَ.".into());
    }
    Ok(Plan { level: level_name, overall_x_mm: span(&x_axes), overall_y_mm: span(&y_axes), x_axes, y_axes, columns: col_rows, walls: wall_rows, checks, overall, svg, notes })
}

fn draw(title: &str, xa: &[AxisRow], ya: &[AxisRow], cols: &[Column], walls: &[(Id, Wall)], rows: &[ColumnRow]) -> String {
    const VIEW: f64 = 760.0;
    const PAD: f64 = 100.0;
    const TOP: f64 = 30.0;
    // Extent of everything drawn, in model millimetres.
    let mut ext = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    let mut grow = |x: f64, y: f64| {
        ext = [ext[0].min(x), ext[1].min(y), ext[2].max(x), ext[3].max(y)];
    };
    // Axes are never empty here (both directions are required or derived from at least one column).
    for a in xa {
        grow(a.pos_mm, ya.first().map_or(0.0, |y| y.pos_mm));
    }
    for a in ya {
        grow(xa.first().map_or(0.0, |x| x.pos_mm), a.pos_mm);
    }
    for c in cols {
        grow((c.x - c.b / 2) as f64, (c.y - c.h / 2) as f64);
        grow((c.x + c.b / 2) as f64, (c.y + c.h / 2) as f64);
    }
    for (_, w) in walls {
        let t = w.thickness as f64 / 2.0;
        grow(w.x1.min(w.x2) as f64 - t, w.y1.min(w.y2) as f64 - t);
        grow(w.x1.max(w.x2) as f64 + t, w.y1.max(w.y2) as f64 + t);
    }
    let (x0, y0, x1, y1) = (ext[0], ext[1], ext[2], ext[3]);
    let s = VIEW / (x1 - x0).max(y1 - y0).max(1.0);
    let (pw, ph) = ((x1 - x0) * s, (y1 - y0) * s);
    let (tw, th) = (pw + 2.0 * PAD, ph + 2.0 * PAD + TOP);
    let px = |x: f64| PAD + (x - x0) * s;
    let py = |y: f64| TOP + PAD + (y1 - y) * s;
    let mut o = String::new();
    let _ = write!(o, "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {tw:.1} {th:.1}\" width=\"{tw:.0}\" height=\"{th:.0}\" font-family=\"sans-serif\" font-size=\"12\">");
    let _ = write!(o, "<rect width=\"100%\" height=\"100%\" fill=\"#fff\"/><text x=\"{:.1}\" y=\"22\" font-size=\"15\" font-weight=\"bold\" text-anchor=\"middle\">{}</text>", tw / 2.0, esc(title));
    for (_, w) in walls {
        let _ = write!(o, "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"#888\" stroke-width=\"{:.1}\"/>", px(w.x1 as f64), py(w.y1 as f64), px(w.x2 as f64), py(w.y2 as f64), (w.thickness as f64 * s).max(1.0));
    }
    let bad: HashSet<String> = rows.iter().filter(|r| r.status == "off_axis").map(|r| r.column.clone()).collect();
    for (i, c) in cols.iter().enumerate() {
        let name = if c.grid_ref.trim().is_empty() { format!("COL-{}", i + 1) } else { c.grid_ref.trim().to_string() };
        let (stroke, fill) = if bad.contains(&name) { ("#b00", "#fdd") } else { ("#222", "#555") };
        let _ = write!(o, "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"1\"/>", px((c.x - c.b / 2) as f64), py((c.y + c.h / 2) as f64), (c.b as f64 * s).max(2.0), (c.h as f64 * s).max(2.0));
    }
    let (top, bottom, left, right) = (py(y1) - 40.0, py(y0) + 40.0, px(x0) - 40.0, px(x1) + 40.0);
    for a in xa {
        let _ = write!(o, "<line x1=\"{x:.1}\" y1=\"{top:.1}\" x2=\"{x:.1}\" y2=\"{bottom:.1}\" stroke=\"#06a\" stroke-width=\"0.8\" stroke-dasharray=\"10 3 2 3\"/><circle cx=\"{x:.1}\" cy=\"{:.1}\" r=\"12\" fill=\"#fff\" stroke=\"#06a\"/><text x=\"{x:.1}\" y=\"{:.1}\" text-anchor=\"middle\" fill=\"#06a\">{}</text>", top - 12.0, top - 8.0, esc(&a.label), x = px(a.pos_mm));
    }
    for a in ya {
        let _ = write!(o, "<line x1=\"{left:.1}\" y1=\"{y:.1}\" x2=\"{right:.1}\" y2=\"{y:.1}\" stroke=\"#06a\" stroke-width=\"0.8\" stroke-dasharray=\"10 3 2 3\"/><circle cx=\"{:.1}\" cy=\"{y:.1}\" r=\"12\" fill=\"#fff\" stroke=\"#06a\"/><text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" fill=\"#06a\">{}</text>", right + 12.0, right + 12.0, py(a.pos_mm) + 4.0, esc(&a.label), y = py(a.pos_mm));
    }
    // Dimension chains: bay by bay under the plan with the overall one below that; the same on the west side.
    let tick = |o: &mut String, x: f64, y: f64| {
        let _ = write!(o, "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"#a60\" stroke-width=\"1\"/>", x - 4.0, y + 4.0, x + 4.0, y - 4.0);
    };
    let dim_y = bottom + 16.0;
    for w in xa.windows(2) {
        let (a, b) = (px(w[0].pos_mm), px(w[1].pos_mm));
        let _ = write!(o, "<line x1=\"{a:.1}\" y1=\"{dim_y:.1}\" x2=\"{b:.1}\" y2=\"{dim_y:.1}\" stroke=\"#a60\"/><text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-size=\"10\" fill=\"#a60\">{}</text>", (a + b) / 2.0, dim_y - 3.0, r1(w[1].pos_mm - w[0].pos_mm));
        tick(&mut o, a, dim_y);
        tick(&mut o, b, dim_y);
    }
    if let (Some(f), Some(l), true) = (xa.first(), xa.last(), xa.len() > 1) {
        let (a, b) = (px(f.pos_mm), px(l.pos_mm));
        let y = dim_y + 22.0;
        let _ = write!(o, "<line x1=\"{a:.1}\" y1=\"{y:.1}\" x2=\"{b:.1}\" y2=\"{y:.1}\" stroke=\"#a60\"/><text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-size=\"11\" fill=\"#a60\">{}</text>", (a + b) / 2.0, y + 13.0, r1(l.pos_mm - f.pos_mm));
        tick(&mut o, a, y);
        tick(&mut o, b, y);
    }
    let dim_x = left - 16.0;
    for w in ya.windows(2) {
        let (a, b) = (py(w[0].pos_mm), py(w[1].pos_mm));
        let _ = write!(o, "<line x1=\"{dim_x:.1}\" y1=\"{a:.1}\" x2=\"{dim_x:.1}\" y2=\"{b:.1}\" stroke=\"#a60\"/><text transform=\"translate({:.1} {:.1}) rotate(-90)\" text-anchor=\"middle\" font-size=\"10\" fill=\"#a60\">{}</text>", dim_x - 3.0, (a + b) / 2.0, r1(w[1].pos_mm - w[0].pos_mm));
        tick(&mut o, dim_x, a);
        tick(&mut o, dim_x, b);
    }
    if let (Some(f), Some(l), true) = (ya.first(), ya.last(), ya.len() > 1) {
        let (a, b) = (py(f.pos_mm), py(l.pos_mm));
        let x = dim_x - 22.0;
        let _ = write!(o, "<line x1=\"{x:.1}\" y1=\"{a:.1}\" x2=\"{x:.1}\" y2=\"{b:.1}\" stroke=\"#a60\"/><text transform=\"translate({:.1} {:.1}) rotate(-90)\" text-anchor=\"middle\" font-size=\"11\" fill=\"#a60\">{}</text>", x - 3.0, (a + b) / 2.0, r1(l.pos_mm - f.pos_mm));
        tick(&mut o, x, a);
        tick(&mut o, x, b);
    }
    o.push_str("</svg>");
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Command, Origin};
    use crate::model::Source;
    use crate::typed::element_of;
    use crate::Document;

    fn col(x: i64, y: i64, gref: &str) -> Column {
        Column { x, y, b: 300, h: 300, grid_ref: gref.into(), axial_service_kn: None, extra: Default::default() }
    }

    fn wall(x1: i64, y1: i64, x2: i64, y2: i64) -> Wall {
        Wall { x1, y1, x2, y2, thickness: 200, height: 3000, length: (x2 - x1).abs() + (y2 - y1).abs(), is_exterior: true, room_a: "r".into(), room_b: None, extra: Default::default() }
    }

    fn doc() -> Document {
        let mut d = Document::default();
        let lv = element_of(&Level { name: "Ground".into(), index: 0, extra: Default::default() }, Source::User);
        let lid = lv.id;
        let mut cmds = vec![Command::Create(lv)];
        for c in [col(0, 0, "C1"), col(5000, 0, ""), col(30, 4000, ""), col(5040, 4000, "")] {
            let mut e = element_of(&c, Source::User);
            e.level_id = Some(lid);
            cmds.push(Command::Create(e));
        }
        for w in [wall(0, 0, 5000, 0), wall(5000, 0, 5000, 4000), wall(0, 2000, 3000, 2000)] {
            let mut e = element_of(&w, Source::User);
            e.level_id = Some(lid);
            cmds.push(Command::Create(e));
        }
        d.commit("frame", Origin::User, cmds).unwrap();
        d
    }

    fn axis(l: &str, p: f64) -> AxisInput {
        AxisInput { label: l.into(), pos_mm: p }
    }

    fn entered() -> Input {
        Input { axes_x: vec![axis("1", 0.0), axis("2", 5000.0)], axes_y: vec![axis("A", 0.0), axis("B", 4000.0)], tolerance_mm: Some(25.0), tolerance_source: "Office set-out note 3, 2026-01-05".into(), ..Default::default() }
    }

    #[test]
    fn entered_axes_give_spacings_overall_offsets_and_a_failing_column() {
        let p = build(&doc().model, &entered()).unwrap();
        assert_eq!((p.level.as_str(), p.overall_x_mm, p.overall_y_mm), ("Ground", Some(5000.0), Some(4000.0)));
        assert_eq!(p.x_axes[1].spacing_mm, Some(5000.0));
        assert!(p.x_axes[0].spacing_mm.is_none() && !p.x_axes[0].derived);
        assert_eq!(p.columns.len(), 4);
        let off: Vec<&ColumnRow> = p.columns.iter().filter(|c| c.status == "off_axis").collect();
        assert_eq!(off.len(), 2, "{:?}", p.columns);
        assert_eq!((off[0].x_offset_mm, off[0].y_offset_mm, off[0].x_axis.as_str(), off[0].y_axis.as_str()), (30.0, 0.0, "1", "B"));
        assert_eq!((p.overall, p.checks[0].status), ("fails", "fails"));
        assert!(p.columns.iter().any(|c| c.column == "C1") && p.columns.iter().any(|c| c.column.starts_with("COL-")));
        assert!(p.svg.starts_with("<svg") && p.svg.ends_with("</svg>") && p.svg.contains("#b00") && p.svg.contains(">5000<"));
    }

    #[test]
    fn walls_report_signed_offsets_from_the_parallel_axes() {
        let p = build(&doc().model, &entered()).unwrap();
        assert_eq!(p.walls.len(), 3);
        let mid = p.walls.iter().find(|w| w.measured_against == "y" && w.offset_mm != 0.0).unwrap();
        // y = 2000 is 2000 from both A (0) and B (4000); the first nearest axis wins and the offset is signed.
        assert_eq!((mid.axis.as_str(), mid.offset_mm.abs()), ("A", 2000.0));
        let east = p.walls.iter().find(|w| w.measured_against == "x").unwrap();
        assert_eq!((east.axis.as_str(), east.offset_mm), ("2", 0.0));
    }

    #[test]
    fn axes_read_from_the_columns_are_clustered_and_labelled() {
        let i = Input { from_columns: true, cluster_mm: Some(100.0), ..Default::default() };
        let p = build(&doc().model, &i).unwrap();
        assert_eq!(p.x_axes.iter().map(|a| (a.label.as_str(), a.pos_mm)).collect::<Vec<_>>(), [("1", 15.0), ("2", 5020.0)]);
        assert_eq!(p.y_axes.iter().map(|a| a.label.as_str()).collect::<Vec<_>>(), ["A", "B"]);
        assert!(p.x_axes.iter().all(|a| a.derived));
        assert_eq!(p.overall, "incomplete");
        assert_eq!(p.checks[0].status, "unchecked");
        assert!(p.columns.iter().all(|c| c.status == "unchecked"));
        let tight = Input { from_columns: true, cluster_mm: Some(10.0), ..Default::default() };
        assert_eq!(build(&doc().model, &tight).unwrap().x_axes.len(), 4);
    }

    #[test]
    fn a_passing_plan_needs_every_column_within_the_tolerance() {
        let mut i = entered();
        i.tolerance_mm = Some(50.0);
        let p = build(&doc().model, &i).unwrap();
        assert_eq!((p.overall, p.checks[0].status), ("passes", "passes"));
    }

    #[test]
    fn missing_or_malformed_inputs_are_named() {
        let e = build(&doc().model, &Input::default()).unwrap_err();
        assert!(e.contains("axes_x and axes_y") && e.contains("from_columns"), "{e}");
        let e = build(&doc().model, &Input { from_columns: true, ..Default::default() }).unwrap_err();
        assert!(e.contains("cluster_mm"), "{e}");
        let mut i = entered();
        i.from_columns = true;
        i.cluster_mm = Some(10.0);
        assert!(build(&doc().model, &i).unwrap_err().contains("cannot be combined"));
        let mut i = entered();
        i.axes_x = vec![axis("", 0.0), axis("2", 0.0), axis("2", f64::NAN)];
        i.tolerance_source.clear();
        let e = build(&doc().model, &i).unwrap_err();
        assert!(e.contains("axes_x[0].label") && e.contains("repeats another axis") && e.contains("duplicated") && e.contains("must be a number") && e.contains("tolerance_source"), "{e}");
        let mut i = entered();
        i.axes_y.clear();
        assert!(build(&doc().model, &i).unwrap_err().contains("must both be given"));
        let mut i = entered();
        i.tolerance_mm = None;
        assert!(build(&doc().model, &i).unwrap_err().contains("without a tolerance"));
        let mut i = entered();
        i.level = "Roof".into();
        assert!(build(&doc().model, &i).unwrap_err().contains("not a level"));
        // No columns on an empty model to read axes from.
        let empty = Input { from_columns: true, cluster_mm: Some(10.0), ..Default::default() };
        assert!(build(&Document::default().model, &empty).unwrap_err().contains("no columns"));
    }

    #[test]
    fn a_model_with_two_levels_needs_the_level_named() {
        let mut d = doc();
        let lv = element_of(&Level { name: "First".into(), index: 1, extra: Default::default() }, Source::User);
        d.commit("level", Origin::User, vec![Command::Create(lv)]).unwrap();
        assert!(build(&d.model, &entered()).unwrap_err().contains("level (the model has 2 levels"));
        let mut i = entered();
        i.level = "First".into();
        let p = build(&d.model, &i).unwrap();
        assert!(p.columns.is_empty() && p.checks[0].status == "unchecked");
    }

    #[test]
    fn labels_run_past_z_and_text_is_escaped() {
        assert_eq!((alpha(0).as_str(), alpha(25).as_str(), alpha(26).as_str(), alpha(27).as_str()), ("A", "Z", "AA", "AB"));
        let mut i = entered();
        i.title = "<script>x</script>".into();
        i.axes_x[0].label = "<b>".into();
        let p = build(&doc().model, &i).unwrap();
        assert!(!p.svg.contains("<script>") && !p.svg.contains("<b>") && p.svg.contains("&lt;script&gt;"));
    }
}
