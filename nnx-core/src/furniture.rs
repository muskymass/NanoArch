//! Parametric furniture footprints per room type and the walkway clearance between them.
//!
//! The footprints are DESIGN sizes of this application (`PARAMS`), not code values: they only show how a room could be used. The
//! walkway limit is an input (`meta.furniture.minClearMm`, entered by the user; the page offers 900 mm as its starting value):
//! any passage between two pieces, or between a piece and a wall, that is open but narrower than it is flagged. A piece touching
//! a wall or another piece leaves no passage and is not flagged. Doors and their swing are not modelled.

use crate::model3d::{Box3, Scene};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FurnitureInput {
    /// Smallest clear walkway, millimetres. Required: nothing is assumed.
    pub min_clear_mm: Option<f64>,
}

/// Design sizes in metres: `(name, value)`.
pub const PARAMS: [(&str, f64); 12] = [
    ("bed width", 1.6),
    ("bed length", 2.0),
    ("seat depth (majlis)", 0.8),
    ("sofa depth (living)", 0.9),
    ("sofa length (living)", 3.0),
    ("coffee table width", 0.6),
    ("coffee table length", 1.2),
    ("dining table width", 1.0),
    ("dining table length", 1.8),
    ("kitchen counter depth", 0.6),
    ("bed height", 0.5),
    ("seat height", 0.45),
];

