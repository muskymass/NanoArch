//! Field QA/QC inspection checklists, by construction stage.
//!
//! Six stages tied to the 4D schedule: excavation, reinforcement (before the pour), waterproofing, MEP pressure tests, MEP electrical
//! continuity tests and handover. Each stage has the checks an inspector walks through; a check that depends on a figure (a test
//! pressure, a coat thickness, a resistance) never carries that figure: it names the figure to RECORD from the project specification or
//! the drawings, and the inspector enters what was required and measured. The application states no acceptance limit.
//!
//! The status of every check (pending, pass, fail, not applicable), who inspected, when and the recorded value are stored in the
//! project (`meta.execution.qaqc`), so undo, redo and the `.nnx` file follow. A stage is `passed` only when every applicable check passed
//! and every check that asks for a recorded value has one; any `fail` makes the stage `failed`. Each stage shows the date it is due:
//! the start or finish of the schedule activity it must be done before or after.

use crate::execution::{Activity, Link, Trigger, blank, esc, link_date, parse_date};
use crate::xlsx::{Cell, Sheet, Workbook, style};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub struct ItemDef {
    pub id: &'static str,
    pub en: &'static str,
    pub ar: &'static str,
    /// The figure the inspector records (English, Arabic), when the check depends on one.
    pub record: Option<(&'static str, &'static str)>,
}

pub struct StageDef {
    pub id: &'static str,
    pub en: &'static str,
    pub ar: &'static str,
    /// The activity the inspection is tied to and which end of it.
    pub due_key: &'static str,
    pub due_at: Trigger,
    /// Why that date, in words.
    pub due_en: &'static str,
    pub due_ar: &'static str,
    pub items: &'static [ItemDef],
}

const fn it(id: &'static str, en: &'static str, ar: &'static str) -> ItemDef {
    ItemDef { id, en, ar, record: None }
}

const fn rec(id: &'static str, en: &'static str, ar: &'static str, re: &'static str, ra: &'static str) -> ItemDef {
    ItemDef { id, en, ar, record: Some((re, ra)) }
}

