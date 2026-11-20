//! Execution inputs: everything Milestone 6 (cost tiers, tender pack, payment schedule, long-lead procurement, QA/QC, apartment ROI)
//! needs from the user, stored as ONE value of the project (`meta.execution`). Because the project is a model element, every edit is a
//! `SetProp` on the document's stack, so undo, redo, the dirty flag and the `.nnx` file follow with no extra bookkeeping.
//!
//! Nothing here supplies a price, a rate, a lead time, a rent or a percentage: each is entered by the user with its source, and a
//! value that is absent is reported by name and the figure that needs it is not computed.
//!
//! This module also holds what the features share: the schedule setup (start date, working week, holidays, zones and the production
//! rates the 4D schedule needs) and the link from a record to the schedule (an activity by its stable key, or a whole phase).

use crate::qto::Qto;
use crate::schedule::{self, Calendar, Date, PHASES, RateLib, Schedule};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub(crate) fn round(v: f64, d: i32) -> f64 {
    let m = 10f64.powi(d);
    (v * m).round() / m
}

pub(crate) fn blank(s: &Option<String>) -> bool {
    s.as_deref().is_none_or(|s| s.trim().is_empty())
}

pub(crate) fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `YYYY-MM-DD` or an error naming the field.
pub(crate) fn parse_date(what: &str, s: &str) -> Result<Date, String> {
    Date::parse(s.trim()).ok_or_else(|| format!("{what}: '{s}' is not a date (use YYYY-MM-DD)"))
}

/// Which end of an activity a record is tied to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    #[default]
    Start,
    Finish,
}

/// The place in the 4D schedule a record is tied to: one activity by its template key (`footing_pour`, `plaster`; a zoned activity
/// spans all its zones) or a whole phase (`Substructure` ... `Commissioning`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Link {
    Task { key: String },
    Phase { name: String },
}

impl Link {
    pub fn check(&self, what: &str) -> Result<(), String> {
        match self {
            Link::Task { key } if !schedule::template_keys().iter().any(|(k, _)| k == key) => Err(format!("{what}: '{key}' is not a schedule activity key")),
            Link::Phase { name } if !PHASES.contains(&name.as_str()) => Err(format!("{what}: '{name}' is not a schedule phase ({})", PHASES.join(", "))),
            _ => Ok(()),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Link::Task { key } => key.clone(),
            Link::Phase { name } => name.clone(),
        }
    }
}

/// One scheduled activity reduced to what the records need.
#[derive(Debug, Clone)]
pub struct Activity {
    pub key: &'static str,
    pub name: String,
    pub phase: &'static str,
    pub start: Date,
    pub finish: Date,
}

pub fn activities(s: &Schedule) -> Vec<Activity> {
    s.tasks.iter().map(|t| Activity { key: t.key, name: t.name.clone(), phase: t.phase, start: t.start, finish: t.finish }).collect()
}

/// The date a link and trigger point to. An optional activity the take-off has no work for is not in the schedule: that is an error
/// naming it, not a guess.
pub fn link_date(acts: &[Activity], link: &Link, trigger: Trigger) -> Result<Date, String> {
    let hits: Vec<&Activity> = match link {
        Link::Task { key } => acts.iter().filter(|a| a.key == key).collect(),
        Link::Phase { name } => acts.iter().filter(|a| a.phase == name).collect(),
    };
    if hits.is_empty() {
        return Err(format!("'{}' is not in the computed schedule (an activity the model has no work for is left out)", link.label()));
    }
    Ok(match trigger {
        Trigger::Start => hits.iter().map(|a| a.start).min(),
        Trigger::Finish => hits.iter().map(|a| a.finish).max(),
    }
    .unwrap_or(hits[0].start))
}

/// The calendar and the production rates of the 4D schedule. The page sends the same fields to the `schedule` command; stored here they
/// are the single copy every Milestone 6 feature reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScheduleSetup {
    pub start: String,
    /// Seven entries, Monday first.
    pub workdays: Vec<bool>,
    pub holidays: Vec<String>,
    pub zones: u32,
    /// Activity key -> `{ per_crew_day | fixed_days, crews, source }`.
    pub rates: Map<String, Value>,
}

/// Why a schedule could not be built.
#[derive(Debug, Clone, PartialEq)]
pub enum BuildError {
    /// Production rates or durations that are not set, by key.
    MissingRates(Vec<String>),
    Invalid(String),
}

impl ScheduleSetup {
    pub fn check(&self) -> Result<(), String> {
        parse_date("schedule start", &self.start)?;
        if self.workdays.len() != 7 {
            return Err("workdays needs seven entries, Monday first".into());
        }
        if !self.workdays.iter().any(|&w| w) {
            return Err("at least one working day a week is required".into());
        }
        for h in &self.holidays {
            parse_date("holiday", h)?;
        }
        serde_json::from_value::<RateLib>(serde_json::json!({ "rates": self.rates })).map_err(|e| format!("rates: {e}"))?;
        Ok(())
    }

    /// Builds the 4D schedule of the model's quantities.
    pub fn build(&self, model: &crate::Model, qto: &Qto) -> Result<(Schedule, Calendar), BuildError> {
        self.check().map_err(BuildError::Invalid)?;
        let lib: RateLib = serde_json::from_value(serde_json::json!({ "rates": self.rates })).map_err(|e| BuildError::Invalid(format!("rates: {e}")))?;
        let start = parse_date("schedule start", &self.start).map_err(BuildError::Invalid)?;
        let wd: [bool; 7] = self.workdays.clone().try_into().map_err(|_| BuildError::Invalid("workdays needs seven entries, Monday first".into()))?;
        let mut cal = Calendar::new(start, wd).ok_or_else(|| BuildError::Invalid("at least one working day a week is required".into()))?;
        for h in &self.holidays {
            cal.holidays.insert(parse_date("holiday", h).map_err(BuildError::Invalid)?);
        }
        match schedule::build_with_model(model, qto, &lib, &cal, self.zones.max(1) as usize) {
            Ok(s) => Ok((s, cal)),
            Err(schedule::ScheduleError::MissingRates(keys)) => Err(BuildError::MissingRates(keys)),
            Err(e) => Err(BuildError::Invalid(e.to_string())),
        }
    }
}

/// Every input of Milestone 6.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExecutionInputs {
    pub schedule: Option<ScheduleSetup>,
    pub cost: crate::costtier::CostInputs,
    pub tender: crate::tender::TenderInputs,
    pub payment: crate::payment::PaymentInputs,
    pub procurement: crate::procure::ProcureInputs,
    pub qaqc: crate::qaqc::QaInputs,
    pub roi: crate::roi::RoiInputs,
}

impl ExecutionInputs {
    pub fn check(&self) -> Result<(), String> {
        if let Some(s) = &self.schedule {
            s.check()?;
        }
        self.cost.check()?;
        self.tender.check()?;
        self.payment.check()?;
        self.procurement.check()?;
        self.qaqc.check()?;
        self.roi.check()
    }
}
