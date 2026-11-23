//! Mezzanines and partial floors: a floor level that covers only part of the plan of the floor below.
//!
//! The user enters the outline of the partial floor as a closed rectilinear polygon in millimetres, in the frame of the
//! placed rooms (the envelope's lower-left corner is the origin), together with the vertical set-out: the height of the
//! double-height space, the mezzanine's finished-floor level above the lower floor and the floor build-up thickness.
//! Nothing is assumed:
//! - an outline that is not a simple polygon, leaves the envelope or has a slanted edge is refused with the reason;
//! - a vertical set-out that is not positive or does not nest (build-up < level < storey height) is refused;
//! - the minimum clear heights and the maximum share of the lower floor are user inputs taken from the code or the
//!   approving authority; without one the matching check is reported as `unchecked`, never as passed.
//!
//! The report gives the exact area, its share of the lower floor plate (the envelope), the clear height below and above the
//! partial floor, the void left open in the lower floor, the lower-floor rooms it covers (with the area), and one segment per
//! outline edge: supported (on the envelope wall or along a lower room's wall) or open to the void, which needs a guard.

use crate::courtyard::{overlap_len, overlaps, rectilinear_rects, validate_outline, Finding, Pt, Rect2};
use crate::layout::{Layout, Rect, Side};
use serde::{Deserialize, Serialize};

