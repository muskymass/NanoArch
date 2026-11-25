//! Walls and openings read from a COLOURED raster floor plan (a screenshot, a rendered PDF page), as segments.
//!
//! `rasterplan` finds rooms as free regions; that fails on presentation plans, where furniture, hatching and wide openings let every
//! region leak. This module reads what such plans do state clearly: walls are thick ink of a few flat colours. The method was worked out
//! and measured on real drawings before it was written here (see the project notes that came with it); its limits are in the notes it returns.
//!
//! - **The wall colours are an input.** `wall_colour_candidates` lists the thick, dark, saturated colours of the image; the caller says
//!   which of them are walls (a plot boundary line or a neighbouring building is often drawn the same way). Nothing is chosen silently.
//! - **Pixels become millimetres only through `mm_per_px`**, the caller's scale. Without it nothing is read.
//! - **Openings.** On every scan line a gap between two wall runs is an opening when both runs are long (the wall carries on at both
//!   sides: up to `open_both_mm`) or when one is long (a door beside a corner: up to `open_one_mm`). Two walls seen across have short
//!   runs on both sides, so the room between them stays open.
//! - **Segments.** Along each direction, wall pixels lying in a run of at least `min_wall_mm` form blobs; a blob is projected on its
//!   axis and cut where its middle moves, so two walls side by side are two segments. Ends are moved onto the axis of the wall they meet.
//! - **Nothing is confirmed, and heights are not here.** A plan has no wall height, door height or sill: those are the user's to give.
//!   The result carries how many wall pixels the segments fail to cover and how many they add, so the caller can see how close it is.

use serde::{Deserialize, Serialize};

/// What the caller must and may give. `None` means "use the stated default and name it in the notes".
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WallParams {
    /// The drawing's scale. Required.
    pub mm_per_px: f64,
    /// The colours walls are drawn in, as RGB. Required, at least one.
    pub wall_colours: Vec<[u8; 3]>,
    /// How far (Euclidean, in RGB) a pixel may be from a wall colour and still be wall.
    pub colour_tolerance: Option<u32>,
    /// Widest opening in a wall that carries on at both sides.
    pub open_both_mm: Option<u32>,
    /// Widest opening with a long wall on one side only (a door beside a corner).
    pub open_one_mm: Option<u32>,
    /// A wall run at least this long, seen along a scan line, is "long".
    pub long_run_mm: Option<u32>,
    /// Shortest wall piece kept as a segment.
    pub min_wall_mm: Option<u32>,
    /// Narrowest gap reported as an opening.
    pub min_opening_mm: Option<u32>,
}

pub const DEFAULT_COLOUR_TOLERANCE: u32 = 40;
pub const DEFAULT_OPEN_BOTH_MM: u32 = 4000;
pub const DEFAULT_OPEN_ONE_MM: u32 = 1400;
pub const DEFAULT_LONG_RUN_MM: u32 = 450;
pub const DEFAULT_MIN_WALL_MM: u32 = 600;
pub const DEFAULT_MIN_OPENING_MM: u32 = 500;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WallSeg {
    pub horizontal: bool,
    /// The axis: y for a horizontal wall, x for a vertical one. Image pixels, y down.
    pub axis_px: f64,
    pub lo_px: f64,
    pub hi_px: f64,
    pub thickness_px: u32,
}

/// What a gap in a wall is, as far as the drawing shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpeningKind {
    /// Lines are drawn inside the wall's thickness (frame and glass).
    Window,
    /// A bare gap with a swing arc drawn beside it.
    Door,
    /// A bare gap with no arc: an opening without a leaf.
    Passage,
    /// Neither pattern: stair treads, a gap crossed by furniture. For the user to decide.
    Unclear,
}

