//! The clarifying conversation: after the description is read (`briefai`), ask only for what the brief still lacks, one question
//! at a time, apply each answer to the draft, and stop when nothing is missing.
//!
//! - `missing` lists the fields the brief needs and the draft does not have, in the order they are asked.
//! - `next_question` is the first of them, or `None` when the brief is complete: the conversation ends there.
//! - `apply_answer` reads the user's reply to one field. A reply that cannot be read is an error naming the field and what was
//!   expected; the draft is not changed and the same question stays open. Nothing is guessed: no width, length, city or style
//!   is filled by this module.
//!
//! The questions and the errors carry both languages.

use crate::briefai::{self, BriefDraft, DraftRoom, FUNCS};
use crate::style;
use serde::Serialize;

/// What the brief needs, in the order it is asked.
pub const FIELDS: [&str; 6] = ["building_type", "floors", "plot_size", "city", "style", "rooms"];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Question {
    /// One of `FIELDS`.
    pub field: &'static str,
    pub ar: String,
    pub en: String,
    /// Valid answers when the field has a closed set (building types, styles), as `(key, Arabic, English)`.
    pub options: Vec<(String, String, String)>,
}

fn q(field: &'static str, ar: &str, en: &str) -> Question {
    Question { field, ar: ar.into(), en: en.into(), options: Vec::new() }
}

fn question_for(field: &'static str, d: &BriefDraft) -> Question {
    match field {
        "building_type" => {
            let mut x = q("building_type", "ما نوع المبنى؟ (فيلا، منزل، قصر، عمارة سكنية)", "What type of building is it? (villa, house, palace, apartment building)");
            x.options = [("villa", "فيلا", "Villa"), ("house", "منزل", "House"), ("palace", "قصر", "Palace"), ("apartment_building", "عمارة سكنية", "Apartment building")].iter().map(|(k, a, e)| (k.to_string(), a.to_string(), e.to_string())).collect();
            x
        }
        "floors" => q("floors", "كم عدد الأدوار؟", "How many floors?"),
        "plot_size" => match d.plot_area_m2 {
            Some(a) => q("plot_size", &format!("مساحة الأرض {a} م²: ما عرضها وطولها بالمتر؟ (مثال: 20×30)"), &format!("The plot area is {a} m²: what are its width and length in metres? (for example 20x30)")),
            None => q("plot_size", "ما عرض الأرض وطولها بالمتر؟ (مثال: 20×30)", "What are the plot's width and length in metres? (for example 20x30)"),
        },
        "city" => q("city", "في أي مدينة يقع المشروع؟ (تحدد المنطقة المناخية)", "In which city is the project? (it sets the climate zone)"),
        "style" => {
            let mut x = q("style", "ما الطراز المعماري؟", "Which architectural style?");
            x.options = style::ALL.iter().map(|s| (s.key().to_string(), s.label().1.to_string(), s.label().0.to_string())).collect();
            x
        }
        _ => q("rooms", "كم غرفة نوم تريد، وما الغرف الأخرى (مجلس، صالة، مطبخ، حمامات)؟", "How many bedrooms do you want, and which other rooms (majlis, living room, kitchen, bathrooms)?"),
    }
}

fn is_missing(field: &str, d: &BriefDraft) -> bool {
    match field {
        "building_type" => d.building_type.is_none(),
        "floors" => d.floors.is_none(),
        "plot_size" => d.plot_width_m.is_none() || d.plot_length_m.is_none(),
        "city" => d.city.is_none() || d.climate_zone.is_none(),
        "style" => d.style.is_none(),
        _ => d.rooms.is_empty(),
    }
}

/// Every question the draft still needs answered, in order.
pub fn missing(d: &BriefDraft) -> Vec<Question> {
    FIELDS.iter().filter(|f| is_missing(f, d)).map(|f| question_for(f, d)).collect()
}

/// The next question, or `None` when the brief is complete.
pub fn next_question(d: &BriefDraft) -> Option<Question> {
    FIELDS.iter().find(|f| is_missing(f, d)).map(|f| question_for(f, d))
}

pub fn is_complete(d: &BriefDraft) -> bool {
    FIELDS.iter().all(|f| !is_missing(f, d))
}

fn bad(ar: &str, en: &str) -> String {
    format!("{ar} | {en}")
}