pub static STAGES: &[StageDef] = &[
    StageDef {
        id: "excavation",
        en: "Excavation and foundations",
        ar: "الحفر والأساسات",
        due_key: "footing_form",
        due_at: Trigger::Start,
        due_en: "before footing formwork starts",
        due_ar: "قبل بدء شدات القواعد",
        items: &[
            it("excavation.setout", "Setting-out points and the benchmark checked against the drawings", "مراجعة نقاط التوقيع والمنسوب المرجعي مع المخططات"),
            rec("excavation.level", "Excavation level and plan size match the drawings at every footing", "منسوب الحفر وأبعاده مطابقة للمخططات عند كل قاعدة", "measured level and size", "المنسوب والأبعاد المقيسة"),
            rec("excavation.soil", "Founding stratum confirmed by the engineer against the soil investigation report", "تأكيد المهندس لطبقة التأسيس مقارنةً بتقرير فحص التربة", "soil report reference", "مرجع تقرير التربة"),
            it("excavation.clean", "Base clean, free of water, loose material and organic matter", "القاع نظيف وخالٍ من الماء والمواد السائبة والعضوية"),
            it("excavation.safety", "Shoring and edge protection in place as in the method statement", "الدعم وحواجز الحافة قائمة كما في بيان الطريقة"),
            it("excavation.blinding", "Blinding or lean concrete placed where the drawings require it", "تم صب الخرسانة العادية حيث تشترطها المخططات"),
        ],
    },
    StageDef {
        id: "rebar",
        en: "Reinforcement and formwork (before the pour)",
        ar: "حديد التسليح والشدات (قبل الصب)",
        due_key: "footing_pour",
        due_at: Trigger::Start,
        due_en: "hold point before the first concrete pour",
        due_ar: "نقطة إيقاف قبل أول صبة خرسانية",
        items: &[
            it("rebar.marks", "Bar marks, diameters and counts match the bar bending schedule", "رموز الأسياخ وأقطارها وأعدادها مطابقة لجدول ثني الحديد"),
            rec("rebar.laps", "Spacing, laps and anchorage match the structural drawings", "التباعد والتراكب والتثبيت مطابقة للمخططات الإنشائية", "lap length required by the drawings", "طول التراكب المطلوب في المخططات"),
            rec("rebar.cover", "Cover blocks or spacers in place at the specified cover", "قطع التغطية موضوعة بالغطاء المحدد", "cover specified", "الغطاء الخرساني المحدد"),
            it("rebar.clean", "Reinforcement clean: no loose rust, oil, mud or form-release agent", "الحديد نظيف: بلا صدأ سائب أو زيت أو طين"),
            it("rebar.formwork", "Formwork dimensions, levels, ties and bracing checked", "فحص أبعاد الشدات ومناسيبها وربطها وتدعيمها"),
            it("rebar.embeds", "Sleeves, conduits and embedded items placed and coordinated with MEP", "وضع الأكمام والمواسير والعناصر المطمورة وتنسيقها مع الكهروميكانيك"),
            it("rebar.approval", "Engineer's written approval to pour", "موافقة المهندس الخطية على الصب"),
        ],
    },
    StageDef {
        id: "waterproofing",
        en: "Waterproofing",
        ar: "العزل المائي",
        due_key: "insulation",
        due_at: Trigger::Finish,
        due_en: "when the insulation and waterproofing activity finishes",
        due_ar: "عند انتهاء نشاط العزل",
        items: &[
            it("waterproofing.substrate", "Substrate clean, sound and cured as the manufacturer's data sheet requires", "السطح نظيف وسليم ومعالج كما تشترط نشرة المصنّع"),
            rec("waterproofing.product", "Product and batch match the approved submittal", "المنتج والدفعة مطابقان للمعتمد", "product and batch number", "المنتج ورقم الدفعة"),
            rec("waterproofing.thickness", "Number of coats and thickness as specified", "عدد الطبقات والسماكة كما في المواصفة", "thickness / coats measured", "السماكة وعدد الطبقات المقيسة"),
            it("waterproofing.details", "Upturns, corners, drains, joints and penetrations detailed as drawn", "الأركان والمصارف والفواصل والاختراقات منفذة كما في التفاصيل"),
            rec("waterproofing.flood", "Water or flood test done where the specification requires it", "اختبار الغمر أو الماء منفذ حيث تشترطه المواصفة", "test duration and result", "مدة الاختبار ونتيجته"),
            it("waterproofing.protection", "Protection layer placed before backfill, screed or tiling", "طبقة الحماية موضوعة قبل الردم أو اللياسة أو البلاط"),
        ],
    },
    StageDef {
        id: "mep_pressure",
        en: "MEP pressure and leak tests",
        ar: "اختبارات الضغط والتسرب للكهروميكانيك",
        due_key: "plaster",
        due_at: Trigger::Start,
        due_en: "before walls are closed by plastering",
        due_ar: "قبل إغلاق الجدران بالبلاستر",
        items: &[
            it("mep_pressure.material", "Pipe material, diameters and routes match the approved MEP drawings", "مادة المواسير وأقطارها ومساراتها مطابقة لمخططات الكهروميكانيك المعتمدة"),
            rec("mep_pressure.pressure", "Supply pipework pressure-tested at the pressure and duration in the project specification, with no drop", "اختبار ضغط مواسير التغذية بالضغط والمدة الواردين في مواصفة المشروع دون هبوط", "test pressure, duration and result", "ضغط الاختبار ومدته ونتيجته"),
            rec("mep_pressure.drain", "Drainage water-tested or smoke-tested; falls checked", "اختبار الصرف بالماء أو الدخان وفحص الميول", "method and result", "الطريقة والنتيجة"),
            it("mep_pressure.supports", "Supports, sleeves and fire-stopping at penetrations in place", "الحوامل والأكمام وسدّ الاختراقات ضد الحريق في أماكنها"),
            it("mep_pressure.hvac", "Ductwork and refrigerant lines inspected and tested per the specification", "فحص مجاري الهواء وخطوط التبريد واختبارها حسب المواصفة"),
            it("mep_pressure.witness", "Tests witnessed and signed by the consultant or supervisor", "الاختبارات بحضور الاستشاري أو المشرف وموقّعة منه"),
        ],
    },
    StageDef {
        id: "mep_continuity",
        en: "MEP electrical continuity and insulation tests",
        ar: "اختبارات استمرارية العزل الكهربائي",
        due_key: "plaster",
        due_at: Trigger::Start,
        due_en: "before walls are closed by plastering (first fix), repeated at second fix",
        due_ar: "قبل البلاستر (التمديد الأول) وتُعاد عند التمديد الثاني",
        items: &[
            it("mep_continuity.cables", "Cable sizes, types and circuit identification match the schedule", "أحجام الكابلات وأنواعها وترقيم الدوائر مطابقة للجداول"),
            rec("mep_continuity.continuity", "Continuity of protective and ring conductors tested", "اختبار استمرارية موصلات الحماية والحلقات", "readings recorded", "القراءات المسجلة"),
            rec("mep_continuity.insulation", "Insulation resistance tested at the voltage and limit in the specification", "اختبار مقاومة العزل بالجهد والحد الواردين في المواصفة", "voltage, limit and readings", "الجهد والحد والقراءات"),
            rec("mep_continuity.earth", "Earthing system tested", "اختبار نظام التأريض", "earth resistance reading", "قراءة مقاومة التأريض"),
            it("mep_continuity.boards", "Distribution boards labelled; protective devices tested", "لوحات التوزيع مرقمة وأجهزة الحماية مختبرة"),
            it("mep_continuity.witness", "Tests witnessed and signed by the consultant or supervisor", "الاختبارات بحضور الاستشاري أو المشرف وموقّعة منه"),
        ],
    },
    StageDef {
        id: "handover",
        en: "Testing, snagging and handover",
        ar: "الاختبار والملاحظات والتسليم",
        due_key: "handover",
        due_at: Trigger::Finish,
        due_en: "when the snagging and handover activity finishes",
        due_ar: "عند انتهاء نشاط الملاحظات والتسليم",
        items: &[
            it("handover.snags", "Snag list closed, or the open items agreed in writing", "إغلاق قائمة الملاحظات أو الاتفاق كتابياً على المتبقي"),
            it("handover.asbuilt", "As-built drawings delivered", "تسليم مخططات ما بعد التنفيذ"),
            it("handover.manuals", "Operation manuals, warranties and test certificates collected", "تجميع كتيبات التشغيل والضمانات وشهادات الاختبار"),
            rec("handover.utilities", "Utilities connected and meters read", "توصيل الخدمات وقراءة العدادات", "meter readings", "قراءات العدادات"),
            it("handover.authority", "Authority inspection records and completion documents filed as the authority requires", "حفظ سجلات فحص الجهة المختصة ووثائق الإتمام كما تشترط"),
            it("handover.keys", "Keys, spare materials and the handover record signed by the owner", "تسليم المفاتيح والمواد الاحتياطية وتوقيع المالك على محضر التسليم"),
        ],
    },
];

