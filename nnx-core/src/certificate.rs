//! The preliminary SBC compliance certificate: one printable A4 document (HTML for the shell's PDF printing) that gathers every
//! check of the application with the clause and the file page each limit was read from, the element ids, and a pass / warn /
//! skipped audit trail.
//!
//! It is NOT an official certificate of Balady or of any authority: it carries no logo or seal of one, says so on its first page,
//! and leaves the review box for the licensed engineer. A limit that was not read from the code files is never printed as a code
//! value: it appears as the user's input (with the source the user entered) or as skipped.

use crate::audits::{self, AuditReport};
use crate::malath;
use crate::model::Model;
use crate::project::NanoArchProject;
use crate::revisions::{self, Change};
use crate::sbc;
use crate::thermal;
use std::fmt::Write;

pub struct CertData<'a> {
    pub model: &'a Model,
    pub project: &'a NanoArchProject,
    pub changes: &'a [Change],
    /// `YYYY-MM-DD`.
    pub date: String,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const TD: &str = "border:1px solid #999;padding:3px 6px;vertical-align:top";

fn table(o: &mut String, head: &[&str], rows: &[Vec<String>]) {
    o.push_str("<table style=\"border-collapse:collapse;width:100%;font-size:11.5px;margin:6px 0\"><thead><tr>");
    for h in head {
        let _ = write!(o, "<th style=\"{TD};background:#eee\">{h}</th>");
    }
    o.push_str("</tr></thead><tbody>");
    for r in rows {
        o.push_str("<tr>");
        for c in r {
            let _ = write!(o, "<td style=\"{TD}\">{c}</td>");
        }
        o.push_str("</tr>");
    }
    o.push_str("</tbody></table>");
}

fn status_cell(s: &str, arabic: bool) -> String {
    let (color, ar, en) = match s {
        "ok" => ("#1b5e20", "مطابق", "pass"),
        "violation" => ("#b71c1c", "مخالفة", "violation"),
        "warn" => ("#e65100", "تنبيه", "warn"),
        "skipped" => ("#555", "لم يُفحص", "skipped"),
        _ => ("#555", "معلومة", "info"),
    };
    format!("<b style=\"color:{color}\">{}</b>", if arabic { ar } else { en })
}

fn audit_rows(rep: &AuditReport, arabic: bool) -> Vec<Vec<String>> {
    rep.rows.iter().map(|r| vec![status_cell(r.status, arabic), esc(&r.clause), esc(if arabic { &r.ar } else { &r.en }), r.element.as_ref().map_or(String::new(), |e| format!("<span dir=\"ltr\">{}</span>", &e[..e.len().min(8)])), esc(&r.source)]).collect()
}