/// At or above this share of drawn pixels inside the wall band, a gap is a window; below `BARE_INK_SHARE` it is bare floor.
pub const WINDOW_INK_SHARE: f64 = 0.40;
pub const BARE_INK_SHARE: f64 = 0.07;
/// Thin wall-colour ink beside a bare gap, per pixel of its width: a quarter circle is about 1.57. Inside this band it is a door's arc.
pub const ARC_MIN: f64 = 0.40;
pub const ARC_MAX: f64 = 1.60;
/// Below this there is no arc: a passage.
pub const NO_ARC: f64 = 0.20;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Opening {
    /// Index into `walls`.
    pub wall: usize,
    /// Along the wall's axis.
    pub lo_px: f64,
    pub hi_px: f64,
    pub kind: OpeningKind,
    /// Share of the pixels inside the wall band that are not a flat light fill. The measure `kind` was decided on.
    pub ink_share: f64,
    /// Thin wall-colour ink in the square beside the gap (the fuller side), per pixel of width. Measured for bare gaps only.
    pub arc_measure: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FoundWalls {
    pub walls: Vec<WallSeg>,
    pub openings: Vec<Opening>,
    pub mm_per_px: f64,
    /// Pixels of the wall colours that are thick enough to be walls.
    pub wall_pixels: u64,
    /// Wall pixels (openings sealed) the segments do not cover when drawn again.
    pub lost_pixels: u64,
    /// Pixels the segments cover that were neither wall nor opening.
    pub added_pixels: u64,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ColourCandidate {
    pub rgb: [u8; 3],
    pub pixels: u64,
    /// Share of those pixels that survive a 3 x 3 opening: walls are thick (near 1), furniture and text lines are thin (near 0).
    pub thick_share: f64,
}

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

fn colour_mask(rgb: &[u8], w: usize, h: usize, colours: &[[u8; 3]], tol: u32) -> Vec<u8> {
    let tol2 = i64::from(tol) * i64::from(tol);
    (0..w * h)
        .map(|i| {
            let p = &rgb[i * 3..i * 3 + 3];
            u8::from(colours.iter().any(|c| (0..3).map(|k| (i64::from(p[k]) - i64::from(c[k])).pow(2)).sum::<i64>() < tol2))
        })
        .collect()
}

/// 3 x 3 opening: what is thinner than 3 pixels goes. Outside the image counts as wall for the erosion, so the border does not eat walls.
fn open3(m: &[u8], w: usize, h: usize) -> Vec<u8> {
    let at = |v: &[u8], x: isize, y: isize, outside: u8| if x < 0 || y < 0 || x >= w as isize || y >= h as isize { outside } else { v[y as usize * w + x as usize] };
    let pass = |v: &[u8], erode: bool| -> Vec<u8> {
        let mut o = vec![0u8; w * h];
        for y in 0..h as isize {
            for x in 0..w as isize {
                let mut all = true;
                let mut any = false;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let p = at(v, x + dx, y + dy, u8::from(erode)) == 1;
                        all &= p;
                        any |= p;
                    }
                }
                o[y as usize * w + x as usize] = u8::from(if erode { all } else { any });
            }
        }
        o
    };
    pass(&pass(m, true), false)
}

/// 3 x 3 dilation.
fn dilate3(m: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut o = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            if m[y * w + x] == 1 {
                for yy in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                    for xx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                        o[yy * w + xx] = 1;
                    }
                }
            }
        }
    }
    o
}

const FILL_TOLERANCE: i64 = 14;
const ARC_COLOUR_TOLERANCE: u32 = 70;

/// The flat light colours of the drawing (floors, paper): the five most common colours whose brightest channel is at least 200.
fn light_fills(rgb: &[u8]) -> Vec<[u8; 3]> {
    let mut hist = std::collections::BTreeMap::<[u8; 3], u64>::new();
    for p in rgb.as_chunks::<3>().0 {
        if p[0].max(p[1]).max(p[2]) >= 200 {
            *hist.entry([p[0] / 8 * 8, p[1] / 8 * 8, p[2] / 8 * 8]).or_default() += 1;
        }
    }
    let mut v: Vec<([u8; 3], u64)> = hist.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.into_iter().take(5).map(|(c, _)| [c[0].saturating_add(4), c[1].saturating_add(4), c[2].saturating_add(4)]).collect()
}

