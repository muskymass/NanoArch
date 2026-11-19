//! What the user enters for the compliance pack (`meta.compliance`), the baseline of the change log, and the revision history.
//!
//! Every value here is an INPUT stored in the project (so it takes part in undo, redo, dirty and the `.nnx` file). Nothing in this
//! module is a statutory value; the audits that read it (`thermal`, `audits`, `malath`) cite the clause for each limit they use
//! and name any input that is missing.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One layer of a wall or roof: a catalog material (or `custom`) and its thickness. `lambda` overrides or supplies the conductivity.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LayerPick {
    pub material: String,
    pub thickness_mm: Option<f64>,
    pub lambda: Option<f64>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThermalInputs {
    pub wall_layers: Vec<LayerPick>,
    pub roof_layers: Vec<LayerPick>,
    /// Surface resistances, m2 K / W.
    pub rsi: Option<f64>,
    pub rse: Option<f64>,
    pub window_u: Option<f64>,
    pub window_shgc: Option<f64>,
    /// Extra heat loss through thermal bridges, percent of the weighted U. Left out when not given.
    pub bridge_pct: Option<f64>,
    /// Upper bounds of bands A to E as a ratio of the weighted U to the reference U of the zone (five ascending numbers); F is above.
    pub band_ratios: Option<Vec<f64>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AccessInputs {
    /// Smallest clear door width, mm, and what to take off the nominal width to get the clear width (zero when not entered).
    pub door_min_clear_mm: Option<f64>,
    pub door_deduction_mm: Option<f64>,
    /// Steepest ramp as `1 : N`.
    pub ramp_max_ratio_n: Option<f64>,
    /// Diameter of the turning circle in an accessible bathroom, mm.
    pub turning_circle_mm: Option<f64>,
    pub entrance_ramp_rise_mm: Option<f64>,
    pub entrance_ramp_run_mm: Option<f64>,
    /// Keys of the rooms designed as accessible bathrooms.
    pub accessible_rooms: Vec<String>,
    /// Where these limits come from (the code section, the authority's circular...).
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Lightwell {
    pub name: String,
    pub width_m: f64,
    pub length_m: f64,
    /// Number of floors the lightwell rises through.
    pub floors: u32,
    /// Windows open from two opposite sides or from all sides (otherwise from one side or two adjacent sides).
    pub opposite_or_all: bool,
    /// Keys of the rooms that take light or air from it.
    pub rooms: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VentInputs {
    pub lightwells: Vec<Lightwell>,
    /// Keys of the rooms served by a mechanical exhaust duct.
    pub exhaust_rooms: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Projection {
    pub name: String,
    /// Floor number, 2 and up.
    pub level: u32,
    /// `south`, `north`, `east` or `west` of the building.
    pub side: String,
    pub depth_m: f64,
    /// `balcony`, `bay` or `cornice`.
    pub kind: String,
    /// Height of the balcony's protective barrier, m (balconies only).
    pub guard_height_m: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SoilReport {
    /// `none`, `ordered`, `received` or `reviewed`.
    pub status: String,
    pub borings: Option<u32>,
    pub reference: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CoverInput {
    /// `ground`, `weather` or `protected`.
    pub exposure: Option<String>,
    /// `slab` (slabs, joists, walls) or `frame` (beams, columns, ties).
    pub member: Option<String>,
    pub bar_dia_mm: Option<f64>,
    pub specified_mm: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MalathState {
    /// `pending`, `accepted`, `rejected` or `na`.
    pub status: String,
    pub note: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ComplianceInputs {
    /// Width of the street at the front of the plot, m (when the plot outline does not carry it).
    pub street_width_m: Option<f64>,
    pub thermal: ThermalInputs,
    pub access: AccessInputs,
    pub vent: VentInputs,
    pub projections: Vec<Projection>,
    pub soil: SoilReport,
    pub cover: Option<CoverInput>,
    /// Checklist item id -> state.
    pub malath: BTreeMap<String, MalathState>,
}

impl ComplianceInputs {
    /// What is wrong with the numbers themselves.
    pub fn check(&self) -> Result<(), String> {
        let pos = |n: &str, v: Option<f64>| match v {
            Some(x) if !x.is_finite() || x <= 0.0 => Err(format!("{n} must be a positive number")),
            _ => Ok(()),
        };
        pos("street width", self.street_width_m)?;
        let t = &self.thermal;
        for (n, v) in [("surface resistance Rsi", t.rsi), ("surface resistance Rse", t.rse), ("window U", t.window_u), ("window SHGC", t.window_shgc)] {
            pos(n, v)?;
        }
        if t.bridge_pct.is_some_and(|b| !b.is_finite() || b < 0.0) {
            return Err("thermal bridge percentage must be zero or more".into());
        }
        for l in t.wall_layers.iter().chain(&t.roof_layers) {
            pos(&format!("thickness of layer '{}'", l.material), l.thickness_mm)?;
            pos(&format!("lambda of layer '{}'", l.material), l.lambda)?;
        }
        if let Some(b) = &t.band_ratios
            && (b.len() != 5 || b.iter().any(|x| !x.is_finite() || *x <= 0.0) || b.windows(2).any(|w| w[0] >= w[1]))
        {
            return Err("the five band limits (A to E) must be positive and ascending".into());
        }
        let a = &self.access;
        for (n, v) in [("door clear width", a.door_min_clear_mm), ("ramp ratio N", a.ramp_max_ratio_n), ("turning circle", a.turning_circle_mm), ("ramp rise", a.entrance_ramp_rise_mm), ("ramp run", a.entrance_ramp_run_mm)] {
            pos(n, v)?;
        }
        if a.door_deduction_mm.is_some_and(|d| !d.is_finite() || d < 0.0) {
            return Err("door deduction must be zero or more".into());
        }
        for l in &self.vent.lightwells {
            if !(l.width_m.is_finite() && l.width_m > 0.0 && l.length_m.is_finite() && l.length_m > 0.0) || l.floors == 0 {
                return Err(format!("lightwell '{}': width, length and floors must be positive", l.name));
            }
        }
        for p in &self.projections {
            if !(p.depth_m.is_finite() && p.depth_m >= 0.0) || p.level < 2 || !["south", "north", "east", "west"].contains(&p.side.as_str()) || !["balcony", "bay", "cornice"].contains(&p.kind.as_str()) {
                return Err(format!("projection '{}': level (2 or more), side (south, north, east, west), kind (balcony, bay, cornice) and a depth of zero or more are required", p.name));
            }
            pos(&format!("guard height of '{}'", p.name), p.guard_height_m)?;
        }
        if !["", "none", "ordered", "received", "reviewed"].contains(&self.soil.status.as_str()) {
            return Err(format!("unknown soil report status '{}'", self.soil.status));
        }
        if let Some(c) = &self.cover {
            if c.exposure.as_deref().is_some_and(|e| !["ground", "weather", "protected"].contains(&e)) || c.member.as_deref().is_some_and(|m| !["slab", "frame"].contains(&m)) {
                return Err("concrete cover: exposure is ground, weather or protected; member is slab or frame".into());
            }
            pos("bar diameter", c.bar_dia_mm)?;
            pos("specified cover", c.specified_mm)?;
        }
        for (id, s) in &self.malath {
            if !["pending", "accepted", "rejected", "na"].contains(&s.status.as_str()) {
                return Err(format!("checklist item '{id}': unknown status '{}'", s.status));
            }
        }
        Ok(())
    }
}

/// The model as it was when the baseline was set: per element id, its kind and a hash of its properties.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Baseline {
    pub name: String,
    pub created_unix: u64,
    pub elements: BTreeMap<String, (String, u64)>,
}

/// One closed revision: the baseline that was replaced and what had changed against it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RevisionEntry {
    pub rev: u32,
    pub name: String,
    pub created_unix: u64,
    pub added: usize,
    pub modified: usize,
    pub removed: usize,
}
