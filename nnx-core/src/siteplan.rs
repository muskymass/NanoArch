//! General site plan with boundary fences and gates: which plot edges are fenced, with what, and where every gate (vehicle or
//! pedestrian) opens, drawn over the plot, the setback polygon and the building, with a fence and gate schedule.
//!
//! Nothing here is a statutory figure. Fence heights and thicknesses, gate positions and widths are entered by the user and each
//! fence and gate carries a `source` (the brief, the municipality's letter, the product data sheet). A fence or gate without one
//! is refused by name, and no height or width is defaulted.
//!
//! Edge numbers follow `SiteResult::edges` (edge `i` runs from vertex `i` to vertex `i + 1` of `polygon_m`, counter-clockwise,
//! interior on the left). A gate sits on a fenced edge: `offset_m` is the distance from the edge's start to the near jamb along
//! the edge. The fenced length of an edge is its length less the gates cut into it.
//!
//! Not modelled and stated in `notes`: corner returns, pier and post spacing, gate leaf clearances against the building, and
//! whether the municipality allows a fence or a gate at any position (including inside a setback).

use crate::permitpack::esc;
use crate::site::{EdgeKind, Pt, SiteResult};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

/// The most fences and gates accepted in one call.
pub const MAX_ITEMS: usize = 200;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FenceIn {
    /// Index into the site result's edges.
    pub edge: usize,
    /// Free text: `solid wall`, `railing`, `mesh`, ...
    pub kind: String,
    pub height_m: f64,
    pub thickness_m: f64,
    /// Where the height and thickness come from. Required.
    pub source: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct GateIn {
    pub key: String,
    pub edge: usize,
    /// `vehicle` or `pedestrian`.
    pub kind: String,
    /// Distance from the edge's start to the near jamb, metres.
    pub offset_m: f64,
    pub width_m: f64,
    /// Optional: `swing_in`, `swing_out` or `sliding`. Swings draw the open leaf and its arc.
    pub operation: String,
    /// Where the position and width come from. Required.
    pub source: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Input {
    pub title: String,
    pub fences: Vec<FenceIn>,
    pub gates: Vec<GateIn>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FenceRow {
    pub edge: usize,
    pub edge_kind: &'static str,
    pub kind: String,
    pub height_m: f64,
    pub thickness_m: f64,
    pub edge_length_m: f64,
    pub gates: usize,
    pub gate_total_m: f64,
    /// Edge length less the gates cut into it.
    pub fenced_length_m: f64,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GateRow {
    pub key: String,
    pub edge: usize,
    pub kind: String,
    pub operation: String,
    pub offset_m: f64,
    pub width_m: f64,
    /// The two jambs in the plot frame, metres.
    pub jambs_m: [Pt; 2],
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub fences: Vec<FenceRow>,
    pub gates: Vec<GateRow>,
    pub total_fenced_length_m: f64,
    pub vehicle_gates: usize,
    pub pedestrian_gates: usize,
    /// Edges of the plot with no fence entered.
    pub unfenced_edges: Vec<usize>,
    pub svg: String,
    pub notes: Vec<String>,
}

const KINDS: [&str; 2] = ["vehicle", "pedestrian"];
const OPERATIONS: [&str; 4] = ["", "swing_in", "swing_out", "sliding"];

fn pos(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn edge_kind_name(k: EdgeKind) -> &'static str {
    match k {
        EdgeKind::FrontStreet => "front street",
        EdgeKind::SideStreet => "side street",
        EdgeKind::Neighbour => "neighbour",
        EdgeKind::Plaza => "plaza",
    }
}

fn point_on(a: Pt, b: Pt, d: f64) -> Pt {
    let len = (b[0] - a[0]).hypot(b[1] - a[1]);
    [a[0] + (b[0] - a[0]) * d / len, a[1] + (b[1] - a[1]) * d / len]
}

/// Builds the plan. Refuses, naming each problem, when an input is missing or malformed.
pub fn build(r: &SiteResult, input: &Input) -> Result<Plan, String> {
    let n = r.polygon_m.len();
    let edge_pts = |i: usize| (r.polygon_m[i], r.polygon_m[(i + 1) % n]);
    let edge_len = |i: usize| {
        let (a, b) = edge_pts(i);
        (b[0] - a[0]).hypot(b[1] - a[1])
    };
    let mut errs: Vec<String> = Vec::new();
    if input.fences.is_empty() {
        errs.push("fences (at least one, each with edge, kind, height_m, thickness_m and source)".into());
    }
    if input.fences.len() > MAX_ITEMS || input.gates.len() > MAX_ITEMS {
        errs.push(format!("at most {MAX_ITEMS} fences and {MAX_ITEMS} gates are accepted"));
    }
    let mut fenced: BTreeMap<usize, &FenceIn> = BTreeMap::new();
    for (i, f) in input.fences.iter().enumerate() {
        if f.edge >= n {
            errs.push(format!("fences[{i}].edge {} is not an edge of the plot (it has {n}, numbered from 0)", f.edge));
            continue;
        }
        if fenced.insert(f.edge, f).is_some() {
            errs.push(format!("fences[{i}]: edge {} has more than one fence", f.edge));
        }
        if !pos(f.height_m) || !pos(f.thickness_m) {
            errs.push(format!("fences[{i}] (edge {}): height_m and thickness_m must be positive numbers", f.edge));
        }
        if f.source.trim().is_empty() {
            errs.push(format!("fences[{i}] (edge {}): source (where its height and thickness come from)", f.edge));
        }
    }
    let mut keys: HashSet<&str> = HashSet::new();
    for (i, g) in input.gates.iter().enumerate() {
        let k = g.key.trim();
        if k.is_empty() {
            errs.push(format!("gates[{i}].key"));
        } else if !keys.insert(k) {
            errs.push(format!("gates[{i}].key `{k}` is duplicated"));
        }
        let label = if k.is_empty() { format!("gates[{i}]") } else { format!("gate `{k}`") };
        if !KINDS.contains(&g.kind.trim()) {
            errs.push(format!("{label}: kind must be vehicle or pedestrian, got `{}`", g.kind.trim()));
        }
        if !OPERATIONS.contains(&g.operation.trim()) {
            errs.push(format!("{label}: operation must be swing_in, swing_out or sliding, got `{}`", g.operation.trim()));
        }
        if !pos(g.width_m) || !g.offset_m.is_finite() || g.offset_m < 0.0 {
            errs.push(format!("{label}: width_m must be positive and offset_m zero or more"));
        }
        if g.source.trim().is_empty() {
            errs.push(format!("{label}: source (where its position and width come from)"));
        }
        if g.edge >= n {
            errs.push(format!("{label}: edge {} is not an edge of the plot (it has {n}, numbered from 0)", g.edge));
        } else if !fenced.contains_key(&g.edge) {
            errs.push(format!("{label}: edge {} has no fence to cut the gate into", g.edge));
        } else if pos(g.width_m) && g.offset_m.is_finite() && g.offset_m + g.width_m > edge_len(g.edge) + 1e-9 {
            errs.push(format!("{label}: runs from {} to {} m past the end of edge {}, which is {} m long", g.offset_m, r2(g.offset_m + g.width_m), g.edge, r2(edge_len(g.edge))));
        }
    }
    if errs.is_empty() {
        for e in fenced.keys() {
            let mut on: Vec<&GateIn> = input.gates.iter().filter(|g| g.edge == *e).collect();
            on.sort_by(|a, b| a.offset_m.total_cmp(&b.offset_m));
            for w in on.windows(2) {
                if w[0].offset_m + w[0].width_m > w[1].offset_m + 1e-9 {
                    errs.push(format!("gates `{}` and `{}` overlap on edge {e}", w[0].key.trim(), w[1].key.trim()));
                }
            }
        }
    }
    if !errs.is_empty() {
        return Err(format!("site fence plan: {}. Nothing is defaulted and no height or width is built in; enter them.", errs.join("; ")));
    }

    let mut gate_rows: Vec<GateRow> = Vec::new();
    for g in &input.gates {
        let (a, b) = edge_pts(g.edge);
        gate_rows.push(GateRow {
            key: g.key.trim().to_string(),
            edge: g.edge,
            kind: g.kind.trim().to_string(),
            operation: g.operation.trim().to_string(),
            offset_m: g.offset_m,
            width_m: g.width_m,
            jambs_m: [point_on(a, b, g.offset_m), point_on(a, b, g.offset_m + g.width_m)],
            source: g.source.trim().to_string(),
        });
    }
    let mut fence_rows: Vec<FenceRow> = Vec::new();
    for (e, f) in &fenced {
        let on: Vec<&GateRow> = gate_rows.iter().filter(|g| g.edge == *e).collect();
        let total: f64 = on.iter().map(|g| g.width_m).sum();
        fence_rows.push(FenceRow {
            edge: *e,
            edge_kind: edge_kind_name(r.edges[*e].kind),
            kind: f.kind.trim().to_string(),
            height_m: f.height_m,
            thickness_m: f.thickness_m,
            edge_length_m: r2(edge_len(*e)),
            gates: on.len(),
            gate_total_m: r2(total),
            fenced_length_m: r2((edge_len(*e) - total).max(0.0)),
            source: f.source.trim().to_string(),
        });
    }
    let total_fenced = r2(fence_rows.iter().map(|f| f.fenced_length_m).sum());
    let unfenced: Vec<usize> = (0..n).filter(|i| !fenced.contains_key(i)).collect();
    let vehicle = gate_rows.iter().filter(|g| g.kind == "vehicle").count();
    let pedestrian = gate_rows.len() - vehicle;
    let title = if input.title.trim().is_empty() { "Site plan: fences and gates" } else { input.title.trim() };
    let svg = draw(r, title, input, &fence_rows, &gate_rows);
    let mut notes = vec![
        "Fence kinds, heights, thicknesses, gate positions, widths and every source are the user's entries; the application holds no figure for them. Edge numbers follow the site result's edges, from 0. Corner returns, post spacing and gate clearances against the building are not modelled, and the plan does not check what the municipality allows at any position, including inside a setback. Preliminary plan / المقاسات والمصادر من إدخال المستخدم؛ لا تُفحص أنظمة البلدية؛ مخطط أولي.".to_string(),
    ];
    if !unfenced.is_empty() {
        notes.push(format!("Edges with no fence entered: {} / أضلاع بلا سور.", unfenced.iter().map(|e| e.to_string()).collect::<Vec<_>>().join(", ")));
    }
    if gate_rows.is_empty() {
        notes.push("No gate entered: the fenced plot has no opening / لم تُدخل بوابة.".into());
    }
    Ok(Plan { fences: fence_rows, gates: gate_rows, total_fenced_length_m: total_fenced, vehicle_gates: vehicle, pedestrian_gates: pedestrian, unfenced_edges: unfenced, svg, notes })
}

fn draw(r: &SiteResult, title: &str, input: &Input, fences: &[FenceRow], gates: &[GateRow]) -> String {
    let n = r.polygon_m.len();
    let pts = || r.polygon_m.iter();
    let (minx, miny) = (pts().map(|p| p[0]).fold(f64::MAX, f64::min), pts().map(|p| p[1]).fold(f64::MAX, f64::min));
    let (maxx, maxy) = (pts().map(|p| p[0]).fold(f64::MIN, f64::max), pts().map(|p| p[1]).fold(f64::MIN, f64::max));
    let s = (620.0 / (maxx - minx).max(maxy - miny)).clamp(2.0, 40.0);
    let m = 70.0;
    let rows_h = 18.0 * (fences.len() + gates.len() + 2) as f64;
    let (w, h) = ((maxx - minx) * s + 2.0 * m, (maxy - miny) * s + 2.0 * m + 30.0 + rows_h);
    let tx = |x: f64| m + (x - minx) * s;
    let ty = |y: f64| m + (maxy - y) * s;
    let poly = |p: &[Pt]| p.iter().map(|q| format!("{:.1},{:.1}", tx(q[0]), ty(q[1]))).collect::<Vec<_>>().join(" ");
    let mut o = String::new();
    let _ = write!(o, "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w:.0} {h:.0}\" width=\"{w:.0}\" height=\"{h:.0}\" font-family=\"Segoe UI, Tahoma, Arial, sans-serif\" font-size=\"11\">");
    let _ = write!(o, "<rect width=\"100%\" height=\"100%\" fill=\"#fff\"/><text x=\"{:.1}\" y=\"24\" font-size=\"15\" font-weight=\"bold\" text-anchor=\"middle\">{}</text>", w / 2.0, esc(title));
    let _ = write!(o, "<polygon points=\"{}\" fill=\"#f4f1e8\" stroke=\"#999\" stroke-width=\"1\"/>", poly(&r.polygon_m));
    let _ = write!(o, "<polygon points=\"{}\" fill=\"#dff0d8\" stroke=\"#3c763d\" stroke-dasharray=\"6 4\"/>", poly(&r.buildable_m));
    let _ = write!(o, "<polygon points=\"{}\" fill=\"#9ec9ea\" stroke=\"#1b4f7c\" stroke-width=\"2\"/>", poly(&r.building.corners_m));
    for f in fences {
        let (a, b) = (r.polygon_m[f.edge], r.polygon_m[(f.edge + 1) % n]);
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        let mut on: Vec<&GateRow> = gates.iter().filter(|g| g.edge == f.edge).collect();
        on.sort_by(|x, y| x.offset_m.total_cmp(&y.offset_m));
        let mut cursor = 0.0;
        let mut spans: Vec<(f64, f64)> = Vec::new();
        for g in &on {
            if g.offset_m > cursor + 1e-9 {
                spans.push((cursor, g.offset_m));
            }
            cursor = g.offset_m + g.width_m;
        }
        if cursor < len - 1e-9 {
            spans.push((cursor, len));
        }
        for (d0, d1) in spans {
            let (p, q) = (point_on(a, b, d0), point_on(a, b, d1));
            let _ = write!(o, "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"#4e342e\" stroke-width=\"{:.1}\"/>", tx(p[0]), ty(p[1]), tx(q[0]), ty(q[1]), (f.thickness_m * s).max(2.5));
        }
        let (mx, my) = ((a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0);
        let (cx, cy) = ((minx + maxx) / 2.0, (miny + maxy) / 2.0);
        let (dx, dy) = (mx - cx, my - cy);
        let k = dx.hypot(dy).max(1e-9);
        let _ = write!(o, "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" fill=\"#333\">E{} · {} m · h {} m</text>", tx(mx) + dx / k * 30.0, ty(my) - dy / k * 22.0, f.edge, f.fenced_length_m, f.height_m);
    }
    for g in gates {
        let (a, b) = (r.polygon_m[g.edge], r.polygon_m[(g.edge + 1) % n]);
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        let u = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
        // The interior is on the left of a counter-clockwise edge.
        let inward = [-u[1], u[0]];
        let (j0, j1) = (g.jambs_m[0], g.jambs_m[1]);
        let colour = if g.kind == "vehicle" { "#e65100" } else { "#6a1b9a" };
        let _ = write!(o, "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"{colour}\" stroke-width=\"3\" stroke-dasharray=\"4 3\"/>", tx(j0[0]), ty(j0[1]), tx(j1[0]), ty(j1[1]));
        let sign = match g.operation.as_str() {
            "swing_in" => Some(1.0),
            "swing_out" => Some(-1.0),
            _ => None,
        };
        if let Some(sg) = sign {
            let open = [j0[0] + inward[0] * sg * g.width_m, j0[1] + inward[1] * sg * g.width_m];
            let sweep = if sg > 0.0 { 1 } else { 0 };
            let _ = write!(o, "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"{colour}\" stroke-width=\"1.5\"/>", tx(j0[0]), ty(j0[1]), tx(open[0]), ty(open[1]));
            let _ = write!(o, "<path d=\"M {:.1} {:.1} A {rr:.1} {rr:.1} 0 0 {sweep} {:.1} {:.1}\" fill=\"none\" stroke=\"{colour}\" stroke-width=\"0.8\" stroke-dasharray=\"3 2\"/>", tx(j1[0]), ty(j1[1]), tx(open[0]), ty(open[1]), rr = g.width_m * s);
        }
        let mid = [(j0[0] + j1[0]) / 2.0, (j0[1] + j1[1]) / 2.0];
        let _ = write!(o, "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" fill=\"{colour}\" font-weight=\"bold\">{} {} m</text>", tx(mid[0]) - inward[0] * 14.0, ty(mid[1]) + inward[1] * 14.0 + 4.0, esc(&g.key), g.width_m);
    }
    // Schedule.
    let mut y = m + (maxy - miny) * s + 40.0;
    let _ = write!(o, "<text x=\"{m:.0}\" y=\"{y:.0}\" font-weight=\"bold\">Fences / الأسوار</text>");
    for f in fences {
        y += 18.0;
        let _ = write!(o, "<text x=\"{m:.0}\" y=\"{y:.0}\">E{}: {} · {} m long, {} m fenced · h {} m, t {} m · {}</text>", f.edge, esc(&f.kind), f.edge_length_m, f.fenced_length_m, f.height_m, f.thickness_m, esc(&f.source));
    }
    y += 18.0;
    let _ = write!(o, "<text x=\"{m:.0}\" y=\"{y:.0}\" font-weight=\"bold\">Gates / البوابات</text>");
    for g in gates {
        y += 18.0;
        let _ = write!(o, "<text x=\"{m:.0}\" y=\"{y:.0}\">{}: {} on E{} · {} m wide at {} m{} · {}</text>", esc(&g.key), esc(&g.kind), g.edge, g.width_m, g.offset_m, if g.operation.is_empty() { String::new() } else { format!(" · {}", g.operation) }, esc(&g.source));
    }
    let _ = input;
    o.push_str("</svg>");
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::site;

    fn plot() -> SiteResult {
        site::analyse(&site::rectangle_with_setbacks(20.0, 30.0, 4.0, 3.0, 2.0)).unwrap()
    }

    fn fence(edge: usize) -> FenceIn {
        FenceIn { edge, kind: "solid wall".into(), height_m: 2.4, thickness_m: 0.2, source: "Brief 3.1, 2026-01-01".into() }
    }

    fn gate(key: &str, edge: usize, kind: &str, offset: f64, width: f64) -> GateIn {
        GateIn { key: key.into(), edge, kind: kind.into(), offset_m: offset, width_m: width, operation: String::new(), source: "Brief 3.2".into() }
    }

    fn input() -> Input {
        Input { title: String::new(), fences: (0..4).map(fence).collect(), gates: vec![gate("G1", 0, "vehicle", 4.0, 5.0), gate("P1", 0, "pedestrian", 12.0, 1.2)] }
    }

    #[test]
    fn fenced_length_is_the_edge_less_the_gates_cut_into_it() {
        let p = build(&plot(), &input()).unwrap();
        let e0 = &p.fences[0];
        assert_eq!((e0.edge_length_m, e0.gates, e0.gate_total_m, e0.fenced_length_m, e0.edge_kind), (20.0, 2, 6.2, 13.8, "front street"));
        assert_eq!(p.total_fenced_length_m, 13.8 + 30.0 + 20.0 + 30.0);
        assert_eq!((p.vehicle_gates, p.pedestrian_gates), (1, 1));
        assert!(p.unfenced_edges.is_empty());
        assert_eq!(p.gates[0].jambs_m, [[4.0, 0.0], [9.0, 0.0]]);
        assert!(p.svg.starts_with("<svg") && p.svg.ends_with("</svg>") && p.svg.contains("G1 5 m"));
    }

    #[test]
    fn unfenced_edges_and_missing_gates_are_reported_not_hidden() {
        let mut i = input();
        i.fences.truncate(2);
        i.gates.clear();
        let p = build(&plot(), &i).unwrap();
        assert_eq!(p.unfenced_edges, [2, 3]);
        assert!(p.notes.iter().any(|n| n.contains("2, 3")) && p.notes.iter().any(|n| n.contains("No gate")));
    }

    #[test]
    fn swing_gates_draw_the_leaf_and_arc() {
        let mut i = input();
        i.gates[0].operation = "swing_in".into();
        let svg = build(&plot(), &i).unwrap().svg;
        assert!(svg.contains("<path") && svg.contains("0 0 1"));
        i.gates[0].operation = "sliding".into();
        assert!(!build(&plot(), &i).unwrap().svg.contains("<path"));
    }

    #[test]
    fn missing_or_malformed_inputs_are_named() {
        let e = build(&plot(), &Input::default()).unwrap_err();
        assert!(e.contains("fences"), "{e}");
        let mut i = input();
        i.fences[0].source = " ".into();
        i.fences[1].height_m = 0.0;
        i.fences[2].edge = 1;
        i.gates[0].kind = "boat".into();
        i.gates[1].key = "G1".into();
        let e = build(&plot(), &i).unwrap_err();
        for part in ["source", "height_m and thickness_m", "more than one fence", "vehicle or pedestrian", "duplicated"] {
            assert!(e.contains(part), "{part}: {e}");
        }
        let mut i = input();
        i.gates[0].source.clear();
        i.gates[0].operation = "spin".into();
        let e = build(&plot(), &i).unwrap_err();
        assert!(e.contains("source") && e.contains("operation"), "{e}");
    }

    #[test]
    fn gates_must_fit_their_fenced_edge_and_not_overlap() {
        let mut i = input();
        i.gates[0].offset_m = 18.0;
        assert!(build(&plot(), &i).unwrap_err().contains("past the end of edge 0"));
        let mut i = input();
        i.gates[1].offset_m = 8.0;
        assert!(build(&plot(), &i).unwrap_err().contains("overlap"));
        let mut i = input();
        i.fences.remove(0);
        assert!(build(&plot(), &i).unwrap_err().contains("no fence to cut"));
        let mut i = input();
        i.gates[0].edge = 9;
        assert!(build(&plot(), &i).unwrap_err().contains("not an edge"));
    }

    #[test]
    fn text_is_escaped_in_the_drawing() {
        let mut i = input();
        i.title = "<script>x</script>".into();
        i.gates[0].key = "<b>".into();
        let svg = build(&plot(), &i).unwrap().svg;
        assert!(!svg.contains("<script>") && !svg.contains("<b>") && svg.contains("&lt;b&gt;"));
    }
}
