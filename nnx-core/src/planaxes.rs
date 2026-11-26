//! Structural axes of a raster plan: where the axis bubbles are, and what they say about the drawing's scale and position.
//!
//! A plan sheet draws its axes as lines ending in bubbles (a ring with a label). The labels are not read here; what the program uses is
//! that bubbles stand at known distances from each other, which the owner types from the drawing's dimension chain:
//!
//! - **`find_bubbles`** finds the circles (the ring must be closed all round; which of them are axis bubbles is for `fit_sheet` to say, by the axes the owner gave): edges are taken from the colour difference between neighbours (so a light grey ring on a beige sheet
//!   still shows), and each edge pixel votes along its gradient for the centres of circles of each radius in the range.
//! - **`fit_axis`** lays the bubbles of one side (all in a row or a column) over the axes the owner gave. It tries every way two bubbles can be
//!   two of the axes, keeps the one that explains most bubbles, and refines it by least squares. A bubble the drawing does not show (a crop) or
//!   the finder missed is no harm; a bubble that fits no axis is not counted. The fit gives the scale and the position at once, and the same
//!   axes in every drawing put every drawing in the same coordinates.
//! - **`fit_scale`** takes the two sides together: pixels are square, so the scale from the columns and the scale from the rows must agree.
//!   When several fits explain the bubbles about equally well, the pair whose scales agree is taken; if there is no pair, or more than one, the
//!   answer is an error that says so, never a guess.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Bubble {
    pub x: f64,
    pub y: f64,
    pub radius: f64,
    /// Votes the circle collected per unit of circumference: about 1 for a disc, up to 2 for a ring with two clear edges.
    pub support: f64,
}

/// One axis: a name for the report and its position along the side, in millimetres from the origin the owner chose.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Axis {
    pub name: String,
    pub pos_mm: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AxisFit {
    /// Millimetres per pixel along this side.
    pub mm_per_px: f64,
    /// World position (mm) of pixel coordinate 0 on this side: `world = offset_mm + px * mm_per_px`.
    pub offset_mm: f64,
    /// Bubbles explained, each with the axis it was taken for and how far (px) it lies from where that axis should be.
    pub matched: Vec<(f64, String, f64)>,
    /// Bubbles of this side that fit no axis.
    pub unexplained: Vec<f64>,
    pub rms_px: f64,
}

/// Bounds the scale may take. A plan is a drawing of a building: a pixel is a few centimetres, not metres and not tenths of a millimetre.
pub const MIN_MM_PER_PX: f64 = 5.0;
pub const MAX_MM_PER_PX: f64 = 400.0;
/// A bubble is taken for an axis when it lies within this many pixels of it.
pub const MATCH_PX: f64 = 3.0;

fn check(rgb: &[u8], width: u32, height: u32) -> Result<(usize, usize), String> {
    let (w, h) = (width as usize, height as usize);
    if w < 16 || h < 16 {
        return Err(format!("the image is {w} x {h} pixels: too small to hold a plan"));
    }
    if rgb.len() != w * h * 3 {
        return Err(format!("the bitmap has {} bytes, but {w} x {h} RGB pixels need {}", rgb.len(), w * h * 3));
    }
    Ok((w, h))
}

