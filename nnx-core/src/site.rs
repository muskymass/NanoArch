//! Plots of any shape: an N-vertex polygon, each edge a front street, a side street, a neighbour or a plaza, the setback of
//! every edge, the buildable polygon left after the setbacks, and the building the planner can draw in it.
//!
//! - **Setbacks.** Each edge has its own distance: the one you entered for it, or the rule of the selected rule set. The
//!   rule sets are the Ministry of Municipal and Rural Affairs residential guidelines (1446 AH, clause 4-1 for villas, 4-2
//!   for apartment buildings, pdf p.18 and p.22 of the guideline file), see `presets`-style citations below. Municipalities
//!   and development authorities may override them, so every distance can be overridden per edge, and a case the guideline
//!   does not cover (a side street 30 m wide or more) is reported, not guessed.
//! - **Offset.** The buildable polygon is the plot moved inward by the distance of each edge (the offset lines of neighbouring
//!   edges are intersected). If the setbacks collapse the shape, or an edge disappears, it says so.
//! - **Building.** The planner draws rectangular buildings, so the building is the largest rectangle inside the buildable
//!   polygon, turned to be parallel to the front street. The part of the buildable polygon it does not use is reported.
//!
//! Coordinates are metres, x east and y north, counter-clockwise (a clockwise polygon is turned round, with its edges).

use serde::{Deserialize, Serialize};

pub type Pt = [f64; 2];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// The street the building faces (the entrance side).
    FrontStreet,
    SideStreet,
    #[default]
    Neighbour,
    Plaza,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Edge {
    pub kind: EdgeKind,
    /// Width of the street the edge faces, m (streets only).
    pub street_width_m: Option<f64>,
    /// A setback you entered for this edge, m. It replaces the rule's.
    pub setback_m: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuleSet {
    /// Ministry guidelines 1446 AH, clause 4-1 (villas), pdf p.18.
    #[default]
    MinistryVilla,
    /// Ministry guidelines 1446 AH, clause 4-2 (apartment buildings), pdf p.22: needs the number of floors.
    MinistryApartment,
    /// No rule: every edge must carry its own setback.
    Manual,
}

pub const RULE_SOURCE: &str = "Ministry of Municipal and Rural Affairs, residential building guidelines 1446 AH (2024), clause 4-1 (villas, pdf p.18) / 4-2 (apartment buildings, pdf p.22); a municipality or development authority may set other values";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SiteInput {
    pub polygon_m: Vec<Pt>,
    pub edges: Vec<Edge>,
    pub rule: RuleSet,
    /// Number of floors, for the apartment rule.
    pub floors: Option<u32>,
}

/// The building the planner can draw: `width_m` along the front street, `length_m` into the plot.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Building {
    pub width_m: f64,
    pub length_m: f64,
    /// World position of the lower-left corner of the building in its own frame, m.
    pub origin_m: Pt,
    /// Turn of the building's x axis from the world x axis, degrees (counter-clockwise).
    pub angle_deg: f64,
    /// The four corners in the world, counter-clockwise from the lower-left.
    pub corners_m: [Pt; 4],
    /// The side of the building that faces the front street, in its own frame (`south` after the turn).
    pub entry: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct SiteResult {
    pub polygon_m: Vec<Pt>,
    pub edges: Vec<Edge>,
    pub area_m2: f64,
    /// The setback of each edge, m, and where it came from.
    pub setbacks_m: Vec<f64>,
    pub setback_sources: Vec<String>,
    pub buildable_m: Vec<Pt>,
    pub buildable_area_m2: f64,
    pub building: Building,
    pub building_area_m2: f64,
    /// Buildable area the rectangle does not use, m2.
    pub unused_buildable_m2: f64,
    /// Footprint over plot area.
    pub coverage: f64,
    pub warnings: Vec<String>,
}

// ------------------------------------------------------------------ geometry helpers

pub fn signed_area(p: &[Pt]) -> f64 {
    let n = p.len();
    (0..n).map(|i| p[i][0] * p[(i + 1) % n][1] - p[(i + 1) % n][0] * p[i][1]).sum::<f64>() / 2.0
}

fn cross(o: Pt, a: Pt, b: Pt) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// Distances below this (metres) are treated as zero: coordinates that went through a rotation are exact only to rounding.
const EPS: f64 = 1e-7;

/// Which side of the line `ab` the point is on: 1 left, -1 right, 0 on it (within `EPS`).
fn side(a: Pt, b: Pt, p: Pt) -> i8 {
    let d = cross(a, b, p) / (b[0] - a[0]).hypot(b[1] - a[1]).max(1e-12);
    if d > EPS { 1 } else if d < -EPS { -1 } else { 0 }
}

/// Do segments `ab` and `cd` cross at a point that is interior to both?
fn properly_cross(a: Pt, b: Pt, c: Pt, d: Pt) -> bool {
    side(c, d, a) * side(c, d, b) < 0 && side(a, b, c) * side(a, b, d) < 0
}

fn touches(a: Pt, b: Pt, p: Pt) -> bool {
    side(a, b, p) == 0 && p[0] >= a[0].min(b[0]) - EPS && p[0] <= a[0].max(b[0]) + EPS && p[1] >= a[1].min(b[1]) - EPS && p[1] <= a[1].max(b[1]) + EPS
}

/// Is `p` inside or on the boundary of the polygon?
pub fn contains(poly: &[Pt], p: Pt) -> bool {
    let n = poly.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if touches(a, b, p) {
            return true;
        }
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
    }
    inside
}

