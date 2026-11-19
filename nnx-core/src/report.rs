//! The engineering report: MEP load schedules (HVAC with Btu/hr and tons, electrical with kVA by board, plumbing), the routes
//! that were generated, and the structural schedules (columns `COL-*`, footings `FND-*`, tie beams `TB-*`, concrete, and the bar
//! bending schedule with weights), as a printable HTML document (A4, the shell prints it to PDF) and as an XLSX workbook.
//! Both come from the same two reports, so a number is the same in the page, the PDF and the sheet.
//!
//! Everything is preliminary: it carries the missing inputs and the warnings of the calculations that were not complete.

use crate::mep::MepReport;
use crate::sizing::SizingReport;
use crate::xlsx::{Cell, Sheet, Workbook, style};

pub struct ReportData<'a> {
    pub title: String,
    /// `YYYY-MM-DD`.
    pub date: String,
    pub mep: Option<&'a MepReport>,
    pub sizing: Option<&'a SizingReport>,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn f(v: f64, d: usize) -> String {
    format!("{v:.d$}")
}

/// A table: header cells and rows, as HTML.
fn table(head: &[&str], rows: &[Vec<String>], numeric_from: usize) -> String {
    let mut o = String::from("<table><thead><tr>");
    for h in head {
        o.push_str(&format!("<th>{}</th>", esc(h)));
    }
    o.push_str("</tr></thead><tbody>");
    for r in rows {
        o.push_str("<tr>");
        for (i, c) in r.iter().enumerate() {
            o.push_str(&format!("<td{}>{}</td>", if i >= numeric_from { " class=\"n\"" } else { "" }, esc(c)));
        }
        o.push_str("</tr>");
    }
    o.push_str("</tbody></table>");
    o
}

