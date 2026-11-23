//! The permit package: the plan set as numbered A3 sheets, each with a title-block header and the licensed engineer's stamp
//! box, plus an index sheet that lists every sheet by number.
//!
//! Everything about the engineer and the submission is the user's input and none of it has a default: the engineer's name, the
//! licence number, the body that issued the licence, the project number, the sheet-number prefix and the revision label. A missing
//! one is reported by name and no package is built. The drawings are the same SVG drawings as on screen (`drawing::plan_svg`)
//! and each sheet lists its level's rooms with the area measured from the model.
//!
//! The stamp box prints the entered engineer details with an EMPTY area for the physical seal and signature. This module never
//! draws a seal, never signs, and never states that the package was approved: the stamp reads "not valid until stamped and signed
//! by the licensed engineer". It is a preliminary document and does not replace the approving authority.

use crate::building;
use crate::drawing::{DrawOpts, plan_svg};
use crate::model::{Id, Model};
use crate::typed::{Level, Space, Typed};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Engineer {
    pub name: String,
    pub licence_no: String,
    /// The body that issued the licence (for example the professional council), as the user states it.
    pub licensing_body: String,
    /// The engineer's discipline (architectural, structural, ...), shown in the stamp when entered.
    pub discipline: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Input {
    pub engineer: Engineer,
    pub project_no: String,
    /// Letters before the sheet's number, for example `A` gives `A-01`.
    pub sheet_prefix: String,
    /// Revision label printed in every header, for example `Rev A`.
    pub revision: String,
    /// What the package is issued for, shown in the header when entered.
    pub issued_for: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SheetInfo {
    /// `A-01`: the prefix and the position of the sheet in the package.
    pub number: String,
    pub title: String,
    /// `index` or `plan`.
    pub kind: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Pack {
    pub html: String,
    pub sheets: Vec<SheetInfo>,
    pub notes: Vec<String>,
}

pub struct Context<'a> {
    pub model: &'a Model,
    pub title: &'a str,
    pub labels: BTreeMap<String, String>,
    pub arabic: bool,
    pub show_structure: bool,
    pub show_mep: bool,
    /// `YYYY-MM-DD`, supplied by the caller.
    pub date: &'a str,
}

pub(crate) fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The names of the inputs that are missing, in the order of the form.
fn missing(i: &Input) -> Vec<&'static str> {
    let mut m = Vec::new();
    for (name, v) in [
        ("engineer.name", &i.engineer.name),
        ("engineer.licence_no", &i.engineer.licence_no),
        ("engineer.licensing_body", &i.engineer.licensing_body),
        ("project_no", &i.project_no),
        ("sheet_prefix", &i.sheet_prefix),
        ("revision", &i.revision),
    ] {
        if v.trim().is_empty() {
            m.push(name);
        }
    }
    m
}

pub(crate) fn valid_date(d: &str) -> bool {
    let b = d.as_bytes();
    b.len() == 10 && b[4] == b'-' && b[7] == b'-' && b.iter().enumerate().all(|(k, c)| k == 4 || k == 7 || c.is_ascii_digit()) && (1..=12).contains(&d[5..7].parse::<u32>().unwrap_or(0)) && (1..=31).contains(&d[8..10].parse::<u32>().unwrap_or(0))
}

fn func_ar(f: &str) -> &str {
    match f {
        "majlis" => "مجلس",
        "living" => "صالة",
        "dining" => "طعام",
        "kitchen" => "مطبخ",
        "bedroom" => "نوم",
        "bathroom" => "حمام",
        "stair" => "درج",
        "lift" => "مصعد",
        "courtyard" => "فناء",
        "hall" => "ردهة",
        "circulation_reserve" => "حركة وتنقل",
        other => other,
    }
}

/// The sheet number `PREFIX-NN` for the 1-based position `n`.
pub fn sheet_number(prefix: &str, n: usize) -> String {
    format!("{}-{n:02}", prefix.trim())
}

struct Chrome<'a> {
    input: &'a Input,
    ctx: &'a Context<'a>,
    total: usize,
}

