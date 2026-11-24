//! Construction supervision registers: site handover, material and shop-drawing submittals (MAR), work inspection requests with
//! hold points (WIR), material inspection requests (MIR), laboratory tests, non-conformance reports (NCR), requests for information
//! (RFI), minutes of meeting (MOM), variation orders (VO), progress reports with the planned / actual S-curve, the snag list by room
//! and the close-out dossier, stored as ONE value of the project (`meta.supervision`). Because the project is a model element every
//! edit is a `SetProp` on the document's stack, so undo, redo, the dirty flag and the `.nnx` file follow with no extra bookkeeping.
//!
//! Nothing here supplies an acceptance limit, a rate or a date. The report only states what the user recorded and what that implies:
//! - an approval needs the reviewer and the date; a rejection or an approval with comments needs the comment;
//! - a work inspection cannot be approved while one of its hold points is unsigned, and it cannot be approved with no hold point;
//! - a material inspection must name an approved MAR;
//! - a laboratory result is judged only against the acceptance value the user entered WITH its source; without it the verdict is
//!   `unchecked`, never `pass`;
//! - the close-out is ready only when every checklist item is done and nothing is open (NCR, snag, failed test, unapproved inspection).

use crate::execution::{blank, esc, parse_date};
use crate::schedule::Date;
use crate::xlsx::{Cell, Sheet, Workbook, style};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The decision on a submittal or an inspection request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Review {
    #[default]
    Submitted,
    Approved,
    ApprovedWithComments,
    Rejected,
}

impl Review {
    fn approved(self) -> bool {
        matches!(self, Review::Approved | Review::ApprovedWithComments)
    }
    fn decided(self) -> bool {
        self != Review::Submitted
    }
    fn label(self, ar: bool) -> &'static str {
        match (self, ar) {
            (Review::Submitted, false) => "Submitted",
            (Review::Approved, false) => "Approved",
            (Review::ApprovedWithComments, false) => "Approved with comments",
            (Review::Rejected, false) => "Rejected",
            (Review::Submitted, true) => "مقدَّم",
            (Review::Approved, true) => "معتمد",
            (Review::ApprovedWithComments, true) => "معتمد مع ملاحظات",
            (Review::Rejected, true) => "مرفوض",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Signatory {
    pub role: String,
    pub name: String,
    pub signed_date: Option<String>,
}

/// The site handover minutes. The coordinate system is the user's own text: no datum is assumed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Handover {
    pub date: Option<String>,
    pub site_ref: Option<String>,
    pub easting_m: Option<f64>,
    pub northing_m: Option<f64>,
    /// For example the projection and datum the survey used, as written on the survey.
    pub crs: Option<String>,
    pub benchmark_description: Option<String>,
    pub benchmark_level_m: Option<f64>,
    pub benchmark_datum: Option<String>,
    pub start_date: Option<String>,
    pub signatories: Vec<Signatory>,
}

/// Categories of material submittal (a MAR).
pub const MAR_CATEGORIES: [(&str, &str, &str); 7] = [
    ("concrete", "Concrete", "الخرسانة"),
    ("rebar", "Reinforcing steel", "حديد التسليح"),
    ("blocks", "Blocks", "البلوك"),
    ("waterproofing", "Waterproofing", "العزل المائي"),
    ("cables", "Cables", "الكابلات"),
    ("tiles", "Tiles", "البلاط"),
    ("other", "Other", "أخرى"),
];

/// A material submittal (`category` is one of `MAR_CATEGORIES`) or a shop drawing (`category` is free text such as a discipline).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Submittal {
    pub id: String,
    pub category: String,
    pub description: String,
    /// The supplier (material) or the drawing number (shop drawing).
    pub reference: Option<String>,
    pub submitted: Option<String>,
    pub status: Review,
    pub reviewed_by: Option<String>,
    pub reviewed_date: Option<String>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HoldPoint {
    pub text: String,
    pub signed_by: Option<String>,
    pub signed_date: Option<String>,
}

impl HoldPoint {
    fn signed(&self) -> bool {
        !blank(&self.signed_by) && !blank(&self.signed_date)
    }
}

/// The stages a work inspection request covers: `(id, English, Arabic)`.
pub const WIR_STAGES: [(&str, &str, &str); 8] = [
    ("excavation", "Excavation", "الحفر"),
    ("blinding", "Blinding concrete", "الخرسانة العادية"),
    ("foundation_rebar", "Foundation reinforcement", "حديد القواعد"),
    ("foundation_formwork", "Foundation formwork", "شدات القواعد"),
    ("columns", "Columns", "الأعمدة"),
    ("slabs", "Slabs", "البلاطات"),
    ("waterproofing", "Waterproofing", "العزل المائي"),
    ("concealed_mep", "Concealed MEP", "أعمال MEP المخفية"),
];

/// Procedural hold points of a stage: what the inspector must see before the work is covered or poured. They state no figure: a
/// dimension, cover or strength comes from the approved drawings and specification, and the inspector records it.
pub fn hold_templates(stage: &str) -> Vec<(&'static str, &'static str)> {
    match stage {
        "excavation" => vec![("Extent and level checked against the approved drawings", "حدود الحفر ومنسوبه مطابقان للمخططات المعتمدة"), ("Soil checked against the geotechnical report", "التربة مطابقة لتقرير التربة")],
        "blinding" => vec![("Subgrade accepted before the blinding is poured", "قاع الحفر مقبول قبل صب الخرسانة العادية"), ("Level of the blinding checked", "منسوب الخرسانة العادية مفحوص")],
        "foundation_rebar" => vec![("Bar sizes, spacing and cover checked against the approved shop drawings", "الأقطار والتباعد والغطاء مطابقة للمخططات التنفيذية المعتمدة"), ("Concrete mix and pour approved", "خلطة الخرسانة وأمر الصب معتمدان")],
        "foundation_formwork" => vec![("Formwork dimensions and levels checked", "أبعاد الشدات ومناسيبها مفحوصة"), ("Formwork supports and tightness checked", "ثبات الشدات وإحكامها مفحوصان")],
        "columns" => vec![("Column reinforcement and verticality checked", "تسليح الأعمدة ورأسيتها مفحوصان"), ("Pour approved", "أمر الصب معتمد")],
        "slabs" => vec![("Slab reinforcement, cover and embedded services checked", "تسليح البلاطة والغطاء والخدمات المدفونة مفحوصة"), ("Pour approved", "أمر الصب معتمد")],
        "waterproofing" => vec![("Surface preparation accepted", "تجهيز السطح مقبول"), ("Membrane applied as specified and inspected before cover", "الغشاء منفَّذ حسب المواصفة ومفحوص قبل التغطية")],
        "concealed_mep" => vec![("Routes and supports checked against the approved drawings", "المسارات والحوامل مطابقة للمخططات المعتمدة"), ("Pressure or continuity test witnessed before closing", "اختبار الضغط أو الاستمرارية مشهود قبل الإغلاق")],
        _ => Vec::new(),
    }
}

/// A work inspection request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Wir {
    pub id: String,
    pub stage: String,
    /// The building element or zone: a schedule name (`COL-3`), an element id or free text.
    pub element: String,
    pub requested: Option<String>,
    pub status: Review,
    pub inspector: Option<String>,
    pub decided_date: Option<String>,
    pub comment: Option<String>,
    pub hold_points: Vec<HoldPoint>,
}

/// A material inspection request: a delivery verified against an approved MAR.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Mir {
    pub id: String,
    pub mar: String,
    pub delivered: Option<String>,
    pub quantity: Option<f64>,
    pub unit: Option<String>,
    pub delivery_note: Option<String>,
    pub status: Review,
    pub checked_by: Option<String>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabKind {
    #[default]
    Cube,
    Proctor,
    Pressure,
    Other,
}

/// Which side of the acceptance value passes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    /// The measured value must be at least the acceptance value (strength, compaction).
    #[default]
    AtLeast,
    /// The measured value must be at most the acceptance value (pressure drop, leakage).
    AtMost,
}