/// A polygon is simple when no two edges cross and no vertex lies on a non-adjacent edge.
pub fn is_simple(p: &[Pt]) -> bool {
    let n = p.len();
    for i in 0..n {
        for j in i + 1..n {
            if j == i + 1 || (i == 0 && j == n - 1) {
                continue;
            }
            let (a, b, c, d) = (p[i], p[(i + 1) % n], p[j], p[(j + 1) % n]);
            if properly_cross(a, b, c, d) || touches(a, b, c) || touches(a, b, d) || touches(c, d, a) || touches(c, d, b) {
                return false;
            }
        }
    }
    true
}

/// Checks the polygon and turns it counter-clockwise, with its edges. Edge `i` runs from vertex `i` to vertex `i + 1`.
pub fn normalise(input: &SiteInput) -> Result<(Vec<Pt>, Vec<Edge>), String> {
    let p = &input.polygon_m;
    if p.len() < 3 {
        return Err("the plot needs at least 3 vertices".into());
    }
    if p.iter().flatten().any(|v| !v.is_finite()) {
        return Err("every vertex must be a pair of numbers (metres)".into());
    }
    if input.edges.len() != p.len() {
        return Err(format!("the plot has {} vertices and so {} edges, but {} edge descriptions were given", p.len(), p.len(), input.edges.len()));
    }
    for i in 0..p.len() {
        let (a, b) = (p[i], p[(i + 1) % p.len()]);
        if (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-3 {
            return Err(format!("vertices {} and {} coincide", i + 1, (i + 1) % p.len() + 1));
        }
    }
    if !is_simple(p) {
        return Err("the plot outline crosses itself or touches itself".into());
    }
    let area = signed_area(p);
    if area.abs() < 1e-6 {
        return Err("the plot has no area".into());
    }
    if area > 0.0 {
        return Ok((p.clone(), input.edges.clone()));
    }
    // Clockwise: reverse the vertices; edge i (v_i to v_i+1) becomes the edge from the reversed order's v_i+1 to v_i.
    let n = p.len();
    let rp: Vec<Pt> = p.iter().rev().copied().collect();
    // After reversing, new edge k joins rp[k] = p[n-1-k] and rp[k+1] = p[n-2-k], which is old edge n-2-k (mod n).
    let re: Vec<Edge> = (0..n).map(|k| input.edges[(2 * n - 2 - k) % n].clone()).collect();
    Ok((rp, re))
}

fn len(a: Pt, b: Pt) -> f64 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}

