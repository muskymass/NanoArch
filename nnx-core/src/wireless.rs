//! Wi-Fi coverage and exterior CCTV coverage, from your parameters, drawn in 2D and 3D.
//!
//! **Wi-Fi**: a log-distance model on a 500 mm grid over the rooms of one floor.
//! `signal = tx + gain - PL1m - 10 n log10(d) - sum of wall losses`, with `PL1m = 20 log10(f MHz) + 20 log10(0.001) + 32.44` (the
//! free-space formula at one metre: physics) and `d` at least 1 m. The exponent `n`, the loss of an interior and of an exterior wall,
//! the transmit power, the antenna gain and the acceptable threshold are YOURS; the walls an access-point-to-point segment crosses
//! are counted from the model (doors and windows are not subtracted: they only make the real signal better). Access points you place
//! are kept; more are suggested at room centres (greedy: the one that covers the most uncovered cells) until your target coverage is
//! reached or your maximum number of access points. Each floor is analysed alone: floors do not attenuate each other here.
//!
//! **CCTV**: each camera sees a sector (azimuth, field of view, range) on the ground outside the building, except behind walls (a
//! segment from the camera to the cell that crosses a wall hides it). The coverage of the line around the building at your offset is
//! reported, with the uncovered points. The mount height is drawn; it does not change the ground coverage (no tilt model).

use crate::mepx::{CctvIn, WifiIn, r2};
use crate::model::{Id, Model};
use crate::typed::{Space, Typed, Wall};
use serde::Serialize;

const CELL: f64 = 500.0;

type Pt = [f64; 2];