impl Chrome<'_> {
    fn t(&self, ar: &'static str, en: &'static str) -> &'static str {
        if self.ctx.arabic { ar } else { en }
    }

    /// The title-block header of a sheet: project, number, revision, date, issue purpose.
    fn header(&self, number: &str, title: &str, position: usize) -> String {
        let i = self.input;
        let issued = if i.issued_for.trim().is_empty() { String::new() } else { format!("<td>{}<b>{}</b></td>", self.t("الغرض: ", "Issued for: "), esc(&i.issued_for)) };
        format!(
            "<table class=\"hdr\"><tr><td class=\"big\">{}</td><td>{}<b><bdi dir=\"ltr\">{}</bdi></b></td><td>{}<b><bdi dir=\"ltr\">{}</bdi></b></td><td>{}<b><bdi dir=\"ltr\">{}</bdi></b></td>{issued}</tr>\
             <tr><td class=\"big\">{}</td><td>{}<b><bdi dir=\"ltr\">{}</bdi></b></td><td>{}<b><bdi dir=\"ltr\">{}</bdi></b></td><td>{}<b><bdi dir=\"ltr\">{} / {}</bdi></b></td></tr></table>",
            esc(self.ctx.title),
            self.t("رقم المشروع: ", "Project no.: "),
            esc(&i.project_no),
            self.t("المراجعة: ", "Revision: "),
            esc(&i.revision),
            self.t("التاريخ: ", "Date: "),
            esc(self.ctx.date),
            esc(title),
            self.t("رقم اللوحة: ", "Sheet no.: "),
            esc(number),
            self.t("المقياس: ", "Scale: "),
            self.t("غير محدد", "not set"),
            self.t("الصفحة: ", "Page: "),
            position,
            self.total
        )
    }

    /// The engineer's stamp: the entered details and an empty box for the seal and signature.
    fn stamp(&self) -> String {
        let e = &self.input.engineer;
        let disc = if e.discipline.trim().is_empty() { String::new() } else { format!("<div>{}<b>{}</b></div>", self.t("التخصص: ", "Discipline: "), esc(&e.discipline)) };
        format!(
            "<div class=\"stamp\"><div class=\"sd\"><div>{}<b>{}</b></div><div>{}<b><bdi dir=\"ltr\">{}</bdi></b></div><div>{}<b>{}</b></div>{disc}</div>\
             <div class=\"seal\">{}</div><div class=\"sig\">{}</div></div>\
             <p class=\"warn\">{}</p>",
            self.t("المهندس: ", "Engineer: "),
            esc(&e.name),
            self.t("رقم الترخيص: ", "Licence no.: "),
            esc(&e.licence_no),
            self.t("جهة الترخيص: ", "Licensing body: "),
            esc(&e.licensing_body),
            self.t("الختم", "Seal"),
            self.t("التوقيع", "Signature"),
            self.t(
                "غير نافذة حتى يختمها المهندس المرخّص ويوقّعها. أولية ولا تغني عن اعتماد الجهة المختصة.",
                "Not valid until stamped and signed by the licensed engineer. Preliminary; it does not replace the approving authority."
            )
        )
    }
}

