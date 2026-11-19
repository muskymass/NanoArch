//! DXF export (AutoCAD R12 ASCII, `AC1009`) of the plan, one sheet per level, in millimetres.
//!
//! R12 is written because every CAD program reads it. The geometry is exactly the model's, in model coordinates
//! (x east, y north), on named layers so each discipline can be switched on and off:
//!
//! | layer | content |
//! |---|---|
//! | `A-WALL-EXT`, `A-WALL-INT` | walls as closed outlines, cut around their doors and windows |
//! | `A-DOOR` | jambs, the open leaf and its swing arc (towards the room the door opens into) |
//! | `A-GLAZ` | window frame and glass line |
//! | `S-COLS` | columns (outline and fill) |
//! | `S-BEAM` | beam centre lines (dashed) |
//! | `S-FTNG` | footings (hidden line); their depth is not designed, only the plan size is drawn |
//! | `A-ROOM`, `A-TEXT` | room outlines, names and areas |
//! | `A-STAIR` | the stair shaft |
//! | `E-LITE`, `E-POWR`, `E-PANL`, `P-FIXT`, `M-HVAC` | MEP points |
//! | `A-DIMS` | overall dimensions, title and the preliminary note |
//!
//! Several levels are laid side by side on one file, left to right, each under its own title. Text outside ASCII
//! (Arabic names) uses the DXF `\U+XXXX` escape.

use crate::model::{Id, Model};
use crate::typed::{Beam, Column, Door, Footing, Level, MepPoint, Space, Typed, Wall, Window};
use std::collections::BTreeMap;

pub struct DxfOpts {
    /// Project title written under each sheet.
    pub title: String,
    /// Display names of rooms by requirement key.
    pub labels: BTreeMap<String, String>,
    /// Draw the MEP points that exist in the model.
    pub mep: bool,
}

#[derive(Debug, Clone)]
pub struct DxfOut {
    pub text: String,
    /// Entities written, by layer.
    pub counts: BTreeMap<String, usize>,
}

/// Layer name, AutoCAD colour index, line type.
const LAYERS: [(&str, i32, &str); 15] = [
    ("A-WALL-EXT", 7, "CONTINUOUS"),
    ("A-WALL-INT", 8, "CONTINUOUS"),
    ("A-DOOR", 3, "CONTINUOUS"),
    ("A-GLAZ", 5, "CONTINUOUS"),
    ("S-COLS", 1, "CONTINUOUS"),
    ("S-BEAM", 30, "DASHED"),
    ("S-FTNG", 6, "HIDDEN"),
    ("A-ROOM", 9, "CONTINUOUS"),
    ("A-TEXT", 7, "CONTINUOUS"),
    ("A-STAIR", 4, "CONTINUOUS"),
    ("E-LITE", 2, "CONTINUOUS"),
    ("E-POWR", 5, "CONTINUOUS"),
    ("E-PANL", 6, "CONTINUOUS"),
    ("P-FIXT", 3, "CONTINUOUS"),
    ("M-HVAC", 4, "CONTINUOUS"),
];
const DIMS_LAYER: (&str, i32, &str) = ("A-DIMS", 2, "CONTINUOUS");

/// DXF number: plain decimal, no exponent.
fn n(v: f64) -> String {
    let v = if v.abs() < 5e-4 { 0.0 } else { v };
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0');
    if s.ends_with('.') { format!("{s}0") } else { s.to_string() }
}

/// Text as DXF wants it: ASCII as is, other characters as `\U+XXXX`.
pub fn dxf_text(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if (' '..='~').contains(&ch) {
            out.push(ch);
        } else if (ch as u32) <= 0xFFFF {
            out.push_str(&format!("\\U+{:04X}", ch as u32));
        } else {
            out.push('?');
        }
    }
    out
}

struct D {
    body: String,
    counts: BTreeMap<String, usize>,
}

