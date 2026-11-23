//! Explosive thermal spalling screen for concrete structural elements exposed to fire.
//!
//! Spalling risk depends on moisture, concrete strength, cover and whether the mix carries mitigation (polypropylene fibres).
//! The thresholds that separate "likely" from "unlikely" belong to a design code, and none of them was read from a source in
//! this project. So every threshold is an input the caller supplies with its own source; nothing is assumed. A missing threshold
//! or a missing measured value leaves that check `unchecked`, never `ok`.
//!
//! Per element three trigger checks run against the caller's limits:
//! - moisture content (percent by mass) must not exceed `max_moisture_pct`;
//! - compressive strength `fc_mpa` must not exceed `max_fc_mpa` (the strength above which the caller treats the mix as high
//!   strength for spalling purposes);
//! - nominal cover `cover_mm` must not exceed `max_cover_mm` (a large cover needs surface reinforcement the caller must detail).
//!
//! A failed trigger is `fails` unless the mix carries fibres at or above `min_pp_fibre_kg_m3` (caller's limit), in which case the
//! element is `mitigated` and the report says the mitigation is only as good as the limit the caller entered. When no trigger
//! fails the element is `ok` only if every trigger was actually checked; otherwise `unchecked`.
//!
//! This is a preliminary screen. It does not compute heat transfer, section temperatures, fire resistance rating or residual
//! strength, and it does not replace a fire-engineering assessment or the approving authority.

use serde::{Deserialize, Serialize};

