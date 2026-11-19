//! Combined footings and rafts: isolated footings that overlap or stand closer than `minGapMm` are merged into one combined footing
//! (`CFND-n`); when the footings cover more than `raftThresholdPct` of the ground-floor footprint they are all merged into one raft
//! (`RAFT-1`), extended `raftEdgeMm` beyond the outermost footings.
//!
//! The merged footing is an ordinary `Footing` element (so the 2D plan, the 3D scene, the schedules and the BOQ follow by themselves)
//! that carries its own `label`, the ids of ALL the columns it carries (`columns`) and the footings it replaces (`mergedFrom`). It is
//! sized to the sum of the loads at the lowest allowable bearing of the group: when the bounding box of the group is smaller than that
//! area, it is enlarged evenly (to 50 mm). The three limits are your inputs; without them nothing is merged and they are named.
//! "Sync structure" does not recreate the single footings of columns a merged footing carries.

use crate::command::Command;
use crate::model::{Id, Model, Source};
use crate::structx::schedule_names;
use crate::typed::{Footing, Space, Typed, element_of};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MergeInputs {
    /// Footings closer than this (or overlapping) are merged, mm.
    pub min_gap_mm: Option<f64>,
    /// Merge everything into a raft when the footings cover more than this percentage of the ground-floor footprint.
    pub raft_threshold_pct: Option<f64>,
    /// How far a raft reaches beyond the outermost footings, mm.
    pub raft_edge_mm: Option<f64>,
}

