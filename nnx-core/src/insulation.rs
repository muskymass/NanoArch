//! Thermal insulation and waterproofing assemblies: U-values and the quantities they imply.
//!
//! The mechanics are physics: `U = 1 / (Rsi + sum(thickness / lambda) + Rse)`, and a quantity is an area
//! (membranes, finishes) or a volume (insulation boards) taken from the model's elements. The DATA is yours:
//! layer thicknesses, thermal conductivities (lambda), the surface resistances Rsi and Rse, the U-value limit
//! of the code that applies, and the height a waterproofing membrane turns up the wall. Nothing is defaulted;
//! an assembly with a missing value is reported as incomplete and names what is missing.
//!
//! Scopes (the assembly names the library may define):
//! - `wall`: exterior walls, net of their doors and windows.
//! - `roof`: the slabs of the top level (a single-storey plan: every slab is a roof).
//! - `wet_floor`: waterproofing only (no U-value): floor area of bathrooms and toilets, plus the membrane turn-up along their perimeter when
//!   `wet_upturn_mm` is given.
//!
//! A layer with `kind: "membrane"` and no lambda is left out of the thermal sum (its resistance is
//! negligible); every other layer needs a lambda for a U-value to be computed.

use crate::model::{Id, Model};
use crate::qto::{QtoItem, Unit};
use crate::typed::{Slab, Space, Typed, Wall};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Layer {
    pub material: String,
    pub thickness_mm: f64,
    /// Thermal conductivity, W/(m K).
    #[serde(default)]
    pub lambda: Option<f64>,
    /// `insulation` (counted by volume), `membrane` (area, no thermal resistance) or anything else (area).
    #[serde(default)]
    pub kind: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Assembly {
    #[serde(default)]
    pub layers: Vec<Layer>,
    /// Internal and external surface resistances, m2 K / W.
    #[serde(default)]
    pub rsi: Option<f64>,
    #[serde(default)]
    pub rse: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct InsulationLib {
    #[serde(default)]
    pub assemblies: BTreeMap<String, Assembly>,
    /// Maximum allowed U per scope, W/(m2 K), from the code that applies to the project.
    #[serde(default)]
    pub u_limits: BTreeMap<String, f64>,
    #[serde(default)]
    pub wet_upturn_mm: Option<f64>,
}

impl Assembly {
    /// U in W/(m2 K). `Err` lists every missing value.
    pub fn u_value(&self) -> Result<f64, Vec<String>> {
        let mut missing = Vec::new();
        if self.rsi.is_none() {
            missing.push("rsi".to_string());
        }
        if self.rse.is_none() {
            missing.push("rse".to_string());
        }
        let mut r = self.rsi.unwrap_or(0.0) + self.rse.unwrap_or(0.0);
        for l in &self.layers {
            match l.lambda {
                Some(lam) if lam > 0.0 => r += l.thickness_mm / 1000.0 / lam,
                None if l.kind == "membrane" => {}
                _ => missing.push(format!("lambda of '{}'", l.material)),
            }
        }
        if self.layers.is_empty() {
            missing.push("layers".to_string());
        }
        if missing.is_empty() { Ok(1.0 / r) } else { Err(missing) }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AssemblyReport {
    pub scope: String,
    pub area_m2: f64,
    pub elements: usize,
    pub u_value: Option<f64>,
    pub u_limit: Option<f64>,
    /// `ok`, `exceeds limit`, `no limit given` or `incomplete`.
    pub status: String,
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct InsulationReport {
    pub assemblies: Vec<AssemblyReport>,
    pub items: Vec<QtoItem>,
    pub warnings: Vec<String>,
}

const SCOPES: [&str; 3] = ["wall", "roof", "wet_floor"];

/// Area (m2) and source elements of one scope, measured from the model.
/// The exterior walls with their area net of the doors and windows they host, m2.
pub(crate) fn exterior_wall_areas(model: &Model) -> Vec<(Id, f64)> {
    let mut open: BTreeMap<Id, f64> = BTreeMap::new();
    for e in model.elements.values().filter(|e| e.kind == "Door" || e.kind == "Window") {
        if let (Some(h), Some(w), Some(hh)) = (e.host_id, e.props.get("width").and_then(|v| v.as_f64()), e.props.get("height").and_then(|v| v.as_f64())) {
            *open.entry(h).or_default() += w * hh;
        }
    }
    model
        .elements
        .values()
        .filter(|e| e.kind == Wall::KIND)
        .filter_map(|e| e.typed::<Wall>().ok().filter(|w| w.is_exterior).map(|w| (e.id, ((w.length * w.height) as f64 - open.get(&e.id).copied().unwrap_or(0.0)).max(0.0) / 1e6)))
        .collect()
}

pub(crate) fn scope_area(model: &Model, scope: &str, upturn_mm: Option<f64>) -> (f64, Vec<Id>) {
    match scope {
        "wall" => {
            let walls = exterior_wall_areas(model);
            (walls.iter().map(|(_, a)| a).sum(), walls.iter().map(|(id, _)| *id).collect())
        }
        "roof" => {
            // With several levels only the top level's slabs are the roof; the others are floors between storeys.
            let top = crate::building::levels(model).last().map(|(_, id)| *id);
            let slabs: Vec<_> = model.elements.values().filter(|e| e.kind == Slab::KIND && (top.is_none() || e.level_id == top)).filter_map(|e| Some((e.id, e.typed::<Slab>().ok()?))).collect();
            (slabs.iter().map(|(_, s)| (s.w * s.h) as f64 / 1e6).sum(), slabs.iter().map(|(id, _)| *id).collect())
        }
        "wet_floor" => {
            let rooms: Vec<_> = model
                .elements
                .values()
                .filter(|e| e.kind == Space::KIND)
                .filter_map(|e| Some((e.id, e.typed::<Space>().ok()?)))
                .filter(|(_, s)| s.func == "bathroom" || s.func == "toilet")
                .collect();
            let floor: f64 = rooms.iter().map(|(_, s)| s.area).sum();
            let turn_up: f64 = upturn_mm.map_or(0.0, |u| rooms.iter().map(|(_, s)| 2.0 * (s.w + s.h) as f64 / 1000.0 * u / 1000.0).sum());
            (floor + turn_up, rooms.iter().map(|(id, _)| *id).collect())
        }
        _ => (0.0, Vec::new()),
    }
}

pub fn analyze(model: &Model, lib: &InsulationLib) -> InsulationReport {
    analyze_for_phase(model, lib, None)
}

/// Phase-filtered insulation quantity calculation. The original, unfiltered model
/// determines roof level and hosted openings to prevent false extra quantities.
pub fn analyze_for_phase(model: &Model, lib: &InsulationLib, phase: Option<&str>) -> InsulationReport {
    let mut rep = InsulationReport::default();
    for (scope, asm) in &lib.assemblies {
        if !SCOPES.contains(&scope.as_str()) {
            rep.warnings.push(format!("assembly '{scope}' is not a known scope (known: {})", SCOPES.join(", ")));
            continue;
        }
        let (full_area, full_ids) = scope_area(model, scope, lib.wet_upturn_mm);
        let (area, ids) = if let Some(phase) = phase {
            let included: Vec<Id> = full_ids.into_iter().filter(|id| model.elements.get(id).is_some_and(|e| crate::qto::matches_phase(e, Some(phase)))).collect();
            let area = match scope.as_str() {
                "wall" => exterior_wall_areas(model).into_iter().filter(|(id, _)| included.contains(id)).map(|(_, a)| a).sum(),
                "roof" => model.elements.values().filter(|e| included.contains(&e.id)).filter_map(|e| e.typed::<Slab>().ok()).map(|s| (s.w * s.h) as f64 / 1e6).sum(),
                "wet_floor" => model.elements.values().filter(|e| included.contains(&e.id)).filter_map(|e| e.typed::<Space>().ok()).map(|s| s.area + lib.wet_upturn_mm.unwrap_or(0.0) / 1e6 * 2.0 * (s.w + s.h) as f64).sum(),
                _ => 0.0,
            };
            (area, included)
        } else { (full_area, full_ids) };
        if ids.is_empty() {
            rep.warnings.push(format!("no {scope} elements in the model: '{scope}' has no area"));
        }
        // Waterproofing under wet floors has no thermal requirement: only its quantities are produced.
        let thermal = scope != "wet_floor";
        let (u, missing) = match asm.u_value() {
            _ if !thermal => (None, Vec::new()),
            Ok(u) => (Some(u), Vec::new()),
            Err(m) => (None, m),
        };
        let limit = lib.u_limits.get(scope).copied();
        let status = match (u, limit) {
            _ if !thermal => "quantities only",
            (None, _) => "incomplete",
            (Some(_), None) => "no limit given",
            (Some(u), Some(l)) if u <= l + 1e-9 => "ok",
            _ => "exceeds limit",
        };
        rep.assemblies.push(AssemblyReport {
            scope: scope.clone(),
            area_m2: (area * 1e4).round() / 1e4,
            elements: ids.len(),
            u_value: u.map(|u| (u * 1e4).round() / 1e4),
            u_limit: limit,
            status: status.into(),
            missing,
        });
        if ids.is_empty() {
            continue;
        }
        for l in &asm.layers {
            let (unit, q) = if l.kind == "insulation" { (Unit::M3, area * l.thickness_mm / 1000.0) } else { (Unit::M2, area) };
            rep.items.push(QtoItem {
                code: format!("INS.{scope}.{}", l.material),
                description: format!("{} {} ({scope}, {} mm)", if l.kind == "insulation" { "Insulation" } else { "Layer" }, l.material, l.thickness_mm),
                unit,
                quantity: (q * 1e4).round() / 1e4,
                waste_pct: None,
                sources: ids.clone(),
                parts: Vec::new(),
            });
        }
    }
    rep
}
