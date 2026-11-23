//! Water storage (ground and roof tanks) and pump duty.
//!
//! Every value that decides a result is the caller's: the number of people, the litres per person per day (with its source),
//! the days of demand each tank must hold, the usable share of a tank, the refill time of the roof tank, the elevations of
//! the water levels, the transfer main (length, diameter, Hazen-Williams C, sum of minor-loss coefficients) and the pump and
//! motor efficiencies. Nothing here was read from a plumbing code or a pump catalogue; a missing value is refused by name.
//! Optional limits (motor sizes on offer, pipe velocity, tank water depth) are the caller's: a missing limit leaves that
//! check `unchecked`, never `ok`.
//!
//! Storage: daily demand = persons x litres per person per day. A tank's net volume is the demand times its storage days and
//! its gross volume is the net divided by the usable fraction. With a footprint given, the water depth is gross / footprint.
//!
//! Transfer pump (ground tank to roof tank): flow = roof net volume / refill hours. Total head = static head (roof highest
//! water level minus ground lowest water level) + Hazen-Williams friction over the main + minor losses `K v^2 / 2g`. Hydraulic
//! power = rho g Q H with rho = 1000 kg/m3 and g = 9.80665 m/s2; the electrical demand divides by the pump and motor
//! efficiencies and the result is multiplied by the motor margin. The motor is the smallest listed size at or above that.
//! No listed size large enough is a failure, never a silent choice of the largest.
//!
//! Gravity supply (roof tank to the highest fixture), optional: the head available at the lowest roof water level is compared
//! with the residual head the fixture needs plus the friction and minor loss at the peak flow. A shortfall means a booster
//! is needed and is reported as `fails`.
//!
//! This is a preliminary check. It does not cover pump curves or NPSH, water hammer, simultaneous draw during the refill,
//! fire storage, duty/standby arrangement, pipe-by-pipe network losses or tank structure, and it does not replace a
//! plumbing design or the approving authority.

use serde::{Deserialize, Serialize};

