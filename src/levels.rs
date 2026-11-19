//! Multi-storey: planning the levels from the user's brief, assembling them into one model, and keeping the
//! structure of every level in step.
//!
//! - Each room says which floor it is on (floor 1 is the ground floor). A level with no rooms of its own repeats the
//!   level below it, and says so. Room keys must be unique across floors (a repeated room gets `@<level>`).
//! - With more than one floor the user must give the stair width, length and side; nothing is defaulted. The stair
//!   block (shaft + hall) is the same on every level, and the generator fills what is left of the envelope.
//! - Only the ground level has an entrance door and footings. Upper columns pass their load to the columns below.
//! - One level is the old single-storey model, with no `Level` element, so existing projects open unchanged.

use crate::backend::{BriefIn, RoomIn, adjacency_of, envelope_and_entry, room_reqs};
use nnx_core::building::{self, Stair};
use nnx_core::layout::{self, Brief, Layout, Options, RoomReq};
use nnx_core::structural::{self, StructOpts};
use nnx_core::typed::{Column, Level, Space, Typed, element_of};
use nnx_core::walls::{self, WindowOpts};
use nnx_core::{Command, Document, Id, Model, Origin, Source};
use std::collections::BTreeMap;

pub struct LevelPlan {
    /// The brief the generator sees for this level: the envelope minus the stair strip.
    pub brief: Brief,
    pub block: Option<building::Block>,
}

pub struct BuildingPlan {
    pub levels: Vec<LevelPlan>,
    /// Every room of every level plus the stair block, over the whole envelope: what plan edits look rooms up in.
    pub merged: Brief,
    pub labels: BTreeMap<String, String>,
    pub notes: Vec<String>,
}

/// One alternative: a layout per level, ground first.
#[derive(Clone)]
pub struct Building {
    pub levels: Vec<Layout>,
}

fn level_name(i: usize) -> String {
    if i == 0 { "Ground floor".into() } else { format!("Floor {i}") }
}

pub fn plan_building(b: &BriefIn, n: u32) -> Result<BuildingPlan, String> {
    let n = n.max(1) as usize;
    let (envelope, entry) = envelope_and_entry(b)?;
    if let Some(r) = b.rooms.iter().find(|r| r.floor.unwrap_or(1) == 0 || r.floor.unwrap_or(1) as usize > n) {
        return Err(format!("room '{}' is on floor {} but the building has {n} floor(s)", r.key, r.floor.unwrap_or(1)));
    }
    let own: Vec<Vec<&RoomIn>> = (1..=n).map(|f| b.rooms.iter().filter(|r| r.floor.unwrap_or(1) as usize == f).collect()).collect();
    if own[0].is_empty() {
        return Err("the ground floor has no rooms".into());
    }
    let mut notes = Vec::new();
    let mut labels: BTreeMap<String, String> = BTreeMap::new();
    let stair = if n > 1 {
        let s = b.stair.as_ref().ok_or("the building has several floors: enter the stair width, length and side")?;
        let west = match s.side.as_str() {
            "west" => true,
            "east" => false,
            other => return Err(format!("unknown stair side '{other}' (west or east)")),
        };
        Some(Stair { width_mm: (s.width_m * 1000.0).round() as i64, length_mm: (s.length_m * 1000.0).round() as i64, west })
    } else {
        None
    };

    let mut levels = Vec::new();
    let mut merged_rooms: Vec<RoomReq> = Vec::new();
    let mut merged_adj = Vec::new();
    let mut seen_keys: BTreeMap<String, usize> = BTreeMap::new();
    let mut source: Vec<&RoomIn> = Vec::new();
    for (l, rooms) in own.iter().enumerate() {
        let repeats = rooms.is_empty();
        if repeats {
            notes.push(format!("floor {} has no rooms of its own: it repeats floor {}", l + 1, l));
        } else {
            source = rooms.clone();
        }
        let suffix = if l == 0 { String::new() } else { format!("@{l}") };
        let (mut reqs, lab) = room_reqs(&source)?;
        if repeats {
            for r in &mut reqs {
                r.key = format!("{}@{l}", r.key);
            }
            for (k, v) in lab {
                labels.insert(format!("{k}@{l}"), v);
            }
        } else {
            labels.extend(lab);
        }
        for r in &reqs {
            if let Some(prev) = seen_keys.insert(r.key.clone(), l + 1) {
                return Err(format!("room key '{}' is used on floor {prev} and floor {}: keys must be unique", r.key, l + 1));
            }
        }
        let adjacency = adjacency_of(&reqs);
        let (main_env, block) = match &stair {
            Some(s) => {
                let blk = building::stair_block(envelope, entry, s, &suffix)?;
                (blk.main, Some(blk))
            }
            None => (envelope, None),
        };
        merged_rooms.extend(reqs.clone());
        merged_adj.extend(adjacency.clone());
        if let Some(blk) = &block {
            for pr in &blk.rooms {
                merged_rooms.push(RoomReq { key: pr.key.clone(), func: pr.func.clone(), target_area_m2: pr.area_m2, min_width_mm: 1000, zone: pr.zone, needs_light: pr.needs_light });
                let name = if pr.func == "stair" { "Stair" } else { "Stair hall" };
                labels.insert(pr.key.clone(), name.into());
            }
        }
        levels.push(LevelPlan { brief: Brief { envelope: main_env, rooms: reqs, adjacency, entry }, block });
    }
    labels.insert("reserve".into(), "Hall".into());
    for l in 1..n {
        labels.insert(format!("reserve@{l}"), "Hall".into());
    }
    Ok(BuildingPlan { levels, merged: Brief { envelope, rooms: merged_rooms, adjacency: merged_adj, entry }, labels, notes })
}

