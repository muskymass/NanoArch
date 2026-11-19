//! Vertical circulation and extra levels, stored as inputs in the project (`meta.vertical`): an elevator shaft, basement levels
//! with a car ramp, and a roof annex (الملحق العلوي).
//!
//! Every dimension is an input: the application has no statutory or manufacturer figure for a car, a clearance, a pit, an overhead
//! or a ramp slope, and a missing one is reported by name. The shaft is the car plus the clearances plus the walls; it is placed
//! where the user says and checked against the building. Basements and the annex are massing volumes with their area and volume,
//! not full structural levels. The ramp is drawn as steps.

use crate::model3d::{Box3, Scene};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ElevatorIn {
    pub car_width_mm: Option<f64>,
    pub car_depth_mm: Option<f64>,
    pub clear_side_mm: Option<f64>,
    pub clear_back_mm: Option<f64>,
    pub clear_front_mm: Option<f64>,
    pub wall_mm: Option<f64>,
    pub pit_mm: Option<f64>,
    pub overhead_mm: Option<f64>,
    /// South-west corner of the shaft's outer face, in the model frame.
    pub x_mm: Option<f64>,
    pub y_mm: Option<f64>,
    /// With a machine room above the shaft (otherwise machine-room-less); its height is then required.
    pub machine_room: bool,
    pub machine_room_height_mm: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RampIn {
    pub width_mm: Option<f64>,
    pub run_mm: Option<f64>,
    /// South-west corner of the ramp's footprint (it runs along +y, descending away from the ground).
    pub x_mm: Option<f64>,
    pub y_mm: Option<f64>,
    /// The limit to check against, with the source you took it from.
    pub max_slope_pct: Option<f64>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BasementIn {
    pub levels: u32,
    pub height_mm: Option<f64>,
    pub ramp: Option<RampIn>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AnnexIn {
    pub x_mm: Option<f64>,
    pub y_mm: Option<f64>,
    pub width_mm: Option<f64>,
    pub length_mm: Option<f64>,
    pub height_mm: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VerticalInput {
    pub elevator: Option<ElevatorIn>,
    pub basement: Option<BasementIn>,
    pub roof_annex: Option<AnnexIn>,
}

impl VerticalInput {
    /// What is wrong with the numbers themselves (not what is missing: that is named when the report is made).
    pub fn check(&self) -> Result<(), String> {
        let bad = |n: &str, v: Option<f64>, allow_zero: bool| match v {
            Some(x) if !x.is_finite() || x < 0.0 || (!allow_zero && x == 0.0) => Err(format!("{n} must be a {} number of millimetres", if allow_zero { "zero or positive" } else { "positive" })),
            _ => Ok(()),
        };
        if let Some(e) = &self.elevator {
            for (n, v) in [("car width", e.car_width_mm), ("car depth", e.car_depth_mm), ("wall thickness", e.wall_mm)] {
                bad(n, v, false)?;
            }
            for (n, v) in [("side clearance", e.clear_side_mm), ("back clearance", e.clear_back_mm), ("front clearance", e.clear_front_mm), ("pit", e.pit_mm), ("overhead", e.overhead_mm), ("shaft x", e.x_mm), ("shaft y", e.y_mm)] {
                bad(n, v, true)?;
            }
            bad("machine room height", e.machine_room_height_mm, false)?;
        }
        if let Some(b) = &self.basement {
            if b.levels > 5 {
                return Err("more than 5 basement levels is not supported".into());
            }
            bad("basement height", b.height_mm, false)?;
            if let Some(r) = &b.ramp {
                for (n, v) in [("ramp width", r.width_mm), ("ramp run", r.run_mm), ("ramp slope limit", r.max_slope_pct)] {
                    bad(n, v, false)?;
                }
                bad("ramp x", r.x_mm, true)?;
                bad("ramp y", r.y_mm, true)?;
            }
        }
        if let Some(a) = &self.roof_annex {
            for (n, v) in [("annex width", a.width_mm), ("annex length", a.length_mm), ("annex height", a.height_mm)] {
                bad(n, v, false)?;
            }
            bad("annex x", a.x_mm, true)?;
            bad("annex y", a.y_mm, true)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ShaftReport {
    pub inner_mm: [f64; 2],
    pub outer_mm: [f64; 2],
    pub outer_area_m2: f64,
    pub rect_m: [f64; 4],
    /// Pit floor to the top of the overhead, metres.
    pub total_height_m: f64,
    pub stops: usize,
    pub machine_room: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RampReport {
    pub rise_mm: f64,
    pub slope_pct: f64,
    pub max_slope_pct: Option<f64>,
    pub source: Option<String>,
    pub ok: Option<bool>,
    pub min_run_mm: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerticalReport {
    pub shaft: Option<ShaftReport>,
    /// Basement level names, `LVL-B1` and down.
    pub basement_levels: Vec<String>,
    pub basement_area_m2: f64,
    pub basement_volume_m3: f64,
    pub ramp: Option<RampReport>,
    pub annex_area_m2: f64,
    pub annex_volume_m3: f64,
    pub problems: Vec<String>,
}

struct Ext {
    rect: [f64; 4],
    roof_top: f64,
    stops: usize,
}

fn extents(scene: &Scene) -> Result<Ext, String> {
    let mut r: Option<[f64; 4]> = None;
    for b in scene.boxes.iter().filter(|b| b.kind == "Space") {
        r = Some(r.map_or([b.min[0], b.min[1], b.max[0], b.max[1]], |p| [p[0].min(b.min[0]), p[1].min(b.min[1]), p[2].max(b.max[0]), p[3].max(b.max[1])]));
    }
    let rect = r.ok_or("the model has no rooms")?;
    let roof_top = scene.boxes.iter().filter(|b| b.kind == "Slab").map(|b| b.max[2]).fold(f64::MIN, f64::max);
    if !roof_top.is_finite() {
        return Err("the model has no slabs".into());
    }
    Ok(Ext { rect, roof_top, stops: scene.levels.len().max(1) })
}

macro_rules! need {
    ($p:expr, $v:expr, $n:expr) => {
        match $v {
            Some(x) => x,
            None => {
                $p.push(format!("{} is missing", $n));
                f64::NAN
            }
        }
    };
}

pub fn report(scene: &Scene, v: &VerticalInput) -> Result<VerticalReport, String> {
    v.check()?;
    let ext = extents(scene)?;
    let mut problems = Vec::new();
    let mut shaft = None;
    if let Some(e) = &v.elevator {
        let mut miss = Vec::new();
        let (cw, cd, cs, cb, cf, wall, pit, oh, x, y) = (
            need!(miss, e.car_width_mm, "elevator car width"),
            need!(miss, e.car_depth_mm, "elevator car depth"),
            need!(miss, e.clear_side_mm, "elevator side clearance"),
            need!(miss, e.clear_back_mm, "elevator back clearance"),
            need!(miss, e.clear_front_mm, "elevator front clearance"),
            need!(miss, e.wall_mm, "elevator wall thickness"),
            need!(miss, e.pit_mm, "elevator pit depth"),
            need!(miss, e.overhead_mm, "elevator overhead"),
            need!(miss, e.x_mm, "elevator x"),
            need!(miss, e.y_mm, "elevator y"),
        );
        if e.machine_room && e.machine_room_height_mm.is_none() {
            miss.push("machine room height is missing".to_string());
        }
        if miss.is_empty() {
            let inner = [cw + 2.0 * cs, cd + cb + cf];
            let outer = [inner[0] + 2.0 * wall, inner[1] + 2.0 * wall];
            let rect = [x / 1000.0, y / 1000.0, (x + outer[0]) / 1000.0, (y + outer[1]) / 1000.0];
            if rect[0] < ext.rect[0] - 1e-6 || rect[1] < ext.rect[1] - 1e-6 || rect[2] > ext.rect[2] + 1e-6 || rect[3] > ext.rect[3] + 1e-6 {
                problems.push(format!("the elevator shaft ({:.2} x {:.2} m) at ({:.2}, {:.2}) is not inside the building", outer[0] / 1000.0, outer[1] / 1000.0, x / 1000.0, y / 1000.0));
            }
            shaft = Some(ShaftReport { inner_mm: inner, outer_mm: outer, outer_area_m2: (outer[0] * outer[1] / 1e6 * 100.0).round() / 100.0, rect_m: rect, total_height_m: ext.roof_top + (pit + oh) / 1000.0, stops: ext.stops, machine_room: e.machine_room });
        } else {
            problems.extend(miss);
        }
    }
    let (mut b_area, mut b_vol, mut names, mut ramp) = (0.0, 0.0, Vec::new(), None);
    if let Some(b) = v.basement.as_ref().filter(|b| b.levels > 0) {
        let mut miss = Vec::new();
        let h = need!(miss, b.height_mm, "basement height");
        problems.extend(miss);
        if h.is_finite() {
            names = (1..=b.levels).map(|i| format!("LVL-B{i}")).collect();
            let area = (ext.rect[2] - ext.rect[0]) * (ext.rect[3] - ext.rect[1]);
            b_area = (area * b.levels as f64 * 100.0).round() / 100.0;
            b_vol = (area * b.levels as f64 * h / 1000.0 * 100.0).round() / 100.0;
            if let Some(r) = &b.ramp {
                let rise = h * b.levels as f64;
                match r.run_mm {
                    Some(run) => {
                        let slope = rise / run * 100.0;
                        ramp = Some(RampReport { rise_mm: rise, slope_pct: (slope * 100.0).round() / 100.0, max_slope_pct: r.max_slope_pct, source: r.source.clone(), ok: r.max_slope_pct.map(|m| slope <= m + 1e-9), min_run_mm: r.max_slope_pct.map(|m| (rise / (m / 100.0)).ceil()) });
                        if r.max_slope_pct.is_none() {
                            problems.push("the ramp slope limit is missing: the slope is reported but not judged".into());
                        }
                    }
                    None => problems.push("ramp run is missing".into()),
                }
            }
        }
    } else if v.basement.is_some() {
        problems.push("the number of basement levels is zero: no basement".into());
    }
    let (mut a_area, mut a_vol) = (0.0, 0.0);
    if let Some(a) = &v.roof_annex {
        let mut miss = Vec::new();
        let (w, l, h) = (need!(miss, a.width_mm, "annex width"), need!(miss, a.length_mm, "annex length"), need!(miss, a.height_mm, "annex height"));
        let (x, y) = (need!(miss, a.x_mm, "annex x"), need!(miss, a.y_mm, "annex y"));
        if miss.is_empty() {
            let r = [x / 1000.0, y / 1000.0, (x + w) / 1000.0, (y + l) / 1000.0];
            if r[0] < ext.rect[0] - 1e-6 || r[1] < ext.rect[1] - 1e-6 || r[2] > ext.rect[2] + 1e-6 || r[3] > ext.rect[3] + 1e-6 {
                problems.push("the roof annex is not inside the building outline".into());
            }
            a_area = (w * l / 1e6 * 100.0).round() / 100.0;
            a_vol = (w * l * h / 1e9 * 100.0).round() / 100.0;
        } else {
            problems.extend(miss);
        }
    }
    Ok(VerticalReport { shaft, basement_levels: names, basement_area_m2: b_area, basement_volume_m3: b_vol, ramp, annex_area_m2: a_area, annex_volume_m3: a_vol, problems })
}

/// 3D solids: the shaft from the pit to above the roof, the machine room, basement volumes, the ramp as steps, the roof annex.
pub fn boxes(scene: &Scene, v: &VerticalInput) -> Vec<Box3> {
    let Ok(rep) = report(scene, v) else { return Vec::new() };
    let Ok(ext) = extents(scene) else { return Vec::new() };
    let mut out = Vec::new();
    let mut push = |class: &str, min: [f64; 3], max: [f64; 3]| {
        out.push(Box3 { id: format!("vert:{}", out.len() + 1), kind: "Vertical", class: class.to_string(), min, max, visual: true, level: None, discipline: "architectural" });
    };
    if let (Some(s), Some(e)) = (&rep.shaft, &v.elevator) {
        let (pit, oh) = (e.pit_mm.unwrap_or(0.0) / 1000.0, e.overhead_mm.unwrap_or(0.0) / 1000.0);
        push("elevator", [s.rect_m[0], s.rect_m[1], -pit], [s.rect_m[2], s.rect_m[3], ext.roof_top + oh]);
        if let (true, Some(h)) = (e.machine_room, e.machine_room_height_mm) {
            push("machine", [s.rect_m[0], s.rect_m[1], ext.roof_top + oh], [s.rect_m[2], s.rect_m[3], ext.roof_top + oh + h / 1000.0]);
        }
    }
    if let Some(b) = v.basement.as_ref().filter(|_| !rep.basement_levels.is_empty()) {
        let h = b.height_mm.unwrap_or(0.0) / 1000.0;
        for i in 0..b.levels {
            push("basement", [ext.rect[0], ext.rect[1], -h * (i + 1) as f64], [ext.rect[2], ext.rect[3], -h * i as f64]);
        }
        if let Some(r) = &b.ramp
            && let (Some(w), Some(run), Some(x), Some(y)) = (r.width_mm, r.run_mm, r.x_mm, r.y_mm)
        {
            let (w, run, x, y, total) = (w / 1000.0, run / 1000.0, x / 1000.0, y / 1000.0, h * b.levels as f64);
            let steps = 12;
            for k in 0..steps {
                let (y0, y1) = (y + run * k as f64 / steps as f64, y + run * (k + 1) as f64 / steps as f64);
                let z = -total * (k + 1) as f64 / steps as f64;
                push("ramp", [x, y0, z - 0.2], [x + w, y1, z]);
            }
        }
    }
    if let (Some(a), true) = (&v.roof_annex, rep.problems.iter().all(|p| !p.contains("annex")))
        && let (Some(x), Some(y), Some(w), Some(l), Some(h)) = (a.x_mm, a.y_mm, a.width_mm, a.length_mm, a.height_mm)
    {
        push("annex", [x / 1000.0, y / 1000.0, ext.roof_top], [(x + w) / 1000.0, (y + l) / 1000.0, ext.roof_top + h / 1000.0]);
    }
    out
}
