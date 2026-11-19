//! Architectural styles as parametric facades: extra solids on the exterior of the 3D scene, which the elevations and sections
//! draw too. A style never changes the model (walls, openings, structure): it adds visual elements, with ids `facade:<n>`, to the
//! scene built from it, so the plan, quantities and structure stay as they are.
//!
//! | style | what is added |
//! |---|---|
//! | Modern Najdi | stepped triangular merlons along the roof edge on a solid cap, vertical recessed slits between the windows |
//! | Contemporary | a cantilevered horizontal fascia at every floor, curtain glazing with mullions on the entrance facade |
//! | Neo-Classical | a plinth, pilasters at the corners and along the facade, a stepped cornice under an attic cap |
//! | Minimalist | a thin flat roof cap and a shadow reveal at every floor line |
//!
//! The sizes are DESIGN choices of this application (a motif is 0.5 m wide, a pilaster 0.35 m...), shown in `PARAMS`, not code
//! values and not structural. They are visual only.

use crate::model3d::{Box3, Scene};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    ModernNajdi,
    Contemporary,
    NeoClassical,
    Minimalist,
}

pub const ALL: [Style; 4] = [Style::ModernNajdi, Style::Contemporary, Style::NeoClassical, Style::Minimalist];

impl Style {
    pub fn key(self) -> &'static str {
        match self {
            Style::ModernNajdi => "modern_najdi",
            Style::Contemporary => "contemporary",
            Style::NeoClassical => "neo_classical",
            Style::Minimalist => "minimalist",
        }
    }

    /// `(English, Arabic)`.
    pub fn label(self) -> (&'static str, &'static str) {
        match self {
            Style::ModernNajdi => ("Modern Najdi", "نجدي حديث"),
            Style::Contemporary => ("Contemporary", "معاصر"),
            Style::NeoClassical => ("Neo-Classical", "كلاسيكي حديث"),
            Style::Minimalist => ("Minimalist", "مينيمال"),
        }
    }
}

/// The style a free-text entry names: the key, the English or Arabic label, or a keyword.
pub fn parse(text: &str) -> Option<Style> {
    let t = text.trim().to_lowercase();
    if t.is_empty() {
        return None;
    }
    let has = |ks: &[&str]| ks.iter().any(|k| t.contains(k));
    if has(&["najdi", "نجدي", "نجدى"]) {
        Some(Style::ModernNajdi)
    } else if has(&["contemporary", "معاصر", "عصري", "modern"]) {
        Some(Style::Contemporary)
    } else if has(&["neo", "classic", "كلاسيك", "كلاسيكي"]) {
        Some(Style::NeoClassical)
    } else if has(&["minimal", "مينيمال", "بسيط", "ميني"]) {
        Some(Style::Minimalist)
    } else {
        None
    }
}

/// The design sizes used, in metres, for the page's assumptions list.
pub const PARAMS: [(&str, f64); 14] = [
    ("merlon width", 0.5),
    ("merlon step height", 0.2),
    ("merlon spacing", 0.9),
    ("parapet cap height", 0.35),
    ("slit width", 0.12),
    ("slit spacing", 1.8),
    ("fascia overhang", 0.6),
    ("curtain glazing share of the facade", 0.7),
    ("mullion spacing", 1.2),
    ("plinth height", 0.6),
    ("pilaster width", 0.35),
    ("pilaster projection", 0.12),
    ("cornice depth", 0.36),
    ("minimalist roof cap", 0.25),
];

#[derive(Clone, Copy)]
enum Side {
    South,
    North,
    West,
    East,
}

impl Side {
    fn name(self) -> &'static str {
        match self {
            Side::South => "south",
            Side::North => "north",
            Side::West => "west",
            Side::East => "east",
        }
    }
}

const SIDES: [Side; 4] = [Side::South, Side::North, Side::West, Side::East];