// ------------------------------------------------------------------ setbacks

/// The setback of one edge by the rule, with its source, or why the rule has none.
fn rule_setback(rule: RuleSet, e: &Edge, floors: Option<u32>) -> Result<(f64, String), String> {
    let w = e.street_width_m;
    let street = |what: &str| -> Result<f64, String> { w.filter(|v| v.is_finite() && *v > 0.0).ok_or_else(|| format!("the {what} needs the width of its street")) };
    let (clause, villa) = match rule {
        RuleSet::MinistryVilla => ("clause 4-1 (4), (5) pdf p.18", true),
        RuleSet::MinistryApartment => ("clause 4-2 (4) pdf p.22", false),
        RuleSet::Manual => return Err("no rule is selected: enter the setback of this edge".into()),
    };
    match e.kind {
        EdgeKind::FrontStreet => {
            let w = street("front street")?;
            // Streets under 30 m: a fifth of the width, not less than 3 m. 30 m or more (villas): not less than 6 m.
            let v = if w >= 30.0 && villa { 6.0 } else { (w / 5.0).max(3.0) };
            Ok((v, format!("{clause}: front setback, street {w} m")))
        }
        EdgeKind::SideStreet => {
            let w = street("side street")?;
            if w >= 30.0 {
                return Err(format!("the guideline gives no setback for a side street {w} m wide (30 m or more): enter it"));
            }
            Ok(((w / 5.0).max(2.0), format!("{clause}: side/rear street setback, street {w} m")))
        }
        EdgeKind::Neighbour => {
            if villa {
                Ok((1.5, format!("{clause}: towards neighbours")))
            } else {
                let f = floors.ok_or("the apartment rule needs the number of floors (neighbour setback 3 m above 5 floors, 2 m up to 5)")?;
                Ok((if f > 5 { 3.0 } else { 2.0 }, format!("{clause}: towards neighbours, {f} floors")))
            }
        }
        EdgeKind::Plaza => Ok((3.0, format!("{clause}: towards squares and plazas"))),
    }
}

/// Setback of every edge: yours when entered, the rule's otherwise. All the edges that cannot be given one are named.
pub fn edge_setbacks(edges: &[Edge], rule: RuleSet, floors: Option<u32>) -> Result<(Vec<f64>, Vec<String>), Vec<String>> {
    let (mut d, mut src, mut errs) = (Vec::new(), Vec::new(), Vec::new());
    for (i, e) in edges.iter().enumerate() {
        match e.setback_m {
            Some(v) if v.is_finite() && v >= 0.0 => {
                d.push(v);
                src.push("entered by you".to_string());
            }
            Some(v) => {
                errs.push(format!("edge {}: the setback {v} must be zero or more", i + 1));
                d.push(0.0);
                src.push(String::new());
            }
            None => match rule_setback(rule, e, floors) {
                Ok((v, s)) => {
                    d.push(v);
                    src.push(s);
                }
                Err(m) => {
                    errs.push(format!("edge {}: {m}", i + 1));
                    d.push(0.0);
                    src.push(String::new());
                }
            },
        }
    }
    if errs.is_empty() { Ok((d, src)) } else { Err(errs) }
}

// ------------------------------------------------------------------ offset