/// The image with rows and columns swapped: width `h`, height `w`.
fn transpose(m: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut t = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            t[x * h + y] = m[y * w + x];
        }
    }
    t
}

/// Wall runs `[start, end)` of one row.
fn row_runs(row: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    for (x, &v) in row.iter().enumerate() {
        match (v == 1, start) {
            (true, None) => start = Some(x),
            (false, Some(s)) => {
                out.push((s, x));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s, row.len()));
    }
    out
}

/// Gaps along the rows that are openings by the rule in the module notes.
fn bridge_rows(wall: &[u8], w: usize, h: usize, both: usize, one: usize, long: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        let runs = row_runs(&wall[y * w..(y + 1) * w]);
        for k in 0..runs.len().saturating_sub(1) {
            let (a, b) = (runs[k].1 - runs[k].0, runs[k + 1].1 - runs[k + 1].0);
            let gap = runs[k + 1].0 - runs[k].1;
            if (a.min(b) >= long && gap <= both) || (a.max(b) >= long && gap <= one) {
                out[y * w + runs[k].1..y * w + runs[k + 1].0].fill(1);
            }
        }
    }
    out
}

/// The pixels lying in a row run of at least `k`.
fn long_runs(m: &[u8], w: usize, h: usize, k: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for (s, e) in row_runs(&m[y * w..(y + 1) * w]) {
            if e - s >= k {
                out[y * w + s..y * w + e].fill(1);
            }
        }
    }
    out
}

struct Blob {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    /// Row-major, `w * h`, 1 where the blob is.
    cells: Vec<u8>,
}

/// 4-connected blobs of a mask, in scan order of their first pixel.
fn blobs(m: &[u8], w: usize, h: usize) -> Vec<Blob> {
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if m[start] == 0 || seen[start] {
            continue;
        }
        let mut pix = Vec::new();
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
        seen[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            pix.push(i);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
            let mut go = |j: usize| {
                if m[j] == 1 && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                go(i - 1);
            }
            if x + 1 < w {
                go(i + 1);
            }
            if y > 0 {
                go(i - w);
            }
            if y + 1 < h {
                go(i + w);
            }
        }
        let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
        let mut cells = vec![0u8; bw * bh];
        for i in pix {
            cells[(i / w - y0) * bw + (i % w - x0)] = 1;
        }
        out.push(Blob { x: x0, y: y0, w: bw, h: bh, cells });
    }
    out
}

fn median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    let n = s.len();
    if n % 2 == 1 { s[n / 2] } else { (s[n / 2 - 1] + s[n / 2]) / 2.0 }
}

/// Horizontal wall segments of a mask (give the transposed mask for the vertical ones). `(axis, lo, hi, thickness)`.
fn lines(m: &[u8], w: usize, h: usize, k: usize) -> Vec<(f64, f64, f64, u32)> {
    let mut out = Vec::new();
    for b in blobs(&long_runs(m, w, h, k), w, h) {
        let cnt: Vec<usize> = (0..b.w).map(|x| (0..b.h).filter(|y| b.cells[y * b.w + x] == 1).count()).collect();
        let mid: Vec<f64> = (0..b.w).map(|x| if cnt[x] == 0 { -1.0 } else { (0..b.h).filter(|y| b.cells[y * b.w + x] == 1).sum::<usize>() as f64 / cnt[x] as f64 }).collect();
        let mut emit = |a0: usize, a1: usize| {
            if a1 - a0 < k {
                return;
            }
            let rows: Vec<usize> = (0..b.h).filter(|y| (a0..a1).filter(|x| b.cells[y * b.w + x] == 1).count() >= (a1 - a0) / 2).collect();
            if let (Some(&r0), Some(&r1)) = (rows.first(), rows.last()) {
                out.push((b.y as f64 + (r0 + r1 + 1) as f64 / 2.0, (b.x + a0) as f64, (b.x + a1) as f64, (r1 - r0 + 1) as u32));
            }
        };
        let mut a0: Option<usize> = None;
        for xx in 0..=b.w {
            let ok = xx < b.w && cnt[xx] >= 2;
            if ok && a0.is_none() {
                a0 = Some(xx);
            }
            if let Some(s) = a0 {
                // Two walls side by side in one blob: the middle of the covered rows moves by more than 2 px.
                let cut = xx > s && (!ok || (mid[xx] - median(&mid[s..xx])).abs() > 2.0);
                if !ok || cut {
                    emit(s, xx);
                    a0 = ok.then_some(xx);
                }
            }
        }
    }
    out
}

