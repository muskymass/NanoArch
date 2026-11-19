//! Cutting-stock optimizer for reinforcing bars: nests every bar length of the bar bending schedule into stock bars (12 000 mm unless
//! you enter another length) to give the exact number of stock bars and the offcut waste.
//!
//! - Per bar diameter, the pieces are packed with best-fit decreasing and first-fit decreasing; the better packing is kept. The lower
//!   bound `ceil(total length / stock)` is reported next to it, so the gap to the optimum is visible. It is a heuristic: when the
//!   result equals the lower bound it is optimal.
//! - A bar longer than the stock is spliced: `n = ceil((L - lap) / (stock - lap))` pieces, each splice adding one lap. Without a lap
//!   length those bars are left out and named.
//! - Waste is the stock length bought minus the length cut, as a percentage of the stock bought. No offcut is reused.

use crate::sizing::BarRow;
use crate::tanks::TankBar;
use serde::Serialize;
use std::collections::BTreeMap;
use std::f64::consts::PI;

const STEEL_KG_M3: f64 = 7850.0;

#[derive(Debug, Clone)]
pub struct Piece {
    pub dia_mm: f64,
    pub length_mm: f64,
    pub number: u64,
    /// Where the bar comes from (element and mark).
    pub tag: String,
}

pub fn pieces_from_schedule(bars: &[BarRow]) -> Vec<Piece> {
    bars.iter().map(|b| Piece { dia_mm: b.dia_mm, length_mm: b.cut_length_mm, number: b.number, tag: format!("{} {}", b.element, b.mark) }).collect()
}

