//! Construction schedule built from the quantities.
//!
//! Each activity template in [`TEMPLATES`] (the build sequence: site, foundations, frame,
//! masonry, finishes, handover) takes its quantity from the take-off, its duration from a
//! production rate YOU supply, and its order from the template's predecessors. Zoned
//! activities (the concrete frame) are split into pour zones that follow each other so a
//! crew keeps moving. A forward and backward pass (critical path method) gives dates,
//! float and the critical path on a working-day calendar.
//!
//! Nothing here is invented data:
//! - Production rates and fixed durations (including concrete curing time) are inputs from
//!   the contractor or company records. They default to unset; any unset rate is an
//!   error that lists exactly which are missing, and no schedule is produced.
//! - The working week and holidays are project settings, not assumptions baked in.
//!
//! Every task keeps the ids of the model elements its quantity came from.

use crate::model::{Id, Model};
use crate::qto::{Qto, Unit};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

// ------------------------------------------------------------------ dates and calendar

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub y: i32,
    pub m: u32,
    pub d: u32,
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

impl Date {
    pub fn new(y: i32, m: u32, d: u32) -> Option<Date> {
        ((1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m)).then_some(Date { y, m, d })
    }

    /// Parses `YYYY-MM-DD`.
    pub fn parse(s: &str) -> Option<Date> {
        let mut it = s.split('-');
        let (y, m, d) = (it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?);
        if it.next().is_some() { None } else { Date::new(y, m, d) }
    }

    pub fn iso(self) -> String {
        format!("{:04}-{:02}-{:02}", self.y, self.m, self.d)
    }

    /// Days since 1970-01-01 (proleptic Gregorian).
    pub fn to_days(self) -> i64 {
        let y = self.y as i64 - (self.m <= 2) as i64;
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let m = self.m as i64;
        let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + self.d as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    pub fn from_days(z: i64) -> Date {
        let z = z + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let y = (yoe + era * 400) as i32 + (m <= 2) as i32;
        Date { y, m, d }
    }

    /// Monday = 0 ... Sunday = 6.
    pub fn weekday(self) -> usize {
        (self.to_days() + 3).rem_euclid(7) as usize
    }
}

/// Working calendar. The weekend and holidays are project settings.
#[derive(Debug, Clone)]
pub struct Calendar {
    start: Date,
    /// Monday = index 0 ... Sunday = index 6.
    work_days: [bool; 7],
    pub holidays: BTreeSet<Date>,
}

impl Calendar {
    /// `None` if no weekday is a working day.
    pub fn new(start: Date, work_days: [bool; 7]) -> Option<Calendar> {
        work_days.iter().any(|&w| w).then(|| Calendar { start, work_days, holidays: BTreeSet::new() })
    }

    pub fn is_work(&self, day: i64) -> bool {
        let date = Date::from_days(day);
        self.work_days[date.weekday()] && !self.holidays.contains(&date)
    }

