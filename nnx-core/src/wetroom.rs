//! Bathroom and kitchen detail: an enlarged plan of one room with the fixtures the user places in it, dimensioned to the walls,
//! checked for fit, overlap, door swing and clearance.
//!
//! Nothing about a fixture is built in. Every fixture (WC, basin, shower tray, sink, hob, cabinet run, ...) is entered with its
//! footprint (`w_mm` x `d_mm`), its position in the room's own coordinates (origin at the room's south-west corner, x east, y
//! north) and a `source` (the product data sheet or specification it was taken from). The room is the model's room, found by its
//! requirement key; its rectangle (`w` x `h`) is the bounding box of the room, so an L-shaped room is checked against that box
//! (stated in `notes`).
//!
//! Checks, each `passes`, `fails` or `unchecked`:
//!
//! - `fit`: every fixture lies inside the room rectangle;
//! - `overlap`: no two fixtures share floor area (touching is not an overlap);
//! - `door_swing`: no fixture intrudes on a door's swing zone, the `width_mm` x `swing_mm` rectangle in front of the opening. It
//!   is `unchecked` unless a door gives `swing_mm` (the zone is the leaf's bounding box, so it is conservative);
//! - `clearance`: every pair of fixtures is at least `min_clearance_mm` apart, except two fixtures of the same non-empty `group`
//!   (a kitchen run). The limit and its `clearance_source` are the user's; with neither the check is `unchecked`. No clearance
//!   figure from any code is built in.
//!
//! The result is a preliminary detail from the user's own dimensions, not a shop drawing and not a code check.

use crate::model::{Id, Model};
use crate::permitpack::esc;
use crate::typed::{Level, Space, Typed};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

