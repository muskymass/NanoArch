//! Approximate room dimensions from ONE phone photo, for the user to review. Everything here is local arithmetic.
//!
//! - **Scale comes from the floor, not from the picture.** The user lays or names a rectangle of known size on the floor
//!   (a sheet, a tile, a marked rectangle) and gives its four corners in the photo, in order around it, with its real width
//!   (corner 1 to 2) and depth (corner 2 to 3) in millimetres. From these four pairs a plane homography is solved
//!   (`Homography::from_reference`) and the floor corners of the room, also picked in the photo, are mapped to millimetres.
//!   No focal length, lens or sensor value is assumed, and no size is guessed from the image content.
//! - **Plane assumption.** Every picked point must lie ON the floor (the line where wall meets floor), and the reference
//!   too. A point picked up on a wall or a ceiling is wrong by an amount this module cannot see; the report says so.
//! - **Always uncertain.** The result is `unconfirmed` and `approximate`. With a pixel tolerance entered by the user (how far
//!   off a tap can be) every measured side gets a `±` bound found by moving each picked point by that tolerance in each
//!   direction and recomputing; with none the bounds are `unchecked`. Nothing is rounded to look exact.
//! - **Refused, with a name:** a degenerate reference (collinear or repeated corners, a reference not convex), a point that
//!   maps behind the camera horizon, fewer than three room corners, a self-crossing room outline, a non-positive size.
//! - **Reviewed.** `accept` returns whole-millimetre width and length only for a four-corner outline that is a rectangle
//!   within the user's angle tolerance, and only with `confirmed: true`; the user may override either value.

use serde::{Deserialize, Serialize};

/// A point in the photo, in pixels (x right, y down).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Px {
    pub x: f64,
    pub y: f64,
}

/// A point on the floor, in millimetres.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Mm {
    pub x: f64,
    pub y: f64,
}

/// Maps photo pixels to floor millimetres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Homography {
    h: [f64; 9],
}

fn finite(p: Px) -> bool {
    p.x.is_finite() && p.y.is_finite()
}

fn cross(o: Px, a: Px, b: Px) -> f64 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

