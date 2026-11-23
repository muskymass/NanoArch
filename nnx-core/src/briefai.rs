#![allow(dead_code)]
//! From a client's description to the fields of the brief. Two sources, one result:
//!
//! - **Keywords** (`from_keywords`): Arabic and English words for the building type, the number of floors, an annex, the city,
//!   the style, the plot area or size, and the rooms named. It works offline and does exactly what it says: only what the text
//!   states is filled.
//! - **A local language model** (`from_model_json`): the answer of an Ollama model on this computer, checked field by field. Anything
//!   the model returns that the text states differently, in numbers, words for floors, city, style or building type, is replaced by
//!   what the text says, and the replacement is listed. A room program the model proposes (a typical villa program) is marked
//!   `assumed` so it is reviewed, never accepted silently.
//!
//! Nothing here commits anything: the result fills the form, the user reviews it, and only then is a plan generated. Areas the text
//! does not give are left empty for the user, except in a room program the model proposed and marked as assumed.

use crate::presets;
use crate::project::BuildingType;
use crate::style::{self, Style};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The room functions the planner knows.
pub const FUNCS: [&str; 7] = ["majlis", "living", "dining", "kitchen", "bedroom", "bathroom", "other"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DraftRoom {
    pub func: String,
    /// Display name (Arabic when the description is in Arabic).
    pub name: Option<String>,
    pub area_m2: Option<f64>,
    /// `area_m2` is a target proposed by `suggested_area_m2`, not a figure from the client or the model.
    #[serde(default)]
    pub area_suggested: bool,
    /// 1 is the ground floor.
    pub floor: u32,
    /// Proposed by the model, not stated by the client.
    pub assumed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct BriefDraft {
    pub title: Option<String>,
    pub building_type: Option<BuildingType>,
    pub style: Option<String>,
    pub city: Option<String>,
    pub climate_zone: Option<u8>,
    pub floors: Option<u32>,
    pub annex: bool,
    pub plot_area_m2: Option<f64>,
    pub plot_width_m: Option<f64>,
    pub plot_length_m: Option<f64>,
    pub rooms: Vec<DraftRoom>,
    pub smart_tech: Vec<String>,
    /// What was found, what was replaced, what was ignored, and what the user must still enter.
    pub notes: Vec<String>,
    /// `keywords` or `ollama:<model>`.
    pub source: String,
}

// ------------------------------------------------------------------ suggested room areas

/// A design target for a room of this function, in m², used when nobody gave an area. These are this application's own starting
/// figures (a typical villa program), NOT code values: each one is above the SBC minimum for its type (`sbc::code_value`
/// `room_min_area_m2.<func>`, checked by a test), so a plan built from them does not fail the minimum-area check, and the page
/// marks them as suggested so they are reviewed.
pub fn suggested_area_m2(func: &str) -> f64 {
    match func {
        "majlis" => 30.0,
        "living" => 25.0,
        "dining" => 18.0,
        "kitchen" => 15.0,
        "bedroom" => 16.0,
        "bathroom" => 5.0,
        _ => 10.0,
    }
}

/// Gives every room without an area its suggested target and marks it. Returns how many were filled.
pub fn suggest_areas(rooms: &mut [DraftRoom]) -> usize {
    let mut n = 0;
    for r in rooms.iter_mut().filter(|r| r.area_m2.is_none()) {
        r.area_m2 = Some(suggested_area_m2(&r.func));
        r.area_suggested = true;
        n += 1;
    }
    n
}

const SUGGESTED_NOTE: &str = "rooms marked suggested have a starting target area from this application (each above the SBC minimum for its type), not a figure from the client: review them";

// ------------------------------------------------------------------ text helpers

/// Lower case, Arabic letters unified (hamza forms, ya, ta marbuta), diacritics removed, digits as ASCII.
pub fn norm(s: &str) -> String {
    s.chars()
        .filter_map(|c| match c {
            'أ' | 'إ' | 'آ' => Some('ا'),
            'ى' => Some('ي'),
            'ة' => Some('ه'),
            '\u{064B}'..='\u{0652}' | '\u{0640}' => None,
            '٠'..='٩' => char::from_digit(c as u32 - '٠' as u32, 10),
            '۰'..='۹' => char::from_digit(c as u32 - '۰' as u32, 10),
            _ => Some(c),
        })
        .flat_map(char::to_lowercase)
        .collect()
}

const NUMBER_WORDS: [(&str, u32); 22] = [
    ("واحد", 1),
    ("one", 1),
    ("اثنين", 2),
    ("اثنتين", 2),
    ("two", 2),
    ("ثلاث", 3),
    ("three", 3),
    ("اربع", 4),
    ("four", 4),
    ("خمس", 5),
    ("five", 5),
    ("ست", 6),
    ("six", 6),
    ("سبع", 7),
    ("seven", 7),
    ("ثماني", 8),
    ("eight", 8),
    ("تسع", 9),
    ("nine", 9),
    ("عشر", 10),
    ("ten", 10),
    ("single", 1),
];

/// A number word (or an exact word with a clitic) as its value: `three`, `ثلاث`, `ثلاثة`.
pub(crate) fn count_word(token: &str) -> Option<u32> {
    NUMBER_WORDS.iter().find(|(w, _)| token == *w || (token.starts_with(w) && token.len() <= w.len() + 3)).map(|(_, n)| *n)
}

/// The count written just before `at` in `text` (digits or a number word): `4 غرف نوم`, `three bedrooms`.
fn count_before(text: &str, at: usize) -> Option<u32> {
    let head = text[..at].trim_end();
    let tail: String = head.chars().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect();
    let token = tail.split(|c: char| c.is_whitespace() || c == ',' || c == '،' || c == 'و').next_back().unwrap_or("");
    if let Ok(n) = token.trim_matches(|c: char| !c.is_ascii_digit()).parse::<u32>() {
        return Some(n);
    }
    NUMBER_WORDS.iter().find(|(w, _)| token == *w || token.strip_prefix('و') == Some(w) || token.strip_prefix('ب') == Some(w) || token.starts_with(w) && token.len() <= w.len() + 3).map(|(_, n)| *n)
}

fn find_any(text: &str, keys: &[&str]) -> Option<(usize, usize)> {
    keys.iter().filter_map(|k| text.find(&norm(k)).map(|i| (i, i + norm(k).len()))).min_by_key(|(i, _)| *i)
}

/// All numbers (digits, optional decimal) in `text`, with where they start and end.
fn numbers(text: &str) -> Vec<(f64, usize, usize)> {
    let b: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].1.is_ascii_digit() {
            let start = b[i].0;
            let mut j = i;
            while j < b.len() && (b[j].1.is_ascii_digit() || ((b[j].1 == '.' || b[j].1 == ',') && j + 1 < b.len() && b[j + 1].1.is_ascii_digit())) {
                j += 1;
            }
            let end = if j < b.len() { b[j].0 } else { text.len() };
            if let Ok(v) = text[start..end].replace(',', ".").parse::<f64>() {
                out.push((v, start, end));
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

const AREA_UNITS: [&str; 9] = ["m2", "m²", "sqm", "sq m", "sq.m", "square meter", "square metre", "متر مربع", "م2"];
const LAND_WORDS: [&str; 7] = ["ارض", "قطعه", "مساحه", "plot", "land", "lot", "area"];

/// The plot area written as `600م`, `600 م²`, `600 square meters`, in a plot context.
fn plot_area(text: &str) -> Option<f64> {
    for (v, s, e) in numbers(text) {
        let after = text[e..].trim_start();
        let explicit = AREA_UNITS.iter().any(|u| after.starts_with(&norm(u)));
        let bare_m = after.starts_with('م') && after.chars().nth(1).is_none_or(|c| !c.is_alphabetic());
        let before = &text[..s];
        let near_land = LAND_WORDS.iter().any(|w| before.rfind(&norm(w)).is_some_and(|p| before[p..].chars().count() <= 22));
        if v >= 50.0 && (explicit || (bare_m && near_land)) {
            return Some(v);
        }
    }
    None
}

/// Plot size written as `20×30`, `20x30`, `20 * 30`: `(width, length)` in metres.
pub(crate) fn plot_size_in(text: &str) -> Option<(f64, f64)> {
    let ns = numbers(text);
    for w in ns.windows(2) {
        // The text between the end of one number and the start of the next.
        let between = text[w[0].2..w[1].1].trim();
        if ["x", "×", "*", "في", "by"].contains(&between) && w[0].0 >= 5.0 && w[1].0 >= 5.0 {
            return Some((w[0].0, w[1].0));
        }
    }
    None
}

// ------------------------------------------------------------------ the keyword reading

pub(crate) fn building_type_of(t: &str) -> Option<BuildingType> {
    let at = |k: &[&str]| find_any(t, k).map(|x| x.0);
    [(at(&["فيلا", "villa"]), BuildingType::Villa), (at(&["قصر", "palace"]), BuildingType::Palace), (at(&["عمارة", "apartment building", "residential building", "building of apartments"]), BuildingType::ApartmentBuilding), (at(&["منزل", "بيت", "house", "home"]), BuildingType::House)]
        .into_iter()
        .filter_map(|(i, b)| i.map(|i| (i, b)))
        .min_by_key(|(i, _)| *i)
        .map(|(_, b)| b)
}

pub(crate) fn floors_in(t: &str) -> Option<u32> {
    if find_any(t, &["دورين", "طابقين", "two floors", "two-storey", "two storey", "2-storey", "double storey"]).is_some() {
        return Some(2);
    }
    if find_any(t, &["دور واحد", "طابق واحد", "single floor", "single-storey", "single storey", "one floor", "one-storey"]).is_some() {
        return Some(1);
    }
    for key in ["ادوار", "طوابق", "floors", "storeys", "stories", "levels", "storey"] {
        let k = norm(key);
        let mut from = 0;
        while let Some(p) = t[from..].find(&k) {
            let at = from + p;
            if let Some(n) = count_before(t, at) {
                return Some(n.clamp(1, 20));
            }
            from = at + k.len();
        }
    }
    None
}

fn bedrooms_of(t: &str) -> Option<u32> {
    for key in ["غرف نوم", "غرفه نوم", "غرفتين نوم", "غرف النوم", "bedrooms", "bedroom"] {
        let k = norm(key);
        if let Some(p) = t.find(&k) {
            if key.contains("غرفتين") {
                return Some(2);
            }
            return count_before(t, p).or(Some(1));
        }
    }
    bare_rooms_of(t)
}

/// "Rooms" with no room type after the count (`بثلاث غرف`, `غرفتين`, `4 rooms`) are bedrooms. A count is required: a room word
/// alone gives nothing. A room word followed by another room type (`غرف طعام`, `غرفة جلوس`) names that room, not a bedroom.
fn bare_rooms_of(t: &str) -> Option<u32> {
    const OTHER_ROOM: [&str; 6] = ["طعام", "جلوس", "معيشه", "ضيوف", "خادمه", "سائق"];
    let followed_by_other = |end: usize| {
        let next = t[end..].trim_start();
        OTHER_ROOM.iter().any(|w| next.starts_with(&norm(w)))
    };
    let word_start = |at: usize| t[..at].chars().next_back().is_none_or(|c| !c.is_alphabetic());
    for key in ["غرفتين", "غرف", "rooms", "room"] {
        let k = norm(key);
        let mut from = 0;
        while let Some(p) = t[from..].find(&k) {
            let at = from + p;
            let end = at + k.len();
            from = end;
            let starts_word = word_start(at) || (key.starts_with('غ') && t[..at].ends_with('ب'));
            if !starts_word || followed_by_other(end) {
                continue;
            }
            if key == "غرفتين" {
                return Some(2);
            }
            if let Some(n) = count_before(t, at) {
                return Some(n);
            }
        }
    }
    None
}

fn count_of(t: &str, singular: &[&str], dual: &[&str]) -> u32 {
    if find_any(t, dual).is_some() {
        return 2;
    }
    match find_any(t, singular) {
        Some((s, _)) => count_before(t, s).unwrap_or(1),
        None => 0,
    }
}

/// The city of the map's table named in the (normalised) text: the longest name found.
pub(crate) fn city_in(t: &str) -> Option<&'static presets::CityZone> {
    let mut best: Option<(usize, &presets::CityZone)> = None;
    for c in presets::CITIES.iter() {
        for name in [norm(c.en), norm(c.ar)] {
            let base = name.split(" (").next().unwrap_or(&name).to_string();
            if !base.is_empty() && t.contains(&base) && best.is_none_or(|(l, _)| base.len() > l) {
                best = Some((base.len(), c));
            }
        }
    }
    best.map(|(_, c)| c)
}

pub(crate) fn new_room(func: &str, name: &str, floor: u32) -> DraftRoom {
    DraftRoom { func: func.into(), name: Some(name.into()), area_m2: None, area_suggested: false, floor, assumed: false }
}

/// What the text states, without any model.
pub fn from_keywords(text: &str) -> BriefDraft {
    let t = norm(text);
    let arabic = text.chars().any(|c| ('\u{0600}'..='\u{06FF}').contains(&c));
    let mut d = BriefDraft { source: "keywords".into(), ..Default::default() };
    d.building_type = building_type_of(&t);
    d.floors = floors_in(&t);
    d.annex = find_any(&t, &["ملحق", "annex", "outbuilding", "guest house"]).is_some();
    d.style = style::parse(text).map(|s| s.key().to_string());
    d.plot_area_m2 = plot_area(&t);
    if let Some((w, l)) = plot_size_in(&t) {
        d.plot_width_m = Some(w);
        d.plot_length_m = Some(l);
        d.plot_area_m2.get_or_insert(w * l);
    }
    if let Some(c) = city_in(&t) {
        d.city = Some(c.en.to_string());
        d.climate_zone = Some(c.zone);
    }
    if find_any(&t, &["ذكي", "ذكيه", "smart home", "home automation", "smart"]).is_some() {
        d.smart_tech.push("smart_home".into());
    }
    // Rooms the client named. The floor of a room is stated only when the text says so; bedrooms of a two-floor house are
    // proposed upstairs and marked as such.
    let floors = d.floors.unwrap_or(1);
    let mut rooms = Vec::new();
    let beds = bedrooms_of(&t);
    if let Some(n) = beds {
        for i in 0..n {
            let mut r = new_room("bedroom", &if arabic { format!("غرفة نوم {}", i + 1) } else { format!("Bedroom {}", i + 1) }, if floors > 1 { 2 } else { 1 });
            r.assumed = floors > 1;
            rooms.push(r);
        }
    }
    // A men's and a women's majlis are two rooms; one of them alone is one named room.
    let mens = find_any(&t, &["مجلس رجال", "مجلس الرجال", "مجلس للرجال", "men's majlis", "mens majlis", "majlis for men"]).is_some();
    let womens = find_any(&t, &["مجلس نساء", "مجلس النساء", "مجلس للنساء", "women's majlis", "womens majlis", "majlis for women"]).is_some();
    let both = find_any(&t, &["مجلس رجال ونساء", "مجلس الرجال والنساء"]).is_some();
    let gendered: Vec<(&str, &str)> = match (mens || both, womens || both) {
        (true, true) => vec![("مجلس رجال", "Men's majlis"), ("مجلس نساء", "Women's majlis")],
        (true, false) => vec![("مجلس رجال", "Men's majlis")],
        (false, true) => vec![("مجلس نساء", "Women's majlis")],
        (false, false) => vec![],
    };
    for (ar_name, en_name) in &gendered {
        rooms.push(new_room("majlis", if arabic { ar_name } else { en_name }, 1));
    }
    for (func, ar, en, singular, dual) in [
        ("majlis", "مجلس", "Majlis", vec!["مجلس", "majlis"], vec!["مجلسين", "two majlis"]),
        ("living", "صالة", "Living room", vec!["صاله", "living room", "living hall"], vec!["صالتين"]),
        ("dining", "غرفة طعام", "Dining room", vec!["غرفه طعام", "صاله طعام", "dining"], vec![]),
        ("kitchen", "مطبخ", "Kitchen", vec!["مطبخ", "kitchen"], vec!["مطبخين"]),
    ] {
        let n = if func == "majlis" && !gendered.is_empty() { 0 } else { count_of(&t, &singular, &dual) };
        for i in 0..n {
            rooms.push(new_room(func, &if n > 1 { format!("{} {}", if arabic { ar } else { en }, i + 1) } else if arabic { ar.to_string() } else { en.to_string() }, 1));
        }
    }
    let baths = count_of(&t, &["حمام", "دورة مياه", "دورات مياه", "bathroom", "toilet", "wc"], &["حمامين", "two bathrooms"]);
    for i in 0..baths {
        rooms.push(new_room("bathroom", &if arabic { format!("حمام {}", i + 1) } else { format!("Bathroom {}", i + 1) }, 1));
    }
    d.rooms = rooms;
    let suggested = suggest_areas(&mut d.rooms);
    // What was found, and what is left to the user.
    let mut found: Vec<String> = Vec::new();
    if let Some(b) = d.building_type {
        found.push(format!("building type: {}", b.label().0));
    }
    if let Some(f) = d.floors {
        found.push(format!("floors: {f}"));
    }
    if d.annex {
        found.push("an annex is mentioned: add its rooms (the planner draws one building)".into());
    }
    if let Some(c) = &d.city {
        found.push(format!("city: {c}, climate zone {}", d.climate_zone.unwrap_or(0)));
    }
    if let Some(s) = &d.style {
        found.push(format!("style: {s}"));
    }
    if let Some(a) = d.plot_area_m2 {
        found.push(format!("plot area: {a} m2"));
        if d.plot_width_m.is_none() {
            let side = a.sqrt();
            d.notes.push(format!("only the area ({a} m²) is given: enter the plot's width and length (for example {:.0} x {:.0} m is {:.0} m²); no shape is assumed", side.round(), (a / side.round()).round(), side.round() * (a / side.round()).round()));
        }
    }
    d.notes.extend(found);
    if suggested > 0 {
        d.notes.push(format!("the text does not give room areas: {suggested} room(s) got a suggested target area. {SUGGESTED_NOTE}"));
    }
    if floors > 1 {
        d.notes.push("several floors need the stair's width, length and side: enter them".into());
    }
    d
}

// ------------------------------------------------------------------ the model's answer

/// The JSON schema the local model is asked to follow (Ollama structured output).
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "title": { "type": ["string", "null"] },
            "building_type": { "type": ["string", "null"], "enum": ["villa", "house", "palace", "apartment_building", null] },
            "style": { "type": ["string", "null"], "enum": ["modern_najdi", "contemporary", "neo_classical", "minimalist", null] },
            "city": { "type": ["string", "null"] },
            "floors": { "type": ["integer", "null"] },
            "annex": { "type": "boolean" },
            "plot_area_m2": { "type": ["number", "null"] },
            "plot_width_m": { "type": ["number", "null"] },
            "plot_length_m": { "type": ["number", "null"] },
            "smart_tech": { "type": "array", "items": { "type": "string" } },
            "rooms": { "type": "array", "items": { "type": "object", "properties": {
                "func": { "type": "string", "enum": FUNCS },
                "name": { "type": ["string", "null"] },
                "area_m2": { "type": ["number", "null"] },
                "floor": { "type": "integer" },
                "assumed": { "type": "boolean" } }, "required": ["func", "floor", "assumed"] } }
        },
        "required": ["building_type", "floors", "annex", "rooms"]
    })
}