    /// The date of working day `n` (0 is the first working day on or after the start).
    pub fn nth_workday(&self, n: i64) -> Date {
        let mut day = self.start.to_days();
        let mut left = n.max(0);
        loop {
            if self.is_work(day) {
                if left == 0 {
                    return Date::from_days(day);
                }
                left -= 1;
            }
            day += 1;
        }
    }
}

// ------------------------------------------------------------------ critical path method

#[derive(Debug, Clone, PartialEq)]
pub struct Cpm {
    pub es: Vec<i64>,
    pub ef: Vec<i64>,
    pub ls: Vec<i64>,
    pub lf: Vec<i64>,
    pub float: Vec<i64>,
    pub finish: i64,
}

/// Critical path method on durations in working days. `preds[i]` lists `(task, lag)` finish-to-start
/// relations; every predecessor index must be smaller than `i` (tasks are in dependency order).
pub fn cpm(durations: &[i64], preds: &[Vec<(usize, i64)>]) -> Cpm {
    let n = durations.len();
    let mut es = vec![0i64; n];
    let mut ef = vec![0i64; n];
    for i in 0..n {
        es[i] = preds[i]
            .iter()
            .map(|&(p, lag)| {
                assert!(p < i, "predecessor {p} of task {i} is not earlier");
                ef[p] + lag
            })
            .max()
            .unwrap_or(0)
            .max(0);
        ef[i] = es[i] + durations[i];
    }
    let finish = ef.iter().copied().max().unwrap_or(0);
    let mut lf = vec![finish; n];
    let mut ls = vec![0i64; n];
    for i in (0..n).rev() {
        // lf[i] was lowered by its successors, which are all later and already processed.
        ls[i] = lf[i] - durations[i];
        for &(p, lag) in &preds[i] {
            lf[p] = lf[p].min(ls[i] - lag);
        }
    }
    let float = (0..n).map(|i| ls[i] - es[i]).collect();
    Cpm { es, ef, ls, lf, float, finish }
}

// ------------------------------------------------------------------ rates and templates

fn one() -> u32 {
    1
}

/// Production rate or fixed duration for one activity. All fields are your project's data.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Rate {
    /// Quantity one crew completes per working day (for quantity-driven activities).
    #[serde(default)]
    pub per_crew_day: Option<f64>,
    /// Duration in working days (for fixed activities such as curing, mobilisation, testing).
    #[serde(default)]
    pub fixed_days: Option<f64>,
    #[serde(default = "one")]
    pub crews: u32,
    /// Where the rate comes from (company records, subcontractor quotation...).
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RateLib {
    #[serde(default)]
    pub rates: BTreeMap<String, Rate>,
}

impl RateLib {
    pub fn from_json(json: &str) -> Result<RateLib, serde_json::Error> {
        serde_json::from_str(json)
    }
}

enum Kind {
    /// Sum of the take-off items whose code equals an entry, or starts with it if the entry ends in `*`.
    Qty(&'static [&'static str]),
    Fixed,
}

/// The work breakdown: the six phases, in the order the work follows.
pub const PHASES: [&str; 6] = ["Substructure", "Superstructure", "Insulation", "MEP rough-in", "Finishing", "Commissioning"];

struct Tpl {
    key: &'static str,
    name: &'static str,
    kind: Kind,
    preds: &'static [&'static str],
    /// Split into pour zones that follow one another.
    zoned: bool,
    /// Index into `PHASES`.
    phase: usize,
    /// Present only when the take-off has a quantity for it (no insulation, no insulation task). An absent task passes its
    /// successors on to its own predecessor, and its rate is not required.
    optional: bool,
    /// Further quantities measured for the same task, as `(label, item codes)`: the concrete and the steel of the same work.
    also: &'static [(&'static str, &'static [&'static str])],
}

use Kind::{Fixed, Qty};

const fn tpl(key: &'static str, name: &'static str, kind: Kind, preds: &'static [&'static str], phase: usize) -> Tpl {
    Tpl { key, name, kind, preds, zoned: false, phase, optional: false, also: &[] }
}

const POINTS: &[&str] = &["MEP.light", "MEP.socket", "MEP.panel", "MEP.fixture.*", "MEP.ac_indoor", "MEP.diffuser", "MEP.grille"];

