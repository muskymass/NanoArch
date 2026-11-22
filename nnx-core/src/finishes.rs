//! Finishes: the facade finish and the interior floor finish the project uses, shown in 3D and carried into the quantities and the BOQ.
//!
//! - The catalog holds NAMES and a display COLOUR (a visual choice, not a material property) and a BOQ code; it holds no price,
//!   thickness or performance value. The price of a finish is the rate YOU enter for its code in the BOQ library (`items`), so changing
//!   the finish changes the BOQ row that is priced.
//! - The selection is stored in the project (`meta.finishes`): undo, redo, dirty and the `.nnx` file follow.
//! - Quantities: the facade finish covers the exterior walls net of their openings; the floor finish covers every room except
//!   bathrooms and toilets (wet areas have their own finish). They appear in the take-off as `FIN.facade.<id>` and `FIN.floor.<id>`.

use crate::boq::BoqLib;
use crate::model::{Id, Model};
use crate::qto::{QtoItem, Unit};
use crate::typed::{Space, Typed};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Finish {
    pub id: &'static str,
    /// `facade` or `floor`.
    pub kind: &'static str,
    pub en: &'static str,
    pub ar: &'static str,
    /// Display colour, 0 to 1.
    pub color: [f64; 3],
}

pub const CATALOG: [Finish; 8] = [
    Finish { id: "riyadh_stone", kind: "facade", en: "Riyadh stone", ar: "حجر الرياض", color: [0.80, 0.66, 0.48] },
    Finish { id: "travertine", kind: "facade", en: "Travertine", ar: "ترافرتين", color: [0.88, 0.82, 0.70] },
    Finish { id: "profile_plaster", kind: "facade", en: "Profile plaster", ar: "دهان بروفيل", color: [0.93, 0.92, 0.88] },
    Finish { id: "wood_cladding", kind: "facade", en: "Wood cladding", ar: "كسوة خشب", color: [0.62, 0.43, 0.26] },
    Finish { id: "porcelain", kind: "floor", en: "Porcelain tiles", ar: "بورسلان", color: [0.85, 0.85, 0.83] },
    Finish { id: "marble", kind: "floor", en: "Marble", ar: "رخام", color: [0.93, 0.92, 0.90] },
    Finish { id: "parquet", kind: "floor", en: "Wood parquet", ar: "باركيه خشب", color: [0.68, 0.50, 0.32] },
    Finish { id: "polished_concrete", kind: "floor", en: "Polished concrete", ar: "خرسانة مصقولة", color: [0.60, 0.60, 0.62] },
];

pub fn find(kind: &str, id: &str) -> Option<&'static Finish> {
    CATALOG.iter().find(|f| f.kind == kind && f.id == id)
}

