//! Courtyards of any simple outline (not only rectangles): validation against the envelope and the placed rooms, area and
//! perimeter, a decomposition into rectangles, and how much of the outline each neighbouring room (or the plot edge) touches.
//!
//! The outline is entered by the user as a closed polygon in millimetres, in the same frame as the placed rooms (the
//! envelope's lower-left corner is the origin). Nothing is assumed: an outline that is not a simple polygon, leaves the
//! envelope or covers part of a placed room is refused with the reason, and no area is reported for it.
//!
//! The decomposition into rectangles exists so the rectangle-based parts of the engine (quantities, walls) can take the
//! courtyard piece by piece. It is exact for outlines whose edges are all horizontal or vertical (`rectilinear`); an
//! outline with a slanted edge returns no rectangles and says so, instead of approximating the slant.
//!
//! Contact lengths are measured on horizontal and vertical edges only (a slanted edge cannot share a wall line with a
//! rectangular room); the report counts the slanted edges it did not measure.

use crate::layout::{Layout, Rect};
use serde::Serialize;

/// One vertex of the outline, millimetres.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Pt {
    pub x: i64,
    pub y: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    Rectangle,
    /// Six vertices, all edges horizontal or vertical, one inward corner.
    LShape,
    /// Horizontal and vertical edges only, any other layout (T, U, Z, stepped...).
    Rectilinear,
    /// At least one slanted edge, no inward corner.
    ConvexPolygon,
    /// At least one slanted edge and at least one inward corner.
    ConcavePolygon,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Contact {
    /// Key of the touching room.
    pub room: String,
    /// Length (mm) of the outline that lies along this room's wall line.
    pub length_mm: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub ar: String,
    pub en: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CourtyardReport {
    pub shape: Shape,
    pub vertices: usize,
    pub area_m2: f64,
    pub perimeter_m: f64,
    /// Bounding box of the outline.
    pub bbox: Rect2,
    /// Number of inward (reflex) corners.
    pub reflex_corners: usize,
    /// Edges that are neither horizontal nor vertical.
    pub slanted_edges: usize,
    /// Disjoint rectangles covering the outline exactly; empty when the outline has a slanted edge.
    pub rects: Vec<Rect2>,
    /// Rooms whose wall lines the outline runs along, longest contact first.
    pub contacts: Vec<Contact>,
    /// Length (mm) of the outline lying on the envelope boundary.
    pub on_envelope_mm: i64,
    /// Length (mm) of horizontal and vertical edges that touch no room and not the envelope boundary.
    pub open_mm: i64,
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Rect2 {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

pub(crate) fn cross(o: Pt, a: Pt, b: Pt) -> i128 {
    (a.x - o.x) as i128 * (b.y - o.y) as i128 - (a.y - o.y) as i128 * (b.x - o.x) as i128
}

pub(crate) fn on_segment(a: Pt, b: Pt, p: Pt) -> bool {
    cross(a, b, p) == 0 && p.x >= a.x.min(b.x) && p.x <= a.x.max(b.x) && p.y >= a.y.min(b.y) && p.y <= a.y.max(b.y)
}

/// True when the closed segments share at least one point.
pub(crate) fn segments_touch(a: Pt, b: Pt, c: Pt, d: Pt) -> bool {
    let (d1, d2, d3, d4) = (cross(a, b, c).signum(), cross(a, b, d).signum(), cross(c, d, a).signum(), cross(c, d, b).signum());
    if d1 * d2 < 0 && d3 * d4 < 0 {
        return true;
    }
    on_segment(a, b, c) || on_segment(a, b, d) || on_segment(c, d, a) || on_segment(c, d, b)
}

pub(crate) fn signed_area2(p: &[Pt]) -> i128 {
    (0..p.len()).map(|i| (p[i].x as i128) * (p[(i + 1) % p.len()].y as i128) - (p[(i + 1) % p.len()].x as i128) * (p[i].y as i128)).sum()
}

/// Point strictly inside the polygon (ray casting; the caller guarantees the point is not on the boundary).
fn inside(p: &[Pt], x: f64, y: f64) -> bool {
    let mut c = false;
    for i in 0..p.len() {
        let (a, b) = (p[i], p[(i + 1) % p.len()]);
        let (ay, by) = (a.y as f64, b.y as f64);
        if (ay > y) != (by > y) {
            let xi = a.x as f64 + (y - ay) / (by - ay) * (b.x as f64 - a.x as f64);
            if x < xi {
                c = !c;
            }
        }
    }
    c
}

/// True when the polygon and the rectangle share area (touching along an edge is not overlap).
pub(crate) fn overlaps(p: &[Pt], r: &Rect) -> bool {
    const EPS: f64 = 1e-6;
    let (x0, x1, y0, y1) = (r.x as f64, (r.x + r.w) as f64, r.y as f64, (r.y + r.h) as f64);
    let strictly = |x: f64, y: f64| x > x0 + EPS && x < x1 - EPS && y > y0 + EPS && y < y1 - EPS;
    for i in 0..p.len() {
        let (a, b) = (p[i], p[(i + 1) % p.len()]);
        let (dx, dy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
        let mut ts = vec![0.0, 1.0];
        for (c, origin, delta) in [(x0, a.x as f64, dx), (x1, a.x as f64, dx), (y0, a.y as f64, dy), (y1, a.y as f64, dy)] {
            if delta != 0.0 {
                let t = (c - origin) / delta;
                if t > 0.0 && t < 1.0 {
                    ts.push(t);
                }
            }
        }
        ts.sort_by(f64::total_cmp);
        for w in ts.windows(2) {
            let t = (w[0] + w[1]) / 2.0;
            if strictly(a.x as f64 + dx * t, a.y as f64 + dy * t) {
                return true;
            }
        }
    }
    // No edge enters the open rectangle: it lies wholly inside or wholly outside the outline.
    inside(p, (x0 + x1) / 2.0, (y0 + y1) / 2.0)
}

pub(crate) fn rectilinear_rects(p: &[Pt]) -> Vec<Rect2> {
    let mut ys: Vec<i64> = p.iter().map(|v| v.y).collect();
    ys.sort_unstable();
    ys.dedup();
    let mut rows: Vec<Rect2> = Vec::new();
    for w in ys.windows(2) {
        let (y0, y1) = (w[0], w[1]);
        let mut xs: Vec<i64> = (0..p.len())
            .filter_map(|i| {
                let (a, b) = (p[i], p[(i + 1) % p.len()]);
                (a.x == b.x && a.y.min(b.y) <= y0 && a.y.max(b.y) >= y1).then_some(a.x)
            })
            .collect();
        xs.sort_unstable();
        for pair in xs.chunks(2) {
            if let [x0, x1] = pair {
                rows.push(Rect2 { x: *x0, y: y0, w: x1 - x0, h: y1 - y0 });
            }
        }
    }
    // Merge slabs that continue the same horizontal span.
    let mut out: Vec<Rect2> = Vec::new();
    for r in rows {
        match out.iter_mut().find(|o| o.x == r.x && o.w == r.w && o.y + o.h == r.y) {
            Some(o) => o.h += r.h,
            None => out.push(r),
        }
    }
    out
}

pub(crate) fn overlap_len(a0: i64, a1: i64, b0: i64, b1: i64) -> i64 {
    (a1.min(b1) - a0.max(b0)).max(0)
}

/// Shared by every outline-based check: the envelope must have area, the outline at least 3 distinct vertices inside the
/// envelope, no zero-width spikes, no self-touching, and a non-zero area. Returns the vertices in counter-clockwise order.
pub(crate) fn validate_outline(tag: &str, envelope: &Rect, outline: &[Pt]) -> Result<Vec<Pt>, String> {
    if envelope.w <= 0 || envelope.h <= 0 {
        return Err(format!("{tag}: the envelope must have a positive width and depth, not {} x {} mm", envelope.w, envelope.h));
    }
    let n = outline.len();
    if n < 3 {
        return Err(format!("{tag}: an outline needs at least 3 vertices, {n} given"));
    }
    for (i, v) in outline.iter().enumerate() {
        if v.x < 0 || v.y < 0 || v.x > envelope.w || v.y > envelope.h {
            return Err(format!("{tag}: vertex {} ({}, {}) lies outside the envelope 0..{} x 0..{} mm", i + 1, v.x, v.y, envelope.w, envelope.h));
        }
    }
    for i in 0..n {
        if outline[i] == outline[(i + 1) % n] {
            return Err(format!("{tag}: vertices {} and {} are the same point", i + 1, (i + 1) % n + 1));
        }
    }
    for i in 0..n {
        for j in i + 1..n {
            let (a, b, c, d) = (outline[i], outline[(i + 1) % n], outline[j], outline[(j + 1) % n]);
            if j == i + 1 || (i == 0 && j == n - 1) {
                // Neighbouring edges meet only at their shared vertex: folding back on each other is a degenerate spike.
                let (shared, p, q) = if j == i + 1 { (b, a, d) } else { (a, b, c) };
                if cross(shared, p, q) == 0 && (p.x - shared.x) as i128 * (q.x - shared.x) as i128 + (p.y - shared.y) as i128 * (q.y - shared.y) as i128 > 0 {
                    return Err(format!("{tag}: the edges at vertex {} fold back on themselves (a zero-width spike)", if j == i + 1 { i + 2 } else { 1 }));
                }
            } else if segments_touch(a, b, c, d) {
                return Err(format!("{tag}: the outline crosses or touches itself between edges {} and {}", i + 1, j + 1));
            }
        }
    }
    let twice = signed_area2(outline);
    if twice == 0 {
        return Err(format!("{tag}: the outline encloses no area (all vertices on one line)"));
    }
    let mut poly: Vec<Pt> = outline.to_vec();
    if twice < 0 {
        poly.reverse();
    }
    Ok(poly)
}

/// Checks a courtyard outline against the envelope and the rooms already placed on the level.
/// `outline` lists the vertices in order, either direction, without repeating the first one at the end.
pub fn check_courtyard(layout: &Layout, envelope: &Rect, outline: &[Pt]) -> Result<CourtyardReport, String> {
    let poly = validate_outline("courtyard", envelope, outline)?;
    let n = poly.len();
    let twice = signed_area2(&poly);
    let covered: Vec<&str> = layout.rooms.iter().filter(|r| overlaps(&poly, &r.rect)).map(|r| r.key.as_str()).collect();
    if !covered.is_empty() {
        return Err(format!("courtyard: the outline covers part of the room(s) {}; move the vertices or the rooms", covered.join(", ")));
    }

    let mut slanted = 0;
    let mut reflex = 0;
    for i in 0..n {
        let (p, c, q) = (poly[(i + n - 1) % n], poly[i], poly[(i + 1) % n]);
        if c.x != q.x && c.y != q.y {
            slanted += 1;
        }
        if cross(p, c, q) < 0 {
            reflex += 1;
        }
    }
    let shape = match (slanted, reflex, n) {
        (0, 0, 4) => Shape::Rectangle,
        (0, 1, 6) => Shape::LShape,
        (0, _, _) => Shape::Rectilinear,
        (_, 0, _) => Shape::ConvexPolygon,
        _ => Shape::ConcavePolygon,
    };
    let perimeter_mm: f64 = (0..n).map(|i| (((poly[(i + 1) % n].x - poly[i].x) as f64).powi(2) + ((poly[(i + 1) % n].y - poly[i].y) as f64).powi(2)).sqrt()).sum();
    let (minx, maxx) = (poly.iter().map(|p| p.x).min().unwrap_or(0), poly.iter().map(|p| p.x).max().unwrap_or(0));
    let (miny, maxy) = (poly.iter().map(|p| p.y).min().unwrap_or(0), poly.iter().map(|p| p.y).max().unwrap_or(0));
    let rects = if slanted == 0 { rectilinear_rects(&poly) } else { Vec::new() };

    let mut contacts: Vec<Contact> = Vec::new();
    let (mut on_env, mut open) = (0i64, 0i64);
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if a.x != b.x && a.y != b.y {
            continue;
        }
        let vertical = a.x == b.x;
        let (fixed, lo, hi) = if vertical { (a.x, a.y.min(b.y), a.y.max(b.y)) } else { (a.y, a.x.min(b.x), a.x.max(b.x)) };
        let env_line = if vertical { [0, envelope.w] } else { [0, envelope.h] };
        let mut touched = 0i64;
        if env_line.contains(&fixed) {
            on_env += hi - lo;
            touched = hi - lo;
        } else {
            for room in &layout.rooms {
                let r = &room.rect;
                let (r_lo, r_hi, lines) = if vertical { (r.y, r.y + r.h, [r.x, r.x + r.w]) } else { (r.x, r.x + r.w, [r.y, r.y + r.h]) };
                if lines.contains(&fixed) {
                    let len = overlap_len(lo, hi, r_lo, r_hi);
                    if len > 0 {
                        touched += len;
                        match contacts.iter_mut().find(|c| c.room == room.key) {
                            Some(c) => c.length_mm += len,
                            None => contacts.push(Contact { room: room.key.clone(), length_mm: len }),
                        }
                    }
                }
            }
        }
        open += (hi - lo - touched).max(0);
    }
    contacts.sort_by(|a, b| b.length_mm.cmp(&a.length_mm).then_with(|| a.room.cmp(&b.room)));

    let mut findings = Vec::new();
    if slanted > 0 {
        findings.push(Finding {
            ar: format!("في المحيط {slanted} ضلع مائل: لم يُقَس تلامسه مع الغرف ولم يُفكَّك إلى مستطيلات."),
            en: format!("The outline has {slanted} slanted edge(s): their contact with rooms was not measured and no rectangles were produced."),
        });
    }
    if contacts.is_empty() && on_env == 0 {
        findings.push(Finding { ar: "لا غرفة تفتح على الفناء ولا حدّ له على المبنى: لا يخدم إضاءة ولا تهوية.".to_string(), en: "No room borders the courtyard and none of it lies on the envelope boundary: it serves no room with light or air.".to_string() });
    }
    if open > 0 {
        findings.push(Finding {
            ar: format!("{:.2} م من محيط الفناء لا يلامس غرفة ولا حدّ المبنى (فراغ غير مخصص)، أضف غرفة أو سوّر الحافة.", open as f64 / 1000.0),
            en: format!("{:.2} m of the outline touches no room and not the envelope boundary (unassigned space): place a room there or enclose the edge.", open as f64 / 1000.0),
        });
    }
    Ok(CourtyardReport {
        shape,
        vertices: n,
        area_m2: twice.unsigned_abs() as f64 / 2.0 / 1e6,
        perimeter_m: perimeter_mm / 1000.0,
        bbox: Rect2 { x: minx, y: miny, w: maxx - minx, h: maxy - miny },
        reflex_corners: reflex,
        slanted_edges: slanted,
        rects,
        contacts,
        on_envelope_mm: on_env,
        open_mm: open,
        findings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Breakdown, PlacedRoom, Zone};

    fn room(key: &str, x: i64, y: i64, w: i64, h: i64) -> PlacedRoom {
        let rect = Rect { x, y, w, h };
        PlacedRoom { key: key.into(), func: "bedroom".into(), zone: Zone::Private, needs_light: true, rect, area_m2: rect.area_m2(), target_m2: rect.area_m2(), deviation_pct: 0.0 }
    }
    fn layout(rooms: Vec<PlacedRoom>) -> Layout {
        Layout { rooms, cost: 0.0, breakdown: Breakdown::default(), reserve_m2: 0.0 }
    }
    fn pts(v: &[(i64, i64)]) -> Vec<Pt> {
        v.iter().map(|&(x, y)| Pt { x, y }).collect()
    }
    const ENV: Rect = Rect { x: 0, y: 0, w: 12000, h: 12000 };

    #[test]
    fn an_l_shaped_courtyard_has_exact_area_and_rectangles() {
        // 6 x 6 m square minus the 3 x 3 m upper-right corner = 27 m2.
        let l = layout(vec![]);
        let r = check_courtyard(&l, &ENV, &pts(&[(3000, 3000), (9000, 3000), (9000, 6000), (6000, 6000), (6000, 9000), (3000, 9000)])).unwrap();
        assert_eq!(r.shape, Shape::LShape);
        assert_eq!(r.reflex_corners, 1);
        assert!((r.area_m2 - 27.0).abs() < 1e-9, "{}", r.area_m2);
        assert!((r.perimeter_m - 24.0).abs() < 1e-9);
        let sum: f64 = r.rects.iter().map(|q| q.w as f64 * q.h as f64 / 1e6).sum();
        assert!((sum - r.area_m2).abs() < 1e-9, "rectangles {:?}", r.rects);
        assert_eq!(r.rects.len(), 2);
    }

    #[test]
    fn direction_of_the_outline_does_not_matter() {
        let a = pts(&[(3000, 3000), (9000, 3000), (9000, 6000), (6000, 6000), (6000, 9000), (3000, 9000)]);
        let mut b = a.clone();
        b.reverse();
        let l = layout(vec![]);
        assert_eq!(check_courtyard(&l, &ENV, &a).unwrap().area_m2, check_courtyard(&l, &ENV, &b).unwrap().area_m2);
    }

    #[test]
    fn a_t_shape_is_rectilinear_and_decomposes_exactly() {
        let l = layout(vec![]);
        // Bar 9 x 3 m with a 3 x 4 m stem: 27 + 12 = 39 m2.
        let t = check_courtyard(&l, &ENV, &pts(&[(1000, 1000), (10000, 1000), (10000, 4000), (7000, 4000), (7000, 8000), (4000, 8000), (4000, 4000), (1000, 4000)])).unwrap();
        assert_eq!(t.shape, Shape::Rectilinear);
        assert!((t.area_m2 - 39.0).abs() < 1e-9, "{}", t.area_m2);
        assert_eq!(t.reflex_corners, 2);
        let sum: f64 = t.rects.iter().map(|q| q.w as f64 * q.h as f64 / 1e6).sum();
        assert!((sum - 39.0).abs() < 1e-9);
    }

    #[test]
    fn a_slanted_outline_reports_area_but_no_rectangles() {
        let l = layout(vec![]);
        let r = check_courtyard(&l, &ENV, &pts(&[(2000, 2000), (8000, 2000), (5000, 6000)])).unwrap();
        assert_eq!(r.shape, Shape::ConvexPolygon);
        assert!((r.area_m2 - 12.0).abs() < 1e-9);
        assert_eq!(r.slanted_edges, 2);
        assert!(r.rects.is_empty());
        assert!(r.findings.iter().any(|f| f.en.contains("slanted")));
    }

    #[test]
    fn contacts_are_measured_along_the_wall_lines_of_the_neighbours() {
        let l = layout(vec![room("west", 0, 3000, 3000, 6000), room("south", 3000, 0, 6000, 3000)]);
        let r = check_courtyard(&l, &ENV, &pts(&[(3000, 3000), (9000, 3000), (9000, 9000), (3000, 9000)])).unwrap();
        assert_eq!(r.shape, Shape::Rectangle);
        assert_eq!(r.contacts, vec![Contact { room: "south".into(), length_mm: 6000 }, Contact { room: "west".into(), length_mm: 6000 }]);
        assert_eq!(r.on_envelope_mm, 0);
        assert_eq!(r.open_mm, 12000, "the east and north edges touch nothing");
        assert!(r.findings.iter().any(|f| f.en.contains("unassigned")));
    }

    #[test]
    fn an_edge_on_the_envelope_boundary_is_counted_apart() {
        let l = layout(vec![room("south", 0, 0, 12000, 3000)]);
        let r = check_courtyard(&l, &ENV, &pts(&[(0, 3000), (6000, 3000), (6000, 12000), (0, 12000)])).unwrap();
        assert_eq!(r.on_envelope_mm, 15000, "west edge 9000 mm on x=0 plus north edge 6000 mm on y=12000");
        assert_eq!(r.contacts, vec![Contact { room: "south".into(), length_mm: 6000 }]);
        assert_eq!(r.open_mm, 9000, "only the east edge is open");
    }

    #[test]
    fn invalid_outlines_are_refused_with_the_reason() {
        let l = layout(vec![room("living", 4000, 4000, 2000, 2000)]);
        let e = |v: &[(i64, i64)]| check_courtyard(&l, &ENV, &pts(v)).unwrap_err();
        assert!(e(&[(0, 0), (100, 0)]).contains("at least 3"));
        assert!(e(&[(0, 0), (13000, 0), (13000, 100)]).contains("outside the envelope"));
        assert!(e(&[(1000, 1000), (1000, 1000), (3000, 3000)]).contains("same point"));
        assert!(e(&[(1000, 1000), (2000, 2000), (3000, 3000)]).contains("fold back"), "collinear vertices enclose nothing");
        assert!(e(&[(0, 0), (4000, 4000), (4000, 0), (0, 4000)]).contains("crosses or touches itself"));
        assert!(e(&[(0, 0), (4000, 0), (2000, 0), (2000, 3000)]).contains("fold back"));
        assert!(e(&[(3000, 3000), (7000, 3000), (7000, 7000), (3000, 7000)]).contains("living"), "outline surrounding a room");
        assert!(e(&[(5000, 5000), (8000, 5000), (8000, 8000), (5000, 8000)]).contains("living"), "outline overlapping a corner");
        assert!(e(&[(4500, 4500), (5500, 4500), (5500, 5500), (4500, 5500)]).contains("living"), "outline inside a room");
        assert!(e(&[(0, 5000), (12000, 5000), (12000, 5500), (0, 5500)]).contains("living"), "outline crossing a room");
    }

    #[test]
    fn touching_a_room_along_an_edge_is_not_an_overlap() {
        let l = layout(vec![room("living", 4000, 4000, 2000, 2000)]);
        assert!(check_courtyard(&l, &ENV, &pts(&[(6000, 4000), (9000, 4000), (9000, 6000), (6000, 6000)])).is_ok());
    }
}