/// The polygon moved inward by `d[i]` at edge `i` (counter-clockwise polygon, interior on the left). Neighbouring offset lines
/// are intersected. Fails when the result is not a simple polygon of the same orientation with every edge still pointing the
/// way it did (a collapsed edge flips), or leaves the plot.
pub fn inset(poly: &[Pt], d: &[f64]) -> Result<Vec<Pt>, String> {
    let n = poly.len();
    // Offset line i: a point and a direction.
    let lines: Vec<(Pt, Pt)> = (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            let l = len(a, b);
            let dir = [(b[0] - a[0]) / l, (b[1] - a[1]) / l];
            let normal = [-dir[1], dir[0]]; // left of the direction: the interior
            ([a[0] + normal[0] * d[i], a[1] + normal[1] * d[i]], dir)
        })
        .collect();
    let mut out: Vec<Pt> = Vec::with_capacity(n);
    for i in 0..n {
        // Vertex i is between edge i-1 and edge i.
        let (p0, d0) = lines[(i + n - 1) % n];
        let (p1, d1) = lines[i];
        let det = d0[0] * d1[1] - d0[1] * d1[0];
        if det.abs() < 1e-9 {
            // Collinear neighbours: the offset vertex is the first line's point moved to the vertex's own position along it.
            if (d[(i + n - 1) % n] - d[i]).abs() > 1e-6 {
                return Err(format!("edges {} and {} are in line but have different setbacks", (i + n - 1) % n + 1, i + 1));
            }
            let v = poly[i];
            let normal = [-d1[1], d1[0]];
            out.push([v[0] + normal[0] * d[i], v[1] + normal[1] * d[i]]);
            continue;
        }
        let t = ((p1[0] - p0[0]) * d1[1] - (p1[1] - p0[1]) * d1[0]) / det;
        out.push([p0[0] + d0[0] * t, p0[1] + d0[1] * t]);
    }
    // Validate: same orientation, simple, every edge keeps its direction, inside the plot.
    if signed_area(&out) <= 0.0 || signed_area(&out) > signed_area(poly) + 1e-9 {
        return Err("the setbacks leave no buildable area".into());
    }
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let (c, e) = (out[i], out[(i + 1) % n]);
        if (b[0] - a[0]) * (e[0] - c[0]) + (b[1] - a[1]) * (e[1] - c[1]) <= 0.0 {
            return Err(format!("the setbacks make edge {} disappear: reduce them or reshape the plot", i + 1));
        }
    }
    if !is_simple(&out) {
        return Err("the setbacks make the buildable outline cross itself: reduce them".into());
    }
    if let Some(i) = out.iter().position(|v| !contains(poly, *v)) {
        return Err(format!("the buildable outline leaves the plot at vertex {}: reduce the setbacks", i + 1));
    }
    Ok(dedupe(out))
}

/// Drops vertices that lie on the line of their neighbours.
fn dedupe(p: Vec<Pt>) -> Vec<Pt> {
    let n = p.len();
    let keep: Vec<Pt> = (0..n).filter(|&i| cross(p[(i + n - 1) % n], p[i], p[(i + 1) % n]).abs() > 1e-9).map(|i| p[i]).collect();
    if keep.len() >= 3 { keep } else { p }
}

// ------------------------------------------------------------------ the building

fn rotate(p: Pt, angle: f64) -> Pt {
    let (s, c) = angle.sin_cos();
    [p[0] * c - p[1] * s, p[0] * s + p[1] * c]
}

/// Is the rectangle `[x0, x1] x [y0, y1]` inside the polygon: corners inside, no edge crossing it, no vertex strictly inside it.
fn rect_inside(poly: &[Pt], x0: f64, y0: f64, x1: f64, y1: f64) -> bool {
    let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    if corners.iter().any(|c| !contains(poly, *c)) {
        return false;
    }
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        for k in 0..4 {
            if properly_cross(a, b, corners[k], corners[(k + 1) % 4]) {
                return false;
            }
        }
        if a[0] > x0 + EPS && a[0] < x1 - EPS && a[1] > y0 + EPS && a[1] < y1 - EPS {
            return false;
        }
    }
    true
}

