//! Suspended ceiling plan: a reflected ceiling plan of one room with the luminaires, air terminals and other devices the user
//! places in it, the tile grid, and the checks that follow from those entries.
//!
//! Nothing about a ceiling is built in. The ceiling height, the structural soffit height, every device (luminaire, supply
//! diffuser, return grille, detector, ...) with its footprint (`w_mm` x `d_mm`), its centre position in the room's own
//! coordinates (origin at the room's south-west corner, x east, y north), its `recess_mm` (how far it extends above the ceiling
//! plane) and a `source` (the product data sheet) are the user's. The room is the model's room, found by its requirement key; its
//! rectangle (`w` x `h`) is the bounding box of the room, so an L-shaped room is checked against that box (stated in `notes`).
//!
//! Checks, each `passes`, `fails` or `unchecked`:
//!
//! - `fit`: every device lies inside the room rectangle;
//! - `overlap`: no two devices share ceiling area (touching is not an overlap);
//! - `spacing`: every pair of devices is at least `min_spacing_mm` apart. The limit and its `spacing_source` are the user's; with
//!   neither the check is `unchecked`;
//! - `plenum`: every device's `recess_mm` fits in the void between the ceiling and the soffit (`soffit_height_mm` minus
//!   `ceiling_height_mm`). `unchecked` unless the soffit height is entered and at least one device gives `recess_mm`;
//! - `headroom`: `ceiling_height_mm` is at least `min_ceiling_height_mm`. The limit and its `headroom_source` are the user's;
//!   with neither the check is `unchecked`.
//!
//! The tile grid (`tile_w_mm` x `tile_d_mm`, optional) starts at the room's south-west corner. The plan reports full and cut tiles
//! and how many tiles a device touches (those tiles are cut or replaced), counts of devices by kind and the ceiling area. No
//! lighting level, airflow or code figure is computed or built in. The result is a preliminary plan from the user's own entries,
//! not a shop drawing and not a code check.

use crate::model::{Id, Model};
use crate::permitpack::esc;
use crate::typed::{Level, Space, Typed};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