pub fn stage(id: &str) -> Option<&'static StageDef> {
    STAGES.iter().find(|s| s.id == id)
}

fn template_item(id: &str) -> Option<&'static ItemDef> {
    STAGES.iter().flat_map(|s| s.items.iter()).find(|i| i.id == id)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Pending,
    Pass,
    Fail,
    Na,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QaRecord {
    pub item: String,
    #[serde(default)]
    pub status: Status,
    /// The inspector (required once the check is judged).
    pub by: Option<String>,
    pub date: Option<String>,
    /// The figure the check asks to record.
    pub value: Option<String>,
    pub note: Option<String>,
}

/// A check the user adds to a stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomItem {
    pub id: String,
    pub stage: String,
    pub text: String,
    /// What to record, when the check depends on a figure.
    pub record: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QaInputs {
    pub records: Vec<QaRecord>,
    pub custom_items: Vec<CustomItem>,
}

impl QaInputs {
    pub fn check(&self) -> Result<(), String> {
        let mut custom = BTreeSet::new();
        for c in &self.custom_items {
            if stage(&c.stage).is_none() {
                return Err(format!("custom check '{}': '{}' is not a stage", c.id, c.stage));
            }
            if c.id.trim().is_empty() || c.text.trim().is_empty() {
                return Err("a custom check needs an id and a text".into());
            }
            if template_item(&c.id).is_some() || !custom.insert(c.id.as_str()) {
                return Err(format!("the check id '{}' is already used", c.id));
            }
        }
        let mut seen = BTreeSet::new();
        for r in &self.records {
            if template_item(&r.item).is_none() && !custom.contains(r.item.as_str()) {
                return Err(format!("'{}' is not a checklist item", r.item));
            }
            if !seen.insert(r.item.as_str()) {
                return Err(format!("check '{}' has two records", r.item));
            }
            if let Some(d) = &r.date {
                parse_date(&format!("check '{}'", r.item), d)?;
            }
            if matches!(r.status, Status::Pass | Status::Fail) && blank(&r.by) {
                return Err(format!("check '{}': enter who inspected it before marking pass or fail", r.item));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ItemReport {
    pub id: String,
    pub en: String,
    pub ar: String,
    pub custom: bool,
    pub record_en: Option<String>,
    pub record_ar: Option<String>,
    pub status: Status,
    pub by: Option<String>,
    pub date: Option<String>,
    pub value: Option<String>,
    pub note: Option<String>,
    /// A pass without the figure the check asks for.
    pub issue: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StageReport {
    pub id: &'static str,
    pub en: &'static str,
    pub ar: &'static str,
    pub due: Option<String>,
    pub due_en: &'static str,
    pub due_ar: &'static str,
    pub due_task: &'static str,
    pub items: Vec<ItemReport>,
    pub pass: usize,
    pub fail: usize,
    pub pending: usize,
    pub na: usize,
    /// `not_started`, `in_progress`, `passed` or `failed`.
    pub state: &'static str,
    /// Share of the applicable checks that passed.
    pub progress_pct: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct QaReport {
    pub stages: Vec<StageReport>,
    pub passed_stages: usize,
    pub failed_stages: usize,
    pub issues: Vec<String>,
}

pub fn report(inp: &QaInputs, acts: Option<&[Activity]>) -> QaReport {
    let mut issues = Vec::new();
    if acts.is_none() {
        issues.push("the 4D schedule has not been computed (Schedule tab): stages have no due dates".into());
    }
    let mut stages = Vec::new();
    for s in STAGES {
        let due = acts.and_then(|a| link_date(a, &Link::Task { key: s.due_key.into() }, s.due_at).ok()).map(|d| d.iso());
        let mut items: Vec<ItemReport> = Vec::new();
        let mut push = |id: &str, en: &str, ar: &str, rec: Option<(String, String)>, custom: bool| {
            let r = inp.records.iter().find(|r| r.item == id);
            let status = r.map_or(Status::Pending, |r| r.status);
            let value = r.and_then(|r| r.value.clone()).filter(|v| !v.trim().is_empty());
            let issue = (status == Status::Pass && rec.is_some() && value.is_none()).then(|| format!("record the {} before passing", rec.as_ref().map_or("", |r| r.0.as_str())));
            items.push(ItemReport {
                id: id.into(),
                en: en.into(),
                ar: ar.into(),
                custom,
                record_en: rec.as_ref().map(|r| r.0.clone()),
                record_ar: rec.map(|r| r.1),
                status,
                by: r.and_then(|r| r.by.clone()),
                date: r.and_then(|r| r.date.clone()),
                value,
                note: r.and_then(|r| r.note.clone()),
                issue,
            });
        };
        for i in s.items {
            push(i.id, i.en, i.ar, i.record.map(|(a, b)| (a.to_string(), b.to_string())), false);
        }
        for c in inp.custom_items.iter().filter(|c| c.stage == s.id) {
            push(&c.id, &c.text, &c.text, c.record.clone().filter(|r| !r.trim().is_empty()).map(|r| (r.clone(), r)), true);
        }
        let count = |st: Status| items.iter().filter(|i| i.status == st).count();
        let (pass, fail, pending, na) = (count(Status::Pass), count(Status::Fail), count(Status::Pending), count(Status::Na));
        let open_issue = items.iter().any(|i| i.issue.is_some());
        let applicable = items.len() - na;
        let state = if fail > 0 {
            "failed"
        } else if applicable > 0 && pass == applicable && !open_issue {
            "passed"
        } else if pass + na == 0 {
            "not_started"
        } else {
            "in_progress"
        };
        for i in items.iter().filter(|i| i.issue.is_some()) {
            issues.push(format!("{}: {}", i.id, i.issue.as_deref().unwrap_or("")));
        }
        let progress_pct = if applicable == 0 { 0.0 } else { crate::execution::round(pass as f64 / applicable as f64 * 100.0, 1) };
        stages.push(StageReport { id: s.id, en: s.en, ar: s.ar, due, due_en: s.due_en, due_ar: s.due_ar, due_task: s.due_key, items, pass, fail, pending, na, state, progress_pct });
    }
    QaReport { passed_stages: stages.iter().filter(|s| s.state == "passed").count(), failed_stages: stages.iter().filter(|s| s.state == "failed").count(), stages, issues }
}

fn status_label(s: Status, arabic: bool) -> &'static str {
    match (s, arabic) {
        (Status::Pending, false) => "Pending",
        (Status::Pass, false) => "Pass",
        (Status::Fail, false) => "FAIL",
        (Status::Na, false) => "N/A",
        (Status::Pending, true) => "قيد الانتظار",
        (Status::Pass, true) => "مطابق",
        (Status::Fail, true) => "غير مطابق",
        (Status::Na, true) => "لا ينطبق",
    }
}

fn state_label(s: &str, arabic: bool) -> &'static str {
    match (s, arabic) {
        ("passed", false) => "Passed",
        ("failed", false) => "FAILED",
        ("in_progress", false) => "In progress",
        (_, false) => "Not started",
        ("passed", true) => "مُجتاز",
        ("failed", true) => "راسب",
        ("in_progress", true) => "جارٍ",
        (_, true) => "لم يبدأ",
    }
}

impl QaReport {
    /// A printable report to sign: every stage with its checks, who inspected and when, and the recorded figures.
    pub fn html(&self, title: &str, arabic: bool) -> String {
        let t = |ar: &'static str, en: &'static str| if arabic { ar } else { en };
        let mut h = format!(
            "<!doctype html><html lang=\"{}\" dir=\"{}\"><head><meta charset=\"utf-8\"><title>{}</title><style>\
             body{{font-family:'Segoe UI',Tahoma,sans-serif;margin:28px;color:#111}} h1{{font-size:22px;margin:0 0 4px}} h2{{font-size:16px;margin:24px 0 6px;border-bottom:2px solid #333;padding-bottom:3px}}\
             table{{border-collapse:collapse;width:100%;font-size:12px}} th,td{{border:1px solid #999;padding:4px 6px;text-align:start;vertical-align:top}} th{{background:#eee}}\
             .pass{{color:#166534;font-weight:700}} .fail{{color:#b91c1c;font-weight:700}} .warn{{color:#a33}} .note{{font-size:11px;color:#444;margin-top:18px}}\
             .sign{{margin-top:12px;display:flex;gap:40px}} .sign div{{flex:1;border-top:1px solid #333;padding-top:4px;font-size:11px}} @page{{size:A4 portrait;margin:12mm}} @media print{{body{{margin:0}} h2{{break-after:avoid}} tr{{break-inside:avoid}}}}</style></head><body>",
            if arabic { "ar" } else { "en" },
            if arabic { "rtl" } else { "ltr" },
            esc(title)
        );
        h.push_str(&format!("<h1>{} — {}</h1><p>{}: {} / {} · {}: {}</p>", t("قوائم فحص الجودة في الموقع", "Site QA/QC inspection checklists"), esc(title), t("مراحل مجتازة", "Stages passed"), self.passed_stages, self.stages.len(), t("مراحل راسبة", "Stages failed"), self.failed_stages));
        for (n, s) in self.stages.iter().enumerate() {
            h.push_str(&format!(
                "<h2>{}. {} — {}</h2><p>{}: {} · {}: {}</p><table><thead><tr><th>#</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr></thead><tbody>",
                n + 1,
                esc(if arabic { s.ar } else { s.en }),
                state_label(s.state, arabic),
                t("الاستحقاق", "Due"),
                s.due.as_deref().map_or("—".to_string(), |d| format!("{d} ({})", if arabic { s.due_ar } else { s.due_en })),
                t("نسبة الإنجاز", "Progress"),
                format_args!("{:.1}%", s.progress_pct),
                t("الفحص", "Check"),
                t("الحالة", "Status"),
                t("المفتش / التاريخ", "Inspector / date"),
                t("القيمة المسجلة", "Recorded value"),
                t("ملاحظة", "Note")
            ));
            for (i, it) in s.items.iter().enumerate() {
                let text = if arabic { &it.ar } else { &it.en };
                let want = if arabic { it.record_ar.as_deref() } else { it.record_en.as_deref() };
                let class = match it.status {
                    Status::Pass => "pass",
                    Status::Fail => "fail",
                    _ => "",
                };
                h.push_str(&format!(
                    "<tr><td>{}</td><td>{}{}</td><td class=\"{class}\">{}</td><td>{} {}</td><td>{}{}</td><td>{}</td></tr>",
                    i + 1,
                    esc(text),
                    want.map_or(String::new(), |w| format!("<br><small>[{}: {}]</small>", t("سجّل", "record"), esc(w))),
                    status_label(it.status, arabic),
                    esc(it.by.as_deref().unwrap_or("")),
                    esc(it.date.as_deref().unwrap_or("")),
                    esc(it.value.as_deref().unwrap_or("")),
                    it.issue.as_ref().map_or(String::new(), |m| format!("<br><span class=\"warn\">{}</span>", esc(m))),
                    esc(it.note.as_deref().unwrap_or(""))
                ));
            }
            h.push_str("</tbody></table>");
        }
        h.push_str(&format!("<div class=\"sign\"><div>{}</div><div>{}</div><div>{}</div></div>", t("المقاول", "Contractor"), t("المشرف / الاستشاري", "Supervisor / consultant"), t("المالك", "Owner")));
        h.push_str(&format!("<p class=\"note\">{}</p></body></html>", t("قائمة إجرائية: لا تذكر حدود قبول؛ الحدود من مواصفة المشروع والمخططات والجهة المختصة، ويسجلها المفتش.", "A procedural checklist: it states no acceptance limit. Limits come from the project specification, the drawings and the authority, and the inspector records them.")));
        h
    }

    pub fn workbook(&self, arabic: bool) -> Workbook {
        let t = |ar: &'static str, en: &'static str| if arabic { ar } else { en };
        let text = |s: &str, st: u8| (Cell::Text(s.to_string()), st);
        let mut rows: Vec<Vec<(Cell, u8)>> = vec![[t("المرحلة", "Stage"), t("الاستحقاق", "Due"), t("الرمز", "Id"), t("الفحص", "Check"), t("الحالة", "Status"), t("المفتش", "Inspector"), t("التاريخ", "Date"), t("القيمة المسجلة", "Recorded value"), t("ملاحظة", "Note")].iter().map(|l| text(l, style::HEADER)).collect()];
        for s in &self.stages {
            for it in &s.items {
                rows.push(vec![
                    text(if arabic { s.ar } else { s.en }, style::TEXT),
                    text(s.due.as_deref().unwrap_or(""), style::TEXT),
                    text(&it.id, style::TEXT),
                    text(if arabic { &it.ar } else { &it.en }, style::TEXT),
                    text(status_label(it.status, arabic), style::TEXT),
                    text(it.by.as_deref().unwrap_or(""), style::TEXT),
                    text(it.date.as_deref().unwrap_or(""), style::TEXT),
                    text(it.value.as_deref().unwrap_or(""), style::TEXT),
                    text(it.note.as_deref().unwrap_or(""), style::TEXT),
                ]);
            }
        }
        let last = rows.len();
        let checks = Sheet { name: t("الفحوص", "Checks").into(), rows, widths: vec![30.0, 12.0, 24.0, 60.0, 12.0, 18.0, 12.0, 26.0, 30.0], rtl: arabic, header: true, filter_to: last };
        let mut rows: Vec<Vec<(Cell, u8)>> = vec![[t("المرحلة", "Stage"), t("الحالة", "State"), t("مطابق", "Pass"), t("غير مطابق", "Fail"), t("قيد الانتظار", "Pending"), t("لا ينطبق", "N/A"), t("نسبة الإنجاز %", "Progress %")].iter().map(|l| text(l, style::HEADER)).collect()];
        for s in &self.stages {
            rows.push(vec![
                text(if arabic { s.ar } else { s.en }, style::TEXT),
                text(state_label(s.state, arabic), style::TEXT),
                (Cell::Num(s.pass as f64), style::TEXT),
                (Cell::Num(s.fail as f64), style::TEXT),
                (Cell::Num(s.pending as f64), style::TEXT),
                (Cell::Num(s.na as f64), style::TEXT),
                (Cell::Num(s.progress_pct), style::PCT),
            ]);
        }
        let summary = Sheet { name: t("الملخص", "Summary").into(), rows, widths: vec![44.0, 14.0, 10.0, 12.0, 14.0, 10.0, 14.0], rtl: arabic, header: true, filter_to: 0 };
        Workbook { sheets: vec![summary, checks] }
    }
}