struct Geo {
    /// Plan extent of the rooms `[x0, y0, x1, y1]`, metres.
    e: [f64; 4],
    /// Thickness of the exterior walls.
    t: f64,
    roof: f64,
    /// `(z0, storey, slab thickness above it)` of every level.
    levels: Vec<(f64, f64, f64)>,
    /// Openings on the facade: `(side, z0, z1, along0, along1)`.
    openings: Vec<(&'static str, f64, f64, f64, f64)>,
    entry: &'static str,
}

fn geo(scene: &Scene) -> Option<Geo> {
    let mut e: Option<[f64; 4]> = None;
    for b in scene.boxes.iter().filter(|b| b.kind == "Space") {
        e = Some(e.map_or([b.min[0], b.min[1], b.max[0], b.max[1]], |p| [p[0].min(b.min[0]), p[1].min(b.min[1]), p[2].max(b.max[0]), p[3].max(b.max[1])]));
    }
    let e = e?;
    let t = scene.boxes.iter().filter(|b| b.kind == "Wall" && b.class == "ext").map(|b| (b.max[0] - b.min[0]).min(b.max[1] - b.min[1])).fold(0.0, f64::max).max(0.1);
    let slabs: Vec<&Box3> = scene.boxes.iter().filter(|b| b.kind == "Slab").collect();
    let roof = slabs.iter().map(|b| b.max[2]).fold(f64::MIN, f64::max);
    if !roof.is_finite() {
        return None;
    }
    let levels: Vec<(f64, f64, f64)> = if scene.levels.is_empty() {
        let thick = slabs.iter().map(|b| b.max[2] - b.min[2]).fold(0.0, f64::max);
        vec![(0.0, scene.storey_height, thick)]
    } else {
        scene.levels.iter().map(|l| (l.z0, l.storey, slabs.iter().filter(|b| b.level == Some(l.index)).map(|b| b.max[2] - b.min[2]).fold(0.0, f64::max))).collect()
    };
    let side_of = |b: &Box3| -> Option<&'static str> {
        let (cx, cy) = ((b.min[0] + b.max[0]) / 2.0, (b.min[1] + b.max[1]) / 2.0);
        let near = |v: f64, f: f64| (v - f).abs() <= t + 0.1;
        if near(cy, e[1]) { Some("south") } else if near(cy, e[3]) { Some("north") } else if near(cx, e[0]) { Some("west") } else if near(cx, e[2]) { Some("east") } else { None }
    };
    let mut openings = Vec::new();
    let mut entry = "south";
    for b in scene.boxes.iter().filter(|b| b.kind == "Window" || b.kind == "Door") {
        if let Some(s) = side_of(b) {
            let along = if s == "south" || s == "north" { (b.min[0], b.max[0]) } else { (b.min[1], b.max[1]) };
            openings.push((s, b.min[2], b.max[2], along.0, along.1));
            if b.kind == "Door" {
                entry = s;
            }
        }
    }
    Some(Geo { e, t, roof, levels, openings, entry })
}

/// A solid on one facade: `along0..along1` across it, `out0..out1` outward from the outer face of the wall (negative: into it),
/// `z0..z1` up.
struct Piece {
    side: Side,
    along: (f64, f64),
    out: (f64, f64),
    z: (f64, f64),
    class: &'static str,
}

fn to_box(g: &Geo, p: &Piece, n: usize) -> Box3 {
    let (min, max) = match p.side {
        Side::South => {
            let y = g.e[1] - g.t / 2.0;
            ([p.along.0, y - p.out.1, p.z.0], [p.along.1, y - p.out.0, p.z.1])
        }
        Side::North => {
            let y = g.e[3] + g.t / 2.0;
            ([p.along.0, y + p.out.0, p.z.0], [p.along.1, y + p.out.1, p.z.1])
        }
        Side::West => {
            let x = g.e[0] - g.t / 2.0;
            ([x - p.out.1, p.along.0, p.z.0], [x - p.out.0, p.along.1, p.z.1])
        }
        Side::East => {
            let x = g.e[2] + g.t / 2.0;
            ([x + p.out.0, p.along.0, p.z.0], [x + p.out.1, p.along.1, p.z.1])
        }
    };
    Box3 { id: format!("facade:{n}"), kind: "Facade", class: format!("{}:{}", p.class, p.side.name()), min, max, visual: true, level: None, discipline: "architectural" }
}

