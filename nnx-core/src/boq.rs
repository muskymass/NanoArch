//! Bill of Quantities (BOQ): the take-off quantities, their specifications and rates, the materials they
//! consume, and a printable document that holds all of it.
//!
//! The quantities come from the model (`qto`, `insulation`, `mep`). Everything else is YOURS and is never
//! defaulted: the specification text of each item, its rate, the waste allowance, the consumption of each
//! material per unit of work (cement per m3 of concrete, steel kg per m3, blocks per m2 ...), material rates,
//! and the percentage mark-ups. An item without a specification or a rate is listed by code, is left out of
//! the money totals, and the totals are marked partial. Steel is counted only through YOUR kg-per-m3 factors:
//! this application does not design reinforcement.
//!
//! Conventions:
//! - `quantity` is the net measured quantity, rounded to 3 decimals as shown; the BOQ amount is `rate x quantity`
//!   on that shown quantity, so every row can be checked by hand. Waste is shown separately
//!   and drives `quantity to order` and the material list only.
//! - A lookup key is an exact item code or a prefix ending in `*` (`FLOOR.AREA.*`); the longest match wins.
//! - Mark-ups are applied one after the other, each on the running total.
//! - Two items that measure the same work two ways (wall area and wall volume) are not both priced silently:
//!   a warning is raised when both carry a rate.

