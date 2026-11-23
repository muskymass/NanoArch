//! Stair detail: the risers and treads of one stair, as a numbered step table, a section and a plan.
//!
//! Nothing about the stair is built in. The floor-to-floor rise, the number of risers, the going, the width, the optional
//! intermediate landing and every limit the stair is checked against are the user's entries; every limit comes with a
//! `limits.source` (the code, edition and clause it was read from). A limit that was not entered is `unchecked` and is never
//! reported as passed.
//!
//! Geometry (a straight stair, or a straight stair with one flat landing between two straight flights):
//!
//! - the rise is divided into `riser_count` equal risers; the going is the horizontal distance between two successive risers;
//! - without a landing there are `riser_count - 1` treads (the top riser lands on the upper floor);
//! - with a landing after riser `N` (`landing_after_riser`), the top of riser `N` is the landing, flight 1 has `N - 1` treads,
//!   flight 2 starts with riser `N + 1` and has `riser_count - N - 1` treads, and the run is `(riser_count - 2) x going + landing`;
//! - the run is measured from the face of the first riser to the face of the last one.
//!
//! Checks, each `passes`, `fails` or `unchecked`: `riser` (min / max riser height), `going` (min / max going), `width`,
//! `landing` (landing length), `step_formula` (2R + G between a minimum and a maximum), `pitch` (maximum angle) and `fit` (the
//! run and width inside the model room named by `room`, along `run_axis`).
//!
//! Not computed and not claimed: headroom, handrail and balustrade, winders, quarter-turn landings, non-uniform risers, the
//! structural design of the flight. The result is a preliminary detail from the user's own dimensions, not a shop drawing and
//! not a code check.

use crate::model::{Id, Model};
use crate::permitpack::esc;
use crate::typed::{Level, Space, Typed};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// The most risers accepted in one call.
pub const MAX_RISERS: usize = 200;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Limits {
    pub min_riser_mm: Option<f64>,
    pub max_riser_mm: Option<f64>,
    pub min_going_mm: Option<f64>,
    pub max_going_mm: Option<f64>,
    pub min_width_mm: Option<f64>,
    pub min_landing_mm: Option<f64>,
    /// Lower and upper bound of 2 x riser + going, mm.
    pub min_step_formula_mm: Option<f64>,
    pub max_step_formula_mm: Option<f64>,
    pub max_pitch_deg: Option<f64>,
    /// The code, edition and clause the limits above were read from. Required as soon as one limit is entered.
    pub source: String,
}

