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
use crate::typed::{Beam, Column, Door, Feature, Footing, Slab, Space, Typed, Wall, Window};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    M,
    M2,
    M3,
    Nr,
    Kg,
}

impl Unit {
    pub fn label(self) -> &'static str {
        match self {
            Unit::M => "m",
            Unit::M2 => "m2",
            Unit::M3 => "m3",
            Unit::Nr => "nr",
            Unit::Kg => "kg",
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
    /// Restrict to the elements of one construction phase (`EXISTING`, `NEW`, `TO_BE_DEMOLISHED`, `TO_BE_MODIFIED`, `TEMPORARY`,
    /// `UNVERIFIED`), or `UNSET` for those that carry no phase. `None` takes every element. Structural members (slabs, beams, columns,
    /// footings, tie beams) and MEP points and runs take a phase from `set_phase` (by room or location); until it is set they appear under `None` or `UNSET`.
    pub phase: Option<String>,
}

/// One element's share of one quantity item: the link from the take-off back to the model element it was measured on.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementQty {
    pub element: Id,
    /// The item code this share belongs to; the shares of one code add up to that item's `quantity`.
    pub code: String,
    pub unit: Unit,
    pub quantity: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Qto {
    pub items: Vec<QtoItem>,
    pub warnings: Vec<String>,
    /// Per-element shares of the items `compute` measures element by element. Items that other modules append
    /// (MEP, insulation, finishes) have no rows here; `unlinked_codes` names every item without any.
    pub element_rows: Vec<ElementQty>,
}

impl Qto {
    /// Every quantity measured on one element.
    pub fn for_element(&self, id: Id) -> Vec<&ElementQty> {
        self.element_rows.iter().filter(|r| r.element == id).collect()
    }

