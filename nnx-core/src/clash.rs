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

/// A conflict of a class other than the hard clash: the two elements, the overlap box and the class.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Conflict {
    /// `clearance`, `routing` or `penetration`.
    pub class: &'static str,
    pub a_id: String,
    pub a_kind: &'static str,
    /// System of the run (`duct`, `pipe`, `drain`, `cable`).
    pub a_system: String,
    pub b_id: String,
    pub b_kind: &'static str,
    pub b_system: String,
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub volume_m3: f64,
    pub level: Option<i64>,
}

/// The conflict classes of the 3D scene. `hard` is exactly `detect`; the other classes are listed apart so a report can state each count.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ClassifiedReport {
    /// Run overlapping a beam, column or footing.
    pub hard: Vec<Clash>,
    /// Run inside the entered clearance of a beam, column or footing without touching it. Empty when no clearance was entered.
    pub clearance: Vec<Conflict>,
    /// Two runs of different systems (duct, pipe, drain, cable) occupying the same space.
    pub routing: Vec<Conflict>,
    /// Run crossing a slab or a wall: a sleeve or an opening is needed, a design decision, so it is listed, not called a clash.
    pub penetrations: Vec<Conflict>,
    pub runs: usize,
    pub structural: usize,
    /// What was not assessed and why.
    pub not_assessed: Vec<String>,
}

fn inflate(b: &Box3, c: f64) -> Box3 {
    let mut o = b.clone();
    for i in 0..3 {
        o.min[i] -= c;
        o.max[i] += c;
    }
    o
}

fn conflict(class: &'static str, a: &Box3, b: &Box3, lo: [f64; 3], hi: [f64; 3]) -> Conflict {
    Conflict { class, a_id: a.id.clone(), a_kind: a.kind, a_system: a.class.clone(), b_id: b.id.clone(), b_kind: b.kind, b_system: b.class.clone(), volume_m3: (0..3).map(|i| hi[i] - lo[i]).product(), min: lo, max: hi, level: a.level }
}

/// Every conflict class the scene can show. `clearance_m` is the gap the user requires around structure (None: not assessed, nothing is assumed).
pub fn classify(scene: &Scene, opts: &ClashOpts, clearance_m: Option<f64>) -> ClassifiedReport {
    let base = detect(scene, opts);
    let runs: Vec<&Box3> = scene.boxes.iter().filter(|b| b.kind == "MepRun").collect();
    let structural: Vec<&Box3> = scene.boxes.iter().filter(|b| matches!(b.kind, "Beam" | "Column" | "Footing")).collect();
    let gap = clearance_m.filter(|c| c.is_finite() && *c > 0.0);
    let mut clearance = Vec::new();
    if let Some(c) = gap {
        for r in &runs {
            for s in &structural {
                if overlap(r, s, opts.tolerance).is_some() {
                    continue;
                }
                if let Some((lo, hi)) = overlap(r, &inflate(s, c), opts.tolerance) {
                    clearance.push(conflict("clearance", r, s, lo, hi));
                }
            }
        }
    }
    let mut routing = Vec::new();
    for (i, a) in runs.iter().enumerate() {
        for b in &runs[i + 1..] {
            if a.class == b.class {
                continue;
            }
            if let Some((lo, hi)) = overlap(a, b, opts.tolerance) {
                routing.push(conflict("routing", a, b, lo, hi));
            }
        }
    }
    let mut penetrations = Vec::new();
    for r in &runs {
        for s in scene.boxes.iter().filter(|b| matches!(b.kind, "Slab" | "Wall")) {
            if let Some((lo, hi)) = overlap(r, s, opts.tolerance) {
                penetrations.push(conflict("penetration", r, s, lo, hi));
            }
        }
    }
    let mut not_assessed = vec!["maintenance access: the model has no service-clearance input for equipment, so no access zone is tested".to_string()];
    if gap.is_none() {
        not_assessed.push("clearance: no required gap was entered, so only overlaps are reported".into());
    }
    routing.sort_by(|x, y| (&x.a_id, &x.b_id).cmp(&(&y.a_id, &y.b_id)));
    penetrations.sort_by(|x, y| (&x.a_id, &x.b_id).cmp(&(&y.a_id, &y.b_id)));
    ClassifiedReport { hard: base.clashes, clearance, routing, penetrations, runs: base.runs, structural: base.structural, not_assessed }
}