/// The instructions for the model.
pub fn system_prompt() -> String {
    format!(
        "You read a client's description of a residential building (Arabic or English) and return ONLY a JSON object for an architectural brief. \
         Rules: use null for anything the text does not state (plot size, area, city, style); never invent a number for a plot or a city. \
         building_type is villa, house, palace or apartment_building. style is modern_najdi (نجدي حديث), contemporary, neo_classical or minimalist. \
         floors is the number of floors the text states. annex is true when an annex (ملحق) is mentioned. \
         rooms: list the rooms the text names, each with func in {FUNCS:?}, floor (1 is the ground floor), area_m2 only if the text gives it, and assumed false. \
         If the text names no rooms you may propose a typical program for the building type and mark every proposed room assumed true. \
         Use Arabic names for the rooms when the text is in Arabic. Output JSON only."
    )
}

/// Reads the model's JSON, field by field. Invalid values are dropped and named in the notes.
pub fn from_model_json(v: &Value, model: &str) -> Result<BriefDraft, String> {
    let o = v.as_object().ok_or("the model's answer is not a JSON object")?;
    let mut d = BriefDraft { source: format!("ollama:{model}"), ..Default::default() };
    let text = |k: &str| o.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    d.title = text("title").map(str::to_string);
    d.building_type = match text("building_type") {
        None => None,
        Some(s) => match serde_json::from_value::<BuildingType>(json!(s)) {
            Ok(b) => Some(b),
            Err(_) => {
                d.notes.push(format!("ignored the model's building type '{s}'"));
                None
            }
        },
    };
    d.style = match text("style") {
        None => None,
        Some(s) => match style::parse(s) {
            Some(st) => Some(st.key().to_string()),
            None => {
                d.notes.push(format!("ignored the model's style '{s}'"));
                None
            }
        },
    };
    if let Some(c) = text("city") {
        match presets::zone_of(c) {
            Some(z) => {
                d.city = Some(c.to_string());
                d.climate_zone = Some(z);
            }
            None => d.notes.push(format!("the model named the city '{c}', which is not in the climate-zone table: set the zone yourself")),
        }
    }
    let positive = |k: &str| o.get(k).and_then(Value::as_f64).filter(|x| x.is_finite() && *x > 0.0);
    d.floors = positive("floors").map(|f| (f as u32).clamp(1, 20));
    d.annex = o.get("annex").and_then(Value::as_bool).unwrap_or(false);
    d.plot_area_m2 = positive("plot_area_m2");
    d.plot_width_m = positive("plot_width_m");
    d.plot_length_m = positive("plot_length_m");
    d.smart_tech = o.get("smart_tech").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    let floors = d.floors.unwrap_or(1);
    for r in o.get("rooms").and_then(Value::as_array).into_iter().flatten() {
        let func = r.get("func").and_then(Value::as_str).unwrap_or("");
        if !FUNCS.contains(&func) {
            d.notes.push(format!("ignored a room with the unknown function '{func}'"));
            continue;
        }
        let floor = r.get("floor").and_then(Value::as_u64).unwrap_or(1) as u32;
        if floor < 1 || floor > floors {
            d.notes.push(format!("ignored a {func} on floor {floor}: the building has {floors} floor(s)"));
            continue;
        }
        let area = r.get("area_m2").and_then(Value::as_f64).filter(|a| a.is_finite() && *a > 0.0);
        d.rooms.push(DraftRoom { func: func.into(), name: r.get("name").and_then(Value::as_str).map(str::to_string), area_m2: area, area_suggested: false, floor, assumed: r.get("assumed").and_then(Value::as_bool).unwrap_or(false) });
    }
    suggest_areas(&mut d.rooms);
    Ok(d)
}