fn px(mm: u32, mm_per_px: f64) -> usize {
    (f64::from(mm) / mm_per_px).round_ties_even().max(1.0) as usize
}

/// The thick, dark, saturated colours of the image, most common first: what a wall colour looks like. The caller picks among them.
pub fn wall_colour_candidates(rgb: &[u8], width: u32, height: u32) -> Result<Vec<ColourCandidate>, String> {
    let (w, h) = check(rgb, width, height)?;
    let mut hist = std::collections::BTreeMap::<[u8; 3], u64>::new();
    for p in rgb.as_chunks::<3>().0 {
        let (mx, mn) = (p[0].max(p[1]).max(p[2]), p[0].min(p[1]).min(p[2]));
        if mx <= 160 && mx - mn >= 50 {
            *hist.entry([p[0] / 8 * 8 + 4, p[1] / 8 * 8 + 4, p[2] / 8 * 8 + 4]).or_default() += 1;
        }
    }
    let mut by_count: Vec<([u8; 3], u64)> = hist.into_iter().collect();
    by_count.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut out: Vec<ColourCandidate> = Vec::new();
    for (c, _) in by_count.into_iter().take(12) {
        // A colour within the tolerance of one already listed is the same ink (anti-aliasing spreads it over neighbouring bins).
        if out.iter().any(|o| (0..3).map(|k| (i64::from(o.rgb[k]) - i64::from(c[k])).pow(2)).sum::<i64>() < i64::from(DEFAULT_COLOUR_TOLERANCE).pow(2)) {
            continue;
        }
        let m = colour_mask(rgb, w, h, &[c], DEFAULT_COLOUR_TOLERANCE);
        let pixels: u64 = m.iter().map(|&v| u64::from(v)).sum();
        let thick: u64 = open3(&m, w, h).iter().map(|&v| u64::from(v)).sum();
        if pixels > 0 {
            out.push(ColourCandidate { rgb: c, pixels, thick_share: thick as f64 / pixels as f64 });
        }
    }
    Ok(out)
}