pub fn engineering_html(d: &ReportData, arabic: bool) -> String {
    let t = |ar: &'static str, en: &'static str| if arabic { ar } else { en };
    let mut b = String::new();
    let h2 = |s: &str| format!("<h2>{}</h2>", esc(s));
    let note = |s: &str| format!("<p class=\"note\">{}</p>", esc(s));
    let missing_and_warnings = |missing: &std::collections::BTreeMap<String, Vec<String>>, warnings: &[String]| -> String {
        let mut o = String::new();
        if !missing.is_empty() {
            o.push_str(&format!("<h3 class=\"bad\">{}</h3><ul>", esc(t("مدخلات ناقصة", "Missing inputs"))));
            for (k, v) in missing {
                o.push_str(&format!("<li><b>{}</b>: {}</li>", esc(k), esc(&v.join("; "))));
            }
            o.push_str("</ul>");
        }
        if !warnings.is_empty() {
            o.push_str(&format!("<h3>{}</h3><ul>", esc(t("تنبيهات", "Warnings"))));
            for w in warnings {
                o.push_str(&format!("<li>{}</li>", esc(w)));
            }
            o.push_str("</ul>");
        }
        o
    };

    if let Some(m) = d.mep {
        b.push_str(&h2(t("١. أحمال التكييف", "1. HVAC load schedule")));
        match &m.hvac {
            Some(h) => {
                let rows: Vec<Vec<String>> = h
                    .rooms
                    .iter()
                    .map(|r| vec![r.room.clone(), f(r.load_w, 0), f(r.btu_hr, 0), f(r.tons, 2), r.airflow_lps.map_or(String::new(), |a| f(a, 1)), format!("{} x {} W", r.units, r.unit_w)])
                    .collect();
                b.push_str(&table(&[t("الغرفة", "Room"), "W", "Btu/hr", t("طن تبريد", "Tons"), t("تدفق الهواء L/s", "Airflow L/s"), t("الوحدات", "Units")], &rows, 1));
                b.push_str(&note(&format!("{}: {} W · {} Btu/hr · {} {}", t("المجموع", "Total"), f(h.total_w, 0), f(h.total_btu_hr, 0), f(h.total_tons, 2), t("طن", "tons"))));
            }
            None => b.push_str(&note(t("لم تُحسب الأحمال: راجع المدخلات الناقصة.", "Loads were not computed: see the missing inputs."))),
        }
        b.push_str(&h2(t("٢. الأحمال الكهربائية", "2. Electrical load schedule")));
        match &m.electrical {
            Some(e) => {
                let rows: Vec<Vec<String>> = e
                    .circuits
                    .iter()
                    .map(|c| vec![c.id.clone(), c.board.clone(), c.kind.clone(), c.points.to_string(), f(c.watts, 0), f(c.current_a, 2), f(c.breaker_a, 0), f(c.cable_mm2, 1), f(c.length_m, 1), f(c.vdrop_pct, 2)])
                    .collect();
                b.push_str(&table(&[t("الدائرة", "Circuit"), t("اللوحة", "Board"), t("النوع", "Kind"), t("النقاط", "Points"), "W", "A", t("القاطع A", "Breaker A"), "mm²", t("الطول m", "Length m"), "VD %"], &rows, 3));
                let boards: Vec<Vec<String>> = e.boards.iter().map(|x| vec![x.room.clone(), x.circuits.to_string(), f(x.connected_w, 0), f(x.demand_w, 0), f(x.demand_kva, 2)]).collect();
                b.push_str(&table(&[t("لوحة التوزيع", "Distribution board"), t("الدوائر", "Circuits"), t("المتصل W", "Connected W"), t("الطلب W", "Demand W"), "kVA"], &boards, 1));
                b.push_str(&note(&format!(
                    "{}: {} kVA ({} W) · {}: {} kVA ({} W) · {} A · {} A",
                    t("المتصل", "Connected"),
                    f(e.connected_kva, 2),
                    f(e.connected_w, 0),
                    t("الطلب", "Demand"),
                    f(e.demand_kva, 2),
                    f(e.demand_w, 0),
                    f(e.main_current_a, 1),
                    f(e.main_breaker_a, 0)
                )));
            }
            None => b.push_str(&note(t("لم تُحسب الدوائر: راجع المدخلات الناقصة.", "Circuits were not computed: see the missing inputs."))),
        }
        b.push_str(&h2(t("٣. السباكة والصرف", "3. Plumbing and drainage")));
        match &m.plumbing {
            Some(p) => {
                let rows: Vec<Vec<String>> = p.rooms.iter().map(|r| vec![r.room.clone(), f(r.flow_lps, 2), r.pipe.clone(), f(r.dfu, 1), f(r.drain_mm, 0)]).collect();
                b.push_str(&table(&[t("الغرفة", "Room"), "L/s", t("الأنبوب", "Pipe"), "DFU", t("المصرف mm", "Drain mm")], &rows, 1));
                b.push_str(&note(&format!(
                    "{}: {} L/s · {}: {} · DFU {} → {} mm{}",
                    t("تدفق التصميم", "Design flow"),
                    f(p.design_flow_lps, 2),
                    t("الأنبوب الرئيسي", "Main pipe"),
                    p.main_pipe,
                    f(p.total_dfu, 1),
                    f(p.main_drain_mm, 0),
                    p.tank_m3.map_or(String::new(), |v| format!(" · {} {} m³", t("الخزان", "Tank"), f(v, 2)))
                )));
            }
            None => b.push_str(&note(t("لم تُحسب السباكة: راجع المدخلات الناقصة.", "Plumbing was not computed: see the missing inputs."))),
        }
        if !m.routes.is_empty() {
            b.push_str(&h2(t("٤. المسارات المولّدة", "4. Generated routes")));
            let rows: Vec<Vec<String>> = m
                .routes
                .iter()
                .map(|r| vec![r.system.clone(), r.role.clone(), r.room.clone().unwrap_or_default(), format!("{}, {}, {}", r.from[0], r.from[1], r.from[2]), format!("{}, {}, {}", r.to[0], r.to[1], r.to[2]), f(r.length_m, 2), r.size_mm.clone(), r.slope_pct.map_or(String::new(), |s| f(s, 2))])
                .collect();
            b.push_str(&table(&[t("النظام", "System"), t("الدور", "Role"), t("الغرفة", "Room"), t("من (مم)", "From (mm)"), t("إلى (مم)", "To (mm)"), t("الطول m", "Length m"), "mm", t("الميل %", "Slope %")], &rows, 5));
        }
        b.push_str(&missing_and_warnings(&m.missing, &m.warnings));
    }

    if let Some(s) = d.sizing {
        b.push_str(&h2(t("٥. الجداول الإنشائية", "5. Structural schedules")));
        if !s.columns.is_empty() {
            b.push_str(&format!("<h3>{}</h3>", esc(t("الأعمدة", "Columns"))));
            let rows: Vec<Vec<String>> = s.columns.iter().map(|c| vec![c.name.clone(), c.id.clone(), c.grid_ref.clone(), c.section.clone(), f(c.tributary_m2, 2), c.axial_kn.map_or(String::new(), |v| f(v, 1))]).collect();
            b.push_str(&table(&["COL", "ID", t("المحور", "Grid"), t("المقطع", "Section"), t("المساحة المساهمة m²", "Tributary m²"), t("الحمل kN", "Axial kN")], &rows, 4));
        }
        if !s.footings.is_empty() {
            b.push_str(&format!("<h3>{}</h3>", esc(t("القواعد", "Footings"))));
            let rows: Vec<Vec<String>> = s.footings.iter().map(|x| vec![x.name.clone(), x.id.clone(), x.column.clone(), x.provided_mm.clone(), f(x.load_kn, 1), x.required_m2.map_or(String::new(), |v| f(v, 3)), x.status.clone()]).collect();
            b.push_str(&table(&["FND", "ID", "COL", t("القائم mm", "Provided mm"), t("الحمل kN", "Load kN"), t("المطلوب m²", "Required m²"), t("الحالة", "Status")], &rows, 4));
        }
        if !s.tie_beams.is_empty() {
            b.push_str(&format!("<h3>{}</h3>", esc(t("الميدات الرابطة", "Tie beams"))));
            let rows: Vec<Vec<String>> = s.tie_beams.iter().map(|x| vec![x.name.clone(), x.id.clone().unwrap_or_default(), x.from.clone(), x.to.clone(), x.span_mm.to_string(), x.clear_mm.to_string(), format!("{}x{}", x.width_mm, x.depth_mm)]).collect();
            b.push_str(&table(&["TB", "ID", t("من", "From"), t("إلى", "To"), t("البحر mm", "Span mm"), t("الصافي mm", "Clear mm"), t("المقطع", "Section")], &rows, 4));
        }
        if !s.concrete.is_empty() {
            b.push_str(&format!("<h3>{}</h3>", esc(t("الخرسانة", "Concrete"))));
            let mut rows: Vec<Vec<String>> = s.concrete.iter().map(|c| vec![c.element.clone(), c.id.clone().unwrap_or_default(), c.kind.clone(), f(c.volume_m3, 3), c.basis.clone()]).collect();
            rows.push(vec![t("المجموع", "Total").into(), String::new(), String::new(), f(s.concrete_total_m3, 3), String::new()]);
            b.push_str(&table(&[t("العنصر", "Element"), "ID", t("النوع", "Kind"), "m³", t("الأساس", "Basis")], &rows, 3));
        }
        if !s.bars.is_empty() {
            b.push_str(&format!("<h3>{}</h3>", esc(t("جدول ثني الحديد (BBS)", "Bar bending schedule (BBS)"))));
            let mut rows: Vec<Vec<String>> = s.bars.iter().map(|r| vec![r.mark.clone(), r.element.clone(), r.role.clone(), f(r.dia_mm, 0), r.number.to_string(), f(r.cut_length_mm, 0), f(r.total_m, 2), f(r.kg, 2)]).collect();
            rows.push(vec![t("المجموع", "Total").into(), String::new(), String::new(), String::new(), String::new(), String::new(), String::new(), f(s.steel_total_kg, 2)]);
            b.push_str(&table(&[t("العلامة", "Mark"), t("العنصر", "Element"), t("الدور", "Role"), "Ø mm", t("العدد", "No."), t("طول القص mm", "Cut mm"), "m", "kg"], &rows, 3));
        }
        b.push_str(&missing_and_warnings(&s.missing, &s.warnings));
    }

    format!(
        "<!doctype html><html lang=\"{lang}\" dir=\"{dir}\"><head><meta charset=\"utf-8\"><title>{title}</title><style>\
         body{{font-family:'Segoe UI',Tahoma,Arial,sans-serif;color:#111;font-size:11px;margin:0}}\
         h1{{font-size:18px;margin:0 0 2px}} h2{{font-size:14px;margin:16px 0 4px;border-bottom:1px solid #888;padding-bottom:2px}} h3{{font-size:12px;margin:10px 0 3px}}\
         table{{border-collapse:collapse;width:100%;margin:2px 0 6px}} th,td{{border:1px solid #999;padding:2px 5px;text-align:start}} th{{background:#eee}} td.n{{text-align:end;font-variant-numeric:tabular-nums}}\
         .note{{margin:2px 0 8px;color:#333}} .bad{{color:#a00}} .sub{{color:#555;margin:0 0 8px}} ul{{margin:2px 0 6px;padding-inline-start:18px}}\
         tr{{page-break-inside:avoid}}</style></head><body>\
         <h1>{title}</h1><p class=\"sub\">{sub} · {date}</p>{body}</body></html>",
        lang = if arabic { "ar" } else { "en" },
        dir = if arabic { "rtl" } else { "ltr" },
        title = esc(&d.title),
        sub = esc(t("تقرير هندسي أولي: الأحمال الكهروميكانيكية والجداول الإنشائية. لا يغني عن مهندس مرخّص.", "Preliminary engineering report: MEP loads and structural schedules. Does not replace a licensed engineer.")),
        date = esc(&d.date),
        body = b
    )
}

