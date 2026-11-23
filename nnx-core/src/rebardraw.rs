//! Reinforcement detail drawings (DXF R12, millimetres, drawn 1:1): the section and the elevation of each reinforced member kind,
//! from the bars the sizing report resolved for it (your template or the `rebar` design).
//!
//! Nothing here is a design value. Every size, count, diameter, spacing, cover, anchorage and hook comes from
//! [`SizingReport::details`]; this module only places them on a sheet.
//!
//! - **Column**: the section (concrete outline, tie at the cover, the longitudinal bars) and the elevation of one storey (bars with
//!   the anchorage leg below the footing top, ties at the entered spacing). The bars are the 4 corner bars plus the rest spread over
//!   the faces in proportion to their length; the drawing states the least clear spacing it measured. Fewer than 4 bars is refused,
//!   it has no rectangular tied section to draw.
//! - **Beam and tie beam**: the section (stirrup at the cover, top and bottom bars in one layer) and the elevation (bars, stirrups
//!   at the entered spacing, the hook allowance as a 90 degree leg). Bars that do not fit in one layer are refused: the layering is
//!   not decided here.
//! - **Footing**: the plan (the mesh in both directions at the entered spacing) and, when the footing thickness was entered, the
//!   section. The application does not decide which direction lies lower; the bars along x are drawn at the lower layer, and the
//!   sheet says so.
//!
//! Members whose section, bars, cover, anchorage, hook and length are identical share one detail. A member that cannot be drawn
//! is listed with the reason and is never approximated.
//!
//! Not drawn, on purpose: lap splices and joint (development) lengths. They come from the tables of SBC 304 / ACI 318, which were
//! not read for this project, and the bar schedule carries no lap, so a drawn lap would disagree with the cut lengths of `bbs`.
//! The anchorage and hook shown are the allowances you entered. The anchorage leg is drawn straight: its shape is not entered.
//!
//! Preliminary: it does not replace a licensed structural engineer.

use crate::dxf::{D, assemble};
use crate::sizing::{BeamBars, ColumnBars, MemberDetail, MeshBars, SizingReport};
use serde::Serialize;
use std::collections::BTreeMap;

const OUT: &str = "S-RBAR-OUT";
const BAR: &str = "S-RBAR-BAR";
const BAR2: &str = "S-RBAR-BAR2";
const LINK: &str = "S-RBAR-LINK";
const HID: &str = "S-RBAR-HID";
const DIM: &str = "S-RBAR-DIM";
const TXT: &str = "S-RBAR-TEXT";

const LAYERS: [(&str, i32, &str); 7] = [(OUT, 7, "CONTINUOUS"), (BAR, 1, "CONTINUOUS"), (BAR2, 5, "CONTINUOUS"), (LINK, 3, "CONTINUOUS"), (HID, 6, "DASHED"), (DIM, 2, "CONTINUOUS"), (TXT, 7, "CONTINUOUS")];

