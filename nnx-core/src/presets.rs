//! Engineering reference presets read from the code files in `C:\Projects\البناء\الكود السعودي\`.
//!
//! What is here was READ from those files, with the page; what is not in them is not here:
//! - The files contain no SBC 602 and no SBC 304 text. Every value below comes from the contractor's guide to SBC 1101-1102
//!   (first edition, March 2023), which reproduces parts of SBC 1101 Chapter 4A (concrete) and SBC 602 Table 5.2 (insulation).
//!   They are therefore SECONDARY sources: check them against the code text before relying on them.
//! - NOT FOUND in the files, so not given and left to the user: exterior design temperatures of the cities, the yield
//!   strength `fy`, minimum and maximum reinforcement ratios, load factors and strength-reduction factors. The rebar
//!   design in `rebar` takes all of them as inputs and names the missing ones.
//!
//! Page numbers are physical PDF pages of the guide (they equal the printed numbers).

use serde::Serialize;

pub const GUIDE_FILE: &str = "الدليل الاسترشادي للكود السعودي للمباني السكنية SBC 1101-1102 للمقاولين.pdf";
pub const GUIDE_NOTE: &str = "Contractor's guide to SBC 1101-1102, 1st edition, March 2023 (secondary source: SBC 602 and SBC 304 themselves are not in the folder)";

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Source {
    pub what: &'static str,
    pub clause: &'static str,
    pub page: u32,
}

impl Source {
    pub fn text(&self) -> String {
        format!("{}, {} {}, p.{}", GUIDE_NOTE, self.what, self.clause, self.page)
    }
}

/// Insulation limits of a climate zone (SBC 602 Table 5.2 as reproduced in the guide).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ZonePreset {
    pub zone: u8,
    /// How the zone is defined: cooling degree-hours at 10 C.
    pub cdd_rule_en: &'static str,
    pub cdd_rule_ar: &'static str,
    /// Maximum U-value, W/m2.K.
    pub wall_u_max: f64,
    pub roof_u_max: f64,
    /// Minimum thermal resistance of continuous insulation, m2.K/W.
    pub wall_r_min: f64,
    pub roof_r_min: f64,
    /// Vertical glazing, 0-25 % of the wall: maximum U (W/m2.K) and SHGC. The same in all zones.
    pub window_u_max: f64,
    pub window_shgc_max: f64,
    pub source: Source,
}

const U_TABLE: Source = Source { what: "Tables 1 and 2 (SBC 602 Table 5.2)", clause: "section 7A8.7", page: 172 };

/// Zone 1: more than 5000 cooling degree-hours; zone 2: more than 3500 up to 5000; zone 3: 3500 or less.
pub const ZONES: [ZonePreset; 3] = [
    ZonePreset { zone: 1, cdd_rule_en: "cooling degree-hours at 10 C above 5000 h", cdd_rule_ar: "درجات التبريد عند 10 م° أكبر من 5000 ساعة", wall_u_max: 0.403, roof_u_max: 0.272, wall_r_min: 2.36, roof_r_min: 3.54, window_u_max: 2.668, window_shgc_max: 0.25, source: U_TABLE },
    ZonePreset { zone: 2, cdd_rule_en: "cooling degree-hours at 10 C above 3500 h and up to 5000 h", cdd_rule_ar: "درجات التبريد عند 10 م° أقل من أو يساوي 5000 وأكبر من 3500 ساعة", wall_u_max: 0.454, roof_u_max: 0.34, wall_r_min: 2.06, roof_r_min: 2.8, window_u_max: 2.668, window_shgc_max: 0.25, source: U_TABLE },
    ZonePreset { zone: 3, cdd_rule_en: "cooling degree-hours at 10 C of 3500 h or less", cdd_rule_ar: "درجات التبريد عند 10 م° أقل من أو يساوي 3500 ساعة", wall_u_max: 0.511, roof_u_max: 0.34, wall_r_min: 1.81, roof_r_min: 2.38, window_u_max: 2.668, window_shgc_max: 0.25, source: U_TABLE },
];