/// The largest axis-aligned rectangle inside `poly`: `(x0, y0, x1, y1)`, found on a grid of `cell` metres and then checked
/// exactly (it is shrunk by one cell at a time until it is truly inside).
pub fn largest_rect(poly: &[Pt], cell: f64) -> Option<(f64, f64, f64, f64)> {
    let (minx, miny) = (poly.iter().map(|p| p[0]).fold(f64::MAX, f64::min), poly.iter().map(|p| p[1]).fold(f64::MAX, f64::min));
    let (maxx, maxy) = (poly.iter().map(|p| p[0]).fold(f64::MIN, f64::max), poly.iter().map(|p| p[1]).fold(f64::MIN, f64::max));
    let cell = cell.max(((maxx - minx).max(maxy - miny)) / 600.0);
    let (cols, rows) = (((maxx - minx) / cell).ceil() as usize, ((maxy - miny) / cell).ceil() as usize);
    if cols == 0 || rows == 0 {
        return None;
    }
    // A cell is usable when all four of its corners are inside the polygon.
    let mut ok = vec![vec![false; cols]; rows];
    for (r, row) in ok.iter_mut().enumerate() {
        for (c, cellv) in row.iter_mut().enumerate() {
            let (x0, y0) = (minx + c as f64 * cell, miny + r as f64 * cell);
            *cellv = [[x0, y0], [x0 + cell, y0], [x0, y0 + cell], [x0 + cell, y0 + cell]].iter().all(|p| contains(poly, *p));
        }
    }
    // Largest rectangle of true cells (histogram method).
    let mut heights = vec![0usize; cols];
    let mut best: Option<(usize, usize, usize, usize, usize)> = None; // area, r0, r1, c0, c1
    for (r, okrow) in ok.iter().enumerate() {
        for c in 0..cols {
            heights[c] = if okrow[c] { heights[c] + 1 } else { 0 };
        }
        let mut stack: Vec<usize> = Vec::new();
        for c in 0..=cols {
            let h = if c < cols { heights[c] } else { 0 };
            while let Some(&top) = stack.last() {
                if heights[top] <= h {
                    break;
                }
                stack.pop();
                let height = heights[top];
                let left = stack.last().map_or(0, |&l| l + 1);
                let area = height * (c - left);
                if best.is_none_or(|b| area > b.0) {
                    best = Some((area, r + 1 - height, r + 1, left, c));
                }
            }
            stack.push(c.min(cols.saturating_sub(1)));
            if c == cols {
                break;
            }
        }
    }
    let (_, r0, r1, c0, c1) = best.filter(|b| b.0 > 0)?;
    let (mut x0, mut y0, mut x1, mut y1) = (minx + c0 as f64 * cell, miny + r0 as f64 * cell, minx + c1 as f64 * cell, miny + r1 as f64 * cell);
    for _ in 0..40 {
        if rect_inside(poly, x0, y0, x1, y1) {
            return Some((x0, y0, x1, y1));
        }
        // Shrink the side that is longer, by one cell, evenly from both ends.
        if x1 - x0 >= y1 - y0 {
            x0 += cell / 2.0;
            x1 -= cell / 2.0;
        } else {
            y0 += cell / 2.0;
            y1 -= cell / 2.0;
        }
        if x1 - x0 <= cell || y1 - y0 <= cell {
            break;
        }
    }
    None
}