/// The build sequence. Order matters: predecessors come first. Critical path of the whole job:
/// substructure, superstructure, insulation, MEP rough-in, finishing, commissioning.
static TEMPLATES: &[Tpl] = &[
    tpl("mobilise", "Site mobilisation", Fixed, &[], 0),
    tpl("set_out", "Setting out", Fixed, &["mobilise"], 0),
    tpl("excavate", "Excavation for footings", Qty(&["FOOTING.AREA"]), &["set_out"], 0),
    tpl("footing_form", "Footing formwork", Qty(&["FOOTING.AREA"]), &["excavate"], 0),
    Tpl { also: &[("steel", &["FND.STEEL"])], ..tpl("footing_steel", "Footing reinforcement", Qty(&["FOOTING.COUNT"]), &["footing_form"], 0) },
    Tpl { also: &[("concrete", &["FND.CONC"])], ..tpl("footing_pour", "Footing concrete pour", Qty(&["FOOTING.AREA"]), &["footing_steel"], 0) },
    tpl("footing_cure", "Footing curing", Fixed, &["footing_pour"], 0),
    Tpl { optional: true, also: &[("steel", &["TB.STEEL"])], ..tpl("tie_beam", "Tie beams", Qty(&["TIEBEAM.VOL"]), &["footing_cure"], 0) },
    tpl("backfill", "Backfill around footings", Qty(&["FOOTING.AREA"]), &["footing_cure", "tie_beam"], 0),
    Tpl { optional: true, ..tpl("drain_runs", "Below-slab drainage", Qty(&["MEP.run.drain.*"]), &["backfill"], 0) },
    Tpl { zoned: true, ..tpl("column_form", "Column formwork", Qty(&["COLUMN.FORM"]), &["backfill", "drain_runs"], 1) },
    Tpl { zoned: true, also: &[("steel", &["COL.STEEL"])], ..tpl("column_steel", "Column reinforcement", Qty(&["COLUMN.COUNT"]), &["column_form"], 1) },
    Tpl { zoned: true, also: &[("concrete", &["COL.CONC"])], ..tpl("column_pour", "Column concrete pour", Qty(&["COLUMN.VOL"]), &["column_steel"], 1) },
    Tpl { zoned: true, ..tpl("slab_form", "Slab and beam formwork", Qty(&["SLAB.FORM", "BEAM.FORM"]), &["column_pour"], 1) },
    Tpl { zoned: true, ..tpl("slab_steel", "Slab and beam reinforcement", Qty(&["SLAB.FORM"]), &["slab_form"], 1) },
    tpl("mep_embed", "Embedded electrical and plumbing", Fixed, &["slab_steel"], 1),
    Tpl { zoned: true, ..tpl("slab_pour", "Slab and beam concrete pour", Qty(&["SLAB.VOL", "BEAM.VOL"]), &["mep_embed"], 1) },
    tpl("slab_cure", "Slab curing", Fixed, &["slab_pour"], 1),
    tpl("strip_form", "Formwork stripping", Qty(&["SLAB.FORM"]), &["slab_cure"], 1),
    tpl("blockwork", "Blockwork", Qty(&["WALL.EXT.AREA", "WALL.INT.AREA"]), &["strip_form"], 1),
    Tpl { optional: true, ..tpl("insulation", "Thermal insulation and waterproofing", Qty(&["INS.*"]), &["blockwork"], 2) },
    tpl("mep_first_fix", "First-fix electrical and plumbing", Fixed, &["insulation"], 3),
    Tpl { optional: true, ..tpl("duct_runs", "HVAC ductwork", Qty(&["MEP.run.duct.*"]), &["mep_first_fix"], 3) },
    Tpl { optional: true, ..tpl("supply_runs", "Water supply pipework", Qty(&["MEP.run.pipe.*"]), &["mep_first_fix"], 3) },
    Tpl { optional: true, ..tpl("cable_runs", "Cable trays", Qty(&["MEP.run.cable.*"]), &["mep_first_fix"], 3) },
    tpl("plaster", "Plastering", Qty(&["WALL.FACES.AREA"]), &["mep_first_fix", "duct_runs", "supply_runs", "cable_runs"], 4),
    tpl("doors_windows", "Doors and windows", Qty(&["DOOR.*", "WINDOW.*"]), &["plaster"], 4),
    tpl("ceiling", "Ceilings", Qty(&["CEILING.AREA.*"]), &["plaster"], 4),
    tpl("paint", "Painting", Qty(&["WALL.FACES.AREA"]), &["ceiling", "doors_windows"], 4),
    tpl("floor_finish", "Floor finishes", Qty(&["FLOOR.AREA.*"]), &["paint"], 4),
    tpl("mep_second_fix", "Second-fix and fixtures", Fixed, &["paint"], 4),
    Tpl { optional: true, ..tpl("mep_points", "Devices, diffusers and fixtures installed", Qty(POINTS), &["paint"], 4) },
    tpl("testing", "Testing and commissioning", Fixed, &["mep_second_fix", "floor_finish", "mep_points"], 5),
    tpl("handover", "Snagging and handover", Fixed, &["testing"], 5),
];

fn templates() -> impl Iterator<Item = &'static Tpl> {
    TEMPLATES.iter()
}

/// Every rate key the schedule can need, and whether it is a fixed duration (`true`) or a production rate. Optional tasks
/// are included: their rate is required only when the take-off has something for them (see `optional_keys`).
pub fn template_keys() -> Vec<(&'static str, bool)> {
    templates().map(|t| (t.key, matches!(t.kind, Fixed))).collect()
}

