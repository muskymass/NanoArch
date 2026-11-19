//! Early-design analyses that are pure geometry or arithmetic on the project's own numbers. Nothing here is a statutory value:
//! every limit that is not geometry (a slope, a tolerance, a circulation share) is an INPUT that the caller supplies and the result
//! repeats, and a missing input is reported by name.
//!
//! - `solar`: the sun's position from the standard declination / hour-angle formulas (approximate, about 1 degree), the sun table
//!   at the solstices and equinoxes, the summer and winter hours each compass facade is in direct sun, and the overhang depth that
//!   shades a window at the summer-solstice noon. Prevailing wind is NOT computed: this application holds no verified wind data.
//! - `screen_height`: the height of a parapet or screen that blocks the line of sight from a viewer to a point in a private zone.
//! - `efficiency` and `compare`: net-to-gross ratio, circulation share, and a side-by-side matrix of layout alternatives.
//! - `stair` and `ramp`: the comfort formula `2R + T` for a stair and the slope of a ramp, against limits the caller provides.

use serde::Serialize;

// ------------------------------------------------------------------ solar

pub const FACADES: [(&str, f64); 8] = [("north", 0.0), ("north-east", 45.0), ("east", 90.0), ("south-east", 135.0), ("south", 180.0), ("south-west", 225.0), ("west", 270.0), ("north-west", 315.0)];

/// Day numbers of the solstices and equinoxes (21 Jun, 21 Dec, 21 Mar, 23 Sep): fixed calendar approximations.
pub const DAYS: [(&str, u32); 4] = [("summer solstice", 172), ("winter solstice", 355), ("spring equinox", 80), ("autumn equinox", 266)];

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Sun {
    pub altitude_deg: f64,
    /// Compass bearing, degrees clockwise from north.
    pub azimuth_deg: f64,
}

/// Declination of the sun on day `n` of the year (Cooper's formula), degrees.
pub fn declination_deg(day: u32) -> f64 {
    23.45 * (360.0 / 365.0 * (284.0 + day as f64)).to_radians().sin()
}

/// The sun at `solar_hour` (12.0 is solar noon) on day `day` at latitude `lat_deg`; `None` when it is below the horizon.
pub fn sun(lat_deg: f64, day: u32, solar_hour: f64) -> Option<Sun> {
    let (phi, delta, omega) = (lat_deg.to_radians(), declination_deg(day).to_radians(), (15.0 * (solar_hour - 12.0)).to_radians());
    let sin_alt = phi.sin() * delta.sin() + phi.cos() * delta.cos() * omega.cos();
    let alt = sin_alt.clamp(-1.0, 1.0).asin();
    if alt <= 0.0 {
        return None;
    }
    let cos_az = ((delta.sin() - sin_alt * phi.sin()) / (alt.cos() * phi.cos())).clamp(-1.0, 1.0);
    let az = cos_az.acos().to_degrees();
    Some(Sun { altitude_deg: alt.to_degrees(), azimuth_deg: if omega <= 0.0 { az } else { 360.0 - az } })
}

