//! Rooms found in a raster floor plan (a scan, a photo, a rendered PDF page) by image analysis alone.
//!
//! - **No model, no guess about size.** The input is a grey bitmap; the output is boxes in IMAGE PIXELS. Pixels become
//!   millimetres only through the user's scale reference (see `sketchread::Scale`), exactly as for the manual tracing tool.
//! - **Walls are thick ink.** A pixel is ink when it is darker than the Otsu threshold of the bitmap. Thin ink (text, dimension
//!   lines, hatching) is dropped by a thickness test: a wall pixel belongs to a run at least `wall_px` thick across and
//!   `6 * wall_px` long along. `wall_px` is given, or derived from the image size and then named in the notes.
//! - **Doors do not leak.** Walls are widened by half of `gap_px` before the free space is labelled, so an opening up to
//!   `gap_px` wide does not join two rooms; each box is widened back by the same amount.
//! - **Rooms are free regions closed by walls.** A region touching the image border is outside, not a room. A region whose
//!   box is not mostly filled (an L or T shape, or a leak) is returned with `fill_ratio` below 1 and an issue, never forced
//!   into a rectangle silently.
//! - **Nothing is confirmed.** Every room comes back for the user to review.

use serde::Serialize;

/// Tuning the caller may give. `None` means "derive it from the image and say so".
#[derive(Debug, Clone, Default)]
pub struct Params {
    /// Thinnest wall, in pixels.
    pub wall_px: Option<u32>,
    /// Widest opening a door may leave in a wall, in pixels.
    pub gap_px: Option<u32>,
    /// Smallest room side, in pixels.
    pub min_room_px: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FoundRoom {
    /// `[x0, y0, x1, y1]` in image pixels, y down.
    pub bbox_px: [u32; 4],
    pub area_px: u64,
    /// Region area over box area. 1.0 is a perfect rectangle.
    pub fill_ratio: f64,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Found {
    pub rooms: Vec<FoundRoom>,
    pub wall_px: u32,
    pub gap_px: u32,
    pub ink_threshold: u8,
    pub wall_pixels: u64,
    pub notes: Vec<String>,
}

/// Fraction of the box a region must fill to be treated as a rectangular room.
pub const MIN_FILL: f64 = 0.85;

/// Otsu's threshold of a grey bitmap: the level that best separates dark ink from light paper.
pub fn otsu(gray: &[u8]) -> u8 {
    let mut hist = [0u64; 256];
    for &g in gray {
        hist[g as usize] += 1;
    }
    let total = gray.len() as f64;
    let sum_all: f64 = hist.iter().enumerate().map(|(i, &c)| i as f64 * c as f64).sum();
    let (mut w_b, mut sum_b, mut best, mut thr) = (0.0, 0.0, -1.0, 0u8);
    for (t, &c) in hist.iter().enumerate() {
        w_b += c as f64;
        if w_b == 0.0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f == 0.0 {
            break;
        }
        sum_b += t as f64 * c as f64;
        let (m_b, m_f) = (sum_b / w_b, (sum_all - sum_b) / w_f);
        let between = w_b * w_f * (m_b - m_f).powi(2);
        if between > best {
            best = between;
            thr = t as u8;
        }
    }
    thr
}

/// Length of the run of `true` through each pixel, along x (`horizontal`) or along y.
fn runs(mask: &[bool], w: usize, h: usize, horizontal: bool) -> Vec<u32> {
    let mut out = vec![0u32; w * h];
    let (outer, inner) = if horizontal { (h, w) } else { (w, h) };
    for o in 0..outer {
        let at = |i: usize| if horizontal { o * w + i } else { i * w + o };
        let mut i = 0;
        while i < inner {
            if !mask[at(i)] {
                i += 1;
                continue;
            }
            let start = i;
            while i < inner && mask[at(i)] {
                i += 1;
            }
            for k in start..i {
                out[at(k)] = (i - start) as u32;
            }
        }
    }
    out
}

/// A square dilation by `r` pixels on each side, through an integral image.
fn dilate(mask: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    if r == 0 {
        return mask.to_vec();
    }
    let stride = w + 1;
    let mut integral = vec![0u32; stride * (h + 1)];
    for y in 0..h {
        let mut row = 0u32;
        for x in 0..w {
            row += mask[y * w + x] as u32;
            integral[(y + 1) * stride + x + 1] = integral[y * stride + x + 1] + row;
        }
    }
    let mut out = vec![false; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let s = integral[y1 * stride + x1] + integral[y0 * stride + x0] - integral[y0 * stride + x1] - integral[y1 * stride + x0];
            out[y * w + x] = s > 0;
        }
    }
    out
}

/// Finds the rooms of a grey bitmap (`gray.len() == width * height`, 0 black, 255 white).
pub fn detect(gray: &[u8], width: u32, height: u32, p: &Params) -> Result<Found, String> {
    detect_impl(gray, None, width, height, p)
}

/// Like `detect`, from a colour bitmap (`rgb.len() == 3 * width * height`): the wall colour is learned from the thick ink.
pub fn detect_rgb(rgb: &[u8], width: u32, height: u32, p: &Params) -> Result<Found, String> {
    let n = width as usize * height as usize;
    if rgb.len() != n * 3 {
        return Err(format!("the colour bitmap has {} bytes but {} x {} x 3 = {} were expected", rgb.len(), width, height, n * 3));
    }
    let gray: Vec<u8> = (0..n).map(|i| ((rgb[i * 3] as u32 * 299 + rgb[i * 3 + 1] as u32 * 587 + rgb[i * 3 + 2] as u32 * 114) / 1000) as u8).collect();
    detect_impl(&gray, Some(rgb), width, height, p)
}

fn detect_impl(gray: &[u8], rgb: Option<&[u8]>, width: u32, height: u32, p: &Params) -> Result<Found, String> {
    let (w, h) = (width as usize, height as usize);
    if w < 40 || h < 40 {
        return Err("the image is too small to read a plan from (at least 40 x 40 pixels)".into());
    }
    if gray.len() != w * h {
        return Err(format!("the bitmap has {} pixels but the size says {} x {}", gray.len(), width, height));
    }
    let mut notes = Vec::new();
    let thr = otsu(gray);
    let ink: Vec<bool> = gray.iter().map(|&g| g <= thr).collect();
    let ink_n = ink.iter().filter(|b| **b).count();
    if ink_n == 0 || ink_n * 2 > ink.len() {
        return Err("the image has no clear dark drawing on a light page: it cannot be separated into ink and paper".into());
    }
    let wall = match p.wall_px {
        Some(v) if v >= 1 => v,
        Some(_) => return Err("the wall thickness must be at least 1 pixel".into()),
        None => {
            let v = ((w.min(h) as f64 / 150.0).round() as u32).max(2);
            notes.push(format!("the wall thickness was taken as {v} px (one 150th of the image); give it if the walls are thinner or thicker"));
            v
        }
    };
    let gap = match p.gap_px {
        Some(v) => v,
        None => {
            let v = wall * 6;
            notes.push(format!("openings up to {v} px wide were treated as doors that do not join rooms"));
            v
        }
    };
    let min_room = p.min_room_px.unwrap_or(wall * 6).max(1) as usize;
    let long = (wall * 6) as u32;
    // The across-thickness is bounded above too: where two thin lines cross, the run through the crossing is as long as a whole
    // line, and that is not a thick wall. Junction pixels of real walls fall out the same way; widening closes those pinholes.
    let thick = wall * 8;
    let wall_mask = |ink: &[bool]| -> Vec<bool> {
        let hr = runs(ink, w, h, true);
        let vr = runs(ink, w, h, false);
        (0..w * h).map(|i| ink[i] && ((vr[i] >= wall && vr[i] <= thick && hr[i] >= long) || (hr[i] >= wall && hr[i] <= thick && vr[i] >= long))).collect()
    };
    let mut walls = wall_mask(&ink);
    if let Some(c) = rgb {
        // A coloured plan draws its walls in one colour and everything else (symbols, numbered markers, bold text) in others. The
        // colour of the thick ink is taken as the wall colour and only ink of that colour is kept.
        let mut chan: [Vec<u8>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for i in (0..w * h).filter(|i| walls[*i]) {
            for k in 0..3 {
                chan[k].push(c[i * 3 + k]);
            }
        }
        if !chan[0].is_empty() {
            let med = [0, 1, 2].map(|k| {
                chan[k].sort_unstable();
                chan[k][chan[k].len() / 2] as i32
            });
            let near: Vec<bool> = (0..w * h).map(|i| ink[i] && (0..3).all(|k| (c[i * 3 + k] as i32 - med[k]).abs() <= 48)).collect();
            walls = wall_mask(&near);
            notes.push(format!("the wall colour was taken as RGB({}, {}, {}); ink of other colours was ignored", med[0], med[1], med[2]));
        }
    }
    let wall_n = walls.iter().filter(|b| **b).count() as u64;
    if wall_n == 0 {
        return Err(format!("no wall was found: nothing is {wall} px thick and {long} px long. Give the wall thickness in pixels"));
    }
    let half = (gap / 2) as usize;
    let closed = dilate(&walls, w, h, half);

    let mut label = vec![false; w * h];
    let mut rooms = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if closed[start] || label[start] {
            continue;
        }
        label[start] = true;
        stack.push(start);
        let (mut x0, mut y0, mut x1, mut y1, mut area) = (w, h, 0usize, 0usize, 0u64);
        let mut outside = false;
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            area += 1;
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
            if x == 0 || y == 0 || x + 1 == w || y + 1 == h {
                outside = true;
            }
            let mut push = |j: usize| {
                if !closed[j] && !label[j] {
                    label[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                push(i - 1);
            }
            if x + 1 < w {
                push(i + 1);
            }
            if y > 0 {
                push(i - w);
            }
            if y + 1 < h {
                push(i + w);
            }
        }
        if outside {
            continue;
        }
        // The region stops `half` pixels short of each wall; the room's face is there.
        let (bx0, by0) = (x0.saturating_sub(half), y0.saturating_sub(half));
        let (bx1, by1) = ((x1 + 1 + half).min(w), (y1 + 1 + half).min(h));
        if bx1 - bx0 < min_room || by1 - by0 < min_room {
            continue;
        }
        let box_area = ((x1 - x0 + 1) * (y1 - y0 + 1)) as f64;
        let fill = area as f64 / box_area;
        let mut issues = Vec::new();
        if fill < MIN_FILL {
            issues.push(format!("the region fills only {:.0}% of its box: it is not a rectangle (an L or T room, or a wall that does not close). Trace it by hand or split it", fill * 100.0));
        }
        rooms.push(FoundRoom { bbox_px: [bx0 as u32, by0 as u32, bx1 as u32, by1 as u32], area_px: area, fill_ratio: (fill * 1000.0).round() / 1000.0, issues });
    }
    rooms.sort_by_key(|r| (r.bbox_px[1], r.bbox_px[0]));
    if rooms.is_empty() {
        return Err("no closed room was found: the walls do not close any region. Raise the door gap or draw the missing wall".into());
    }
    notes.push("these boxes were found by image analysis and are unconfirmed: review every one against the drawing before using it".into());
    Ok(Found { rooms, wall_px: wall, gap_px: gap, ink_threshold: thr, wall_pixels: wall_n, notes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(g: &mut [u8], w: usize, x0: usize, y0: usize, x1: usize, y1: usize, v: u8) {
        for y in y0..y1 {
            for x in x0..x1 {
                g[y * w + x] = v;
            }
        }
    }

    /// 240 x 160 page: outer walls 6 px thick, one partition with a 20 px door, stray thin lines and a 1 px "text" dash.
    fn plan() -> Vec<u8> {
        let (w, h) = (240usize, 160usize);
        let mut g = vec![255u8; w * h];
        rect(&mut g, w, 20, 20, 220, 26, 30);
        rect(&mut g, w, 20, 134, 220, 140, 30);
        rect(&mut g, w, 20, 20, 26, 140, 30);
        rect(&mut g, w, 214, 20, 220, 140, 30);
        rect(&mut g, w, 117, 26, 123, 70, 30);
        rect(&mut g, w, 117, 90, 123, 134, 30);
        rect(&mut g, w, 40, 60, 100, 61, 30);
        rect(&mut g, w, 140, 100, 141, 125, 30);
        g
    }

    #[test]
    fn two_rooms_are_found_and_the_door_gap_does_not_join_them() {
        let f = detect(&plan(), 240, 160, &Params { wall_px: Some(5), gap_px: Some(26), min_room_px: None }).unwrap();
        assert_eq!(f.rooms.len(), 2, "{:?}", f.rooms);
        let (a, b) = (&f.rooms[0], &f.rooms[1]);
        // the inside faces of the walls: 26..117 and 123..214 across, 26..134 down (a few pixels of tolerance)
        let near = |v: u32, t: u32| v.abs_diff(t) <= 3;
        assert!(near(a.bbox_px[0], 26) && near(a.bbox_px[2], 117) && near(a.bbox_px[1], 26) && near(a.bbox_px[3], 134), "{:?}", a.bbox_px);
        assert!(near(b.bbox_px[0], 123) && near(b.bbox_px[2], 214), "{:?}", b.bbox_px);
        assert!(a.fill_ratio >= MIN_FILL && b.fill_ratio >= MIN_FILL && a.issues.is_empty());
    }

    #[test]
    fn a_door_wider_than_the_gap_joins_the_rooms_into_one_that_is_still_closed_by_the_outer_walls() {
        let f = detect(&plan(), 240, 160, &Params { wall_px: Some(5), gap_px: Some(8), min_room_px: None }).unwrap();
        assert_eq!(f.rooms.len(), 1);
    }

    #[test]
    fn thin_lines_are_not_walls_and_missing_walls_are_reported() {
        let (w, h) = (120usize, 80usize);
        let mut g = vec![255u8; w * h];
        rect(&mut g, w, 10, 40, 110, 41, 0);
        rect(&mut g, w, 60, 10, 61, 70, 0);
        let e = detect(&g, 120, 80, &Params { wall_px: Some(5), gap_px: None, min_room_px: None }).unwrap_err();
        assert!(e.contains("no wall was found"), "{e}");
    }

    #[test]
    fn bad_inputs_are_errors_not_guesses() {
        assert!(detect(&[0; 10], 5, 2, &Params::default()).unwrap_err().contains("too small"));
        assert!(detect(&[255; 100 * 100], 100, 100, &Params::default()).unwrap_err().contains("no clear dark drawing"));
        assert!(detect(&[255; 99], 100, 100, &Params::default()).unwrap_err().contains("pixels but the size says"));
        assert!(detect(&plan(), 240, 160, &Params { wall_px: Some(0), ..Params::default() }).unwrap_err().contains("at least 1 pixel"));
    }

    #[test]
    fn an_l_shaped_region_is_flagged_not_forced_into_a_rectangle() {
        let (w, h) = (200usize, 160usize);
        let mut g = vec![255u8; w * h];
        for (x0, y0, x1, y1) in [(20, 20, 180, 26), (20, 134, 180, 140), (20, 20, 26, 140), (174, 20, 180, 80), (100, 74, 180, 80), (100, 74, 106, 140)] {
            rect(&mut g, w, x0, y0, x1, y1, 20);
        }
        // the right-hand lower block (106..174 x 80..134) is closed; the L is the rest.
        rect(&mut g, w, 174, 80, 180, 140, 20);
        let f = detect(&g, 200, 160, &Params { wall_px: Some(5), gap_px: Some(10), min_room_px: None }).unwrap();
        assert!(f.rooms.iter().any(|r| r.fill_ratio < MIN_FILL && !r.issues.is_empty()), "{:?}", f.rooms);
    }

    #[test]
    fn derived_settings_are_named_in_the_notes() {
        let f = detect(&plan(), 240, 160, &Params::default()).unwrap();
        assert!(f.notes.iter().any(|n| n.contains("wall thickness was taken as")));
        assert!(f.notes.iter().any(|n| n.contains("unconfirmed")));
    }
}