/// The most fixtures and doors accepted in one call.
pub const MAX_ITEMS: usize = 200;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Fixture {
    pub key: String,
    /// Free text: `wc`, `basin`, `shower`, `sink`, `hob`, ...
    pub kind: String,
    /// West edge of the footprint, mm from the room's south-west corner.
    pub x_mm: f64,
    /// South edge of the footprint, mm from the room's south-west corner.
    pub y_mm: f64,
    /// Footprint size along x and along y, mm.
    pub w_mm: f64,
    pub d_mm: f64,
    /// Fixtures of the same non-empty group may touch (a kitchen run); the clearance check skips their mutual pairs.
    pub group: String,
    /// The data sheet or specification the footprint comes from. Required.
    pub source: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DoorIn {
    /// `south` (y = 0), `north` (y = h), `west` (x = 0) or `east` (x = w).
    pub wall: String,
    /// Distance from the wall's start (x = 0 or y = 0) to the near jamb.
    pub offset_mm: f64,
    pub width_mm: f64,
    /// How far the leaf sweeps into the room; enables the door swing check.
    pub swing_mm: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Input {
    /// The room's requirement key.
    pub room: String,
    /// Restrict the lookup to one level by name; required when the key exists on more than one level.
    pub level: String,
    pub title: String,
    pub fixtures: Vec<Fixture>,
    pub doors: Vec<DoorIn>,
    pub min_clearance_mm: Option<f64>,
    pub clearance_source: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FixtureRow {
    pub key: String,
    pub kind: String,
    pub w_mm: f64,
    pub d_mm: f64,
    pub area_m2: f64,
    pub source: String,
    /// Gap to the west, east, south and north walls, mm (negative when the fixture crosses that wall).
    pub to_west_mm: f64,
    pub to_east_mm: f64,
    pub to_south_mm: f64,
    pub to_north_mm: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Issue {
    /// `fit`, `overlap`, `door_swing` or `clearance`.
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

#[derive(Debug, Clone, Serialize)]
pub struct Detail {
    pub room: String,
    pub level: String,
    pub function: String,
    pub width_mm: f64,
    pub depth_mm: f64,
    pub fixtures: Vec<FixtureRow>,
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
    fn of(f: &Fixture) -> Rect {
        Rect { x0: f.x_mm, y0: f.y_mm, x1: f.x_mm + f.w_mm, y1: f.y_mm + f.d_mm }
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

fn swing_zone(d: &DoorIn, w: f64, h: f64) -> Option<Rect> {
    let s = d.swing_mm?;
    let (a, b) = (d.offset_mm, d.offset_mm + d.width_mm);
    Some(match d.wall.trim() {
        "south" => Rect { x0: a, y0: 0.0, x1: b, y1: s },
        "north" => Rect { x0: a, y0: h - s, x1: b, y1: h },
        "west" => Rect { x0: 0.0, y0: a, x1: s, y1: b },
        _ => Rect { x0: w - s, y0: a, x1: w, y1: b },
    })
}

/// Builds the detail. Refuses, naming each problem, when an input is missing or malformed.
pub fn build(model: &Model, input: &Input) -> Result<Detail, String> {
    let mut errs: Vec<String> = Vec::new();
    let room_key = input.room.trim();
    if room_key.is_empty() {
        errs.push("room (the requirement key of the room to detail)".into());
    }
    if input.fixtures.is_empty() {
        errs.push("fixtures (at least one, each with key, x_mm, y_mm, w_mm, d_mm and source)".into());
    }
    if input.fixtures.len() > MAX_ITEMS || input.doors.len() > MAX_ITEMS {
        errs.push(format!("at most {MAX_ITEMS} fixtures and {MAX_ITEMS} doors are accepted"));
    }
    match (input.min_clearance_mm, input.clearance_source.trim().is_empty()) {
        (Some(c), _) if !(c.is_finite() && c >= 0.0) => errs.push(format!("min_clearance_mm must be zero or more, got {c}")),
        (Some(_), true) => errs.push("clearance_source (the code or specification of min_clearance_mm)".into()),
        (None, false) => errs.push("min_clearance_mm (a clearance_source was given without a limit)".into()),
        _ => {}
    }
    let mut keys: HashSet<&str> = HashSet::new();
    for (i, f) in input.fixtures.iter().enumerate() {
        let k = f.key.trim();
        if k.is_empty() {
            errs.push(format!("fixtures[{i}].key"));
        } else if !keys.insert(k) {
            errs.push(format!("fixtures[{i}].key `{k}` is duplicated"));
        }
        let label = if k.is_empty() { format!("fixtures[{i}]") } else { format!("fixture `{k}`") };
        if !pos(f.w_mm) || !pos(f.d_mm) {
            errs.push(format!("{label}: w_mm and d_mm must be positive numbers"));
        }
        if !f.x_mm.is_finite() || !f.y_mm.is_finite() {
            errs.push(format!("{label}: x_mm and y_mm must be numbers"));
        }
        if f.source.trim().is_empty() {
            errs.push(format!("{label}: source (the data sheet or specification of its footprint)"));
        }
    }
    for (i, d) in input.doors.iter().enumerate() {
        if !["south", "north", "west", "east"].contains(&d.wall.trim()) {
            errs.push(format!("doors[{i}].wall must be south, north, west or east, got `{}`", d.wall.trim()));
        }
        if !pos(d.width_mm) || !d.offset_mm.is_finite() || d.offset_mm < 0.0 {
            errs.push(format!("doors[{i}]: width_mm must be positive and offset_mm zero or more"));
        }
        if d.swing_mm.is_some_and(|s| !pos(s)) {
            errs.push(format!("doors[{i}].swing_mm must be a positive number"));
        }
    }

    let level_names: BTreeMap<Id, String> = model.elements.values().filter(|e| e.kind == Level::KIND).filter_map(|e| Some((e.id, e.typed::<Level>().ok()?.name))).collect();
    let level_filter = input.level.trim();
    if !level_filter.is_empty() && !level_names.values().any(|n| n == level_filter) {
        errs.push(format!("level `{level_filter}` is not a level of the model"));
    }
    if !errs.is_empty() {
        return Err(format!("wet room detail: {}. Nothing is defaulted and no fixture is built in; enter them.", errs.join("; ")));
    }

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
        0 => return Err(format!("wet room detail: the model has no room with key `{room_key}`{}", if level_filter.is_empty() { String::new() } else { format!(" on level `{level_filter}`") })),
        1 => found.remove(0),
        n => {
            let names: Vec<String> = found.iter().map(|(_, l)| format!("`{l}`")).collect();
            return Err(format!("wet room detail: {n} rooms have key `{room_key}` (levels {}); set level to choose one", names.join(", ")));
        }
    };
    let (w, h) = (space.w as f64, space.h as f64);
    if !pos(w) || !pos(h) {
        return Err(format!("wet room detail: room `{room_key}` has no positive width and depth in the model"));
    }
    for (i, d) in input.doors.iter().enumerate() {
        let wall_len = if matches!(d.wall.trim(), "south" | "north") { w } else { h };
        if d.offset_mm + d.width_mm > wall_len {
            errs.push(format!("doors[{i}] ({} .. {} mm) runs past the {} wall, which is {} mm long", d.offset_mm, d.offset_mm + d.width_mm, d.wall.trim(), wall_len));
        }
    }
    if !errs.is_empty() {
        return Err(format!("wet room detail: {}.", errs.join("; ")));
    }

    let rects: Vec<Rect> = input.fixtures.iter().map(Rect::of).collect();
    let mut issues: Vec<Issue> = Vec::new();
    let mut rows: Vec<FixtureRow> = Vec::new();
    let mut outside = 0usize;
    for (f, r) in input.fixtures.iter().zip(&rects) {
        let row = FixtureRow {
            key: f.key.trim().to_string(),
            kind: f.kind.trim().to_string(),
            w_mm: f.w_mm,
            d_mm: f.d_mm,
            area_m2: (f.w_mm * f.d_mm / 1e4).round() / 100.0,
            source: f.source.trim().to_string(),
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
            let (a, b) = (&input.fixtures[i], &input.fixtures[j]);
            if rects[i].overlaps(&rects[j]) {
                overlaps += 1;
                issues.push(Issue { check: "overlap", message: format!("`{}` and `{}` overlap", a.key.trim(), b.key.trim()) });
                continue;
            }
            let same_group = !a.group.trim().is_empty() && a.group.trim() == b.group.trim();
            if let (Some(min), false) = (input.min_clearance_mm, same_group) {
                let gap = rects[i].gap(&rects[j]);
                if gap < min {
                    tight += 1;
                    issues.push(Issue { check: "clearance", message: format!("`{}` and `{}` are {} mm apart, under the {min} mm limit", a.key.trim(), b.key.trim(), r1(gap)) });
                }
            }
        }
    }
    let zones: Vec<(usize, Rect)> = input.doors.iter().enumerate().filter_map(|(i, d)| swing_zone(d, w, h).map(|z| (i, z))).collect();
    let mut swing_hits = 0usize;
    for (di, z) in &zones {
        for (f, r) in input.fixtures.iter().zip(&rects) {
            if z.overlaps(r) {
                swing_hits += 1;
                issues.push(Issue { check: "door_swing", message: format!("`{}` intrudes on the swing of door {} ({} wall)", f.key.trim(), di + 1, input.doors[*di].wall.trim()) });
            }
        }
    }

    let verdict = |failed: usize, ok: String| if failed > 0 { ("fails", format!("{failed} problem(s)")) } else { ("passes", ok) };
    let mut checks: Vec<CheckRow> = Vec::new();
    let (s, r) = verdict(outside, format!("all {} fixture(s) are inside the room", rows.len()));
    checks.push(CheckRow { check: "fit", status: s, reason: r });
    let (s, r) = verdict(overlaps, "no two fixtures overlap".to_string());
    checks.push(CheckRow { check: "overlap", status: s, reason: r });
    if zones.is_empty() {
        checks.push(CheckRow { check: "door_swing", status: "unchecked", reason: "no door gave swing_mm".into() });
    } else {
        let (s, r) = verdict(swing_hits, format!("{} door swing zone(s) are clear", zones.len()));
        checks.push(CheckRow { check: "door_swing", status: s, reason: r });
    }
    match input.min_clearance_mm {
        None => checks.push(CheckRow { check: "clearance", status: "unchecked", reason: "no min_clearance_mm with its source was entered".into() }),
        Some(m) => {
            let (s, r) = verdict(tight, format!("every pair is at least {m} mm apart ({})", input.clearance_source.trim()));
            checks.push(CheckRow { check: "clearance", status: s, reason: r });
        }
    }
    let overall = if checks.iter().any(|c| c.status == "fails") {
        "fails"
    } else if checks.iter().any(|c| c.status == "unchecked") {
        "incomplete"
    } else {
        "passes"
    };

    let title = if input.title.trim().is_empty() { format!("{room_key} / {}", space.func) } else { input.title.trim().to_string() };
    let svg = draw(&title, w, h, input, &rects, &zones.iter().map(|(_, z)| *z).collect::<Vec<_>>(), &issues);
    let mut notes = vec![
        "Fixture footprints, positions, the clearance limit and every source are the user's entries; the application holds no fixture or clearance figure. The room is the model room's bounding rectangle (an L-shaped room is checked against its box). Door swing zones are the leaf's bounding box. Preliminary detail, not a shop drawing and not a code check / المقاسات والمصادر من إدخال المستخدم؛ الغرفة مستطيلها المحيط؛ تفصيل أولي لا مخطط تنفيذ ولا فحص كود.".to_string(),
    ];
    if overall == "incomplete" {
        let names: Vec<&str> = checks.iter().filter(|c| c.status == "unchecked").map(|c| c.check).collect();
        notes.push(format!("Not checked (never reported as passed): {} / فحوص لم تُجرَ.", names.join(", ")));
    }
    Ok(Detail { room: room_key.to_string(), level, function: space.func.clone(), width_mm: w, depth_mm: h, fixtures: rows, checks, issues, overall, svg, notes })
}

fn draw(title: &str, w: f64, h: f64, input: &Input, rects: &[Rect], zones: &[Rect], issues: &[Issue]) -> String {
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
    for z in zones {
        let _ = write!(o, "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" fill=\"#cde\" fill-opacity=\"0.5\" stroke=\"#58a\" stroke-dasharray=\"5 3\"/>", px(z.x0), py(z.y1), (z.x1 - z.x0) * s, (z.y1 - z.y0) * s);
    }
    let _ = write!(o, "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{pw:.1}\" height=\"{ph:.1}\" fill=\"none\" stroke=\"#222\" stroke-width=\"3\"/>", px(0.0), py(h));
    for d in &input.doors {
        let (a, b) = (d.offset_mm, d.offset_mm + d.width_mm);
        let (x0, y0, x1, y1) = match d.wall.trim() {
            "south" => (px(a), py(0.0), px(b), py(0.0)),
            "north" => (px(a), py(h), px(b), py(h)),
            "west" => (px(0.0), py(a), px(0.0), py(b)),
            _ => (px(w), py(a), px(w), py(b)),
        };
        let _ = write!(o, "<line x1=\"{x0:.1}\" y1=\"{y0:.1}\" x2=\"{x1:.1}\" y2=\"{y1:.1}\" stroke=\"#fff\" stroke-width=\"5\"/><line x1=\"{x0:.1}\" y1=\"{y0:.1}\" x2=\"{x1:.1}\" y2=\"{y1:.1}\" stroke=\"#58a\" stroke-width=\"2\" stroke-dasharray=\"2 2\"/>");
    }
    for (f, r) in input.fixtures.iter().zip(rects) {
        let key = f.key.trim();
        let (stroke, fill) = if bad.contains(key) { ("#b00", "#fdd") } else { ("#333", "#eef2f6") };
        let _ = write!(o, "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"1.5\"/>", px(r.x0), py(r.y1), (r.x1 - r.x0) * s, (r.y1 - r.y0) * s);
        let label = if f.kind.trim().is_empty() { key.to_string() } else { format!("{key} ({})", f.kind.trim()) };
        let _ = write!(o, "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-size=\"11\">{}</text>", px((r.x0 + r.x1) / 2.0), py((r.y0 + r.y1) / 2.0) + 4.0, esc(&label));
        // Dimension to the west and south walls.
        if r.x0 > 0.0 {
            let _ = write!(o, "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"#a60\" stroke-width=\"0.8\"/><text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-size=\"10\" fill=\"#a60\">{}</text>", px(0.0), py((r.y0 + r.y1) / 2.0) - 8.0, px(r.x0), py((r.y0 + r.y1) / 2.0) - 8.0, px(r.x0 / 2.0), py((r.y0 + r.y1) / 2.0) - 11.0, r.x0.round());
        }
        if r.y0 > 0.0 {
            let _ = write!(o, "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"#a60\" stroke-width=\"0.8\"/><text x=\"{:.1}\" y=\"{:.1}\" font-size=\"10\" fill=\"#a60\">{}</text>", px((r.x0 + r.x1) / 2.0) + 8.0, py(0.0), px((r.x0 + r.x1) / 2.0) + 8.0, py(r.y0), px((r.x0 + r.x1) / 2.0) + 11.0, py(r.y0 / 2.0) + 4.0, r.y0.round());
        }
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
        for s in [space("bath", "bathroom", 2400, 1800), space("kit", "kitchen", 3600, 3000)] {
            let mut e = element_of(&s, Source::User);
            e.level_id = Some(lid);
            cmds.push(Command::Create(e));
        }
        d.commit("rooms", Origin::User, cmds).unwrap();
        d
    }

    fn fx(key: &str, x: f64, y: f64, w: f64, d: f64) -> Fixture {
        Fixture { key: key.into(), kind: "wc".into(), x_mm: x, y_mm: y, w_mm: w, d_mm: d, group: String::new(), source: "Maker data sheet 12, 2026-01-01".into() }
    }

    fn input() -> Input {
        Input {
            room: "bath".into(),
            fixtures: vec![fx("wc", 100.0, 100.0, 400.0, 700.0), fx("basin", 1000.0, 100.0, 500.0, 400.0)],
            doors: vec![DoorIn { wall: "south".into(), offset_mm: 1800.0, width_mm: 600.0, swing_mm: Some(600.0) }],
            min_clearance_mm: Some(300.0),
            clearance_source: "Project brief 4.2, 2026-01-02".into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_fitting_layout_passes_and_the_gaps_to_the_walls_are_measured() {
        let d = build(&doc().model, &input()).unwrap();
        assert_eq!((d.overall, d.level.as_str(), d.width_mm, d.depth_mm), ("passes", "Ground", 2400.0, 1800.0));
        assert!(d.issues.is_empty(), "{:?}", d.issues);
        let wc = &d.fixtures[0];
        assert_eq!((wc.to_west_mm, wc.to_east_mm, wc.to_south_mm, wc.to_north_mm, wc.area_m2), (100.0, 1900.0, 100.0, 1000.0, 0.28));
        assert!(d.svg.starts_with("<svg") && d.svg.contains("2400 mm") && d.svg.ends_with("</svg>"));
    }

    #[test]
    fn crossing_overlap_swing_and_clearance_problems_are_each_reported() {
        let mut i = input();
        i.fixtures = vec![fx("wc", -50.0, 100.0, 400.0, 700.0), fx("basin", 200.0, 300.0, 500.0, 400.0), fx("tub", 1900.0, 200.0, 400.0, 400.0), fx("bidet", 800.0, 100.0, 300.0, 300.0)];
        let d = build(&doc().model, &i).unwrap();
        let has = |c: &str| d.issues.iter().any(|x| x.check == c);
        assert!(has("fit") && has("overlap") && has("door_swing") && has("clearance"), "{:?}", d.issues);
        assert_eq!(d.overall, "fails");
        assert!(d.svg.contains("#b00"));
    }

    #[test]
    fn same_group_fixtures_may_touch_and_missing_checks_are_unchecked_not_passed() {
        let mut i = input();
        i.fixtures = vec![fx("a", 0.0, 0.0, 500.0, 600.0), fx("b", 500.0, 0.0, 500.0, 600.0)];
        i.fixtures[0].group = "run".into();
        i.fixtures[1].group = "run".into();
        i.min_clearance_mm = None;
        i.clearance_source.clear();
        let d = build(&doc().model, &i).unwrap();
        assert_eq!(d.overall, "incomplete");
        assert_eq!(d.checks.iter().filter(|c| c.status == "unchecked").map(|c| c.check).collect::<Vec<_>>(), ["clearance"]);
        let doors = std::mem::take(&mut i.doors);
        let d = build(&doc().model, &i).unwrap();
        assert_eq!(d.checks.iter().filter(|c| c.status == "unchecked").count(), 2);
        i.doors = doors;
        i.min_clearance_mm = Some(200.0);
        i.clearance_source = "Brief".into();
        assert_eq!(build(&doc().model, &i).unwrap().overall, "passes");
        i.fixtures[1].group.clear();
        assert_eq!(build(&doc().model, &i).unwrap().overall, "fails");
    }

    #[test]
    fn missing_or_malformed_inputs_are_named_and_unknown_rooms_refused() {
        let e = build(&doc().model, &Input::default()).unwrap_err();
        assert!(e.contains("room") && e.contains("fixtures"), "{e}");
        let mut i = input();
        i.fixtures[0].source = " ".into();
        i.fixtures[1].w_mm = 0.0;
        i.fixtures[1].key = "wc".into();
        i.doors[0].wall = "up".into();
        i.clearance_source.clear();
        let e = build(&doc().model, &i).unwrap_err();
        assert!(e.contains("source") && e.contains("w_mm and d_mm") && e.contains("duplicated") && e.contains("doors[0].wall") && e.contains("clearance_source"), "{e}");
        let mut i = input();
        i.room = "garage".into();
        assert!(build(&doc().model, &i).unwrap_err().contains("no room with key `garage`"));
        let mut i = input();
        i.doors[0].offset_mm = 2000.0;
        assert!(build(&doc().model, &i).unwrap_err().contains("runs past"));
    }

    #[test]
    fn text_is_escaped_in_the_drawing() {
        let mut i = input();
        i.title = "<script>x</script>".into();
        let d = build(&doc().model, &i).unwrap();
        assert!(!d.svg.contains("<script>") && d.svg.contains("&lt;script&gt;"));
    }
}