/// The most devices accepted in one call.
pub const MAX_ITEMS: usize = 500;
/// The most tiles in the grid, so a tiny module cannot make an unbounded drawing.
pub const MAX_TILES: usize = 40_000;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Device {
    pub key: String,
    /// Free text: `downlight`, `panel`, `diffuser`, `grille`, `detector`, ...
    pub kind: String,
    /// Centre of the footprint, mm from the room's south-west corner.
    pub x_mm: f64,
    pub y_mm: f64,
    /// Footprint size along x and along y, mm.
    pub w_mm: f64,
    pub d_mm: f64,
    /// How far the device extends above the ceiling plane, mm. Enables the plenum check.
    pub recess_mm: Option<f64>,
    /// The data sheet or specification the footprint and recess come from. Required.
    pub source: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Input {
    /// The room's requirement key.
    pub room: String,
    /// Restrict the lookup to one level by name; required when the key exists on more than one level.
    pub level: String,
    pub title: String,
    /// Finished ceiling height above the floor, mm. Required.
    pub ceiling_height_mm: Option<f64>,
    /// Height of the underside of the structure above, mm. Enables the plenum check.
    pub soffit_height_mm: Option<f64>,
    pub tile_w_mm: Option<f64>,
    pub tile_d_mm: Option<f64>,
    pub devices: Vec<Device>,
    pub min_spacing_mm: Option<f64>,
    pub spacing_source: String,
    pub min_ceiling_height_mm: Option<f64>,
    pub headroom_source: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DeviceRow {
    pub key: String,
    pub kind: String,
    pub w_mm: f64,
    pub d_mm: f64,
    pub recess_mm: Option<f64>,
    pub source: String,
    /// Gap to the west, east, south and north walls, mm (negative when the device crosses that wall).
    pub to_west_mm: f64,
    pub to_east_mm: f64,
    pub to_south_mm: f64,
    pub to_north_mm: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Issue {
    /// `fit`, `overlap`, `spacing`, `plenum` or `headroom`.
    pub check: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CheckRow {
    pub check: &'static str,
    /// `passes`, `fails` or `unchecked`.
    pub status: &'static str,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TileSummary {
    pub tile_w_mm: f64,
    pub tile_d_mm: f64,
    pub columns: usize,
    pub rows: usize,
    /// Tiles whole inside the room.
    pub full_tiles: usize,
    /// Border tiles that must be cut to the room edge.
    pub cut_tiles: usize,
    /// Distinct tiles a device footprint touches (cut or replaced by the device).
    pub tiles_touched_by_devices: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub room: String,
    pub level: String,
    pub function: String,
    pub width_mm: f64,
    pub depth_mm: f64,
    pub ceiling_area_m2: f64,
    pub ceiling_height_mm: f64,
    /// `soffit_height_mm - ceiling_height_mm`, when the soffit height was entered.
    pub plenum_mm: Option<f64>,
    pub devices: Vec<DeviceRow>,
    /// Number of devices of each kind.
    pub counts: BTreeMap<String, usize>,
    pub tiles: Option<TileSummary>,
    pub checks: Vec<CheckRow>,
    pub issues: Vec<Issue>,
    /// `fails` when any check fails, else `passes` when every check ran, else `incomplete`.
    pub overall: &'static str,
    pub svg: String,
    pub notes: Vec<String>,
}

#[derive(Clone, Copy)]
struct Rect {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Rect {
    fn of(d: &Device) -> Rect {
        Rect { x0: d.x_mm - d.w_mm / 2.0, y0: d.y_mm - d.d_mm / 2.0, x1: d.x_mm + d.w_mm / 2.0, y1: d.y_mm + d.d_mm / 2.0 }
    }

    fn overlaps(&self, o: &Rect) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }

    /// Shortest distance between the two rectangles; 0 when they touch or overlap.
    fn gap(&self, o: &Rect) -> f64 {
        let dx = (o.x0 - self.x1).max(self.x0 - o.x1).max(0.0);
        let dy = (o.y0 - self.y1).max(self.y0 - o.y1).max(0.0);
        dx.hypot(dy)
    }
}

fn pos(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// Builds the plan. Refuses, naming each problem, when an input is missing or malformed.
pub fn build(model: &Model, input: &Input) -> Result<Plan, String> {
    let mut errs: Vec<String> = Vec::new();
    let room_key = input.room.trim();
    if room_key.is_empty() {
        errs.push("room (the requirement key of the room whose ceiling is planned)".into());
    }
    match input.ceiling_height_mm {
        None => errs.push("ceiling_height_mm (the finished ceiling height above the floor)".into()),
        Some(c) if !pos(c) => errs.push(format!("ceiling_height_mm must be a positive number, got {c}")),
        _ => {}
    }
    if let (Some(s), Some(c)) = (input.soffit_height_mm, input.ceiling_height_mm) {
        if !pos(s) {
            errs.push(format!("soffit_height_mm must be a positive number, got {s}"));
        } else if pos(c) && s < c {
            errs.push(format!("soffit_height_mm ({s}) is below ceiling_height_mm ({c}); the ceiling cannot hang above the structure"));
        }
    }
    if input.devices.is_empty() {
        errs.push("devices (at least one, each with key, x_mm, y_mm, w_mm, d_mm and source)".into());
    }
    if input.devices.len() > MAX_ITEMS {
        errs.push(format!("at most {MAX_ITEMS} devices are accepted"));
    }
    match (input.min_spacing_mm, input.spacing_source.trim().is_empty()) {
        (Some(c), _) if !(c.is_finite() && c >= 0.0) => errs.push(format!("min_spacing_mm must be zero or more, got {c}")),
        (Some(_), true) => errs.push("spacing_source (the code or specification of min_spacing_mm)".into()),
        (None, false) => errs.push("min_spacing_mm (a spacing_source was given without a limit)".into()),
        _ => {}
    }
    match (input.min_ceiling_height_mm, input.headroom_source.trim().is_empty()) {
        (Some(c), _) if !pos(c) => errs.push(format!("min_ceiling_height_mm must be a positive number, got {c}")),
        (Some(_), true) => errs.push("headroom_source (the code or specification of min_ceiling_height_mm)".into()),
        (None, false) => errs.push("min_ceiling_height_mm (a headroom_source was given without a limit)".into()),
        _ => {}
    }
    match (input.tile_w_mm, input.tile_d_mm) {
        (None, None) => {}
        (Some(a), Some(b)) if pos(a) && pos(b) => {}
        (Some(_), Some(_)) => errs.push("tile_w_mm and tile_d_mm must be positive numbers".into()),
        _ => errs.push("tile_w_mm and tile_d_mm must be given together".into()),
    }
    let mut keys: HashSet<&str> = HashSet::new();
    for (i, d) in input.devices.iter().enumerate() {
        let k = d.key.trim();
        if k.is_empty() {
            errs.push(format!("devices[{i}].key"));
        } else if !keys.insert(k) {
            errs.push(format!("devices[{i}].key `{k}` is duplicated"));
        }
        let label = if k.is_empty() { format!("devices[{i}]") } else { format!("device `{k}`") };
        if !pos(d.w_mm) || !pos(d.d_mm) {
            errs.push(format!("{label}: w_mm and d_mm must be positive numbers"));
        }
        if !d.x_mm.is_finite() || !d.y_mm.is_finite() {
            errs.push(format!("{label}: x_mm and y_mm must be numbers"));
        }
        if d.recess_mm.is_some_and(|r| !(r.is_finite() && r >= 0.0)) {
            errs.push(format!("{label}: recess_mm must be zero or more"));
        }
        if d.source.trim().is_empty() {
            errs.push(format!("{label}: source (the data sheet or specification of its footprint)"));
        }
    }

    let level_names: BTreeMap<Id, String> = model.elements.values().filter(|e| e.kind == Level::KIND).filter_map(|e| Some((e.id, e.typed::<Level>().ok()?.name))).collect();
    let level_filter = input.level.trim();
    if !level_filter.is_empty() && !level_names.values().any(|n| n == level_filter) {
        errs.push(format!("level `{level_filter}` is not a level of the model"));
    }
    if !errs.is_empty() {
        return Err(format!("ceiling plan: {}. Nothing is defaulted and no device or height is built in; enter them.", errs.join("; ")));
    }
    let ceiling_height = input.ceiling_height_mm.unwrap_or_default();

    let mut found: Vec<(Space, String)> = Vec::new();
    for e in model.elements.values().filter(|e| e.kind == Space::KIND) {
        let Ok(s) = e.typed::<Space>() else { continue };
        if s.requirement_key != room_key {
            continue;
        }
        let level = e.level_id.and_then(|id| level_names.get(&id).cloned()).unwrap_or_default();
        if !level_filter.is_empty() && level != level_filter {
            continue;
        }
        found.push((s, level));
    }
    let (space, level) = match found.len() {
        0 => return Err(format!("ceiling plan: the model has no room with key `{room_key}`{}", if level_filter.is_empty() { String::new() } else { format!(" on level `{level_filter}`") })),
        1 => found.remove(0),
        n => {
            let names: Vec<String> = found.iter().map(|(_, l)| format!("`{l}`")).collect();
            return Err(format!("ceiling plan: {n} rooms have key `{room_key}` (levels {}); set level to choose one", names.join(", ")));
        }
    };
    let (w, h) = (space.w as f64, space.h as f64);
    if !pos(w) || !pos(h) {
        return Err(format!("ceiling plan: room `{room_key}` has no positive width and depth in the model"));
    }

    // Tile grid, bounded before any drawing.
    let grid = match (input.tile_w_mm, input.tile_d_mm) {
        (Some(tw), Some(td)) => {
            let (cols, rows) = ((w / tw).ceil() as usize, (h / td).ceil() as usize);
            if cols.saturating_mul(rows) > MAX_TILES {
                return Err(format!("ceiling plan: a {tw} x {td} mm tile gives {cols} x {rows} tiles in this room; at most {MAX_TILES} are accepted. Enter a larger tile module."));
            }
            Some((tw, td, cols, rows))
        }
        _ => None,
    };

    let rects: Vec<Rect> = input.devices.iter().map(Rect::of).collect();
    let mut issues: Vec<Issue> = Vec::new();
    let mut rows: Vec<DeviceRow> = Vec::new();
    let mut outside = 0usize;
    for (d, r) in input.devices.iter().zip(&rects) {
        let row = DeviceRow {
            key: d.key.trim().to_string(),
            kind: d.kind.trim().to_string(),
            w_mm: d.w_mm,
            d_mm: d.d_mm,
            recess_mm: d.recess_mm,
            source: d.source.trim().to_string(),
            to_west_mm: r1(r.x0),
            to_east_mm: r1(w - r.x1),
            to_south_mm: r1(r.y0),
            to_north_mm: r1(h - r.y1),
        };
        if row.to_west_mm < 0.0 || row.to_east_mm < 0.0 || row.to_south_mm < 0.0 || row.to_north_mm < 0.0 {
            outside += 1;
            issues.push(Issue { check: "fit", message: format!("`{}` crosses a wall of the {w} x {h} mm room (gaps west {}, east {}, south {}, north {} mm)", row.key, row.to_west_mm, row.to_east_mm, row.to_south_mm, row.to_north_mm) });
        }
        rows.push(row);
    }
    let mut overlaps = 0usize;
    let mut tight = 0usize;
    for i in 0..rects.len() {
        for j in (i + 1)..rects.len() {
            let (a, b) = (&input.devices[i], &input.devices[j]);
            if rects[i].overlaps(&rects[j]) {
                overlaps += 1;
                issues.push(Issue { check: "overlap", message: format!("`{}` and `{}` overlap", a.key.trim(), b.key.trim()) });
                continue;
            }
            if let Some(min) = input.min_spacing_mm {
                let gap = rects[i].gap(&rects[j]);
                if gap < min {
                    tight += 1;
                    issues.push(Issue { check: "spacing", message: format!("`{}` and `{}` are {} mm apart, under the {min} mm limit", a.key.trim(), b.key.trim(), r1(gap)) });
                }
            }
        }
    }

    let plenum = input.soffit_height_mm.map(|s| s - ceiling_height);
    let recessed = input.devices.iter().filter(|d| d.recess_mm.is_some()).count();
    let mut too_deep = 0usize;
    if let Some(p) = plenum {
        for d in &input.devices {
            if let Some(rc) = d.recess_mm.filter(|rc| *rc > p) {
                too_deep += 1;
                issues.push(Issue { check: "plenum", message: format!("`{}` extends {rc} mm above the ceiling but the void is {} mm", d.key.trim(), r1(p)) });
            }
        }
    }
    let headroom_fail = input.min_ceiling_height_mm.filter(|m| ceiling_height < *m);
    if let Some(m) = headroom_fail {
        issues.push(Issue { check: "headroom", message: format!("ceiling height {ceiling_height} mm is under the {m} mm limit") });
    }

    let verdict = |failed: usize, ok: String| if failed > 0 { ("fails", format!("{failed} problem(s)")) } else { ("passes", ok) };
    let mut checks: Vec<CheckRow> = Vec::new();
    let (s, r) = verdict(outside, format!("all {} device(s) are inside the room", rows.len()));
    checks.push(CheckRow { check: "fit", status: s, reason: r });
    let (s, r) = verdict(overlaps, "no two devices overlap".to_string());
    checks.push(CheckRow { check: "overlap", status: s, reason: r });
    match input.min_spacing_mm {
        None => checks.push(CheckRow { check: "spacing", status: "unchecked", reason: "no min_spacing_mm with its source was entered".into() }),
        Some(m) => {
            let (s, r) = verdict(tight, format!("every pair is at least {m} mm apart ({})", input.spacing_source.trim()));
            checks.push(CheckRow { check: "spacing", status: s, reason: r });
        }
    }
    match (plenum, recessed) {
        (None, _) => checks.push(CheckRow { check: "plenum", status: "unchecked", reason: "no soffit_height_mm was entered".into() }),
        (Some(_), 0) => checks.push(CheckRow { check: "plenum", status: "unchecked", reason: "no device gave recess_mm".into() }),
        (Some(p), n) => {
            let (s, r) = verdict(too_deep, format!("{n} device(s) with a recess fit the {} mm void", r1(p)));
            checks.push(CheckRow { check: "plenum", status: s, reason: r });
        }
    }
    match input.min_ceiling_height_mm {
        None => checks.push(CheckRow { check: "headroom", status: "unchecked", reason: "no min_ceiling_height_mm with its source was entered".into() }),
        Some(m) => {
            let (s, r) = verdict(usize::from(headroom_fail.is_some()), format!("{ceiling_height} mm is at least {m} mm ({})", input.headroom_source.trim()));
            checks.push(CheckRow { check: "headroom", status: s, reason: r });
        }
    }
    let overall = if checks.iter().any(|c| c.status == "fails") {
        "fails"
    } else if checks.iter().any(|c| c.status == "unchecked") {
        "incomplete"
    } else {
        "passes"
    };

    let tiles = grid.map(|(tw, td, cols, nrows)| {
        let full = ((w / tw + 1e-9).floor() as usize) * ((h / td + 1e-9).floor() as usize);
        let mut touched: HashSet<(usize, usize)> = HashSet::new();
        for r in &rects {
            if r.x1 <= 0.0 || r.y1 <= 0.0 || r.x0 >= w || r.y0 >= h {
                continue;
            }
            let c0 = (r.x0 / tw).floor().max(0.0) as usize;
            let c1 = (((r.x1 / tw).ceil() - 1.0).max(0.0) as usize).min(cols - 1);
            let r0 = (r.y0 / td).floor().max(0.0) as usize;
            let r1_ = (((r.y1 / td).ceil() - 1.0).max(0.0) as usize).min(nrows - 1);
            for c in c0..=c1 {
                for rr in r0..=r1_ {
                    touched.insert((c, rr));
                }
            }
        }
        TileSummary { tile_w_mm: tw, tile_d_mm: td, columns: cols, rows: nrows, full_tiles: full, cut_tiles: cols * nrows - full, tiles_touched_by_devices: touched.len() }
    });

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for d in &input.devices {
        let k = d.kind.trim();
        *counts.entry(if k.is_empty() { "unspecified".to_string() } else { k.to_string() }).or_default() += 1;
    }

    let title = if input.title.trim().is_empty() { format!("{room_key} / {} - RCP", space.func) } else { input.title.trim().to_string() };
    let svg = draw(&title, w, h, grid.map(|(tw, td, _, _)| (tw, td)), input, &rects, &issues);
    let mut notes = vec![
        "Heights, device footprints, positions, recesses, limits and every source are the user's entries; the application holds no ceiling, lighting or air-terminal figure and computes no lux level or airflow. The room is the model room's bounding rectangle (an L-shaped room is checked against its box). The tile grid starts at the room's south-west corner; border tiles are counted as cut. Preliminary plan, not a shop drawing and not a code check / الأرقام والمصادر من إدخال المستخدم؛ لا تُحسب إضاءة ولا تدفق هواء؛ الغرفة مستطيلها المحيط؛ مخطط أولي لا مخطط تنفيذ ولا فحص كود.".to_string(),
    ];
    if overall == "incomplete" {
        let names: Vec<&str> = checks.iter().filter(|c| c.status == "unchecked").map(|c| c.check).collect();
        notes.push(format!("Not checked (never reported as passed): {} / فحوص لم تُجرَ.", names.join(", ")));
    }
    Ok(Plan {
        room: room_key.to_string(),
        level,
        function: space.func.clone(),
        width_mm: w,
        depth_mm: h,
        ceiling_area_m2: (w * h / 1e4).round() / 100.0,
        ceiling_height_mm: ceiling_height,
        plenum_mm: plenum,
        devices: rows,
        counts,
        tiles,
        checks,
        issues,
        overall,
        svg,
        notes,
    })
}

fn draw(title: &str, w: f64, h: f64, grid: Option<(f64, f64)>, input: &Input, rects: &[Rect], issues: &[Issue]) -> String {
    const VIEW: f64 = 760.0;
    const PAD: f64 = 70.0;
    let s = VIEW / w.max(h);
    let (pw, ph) = (w * s, h * s);
    let (tw, th) = (pw + 2.0 * PAD, ph + 2.0 * PAD + 30.0);
    // Plan coordinates: x east, y north; the SVG y axis points down.
    let px = |x: f64| PAD + x * s;
    let py = |y: f64| 30.0 + PAD + (h - y) * s;
    let bad: HashSet<&str> = issues.iter().flat_map(|i| i.message.split('`').skip(1).step_by(2)).collect();
    let mut o = String::new();
    let _ = write!(o, "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {tw:.1} {th:.1}\" width=\"{tw:.0}\" height=\"{th:.0}\" font-family=\"sans-serif\" font-size=\"12\">");
    let _ = write!(o, "<rect width=\"100%\" height=\"100%\" fill=\"#fff\"/><text x=\"{:.1}\" y=\"22\" font-size=\"15\" font-weight=\"bold\" text-anchor=\"middle\">{}</text>", tw / 2.0, esc(title));
    if let Some((gw, gd)) = grid {
        let mut x = gw;
        while x < w {
            let _ = write!(o, "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"#bbb\" stroke-width=\"0.6\"/>", px(x), py(0.0), px(x), py(h));
            x += gw;
        }
        let mut y = gd;
        while y < h {
            let _ = write!(o, "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"#bbb\" stroke-width=\"0.6\"/>", px(0.0), py(y), px(w), py(y));
            y += gd;
        }
    }
    let _ = write!(o, "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{pw:.1}\" height=\"{ph:.1}\" fill=\"none\" stroke=\"#222\" stroke-width=\"3\"/>", px(0.0), py(h));
    for (d, r) in input.devices.iter().zip(rects) {
        let key = d.key.trim();
        let (stroke, fill) = if bad.contains(key) { ("#b00", "#fdd") } else { ("#333", "#fff7d6") };
        let _ = write!(o, "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"1.5\"/>", px(r.x0), py(r.y1), (r.x1 - r.x0) * s, (r.y1 - r.y0) * s);
        let label = if d.kind.trim().is_empty() { key.to_string() } else { format!("{key} ({})", d.kind.trim()) };
        let _ = write!(o, "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-size=\"11\">{}</text>", px(d.x_mm), py(d.y_mm) + 4.0, esc(&label));
        // Centre dimensions to the west and south walls.
        let _ = write!(o, "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"#a60\" stroke-width=\"0.8\" stroke-dasharray=\"3 2\"/><text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-size=\"10\" fill=\"#a60\">{}</text>", px(0.0), py(d.y_mm), px(d.x_mm), py(d.y_mm), px(d.x_mm / 2.0), py(d.y_mm) - 3.0, d.x_mm.round());
        let _ = write!(o, "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"#a60\" stroke-width=\"0.8\" stroke-dasharray=\"3 2\"/><text x=\"{:.1}\" y=\"{:.1}\" font-size=\"10\" fill=\"#a60\">{}</text>", px(d.x_mm), py(0.0), px(d.x_mm), py(d.y_mm), px(d.x_mm) + 3.0, py(d.y_mm / 2.0), d.y_mm.round());
    }
    let _ = write!(o, "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\">{} mm</text>", px(w / 2.0), py(0.0) + 30.0, w.round());
    let _ = write!(o, "<text transform=\"translate({:.1} {:.1}) rotate(-90)\" text-anchor=\"middle\">{} mm</text></svg>", px(0.0) - 34.0, py(h / 2.0), h.round());
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Command, Origin};
    use crate::model::Source;
    use crate::typed::element_of;
    use crate::Document;

    fn space(key: &str, func: &str, w: i64, h: i64) -> Space {
        Space { func: func.into(), requirement_key: key.into(), area: (w * h) as f64 / 1e6, target_area: 0.0, x: 0, y: 0, w, h, boundary: vec![[0, 0], [w, 0], [w, h], [0, h]], net_area: None, glazing_ratio: None, extra: Default::default() }
    }

    fn doc() -> Document {
        let mut d = Document::default();
        let lv = element_of(&Level { name: "Ground".into(), index: 0, extra: Default::default() }, Source::User);
        let lid = lv.id;
        let mut cmds = vec![Command::Create(lv)];
        for s in [space("living", "living", 4800, 3600), space("kit", "kitchen", 3000, 3000)] {
            let mut e = element_of(&s, Source::User);
            e.level_id = Some(lid);
            cmds.push(Command::Create(e));
        }
        d.commit("rooms", Origin::User, cmds).unwrap();
        d
    }

    fn dev(key: &str, kind: &str, x: f64, y: f64, w: f64, d: f64, recess: Option<f64>) -> Device {
        Device { key: key.into(), kind: kind.into(), x_mm: x, y_mm: y, w_mm: w, d_mm: d, recess_mm: recess, source: "Maker data sheet 7, 2026-01-01".into() }
    }

    fn input() -> Input {
        Input {
            room: "living".into(),
            ceiling_height_mm: Some(2700.0),
            soffit_height_mm: Some(3000.0),
            tile_w_mm: Some(600.0),
            tile_d_mm: Some(600.0),
            devices: vec![dev("L1", "panel", 1200.0, 900.0, 600.0, 600.0, Some(80.0)), dev("L2", "panel", 3600.0, 900.0, 600.0, 600.0, Some(80.0)), dev("D1", "diffuser", 2400.0, 2700.0, 300.0, 300.0, Some(200.0))],
            min_spacing_mm: Some(500.0),
            spacing_source: "Project brief 5.1, 2026-01-02".into(),
            min_ceiling_height_mm: Some(2400.0),
            headroom_source: "Project brief 5.2, 2026-01-02".into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_fitting_plan_passes_and_reports_walls_counts_plenum_and_tiles() {
        let p = build(&doc().model, &input()).unwrap();
        assert_eq!((p.overall, p.level.as_str(), p.width_mm, p.depth_mm, p.plenum_mm), ("passes", "Ground", 4800.0, 3600.0, Some(300.0)));
        assert!(p.issues.is_empty(), "{:?}", p.issues);
        let l1 = &p.devices[0];
        assert_eq!((l1.to_west_mm, l1.to_east_mm, l1.to_south_mm, l1.to_north_mm), (900.0, 3300.0, 600.0, 2400.0));
        assert_eq!((p.counts["panel"], p.counts["diffuser"], p.ceiling_area_m2), (2, 1, 17.28));
        let t = p.tiles.unwrap();
        assert_eq!((t.columns, t.rows, t.full_tiles, t.cut_tiles), (8, 6, 48, 0));
        // L1 spans x 900..1500 (columns 1-2) on row 1; L2 spans columns 5-6 on row 1; D1 spans columns 3-4 on row 4.
        assert_eq!(t.tiles_touched_by_devices, 2 + 2 + 2);
        assert!(p.svg.starts_with("<svg") && p.svg.contains("4800 mm") && p.svg.ends_with("</svg>"));
    }

    #[test]
    fn border_tiles_are_cut_when_the_module_does_not_divide_the_room() {
        let mut i = input();
        i.tile_w_mm = Some(1000.0);
        i.tile_d_mm = Some(1200.0);
        let t = build(&doc().model, &i).unwrap().tiles.unwrap();
        assert_eq!((t.columns, t.rows, t.full_tiles, t.cut_tiles), (5, 3, 12, 3));
    }

    #[test]
    fn crossing_overlap_spacing_plenum_and_headroom_problems_are_each_reported() {
        let mut i = input();
        i.devices = vec![dev("a", "panel", 100.0, 900.0, 600.0, 600.0, Some(80.0)), dev("b", "panel", 500.0, 900.0, 600.0, 600.0, Some(80.0)), dev("c", "panel", 1500.0, 900.0, 600.0, 600.0, Some(400.0)), dev("d", "panel", 2400.0, 900.0, 600.0, 600.0, None)];
        i.min_ceiling_height_mm = Some(2800.0);
        let p = build(&doc().model, &i).unwrap();
        let has = |c: &str| p.issues.iter().any(|x| x.check == c);
        assert!(has("fit") && has("overlap") && has("spacing") && has("plenum") && has("headroom"), "{:?}", p.issues);
        assert_eq!(p.overall, "fails");
        assert!(p.svg.contains("#b00"));
    }

    #[test]
    fn checks_without_their_inputs_are_unchecked_never_passed() {
        let mut i = input();
        i.soffit_height_mm = None;
        i.min_spacing_mm = None;
        i.spacing_source.clear();
        i.min_ceiling_height_mm = None;
        i.headroom_source.clear();
        i.tile_w_mm = None;
        i.tile_d_mm = None;
        let p = build(&doc().model, &i).unwrap();
        assert_eq!(p.overall, "incomplete");
        assert_eq!(p.checks.iter().filter(|c| c.status == "unchecked").map(|c| c.check).collect::<Vec<_>>(), ["spacing", "plenum", "headroom"]);
        assert!(p.tiles.is_none() && p.plenum_mm.is_none());
        let mut i = input();
        for d in &mut i.devices {
            d.recess_mm = None;
        }
        let p = build(&doc().model, &i).unwrap();
        assert_eq!(p.checks.iter().find(|c| c.check == "plenum").unwrap().status, "unchecked");
    }

    #[test]
    fn missing_or_malformed_inputs_are_named_and_unknown_rooms_refused() {
        let e = build(&doc().model, &Input::default()).unwrap_err();
        assert!(e.contains("room") && e.contains("ceiling_height_mm") && e.contains("devices"), "{e}");
        let mut i = input();
        i.devices[0].source = " ".into();
        i.devices[1].w_mm = 0.0;
        i.devices[2].key = "L1".into();
        i.spacing_source.clear();
        i.headroom_source.clear();
        i.tile_d_mm = None;
        i.soffit_height_mm = Some(2500.0);
        let e = build(&doc().model, &i).unwrap_err();
        assert!(e.contains("source") && e.contains("w_mm and d_mm") && e.contains("duplicated") && e.contains("spacing_source") && e.contains("headroom_source") && e.contains("given together") && e.contains("below ceiling_height_mm"), "{e}");
        let mut i = input();
        i.room = "garage".into();
        assert!(build(&doc().model, &i).unwrap_err().contains("no room with key `garage`"));
        let mut i = input();
        i.tile_w_mm = Some(1.0);
        i.tile_d_mm = Some(1.0);
        assert!(build(&doc().model, &i).unwrap_err().contains("at most"));
        let mut i = input();
        i.level = "Roof".into();
        assert!(build(&doc().model, &i).unwrap_err().contains("not a level"));
    }

    #[test]
    fn text_is_escaped_in_the_drawing() {
        let mut i = input();
        i.title = "<script>x</script>".into();
        i.devices[0].kind = "<b>".into();
        let p = build(&doc().model, &i).unwrap();
        assert!(!p.svg.contains("<script>") && !p.svg.contains("(<b>)") && p.svg.contains("&lt;script&gt;"));
    }
}