/// Alternatives for the whole building: alternative `k` takes the `k`-th layout of every level.
pub fn generate_building(plan: &BuildingPlan, seed: u64) -> Result<Vec<Building>, String> {
    let mut per_level: Vec<Vec<Layout>> = Vec::new();
    for (l, lp) in plan.levels.iter().enumerate() {
        let layouts = layout::generate(&lp.brief, &Options { seed: seed.wrapping_add(l as u64), ..Options::default() }).map_err(|e| format!("floor {}: {e}", l + 1))?;
        per_level.push(
            layouts
                .into_iter()
                .map(|mut lay| {
                    // Space keys are unique across the building: the reserve (circulation) room of an upper level
                    // is `reserve@<level>`.
                    if l > 0 {
                        for r in lay.rooms.iter_mut().filter(|r| r.key == "reserve") {
                            r.key = format!("reserve@{l}");
                        }
                    }
                    if let Some(blk) = &lp.block {
                        lay.rooms.extend(blk.rooms.iter().cloned());
                    }
                    lay
                })
                .collect(),
        );
    }
    let count = per_level.iter().map(Vec::len).min().unwrap_or(0).max(1);
    Ok((0..count).map(|k| Building { levels: per_level.iter().map(|v| v[k.min(v.len() - 1)].clone()).collect() }).collect())
}

/// A building whose levels follow rooms the user marked on a sketch. `placement` maps a room key to its rectangle
/// `{x, y, w, h}` in millimetres, in the buildable envelope's own frame (origin at its south-west corner, y up).
/// A level is taken from the sketch only when every one of its rooms is marked; the other levels come from `other`.
/// Rooms must lie inside the envelope and must not overlap each other or the stair block. The rest is left empty:
/// nothing is invented to fill the gaps.
pub fn placed_building(plan: &BuildingPlan, placement: &serde_json::Value, other: &Building) -> Result<Building, String> {
    let get = |key: &str| -> Option<layout::Rect> {
        let r = placement.get(key)?;
        let f = |k: &str| r.get(k).and_then(serde_json::Value::as_f64);
        let round10 = |v: f64| ((v / 10.0).round() as i64) * 10;
        Some(layout::Rect { x: round10(f("x")?), y: round10(f("y")?), w: round10(f("w")?), h: round10(f("h")?) })
    };
    let overlap = |a: &layout::Rect, b: &layout::Rect| a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
    let mut used = 0;
    let mut out = Vec::new();
    for (l, lp) in plan.levels.iter().enumerate() {
        let rects: Vec<Option<layout::Rect>> = lp.brief.rooms.iter().map(|r| get(&r.key)).collect();
        if rects.is_empty() || rects.iter().any(Option::is_none) {
            out.push(other.levels[l.min(other.levels.len() - 1)].clone());
            continue;
        }
        let env = lp.brief.envelope;
        let mut rooms: Vec<layout::PlacedRoom> = Vec::new();
        for (req, rect) in lp.brief.rooms.iter().zip(rects.into_iter().flatten()) {
            if rect.w < 500 || rect.h < 500 {
                return Err(format!("room '{}' marked on the sketch is smaller than 0.5 m in one side", req.key));
            }
            if rect.x < env.x || rect.y < env.y || rect.x + rect.w > env.x + env.w || rect.y + rect.h > env.y + env.h {
                return Err(format!("room '{}' marked on the sketch lies outside the buildable area (plot minus setbacks)", req.key));
            }
            let area = rect.area_m2();
            rooms.push(layout::PlacedRoom { key: req.key.clone(), func: req.func.clone(), zone: req.zone, needs_light: req.needs_light, rect, area_m2: area, target_m2: req.target_area_m2, deviation_pct: (area - req.target_area_m2) / req.target_area_m2 * 100.0 });
        }
        if let Some(blk) = &lp.block {
            rooms.extend(blk.rooms.iter().cloned());
        }
        for i in 0..rooms.len() {
            for j in i + 1..rooms.len() {
                if overlap(&rooms[i].rect, &rooms[j].rect) {
                    return Err(format!("rooms '{}' and '{}' overlap on the sketch", rooms[i].key, rooms[j].key));
                }
            }
        }
        used += 1;
        out.push(layout::Layout { rooms, cost: 0.0, breakdown: layout::Breakdown::default(), reserve_m2: 0.0 });
    }
    if used == 0 {
        return Err("no level has all its rooms marked on the sketch".into());
    }
    Ok(Building { levels: out })
}