pub fn code(kind: &str, id: &str) -> String {
    format!("FIN.{kind}.{id}")
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Finishes {
    pub facade: Option<String>,
    pub floor: Option<String>,
}

impl Finishes {
    pub fn check(&self) -> Result<(), String> {
        if let Some(f) = &self.facade
            && find("facade", f).is_none()
        {
            return Err(format!("unknown facade finish '{f}'"));
        }
        if let Some(f) = &self.floor
            && find("floor", f).is_none()
        {
            return Err(format!("unknown floor finish '{f}'"));
        }
        Ok(())
    }
}

fn floor_spaces(model: &Model) -> (f64, Vec<Id>) {
    let v: Vec<(Id, Space)> = model.elements.values().filter(|e| e.kind == Space::KIND).filter_map(|e| Some((e.id, e.typed::<Space>().ok()?))).filter(|(_, s)| s.func != "bathroom" && s.func != "toilet").collect();
    (v.iter().map(|(_, s)| s.w as f64 * s.h as f64 / 1e6).sum(), v.iter().map(|(id, _)| *id).collect())
}

/// The take-off items of the chosen finishes.
pub fn items(model: &Model, f: &Finishes) -> Vec<QtoItem> {
    items_for_phase(model, f, None)
}

pub fn items_for_phase(model: &Model, f: &Finishes, phase: Option<&str>) -> Vec<QtoItem> {
    let mut out = Vec::new();
    let round = |v: f64| (v * 1e4).round() / 1e4;
    if let Some(id) = f.facade.as_deref().and_then(|i| find("facade", i)) {
        let (area, ids) = if let Some(phase) = phase {
            let parts: Vec<_> = crate::insulation::exterior_wall_areas(model).into_iter()
                .filter(|(id, _)| model.elements.get(id).is_some_and(|e| crate::qto::matches_phase(e, Some(phase)))).collect();
            (parts.iter().map(|(_, a)| *a).sum(), parts.iter().map(|(id, _)| *id).collect())
        } else { crate::insulation::scope_area(model, "wall", None) };
        if area > 0.0 {
            out.push(QtoItem { code: code("facade", id.id), description: format!("Facade finish: {}", id.en), unit: Unit::M2, quantity: round(area), waste_pct: None, sources: ids, parts: Vec::new() });
        }
    }
    if let Some(id) = f.floor.as_deref().and_then(|i| find("floor", i)) {
        let (area, ids) = if let Some(phase) = phase {
            let parts: Vec<_> = model.elements.values().filter(|e| e.kind == Space::KIND && crate::qto::matches_phase(e, Some(phase)))
                .filter_map(|e| Some((e.id, e.typed::<Space>().ok()?)))
                .filter(|(_, s)| s.func != "bathroom" && s.func != "toilet")
                .collect();
            (parts.iter().map(|(_, s)| s.w as f64 * s.h as f64 / 1e6).sum(), parts.iter().map(|(id, _)| *id).collect())
        } else { floor_spaces(model) };
        if area > 0.0 {
            out.push(QtoItem { code: code("floor", id.id), description: format!("Floor finish: {}", id.en), unit: Unit::M2, quantity: round(area), waste_pct: None, sources: ids, parts: Vec::new() });
        }
    }
    out
}

#[derive(Debug, Clone, Serialize)]
pub struct FinishLine {
    pub kind: &'static str,
    pub id: &'static str,
    pub en: &'static str,
    pub ar: &'static str,
    pub code: String,
    pub area_m2: f64,
    /// The rate you entered for this code in the BOQ library (none is built in).
    pub rate: Option<f64>,
    pub amount: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct FinishReport {
    pub lines: Vec<FinishLine>,
    pub total: Option<f64>,
    pub currency: Option<String>,
    pub notes: Vec<String>,
}

pub fn report(model: &Model, f: &Finishes, boq: Option<&BoqLib>) -> FinishReport {
    let mut rep = FinishReport { currency: boq.and_then(|b| b.project.currency.clone()), ..Default::default() };
    for it in items(model, f) {
        let (kind, id) = it.code.strip_prefix("FIN.").and_then(|r| r.split_once('.')).unwrap_or(("", ""));
        let Some(fin) = find(kind, id) else { continue };
        let rate = boq.and_then(|b| b.items.get(&it.code)).and_then(|d| d.rate);
        rep.lines.push(FinishLine { kind: fin.kind, id: fin.id, en: fin.en, ar: fin.ar, code: it.code.clone(), area_m2: it.quantity, rate, amount: rate.map(|r| (r * it.quantity * 100.0).round() / 100.0) });
    }
    if !rep.lines.is_empty() && rep.lines.iter().all(|l| l.amount.is_some()) {
        rep.total = Some((rep.lines.iter().filter_map(|l| l.amount).sum::<f64>() * 100.0).round() / 100.0);
    }
    for l in rep.lines.iter().filter(|l| l.rate.is_none()) {
        rep.notes.push(format!("no rate for {}: enter one for the code {} in the BOQ library", l.en, l.code));
    }
    if f.facade.is_none() && f.floor.is_none() {
        rep.notes.push("no finish chosen yet".into());
    }
    rep
}

/// Sets the rate of a finish code in the BOQ library (creating the entry and the library if needed).
pub fn set_rate(lib: &mut Option<BoqLib>, code: &str, rate: f64) -> Result<(), String> {
    if !rate.is_finite() || rate < 0.0 {
        return Err("a rate must be zero or more".into());
    }
    let valid = code.strip_prefix("FIN.").and_then(|r| r.split_once('.')).is_some_and(|(k, i)| find(k, i).is_some());
    if !valid {
        return Err(format!("'{code}' is not a finish code"));
    }
    lib.get_or_insert_with(Default::default).items.entry(code.to_string()).or_default().rate = Some(rate);
    Ok(())
}
