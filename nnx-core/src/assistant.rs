//! In-app assistant (tracker item 100): explains every warning the engine raises and offers a one-click fix where one exists.
//!
//! It reads what the engine already computes (the integrity check and the rule check) and adds no engineering of its own:
//! - an **integrity issue** is explained from a fixed table keyed by its code. The one automatic fix is for
//!   `OPENING.IN_COLUMN`: the doors and windows `building::clear_openings` can move clear of a column are moved, and the ones
//!   it cannot move are left exactly where they are.
//! - a **rule finding** is explained from the rule itself (id, clause, layer, message). Its one-click fix exists only when the
//!   rule defines one: the corrected value comes from the rule's own expression over its sourced parameters.
//! - a **skipped rule** is explained as "not checked" (never as a pass) and points at the parameter the user must enter, with
//!   its source. It has no fix: the engine does not invent the value.
//!
//! Every other warning is explained and carries no fix; the assistant never proposes a guess. A fix is applied by re-running the
//! analysis and matching the item id, so a stale list can not apply a command computed for an older model, and the change is one
//! undoable step. The text is a preliminary aid: it does not replace a licensed engineer or the approving authority.

use crate::building;
use crate::command::{Command, Origin};
use crate::history::Document;
use crate::model::{Id, Model};
use crate::precheck::{self, Level};
use crate::rules::{self, Severity};
use serde::Serialize;

/// One property change a fix would make, shown to the user before they accept it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FixChange {
    pub element: String,
    pub prop: String,
    /// Current value, `None` when the property is not set.
    pub from: Option<serde_json::Value>,
    pub to: Option<serde_json::Value>,
}

/// A one-click correction: what it does and the exact changes it makes.
#[derive(Debug, Clone, Serialize)]
pub struct Fix {
    pub summary_en: String,
    pub summary_ar: String,
    pub changes: Vec<FixChange>,
    #[serde(skip)]
    commands: Vec<Command>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    /// Stable across runs on the same model: `integrity:<code>:<key>`, `rule:<rule>:<element>` or `skipped:<rule>`.
    pub id: String,
    /// `integrity`, `rule` or `skipped`.
    pub source: &'static str,
    pub code: String,
    /// `error`, `warning`, `info` or `not_checked`.
    pub severity: &'static str,
    pub message: String,
    pub elements: Vec<String>,
    pub why_en: String,
    pub why_ar: String,
    pub how_en: String,
    pub how_ar: String,
    pub fix: Option<Fix>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Advice {
    pub items: Vec<Item>,
    /// How many items carry a one-click fix.
    pub fixable: usize,
}

/// What an integrity code means and what to do about it. `None` for a code this table does not know: the caller then says so.
fn integrity_text(code: &str) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    Some(match code {
        "ROOM.OVERLAP" => (
            "Two rooms on the same level cover the same floor area, so areas, finishes and quantities would be counted twice.",
            "غرفتان على الدور نفسه تشغلان المساحة ذاتها، فتُحسب المساحات والتشطيبات والكميات مرتين.",
            "Resize or move one of the two rooms so they only touch along an edge. This is a design choice, so there is no automatic fix.",
            "غيّر مقاس إحدى الغرفتين أو موضعها بحيث تتلامسان على حافة فقط. القرار تصميمي فلا يوجد إصلاح تلقائي.",
        ),
        "OPENING.OVERLAP" => (
            "A door and a window, or two doors, sit on the same stretch of one wall, which would put an impossible drawing in a contractor's hands.",
            "بابان أو باب ونافذة على المقطع نفسه من الجدار، وهذا يُخرج مخططاً مستحيلاً التنفيذ.",
            "Move one of the openings along its wall, or remove one. The choice of which opening stays is the designer's, so there is no automatic fix.",
            "حرّك أحد الفتحتين على طول الجدار أو احذف إحداهما. اختيار الفتحة التي تبقى للمصمم فلا يوجد إصلاح تلقائي.",
        ),
        "COLUMN.UNSUPPORTED" => (
            "A column has no column under it on the level below, so its load has no direct path down to the foundation.",
            "عمود لا يقابله عمود تحته في الدور السفلي، فلا مسار مباشر لحمله إلى الأساس.",
            "Align the column with one below, or ask the structural engineer to provide a transfer beam. This is a structural decision, so there is no automatic fix.",
            "حاذِ العمود بعمود تحته، أو اطلب من المهندس الإنشائي كمرة نقل. القرار إنشائي فلا يوجد إصلاح تلقائي.",
        ),
        "OPENING.IN_COLUMN" => (
            "A door or window position falls inside a column, so the opening cannot be built where it is drawn.",
            "موضع باب أو نافذة يقع داخل عمود، فلا يمكن تنفيذ الفتحة حيث رُسمت.",
            "The fix moves each such opening along its wall to the nearest position that clears every column and every other opening and keeps the wall's end margins. Openings it cannot move clear stay exactly where they are and need a manual decision.",
            "ينقل الإصلاح كل فتحة كهذه على طول جدارها إلى أقرب موضع يتجاوز الأعمدة والفتحات الأخرى ويحفظ هوامش طرف الجدار. الفتحات التي يتعذّر نقلها تبقى كما هي وتحتاج قراراً يدوياً.",
        ),
        "DOOR.UNKNOWN_ROOM" | "WINDOW.UNKNOWN_ROOM" => (
            "A door or window names a room that is no longer on the level; re-tiling a drafted plan leaves such names behind.",
            "باب أو نافذة يذكر غرفة لم تعد موجودة على الدور؛ إعادة تقسيم مخطط مرسوم تترك أسماء كهذه.",
            "Open the element and point it at an existing room, or delete it if the room was removed on purpose. Which room it belongs to is the designer's choice, so there is no automatic fix.",
            "افتح العنصر واجعله يشير إلى غرفة موجودة، أو احذفه إن أُزيلت الغرفة عمداً. تحديد الغرفة قرار المصمم فلا يوجد إصلاح تلقائي.",
        ),
        "ROOM.NO_DOOR" => (
            "A room is enclosed by walls but nothing in the model lets anyone enter it.",
            "غرفة محاطة بجدران ولا يوجد في النموذج ما يتيح الدخول إليها.",
            "Add a door on one of its walls. Where the door goes affects circulation, so there is no automatic fix.",
            "أضف باباً على أحد جدرانها. موضع الباب يؤثر في الحركة فلا يوجد إصلاح تلقائي.",
        ),
        _ => return None,
    })
}

