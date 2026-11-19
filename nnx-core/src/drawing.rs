//! 2D plan drawing (SVG), rendered directly from the unified model.
//!
//! The renderer reads only typed `Space`, `Wall`, `Door` and `Window` elements; it keeps no
//! copy of the plan. Elements that fail to parse as their type are skipped (the commit
//! check in `Document` normally prevents them). Every drawn element carries `data-id`,
//! the id of its model element, so a click in a viewer maps back to the single source of
//! truth, and any edit to the model shows up the next time the plan is drawn.
//!
//! North is up. Lengths in the model are millimetres.

use crate::model::{Element, Id, Model};
use crate::typed::{Beam, Column, Door, MepPoint, Space, Typed, Wall, Window};
use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Debug, Clone)]
pub struct DrawOpts {
    pub title: String,
    /// Display names by room key or function (for example Arabic names).
    pub labels: BTreeMap<String, String>,
    pub px_per_m: f64,
    pub margin_px: f64,
    pub show_dims: bool,
    /// Draw columns (with grid references) and beams over the plan.
    pub show_structure: bool,
    /// Draw MEP points (lights, sockets, fixtures, board, air-conditioning units) over the plan.
    pub show_mep: bool,
}

impl Default for DrawOpts {
    fn default() -> Self {
        Self { title: "Ground floor plan".into(), labels: BTreeMap::new(), px_per_m: 50.0, margin_px: 90.0, show_dims: true, show_structure: false, show_mep: false }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum DrawError {
    #[error("the model has no spaces to draw")]
    NoSpaces,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// All elements of kind `T` on the level (or all levels), parsed; unparsable ones are skipped.
fn collect<T: Typed>(model: &Model, level: Option<Id>) -> Vec<(&Element, T)> {
    model
        .elements
        .values()
        .filter(|e| e.kind == T::KIND && (level.is_none() || e.level_id == level))
        .filter_map(|e| e.typed::<T>().ok().map(|t| (e, t)))
        .collect()
}

/// Geometry of an opening inside its host wall.
struct Host {
    horiz: bool,
    cl: f64,    // centre-line coordinate across the wall
    start: f64, // wall start along its length
    t: f64,
}

pub fn plan_svg(model: &Model, level: Option<Id>, opts: &DrawOpts) -> Result<String, DrawError> {
    let spaces = collect::<Space>(model, level);
    let walls = collect::<Wall>(model, level);
    let doors = collect::<Door>(model, level);
    let windows = collect::<Window>(model, level);
    if spaces.is_empty() {
        return Err(DrawError::NoSpaces);
    }

    let minx = spaces.iter().map(|(_, s)| s.x as f64).fold(f64::INFINITY, f64::min);
    let miny = spaces.iter().map(|(_, s)| s.y as f64).fold(f64::INFINITY, f64::min);
    let maxx = spaces.iter().map(|(_, s)| (s.x + s.w) as f64).fold(f64::NEG_INFINITY, f64::max);
    let maxy = spaces.iter().map(|(_, s)| (s.y + s.h) as f64).fold(f64::NEG_INFINITY, f64::max);

    let s = opts.px_per_m / 1000.0;
    let m = opts.margin_px;
    let title_h = 110.0;
    let width = 2.0 * m + (maxx - minx) * s;
    let height = 2.0 * m + (maxy - miny) * s + title_h;
    let tx = |x: f64| m + (x - minx) * s;
    let ty = |y: f64| m + (maxy - y) * s;

    let mut o = String::new();
    let _ = write!(
        o,
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.1} {h:.1}" width="{w:.0}" height="{h:.0}" font-family="Segoe UI, Tahoma, Arial, sans-serif">
<style>
 .bg{{fill:#fff}} .room{{fill:#fafaf7;stroke:none}} .room.wet{{fill:#e6f0f7}}
 .wall{{fill:#2b2b2b}} .wall.int{{fill:#4a4a4a}} .cut{{fill:#fff}}
 .leaf{{stroke:#111;stroke-width:1.2;fill:none}} .swing{{stroke:#777;stroke-width:.8;fill:none;stroke-dasharray:3 2}}
 .glass{{stroke:#2a6f97;stroke-width:1;fill:none}} .frame{{stroke:#111;stroke-width:1;fill:none}}
 .name{{font-size:13px;font-weight:600;fill:#111;text-anchor:middle}} .meta{{font-size:10.5px;fill:#555;text-anchor:middle}}
 .dim{{stroke:#444;stroke-width:.8;fill:none}} .dimtxt{{font-size:11px;fill:#222;text-anchor:middle}}
.beam{{stroke:#b5451b;stroke-width:1.4;stroke-dasharray:7 4;fill:none}} .column{{fill:#b5451b}} .gridref{{font-size:9px;fill:#b5451b;text-anchor:middle}}
.run{{fill:none;stroke-opacity:.45;stroke-linecap:butt}} .run-duct{{stroke:#0b7285}} .run-pipe{{stroke:#2b7a3d}} .run-drain{{stroke:#8a5a1b}} .run-cable{{stroke:#6a1b9a;stroke-dasharray:6 3}} .mep-diffuser{{fill:#fff;stroke:#0b7285;stroke-width:1.3}} .mep-grille{{fill:none;stroke:#0b7285;stroke-width:1.1;stroke-dasharray:3 2}}
.mep-light{{fill:#f2b705;stroke:#7a5c00;stroke-width:.8}} .mep-socket{{fill:#fff;stroke:#1b6ca8;stroke-width:1.3}} .mep-fixture{{fill:#2a9d8f;stroke:#14524b;stroke-width:.8}} .mep-panel{{fill:#6a1b9a}} .mep-ac{{fill:none;stroke:#0b7285;stroke-width:1.4}} .mep-txt{{font-size:8.5px;fill:#333;text-anchor:middle}}
 .title{{font-size:16px;font-weight:700;fill:#111}} .note{{font-size:10.5px;fill:#a33}}
</style>
<rect class="bg" width="{w:.1}" height="{h:.1}"/>
"##,
        w = width,
        h = height
    );

    // Rooms (floor fill first so walls sit on top).
    for (e, sp) in &spaces {
        let wet = matches!(sp.func.as_str(), "bathroom" | "toilet");
        let _ = writeln!(
            o,
            r#"<rect class="room{}" data-id="{}" x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}"/>"#,
            if wet { " wet" } else { "" },
            e.id,
            tx(sp.x as f64),
            ty((sp.y + sp.h) as f64),
            sp.w as f64 * s,
            sp.h as f64 * s
        );
    }

    // Walls: thick rectangles around each centre line.
    for (e, w) in &walls {
        let (x0, x1, y0, y1) = (w.x1.min(w.x2) as f64, w.x1.max(w.x2) as f64, w.y1.min(w.y2) as f64, w.y1.max(w.y2) as f64);
        let t = w.thickness as f64;
        let (rx, ry, rw, rh) = if w.horizontal() { (x0, y0 + t / 2.0, x1 - x0, t) } else { (x0 - t / 2.0, y1, t, y1 - y0) };
        let _ = writeln!(
            o,
            r#"<rect class="wall{}" data-id="{}" x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}"/>"#,
            if w.is_exterior { "" } else { " int" },
            e.id,
            tx(rx),
            ty(ry),
            rw * s,
            rh * s
        );
    }

    let host_of = |e: &Element| -> Option<Host> {
        let w: Wall = model.get(&e.host_id?)?.typed().ok()?;
        Some(Host { horiz: w.horizontal(), cl: w.centre_line() as f64, start: w.start() as f64, t: w.thickness as f64 })
    };
    // Screen point from (along, across) in the host's frame.
    let pt = |h: &Host, along: f64, across: f64| if h.horiz { (tx(along), ty(across)) } else { (tx(across), ty(along)) };
    let cut = |o: &mut String, id: &Id, h: &Host, a0: f64, a1: f64| {
        let (px0, py0) = pt(h, a0, h.cl - h.t / 2.0);
        let (px1, py1) = pt(h, a1, h.cl + h.t / 2.0);
        let _ = writeln!(
            o,
            r#"<rect class="cut" data-for="{}" x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}"/>"#,
            id,
            px0.min(px1),
            py0.min(py1),
            (px1 - px0).abs(),
            (py1 - py0).abs()
        );
    };

    // Windows: cut the wall and draw the glass.
    for (e, win) in &windows {
        let Some(h) = host_of(e) else { continue };
        let (a0, a1) = (h.start + (win.offset_center - win.width / 2) as f64, h.start + (win.offset_center + win.width / 2) as f64);
        let _ = writeln!(o, r#"<g class="window" data-id="{}">"#, e.id);
        cut(&mut o, &e.id, &h, a0, a1);
        for across in [h.cl - h.t / 2.0, h.cl + h.t / 2.0] {
            let (p, q) = (pt(&h, a0, across), pt(&h, a1, across));
            let _ = writeln!(o, r#"<line class="frame" x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}"/>"#, p.0, p.1, q.0, q.1);
        }
        let (p, q) = (pt(&h, a0, h.cl), pt(&h, a1, h.cl));
        let _ = writeln!(o, r#"<line class="glass" x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}"/>"#, p.0, p.1, q.0, q.1);
        let _ = writeln!(o, "</g>");
    }

    // Doors: cut the wall, draw the leaf at 90 degrees and its swing arc.
    let space_by_key: BTreeMap<&str, &Space> = spaces.iter().map(|(_, sp)| (sp.requirement_key.as_str(), sp)).collect();
    for (e, door) in &doors {
        let Some(h) = host_of(e) else { continue };
        let w = door.width as f64;
        let (a0, a1) = (h.start + door.offset_center as f64 - w / 2.0, h.start + door.offset_center as f64 + w / 2.0);
        let _ = writeln!(o, r#"<g class="door" data-id="{}">"#, e.id);
        cut(&mut o, &e.id, &h, a0, a1);
        // Swing towards the room the door opens into.
        let side = space_by_key
            .get(door.opens_into.as_str())
            .map(|sp| {
                if h.horiz { sp.y as f64 + sp.h as f64 / 2.0 - h.cl } else { sp.x as f64 + sp.w as f64 / 2.0 - h.cl }
            })
            .map_or(1.0, |d| if d >= 0.0 { 1.0 } else { -1.0 });
        let hinge = pt(&h, a0, h.cl);
        let tip = pt(&h, a0, h.cl + side * w);
        let end = pt(&h, a1, h.cl);
        let n = (tip.0 - hinge.0, tip.1 - hinge.1);
        let d = (end.0 - hinge.0, end.1 - hinge.1);
        let sweep = if n.0 * d.1 - n.1 * d.0 > 0.0 { 1 } else { 0 };
        let r = w * s;
        let _ = writeln!(o, r#"<line class="leaf" x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}"/>"#, hinge.0, hinge.1, tip.0, tip.1);
        let _ = writeln!(
            o,
            r#"<path class="swing" d="M {:.1} {:.1} A {r:.1} {r:.1} 0 0 {sweep} {:.1} {:.1}"/>"#,
            tip.0, tip.1, end.0, end.1
        );
        let _ = writeln!(o, "</g>");
    }

    // Labels.
    for (e, sp) in &spaces {
        let name = opts
            .labels
            .get(&sp.requirement_key)
            .or_else(|| opts.labels.get(&sp.func))
            .cloned()
            .unwrap_or_else(|| sp.requirement_key.clone());
        let area = sp.net_area.unwrap_or(sp.area);
        let cx = tx(sp.x as f64 + sp.w as f64 / 2.0);
        let cy = ty(sp.y as f64 + sp.h as f64 / 2.0);
        let _ = writeln!(o, r#"<text class="name" data-for="{}" x="{:.1}" y="{:.1}">{}</text>"#, e.id, cx, cy - 4.0, esc(&name));
        let _ = writeln!(
            o,
            r#"<text class="meta" data-for="{}" x="{:.1}" y="{:.1}">{:.1} m²  ·  {:.2} × {:.2}</text>"#,
            e.id,
            cx,
            cy + 10.0,
            area,
            sp.w as f64 / 1000.0,
            sp.h as f64 / 1000.0
        );
    }

    // Structure overlay: beams under columns, grid references above.
    if opts.show_structure {
        for (e, b) in &collect::<Beam>(model, level) {
            let _ = writeln!(
                o,
                r#"<line class="beam" data-id="{}" x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}"/>"#,
                e.id,
                tx(b.x1 as f64),
                ty(b.y1 as f64),
                tx(b.x2 as f64),
                ty(b.y2 as f64)
            );
        }
        for (e, c) in &collect::<Column>(model, level) {
            let (px, py) = (tx(c.x as f64 - c.b as f64 / 2.0), ty(c.y as f64 + c.h as f64 / 2.0));
            let _ = writeln!(
                o,
                r#"<rect class="column" data-id="{}" x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}"/>"#,
                e.id,
                px,
                py,
                c.b as f64 * s,
                c.h as f64 * s
            );
            let _ = writeln!(o, r#"<text class="gridref" x="{:.1}" y="{:.1}">{}</text>"#, tx(c.x as f64), py - 2.0, esc(&c.grid_ref));
        }
    }

    // MEP overlay: one symbol per point, each carrying the point's id.
    if opts.show_mep {
        for e in model.elements.values().filter(|e| e.kind == MepPoint::KIND) {
            let Ok(p) = e.typed::<MepPoint>() else { continue };
            let (x, y) = (tx(p.x as f64), ty(p.y as f64));
            let tip = esc(&p.label.clone().unwrap_or_else(|| p.role.clone()));
            let _ = match p.role.as_str() {
                "light" => writeln!(o, r#"<circle class="mep mep-light" data-id="{}" cx="{x:.1}" cy="{y:.1}" r="3.6"><title>{tip}</title></circle>"#, e.id),
                "socket" => writeln!(o, r#"<rect class="mep mep-socket" data-id="{}" x="{:.1}" y="{:.1}" width="6" height="6"><title>{tip}</title></rect>"#, e.id, x - 3.0, y - 3.0),
                "fixture" => writeln!(o, r#"<circle class="mep mep-fixture" data-id="{}" cx="{x:.1}" cy="{y:.1}" r="4.2"><title>{tip}</title></circle>"#, e.id),
                "panel" => writeln!(o, r#"<rect class="mep mep-panel" data-id="{}" x="{:.1}" y="{:.1}" width="12" height="12"><title>{tip}</title></rect>"#, e.id, x - 6.0, y - 6.0),
                "diffuser" => writeln!(o, r#"<rect class="mep mep-diffuser" data-id="{}" x="{:.1}" y="{:.1}" width="14" height="14"><title>{tip}</title></rect>"#, e.id, x - 7.0, y - 7.0),
                "grille" => writeln!(o, r#"<rect class="mep mep-grille" data-id="{}" x="{:.1}" y="{:.1}" width="12" height="12"><title>{tip}</title></rect>"#, e.id, x - 6.0, y - 6.0),
                _ => writeln!(
                    o,
                    r#"<g class="mep" data-id="{}"><rect class="mep-ac" x="{:.1}" y="{:.1}" width="26" height="12"/><text class="mep-txt" x="{x:.1}" y="{:.1}">{tip}</text></g>"#,
                    e.id,
                    x - 13.0,
                    y - 6.0,
                    y + 3.0
                ),
            };
        }
    }

    // Ducts, pipes, drains and cable trays of the level, drawn at their real width in plan.
    if opts.show_mep {
        for e in model.elements.values().filter(|e| e.kind == crate::typed::MepRun::KIND && (level.is_none() || e.level_id == level)) {
            let Ok(r) = e.typed::<crate::typed::MepRun>() else { continue };
            let w = (r.width as f64 * s).max(1.0);
            let tip = esc(&format!("{} {}x{}{}", r.system, r.width, r.height, r.label.as_ref().map_or(String::new(), |l| format!(" {l}"))));
            if r.x1 == r.x2 && r.y1 == r.y2 {
                let _ = writeln!(o, r#"<circle class="run run-{}" data-id="{}" cx="{:.1}" cy="{:.1}" r="{:.1}" style="fill:#7a7a7a"><title>{tip}</title></circle>"#, esc(&r.system), e.id, tx(r.x1 as f64), ty(r.y1 as f64), w / 2.0);
            } else {
                let _ = writeln!(o, r#"<line class="run run-{}" data-id="{}" x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke-width="{w:.1}"><title>{tip}</title></line>"#, esc(&r.system), e.id, tx(r.x1 as f64), ty(r.y1 as f64), tx(r.x2 as f64), ty(r.y2 as f64));
            }
        }
    }

    // Overall dimensions.
    if opts.show_dims {
        let (bx0, bx1, by) = (tx(minx), tx(maxx), ty(miny) + 34.0);
        let _ = writeln!(o, r#"<path class="dim" d="M {bx0:.1} {by:.1} H {bx1:.1} M {bx0:.1} {:.1} V {:.1} M {bx1:.1} {:.1} V {:.1}"/>"#, by - 5.0, by + 5.0, by - 5.0, by + 5.0);
        let _ = writeln!(o, r#"<text class="dimtxt" x="{:.1}" y="{:.1}">{:.2} m</text>"#, (bx0 + bx1) / 2.0, by - 6.0, (maxx - minx) / 1000.0);
        let (dx, y0, y1) = (tx(minx) - 34.0, ty(maxy), ty(miny));
        let _ = writeln!(o, r#"<path class="dim" d="M {dx:.1} {y0:.1} V {y1:.1} M {:.1} {y0:.1} H {:.1} M {:.1} {y1:.1} H {:.1}"/>"#, dx - 5.0, dx + 5.0, dx - 5.0, dx + 5.0);
        let _ = writeln!(
            o,
            r#"<text class="dimtxt" transform="translate({:.1} {:.1}) rotate(-90)">{:.2} m</text>"#,
            dx - 8.0,
            (y0 + y1) / 2.0,
            (maxy - miny) / 1000.0
        );
    }

    // North arrow, scale bar, title block.
    let (nx, ny) = (width - m + 20.0, m - 20.0);
    let _ = writeln!(
        o,
        r##"<g class="north"><path d="M {nx:.1} {:.1} L {:.1} {:.1} L {nx:.1} {:.1} L {:.1} {:.1} Z" fill="#111"/><text class="dimtxt" x="{nx:.1}" y="{:.1}">N</text></g>"##,
        ny - 18.0,
        nx + 7.0,
        ny + 8.0,
        ny + 2.0,
        nx - 7.0,
        ny + 8.0,
        ny - 22.0
    );
    let bar = 5.0 * opts.px_per_m;
    let by = height - 82.0;
    let _ = writeln!(
        o,
        r#"<g class="scale"><path class="dim" d="M {m:.1} {by:.1} h {bar:.1} M {m:.1} {:.1} v 8 M {:.1} {:.1} v 8"/><text class="dimtxt" x="{:.1}" y="{:.1}">5 m</text></g>"#,
        by - 4.0,
        m + bar,
        by - 4.0,
        m + bar / 2.0,
        by - 9.0
    );
    let _ = writeln!(o, r#"<text class="title" x="{:.1}" y="{:.1}">{}</text>"#, m, height - 36.0, esc(&opts.title));
    let _ = writeln!(
        o,
        r#"<text class="note" x="{:.1}" y="{:.1}">Preliminary schematic. Not for construction; requires review by a licensed engineer.</text>"#,
        m,
        height - 10.0
    );
    o.push_str("</svg>\n");
    Ok(o)
}