#[derive(Debug, Clone, Serialize)]
pub struct SunRow {
    pub day: &'static str,
    pub declination_deg: f64,
    pub noon_altitude_deg: f64,
    pub sunrise_azimuth_deg: f64,
    pub sunset_azimuth_deg: f64,
    pub daylight_hours: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FacadeSun {
    pub facade: &'static str,
    pub summer_sun_hours: f64,
    pub winter_sun_hours: f64,
    /// `summer / winter`: lower is better for a facade with glazing in a hot climate (`None` when winter has no direct sun on it).
    pub summer_to_winter: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SolarReport {
    pub latitude_deg: f64,
    pub table: Vec<SunRow>,
    pub facades: Vec<FacadeSun>,
    /// Facades ordered from the least summer sun to the most.
    pub best_for_glazing: Vec<&'static str>,
    /// Overhang depth that shades the whole window at the summer-solstice noon, for a south-facing (north in the southern hemisphere) window.
    pub overhang_m: Option<f64>,
    pub notes: Vec<String>,
}

fn sun_hours(lat: f64, day: u32, facade_az: f64) -> f64 {
    // A vertical facade sees the sun while the sun is up and within 90 degrees of the facade's outward normal.
    let step = 0.05;
    let mut h = 0.0;
    let mut t = 0.0;
    while t < 24.0 {
        if let Some(s) = sun(lat, day, t) {
            let d = ((s.azimuth_deg - facade_az + 540.0) % 360.0) - 180.0;
            if d.abs() < 90.0 {
                h += step;
            }
        }
        t += step;
    }
    (h * 10.0).round() / 10.0
}

pub fn solar(lat_deg: f64, window_height_m: Option<f64>) -> Result<SolarReport, String> {
    if !lat_deg.is_finite() || lat_deg.abs() >= 66.0 {
        return Err("latitude must be a number between -66 and 66 degrees".into());
    }
    let r1 = |v: f64| (v * 10.0).round() / 10.0;
    let table = DAYS
        .iter()
        .map(|&(name, d)| {
            let noon = sun(lat_deg, d, 12.0).map_or(0.0, |s| s.altitude_deg);
            let cos_ws = -(lat_deg.to_radians().tan() * declination_deg(d).to_radians().tan());
            let ws = cos_ws.clamp(-1.0, 1.0).acos().to_degrees();
            let rise = sun(lat_deg, d, 12.0 - ws / 15.0 + 0.02).map_or(0.0, |s| s.azimuth_deg);
            let set = sun(lat_deg, d, 12.0 + ws / 15.0 - 0.02).map_or(0.0, |s| s.azimuth_deg);
            SunRow { day: name, declination_deg: r1(declination_deg(d)), noon_altitude_deg: r1(noon), sunrise_azimuth_deg: r1(rise), sunset_azimuth_deg: r1(set), daylight_hours: r1(2.0 * ws / 15.0) }
        })
        .collect();
    let (summer, winter) = if lat_deg >= 0.0 { (172, 355) } else { (355, 172) };
    let facades: Vec<FacadeSun> = FACADES
        .iter()
        .map(|&(name, az)| {
            let (s, w) = (sun_hours(lat_deg, summer, az), sun_hours(lat_deg, winter, az));
            FacadeSun { facade: name, summer_sun_hours: s, winter_sun_hours: w, summer_to_winter: (w > 0.0).then(|| r1(s / w * 100.0) / 100.0) }
        })
        .collect();
    let mut order: Vec<&FacadeSun> = facades.iter().collect();
    order.sort_by(|a, b| a.summer_sun_hours.total_cmp(&b.summer_sun_hours).then(b.winter_sun_hours.total_cmp(&a.winter_sun_hours)));
    let best = order.iter().map(|f| f.facade).collect();
    let noon_summer = sun(lat_deg, summer, 12.0).map_or(0.0, |s| s.altitude_deg);
    let overhang = window_height_m.filter(|h| *h > 0.0).and_then(|h| (noon_summer > 0.0 && noon_summer < 89.9).then(|| r1(h / noon_summer.to_radians().tan() * 100.0) / 100.0));
    Ok(SolarReport {
        latitude_deg: lat_deg,
        table,
        facades,
        best_for_glazing: best,
        overhang_m: overhang,
        notes: vec![
            "approximate astronomical formulas (about 1 degree): not a shading simulation, no obstructions, no atmosphere".into(),
            "prevailing wind is not computed: this application holds no verified wind data for any city, enter it from a source you trust".into(),
        ],
    })
}

// ------------------------------------------------------------------ privacy sightline

#[derive(Debug, Clone, Serialize)]
pub struct Screen {
    /// Height above the private zone's floor that a screen at the boundary must reach to block the sight line.
    pub required_height_m: f64,
    pub notes: Vec<String>,
}

/// A viewer whose eyes are `eye_m` above the private zone's floor level (add the difference if the street is higher), standing
/// `viewer_to_screen_m` from the screen; the point to hide is `target_m` high and `screen_to_target_m` behind the screen.
pub fn screen_height(eye_m: f64, viewer_to_screen_m: f64, screen_to_target_m: f64, target_m: f64) -> Result<Screen, String> {
    for (n, v) in [("eye height", eye_m), ("viewer distance", viewer_to_screen_m), ("target distance", screen_to_target_m), ("target height", target_m)] {
        if !v.is_finite() || v < 0.0 {
            return Err(format!("{n} must be zero or more"));
        }
    }
    if viewer_to_screen_m <= 0.0 {
        return Err("the viewer must stand away from the screen".into());
    }
    let h = eye_m + (target_m - eye_m) * viewer_to_screen_m / (viewer_to_screen_m + screen_to_target_m);
    let mut notes = vec!["the screen blocks the straight line from the eye to the top of the target; a viewer who climbs or leans is not covered".to_string()];
    if h <= 0.0 {
        notes.push("the target is already hidden by the ground level".into());
    }
    Ok(Screen { required_height_m: (h.max(0.0) * 100.0).round() / 100.0, notes })
}

// ------------------------------------------------------------------ space efficiency

/// Functions counted as circulation.
pub const CIRCULATION: [&str; 3] = ["hall", "stair", "corridor"];

#[derive(Debug, Clone, Serialize)]
pub struct Efficiency {
    pub gross_m2: f64,
    pub net_m2: f64,
    pub circulation_m2: f64,
    pub net_to_gross: f64,
    pub circulation_pct: f64,
    pub flagged: bool,
    pub hints: Vec<String>,
}

/// `rooms`: `(function, area m2)`. `max_circulation_pct` is the caller's limit (the brief asks for 15).
pub fn efficiency(rooms: &[(String, f64)], max_circulation_pct: f64) -> Result<Efficiency, String> {
    if !max_circulation_pct.is_finite() || max_circulation_pct <= 0.0 {
        return Err("the circulation limit (%) is missing".into());
    }
    let gross: f64 = rooms.iter().map(|r| r.1).sum();
    if gross <= 0.0 {
        return Err("there are no rooms to measure".into());
    }
    let circ: f64 = rooms.iter().filter(|r| CIRCULATION.contains(&r.0.as_str())).map(|r| r.1).sum();
    let pct = circ / gross * 100.0;
    let flagged = pct > max_circulation_pct;
    let mut hints = Vec::new();
    if flagged {
        hints.push(format!("circulation is {:.1}% of the floor area, above the {max_circulation_pct}% limit: shorten the corridor or let rooms open onto a shared hall", pct));
    }
    let r2 = |v: f64| (v * 100.0).round() / 100.0;
    Ok(Efficiency { gross_m2: r2(gross), net_m2: r2(gross - circ), circulation_m2: r2(circ), net_to_gross: r2((gross - circ) / gross), circulation_pct: (pct * 10.0).round() / 10.0, flagged, hints })
}

#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub index: usize,
    pub cost: f64,
    pub efficiency: Efficiency,
}

#[derive(Debug, Clone, Serialize)]
pub struct Comparison {
    pub rows: Vec<Row>,
    pub lowest_cost: usize,
    pub least_circulation: usize,
    /// Columns the brief asks for that this analysis cannot fill, by name.
    pub not_available: Vec<&'static str>,
}

/// Up to three alternatives side by side: `(layout cost, rooms)`.
pub fn compare(alts: &[(f64, Vec<(String, f64)>)], max_circulation_pct: f64) -> Result<Comparison, String> {
    if alts.is_empty() {
        return Err("generate alternatives first".into());
    }
    let mut rows = Vec::new();
    for (i, (cost, rooms)) in alts.iter().take(3).enumerate() {
        rows.push(Row { index: i, cost: (cost * 100.0).round() / 100.0, efficiency: efficiency(rooms, max_circulation_pct)? });
    }
    let lowest_cost = rows.iter().min_by(|a, b| a.cost.total_cmp(&b.cost)).map_or(0, |r| r.index);
    let least_circulation = rows.iter().min_by(|a, b| a.efficiency.circulation_pct.total_cmp(&b.efficiency.circulation_pct)).map_or(0, |r| r.index);
    Ok(Comparison { rows, lowest_cost, least_circulation, not_available: vec!["structural shell cost", "glazing area", "cooling load"] })
}

// ------------------------------------------------------------------ stairs and ramps

#[derive(Debug, Clone, Serialize)]
pub struct StairCheck {
    pub riser_mm: f64,
    pub tread_mm: f64,
    pub two_r_plus_t_mm: f64,
    pub target_mm: f64,
    pub deviation_mm: f64,
    pub comfortable: bool,
    pub run_mm: f64,
    pub note: &'static str,
}

/// `floor_height_mm` divided into `risers` equal risers; comfort is `2R + T` within `tolerance_mm` of `target_mm`
/// (a design rule, not an SBC clause: the SBC limits on riser and tread are a separate check).
pub fn stair(floor_height_mm: f64, risers: u32, tread_mm: f64, target_mm: f64, tolerance_mm: f64) -> Result<StairCheck, String> {
    if floor_height_mm <= 0.0 || risers == 0 || tread_mm <= 0.0 || target_mm <= 0.0 || tolerance_mm < 0.0 {
        return Err("floor height, riser count, tread, target and tolerance must be positive".into());
    }
    let r = floor_height_mm / risers as f64;
    let v = 2.0 * r + tread_mm;
    let dev = v - target_mm;
    Ok(StairCheck { riser_mm: (r * 10.0).round() / 10.0, tread_mm, two_r_plus_t_mm: (v * 10.0).round() / 10.0, target_mm, deviation_mm: (dev * 10.0).round() / 10.0, comfortable: dev.abs() <= tolerance_mm, run_mm: tread_mm * (risers.saturating_sub(1)) as f64, note: "2R + T is a comfort rule of thumb; the code limits on riser and tread are checked separately" })
}

#[derive(Debug, Clone, Serialize)]
pub struct RampCheck {
    pub slope_pct: f64,
    pub max_slope_pct: f64,
    pub ok: bool,
    /// The shortest run that keeps the slope at the limit for this rise.
    pub min_run_mm: f64,
}

pub fn ramp(rise_mm: f64, run_mm: f64, max_slope_pct: Option<f64>) -> Result<RampCheck, String> {
    let max = max_slope_pct.filter(|m| m.is_finite() && *m > 0.0).ok_or("the maximum ramp slope (%) is missing: enter it with its source")?;
    if rise_mm <= 0.0 || run_mm <= 0.0 {
        return Err("rise and run must be positive".into());
    }
    let s = rise_mm / run_mm * 100.0;
    Ok(RampCheck { slope_pct: (s * 100.0).round() / 100.0, max_slope_pct: max, ok: s <= max + 1e-9, min_run_mm: (rise_mm / (max / 100.0)).ceil() })
}