/// Walls and openings of a coloured plan. See the module notes for the method and its limits.
pub fn read_walls(rgb: &[u8], width: u32, height: u32, p: &WallParams) -> Result<FoundWalls, String> {
    let (w, h) = check(rgb, width, height)?;
    if !(p.mm_per_px.is_finite() && p.mm_per_px > 0.0) {
        return Err("the scale (mm_per_px) is missing: set the drawing's scale from a known dimension first".into());
    }
    if p.wall_colours.is_empty() {
        return Err("no wall colour was given: choose which colours of the drawing are walls".into());
    }
    let mut notes = Vec::new();
    let mut setting = |given: Option<u32>, default: u32, what: &str, unit: &str| -> u32 {
        if given.is_none() {
            notes.push(format!("{what} was taken as {default} {unit}; give it if this drawing differs"));
        }
        given.unwrap_or(default)
    };
    let tol = setting(p.colour_tolerance, DEFAULT_COLOUR_TOLERANCE, "the colour tolerance", "(RGB distance)");
    let both = px(setting(p.open_both_mm, DEFAULT_OPEN_BOTH_MM, "the widest opening in a wall that carries on at both sides", "mm"), p.mm_per_px);
    let one = px(setting(p.open_one_mm, DEFAULT_OPEN_ONE_MM, "the widest opening beside a corner", "mm"), p.mm_per_px);
    let long = px(setting(p.long_run_mm, DEFAULT_LONG_RUN_MM, "the shortest wall run that counts as a wall carrying on", "mm"), p.mm_per_px);
    let k = px(setting(p.min_wall_mm, DEFAULT_MIN_WALL_MM, "the shortest wall kept", "mm"), p.mm_per_px);
    let min_open_mm = f64::from(setting(p.min_opening_mm, DEFAULT_MIN_OPENING_MM, "the narrowest opening reported", "mm"));

    let wall = open3(&colour_mask(rgb, w, h, &p.wall_colours, tol), w, h);
    let wall_pixels: u64 = wall.iter().map(|&v| u64::from(v)).sum();
    if wall_pixels == 0 {
        return Err("no thick ink of the given wall colours was found: check the colours, or the drawing is a line drawing this reader cannot use".into());
    }
    let wall_t = transpose(&wall, w, h);
    // Openings along rows (in horizontal walls) and along columns (in vertical walls); a wall pixel is never an opening.
    let bh: Vec<u8> = bridge_rows(&wall, w, h, both, one, long).iter().zip(&wall).map(|(&b, &m)| b & (1 - m)).collect();
    let bv_t: Vec<u8> = bridge_rows(&wall_t, h, w, both, one, long).iter().zip(&wall_t).map(|(&b, &m)| b & (1 - m)).collect();
    let bv = transpose(&bv_t, h, w);
    let sealed: Vec<u8> = (0..w * h).map(|i| wall[i] | bh[i] | bv[i]).collect();
    let sealed_t = transpose(&sealed, w, h);

    let mut walls: Vec<WallSeg> = lines(&sealed, w, h, k).into_iter().map(|(c, lo, hi, t)| WallSeg { horizontal: true, axis_px: c, lo_px: lo, hi_px: hi, thickness_px: t }).collect();
    walls.extend(lines(&sealed_t, h, w, k).into_iter().map(|(c, lo, hi, t)| WallSeg { horizontal: false, axis_px: c, lo_px: lo, hi_px: hi, thickness_px: t }));
    if walls.is_empty() {
        return Err(format!("no wall at least {} mm long was found in the given colours", (k as f64 * p.mm_per_px).round()));
    }
    // Corners and T-junctions: an end that stops on a perpendicular wall is moved onto that wall's axis.
    for i in 0..walls.len() {
        for end in 0..2 {
            let (c, at) = (walls[i].axis_px, if end == 0 { walls[i].lo_px } else { walls[i].hi_px });
            let best = walls
                .iter()
                .filter(|o| o.horizontal != walls[i].horizontal && o.lo_px - f64::from(o.thickness_px) <= c && c <= o.hi_px + f64::from(o.thickness_px))
                .map(|o| ((o.axis_px - at).abs(), o.axis_px, f64::from(o.thickness_px)))
                .filter(|(d, _, t)| *d <= t + 1.0)
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((_, axis, _)) = best {
                if end == 0 { walls[i].lo_px = axis } else { walls[i].hi_px = axis }
            }
        }
    }
    let r = |v: f64| v.round_ties_even().max(0.0) as usize;
    // For telling doors from windows: the flat light fills of the drawing (floors, paper), and the THIN ink of the wall colours
    // (door arcs), which the wall mask threw away. Arcs are anti-aliased, so they are looked for with a wider tolerance.
    let fills = light_fills(rgb);
    let is_fill = |i: usize| {
        let px = &rgb[i * 3..i * 3 + 3];
        fills.iter().any(|f| (0..3).map(|k| (i64::from(px[k]) - i64::from(f[k])).pow(2)).sum::<i64>() <= FILL_TOLERANCE * FILL_TOLERANCE)
    };
    let loose = colour_mask(rgb, w, h, &p.wall_colours, ARC_COLOUR_TOLERANCE);
    let thick = dilate3(&open3(&loose, w, h), w, h);
    let thin: Vec<u8> = loose.iter().zip(&thick).map(|(&m, &t)| m & (1 - t)).collect();
    // Openings: the sealed gaps along each wall, where at least half of its thickness was bridged.
    let mut openings = Vec::new();
    for (i, s) in walls.iter().enumerate() {
        let half = f64::from(s.thickness_px) / 2.0;
        let (a, b) = (r(s.axis_px - half), r(s.axis_px + half));
        let (lo, hi) = (r(s.lo_px), r(s.hi_px));
        let (across, along) = if s.horizontal { (h, w) } else { (w, h) };
        let (b, hi) = (b.min(across), hi.min(along));
        if b <= a || hi <= lo {
            continue;
        }
        let bridged = |q: usize, t: usize| if s.horizontal { bh[q * w + t] } else { bv[t * w + q] };
        let prof: Vec<u8> = (lo..hi).map(|t| u8::from((a..b).filter(|&q| bridged(q, t) == 1).count() >= ((b - a) / 2).max(1))).collect();
        for (x0, x1) in row_runs(&prof) {
            if (x1 - x0) as f64 * p.mm_per_px >= min_open_mm {
                let (g0, g1) = (lo + x0, lo + x1);
                // Door, window or passage: what is drawn in the gap and beside it (see `OpeningKind`). Both measures are returned.
                let at = |q: usize, t: usize| if s.horizontal { q * w + t } else { t * w + q };
                let band = (b - a) * (g1 - g0);
                let inked = (a..b).map(|q| (g0..g1).filter(|&t| !is_fill(at(q, t))).count()).sum::<usize>();
                let ink_share = inked as f64 / band.max(1) as f64;
                let width = g1 - g0;
                let side = |from: usize, to: usize| (from..to.min(across)).map(|q| (g0..g1).filter(|&t| thin[at(q, t)] == 1).count()).sum::<usize>();
                let reach = s.thickness_px as usize / 2 + 1;
                let axis = r(s.axis_px);
                let after = side(axis + reach, axis + reach + width);
                let before = side(axis.saturating_sub(reach).saturating_sub(width), axis.saturating_sub(reach));
                let arc = after.max(before) as f64 / width.max(1) as f64;
                let bare = ink_share < BARE_INK_SHARE;
                let kind = if ink_share >= WINDOW_INK_SHARE {
                    OpeningKind::Window
                } else if bare && (ARC_MIN..=ARC_MAX).contains(&arc) {
                    OpeningKind::Door
                } else if bare && (arc < NO_ARC || width as f64 * p.mm_per_px > f64::from(DEFAULT_OPEN_ONE_MM)) {
                    // No arc; or a gap wider than any door, whose square caught furniture, not an arc.
                    OpeningKind::Passage
                } else {
                    OpeningKind::Unclear
                };
                openings.push(Opening { wall: i, lo_px: g0 as f64, hi_px: g1 as f64, kind, ink_share, arc_measure: bare.then_some(arc) });
            }
        }
    }
    // The segments drawn again, against the sealed walls they came from: the caller sees how much was lost or added.
    let mut again = vec![0u8; w * h];
    for s in &walls {
        let half = f64::from(s.thickness_px) / 2.0;
        let (a, b) = (r(s.axis_px - half), r(s.axis_px + half));
        let (lo, hi) = (r(s.lo_px - half), r(s.hi_px + half));
        let (across, along) = if s.horizontal { (h, w) } else { (w, h) };
        for q in a..b.min(across) {
            for t in lo..hi.min(along) {
                again[if s.horizontal { q * w + t } else { t * w + q }] = 1;
            }
        }
    }
    let lost_pixels = (0..w * h).filter(|&i| sealed[i] == 1 && again[i] == 0).count() as u64;
    let added_pixels = (0..w * h).filter(|&i| again[i] == 1 && sealed[i] == 0).count() as u64;
    notes.push(format!("{} wall segment(s) and {} opening(s) were read from {wall_pixels} wall pixels; drawn again, the segments miss {lost_pixels} pixel(s) and add {added_pixels}", walls.len(), openings.len()));
    notes.push(format!("one pixel is {:.0} mm at this scale: every position and thickness is known only to that", p.mm_per_px));
    notes.push("a plan holds no heights: wall height, door and window height and sill height must be given before these walls can be built".into());
    let count = |k: OpeningKind| openings.iter().filter(|o| o.kind == k).count();
    notes.push(format!(
        "openings by what is drawn: {} window(s) (lines inside the wall), {} door(s) (a bare gap with a swing arc), {} passage(s) (a bare gap, no arc), {} unclear: each carries the measures it was decided on, for review",
        count(OpeningKind::Window),
        count(OpeningKind::Door),
        count(OpeningKind::Passage),
        count(OpeningKind::Unclear)
    ));
    notes.push("stairs drawn with thick treads come out as short walls and openings: review that area".into());
    Ok(FoundWalls { walls, openings, mm_per_px: p.mm_per_px, wall_pixels, lost_pixels, added_pixels, notes })
}

