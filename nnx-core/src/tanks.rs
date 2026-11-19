//! Underground water and septic tanks in reinforced concrete: wall, base and top slab thicknesses, reinforcement and its schedule, for
//! the soil pressure on the walls and the groundwater uplift on the base.
//!
//! WHAT THIS IS: a preliminary sizing with textbook mechanics (earth pressure `K0 * (gamma z + q)`, hydrostatic pressure, strip
//! moments, the rectangular stress block `Mu = phi As fy (d - As fy / (1.7 f'c b))`). NONE of its coefficients is read from the code
//! files (SBC 304 is not among them): the soil, the groundwater, the load factors, phi, f'c, fy, the reinforcement limits and the
//! bar sizes are all your inputs, and a missing one is named. The soil and groundwater of a coastal site such as Al Jubail come from
//! YOUR soil report: the application proposes no values for any city.
//!
//! Cases: the EMPTY tank with soil and groundwater outside (walls simply supported between the slabs, base against uplift), and the
//! FULL tank with liquid inside and no soil (walls cantilevering from the base). The wall and the base are designed for the larger
//! moment. The top slab carries its own weight, the soil over it and the surcharge, simply supported over the short inner side.
//! Uplift is checked for the empty tank: weight of the structure and of the soil over it against `gammaW * head * outer area`.
//! A tank whose thickness you leave empty is sized upwards in 50 mm steps until the section works.
//!
//! The reinforcement is the same on both faces: the main bars across the short span and distribution bars at the minimum ratio.
//! A septic tank with several compartments gets partition walls with the minimum reinforcement.

use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

const GAMMA_W_FRESH: f64 = 9.81;
const STEEL_KG_M3: f64 = 7850.0;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TankIn {
    pub name: String,
    /// `water` or `septic`.
    pub kind: String,
    pub inner_length_mm: Option<f64>,
    pub inner_width_mm: Option<f64>,
    /// Depth of the liquid when full.
    pub liquid_depth_mm: Option<f64>,
    /// Clear height above the liquid, up to the underside of the top slab.
    pub freeboard_mm: Option<f64>,
    /// Soil over the top slab.
    pub soil_cover_mm: Option<f64>,
    /// Depth of the groundwater below the ground surface; absent: no groundwater.
    pub groundwater_depth_mm: Option<f64>,
    pub soil_unit_weight_kn_m3: Option<f64>,
    /// Lateral earth-pressure coefficient.
    pub k0: Option<f64>,
    pub surcharge_kpa: Option<f64>,
    pub liquid_unit_weight_kn_m3: Option<f64>,
    /// Unit weight of the groundwater; fresh water 9.81 when absent.
    pub groundwater_unit_weight_kn_m3: Option<f64>,
    /// Septic tank compartments (1 = a single chamber).
    pub compartments: u32,
    pub wall_mm: Option<f64>,
    pub base_mm: Option<f64>,
    pub top_mm: Option<f64>,
}

