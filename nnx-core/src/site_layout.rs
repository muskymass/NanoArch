//! Site and landscape layout inside the plot: boundary walls with a gate, a guard room, a shaded carport, a swimming pool and a
//! courtyard, placed in the part of the plot the building does not use (which includes the setbacks).
//!
//! Every size is a user input stored in the project (`meta.siteLayout`): the application has no statutory size for any of them,
//! and a missing size is reported by name, not defaulted. The positions are a design heuristic (a grid search, scored by the
//! distance to the gate): they do NOT mean the municipality allows a structure inside a setback; that is not verified here and
//! the result says so.
//!
//! Frames: the plot frame of `site::SiteResult` (north up, metres) and the building frame (x along the building's front, y along
//! its side, origin at its south-west corner: the model's own frame). `Frame` converts between them.

use crate::site::{self, EdgeKind, Pt, SiteResult};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Footprint {
    pub width_m: f64,
    pub length_m: f64,
    /// Roof or top height; zones (pool, courtyard) may leave it out.
    pub height_m: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SiteLayoutInput {
    pub wall_height_m: Option<f64>,
    pub wall_thickness_m: Option<f64>,
    pub gate_width_m: Option<f64>,
    pub guard: Option<Footprint>,
    pub carport: Option<Footprint>,
    pub pool: Option<Footprint>,
    pub courtyard: Option<Footprint>,
}

impl SiteLayoutInput {
    /// What is wrong with the numbers themselves.
    pub fn check(&self) -> Result<(), String> {
        let pos = |n: &str, v: Option<f64>| match v {
            Some(x) if !(x.is_finite() && x > 0.0) => Err(format!("{n} must be a positive number of metres")),
            _ => Ok(()),
        };
        pos("wall height", self.wall_height_m)?;
        pos("wall thickness", self.wall_thickness_m)?;
        pos("gate width", self.gate_width_m)?;
        for (n, f) in [("guard room", &self.guard), ("carport", &self.carport), ("pool", &self.pool), ("courtyard", &self.courtyard)] {
            if let Some(f) = f {
                pos(&format!("{n} width"), Some(f.width_m))?;
                pos(&format!("{n} length"), Some(f.length_m))?;
                pos(&format!("{n} height"), f.height_m)?;
            }
        }
        Ok(())
    }
}

/// Conversion between the plot frame and the building frame of a site analysis.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    o: Pt,
    ex: Pt,
    ey: Pt,
    pub width_m: f64,
    pub length_m: f64,
}