/// The facade solids of `style` for the building in `scene` (empty when the scene has no rooms or slabs).
pub fn facade_boxes(scene: &Scene, style: Style) -> Vec<Box3> {
    let Some(g) = geo(scene) else { return Vec::new() };
    let mut pieces: Vec<Piece> = Vec::new();
    for side in SIDES {
        let along = match side {
            Side::South | Side::North => (g.e[0] - g.t / 2.0, g.e[2] + g.t / 2.0),
            Side::West | Side::East => (g.e[1] - g.t / 2.0, g.e[3] + g.t / 2.0),
        };
        let len = along.1 - along.0;
        let mut add = |a0: f64, a1: f64, o0: f64, o1: f64, z0: f64, z1: f64, class: &'static str| pieces.push(Piece { side, along: (a0, a1), out: (o0, o1), z: (z0, z1), class });
        match style {
            Style::ModernNajdi => {
                let cap = 0.35;
                add(along.0, along.1, -g.t, 0.0, g.roof, g.roof + cap, "facade-cap");
                // Stepped triangular merlons: three steps, each narrower and 0.2 m higher.
                let n = (len / 0.9).floor().max(1.0);
                let pitch = len / n;
                for k in 0..n as usize {
                    let c = along.0 + pitch * (k as f64 + 0.5);
                    for (i, w) in [0.5, 0.34, 0.18].into_iter().enumerate() {
                        add(c - w / 2.0, c + w / 2.0, -g.t, 0.0, g.roof + cap + 0.2 * i as f64, g.roof + cap + 0.2 * (i as f64 + 1.0), "facade-merlon");
                    }
                }
                // Recessed vertical slits between the openings, on every floor.
                for &(z0, storey, _) in &g.levels {
                    let h = (storey - 1.2).clamp(0.4, 1.4);
                    let n = (len / 1.8).floor().max(1.0);
                    let pitch = len / n;
                    for k in 0..n as usize {
                        let c = along.0 + pitch * (k as f64 + 0.5);
                        let (a0, a1) = (c - 0.06, c + 0.06);
                        let clear = !g.openings.iter().any(|&(s, oz0, oz1, o0, o1)| s == side.name() && oz1 > z0 + 0.5 && oz0 < z0 + 0.9 + h && o1 > a0 - 0.3 && o0 < a1 + 0.3);
                        if clear {
                            add(a0, a1, -0.03, 0.01, z0 + 0.9, z0 + 0.9 + h, "facade-slit");
                        }
                    }
                }
            }
            Style::Contemporary => {
                for &(z0, storey, slab) in &g.levels {
                    let top = z0 + storey;
                    add(along.0 - 0.3, along.1 + 0.3, 0.0, 0.6, top, top + slab.max(0.15), "facade-cantilever");
                }
                if side.name() == g.entry {
                    let (c, w) = ((along.0 + along.1) / 2.0, len * 0.7);
                    for &(z0, storey, _) in &g.levels {
                        add(c - w / 2.0, c + w / 2.0, 0.02, 0.06, z0 + 0.3, z0 + storey - 0.3, "facade-curtain");
                        let n = (w / 1.2).floor().max(1.0) as usize;
                        for k in 0..=n {
                            let x = c - w / 2.0 + w * k as f64 / n as f64;
                            add(x - 0.03, x + 0.03, 0.02, 0.1, z0 + 0.3, z0 + storey - 0.3, "facade-mullion");
                        }
                    }
                }
            }
            Style::NeoClassical => {
                add(along.0, along.1, 0.0, 0.08, 0.0, 0.6, "facade-plinth");
                let n = (len / 3.6).round().max(1.0);
                for k in 0..=n as usize {
                    let c = if k == 0 { along.0 + 0.2 } else if k == n as usize { along.1 - 0.2 } else { along.0 + len * k as f64 / n };
                    add(c - 0.175, c + 0.175, 0.0, 0.12, 0.6, g.roof - 0.36, "facade-pilaster");
                }
                for (i, depth) in [0.12, 0.24, 0.36].into_iter().enumerate() {
                    add(along.0 - depth, along.1 + depth, 0.0, depth, g.roof - 0.36 + 0.12 * i as f64, g.roof - 0.24 + 0.12 * i as f64, "facade-cornice");
                }
                add(along.0, along.1, -g.t, 0.0, g.roof, g.roof + 0.3, "facade-cap");
            }
            Style::Minimalist => {
                add(along.0, along.1, -g.t, 0.0, g.roof, g.roof + 0.25, "facade-cap");
                for &(z0, storey, _) in &g.levels {
                    add(along.0, along.1, -0.03, 0.0, z0 + storey - 0.02, z0 + storey + 0.02, "facade-reveal");
                }
            }
        }
    }
    pieces.iter().enumerate().map(|(n, p)| to_box(&g, p, n + 1)).collect()
}

/// The side a facade solid belongs to (`south`, `north`, `west`, `east`) and its motif, from its class (`facade-merlon:south`).
pub fn split_class(class: &str) -> Option<(&str, &str)> {
    class.strip_prefix("facade-").and_then(|r| r.split_once(':'))
}
