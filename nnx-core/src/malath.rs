//! The latent-defects (Malath / ملاذ) audit checklist: stage gates for soil, foundations and wet areas, kept in the project.
//!
//! WHAT THIS IS: a checklist built from items of the Inspectors' checklist file (قائمة تحقق المفتشين الفاحصين), each with its
//! item number and page. It is NOT the insurer's own list and does not replace the inspection office or the insurer. Items marked
//! `source: None` were added by this application because the files hold no clause for them (settlement and expansion joints), and
//! the report says so.
//!
//! Three items are judged from your inputs instead of being ticked: the number of borings in the soil report (the inspectors'
//! item 1: fewer than 3 is a defect) and the concrete cover against Table 4A5 (`presets::cover_mm`). The others you mark
//! `accepted`, `rejected`, `na` or leave `pending`.

use crate::compliance::{ComplianceInputs, CoverInput};
use crate::presets::{self, Exposure, Member};
use serde::Serialize;

const FILE: &str = "قائمة تحقق المفتشين الفاحصين.pdf";

#[derive(Debug, Clone, Copy)]
pub struct Item {
    pub id: &'static str,
    /// Stage gate, in order: `soil`, `foundations`, `joints`, `wet`.
    pub stage: &'static str,
    pub en: &'static str,
    pub ar: &'static str,
    /// `(item number in the file, page)`; `None` for an item the application added.
    pub source: Option<(&'static str, u32)>,
    pub auto: bool,
}

pub const STAGES: [(&str, &str, &str); 4] = [("soil", "Soil investigation", "فحص التربة"), ("foundations", "Foundations and waterproofing", "الأساسات والعزل"), ("joints", "Settlement and expansion joints", "فواصل الهبوط والتمدد"), ("wet", "Wet areas", "المناطق الرطبة")];

pub const ITEMS: [Item; 11] = [
    Item { id: "soil.borings", stage: "soil", en: "The soil report has at least 3 borings (inspectors' item 1: fewer than 3 is a defect)", ar: "تقرير التربة فيه 3 جسات على الأقل", source: Some(("1", 15)), auto: true },
    Item { id: "soil.level", stage: "soil", en: "The foundation level required by the soil report was reached", ar: "تم الوصول لمستوى التأسيس المطلوب في تقرير التربة", source: Some(("2", 15)), auto: false },
    Item { id: "soil.compaction", stage: "soil", en: "Compaction layers and the tests the soil report recommends were done under an accredited soil laboratory", ar: "نُفذت طبقات الإحلال والاختبارات الموصى بها تحت إشراف مختبر تربة معتمد", source: Some(("3", 15)), auto: false },
    Item { id: "soil.groundwater", stage: "soil", en: "Where there is groundwater, a lowering system was designed and installed", ar: "عند وجود مياه جوفية صُمم ونُفذ نظام خفض المنسوب", source: Some(("4", 15)), auto: false },
    Item { id: "found.waterproofing", stage: "foundations", en: "Damp-proofing and waterproofing requirements were met (continuous under and around the foundations)", ar: "تحققت اشتراطات عزل الرطوبة والعزل المائي (باستمرار حول الأساسات)", source: Some(("29", 19)), auto: false },
    Item { id: "found.penetrations", stage: "foundations", en: "Pipes through concrete and masonry walls are in sleeves and the annular space is sealed with water-resistant material", ar: "الأنابيب المخترقة للجدران محمية بأكمام والفراغ الحلقي مغلق بمادة مقاومة للمياه", source: Some(("33-34", 19)), auto: false },
    Item { id: "found.cover", stage: "foundations", en: "Minimum concrete cover for the exposure class (SBC 1101 section 4A7.1, Table 4A5)", ar: "الغطاء الخرساني الأدنى حسب فئة التعرض (SBC 1101 القسم 4A7.1)", source: Some(("on concrete cover", 22)), auto: true },
    Item { id: "joints.settlement", stage: "joints", en: "Settlement joints between parts founded at different levels or on different soils are provided where the engineer requires them", ar: "فواصل هبوط بين الأجزاء المؤسسة على مناسيب أو تربة مختلفة حيث يتطلبها المهندس", source: None, auto: false },
    Item { id: "joints.expansion", stage: "joints", en: "Expansion joints are provided where the engineer requires them, with the waterproofing continuous across them", ar: "فواصل تمدد حيث يتطلبها المهندس مع استمرار العزل عبرها", source: None, auto: false },
    Item { id: "wet.floor", stage: "wet", en: "The bathroom floor waterproofing passed the water test required by the code", ar: "اجتاز العزل المائي لأرضية الحمام اختبار الماء وفق الكود", source: Some(("13", 45)), auto: false },
    Item { id: "wet.walls", stage: "wet", en: "Bathroom walls and floor are non-absorbent up to at least 1800 mm above the standing floor", ar: "جدران وأرضية الحمام غير ماصة للمياه وبارتفاع 1800 مم على الأقل", source: Some(("14", 45)), auto: false },
];

pub fn item(id: &str) -> Option<&'static Item> {
    ITEMS.iter().find(|i| i.id == id)
}

