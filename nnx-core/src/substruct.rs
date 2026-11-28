//! The structural systems the owner declares for a building that has no structural design yet: a raft foundation and hourdi
//! (one-way ribbed, with blocks) slabs. From the drawn walls and rooms it measures the concrete, the formwork, the excavation under
//! the raft and the number of blocks. It designs nothing: no reinforcement, no thickness check, no beams or solid parts.
//!
//! Every dimension is an input with no default. A quantity whose inputs are not all given is not computed, and the inputs it lacks
//! are named.
//!
//! The slab over a level roofs it and floors the one above, so it covers both footprints. The raft covers the ground footprint
//! only, and the area the floor above has outside it is reported.
//!
//! The footprint of a level is the union of its walls and rooms with everything they enclose. A wall line left open to the outside
//! with no room behind it lets that area out of the footprint, so the footprint area is reported for the reader to compare.

use crate::building;
use crate::model::{Id, Model};
use crate::typed::{Feature, Space, Typed, Wall};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RaftIn {
    /// Level of the top of the raft above the ground surface, mm (negative when it is below it).
    pub top_above_ground_mm: Option<f64>,
    pub thickness_mm: Option<f64>,
    /// How far the raft reaches beyond the outer face of the ground-floor walls, mm.
    pub projection_mm: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HourdiIn {
    /// Whole depth of the slab, topping included.
    pub total_mm: Option<f64>,
    pub topping_mm: Option<f64>,
    pub rib_width_mm: Option<f64>,
    /// Centre to centre of the ribs.
    pub rib_spacing_mm: Option<f64>,
    /// Length of one block along the rib.
    pub block_length_mm: Option<f64>,
}

/// The founding soil as the soil report classes it, in the terms of SBC 303 5.2.0.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Soil {
    Cohesionless,
    SiltyClay,
    Rock,
}

/// What the two foundation-depth clauses of SBC 303 need beyond the raft itself.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FoundingIn {
    pub soil: Option<Soil>,
    /// Depth of the neighbouring existing footing's underside below the natural ground, mm.
    pub existing_depth_mm: Option<f64>,
    /// Horizontal distance between the lower edges of the raft and of that footing, mm.
    pub existing_gap_mm: Option<f64>,
}

/// What the owner declares. A system that is not declared (`None`) adds nothing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Declared {
    pub raft: Option<RaftIn>,
    pub hourdi: Option<HourdiIn>,
    /// Area of the first floor that rests on the roof of a building already standing, m2: no new slab is cast under it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub existing_roof_m2: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub founding: Option<FoundingIn>,
    /// Openings of each slab that the drawing does not show as an element (a stair well), m2, by the level the slab roofs.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub openings_m2: std::collections::BTreeMap<String, f64>,
    /// Where the declared dimensions come from, in the owner's words.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// The dimensions are for a first estimate only, not an approved design.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub estimate: bool,
}

/// Starts the label of a quantity that rests on values declared as an estimate.
pub const ESTIMATE_MARK: &str = "[ESTIMATE";