    /// Codes of items that carry no per-element breakdown.
    pub fn unlinked_codes(&self) -> Vec<&str> {
        self.items.iter().filter(|i| !self.element_rows.iter().any(|r| r.code == i.code)).map(|i| i.code.as_str()).collect()
    }

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

/// The construction phases an element can carry. Nothing is assumed: an element with no phase stays unset.
pub const PHASES: [&str; 6] = ["EXISTING", "TO_BE_DEMOLISHED", "TO_BE_MODIFIED", "NEW", "TEMPORARY", "UNVERIFIED"];

/// True when the element belongs to a requested construction-status phase.
/// An unset phase is never silently treated as NEW or EXISTING.
pub fn matches_phase(e: &Element, phase: Option<&str>) -> bool {
    match phase {
        None => true,
        Some("UNSET") => e.props.get("phase").and_then(|v| v.as_str()).is_none(),
        Some(p) => e.props.get("phase").and_then(|v| v.as_str()) == Some(p),
    }
}

fn collect<'a, T: Typed>(model: &'a Model, opts: &QtoOpts) -> Vec<(&'a Element, T)> {
    let (level, phase) = (opts.level, opts.phase.as_deref());
    model
        .elements
        .values()
        .filter(|e| e.kind == T::KIND && (level.is_none() || e.level_id == level))
        .filter(|e| matches_phase(e, phase))
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
    rows: Vec<ElementQty>,
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

    /// Records each element's share of an item. Call it with the same elements the item's `add` received.
    fn link(&mut self, code: &str, unit: Unit, shares: &[(Id, f64)]) {
        self.rows.extend(shares.iter().map(|(id, q)| ElementQty { element: *id, code: code.to_string(), unit, quantity: r4(*q) }));
    }

    /// An item that is the sum of per-element quantities; keeps the breakdown.
    fn add_parts(&mut self, code: &str, description: &str, unit: Unit, parts: Vec<(Id, f64)>) {
        let total: f64 = parts.iter().map(|p| p.1).sum();
        let sources = parts.iter().map(|p| p.0).collect();
        self.add(code, description, unit, total, sources);
        self.link(code, unit, &parts);
        if let Some(item) = self.items.last_mut().filter(|i| i.code == code) {
            item.parts = parts;
        }
    }
}

/// Length (mm) of the room separator `sep` that lies along the sides of the room rectangle `s`.
fn along_edges(sep: &crate::typed::RoomSeparator, s: &crate::typed::Space) -> f64 {
    let (x0, y0, x1, y1) = (s.x, s.y, s.x + s.w, s.y + s.h);
    let overlap = |a: i64, b: i64, lo: i64, hi: i64| (a.max(b).min(hi) - a.min(b).max(lo)).max(0) as f64;
    if sep.x1 == sep.x2 && (sep.x1 == x0 || sep.x1 == x1) {
        overlap(sep.y1, sep.y2, y0, y1)
    } else if sep.y1 == sep.y2 && (sep.y1 == y0 || sep.y1 == y1) {
        overlap(sep.x1, sep.x2, x0, x1)
    } else {
        0.0
    }
}

pub fn compute(model: &Model, opts: &QtoOpts) -> Qto {
    let mut acc = Acc { items: Vec::new(), rows: Vec::new(), opts };
    let mut warnings = Vec::new();

    let walls = collect::<Wall>(model, opts);
    let doors = collect::<Door>(model, opts);
    let windows = collect::<Window>(model, opts);
    // Physical openings remain in their host wall even if a phase was set only
    // on the opening or the host. The phase controls which item is billed, not
    // the net geometry of a wall/room.
    let all_doors = collect::<Door>(model, &QtoOpts::default());
    let all_windows = collect::<Window>(model, &QtoOpts::default());
    let slabs = collect::<Slab>(model, opts);
    let all_slabs = collect::<Slab>(model, &QtoOpts::default());
    let beams = collect::<Beam>(model, opts);
    let columns = collect::<Column>(model, opts);
    let footings = collect::<Footing>(model, opts);
    let spaces = collect::<Space>(model, opts);

    // ---- walls: area net of hosted openings, then volume ----
    let mut open_by_wall: BTreeMap<Id, f64> = BTreeMap::new();
    for (e, d) in &all_doors {
        if let Some(h) = e.host_id {
            *open_by_wall.entry(h).or_default() += d.width as f64 * d.height as f64;
        }
    }
    for (e, w) in &all_windows {
        if let Some(h) = e.host_id {
            *open_by_wall.entry(h).or_default() += w.width as f64 * w.height as f64;
        }
    }
    let (mut ext_a, mut int_a, mut ext_v, mut int_v, mut deducted) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let (mut ext_ids, mut int_ids, mut open_ids) = (Vec::new(), Vec::new(), Vec::new());
    let mut net_by_wall: Vec<(Id, f64)> = Vec::new();
    let mut wall_rows: Vec<(String, Unit, Id, f64)> = Vec::new();
    let mut open_rows: Vec<(Id, f64)> = Vec::new();
    for (e, w) in &walls {
        let open = open_by_wall.get(&e.id).copied().unwrap_or(0.0);
        let gross = w.length as f64 * w.height as f64;
        if open > gross {
            warnings.push(format!("wall {} has more opening area than wall area", e.id));
        }
        let net = (gross - open).max(0.0);
        deducted += open.min(gross);
        net_by_wall.push((e.id, net / MM2_PER_M2));
        if open > 0.0 {
            open_ids.push(e.id);
            open_rows.push((e.id, open.min(gross) / MM2_PER_M2));
        }
        let side = if w.is_exterior { "EXT" } else { "INT" };
        wall_rows.push((format!("WALL.{side}.AREA"), Unit::M2, e.id, net / MM2_PER_M2));
        wall_rows.push((format!("WALL.{side}.VOL"), Unit::M3, e.id, net * w.thickness as f64 / MM3_PER_M3));
        wall_rows.push(("WALL.FACES.AREA".to_string(), Unit::M2, e.id, 2.0 * net / MM2_PER_M2));
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
    // Walls marked TO_BE_DEMOLISHED: the whole wall comes down, openings included, so the gross face area and the volume are measured.
    // They are in the wall items above as well (every wall is); this is the work of taking them down.
    let (mut dem_a, mut dem_v, mut dem_ids) = (0.0, 0.0, Vec::new());
    let mut dem_rows: Vec<(String, Unit, Id, f64)> = Vec::new();
    for (e, w) in &walls {
        if e.props.get("phase").and_then(|v| v.as_str()) == Some("TO_BE_DEMOLISHED") {
            let gross = w.length as f64 * w.height as f64;
            dem_a += gross / MM2_PER_M2;
            dem_v += gross * w.thickness as f64 / MM3_PER_M3;
            dem_ids.push(e.id);
            dem_rows.push(("DEMOLISH.WALL.AREA".to_string(), Unit::M2, e.id, gross / MM2_PER_M2));
            dem_rows.push(("DEMOLISH.WALL.VOL".to_string(), Unit::M3, e.id, gross * w.thickness as f64 / MM3_PER_M3));
        }
    }
    for (code, unit, id, q) in wall_rows.iter().chain(&dem_rows) {
        acc.link(code, *unit, &[(*id, *q)]);
    }
    acc.link("WALL.OPENINGS.AREA", Unit::M2, &open_rows);
    acc.add("DEMOLISH.WALL.AREA", "Walls to be demolished, gross face area", Unit::M2, dem_a, dem_ids.clone());
    acc.add("DEMOLISH.WALL.VOL", "Walls to be demolished, volume", Unit::M3, dem_v, dem_ids);
    acc.add("WALL.EXT.AREA", "Exterior walls, area net of openings", Unit::M2, ext_a, ext_ids.clone());
    acc.add("WALL.INT.AREA", "Interior walls, area net of openings", Unit::M2, int_a, int_ids.clone());
    acc.add("WALL.EXT.VOL", "Exterior walls, volume net of openings", Unit::M3, ext_v, ext_ids.clone());
    acc.add("WALL.INT.VOL", "Interior walls, volume net of openings", Unit::M3, int_v, int_ids.clone());
    acc.add("WALL.OPENINGS.AREA", "Door and window openings deducted from walls", Unit::M2, deducted / MM2_PER_M2, open_ids);
    // One item per layer of every wall assembly in use: the layer's area is the net area of the walls that use the assembly.
    let resolver = crate::assembly::Resolver::of(model);
    for asm in &resolver.list {
        let users: Vec<(Id, f64)> = walls.iter().filter(|(e, w)| resolver.of_wall(e, w).is_some_and(|a| a.id == asm.id)).filter_map(|(e, _)| net_by_wall.iter().find(|(id, _)| *id == e.id).copied()).collect();
        if users.is_empty() {
            continue;
        }
        let area: f64 = users.iter().map(|(_, a)| a).sum();
        let ids: Vec<Id> = users.iter().map(|(id, _)| *id).collect();
        for (i, l) in asm.layers.iter().enumerate() {
            let mat: String = l.material.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
            let t = l.thickness_mm.unwrap_or(0.0);
            let code = format!("WALL.LAYER.{}.{}.{}", asm.id, i + 1, mat);
            acc.add(&code, &format!("Wall layer {}: {} {} mm ({})", i + 1, l.label.clone().unwrap_or_else(|| l.material.clone()), t, asm.name), Unit::M2, area, ids.clone());
            acc.link(&code, Unit::M2, &users);
        }
    }
    let faces: Vec<Id> = ext_ids.iter().chain(&int_ids).copied().collect();
    acc.add("WALL.FACES.AREA", "Wall faces to finish (both sides, openings deducted)", Unit::M2, 2.0 * (ext_a + int_a), faces);

    // ---- openings ----
    // A door into a lift shaft is a landing opening, not an interior door: it is counted apart and left out of the door items, so a
    // lift landing is never priced as a door leaf. The car, the guides, the machine and the landing doors are the lift supplier's.
    let lift_keys: std::collections::BTreeSet<&str> = spaces.iter().filter(|(_, s)| s.func == "lift").map(|(_, s)| s.requirement_key.as_str()).collect();
    let (lift_doors, plain_doors): (Vec<_>, Vec<_>) = doors.iter().partition(|(_, d)| lift_keys.contains(d.from.as_str()) || lift_keys.contains(d.to.as_str()));
    let mut door_types: BTreeMap<(i64, i64), Vec<Id>> = BTreeMap::new();
    for (e, d) in plain_doors {
        door_types.entry((d.width, d.height)).or_default().push(e.id);
    }
    if !lift_keys.is_empty() {
        let shafts: Vec<(Id, f64)> = spaces.iter().filter(|(_, s)| s.func == "lift").map(|(e, s)| (e.id, s.area)).collect();
        acc.add("LIFT.SHAFT", "Lift shafts, one per level served (car, guides, machine and landing doors are not quantified here)", Unit::Nr, shafts.len() as f64, shafts.iter().map(|(i, _)| *i).collect());
        acc.add("LIFT.SHAFT.AREA", "Lift shaft plan area, all levels", Unit::M2, shafts.iter().map(|(_, a)| a).sum(), shafts.iter().map(|(i, _)| *i).collect());
        acc.link("LIFT.SHAFT", Unit::Nr, &shafts.iter().map(|(i, _)| (*i, 1.0)).collect::<Vec<_>>());
        acc.link("LIFT.SHAFT.AREA", Unit::M2, &shafts);
        if !lift_doors.is_empty() {
            acc.add("LIFT.LANDING", "Lift landing openings (leaf supplied with the lift)", Unit::Nr, lift_doors.len() as f64, lift_doors.iter().map(|(e, _)| e.id).collect());
            acc.link("LIFT.LANDING", Unit::Nr, &lift_doors.iter().map(|(e, _)| (e.id, 1.0)).collect::<Vec<_>>());
        }
    }
    for ((w, h), ids) in door_types {
        let code = format!("DOOR.{w}x{h}");
        acc.add(&code, &format!("Door {w} x {h} mm"), Unit::Nr, ids.len() as f64, ids.clone());
        acc.link(&code, Unit::Nr, &ids.iter().map(|i| (*i, 1.0)).collect::<Vec<_>>());
    }
    let mut win_types: BTreeMap<(i64, i64), Vec<Id>> = BTreeMap::new();
    let mut glazing = 0.0;
    for (e, w) in &windows {
        win_types.entry((w.width, w.height)).or_default().push(e.id);
        glazing += w.width as f64 * w.height as f64 / MM2_PER_M2;
    }
    let win_ids: Vec<Id> = windows.iter().map(|(e, _)| e.id).collect();
    for ((w, h), ids) in win_types {
        let code = format!("WINDOW.{w}x{h}");
        acc.add(&code, &format!("Window {w} x {h} mm"), Unit::Nr, ids.len() as f64, ids.clone());
        acc.link(&code, Unit::Nr, &ids.iter().map(|i| (*i, 1.0)).collect::<Vec<_>>());
    }
    acc.add("WINDOW.AREA", "Total glazed opening area", Unit::M2, glazing, win_ids);
    acc.link("WINDOW.AREA", Unit::M2, &windows.iter().map(|(e, w)| (e.id, w.width as f64 * w.height as f64 / MM2_PER_M2)).collect::<Vec<_>>());

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
        let slab_t = all_slabs
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
    let storey = collect::<Wall>(model, &QtoOpts::default()).iter().map(|(_, w)| w.height).max();
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
        acc.link("FOOTING.AREA", Unit::M2, &footings.iter().map(|(e, f)| (e.id, f.length as f64 * f.width as f64 / MM2_PER_M2)).collect::<Vec<_>>());
        acc.link("FOOTING.COUNT", Unit::Nr, &footings.iter().map(|(e, _)| (e.id, 1.0)).collect::<Vec<_>>());
        warnings.push("footing depth is not designed: footing concrete, excavation and reinforcement are not quantified".into());
    }
    // ---- tie beams between footings: concrete from their own size ----
    let ties: Vec<(Id, f64)> = model
        .elements
        .values()
        .filter(|e| e.kind == crate::typed::TieBeam::KIND && (opts.level.is_none() || e.level_id == opts.level))
        .filter(|e| matches_phase(e, opts.phase.as_deref()))
        .filter_map(|e| {
            let t = e.typed::<crate::typed::TieBeam>().ok()?;
            let len = ((t.x2 - t.x1).abs() + (t.y2 - t.y1).abs()) as f64;
            Some((e.id, t.width as f64 * t.depth as f64 * len / MM3_PER_M3))
        })
        .collect();
    if !ties.is_empty() {
        acc.add_parts("TIEBEAM.COUNT", "Tie beams", Unit::Nr, ties.iter().map(|(id, _)| (*id, 1.0)).collect());
        acc.add_parts("TIEBEAM.VOL", "Tie beam concrete", Unit::M3, ties);
    }
    if !slabs.is_empty() || !beams.is_empty() || !columns.is_empty() {
        warnings.push("reinforcement is not quantified: it needs a structural design".into());
    }

    // ---- a raft and hourdi slabs the owner declares (see `substruct`): the whole building, not one level or one phase ----
    if let Some((project, declared)) = crate::project::find(model).and_then(|e| Some((e.id, e.typed::<crate::project::NanoArchProject>().ok()?.meta.declared))).filter(|(_, d)| !d.is_empty()) {
        if opts.level.is_some() || opts.phase.is_some() {
            warnings.push("the declared raft and hourdi slabs are quantified for the whole building only, not for one level or phase".into());
        } else {
            let rep = crate::substruct::compute(model, &declared);
            let src = || vec![project];
            let first = acc.items.len();
            if let Some(r) = &rep.raft {
                for (code, text, unit, q) in [("RAFT.AREA", "Raft foundation, plan area", Unit::M2, r.area_m2), ("RAFT.CONC", "Raft concrete", Unit::M3, r.concrete_m3), ("RAFT.FORM", "Raft side formwork", Unit::M2, r.side_formwork_m2), ("RAFT.EXCAV", "Excavation under the raft", Unit::M3, r.excavation_m3)] {
                    if let Some(q) = q.filter(|q| *q > 0.0) {
                        acc.add(code, text, unit, q, src());
                    }
                }
            }
            if !rep.slabs.is_empty() && rep.slabs.iter().all(|s| !s.refused) {
                acc.add("SLAB.FORM", "Hourdi slab soffit formwork (whole floor footprints)", Unit::M2, rep.slabs.iter().map(|s| s.area_m2).sum(), src());
                if let Some(v) = rep.slabs.iter().map(|s| s.concrete_m3).collect::<Option<Vec<f64>>>() {
                    acc.add("SLAB.VOL", "Hourdi slab concrete (ribs and topping)", Unit::M3, v.iter().sum(), src());
                }
                if let Some(v) = rep.slabs.iter().map(|s| s.blocks).collect::<Option<Vec<u64>>>() {
                    acc.add("SLAB.BLOCKS", "Hourdi blocks", Unit::Nr, v.iter().sum::<u64>() as f64, src());
                }
            }
            let label = declared.label();
            for item in &mut acc.items[first..] {
                item.description = format!("{} {label}", item.description);
            }
            warnings.extend(rep.missing.iter().map(|m| format!("declared structure: the {m} is not given, so what depends on it is not quantified")));
            warnings.extend(rep.notes);
        }
    }

    // ---- finishes by room function ----
    let mut floor: BTreeMap<String, (f64, Vec<Id>)> = BTreeMap::new();
    let mut skirting: BTreeMap<String, (f64, Vec<Id>)> = BTreeMap::new();
    // A room separator is a line on the drawing, not a wall: no skirting runs along it.
    // ---- features the drawing names: counted by role, with length (lines) and area (rectangles) ----
    {
        let mut by_role: BTreeMap<String, Vec<(Id, f64, f64, u32)>> = BTreeMap::new();
        for (e, f) in collect::<Feature>(model, opts) {
            let (len, area) = f.size();
            by_role.entry(f.role.clone()).or_default().push((e.id, len, area, f.risers.unwrap_or(0)));
        }
        for (role, v) in by_role {
            let ids: Vec<Id> = v.iter().map(|x| x.0).collect();
            acc.add(&format!("FEATURE.{role}.COUNT"), &format!("{role}: number of features"), Unit::Nr, v.len() as f64, ids.clone());
            acc.link(&format!("FEATURE.{role}.COUNT"), Unit::Nr, &v.iter().map(|x| (x.0, 1.0)).collect::<Vec<_>>());
            let len: f64 = v.iter().map(|x| x.1).sum();
            if len > 0.0 {
                acc.add(&format!("FEATURE.{role}.LEN"), &format!("{role}: total length"), Unit::M, len / 1000.0, ids.clone());
                acc.link(&format!("FEATURE.{role}.LEN"), Unit::M, &v.iter().map(|x| (x.0, x.1 / 1000.0)).collect::<Vec<_>>());
            }
            let area: f64 = v.iter().map(|x| x.2).sum();
            if area > 0.0 {
                acc.add(&format!("FEATURE.{role}.AREA"), &format!("{role}: total area"), Unit::M2, area / MM2_PER_M2, ids.clone());
                acc.link(&format!("FEATURE.{role}.AREA"), Unit::M2, &v.iter().map(|x| (x.0, x.2 / MM2_PER_M2)).collect::<Vec<_>>());
            }
            // Steps the drawing states: the riser count only. Treads, concrete and finishes need a tread depth the drawing has not given.
            let risers: u32 = v.iter().map(|x| x.3).sum();
            if risers > 0 {
                acc.add(&format!("FEATURE.{role}.RISERS"), &format!("{role}: number of risers"), Unit::Nr, risers as f64, ids);
                acc.link(&format!("FEATURE.{role}.RISERS"), Unit::Nr, &v.iter().map(|x| (x.0, x.3 as f64)).collect::<Vec<_>>());
            }
        }
    }
    let separators = collect::<crate::typed::RoomSeparator>(model, &QtoOpts::default());
    let mut open_edge_mm = 0.0;
    let mut space_rows: Vec<(String, Unit, Id, f64)> = Vec::new();
    for (e, s) in &spaces {
        let area = s.net_area.unwrap_or(s.area);
        let f = floor.entry(s.func.clone()).or_default();
        f.0 += area;
        f.1.push(e.id);
        space_rows.push((format!("FLOOR.AREA.{}", s.func), Unit::M2, e.id, area));

        // A courtyard has paving but no walls of its own and no ceiling: no skirting, no ceiling, not part of the floor area.
        if s.func == "courtyard" {
            continue;
        }
        space_rows.push((format!("CEILING.AREA.{}", s.func), Unit::M2, e.id, area));
        let door_widths: f64 = all_doors.iter().filter(|(_, d)| d.from == s.requirement_key || d.to == s.requirement_key).map(|(_, d)| d.width as f64).sum();
        let open: f64 = separators.iter().map(|(_, sep)| along_edges(sep, s)).sum();
        open_edge_mm += open;
        let perimeter = 2.0 * (s.w + s.h) as f64 - open;
        let k = skirting.entry(s.func.clone()).or_default();
        let run = ((perimeter - door_widths) / 1000.0).max(0.0);
        k.0 += run;
        k.1.push(e.id);
        space_rows.push((format!("SKIRTING.{}", s.func), Unit::M, e.id, run));
    }
    for (func, (area, ids)) in floor {
        acc.add(&format!("FLOOR.AREA.{func}"), &format!("Floor finish, {func} (net area)"), Unit::M2, area, ids.clone());
        if func != "courtyard" {
            acc.add(&format!("CEILING.AREA.{func}"), &format!("Ceiling, {func} (net area)"), Unit::M2, area, ids);
        }
    }
    if open_edge_mm > 0.0 {
        warnings.push(format!("skirting leaves out {:.2} m of room edge that lies along room separators (no wall there)", open_edge_mm / 1000.0));
    }
    for (func, (len, ids)) in skirting {
        acc.add(&format!("SKIRTING.{func}"), &format!("Skirting, {func} (perimeter minus doors)"), Unit::M, len, ids);
    }
    let covered = |s: &Space| s.func != "courtyard";
    let all_spaces: Vec<Id> = spaces.iter().filter(|(_, s)| covered(s)).map(|(e, _)| e.id).collect();
    acc.add("SPACE.GROSS.AREA", "Total gross floor area (roofed spaces)", Unit::M2, spaces.iter().filter(|(_, s)| covered(s)).map(|(_, s)| s.area).sum(), all_spaces);
    for (code, unit, id, q) in &space_rows {
        acc.link(code, *unit, &[(*id, *q)]);
    }
    acc.link("SPACE.GROSS.AREA", Unit::M2, &spaces.iter().filter(|(_, s)| covered(s)).map(|(e, s)| (e.id, s.area)).collect::<Vec<_>>());
    let courts: Vec<(Id, f64)> = spaces.iter().filter(|(_, s)| !covered(s)).map(|(e, s)| (e.id, s.area)).collect();
    if !courts.is_empty() {
        acc.add("COURT.AREA", "Courtyard (open to the sky)", Unit::M2, courts.iter().map(|c| c.1).sum(), courts.iter().map(|c| c.0).collect());
        acc.link("COURT.AREA", Unit::M2, &courts);
    }

    Qto { items: acc.items, warnings, element_rows: acc.rows }
}
