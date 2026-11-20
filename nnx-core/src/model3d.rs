//! 3D geometry derived from the model. Nothing is stored: change the model and build again.
//!
//! Every solid is an axis-aligned box in metres (x east, y north as in the plan, z up, floor top at z = 0) that
//! carries the id of the element it came from, so the 3D view and the drawing, quantities and schedule all
//! point at the same elements.
//!
//! - A wall is cut into boxes around its doors and windows: solid pieces between openings, the part under a
//!   window sill and the lintel above every opening. The solid volume therefore equals the wall volume of the
//!   quantity take-off (net of openings).
//! - A door gets a thin closed leaf and a window a thin glass pane (marked `glass`, drawn translucent).
//! - Columns run from the floor to the tallest wall, beams hang below the slab, the slab sits on top of the walls.
//! - Levels stack: the floor of level n+1 is the top of the slab of level n, so a level's elevation is the sum of the
//!   storey heights and slab thicknesses below it (derived here, never stored).
//! - Every box also carries its discipline (`DISCIPLINES`), so a viewer can isolate or hide architecture, structure,
//!   electrical, plumbing (with drainage) and HVAC; `Scene::filter` does the same on the data.
//! - Floor plates (one per room) are a visual aid 50 mm thick under the floor line. Footings have no designed depth in
//!   this application, so their 3D thickness is a fixed VISUAL value and is flagged `visual`.

use crate::model::{Id, Model};
use crate::typed::{Beam, Column, Door, Footing, MepPoint, MepRun, Slab, Space, TieBeam, Typed, Wall, Window};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
pub struct Box3 {
    pub id: String,
    /// Element kind: Wall, Slab, Column, Beam, Footing, Door, Window, Space, MepPoint.
    pub kind: &'static str,
    /// Finer class: wall `ext`/`int`, room function, MEP role, `glass`, `leaf`.
    pub class: String,
    pub min: [f64; 3],
    pub max: [f64; 3],
    /// True when the size is not derived from a design (footing thickness).
    pub visual: bool,
    /// Index of the level the element belongs to (`None` for a model without levels).
    pub level: Option<i64>,
    /// One of `DISCIPLINES`.
    pub discipline: &'static str,
}

/// The layers a scene can be isolated by.
pub const DISCIPLINES: [&str; 5] = ["architectural", "structural", "electrical", "plumbing", "hvac"];

/// The layer of an element kind and class (an MEP point's role, a run's system).
fn discipline_of(kind: &str, class: &str) -> &'static str {
    match kind {
        "Column" | "Beam" | "Footing" | "Slab" | "TieBeam" => "structural",
        "MepPoint" => match class {
            "light" | "socket" | "panel" => "electrical",
            "ac_indoor" | "diffuser" | "grille" => "hvac",
            _ => "plumbing",
        },
        "MepRun" => match class {
            "duct" => "hvac",
            "cable" => "electrical",
            _ => "plumbing",
        },
        _ => "architectural",
    }
}

