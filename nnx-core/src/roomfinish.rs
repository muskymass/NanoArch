//! Room finishes schedule: floor, wall and ceiling finish of every room, with the measured area of each surface.
//!
//! The finish catalogue is the user's: each entry has a code, a name, the surface it is for (`floor`, `wall` or `ceiling`) and a
//! `source` (the specification or supplier document). No finish, thickness, price or rating is built in. Finishes are assigned to
//! a room by its requirement key (wins) or by its function; a surface with no assignment is listed as `unassigned` and never
//! given a finish. Areas come from the model's rooms:
//!
//! - floor and ceiling area = the room's net area when the model stores one, else its gross rectangle (`area_basis` says which);
//!   the ceiling is taken as flat and equal to the floor area;
//! - wall area = the room's rectangle perimeter x the wall height you enter, GROSS: door and window openings are not deducted and
//!   wall thickness is not subtracted. The wall height is required as soon as any wall finish is assigned, and is never defaulted.
//!
//! The totals per finish code are what a take-off row needs. This is a schedule of the user's choices with measured areas, not a
//! specification and not a price.

use crate::model::{Id, Model};
use crate::typed::{Level, Space, Typed};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

/// The most finishes and assignments accepted in one call.
pub const MAX_ITEMS: usize = 500;

pub const SURFACES: [&str; 3] = ["floor", "wall", "ceiling"];

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FinishDef {
    pub code: String,
    pub name: String,
    /// `floor`, `wall` or `ceiling`.
    pub surface: String,
    /// The specification or supplier document the finish comes from. Required.
    pub source: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Assignment {
    /// The room's requirement key. Wins over `func`.
    pub key: String,
    /// The room function (for example `bedroom`); applies to every room of it without its own key assignment.
    pub func: String,
    /// Finish codes. An empty one leaves that surface unassigned.
    pub floor: String,
    pub wall: String,
    pub ceiling: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Input {
    pub finishes: Vec<FinishDef>,
    pub assignments: Vec<Assignment>,
    /// Wall height in millimetres; required when any wall finish is assigned.
    pub wall_height_mm: Option<f64>,
    /// Restrict to the rooms of one level by name; empty means every level.
    pub level: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Surface {
    /// `floor`, `wall` or `ceiling`.
    pub surface: &'static str,
    pub code: Option<String>,
    pub name: Option<String>,
    pub source: Option<String>,
    /// m2; `None` when it cannot be measured (a wall without a wall height).
    pub area_m2: Option<f64>,
    /// `assigned` or `unassigned`.
    pub status: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoomRow {
    pub key: String,
    pub func: String,
    pub level: String,
    /// `net` or `gross`: which floor area the model gave.
    pub area_basis: &'static str,
    pub surfaces: Vec<Surface>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Total {
    pub surface: &'static str,
    pub code: String,
    pub name: String,
    pub rooms: usize,
    pub area_m2: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Schedule {
    pub rows: Vec<RoomRow>,
    pub totals: Vec<Total>,
    /// `room / surface` pairs with no finish assigned.
    pub unassigned: Vec<String>,
    pub complete: bool,
    pub csv: String,
    pub notes: Vec<String>,
}

fn csv_field(s: &str) -> String {
    let guarded = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{s}") } else { s.to_string() };
    format!("\"{}\"", guarded.replace('"', "\"\""))
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

struct RoomData {
    key: String,
    func: String,
    level_id: Option<Id>,
    area_m2: f64,
    basis: &'static str,
    perimeter_m: f64,
}

/// Builds the schedule. Refuses, naming each problem, when an input is missing or malformed.
pub fn build(model: &Model, input: &Input) -> Result<Schedule, String> {
    let mut errs: Vec<String> = Vec::new();
    if input.finishes.is_empty() {
        errs.push("finishes (the catalogue of finishes with code, name, surface and source)".into());
    }
    if input.assignments.is_empty() {
        errs.push("assignments (which finish goes to which room key or function)".into());
    }
    if input.finishes.len() > MAX_ITEMS || input.assignments.len() > MAX_ITEMS {
        errs.push(format!("at most {MAX_ITEMS} finishes and {MAX_ITEMS} assignments are accepted"));
    }
    let mut defs: HashMap<&str, &FinishDef> = HashMap::new();
    for (i, f) in input.finishes.iter().enumerate() {
        let code = f.code.trim();
        if code.is_empty() {
            errs.push(format!("finishes[{i}].code"));
            continue;
        }
        if f.name.trim().is_empty() {
            errs.push(format!("finishes[{i}].name"));
        }
        if !SURFACES.contains(&f.surface.trim()) {
            errs.push(format!("finishes[{i}].surface must be one of floor, wall, ceiling, got `{}`", f.surface.trim()));
        }
        if f.source.trim().is_empty() {
            errs.push(format!("finishes[{i}].source (the specification or supplier document of `{code}`)"));
        }
        if defs.insert(code, f).is_some() {
            errs.push(format!("finishes[{i}].code `{code}` is duplicated"));
        }
    }
    let mut by_key: HashMap<&str, &Assignment> = HashMap::new();
    let mut by_func: HashMap<&str, &Assignment> = HashMap::new();
    let mut any_wall = false;
    for (i, a) in input.assignments.iter().enumerate() {
        let (k, f) = (a.key.trim(), a.func.trim());
        match (k.is_empty(), f.is_empty()) {
            (true, true) => errs.push(format!("assignments[{i}] needs a key or a func")),
            (false, false) => errs.push(format!("assignments[{i}] has both key and func; use one")),
            (false, true) => {
                if by_key.insert(k, a).is_some() {
                    errs.push(format!("assignments[{i}].key `{k}` is duplicated"));
                }
            }
            (true, false) => {
                if by_func.insert(f, a).is_some() {
                    errs.push(format!("assignments[{i}].func `{f}` is duplicated"));
                }
            }
        }
        for (surface, code) in [("floor", &a.floor), ("wall", &a.wall), ("ceiling", &a.ceiling)] {
            let c = code.trim();
            if c.is_empty() {
                continue;
            }
            match defs.get(c) {
                None => errs.push(format!("assignments[{i}].{surface} `{c}` is not in the finishes catalogue")),
                Some(d) if d.surface.trim() != surface => errs.push(format!("assignments[{i}].{surface} `{c}` is a {} finish", d.surface.trim())),
                Some(_) => {}
            }
            any_wall |= surface == "wall";
        }
    }
    let wall_h = match input.wall_height_mm {
        Some(h) if h.is_finite() && h > 0.0 => Some(h),
        Some(h) => {
            errs.push(format!("wall_height_mm must be a positive number, got {h}"));
            None
        }
        None => {
            if any_wall {
                errs.push("wall_height_mm (required because a wall finish is assigned; it is never defaulted)".into());
            }
            None
        }
    };
    let level_names: BTreeMap<Id, String> = model.elements.values().filter(|e| e.kind == Level::KIND).filter_map(|e| Some((e.id, e.typed::<Level>().ok()?.name))).collect();
    let level_filter = input.level.trim();
    let filter_id: Option<Id> = if level_filter.is_empty() {
        None
    } else {
        match level_names.iter().find(|(_, n)| n.as_str() == level_filter) {
            Some((id, _)) => Some(*id),
            None => {
                errs.push(format!("level `{level_filter}` is not a level of the model"));
                None
            }
        }
    };
    if !errs.is_empty() {
        return Err(format!("room finishes schedule: {}. Nothing is defaulted and no finish catalogue is built in; enter them.", errs.join("; ")));
    }

    let mut rooms: Vec<RoomData> = Vec::new();
    for e in model.elements.values().filter(|e| e.kind == Space::KIND) {
        if !level_filter.is_empty() && e.level_id != filter_id {
            continue;
        }
        let Ok(s) = e.typed::<Space>() else { continue };
        let (area_m2, basis) = match s.net_area {
            Some(n) if n.is_finite() && n > 0.0 => (n, "net"),
            _ => (s.w as f64 * s.h as f64 / 1e6, "gross"),
        };
        rooms.push(RoomData { key: s.requirement_key, func: s.func, level_id: e.level_id, area_m2, basis, perimeter_m: 2.0 * (s.w + s.h) as f64 / 1000.0 });
    }
    if rooms.is_empty() {
        return Err("room finishes schedule: the model has no rooms (Space) to schedule".into());
    }
    rooms.sort_by(|a, b| (a.level_id, &a.key).cmp(&(b.level_id, &b.key)));

    let mut rows: Vec<RoomRow> = Vec::new();
    let mut unassigned: Vec<String> = Vec::new();
    let mut totals: BTreeMap<(usize, String), Total> = BTreeMap::new();
    let mut used_keys: HashSet<&str> = HashSet::new();
    let mut used_funcs: HashSet<&str> = HashSet::new();
    for r in &rooms {
        let level = r.level_id.and_then(|id| level_names.get(&id).cloned()).unwrap_or_default();
        let assign = by_key.get(r.key.as_str()).inspect(|_| {
            used_keys.insert(r.key.as_str());
        });
        let assign = assign.or_else(|| {
            by_func.get(r.func.as_str()).inspect(|_| {
                used_funcs.insert(r.func.as_str());
            })
        });
        let mut surfaces = Vec::new();
        for (si, surface) in SURFACES.iter().enumerate() {
            let code = assign.map(|a| match *surface {
                "floor" => a.floor.trim(),
                "wall" => a.wall.trim(),
                _ => a.ceiling.trim(),
            });
            let area = match *surface {
                "wall" => wall_h.map(|h| round2(r.perimeter_m * h / 1000.0)),
                _ => Some(round2(r.area_m2)),
            };
            match code.filter(|c| !c.is_empty()).and_then(|c| defs.get(c)) {
                Some(d) => {
                    if let Some(a) = area {
                        let t = totals.entry((si, d.code.trim().to_string())).or_insert_with(|| Total { surface, code: d.code.trim().to_string(), name: d.name.trim().to_string(), rooms: 0, area_m2: 0.0 });
                        t.rooms += 1;
                        t.area_m2 = round2(t.area_m2 + a);
                    }
                    surfaces.push(Surface { surface, code: Some(d.code.trim().to_string()), name: Some(d.name.trim().to_string()), source: Some(d.source.trim().to_string()), area_m2: area, status: "assigned" });
                }
                None => {
                    unassigned.push(format!("{} ({}) / {surface}", r.key, if level.is_empty() { "-" } else { &level }));
                    surfaces.push(Surface { surface, code: None, name: None, source: None, area_m2: area, status: "unassigned" });
                }
            }
        }
        rows.push(RoomRow { key: r.key.clone(), func: r.func.clone(), level, area_basis: r.basis, surfaces });
    }

    let mut csv = String::from("level,room,function,surface,finish_code,finish_name,source,area_m2,area_basis,status\n");
    for row in &rows {
        for s in &row.surfaces {
            csv.push_str(&format!(
                "{},{},{},{},{},{},{},{},{},{}\n",
                csv_field(&row.level),
                csv_field(&row.key),
                csv_field(&row.func),
                s.surface,
                csv_field(s.code.as_deref().unwrap_or("")),
                csv_field(s.name.as_deref().unwrap_or("")),
                csv_field(s.source.as_deref().unwrap_or("")),
                s.area_m2.map(|a| format!("{a:.2}")).unwrap_or_default(),
                if s.surface == "wall" { "gross" } else { row.area_basis },
                s.status
            ));
        }
    }

    let mut notes = vec![
        "Finishes are the user's catalogue with its sources; the application does not know any finish. Floor and ceiling area is the room's net area when the model has one, else its gross rectangle; the ceiling is taken as flat. Wall area is perimeter x the wall height entered, gross: openings and wall thickness are not deducted / التشطيبات من كتالوج المستخدم بمصادره؛ مساحة الجدار إجمالية (بلا خصم الفتحات وسماكة الجدران) والسقف مستوٍ.".to_string(),
    ];
    if wall_h.is_none() {
        notes.push("No wall height was entered and no wall finish is assigned: wall areas are not measured / لم يُدخل ارتفاع جدار ولا تشطيب جدران؛ لم تُقس مساحات الجدران.".to_string());
    }
    let dead_keys: Vec<&str> = by_key.keys().copied().filter(|k| !used_keys.contains(k)).collect();
    let dead_funcs: Vec<&str> = by_func.keys().copied().filter(|f| !used_funcs.contains(f)).collect();
    if !dead_keys.is_empty() || !dead_funcs.is_empty() {
        let mut names: Vec<String> = dead_keys.iter().chain(dead_funcs.iter()).map(|s| s.to_string()).collect();
        names.sort();
        notes.push(format!("Assignments that matched no room in the schedule (check for typos or the level filter): {} / تعيينات لم تطابق أي غرفة.", names.join(", ")));
    }
    if !unassigned.is_empty() {
        notes.push(format!("{} room surface(s) have no finish assigned / أسطح غرف بلا تشطيب معيَّن.", unassigned.len()));
    }
    let mut totals: Vec<Total> = totals.into_values().collect();
    totals.sort_by(|a, b| (a.surface, &a.code).cmp(&(b.surface, &b.code)));
    Ok(Schedule { rows, totals, complete: unassigned.is_empty(), unassigned, csv, notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Command, Origin};
    use crate::model::Source;
    use crate::typed::element_of;
    use crate::Document;

    fn space(key: &str, func: &str, w: i64, h: i64, net: Option<f64>) -> Space {
        Space { func: func.into(), requirement_key: key.into(), area: (w * h) as f64 / 1e6, target_area: 0.0, x: 0, y: 0, w, h, boundary: vec![[0, 0], [w, 0], [w, h], [0, h]], net_area: net, glazing_ratio: None, extra: Default::default() }
    }

    fn doc() -> Document {
        let mut d = Document::default();
        let lv = Level { name: "Ground".into(), index: 0, extra: Default::default() };
        let lel = element_of(&lv, Source::User);
        let lid = lel.id;
        let mut cmds = vec![Command::Create(lel)];
        for s in [space("bed1", "bedroom", 4000, 3000, None), space("bed2", "bedroom", 4000, 4000, Some(14.0)), space("bath", "bathroom", 2000, 2000, None)] {
            let mut e = element_of(&s, Source::User);
            e.level_id = Some(lid);
            cmds.push(Command::Create(e));
        }
        d.commit("rooms", Origin::User, cmds).unwrap();
        d
    }

    fn def(code: &str, surface: &str) -> FinishDef {
        FinishDef { code: code.into(), name: format!("{code} name"), surface: surface.into(), source: "Spec section 09, 2026-01-01".into() }
    }

    fn input() -> Input {
        Input {
            finishes: vec![def("F1", "floor"), def("F2", "floor"), def("W1", "wall"), def("C1", "ceiling")],
            assignments: vec![
                Assignment { func: "bedroom".into(), floor: "F1".into(), wall: "W1".into(), ceiling: "C1".into(), ..Default::default() },
                Assignment { key: "bed2".into(), floor: "F2".into(), wall: "W1".into(), ceiling: "C1".into(), ..Default::default() },
            ],
            wall_height_mm: Some(3000.0),
            level: String::new(),
        }
    }

    #[test]
    fn measures_areas_and_a_key_assignment_wins_over_the_function() {
        let s = build(&doc().model, &input()).unwrap();
        let bed1 = s.rows.iter().find(|r| r.key == "bed1").unwrap();
        assert_eq!(bed1.level, "Ground");
        assert_eq!(bed1.surfaces[0].code.as_deref(), Some("F1"));
        assert_eq!(bed1.surfaces[0].area_m2, Some(12.0));
        // perimeter 14 m x 3 m
        assert_eq!(bed1.surfaces[1].area_m2, Some(42.0));
        let bed2 = s.rows.iter().find(|r| r.key == "bed2").unwrap();
        assert_eq!(bed2.surfaces[0].code.as_deref(), Some("F2"));
        assert_eq!((bed2.area_basis, bed2.surfaces[0].area_m2), ("net", Some(14.0)));
        let w1 = s.totals.iter().find(|t| t.code == "W1").unwrap();
        assert_eq!((w1.rooms, w1.area_m2), (2, 42.0 + 48.0));
    }

    #[test]
    fn unassigned_surfaces_are_listed_and_never_given_a_finish() {
        let s = build(&doc().model, &input()).unwrap();
        assert!(!s.complete);
        assert_eq!(s.unassigned.len(), 3);
        assert!(s.unassigned.iter().all(|u| u.starts_with("bath")));
        let bath = s.rows.iter().find(|r| r.key == "bath").unwrap();
        assert!(bath.surfaces.iter().all(|x| x.status == "unassigned" && x.code.is_none()));
    }

    #[test]
    fn missing_or_inconsistent_inputs_are_named() {
        let e = build(&doc().model, &Input::default()).unwrap_err();
        assert!(e.contains("finishes") && e.contains("assignments"), "{e}");
        let mut i = input();
        i.wall_height_mm = None;
        i.finishes[0].source = " ".into();
        i.assignments[0].ceiling = "W1".into();
        i.assignments[1].floor = "NOPE".into();
        i.level = "Roof".into();
        let e = build(&doc().model, &i).unwrap_err();
        assert!(e.contains("wall_height_mm") && e.contains("finishes[0].source") && e.contains("is a wall finish") && e.contains("not in the finishes catalogue") && e.contains("Roof"), "{e}");
    }

    #[test]
    fn wall_height_is_not_needed_without_a_wall_finish() {
        let mut i = input();
        i.wall_height_mm = None;
        for a in &mut i.assignments {
            a.wall.clear();
        }
        let s = build(&doc().model, &i).unwrap();
        assert!(s.rows[0].surfaces[1].area_m2.is_none());
        assert!(s.totals.iter().all(|t| t.surface != "wall"));
    }

    #[test]
    fn reports_assignments_that_match_no_room_and_guards_csv() {
        let mut i = input();
        i.assignments.push(Assignment { func: "kitchn".into(), floor: "F1".into(), ..Default::default() });
        i.finishes[0].name = "=cmd|' /C calc'!A0".into();
        let s = build(&doc().model, &i).unwrap();
        assert!(s.notes.iter().any(|n| n.contains("kitchn")));
        assert!(s.csv.starts_with("level,room,function,surface,"));
        assert!(s.csv.contains("\"'=cmd"), "{}", s.csv);
    }
}
