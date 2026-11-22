//! Elevations and cross-sections (SVG), drawn from the 3D scene (`model3d`) of the model, so they follow every edit and
//! carry the same element ids (`data-id`) as the plan and the 3D view.
//!
//! - **Elevation** of one side (north, south, east or west): the exterior walls on that side with their window panes and
//!   doors, the edges of the floor slabs, the optional roof parapet, the ground line, the level marks and the heights.
//! - **Section** `A-A` (a plane of constant y, looking north: east is on the right) or `B-B` (a plane of constant x,
//!   looking west: north is on the right): everything the plane cuts is filled dark, everything beyond it is drawn light.
//!
//! North is +y, east is +x, heights are metres above the top of the ground floor slab (the level the model calls
//! ground); no site datum is assumed. The parapet height is an input (`ViewOpts::parapet_m`); it is not defaulted.

use crate::model3d::{self, Box3, Scene};
use crate::model::Model;
use std::fmt::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    North,
    South,
    East,
    West,
}

impl Side {
    pub fn parse(s: &str) -> Option<Side> {
        match s {
            "north" => Some(Side::North),
            "south" => Some(Side::South),
            "east" => Some(Side::East),
            "west" => Some(Side::West),
            _ => None,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Side::North => "North elevation",
            Side::South => "South elevation",
            Side::East => "East elevation",
            Side::West => "West elevation",
        }
    }
}

/// Where a section is cut.
#[derive(Debug, Clone, PartialEq)]
pub enum Cut {
    /// A-A: the plane y = `at` metres, looking north.
    AtY { name: String, at: f64 },
    /// B-B: the plane x = `at` metres, looking west.
    AtX { name: String, at: f64 },
}

#[derive(Debug, Clone)]
pub struct ViewOpts {
    pub title: String,
    pub px_per_m: f64,
    pub margin_px: f64,
    /// Height of the roof parapet in metres. `None` draws none (it is not defaulted).
    pub parapet_m: Option<f64>,
    /// The architectural style whose facade solids are drawn.
    pub style: Option<crate::style::Style>,
}

