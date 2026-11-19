//! The materials catalog: only what the code files give, with the page; every other material the brief names is listed WITHOUT
//! values and flagged `needs_user_values`, so choosing it asks you for its data and its source instead of inventing it.
//!
//! Read from the contractor's guide to SBC 1101-1102 (`presets::GUIDE_FILE`), a secondary source:
//! - page 195: thermal insulation boards (expanded and extruded polystyrene, polyurethane foam board, rock wool): density, thermal
//!   conductivity and the least insulation thickness per climate zone for the external-insulation system on a 20 cm block;
//! - page 199: the worked wall example: hollow cement block, exterior and interior plaster, and the surface resistances.
//!
//! NOT in the files, so not given: AAC and volcanic (SASO) insulated blocks, PIR boards, Low-E double or triple glazing, SABIC rebar
//! grades (including `fy`). Enter their values from the manufacturer's data sheet or the certificate.

use crate::presets::Source;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Material {
    pub id: &'static str,
    /// `insulation`, `masonry`, `plaster`, `glazing` or `rebar`.
    pub kind: &'static str,
    pub family_en: &'static str,
    pub family_ar: &'static str,
    pub name_en: &'static str,
    pub name_ar: &'static str,
    pub density_kg_m3: Option<f64>,
    /// Thermal conductivity, W/(m K).
    pub lambda: Option<f64>,
    /// Least insulation thickness in cm for zones 1, 2 and 3 (page 195 system only).
    pub min_thickness_cm: Option<[f64; 3]>,
    /// Thickness used in the guide's own example, mm (page 199).
    pub example_mm: Option<f64>,
    pub source: Option<Source>,
    pub needs_user_values: bool,
    pub note_en: &'static str,
}

const P195: Source = Source { what: "table of insulation materials: density, conductivity, least thickness by zone (external insulation on 20 cm block)", clause: "section 7A8.7 system tables", page: 195 };
const P199: Source = Source { what: "worked example of the U-value of an exterior wall", clause: "SBC 1102 section 38-3.5 as reproduced", page: 199 };

#[allow(clippy::too_many_arguments)]
const fn ins(id: &'static str, family_en: &'static str, family_ar: &'static str, name_en: &'static str, name_ar: &'static str, density: f64, lambda: f64, min: [f64; 3]) -> Material {
    Material { id, kind: "insulation", family_en, family_ar, name_en, name_ar, density_kg_m3: Some(density), lambda: Some(lambda), min_thickness_cm: Some(min), example_mm: None, source: Some(P195), needs_user_values: false, note_en: "least thickness applies to the guide's external-insulation system; check it for your system" }
}

const fn open(id: &'static str, kind: &'static str, family_en: &'static str, family_ar: &'static str, name_en: &'static str, name_ar: &'static str) -> Material {
    Material { id, kind, family_en, family_ar, name_en, name_ar, density_kg_m3: None, lambda: None, min_thickness_cm: None, example_mm: None, source: None, needs_user_values: true, note_en: "no value for this material in the code files: enter it from the data sheet or certificate, with its source" }
}