pub fn certificate_html(d: &CertData, arabic: bool) -> String {
    let t = |a: &'static str, e: &'static str| if arabic { a } else { e };
    let p = d.project;
    let mut o = String::new();
    let _ = write!(
        o,
        "<!doctype html><html lang=\"{}\" dir=\"{}\"><head><meta charset=\"utf-8\"><title>balady_certificate</title><style>@page{{size:A4;margin:14mm}}body{{font-family:'Segoe UI',Tahoma,Arial,sans-serif;font-size:12.5px;color:#111}}h1{{font-size:20px;margin:0 0 4px}}h2{{font-size:15px;margin:16px 0 4px;border-bottom:2px solid #444;padding-bottom:2px}}.box{{border:2px solid #b71c1c;padding:8px;margin:8px 0;font-weight:600}}.sig td{{height:46px}}</style></head><body>",
        if arabic { "ar" } else { "en" },
        if arabic { "rtl" } else { "ltr" }
    );
    let _ = write!(o, "<h1>{}</h1>", t("تقرير امتثال مبدئي لكود البناء السعودي", "Preliminary Saudi Building Code compliance report"));
    let _ = write!(o, "<div class=\"box\">{}</div>", t("هذا التقرير ليس شهادة رسمية من بلدي أو من أي جهة، ولا يغني عن مراجعة الجهة المعتمدة ومهندس مرخّص. كل حد مذكور فيه إما قُرئ من ملفات الكود (مع البند والصفحة) أو أدخله المستخدم (مع مصدره) أو لم يُفحص.", "This report is NOT an official certificate of Balady or of any authority and does not replace the approving authority or a licensed engineer. Every limit in it was either read from the code files (clause and page given), entered by the user (with the source entered), or is shown as skipped."));
    let bt = p.meta.building_type.map_or("-".to_string(), |b| if arabic { b.label().1.to_string() } else { b.label().0.to_string() });
    table(&mut o, &[t("البند", "Item"), t("القيمة", "Value")], &[
        vec![t("المشروع", "Project").into(), esc(&p.meta.title)],
        vec![t("نوع المبنى", "Building type").into(), esc(&bt)],
        vec![t("الطراز", "Style").into(), esc(p.meta.style.as_deref().unwrap_or("-"))],
        vec![t("المنطقة المناخية", "Climate zone").into(), esc(p.meta.climate_zone.as_deref().unwrap_or("-"))],
        vec![t("مساحة الأرض (م²)", "Plot area (m²)").into(), p.plot_area_m2().map_or("-".into(), |a| format!("{a:.1}"))],
        vec![t("عدد الأدوار", "Floors").into(), p.structure.floors.map_or("-".into(), |f| f.to_string())],
        vec![t("التاريخ", "Date").into(), d.date.clone()],
        vec![t("المرجع", "Reference").into(), "Ministry of Municipal and Rural Affairs, residential building guidelines, 1446 AH; SBC 1101/1102 contractors' guide (1st ed. 2023); inspectors' checklist; SBC 303".into()],
    ]);

    // 1. The SBC checks of the model.
    let rep = sbc::check(d.model);
    let _ = write!(o, "<h2>1. {}</h2>", t("فحوص الكود على المخطط (غرف، إضاءة، ارتدادات)", "Code checks on the plan (rooms, daylight, setbacks)"));
    let _ = write!(o, "<p>{}: <b>{}</b> · {}: <b>{}</b> · {}: <b>{}</b></p>", t("مقارنات أُجريت", "comparisons made"), rep.checked, t("مخالفات", "violations"), rep.violations.len(), t("لم تُفحص", "skipped"), rep.skipped.len());
    let rows: Vec<Vec<String>> = rep.violations.iter().map(|v| vec![status_cell("violation", arabic), esc(&format!("{} {}", v.code, v.clause)), esc(if arabic { &v.ar } else { &v.en }), v.element.map_or(String::new(), |e| format!("<span dir=\"ltr\">{}</span>", &e.to_string()[..8.min(e.to_string().len())])), esc(&v.source)]).collect();
    if rows.is_empty() {
        let _ = write!(o, "<p style=\"color:#1b5e20\"><b>{}</b></p>", t("لا مخالفات في ما فُحص", "No violations in what was checked"));
    } else {
        table(&mut o, &[t("الحالة", "Status"), t("البند", "Clause"), t("الوصف", "Finding"), t("العنصر", "Element"), t("المصدر", "Source")], &rows);
    }
    let rows: Vec<Vec<String>> = rep.skipped.iter().map(|s| vec![status_cell("skipped", arabic), esc(&s.rule), esc(if arabic { &s.ar } else { &s.en })]).collect();
    if !rows.is_empty() {
        table(&mut o, &[t("الحالة", "Status"), t("القاعدة", "Rule"), t("السبب", "Reason")], &rows);
    }
    let _ = write!(o, "<p>{}</p>", t("القيم المقروءة من ملفات الكود المستخدمة في الفحوص:", "Code values read from the files and used in the checks:"));
    let rows: Vec<Vec<String>> = sbc::CODE_VALUES.iter().map(|v| vec![esc(v.id), format!("{} {}", v.value, esc(v.unit)), esc(&format!("{} {}", v.code, v.clause)), esc(&format!("{} p.{}", v.file, v.page))]).collect();
    table(&mut o, &[t("المعرّف", "Id"), t("القيمة", "Value"), t("البند", "Clause"), t("الملف والصفحة", "File and page")], &rows);

    // 2. Setbacks.
    let _ = write!(o, "<h2>2. {}</h2>", t("الارتدادات", "Setbacks"));
    match p.site_result() {
        Some(Ok(r)) => {
            let rows: Vec<Vec<String>> = r.setbacks_m.iter().zip(&r.setback_sources).enumerate().map(|(i, (s, src))| vec![(i + 1).to_string(), format!("{s} m"), esc(src)]).collect();
            table(&mut o, &[t("الضلع", "Edge"), t("الارتداد", "Setback"), t("المصدر", "Source")], &rows);
            let _ = write!(o, "<p>{}: {:.1} m² ({:.0}%)</p>", t("مساحة المبنى", "Building footprint"), r.building_area_m2, r.coverage * 100.0);
        }
        Some(Err(e)) => {
            let _ = write!(o, "<p style=\"color:#b71c1c\">{}</p>", esc(&e.join("; ")));
        }
        None => {
            let _ = write!(o, "<p>{}</p>", t("لا موجز بعد.", "No brief yet."));
        }
    }

    // 3. Thermal.
    let th = thermal::report(d.model, p);
    let _ = write!(o, "<h2>3. {}</h2>", t("غلاف المبنى الحراري (SBC 602)", "Thermal envelope (SBC 602)"));
    let rows: Vec<Vec<String>> = th.rows.iter().map(|r| vec![esc(r.class), format!("{:.2}", r.area_m2), r.u.map_or("-".into(), |u| u.to_string()), r.limit.map_or("-".into(), |u| u.to_string()), esc(&r.status)]).collect();
    table(&mut o, &[t("الفئة", "Class"), t("المساحة م²", "Area m²"), "U", t("حد المنطقة", "Zone limit"), t("الحالة", "Status")], &rows);
    let _ = write!(o, "<p>{}: <b>{}</b> · {}: <b>{}</b> · {}: <b>{}</b> · {}: <b>{}</b></p>", t("المنطقة", "Zone"), th.zone.map_or("-".to_string(), |z| z.to_string()), t("U المرجّح", "Weighted U"), th.weighted_u.map_or("-".to_string(), |u| u.to_string()), t("النسبة إلى المرجع", "Ratio to reference"), th.ratio.map_or("-".to_string(), |u| u.to_string()), t("الفئة", "Band"), th.band.unwrap_or("-"));
    let _ = write!(o, "<p style=\"font-size:11px\">{}<br>{}<br>{}</p>", esc(&th.limits_source), esc(if arabic { th.caveat_ar } else { th.caveat_en }), esc(&th.notes.join(" · ")));
    if !th.missing.is_empty() {
        let _ = write!(o, "<p style=\"color:#e65100\">{}: {}</p>", t("مدخلات ناقصة", "Missing inputs"), esc(&th.missing.join("; ")));
    }

    // 4. Concrete.
    let _ = write!(o, "<h2>4. {}</h2>", t("الخرسانة والغطاء", "Concrete grade and cover"));
    let mal = malath::report(&p.meta.compliance);
    let _ = write!(o, "<p>{} {} MPa ({}); {}.</p>", t("أقل مقاومة انضغاط للخرسانة الإنشائية العادية:", "Least specified strength of normal-weight structural concrete:"), crate::presets::FC_MIN_MPA, esc(&crate::presets::FC_SOURCE.text()), esc(&crate::presets::EXPOSURE_SOURCE.text()));
    match &mal.cover {
        Some(c) => {
            let _ = write!(o, "<p>{}: {} / {} mm — {} ({})</p>", t("الغطاء المحدد / المطلوب", "Specified / required cover"), c.specified_mm.map_or("-".into(), |v| v.to_string()), c.required_mm.map_or("-".into(), |v| v.to_string()), status_cell(match c.ok { Some(true) => "ok", Some(false) => "violation", None => "skipped" }, arabic), esc(&c.source));
        }
        None => {
            let _ = write!(o, "<p>{}</p>", t("لم يُدخل الغطاء الخرساني: لم يُفحص.", "No concrete cover entered: not checked."));
        }
    }

    // 5. Audits.
    let acc = audits::accessibility(d.model, p);
    let vent = audits::ventilation(d.model, p);
    let cant = audits::cantilever(d.model, p);
    for (n, title, rep) in [(5, t("إمكانية الوصول (حدود يدخلها المستخدم)", "Accessibility (limits entered by the user)"), &acc), (6, t("تهوية المطابخ ودورات المياه والمناور (5-1-6)", "Kitchen and bathroom ventilation and lightwells (5-1-6)"), &vent), (7, t("البروزات والشرفات (5-1-11)", "Projections and balconies (5-1-11)"), &cant)] {
        let _ = write!(o, "<h2>{n}. {title}</h2>");
        if rep.rows.is_empty() {
            let _ = write!(o, "<p>{}</p>", t("لا عناصر للفحص.", "Nothing to check."));
        } else {
            table(&mut o, &[t("الحالة", "Status"), t("البند", "Clause"), t("الوصف", "Finding"), t("العنصر", "Element"), t("المصدر", "Source")], &audit_rows(rep, arabic));
        }
    }

    // 8. Malath checklist.
    let _ = write!(o, "<h2>8. {}</h2><p style=\"font-size:11px\">{}</p>", t("قائمة تدقيق العيوب الخفية (ملاذ)", "Latent-defects (Malath) audit checklist"), esc(if arabic { mal.disclaimer_ar } else { mal.disclaimer_en }));
    let rows: Vec<Vec<String>> = mal.items.iter().map(|i| vec![esc(i.id), esc(if arabic { i.ar } else { i.en }), status_cell(match i.status.as_str() { "accepted" => "ok", "rejected" => "violation", "na" => "info", _ => "skipped" }, arabic), esc(&i.note), esc(&i.source)]).collect();
    table(&mut o, &[t("الرمز", "Id"), t("البند", "Item"), t("الحالة", "Status"), t("ملاحظة", "Note"), t("المصدر", "Source")], &rows);
    let rows: Vec<Vec<String>> = mal.stages.iter().map(|s| vec![esc(if arabic { s.ar } else { s.en }), s.accepted.to_string(), s.rejected.to_string(), s.pending.to_string(), (if s.complete { "✓" } else { "" }).to_string()]).collect();
    table(&mut o, &[t("المرحلة", "Stage"), t("مقبول", "Accepted"), t("مرفوض", "Rejected"), t("معلّق", "Pending"), t("مكتملة", "Complete")], &rows);

    // 9. Audit trail totals.
    let _ = write!(o, "<h2>9. {}</h2>", t("ملخص مسار التدقيق", "Audit trail summary"));
    let tot = |r: &AuditReport, s: &str| r.count(s).to_string();
    table(&mut o, &[t("الفحص", "Audit"), t("مطابق", "Pass"), t("تنبيه", "Warn"), t("مخالفة", "Violation"), t("لم يُفحص", "Skipped")], &[
        vec![t("إمكانية الوصول", "Accessibility").into(), tot(&acc, "ok"), tot(&acc, "warn"), tot(&acc, "violation"), tot(&acc, "skipped")],
        vec![t("التهوية", "Ventilation").into(), tot(&vent, "ok"), tot(&vent, "warn"), tot(&vent, "violation"), tot(&vent, "skipped")],
        vec![t("البروزات", "Projections").into(), tot(&cant, "ok"), tot(&cant, "warn"), tot(&cant, "violation"), tot(&cant, "skipped")],
        vec![t("فحوص الكود على المخطط", "Code checks on the plan").into(), rep.checked.saturating_sub(rep.violations.len()).to_string(), "0".into(), rep.violations.len().to_string(), rep.skipped.len().to_string()],
    ]);

    // 10. Revisions and the review box.
    o.push_str(&revisions::history_html(p, d.changes, arabic));
    let _ = write!(o, "<h2>{}</h2><table class=\"sig\" style=\"width:100%;border-collapse:collapse\"><tr><td style=\"{TD}\">{}</td><td style=\"{TD}\">{}</td><td style=\"{TD}\">{}</td></tr></table></body></html>", t("مراجعة المهندس المرخّص", "Licensed engineer's review"), t("الاسم والرقم", "Name and licence no."), t("التوقيع", "Signature"), t("التاريخ", "Date"));
    o
}
