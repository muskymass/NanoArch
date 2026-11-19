//! The structural inputs of milestone 3 (`meta.structural`) and what the structural modules share.
//!
//! Every number here is an input of the user: the code files hold no structural design coefficient (SBC 304 is not among them),
//! so no factor, strength, limit or rate is built in. Where the page offers a starting value (the punching factor 0.33, the 12 m
//! stock length) it comes from the feature brief and is labelled as such.

use crate::gridopt::GridInputs;
use crate::merger::MergeInputs;
use crate::model::Model;
use crate::sizing::{self, SizingInputs};
use crate::slabadv::SlabAdvInputs;
use crate::tanks::{TankDesignIn, TankIn};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CuttingIn {
    /// Length of a stock bar, mm.
    pub stock_mm: Option<f64>,
    /// Lap length when a bar longer than the stock must be spliced, mm.
    pub lap_mm: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StructuralInputs {
    pub fc_mpa: Option<f64>,
    pub fy_mpa: Option<f64>,
    /// Service to factored (ultimate) load.
    pub load_factor: Option<f64>,
    pub phi_shear: Option<f64>,
    /// Punching stress capacity is this factor times the square root of f'c (MPa).
    pub punching_factor: Option<f64>,
    pub cover_mm: Option<f64>,
    pub bar_dia_mm: Option<f64>,
    pub beam_span_depth_max: Option<f64>,
    pub slab_span_depth_max: Option<f64>,
    /// Check the slabs as flat slabs resting on the columns.
    pub treat_slabs_as_flat: bool,
    pub grid: GridInputs,
    pub slabs: SlabAdvInputs,
    pub merge: MergeInputs,
    pub tanks: Vec<TankIn>,
    pub tank_design: TankDesignIn,
    pub cutting: CuttingIn,
}

impl StructuralInputs {
    pub fn check(&self) -> Result<(), String> {
        for (n, v) in [("f'c", self.fc_mpa), ("fy", self.fy_mpa), ("load factor", self.load_factor), ("phi", self.phi_shear), ("punching factor", self.punching_factor), ("cover", self.cover_mm), ("bar diameter", self.bar_dia_mm), ("beam span-to-depth limit", self.beam_span_depth_max), ("slab span-to-depth limit", self.slab_span_depth_max), ("stock length", self.cutting.stock_mm)] {
            if v.is_some_and(|x| !x.is_finite() || x <= 0.0) {
                return Err(format!("{n} must be a positive number"));
            }
        }
        if self.cutting.lap_mm.is_some_and(|x| !x.is_finite() || x < 0.0) {
            return Err("lap length must be zero or more".into());
        }
        self.grid.check()?;
        self.slabs.check()?;
        self.merge.check()?;
        self.tank_design.check()?;
        for t in &self.tanks {
            t.check()?;
        }
        Ok(())
    }
}

/// Element id -> its name in the schedules (`COL-n`, `FND-n` or a combined footing's label, `BM-n`, `SL-n`).
pub fn schedule_names(model: &Model, footing_thickness_mm: Option<f64>) -> BTreeMap<String, String> {
    let rep = sizing::compute(model, None, &SizingInputs { footing_thickness_mm, ..Default::default() });
    let mut m = BTreeMap::new();
    for c in &rep.columns {
        m.insert(c.id.clone(), c.name.clone());
    }
    for f in &rep.footings {
        m.insert(f.id.clone(), f.name.clone());
    }
    for c in &rep.concrete {
        if let Some(id) = &c.id {
            m.insert(id.clone(), c.element.clone());
        }
    }
    m
}
