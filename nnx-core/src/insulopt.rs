//! Insulation options compared by cost and yearly saving.
//!
//! The user's insulation library holds the project's base assembly for a scope (`wall` or `roof`). Each option is a candidate
//! insulation material (conductivity, price) with one or more thicknesses to try. An option REPLACES every layer of kind
//! `insulation` in the base assembly with its own layer (or is added when the base has none); the other layers, Rsi and Rse are
//! the base assembly's. The base assembly as it stands is the baseline.
//!
//! Every number that decides a result is the caller's and has no default: the cooling degree-hours per year (with its source),
//! the cooling system's seasonal COP, the electricity tariff, the study period in years, and per option the lambda, the price
//! per m3 of material and the installation cost per m2 (zero must be stated). A missing value is refused by name.
//!
//! Method (physics and arithmetic only):
//! - `U = 1 / (Rsi + sum(t / lambda) + Rse)` for the baseline and for each option and thickness.
//! - Yearly cooling energy saved (kWh) = `(U_base - U_option) x area x degree_hours / 1000 / COP`, where `degree_hours` is
//!   K.h per year above the indoor set point. Yearly saving = that energy x tariff.
//! - Cost of an option = `area x (thickness x price_per_m3 + install_per_m2)`, less `baseline_insulation_cost_per_m2 x area`
//!   when the caller states what the replaced insulation already cost (otherwise nothing is credited and the note says so).
//! - Simple payback = cost / yearly saving (none when the saving is not positive). Net over the study = saving x years - cost,
//!   undiscounted, with no tariff escalation, price change or maintenance.
//! - Against the scope's U limit (when given) each row is `ok` or `exceeds limit`; without a limit it is `unchecked`.
//!
//! The report names the cheapest row that meets the limit and the row with the best net; it never picks one for the user. Only
//! heat gain through the envelope in cooling mode is modelled (no heating, no solar gain, no thermal mass, no infiltration).
//! It is a preliminary comparison and does not replace an energy simulation or a licensed engineer.