/// The circles of radius `min_r..=max_r` pixels (a ring or a disc) in the picture, strongest first, none closer to another than its radius.
pub fn find_bubbles(rgb: &[u8], width: u32, height: u32, min_r: u32, max_r: u32) -> Result<Vec<Bubble>, String> {
    let (w, h) = check(rgb, width, height)?;
    if min_r < 3 || max_r < min_r || max_r > 200 {
        return Err(format!("the bubble radius range {min_r}..{max_r} px is not usable: give 3 or more, and a maximum that is not below the minimum"));
    }
    // Edge strength and direction from the colour difference of the pixels either side: the largest of the three channels, so a pale ring
    // on a pale sheet counts as much as its colour difference allows.
    let at = |x: usize, y: usize, c: usize| f64::from(rgb[(y * w + x) * 3 + c]);
    let mut edges: Vec<(usize, usize, f64, f64)> = Vec::new();
    let mut edge_mask = vec![false; w * h];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let (mut best, mut gx, mut gy) = (0.0f64, 0.0, 0.0);
            for c in 0..3 {
                let dx = at(x + 1, y, c) - at(x - 1, y, c);
                let dy = at(x, y + 1, c) - at(x, y - 1, c);
                let m = dx.hypot(dy);
                if m > best {
                    (best, gx, gy) = (m, dx / m, dy / m);
                }
            }
            if best >= EDGE_MIN {
                edges.push((x, y, gx, gy));
                edge_mask[y * w + x] = true;
            }
        }
    }
    let layers = (max_r - min_r + 1) as usize;
    let mut acc = vec![0u16; layers * w * h];
    for &(x, y, gx, gy) in &edges {
        for (li, r) in (min_r..=max_r).enumerate() {
            for sign in [-1.0, 1.0] {
                let (cx, cy) = (x as f64 + sign * f64::from(r) * gx, y as f64 + sign * f64::from(r) * gy);
                let (ix, iy) = (cx.round(), cy.round());
                if ix >= 0.0 && iy >= 0.0 && (ix as usize) < w && (iy as usize) < h {
                    let a = &mut acc[li * w * h + iy as usize * w + ix as usize];
                    *a = a.saturating_add(1);
                }
            }
        }
    }
    // The votes of one circle land in neighbouring cells (the centre is rounded) and, for a thin ring, in neighbouring radii (its two edges are
    // a pixel or two apart). So each layer is summed over 3 x 3 cells, then over the layers within two of the radius, before a peak is looked for.
    let sum3 = |layer: &[u16]| -> Vec<u16> {
        let mut o = vec![0u16; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut t = 0u32;
                for yy in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                    for xx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                        t += u32::from(layer[yy * w + xx]);
                    }
                }
                o[y * w + x] = t.min(u32::from(u16::MAX)) as u16;
            }
        }
        o
    };
    let blurred: Vec<Vec<u16>> = (0..layers).map(|li| sum3(&acc[li * w * h..(li + 1) * w * h])).collect();
    let mut found: Vec<Bubble> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let mut best: Option<(f64, f64)> = None;
            for (li, r) in (min_r..=max_r).enumerate() {
                let total: u32 = (li.saturating_sub(2)..=(li + 2).min(layers - 1)).map(|k| u32::from(blurred[k][y * w + x])).sum();
                let support = f64::from(total) / (2.0 * std::f64::consts::PI * f64::from(r));
                if support >= SUPPORT_MIN && best.is_none_or(|b| support > b.0) {
                    best = Some((support, f64::from(r)));
                }
            }
            // A corner of two walls or a crossing of lines also collects votes; a bubble is a closed ring, so its edge must be there all round.
            if let Some((support, radius)) = best
                && ring_coverage(&edge_mask, w, h, x as f64, y as f64, radius) >= COVERAGE_MIN
            {
                found.push(Bubble { x: x as f64, y: y as f64, radius, support });
            }
        }
    }
    found.sort_by(|a, b| b.support.total_cmp(&a.support));
    let mut out: Vec<Bubble> = Vec::new();
    for b in found {
        if !out.iter().any(|o| (o.x - b.x).hypot(o.y - b.y) < o.radius.max(b.radius)) {
            out.push(b);
        }
    }
    Ok(out)
}

/// Share of the directions around a candidate centre (24 of them) in which an edge lies at the radius, give or take two pixels.
fn ring_coverage(edge: &[bool], w: usize, h: usize, cx: f64, cy: f64, r: f64) -> f64 {
    const DIRECTIONS: usize = 24;
    let mut hits = 0;
    for k in 0..DIRECTIONS {
        let a = k as f64 * std::f64::consts::TAU / DIRECTIONS as f64;
        let on = (-2i32..=2).any(|d| {
            let rr = r + f64::from(d);
            let (x, y) = ((cx + rr * a.cos()).round(), (cy + rr * a.sin()).round());
            x >= 0.0 && y >= 0.0 && (x as usize) < w && (y as usize) < h && edge[y as usize * w + x as usize]
        });
        hits += usize::from(on);
    }
    hits as f64 / DIRECTIONS as f64
}

/// A bubble's edge must be present in at least this share of the directions around it.
const COVERAGE_MIN: f64 = 0.8;

