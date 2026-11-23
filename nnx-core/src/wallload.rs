//! Masonry/partition wall loads on the beams that carry them.
//!
//! A wall standing on a beam is a line load: `w = thickness * height * unit_weight + face_finish * faces * height`
//! (kN/m). Openings (doors, windows) carry no wall, so each opening's interval is cut out of the line load.
//! Every wall whose line lies on a beam's axis and overlaps it adds a piecewise-uniform load on that beam. For each beam this
//! module returns the total load, the two end reactions, the maximum shear and the maximum bending moment of a
//! **simply supported** span (an idealization stated in every result: continuity, fixed ends and the beam's own weight
//! are not included), and the equivalent uniform line load.
//!
//! - Nothing is assumed: the unit weight of the wall material, the wall height and thickness, and the finish load are inputs
//!   with a stated source. A missing or non-positive one is refused by name and nothing is computed.
//! - Walls that rest on no beam are listed (`unsupported`), never silently dropped: they bear on a slab or on the ground
//!   and need their own check.
//! - Output is a load take-down for the member design (`rebar`, `sizing`); it is not a design and not a code check.

use serde::{Deserialize, Serialize};

/// Collinearity tolerance between a wall axis and a beam axis, mm.
const AXIS_TOL_MM: i64 = 50;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Opening {
    /// Distance from the wall's `a` end to the opening's start, mm.
    pub offset_mm: i64,
    pub width_mm: i64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WallIn {
    pub key: String,
    pub a: (i64, i64),
    pub b: (i64, i64),
    pub thickness_mm: Option<i64>,
    pub height_mm: Option<i64>,
    /// Unit weight of the wall material, kN/m3 (block, brick, ...). Source required by the caller; no default.
    pub unit_weight_kn_m3: Option<f64>,
    /// Plaster/finish weight per face, kN/m2 of wall face. Omitted means the caller states there is none (0).
    #[serde(default)]
    pub finish_kn_m2_per_face: f64,
    /// Number of finished faces (0, 1 or 2).
    #[serde(default)]
    pub finished_faces: u32,
    #[serde(default)]
    pub openings: Vec<Opening>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct BeamIn {
    pub key: String,
    pub a: (i64, i64),
    pub b: (i64, i64),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Contribution {
    pub wall: String,
    /// Line load of the solid wall, kN/m.
    pub line_load_kn_m: f64,
    /// Length of wall resting on the beam after cutting openings, mm.
    pub loaded_length_mm: i64,
    /// Load this wall puts on the beam, kN.
    pub load_kn: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BeamLoad {
    pub beam: String,
    pub span_mm: i64,
    pub contributions: Vec<Contribution>,
    pub total_kn: f64,
    /// Reaction at the beam's `a` end and `b` end, kN (simply supported).
    pub reaction_a_kn: f64,
    pub reaction_b_kn: f64,
    pub max_shear_kn: f64,
    pub max_moment_knm: f64,
    /// Total load spread evenly over the span, kN/m.
    pub equivalent_udl_kn_m: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub ar: String,
    pub en: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WallLoadReport {
    pub model: String,
    pub beams: Vec<BeamLoad>,
    /// Walls with no beam under any part of them.
    pub unsupported: Vec<String>,
    pub total_wall_load_kn: f64,
    pub findings: Vec<Finding>,
}

/// Wall axis as a 1-D coordinate system: returns (horizontal?, fixed line coordinate, lo, hi).
fn axis(a: (i64, i64), b: (i64, i64), what: &str, key: &str) -> Result<(bool, i64, i64, i64), String> {
    if a == b {
        return Err(format!("{what} '{key}' has zero length"));
    }
    if a.1 == b.1 {
        Ok((true, a.1, a.0.min(b.0), a.0.max(b.0)))
    } else if a.0 == b.0 {
        Ok((false, a.0, a.1.min(b.1), a.1.max(b.1)))
    } else {
        Err(format!("{what} '{key}' is not parallel to an axis; walls and beams here are orthogonal and a skew line is not approximated"))
    }
}

fn need<T: Copy>(v: Option<T>, field: &str, key: &str) -> Result<T, String> {
    v.ok_or_else(|| format!("wall '{key}': {field} is required; it is an input with a stated source and nothing is assumed"))
}

/// Wall's loaded intervals as `(from, to, w)` in the shared axis coordinate, openings cut out.
fn loaded_intervals(w: &WallIn, lo: i64, hi: i64, a_is_lo: bool) -> Result<(f64, Vec<(i64, i64)>), String> {
    let t = need(w.thickness_mm, "thickness_mm", &w.key)?;
    let h = need(w.height_mm, "height_mm", &w.key)?;
    let g = need(w.unit_weight_kn_m3, "unit_weight_kn_m3", &w.key)?;
    if t <= 0 || h <= 0 {
        return Err(format!("wall '{}': thickness_mm and height_mm must be positive, got {t} and {h}", w.key));
    }
    if !(g.is_finite() && g > 0.0) {
        return Err(format!("wall '{}': unit_weight_kn_m3 must be a positive number, got {g}", w.key));
    }
    if !(w.finish_kn_m2_per_face.is_finite() && w.finish_kn_m2_per_face >= 0.0) {
        return Err(format!("wall '{}': finish_kn_m2_per_face must be zero or more", w.key));
    }
    if w.finished_faces > 2 {
        return Err(format!("wall '{}': finished_faces is 0, 1 or 2, got {}", w.key, w.finished_faces));
    }
    let hm = h as f64 / 1000.0;
    let line = (t as f64 / 1000.0) * hm * g + w.finish_kn_m2_per_face * w.finished_faces as f64 * hm;
    let len = hi - lo;
    let mut cuts: Vec<(i64, i64)> = Vec::new();
    for o in &w.openings {
        if o.width_mm <= 0 || o.offset_mm < 0 || o.offset_mm + o.width_mm > len {
            return Err(format!("wall '{}': opening at {} mm, {} mm wide does not lie inside the {} mm wall", w.key, o.offset_mm, o.width_mm, len));
        }
        let (s, e) = if a_is_lo { (lo + o.offset_mm, lo + o.offset_mm + o.width_mm) } else { (hi - o.offset_mm - o.width_mm, hi - o.offset_mm) };
        cuts.push((s, e));
    }
    cuts.sort();
    for p in cuts.windows(2) {
        if p[1].0 < p[0].1 {
            return Err(format!("wall '{}': openings overlap", w.key));
        }
    }
    let mut solid = Vec::new();
    let mut at = lo;
    for (s, e) in cuts {
        if s > at {
            solid.push((at, s));
        }
        at = e;
    }
    if hi > at {
        solid.push((at, hi));
    }
    Ok((line, solid))
}

/// Shear and moment of a simply supported span under piecewise-uniform loads `(from, to, w kN/m)` measured from the `a` end (m).
fn simple_span(span_m: f64, loads: &[(f64, f64, f64)]) -> (f64, f64, f64, f64) {
    let total: f64 = loads.iter().map(|(s, e, w)| (e - s) * w).sum();
    let moment_about_a: f64 = loads.iter().map(|(s, e, w)| (e - s) * w * (s + e) / 2.0).sum();
    let rb = moment_about_a / span_m;
    let ra = total - rb;
    // Shear is piecewise linear: check it at every load boundary and find the zero crossing inside each piece.
    let mut xs: Vec<f64> = vec![0.0, span_m];
    for (s, e, _) in loads {
        xs.push(*s);
        xs.push(*e);
    }
    xs.sort_by(|a, b| a.partial_cmp(b).expect("finite coordinates"));
    xs.dedup();
    let shear_at = |x: f64| -> f64 { ra - loads.iter().map(|(s, e, w)| w * (x.clamp(*s, *e) - s)).sum::<f64>() };
    let moment_at = |x: f64| -> f64 {
        let mut m = ra * x;
        for (s, e, w) in loads {
            let upto = x.min(*e);
            if upto > *s {
                m -= w * (upto - s) * (x - (s + upto) / 2.0);
            }
        }
        m
    };
    let mut max_v = ra.abs().max(rb.abs());
    let mut max_m = 0.0_f64;
    for pair in xs.windows(2) {
        let (x0, x1) = (pair[0], pair[1]);
        let v0 = shear_at(x0 + (x1 - x0) * 1e-9);
        let v1 = shear_at(x1 - (x1 - x0) * 1e-9);
        max_v = max_v.max(v0.abs()).max(v1.abs());
        max_m = max_m.max(moment_at(x0)).max(moment_at(x1));
        if v0 > 0.0 && v1 < 0.0 {
            let x = x0 + (x1 - x0) * v0 / (v0 - v1);
            max_m = max_m.max(moment_at(x));
        }
    }
    (ra, rb, max_v, max_m)
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// Takes the wall loads down onto the beams.
pub fn take_down(walls: &[WallIn], beams: &[BeamIn]) -> Result<WallLoadReport, String> {
    if walls.is_empty() {
        return Err("give at least one wall".to_string());
    }
    if beams.is_empty() {
        return Err("give at least one beam".to_string());
    }
    let mut keys = std::collections::BTreeSet::new();
    for k in walls.iter().map(|w| &w.key).chain(beams.iter().map(|b| &b.key)) {
        if !keys.insert(k.clone()) {
            return Err(format!("key '{k}' is used twice; wall and beam keys must be unique"));
        }
    }
    struct Wax {
        horizontal: bool,
        line: i64,
        w: f64,
        solid: Vec<(i64, i64)>,
    }
    let mut wax = Vec::new();
    for w in walls {
        let (hz, line, lo, hi) = axis(w.a, w.b, "wall", &w.key)?;
        let a_is_lo = if hz { w.a.0 < w.b.0 } else { w.a.1 < w.b.1 };
        let (lw, solid) = loaded_intervals(w, lo, hi, a_is_lo)?;
        wax.push(Wax { horizontal: hz, line, w: lw, solid });
    }
    let mut supported = vec![false; walls.len()];
    let mut out = Vec::new();
    for b in beams {
        let (hz, line, lo, hi) = axis(b.a, b.b, "beam", &b.key)?;
        let span_mm = hi - lo;
        let b_a_is_lo = if hz { b.a.0 < b.b.0 } else { b.a.1 < b.b.1 };
        let mut loads: Vec<(f64, f64, f64)> = Vec::new();
        let mut contributions = Vec::new();
        for (i, (w, spec)) in wax.iter().zip(walls).enumerate() {
            if w.horizontal != hz || (w.line - line).abs() > AXIS_TOL_MM {
                continue;
            }
            let mut loaded = 0_i64;
            for &(s, e) in &w.solid {
                let (s, e) = (s.max(lo), e.min(hi));
                if e > s {
                    loaded += e - s;
                    // Measure from the beam's `a` end.
                    let (from, to) = if b_a_is_lo { (s - lo, e - lo) } else { (hi - e, hi - s) };
                    loads.push((from as f64 / 1000.0, to as f64 / 1000.0, w.w));
                }
            }
            if loaded > 0 {
                supported[i] = true;
                contributions.push(Contribution { wall: spec.key.clone(), line_load_kn_m: round3(w.w), loaded_length_mm: loaded, load_kn: round3(w.w * loaded as f64 / 1000.0) });
            }
        }
        let span_m = span_mm as f64 / 1000.0;
        let total: f64 = loads.iter().map(|(s, e, w)| (e - s) * w).sum();
        let (ra, rb, v, m) = simple_span(span_m, &loads);
        out.push(BeamLoad {
            beam: b.key.clone(),
            span_mm,
            contributions,
            total_kn: round3(total),
            reaction_a_kn: round3(ra),
            reaction_b_kn: round3(rb),
            max_shear_kn: round3(v),
            max_moment_knm: round3(m),
            equivalent_udl_kn_m: round3(total / span_m),
        });
    }
    let unsupported: Vec<String> = walls.iter().zip(&supported).filter(|(_, s)| !**s).map(|(w, _)| w.key.clone()).collect();
    let total_wall_load_kn = round3(out.iter().map(|b| b.total_kn).sum());
    let mut findings = vec![Finding {
        ar: "النموذج: كمرة بسيطة الإسناد تحت أحمال خطية متجزئة؛ بلا استمرارية ولا وزن ذاتي للكمرة ولا معاملات حمل. المخرج تحميل للتصميم وليس تصميماً.".into(),
        en: "Model: simply supported beam under piecewise line loads; no continuity, no beam self-weight, no load factors. The output is a load take-down for member design, not a design.".into(),
    }];
    for u in &unsupported {
        findings.push(Finding {
            ar: format!("الجدار «{u}» لا تحته كمرة: يستند إلى بلاطة أو إلى الأرض ويحتاج فحصاً مستقلاً."),
            en: format!("Wall '{u}' has no beam under it: it bears on a slab or on the ground and needs its own check."),
        });
    }
    Ok(WallLoadReport { model: "simply_supported_piecewise_uniform".into(), beams: out, unsupported, total_wall_load_kn, findings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall(key: &str, a: (i64, i64), b: (i64, i64)) -> WallIn {
        WallIn { key: key.into(), a, b, thickness_mm: Some(200), height_mm: Some(3000), unit_weight_kn_m3: Some(14.0), finish_kn_m2_per_face: 0.0, finished_faces: 0, openings: vec![] }
    }
    fn beam(key: &str, a: (i64, i64), b: (i64, i64)) -> BeamIn {
        BeamIn { key: key.into(), a, b }
    }
    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn a_full_length_wall_gives_wl_squared_over_eight() {
        // 0.2 * 3.0 * 14 = 8.4 kN/m on a 5 m span: R = 21, M = 8.4*25/8 = 26.25.
        let r = take_down(&[wall("w", (0, 0), (5000, 0))], &[beam("b", (0, 0), (5000, 0))]).unwrap();
        let b = &r.beams[0];
        assert!(near(b.total_kn, 42.0) && near(b.reaction_a_kn, 21.0) && near(b.reaction_b_kn, 21.0));
        assert!(near(b.max_moment_knm, 26.25) && near(b.max_shear_kn, 21.0) && near(b.equivalent_udl_kn_m, 8.4));
        assert!(r.unsupported.is_empty());
    }

    #[test]
    fn finish_on_both_faces_adds_to_the_line_load() {
        let mut w = wall("w", (0, 0), (4000, 0));
        w.finish_kn_m2_per_face = 0.5;
        w.finished_faces = 2;
        let r = take_down(&[w], &[beam("b", (0, 0), (4000, 0))]).unwrap();
        assert!(near(r.beams[0].contributions[0].line_load_kn_m, 8.4 + 3.0));
    }

    #[test]
    fn an_opening_removes_its_length_and_shifts_the_reactions() {
        // 4 m wall, 1 m opening at the b end: load on 0..3 m only. R_b = w*3*1.5/4, R_a = w*3 - R_b.
        let mut w = wall("w", (0, 0), (4000, 0));
        w.openings = vec![Opening { offset_mm: 3000, width_mm: 1000 }];
        let r = take_down(&[w], &[beam("b", (0, 0), (4000, 0))]).unwrap();
        let b = &r.beams[0];
        assert!(near(b.total_kn, 25.2));
        assert!(near(b.reaction_b_kn, 8.4 * 3.0 * 1.5 / 4.0) && near(b.reaction_a_kn, 25.2 - 9.45));
        // Zero shear at x = R_a / w = 1.78 m: M = R_a x - w x^2 / 2.
        let x = b.reaction_a_kn / 8.4;
        assert!(near(b.max_moment_knm, b.reaction_a_kn * x - 8.4 * x * x / 2.0));
    }

    #[test]
    fn reversed_wall_direction_puts_the_opening_at_the_same_place() {
        let mut fwd = wall("w", (0, 0), (4000, 0));
        fwd.openings = vec![Opening { offset_mm: 0, width_mm: 1000 }];
        let mut rev = wall("w", (4000, 0), (0, 0));
        rev.openings = vec![Opening { offset_mm: 3000, width_mm: 1000 }];
        let a = take_down(&[fwd], &[beam("b", (0, 0), (4000, 0))]).unwrap();
        let b = take_down(&[rev], &[beam("b", (0, 0), (4000, 0))]).unwrap();
        assert_eq!(a.beams[0], b.beams[0]);
    }

    #[test]
    fn a_short_wall_on_part_of_a_beam_is_a_partial_load() {
        let r = take_down(&[wall("w", (1000, 0), (3000, 0))], &[beam("b", (0, 0), (4000, 0))]).unwrap();
        let b = &r.beams[0];
        assert!(near(b.total_kn, 16.8) && near(b.reaction_a_kn, 8.4) && near(b.reaction_b_kn, 8.4));
        assert_eq!(b.contributions[0].loaded_length_mm, 2000);
    }

    #[test]
    fn a_wall_with_no_beam_is_listed_unsupported() {
        let r = take_down(&[wall("on", (0, 0), (4000, 0)), wall("off", (0, 3000), (4000, 3000))], &[beam("b", (0, 0), (4000, 0))]).unwrap();
        assert_eq!(r.unsupported, vec!["off".to_string()]);
        assert!(r.findings.iter().any(|f| f.en.contains("'off'")));
    }

    #[test]
    fn vertical_walls_and_beams_match_on_their_own_axis() {
        let r = take_down(&[wall("w", (2000, 0), (2000, 4000))], &[beam("b", (2000, 4000), (2000, 0)), beam("other", (0, 0), (0, 4000))]).unwrap();
        assert!(near(r.beams[0].total_kn, 33.6));
        assert!(near(r.beams[1].total_kn, 0.0));
    }

    #[test]
    fn missing_or_bad_inputs_are_refused_by_name() {
        let b = [beam("b", (0, 0), (4000, 0))];
        let mut w = wall("w", (0, 0), (4000, 0));
        w.unit_weight_kn_m3 = None;
        assert!(take_down(&[w], &b).unwrap_err().contains("unit_weight_kn_m3 is required"));
        let mut w = wall("w", (0, 0), (4000, 0));
        w.height_mm = None;
        assert!(take_down(&[w], &b).unwrap_err().contains("height_mm is required"));
        let mut w = wall("w", (0, 0), (4000, 0));
        w.thickness_mm = Some(0);
        assert!(take_down(&[w], &b).unwrap_err().contains("must be positive"));
        assert!(take_down(&[wall("w", (0, 0), (4000, 1000))], &b).unwrap_err().contains("not parallel"));
        let mut w = wall("w", (0, 0), (4000, 0));
        w.openings = vec![Opening { offset_mm: 3500, width_mm: 1000 }];
        assert!(take_down(&[w], &b).unwrap_err().contains("does not lie inside"));
        assert!(take_down(&[wall("x", (0, 0), (1, 0))], &[beam("x", (0, 0), (1, 0))]).unwrap_err().contains("used twice"));
        assert!(take_down(&[], &b).is_err());
    }
}