impl Limits {
    fn entered(&self) -> Vec<(&'static str, f64)> {
        [
            ("min_riser_mm", self.min_riser_mm),
            ("max_riser_mm", self.max_riser_mm),
            ("min_going_mm", self.min_going_mm),
            ("max_going_mm", self.max_going_mm),
            ("min_width_mm", self.min_width_mm),
            ("min_landing_mm", self.min_landing_mm),
            ("min_step_formula_mm", self.min_step_formula_mm),
            ("max_step_formula_mm", self.max_step_formula_mm),
            ("max_pitch_deg", self.max_pitch_deg),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.map(|v| (k, v)))
        .collect()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Input {
    pub title: String,
    /// Floor-to-floor height the stair climbs, mm.
    pub rise_mm: f64,
    /// Number of risers (steps up, counting the top one onto the upper floor).
    pub riser_count: usize,
    /// Horizontal distance between two successive risers, mm.
    pub going_mm: f64,
    /// Clear width of the flight(s), mm.
    pub width_mm: f64,
    /// Nosing projection past each riser face, mm. Drawn in the plan only; absent means none is drawn.
    pub nosing_mm: Option<f64>,
    /// The landing sits at the top of this riser (1 ..= riser_count - 1). Enter together with `landing_length_mm`.
    pub landing_after_riser: Option<usize>,
    pub landing_length_mm: Option<f64>,
    /// Optional: the model room (requirement key) the stair must fit, with the axis the run follows (`x` or `y`).
    pub room: String,
    pub level: String,
    pub run_axis: String,
    pub limits: Limits,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StepRow {
    /// Riser number, 1 at the bottom.
    pub riser: usize,
    /// Height of the top of this riser above the lower floor, mm.
    pub top_height_mm: f64,
    /// Distance of this riser's face from the face of riser 1, mm.
    pub face_run_mm: f64,
    /// `flight 1`, `landing` (the top of the riser the landing sits on) or `flight 2`; `flight 1` when there is no landing.
    pub surface: &'static str,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FlightRow {
    pub flight: usize,
    pub first_riser: usize,
    pub last_riser: usize,
    pub risers: usize,
    pub treads: usize,
    /// Run of the treads of this flight, mm.
    pub tread_run_mm: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CheckRow {
    pub check: &'static str,
    /// `passes`, `fails` or `unchecked`.
    pub status: &'static str,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Detail {
    pub rise_mm: f64,
    pub riser_count: usize,
    pub riser_height_mm: f64,
    pub going_mm: f64,
    pub width_mm: f64,
    pub tread_count: usize,
    pub landing_length_mm: Option<f64>,
    /// First riser face to last riser face, mm.
    pub total_run_mm: f64,
    pub pitch_deg: f64,
    /// 2 x riser + going, mm.
    pub step_formula_mm: f64,
    pub flights: Vec<FlightRow>,
    pub steps: Vec<StepRow>,
    pub room: String,
    pub checks: Vec<CheckRow>,
    /// `fails` when any check fails, else `passes` when every check ran, else `incomplete`.
    pub overall: &'static str,
    pub svg: String,
    pub notes: Vec<String>,
}

fn pos(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn unchecked(check: &'static str, reason: &str) -> CheckRow {
    CheckRow { check, status: "unchecked", reason: reason.to_string() }
}

/// Checks `value` against an optional inclusive minimum and maximum. Neither entered -> `unchecked`.
fn range(check: &'static str, what: &str, unit: &str, value: f64, min: Option<f64>, max: Option<f64>, source: &str) -> CheckRow {
    if min.is_none() && max.is_none() {
        return unchecked(check, "no limit was entered");
    }
    const EPS: f64 = 1e-9;
    let mut bad: Vec<String> = Vec::new();
    if let Some(m) = min.filter(|m| value < m - EPS) {
        bad.push(format!("{what} {} {unit} is under the {m} {unit} minimum", r1(value)));
    }
    if let Some(m) = max.filter(|m| value > m + EPS) {
        bad.push(format!("{what} {} {unit} is over the {m} {unit} maximum", r1(value)));
    }
    if bad.is_empty() {
        CheckRow { check, status: "passes", reason: format!("{what} {} {unit} is within the entered limit ({source})", r1(value)) }
    } else {
        CheckRow { check, status: "fails", reason: bad.join("; ") }
    }
}

fn find_room(model: &Model, room: &str, level_filter: &str) -> Result<(Space, String), String> {
    let level_names: BTreeMap<Id, String> = model.elements.values().filter(|e| e.kind == Level::KIND).filter_map(|e| Some((e.id, e.typed::<Level>().ok()?.name))).collect();
    if !level_filter.is_empty() && !level_names.values().any(|n| n == level_filter) {
        return Err(format!("stair detail: level `{level_filter}` is not a level of the model"));
    }
    let mut found: Vec<(Space, String)> = Vec::new();
    for e in model.elements.values().filter(|e| e.kind == Space::KIND) {
        let Ok(s) = e.typed::<Space>() else { continue };
        if s.requirement_key != room {
            continue;
        }
        let level = e.level_id.and_then(|id| level_names.get(&id).cloned()).unwrap_or_default();
        if !level_filter.is_empty() && level != level_filter {
            continue;
        }
        found.push((s, level));
    }
    match found.len() {
        0 => Err(format!("stair detail: the model has no room with key `{room}`{}", if level_filter.is_empty() { String::new() } else { format!(" on level `{level_filter}`") })),
        1 => Ok(found.remove(0)),
        n => {
            let names: Vec<String> = found.iter().map(|(_, l)| format!("`{l}`")).collect();
            Err(format!("stair detail: {n} rooms have key `{room}` (levels {}); set level to choose one", names.join(", ")))
        }
    }
}

/// Builds the detail. Refuses, naming each problem, when an input is missing or malformed.
pub fn build(model: &Model, input: &Input) -> Result<Detail, String> {
    let mut errs: Vec<String> = Vec::new();
    if !pos(input.rise_mm) {
        errs.push("rise_mm (the floor-to-floor height, a positive number)".into());
    }
    if input.riser_count == 0 {
        errs.push("riser_count (the number of risers, at least 1)".into());
    } else if input.riser_count > MAX_RISERS {
        errs.push(format!("riser_count must be at most {MAX_RISERS}, got {}", input.riser_count));
    }
    if !pos(input.going_mm) {
        errs.push("going_mm (the horizontal distance between successive risers, a positive number)".into());
    }
    if !pos(input.width_mm) {
        errs.push("width_mm (the flight width, a positive number)".into());
    }
    if input.nosing_mm.is_some_and(|n| !(n.is_finite() && n >= 0.0)) {
        errs.push("nosing_mm must be zero or more".into());
    }
    match (input.landing_after_riser, input.landing_length_mm) {
        (None, None) => {}
        (Some(n), Some(l)) => {
            if n == 0 || n + 1 > input.riser_count {
                errs.push(format!("landing_after_riser must be from 1 to riser_count - 1 ({}), got {n}", input.riser_count.saturating_sub(1)));
            }
            if !pos(l) {
                errs.push("landing_length_mm must be a positive number".into());
            }
        }
        (Some(_), None) => errs.push("landing_length_mm (a landing_after_riser was given without a length)".into()),
        (None, Some(_)) => errs.push("landing_after_riser (a landing_length_mm was given without the riser it sits on)".into()),
    }
    let entered = input.limits.entered();
    for (k, v) in &entered {
        if !pos(*v) {
            errs.push(format!("limits.{k} must be a positive number, got {v}"));
        }
    }
    if !entered.is_empty() && input.limits.source.trim().is_empty() {
        errs.push("limits.source (the code, edition and clause the limits were read from)".into());
    }
    let l = &input.limits;
    for (lo, hi, name) in [(l.min_riser_mm, l.max_riser_mm, "riser"), (l.min_going_mm, l.max_going_mm, "going"), (l.min_step_formula_mm, l.max_step_formula_mm, "step formula")] {
        if let Some((a, b)) = lo.zip(hi).filter(|(a, b)| a > b) {
            errs.push(format!("the {name} minimum ({a}) is above its maximum ({b})"));
        }
    }
    let room_key = input.room.trim();
    let axis = input.run_axis.trim();
    if room_key.is_empty() {
        if !axis.is_empty() || !input.level.trim().is_empty() {
            errs.push("room (run_axis or level was given without the room key)".into());
        }
    } else if !["x", "y"].contains(&axis) {
        errs.push(format!("run_axis must be x or y (the model axis the run follows), got `{axis}`"));
    }
    if !errs.is_empty() {
        return Err(format!("stair detail: {}. Nothing is defaulted and no stair limit is built in; enter them.", errs.join("; ")));
    }

    let rc = input.riser_count;
    let riser_h = input.rise_mm / rc as f64;
    let g = input.going_mm;
    let landing = input.landing_after_riser.zip(input.landing_length_mm);
    let tread_count = if landing.is_some() { rc - 2 } else { rc - 1 };
    let land_len = landing.map_or(0.0, |(_, len)| len);
    let total_run = tread_count as f64 * g + land_len;
    let pitch = (riser_h / g).atan().to_degrees();
    let formula = 2.0 * riser_h + g;

    // Per-riser table and flights.
    let mut steps: Vec<StepRow> = Vec::with_capacity(rc);
    let mut x = 0.0_f64;
    for i in 1..=rc {
        let surface = match landing {
            Some((n, _)) if i == n => "landing",
            Some((n, _)) if i > n => "flight 2",
            _ => "flight 1",
        };
        steps.push(StepRow { riser: i, top_height_mm: r1(i as f64 * riser_h), face_run_mm: r1(x), surface });
        match landing {
            Some((n, len)) if i == n => x += len,
            _ => x += g,
        }
    }
    let flights: Vec<FlightRow> = match landing {
        Some((n, _)) => vec![
            FlightRow { flight: 1, first_riser: 1, last_riser: n, risers: n, treads: n - 1, tread_run_mm: (n - 1) as f64 * g },
            FlightRow { flight: 2, first_riser: n + 1, last_riser: rc, risers: rc - n, treads: rc - n - 1, tread_run_mm: (rc - n - 1) as f64 * g },
        ],
        None => vec![FlightRow { flight: 1, first_riser: 1, last_riser: rc, risers: rc, treads: rc - 1, tread_run_mm: (rc - 1) as f64 * g }],
    };

    let src = l.source.trim();
    let mut checks: Vec<CheckRow> = vec![
        range("riser", "riser height", "mm", riser_h, l.min_riser_mm, l.max_riser_mm, src),
        range("going", "going", "mm", g, l.min_going_mm, l.max_going_mm, src),
        range("width", "width", "mm", input.width_mm, l.min_width_mm, None, src),
    ];
    checks.push(match (landing, l.min_landing_mm) {
        (_, None) => unchecked("landing", "no limit was entered"),
        (None, Some(_)) => unchecked("landing", "a landing limit was entered but this stair has no landing"),
        (Some((_, len)), min) => range("landing", "landing length", "mm", len, min, None, src),
    });
    checks.push(range("step_formula", "2 x riser + going", "mm", formula, l.min_step_formula_mm, l.max_step_formula_mm, src));
    checks.push(range("pitch", "pitch", "deg", pitch, None, l.max_pitch_deg, src));

    let mut room_name = String::new();
    if room_key.is_empty() {
        checks.push(unchecked("fit", "no room was given to fit the stair into"));
    } else {
        let (space, level) = find_room(model, room_key, input.level.trim())?;
        let (rw, rh) = (space.w as f64, space.h as f64);
        if !pos(rw) || !pos(rh) {
            return Err(format!("stair detail: room `{room_key}` has no positive width and depth in the model"));
        }
        let (along, across) = if axis == "x" { (rw, rh) } else { (rh, rw) };
        let mut bad: Vec<String> = Vec::new();
        if total_run > along + 1e-9 {
            bad.push(format!("the run ({} mm) is longer than the room along {axis} ({} mm)", r1(total_run), r1(along)));
        }
        if input.width_mm > across + 1e-9 {
            bad.push(format!("the width ({} mm) is wider than the room across the run ({} mm)", r1(input.width_mm), r1(across)));
        }
        checks.push(if bad.is_empty() {
            CheckRow { check: "fit", status: "passes", reason: format!("the {} x {} mm stair is inside room `{room_key}` ({} x {} mm along x and y, level `{level}`)", r1(total_run), r1(input.width_mm), r1(rw), r1(rh)) }
        } else {
            CheckRow { check: "fit", status: "fails", reason: bad.join("; ") }
        });
        room_name = room_key.to_string();
    }
    let overall = if checks.iter().any(|c| c.status == "fails") {
        "fails"
    } else if checks.iter().any(|c| c.status == "unchecked") {
        "incomplete"
    } else {
        "passes"
    };

    let title = if input.title.trim().is_empty() { "Stair detail".to_string() } else { input.title.trim().to_string() };
    let svg = draw(&title, input, riser_h, total_run, tread_count, landing, &steps);
    let mut notes = vec![
        "Rise, riser count, going, width, landing and every limit with its source are the user's entries; the application holds no stair figure. The risers are equal; the run is measured from the face of the first riser to the face of the last. Not computed: headroom, handrail and balustrade, winders, quarter-turn landings, the structural design of the flight. Preliminary detail, not a shop drawing and not a code check / المقاسات والحدود ومصادرها من إدخال المستخدم؛ لا حساب لارتفاع الرأس ولا الدرابزين ولا الدرج المتدرج؛ تفصيل أولي لا مخطط تنفيذ ولا فحص كود.".to_string(),
    ];
    if input.nosing_mm.is_none() {
        notes.push("No nosing was entered, so none is drawn in the plan / لم يُدخل بروز للدرجة فلا يُرسم.".to_string());
    }
    if overall == "incomplete" {
        let names: Vec<&str> = checks.iter().filter(|c| c.status == "unchecked").map(|c| c.check).collect();
        notes.push(format!("Not checked (never reported as passed): {} / فحوص لم تُجرَ.", names.join(", ")));
    }
    Ok(Detail {
        rise_mm: input.rise_mm,
        riser_count: rc,
        riser_height_mm: r1(riser_h),
        going_mm: g,
        width_mm: input.width_mm,
        tread_count,
        landing_length_mm: landing.map(|(_, len)| len),
        total_run_mm: r1(total_run),
        pitch_deg: (pitch * 100.0).round() / 100.0,
        step_formula_mm: r1(formula),
        flights,
        steps,
        room: room_name,
        checks,
        overall,
        svg,
        notes,
    })
}

/// Section above, plan below, one scale for both so the proportions are true.
fn draw(title: &str, input: &Input, riser_h: f64, run: f64, treads: usize, landing: Option<(usize, f64)>, steps: &[StepRow]) -> String {
    const VIEW: f64 = 760.0;
    const PAD: f64 = 70.0;
    let rise = input.rise_mm;
    let width = input.width_mm;
    let rc = input.riser_count;
    let g = input.going_mm;
    let ext = (run * 0.12).max(g);
    let span = run + 2.0 * ext;
    let s = VIEW / span.max(rise).max(width);
    let tw = span * s + 2.0 * PAD;
    let sec_top = 50.0;
    let sec_h = rise * s;
    let plan_top = sec_top + sec_h + 70.0;
    let plan_h = width * s;
    let th = plan_top + plan_h + 60.0;
    // Section coordinates: x along the run from the first riser face (the lower floor extends to the west), y upward from the lower floor.
    let sx = |x: f64| PAD + (x + ext) * s;
    let sy = |y: f64| sec_top + (rise - y) * s;
    let py = |y: f64| plan_top + y * s;
    let mut o = String::new();
    let _ = write!(o, "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {tw:.1} {th:.1}\" width=\"{tw:.0}\" height=\"{th:.0}\" font-family=\"sans-serif\" font-size=\"12\">");
    let _ = write!(o, "<rect width=\"100%\" height=\"100%\" fill=\"#fff\"/><text x=\"{:.1}\" y=\"22\" font-size=\"15\" font-weight=\"bold\" text-anchor=\"middle\">{}</text>", tw / 2.0, esc(title));
    let _ = write!(o, "<text x=\"{:.1}\" y=\"40\" font-size=\"11\" fill=\"#555\">Section / مقطع</text>", PAD);

    // Section profile: riser up, then tread (or the landing) along.
    let mut path = format!("M {:.1} {:.1}", sx(-ext), sy(0.0));
    let mut x = 0.0_f64;
    let _ = write!(path, " L {:.1} {:.1}", sx(0.0), sy(0.0));
    for i in 1..=rc {
        let y = i as f64 * riser_h;
        let _ = write!(path, " L {:.1} {:.1}", sx(x), sy(y));
        if i == rc {
            break;
        }
        x += match landing {
            Some((n, len)) if i == n => len,
            _ => g,
        };
        let _ = write!(path, " L {:.1} {:.1}", sx(x), sy(y));
    }
    let _ = write!(path, " L {:.1} {:.1}", sx(x + ext), sy(rise));
    let _ = write!(o, "<path d=\"{path}\" fill=\"none\" stroke=\"#222\" stroke-width=\"2.5\"/>");
    for st in steps {
        let (rx, ry) = (st.face_run_mm, st.top_height_mm);
        let _ = write!(o, "<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"10\" fill=\"#a60\" text-anchor=\"end\">{}</text>", sx(rx) - 4.0, sy(ry - riser_h / 2.0) + 3.0, st.riser);
    }
    // Overall dimensions.
    let _ = write!(o, "<line x1=\"{x1:.1}\" y1=\"{y0:.1}\" x2=\"{x1:.1}\" y2=\"{y1:.1}\" stroke=\"#58a\"/><text transform=\"translate({tx:.1} {ty:.1}) rotate(-90)\" text-anchor=\"middle\" fill=\"#58a\">rise {rise} mm ({rc} x {rh} mm)</text>", x1 = sx(run + ext) + 14.0, y0 = sy(0.0), y1 = sy(rise), tx = sx(run + ext) + 28.0, ty = sy(rise / 2.0), rise = r1(rise), rh = r1(riser_h));
    let _ = write!(o, "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" fill=\"#58a\">run {} mm, going {} mm</text>", sx(run / 2.0), sy(0.0) + 22.0, r1(run), r1(g));

    // Plan.
    let _ = write!(o, "<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"11\" fill=\"#555\">Plan / مسقط</text>", PAD, plan_top - 12.0);
    let _ = write!(o, "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" fill=\"#f4f6f8\" stroke=\"#222\" stroke-width=\"2.5\"/>", sx(0.0), py(0.0), run * s, plan_h);
    if let Some((n, len)) = landing {
        let lx = steps.iter().find(|st| st.riser == n).map_or(0.0, |st| st.face_run_mm);
        let _ = write!(o, "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" fill=\"#dde6ee\" stroke=\"#222\"/><text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-size=\"11\">landing {} mm</text>", sx(lx), py(0.0), len * s, plan_h, sx(lx + len / 2.0), py(width / 2.0) + 4.0, r1(len));
    }
    let nosing = input.nosing_mm.filter(|n| *n > 0.0);
    for st in steps {
        let fx = st.face_run_mm;
        let _ = write!(o, "<line x1=\"{x:.1}\" y1=\"{a:.1}\" x2=\"{x:.1}\" y2=\"{b:.1}\" stroke=\"#333\" stroke-width=\"1.2\"/>", x = sx(fx), a = py(0.0), b = py(width));
        if let Some(n) = nosing {
            let _ = write!(o, "<line x1=\"{a:.1}\" y1=\"{y0:.1}\" x2=\"{a:.1}\" y2=\"{y1:.1}\" stroke=\"#a60\" stroke-dasharray=\"3 2\"/>", a = sx(fx - n), y0 = py(0.0), y1 = py(width));
        }
        let next = steps.iter().find(|n| n.riser == st.riser + 1).map_or(run, |n| n.face_run_mm);
        if st.surface != "landing" && st.riser < rc {
            let _ = write!(o, "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-size=\"10\" fill=\"#a60\">{}</text>", sx((fx + next) / 2.0), py(width / 2.0) - 14.0, st.riser);
        }
    }
    // Up arrow along the centre line.
    let cy = py(width / 2.0);
    let _ = write!(o, "<line x1=\"{:.1}\" y1=\"{cy:.1}\" x2=\"{:.1}\" y2=\"{cy:.1}\" stroke=\"#58a\" stroke-width=\"1.5\"/><polygon points=\"{:.1},{cy:.1} {:.1},{:.1} {:.1},{:.1}\" fill=\"#58a\"/><text x=\"{:.1}\" y=\"{:.1}\" font-size=\"11\" fill=\"#58a\">UP</text>", sx(0.0) + 6.0, sx(run) - 6.0, sx(run) - 6.0, sx(run) - 16.0, cy - 5.0, sx(run) - 16.0, cy + 5.0, sx(0.0) + 8.0, cy + 18.0);
    let _ = write!(o, "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\">{} treads, width {} mm</text></svg>", sx(run / 2.0), py(width) + 26.0, treads, r1(width));
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Command, Origin};
    use crate::model::Source;
    use crate::typed::element_of;
    use crate::Document;

    fn space(key: &str, w: i64, h: i64) -> Space {
        Space { func: "stair".into(), requirement_key: key.into(), area: (w * h) as f64 / 1e6, target_area: 0.0, x: 0, y: 0, w, h, boundary: vec![[0, 0], [w, 0], [w, h], [0, h]], net_area: None, glazing_ratio: None, extra: Default::default() }
    }

    fn doc() -> Document {
        let mut d = Document::default();
        let lv = element_of(&Level { name: "Ground".into(), index: 0, extra: Default::default() }, Source::User);
        let lid = lv.id;
        let mut e = element_of(&space("stair", 2800, 5000), Source::User);
        e.level_id = Some(lid);
        d.commit("room", Origin::User, vec![Command::Create(lv), Command::Create(e)]).unwrap();
        d
    }

    fn input() -> Input {
        Input { rise_mm: 3000.0, riser_count: 15, going_mm: 250.0, width_mm: 1200.0, ..Default::default() }
    }

    #[test]
    fn a_straight_flight_has_one_tread_fewer_than_risers_and_equal_risers() {
        let d = build(&doc().model, &input()).unwrap();
        assert_eq!((d.riser_height_mm, d.tread_count, d.total_run_mm), (200.0, 14, 3500.0));
        assert_eq!(d.flights.len(), 1);
        assert_eq!((d.flights[0].risers, d.flights[0].treads), (15, 14));
        assert_eq!((d.steps[0].top_height_mm, d.steps[0].face_run_mm), (200.0, 0.0));
        assert_eq!((d.steps[14].top_height_mm, d.steps[14].face_run_mm), (3000.0, 3500.0));
        assert_eq!(d.step_formula_mm, 650.0);
        assert!((d.pitch_deg - 38.66).abs() < 0.01, "{}", d.pitch_deg);
        assert!(d.svg.starts_with("<svg") && d.svg.ends_with("</svg>") && d.svg.contains("14 treads"));
    }

    #[test]
    fn a_landing_splits_the_flights_and_adds_its_length_to_the_run() {
        let mut i = input();
        i.landing_after_riser = Some(8);
        i.landing_length_mm = Some(1200.0);
        let d = build(&doc().model, &i).unwrap();
        assert_eq!(d.tread_count, 13);
        assert_eq!(d.total_run_mm, 13.0 * 250.0 + 1200.0);
        assert_eq!((d.flights[0].treads, d.flights[1].treads, d.flights[1].first_riser), (7, 6, 9));
        assert_eq!(d.steps[7].surface, "landing");
        assert_eq!((d.steps[7].face_run_mm, d.steps[8].face_run_mm), (1750.0, 2950.0));
        assert_eq!(d.steps[14].face_run_mm, d.total_run_mm);
        assert!(d.svg.contains("landing 1200 mm"));
        i.landing_after_riser = Some(1);
        assert_eq!(build(&doc().model, &i).unwrap().flights[0].treads, 0);
    }

    #[test]
    fn limits_are_unchecked_unless_entered_with_a_source_and_never_pass_silently() {
        let d = build(&doc().model, &input()).unwrap();
        assert_eq!(d.overall, "incomplete");
        assert!(d.checks.iter().all(|c| c.status == "unchecked"), "{:?}", d.checks);
        assert!(d.notes.iter().any(|n| n.contains("Not checked")));
        let mut i = input();
        i.limits = Limits { max_riser_mm: Some(190.0), min_going_mm: Some(250.0), min_width_mm: Some(1000.0), min_step_formula_mm: Some(600.0), max_step_formula_mm: Some(700.0), max_pitch_deg: Some(40.0), source: "Brief 3.1, 2026-01-01".into(), ..Default::default() };
        let d = build(&doc().model, &i).unwrap();
        let st = |c: &str| d.checks.iter().find(|x| x.check == c).unwrap().status;
        assert_eq!((st("riser"), st("going"), st("width"), st("step_formula"), st("pitch")), ("fails", "passes", "passes", "passes", "passes"));
        assert_eq!((st("landing"), st("fit")), ("unchecked", "unchecked"));
        assert_eq!(d.overall, "fails");
        i.limits.max_riser_mm = Some(200.0);
        assert_eq!(build(&doc().model, &i).unwrap().checks[0].status, "passes");
    }

    #[test]
    fn the_landing_limit_needs_a_landing_and_the_room_fit_uses_the_run_axis() {
        let mut i = input();
        i.limits = Limits { min_landing_mm: Some(1000.0), source: "Brief".into(), ..Default::default() };
        let c = build(&doc().model, &i).unwrap().checks;
        assert!(c.iter().any(|x| x.check == "landing" && x.status == "unchecked" && x.reason.contains("no landing")));
        i.landing_after_riser = Some(8);
        i.landing_length_mm = Some(900.0);
        assert!(build(&doc().model, &i).unwrap().checks.iter().any(|x| x.check == "landing" && x.status == "fails"));
        let mut i = input();
        i.room = "stair".into();
        i.run_axis = "y".into();
        let d = build(&doc().model, &i).unwrap();
        assert_eq!((d.room.as_str(), d.checks.iter().find(|c| c.check == "fit").unwrap().status), ("stair", "passes"));
        i.run_axis = "x".into();
        let d = build(&doc().model, &i).unwrap();
        let fit = d.checks.iter().find(|c| c.check == "fit").unwrap();
        assert!(fit.status == "fails" && fit.reason.contains("longer than the room"), "{fit:?}");
    }

    #[test]
    fn missing_or_malformed_inputs_are_named_and_unknown_rooms_refused() {
        let e = build(&doc().model, &Input::default()).unwrap_err();
        assert!(e.contains("rise_mm") && e.contains("riser_count") && e.contains("going_mm") && e.contains("width_mm"), "{e}");
        let mut i = input();
        i.landing_after_riser = Some(15);
        i.landing_length_mm = Some(-5.0);
        i.limits.max_riser_mm = Some(190.0);
        i.nosing_mm = Some(-1.0);
        i.room = "stair".into();
        let e = build(&doc().model, &i).unwrap_err();
        for part in ["landing_after_riser must be", "landing_length_mm must be", "limits.source", "nosing_mm", "run_axis"] {
            assert!(e.contains(part), "{part} in {e}");
        }
        let mut i = input();
        i.limits = Limits { min_riser_mm: Some(210.0), max_riser_mm: Some(180.0), source: "Brief".into(), ..Default::default() };
        assert!(build(&doc().model, &i).unwrap_err().contains("above its maximum"));
        let mut i = input();
        i.room = "garage".into();
        i.run_axis = "x".into();
        assert!(build(&doc().model, &i).unwrap_err().contains("no room with key `garage`"));
        let mut i = input();
        i.riser_count = MAX_RISERS + 1;
        assert!(build(&doc().model, &i).unwrap_err().contains("at most"));
    }

    #[test]
    fn text_is_escaped_in_the_drawing_and_nosing_is_drawn_only_when_entered() {
        let mut i = input();
        i.title = "<script>x</script>".into();
        let d = build(&doc().model, &i).unwrap();
        assert!(!d.svg.contains("<script>") && d.svg.contains("&lt;script&gt;"));
        assert!(!d.svg.contains("stroke-dasharray=\"3 2\"") && d.notes.iter().any(|n| n.contains("No nosing")));
        i.nosing_mm = Some(25.0);
        let d = build(&doc().model, &i).unwrap();
        assert!(d.svg.contains("stroke-dasharray=\"3 2\"") && !d.notes.iter().any(|n| n.contains("No nosing")));
    }
}