/// What the guide says about the columns of its Table 1 and the reproduction of Table 5.2, which the application cannot settle:
/// the table reproduced on p.173 labels the zone 1 values 0.403 / 0.272 as "Residential UnConditioned", while Tables 1 and 2 on
/// p.172 do not say which occupancy they apply to. Check against SBC 602 Table 5.2 itself.
pub const ZONE_CAVEAT_EN: &str = "The guide's own column labels conflict (it labels the zone 1 values 0.403 / 0.272 as unconditioned on p.173 while Tables 1-2 on p.172 do not say): check SBC 602 Table 5.2.";
pub const ZONE_CAVEAT_AR: &str = "تسميات أعمدة الدليل متعارضة (يسمّي القيم 0.403 / 0.272 للمنطقة 1 «غير مكيّفة» في ص173 بينما لا يحدد الجدولان 1 و2 في ص172): راجع SBC 602 الجدول 5.2 نفسه.";

/// Glazing bands of the same table (p.174): maximum U by the share of glazing in the wall. Skylights: U 4.259, SHGC 0.35.
pub const GLAZING_BANDS: [(&str, f64); 3] = [("0-25 % of wall", 2.668), (">25-50 % of wall", 2.38), (">50-100 % of wall", 1.87)];

pub fn zone(z: u8) -> Option<&'static ZonePreset> {
    ZONES.iter().find(|p| p.zone == z)
}

/// A city and the zone the guide's map (figure 1/7-11, p.170) shows it in.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct CityZone {
    pub en: &'static str,
    pub ar: &'static str,
    pub zone: u8,
    pub note: &'static str,
}

const MAP: &str = "named on the guide's map, p.170";

pub const CITIES: [CityZone; 27] = [
    CityZone { en: "Riyadh", ar: "الرياض", zone: 1, note: MAP },
    CityZone { en: "Dammam (King Fahd Airport)", ar: "الدمام (مطار الملك فهد)", zone: 1, note: MAP },
    CityZone { en: "Dhahran", ar: "الظهران", zone: 1, note: MAP },
    CityZone { en: "Al-Ahsa", ar: "الأحساء", zone: 1, note: MAP },
    CityZone { en: "Jubail", ar: "الجبيل", zone: 1, note: "not named on the map: it lies in the zone 1 area of the Eastern Province coast beside Dammam and Dhahran, so confirm with the municipality" },
    CityZone { en: "Qaisumah", ar: "القيصومة", zone: 1, note: MAP },
    CityZone { en: "Madinah", ar: "المدينة المنورة", zone: 1, note: MAP },
    CityZone { en: "Yanbu", ar: "ينبع", zone: 1, note: MAP },
    CityZone { en: "Makkah", ar: "مكة المكرمة", zone: 1, note: MAP },
    CityZone { en: "Jeddah", ar: "جدة", zone: 1, note: MAP },
    CityZone { en: "Bisha", ar: "بيشة", zone: 1, note: MAP },
    CityZone { en: "Wadi Al-Dawasir", ar: "وادي الدواسر", zone: 1, note: MAP },
    CityZone { en: "Sharurah", ar: "شرورة", zone: 1, note: MAP },
    CityZone { en: "Najran", ar: "نجران", zone: 1, note: MAP },
    CityZone { en: "Jazan", ar: "جيزان", zone: 1, note: MAP },
    CityZone { en: "Qurayyat", ar: "القريات", zone: 2, note: MAP },
    CityZone { en: "Turaif", ar: "طريف", zone: 2, note: MAP },
    CityZone { en: "Arar", ar: "عرعر", zone: 2, note: MAP },
    CityZone { en: "Rafha", ar: "رفحاء", zone: 2, note: MAP },
    CityZone { en: "Al-Jouf", ar: "الجوف", zone: 2, note: MAP },
    CityZone { en: "Tabuk", ar: "تبوك", zone: 2, note: MAP },
    CityZone { en: "Al-Wajh", ar: "الوجه", zone: 2, note: MAP },
    CityZone { en: "Hail", ar: "حائل", zone: 2, note: MAP },
    CityZone { en: "Taif", ar: "الطائف", zone: 2, note: MAP },
    CityZone { en: "Al-Baha", ar: "الباحة", zone: 2, note: MAP },
    CityZone { en: "Khamis Mushait", ar: "خميس مشيط", zone: 2, note: MAP },
    CityZone { en: "Abha", ar: "أبها", zone: 3, note: "named on the map; its label is partly overlapped by the zone 3 marker, so check it" },
];

