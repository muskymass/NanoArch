//! Yearly electricity bill estimate from connected loads and a tariff the user enters.
//!
//! Nothing here is a statutory or market value. The tariff (blocks, rates, fixed charge, VAT), its source and its date are the
//! caller's, as are the loads, their hours and the months they run. A missing value is refused by name; no tariff is built in
//! because tariffs change and differ by consumer class.
//!
//! Method (arithmetic only):
//! - Load energy for a month = `kw x demand_factor x hours_per_day x days_in_month x quantity`, summed over the months the load
//!   is active. Months have their calendar length (a non-leap year, 365 days).
//! - Optional on-site generation per month (kWh, the caller's figure) is deducted from that month's consumption, floored at zero.
//!   Surplus is reported and is NOT credited: no export tariff or net-metering rule is assumed.
//! - Energy charge = the month's billed kWh run through the tariff blocks (`up_to_kwh` is the cumulative upper bound inside one
//!   month; the last block has none). The fixed monthly charge is added, then VAT on the sum at the caller's percentage.
//! - Blocks are applied per month, which is how a monthly-billed tariff works; a tariff billed on another cycle is not modelled.
//!
//! It is a preliminary estimate from stated loads: no load diversity beyond the stated demand factor, no seasonal weather model,
//! no demand (kVA) charge, no power-factor penalty and no tariff change during the year.

use serde::{Deserialize, Serialize};

