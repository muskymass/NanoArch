//! The change report between two versions of a project: a printable bilingual document and a CSV, built from the stored
//! snapshot comparison (`snapshot::compare`). It states what the comparison found and nothing else: no value is invented, a
//! total whose change was not computed (partial BOQ, different cost tiers) is printed without a change and with the reason, and
//! a version that holds no priced item shows no cost line.
//!
//! Only what a snapshot stores is compared: elements (with every property), rooms by requirement key, take-off quantities and
//! the cost figures. Project inputs outside the model (limits, structural or MEP inputs) are not part of a snapshot and are not
//! in this report.

use crate::snapshot::{Diff, PropChange};
use serde_json::Value;
use std::fmt::Write;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn csv_field(s: &str) -> String {
    let guarded = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{s}") } else { s.to_string() };
    format!("\"{}\"", guarded.replace('"', "\"\""))
}

fn val(v: &Option<Value>) -> String {
    match v {
        None | Some(Value::Null) => "-".into(),
        Some(Value::String(s)) => s.clone(),
        Some(o) => o.to_string(),
    }
}

fn num(v: Option<f64>) -> String {
    v.map_or_else(|| "-".into(), |x| format!("{x}"))
}

fn status_label(s: &str, ar: bool) -> &'static str {
    match (s, ar) {
        ("added", true) => "مضاف",
        ("removed", true) => "محذوف",
        ("modified", true) => "معدّل",
        ("added", false) => "Added",
        ("removed", false) => "Removed",
        _ => "Modified",
    }
}

fn change_text(c: &PropChange) -> String {
    format!("{}: {} -> {}", c.key, val(&c.from), val(&c.to))
}

/// Whether the two versions differ in anything the comparison covers (elements, rooms, quantities, cost rows or a total).
pub fn has_changes(d: &Diff) -> bool {
    !d.elements.is_empty()
        || !d.rooms.is_empty()
        || !d.quantities.is_empty()
        || !d.costs.is_empty()
        || d.totals.boq_delta.is_some_and(|x| x != 0.0)
        || d.totals.tier_delta.is_some_and(|x| x != 0.0)
}

