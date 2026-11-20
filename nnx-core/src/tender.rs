//! Tender specifications and BOQ pack: one contractor-ready package.
//!
//! The pack combines, from the project and nothing else:
//! - the project description (title, type, style, floors, plot, the structural loads and soil value the user entered, each with its
//!   source) and the scope text and technical notes the user wrote (`meta.execution.tender`);
//! - the finishes chosen for the project and the specification text of each BOQ item (user-entered, with their sources);
//! - the priced BOQ: quantities from the model, rates and mark-ups from the user, with the subtotal, mark-ups and total (live formulas in
//!   the workbook, so the contractor can change a rate and see the total move);
//! - the selected cost tier's breakdown by WBS phase and, when entered, the payment milestones.
//!
//! What is missing (no specification, no rate, no source, loads not entered) is listed in the pack by name, and the totals say PARTIAL
//! when any item is unpriced. Nothing is filled in on the user's behalf.

use crate::boq::BoqReport;
use crate::costtier::{CostReport, TierResult};
use crate::execution::{blank, esc, parse_date};
use crate::finishes::FinishReport;
use crate::payment::PaymentReport;
use crate::project::NanoArchProject;
use crate::xlsx::{Cell, Sheet, Workbook, boq_workbook, style};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenderNote {
    pub title: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TenderInputs {
    /// The scope of work in the user's words.
    pub scope: Option<String>,
    /// Technical specification notes and general conditions, each a titled paragraph.
    pub notes: Vec<TenderNote>,
    /// `YYYY-MM-DD`.
    pub bid_deadline: Option<String>,
    pub validity_days: Option<u32>,
    pub contact: Option<String>,
}

impl TenderInputs {
    pub fn check(&self) -> Result<(), String> {
        if let Some(d) = &self.bid_deadline {
            parse_date("bid deadline", d)?;
        }
        if self.validity_days.is_some_and(|v| v == 0 || v > 3650) {
            return Err("the bid validity must be between 1 and 3650 days".into());
        }
        if let Some(n) = self.notes.iter().find(|n| n.title.trim().is_empty() || n.text.trim().is_empty()) {
            return Err(format!("a tender note needs a title and a text (note '{}')", n.title));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Fact {
    pub en: &'static str,
    pub ar: &'static str,
    pub value: String,
    pub source: Option<String>,
}

/// What the pack is built from.
pub struct TenderParts<'a> {
    pub project: &'a NanoArchProject,
    pub boq: BoqReport,
    pub cost: CostReport,
    pub finishes: FinishReport,
    pub payment: Option<PaymentReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TenderPack {
    pub title: String,
    pub facts: Vec<Fact>,
    pub inputs: TenderInputs,
    pub finishes: FinishReport,
    pub boq: BoqReport,
    pub cost: CostReport,
    pub payment: Option<PaymentReport>,
    pub missing: Vec<String>,
}

pub fn pack(parts: TenderParts<'_>) -> TenderPack {
    let p = parts.project;
    let user = Some("entered by the user".to_string());
    let mut facts = vec![Fact { en: "Project", ar: "المشروع", value: if p.meta.title.trim().is_empty() { "NanoArch project".into() } else { p.meta.title.clone() }, source: None }];
    let mut missing = Vec::new();
    if let Some(bt) = p.meta.building_type {
        let (en, ar) = bt.label();
        facts.push(Fact { en: "Building type", ar: "نوع المبنى", value: format!("{en} / {ar}"), source: None });
    }
    if let Some(s) = p.meta.style.as_deref().filter(|s| !s.trim().is_empty()) {
        facts.push(Fact { en: "Style", ar: "الطراز", value: s.to_string(), source: user.clone() });
    }
    if let Some(z) = p.meta.climate_zone.as_deref().filter(|s| !s.trim().is_empty()) {
        facts.push(Fact { en: "Climate zone", ar: "المنطقة المناخية", value: z.to_string(), source: user.clone() });
    }
    if let Some(a) = p.plot_area_m2() {
        facts.push(Fact { en: "Plot area (m2)", ar: "مساحة الأرض (م٢)", value: format!("{a:.2}"), source: None });
    }
    if let Some(a) = parts.cost.gross_area_m2 {
        facts.push(Fact { en: "Gross floor area (m2)", ar: "إجمالي مساحة الأدوار (م٢)", value: format!("{a:.2}"), source: Some("measured from the model".into()) });
    }
    if let Some(f) = p.structure.floors {
        facts.push(Fact { en: "Floors", ar: "عدد الأدوار", value: f.to_string(), source: user.clone() });
    }
    let loads = [("Dead load (kPa)", "الحمل الميت (كيلوباسكال)", p.structure.dead_kpa), ("Live load (kPa)", "الحمل الحي (كيلوباسكال)", p.structure.live_kpa), ("Soil bearing (kPa)", "قدرة تحمل التربة (كيلوباسكال)", p.structure.bearing_kpa)];
    for (en, ar, v) in loads {
        match v {
            Some(v) => facts.push(Fact { en, ar, value: format!("{v}"), source: user.clone() }),
            None => missing.push(format!("{en}: not entered")),
        }
    }
    if !p.meta.smart_tech.is_empty() {
        facts.push(Fact { en: "Smart technology", ar: "التقنيات الذكية", value: p.meta.smart_tech.join(", "), source: user.clone() });
    }
    let t = &p.meta.execution.tender;
    if blank(&t.scope) {
        missing.push("scope of work: not written".into());
    }
    if t.notes.is_empty() {
        missing.push("technical notes: none written".into());
    }
    for c in &parts.boq.missing.no_spec {
        missing.push(format!("specification of {c}"));
    }
    for c in &parts.boq.missing.no_rate {
        missing.push(format!("rate of {c}"));
    }
    for s in &parts.boq.missing.no_source {
        missing.push(format!("source of the {s}"));
    }
    if parts.cost.selected_result().is_none() {
        missing.push("cost tier breakdown: no tier selected".into());
    }
    TenderPack { title: facts[0].value.clone(), facts, inputs: t.clone(), finishes: parts.finishes, boq: parts.boq, cost: parts.cost, payment: parts.payment, missing }
}

fn text(s: &str, st: u8) -> (Cell, u8) {
    (Cell::Text(s.to_string()), st)
}

impl TenderPack {
    fn currency(&self) -> &str {
        self.boq.project.currency.as_deref().or(self.cost.currency.as_deref()).unwrap_or("")
    }

    fn state(&self) -> &'static str {
        if self.boq.complete { "complete" } else { "PARTIAL" }
    }

    /// The pack as a workbook with live formulas: the summary, the BOQ, the materials, the specifications, the cost tier, the payment
    /// milestones and the technical notes.
    pub fn workbook(&self, arabic: bool) -> Workbook {
        let t = |ar: &'static str, en: &'static str| if arabic { ar } else { en };
        let hdr = |labels: &[&str]| -> Vec<(Cell, u8)> { labels.iter().map(|l| text(l, style::HEADER)).collect() };
        let mut book = boq_workbook(&self.boq, arabic);
        let boq_last = (self.boq.rows.len() + 1).max(2);

        // ---- cost sheet (selected tier)
        let tier: Option<&TierResult> = self.cost.selected_result();
        let cost_name = t("التكلفة", "Cost").to_string();
        let mut cost_total_cell: Option<String> = None;
        let mut cost_sheet = None;
        if let Some(tr) = tier {
            let mut rows = vec![hdr(&[t("المرحلة", "Phase"), t("البند", "Line"), t("الوحدة", "Unit"), t("الكمية", "Quantity"), t("السعر", "Rate"), t("المبلغ", "Amount"), t("المصدر", "Source")])];
            for l in &tr.lines {
                let n = rows.len() + 1;
                let lump = l.unit.is_none() && l.quantity.is_none() && l.amount.is_some();
                let amount = match (lump, l.quantity, l.amount) {
                    (true, _, Some(a)) => (Cell::FormulaNum(format!("E{n}"), a), style::MONEY),
                    (_, Some(_), Some(a)) => (Cell::FormulaNum(format!("ROUND(D{n}*E{n},2)"), a), style::MONEY),
                    _ => (Cell::Empty, style::MONEY),
                };
                rows.push(vec![
                    text(&l.phase, style::TEXT),
                    text(&l.label, style::TEXT),
                    text(l.unit.as_deref().unwrap_or(""), style::TEXT),
                    l.quantity.map_or((Cell::Empty, style::QTY), |q| (Cell::Num(q), style::QTY)),
                    (Cell::Num(l.rate), style::MONEY),
                    amount,
                    text(&l.source, style::TEXT),
                ]);
            }
            let last = rows.len().max(2);
            rows.push(vec![]);
            let n = rows.len() + 1;
            let mut tot = vec![text(t("إجمالي المستوى", "Tier total"), style::TOTAL_TEXT)];
            tot.extend(vec![(Cell::Empty, style::TOTAL_TEXT); 4]);
            tot.push((Cell::FormulaNum(format!("SUM(F2:F{last})"), tr.total), style::TOTAL_MONEY));
            tot.push(text(if tr.complete { "complete" } else { "PARTIAL" }, style::TOTAL_TEXT));
            rows.push(tot);
            cost_total_cell = Some(format!("'{cost_name}'!F{n}"));
            rows.push(vec![]);
            rows.push(vec![text(t("حسب المرحلة", "By phase"), style::BOLD)]);
            for p in &tr.phases {
                let n = rows.len() + 1;
                rows.push(vec![text(p.phase, style::TEXT), (Cell::Empty, style::TEXT), (Cell::Empty, style::TEXT), (Cell::Empty, style::TEXT), (Cell::Empty, style::TEXT), (Cell::FormulaNum(format!("SUMIF(A2:A{last},A{n},F2:F{last})"), p.amount), style::MONEY)]);
            }
            cost_sheet = Some(Sheet { name: cost_name.clone(), rows, widths: vec![18.0, 40.0, 8.0, 14.0, 12.0, 16.0, 30.0], rtl: arabic, header: true, filter_to: 0 });
        }

        // ---- summary sheet
        let mut rows: Vec<Vec<(Cell, u8)>> = vec![vec![text(&format!("{} — {}", t("حزمة المناقصة", "Tender pack"), self.title), style::BOLD)]];
        let inputs = &self.inputs;
        let mut info: Vec<(&str, String)> = Vec::new();
        if let Some(d) = &inputs.bid_deadline {
            info.push((t("آخر موعد للعروض", "Bid deadline"), d.clone()));
        }
        if let Some(v) = inputs.validity_days {
            info.push((t("سريان العرض (يوم)", "Bid validity (days)"), v.to_string()));
        }
        if let Some(c) = inputs.contact.as_deref().filter(|c| !c.trim().is_empty()) {
            info.push((t("جهة الاتصال", "Contact"), c.to_string()));
        }
        info.push((t("العملة", "Currency"), self.currency().to_string()));
        info.push((t("حالة الأسعار", "Pricing status"), self.state().to_string()));
        for (k, v) in info {
            rows.push(vec![text(k, style::TEXT), text(&v, style::TEXT)]);
        }
        rows.push(vec![]);
        rows.push(vec![text(t("المبالغ", "Amounts"), style::HEADER), text(t("المبلغ", "Amount"), style::HEADER)]);
        let sub = rows.len() + 1;
        rows.push(vec![text(t("المجموع الفرعي لجدول الكميات", "BOQ subtotal"), style::TEXT), (Cell::FormulaNum(format!("SUM(BOQ!K2:K{boq_last})"), self.boq.subtotal), style::MONEY)]);
        let first = rows.len() + 1;
        for (i, m) in self.boq.markups.iter().enumerate() {
            let n = rows.len() + 1;
            let f = if i == 0 { format!("ROUND(B{sub}*{}/100,2)", m.pct) } else { format!("ROUND((B{sub}+SUM(B{first}:B{}))*{}/100,2)", n - 1, m.pct) };
            rows.push(vec![text(&format!("{} ({}%)", m.name, m.pct), style::TEXT), (Cell::FormulaNum(f, m.amount), style::MONEY)]);
        }
        let total_f = if self.boq.markups.is_empty() { format!("B{sub}") } else { format!("B{sub}+SUM(B{first}:B{})", rows.len()) };
        rows.push(vec![text(&format!("{} {}", t("إجمالي العطاء", "Tender total"), self.currency()), style::TOTAL_TEXT), (Cell::FormulaNum(total_f, self.boq.total), style::TOTAL_MONEY)]);
        rows.push(vec![]);
        match (tier, &cost_total_cell) {
            (Some(tr), Some(cell)) => {
                rows.push(vec![text(&format!("{}: {}", t("المستوى المختار", "Selected tier"), if arabic { tr.ar } else { tr.en }), style::TEXT), (Cell::FormulaNum(cell.clone(), tr.total), style::MONEY)]);
                if let Some(m2) = tr.per_m2 {
                    rows.push(vec![text(t("التكلفة للمتر المربع", "Cost per m2"), style::TEXT), (Cell::Num(m2), style::MONEY)]);
                }
            }
            _ => rows.push(vec![text(t("المستوى المختار", "Selected tier"), style::TEXT), text(t("لم يُختر", "not selected"), style::TEXT)]),
        }
        if !self.missing.is_empty() {
            rows.push(vec![]);
            rows.push(vec![text(&format!("{} ({})", t("نواقص", "Missing"), self.missing.len()), style::BOLD)]);
            for m in &self.missing {
                rows.push(vec![(Cell::Empty, style::DEFAULT), text(m, style::DEFAULT)]);
            }
        }
        let summary = Sheet { name: t("المناقصة", "Tender").into(), rows, widths: vec![46.0, 30.0], rtl: arabic, header: false, filter_to: 0 };

        // ---- technical sheet
        let mut rows: Vec<Vec<(Cell, u8)>> = vec![hdr(&[t("البند", "Item"), t("القيمة", "Value"), t("المصدر", "Source")])];
        for f in &self.facts {
            rows.push(vec![text(if arabic { f.ar } else { f.en }, style::TEXT), text(&f.value, style::TEXT), text(f.source.as_deref().unwrap_or(""), style::TEXT)]);
        }
        for l in &self.finishes.lines {
            rows.push(vec![text(&format!("{} ({})", t("التشطيب", "Finish"), l.kind), style::TEXT), text(if arabic { l.ar } else { l.en }, style::TEXT), text(&format!("{:.2} m2", l.area_m2), style::TEXT)]);
        }
        if let Some(s) = inputs.scope.as_deref().filter(|s| !s.trim().is_empty()) {
            rows.push(vec![text(t("نطاق العمل", "Scope of work"), style::TEXT), text(s, style::TEXT)]);
        }
        for n in &inputs.notes {
            rows.push(vec![text(&n.title, style::TEXT), text(&n.text, style::TEXT)]);
        }
        let technical = Sheet { name: t("المواصفات الفنية", "Technical notes").into(), rows, widths: vec![34.0, 90.0, 24.0], rtl: arabic, header: true, filter_to: 0 };

        let mut sheets = vec![summary];
        sheets.append(&mut book.sheets);
        sheets.extend(cost_sheet);
        sheets.push(technical);
        if let Some(pay) = self.payment.as_ref().filter(|p| !p.milestones.is_empty()) {
            let mut rows = vec![hdr(&[t("الدفعة", "Milestone"), t("مرتبطة بـ", "Linked to"), t("التاريخ", "Date"), t("النسبة %", "Percent"), t("المبلغ", "Amount"), t("التراكمي", "Cumulative")])];
            for m in &pay.milestones {
                let n = rows.len() + 1;
                rows.push(vec![
                    text(&m.name, style::TEXT),
                    text(&m.link.label(), style::TEXT),
                    text(m.date.as_deref().unwrap_or(""), style::TEXT),
                    m.pct.map_or((Cell::Empty, style::PCT), |p| (Cell::Num(p), style::PCT)),
                    m.amount.map_or((Cell::Empty, style::MONEY), |a| (Cell::Num(a), style::MONEY)),
                    m.cumulative_amount.map_or((Cell::Empty, style::MONEY), |c| (Cell::FormulaNum(format!("SUM(E$2:E{n})"), c), style::MONEY)),
                ]);
            }
            let last = rows.len().max(2);
            rows.push(vec![text(t("المجموع", "Total"), style::TOTAL_TEXT), (Cell::Empty, style::TOTAL_TEXT), (Cell::Empty, style::TOTAL_TEXT), (Cell::FormulaNum(format!("SUM(D2:D{last})"), pay.allocated_pct), style::TOTAL_MONEY), (Cell::FormulaNum(format!("SUM(E2:E{last})"), pay.allocated_amount.unwrap_or(0.0)), style::TOTAL_MONEY), (Cell::Empty, style::TOTAL_TEXT)]);
            sheets.push(Sheet { name: t("الدفعات", "Payments").into(), rows, widths: vec![36.0, 24.0, 12.0, 12.0, 16.0, 16.0], rtl: arabic, header: true, filter_to: 0 });
        }
        Workbook { sheets }
    }

    /// The printable pack (A4): the same content as the workbook, for the PDF.
    pub fn html(&self, arabic: bool) -> String {
        let t = |ar: &'static str, en: &'static str| if arabic { ar } else { en };
        let cur = esc(self.currency());
        let n2 = |v: Option<f64>| v.map_or(String::new(), |x| format!("{x:.2}"));
        let mut h = format!(
            "<!doctype html><html lang=\"{}\" dir=\"{}\"><head><meta charset=\"utf-8\"><title>{}</title><style>\
             body{{font-family:'Segoe UI',Tahoma,sans-serif;margin:28px;color:#111}} h1{{font-size:22px;margin:0 0 4px}} h2{{font-size:16px;margin:24px 0 8px;border-bottom:2px solid #333;padding-bottom:3px}}\
             table{{border-collapse:collapse;width:100%;font-size:12px}} th,td{{border:1px solid #999;padding:4px 6px;text-align:start;vertical-align:top}} th{{background:#eee}}\
             td.n{{text-align:end;font-variant-numeric:tabular-nums}} tr.sec td{{background:#f4f4f4;font-weight:700}} tr.tot td{{font-weight:700;background:#fafafa}}\
             .warn{{color:#a33}} .note{{font-size:11px;color:#444;margin-top:18px}} p.pre{{white-space:pre-wrap}} @page{{size:A4 portrait;margin:12mm;@bottom-center{{content:counter(page) \" / \" counter(pages);direction:ltr;font-size:9pt;color:#555}}}} thead{{display:table-header-group}} @media print{{body{{margin:0}} h2{{break-after:avoid}} tr{{break-inside:avoid}}}}</style></head><body>",
            if arabic { "ar" } else { "en" },
            if arabic { "rtl" } else { "ltr" },
            esc(&self.title)
        );
        h.push_str(&format!("<h1>{} — {}</h1>", t("حزمة المناقصة: المواصفات وجدول الكميات", "Tender pack: specifications and bill of quantities"), esc(&self.title)));
        let i = &self.inputs;
        let mut meta = String::new();
        for (k, v) in [(t("آخر موعد للعروض", "Bid deadline"), i.bid_deadline.clone()), (t("سريان العرض (يوم)", "Bid validity (days)"), i.validity_days.map(|v| v.to_string())), (t("جهة الاتصال", "Contact"), i.contact.clone()), (t("العميل", "Client"), self.boq.project.client.clone()), (t("الموقع", "Location"), self.boq.project.location.clone()), (t("التاريخ", "Date"), self.boq.project.date.clone())] {
            if let Some(v) = v.filter(|v| !v.trim().is_empty()) {
                meta.push_str(&format!("<tr><td><b>{k}</b></td><td>{}</td></tr>", esc(&v)));
            }
        }
        if !meta.is_empty() {
            h.push_str(&format!("<table>{meta}</table>"));
        }
        if !self.boq.complete {
            h.push_str(&format!("<p class=\"warn\"><b>{}</b></p>", t("تنبيه: الأسعار ناقصة لبعض البنود، والإجماليات جزئية.", "Warning: some items have no rate, so the totals are PARTIAL.")));
        }
        h.push_str(&format!("<h2>1. {}</h2><table><thead><tr><th>{}</th><th>{}</th><th>{}</th></tr></thead><tbody>", t("وصف المشروع", "Project description"), t("البند", "Item"), t("القيمة", "Value"), t("المصدر", "Source")));
        for f in &self.facts {
            h.push_str(&format!("<tr><td>{}</td><td>{}</td><td>{}</td></tr>", if arabic { f.ar } else { f.en }, esc(&f.value), esc(f.source.as_deref().unwrap_or(""))));
        }
        for l in &self.finishes.lines {
            h.push_str(&format!("<tr><td>{} ({})</td><td>{}</td><td>{:.2} m2</td></tr>", t("التشطيب", "Finish"), l.kind, if arabic { l.ar } else { l.en }, l.area_m2));
        }
        h.push_str("</tbody></table>");
        if let Some(s) = i.scope.as_deref().filter(|s| !s.trim().is_empty()) {
            h.push_str(&format!("<h2>2. {}</h2><p class=\"pre\">{}</p>", t("نطاق العمل", "Scope of work"), esc(s)));
        }
        if !i.notes.is_empty() {
            h.push_str(&format!("<h2>3. {}</h2>", t("المواصفات الفنية والشروط", "Technical specifications and conditions")));
            for n in &i.notes {
                h.push_str(&format!("<h3>{}</h3><p class=\"pre\">{}</p>", esc(&n.title), esc(&n.text)));
            }
        }
        h.push_str(&format!(
            "<h2>4. {}</h2><table><thead><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr></thead><tbody>",
            t("جدول الكميات والأسعار", "Priced bill of quantities"),
            t("رقم", "No."),
            t("الوصف", "Description"),
            t("المواصفة", "Specification"),
            t("الوحدة", "Unit"),
            t("الكمية", "Quantity"),
            t("السعر", "Rate"),
            t("المبلغ", "Amount")
        ));
        let mut last = "";
        for r in &self.boq.rows {
            if r.section != last {
                h.push_str(&format!("<tr class=\"sec\"><td colspan=\"7\">{}</td></tr>", esc(&r.section)));
                last = &r.section;
            }
            h.push_str(&format!(
                "<tr><td>{}</td><td>{}<br><small>{}</small></td><td>{}</td><td>{}</td><td class=\"n\">{:.2}</td><td class=\"n\">{}</td><td class=\"n\">{}</td></tr>",
                r.no,
                esc(&r.description),
                esc(&r.code),
                esc(r.spec.as_deref().unwrap_or("—")),
                r.unit,
                r.quantity,
                n2(r.rate),
                if r.reference_only { t("مرجعي", "ref.").to_string() } else { n2(r.amount) }
            ));
        }
        h.push_str(&format!("<tr class=\"tot\"><td colspan=\"6\">{} {cur}</td><td class=\"n\">{:.2}</td></tr>", t("المجموع الفرعي", "Subtotal"), self.boq.subtotal));
        for m in &self.boq.markups {
            h.push_str(&format!("<tr><td colspan=\"6\">{} ({}%)</td><td class=\"n\">{:.2}</td></tr>", esc(&m.name), m.pct, m.amount));
        }
        h.push_str(&format!("<tr class=\"tot\"><td colspan=\"6\">{} {cur} ({})</td><td class=\"n\">{:.2}</td></tr></tbody></table>", t("إجمالي العطاء", "Tender total"), self.state(), self.boq.total));
        if let Some(tr) = self.cost.selected_result() {
            h.push_str(&format!("<h2>5. {}: {}</h2><table><thead><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr></thead><tbody>", t("تكلفة المستوى المختار", "Selected cost tier"), if arabic { tr.ar } else { tr.en }, t("المرحلة", "Phase"), t("البند", "Line"), t("الكمية", "Quantity"), t("السعر", "Rate"), t("المبلغ", "Amount"), t("المصدر", "Source")));
            for l in &tr.lines {
                h.push_str(&format!("<tr><td>{}</td><td>{}</td><td class=\"n\">{} {}</td><td class=\"n\">{:.2}</td><td class=\"n\">{}</td><td>{}</td></tr>", esc(&l.phase), esc(&l.label), n2(l.quantity), esc(l.unit.as_deref().unwrap_or("")), l.rate, n2(l.amount), esc(&l.source)));
            }
            for p in tr.phases.iter().filter(|p| p.lines > 0) {
                h.push_str(&format!("<tr class=\"sec\"><td colspan=\"4\">{}</td><td class=\"n\">{:.2}</td><td></td></tr>", esc(p.phase), p.amount));
            }
            h.push_str(&format!("<tr class=\"tot\"><td colspan=\"4\">{} {cur} ({})</td><td class=\"n\">{:.2}</td><td></td></tr></tbody></table>", t("إجمالي المستوى", "Tier total"), if tr.complete { "complete" } else { "PARTIAL" }, tr.total));
        }
        if let Some(pay) = self.payment.as_ref().filter(|p| !p.milestones.is_empty()) {
            h.push_str(&format!("<h2>6. {}</h2><table><thead><tr><th>{}</th><th>{}</th><th>{}</th><th>%</th><th>{}</th></tr></thead><tbody>", t("جدول الدفعات", "Payment schedule"), t("الدفعة", "Milestone"), t("مرتبطة بـ", "Linked to"), t("التاريخ", "Date"), t("المبلغ", "Amount")));
            for m in &pay.milestones {
                h.push_str(&format!("<tr><td>{}</td><td>{}</td><td>{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td></tr>", esc(&m.name), esc(&m.link.label()), m.date.as_deref().unwrap_or("—"), m.pct.map_or(String::new(), |p| format!("{p:.2}")), n2(m.amount)));
            }
            h.push_str("</tbody></table>");
        }
        if !self.missing.is_empty() {
            h.push_str(&format!("<h2>{} ({})</h2><ul>", t("نواقص تحتاج إكمالاً", "Gaps to complete"), self.missing.len()));
            for m in &self.missing {
                h.push_str(&format!("<li class=\"warn\">{}</li>", esc(m)));
            }
            h.push_str("</ul>");
        }
        for w in self.boq.warnings.iter().chain(self.cost.notes.iter()) {
            h.push_str(&format!("<p class=\"warn\">{}</p>", esc(w)));
        }
        h.push_str(&format!("<p class=\"note\">{}</p></body></html>", t("أولي: الكميات مقيسة من النموذج، والمواصفات والأسعار والنسب من مدخلات المستخدم بمصادرها. يراجعه مهندس مرخّص قبل الطرح.", "Preliminary: quantities are measured from the model; specifications, rates and percentages are the user's inputs with their sources. To be checked by a licensed engineer before it is issued.")));
        h
    }
}