impl MergeInputs {
    pub fn check(&self) -> Result<(), String> {
        for (n, v) in [("minimum gap", self.min_gap_mm), ("raft threshold", self.raft_threshold_pct), ("raft edge", self.raft_edge_mm)] {
            if v.is_some_and(|x| !x.is_finite() || x < 0.0) {
                return Err(format!("{n} must be zero or more"));
            }
        }
        if self.raft_threshold_pct.is_some_and(|x| x > 100.0) {
            return Err("the raft threshold is a percentage up to 100".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Group {
    pub label: String,
    /// `combined` or `raft`.
    pub kind: &'static str,
    pub footings: Vec<String>,
    pub footing_ids: Vec<String>,
    pub columns: Vec<String>,
    /// `[x0, y0, x1, y1]`, mm.
    pub rect: [i64; 4],
    pub length_mm: i64,
    pub width_mm: i64,
    pub load_kn: f64,
    pub bearing_kpa: f64,
    pub required_m2: f64,
    pub provided_m2: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct MergePlan {
    pub groups: Vec<Group>,
    pub coverage_pct: Option<f64>,
    pub raft: bool,
    pub footings: usize,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
}

struct F {
    id: Id,
    name: String,
    f: Footing,
    r: [i64; 4],
}

fn gap(a: &[i64; 4], b: &[i64; 4]) -> f64 {
    let dx = (a[0].max(b[0]) - a[2].min(b[2])).max(0) as f64;
    let dy = (a[1].max(b[1]) - a[3].min(b[3])).max(0) as f64;
    dx.hypot(dy)
}

fn up50(v: f64) -> i64 {
    ((v / 50.0).ceil() * 50.0) as i64
}

pub fn plan(model: &Model, inp: &MergeInputs, footing_thickness_mm: Option<f64>) -> MergePlan {
    let mut out = MergePlan::default();
    for (n, ok) in [("minimum gap between footings (mm)", inp.min_gap_mm.is_some()), ("raft threshold (% of the footprint)", inp.raft_threshold_pct.is_some()), ("raft edge distance (mm)", inp.raft_edge_mm.is_some())] {
        if !ok {
            out.missing.push(n.to_string());
        }
    }
    let nm = schedule_names(model, footing_thickness_mm);
    let mut fs: Vec<F> = model
        .elements
        .values()
        .filter(|e| e.kind == Footing::KIND)
        .filter_map(|e| {
            let f = e.typed::<Footing>().ok()?;
            let r = [f.x - f.length / 2, f.y - f.width / 2, f.x + f.length / 2, f.y + f.width / 2];
            Some(F { id: e.id, name: nm.get(&e.id.to_string()).cloned().unwrap_or_else(|| e.id.to_string()), f, r })
        })
        .collect();
    fs.sort_by(|a, b| a.name.cmp(&b.name));
    out.footings = fs.len();
    if fs.len() < 2 {
        out.notes.push("fewer than two footings: nothing to merge".into());
        return out;
    }
    // Coverage of the ground-floor footprint.
    let ground = crate::building::levels(model).first().map(|(_, id)| *id);
    let footprint: f64 = model.elements.values().filter(|e| e.kind == Space::KIND && (ground.is_none() || e.level_id == ground)).filter_map(|e| e.typed::<Space>().ok()).map(|s| s.w as f64 * s.h as f64 / 1e6).sum();
    let covered: f64 = fs.iter().map(|f| f.f.length as f64 * f.f.width as f64 / 1e6).sum();
    if footprint > 0.0 {
        out.coverage_pct = Some((covered / footprint * 1000.0).round() / 10.0);
    } else {
        out.missing.push("ground-floor rooms (the footprint the coverage is measured against)".into());
    }
    let (Some(gap_min), Some(thr), Some(edge)) = (inp.min_gap_mm, inp.raft_threshold_pct, inp.raft_edge_mm) else { return out };
    let Some(cov) = out.coverage_pct else { return out };
    // Union-find on "closer than the gap".
    let n = fs.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], i: usize) -> usize {
        if p[i] != i {
            let r = find(p, p[i]);
            p[i] = r;
        }
        p[i]
    }
    out.raft = cov > thr + 1e-9;
    for i in 0..n {
        for j in i + 1..n {
            if out.raft || gap(&fs[i].r, &fs[j].r) < gap_min - 1e-9 || gap(&fs[i].r, &fs[j].r) == 0.0 {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                parent[a] = b;
            }
        }
    }
    let mut roots: Vec<usize> = (0..n).map(|i| find(&mut parent, i)).collect();
    roots.sort_unstable();
    roots.dedup();
    let mut cf = 0;
    for root in roots {
        let members: Vec<usize> = (0..n).filter(|&i| find(&mut parent, i) == root).collect();
        if members.len() < 2 {
            continue;
        }
        let pad = if out.raft { edge as i64 } else { 0 };
        let rect = members.iter().fold([i64::MAX, i64::MAX, i64::MIN, i64::MIN], |r, &i| [r[0].min(fs[i].r[0]), r[1].min(fs[i].r[1]), r[2].max(fs[i].r[2]), r[3].max(fs[i].r[3])]);
        let rect = [rect[0] - pad, rect[1] - pad, rect[2] + pad, rect[3] + pad];
        let load: f64 = members.iter().map(|&i| fs[i].f.service_load_kn).sum();
        let bearing = members.iter().map(|&i| fs[i].f.bearing_kpa).fold(f64::MAX, f64::min);
        let required = load / bearing;
        let (mut l, mut w) = ((rect[2] - rect[0]) as f64, (rect[3] - rect[1]) as f64);
        if l * w / 1e6 < required {
            let k = (required / (l * w / 1e6)).sqrt();
            l = up50(l * k) as f64;
            w = up50(w * k) as f64;
        }
        let (cx, cy) = ((rect[0] + rect[2]) / 2, (rect[1] + rect[3]) / 2);
        let (li, wi) = (l as i64, w as i64);
        let label = if out.raft {
            "RAFT-1".to_string()
        } else {
            cf += 1;
            format!("CFND-{cf}")
        };
        out.groups.push(Group {
            label,
            kind: if out.raft { "raft" } else { "combined" },
            footings: members.iter().map(|&i| fs[i].name.clone()).collect(),
            footing_ids: members.iter().map(|&i| fs[i].id.to_string()).collect(),
            columns: members.iter().map(|&i| fs[i].f.column.clone()).collect(),
            rect: [cx - li / 2, cy - wi / 2, cx - li / 2 + li, cy - wi / 2 + wi],
            length_mm: li,
            width_mm: wi,
            load_kn: (load * 10.0).round() / 10.0,
            bearing_kpa: bearing,
            required_m2: (required * 100.0).round() / 100.0,
            provided_m2: (l * w / 1e4).round() / 100.0,
        });
    }
    if out.groups.is_empty() {
        out.notes.push(format!("no footings overlap or stand closer than {gap_min} mm, and the coverage {cov}% is within {thr}%: nothing to merge"));
    }
    out
}

/// One undoable step: the tie beams of the merged footings and the footings are deleted, the combined footing or raft is created.
pub fn apply(model: &Model, plan: &MergePlan) -> Vec<Command> {
    let mut cmds = Vec::new();
    for g in &plan.groups {
        let ids: Vec<&String> = g.footing_ids.iter().collect();
        for e in model.elements.values().filter(|e| e.kind == "TieBeam") {
            let hit = |k: &str| e.props.get(k).and_then(|v| v.as_str()).is_some_and(|s| ids.iter().any(|i| i.as_str() == s));
            if hit("from") || hit("to") {
                cmds.push(Command::Delete(e.id));
            }
        }
        let level = g.footing_ids.first().and_then(|i| i.parse::<Id>().ok()).and_then(|i| model.get(&i)).and_then(|e| e.level_id);
        for i in &ids {
            if let Ok(id) = i.parse::<Id>() {
                cmds.push(Command::Delete(id));
            }
        }
        let mut extra = serde_json::Map::new();
        extra.insert("label".into(), json!(g.label));
        extra.insert("merged".into(), json!(g.kind));
        extra.insert("columns".into(), json!(g.columns));
        extra.insert("mergedFrom".into(), json!(g.footings));
        let f = Footing { x: (g.rect[0] + g.rect[2]) / 2, y: (g.rect[1] + g.rect[3]) / 2, length: g.length_mm, width: g.width_mm, service_load_kn: g.load_kn, bearing_kpa: g.bearing_kpa, column: g.columns.first().cloned().unwrap_or_default(), extra };
        let mut el = element_of(&f, Source::User);
        el.level_id = level;
        cmds.push(Command::Create(el));
    }
    cmds
}
