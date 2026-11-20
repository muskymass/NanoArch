//! Wall assemblies: named wall types made of explicit material layers (exterior insulated wall, interior partition, structural shear
//! wall, retaining wall), stored in the project (`meta.assemblies`) and read by the thermal check, the take-off and the 3D view.
//!
//! - **One source of truth.** The layers live here and nowhere else for the walls that use them. The thermal U-value, the layer
//!   quantities of the BOQ and the thickness of the wall all derive from them. A wall's thickness is the sum of its assembly's layers.
//! - **Nothing is invented.** A layer is a catalog material (or `custom`) with a thickness the user entered; a conductivity the catalog
//!   does not hold is entered by the user (`lambda`). A missing value is named by the check that needs it.
//! - **Which wall uses which assembly.** A wall names one explicitly (`assembly` on the wall), or falls back to the project's default
//!   for exterior walls or for interior walls (`meta.assemblyDefaults`). A wall with neither keeps the project's single wall build-up
//!   (`compliance.thermal.wallLayers`) for the thermal check and has no layer quantities.

use crate::command::Command;
use crate::compliance::{LayerPick, ThermalInputs};
use crate::model::{Element, Id, Model};
use crate::typed::{Typed, Wall};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssemblyKind {
    #[default]
    ExteriorInsulated,
    InteriorPartition,
    /// A structural shear wall (carries load: it may stand inside or on the envelope).
    Structural,
    Retaining,
}