pub fn pieces_from_tank(name: &str, bars: &[TankBar]) -> Vec<Piece> {
    bars.iter().map(|b| Piece { dia_mm: b.dia_mm, length_mm: b.cut_length_mm, number: b.number, tag: format!("{name} {}", b.mark) }).collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct StockBar {
    /// The cuts of this stock bar, mm, longest first.
    pub cuts: Vec<f64>,
    pub tags: Vec<String>,
    pub offcut_mm: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiaPlan {
    pub dia_mm: f64,
    pub pieces: u64,
    pub stock_bars: usize,
    pub lower_bound: usize,
    pub cut_m: f64,
    pub stock_m: f64,
    pub waste_m: f64,
    pub waste_pct: f64,
    pub stock_kg: f64,
    pub cut_kg: f64,
    pub splices: u64,
    pub patterns: Vec<StockBar>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CuttingReport {
    pub stock_mm: f64,
    pub per_dia: Vec<DiaPlan>,
    pub stock_bars: usize,
    pub waste_pct: f64,
    pub stock_kg: f64,
    pub cut_kg: f64,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
}

fn kg(dia: f64, m: f64) -> f64 {
    PI / 4.0 * (dia / 1000.0).powi(2) * m * STEEL_KG_M3
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Packs `items` (length, tag) into bars of `stock`: `best` chooses the fullest bar that fits (best fit), otherwise the first.
fn pack(items: &[(f64, String)], stock: f64, best: bool) -> Vec<StockBar> {
    let mut bars: Vec<(f64, Vec<f64>, Vec<String>)> = Vec::new(); // remaining, cuts, tags
    for (len, tag) in items {
        let slot = if best { bars.iter().enumerate().filter(|(_, b)| b.0 + 1e-9 >= *len).min_by(|a, b| a.1.0.total_cmp(&b.1.0)).map(|(i, _)| i) } else { bars.iter().position(|b| b.0 + 1e-9 >= *len) };
        match slot {
            Some(i) => {
                bars[i].0 -= len;
                bars[i].1.push(*len);
                bars[i].2.push(tag.clone());
            }
            None => bars.push((stock - len, vec![*len], vec![tag.clone()])),
        }
    }
    bars.into_iter().map(|(rem, cuts, tags)| StockBar { cuts, tags, offcut_mm: (rem * 10.0).round() / 10.0 }).collect()
}

pub fn optimize(pieces: &[Piece], stock_mm: Option<f64>, lap_mm: Option<f64>) -> CuttingReport {
    let mut rep = CuttingReport::default();
    let Some(stock) = stock_mm.filter(|s| *s > 0.0) else {
        rep.missing.push("stock bar length (mm)".into());
        return rep;
    };
    rep.stock_mm = stock;
    let mut by_dia: BTreeMap<u64, Vec<(f64, String)>> = BTreeMap::new();
    let mut splices: BTreeMap<u64, u64> = BTreeMap::new();
    let mut skipped_long = false;
    for p in pieces.iter().filter(|p| p.number > 0 && p.length_mm > 0.0 && p.dia_mm > 0.0) {
        let key = (p.dia_mm * 100.0).round() as u64;
        if p.length_mm <= stock {
            by_dia.entry(key).or_default().extend((0..p.number).map(|_| (p.length_mm, p.tag.clone())));
            continue;
        }
        let Some(lap) = lap_mm.filter(|l| *l >= 0.0 && *l < stock) else {
            skipped_long = true;
            continue;
        };
        let n = ((p.length_mm - lap) / (stock - lap)).ceil().max(2.0) as u64;
        let total = p.length_mm + (n - 1) as f64 * lap;
        let last = total - (n - 1) as f64 * stock;
        for _ in 0..p.number {
            for _ in 0..n - 1 {
                by_dia.entry(key).or_default().push((stock, format!("{} (spliced)", p.tag)));
            }
            by_dia.entry(key).or_default().push((last, format!("{} (spliced)", p.tag)));
        }
        *splices.entry(key).or_default() += (n - 1) * p.number;
    }
    if skipped_long {
        rep.missing.push("lap length (mm): some bars are longer than the stock bar and were left out".into());
    }
    for (key, mut items) in by_dia {
        let dia = key as f64 / 100.0;
        items.sort_by(|a, b| b.0.total_cmp(&a.0));
        let (a, b) = (pack(&items, stock, true), pack(&items, stock, false));
        let patterns = if a.len() <= b.len() { a } else { b };
        let cut_mm: f64 = items.iter().map(|i| i.0).sum();
        let stock_total = patterns.len() as f64 * stock;
        let sp = splices.get(&key).copied().unwrap_or(0);
        rep.per_dia.push(DiaPlan {
            dia_mm: dia,
            pieces: items.len() as u64,
            stock_bars: patterns.len(),
            lower_bound: (cut_mm / stock).ceil() as usize,
            cut_m: r2(cut_mm / 1000.0),
            stock_m: r2(stock_total / 1000.0),
            waste_m: r2((stock_total - cut_mm) / 1000.0),
            waste_pct: r2((stock_total - cut_mm) / stock_total * 100.0),
            stock_kg: r2(kg(dia, stock_total / 1000.0)),
            cut_kg: r2(kg(dia, cut_mm / 1000.0)),
            splices: sp,
            patterns,
        });
    }
    rep.stock_bars = rep.per_dia.iter().map(|d| d.stock_bars).sum();
    let (stock_total, cut_total): (f64, f64) = rep.per_dia.iter().fold((0.0, 0.0), |a, d| (a.0 + d.stock_m, a.1 + d.cut_m));
    rep.waste_pct = if stock_total > 0.0 { r2((stock_total - cut_total) / stock_total * 100.0) } else { 0.0 };
    rep.stock_kg = r2(rep.per_dia.iter().map(|d| d.stock_kg).sum());
    rep.cut_kg = r2(rep.per_dia.iter().map(|d| d.cut_kg).sum());
    if rep.per_dia.iter().any(|d| d.stock_bars > d.lower_bound) {
        rep.notes.push("a heuristic packing: where the bars equal the lower bound the packing is optimal, elsewhere it may be one bar above the optimum".into());
    }
    rep.notes.push("offcuts are not reused between diameters or schedules".into());
    rep
}