/// `(key, name, phase)` of every activity of the build sequence, for the pages that link a record to one.
pub fn template_names() -> Vec<(&'static str, &'static str, &'static str)> {
    templates().map(|t| (t.key, t.name, PHASES[t.phase])).collect()
}

/// The keys of the tasks that exist only when the model has the work.
pub fn optional_keys() -> Vec<&'static str> {
    templates().filter(|t| t.optional).map(|t| t.key).collect()
}

// ------------------------------------------------------------------ schedule

/// An additional quantity measured for a task, for example the steel of a concrete pour.
#[derive(Debug, Clone)]
pub struct Measure {
    pub label: String,
    pub quantity: f64,
    pub unit: Unit,
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    /// The template key of the activity (`footing_pour`, `plaster` ...): stable, unlike `id`, so other records can link to it.
    pub key: &'static str,
    pub name: String,
    /// Phase of the work breakdown (`PHASES`) and the task's number in it, for example `2.3`.
    pub phase: &'static str,
    pub wbs: String,
    pub zone: Option<usize>,
    pub quantity: Option<f64>,
    pub unit: Option<Unit>,
    /// Further quantities of the same work (concrete m3, steel kg).
    pub also: Vec<Measure>,
    pub duration: i64,
    /// `(task index, lag in working days)`, finish to start.
    pub preds: Vec<(usize, i64)>,
    pub es: i64,
    pub ef: i64,
    pub ls: i64,
    pub lf: i64,
    pub float: i64,
    pub critical: bool,
    pub start: Date,
    pub finish: Date,
    pub rate_source: String,
    pub qto_codes: Vec<String>,
    /// Model elements the quantity was measured from.
    pub elements: Vec<Id>,
}

