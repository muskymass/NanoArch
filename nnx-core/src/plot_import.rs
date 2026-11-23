//! Plot boundary import from a DXF drawing (ASCII DXF, any AutoCAD release that writes `LWPOLYLINE`, `POLYLINE` or `LINE`).
//!
//! - **Boundaries.** A boundary is a closed `LWPOLYLINE` or `POLYLINE` (closed flag, or first vertex equal to the last), or a
//!   set of `LINE` entities that join end to end into one closed loop. Arc segments (bulges) are tessellated every 5 degrees.
//!   Only the `ENTITIES` section is read: block definitions are not boundaries.
//! - **Units.** Never assumed. The drawing's `$INSUNITS` is used when it is set; a drawing without one (or with 0, unitless)
//!   is refused until the caller states the unit. A stated unit that differs from the header's wins and is reported.
//! - **Choice.** A drawing usually has more than one closed shape (plot, building, title block). Nothing is picked silently:
//!   with exactly one usable boundary it is selected, otherwise the candidates are listed and the caller picks one by index.
//! - **DWG.** The binary DWG format has no open specification; reading it needs the licensed Open Design Alliance
//!   libraries. A DWG is detected and refused with the instruction to save it as DXF.
//!
//! Output coordinates are metres, moved so that the lowest x and y of the chosen boundary are 0; the move is reported.

use crate::site::{Pt, is_simple, signed_area};
use serde::Serialize;

/// Segments of a bulge arc are cut at most this many degrees apart.
const ARC_STEP_DEG: f64 = 5.0;

#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub layer: String,
    /// `LWPOLYLINE`, `POLYLINE` or `LINE chain`.
    pub source: String,
    /// Metres, counter-clockwise. The chosen boundary is moved to the origin (see `ImportResult::origin_offset_m`); the others keep the drawing's coordinates.
    pub polygon_m: Vec<Pt>,
    pub area_m2: f64,
    pub perimeter_m: f64,
    /// Why it cannot be used as a plot, if so.
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportResult {
    pub unit: String,
    pub unit_source: String,
    pub candidates: Vec<Candidate>,
    /// Index into `candidates` of the chosen boundary.
    pub selected: Option<usize>,
    /// What was added to the drawing's coordinates (metres) to put the chosen boundary at the origin.
    pub origin_offset_m: Option<Pt>,
    pub warnings: Vec<String>,
}

struct Raw {
    layer: String,
    source: &'static str,
    pts: Vec<Pt>,
}

fn unit_factor(name: &str) -> Option<f64> {
    match name.trim().to_ascii_lowercase().as_str() {
        "m" | "meter" | "metre" | "meters" | "metres" => Some(1.0),
        "cm" => Some(0.01),
        "mm" => Some(0.001),
        "ft" | "feet" | "foot" => Some(0.3048),
        "in" | "inch" | "inches" => Some(0.0254),
        _ => None,
    }
}

fn insunits_name(code: i64) -> Option<&'static str> {
    match code {
        1 => Some("in"),
        2 => Some("ft"),
        4 => Some("mm"),
        5 => Some("cm"),
        6 => Some("m"),
        _ => None,
    }
}

fn tokens(text: &str) -> Result<Vec<(i32, String)>, String> {
    let mut lines = text.lines();
    let mut out = Vec::new();
    while let Some(c) = lines.next() {
        if c.trim().is_empty() && out.is_empty() {
            continue;
        }
        let code: i32 = c.trim().parse().map_err(|_| format!("not a DXF file: expected a group code, found '{}'", c.trim().chars().take(24).collect::<String>()))?;
        let v = lines.next().ok_or("the DXF file ends in the middle of a group")?;
        out.push((code, v.trim().to_string()));
    }
    Ok(out)
}

fn num(v: &str, what: &str) -> Result<f64, String> {
    v.parse::<f64>().ok().filter(|x| x.is_finite()).ok_or_else(|| format!("{what}: '{v}' is not a number"))
}