/// The report as a self-contained HTML document (RTL when `arabic`). `title` is the project title, `date` is `YYYY-MM-DD`.
pub fn html(d: &Diff, title: &str, date: &str, arabic: bool) -> String {
    let t = |ar: &'static str, en: &'static str| esc(if arabic { ar } else { en });
    let mut h = String::new();
    let _ = write!(
        h,
        "<!DOCTYPE html><html lang=\"{}\" dir=\"{}\"><head><meta charset=\"utf-8\"><title>{}</title><style>body{{font-family:Segoe UI,Tahoma,sans-serif;margin:24px;color:#111}}table{{border-collapse:collapse;width:100%;margin:8px 0 20px}}th,td{{border:1px solid #999;padding:4px 8px;text-align:start;vertical-align:top}}th{{background:#eee}}h1{{font-size:20px}}h2{{font-size:16px;margin-top:22px}}.note{{color:#555;font-size:12px}}</style></head><body>",
        if arabic { "ar" } else { "en" },
        if arabic { "rtl" } else { "ltr" },
        t("تقرير التغييرات", "Change report")
    );
    let _ = write!(h, "<h1>{} - {}</h1>", t("تقرير التغييرات بين نسختين", "Change report between two versions"), esc(title));
    let _ = write!(
        h,
        "<table><tr><th></th><th>{}</th><th>{}</th></tr><tr><td>{}</td><td>{} ({})</td><td>{} ({})</td></tr><tr><td>{}</td><td>{}</td><td>{}</td></tr><tr><td>{}</td><td colspan=\"2\">{}</td></tr></table>",
        t("من", "From"),
        t("إلى", "To"),
        t("النسخة", "Version"),
        esc(&d.a.name),
        esc(&d.a.id),
        esc(&d.b.name),
        esc(&d.b.id),
        t("عدد العناصر", "Elements"),
        d.a.elements,
        d.b.elements,
        t("تاريخ التقرير", "Report date"),
        esc(date)
    );
    let _ = write!(
        h,
        "<p>{}: {} | {}: {} | {}: {}</p>",
        t("مضاف", "Added"),
        d.added,
        t("محذوف", "Removed"),
        d.removed,
        t("معدّل", "Modified"),
        d.modified
    );
    if !has_changes(d) {
        let _ = write!(h, "<p>{}</p>", t("لا فرق بين النسختين في العناصر والغرف والكميات والتكلفة المخزنة.", "No difference between the two versions in the stored elements, rooms, quantities or cost."));
    }
    if !d.elements.is_empty() {
        let _ = write!(h, "<h2>{}</h2><table><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr>", t("العناصر", "Elements"), t("المعرّف", "Id"), t("النوع", "Kind"), t("الاسم", "Label"), t("الحالة", "Status"), t("تغيّر الخصائص", "Property changes"));
        for e in &d.elements {
            let ch = e.changes.iter().map(|c| esc(&change_text(c))).collect::<Vec<_>>().join("<br>");
            let _ = write!(h, "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>", esc(&e.id), esc(&e.kind), esc(&e.label), status_label(e.status, arabic), ch);
        }
        h.push_str("</table>");
    }
    if !d.rooms.is_empty() {
        let _ = write!(h, "<h2>{}</h2><table><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr>", t("الغرف (م²)", "Rooms (m²)"), t("المفتاح", "Key"), t("الحالة", "Status"), t("من", "From"), t("إلى", "To"), t("الفرق", "Change"));
        for r in &d.rooms {
            let _ = write!(h, "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>", esc(&r.key), status_label(r.status, arabic), num(r.area_from), num(r.area_to), r.delta);
        }
        h.push_str("</table>");
    }
    if !d.quantities.is_empty() {
        let _ = write!(h, "<h2>{}</h2><table><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr>", t("الكميات", "Quantities"), t("الرمز", "Code"), t("الوحدة", "Unit"), t("من", "From"), t("إلى", "To"), t("الفرق", "Change"));
        for q in &d.quantities {
            let _ = write!(h, "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>", esc(&q.code), esc(&q.unit), num(q.from), num(q.to), q.delta);
        }
        h.push_str("</table>");
    }
    let cur = d.totals.currency.as_deref().unwrap_or("");
    if !d.costs.is_empty() {
        let _ = write!(h, "<h2>{} {}</h2><table><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr>", t("بنود التكلفة", "Cost rows"), esc(cur), t("الرمز", "Code"), t("من", "From"), t("إلى", "To"), t("الفرق", "Change"));
        for c in &d.costs {
            let _ = write!(h, "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>", esc(&c.code), num(c.from), num(c.to), c.delta);
        }
        h.push_str("</table>");
    }
    let tt = &d.totals;
    if tt.boq_from.is_some() || tt.boq_to.is_some() || tt.tier_from.is_some() || tt.tier_to.is_some() {
        let _ = write!(h, "<h2>{} {}</h2><table><tr><th></th><th>{}</th><th>{}</th><th>{}</th></tr>", t("الإجماليات", "Totals"), esc(cur), t("من", "From"), t("إلى", "To"), t("الفرق", "Change"));
        let _ = write!(h, "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>", t("جدول الكميات والتسعير", "BOQ"), num(tt.boq_from), num(tt.boq_to), num(tt.boq_delta));
        let _ = write!(h, "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr></table>", t("فئة التكلفة المختارة", "Selected cost tier"), num(tt.tier_from), num(tt.tier_to), num(tt.tier_delta));
    }
    for n in &d.notes {
        let _ = write!(h, "<p class=\"note\">{}</p>", esc(n));
    }
    let _ = write!(h, "<p class=\"note\">{}</p></body></html>", t("مقارنة آلية لما حُفظ في النسختين فقط؛ لا تغني عن مراجعة مهندس مرخّص.", "An automatic comparison of what the two versions stored; it does not replace review by a licensed engineer."));
    h
}

