//! As-built deviation check: measured coordinates or dimensions compared with the design model, in millimetres.
//!
//! - **Input**: rows of a CSV (or stored in the project, `meta.asBuilt`): a name and any of `x_mm, y_mm, length_mm, width_mm,
//!   height_mm, thickness_mm`. The name is a schedule name (`COL-3`, `FND-1`, `BM-2`, `SL-1`), an element id, or `room:<key>`.
//! - **Design values** are those of the element: a column or footing's centre (`x`, `y`), a slab's or room's south-west corner, a
//!   beam's `length`, `width` (its b) and `height` (its h), a wall's `length`, `thickness`, `height`. A quantity an element does not
//!   have is reported, never skipped silently.
//! - **Deviation** = measured - design, signed, in mm. The tolerance is YOURS (`toleranceMm`); without it every deviation is reported
//!   as `no tolerance` and none is judged. Rows that match no element are listed.
//! - Point deviations (`x`, `y`) are drawn on the 2D plan as markers (green within the tolerance, red beyond it, grey unjudged) and the
//!   3D viewer colours the elements the same way.

use crate::drawing::{self, DrawOpts};
use crate::model::{Id, Model};
use crate::structx::schedule_names;
use crate::typed::{Beam, Column, Footing, Slab, Space, Wall};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Measure {
    pub name: String,
    pub x_mm: Option<f64>,
    pub y_mm: Option<f64>,
    pub length_mm: Option<f64>,
    pub width_mm: Option<f64>,
    pub height_mm: Option<f64>,
    pub thickness_mm: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AsBuilt {
    pub tolerance_mm: Option<f64>,
    pub source: Option<String>,
    pub rows: Vec<Measure>,
}

impl AsBuilt {
    pub fn check(&self) -> Result<(), String> {
        if self.tolerance_mm.is_some_and(|t| !t.is_finite() || t < 0.0) {
            return Err("the as-built tolerance must be zero or more".into());
        }
        for r in &self.rows {
            if r.name.trim().is_empty() {
                return Err("an as-built row has no name".into());
            }
            for v in [r.x_mm, r.y_mm, r.length_mm, r.width_mm, r.height_mm, r.thickness_mm].into_iter().flatten() {
                if !v.is_finite() {
                    return Err(format!("as-built row '{}': a value is not a number", r.name));
                }
            }
        }
        Ok(())
    }
}

const COLS: [&str; 7] = ["name", "x_mm", "y_mm", "length_mm", "width_mm", "height_mm", "thickness_mm"];

/// Reads CSV text (comma or semicolon; a header line naming the columns). Errors carry the line number.
pub fn parse_csv(text: &str) -> Result<Vec<Measure>, String> {
    let mut lines = text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
    let (_, head) = lines.next().ok_or("the file is empty")?;
    let sep = if head.contains(';') && !head.contains(',') { ';' } else { ',' };
    let cols: Vec<String> = head.split(sep).map(|c| c.trim().trim_start_matches('\u{feff}').to_lowercase()).collect();
    if let Some(bad) = cols.iter().find(|c| !COLS.contains(&c.as_str())) {
        return Err(format!("line 1: unknown column '{bad}' (use: {})", COLS.join(", ")));
    }
    let name_at = cols.iter().position(|c| c == "name").ok_or("line 1: a 'name' column is required")?;
    let mut out = Vec::new();
    for (i, line) in lines {
        let cells: Vec<&str> = line.split(sep).map(str::trim).collect();
        if cells.len() > cols.len() {
            return Err(format!("line {}: {} values for {} columns", i + 1, cells.len(), cols.len()));
        }
        let mut m = Measure { name: cells.get(name_at).copied().unwrap_or("").to_string(), ..Default::default() };
        if m.name.is_empty() {
            return Err(format!("line {}: the name is empty", i + 1));
        }
        for (k, c) in cols.iter().enumerate() {
            let Some(cell) = cells.get(k).filter(|c| !c.is_empty()) else { continue };
            if c == "name" {
                continue;
            }
            let v: f64 = cell.parse().map_err(|_| format!("line {}: '{cell}' in {c} is not a number", i + 1))?;
            match c.as_str() {
                "x_mm" => m.x_mm = Some(v),
                "y_mm" => m.y_mm = Some(v),
                "length_mm" => m.length_mm = Some(v),
                "width_mm" => m.width_mm = Some(v),
                "height_mm" => m.height_mm = Some(v),
                _ => m.thickness_mm = Some(v),
            }
        }
        out.push(m);
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
pub struct Deviation {
    pub name: String,
    pub id: String,
    pub what: &'static str,
    pub design_mm: f64,
    pub measured_mm: f64,
    pub deviation_mm: f64,
    /// `ok`, `over` or `no tolerance`.
    pub status: &'static str,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DevReport {
    pub deviations: Vec<Deviation>,
    pub matched: usize,
    pub unmatched: Vec<String>,
    pub over: usize,
    pub max_abs_mm: f64,
    /// Elements with a deviation, by id, with the largest one (signed) and its status: what the viewers colour.
    pub by_element: BTreeMap<String, (f64, &'static str)>,
    /// Point deviations for the plan: `(id, design x, design y, dx, dy, status)`.
    pub points: Vec<(String, [f64; 2], [f64; 2], &'static str)>,
    pub notes: Vec<String>,
}

/// The design values of an element: `(what, value)` pairs it has.
fn design(e: &crate::model::Element) -> Vec<(&'static str, f64)> {
    if let Ok(c) = e.typed::<Column>() {
        return vec![("x", c.x as f64), ("y", c.y as f64), ("width", c.b as f64), ("length", c.h as f64)];
    }
    if let Ok(f) = e.typed::<Footing>() {
        return vec![("x", f.x as f64), ("y", f.y as f64), ("width", f.width as f64), ("length", f.length as f64)];
    }
    if let Ok(b) = e.typed::<Beam>() {
        return vec![("length", b.length as f64), ("width", b.b as f64), ("height", b.h as f64)];
    }
    if let Ok(s) = e.typed::<Slab>() {
        return vec![("x", s.x as f64), ("y", s.y as f64), ("width", s.w as f64), ("length", s.h as f64), ("thickness", s.thickness as f64)];
    }
    if let Ok(w) = e.typed::<Wall>() {
        return vec![("length", w.length as f64), ("thickness", w.thickness as f64), ("height", w.height as f64)];
    }
    if let Ok(s) = e.typed::<Space>() {
        return vec![("x", s.x as f64), ("y", s.y as f64), ("width", s.w as f64), ("length", s.h as f64)];
    }
    Vec::new()
}

/// Finds the element a name refers to: a schedule name (`COL-3`), `room:<requirement key>`, or an element id.
/// `names` is [`schedule_names`] of the same model.
pub fn resolve_ref(model: &Model, names: &BTreeMap<String, String>, name: &str) -> Option<Id> {
    if let Some(k) = name.strip_prefix("room:") {
        return model.elements.values().find(|e| e.typed::<Space>().is_ok_and(|s| s.requirement_key == k)).map(|e| e.id);
    }
    if let Some((id, _)) = names.iter().find(|(_, n)| n.as_str() == name) {
        return id.parse().ok();
    }
    name.parse().ok().filter(|i| model.get(i).is_some())
}

pub fn compare(model: &Model, ab: &AsBuilt) -> DevReport {
    let mut rep = DevReport::default();
    let names = schedule_names(model, None);
    for row in &ab.rows {
        let Some(el) = resolve_ref(model, &names, &row.name).and_then(|i| model.get(&i)) else {
            rep.unmatched.push(row.name.clone());
            continue;
        };
        rep.matched += 1;
        let d = design(el);
        let given: [(&'static str, Option<f64>); 6] = [("x", row.x_mm), ("y", row.y_mm), ("length", row.length_mm), ("width", row.width_mm), ("height", row.height_mm), ("thickness", row.thickness_mm)];
        let mut point: (Option<f64>, Option<f64>) = (None, None);
        for (what, measured) in given.iter().filter_map(|(w, v)| v.map(|v| (*w, v))) {
            let Some((_, dv)) = d.iter().find(|(w, _)| *w == what) else {
                rep.notes.push(format!("'{}' ({}) has no design {what} to compare with", row.name, el.kind));
                continue;
            };
            let dev = measured - dv;
            let status = match ab.tolerance_mm {
                None => "no tolerance",
                Some(t) if dev.abs() <= t + 1e-9 => "ok",
                Some(_) => "over",
            };
            if status == "over" {
                rep.over += 1;
            }
            rep.max_abs_mm = rep.max_abs_mm.max(dev.abs());
            let e = rep.by_element.entry(el.id.to_string()).or_insert((0.0, "ok"));
            if dev.abs() >= e.0.abs() {
                e.0 = (dev * 100.0).round() / 100.0;
            }
            if status == "over" || (status == "no tolerance" && e.1 == "ok") {
                e.1 = status;
            }
            if what == "x" {
                point.0 = Some(dev);
            }
            if what == "y" {
                point.1 = Some(dev);
            }
            rep.deviations.push(Deviation { name: row.name.clone(), id: el.id.to_string(), what, design_mm: (dv * 100.0).round() / 100.0, measured_mm: measured, deviation_mm: (dev * 100.0).round() / 100.0, status });
        }
        if let (true, Some(dx0), Some(dy0)) = (point.0.is_some() || point.1.is_some(), d.iter().find(|(w, _)| *w == "x").map(|p| p.1), d.iter().find(|(w, _)| *w == "y").map(|p| p.1)) {
            let st = rep.deviations.iter().rev().find(|v| v.id == el.id.to_string() && (v.what == "x" || v.what == "y")).map_or("no tolerance", |v| v.status);
            let over = rep.deviations.iter().filter(|v| v.id == el.id.to_string() && (v.what == "x" || v.what == "y")).any(|v| v.status == "over");
            rep.points.push((el.id.to_string(), [dx0, dy0], [point.0.unwrap_or(0.0), point.1.unwrap_or(0.0)], if over { "over" } else { st }));
        }
    }
    rep.max_abs_mm = (rep.max_abs_mm * 100.0).round() / 100.0;
    if ab.tolerance_mm.is_none() && !rep.deviations.is_empty() {
        rep.notes.push("no tolerance entered: the deviations are reported but none is judged".into());
    }
    rep
}

/// Markers of the point deviations on a plan SVG made by `drawing::plan_svg` with the same options: a circle at the design position,
/// a line to the measured one drawn at the plan's scale, coloured by status.
pub fn add_markers(svg: &str, model: &Model, level: Option<Id>, opts: &DrawOpts, rep: &DevReport) -> String {
    let Some((minx, maxy, s, m)) = drawing::frame(model, level, opts) else { return svg.to_string() };
    let mut g = String::from("<g class=\"asbuilt\">\n");
    let mut n = 0;
    for (id, at, d, status) in &rep.points {
        let Some(el) = model.get(&id.parse::<Id>().unwrap_or_default()) else { continue };
        if level.is_some() && el.level_id != level {
            continue;
        }
        let col = match *status {
            "ok" => "#2e7d32",
            "over" => "#c62828",
            _ => "#757575",
        };
        let (x, y) = (m + (at[0] - minx) * s, m + (maxy - at[1]) * s);
        let (x2, y2) = (m + (at[0] + d[0] - minx) * s, m + (maxy - at[1] - d[1]) * s);
        let _ = writeln!(g, "<circle class=\"dev\" data-id=\"{id}\" data-status=\"{status}\" cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"7\" style=\"fill:none;stroke:{col};stroke-width:2\"><title>dx {:.1} mm, dy {:.1} mm</title></circle><line x1=\"{x:.1}\" y1=\"{y:.1}\" x2=\"{x2:.1}\" y2=\"{y2:.1}\" style=\"stroke:{col};stroke-width:2\"/>", d[0], d[1]);
        n += 1;
    }
    g.push_str("</g>\n");
    if n == 0 {
        return svg.to_string();
    }
    match svg.rfind("</svg>") {
        Some(i) => format!("{}{}{}", &svg[..i], g, &svg[i..]),
        None => svg.to_string(),
    }
}
