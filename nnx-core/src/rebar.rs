//! Preliminary reinforcement sizing: bar counts and diameters, from your design parameters and the loads of the model.
//!
//! This is mechanics plus YOUR criteria. The code files in the project folder do not contain the strength parameters of SBC 304
//! (yield strength, minimum and maximum steel ratios, load factors, strength-reduction factors), so none is supplied here:
//! every one is an input, and a section with an input missing is named and not designed. What the files do contain, from the
//! contractor's guide to SBC 1101, is used: the least `f'c`, the exposure classes and the concrete cover (see `presets`).
//!
//! What is computed, per element:
//! - **Columns**: axial capacity `phi * alpha * (0.85 f'c (Ag - As) + fy As) >= load_factor * N`, giving `As`; at least
//!   `ratio_min * Ag`; at most `ratio_max * Ag`. Bars are the smallest of your sizes whose count fits around the section with
//!   your minimum clear spacing. Moments, slenderness and confinement are NOT checked.
//! - **Footings**: the cantilever from the column face under the factored soil pressure, flexure with `phi_flexure`, at least
//!   `ratio_min * b * h`, spacing capped by your maximum and floored by the clear spacing. Punching shear and one-way shear are
//!   NOT checked.
//! - **Beams and tie beams**: the minimum steel `ratio_min * b * d` only: bending is not designed, and the row says so.
//!
//! Every row carries the element name and id, the required and provided steel area, the ratio, the cover used and a status.

