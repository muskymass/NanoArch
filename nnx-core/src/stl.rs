//! Binary STL export of the 3D scene for physical architectural models, at a scale you choose (1:100 means 100).
//!
//! - **Solids**: the walls, slabs, columns, beams, footings, tie beams, facade pieces, site solids and basement/annex volumes of the
//!   scene; doors, windows, rooms, MEP, furniture and coverage are not printed. Each solid is a closed box of 12 triangles with outward
//!   normals, so every solid is watertight (`each_solid_closed` checks every edge of a solid is shared by exactly two triangles). Overlapping solids are
//!   NOT merged by boolean union: a slicer fuses them when printing.
//! - **Units**: the file is in millimetres of the MODEL: scene metres x 1000 / scale. The origin is the building's lowest south-west
//!   corner so the model sits on the print bed (z >= 0 is kept: a basement is cut off at the bed level only if you ask for one floor).
//! - **Floor by floor**: `level = Some(i)` exports only the solids of that level (those without a level go with the first level), so
//!   each floor can be printed and stacked apart; `None` exports everything.
//! - **Thin parts**: with `minFeatureMm` (your printer's smallest wall) the solids thinner than it at the model scale are counted and
//!   named in the report; none is changed.

use crate::model3d::{Box3, Scene};
use serde::Serialize;
use std::collections::HashMap;

pub const KINDS: [&str; 9] = ["Wall", "Slab", "Column", "Beam", "Footing", "TieBeam", "Facade", "Site", "Vertical"];

#[derive(Debug, Clone, Default)]
pub struct StlOpts {
    /// The scale denominator (100 for 1:100).
    pub scale: Option<f64>,
    pub level: Option<i64>,
    pub min_feature_mm: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct StlReport {
    pub solids: usize,
    pub triangles: usize,
    pub scale: f64,
    /// Size of the model, mm.
    pub size_mm: [f64; 3],
    pub thin: Vec<String>,
    pub notes: Vec<String>,
}

pub type Tri = [[f32; 3]; 3];

fn quad(out: &mut Vec<Tri>, a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]) {
    out.push([a, b, c]);
    out.push([a, c, d]);
}

/// The 12 triangles of a box, counter-clockwise seen from outside.
pub fn box_triangles(min: [f32; 3], max: [f32; 3]) -> Vec<Tri> {
    let [x0, y0, z0] = min;
    let [x1, y1, z1] = max;
    let mut t = Vec::with_capacity(12);
    quad(&mut t, [x0, y0, z0], [x0, y1, z0], [x1, y1, z0], [x1, y0, z0]); // bottom, normal -z
    quad(&mut t, [x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]); // top, +z
    quad(&mut t, [x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]); // south, -y
    quad(&mut t, [x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]); // east, +x
    quad(&mut t, [x1, y1, z0], [x0, y1, z0], [x0, y1, z1], [x1, y1, z1]); // north, +y
    quad(&mut t, [x0, y1, z0], [x0, y0, z0], [x0, y0, z1], [x0, y1, z1]); // west, -x
    t
}

fn wanted(b: &Box3, first_level: Option<i64>, level: Option<i64>) -> bool {
    if !KINDS.contains(&b.kind) {
        return false;
    }
    match level {
        None => true,
        Some(l) => b.level == Some(l) || (b.level.is_none() && Some(l) == first_level),
    }
}