/// One detail on the sheet: the members it stands for and the views drawn.
#[derive(Debug, Clone, Serialize)]
pub struct DetailSheet {
    /// `column`, `beam`, `tie_beam` or `footing`.
    pub kind: String,
    /// The sizing marks (`COL-1`, ...) that share this detail.
    pub members: Vec<String>,
    pub title: String,
    pub views: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Skipped {
    pub members: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RebarDrawings {
    pub dxf: String,
    pub sheets: Vec<DetailSheet>,
    pub skipped: Vec<Skipped>,
    pub notes: Vec<String>,
    /// Entities written, by layer.
    pub counts: BTreeMap<String, usize>,
}

/// Drawing pen that shifts everything by the origin of the view being drawn.
struct P<'a> {
    d: &'a mut D,
    ox: f64,
    oy: f64,
    /// Text height of the block.
    th: f64,
}

impl P<'_> {
    fn line(&mut self, layer: &str, a: (f64, f64), b: (f64, f64)) {
        self.d.line(layer, (a.0 + self.ox, a.1 + self.oy), (b.0 + self.ox, b.1 + self.oy));
    }
    fn rect(&mut self, layer: &str, x0: f64, y0: f64, x1: f64, y1: f64) {
        self.d.rect(layer, x0 + self.ox, y0 + self.oy, x1 + self.ox, y1 + self.oy);
    }
    fn circle(&mut self, layer: &str, c: (f64, f64), r: f64) {
        self.d.circle(layer, (c.0 + self.ox, c.1 + self.oy), r);
    }
    fn text(&mut self, at: (f64, f64), height: f64, s: &str) {
        self.d.text(TXT, (at.0 + self.ox, at.1 + self.oy), height, s);
    }
    /// Horizontal dimension line from `x0` to `x1` at height `y`, label above it.
    fn dim_h(&mut self, x0: f64, x1: f64, y: f64, label: &str) {
        let t = self.th;
        self.line(DIM, (x0, y), (x1, y));
        for x in [x0, x1] {
            self.line(DIM, (x, y - t * 0.4), (x, y + t * 0.4));
        }
        self.text(((x0 + x1) / 2.0, y + t * 0.9), t, label);
    }
    /// Vertical dimension line at `x` from `y0` to `y1`, label to its left.
    fn dim_v(&mut self, x: f64, y0: f64, y1: f64, label: &str) {
        let t = self.th;
        self.line(DIM, (x, y0), (x, y1));
        for y in [y0, y1] {
            self.line(DIM, (x - t * 0.4, y), (x + t * 0.4, y));
        }
        let half = label.chars().count() as f64 * t * 0.3;
        self.text((x - t * 0.9 - half, (y0 + y1) / 2.0), t, label);
    }
}

fn fmt(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 { format!("{}", v as i64) } else { format!("{v:.1}") }
}

/// Text height for a block whose largest drawn dimension is `extent`.
fn text_height(extent: f64) -> f64 {
    (extent / 40.0).clamp(20.0, 100.0)
}