pub const CATALOG: [Material; 22] = [
    ins("eps_18", "Expanded polystyrene (EPS)", "بوليسترين ممدد", "EPS 18 kg/m3", "بوليسترين ممدد 18", 18.0, 0.038, [7.5, 7.0, 6.0]),
    ins("eps_23", "Expanded polystyrene (EPS)", "بوليسترين ممدد", "EPS 23 kg/m3", "بوليسترين ممدد 23", 23.0, 0.035, [7.0, 6.0, 5.5]),
    ins("eps_30", "Expanded polystyrene (EPS)", "بوليسترين ممدد", "EPS 30 kg/m3", "بوليسترين ممدد 30", 30.0, 0.030, [6.0, 5.5, 4.5]),
    ins("xps_25", "Extruded polystyrene (XPS)", "بوليسترين منبثق", "XPS 25 kg/m3", "بوليسترين منبثق 25", 25.0, 0.037, [7.5, 6.5, 5.5]),
    ins("xps_35", "Extruded polystyrene (XPS)", "بوليسترين منبثق", "XPS 35 kg/m3", "بوليسترين منبثق 35", 35.0, 0.027, [5.5, 5.0, 4.0]),
    ins("xps_42", "Extruded polystyrene (XPS)", "بوليسترين منبثق", "XPS 42 kg/m3", "بوليسترين منبثق 42", 42.0, 0.026, [5.5, 4.5, 4.0]),
    ins("pu_28", "Polyurethane foam board", "ألواح رغوة البولي يوريثان", "PU foam board 28 kg/m3", "رغوة بولي يوريثان 28", 28.0, 0.030, [6.0, 5.5, 4.5]),
    ins("pu_32", "Polyurethane foam board", "ألواح رغوة البولي يوريثان", "PU foam board 32 kg/m3", "رغوة بولي يوريثان 32", 32.0, 0.028, [6.0, 5.0, 4.5]),
    ins("pu_35", "Polyurethane foam board", "ألواح رغوة البولي يوريثان", "PU foam board 35 kg/m3", "رغوة بولي يوريثان 35", 35.0, 0.023, [5.0, 4.0, 3.5]),
    ins("rockwool_92", "Rock wool", "صوف صخري", "Rock wool 92 kg/m3", "صوف صخري 92", 92.0, 0.032, [6.5, 5.5, 5.0]),
    ins("rockwool_138", "Rock wool", "صوف صخري", "Rock wool 138 kg/m3", "صوف صخري 138", 138.0, 0.042, [8.5, 7.5, 6.5]),
    ins("rockwool_150", "Rock wool", "صوف صخري", "Rock wool 150 kg/m3", "صوف صخري 150", 150.0, 0.047, [9.5, 8.5, 7.0]),
    Material { id: "block_hollow_1216", kind: "masonry", family_en: "Cement block", family_ar: "بلوك أسمنتي", name_en: "Hollow cement block 1216 kg/m3", name_ar: "بلوك أسمنتي مفرغ 1216", density_kg_m3: Some(1216.0), lambda: Some(1.353), min_thickness_cm: None, example_mm: Some(100.0), source: Some(P199), needs_user_values: false, note_en: "the guide's example uses 100 mm (outer leaf) and 150 mm (inner leaf)" },
    Material { id: "plaster_ext_1856", kind: "plaster", family_en: "Plaster", family_ar: "لياسة", name_en: "Exterior profile plaster 1856 kg/m3", name_ar: "لياسة خارجية بروفيل 1856", density_kg_m3: Some(1856.0), lambda: Some(0.75), min_thickness_cm: None, example_mm: Some(30.0), source: Some(P199), needs_user_values: false, note_en: "" },
    Material { id: "plaster_int_1100", kind: "plaster", family_en: "Plaster", family_ar: "لياسة", name_en: "Interior decorative plaster 1100 kg/m3", name_ar: "لياسة داخلية ديكور 1100", density_kg_m3: Some(1100.0), lambda: Some(0.7), min_thickness_cm: None, example_mm: Some(20.0), source: Some(P199), needs_user_values: false, note_en: "" },
    open("aac_block", "masonry", "Cement block", "بلوك أسمنتي", "Autoclaved aerated concrete (AAC) block", "بلوك خرسانة مهوّاة (AAC)"),
    open("volcanic_block_saso", "masonry", "Cement block", "بلوك أسمنتي", "Insulated volcanic block (SASO)", "بلوك بركاني معزول (ساسو)"),
    open("pir_board", "insulation", "Rigid foam", "ألواح عازلة", "PIR board", "ألواح PIR"),
    open("glass_low_e_double", "glazing", "Glazing", "زجاج", "Double Low-E glazing", "زجاج مزدوج Low-E"),
    open("glass_low_e_triple", "glazing", "Glazing", "زجاج", "Triple Low-E glazing", "زجاج ثلاثي Low-E"),
    open("rebar_sabic_g60", "rebar", "Reinforcing steel", "حديد التسليح", "SABIC rebar, grade 60", "حديد سابك درجة 60"),
    open("rebar_sabic_g40", "rebar", "Reinforcing steel", "حديد التسليح", "SABIC rebar, grade 40", "حديد سابك درجة 40"),
];

/// Surface resistances of the guide's worked example (page 199): inside 0.120, outside 0.03 m2 K/W, at 24 C inside and 50 C outside.
pub const RSI_EXAMPLE: f64 = 0.120;
pub const RSE_EXAMPLE: f64 = 0.03;
pub const FILM_SOURCE: Source = P199;

pub fn find(id: &str) -> Option<&'static Material> {
    CATALOG.iter().find(|m| m.id == id)
}

pub fn catalog_json() -> serde_json::Value {
    serde_json::json!({
        "materials": CATALOG.iter().map(|m| serde_json::json!({
            "id": m.id, "kind": m.kind, "family_en": m.family_en, "family_ar": m.family_ar, "name_en": m.name_en, "name_ar": m.name_ar,
            "density": m.density_kg_m3, "lambda": m.lambda, "min_thickness_cm": m.min_thickness_cm, "example_mm": m.example_mm,
            "needs_user_values": m.needs_user_values, "note": m.note_en, "source": m.source.map(|s| s.text()),
        })).collect::<Vec<_>>(),
        "rsi_example": RSI_EXAMPLE, "rse_example": RSE_EXAMPLE, "film_source": FILM_SOURCE.text(),
    })
}