#[derive(Debug, Clone)]
pub struct Schedule {
    pub tasks: Vec<Task>,
    pub project_days: i64,
    pub finish: Date,
    pub critical: Vec<usize>,
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ScheduleError {
    #[error("{} production rates or durations are not set: {}", .0.len(), .0.join(", "))]
    MissingRates(Vec<String>),
    #[error("zones must be at least 1")]
    BadZones,
}

fn matches_code(entry: &str, code: &str) -> bool {
    match entry.strip_suffix('*') {
        Some(prefix) => code.starts_with(prefix),
        None => code == entry,
    }
}

/// Builds the schedule from the quantities alone. With more than one zone, each zoned quantity is
/// split equally, which is a convenience only: use [`build_with_model`] to tie zones to real elements.
pub fn build(qto: &Qto, lib: &RateLib, cal: &Calendar, zones: usize) -> Result<Schedule, ScheduleError> {
    let mut s = build_inner(qto, lib, cal, zones, None)?;
    if zones > 1 {
        s.warnings.push("pour zones are an equal split of each quantity, not tied to elements; use build_with_model".into());
    }
    Ok(s)
}

/// Builds the schedule with pour zones tied to the model: every frame element (column, beam, slab,
/// footing) belongs to the zone its centre falls in, west to east in equal-width bands, and each
/// zoned task takes the quantity and the element ids of its zone only.
pub fn build_with_model(model: &Model, qto: &Qto, lib: &RateLib, cal: &Calendar, zones: usize) -> Result<Schedule, ScheduleError> {
    build_inner(qto, lib, cal, zones, Some(&zone_map(model, zones.max(1))))
}

/// Zone (0-based) of every frame element, by the east-west position of its centre.
fn zone_map(model: &Model, zones: usize) -> BTreeMap<Id, usize> {
    use crate::typed::{Beam, Column, Footing, Slab, TieBeam, Typed};
    let mut xs: Vec<(Id, i64)> = Vec::new();
    for e in model.elements.values() {
        let x = match e.kind.as_str() {
            Column::KIND => e.typed::<Column>().ok().map(|c| c.x),
            Slab::KIND => e.typed::<Slab>().ok().map(|s| s.x + s.w / 2),
            Beam::KIND => e.typed::<Beam>().ok().map(|b| (b.x1 + b.x2) / 2),
            Footing::KIND => e.typed::<Footing>().ok().map(|f| f.x),
            TieBeam::KIND => e.typed::<TieBeam>().ok().map(|b| (b.x1 + b.x2) / 2),
            _ => None,
        };
        if let Some(x) = x {
            xs.push((e.id, x));
        }
    }
    let lo = xs.iter().map(|p| p.1).min().unwrap_or(0);
    let hi = xs.iter().map(|p| p.1).max().unwrap_or(0);
    let span = (hi - lo + 1) as i128;
    xs.into_iter().map(|(id, x)| (id, (((x - lo) as i128 * zones as i128) / span) as usize)).collect()
}

/// What the take-off holds for a list of item codes: the total, its unit, the codes, the elements and each element's share.
struct Gathered {
    total: f64,
    unit: Option<Unit>,
    codes: Vec<String>,
    ids: Vec<Id>,
    parts: Vec<(Id, f64)>,
}

fn gather(qto: &Qto, entries: &[&str]) -> Gathered {
    let items: Vec<_> = qto.items.iter().filter(|i| entries.iter().any(|e| matches_code(e, &i.code))).collect();
    // Counts (DOOR.*) and areas (WINDOW.AREA) can match one entry: keep items of the first unit only.
    let unit = items.first().map(|i| i.unit);
    let items: Vec<_> = items.into_iter().filter(|i| Some(i.unit) == unit).collect();
    let mut ids: Vec<Id> = items.iter().flat_map(|i| i.sources.iter().copied()).collect();
    ids.sort();
    ids.dedup();
    Gathered { total: items.iter().map(|i| i.quantity).sum(), unit, codes: items.iter().map(|i| i.code.clone()).collect(), ids, parts: items.iter().flat_map(|i| i.parts.iter().copied()).collect() }
}

/// The share of `g` that falls to zone `z` of `n`: the elements standing in the zone with their own quantities when the
/// model is known, otherwise an equal split.
fn zone_share(g: &Gathered, zoned: bool, z: usize, n: usize, zones: usize, zm: Option<&BTreeMap<Id, usize>>) -> (f64, Vec<Id>, bool) {
    match (zm, zoned && zones > 1 && !g.parts.is_empty()) {
        (Some(zm), true) => {
            let mine: Vec<&(Id, f64)> = g.parts.iter().filter(|(id, _)| zm.get(id).copied().unwrap_or(0) == z).collect();
            let mut ids: Vec<Id> = mine.iter().map(|p| p.0).collect();
            ids.sort();
            ids.dedup();
            let q: f64 = mine.iter().map(|p| p.1).sum();
            (q, ids, true)
        }
        _ => (g.total / n as f64, g.ids.clone(), false),
    }
}

fn build_inner(qto: &Qto, lib: &RateLib, cal: &Calendar, zones: usize, zone_map: Option<&BTreeMap<Id, usize>>) -> Result<Schedule, ScheduleError> {
    if zones == 0 {
        return Err(ScheduleError::BadZones);
    }
    // Which tasks exist: all of them, except optional ones the take-off has nothing for.
    let present = |t: &Tpl| -> bool {
        match (&t.kind, t.optional) {
            (Qty(entries), true) => gather(qto, entries).total > 0.0,
            _ => true,
        }
    };
    let missing: Vec<String> = templates()
        .filter(|t| present(t))
        .filter(|t| {
            let r = lib.rates.get(t.key);
            match t.kind {
                Fixed => !r.is_some_and(|r| r.fixed_days.is_some_and(|d| d >= 0.0)),
                Qty(_) => !r.is_some_and(|r| r.per_crew_day.is_some_and(|p| p > 0.0) && r.crews >= 1),
            }
        })
        .map(|t| t.key.to_string())
        .collect();
    if !missing.is_empty() {
        return Err(ScheduleError::MissingRates(missing));
    }

    let mut warnings = Vec::new();
    let mut tasks: Vec<Task> = Vec::new();
    // Task indices per template key, one per zone. An absent optional task stands for its predecessor.
    let mut by_key: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    let mut per_phase = [0usize; PHASES.len()];
    let placeholder = cal.nth_workday(0);

    for t in templates() {
        if !present(t) {
            if let Some(p) = t.preds.first().and_then(|p| by_key.get(p)).cloned() {
                by_key.insert(t.key, p);
            }
            continue;
        }
        let rate = &lib.rates[t.key];
        let n = if t.zoned { zones } else { 1 };
        let main = match &t.kind {
            Fixed => None,
            Qty(entries) => Some(gather(qto, entries)),
        };
        if let Some(g) = &main
            && g.total <= 0.0
        {
            warnings.push(format!("'{}' has no quantity in the take-off: scheduled with zero duration", t.name));
        }
        let extras: Vec<(&str, Gathered)> = t.also.iter().map(|(label, codes)| (*label, gather(qto, codes))).filter(|(_, g)| g.total > 0.0).collect();
        per_phase[t.phase] += 1;
        let wbs = format!("{}.{}", t.phase + 1, per_phase[t.phase]);
        for z in 0..n {
            // With a model, a zone takes the elements that stand in it; otherwise an equal share.
            let (qty_z, mut elems_z): (Option<f64>, Vec<Id>) = match &main {
                None => (None, Vec::new()),
                Some(g) => {
                    let (q, ids, by_zone) = zone_share(g, t.zoned, z, n, zones, zone_map);
                    if by_zone && q <= 0.0 && g.total > 0.0 {
                        warnings.push(format!("zone {} has no elements for '{}'", z + 1, t.name));
                    }
                    (Some(q), ids)
                }
            };
            let mut also: Vec<Measure> = Vec::new();
            for (label, g) in &extras {
                let (q, ids, _) = zone_share(g, t.zoned, z, n, zones, zone_map);
                if let Some(unit) = g.unit {
                    also.push(Measure { label: (*label).to_string(), quantity: (q * 1e4).round() / 1e4, unit });
                }
                elems_z.extend(ids);
            }
            elems_z.sort();
            elems_z.dedup();
            let duration = match (&t.kind, qty_z) {
                (Fixed, _) => rate.fixed_days.unwrap_or(0.0).ceil() as i64,
                (Qty(_), Some(q)) if q > 0.0 => (q / (rate.per_crew_day.unwrap_or(1.0) * rate.crews as f64)).ceil() as i64,
                _ => 0,
            };
            let mut preds: Vec<(usize, i64)> = Vec::new();
            for p in t.preds {
                let Some(ptasks) = by_key.get(p) else { continue };
                let idx = if t.zoned && ptasks.len() > 1 { ptasks[z] } else { *ptasks.last().expect("predecessor exists") };
                if !preds.iter().any(|(i, _)| *i == idx) {
                    preds.push((idx, 0)); // the same zone of the previous zoned activity
                }
            }
            if z > 0 {
                preds.push((tasks.len() - 1, 0)); // the crew moves on to the next zone
            }
            let idx = tasks.len();
            tasks.push(Task {
                id: format!("T{:02}", idx + 1),
                key: t.key,
                name: if t.zoned && zones > 1 { format!("{} (zone {})", t.name, z + 1) } else { t.name.to_string() },
                phase: PHASES[t.phase],
                wbs: wbs.clone(),
                zone: (t.zoned && zones > 1).then_some(z + 1),
                quantity: qty_z.map(|q| (q * 1e4).round() / 1e4),
                unit: main.as_ref().and_then(|g| g.unit),
                also,
                duration,
                preds,
                es: 0,
                ef: 0,
                ls: 0,
                lf: 0,
                float: 0,
                critical: false,
                start: placeholder,
                finish: placeholder,
                rate_source: rate.source.clone(),
                qto_codes: main.as_ref().map(|g| g.codes.clone()).unwrap_or_default().into_iter().chain(extras.iter().flat_map(|(_, g)| g.codes.clone())).collect(),
                elements: elems_z,
            });
            by_key.entry(t.key).or_default().push(idx);
        }
    }

    let durations: Vec<i64> = tasks.iter().map(|t| t.duration).collect();
    let preds: Vec<Vec<(usize, i64)>> = tasks.iter().map(|t| t.preds.clone()).collect();
    let c = cpm(&durations, &preds);
    let mut critical = Vec::new();
    for (i, t) in tasks.iter_mut().enumerate() {
        t.es = c.es[i];
        t.ef = c.ef[i];
        t.ls = c.ls[i];
        t.lf = c.lf[i];
        t.float = c.float[i];
        t.critical = c.float[i] == 0;
        t.start = cal.nth_workday(t.es);
        t.finish = if t.duration > 0 { cal.nth_workday(t.ef - 1) } else { t.start };
        if t.critical {
            critical.push(i);
        }
    }
    let finish = cal.nth_workday((c.finish - 1).max(0));
    Ok(Schedule { tasks, project_days: c.finish, finish, critical, warnings })
}
impl Schedule {
    pub fn to_csv(&self) -> String {
        fn cell(s: &str) -> String {
            if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
        }
        let mut out = String::from("\u{feff}id,name,zone,quantity,unit,duration_days,predecessors,start,finish,total_float,critical,rate_source,element_count,status\n");
        for t in &self.tasks {
            let preds = t
                .preds
                .iter()
                .map(|&(p, lag)| if lag == 0 { self.tasks[p].id.clone() } else { format!("{}+{lag}", self.tasks[p].id) })
                .collect::<Vec<_>>()
                .join(";");
            out.push_str(&format!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{},preliminary\n",
                t.id,
                cell(&t.name),
                t.zone.map_or(String::new(), |z| z.to_string()),
                t.quantity.map_or(String::new(), |q| format!("{q:.3}")),
                t.unit.map_or("", |u| u.label()),
                t.duration,
                preds,
                t.start.iso(),
                t.finish.iso(),
                t.float,
                t.critical,
                cell(&t.rate_source),
                t.elements.len()
            ));
        }
        out
    }