impl AssemblyKind {
    pub const ALL: [AssemblyKind; 4] = [AssemblyKind::ExteriorInsulated, AssemblyKind::InteriorPartition, AssemblyKind::Structural, AssemblyKind::Retaining];
    pub fn id(self) -> &'static str {
        match self {
            AssemblyKind::ExteriorInsulated => "exterior_insulated",
            AssemblyKind::InteriorPartition => "interior_partition",
            AssemblyKind::Structural => "structural",
            AssemblyKind::Retaining => "retaining",
        }
    }
    /// `(Arabic, English)`.
    pub fn label(self) -> (&'static str, &'static str) {
        match self {
            AssemblyKind::ExteriorInsulated => ("جدار خارجي معزول", "Exterior insulated wall"),
            AssemblyKind::InteriorPartition => ("حاجز داخلي", "Interior partition"),
            AssemblyKind::Structural => ("جدار قص إنشائي", "Structural shear wall"),
            AssemblyKind::Retaining => ("جدار استنادي", "Retaining wall"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WallAssembly {
    pub id: String,
    pub name: String,
    pub kind: AssemblyKind,
    /// From the outside (or the first face) to the inside.
    pub layers: Vec<LayerPick>,
    /// Where the layer values come from (the data sheet, the code page...).
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AssemblyDefaults {
    /// Assembly id used by exterior walls that name none.
    pub exterior: Option<String>,
    /// Assembly id used by interior walls that name none.
    pub interior: Option<String>,
}

impl WallAssembly {
    /// Sum of the layer thicknesses in whole millimetres. `Err` names the layer that has no usable thickness.
    pub fn total_thickness_mm(&self) -> Result<i64, String> {
        let mut sum = 0.0;
        for (i, l) in self.layers.iter().enumerate() {
            match l.thickness_mm {
                Some(t) if t.is_finite() && t > 0.0 => sum += t,
                _ => return Err(format!("assembly '{}': layer {} ('{}') needs a thickness above zero", self.id, i + 1, l.material)),
            }
        }
        let total = sum.round() as i64;
        if total < 1 {
            return Err(format!("assembly '{}' has no layers", self.id));
        }
        Ok(total)
    }

    /// The U-value `1 / (Rsi + sum(t / lambda) + Rse)` with the surface resistances of the project's thermal inputs.
    pub fn u_value(&self, t: &ThermalInputs) -> Result<f64, Vec<String>> {
        crate::thermal::assembly_u(&self.layers, t.rsi, t.rse, &format!("assembly '{}'", self.name))
    }
}

/// Everything wrong with the assemblies themselves.
pub fn check_all(list: &[WallAssembly], defaults: &AssemblyDefaults) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for a in list {
        let id = a.id.trim();
        if id.is_empty() || id.chars().count() > 40 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(format!("an assembly id is 1 to 40 letters, digits, '-' or '_' (got '{}')", a.id));
        }
        if !seen.insert(id.to_string()) {
            return Err(format!("two assemblies are named '{id}'"));
        }
        if a.name.trim().is_empty() {
            return Err(format!("assembly '{id}' needs a name"));
        }
        if a.layers.is_empty() {
            return Err(format!("assembly '{id}' needs at least one layer"));
        }
        for (i, l) in a.layers.iter().enumerate() {
            if l.material.trim().is_empty() {
                return Err(format!("assembly '{id}': layer {} needs a material", i + 1));
            }
            if l.lambda.is_some_and(|x| !x.is_finite() || x <= 0.0) {
                return Err(format!("assembly '{id}': the conductivity of layer {} must be above zero", i + 1));
            }
        }
        a.total_thickness_mm()?;
    }
    for (what, d, want) in [("exterior", &defaults.exterior, [AssemblyKind::ExteriorInsulated, AssemblyKind::Structural]), ("interior", &defaults.interior, [AssemblyKind::InteriorPartition, AssemblyKind::Structural])] {
        if let Some(id) = d {
            let a = list.iter().find(|a| &a.id == id).ok_or_else(|| format!("the default {what} assembly '{id}' does not exist"))?;
            if !want.contains(&a.kind) {
                return Err(format!("the default {what} assembly '{id}' is a {} and cannot be used for {what} walls", a.kind.label().1.to_lowercase()));
            }
        }
    }
    Ok(())
}

/// The project's assemblies, read once from the model.
#[derive(Debug, Clone, Default)]
pub struct Resolver {
    pub list: Vec<WallAssembly>,
    pub defaults: AssemblyDefaults,
}

impl Resolver {
    pub fn of(model: &Model) -> Resolver {
        match crate::project::get(model) {
            Ok(Some(p)) => Resolver { list: p.meta.assemblies, defaults: p.meta.assembly_defaults },
            _ => Resolver::default(),
        }
    }
    pub fn get(&self, id: &str) -> Option<&WallAssembly> {
        self.list.iter().find(|a| a.id == id)
    }
    /// The assembly a wall uses: the one it names, else the project's default for its side.
    pub fn of_wall(&self, e: &Element, w: &Wall) -> Option<&WallAssembly> {
        if let Some(id) = e.props.get("assembly").and_then(Value::as_str) {
            return self.get(id);
        }
        let d = if w.is_exterior { &self.defaults.exterior } else { &self.defaults.interior };
        d.as_deref().and_then(|id| self.get(id))
    }
}

/// Assigns `assembly` to the walls (or clears it with `None`) and sets each wall's thickness to the sum of the assembly's layers, so the
/// geometry and the layers cannot disagree. Opening placements in the wall are untouched: they are measured along it.
pub fn assign_commands(model: &Model, walls: &[Id], assembly: Option<&str>) -> Result<Vec<Command>, String> {
    let resolver = Resolver::of(model);
    let asm = match assembly {
        Some(id) => Some(resolver.get(id).ok_or_else(|| format!("there is no assembly '{id}'"))?),
        None => None,
    };
    let total = asm.map(WallAssembly::total_thickness_mm).transpose()?;
    let mut cmds = Vec::new();
    for id in walls {
        let e = model.get(id).ok_or("a wall in the selection no longer exists")?;
        let w = e.typed::<Wall>().map_err(|_| "assemblies apply to walls only".to_string())?;
        if let Some(a) = asm {
            match (a.kind, w.is_exterior) {
                (AssemblyKind::ExteriorInsulated, false) => return Err(format!("'{}' is an exterior wall type: wall {} is an interior wall", a.name, short(*id))),
                (AssemblyKind::InteriorPartition, true) => return Err(format!("'{}' is an interior partition: wall {} is an exterior wall", a.name, short(*id))),
                _ => {}
            }
        }
        cmds.push(Command::SetProp { id: *id, key: "assembly".into(), value: asm.map(|a| json!(a.id)) });
        if let Some(t) = total
            && w.thickness != t
        {
            cmds.push(Command::SetProp { id: *id, key: "thickness".into(), value: Some(json!(t)) });
        }
    }
    Ok(cmds)
}

/// The commands that set a wall's thickness to its explicit assembly's total again, for every wall whose assembly's layers changed.
pub fn retarget_commands(model: &Model, after: &[WallAssembly]) -> Result<Vec<Command>, String> {
    let mut cmds = Vec::new();
    for e in model.elements.values().filter(|e| e.kind == Wall::KIND) {
        let Some(id) = e.props.get("assembly").and_then(Value::as_str) else { continue };
        let Some(a) = after.iter().find(|a| a.id == id) else {
            return Err(format!("wall {} uses the assembly '{id}': clear it from the wall first", short(e.id)));
        };
        let t = a.total_thickness_mm()?;
        if e.props.get("thickness").and_then(Value::as_i64) != Some(t) {
            cmds.push(Command::SetProp { id: e.id, key: "thickness".into(), value: Some(json!(t)) });
        }
    }
    Ok(cmds)
}

fn short(id: Id) -> String {
    id.to_string().chars().take(8).collect()
}

/// Walls whose thickness differs from their assembly's total (the default one included): reported, never changed silently.
pub fn mismatches(model: &Model) -> Vec<(Id, i64, i64, String)> {
    let r = Resolver::of(model);
    let mut out = Vec::new();
    for e in model.elements.values().filter(|e| e.kind == Wall::KIND) {
        let Ok(w) = e.typed::<Wall>() else { continue };
        let Some(a) = r.of_wall(e, &w) else { continue };
        if let Ok(t) = a.total_thickness_mm()
            && t != w.thickness
        {
            out.push((e.id, w.thickness, t, a.name.clone()));
        }
    }
    out
}

/// The panel's data: the assemblies with their thickness and U-value (or what is missing), the defaults, the walls that use each and
/// the walls whose thickness disagrees with their assembly.
pub fn report(model: &Model) -> Value {
    let r = Resolver::of(model);
    let thermal = crate::project::get(model).ok().flatten().map(|p| p.meta.compliance.thermal).unwrap_or_default();
    let walls: Vec<(&Element, Wall)> = model.elements.values().filter(|e| e.kind == Wall::KIND).filter_map(|e| Some((e, e.typed::<Wall>().ok()?))).collect();
    let rows: Vec<Value> = r
        .list
        .iter()
        .map(|a| {
            let used: Vec<String> = walls.iter().filter(|(e, w)| r.of_wall(e, w).is_some_and(|x| x.id == a.id)).map(|(e, _)| e.id.to_string()).collect();
            let (u, missing) = match a.u_value(&thermal) {
                Ok(u) => (Some((u * 1000.0).round() / 1000.0), Vec::new()),
                Err(m) => (None, m),
            };
            json!({ "assembly": a, "kind_label": { "ar": a.kind.label().0, "en": a.kind.label().1 }, "thickness_mm": a.total_thickness_mm().ok(), "u": u, "missing": missing, "walls": used })
        })
        .collect();
    json!({
        "assemblies": rows,
        "defaults": r.defaults,
        "kinds": AssemblyKind::ALL.iter().map(|k| json!({ "id": k.id(), "ar": k.label().0, "en": k.label().1 })).collect::<Vec<_>>(),
        "mismatches": mismatches(model).into_iter().map(|(id, have, want, name)| json!({ "wall": id.to_string(), "thickness": have, "assembly_thickness": want, "assembly": name })).collect::<Vec<_>>(),
        "rsi_rse": { "rsi": thermal.rsi, "rse": thermal.rse },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(m: &str, t: Option<f64>) -> LayerPick {
        LayerPick { material: m.into(), thickness_mm: t, lambda: None, label: None }
    }

    fn asm(id: &str, kind: AssemblyKind) -> WallAssembly {
        WallAssembly { id: id.into(), name: format!("name {id}"), kind, layers: vec![layer("block_hollow_1216", Some(150.0)), layer("plaster_int_1100", Some(20.0))], source: None }
    }

    #[test]
    fn the_thickness_is_the_sum_of_the_layers_and_a_missing_thickness_is_named() {
        assert_eq!(asm("A", AssemblyKind::InteriorPartition).total_thickness_mm(), Ok(170));
        let mut a = asm("A", AssemblyKind::InteriorPartition);
        a.layers[1].thickness_mm = None;
        assert!(a.total_thickness_mm().unwrap_err().contains("layer 2"));
        a.layers[1].thickness_mm = Some(-5.0);
        assert!(a.total_thickness_mm().is_err());
    }

    #[test]
    fn the_checks_refuse_duplicate_ids_empty_assemblies_and_a_default_of_the_wrong_kind() {
        let d = AssemblyDefaults::default();
        assert!(check_all(&[asm("A", AssemblyKind::Structural), asm("A", AssemblyKind::Retaining)], &d).unwrap_err().contains("two assemblies"));
        let mut empty = asm("B", AssemblyKind::Structural);
        empty.layers.clear();
        assert!(check_all(&[empty], &d).unwrap_err().contains("at least one layer"));
        let list = [asm("P", AssemblyKind::InteriorPartition), asm("R", AssemblyKind::Retaining)];
        assert!(check_all(&list, &AssemblyDefaults { exterior: Some("P".into()), interior: None }).unwrap_err().contains("cannot be used for exterior"));
        assert!(check_all(&list, &AssemblyDefaults { exterior: Some("ZZ".into()), interior: None }).unwrap_err().contains("does not exist"));
        assert!(check_all(&list, &AssemblyDefaults { exterior: None, interior: Some("P".into()) }).is_ok());
    }

    #[test]
    fn the_u_value_follows_the_layers_and_names_what_is_missing() {
        let t = ThermalInputs { rsi: Some(0.12), rse: Some(0.03), ..Default::default() };
        let u = asm("P", AssemblyKind::InteriorPartition).u_value(&t).unwrap();
        let r: f64 = 0.12 + 0.03 + 0.150 / 1.353 + 0.020 / 0.7;
        assert!((u - 1.0 / r).abs() < 1e-9);
        assert!(asm("P", AssemblyKind::InteriorPartition).u_value(&ThermalInputs::default()).unwrap_err().iter().any(|m| m.contains("Rsi")));
    }
}