use crate::presets::{self, Exposure, Member};
use crate::sizing::{BeamBars, ColumnBars, MeshBars};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MemberExposure {
    pub column: Option<Exposure>,
    pub footing: Option<Exposure>,
    pub beam: Option<Exposure>,
    pub tie_beam: Option<Exposure>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ColumnDesign {
    pub ratio_min: Option<f64>,
    pub ratio_max: Option<f64>,
    /// Fewest longitudinal bars.
    pub min_bars: Option<u32>,
    /// Strength-reduction factor and the factor for the maximum axial capacity of a tied column.
    pub phi: Option<f64>,
    pub alpha: Option<f64>,
    pub tie_dia_mm: Option<f64>,
    pub tie_spacing_mm: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct FootingDesign {
    pub ratio_min: Option<f64>,
    pub phi_flexure: Option<f64>,
    pub max_spacing_mm: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BeamDesign {
    pub ratio_min: Option<f64>,
    pub min_top_bars: Option<u32>,
    pub stirrup_dia_mm: Option<f64>,
    pub stirrup_spacing_mm: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DesignIn {
    /// Specified compressive strength, MPa. Checked against the least value of SBC 1101 chapter 4A.
    pub fc_mpa: Option<f64>,
    /// Exposure class of the concrete (`S0`..`S3`, `C0`..`C5`): the least `f'c` of its row is checked too.
    pub exposure_class: Option<String>,
    pub fy_mpa: Option<f64>,
    /// The bar diameters available, mm.
    #[serde(default)]
    pub bar_dias_mm: Vec<f64>,
    pub min_clear_spacing_mm: Option<f64>,
    /// Factor from service to factored load.
    pub load_factor: Option<f64>,
    pub exposure: Option<MemberExposure>,
    pub column: Option<ColumnDesign>,
    pub footing: Option<FootingDesign>,
    pub beam: Option<BeamDesign>,
}

pub struct ColIn<'a> {
    pub name: &'a str,
    pub id: &'a str,
    pub b: f64,
    pub h: f64,
    pub axial_kn: Option<f64>,
}

pub struct FndIn<'a> {
    pub name: &'a str,
    pub id: &'a str,
    pub length: f64,
    pub width: f64,
    pub load_kn: f64,
    /// Section of the column it carries.
    pub col_b: f64,
    pub col_h: f64,
}

pub struct BeamIn<'a> {
    pub name: &'a str,
    pub id: Option<&'a str>,
    pub b: f64,
    pub depth: f64,
    pub tie: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DesignRow {
    pub element: String,
    pub id: Option<String>,
    pub kind: String,
    pub required_mm2: f64,
    pub provided_mm2: f64,
    pub ratio_pct: f64,
    /// For example `8 x Dia 16`.
    pub bars: String,
    pub cover_mm: f64,
    /// `ok`, `minimum steel only`, `section too small` or `no arrangement fits`.
    pub status: String,
    pub notes: String,
}

#[derive(Debug, Clone, Default)]
pub struct DesignOut {
    pub rows: Vec<DesignRow>,
    /// Bars by element name, with the cover each was designed for.
    pub columns: BTreeMap<String, (ColumnBars, f64)>,
    pub footings: BTreeMap<String, (MeshBars, f64)>,
    pub beams: BTreeMap<String, (BeamBars, f64)>,
    pub missing: Vec<String>,
    pub warnings: Vec<String>,
}

fn area(dia: f64) -> f64 {
    std::f64::consts::PI / 4.0 * dia * dia
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// The positive values a design needs, or the names of the ones missing.
struct Common {
    fc: f64,
    fy: f64,
    dias: Vec<f64>,
    clear: f64,
    gamma: f64,
}

fn common(d: &DesignIn, missing: &mut Vec<String>, warnings: &mut Vec<String>) -> Option<Common> {
    let pos = |name: &str, v: Option<f64>, missing: &mut Vec<String>| match v {
        Some(x) if x.is_finite() && x > 0.0 => Some(x),
        _ => {
            missing.push(name.to_string());
            None
        }
    };
    let (fc, fy, clear, gamma) = (pos("design.fc_mpa", d.fc_mpa, missing), pos("design.fy_mpa", d.fy_mpa, missing), pos("design.min_clear_spacing_mm", d.min_clear_spacing_mm, missing), pos("design.load_factor", d.load_factor, missing));
    let mut dias: Vec<f64> = d.bar_dias_mm.iter().copied().filter(|x| x.is_finite() && *x > 0.0).collect();
    dias.sort_by(f64::total_cmp);
    dias.dedup();
    if dias.is_empty() {
        missing.push("design.bar_dias_mm".into());
    }
    if let Some(f) = fc {
        if f < presets::FC_MIN_MPA {
            warnings.push(format!("f'c {f} MPa is below the least value {} MPa of {}", presets::FC_MIN_MPA, presets::FC_SOURCE.text()));
        }
        if let Some(name) = &d.exposure_class {
            match presets::exposure_class(name) {
                Some(c) if f < c.min_fc_mpa => warnings.push(format!("f'c {f} MPa is below the {} MPa required for exposure class {} ({})", c.min_fc_mpa, c.class, presets::EXPOSURE_SOURCE.text())),
                Some(_) => {}
                None => warnings.push(format!("unknown exposure class '{name}' (S0-S3, C0-C5)")),
            }
        }
    }
    if d.exposure.is_none() {
        missing.push("design.exposure (column, footing, beam, tie_beam: ground, weather or protected)".into());
    }
    Some(Common { fc: fc?, fy: fy?, dias, clear: clear?, gamma: gamma? }).filter(|c| !c.dias.is_empty())
}

fn exposure_of(d: &DesignIn, pick: impl Fn(&MemberExposure) -> Option<Exposure>, name: &str, missing: &mut Vec<String>) -> Option<Exposure> {
    let e = d.exposure.as_ref().and_then(pick);
    if e.is_none() && d.exposure.is_some() {
        missing.push(format!("design.exposure.{name}"));
    }
    e
}

/// Designs every element it is given. Elements whose inputs are missing are left out and the missing names are listed.
pub fn design(d: &DesignIn, footing_thickness_mm: Option<f64>, cols: &[ColIn], fnds: &[FndIn], beams: &[BeamIn]) -> DesignOut {
    let mut out = DesignOut::default();
    let Some(c) = common(d, &mut out.missing, &mut out.warnings) else {
        out.missing.sort();
        out.missing.dedup();
        return out;
    };

    // ---- columns
    if !cols.is_empty() {
        match &d.column {
            None => out.missing.push("design.column".into()),
            Some(cd) => {
                let mut miss = Vec::new();
                let pos = |n: &str, v: Option<f64>, miss: &mut Vec<String>| v.filter(|x| x.is_finite() && *x > 0.0).or_else(|| {
                    miss.push(format!("design.column.{n}"));
                    None
                });
                let (rmin, rmax, phi, alpha, tie) = (pos("ratio_min", cd.ratio_min, &mut miss), pos("ratio_max", cd.ratio_max, &mut miss), pos("phi", cd.phi, &mut miss), pos("alpha", cd.alpha, &mut miss), pos("tie_dia_mm", cd.tie_dia_mm, &mut miss));
                let spacing = pos("tie_spacing_mm", cd.tie_spacing_mm, &mut miss);
                let min_bars = cd.min_bars.filter(|n| *n > 0).or_else(|| {
                    miss.push("design.column.min_bars".into());
                    None
                });
                let exp = exposure_of(d, |e| e.column, "column", &mut miss);
                out.missing.extend(miss);
                if let (Some(rmin), Some(rmax), Some(phi), Some(alpha), Some(tie), Some(spacing), Some(min_bars), Some(exp)) = (rmin, rmax, phi, alpha, tie, spacing, min_bars, exp) {
                    if c.fy <= 0.85 * c.fc {
                        out.warnings.push("fy must exceed 0.85 f'c for the column capacity formula".into());
                    } else {
                        for col in cols {
                            let Some(n_kn) = col.axial_kn else {
                                out.missing.push(format!("{}: axial load (enter the floor loads: dead and live kPa)", col.name));
                                continue;
                            };
                            let ag = col.b * col.h;
                            let nu = c.gamma * n_kn * 1000.0;
                            let as_axial = ((nu / (phi * alpha) - 0.85 * c.fc * ag) / (c.fy - 0.85 * c.fc)).max(0.0);
                            let as_req = as_axial.max(rmin * ag);
                            let mut chosen: Option<(f64, u32, f64)> = None;
                            for &dia in &c.dias {
                                let Some(cover) = presets::cover_mm(exp, Member::Frame, dia) else { continue };
                                let n = min_bars.max((as_req / area(dia)).ceil() as u32);
                                let centre_perimeter = 2.0 * ((col.b - 2.0 * (cover + tie + dia / 2.0)) + (col.h - 2.0 * (cover + tie + dia / 2.0)));
                                let gap = centre_perimeter / n as f64 - dia;
                                if centre_perimeter > 0.0 && gap >= c.clear && n as f64 * area(dia) <= rmax * ag {
                                    chosen = Some((dia, n, cover));
                                    break;
                                }
                            }
                            let (status, dia, n, cover, notes) = match chosen {
                                Some((dia, n, cover)) => ("ok", dia, n, cover, String::new()),
                                None => {
                                    // The largest bar at the most the maximum ratio allows: flagged, not hidden.
                                    let dia = *c.dias.last().unwrap_or(&0.0);
                                    let cover = presets::cover_mm(exp, Member::Frame, dia).unwrap_or(40.0);
                                    let n = ((rmax * ag / area(dia)).floor() as u32).max(min_bars);
                                    let status = if as_req > rmax * ag { "section too small" } else { "no arrangement fits" };
                                    (status, dia, n, cover, format!("the load needs {as_req:.0} mm2 against the maximum {:.0} mm2: enlarge the section", rmax * ag))
                                }
                            };
                            let provided = n as f64 * area(dia);
                            out.columns.insert(col.name.to_string(), (ColumnBars { bar_dia_mm: dia, count: n, tie_dia_mm: tie, tie_spacing_mm: spacing }, cover));
                            out.rows.push(DesignRow {
                                element: col.name.into(),
                                id: Some(col.id.into()),
                                kind: "Column".into(),
                                required_mm2: r2(as_req),
                                provided_mm2: r2(provided),
                                ratio_pct: r2(provided / ag * 100.0),
                                bars: format!("{n} x Dia {dia}"),
                                cover_mm: cover,
                                status: status.into(),
                                notes: if notes.is_empty() { "axial load only: moments, slenderness and confinement are not checked".into() } else { notes },
                            });
                        }
                    }
                }
            }
        }
    }

    // ---- footings
    if !fnds.is_empty() {
        match &d.footing {
            None => out.missing.push("design.footing".into()),
            Some(fd) => {
                let mut miss = Vec::new();
                let pos = |n: &str, v: Option<f64>, miss: &mut Vec<String>| v.filter(|x| x.is_finite() && *x > 0.0).or_else(|| {
                    miss.push(format!("design.footing.{n}"));
                    None
                });
                let (rmin, phi, maxs) = (pos("ratio_min", fd.ratio_min, &mut miss), pos("phi_flexure", fd.phi_flexure, &mut miss), pos("max_spacing_mm", fd.max_spacing_mm, &mut miss));
                let thick = match footing_thickness_mm.filter(|t| *t > 0.0) {
                    Some(t) => Some(t),
                    None => {
                        miss.push("footing_thickness_mm".into());
                        None
                    }
                };
                let exp = exposure_of(d, |e| e.footing, "footing", &mut miss);
                out.missing.extend(miss);
                if let (Some(rmin), Some(phi), Some(maxs), Some(thick), Some(exp)) = (rmin, phi, maxs, thick, exp) {
                    for f in fnds {
                        let qu = c.gamma * f.load_kn / (f.length * f.width / 1e6);
                        let mut chosen: Option<(f64, f64, f64, f64)> = None;
                        let mut last = (0.0, 0.0);
                        for &dia in &c.dias {
                            // A footing is cast against the ground (75 mm); one in the open or protected takes the frame rows of the table.
                            let Some(cover) = presets::cover_mm(exp, Member::Frame, dia) else { continue };
                            let d_eff = thick - cover - dia / 2.0;
                            if d_eff <= 0.0 {
                                continue;
                            }
                            // The larger cantilever of the two directions governs a square mesh.
                            let a_mm = ((f.length - f.col_b) / 2.0).max((f.width - f.col_h) / 2.0).max(0.0);
                            let mu = qu * (a_mm / 1000.0).powi(2) / 2.0 * 1e6; // N.mm per 1000 mm width
                            let (aa, bb) = (phi * c.fy * c.fy / (1.7 * c.fc * 1000.0), -phi * c.fy * d_eff);
                            let disc = bb * bb - 4.0 * aa * mu;
                            if disc < 0.0 {
                                last = (cover, dia);
                                continue;
                            }
                            let as_flex = (-bb - disc.sqrt()) / (2.0 * aa);
                            let as_req = as_flex.max(rmin * 1000.0 * thick);
                            let spacing = ((area(dia) * 1000.0 / as_req / 10.0).floor() * 10.0).min(maxs);
                            if spacing >= dia + c.clear {
                                chosen = Some((dia, spacing, cover, as_req));
                                break;
                            }
                            last = (cover, dia);
                        }
                        let (status, dia, spacing, cover, as_req, note) = match chosen {
                            Some((dia, s, cover, a)) => ("ok", dia, s, cover, a, "flexure of the cantilever only: punching and one-way shear are not checked".to_string()),
                            None => {
                                let dia = if last.1 > 0.0 { last.1 } else { *c.dias.last().unwrap_or(&0.0) };
                                ("section too small", dia, dia + c.clear, last.0, 0.0, "no bar of your sizes carries the cantilever moment within the thickness and spacing limits: thicken the footing".to_string())
                            }
                        };
                        let provided = if spacing > 0.0 { area(dia) * 1000.0 / spacing } else { 0.0 };
                        out.footings.insert(f.name.to_string(), (MeshBars { bar_dia_mm: dia, spacing_mm: spacing }, cover));
                        out.rows.push(DesignRow {
                            element: f.name.into(),
                            id: Some(f.id.into()),
                            kind: "Footing".into(),
                            required_mm2: r2(as_req),
                            provided_mm2: r2(provided),
                            ratio_pct: r2(provided / (1000.0 * thick) * 100.0),
                            bars: format!("Dia {dia} @ {spacing} each way"),
                            cover_mm: cover,
                            status: status.into(),
                            notes: note,
                        });
                    }
                }
            }
        }
    }

    // ---- beams and tie beams: minimum steel
    if !beams.is_empty() {
        match &d.beam {
            None => out.missing.push("design.beam".into()),
            Some(bd) => {
                let mut miss = Vec::new();
                let pos = |n: &str, v: Option<f64>, miss: &mut Vec<String>| v.filter(|x| x.is_finite() && *x > 0.0).or_else(|| {
                    miss.push(format!("design.beam.{n}"));
                    None
                });
                let (rmin, sdia, sspc) = (pos("ratio_min", bd.ratio_min, &mut miss), pos("stirrup_dia_mm", bd.stirrup_dia_mm, &mut miss), pos("stirrup_spacing_mm", bd.stirrup_spacing_mm, &mut miss));
                let top = bd.min_top_bars.filter(|n| *n > 0).or_else(|| {
                    miss.push("design.beam.min_top_bars".into());
                    None
                });
                let (eb, et) = (exposure_of(d, |e| e.beam, "beam", &mut miss), if beams.iter().any(|b| b.tie) { exposure_of(d, |e| e.tie_beam, "tie_beam", &mut miss) } else { None });
                out.missing.extend(miss);
                if let (Some(rmin), Some(sdia), Some(sspc), Some(top)) = (rmin, sdia, sspc, top) {
                    for b in beams {
                        let Some(exp) = (if b.tie { et } else { eb }) else { continue };
                        let mut chosen: Option<(f64, u32, f64, f64)> = None;
                        for &dia in &c.dias {
                            let Some(cover) = presets::cover_mm(exp, Member::Frame, dia) else { continue };
                            let d_eff = b.depth - cover - sdia - dia / 2.0;
                            if d_eff <= 0.0 {
                                continue;
                            }
                            let as_min = rmin * b.b * d_eff;
                            let n = 2.max((as_min / area(dia)).ceil() as u32);
                            let gap = if n > 1 { (b.b - 2.0 * (cover + sdia) - n as f64 * dia) / (n as f64 - 1.0) } else { f64::MAX };
                            if gap >= c.clear {
                                chosen = Some((dia, n, cover, as_min));
                                break;
                            }
                        }
                        let (status, dia, n, cover, as_min) = match chosen {
                            Some((dia, n, cover, a)) => ("minimum steel only", dia, n, cover, a),
                            None => ("no arrangement fits", *c.dias.last().unwrap_or(&0.0), 2, presets::cover_mm(exp, Member::Frame, *c.dias.last().unwrap_or(&20.0)).unwrap_or(40.0), 0.0),
                        };
                        out.beams.insert(b.name.to_string(), (BeamBars { top_dia_mm: dia, top_count: top, bottom_dia_mm: dia, bottom_count: n, stirrup_dia_mm: sdia, stirrup_spacing_mm: sspc }, cover));
                        out.rows.push(DesignRow {
                            element: b.name.into(),
                            id: b.id.map(str::to_string),
                            kind: if b.tie { "TieBeam".into() } else { "Beam".into() },
                            required_mm2: r2(as_min),
                            provided_mm2: r2(n as f64 * area(dia)),
                            ratio_pct: r2(n as f64 * area(dia) / (b.b * b.depth) * 100.0),
                            bars: format!("{n} x Dia {dia} bottom, {top} x Dia {dia} top"),
                            cover_mm: cover,
                            status: status.into(),
                            notes: "minimum steel only: bending is not designed".into(),
                        });
                    }
                }
            }
        }
    }
    out.missing.sort();
    out.missing.dedup();
    out
}