use crate::qto::QtoItem;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Project {
    pub name: Option<String>,
    pub client: Option<String>,
    pub location: Option<String>,
    pub date: Option<String>,
    pub prepared_by: Option<String>,
    pub currency: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ItemData {
    pub spec: Option<String>,
    pub spec_source: Option<String>,
    pub rate: Option<f64>,
    pub rate_source: Option<String>,
    /// The date the rate was quoted or read, `YYYY-MM-DD`. Entered by the user, never defaulted.
    #[serde(default)]
    pub rate_date: Option<String>,
    pub waste_pct: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MatFactor {
    pub material: String,
    pub unit: String,
    /// Quantity of the material used per ONE unit of the item (per m3, per m2, per nr ...).
    pub per_unit: f64,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MatData {
    pub spec: Option<String>,
    pub spec_source: Option<String>,
    pub rate: Option<f64>,
    pub rate_source: Option<String>,
    /// The date the rate was quoted or read, `YYYY-MM-DD`. Entered by the user, never defaulted.
    #[serde(default)]
    pub rate_date: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Markup {
    pub name: String,
    pub pct: f64,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BoqLib {
    #[serde(default)]
    pub project: Project,
    #[serde(default)]
    pub items: BTreeMap<String, ItemData>,
    #[serde(default)]
    pub materials: BTreeMap<String, Vec<MatFactor>>,
    #[serde(default)]
    pub material_data: BTreeMap<String, MatData>,
    #[serde(default)]
    pub markups: Vec<Markup>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BoqRow {
    pub no: String,
    pub section: String,
    pub code: String,
    pub description: String,
    pub spec: Option<String>,
    pub unit: String,
    pub quantity: f64,
    pub waste_pct: Option<f64>,
    pub ordered: f64,
    pub rate: Option<f64>,
    pub rate_date: Option<String>,
    pub amount: Option<f64>,
    pub elements: usize,
    pub reference_only: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SectionTotal {
    pub section: String,
    pub amount: f64,
    pub priced: usize,
    pub items: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct MaterialRow {
    pub material: String,
    pub unit: String,
    pub quantity: f64,
    pub spec: Option<String>,
    pub rate: Option<f64>,
    pub rate_date: Option<String>,
    pub cost: Option<f64>,
    pub used_in: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpecRow {
    pub code: String,
    pub description: String,
    pub spec: Option<String>,
    pub source: Option<String>,
    pub unit: String,
    pub quantity: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MarkupLine {
    pub name: String,
    pub pct: f64,
    pub amount: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Missing {
    pub no_spec: Vec<String>,
    pub no_rate: Vec<String>,
    pub no_material_factors: Vec<String>,
    /// Figures that were entered without a source.
    pub no_source: Vec<String>,
    /// Rates (items and materials) entered without the date they were quoted.
    pub no_date: Vec<String>,
}

/// The oldest and newest rate date among the priced items (`YYYY-MM-DD` sorts as text).
#[derive(Debug, Clone, Serialize)]
pub struct RateDates {
    pub oldest: String,
    pub newest: String,
    /// Priced items whose rate carries a date.
    pub dated: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct BoqReport {
    pub project: Project,
    pub rows: Vec<BoqRow>,
    pub sections: Vec<SectionTotal>,
    pub materials: Vec<MaterialRow>,
    pub specs: Vec<SpecRow>,
    pub subtotal: f64,
    pub markups: Vec<MarkupLine>,
    pub total: f64,
    pub priced: usize,
    pub unpriced: usize,
    /// True only when every non-reference item has a rate.
    pub complete: bool,
    pub material_cost: Option<f64>,
    /// `None` when no priced item has a dated rate.
    pub rate_dates: Option<RateDates>,
    pub missing: Missing,
    pub warnings: Vec<String>,
}

const REFERENCE: [&str; 3] = ["SPACE.GROSS.AREA", "WALL.OPENINGS.AREA", "WINDOW.AREA"];
/// Items that measure the same work in different ways: pricing two of one group double counts.
const ALTERNATIVES: [&[&str]; 3] = [&["WALL.EXT.AREA", "WALL.EXT.VOL"], &["WALL.INT.AREA", "WALL.INT.VOL"], &["FOOTING.AREA", "FOOTING.COUNT"]];

fn section_of(code: &str) -> (u8, &'static str) {
    match code.split('.').next().unwrap_or("") {
        "SPACE" => (0, "Reference quantities (not priced)"),
        "FOOTING" => (1, "Substructure"),
        "COLUMN" | "BEAM" | "SLAB" => (2, "Reinforced concrete frame"),
        "WALL" => (3, "Walls"),
        "DOOR" | "WINDOW" => (4, "Doors and windows"),
        "FLOOR" | "CEILING" | "SKIRTING" => (5, "Finishes"),
        "INS" => (6, "Thermal insulation and waterproofing"),
        "MEP" => (7, "MEP points"),
        _ => (9, "Other"),
    }
}

/// The section heading in the language of the document.
fn section_label(name: &str, arabic: bool) -> String {
    if !arabic {
        return name.to_string();
    }
    match name {
        "Reference quantities (not priced)" => "كميات مرجعية (غير مسعّرة)",
        "Substructure" => "الأساسات",
        "Reinforced concrete frame" => "الهيكل الخرساني",
        "Walls" => "الجدران",
        "Doors and windows" => "الأبواب والنوافذ",
        "Finishes" => "التشطيبات",
        "Thermal insulation and waterproofing" => "العزل الحراري والمائي",
        "MEP points" => "نقاط الكهروميكانيكا",
        "Other" => "أخرى",
        other => other,
    }
    .to_string()
}

/// Exact code first, then the longest `prefix*` key that matches.
fn find<'a, T>(map: &'a BTreeMap<String, T>, code: &str) -> Option<&'a T> {
    if let Some(v) = map.get(code) {
        return Some(v);
    }
    map.iter().filter_map(|(k, v)| k.strip_suffix('*').filter(|p| code.starts_with(p)).map(|p| (p.len(), v))).max_by_key(|(l, _)| *l).map(|(_, v)| v)
}

fn round(v: f64, d: i32) -> f64 {
    let m = 10f64.powi(d);
    (v * m).round() / m
}

/// `YYYY-MM-DD` with a real calendar day (leap years included).
pub(crate) fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' || !b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit()) {
        return false;
    }
    let (Ok(y), Ok(m), Ok(d)) = (s[0..4].parse::<u32>(), s[5..7].parse::<u32>(), s[8..10].parse::<u32>()) else { return false };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    y >= 1900 && (1..=days).contains(&d)
}

/// Rejects, by name, a rate date that is not a real `YYYY-MM-DD` day or that sits on an entry with no rate.
pub fn validate(lib: &BoqLib) -> Result<(), String> {
    let mut errors = Vec::new();
    let entries = lib.items.iter().map(|(k, d)| (format!("item '{k}'"), d.rate, &d.rate_date)).chain(lib.material_data.iter().map(|(k, d)| (format!("material '{k}'"), d.rate, &d.rate_date)));
    for (name, rate, date) in entries {
        let Some(date) = date.as_deref() else { continue };
        if rate.is_none() {
            errors.push(format!("{name} has a rate date ({date}) but no rate"));
        } else if !valid_date(date) {
            errors.push(format!("{name}: rate date '{date}' is not a real date in the form YYYY-MM-DD"));
        }
    }
    if errors.is_empty() { Ok(()) } else { Err(format!("BOQ inputs: {}", errors.join("; "))) }
}

fn blank(s: &Option<String>) -> bool {
    s.as_deref().is_none_or(|s| s.trim().is_empty())
}

pub fn compute(items: &[QtoItem], lib: &BoqLib) -> BoqReport {
    let mut miss = Missing::default();
    let mut warnings = Vec::new();
    let mut sorted: Vec<&QtoItem> = items.iter().filter(|i| i.quantity > 0.0).collect();
    sorted.sort_by(|a, b| (section_of(&a.code).0, &a.code).cmp(&(section_of(&b.code).0, &b.code)));

    let mut rows: Vec<BoqRow> = Vec::new();
    let mut counters: BTreeMap<u8, usize> = BTreeMap::new();
    for i in &sorted {
        let (sn, sname) = section_of(&i.code);
        let n = counters.entry(sn).or_default();
        *n += 1;
        let data = find(&lib.items, &i.code);
        let reference = REFERENCE.contains(&i.code.as_str());
        let waste = i.waste_pct.or_else(|| data.and_then(|d| d.waste_pct));
        let spec = data.and_then(|d| d.spec.clone()).filter(|s| !s.trim().is_empty());
        let rate = data.and_then(|d| d.rate);
        let rate_date = data.filter(|d| d.rate.is_some()).and_then(|d| d.rate_date.clone()).filter(|d| !d.trim().is_empty());
        if !reference {
            if spec.is_none() {
                miss.no_spec.push(i.code.clone());
            }
            if rate.is_none() {
                miss.no_rate.push(i.code.clone());
            }
            if let Some(d) = data {
                if d.rate.is_some() && blank(&d.rate_source) {
                    miss.no_source.push(format!("rate of {}", i.code));
                }
                if d.rate.is_some() && rate_date.is_none() {
                    miss.no_date.push(format!("rate of {}", i.code));
                }
                if d.spec.is_some() && blank(&d.spec_source) {
                    miss.no_source.push(format!("specification of {}", i.code));
                }
            }
        }
        rows.push(BoqRow {
            no: format!("{:02}.{:02}", sn, n),
            section: sname.to_string(),
            code: i.code.clone(),
            description: i.description.clone(),
            spec,
            unit: i.unit.label().to_string(),
            quantity: round(i.quantity, 3),
            waste_pct: waste,
            ordered: round(i.quantity * (1.0 + waste.unwrap_or(0.0) / 100.0), 3),
            rate: if reference { None } else { rate },
            rate_date: if reference { None } else { rate_date },
            amount: if reference { None } else { rate.map(|r| round(r * round(i.quantity, 3), 2)) },
            elements: i.sources.len(),
            reference_only: reference,
        });
    }
    for group in ALTERNATIVES {
        let priced: Vec<&str> = rows.iter().filter(|r| group.contains(&r.code.as_str()) && r.amount.is_some()).map(|r| r.code.as_str()).collect();
        if priced.len() > 1 {
            warnings.push(format!("{} are priced together but measure the same work two ways: the total counts it twice", priced.join(" and ")));
        }
    }

    // Layer items of a wall assembly measure the same walls as the plain wall area and volume items.
    let layers_priced = rows.iter().any(|r| r.code.starts_with("WALL.LAYER.") && r.amount.is_some());
    let plain: Vec<&str> = rows.iter().filter(|r| ["WALL.EXT.AREA", "WALL.EXT.VOL", "WALL.INT.AREA", "WALL.INT.VOL"].contains(&r.code.as_str()) && r.amount.is_some()).map(|r| r.code.as_str()).collect();
    if layers_priced && !plain.is_empty() {
        warnings.push(format!("the wall layer items (WALL.LAYER.*) and {} are priced together but measure the same walls: the total counts them twice", plain.join(" and ")));
    }

    let mut sections: Vec<SectionTotal> = Vec::new();
    for r in rows.iter().filter(|r| !r.reference_only) {
        match sections.iter_mut().find(|s| s.section == r.section) {
            Some(s) => {
                s.items += 1;
                if let Some(a) = r.amount {
                    s.amount = round(s.amount + a, 2);
                    s.priced += 1;
                }
            }
            None => sections.push(SectionTotal { section: r.section.clone(), amount: r.amount.unwrap_or(0.0), priced: usize::from(r.amount.is_some()), items: 1 }),
        }
    }
    let priced = rows.iter().filter(|r| r.amount.is_some()).count();
    let unpriced = rows.iter().filter(|r| !r.reference_only && r.amount.is_none()).count();
    let subtotal = round(rows.iter().filter_map(|r| r.amount).sum(), 2);
    let mut running = subtotal;
    let mut markups = Vec::new();
    for m in &lib.markups {
        if blank(&m.source) {
            miss.no_source.push(format!("mark-up '{}'", m.name));
        }
        let amount = round(running * m.pct / 100.0, 2);
        running = round(running + amount, 2);
        markups.push(MarkupLine { name: m.name.clone(), pct: m.pct, amount });
    }
    if unpriced > 0 {
        warnings.push(format!("{unpriced} items have no rate: the totals cover only the {priced} priced items and are PARTIAL"));
    }

    // Materials: ordered quantity of each item x its consumption factors.
    let mut mats: BTreeMap<(String, String), (f64, Vec<String>)> = BTreeMap::new();
    for r in rows.iter().filter(|r| !r.reference_only) {
        match find(&lib.materials, &r.code) {
            Some(fs) if !fs.is_empty() => {
                for f in fs {
                    if blank(&f.source) {
                        miss.no_source.push(format!("consumption of {} in {}", f.material, r.code));
                    }
                    let e = mats.entry((f.material.clone(), f.unit.clone())).or_default();
                    e.0 += r.ordered * f.per_unit;
                    e.1.push(r.code.clone());
                }
            }
            _ => miss.no_material_factors.push(r.code.clone()),
        }
    }
    let mut undated_materials: Vec<String> = Vec::new();
    let materials: Vec<MaterialRow> = mats
        .into_iter()
        .map(|((material, unit), (q, used_in))| {
            let d = find(&lib.material_data, &material);
            let rate = d.and_then(|d| d.rate);
            let rate_date = d.filter(|d| d.rate.is_some()).and_then(|d| d.rate_date.clone()).filter(|d| !d.trim().is_empty());
            if rate.is_some() && rate_date.is_none() {
                undated_materials.push(format!("rate of material {material}"));
            }
            MaterialRow { spec: d.and_then(|d| d.spec.clone()), rate, rate_date, cost: rate.map(|r| round(r * q, 2)), quantity: round(q, 3), material, unit, used_in }
        })
        .collect();
    let material_cost = if !materials.is_empty() && materials.iter().all(|m| m.cost.is_some()) { Some(round(materials.iter().filter_map(|m| m.cost).sum(), 2)) } else { None };

    miss.no_date.extend(undated_materials);
    let dates: Vec<&str> = rows.iter().filter(|r| r.amount.is_some()).filter_map(|r| r.rate_date.as_deref()).collect();
    let rate_dates = match (dates.iter().min(), dates.iter().max()) {
        (Some(o), Some(n)) => Some(RateDates { oldest: (*o).to_string(), newest: (*n).to_string(), dated: dates.len() }),
        _ => None,
    };
    if !miss.no_date.is_empty() {
        warnings.push(format!("{} rates carry no date, so their age cannot be judged: {}", miss.no_date.len(), miss.no_date.join(", ")));
    }

    let specs = rows
        .iter()
        .filter(|r| !r.reference_only)
        .map(|r| SpecRow {
            code: r.code.clone(),
            description: r.description.clone(),
            spec: r.spec.clone(),
            source: find(&lib.items, &r.code).and_then(|d| d.spec_source.clone()),
            unit: r.unit.clone(),
            quantity: r.quantity,
        })
        .collect();

    BoqReport { project: lib.project.clone(), rows, sections, materials, specs, subtotal, markups, total: running, priced, unpriced, complete: unpriced == 0 && priced > 0, material_cost, rate_dates, missing: miss, warnings }
}

// ------------------------------------------------------------------ exports

fn cell(s: &str) -> String {
    if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

fn opt(v: Option<f64>, d: usize) -> String {
    v.map_or(String::new(), |x| format!("{x:.d$}"))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

impl BoqReport {
    fn currency(&self) -> &str {
        self.project.currency.as_deref().unwrap_or("")
    }

    fn state(&self) -> &'static str {
        if self.complete { "complete" } else { "PARTIAL" }
    }

    /// The BOQ lines as CSV (UTF-8 with a byte-order mark so Excel reads Arabic).
    pub fn boq_csv(&self) -> String {
        let mut o = String::from("\u{feff}no,section,code,description,specification,unit,quantity,waste_pct,quantity_to_order,rate,rate_date,amount,currency,status\n");
        for r in &self.rows {
            let status = if r.reference_only { "reference" } else if r.amount.is_some() { "priced" } else { "no rate" };
            o.push_str(&format!(
                "{},{},{},{},{},{},{:.3},{},{:.3},{},{},{},{},{}\n",
                r.no,
                cell(&r.section),
                cell(&r.code),
                cell(&r.description),
                cell(r.spec.as_deref().unwrap_or("")),
                r.unit,
                r.quantity,
                opt(r.waste_pct, 2),
                r.ordered,
                opt(r.rate, 2),
                cell(r.rate_date.as_deref().unwrap_or("")),
                opt(r.amount, 2),
                cell(self.currency()),
                status
            ));
        }
        o.push_str(&format!(",,,Subtotal ({} of {} items priced),,,,,,,,{:.2},{},{}\n", self.priced, self.priced + self.unpriced, self.subtotal, cell(self.currency()), self.state()));
        for m in &self.markups {
            o.push_str(&format!(",,,{} {}%,,,,,,,,{:.2},{},\n", cell(&m.name), m.pct, m.amount, cell(self.currency())));
        }
        o.push_str(&format!(",,,Total,,,,,,,,{:.2},{},{}\n", self.total, cell(self.currency()), self.state()));
        o
    }

    pub fn materials_csv(&self) -> String {
        let mut o = String::from("\u{feff}material,unit,quantity,specification,rate,rate_date,cost,used_in\n");
        for m in &self.materials {
            o.push_str(&format!("{},{},{:.3},{},{},{},{},{}\n", cell(&m.material), cell(&m.unit), m.quantity, cell(m.spec.as_deref().unwrap_or("")), opt(m.rate, 2), cell(m.rate_date.as_deref().unwrap_or("")), opt(m.cost, 2), cell(&m.used_in.join(";"))));
        }
        o
    }

    pub fn specs_csv(&self) -> String {
        let mut o = String::from("\u{feff}code,description,specification,source,unit,quantity\n");
        for s in &self.specs {
            o.push_str(&format!("{},{},{},{},{},{:.3}\n", cell(&s.code), cell(&s.description), cell(s.spec.as_deref().unwrap_or("")), cell(s.source.as_deref().unwrap_or("")), s.unit, s.quantity));
        }
        o
    }

    /// One self-contained printable document: project data, BOQ, material list, specification schedule, totals.
    /// `arabic` chooses the language of the headings and the text direction.
    pub fn html(&self, arabic: bool) -> String {
        let t = |ar: &'static str, en: &'static str| if arabic { ar } else { en };
        let p = &self.project;
        let cur = esc(self.currency());
        let mut h = format!(
            "<!doctype html><html lang=\"{}\" dir=\"{}\"><head><meta charset=\"utf-8\"><title>{}</title><style>\
             body{{font-family:'Segoe UI',Tahoma,sans-serif;margin:28px;color:#111}} h1{{font-size:22px;margin:0 0 4px}} h2{{font-size:16px;margin:26px 0 8px;border-bottom:2px solid #333;padding-bottom:3px}}\
             table{{border-collapse:collapse;width:100%;font-size:12px}} th,td{{border:1px solid #999;padding:4px 6px;text-align:start;vertical-align:top}} th{{background:#eee}}\
             td.n{{text-align:end;font-variant-numeric:tabular-nums}} tr.sec td{{background:#f4f4f4;font-weight:700}} tr.tot td{{font-weight:700;background:#fafafa}} .meta td{{border:0;padding:2px 8px 2px 0}}\
             .warn{{color:#a33}} .note{{font-size:11px;color:#444;margin-top:18px}} @page{{size:A4 portrait;margin:12mm;@bottom-center{{content:counter(page) \" / \" counter(pages);direction:ltr;font-size:9pt;color:#555}}}} thead{{display:table-header-group}} @media print{{body{{margin:0}} h2{{break-after:avoid}} tr{{break-inside:avoid}}}}</style></head><body>",
            if arabic { "ar" } else { "en" },
            if arabic { "rtl" } else { "ltr" },
            esc(p.name.as_deref().unwrap_or("BOQ"))
        );
        h.push_str(&format!("<h1>{} — {}</h1>", t("جدول الكميات (BOQ)", "Bill of Quantities (BOQ)"), esc(p.name.as_deref().unwrap_or(""))));
        h.push_str("<table class=\"meta\">");
        for (label, v) in [(t("العميل", "Client"), &p.client), (t("الموقع", "Location"), &p.location), (t("التاريخ", "Date"), &p.date), (t("أعدّه", "Prepared by"), &p.prepared_by), (t("العملة", "Currency"), &p.currency)] {
            if let Some(v) = v.as_deref().filter(|v| !v.is_empty()) {
                h.push_str(&format!("<tr><td><b>{label}</b></td><td>{}</td></tr>", esc(v)));
            }
        }
        h.push_str("</table>");
        if !self.complete {
            h.push_str(&format!("<p class=\"warn\"><b>{}</b></p>", t("تنبيه: الأسعار ناقصة لبعض البنود، والإجماليات جزئية.", "Warning: some items have no rate, so the totals are PARTIAL.")));
        }
        h.push_str(&format!(
            "<h2>1. {}</h2><table><thead><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr></thead><tbody>",
            t("جدول الكميات والأسعار", "Bill of quantities"),
            t("رقم", "No."),
            t("الوصف", "Description"),
            t("المواصفة", "Specification"),
            t("الوحدة", "Unit"),
            t("الكمية", "Quantity"),
            t("السعر", "Rate"),
            t("المبلغ", "Amount")
        ));
        let mut last = "";
        for r in &self.rows {
            if r.section != last {
                h.push_str(&format!("<tr class=\"sec\"><td colspan=\"7\">{}</td></tr>", esc(&section_label(&r.section, arabic))));
                last = &r.section;
            }
            h.push_str(&format!(
                "<tr><td>{}</td><td>{}<br><small>{}</small></td><td>{}</td><td>{}</td><td class=\"n\">{:.2}</td><td class=\"n\">{}{}</td><td class=\"n\">{}</td></tr>",
                r.no,
                esc(&r.description),
                esc(&r.code),
                esc(r.spec.as_deref().unwrap_or("—")),
                r.unit,
                r.quantity,
                opt(r.rate, 2),
                r.rate_date.as_deref().map_or(String::new(), |d| format!("<br><small>{}</small>", esc(d))),
                if r.reference_only { t("مرجعي", "ref.").to_string() } else { opt(r.amount, 2) }
            ));
        }
        h.push_str(&format!("<tr class=\"tot\"><td colspan=\"6\">{} {cur}</td><td class=\"n\">{:.2}</td></tr>", t("المجموع الفرعي", "Subtotal"), self.subtotal));
        for m in &self.markups {
            h.push_str(&format!("<tr><td colspan=\"6\">{} ({}%)</td><td class=\"n\">{:.2}</td></tr>", esc(&m.name), m.pct, m.amount));
        }
        h.push_str(&format!("<tr class=\"tot\"><td colspan=\"6\">{} {cur}</td><td class=\"n\">{:.2}</td></tr></tbody></table>", t("الإجمالي", "Total"), self.total));
        h.push_str(&format!(
            "<h2>2. {}</h2><table><thead><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr></thead><tbody>",
            t("قائمة المواد", "Material list"),
            t("المادة", "Material"),
            t("الوحدة", "Unit"),
            t("الكمية", "Quantity"),
            t("المواصفة", "Specification"),
            t("تُستعمل في", "Used in")
        ));
        for m in &self.materials {
            h.push_str(&format!("<tr><td>{}</td><td>{}</td><td class=\"n\">{:.2}</td><td>{}</td><td><small>{}</small></td></tr>", esc(&m.material), esc(&m.unit), m.quantity, esc(m.spec.as_deref().unwrap_or("—")), esc(&m.used_in.join(", "))));
        }
        h.push_str(&format!(
            "</tbody></table><h2>3. {}</h2><table><thead><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr></thead><tbody>",
            t("جدول المواصفات والكميات", "Specification and quantity schedule"),
            t("البند", "Item"),
            t("المواصفة", "Specification"),
            t("المصدر", "Source"),
            t("الوحدة", "Unit"),
            t("الكمية", "Quantity")
        ));
        for s in &self.specs {
            h.push_str(&format!("<tr><td>{}<br><small>{}</small></td><td>{}</td><td>{}</td><td>{}</td><td class=\"n\">{:.2}</td></tr>", esc(&s.description), esc(&s.code), esc(s.spec.as_deref().unwrap_or("—")), esc(s.source.as_deref().unwrap_or("—")), s.unit, s.quantity));
        }
        h.push_str("</tbody></table>");
        for w in &self.warnings {
            h.push_str(&format!("<p class=\"warn\">{}</p>", esc(w)));
        }
        h.push_str(&format!(
            "<p class=\"note\">{}</p></body></html>",
            t(
                "أولي: الكميات مقيسة من النموذج، والمواصفات والأسعار ومعاملات الاستهلاك من مدخلات المستخدم. لا يحسب البرنامج حديد التسليح إلا بمعاملات المستخدم. يراجعه مهندس مرخّص قبل التعاقد.",
                "Preliminary: quantities are measured from the model; specifications, rates and consumption factors are the user's inputs. Reinforcement is counted only through the user's factors. To be checked by a licensed engineer before contracting."
            )
        ));
        h
    }
}