pub fn triangles(scene: &Scene, opts: &StlOpts) -> Result<(Vec<Tri>, StlReport), String> {
    let scale = opts.scale.filter(|s| s.is_finite() && *s > 0.0).ok_or("the scale (for 1:100 enter 100) is missing")?;
    let first = scene.levels.iter().map(|l| l.index).min();
    if let Some(l) = opts.level
        && !scene.levels.iter().any(|x| x.index == l)
    {
        return Err(format!("the model has no level {l}"));
    }
    let boxes: Vec<&Box3> = scene.boxes.iter().filter(|b| wanted(b, first, opts.level) && b.max.iter().zip(&b.min).all(|(h, l)| h > l)).collect();
    if boxes.is_empty() {
        return Err("nothing to export: the model has no printable solids".into());
    }
    let lo = boxes.iter().fold([f64::MAX; 3], |a, b| [a[0].min(b.min[0]), a[1].min(b.min[1]), a[2].min(b.min[2])]);
    let hi = boxes.iter().fold([f64::MIN; 3], |a, b| [a[0].max(b.max[0]), a[1].max(b.max[1]), a[2].max(b.max[2])]);
    let k = 1000.0 / scale;
    // The lowest point of the solids sits on the bed; x and y start at the south-west corner.
    let f = |p: [f64; 3]| [((p[0] - lo[0]) * k) as f32, ((p[1] - lo[1]) * k) as f32, ((p[2] - lo[2]) * k) as f32];
    let mut tris = Vec::new();
    let mut rep = StlReport { solids: boxes.len(), scale, size_mm: [(hi[0] - lo[0]) * k, (hi[1] - lo[1]) * k, (hi[2] - lo[2]) * k].map(|v| (v * 100.0).round() / 100.0), ..Default::default() };
    for b in &boxes {
        tris.extend(box_triangles(f(b.min), f(b.max)));
        if let Some(m) = opts.min_feature_mm {
            let thin = (0..3).map(|i| (b.max[i] - b.min[i]) * k).fold(f64::MAX, f64::min);
            if thin < m {
                rep.thin.push(format!("{} {} ({:.2} mm)", b.kind, b.id, thin));
            }
        }
    }
    rep.triangles = tris.len();
    if opts.min_feature_mm.is_none() {
        rep.notes.push("no smallest printable wall entered: thin parts are not checked".into());
    }
    if !rep.thin.is_empty() {
        rep.notes.push(format!("{} solid(s) are thinner than your printer's limit at this scale: they may not print", rep.thin.len()));
    }
    rep.notes.push("each solid is closed; overlapping solids are not merged here (a slicer fuses them)".into());
    Ok((tris, rep))
}

/// Binary STL: 80-byte header, the triangle count, then per triangle the normal, three vertices and a zero attribute.
pub fn to_binary(tris: &[Tri], header: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(84 + tris.len() * 50);
    let mut h = [0u8; 80];
    for (i, b) in header.bytes().take(80).enumerate() {
        h[i] = b;
    }
    out.extend_from_slice(&h);
    out.extend_from_slice(&(tris.len() as u32).to_le_bytes());
    for t in tris {
        let u = [t[1][0] - t[0][0], t[1][1] - t[0][1], t[1][2] - t[0][2]];
        let v = [t[2][0] - t[0][0], t[2][1] - t[0][1], t[2][2] - t[0][2]];
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        let n = if l > 0.0 { [n[0] / l, n[1] / l, n[2] / l] } else { [0.0; 3] };
        for c in n.iter().chain(t.iter().flatten()) {
            out.extend_from_slice(&c.to_le_bytes());
        }
        out.extend_from_slice(&[0, 0]);
    }
    out
}

/// True when every solid (a run of 12 triangles) is a closed, consistently oriented mesh: each directed edge appears once and its
/// reverse appears once.
pub fn each_solid_closed(tris: &[Tri]) -> bool {
    let key = |p: [f32; 3]| [(p[0] * 1000.0).round() as i64, (p[1] * 1000.0).round() as i64, (p[2] * 1000.0).round() as i64];
    tris.len().is_multiple_of(12)
        && tris.chunks(12).all(|solid| {
            let mut dir: HashMap<([i64; 3], [i64; 3]), i32> = HashMap::new();
            for t in solid {
                for i in 0..3 {
                    *dir.entry((key(t[i]), key(t[(i + 1) % 3]))).or_default() += 1;
                }
            }
            dir.iter().all(|(&(a, b), &n)| n == 1 && dir.get(&(b, a)) == Some(&1))
        })
}