/// Smallest colour difference between neighbours that is an edge.
const EDGE_MIN: f64 = 14.0;
/// Votes, per unit of circumference, that a circle must collect to count: about 1 for the edge of a disc, up to 2 for a ring (two edges).
const SUPPORT_MIN: f64 = 0.55;

fn check_axes(axes: &[Axis]) -> Result<(), String> {
    if axes.len() < 2 {
        return Err("give at least two axes with their distance: a scale needs a known distance".into());
    }
    if axes.windows(2).any(|p| p[1].pos_mm <= p[0].pos_mm) {
        return Err("the axes must be listed in increasing position, each farther than the one before".into());
    }
    Ok(())
}

/// Lays `positions_px` (the bubble coordinates along one side) over `axes` and returns the fit that explains most of them.
/// `scale_hint` is `None` to let the bubbles decide within the bounds, or `Some(mm_per_px)` when the other side already fixed it.
pub fn fit_axis(positions_px: &[f64], axes: &[Axis], scale_hint: Option<f64>) -> Result<Vec<AxisFit>, String> {
    check_axes(axes)?;
    if positions_px.len() < 2 {
        return Err(format!("only {} bubble(s) were found along this side: two are needed to fix a scale", positions_px.len()));
    }
    let mut p: Vec<f64> = positions_px.to_vec();
    p.sort_by(f64::total_cmp);
    let mut fits: Vec<AxisFit> = Vec::new();
    for i in 0..p.len() {
        for j in i + 1..p.len() {
            for a in 0..axes.len() {
                for b in a + 1..axes.len() {
                    let s = (axes[b].pos_mm - axes[a].pos_mm) / (p[j] - p[i]);
                    if !(MIN_MM_PER_PX..=MAX_MM_PER_PX).contains(&s) || scale_hint.is_some_and(|h| (s - h).abs() > 0.03 * h) {
                        continue;
                    }
                    let off = axes[a].pos_mm - s * p[i];
                    fits.push(refine(&p, axes, s, off));
                }
            }
        }
    }
    // Most bubbles explained first, then the smallest residual. Equivalent fits (the same pairing found from another seed) are one.
    fits.sort_by(|x, y| y.matched.len().cmp(&x.matched.len()).then(x.rms_px.total_cmp(&y.rms_px)));
    let mut out: Vec<AxisFit> = Vec::new();
    for f in fits {
        if !out.iter().any(|o| (o.mm_per_px - f.mm_per_px).abs() < 0.01 * f.mm_per_px && (o.offset_mm - f.offset_mm).abs() < 2.0 * f.mm_per_px && o.matched.len() == f.matched.len()) {
            out.push(f);
        }
    }
    if out.is_empty() {
        return Err("no way of laying the bubbles over the axes keeps the scale within the allowed range".into());
    }
    // Any two points fit any two axes, so a fit counts only if it explains more of the bubbles than it leaves out: a circle that is no axis
    // may be found, but not more of them than the real ones.
    out.retain(|f| f.matched.len() >= 2 && f.matched.len() > f.unexplained.len());
    if out.is_empty() {
        return Err("the bubbles found do not fit the axes given: most of them stand where no axis is. Check the axis distances, or that the bubbles were found on the right side".into());
    }
    Ok(out)
}