/// Builds the package. Refuses, naming them, when an input is missing or the date is malformed, and when the model has no rooms to draw.
pub fn build(ctx: &Context, input: &Input) -> Result<Pack, String> {
    let miss = missing(input);
    if !miss.is_empty() {
        return Err(format!("permit pack: missing input(s): {}. Nothing is defaulted; enter them.", miss.join(", ")));
    }
    if !valid_date(ctx.date) {
        return Err(format!("permit pack: the date must be YYYY-MM-DD, got `{}`", ctx.date));
    }
    let model = ctx.model;
    let levels: Vec<Option<Id>> = {
        let l = building::levels(model);
        if l.is_empty() { vec![None] } else { l.into_iter().map(|(_, id)| Some(id)).collect() }
    };
    // Draw first so the total is known: a level with nothing to draw gets no sheet and no number.
    let mut drawn: Vec<(String, String, String)> = Vec::new(); // (title, svg, room table)
    let mut notes = Vec::new();
    for lv in &levels {
        let name = lv
            .and_then(|l| model.get(&l))
            .and_then(|e| e.typed::<Level>().ok())
            .map_or(String::new(), |l| match (l.index, ctx.arabic) {
                (0, true) => "الدور الأرضي".to_string(),
                (0, false) => "Ground floor".to_string(),
                (n, true) => format!("الدور {n}"),
                (n, false) => format!("Floor {n}"),
            });
        let title = if name.is_empty() { ctx.title.to_string() } else { format!("{} — {name}", ctx.title) };
        let draw = DrawOpts { title: title.clone(), labels: ctx.labels.clone(), show_structure: ctx.show_structure, show_mep: ctx.show_mep, ..DrawOpts::default() };
        let Ok(svg) = plan_svg(model, *lv, &draw) else {
            notes.push(if ctx.arabic { format!("لا فراغات في «{title}»: لا لوحة لها.") } else { format!("No rooms in `{title}`: no sheet for it.") });
            continue;
        };
        let mut rows = String::new();
        let mut total = 0.0;
        for e in model.elements.values().filter(|e| e.kind == Space::KIND && (lv.is_none() || e.level_id == *lv)) {
            let Ok(s) = e.typed::<Space>() else { continue };
            total += s.area;
            let label = crate::annot::room_name(&s, &ctx.labels).unwrap_or_else(|| s.requirement_key.clone());
            let func = if ctx.arabic { func_ar(&s.func).to_string() } else { s.func.clone() };
            rows.push_str(&format!("<tr><td>{}</td><td>{}</td><td class=\"n\">{:.1}</td></tr>", esc(&label), esc(&func), s.area));
        }
        let (a, b, c) = if ctx.arabic { ("الفراغ", "النوع", "المساحة (م²)") } else { ("Room", "Type", "Area (m²)") };
        let table = format!("<table class=\"rooms\"><tr><th>{a}</th><th>{b}</th><th>{c}</th></tr>{rows}<tr class=\"tot\"><td colspan=\"2\">{}</td><td class=\"n\">{total:.1}</td></tr></table>", if ctx.arabic { "المجموع" } else { "Total" });
        drawn.push((title, svg, table));
    }
    if drawn.is_empty() {
        return Err("permit pack: nothing to print: the model has no rooms".into());
    }
    let total = drawn.len() + 1;
    let chrome = Chrome { input, ctx, total };
    let mut sheets = Vec::with_capacity(total);
    let mut body = String::new();

    let index_title = chrome.t("فهرس اللوحات", "Sheet index").to_string();
    let n0 = sheet_number(&input.sheet_prefix, 1);
    sheets.push(SheetInfo { number: n0.clone(), title: index_title.clone(), kind: "index" });
    let mut index_rows = format!("<tr><td><bdi dir=\"ltr\">{}</bdi></td><td>{}</td></tr>", esc(&n0), esc(&index_title));
    for (k, (title, _, _)) in drawn.iter().enumerate() {
        let n = sheet_number(&input.sheet_prefix, k + 2);
        index_rows.push_str(&format!("<tr><td><bdi dir=\"ltr\">{}</bdi></td><td>{}</td></tr>", esc(&n), esc(title)));
        sheets.push(SheetInfo { number: n, title: title.clone(), kind: "plan" });
    }
    body.push_str(&format!(
        "<section class=\"sheet\">{}<h1>{}</h1><table class=\"rooms\"><tr><th>{}</th><th>{}</th></tr>{index_rows}</table>{}</section>",
        chrome.header(&n0, &index_title, 1),
        esc(&index_title),
        chrome.t("رقم اللوحة", "Sheet no."),
        chrome.t("العنوان", "Title"),
        chrome.stamp()
    ));
    for (k, (title, svg, table)) in drawn.iter().enumerate() {
        let n = sheet_number(&input.sheet_prefix, k + 2);
        body.push_str(&format!("<section class=\"sheet\">{}<div class=\"plan\">{svg}</div>{table}{}</section>", chrome.header(&n, title, k + 2), chrome.stamp()));
    }
    let html = format!(
        "<!doctype html><html lang=\"{}\" dir=\"{}\"><head><meta charset=\"utf-8\"><title>{}</title><style>\
         @page{{size:A3 portrait;margin:12mm}} *{{-webkit-print-color-adjust:exact;print-color-adjust:exact}}\
         body{{font-family:'Segoe UI',Tahoma,sans-serif;color:#111;margin:0}} .sheet{{page-break-after:always;break-after:page}} .sheet:last-child{{page-break-after:auto;break-after:auto}}\
         h1{{font-size:20px;margin:10px 0 6px}} table{{border-collapse:collapse;width:100%;font-size:11px;margin:6px 0}} th,td{{border:1px solid #999;padding:2px 6px;text-align:start}} th{{background:#eee}} td.n{{text-align:end}} tr.tot td{{font-weight:700;background:#f6f6f6}}\
         .hdr td{{border:1.5px solid #333;font-size:12px}} .hdr td.big{{font-size:16px;font-weight:700}} .plan{{direction:ltr}} .plan svg{{width:100%;height:auto;max-height:190mm;display:block}}\
         .stamp{{display:flex;gap:8px;margin-top:10px;border:2px solid #333;padding:6px;min-height:34mm}} .stamp .sd{{flex:2;font-size:12px;line-height:1.7}} .stamp .seal,.stamp .sig{{flex:1;border:1px dashed #777;font-size:10px;color:#777;padding:2px}}\
         .warn{{font-size:11px;color:#b71c1c;font-weight:600;margin:4px 0}}</style></head><body>{body}</body></html>",
        if ctx.arabic { "ar" } else { "en" },
        if ctx.arabic { "rtl" } else { "ltr" },
        esc(ctx.title)
    );
    notes.push(chrome.t("المقياس غير محدد في الرسم؛ تُقرأ الأبعاد من التأطير لا من مقياس مطبوع.", "The drawings carry no printed scale; read dimensions from the dimension lines, not by measuring the print.").to_string());
    Ok(Pack { html, sheets, notes })
}