/// The structural options for one level, derived from the model itself so a sync after opening a file agrees with
/// the one made at generation: no footings above the ground, columns above pass their loads down, and a stair
/// shaft is left open in the slab unless the level is the top one.
pub fn level_opts(model: &Model, base: &StructOpts, level: Option<Id>) -> StructOpts {
    let Some(lid) = level else { return base.clone() };
    let lv = building::levels(model);
    let idx = lv.iter().position(|(_, id)| *id == lid).unwrap_or(0);
    let mut o = base.clone();
    o.floors = 1;
    o.footings = idx == 0;
    if idx + 1 < lv.len() {
        o.open_rooms = model.elements.values().filter(|e| e.kind == Space::KIND && e.level_id == Some(lid)).filter_map(|e| e.typed::<Space>().ok()).filter(|s| s.func == "stair").map(|s| s.requirement_key).collect();
        let above = lv[idx + 1].1;
        let cols: Vec<Column> = model.elements.values().filter(|e| e.kind == Column::KIND && e.level_id == Some(above)).filter_map(|e| e.typed::<Column>().ok()).collect();
        o.forced_columns = cols.iter().map(|c| (c.x, c.y)).collect();
        o.extra_loads = cols.iter().filter_map(|c| Some((c.x, c.y, c.axial_service_kn?))).collect();
    }
    o
}

#[derive(Debug, Default)]
pub struct ChainReport {
    pub created: usize,
    pub removed: usize,
    pub unchanged: usize,
    /// Doors and windows moved along their wall to clear a column.
    pub shifted: usize,
    pub warnings: Vec<String>,
}

/// Brings the structure of the levels `from` and below in line with the walls, top down (a level needs the loads of
/// the one above). Each step is applied to `scratch`; the commands of all steps are returned in order. `from` is a
/// level index; `None` means every level.
pub fn structure_chain(scratch: &mut Document, base: &StructOpts, from: Option<usize>) -> Result<(Vec<Command>, ChainReport), String> {
    let lv = building::levels(&scratch.model);
    let mut all = Vec::new();
    let mut rep = ChainReport::default();
    let steps: Vec<Option<Id>> = if lv.is_empty() { vec![None] } else { lv.iter().enumerate().rev().filter(|(i, _)| from.is_none_or(|f| *i <= f)).map(|(_, (_, id))| Some(*id)).collect() };
    let mut orphans = 0usize;
    for lid in steps {
        let opts = level_opts(&scratch.model, base, lid);
        let (cmds, r) = structural::sync_commands(&scratch.model, &opts, lid)?;
        rep.created += r.created;
        rep.removed += r.removed;
        rep.unchanged += r.unchanged;
        for w in r.warnings {
            if w.starts_with("a column above at") {
                orphans += 1;
            } else if !rep.warnings.contains(&w) {
                rep.warnings.push(w);
            }
        }
        if !cmds.is_empty() {
            scratch.commit("structure", Origin::Generator, cmds.clone()).map_err(|e| e.to_string())?;
            all.extend(cmds);
        }
    }
    if orphans > 0 {
        rep.warnings.push(format!("{orphans} column(s) on upper levels have no column below them: their load is not carried to the foundations (align the rooms or add transfer beams; an engineer must decide)"));
    }
    Ok((all, rep))
}