/// Matches every bubble to the nearest axis under `world = off + s * px`, then fits scale and offset to those pairs by least squares (twice).
fn refine(p: &[f64], axes: &[Axis], mut s: f64, mut off: f64) -> AxisFit {
    let mut pairs: Vec<(f64, usize)> = Vec::new();
    for _ in 0..2 {
        pairs.clear();
        for &px in p {
            let world = off + s * px;
            if let Some((k, d)) = axes.iter().enumerate().map(|(k, a)| (k, (a.pos_mm - world).abs() / s)).min_by(|x, y| x.1.total_cmp(&y.1))
                && d <= MATCH_PX
                && !pairs.iter().any(|&(_, kk)| kk == k)
            {
                pairs.push((px, k));
            }
        }
        if pairs.len() >= 2 {
            let n = pairs.len() as f64;
            let (sx, sy): (f64, f64) = pairs.iter().fold((0.0, 0.0), |a, &(x, k)| (a.0 + x, a.1 + axes[k].pos_mm));
            let (mx, my) = (sx / n, sy / n);
            let (sxx, sxy): (f64, f64) = pairs.iter().fold((0.0, 0.0), |a, &(x, k)| (a.0 + (x - mx).powi(2), a.1 + (x - mx) * (axes[k].pos_mm - my)));
            if sxx > 0.0 {
                s = sxy / sxx;
                off = my - s * mx;
            }
        }
    }
    let matched: Vec<(f64, String, f64)> = pairs.iter().map(|&(x, k)| (x, axes[k].name.clone(), ((off + s * x) - axes[k].pos_mm) / s)).collect();
    let rms = if matched.is_empty() { f64::INFINITY } else { (matched.iter().map(|m| m.2 * m.2).sum::<f64>() / matched.len() as f64).sqrt() };
    let explained: Vec<f64> = pairs.iter().map(|p| p.0).collect();
    AxisFit { mm_per_px: s, offset_mm: off, matched, unexplained: p.iter().copied().filter(|x| !explained.contains(x)).collect(), rms_px: rms }
}

/// Both sides of one drawing, with the scale they agree on.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SheetFit {
    pub x: AxisFit,
    pub y: AxisFit,
    /// The scale both sides agree on.
    pub mm_per_px: f64,
    /// Where pixel 0 stands on each side when both use that one scale: `world = offset_mm + px * mm_per_px`.
    pub x_offset_mm: f64,
    pub y_offset_mm: f64,
    pub notes: Vec<String>,
}

/// Fits the columns (`xs`, bubble x positions) and the rows (`ys`) of one drawing so that both sides give the same scale. A side with
/// only one candidate is taken as it is; otherwise the pair of candidates whose scales differ by no more than `AGREE` is taken.
pub fn fit_scale(xs: &[f64], x_axes: &[Axis], ys: &[f64], y_axes: &[Axis]) -> Result<SheetFit, String> {
    let fx = fit_axis(xs, x_axes, None).map_err(|e| format!("columns: {e}"))?;
    let fy = fit_axis(ys, y_axes, None).map_err(|e| format!("rows: {e}"))?;
    let top = |v: &[AxisFit]| v.iter().take_while(|f| f.matched.len() >= v[0].matched.len().saturating_sub(1) && f.matched.len() >= 2).cloned().collect::<Vec<_>>();
    let (cx, cy) = (top(&fx), top(&fy));
    let mut pairs: Vec<(AxisFit, AxisFit)> = Vec::new();
    for a in &cx {
        for b in &cy {
            if (a.mm_per_px - b.mm_per_px).abs() <= AGREE * a.mm_per_px.max(b.mm_per_px) {
                pairs.push((a.clone(), b.clone()));
            }
        }
    }
    pairs.sort_by(|p, q| (q.0.matched.len() + q.1.matched.len()).cmp(&(p.0.matched.len() + p.1.matched.len())).then((p.0.rms_px + p.1.rms_px).total_cmp(&(q.0.rms_px + q.1.rms_px))));
    let Some((a, b)) = pairs.first().cloned() else {
        return Err(format!(
            "the columns and the rows do not give the same scale: columns {:?} mm/px, rows {:?} mm/px. Check the axis distances and that the right bubbles were found",
            cx.iter().map(|f| (f.mm_per_px * 10.0).round() / 10.0).collect::<Vec<_>>(),
            cy.iter().map(|f| (f.mm_per_px * 10.0).round() / 10.0).collect::<Vec<_>>()
        ));
    };
    if let Some(second) = pairs.get(1)
        && second.0.matched.len() + second.1.matched.len() == a.matched.len() + b.matched.len()
        && (second.0.rms_px + second.1.rms_px - (a.rms_px + b.rms_px)).abs() < 0.2
        && ((second.0.mm_per_px - a.mm_per_px).abs() > 0.02 * a.mm_per_px || (second.0.offset_mm - a.offset_mm).abs() > 10.0)
    {
        return Err("more than one way of laying the bubbles over the axes explains them equally well: give more bubbles (a fuller crop) or a longer dimension chain".into());
    }
    let s = (a.mm_per_px * (a.matched.len() as f64) + b.mm_per_px * (b.matched.len() as f64)) / ((a.matched.len() + b.matched.len()) as f64);
    let mut notes = vec![format!(
        "scale {s:.2} mm per pixel from {} column bubble(s) and {} row bubble(s); residuals {:.2} px (columns) and {:.2} px (rows)",
        a.matched.len(),
        b.matched.len(),
        a.rms_px,
        b.rms_px
    )];
    for (side, f) in [("column", &a), ("row", &b)] {
        if f.matched.len() < 3 {
            notes.push(format!("only {} {side} bubble(s) fixed the {side} axis: its position is not checked by a third, so confirm it", f.matched.len()));
        }
        if !f.unexplained.is_empty() {
            notes.push(format!("{} {side} bubble(s) fit no axis and were left out", f.unexplained.len()));
        }
    }
    // Both sides now use one scale, so each side's position is refitted with that slope fixed: the mean distance of the matched bubbles.
    let at = |f: &AxisFit, axes: &[Axis]| {
        let v: Vec<f64> = f.matched.iter().filter_map(|m| axes.iter().find(|x| x.name == m.1).map(|x| x.pos_mm - s * m.0)).collect();
        v.iter().sum::<f64>() / v.len().max(1) as f64
    };
    let (x_offset_mm, y_offset_mm) = (at(&a, x_axes), at(&b, y_axes));
    Ok(SheetFit { x: a, y: b, mm_per_px: s, x_offset_mm, y_offset_mm, notes })
}

