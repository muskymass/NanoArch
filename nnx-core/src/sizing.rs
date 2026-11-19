//! Structural schedules from the model: column loads, footings, tie beams, concrete and reinforcement, every row linked to
//! the id of the element it describes.
//!
//! - **Columns (`COL-n`)**: the axial service load the model holds (`axial_service_kn`, from the tributary take-down in
//!   `structural`) next to the tributary floor area that produces it.
//! - **Footings (`FND-n`)**: the footing of each column as the model holds it, checked against the allowable bearing
//!   pressure of the soil report. A footing smaller than the load needs is flagged.
//! - **Tie beams (`TB-n`)**: between footings that are neighbours in a row or column of the grid, sized from your width and
//!   span-to-depth ratio. They are a schedule here, not model elements.
//! - **Concrete (m3)** of columns, footings, beams, slabs and tie beams, by element.
//! - **Reinforcement (bar bending schedule)**: from bar templates YOU give (diameters, counts, spacings, cover, anchorage and
//!   hook allowance). The application does not design reinforcement, so nothing is derived from loads: the schedule is
//!   cut lengths and weights of the bars you specified. The only constant is the density of steel (7850 kg/m3).
//!
//! No code coefficient is used. Results are preliminary and need a licensed structural engineer.

use crate::building;
use crate::model::{Id, Model};
use crate::typed::{Beam, Column, Footing, Slab, Typed, Wall};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write;

const STEEL_KG_M3: f64 = 7850.0;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TieBeamIn {
    pub width_mm: Option<f64>,
    /// Span divided by depth: depth = span / ratio, rounded up to 50 mm.
    pub span_depth_ratio: Option<f64>,
}

/// Longitudinal bars and ties of a column.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ColumnBars {
    pub bar_dia_mm: f64,
    pub count: u32,
    pub tie_dia_mm: f64,
    pub tie_spacing_mm: f64,
}

/// A mesh of bars in both directions (footings).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MeshBars {
    pub bar_dia_mm: f64,
    pub spacing_mm: f64,
}

