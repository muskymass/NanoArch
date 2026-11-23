//! Bar bending schedule (BBS): the bar rows of the structural sizing report, classified by bar shape, with bend counts, bar types
//! sharing one type mark, and a summary per diameter.
//!
//! Nothing here is a design value. Every length, count and diameter comes from the rows of [`SizingReport::bars`] (your bar templates
//! or the design of `rebar`); this module only classifies and totals them.
//!
//! - **Shape** is read from the role of the row: `straight` (column vertical bars, mesh, top and bottom bars) or `closed_link`
//!   (column ties and beam stirrups, a closed rectangle). No code shape number is attached, because the shape tables of a code are
//!   not in the references read for this project.
//! - **Bends** are only those the geometry fixes: a closed link has 4 corner bends; when the hook allowance `rebar.hook_mm` is above
//!   zero every row that carries it adds 2 hook bends (a link: its two ends; a straight bar: its two ends). The anchorage leg of a
//!   column bar into its footing has no entered shape (the template gives only its length), so it is not counted as a bend and the
//!   schedule says so.
//! - **Type marks** (`M1`, `M2`, ...) are given to each distinct shape, diameter and cut length, in order of first appearance, so
//!   identical bars of different elements are ordered together.
//! - Weights are the ones of the sizing report (steel density 7850 kg/m3 over the exact cut lengths); no waste or stock length
//!   is applied here: see `cutting` for the stock bars.
//!
//! Preliminary: it does not replace a licensed structural engineer.

