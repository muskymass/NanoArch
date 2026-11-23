//! Preliminary energy-efficiency rating from stated annual end-use energy and a rating scale the user enters.
//!
//! No rating scheme is built in: the bands (label and upper limit in kWh/m2.year), the scheme's source and its date are the
//! caller's, as are the end uses, the conditioned area and any on-site generation. A missing value is refused by name.
//!
//! Method (arithmetic only):
//! - Delivered energy = sum of the end uses' kWh per year. Optional on-site generation (kWh per year, the caller's figure) is
//!   deducted and the result floored at zero; surplus is reported and not credited.
//! - Energy use intensity (EUI) = net kWh per year / conditioned area.
//! - The band is the first whose `up_to_kwh_m2` is at or above the EUI (bounds are cumulative and ascending; the last band has
//!   none and catches everything above). The report also gives the reduction in kWh per year needed to reach the next better band.
//! - An emissions figure is produced only when the caller supplies a factor with its source.
//!
//! It is an estimate from stated figures. It is not an official energy certificate, which only the competent authority or an
//! accredited assessor issues, and it does not model weather, occupancy or primary-energy weighting.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EndUse {
    #[serde(default)]
    pub name: String,
    /// Delivered energy per year, kWh.
    #[serde(default)]
    pub kwh_per_year: Option<f64>,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Band {
    #[serde(default)]
    pub label: String,
    /// Cumulative upper bound of the band, kWh/m2.year. Absent only on the last (worst) band.
    #[serde(default)]
    pub up_to_kwh_m2: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Input {
    #[serde(default)]
    pub end_uses: Vec<EndUse>,
    #[serde(default)]
    pub conditioned_area_m2: Option<f64>,
    #[serde(default)]
    pub area_source: Option<String>,
    /// Bands from best to worst.
    #[serde(default)]
    pub bands: Vec<Band>,
    /// Document or page the rating scale was read from.
    #[serde(default)]
    pub scheme_source: Option<String>,
    #[serde(default)]
    pub scheme_date: Option<String>,
    #[serde(default)]
    pub generation_kwh_per_year: Option<f64>,
    #[serde(default)]
    pub generation_source: Option<String>,
    /// kg CO2e per kWh. Optional; when given `emission_source` is required.
    #[serde(default)]
    pub emission_factor_kg_per_kwh: Option<f64>,
    #[serde(default)]
    pub emission_source: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UseRow {
    pub name: String,
    pub kwh_per_year: f64,
    pub share_pct: f64,
    pub kwh_per_m2: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct NextBand {
    pub label: String,
    pub max_kwh_m2: f64,
    pub reduction_kwh_per_year: f64,
    pub reduction_pct: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub end_uses: Vec<UseRow>,
    pub delivered_kwh_per_year: f64,
    pub generation_kwh_per_year: f64,
    pub net_kwh_per_year: f64,
    pub surplus_kwh_per_year: f64,
    pub conditioned_area_m2: f64,
    pub eui_kwh_m2: f64,
    pub band: String,
    /// Zero-based position in the entered scale, 0 being the best band.
    pub band_index: usize,
    pub band_count: usize,
    /// None when the result is already in the best band.
    pub next_better: Option<NextBand>,
    pub emissions_kg_per_year: Option<f64>,
    pub emissions_kg_per_m2: Option<f64>,
    pub scheme_source: String,
    pub scheme_date: String,
    pub notes: Vec<String>,
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn positive(v: Option<f64>, field: &str) -> Result<f64, String> {
    let v = v.ok_or_else(|| format!("{field} is required and has no default"))?;
    if v.is_finite() && v > 0.0 { Ok(v) } else { Err(format!("{field} must be a positive number, got {v}")) }
}

fn non_negative(v: f64, field: &str) -> Result<f64, String> {
    if v.is_finite() && v >= 0.0 { Ok(v) } else { Err(format!("{field} must be zero or more, got {v}")) }
}

fn text(v: &Option<String>, field: &str) -> Result<String, String> {
    match v.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => Ok(s.to_string()),
        _ => Err(format!("{field} is required and has no default")),
    }
}

/// Validated bands as `(label, upper bound or infinity)`, best first.
fn bands(list: &[Band]) -> Result<Vec<(String, f64)>, String> {
    if list.len() < 2 {
        return Err("bands is required: enter the rating scale from best to worst (at least two bands, the last with no up_to_kwh_m2)".into());
    }
    let last = list.len() - 1;
    let mut out = Vec::with_capacity(list.len());
    let mut prev = 0.0;
    for (i, b) in list.iter().enumerate() {
        let label = b.label.trim();
        if label.is_empty() {
            return Err(format!("bands[{i}].label is required and has no default"));
        }
        if out.iter().any(|(l, _): &(String, f64)| l == label) {
            return Err(format!("bands[{i}].label \"{label}\" is repeated"));
        }
        let bound = match (b.up_to_kwh_m2, i == last) {
            (None, true) => f64::INFINITY,
            (None, false) => return Err(format!("bands[{i}].up_to_kwh_m2 is required on every band except the last")),
            (Some(_), true) => return Err(format!("bands[{i}] is the last band and must have no up_to_kwh_m2 (it covers everything above)")),
            (Some(v), false) => {
                if !(v.is_finite() && v > prev) {
                    return Err(format!("bands[{i}].up_to_kwh_m2 must be above the previous bound {prev}, got {v}"));
                }
                prev = v;
                v
            }
        };
        out.push((label.to_string(), bound));
    }
    Ok(out)
}

pub fn estimate(input: &Input) -> Result<Report, String> {
    if input.end_uses.is_empty() {
        return Err("end_uses is required: list each end use (heating/cooling, lighting, hot water, ...) with kwh_per_year and source".into());
    }
    let scheme_source = text(&input.scheme_source, "scheme_source")?;
    let scheme_date = text(&input.scheme_date, "scheme_date")?;
    let scale = bands(&input.bands)?;
    let area = positive(input.conditioned_area_m2, "conditioned_area_m2")?;
    text(&input.area_source, "area_source")?;

    let mut rows = Vec::with_capacity(input.end_uses.len());
    let mut delivered = 0.0;
    for (i, u) in input.end_uses.iter().enumerate() {
        let name = if u.name.trim().is_empty() { format!("end use {}", i + 1) } else { u.name.trim().to_string() };
        let at = |f: &str| format!("end_uses[{i}] ({name}).{f}");
        let kwh = non_negative(positive_or_zero(u.kwh_per_year, &at("kwh_per_year"))?, &at("kwh_per_year"))?;
        text(&u.source, &at("source"))?;
        delivered += kwh;
        rows.push((name, kwh));
    }

    let generation = match input.generation_kwh_per_year {
        None => 0.0,
        Some(g) => {
            text(&input.generation_source, "generation_source")?;
            non_negative(g, "generation_kwh_per_year")?
        }
    };
    let net = (delivered - generation).max(0.0);
    let surplus = (generation - delivered).max(0.0);
    let eui = net / area;

    let band_index = scale.iter().position(|(_, bound)| eui <= *bound).unwrap_or(scale.len() - 1);
    let next_better = if band_index == 0 {
        None
    } else {
        let (label, bound) = &scale[band_index - 1];
        let reduction = ((eui - bound) * area).max(0.0);
        Some(NextBand {
            label: label.clone(),
            max_kwh_m2: *bound,
            reduction_kwh_per_year: round2(reduction),
            reduction_pct: if net > 0.0 { round2(reduction / net * 100.0) } else { 0.0 },
        })
    };

    let (emissions, emissions_m2) = match input.emission_factor_kg_per_kwh {
        None => (None, None),
        Some(f) => {
            text(&input.emission_source, "emission_source")?;
            let f = non_negative(f, "emission_factor_kg_per_kwh")?;
            (Some(round2(net * f)), Some(round2(net * f / area)))
        }
    };

    let end_uses = rows
        .into_iter()
        .map(|(name, kwh)| UseRow {
            name,
            kwh_per_year: round2(kwh),
            share_pct: if delivered > 0.0 { round2(kwh / delivered * 100.0) } else { 0.0 },
            kwh_per_m2: round2(kwh / area),
        })
        .collect();

    let mut notes = vec![
        "Preliminary rating from the figures and the rating scale you entered. It is not an official energy certificate; only the competent authority or an accredited assessor issues one.".to_string(),
        "No weather, occupancy or primary-energy weighting is modelled: the intensity is stated end-use energy divided by conditioned area.".to_string(),
    ];
    if input.generation_kwh_per_year.is_some() {
        notes.push("On-site generation is deducted from delivered energy and floored at zero; surplus is not credited.".to_string());
    }
    if eui > 0.0 && band_index == scale.len() - 1 {
        notes.push("The result falls in the last band of the scale you entered.".to_string());
    }
    Ok(Report {
        end_uses,
        delivered_kwh_per_year: round2(delivered),
        generation_kwh_per_year: round2(generation),
        net_kwh_per_year: round2(net),
        surplus_kwh_per_year: round2(surplus),
        conditioned_area_m2: area,
        eui_kwh_m2: round2(eui),
        band: scale[band_index].0.clone(),
        band_index,
        band_count: scale.len(),
        next_better,
        emissions_kg_per_year: emissions,
        emissions_kg_per_m2: emissions_m2,
        scheme_source,
        scheme_date,
        notes,
    })
}

fn positive_or_zero(v: Option<f64>, field: &str) -> Result<f64, String> {
    v.ok_or_else(|| format!("{field} is required and has no default (state 0 if there is none)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band(label: &str, up: Option<f64>) -> Band {
        Band { label: label.into(), up_to_kwh_m2: up }
    }

    fn input(kwh: f64) -> Input {
        Input {
            end_uses: vec![
                EndUse { name: "cooling".into(), kwh_per_year: Some(kwh * 0.75), source: Some("mepx".into()) },
                EndUse { name: "lighting".into(), kwh_per_year: Some(kwh * 0.25), source: Some("lighting plan".into()) },
            ],
            conditioned_area_m2: Some(100.0),
            area_source: Some("model".into()),
            bands: vec![band("A", Some(50.0)), band("B", Some(100.0)), band("C", None)],
            scheme_source: Some("test scheme".into()),
            scheme_date: Some("2026-01-01".into()),
            ..Input::default()
        }
    }

    #[test]
    fn band_and_next_band_gap_match_hand_arithmetic() {
        // 8000 kWh / 100 m2 = 80 kWh/m2 -> band B; band A needs <= 50 -> 30 kWh/m2 x 100 = 3000 kWh (37.5 %).
        let r = estimate(&input(8000.0)).unwrap();
        assert_eq!(r.eui_kwh_m2, 80.0);
        assert_eq!(r.band, "B");
        assert_eq!(r.band_index, 1);
        let n = r.next_better.unwrap();
        assert_eq!(n.label, "A");
        assert_eq!(n.reduction_kwh_per_year, 3000.0);
        assert_eq!(n.reduction_pct, 37.5);
        assert_eq!(r.end_uses[0].share_pct, 75.0);
        assert_eq!(r.end_uses[0].kwh_per_m2, 60.0);
    }

    #[test]
    fn bounds_are_inclusive_best_has_no_next_and_worst_catches_all() {
        let r = estimate(&input(5000.0)).unwrap();
        assert_eq!(r.band, "A");
        assert!(r.next_better.is_none());
        let r = estimate(&input(10000.0)).unwrap();
        assert_eq!(r.band, "B");
        let r = estimate(&input(30000.0)).unwrap();
        assert_eq!(r.band, "C");
        assert!(r.notes.iter().any(|n| n.contains("last band")));
    }

    #[test]
    fn generation_offsets_and_emissions_need_a_sourced_factor() {
        let mut i = input(8000.0);
        i.generation_kwh_per_year = Some(3000.0);
        i.generation_source = Some("pv sizing".into());
        i.emission_factor_kg_per_kwh = Some(0.5);
        i.emission_source = Some("test factor".into());
        let r = estimate(&i).unwrap();
        assert_eq!(r.net_kwh_per_year, 5000.0);
        assert_eq!(r.band, "A");
        assert_eq!(r.emissions_kg_per_year, Some(2500.0));
        assert_eq!(r.emissions_kg_per_m2, Some(25.0));
        i.generation_kwh_per_year = Some(9000.0);
        let r = estimate(&i).unwrap();
        assert_eq!(r.net_kwh_per_year, 0.0);
        assert_eq!(r.surplus_kwh_per_year, 1000.0);
        let mut no_src = i.clone();
        no_src.emission_source = None;
        assert!(estimate(&no_src).unwrap_err().contains("emission_source is required"));
        let mut no_gen = i.clone();
        no_gen.generation_source = None;
        assert!(estimate(&no_gen).unwrap_err().contains("generation_source is required"));
        assert!(estimate(&input(8000.0)).unwrap().emissions_kg_per_year.is_none());
    }

    #[test]
    fn missing_values_are_named_and_nothing_is_defaulted() {
        let ok = input(8000.0);
        let mut i = ok.clone();
        i.scheme_source = None;
        assert!(estimate(&i).unwrap_err().contains("scheme_source is required"));
        let mut i = ok.clone();
        i.scheme_date = Some(" ".into());
        assert!(estimate(&i).unwrap_err().contains("scheme_date is required"));
        let mut i = ok.clone();
        i.conditioned_area_m2 = None;
        assert!(estimate(&i).unwrap_err().contains("conditioned_area_m2 is required"));
        let mut i = ok.clone();
        i.area_source = None;
        assert!(estimate(&i).unwrap_err().contains("area_source is required"));
        let mut i = ok.clone();
        i.bands.clear();
        assert!(estimate(&i).unwrap_err().contains("bands is required"));
        let mut i = ok.clone();
        i.end_uses[0].kwh_per_year = None;
        assert!(estimate(&i).unwrap_err().contains("kwh_per_year is required"));
        let mut i = ok.clone();
        i.end_uses[1].source = None;
        assert!(estimate(&i).unwrap_err().contains("source is required"));
        assert!(estimate(&Input::default()).unwrap_err().contains("end_uses is required"));
    }

    #[test]
    fn malformed_values_are_refused() {
        let ok = input(8000.0);
        let mut i = ok.clone();
        i.conditioned_area_m2 = Some(0.0);
        assert!(estimate(&i).unwrap_err().contains("positive"));
        let mut i = ok.clone();
        i.end_uses[0].kwh_per_year = Some(-1.0);
        assert!(estimate(&i).unwrap_err().contains("zero or more"));
        let mut i = ok.clone();
        i.bands[2].up_to_kwh_m2 = Some(200.0);
        assert!(estimate(&i).unwrap_err().contains("last band"));
        let mut i = ok.clone();
        i.bands[0].up_to_kwh_m2 = None;
        assert!(estimate(&i).unwrap_err().contains("every band except the last"));
        let mut i = ok.clone();
        i.bands[1].up_to_kwh_m2 = Some(40.0);
        assert!(estimate(&i).unwrap_err().contains("above the previous bound"));
        let mut i = ok.clone();
        i.bands[1].label = "A".into();
        assert!(estimate(&i).unwrap_err().contains("repeated"));
        let mut i = ok.clone();
        i.bands[0].label = " ".into();
        assert!(estimate(&i).unwrap_err().contains("label is required"));
    }
}
