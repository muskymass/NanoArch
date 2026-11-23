//! Luminaire count and layout for a target illuminance (lumen method).
//!
//! `N = E * A / (F * UF * MF)` with `E` the target maintained illuminance (lux), `A` the room area (m2), `F` the luminous flux of
//! one luminaire (lm), `UF` the utilisation factor and `MF` the maintenance factor. No value in this module was read from a
//! lighting code or a photometric table in this project. The target lux of every room (with its source), and every luminaire's
//! flux, utilisation factor and maintenance factor, are inputs; nothing is assumed. A missing one is refused by name.
//! Optional limits (maximum spacing-to-height ratio, maximum power density) are the caller's: a missing limit leaves that check
//! `unchecked`, never `ok`.
//!
//! The layout is a regular grid of `rows x cols` luminaires at the centres of equal cells, chosen as the smallest count that
//! reaches `N` (ties go to the squarer spacing, then fewer columns). Because the grid is regular the count can exceed `N`; the
//! achieved average illuminance is reported with the count actually placed.
//!
//! This is a preliminary average-illuminance screen. It does not compute point illuminance, uniformity, glare, daylight or
//! emergency lighting, and it does not replace a lighting design or the approving authority.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Luminaire {
    pub key: String,
    /// Luminous flux of one luminaire, lm.
    #[serde(default)]
    pub lumens: Option<f64>,
    /// Utilisation factor, greater than 0 and at most 1.
    #[serde(default)]
    pub utilisation: Option<f64>,
    /// Maintenance factor, greater than 0 and at most 1.
    #[serde(default)]
    pub maintenance: Option<f64>,
    /// Electrical power of one luminaire, W. Absent: no power figures for rooms using it.
    #[serde(default)]
    pub watts: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LightRoom {
    pub key: String,
    pub x_mm: f64,
    pub y_mm: f64,
    pub w_mm: f64,
    pub h_mm: f64,
    /// Target maintained illuminance, lux.
    #[serde(default)]
    pub target_lux: Option<f64>,
    /// Where the target came from (code, edition, table); required with the target.
    #[serde(default)]
    pub target_source: Option<String>,
    /// Key of the luminaire used in this room.
    pub luminaire: String,
    /// Mounting height above the working plane, mm. Absent: the spacing check is `unchecked`.
    #[serde(default)]
    pub mounting_height_mm: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LightInput {
    pub rooms: Vec<LightRoom>,
    #[serde(default)]
    pub luminaires: Vec<Luminaire>,
    /// Largest acceptable spacing / mounting-height ratio. Absent: spacing is `unchecked`.
    #[serde(default)]
    pub max_spacing_to_height: Option<f64>,
    /// Largest acceptable installed power density, W/m2. Absent: power is `unchecked`.
    #[serde(default)]
    pub max_w_per_m2: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Point {
    pub x_mm: f64,
    pub y_mm: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LightRow {
    pub key: String,
    pub luminaire: String,
    pub area_m2: f64,
    pub target_lux: f64,
    pub target_source: String,
    /// Count from the lumen method before rounding up to a regular grid.
    pub required: f64,
    pub count: u32,
    pub rows: u32,
    pub cols: u32,
    pub spacing_x_mm: f64,
    pub spacing_y_mm: f64,
    pub achieved_lux: f64,
    pub points: Vec<Point>,
    pub spacing_status: &'static str,
    pub power_w: Option<f64>,
    pub power_w_per_m2: Option<f64>,
    pub power_status: &'static str,
    pub status: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LightCounts {
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
pub struct LightReport {
    pub rooms: Vec<LightRow>,
    pub total_luminaires: u32,
    pub total_power_w: Option<f64>,
    pub counts: LightCounts,
    pub findings: Vec<Finding>,
}

fn positive(v: f64, field: &str, key: &str) -> Result<f64, String> {
    if v.is_finite() && v > 0.0 {
        Ok(v)
    } else {
        Err(format!("'{key}': {field} must be a positive number, got {v}"))
    }
}

fn factor(v: Option<f64>, field: &str, key: &str) -> Result<f64, String> {
    let v = v.ok_or_else(|| format!("luminaire '{key}': {field} is required and has no default"))?;
    if v.is_finite() && v > 0.0 && v <= 1.0 {
        Ok(v)
    } else {
        Err(format!("luminaire '{key}': {field} must be above 0 and at most 1, got {v}"))
    }
}

/// Smallest regular grid with at least `n` cells for a `w` x `h` room: fewest cells, then squarest spacing, then fewer columns.
fn grid(n: u32, w: f64, h: f64) -> (u32, u32) {
    let mut best: Option<(u32, u32, f64)> = None;
    for cols in 1..=n {
        let rows = n.div_ceil(cols);
        let cells = rows * cols;
        let skew = (w / f64::from(cols) - h / f64::from(rows)).abs();
        let better = match best {
            None => true,
            Some((br, bc, bs)) => {
                let bcells = br * bc;
                cells < bcells || (cells == bcells && skew < bs - 1e-9)
            }
        };
        if better {
            best = Some((rows, cols, skew));
        }
    }
    best.map_or((1, 1), |(r, c, _)| (r, c))
}

fn worst(statuses: [&'static str; 2]) -> &'static str {
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
        (Some(v), Some(l)) if v <= l => "ok",
        (Some(_), Some(_)) => "fails",
        _ => "unchecked",
    }
}

fn row(r: &LightRoom, inp: &LightInput) -> Result<LightRow, String> {
    if r.key.trim().is_empty() {
        return Err("a room has an empty key".into());
    }
    let w = positive(r.w_mm, "w_mm", &r.key)?;
    let h = positive(r.h_mm, "h_mm", &r.key)?;
    if !r.x_mm.is_finite() || !r.y_mm.is_finite() {
        return Err(format!("'{}': x_mm and y_mm must be finite", r.key));
    }
    let target = r.target_lux.ok_or_else(|| format!("room '{}': target_lux is required and has no default", r.key))?;
    let target = positive(target, "target_lux", &r.key)?;
    let source = r
        .target_source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("room '{}': target_source is required (code, edition, table the target_lux came from)", r.key))?;
    let lum = inp
        .luminaires
        .iter()
        .find(|l| l.key == r.luminaire)
        .ok_or_else(|| format!("room '{}': luminaire '{}' is not in the luminaire list", r.key, r.luminaire))?;
    let flux = lum.lumens.ok_or_else(|| format!("luminaire '{}': lumens is required and has no default", lum.key))?;
    let flux = positive(flux, "lumens", &lum.key)?;
    let uf = factor(lum.utilisation, "utilisation", &lum.key)?;
    let mf = factor(lum.maintenance, "maintenance", &lum.key)?;
    let watts = lum.watts.map(|v| positive(v, "watts", &lum.key)).transpose()?;
    let height = r.mounting_height_mm.map(|v| positive(v, "mounting_height_mm", &r.key)).transpose()?;

    let area = w * h / 1.0e6;
    let required = target * area / (flux * uf * mf);
    // A tiny tolerance keeps an exact whole count (e.g. 2.0000000001) from adding a luminaire.
    let n = (required - 1e-9).ceil().max(1.0) as u32;
    let (rows, cols) = grid(n, w, h);
    let count = rows * cols;
    let sx = w / f64::from(cols);
    let sy = h / f64::from(rows);
    let achieved = f64::from(count) * flux * uf * mf / area;
    let mut points = Vec::with_capacity(count as usize);
    for j in 0..rows {
        for i in 0..cols {
            points.push(Point { x_mm: r.x_mm + (f64::from(i) + 0.5) * sx, y_mm: r.y_mm + (f64::from(j) + 0.5) * sy });
        }
    }
    let spacing_status = limit_status(height.map(|hm| sx.max(sy) / hm), inp.max_spacing_to_height);
    let power_w = watts.map(|v| v * f64::from(count));
    let power_w_per_m2 = power_w.map(|p| p / area);
    let power_status = limit_status(power_w_per_m2, inp.max_w_per_m2);
    Ok(LightRow {
        key: r.key.clone(),
        luminaire: lum.key.clone(),
        area_m2: area,
        target_lux: target,
        target_source: source.to_string(),
        required,
        count,
        rows,
        cols,
        spacing_x_mm: sx,
        spacing_y_mm: sy,
        achieved_lux: achieved,
        points,
        spacing_status,
        power_w,
        power_w_per_m2,
        power_status,
        status: worst([spacing_status, power_status]),
    })
}

pub fn check(inp: &LightInput) -> Result<LightReport, String> {
    if inp.rooms.is_empty() {
        return Err("nothing to light: give at least one room".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for r in &inp.rooms {
        if !seen.insert(r.key.as_str()) {
            return Err(format!("room key '{}' is used twice", r.key));
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for l in &inp.luminaires {
        if !seen.insert(l.key.as_str()) {
            return Err(format!("luminaire key '{}' is used twice", l.key));
        }
    }
    if let Some(v) = inp.max_spacing_to_height {
        positive(v, "max_spacing_to_height", "input")?;
    }
    if let Some(v) = inp.max_w_per_m2 {
        positive(v, "max_w_per_m2", "input")?;
    }

    let rooms = inp.rooms.iter().map(|r| row(r, inp)).collect::<Result<Vec<_>, _>>()?;
    let mut counts = LightCounts::default();
    for r in &rooms {
        match r.status {
            "ok" => counts.ok += 1,
            "fails" => counts.fails += 1,
            _ => counts.unchecked += 1,
        }
    }
    let total_luminaires = rooms.iter().map(|r| r.count).sum();
    let total_power_w = if rooms.iter().all(|r| r.power_w.is_some()) { Some(rooms.iter().filter_map(|r| r.power_w).sum()) } else { None };

    let mut findings = Vec::new();
    if inp.max_spacing_to_height.is_none() {
        findings.push(Finding {
            ar: "لم يُدخل حد نسبة التباعد إلى الارتفاع (max_spacing_to_height) فلم يُحكم على التباعد.".into(),
            en: "No max_spacing_to_height given, so no pass or fail is declared for spacing.".into(),
        });
    }
    if inp.max_w_per_m2.is_none() {
        findings.push(Finding {
            ar: "لم يُدخل حد كثافة القدرة (max_w_per_m2) فلم يُحكم على القدرة.".into(),
            en: "No max_w_per_m2 given, so no pass or fail is declared for power density.".into(),
        });
    }
    for (r, src) in rooms.iter().zip(&inp.rooms) {
        if src.mounting_height_mm.is_none() {
            findings.push(Finding {
                ar: format!("الغرفة «{}»: لم يُدخل ارتفاع التركيب فلم يُفحص التباعد.", r.key),
                en: format!("Room '{}': no mounting_height_mm given, so spacing is not checked.", r.key),
            });
        }
        if r.power_w.is_none() {
            findings.push(Finding {
                ar: format!("الغرفة «{}»: لم تُدخل قدرة الوحدة (watts) فلم تُحسب القدرة.", r.key),
                en: format!("Room '{}': the luminaire has no watts, so power is not computed.", r.key),
            });
        }
        if f64::from(r.count) > r.required.ceil() {
            findings.push(Finding {
                ar: format!(
                    "الغرفة «{}»: الشبكة المنتظمة تضع {} وحدة والحساب يطلب {:.2}؛ الإضاءة الفعلية {:.0} لوكس والهدف {:.0}.",
                    r.key, r.count, r.required, r.achieved_lux, r.target_lux
                ),
                en: format!(
                    "Room '{}': the regular grid places {} luminaires where the calculation asks for {:.2}; achieved {:.0} lx against a target of {:.0} lx.",
                    r.key, r.count, r.required, r.achieved_lux, r.target_lux
                ),
            });
        }
    }
    findings.push(Finding {
        ar: "فحص أولي لمتوسط الإضاءة بمدخلاتك: لا إضاءة نقطية ولا انتظامية ولا انبهار ولا ضوء نهار ولا إنارة طوارئ، ولا يغني عن تصميم إنارة أو الجهة المعتمدة.".into(),
        en: "Preliminary average-illuminance screen from your inputs: no point illuminance, uniformity, glare, daylight or emergency lighting, and it does not replace a lighting design or the approving authority.".into(),
    });
    Ok(LightReport { rooms, total_luminaires, total_power_w, counts, findings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lum() -> Luminaire {
        Luminaire { key: "L1".into(), lumens: Some(4000.0), utilisation: Some(0.5), maintenance: Some(0.8), watts: Some(40.0) }
    }
    fn room(key: &str, w: f64, h: f64, lux: f64) -> LightRoom {
        LightRoom {
            key: key.into(),
            x_mm: 1000.0,
            y_mm: 2000.0,
            w_mm: w,
            h_mm: h,
            target_lux: Some(lux),
            target_source: Some("client brief".into()),
            luminaire: "L1".into(),
            mounting_height_mm: Some(2500.0),
        }
    }
    fn input(rooms: Vec<LightRoom>) -> LightInput {
        LightInput { rooms, luminaires: vec![lum()], max_spacing_to_height: Some(1.5), max_w_per_m2: Some(12.0) }
    }

    #[test]
    fn lumen_method_count_and_achieved_lux() {
        // 5 x 4 m = 20 m2, 300 lx: 6000 lm / (4000*0.5*0.8 = 1600) = 3.75 -> 4 luminaires in a 2x2 grid.
        let r = check(&input(vec![room("a", 5000.0, 4000.0, 300.0)])).unwrap();
        let a = &r.rooms[0];
        assert!((a.required - 3.75).abs() < 1e-9);
        assert_eq!((a.count, a.rows, a.cols), (4, 2, 2));
        assert!((a.achieved_lux - 4.0 * 1600.0 / 20.0).abs() < 1e-9);
        assert!(a.achieved_lux >= a.target_lux);
        assert_eq!(a.points.len(), 4);
        assert_eq!((a.points[0].x_mm, a.points[0].y_mm), (1000.0 + 1250.0, 2000.0 + 1000.0));
        assert_eq!((a.points[3].x_mm, a.points[3].y_mm), (1000.0 + 3750.0, 2000.0 + 3000.0));
    }

    #[test]
    fn exact_whole_count_adds_no_extra_luminaire() {
        // 4 x 4 m = 16 m2, 200 lx: 3200 / 1600 = 2.0 exactly.
        let r = check(&input(vec![room("a", 4000.0, 4000.0, 200.0)])).unwrap();
        assert_eq!(r.rooms[0].count, 2);
    }

    #[test]
    fn grid_follows_the_room_shape() {
        assert_eq!(grid(6, 12000.0, 2000.0), (1, 6));
        assert_eq!(grid(6, 2000.0, 12000.0), (6, 1));
        let (rows, cols) = grid(4, 4000.0, 4000.0);
        assert_eq!((rows, cols), (2, 2));
    }

    #[test]
    fn spacing_and_power_limits_fail_or_stay_unchecked() {
        let mut tall = room("tall", 5000.0, 4000.0, 300.0);
        tall.mounting_height_mm = Some(1000.0);
        let r = check(&input(vec![tall])).unwrap();
        assert_eq!((r.rooms[0].spacing_status, r.rooms[0].status), ("fails", "fails"));

        let mut limits = input(vec![room("a", 5000.0, 4000.0, 300.0)]);
        limits.max_w_per_m2 = Some(5.0); // 160 W / 20 m2 = 8 W/m2
        let r = check(&limits).unwrap();
        assert_eq!(r.rooms[0].power_status, "fails");
        assert!((r.rooms[0].power_w_per_m2.unwrap() - 8.0).abs() < 1e-9);

        let mut none = input(vec![room("a", 5000.0, 4000.0, 300.0)]);
        none.max_spacing_to_height = None;
        none.max_w_per_m2 = None;
        let r = check(&none).unwrap();
        assert_eq!(r.rooms[0].status, "unchecked");
        assert_eq!(r.counts, LightCounts { ok: 0, fails: 0, unchecked: 1 });
        assert_eq!(r.findings.iter().filter(|f| f.en.starts_with("No max_")).count(), 2);
    }

    #[test]
    fn within_all_limits_is_ok_and_totals_add_up() {
        let mut b = room("b", 4000.0, 4000.0, 200.0);
        b.mounting_height_mm = Some(3000.0); // spacing 4000 / 3000 = 1.33, within the 1.5 limit
        let r = check(&input(vec![room("a", 5000.0, 4000.0, 300.0), b])).unwrap();
        assert_eq!(r.rooms[0].status, "ok");
        assert_eq!(r.total_luminaires, 6);
        assert_eq!(r.total_power_w, Some(240.0));
        assert_eq!(r.counts.ok, 2);
    }

    #[test]
    fn missing_watts_gives_no_power_figures() {
        let mut inp = input(vec![room("a", 5000.0, 4000.0, 300.0)]);
        inp.luminaires[0].watts = None;
        let r = check(&inp).unwrap();
        assert_eq!((r.rooms[0].power_w, r.total_power_w, r.rooms[0].power_status, r.rooms[0].status), (None, None, "unchecked", "unchecked"));
    }

    #[test]
    fn missing_or_bad_inputs_are_refused_by_name() {
        assert!(check(&LightInput::default()).unwrap_err().contains("nothing to light"));
        let mut no_target = room("a", 5000.0, 4000.0, 300.0);
        no_target.target_lux = None;
        assert!(check(&input(vec![no_target])).unwrap_err().contains("target_lux is required"));
        let mut no_source = room("a", 5000.0, 4000.0, 300.0);
        no_source.target_source = Some("  ".into());
        assert!(check(&input(vec![no_source])).unwrap_err().contains("target_source is required"));
        let mut ghost = room("a", 5000.0, 4000.0, 300.0);
        ghost.luminaire = "nope".into();
        assert!(check(&input(vec![ghost])).unwrap_err().contains("not in the luminaire list"));
        for field in ["lumens", "utilisation", "maintenance"] {
            let mut inp = input(vec![room("a", 5000.0, 4000.0, 300.0)]);
            match field {
                "lumens" => inp.luminaires[0].lumens = None,
                "utilisation" => inp.luminaires[0].utilisation = None,
                _ => inp.luminaires[0].maintenance = None,
            }
            assert!(check(&inp).unwrap_err().contains(&format!("{field} is required")), "{field}");
        }
        let mut uf = input(vec![room("a", 5000.0, 4000.0, 300.0)]);
        uf.luminaires[0].utilisation = Some(1.2);
        assert!(check(&uf).unwrap_err().contains("at most 1"));
        let zero = room("a", 0.0, 4000.0, 300.0);
        assert!(check(&input(vec![zero])).unwrap_err().contains("w_mm must be a positive"));
        assert!(check(&input(vec![room("d", 3000.0, 3000.0, 100.0), room("d", 3000.0, 3000.0, 100.0)])).unwrap_err().contains("used twice"));
        let mut bad = input(vec![room("a", 5000.0, 4000.0, 300.0)]);
        bad.max_w_per_m2 = Some(-1.0);
        assert!(check(&bad).unwrap_err().contains("max_w_per_m2"));
    }
}
