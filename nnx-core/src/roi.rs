//! Apartment building feasibility and return on investment.
//!
//! For an `apartment_building` project (or any project the user switches it on for):
//! - **total units** = the sum of the counts of the unit types the user entered (`1BR x 8`, `2BR x 12`);
//! - **CAPEX** = a basis (the selected cost tier's total, the project's BOQ total, or a figure the user enters) plus the other costs the
//!   user lists (land, design fees, authority fees ...), each with its source;
//! - **annual rent** = for each unit type, count x the yearly rent the user entered for ONE unit;
//! - **gross yield %** = potential annual rent / CAPEX x 100;
//! - **payback (years)** = CAPEX / annual income, where the income is the net operating income (rent less the vacancy % and the yearly
//!   operating cost, when the user entered them) or, when neither was entered, the gross rent, and the report says which.
//!
//! No rent, price or ratio is built in. A unit type without a rent, a missing CAPEX basis or an incomplete basis is named, and the
//! figures that need it are not computed (a partial CAPEX or rent never produces a yield).

use crate::execution::{blank, round};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapexBasis {
    /// The selected tier's total of the cost breakdown.
    #[default]
    CostTier,
    /// The BOQ total.
    Boq,
    /// The amount in `capex_manual`.
    Manual,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnitType {
    pub name: String,
    pub count: u32,
    /// Yearly rent of ONE unit of this type.
    pub annual_rent: Option<f64>,
    /// Where the rent comes from (a market survey, an existing lease).
    pub rent_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtherCost {
    pub name: String,
    pub amount: f64,
    pub source: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RoiInputs {
    /// Switch the calculator on for a project that is not an apartment building.
    pub enabled: bool,
    pub currency: Option<String>,
    pub unit_types: Vec<UnitType>,
    pub capex_basis: CapexBasis,
    pub capex_manual: Option<f64>,
    pub capex_manual_source: Option<String>,
    pub other_costs: Vec<OtherCost>,
    /// Share of the year the units stand empty, 0 to 100.
    pub vacancy_pct: Option<f64>,
    pub vacancy_source: Option<String>,
    /// Yearly operating cost (maintenance, management, utilities of common areas, insurance).
    pub annual_opex: Option<f64>,
    pub opex_source: Option<String>,
}

impl RoiInputs {
    pub fn check(&self) -> Result<(), String> {
        for u in &self.unit_types {
            if u.name.trim().is_empty() {
                return Err("a unit type needs a name".into());
            }
            if u.count == 0 {
                return Err(format!("unit type '{}': the count must be at least 1", u.name));
            }
            if let Some(r) = u.annual_rent {
                if !r.is_finite() || r < 0.0 {
                    return Err(format!("unit type '{}': the yearly rent must be zero or more", u.name));
                }
                if blank(&u.rent_source) {
                    return Err(format!("unit type '{}': the rent needs its source (market survey, existing lease)", u.name));
                }
            }
        }
        for o in &self.other_costs {
            if o.name.trim().is_empty() || !o.amount.is_finite() || o.amount < 0.0 {
                return Err("an other cost needs a name and an amount of zero or more".into());
            }
            if o.source.trim().is_empty() {
                return Err(format!("other cost '{}' needs its source", o.name));
            }
        }
        if let Some(m) = self.capex_manual {
            if !m.is_finite() || m < 0.0 {
                return Err("the CAPEX amount must be zero or more".into());
            }
            if blank(&self.capex_manual_source) {
                return Err("the CAPEX amount needs its source".into());
            }
        }
        if let Some(v) = self.vacancy_pct {
            if !v.is_finite() || !(0.0..=100.0).contains(&v) {
                return Err("the vacancy must be between 0 and 100 percent".into());
            }
            if blank(&self.vacancy_source) {
                return Err("the vacancy allowance needs its source".into());
            }
        }
        if let Some(o) = self.annual_opex {
            if !o.is_finite() || o < 0.0 {
                return Err("the yearly operating cost must be zero or more".into());
            }
            if blank(&self.opex_source) {
                return Err("the yearly operating cost needs its source".into());
            }
        }
        Ok(())
    }
}

/// The project figures the calculation draws on.
#[derive(Debug, Clone, Default)]
pub struct RoiContext {
    pub is_apartment_building: bool,
    /// Total of the selected cost tier and whether it is complete.
    pub tier_total: Option<(f64, bool)>,
    /// BOQ total and whether every item is priced.
    pub boq_total: Option<(f64, bool)>,
    pub floors: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UnitLine {
    pub name: String,
    pub count: u32,
    pub annual_rent: Option<f64>,
    pub annual_total: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoiReport {
    pub active: bool,
    pub currency: Option<String>,
    pub total_units: u32,
    pub units_per_floor: Option<f64>,
    pub unit_lines: Vec<UnitLine>,
    pub capex_basis: CapexBasis,
    pub capex_base: Option<f64>,
    pub other_costs: f64,
    pub capex: Option<f64>,
    pub gross_rent: Option<f64>,
    pub vacancy_loss: Option<f64>,
    pub opex: Option<f64>,
    pub net_income: Option<f64>,
    /// `gross rent` or `net operating income`: the income the payback uses.
    pub payback_basis: &'static str,
    pub gross_yield_pct: Option<f64>,
    pub payback_years: Option<f64>,
    pub capex_per_unit: Option<f64>,
    /// What stops a figure from being computed, by name.
    pub missing: Vec<String>,
    pub notes: Vec<String>,
}

pub fn compute(inp: &RoiInputs, ctx: &RoiContext) -> RoiReport {
    let active = ctx.is_apartment_building || inp.enabled;
    let mut missing = Vec::new();
    let mut notes = Vec::new();
    let total_units: u32 = inp.unit_types.iter().map(|u| u.count).sum();
    let unit_lines: Vec<UnitLine> = inp.unit_types.iter().map(|u| UnitLine { name: u.name.clone(), count: u.count, annual_rent: u.annual_rent, annual_total: u.annual_rent.map(|r| round(r * f64::from(u.count), 2)) }).collect();
    let mut rep = RoiReport {
        active,
        currency: inp.currency.clone(),
        total_units,
        units_per_floor: ctx.floors.filter(|f| *f > 0 && total_units > 0).map(|f| round(f64::from(total_units) / f64::from(f), 2)),
        unit_lines,
        capex_basis: inp.capex_basis,
        capex_base: None,
        other_costs: round(inp.other_costs.iter().map(|o| o.amount).sum(), 2),
        capex: None,
        gross_rent: None,
        vacancy_loss: None,
        opex: inp.annual_opex,
        net_income: None,
        payback_basis: "gross rent",
        gross_yield_pct: None,
        payback_years: None,
        capex_per_unit: None,
        missing: Vec::new(),
        notes: Vec::new(),
    };
    if !active {
        notes.push("the calculator is for apartment buildings: set the building type to apartment building or switch it on for this project".into());
        rep.notes = notes;
        return rep;
    }
    if inp.unit_types.is_empty() {
        missing.push("unit types and counts".into());
    }

    // CAPEX
    let base = match inp.capex_basis {
        CapexBasis::CostTier => match ctx.tier_total {
            Some((t, complete)) if t > 0.0 => {
                if !complete {
                    missing.push("a complete cost breakdown for the selected tier (its total is PARTIAL)".into());
                }
                complete.then_some(t)
            }
            _ => {
                missing.push("the selected tier's cost total (select a tier and enter its cost lines)".into());
                None
            }
        },
        CapexBasis::Boq => match ctx.boq_total {
            Some((t, complete)) if t > 0.0 => {
                if !complete {
                    missing.push("a complete BOQ (some items have no rate: its total is PARTIAL)".into());
                }
                complete.then_some(t)
            }
            _ => {
                missing.push("the BOQ total (price the BOQ items)".into());
                None
            }
        },
        CapexBasis::Manual => {
            if inp.capex_manual.is_none() {
                missing.push("the CAPEX amount".into());
            }
            inp.capex_manual
        }
    };
    rep.capex_base = base.map(|b| round(b, 2));
    rep.capex = base.map(|b| round(b + rep.other_costs, 2)).filter(|c| *c > 0.0);
    if inp.other_costs.is_empty() {
        notes.push("no other costs entered (land, design and authority fees): the CAPEX is the construction cost alone".into());
    }

    // Rent
    let rents: Vec<Option<f64>> = inp.unit_types.iter().map(|u| u.annual_rent).collect();
    for u in inp.unit_types.iter().filter(|u| u.annual_rent.is_none()) {
        missing.push(format!("yearly rent of unit type '{}'", u.name));
    }
    if !inp.unit_types.is_empty() && rents.iter().all(Option::is_some) {
        rep.gross_rent = Some(round(inp.unit_types.iter().map(|u| u.annual_rent.unwrap_or(0.0) * f64::from(u.count)).sum(), 2));
    }

    // Net income
    if let Some(g) = rep.gross_rent {
        let vacancy = inp.vacancy_pct.map(|v| round(g * v / 100.0, 2));
        rep.vacancy_loss = vacancy;
        if vacancy.is_some() || inp.annual_opex.is_some() {
            rep.net_income = Some(round(g - vacancy.unwrap_or(0.0) - inp.annual_opex.unwrap_or(0.0), 2));
            rep.payback_basis = "net operating income";
        } else {
            notes.push("no vacancy or operating cost entered: the payback uses the gross rent".into());
        }
    }

    // Yield and payback need a complete CAPEX and a complete rent.
    if let (Some(c), Some(g)) = (rep.capex, rep.gross_rent) {
        rep.gross_yield_pct = Some(round(g / c * 100.0, 2));
        rep.capex_per_unit = (total_units > 0).then(|| round(c / f64::from(total_units), 2));
        let income = rep.net_income.unwrap_or(g);
        if income > 0.0 {
            rep.payback_years = Some(round(c / income, 2));
        } else {
            missing.push("a positive annual income (the costs and vacancy entered exceed the rent): no payback".into());
        }
    }
    rep.missing = missing;
    rep.notes = notes;
    rep
}