use crate::insulation::{Assembly, InsulationLib, Layer};
use crate::model::Model;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Option_ {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub lambda: Option<f64>,
    #[serde(default)]
    pub thicknesses_mm: Vec<f64>,
    /// Price of the insulation material per m3.
    #[serde(default)]
    pub price_per_m3: Option<f64>,
    /// Fixing, labour and accessories per m2 of treated surface (zero is allowed but must be stated).
    #[serde(default)]
    pub install_per_m2: Option<f64>,
    /// Where lambda and the prices came from.
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Input {
    /// `wall` or `roof`.
    #[serde(default)]
    pub scope: Option<String>,
    /// Cooling degree-hours per year, K.h.
    #[serde(default)]
    pub degree_hours: Option<f64>,
    #[serde(default)]
    pub degree_hours_source: Option<String>,
    /// Seasonal COP of the cooling system.
    #[serde(default)]
    pub cop: Option<f64>,
    /// Electricity tariff per kWh.
    #[serde(default)]
    pub tariff_per_kwh: Option<f64>,
    #[serde(default)]
    pub study_years: Option<f64>,
    /// What the base assembly's insulation already costs per m2; credited against each option when given.
    #[serde(default)]
    pub baseline_insulation_cost_per_m2: Option<f64>,
    #[serde(default)]
    pub options: Vec<Option_>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub option: String,
    pub thickness_mm: f64,
    pub u_value: f64,
    /// `ok`, `exceeds limit` or `unchecked`.
    pub limit_status: &'static str,
    pub cost: f64,
    pub kwh_saved_per_year: f64,
    pub saving_per_year: f64,
    pub payback_years: Option<f64>,
    pub net_over_study: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub scope: String,
    pub area_m2: f64,
    pub base_u: f64,
    pub u_limit: Option<f64>,
    pub base_limit_status: &'static str,
    pub rows: Vec<Row>,
    /// Index into `rows` of the cheapest row meeting the limit, when a limit was given and one meets it.
    pub cheapest_compliant: Option<usize>,
    /// Index into `rows` of the highest net over the study period.
    pub best_net: Option<usize>,
    pub notes: Vec<String>,
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn positive(v: Option<f64>, field: &str) -> Result<f64, String> {
    let v = v.ok_or_else(|| format!("{field} is required and has no default"))?;
    if v.is_finite() && v > 0.0 { Ok(v) } else { Err(format!("{field} must be a positive number, got {v}")) }
}

fn non_negative(v: Option<f64>, field: &str) -> Result<f64, String> {
    let v = v.ok_or_else(|| format!("{field} is required and has no default (state 0 if there is none)"))?;
    if v.is_finite() && v >= 0.0 { Ok(v) } else { Err(format!("{field} must be zero or more, got {v}")) }
}

fn status(u: f64, limit: Option<f64>) -> &'static str {
    match limit {
        None => "unchecked",
        Some(l) if u <= l + 1e-9 => "ok",
        Some(_) => "exceeds limit",
    }
}

/// The comparison for a scope's area, from the library's base assembly.
pub fn compare_for_area(area_m2: f64, lib: &InsulationLib, inp: &Input) -> Result<Report, String> {
    let scope = inp.scope.as_deref().ok_or("scope is required (wall or roof)")?;
    if scope != "wall" && scope != "roof" {
        return Err(format!("scope must be 'wall' or 'roof', got '{scope}'"));
    }
    let base = lib.assemblies.get(scope).ok_or_else(|| format!("the insulation library has no '{scope}' assembly to use as the baseline"))?;
    let base_u = base.u_value().map_err(|m| format!("the '{scope}' base assembly is incomplete: missing {}", m.join(", ")))?;
    if !area_m2.is_finite() || area_m2 <= 0.0 {
        return Err(format!("the model has no {scope} area to insulate"));
    }
    let dh = positive(inp.degree_hours, "degree_hours")?;
    if inp.degree_hours_source.as_deref().is_none_or(|s| s.trim().is_empty()) {
        return Err("degree_hours_source is required: say where the degree-hours came from".into());
    }
    let cop = positive(inp.cop, "cop")?;
    let tariff = positive(inp.tariff_per_kwh, "tariff_per_kwh")?;
    let years = positive(inp.study_years, "study_years")?;
    let credit = match inp.baseline_insulation_cost_per_m2 {
        Some(c) => Some(non_negative(Some(c), "baseline_insulation_cost_per_m2")?),
        None => None,
    };
    if inp.options.is_empty() {
        return Err("options are required: give at least one insulation option".into());
    }
    let limit = lib.u_limits.get(scope).copied();
    let mut rows = Vec::new();
    for (i, o) in inp.options.iter().enumerate() {
        let name = if o.name.trim().is_empty() { return Err(format!("options[{i}].name is required")) } else { o.name.trim() };
        let lambda = positive(o.lambda, &format!("option '{name}': lambda"))?;
        let price = non_negative(o.price_per_m3, &format!("option '{name}': price_per_m3"))?;
        let install = non_negative(o.install_per_m2, &format!("option '{name}': install_per_m2"))?;
        if o.source.as_deref().is_none_or(|s| s.trim().is_empty()) {
            return Err(format!("option '{name}': source is required: say where lambda and the prices came from"));
        }
        if o.thicknesses_mm.is_empty() {
            return Err(format!("option '{name}': thicknesses_mm is required: give at least one thickness"));
        }
        for &t in &o.thicknesses_mm {
            positive(Some(t), &format!("option '{name}': thickness"))?;
            let mut a: Assembly = base.clone();
            a.layers.retain(|l| l.kind != "insulation");
            a.layers.push(Layer { material: name.to_string(), thickness_mm: t, lambda: Some(lambda), kind: "insulation".into() });
            let u = a.u_value().map_err(|m| format!("option '{name}': {}", m.join(", ")))?;
            let kwh = (base_u - u) * area_m2 * dh / 1000.0 / cop;
            let saving = kwh * tariff;
            let cost = area_m2 * (t / 1000.0 * price + install) - credit.unwrap_or(0.0) * area_m2;
            rows.push(Row {
                option: name.to_string(),
                thickness_mm: t,
                u_value: (u * 1e4).round() / 1e4,
                limit_status: status(u, limit),
                cost: round2(cost),
                kwh_saved_per_year: round2(kwh),
                saving_per_year: round2(saving),
                payback_years: (saving > 0.0 && cost > 0.0).then(|| round2(cost / saving)),
                net_over_study: round2(saving * years - cost),
            });
        }
    }
    let cheapest_compliant = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.limit_status == "ok")
        .min_by(|a, b| a.1.cost.total_cmp(&b.1.cost))
        .map(|(i, _)| i);
    let best_net = rows.iter().enumerate().max_by(|a, b| a.1.net_over_study.total_cmp(&b.1.net_over_study)).map(|(i, _)| i);
    let mut notes = vec![
        "Cooling-season heat gain through this scope only: no heating, solar gain, thermal mass or infiltration; undiscounted, no tariff escalation or maintenance.".to_string(),
        "Each option replaces the base assembly's insulation layers; the other layers, Rsi and Rse are the base assembly's.".to_string(),
    ];
    if credit.is_none() {
        notes.push("No baseline_insulation_cost_per_m2 was given: the cost of the insulation being replaced is not credited, so costs are gross.".into());
    }
    if limit.is_none() {
        notes.push(format!("No U limit is recorded for '{scope}': limit_status is unchecked and no compliant option can be named."));
    }
    if rows.iter().any(|r| r.saving_per_year <= 0.0) {
        notes.push("Some rows save nothing against the base assembly (their U is not lower): a negative or zero saving is reported as it is.".into());
    }
    Ok(Report {
        scope: scope.to_string(),
        area_m2: round2(area_m2),
        base_u: (base_u * 1e4).round() / 1e4,
        u_limit: limit,
        base_limit_status: status(base_u, limit),
        rows,
        cheapest_compliant,
        best_net,
        notes,
    })
}