impl Frame {
    pub fn of(r: &SiteResult) -> Frame {
        let c = &r.building.corners_m;
        let (w, l) = (r.building.width_m, r.building.length_m);
        Frame { o: c[0], ex: [(c[1][0] - c[0][0]) / w, (c[1][1] - c[0][1]) / w], ey: [(c[2][0] - c[1][0]) / l, (c[2][1] - c[1][1]) / l], width_m: w, length_m: l }
    }
    pub fn to_building(&self, p: Pt) -> Pt {
        let d = [p[0] - self.o[0], p[1] - self.o[1]];
        [d[0] * self.ex[0] + d[1] * self.ex[1], d[0] * self.ey[0] + d[1] * self.ey[1]]
    }
    pub fn to_plot(&self, p: Pt) -> Pt {
        [self.o[0] + p[0] * self.ex[0] + p[1] * self.ey[0], self.o[1] + p[0] * self.ex[1] + p[1] * self.ey[1]]
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SiteItem {
    /// `guard`, `carport`, `pool` or `courtyard`.
    pub kind: &'static str,
    /// `[x0, y0, x1, y1]` in the building frame.
    pub rect_b: [f64; 4],
    pub corners_plot: [Pt; 4],
    pub height_m: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SiteWall {
    /// End points in the plot frame; the wall is `thickness_m` thick and `height_m` high.
    pub a: Pt,
    pub b: Pt,
}

#[derive(Debug, Clone, Serialize)]
pub struct SiteLayout {
    pub items: Vec<SiteItem>,
    pub walls: Vec<SiteWall>,
    pub wall_height_m: f64,
    pub wall_thickness_m: f64,
    pub gate_m: Option<[Pt; 2]>,
    pub warnings: Vec<String>,
}

fn rect_inside_poly(poly: &[Pt], r: [f64; 4]) -> bool {
    let pts = [[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]], [(r[0] + r[2]) / 2.0, r[1]], [(r[0] + r[2]) / 2.0, r[3]], [r[0], (r[1] + r[3]) / 2.0], [r[2], (r[1] + r[3]) / 2.0]];
    if !pts.iter().all(|p| site::contains(poly, *p)) {
        return false;
    }
    // No outline vertex strictly inside the rectangle (a reflex corner poking in).
    !poly.iter().any(|v| v[0] > r[0] + 1e-6 && v[0] < r[2] - 1e-6 && v[1] > r[1] + 1e-6 && v[1] < r[3] - 1e-6)
}

fn overlaps(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] < b[2] - 1e-9 && b[0] < a[2] - 1e-9 && a[1] < b[3] - 1e-9 && b[1] < a[3] - 1e-9
}

/// Places what the input asks for. The walls follow the outline; the gate is in the middle of the first front-street edge.
pub fn layout(r: &SiteResult, input: &SiteLayoutInput) -> Result<SiteLayout, String> {
    input.check()?;
    let f = Frame::of(r);
    let poly: Vec<Pt> = r.polygon_m.iter().map(|p| f.to_building(*p)).collect();
    let n = poly.len();
    let mut warnings = vec!["structures inside the setbacks are not checked against the municipality's rules: confirm what it allows".to_string()];
    // The gate: the middle of the first front-street edge.
    let front = r.edges.iter().position(|e| e.kind == EdgeKind::FrontStreet);
    let gate_mid = front.map(|i| {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
    });
    // Walls.
    let mut walls = Vec::new();
    let mut gate_m = None;
    let (wh, wt) = (input.wall_height_m, input.wall_thickness_m);
    if wh.is_some() != wt.is_some() {
        warnings.push(format!("boundary walls need both the wall {}: none drawn", if wh.is_none() { "height" } else { "thickness" }));
    }
    if let (Some(_), Some(_)) = (wh, wt) {
        for i in 0..n {
            let (a, b) = (r.polygon_m[i], r.polygon_m[(i + 1) % n]);
            let len = (b[0] - a[0]).hypot(b[1] - a[1]);
            if Some(i) == front {
                let Some(g) = input.gate_width_m else {
                    warnings.push("the gate width is missing: the front wall is drawn without a gate".into());
                    walls.push(SiteWall { a, b });
                    continue;
                };
                if g >= len {
                    return Err(format!("the gate ({g} m) is as wide as the front edge ({len:.2} m)"));
                }
                let (u, h) = ([(b[0] - a[0]) / len, (b[1] - a[1]) / len], g / 2.0);
                let m = len / 2.0;
                let (g0, g1) = ([a[0] + u[0] * (m - h), a[1] + u[1] * (m - h)], [a[0] + u[0] * (m + h), a[1] + u[1] * (m + h)]);
                walls.push(SiteWall { a, b: g0 });
                walls.push(SiteWall { a: g1, b });
                gate_m = Some([g0, g1]);
            } else {
                walls.push(SiteWall { a, b });
            }
        }
    } else if wh.is_none() && wt.is_none() {
        warnings.push("no boundary wall height and thickness entered: no boundary walls".into());
    }
    // Items, placed one after another so none overlaps another or the building.
    let bld = [0.0, 0.0, f.width_m, f.length_m];
    let (minx, miny) = (poly.iter().map(|p| p[0]).fold(f64::MAX, f64::min), poly.iter().map(|p| p[1]).fold(f64::MAX, f64::min));
    let (maxx, maxy) = (poly.iter().map(|p| p[0]).fold(f64::MIN, f64::max), poly.iter().map(|p| p[1]).fold(f64::MIN, f64::max));
    let gate = gate_mid.unwrap_or([(minx + maxx) / 2.0, miny]);
    // The wall is centred on the boundary, so its inner face is half a thickness inside it.
    let clear = wt.unwrap_or(0.0) / 2.0;
    let mut placed: Vec<[f64; 4]> = Vec::new();
    let mut items = Vec::new();
    for (kind, fp, near_gate) in [("guard", &input.guard, true), ("carport", &input.carport, true), ("pool", &input.pool, false), ("courtyard", &input.courtyard, true)] {
        let Some(fp) = fp else { continue };
        let mut best: Option<(f64, [f64; 4])> = None;
        for (w, l) in [(fp.width_m, fp.length_m), (fp.length_m, fp.width_m)] {
            let mut x = minx;
            while x + w <= maxx + 1e-9 {
                let mut y = miny;
                while y + l <= maxy + 1e-9 {
                    let cand = [x, y, x + w, y + l];
                    let inner = [x - clear, y - clear, x + w + clear, y + l + clear];
                    if rect_inside_poly(&poly, cand) && rect_inside_poly(&poly, inner) && !overlaps(cand, bld) && !placed.iter().any(|p| overlaps(cand, *p)) {
                        let (cx, cy) = (x + w / 2.0, y + l / 2.0);
                        let d = (cx - gate[0]).hypot(cy - gate[1]);
                        // Near the gate: the smaller distance wins. Pool: the farthest from the gate wins.
                        let score = if near_gate { -d } else { d };
                        if best.is_none_or(|b| score > b.0 + 1e-9) {
                            best = Some((score, cand));
                        }
                    }
                    y += 0.25;
                }
                x += 0.25;
            }
        }
        match best {
            Some((_, c)) => {
                placed.push(c);
                items.push(SiteItem { kind, rect_b: c, corners_plot: [f.to_plot([c[0], c[1]]), f.to_plot([c[2], c[1]]), f.to_plot([c[2], c[3]]), f.to_plot([c[0], c[3]])], height_m: fp.height_m.unwrap_or(0.0) });
            }
            None => warnings.push(format!("no room for the {kind} ({} x {} m) outside the building: reduce it or the building", fp.width_m, fp.length_m)),
        }
    }
    Ok(SiteLayout { items, walls, wall_height_m: wh.unwrap_or(0.0), wall_thickness_m: wt.unwrap_or(0.0), gate_m, warnings })
}

/// Axis-aligned solids of the layout in the building frame, `(kind, [x0, y0, x1, y1], z0, z1)`: the model's 3D scene draws boxes.
/// A wall that is not axis-aligned is drawn as small steps one thickness long.
pub fn solids(r: &SiteResult, l: &SiteLayout) -> Vec<(&'static str, [f64; 4], f64, f64)> {
    let f = Frame::of(r);
    let mut out = Vec::new();
    for i in &l.items {
        let (z1, kind) = (if i.height_m > 0.0 { i.height_m } else { 0.05 }, i.kind);
        out.push((kind, i.rect_b, 0.0, z1));
    }
    let t = l.wall_thickness_m;
    for w in &l.walls {
        let (a, b) = (f.to_building(w.a), f.to_building(w.b));
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        if len < 1e-9 {
            continue;
        }
        if (a[0] - b[0]).abs() < 1e-6 {
            out.push(("wall", [a[0] - t / 2.0, a[1].min(b[1]), a[0] + t / 2.0, a[1].max(b[1])], 0.0, l.wall_height_m));
        } else if (a[1] - b[1]).abs() < 1e-6 {
            out.push(("wall", [a[0].min(b[0]), a[1] - t / 2.0, a[0].max(b[0]), a[1] + t / 2.0], 0.0, l.wall_height_m));
        } else {
            let steps = (len / t).ceil().max(1.0) as usize;
            for k in 0..steps {
                let u = (k as f64 + 0.5) / steps as f64;
                let c = [a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u];
                out.push(("wall", [c[0] - t / 2.0, c[1] - t / 2.0, c[0] + t / 2.0, c[1] + t / 2.0], 0.0, l.wall_height_m));
            }
        }
    }
    out
}
