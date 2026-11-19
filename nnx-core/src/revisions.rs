//! The engineering change log: a baseline snapshot of the model, the elements added, modified or removed since, revision clouds
//! drawn around the changed geometry in the 2D plan, and a revision-history table for the exports.
//!
//! The baseline stores for every element its id, kind and an FNV-1a hash of its properties, level and host: nothing is guessed
//! from geometry, and an element is "modified" exactly when its stored properties differ. The project element itself and the
//! scene-only solids (facades, site layout, furniture: not elements) are not part of it.

use crate::compliance::{Baseline, RevisionEntry};
use crate::drawing::{self, DrawOpts};
use crate::model::{Id, Model};
use crate::project::NanoArchProject;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write;

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// `id -> (kind, hash)` of the model as it is now.
pub fn snapshot(model: &Model) -> BTreeMap<String, (String, u64)> {
    model
        .elements
        .values()
        .filter(|e| e.kind != "Project")
        .map(|e| {
            let text = format!("{}|{}|{:?}|{:?}", e.kind, serde_json::to_string(&e.props).unwrap_or_default(), e.level_id, e.host_id);
            (e.id.to_string(), (e.kind.clone(), fnv(&text)))
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Change {
    pub id: String,
    pub kind: String,
    /// `added`, `modified` or `removed`.
    pub status: &'static str,
}

pub fn changes(model: &Model, base: &Baseline) -> Vec<Change> {
    let now = snapshot(model);
    let mut out = Vec::new();
    for (id, (kind, h)) in &now {
        match base.elements.get(id) {
            None => out.push(Change { id: id.clone(), kind: kind.clone(), status: "added" }),
            Some((_, bh)) if bh != h => out.push(Change { id: id.clone(), kind: kind.clone(), status: "modified" }),
            _ => {}
        }
    }
    for (id, (kind, _)) in &base.elements {
        if !now.contains_key(id) {
            out.push(Change { id: id.clone(), kind: kind.clone(), status: "removed" });
        }
    }
    out
}

pub fn counts(c: &[Change]) -> (usize, usize, usize) {
    let n = |s: &str| c.iter().filter(|x| x.status == s).count();
    (n("added"), n("modified"), n("removed"))
}

/// A new baseline of the model. The previous one, if any, is closed into the history with what had changed against it.
pub fn set_baseline(model: &Model, p: &mut NanoArchProject, name: &str, now_unix: u64) {
    if let Some(old) = p.meta.baseline.take() {
        let (a, m, r) = counts(&changes(model, &old));
        let rev = p.meta.revision_history.last().map_or(1, |h| h.rev + 1);
        p.meta.revision_history.push(RevisionEntry { rev, name: old.name, created_unix: old.created_unix, added: a, modified: m, removed: r });
    }
    p.meta.baseline = Some(Baseline { name: name.to_string(), created_unix: now_unix, elements: snapshot(model) });
}

/// The bounding box of an element in plan, millimetres `[x0, y0, x1, y1]`, taken from the 3D scene's solids of that element.
fn bbox(scene: &crate::model3d::Scene, id: &str) -> Option<[f64; 4]> {
    let mut r: Option<[f64; 4]> = None;
    for b in scene.boxes.iter().filter(|b| b.id == id) {
        let c = [b.min[0] * 1000.0, b.min[1] * 1000.0, b.max[0] * 1000.0, b.max[1] * 1000.0];
        r = Some(r.map_or(c, |p| [p[0].min(c[0]), p[1].min(c[1]), p[2].max(c[2]), p[3].max(c[3])]));
    }
    r
}

/// A scalloped cloud around a rectangle (pixels), clockwise, arcs bulging outwards.
fn cloud_path(x0: f64, y0: f64, x1: f64, y1: f64) -> String {
    let r = 7.0;
    let mut d = format!("M{x0:.1},{y0:.1}");
    let mut side = |ax: f64, ay: f64, bx: f64, by: f64| {
        let len = (bx - ax).hypot(by - ay);
        let n = ((len / (2.0 * r)).round() as usize).max(1);
        for k in 1..=n {
            let t = k as f64 / n as f64;
            let _ = write!(d, " A{:.1},{:.1} 0 0 1 {:.1},{:.1}", len / n as f64 / 1.7, len / n as f64 / 1.7, ax + (bx - ax) * t, ay + (by - ay) * t);
        }
    };
    side(x0, y0, x1, y0);
    side(x1, y0, x1, y1);
    side(x1, y1, x0, y1);
    side(x0, y1, x0, y0);
    d.push_str(" Z");
    d
}

/// Puts revision clouds around the changed elements of `level` into a plan SVG made by `drawing::plan_svg` with the same options.
pub fn add_clouds(svg: &str, model: &Model, level: Option<Id>, opts: &DrawOpts, ch: &[Change]) -> String {
    let Some((minx, maxy, s, m)) = drawing::frame(model, level, opts) else { return svg.to_string() };
    let scene = crate::model3d::build(model);
    let mut g = String::from("<g class=\"rev-clouds\" fill=\"none\" stroke=\"#d32f2f\" stroke-width=\"2\">\n");
    let mut n = 0;
    for c in ch.iter().filter(|c| c.status != "removed") {
        let Some(el) = model.elements.values().find(|e| e.id.to_string() == c.id) else { continue };
        if level.is_some() && el.level_id != level && el.kind != "Level" {
            continue;
        }
        let Some(b) = bbox(&scene, &c.id) else { continue };
        let pad = 250.0;
        let (x0, x1) = (m + (b[0] - pad - minx) * s, m + (b[2] + pad - minx) * s);
        let (y0, y1) = (m + (maxy - (b[3] + pad)) * s, m + (maxy - (b[1] - pad)) * s);
        let _ = writeln!(g, "<path class=\"rev-cloud\" data-id=\"{}\" data-status=\"{}\" d=\"{}\"><title>{} {} {}</title></path>", c.id, c.status, cloud_path(x0, y0, x1, y1), c.status, c.kind, c.id);
        n += 1;
    }
    g.push_str("</g>\n");
    if n == 0 {
        return svg.to_string();
    }
    match svg.rfind("</svg>") {
        Some(i) => format!("{}{}{}", &svg[..i], g, &svg[i..]),
        None => svg.to_string(),
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// `unix` seconds as `YYYY-MM-DD` (UTC).
pub fn date(unix: u64) -> String {
    let z = (unix / 86400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

/// The revision-history table (closed revisions and the open one) as an HTML fragment for the exports.
pub fn history_html(p: &NanoArchProject, ch: &[Change], arabic: bool) -> String {
    let t = |a: &'static str, e: &'static str| if arabic { a } else { e };
    let mut o = String::new();
    let _ = write!(o, "<section class=\"revhist\" style=\"margin-top:18px;page-break-inside:avoid\"><h2 style=\"font-size:15px\">{}</h2><table style=\"border-collapse:collapse;width:100%;font-size:12px\"><thead><tr>", t("سجل المراجعات", "Revision history"));
    for h in [t("المراجعة", "Rev."), t("الاسم", "Name"), t("التاريخ", "Date"), t("مضاف", "Added"), t("معدّل", "Modified"), t("محذوف", "Removed")] {
        let _ = write!(o, "<th style=\"border:1px solid #999;padding:3px 6px;background:#eee\">{h}</th>");
    }
    o.push_str("</tr></thead><tbody>");
    let cell = |s: String| format!("<td style=\"border:1px solid #999;padding:3px 6px\">{s}</td>");
    for h in &p.meta.revision_history {
        let _ = write!(o, "<tr>{}{}{}{}{}{}</tr>", cell(h.rev.to_string()), cell(esc(&h.name)), cell(date(h.created_unix)), cell(h.added.to_string()), cell(h.modified.to_string()), cell(h.removed.to_string()));
    }
    match &p.meta.baseline {
        Some(b) => {
            let (a, m, r) = counts(ch);
            let rev = p.meta.revision_history.last().map_or(1, |h| h.rev + 1);
            let _ = write!(o, "<tr>{}{}{}{}{}{}</tr>", cell(format!("{rev} ({})", t("الحالية", "current"))), cell(esc(&b.name)), cell(date(b.created_unix)), cell(a.to_string()), cell(m.to_string()), cell(r.to_string()));
        }
        None => {
            let _ = write!(o, "<tr><td colspan=\"6\" style=\"padding:6px\">{}</td></tr>", t("لم يُحدَّد خط أساس بعد", "No baseline has been set yet"));
        }
    }
    o.push_str("</tbody></table>");
    if !ch.is_empty() {
        let _ = write!(o, "<p style=\"font-size:11px;color:#555\">{}</p><ul style=\"font-size:11px;columns:2\">", t("العناصر المتغيّرة منذ خط الأساس", "Elements changed since the baseline"));
        for c in ch.iter().take(200) {
            let _ = write!(o, "<li>{} · {} · <span dir=\"ltr\">{}</span></li>", c.status, esc(&c.kind), c.id);
        }
        o.push_str("</ul>");
    }
    o.push_str("</section>");
    o
}
