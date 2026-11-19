//! Quantity take-off, computed from the model only.
//!
//! Every quantity is derived from typed elements and carries the ids of the elements it
//! came from, so a quantity can be traced back to the drawing and forward to the schedule.
//! Nothing is stored: change the model and compute again.
//!
//! Conventions (stated so they can be checked):
//! - Wall area is length x height minus the doors and windows hosted by the wall.
//!   Wall pieces overlap at corners, so wall quantities are slightly over-counted until
//!   corner trimming exists.
//! - A beam's quantity is the part below the slab (depth minus the thickest adjacent slab).
//! - Column height is the tallest wall height in the model (storey height).
//! - Footing volume, excavation and all reinforcement are NOT quantified: they need a design
//!   (depth, bar sizes) this application does not make. The plan area and count are given.
//! - Waste is applied only where `QtoOpts::waste_pct` names the item code. No default.
//! - No prices. Rates belong to the project and are entered with their source and date.

use crate::model::{Element, Id, Model};
use crate::typed::{Beam, Column, Door, Footing, Slab, Space, Typed, Wall, Window};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    M,
    M2,
    M3,
    Nr,
}

impl Unit {
    pub fn label(self) -> &'static str {
        match self {
            Unit::M => "m",
            Unit::M2 => "m2",
            Unit::M3 => "m3",
            Unit::Nr => "nr",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct QtoItem {
    /// Local item code, for example `SLAB.VOL` or `DOOR.900x2100`.
    pub code: String,
    pub description: String,
    pub unit: Unit,
    pub quantity: f64,
    /// Waste allowance in percent, only if the user supplied one for this code.
    pub waste_pct: Option<f64>,
    /// Elements this quantity was measured from.
    pub sources: Vec<Id>,
    /// Each source element's share of the quantity, for items that are a plain sum over elements
    /// (frame concrete and formwork). Empty when the item is a derived total.
    pub parts: Vec<(Id, f64)>,
}

impl QtoItem {
    /// Quantity to order: the measured quantity plus the user's waste allowance, if any.
    pub fn ordered(&self) -> f64 {
        self.quantity * (1.0 + self.waste_pct.unwrap_or(0.0) / 100.0)
    }
}

#[derive(Debug, Clone, Default)]
pub struct QtoOpts {
    /// Waste allowance by item code, in percent. Empty by default.
    pub waste_pct: BTreeMap<String, f64>,
    /// Restrict to one level; `None` takes every level.
    pub level: Option<Id>,
}

#[derive(Debug, Clone, Default)]
pub struct Qto {
    pub items: Vec<QtoItem>,
    pub warnings: Vec<String>,
}

impl Qto {
    pub fn get(&self, code: &str) -> Option<&QtoItem> {
        self.items.iter().find(|i| i.code == code)
    }

    /// CSV with a UTF-8 byte-order mark so Excel reads Arabic text correctly.
    pub fn to_csv(&self) -> String {
        fn cell(s: &str) -> String {
            if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
        }
        let mut out = String::from("\u{feff}code,description,unit,quantity,waste_pct,ordered_quantity,source_count,sources,status\n");
        for i in &self.items {
            let sources = i.sources.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(";");
            out.push_str(&format!(
                "{},{},{},{:.3},{},{:.3},{},{},preliminary\n",
                cell(&i.code),
                cell(&i.description),
                i.unit.label(),
                i.quantity,
                i.waste_pct.map_or(String::new(), |w| w.to_string()),
                i.ordered(),
                i.sources.len(),
                sources
            ));
        }
        out
    }
}

fn collect<T: Typed>(model: &Model, level: Option<Id>) -> Vec<(&Element, T)> {
    model
        .elements
        .values()
        .filter(|e| e.kind == T::KIND && (level.is_none() || e.level_id == level))
        .filter_map(|e| e.typed::<T>().ok().map(|t| (e, t)))
        .collect()
}

/// Rounded to 4 decimals to drop floating noise.
fn r4(v: f64) -> f64 {
    (v * 1e4).round() / 1e4
}

const MM2_PER_M2: f64 = 1e6;
const MM3_PER_M3: f64 = 1e9;

struct Acc<'a> {
    items: Vec<QtoItem>,
    opts: &'a QtoOpts,
}

impl Acc<'_> {
    fn add(&mut self, code: &str, description: &str, unit: Unit, quantity: f64, sources: Vec<Id>) {
        if sources.is_empty() {
            return;
        }
        self.items.push(QtoItem {
            code: code.to_string(),
            description: description.to_string(),
            unit,
            quantity: r4(quantity),
            waste_pct: self.opts.waste_pct.get(code).copied(),
            sources,
            parts: Vec::new(),
        });
    }

    /// An item that is the sum of per-element quantities; keeps the breakdown.
    fn add_parts(&mut self, code: &str, description: &str, unit: Unit, parts: Vec<(Id, f64)>) {
        let total: f64 = parts.iter().map(|p| p.1).sum();
        let sources = parts.iter().map(|p| p.0).collect();
        self.add(code, description, unit, total, sources);
        if let Some(item) = self.items.last_mut().filter(|i| i.code == code) {
            item.parts = parts;
        }
    }
}