#[derive(Debug, Clone, Serialize)]
pub struct ItemReport {
    pub id: &'static str,
    pub stage: &'static str,
    pub en: &'static str,
    pub ar: &'static str,
    pub source: String,
    pub status: String,
    pub note: String,
    /// Set from your inputs, not ticked.
    pub auto: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct StageReport {
    pub stage: &'static str,
    pub en: &'static str,
    pub ar: &'static str,
    pub accepted: usize,
    pub rejected: usize,
    pub pending: usize,
    pub na: usize,
    /// Every item accepted or not applicable.
    pub complete: bool,
    /// The earlier gates are complete: this one may start.
    pub open: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MalathReport {
    pub items: Vec<ItemReport>,
    pub stages: Vec<StageReport>,
    pub soil_status: String,
    pub borings: Option<u32>,
    pub cover: Option<CoverResult>,
    pub disclaimer_en: &'static str,
    pub disclaimer_ar: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct CoverResult {
    pub required_mm: Option<f64>,
    pub specified_mm: Option<f64>,
    pub ok: Option<bool>,
    pub source: String,
    pub missing: Vec<String>,
}

pub fn cover_result(c: &CoverInput) -> CoverResult {
    let mut missing = Vec::new();
    let exp = match c.exposure.as_deref() {
        Some("ground") => Some(Exposure::Ground),
        Some("weather") => Some(Exposure::Weather),
        Some("protected") => Some(Exposure::Protected),
        _ => {
            missing.push("exposure (ground, weather or protected)".to_string());
            None
        }
    };
    let mem = match c.member.as_deref() {
        Some("slab") => Some(Member::Slab),
        Some("frame") => Some(Member::Frame),
        _ => {
            missing.push("member (slab or frame)".to_string());
            None
        }
    };
    if c.bar_dia_mm.is_none() {
        missing.push("bar diameter".into());
    }
    if c.specified_mm.is_none() {
        missing.push("specified cover".into());
    }
    let required = match (exp, mem, c.bar_dia_mm) {
        (Some(e), Some(m), Some(d)) => presets::cover_mm(e, m, d),
        _ => None,
    };
    if required.is_none() && missing.is_empty() {
        missing.push("Table 4A5 gives no cover for this slab bar size".into());
    }
    let ok = match (required, c.specified_mm) {
        (Some(r), Some(s)) => Some(s + 1e-9 >= r),
        _ => None,
    };
    CoverResult { required_mm: required, specified_mm: c.specified_mm, ok, source: presets::COVER_SOURCE.text(), missing }
}

pub fn report(c: &ComplianceInputs) -> MalathReport {
    let cover = c.cover.as_ref().map(cover_result);
    let mut items = Vec::new();
    for it in ITEMS {
        let st = c.malath.get(it.id);
        let (mut status, mut note) = st.map_or(("pending".to_string(), String::new()), |s| (s.status.clone(), s.note.clone()));
        if status.is_empty() {
            status = "pending".into();
        }
        if it.id == "soil.borings"
            && let Some(n) = c.soil.borings
        {
            status = if n >= 3 { "accepted" } else { "rejected" }.into();
            note = format!("{n} boring(s) in the soil report");
        }
        if it.id == "found.cover"
            && let Some(r) = cover.as_ref().and_then(|r| r.ok)
        {
            status = if r { "accepted" } else { "rejected" }.into();
            note = format!("specified {} mm against {} mm (Table 4A5)", cover.as_ref().and_then(|r| r.specified_mm).unwrap_or(0.0), cover.as_ref().and_then(|r| r.required_mm).unwrap_or(0.0));
        }
        items.push(ItemReport {
            id: it.id,
            stage: it.stage,
            en: it.en,
            ar: it.ar,
            source: it.source.map_or_else(|| "added by this application: no clause for it in the files".to_string(), |(n, p)| format!("inspectors' checklist, item {n}, {FILE} p.{p}")),
            status,
            note,
            auto: it.auto,
        });
    }
    let mut stages = Vec::new();
    let mut earlier_complete = true;
    for (s, en, ar) in STAGES {
        let mine: Vec<&ItemReport> = items.iter().filter(|i| i.stage == s).collect();
        let count = |k: &str| mine.iter().filter(|i| i.status == k).count();
        let complete = mine.iter().all(|i| i.status == "accepted" || i.status == "na");
        stages.push(StageReport { stage: s, en, ar, accepted: count("accepted"), rejected: count("rejected"), pending: count("pending"), na: count("na"), complete, open: earlier_complete });
        earlier_complete &= complete;
    }
    MalathReport {
        items,
        stages,
        soil_status: if c.soil.status.is_empty() { "none".into() } else { c.soil.status.clone() },
        borings: c.soil.borings,
        cover,
        disclaimer_en: "A checklist built from the inspectors' checklist items with their pages; it is not the insurer's list and does not replace the inspection office or the insurer.",
        disclaimer_ar: "قائمة مبنية من بنود قائمة تحقق المفتشين مع صفحاتها؛ ليست قائمة شركة التأمين ولا تغني عن مكتب الفحص أو شركة التأمين.",
    }
}

/// Sets an item's status (and note) in the inputs. Items judged from your inputs cannot be ticked while their input is present.
pub fn set_status(c: &mut ComplianceInputs, id: &str, status: &str, note: &str) -> Result<(), String> {
    let it = item(id).ok_or_else(|| format!("unknown checklist item '{id}'"))?;
    if !["pending", "accepted", "rejected", "na"].contains(&status) {
        return Err(format!("unknown status '{status}' (pending, accepted, rejected, na)"));
    }
    if it.auto && ((id == "soil.borings" && c.soil.borings.is_some()) || (id == "found.cover" && c.cover.as_ref().is_some_and(|x| cover_result(x).ok.is_some()))) {
        return Err(format!("'{id}' is judged from the inputs you entered (the soil report or the concrete cover): change them instead"));
    }
    if status == "pending" && note.is_empty() {
        c.malath.remove(id);
    } else {
        c.malath.insert(id.to_string(), crate::compliance::MalathState { status: status.to_string(), note: note.to_string() });
    }
    Ok(())
}