/// Vertical set-out and optional limits, all lengths in millimetres.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MezzanineSpec {
    /// Floor-to-floor height of the space the mezzanine sits in (lower finished floor to the ceiling or slab above).
    pub storey_height_mm: i64,
    /// Finished-floor level of the mezzanine above the lower finished floor.
    pub level_mm: i64,
    /// Thickness of the mezzanine floor build-up (structure plus finishes).
    pub slab_mm: i64,
    /// Smallest clear height accepted under the mezzanine; `None` leaves that check `unchecked`.
    #[serde(default)]
    pub min_clear_below_mm: Option<i64>,
    /// Smallest clear height accepted over the mezzanine; `None` leaves that check `unchecked`.
    #[serde(default)]
    pub min_clear_above_mm: Option<i64>,
    /// Largest share (0 to 1) of the lower floor plate the mezzanine may cover; `None` leaves that check `unchecked`.
    #[serde(default)]
    pub max_area_share: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    Pass,
    Fail,
    /// No limit was entered: nothing was compared.
    Unchecked,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Covered {
    /// Key of the lower-floor room under the mezzanine.
    pub room: String,
    pub area_m2: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Edge {
    pub from: Pt,
    pub to: Pt,
    pub length_mm: i64,
    /// The side the outer face of the edge looks towards (`south` is the low-y side).
    pub faces: String,
    /// Length resting on the envelope wall.
    pub on_envelope_mm: i64,
    /// Length resting on the wall line of a lower-floor room.
    pub on_room_mm: i64,
    /// Length with nothing below it to carry a wall: open to the void, a guard is needed.
    pub open_mm: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MezzanineReport {
    pub vertices: usize,
    pub area_m2: f64,
    pub perimeter_m: f64,
    pub bbox: Rect2,
    /// Disjoint rectangles covering the mezzanine exactly.
    pub rects: Vec<Rect2>,
    /// Area of the lower floor plate (the envelope) the share below is measured against.
    pub lower_floor_m2: f64,
    /// Mezzanine area divided by the lower floor plate.
    pub area_share: f64,
    /// Lower floor plate not covered: the double-height void.
    pub void_m2: f64,
    pub clear_below_mm: i64,
    pub clear_above_mm: i64,
    pub covered_rooms: Vec<Covered>,
    pub edges: Vec<Edge>,
    pub open_edge_m: f64,
    pub clear_below_check: Check,
    pub clear_above_check: Check,
    pub share_check: Check,
    pub findings: Vec<Finding>,
}

fn side_name(dx: i64, dy: i64) -> &'static str {
    // The outline is counter-clockwise, so the outside is on the right of the direction of travel.
    let s = match (dx.signum(), dy.signum()) {
        (1, _) => Side::South,
        (-1, _) => Side::North,
        (_, 1) => Side::East,
        _ => Side::West,
    };
    match s {
        Side::South => "south",
        Side::North => "north",
        Side::East => "east",
        Side::West => "west",
    }
}

fn compare(value: i64, min: Option<i64>) -> Check {
    match min {
        None => Check::Unchecked,
        Some(m) if value >= m => Check::Pass,
        Some(_) => Check::Fail,
    }
}

fn validate_spec(spec: &MezzanineSpec) -> Result<(), String> {
    if spec.slab_mm <= 0 || spec.level_mm <= 0 || spec.storey_height_mm <= 0 {
        return Err(format!(
            "mezzanine: storey_height_mm, level_mm and slab_mm must be positive (got {}, {}, {})",
            spec.storey_height_mm, spec.level_mm, spec.slab_mm
        ));
    }
    if spec.slab_mm >= spec.level_mm {
        return Err(format!("mezzanine: the floor build-up ({} mm) must be thinner than the mezzanine level ({} mm)", spec.slab_mm, spec.level_mm));
    }
    if spec.level_mm >= spec.storey_height_mm {
        return Err(format!("mezzanine: the mezzanine level ({} mm) must be below the storey height ({} mm)", spec.level_mm, spec.storey_height_mm));
    }
    for (name, v) in [("min_clear_below_mm", spec.min_clear_below_mm), ("min_clear_above_mm", spec.min_clear_above_mm)] {
        if let Some(m) = v.filter(|m| *m <= 0) {
            return Err(format!("mezzanine: {name} must be positive, not {m} mm"));
        }
    }
    if let Some(s) = spec.max_area_share.filter(|s| !s.is_finite() || *s <= 0.0 || *s > 1.0) {
        return Err(format!("mezzanine: max_area_share must be in (0, 1], not {s}"));
    }
    Ok(())
}

/// Checks a mezzanine or partial floor against the floor below. `outline` lists the vertices in order, either direction,
/// without repeating the first one at the end.
pub fn check_mezzanine(layout: &Layout, envelope: &Rect, outline: &[Pt], spec: &MezzanineSpec) -> Result<MezzanineReport, String> {
    validate_spec(spec)?;
    let poly = validate_outline("mezzanine", envelope, outline)?;
    let n = poly.len();
    let slanted = (0..n).filter(|&i| poly[i].x != poly[(i + 1) % n].x && poly[i].y != poly[(i + 1) % n].y).count();
    if slanted > 0 {
        return Err(format!("mezzanine: {slanted} edge(s) are slanted; a floor outline must use horizontal and vertical edges only"));
    }
    let rects = rectilinear_rects(&poly);
    let area_mm2: i128 = rects.iter().map(|r| r.w as i128 * r.h as i128).sum();
    let area_m2 = area_mm2 as f64 / 1e6;
    let lower_floor_m2 = envelope.w as f64 * envelope.h as f64 / 1e6;
    let area_share = area_m2 / lower_floor_m2;
    let (minx, maxx) = (poly.iter().map(|p| p.x).min().unwrap_or(0), poly.iter().map(|p| p.x).max().unwrap_or(0));
    let (miny, maxy) = (poly.iter().map(|p| p.y).min().unwrap_or(0), poly.iter().map(|p| p.y).max().unwrap_or(0));
    let bbox = Rect2 { x: minx, y: miny, w: maxx - minx, h: maxy - miny };

    let mut covered_rooms = Vec::new();
    for room in layout.rooms.iter().filter(|r| overlaps(&poly, &r.rect)) {
        let r = &room.rect;
        let mm2: i128 = rects
            .iter()
            .map(|q| overlap_len(q.x, q.x + q.w, r.x, r.x + r.w) as i128 * overlap_len(q.y, q.y + q.h, r.y, r.y + r.h) as i128)
            .sum();
        covered_rooms.push(Covered { room: room.key.clone(), area_m2: mm2 as f64 / 1e6 });
    }

    let mut edges = Vec::with_capacity(n);
    let mut perimeter_mm = 0i64;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let vertical = a.x == b.x;
        let (fixed, lo, hi) = if vertical { (a.x, a.y.min(b.y), a.y.max(b.y)) } else { (a.y, a.x.min(b.x), a.x.max(b.x)) };
        let length = hi - lo;
        perimeter_mm += length;
        let env_line = if vertical { [0, envelope.w] } else { [0, envelope.h] };
        let (on_env, on_room) = if env_line.contains(&fixed) {
            (length, 0)
        } else {
            // Rooms are disjoint, so their wall-line contacts along one edge do not double count.
            let touched: i64 = layout
                .rooms
                .iter()
                .map(|room| {
                    let r = &room.rect;
                    let (r_lo, r_hi, lines) = if vertical { (r.y, r.y + r.h, [r.x, r.x + r.w]) } else { (r.x, r.x + r.w, [r.y, r.y + r.h]) };
                    if lines.contains(&fixed) { overlap_len(lo, hi, r_lo, r_hi) } else { 0 }
                })
                .sum();
            (0, touched.min(length))
        };
        edges.push(Edge {
            from: a,
            to: b,
            length_mm: length,
            faces: side_name(b.x - a.x, b.y - a.y).to_string(),
            on_envelope_mm: on_env,
            on_room_mm: on_room,
            open_mm: length - on_env - on_room,
        });
    }
    let open_total: i64 = edges.iter().map(|e| e.open_mm).sum();

    let clear_below_mm = spec.level_mm - spec.slab_mm;
    let clear_above_mm = spec.storey_height_mm - spec.level_mm;
    let clear_below_check = compare(clear_below_mm, spec.min_clear_below_mm);
    let clear_above_check = compare(clear_above_mm, spec.min_clear_above_mm);
    let share_check = match spec.max_area_share {
        None => Check::Unchecked,
        Some(s) if area_share <= s => Check::Pass,
        Some(_) => Check::Fail,
    };

    let mut findings = Vec::new();
    if let (Check::Fail, Some(m)) = (clear_below_check, spec.min_clear_below_mm) {
        findings.push(Finding {
            ar: format!("الارتفاع الصافي تحت الميزانين {clear_below_mm} مم أقل من الحد الأدنى {m} مم."),
            en: format!("Clear height under the mezzanine is {clear_below_mm} mm, below the {m} mm minimum."),
        });
    }
    if let (Check::Fail, Some(m)) = (clear_above_check, spec.min_clear_above_mm) {
        findings.push(Finding {
            ar: format!("الارتفاع الصافي فوق الميزانين {clear_above_mm} مم أقل من الحد الأدنى {m} مم."),
            en: format!("Clear height over the mezzanine is {clear_above_mm} mm, below the {m} mm minimum."),
        });
    }
    if let (Check::Fail, Some(s)) = (share_check, spec.max_area_share) {
        findings.push(Finding {
            ar: format!("الميزانين يغطي {:.1}% من مسطح الدور السفلي وهو فوق الحد الأقصى {:.1}%.", area_share * 100.0, s * 100.0),
            en: format!("The mezzanine covers {:.1}% of the lower floor plate, above the {:.1}% maximum.", area_share * 100.0, s * 100.0),
        });
    }
    for (check, ar, en) in [
        (clear_below_check, "الارتفاع الصافي تحت الميزانين", "clear height under the mezzanine"),
        (clear_above_check, "الارتفاع الصافي فوق الميزانين", "clear height over the mezzanine"),
        (share_check, "نسبة مساحة الميزانين إلى الدور السفلي", "the mezzanine's share of the lower floor"),
    ] {
        if check == Check::Unchecked {
            findings.push(Finding { ar: format!("لم يُدخَل حدّ لـ{ar}: لم يُفحص."), en: format!("No limit was entered for {en}: not checked.") });
        }
    }
    if open_total > 0 {
        findings.push(Finding {
            ar: format!("{:.2} م من محيط الميزانين مفتوح على الفراغ ولا يستند إلى جدار: يلزمه حاجز حماية.", open_total as f64 / 1000.0),
            en: format!("{:.2} m of the mezzanine edge is open to the void with no wall below: it needs a guard.", open_total as f64 / 1000.0),
        });
    }
    Ok(MezzanineReport {
        vertices: n,
        area_m2,
        perimeter_m: perimeter_mm as f64 / 1000.0,
        bbox,
        rects,
        lower_floor_m2,
        area_share,
        void_m2: lower_floor_m2 - area_m2,
        clear_below_mm,
        clear_above_mm,
        covered_rooms,
        edges,
        open_edge_m: open_total as f64 / 1000.0,
        clear_below_check,
        clear_above_check,
        share_check,
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
    const ENV: Rect = Rect { x: 0, y: 0, w: 10000, h: 10000 };
    const BACK: [(i64, i64); 4] = [(0, 5000), (10000, 5000), (10000, 10000), (0, 10000)];
    fn spec() -> MezzanineSpec {
        MezzanineSpec { storey_height_mm: 5400, level_mm: 2800, slab_mm: 300, min_clear_below_mm: None, min_clear_above_mm: None, max_area_share: None }
    }

    #[test]
    fn a_half_plan_mezzanine_has_exact_area_share_void_and_clear_heights() {
        let l = layout(vec![room("shop", 0, 0, 10000, 5000), room("store", 0, 5000, 10000, 5000)]);
        let s = MezzanineSpec { min_clear_below_mm: Some(2300), min_clear_above_mm: Some(2300), max_area_share: Some(0.5), ..spec() };
        let r = check_mezzanine(&l, &ENV, &pts(&BACK), &s).unwrap();
        assert!((r.area_m2 - 50.0).abs() < 1e-9);
        assert!((r.lower_floor_m2 - 100.0).abs() < 1e-9);
        assert!((r.area_share - 0.5).abs() < 1e-12);
        assert!((r.void_m2 - 50.0).abs() < 1e-9);
        assert_eq!((r.clear_below_mm, r.clear_above_mm), (2500, 2600));
        assert_eq!((r.clear_below_check, r.clear_above_check, r.share_check), (Check::Pass, Check::Pass, Check::Pass));
        assert_eq!(r.covered_rooms, vec![Covered { room: "store".into(), area_m2: 50.0 }]);
    }

    #[test]
    fn edges_are_supported_on_the_envelope_and_room_walls_or_open_to_the_void() {
        let l = layout(vec![room("shop", 0, 0, 10000, 5000), room("store", 0, 5000, 10000, 5000)]);
        let r = check_mezzanine(&l, &ENV, &pts(&BACK), &spec()).unwrap();
        assert_eq!(r.edges.len(), 4);
        // The south edge (y = 5000) runs along the wall line between the two rooms, so it is supported.
        let south = r.edges.iter().find(|e| e.faces == "south").unwrap();
        assert_eq!((south.on_envelope_mm, south.on_room_mm, south.open_mm), (0, 10000, 0));
        assert_eq!(r.open_edge_m, 0.0);
        // With no room on that line the same edge is open to the void and the report asks for a guard.
        let bare = layout(vec![room("hall", 0, 0, 4000, 4000)]);
        let r = check_mezzanine(&bare, &ENV, &pts(&BACK), &spec()).unwrap();
        assert_eq!(r.open_edge_m, 10.0);
        assert!(r.findings.iter().any(|f| f.en.contains("needs a guard")));
    }

    #[test]
    fn an_l_shaped_partial_floor_decomposes_exactly() {
        let l = layout(vec![room("hall", 0, 0, 10000, 10000)]);
        let lshape = [(0, 0), (10000, 0), (10000, 4000), (4000, 4000), (4000, 10000), (0, 10000)];
        let r = check_mezzanine(&l, &ENV, &pts(&lshape), &spec()).unwrap();
        assert!((r.area_m2 - 64.0).abs() < 1e-9);
        let sum: f64 = r.rects.iter().map(|q| q.w as f64 * q.h as f64 / 1e6).sum();
        assert!((sum - r.area_m2).abs() < 1e-9);
        assert!((r.covered_rooms[0].area_m2 - 64.0).abs() < 1e-9, "the room under it is covered only where the L lies");
    }

    #[test]
    fn limits_that_are_not_met_fail_and_limits_not_entered_are_unchecked() {
        let l = layout(vec![room("shop", 0, 0, 10000, 10000)]);
        let none = check_mezzanine(&l, &ENV, &pts(&BACK), &spec()).unwrap();
        assert_eq!((none.clear_below_check, none.clear_above_check, none.share_check), (Check::Unchecked, Check::Unchecked, Check::Unchecked));
        assert_eq!(none.findings.iter().filter(|f| f.en.contains("not checked")).count(), 3);
        let tight = MezzanineSpec { min_clear_below_mm: Some(2600), min_clear_above_mm: Some(2700), max_area_share: Some(0.4), ..spec() };
        let r = check_mezzanine(&l, &ENV, &pts(&BACK), &tight).unwrap();
        assert_eq!((r.clear_below_check, r.clear_above_check, r.share_check), (Check::Fail, Check::Fail, Check::Fail));
        assert!(r.findings.iter().any(|f| f.en.contains("Clear height under the mezzanine is 2500 mm, below the 2600 mm")));
        assert!(r.findings.iter().any(|f| f.en.contains("50.0% of the lower floor plate, above the 40.0% maximum")));
    }

    #[test]
    fn invalid_input_is_refused_with_the_reason() {
        let l = layout(vec![room("shop", 0, 0, 10000, 10000)]);
        let e = |v: &[(i64, i64)], s: &MezzanineSpec| check_mezzanine(&l, &ENV, &pts(v), s).unwrap_err();
        assert!(e(&BACK, &MezzanineSpec { slab_mm: 0, ..spec() }).contains("must be positive"));
        assert!(e(&BACK, &MezzanineSpec { slab_mm: 2800, ..spec() }).contains("thinner than the mezzanine level"));
        assert!(e(&BACK, &MezzanineSpec { level_mm: 5400, ..spec() }).contains("below the storey height"));
        assert!(e(&BACK, &MezzanineSpec { min_clear_below_mm: Some(0), ..spec() }).contains("min_clear_below_mm must be positive"));
        assert!(e(&BACK, &MezzanineSpec { max_area_share: Some(1.5), ..spec() }).contains("max_area_share"));
        assert!(e(&BACK, &MezzanineSpec { max_area_share: Some(f64::NAN), ..spec() }).contains("max_area_share"));
        assert!(e(&[(0, 0), (4000, 0), (2000, 3000)], &spec()).contains("slanted"));
        assert!(e(&[(0, 0), (11000, 0), (11000, 100), (0, 100)], &spec()).contains("outside the envelope"));
        assert!(e(&[(0, 0), (4000, 4000), (4000, 0), (0, 4000)], &spec()).contains("crosses or touches itself"));
    }
}
