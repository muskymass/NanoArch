//! Compliance audits on the model and the project: universal accessibility, ventilation of wet areas, and projections (cantilevers).
//!
//! What each audit may call a limit:
//! - **Ventilation** (`ventilation`): clause 5-1-6 of the Ministry's residential guidelines (1446 AH), pdf p.27: the least lightwell
//!   sizes by height and window arrangement, and "mechanical ventilation is allowed for bathrooms and toilets, no lightwell needed".
//!   Kitchens are not named in that sentence, so a kitchen served only by an exhaust is a warning, not a pass.
//! - **Projections** (`cantilever`): clause 5-1-11, pdf p.30: none over a street or pedestrian way narrower than 10 m; on a street of
//!   10 m or more at most 1.25 m; none in the side or rear setback except a decorative cornice up to 25 cm; balcony barrier at least
//!   90 cm. Item 2 of that clause is worded ambiguously in the text extraction; the stricter reading (1.25 m) is applied.
//! - **Accessibility** (`accessibility`): the code files carry NO figure (SBC 201 chapter 11 has no numbers in the Arabic edition
//!   and SBC 1101 is not among them). Every limit is an input with its source; a missing one skips its check.
//!
//! The result is a preliminary check; it does not replace the approving authority.

use crate::building;
use crate::compliance::Lightwell;
use crate::model::Model;
use crate::project::NanoArchProject;
use crate::typed::{Door, Space, Typed, Wall, Window};
use serde::Serialize;

pub const MIN_FILE: &str = "ashtratat a'nsha almbany alsknyt almnshwrt ly mwq alwzart.pdf";
pub const MIN_ED: &str = "Ministry residential guidelines 1446 AH (2024)";

