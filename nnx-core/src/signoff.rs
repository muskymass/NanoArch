//! The licensed engineer's sign-off form with a statement of responsibility: one printable A4 page (HTML for the shell's PDF
//! printing) that the engineer signs on paper.
//!
//! Nothing here is defaulted or invented. The engineer's details, the project, the scope the engineer answers for and the
//! WORDING of the responsibility statement are all the user's input: the statement is the text of the licensing body's or the
//! office's own form, and this module does not write legal text. The statement must also carry its `statement_source` (where the
//! wording comes from). A missing input is reported by name and no form is built.
//!
//! The page leaves the signature, seal and date boxes EMPTY, never draws a seal, never signs, and never states that the engineer
//! approved anything; it says it is void until the engineer signs. It does not replace the approving authority.

use crate::permitpack::{Engineer, esc, valid_date};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Input {
    pub engineer: Engineer,
    pub project_no: String,
    pub revision: String,
    /// The works or documents the engineer answers for, one per entry (for example "Architectural drawings A-01 to A-05").
    pub scope: Vec<String>,
    /// The responsibility statement, word for word as the licensing body or the office requires it.
    pub statement: String,
    /// Where the statement's wording comes from (document, issuer, date).
    pub statement_source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Form {
    pub html: String,
    pub notes: Vec<String>,
}

/// The names of the inputs that are missing, in the order of the form.
fn missing(i: &Input) -> Vec<&'static str> {
    let mut m = Vec::new();
    for (name, v) in [
        ("engineer.name", &i.engineer.name),
        ("engineer.licence_no", &i.engineer.licence_no),
        ("engineer.licensing_body", &i.engineer.licensing_body),
        ("project_no", &i.project_no),
        ("revision", &i.revision),
        ("statement", &i.statement),
        ("statement_source", &i.statement_source),
    ] {
        if v.trim().is_empty() {
            m.push(name);
        }
    }
    if i.scope.iter().all(|s| s.trim().is_empty()) {
        m.push("scope");
    }
    m
}

/// Builds the form. Refuses, naming them, when an input is missing or the date is malformed.
pub fn build(title: &str, date: &str, arabic: bool, input: &Input) -> Result<Form, String> {
    let miss = missing(input);
    if !miss.is_empty() {
        return Err(format!("sign-off form: missing input(s): {}. Nothing is defaulted and the statement wording is not written for you; enter them.", miss.join(", ")));
    }
    if !valid_date(date) {
        return Err(format!("sign-off form: the date must be YYYY-MM-DD, got `{date}`"));
    }
    let t = |ar: &'static str, en: &'static str| if arabic { ar } else { en };
    let e = &input.engineer;
    let disc = if e.discipline.trim().is_empty() { String::new() } else { format!("<tr><th>{}</th><td>{}</td></tr>", t("التخصص", "Discipline"), esc(&e.discipline)) };
    let scope: String = input.scope.iter().filter(|s| !s.trim().is_empty()).map(|s| format!("<li>{}</li>", esc(s.trim()))).collect();
    let html = format!(
        "<!doctype html><html lang=\"{lang}\" dir=\"{dir}\"><head><meta charset=\"utf-8\"><title>{ttl}</title><style>\
         @page{{size:A4 portrait;margin:15mm}} *{{-webkit-print-color-adjust:exact;print-color-adjust:exact}}\
         body{{font-family:'Segoe UI',Tahoma,sans-serif;color:#111;font-size:13px;line-height:1.6}} h1{{font-size:20px;margin:0 0 8px}} h2{{font-size:15px;margin:14px 0 4px}}\
         table{{border-collapse:collapse;width:100%}} th,td{{border:1px solid #999;padding:4px 8px;text-align:start}} th{{background:#eee;width:30%}}\
         .stmt{{border:1.5px solid #333;padding:8px 10px;white-space:pre-wrap}} .src{{font-size:11px;color:#555}}\
         .sign{{display:flex;gap:10px;margin-top:12px}} .box{{flex:1;border:1px dashed #777;min-height:30mm;padding:3px;font-size:11px;color:#777}}\
         .warn{{font-size:12px;color:#b71c1c;font-weight:600;margin-top:10px}}</style></head><body>\
         <h1>{h1}</h1>\
         <table><tr><th>{pn}</th><td>{ttl}</td></tr><tr><th>{pno}</th><td><bdi dir=\"ltr\">{pno_v}</bdi></td></tr><tr><th>{rev}</th><td><bdi dir=\"ltr\">{rev_v}</bdi></td></tr><tr><th>{dt}</th><td><bdi dir=\"ltr\">{date}</bdi></td></tr>\
         <tr><th>{en}</th><td>{en_v}</td></tr><tr><th>{ln}</th><td><bdi dir=\"ltr\">{ln_v}</bdi></td></tr><tr><th>{lb}</th><td>{lb_v}</td></tr>{disc}</table>\
         <h2>{sc}</h2><ul>{scope}</ul>\
         <h2>{st}</h2><div class=\"stmt\">{stmt}</div><div class=\"src\">{ss}: {ss_v}</div>\
         <div class=\"sign\"><div class=\"box\">{sig}</div><div class=\"box\">{seal}</div><div class=\"box\">{sd}</div></div>\
         <p class=\"warn\">{warn}</p></body></html>",
        lang = if arabic { "ar" } else { "en" },
        dir = if arabic { "rtl" } else { "ltr" },
        ttl = esc(title),
        h1 = t("نموذج توقيع المهندس المعتمد وبيان المسؤولية", "Licensed engineer sign-off and statement of responsibility"),
        pn = t("المشروع", "Project"),
        pno = t("رقم المشروع", "Project no."),
        pno_v = esc(&input.project_no),
        rev = t("المراجعة", "Revision"),
        rev_v = esc(&input.revision),
        dt = t("تاريخ إعداد النموذج", "Form prepared on"),
        en = t("المهندس", "Engineer"),
        en_v = esc(&e.name),
        ln = t("رقم الترخيص", "Licence no."),
        ln_v = esc(&e.licence_no),
        lb = t("جهة الترخيص", "Licensing body"),
        lb_v = esc(&e.licensing_body),
        sc = t("نطاق ما يتحمل المهندس مسؤوليته", "Scope the engineer answers for"),
        st = t("بيان المسؤولية", "Statement of responsibility"),
        stmt = esc(input.statement.trim()),
        ss = t("مصدر نص البيان", "Source of the statement wording"),
        ss_v = esc(input.statement_source.trim()),
        sig = t("التوقيع", "Signature"),
        seal = t("الختم", "Seal"),
        sd = t("تاريخ التوقيع", "Date signed"),
        warn = t(
            "لا يعتد بهذا النموذج حتى يوقّعه المهندس المرخّص ويختمه ويؤرّخه بخطه. أولي ولا يغني عن اعتماد الجهة المختصة.",
            "This form is void until the licensed engineer signs, stamps and dates it by hand. Preliminary; it does not replace the approving authority."
        ),
    );
    let notes = vec![t(
        "نص بيان المسؤولية من إدخال المستخدم وبمصدره؛ لم يُكتب أو يُعدَّل آلياً.",
        "The statement wording is the user's input with its stated source; it was not written or altered by the application.",
    )
    .to_string()];
    Ok(Form { html, notes })
}
