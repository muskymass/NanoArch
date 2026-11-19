//! One-click starter projects: a plot, a style, a building type and a room program, ready for "Generate".
//!
//! What a template is: a STARTING POINT of this application, not a design and not a code value.
//! - The plot size, the number of floors, the room areas and the street width are assumptions stated in `assumptions`; the user
//!   edits any of them.
//! - The setbacks are NOT chosen here: `setbacks` runs the plot through `site::analyse` with the Ministry guideline rule of the
//!   template (villa clause 4-1, apartment clause 4-2), so each value carries its clause and page.
//! - The room areas come from `briefai::suggested_area_m2` unless a larger target is stated; every area is above the SBC minimum
//!   for its type (checked by a test against `sbc::code_value`).

use crate::briefai::suggested_area_m2;
use crate::site::{self, Edge, EdgeKind, RuleSet, SiteInput};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct TplRoom {
    pub key: String,
    pub name: String,
    pub func: &'static str,
    pub area_m2: f64,
    /// `public`, `private` or `service`, as the planner groups rooms.
    pub zone: &'static str,
    pub floor: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Template {
    pub id: &'static str,
    pub title_ar: &'static str,
    pub title_en: &'static str,
    pub project_title: &'static str,
    pub building_type: &'static str,
    pub style: &'static str,
    pub city: &'static str,
    pub plot_width_m: f64,
    pub plot_length_m: f64,
    pub floors: u32,
    pub rule: RuleSet,
    /// Width of the street at the front of the plot: an assumption, edit it.
    pub street_width_m: f64,
    /// `(width, length, side)` of the stair shaft when there is more than one floor.
    pub stair: Option<(f64, f64, &'static str)>,
    pub rooms: Vec<TplRoom>,
    pub assumptions: Vec<String>,
}

fn zone_of(func: &str) -> &'static str {
    match func {
        "majlis" | "living" | "dining" => "public",
        "bedroom" => "private",
        _ => "service",
    }
}

fn room(key: &str, name: &str, func: &'static str, area: Option<f64>, floor: u32) -> TplRoom {
    TplRoom { key: key.into(), name: name.into(), func, area_m2: area.unwrap_or_else(|| suggested_area_m2(func)), zone: zone_of(func), floor }
}

/// `n` bedrooms and `b` bathrooms on one floor, the first bedroom a master.
fn sleeping_floor(floor: u32, bedrooms: u32, bathrooms: u32, master: f64, other: f64, bath: f64) -> Vec<TplRoom> {
    let mut v = vec![room(&format!("f{floor}master"), "الماستر", "bedroom", Some(master), floor)];
    for i in 1..bedrooms {
        v.push(room(&format!("f{floor}bed{i}"), &format!("غرفة {i}"), "bedroom", Some(other), floor));
    }
    for i in 1..=bathrooms {
        v.push(room(&format!("f{floor}bath{i}"), &format!("حمام {i}"), "bathroom", Some(bath), floor));
    }
    v
}

pub fn all() -> Vec<Template> {
    let villa = {
        let mut rooms = vec![room("majlis", "المجلس", "majlis", None, 1), room("living", "الصالة", "living", None, 1), room("dining", "غرفة الطعام", "dining", None, 1), room("kitchen", "المطبخ", "kitchen", None, 1), room("bath0", "حمام الضيوف", "bathroom", None, 1)];
        rooms.extend(sleeping_floor(2, 4, 3, 24.0, 16.0, 6.0));
        Template {
            id: "villa_jubail",
            title_ar: "فيلا سكنية - الجبيل (600 م² - طراز نجدي حديث)",
            title_en: "Villa - Jubail (600 m² - modern Najdi)",
            project_title: "فيلا سكنية - الجبيل",
            building_type: "villa",
            style: "modern_najdi",
            city: "Jubail",
            plot_width_m: 20.0,
            plot_length_m: 30.0,
            floors: 2,
            rule: RuleSet::MinistryVilla,
            street_width_m: 15.0,
            stair: Some((2.5, 5.0, "west")),
            rooms,
            assumptions: vec![],
        }
    };
    let palace = {
        let mut rooms = vec![
            room("majlis1", "مجلس الرجال", "majlis", Some(45.0), 1),
            room("majlis2", "مجلس النساء", "majlis", Some(40.0), 1),
            room("living", "الصالة", "living", Some(40.0), 1),
            room("dining", "غرفة الطعام", "dining", Some(30.0), 1),
            room("kitchen", "المطبخ", "kitchen", Some(28.0), 1),
            room("bath0", "حمام الضيوف", "bathroom", Some(6.0), 1),
        ];
        rooms.extend(sleeping_floor(2, 6, 4, 42.0, 24.0, 8.0));
        Template {
            id: "palace",
            title_ar: "قصر سكني (1,500 م² - كلاسيكي حديث)",
            title_en: "Residential palace (1,500 m² - neo-classical)",
            project_title: "قصر سكني",
            building_type: "palace",
            style: "neo_classical",
            city: "Riyadh",
            plot_width_m: 30.0,
            plot_length_m: 50.0,
            floors: 2,
            rule: RuleSet::MinistryVilla,
            street_width_m: 20.0,
            stair: Some((3.0, 6.0, "west")),
            rooms,
            assumptions: vec!["the Ministry villa guideline (clause 4-1) is applied because the guidelines give no separate rule for a palace: confirm with the municipality".into()],
        }
    };
    let apartments = {
        let mut rooms = Vec::new();
        for f in 1..=4 {
            rooms.push(room(&format!("f{f}living"), "الصالة", "living", Some(30.0), f));
            rooms.push(room(&format!("f{f}kitchen"), "المطبخ", "kitchen", None, f));
            rooms.extend(sleeping_floor(f, 3, 2, 20.0, 16.0, 5.0));
        }
        Template {
            id: "apartments_4",
            title_ar: "عمارة سكنية 4 أدوار (900 م² - معاصر)",
            title_en: "Apartment building, 4 floors (900 m² - contemporary)",
            project_title: "عمارة سكنية 4 أدوار",
            building_type: "apartment_building",
            style: "contemporary",
            city: "Riyadh",
            plot_width_m: 30.0,
            plot_length_m: 30.0,
            floors: 4,
            rule: RuleSet::MinistryApartment,
            street_width_m: 15.0,
            stair: Some((2.5, 6.0, "west")),
            rooms,
            assumptions: vec!["one apartment per floor".into()],
        }
    };
    let mut v = vec![villa, palace, apartments];
    for t in &mut v {
        t.assumptions.insert(0, format!("plot {} x {} m, {} floor(s), street {} m at the front: assumptions, edit them", t.plot_width_m, t.plot_length_m, t.floors, t.street_width_m));
        t.assumptions.push("room areas are starting targets of this application, each above the SBC minimum for its type: review them".into());
    }
    v
}

pub fn find(id: &str) -> Option<Template> {
    all().into_iter().find(|t| t.id == id)
}

/// The Ministry-rule setbacks of the template's plot, `[front, side, rear]`, with the clause each came from.
pub fn setbacks(t: &Template) -> Result<([f64; 3], Vec<String>), Vec<String>> {
    let edge = |kind, w: Option<f64>| Edge { kind, street_width_m: w, setback_m: None };
    let input = SiteInput {
        polygon_m: vec![[0.0, 0.0], [t.plot_width_m, 0.0], [t.plot_width_m, t.plot_length_m], [0.0, t.plot_length_m]],
        edges: vec![edge(EdgeKind::FrontStreet, Some(t.street_width_m)), edge(EdgeKind::Neighbour, None), edge(EdgeKind::Neighbour, None), edge(EdgeKind::Neighbour, None)],
        rule: t.rule,
        floors: Some(t.floors),
    };
    let r = site::analyse(&input)?;
    let (right, left) = (r.setbacks_m[1], r.setbacks_m[3]);
    if (right - left).abs() > 1e-9 {
        return Err(vec![format!("the two side setbacks differ ({right} and {left} m)")]);
    }
    Ok(([r.setbacks_m[0], right, r.setbacks_m[2]], r.setback_sources))
}