/// The report as CSV: `section,id,kind_or_code,label_or_unit,status,from,to,change`.
pub fn csv(d: &Diff) -> String {
    // A missing number is an empty cell (a dash would be guarded as a formula).
    let num = |v: Option<f64>| v.map_or_else(String::new, |x| x.to_string());
    let mut s = String::from("section,id,kind_or_code,label_or_unit,status,from,to,change\n");
    let mut row = |cells: [String; 8]| {
        s.push_str(&cells.map(|c| csv_field(&c)).join(","));
        s.push('\n');
    };
    for e in &d.elements {
        let ch = e.changes.iter().map(change_text).collect::<Vec<_>>().join("; ");
        row(["element".into(), e.id.clone(), e.kind.clone(), e.label.clone(), e.status.into(), String::new(), String::new(), ch]);
    }
    for r in &d.rooms {
        row(["room".into(), r.key.clone(), String::new(), String::new(), r.status.into(), num(r.area_from), num(r.area_to), r.delta.to_string()]);
    }
    for q in &d.quantities {
        row(["quantity".into(), String::new(), q.code.clone(), q.unit.clone(), String::new(), num(q.from), num(q.to), q.delta.to_string()]);
    }
    for c in &d.costs {
        row(["cost".into(), String::new(), c.code.clone(), String::new(), String::new(), num(c.from), num(c.to), c.delta.to_string()]);
    }
    let t = &d.totals;
    row(["total".into(), String::new(), "boq".into(), t.currency.clone().unwrap_or_default(), String::new(), num(t.boq_from), num(t.boq_to), num(t.boq_delta)]);
    row(["total".into(), String::new(), "tier".into(), t.currency.clone().unwrap_or_default(), String::new(), num(t.tier_from), num(t.tier_to), num(t.tier_delta)]);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{compare, CostSnap, SnapElement, Snapshot};

    fn snap(id: &str, name: &str) -> Snapshot {
        Snapshot { id: id.into(), name: name.into(), ..Default::default() }
    }

    fn el(kind: &str, label: &str, x: f64) -> SnapElement {
        let mut props = serde_json::Map::new();
        props.insert("x".into(), serde_json::json!(x));
        SnapElement { kind: kind.into(), label: label.into(), level: None, host: None, props }
    }

    fn pair() -> (Snapshot, Snapshot) {
        let mut a = snap("S1", "Rev <A>");
        let mut b = snap("S2", "Rev B");
        a.elements.insert("1".into(), el("Wall", "W-1", 6000.0));
        a.elements.insert("2".into(), el("Column", "=COL-2", 0.0));
        b.elements.insert("1".into(), el("Wall", "W-1", 6200.0));
        b.elements.insert("3".into(), el("Beam", "B-1", 0.0));
        a.costs = CostSnap { boq_total: Some(1000.0), boq_complete: true, currency: Some("SAR".into()), ..Default::default() };
        b.costs = CostSnap { boq_total: Some(1250.5), boq_complete: false, currency: Some("SAR".into()), ..Default::default() };
        (a, b)
    }

    #[test]
    fn report_lists_each_change_and_escapes_text() {
        let (a, b) = pair();
        let d = compare(&a, &b);
        let en = html(&d, "Villa & <Co>", "2026-11-23", false);
        assert!(en.contains("dir=\"ltr\"") && en.contains("Villa &amp; &lt;Co&gt;") && en.contains("Rev &lt;A&gt;"));
        assert!(en.contains("x: 6000.0 -&gt; 6200.0"));
        assert!(en.contains("Added") && en.contains("Removed") && en.contains("Modified"));
        assert!(!en.contains("<Co>"));
        let ar = html(&d, "x", "2026-11-23", true);
        assert!(ar.contains("dir=\"rtl\"") && ar.contains("معدّل") && ar.contains("مضاف") && ar.contains("محذوف"));
    }

    #[test]
    fn partial_total_prints_no_change_and_keeps_the_reason() {
        let (a, b) = pair();
        let d = compare(&a, &b);
        assert!(d.totals.boq_delta.is_none());
        assert!(html(&d, "x", "2026-11-23", false).contains("PARTIAL"));
        assert!(csv(&d).contains("\"total\",\"\",\"boq\",\"SAR\",\"\",\"1000\",\"1250.5\",\"\""));
    }

    #[test]
    fn csv_guards_formula_injection_and_identical_versions_report_no_difference() {
        let (a, b) = pair();
        let c = csv(&compare(&a, &b));
        assert!(c.starts_with("section,id,kind_or_code,label_or_unit,status,from,to,change\n"));
        assert!(c.contains("\"'=COL-2\""));
        let same = compare(&a, &a.clone());
        assert!(!has_changes(&same));
        assert!(html(&same, "x", "2026-11-23", false).contains("No difference between the two versions"));
        assert!(has_changes(&compare(&a, &b)));
    }
}