/// The zone a free-text entry names: `1`, `zone 2`, `المنطقة 3`, or a city of the table (English or Arabic, any case).
pub fn zone_of(text: &str) -> Option<u8> {
    let t = text.trim().to_lowercase();
    if t.is_empty() {
        return None;
    }
    if let Some(c) = CITIES.iter().find(|c| c.en.to_lowercase() == t || c.ar == t) {
        return Some(c.zone);
    }
    let digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
    let letters: String = t.chars().filter(|c| !c.is_ascii_digit() && !c.is_whitespace() && *c != ':' && *c != '-').collect();
    let named = letters.is_empty() || letters == "zone" || letters == "المنطقة" || letters == "منطقة";
    match (named, digits.parse::<u8>()) {
        (true, Ok(z)) if zone(z).is_some() => Some(z),
        _ => None,
    }
}

// ------------------------------------------------------------------ concrete

/// An exposure class of SBC 1101 Table 4A2 (p.132): the least `f'c` and the greatest water-to-cementitious ratio.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ExposureClass {
    pub class: &'static str,
    pub min_fc_mpa: f64,
    pub max_wcm: Option<f64>,
}

pub const EXPOSURE_CLASSES: [ExposureClass; 10] = [
    ExposureClass { class: "S0", min_fc_mpa: 20.0, max_wcm: None },
    ExposureClass { class: "S1", min_fc_mpa: 28.0, max_wcm: Some(0.50) },
    ExposureClass { class: "S2", min_fc_mpa: 31.0, max_wcm: Some(0.45) },
    ExposureClass { class: "S3", min_fc_mpa: 31.0, max_wcm: Some(0.45) },
    ExposureClass { class: "C0", min_fc_mpa: 20.0, max_wcm: None },
    ExposureClass { class: "C1", min_fc_mpa: 28.0, max_wcm: Some(0.50) },
    ExposureClass { class: "C2", min_fc_mpa: 28.0, max_wcm: Some(0.50) },
    ExposureClass { class: "C3", min_fc_mpa: 31.0, max_wcm: Some(0.45) },
    ExposureClass { class: "C4", min_fc_mpa: 35.0, max_wcm: Some(0.40) },
    ExposureClass { class: "C5", min_fc_mpa: 40.0, max_wcm: Some(0.35) },
];

pub const EXPOSURE_SOURCE: Source = Source { what: "Table 4A2, mix requirements by exposure class", clause: "SBC 1101 chapter 4A", page: 132 };

/// Least specified compressive strength of normal-weight structural concrete, MPa (SBC 1101 chapter 4A, p.131).
pub const FC_MIN_MPA: f64 = 20.0;
pub const FC_SOURCE: Source = Source { what: "specified compressive strength of normal-weight concrete", clause: "SBC 1101 chapter 4A", page: 131 };

pub fn exposure_class(name: &str) -> Option<&'static ExposureClass> {
    EXPOSURE_CLASSES.iter().find(|c| c.class.eq_ignore_ascii_case(name.trim()))
}

/// How a member is exposed, for the concrete cover of Table 4A5 (p.134).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Exposure {
    /// Cast against and permanently in contact with the ground.
    Ground,
    /// Exposed to the weather or in contact with the ground (not cast against it).
    Weather,
    /// Not exposed to the weather and not in contact with the ground.
    Protected,
}