const DAYS: [f64; 12] = [31.0, 28.0, 31.0, 30.0, 31.0, 30.0, 31.0, 31.0, 30.0, 31.0, 30.0, 31.0];

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Load {
    #[serde(default)]
    pub name: String,
    /// Rated electrical power of one unit, kW.
    #[serde(default)]
    pub kw: Option<f64>,
    #[serde(default)]
    pub quantity: Option<f64>,
    /// Share of the rating drawn while running, 0 to 1.
    #[serde(default)]
    pub demand_factor: Option<f64>,
    #[serde(default)]
    pub hours_per_day: Option<f64>,
    /// Months (1 to 12) in which the load runs.
    #[serde(default)]
    pub active_months: Vec<u32>,
    /// Where the rating and the hours came from.
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Block {
    /// Cumulative upper bound of the block inside one month, kWh. Absent only on the last block.
    #[serde(default)]
    pub up_to_kwh: Option<f64>,
    #[serde(default)]
    pub rate_per_kwh: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Tariff {
    #[serde(default)]
    pub blocks: Vec<Block>,
    /// Fixed charge per billing month (zero must be stated).
    #[serde(default)]
    pub fixed_per_month: Option<f64>,
    /// VAT as a percentage of energy plus fixed charge (zero must be stated).
    #[serde(default)]
    pub vat_pct: Option<f64>,
    /// Document or page the tariff was read from.
    #[serde(default)]
    pub source: Option<String>,
    /// Date of that source.
    #[serde(default)]
    pub source_date: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Input {
    #[serde(default)]
    pub loads: Vec<Load>,
    #[serde(default)]
    pub tariff: Tariff,
    /// On-site generation per month, kWh, January first. Optional; twelve values when given.
    #[serde(default)]
    pub generation_kwh_per_month: Option<Vec<f64>>,
    #[serde(default)]
    pub generation_source: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MonthRow {
    pub month: u32,
    pub load_kwh: f64,
    pub generation_kwh: f64,
    pub billed_kwh: f64,
    pub surplus_kwh: f64,
    pub energy_charge: f64,
    pub fixed_charge: f64,
    pub vat: f64,
    pub total: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoadRow {
    pub name: String,
    pub kwh_per_year: f64,
    pub share_pct: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub months: Vec<MonthRow>,
    pub loads: Vec<LoadRow>,
    pub load_kwh_per_year: f64,
    pub billed_kwh_per_year: f64,
    pub surplus_kwh_per_year: f64,
    pub energy_charge_per_year: f64,
    pub fixed_charge_per_year: f64,
    pub vat_per_year: f64,
    pub total_per_year: f64,
    /// Total per billed kWh; none when nothing is billed.
    pub average_rate_per_kwh: Option<f64>,
    pub tariff_source: String,
    pub tariff_source_date: String,
    pub notes: Vec<String>,
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn need(v: Option<f64>, field: &str) -> Result<f64, String> {
    v.ok_or_else(|| format!("{field} is required and has no default"))
}

fn positive(v: Option<f64>, field: &str) -> Result<f64, String> {
    let v = need(v, field)?;
    if v.is_finite() && v > 0.0 { Ok(v) } else { Err(format!("{field} must be a positive number, got {v}")) }
}

fn non_negative(v: Option<f64>, field: &str) -> Result<f64, String> {
    let v = v.ok_or_else(|| format!("{field} is required and has no default (state 0 if there is none)"))?;
    if v.is_finite() && v >= 0.0 { Ok(v) } else { Err(format!("{field} must be zero or more, got {v}")) }
}

fn text(v: &Option<String>, field: &str) -> Result<String, String> {
    match v.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => Ok(s.to_string()),
        _ => Err(format!("{field} is required and has no default")),
    }
}

/// Validated tariff blocks as `(upper bound or infinity, rate)` in ascending order.
fn blocks(t: &Tariff) -> Result<Vec<(f64, f64)>, String> {
    if t.blocks.is_empty() {
        return Err("tariff.blocks is required and has no default (one block with no up_to_kwh is a flat rate)".into());
    }
    let last = t.blocks.len() - 1;
    let mut out = Vec::with_capacity(t.blocks.len());
    let mut prev = 0.0;
    for (i, b) in t.blocks.iter().enumerate() {
        let rate = non_negative(b.rate_per_kwh, &format!("tariff.blocks[{i}].rate_per_kwh"))?;
        let bound = match (b.up_to_kwh, i == last) {
            (None, true) => f64::INFINITY,
            (None, false) => return Err(format!("tariff.blocks[{i}].up_to_kwh is required on every block except the last")),
            (Some(_), true) => return Err(format!("tariff.blocks[{i}] is the last block and must have no up_to_kwh (it covers all remaining kWh)")),
            (Some(v), false) => {
                if !(v.is_finite() && v > prev) {
                    return Err(format!("tariff.blocks[{i}].up_to_kwh must be above the previous bound {prev}, got {v}"));
                }
                v
            }
        };
        prev = if bound.is_finite() { bound } else { prev };
        out.push((bound, rate));
    }
    Ok(out)
}

fn energy_charge(kwh: f64, blocks: &[(f64, f64)]) -> f64 {
    let mut left = kwh;
    let mut lower = 0.0;
    let mut sum = 0.0;
    for &(upper, rate) in blocks {
        if left <= 0.0 {
            break;
        }
        let width = upper - lower;
        let take = left.min(width);
        sum += take * rate;
        left -= take;
        lower = upper;
    }
    sum
}

pub fn estimate(input: &Input) -> Result<Report, String> {
    if input.loads.is_empty() {
        return Err("loads is required: list at least one load with kw, quantity, demand_factor, hours_per_day and active_months".into());
    }
    let t = &input.tariff;
    let tariff_source = text(&t.source, "tariff.source")?;
    let tariff_source_date = text(&t.source_date, "tariff.source_date")?;
    let blocks = blocks(t)?;
    let fixed = non_negative(t.fixed_per_month, "tariff.fixed_per_month")?;
    let vat_pct = non_negative(t.vat_pct, "tariff.vat_pct")?;

    let generation = match &input.generation_kwh_per_month {
        None => [0.0; 12],
        Some(g) => {
            if g.len() != 12 {
                return Err(format!("generation_kwh_per_month needs 12 values (January first), got {}", g.len()));
            }
            text(&input.generation_source, "generation_source")?;
            let mut a = [0.0; 12];
            for (i, v) in g.iter().enumerate() {
                if !(v.is_finite() && *v >= 0.0) {
                    return Err(format!("generation_kwh_per_month[{i}] must be zero or more, got {v}"));
                }
                a[i] = *v;
            }
            a
        }
    };

    let mut monthly_load = [0.0_f64; 12];
    let mut loads = Vec::with_capacity(input.loads.len());
    for (i, l) in input.loads.iter().enumerate() {
        let name = if l.name.trim().is_empty() { format!("load {}", i + 1) } else { l.name.trim().to_string() };
        let at = |f: &str| format!("loads[{i}] ({name}).{f}");
        let kw = positive(l.kw, &at("kw"))?;
        let qty = positive(l.quantity, &at("quantity"))?;
        let df = positive(l.demand_factor, &at("demand_factor"))?;
        if df > 1.0 {
            return Err(format!("{} must be between 0 and 1, got {df}", at("demand_factor")));
        }
        let hours = positive(l.hours_per_day, &at("hours_per_day"))?;
        if hours > 24.0 {
            return Err(format!("{} cannot exceed 24, got {hours}", at("hours_per_day")));
        }
        text(&l.source, &at("source"))?;
        if l.active_months.is_empty() {
            return Err(format!("{} is required and has no default", at("active_months")));
        }
        let mut seen = [false; 12];
        let mut year = 0.0;
        for &m in &l.active_months {
            if !(1..=12).contains(&m) {
                return Err(format!("{} holds {m}; months run from 1 to 12", at("active_months")));
            }
            let idx = (m - 1) as usize;
            if seen[idx] {
                return Err(format!("{} lists month {m} twice", at("active_months")));
            }
            seen[idx] = true;
            let kwh = kw * df * hours * DAYS[idx] * qty;
            monthly_load[idx] += kwh;
            year += kwh;
        }
        loads.push(LoadRow { name, kwh_per_year: year, share_pct: 0.0 });
    }

    let load_total: f64 = monthly_load.iter().sum();
    for l in &mut loads {
        l.share_pct = round2(l.kwh_per_year / load_total * 100.0);
        l.kwh_per_year = round2(l.kwh_per_year);
    }

    let mut months = Vec::with_capacity(12);
    let (mut billed_y, mut surplus_y, mut energy_y, mut fixed_y, mut vat_y, mut total_y) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for m in 0..12 {
        let billed = (monthly_load[m] - generation[m]).max(0.0);
        let surplus = (generation[m] - monthly_load[m]).max(0.0);
        let energy = energy_charge(billed, &blocks);
        let vat = (energy + fixed) * vat_pct / 100.0;
        let total = energy + fixed + vat;
        billed_y += billed;
        surplus_y += surplus;
        energy_y += energy;
        fixed_y += fixed;
        vat_y += vat;
        total_y += total;
        months.push(MonthRow {
            month: m as u32 + 1,
            load_kwh: round2(monthly_load[m]),
            generation_kwh: round2(generation[m]),
            billed_kwh: round2(billed),
            surplus_kwh: round2(surplus),
            energy_charge: round2(energy),
            fixed_charge: round2(fixed),
            vat: round2(vat),
            total: round2(total),
        });
    }

    let mut notes = vec![
        "Estimate from the stated loads and the tariff you entered; it is not a utility bill and does not replace an energy audit.".to_string(),
        "Blocks are applied per calendar month. Demand (kVA) charges, power-factor penalties and mid-year tariff changes are not modelled.".to_string(),
    ];
    if input.generation_kwh_per_month.is_some() {
        notes.push("Generation is deducted from each month's consumption and floored at zero; surplus is not credited (no export tariff is assumed).".to_string());
    }
    Ok(Report {
        months,
        loads,
        load_kwh_per_year: round2(load_total),
        billed_kwh_per_year: round2(billed_y),
        surplus_kwh_per_year: round2(surplus_y),
        energy_charge_per_year: round2(energy_y),
        fixed_charge_per_year: round2(fixed_y),
        vat_per_year: round2(vat_y),
        total_per_year: round2(total_y),
        average_rate_per_kwh: if billed_y > 0.0 { Some(total_y / billed_y) } else { None },
        tariff_source,
        tariff_source_date,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(kw: f64, hours: f64, months: Vec<u32>) -> Load {
        Load {
            name: "ac".into(),
            kw: Some(kw),
            quantity: Some(1.0),
            demand_factor: Some(1.0),
            hours_per_day: Some(hours),
            active_months: months,
            source: Some("test data".into()),
        }
    }

    fn tariff(blocks: Vec<(Option<f64>, f64)>) -> Tariff {
        Tariff {
            blocks: blocks.into_iter().map(|(u, r)| Block { up_to_kwh: u, rate_per_kwh: Some(r) }).collect(),
            fixed_per_month: Some(0.0),
            vat_pct: Some(0.0),
            source: Some("test tariff".into()),
            source_date: Some("2026-01-01".into()),
        }
    }

    fn input(loads: Vec<Load>, t: Tariff) -> Input {
        Input { loads, tariff: t, generation_kwh_per_month: None, generation_source: None }
    }

    #[test]
    fn flat_rate_year_matches_hand_arithmetic() {
        // 1 kW x 10 h x 365 days = 3650 kWh at 0.5.
        let r = estimate(&input(vec![load(1.0, 10.0, (1..=12).collect())], tariff(vec![(None, 0.5)]))).unwrap();
        assert_eq!(r.load_kwh_per_year, 3650.0);
        assert_eq!(r.energy_charge_per_year, 1825.0);
        assert_eq!(r.total_per_year, 1825.0);
        assert_eq!(r.months[1].load_kwh, 280.0);
        assert_eq!(r.loads[0].share_pct, 100.0);
    }

    #[test]
    fn blocks_apply_per_month_with_fixed_charge_and_vat() {
        // January only: 10 kW x 10 h x 31 = 3100 kWh. Blocks: first 2000 at 0.1, rest at 0.3 -> 200 + 330 = 530.
        let mut t = tariff(vec![(Some(2000.0), 0.1), (None, 0.3)]);
        t.fixed_per_month = Some(10.0);
        t.vat_pct = Some(10.0);
        let r = estimate(&input(vec![load(10.0, 10.0, vec![1])], t)).unwrap();
        let jan = &r.months[0];
        assert_eq!(jan.energy_charge, 530.0);
        assert_eq!(jan.vat, 54.0);
        assert_eq!(jan.total, 594.0);
        // Other months bill only the fixed charge plus VAT.
        assert_eq!(r.months[1].total, 11.0);
        assert_eq!(r.fixed_charge_per_year, 120.0);
        assert_eq!(r.total_per_year, 594.0 + 11.0 * 11.0);
    }

    #[test]
    fn generation_offsets_consumption_and_surplus_is_not_credited() {
        let mut i = input(vec![load(1.0, 10.0, vec![1, 2])], tariff(vec![(None, 1.0)]));
        // Jan load 310, Feb load 280.
        let mut g = vec![0.0; 12];
        g[0] = 100.0;
        g[1] = 400.0;
        i.generation_kwh_per_month = Some(g);
        i.generation_source = Some("pv sizing".into());
        let r = estimate(&i).unwrap();
        assert_eq!(r.months[0].billed_kwh, 210.0);
        assert_eq!(r.months[1].billed_kwh, 0.0);
        assert_eq!(r.months[1].surplus_kwh, 120.0);
        assert_eq!(r.total_per_year, 210.0);
        assert!(r.notes.iter().any(|n| n.contains("surplus is not credited")));
    }

    #[test]
    fn missing_values_are_named_and_nothing_is_defaulted() {
        let ok = input(vec![load(1.0, 1.0, vec![1])], tariff(vec![(None, 0.1)]));
        let mut i = ok.clone();
        i.tariff.source = None;
        assert!(estimate(&i).unwrap_err().contains("tariff.source is required"));
        let mut i = ok.clone();
        i.tariff.source_date = Some("  ".into());
        assert!(estimate(&i).unwrap_err().contains("tariff.source_date is required"));
        let mut i = ok.clone();
        i.tariff.vat_pct = None;
        assert!(estimate(&i).unwrap_err().contains("tariff.vat_pct is required"));
        let mut i = ok.clone();
        i.tariff.fixed_per_month = None;
        assert!(estimate(&i).unwrap_err().contains("tariff.fixed_per_month is required"));
        let mut i = ok.clone();
        i.tariff.blocks.clear();
        assert!(estimate(&i).unwrap_err().contains("tariff.blocks is required"));
        let mut i = ok.clone();
        i.loads[0].hours_per_day = None;
        assert!(estimate(&i).unwrap_err().contains("hours_per_day is required"));
        let mut i = ok.clone();
        i.loads[0].demand_factor = None;
        assert!(estimate(&i).unwrap_err().contains("demand_factor is required"));
        let mut i = ok.clone();
        i.loads[0].active_months.clear();
        assert!(estimate(&i).unwrap_err().contains("active_months is required"));
        let mut i = ok.clone();
        i.loads[0].source = None;
        assert!(estimate(&i).unwrap_err().contains("source is required"));
        assert!(estimate(&Input::default()).unwrap_err().contains("loads is required"));
    }

    #[test]
    fn malformed_values_are_refused() {
        let ok = input(vec![load(1.0, 1.0, vec![1])], tariff(vec![(Some(100.0), 0.1), (None, 0.2)]));
        let mut i = ok.clone();
        i.loads[0].demand_factor = Some(1.5);
        assert!(estimate(&i).unwrap_err().contains("between 0 and 1"));
        let mut i = ok.clone();
        i.loads[0].hours_per_day = Some(25.0);
        assert!(estimate(&i).unwrap_err().contains("cannot exceed 24"));
        let mut i = ok.clone();
        i.loads[0].active_months = vec![13];
        assert!(estimate(&i).unwrap_err().contains("1 to 12"));
        let mut i = ok.clone();
        i.loads[0].active_months = vec![2, 2];
        assert!(estimate(&i).unwrap_err().contains("twice"));
        let mut i = ok.clone();
        i.tariff.blocks[1].up_to_kwh = Some(500.0);
        assert!(estimate(&i).unwrap_err().contains("last block"));
        let mut i = ok.clone();
        i.tariff.blocks[0].up_to_kwh = None;
        assert!(estimate(&i).unwrap_err().contains("every block except the last"));
        let mut i = ok.clone();
        i.tariff.blocks.insert(1, Block { up_to_kwh: Some(50.0), rate_per_kwh: Some(0.15) });
        assert!(estimate(&i).unwrap_err().contains("above the previous bound"));
        let mut i = ok.clone();
        i.generation_kwh_per_month = Some(vec![1.0; 11]);
        assert!(estimate(&i).unwrap_err().contains("12 values"));
        let mut i = ok.clone();
        i.generation_kwh_per_month = Some(vec![1.0; 12]);
        assert!(estimate(&i).unwrap_err().contains("generation_source is required"));
    }
}
