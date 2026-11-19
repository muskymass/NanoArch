//! Geometric clash detection on the 3D scene (`model3d`).
//!
//! A clash is a duct, pipe or drain (`MepRun`) whose box overlaps a structural beam, column or footing by more than
//! `tolerance` in every direction. Both elements are named by their model id, with the overlap box, its volume, and the
//! level, so a click in a viewer or a line in a report points at the exact elements. Slabs are not tested: a run
//! crossing a slab needs a sleeve, which is a design decision, not a clash. Walls and doors are architecture, not
//! structure, and are not tested either.
//!
//! The test is exact for the boxes the scene holds (axis-aligned, in metres). It finds nothing when no runs exist in the
//! model, which is the case until they are entered or imported.

use crate::model3d::{Box3, Scene};
use serde::Serialize;

#[derive(Debug, Clone, Copy)]
pub struct ClashOpts {
    /// Overlap smaller than this (metres) in any direction is a touch, not a clash.
    pub tolerance: f64,
}

impl Default for ClashOpts {
    fn default() -> Self {
        Self { tolerance: 0.005 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Clash {
    /// The run (`MepRun`) and its system: `duct`, `pipe` or `drain`.
    pub run_id: String,
    pub run_system: String,
    pub run_discipline: &'static str,
    /// The structural element hit, its kind (`Beam`, `Column`, `Footing`) and discipline.
    pub struct_id: String,
    pub struct_kind: &'static str,
    pub struct_discipline: &'static str,
    /// The overlap box, metres.
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub volume_m3: f64,
    /// Level index of the run (`None` for a model without levels).
    pub level: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ClashReport {
    pub clashes: Vec<Clash>,
    /// Number of runs and structural elements that were tested against each other.
    pub runs: usize,
    pub structural: usize,
}

fn overlap(a: &Box3, b: &Box3, tol: f64) -> Option<([f64; 3], [f64; 3])> {
    let mut lo = [0.0; 3];
    let mut hi = [0.0; 3];
    for i in 0..3 {
        lo[i] = a.min[i].max(b.min[i]);
        hi[i] = a.max[i].min(b.max[i]);
        if hi[i] - lo[i] <= tol {
            return None;
        }
    }
    Some((lo, hi))
}

/// Every run-versus-structure clash in `scene`, ordered by run id, then structural id.
pub fn detect(scene: &Scene, opts: &ClashOpts) -> ClashReport {
    let runs: Vec<&Box3> = scene.boxes.iter().filter(|b| b.kind == "MepRun").collect();
    let structural: Vec<&Box3> = scene.boxes.iter().filter(|b| matches!(b.kind, "Beam" | "Column" | "Footing")).collect();
    let mut clashes = Vec::new();
    for r in &runs {
        for s in &structural {
            if let Some((min, max)) = overlap(r, s, opts.tolerance) {
                clashes.push(Clash {
                    run_id: r.id.clone(),
                    run_system: r.class.clone(),
                    run_discipline: r.discipline,
                    struct_id: s.id.clone(),
                    struct_kind: s.kind,
                    struct_discipline: s.discipline,
                    volume_m3: (0..3).map(|i| max[i] - min[i]).product(),
                    min,
                    max,
                    level: r.level,
                });
            }
        }
    }
    clashes.sort_by(|a, b| (&a.run_id, &a.struct_id).cmp(&(&b.run_id, &b.struct_id)));
    ClashReport { clashes, runs: runs.len(), structural: structural.len() }
}