fn sev_name(l: Level) -> &'static str {
    match l {
        Level::Error => "error",
        Level::Warning => "warning",
    }
}

fn rule_sev_name(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
    }
}

/// The changes a command list would make, read against the current model.
fn changes_of(model: &Model, cmds: &[Command]) -> Vec<FixChange> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::SetProp { id, key, value } => Some(FixChange {
                element: id.to_string(),
                prop: key.clone(),
                from: model.get(id).and_then(|e| e.props.get(key).cloned()),
                to: value.clone(),
            }),
            _ => None,
        })
        .collect()
}

fn fix_of(model: &Model, cmds: Vec<Command>, en: String, ar: String) -> Option<Fix> {
    if cmds.is_empty() {
        return None;
    }
    Some(Fix { summary_en: en, summary_ar: ar, changes: changes_of(model, &cmds), commands: cmds })
}

/// Builds the explanation list from the integrity report and, when it ran, the rule report. Errors come first, then warnings,
/// then rule findings of lower severity, then the rules that did not run.
pub fn advise(model: &Model, integrity: &precheck::Report, rules: Option<&rules::Report>) -> Advice {
    let mut items: Vec<Item> = Vec::new();

    let levels: Vec<Option<Id>> = {
        let l = building::levels(model);
        if l.is_empty() { vec![None] } else { l.into_iter().map(|(_, id)| Some(id)).collect() }
    };

    for issue in &integrity.issues {
        let elements: Vec<String> = issue.elements.iter().map(|i| i.to_string()).collect();
        let key = if elements.is_empty() { issue.message.clone() } else { elements.join(",") };
        let (why_en, why_ar, how_en, how_ar) = match integrity_text(issue.code) {
            Some((a, b, c, d)) => (a.to_string(), b.to_string(), c.to_string(), d.to_string()),
            None => (
                format!("The integrity check raised {} and this assistant has no explanation for that code yet.", issue.code),
                format!("فحص السلامة أصدر {} ولا يملك المساعد شرحاً لهذا الرمز بعد.", issue.code),
                "Read the message and fix the elements it names; there is no automatic fix.".to_string(),
                "اقرأ الرسالة وعالج العناصر التي تذكرها؛ لا يوجد إصلاح تلقائي.".to_string(),
            ),
        };
        let fix = if issue.code == "OPENING.IN_COLUMN" {
            let (cmds, _) = building::clear_openings(model, &levels);
            let n = cmds.len();
            fix_of(model, cmds, format!("Move {n} door/window position(s) clear of the columns"), format!("نقل {n} من مواضع الأبواب والنوافذ بعيداً عن الأعمدة"))
        } else {
            None
        };
        items.push(Item {
            id: format!("integrity:{}:{}", issue.code, key),
            source: "integrity",
            code: issue.code.to_string(),
            severity: sev_name(issue.level),
            message: issue.message.clone(),
            elements,
            why_en,
            why_ar,
            how_en,
            how_ar,
            fix,
        });
    }

    if let Some(r) = rules {
        for f in &r.findings {
            let fix = f.fix.clone().and_then(|c| {
                let (en, ar) = match &c {
                    Command::SetProp { key, value, .. } => {
                        let v = value.as_ref().map(|v| v.to_string()).unwrap_or_else(|| "(removed)".into());
                        (format!("Set '{key}' to {v}, the value rule {} computes from its sourced parameters", f.rule_id), format!("ضبط '{key}' على {v}، وهي القيمة التي تحسبها القاعدة {} من معاملاتها ذات المصدر", f.rule_id))
                    }
                    _ => (format!("Apply the correction defined by rule {}", f.rule_id), format!("تطبيق التصحيح الذي تعرّفه القاعدة {}", f.rule_id)),
                };
                fix_of(model, vec![c], en, ar)
            });
            let how_en = if fix.is_some() {
                "Accept the one-click fix, or change the element yourself so the check passes.".to_string()
            } else {
                "This rule defines no automatic fix: change the element so the check passes, or correct the rule's entered limit if that value is wrong.".to_string()
            };
            let how_ar = if fix.is_some() {
                "اقبل الإصلاح بنقرة، أو عدّل العنصر بنفسك حتى ينجح الفحص.".to_string()
            } else {
                "لا تعرّف هذه القاعدة إصلاحاً تلقائياً: عدّل العنصر حتى ينجح الفحص، أو صحّح الحد المُدخل للقاعدة إن كانت قيمته خاطئة.".to_string()
            };
            items.push(Item {
                id: format!("rule:{}:{}", f.rule_id, f.element),
                source: "rule",
                code: f.rule_id.clone(),
                severity: rule_sev_name(f.severity),
                message: f.message.clone(),
                elements: vec![f.element.to_string()],
                why_en: format!("Rule {} (clause: {}, layer: {}) is not met: {}", f.rule_id, f.clause, f.layer, f.message),
                why_ar: format!("القاعدة {} (البند: {}، الطبقة: {}) غير متحققة: {}", f.rule_id, f.clause, f.layer, f.message),
                how_en,
                how_ar,
                fix,
            });
        }
        for s in &r.skipped {
            items.push(Item {
                id: format!("skipped:{}", s.rule_id),
                source: "skipped",
                code: s.rule_id.clone(),
                severity: "not_checked",
                message: s.reason.clone(),
                elements: s.elements.iter().map(|i| i.to_string()).collect(),
                why_en: format!("Rule {} did not run, so {} element(s) were NOT checked. A skipped rule is not a pass. Reason: {}", s.rule_id, s.elements.len(), s.reason),
                why_ar: format!("القاعدة {} لم تُشغَّل، فلم يُفحص {} عنصر. القاعدة المتخطاة ليست نجاحاً. السبب: {}", s.rule_id, s.elements.len(), s.reason),
                how_en: format!("Enter a value for '{}' together with where it comes from (code, edition, clause or the authority's document). The assistant does not supply it.", s.param),
                how_ar: format!("أدخل قيمة '{}' مع مصدرها (الكود والإصدار والبند أو وثيقة الجهة). لا يضع المساعد القيمة بنفسه.", s.param),
                fix: None,
            });
        }
    }

    // Errors first, then warnings, then the rest; the original order is kept inside each group.
    let rank = |s: &str| match s {
        "error" => 0,
        "warning" => 1,
        "info" => 2,
        _ => 3,
    };
    items.sort_by_key(|i| rank(i.severity));
    let fixable = items.iter().filter(|i| i.fix.is_some()).count();
    Advice { items, fixable }
}

/// What applying a fix did.
#[derive(Debug, Clone, Serialize)]
pub struct Applied {
    pub id: String,
    pub changes: Vec<FixChange>,
}

/// Applies the one-click fix of the item named `id` from a fresh `advice` (computed on `doc`'s current model), as one undoable
/// step. Refused by name when the item no longer exists (the model changed since the list was shown) or has no fix.
pub fn apply(doc: &mut Document, advice: &Advice, id: &str) -> Result<Applied, String> {
    let item = advice.items.iter().find(|i| i.id == id).ok_or_else(|| format!("'{id}' is not in the current warning list: it was fixed or the model changed. Refresh the list"))?;
    let fix = item.fix.as_ref().ok_or_else(|| format!("'{}' has no automatic fix: {}", item.code, item.how_en))?;
    doc.commit(&format!("Assistant fix: {}", item.code), Origin::User, fix.commands.clone()).map_err(|e| format!("the fix for '{}' was refused and nothing changed: {e}", item.code))?;
    Ok(Applied { id: item.id.clone(), changes: fix.changes.clone() })
}
