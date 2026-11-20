//! Construction cost by tier and by WBS phase.
//!
//! Five tiers (shell with materials, shell labour only, commercial finish, deluxe finish, luxury palace finish), each broken down over
//! the six WBS phases of the 4D schedule (`schedule::PHASES`). A tier's cost is the sum of its **cost lines**; a line is a rate YOU enter
//! with its source, multiplied by a quantity the model measures:
//! - `gross_area`: the gross floor area of the rooms (`SPACE.GROSS.AREA`), for a rate per m2;
//! - `qto`: the quantity of a take-off code (`SLAB.VOL`, or a prefix such as `FLOOR.AREA.*`), for a rate per its unit;
//! - `lump`: a fixed amount you enter (the `rate` is the amount).
//!
//! No market price is built in. A phase a tier is expected to price (the shell tiers: substructure and superstructure; the finish
//! tiers: all six) that has no line is named, and the tier's total is marked PARTIAL. A line whose quantity the model does not have is
//! listed and left out of the money. Per-m2 cost is shown only when a gross area and a total exist.

use crate::execution::round;
use crate::qto::QtoItem;
use crate::schedule::PHASES;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    ShellWithMaterials,
    ShellLaborOnly,
    CommercialFinish,
    DeluxeFinish,
    LuxuryPalaceFinish,
}

impl Tier {
    pub const ALL: [Tier; 5] = [Tier::ShellWithMaterials, Tier::ShellLaborOnly, Tier::CommercialFinish, Tier::DeluxeFinish, Tier::LuxuryPalaceFinish];

    pub fn id(self) -> &'static str {
        match self {
            Tier::ShellWithMaterials => "shell_with_materials",
            Tier::ShellLaborOnly => "shell_labor_only",
            Tier::CommercialFinish => "commercial_finish",
            Tier::DeluxeFinish => "deluxe_finish",
            Tier::LuxuryPalaceFinish => "luxury_palace_finish",
        }
    }

    /// `(English, Arabic)`.
    pub fn label(self) -> (&'static str, &'static str) {
        match self {
            Tier::ShellWithMaterials => ("Shell with materials", "هيكل بالمواد"),
            Tier::ShellLaborOnly => ("Shell, labour only", "هيكل عمالة فقط"),
            Tier::CommercialFinish => ("Commercial finish", "تشطيب تجاري"),
            Tier::DeluxeFinish => ("Deluxe finish", "تشطيب ديلوكس"),
            Tier::LuxuryPalaceFinish => ("Luxury palace finish", "تشطيب قصور فاخر"),
        }
    }

    /// The phases (indexes of `PHASES`) the tier's scope covers. A scope definition, not a price: the shell tiers stop after the frame.
    pub fn expected_phases(self) -> &'static [usize] {
        match self {
            Tier::ShellWithMaterials | Tier::ShellLaborOnly => &[0, 1],
            _ => &[0, 1, 2, 3, 4, 5],
        }
    }
}