impl Declared {
    /// What every quantity taken from the declared values says about them, in the exports too.
    pub fn label(&self) -> String {
        match (self.estimate, self.source.trim()) {
            (true, "") => format!("{ESTIMATE_MARK}, not for ordering or construction; source not stated]"),
            (true, s) => format!("{ESTIMATE_MARK}, not for ordering or construction; source: {s}]"),
            (false, "") => "[source of the declared values not stated]".into(),
            (false, s) => format!("[source: {s}]"),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.raft.is_none() && self.hourdi.is_none() && self.existing_roof_m2.is_none() && self.founding.is_none()
    }

    pub fn check(&self) -> Result<(), String> {
        let pos = |name: &str, v: Option<f64>| match v {
            Some(x) if !x.is_finite() || x <= 0.0 => Err(format!("{name} must be a positive number")),
            _ => Ok(()),
        };
        pos("the area on the existing roof", self.existing_roof_m2)?;
        for v in self.openings_m2.values() {
            pos("the slab opening area", Some(*v))?;
        }
        if let Some(f) = &self.founding {
            pos("the depth of the existing footing", f.existing_depth_mm)?;
            if f.existing_gap_mm.is_some_and(|x| !x.is_finite() || x < 0.0) {
                return Err("the distance to the existing footing must be zero or more".into());
            }
        }
        if let Some(r) = &self.raft {
            if r.top_above_ground_mm.is_some_and(|x| !x.is_finite()) {
                return Err("the raft top level must be a number".into());
            }
            pos("the raft thickness", r.thickness_mm)?;
            if r.projection_mm.is_some_and(|x| !x.is_finite() || x < 0.0) {
                return Err("the raft projection must be zero or more".into());
            }
        }
        if let Some(h) = &self.hourdi {
            pos("the slab depth", h.total_mm)?;
            pos("the slab topping", h.topping_mm)?;
            pos("the rib width", h.rib_width_mm)?;
            pos("the rib spacing", h.rib_spacing_mm)?;
            pos("the block length", h.block_length_mm)?;
            if let (Some(t), Some(d)) = (h.topping_mm, h.total_mm)
                && t >= d
            {
                return Err("the slab topping must be less than the slab depth".into());
            }
            if let (Some(w), Some(s)) = (h.rib_width_mm, h.rib_spacing_mm)
                && w >= s
            {
                return Err("the rib width must be less than the rib spacing".into());
            }
        }
        Ok(())
    }
}

/// `x0, y0, x1, y1`, millimetres.
type Rect = [i64; 4];

/// The union of rectangles as filled cells of the grid their edges make.
struct Cells {
    xs: Vec<i64>,
    ys: Vec<i64>,
    filled: Vec<bool>,
}

impl Cells {
    fn of(rects: &[Rect]) -> Cells {
        let axis = |a: usize, b: usize| {
            let mut v: Vec<i64> = rects.iter().flat_map(|r| [r[a], r[b]]).collect();
            v.sort_unstable();
            v.dedup();
            // One empty cell all round, so the outside is connected.
            if let (Some(&lo), Some(&hi)) = (v.first(), v.last()) {
                v.insert(0, lo - 1);
                v.push(hi + 1);
            }
            v
        };
        let (xs, ys) = (axis(0, 2), axis(1, 3));
        let (w, h) = (xs.len().saturating_sub(1), ys.len().saturating_sub(1));
        let mut filled = vec![false; w * h];
        for r in rects {
            for j in 0..h {
                if ys[j] < r[1] || ys[j + 1] > r[3] {
                    continue;
                }
                for i in 0..w {
                    if xs[i] >= r[0] && xs[i + 1] <= r[2] {
                        filled[j * w + i] = true;
                    }
                }
            }
        }
        Cells { xs, ys, filled }
    }

    fn size(&self) -> (usize, usize) {
        (self.xs.len().saturating_sub(1), self.ys.len().saturating_sub(1))
    }

    /// Fills every empty cell that cannot be reached from the outside.
    fn fill_enclosed(&mut self) {
        let (w, h) = self.size();
        if w == 0 || h == 0 {
            return;
        }
        let mut outside = vec![false; w * h];
        let mut stack = vec![0usize];
        outside[0] = true;
        while let Some(c) = stack.pop() {
            let (i, j) = (c % w, c / w);
            let near = [(i > 0).then(|| c - 1), (i + 1 < w).then(|| c + 1), (j > 0).then(|| c - w), (j + 1 < h).then(|| c + w)];
            for n in near.into_iter().flatten() {
                if !outside[n] && !self.filled[n] {
                    outside[n] = true;
                    stack.push(n);
                }
            }
        }
        for (f, o) in self.filled.iter_mut().zip(outside) {
            *f = !o;
        }
    }

    fn rects(&self) -> Vec<Rect> {
        let (w, _) = self.size();
        self.filled.iter().enumerate().filter(|(_, f)| **f).map(|(c, _)| [self.xs[c % w], self.ys[c / w], self.xs[c % w + 1], self.ys[c / w + 1]]).collect()
    }

    fn area_m2(&self) -> f64 {
        self.rects().iter().map(|r| (r[2] - r[0]) as f64 * (r[3] - r[1]) as f64).sum::<f64>() / 1e6
    }