// ------------------------------------------------------------------ workbook

fn sheet(name: &str, arabic: bool, head: &[&str], rows: Vec<Vec<(Cell, u8)>>, widths: Vec<f64>) -> Sheet {
    let mut all: Vec<Vec<(Cell, u8)>> = vec![head.iter().map(|h| (Cell::Text((*h).to_string()), style::HEADER)).collect()];
    all.extend(rows);
    let filter_to = all.len();
    Sheet { name: name.to_string(), rows: all, widths, rtl: arabic, header: true, filter_to }
}

fn tx(s: &str) -> (Cell, u8) {
    (Cell::Text(s.to_string()), style::TEXT)
}

fn qty(v: f64) -> (Cell, u8) {
    (Cell::Num(v), style::QTY)
}

/// A total row with a live `SUM` over `col` of the data rows (2..=n+1).
fn total_row(label: &str, width: usize, col_index: usize, n: usize, total: f64) -> Vec<(Cell, u8)> {
    let mut r: Vec<(Cell, u8)> = (0..width).map(|_| (Cell::Empty, style::TOTAL_TEXT)).collect();
    r[0] = (Cell::Text(label.to_string()), style::TOTAL_TEXT);
    let c = crate::xlsx::col(col_index);
    r[col_index] = (Cell::FormulaNum(format!("SUM({c}2:{c}{})", n + 1), total), style::TOTAL_MONEY);
    r
}