#[derive(Debug, Clone, Serialize)]
pub struct AuditRow {
    pub rule: String,
    pub clause: String,
    pub source: String,
    pub element: Option<String>,
    /// `ok`, `violation`, `warn`, `info` or `skipped`.
    pub status: &'static str,
    pub en: String,
    pub ar: String,
    pub value: Option<f64>,
    pub limit: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AuditReport {
    pub rows: Vec<AuditRow>,
}

impl AuditReport {
    pub fn count(&self, status: &str) -> usize {
        self.rows.iter().filter(|r| r.status == status).count()
    }
}

#[allow(clippy::too_many_arguments)]
fn row(rule: &str, clause: &str, source: String, element: Option<String>, status: &'static str, en: String, ar: String, value: Option<f64>, limit: Option<f64>) -> AuditRow {
    AuditRow { rule: rule.into(), clause: clause.into(), source, element, status, en, ar, value, limit }
}

fn min_source(clause: &str, page: u32) -> String {
    format!("{MIN_ED} {clause}, {MIN_FILE} p.{page}")
}

fn spaces(model: &Model) -> Vec<(crate::model::Id, Space, Option<crate::model::Id>)> {
    model.elements.values().filter(|e| e.kind == Space::KIND).filter_map(|e| Some((e.id, e.typed::<Space>().ok()?, e.level_id))).collect()
}

/// `(key, function, floor index)` of every room, for the page's pickers.
pub fn room_keys(model: &Model) -> Vec<serde_json::Value> {
    let lv = building::levels(model);
    spaces(model).into_iter().map(|(_, s, l)| serde_json::json!({ "key": s.requirement_key, "func": s.func, "level": l.and_then(|l| lv.iter().position(|x| x.1 == l)).map(|i| i + 1).unwrap_or(1) })).collect()
}

// ------------------------------------------------------------------ accessibility

pub fn accessibility(model: &Model, p: &NanoArchProject) -> AuditReport {
    let a = &p.meta.compliance.access;
    let mut rep = AuditReport::default();
    let src = a.source.clone().map_or_else(|| "limits entered by the user (no source entered)".to_string(), |s| format!("limits entered by the user: {s}"));
    let ground = building::levels(model).first().map(|(_, id)| *id);
    // Doors of the ground floor: the entrance and the doors of the accessible rooms must meet the clear width; the others are listed.
    match a.door_min_clear_mm {
        None => rep.rows.push(row("access.door_width", "user input", src.clone(), None, "skipped", "accessible door width: the smallest clear width is missing, enter it with its source".into(), "عرض الأبواب لذوي الإعاقة: لم يُدخل الحد الأدنى للعرض الصافي، أدخله مع مصدره".into(), None, None)),
        Some(min) => {
            let ded = a.door_deduction_mm.unwrap_or(0.0);
            for e in model.elements.values().filter(|e| e.kind == Door::KIND && (ground.is_none() || e.level_id == ground)) {
                let Ok(d) = e.typed::<Door>() else { continue };
                let entrance = d.is_egress || d.to == "out" || d.from == "out";
                let accessible = a.accessible_rooms.iter().any(|r| *r == d.from || *r == d.to);
                let clear = d.width as f64 - ded;
                let required = entrance || accessible;
                let ok = clear + 1e-9 >= min;
                let (status, en, ar) = match (ok, required) {
                    (true, _) => ("ok", format!("door {} mm clear (limit {} mm)", clear, min), format!("باب بعرض صافٍ {} مم (الحد {} مم)", clear, min)),
                    (false, true) => ("violation", format!("{} door is {} mm clear, under the {} mm limit", if entrance { "entrance" } else { "accessible-room" }, clear, min), format!("عرض الباب الصافي {} مم أقل من الحد {} مم", clear, min)),
                    (false, false) => ("info", format!("door {} mm clear is under {} mm but it is not on the accessible route (entrance or an accessible room)", clear, min), format!("باب بعرض {} مم أقل من {} مم لكنه خارج المسار المتاح", clear, min)),
                };
                rep.rows.push(row("access.door_width", "user input", src.clone(), Some(e.id.to_string()), status, en, ar, Some(clear), Some(min)));
            }
            if a.door_deduction_mm.is_none() {
                rep.rows.push(row("access.door_width.note", "user input", src.clone(), None, "info", "no clear-width deduction entered: the nominal door width was compared".into(), "لم يُدخل خصم العرض الصافي: قورن العرض الاسمي".into(), None, None));
            }
        }
    }
    match (a.entrance_ramp_rise_mm, a.entrance_ramp_run_mm, a.ramp_max_ratio_n) {
        (Some(rise), Some(run), Some(n)) => {
            let ratio = run / rise;
            let ok = ratio + 1e-9 >= n;
            rep.rows.push(row(
                "access.ramp",
                "user input",
                src.clone(),
                None,
                if ok { "ok" } else { "violation" },
                format!("entrance ramp 1:{:.1} (limit 1:{n})", ratio),
                format!("المنحدر 1:{:.1} (الحد 1:{n})", ratio),
                Some(rise / run * 100.0),
                Some(100.0 / n),
            ));
        }
        (None, None, _) => rep.rows.push(row("access.ramp", "user input", src.clone(), None, "skipped", "entrance ramp: no rise and run entered (nothing to check)".into(), "المنحدر: لم يُدخل ارتفاع وطول".into(), None, None)),
        _ => rep.rows.push(row("access.ramp", "user input", src.clone(), None, "skipped", "entrance ramp: enter the rise, the run and the steepest ratio 1:N".into(), "المنحدر: أدخل الارتفاع والطول والنسبة القصوى 1:N".into(), None, None)),
    }
    match a.turning_circle_mm {
        None if !a.accessible_rooms.is_empty() => rep.rows.push(row("access.turning_circle", "user input", src.clone(), None, "skipped", "accessible bathrooms: the turning-circle diameter is missing, enter it with its source".into(), "دورات المياه المتاحة: لم يُدخل قطر دائرة الدوران".into(), None, None)),
        None => {}
        Some(d) => {
            for (id, s, _) in spaces(model).into_iter().filter(|(_, s, _)| a.accessible_rooms.contains(&s.requirement_key)) {
                let inner = s.w.min(s.h) as f64;
                let ok = inner + 1e-9 >= d;
                rep.rows.push(row(
                    "access.turning_circle",
                    "user input",
                    src.clone(),
                    Some(id.to_string()),
                    if ok { "ok" } else { "violation" },
                    format!("room '{}': a {} mm circle {} the {} mm side (fixtures are not placed)", s.requirement_key, d, if ok { "fits in" } else { "does not fit in" }, inner),
                    format!("الغرفة '{}': دائرة {} مم {} ضلع {} مم (دون تركيبات)", s.requirement_key, d, if ok { "تتسع في" } else { "لا تتسع في" }, inner),
                    Some(inner),
                    Some(d),
                ));
            }
        }
    }
    for k in &a.accessible_rooms {
        if !spaces(model).iter().any(|(_, s, _)| s.requirement_key == *k) {
            rep.rows.push(row("access.room", "user input", src.clone(), None, "warn", format!("accessible room '{k}' is not in the plan"), format!("الغرفة '{k}' غير موجودة في المخطط"), None, None));
        }
    }
    rep
}

// ------------------------------------------------------------------ ventilation

/// Least lightwell `(width, length)` in metres for `floors` floors (clause 5-1-6 (1), pdf p.27); `None` above 14 floors.
pub fn lightwell_min(floors: u32, opposite_or_all: bool) -> Option<(f64, f64)> {
    let t: [((f64, f64), (f64, f64)); 4] = [((0.90, 1.8), (1.5, 1.8)), ((1.5, 1.8), (1.8, 1.8)), ((1.8, 2.4), (2.1, 2.4)), ((2.1, 3.0), (2.4, 3.0))];
    match floors {
        0 => None,
        1..=4 => {
            let r = t[floors as usize - 1];
            Some(if opposite_or_all { r.1 } else { r.0 })
        }
        5..=14 => {
            let base = t[3];
            let b = if opposite_or_all { base.1 } else { base.0 };
            let extra = (floors - 4) as f64;
            Some((((b.0 + 0.30 * extra) * 100.0).round() / 100.0, ((b.1 + 0.60 * extra) * 100.0).round() / 100.0))
        }
        _ => None,
    }
}

fn lightwell_ok(l: &Lightwell) -> Result<(), String> {
    let (w, len) = lightwell_min(l.floors, l.opposite_or_all).ok_or_else(|| format!("lightwell '{}' rises {} floors: the table gives sizes up to 14 floors", l.name, l.floors))?;
    // The width is the smaller side, the length the larger one.
    let (lw, ll) = (l.width_m.min(l.length_m), l.width_m.max(l.length_m));
    if lw + 1e-9 >= w && ll + 1e-9 >= len { Ok(()) } else { Err(format!("lightwell '{}' is {lw:.2} x {ll:.2} m; {} floors with windows on {} need at least {w:.2} x {len:.2} m", l.name, l.floors, if l.opposite_or_all { "opposite or all sides" } else { "one side or two adjacent sides" })) }
}

pub fn ventilation(model: &Model, p: &NanoArchProject) -> AuditReport {
    let v = &p.meta.compliance.vent;
    let mut rep = AuditReport::default();
    let cl = "5-1-6 (1), (2)";
    let src = min_source(cl, 27);
    for l in &v.lightwells {
        match lightwell_ok(l) {
            Ok(()) => rep.rows.push(row("vent.lightwell", cl, src.clone(), None, "ok", format!("lightwell '{}' meets the least size for {} floor(s)", l.name, l.floors), format!("المنور '{}' يحقق الحد الأدنى لـ {} دور", l.name, l.floors), Some(l.width_m.min(l.length_m)), lightwell_min(l.floors, l.opposite_or_all).map(|m| m.0))),
            Err(e) => rep.rows.push(row("vent.lightwell", cl, src.clone(), None, "violation", e.clone(), format!("المنور '{}' أصغر من الحد الأدنى (انظر الجدول 5-1-6)", l.name), Some(l.width_m.min(l.length_m)), lightwell_min(l.floors, l.opposite_or_all).map(|m| m.0))),
        }
    }
    for (id, s, level) in spaces(model).into_iter().filter(|(_, s, _)| ["kitchen", "bathroom", "toilet"].contains(&s.func.as_str())) {
        let key = &s.requirement_key;
        let wet = s.func != "kitchen";
        // An external window: a window of this room on an exterior wall.
        let has_window = model.elements.values().any(|e| e.kind == Window::KIND && e.level_id == level && e.typed::<Window>().is_ok_and(|w| w.room == *key) && e.host_id.and_then(|h| model.get(&h)).and_then(|h| h.typed::<Wall>().ok()).is_some_and(|w| w.is_exterior));
        let wells: Vec<&Lightwell> = v.lightwells.iter().filter(|l| l.rooms.contains(key)).collect();
        let eid = Some(id.to_string());
        let (status, en, ar): (&'static str, String, String) = if has_window {
            ("ok", format!("{} '{key}' has an external window", s.func), format!("'{key}' لها نافذة خارجية"))
        } else if let Some(l) = wells.iter().find(|l| lightwell_ok(l).is_ok()) {
            ("ok", format!("'{key}' opens onto the compliant lightwell '{}'", l.name), format!("'{key}' تطل على المنور '{}' المطابق", l.name))
        } else if !wells.is_empty() {
            ("violation", format!("'{key}' opens only onto lightwell(s) below the least size: {}", wells.iter().filter_map(|l| lightwell_ok(l).err()).collect::<Vec<_>>().join("; ")), format!("'{key}' تطل على منور أصغر من الحد الأدنى"))
        } else if v.exhaust_rooms.contains(key) && wet {
            ("ok", format!("'{key}' is served by a mechanical exhaust (allowed for bathrooms and toilets, no lightwell needed)"), format!("'{key}' يخدمها شفاط ميكانيكي (مسموح لدورات المياه دون منور)"))
        } else if v.exhaust_rooms.contains(key) {
            ("warn", format!("kitchen '{key}' has only an exhaust: the clause allows mechanical ventilation for bathrooms and toilets; confirm it for kitchens"), format!("المطبخ '{key}' بشفاط فقط: البند يسمح بالتهوية الميكانيكية لدورات المياه، تأكد من قبولها للمطابخ"))
        } else {
            ("violation", format!("{} '{key}' has no external window, lightwell or exhaust duct", s.func), format!("'{key}' بلا نافذة خارجية أو منور أو شفاط"))
        };
        rep.rows.push(row("vent.room", cl, src.clone(), eid, status, en, ar, None, None));
    }
    for k in v.exhaust_rooms.iter().chain(v.lightwells.iter().flat_map(|l| l.rooms.iter())) {
        if !spaces(model).iter().any(|(_, s, _)| s.requirement_key == *k) {
            rep.rows.push(row("vent.room.unknown", cl, src.clone(), None, "warn", format!("'{k}' is named in the ventilation inputs but is not in the plan"), format!("'{k}' مذكورة في مدخلات التهوية وغير موجودة في المخطط"), None, None));
        }
    }
    rep
}

// ------------------------------------------------------------------ projections

pub fn cantilever(model: &Model, p: &NanoArchProject) -> AuditReport {
    let c = &p.meta.compliance;
    let mut rep = AuditReport::default();
    let front = p.brief.as_ref().map_or("south", |b| b.entry.as_str()).to_string();
    // Street width: entered, or the front street edge of the plot outline.
    let street = c.street_width_m.or_else(|| p.meta.plot_edges.iter().find(|e| e.kind == crate::site::EdgeKind::FrontStreet).and_then(|e| e.street_width_m));
    // Projections implied by the model: an upper level whose rooms reach beyond the ground level's.
    let levels = building::levels(model);
    let extent = |lv: crate::model::Id| -> Option<[i64; 4]> {
        let sp: Vec<Space> = spaces(model).into_iter().filter(|(_, _, l)| *l == Some(lv)).map(|(_, s, _)| s).collect();
        (!sp.is_empty()).then(|| [sp.iter().map(|s| s.x).min().unwrap_or(0), sp.iter().map(|s| s.y).min().unwrap_or(0), sp.iter().map(|s| s.x + s.w).max().unwrap_or(0), sp.iter().map(|s| s.y + s.h).max().unwrap_or(0)])
    };
    struct Item {
        name: String,
        side: String,
        depth: f64,
        kind: String,
        guard: Option<f64>,
        element: Option<String>,
    }
    let mut items: Vec<Item> = Vec::new();
    for pr in &c.projections {
        items.push(Item { name: pr.name.clone(), side: pr.side.clone(), depth: pr.depth_m, kind: pr.kind.clone(), guard: pr.guard_height_m, element: None });
    }
    if let Some(&(_, g)) = levels.first().filter(|_| levels.len() > 1)
        && let Some(base) = extent(g)
    {
        for (idx, lv) in levels.iter().skip(1) {
            let Some(e) = extent(*lv) else { continue };
            for (side, d) in [("west", base[0] - e[0]), ("south", base[1] - e[1]), ("east", e[2] - base[2]), ("north", e[3] - base[3])] {
                if d > 0 {
                    items.push(Item { name: format!("floor {} overhang", idx + 1), side: side.into(), depth: d as f64 / 1000.0, kind: "bay".into(), guard: None, element: Some(lv.to_string()) });
                }
            }
        }
    }
    let cl = "5-1-11";
    let src = min_source(cl, 30);
    if items.is_empty() {
        return rep;
    }
    for Item { name, side, depth, kind, guard, element: el } in items {
        let is_front = side == front;
        let (status, en, ar, limit): (&'static str, String, String, Option<f64>) = if depth <= 1e-9 {
            ("ok", format!("{name}: no projection"), format!("{name}: بلا بروز"), None)
        } else if is_front {
            match street {
                None => ("skipped", format!("{name} ({side}, front): the street width is missing: enter it to judge a projection over the street"), format!("{name}: عرض الشارع غير مُدخل"), None),
                Some(w) if w < 10.0 => ("violation", format!("{name} projects {depth} m over a street {w} m wide: none is allowed over a street or pedestrian way under 10 m (5-1-11 (1))"), format!("{name}: بروز {depth} م على شارع {w} م، ولا يُسمح تحت 10 م"), Some(0.0)),
                Some(_) if depth > 1.25 + 1e-9 => ("violation", format!("{name} projects {depth} m over the street: the most allowed is 1.25 m (5-1-11 (1))"), format!("{name}: بروز {depth} م يتجاوز 1.25 م"), Some(1.25)),
                Some(_) => ("ok", format!("{name} projects {depth} m over the street (limit 1.25 m)"), format!("{name}: بروز {depth} م ضمن 1.25 م"), Some(1.25)),
            }
        } else if depth > 0.25 + 1e-9 {
            ("violation", format!("{name} projects {depth} m into the {side} setback: nothing is allowed there but a decorative cornice up to 25 cm (5-1-11 (3))"), format!("{name}: بروز {depth} م في الارتداد، المسموح كرنيش زخرفي حتى 25 سم فقط"), Some(0.25))
        } else {
            ("warn", format!("{name} projects {depth} m into the {side} setback: allowed only as a decorative cornice up to 25 cm (5-1-11 (3))"), format!("{name}: بروز {depth} م في الارتداد: يُسمح فقط ككرنيش زخرفي حتى 25 سم"), Some(0.25))
        };
        rep.rows.push(row("projection.depth", cl, src.clone(), el.clone(), status, en, ar, Some(depth), limit));
        if kind == "balcony" {
            let (st, en, ar) = match guard {
                None => ("skipped", format!("{name}: the balcony barrier height is missing (least 0.90 m, 5-1-11 (5))"), format!("{name}: ارتفاع حاجز الشرفة غير مُدخل")),
                Some(h) if h + 1e-9 >= 0.90 => ("ok", format!("{name}: barrier {h} m (least 0.90 m)"), format!("{name}: الحاجز {h} م")),
                Some(h) => ("violation", format!("{name}: barrier {h} m is under the 0.90 m minimum (5-1-11 (5))"), format!("{name}: الحاجز {h} م أقل من 0.90 م")),
            };
            rep.rows.push(row("projection.guard", cl, src.clone(), el, st, en, ar, guard, Some(0.90)));
        }
    }
    rep
}