    /// Length of the boundary between filled and empty cells (holes included).
    fn perimeter_m(&self) -> f64 {
        let (w, h) = self.size();
        let at = |i: usize, j: usize| self.filled[j * w + i];
        let mut mm = 0i64;
        for j in 0..h {
            for i in 0..w {
                if !at(i, j) {
                    continue;
                }
                let (dx, dy) = (self.xs[i + 1] - self.xs[i], self.ys[j + 1] - self.ys[j]);
                // The border cells are always empty, so every filled cell has four neighbours.
                mm += [(!at(i - 1, j), dy), (!at(i + 1, j), dy), (!at(i, j - 1), dx), (!at(i, j + 1), dx)].iter().filter(|(open, _)| *open).map(|(_, l)| l).sum::<i64>();
            }
        }
        mm as f64 / 1000.0
    }
}

/// The rectangle of each wall, run on to the outer face of a wall it meets at an end, so corners are whole.
fn wall_rects(walls: &[Wall]) -> Vec<Rect> {
    walls
        .iter()
        .map(|w| {
            let (mut lo, mut hi) = (w.start(), w.start() + w.length);
            let (c, half) = (w.centre_line(), w.thickness / 2);
            for o in walls.iter().filter(|o| o.horizontal() != w.horizontal()) {
                let (oc, oh) = (o.centre_line(), o.thickness / 2);
                if c < o.start() - half || c > o.start() + o.length + half {
                    continue;
                }
                if (lo - oc).abs() <= oh {
                    lo = lo.min(oc - oh);
                }
                if (hi - oc).abs() <= oh {
                    hi = hi.max(oc + oh);
                }
            }
            if w.horizontal() { [lo, c - half, hi, c + w.thickness - half] } else { [c - half, lo, c + w.thickness - half, hi] }
        })
        .collect()
}

/// The drawn lightwells, shafts and lifts of one level, as far as they lie inside the slab, m2 (overlaps counted once).
fn drawn_openings(model: &Model, level: Option<Id>, slab: &Cells) -> f64 {
    let features = model.elements.values().filter(|e| e.kind == Feature::KIND && e.level_id == level).filter_map(|e| e.typed::<Feature>().ok());
    let inside: Vec<Rect> = features
        .filter(|f| f.shape == "rect" && matches!(f.role.as_str(), "LIGHTWELL" | "SHAFT" | "LIFT"))
        .flat_map(|f| {
            let r = [f.x1.min(f.x2), f.y1.min(f.y2), f.x1.max(f.x2), f.y1.max(f.y2)];
            slab.rects().into_iter().map(move |c| [r[0].max(c[0]), r[1].max(c[1]), r[2].min(c[2]), r[3].min(c[3])]).filter(|c| c[0] < c[2] && c[1] < c[3])
        })
        .collect();
    Cells::of(&inside).area_m2()
}

/// The footprint of one level.
fn footprint(model: &Model, level: Option<Id>) -> Cells {
    let walls: Vec<Wall> = model.elements.values().filter(|e| e.kind == Wall::KIND && e.level_id == level).filter_map(|e| e.typed::<Wall>().ok()).collect();
    let mut rects = wall_rects(&walls);
    rects.extend(model.elements.values().filter(|e| e.kind == Space::KIND && e.level_id == level).filter_map(|e| e.typed::<Space>().ok()).map(|s| [s.x, s.y, s.x + s.w, s.y + s.h]));
    let mut cells = Cells::of(&rects);
    cells.fill_enclosed();
    cells
}

/// Everything either footprint covers.
fn union(a: &Cells, b: &Cells) -> Cells {
    let mut rects = a.rects();
    rects.extend(b.rects());
    let mut cells = Cells::of(&rects);
    cells.fill_enclosed();
    cells
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct RaftOut {
    pub footprint_m2: f64,
    pub area_m2: Option<f64>,
    pub perimeter_m: Option<f64>,
    pub concrete_m3: Option<f64>,
    pub side_formwork_m2: Option<f64>,
    /// Depth of the raft's underside below the ground surface (0 when the raft stands wholly above it).
    pub depth_below_ground_mm: Option<f64>,
    pub excavation_m3: Option<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SlabOut {
    /// 0 is the ground floor: the slab that roofs it.
    pub level: i64,
    /// Net of the openings.
    pub area_m2: f64,
    pub openings_m2: f64,
    /// Its openings are not smaller than the slab: nothing is quantified from it, and no total is given for the slabs.
    pub refused: bool,
    pub concrete_m3: Option<f64>,
    pub blocks: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub raft: Option<RaftOut>,
    pub slabs: Vec<SlabOut>,
    /// What the first floor covers outside the ground footprint, m2 (None for a building of one floor).
    pub outside_ground_m2: Option<f64>,
    /// Inputs a quantity lacks, by name.
    pub missing: Vec<String>,
    pub notes: Vec<String>,
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// SBC 303 (2007) 5.2.0 and 5.2.1 (chapter 5, PDF page 43): the minimum depth below the natural ground by soil, and on
/// granular soil a line between the lower edges of neighbouring foundations no steeper than 30 degrees. Nothing passes
/// without all of its inputs. The ground the raft level is measured from is taken as the natural ground.
fn founding_depth(r: &RaftIn, f: Option<&FoundingIn>, notes: &mut Vec<String>) {
    let soil = f.and_then(|f| f.soil);
    // Signed: negative when the underside is above the ground.
    let depth = r.thickness_mm.zip(r.top_above_ground_mm).map(|(t, top)| (t - top) / 1000.0);
    // (the least depth the clause allows at all, the depth from which it is met without judgement), metres.
    let limits = soil.map(|s| match s {
        Soil::Cohesionless => (1.2, 1.2),
        Soil::SiltyClay => (1.5, 1.5),
        Soil::Rock => (0.6, 1.2),
    });
    notes.push(match (limits, depth, r.top_above_ground_mm) {
        (None, ..) => "SBC 303 (2007) 5.2.0 not checked: choose the founding soil from the soil report".into(),
        (Some((least, _)), None, Some(top)) => format!("SBC 303 (2007) 5.2.0 not checked: the raft thickness is not entered; to put its underside {least:.2} m below ground at this top level it must be at least {:.2} m thick", (least + top / 1000.0).max(0.0)),
        (Some(_), None, None) | (Some(_), Some(_), None) => "SBC 303 (2007) 5.2.0 not checked: the raft top level and thickness are needed".into(),
        (Some((_, met)), Some(d), Some(_)) if d >= met - 1e-9 => format!("SBC 303 (2007) 5.2.0 met: the raft underside is {d:.2} m below ground, not less than the {met:.2} m minimum for the declared soil"),
        (Some((least, _)), Some(d), Some(_)) if d >= least - 1e-9 => format!("SBC 303 (2007) 5.2.0 needs your engineer: on rock the minimum is 0.60 to 1.20 m by the rock's strength and integrity, and the raft underside is {d:.2} m below ground"),
        (Some((least, _)), Some(d), Some(top)) => {
            let thick = (least + top / 1000.0).max(0.0);
            if d < 0.0 {
                format!("SBC 303 (2007) 5.2.0 not met: the raft underside is {:.2} m above ground, and the minimum for the declared soil is {least:.2} m below it: at this top level the raft would need to be {thick:.2} m thick, or to be set lower", -d)
            } else {
                format!("SBC 303 (2007) 5.2.0 not met: the raft underside is {d:.2} m below ground, less than the {least:.2} m minimum for the declared soil: at this top level the raft would need to be {thick:.2} m thick, or to be set lower")
            }
        }
    });
    let existing = f.and_then(|f| f.existing_depth_mm.zip(f.existing_gap_mm));
    notes.push(match (soil, existing, depth) {
        (Some(Soil::SiltyClay | Soil::Rock), ..) => "SBC 303 (2007) 5.2.1 not applied: it covers footings on granular soil, and the declared soil is not cohesionless; your engineer must still check the neighbouring foundations".into(),
        (Some(Soil::Cohesionless), Some((e, gap)), Some(d)) => {
            let (rise, run) = ((e / 1000.0 - d).abs(), gap / 1000.0);
            let angle = rise.atan2(run).to_degrees();
            if angle <= 30.0 + 1e-9 {
                format!("SBC 303 (2007) 5.2.1 met: the line between the two lower edges slopes {angle:.1} degrees, not more than 30")
            } else {
                format!("SBC 303 (2007) 5.2.1 not met: the line between the two lower edges slopes {angle:.1} degrees, more than 30: it needs lateral support or an engineering analysis accepted by the building official, or a distance of at least {:.2} m", rise / 30f64.to_radians().tan())
            }
        }
        _ => "SBC 303 (2007) 5.2.1 not checked: it needs the founding soil, the depth of the existing footing underside, the horizontal distance between the two lower edges, and the raft thickness and top level".into(),
    });
    notes.push("SBC 303 (2007) clauses 5.2.0 and 5.2.1 are in the spread footings chapter: your engineer confirms how they apply to a raft; depths are measured from the natural ground level".into());
}

pub fn compute(model: &Model, d: &Declared) -> Report {
    let mut rep = Report::default();
    if d.is_empty() {
        return rep;
    }
    let levels = building::levels(model);
    let ids: Vec<(i64, Option<Id>)> = if levels.is_empty() { vec![(0, None)] } else { levels.iter().map(|(i, id)| (*i, Some(*id))).collect() };
    let ground = footprint(model, ids[0].1);
    rep.outside_ground_m2 = ids.get(1).map(|(_, above)| r2(union(&ground, &footprint(model, *above)).area_m2() - ground.area_m2()));
    // An area on an existing roof must lie within what the floor above covers outside the new ground floor.
    let on_roof = d.existing_roof_m2.filter(|a| *a <= rep.outside_ground_m2.unwrap_or(0.0) + 0.01);
    if d.raft.is_some() || d.hourdi.is_some() {
        if d.estimate {
            rep.notes.push("the declared structural values are an estimate: nothing is to be ordered or built from their quantities before your engineer approves the design".into());
        }
        rep.notes.push(match d.source.trim() {
            "" => "the source of the declared structural values is not stated".into(),
            s => format!("source of the declared structural values: {s}"),
        });
    }
    if let Some(a) = d.existing_roof_m2 {
        if on_roof.is_some() {
            rep.notes.push(format!("{a:.2} m2 of the floor above rests on the roof of an existing building: no new slab or raft is counted under it, and a structural engineer must assess that building's foundations, columns and roof, and the joint between the two, before construction"));
        } else {
            rep.notes.push(format!("the area declared on the existing roof ({a:.2} m2) is more than the floor above covers outside the ground footprint ({:.2} m2): the slabs are not quantified until it is corrected", rep.outside_ground_m2.unwrap_or(0.0)));
        }
    }
    if d.raft.is_some() && model.elements.values().any(|e| e.kind == crate::typed::Footing::KIND) {
        rep.notes.push("the model has its own footings: their quantities are used, and the declared raft is not added to them".into());
    } else if let Some(r) = &d.raft {
        let cells = &ground;
        let mut out = RaftOut { footprint_m2: r2(cells.area_m2()), ..Default::default() };
        if out.footprint_m2 <= 0.0 {
            rep.notes.push("the ground floor has no walls or rooms: there is no footprint for the raft".into());
        } else {
            for (name, v) in [("raft projection", r.projection_mm), ("raft thickness", r.thickness_mm), ("raft top level above ground", r.top_above_ground_mm)] {
                if v.is_none() {
                    rep.missing.push(name.to_string());
                }
            }
            if let Some(p) = r.projection_mm {
                let p = p.round() as i64;
                let mut grown = Cells::of(&cells.rects().iter().map(|c| [c[0] - p, c[1] - p, c[2] + p, c[3] + p]).collect::<Vec<_>>());
                grown.fill_enclosed();
                let (area, perimeter) = (grown.area_m2(), grown.perimeter_m());
                out.area_m2 = Some(r2(area));
                out.perimeter_m = Some(r2(perimeter));
                if let Some(t) = r.thickness_mm {
                    out.concrete_m3 = Some(r2(area * t / 1000.0));
                    out.side_formwork_m2 = Some(r2(perimeter * t / 1000.0));
                    if let Some(top) = r.top_above_ground_mm {
                        let depth = (t - top).max(0.0);
                        out.depth_below_ground_mm = Some(depth);
                        out.excavation_m3 = Some(r2(area * depth / 1000.0));
                    }
                }
            }
            if let Some(all) = rep.outside_ground_m2 {
                let outside = all - on_roof.unwrap_or(0.0);
                if outside >= 0.01 {
                    rep.notes.push(format!("the floor above covers {outside:.2} m2 outside the ground footprint: the raft does not reach under it (check the ground floor drawing)"));
                }
            }
            rep.notes.push("the excavation is the raft's own plan area down to its underside: working space, blinding and soil replacement are not in it".into());
        }
        rep.raft = Some(out);
        founding_depth(r, d.founding.as_ref(), &mut rep.notes);
    }
    if let Some(h) = &d.hourdi {
        if model.elements.values().any(|e| e.kind == crate::typed::Slab::KIND) {
            rep.notes.push("the model has its own slabs: their quantities are used, and the declared hourdi slabs are not added to them".into());
        } else if d.existing_roof_m2.is_none() || on_roof.is_some() {
            for (name, v) in [("slab depth", h.total_mm), ("slab topping", h.topping_mm), ("rib width", h.rib_width_mm), ("rib spacing", h.rib_spacing_mm), ("block length", h.block_length_mm)] {
                if v.is_none() {
                    rep.missing.push(name.to_string());
                }
            }
            for (n, (level, id)) in ids.iter().enumerate() {
                let own = footprint(model, *id);
                let (slab, above) = match ids.get(n + 1) {
                    Some((_, above)) => (union(&own, &footprint(model, *above)), Some(*above)),
                    None => (footprint(model, *id), None),
                };
                let area = slab.area_m2();
                // The roof already standing serves as this much of the first floor.
                let area = if n == 0 { area - on_roof.unwrap_or(0.0) } else { area };
                if area <= 0.0 {
                    continue;
                }
                let drawn = above.map_or(0.0, |a| drawn_openings(model, a, &slab));
                let openings = drawn + d.openings_m2.get(&level.to_string()).copied().unwrap_or(0.0);
                if openings >= area {
                    rep.notes.push(format!("the opening area of the slab over level {level} ({openings:.2} m2) is not less than that slab ({area:.2} m2): the slab totals are not given until it is corrected"));
                    // Still listed, so the wrong value can be seen and changed.
                    rep.slabs.push(SlabOut { level: *level, area_m2: r2(area), openings_m2: r2(openings), refused: true, concrete_m3: None, blocks: None });
                    continue;
                }
                let area = area - openings;
                let concrete = match (h.total_mm, h.topping_mm, h.rib_width_mm, h.rib_spacing_mm) {
                    (Some(d), Some(t), Some(w), Some(s)) => Some(r2(area * (t + (d - t) * w / s) / 1000.0)),
                    _ => None,
                };
                let blocks = match (h.rib_spacing_mm, h.block_length_mm) {
                    (Some(s), Some(l)) => Some((area / (s / 1000.0 * l / 1000.0)).ceil() as u64),
                    _ => None,
                };
                rep.slabs.push(SlabOut { level: *level, area_m2: r2(area), openings_m2: r2(openings), refused: false, concrete_m3: concrete, blocks });
            }
            rep.notes.push("the hourdi quantities take each slab's area (the floor it roofs together with the floor above it) as ribs and blocks: only the lightwells, shafts and lifts drawn on the floor above and the opening areas you declare are taken out (stairs are not read from the drawing), and beams and solid parts are not added".into());
        }
    }
    rep.notes.push("reinforcement is not computed: it needs a structural design".into());
    rep
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_enclosed_hole_is_part_of_the_footprint_and_an_open_notch_is_not() {
        // Four walls of a 10 x 8 m box (outer size), drawn as rectangles: the inside is enclosed.
        let ring = [[0, 0, 10_000, 250], [0, 7_750, 10_000, 8_000], [0, 0, 250, 8_000], [9_750, 0, 10_000, 8_000]];
        let mut c = Cells::of(&ring);
        c.fill_enclosed();
        assert!((c.area_m2() - 80.0).abs() < 1e-9 && (c.perimeter_m() - 36.0).abs() < 1e-9);
        // Without one side the inside opens to the outside: only the walls remain.
        let mut open = Cells::of(&ring[..3]);
        open.fill_enclosed();
        assert!(open.area_m2() < 10.0);
    }

    #[test]
    fn wrong_values_are_refused_and_empty_inputs_are_allowed() {
        assert!(Declared::default().check().is_ok());
        let bad = Declared { hourdi: Some(HourdiIn { total_mm: Some(70.0), topping_mm: Some(70.0), ..Default::default() }), ..Default::default() };
        assert!(bad.check().unwrap_err().contains("topping"));
        let bad = Declared { raft: Some(RaftIn { thickness_mm: Some(-1.0), ..Default::default() }), ..Default::default() };
        assert!(bad.check().is_err());
    }
}