impl Homography {
    /// Solves the 3×3 matrix taking the four photo corners to a `width_mm` × `depth_mm` rectangle with corner 1 at the
    /// origin, corner 2 at `(width, 0)`, corner 3 at `(width, depth)`, corner 4 at `(0, depth)`.
    pub fn from_reference(corners: &[Px; 4], width_mm: f64, depth_mm: f64) -> Result<Homography, String> {
        if !(width_mm.is_finite() && depth_mm.is_finite()) || width_mm <= 0.0 || depth_mm <= 0.0 {
            return Err("the reference rectangle needs a positive width and depth in millimetres".into());
        }
        if !corners.iter().all(|p| finite(*p)) {
            return Err("a reference corner has a value that is not a number".into());
        }
        // The four photo corners must form a convex quadrilateral taken in one direction.
        let signs: Vec<f64> = (0..4).map(|i| cross(corners[i], corners[(i + 1) % 4], corners[(i + 2) % 4])).collect();
        if signs.iter().any(|s| s.abs() < 1e-9) {
            return Err("the reference corners are repeated or in a line: pick the four corners of the rectangle".into());
        }
        if !(signs.iter().all(|s| *s > 0.0) || signs.iter().all(|s| *s < 0.0)) {
            return Err("the reference corners are not in order around a convex shape: pick them 1-2-3-4 around the rectangle".into());
        }
        let dst = [(0.0, 0.0), (width_mm, 0.0), (width_mm, depth_mm), (0.0, depth_mm)];
        // 8 unknowns, h33 = 1: for each pair  h11 x + h12 y + h13 - X(h31 x + h32 y) = X  and the same for Y.
        let mut a = [[0.0f64; 9]; 8];
        for i in 0..4 {
            let (x, y) = (corners[i].x, corners[i].y);
            let (u, v) = dst[i];
            a[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
            a[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
        }
        let h = solve8(a).ok_or("the reference corners cannot fix a scale (the photo is edge-on to the floor)")?;
        Ok(Homography { h: [h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0] })
    }

    /// Floor position of a photo point; refused when it lies on or beyond the horizon line of the floor.
    pub fn map(&self, p: Px) -> Result<Mm, String> {
        if !finite(p) {
            return Err("a picked point has a value that is not a number".into());
        }
        let h = &self.h;
        let w = h[6] * p.x + h[7] * p.y + h[8];
        if !w.is_finite() || w.abs() < 1e-12 {
            return Err("a picked point lies on the horizon of the floor: it cannot be placed".into());
        }
        let x = (h[0] * p.x + h[1] * p.y + h[2]) / w;
        let y = (h[3] * p.x + h[4] * p.y + h[5]) / w;
        if !(x.is_finite() && y.is_finite()) {
            return Err("a picked point cannot be placed on the floor".into());
        }
        Ok(Mm { x, y })
    }

    fn w_sign(&self, p: Px) -> f64 {
        let h = &self.h;
        (h[6] * p.x + h[7] * p.y + h[8]).signum()
    }
}

/// Gaussian elimination with partial pivoting on an 8×9 augmented matrix.
fn solve8(mut a: [[f64; 9]; 8]) -> Option<[f64; 8]> {
    for col in 0..8 {
        let piv = (col..8).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        for r in 0..8 {
            if r != col {
                let f = a[r][col] / a[col][col];
                let pivot_row = a[col];
                for (cell, p) in a[r].iter_mut().zip(pivot_row).skip(col) {
                    *cell -= f * p;
                }
            }
        }
    }
    let mut out = [0.0; 8];
    for i in 0..8 {
        out[i] = a[i][8] / a[i][i];
    }
    out.iter().all(|v| v.is_finite()).then_some(out)
}

/// What the user picked in the photo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capture {
    /// Four corners of the known floor rectangle, in order around it, in photo pixels.
    pub reference: [Px; 4],
    /// Real width (corner 1 to 2) in mm.
    pub reference_width_mm: f64,
    /// Real depth (corner 2 to 3) in mm.
    pub reference_depth_mm: f64,
    /// The room's floor corners in order around it, in photo pixels (3 or more).
    pub corners: Vec<Px>,
    /// How far off a tapped point can be, in pixels. `None`: the bounds are `unchecked`.
    pub pixel_tolerance: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Side {
    pub from: usize,
    pub to: usize,
    pub length_mm: f64,
    /// `±` mm from the pixel tolerance; `None` when no tolerance was given.
    pub plus_minus_mm: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub status: String,
    pub approximate: bool,
    /// Room corners on the floor in mm, relative to reference corner 1.
    pub floor: Vec<Mm>,
    pub sides: Vec<Side>,
    pub area_m2: f64,
    pub area_plus_minus_m2: Option<f64>,
    /// `ok` (tolerance given), or `unchecked` (none given).
    pub bounds: String,
    /// Largest deviation of any corner angle from 90°, when the outline has four corners.
    pub max_right_angle_error_deg: Option<f64>,
    pub notes: Vec<String>,
}

fn polygon_area(p: &[Mm]) -> f64 {
    let mut s = 0.0;
    for i in 0..p.len() {
        let (a, b) = (p[i], p[(i + 1) % p.len()]);
        s += a.x * b.y - b.x * a.y;
    }
    s / 2.0
}

fn segments_cross(a: Mm, b: Mm, c: Mm, d: Mm) -> bool {
    let o = |p: Mm, q: Mm, r: Mm| (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x);
    let (d1, d2, d3, d4) = (o(a, b, c), o(a, b, d), o(c, d, a), o(c, d, b));
    (d1 * d2 < 0.0) && (d3 * d4 < 0.0)
}

fn measure(cap: &Capture, h: &Homography) -> Result<Vec<Mm>, String> {
    let sign = h.w_sign(cap.reference[0]);
    let mut out = Vec::with_capacity(cap.corners.len());
    for (i, p) in cap.corners.iter().enumerate() {
        if !finite(*p) {
            return Err(format!("room corner {} has a value that is not a number", i + 1));
        }
        if h.w_sign(*p) != sign {
            return Err(format!("room corner {} lies beyond the horizon of the floor: it is not on the floor in the photo", i + 1));
        }
        out.push(h.map(*p)?);
    }
    Ok(out)
}

fn side_lengths(floor: &[Mm]) -> Vec<f64> {
    (0..floor.len()).map(|i| { let (a, b) = (floor[i], floor[(i + 1) % floor.len()]); (b.x - a.x).hypot(b.y - a.y) }).collect()
}

/// Measures the room. Refuses, by name, anything it cannot place; never fills a value in.
pub fn capture_room(cap: &Capture) -> Result<Report, String> {
    if cap.corners.len() < 3 {
        return Err("a room needs at least three floor corners picked in the photo".into());
    }
    if let Some(t) = cap.pixel_tolerance && (!t.is_finite() || t <= 0.0) {
        return Err("the pixel tolerance must be a positive number of pixels".into());
    }
    let h = Homography::from_reference(&cap.reference, cap.reference_width_mm, cap.reference_depth_mm)?;
    let floor = measure(cap, &h)?;
    for i in 0..floor.len() {
        for j in (i + 2)..floor.len() {
            if i == 0 && j == floor.len() - 1 {
                continue;
            }
            if segments_cross(floor[i], floor[(i + 1) % floor.len()], floor[j], floor[(j + 1) % floor.len()]) {
                return Err(format!("the room outline crosses itself between corners {}-{} and {}-{}: pick the corners in order around the room", i + 1, i + 2, j + 1, (j + 1) % floor.len() + 1));
            }
        }
    }
    let area = polygon_area(&floor).abs();
    if area <= 0.0 {
        return Err("the room corners enclose no area".into());
    }
    let lens = side_lengths(&floor);

    // Bounds: move every picked point by the tolerance along each axis (both ways) one at a time; the largest change wins.
    let (mut pm_sides, mut pm_area) = (None, None);
    if let Some(t) = cap.pixel_tolerance {
        let mut dl = vec![0.0f64; lens.len()];
        let mut da = 0.0f64;
        let mut variants: Vec<Capture> = Vec::new();
        for (dx, dy) in [(t, 0.0), (-t, 0.0), (0.0, t), (0.0, -t)] {
            for k in 0..4 {
                let mut c = cap.clone();
                c.reference[k].x += dx;
                c.reference[k].y += dy;
                variants.push(c);
            }
            for k in 0..cap.corners.len() {
                let mut c = cap.clone();
                c.corners[k].x += dx;
                c.corners[k].y += dy;
                variants.push(c);
            }
        }
        for v in variants {
            // A shifted point that can no longer be placed makes the bound unknowable: say so rather than skip it.
            let hv = Homography::from_reference(&v.reference, v.reference_width_mm, v.reference_depth_mm)
                .map_err(|e| format!("the pixel tolerance is too large for this photo: {e}"))?;
            let fv = measure(&v, &hv).map_err(|e| format!("the pixel tolerance is too large for this photo: {e}"))?;
            for (i, l) in side_lengths(&fv).iter().enumerate() {
                dl[i] = dl[i].max((l - lens[i]).abs());
            }
            da = da.max((polygon_area(&fv).abs() - area).abs());
        }
        pm_sides = Some(dl);
        pm_area = Some(da);
    }

    let n = floor.len();
    let sides: Vec<Side> = (0..n)
        .map(|i| Side { from: i + 1, to: (i + 1) % n + 1, length_mm: lens[i], plus_minus_mm: pm_sides.as_ref().map(|d| d[i]) })
        .collect();
    let max_err = (n == 4).then(|| {
        (0..4)
            .map(|i| {
                let (p, c, q) = (floor[(i + 3) % 4], floor[i], floor[(i + 1) % 4]);
                let (ux, uy, vx, vy) = (p.x - c.x, p.y - c.y, q.x - c.x, q.y - c.y);
                let cos = ((ux * vx + uy * vy) / (ux.hypot(uy) * vx.hypot(vy))).clamp(-1.0, 1.0);
                (cos.acos().to_degrees() - 90.0).abs()
            })
            .fold(0.0, f64::max)
    });
    let mut notes = vec![
        "approximate: measured from one photo, every picked point is assumed to lie on the floor plane of the reference".to_string(),
        "unconfirmed: confirm or correct the sizes before they enter the brief".to_string(),
    ];
    if cap.pixel_tolerance.is_none() {
        notes.push("no pixel tolerance given: the ± bounds are unchecked".into());
    }
    Ok(Report {
        status: "unconfirmed".into(),
        approximate: true,
        floor,
        sides,
        area_m2: area / 1.0e6,
        area_plus_minus_m2: pm_area.map(|a| a / 1.0e6),
        bounds: if cap.pixel_tolerance.is_some() { "ok" } else { "unchecked" }.into(),
        max_right_angle_error_deg: max_err,
        notes,
    })
}

/// Whole-millimetre room size the user has confirmed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Accepted {
    pub width_mm: i64,
    pub length_mm: i64,
    pub source: String,
}

/// Turns a four-corner report into a width and length. Needs `confirmed`, an angle tolerance from the user, and a
/// rectangular outline within it; the user may override either size.
pub fn accept(report: &Report, confirmed: bool, angle_tolerance_deg: Option<f64>, width_mm: Option<f64>, length_mm: Option<f64>) -> Result<Accepted, String> {
    if !confirmed {
        return Err("the measurement is unconfirmed: the user must confirm it before it is used".into());
    }
    let tol = angle_tolerance_deg.ok_or("'angle_tolerance_deg' is required: how far from a right angle a corner may be")?;
    if !tol.is_finite() || tol <= 0.0 {
        return Err("'angle_tolerance_deg' must be a positive number of degrees".into());
    }
    let err = report.max_right_angle_error_deg.ok_or("only a four-corner outline can become a width and length")?;
    if err > tol {
        return Err(format!("the outline is {err:.1}° from rectangular, over the {tol}° tolerance: re-pick the corners or give the sizes yourself in the brief"));
    }
    let measured_w = (report.sides[0].length_mm + report.sides[2].length_mm) / 2.0;
    let measured_l = (report.sides[1].length_mm + report.sides[3].length_mm) / 2.0;
    let pick = |over: Option<f64>, measured: f64, what: &str| -> Result<(i64, bool), String> {
        let v = over.unwrap_or(measured);
        if !v.is_finite() || v < 500.0 {
            return Err(format!("the {what} is {v:.0} mm: the smallest room side the generator accepts is 500 mm"));
        }
        Ok((v.round() as i64, over.is_some()))
    };
    let (w, wo) = pick(width_mm, measured_w, "width")?;
    let (l, lo) = pick(length_mm, measured_l, "length")?;
    let source = match (wo, lo) {
        (false, false) => "photo measurement, confirmed by the user",
        (true, true) => "entered by the user",
        _ => "photo measurement with one size corrected by the user",
    };
    Ok(Accepted { width_mm: w, length_mm: l, source: source.into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(x: f64, y: f64) -> Px {
        Px { x, y }
    }

    /// A synthetic photo: floor mm (X, Y) -> pixel by a known perspective map, so true sizes are known.
    fn project(x: f64, y: f64) -> Px {
        let (a, b, c, d, e, f, g, hh) = (0.2, 0.05, 100.0, 0.0, 0.12, 80.0, 0.00002, 0.00005);
        let w = g * x + hh * y + 1.0;
        px((a * x + b * y + c) / w, (d * x + e * y + f) / w)
    }

    fn cap(room: &[(f64, f64)], tol: Option<f64>) -> Capture {
        Capture {
            reference: [project(1000.0, 1000.0), project(1600.0, 1000.0), project(1600.0, 1400.0), project(1000.0, 1400.0)],
            reference_width_mm: 600.0,
            reference_depth_mm: 400.0,
            corners: room.iter().map(|&(x, y)| project(x + 1000.0, y + 1000.0)).collect(),
            pixel_tolerance: tol,
        }
    }

    #[test]
    fn a_known_room_is_recovered_from_a_perspective_photo() {
        let r = capture_room(&cap(&[(0.0, 0.0), (4200.0, 0.0), (4200.0, 3100.0), (0.0, 3100.0)], None)).unwrap();
        assert_eq!(r.status, "unconfirmed");
        assert!(r.approximate);
        assert!((r.sides[0].length_mm - 4200.0).abs() < 0.5, "{:?}", r.sides[0]);
        assert!((r.sides[1].length_mm - 3100.0).abs() < 0.5);
        assert!((r.area_m2 - 13.02).abs() < 0.01);
        assert!(r.max_right_angle_error_deg.unwrap() < 0.01);
        assert_eq!(r.bounds, "unchecked");
        assert!(r.sides[0].plus_minus_mm.is_none() && r.area_plus_minus_m2.is_none());
    }

    #[test]
    fn a_pixel_tolerance_gives_bounds_that_grow_with_it() {
        let room = [(0.0, 0.0), (4200.0, 0.0), (4200.0, 3100.0), (0.0, 3100.0)];
        let a = capture_room(&cap(&room, Some(1.0))).unwrap();
        let b = capture_room(&cap(&room, Some(3.0))).unwrap();
        assert_eq!(a.bounds, "ok");
        assert!(a.sides[0].plus_minus_mm.unwrap() > 0.0);
        assert!(b.sides[0].plus_minus_mm.unwrap() > a.sides[0].plus_minus_mm.unwrap());
        assert!(b.area_plus_minus_m2.unwrap() > a.area_plus_minus_m2.unwrap());
    }

    #[test]
    fn bad_input_is_refused_by_name() {
        let room = [(0.0, 0.0), (4200.0, 0.0), (4200.0, 3100.0), (0.0, 3100.0)];
        let mut c = cap(&room, None);
        c.corners.truncate(2);
        assert!(capture_room(&c).unwrap_err().contains("three"));
        let mut c = cap(&room, None);
        c.reference_width_mm = 0.0;
        assert!(capture_room(&c).unwrap_err().contains("positive"));
        let mut c = cap(&room, None);
        c.reference[1] = c.reference[0];
        assert!(capture_room(&c).unwrap_err().contains("repeated or in a line"));
        let mut c = cap(&room, None);
        c.reference.swap(1, 2);
        assert!(capture_room(&c).unwrap_err().contains("in order"));
        let mut c = cap(&room, Some(-1.0));
        assert!(capture_room(&c).unwrap_err().contains("tolerance"));
        c.pixel_tolerance = Some(f64::NAN);
        assert!(capture_room(&c).unwrap_err().contains("tolerance"));
        let bow = cap(&[(0.0, 0.0), (4200.0, 3100.0), (4200.0, 0.0), (0.0, 3100.0)], None);
        assert!(capture_room(&bow).unwrap_err().contains("crosses itself"));
        let mut c = cap(&room, None);
        c.corners[0].x = f64::NAN;
        assert!(capture_room(&c).unwrap_err().contains("not a number"));
    }

    #[test]
    fn a_point_beyond_the_horizon_is_refused() {
        // Perspective terms make w = 1 + 0.00002 X + 0.00005 Y vanish far away; a pixel past that line has the opposite sign.
        let room = [(0.0, 0.0), (4200.0, 0.0), (4200.0, 3100.0), (0.0, 3100.0)];
        let mut c = cap(&room, None);
        c.corners[2] = project(1000.0 - 60000.0, 1000.0 - 60000.0);
        assert!(capture_room(&c).unwrap_err().contains("horizon"));
    }

    #[test]
    fn accept_needs_confirmation_a_tolerance_and_a_rectangle() {
        let r = capture_room(&cap(&[(0.0, 0.0), (4200.0, 0.0), (4200.0, 3100.0), (0.0, 3100.0)], None)).unwrap();
        assert!(accept(&r, false, Some(2.0), None, None).unwrap_err().contains("unconfirmed"));
        assert!(accept(&r, true, None, None, None).unwrap_err().contains("angle_tolerance_deg"));
        let a = accept(&r, true, Some(2.0), None, None).unwrap();
        assert_eq!((a.width_mm, a.length_mm), (4200, 3100));
        assert!(a.source.contains("confirmed"));
        let o = accept(&r, true, Some(2.0), Some(4250.0), None).unwrap();
        assert_eq!((o.width_mm, o.length_mm), (4250, 3100));
        assert!(o.source.contains("corrected"));
        assert!(accept(&r, true, Some(2.0), Some(300.0), None).unwrap_err().contains("500"));
        let skew = capture_room(&cap(&[(0.0, 0.0), (4200.0, 0.0), (4800.0, 3100.0), (0.0, 3100.0)], None)).unwrap();
        assert!(accept(&skew, true, Some(2.0), None, None).unwrap_err().contains("rectangular"));
        let tri = capture_room(&cap(&[(0.0, 0.0), (4200.0, 0.0), (0.0, 3100.0)], None)).unwrap();
        assert!(accept(&tri, true, Some(2.0), None, None).unwrap_err().contains("four-corner"));
    }
}