/// The structure of the levels `from` and below, then every door or window standing in a column moved clear of it,
/// repeated until nothing changes (moving an opening can change where an extra column goes). All commands are
/// applied to `scratch` and returned in order. Openings that cannot be moved are named in the report.
pub fn settle(scratch: &mut Document, base: &StructOpts, from: Option<usize>) -> Result<(Vec<Command>, ChainReport), String> {
    let mut all = Vec::new();
    let mut total = ChainReport::default();
    for round in 0..4 {
        let (cmds, rep) = structure_chain(scratch, base, from)?;
        all.extend(cmds);
        total.created += rep.created;
        total.removed += rep.removed;
        if round == 0 {
            total.unchanged = rep.unchanged;
        }
        let lv = building::levels(&scratch.model);
        let ids: Vec<Option<Id>> = if lv.is_empty() { vec![None] } else { lv.iter().enumerate().filter(|(i, _)| from.is_none_or(|f| *i <= f)).map(|(_, (_, id))| Some(*id)).collect() };
        let (fix, notes) = building::clear_openings(&scratch.model, &ids);
        if fix.is_empty() {
            total.warnings = rep.warnings;
            total.warnings.extend(notes);
            if total.shifted > 0 {
                total.warnings.insert(0, format!("{} door/window position(s) were moved along their wall to clear columns", total.shifted));
            }
            return Ok((all, total));
        }
        total.shifted += fix.len();
        scratch.commit("move openings clear of columns", Origin::Generator, fix.clone()).map_err(|e| e.to_string())?;
        all.extend(fix);
    }
    total.warnings.push("door and window positions did not settle after 4 passes: check the openings next to columns".into());
    Ok((all, total))
}

/// Commits the structure of every level as one undoable step on `doc`.
pub fn sync_all(doc: &mut Document, base: &StructOpts) -> Result<ChainReport, String> {
    let mut scratch = Document::new(doc.model.clone());
    let (cmds, rep) = settle(&mut scratch, base, None)?;
    if !cmds.is_empty() {
        doc.commit("sync structure with the model", Origin::Generator, cmds).map_err(|e| e.to_string())?;
    }
    Ok(rep)
}

/// Spaces, walls, doors, windows and structure of the whole building, committed as one step.
pub fn assemble(b: &Building, plan: &BuildingPlan, base: &StructOpts) -> Result<(Document, Vec<String>), String> {
    let n = b.levels.len();
    let multi = n > 1;
    let mut arch: Vec<Command> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for (l, layout) in b.levels.iter().enumerate() {
        let lid = if multi {
            let el = element_of(&Level { name: level_name(l), index: l as i64, extra: Default::default() }, Source::Generated);
            let id = el.id;
            arch.push(Command::Create(el));
            Some(id)
        } else {
            None
        };
        let mut wo = walls::opts_from_adjacency(&plan.levels[l].brief.adjacency);
        wo.entry = l == 0;
        let wplan = walls::build(layout, plan.merged.envelope, plan.merged.entry, &wo);
        let wp = walls::place_windows(layout, &wplan, &wo, &WindowOpts::default());
        let space_cmds = layout::to_commands(layout, lid);
        let ids = walls::space_ids(&space_cmds);
        arch.extend(space_cmds);
        arch.extend(walls::to_commands(&wplan, &ids, lid));
        arch.extend(walls::window_commands(&wplan, &wp, &ids, lid));
        let tag = if multi { format!("{}: ", level_name(l)) } else { String::new() };
        warnings.extend(wplan.warnings.iter().chain(&wp.warnings).map(|w| format!("{tag}{w}")));
    }
    // The structure is derived from the architecture as the MODEL holds it, exactly as a later sync does.
    let mut scratch = Document::default();
    scratch.commit("architecture", Origin::Generator, arch.clone()).map_err(|e| e.to_string())?;
    let (cmds, rep) = settle(&mut scratch, base, None)?;
    warnings.extend(rep.warnings);
    let mut all = arch;
    all.extend(cmds);
    let mut doc = Document::default();
    doc.commit("generate house", Origin::Generator, all).map_err(|e| e.to_string())?;
    Ok((doc, warnings))
}