#[derive(Debug, Clone, Serialize)]
pub struct Ap {
    pub name: String,
    pub x_mm: f64,
    pub y_mm: f64,
    pub suggested: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoomCov {
    pub room: String,
    pub covered_pct: f64,
    pub weakest_dbm: f64,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Cell {
    pub x: f64,
    pub y: f64,
    pub dbm: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct WifiReport {
    pub floor: u32,
    pub aps: Vec<Ap>,
    pub coverage_pct: Option<f64>,
    pub target_met: Option<bool>,
    pub rooms: Vec<RoomCov>,
    pub path_loss_1m_db: Option<f64>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
    #[serde(skip)]
    pub cells: Vec<Cell>,
}

struct Geo {
    rooms: Vec<(String, [f64; 4])>,
    /// Wall centre lines with their thickness and whether they are exterior.
    walls: Vec<([f64; 4], f64, bool)>,
}

fn level_of(model: &Model, floor: u32) -> Result<Option<Id>, String> {
    let lv = crate::building::levels(model);
    if lv.is_empty() {
        return if floor <= 1 { Ok(None) } else { Err(format!("the model has no floor {floor}")) };
    }
    lv.get(floor.max(1) as usize - 1).map(|l| Some(l.1)).ok_or_else(|| format!("the model has no floor {floor}"))
}

fn geo(model: &Model, level: Option<Id>) -> Geo {
    let rooms = model.elements.values().filter(|e| e.kind == Space::KIND && (level.is_none() || e.level_id == level)).filter_map(|e| e.typed::<Space>().ok()).map(|s| (s.requirement_key, [s.x as f64, s.y as f64, (s.x + s.w) as f64, (s.y + s.h) as f64])).collect();
    let walls = model.elements.values().filter(|e| e.kind == Wall::KIND && (level.is_none() || e.level_id == level)).filter_map(|e| e.typed::<Wall>().ok()).map(|w| ([w.x1 as f64, w.y1 as f64, w.x2 as f64, w.y2 as f64], w.thickness as f64, w.is_exterior)).collect();
    Geo { rooms, walls }
}

fn ccw(a: Pt, b: Pt, c: Pt) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn seg_cross(p: Pt, q: Pt, a: Pt, b: Pt) -> bool {
    let (d1, d2, d3, d4) = (ccw(a, b, p), ccw(a, b, q), ccw(p, q, a), ccw(p, q, b));
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

/// A segment crosses a wall's body (a rectangle of its thickness along the centre line).
fn seg_hits_wall(p: Pt, q: Pt, w: &[f64; 4], t: f64) -> bool {
    let (dx, dy) = (w[2] - w[0], w[3] - w[1]);
    let len = dx.hypot(dy).max(1.0);
    let (nx, ny) = (-dy / len * t / 2.0, dx / len * t / 2.0);
    let c = [[w[0] + nx, w[1] + ny], [w[2] + nx, w[3] + ny], [w[2] - nx, w[3] - ny], [w[0] - nx, w[1] - ny]];
    let inside = |pt: Pt| (0..4).all(|i| ccw(c[i], c[(i + 1) % 4], pt) >= -1e-6) || (0..4).all(|i| ccw(c[i], c[(i + 1) % 4], pt) <= 1e-6);
    inside(p) || inside(q) || (0..4).any(|i| seg_cross(p, q, c[i], c[(i + 1) % 4]))
}

fn cells_of(g: &Geo) -> Vec<(usize, Pt)> {
    let mut v = Vec::new();
    for (i, (_, r)) in g.rooms.iter().enumerate() {
        let (nx, ny) = (((r[2] - r[0]) / CELL).ceil().max(1.0), ((r[3] - r[1]) / CELL).ceil().max(1.0));
        let (sx, sy) = ((r[2] - r[0]) / nx, (r[3] - r[1]) / ny);
        let mut y = r[1] + sy / 2.0;
        for _ in 0..ny as usize {
            let mut x = r[0] + sx / 2.0;
            for _ in 0..nx as usize {
                v.push((i, [x, y]));
                x += sx;
            }
            y += sy;
        }
    }
    v
}

pub fn wifi(model: &Model, inp: &WifiIn) -> WifiReport {
    let floor = inp.floor.unwrap_or(1);
    let mut rep = WifiReport { floor, ..Default::default() };
    for (n, ok) in [("frequency band (GHz)", inp.band_ghz.is_some()), ("transmit power (dBm)", inp.tx_power_dbm.is_some()), ("path-loss exponent", inp.exponent.is_some()), ("interior wall loss (dB)", inp.interior_wall_db.is_some()), ("exterior wall loss (dB)", inp.exterior_wall_db.is_some()), ("signal threshold (dBm)", inp.threshold_dbm.is_some()), ("target coverage (%)", inp.target_coverage_pct.is_some()), ("maximum access points", inp.max_aps.is_some())] {
        if !ok {
            rep.missing.push(n.to_string());
        }
    }
    let (Some(f), Some(tx), Some(n), Some(wi), Some(we), Some(th), Some(target), Some(maxap)) = (inp.band_ghz, inp.tx_power_dbm, inp.exponent, inp.interior_wall_db, inp.exterior_wall_db, inp.threshold_dbm, inp.target_coverage_pct, inp.max_aps) else { return rep };
    let level = match level_of(model, floor) {
        Ok(l) => l,
        Err(e) => {
            rep.notes.push(e);
            return rep;
        }
    };
    let g = geo(model, level);
    let cells = cells_of(&g);
    if cells.is_empty() {
        rep.notes.push("no rooms on this floor".into());
        return rep;
    }
    let pl1 = 20.0 * (f * 1000.0).log10() - 27.56;
    rep.path_loss_1m_db = Some(r2(pl1));
    let gain = inp.antenna_gain_dbi.unwrap_or(0.0);
    if inp.antenna_gain_dbi.is_none() {
        rep.notes.push("no antenna gain entered: 0 dBi".into());
    }
    let signal = |ap: Pt, p: Pt| -> f64 {
        let d = ((p[0] - ap[0]).hypot(p[1] - ap[1]) / 1000.0).max(1.0);
        let walls: f64 = g.walls.iter().filter(|(w, t, _)| seg_hits_wall(ap, p, w, *t) && !seg_hits_wall(ap, ap, w, *t)).map(|(_, _, ext)| if *ext { we } else { wi }).sum();
        tx + gain - pl1 - 10.0 * n * d.log10() - walls
    };
    let mut aps: Vec<Ap> = inp.aps.iter().filter(|a| a.floor.unwrap_or(1) == floor).map(|a| Ap { name: a.name.clone(), x_mm: a.x_mm, y_mm: a.y_mm, suggested: false }).collect();
    let mut best: Vec<f64> = vec![f64::NEG_INFINITY; cells.len()];
    for a in &aps {
        for (k, (_, p)) in cells.iter().enumerate() {
            best[k] = best[k].max(signal([a.x_mm, a.y_mm], *p));
        }
    }
    let covered = |b: &[f64]| b.iter().filter(|v| **v >= th).count();
    // Candidates: the centre of every room; signals cached per candidate.
    let cand: Vec<(Pt, Vec<f64>)> = g.rooms.iter().map(|(_, r)| [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0]).map(|c| (c, cells.iter().map(|(_, p)| signal(c, *p)).collect())).collect();
    while (covered(&best) as f64 / cells.len() as f64 * 100.0) < target && (aps.len() as f64) < maxap {
        let pick = cand.iter().enumerate().filter(|(_, (c, _))| !aps.iter().any(|a| (a.x_mm - c[0]).abs() < 1.0 && (a.y_mm - c[1]).abs() < 1.0)).max_by_key(|(_, (_, s))| (0..cells.len()).filter(|&k| best[k] < th && s[k] >= th).count());
        let Some((i, (c, s))) = pick else { break };
        if (0..cells.len()).all(|k| !(best[k] < th && s[k] >= th)) {
            break;
        }
        for k in 0..cells.len() {
            best[k] = best[k].max(s[k]);
        }
        aps.push(Ap { name: format!("AP-{}", aps.len() + 1), x_mm: c[0].round(), y_mm: c[1].round(), suggested: true });
        let _ = i;
    }
    let pct = covered(&best) as f64 / cells.len() as f64 * 100.0;
    rep.coverage_pct = Some(r2(pct));
    rep.target_met = Some(pct + 1e-9 >= target);
    for (i, (key, _)) in g.rooms.iter().enumerate() {
        let mine: Vec<f64> = cells.iter().enumerate().filter(|(_, (r, _))| *r == i).map(|(k, _)| best[k]).collect();
        let ok = mine.iter().filter(|v| **v >= th).count();
        rep.rooms.push(RoomCov { room: key.clone(), covered_pct: r2(ok as f64 / mine.len().max(1) as f64 * 100.0), weakest_dbm: r2(mine.iter().cloned().fold(f64::INFINITY, f64::min)) });
    }
    rep.rooms.sort_by(|a, b| a.covered_pct.total_cmp(&b.covered_pct).then(a.room.cmp(&b.room)));
    if rep.target_met == Some(false) {
        rep.notes.push(format!("the target {target}% is not reached with {} access point(s): raise the maximum, add one by hand in the weakest room, or reduce the wall losses", aps.len()));
    }
    if aps.is_empty() {
        rep.notes.push("no access point yet".into());
    }
    rep.notes.push("doors and windows are not subtracted from the wall losses; floors do not attenuate each other; a preliminary model, not a site survey".into());
    rep.aps = aps;
    rep.cells = cells.iter().enumerate().map(|(k, (_, p))| Cell { x: p[0], y: p[1], dbm: r2(best[k]) }).collect();
    rep
}

// ------------------------------------------------------------------ CCTV

#[derive(Debug, Clone, Serialize)]
pub struct CamRow {
    pub name: String,
    pub covered_m2: f64,
    pub perimeter_pct: f64,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct SeenCell {
    pub x: f64,
    pub y: f64,
    pub camera: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CctvReport {
    pub cameras: Vec<CamRow>,
    pub covered_m2: f64,
    pub perimeter_pct: Option<f64>,
    pub perimeter_points: usize,
    pub blind_spots: Vec<[f64; 2]>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
    #[serde(skip)]
    pub cells: Vec<SeenCell>,
}

fn sees(cam: &crate::mepx::Camera, from: Pt, p: Pt, walls: &[([f64; 4], f64, bool)]) -> bool {
    let (dx, dy) = (p[0] - cam.x_mm, p[1] - cam.y_mm);
    let d = dx.hypot(dy);
    if d > cam.range_m * 1000.0 || d < 1.0 {
        return false;
    }
    // Compass bearing: clockwise from north (+y).
    let bearing = dx.atan2(dy).to_degrees();
    let diff = ((bearing - cam.azimuth_deg + 540.0) % 360.0) - 180.0;
    diff.abs() <= cam.fov_deg / 2.0 + 1e-9 && !walls.iter().any(|(w, t, _)| seg_hits_wall(from, p, w, *t))
}

pub fn cctv(model: &Model, inp: &CctvIn) -> CctvReport {
    let mut rep = CctvReport::default();
    if inp.cameras.is_empty() {
        rep.notes.push("no camera yet".into());
        return rep;
    }
    let g = geo(model, None);
    if g.rooms.is_empty() {
        rep.missing.push("rooms (the building the cameras look around)".into());
        return rep;
    }
    let bb = g.rooms.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |a, (_, r)| [a[0].min(r[0]), a[1].min(r[1]), a[2].max(r[2]), a[3].max(r[3])]);
    let thick = g.walls.iter().map(|w| w.1).fold(0.0, f64::max);
    let outer = [bb[0] - thick / 2.0, bb[1] - thick / 2.0, bb[2] + thick / 2.0, bb[3] + thick / 2.0];
    let inside = |p: Pt| g.rooms.iter().any(|(_, r)| p[0] >= r[0] - thick / 2.0 && p[0] <= r[2] + thick / 2.0 && p[1] >= r[1] - thick / 2.0 && p[1] <= r[3] + thick / 2.0);
    let rmax = inp.cameras.iter().map(|c| c.range_m * 1000.0).fold(0.0, f64::max);
    let froms: Vec<Pt> = inp.cameras.iter().map(|c| {
        // A camera on a wall looks from just outside it.
        let a = c.azimuth_deg.to_radians();
        [c.x_mm + a.sin() * (thick / 2.0 + 50.0), c.y_mm + a.cos() * (thick / 2.0 + 50.0)]
    }).collect();
    let (x0, y0, x1, y1) = (bb[0] - rmax, bb[1] - rmax, bb[2] + rmax, bb[3] + rmax);
    let mut seen_any: std::collections::HashSet<(i64, i64)> = Default::default();
    let mut per_cam = vec![0usize; inp.cameras.len()];
    let mut y = y0;
    while y <= y1 {
        let mut x = x0;
        while x <= x1 {
            let p = [x, y];
            if !inside(p) {
                for (i, c) in inp.cameras.iter().enumerate() {
                    if sees(c, froms[i], p, &g.walls) {
                        per_cam[i] += 1;
                        seen_any.insert(((x / CELL) as i64, (y / CELL) as i64));
                        rep.cells.push(SeenCell { x, y, camera: i });
                    }
                }
            }
            x += CELL;
        }
        y += CELL;
    }
    rep.covered_m2 = r2(seen_any.len() as f64 * CELL * CELL / 1e6);
    // The line around the building.
    let off = inp.perimeter_offset_mm;
    if off.is_none() {
        rep.missing.push("perimeter offset (mm): how far out the line around the building is".into());
    }
    let mut cam_cover = vec![0usize; inp.cameras.len()];
    if let Some(o) = off {
        let r = [outer[0] - o, outer[1] - o, outer[2] + o, outer[3] + o];
        let mut pts: Vec<Pt> = Vec::new();
        let mut t = 0.0;
        while t <= r[2] - r[0] {
            pts.push([r[0] + t, r[1]]);
            pts.push([r[0] + t, r[3]]);
            t += CELL;
        }
        t = CELL;
        while t < r[3] - r[1] {
            pts.push([r[0], r[1] + t]);
            pts.push([r[2], r[1] + t]);
            t += CELL;
        }
        rep.perimeter_points = pts.len();
        let mut ok = 0;
        for p in &pts {
            let mut any = false;
            for (i, c) in inp.cameras.iter().enumerate() {
                if sees(c, froms[i], *p, &g.walls) {
                    cam_cover[i] += 1;
                    any = true;
                }
            }
            if any {
                ok += 1;
            } else if rep.blind_spots.len() < 200 {
                rep.blind_spots.push([p[0].round(), p[1].round()]);
            }
        }
        rep.perimeter_pct = Some(r2(ok as f64 / pts.len().max(1) as f64 * 100.0));
    }
    for (i, c) in inp.cameras.iter().enumerate() {
        rep.cameras.push(CamRow { name: c.name.clone(), covered_m2: r2(per_cam[i] as f64 * CELL * CELL / 1e6), perimeter_pct: if rep.perimeter_points > 0 { r2(cam_cover[i] as f64 / rep.perimeter_points as f64 * 100.0) } else { 0.0 } });
    }
    rep.notes.push("coverage is the ground seen from each camera's position, hidden behind walls; the mount height is drawn but does not change the coverage (no tilt or lens model)".into());
    rep
}

// ------------------------------------------------------------------ drawings

/// 3D solids: the cells the cameras see (thin slabs on the ground) and the camera bodies.
pub fn cctv_boxes(model: &Model, inp: &CctvIn) -> Vec<crate::model3d::Box3> {
    let rep = cctv(model, inp);
    let mut out = Vec::new();
    for (n, c) in rep.cells.iter().enumerate() {
        out.push(crate::model3d::Box3 { id: format!("cctv:{}", n + 1), kind: "Coverage", class: "cctv".into(), min: [(c.x - CELL / 2.0) / 1000.0, (c.y - CELL / 2.0) / 1000.0, 0.0], max: [(c.x + CELL / 2.0) / 1000.0, (c.y + CELL / 2.0) / 1000.0, 0.03], visual: true, level: None, discipline: "architectural" });
    }
    for (i, c) in inp.cameras.iter().enumerate() {
        out.push(crate::model3d::Box3 { id: format!("cam:{}", i + 1), kind: "Coverage", class: "camera".into(), min: [(c.x_mm - 100.0) / 1000.0, (c.y_mm - 100.0) / 1000.0, (c.mount_height_mm - 100.0) / 1000.0], max: [(c.x_mm + 100.0) / 1000.0, (c.y_mm + 100.0) / 1000.0, (c.mount_height_mm + 100.0) / 1000.0], visual: true, level: None, discipline: "architectural" });
    }
    out
}

/// 3D solids for the access points the user placed (small boxes at the ceiling of the floor they are on).
pub fn ap_boxes(model: &Model, inp: &WifiIn) -> Vec<crate::model3d::Box3> {
    let scene = crate::model3d::build(model);
    inp.aps
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let z0 = scene.levels.get(a.floor.unwrap_or(1).max(1) as usize - 1).map_or(0.0, |l| l.z0);
            let top = scene.levels.get(a.floor.unwrap_or(1).max(1) as usize - 1).map_or(scene.storey_height, |l| l.storey);
            crate::model3d::Box3 { id: format!("ap:{}", i + 1), kind: "Coverage", class: "ap".into(), min: [(a.x_mm - 120.0) / 1000.0, (a.y_mm - 120.0) / 1000.0, z0 + top - 0.25], max: [(a.x_mm + 120.0) / 1000.0, (a.y_mm + 120.0) / 1000.0, z0 + top - 0.1], visual: true, level: None, discipline: "architectural" }
        })
        .collect()
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The network plan: rooms and walls, the Wi-Fi signal cells and access points, the CCTV sectors and seen cells, the smart devices and bus.
pub fn network_svg(model: &Model, floor: u32, wifi: Option<&WifiReport>, cctv_in: Option<(&CctvIn, &CctvReport)>, smart: Option<(&crate::mepx::SmartIn, &crate::smarthome::SmartReport)>, threshold_dbm: Option<f64>) -> Result<String, String> {
    use std::fmt::Write;
    let level = level_of(model, floor)?;
    let g = geo(model, level);
    if g.rooms.is_empty() {
        return Err("no rooms on this floor".into());
    }
    let mut bb = g.rooms.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |a, (_, r)| [a[0].min(r[0]), a[1].min(r[1]), a[2].max(r[2]), a[3].max(r[3])]);
    if let Some((c, _)) = cctv_in {
        for cam in &c.cameras {
            let r = cam.range_m * 1000.0;
            bb = [bb[0].min(cam.x_mm - r), bb[1].min(cam.y_mm - r), bb[2].max(cam.x_mm + r), bb[3].max(cam.y_mm + r)];
        }
    }
    let (s, m) = (0.04f64.min(900.0 / (bb[2] - bb[0]).max(bb[3] - bb[1])), 30.0);
    let (w, h) = ((bb[2] - bb[0]) * s + 2.0 * m, (bb[3] - bb[1]) * s + 2.0 * m + 24.0);
    let tx = |x: f64| m + (x - bb[0]) * s;
    let ty = |y: f64| m + (bb[3] - y) * s;
    let mut o = String::new();
    let _ = writeln!(o, "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w:.0} {h:.0}\" width=\"{w:.0}\" height=\"{h:.0}\" font-family=\"Segoe UI, Arial, sans-serif\"><rect width=\"100%\" height=\"100%\" fill=\"#fff\"/>");
    if let Some(wf) = wifi {
        let th = threshold_dbm.unwrap_or(-70.0);
        for c in &wf.cells {
            let margin = c.dbm - th;
            let col = if margin >= 20.0 { "#66bb6a" } else if margin >= 10.0 { "#c0ca33" } else if margin >= 0.0 { "#ffb74d" } else { "#ef5350" };
            let _ = writeln!(o, "<rect class=\"wifi\" x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" style=\"fill:{col};fill-opacity:.55\"><title>{} dBm</title></rect>", tx(c.x - CELL / 2.0), ty(c.y + CELL / 2.0), CELL * s, CELL * s, c.dbm);
        }
    }
    for (key, r) in &g.rooms {
        let _ = writeln!(o, "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" style=\"fill:none;stroke:#999;stroke-width:1\"/><text x=\"{:.1}\" y=\"{:.1}\" style=\"font:10px sans-serif;fill:#666\">{}</text>", tx(r[0]), ty(r[3]), (r[2] - r[0]) * s, (r[3] - r[1]) * s, tx(r[0]) + 3.0, ty(r[3]) + 11.0, esc(key));
    }
    for (wl, t, ext) in &g.walls {
        let _ = writeln!(o, "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" style=\"stroke:{};stroke-width:{:.1}\"/>", tx(wl[0]), ty(wl[1]), tx(wl[2]), ty(wl[3]), if *ext { "#212121" } else { "#616161" }, (t * s).max(1.5));
    }
    if let Some((c, rep)) = cctv_in {
        for sc in &rep.cells {
            let _ = writeln!(o, "<rect class=\"cctv\" x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" style=\"fill:#e53935;fill-opacity:.22\"/>", tx(sc.x - CELL / 2.0), ty(sc.y + CELL / 2.0), CELL * s, CELL * s);
        }
        for cam in &c.cameras {
            let (a0, a1) = ((cam.azimuth_deg - cam.fov_deg / 2.0).to_radians(), (cam.azimuth_deg + cam.fov_deg / 2.0).to_radians());
            let r = cam.range_m * 1000.0;
            let p = |a: f64| (tx(cam.x_mm + a.sin() * r), ty(cam.y_mm + a.cos() * r));
            let ((x0, y0), (x1, y1)) = (p(a0), p(a1));
            let _ = writeln!(o, "<path class=\"cctv-cone\" d=\"M{:.1},{:.1} L{:.1},{:.1} A{:.1},{:.1} 0 {} 1 {:.1},{:.1} Z\" style=\"fill:none;stroke:#e53935;stroke-width:1;stroke-dasharray:4 3\"/><circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"4\" style=\"fill:#b71c1c\"/><text x=\"{:.1}\" y=\"{:.1}\" style=\"font:10px sans-serif;fill:#b71c1c\">{}</text>", tx(cam.x_mm), ty(cam.y_mm), x0, y0, r * s, r * s, if cam.fov_deg > 180.0 { 1 } else { 0 }, x1, y1, tx(cam.x_mm), ty(cam.y_mm), tx(cam.x_mm) + 6.0, ty(cam.y_mm) - 6.0, esc(&cam.name));
        }
        for b in &rep.blind_spots {
            let _ = writeln!(o, "<circle class=\"blind\" cx=\"{:.1}\" cy=\"{:.1}\" r=\"2\" style=\"fill:#000\"/>", tx(b[0]), ty(b[1]));
        }
    }
    if let Some((_, sm)) = smart {
        for l in &sm.lines {
            let pts: Vec<String> = {
                let mut v = Vec::new();
                for wdw in l.points.windows(2) {
                    v.push(format!("{:.1},{:.1}", tx(wdw[0][0] as f64), ty(wdw[0][1] as f64)));
                    v.push(format!("{:.1},{:.1}", tx(wdw[1][0] as f64), ty(wdw[0][1] as f64)));
                    v.push(format!("{:.1},{:.1}", tx(wdw[1][0] as f64), ty(wdw[1][1] as f64)));
                }
                v
            };
            let _ = writeln!(o, "<polyline class=\"bus\" points=\"{}\" style=\"fill:none;stroke:#6a1b9a;stroke-width:1.5\"/>", pts.join(" "));
        }
        if let Some(p) = sm.panel {
            let _ = writeln!(o, "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"12\" height=\"12\" style=\"fill:#6a1b9a\"/><text x=\"{:.1}\" y=\"{:.1}\" style=\"font:10px sans-serif;fill:#6a1b9a\">panel</text>", tx(p[0] as f64) - 6.0, ty(p[1] as f64) - 6.0, tx(p[0] as f64) + 8.0, ty(p[1] as f64) + 4.0);
        }
    }
    if let Some(wf) = wifi {
        for a in &wf.aps {
            let _ = writeln!(o, "<circle class=\"ap\" cx=\"{:.1}\" cy=\"{:.1}\" r=\"6\" style=\"fill:#1565c0;stroke:#fff;stroke-width:1.5\"/><text x=\"{:.1}\" y=\"{:.1}\" style=\"font:10px sans-serif;fill:#0d47a1\">{}{}</text>", tx(a.x_mm), ty(a.y_mm), tx(a.x_mm) + 8.0, ty(a.y_mm) - 6.0, esc(&a.name), if a.suggested { " (suggested)" } else { "" });
        }
    }
    let _ = writeln!(o, "<text x=\"{m}\" y=\"{:.0}\" style=\"font:11px sans-serif;fill:#333\">floor {floor}{}{}</text></svg>", h - 8.0, wifi.and_then(|w| w.coverage_pct).map_or(String::new(), |p| format!(" · Wi-Fi coverage {p}%")), cctv_in.and_then(|(_, r)| r.perimeter_pct).map_or(String::new(), |p| format!(" · CCTV perimeter {p}%")));
    Ok(o)
}