/// What a line's rate multiplies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Basis {
    GrossArea,
    Qto { code: String },
    Lump,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostLine {
    pub tier: Tier,
    /// One of `schedule::PHASES`.
    pub phase: String,
    pub basis: Basis,
    /// Per unit of the basis; for `lump` the amount itself.
    pub rate: f64,
    pub label: Option<String>,
    /// Where the rate comes from (a quotation, company records, a published schedule of rates).
    pub source: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CostInputs {
    pub selected: Option<Tier>,
    pub currency: Option<String>,
    pub lines: Vec<CostLine>,
}

impl CostInputs {
    pub fn check(&self) -> Result<(), String> {
        for (i, l) in self.lines.iter().enumerate() {
            let n = i + 1;
            if !PHASES.contains(&l.phase.as_str()) {
                return Err(format!("cost line {n}: '{}' is not a WBS phase ({})", l.phase, PHASES.join(", ")));
            }
            if !l.rate.is_finite() || l.rate < 0.0 {
                return Err(format!("cost line {n}: the rate must be zero or more"));
            }
            if l.source.trim().is_empty() {
                return Err(format!("cost line {n}: the rate needs its source (quotation, company records, schedule of rates)"));
            }
            if let Basis::Qto { code } = &l.basis
                && code.trim().is_empty()
            {
                return Err(format!("cost line {n}: the take-off code is empty"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LineResult {
    pub phase: String,
    pub basis: Basis,
    pub label: String,
    pub unit: Option<String>,
    pub quantity: Option<f64>,
    pub rate: f64,
    pub amount: Option<f64>,
    pub source: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PhaseTotal {
    pub phase: &'static str,
    pub amount: f64,
    pub lines: usize,
    pub priced: usize,
    /// The tier's scope includes this phase.
    pub expected: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TierResult {
    pub tier: Tier,
    pub id: &'static str,
    pub en: &'static str,
    pub ar: &'static str,
    pub lines: Vec<LineResult>,
    pub phases: Vec<PhaseTotal>,
    pub total: f64,
    /// Total divided by the gross floor area; only when both exist.
    pub per_m2: Option<f64>,
    /// Every expected phase has a priced line and no line is unpriced.
    pub complete: bool,
    pub missing_phases: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CostReport {
    pub selected: Option<Tier>,
    pub currency: Option<String>,
    pub gross_area_m2: Option<f64>,
    pub tiers: Vec<TierResult>,
    pub notes: Vec<String>,
}

impl CostReport {
    pub fn selected_result(&self) -> Option<&TierResult> {
        let s = self.selected?;
        self.tiers.iter().find(|t| t.tier == s)
    }
}

fn matches(entry: &str, code: &str) -> bool {
    match entry.strip_suffix('*') {
        Some(p) => code.starts_with(p),
        None => code == entry,
    }
}

/// `(quantity, unit)` of a basis, or the reason there is none.
fn quantity(items: &[QtoItem], basis: &Basis) -> Result<(Option<f64>, Option<String>), String> {
    match basis {
        Basis::Lump => Ok((None, None)),
        Basis::GrossArea => match items.iter().find(|i| i.code == "SPACE.GROSS.AREA" && i.quantity > 0.0) {
            Some(i) => Ok((Some(round(i.quantity, 3)), Some("m2".into()))),
            None => Err("the model has no rooms, so there is no gross floor area".into()),
        },
        Basis::Qto { code } => {
            let hit: Vec<&QtoItem> = items.iter().filter(|i| matches(code.trim(), &i.code) && i.quantity > 0.0).collect();
            let Some(first) = hit.first() else { return Err(format!("the take-off has no quantity for '{code}'")) };
            // A prefix can match items of different units (counts and areas): keep the first unit only.
            let unit = first.unit;
            Ok((Some(round(hit.iter().filter(|i| i.unit == unit).map(|i| i.quantity).sum(), 3)), Some(unit.label().to_string())))
        }
    }
}

pub fn compute(items: &[QtoItem], inp: &CostInputs) -> CostReport {
    let gross = items.iter().find(|i| i.code == "SPACE.GROSS.AREA" && i.quantity > 0.0).map(|i| round(i.quantity, 3));
    let mut notes = Vec::new();
    let mut tiers = Vec::new();
    for tier in Tier::ALL {
        let mine: Vec<&CostLine> = inp.lines.iter().filter(|l| l.tier == tier).collect();
        if mine.is_empty() && inp.selected != Some(tier) {
            continue;
        }
        let mut lines = Vec::new();
        for l in mine {
            let (q, unit, note) = match quantity(items, &l.basis) {
                Ok((q, u)) => (q, u, None),
                Err(why) => (None, None, Some(why)),
            };
            let amount = match (&l.basis, q) {
                (Basis::Lump, _) => Some(round(l.rate, 2)),
                (_, Some(q)) => Some(round(l.rate * q, 2)),
                _ => None,
            };
            let label = l.label.clone().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| match &l.basis {
                Basis::GrossArea => "Gross floor area".to_string(),
                Basis::Qto { code } => code.clone(),
                Basis::Lump => "Lump sum".to_string(),
            });
            lines.push(LineResult { phase: l.phase.clone(), basis: l.basis.clone(), label, unit, quantity: q, rate: l.rate, amount, source: l.source.clone(), note });
        }
        let expected = tier.expected_phases();
        let phases: Vec<PhaseTotal> = PHASES
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let in_phase: Vec<&LineResult> = lines.iter().filter(|l| l.phase == *p).collect();
                PhaseTotal { phase: p, amount: round(in_phase.iter().filter_map(|l| l.amount).sum(), 2), lines: in_phase.len(), priced: in_phase.iter().filter(|l| l.amount.is_some()).count(), expected: expected.contains(&i) }
            })
            .collect();
        let missing_phases: Vec<&'static str> = phases.iter().filter(|p| p.expected && p.priced == 0).map(|p| p.phase).collect();
        let unpriced = lines.iter().filter(|l| l.amount.is_none()).count();
        let total = round(phases.iter().map(|p| p.amount).sum(), 2);
        let (en, ar) = tier.label();
        if !missing_phases.is_empty() {
            notes.push(format!("{en}: no priced line for {}: the total is PARTIAL", missing_phases.join(", ")));
        }
        if unpriced > 0 {
            notes.push(format!("{en}: {unpriced} line(s) have no quantity in the model and are left out of the total"));
        }
        let per_m2 = gross.filter(|a| *a > 0.0 && total > 0.0).map(|a| round(total / a, 2));
        tiers.push(TierResult { tier, id: tier.id(), en, ar, complete: missing_phases.is_empty() && unpriced == 0 && !lines.is_empty(), lines, phases, total, per_m2, missing_phases });
    }
    if inp.selected.is_none() {
        notes.push("no tier selected: choose the tier the contract is priced on".into());
    }
    if gross.is_none() {
        notes.push("the model has no rooms, so no gross floor area and no cost per m2".into());
    }
    CostReport { selected: inp.selected, currency: inp.currency.clone(), gross_area_m2: gross, tiers, notes }
}