    /// Gantt chart as SVG. Bars sit on a working-day axis; critical tasks are red.
    pub fn gantt_svg(&self, cal: &Calendar, title: &str) -> String {
        use std::fmt::Write;
        let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
        let (label_w, row_h, top, px) = (270.0, 20.0, 70.0, 6.0);
        let width = label_w + self.project_days.max(1) as f64 * px + 40.0;
        let height = top + self.tasks.len() as f64 * row_h + 30.0;
        let mut o = String::new();
        let _ = write!(
            o,
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width:.0} {height:.0}" width="{width:.0}" height="{height:.0}" font-family="Segoe UI, Tahoma, Arial, sans-serif">
<style>.bg{{fill:#fff}} .t{{font-size:11px;fill:#222}} .h{{font-size:15px;font-weight:700;fill:#111}} .s{{font-size:11px;fill:#555}}
.bar{{fill:#3a6ea5}} .bar.critical{{fill:#c0392b}} .ms{{fill:#555}} .ms.critical{{fill:#c0392b}} .grid{{stroke:#e2e2e2;stroke-width:1}} .tick{{font-size:9px;fill:#666}}</style>
<rect class="bg" width="{width:.0}" height="{height:.0}"/>
<text class="h" x="12" y="22">{}</text>
<text class="s" x="12" y="40">{} working days, finish {}. Red = critical path.</text>
"##,
            esc(title),
            self.project_days,
            self.finish.iso()
        );
        let mut d = 0;
        while d <= self.project_days {
            let x = label_w + d as f64 * px;
            let _ = writeln!(o, r#"<line class="grid" x1="{x:.1}" y1="{:.1}" x2="{x:.1}" y2="{:.1}"/>"#, top - 8.0, height - 24.0);
            let _ = writeln!(o, r#"<text class="tick" x="{x:.1}" y="{:.1}">{}</text>"#, top - 12.0, cal.nth_workday(d).iso());
            d += 20;
        }
        for (i, t) in self.tasks.iter().enumerate() {
            let y = top + i as f64 * row_h;
            let _ = writeln!(o, r#"<text class="t" x="12" y="{:.1}">{} {}</text>"#, y + 13.0, t.id, esc(&t.name));
            let x = label_w + t.es as f64 * px;
            let class = if t.critical { " critical" } else { "" };
            if t.duration > 0 {
                let _ = writeln!(
                    o,
                    r#"<rect class="bar{class}" data-task="{}" x="{x:.1}" y="{:.1}" width="{:.1}" height="{:.1}"/>"#,
                    t.id,
                    y + 3.0,
                    t.duration as f64 * px,
                    row_h - 6.0
                );
            } else {
                let _ = writeln!(
                    o,
                    r#"<path class="ms{class}" data-task="{}" d="M {x:.1} {:.1} l 5 5 l -5 5 l -5 -5 Z"/>"#,
                    t.id,
                    y + 4.0
                );
            }
        }
        o.push_str("</svg>\n");
        o
    }
}