const KINDS: [&str; 5] = ["column", "beam", "slab", "wall", "footing"];

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SpallElement {
    pub key: String,
    /// `column`, `beam`, `slab`, `wall` or `footing`.
    pub kind: String,
    pub b_mm: f64,
    pub h_mm: f64,
    /// Nominal cover to the outermost reinforcement, mm.
    pub cover_mm: f64,
    /// Specified compressive strength, MPa.
    pub fc_mpa: f64,
    /// Number of faces exposed to the fire, 1 to 4.
    pub exposed_sides: u8,
    /// Moisture content, percent by mass, measured or declared by the caller. Absent: the moisture check is `unchecked`.
    #[serde(default)]
    pub moisture_pct: Option<f64>,
    /// Polypropylene fibre dosage, kg/m3. Absent: no mitigation is credited.
    #[serde(default)]
    pub pp_fibre_kg_m3: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SpallInput {
    pub elements: Vec<SpallElement>,
    /// Highest acceptable moisture content, percent by mass. Absent: moisture is `unchecked`.
    #[serde(default)]
    pub max_moisture_pct: Option<f64>,
    /// Highest strength not treated as high strength, MPa. Absent: strength is `unchecked`.
    #[serde(default)]
    pub max_fc_mpa: Option<f64>,
    /// Largest cover not needing surface reinforcement, mm. Absent: cover is `unchecked`.
    #[serde(default)]
    pub max_cover_mm: Option<f64>,
    /// Smallest fibre dosage credited as mitigation, kg/m3. Absent: no mitigation is credited.
    #[serde(default)]
    pub min_pp_fibre_kg_m3: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpallRow {
    pub key: String,
    pub kind: String,
    pub exposed_sides: u8,
    pub moisture_status: &'static str,
    pub strength_status: &'static str,
    pub cover_status: &'static str,
    /// Evaluated only when a trigger fails: `ok`, `fails` or `unchecked`; `not_needed` when no trigger failed.
    pub mitigation_status: &'static str,
    pub status: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SpallCounts {
    pub ok: u32,
    pub mitigated: u32,
    pub fails: u32,
    pub unchecked: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub ar: String,
    pub en: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpallReport {
    pub elements: Vec<SpallRow>,
    pub counts: SpallCounts,
    pub findings: Vec<Finding>,
}

fn positive(v: f64, field: &str, key: &str) -> Result<f64, String> {
    if v.is_finite() && v > 0.0 {
        Ok(v)
    } else {
        Err(format!("'{key}': {field} must be a positive number, got {v}"))
    }
}

fn non_negative(v: f64, field: &str, key: &str) -> Result<f64, String> {
    if v.is_finite() && v >= 0.0 {
        Ok(v)
    } else {
        Err(format!("'{key}': {field} must be zero or more, got {v}"))
    }
}

/// `value <= limit` is `ok`; a missing value or a missing limit is `unchecked`.
fn at_most(value: Option<f64>, limit: Option<f64>) -> &'static str {
    match (value, limit) {
        (Some(v), Some(l)) if v <= l => "ok",
        (Some(_), Some(_)) => "fails",
        _ => "unchecked",
    }
}

fn row(e: &SpallElement, inp: &SpallInput) -> Result<SpallRow, String> {
    if e.key.trim().is_empty() {
        return Err("an element has an empty key".into());
    }
    if !KINDS.contains(&e.kind.as_str()) {
        return Err(format!("element '{}': kind '{}' is not one of {}", e.key, e.kind, KINDS.join(", ")));
    }
    let b = positive(e.b_mm, "b_mm", &e.key)?;
    let h = positive(e.h_mm, "h_mm", &e.key)?;
    let cover = positive(e.cover_mm, "cover_mm", &e.key)?;
    let fc = positive(e.fc_mpa, "fc_mpa", &e.key)?;
    if cover * 2.0 >= b.min(h) {
        return Err(format!("element '{}': cover_mm {cover} leaves no core in a {b} x {h} mm section", e.key));
    }
    if !(1..=4).contains(&e.exposed_sides) {
        return Err(format!("element '{}': exposed_sides must be 1 to 4, got {}", e.key, e.exposed_sides));
    }
    let moisture = e.moisture_pct.map(|m| non_negative(m, "moisture_pct", &e.key)).transpose()?;
    if moisture.is_some_and(|m| m > 100.0) {
        return Err(format!("element '{}': moisture_pct cannot exceed 100", e.key));
    }
    let fibre = e.pp_fibre_kg_m3.map(|f| non_negative(f, "pp_fibre_kg_m3", &e.key)).transpose()?;

    let moisture_status = at_most(moisture, inp.max_moisture_pct);
    let strength_status = at_most(Some(fc), inp.max_fc_mpa);
    let cover_status = at_most(Some(cover), inp.max_cover_mm);
    let triggers = [moisture_status, strength_status, cover_status];
    let any_failed = triggers.contains(&"fails");
    let any_unchecked = triggers.contains(&"unchecked");

    let mitigation_status = if any_failed {
        match (fibre, inp.min_pp_fibre_kg_m3) {
            (Some(f), Some(min)) if f >= min => "ok",
            (Some(_), Some(_)) => "fails",
            _ => "unchecked",
        }
    } else {
        "not_needed"
    };
    let status = match (any_failed, mitigation_status, any_unchecked) {
        (true, "ok", _) => "mitigated",
        (true, _, _) => "fails",
        (false, _, true) => "unchecked",
        (false, _, false) => "ok",
    };
    Ok(SpallRow { key: e.key.clone(), kind: e.kind.clone(), exposed_sides: e.exposed_sides, moisture_status, strength_status, cover_status, mitigation_status, status })
}

pub fn check(inp: &SpallInput) -> Result<SpallReport, String> {
    if inp.elements.is_empty() {
        return Err("nothing to check: give at least one element".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for e in &inp.elements {
        if !seen.insert(e.key.as_str()) {
            return Err(format!("element key '{}' is used twice", e.key));
        }
    }
    let limits = [
        ("max_moisture_pct", inp.max_moisture_pct, false),
        ("max_fc_mpa", inp.max_fc_mpa, true),
        ("max_cover_mm", inp.max_cover_mm, true),
        ("min_pp_fibre_kg_m3", inp.min_pp_fibre_kg_m3, false),
    ];
    for (name, v, strictly_positive) in limits {
        if let Some(v) = v {
            if strictly_positive {
                positive(v, name, "input")?;
            } else {
                non_negative(v, name, "input")?;
            }
        }
    }

    let elements = inp.elements.iter().map(|e| row(e, inp)).collect::<Result<Vec<_>, _>>()?;
    let mut counts = SpallCounts::default();
    for r in &elements {
        match r.status {
            "ok" => counts.ok += 1,
            "mitigated" => counts.mitigated += 1,
            "fails" => counts.fails += 1,
            _ => counts.unchecked += 1,
        }
    }

    let mut findings = Vec::new();
    if inp.max_moisture_pct.is_none() {
        findings.push(Finding { ar: "لم يُدخل حد الرطوبة (max_moisture_pct) فلم يُحكم على الرطوبة بنجاح أو فشل.".into(), en: "No max_moisture_pct given, so no pass or fail is declared for moisture.".into() });
    }
    if inp.max_fc_mpa.is_none() {
        findings.push(Finding { ar: "لم يُدخل حد المقاومة (max_fc_mpa) فلم يُحكم على المقاومة بنجاح أو فشل.".into(), en: "No max_fc_mpa given, so no pass or fail is declared for strength.".into() });
    }
    if inp.max_cover_mm.is_none() {
        findings.push(Finding { ar: "لم يُدخل حد الغطاء (max_cover_mm) فلم يُحكم على الغطاء بنجاح أو فشل.".into(), en: "No max_cover_mm given, so no pass or fail is declared for cover.".into() });
    }
    for (r, e) in elements.iter().zip(&inp.elements) {
        if e.moisture_pct.is_none() {
            findings.push(Finding { ar: format!("العنصر «{}»: لم تُدخل رطوبة الخرسانة فلم تُفحص.", e.key), en: format!("Element '{}': no moisture_pct given, so moisture is not checked.", e.key) });
        }
        match r.status {
            "fails" if r.mitigation_status == "unchecked" => findings.push(Finding {
                ar: format!("العنصر «{}»: تجاوز حداً ولا يمكن الاعتداد بالألياف لغياب جرعتها أو حدها الأدنى (pp_fibre_kg_m3 وmin_pp_fibre_kg_m3).", e.key),
                en: format!("Element '{}': a limit is exceeded and no mitigation can be credited because the fibre dosage or its minimum limit is missing (pp_fibre_kg_m3 and min_pp_fibre_kg_m3).", e.key),
            }),
            "fails" => findings.push(Finding {
                ar: format!("العنصر «{}»: تجاوز حداً وجرعة الألياف دون الحد الأدنى المُدخل.", e.key),
                en: format!("Element '{}': a limit is exceeded and the fibre dosage is below the minimum entered.", e.key),
            }),
            "mitigated" => findings.push(Finding {
                ar: format!("العنصر «{}»: تجاوز حداً لكن جرعة الألياف تبلغ الحد الأدنى المُدخل؛ كفاية التخفيف مرهونة بذلك الحد ومصدره وليست مُثبتة هنا.", e.key),
                en: format!("Element '{}': a limit is exceeded but the fibre dosage meets the minimum entered; the adequacy of the mitigation rests on that limit and its source and is not proven here.", e.key),
            }),
            _ => {}
        }
    }
    findings.push(Finding {
        ar: "فحص أولي بحدود من مدخلاتك: لا انتقال حراري ولا درجات حرارة للمقطع ولا درجة مقاومة حريق ولا مقاومة متبقية، ولا يغني عن تقييم هندسة الحريق أو الجهة المعتمدة.".into(),
        en: "Preliminary screen against limits you entered: no heat transfer, section temperatures, fire-resistance rating or residual strength, and it does not replace a fire-engineering assessment or the approving authority.".into(),
    });
    Ok(SpallReport { elements, counts, findings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(key: &str) -> SpallElement {
        SpallElement { key: key.into(), kind: "column".into(), b_mm: 400.0, h_mm: 400.0, cover_mm: 40.0, fc_mpa: 40.0, exposed_sides: 4, moisture_pct: Some(2.0), pp_fibre_kg_m3: None }
    }
    fn limits(elements: Vec<SpallElement>) -> SpallInput {
        SpallInput { elements, max_moisture_pct: Some(3.0), max_fc_mpa: Some(60.0), max_cover_mm: Some(70.0), min_pp_fibre_kg_m3: Some(2.0) }
    }

    #[test]
    fn all_triggers_within_limits_is_ok() {
        let r = check(&limits(vec![el("c1")])).unwrap();
        assert_eq!(r.elements[0].status, "ok");
        assert_eq!(r.elements[0].mitigation_status, "not_needed");
        assert_eq!(r.counts, SpallCounts { ok: 1, mitigated: 0, fails: 0, unchecked: 0 });
    }

    #[test]
    fn a_failed_trigger_fails_unless_fibres_reach_the_entered_minimum() {
        let mut wet = el("wet");
        wet.moisture_pct = Some(4.5);
        let mut fibre_low = wet.clone();
        fibre_low.key = "low".into();
        fibre_low.pp_fibre_kg_m3 = Some(1.0);
        let mut fibre_ok = wet.clone();
        fibre_ok.key = "fib".into();
        fibre_ok.pp_fibre_kg_m3 = Some(2.0);
        let r = check(&limits(vec![wet, fibre_low, fibre_ok])).unwrap();
        assert_eq!((r.elements[0].status, r.elements[0].mitigation_status), ("fails", "unchecked"));
        assert_eq!((r.elements[1].status, r.elements[1].mitigation_status), ("fails", "fails"));
        assert_eq!((r.elements[2].status, r.elements[2].mitigation_status), ("mitigated", "ok"));
        assert_eq!(r.counts, SpallCounts { ok: 0, mitigated: 1, fails: 2, unchecked: 0 });
    }

    #[test]
    fn strength_and_cover_each_trigger_independently() {
        let mut strong = el("hs");
        strong.fc_mpa = 90.0;
        let mut deep = el("cv");
        deep.cover_mm = 80.0;
        let r = check(&limits(vec![strong, deep])).unwrap();
        assert_eq!((r.elements[0].strength_status, r.elements[0].cover_status, r.elements[0].status), ("fails", "ok", "fails"));
        assert_eq!((r.elements[1].strength_status, r.elements[1].cover_status, r.elements[1].status), ("ok", "fails", "fails"));
    }

    #[test]
    fn missing_limit_or_value_is_unchecked_never_ok() {
        let mut no_moisture = el("m");
        no_moisture.moisture_pct = None;
        let r = check(&limits(vec![no_moisture])).unwrap();
        assert_eq!((r.elements[0].moisture_status, r.elements[0].status), ("unchecked", "unchecked"));
        assert!(r.findings.iter().any(|f| f.en.contains("no moisture_pct")));

        let r = check(&SpallInput { elements: vec![el("c")], ..Default::default() }).unwrap();
        assert_eq!(r.elements[0].status, "unchecked");
        assert_eq!(r.counts.unchecked, 1);
        assert_eq!(r.findings.iter().filter(|f| f.en.starts_with("No max_")).count(), 3);
    }

    #[test]
    fn a_failure_outranks_an_unchecked_trigger() {
        let mut e = el("x");
        e.moisture_pct = None;
        e.fc_mpa = 90.0;
        assert_eq!(check(&limits(vec![e])).unwrap().elements[0].status, "fails");
    }

    #[test]
    fn bad_inputs_are_refused_by_name() {
        assert!(check(&SpallInput::default()).unwrap_err().contains("nothing to check"));
        let mut kind = el("k");
        kind.kind = "stair".into();
        assert!(check(&limits(vec![kind])).unwrap_err().contains("kind 'stair'"));
        let mut cover = el("c");
        cover.cover_mm = 200.0;
        assert!(check(&limits(vec![cover])).unwrap_err().contains("leaves no core"));
        let mut sides = el("s");
        sides.exposed_sides = 5;
        assert!(check(&limits(vec![sides])).unwrap_err().contains("exposed_sides"));
        let mut fc = el("f");
        fc.fc_mpa = 0.0;
        assert!(check(&limits(vec![fc])).unwrap_err().contains("fc_mpa must be a positive"));
        let mut m = el("m");
        m.moisture_pct = Some(101.0);
        assert!(check(&limits(vec![m])).unwrap_err().contains("moisture_pct"));
        assert!(check(&limits(vec![el("d"), el("d")])).unwrap_err().contains("used twice"));
        let mut bad = limits(vec![el("c")]);
        bad.max_fc_mpa = Some(-1.0);
        assert!(check(&bad).unwrap_err().contains("max_fc_mpa"));
    }
}