/// A reply that is only a number (`3`, `٣`, `three`, `ثلاثة`): its value.
fn bare_count(t: &str) -> Option<u32> {
    let t = t.trim().trim_matches(|c: char| !c.is_alphanumeric());
    if let Ok(n) = t.parse::<u32>() {
        return Some(n);
    }
    briefai::count_word(t)
}

/// Applies the user's reply to `field`. On error the draft is unchanged.
pub fn apply_answer(d: &mut BriefDraft, field: &str, answer: &str) -> Result<(), String> {
    let text = answer.trim();
    if text.is_empty() {
        return Err(bad("الإجابة فارغة", "the answer is empty"));
    }
    let t = briefai::norm(text);
    match field {
        "building_type" => {
            let b = briefai::building_type_of(&t).ok_or_else(|| bad("لم أفهم نوع المبنى: اكتب فيلا أو منزل أو قصر أو عمارة", "could not read the building type: write villa, house, palace or apartment building"))?;
            d.building_type = Some(b);
        }
        "floors" => {
            let n = bare_count(&t).or_else(|| briefai::floors_in(&t)).ok_or_else(|| bad("لم أفهم عدد الأدوار: اكتب رقماً مثل 2", "could not read the number of floors: write a number such as 2"))?;
            if !(1..=20).contains(&n) {
                return Err(bad(&format!("عدد الأدوار {n} خارج المدى 1–20"), &format!("{n} floors is outside 1–20")));
            }
            d.floors = Some(n);
        }
        "plot_size" => {
            let (w, l) = briefai::plot_size_in(&t).ok_or_else(|| bad("لم أفهم أبعاد الأرض: اكتب العرض والطول مثل 20×30", "could not read the plot size: write width and length such as 20x30"))?;
            if let Some(a) = d.plot_area_m2
                && (a - w * l).abs() > 0.02 * a
            {
                return Err(bad(&format!("{w}×{l} = {} م² ولا يساوي مساحة الأرض {a} م²: صحّح الأبعاد أو المساحة", w * l), &format!("{w} x {l} = {} m² does not equal the plot area {a} m²: correct the size or the area", w * l)));
            }
            d.plot_width_m = Some(w);
            d.plot_length_m = Some(l);
            d.plot_area_m2.get_or_insert(w * l);
        }
        "city" => {
            let c = briefai::city_in(&t).ok_or_else(|| bad("المدينة ليست في جدول المناطق المناخية: اختر مدينة من الجدول", "that city is not in the climate-zone table: pick a city from the table"))?;
            d.city = Some(c.en.to_string());
            d.climate_zone = Some(c.zone);
        }
        "style" => {
            let s = style::parse(text).ok_or_else(|| bad("لم أفهم الطراز: نجدي حديث أو معاصر أو كلاسيكي حديث أو مينيمال", "could not read the style: modern Najdi, contemporary, neo-classical or minimalist"))?;
            d.style = Some(s.key().to_string());
        }
        "rooms" => {
            let floors = d.floors.unwrap_or(1);
            let arabic = text.chars().any(|c| ('\u{0600}'..='\u{06FF}').contains(&c));
            let mut rooms: Vec<DraftRoom> = match bare_count(&t) {
                Some(n) if n > 0 => (1..=n).map(|i| briefai::new_room("bedroom", &if arabic { format!("غرفة نوم {i}") } else { format!("Bedroom {i}") }, 1)).collect(),
                _ => briefai::from_keywords(text).rooms,
            };
            if rooms.is_empty() {
                return Err(bad("لم أجد غرفاً في الإجابة: اكتب عدد غرف النوم ثم الغرف الأخرى", "found no rooms in the answer: write the number of bedrooms, then the other rooms"));
            }
            for r in rooms.iter_mut().filter(|r| r.func == "bedroom") {
                r.floor = if floors > 1 { 2 } else { 1 };
                r.assumed = floors > 1;
            }
            debug_assert!(rooms.iter().all(|r| FUNCS.contains(&r.func.as_str())));
            briefai::suggest_areas(&mut rooms);
            d.rooms = rooms;
        }
        other => return Err(bad(&format!("حقل غير معروف '{other}'"), &format!("unknown field '{other}'"))),
    }
    d.notes.push(format!("{field}: set from the user's answer"));
    Ok(())
}