/// Replaces the bulge segment `a -> b` by points along the arc (the end point `b` is not included).
fn arc_points(a: Pt, b: Pt, bulge: f64, out: &mut Vec<Pt>) {
    out.push(a);
    if bulge.abs() < 1e-12 {
        return;
    }
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let c = dx.hypot(dy);
    if c < 1e-12 {
        return;
    }
    let theta = 4.0 * bulge.atan();
    let d = (c / 2.0) * (1.0 - bulge * bulge) / (2.0 * bulge);
    let (mx, my) = ((a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0);
    let centre = [mx - dy / c * d, my + dx / c * d];
    let steps = (theta.abs().to_degrees() / ARC_STEP_DEG).ceil().max(1.0) as usize;
    for k in 1..steps {
        let phi = theta * k as f64 / steps as f64;
        let (s, co) = phi.sin_cos();
        let (rx, ry) = (a[0] - centre[0], a[1] - centre[1]);
        out.push([centre[0] + rx * co - ry * s, centre[1] + rx * s + ry * co]);
    }
}

struct Verts {
    pts: Vec<Pt>,
    bulges: Vec<f64>,
}

impl Verts {
    fn new() -> Self {
        Verts { pts: Vec::new(), bulges: Vec::new() }
    }
    fn expand(&self, closed: bool) -> Vec<Pt> {
        let n = self.pts.len();
        let mut out = Vec::new();
        for i in 0..n {
            if i + 1 == n && !closed {
                out.push(self.pts[i]);
            } else {
                arc_points(self.pts[i], self.pts[(i + 1) % n], self.bulges[i], &mut out);
            }
        }
        out
    }
}

fn dedupe(mut p: Vec<Pt>, tol: f64) -> Vec<Pt> {
    p.dedup_by(|b, a| (a[0] - b[0]).hypot(a[1] - b[1]) < tol);
    while p.len() > 1 && (p[0][0] - p[p.len() - 1][0]).hypot(p[0][1] - p[p.len() - 1][1]) < tol {
        p.pop();
    }
    p
}

/// Reads the drawing: the unit it declares and every closed polygon in its `ENTITIES` section (in drawing units).
fn read_dxf(text: &str) -> Result<(Option<i64>, Vec<Raw>, usize), String> {
    let t = tokens(text)?;
    let mut insunits = None;
    let mut i = 0;
    // HEADER: `9 $INSUNITS` then `70 n`.
    while i + 1 < t.len() {
        if t[i].0 == 9 && t[i].1.eq_ignore_ascii_case("$INSUNITS") && t[i + 1].0 == 70 {
            insunits = Some(t[i + 1].1.parse::<i64>().map_err(|_| format!("$INSUNITS: '{}' is not a number", t[i + 1].1))?);
        }
        i += 1;
    }
    // ENTITIES section.
    let start = (0..t.len().saturating_sub(1)).find(|&k| t[k].0 == 0 && t[k].1 == "SECTION" && t[k + 1].0 == 2 && t[k + 1].1 == "ENTITIES").ok_or("the DXF file has no ENTITIES section")?;
    let mut k = start + 2;
    let mut raws = Vec::new();
    let mut lines: Vec<(String, Pt, Pt)> = Vec::new();
    let mut open_shapes = 0usize;
    while k < t.len() {
        if t[k].0 != 0 {
            k += 1;
            continue;
        }
        let kind = t[k].1.clone();
        if kind == "ENDSEC" {
            break;
        }
        // The group codes of this entity run to the next code 0.
        let mut e = k + 1;
        while e < t.len() && t[e].0 != 0 {
            e += 1;
        }
        let body = &t[k + 1..e];
        let layer = body.iter().find(|g| g.0 == 8).map(|g| g.1.clone()).unwrap_or_else(|| "0".into());
        let flags = match body.iter().find(|g| g.0 == 70) {
            Some(g) => g.1.parse::<i64>().map_err(|_| format!("{kind}: flags '{}' is not a number", g.1))?,
            None => 0,
        };
        match kind.as_str() {
            "LWPOLYLINE" => {
                let mut v = Verts::new();
                let mut pending_x: Option<f64> = None;
                for g in body {
                    match g.0 {
                        10 => pending_x = Some(num(&g.1, "LWPOLYLINE x")?),
                        20 => {
                            let x = pending_x.take().ok_or("LWPOLYLINE: a y without its x")?;
                            v.pts.push([x, num(&g.1, "LWPOLYLINE y")?]);
                            v.bulges.push(0.0);
                        }
                        42 => {
                            let b = num(&g.1, "LWPOLYLINE bulge")?;
                            *v.bulges.last_mut().ok_or("LWPOLYLINE: a bulge before any vertex")? = b;
                        }
                        _ => {}
                    }
                }
                let closed = flags & 1 == 1 || (v.pts.len() > 3 && v.pts.first() == v.pts.last());
                if closed {
                    raws.push(Raw { layer, source: "LWPOLYLINE", pts: v.expand(true) });
                } else {
                    open_shapes += 1;
                }
                k = e;
            }
            "POLYLINE" => {
                // Old style: VERTEX entities follow, up to SEQEND.
                let mut v = Verts::new();
                let mut j = e;
                while j < t.len() && !(t[j].0 == 0 && t[j].1 == "SEQEND") {
                    if t[j].0 == 0 && t[j].1 == "VERTEX" {
                        let mut m = j + 1;
                        let (mut x, mut y, mut b, mut vflags) = (None, None, 0.0, 0i64);
                        while m < t.len() && t[m].0 != 0 {
                            match t[m].0 {
                                10 => x = Some(num(&t[m].1, "VERTEX x")?),
                                20 => y = Some(num(&t[m].1, "VERTEX y")?),
                                42 => b = num(&t[m].1, "VERTEX bulge")?,
                                70 => vflags = t[m].1.parse::<i64>().unwrap_or(0),
                                _ => {}
                            }
                            m += 1;
                        }
                        // Flag 16 is a spline control point, 64 a mesh vertex: not boundary corners.
                        if vflags & (16 | 64) == 0 {
                            v.pts.push([x.ok_or("VERTEX without x")?, y.ok_or("VERTEX without y")?]);
                            v.bulges.push(b);
                        }
                        j = m;
                    } else {
                        j += 1;
                    }
                }
                let closed = flags & 1 == 1 || (v.pts.len() > 3 && v.pts.first() == v.pts.last());
                if closed {
                    raws.push(Raw { layer, source: "POLYLINE", pts: v.expand(true) });
                } else {
                    open_shapes += 1;
                }
                k = j;
            }
            "LINE" => {
                let get = |code: i32| body.iter().find(|g| g.0 == code).map(|g| num(&g.1, "LINE")).transpose();
                let (x1, y1, x2, y2) = (get(10)?, get(20)?, get(11)?, get(21)?);
                match (x1, y1, x2, y2) {
                    (Some(a), Some(b), Some(c), Some(d)) => lines.push((layer, [a, b], [c, d])),
                    _ => return Err("a LINE is missing a coordinate".into()),
                }
                k = e;
            }
            _ => k = e,
        }
    }
    let (loops, loose) = chain_lines(&lines);
    raws.extend(loops);
    Ok((insunits, raws, open_shapes + loose))
}

/// Joins `LINE` entities of one layer into closed loops (ends within a billionth of the drawing's extent are one corner).
/// Returns the loops and how many segments belong to none.
fn chain_lines(lines: &[(String, Pt, Pt)]) -> (Vec<Raw>, usize) {
    let span = lines.iter().flat_map(|l| [l.1, l.2]).fold(0.0f64, |m, p| m.max(p[0].abs()).max(p[1].abs()));
    let tol = (span * 1e-9).max(1e-9);
    let mut loose = 0usize;
    let mut layers: Vec<&str> = lines.iter().map(|l| l.0.as_str()).collect();
    layers.sort_unstable();
    layers.dedup();
    let near = |a: Pt, b: Pt| (a[0] - b[0]).hypot(a[1] - b[1]) <= tol;
    let mut out = Vec::new();
    for layer in layers {
        let segs: Vec<(Pt, Pt)> = lines.iter().filter(|l| l.0 == layer).map(|l| (l.1, l.2)).collect();
        let mut used = vec![false; segs.len()];
        for s in 0..segs.len() {
            if used[s] {
                continue;
            }
            // Walk from this segment, taking the segment that continues from the current end, until back at the start.
            let mut path = vec![segs[s].0];
            let mut here = segs[s].1;
            let mut taken = vec![s];
            let mut closed = false;
            loop {
                path.push(here);
                if near(here, path[0]) && taken.len() >= 3 {
                    path.pop();
                    closed = true;
                    break;
                }
                let next = (0..segs.len()).find(|&q| !used[q] && !taken.contains(&q) && (near(segs[q].0, here) || near(segs[q].1, here)));
                match next {
                    Some(q) => {
                        here = if near(segs[q].0, here) { segs[q].1 } else { segs[q].0 };
                        taken.push(q);
                    }
                    None => break,
                }
            }
            if closed {
                for q in taken {
                    used[q] = true;
                }
                out.push(Raw { layer: layer.to_string(), source: "LINE chain", pts: path });
            }
        }
        loose += used.iter().filter(|u| !**u).count();
    }
    (out, loose)
}

/// Reads plot boundary candidates from a DXF drawing. `units` states the drawing unit (`m`, `cm`, `mm`, `ft`, `in`) and is
/// required when the drawing does not declare one. `index` picks one of several candidates.
pub fn import_dxf(bytes: &[u8], units: Option<&str>, index: Option<usize>) -> Result<ImportResult, String> {
    if bytes.starts_with(b"AC10") {
        return Err("this is a DWG file: its binary format can only be read with the licensed Open Design Alliance libraries. Save the drawing as DXF (ASCII) from your CAD program and import that".into());
    }
    if bytes.starts_with(b"AutoCAD Binary DXF") {
        return Err("this is a binary DXF: save it as ASCII DXF".into());
    }
    let text = String::from_utf8_lossy(bytes);
    let (insunits, raws, open_shapes) = read_dxf(&text)?;
    let mut warnings = Vec::new();

    let header = insunits.and_then(insunits_name);
    let (unit, unit_source) = match (units, header) {
        (Some(u), h) => {
            unit_factor(u).ok_or_else(|| format!("unknown unit '{u}': use m, cm, mm, ft or in"))?;
            if let Some(h) = h.filter(|h| unit_factor(h) != unit_factor(u)) {
                warnings.push(format!("the drawing declares {h} but {u} was stated: {u} is used"));
            }
            (u.trim().to_ascii_lowercase(), "stated by the user".to_string())
        }
        (None, Some(h)) => (h.to_string(), "the drawing's $INSUNITS".to_string()),
        (None, None) => {
            let why = match insunits {
                Some(0) => "is unitless (0)".to_string(),
                Some(c) => format!("is {c}, a unit this import does not convert"),
                None => "has no $INSUNITS".to_string(),
            };
            return Err(format!("the drawing {why}: state its unit (m, cm, mm, ft or in) so the plot is not read at the wrong size"));
        }
    };
    let f = unit_factor(&unit).ok_or("unit")?;

    if open_shapes > 0 {
        warnings.push(format!("{open_shapes} open shape(s) or loose line(s) were ignored: a plot boundary must be closed"));
    }
    let mut candidates = Vec::new();
    for r in raws {
        let pts = dedupe(r.pts.iter().map(|p| [p[0] * f, p[1] * f]).collect(), 1e-6);
        let mut c = Candidate { layer: r.layer, source: r.source.to_string(), polygon_m: pts.clone(), area_m2: 0.0, perimeter_m: 0.0, problem: None };
        if pts.len() < 3 {
            c.problem = Some("fewer than 3 distinct corners".into());
        } else {
            let a = signed_area(&pts);
            c.area_m2 = a.abs();
            c.perimeter_m = (0..pts.len()).map(|i| (pts[(i + 1) % pts.len()][0] - pts[i][0]).hypot(pts[(i + 1) % pts.len()][1] - pts[i][1])).sum();
            if a.abs() < 1e-6 {
                c.problem = Some("no area".into());
            } else if !is_simple(&pts) {
                c.problem = Some("the outline crosses itself or touches itself".into());
            } else if a < 0.0 {
                c.polygon_m.reverse();
            }
        }
        candidates.push(c);
    }
    if candidates.is_empty() {
        return Err("no closed boundary was found: draw the plot as a closed polyline, or as lines that meet end to end".into());
    }
    let usable: Vec<usize> = (0..candidates.len()).filter(|&i| candidates[i].problem.is_none()).collect();
    let selected = match index {
        Some(i) => {
            let c = candidates.get(i).ok_or_else(|| format!("there is no candidate {i}: the drawing has {}", candidates.len()))?;
            if let Some(p) = &c.problem {
                return Err(format!("candidate {i} cannot be a plot: {p}"));
            }
            Some(i)
        }
        None if usable.len() == 1 => Some(usable[0]),
        None if usable.is_empty() => return Err("every closed shape in the drawing is unusable as a plot; see the candidates' problems".into()),
        None => {
            warnings.push(format!("{} usable boundaries: choose one by its index", usable.len()));
            None
        }
    };
    // Put the chosen boundary at the origin (survey coordinates are often millions of metres from it).
    let mut origin_offset_m = None;
    if let Some(i) = selected {
        let p = &candidates[i].polygon_m;
        let (mx, my) = (p.iter().map(|q| q[0]).fold(f64::INFINITY, f64::min), p.iter().map(|q| q[1]).fold(f64::INFINITY, f64::min));
        for q in &mut candidates[i].polygon_m {
            q[0] -= mx;
            q[1] -= my;
        }
        origin_offset_m = Some([-mx, -my]);
    }
    Ok(ImportResult { unit, unit_source, candidates, selected, origin_offset_m, warnings })
}