/// How far two scales may differ (as a fraction) and still be the same scale.
pub const AGREE: f64 = 0.03;

/// Bubbles of one row or column agree on that coordinate to within this many pixels.
const LINE_PX: f64 = 3.0;

/// The result of `fit_sheet`: the fit and the row and column of bubbles it was made from.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SheetResult {
    pub fit: SheetFit,
    /// The y (px) of the row of bubbles along the top (or bottom), and the x (px) of the column down a side.
    pub row_y: f64,
    pub col_x: f64,
    /// Circles that were candidates (rings of any size), and how many of them lie in the two lines used.
    pub candidates: usize,
    pub in_lines: usize,
}

/// Every set of two or more candidates that share a coordinate (within `LINE_PX`): `by_y` for rows (same y), else columns (same x).
fn lines(b: &[Bubble], by_y: bool) -> Vec<Vec<usize>> {
    let key = |c: &Bubble| if by_y { c.y } else { c.x };
    let mut out: Vec<Vec<usize>> = Vec::new();
    for c in b {
        let mut m: Vec<usize> = (0..b.len()).filter(|&k| (key(&b[k]) - key(c)).abs() <= LINE_PX).collect();
        m.sort_unstable();
        if m.len() >= 2 && !out.contains(&m) {
            out.push(m);
        }
    }
    out
}

type Candidate = (AxisFit, f64, Vec<f64>);

/// Two bubbles of neighbouring axes are drawn side by side and cannot overlap, so the nearest two matched ones stand at least this many radii
/// apart (centre to centre). Without this a run of small circles that happen to be a few pixels apart (stair treads, hatching) fits an axis
/// pattern by chance at a huge scale, where a pixel tolerance is metres.
pub const MIN_SPACING_RADII: f64 = 2.5;

fn apart_enough(f: &AxisFit, radius: f64) -> bool {
    let mut px: Vec<f64> = f.matched.iter().map(|m| m.0).collect();
    px.sort_by(f64::total_cmp);
    px.windows(2).all(|p| p[1] - p[0] >= MIN_SPACING_RADII * radius)
}

