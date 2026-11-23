//! Accessibility check for persons with disabilities: door clear widths, corridor widths, wheelchair turning space in rooms,
//! ramp slope and width, and threshold heights.
//!
//! No value in this module was read from an accessibility code in this project. Every limit (smallest door clear width, smallest
//! corridor width, smallest turning-circle diameter, steepest ramp slope, smallest ramp width, tallest threshold) is the caller's
//! input, and `source` (the code, edition and clause the limits came from) is required as soon as any limit is given. A limit that is
//! absent leaves its checks `unchecked`, never `passes`. A failure outranks `unchecked` in the overall verdict.
//!
//! What is measured:
//! - Door width: the door's `clear_width_mm` against the limit; a door without a width is `unchecked`.
//! - Threshold: the door's `threshold_height_mm` against the limit; a door without one is `unchecked`.
//! - Corridor width: the shorter side of the corridor's rectangle against the global limit or the corridor's own `min_width_mm`.
//! - Turning space: the shorter side of the room's rectangle (the largest circle that fits an empty rectangle) against the
//!   turning-circle diameter. Furniture, fixtures and door swings are not subtracted, so it is an upper bound of the real space.
//!   A room the caller marks `accessible: false` is left out.
//! - Ramp slope: `rise / run` (a ratio, so 1:12 is 0.0833) against the steepest allowed ratio; ramp width against its minimum.
//!
//! Not computed: stair and handrail geometry, landings, lift cabin sizes, accessible toilet fittings, reach ranges, tactile paving,
//! signage, parking bays and door opening force. A preliminary check; it does not replace an accessibility consultant or the approving
//! authority.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Most items of one kind accepted in a single call.
const MAX_ITEMS: usize = 500;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AccessDoor {
    pub key: String,
    /// Clear opening width, mm. Absent: its width check is `unchecked`.
    #[serde(default)]
    pub clear_width_mm: Option<f64>,
    /// Height of the threshold or level change at the door, mm (0 for a flush door). Absent: `unchecked`.
    #[serde(default)]
    pub threshold_height_mm: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AccessCorridor {
    pub key: String,
    pub w_mm: f64,
    pub h_mm: f64,
    /// This corridor's own minimum width, mm; overrides `min_corridor_width_mm` for it.
    #[serde(default)]
    pub min_width_mm: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AccessRoom {
    pub key: String,
    pub w_mm: f64,
    pub h_mm: f64,
    /// Whether the room must give a wheelchair turning space. Absent: it must (the safe reading); `false` leaves it out.
    #[serde(default)]
    pub accessible: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AccessRamp {
    pub key: String,
    /// Vertical rise, mm (not negative).
    pub rise_mm: f64,
    /// Horizontal run, mm (above 0).
    pub run_mm: f64,
    /// Clear width between handrails or edges, mm. Absent: its width check is `unchecked`.
    #[serde(default)]
    pub width_mm: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AccessInput {
    #[serde(default)]
    pub doors: Vec<AccessDoor>,
    #[serde(default)]
    pub corridors: Vec<AccessCorridor>,
    #[serde(default)]
    pub rooms: Vec<AccessRoom>,
    #[serde(default)]
    pub ramps: Vec<AccessRamp>,
    /// Smallest door clear width, mm.
    #[serde(default)]
    pub min_door_clear_width_mm: Option<f64>,
    /// Tallest threshold, mm (0 allowed: flush only).
    #[serde(default)]
    pub max_threshold_height_mm: Option<f64>,
    /// Smallest corridor width, mm.
    #[serde(default)]
    pub min_corridor_width_mm: Option<f64>,
    /// Smallest wheelchair turning-circle diameter, mm.
    #[serde(default)]
    pub min_turning_diameter_mm: Option<f64>,
    /// Steepest ramp slope as a ratio rise/run (1:12 is 0.0833).
    #[serde(default)]
    pub max_ramp_slope: Option<f64>,
    /// Smallest ramp width, mm.
    #[serde(default)]
    pub min_ramp_width_mm: Option<f64>,
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
    /// `door_width`, `threshold`, `corridor_width`, `turning_space`, `ramp_slope` or `ramp_width`.
    pub rule: &'static str,
    /// Key of the door, corridor, room or ramp the row is about.
    pub subject: String,
    pub status: Status,
    pub value: Option<f64>,
    pub limit: Option<f64>,
    pub source: Option<String>,
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
pub struct AccessReport {
    pub checks: Vec<Check>,
    pub overall: Status,
    pub counts: Counts,
    pub notes: Vec<String>,
}

fn finite_positive(v: f64, what: &str) -> Result<(), String> {
    if v.is_finite() && v > 0.0 {
        Ok(())
    } else {
        Err(format!("accessibility check: {what} must be a finite number above 0 (got {v})"))
    }
}

fn finite_not_negative(v: f64, what: &str) -> Result<(), String> {
    if v.is_finite() && v >= 0.0 {
        Ok(())
    } else {
        Err(format!("accessibility check: {what} must be a finite number, 0 or more (got {v})"))
    }
}

fn unique_keys<'a>(keys: impl Iterator<Item = &'a str>, what: &str) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for k in keys {
        if k.trim().is_empty() {
            return Err(format!("accessibility check: a {what} has an empty key"));
        }
        if !seen.insert(k) {
            return Err(format!("accessibility check: {what} key '{k}' is used more than once"));
        }
    }
    Ok(())
}

fn validate(input: &AccessInput) -> Result<(), String> {
    if input.doors.is_empty() && input.corridors.is_empty() && input.rooms.is_empty() && input.ramps.is_empty() {
        return Err("accessibility check: give at least one door, corridor, room or ramp".into());
    }
    if input.doors.len() > MAX_ITEMS || input.corridors.len() > MAX_ITEMS || input.rooms.len() > MAX_ITEMS || input.ramps.len() > MAX_ITEMS {
        return Err(format!("accessibility check: at most {MAX_ITEMS} doors, corridors, rooms and ramps are accepted per call"));
    }
    unique_keys(input.doors.iter().map(|d| d.key.as_str()), "door")?;
    unique_keys(input.corridors.iter().map(|c| c.key.as_str()), "corridor")?;
    unique_keys(input.rooms.iter().map(|r| r.key.as_str()), "room")?;
    unique_keys(input.ramps.iter().map(|r| r.key.as_str()), "ramp")?;
    for d in &input.doors {
        if let Some(w) = d.clear_width_mm {
            finite_positive(w, &format!("door '{}' clear_width_mm", d.key))?;
        }
        if let Some(t) = d.threshold_height_mm {
            finite_not_negative(t, &format!("door '{}' threshold_height_mm", d.key))?;
        }
    }
    for c in &input.corridors {
        finite_positive(c.w_mm, &format!("corridor '{}' w_mm", c.key))?;
        finite_positive(c.h_mm, &format!("corridor '{}' h_mm", c.key))?;
        if let Some(w) = c.min_width_mm {
            finite_positive(w, &format!("corridor '{}' min_width_mm", c.key))?;
        }
    }
    for r in &input.rooms {
        finite_positive(r.w_mm, &format!("room '{}' w_mm", r.key))?;
        finite_positive(r.h_mm, &format!("room '{}' h_mm", r.key))?;
    }
    for r in &input.ramps {
        finite_not_negative(r.rise_mm, &format!("ramp '{}' rise_mm", r.key))?;
        finite_positive(r.run_mm, &format!("ramp '{}' run_mm", r.key))?;
        if let Some(w) = r.width_mm {
            finite_positive(w, &format!("ramp '{}' width_mm", r.key))?;
        }
    }
    for (v, name) in [
        (input.min_door_clear_width_mm, "min_door_clear_width_mm"),
        (input.min_corridor_width_mm, "min_corridor_width_mm"),
        (input.min_turning_diameter_mm, "min_turning_diameter_mm"),
        (input.max_ramp_slope, "max_ramp_slope"),
        (input.min_ramp_width_mm, "min_ramp_width_mm"),
    ] {
        if let Some(v) = v {
            finite_positive(v, name)?;
        }
    }
    if let Some(t) = input.max_threshold_height_mm {
        finite_not_negative(t, "max_threshold_height_mm")?;
    }
    let any_limit = input.min_door_clear_width_mm.is_some()
        || input.max_threshold_height_mm.is_some()
        || input.min_corridor_width_mm.is_some()
        || input.min_turning_diameter_mm.is_some()
        || input.max_ramp_slope.is_some()
        || input.min_ramp_width_mm.is_some()
        || input.corridors.iter().any(|c| c.min_width_mm.is_some());
    if any_limit && input.source.as_deref().is_none_or(|s| s.trim().is_empty()) {
        return Err("accessibility check: source is required when any limit is given (code, edition and clause)".into());
    }
    Ok(())
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

/// What a row is about: its rule, subject, English and Arabic names, and the unit shown after numbers.
struct Row<'a> {
    rule: &'static str,
    subject: &'a str,
    name: (&'a str, &'a str),
    unit: &'a str,
    /// `at_least`: the value must reach the limit; otherwise it must not exceed it.
    at_least: bool,
    /// Decimals shown in the message.
    decimals: usize,
    missing_value: (&'a str, &'a str),
}

fn compare(row: &Row, limit: Option<f64>, source: &Option<String>, value: Option<f64>) -> Check {
    let mut c = Check {
        rule: row.rule,
        subject: row.subject.to_string(),
        status: Status::Unchecked,
        value,
        limit,
        source: limit.and(source.clone()),
        en: String::new(),
        ar: String::new(),
    };
    let (name_en, name_ar) = row.name;
    let Some(lim) = limit else {
        c.en = format!("{name_en} '{}': no limit was entered, so it was not checked", row.subject);
        c.ar = format!("{name_ar} '{}': لم يُدخل حد فلم يُفحص", row.subject);
        return c;
    };
    let Some(v) = value else {
        c.en = format!("{name_en} '{}': {}", row.subject, row.missing_value.0);
        c.ar = format!("{name_ar} '{}': {}", row.subject, row.missing_value.1);
        return c;
    };
    let ok = if row.at_least { v + 1e-9 >= lim } else { v <= lim + 1e-9 };
    c.status = if ok { Status::Passes } else { Status::Fails };
    let (word_en, word_ar) = match (row.at_least, ok) {
        (true, true) => ("meets the minimum", "يحقق الحد الأدنى"),
        (true, false) => ("is below the minimum", "أقل من الحد الأدنى"),
        (false, true) => ("is within the maximum", "ضمن الحد الأقصى"),
        (false, false) => ("exceeds the maximum", "يتجاوز الحد الأقصى"),
    };
    let d = row.decimals;
    let u = row.unit;
    c.en = format!("{name_en} '{}' {v:.d$} {u} {word_en} {lim:.d$} {u}", row.subject);
    c.ar = format!("{name_ar} '{}' {v:.d$} {u} {word_ar} {lim:.d$} {u}", row.subject);
    c
}

pub fn check(input: &AccessInput) -> Result<AccessReport, String> {
    validate(input)?;
    let src = &input.source;
    let mut checks = Vec::new();

    for d in &input.doors {
        checks.push(compare(
            &Row {
                rule: "door_width",
                subject: &d.key,
                name: ("Clear width of door", "العرض الصافي للباب"),
                unit: "mm",
                at_least: true,
                decimals: 0,
                missing_value: ("no clear width was entered for this door, so it was not checked", "لم يُدخل عرض صافٍ لهذا الباب فلم يُفحص"),
            },
            input.min_door_clear_width_mm,
            src,
            d.clear_width_mm,
        ));
        checks.push(compare(
            &Row {
                rule: "threshold",
                subject: &d.key,
                name: ("Threshold height of door", "ارتفاع العتبة للباب"),
                unit: "mm",
                at_least: false,
                decimals: 0,
                missing_value: ("no threshold height was entered for this door, so it was not checked", "لم يُدخل ارتفاع عتبة لهذا الباب فلم يُفحص"),
            },
            input.max_threshold_height_mm,
            src,
            d.threshold_height_mm,
        ));
    }

    for c in &input.corridors {
        checks.push(compare(
            &Row {
                rule: "corridor_width",
                subject: &c.key,
                name: ("Corridor width of", "عرض الممر"),
                unit: "mm",
                at_least: true,
                decimals: 0,
                missing_value: ("the width is unknown", "العرض غير معروف"),
            },
            c.min_width_mm.or(input.min_corridor_width_mm),
            src,
            Some(c.w_mm.min(c.h_mm)),
        ));
    }

    for r in input.rooms.iter().filter(|r| r.accessible != Some(false)) {
        checks.push(compare(
            &Row {
                rule: "turning_space",
                subject: &r.key,
                name: ("Largest turning circle in room", "أكبر دائرة دوران في الغرفة"),
                unit: "mm",
                at_least: true,
                decimals: 0,
                missing_value: ("the room size is unknown", "مقاس الغرفة غير معروف"),
            },
            input.min_turning_diameter_mm,
            src,
            Some(r.w_mm.min(r.h_mm)),
        ));
    }

    for r in &input.ramps {
        checks.push(compare(
            &Row {
                rule: "ramp_slope",
                subject: &r.key,
                name: ("Slope of ramp", "ميل المنحدر"),
                unit: "(rise/run)",
                at_least: false,
                decimals: 4,
                missing_value: ("the slope is unknown", "الميل غير معروف"),
            },
            input.max_ramp_slope,
            src,
            Some(r.rise_mm / r.run_mm),
        ));
        checks.push(compare(
            &Row {
                rule: "ramp_width",
                subject: &r.key,
                name: ("Width of ramp", "عرض المنحدر"),
                unit: "mm",
                at_least: true,
                decimals: 0,
                missing_value: ("no width was entered for this ramp, so it was not checked", "لم يُدخل عرض لهذا المنحدر فلم يُفحص"),
            },
            input.min_ramp_width_mm,
            src,
            r.width_mm,
        ));
    }

    let (counts, overall) = tally(&checks);
    let notes = vec![
        "Turning space is the shorter side of the room rectangle: the largest circle in an empty rectangle. Furniture, fixtures and door swings are not subtracted, so the real space can be smaller.".to_string(),
        "Ramp slope is rise divided by run (1:12 is 0.0833). Every limit is the caller's, with its source; none was read from a code in this project.".to_string(),
        "Not computed: stairs and handrails, landings, lift cabins, accessible toilet fittings, reach ranges, tactile paving, signage, parking bays and door opening force.".to_string(),
        "Preliminary check; it does not replace an accessibility consultant or the approving authority.".to_string(),
    ];
    Ok(AccessReport { checks, overall, counts, notes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn door(key: &str, w: Option<f64>, t: Option<f64>) -> AccessDoor {
        AccessDoor { key: key.into(), clear_width_mm: w, threshold_height_mm: t }
    }

    fn room(key: &str, w: f64, h: f64, accessible: Option<bool>) -> AccessRoom {
        AccessRoom { key: key.into(), w_mm: w, h_mm: h, accessible }
    }

    fn ramp(key: &str, rise: f64, run: f64, width: Option<f64>) -> AccessRamp {
        AccessRamp { key: key.into(), rise_mm: rise, run_mm: run, width_mm: width }
    }

    fn find<'a>(r: &'a AccessReport, rule: &str, subject: &str) -> &'a Check {
        r.checks.iter().find(|c| c.rule == rule && c.subject == subject).unwrap()
    }

    #[test]
    fn doors_are_checked_for_width_and_threshold() {
        let input = AccessInput {
            doors: vec![door("wide", Some(900.0), Some(10.0)), door("narrow", Some(750.0), Some(30.0)), door("blank", None, None)],
            min_door_clear_width_mm: Some(800.0),
            max_threshold_height_mm: Some(13.0),
            source: Some("test table".into()),
            ..Default::default()
        };
        let r = check(&input).unwrap();
        assert_eq!(find(&r, "door_width", "wide").status, Status::Passes);
        assert_eq!(find(&r, "door_width", "narrow").status, Status::Fails);
        assert_eq!(find(&r, "door_width", "blank").status, Status::Unchecked);
        assert_eq!(find(&r, "threshold", "wide").status, Status::Passes);
        assert_eq!(find(&r, "threshold", "narrow").status, Status::Fails);
        assert_eq!(find(&r, "threshold", "blank").status, Status::Unchecked);
        assert_eq!(find(&r, "door_width", "wide").source.as_deref(), Some("test table"));
        assert_eq!(r.overall, Status::Fails);
    }

    #[test]
    fn flush_threshold_of_zero_is_a_valid_limit_and_value() {
        let input = AccessInput {
            doors: vec![door("flush", Some(900.0), Some(0.0)), door("step", Some(900.0), Some(5.0))],
            max_threshold_height_mm: Some(0.0),
            source: Some("t".into()),
            ..Default::default()
        };
        let r = check(&input).unwrap();
        assert_eq!(find(&r, "threshold", "flush").status, Status::Passes);
        assert_eq!(find(&r, "threshold", "step").status, Status::Fails);
    }

    #[test]
    fn turning_space_uses_the_short_side_and_skips_rooms_marked_not_accessible() {
        let input = AccessInput {
            rooms: vec![room("wc", 1500.0, 2400.0, None), room("tight", 1200.0, 3000.0, Some(true)), room("store", 800.0, 800.0, Some(false))],
            min_turning_diameter_mm: Some(1500.0),
            source: Some("t".into()),
            ..Default::default()
        };
        let r = check(&input).unwrap();
        assert_eq!(find(&r, "turning_space", "wc").value, Some(1500.0));
        assert_eq!(find(&r, "turning_space", "wc").status, Status::Passes);
        assert_eq!(find(&r, "turning_space", "tight").status, Status::Fails);
        assert!(r.checks.iter().all(|c| c.subject != "store"));
    }

    #[test]
    fn ramp_slope_is_rise_over_run_and_width_missing_is_unchecked() {
        let input = AccessInput {
            ramps: vec![ramp("gentle", 150.0, 1800.0, Some(1200.0)), ramp("steep", 300.0, 2000.0, None)],
            max_ramp_slope: Some(1.0 / 12.0),
            min_ramp_width_mm: Some(1000.0),
            source: Some("t".into()),
            ..Default::default()
        };
        let r = check(&input).unwrap();
        assert!((find(&r, "ramp_slope", "gentle").value.unwrap() - 150.0 / 1800.0).abs() < 1e-12);
        assert_eq!(find(&r, "ramp_slope", "gentle").status, Status::Passes);
        assert_eq!(find(&r, "ramp_slope", "steep").status, Status::Fails);
        assert_eq!(find(&r, "ramp_width", "gentle").status, Status::Passes);
        assert_eq!(find(&r, "ramp_width", "steep").status, Status::Unchecked);
    }

    #[test]
    fn corridor_width_uses_the_short_side_and_a_per_corridor_override() {
        let input = AccessInput {
            corridors: vec![
                AccessCorridor { key: "c1".into(), w_mm: 20000.0, h_mm: 1200.0, min_width_mm: None },
                AccessCorridor { key: "c2".into(), w_mm: 1200.0, h_mm: 20000.0, min_width_mm: Some(1500.0) },
            ],
            min_corridor_width_mm: Some(1100.0),
            source: Some("t".into()),
            ..Default::default()
        };
        let r = check(&input).unwrap();
        assert_eq!(find(&r, "corridor_width", "c1").status, Status::Passes);
        assert_eq!(find(&r, "corridor_width", "c2").limit, Some(1500.0));
        assert_eq!(find(&r, "corridor_width", "c2").status, Status::Fails);
    }

    #[test]
    fn no_limits_means_unchecked_and_no_source_is_needed() {
        let input = AccessInput {
            doors: vec![door("d", Some(900.0), Some(0.0))],
            rooms: vec![room("r", 2000.0, 2000.0, None)],
            ramps: vec![ramp("p", 100.0, 1200.0, Some(1200.0))],
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
        let input = AccessInput { doors: vec![door("d", Some(900.0), None)], min_door_clear_width_mm: Some(800.0), ..Default::default() };
        assert!(check(&input).unwrap_err().contains("source is required"));
        let blank = AccessInput { source: Some("  ".into()), ..input };
        assert!(check(&blank).unwrap_err().contains("source is required"));
    }

    #[test]
    fn everything_passing_gives_a_passing_verdict() {
        let input = AccessInput {
            doors: vec![door("d", Some(900.0), Some(10.0))],
            corridors: vec![AccessCorridor { key: "c".into(), w_mm: 1500.0, h_mm: 9000.0, min_width_mm: None }],
            rooms: vec![room("r", 2000.0, 2500.0, None)],
            ramps: vec![ramp("p", 100.0, 1500.0, Some(1200.0))],
            min_door_clear_width_mm: Some(800.0),
            max_threshold_height_mm: Some(13.0),
            min_corridor_width_mm: Some(1200.0),
            min_turning_diameter_mm: Some(1500.0),
            max_ramp_slope: Some(0.0833),
            min_ramp_width_mm: Some(1000.0),
            source: Some("t".into()),
        };
        let r = check(&input).unwrap();
        assert_eq!(r.overall, Status::Passes);
        assert_eq!(r.counts, Counts { passes: 6, fails: 0, unchecked: 0 });
    }

    #[test]
    fn bad_inputs_are_refused_by_name() {
        let base = || AccessInput { doors: vec![door("d", Some(900.0), None)], ..Default::default() };
        assert!(check(&AccessInput::default()).unwrap_err().contains("at least one"));
        let mut i = base();
        i.doors.push(door("d", None, None));
        assert!(check(&i).unwrap_err().contains("door key 'd' is used more than once"));
        let mut i = base();
        i.doors[0].clear_width_mm = Some(0.0);
        assert!(check(&i).unwrap_err().contains("door 'd' clear_width_mm"));
        let mut i = base();
        i.doors[0].threshold_height_mm = Some(-1.0);
        assert!(check(&i).unwrap_err().contains("threshold_height_mm"));
        let mut i = base();
        i.ramps.push(ramp("p", 100.0, 0.0, None));
        assert!(check(&i).unwrap_err().contains("ramp 'p' run_mm"));
        let mut i = base();
        i.rooms.push(room("r", f64::NAN, 1000.0, None));
        assert!(check(&i).unwrap_err().contains("room 'r' w_mm"));
        let mut i = base();
        i.max_ramp_slope = Some(-0.1);
        assert!(check(&i).unwrap_err().contains("max_ramp_slope"));
    }
}
