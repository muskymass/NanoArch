//! Cable routes and voltage drop.
//!
//! Every value that decides a result is the caller's: the supply (phase-to-neutral voltage, power factor, single or three
//! phase), the cable catalogue (cross-section, resistance and reactance per km, ampacity, and the source of those figures),
//! the derating factor, the termination allowance added to each run, and each circuit's load. Nothing here was read from an
//! electrical code or a manufacturer's catalogue; a missing value is refused by name. The limits (largest voltage drop in
//! percent, clearance to obstacles) are the caller's: a missing drop limit leaves that check `unchecked`, never `ok`.
//!
//! Route: a circuit either lists its own waypoints (`via`) or lets the module try four orthogonal candidates between its two
//! ends, in this fixed order: horizontal then vertical, vertical then horizontal, a Z through the middle x, a Z through the
//! middle y. The first candidate that keeps `clearance_mm` from every obstacle rectangle (the structural columns) is used.
//! A route that crosses an obstacle, or no clear candidate, is reported by obstacle key and the circuit's length and drop
//! are not computed. Length is the plan length plus `rise_mm` (the vertical run between levels) plus the allowance.
//!
//! Voltage drop uses the line-drop form with the catalogue's R and X per km at the cable's own operating figures (no
//! temperature correction): single phase `2*I*L*(R*cos(phi) + X*sin(phi))`, balanced three phase `sqrt(3)*I*L*(R*cos(phi) +
//! X*sin(phi))`, and the percentage is taken on the line-to-line voltage `sqrt(3)*V` (three phase) or `V` (single phase).
//! The cable is checked, or chosen as the smallest catalogue size, against `ampacity * derating >= current` and, when a
//! limit was given, against the drop limit. No size large enough is a failure, never a silent choice of the largest.
//!
//! This is a preliminary check. It does not cover short-circuit withstand, grouping or installation method (fold those into
//! the derating factor yourself), neutral or harmonic loading, earthing or motor starting, and it does not replace an
//! electrical design or the approving authority.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Point {
    pub x_mm: f64,
    pub y_mm: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Obstacle {
    pub key: String,
    pub x_mm: f64,
    pub y_mm: f64,
    pub w_mm: f64,
    pub h_mm: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CableType {
    pub key: String,
    #[serde(default)]
    pub area_mm2: Option<f64>,
    /// Conductor resistance, ohm per km.
    #[serde(default)]
    pub r_ohm_km: Option<f64>,
    /// Reactance, ohm per km.
    #[serde(default)]
    pub x_ohm_km: Option<f64>,
    /// Current-carrying capacity for the installation the caller has in mind, A.
    #[serde(default)]
    pub ampacity_a: Option<f64>,
    /// Where these figures came from (standard and table, or the manufacturer's data sheet).
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Circuit {
    pub id: String,
    pub from: Option<Point>,
    pub to: Option<Point>,
    /// Waypoints between the two ends. Absent: the module tries the orthogonal candidates.
    #[serde(default)]
    pub via: Option<Vec<Point>>,
    /// Vertical run between levels, mm. Absent: none (a same-level circuit).
    #[serde(default)]
    pub rise_mm: Option<f64>,
    /// Design power, W. Give this or `current_a`, not both.
    #[serde(default)]
    pub watts: Option<f64>,
    /// Design current per line, A.
    #[serde(default)]
    pub current_a: Option<f64>,
    /// 1 or 3.
    #[serde(default)]
    pub phases: Option<u8>,
    /// Key of the cable type fitted. Absent: the smallest catalogue size that passes is chosen.
    #[serde(default)]
    pub cable: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CableInput {
    /// Phase-to-neutral voltage, V.
    #[serde(default)]
    pub voltage_v: Option<f64>,
    #[serde(default)]
    pub power_factor: Option<f64>,
    /// Whether the supply is three-phase (a three-phase circuit needs it).
    #[serde(default)]
    pub three_phase: Option<bool>,
    /// Multiplier on the catalogue ampacity (grouping, ambient, installation), in (0, 1].
    #[serde(default)]
    pub derating_factor: Option<f64>,
    /// Length added to every run for terminations and slack, mm (zero is allowed, but it must be stated).
    #[serde(default)]
    pub allowance_mm: Option<f64>,
    #[serde(default)]
    pub catalogue: Vec<CableType>,
    #[serde(default)]
    pub obstacles: Vec<Obstacle>,
    /// Smallest distance a route keeps from an obstacle, mm. Required when there are obstacles.
    #[serde(default)]
    pub clearance_mm: Option<f64>,
    pub circuits: Vec<Circuit>,
    /// Largest voltage drop, percent. Absent: `unchecked`.
    #[serde(default)]
    pub max_drop_pct: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub ar: String,
    pub en: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CircuitRow {
    pub id: String,
    pub phases: u8,
    pub current_a: f64,
    /// `ok` (route found), `blocked` (crosses an obstacle) or `fails`.
    pub route_status: &'static str,
    pub blocked_by: Option<String>,
    /// Route points in mm, ends included. Empty when blocked.
    pub route: Vec<(f64, f64)>,
    pub plan_length_mm: Option<f64>,
    pub length_m: Option<f64>,
    pub cable: Option<String>,
    pub area_mm2: Option<f64>,
    pub ampacity_a: Option<f64>,
    pub derated_ampacity_a: Option<f64>,
    pub ampacity_status: &'static str,
    pub drop_v: Option<f64>,
    pub drop_pct: Option<f64>,
    pub drop_status: &'static str,
    pub status: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CableCounts {
    pub ok: u32,
    pub fails: u32,
    pub unchecked: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CableReport {
    pub circuits: Vec<CircuitRow>,
    pub total_length_m: f64,
    pub counts: CableCounts,
    pub findings: Vec<Finding>,
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn positive(v: f64, field: &str) -> Result<f64, String> {
    if v.is_finite() && v > 0.0 {
        Ok(v)
    } else {
        Err(format!("{field} must be a positive number, got {v}"))
    }
}

fn required(v: Option<f64>, field: &str) -> Result<f64, String> {
    positive(v.ok_or_else(|| format!("{field} is required and has no default"))?, field)
}

fn finite(v: f64, field: &str) -> Result<f64, String> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(format!("{field} must be a finite number, got {v}"))
    }
}

fn worst(statuses: &[&'static str]) -> &'static str {
    if statuses.contains(&"fails") {
        "fails"
    } else if statuses.contains(&"unchecked") {
        "unchecked"
    } else {
        "ok"
    }
}

/// A catalogue entry whose every figure is present and valid.
struct Cable {
    key: String,
    area: f64,
    r: f64,
    x: f64,
    amp: f64,
}

struct Supply {
    v: f64,
    pf: f64,
    three: bool,
    derate: f64,
    allowance: f64,
    clearance: f64,
    cables: Vec<Cable>,
}

fn validate(inp: &CableInput) -> Result<Supply, String> {
    if inp.circuits.is_empty() {
        return Err("nothing to check: give at least one circuit".into());
    }
    let v = required(inp.voltage_v, "voltage_v")?;
    let pf = required(inp.power_factor, "power_factor")?;
    if pf > 1.0 {
        return Err(format!("power_factor must be above 0 and at most 1, got {pf}"));
    }
    let three = inp.three_phase.ok_or_else(|| "three_phase is required and has no default".to_string())?;
    let derate = required(inp.derating_factor, "derating_factor")?;
    if derate > 1.0 {
        return Err(format!("derating_factor must be above 0 and at most 1, got {derate}"));
    }
    let allowance = inp.allowance_mm.ok_or_else(|| "allowance_mm is required and has no default (state 0 for none)".to_string())?;
    if !(allowance.is_finite() && allowance >= 0.0) {
        return Err(format!("allowance_mm must be zero or more, got {allowance}"));
    }
    if let Some(l) = inp.max_drop_pct {
        positive(l, "max_drop_pct")?;
    }
    if inp.catalogue.is_empty() {
        return Err("catalogue is required and has no default (list the cable sizes with their resistance, reactance and ampacity)".into());
    }
    let mut keys = BTreeSet::new();
    let mut cables = Vec::with_capacity(inp.catalogue.len());
    for c in &inp.catalogue {
        if c.key.trim().is_empty() {
            return Err("a catalogue entry has an empty key".into());
        }
        if !keys.insert(c.key.as_str()) {
            return Err(format!("catalogue key '{}' is used twice", c.key));
        }
        let name = |f: &str| format!("catalogue '{}': {f}", c.key);
        let source = c.source.as_deref().map(str::trim).unwrap_or("");
        if source.is_empty() {
            return Err(format!("catalogue '{}': source is required (where the resistance, reactance and ampacity came from)", c.key));
        }
        let r = c.r_ohm_km.ok_or_else(|| format!("{} is required and has no default", name("r_ohm_km")))?;
        let x = c.x_ohm_km.ok_or_else(|| format!("{} is required and has no default", name("x_ohm_km")))?;
        if !(r.is_finite() && r >= 0.0) {
            return Err(format!("{} must be zero or more, got {r}", name("r_ohm_km")));
        }
        if !(x.is_finite() && x >= 0.0) {
            return Err(format!("{} must be zero or more, got {x}", name("x_ohm_km")));
        }
        cables.push(Cable {
            key: c.key.clone(),
            area: required(c.area_mm2, &name("area_mm2"))?,
            r,
            x,
            amp: required(c.ampacity_a, &name("ampacity_a"))?,
        });
    }
    cables.sort_by(|a, b| a.area.total_cmp(&b.area).then(a.key.cmp(&b.key)));
    let mut okeys = BTreeSet::new();
    for o in &inp.obstacles {
        if o.key.trim().is_empty() {
            return Err("an obstacle has an empty key".into());
        }
        if !okeys.insert(o.key.as_str()) {
            return Err(format!("obstacle key '{}' is used twice", o.key));
        }
        for (f, val) in [("x_mm", o.x_mm), ("y_mm", o.y_mm)] {
            finite(val, &format!("obstacle '{}': {f}", o.key))?;
        }
        positive(o.w_mm, &format!("obstacle '{}': w_mm", o.key))?;
        positive(o.h_mm, &format!("obstacle '{}': h_mm", o.key))?;
    }
    let clearance = match inp.clearance_mm {
        Some(c) if c.is_finite() && c >= 0.0 => c,
        Some(c) => return Err(format!("clearance_mm must be zero or more, got {c}")),
        None if inp.obstacles.is_empty() => 0.0,
        None => return Err("clearance_mm is required when obstacles are given and has no default".into()),
    };
    let mut ids = BTreeSet::new();
    for c in &inp.circuits {
        if c.id.trim().is_empty() {
            return Err("a circuit has an empty id".into());
        }
        if !ids.insert(c.id.as_str()) {
            return Err(format!("circuit id '{}' is used twice", c.id));
        }
    }
    Ok(Supply { v, pf, three, derate, allowance, clearance, cables })
}

/// True when the segment `a-b` touches the rectangle grown by `m` on every side (Liang-Barsky clipping).
fn segment_hits(a: (f64, f64), b: (f64, f64), o: &Obstacle, m: f64) -> bool {
    let (x0, x1, y0, y1) = (o.x_mm - m, o.x_mm + o.w_mm + m, o.y_mm - m, o.y_mm + o.h_mm + m);
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [(-dx, a.0 - x0), (dx, x1 - a.0), (-dy, a.1 - y0), (dy, y1 - a.1)] {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}

fn first_block<'a>(path: &[(f64, f64)], obstacles: &'a [Obstacle], m: f64) -> Option<&'a Obstacle> {
    path.windows(2).find_map(|w| obstacles.iter().find(|o| segment_hits(w[0], w[1], o, m)))
}

fn plan_length(path: &[(f64, f64)]) -> f64 {
    path.windows(2).map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1)).sum()
}

/// Candidate routes between two points, in the fixed order of the module comment. Duplicate points are dropped.
fn candidates(a: (f64, f64), b: (f64, f64)) -> Vec<Vec<(f64, f64)>> {
    let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    let raw = [
        vec![a, (b.0, a.1), b],
        vec![a, (a.0, b.1), b],
        vec![a, (mid.0, a.1), (mid.0, b.1), b],
        vec![a, (a.0, mid.1), (b.0, mid.1), b],
    ];
    raw.into_iter()
        .map(|mut p| {
            p.dedup();
            p
        })
        .collect()
}

fn point(p: &Option<Point>, id: &str, which: &str) -> Result<(f64, f64), String> {
    let p = p.as_ref().ok_or_else(|| format!("circuit '{id}': {which} is required and has no default"))?;
    Ok((finite(p.x_mm, &format!("circuit '{id}': {which}.x_mm"))?, finite(p.y_mm, &format!("circuit '{id}': {which}.y_mm"))?))
}

/// The route of one circuit: the path, or the key of the obstacle that blocks every try.
fn route(c: &Circuit, inp: &CableInput, s: &Supply) -> Result<Result<Vec<(f64, f64)>, String>, String> {
    let a = point(&c.from, &c.id, "from")?;
    let b = point(&c.to, &c.id, "to")?;
    if let Some(via) = &c.via {
        let mut path = vec![a];
        for (i, p) in via.iter().enumerate() {
            path.push((finite(p.x_mm, &format!("circuit '{}': via[{i}].x_mm", c.id))?, finite(p.y_mm, &format!("circuit '{}': via[{i}].y_mm", c.id))?));
        }
        path.push(b);
        path.dedup();
        return Ok(match first_block(&path, &inp.obstacles, s.clearance) {
            Some(o) => Err(o.key.clone()),
            None => Ok(path),
        });
    }
    let mut first_blocker = None;
    for path in candidates(a, b) {
        match first_block(&path, &inp.obstacles, s.clearance) {
            None => return Ok(Ok(path)),
            Some(o) => {
                first_blocker.get_or_insert_with(|| o.key.clone());
            }
        }
    }
    Ok(Err(first_blocker.unwrap_or_default()))
}

/// Line drop in volts for current `i` (A) over `len_m` metres of `cable`.
fn drop_v(cable: &Cable, s: &Supply, phases: u8, i: f64, len_m: f64) -> f64 {
    let sin = (1.0 - s.pf * s.pf).max(0.0).sqrt();
    let per_km = cable.r * s.pf + cable.x * sin;
    let k = if phases == 3 { 3.0_f64.sqrt() } else { 2.0 };
    k * i * (len_m / 1000.0) * per_km
}

fn drop_pct(dv: f64, s: &Supply, phases: u8) -> f64 {
    let base = if phases == 3 { 3.0_f64.sqrt() * s.v } else { s.v };
    dv / base * 100.0
}

fn circuit_row(c: &Circuit, inp: &CableInput, s: &Supply) -> Result<CircuitRow, String> {
    let phases = c.phases.ok_or_else(|| format!("circuit '{}': phases is required (1 or 3) and has no default", c.id))?;
    if phases != 1 && phases != 3 {
        return Err(format!("circuit '{}': phases must be 1 or 3, got {phases}", c.id));
    }
    if phases == 3 && !s.three {
        return Err(format!("circuit '{}': a three-phase circuit needs three_phase = true", c.id));
    }
    let current = match (c.watts, c.current_a) {
        (Some(_), Some(_)) => return Err(format!("circuit '{}': give watts or current_a, not both", c.id)),
        (None, None) => return Err(format!("circuit '{}': watts or current_a is required and has no default", c.id)),
        (Some(w), None) => {
            let w = positive(w, &format!("circuit '{}': watts", c.id))?;
            if phases == 3 { w / (3.0 * s.v * s.pf) } else { w / (s.v * s.pf) }
        }
        (None, Some(i)) => positive(i, &format!("circuit '{}': current_a", c.id))?,
    };
    let rise = match c.rise_mm {
        None => 0.0,
        Some(r) if r.is_finite() && r >= 0.0 => r,
        Some(r) => return Err(format!("circuit '{}': rise_mm must be zero or more, got {r}", c.id)),
    };
    let fitted = match &c.cable {
        None => None,
        Some(k) => Some(
            s.cables
                .iter()
                .find(|x| &x.key == k)
                .ok_or_else(|| format!("circuit '{}': cable '{k}' is not in the catalogue", c.id))?,
        ),
    };
    let mut row = CircuitRow {
        id: c.id.clone(),
        phases,
        current_a: round2(current),
        route_status: "ok",
        blocked_by: None,
        route: Vec::new(),
        plan_length_mm: None,
        length_m: None,
        cable: None,
        area_mm2: None,
        ampacity_a: None,
        derated_ampacity_a: None,
        ampacity_status: "unchecked",
        drop_v: None,
        drop_pct: None,
        drop_status: "unchecked",
        status: "unchecked",
    };
    let path = match route(c, inp, s)? {
        Ok(p) => p,
        Err(blocker) => {
            row.route_status = "blocked";
            row.blocked_by = Some(blocker);
            row.status = "fails";
            return Ok(row);
        }
    };
    let plan = plan_length(&path);
    let len_m = (plan + rise + s.allowance) / 1000.0;
    row.plan_length_mm = Some(round2(plan));
    row.length_m = Some(round2(len_m));
    row.route = path.iter().map(|p| (round2(p.0), round2(p.1))).collect();

    let amp_ok = |k: &Cable| k.amp * s.derate >= current - 1e-9;
    let drop_ok = |k: &Cable| inp.max_drop_pct.is_none_or(|l| drop_pct(drop_v(k, s, phases, current, len_m), s, phases) <= l + 1e-9);
    let chosen = match fitted {
        Some(k) => Some(k),
        None => s.cables.iter().find(|k| amp_ok(k) && drop_ok(k)),
    };
    let Some(k) = chosen else {
        // Nothing in the catalogue passes: report against the largest size so the shortfall is visible, and fail.
        let k = s.cables.last().ok_or_else(|| "catalogue is empty".to_string())?;
        let dv = drop_v(k, s, phases, current, len_m);
        row.cable = None;
        row.area_mm2 = Some(k.area);
        row.derated_ampacity_a = Some(round2(k.amp * s.derate));
        row.ampacity_status = if amp_ok(k) { "ok" } else { "fails" };
        row.drop_v = Some(round2(dv));
        row.drop_pct = Some(round2(drop_pct(dv, s, phases)));
        row.drop_status = if drop_ok(k) { "ok" } else { "fails" };
        row.status = "fails";
        return Ok(row);
    };
    let dv = drop_v(k, s, phases, current, len_m);
    row.cable = Some(k.key.clone());
    row.area_mm2 = Some(k.area);
    row.ampacity_a = Some(k.amp);
    row.derated_ampacity_a = Some(round2(k.amp * s.derate));
    row.ampacity_status = if amp_ok(k) { "ok" } else { "fails" };
    row.drop_v = Some(round2(dv));
    row.drop_pct = Some(round2(drop_pct(dv, s, phases)));
    row.drop_status = match inp.max_drop_pct {
        None => "unchecked",
        Some(_) if drop_ok(k) => "ok",
        Some(_) => "fails",
    };
    row.status = worst(&[row.route_status, row.ampacity_status, row.drop_status]);
    Ok(row)
}

pub fn check(inp: &CableInput) -> Result<CableReport, String> {
    let s = validate(inp)?;
    let circuits = inp.circuits.iter().map(|c| circuit_row(c, inp, &s)).collect::<Result<Vec<_>, _>>()?;
    let mut counts = CableCounts::default();
    for c in &circuits {
        match c.status {
            "ok" => counts.ok += 1,
            "fails" => counts.fails += 1,
            _ => counts.unchecked += 1,
        }
    }
    let total: f64 = circuits.iter().filter_map(|c| c.length_m).sum();
    let mut findings = Vec::new();
    if inp.max_drop_pct.is_none() {
        findings.push(Finding {
            ar: "لم يُدخل حد هبوط الجهد (max_drop_pct) فلم يُحكم عليه؛ اختير المقاس بالحمل المسموح وحده.".into(),
            en: "No max_drop_pct given, so no pass or fail is declared for voltage drop; sizes were chosen on ampacity alone.".into(),
        });
    }
    for c in &circuits {
        if let Some(b) = &c.blocked_by {
            findings.push(Finding {
                ar: format!("الدائرة «{}»: كل مسار جُرِّب يقطع العائق «{}» فلم يُحسب طولها ولا هبوطها.", c.id, b),
                en: format!("Circuit '{}': every route tried crosses obstacle '{}', so its length and drop were not computed.", c.id, b),
            });
        } else if c.status == "fails" && c.cable.is_none() {
            findings.push(Finding {
                ar: format!("الدائرة «{}»: لا مقاس في الجدول يحقق الحمل المسموح وحد الهبوط معاً.", c.id),
                en: format!("Circuit '{}': no catalogue size meets both the ampacity and the drop limit.", c.id),
            });
        } else if c.ampacity_status == "fails" || c.drop_status == "fails" {
            findings.push(Finding {
                ar: format!("الدائرة «{}»: الكابل المحدد لا يحقق الحمل المسموح أو حد الهبوط.", c.id),
                en: format!("Circuit '{}': the cable named does not meet the ampacity or the drop limit.", c.id),
            });
        }
    }
    findings.push(Finding {
        ar: "فحص أولي بمدخلاتك: لا تيار قصر ولا تجميع كابلات ولا نيوترال ولا توافقيات ولا تأريض ولا بدء محركات، ولا يغني عن تصميم كهربائي أو الجهة المعتمدة.".into(),
        en: "Preliminary check from your inputs: no short-circuit withstand, cable grouping, neutral or harmonic loading, earthing or motor starting, and it does not replace an electrical design or the approving authority.".into(),
    });
    Ok(CableReport { circuits, total_length_m: round2(total), counts, findings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Option<Point> {
        Some(Point { x_mm: x, y_mm: y })
    }
    fn cable(key: &str, area: f64, r: f64, amp: f64) -> CableType {
        CableType { key: key.into(), area_mm2: Some(area), r_ohm_km: Some(r), x_ohm_km: Some(0.0), ampacity_a: Some(amp), source: Some("test data".into()) }
    }
    fn circuit(id: &str, to: (f64, f64), amps: f64) -> Circuit {
        Circuit { id: id.into(), from: p(0.0, 0.0), to: p(to.0, to.1), via: None, rise_mm: None, watts: None, current_a: Some(amps), phases: Some(1), cable: None }
    }
    fn input(circuits: Vec<Circuit>) -> CableInput {
        CableInput {
            voltage_v: Some(230.0),
            power_factor: Some(1.0),
            three_phase: Some(true),
            derating_factor: Some(1.0),
            allowance_mm: Some(0.0),
            catalogue: vec![cable("c10", 10.0, 2.0, 60.0), cable("c4", 4.0, 5.0, 30.0), cable("c25", 25.0, 1.0, 100.0)],
            obstacles: vec![],
            clearance_mm: None,
            circuits,
            max_drop_pct: Some(3.0),
        }
    }

    #[test]
    fn single_phase_drop_matches_hand_calculation() {
        // 20 A over 10 m of 5 ohm/km: 2 * 20 * 0.010 * 5 = 2 V = 0.8696 % of 230 V.
        let mut inp = input(vec![circuit("a", (10_000.0, 0.0), 20.0)]);
        inp.circuits[0].cable = Some("c4".into());
        let r = check(&inp).unwrap();
        let c = &r.circuits[0];
        assert_eq!(c.length_m, Some(10.0));
        assert_eq!(c.drop_v, Some(2.0));
        assert_eq!(c.drop_pct, Some(0.87));
        assert_eq!(c.status, "ok");
    }

    #[test]
    fn three_phase_drop_uses_root_three_and_line_voltage() {
        // 30 A, 100 m, 1 ohm/km: sqrt(3) * 30 * 0.1 * 1 = 5.196 V; base sqrt(3) * 230.
        let mut c = circuit("t", (100_000.0, 0.0), 30.0);
        c.phases = Some(3);
        c.cable = Some("c25".into());
        let r = check(&input(vec![c])).unwrap();
        assert_eq!(r.circuits[0].drop_v, Some(5.2));
        assert_eq!(r.circuits[0].drop_pct, Some(1.3));
    }

    #[test]
    fn smallest_passing_size_is_chosen_and_drop_limit_pushes_it_up() {
        // 25 A: c4 (30 A) passes ampacity but drops 2*25*0.05*5 = 12.5 V on 50 m; c10 drops 5 V (2.17 %), passes 3 %.
        let r = check(&input(vec![circuit("a", (50_000.0, 0.0), 25.0)])).unwrap();
        assert_eq!(r.circuits[0].cable.as_deref(), Some("c10"));
        assert_eq!(r.circuits[0].status, "ok");
    }

    #[test]
    fn no_limit_leaves_the_drop_unchecked_and_chooses_on_ampacity() {
        let mut inp = input(vec![circuit("a", (50_000.0, 0.0), 25.0)]);
        inp.max_drop_pct = None;
        let r = check(&inp).unwrap();
        assert_eq!(r.circuits[0].cable.as_deref(), Some("c4"));
        assert_eq!(r.circuits[0].drop_status, "unchecked");
        assert_eq!(r.circuits[0].status, "unchecked");
        assert!(r.findings.iter().any(|f| f.en.contains("max_drop_pct")));
    }

    #[test]
    fn nothing_big_enough_is_a_failure_not_the_largest_size() {
        let r = check(&input(vec![circuit("a", (10_000.0, 0.0), 500.0)])).unwrap();
        assert_eq!(r.circuits[0].cable, None);
        assert_eq!(r.circuits[0].status, "fails");
        assert_eq!(r.counts.fails, 1);
    }

    #[test]
    fn named_cable_that_is_too_small_fails() {
        let mut c = circuit("a", (10_000.0, 0.0), 40.0);
        c.cable = Some("c4".into());
        let r = check(&input(vec![c])).unwrap();
        assert_eq!(r.circuits[0].ampacity_status, "fails");
        assert_eq!(r.circuits[0].status, "fails");
    }

    #[test]
    fn derating_rise_and_allowance_add_up() {
        let mut inp = input(vec![circuit("a", (3_000.0, 4_000.0), 20.0)]);
        inp.circuits[0].rise_mm = Some(3_000.0);
        inp.allowance_mm = Some(2_000.0);
        inp.derating_factor = Some(0.5);
        let r = check(&inp).unwrap();
        // Plan 3000 + 4000 (an L, not the diagonal) + rise 3000 + allowance 2000 = 12 m.
        assert_eq!(r.circuits[0].plan_length_mm, Some(7000.0));
        assert_eq!(r.circuits[0].length_m, Some(12.0));
        // 20 A needs 40 A before derating: c4 (30) is out, c10 (60) is the smallest.
        assert_eq!(r.circuits[0].cable.as_deref(), Some("c10"));
        assert_eq!(r.circuits[0].derated_ampacity_a, Some(30.0));
    }

    #[test]
    fn watts_become_current_by_phase_count() {
        let mut a = circuit("a", (1_000.0, 0.0), 0.0);
        a.current_a = None;
        a.watts = Some(2300.0);
        let mut b = a.clone();
        b.id = "b".into();
        b.phases = Some(3);
        b.watts = Some(6900.0);
        let r = check(&input(vec![a, b])).unwrap();
        assert_eq!(r.circuits[0].current_a, 10.0);
        assert_eq!(r.circuits[1].current_a, 10.0);
    }

    #[test]
    fn route_goes_around_a_column_or_reports_it() {
        let col = Obstacle { key: "C1".into(), x_mm: 4_000.0, y_mm: -500.0, w_mm: 400.0, h_mm: 1_000.0 };
        let mut inp = input(vec![circuit("a", (8_000.0, 3_000.0), 10.0)]);
        inp.obstacles = vec![col];
        inp.clearance_mm = Some(100.0);
        // Horizontal-first runs along y = 0 through the column; vertical-first runs along x = 0 then y = 3000: clear.
        let r = check(&inp).unwrap();
        assert_eq!(r.circuits[0].route, vec![(0.0, 0.0), (0.0, 3000.0), (8000.0, 3000.0)]);
        // A wall of obstacles that every candidate crosses.
        inp.obstacles = vec![Obstacle { key: "W".into(), x_mm: -100_000.0, y_mm: -100_000.0, w_mm: 200_000.0, h_mm: 200_000.0 }];
        let r = check(&inp).unwrap();
        assert_eq!(r.circuits[0].route_status, "blocked");
        assert_eq!(r.circuits[0].blocked_by.as_deref(), Some("W"));
        assert_eq!(r.circuits[0].length_m, None);
        assert_eq!(r.circuits[0].status, "fails");
    }

    #[test]
    fn clearance_counts_and_waypoints_are_checked() {
        let col = Obstacle { key: "C1".into(), x_mm: 4_000.0, y_mm: 200.0, w_mm: 400.0, h_mm: 400.0 };
        let mut inp = input(vec![circuit("a", (8_000.0, 0.0), 10.0)]);
        inp.circuits[0].via = Some(vec![]);
        inp.obstacles = vec![col];
        // 200 mm off the line: clear with 100 mm clearance, blocked with 300 mm.
        inp.clearance_mm = Some(100.0);
        assert_eq!(check(&inp).unwrap().circuits[0].route_status, "ok");
        inp.clearance_mm = Some(300.0);
        assert_eq!(check(&inp).unwrap().circuits[0].route_status, "blocked");
    }

    #[test]
    fn missing_inputs_are_refused_by_name() {
        let base = input(vec![circuit("a", (1_000.0, 0.0), 10.0)]);
        type Edit = Box<dyn Fn(&mut CableInput)>;
        let cases: Vec<(&str, Edit)> = vec![
            ("voltage_v", Box::new(|i| i.voltage_v = None)),
            ("power_factor", Box::new(|i| i.power_factor = None)),
            ("three_phase", Box::new(|i| i.three_phase = None)),
            ("derating_factor", Box::new(|i| i.derating_factor = None)),
            ("allowance_mm", Box::new(|i| i.allowance_mm = None)),
            ("catalogue", Box::new(|i| i.catalogue.clear())),
            ("r_ohm_km", Box::new(|i| i.catalogue[0].r_ohm_km = None)),
            ("source is required", Box::new(|i| i.catalogue[0].source = None)),
            ("phases", Box::new(|i| i.circuits[0].phases = None)),
            ("watts or current_a", Box::new(|i| i.circuits[0].current_a = None)),
            ("from", Box::new(|i| i.circuits[0].from = None)),
            ("clearance_mm", Box::new(|i| i.obstacles = vec![Obstacle { key: "C".into(), x_mm: 0.0, y_mm: 0.0, w_mm: 1.0, h_mm: 1.0 }])),
        ];
        for (needle, edit) in cases {
            let mut i = base.clone();
            edit(&mut i);
            let e = check(&i).unwrap_err();
            assert!(e.contains(needle), "{needle}: {e}");
        }
        let mut both = base.clone();
        both.circuits[0].watts = Some(100.0);
        assert!(check(&both).unwrap_err().contains("not both"));
        let mut unknown = base.clone();
        unknown.circuits[0].cable = Some("zz".into());
        assert!(check(&unknown).unwrap_err().contains("not in the catalogue"));
        let mut single = base;
        single.three_phase = Some(false);
        single.circuits[0].phases = Some(3);
        assert!(check(&single).unwrap_err().contains("three_phase = true"));
    }
}