impl D {
    fn pair(&mut self, code: i32, v: &str) {
        self.body.push_str(&format!("{code}\n{v}\n"));
    }
    fn head(&mut self, kind: &str, layer: &str) {
        self.pair(0, kind);
        self.pair(8, layer);
        *self.counts.entry(layer.to_string()).or_default() += 1;
    }
    fn pt(&mut self, base: i32, x: f64, y: f64) {
        self.pair(base, &n(x));
        self.pair(base + 10, &n(y));
        self.pair(base + 20, "0.0");
    }
    fn line(&mut self, layer: &str, a: (f64, f64), b: (f64, f64)) {
        self.head("LINE", layer);
        self.pt(10, a.0, a.1);
        self.pt(11, b.0, b.1);
    }
    fn poly(&mut self, layer: &str, pts: &[(f64, f64)], closed: bool) {
        self.head("POLYLINE", layer);
        self.pair(66, "1");
        self.pair(70, if closed { "1" } else { "0" });
        for p in pts {
            self.pair(0, "VERTEX");
            self.pair(8, layer);
            self.pt(10, p.0, p.1);
        }
        self.pair(0, "SEQEND");
        self.pair(8, layer);
    }
    fn rect(&mut self, layer: &str, x0: f64, y0: f64, x1: f64, y1: f64) {
        self.poly(layer, &[(x0, y0), (x1, y0), (x1, y1), (x0, y1)], true);
    }
    fn solid(&mut self, layer: &str, x0: f64, y0: f64, x1: f64, y1: f64) {
        self.head("SOLID", layer);
        // SOLID corners are in "Z" order: 1, 2, 4, 3
        self.pt(10, x0, y0);
        self.pt(11, x1, y0);
        self.pt(12, x0, y1);
        self.pt(13, x1, y1);
    }
    fn circle(&mut self, layer: &str, c: (f64, f64), r: f64) {
        self.head("CIRCLE", layer);
        self.pt(10, c.0, c.1);
        self.pair(40, &n(r));
    }
    fn arc(&mut self, layer: &str, c: (f64, f64), r: f64, a0: f64, a1: f64) {
        self.head("ARC", layer);
        self.pt(10, c.0, c.1);
        self.pair(40, &n(r));
        self.pair(50, &n(a0));
        self.pair(51, &n(a1));
    }
    /// Centred text.
    fn text(&mut self, layer: &str, at: (f64, f64), height: f64, s: &str) {
        self.head("TEXT", layer);
        self.pt(10, at.0, at.1);
        self.pair(40, &n(height));
        self.pair(1, &dxf_text(s));
        self.pair(72, "1");
        self.pt(11, at.0, at.1);
        self.pair(73, "2");
    }
}

struct Sheet {
    level: Option<Id>,
    name: String,
    min: (i64, i64),
    max: (i64, i64),
}

fn on_level(e: &crate::model::Element, level: Option<Id>) -> bool {
    level.is_none() || e.level_id == level
}

