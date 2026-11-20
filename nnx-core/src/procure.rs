//! Long-lead material procurement: the latest date each long-lead item must be ordered.
//!
//! `order by = date the work needs the item on site (start or finish of the linked 4D activity or phase) - lead time - buffer`, in
//! calendar days. The lead time is what the supplier quoted and the buffer is your own safety margin; both are entered by the user, the
//! lead time with its source. Nothing is defaulted: an item without a lead time is listed without a date.
//!
//! Each dated item is judged against two reference dates:
//! - `as_of` (today, or the date the user enters): an order date already before it is `overdue`;
//! - the project start (the schedule's first activity): an order date before it means the order must be placed before work starts.

use crate::execution::{Activity, Link, Trigger, blank, link_date, parse_date};
use crate::schedule::Date;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Typical categories, offered by the page as a hint; the category is free text.
pub const CATEGORIES: [&str; 5] = ["elevator", "windows", "hvac", "finishes", "custom"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LongLeadItem {
    pub id: String,
    pub name: String,
    pub category: String,
    /// The activity or phase that needs the item.
    pub needed_for: Link,
    #[serde(default)]
    pub needed_at: Trigger,
    pub lead_days: Option<u32>,
    /// Safety margin in days; unset counts as zero and is shown as such.
    pub buffer_days: Option<u32>,
    /// Where the lead time comes from (a supplier quotation, a data sheet).
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProcureInputs {
    /// The reference date for "already passed", `YYYY-MM-DD`. Unset: the system date is used and said so.
    pub as_of: Option<String>,
    pub items: Vec<LongLeadItem>,
}

impl ProcureInputs {
    pub fn check(&self) -> Result<(), String> {
        if let Some(d) = &self.as_of {
            parse_date("as-of date", d)?;
        }
        let mut seen = BTreeSet::new();
        for it in &self.items {
            if it.id.trim().is_empty() || it.name.trim().is_empty() {
                return Err("a long-lead item needs an id and a name".into());
            }
            if !seen.insert(it.id.as_str()) {
                return Err(format!("the long-lead item id '{}' is used twice", it.id));
            }
            it.needed_for.check(&format!("item '{}'", it.id))?;
            if it.lead_days.is_some() && blank(&it.source) {
                return Err(format!("item '{}': the lead time needs its source (supplier quotation, data sheet)", it.id));
            }
            if it.lead_days.is_some_and(|d| d > 3650) || it.buffer_days.is_some_and(|d| d > 3650) {
                return Err(format!("item '{}': a lead time or buffer above 3650 days is not a number of days", it.id));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ItemResult {
    pub id: String,
    pub name: String,
    pub category: String,
    pub needed_for: Link,
    pub needed_at: Trigger,
    pub needed_on: Option<String>,
    pub lead_days: Option<u32>,
    pub buffer_days: u32,
    pub order_by: Option<String>,
    /// Days from `as_of` to the order date; negative when it has passed.
    pub days_to_order: Option<i64>,
    /// `overdue`, `due` (the order date is today), `ok` or `unknown`.
    pub status: &'static str,
    /// The order date is before the project start.
    pub before_project_start: bool,
    pub source: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcureReport {
    pub as_of: Option<String>,
    /// `entered` or `system clock`.
    pub as_of_source: &'static str,
    pub project_start: Option<String>,
    pub items: Vec<ItemResult>,
    pub overdue: usize,
    pub before_project_start: usize,
    pub unknown: usize,
    pub issues: Vec<String>,
}

/// `acts` is the computed schedule; `today` the system date (used when the user did not enter `as_of`); `has_elevator` is true when
/// the model carries an elevator shaft.
pub fn compute(inp: &ProcureInputs, acts: Option<&[Activity]>, today: Option<Date>, has_elevator: bool) -> ProcureReport {
    let mut issues = Vec::new();
    let (as_of, as_of_source) = match inp.as_of.as_deref().and_then(Date::parse) {
        Some(d) => (Some(d), "entered"),
        None => (today, "system clock"),
    };
    let start = acts.and_then(|a| a.iter().map(|x| x.start).min());
    if acts.is_none() && !inp.items.is_empty() {
        issues.push("the 4D schedule has not been computed (Schedule tab): no order dates".into());
    }
    let mut items: Vec<ItemResult> = Vec::new();
    for it in &inp.items {
        let buffer = it.buffer_days.unwrap_or(0);
        let mut note = None;
        let needed = match acts {
            Some(a) => match link_date(a, &it.needed_for, it.needed_at) {
                Ok(d) => Some(d),
                Err(why) => {
                    note = Some(why);
                    None
                }
            },
            None => None,
        };
        if it.lead_days.is_none() {
            note = Some("no lead time entered".into());
        }
        let order = match (needed, it.lead_days) {
            (Some(n), Some(l)) => Some(Date::from_days(n.to_days() - i64::from(l) - i64::from(buffer))),
            _ => None,
        };
        let days = order.zip(as_of).map(|(o, t)| o.to_days() - t.to_days());
        let status = match days {
            Some(d) if d < 0 => "overdue",
            Some(0) => "due",
            Some(_) => "ok",
            None => "unknown",
        };
        if status == "unknown" {
            issues.push(format!("item '{}': {}", it.id, note.as_deref().unwrap_or("no reference date")));
        }
        items.push(ItemResult {
            id: it.id.clone(),
            name: it.name.clone(),
            category: it.category.clone(),
            needed_for: it.needed_for.clone(),
            needed_at: it.needed_at,
            needed_on: needed.map(|d| d.iso()),
            lead_days: it.lead_days,
            buffer_days: buffer,
            order_by: order.map(|d| d.iso()),
            days_to_order: days,
            status,
            before_project_start: order.zip(start).is_some_and(|(o, s)| o < s),
            source: it.source.clone(),
            note,
        });
    }
    // Earliest order date first; items without a date after the dated ones.
    items.sort_by(|a, b| (a.order_by.is_none(), &a.order_by).cmp(&(b.order_by.is_none(), &b.order_by)));
    if has_elevator && !inp.items.iter().any(|i| i.category.eq_ignore_ascii_case("elevator")) {
        issues.push("the model has an elevator shaft but no long-lead item of category 'elevator' was entered".into());
    }
    let overdue = items.iter().filter(|i| i.status == "overdue").count();
    ProcureReport {
        as_of: as_of.map(|d| d.iso()),
        as_of_source,
        project_start: start.map(|d| d.iso()),
        overdue,
        before_project_start: items.iter().filter(|i| i.before_project_start).count(),
        unknown: items.iter().filter(|i| i.status == "unknown").count(),
        items,
        issues,
    }
}

impl ProcureReport {
    pub fn csv(&self) -> String {
        let cell = |s: &str| if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
        let mut o = String::from("\u{feff}id,item,category,needed_for,needed_on,lead_days,buffer_days,order_by,days_to_order,status,before_project_start,source\n");
        for i in &self.items {
            o.push_str(&format!(
                "{},{},{},{},{},{},{},{},{},{},{},{}\n",
                cell(&i.id),
                cell(&i.name),
                cell(&i.category),
                cell(&i.needed_for.label()),
                i.needed_on.as_deref().unwrap_or(""),
                i.lead_days.map_or(String::new(), |d| d.to_string()),
                i.buffer_days,
                i.order_by.as_deref().unwrap_or(""),
                i.days_to_order.map_or(String::new(), |d| d.to_string()),
                i.status,
                if i.before_project_start { "yes" } else { "no" },
                cell(i.source.as_deref().unwrap_or(""))
            ));
        }
        o
    }
}