/// Which circles of a drawing are its axis bubbles, found by the axes the owner gave. Furniture, fittings and stair treads are circles too and
/// there are many more of them than axis bubbles, so size, strength and alignment cannot tell them apart; the distances can. Every row of
/// circles is laid over the columns' axes, every column over the rows' axes, and the pair whose scales agree and that explains most bubbles
/// is the answer. Circles of one line need to be of one size (within a pixel and a half). An error says how many lines were tried.
pub fn fit_sheet(candidates: &[Bubble], x_axes: &[Axis], y_axes: &[Axis]) -> Result<SheetResult, String> {
    // A fault in the axes the owner gave is the owner's to fix, and is said as such, not as "nothing lined up".
    for (side, axes) in [("columns", x_axes), ("rows", y_axes)] {
        check_axes(axes).map_err(|e| format!("{side}: {e}"))?;
    }
    let mut radii: Vec<i64> = candidates.iter().map(|c| c.radius.round() as i64).collect();
    radii.sort_unstable();
    radii.dedup();
    let mut xs_fits: Vec<Candidate> = Vec::new();
    let mut ys_fits: Vec<Candidate> = Vec::new();
    let mut tried = 0;
    for r in radii {
        let class: Vec<Bubble> = candidates.iter().filter(|c| (c.radius - r as f64).abs() <= 1.5).cloned().collect();
        for by_y in [true, false] {
            for set in lines(&class, by_y) {
                tried += 1;
                let pos: Vec<f64> = set.iter().map(|&k| if by_y { class[k].x } else { class[k].y }).collect();
                let key = set.iter().map(|&k| if by_y { class[k].y } else { class[k].x }).sum::<f64>() / set.len() as f64;
                let axes = if by_y { x_axes } else { y_axes };
                if let Ok(f) = fit_axis(&pos, axes, None)
                    && let Some(best) = f.into_iter().next()
                    && apart_enough(&best, r as f64)
                {
                    if by_y { xs_fits.push((best, key, pos)) } else { ys_fits.push((best, key, pos)) }
                }
            }
        }
    }
    let rank = |v: &mut Vec<Candidate>| {
        v.sort_by(|a, b| b.0.matched.len().cmp(&a.0.matched.len()).then(a.0.rms_px.total_cmp(&b.0.rms_px)));
        v.truncate(16);
    };
    rank(&mut xs_fits);
    rank(&mut ys_fits);
    let mut best: Option<(usize, f64, SheetResult)> = None;
    for (fx, row_y, xs) in &xs_fits {
        for (fy, col_x, ys) in &ys_fits {
            if (fx.mm_per_px - fy.mm_per_px).abs() > AGREE * fx.mm_per_px.max(fy.mm_per_px) {
                continue;
            }
            let Ok(fit) = fit_scale(xs, x_axes, ys, y_axes) else { continue };
            let score = fit.x.matched.len() + fit.y.matched.len();
            let rms = fit.x.rms_px + fit.y.rms_px;
            if best.as_ref().is_none_or(|(s0, r0, _)| score > *s0 || (score == *s0 && rms < *r0)) {
                best = Some((score, rms, SheetResult { fit, row_y: *row_y, col_x: *col_x, candidates: candidates.len(), in_lines: xs.len() + ys.len() }));
            }
        }
    }
    best.map(|b| b.2).ok_or_else(|| {
        format!(
            "none of the {} circle(s) found ({tried} row/column sets tried) lines up with the axis distances given: columns and rows must both be there, at the same scale. Check the distances, or that the axis bubbles are in the picture",
            candidates.len()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 300;
    const H: usize = 260;

    fn ring(img: &mut [u8], cx: f64, cy: f64, r: f64, c: [u8; 3]) {
        for y in 0..H {
            for x in 0..W {
                let d = (x as f64 - cx).hypot(y as f64 - cy);
                if (d - r).abs() <= 1.0 {
                    img[(y * W + x) * 3..(y * W + x) * 3 + 3].copy_from_slice(&c);
                }
            }
        }
    }

    fn sheet(top: &[f64], right: &[f64]) -> Vec<u8> {
        let mut g = vec![240u8; W * H * 3];
        for &x in top {
            ring(&mut g, x, 30.0, 14.0, [150, 160, 160]);
        }
        for &y in right {
            ring(&mut g, 260.0, y, 14.0, [150, 160, 160]);
        }
        g
    }

    fn axes(names: &[&str], pos: &[f64]) -> Vec<Axis> {
        names.iter().zip(pos).map(|(n, p)| Axis { name: (*n).into(), pos_mm: *p }).collect()
    }

    #[test]
    fn ring_bubbles_are_found_with_their_centres_and_radius() {
        let g = sheet(&[40.0, 120.0, 215.0], &[90.0, 180.0]);
        let b = find_bubbles(&g, W as u32, H as u32, 11, 18).unwrap();
        for (x, y) in [(40.0, 30.0), (120.0, 30.0), (215.0, 30.0), (260.0, 90.0), (260.0, 180.0)] {
            let near = b.iter().find(|c| (c.x - x).hypot(c.y - y) <= 1.5).unwrap_or_else(|| panic!("no bubble near ({x}, {y}): {b:#?}"));
            assert!((near.radius - 14.0).abs() <= 1.0, "{near:?}");
        }
        assert_eq!(b.len(), 5, "no other circle: {b:#?}");
        assert!(find_bubbles(&g, W as u32, H as u32, 2, 18).unwrap_err().contains("radius"));
        assert!(find_bubbles(&g[..10], W as u32, H as u32, 11, 18).unwrap_err().contains("bytes"));
    }

    #[test]
    fn a_chain_of_distances_gives_the_scale_and_the_position_even_when_bubbles_are_missing() {
        // Axes 0, 6000, 11000, 16000 mm; the drawing is at 50 mm/px, the first axis at pixel 20. Pixel positions: 20, 140, 240, 340.
        let ax = axes(&["A", "B", "C", "D"], &[0.0, 6000.0, 11000.0, 16000.0]);
        let full = fit_axis(&[20.0, 140.0, 240.0, 340.0], &ax, None).unwrap();
        assert!((full[0].mm_per_px - 50.0).abs() < 0.01 && full[0].matched.len() == 4 && full[0].rms_px < 0.01, "{:#?}", full[0]);
        // The crop shows only B, C and D: the same scale, and the position says which axes they are.
        let part = fit_axis(&[140.0, 240.0, 340.0], &ax, None).unwrap();
        assert!((part[0].mm_per_px - 50.0).abs() < 0.01 && part[0].matched.iter().map(|m| m.1.as_str()).collect::<Vec<_>>() == ["B", "C", "D"], "{:#?}", part[0]);
        assert!((part[0].offset_mm + 20.0 * 50.0).abs() < 0.5, "{}", part[0].offset_mm);
        // A stray circle that is no axis is not counted.
        let stray = fit_axis(&[20.0, 140.0, 240.0, 340.0, 300.0], &ax, None).unwrap();
        assert_eq!((stray[0].matched.len(), stray[0].unexplained.clone()), (4, vec![300.0]));
        assert!(fit_axis(&[20.0], &ax, None).unwrap_err().contains("two are needed"));
        assert!(fit_axis(&[20.0, 140.0], &axes(&["A"], &[0.0]), None).unwrap_err().contains("at least two"));
        assert!(fit_axis(&[20.0, 140.0], &axes(&["A", "B"], &[6000.0, 0.0]), None).unwrap_err().contains("increasing"));
    }

    #[test]
    fn columns_and_rows_must_agree_on_one_scale_and_a_doubtful_fit_is_an_error() {
        let xa = axes(&["A", "B", "C", "D"], &[0.0, 6000.0, 11000.0, 17000.0]);
        let ya = axes(&["1", "2", "3"], &[0.0, 4500.0, 9000.0]);
        // Two bubbles in the columns fit several ways (A-B and C-D both at 60 mm/px, B-C at 50): the rows, with three, give 50 and decide.
        let s = fit_scale(&[140.0, 240.0], &xa, &[30.0, 120.0, 210.0], &ya).unwrap();
        assert!((s.mm_per_px - 50.0).abs() < 0.2, "{s:#?}");
        assert!(s.notes.iter().any(|n| n.contains("confirm")), "two bubbles on one side are said to be unchecked: {:?}", s.notes);
        // Rows that cannot give the scale the columns give.
        let bad = fit_scale(&[20.0, 140.0, 240.0, 340.0], &xa, &[30.0, 100.0, 450.0], &ya);
        assert!(bad.unwrap_err().contains("same scale"));
        // Two bubbles of equally spaced axes cannot say which two they are: the answer is that, not a guess.
        let even = axes(&["A", "B", "C", "D"], &[0.0, 5000.0, 10000.0, 15000.0]);
        let amb = fit_scale(&[140.0, 240.0], &even, &[30.0, 120.0, 210.0], &ya);
        assert!(amb.unwrap_err().contains("more than one way"));
    }
}