pub fn export_dxf(model: &Model, levels: &[Option<Id>], opts: &DxfOpts) -> Result<DxfOut, String> {
    // The sheets: one per requested level, with the extent of its rooms.
    let mut sheets: Vec<Sheet> = Vec::new();
    for &lv in levels {
        let spaces: Vec<Space> = model.elements.values().filter(|e| e.kind == Space::KIND && on_level(e, lv)).filter_map(|e| e.typed::<Space>().ok()).collect();
        if spaces.is_empty() {
            continue;
        }
        let min = (spaces.iter().map(|s| s.x).min().unwrap_or(0), spaces.iter().map(|s| s.y).min().unwrap_or(0));
        let max = (spaces.iter().map(|s| s.x + s.w).max().unwrap_or(0), spaces.iter().map(|s| s.y + s.h).max().unwrap_or(0));
        let name = lv.and_then(|l| model.get(&l)).and_then(|e| e.typed::<Level>().ok()).map_or(String::new(), |l| l.name);
        sheets.push(Sheet { level: lv, name, min, max });
    }
    if sheets.is_empty() {
        return Err("nothing to export: no rooms on the requested level(s)".into());
    }
    let step = sheets.iter().map(|s| s.max.0 - s.min.0).max().unwrap_or(0) + 6000;

    let mut d = D { body: String::new(), counts: BTreeMap::new() };
    let (mut gx0, mut gy0, mut gx1, mut gy1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (k, sh) in sheets.iter().enumerate() {
        // The sheet is shifted so its lower-left corner sits at k * step; y is unchanged.
        let ox = (k as i64 * step - sh.min.0) as f64;
        let px = |x: i64| x as f64 + ox;
        let py = |y: i64| y as f64;
        gx0 = gx0.min(px(sh.min.0) - 1500.0);
        gy0 = gy0.min(py(sh.min.1) - 2600.0);
        gx1 = gx1.max(px(sh.max.0) + 600.0);
        gy1 = gy1.max(py(sh.max.1) + 600.0);

        // Rooms: outline, name and area; the stair shaft crossed.
        let spaces: Vec<Space> = model.elements.values().filter(|e| e.kind == Space::KIND && on_level(e, sh.level)).filter_map(|e| e.typed::<Space>().ok()).collect();
        for s in &spaces {
            d.rect("A-ROOM", px(s.x), py(s.y), px(s.x + s.w), py(s.y + s.h));
            let (cx, cy) = (px(s.x) + s.w as f64 / 2.0, py(s.y) + s.h as f64 / 2.0);
            let name = opts.labels.get(&s.requirement_key).cloned().unwrap_or_else(|| s.requirement_key.clone());
            d.text("A-TEXT", (cx, cy + 120.0), 200.0, &name);
            d.text("A-TEXT", (cx, cy - 200.0), 140.0, &format!("{:.1} m2", s.area));
            if s.func == "stair" {
                let (x0, y0, x1, y1) = (px(s.x), py(s.y), px(s.x + s.w), py(s.y + s.h));
                d.line("A-STAIR", (x0, y0), (x1, y1));
                d.line("A-STAIR", (x0, y1), (x1, y0));
            }
        }

        // Walls cut around their openings, then the openings themselves.
        for e in model.elements.values().filter(|e| e.kind == Wall::KIND && on_level(e, sh.level)) {
            let Ok(w) = e.typed::<Wall>() else { continue };
            let layer = if w.is_exterior { "A-WALL-EXT" } else { "A-WALL-INT" };
            let (s0, s1, cl, t) = (w.start(), w.start() + w.length, w.centre_line(), w.thickness);
            let mut gaps: Vec<(i64, i64)> = Vec::new();
            // (is a door, centre along the wall, width, the room a door opens into)
            let mut openings: Vec<(bool, i64, i64, String)> = Vec::new();
            for o in model.elements.values().filter(|o| o.host_id == Some(e.id)) {
                if let Ok(dr) = o.typed::<Door>() {
                    let c = s0 + dr.offset_center;
                    gaps.push((c - dr.width / 2, c + dr.width / 2));
                    openings.push((true, c, dr.width, dr.opens_into));
                } else if let Ok(wn) = o.typed::<Window>() {
                    let c = s0 + wn.offset_center;
                    gaps.push((c - wn.width / 2, c + wn.width / 2));
                    openings.push((false, c, wn.width, String::new()));
                }
            }
            gaps.sort_unstable();
            let mut cursor = s0;
            let mut pieces: Vec<(i64, i64)> = Vec::new();
            for (a, b) in gaps {
                if a > cursor {
                    pieces.push((cursor, a));
                }
                cursor = cursor.max(b);
            }
            if cursor < s1 {
                pieces.push((cursor, s1));
            }
            let band = |a: i64, b: i64| -> (f64, f64, f64, f64) {
                if w.horizontal() { (px(a), py(cl - t / 2), px(b), py(cl + t / 2)) } else { (px(cl - t / 2), py(a), px(cl + t / 2), py(b)) }
            };
            for (a, b) in pieces {
                let (x0, y0, x1, y1) = band(a, b);
                d.rect(layer, x0, y0, x1, y1);
            }
            for (door, c, width, into) in openings {
                let (a, b) = (c - width / 2, c + width / 2);
                // Which side of the wall the room the door opens into lies on (+1: north / east).
                let side = spaces.iter().find(|s| s.requirement_key == into).map_or(1.0, |s| {
                    let centre = if w.horizontal() { s.y + s.h / 2 } else { s.x + s.w / 2 };
                    if centre >= cl { 1.0 } else { -1.0 }
                });
                let (x0, y0, x1, y1) = band(a, b);
                if door {
                    // jambs across the wall, the leaf open at 90 degrees from the lower end, and its swing
                    if w.horizontal() {
                        d.line("A-DOOR", (x0, y0), (x0, y1));
                        d.line("A-DOOR", (x1, y0), (x1, y1));
                        let hinge = (x0, if side > 0.0 { y1 } else { y0 });
                        d.line("A-DOOR", hinge, (hinge.0, hinge.1 + side * width as f64));
                        let (a0, a1) = if side > 0.0 { (0.0, 90.0) } else { (270.0, 360.0) };
                        d.arc("A-DOOR", hinge, width as f64, a0, a1);
                    } else {
                        d.line("A-DOOR", (x0, y0), (x1, y0));
                        d.line("A-DOOR", (x0, y1), (x1, y1));
                        let hinge = (if side > 0.0 { x1 } else { x0 }, y0);
                        d.line("A-DOOR", hinge, (hinge.0 + side * width as f64, hinge.1));
                        let (a0, a1) = if side > 0.0 { (0.0, 90.0) } else { (90.0, 180.0) };
                        d.arc("A-DOOR", hinge, width as f64, a0, a1);
                    }
                } else {
                    d.rect("A-GLAZ", x0, y0, x1, y1);
                    if w.horizontal() {
                        d.line("A-GLAZ", (x0, (y0 + y1) / 2.0), (x1, (y0 + y1) / 2.0));
                    } else {
                        d.line("A-GLAZ", ((x0 + x1) / 2.0, y0), ((x0 + x1) / 2.0, y1));
                    }
                }
            }
        }

        // Structure.
        for e in model.elements.values().filter(|e| e.kind == Beam::KIND && on_level(e, sh.level)) {
            if let Ok(b) = e.typed::<Beam>() {
                d.line("S-BEAM", (px(b.x1), py(b.y1)), (px(b.x2), py(b.y2)));
            }
        }
        for e in model.elements.values().filter(|e| e.kind == Footing::KIND && on_level(e, sh.level)) {
            if let Ok(f) = e.typed::<Footing>() {
                d.rect("S-FTNG", px(f.x - f.length / 2), py(f.y - f.width / 2), px(f.x + f.length / 2), py(f.y + f.width / 2));
            }
        }
        for e in model.elements.values().filter(|e| e.kind == Column::KIND && on_level(e, sh.level)) {
            if let Ok(c) = e.typed::<Column>() {
                let (x0, y0, x1, y1) = (px(c.x - c.b / 2), py(c.y - c.h / 2), px(c.x + c.b / 2), py(c.y + c.h / 2));
                d.solid("S-COLS", x0, y0, x1, y1);
                d.rect("S-COLS", x0, y0, x1, y1);
            }
        }

        // MEP points.
        if opts.mep {
            for e in model.elements.values().filter(|e| e.kind == MepPoint::KIND && on_level(e, sh.level)) {
                let Ok(p) = e.typed::<MepPoint>() else { continue };
                let c = (px(p.x), py(p.y));
                match p.role.as_str() {
                    "light" => d.circle("E-LITE", c, 100.0),
                    "socket" => d.rect("E-POWR", c.0 - 60.0, c.1 - 60.0, c.0 + 60.0, c.1 + 60.0),
                    "panel" => d.solid("E-PANL", c.0 - 150.0, c.1 - 150.0, c.0 + 150.0, c.1 + 150.0),
                    "fixture" => d.circle("P-FIXT", c, 120.0),
                    _ => {
                        d.rect("M-HVAC", c.0 - 300.0, c.1 - 150.0, c.0 + 300.0, c.1 + 150.0);
                        if let Some(l) = &p.label {
                            d.text("M-HVAC", c, 90.0, l);
                        }
                    }
                }
            }
        }

        // Overall dimensions, title and the preliminary note.
        let (w_mm, h_mm) = (sh.max.0 - sh.min.0, sh.max.1 - sh.min.1);
        let (x0, x1, y) = (px(sh.min.0), px(sh.max.0), py(sh.min.1) - 900.0);
        d.line(DIMS_LAYER.0, (x0, y), (x1, y));
        for x in [x0, x1] {
            d.line(DIMS_LAYER.0, (x, y - 120.0), (x, y + 120.0));
        }
        d.text(DIMS_LAYER.0, ((x0 + x1) / 2.0, y + 220.0), 220.0, &format!("{:.2} m", w_mm as f64 / 1000.0));
        let (dx, y0, y1) = (px(sh.min.0) - 900.0, py(sh.min.1), py(sh.max.1));
        d.line(DIMS_LAYER.0, (dx, y0), (dx, y1));
        for yy in [y0, y1] {
            d.line(DIMS_LAYER.0, (dx - 120.0, yy), (dx + 120.0, yy));
        }
        d.text(DIMS_LAYER.0, (dx - 330.0, (y0 + y1) / 2.0), 220.0, &format!("{:.2} m", h_mm as f64 / 1000.0));
        let title = if sh.name.is_empty() { opts.title.clone() } else { format!("{} - {}", opts.title, sh.name) };
        d.text(DIMS_LAYER.0, ((x0 + x1) / 2.0, py(sh.min.1) - 1500.0), 300.0, &title);
        d.text(DIMS_LAYER.0, ((x0 + x1) / 2.0, py(sh.min.1) - 2000.0), 180.0, "PRELIMINARY - NOT FOR CONSTRUCTION");
    }

    // Assemble the file.
    let mut out = String::new();
    let mut pair = |code: i32, v: &str| out.push_str(&format!("{code}\n{v}\n"));
    pair(0, "SECTION");
    pair(2, "HEADER");
    pair(9, "$ACADVER");
    pair(1, "AC1009");
    pair(9, "$INSUNITS");
    pair(70, "4");
    pair(9, "$EXTMIN");
    pair(10, &n(gx0));
    pair(20, &n(gy0));
    pair(30, "0.0");
    pair(9, "$EXTMAX");
    pair(10, &n(gx1));
    pair(20, &n(gy1));
    pair(30, "0.0");
    pair(0, "ENDSEC");
    pair(0, "SECTION");
    pair(2, "TABLES");
    pair(0, "TABLE");
    pair(2, "LTYPE");
    pair(70, "3");
    for (name, desc, pattern) in [("CONTINUOUS", "Solid line", vec![]), ("DASHED", "Dashed __ __ __", vec![600.0, -300.0]), ("HIDDEN", "Hidden __ __ __", vec![300.0, -150.0])] {
        pair(0, "LTYPE");
        pair(2, name);
        pair(70, "0");
        pair(3, desc);
        pair(72, "65");
        pair(73, &pattern.len().to_string());
        pair(40, &n(pattern.iter().map(|v: &f64| v.abs()).sum()));
        for v in pattern {
            pair(49, &n(v));
        }
    }
    pair(0, "ENDTAB");
    pair(0, "TABLE");
    pair(2, "LAYER");
    pair(70, &(LAYERS.len() + 1).to_string());
    for (name, color, ltype) in LAYERS.iter().copied().chain(std::iter::once(DIMS_LAYER)) {
        pair(0, "LAYER");
        pair(2, name);
        pair(70, "0");
        pair(62, &color.to_string());
        pair(6, ltype);
    }
    pair(0, "ENDTAB");
    pair(0, "TABLE");
    pair(2, "STYLE");
    pair(70, "1");
    pair(0, "STYLE");
    pair(2, "STANDARD");
    pair(70, "0");
    pair(40, "0.0");
    pair(41, "1.0");
    pair(50, "0.0");
    pair(71, "0");
    pair(42, "200.0");
    pair(3, "txt");
    pair(4, "");
    pair(0, "ENDTAB");
    pair(0, "ENDSEC");
    pair(0, "SECTION");
    pair(2, "ENTITIES");
    out.push_str(&d.body);
    out.push_str("0\nENDSEC\n0\nEOF\n");
    Ok(DxfOut { text: out, counts: d.counts })
}