impl TankIn {
    pub fn check(&self) -> Result<(), String> {
        if !["water", "septic", ""].contains(&self.kind.as_str()) {
            return Err(format!("tank '{}': kind is water or septic", self.name));
        }
        for (n, v) in [("inner length", self.inner_length_mm), ("inner width", self.inner_width_mm), ("liquid depth", self.liquid_depth_mm), ("soil unit weight", self.soil_unit_weight_kn_m3), ("k0", self.k0), ("liquid unit weight", self.liquid_unit_weight_kn_m3), ("groundwater unit weight", self.groundwater_unit_weight_kn_m3), ("wall thickness", self.wall_mm), ("base thickness", self.base_mm), ("top thickness", self.top_mm)] {
            if v.is_some_and(|x| !x.is_finite() || x <= 0.0) {
                return Err(format!("tank '{}': {n} must be positive", self.name));
            }
        }
        for (n, v) in [("freeboard", self.freeboard_mm), ("soil cover", self.soil_cover_mm), ("groundwater depth", self.groundwater_depth_mm), ("surcharge", self.surcharge_kpa)] {
            if v.is_some_and(|x| !x.is_finite() || x < 0.0) {
                return Err(format!("tank '{}': {n} must be zero or more", self.name));
            }
        }
        if self.compartments > 6 {
            return Err(format!("tank '{}': at most 6 compartments", self.name));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TankDesignIn {
    pub fc_mpa: Option<f64>,
    pub fy_mpa: Option<f64>,
    pub phi_flexure: Option<f64>,
    /// Load factors on soil, water, liquid and dead load.
    pub gamma_soil: Option<f64>,
    pub gamma_water: Option<f64>,
    pub gamma_liquid: Option<f64>,
    pub gamma_dead: Option<f64>,
    pub concrete_unit_weight_kn_m3: Option<f64>,
    /// Minimum and maximum reinforcement ratio (of b x h and b x d).
    pub rho_min: Option<f64>,
    pub rho_max: Option<f64>,
    pub cover_mm: Option<f64>,
    /// Bar diameters available, mm.
    pub bar_dias_mm: Vec<f64>,
    pub min_spacing_mm: Option<f64>,
    pub max_spacing_mm: Option<f64>,
    /// Required safety factor against uplift.
    pub uplift_fs_min: Option<f64>,
    /// Smallest thickness tried when a thickness is left to the design, mm.
    pub min_thickness_mm: Option<f64>,
    /// Length added to each bar for the hooks, mm.
    pub hook_mm: Option<f64>,
}

impl TankDesignIn {
    pub fn check(&self) -> Result<(), String> {
        for (n, v) in [("f'c", self.fc_mpa), ("fy", self.fy_mpa), ("phi", self.phi_flexure), ("soil load factor", self.gamma_soil), ("water load factor", self.gamma_water), ("liquid load factor", self.gamma_liquid), ("dead load factor", self.gamma_dead), ("concrete unit weight", self.concrete_unit_weight_kn_m3), ("rho min", self.rho_min), ("rho max", self.rho_max), ("cover", self.cover_mm), ("min spacing", self.min_spacing_mm), ("max spacing", self.max_spacing_mm), ("uplift safety factor", self.uplift_fs_min), ("minimum thickness", self.min_thickness_mm)] {
            if v.is_some_and(|x| !x.is_finite() || x <= 0.0) {
                return Err(format!("tank design: {n} must be positive"));
            }
        }
        if self.bar_dias_mm.iter().any(|d| !d.is_finite() || *d <= 0.0) {
            return Err("tank design: bar diameters must be positive".into());
        }
        if let (Some(a), Some(b)) = (self.rho_min, self.rho_max)
            && a >= b
        {
            return Err("tank design: rho min must be less than rho max".into());
        }
        if let (Some(a), Some(b)) = (self.min_spacing_mm, self.max_spacing_mm)
            && a > b
        {
            return Err("tank design: min spacing must not exceed max spacing".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ElementDesign {
    pub role: &'static str,
    pub thickness_mm: f64,
    pub d_mm: f64,
    pub mu_kn_m: f64,
    pub as_required_mm2_m: f64,
    pub as_min_mm2_m: f64,
    pub bar_dia_mm: f64,
    pub spacing_mm: f64,
    pub as_provided_mm2_m: f64,
    pub sized_by: &'static str,
    pub status: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct TankBar {
    pub mark: String,
    pub role: String,
    pub dia_mm: f64,
    pub spacing_mm: f64,
    pub number: u64,
    pub cut_length_mm: f64,
    pub total_m: f64,
    pub kg: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TankResult {
    pub name: String,
    pub kind: String,
    pub inner_mm: [f64; 3],
    pub outer_mm: [f64; 3],
    pub depth_below_ground_m: f64,
    pub elements: Vec<ElementDesign>,
    pub wall_moment_empty_kn_m: f64,
    pub wall_moment_full_kn_m: f64,
    pub uplift_kn: f64,
    pub resisting_kn: f64,
    pub uplift_fs: Option<f64>,
    pub extra_base_mm: f64,
    pub bars: Vec<TankBar>,
    pub concrete_m3: f64,
    pub steel_kg: f64,
    /// `ok`, `uplift` (the safety factor is not reached with the thickness you gave) or `section too small`.
    pub status: &'static str,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
}

/// A bar and spacing for a moment: `(dia, spacing, as_req, as_prov, d)`, or why none was found.
type Sec = Result<(f64, f64, f64, f64, f64), &'static str>;

/// A base-slab trial: its section, the depth below ground, the uplift, the resisting weight, the design moment and the uplift moment.
type Trial = (Sec, f64, f64, f64, f64, f64);

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Required steel area (mm2 per m) for `mu` kN.m/m, or `None` when the section is too small for the stress block.
pub fn as_for(mu_knm: f64, d: f64, fc: f64, fy: f64, phi: f64) -> Option<f64> {
    let rn = mu_knm * 1e6 / (phi * 1000.0 * d * d);
    let inner = 1.0 - 2.0 * rn / (0.85 * fc);
    if inner < 0.0 {
        return None;
    }
    let rho = 0.85 * fc / fy * (1.0 - inner.sqrt());
    Some(rho * 1000.0 * d)
}

struct Dsn<'a> {
    fc: f64,
    fy: f64,
    phi: f64,
    cover: f64,
    rho_min: f64,
    rho_max: f64,
    dias: &'a [f64],
    smin: f64,
    smax: f64,
}

/// A bar and spacing for the moment at thickness `h`: `(dia, spacing, as_req, as_prov, d)` or why not.
fn section(mu: f64, h: f64, d: &Dsn) -> Result<(f64, f64, f64, f64, f64), &'static str> {
    let mut why = "no bar size fits at the minimum spacing";
    for &dia in d.dias {
        let eff = h - d.cover - dia / 2.0;
        if eff <= 0.0 {
            why = "the thickness leaves no effective depth";
            continue;
        }
        let Some(req) = as_for(mu, eff, d.fc, d.fy, d.phi) else { return Err("section too small") };
        if req > d.rho_max * 1000.0 * eff + 1e-9 {
            return Err("section too small");
        }
        let req = req.max(d.rho_min * 1000.0 * h);
        let ab = PI / 4.0 * dia * dia;
        let s = ((1000.0 * ab / req) / 25.0).floor() * 25.0;
        if s < d.smin {
            continue;
        }
        let s = s.min(d.smax);
        return Ok((dia, s, req, 1000.0 * ab / s, eff));
    }
    Err(why)
}

/// Moment (kN.m/m) of a simply supported strip of span `hi` under `p(y)` (kPa), maximum along the span.
fn ss_moment(hi: f64, p: &dyn Fn(f64) -> f64) -> f64 {
    let n = 400;
    let dy = hi / n as f64;
    let ys: Vec<f64> = (0..n).map(|i| (i as f64 + 0.5) * dy).collect();
    let r0: f64 = ys.iter().map(|&y| p(y) * (hi - y) / hi * dy).sum();
    let mut best = 0.0f64;
    for k in 1..=n {
        let yk = k as f64 * dy;
        let m: f64 = r0 * yk - ys.iter().filter(|&&s| s < yk).map(|&s| p(s) * (yk - s) * dy).sum::<f64>();
        best = best.max(m);
    }
    best
}

fn bar(rows: &mut Vec<TankBar>, mark: &str, role: &str, dia: f64, spacing: f64, number: u64, cut_mm: f64) {
    if number == 0 || cut_mm <= 0.0 {
        return;
    }
    let total = number as f64 * cut_mm / 1000.0;
    rows.push(TankBar { mark: mark.into(), role: role.into(), dia_mm: dia, spacing_mm: spacing, number, cut_length_mm: cut_mm.round(), total_m: r2(total), kg: r2(PI / 4.0 * (dia / 1000.0).powi(2) * total * STEEL_KG_M3) });
}

pub fn design(t: &TankIn, d: &TankDesignIn) -> TankResult {
    let mut missing: Vec<String> = Vec::new();
    fn need(missing: &mut Vec<String>, n: &str, ok: bool) {
        if !ok {
            missing.push(n.to_string());
        }
    }
    need(&mut missing, "inner length (mm)", t.inner_length_mm.is_some());
    need(&mut missing, "inner width (mm)", t.inner_width_mm.is_some());
    need(&mut missing, "liquid depth (mm)", t.liquid_depth_mm.is_some());
    need(&mut missing, "freeboard (mm)", t.freeboard_mm.is_some());
    need(&mut missing, "soil cover over the top slab (mm)", t.soil_cover_mm.is_some());
    need(&mut missing, "soil unit weight (kN/m3)", t.soil_unit_weight_kn_m3.is_some());
    need(&mut missing, "lateral earth-pressure coefficient k0", t.k0.is_some());
    need(&mut missing, "surcharge (kPa)", t.surcharge_kpa.is_some());
    need(&mut missing, "liquid unit weight (kN/m3)", t.liquid_unit_weight_kn_m3.is_some());
    for (n, v) in [("f'c", d.fc_mpa), ("fy", d.fy_mpa), ("phi", d.phi_flexure), ("soil load factor", d.gamma_soil), ("water load factor", d.gamma_water), ("liquid load factor", d.gamma_liquid), ("dead load factor", d.gamma_dead), ("concrete unit weight", d.concrete_unit_weight_kn_m3), ("rho min", d.rho_min), ("rho max", d.rho_max), ("cover", d.cover_mm), ("min bar spacing", d.min_spacing_mm), ("max bar spacing", d.max_spacing_mm), ("uplift safety factor", d.uplift_fs_min), ("hook length", d.hook_mm)] {
        need(&mut missing, &format!("design: {n}"), v.is_some());
    }
    if d.bar_dias_mm.is_empty() {
        missing.push("design: bar diameters".into());
    }
    if (t.wall_mm.is_none() || t.base_mm.is_none() || t.top_mm.is_none()) && d.min_thickness_mm.is_none() {
        missing.push("design: smallest thickness to try (a thickness is left to the design)".into());
    }
    let mut res = TankResult { name: t.name.clone(), kind: t.kind.clone(), inner_mm: [0.0; 3], outer_mm: [0.0; 3], depth_below_ground_m: 0.0, elements: vec![], wall_moment_empty_kn_m: 0.0, wall_moment_full_kn_m: 0.0, uplift_kn: 0.0, resisting_kn: 0.0, uplift_fs: None, extra_base_mm: 0.0, bars: vec![], concrete_m3: 0.0, steel_kg: 0.0, status: "ok", missing: missing.clone(), notes: vec![] };
    if !missing.is_empty() {
        return res;
    }
    let (li, wi, hl, fb, cs, gs, k0, q, gl) = (t.inner_length_mm.unwrap_or(0.0) / 1000.0, t.inner_width_mm.unwrap_or(0.0) / 1000.0, t.liquid_depth_mm.unwrap_or(0.0) / 1000.0, t.freeboard_mm.unwrap_or(0.0) / 1000.0, t.soil_cover_mm.unwrap_or(0.0) / 1000.0, t.soil_unit_weight_kn_m3.unwrap_or(0.0), t.k0.unwrap_or(0.0), t.surcharge_kpa.unwrap_or(0.0), t.liquid_unit_weight_kn_m3.unwrap_or(0.0));
    let gw = t.groundwater_unit_weight_kn_m3.unwrap_or(GAMMA_W_FRESH);
    let zw = t.groundwater_depth_mm.map(|v| v / 1000.0);
    let hi = hl + fb;
    let (gam_s, gam_w, gam_l, gam_d, gc) = (d.gamma_soil.unwrap_or(0.0), d.gamma_water.unwrap_or(0.0), d.gamma_liquid.unwrap_or(0.0), d.gamma_dead.unwrap_or(0.0), d.concrete_unit_weight_kn_m3.unwrap_or(0.0));
    let dsn = Dsn { fc: d.fc_mpa.unwrap_or(0.0), fy: d.fy_mpa.unwrap_or(0.0), phi: d.phi_flexure.unwrap_or(0.0), cover: d.cover_mm.unwrap_or(0.0), rho_min: d.rho_min.unwrap_or(0.0), rho_max: d.rho_max.unwrap_or(0.0), dias: &d.bar_dias_mm, smin: d.min_spacing_mm.unwrap_or(0.0), smax: d.max_spacing_mm.unwrap_or(0.0) };
    let start = d.min_thickness_mm.unwrap_or(150.0);
    let short_in = li.min(wi);

    // Moments of the wall: the empty tank (soil and water outside) and the full one (liquid inside).
    let soil_p = |y: f64, tt_m: f64, tb_m: f64| -> f64 {
        let _ = tb_m;
        let z = cs + tt_m + (hi - y);
        let (zdry, zwet) = match zw {
            Some(w) => (z.min(w), (z - w).max(0.0)),
            None => (z, 0.0),
        };
        let gprime = gs - gw;
        gam_s * k0 * (gs * zdry + gprime * zwet + q) + gam_w * gw * zwet
    };
    let tw_try = t.wall_mm.unwrap_or(start);
    let tt_try = t.top_mm.unwrap_or(start);
    let m_full = {
        // cantilever from the base: sum of p(s) * s ds
        let n = 400;
        let ds = hl / n as f64;
        (0..n).map(|i| (i as f64 + 0.5) * ds).map(|s| gam_l * gl * (hl - s) * s * ds).sum::<f64>()
    };
    let m_empty_at = |tt_m: f64| ss_moment(hi, &|y| soil_p(y, tt_m, 0.0));
    let mu_wall = |tt_m: f64| m_empty_at(tt_m).max(m_full);

    // Wall thickness.
    let wall_res = |tw: f64, tt_m: f64| section(mu_wall(tt_m), tw, &dsn);
    let mut tw = tw_try;
    let mut wall = wall_res(tw, tt_try / 1000.0);
    let wall_auto = t.wall_mm.is_none();
    while wall_auto && wall.is_err() && tw < 1500.0 {
        tw += 50.0;
        wall = wall_res(tw, tt_try / 1000.0);
    }
    // Top slab.
    let top_res = |tt: f64| {
        let w = gam_d * gc * tt / 1000.0 + gam_s * gs * cs + gam_s * q;
        section(w * short_in * short_in / 8.0, tt, &dsn).map(|s| (s, w * short_in * short_in / 8.0))
    };
    let mut tt = tt_try;
    let mut top = top_res(tt);
    while t.top_mm.is_none() && top.is_err() && tt < 1500.0 {
        tt += 50.0;
        top = top_res(tt);
    }
    // Base slab and uplift: the thickness changes the depth, the weight and the uplift.
    let (lo_of, wo_of) = (|tw: f64| li + 2.0 * tw / 1000.0, |tw: f64| wi + 2.0 * tw / 1000.0);
    let evaluate = |tb: f64| -> Trial {
        let (lo, wo) = (lo_of(tw), wo_of(tw));
        let depth = cs + tt / 1000.0 + hi + tb / 1000.0;
        let head = zw.map_or(0.0, |w| (depth - w).max(0.0));
        let uplift = gw * head * lo * wo;
        let walls = (lo * wo - li * wi) * hi;
        let parts = (t.compartments.max(1) - 1) as f64 * wi * tw / 1000.0 * hi;
        let slabs = lo * wo * (tb + tt) / 1000.0;
        let soil_w = lo * wo * cs * match zw {
            Some(w) if w < cs => gs - gw,
            _ => gs,
        };
        let resisting = gc * (walls + parts + slabs) + soil_w;
        let w_up = (gam_w * gw * head - gc * tb / 1000.0).max(0.0);
        let m_up = w_up * short_in * short_in / 8.0;
        let mu_b = m_up.max(mu_wall(tt / 1000.0));
        (section(mu_b, tb, &dsn).map(|s| (s.0, s.1, s.2, s.3, s.4)), depth, uplift, resisting, mu_b, m_up)
    };
    let fs_min = d.uplift_fs_min.unwrap_or(0.0);
    let ok_fs = |u: f64, r: f64| u <= 0.0 || r / u + 1e-9 >= fs_min;
    let mut tb = t.base_mm.unwrap_or(start);
    let mut ev = evaluate(tb);
    let base_auto = t.base_mm.is_none();
    while base_auto && (ev.0.is_err() || !ok_fs(ev.2, ev.3)) && tb < 1500.0 {
        tb += 50.0;
        ev = evaluate(tb);
    }
    let (base_sec, depth, uplift, resisting, mu_base, _) = ev;
    let fs = if uplift > 0.0 { Some(r2(resisting / uplift)) } else { None };
    // Thickening the base adds weight gc * A per metre and uplift gw * A per metre: it helps only while gc > fs * gw.
    let den = gc - fs_min * gw;
    let short_of_fs = uplift > 0.0 && resisting / uplift < fs_min;
    let extra = if short_of_fs && den > 1e-9 { ((fs_min * uplift - resisting) / (lo_of(tw) * wo_of(tw) * den) * 1000.0 / 50.0).ceil() * 50.0 } else { 0.0 };
    let (lo, wo) = (lo_of(tw), wo_of(tw));
    res.inner_mm = [li * 1000.0, wi * 1000.0, hi * 1000.0];
    res.outer_mm = [lo * 1000.0, wo * 1000.0, hi * 1000.0 + tb + tt];
    res.depth_below_ground_m = r2(depth);
    res.wall_moment_empty_kn_m = r2(m_empty_at(tt / 1000.0));
    res.wall_moment_full_kn_m = r2(m_full);
    res.uplift_kn = r2(uplift);
    res.resisting_kn = r2(resisting);
    res.uplift_fs = fs;
    res.extra_base_mm = extra;
    let mut status = "ok";
    let describe = |role: &'static str, h: f64, mu: f64, s: &Result<(f64, f64, f64, f64, f64), &'static str>, auto: bool| -> (ElementDesign, bool) {
        let sized_by = if auto { "design" } else { "your thickness" };
        match s {
            Ok((dia, sp, req, prov, eff)) => (ElementDesign { role, thickness_mm: h, d_mm: r2(*eff), mu_kn_m: r2(mu), as_required_mm2_m: r2(*req), as_min_mm2_m: r2(dsn.rho_min * 1000.0 * h), bar_dia_mm: *dia, spacing_mm: *sp, as_provided_mm2_m: r2(*prov), sized_by, status: "ok" }, true),
            Err(why) => (ElementDesign { role, thickness_mm: h, d_mm: 0.0, mu_kn_m: r2(mu), as_required_mm2_m: 0.0, as_min_mm2_m: r2(dsn.rho_min * 1000.0 * h), bar_dia_mm: 0.0, spacing_mm: 0.0, as_provided_mm2_m: 0.0, sized_by, status: why }, false),
        }
    };
    let wall_s = wall.map(|s| (s.0, s.1, s.2, s.3, s.4));
    let (wd, wok) = describe("wall", tw, mu_wall(tt / 1000.0), &wall_s, wall_auto);
    let top_s = top.as_ref().map(|(s, _)| (s.0, s.1, s.2, s.3, s.4)).map_err(|e| *e);
    let top_mu = top.as_ref().map(|(_, m)| *m).unwrap_or(0.0);
    let (td, tok) = describe("top slab", tt, top_mu, &top_s, t.top_mm.is_none());
    let (bd, bok) = describe("base slab", tb, mu_base, &base_sec, base_auto);
    if !(wok && tok && bok) {
        status = "section too small";
    } else if !ok_fs(uplift, resisting) {
        status = "uplift";
    }
    // The reinforcement schedule: main bars across the short span, distribution bars at the minimum ratio; both faces.
    let (cover, hook) = (dsn.cover, d.hook_mm.unwrap_or(0.0));
    let mut bars = Vec::new();
    if wok && tok && bok {
        let (wdia, wsp) = (wd.bar_dia_mm, wd.spacing_mm);
        let per = ((2.0 * (lo + wo) - 4.0 * tw / 1000.0) * 1000.0 / wsp).ceil() as u64;
        bar(&mut bars, "W1", "wall vertical (each face)", wdia, wsp, per * 2, hi * 1000.0 + tb + tt - 2.0 * cover + 2.0 * hook);
        let dd = dsn.dias.iter().cloned().fold(f64::MAX, f64::min);
        // Distribution bars: the smallest bar at the spacing that gives the minimum ratio.
        let dist = |h: f64| -> (f64, f64) {
            let ab = PI / 4.0 * dd * dd;
            let s = ((1000.0 * ab / (dsn.rho_min * 1000.0 * h)) / 25.0).floor() * 25.0;
            (dd, s.clamp(dsn.smin, dsn.smax))
        };
        let (hd, hs) = dist(tw);
        let ring = 2.0 * (lo * 1000.0 - 2.0 * cover) + 2.0 * (wo * 1000.0 - 2.0 * cover) + 2.0 * hook;
        bar(&mut bars, "W2", "wall horizontal (each face)", hd, hs, (hi * 1000.0 / hs).ceil() as u64 * 2, ring);
        let (bdia, bsp) = (bd.bar_dia_mm, bd.spacing_mm);
        let (long_o, short_o) = (lo.max(wo) * 1000.0, lo.min(wo) * 1000.0);
        bar(&mut bars, "B1", "base main (each face)", bdia, bsp, (long_o / bsp).ceil() as u64 * 2, short_o - 2.0 * cover + 2.0 * hook);
        let (bd2, bs2) = dist(tb);
        bar(&mut bars, "B2", "base distribution (each face)", bd2, bs2, (short_o / bs2).ceil() as u64 * 2, long_o - 2.0 * cover + 2.0 * hook);
        let (tdia, tsp) = (td.bar_dia_mm, td.spacing_mm);
        bar(&mut bars, "T1", "top main (each face)", tdia, tsp, (long_o / tsp).ceil() as u64 * 2, short_o - 2.0 * cover + 2.0 * hook);
        let (td2, ts2) = dist(tt);
        bar(&mut bars, "T2", "top distribution (each face)", td2, ts2, (short_o / ts2).ceil() as u64 * 2, long_o - 2.0 * cover + 2.0 * hook);
        if t.compartments > 1 {
            let n = (t.compartments - 1) as u64;
            let (pd, ps) = dist(tw);
            bar(&mut bars, "P1", "partition vertical (each face)", pd, ps, ((wi * 1000.0 / ps).ceil() as u64 + 1) * 2 * n, hi * 1000.0 - 2.0 * cover + 2.0 * hook);
            bar(&mut bars, "P2", "partition horizontal (each face)", pd, ps, (hi * 1000.0 / ps).ceil() as u64 * 2 * n, wi * 1000.0 - 2.0 * cover + 2.0 * hook);
        }
    }
    let parts_vol = (t.compartments.max(1) - 1) as f64 * wi * tw / 1000.0 * hi;
    res.concrete_m3 = r2((lo * wo - li * wi) * hi + lo * wo * (tb + tt) / 1000.0 + parts_vol);
    res.steel_kg = r2(bars.iter().map(|b| b.kg).sum());
    res.bars = bars;
    res.elements = vec![wd, bd, td];
    res.status = status;
    if status == "uplift" {
        if den > 1e-9 {
            res.notes.push(format!("the safety factor {} is below {fs_min}: thicken the base by about {} mm, or lower the groundwater", fs.unwrap_or(0.0), extra));
        } else {
            res.notes.push(format!("the safety factor {} is below {fs_min} and thickening the base cannot reach it (concrete weighs less than {fs_min} x the water): add weight over the tank, anchor it or lower the groundwater", fs.unwrap_or(0.0)));
        }
    }
    if zw.is_none() {
        res.notes.push("no groundwater depth entered: no uplift and no water pressure outside".into());
    }
    if t.groundwater_unit_weight_kn_m3.is_none() {
        res.notes.push("groundwater unit weight not entered: fresh water 9.81 kN/m3 was used".into());
    }
    res.notes.push("preliminary: textbook mechanics with your inputs; not from the code text; a licensed structural engineer must verify".into());
    res
}