/// The report as a workbook: one sheet per schedule, with live totals.
pub fn engineering_workbook(d: &ReportData, arabic: bool) -> Workbook {
    let t = |ar: &'static str, en: &'static str| if arabic { ar } else { en };
    let mut sheets: Vec<Sheet> = Vec::new();
    if let Some(m) = d.mep {
        if let Some(h) = &m.hvac {
            let rows: Vec<Vec<(Cell, u8)>> =
                h.rooms.iter().map(|r| vec![tx(&r.room), qty(r.load_w), qty(r.btu_hr), qty(r.tons), r.airflow_lps.map_or((Cell::Empty, style::TEXT), qty), tx(&format!("{} x {} W", r.units, r.unit_w))]).collect();
            let n = rows.len();
            let mut rows = rows;
            rows.push(total_row(t("المجموع", "Total"), 6, 1, n, h.total_w));
            let mut tot = rows.pop().unwrap_or_default();
            let (c2, c3) = (crate::xlsx::col(2), crate::xlsx::col(3));
            tot[2] = (Cell::FormulaNum(format!("SUM({c2}2:{c2}{})", n + 1), h.total_btu_hr), style::TOTAL_MONEY);
            tot[3] = (Cell::FormulaNum(format!("SUM({c3}2:{c3}{})", n + 1), h.total_tons), style::TOTAL_MONEY);
            rows.push(tot);
            sheets.push(sheet("HVAC", arabic, &[t("الغرفة", "Room"), "W", "Btu/hr", t("طن تبريد", "Tons"), "L/s", t("الوحدات", "Units")], rows, vec![18.0, 12.0, 14.0, 12.0, 12.0, 20.0]));
        }
        if let Some(e) = &m.electrical {
            let rows: Vec<Vec<(Cell, u8)>> = e
                .circuits
                .iter()
                .map(|c| vec![tx(&c.id), tx(&c.board), tx(&c.kind), qty(c.points as f64), qty(c.watts), qty(c.current_a), qty(c.breaker_a), qty(c.cable_mm2), qty(c.length_m), qty(c.vdrop_pct)])
                .collect();
            let n = rows.len();
            let mut rows = rows;
            rows.push(total_row(t("المجموع", "Total"), 10, 4, n, e.circuits.iter().map(|c| c.watts).sum()));
            sheets.push(sheet("Electrical", arabic, &[t("الدائرة", "Circuit"), t("اللوحة", "Board"), t("النوع", "Kind"), t("النقاط", "Points"), "W", "A", t("القاطع A", "Breaker A"), "mm²", "m", "VD %"], rows, vec![10.0, 14.0, 12.0, 9.0, 11.0, 9.0, 11.0, 9.0, 9.0, 9.0]));
            let boards: Vec<Vec<(Cell, u8)>> = e.boards.iter().map(|b| vec![tx(&b.room), qty(b.circuits as f64), qty(b.connected_w), qty(b.demand_w), qty(b.demand_kva)]).collect();
            let nb = boards.len();
            let mut boards = boards;
            boards.push(total_row(t("المجموع", "Total"), 5, 4, nb, e.demand_kva));
            sheets.push(sheet("Boards kVA", arabic, &[t("لوحة التوزيع", "Distribution board"), t("الدوائر", "Circuits"), t("المتصل W", "Connected W"), t("الطلب W", "Demand W"), "kVA"], boards, vec![20.0, 10.0, 14.0, 14.0, 10.0]));
        }
        if let Some(p) = &m.plumbing {
            let rows: Vec<Vec<(Cell, u8)>> = p.rooms.iter().map(|r| vec![tx(&r.room), qty(r.flow_lps), tx(&r.pipe), qty(r.dfu), qty(r.drain_mm)]).collect();
            sheets.push(sheet("Plumbing", arabic, &[t("الغرفة", "Room"), "L/s", t("الأنبوب", "Pipe"), "DFU", t("المصرف mm", "Drain mm")], rows, vec![18.0, 10.0, 14.0, 10.0, 14.0]));
        }
        if !m.routes.is_empty() {
            let rows: Vec<Vec<(Cell, u8)>> = m
                .routes
                .iter()
                .map(|r| {
                    vec![
                        tx(&r.system),
                        tx(&r.role),
                        tx(r.room.as_deref().unwrap_or("")),
                        tx(&format!("{}, {}, {}", r.from[0], r.from[1], r.from[2])),
                        tx(&format!("{}, {}, {}", r.to[0], r.to[1], r.to[2])),
                        qty(r.length_m),
                        tx(&r.size_mm),
                        r.slope_pct.map_or((Cell::Empty, style::TEXT), qty),
                    ]
                })
                .collect();
            let n = rows.len();
            let mut rows = rows;
            rows.push(total_row(t("المجموع", "Total"), 8, 5, n, m.routes.iter().map(|r| r.length_m).sum()));
            sheets.push(sheet("Routes", arabic, &[t("النظام", "System"), t("الدور", "Role"), t("الغرفة", "Room"), t("من", "From"), t("إلى", "To"), "m", "mm", t("الميل %", "Slope %")], rows, vec![10.0, 10.0, 14.0, 22.0, 22.0, 10.0, 12.0, 10.0]));
        }
    }
    if let Some(s) = d.sizing {
        if !s.columns.is_empty() {
            let rows: Vec<Vec<(Cell, u8)>> = s.columns.iter().map(|c| vec![tx(&c.name), tx(&c.id), tx(&c.grid_ref), tx(&c.section), qty(c.tributary_m2), c.axial_kn.map_or((Cell::Empty, style::TEXT), qty)]).collect();
            sheets.push(sheet("COL", arabic, &["COL", "ID", t("المحور", "Grid"), t("المقطع", "Section"), t("المساحة المساهمة m²", "Tributary m²"), t("الحمل kN", "Axial kN")], rows, vec![9.0, 38.0, 8.0, 12.0, 16.0, 12.0]));
        }
        if !s.footings.is_empty() {
            let rows: Vec<Vec<(Cell, u8)>> = s.footings.iter().map(|x| vec![tx(&x.name), tx(&x.id), tx(&x.column), tx(&x.provided_mm), qty(x.load_kn), x.required_m2.map_or((Cell::Empty, style::TEXT), qty), tx(&x.status)]).collect();
            sheets.push(sheet("FND", arabic, &["FND", "ID", "COL", t("القائم mm", "Provided mm"), t("الحمل kN", "Load kN"), t("المطلوب m²", "Required m²"), t("الحالة", "Status")], rows, vec![9.0, 38.0, 9.0, 14.0, 12.0, 14.0, 14.0]));
        }
        if !s.tie_beams.is_empty() {
            let rows: Vec<Vec<(Cell, u8)>> = s.tie_beams.iter().map(|x| vec![tx(&x.name), tx(&x.id.clone().unwrap_or_default()), tx(&x.from), tx(&x.to), qty(x.span_mm as f64), qty(x.clear_mm as f64), tx(&format!("{}x{}", x.width_mm, x.depth_mm))]).collect();
            sheets.push(sheet("TB", arabic, &["TB", "ID", t("من", "From"), t("إلى", "To"), t("البحر mm", "Span mm"), t("الصافي mm", "Clear mm"), t("المقطع", "Section")], rows, vec![9.0, 38.0, 9.0, 9.0, 12.0, 12.0, 12.0]));
        }
        if !s.concrete.is_empty() {
            let rows: Vec<Vec<(Cell, u8)>> = s.concrete.iter().map(|c| vec![tx(&c.element), tx(&c.id.clone().unwrap_or_default()), tx(&c.kind), qty(c.volume_m3), tx(&c.basis)]).collect();
            let n = rows.len();
            let mut rows = rows;
            rows.push(total_row(t("المجموع", "Total"), 5, 3, n, s.concrete_total_m3));
            sheets.push(sheet("Concrete", arabic, &[t("العنصر", "Element"), "ID", t("النوع", "Kind"), "m³", t("الأساس", "Basis")], rows, vec![12.0, 38.0, 12.0, 12.0, 44.0]));
        }
        if !s.bars.is_empty() {
            let rows: Vec<Vec<(Cell, u8)>> = s.bars.iter().map(|b| vec![tx(&b.mark), tx(&b.element), tx(&b.id.clone().unwrap_or_default()), tx(&b.role), qty(b.dia_mm), qty(b.number as f64), qty(b.cut_length_mm), qty(b.total_m), qty(b.kg)]).collect();
            let n = rows.len();
            let mut rows = rows;
            rows.push(total_row(t("المجموع", "Total"), 9, 8, n, s.steel_total_kg));
            sheets.push(sheet("BBS", arabic, &[t("العلامة", "Mark"), t("العنصر", "Element"), "ID", t("الدور", "Role"), "Ø mm", t("العدد", "No."), t("طول القص mm", "Cut mm"), "m", "kg"], rows, vec![12.0, 10.0, 38.0, 22.0, 8.0, 8.0, 14.0, 10.0, 10.0]));
        }
    }
    if sheets.is_empty() {
        sheets.push(sheet("Report", arabic, &[t("ملاحظة", "Note")], vec![vec![tx(t("لا بيانات: أدخل مدخلات الكهروميكانيكا والجداول الإنشائية أولاً.", "No data: enter the MEP and structural inputs first."))]], vec![60.0]));
    }
    Workbook { sheets }
}