pub fn compute(model: &Model, opts: &QtoOpts) -> Qto {
    let mut acc = Acc { items: Vec::new(), opts };
    let mut warnings = Vec::new();

    let walls = collect::<Wall>(model, opts.level);
    let doors = collect::<Door>(model, opts.level);
    let windows = collect::<Window>(model, opts.level);
    let slabs = collect::<Slab>(model, opts.level);
    let beams = collect::<Beam>(model, opts.level);
    let columns = collect::<Column>(model, opts.level);
    let footings = collect::<Footing>(model, opts.level);
    let spaces = collect::<Space>(model, opts.level);

    // ---- walls: area net of hosted openings, then volume ----
    let mut open_by_wall: BTreeMap<Id, f64> = BTreeMap::new();
    for (e, d) in &doors {
        if let Some(h) = e.host_id {
            *open_by_wall.entry(h).or_default() += d.width as f64 * d.height as f64;
        }
    }
    for (e, w) in &windows {
        if let Some(h) = e.host_id {
            *open_by_wall.entry(h).or_default() += w.width as f64 * w.height as f64;
        }
    }
    let (mut ext_a, mut int_a, mut ext_v, mut int_v, mut deducted) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let (mut ext_ids, mut int_ids, mut open_ids) = (Vec::new(), Vec::new(), Vec::new());
    for (e, w) in &walls {
        let open = open_by_wall.get(&e.id).copied().unwrap_or(0.0);
        let gross = w.length as f64 * w.height as f64;
        if open > gross {
            warnings.push(format!("wall {} has more opening area than wall area", e.id));
        }
        let net = (gross - open).max(0.0);
        deducted += open.min(gross);
        if open > 0.0 {
            open_ids.push(e.id);
        }
        if w.is_exterior {
            ext_a += net / MM2_PER_M2;
            ext_v += net * w.thickness as f64 / MM3_PER_M3;
            ext_ids.push(e.id);
        } else {
            int_a += net / MM2_PER_M2;
            int_v += net * w.thickness as f64 / MM3_PER_M3;
            int_ids.push(e.id);
        }
    }
    acc.add("WALL.EXT.AREA", "Exterior walls, area net of openings", Unit::M2, ext_a, ext_ids.clone());
    acc.add("WALL.INT.AREA", "Interior walls, area net of openings", Unit::M2, int_a, int_ids.clone());
    acc.add("WALL.EXT.VOL", "Exterior walls, volume net of openings", Unit::M3, ext_v, ext_ids.clone());
    acc.add("WALL.INT.VOL", "Interior walls, volume net of openings", Unit::M3, int_v, int_ids.clone());
    acc.add("WALL.OPENINGS.AREA", "Door and window openings deducted from walls", Unit::M2, deducted / MM2_PER_M2, open_ids);
    let faces: Vec<Id> = ext_ids.iter().chain(&int_ids).copied().collect();
    acc.add("WALL.FACES.AREA", "Wall faces to finish (both sides, openings deducted)", Unit::M2, 2.0 * (ext_a + int_a), faces);

    // ---- openings ----
    let mut door_types: BTreeMap<(i64, i64), Vec<Id>> = BTreeMap::new();
    for (e, d) in &doors {
        door_types.entry((d.width, d.height)).or_default().push(e.id);
    }
    for ((w, h), ids) in door_types {
        acc.add(&format!("DOOR.{w}x{h}"), &format!("Door {w} x {h} mm"), Unit::Nr, ids.len() as f64, ids);
    }
    let mut win_types: BTreeMap<(i64, i64), Vec<Id>> = BTreeMap::new();
    let mut glazing = 0.0;
    for (e, w) in &windows {
        win_types.entry((w.width, w.height)).or_default().push(e.id);
        glazing += w.width as f64 * w.height as f64 / MM2_PER_M2;
    }
    let win_ids: Vec<Id> = windows.iter().map(|(e, _)| e.id).collect();
    for ((w, h), ids) in win_types {
        acc.add(&format!("WINDOW.{w}x{h}"), &format!("Window {w} x {h} mm"), Unit::Nr, ids.len() as f64, ids);
    }
    acc.add("WINDOW.AREA", "Total glazed opening area", Unit::M2, glazing, win_ids);

    // ---- slabs ----
    let slab_vol: Vec<(Id, f64)> = slabs.iter().map(|(e, s)| (e.id, s.w as f64 * s.h as f64 * s.thickness as f64 / MM3_PER_M3)).collect();
    let slab_form: Vec<(Id, f64)> = slabs.iter().map(|(e, s)| (e.id, s.w as f64 * s.h as f64 / MM2_PER_M2)).collect();
    acc.add_parts("SLAB.VOL", "Slab concrete", Unit::M3, slab_vol);
    acc.add_parts("SLAB.FORM", "Slab soffit formwork", Unit::M2, slab_form);

    // ---- beams: only the part below the slab ----
    let mut beam_vol: Vec<(Id, f64)> = Vec::new();
    let mut beam_form: Vec<(Id, f64)> = Vec::new();
    for (e, b) in &beams {
        let (mx, my) = ((b.x1 + b.x2) as f64 / 2.0, (b.y1 + b.y2) as f64 / 2.0);
        let tol = b.b as f64;
        let slab_t = slabs
            .iter()
            .filter(|(_, s)| {
                mx >= s.x as f64 - tol && mx <= (s.x + s.w) as f64 + tol && my >= s.y as f64 - tol && my <= (s.y + s.h) as f64 + tol
            })
            .map(|(_, s)| s.thickness)
            .max()
            .unwrap_or(0);
        let dropped = (b.h - slab_t).max(0) as f64;
        beam_vol.push((e.id, b.length as f64 * b.b as f64 * dropped / MM3_PER_M3));
        beam_form.push((e.id, b.length as f64 * (b.b as f64 + 2.0 * dropped) / MM2_PER_M2));
    }
    acc.add_parts("BEAM.VOL", "Beam concrete below the slab", Unit::M3, beam_vol);
    acc.add_parts("BEAM.FORM", "Beam formwork (soffit and two sides, below the slab)", Unit::M2, beam_form);

    // ---- columns ----
    let storey = walls.iter().map(|(_, w)| w.height).max();
    if !columns.is_empty() {
        match storey {
            Some(h) => {
                let per = |f: &dyn Fn(&Column) -> f64| -> Vec<(Id, f64)> { columns.iter().map(|(e, c)| (e.id, f(c))).collect() };
                let vol = per(&|c| c.b as f64 * c.h as f64 * h as f64 / MM3_PER_M3);
                let form = per(&|c| 2.0 * (c.b + c.h) as f64 * h as f64 / MM2_PER_M2);
                let count = per(&|_| 1.0);
                acc.add_parts("COLUMN.VOL", "Column concrete (storey height = tallest wall)", Unit::M3, vol);
                acc.add_parts("COLUMN.FORM", "Column formwork", Unit::M2, form);
                acc.add_parts("COLUMN.COUNT", "Columns", Unit::Nr, count);
            }
            None => warnings.push("columns exist but there is no wall to give the storey height: column quantities skipped".into()),
        }
    }

    // ---- footings: plan area and count only ----
    if !footings.is_empty() {
        let ids: Vec<Id> = footings.iter().map(|(e, _)| e.id).collect();
        let area: f64 = footings.iter().map(|(_, f)| f.length as f64 * f.width as f64 / MM2_PER_M2).sum();
        acc.add("FOOTING.AREA", "Footings, plan area", Unit::M2, area, ids.clone());
        acc.add("FOOTING.COUNT", "Footings", Unit::Nr, footings.len() as f64, ids);
        warnings.push("footing depth is not designed: footing concrete, excavation and reinforcement are not quantified".into());
    }
    if !slabs.is_empty() || !beams.is_empty() || !columns.is_empty() {
        warnings.push("reinforcement is not quantified: it needs a structural design".into());
    }

    // ---- finishes by room function ----
    let mut floor: BTreeMap<String, (f64, Vec<Id>)> = BTreeMap::new();
    let mut skirting: BTreeMap<String, (f64, Vec<Id>)> = BTreeMap::new();
    for (e, s) in &spaces {
        let area = s.net_area.unwrap_or(s.area);
        let f = floor.entry(s.func.clone()).or_default();
        f.0 += area;
        f.1.push(e.id);

        let door_widths: f64 = doors.iter().filter(|(_, d)| d.from == s.requirement_key || d.to == s.requirement_key).map(|(_, d)| d.width as f64).sum();
        let perimeter = 2.0 * (s.w + s.h) as f64;
        let k = skirting.entry(s.func.clone()).or_default();
        k.0 += ((perimeter - door_widths) / 1000.0).max(0.0);
        k.1.push(e.id);
    }
    for (func, (area, ids)) in floor {
        acc.add(&format!("FLOOR.AREA.{func}"), &format!("Floor finish, {func} (net area)"), Unit::M2, area, ids.clone());
        acc.add(&format!("CEILING.AREA.{func}"), &format!("Ceiling, {func} (net area)"), Unit::M2, area, ids);
    }
    for (func, (len, ids)) in skirting {
        acc.add(&format!("SKIRTING.{func}"), &format!("Skirting, {func} (perimeter minus doors)"), Unit::M, len, ids);
    }
    let all_spaces: Vec<Id> = spaces.iter().map(|(e, _)| e.id).collect();
    acc.add("SPACE.GROSS.AREA", "Total gross floor area", Unit::M2, spaces.iter().map(|(_, s)| s.area).sum(), all_spaces);

    Qto { items: acc.items, warnings }
}