#[cfg(test)]
mod tests {
    use super::*;

    const WALL: [u8; 3] = [28, 124, 108];
    const W: usize = 240;
    const H: usize = 160;

    fn paint(img: &mut [u8], x0: usize, y0: usize, x1: usize, y1: usize, c: [u8; 3]) {
        for y in y0..y1 {
            for x in x0..x1 {
                img[(y * W + x) * 3..(y * W + x) * 3 + 3].copy_from_slice(&c);
            }
        }
    }

    /// Two rooms side by side, walls 6 px thick, 50 mm per pixel: outer box 20..220 x 20..140, a partition at x = 117..123 with a door gap
    /// of 20 px (1000 mm) in it, and a window gap of 30 px (1500 mm) in the top wall of the left room.
    fn plan() -> Vec<u8> {
        let mut g = vec![235u8; W * H * 3];
        paint(&mut g, 20, 20, 220, 26, WALL);
        paint(&mut g, 20, 134, 220, 140, WALL);
        paint(&mut g, 20, 20, 26, 140, WALL);
        paint(&mut g, 214, 20, 220, 140, WALL);
        paint(&mut g, 117, 20, 123, 140, WALL);
        paint(&mut g, 117, 70, 123, 90, [235, 235, 235]);
        paint(&mut g, 50, 20, 80, 26, [235, 235, 235]);
        // The window: frame lines inside the wall's thickness. The door: a one-pixel line of its swing drawn beside the gap.
        paint(&mut g, 50, 20, 80, 21, [90, 100, 110]);
        paint(&mut g, 50, 22, 80, 23, [90, 100, 110]);
        paint(&mut g, 50, 25, 80, 26, [90, 100, 110]);
        paint(&mut g, 125, 70, 143, 71, WALL);
        g
    }

