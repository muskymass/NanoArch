//! Smart-home infrastructure: control points (lighting, HVAC zones, shading, locks, sensors, switches), a central panel and the
//! low-voltage bus that joins them.
//!
//! - The devices and the panel are inputs stored in the project; `suggest` proposes a starting set from the rooms and openings (one
//!   lighting actuator and one sensor per room, a thermostat in each conditioned room, one shading motor per window, a lock on the
//!   entrance door): it is a rule on the model, not a design, and you edit the list.
//! - **KNX** is a wired bus: the devices are chained from the panel by nearest neighbour into lines, a new line starting when the
//!   devices per line or the line length you entered would be exceeded; each line becomes cable runs (`MepRun`, system `cable`) at the
//!   bus height you enter. **Matter** and **Zigbee** are wireless: no bus is routed, the panel is the hub. A device on another level
//!   than the panel is not routed (no riser is generated).
//! - No product limit is built in: lines per device count and per length come from your inputs.

use crate::mepx::{SmartDevice, SmartIn, r2};
use crate::model::{Id, Model};
use crate::routing::RunSpec;
use crate::typed::{Door, Space, Typed, Wall, Window};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
pub struct Line {
    pub n: usize,
    pub devices: Vec<String>,
    pub length_m: f64,
    /// The path from the panel through the devices, mm.
    pub points: Vec<[i64; 2]>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SmartReport {
    pub protocol: String,
    pub wired: bool,
    pub panel: Option<[i64; 2]>,
    pub devices: usize,
    pub by_kind: BTreeMap<String, usize>,
    pub lines: Vec<Line>,
    pub total_bus_m: f64,
    pub unplaced: Vec<String>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
    #[serde(skip)]
    pub runs: Vec<RunSpec>,
}

struct R {
    level: Option<Id>,
    key: String,
    func: String,
    centre: [i64; 2],
}

fn rooms(model: &Model) -> Vec<R> {
    let mut v: Vec<R> = model.elements.values().filter(|e| e.kind == Space::KIND).filter_map(|e| Some((e.level_id, e.typed::<Space>().ok()?))).map(|(level, s)| R { level, key: s.requirement_key, func: s.func, centre: [s.x + s.w / 2, s.y + s.h / 2] }).collect();
    v.sort_by(|a, b| a.key.cmp(&b.key));
    v
}

/// The point of a door or window on its wall's centre line.
fn opening_point(model: &Model, e: &crate::model::Element, offset: i64) -> Option<[i64; 2]> {
    let w = model.get(&e.host_id?)?.typed::<Wall>().ok()?;
    let len = ((w.x2 - w.x1) as f64).hypot((w.y2 - w.y1) as f64).max(1.0);
    let t = offset as f64 / len;
    Some([(w.x1 as f64 + (w.x2 - w.x1) as f64 * t).round() as i64, (w.y1 as f64 + (w.y2 - w.y1) as f64 * t).round() as i64])
}

/// A starting set of devices from the rooms and openings.
pub fn suggest(model: &Model) -> Vec<SmartDevice> {
    let rs = rooms(model);
    let mut out = Vec::new();
    let mut dev = |name: String, kind: &str, room: &str, at: Option<[i64; 2]>| out.push(SmartDevice { name, kind: kind.into(), room: room.into(), x_mm: at.map(|p| p[0] as f64), y_mm: at.map(|p| p[1] as f64) });
    for r in &rs {
        dev(format!("Light {}", r.key), "lighting", &r.key, None);
        dev(format!("Sensor {}", r.key), "sensor", &r.key, None);
        if !["bathroom", "toilet", "stair", "hall", "store"].contains(&r.func.as_str()) {
            dev(format!("Thermostat {}", r.key), "hvac", &r.key, None);
        }
    }
    let mut n = 0;
    let mut wins: Vec<(&crate::model::Element, Window)> = model.elements.values().filter(|e| e.kind == Window::KIND).filter_map(|e| Some((e, e.typed::<Window>().ok()?))).collect();
    wins.sort_by_key(|a| (a.1.room.clone(), a.1.offset_center));
    for (e, w) in &wins {
        n += 1;
        dev(format!("Shading {} #{n}", w.room), "shading", &w.room, opening_point(model, e, w.offset_center));
    }
    for e in model.elements.values().filter(|e| e.kind == Door::KIND) {
        if let Ok(d) = e.typed::<Door>()
            && d.is_egress
        {
            dev("Lock entrance".into(), "lock", &d.from, opening_point(model, e, d.offset_center));
        }
    }
    out
}

fn manhattan(a: [i64; 2], b: [i64; 2]) -> f64 {
    ((a[0] - b[0]).abs() + (a[1] - b[1]).abs()) as f64
}

pub fn topology(model: &Model, inp: &SmartIn) -> SmartReport {
    let mut rep = SmartReport { protocol: inp.protocol.clone(), wired: inp.protocol == "knx", devices: inp.devices.len(), ..Default::default() };
    for d in &inp.devices {
        *rep.by_kind.entry(d.kind.clone()).or_default() += 1;
    }
    if inp.protocol.is_empty() {
        rep.missing.push("protocol (knx, matter or zigbee)".into());
    }
    let rs = rooms(model);
    let Some(panel_room) = inp.panel_room.as_deref().and_then(|k| rs.iter().find(|r| r.key == k)) else {
        rep.missing.push("panel room (a room of the plan)".into());
        return rep;
    };
    rep.panel = Some(panel_room.centre);
    // Resolve every device to a point and a level.
    let mut placed: Vec<(&SmartDevice, [i64; 2])> = Vec::new();
    for d in &inp.devices {
        let room = rs.iter().find(|r| r.key == d.room);
        let at = match (d.x_mm, d.y_mm, room) {
            (Some(x), Some(y), _) => Some([x.round() as i64, y.round() as i64]),
            (_, _, Some(r)) => Some(r.centre),
            _ => None,
        };
        match (at, room) {
            (Some(p), Some(r)) if r.level == panel_room.level => placed.push((d, p)),
            (Some(_), Some(_)) => {
                rep.unplaced.push(d.name.clone());
                rep.notes.push(format!("'{}' is on another level than the panel: no bus is routed to it (no riser is generated)", d.name));
            }
            _ => {
                rep.unplaced.push(d.name.clone());
                rep.notes.push(format!("'{}': room '{}' is not in the plan", d.name, d.room));
            }
        }
    }
    if !rep.wired {
        if !inp.protocol.is_empty() {
            rep.notes.push(format!("{} is wireless: no bus is routed; the panel is the hub and every device needs power and radio range", inp.protocol));
        }
        return rep;
    }
    let (Some(maxd), Some(maxl), Some(rf), Some(z), Some(bus)) = (inp.max_devices_per_line, inp.max_line_m, inp.route_factor, inp.bus_z_mm, inp.bus_mm) else {
        for (n, ok) in [("devices per line", inp.max_devices_per_line.is_some()), ("line length (m)", inp.max_line_m.is_some()), ("route factor", inp.route_factor.is_some()), ("bus height (mm)", inp.bus_z_mm.is_some()), ("bus section (mm)", inp.bus_mm.is_some())] {
            if !ok {
                rep.missing.push(n.to_string());
            }
        }
        return rep;
    };
    let mut left: Vec<(&SmartDevice, [i64; 2])> = placed;
    let panel = panel_room.centre;
    while !left.is_empty() {
        let mut line = Line { n: rep.lines.len() + 1, devices: Vec::new(), length_m: 0.0, points: vec![panel] };
        let mut pos = panel;
        let mut len = 0.0;
        loop {
            let next = left.iter().enumerate().min_by(|a, b| manhattan(pos, a.1.1).total_cmp(&manhattan(pos, b.1.1))).map(|(i, _)| i);
            let Some(i) = next else { break };
            let step = manhattan(pos, left[i].1) * rf / 1000.0;
            if (line.devices.len() as f64 + 1.0 > maxd || len + step > maxl) && !line.devices.is_empty() {
                break;
            }
            let (d, p) = left.remove(i);
            len += step;
            line.devices.push(d.name.clone());
            line.points.push(p);
            pos = p;
        }
        line.length_m = r2(len);
        // Runs: x first, then y, between consecutive points.
        for w in line.points.windows(2) {
            let (a, b) = (w[0], w[1]);
            let corner = [b[0], a[1]];
            for (p, q) in [(a, corner), (corner, b)] {
                if p != q {
                    rep.runs.push(RunSpec { level: panel_room.level, system: "cable".into(), role: "smart-bus".into(), room: None, label: Some(format!("KNX line {}", line.n)), p1: [p[0], p[1], z.round() as i64], p2: [q[0], q[1], z.round() as i64], width: bus.round() as i64, height: bus.round() as i64 });
                }
            }
        }
        rep.total_bus_m += len;
        if line.length_m > maxl {
            rep.notes.push(format!("line {}: a single device is {:.1} m from the panel, beyond the {maxl} m limit", line.n, line.length_m));
        }
        rep.lines.push(line);
    }
    rep.total_bus_m = r2(rep.total_bus_m);
    rep.notes.push("a bus line also needs its power supply and couplers: they are not counted here".into());
    rep
}
