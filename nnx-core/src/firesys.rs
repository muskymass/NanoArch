//! Fire detection / alarm and sprinkler layout, call-point reach and sprinkler water demand.
//!
//! No value in this module was read from a fire code, a listing or a manufacturer datasheet in this project. Every device's
//! coverage area, spacing limit and source, the call-point travel limit, the zone limits and the sprinkler design density, design
//! area and duration are the caller's inputs. A missing required one is refused by name; a missing optional limit leaves its check
//! `unchecked`, never `ok`. Failure outranks `unchecked`.
//!
//! Layout. For a rectangular room the devices sit at the centres of `rows x cols` equal cells. Spacing between devices is the cell
//! size, so `cols >= ceil(w / max_spacing)`, `rows >= ceil(h / max_spacing)` and `rows * cols >= area / coverage`. The smallest
//! count meeting all three is chosen (ties go to the squarer cell). The distance from a device to the nearest wall is half the cell
//! size, the larger of the two directions is reported and compared with the optional `max_wall_distance_mm`.
//!
//! Call points are placed at the exits the caller lists. The reach check is the straight-line distance from the farthest corner of
//! every room to the nearest call point, which is a lower bound of the real travel path; it does not follow corridors or doors.
//!
//! Water demand: `flow = density * design_area` (1 mm/min over 1 m2 = 1 L/min), plus the hose allowance if the caller gives one,
//! and `storage = flow * duration`. Hydraulic calculation of the sprinkler pipework, pumps, detector sensitivity, alarm audibility,
//! cause-and-effect and battery sizing are not computed. This is a preliminary layout screen and it does not replace a fire
//! engineer's design or the approving authority.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Largest number of devices one room may need before the input is refused as implausible (a unit slip such as m2 for mm2).
const MAX_DEVICES_PER_ROOM: u32 = 10_000;

/// A detector or sprinkler type. Every figure is the caller's, from a datasheet or code they name in `source`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Device {
    pub key: String,
    /// Largest floor area one device may cover, m2.
    #[serde(default)]
    pub coverage_m2: Option<f64>,
    /// Largest distance between neighbouring devices, mm.
    #[serde(default)]
    pub max_spacing_mm: Option<f64>,
    /// Largest distance from a device to a wall, mm. Absent: the wall-distance check is `unchecked`.
    #[serde(default)]
    pub max_wall_distance_mm: Option<f64>,
    /// Highest ceiling the device is rated for, mm. Absent: the height check is `unchecked`.
    #[serde(default)]
    pub max_ceiling_height_mm: Option<f64>,
    /// Where the figures came from (datasheet, code, edition, table); required.
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FireRoom {
    pub key: String,
    pub x_mm: f64,
    pub y_mm: f64,
    pub w_mm: f64,
    pub h_mm: f64,
    /// Key of the detector type protecting this room. Absent: no detection is laid out.
    #[serde(default)]
    pub detector: Option<String>,
    /// Key of the sprinkler type protecting this room. Absent: no sprinklers are laid out.
    #[serde(default)]
    pub sprinkler: Option<String>,
    /// Detection zone the room belongs to (needed for the zone limits).
    #[serde(default)]
    pub zone: Option<String>,
    /// Ceiling height, mm. Absent: the device height check is `unchecked`.
    #[serde(default)]
    pub ceiling_height_mm: Option<f64>,
}

