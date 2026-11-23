//! Rooms that are not rectangles: L, T and other rectilinear outlines.
//!
//! The user enters the outline of one placed room as a closed polygon in millimetres, in the frame of the placed rooms
//! (the envelope's lower-left corner is the origin). The room named `key` is the one being reshaped: its current rectangle
//! is ignored, every other room is an obstacle. Nothing is assumed:
//! - an outline that is not a simple polygon, leaves the envelope, or covers part of another room is refused with the reason;
//! - an outline with a slanted edge is refused (walls, slabs and quantities in the engine are orthogonal), never approximated;
//! - the minimum wall length is a user input; without it the length check is reported as `unchecked`, not as passed.
//!
//! The report gives the exact area and perimeter, the shape class, a decomposition into disjoint rectangles whose areas sum
//! to the room area (so the rectangle-based quantity code can take the room piece by piece), the area of the bounding box the
//! room leaves empty (the notch), and one wall segment per outline edge with what lies on its outer side: the envelope
//! boundary, another room (with the length shared) or unassigned space.

use crate::courtyard::{cross, overlap_len, overlaps, rectilinear_rects, validate_outline, Finding, Pt, Rect2};
use crate::layout::{Layout, Rect, Side};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomShape {
    Rectangle,
    /// Six vertices, one inward corner.
    LShape,
    /// Eight vertices, two inward corners joined by a stem of equal-length sides on a flat body edge.
    TShape,
    /// Any other horizontal-and-vertical outline (U, Z, plus, stepped...).
    Rectilinear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LengthCheck {
    /// Every wall segment is at least the minimum entered by the user.
    Pass,
    /// At least one segment is shorter.
    Short,
    /// No minimum was entered: nothing was compared.
    Unchecked,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Neighbour {
    pub room: String,
    pub length_mm: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WallSegment {
    pub from: Pt,
    pub to: Pt,
    pub length_mm: i64,
    /// The side the outer face of the segment looks towards (`south` is the low-y side).
    pub faces: String,
    /// Length lying on the envelope boundary.
    pub on_envelope_mm: i64,
    /// Other rooms whose wall lines run along this segment.
    pub shared: Vec<Neighbour>,
    /// Length touching neither the envelope boundary nor a room.
    pub open_mm: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RoomShapeReport {
    pub key: String,
    pub shape: RoomShape,
    pub vertices: usize,
    pub reflex_corners: usize,
    pub area_m2: f64,
    pub perimeter_m: f64,
    pub bbox: Rect2,
    /// Area of the bounding box the room does not occupy.
    pub notch_m2: f64,
    /// Disjoint rectangles covering the room exactly.
    pub rects: Vec<Rect2>,
    pub walls: Vec<WallSegment>,
    pub shortest_wall_mm: i64,
    pub min_wall_mm: Option<i64>,
    pub length_check: LengthCheck,
    pub findings: Vec<Finding>,
}

fn side_of(dx: i64, dy: i64) -> Side {
    // The outline is counter-clockwise, so the outside is on the right of the direction of travel.
    match (dx.signum(), dy.signum()) {
        (1, _) => Side::South,
        (-1, _) => Side::North,
        (_, 1) => Side::East,
        _ => Side::West,
    }
}

fn side_name(s: Side) -> &'static str {
    match s {
        Side::South => "south",
        Side::North => "north",
        Side::East => "east",
        Side::West => "west",
    }
}

fn classify(poly: &[Pt], reflex_idx: &[usize]) -> RoomShape {
    let n = poly.len();
    match (n, reflex_idx.len()) {
        (4, 0) => RoomShape::Rectangle,
        (6, 1) => RoomShape::LShape,
        (8, 2) => {
            let (a, b) = (reflex_idx[0], reflex_idx[1]);
            let gap = (b - a).min(n - (b - a));
            if gap == 3 {
                // The three edges between the inward corners form the stem; equal outer legs keep the body edge flat.
                let (first, last) = if b - a == 3 { (a, b) } else { (b, a) };
                let len = |i: usize| {
                    let (p, q) = (poly[i % n], poly[(i + 1) % n]);
                    (q.x - p.x).abs() + (q.y - p.y).abs()
                };
                if len(first) == len(last + n - 1) {
                    return RoomShape::TShape;
                }
            }
            RoomShape::Rectilinear
        }
        _ => RoomShape::Rectilinear,
    }
}

/// Checks the outline of room `key` against the envelope and the other placed rooms.
/// `outline` lists the vertices in order, either direction, without repeating the first one at the end.
/// `min_wall_mm` is the shortest wall segment the user accepts; `None` leaves the length check `unchecked`.
pub fn check_room_shape(layout: &Layout, envelope: &Rect, key: &str, outline: &[Pt], min_wall_mm: Option<i64>) -> Result<RoomShapeReport, String> {
    if !layout.rooms.iter().any(|r| r.key == key) {
        return Err(format!("room shape: there is no room '{key}' in the plan"));
    }
    if let Some(m) = min_wall_mm.filter(|m| *m <= 0) {
        return Err(format!("room shape: the minimum wall length must be positive, not {m} mm"));
    }
    let poly = validate_outline("room shape", envelope, outline)?;
    let n = poly.len();
    let slanted = (0..n).filter(|&i| poly[i].x != poly[(i + 1) % n].x && poly[i].y != poly[(i + 1) % n].y).count();
    if slanted > 0 {
        return Err(format!("room shape: {slanted} edge(s) are slanted; a room outline must use horizontal and vertical edges only"));
    }
    let others: Vec<_> = layout.rooms.iter().filter(|r| r.key != key).collect();
    let covered: Vec<&str> = others.iter().filter(|r| overlaps(&poly, &r.rect)).map(|r| r.key.as_str()).collect();
    if !covered.is_empty() {
        return Err(format!("room shape: the outline of '{key}' covers part of the room(s) {}; move the vertices or the rooms", covered.join(", ")));
    }

    let reflex_idx: Vec<usize> = (0..n).filter(|&i| cross(poly[(i + n - 1) % n], poly[i], poly[(i + 1) % n]) < 0).collect();
    let shape = classify(&poly, &reflex_idx);
    let twice: i128 = (0..n).map(|i| poly[i].x as i128 * poly[(i + 1) % n].y as i128 - poly[(i + 1) % n].x as i128 * poly[i].y as i128).sum();
    let area_m2 = twice as f64 / 2.0 / 1e6;
    let (minx, maxx) = (poly.iter().map(|p| p.x).min().unwrap_or(0), poly.iter().map(|p| p.x).max().unwrap_or(0));
    let (miny, maxy) = (poly.iter().map(|p| p.y).min().unwrap_or(0), poly.iter().map(|p| p.y).max().unwrap_or(0));
    let bbox = Rect2 { x: minx, y: miny, w: maxx - minx, h: maxy - miny };
    let rects = rectilinear_rects(&poly);

    let mut walls = Vec::with_capacity(n);
    let mut perimeter_mm = 0i64;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let vertical = a.x == b.x;
        let (fixed, lo, hi) = if vertical { (a.x, a.y.min(b.y), a.y.max(b.y)) } else { (a.y, a.x.min(b.x), a.x.max(b.x)) };
        let length = hi - lo;
        perimeter_mm += length;
        let env_line = if vertical { [0, envelope.w] } else { [0, envelope.h] };
        let mut shared: Vec<Neighbour> = Vec::new();
        let (on_env, touched) = if env_line.contains(&fixed) {
            (length, length)
        } else {
            let mut touched = 0;
            for room in &others {
                let r = &room.rect;
                let (r_lo, r_hi, lines) = if vertical { (r.y, r.y + r.h, [r.x, r.x + r.w]) } else { (r.x, r.x + r.w, [r.y, r.y + r.h]) };
                if lines.contains(&fixed) {
                    let len = overlap_len(lo, hi, r_lo, r_hi);
                    if len > 0 {
                        touched += len;
                        shared.push(Neighbour { room: room.key.clone(), length_mm: len });
                    }
                }
            }
            (0, touched)
        };
        walls.push(WallSegment {
            from: a,
            to: b,
            length_mm: length,
            faces: side_name(side_of(b.x - a.x, b.y - a.y)).to_string(),
            on_envelope_mm: on_env,
            shared,
            open_mm: (length - touched).max(0),
        });
    }

    let shortest = walls.iter().map(|w| w.length_mm).min().unwrap_or(0);
    let length_check = match min_wall_mm {
        None => LengthCheck::Unchecked,
        Some(m) if shortest >= m => LengthCheck::Pass,
        Some(_) => LengthCheck::Short,
    };
    let open_total: i64 = walls.iter().map(|w| w.open_mm).sum();
    let mut findings = Vec::new();
    if let (LengthCheck::Short, Some(m)) = (length_check, min_wall_mm) {
        let short: Vec<String> = walls.iter().enumerate().filter(|(_, w)| w.length_mm < m).map(|(i, w)| format!("{} ({} mm)", i + 1, w.length_mm)).collect();
        findings.push(Finding {
            ar: format!("جدران أقصر من الحد الأدنى {m} مم: الضلع {}.", short.join("، ")),
            en: format!("Wall segment(s) shorter than the {m} mm minimum: {}.", short.join(", ")),
        });
    }
    if length_check == LengthCheck::Unchecked {
        findings.push(Finding {
            ar: "لم يُدخَل حدّ أدنى لطول الجدار: لم يُفحص طول الأضلاع.".to_string(),
            en: "No minimum wall length was entered: segment lengths were not checked.".to_string(),
        });
    }
    if open_total > 0 {
        findings.push(Finding {
            ar: format!("{:.2} م من محيط الغرفة لا يلامس غرفة ولا حدّ المبنى (فراغ غير مخصص): ضع غرفة هناك أو أعد رسم المحيط.", open_total as f64 / 1000.0),
            en: format!("{:.2} m of the room outline touches no room and not the envelope boundary (unassigned space): place a room there or redraw the outline.", open_total as f64 / 1000.0),
        });
    }
    let notch_m2 = bbox.w as f64 * bbox.h as f64 / 1e6 - area_m2;
    Ok(RoomShapeReport {
        key: key.to_string(),
        shape,
        vertices: n,
        reflex_corners: reflex_idx.len(),
        area_m2,
        perimeter_m: perimeter_mm as f64 / 1000.0,
        bbox,
        notch_m2,
        rects,
        walls,
        shortest_wall_mm: shortest,
        min_wall_mm,
        length_check,
        findings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Breakdown, PlacedRoom, Zone};

    fn room(key: &str, x: i64, y: i64, w: i64, h: i64) -> PlacedRoom {
        let rect = Rect { x, y, w, h };
        PlacedRoom { key: key.into(), func: "living".into(), zone: Zone::Public, needs_light: true, rect, area_m2: rect.area_m2(), target_m2: rect.area_m2(), deviation_pct: 0.0 }
    }
    fn layout(rooms: Vec<PlacedRoom>) -> Layout {
        Layout { rooms, cost: 0.0, breakdown: Breakdown::default(), reserve_m2: 0.0 }
    }
    fn pts(v: &[(i64, i64)]) -> Vec<Pt> {
        v.iter().map(|&(x, y)| Pt { x, y }).collect()
    }
    const ENV: Rect = Rect { x: 0, y: 0, w: 12000, h: 12000 };
    const L: [(i64, i64); 6] = [(0, 0), (8000, 0), (8000, 4000), (4000, 4000), (4000, 8000), (0, 8000)];

    #[test]
    fn an_l_room_has_exact_area_notch_and_rectangles() {
        let l = layout(vec![room("living", 0, 0, 8000, 8000)]);
        let r = check_room_shape(&l, &ENV, "living", &pts(&L), Some(3000)).unwrap();
        assert_eq!(r.shape, RoomShape::LShape);
        assert_eq!(r.reflex_corners, 1);
        assert!((r.area_m2 - 48.0).abs() < 1e-9, "{}", r.area_m2);
        assert!((r.perimeter_m - 32.0).abs() < 1e-9);
        assert!((r.notch_m2 - 16.0).abs() < 1e-9, "8x8 box minus 48 m2");
        let sum: f64 = r.rects.iter().map(|q| q.w as f64 * q.h as f64 / 1e6).sum();
        assert!((sum - r.area_m2).abs() < 1e-9, "{:?}", r.rects);
        assert_eq!(r.walls.len(), 6);
        assert_eq!(r.shortest_wall_mm, 4000);
        assert_eq!(r.length_check, LengthCheck::Pass);
    }

    #[test]
    fn the_direction_of_the_outline_does_not_change_the_result() {
        let l = layout(vec![room("living", 0, 0, 8000, 8000)]);
        let mut rev = pts(&L);
        rev.reverse();
        let a = check_room_shape(&l, &ENV, "living", &pts(&L), None).unwrap();
        let b = check_room_shape(&l, &ENV, "living", &rev, None).unwrap();
        assert_eq!(a.area_m2, b.area_m2);
        assert_eq!(a.walls.iter().map(|w| w.faces.clone()).collect::<Vec<_>>().len(), 6);
    }

    #[test]
    fn walls_face_outwards_and_touch_the_envelope_and_neighbours() {
        // The notch of the L is filled by the "study" room, so the two inner sides are shared and nothing is open.
        let l = layout(vec![room("living", 0, 0, 8000, 8000), room("study", 4000, 4000, 4000, 4000)]);
        let r = check_room_shape(&l, &ENV, "living", &pts(&L), None).unwrap();
        let faces: Vec<&str> = r.walls.iter().map(|w| w.faces.as_str()).collect();
        assert_eq!(faces, ["south", "east", "north", "east", "north", "west"]);
        assert_eq!(r.walls[0].on_envelope_mm, 8000, "south side on y = 0");
        assert_eq!(r.walls[5].on_envelope_mm, 8000, "west side on x = 0");
        assert_eq!(r.walls[2].shared, vec![Neighbour { room: "study".into(), length_mm: 4000 }]);
        assert_eq!(r.walls[3].shared, vec![Neighbour { room: "study".into(), length_mm: 4000 }]);
        // East side (x = 8000) and the upper end (y = 8000) touch neither the envelope nor a room.
        assert_eq!(r.walls[1].open_mm, 4000);
        assert!(r.findings.iter().any(|f| f.en.contains("unassigned")));
    }

    #[test]
    fn a_t_room_is_recognised_and_decomposes_exactly() {
        let l = layout(vec![room("hall", 1000, 1000, 9000, 7000)]);
        let t = [(1000, 1000), (10000, 1000), (10000, 4000), (7000, 4000), (7000, 8000), (4000, 8000), (4000, 4000), (1000, 4000)];
        let r = check_room_shape(&l, &ENV, "hall", &pts(&t), None).unwrap();
        assert_eq!(r.shape, RoomShape::TShape);
        assert!((r.area_m2 - 39.0).abs() < 1e-9);
        let sum: f64 = r.rects.iter().map(|q| q.w as f64 * q.h as f64 / 1e6).sum();
        assert!((sum - 39.0).abs() < 1e-9);
        // The same T turned on its side is still a T.
        let sideways = [(1000, 1000), (4000, 1000), (4000, 4000), (8000, 4000), (8000, 7000), (4000, 7000), (4000, 10000), (1000, 10000)];
        assert_eq!(check_room_shape(&l, &ENV, "hall", &pts(&sideways), None).unwrap().shape, RoomShape::TShape);
    }

    #[test]
    fn a_stepped_or_z_outline_is_not_called_a_t() {
        let l = layout(vec![room("hall", 0, 0, 6000, 4000)]);
        let z = [(0, 0), (4000, 0), (4000, 2000), (6000, 2000), (6000, 4000), (2000, 4000), (2000, 2000), (0, 2000)];
        assert_eq!(check_room_shape(&l, &ENV, "hall", &pts(&z), None).unwrap().shape, RoomShape::Rectilinear);
        let u = [(0, 0), (6000, 0), (6000, 4000), (4000, 4000), (4000, 2000), (2000, 2000), (2000, 4000), (0, 4000)];
        assert_eq!(check_room_shape(&l, &ENV, "hall", &pts(&u), None).unwrap().shape, RoomShape::Rectilinear);
        let rect = check_room_shape(&l, &ENV, "hall", &pts(&[(0, 0), (6000, 0), (6000, 4000), (0, 4000)]), None).unwrap();
        assert_eq!(rect.shape, RoomShape::Rectangle);
        assert_eq!(rect.notch_m2, 0.0);
    }

    #[test]
    fn the_wall_length_check_is_unchecked_without_a_minimum_and_short_below_it() {
        let l = layout(vec![room("living", 0, 0, 8000, 8000)]);
        let none = check_room_shape(&l, &ENV, "living", &pts(&L), None).unwrap();
        assert_eq!(none.length_check, LengthCheck::Unchecked);
        assert!(none.findings.iter().any(|f| f.en.contains("not checked")));
        let short = check_room_shape(&l, &ENV, "living", &pts(&L), Some(5000)).unwrap();
        assert_eq!(short.length_check, LengthCheck::Short);
        assert!(short.findings.iter().any(|f| f.en.contains("shorter than the 5000 mm minimum")));
    }

    #[test]
    fn invalid_input_is_refused_with_the_reason() {
        let l = layout(vec![room("living", 0, 0, 4000, 4000), room("other", 6000, 6000, 2000, 2000)]);
        let e = |key: &str, v: &[(i64, i64)], m: Option<i64>| check_room_shape(&l, &ENV, key, &pts(v), m).unwrap_err();
        let sq = [(0, 0), (4000, 0), (4000, 4000), (0, 4000)];
        assert!(e("ghost", &sq, None).contains("no room 'ghost'"));
        assert!(e("living", &sq, Some(0)).contains("must be positive"));
        assert!(e("living", &[(0, 0), (4000, 0), (2000, 3000)], None).contains("slanted"));
        assert!(e("living", &[(0, 0), (13000, 0), (13000, 100), (0, 100)], None).contains("outside the envelope"));
        assert!(e("living", &[(0, 0), (4000, 4000), (4000, 0), (0, 4000)], None).contains("crosses or touches itself"));
        assert!(e("living", &[(5000, 5000), (9000, 5000), (9000, 9000), (5000, 9000)], None).contains("other"), "covers another room");
    }

    #[test]
    fn the_room_being_reshaped_does_not_count_as_its_own_obstacle() {
        // The old rectangle of "living" is larger than the new L; it must not be seen as overlapping itself.
        let l = layout(vec![room("living", 0, 0, 12000, 12000)]);
        assert!(check_room_shape(&l, &ENV, "living", &pts(&L), None).is_ok());
    }
}