/// One laboratory or field test result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LabTest {
    pub id: String,
    pub kind: LabKind,
    /// What was tested: the element poured, the layer compacted, the pipe run.
    pub subject: String,
    /// Casting or sampling date (concrete cubes: the age is counted from it).
    pub sampled: Option<String>,
    pub tested: Option<String>,
    pub measured: Option<f64>,
    pub unit: Option<String>,
    /// The acceptance value, as the specification or the design states it.
    pub required: Option<f64>,
    pub rule: Rule,
    /// Where `required` comes from (specification clause, drawing note, engineer's instruction). Required with `required`.
    pub required_source: Option<String>,
    /// Concrete cubes: the design characteristic strength f'c in MPa, for the ratio column only.
    pub design_fc_mpa: Option<f64>,
    pub lab: Option<String>,
    pub report_ref: Option<String>,
}

/// The outcome of one test against its acceptance value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// No result entered yet.
    Pending,
    /// A result but no acceptance value: nothing is claimed.
    Unchecked,
    Pass,
    Fail,
}

impl Verdict {
    fn label(self, ar: bool) -> &'static str {
        match (self, ar) {
            (Verdict::Pending, false) => "Pending",
            (Verdict::Unchecked, false) => "Unchecked (no acceptance value)",
            (Verdict::Pass, false) => "Pass",
            (Verdict::Fail, false) => "FAIL",
            (Verdict::Pending, true) => "بانتظار النتيجة",
            (Verdict::Unchecked, true) => "غير مفحوص (بلا قيمة قبول)",
            (Verdict::Pass, true) => "مطابق",
            (Verdict::Fail, true) => "غير مطابق",
        }
    }
}

impl LabTest {
    pub fn verdict(&self) -> Verdict {
        match (self.measured, self.required) {
            (None, _) => Verdict::Pending,
            (Some(_), None) => Verdict::Unchecked,
            (Some(m), Some(r)) => {
                let ok = match self.rule {
                    Rule::AtLeast => m >= r,
                    Rule::AtMost => m <= r,
                };
                if ok { Verdict::Pass } else { Verdict::Fail }
            }
        }
    }

