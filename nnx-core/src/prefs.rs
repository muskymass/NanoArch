//! Learned room-size preferences: what the user's earlier projects say about how large they make each kind of room.
//!
//! - `learn` reads the rooms of past projects and keeps, per room function, the median area and how many rooms it came from.
//! - A function with fewer than `MIN_SAMPLES` rooms is not learned: one or two rooms are an accident, not a habit, and the
//!   built-in starting target stays in force for it.
//! - `apply` replaces only the *suggested* areas of a draft (never an area the client or the model stated) and says so in the
//!   draft's notes, so the page can show "from your previous N rooms". The learned figure is not checked against the SBC here:
//!   the rule engine checks the minimum area when the plan is built and reports a failure by name.

use crate::briefai::BriefDraft;
use crate::project::NanoArchProject;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Fewest rooms of one function before a preference is trusted.
pub const MIN_SAMPLES: usize = 3;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomPreference {
    pub func: String,
    /// Median area in m² of the rooms of this function in the past projects.
    pub median_m2: f64,
    pub min_m2: f64,
    pub max_m2: f64,
    pub samples: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Preferences {
    pub rooms: Vec<RoomPreference>,
    /// How many projects were read, and how many of them had a brief with rooms.
    pub projects_read: usize,
    pub projects_used: usize,
    /// Functions seen but with too few rooms to learn from, as `(function, rooms found)`.
    pub too_few: Vec<(String, usize)>,
}

impl Preferences {
    pub fn get(&self, func: &str) -> Option<&RoomPreference> {
        self.rooms.iter().find(|r| r.func == func)
    }
}

fn median(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    if n % 2 == 1 { sorted[n / 2] } else { (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0 }
}

/// Reads the preferences out of past projects. Rooms with a non-finite or non-positive area are ignored.
pub fn learn(projects: &[NanoArchProject]) -> Preferences {
    let mut by_func: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut used = 0;
    for p in projects {
        let Some(b) = &p.brief else { continue };
        let before: usize = by_func.values().map(Vec::len).sum();
        for r in b.rooms.iter().filter(|r| r.area.is_finite() && r.area > 0.0) {
            by_func.entry(r.func.clone()).or_default().push(r.area);
        }
        if by_func.values().map(Vec::len).sum::<usize>() > before {
            used += 1;
        }
    }
    let mut out = Preferences { projects_read: projects.len(), projects_used: used, ..Default::default() };
    for (func, mut areas) in by_func {
        if areas.len() < MIN_SAMPLES {
            out.too_few.push((func, areas.len()));
            continue;
        }
        areas.sort_by(|a, b| a.total_cmp(b));
        out.rooms.push(RoomPreference { func, median_m2: (median(&areas) * 10.0).round() / 10.0, min_m2: areas[0], max_m2: areas[areas.len() - 1], samples: areas.len() });
    }
    out
}

/// Puts the learned medians on the draft's suggested areas. Returns how many rooms changed.
pub fn apply(draft: &mut BriefDraft, prefs: &Preferences) -> usize {
    let mut changed = 0;
    for r in draft.rooms.iter_mut().filter(|r| r.area_suggested) {
        if let Some(p) = prefs.get(&r.func)
            && r.area_m2 != Some(p.median_m2)
        {
            r.area_m2 = Some(p.median_m2);
            changed += 1;
        }
    }
    if changed > 0 {
        draft.notes.push(format!("{changed} suggested room area(s) taken from the median of your earlier projects (at least {MIN_SAMPLES} rooms of the kind): review them"));
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::briefai::DraftRoom;
    use crate::project::{BriefIn, RoomIn};

    fn room(func: &str, area: f64) -> RoomIn {
        RoomIn { key: format!("{func}{area}"), name: None, func: func.into(), area, zone: "private".into(), light: false, min_width_mm: None, floor: None, phase: None }
    }

    fn project(rooms: Vec<RoomIn>) -> NanoArchProject {
        NanoArchProject {
            brief: Some(BriefIn { title: None, width_m: 20.0, length_m: 30.0, setback_front_m: 0.0, setback_back_m: 0.0, setback_side_m: 0.0, entry: "south".into(), rooms, seed: None, stair: None, wall_exterior_mm: None, wall_interior_mm: None, wall_height_mm: None, adjacency: vec![] }),
            ..Default::default()
        }
    }

    #[test]
    fn median_needs_enough_rooms_and_ignores_bad_areas() {
        let ps = vec![
            project(vec![room("bedroom", 20.0), room("bedroom", 18.0), room("majlis", 40.0), room("kitchen", f64::NAN)]),
            project(vec![room("bedroom", 24.0), room("bedroom", -3.0), room("majlis", 36.0)]),
            NanoArchProject::default(),
        ];
        let p = learn(&ps);
        assert_eq!((p.projects_read, p.projects_used), (3, 2));
        let b = p.get("bedroom").unwrap();
        assert_eq!((b.median_m2, b.min_m2, b.max_m2, b.samples), (20.0, 18.0, 24.0, 3));
        assert!(p.get("majlis").is_none(), "two majlis rooms are not a habit");
        assert_eq!(p.too_few, vec![("majlis".to_string(), 2)]);
    }

    #[test]
    fn even_count_averages_the_middle_pair() {
        let p = learn(&[project(vec![room("bedroom", 12.0), room("bedroom", 14.0), room("bedroom", 18.0), room("bedroom", 20.0)])]);
        assert_eq!(p.get("bedroom").unwrap().median_m2, 16.0);
    }

    #[test]
    fn apply_changes_only_suggested_areas_of_learned_functions() {
        let p = learn(&[project(vec![room("bedroom", 20.0), room("bedroom", 22.0), room("bedroom", 24.0)])]);
        let mk = |func: &str, area: f64, suggested: bool| DraftRoom { func: func.into(), area_m2: Some(area), area_suggested: suggested, floor: 1, ..Default::default() };
        let mut d = BriefDraft { rooms: vec![mk("bedroom", 16.0, true), mk("bedroom", 14.0, false), mk("kitchen", 15.0, true)], ..Default::default() };
        assert_eq!(apply(&mut d, &p), 1);
        assert_eq!(d.rooms[0].area_m2, Some(22.0));
        assert_eq!(d.rooms[1].area_m2, Some(14.0), "a stated area is never overwritten");
        assert_eq!(d.rooms[2].area_m2, Some(15.0), "no preference for the kitchen: the starting target stays");
        assert_eq!(d.notes.len(), 1);
        assert_eq!(apply(&mut d, &p), 0);
        assert_eq!(d.notes.len(), 1);
    }

    #[test]
    fn no_projects_learns_nothing() {
        assert_eq!(learn(&[]), Preferences::default());
    }
}