use crate::sizing::{BarRow, SizingInputs, SizingReport};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Debug, Clone, Serialize)]
pub struct BbsLine {
    pub element: String,
    pub id: Option<String>,
    /// The mark of the sizing row, e.g. `COL-1/V`.
    pub mark: String,
    /// Type mark shared by every bar of the same shape, diameter and cut length.
    pub type_mark: String,
    pub role: String,
    /// `straight` or `closed_link`.
    pub shape: String,
    pub dia_mm: f64,
    pub number: u64,
    pub cut_length_mm: f64,
    pub total_m: f64,
    pub kg: f64,
    pub bends_each: u32,
    pub total_bends: u64,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BarType {
    pub type_mark: String,
    pub shape: String,
    pub dia_mm: f64,
    pub cut_length_mm: f64,
    pub number: u64,
    pub kg: f64,
    pub bends_each: u32,
    /// The sizing marks that use this type.
    pub used_in: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiameterRow {
    pub dia_mm: f64,
    /// Mass per metre of this diameter (pi/4 d^2 x 7850 kg/m3), kg/m.
    pub unit_kg_m: f64,
    pub number: u64,
    pub total_m: f64,
    pub kg: f64,
    pub total_bends: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Bbs {
    pub lines: Vec<BbsLine>,
    pub types: Vec<BarType>,
    pub by_diameter: Vec<DiameterRow>,
    pub total_kg: f64,
    pub total_bends: u64,
    pub notes: Vec<String>,
}

const STEEL_KG_M3: f64 = 7850.0;

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn shape_of(role: &str) -> Result<&'static str, String> {
    match role {
        "ties" | "stirrups" => Ok("closed_link"),
        "vertical bars" | "bottom mesh, along x" | "bottom mesh, along y" | "top bars" | "bottom bars" => Ok("straight"),
        other => Err(format!("bar role '{other}' has no known shape: it is not scheduled; add it to the shape table of `bbs` first")),
    }
}

/// Hook bends and corner bends per bar of this row. `vertical bars` carry the anchorage, not the hook allowance.
fn bends_of(b: &BarRow, shape: &str, hook_mm: f64) -> u32 {
    let hooks = if hook_mm > 0.0 && b.role != "vertical bars" { 2 } else { 0 };
    match shape {
        "closed_link" => 4 + hooks,
        _ => hooks,
    }
}

/// The bar bending schedule of a sizing report. Without reinforcement rows (no templates, no design) the schedule is empty and says
/// why; an unknown bar role is an error, never a guess.
pub fn build(rep: &SizingReport, inp: &SizingInputs) -> Result<Bbs, String> {
    let mut out = Bbs::default();
    if rep.bars.is_empty() {
        let why = rep.missing.get("reinforcement").map(|m| m.join(", ")).unwrap_or_else(|| "no reinforcement rows in the sizing report".into());
        out.notes.push(format!("empty schedule: {why}"));
        return Ok(out);
    }
    // The hook allowance is an entered input; without it the rows would not exist, but a report built elsewhere must not pass silently.
    let hook_mm = inp.rebar.as_ref().and_then(|r| r.hook_mm).filter(|v| v.is_finite() && *v >= 0.0).ok_or("rebar.hook_mm is missing: the bend count of the schedule needs it")?;

    let mut mark_of: BTreeMap<(String, u64, u64), String> = BTreeMap::new();
    let mut types: Vec<BarType> = Vec::new();
    for b in &rep.bars {
        let shape = shape_of(&b.role)?;
        let bends_each = bends_of(b, shape, hook_mm);
        let key = (shape.to_string(), (b.dia_mm * 1000.0).round() as u64, (b.cut_length_mm * 10.0).round() as u64);
        let type_mark = match mark_of.get(&key) {
            Some(m) => m.clone(),
            None => {
                let m = format!("M{}", types.len() + 1);
                mark_of.insert(key, m.clone());
                types.push(BarType { type_mark: m.clone(), shape: shape.into(), dia_mm: b.dia_mm, cut_length_mm: b.cut_length_mm, number: 0, kg: 0.0, bends_each, used_in: Vec::new() });
                m
            }
        };
        if let Some(t) = types.iter_mut().find(|t| t.type_mark == type_mark) {
            t.number += b.number;
            t.kg = round3(t.kg + b.kg);
            if !t.used_in.contains(&b.mark) {
                t.used_in.push(b.mark.clone());
            }
        }
        out.lines.push(BbsLine {
            element: b.element.clone(),
            id: b.id.clone(),
            mark: b.mark.clone(),
            type_mark,
            role: b.role.clone(),
            shape: shape.into(),
            dia_mm: b.dia_mm,
            number: b.number,
            cut_length_mm: b.cut_length_mm,
            total_m: b.total_m,
            kg: b.kg,
            bends_each,
            total_bends: b.number * bends_each as u64,
            source: b.source.clone(),
        });
    }
    out.types = types;

    let mut per: BTreeMap<u64, DiameterRow> = BTreeMap::new();
    for l in &out.lines {
        let d = per.entry((l.dia_mm * 1000.0).round() as u64).or_insert_with(|| DiameterRow { dia_mm: l.dia_mm, unit_kg_m: round3(std::f64::consts::PI / 4.0 * (l.dia_mm / 1000.0).powi(2) * STEEL_KG_M3), number: 0, total_m: 0.0, kg: 0.0, total_bends: 0 });
        d.number += l.number;
        d.total_m = round3(d.total_m + l.total_m);
        d.kg = round3(d.kg + l.kg);
        d.total_bends += l.total_bends;
    }
    out.by_diameter = per.into_values().collect();
    out.total_kg = round3(out.lines.iter().map(|l| l.kg).sum());
    out.total_bends = out.lines.iter().map(|l| l.total_bends).sum();

    if (out.total_kg - rep.steel_total_kg).abs() > 0.01 + rep.steel_total_kg * 1e-6 {
        return Err(format!("the schedule weighs {} kg but the sizing report holds {} kg: the rows are out of step", out.total_kg, rep.steel_total_kg));
    }
    if rep.bars.iter().any(|b| b.role == "vertical bars") {
        out.notes.push("column bar anchorage legs into the footing have no entered shape, so no bend is counted for them".into());
    }
    out.notes.push("lengths are cut lengths of your templates or of the `rebar` design; no waste, stock length or lap is applied (see `cutting`)".into());
    Ok(out)
}

fn csv_cell(s: &str) -> String {
    if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

impl Bbs {
    /// One row per bar line, with type mark, shape and bends, and a total row.
    pub fn lines_csv(&self) -> String {
        let mut o = String::from("element,element_id,mark,type_mark,role,shape,dia_mm,number,cut_length_mm,total_m,kg,bends_each,total_bends,source\n");
        for l in &self.lines {
            let _ = writeln!(o, "{},{},{},{},{},{},{},{},{},{},{},{},{},{}", l.element, l.id.clone().unwrap_or_default(), l.mark, l.type_mark, csv_cell(&l.role), l.shape, l.dia_mm, l.number, l.cut_length_mm, l.total_m, l.kg, l.bends_each, l.total_bends, l.source);
        }
        let _ = writeln!(o, "TOTAL,,,,,,,,,,{},,{},", self.total_kg, self.total_bends);
        o
    }

    /// One row per bar type (the ordering list for the bender) and one per diameter (the purchase summary).
    pub fn summary_csv(&self) -> String {
        let mut o = String::from("type_mark,shape,dia_mm,cut_length_mm,number,kg,bends_each,used_in\n");
        for t in &self.types {
            let _ = writeln!(o, "{},{},{},{},{},{},{},{}", t.type_mark, t.shape, t.dia_mm, t.cut_length_mm, t.number, t.kg, t.bends_each, csv_cell(&t.used_in.join(" ")));
        }
        o.push_str("\ndia_mm,unit_kg_m,number,total_m,kg,total_bends\n");
        for d in &self.by_diameter {
            let _ = writeln!(o, "{},{},{},{},{},{}", d.dia_mm, d.unit_kg_m, d.number, d.total_m, d.kg, d.total_bends);
        }
        let _ = writeln!(o, "TOTAL,,,,{},{}", self.total_kg, self.total_bends);
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sizing::{RebarIn, SizingReport};

    fn row(element: &str, mark: &str, role: &str, dia: f64, number: u64, cut: f64) -> BarRow {
        let total_m = number as f64 * cut / 1000.0;
        let kg = round3(std::f64::consts::PI / 4.0 * (dia / 1000.0).powi(2) * total_m * STEEL_KG_M3);
        BarRow { element: element.into(), id: Some("1".into()), mark: mark.into(), role: role.into(), dia_mm: dia, number, cut_length_mm: cut, total_m: round3(total_m), kg, source: "template".into() }
    }

    fn report(bars: Vec<BarRow>) -> SizingReport {
        let steel_total_kg = round3(bars.iter().map(|b| b.kg).sum());
        SizingReport { bars, steel_total_kg, ..Default::default() }
    }

    fn inputs(hook: Option<f64>) -> SizingInputs {
        SizingInputs { rebar: Some(RebarIn { hook_mm: hook, ..Default::default() }), ..Default::default() }
    }

    #[test]
    fn identical_bars_of_two_elements_share_a_type_mark_and_totals_agree() {
        let rep = report(vec![
            row("COL-1", "COL-1/V", "vertical bars", 16.0, 8, 3300.0),
            row("COL-2", "COL-2/V", "vertical bars", 16.0, 8, 3300.0),
            row("COL-1", "COL-1/T", "ties", 8.0, 16, 1080.0),
        ]);
        let b = build(&rep, &inputs(Some(100.0))).unwrap();
        assert_eq!(b.types.len(), 2);
        assert_eq!(b.lines[0].type_mark, b.lines[1].type_mark);
        assert_ne!(b.lines[0].type_mark, b.lines[2].type_mark);
        let t = &b.types[0];
        assert_eq!((t.number, t.used_in.clone()), (16, vec!["COL-1/V".to_string(), "COL-2/V".to_string()]));
        assert_eq!(b.by_diameter.len(), 2);
        assert!((b.total_kg - rep.steel_total_kg).abs() < 1e-9);
        assert!((b.by_diameter.iter().map(|d| d.kg).sum::<f64>() - b.total_kg).abs() < 0.01);
        assert!((b.by_diameter[0].unit_kg_m - 0.395).abs() < 0.001, "8 mm bar is {} kg/m", b.by_diameter[0].unit_kg_m);
    }

    #[test]
    fn bends_follow_the_shape_and_the_hook_allowance() {
        let rep = report(vec![row("COL-1", "COL-1/T", "ties", 8.0, 10, 1080.0), row("BM-1", "BM-1/B", "bottom bars", 16.0, 3, 4200.0), row("COL-1", "COL-1/V", "vertical bars", 16.0, 8, 3300.0)]);
        let with_hook = build(&rep, &inputs(Some(100.0))).unwrap();
        assert_eq!(with_hook.lines.iter().map(|l| l.bends_each).collect::<Vec<_>>(), vec![6, 2, 0]);
        assert_eq!(with_hook.total_bends, 60 + 6);
        let no_hook = build(&rep, &inputs(Some(0.0))).unwrap();
        assert_eq!(no_hook.lines.iter().map(|l| l.bends_each).collect::<Vec<_>>(), vec![4, 0, 0]);
        assert!(no_hook.notes.iter().any(|n| n.contains("anchorage")));
    }

    #[test]
    fn a_missing_hook_input_or_an_unknown_role_is_an_error_not_a_guess() {
        let rep = report(vec![row("COL-1", "COL-1/V", "vertical bars", 16.0, 8, 3300.0)]);
        assert!(build(&rep, &inputs(None)).unwrap_err().contains("rebar.hook_mm"));
        assert!(build(&rep, &SizingInputs::default()).unwrap_err().contains("rebar.hook_mm"));
        let odd = report(vec![row("X", "X/1", "spiral", 10.0, 1, 1000.0)]);
        assert!(build(&odd, &inputs(Some(0.0))).unwrap_err().contains("spiral"));
    }

    #[test]
    fn an_empty_report_gives_an_empty_schedule_with_the_reason() {
        let mut rep = SizingReport::default();
        rep.missing.insert("reinforcement".into(), vec!["rebar.anchorage_mm".into()]);
        let b = build(&rep, &SizingInputs::default()).unwrap();
        assert!(b.lines.is_empty() && b.notes[0].contains("rebar.anchorage_mm"));
    }

    #[test]
    fn a_report_whose_total_disagrees_with_its_rows_is_refused() {
        let mut rep = report(vec![row("COL-1", "COL-1/V", "vertical bars", 16.0, 8, 3300.0)]);
        rep.steel_total_kg += 5.0;
        assert!(build(&rep, &inputs(Some(0.0))).unwrap_err().contains("out of step"));
    }

    #[test]
    fn csv_has_a_total_row_and_quotes_commas() {
        let rep = report(vec![row("FND-1", "FND-1/X", "bottom mesh, along x", 12.0, 5, 2000.0)]);
        let b = build(&rep, &inputs(Some(50.0))).unwrap();
        let lines = b.lines_csv();
        assert!(lines.contains("\"bottom mesh, along x\"") && lines.lines().last().unwrap().starts_with("TOTAL"));
        let s = b.summary_csv();
        assert!(s.contains("M1,straight,12") && s.contains("unit_kg_m"));
    }
}