fn round_mm(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Everything about a plot: the setbacks, the buildable polygon, the building and the footprint.
pub fn analyse(input: &SiteInput) -> Result<SiteResult, Vec<String>> {
    let (poly, edges) = normalise(input).map_err(|e| vec![e])?;
    let area = signed_area(&poly);
    let front = edges.iter().position(|e| e.kind == EdgeKind::FrontStreet).ok_or_else(|| vec!["mark one edge as the front street (the side the building faces)".to_string()])?;
    if edges.iter().filter(|e| e.kind == EdgeKind::FrontStreet).count() > 1 {
        return Err(vec!["only one edge can be the front street".to_string()]);
    }
    let (setbacks, sources) = edge_setbacks(&edges, input.rule, input.floors)?;
    let buildable = inset(&poly, &setbacks).map_err(|e| vec![e])?;
    // Turn the buildable polygon so the front street is horizontal with the plot above it.
    let (a, b) = (poly[front], poly[(front + 1) % poly.len()]);
    let theta = (b[1] - a[1]).atan2(b[0] - a[0]);
    let turned: Vec<Pt> = buildable.iter().map(|p| rotate([p[0] - a[0], p[1] - a[1]], -theta)).collect();
    let (x0, y0, x1, y1) = largest_rect(&turned, 0.1).ok_or_else(|| vec!["no rectangle fits in the buildable area".to_string()])?;
    // Whole 10 mm, kept inside.
    // Whole centimetres, kept inside; a value within a micrometre of one counts as on it (rotated coordinates carry rounding).
    let (x0, y0, x1, y1) = (((x0 * 100.0) - 1e-4).ceil() / 100.0, ((y0 * 100.0) - 1e-4).ceil() / 100.0, ((x1 * 100.0) + 1e-4).floor() / 100.0, ((y1 * 100.0) + 1e-4).floor() / 100.0);
    let world = |p: Pt| -> Pt {
        let r = rotate(p, theta);
        [round_mm(r[0] + a[0]), round_mm(r[1] + a[1])]
    };
    let corners = [world([x0, y0]), world([x1, y0]), world([x1, y1]), world([x0, y1])];
    let (w, l) = (round_mm(x1 - x0), round_mm(y1 - y0));
    let mut warnings = Vec::new();
    let buildable_area = signed_area(&buildable);
    let building_area = w * l;
    if buildable_area - building_area > 0.05 * buildable_area {
        warnings.push(format!("the planner draws rectangles: {:.1} m2 of the {:.1} m2 buildable area is not used by the {w} x {l} m building", buildable_area - building_area, buildable_area));
    }
    Ok(SiteResult {
        polygon_m: poly,
        edges,
        area_m2: round_mm(area),
        setbacks_m: setbacks,
        setback_sources: sources,
        buildable_area_m2: round_mm(buildable_area),
        buildable_m: buildable,
        building: Building { width_m: w, length_m: l, origin_m: corners[0], angle_deg: round_mm(theta.to_degrees()), corners_m: corners, entry: "south" },
        building_area_m2: round_mm(building_area),
        unused_buildable_m2: round_mm((buildable_area - building_area).max(0.0)),
        coverage: building_area / area,
        warnings,
    })
}

/// Rectangle plots: width along the front street, length into the plot, the front street on the south edge.
/// A rectangular plot with the setbacks the user entered (front on the south edge, rear on the north, the sides on both).
pub fn rectangle_with_setbacks(width_m: f64, length_m: f64, front_m: f64, back_m: f64, side_m: f64) -> SiteInput {
    let mut i = rectangle(width_m, length_m);
    for (e, v) in i.edges.iter_mut().zip([front_m, side_m, back_m, side_m]) {
        e.setback_m = Some(v);
    }
    i
}

pub fn rectangle(width_m: f64, length_m: f64) -> SiteInput {
    let street = |kind| Edge { kind, ..Default::default() };
    SiteInput { polygon_m: vec![[0.0, 0.0], [width_m, 0.0], [width_m, length_m], [0.0, length_m]], edges: vec![street(EdgeKind::FrontStreet), street(EdgeKind::Neighbour), street(EdgeKind::Neighbour), street(EdgeKind::Neighbour)], rule: RuleSet::Manual, floors: None }
}

// ------------------------------------------------------------------ the drawing

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The site plan as SVG (north up): the plot, the setback distances, the buildable polygon and the building.
pub fn site_svg(r: &SiteResult, title: &str) -> String {
    site_svg_with(r, title, None)
}

/// The site plan with the landscape layout (boundary walls, gate, guard room, carport, pool, courtyard) drawn over it.
pub fn site_svg_with(r: &SiteResult, title: &str, layout: Option<&crate::site_layout::SiteLayout>) -> String {
    use std::fmt::Write;
    let pts = || r.polygon_m.iter();
    let (minx, miny) = (pts().map(|p| p[0]).fold(f64::MAX, f64::min), pts().map(|p| p[1]).fold(f64::MAX, f64::min));
    let (maxx, maxy) = (pts().map(|p| p[0]).fold(f64::MIN, f64::max), pts().map(|p| p[1]).fold(f64::MIN, f64::max));
    let s = (620.0 / (maxx - minx).max(maxy - miny)).clamp(2.0, 40.0);
    let m = 60.0;
    let (w, h) = ((maxx - minx) * s + 2.0 * m, (maxy - miny) * s + 2.0 * m + 40.0);
    let tx = |x: f64| m + (x - minx) * s;
    let ty = |y: f64| m + (maxy - y) * s;
    let poly = |p: &[Pt]| p.iter().map(|q| format!("{:.1},{:.1}", tx(q[0]), ty(q[1]))).collect::<Vec<_>>().join(" ");
    let mut o = String::new();
    let _ = write!(o, "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w:.0} {h:.0}\" width=\"{w:.0}\" height=\"{h:.0}\" font-family=\"Segoe UI, Tahoma, Arial, sans-serif\">\n<style>.bg{{fill:#fff}} .plot{{fill:#f4f1e8;stroke:#222;stroke-width:2}} .buildable{{fill:#dff0d8;stroke:#3c763d;stroke-width:1.5;stroke-dasharray:6 4}} .building{{fill:#9ec9ea;stroke:#1b4f7c;stroke-width:2}} .front{{stroke:#c0392b;stroke-width:4}} .t{{font-size:11px;fill:#222}} .sb{{font-size:10.5px;fill:#555}} .title{{font-size:15px;font-weight:700;fill:#111}}</style>\n<rect class=\"bg\" width=\"{w:.0}\" height=\"{h:.0}\"/>\n");
    let _ = writeln!(o, "<polygon class=\"plot\" points=\"{}\"/>", poly(&r.polygon_m));
    let _ = writeln!(o, "<polygon class=\"buildable\" points=\"{}\"/>", poly(&r.buildable_m));
    let _ = writeln!(o, "<polygon class=\"building\" points=\"{}\"/>", poly(&r.building.corners_m));
    if let Some(l) = layout {
        for it in &l.items {
            let (fill, label) = match it.kind {
                "guard" => ("#c9b79c", "guard"),
                "carport" => ("#b0bec5", "carport"),
                "pool" => ("#80deea", "pool"),
                _ => ("#c5e1a5", "courtyard"),
            };
            let c = it.corners_plot;
            let _ = writeln!(o, "<polygon points=\"{}\" style=\"fill:{fill};stroke:#37474f;stroke-width:1\"/><text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" style=\"font:11px sans-serif;fill:#263238\">{label}</text>", poly(&c), tx((c[0][0] + c[2][0]) / 2.0), ty((c[0][1] + c[2][1]) / 2.0));
        }
        for w in &l.walls {
            let _ = writeln!(o, "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" style=\"stroke:#4e342e;stroke-width:{:.1}\"/>", tx(w.a[0]), ty(w.a[1]), tx(w.b[0]), ty(w.b[1]), (l.wall_thickness_m * s).max(2.5));
        }
        if let Some([g0, g1]) = l.gate_m {
            let _ = writeln!(o, "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" style=\"stroke:#e65100;stroke-width:3;stroke-dasharray:4 3\"/><text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" style=\"font:11px sans-serif;fill:#e65100\">gate</text>", tx(g0[0]), ty(g0[1]), tx(g1[0]), ty(g1[1]), tx((g0[0] + g1[0]) / 2.0), ty((g0[1] + g1[1]) / 2.0) + 16.0);
        }
    }
    let n = r.polygon_m.len();
    for i in 0..n {
        let (a, b) = (r.polygon_m[i], r.polygon_m[(i + 1) % n]);
        if r.edges[i].kind == EdgeKind::FrontStreet {
            let _ = writeln!(o, "<line class=\"front\" x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\"/>", tx(a[0]), ty(a[1]), tx(b[0]), ty(b[1]));
        }
        let (mx, my) = ((a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0);
        // Label on the outside of the edge: away from the centre of the plot.
        let (cx, cy) = ((minx + maxx) / 2.0, (miny + maxy) / 2.0);
        let (dx, dy) = (mx - cx, my - cy);
        let norm = dx.hypot(dy).max(1e-9);
        let kind = match r.edges[i].kind {
            EdgeKind::FrontStreet => "front street",
            EdgeKind::SideStreet => "side street",
            EdgeKind::Neighbour => "neighbour",
            EdgeKind::Plaza => "plaza",
        };
        let wtxt = r.edges[i].street_width_m.map_or(String::new(), |w| format!(" {w} m"));
        let _ = writeln!(o, "<text class=\"t\" x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\">{}{} · {:.1} m</text>", tx(mx) + dx / norm * 26.0, ty(my) - dy / norm * 20.0, esc(kind), esc(&wtxt), len(a, b));
        let _ = writeln!(o, "<text class=\"sb\" x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\">{} m</text>", tx(mx) - dx / norm * 14.0, ty(my) + dy / norm * 14.0, r.setbacks_m[i]);
    }
    // The dimensions of the building rectangle along its own edges (width along the street, then length), inside the footprint.
    let c = &r.building.corners_m;
    for (a, b, v) in [(c[0], c[1], r.building.width_m), (c[1], c[2], r.building.length_m)] {
        let (mx, my) = ((a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0);
        let (cx, cy) = ((c[0][0] + c[2][0]) / 2.0, (c[0][1] + c[2][1]) / 2.0);
        let (dx, dy) = (cx - mx, cy - my);
        let k = dx.hypot(dy).max(1e-9);
        let _ = writeln!(o, "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" style=\"font:600 12px sans-serif;fill:#1b5e20\">{v} m</text>", tx(mx) + dx / k * 14.0, ty(my) - dy / k * 14.0);
    }
    // North arrow (north is up in this drawing) and a 10 m scale bar.
    let (nx, ny) = (w - 34.0, 52.0);
    let _ = writeln!(o, "<g style=\"stroke:#222;fill:#222\"><polygon points=\"{nx:.0},{:.0} {:.0},{:.0} {nx:.0},{:.0} {:.0},{:.0}\" style=\"fill:#222\"/><polygon points=\"{nx:.0},{:.0} {:.0},{:.0} {nx:.0},{:.0}\" style=\"fill:#fff\"/><text x=\"{nx:.0}\" y=\"{:.0}\" text-anchor=\"middle\" style=\"font:700 13px sans-serif;stroke:none\">N</text></g>", ny - 18.0, nx - 9.0, ny + 14.0, ny + 6.0, nx + 9.0, ny + 14.0, ny - 18.0, nx + 9.0, ny + 14.0, ny + 6.0, ny - 24.0);
    let bar = 10.0 * s;
    let _ = writeln!(o, "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" style=\"stroke:#222;stroke-width:3\"/><text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" style=\"font:11px sans-serif;fill:#222\">10 m</text>", w - m - bar, h - 52.0, w - m, h - 52.0, w - m - bar / 2.0, h - 58.0);
    let _ = writeln!(o, "<text class=\"title\" x=\"{m}\" y=\"{:.0}\">{}</text>", h - 30.0, esc(title));
    let _ = writeln!(
        o,
        "<text class=\"sb\" x=\"{m}\" y=\"{:.0}\">plot {:.1} m² · buildable {:.1} m² · building {} x {} m ({:.1} m², {:.0}% of the plot), turned {}°</text></svg>",
        h - 14.0,
        r.area_m2,
        r.buildable_area_m2,
        r.building.width_m,
        r.building.length_m,
        r.building_area_m2,
        r.coverage * 100.0,
        r.building.angle_deg
    );
    o
}