/// Top and bottom bars and stirrups of a beam.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BeamBars {
    pub top_dia_mm: f64,
    pub top_count: u32,
    pub bottom_dia_mm: f64,
    pub bottom_count: u32,
    pub stirrup_dia_mm: f64,
    pub stirrup_spacing_mm: f64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RebarIn {
    pub cover_mm: Option<f64>,
    /// Extra length of a column bar into the footing.
    pub anchorage_mm: Option<f64>,
    /// Extra length of each hook or bend.
    pub hook_mm: Option<f64>,
    pub column: Option<ColumnBars>,
    pub footing: Option<MeshBars>,
    pub beam: Option<BeamBars>,
    /// Tie beams use the beam template when this is absent.
    pub tie_beam: Option<BeamBars>,
    /// Design parameters: bars not covered by a template are sized from the loads (see `rebar`).
    pub design: Option<crate::rebar::DesignIn>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SizingInputs {
    /// Footing thickness, mm. Footings have no designed depth in this application, so the concrete of footings needs it.
    pub footing_thickness_mm: Option<f64>,
    pub tie_beam: Option<TieBeamIn>,
    pub rebar: Option<RebarIn>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ColumnRow {
    pub name: String,
    pub id: String,
    pub grid_ref: String,
    pub level: Option<i64>,
    pub x: i64,
    pub y: i64,
    pub section: String,
    pub tributary_m2: f64,
    pub axial_kn: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FootingRow {
    pub name: String,
    pub id: String,
    pub column: String,
    pub provided_mm: String,
    pub load_kn: f64,
    pub bearing_kpa: f64,
    /// Required plan area for the load at the soil value, m2 (None without a soil value).
    pub required_m2: Option<f64>,
    /// Required side of a square footing, mm, rounded up to 50 mm.
    pub required_side_mm: Option<i64>,
    /// `ok`, `undersized` or `no soil value`.
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TieBeamRow {
    pub name: String,
    /// The `TieBeam` element once `apply_tie_beams` has put it in the model.
    pub id: Option<String>,
    /// Clear ends on the line between the two footings, mm.
    pub x1: i64,
    pub y1: i64,
    pub x2: i64,
    pub y2: i64,
    pub from: String,
    pub to: String,
    pub from_id: String,
    pub to_id: String,
    pub span_mm: i64,
    pub clear_mm: i64,
    pub width_mm: i64,
    pub depth_mm: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConcreteRow {
    pub element: String,
    pub id: Option<String>,
    pub kind: String,
    pub volume_m3: f64,
    pub basis: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BarRow {
    pub element: String,
    pub id: Option<String>,
    pub mark: String,
    pub role: String,
    pub dia_mm: f64,
    pub number: u64,
    pub cut_length_mm: f64,
    pub total_m: f64,
    pub kg: f64,
    /// `template` (yours) or `design` (sized by `rebar`).
    pub source: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SizingReport {
    pub columns: Vec<ColumnRow>,
    pub footings: Vec<FootingRow>,
    pub tie_beams: Vec<TieBeamRow>,
    pub concrete: Vec<ConcreteRow>,
    pub concrete_total_m3: f64,
    pub bars: Vec<BarRow>,
    pub steel_total_kg: f64,
    /// What the design chose per element, with its status.
    pub designs: Vec<crate::rebar::DesignRow>,
    /// Section -> the inputs that are missing, so it was not computed.
    pub missing: BTreeMap<String, Vec<String>>,
    pub warnings: Vec<String>,
}

fn round_up_50(v: f64) -> i64 {
    ((v / 50.0).ceil() * 50.0) as i64
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn mm_pos(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite() && *x > 0.0)
}

fn kg_of(dia_mm: f64, total_m: f64) -> f64 {
    std::f64::consts::PI / 4.0 * (dia_mm / 1000.0).powi(2) * total_m * STEEL_KG_M3
}

fn level_index(model: &Model, level: Option<Id>) -> Option<i64> {
    let l = level?;
    building::levels(model).iter().find(|(_, id)| *id == l).map(|(i, _)| *i)
}

fn storey_mm(model: &Model, level: Option<Id>) -> Option<i64> {
    let at = |lvl: Option<Id>| model.elements.values().filter(|e| e.kind == Wall::KIND && e.level_id == lvl).filter_map(|e| e.typed::<Wall>().ok()).map(|w| w.height).max();
    at(level).or_else(|| model.elements.values().filter(|e| e.kind == Wall::KIND).filter_map(|e| e.typed::<Wall>().ok()).map(|w| w.height).max())
}

/// Concrete and bar bending schedules need a bar row builder: one per template line.
fn bars(rows: &mut Vec<BarRow>, who: (&str, Option<String>, &str), mark: &str, role: &str, dia: f64, number: u64, cut_mm: f64) {
    let (element, id, source) = who;
    if number == 0 || cut_mm <= 0.0 || dia <= 0.0 {
        return;
    }
    let total_m = number as f64 * cut_mm / 1000.0;
    rows.push(BarRow { element: element.into(), id, mark: mark.into(), role: role.into(), dia_mm: dia, number, cut_length_mm: (cut_mm * 10.0).round() / 10.0, total_m: round3(total_m), kg: round3(kg_of(dia, total_m)), source: source.into() });
}

pub fn compute(model: &Model, soil_kpa: Option<f64>, inp: &SizingInputs) -> SizingReport {
    let mut rep = SizingReport::default();
    let tributary = crate::structural::column_tributary(model);

    // ---- columns
    let mut cols: Vec<(&crate::model::Element, Column)> = model.elements.values().filter(|e| e.kind == Column::KIND).filter_map(|e| Some((e, e.typed::<Column>().ok()?))).collect();
    cols.sort_by_key(|(e, c)| (level_index(model, e.level_id), c.y, c.x));
    let mut col_name: BTreeMap<Id, String> = BTreeMap::new();
    for (i, (e, c)) in cols.iter().enumerate() {
        let name = format!("COL-{}", i + 1);
        col_name.insert(e.id, name.clone());
        rep.columns.push(ColumnRow {
            name,
            id: e.id.to_string(),
            grid_ref: c.grid_ref.clone(),
            level: level_index(model, e.level_id),
            x: c.x,
            y: c.y,
            section: format!("{}x{}", c.b, c.h),
            tributary_m2: round3(tributary.get(&e.id).copied().unwrap_or(0.0)),
            axial_kn: c.axial_service_kn,
        });
    }
    if cols.is_empty() {
        rep.warnings.push("the model has no columns: press Sync structure after choosing a layout".into());
    } else if cols.iter().all(|(_, c)| c.axial_service_kn.is_none()) {
        rep.missing.insert("columns".into(), vec!["floor loads (dead and live kPa): without them no axial load is computed".into()]);
    }

    // ---- footings
    let mut fnds: Vec<(&crate::model::Element, Footing)> = model.elements.values().filter(|e| e.kind == Footing::KIND).filter_map(|e| Some((e, e.typed::<Footing>().ok()?))).collect();
    fnds.sort_by_key(|(_, f)| (col_name.get(&f.column.parse::<Id>().unwrap_or_default()).and_then(|n| n.trim_start_matches("COL-").parse::<usize>().ok()).unwrap_or(usize::MAX), f.x, f.y));
    let mut fnd_name: BTreeMap<Id, String> = BTreeMap::new();
    for (i, (e, f)) in fnds.iter().enumerate() {
        let name = format!("FND-{}", i + 1);
        fnd_name.insert(e.id, name.clone());
        let col = f.column.parse::<Id>().ok().and_then(|c| col_name.get(&c).cloned()).unwrap_or_else(|| f.column.clone());
        let (req, side, status) = match soil_kpa.filter(|q| *q > 0.0) {
            Some(q) => {
                let area = f.service_load_kn / q;
                let provided = f.length as f64 / 1000.0 * f.width as f64 / 1000.0;
                (Some(round3(area)), Some(round_up_50(area.sqrt() * 1000.0)), if provided * q + 1e-6 >= f.service_load_kn { "ok" } else { "undersized" })
            }
            None => (None, None, "no soil value"),
        };
        rep.footings.push(FootingRow { name, id: e.id.to_string(), column: col, provided_mm: format!("{}x{}", f.length, f.width), load_kn: f.service_load_kn, bearing_kpa: f.bearing_kpa, required_m2: req, required_side_mm: side, status: status.into() });
    }
    if fnds.is_empty() && !cols.is_empty() {
        rep.missing.insert("footings".into(), vec!["soil bearing capacity (kPa) and floor loads: footings are made when both are entered".into()]);
    } else if soil_kpa.is_none() && !fnds.is_empty() {
        rep.missing.insert("footing check".into(), vec!["soil bearing capacity (kPa) from the soil report, to check the footings against".into()]);
    }

    // ---- tie beams between neighbouring footings of a row or column of the grid
    let mut pairs: Vec<(Id, Id, bool)> = Vec::new();
    let tol = 50;
    let mut by_row: BTreeMap<i64, Vec<&(&crate::model::Element, Footing)>> = BTreeMap::new();
    let mut by_col: BTreeMap<i64, Vec<&(&crate::model::Element, Footing)>> = BTreeMap::new();
    for f in &fnds {
        by_row.entry((f.1.y as f64 / tol as f64).round() as i64).or_default().push(f);
        by_col.entry((f.1.x as f64 / tol as f64).round() as i64).or_default().push(f);
    }
    for (along_x, groups) in [(true, &mut by_row), (false, &mut by_col)] {
        for g in groups.values_mut() {
            g.sort_by_key(|f| if along_x { f.1.x } else { f.1.y });
            for w in g.windows(2) {
                pairs.push((w[0].0.id, w[1].0.id, along_x));
            }
        }
    }
    let tb_in = inp.tie_beam.clone().unwrap_or_default();
    let (tb_w, tb_r) = (mm_pos(tb_in.width_mm), mm_pos(tb_in.span_depth_ratio));
    if !pairs.is_empty() {
        let mut miss = Vec::new();
        if tb_w.is_none() {
            miss.push("tie_beam.width_mm".to_string());
        }
        if tb_r.is_none() {
            miss.push("tie_beam.span_depth_ratio".to_string());
        }
        if !miss.is_empty() {
            rep.missing.insert("tie beams".into(), miss);
        }
    }
    if let (Some(w), Some(r)) = (tb_w, tb_r) {
        pairs.sort_by_key(|(a, b, _)| (fnd_name.get(a).cloned().unwrap_or_default(), fnd_name.get(b).cloned().unwrap_or_default()));
        for (a, b, along_x) in &pairs {
            let (fa, fb) = (model.get(a).and_then(|e| e.typed::<Footing>().ok()), model.get(b).and_then(|e| e.typed::<Footing>().ok()));
            let (Some(fa), Some(fb)) = (fa, fb) else { continue };
            let (span, half) = if *along_x { ((fb.x - fa.x).abs(), fa.length / 2 + fb.length / 2) } else { ((fb.y - fa.y).abs(), fa.width / 2 + fb.width / 2) };
            let clear = span - half;
            let (na, nb) = (fnd_name.get(a).cloned().unwrap_or_default(), fnd_name.get(b).cloned().unwrap_or_default());
            if clear <= 0 {
                rep.warnings.push(format!("{na} and {nb} touch or overlap: no tie beam between them"));
                continue;
            }
            let (x1, y1, x2, y2) = if *along_x { (fa.x + fa.length / 2, fa.y, fb.x - fb.length / 2, fa.y) } else { (fa.x, fa.y + fa.width / 2, fa.x, fb.y - fb.width / 2) };
            let existing = model.elements.values().find(|e| e.kind == crate::typed::TieBeam::KIND && e.props.get("from").and_then(|v| v.as_str()) == Some(&a.to_string()) && e.props.get("to").and_then(|v| v.as_str()) == Some(&b.to_string())).map(|e| e.id.to_string());
            rep.tie_beams.push(TieBeamRow {
                name: format!("TB-{}", rep.tie_beams.len() + 1),
                id: existing,
                x1,
                y1,
                x2,
                y2,
                from: na,
                to: nb,
                from_id: a.to_string(),
                to_id: b.to_string(),
                span_mm: span,
                clear_mm: clear,
                width_mm: w.round() as i64,
                depth_mm: round_up_50(span as f64 / r),
            });
        }
    }

    // ---- concrete
    let add = |rep: &mut SizingReport, element: String, id: Option<String>, kind: &str, vol: f64, basis: String| {
        rep.concrete.push(ConcreteRow { element, id, kind: kind.into(), volume_m3: round3(vol), basis });
        rep.concrete_total_m3 = round3(rep.concrete_total_m3 + round3(vol));
    };
    for (row, (e, c)) in rep.columns.clone().iter().zip(&cols) {
        let Some(h) = storey_mm(model, e.level_id) else { continue };
        add(&mut rep, row.name.clone(), Some(row.id.clone()), "Column", c.b as f64 * c.h as f64 * h as f64 / 1e9, format!("{} x {} x {} mm storey", c.b, c.h, h));
    }
    let thick = mm_pos(inp.footing_thickness_mm);
    if !fnds.is_empty() {
        match thick {
            Some(t) => {
                for (row, (_, f)) in rep.footings.clone().iter().zip(&fnds) {
                    add(&mut rep, row.name.clone(), Some(row.id.clone()), "Footing", f.length as f64 * f.width as f64 * t / 1e9, format!("{} x {} x {t} mm", f.length, f.width));
                }
            }
            None => {
                rep.missing.insert("footing concrete".into(), vec!["footing_thickness_mm (footings have no designed depth)".into()]);
            }
        }
    }
    let mut beams: Vec<(&crate::model::Element, Beam)> = model.elements.values().filter(|e| e.kind == Beam::KIND).filter_map(|e| Some((e, e.typed::<Beam>().ok()?))).collect();
    beams.sort_by_key(|(e, b)| (level_index(model, e.level_id), b.y1.min(b.y2), b.x1.min(b.x2), b.x1 == b.x2));
    let slab_t = |level: Option<Id>| model.elements.values().filter(|e| e.kind == Slab::KIND && e.level_id == level).filter_map(|e| e.typed::<Slab>().ok()).map(|s| s.thickness).max().unwrap_or(0);
    let mut beam_rows: Vec<(String, String, Beam, i64)> = Vec::new();
    for (i, (e, b)) in beams.iter().enumerate() {
        let name = format!("BM-{}", i + 1);
        let below = (b.h - slab_t(e.level_id)).max(0);
        add(&mut rep, name.clone(), Some(e.id.to_string()), "Beam", b.b as f64 * below as f64 * b.length as f64 / 1e9, format!("{} x {} x {} mm (depth below the slab)", b.b, below, b.length));
        beam_rows.push((name, e.id.to_string(), b.clone(), below));
    }
    for tb in rep.tie_beams.clone() {
        add(&mut rep, tb.name.clone(), tb.id.clone(), "TieBeam", tb.width_mm as f64 * tb.depth_mm as f64 * tb.clear_mm as f64 / 1e9, format!("{} x {} x {} mm clear between {} and {}", tb.width_mm, tb.depth_mm, tb.clear_mm, tb.from, tb.to));
    }
    let mut slabs: Vec<(&crate::model::Element, Slab)> = model.elements.values().filter(|e| e.kind == Slab::KIND).filter_map(|e| Some((e, e.typed::<Slab>().ok()?))).collect();
    slabs.sort_by_key(|(e, s)| (level_index(model, e.level_id), s.y, s.x));
    for (i, (e, s)) in slabs.iter().enumerate() {
        add(&mut rep, format!("SL-{}", i + 1), Some(e.id.to_string()), "Slab", s.w as f64 * s.h as f64 * s.thickness as f64 / 1e9, format!("{} x {} x {} mm", s.w, s.h, s.thickness));
    }

    // ---- reinforcement: your templates first, and the design (`rebar`) for what has none
    if let Some(rb) = &inp.rebar {
        let has_template = rb.column.is_some() || rb.footing.is_some() || rb.beam.is_some() || rb.tie_beam.is_some();
        let (cover, anchor, hook) = (mm_pos(rb.cover_mm), rb.anchorage_mm.filter(|v| *v >= 0.0), rb.hook_mm.filter(|v| *v >= 0.0));
        let needs_cover = has_template || rb.design.is_none();
        let mut miss = Vec::new();
        for (n, v) in [("rebar.cover_mm", cover.is_some() || !needs_cover), ("rebar.anchorage_mm", anchor.is_some()), ("rebar.hook_mm", hook.is_some())] {
            if !v {
                miss.push(n.to_string());
            }
        }
        let designed = rb.design.as_ref().map(|dsn| {
            let col_in: Vec<crate::rebar::ColIn> = rep.columns.iter().zip(&cols).map(|(row, (_, c))| crate::rebar::ColIn { name: &row.name, id: &row.id, b: c.b as f64, h: c.h as f64, axial_kn: row.axial_kn }).collect();
            let fnd_in: Vec<crate::rebar::FndIn> = rep
                .footings
                .iter()
                .zip(&fnds)
                .map(|(row, (_, f))| {
                    let (cb, ch) = f.column.parse::<Id>().ok().and_then(|cid| cols.iter().find(|(e, _)| e.id == cid)).map_or((0.0, 0.0), |(_, c)| (c.b as f64, c.h as f64));
                    crate::rebar::FndIn { name: &row.name, id: &row.id, length: f.length as f64, width: f.width as f64, load_kn: f.service_load_kn, col_b: cb, col_h: ch }
                })
                .collect();
            let beam_in: Vec<crate::rebar::BeamIn> = beam_rows
                .iter()
                .map(|(n, id, b, below)| crate::rebar::BeamIn { name: n, id: Some(id), b: b.b as f64, depth: (*below).max(b.h) as f64, tie: false })
                .chain(rep.tie_beams.iter().map(|t| crate::rebar::BeamIn { name: &t.name, id: t.id.as_deref(), b: t.width_mm as f64, depth: t.depth_mm as f64, tie: true }))
                .collect();
            crate::rebar::design(dsn, thick, &col_in, &fnd_in, &beam_in)
        });
        if let Some(d) = &designed {
            rep.designs = d.rows.clone();
            rep.warnings.extend(d.warnings.clone());
            if !d.missing.is_empty() {
                rep.missing.insert("design".into(), d.missing.clone());
            }
        }
        if !miss.is_empty() {
            rep.missing.insert("reinforcement".into(), miss);
        } else {
            let (a, hk) = (anchor.expect("checked"), hook.expect("checked"));
            let c_tmpl = cover.unwrap_or(0.0);
            let none = crate::rebar::DesignOut::default();
            let des = designed.as_ref().unwrap_or(&none);
            for (row, (e, col)) in rep.columns.clone().iter().zip(&cols) {
                let Some(h) = storey_mm(model, e.level_id) else { continue };
                let h = h as f64;
                let (t, c, src) = match (&rb.column, des.columns.get(&row.name)) {
                    (Some(t), _) => (t.clone(), c_tmpl, "template"),
                    (None, Some((t, cv))) => (t.clone(), *cv, "design"),
                    _ => continue,
                };
                bars(&mut rep.bars, (&row.name, Some(row.id.clone()), src), &format!("{}/V", row.name), "vertical bars", t.bar_dia_mm, t.count as u64, h + a);
                let perimeter = 2.0 * ((col.b as f64 - 2.0 * c) + (col.h as f64 - 2.0 * c)) + 2.0 * hk;
                let n = if t.tie_spacing_mm > 0.0 { (h / t.tie_spacing_mm).floor() as u64 + 1 } else { 0 };
                bars(&mut rep.bars, (&row.name, Some(row.id.clone()), src), &format!("{}/T", row.name), "ties", t.tie_dia_mm, n, perimeter);
            }
            for (row, (_, f)) in rep.footings.clone().iter().zip(&fnds) {
                let (m, c, src) = match (&rb.footing, des.footings.get(&row.name)) {
                    (Some(m), _) => (m.clone(), c_tmpl, "template"),
                    (None, Some((m, cv))) => (m.clone(), *cv, "design"),
                    _ => continue,
                };
                if m.spacing_mm <= 0.0 {
                    continue;
                }
                let (l, w) = (f.length as f64, f.width as f64);
                bars(&mut rep.bars, (&row.name, Some(row.id.clone()), src), &format!("{}/X", row.name), "bottom mesh, along x", m.bar_dia_mm, ((w - 2.0 * c) / m.spacing_mm).floor() as u64 + 1, l - 2.0 * c + 2.0 * hk);
                bars(&mut rep.bars, (&row.name, Some(row.id.clone()), src), &format!("{}/Y", row.name), "bottom mesh, along y", m.bar_dia_mm, ((l - 2.0 * c) / m.spacing_mm).floor() as u64 + 1, w - 2.0 * c + 2.0 * hk);
            }
            let beam_bars = |rep: &mut SizingReport, name: &str, id: Option<String>, b_mm: f64, depth: f64, len: f64, t: &BeamBars, c: f64, src: &str| {
                bars(&mut rep.bars, (name, id.clone(), src), &format!("{name}/T"), "top bars", t.top_dia_mm, t.top_count as u64, len - 2.0 * c + 2.0 * hk);
                bars(&mut rep.bars, (name, id.clone(), src), &format!("{name}/B"), "bottom bars", t.bottom_dia_mm, t.bottom_count as u64, len - 2.0 * c + 2.0 * hk);
                let perimeter = 2.0 * ((b_mm - 2.0 * c) + (depth - 2.0 * c)) + 2.0 * hk;
                let n = if t.stirrup_spacing_mm > 0.0 { ((len - 2.0 * c) / t.stirrup_spacing_mm).floor() as u64 + 1 } else { 0 };
                bars(&mut rep.bars, (name, id, src), &format!("{name}/S"), "stirrups", t.stirrup_dia_mm, n, perimeter);
            };
            for (name, id, b, below) in &beam_rows {
                let (t, c, src) = match (&rb.beam, des.beams.get(name)) {
                    (Some(t), _) => (t.clone(), c_tmpl, "template"),
                    (None, Some((t, cv))) => (t.clone(), *cv, "design"),
                    _ => continue,
                };
                beam_bars(&mut rep, name, Some(id.clone()), b.b as f64, (*below).max(b.h) as f64, b.length as f64, &t, c, src);
            }
            for tb in rep.tie_beams.clone() {
                let (t, c, src) = match (rb.tie_beam.as_ref().or(rb.beam.as_ref()), des.beams.get(&tb.name)) {
                    (Some(t), _) => (t.clone(), c_tmpl, "template"),
                    (None, Some((t, cv))) => (t.clone(), *cv, "design"),
                    _ => continue,
                };
                beam_bars(&mut rep, &tb.name, tb.id.clone(), tb.width_mm as f64, tb.depth_mm as f64, tb.clear_mm as f64, &t, c, src);
            }
            rep.steel_total_kg = round3(rep.bars.iter().map(|b| b.kg).sum());
        }
    } else {
        rep.missing.insert("reinforcement".into(), vec!["rebar (anchorage, hook and either bar templates or the design parameters): enter them".into()]);
    }
    rep.warnings.push("preliminary: self-weight is not in the column loads; reinforcement is your templates or the minimum/axial design of `rebar` (see its notes), not a complete design; footing depth is your input".into());
    rep
}

/// Puts the tie beams of the report into the model as `TieBeam` elements, replacing the ones generated before, as one undoable
/// step. Identical beams keep their ids. Returns `(created, removed)`.
pub fn apply_tie_beams(doc: &mut crate::history::Document, rep: &SizingReport) -> Result<(usize, usize), String> {
    use crate::command::Command;
    use crate::model::Source;
    use crate::typed::{TieBeam, element_of};
    let signature = |e: &crate::model::Element| format!("{}", serde_json::Value::Object(e.props.clone()));
    let mut existing: BTreeMap<String, Vec<Id>> = BTreeMap::new();
    let mut all: Vec<Id> = Vec::new();
    for e in doc.model.elements.values().filter(|e| e.kind == TieBeam::KIND && e.provenance.source == Source::Generated && !e.provenance.locked) {
        existing.entry(signature(e)).or_default().push(e.id);
        all.push(e.id);
    }
    let (mut keep, mut creates): (Vec<Id>, Vec<Command>) = (Vec::new(), Vec::new());
    for t in &rep.tie_beams {
        let mut el = element_of(&TieBeam { x1: t.x1, y1: t.y1, x2: t.x2, y2: t.y2, width: t.width_mm, depth: t.depth_mm, from: t.from_id.clone(), to: t.to_id.clone(), extra: Default::default() }, Source::Generated);
        // On the level of the footings it ties.
        el.level_id = t.from_id.parse::<Id>().ok().and_then(|f| doc.model.get(&f)).and_then(|f| f.level_id);
        match existing.get_mut(&signature(&el)).and_then(|v| v.pop()) {
            Some(id) => keep.push(id),
            None => creates.push(Command::Create(el)),
        }
    }
    let mut cmds: Vec<Command> = all.iter().filter(|id| !keep.contains(id)).map(|id| Command::Delete(*id)).collect();
    let (removed, created) = (cmds.len(), creates.len());
    cmds.extend(creates);
    if !cmds.is_empty() {
        doc.commit("tie beams", crate::command::Origin::User, cmds).map_err(|e| e.to_string())?;
    }
    Ok((created, removed))
}

/// Concrete (m3) and steel (kg) of columns, footings and tie beams as take-off items for the schedule, each with the ids of the
/// elements it was measured from: `COL.CONC`, `FND.CONC`, `TB.CONC`, `COL.STEEL`, `FND.STEEL`, `TB.STEEL`.
pub fn schedule_items(rep: &SizingReport) -> Vec<crate::qto::QtoItem> {
    use crate::qto::{QtoItem, Unit};
    let parse = |s: &Option<String>| s.as_ref().and_then(|v| v.parse::<Id>().ok());
    let mut out = Vec::new();
    for (code, kind, label) in [("COL.CONC", "Column", "Column concrete"), ("FND.CONC", "Footing", "Footing concrete"), ("TB.CONC", "TieBeam", "Tie beam concrete")] {
        let parts: Vec<(Id, f64)> = rep.concrete.iter().filter(|c| c.kind == kind).filter_map(|c| Some((parse(&c.id)?, c.volume_m3))).collect();
        if !parts.is_empty() {
            out.push(QtoItem { code: code.into(), description: label.into(), unit: Unit::M3, quantity: round3(parts.iter().map(|p| p.1).sum()), waste_pct: None, sources: parts.iter().map(|p| p.0).collect(), parts });
        }
    }
    for (code, prefix, label) in [("COL.STEEL", "COL-", "Column reinforcement"), ("FND.STEEL", "FND-", "Footing reinforcement"), ("TB.STEEL", "TB-", "Tie beam reinforcement")] {
        let mut per: BTreeMap<Id, f64> = BTreeMap::new();
        for b in rep.bars.iter().filter(|b| b.element.starts_with(prefix)) {
            if let Some(id) = parse(&b.id) {
                *per.entry(id).or_default() += b.kg;
            }
        }
        if !per.is_empty() {
            let parts: Vec<(Id, f64)> = per.into_iter().collect();
            out.push(QtoItem { code: code.into(), description: label.into(), unit: Unit::Kg, quantity: round3(parts.iter().map(|p| p.1).sum()), waste_pct: None, sources: parts.iter().map(|p| p.0).collect(), parts });
        }
    }
    out
}

fn csv_cell(s: &str) -> String {
    if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

impl SizingReport {
    /// Columns, footings and tie beams with their element ids.
    pub fn members_csv(&self) -> String {
        let mut o = String::from("mark,element_id,kind,detail,load_kn,tributary_m2,status\n");
        for c in &self.columns {
            let _ = writeln!(o, "{},{},Column,{} at ({} {}) {},{},{},", c.name, c.id, csv_cell(&c.grid_ref), c.x, c.y, c.section, c.axial_kn.map_or(String::new(), |v| v.to_string()), c.tributary_m2);
        }
        for f in &self.footings {
            let _ = writeln!(o, "{},{},Footing,{} under {},{},,{}", f.name, f.id, f.provided_mm, f.column, f.load_kn, f.status);
        }
        for t in &self.tie_beams {
            let _ = writeln!(o, "{},{},TieBeam,{}x{} x {} mm from {} to {},,,", t.name, t.id.clone().unwrap_or_default(), t.width_mm, t.depth_mm, t.clear_mm, t.from, t.to);
        }
        o
    }

    pub fn concrete_csv(&self) -> String {
        let mut o = String::from("element,element_id,kind,volume_m3,basis\n");
        for r in &self.concrete {
            let _ = writeln!(o, "{},{},{},{},{}", r.element, r.id.clone().unwrap_or_default(), r.kind, r.volume_m3, csv_cell(&r.basis));
        }
        let _ = writeln!(o, "TOTAL,,,{},", self.concrete_total_m3);
        o
    }

    /// The bar bending schedule.
    pub fn bbs_csv(&self) -> String {
        let mut o = String::from("element,element_id,mark,role,dia_mm,number,cut_length_mm,total_m,kg,source\n");
        for b in &self.bars {
            let _ = writeln!(o, "{},{},{},{},{},{},{},{},{},{}", b.element, b.id.clone().unwrap_or_default(), b.mark, csv_cell(&b.role), b.dia_mm, b.number, b.cut_length_mm, b.total_m, b.kg, b.source);
        }
        let _ = writeln!(o, "TOTAL,,,,,,,,{},", self.steel_total_kg);
        o
    }
}
