//! Inputs of the advanced MEP pack (`meta.mepExt`): rooftop solar, smart-home topology, Wi-Fi and CCTV, EV chargers, phase balancing and
//! voltage drop, greywater, and plumbing head loss with pump sizing.
//!
//! Every value is an input of the user and is stored in the project (undo, redo, dirty and the `.nnx` file follow). The application
//! holds NO tariff, panel rating, generation figure, radio or cable parameter, and no pump or pipe coefficient: a missing one is named
//! and its calculation is not run. The only constants in the engines are physical (the free-space path-loss formula, the density of
//! water and gravity, the square root of three) and the Hazen-Williams equation, a textbook formula whose roughness coefficient C is
//! yours. Results are preliminary.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `[x0, y0, x1, y1]` in millimetres.
pub type Rect4 = [f64; 4];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SolarIn {
    pub panel_w: Option<f64>,
    pub panel_area_m2: Option<f64>,
    /// Clear strip kept free along every roof edge (parapet, access), mm.
    pub edge_clearance_mm: Option<f64>,
    /// Share of the usable roof the panels may cover (row spacing, walkways), 0 to 1.
    pub coverage_ratio: Option<f64>,
    /// Other things on the roof (water tanks, AC units), mm rectangles.
    pub obstructions: Vec<Rect4>,
    /// Annual energy per installed kW, kWh/kW/year, for this site (from your own source).
    pub specific_yield_kwh_kw: Option<f64>,
    pub tariff_per_kwh: Option<f64>,
    /// Share of the energy used in the building rather than exported, 0 to 1.
    pub self_consumption: Option<f64>,
    /// Panel DC power divided by inverter AC power.
    pub dc_ac_ratio: Option<f64>,
    pub annual_consumption_kwh: Option<f64>,
    /// Smallest share of the roof that must stay usable for the roof to count as suitable, 0 to 1 (your own criterion, with its source).
    pub min_usable_share: Option<f64>,
    /// Fewest panels that make an installation worth doing (your own criterion).
    pub min_panels: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SmartDevice {
    pub name: String,
    /// `lighting`, `hvac`, `shading`, `lock`, `sensor` or `switch`.
    pub kind: String,
    pub room: String,
    pub x_mm: Option<f64>,
    pub y_mm: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SmartIn {
    /// `knx` (wired bus), `matter` or `zigbee` (wireless: no bus is routed).
    pub protocol: String,
    /// Room of the central smart panel.
    pub panel_room: Option<String>,
    /// What one bus line carries and how long it may be: your product's limits.
    pub max_devices_per_line: Option<f64>,
    pub max_line_m: Option<f64>,
    /// Multiplier from straight-line to routed length.
    pub route_factor: Option<f64>,
    /// Height of the bus above the floor top and its section, mm.
    pub bus_z_mm: Option<f64>,
    pub bus_mm: Option<f64>,
    pub devices: Vec<SmartDevice>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AccessPoint {
    pub name: String,
    pub x_mm: f64,
    pub y_mm: f64,
    /// Floor number the access point is on (1 = ground); 1 when absent.
    pub floor: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WifiIn {
    pub band_ghz: Option<f64>,
    pub tx_power_dbm: Option<f64>,
    pub antenna_gain_dbi: Option<f64>,
    /// Path-loss exponent of the log-distance model for the rooms.
    pub exponent: Option<f64>,
    pub interior_wall_db: Option<f64>,
    pub exterior_wall_db: Option<f64>,
    /// Weakest signal that is still acceptable, dBm.
    pub threshold_dbm: Option<f64>,
    pub target_coverage_pct: Option<f64>,
    pub max_aps: Option<f64>,
    /// Floor to analyse (1 = ground).
    pub floor: Option<u32>,
    /// Access points you place; more are suggested until the target is met.
    pub aps: Vec<AccessPoint>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Camera {
    pub name: String,
    pub x_mm: f64,
    pub y_mm: f64,
    pub mount_height_mm: f64,
    /// Compass bearing the camera looks along, degrees clockwise from north (the plan's +y).
    pub azimuth_deg: f64,
    pub fov_deg: f64,
    pub range_m: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CctvIn {
    pub cameras: Vec<Camera>,
    /// The line around the building (this far out) whose coverage is reported, mm.
    pub perimeter_offset_mm: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Charger {
    pub name: String,
    pub current_a: f64,
    /// 1 or 3.
    pub phases: u32,
    pub x_mm: Option<f64>,
    pub y_mm: Option<f64>,
    /// Cable length when you know it; otherwise the straight distance from the board times the route factor.
    pub length_m: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EvIn {
    pub chargers: Vec<Charger>,
    /// A charger is sized for its current times this factor (continuous load).
    pub continuous_factor: Option<f64>,
    /// Share of the chargers' power that runs together in the main distribution board's demand.
    pub demand_factor: Option<f64>,
    /// Room of the board that feeds them (the first board of the electrical inputs when absent).
    pub board_room: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BalanceIn {
    /// Largest accepted imbalance between the phase currents, percent of the mean.
    pub max_imbalance_pct: Option<f64>,
    pub feeder_length_m: Option<f64>,
    pub feeder_mm2: Option<f64>,
    /// Largest total voltage drop (feeder plus circuit), percent.
    pub total_vdrop_max_pct: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GreyIn {
    /// Fixture names (from the plumbing catalog) that discharge greywater.
    pub fixtures: Vec<String>,
    /// Litres per day of each fixture name.
    pub fixture_l_day: BTreeMap<String, f64>,
    /// Count the air-conditioning condensate as greywater.
    pub condensate: bool,
    pub condensate_l_day_per_unit: Option<f64>,
    pub unit_x_mm: Option<f64>,
    pub unit_y_mm: Option<f64>,
    /// What the treated water is used for, litres per day, and the storage in days.
    pub reuse_demand_l_day: Option<f64>,
    pub storage_days: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HydraIn {
    /// Hazen-Williams roughness coefficient C of the pipe material.
    pub hazen_c: Option<f64>,
    /// Fittings and valves as a percentage added to the pipe length.
    pub fittings_pct: Option<f64>,
    /// Water level at the pump and the highest outlet it must serve, mm above the ground floor top (derived from the tank and the
    /// top level when absent).
    pub source_z_mm: Option<f64>,
    pub outlet_z_mm: Option<f64>,
    /// Pressure needed at the outlet, metres of water.
    pub residual_head_m: Option<f64>,
    /// Pressure already available at the pump inlet, metres of water.
    pub inlet_head_m: Option<f64>,
    pub pump_efficiency: Option<f64>,
    /// Extra on the motor power, as a fraction (0.15 = 15 %).
    pub motor_margin: Option<f64>,
    /// Add a standby pump.
    pub standby: bool,
    /// Room whose outlet is the farthest (the farthest by distance when absent).
    pub farthest_room: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MepExtInputs {
    pub solar: SolarIn,
    pub smart: SmartIn,
    pub wifi: WifiIn,
    pub cctv: CctvIn,
    pub ev: EvIn,
    pub balance: BalanceIn,
    pub grey: GreyIn,
    pub hydra: HydraIn,
}

impl MepExtInputs {
    /// What is wrong with the numbers themselves.
    pub fn check(&self) -> Result<(), String> {
        let pos = |n: &str, v: Option<f64>| match v {
            Some(x) if !x.is_finite() || x <= 0.0 => Err(format!("{n} must be a positive number")),
            _ => Ok(()),
        };
        let nonneg = |n: &str, v: Option<f64>| match v {
            Some(x) if !x.is_finite() || x < 0.0 => Err(format!("{n} must be zero or more")),
            _ => Ok(()),
        };
        let s = &self.solar;
        for (n, v) in [("panel power", s.panel_w), ("panel area", s.panel_area_m2), ("specific yield", s.specific_yield_kwh_kw), ("dc/ac ratio", s.dc_ac_ratio), ("annual consumption", s.annual_consumption_kwh)] {
            pos(n, v)?;
        }
        nonneg("edge clearance", s.edge_clearance_mm)?;
        nonneg("tariff", s.tariff_per_kwh)?;
        for (n, v) in [("coverage ratio", s.coverage_ratio), ("self-consumption share", s.self_consumption), ("minimum usable share", s.min_usable_share)] {
            if v.is_some_and(|x| !x.is_finite() || !(0.0..=1.0).contains(&x)) {
                return Err(format!("{n} is between 0 and 1"));
            }
        }
        let m = &self.smart;
        if !["", "knx", "matter", "zigbee"].contains(&m.protocol.as_str()) {
            return Err(format!("unknown smart-home protocol '{}' (knx, matter or zigbee)", m.protocol));
        }
        for d in &m.devices {
            if !["lighting", "hvac", "shading", "lock", "sensor", "switch"].contains(&d.kind.as_str()) {
                return Err(format!("device '{}': kind is lighting, hvac, shading, lock, sensor or switch", d.name));
            }
        }
        for (n, v) in [("devices per line", m.max_devices_per_line), ("line length", m.max_line_m), ("route factor", m.route_factor), ("bus section", m.bus_mm)] {
            pos(n, v)?;
        }
        let w = &self.wifi;
        for (n, v) in [("frequency band", w.band_ghz), ("path-loss exponent", w.exponent), ("target coverage", w.target_coverage_pct), ("maximum access points", w.max_aps)] {
            pos(n, v)?;
        }
        nonneg("interior wall loss", w.interior_wall_db)?;
        nonneg("exterior wall loss", w.exterior_wall_db)?;
        for c in &self.cctv.cameras {
            if !(c.fov_deg > 0.0 && c.fov_deg <= 360.0 && c.range_m > 0.0 && c.mount_height_mm >= 0.0 && c.azimuth_deg.is_finite()) {
                return Err(format!("camera '{}': field of view (1-360), range and mount height must be valid numbers", c.name));
            }
        }
        nonneg("perimeter offset", self.cctv.perimeter_offset_mm)?;
        for c in &self.ev.chargers {
            if !(c.current_a.is_finite() && c.current_a > 0.0) || ![1, 3].contains(&c.phases) {
                return Err(format!("charger '{}': current must be positive and phases 1 or 3", c.name));
            }
        }
        pos("EV continuous factor", self.ev.continuous_factor)?;
        if self.ev.demand_factor.is_some_and(|x| !x.is_finite() || !(0.0..=1.0).contains(&x)) {
            return Err("EV demand factor is between 0 and 1".into());
        }
        for (n, v) in [("imbalance limit", self.balance.max_imbalance_pct), ("feeder length", self.balance.feeder_length_m), ("feeder section", self.balance.feeder_mm2), ("total voltage-drop limit", self.balance.total_vdrop_max_pct)] {
            pos(n, v)?;
        }
        let g = &self.grey;
        for (n, v) in [("condensate per unit", g.condensate_l_day_per_unit), ("reuse demand", g.reuse_demand_l_day), ("storage days", g.storage_days)] {
            nonneg(n, v)?;
        }
        if g.fixture_l_day.values().any(|v| !v.is_finite() || *v < 0.0) {
            return Err("fixture litres per day must be zero or more".into());
        }
        let h = &self.hydra;
        for (n, v) in [("Hazen-Williams C", h.hazen_c), ("pump efficiency", h.pump_efficiency)] {
            pos(n, v)?;
        }
        if h.pump_efficiency.is_some_and(|e| e > 1.0) {
            return Err("pump efficiency is a fraction up to 1".into());
        }
        nonneg("fittings percentage", h.fittings_pct)?;
        nonneg("residual head", h.residual_head_m)?;
        nonneg("inlet head", h.inlet_head_m)?;
        nonneg("motor margin", h.motor_margin)?;
        Ok(())
    }
}

pub(crate) fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Area (m2) of the union of `a` minus the union of `b`, by coordinate compression (exact for rectangles). Millimetre rectangles.
pub fn area_minus_m2(a: &[Rect4], b: &[Rect4]) -> f64 {
    let mut xs: Vec<f64> = a.iter().chain(b).flat_map(|r| [r[0], r[2]]).collect();
    let mut ys: Vec<f64> = a.iter().chain(b).flat_map(|r| [r[1], r[3]]).collect();
    xs.sort_by(f64::total_cmp);
    xs.dedup();
    ys.sort_by(f64::total_cmp);
    ys.dedup();
    let inside = |rs: &[Rect4], x: f64, y: f64| rs.iter().any(|r| x > r[0] && x < r[2] && y > r[1] && y < r[3]);
    let mut area = 0.0;
    for wx in xs.windows(2) {
        for wy in ys.windows(2) {
            let (cx, cy) = ((wx[0] + wx[1]) / 2.0, (wy[0] + wy[1]) / 2.0);
            if inside(a, cx, cy) && !inside(b, cx, cy) {
                area += (wx[1] - wx[0]) * (wy[1] - wy[0]);
            }
        }
    }
    area / 1e6
}
