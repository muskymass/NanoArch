//! Fire-safety egress check: travel distance to exits, number of exits per room, corridor widths and exit widths.
//!
//! No value in this module was read from a fire or building code in this project. Every limit (largest travel distance, smallest
//! corridor width, smallest exit width, smallest number of exits per room) is the caller's input, and `source` (the code, edition and
//! clause the limits came from) is required as soon as any limit is given. A limit that is absent leaves its checks `unchecked`,
//! never `passes`. A failure outranks `unchecked` in the overall verdict.
//!
//! What is measured:
//! - Travel distance: for each room, the largest straight-line distance from any point of the room's rectangle to the nearest
//!   exit that serves it. The exact maximum is found by clipping the room to the region closest to each exit (a Voronoi cell) and
//!   taking the farthest vertex, so it is not limited to the room's corners (two exits on one wall leave the far side of the room,
//!   not a corner, as the worst point). It is a lower bound of the real walking distance: it ignores walls, doors, furniture and
//!   corridors.
//! - Exits per room: the number of exits that serve the room (the keys in the room's `exits`, or every exit when it lists none).
//! - Corridor width: the shorter side of the corridor's rectangle against the global limit or the corridor's own `min_width_mm`.
//! - Exit width: the exit's `clear_width_mm` against the limit; an exit without a width is `unchecked`.
//!
//! Not computed: occupant load, door swing, stair widths, dead-end length, exit separation, refuge areas, smoke control, emergency
//! lighting and signage. A preliminary check; it does not replace a fire engineer or the approving authority.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Most items of one kind accepted in a single call (guards the quadratic clipping below).
const MAX_ITEMS: usize = 500;
/// Slack, in mm squared, for the half-plane tests of the clipping.
const CLIP_EPS: f64 = 1e-6;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EgressRoom {
    pub key: String,
    pub x_mm: f64,
    pub y_mm: f64,
    pub w_mm: f64,
    pub h_mm: f64,
    /// Keys of the exits serving this room. Absent: every exit serves it. An empty list: no exit serves it.
    #[serde(default)]
    pub exits: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EgressExit {
    pub key: String,
    pub x_mm: f64,
    pub y_mm: f64,
    /// Clear width of the exit, mm. Absent: its width check is `unchecked`.
    #[serde(default)]
    pub clear_width_mm: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Corridor {
    pub key: String,
    pub x_mm: f64,
    pub y_mm: f64,
    pub w_mm: f64,
    pub h_mm: f64,
    /// This corridor's own minimum width, mm; overrides `min_corridor_width_mm` for it (for example a different occupant load).
    #[serde(default)]
    pub min_width_mm: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EgressInput {
    pub rooms: Vec<EgressRoom>,
    #[serde(default)]
    pub exits: Vec<EgressExit>,
    #[serde(default)]
    pub corridors: Vec<Corridor>,
    /// Largest travel distance from any point of a room to an exit, mm. Absent: `unchecked`.
    #[serde(default)]
    pub max_travel_distance_mm: Option<f64>,
    /// Smallest corridor width, mm. Absent (and no per-corridor value): `unchecked`.
    #[serde(default)]
    pub min_corridor_width_mm: Option<f64>,
    /// Smallest clear exit width, mm. Absent: `unchecked`.
    #[serde(default)]
    pub min_exit_width_mm: Option<f64>,
    /// Smallest number of exits serving a room. Absent: `unchecked`.
    #[serde(default)]
    pub min_exits_per_room: Option<u32>,
    /// Code, edition and clause the limits came from. Required when any limit is given.
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Passes,
    Fails,
    Unchecked,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Check {
    /// `travel_distance`, `exit_count`, `corridor_width` or `exit_width`.
    pub rule: &'static str,
    /// Key of the room, corridor or exit the row is about.
    pub subject: String,
    pub status: Status,
    pub value: Option<f64>,
    pub limit: Option<f64>,
    pub source: Option<String>,
    /// Travel distance only: the worst point of the room, mm.
    pub worst_point_mm: Option<(f64, f64)>,
    /// Travel distance only: the exit nearest to the worst point.
    pub nearest_exit: Option<String>,
    pub en: String,
    pub ar: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Counts {
    pub passes: usize,
    pub fails: usize,
    pub unchecked: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EgressReport {
    pub checks: Vec<Check>,
    pub overall: Status,
    pub counts: Counts,
    pub notes: Vec<String>,
}

fn finite_positive(v: f64, what: &str) -> Result<(), String> {
    if v.is_finite() && v > 0.0 {
        Ok(())
    } else {
        Err(format!("egress check: {what} must be a finite number above 0 (got {v})"))
    }
}

fn finite(v: f64, what: &str) -> Result<(), String> {
    if v.is_finite() {
        Ok(())
    } else {
        Err(format!("egress check: {what} must be a finite number (got {v})"))
    }
}

fn unique_keys<'a>(keys: impl Iterator<Item = &'a str>, what: &str) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for k in keys {
        if k.trim().is_empty() {
            return Err(format!("egress check: a {what} has an empty key"));
        }
        if !seen.insert(k) {
            return Err(format!("egress check: {what} key '{k}' is used more than once"));
        }
    }
    Ok(())
}

fn validate(input: &EgressInput) -> Result<(), String> {
    if input.rooms.is_empty() {
        return Err("egress check: rooms is required (at least one room)".into());
    }
    if input.rooms.len() > MAX_ITEMS || input.exits.len() > MAX_ITEMS || input.corridors.len() > MAX_ITEMS {
        return Err(format!("egress check: at most {MAX_ITEMS} rooms, exits and corridors are accepted per call"));
    }
    unique_keys(input.rooms.iter().map(|r| r.key.as_str()), "room")?;
    unique_keys(input.exits.iter().map(|e| e.key.as_str()), "exit")?;
    unique_keys(input.corridors.iter().map(|c| c.key.as_str()), "corridor")?;
    for r in &input.rooms {
        finite(r.x_mm, &format!("room '{}' x_mm", r.key))?;
        finite(r.y_mm, &format!("room '{}' y_mm", r.key))?;
        finite_positive(r.w_mm, &format!("room '{}' w_mm", r.key))?;
        finite_positive(r.h_mm, &format!("room '{}' h_mm", r.key))?;
        if let Some(list) = &r.exits {
            for k in list {
                if !input.exits.iter().any(|e| &e.key == k) {
                    return Err(format!("egress check: room '{}' lists exit '{k}', which is not in exits", r.key));
                }
            }
        }
    }
    for e in &input.exits {
        finite(e.x_mm, &format!("exit '{}' x_mm", e.key))?;
        finite(e.y_mm, &format!("exit '{}' y_mm", e.key))?;
        if let Some(w) = e.clear_width_mm {
            finite_positive(w, &format!("exit '{}' clear_width_mm", e.key))?;
        }
    }
    for c in &input.corridors {
        finite(c.x_mm, &format!("corridor '{}' x_mm", c.key))?;
        finite(c.y_mm, &format!("corridor '{}' y_mm", c.key))?;
        finite_positive(c.w_mm, &format!("corridor '{}' w_mm", c.key))?;
        finite_positive(c.h_mm, &format!("corridor '{}' h_mm", c.key))?;
        if let Some(w) = c.min_width_mm {
            finite_positive(w, &format!("corridor '{}' min_width_mm", c.key))?;
        }
    }
    for (v, name) in [
        (input.max_travel_distance_mm, "max_travel_distance_mm"),
        (input.min_corridor_width_mm, "min_corridor_width_mm"),
        (input.min_exit_width_mm, "min_exit_width_mm"),
    ] {
        if let Some(v) = v {
            finite_positive(v, name)?;
        }
    }
    if input.min_exits_per_room == Some(0) {
        return Err("egress check: min_exits_per_room must be at least 1".into());
    }
    let any_limit = input.max_travel_distance_mm.is_some()
        || input.min_corridor_width_mm.is_some()
        || input.min_exit_width_mm.is_some()
        || input.min_exits_per_room.is_some()
        || input.corridors.iter().any(|c| c.min_width_mm.is_some());
    if any_limit && input.source.as_deref().is_none_or(|s| s.trim().is_empty()) {
        return Err("egress check: source is required when any limit is given (code, edition and clause)".into());
    }
    Ok(())
}

type Pt = (f64, f64);

/// Clip a convex polygon to the half-plane `g(p) = a*x + b*y - c <= 0`.
fn clip(poly: &[Pt], a: f64, b: f64, c: f64) -> Vec<Pt> {
    let g = |p: Pt| a * p.0 + b * p.1 - c;
    let mut out = Vec::with_capacity(poly.len() + 1);
    for i in 0..poly.len() {
        let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
        let (gp, gq) = (g(p), g(q));
        let (p_in, q_in) = (gp <= CLIP_EPS, gq <= CLIP_EPS);
        if p_in {
            out.push(p);
        }
        if p_in != q_in {
            let t = gp / (gp - gq);
            out.push((p.0 + t * (q.0 - p.0), p.1 + t * (q.1 - p.1)));
        }
    }
    out
}

fn dist(a: Pt, b: Pt) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// The largest distance from any point of the rectangle to its nearest site, with the point and the site's index.
/// Inside the cell of site `i` the distance to `i` is convex, so its maximum over the (convex) cell is at a vertex.
fn farthest_from_nearest(rect: [Pt; 4], sites: &[Pt]) -> Option<(f64, Pt, usize)> {
    let mut best: Option<(f64, Pt, usize)> = None;
    for (i, &a) in sites.iter().enumerate() {
        let mut cell = rect.to_vec();
        for (j, &b) in sites.iter().enumerate() {
            if i == j || (a.0 == b.0 && a.1 == b.1) {
                continue;
            }
            // Closer to `a` than to `b`: 2(b-a).p <= |b|^2 - |a|^2.
            cell = clip(&cell, 2.0 * (b.0 - a.0), 2.0 * (b.1 - a.1), (b.0 * b.0 + b.1 * b.1) - (a.0 * a.0 + a.1 * a.1));
            if cell.is_empty() {
                break;
            }
        }
        for &v in &cell {
            let d = dist(v, a);
            if best.is_none_or(|(bd, _, _)| d > bd) {
                best = Some((d, v, i));
            }
        }
    }
    best
}

fn tally(checks: &[Check]) -> (Counts, Status) {
    let mut counts = Counts::default();
    for c in checks {
        match c.status {
            Status::Passes => counts.passes += 1,
            Status::Fails => counts.fails += 1,
            Status::Unchecked => counts.unchecked += 1,
        }
    }
    let overall = if counts.fails > 0 {
        Status::Fails
    } else if counts.unchecked > 0 || checks.is_empty() {
        Status::Unchecked
    } else {
        Status::Passes
    };
    (counts, overall)
}

/// A row with only a limit-versus-value comparison. `at_least` means the value must reach the limit; otherwise it must not exceed it.
#[allow(clippy::too_many_arguments)]
fn compare(
    rule: &'static str,
    subject: &str,
    name: (&str, &str),
    unit: &str,
    limit: Option<f64>,
    source: &Option<String>,
    value: Option<f64>,
    at_least: bool,
    missing_value: (&str, &str),
) -> Check {
    let mut c = Check {
        rule,
        subject: subject.to_string(),
        status: Status::Unchecked,
        value,
        limit,
        source: limit.and(source.clone()),
        worst_point_mm: None,
        nearest_exit: None,
        en: String::new(),
        ar: String::new(),
    };
    let Some(lim) = limit else {
        c.en = format!("{} '{}': no limit was entered, so it was not checked", name.0, subject);
        c.ar = format!("{} '{}': لم يُدخل حد فلم يُفحص", name.1, subject);
        return c;
    };
    let Some(v) = value else {
        c.en = format!("{} '{}': {}", name.0, subject, missing_value.0);
        c.ar = format!("{} '{}': {}", name.1, subject, missing_value.1);
        return c;
    };
    let ok = if at_least { v + 1e-9 >= lim } else { v <= lim + 1e-9 };
    c.status = if ok { Status::Passes } else { Status::Fails };
    let (word_en, word_ar) = match (at_least, ok) {
        (true, true) => ("meets the minimum", "يحقق الحد الأدنى"),
        (true, false) => ("is below the minimum", "أقل من الحد الأدنى"),
        (false, true) => ("is within the maximum", "ضمن الحد الأقصى"),
        (false, false) => ("exceeds the maximum", "يتجاوز الحد الأقصى"),
    };
    c.en = format!("{} '{}' {:.0} {unit} {word_en} {:.0} {unit}", name.0, subject, v, lim);
    c.ar = format!("{} '{}' {:.0} {unit} {word_ar} {:.0} {unit}", name.1, subject, v, lim);
    c
}

pub fn check(input: &EgressInput) -> Result<EgressReport, String> {
    validate(input)?;
    let exit_by_key: BTreeMap<&str, &EgressExit> = input.exits.iter().map(|e| (e.key.as_str(), e)).collect();
    let mut checks = Vec::new();

    for room in &input.rooms {
        let serving: Vec<&EgressExit> = match &room.exits {
            Some(keys) => {
                let unique: BTreeSet<&str> = keys.iter().map(String::as_str).collect();
                unique.into_iter().filter_map(|k| exit_by_key.get(k).copied()).collect()
            }
            None => input.exits.iter().collect(),
        };

        // Travel distance.
        let rect = [
            (room.x_mm, room.y_mm),
            (room.x_mm + room.w_mm, room.y_mm),
            (room.x_mm + room.w_mm, room.y_mm + room.h_mm),
            (room.x_mm, room.y_mm + room.h_mm),
        ];
        let sites: Vec<Pt> = serving.iter().map(|e| (e.x_mm, e.y_mm)).collect();
        let far = farthest_from_nearest(rect, &sites);
        let mut row = compare(
            "travel_distance",
            &room.key,
            ("Travel distance of room", "مسافة الهروب للغرفة"),
            "mm",
            input.max_travel_distance_mm,
            &input.source,
            far.map(|f| f.0),
            false,
            ("no exit serves this room, so the distance cannot be measured", "لا يخدم الغرفة مخرج فلا تُقاس المسافة"),
        );
        if let Some((_, p, i)) = far {
            row.worst_point_mm = Some(p);
            row.nearest_exit = Some(serving[i].key.clone());
        }
        checks.push(row);

        // Exits per room.
        checks.push(compare(
            "exit_count",
            &room.key,
            ("Exits serving room", "عدد المخارج للغرفة"),
            "",
            input.min_exits_per_room.map(f64::from),
            &input.source,
            Some(serving.len() as f64),
            true,
            ("the exit count is unknown", "عدد المخارج غير معروف"),
        ));
    }

    for c in &input.corridors {
        let limit = c.min_width_mm.or(input.min_corridor_width_mm);
        checks.push(compare(
            "corridor_width",
            &c.key,
            ("Corridor width of", "عرض الممر"),
            "mm",
            limit,
            &input.source,
            Some(c.w_mm.min(c.h_mm)),
            true,
            ("the width is unknown", "العرض غير معروف"),
        ));
    }

    for e in &input.exits {
        checks.push(compare(
            "exit_width",
            &e.key,
            ("Clear width of exit", "العرض الصافي للمخرج"),
            "mm",
            input.min_exit_width_mm,
            &input.source,
            e.clear_width_mm,
            true,
            ("no clear width was entered for this exit, so it was not checked", "لم يُدخل عرض صافٍ لهذا المخرج فلم يُفحص"),
        ));
    }

    let (counts, overall) = tally(&checks);
    let notes = vec![
        "Travel distance is the straight-line distance from the worst point of the room to the nearest serving exit: a lower bound of the real walking path (walls, doors, furniture and corridors are ignored).".to_string(),
        "Every limit is the caller's, with its source; none was read from a code in this project. Not computed: occupant load, door swing, stair widths, dead ends, exit separation, refuge areas, smoke control, emergency lighting and signage.".to_string(),
        "Preliminary check; it does not replace a fire engineer or the approving authority.".to_string(),
    ];
    Ok(EgressReport { checks, overall, counts, notes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(key: &str, x: f64, y: f64, w: f64, h: f64) -> EgressRoom {
        EgressRoom { key: key.into(), x_mm: x, y_mm: y, w_mm: w, h_mm: h, exits: None }
    }

    fn exit(key: &str, x: f64, y: f64, width: Option<f64>) -> EgressExit {
        EgressExit { key: key.into(), x_mm: x, y_mm: y, clear_width_mm: width }
    }

    fn find<'a>(r: &'a EgressReport, rule: &str, subject: &str) -> &'a Check {
        r.checks.iter().find(|c| c.rule == rule && c.subject == subject).unwrap()
    }

    #[test]
    fn corner_exit_gives_the_opposite_corner_distance() {
        let input = EgressInput {
            rooms: vec![room("a", 0.0, 0.0, 6000.0, 8000.0)],
            exits: vec![exit("e1", 0.0, 0.0, None)],
            max_travel_distance_mm: Some(12000.0),
            source: Some("test table".into()),
            ..Default::default()
        };
        let r = check(&input).unwrap();
        let t = find(&r, "travel_distance", "a");
        assert!((t.value.unwrap() - 10000.0).abs() < 1e-6);
        assert_eq!(t.status, Status::Passes);
        assert_eq!(t.worst_point_mm, Some((6000.0, 8000.0)));
        assert_eq!(t.nearest_exit.as_deref(), Some("e1"));
        assert_eq!(t.source.as_deref(), Some("test table"));
    }

    #[test]
    fn worst_point_can_be_interior_to_a_wall_not_a_corner() {
        // Exits at both ends of a 10 m x 1 m room: every corner is 1 m or 0 m from an exit, but the middle of the far wall is not.
        let mut input = EgressInput {
            rooms: vec![room("long", 0.0, 0.0, 10000.0, 1000.0)],
            exits: vec![exit("w", 0.0, 0.0, None), exit("e", 10000.0, 0.0, None)],
            max_travel_distance_mm: Some(5000.0),
            source: Some("test".into()),
            ..Default::default()
        };
        let r = check(&input).unwrap();
        let t = find(&r, "travel_distance", "long");
        let expected = (5000.0_f64.powi(2) + 1000.0_f64.powi(2)).sqrt();
        assert!((t.value.unwrap() - expected).abs() < 1e-6, "got {:?}", t.value);
        assert_eq!(t.status, Status::Fails);
        let (x, y) = t.worst_point_mm.unwrap();
        assert!((x - 5000.0).abs() < 1e-6 && (y - 1000.0).abs() < 1e-6);
        input.max_travel_distance_mm = Some(5100.0);
        assert_eq!(find(&check(&input).unwrap(), "travel_distance", "long").status, Status::Passes);
    }

    #[test]
    fn room_exit_list_restricts_the_serving_exits_and_the_count() {
        let input = EgressInput {
            rooms: vec![EgressRoom { exits: Some(vec!["far".into(), "far".into()]), ..room("a", 0.0, 0.0, 4000.0, 4000.0) }],
            exits: vec![exit("near", 0.0, 0.0, None), exit("far", 20000.0, 0.0, None)],
            max_travel_distance_mm: Some(10000.0),
            min_exits_per_room: Some(2),
            source: Some("test".into()),
            ..Default::default()
        };
        let r = check(&input).unwrap();
        assert_eq!(find(&r, "travel_distance", "a").status, Status::Fails);
        assert_eq!(find(&r, "travel_distance", "a").nearest_exit.as_deref(), Some("far"));
        let n = find(&r, "exit_count", "a");
        assert_eq!(n.value, Some(1.0));
        assert_eq!(n.status, Status::Fails);
        assert_eq!(r.overall, Status::Fails);
    }

    #[test]
    fn no_limits_means_unchecked_and_no_source_is_needed() {
        let input = EgressInput {
            rooms: vec![room("a", 0.0, 0.0, 3000.0, 3000.0)],
            exits: vec![exit("e", 0.0, 0.0, Some(900.0))],
            corridors: vec![Corridor { key: "c".into(), x_mm: 0.0, y_mm: 0.0, w_mm: 1500.0, h_mm: 9000.0, min_width_mm: None }],
            ..Default::default()
        };
        let r = check(&input).unwrap();
        assert_eq!(r.overall, Status::Unchecked);
        assert_eq!(r.counts.passes, 0);
        assert_eq!(r.counts.unchecked, r.checks.len());
        assert!(r.checks.iter().all(|c| c.source.is_none() && c.limit.is_none()));
    }

    #[test]
    fn a_limit_without_a_source_is_refused() {
        let input = EgressInput {
            rooms: vec![room("a", 0.0, 0.0, 3000.0, 3000.0)],
            min_exit_width_mm: Some(900.0),
            ..Default::default()
        };
        assert!(check(&input).unwrap_err().contains("source is required"));
        let blank = EgressInput { source: Some("  ".into()), ..input };
        assert!(check(&blank).unwrap_err().contains("source is required"));
    }

    #[test]
    fn room_with_no_serving_exit_cannot_be_measured_and_fails_the_count() {
        let input = EgressInput {
            rooms: vec![room("a", 0.0, 0.0, 3000.0, 3000.0)],
            max_travel_distance_mm: Some(30000.0),
            min_exits_per_room: Some(1),
            source: Some("test".into()),
            ..Default::default()
        };
        let r = check(&input).unwrap();
        assert_eq!(find(&r, "travel_distance", "a").status, Status::Unchecked);
        assert_eq!(find(&r, "exit_count", "a").status, Status::Fails);
        assert_eq!(r.overall, Status::Fails);
    }

    #[test]
    fn corridor_width_uses_the_short_side_and_a_per_corridor_override() {
        let input = EgressInput {
            rooms: vec![room("a", 0.0, 0.0, 3000.0, 3000.0)],
            corridors: vec![
                Corridor { key: "c1".into(), x_mm: 0.0, y_mm: 0.0, w_mm: 20000.0, h_mm: 1200.0, min_width_mm: None },
                Corridor { key: "c2".into(), x_mm: 0.0, y_mm: 0.0, w_mm: 1200.0, h_mm: 20000.0, min_width_mm: Some(1500.0) },
            ],
            min_corridor_width_mm: Some(1100.0),
            source: Some("test".into()),
            ..Default::default()
        };
        let r = check(&input).unwrap();
        assert_eq!(find(&r, "corridor_width", "c1").value, Some(1200.0));
        assert_eq!(find(&r, "corridor_width", "c1").status, Status::Passes);
        assert_eq!(find(&r, "corridor_width", "c2").limit, Some(1500.0));
        assert_eq!(find(&r, "corridor_width", "c2").status, Status::Fails);
    }

    #[test]
    fn exit_width_missing_is_unchecked_and_narrow_fails() {
        let input = EgressInput {
            rooms: vec![room("a", 0.0, 0.0, 3000.0, 3000.0)],
            exits: vec![exit("ok", 0.0, 0.0, Some(1000.0)), exit("narrow", 3000.0, 0.0, Some(700.0)), exit("nowidth", 0.0, 3000.0, None)],
            min_exit_width_mm: Some(900.0),
            source: Some("test".into()),
            ..Default::default()
        };
        let r = check(&input).unwrap();
        assert_eq!(find(&r, "exit_width", "ok").status, Status::Passes);
        assert_eq!(find(&r, "exit_width", "narrow").status, Status::Fails);
        assert_eq!(find(&r, "exit_width", "nowidth").status, Status::Unchecked);
        assert_eq!(r.overall, Status::Fails);
    }

    #[test]
    fn everything_passing_gives_a_passing_verdict() {
        let input = EgressInput {
            rooms: vec![room("a", 0.0, 0.0, 4000.0, 3000.0)],
            exits: vec![exit("e", 0.0, 0.0, Some(1000.0))],
            corridors: vec![Corridor { key: "c".into(), x_mm: 0.0, y_mm: 0.0, w_mm: 1500.0, h_mm: 9000.0, min_width_mm: None }],
            max_travel_distance_mm: Some(6000.0),
            min_corridor_width_mm: Some(1200.0),
            min_exit_width_mm: Some(900.0),
            min_exits_per_room: Some(1),
            source: Some("test".into()),
        };
        let r = check(&input).unwrap();
        assert_eq!(r.overall, Status::Passes);
        assert_eq!(r.counts, Counts { passes: 4, fails: 0, unchecked: 0 });
    }

    #[test]
    fn bad_inputs_are_refused_by_name() {
        let base = || EgressInput { rooms: vec![room("a", 0.0, 0.0, 3000.0, 3000.0)], ..Default::default() };
        assert!(check(&EgressInput::default()).unwrap_err().contains("rooms is required"));
        let mut i = base();
        i.rooms.push(room("a", 0.0, 0.0, 1000.0, 1000.0));
        assert!(check(&i).unwrap_err().contains("room key 'a' is used more than once"));
        let mut i = base();
        i.rooms[0].w_mm = 0.0;
        assert!(check(&i).unwrap_err().contains("room 'a' w_mm"));
        let mut i = base();
        i.rooms[0].exits = Some(vec!["ghost".into()]);
        assert!(check(&i).unwrap_err().contains("exit 'ghost'"));
        let mut i = base();
        i.exits.push(exit("e", f64::NAN, 0.0, None));
        assert!(check(&i).unwrap_err().contains("exit 'e' x_mm"));
        let mut i = base();
        i.min_exits_per_room = Some(0);
        assert!(check(&i).unwrap_err().contains("min_exits_per_room"));
        let mut i = base();
        i.max_travel_distance_mm = Some(-1.0);
        assert!(check(&i).unwrap_err().contains("max_travel_distance_mm"));
    }
}