    /// Days from sampling to testing, when both dates are set.
    pub fn age_days(&self) -> Option<i64> {
        let a = Date::parse(self.sampled.as_deref()?.trim())?;
        let b = Date::parse(self.tested.as_deref()?.trim())?;
        Some(b.to_days() - a.to_days())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Ncr {
    pub id: String,
    pub date: Option<String>,
    pub element: Option<String>,
    pub description: String,
    /// The user's own classification (minor / major / critical or the office's wording).
    pub severity: Option<String>,
    pub raised_by: Option<String>,
    pub corrective_action: Option<String>,
    pub due: Option<String>,
    pub closed_date: Option<String>,
    pub closed_by: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Rfi {
    pub id: String,
    pub subject: String,
    /// The drawing sheet or the model element the question is about (an element id or a schedule name such as `COL-3`).
    pub bim_ref: Option<String>,
    pub raised: Option<String>,
    pub due: Option<String>,
    pub answered: Option<String>,
    pub answer: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MeetingItem {
    pub text: String,
    pub owner: Option<String>,
    pub due: Option<String>,
    pub done: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Meeting {
    pub id: String,
    pub date: Option<String>,
    pub title: String,
    pub attendees: Vec<String>,
    pub items: Vec<MeetingItem>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoStatus {
    #[default]
    Proposed,
    Approved,
    Rejected,
}

/// A variation order. Amounts and days are signed (a saving is negative) and entered by the user.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Variation {
    pub id: String,
    pub description: String,
    pub cost_delta: Option<f64>,
    pub days_delta: Option<i64>,
    pub status: VoStatus,
    pub approved_date: Option<String>,
    /// The owner's approval reference. Required to approve.
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProgressEntry {
    pub period_end: String,
    /// Cumulative planned and actual progress, 0 to 100.
    pub planned_pct: Option<f64>,
    pub actual_pct: Option<f64>,
    pub manpower: Option<u32>,
    pub equipment: Option<u32>,
    pub blockers: Option<String>,
    /// File references of the photo log.
    pub photos: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnagStatus {
    #[default]
    Open,
    Fixed,
    Verified,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Snag {
    pub id: String,
    /// The room key of the plan.
    pub room: String,
    pub description: String,
    pub found: Option<String>,
    pub status: SnagStatus,
    pub fixed_date: Option<String>,
    pub verified_by: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CloseRecord {
    pub item: String,
    pub done: bool,
    pub by: Option<String>,
    pub date: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Warranty {
    pub item: String,
    pub supplier: Option<String>,
    pub start: Option<String>,
    pub months: Option<u32>,
    pub doc_ref: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OmManual {
    pub title: String,
    pub doc_ref: Option<String>,
    pub received: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Closeout {
    pub records: Vec<CloseRecord>,
    pub warranties: Vec<Warranty>,
    pub manuals: Vec<OmManual>,
}

/// The close-out checklist: `(id, English, Arabic)`.
pub const CLOSEOUT_ITEMS: [(&str, &str, &str); 8] = [
    ("toc", "Taking Over Certificate issued", "شهادة الاستلام الابتدائي صادرة"),
    ("completion", "Completion certificate obtained", "شهادة إتمام البناء مستلمة"),
    ("as_built", "As-built drawings submitted and accepted", "مخططات ما بعد التنفيذ مقدمة ومقبولة"),
    ("om", "O&M manuals received", "أدلة التشغيل والصيانة مستلمة"),
    ("warranties", "Warranties received", "الضمانات مستلمة"),
    ("test_certs", "Test and commissioning certificates received", "شهادات الاختبار والتشغيل مستلمة"),
    ("utilities", "Utility connection approvals received", "موافقات توصيل الخدمات مستلمة"),
    ("final_account", "Final account agreed", "المستخلص الختامي متفق عليه"),
];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SupervisionInputs {
    pub handover: Option<Handover>,
    pub mar: Vec<Submittal>,
    pub shop_drawings: Vec<Submittal>,
    pub wir: Vec<Wir>,
    pub mir: Vec<Mir>,
    pub lab: Vec<LabTest>,
    pub ncr: Vec<Ncr>,
    pub rfi: Vec<Rfi>,
    pub mom: Vec<Meeting>,
    pub variations: Vec<Variation>,
    pub progress: Vec<ProgressEntry>,
    pub snags: Vec<Snag>,
    pub closeout: Closeout,
    /// Interim payment certificates on the BOQ quantities.
    pub ipc: crate::ipc::IpcInputs,
    /// The contract terms the draft contract is assembled from.
    pub contract: crate::contract::ContractInputs,
    /// The soil investigation report figures.
    pub geotech: crate::geotech::GeotechInputs,
}

fn opt_date(what: &str, d: &Option<String>) -> Result<Option<Date>, String> {
    match d.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(s) => parse_date(what, s).map(Some),
    }
}

fn unique<'a>(what: &str, ids: impl Iterator<Item = &'a str>) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for id in ids {
        if id.trim().is_empty() {
            return Err(format!("{what}: a record has no id"));
        }
        if !seen.insert(id) {
            return Err(format!("{what}: the id '{id}' is used twice"));
        }
    }
    Ok(())
}

fn finite(what: &str, v: Option<f64>) -> Result<(), String> {
    match v {
        Some(x) if !x.is_finite() => Err(format!("{what}: not a finite number")),
        _ => Ok(()),
    }
}

fn check_submittal(what: &str, s: &Submittal, mar: bool) -> Result<(), String> {
    let w = format!("{what} '{}'", s.id);
    if mar && !MAR_CATEGORIES.iter().any(|c| c.0 == s.category) {
        return Err(format!("{w}: '{}' is not a material category ({})", s.category, MAR_CATEGORIES.iter().map(|c| c.0).collect::<Vec<_>>().join(", ")));
    }
    if s.description.trim().is_empty() {
        return Err(format!("{w}: enter a description"));
    }
    opt_date(&format!("{w} submitted"), &s.submitted)?;
    let reviewed = opt_date(&format!("{w} reviewed"), &s.reviewed_date)?;
    if s.status.decided() && (blank(&s.reviewed_by) || reviewed.is_none()) {
        return Err(format!("{w}: enter who reviewed it and the date before recording a decision"));
    }
    if matches!(s.status, Review::Rejected | Review::ApprovedWithComments) && blank(&s.comment) {
        return Err(format!("{w}: a rejection or an approval with comments needs the comment"));
    }
    Ok(())
}

impl SupervisionInputs {
    pub fn check(&self) -> Result<(), String> {
        self.ipc.check()?;
        self.contract.check()?;
        self.geotech.check()?;
        if let Some(h) = &self.handover {
            opt_date("handover date", &h.date)?;
            opt_date("start date", &h.start_date)?;
            finite("handover easting", h.easting_m)?;
            finite("handover northing", h.northing_m)?;
            finite("benchmark level", h.benchmark_level_m)?;
            for s in &h.signatories {
                opt_date(&format!("signature of {}", s.role), &s.signed_date)?;
                if s.signed_date.is_some() && s.name.trim().is_empty() {
                    return Err(format!("handover: the {} signed with no name", s.role));
                }
            }
        }
        unique("MAR", self.mar.iter().map(|s| s.id.as_str()))?;
        for s in &self.mar {
            check_submittal("MAR", s, true)?;
        }
        unique("shop drawing", self.shop_drawings.iter().map(|s| s.id.as_str()))?;
        for s in &self.shop_drawings {
            check_submittal("shop drawing", s, false)?;
        }
        unique("WIR", self.wir.iter().map(|w| w.id.as_str()))?;
        for w in &self.wir {
            let n = format!("WIR '{}'", w.id);
            if !WIR_STAGES.iter().any(|s| s.0 == w.stage) {
                return Err(format!("{n}: '{}' is not a stage ({})", w.stage, WIR_STAGES.iter().map(|s| s.0).collect::<Vec<_>>().join(", ")));
            }
            if w.element.trim().is_empty() {
                return Err(format!("{n}: name the element or zone"));
            }
            opt_date(&format!("{n} requested"), &w.requested)?;
            let decided = opt_date(&format!("{n} decided"), &w.decided_date)?;
            for h in &w.hold_points {
                if h.text.trim().is_empty() {
                    return Err(format!("{n}: a hold point has no text"));
                }
                opt_date(&format!("{n} hold point signature"), &h.signed_date)?;
            }
            if w.status.decided() && (blank(&w.inspector) || decided.is_none()) {
                return Err(format!("{n}: enter the inspector and the date before recording a decision"));
            }
            if w.status.approved() {
                if w.hold_points.is_empty() {
                    return Err(format!("{n}: cannot be approved with no hold point (use the stage's hold points)"));
                }
                if let Some(h) = w.hold_points.iter().find(|h| !h.signed()) {
                    return Err(format!("{n}: hold point '{}' is not signed, so the work cannot be approved", h.text));
                }
            }
            if matches!(w.status, Review::Rejected | Review::ApprovedWithComments) && blank(&w.comment) {
                return Err(format!("{n}: a rejection or an approval with comments needs the comment"));
            }
        }
        unique("MIR", self.mir.iter().map(|m| m.id.as_str()))?;
        for m in &self.mir {
            let n = format!("MIR '{}'", m.id);
            opt_date(&format!("{n} delivered"), &m.delivered)?;
            if m.quantity.is_some_and(|q| !q.is_finite() || q <= 0.0) {
                return Err(format!("{n}: the quantity must be a positive number"));
            }
            match self.mar.iter().find(|s| s.id == m.mar) {
                None => return Err(format!("{n}: '{}' is not a MAR of this project", m.mar)),
                Some(s) if m.status.approved() && !s.status.approved() => return Err(format!("{n}: MAR '{}' is not approved, so the delivery cannot be accepted against it", m.mar)),
                Some(_) => {}
            }
            if m.status.decided() && blank(&m.checked_by) {
                return Err(format!("{n}: enter who checked the delivery before recording a decision"));
            }
            if matches!(m.status, Review::Rejected | Review::ApprovedWithComments) && blank(&m.comment) {
                return Err(format!("{n}: a rejection or an approval with comments needs the comment"));
            }
        }
        unique("lab test", self.lab.iter().map(|t| t.id.as_str()))?;
        for t in &self.lab {
            let n = format!("lab test '{}'", t.id);
            let sampled = opt_date(&format!("{n} sampled"), &t.sampled)?;
            let tested = opt_date(&format!("{n} tested"), &t.tested)?;
            if let (Some(a), Some(b)) = (sampled, tested)
                && b.to_days() < a.to_days()
            {
                return Err(format!("{n}: tested before it was sampled"));
            }
            for (label, v) in [("measured", t.measured), ("required", t.required), ("design f'c", t.design_fc_mpa)] {
                finite(&format!("{n} {label}"), v)?;
            }
            if t.measured.is_some() && tested.is_none() {
                return Err(format!("{n}: enter the test date with the result"));
            }
            if t.required.is_some() && blank(&t.required_source) {
                return Err(format!("{n}: the acceptance value needs its source (specification clause, drawing note or instruction)"));
            }
            if t.design_fc_mpa.is_some_and(|f| f <= 0.0) || t.measured.is_some_and(|m| m < 0.0) {
                return Err(format!("{n}: strength and measured values cannot be negative or zero f'c"));
            }
        }
        unique("NCR", self.ncr.iter().map(|n| n.id.as_str()))?;
        for r in &self.ncr {
            let n = format!("NCR '{}'", r.id);
            if r.description.trim().is_empty() {
                return Err(format!("{n}: enter the description"));
            }
            let raised = opt_date(&format!("{n} date"), &r.date)?;
            opt_date(&format!("{n} due"), &r.due)?;
            let closed = opt_date(&format!("{n} closed"), &r.closed_date)?;
            if let Some(c) = closed {
                if blank(&r.corrective_action) || blank(&r.closed_by) {
                    return Err(format!("{n}: a closed NCR needs the corrective action and who closed it"));
                }
                if raised.is_some_and(|d| c.to_days() < d.to_days()) {
                    return Err(format!("{n}: closed before it was raised"));
                }
            }
        }
        unique("RFI", self.rfi.iter().map(|r| r.id.as_str()))?;
        for r in &self.rfi {
            let n = format!("RFI '{}'", r.id);
            if r.subject.trim().is_empty() {
                return Err(format!("{n}: enter the subject"));
            }
            let raised = opt_date(&format!("{n} raised"), &r.raised)?;
            opt_date(&format!("{n} due"), &r.due)?;
            let answered = opt_date(&format!("{n} answered"), &r.answered)?;
            if answered.is_some() && blank(&r.answer) {
                return Err(format!("{n}: an answered RFI needs the answer"));
            }
            if let (Some(a), Some(b)) = (raised, answered)
                && b.to_days() < a.to_days()
            {
                return Err(format!("{n}: answered before it was raised"));
            }
        }
        unique("MOM", self.mom.iter().map(|m| m.id.as_str()))?;
        for m in &self.mom {
            opt_date(&format!("MOM '{}' date", m.id), &m.date)?;
            for i in &m.items {
                opt_date(&format!("MOM '{}' action due", m.id), &i.due)?;
            }
        }
        unique("variation", self.variations.iter().map(|v| v.id.as_str()))?;
        for v in &self.variations {
            let n = format!("variation '{}'", v.id);
            if v.description.trim().is_empty() {
                return Err(format!("{n}: enter the description"));
            }
            finite(&format!("{n} cost"), v.cost_delta)?;
            let ad = opt_date(&format!("{n} approval date"), &v.approved_date)?;
            if v.status == VoStatus::Approved && (ad.is_none() || blank(&v.source)) {
                return Err(format!("{n}: an approved variation needs the approval date and the owner's reference"));
            }
        }
        let mut last: Option<(i64, f64, f64)> = None;
        for p in &self.progress {
            let d = parse_date("progress period end", &p.period_end)?;
            for (label, v) in [("planned", p.planned_pct), ("actual", p.actual_pct)] {
                if let Some(x) = v
                    && !(0.0..=100.0).contains(&x)
                {
                    return Err(format!("progress {}: {label} must be between 0 and 100", p.period_end));
                }
            }
            if let Some((ld, lp, la)) = last {
                if d.to_days() <= ld {
                    return Err(format!("progress {}: periods must be in increasing date order", p.period_end));
                }
                if p.planned_pct.is_some_and(|x| x < lp) || p.actual_pct.is_some_and(|x| x < la) {
                    return Err(format!("progress {}: cumulative progress cannot go down", p.period_end));
                }
            }
            last = Some((d.to_days(), p.planned_pct.unwrap_or(last.map_or(0.0, |l| l.1)), p.actual_pct.unwrap_or(last.map_or(0.0, |l| l.2))));
        }
        unique("snag", self.snags.iter().map(|s| s.id.as_str()))?;
        for s in &self.snags {
            let n = format!("snag '{}'", s.id);
            if s.room.trim().is_empty() || s.description.trim().is_empty() {
                return Err(format!("{n}: enter the room and the description"));
            }
            opt_date(&format!("{n} found"), &s.found)?;
            let fixed = opt_date(&format!("{n} fixed"), &s.fixed_date)?;
            if s.status != SnagStatus::Open && fixed.is_none() {
                return Err(format!("{n}: enter the date it was fixed"));
            }
            if s.status == SnagStatus::Verified && blank(&s.verified_by) {
                return Err(format!("{n}: enter who verified the fix"));
            }
        }
        let mut seen = BTreeSet::new();
        for r in &self.closeout.records {
            if !CLOSEOUT_ITEMS.iter().any(|c| c.0 == r.item) {
                return Err(format!("close-out: '{}' is not a checklist item", r.item));
            }
            if !seen.insert(r.item.as_str()) {
                return Err(format!("close-out: '{}' has two records", r.item));
            }
            opt_date(&format!("close-out '{}'", r.item), &r.date)?;
            if r.done && (blank(&r.by) || blank(&r.date)) {
                return Err(format!("close-out '{}': enter who confirmed it and the date", r.item));
            }
        }
        for w in &self.closeout.warranties {
            if w.item.trim().is_empty() {
                return Err("close-out: a warranty has no item".into());
            }
            opt_date(&format!("warranty '{}' start", w.item), &w.start)?;
            if w.months == Some(0) {
                return Err(format!("warranty '{}': the period must be at least one month", w.item));
            }
        }
        Ok(())
    }
}

/// What the report needs from outside the registers.
#[derive(Debug, Clone, Copy, Default)]
pub struct Context<'a> {
    /// Today, for overdue detection. Without it nothing is called overdue.
    pub today: Option<Date>,
    /// The room keys of the plan, to catch a snag on a room that does not exist.
    pub room_keys: Option<&'a [String]>,
    /// The finish date of the baseline schedule, so the approved variation days can move it. Without it no revised finish is computed.
    pub baseline_finish: Option<Date>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Table {
    pub id: &'static str,
    pub title: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Kpi {
    pub id: &'static str,
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SCurvePoint {
    pub period_end: String,
    pub planned_pct: Option<f64>,
    pub actual_pct: Option<f64>,
    /// Actual minus planned, in percentage points.
    pub variance_pct: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SupervisionReport {
    pub tables: Vec<Table>,
    pub kpis: Vec<Kpi>,
    pub s_curve: Vec<SCurvePoint>,
    /// Things that need the user's attention, by record.
    pub issues: Vec<String>,
    /// Why the project cannot be closed out yet.
    pub blockers: Vec<String>,
    pub closeout_ready: bool,
}

fn add_months(d: Date, months: u32) -> Date {
    let total = d.y as i64 * 12 + (d.m as i64 - 1) + months as i64;
    let (y, m) = ((total / 12) as i32, (total % 12) as u32 + 1);
    let mut day = d.d;
    loop {
        if let Some(r) = Date::new(y, m, day) {
            return r;
        }
        day -= 1;
    }
}

fn num(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r == r.trunc() { format!("{r:.0}") } else { format!("{r}") }
}

fn or_dash(s: &Option<String>) -> String {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("—").to_string()
}

pub fn report(inp: &SupervisionInputs, ctx: &Context, arabic: bool) -> SupervisionReport {
    let t = |ar: &'static str, en: &'static str| if arabic { ar.to_string() } else { en.to_string() };
    let past = |d: &Option<String>| match (ctx.today, d.as_deref().and_then(|s| Date::parse(s.trim()))) {
        (Some(today), Some(due)) => due.to_days() < today.to_days(),
        _ => false,
    };
    let mut issues: Vec<String> = Vec::new();
    let mut blockers: Vec<String> = Vec::new();
    let mut tables: Vec<Table> = Vec::new();

    // Site handover.
    {
        let mut rows = Vec::new();
        match &inp.handover {
            None => issues.push(t("محضر تسليم الموقع لم يُسجَّل", "The site handover minutes are not recorded")),
            Some(h) => {
                let mut row = |k: String, v: String| rows.push(vec![k, v]);
                row(t("تاريخ التسليم", "Handover date"), or_dash(&h.date));
                row(t("مرجع الموقع", "Site reference"), or_dash(&h.site_ref));
                row(t("الإحداثي الشرقي (م)", "Easting (m)"), h.easting_m.map_or("—".into(), num));
                row(t("الإحداثي الشمالي (م)", "Northing (m)"), h.northing_m.map_or("—".into(), num));
                row(t("نظام الإحداثيات", "Coordinate system"), or_dash(&h.crs));
                row(t("نقطة المنسوب المرجعية", "Benchmark"), or_dash(&h.benchmark_description));
                row(t("منسوب النقطة (م)", "Benchmark level (m)"), h.benchmark_level_m.map_or("—".into(), num));
                row(t("مرجع المنسوب", "Level datum"), or_dash(&h.benchmark_datum));
                row(t("تاريخ بدء الأعمال", "Start date"), or_dash(&h.start_date));
                for s in &h.signatories {
                    row(format!("{}: {}", s.role, s.name), or_dash(&s.signed_date));
                }
                if h.easting_m.is_none() || h.northing_m.is_none() || blank(&h.crs) {
                    issues.push(t("محضر التسليم: الإحداثيات أو نظامها غير مدخلة", "Handover: the coordinates or their coordinate system are not entered"));
                }
                if h.benchmark_level_m.is_none() || blank(&h.benchmark_datum) {
                    issues.push(t("محضر التسليم: منسوب النقطة المرجعية أو مرجعه غير مدخل", "Handover: the benchmark level or its datum is not entered"));
                }
                if h.signatories.is_empty() || h.signatories.iter().any(|s| s.signed_date.is_none()) {
                    issues.push(t("محضر التسليم: توقيعات الأطراف غير مكتملة", "Handover: the signatures are incomplete"));
                }
            }
        }
        tables.push(Table { id: "handover", title: t("محضر تسليم الموقع", "Site handover minutes"), headers: vec![t("البند", "Item"), t("القيمة", "Value")], rows });
    }

    // MAR and shop drawings.
    let sub_headers = vec![t("الرقم", "No."), t("الفئة", "Category"), t("الوصف", "Description"), t("المرجع", "Reference"), t("التقديم", "Submitted"), t("الحالة", "Status"), t("المراجع / التاريخ", "Reviewer / date"), t("ملاحظة", "Comment")];
    let sub_row = |s: &Submittal, mar: bool| {
        let cat = if mar { MAR_CATEGORIES.iter().find(|c| c.0 == s.category).map_or(s.category.clone(), |c| if arabic { c.2.into() } else { c.1.into() }) } else { s.category.clone() };
        vec![s.id.clone(), cat, s.description.clone(), or_dash(&s.reference), or_dash(&s.submitted), s.status.label(arabic).into(), format!("{} {}", s.reviewed_by.as_deref().unwrap_or(""), s.reviewed_date.as_deref().unwrap_or("")).trim().to_string(), or_dash(&s.comment)]
    };
    tables.push(Table { id: "mar", title: t("طلبات اعتماد المواد (MAR)", "Material submittals (MAR)"), headers: sub_headers.clone(), rows: inp.mar.iter().map(|s| sub_row(s, true)).collect() });
    tables.push(Table { id: "shop", title: t("سجل المخططات التنفيذية", "Shop drawings log"), headers: sub_headers, rows: inp.shop_drawings.iter().map(|s| sub_row(s, false)).collect() });

    // WIR.
    {
        let mut rows = Vec::new();
        for w in &inp.wir {
            let stage = WIR_STAGES.iter().find(|s| s.0 == w.stage).map_or(w.stage.clone(), |s| if arabic { s.2.into() } else { s.1.into() });
            let signed = w.hold_points.iter().filter(|h| h.signed()).count();
            rows.push(vec![w.id.clone(), stage, w.element.clone(), or_dash(&w.requested), format!("{signed}/{}", w.hold_points.len()), w.status.label(arabic).into(), format!("{} {}", w.inspector.as_deref().unwrap_or(""), w.decided_date.as_deref().unwrap_or("")).trim().to_string(), or_dash(&w.comment)]);
            if !w.status.approved() {
                blockers.push(format!("WIR {}: {}", w.id, w.status.label(arabic)));
            }
            if w.status == Review::Submitted && past(&w.requested) && w.hold_points.iter().any(|h| !h.signed()) {
                issues.push(format!("WIR {}: {}", w.id, t("بانتظار توقيع نقاط التوقف", "hold points await signature")));
            }
        }
        tables.push(Table { id: "wir", title: t("طلبات فحص الأعمال (WIR)", "Work inspection requests (WIR)"), headers: vec![t("الرقم", "No."), t("المرحلة", "Stage"), t("العنصر", "Element"), t("الطلب", "Requested"), t("نقاط التوقف الموقعة", "Hold points signed"), t("الحالة", "Status"), t("المفتش / التاريخ", "Inspector / date"), t("ملاحظة", "Comment")], rows });
    }

    // MIR.
    tables.push(Table {
        id: "mir",
        title: t("طلبات فحص المواد (MIR)", "Material inspection requests (MIR)"),
        headers: vec![t("الرقم", "No."), "MAR".into(), t("التوريد", "Delivered"), t("الكمية", "Quantity"), t("إشعار التسليم", "Delivery note"), t("الحالة", "Status"), t("الفاحص", "Checked by"), t("ملاحظة", "Comment")],
        rows: inp.mir.iter().map(|m| vec![m.id.clone(), m.mar.clone(), or_dash(&m.delivered), m.quantity.map_or("—".into(), |q| format!("{} {}", num(q), m.unit.as_deref().unwrap_or("")).trim().to_string()), or_dash(&m.delivery_note), m.status.label(arabic).into(), or_dash(&m.checked_by), or_dash(&m.comment)]).collect(),
    });

    // Laboratory tests.
    {
        let mut rows = Vec::new();
        for l in &inp.lab {
            let v = l.verdict();
            let kind = match (l.kind, arabic) {
                (LabKind::Cube, false) => "Concrete cube",
                (LabKind::Proctor, false) => "Proctor compaction",
                (LabKind::Pressure, false) => "Pressure test",
                (LabKind::Other, false) => "Other",
                (LabKind::Cube, true) => "مكعب خرسانة",
                (LabKind::Proctor, true) => "دمك بروكتور",
                (LabKind::Pressure, true) => "اختبار ضغط",
                (LabKind::Other, true) => "أخرى",
            };
            let unit = l.unit.as_deref().unwrap_or("");
            let age = l.age_days();
            let ratio = match (l.kind, l.measured, l.design_fc_mpa) {
                (LabKind::Cube, Some(m), Some(fc)) => format!("{:.1}%", m / fc * 100.0),
                _ => "—".into(),
            };
            rows.push(vec![
                l.id.clone(),
                kind.into(),
                l.subject.clone(),
                age.map_or("—".into(), |a| a.to_string()),
                l.measured.map_or("—".into(), |m| format!("{} {unit}", num(m)).trim().to_string()),
                l.required.map_or("—".into(), |r| format!("{} {} {unit}", if l.rule == Rule::AtLeast { "≥" } else { "≤" }, num(r)).trim().to_string()),
                or_dash(&l.required_source),
                ratio,
                v.label(arabic).into(),
            ]);
            if v == Verdict::Fail {
                blockers.push(format!("{} {}: {}", t("اختبار", "test"), l.id, v.label(arabic)));
            }
            if v == Verdict::Unchecked {
                issues.push(format!("{} {}: {}", t("اختبار", "Lab test"), l.id, t("نتيجة بلا قيمة قبول: لا حكم", "a result with no acceptance value: no verdict")));
            }
            if l.kind == LabKind::Cube
                && let Some(a) = age
                && a != 7
                && a != 28
            {
                issues.push(format!("{} {}: {}", t("اختبار", "Lab test"), l.id, if arabic { format!("عمر المكعب {a} يوماً وليس 7 أو 28") } else { format!("cube age is {a} days, not 7 or 28") }));
            }
        }
        tables.push(Table { id: "lab", title: t("سجل الاختبارات المخبرية", "Laboratory test log"), headers: vec![t("الرقم", "No."), t("النوع", "Type"), t("موضوع الاختبار", "Subject"), t("العمر (يوم)", "Age (days)"), t("المقاس", "Measured"), t("القبول", "Acceptance"), t("مصدر القبول", "Acceptance source"), t("نسبة من f'c", "% of f'c"), t("الحكم", "Verdict")], rows });
    }

    // NCR.
    {
        let mut rows = Vec::new();
        for n in &inp.ncr {
            let closed = n.closed_date.is_some();
            let overdue = !closed && past(&n.due);
            rows.push(vec![n.id.clone(), or_dash(&n.date), or_dash(&n.element), n.description.clone(), or_dash(&n.severity), or_dash(&n.corrective_action), or_dash(&n.due), if closed { t("مغلق", "Closed") } else if overdue { t("متأخر", "OVERDUE") } else { t("مفتوح", "Open") }]);
            if !closed {
                blockers.push(format!("NCR {}: {}", n.id, t("مفتوح", "open")));
            }
            if overdue {
                issues.push(format!("NCR {}: {}", n.id, t("تجاوز موعد الإجراء التصحيحي", "corrective action is past its due date")));
            }
            if !closed && blank(&n.corrective_action) {
                issues.push(format!("NCR {}: {}", n.id, t("بلا إجراء تصحيحي", "no corrective action recorded")));
            }
        }
        tables.push(Table { id: "ncr", title: t("تقارير عدم المطابقة (NCR)", "Non-conformance reports (NCR)"), headers: vec![t("الرقم", "No."), t("التاريخ", "Date"), t("العنصر", "Element"), t("الوصف", "Description"), t("الشدة", "Severity"), t("الإجراء التصحيحي", "Corrective action"), t("الموعد", "Due"), t("الحالة", "State")], rows });
    }

    // RFI.
    {
        let mut rows = Vec::new();
        for r in &inp.rfi {
            let answered = r.answered.is_some();
            let overdue = !answered && past(&r.due);
            rows.push(vec![r.id.clone(), r.subject.clone(), or_dash(&r.bim_ref), or_dash(&r.raised), or_dash(&r.due), or_dash(&r.answer), if answered { t("مُجاب", "Answered") } else if overdue { t("متأخر", "OVERDUE") } else { t("مفتوح", "Open") }]);
            if overdue {
                issues.push(format!("RFI {}: {}", r.id, t("تجاوز موعد الرد", "past its response date")));
            }
        }
        tables.push(Table { id: "rfi", title: t("طلبات المعلومات (RFI)", "Requests for information (RFI)"), headers: vec![t("الرقم", "No."), t("الموضوع", "Subject"), t("مرجع المخطط / العنصر", "Sheet / element"), t("التاريخ", "Raised"), t("الموعد", "Due"), t("الرد", "Answer"), t("الحالة", "State")], rows });
    }

    // MOM.
    {
        let mut rows = Vec::new();
        for m in &inp.mom {
            for (i, it) in m.items.iter().enumerate() {
                rows.push(vec![if i == 0 { format!("{} — {}", m.id, m.title) } else { String::new() }, if i == 0 { or_dash(&m.date) } else { String::new() }, it.text.clone(), or_dash(&it.owner), or_dash(&it.due), if it.done { t("منجز", "Done") } else if past(&it.due) { t("متأخر", "OVERDUE") } else { t("مفتوح", "Open") }]);
                if !it.done && past(&it.due) {
                    issues.push(format!("MOM {}: {} — {}", m.id, it.text, t("إجراء متأخر", "action overdue")));
                }
            }
        }
        tables.push(Table { id: "mom", title: t("محاضر الاجتماعات (MOM)", "Minutes of meeting (MOM)"), headers: vec![t("الاجتماع", "Meeting"), t("التاريخ", "Date"), t("الإجراء", "Action"), t("المسؤول", "Owner"), t("الموعد", "Due"), t("الحالة", "State")], rows });
    }

    // Variation orders.
    let (mut vo_cost, mut vo_days, mut vo_pending_cost) = (0.0, 0i64, 0.0);
    {
        let mut rows = Vec::new();
        for v in &inp.variations {
            match v.status {
                VoStatus::Approved => {
                    vo_cost += v.cost_delta.unwrap_or(0.0);
                    vo_days += v.days_delta.unwrap_or(0);
                }
                VoStatus::Proposed => vo_pending_cost += v.cost_delta.unwrap_or(0.0),
                VoStatus::Rejected => {}
            }
            if v.cost_delta.is_none() || v.days_delta.is_none() {
                issues.push(format!("VO {}: {}", v.id, t("أثر التكلفة أو المدة غير مدخل ولم يُحتسب", "cost or time impact not entered and not counted")));
            }
            let st = match (v.status, arabic) {
                (VoStatus::Proposed, false) => "Proposed",
                (VoStatus::Approved, false) => "Approved",
                (VoStatus::Rejected, false) => "Rejected",
                (VoStatus::Proposed, true) => "مقترح",
                (VoStatus::Approved, true) => "معتمد",
                (VoStatus::Rejected, true) => "مرفوض",
            };
            rows.push(vec![v.id.clone(), v.description.clone(), v.cost_delta.map_or("—".into(), num), v.days_delta.map_or("—".into(), |d| d.to_string()), st.into(), or_dash(&v.approved_date), or_dash(&v.source)]);
        }
        tables.push(Table { id: "vo", title: t("أوامر التغيير (VO)", "Variation orders (VO)"), headers: vec![t("الرقم", "No."), t("الوصف", "Description"), t("أثر التكلفة", "Cost impact"), t("أثر المدة (يوم)", "Time impact (days)"), t("الحالة", "Status"), t("تاريخ الاعتماد", "Approved"), t("مرجع المالك", "Owner reference")], rows });
        // The 4D effect: approved days (calendar days, signed) move the baseline finish.
        let mut rows = Vec::new();
        match ctx.baseline_finish {
            Some(f) => {
                let revised = Date::from_days(f.to_days() + vo_days);
                rows.push(vec![t("الإنجاز في الجدول الأساسي", "Baseline finish"), f.iso()]);
                rows.push(vec![t("أيام التغيير المعتمدة (تقويمية)", "Approved VO days (calendar)"), vo_days.to_string()]);
                rows.push(vec![t("الإنجاز بعد التعديل", "Revised finish"), revised.iso()]);
            }
            None => issues.push(t("لا جدول أساسي محسوب: لم يُحتسب الإنجاز بعد أوامر التغيير", "No baseline schedule is computed: the finish after variation orders was not worked out").to_string()),
        }
        if !rows.is_empty() {
            tables.push(Table { id: "vo_schedule", title: t("أثر أوامر التغيير على الجدول", "Variation effect on the schedule"), headers: vec![t("البند", "Item"), t("القيمة", "Value")], rows });
        }
    }

    // Progress and the S-curve.
    let mut s_curve = Vec::new();
    {
        let mut rows = Vec::new();
        for p in &inp.progress {
            let variance = p.planned_pct.zip(p.actual_pct).map(|(a, b)| (b - a) * 10.0).map(|v| v.round() / 10.0);
            s_curve.push(SCurvePoint { period_end: p.period_end.clone(), planned_pct: p.planned_pct, actual_pct: p.actual_pct, variance_pct: variance });
            rows.push(vec![p.period_end.clone(), p.planned_pct.map_or("—".into(), num), p.actual_pct.map_or("—".into(), num), variance.map_or("—".into(), num), p.manpower.map_or("—".into(), |v| v.to_string()), p.equipment.map_or("—".into(), |v| v.to_string()), or_dash(&p.blockers), p.photos.len().to_string()]);
            if p.planned_pct.is_none() || p.actual_pct.is_none() {
                issues.push(format!("{} {}: {}", t("تقرير", "Progress"), p.period_end, t("نسبة مخططة أو فعلية غير مدخلة", "planned or actual percentage not entered")));
            }
        }
        tables.push(Table { id: "progress", title: t("تقارير التقدم", "Progress reports"), headers: vec![t("نهاية الفترة", "Period end"), t("المخطط %", "Planned %"), t("الفعلي %", "Actual %"), t("الفرق (نقطة)", "Variance (pts)"), t("العمالة", "Manpower"), t("المعدات", "Equipment"), t("المعوقات", "Blockers"), t("صور", "Photos")], rows });
    }

    // Snags.
    {
        let mut rows = Vec::new();
        for s in &inp.snags {
            let st = match (s.status, arabic) {
                (SnagStatus::Open, false) => "Open",
                (SnagStatus::Fixed, false) => "Fixed",
                (SnagStatus::Verified, false) => "Verified",
                (SnagStatus::Open, true) => "مفتوح",
                (SnagStatus::Fixed, true) => "تم الإصلاح",
                (SnagStatus::Verified, true) => "تم التحقق",
            };
            rows.push(vec![s.id.clone(), s.room.clone(), s.description.clone(), or_dash(&s.found), st.into(), or_dash(&s.fixed_date), or_dash(&s.verified_by)]);
            if s.status != SnagStatus::Verified {
                blockers.push(format!("{} {}: {}", t("ملاحظة", "Snag"), s.id, st));
            }
            if let Some(keys) = ctx.room_keys
                && !keys.contains(&s.room)
            {
                issues.push(format!("{} {}: {}", t("ملاحظة", "Snag"), s.id, if arabic { format!("الغرفة '{}' ليست في المخطط", s.room) } else { format!("room '{}' is not in the plan", s.room) }));
            }
        }
        tables.push(Table { id: "snags", title: t("قائمة الملاحظات حسب الغرفة", "Snag list by room"), headers: vec![t("الرقم", "No."), t("الغرفة", "Room"), t("الوصف", "Description"), t("الاكتشاف", "Found"), t("الحالة", "Status"), t("الإصلاح", "Fixed"), t("تحقق", "Verified by")], rows });
    }

    // Close-out.
    {
        let mut rows = Vec::new();
        for (id, en, ar) in CLOSEOUT_ITEMS {
            let r = inp.closeout.records.iter().find(|r| r.item == id);
            let done = r.is_some_and(|r| r.done);
            rows.push(vec![if arabic { ar.into() } else { en.into() }, if done { t("تم", "Done") } else { t("معلّق", "Pending") }, r.map_or("—".into(), |r| format!("{} {}", r.by.as_deref().unwrap_or(""), r.date.as_deref().unwrap_or("")).trim().to_string()), r.map_or("—".into(), |r| or_dash(&r.note))]);
            if !done {
                blockers.push(format!("{}: {}", t("إغلاق", "Close-out"), if arabic { ar } else { en }));
            }
        }
        tables.push(Table { id: "closeout", title: t("قائمة الإغلاق والتسليم", "Close-out checklist"), headers: vec![t("البند", "Item"), t("الحالة", "State"), t("المؤكِّد / التاريخ", "Confirmed by / date"), t("ملاحظة", "Note")], rows });
        let mut rows = Vec::new();
        for w in &inp.closeout.warranties {
            let end = w.start.as_deref().and_then(|s| Date::parse(s.trim())).zip(w.months).map(|(d, m)| add_months(d, m).iso());
            if end.is_none() {
                issues.push(format!("{} '{}': {}", t("ضمان", "Warranty"), w.item, t("البداية أو المدة غير مدخلة: لا تاريخ انتهاء", "start or period not entered: no expiry date")));
            }
            rows.push(vec![w.item.clone(), or_dash(&w.supplier), or_dash(&w.start), w.months.map_or("—".into(), |m| m.to_string()), end.unwrap_or_else(|| "—".into()), or_dash(&w.doc_ref)]);
        }
        tables.push(Table { id: "warranties", title: t("سجل الضمانات", "Warranty register"), headers: vec![t("البند", "Item"), t("المورد", "Supplier"), t("البداية", "Start"), t("المدة (شهر)", "Months"), t("الانتهاء", "Expires"), t("المستند", "Document")], rows });
        tables.push(Table {
            id: "manuals",
            title: t("أدلة التشغيل والصيانة", "O&M manuals"),
            headers: vec![t("الدليل", "Manual"), t("المستند", "Document"), t("الاستلام", "Received")],
            rows: inp.closeout.manuals.iter().map(|m| vec![m.title.clone(), or_dash(&m.doc_ref), if m.received { t("مستلم", "Received") } else { t("معلّق", "Pending") }]).collect(),
        });
    }

    let count = |f: &dyn Fn() -> usize| f().to_string();
    let kpis = vec![
        Kpi { id: "mar_approved", label: t("MAR معتمدة", "MAR approved"), value: format!("{}/{}", inp.mar.iter().filter(|s| s.status.approved()).count(), inp.mar.len()) },
        Kpi { id: "wir_approved", label: t("WIR معتمدة", "WIR approved"), value: format!("{}/{}", inp.wir.iter().filter(|w| w.status.approved()).count(), inp.wir.len()) },
        Kpi { id: "lab_fail", label: t("اختبارات غير مطابقة", "Failed tests"), value: count(&|| inp.lab.iter().filter(|l| l.verdict() == Verdict::Fail).count()) },
        Kpi { id: "ncr_open", label: t("NCR مفتوحة", "Open NCR"), value: count(&|| inp.ncr.iter().filter(|n| n.closed_date.is_none()).count()) },
        Kpi { id: "rfi_open", label: t("RFI مفتوحة", "Open RFI"), value: count(&|| inp.rfi.iter().filter(|r| r.answered.is_none()).count()) },
        Kpi { id: "snag_open", label: t("ملاحظات غير محقّقة", "Unverified snags"), value: count(&|| inp.snags.iter().filter(|s| s.status != SnagStatus::Verified).count()) },
        Kpi { id: "vo_cost", label: t("أثر أوامر التغيير المعتمدة (تكلفة)", "Approved VO cost impact"), value: num(vo_cost) },
        Kpi { id: "vo_days", label: t("أثر أوامر التغيير المعتمدة (أيام)", "Approved VO time impact (days)"), value: vo_days.to_string() },
        Kpi { id: "vo_pending", label: t("أوامر مقترحة (تكلفة)", "Proposed VO cost"), value: num(vo_pending_cost) },
        Kpi { id: "actual_pct", label: t("آخر نسبة إنجاز فعلية %", "Latest actual progress %"), value: inp.progress.iter().rev().find_map(|p| p.actual_pct).map_or("—".into(), num) },
    ];
    SupervisionReport { closeout_ready: blockers.is_empty(), tables, kpis, s_curve, issues, blockers }
}

impl SupervisionReport {
    fn s_curve_svg(&self) -> String {
        let pts: Vec<&SCurvePoint> = self.s_curve.iter().collect();
        if pts.len() < 2 {
            return String::new();
        }
        let (w, h, pad) = (640.0, 240.0, 30.0);
        let x = |i: usize| pad + (w - 2.0 * pad) * i as f64 / (pts.len() - 1) as f64;
        let y = |v: f64| h - pad - (h - 2.0 * pad) * v / 100.0;
        let line = |f: &dyn Fn(&SCurvePoint) -> Option<f64>, color: &str| {
            let p: Vec<String> = pts.iter().enumerate().filter_map(|(i, s)| f(s).map(|v| format!("{:.1},{:.1}", x(i), y(v)))).collect();
            format!("<polyline fill=\"none\" stroke=\"{color}\" stroke-width=\"2\" points=\"{}\"/>", p.join(" "))
        };
        format!(
            "<svg viewBox=\"0 0 {w} {h}\" width=\"{w}\" height=\"{h}\" style=\"max-width:100%\"><rect x=\"{pad}\" y=\"{pad}\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"#999\"/>{}{}\
             <text x=\"{pad}\" y=\"14\" font-size=\"11\">planned (grey) · actual (blue)</text></svg>",
            w - 2.0 * pad,
            h - 2.0 * pad,
            line(&|s| s.planned_pct, "#666"),
            line(&|s| s.actual_pct, "#1d4ed8")
        )
    }

    pub fn html(&self, title: &str, arabic: bool) -> String {
        let t = |ar: &'static str, en: &'static str| if arabic { ar } else { en };
        let mut h = format!(
            "<!doctype html><html lang=\"{}\" dir=\"{}\"><head><meta charset=\"utf-8\"><title>{}</title><style>\
             body{{font-family:'Segoe UI',Tahoma,sans-serif;margin:28px;color:#111}} h1{{font-size:22px;margin:0 0 6px}} h2{{font-size:15px;margin:22px 0 6px;border-bottom:2px solid #333;padding-bottom:3px}}\
             table{{border-collapse:collapse;width:100%;font-size:11.5px}} th,td{{border:1px solid #999;padding:4px 6px;text-align:start;vertical-align:top}} th{{background:#eee}}\
             .kpi{{display:inline-block;border:1px solid #999;padding:6px 10px;margin:0 6px 6px 0;min-width:120px}} .kpi b{{display:block;font-size:17px}} .warn{{color:#a33}}\
             .sign{{margin-top:18px;display:flex;gap:40px}} .sign div{{flex:1;border-top:1px solid #333;padding-top:4px;font-size:11px}} @page{{size:A4 landscape;margin:12mm}} @media print{{body{{margin:0}} h2{{break-after:avoid}} tr{{break-inside:avoid}}}}</style></head><body>",
            if arabic { "ar" } else { "en" },
            if arabic { "rtl" } else { "ltr" },
            esc(title)
        );
        h.push_str(&format!("<h1>{} — {}</h1>", t("سجلات الإشراف وضبط الجودة", "Supervision and QA/QC registers"), esc(title)));
        for k in &self.kpis {
            h.push_str(&format!("<div class=\"kpi\">{}<b>{}</b></div>", esc(&k.label), esc(&k.value)));
        }
        let curve = self.s_curve_svg();
        for tb in &self.tables {
            h.push_str(&format!("<h2>{}</h2>", esc(&tb.title)));
            if tb.id == "progress" {
                h.push_str(&curve);
            }
            h.push_str("<table><thead><tr>");
            for c in &tb.headers {
                h.push_str(&format!("<th>{}</th>", esc(c)));
            }
            h.push_str("</tr></thead><tbody>");
            if tb.rows.is_empty() {
                h.push_str(&format!("<tr><td colspan=\"{}\">{}</td></tr>", tb.headers.len(), t("لا سجلات", "No records")));
            }
            for r in &tb.rows {
                h.push_str("<tr>");
                for c in r {
                    h.push_str(&format!("<td>{}</td>", esc(c)));
                }
                h.push_str("</tr>");
            }
            h.push_str("</tbody></table>");
        }
        if !self.issues.is_empty() {
            h.push_str(&format!("<h2>{}</h2><ul class=\"warn\">", t("تنبيهات", "Attention")));
            for i in &self.issues {
                h.push_str(&format!("<li>{}</li>", esc(i)));
            }
            h.push_str("</ul>");
        }
        h.push_str(&format!("<h2>{}: {}</h2>", t("جاهزية الإغلاق", "Close-out readiness"), if self.closeout_ready { t("جاهز", "Ready") } else { t("غير جاهز", "Not ready") }));
        if !self.blockers.is_empty() {
            h.push_str("<ul>");
            for b in &self.blockers {
                h.push_str(&format!("<li>{}</li>", esc(b)));
            }
            h.push_str("</ul>");
        }
        h.push_str(&format!("<div class=\"sign\"><div>{}</div><div>{}</div><div>{}</div></div></body></html>", t("المقاول", "Contractor"), t("المشرف / الاستشاري", "Supervisor / consultant"), t("المالك", "Owner")));
        h
    }

    pub fn workbook(&self, arabic: bool) -> Workbook {
        let sheet_name = |s: &str, used: &mut Vec<String>| {
            let clean: String = s.chars().filter(|c| !matches!(c, '[' | ']' | ':' | '*' | '?' | '/' | '\\')).take(31).collect();
            let mut n = clean.clone();
            let mut k = 2;
            while used.contains(&n) {
                n = format!("{}{k}", clean.chars().take(28).collect::<String>());
                k += 1;
            }
            used.push(n.clone());
            n
        };
        let mut used = Vec::new();
        let text = |s: &str, st: u8| (Cell::Text(s.to_string()), st);
        let mut sheets = Vec::new();
        let head: Vec<(Cell, u8)> = [if arabic { "المؤشر" } else { "Indicator" }, if arabic { "القيمة" } else { "Value" }].iter().map(|l| text(l, style::HEADER)).collect();
        let mut rows = vec![head];
        for k in &self.kpis {
            rows.push(vec![text(&k.label, style::TEXT), text(&k.value, style::TEXT)]);
        }
        let last = rows.len();
        sheets.push(Sheet { name: sheet_name(if arabic { "الملخص" } else { "Summary" }, &mut used), rows, widths: vec![44.0, 18.0], rtl: arabic, header: true, filter_to: last });
        for tb in &self.tables {
            let mut rows = vec![tb.headers.iter().map(|l| text(l, style::HEADER)).collect::<Vec<_>>()];
            for r in &tb.rows {
                rows.push(r.iter().map(|c| text(c, style::TEXT)).collect());
            }
            let last = rows.len();
            let widths = vec![22.0; tb.headers.len()];
            sheets.push(Sheet { name: sheet_name(&tb.title, &mut used), rows, widths, rtl: arabic, header: true, filter_to: last });
        }
        Workbook { sheets }
    }
}

/// One room rectangle of a level plan, in millimetres, with the key the brief gives it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanRoom {
    pub key: String,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

/// A level's rooms, for the issues plan.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanLevel {
    pub name: String,
    pub rooms: Vec<PlanRoom>,
}

/// The issues plan: each level's rooms drawn to scale with a red marker (the count) on every room that has an unverified snag and a
/// blue square on a room an open RFI names. An RFI whose reference is not a room key cannot be placed, and a snag whose room is not
/// on any level cannot be placed; both are listed under the drawing instead of being put somewhere. Returns `None` without any room.
pub fn issues_plan_svg(levels: &[PlanLevel], inp: &SupervisionInputs, arabic: bool) -> Option<String> {
    let t = |ar: &'static str, en: &'static str| if arabic { ar } else { en };
    let open_snags = |room: &str| inp.snags.iter().filter(|s| s.room == room && s.status != SnagStatus::Verified).count();
    let open_rfis = |room: &str| inp.rfis_open().filter(|r| r.bim_ref.as_deref().map(str::trim) == Some(room)).count();
    let mut placed: BTreeSet<&str> = BTreeSet::new();
    let mut out = String::new();
    for lv in levels.iter().filter(|l| !l.rooms.is_empty()) {
        let (x0, x1) = (lv.rooms.iter().map(|r| r.x).min()?, lv.rooms.iter().map(|r| r.x + r.w).max()?);
        let (y0, y1) = (lv.rooms.iter().map(|r| r.y).min()?, lv.rooms.iter().map(|r| r.y + r.h).max()?);
        let (w, h) = ((x1 - x0).max(1), (y1 - y0).max(1));
        let scale = 900.0 / w as f64;
        let (pw, ph) = (w as f64 * scale, h as f64 * scale);
        out.push_str(&format!("<h3>{}</h3><svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-10 -10 {:.0} {:.0}\" width=\"100%\" style=\"max-width:940px;border:1px solid #888\">", esc(&lv.name), pw + 20.0, ph + 20.0));
        for r in &lv.rooms {
            placed.insert(r.key.as_str());
            let (x, y) = ((r.x - x0) as f64 * scale, (y1 - r.y - r.h) as f64 * scale);
            let (rw, rh) = (r.w as f64 * scale, r.h as f64 * scale);
            out.push_str(&format!("<rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{rw:.1}\" height=\"{rh:.1}\" fill=\"#f8fafc\" stroke=\"#334155\" stroke-width=\"1.5\"/>"));
            out.push_str(&format!("<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"13\" text-anchor=\"middle\" fill=\"#334155\">{}</text>", x + rw / 2.0, y + rh / 2.0, esc(&r.key)));
            let (sn, rf) = (open_snags(&r.key), open_rfis(&r.key));
            if sn > 0 {
                out.push_str(&format!("<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"11\" fill=\"#b91c1c\"/><text x=\"{:.1}\" y=\"{:.1}\" font-size=\"13\" font-weight=\"700\" text-anchor=\"middle\" fill=\"#fff\">{sn}</text>", x + 16.0, y + 16.0, x + 16.0, y + 20.5));
            }
            if rf > 0 {
                out.push_str(&format!("<rect x=\"{:.1}\" y=\"{:.1}\" width=\"22\" height=\"22\" fill=\"#1d4ed8\"/><text x=\"{:.1}\" y=\"{:.1}\" font-size=\"13\" font-weight=\"700\" text-anchor=\"middle\" fill=\"#fff\">{rf}</text>", x + rw - 30.0, y + 5.0, x + rw - 19.0, y + 21.0));
            }
        }
        out.push_str("</svg>");
    }
    if out.is_empty() {
        return None;
    }
    out.push_str(&format!("<p>● {} &nbsp; ■ {}</p>", t("ملاحظات غير محقّقة (العدد)", "Unverified snags (count)"), t("استفسارات مفتوحة تذكر الغرفة", "Open RFIs naming the room")));
    let unplaced_snags: Vec<String> = inp.snags.iter().filter(|s| s.status != SnagStatus::Verified && !placed.contains(s.room.as_str())).map(|s| format!("{} ({})", esc(&s.id), esc(&s.room))).collect();
    if !unplaced_snags.is_empty() {
        out.push_str(&format!("<p>{}: {}</p>", t("ملاحظات لا غرفة لها على المخطط", "Snags whose room is not on the plan"), unplaced_snags.join("، ")));
    }
    let unplaced_rfis: Vec<String> = inp.rfis_open().filter(|r| !r.bim_ref.as_deref().map(str::trim).is_some_and(|k| placed.contains(k))).map(|r| format!("{} ({})", esc(&r.id), esc(r.bim_ref.as_deref().unwrap_or("—")))).collect();
    if !unplaced_rfis.is_empty() {
        out.push_str(&format!("<p>{}: {}</p>", t("استفسارات لا يمكن تحديد موضعها (مرجعها ليس غرفة)", "RFIs that cannot be placed (the reference is not a room)"), unplaced_rfis.join("، ")));
    }
    Some(out)
}

impl SupervisionInputs {
    /// The RFIs not yet answered.
    pub fn rfis_open(&self) -> impl Iterator<Item = &Rfi> {
        self.rfi.iter().filter(|r| r.answered.is_none())
    }
}
