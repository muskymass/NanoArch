//! The printable plan set: one A3 sheet per level (the plan drawing, then its room schedule), as one HTML document
//! that the application prints to PDF with the web engine (which shapes Arabic correctly).
//!
//! Everything comes from the model: the drawings are the same SVG drawings as on screen (`drawing::plan_svg`), the
//! schedule lists each room of the level with its function and area (a room made of several rectangles is one row), and the total is
//! their sum; the net area stands beside the gross one when the rooms have it. A door or window size has one mark in the whole set, so
//! "D2" is the same door on every sheet, and each row gives the count on the level and in the building. Nothing is invented; the sheet
//! says it is preliminary.

use crate::building;
use crate::drawing::{DrawOpts, plan_svg};
use crate::model::{Id, Model};
use crate::typed::{Door, Level, Space, Typed, Window};
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

pub(crate) fn func_ar(f: &str) -> &str {
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
        // A room found from walls, whose use nobody has stated yet.
        "room" => "غرفة",
        "other" => "أخرى",
        other => other,
    }
}

pub fn print_html(model: &Model, opts: &PrintOpts) -> Result<String, String> {
    let t = |ar: &'static str, en: &'static str| if opts.arabic { ar } else { en };
    let levels: Vec<Option<Id>> = {
        let l = building::levels(model);
        if l.is_empty() { vec![None] } else { l.into_iter().map(|(_, id)| Some(id)).collect() }
    };
    // Every door and window size of the building, with how many there are in all: the order of the sizes gives the marks.
    let mut all_doors: BTreeMap<(i64, i64, bool), usize> = BTreeMap::new();
    let mut all_windows: BTreeMap<(i64, i64, i64), usize> = BTreeMap::new();
    for e in model.elements.values() {
        if e.kind == Door::KIND {
            if let Ok(d) = e.typed::<Door>() {
                *all_doors.entry((d.width, d.height, d.is_egress)).or_default() += 1;
            }
        } else if e.kind == Window::KIND
            && let Ok(w) = e.typed::<Window>()
        {
            *all_windows.entry((w.width, w.height, w.sill_height)).or_default() += 1;
        }
    }
    let mut sheets = String::new();
    let mut drawn = 0;
    for (k, lv) in levels.iter().enumerate() {
        let name = lv
            .and_then(|l| model.get(&l))
            .and_then(|e| e.typed::<Level>().ok())
            .map_or(String::new(), |l| if l.index == 0 { t("الدور الأرضي", "Ground floor").to_string() } else if opts.arabic { format!("الدور {}", l.index) } else { format!("Floor {}", l.index) });
        let title = if name.is_empty() { opts.title.clone() } else { format!("{} — {name}", opts.title) };
        let draw = DrawOpts { title: title.clone(), labels: opts.labels.clone(), show_structure: opts.show_structure, show_mep: opts.show_mep, arabic: opts.arabic, ..DrawOpts::default() };
        let Ok(svg) = plan_svg(model, *lv, &draw) else { continue };
        drawn += 1;
        let mut rows: Vec<String> = Vec::new();
        let (mut total, mut net_total) = (0.0, 0.0);
        // The rectangles that are parts of a room are counted in that room's row, not listed as rooms of their own.
        let rooms: Vec<(Option<Id>, Space)> = model.elements.values().filter(|e| e.kind == Space::KIND && (lv.is_none() || e.level_id == *lv)).filter_map(|e| Some((e.level_id, e.typed::<Space>().ok()?))).filter(|(_, s)| crate::annot::part_of(s).is_none()).collect();
        let totals: Vec<crate::annot::RoomTotal> = rooms.iter().map(|(l, s)| crate::annot::room_total(model, *l, s)).collect();
        let has_net = totals.iter().any(|t| t.net_area.is_some());
        for ((_, s), whole) in rooms.iter().zip(&totals) {
            // A room with no net area has no wall on any side: its gross area is its net area.
            let net = whole.net_area.unwrap_or(whole.area);
            total += whole.area;
            net_total += net;
            let label = crate::annot::room_name(s, &opts.labels).unwrap_or_else(|| s.requirement_key.clone());
            let func = if opts.arabic { func_ar(&s.func).to_string() } else { s.func.clone() };
            let net_cell = if has_net { format!("<td class=\"n\">{net:.1}</td>") } else { String::new() };
            rows.push(format!("<tr><td>{}</td><td>{}</td><td class=\"n\">{:.1}</td>{net_cell}</tr>", esc(&label), esc(&func), whole.area));
        }
        // A long schedule is split into two side-by-side tables so the level stays on one sheet with its footer and stamp.
        let net_head = if has_net { format!("<th>{}</th>", t("الصافية (م²)", "Net (m²)")) } else { String::new() };
        let head = format!("<tr><th>{}</th><th>{}</th><th>{}</th>{net_head}</tr>", t("الفراغ", "Room"), t("النوع", "Type"), if has_net { t("الإجمالية (م²)", "Gross (m²)") } else { t("المساحة (م²)", "Area (m²)") });
        let per_table = if rows.len() > 16 { rows.len().div_ceil(2) } else { rows.len().max(1) };
        let net_sum = if has_net { format!("<td class=\"n\">{net_total:.1}</td>") } else { String::new() };
        let total_row = format!("<tr class=\"tot\"><td colspan=\"2\">{}</td><td class=\"n\">{:.1}</td>{net_sum}</tr>", t("المجموع", "Total"), total);
        let chunks: Vec<&[String]> = rows.chunks(per_table).collect();
        let mut tables = String::new();
        for (i, chunk) in chunks.iter().enumerate() {
            let last = i + 1 == chunks.len();
            tables.push_str(&format!("<table>{head}{}{}</table>", chunk.concat(), if last { total_row.as_str() } else { "" }));
        }
        if chunks.is_empty() {
            tables = format!("<table>{head}{total_row}</table>");
        }
        // Door and window schedule of the level: one row per size, with how many there are (sizes in mm, as modelled).
        let mut door_sizes: BTreeMap<(i64, i64, bool), usize> = BTreeMap::new();
        let mut window_sizes: BTreeMap<(i64, i64, i64), usize> = BTreeMap::new();
        for e in model.elements.values().filter(|e| lv.is_none() || e.level_id == *lv) {
            if e.kind == Door::KIND {
                if let Ok(d) = e.typed::<Door>() {
                    *door_sizes.entry((d.width, d.height, d.is_egress)).or_default() += 1;
                }
            } else if e.kind == Window::KIND
                && let Ok(w) = e.typed::<Window>() {
                    *window_sizes.entry((w.width, w.height, w.sill_height)).or_default() += 1;
                }
        }
        let door_rows: String = all_doors
            .iter()
            .enumerate()
            .filter_map(|(i, (size @ (w, h, egress), all))| door_sizes.get(size).map(|n| format!("<tr><td>D{}</td><td class=\"n\">{w} × {h}</td><td>{}</td><td class=\"n\">{n}</td><td class=\"n\">{all}</td></tr>", i + 1, if *egress { t("مدخل", "Entrance") } else { t("داخلي", "Interior") })))
            .collect();
        let window_rows: String = all_windows.iter().enumerate().filter_map(|(i, (size @ (w, h, sill), all))| window_sizes.get(size).map(|n| format!("<tr><td>W{}</td><td class=\"n\">{w} × {h}</td><td class=\"n\">{sill}</td><td class=\"n\">{n}</td><td class=\"n\">{all}</td></tr>", i + 1))).collect();
        let openings = format!(
            "<h2>{}</h2><div class=\"sched\"><table class=\"op\"><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr>{door_rows}</table><table class=\"op\"><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr>{window_rows}</table></div>",
            t("جدول الأبواب والنوافذ (مم)", "Door and window schedule (mm)"),
            t("الباب", "Door"),
            t("العرض × الارتفاع", "Width × height"),
            t("النوع", "Type"),
            t("في الدور", "On level"),
            t("في المبنى", "In building"),
            t("النافذة", "Window"),
            t("العرض × الارتفاع", "Width × height"),
            t("ارتفاع الجلسة", "Sill"),
            t("في الدور", "On level"),
            t("في المبنى", "In building")
        );
        sheets.push_str(&format!(
            "<section class=\"sheet\"><h1>{}</h1><div class=\"plan\">{}</div><h2>{}</h2><div class=\"sched\">{}</div>{openings}\
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
