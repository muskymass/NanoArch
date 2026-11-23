//! Contractor bid comparison on the project's own BOQ.
//!
//! Every bidder prices the same item list with the same measured quantities (the BOQ report's rows), so the totals are comparable line
//! by line. Everything a bid contains comes from the user: contractor, quotation reference (`source`), quotation date, currency, the
//! unit rate per BOQ code and the bidder's own mark-ups. Nothing is invented or filled in:
//! - a bidder that left a measured item unpriced is reported by name and is **not ranked** (its total would not be like for like);
//!   the like-for-like subtotal over the items every bidder priced is given for all of them;
//! - a rate for a code that is not in the BOQ, a rate of zero or less, a bad date, a duplicate bidder or a currency that differs from
//!   the BOQ's are rejected by name before anything is computed.
//!
//! Amount of a line = `rate x quantity` with the quantity rounded to 3 decimals, exactly as the BOQ does; mark-ups compound in the
//! order given, as the BOQ's do. The comparison is arithmetic only: it does not judge a rate as low or high, and the cheapest priced
//! total is not a recommendation to award.

use crate::boq::{BoqReport, BoqRow, MarkupLine, valid_date};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// At most this many bidders are compared (a tender sheet wider than this is not readable).
pub const MAX_BIDS: usize = 20;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BidMarkup {
    pub name: String,
    /// Percent, applied on the running total. Negative values are discounts.
    pub pct: f64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Bid {
    pub contractor: String,
    /// The bidder's quotation reference or document name.
    pub source: String,
    /// Quotation date, `YYYY-MM-DD`.
    pub date: String,
    pub currency: String,
    /// Unit rate per BOQ code. A code that is absent is unpriced by this bidder.
    #[serde(default)]
    pub rates: BTreeMap<String, f64>,
    #[serde(default)]
    pub markups: Vec<BidMarkup>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BidCell {
    pub rate: Option<f64>,
    pub amount: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BidLineRow {
    pub no: String,
    pub code: String,
    pub description: String,
    pub unit: String,
    pub quantity: f64,
    /// One cell per bid, in the order the bids were given.
    pub cells: Vec<BidCell>,
    /// Contractors with the lowest rate on this line (several on a tie); empty when nobody priced it.
    pub lowest: Vec<String>,
    /// Highest rate minus lowest rate, when at least two bidders priced the line.
    pub spread: Option<f64>,
    /// The project's own BOQ rate for the line, when the user entered one (for reference, not a benchmark).
    pub own_rate: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BidTotal {
    pub contractor: String,
    pub source: String,
    pub date: String,
    pub priced_lines: usize,
    /// Measured items this bidder left unpriced (codes).
    pub unpriced: Vec<String>,
    pub complete: bool,
    pub subtotal: f64,
    pub markups: Vec<MarkupLine>,
    pub total: f64,
    /// Subtotal over the items every bidder priced (before mark-ups): the like-for-like figure.
    pub common_subtotal: f64,
    /// 1 = lowest total among complete bids (ties share a rank); `None` for an incomplete bid.
    pub rank: Option<usize>,
    /// Total minus the lowest complete total, as an amount and a percent of that lowest total.
    pub above_lowest: Option<f64>,
    pub above_lowest_pct: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BidComparison {
    pub currency: String,
    pub bidders: Vec<String>,
    pub lines: Vec<BidLineRow>,
    pub totals: Vec<BidTotal>,
    /// Items priced by every bidder.
    pub common_lines: usize,
    /// Measured items no bidder priced.
    pub priced_by_nobody: Vec<String>,
    pub lowest_complete: Option<String>,
    pub notes: Vec<String>,
}

fn round(v: f64, d: i32) -> f64 {
    let m = 10f64.powi(d);
    (v * m).round() / m
}

/// Rejects, by name, everything that would make the comparison unsound.
pub fn validate(boq: &BoqReport, bids: &[Bid]) -> Result<(), String> {
    if bids.len() < 2 {
        return Err("bid comparison needs at least two bids".into());
    }
    if bids.len() > MAX_BIDS {
        return Err(format!("at most {MAX_BIDS} bids can be compared, got {}", bids.len()));
    }
    let known: BTreeSet<&str> = boq.rows.iter().filter(|r| !r.reference_only).map(|r| r.code.as_str()).collect();
    if known.is_empty() {
        return Err("the BOQ has no measured items to price: draw the model first".into());
    }
    let mut seen = BTreeSet::new();
    for (i, b) in bids.iter().enumerate() {
        if b.contractor.trim().is_empty() {
            return Err(format!("bid {}: contractor name is required", i + 1));
        }
        let who = format!("bid '{}'", b.contractor.trim());
        if !seen.insert(b.contractor.trim().to_lowercase()) {
            return Err(format!("{who}: the contractor appears twice"));
        }
        if b.source.trim().is_empty() {
            return Err(format!("{who}: source (quotation reference) is required"));
        }
        if !valid_date(&b.date) {
            return Err(format!("{who}: date '{}' is not a real YYYY-MM-DD day", b.date));
        }
        if b.currency.trim().is_empty() {
            return Err(format!("{who}: currency is required"));
        }
        if b.rates.is_empty() {
            return Err(format!("{who}: no rates entered"));
        }
        for (code, rate) in &b.rates {
            if !known.contains(code.as_str()) {
                return Err(format!("{who}: '{code}' is not a measured item of this BOQ"));
            }
            if !rate.is_finite() || *rate <= 0.0 {
                return Err(format!("{who}: rate for '{code}' must be a number above zero"));
            }
        }
        let mut mk = BTreeSet::new();
        for m in &b.markups {
            if m.name.trim().is_empty() {
                return Err(format!("{who}: a mark-up has no name"));
            }
            if !mk.insert(m.name.trim().to_lowercase()) {
                return Err(format!("{who}: mark-up '{}' appears twice", m.name.trim()));
            }
            if !m.pct.is_finite() || m.pct <= -100.0 {
                return Err(format!("{who}: mark-up '{}' must be a number above -100", m.name.trim()));
            }
        }
    }
    let first = bids[0].currency.trim();
    if let Some(b) = bids.iter().find(|b| b.currency.trim() != first) {
        return Err(format!("bid '{}': currency '{}' differs from '{}': convert before comparing", b.contractor.trim(), b.currency.trim(), first));
    }
    if let Some(c) = boq.project.currency.as_deref().filter(|c| !c.trim().is_empty())
        && c.trim() != first
    {
        return Err(format!("bids are in '{first}' but the BOQ currency is '{}'", c.trim()));
    }
    Ok(())
}

/// Compares the bids against the BOQ's measured rows.
pub fn compare(boq: &BoqReport, bids: &[Bid]) -> Result<BidComparison, String> {
    validate(boq, bids)?;
    let rows: Vec<&BoqRow> = boq.rows.iter().filter(|r| !r.reference_only).collect();
    let n = bids.len();

    let mut lines = Vec::with_capacity(rows.len());
    for r in &rows {
        let quantity = round(r.quantity, 3);
        let cells: Vec<BidCell> = bids
            .iter()
            .map(|b| {
                let rate = b.rates.get(&r.code).copied();
                BidCell { rate, amount: rate.map(|x| round(x * quantity, 2)) }
            })
            .collect();
        let rates: Vec<f64> = cells.iter().filter_map(|c| c.rate).collect();
        let low = rates.iter().copied().fold(f64::INFINITY, f64::min);
        let high = rates.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let lowest = if rates.is_empty() {
            vec![]
        } else {
            bids.iter().zip(&cells).filter(|(_, c)| c.rate == Some(low)).map(|(b, _)| b.contractor.trim().to_string()).collect()
        };
        lines.push(BidLineRow {
            no: r.no.clone(),
            code: r.code.clone(),
            description: r.description.clone(),
            unit: r.unit.clone(),
            quantity,
            lowest,
            spread: (rates.len() >= 2).then(|| round(high - low, 4)),
            own_rate: r.rate,
            cells,
        });
    }

    let common: Vec<usize> = (0..lines.len()).filter(|&i| lines[i].cells.iter().all(|c| c.rate.is_some())).collect();
    let priced_by_nobody: Vec<String> = lines.iter().filter(|l| l.cells.iter().all(|c| c.rate.is_none())).map(|l| l.code.clone()).collect();

    let mut totals: Vec<BidTotal> = Vec::with_capacity(n);
    for (bi, b) in bids.iter().enumerate() {
        let subtotal = round(lines.iter().filter_map(|l| l.cells[bi].amount).sum(), 2);
        let mut running = subtotal;
        let mut markups = Vec::new();
        for m in &b.markups {
            let amount = round(running * m.pct / 100.0, 2);
            running = round(running + amount, 2);
            markups.push(MarkupLine { name: m.name.trim().to_string(), pct: m.pct, amount });
        }
        let unpriced: Vec<String> = lines.iter().filter(|l| l.cells[bi].rate.is_none()).map(|l| l.code.clone()).collect();
        totals.push(BidTotal {
            contractor: b.contractor.trim().to_string(),
            source: b.source.trim().to_string(),
            date: b.date.clone(),
            priced_lines: lines.len() - unpriced.len(),
            complete: unpriced.is_empty(),
            unpriced,
            subtotal,
            markups,
            total: running,
            common_subtotal: round(common.iter().filter_map(|&i| lines[i].cells[bi].amount).sum(), 2),
            rank: None,
            above_lowest: None,
            above_lowest_pct: None,
        });
    }

    let mut order: Vec<usize> = (0..n).filter(|&i| totals[i].complete).collect();
    order.sort_by(|&a, &b| totals[a].total.total_cmp(&totals[b].total));
    let lowest_total = order.first().map(|&i| totals[i].total);
    let mut rank = 0usize;
    let mut prev: Option<f64> = None;
    for (pos, &i) in order.iter().enumerate() {
        if prev != Some(totals[i].total) {
            rank = pos + 1;
            prev = Some(totals[i].total);
        }
        totals[i].rank = Some(rank);
        if let Some(low) = lowest_total {
            let d = round(totals[i].total - low, 2);
            totals[i].above_lowest = Some(d);
            totals[i].above_lowest_pct = (low > 0.0).then(|| round(d / low * 100.0, 2));
        }
    }
    let lowest_complete = order.first().map(|&i| totals[i].contractor.clone());

    let mut notes = vec![
        "Quantities are the project's measured quantities; rates, mark-ups, dates and sources are the bidders' own as entered.".to_string(),
        "Arithmetic comparison only: the lowest total is not an award recommendation, and scope, exclusions and bidder qualifications are not compared.".to_string(),
    ];
    let incomplete: Vec<&str> = totals.iter().filter(|t| !t.complete).map(|t| t.contractor.as_str()).collect();
    if !incomplete.is_empty() {
        notes.push(format!("Not ranked (items left unpriced): {}. Compare them on the like-for-like subtotal only.", incomplete.join(", ")));
    }
    if lowest_complete.is_none() {
        notes.push("No bidder priced every measured item, so no total is ranked.".to_string());
    }
    if !priced_by_nobody.is_empty() {
        notes.push(format!("Items no bidder priced: {}.", priced_by_nobody.join(", ")));
    }

    Ok(BidComparison {
        currency: bids[0].currency.trim().to_string(),
        bidders: bids.iter().map(|b| b.contractor.trim().to_string()).collect(),
        common_lines: common.len(),
        lines,
        totals,
        priced_by_nobody,
        lowest_complete,
        notes,
    })
}

fn cell(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) || s.starts_with(['=', '+', '-', '@']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

impl BidComparison {
    /// The comparison sheet: one row per measured item with each bidder's rate and amount, then the totals.
    pub fn csv(&self) -> String {
        let mut o = String::from("\u{feff}no,code,description,unit,quantity,own_rate");
        for b in &self.bidders {
            o.push_str(&format!(",{},{}", cell(&format!("{b} rate")), cell(&format!("{b} amount"))));
        }
        o.push_str(",lowest\n");
        let opt = |v: Option<f64>| v.map(|x| format!("{x:.2}")).unwrap_or_default();
        for l in &self.lines {
            o.push_str(&format!("{},{},{},{},{},{}", cell(&l.no), cell(&l.code), cell(&l.description), cell(&l.unit), l.quantity, opt(l.own_rate)));
            for c in &l.cells {
                o.push_str(&format!(",{},{}", opt(c.rate), opt(c.amount)));
            }
            o.push_str(&format!(",{}\n", cell(&l.lowest.join(" / "))));
        }
        o.push_str("\ncontractor,source,date,priced_lines,unpriced,subtotal,total,common_subtotal,rank,above_lowest,above_lowest_pct,currency\n");
        for t in &self.totals {
            o.push_str(&format!(
                "{},{},{},{},{},{:.2},{:.2},{:.2},{},{},{},{}\n",
                cell(&t.contractor),
                cell(&t.source),
                t.date,
                t.priced_lines,
                cell(&t.unpriced.join(" ")),
                t.subtotal,
                t.total,
                t.common_subtotal,
                t.rank.map(|r| r.to_string()).unwrap_or_else(|| "not ranked".into()),
                opt(t.above_lowest),
                opt(t.above_lowest_pct),
                cell(&self.currency),
            ));
        }
        o
    }
}
