//! The printable plan set: one A3 sheet per level (the plan drawing, then its room schedule), as one HTML document
//! that the application prints to PDF with the web engine (which shapes Arabic correctly).
//!
//! Everything comes from the model: the drawings are the same SVG drawings as on screen (`drawing::plan_svg`), the
//! schedule lists each room of the level with its function and area, and the total is their sum. Nothing is
//! invented; the sheet says it is preliminary.

use crate::building;
use crate::drawing::{DrawOpts, plan_svg};
use crate::model::{Id, Model};
use crate::typed::{Level, Space, Typed};
use std::collections::BTreeMap;

pub struct PrintOpts {
    pub title: String,
    /// Display names of rooms by requirement key.
    pub labels: BTreeMap<String, String>,
    /// Headings and direction in Arabic (otherwise English).
    pub arabic: bool,
    pub show_structure: bool,
    pub show_mep: bool,
    /// Date shown in the footer of each sheet (supplied by the caller).
    pub date: String,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
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
        "hall" => "ردهة",
        "circulation_reserve" => "حركة وتنقل",
        other => other,
    }
}

pub fn print_html(model: &Model, opts: &PrintOpts) -> Result<String, String> {
    let t = |ar: &'static str, en: &'static str| if opts.arabic { ar } else { en };
    let levels: Vec<Option<Id>> = {
        let l = building::levels(model);
        if l.is_empty() { vec![None] } else { l.into_iter().map(|(_, id)| Some(id)).collect() }
    };
    let mut sheets = String::new();
    let mut drawn = 0;
    for (k, lv) in levels.iter().enumerate() {
        let name = lv
            .and_then(|l| model.get(&l))
            .and_then(|e| e.typed::<Level>().ok())
            .map_or(String::new(), |l| if l.index == 0 { t("الدور الأرضي", "Ground floor").to_string() } else if opts.arabic { format!("الدور {}", l.index) } else { format!("Floor {}", l.index) });
        let title = if name.is_empty() { opts.title.clone() } else { format!("{} — {name}", opts.title) };
        let draw = DrawOpts { title: title.clone(), labels: opts.labels.clone(), show_structure: opts.show_structure, show_mep: opts.show_mep, ..DrawOpts::default() };
        let Ok(svg) = plan_svg(model, *lv, &draw) else { continue };
        drawn += 1;
        let mut rows: Vec<String> = Vec::new();
        let mut total = 0.0;
        for e in model.elements.values().filter(|e| e.kind == Space::KIND && (lv.is_none() || e.level_id == *lv)) {
            let Ok(s) = e.typed::<Space>() else { continue };
            total += s.area;
            let label = crate::annot::room_name(&s, &opts.labels).unwrap_or_else(|| s.requirement_key.clone());
            let func = if opts.arabic { func_ar(&s.func).to_string() } else { s.func.clone() };
            rows.push(format!("<tr><td>{}</td><td>{}</td><td class=\"n\">{:.1}</td></tr>", esc(&label), esc(&func), s.area));
        }
        // A long schedule is split into two side-by-side tables so the level stays on one sheet with its footer and stamp.
        let head = format!("<tr><th>{}</th><th>{}</th><th>{}</th></tr>", t("الفراغ", "Room"), t("النوع", "Type"), t("المساحة (م²)", "Area (m²)"));
        let per_table = if rows.len() > 16 { rows.len().div_ceil(2) } else { rows.len().max(1) };
        let total_row = format!("<tr class=\"tot\"><td colspan=\"2\">{}</td><td class=\"n\">{:.1}</td></tr>", t("المجموع", "Total"), total);
        let chunks: Vec<&[String]> = rows.chunks(per_table).collect();
        let mut tables = String::new();
        for (i, chunk) in chunks.iter().enumerate() {
            let last = i + 1 == chunks.len();
            tables.push_str(&format!("<table>{head}{}{}</table>", chunk.concat(), if last { total_row.as_str() } else { "" }));
        }
        if chunks.is_empty() {
            tables = format!("<table>{head}{total_row}</table>");
        }
        sheets.push_str(&format!(
            "<section class=\"sheet\"><h1>{}</h1><div class=\"plan\">{}</div><h2>{}</h2><div class=\"sched\">{}</div>\
             <p class=\"foot\">{} · <bdi dir=\"ltr\">{}</bdi> · {} <bdi dir=\"ltr\">{}/{}</bdi></p></section>",
            esc(&title),
            svg,
            t("جدول الفراغات", "Room schedule"),
            tables,
            t("أولي — غير صالح للتنفيذ، يراجعه مهندس مرخّص", "Preliminary — not for construction; to be checked by a licensed engineer"),
            esc(&opts.date),
            t("ورقة", "Sheet"),
            k + 1,
            levels.len()
        ));
    }
    if drawn == 0 {
        return Err("nothing to print: the model has no rooms".into());
    }
    Ok(format!(
        "<!doctype html><html lang=\"{}\" dir=\"{}\"><head><meta charset=\"utf-8\"><title>{}</title><style>\
         @page{{size:A3 portrait;margin:12mm;@bottom-center{{content:counter(page) \" / \" counter(pages);direction:ltr;font-size:9pt;color:#555}}}} *{{-webkit-print-color-adjust:exact;print-color-adjust:exact}}\
         body{{font-family:'Segoe UI',Tahoma,sans-serif;color:#111;margin:0}} .sheet{{page-break-after:always;break-after:page}} .sheet:last-child{{page-break-after:auto;break-after:auto}}\
         h1{{font-size:20px;margin:0 0 6px}} h2{{font-size:15px;margin:14px 0 6px}} .plan{{direction:ltr}} .plan svg{{width:100%;height:auto;max-height:215mm;display:block}} .sched{{display:flex;gap:10px;align-items:flex-start}} .sched table{{flex:1}}\
         table{{border-collapse:collapse;width:100%;font-size:11px}} th,td{{border:1px solid #999;padding:1px 6px;text-align:start}} th{{background:#eee}} td.n{{text-align:end}} tr.tot td{{font-weight:700;background:#f6f6f6}}\
         .foot{{font-size:11px;color:#555;margin-top:10px}}</style></head><body>{}</body></html>",
        if opts.arabic { "ar" } else { "en" },
        if opts.arabic { "rtl" } else { "ltr" },
        esc(&opts.title),
        sheets
    ))
}