/// An exit; a manual call point is placed at each one.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Exit {
    pub key: String,
    pub x_mm: f64,
    pub y_mm: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WaterDemand {
    /// Design density, mm/min (L/min per m2).
    #[serde(default)]
    pub design_density_mm_min: Option<f64>,
    /// Design operating area, m2.
    #[serde(default)]
    pub design_area_m2: Option<f64>,
    /// Design duration, minutes.
    #[serde(default)]
    pub duration_min: Option<f64>,
    /// Where the density, area and duration came from; required.
    #[serde(default)]
    pub source: Option<String>,
    /// Hose allowance added to the sprinkler flow, L/min. Absent: not included, and the report says so (not taken as 0 silently).
    #[serde(default)]
    pub hose_allowance_lpm: Option<f64>,
    /// Water the project actually stores for fire, m3. Absent: the storage check is `unchecked`.
    #[serde(default)]
    pub available_storage_m3: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct FireInput {
    pub rooms: Vec<FireRoom>,
    #[serde(default)]
    pub detectors: Vec<Device>,
    #[serde(default)]
    pub sprinklers: Vec<Device>,
    #[serde(default)]
    pub exits: Vec<Exit>,
    /// Largest straight-line distance from any room's farthest corner to a call point, mm. Absent: `unchecked`.
    #[serde(default)]
    pub max_call_point_distance_mm: Option<f64>,
    /// Largest detector count in one zone. Absent: that check is `unchecked`.
    #[serde(default)]
    pub max_detectors_per_zone: Option<u32>,
    /// Largest floor area of one zone, m2. Absent: that check is `unchecked`.
    #[serde(default)]
    pub max_zone_area_m2: Option<f64>,
    #[serde(default)]
    pub water: Option<WaterDemand>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Point {
    pub x_mm: f64,
    pub y_mm: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Placement {
    pub device: String,
    pub source: String,
    pub count: u32,
    pub rows: u32,
    pub cols: u32,
    pub spacing_x_mm: f64,
    pub spacing_y_mm: f64,
    /// Largest distance from a device to a wall, mm (half the larger cell side).
    pub wall_distance_mm: f64,
    /// Floor area per device, m2.
    pub area_per_device_m2: f64,
    pub points: Vec<Point>,
    pub wall_status: &'static str,
    pub height_status: &'static str,
    pub status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FireRow {
    pub key: String,
    pub area_m2: f64,
    pub detection: Option<Placement>,
    pub sprinklers: Option<Placement>,
    pub status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reach {
    pub room: String,
    /// Straight-line distance from the room's farthest corner to the nearest call point, mm.
    pub distance_mm: f64,
    pub nearest_exit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CallPoints {
    pub points: Vec<Point>,
    pub keys: Vec<String>,
    pub reach: Vec<Reach>,
    pub worst_distance_mm: Option<f64>,
    pub status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ZoneRow {
    pub zone: String,
    pub rooms: Vec<String>,
    pub detectors: u32,
    pub area_m2: f64,
    pub status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WaterReport {
    pub source: String,
    pub density_mm_min: f64,
    pub design_area_m2: f64,
    pub duration_min: f64,
    pub sprinkler_flow_lpm: f64,
    pub hose_allowance_lpm: Option<f64>,
    pub total_flow_lpm: f64,
    pub required_storage_m3: f64,
    pub available_storage_m3: Option<f64>,
    /// Total floor area that has sprinklers, m2. The design area cannot exceed it.
    pub sprinklered_area_m2: f64,
    pub status: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct FireCounts {
    pub ok: u32,
    pub fails: u32,
    pub unchecked: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub ar: String,
    pub en: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FireReport {
    pub rooms: Vec<FireRow>,
    pub total_detectors: u32,
    pub total_sprinklers: u32,
    pub call_points: CallPoints,
    pub zones: Vec<ZoneRow>,
    pub zone_status: &'static str,
    pub water: Option<WaterReport>,
    pub counts: FireCounts,
    pub findings: Vec<Finding>,
}

fn positive(v: f64, field: &str, key: &str) -> Result<f64, String> {
    if v.is_finite() && v > 0.0 {
        Ok(v)
    } else {
        Err(format!("'{key}': {field} must be a positive number, got {v}"))
    }
}

fn required(v: Option<f64>, field: &str, key: &str) -> Result<f64, String> {
    let v = v.ok_or_else(|| format!("'{key}': {field} is required and has no default"))?;
    positive(v, field, key)
}

fn worst(statuses: &[&'static str]) -> &'static str {
    if statuses.contains(&"fails") {
        "fails"
    } else if statuses.contains(&"unchecked") {
        "unchecked"
    } else {
        "ok"
    }
}

fn limit_status(value: Option<f64>, limit: Option<f64>) -> &'static str {
    match (value, limit) {
        (Some(v), Some(l)) if v <= l + 1e-9 => "ok",
        (Some(_), Some(_)) => "fails",
        _ => "unchecked",
    }
}

fn check_keys<'a>(keys: impl Iterator<Item = &'a str>, what: &str) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for k in keys {
        if k.trim().is_empty() {
            return Err(format!("a {what} has an empty key"));
        }
        if !seen.insert(k) {
            return Err(format!("{what} key '{k}' is used twice"));
        }
    }
    Ok(())
}

/// Smallest `rows x cols` whose cells respect the spacing and coverage limits: fewest cells, then the squarest cell.
fn layout(w: f64, h: f64, max_spacing: f64, coverage_m2: f64, key: &str) -> Result<(u32, u32), String> {
    let cmin = ((w / max_spacing) - 1e-9).ceil().max(1.0);
    let rmin = ((h / max_spacing) - 1e-9).ceil().max(1.0);
    let need = ((w * h / 1.0e6 / coverage_m2) - 1e-9).ceil().max(1.0);
    let n0 = (cmin * rmin).max(need);
    if n0 > f64::from(MAX_DEVICES_PER_ROOM) {
        return Err(format!(
            "room '{key}': the limits ask for more than {MAX_DEVICES_PER_ROOM} devices; check the units of coverage_m2, max_spacing_mm and the room size"
        ));
    }
    let n0 = n0 as u32;
    let (cmin, rmin) = (cmin as u32, rmin as u32);
    let mut best: Option<(u32, u32, f64)> = None;
    for cols in cmin..=n0.max(cmin) {
        let rows = rmin.max(n0.div_ceil(cols));
        let cells = rows * cols;
        let skew = (w / f64::from(cols) - h / f64::from(rows)).abs();
        let better = match best {
            None => true,
            Some((br, bc, bs)) => cells < br * bc || (cells == br * bc && skew < bs - 1e-9),
        };
        if better {
            best = Some((rows, cols, skew));
        }
    }
    best.map(|(r, c, _)| (r, c)).ok_or_else(|| format!("room '{key}': no layout found"))
}

fn device_ready<'a>(list: &'a [Device], key: &str, room: &str, what: &str) -> Result<(&'a Device, f64, f64, String), String> {
    let d = list
        .iter()
        .find(|d| d.key == key)
        .ok_or_else(|| format!("room '{room}': {what} '{key}' is not in the {what} list"))?;
    let cov = required(d.coverage_m2, "coverage_m2", &d.key)?;
    let sp = required(d.max_spacing_mm, "max_spacing_mm", &d.key)?;
    let src = d
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("'{}': source is required (datasheet, code, edition, table the figures came from)", d.key))?;
    Ok((d, cov, sp, src.to_string()))
}

fn place(r: &FireRoom, d: &Device, cov: f64, sp: f64, src: String, ceiling: Option<f64>) -> Result<Placement, String> {
    let (rows, cols) = layout(r.w_mm, r.h_mm, sp, cov, &r.key)?;
    let sx = r.w_mm / f64::from(cols);
    let sy = r.h_mm / f64::from(rows);
    let mut points = Vec::with_capacity((rows * cols) as usize);
    for j in 0..rows {
        for i in 0..cols {
            points.push(Point { x_mm: r.x_mm + (f64::from(i) + 0.5) * sx, y_mm: r.y_mm + (f64::from(j) + 0.5) * sy });
        }
    }
    let wall_distance_mm = sx.max(sy) / 2.0;
    let wall_status = limit_status(Some(wall_distance_mm), d.max_wall_distance_mm);
    let height_status = limit_status(ceiling, d.max_ceiling_height_mm);
    Ok(Placement {
        device: d.key.clone(),
        source: src,
        count: rows * cols,
        rows,
        cols,
        spacing_x_mm: sx,
        spacing_y_mm: sy,
        wall_distance_mm,
        area_per_device_m2: sx * sy / 1.0e6,
        points,
        wall_status,
        height_status,
        status: worst(&[wall_status, height_status]),
    })
}

fn row(r: &FireRoom, inp: &FireInput) -> Result<FireRow, String> {
    positive(r.w_mm, "w_mm", &r.key)?;
    positive(r.h_mm, "h_mm", &r.key)?;
    if !r.x_mm.is_finite() || !r.y_mm.is_finite() {
        return Err(format!("'{}': x_mm and y_mm must be finite", r.key));
    }
    let ceiling = r.ceiling_height_mm.map(|v| positive(v, "ceiling_height_mm", &r.key)).transpose()?;
    let detection = match r.detector.as_deref() {
        Some(k) => {
            let (d, cov, sp, src) = device_ready(&inp.detectors, k, &r.key, "detector")?;
            Some(place(r, d, cov, sp, src, ceiling)?)
        }
        None => None,
    };
    let sprinklers = match r.sprinkler.as_deref() {
        Some(k) => {
            let (d, cov, sp, src) = device_ready(&inp.sprinklers, k, &r.key, "sprinkler")?;
            Some(place(r, d, cov, sp, src, ceiling)?)
        }
        None => None,
    };
    let mut parts: Vec<&'static str> = detection.iter().chain(sprinklers.iter()).map(|p| p.status).collect();
    if parts.is_empty() {
        // Nothing is laid out here; that is not a pass.
        parts.push("unchecked");
    }
    Ok(FireRow { key: r.key.clone(), area_m2: r.w_mm * r.h_mm / 1.0e6, detection, sprinklers, status: worst(&parts) })
}

fn call_points(inp: &FireInput) -> Result<CallPoints, String> {
    for e in &inp.exits {
        if !e.x_mm.is_finite() || !e.y_mm.is_finite() {
            return Err(format!("exit '{}': x_mm and y_mm must be finite", e.key));
        }
    }
    let mut reach = Vec::new();
    if !inp.exits.is_empty() {
        for r in &inp.rooms {
            let corners = [
                (r.x_mm, r.y_mm),
                (r.x_mm + r.w_mm, r.y_mm),
                (r.x_mm, r.y_mm + r.h_mm),
                (r.x_mm + r.w_mm, r.y_mm + r.h_mm),
            ];
            let mut far: Option<(f64, &str)> = None;
            for (cx, cy) in corners {
                let near = inp
                    .exits
                    .iter()
                    .map(|e| (((cx - e.x_mm).powi(2) + (cy - e.y_mm).powi(2)).sqrt(), e.key.as_str()))
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .filter(|(d, _)| far.is_none_or(|(fd, _)| *d > fd));
                if near.is_some() {
                    far = near;
                }
            }
            if let Some((d, k)) = far {
                reach.push(Reach { room: r.key.clone(), distance_mm: d, nearest_exit: k.to_string() });
            }
        }
    }
    let worst_distance_mm = reach.iter().map(|r| r.distance_mm).max_by(f64::total_cmp);
    let status = if inp.exits.is_empty() { "unchecked" } else { limit_status(worst_distance_mm, inp.max_call_point_distance_mm) };
    Ok(CallPoints {
        points: inp.exits.iter().map(|e| Point { x_mm: e.x_mm, y_mm: e.y_mm }).collect(),
        keys: inp.exits.iter().map(|e| e.key.clone()).collect(),
        reach,
        worst_distance_mm,
        status,
    })
}

fn zones(inp: &FireInput, rows: &[FireRow], findings: &mut Vec<Finding>) -> (Vec<ZoneRow>, &'static str) {
    let mut by_zone: BTreeMap<String, (Vec<String>, u32, f64)> = BTreeMap::new();
    let mut unzoned = Vec::new();
    for (r, fr) in inp.rooms.iter().zip(rows) {
        let Some(det) = &fr.detection else { continue };
        match r.zone.as_deref().map(str::trim).filter(|z| !z.is_empty()) {
            Some(z) => {
                let e = by_zone.entry(z.to_string()).or_default();
                e.0.push(r.key.clone());
                e.1 += det.count;
                e.2 += fr.area_m2;
            }
            None => unzoned.push(r.key.clone()),
        }
    }
    let zones: Vec<ZoneRow> = by_zone
        .into_iter()
        .map(|(zone, (rooms, detectors, area_m2))| {
            let status = worst(&[
                limit_status(Some(f64::from(detectors)), inp.max_detectors_per_zone.map(f64::from)),
                limit_status(Some(area_m2), inp.max_zone_area_m2),
            ]);
            ZoneRow { zone, rooms, detectors, area_m2, status }
        })
        .collect();
    let any_limit = inp.max_detectors_per_zone.is_some() || inp.max_zone_area_m2.is_some();
    if !any_limit {
        findings.push(Finding {
            ar: "لم تُدخل حدود المناطق (max_detectors_per_zone وmax_zone_area_m2) فلم يُحكم على تقسيم المناطق.".into(),
            en: "No zone limits (max_detectors_per_zone, max_zone_area_m2) given, so no pass or fail is declared for zoning.".into(),
        });
    }
    if any_limit && !unzoned.is_empty() {
        findings.push(Finding {
            ar: format!("غرف بكاشفات بلا منطقة (zone): {}؛ لم تُفحص حدود المناطق لها.", unzoned.join("، ")),
            en: format!("Rooms with detectors but no zone: {}; the zone limits cannot be checked for them.", unzoned.join(", ")),
        });
    }
    let status = if !any_limit || !unzoned.is_empty() || zones.is_empty() {
        // A failing zone still fails; only the all-clear is withheld when something was not checked.
        if zones.iter().any(|z| z.status == "fails") {
            "fails"
        } else {
            "unchecked"
        }
    } else {
        worst(&zones.iter().map(|z| z.status).collect::<Vec<_>>())
    };
    (zones, status)
}

fn water(inp: &FireInput, rows: &[FireRow], findings: &mut Vec<Finding>) -> Result<Option<WaterReport>, String> {
    let sprinklered_area_m2: f64 = rows.iter().filter(|r| r.sprinklers.is_some()).map(|r| r.area_m2).sum();
    let Some(w) = &inp.water else {
        if sprinklered_area_m2 > 0.0 {
            findings.push(Finding {
                ar: "توجد رشاشات ولم يُدخل قسم الطلب المائي (water) فلم يُحسب التدفق ولا التخزين.".into(),
                en: "Sprinklers are laid out but no water demand section was given, so flow and storage are not computed.".into(),
            });
        }
        return Ok(None);
    };
    let density = required(w.design_density_mm_min, "design_density_mm_min", "water")?;
    let area = required(w.design_area_m2, "design_area_m2", "water")?;
    let duration = required(w.duration_min, "duration_min", "water")?;
    let source = w
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("'water': source is required (code, edition, hazard class table the density, area and duration came from)")?;
    let hose = w.hose_allowance_lpm.map(|v| positive(v, "hose_allowance_lpm", "water")).transpose()?;
    let available = w.available_storage_m3.map(|v| positive(v, "available_storage_m3", "water")).transpose()?;
    let sprinkler_flow = density * area;
    let total_flow = sprinkler_flow + hose.unwrap_or(0.0);
    let required_storage = total_flow * duration / 1000.0;
    let area_fits = area <= sprinklered_area_m2 + 1e-9;
    let storage_status = match available {
        Some(a) if a + 1e-9 >= required_storage => "ok",
        Some(_) => "fails",
        None => "unchecked",
    };
    let status = if !area_fits { "fails" } else { storage_status };
    if !area_fits {
        findings.push(Finding {
            ar: format!(
                "مساحة التصميم {area:.1} م² أكبر من المساحة المرشوشة في المخطط {sprinklered_area_m2:.1} م²؛ الطلب المائي لا يطابق ما رُصّ."
            ),
            en: format!(
                "The design area of {area:.1} m2 is larger than the {sprinklered_area_m2:.1} m2 that has sprinklers in the layout; the water demand does not match what is laid out."
            ),
        });
    }
    if hose.is_none() {
        findings.push(Finding {
            ar: "لم يُدخل بدل خراطيم الحريق (hose_allowance_lpm) فلم يُضف إلى التدفق؛ وليس صفراً مفترضاً بل غير محسوب.".into(),
            en: "No hose_allowance_lpm given, so no hose flow is added; it is not computed, which is not the same as zero.".into(),
        });
    }
    if available.is_none() {
        findings.push(Finding {
            ar: "لم يُدخل التخزين المتاح للحريق (available_storage_m3) فلم يُحكم على كفايته.".into(),
            en: "No available_storage_m3 given, so no pass or fail is declared for the fire water storage.".into(),
        });
    }
    Ok(Some(WaterReport {
        source: source.to_string(),
        density_mm_min: density,
        design_area_m2: area,
        duration_min: duration,
        sprinkler_flow_lpm: sprinkler_flow,
        hose_allowance_lpm: hose,
        total_flow_lpm: total_flow,
        required_storage_m3: required_storage,
        available_storage_m3: available,
        sprinklered_area_m2,
        status,
    }))
}

pub fn check(inp: &FireInput) -> Result<FireReport, String> {
    if inp.rooms.is_empty() {
        return Err("nothing to protect: give at least one room".into());
    }
    check_keys(inp.rooms.iter().map(|r| r.key.as_str()), "room")?;
    check_keys(inp.detectors.iter().map(|d| d.key.as_str()), "detector")?;
    check_keys(inp.sprinklers.iter().map(|d| d.key.as_str()), "sprinkler")?;
    check_keys(inp.exits.iter().map(|e| e.key.as_str()), "exit")?;
    for d in inp.detectors.iter().chain(&inp.sprinklers) {
        for (v, f) in [(d.max_wall_distance_mm, "max_wall_distance_mm"), (d.max_ceiling_height_mm, "max_ceiling_height_mm")] {
            if let Some(v) = v {
                positive(v, f, &d.key)?;
            }
        }
    }
    if let Some(v) = inp.max_call_point_distance_mm {
        positive(v, "max_call_point_distance_mm", "input")?;
    }
    if let Some(v) = inp.max_zone_area_m2 {
        positive(v, "max_zone_area_m2", "input")?;
    }
    if inp.max_detectors_per_zone == Some(0) {
        return Err("'input': max_detectors_per_zone must be a positive number, got 0".into());
    }

    let rooms = inp.rooms.iter().map(|r| row(r, inp)).collect::<Result<Vec<_>, _>>()?;
    let mut findings = Vec::new();
    let call_points = call_points(inp)?;
    let (zones, zone_status) = zones(inp, &rooms, &mut findings);
    let water = water(inp, &rooms, &mut findings)?;

    let mut counts = FireCounts::default();
    for r in &rooms {
        match r.status {
            "ok" => counts.ok += 1,
            "fails" => counts.fails += 1,
            _ => counts.unchecked += 1,
        }
    }
    let total_detectors = rooms.iter().filter_map(|r| r.detection.as_ref()).map(|p| p.count).sum();
    let total_sprinklers = rooms.iter().filter_map(|r| r.sprinklers.as_ref()).map(|p| p.count).sum();

    if inp.exits.is_empty() {
        findings.push(Finding {
            ar: "لم تُدخل مخارج (exits) فلم تُوضع نقاط الإنذار اليدوي ولم تُفحص المسافة إليها.".into(),
            en: "No exits given, so no manual call points are placed and their reach is not checked.".into(),
        });
    } else if inp.max_call_point_distance_mm.is_none() {
        findings.push(Finding {
            ar: "لم يُدخل حد المسافة إلى نقطة الإنذار اليدوي (max_call_point_distance_mm) فلم يُحكم عليها.".into(),
            en: "No max_call_point_distance_mm given, so no pass or fail is declared for call-point reach.".into(),
        });
    }
    for (r, fr) in inp.rooms.iter().zip(&rooms) {
        if fr.detection.is_none() && fr.sprinklers.is_none() {
            findings.push(Finding {
                ar: format!("الغرفة «{}»: لا كاشف ولا رشاش مُسنَد إليها؛ ليست محمية في هذا الفحص (قد يكون الإعفاء قراراً نظامياً لا يقرره التطبيق).", r.key),
                en: format!("Room '{}': no detector and no sprinkler assigned; it is unprotected in this screen (an exemption would be a statutory decision this app does not make).", r.key),
            });
        }
        if r.ceiling_height_mm.is_none() && (fr.detection.is_some() || fr.sprinklers.is_some()) {
            findings.push(Finding {
                ar: format!("الغرفة «{}»: لم يُدخل ارتفاع السقف فلم يُفحص ارتفاع التركيب.", r.key),
                en: format!("Room '{}': no ceiling_height_mm given, so mounting height is not checked.", r.key),
            });
        }
    }
    findings.push(Finding {
        ar: "فحص أولي لتوزيع الكاشفات والرشاشات ونقاط الإنذار اليدوي والطلب المائي بمدخلاتك: لا حساب هيدروليكي لشبكة الرشاشات ولا مضخات ولا حساسية كواشف ولا سمعية إنذار ولا مصفوفة سبب ونتيجة ولا بطاريات، ولا يغني عن تصميم مهندس حريق أو الجهة المعتمدة.".into(),
        en: "Preliminary screen of detector, sprinkler and call-point layout and the water demand from your inputs: no sprinkler pipe hydraulics, pumps, detector sensitivity, alarm audibility, cause-and-effect matrix or batteries, and it does not replace a fire engineer's design or the approving authority.".into(),
    });
    Ok(FireReport { rooms, total_detectors, total_sprinklers, call_points, zones, zone_status, water, counts, findings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det() -> Device {
        Device {
            key: "D1".into(),
            coverage_m2: Some(20.0),
            max_spacing_mm: Some(5000.0),
            max_wall_distance_mm: Some(2600.0),
            max_ceiling_height_mm: Some(4000.0),
            source: Some("test datasheet".into()),
        }
    }
    fn spk() -> Device {
        Device {
            key: "S1".into(),
            coverage_m2: Some(12.0),
            max_spacing_mm: Some(4000.0),
            max_wall_distance_mm: None,
            max_ceiling_height_mm: None,
            source: Some("test datasheet".into()),
        }
    }
    fn room(key: &str, x: f64, y: f64, w: f64, h: f64) -> FireRoom {
        FireRoom {
            key: key.into(),
            x_mm: x,
            y_mm: y,
            w_mm: w,
            h_mm: h,
            detector: Some("D1".into()),
            sprinkler: None,
            zone: Some("Z1".into()),
            ceiling_height_mm: Some(3000.0),
        }
    }
    fn input(rooms: Vec<FireRoom>) -> FireInput {
        FireInput { rooms, detectors: vec![det()], sprinklers: vec![spk()], ..FireInput::default() }
    }

    #[test]
    fn detector_layout_respects_spacing_and_coverage() {
        // 6 x 5 m = 30 m2: cols >= ceil(6/5) = 2, rows >= 1, count >= ceil(30/20) = 2 -> 1 x 2, cells 3000 x 5000.
        let r = check(&input(vec![room("a", 1000.0, 2000.0, 6000.0, 5000.0)])).unwrap();
        let p = r.rooms[0].detection.as_ref().unwrap();
        assert_eq!((p.count, p.rows, p.cols), (2, 1, 2));
        assert_eq!((p.spacing_x_mm, p.spacing_y_mm), (3000.0, 5000.0));
        assert!(p.area_per_device_m2 <= 20.0 && p.spacing_x_mm.max(p.spacing_y_mm) <= 5000.0);
        assert_eq!(p.wall_distance_mm, 2500.0);
        assert_eq!((p.points[0].x_mm, p.points[0].y_mm), (1000.0 + 1500.0, 2000.0 + 2500.0));
        assert_eq!((p.points[1].x_mm, p.points[1].y_mm), (1000.0 + 4500.0, 2000.0 + 2500.0));
        assert_eq!((p.wall_status, p.height_status, r.rooms[0].status), ("ok", "ok", "ok"));
    }

    #[test]
    fn coverage_can_dominate_spacing() {
        // 10 x 10 m = 100 m2, coverage 20 -> 5 needed, spacing needs 2x2 = 4; 5 needed -> 2 x 3 = 6 cells is the least >= 5.
        let r = check(&input(vec![room("a", 0.0, 0.0, 10000.0, 10000.0)])).unwrap();
        let p = r.rooms[0].detection.as_ref().unwrap();
        assert_eq!(p.count, 6);
        assert!(p.area_per_device_m2 <= 20.0);
        // A single small room still gets one device.
        let r = check(&input(vec![room("b", 0.0, 0.0, 2000.0, 2000.0)])).unwrap();
        assert_eq!(r.rooms[0].detection.as_ref().unwrap().count, 1);
    }

    #[test]
    fn wall_distance_and_height_limits_fail_or_stay_unchecked() {
        let mut inp = input(vec![room("a", 0.0, 0.0, 6000.0, 5000.0)]);
        inp.detectors[0].max_wall_distance_mm = Some(2000.0); // 2500 mm actual
        let r = check(&inp).unwrap();
        assert_eq!(r.rooms[0].detection.as_ref().unwrap().wall_status, "fails");
        assert_eq!((r.rooms[0].status, r.counts.fails), ("fails", 1));

        let mut tall = input(vec![room("a", 0.0, 0.0, 6000.0, 5000.0)]);
        tall.rooms[0].ceiling_height_mm = Some(6000.0);
        assert_eq!(check(&tall).unwrap().rooms[0].detection.as_ref().unwrap().height_status, "fails");

        let mut none = input(vec![room("a", 0.0, 0.0, 6000.0, 5000.0)]);
        none.detectors[0].max_wall_distance_mm = None;
        none.rooms[0].ceiling_height_mm = None;
        let r = check(&none).unwrap();
        assert_eq!(r.rooms[0].status, "unchecked");
        assert!(r.findings.iter().any(|f| f.en.contains("no ceiling_height_mm")));
    }

    #[test]
    fn sprinklers_and_detectors_are_laid_out_independently() {
        let mut a = room("a", 0.0, 0.0, 8000.0, 6000.0);
        a.sprinkler = Some("S1".into());
        let r = check(&input(vec![a])).unwrap();
        let s = r.rooms[0].sprinklers.as_ref().unwrap();
        // 48 m2 / 12 = 4, cols >= 2, rows >= 2 -> 2 x 2, cells 4000 x 3000.
        assert_eq!((s.count, s.rows, s.cols), (4, 2, 2));
        assert_eq!((r.total_sprinklers, r.total_detectors), (4, 4));
        assert!(r.rooms[0].detection.is_some());
    }

    #[test]
    fn unprotected_room_is_reported_not_passed() {
        let mut a = room("a", 0.0, 0.0, 4000.0, 4000.0);
        a.detector = None;
        let r = check(&input(vec![a])).unwrap();
        assert_eq!(r.rooms[0].status, "unchecked");
        assert!(r.findings.iter().any(|f| f.en.contains("unprotected")));
    }

    #[test]
    fn call_point_reach_uses_the_farthest_corner() {
        let mut inp = input(vec![room("a", 0.0, 0.0, 6000.0, 5000.0)]);
        inp.exits = vec![Exit { key: "E1".into(), x_mm: 0.0, y_mm: 0.0 }];
        inp.max_call_point_distance_mm = Some(10_000.0);
        let c = check(&inp).unwrap().call_points;
        let expect = (6000.0_f64.powi(2) + 5000.0_f64.powi(2)).sqrt();
        assert!((c.reach[0].distance_mm - expect).abs() < 1e-6);
        assert_eq!((c.status, c.reach[0].nearest_exit.as_str()), ("ok", "E1"));
        inp.max_call_point_distance_mm = Some(7000.0);
        assert_eq!(check(&inp).unwrap().call_points.status, "fails");
        inp.max_call_point_distance_mm = None;
        assert_eq!(check(&inp).unwrap().call_points.status, "unchecked");
        inp.exits.clear();
        let c = check(&inp).unwrap().call_points;
        assert_eq!((c.status, c.worst_distance_mm), ("unchecked", None));
    }

    #[test]
    fn nearest_exit_is_chosen_per_corner() {
        let mut inp = input(vec![room("a", 0.0, 0.0, 6000.0, 5000.0)]);
        inp.exits = vec![
            Exit { key: "W".into(), x_mm: 0.0, y_mm: 0.0 },
            Exit { key: "E".into(), x_mm: 6000.0, y_mm: 5000.0 },
        ];
        inp.max_call_point_distance_mm = Some(6500.0);
        let c = check(&inp).unwrap().call_points;
        // Corner (6000,0) is 6000 from W and 5000 from E; (0,5000) is 5000 from W and 6000 from E: the worst nearest is 5000.
        assert!((c.worst_distance_mm.unwrap() - 5000.0).abs() < 1e-6);
        assert_eq!(c.status, "ok");
    }

    #[test]
    fn zones_count_detectors_and_area() {
        let mut inp = input(vec![room("a", 0.0, 0.0, 6000.0, 5000.0), room("b", 6000.0, 0.0, 6000.0, 5000.0)]);
        inp.max_detectors_per_zone = Some(4);
        inp.max_zone_area_m2 = Some(80.0);
        let r = check(&inp).unwrap();
        assert_eq!(r.zones.len(), 1);
        assert_eq!((r.zones[0].detectors, r.zones[0].area_m2), (4, 60.0));
        assert_eq!(r.zone_status, "ok");
        inp.max_detectors_per_zone = Some(3);
        assert_eq!(check(&inp).unwrap().zone_status, "fails");
        inp.max_detectors_per_zone = None;
        inp.max_zone_area_m2 = None;
        let r = check(&inp).unwrap();
        assert_eq!(r.zone_status, "unchecked");
        assert!(r.findings.iter().any(|f| f.en.starts_with("No zone limits")));
    }

    #[test]
    fn unzoned_detector_room_blocks_a_zone_pass() {
        let mut inp = input(vec![room("a", 0.0, 0.0, 6000.0, 5000.0), room("b", 6000.0, 0.0, 6000.0, 5000.0)]);
        inp.rooms[1].zone = None;
        inp.max_detectors_per_zone = Some(10);
        let r = check(&inp).unwrap();
        assert_eq!(r.zone_status, "unchecked");
        assert!(r.findings.iter().any(|f| f.en.contains("no zone")));
    }

    #[test]
    fn water_demand_flow_and_storage() {
        let mut a = room("a", 0.0, 0.0, 20000.0, 10000.0);
        a.sprinkler = Some("S1".into());
        let mut inp = input(vec![a]);
        inp.water = Some(WaterDemand {
            design_density_mm_min: Some(5.0),
            design_area_m2: Some(120.0),
            duration_min: Some(60.0),
            source: Some("test hazard table".into()),
            hose_allowance_lpm: None,
            available_storage_m3: Some(40.0),
        });
        let w = check(&inp).unwrap().water.unwrap();
        assert_eq!((w.sprinkler_flow_lpm, w.total_flow_lpm, w.required_storage_m3), (600.0, 600.0, 36.0));
        assert_eq!((w.sprinklered_area_m2, w.status), (200.0, "ok"));
        let mut hose = inp.clone();
        hose.water.as_mut().unwrap().hose_allowance_lpm = Some(100.0);
        let w = check(&hose).unwrap().water.unwrap();
        assert_eq!((w.total_flow_lpm, w.required_storage_m3, w.status), (700.0, 42.0, "fails"));
        let mut open = inp.clone();
        open.water.as_mut().unwrap().available_storage_m3 = None;
        let r = check(&open).unwrap();
        assert_eq!(r.water.as_ref().unwrap().status, "unchecked");
        assert!(r.findings.iter().any(|f| f.en.contains("hose_allowance_lpm")));
        assert!(r.findings.iter().any(|f| f.en.contains("available_storage_m3")));
    }

    #[test]
    fn design_area_larger_than_the_sprinklered_area_fails() {
        let mut a = room("a", 0.0, 0.0, 6000.0, 5000.0); // 30 m2
        a.sprinkler = Some("S1".into());
        let mut inp = input(vec![a]);
        inp.water = Some(WaterDemand {
            design_density_mm_min: Some(5.0),
            design_area_m2: Some(120.0),
            duration_min: Some(30.0),
            source: Some("x".into()),
            hose_allowance_lpm: Some(50.0),
            available_storage_m3: Some(1000.0),
        });
        let r = check(&inp).unwrap();
        assert_eq!(r.water.unwrap().status, "fails");
        assert!(r.findings.iter().any(|f| f.en.contains("larger than")));
    }

    #[test]
    fn sprinklers_without_a_water_section_say_so() {
        let mut a = room("a", 0.0, 0.0, 6000.0, 5000.0);
        a.sprinkler = Some("S1".into());
        let r = check(&input(vec![a])).unwrap();
        assert!(r.water.is_none());
        assert!(r.findings.iter().any(|f| f.en.contains("no water demand section")));
    }

    #[test]
    fn missing_or_bad_inputs_are_refused_by_name() {
        assert!(check(&FireInput::default()).unwrap_err().contains("nothing to protect"));
        for field in ["coverage_m2", "max_spacing_mm", "source"] {
            let mut inp = input(vec![room("a", 0.0, 0.0, 6000.0, 5000.0)]);
            match field {
                "coverage_m2" => inp.detectors[0].coverage_m2 = None,
                "max_spacing_mm" => inp.detectors[0].max_spacing_mm = None,
                _ => inp.detectors[0].source = Some(" ".into()),
            }
            assert!(check(&inp).unwrap_err().contains(field), "{field}");
        }
        let mut ghost = room("a", 0.0, 0.0, 6000.0, 5000.0);
        ghost.detector = Some("nope".into());
        assert!(check(&input(vec![ghost])).unwrap_err().contains("not in the detector list"));
        assert!(check(&input(vec![room("a", 0.0, 0.0, 0.0, 5000.0)])).unwrap_err().contains("w_mm must be a positive"));
        assert!(check(&input(vec![room("d", 0.0, 0.0, 3000.0, 3000.0), room("d", 0.0, 0.0, 3000.0, 3000.0)]))
            .unwrap_err()
            .contains("used twice"));
        let mut bad = input(vec![room("a", 0.0, 0.0, 6000.0, 5000.0)]);
        bad.max_call_point_distance_mm = Some(-1.0);
        assert!(check(&bad).unwrap_err().contains("max_call_point_distance_mm"));
        let mut zero = input(vec![room("a", 0.0, 0.0, 6000.0, 5000.0)]);
        zero.max_detectors_per_zone = Some(0);
        assert!(check(&zero).unwrap_err().contains("max_detectors_per_zone"));
        let mut tiny = input(vec![room("a", 0.0, 0.0, 60_000.0, 50_000.0)]);
        tiny.detectors[0].coverage_m2 = Some(0.0001);
        assert!(check(&tiny).unwrap_err().contains("more than"));
        for field in ["design_density_mm_min", "design_area_m2", "duration_min", "source"] {
            let mut a = room("a", 0.0, 0.0, 6000.0, 5000.0);
            a.sprinkler = Some("S1".into());
            let mut inp = input(vec![a]);
            let mut w = WaterDemand {
                design_density_mm_min: Some(5.0),
                design_area_m2: Some(10.0),
                duration_min: Some(30.0),
                source: Some("s".into()),
                hose_allowance_lpm: None,
                available_storage_m3: None,
            };
            match field {
                "design_density_mm_min" => w.design_density_mm_min = None,
                "design_area_m2" => w.design_area_m2 = None,
                "duration_min" => w.duration_min = None,
                _ => w.source = None,
            }
            inp.water = Some(w);
            assert!(check(&inp).unwrap_err().contains(field), "{field}");
        }
    }
}