/// What a member is, for the cover rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Member {
    /// Slabs, joists and walls.
    Slab,
    /// Beams, columns, pedestals and tension ties.
    Frame,
}

pub const COVER_SOURCE: Source = Source { what: "Table 4A5, specified concrete cover for cast-in-place members", clause: "SBC 1101 chapter 4A", page: 134 };

/// Specified cover in mm for a member with bars of `bar_dia_mm`, or `None` where the table gives none (a slab bar above Dia 25 not
/// exposed). Cast against ground 75; exposed to weather or ground 50 for Dia 18 to 32 bars and 40 for Dia 16 and smaller; not
/// exposed: slabs, joists and walls with Dia 25 and smaller 20, beams, columns, pedestals and ties 40 (the table marks this value
/// with an asterisk whose note is not on the page).
pub fn cover_mm(exposure: Exposure, member: Member, bar_dia_mm: f64) -> Option<f64> {
    match (exposure, member) {
        (Exposure::Ground, _) => Some(75.0),
        (Exposure::Weather, _) => Some(if bar_dia_mm >= 18.0 { 50.0 } else { 40.0 }),
        (Exposure::Protected, Member::Frame) => Some(40.0),
        (Exposure::Protected, Member::Slab) => (bar_dia_mm <= 25.0).then_some(20.0),
    }
}

// ------------------------------------------------------------------ the page's view

/// Everything above as JSON for the page.
pub fn presets_json() -> serde_json::Value {
    serde_json::json!({
        "zones": ZONES.iter().map(|z| serde_json::json!({
            "zone": z.zone, "cdd_rule_en": z.cdd_rule_en, "cdd_rule_ar": z.cdd_rule_ar,
            "wall_u_max": z.wall_u_max, "roof_u_max": z.roof_u_max, "wall_r_min": z.wall_r_min, "roof_r_min": z.roof_r_min,
            "window_u_max": z.window_u_max, "window_shgc_max": z.window_shgc_max, "outdoor_design_c": null,
            "source": z.source.text(),
        })).collect::<Vec<_>>(),
        "zone_caveat_en": ZONE_CAVEAT_EN, "zone_caveat_ar": ZONE_CAVEAT_AR,
        "design_temperature_note": "Exterior design temperatures are not in the code files: enter them (hvac.outdoor_c) from SBC 602 or the project criteria.",
        "design_temperature_note_ar": "درجات الحرارة التصميمية الخارجية غير موجودة في ملفات الكود: أدخلها (hvac.outdoor_c) من SBC 602 أو من معايير المشروع.",
        "cities": CITIES,
        "glazing_bands": GLAZING_BANDS.iter().map(|(b, u)| serde_json::json!({ "band": b, "u_max": u })).collect::<Vec<_>>(),
        "concrete": {
            "fc_min_mpa": FC_MIN_MPA, "fc_source": FC_SOURCE.text(),
            "exposure_classes": EXPOSURE_CLASSES, "exposure_source": EXPOSURE_SOURCE.text(),
            "cover_source": COVER_SOURCE.text(),
            "cover_rows": [
                { "exposure": "ground", "member": "all", "bars": "all", "cover_mm": 75 },
                { "exposure": "weather", "member": "all", "bars": "Dia 18 to 32", "cover_mm": 50 },
                { "exposure": "weather", "member": "all", "bars": "Dia 16 and smaller", "cover_mm": 40 },
                { "exposure": "protected", "member": "slabs, joists, walls", "bars": "Dia 25 and smaller", "cover_mm": 20 },
                { "exposure": "protected", "member": "beams, columns, pedestals, ties", "bars": "primary reinforcement, stirrups, ties, spirals, hoops", "cover_mm": 40 },
            ],
            "not_found": ["fy (yield strength of the bars)", "minimum and maximum reinforcement ratios", "load factors", "strength reduction factors"],
        },
    })
}