#[derive(Debug, Clone, Serialize)]
pub struct Piece {
    pub room: String,
    pub func: String,
    pub kind: &'static str,
    /// `[x0, y0, x1, y1]`, metres.
    pub rect: [f64; 4],
    pub height_m: f64,
    pub level: Option<i64>,
    pub z0: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Flag {
    pub room: String,
    pub func: String,
    pub what: String,
    pub gap_mm: f64,
    pub min_mm: f64,
    pub level: Option<i64>,
    /// The gap as a segment `[x0, y0, x1, y1]` for drawing.
    pub at: [f64; 4],
}

#[derive(Debug, Clone, Serialize)]
pub struct RoomRect {
    pub id: String,
    pub func: String,
    pub rect: [f64; 4],
    pub level: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub min_clear_mm: f64,
    pub rooms: Vec<RoomRect>,
    pub pieces: Vec<Piece>,
    pub flags: Vec<Flag>,
}

type R4 = [f64; 4];

fn place(func: &str, room: R4) -> Vec<(&'static str, R4, f64)> {
    let (w, h) = (room[2] - room[0], room[3] - room[1]);
    // Work with the long side along x; transpose back at the end.
    let tr = h > w;
    let (lw, lh) = if tr { (h, w) } else { (w, h) };
    let mut v: Vec<(&'static str, R4, f64)> = Vec::new();
    match func {
        "bedroom" => {
            let cy = lh / 2.0;
            v.push(("bed", [0.0, cy - 0.8, 2.0, cy + 0.8], 0.5));
        }
        "majlis" => {
            v.push(("seating", [0.0, lh - 0.8, lw, lh], 0.45));
            v.push(("seating", [0.0, 0.0, 0.8, lh - 0.8], 0.45));
            v.push(("seating", [lw - 0.8, 0.0, lw, lh - 0.8], 0.45));
        }
        "living" => {
            let len = lw.min(3.0);
            let x0 = (lw - len) / 2.0;
            v.push(("sofa", [x0, lh - 0.9, x0 + len, lh], 0.45));
            let (cx, y1) = (lw / 2.0, lh - 0.9 - 1.0);
            v.push(("coffee table", [cx - 0.6, y1 - 0.6, cx + 0.6, y1], 0.4));
        }
        "dining" => {
            let (cx, cy) = (lw / 2.0, lh / 2.0);
            v.push(("dining table", [cx - 0.9, cy - 0.5, cx + 0.9, cy + 0.5], 0.75));
        }
        "kitchen" => {
            v.push(("counter", [0.0, lh - 0.6, lw, lh], 0.9));
        }
        _ => {}
    }
    v.into_iter()
        .map(|(k, r, hgt)| {
            let r = if tr { [r[1], r[0], r[3], r[2]] } else { r };
            (k, [room[0] + r[0], room[1] + r[1], room[0] + r[2], room[1] + r[3]], hgt)
        })
        .collect()
}

fn gap(a: R4, b: R4) -> Option<(f64, R4)> {
    // Separation along one axis when the pieces overlap on the other.
    let ox = a[0] < b[2] - 1e-9 && b[0] < a[2] - 1e-9;
    let oy = a[1] < b[3] - 1e-9 && b[1] < a[3] - 1e-9;
    if ox && !oy {
        let (lo, hi) = if a[3] <= b[1] { (a, b) } else { (b, a) };
        let (x0, x1) = (a[0].max(b[0]), a[2].min(b[2]));
        let xm = (x0 + x1) / 2.0;
        return Some((hi[1] - lo[3], [xm, lo[3], xm, hi[1]]));
    }
    if oy && !ox {
        let (lo, hi) = if a[2] <= b[0] { (a, b) } else { (b, a) };
        let (y0, y1) = (a[1].max(b[1]), a[3].min(b[3]));
        let ym = (y0 + y1) / 2.0;
        return Some((hi[0] - lo[2], [lo[2], ym, hi[0], ym]));
    }
    None
}

pub fn plan(scene: &Scene, input: &FurnitureInput) -> Result<Report, String> {
    let min = input.min_clear_mm.filter(|m| m.is_finite() && *m > 0.0).ok_or("the minimum clear walkway (mm) is missing: enter it")?;
    let min_m = min / 1000.0;
    let mut rooms = Vec::new();
    let mut pieces = Vec::new();
    let mut flags = Vec::new();
    for b in scene.boxes.iter().filter(|b| b.kind == "Space") {
        let room: R4 = [b.min[0], b.min[1], b.max[0], b.max[1]];
        let z0 = b.level.and_then(|l| scene.levels.iter().find(|x| x.index == l)).map_or(0.0, |l| l.z0);
        rooms.push(RoomRect { id: b.id.clone(), func: b.class.clone(), rect: room, level: b.level });
        let mine: Vec<(&'static str, R4, f64)> = place(&b.class, room);
        let flag = |what: String, g: f64, at: R4| Flag { room: b.id.clone(), func: b.class.clone(), what, gap_mm: (g * 1000.0).round(), min_mm: min, level: b.level, at };
        for (i, (kind, r, h)) in mine.iter().enumerate() {
            pieces.push(Piece { room: b.id.clone(), func: b.class.clone(), kind, rect: *r, height_m: *h, level: b.level, z0 });
            if r[0] < room[0] - 1e-6 || r[1] < room[1] - 1e-6 || r[2] > room[2] + 1e-6 || r[3] > room[3] + 1e-6 {
                flags.push(flag(format!("{kind} does not fit in the room"), 0.0, *r));
                continue;
            }
            // Open passages between the piece and each wall.
            for (g, at, side) in [(r[0] - room[0], [room[0], (r[1] + r[3]) / 2.0, r[0], (r[1] + r[3]) / 2.0], "west"), (room[2] - r[2], [r[2], (r[1] + r[3]) / 2.0, room[2], (r[1] + r[3]) / 2.0], "east"), (r[1] - room[1], [(r[0] + r[2]) / 2.0, room[1], (r[0] + r[2]) / 2.0, r[1]], "south"), (room[3] - r[3], [(r[0] + r[2]) / 2.0, r[3], (r[0] + r[2]) / 2.0, room[3]], "north")] {
                if g > 1e-6 && g < min_m - 1e-9 {
                    flags.push(flag(format!("{kind}: passage to the {side} wall is {:.0} mm", g * 1000.0), g, at));
                }
            }
            for (_, r2, _) in mine.iter().skip(i + 1) {
                if let Some((g, at)) = gap(*r, *r2)
                    && g > 1e-6
                    && g < min_m - 1e-9
                {
                    flags.push(flag(format!("{kind}: passage between two pieces is {:.0} mm", g * 1000.0), g, at));
                }
            }
        }
    }
    Ok(Report { min_clear_mm: min, rooms, pieces, flags })
}

/// 3D solids of the pieces.
pub fn boxes(rep: &Report) -> Vec<Box3> {
    rep.pieces
        .iter()
        .enumerate()
        .map(|(n, p)| Box3 { id: format!("furn:{}", n + 1), kind: "Furniture", class: p.kind.to_string(), min: [p.rect[0], p.rect[1], p.z0 + 0.02], max: [p.rect[2], p.rect[3], p.z0 + 0.02 + p.height_m], visual: true, level: p.level, discipline: "architectural" })
        .collect()
}

/// A plan of one level (`None`: the first) with the furniture and the narrow passages in red.
pub fn svg(rep: &Report, level: Option<i64>) -> String {
    use std::fmt::Write;
    let lv = level.or_else(|| rep.rooms.iter().filter_map(|r| r.level).min());
    let rooms: Vec<&RoomRect> = rep.rooms.iter().filter(|r| r.level == lv).collect();
    let (mut x1, mut y1) = (1.0f64, 1.0f64);
    for r in &rooms {
        x1 = x1.max(r.rect[2]);
        y1 = y1.max(r.rect[3]);
    }
    let s = 40.0;
    let m = 30.0;
    let (w, h) = (x1 * s + 2.0 * m, y1 * s + 2.0 * m + 20.0);
    let tx = |x: f64| m + x * s;
    let ty = |y: f64| m + (y1 - y) * s;
    let mut o = String::new();
    let _ = writeln!(o, "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w:.0} {h:.0}\" width=\"{w:.0}\" height=\"{h:.0}\"><rect width=\"100%\" height=\"100%\" fill=\"#fff\"/>");
    for r in &rooms {
        let _ = writeln!(o, "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" style=\"fill:#fafafa;stroke:#333;stroke-width:1.5\"/><text x=\"{:.1}\" y=\"{:.1}\" style=\"font:10px sans-serif;fill:#777\">{}</text>", tx(r.rect[0]), ty(r.rect[3]), (r.rect[2] - r.rect[0]) * s, (r.rect[3] - r.rect[1]) * s, tx(r.rect[0]) + 3.0, ty(r.rect[3]) + 11.0, r.func);
    }
    for p in rep.pieces.iter().filter(|p| p.level == lv) {
        let _ = writeln!(o, "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" style=\"fill:#d7ccc8;stroke:#5d4037;stroke-width:1\" class=\"furniture\"/>", tx(p.rect[0]), ty(p.rect[3]), (p.rect[2] - p.rect[0]) * s, (p.rect[3] - p.rect[1]) * s);
    }
    for f in rep.flags.iter().filter(|f| f.level == lv) {
        let _ = writeln!(o, "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" class=\"narrow\" style=\"stroke:#d32f2f;stroke-width:3\"><title>{} (min {} mm)</title></line>", tx(f.at[0]), ty(f.at[1]), tx(f.at[2]), ty(f.at[3]), f.what.replace('&', "&amp;").replace('<', "&lt;"), f.min_mm);
    }
    let _ = writeln!(o, "<text x=\"{m}\" y=\"{:.0}\" style=\"font:11px sans-serif;fill:#333\">minimum clear walkway {} mm · {} narrow passage(s)</text></svg>", h - 8.0, rep.min_clear_mm, rep.flags.iter().filter(|f| f.level == lv).count());
    o
}