impl Default for ViewOpts {
    fn default() -> Self {
        Self { title: String::new(), px_per_m: 50.0, margin_px: 70.0, parapet_m: None, style: None }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ViewError {
    #[error("the model has no rooms to draw")]
    Empty,
    #[error("the section plane at {at} m is outside the building ({lo} to {hi} m)")]
    OutsideBuilding { at: f64, lo: f64, hi: f64 },
    #[error("the parapet height must be a positive number of metres")]
    BadParapet,
}

/// Plan extent of the building (from the rooms): `[x0, y0, x1, y1]` in metres.
fn extent(scene: &Scene) -> Option<[f64; 4]> {
    let mut e: Option<[f64; 4]> = None;
    for b in scene.boxes.iter().filter(|b| b.kind == "Space") {
        e = Some(e.map_or([b.min[0], b.min[1], b.max[0], b.max[1]], |p| [p[0].min(b.min[0]), p[1].min(b.min[1]), p[2].max(b.max[0]), p[3].max(b.max[1])]));
    }
    e
}

/// The default cut position: the middle of the building (A-A across its depth, B-B across its width).
pub fn default_cut(model: &Model, name: &str) -> Option<Cut> {
    let e = extent(&model3d::build(model))?;
    match name {
        "A-A" => Some(Cut::AtY { name: "A-A".into(), at: (e[1] + e[3]) / 2.0 }),
        "B-B" => Some(Cut::AtX { name: "B-B".into(), at: (e[0] + e[2]) / 2.0 }),
        _ => None,
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const STYLE: &str = r##"<style>
 .bg{fill:#fff} .ground{stroke:#222;stroke-width:1.6} .hatch{stroke:#999;stroke-width:.7}
 .wall{fill:#e9e4d8;stroke:#2b2b2b;stroke-width:1} .wall.cut{fill:#2b2b2b} .wall.far{fill:#f1eee6;stroke:#9a958a;stroke-width:.6}
 .slab{fill:#cfcac0;stroke:#2b2b2b;stroke-width:1} .slab.cut{fill:#555} .slab.far{fill:#ddd9d0;stroke:#9a958a;stroke-width:.6}
 .glass{fill:#cfe6f3;stroke:#2a6f97;stroke-width:1} .leaf{fill:#b88a5a;stroke:#5a3b1c;stroke-width:1}
 .column{fill:#b5451b;stroke:#7a2d10;stroke-width:.8} .column.far{fill:#e3a48d} .beam{fill:#d9822b;stroke:#8a4e12;stroke-width:.8} .beam.far{fill:#f0c590}
 .footing{fill:#8d8d8d;stroke:#444;stroke-width:.8} .duct{fill:#7fc4d4;stroke:#0b7285;stroke-width:.8} .pipe{fill:#8fd19e;stroke:#2b7a3d;stroke-width:.8} .drain{fill:#b9a07c;stroke:#6b4f2a;stroke-width:.8}
 .parapet{fill:#d6d1c6;stroke:#2b2b2b;stroke-width:1}
 .facade-cap{fill:#cfc6b4;stroke:#2b2b2b;stroke-width:.8} .facade-merlon{fill:#c9bda6;stroke:#2b2b2b;stroke-width:.8} .facade-slit{fill:#3a342c;stroke:#111;stroke-width:.4}
 .facade-cantilever{fill:#bfc5c9;stroke:#2b2b2b;stroke-width:1} .facade-curtain{fill:#bfe0f0;fill-opacity:.55;stroke:#2a6f97;stroke-width:.6} .facade-mullion{fill:#4a5a66}
 .facade-plinth{fill:#b9ad96;stroke:#2b2b2b;stroke-width:.8} .facade-pilaster{fill:#e4dccb;stroke:#2b2b2b;stroke-width:.8} .facade-cornice{fill:#d9cfba;stroke:#2b2b2b;stroke-width:.8} .facade-reveal{fill:#555}
 .lvl{stroke:#1b6ca8;stroke-width:.8;stroke-dasharray:8 4} .lvltxt{font-size:11px;fill:#1b6ca8}
 .dim{stroke:#444;stroke-width:.8;fill:none} .dimtxt{font-size:11px;fill:#222;text-anchor:middle}
 .grid{stroke:#c0392b;stroke-width:.8;stroke-dasharray:14 3 2 3;fill:none} .gbub{fill:#fff;stroke:#c0392b;stroke-width:1} .gtxt{font-size:11px;font-weight:600;fill:#c0392b;text-anchor:middle}
 .title{font-size:16px;font-weight:700;fill:#111} .note{font-size:10.5px;fill:#a33}
</style>"##;

struct Frame {
    s: f64,
    m: f64,
    h0: f64,
    zmax: f64,
}

impl Frame {
    fn x(&self, h: f64) -> f64 {
        self.m + (h - self.h0) * self.s
    }
    fn y(&self, z: f64) -> f64 {
        self.m + (self.zmax - z) * self.s
    }
}

/// One rectangle of a view in view coordinates: `h0..h1` across (metres) and `z0..z1` up (metres), with the class it is drawn
/// in (`wall`, `wall cut`, `wall far`, `glass`, `leaf`, `slab`, `column`, `beam`, `footing`, `duct`, `pipe`, `drain`, `cable`,
/// `parapet`) and the id of the model element it comes from.
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub id: Option<String>,
    pub class: String,
    pub h0: f64,
    pub h1: f64,
    pub z0: f64,
    pub z1: f64,
}

/// A drawn view as geometry, for exports other than SVG.
#[derive(Debug, Clone)]
pub struct ViewShapes {
    pub title: String,
    pub subtitle: String,
    pub shapes: Vec<Shape>,
    pub h_range: (f64, f64),
    pub top: f64,
    pub bottom: f64,
    /// Level names with their elevation above the ground floor top, metres.
    pub levels: Vec<(String, f64)>,
    /// Grid lines seen in the view: name and position across the view (metres).
    pub grids: Vec<(String, f64)>,
}

/// What an export asks for: an elevation or a section.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewSpec {
    Elevation(Side),
    Section(Cut),
}

struct Canvas {
    o: String,
    f: Frame,
    shapes: Vec<Shape>,
    /// Grid lines seen in this view: name and position across the view (metres).
    grids: Vec<(String, f64)>,
}

impl Canvas {
    fn rect(&mut self, class: &str, id: Option<&str>, h0: f64, h1: f64, z0: f64, z1: f64) {
        let (x, y) = (self.f.x(h0), self.f.y(z1));
        let (w, h) = ((h1 - h0) * self.f.s, (z1 - z0) * self.f.s);
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        self.shapes.push(Shape { id: id.map(str::to_string), class: class.to_string(), h0, h1, z0, z1 });
        match id {
            Some(id) => {
                let _ = writeln!(self.o, r#"<rect class="{class}" data-id="{}" x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}"/>"#, esc(id));
            }
            None => {
                let _ = writeln!(self.o, r#"<rect class="{class}" x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}"/>"#);
            }
        }
    }
}

/// Levels as `(name, z0)`; one unnamed ground level for a model without levels.
fn level_marks(scene: &Scene) -> Vec<(String, f64)> {
    if scene.levels.is_empty() { vec![("Ground".into(), 0.0)] } else { scene.levels.iter().map(|l| (l.name.clone(), l.z0)).collect() }
}

fn wall_thickness(scene: &Scene) -> f64 {
    scene.boxes.iter().filter(|b| b.kind == "Wall" && b.class == "ext").map(|b| (b.max[0] - b.min[0]).min(b.max[1] - b.min[1])).fold(0.0, f64::max)
}

/// Everything the drawings share: the frame, the ground line, the level marks, the heights and the title. `h_range` is
/// the horizontal extent of the building in the view, `top` the highest z drawn.
fn begin(opts: &ViewOpts, h_range: (f64, f64), top: f64, bottom: f64) -> Canvas {
    let s = opts.px_per_m;
    let m = opts.margin_px;
    let pad = 1.5;
    let (h0, h1) = (h_range.0 - pad, h_range.1 + pad);
    let title_h = 80.0;
    let width = 2.0 * m + (h1 - h0) * s;
    // Headroom above the building so the top dimension and labels are not on the border.
    let headroom = 0.4;
    let height = 2.0 * m + (top + headroom - bottom) * s + title_h;
    let f = Frame { s, m, h0, zmax: top + headroom };
    let mut o = String::new();
    let _ = write!(o, "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {width:.1} {height:.1}\" width=\"{width:.0}\" height=\"{height:.0}\" font-family=\"Segoe UI, Tahoma, Arial, sans-serif\">\n{STYLE}\n<rect class=\"bg\" width=\"{width:.1}\" height=\"{height:.1}\"/>\n");
    Canvas { o, f, shapes: Vec::new(), grids: Vec::new() }
}

/// Ground line with hatching, level marks, heights, title block. Call last (it needs the final extents).
#[allow(clippy::too_many_arguments)]
fn finish(mut c: Canvas, scene: &Scene, title: &str, subtitle: &str, h_range: (f64, f64), top: f64, bottom: f64) -> (String, ViewShapes) {
    let (h0, h1) = (h_range.0 - 1.5, h_range.1 + 1.5);
    let f = &c.f;
    let (gx0, gx1, gy) = (f.x(h0), f.x(h1), f.y(0.0));
    let mut o = String::new();
    for (name, h) in &c.grids {
        let (x, y0, y1) = (f.x(*h), f.y(bottom), f.y(top + 0.3));
        let _ = writeln!(o, r#"<line class="grid" x1="{x:.1}" y1="{y0:.1}" x2="{x:.1}" y2="{y1:.1}"/><circle class="gbub" cx="{x:.1}" cy="{:.1}" r="12"/><text class="gtxt" x="{x:.1}" y="{:.1}">{}</text>"#, y1 - 12.0, y1 - 8.0, esc(name));
    }
    let _ = writeln!(o, r#"<line class="ground" x1="{gx0:.1}" y1="{gy:.1}" x2="{gx1:.1}" y2="{gy:.1}"/>"#);
    let mut hx = gx0;
    while hx < gx1 {
        let _ = writeln!(o, r#"<line class="hatch" x1="{hx:.1}" y1="{gy:.1}" x2="{:.1}" y2="{:.1}"/>"#, hx - 8.0, gy + 8.0);
        hx += 10.0;
    }
    // Level marks relative to the ground floor's top.
    let marks = level_marks(scene);
    let tx = f.x(h1) - 4.0;
    for (name, z) in &marks {
        let y = f.y(*z);
        let label = if *z == 0.0 { format!("{name} ±0.00") } else { format!("{name} +{z:.2}") };
        let _ = writeln!(o, r#"<line class="lvl" x1="{:.1}" y1="{y:.1}" x2="{:.1}" y2="{y:.1}"/><text class="lvltxt" x="{tx:.1}" y="{:.1}" text-anchor="end">{}</text>"#, f.x(h0) + 4.0, f.x(h1) - 4.0, y - 3.0, esc(&label));
    }
    // Heights on the left: each storey and the overall height.
    let dx = f.x(h0) + 14.0;
    let mut bounds: Vec<f64> = marks.iter().map(|(_, z)| *z).collect();
    bounds.push(top);
    bounds.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    for w in bounds.windows(2) {
        let (ya, yb) = (f.y(w[0]), f.y(w[1]));
        let _ = writeln!(o, r#"<path class="dim" d="M{dx:.1} {ya:.1} V{yb:.1} M{:.1} {ya:.1} H{:.1} M{:.1} {yb:.1} H{:.1}"/><text class="dimtxt" x="{:.1}" y="{:.1}" transform="rotate(-90 {:.1} {:.1})">{:.2}</text>"#, dx - 4.0, dx + 4.0, dx - 4.0, dx + 4.0, dx - 5.0, (ya + yb) / 2.0, dx - 5.0, (ya + yb) / 2.0, w[1] - w[0]);
    }
    // Overall height (ground floor top to the highest point), outside the storey dimensions, and the overall width of the building
    // (the extent of its rooms) under the ground line.
    let (dx2, ya, yb) = (dx - 34.0, f.y(0.0), f.y(top));
    let _ = writeln!(o, r#"<path class="dim" d="M{dx2:.1} {ya:.1} V{yb:.1} M{:.1} {ya:.1} H{:.1} M{:.1} {yb:.1} H{:.1}"/><text class="dimtxt" x="{:.1}" y="{:.1}" transform="rotate(-90 {:.1} {:.1})" text-anchor="middle">H {top:.2}</text>"#, dx2 - 5.0, dx2 + 5.0, dx2 - 5.0, dx2 + 5.0, dx2 - 4.0, (ya + yb) / 2.0, dx2 - 4.0, (ya + yb) / 2.0);
    let (xa, xb, wy) = (f.x(h_range.0), f.x(h_range.1), f.y(bottom) + 16.0);
    let _ = writeln!(o, r#"<path class="dim" d="M{xa:.1} {wy:.1} H{xb:.1} M{xa:.1} {:.1} V{:.1} M{xb:.1} {:.1} V{:.1}"/><text class="dimtxt" x="{:.1}" y="{:.1}" text-anchor="middle">{:.2}</text>"#, wy - 5.0, wy + 5.0, wy - 5.0, wy + 5.0, (xa + xb) / 2.0, wy - 4.0, h_range.1 - h_range.0);
    let ty = f.y(bottom) + 40.0;
    let _ = writeln!(o, r#"<text class="title" x="{:.1}" y="{ty:.1}">{}</text><text class="note" x="{:.1}" y="{:.1}">{}</text>"#, f.m, esc(title), f.m, ty + 18.0, esc(subtitle));
    let _ = writeln!(o, r#"<text class="note" x="{:.1}" y="{:.1}">Preliminary drawing: heights from the top of the ground floor slab, no site datum assumed. Metres.</text></svg>"#, f.m, ty + 34.0);
    c.o.push_str(&o);
    let shapes = ViewShapes { title: title.to_string(), subtitle: subtitle.to_string(), shapes: c.shapes, h_range, top, bottom, levels: marks, grids: std::mem::take(&mut c.grids) };
    (c.o, shapes)
}

/// The grid lines a view looking along an axis shows, with their position across the view in metres: a line seen end-on is a vertical
/// line. `across_x` is true when the view's horizontal axis is x (so the lines that stand at an x are the ones seen); `sign` is -1 when the
/// view looks the other way (the horizontal axis is reversed).
fn view_grids(model: &Model, across_x: bool, sign: f64, range: (f64, f64)) -> Vec<(String, f64)> {
    use crate::typed::{Grid, Typed};
    let mut v: Vec<(String, f64)> = model
        .elements
        .values()
        .filter(|e| e.kind == Grid::KIND)
        .filter_map(|e| e.typed::<Grid>().ok())
        .filter(|g| g.horizontal() != across_x)
        .map(|g| { let h = sign * g.coord() as f64 / 1000.0; (g.name, h) })
        .filter(|(_, h)| *h >= range.0 - 1.5 && *h <= range.1 + 1.5)
        .collect();
    v.sort_by(|a, b| a.1.total_cmp(&b.1));
    v
}

fn top_of(scene: &Scene, parapet: Option<f64>) -> f64 {
    let slab_top = scene.boxes.iter().filter(|b| b.kind == "Slab").map(|b| b.max[2]).fold(f64::MIN, f64::max);
    let wall_top = scene.boxes.iter().filter(|b| b.kind == "Wall").map(|b| b.max[2]).fold(f64::MIN, f64::max);
    let facade_top = scene.boxes.iter().filter(|b| b.kind == "Facade").map(|b| b.max[2]).fold(f64::MIN, f64::max);
    slab_top.max(wall_top).max(facade_top) + parapet.unwrap_or(0.0)
}

fn check_parapet(opts: &ViewOpts) -> Result<(), ViewError> {
    match opts.parapet_m {
        Some(p) if !p.is_finite() || p <= 0.0 => Err(ViewError::BadParapet),
        _ => Ok(()),
    }
}

/// Screen-horizontal range of a box seen from `side`.
fn h_range_side(b: &Box3, side: Side) -> (f64, f64) {
    match side {
        Side::South => (b.min[0], b.max[0]),
        Side::North => (-b.max[0], -b.min[0]),
        Side::East => (b.min[1], b.max[1]),
        Side::West => (-b.max[1], -b.min[1]),
    }
}

pub fn elevation_svg(model: &Model, side: Side, opts: &ViewOpts) -> Result<String, ViewError> {
    Ok(elevation_build(model, side, opts)?.0)
}

/// The elevation as rectangles (for the DXF export and any other drawing format).
pub fn elevation_shapes(model: &Model, side: Side, opts: &ViewOpts) -> Result<ViewShapes, ViewError> {
    Ok(elevation_build(model, side, opts)?.1)
}

fn elevation_build(model: &Model, side: Side, opts: &ViewOpts) -> Result<(String, ViewShapes), ViewError> {
    check_parapet(opts)?;
    let scene = model3d::build_styled(model, opts.style);
    let e = extent(&scene).ok_or(ViewError::Empty)?;
    let hr = h_range_side(&Box3 { id: String::new(), kind: "", class: String::new(), min: [e[0], e[1], 0.0], max: [e[2], e[3], 0.0], visual: false, level: None, discipline: "architectural" }, side);
    let top = top_of(&scene, opts.parapet_m);
    let bottom = -0.3;
    let title = if opts.title.is_empty() { side.name().to_string() } else { format!("{} — {}", opts.title, side.name()) };
    let mut c = begin(opts, hr, top, bottom);
    c.grids = match side {
        Side::South => view_grids(model, true, 1.0, hr),
        Side::North => view_grids(model, true, -1.0, hr),
        Side::East => view_grids(model, false, 1.0, hr),
        Side::West => view_grids(model, false, -1.0, hr),
    };
    // Every exterior wall parallel to the facade (not only those on the building's bounding line: a stepped plan has set-back walls),
    // its openings, and the slabs, drawn from the farthest to the nearest so a nearer piece covers what lies behind it.
    let parallel = |b: &Box3| {
        let (dx, dy) = (b.max[0] - b.min[0], b.max[1] - b.min[1]);
        match side {
            Side::South | Side::North => dy <= dx,
            Side::East | Side::West => dx <= dy,
        }
    };
    let walls: Vec<&Box3> = scene.boxes.iter().filter(|b| b.kind == "Wall" && b.class == "ext" && parallel(b)).collect();
    // The face of an exterior wall that looks outward is the one away from the rooms beside it: rooms on its north side make it a south
    // facade. `Some(true)` when that outward face looks towards the viewer, `Some(false)` when it looks away, `None` when no room touches
    // the wall (then nothing is known and the wall is kept; a nearer wall may still hide it).
    const TOUCH_M: f64 = 0.35;
    let rooms: Vec<&Box3> = scene.boxes.iter().filter(|b| b.kind == "Space").collect();
    let faces_viewer = |w: &Box3| -> Option<bool> {
        let (cx, cy) = ((w.max[0] + w.min[0]) / 2.0, (w.max[1] + w.min[1]) / 2.0);
        // Rooms beside the wall on its low side (smaller coordinate) and on its high side, across the wall's extent along the facade.
        let (mut low, mut high) = (false, false);
        for r in &rooms {
            match side {
                Side::South | Side::North => {
                    if r.min[0] < w.max[0] && r.max[0] > w.min[0] {
                        high |= (r.min[1] - cy).abs() <= TOUCH_M;
                        low |= (r.max[1] - cy).abs() <= TOUCH_M;
                    }
                }
                Side::East | Side::West => {
                    if r.min[1] < w.max[1] && r.max[1] > w.min[1] {
                        high |= (r.min[0] - cx).abs() <= TOUCH_M;
                        low |= (r.max[0] - cx).abs() <= TOUCH_M;
                    }
                }
            }
        }
        match (low, high, side) {
            (false, true, Side::South | Side::West) | (true, false, Side::North | Side::East) => Some(true),
            (true, false, Side::South | Side::West) | (false, true, Side::North | Side::East) => Some(false),
            _ => None,
        }
    };
    let facade_walls: Vec<&Box3> = walls.iter().copied().filter(|w| faces_viewer(w) != Some(false)).collect();
    // An opening belongs to the facade when it stands in a wall that shows (plan overlap with one of them).
    let in_exterior_wall = |o: &Box3| facade_walls.iter().any(|w| o.min[0] < w.max[0] && o.max[0] > w.min[0] && o.min[1] < w.max[1] && o.max[1] > w.min[1]);
    let near = |b: &Box3| -> f64 {
        let (cx, cy) = ((b.max[0] + b.min[0]) / 2.0, (b.max[1] + b.min[1]) / 2.0);
        match side {
            Side::South => -cy,
            Side::North => cy,
            Side::East => cx,
            Side::West => -cx,
        }
    };
    // (distance towards the viewer, rank at equal depth: wall, slab, opening, the piece)
    let mut pieces: Vec<(f64, u8, &Box3)> = Vec::new();
    pieces.extend(facade_walls.iter().map(|b| (near(b), 0, *b)));
    pieces.extend(scene.boxes.iter().filter(|b| b.kind == "Slab").map(|b| (near(b), 1, b)));
    pieces.extend(scene.boxes.iter().filter(|b| matches!(b.kind, "Window" | "Door") && parallel(b) && in_exterior_wall(b)).map(|b| (near(b), 2, b)));
    pieces.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    // A piece wholly behind a nearer wall is not seen: it is left out (the opening of the opposite facade, the far wall of a plain box).
    const BEHIND_M: f64 = 0.15;
    let hidden = |near_p: f64, b: &Box3| -> bool {
        let (h0, h1) = h_range_side(b, side);
        walls.iter().any(|w| {
            let (w0, w1) = h_range_side(w, side);
            near(w) > near_p + BEHIND_M && w0 <= h0 && w1 >= h1 && w.min[2] <= b.min[2] && w.max[2] >= b.max[2]
        })
    };
    for (near_p, _, b) in pieces {
        if hidden(near_p, b) {
            continue;
        }
        let (h0, h1) = h_range_side(b, side);
        let class = match b.kind {
            "Wall" => "wall",
            "Slab" => "slab",
            "Door" => "leaf",
            _ => "glass",
        };
        c.rect(class, Some(&b.id), h0, h1, b.min[2], b.max[2]);
    }
    // The style's solids of this side, in front of the walls and the openings.
    let name = match side {
        Side::South => "south",
        Side::North => "north",
        Side::East => "east",
        Side::West => "west",
    };
    for b in scene.boxes.iter().filter(|b| b.kind == "Facade") {
        if let Some((motif, s)) = crate::style::split_class(&b.class)
            && s == name
        {
            let (h0, h1) = h_range_side(b, side);
            c.rect(&format!("facade-{motif}"), Some(&b.id), h0, h1, b.min[2], b.max[2]);
        }
    }
    if let Some(p) = opts.parapet_m {
        let roof = scene.boxes.iter().filter(|b| b.kind == "Slab").map(|b| b.max[2]).fold(f64::MIN, f64::max);
        let (h0, h1) = hr;
        c.rect("parapet", None, h0, h1, roof, roof + p);
    }
    Ok(finish(c, &scene, &title, "", hr, top, bottom))
}

/// Class used to draw a box in a section, `None` for what a section does not show.
fn section_class(b: &Box3) -> Option<String> {
    match b.kind {
        "Wall" => Some("wall".into()),
        "Slab" => Some("slab".into()),
        "Column" => Some("column".into()),
        "Beam" => Some("beam".into()),
        "Footing" => Some("footing".into()),
        "Window" => Some("glass".into()),
        "Door" => Some("leaf".into()),
        "MepRun" => Some(b.class.clone()),
        "Facade" => crate::style::split_class(&b.class).map(|(motif, _)| format!("facade-{motif}")),
        _ => None,
    }
}

pub fn section_svg(model: &Model, cut: &Cut, opts: &ViewOpts) -> Result<String, ViewError> {
    Ok(section_build(model, cut, opts)?.0)
}

/// The section as rectangles (for the DXF export and any other drawing format).
pub fn section_shapes(model: &Model, cut: &Cut, opts: &ViewOpts) -> Result<ViewShapes, ViewError> {
    Ok(section_build(model, cut, opts)?.1)
}

fn section_build(model: &Model, cut: &Cut, opts: &ViewOpts) -> Result<(String, ViewShapes), ViewError> {
    check_parapet(opts)?;
    let scene = model3d::build_styled(model, opts.style);
    let e = extent(&scene).ok_or(ViewError::Empty)?;
    // Axis the plane is perpendicular to (1: y, 0: x); screen-right is +x looking north, +y looking west.
    let (axis, at, name) = match cut {
        Cut::AtY { name, at } => (1usize, *at, name.as_str()),
        Cut::AtX { name, at } => (0usize, *at, name.as_str()),
    };
    let (lo, hi) = if axis == 1 { (e[1], e[3]) } else { (e[0], e[2]) };
    if !at.is_finite() || at <= lo || at >= hi {
        return Err(ViewError::OutsideBuilding { at, lo, hi });
    }
    let h_of = |b: &Box3| if axis == 1 { (b.min[0], b.max[0]) } else { (b.min[1], b.max[1]) };
    let hr = if axis == 1 { (e[0], e[2]) } else { (e[1], e[3]) };
    let top = top_of(&scene, opts.parapet_m);
    let bottom = scene.boxes.iter().filter(|b| b.kind == "Footing").map(|b| b.min[2]).fold(-0.3, f64::min) - 0.1;
    let looking = if axis == 1 { "looking north, east on the right" } else { "looking west, north on the right" };
    let title = format!("{}{} section {name}", opts.title, if opts.title.is_empty() { "" } else { " — " });
    let sub = format!("Plane {} = {at:.2} m, {looking}", if axis == 1 { "y" } else { "x" });
    let mut c = begin(opts, hr, top, bottom);
    c.grids = view_grids(model, axis == 1, 1.0, hr);
    // Beyond the plane (what is seen past it), far to near; then what the plane cuts, on top.
    let mut beyond: Vec<&Box3> = scene.boxes.iter().filter(|b| section_class(b).is_some()).filter(|b| if axis == 1 { b.min[1] >= at } else { b.max[0] <= at }).collect();
    beyond.sort_by(|a, b| if axis == 1 { b.min[1].total_cmp(&a.min[1]) } else { a.max[0].total_cmp(&b.max[0]) });
    for b in beyond {
        let (h0, h1) = h_of(b);
        let class = section_class(b).unwrap_or_default();
        let far = if matches!(b.kind, "Wall" | "Slab" | "Column" | "Beam") { format!("{class} far") } else { class };
        c.rect(&far, Some(&b.id), h0, h1, b.min[2], b.max[2]);
    }
    for b in scene.boxes.iter().filter(|b| section_class(b).is_some() && b.min[axis] < at && b.max[axis] > at) {
        let (h0, h1) = h_of(b);
        let class = section_class(b).unwrap_or_default();
        let cutc = if matches!(b.kind, "Wall" | "Slab") { format!("{class} cut") } else { class };
        c.rect(&cutc, Some(&b.id), h0, h1, b.min[2], b.max[2]);
    }
    if let Some(p) = opts.parapet_m {
        let roof = scene.boxes.iter().filter(|b| b.kind == "Slab").map(|b| b.max[2]).fold(f64::MIN, f64::max);
        let t = wall_thickness(&scene);
        c.rect("parapet", None, hr.0 - t / 2.0, hr.0 + t / 2.0, roof, roof + p);
        c.rect("parapet", None, hr.1 - t / 2.0, hr.1 + t / 2.0, roof, roof + p);
    }
    Ok(finish(c, &scene, &title, &sub, hr, top, bottom))
}