impl Box3 {
    pub fn volume(&self) -> f64 {
        (0..3).map(|i| self.max[i] - self.min[i]).product()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LevelInfo {
    pub id: String,
    pub name: String,
    pub index: i64,
    /// Elevation of the level's floor top, metres.
    pub z0: f64,
    pub storey: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Scene {
    pub levels: Vec<LevelInfo>,
    pub boxes: Vec<Box3>,
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub storey_height: f64,
    /// Wall id -> (assembly kind, assembly name) for the walls that use a wall assembly, so the view can colour and name them.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub assemblies: BTreeMap<String, (String, String)>,
}

impl Scene {
    /// The scene with only the boxes of the given disciplines (names from `DISCIPLINES`; unknown names match nothing).
    pub fn filter(&self, keep: &[&str]) -> Scene {
        let boxes: Vec<Box3> = self.boxes.iter().filter(|b| keep.contains(&b.discipline)).cloned().collect();
        let mut min = [f64::MAX; 3];
        let mut max = [f64::MIN; 3];
        for b in &boxes {
            for i in 0..3 {
                min[i] = min[i].min(b.min[i]);
                max[i] = max[i].max(b.max[i]);
            }
        }
        if boxes.is_empty() {
            min = [0.0; 3];
            max = [0.0; 3];
        }
        Scene { levels: self.levels.clone(), boxes, min, max, storey_height: self.storey_height, assemblies: self.assemblies.clone() }
    }
}

/// One opening in a wall: start and end along the wall, sill, head, is-door, element id.
type Opening = (f64, f64, f64, f64, bool, Id);

const M: f64 = 1000.0;
const FLOOR_PLATE: f64 = 0.05;
const FOOTING_VISUAL: f64 = 0.25;
const LEAF: f64 = 0.04;
const PANE: f64 = 0.02;
const MEP_SIZE: f64 = 0.12;

fn bx(id: Id, kind: &'static str, class: &str, min: [f64; 3], max: [f64; 3], visual: bool) -> Box3 {
    Box3 { id: id.to_string(), kind, class: class.to_string(), min, max, visual, level: None, discipline: discipline_of(kind, class) }
}

/// A box along a wall: `s0..s1` along its length, `z0..z1` up, centred on the wall's centre line, of its thickness.
fn along(w: &Wall, s0: f64, s1: f64, z0: f64, z1: f64) -> ([f64; 3], [f64; 3]) {
    let t = w.thickness as f64 / M;
    let c = w.centre_line() as f64 / M;
    if w.horizontal() { ([s0, c - t / 2.0, z0], [s1, c + t / 2.0, z1]) } else { ([c - t / 2.0, s0, z0], [c + t / 2.0, s1, z1]) }
}

/// The scene with the facade solids of `style` added (`style::facade_boxes`); the model and the plain scene are unchanged.
pub fn build_styled(model: &Model, style: Option<crate::style::Style>) -> Scene {
    let mut scene = build(model);
    let mut extra: Vec<Box3> = Vec::new();
    if let Some(s) = style {
        extra.extend(crate::style::facade_boxes(&scene, s));
    }
    // What the project stores as inputs and the scene only draws: site layout, vertical elements, furniture.
    if let Some(p) = crate::project::find(model).and_then(|e| e.typed::<crate::project::NanoArchProject>().ok()) {
        if let (Some(input), Some(Ok(r))) = (&p.meta.site_layout, p.site_result())
            && let Ok(l) = crate::site_layout::layout(&r, input)
        {
            for (n, (kind, rc, z0, z1)) in crate::site_layout::solids(&r, &l).into_iter().enumerate() {
                extra.push(Box3 { id: format!("site:{}", n + 1), kind: "Site", class: kind.to_string(), min: [rc[0], rc[1], z0], max: [rc[2], rc[3], z1], visual: true, level: None, discipline: "architectural" });
            }
        }
        if let Some(v) = &p.meta.vertical {
            extra.extend(crate::vertical::boxes(&scene, v));
        }
        if !p.meta.mep_ext.cctv.cameras.is_empty() {
            extra.extend(crate::wireless::cctv_boxes(model, &p.meta.mep_ext.cctv));
        }
        if !p.meta.mep_ext.wifi.aps.is_empty() {
            extra.extend(crate::wireless::ap_boxes(model, &p.meta.mep_ext.wifi));
        }
        if let Some(f) = &p.meta.furniture
            && let Ok(rep) = crate::furniture::plan(&scene, f)
        {
            extra.extend(crate::furniture::boxes(&rep));
        }
    }
    for b in &extra {
        for i in 0..3 {
            scene.min[i] = scene.min[i].min(b.min[i]);
            scene.max[i] = scene.max[i].max(b.max[i]);
        }
    }
    scene.boxes.extend(extra);
    scene
}

/// The elevation, in metres, of the floor of `level` above the floor of the ground level: each level sits on the slab of the one below
/// (the same stacking `build` uses). `None`, or a level that is not in the model, is the ground level: 0.
pub fn level_elevation_m(model: &Model, level: Option<Id>) -> f64 {
    let walls: Vec<Wall> = model.elements.values().filter(|e| e.kind == Wall::KIND).filter_map(|e| e.typed::<Wall>().ok()).collect();
    let global_storey = walls.iter().map(|w| w.height).max().unwrap_or(3000) as f64 / M;
    let mut z = 0.0;
    for (_, lid) in crate::building::levels(model) {
        if Some(lid) == level {
            return z;
        }
        let storey = model.elements.values().filter(|e| e.kind == Wall::KIND && e.level_id == Some(lid)).filter_map(|e| e.typed::<Wall>().ok()).map(|w| w.height).max().map_or(global_storey, |h| h as f64 / M);
        let slab_t = model.elements.values().filter(|e| e.kind == Slab::KIND && e.level_id == Some(lid)).filter_map(|e| e.typed::<Slab>().ok()).map(|s| s.thickness).max().unwrap_or(0) as f64 / M;
        z += storey + slab_t;
    }
    0.0
}

pub fn build(model: &Model) -> Scene {
    let mut boxes: Vec<Box3> = Vec::new();
    let walls: Vec<(Id, Wall)> = model.elements.values().filter(|e| e.kind == Wall::KIND).filter_map(|e| Some((e.id, e.typed::<Wall>().ok()?))).collect();
    let global_storey = walls.iter().map(|(_, w)| w.height).max().unwrap_or(3000) as f64 / M;

    // Level elevations: each level's floor sits on the slab of the one below.
    let mut levels: Vec<LevelInfo> = Vec::new();
    let mut z = 0.0;
    for (index, lid) in crate::building::levels(model) {
        let storey = walls.iter().filter(|(wid, _)| model.get(wid).is_some_and(|e| e.level_id == Some(lid))).map(|(_, w)| w.height).max().map_or(global_storey, |h| h as f64 / M);
        let slab_t = model.elements.values().filter(|e| e.kind == Slab::KIND && e.level_id == Some(lid)).filter_map(|e| e.typed::<Slab>().ok()).map(|s| s.thickness).max().unwrap_or(0) as f64 / M;
        let name = model.get(&lid).and_then(|e| e.typed::<crate::typed::Level>().ok()).map(|l| l.name).unwrap_or_default();
        levels.push(LevelInfo { id: lid.to_string(), name, index, z0: z, storey });
        z += storey + slab_t;
    }
    let info = |lvl: Option<Id>| -> (f64, f64, Option<i64>) { lvl.and_then(|l| levels.iter().find(|i| i.id == l.to_string())).map_or((0.0, global_storey, None), |i| (i.z0, i.storey, Some(i.index))) };
    let shift = |boxes: &mut [Box3], dz: f64, level: Option<i64>| {
        for b in boxes {
            b.min[2] += dz;
            b.max[2] += dz;
            b.level = level;
        }
    };

    // Openings by host wall: (start along the wall, end, sill, head, is_door, id).
    let mut open: BTreeMap<Id, Vec<Opening>> = BTreeMap::new();
    for e in model.elements.values() {
        let Some(host) = e.host_id else { continue };
        let Some((_, w)) = walls.iter().find(|(id, _)| *id == host) else { continue };
        let s = w.start() as f64 / M;
        if e.kind == Door::KIND {
            if let Ok(d) = e.typed::<Door>() {
                let c = s + d.offset_center as f64 / M;
                open.entry(host).or_default().push((c - d.width as f64 / M / 2.0, c + d.width as f64 / M / 2.0, 0.0, d.height as f64 / M, true, e.id));
            }
        } else if e.kind == Window::KIND
            && let Ok(win) = e.typed::<Window>()
        {
            let c = s + win.offset_center as f64 / M;
            let sill = win.sill_height as f64 / M;
            open.entry(host).or_default().push((c - win.width as f64 / M / 2.0, c + win.width as f64 / M / 2.0, sill, sill + win.height as f64 / M, false, e.id));
        }
    }

    let resolver = crate::assembly::Resolver::of(model);
    let mut assemblies: BTreeMap<String, (String, String)> = BTreeMap::new();
    for (id, w) in &walls {
        let (z0, _, lvl) = info(model.get(id).and_then(|e| e.level_id));
        let start = boxes.len();
        if let Some(a) = model.get(id).and_then(|e| resolver.of_wall(e, w)) {
            assemblies.insert(id.to_string(), (a.kind.id().to_string(), a.name.clone()));
        }
        let class = if w.is_exterior { "ext" } else { "int" };
        let (s0, s1, h) = (w.start() as f64 / M, (w.start() + w.length) as f64 / M, w.height as f64 / M);
        let mut ops = open.remove(id).unwrap_or_default();
        ops.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut cursor = s0;
        for (a, b, sill, head, door, oid) in ops {
            let (a, b) = (a.max(cursor), b.min(s1));
            if b <= a {
                continue;
            }
            if a > cursor {
                let (lo, hi) = along(w, cursor, a, 0.0, h);
                boxes.push(bx(*id, Wall::KIND, class, lo, hi, false));
            }
            if sill > 0.0 {
                let (lo, hi) = along(w, a, b, 0.0, sill);
                boxes.push(bx(*id, Wall::KIND, class, lo, hi, false));
            }
            if head < h {
                let (lo, hi) = along(w, a, b, head, h);
                boxes.push(bx(*id, Wall::KIND, class, lo, hi, false));
            }
            // The closed leaf or the pane, thinner than the wall and centred in it.
            let t = (w.thickness as f64 / M).min(if door { LEAF } else { PANE });
            let (mut lo, mut hi) = along(w, a, b, sill, head);
            let mid = w.centre_line() as f64 / M;
            let ax = if w.horizontal() { 1 } else { 0 };
            lo[ax] = mid - t / 2.0;
            hi[ax] = mid + t / 2.0;
            boxes.push(bx(oid, if door { Door::KIND } else { Window::KIND }, if door { "leaf" } else { "glass" }, lo, hi, false));
            cursor = b;
        }
        if cursor < s1 {
            let (lo, hi) = along(w, cursor, s1, 0.0, h);
            boxes.push(bx(*id, Wall::KIND, class, lo, hi, false));
        }
        shift(&mut boxes[start..], z0, lvl);
    }

    for e in model.elements.values() {
        let (z0, storey, lvl) = info(e.level_id);
        let start = boxes.len();
        match e.kind.as_str() {
            Slab::KIND => {
                if let Ok(s) = e.typed::<Slab>() {
                    let t = s.thickness as f64 / M;
                    boxes.push(bx(e.id, Slab::KIND, "slab", [s.x as f64 / M, s.y as f64 / M, storey], [(s.x + s.w) as f64 / M, (s.y + s.h) as f64 / M, storey + t], false));
                }
            }
            Column::KIND => {
                if let Ok(c) = e.typed::<Column>() {
                    let (cx, cy, hb, hh) = (c.x as f64 / M, c.y as f64 / M, c.b as f64 / M / 2.0, c.h as f64 / M / 2.0);
                    boxes.push(bx(e.id, Column::KIND, "column", [cx - hb, cy - hh, 0.0], [cx + hb, cy + hh, storey], false));
                }
            }
            Beam::KIND => {
                if let Ok(b) = e.typed::<Beam>() {
                    let (hw, d) = (b.b as f64 / M / 2.0, b.h as f64 / M);
                    let (x0, x1) = (b.x1.min(b.x2) as f64 / M, b.x1.max(b.x2) as f64 / M);
                    let (y0, y1) = (b.y1.min(b.y2) as f64 / M, b.y1.max(b.y2) as f64 / M);
                    let (lo, hi) = if b.y1 == b.y2 { ([x0, y0 - hw, storey - d], [x1, y0 + hw, storey]) } else { ([x0 - hw, y0, storey - d], [x0 + hw, y1, storey]) };
                    boxes.push(bx(e.id, Beam::KIND, "beam", lo, hi, false));
                }
            }
            Footing::KIND => {
                if let Ok(f) = e.typed::<Footing>() {
                    let (cx, cy, hl, hw) = (f.x as f64 / M, f.y as f64 / M, f.length as f64 / M / 2.0, f.width as f64 / M / 2.0);
                    boxes.push(bx(e.id, Footing::KIND, "footing", [cx - hl, cy - hw, -FLOOR_PLATE - FOOTING_VISUAL], [cx + hl, cy + hw, -FLOOR_PLATE], true));
                }
            }
            TieBeam::KIND => {
                if let Ok(b) = e.typed::<TieBeam>() {
                    let hw = b.width as f64 / M / 2.0;
                    let (x0, x1) = (b.x1.min(b.x2) as f64 / M, b.x1.max(b.x2) as f64 / M);
                    let (y0, y1) = (b.y1.min(b.y2) as f64 / M, b.y1.max(b.y2) as f64 / M);
                    // Its top is level with the top of the footings.
                    let (zt, zb) = (-FLOOR_PLATE, -FLOOR_PLATE - b.depth as f64 / M);
                    let (lo, hi) = if b.y1 == b.y2 { ([x0, y0 - hw, zb], [x1, y0 + hw, zt]) } else { ([x0 - hw, y0, zb], [x0 + hw, y1, zt]) };
                    boxes.push(bx(e.id, TieBeam::KIND, "tiebeam", lo, hi, false));
                }
            }
            Space::KIND => {
                if let Ok(s) = e.typed::<Space>() {
                    boxes.push(bx(e.id, Space::KIND, &s.func, [s.x as f64 / M, s.y as f64 / M, -FLOOR_PLATE], [(s.x + s.w) as f64 / M, (s.y + s.h) as f64 / M, 0.0], false));
                }
            }
            MepPoint::KIND => {
                if let Ok(p) = e.typed::<MepPoint>() {
                    let (cx, cy) = (p.x as f64 / M, p.y as f64 / M);
                    let (z0, z1) = match p.role.as_str() {
                        "light" => (storey - MEP_SIZE, storey),
                        "socket" => (0.3, 0.3 + MEP_SIZE),
                        "panel" => (1.2, 1.2 + 3.0 * MEP_SIZE),
                        "ac_indoor" => (storey - 0.3, storey),
                        "diffuser" | "grille" => (storey - 0.4, storey - 0.25),
                        _ => (0.0, MEP_SIZE),
                    };
                    let h = if p.role == "ac_indoor" { 0.5 } else { MEP_SIZE / 2.0 };
                    boxes.push(bx(e.id, MepPoint::KIND, &p.role, [cx - h, cy - MEP_SIZE / 2.0, z0], [cx + h, cy + MEP_SIZE / 2.0, z1], false));
                }
            }
            MepRun::KIND => {
                if let Ok(r) = e.typed::<MepRun>() {
                    let (hw, hh) = (r.width as f64 / M / 2.0, r.height as f64 / M / 2.0);
                    let (x0, x1) = (r.x1.min(r.x2) as f64 / M, r.x1.max(r.x2) as f64 / M);
                    let (y0, y1) = (r.y1.min(r.y2) as f64 / M, r.y1.max(r.y2) as f64 / M);
                    let (z0, z1) = (r.z1.min(r.z2) as f64 / M, r.z1.max(r.z2) as f64 / M);
                    // The two axes across the run grow by the half-section; heights are centre lines. A falling pipe is
                    // the hull of its slope (lowest to highest centre line).
                    let (lo, hi) = if r.x1 != r.x2 {
                        ([x0, y0 - hw, z0 - hh], [x1, y0 + hw, z1 + hh])
                    } else if r.y1 != r.y2 {
                        ([x0 - hw, y0, z0 - hh], [x0 + hw, y1, z1 + hh])
                    } else {
                        ([x0 - hw, y0 - hh, z0], [x0 + hw, y0 + hh, z1])
                    };
                    boxes.push(bx(e.id, MepRun::KIND, &r.system, lo, hi, false));
                }
            }
            _ => {}
        }
        // Footings sit under the ground floor whatever level they are filed under.
        let dz = if e.kind == Footing::KIND || e.kind == TieBeam::KIND { 0.0 } else { z0 };
        shift(&mut boxes[start..], dz, lvl);
    }

    let mut min = [f64::MAX; 3];
    let mut max = [f64::MIN; 3];
    for b in &boxes {
        for i in 0..3 {
            min[i] = min[i].min(b.min[i]);
            max[i] = max[i].max(b.max[i]);
        }
    }
    if boxes.is_empty() {
        min = [0.0; 3];
        max = [0.0; 3];
    }
    Scene { levels, boxes, min, max, storey_height: global_storey, assemblies }
}