/// Density of water, kg/m3.
const RHO: f64 = 1000.0;
/// Standard gravity, m/s2.
const G: f64 = 9.80665;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Main {
    #[serde(default)]
    pub length_m: Option<f64>,
    #[serde(default)]
    pub diameter_mm: Option<f64>,
    /// Hazen-Williams roughness coefficient.
    #[serde(default)]
    pub hazen_c: Option<f64>,
    /// Sum of the minor-loss coefficients of fittings and valves (zero is allowed, but it must be stated).
    #[serde(default)]
    pub minor_k: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Transfer {
    /// Hours the pump has to refill the roof tank's net volume.
    #[serde(default)]
    pub fill_hours: Option<f64>,
    /// Elevation of the lowest water level in the ground tank, m.
    #[serde(default)]
    pub ground_low_level_m: Option<f64>,
    /// Elevation of the highest water level in the roof tank, m.
    #[serde(default)]
    pub roof_high_level_m: Option<f64>,
    #[serde(default)]
    pub main: Main,
    #[serde(default)]
    pub pump_efficiency: Option<f64>,
    #[serde(default)]
    pub motor_efficiency: Option<f64>,
    /// Multiplier on the electrical demand before choosing a motor, at least 1.
    #[serde(default)]
    pub motor_margin: Option<f64>,
    /// Motor sizes on offer, kW. Absent: the motor is not chosen (`unchecked`).
    #[serde(default)]
    pub motor_sizes_kw: Vec<f64>,
    /// Highest allowed velocity in the main, m/s. Absent: `unchecked`.
    #[serde(default)]
    pub max_velocity_mps: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Gravity {
    /// Elevation of the lowest water level in the roof tank, m.
    #[serde(default)]
    pub roof_low_level_m: Option<f64>,
    /// Elevation of the highest fixture served, m.
    #[serde(default)]
    pub highest_fixture_level_m: Option<f64>,
    /// Residual head the highest fixture needs, m of water.
    #[serde(default)]
    pub residual_head_m: Option<f64>,
    #[serde(default)]
    pub peak_flow_lps: Option<f64>,
    #[serde(default)]
    pub main: Main,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WaterInput {
    #[serde(default)]
    pub persons: Option<f64>,
    #[serde(default)]
    pub litres_per_person_day: Option<f64>,
    /// Where the litres per person per day came from.
    #[serde(default)]
    pub demand_source: Option<String>,
    #[serde(default)]
    pub ground_storage_days: Option<f64>,
    #[serde(default)]
    pub roof_storage_days: Option<f64>,
    /// Usable share of a tank's gross volume, in (0, 1].
    #[serde(default)]
    pub usable_fraction: Option<f64>,
    #[serde(default)]
    pub ground_footprint_m2: Option<f64>,
    #[serde(default)]
    pub roof_footprint_m2: Option<f64>,
    /// Deepest water allowed in either tank, m. Absent: `unchecked`.
    #[serde(default)]
    pub max_water_depth_m: Option<f64>,
    #[serde(default)]
    pub transfer: Transfer,
    /// Roof tank to fixtures. Absent: the gravity supply is `unchecked`.
    #[serde(default)]
    pub gravity: Option<Gravity>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub ar: String,
    pub en: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TankRow {
    pub net_m3: f64,
    pub gross_m3: f64,
    pub footprint_m2: Option<f64>,
    pub water_depth_m: Option<f64>,
    pub depth_status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PumpRow {
    pub flow_lps: f64,
    pub flow_m3_h: f64,
    pub static_head_m: f64,
    pub friction_m: f64,
    pub minor_m: f64,
    pub total_head_m: f64,
    pub velocity_mps: f64,
    pub velocity_status: &'static str,
    pub hydraulic_kw: f64,
    pub electrical_kw: f64,
    pub required_motor_kw: f64,
    pub motor_kw: Option<f64>,
    pub motor_status: &'static str,
    pub status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GravityRow {
    pub available_head_m: f64,
    pub residual_head_m: f64,
    pub friction_m: f64,
    pub minor_m: f64,
    pub required_head_m: f64,
    pub margin_m: f64,
    pub velocity_mps: f64,
    pub status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WaterReport {
    pub daily_demand_m3: f64,
    pub ground: TankRow,
    pub roof: TankRow,
    pub transfer: PumpRow,
    pub gravity: Option<GravityRow>,
    pub gravity_status: &'static str,
    pub status: &'static str,
    pub findings: Vec<Finding>,
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn positive(v: f64, field: &str) -> Result<f64, String> {
    if v.is_finite() && v > 0.0 {
        Ok(v)
    } else {
        Err(format!("{field} must be a positive number, got {v}"))
    }
}

fn required(v: Option<f64>, field: &str) -> Result<f64, String> {
    positive(v.ok_or_else(|| format!("{field} is required and has no default"))?, field)
}

fn level(v: Option<f64>, field: &str) -> Result<f64, String> {
    let v = v.ok_or_else(|| format!("{field} is required and has no default"))?;
    if v.is_finite() {
        Ok(v)
    } else {
        Err(format!("{field} must be a finite number, got {v}"))
    }
}

fn fraction(v: Option<f64>, field: &str) -> Result<f64, String> {
    let v = required(v, field)?;
    if v > 1.0 {
        return Err(format!("{field} must be above 0 and at most 1, got {v}"));
    }
    Ok(v)
}

/// A validated transfer or gravity main.
struct Pipe {
    l: f64,
    d: f64,
    c: f64,
    k: f64,
}

fn pipe(m: &Main, scope: &str) -> Result<Pipe, String> {
    let k = m.minor_k.ok_or_else(|| format!("{scope}.main.minor_k is required and has no default (state 0 for none)"))?;
    if !(k.is_finite() && k >= 0.0) {
        return Err(format!("{scope}.main.minor_k must be zero or more, got {k}"));
    }
    Ok(Pipe {
        l: required(m.length_m, &format!("{scope}.main.length_m"))?,
        d: required(m.diameter_mm, &format!("{scope}.main.diameter_mm"))?,
        c: required(m.hazen_c, &format!("{scope}.main.hazen_c"))?,
        k,
    })
}

/// Velocity (m/s), friction (m) and minor loss (m) of `q_lps` through the pipe.
fn losses(p: &Pipe, q_lps: f64) -> (f64, f64, f64) {
    let area = std::f64::consts::PI * (p.d / 1000.0).powi(2) / 4.0;
    let v = (q_lps / 1000.0) / area;
    (v, crate::hydraulics::hazen_williams_m(p.l, q_lps, p.d, p.c), p.k * v * v / (2.0 * G))
}

fn tank(net: f64, usable: f64, footprint: Option<f64>, max_depth: Option<f64>, name: &str) -> Result<TankRow, String> {
    let gross = net / usable;
    let fp = match footprint {
        Some(f) => Some(positive(f, &format!("{name}_footprint_m2"))?),
        None => None,
    };
    let depth = fp.map(|f| gross / f);
    let depth_status = match (depth, max_depth) {
        (Some(d), Some(m)) if d > m => "fails",
        (Some(_), Some(_)) => "ok",
        _ => "unchecked",
    };
    Ok(TankRow { net_m3: round2(net), gross_m3: round2(gross), footprint_m2: fp, water_depth_m: depth.map(round2), depth_status })
}

pub fn check(inp: &WaterInput) -> Result<WaterReport, String> {
    let persons = required(inp.persons, "persons")?;
    let lpcd = required(inp.litres_per_person_day, "litres_per_person_day")?;
    if inp.demand_source.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Err("demand_source is required (where the litres per person per day came from)".into());
    }
    let ground_days = required(inp.ground_storage_days, "ground_storage_days")?;
    let roof_days = required(inp.roof_storage_days, "roof_storage_days")?;
    let usable = fraction(inp.usable_fraction, "usable_fraction")?;
    let max_depth = match inp.max_water_depth_m {
        Some(m) => Some(positive(m, "max_water_depth_m")?),
        None => None,
    };
    let t = &inp.transfer;
    let fill_hours = required(t.fill_hours, "transfer.fill_hours")?;
    let g_low = level(t.ground_low_level_m, "transfer.ground_low_level_m")?;
    let r_high = level(t.roof_high_level_m, "transfer.roof_high_level_m")?;
    let main = pipe(&t.main, "transfer")?;
    let pump_eff = fraction(t.pump_efficiency, "transfer.pump_efficiency")?;
    let motor_eff = fraction(t.motor_efficiency, "transfer.motor_efficiency")?;
    let margin = required(t.motor_margin, "transfer.motor_margin")?;
    if margin < 1.0 {
        return Err(format!("transfer.motor_margin must be at least 1, got {margin}"));
    }
    if let Some(v) = t.max_velocity_mps {
        positive(v, "transfer.max_velocity_mps")?;
    }
    let mut sizes = Vec::with_capacity(t.motor_sizes_kw.len());
    for s in &t.motor_sizes_kw {
        sizes.push(positive(*s, "transfer.motor_sizes_kw entry")?);
    }
    sizes.sort_by(f64::total_cmp);

    let mut findings: Vec<Finding> = Vec::new();
    let daily = persons * lpcd / 1000.0;
    let ground = tank(daily * ground_days, usable, inp.ground_footprint_m2, max_depth, "ground")?;
    let roof = tank(daily * roof_days, usable, inp.roof_footprint_m2, max_depth, "roof")?;
    for (row, ar, en) in [(&ground, "الخزان الأرضي", "ground tank"), (&roof, "الخزان العلوي", "roof tank")] {
        if row.depth_status == "fails" {
            findings.push(Finding {
                ar: format!("عمق الماء في {ar} ({} م) يتجاوز الحد المُدخل ({} م)", row.water_depth_m.unwrap_or(0.0), max_depth.unwrap_or(0.0)),
                en: format!("Water depth in the {en} ({} m) exceeds the given limit ({} m)", row.water_depth_m.unwrap_or(0.0), max_depth.unwrap_or(0.0)),
            });
        }
    }

    // Transfer pump.
    let static_head = r_high - g_low;
    if static_head < 0.0 {
        return Err(format!("transfer: the roof tank's highest water level ({r_high} m) is below the ground tank's lowest ({g_low} m)"));
    }
    // The refill volume is the unrounded net volume, not the two-decimal display figure.
    let flow_lps = daily * roof_days * 1000.0 / (fill_hours * 3600.0);
    let (vel, fric, minor) = losses(&main, flow_lps);
    let head = static_head + fric + minor;
    let hydraulic_kw = RHO * G * (flow_lps / 1000.0) * head / 1000.0;
    let electrical_kw = hydraulic_kw / (pump_eff * motor_eff);
    let required_motor = electrical_kw * margin;
    let vel_status = match t.max_velocity_mps {
        Some(m) if vel > m => "fails",
        Some(_) => "ok",
        None => "unchecked",
    };
    if vel_status == "fails" {
        findings.push(Finding {
            ar: format!("سرعة الماء في خط النقل ({} م/ث) تتجاوز الحد المُدخل ({} م/ث)", round2(vel), t.max_velocity_mps.unwrap_or(0.0)),
            en: format!("Velocity in the transfer main ({} m/s) exceeds the given limit ({} m/s)", round2(vel), t.max_velocity_mps.unwrap_or(0.0)),
        });
    }
    let (motor_kw, motor_status) = if sizes.is_empty() {
        findings.push(Finding {
            ar: "لم تُدخل أحجام المحركات المتاحة فلم يُختر محرك".into(),
            en: "No motor sizes were given, so no motor was chosen".into(),
        });
        (None, "unchecked")
    } else {
        match sizes.iter().find(|s| **s >= required_motor) {
            Some(s) => (Some(*s), "ok"),
            None => {
                findings.push(Finding {
                    ar: format!("أكبر محرك مُدخل ({} ك.و) أقل من المطلوب ({} ك.و)", sizes[sizes.len() - 1], round2(required_motor)),
                    en: format!("The largest motor offered ({} kW) is below the {} kW required", sizes[sizes.len() - 1], round2(required_motor)),
                });
                (None, "fails")
            }
        }
    };
    let pump_status = if vel_status == "fails" || motor_status == "fails" {
        "fails"
    } else if vel_status == "unchecked" || motor_status == "unchecked" {
        "unchecked"
    } else {
        "ok"
    };
    let transfer = PumpRow {
        flow_lps: round2(flow_lps),
        flow_m3_h: round2(flow_lps * 3.6),
        static_head_m: round2(static_head),
        friction_m: round2(fric),
        minor_m: round2(minor),
        total_head_m: round2(head),
        velocity_mps: round2(vel),
        velocity_status: vel_status,
        hydraulic_kw: round2(hydraulic_kw),
        electrical_kw: round2(electrical_kw),
        required_motor_kw: round2(required_motor),
        motor_kw,
        motor_status,
        status: pump_status,
    };

    // Gravity supply from the roof tank.
    let (gravity, gravity_status) = match &inp.gravity {
        None => {
            findings.push(Finding {
                ar: "لم يُدخل قسم الإمداد بالجاذبية فلم يُفحص الضغط عند أعلى نقطة".into(),
                en: "No gravity-supply section was given, so the head at the highest fixture was not checked".into(),
            });
            (None, "unchecked")
        }
        Some(g) => {
            let low = level(g.roof_low_level_m, "gravity.roof_low_level_m")?;
            let fixture = level(g.highest_fixture_level_m, "gravity.highest_fixture_level_m")?;
            let residual = required(g.residual_head_m, "gravity.residual_head_m")?;
            let peak = required(g.peak_flow_lps, "gravity.peak_flow_lps")?;
            let gp = pipe(&g.main, "gravity")?;
            let (gv, gf, gm) = losses(&gp, peak);
            let available = low - fixture;
            let required_head = residual + gf + gm;
            let margin_m = available - required_head;
            let status = if margin_m >= 0.0 { "ok" } else { "fails" };
            if status == "fails" {
                findings.push(Finding {
                    ar: format!("الضغط المتاح من الخزان العلوي ({} م) أقل من المطلوب ({} م): يلزم مضخة رفع ضغط", round2(available), round2(required_head)),
                    en: format!("Head available from the roof tank ({} m) is below the {} m required: a booster pump is needed", round2(available), round2(required_head)),
                });
            }
            (
                Some(GravityRow {
                    available_head_m: round2(available),
                    residual_head_m: round2(residual),
                    friction_m: round2(gf),
                    minor_m: round2(gm),
                    required_head_m: round2(required_head),
                    margin_m: round2(margin_m),
                    velocity_mps: round2(gv),
                    status,
                }),
                status,
            )
        }
    };

    let parts = [ground.depth_status, roof.depth_status, transfer.status, gravity_status];
    let status = if parts.contains(&"fails") {
        "fails"
    } else if parts.contains(&"unchecked") {
        "unchecked"
    } else {
        "ok"
    };
    Ok(WaterReport { daily_demand_m3: round2(daily), ground, roof, transfer, gravity, gravity_status, status, findings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn main_pipe(len: f64, d: f64) -> Main {
        Main { length_m: Some(len), diameter_mm: Some(d), hazen_c: Some(130.0), minor_k: Some(0.0) }
    }

    fn input() -> WaterInput {
        WaterInput {
            persons: Some(100.0),
            litres_per_person_day: Some(200.0),
            demand_source: Some("test data".into()),
            ground_storage_days: Some(1.0),
            roof_storage_days: Some(0.5),
            usable_fraction: Some(0.8),
            ground_footprint_m2: None,
            roof_footprint_m2: None,
            max_water_depth_m: None,
            transfer: Transfer {
                fill_hours: Some(2.0),
                ground_low_level_m: Some(0.0),
                roof_high_level_m: Some(20.0),
                main: main_pipe(30.0, 50.0),
                pump_efficiency: Some(0.5),
                motor_efficiency: Some(0.8),
                motor_margin: Some(1.25),
                motor_sizes_kw: vec![],
                max_velocity_mps: None,
            },
            gravity: None,
        }
    }

    #[test]
    fn storage_volumes_match_hand_calculation() {
        // 100 persons x 200 L = 20 m3/day; ground 1 day, roof 0.5 day; usable 80 %.
        let r = check(&input()).unwrap();
        assert_eq!(r.daily_demand_m3, 20.0);
        assert_eq!((r.ground.net_m3, r.ground.gross_m3), (20.0, 25.0));
        assert_eq!((r.roof.net_m3, r.roof.gross_m3), (10.0, 12.5));
    }

    #[test]
    fn pump_duty_matches_hand_calculation() {
        // Refill 10 m3 in 2 h: Q = 10000 / 7200 = 1.3889 L/s. Friction by Hazen-Williams over 30 m of 50 mm, C = 130.
        let mut inp = input();
        inp.transfer.motor_sizes_kw = vec![0.37, 0.75, 1.5];
        let r = check(&inp).unwrap().transfer;
        let q = 10_000.0 / 7200.0;
        let friction = 10.67 * 30.0 * (q / 1000.0_f64).powf(1.852) / (130.0_f64.powf(1.852) * 0.05_f64.powf(4.87));
        let head = 20.0 + friction;
        let hydraulic = 1000.0 * 9.80665 * (q / 1000.0) * head / 1000.0;
        assert_eq!(r.flow_lps, round2(q));
        assert_eq!(r.static_head_m, 20.0);
        assert_eq!(r.total_head_m, round2(head));
        assert_eq!(r.hydraulic_kw, round2(hydraulic));
        assert_eq!(r.electrical_kw, round2(hydraulic / 0.4));
        assert_eq!(r.required_motor_kw, round2(hydraulic / 0.4 * 1.25));
        // About 0.27 kW hydraulic, 0.68 kW electrical, 0.85 kW required: the 1.5 kW motor is the smallest that covers it.
        assert_eq!(r.motor_kw, Some(1.5));
        assert_eq!(r.motor_status, "ok");
    }

    #[test]
    fn minor_losses_use_velocity_head() {
        let mut inp = input();
        inp.transfer.main.minor_k = Some(10.0);
        let r = check(&inp).unwrap().transfer;
        let v = (10_000.0 / 7200.0 / 1000.0) / (std::f64::consts::PI * 0.05_f64.powi(2) / 4.0);
        assert_eq!(r.minor_m, round2(10.0 * v * v / (2.0 * 9.80665)));
        assert_eq!(r.velocity_mps, round2(v));
    }

    #[test]
    fn motor_too_small_fails_and_no_sizes_leaves_it_unchecked() {
        let mut inp = input();
        assert_eq!(check(&inp).unwrap().transfer.motor_status, "unchecked");
        assert_eq!(check(&inp).unwrap().status, "unchecked");
        inp.transfer.motor_sizes_kw = vec![0.37];
        let r = check(&inp).unwrap();
        assert_eq!(r.transfer.motor_status, "fails");
        assert_eq!(r.transfer.motor_kw, None);
        assert_eq!(r.status, "fails");
    }

    #[test]
    fn velocity_limit_is_checked_only_when_given() {
        let mut inp = input();
        inp.transfer.motor_sizes_kw = vec![1.5];
        assert_eq!(check(&inp).unwrap().transfer.velocity_status, "unchecked");
        inp.transfer.max_velocity_mps = Some(0.5);
        assert_eq!(check(&inp).unwrap().transfer.velocity_status, "fails");
        inp.transfer.max_velocity_mps = Some(2.0);
        assert_eq!(check(&inp).unwrap().transfer.velocity_status, "ok");
    }

    #[test]
    fn tank_depth_needs_footprint_and_limit() {
        let mut inp = input();
        inp.ground_footprint_m2 = Some(10.0);
        inp.roof_footprint_m2 = Some(2.0);
        assert_eq!(check(&inp).unwrap().ground.depth_status, "unchecked");
        inp.max_water_depth_m = Some(2.0);
        let r = check(&inp).unwrap();
        // Ground 25 m3 over 10 m2 = 2.5 m (over the 2 m limit); roof 12.5 m3 over 2 m2 = 6.25 m.
        assert_eq!(r.ground.water_depth_m, Some(2.5));
        assert_eq!(r.ground.depth_status, "fails");
        assert_eq!(r.roof.depth_status, "fails");
        inp.max_water_depth_m = Some(3.0);
        let r = check(&inp).unwrap();
        assert_eq!(r.ground.depth_status, "ok");
        assert_eq!(r.roof.depth_status, "fails");
    }

    #[test]
    fn gravity_shortfall_calls_for_a_booster() {
        let mut inp = input();
        inp.transfer.motor_sizes_kw = vec![1.5];
        inp.gravity = Some(Gravity {
            roof_low_level_m: Some(30.0),
            highest_fixture_level_m: Some(24.0),
            residual_head_m: Some(10.0),
            peak_flow_lps: Some(2.0),
            main: main_pipe(20.0, 50.0),
        });
        let r = check(&inp).unwrap();
        let g = r.gravity.unwrap();
        assert_eq!(g.available_head_m, 6.0);
        assert!(g.required_head_m > 10.0);
        assert_eq!(g.status, "fails");
        assert_eq!(r.status, "fails");
        assert!(r.findings.iter().any(|f| f.en.contains("booster")));
        inp.gravity.as_mut().unwrap().highest_fixture_level_m = Some(15.0);
        let r = check(&inp).unwrap();
        assert_eq!(r.gravity.unwrap().status, "ok");
        // The velocity limit is still unset, so the whole is `unchecked`, not `ok`.
        assert_eq!(r.status, "unchecked");
        inp.transfer.max_velocity_mps = Some(2.0);
        // The tank depth checks are unset too (no footprint or limit); every check must have run before the whole is `ok`.
        assert_eq!(check(&inp).unwrap().status, "unchecked");
        inp.ground_footprint_m2 = Some(10.0);
        inp.roof_footprint_m2 = Some(10.0);
        inp.max_water_depth_m = Some(3.0);
        assert_eq!(check(&inp).unwrap().status, "ok");
    }

    #[test]
    fn missing_values_are_refused_by_name() {
        let mut inp = input();
        inp.persons = None;
        assert!(check(&inp).unwrap_err().contains("persons is required"));
        let mut inp = input();
        inp.demand_source = None;
        assert!(check(&inp).unwrap_err().contains("demand_source is required"));
        let mut inp = input();
        inp.transfer.main.minor_k = None;
        assert!(check(&inp).unwrap_err().contains("transfer.main.minor_k is required"));
        let mut inp = input();
        inp.transfer.pump_efficiency = None;
        assert!(check(&inp).unwrap_err().contains("transfer.pump_efficiency is required"));
        let mut inp = input();
        inp.usable_fraction = Some(1.5);
        assert!(check(&inp).unwrap_err().contains("usable_fraction"));
        let mut inp = input();
        inp.transfer.motor_margin = Some(0.9);
        assert!(check(&inp).unwrap_err().contains("motor_margin must be at least 1"));
        let mut inp = input();
        inp.transfer.roof_high_level_m = Some(-5.0);
        assert!(check(&inp).unwrap_err().contains("below the ground tank"));
        let mut inp = input();
        inp.gravity = Some(Gravity::default());
        assert!(check(&inp).unwrap_err().contains("gravity.roof_low_level_m is required"));
    }
}