/// The comparison for the model's wall or roof area.
pub fn compare(model: &Model, lib: &InsulationLib, inp: &Input) -> Result<Report, String> {
    let scope = inp.scope.as_deref().ok_or("scope is required (wall or roof)")?;
    let (area, _) = crate::insulation::scope_area(model, scope, lib.wet_upturn_mm);
    compare_for_area(area, lib, inp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib(limit: Option<f64>) -> InsulationLib {
        let mut l = InsulationLib::default();
        l.assemblies.insert(
            "wall".into(),
            Assembly {
                rsi: Some(0.13),
                rse: Some(0.04),
                layers: vec![
                    Layer { material: "block".into(), thickness_mm: 200.0, lambda: Some(1.0), kind: String::new() },
                    Layer { material: "old".into(), thickness_mm: 25.0, lambda: Some(0.05), kind: "insulation".into() },
                ],
            },
        );
        if let Some(v) = limit {
            l.u_limits.insert("wall".into(), v);
        }
        l
    }

    fn input() -> Input {
        Input {
            scope: Some("wall".into()),
            degree_hours: Some(50_000.0),
            degree_hours_source: Some("test data".into()),
            cop: Some(3.0),
            tariff_per_kwh: Some(0.2),
            study_years: Some(10.0),
            baseline_insulation_cost_per_m2: None,
            options: vec![Option_ {
                name: "eps".into(),
                lambda: Some(0.04),
                thicknesses_mm: vec![50.0, 100.0],
                price_per_m3: Some(300.0),
                install_per_m2: Some(10.0),
                source: Some("test data".into()),
            }],
        }
    }

    #[test]
    fn rows_match_hand_calculation() {
        // base R = 0.13 + 0.04 + 0.2 + 0.5 = 0.87 -> U 1.14943; eps 50: R = 0.17 + 0.2 + 1.25 = 1.62 -> U 0.61728
        let r = compare_for_area(100.0, &lib(Some(0.7)), &input()).unwrap();
        assert!((r.base_u - 1.1494).abs() < 1e-4);
        assert_eq!(r.base_limit_status, "exceeds limit");
        let a = &r.rows[0];
        assert!((a.u_value - 0.6173).abs() < 1e-4);
        assert_eq!(a.limit_status, "ok");
        // cost = 100 x (0.05 x 300 + 10) = 2500
        assert_eq!(a.cost, 2500.0);
        // kWh = (1.149425 - 0.617284) x 100 x 50000 / 1000 / 3 = 886.9
        assert!((a.kwh_saved_per_year - 886.9).abs() < 0.1, "{}", a.kwh_saved_per_year);
        assert!((a.saving_per_year - 177.38).abs() < 0.1);
        assert!((a.payback_years.unwrap() - a.cost / a.saving_per_year).abs() < 0.02);
        assert!((a.net_over_study - (a.saving_per_year * 10.0 - 2500.0)).abs() < 0.02);
        assert_eq!(r.cheapest_compliant, Some(0));
    }

    #[test]
    fn baseline_credit_lowers_cost_and_no_limit_names_no_compliant_row() {
        let mut i = input();
        i.baseline_insulation_cost_per_m2 = Some(5.0);
        let r = compare_for_area(100.0, &lib(None), &i).unwrap();
        assert_eq!(r.rows[0].cost, 2000.0);
        assert_eq!(r.rows[0].limit_status, "unchecked");
        assert_eq!(r.cheapest_compliant, None);
        assert!(r.notes.iter().any(|n| n.contains("No U limit")));
        assert!(r.best_net.is_some());
    }

    #[test]
    fn a_worse_option_reports_negative_saving_and_no_payback() {
        let mut i = input();
        i.options[0].thicknesses_mm = vec![5.0];
        let r = compare_for_area(100.0, &lib(Some(0.7)), &i).unwrap();
        assert!(r.rows[0].saving_per_year < 0.0);
        assert_eq!(r.rows[0].payback_years, None);
        assert_eq!(r.rows[0].limit_status, "exceeds limit");
        assert_eq!(r.cheapest_compliant, None);
    }

    #[test]
    fn missing_values_are_refused_by_name() {
        let l = lib(None);
        type Edit = Box<dyn Fn(&mut Input)>;
        let cases: [(&str, Edit); 8] = [
            ("scope is required", Box::new(|i| i.scope = None)),
            ("degree_hours is required", Box::new(|i| i.degree_hours = None)),
            ("degree_hours_source is required", Box::new(|i| i.degree_hours_source = None)),
            ("cop is required", Box::new(|i| i.cop = None)),
            ("tariff_per_kwh is required", Box::new(|i| i.tariff_per_kwh = None)),
            ("study_years is required", Box::new(|i| i.study_years = None)),
            ("lambda is required", Box::new(|i| i.options[0].lambda = None)),
            ("install_per_m2 is required", Box::new(|i| i.options[0].install_per_m2 = None)),
        ];
        for (needle, f) in cases {
            let mut i = input();
            f(&mut i);
            let e = compare_for_area(100.0, &l, &i).unwrap_err();
            assert!(e.contains(needle), "{needle}: {e}");
        }
        let mut i = input();
        i.options[0].source = None;
        assert!(compare_for_area(100.0, &l, &i).unwrap_err().contains("source is required"));
        let mut i = input();
        i.options.clear();
        assert!(compare_for_area(100.0, &l, &i).unwrap_err().contains("options are required"));
        assert!(compare_for_area(0.0, &l, &input()).unwrap_err().contains("no wall area"));
        let mut i = input();
        i.scope = Some("floor".into());
        assert!(compare_for_area(100.0, &l, &i).unwrap_err().contains("scope must be"));
        assert!(compare_for_area(100.0, &InsulationLib::default(), &input()).unwrap_err().contains("no 'wall' assembly"));
    }
}