    fn params() -> WallParams {
        WallParams { mm_per_px: 50.0, wall_colours: vec![WALL], ..Default::default() }
    }

    #[test]
    fn walls_come_out_as_segments_and_gaps_as_openings() {
        let f = read_walls(&plan(), W as u32, H as u32, &params()).unwrap();
        let (hz, vt): (Vec<_>, Vec<_>) = f.walls.iter().partition(|s| s.horizontal);
        assert_eq!((hz.len(), vt.len()), (2, 3), "{:#?}", f.walls);
        assert!(f.walls.iter().all(|s| s.thickness_px == 6), "{:#?}", f.walls);
        // The partition's axis is the middle of its 6 pixels, and it ends on the axes of the top and bottom walls.
        let part = vt.iter().find(|s| (s.axis_px - 120.0).abs() < 0.01).expect("the partition");
        assert_eq!((part.lo_px, part.hi_px), (23.0, 137.0));
        let widths: Vec<f64> = f.openings.iter().map(|o| (o.hi_px - o.lo_px) * f.mm_per_px).collect();
        assert_eq!(f.openings.len(), 2, "{:#?}", f.openings);
        assert!(widths.contains(&1000.0) && widths.contains(&1500.0), "{widths:?}");
        let kind_of = |mm: f64| f.openings.iter().find(|o| (o.hi_px - o.lo_px) * f.mm_per_px == mm).map(|o| o.kind);
        assert_eq!(kind_of(1500.0), Some(OpeningKind::Window), "{:#?}", f.openings);
        assert_eq!(kind_of(1000.0), Some(OpeningKind::Door), "{:#?}", f.openings);
        assert_eq!(f.lost_pixels, 0);
    }