/// Bar centres of a rectangular tied section `b x h` whose bar centres lie `inset` inside the faces: the 4 corners, then the rest
/// spread over the faces in proportion to their length (pairs first, an odd bar on the face with the wider spacing).
fn column_bar_points(b: f64, h: f64, inset: f64, n: u32, dia: f64) -> Result<Vec<(f64, f64)>, String> {
    if n < 4 {
        return Err(format!("{n} longitudinal bars: a rectangular tied section needs at least the 4 corner bars to be drawn"));
    }
    let (w, hh) = (b - 2.0 * inset, h - 2.0 * inset);
    if w <= 0.0 || hh <= 0.0 {
        return Err(format!("cover, tie and bar leave no room in the {} x {} mm section", fmt(b), fmt(h)));
    }
    let r = n - 4;
    let (mut nx, mut ny) = (0u32, 0u32);
    for _ in 0..r / 2 {
        if w / (nx as f64 + 1.0) >= hh / (ny as f64 + 1.0) { nx += 1 } else { ny += 1 }
    }
    let (mut nx_bot, mut ny_left) = (nx, ny);
    if r % 2 == 1 {
        if w / (nx as f64 + 1.0) >= hh / (ny as f64 + 1.0) { nx_bot += 1 } else { ny_left += 1 }
    }
    let mut pts = vec![(inset, inset), (b - inset, inset), (inset, h - inset), (b - inset, h - inset)];
    for i in 1..=nx_bot {
        pts.push((inset + w * i as f64 / (nx_bot as f64 + 1.0), inset));
    }
    for i in 1..=nx {
        pts.push((inset + w * i as f64 / (nx as f64 + 1.0), h - inset));
    }
    for i in 1..=ny_left {
        pts.push((inset, inset + hh * i as f64 / (ny_left as f64 + 1.0)));
    }
    for i in 1..=ny {
        pts.push((b - inset, inset + hh * i as f64 / (ny as f64 + 1.0)));
    }
    let mut least = f64::INFINITY;
    for (i, p) in pts.iter().enumerate() {
        for q in &pts[i + 1..] {
            least = least.min(((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt());
        }
    }
    if least < dia {
        return Err(format!("{n} bars of dia {} mm overlap in the {} x {} mm section (centres {} mm apart)", fmt(dia), fmt(b), fmt(h), fmt(least)));
    }
    Ok(pts)
}

/// Bar centres of one layer of `count` bars across `width`, `inset` inside each face.
fn layer_points(width: f64, inset: f64, count: u32, dia: f64) -> Result<Vec<f64>, String> {
    let (a, z) = (inset, width - inset);
    if count == 0 {
        return Ok(Vec::new());
    }
    if count == 1 {
        return Ok(vec![width / 2.0]);
    }
    if z <= a {
        return Err(format!("no room for {count} bars across {} mm after cover and stirrup", fmt(width)));
    }
    let step = (z - a) / (count as f64 - 1.0);
    if step < dia {
        return Err(format!("{count} bars of dia {} mm do not fit in one layer across {} mm (centres {} mm apart): the layering is not decided here", fmt(dia), fmt(width), fmt(step)));
    }
    Ok((0..count).map(|i| a + step * i as f64).collect())
}

fn draw_column(d: &mut D, ox: f64, m: &MemberDetail, t: &ColumnBars) -> Result<(f64, f64, Vec<String>), String> {
    let (b, h, c) = (m.b_mm, m.h_mm, m.cover_mm);
    let storey = m.length_mm.ok_or("the storey height is not known")?;
    let inset = c + t.tie_dia_mm + t.bar_dia_mm / 2.0;
    let pts = column_bar_points(b, h, inset, t.count, t.bar_dia_mm)?;
    if c + t.tie_dia_mm >= b.min(h) / 2.0 {
        return Err(format!("cover {} and tie dia {} leave no core in the {} x {} mm section", fmt(c), fmt(t.tie_dia_mm), fmt(b), fmt(h)));
    }
    let mut least = f64::INFINITY;
    for (i, p) in pts.iter().enumerate() {
        for q in &pts[i + 1..] {
            least = least.min(((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt());
        }
    }
    let th = text_height(storey.max(b).max(h));
    let mut p = P { d, ox, oy: 0.0, th };
    // Section.
    p.rect(OUT, 0.0, 0.0, b, h);
    p.rect(LINK, c + t.tie_dia_mm / 2.0, c + t.tie_dia_mm / 2.0, b - c - t.tie_dia_mm / 2.0, h - c - t.tie_dia_mm / 2.0);
    for q in &pts {
        p.circle(BAR, *q, t.bar_dia_mm / 2.0);
    }
    p.dim_h(0.0, b, -th * 2.0, &fmt(b));
    p.dim_v(-th * 2.0, 0.0, h, &fmt(h));
    p.text((b / 2.0, h + th * 2.0), th, &format!("SECTION {} x {}", fmt(b), fmt(h)));
    p.text((b / 2.0, h + th * 3.6), th, &format!("{} %%c{} + ties %%c{} @ {} (cover {})", t.count, fmt(t.bar_dia_mm), fmt(t.tie_dia_mm), fmt(t.tie_spacing_mm), fmt(c)));
    p.text((b / 2.0, h + th * 5.2), th, &format!("least clear spacing {} mm", fmt(least - t.bar_dia_mm)));
    // Elevation of one storey, to the right of the section.
    let ex = b + th * 12.0 + b;
    let a = m.anchorage_mm;
    p.rect(OUT, ex, 0.0, ex + b, storey);
    p.line(HID, (ex - b * 0.5, 0.0), (ex + b * 1.5, 0.0));
    p.text((ex + b / 2.0, -a - th * 2.0), th, "top of footing: anchorage leg below it, drawn straight");
    for x in [ex + inset, ex + b - inset] {
        p.line(BAR, (x, -a), (x, storey));
    }
    let n = if t.tie_spacing_mm > 0.0 { (storey / t.tie_spacing_mm).floor() as u64 + 1 } else { 0 };
    for i in 0..n {
        let y = i as f64 * t.tie_spacing_mm;
        p.line(LINK, (ex + c + t.tie_dia_mm / 2.0, y), (ex + b - c - t.tie_dia_mm / 2.0, y));
    }
    p.dim_v(ex + b + th * 2.0, 0.0, storey, &format!("storey {}", fmt(storey)));
    p.dim_v(ex + b + th * 2.0, -a, 0.0, &format!("anchorage {}", fmt(a)));
    p.text((ex + b / 2.0, storey + th * 2.0), th, &format!("ELEVATION: {n} ties @ {}", fmt(t.tie_spacing_mm)));
    let width = ex + b - ox + th * 8.0;
    Ok((width, storey + a + th * 8.0, vec!["section".into(), "elevation".into()]))
}

fn draw_beam(d: &mut D, ox: f64, m: &MemberDetail, t: &BeamBars) -> Result<(f64, f64, Vec<String>), String> {
    let (b, h, c) = (m.b_mm, m.h_mm, m.cover_mm);
    let len = m.length_mm.ok_or("the beam length is not known")?;
    if c + t.stirrup_dia_mm >= b.min(h) / 2.0 {
        return Err(format!("cover {} and stirrup dia {} leave no core in the {} x {} mm section", fmt(c), fmt(t.stirrup_dia_mm), fmt(b), fmt(h)));
    }
    let top_y = h - (c + t.stirrup_dia_mm + t.top_dia_mm / 2.0);
    let bot_y = c + t.stirrup_dia_mm + t.bottom_dia_mm / 2.0;
    if t.top_count > 0 && t.bottom_count > 0 && top_y - t.top_dia_mm / 2.0 < bot_y + t.bottom_dia_mm / 2.0 {
        return Err(format!("top and bottom bars overlap in the {} mm depth", fmt(h)));
    }
    let top_x = layer_points(b, c + t.stirrup_dia_mm + t.top_dia_mm / 2.0, t.top_count, t.top_dia_mm)?;
    let bot_x = layer_points(b, c + t.stirrup_dia_mm + t.bottom_dia_mm / 2.0, t.bottom_count, t.bottom_dia_mm)?;
    let th = text_height(len.max(h));
    let mut p = P { d, ox, oy: 0.0, th };
    // Section.
    p.rect(OUT, 0.0, 0.0, b, h);
    p.rect(LINK, c + t.stirrup_dia_mm / 2.0, c + t.stirrup_dia_mm / 2.0, b - c - t.stirrup_dia_mm / 2.0, h - c - t.stirrup_dia_mm / 2.0);
    for x in &top_x {
        p.circle(BAR, (*x, top_y), t.top_dia_mm / 2.0);
    }
    for x in &bot_x {
        p.circle(BAR, (*x, bot_y), t.bottom_dia_mm / 2.0);
    }
    p.dim_h(0.0, b, -th * 2.0, &fmt(b));
    p.dim_v(-th * 2.0, 0.0, h, &fmt(h));
    p.text((b / 2.0, h + th * 2.0), th, &format!("SECTION {} x {}", fmt(b), fmt(h)));
    p.text((b / 2.0, h + th * 3.6), th, &format!("top {} %%c{}, bottom {} %%c{}, stirrups %%c{} @ {} (cover {})", t.top_count, fmt(t.top_dia_mm), t.bottom_count, fmt(t.bottom_dia_mm), fmt(t.stirrup_dia_mm), fmt(t.stirrup_spacing_mm), fmt(c)));
    // Elevation, below the section: its length axis runs left to right.
    let ex = b + th * 12.0 + b;
    p.rect(OUT, ex, 0.0, ex + len, h);
    if t.top_count > 0 {
        p.line(BAR, (ex + c, top_y), (ex + len - c, top_y));
    }
    if t.bottom_count > 0 {
        p.line(BAR, (ex + c, bot_y), (ex + len - c, bot_y));
    }
    let mut notes = vec!["section".to_string(), "elevation".to_string()];
    if m.hook_mm > 0.0 {
        let room_top = top_y - t.top_dia_mm / 2.0 - (bot_y + t.bottom_dia_mm / 2.0);
        if m.hook_mm < room_top / 2.0 {
            for x in [ex + c, ex + len - c] {
                if t.top_count > 0 {
                    p.line(BAR, (x, top_y), (x, top_y - m.hook_mm));
                }
                if t.bottom_count > 0 {
                    p.line(BAR, (x, bot_y), (x, bot_y + m.hook_mm));
                }
            }
            p.text((ex + len / 2.0, -th * 2.0), th, &format!("hook allowance {} drawn as a 90 deg leg at each bar end", fmt(m.hook_mm)));
        } else {
            notes.push("hook legs not drawn: the allowance is longer than half the room between the bar layers".into());
        }
    }
    let n = if t.stirrup_spacing_mm > 0.0 { ((len - 2.0 * c) / t.stirrup_spacing_mm).floor() as u64 + 1 } else { 0 };
    for i in 0..n {
        let x = ex + c + i as f64 * t.stirrup_spacing_mm;
        p.line(LINK, (x, c + t.stirrup_dia_mm / 2.0), (x, h - c - t.stirrup_dia_mm / 2.0));
    }
    p.dim_h(ex, ex + len, h + th * 2.0, &format!("length {}", fmt(len)));
    p.text((ex + len / 2.0, h + th * 4.0), th, &format!("ELEVATION: {n} stirrups @ {}", fmt(t.stirrup_spacing_mm)));
    let width = ex + len - ox + th * 6.0;
    Ok((width, h + th * 8.0, notes))
}

fn draw_footing(d: &mut D, ox: f64, m: &MemberDetail, t: &MeshBars) -> Result<(f64, f64, Vec<String>), String> {
    let (l, w, c) = (m.b_mm, m.h_mm, m.cover_mm);
    if t.spacing_mm <= 0.0 {
        return Err("the mesh spacing is not above zero".into());
    }
    let th = text_height(l.max(w));
    let mut p = P { d, ox, oy: 0.0, th };
    // Plan.
    p.rect(OUT, 0.0, 0.0, l, w);
    let ny = ((w - 2.0 * c) / t.spacing_mm).floor() as u64 + 1;
    let nx = ((l - 2.0 * c) / t.spacing_mm).floor() as u64 + 1;
    for j in 0..ny {
        let y = c + j as f64 * t.spacing_mm;
        p.line(BAR, (c, y), (l - c, y));
    }
    for j in 0..nx {
        let x = c + j as f64 * t.spacing_mm;
        p.line(BAR2, (x, c), (x, w - c));
    }
    p.dim_h(0.0, l, -th * 2.0, &fmt(l));
    p.dim_v(-th * 2.0, 0.0, w, &fmt(w));
    p.text((l / 2.0, w + th * 2.0), th, &format!("PLAN {} x {}: %%c{} @ {} each way (cover {})", fmt(l), fmt(w), fmt(t.bar_dia_mm), fmt(t.spacing_mm), fmt(c)));
    p.text((l / 2.0, w + th * 3.6), th, &format!("bars along x: {ny} (red), bars along y: {nx} (blue)"));
    let mut views = vec!["plan".to_string()];
    let mut height = w + th * 8.0;
    let mut width = l + th * 8.0;
    // Section, to the right, when the thickness was entered.
    match m.length_mm {
        Some(thick) if 2.0 * c + 2.0 * t.bar_dia_mm <= thick => {
            let sx = l + th * 12.0;
            p.rect(OUT, sx, 0.0, sx + l, thick);
            // The application does not decide the layer order: the bars along x are drawn lower.
            p.line(BAR, (sx + c, c + t.bar_dia_mm / 2.0), (sx + l - c, c + t.bar_dia_mm / 2.0));
            for j in 0..nx {
                p.circle(BAR2, (sx + c + j as f64 * t.spacing_mm, c + t.bar_dia_mm * 1.5), t.bar_dia_mm / 2.0);
            }
            p.dim_v(sx + l + th * 2.0, 0.0, thick, &format!("thickness {}", fmt(thick)));
            p.text((sx + l / 2.0, thick + th * 2.0), th, "SECTION: layer order is not decided by the application; bars along x drawn lower");
            views.push("section".into());
            width = sx + l - ox + th * 8.0;
            height = height.max(thick + th * 6.0);
        }
        Some(thick) => return Err(format!("thickness {} mm cannot hold two bar layers of dia {} mm with {} mm cover", fmt(thick), fmt(t.bar_dia_mm), fmt(c))),
        None => views.push("section not drawn: footing_thickness_mm was not entered".into()),
    }
    Ok((width, height, views))
}

/// The detail drawings of every reinforced member of the report. Identical members share one detail; a member that cannot be drawn
/// is listed with its reason. An empty report is an error that says why, never a blank sheet.
pub fn build(rep: &SizingReport, title: &str) -> Result<RebarDrawings, String> {
    if rep.details.is_empty() {
        let why = rep.missing.get("reinforcement").map(|m| m.join(", ")).unwrap_or_else(|| "the sizing report holds no reinforced member".into());
        return Err(format!("nothing to draw: {why}"));
    }
    // Group the members that share every drawn value.
    let mut groups: Vec<(String, Vec<&MemberDetail>)> = Vec::new();
    for m in &rep.details {
        let mut z = m.clone();
        z.name.clear();
        z.id = None;
        z.source.clear();
        let key = format!("{z:?}");
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(m),
            None => groups.push((key, vec![m])),
        }
    }
    let mut d = D::new();
    let mut out = RebarDrawings::default();
    let (mut cursor, mut max_h) = (0.0_f64, 0.0_f64);
    let mut min_y = 0.0_f64;
    for (_, g) in &groups {
        let m = g[0];
        let names: Vec<String> = g.iter().map(|x| x.name.clone()).collect();
        let drawn = match (m.kind.as_str(), &m.column, &m.mesh, &m.beam) {
            ("column", Some(t), _, _) => draw_column(&mut d, cursor, m, t),
            ("footing", _, Some(t), _) => draw_footing(&mut d, cursor, m, t),
            ("beam" | "tie_beam", _, _, Some(t)) => draw_beam(&mut d, cursor, m, t),
            (k, ..) => Err(format!("member kind '{k}' has no detail drawing")),
        };
        match drawn {
            Ok((width, height, views)) => {
                let th = text_height(width.max(height) / 3.0);
                let summary = format!("{} | {} | source: {}", names.join(", "), m.kind.replace('_', " "), g.iter().map(|x| x.source.as_str()).collect::<std::collections::BTreeSet<_>>().into_iter().collect::<Vec<_>>().join("/"));
                d.text(TXT, (cursor + width / 2.0, height + th * 2.0), th, &summary);
                out.sheets.push(DetailSheet { kind: m.kind.clone(), members: names, title: summary, views: views.clone() });
                for v in views.iter().filter(|v| v.starts_with("hook") || v.contains("not drawn")) {
                    out.notes.push(format!("{}: {v}", g.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(", ")));
                }
                max_h = max_h.max(height + th * 4.0);
                min_y = min_y.min(-m.anchorage_mm - 400.0);
                cursor += width + 3000.0;
            }
            Err(reason) => out.skipped.push(Skipped { members: names, reason }),
        }
    }
    if out.sheets.is_empty() {
        return Err(format!("no member could be drawn: {}", out.skipped.iter().map(|s| format!("{}: {}", s.members.join(", "), s.reason)).collect::<Vec<_>>().join("; ")));
    }
    d.text(TXT, (cursor / 2.0, min_y - 600.0), 100.0, &format!("{title} - reinforcement details, mm, drawn 1:1"));
    d.text(TXT, (cursor / 2.0, min_y - 900.0), 70.0, "PRELIMINARY - NOT FOR CONSTRUCTION - bars are those of your templates or of the rebar design");
    out.notes.push("lap splices and joint development lengths are not drawn: they come from SBC 304 / ACI 318 tables that were not read, and the bar schedule carries no lap".into());
    out.notes.push("anchorage and hook are the allowances you entered; the anchorage leg is drawn straight because its shape is not entered".into());
    let layers = LAYERS.iter().map(|(n, c, l)| (n.to_string(), *c, *l)).collect();
    out.dxf = assemble(&d, layers, (-1500.0, min_y - 1200.0, cursor, max_h + 600.0));
    out.counts = d.counts.clone();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, b: f64, count: u32) -> MemberDetail {
        MemberDetail { name: name.into(), id: Some("1".into()), kind: "column".into(), b_mm: b, h_mm: b, length_mm: Some(3000.0), cover_mm: 40.0, anchorage_mm: 300.0, hook_mm: 100.0, source: "template".into(), column: Some(ColumnBars { bar_dia_mm: 16.0, count, tie_dia_mm: 8.0, tie_spacing_mm: 150.0 }), mesh: None, beam: None }
    }

    fn beam(name: &str, top: u32, bottom: u32) -> MemberDetail {
        MemberDetail { name: name.into(), id: None, kind: "beam".into(), b_mm: 300.0, h_mm: 600.0, length_mm: Some(5000.0), cover_mm: 40.0, anchorage_mm: 300.0, hook_mm: 100.0, source: "design".into(), column: None, mesh: None, beam: Some(BeamBars { top_dia_mm: 16.0, top_count: top, bottom_dia_mm: 20.0, bottom_count: bottom, stirrup_dia_mm: 10.0, stirrup_spacing_mm: 150.0 }) }
    }

    fn footing(name: &str, thickness: Option<f64>) -> MemberDetail {
        MemberDetail { name: name.into(), id: None, kind: "footing".into(), b_mm: 1800.0, h_mm: 1800.0, length_mm: thickness, cover_mm: 75.0, anchorage_mm: 300.0, hook_mm: 100.0, source: "template".into(), column: None, mesh: Some(MeshBars { bar_dia_mm: 14.0, spacing_mm: 150.0 }), beam: None }
    }

    fn report(details: Vec<MemberDetail>) -> SizingReport {
        SizingReport { details, ..Default::default() }
    }

    #[test]
    fn identical_members_share_one_detail_and_the_file_is_a_dxf() {
        let r = build(&report(vec![col("COL-1", 300.0, 8), col("COL-2", 300.0, 8), col("COL-3", 400.0, 8)]), "Villa").unwrap();
        assert_eq!(r.sheets.len(), 2);
        assert_eq!(r.sheets[0].members, vec!["COL-1".to_string(), "COL-2".to_string()]);
        assert!(r.dxf.starts_with("0\nSECTION") && r.dxf.ends_with("0\nEOF\n") && r.dxf.contains("S-RBAR-LINK"));
        assert!(r.dxf.contains("PRELIMINARY - NOT FOR CONSTRUCTION") && r.dxf.contains("%%c16"));
        assert!(r.notes.iter().any(|n| n.contains("lap splices")));
    }

    #[test]
    fn column_bars_are_counted_exactly_and_ties_follow_the_entered_spacing() {
        let r = build(&report(vec![col("COL-1", 300.0, 8)]), "Villa").unwrap();
        // 8 section circles; ties: floor(3000 / 150) + 1 = 21 lines in the elevation, plus the section tie rectangle.
        let circles = r.dxf.matches("\n0\nCIRCLE\n").count() + usize::from(r.dxf.starts_with("0\nCIRCLE"));
        assert_eq!(circles, 8);
        assert_eq!(r.counts.get("S-RBAR-BAR"), Some(&(8 + 2)));
        assert_eq!(r.counts.get("S-RBAR-LINK"), Some(&(1 + 21)));
    }

    #[test]
    fn eight_bars_in_a_square_section_sit_on_the_corners_and_the_face_middles() {
        let pts = column_bar_points(300.0, 300.0, 64.0, 8, 16.0).unwrap();
        assert_eq!(pts.len(), 8);
        assert!(pts.iter().any(|p| (p.0 - 150.0).abs() < 1e-9 && (p.1 - 64.0).abs() < 1e-9));
        assert!(pts.iter().any(|p| (p.0 - 64.0).abs() < 1e-9 && (p.1 - 150.0).abs() < 1e-9));
        // An odd count still places exactly that many bars.
        assert_eq!(column_bar_points(500.0, 300.0, 64.0, 9, 16.0).unwrap().len(), 9);
    }

    #[test]
    fn a_section_that_cannot_hold_its_bars_is_listed_with_the_reason_not_drawn() {
        assert!(column_bar_points(300.0, 300.0, 64.0, 3, 16.0).unwrap_err().contains("at least the 4"));
        let crowded = build(&report(vec![col("COL-1", 300.0, 60), col("COL-2", 300.0, 8)]), "Villa").unwrap();
        assert_eq!(crowded.sheets.len(), 1);
        assert_eq!(crowded.skipped.len(), 1);
        assert_eq!(crowded.skipped[0].members, vec!["COL-1".to_string()]);
        assert!(crowded.skipped[0].reason.contains("overlap"));
    }

    #[test]
    fn beam_bars_that_do_not_fit_one_layer_are_refused_and_stirrups_are_counted() {
        let ok = build(&report(vec![beam("BM-1", 2, 3)]), "Villa").unwrap();
        // floor((5000 - 80) / 150) + 1 = 33 stirrups plus the section rectangle.
        assert_eq!(ok.counts.get("S-RBAR-LINK"), Some(&(1 + 33)));
        let err = build(&report(vec![beam("BM-2", 2, 12)]), "Villa").unwrap_err();
        assert!(err.contains("do not fit in one layer"), "{err}");
    }

    #[test]
    fn a_footing_section_needs_the_entered_thickness() {
        let without = build(&report(vec![footing("FND-1", None)]), "Villa").unwrap();
        assert_eq!(without.sheets[0].views[0], "plan");
        assert!(without.notes.iter().any(|n| n.contains("footing_thickness_mm")));
        let with = build(&report(vec![footing("FND-1", Some(500.0))]), "Villa").unwrap();
        assert_eq!(with.sheets[0].views, vec!["plan".to_string(), "section".to_string()]);
        // Mesh: floor((1800 - 150) / 150) + 1 = 12 lines each way, plus the section's x line.
        assert_eq!(with.counts.get("S-RBAR-BAR"), Some(&(12 + 1)));
        assert!(build(&report(vec![footing("FND-2", Some(100.0))]), "Villa").unwrap_err().contains("cannot hold two bar layers"));
    }

    #[test]
    fn an_empty_report_says_why() {
        let mut rep = SizingReport::default();
        rep.missing.insert("reinforcement".into(), vec!["rebar.anchorage_mm".into()]);
        assert!(build(&rep, "Villa").unwrap_err().contains("rebar.anchorage_mm"));
        assert!(build(&SizingReport::default(), "Villa").unwrap_err().contains("no reinforced member"));
    }
}