/// The model's draft checked against what the text itself states: where the text says one thing and the model another, the text wins
/// and the replacement is listed. Facts the model missed but the text states are added.
pub fn reconcile(mut model: BriefDraft, text: &str) -> BriefDraft {
    let kw = from_keywords(text);
    macro_rules! prefer {
        ($field:ident, $label:expr) => {
            match (&kw.$field, &model.$field) {
                (Some(k), Some(m)) if k != m => {
                    model.notes.push(format!("{}: the text says {:?}, the model said {:?}: kept the text", $label, k, m));
                    model.$field = kw.$field.clone();
                }
                (Some(_), None) => model.$field = kw.$field.clone(),
                _ => {}
            }
        };
    }
    prefer!(building_type, "building type");
    prefer!(style, "style");
    prefer!(floors, "floors");
    prefer!(plot_area_m2, "plot area");
    prefer!(plot_width_m, "plot width");
    prefer!(plot_length_m, "plot length");
    if kw.city.is_some() && kw.city != model.city {
        model.city = kw.city.clone();
        model.climate_zone = kw.climate_zone;
    }
    model.annex |= kw.annex;
    for t in &kw.smart_tech {
        if !model.smart_tech.contains(t) {
            model.smart_tech.push(t.clone());
        }
    }
    // The model's rooms must include at least the bedrooms the text counts.
    let beds_text = kw.rooms.iter().filter(|r| r.func == "bedroom").count();
    let beds_model = model.rooms.iter().filter(|r| r.func == "bedroom").count();
    if beds_text > 0 && beds_text != beds_model {
        model.notes.push(format!("bedrooms: the text says {beds_text}, the model listed {beds_model}: used the text's {beds_text}"));
        model.rooms.retain(|r| r.func != "bedroom");
        model.rooms.extend(kw.rooms.iter().filter(|r| r.func == "bedroom").cloned());
    }
    // Plot area and size must agree.
    if let (Some(a), Some(w), Some(l)) = (model.plot_area_m2, model.plot_width_m, model.plot_length_m)
        && (a - w * l).abs() > 0.02 * a
    {
        model.notes.push(format!("the plot area {a} m² does not equal {w} x {l} m: check them"));
    }
    if model.plot_area_m2.is_some() && model.plot_width_m.is_none() {
        model.notes.push("only the plot area is known: enter the plot's width and length; no shape is assumed".into());
    }
    if model.rooms.iter().any(|r| r.assumed) {
        model.notes.push("rooms marked assumed were proposed by the model, not stated by the client: review them".into());
    }
    suggest_areas(&mut model.rooms);
    if model.rooms.iter().any(|r| r.area_suggested) {
        model.notes.push(SUGGESTED_NOTE.into());
    }
    if model.floors.unwrap_or(1) > 1 {
        model.notes.push("several floors need the stair's width, length and side: enter them".into());
    }
    if let Some(c) = &model.city {
        model.notes.push(format!("city: {c}, climate zone {}", model.climate_zone.unwrap_or(0)));
    }
    model
}

/// The names of the styles for the page's select.
pub fn style_keys() -> [Style; 4] {
    style::ALL
}