    #[test]
    fn a_bare_gap_without_an_arc_is_a_passage_and_the_measures_are_returned() {
        let mut g = plan();
        paint(&mut g, 125, 70, 143, 71, [235, 235, 235]); // the arc is gone
        let f = read_walls(&g, W as u32, H as u32, &params()).unwrap();
        let o = f.openings.iter().find(|o| (o.hi_px - o.lo_px) == 20.0).expect("the gap in the partition");
        assert_eq!(o.kind, OpeningKind::Passage, "{o:#?}");
        assert_eq!((o.ink_share, o.arc_measure), (0.0, Some(0.0)));
        let win = f.openings.iter().find(|o| (o.hi_px - o.lo_px) == 30.0).unwrap();
        assert!(win.ink_share >= WINDOW_INK_SHARE && win.arc_measure.is_none(), "{win:#?}");
    }

    #[test]
    fn thin_coloured_lines_and_other_colours_are_not_walls() {
        let mut g = plan();
        paint(&mut g, 40, 60, 100, 61, WALL); // a 1 px line in the wall colour: furniture, a dimension tick
        paint(&mut g, 140, 50, 200, 56, [60, 100, 156]); // thick, but a colour that was not named
        let clean = read_walls(&plan(), W as u32, H as u32, &params()).unwrap();
        let f = read_walls(&g, W as u32, H as u32, &params()).unwrap();
        assert_eq!(f.walls, clean.walls);
    }

    #[test]
    fn two_walls_seen_across_do_not_close_the_room_between_them() {
        // No gap of the plan is bridged across a room: every opening is one of the two real gaps.
        let f = read_walls(&plan(), W as u32, H as u32, &params()).unwrap();
        assert!(f.openings.iter().all(|o| (o.hi_px - o.lo_px) <= 30.0), "{:#?}", f.openings);
    }

    #[test]
    fn a_missing_scale_or_colour_and_a_bad_bitmap_are_errors_not_guesses() {
        let g = plan();
        assert!(read_walls(&g, W as u32, H as u32, &WallParams { mm_per_px: 0.0, wall_colours: vec![WALL], ..Default::default() }).unwrap_err().contains("scale"));
        assert!(read_walls(&g, W as u32, H as u32, &WallParams { mm_per_px: 50.0, ..Default::default() }).unwrap_err().contains("colour"));
        assert!(read_walls(&g[..100], W as u32, H as u32, &params()).unwrap_err().contains("bytes"));
        let other = WallParams { mm_per_px: 50.0, wall_colours: vec![[200, 30, 30]], ..Default::default() };
        assert!(read_walls(&g, W as u32, H as u32, &other).unwrap_err().contains("no thick ink"));
    }

    #[test]
    fn the_wall_colour_is_offered_and_defaults_are_named() {
        let c = wall_colour_candidates(&plan(), W as u32, H as u32).unwrap();
        assert_eq!(c.len(), 1, "{c:#?}");
        assert!((0..3).all(|k| c[0].rgb[k].abs_diff(WALL[k]) <= 8) && c[0].thick_share > 0.9, "{c:#?}");
        let f = read_walls(&plan(), W as u32, H as u32, &params()).unwrap();
        assert!(f.notes.iter().any(|n| n.contains("4000 mm")) && f.notes.iter().any(|n| n.contains("no heights")), "{:#?}", f.notes);
        let all = WallParams { colour_tolerance: Some(40), open_both_mm: Some(4000), open_one_mm: Some(1400), long_run_mm: Some(450), min_wall_mm: Some(600), min_opening_mm: Some(500), ..params() };
        assert!(!read_walls(&plan(), W as u32, H as u32, &all).unwrap().notes.iter().any(|n| n.contains("was taken as")));
    }
}
