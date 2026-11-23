//! Concrete and steel savings: compares structural alternatives on the quantities their members imply.
//!
//! Each alternative is a list of prismatic members (columns, beams, slabs, footings) with a section, a length and the bars they
//! carry. From those alone the module counts, per alternative:
//! - concrete volume `count * b * h * length` (m3);
//! - bar length `count * bar_count * bar_length`, with links `floor(length / spacing) + 1` per member, and the steel mass
//!   `length * pi/4 * d^2 * 7850 kg/m3` (the same bar mass as `bbs`).
//!
//! It then ranks the alternatives on the criterion YOU name (`steel_kg`, `concrete_m3` or `cost`) and reports the saving of each
//! against a baseline alternative in absolute and percentage terms.
//!
//! - No quantity, price, productivity or code value is supplied here. The criterion has no default. `cost` needs both unit prices,
//!   each with its source and date, and without them the comparison is refused by name. A price is never read from memory.
//! - Bar lengths are the lengths you give (a bar set without `length_mm` takes the member length): laps, hooks and waste are not
//!   added, because their lengths come from code tables not read here (see `bbs` and `cutting` for cut lengths).
//! - This is a comparison of quantities, not a design. An alternative saves material only if it still passes its own design
//!   checks (`sizing`, `rebar`, deflection, punching). Mark the ones that failed with `checks_passed: false` and they are listed
//!   but never ranked or chosen. An alternative with `checks_passed` absent is ranked and flagged as unverified.

use serde::{Deserialize, Serialize};

/// Mass of one cubic metre of steel, the value `bbs` uses for bar weights.
const STEEL_KG_M3: f64 = 7850.0;

/// A set of identical straight bars in a member.
#[derive(Debug, Clone, Deserialize)]
pub struct BarSet {
    pub dia_mm: Option<f64>,
    /// Bars per member.
    pub count: Option<u32>,
    /// Cut length of one bar, mm; absent means the member length.
    pub length_mm: Option<f64>,
}

/// Links (stirrups or ties) along a member.
#[derive(Debug, Clone, Deserialize)]
pub struct LinkSet {
    pub dia_mm: Option<f64>,
    pub spacing_mm: Option<f64>,
    /// Cut length of one link, mm.
    pub length_mm: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MemberIn {
    pub key: String,
    /// A label for the breakdown: `column`, `beam`, `slab`, `footing`, or your own.
    pub kind: String,
    /// Identical members of this kind; absent means one.
    pub count: Option<u32>,
    pub b_mm: Option<f64>,
    pub h_mm: Option<f64>,
    pub length_mm: Option<f64>,
    #[serde(default)]
    pub bars: Vec<BarSet>,
    pub links: Option<LinkSet>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AlternativeIn {
    pub label: String,
    pub members: Vec<MemberIn>,
    /// `Some(false)`: the alternative failed its design checks and is never ranked. `None`: not stated, ranked and flagged.
    pub checks_passed: Option<bool>,
}

/// A unit price with where it came from and when.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Price {
    pub value: Option<f64>,
    pub source: Option<String>,
    pub date: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Prices {
    pub concrete_per_m3: Option<Price>,
    pub steel_per_kg: Option<Price>,
    pub currency: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Criterion {
    SteelKg,
    ConcreteM3,
    Cost,
}

impl Criterion {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "steel_kg" => Ok(Self::SteelKg),
            "concrete_m3" => Ok(Self::ConcreteM3),
            "cost" => Ok(Self::Cost),
            other => Err(format!("criterion '{other}' is unknown: use steel_kg, concrete_m3 or cost")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KindTotal {
    pub kind: String,
    pub concrete_m3: f64,
    pub steel_kg: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Saving {
    /// Baseline minus this alternative; positive means this alternative uses less.
    pub abs: f64,
    /// `abs / baseline * 100`; `None` when the baseline quantity is zero.
    pub pct: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AlternativeResult {
    pub index: usize,
    pub label: String,
    pub concrete_m3: f64,
    pub steel_kg: f64,
    /// Steel per cubic metre of concrete; `None` when there is no concrete.
    pub steel_kg_per_m3: Option<f64>,
    pub cost: Option<f64>,
    pub by_kind: Vec<KindTotal>,
    pub checks_passed: Option<bool>,
    pub saving_concrete: Saving,
    pub saving_steel: Saving,
    pub saving_cost: Option<Saving>,
    /// 1 = best on the criterion among eligible alternatives; `None` for one that failed its checks.
    pub rank: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub ar: String,
    pub en: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SavingsReport {
    pub criterion: Criterion,
    pub currency: Option<String>,
    pub baseline: usize,
    pub alternatives: Vec<AlternativeResult>,
    /// Index of the alternative ranked first; `None` when none is eligible.
    pub best: Option<usize>,
    pub findings: Vec<Finding>,
}

fn pos(name: &str, v: Option<f64>) -> Result<f64, String> {
    match v {
        Some(x) if x.is_finite() && x > 0.0 => Ok(x),
        Some(x) => Err(format!("{name} must be a positive number, not {x}")),
        None => Err(format!("{name} is required: nothing is assumed")),
    }
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn bar_kg(dia_mm: f64, length_mm: f64) -> f64 {
    std::f64::consts::PI / 4.0 * (dia_mm / 1000.0).powi(2) * (length_mm / 1000.0) * STEEL_KG_M3
}

/// Concrete (m3) and steel (kg) of one member line (all `count` members).
fn member_quantities(alt: &str, m: &MemberIn) -> Result<(f64, f64), String> {
    let at = |what: &str| format!("alternative '{alt}', member '{}': {what}", m.key);
    let count = match m.count {
        Some(0) => return Err(at("count must be at least 1")),
        Some(n) => n as f64,
        None => 1.0,
    };
    let b = pos("b_mm", m.b_mm).map_err(|e| at(&e))?;
    let h = pos("h_mm", m.h_mm).map_err(|e| at(&e))?;
    let len = pos("length_mm", m.length_mm).map_err(|e| at(&e))?;
    let concrete = count * b / 1000.0 * h / 1000.0 * len / 1000.0;
    let mut steel = 0.0;
    for (i, bs) in m.bars.iter().enumerate() {
        let dia = pos(&format!("bars[{i}].dia_mm"), bs.dia_mm).map_err(|e| at(&e))?;
        let n = bs.count.filter(|n| *n > 0).ok_or_else(|| at(&format!("bars[{i}].count is required and at least 1")))?;
        let l = match bs.length_mm {
            Some(l) => pos(&format!("bars[{i}].length_mm"), Some(l)).map_err(|e| at(&e))?,
            None => len,
        };
        steel += count * n as f64 * bar_kg(dia, l);
    }
    if let Some(lk) = &m.links {
        let dia = pos("links.dia_mm", lk.dia_mm).map_err(|e| at(&e))?;
        let s = pos("links.spacing_mm", lk.spacing_mm).map_err(|e| at(&e))?;
        let l = pos("links.length_mm", lk.length_mm).map_err(|e| at(&e))?;
        let n = (len / s).floor() + 1.0;
        steel += count * n * bar_kg(dia, l);
    }
    Ok((concrete, steel))
}

fn price(name: &str, p: &Option<Price>) -> Result<f64, String> {
    let p = p.as_ref().ok_or_else(|| format!("criterion cost needs prices.{name} with value, source and date: a price is never assumed"))?;
    let v = pos(&format!("prices.{name}.value"), p.value)?;
    for (field, val) in [("source", &p.source), ("date", &p.date)] {
        if val.as_deref().map(str::trim).unwrap_or("").is_empty() {
            return Err(format!("prices.{name}.{field} is required: every price carries where it came from and when"));
        }
    }
    Ok(v)
}

fn saving(base: f64, v: f64) -> Saving {
    Saving { abs: r3(base - v), pct: if base > 0.0 { Some(r3((base - v) / base * 100.0)) } else { None } }
}

/// Compares the alternatives. `baseline` is the index of the one savings are measured against (absent means the first).
pub fn compare(alternatives: &[AlternativeIn], criterion: Option<&str>, baseline: Option<usize>, prices: Option<&Prices>) -> Result<SavingsReport, String> {
    let criterion = Criterion::parse(criterion.ok_or("criterion is required (steel_kg, concrete_m3 or cost): which quantity to save is your decision")?)?;
    if alternatives.is_empty() {
        return Err("give at least one alternative to compare".to_string());
    }
    let baseline = baseline.unwrap_or(0);
    if baseline >= alternatives.len() {
        return Err(format!("baseline {baseline} is out of range: {} alternatives were given", alternatives.len()));
    }
    let unit = match (criterion, prices) {
        (Criterion::Cost, p) => {
            let p = p.ok_or("criterion cost needs prices (concrete_per_m3 and steel_per_kg, each with value, source and date): a price is never assumed")?;
            Some((price("concrete_per_m3", &p.concrete_per_m3)?, price("steel_per_kg", &p.steel_per_kg)?))
        }
        (_, Some(p)) if p.concrete_per_m3.is_some() || p.steel_per_kg.is_some() => Some((price("concrete_per_m3", &p.concrete_per_m3)?, price("steel_per_kg", &p.steel_per_kg)?)),
        _ => None,
    };
    let currency = prices.and_then(|p| p.currency.clone()).filter(|c| !c.trim().is_empty());
    if unit.is_some() && currency.is_none() {
        return Err("prices.currency is required with prices: a price without its currency is not a price".to_string());
    }

    struct Raw {
        concrete: f64,
        steel: f64,
        kinds: Vec<KindTotal>,
    }
    let mut raws: Vec<Raw> = Vec::new();
    for alt in alternatives {
        if alt.members.is_empty() {
            return Err(format!("alternative '{}' has no members", alt.label));
        }
        let mut kinds: Vec<KindTotal> = Vec::new();
        let (mut c_tot, mut s_tot) = (0.0, 0.0);
        for m in &alt.members {
            let (c, s) = member_quantities(&alt.label, m)?;
            c_tot += c;
            s_tot += s;
            match kinds.iter_mut().find(|k| k.kind == m.kind) {
                Some(k) => {
                    k.concrete_m3 += c;
                    k.steel_kg += s;
                }
                None => kinds.push(KindTotal { kind: m.kind.clone(), concrete_m3: c, steel_kg: s }),
            }
        }
        for k in &mut kinds {
            k.concrete_m3 = r3(k.concrete_m3);
            k.steel_kg = r3(k.steel_kg);
        }
        raws.push(Raw { concrete: c_tot, steel: s_tot, kinds });
    }
    let cost_of = |r: &Raw| unit.map(|(pc, ps)| r.concrete * pc + r.steel * ps);
    let metric = |r: &Raw| match criterion {
        Criterion::SteelKg => r.steel,
        Criterion::ConcreteM3 => r.concrete,
        Criterion::Cost => cost_of(r).unwrap_or(f64::INFINITY),
    };
    // Ranking among eligible alternatives: criterion, then the other quantities, then the order given.
    let mut eligible: Vec<usize> = (0..alternatives.len()).filter(|&i| alternatives[i].checks_passed != Some(false)).collect();
    eligible.sort_by(|&a, &b| {
        metric(&raws[a])
            .total_cmp(&metric(&raws[b]))
            .then(raws[a].steel.total_cmp(&raws[b].steel))
            .then(raws[a].concrete.total_cmp(&raws[b].concrete))
            .then(a.cmp(&b))
    });
    let mut rank: Vec<Option<usize>> = vec![None; alternatives.len()];
    for (pos, &i) in eligible.iter().enumerate() {
        rank[i] = Some(pos + 1);
    }

    let base = &raws[baseline];
    let base_cost = cost_of(base);
    let results: Vec<AlternativeResult> = alternatives
        .iter()
        .zip(&raws)
        .enumerate()
        .map(|(index, (alt, r))| AlternativeResult {
            index,
            label: alt.label.clone(),
            concrete_m3: r3(r.concrete),
            steel_kg: r3(r.steel),
            steel_kg_per_m3: if r.concrete > 0.0 { Some(r3(r.steel / r.concrete)) } else { None },
            cost: cost_of(r).map(r3),
            by_kind: r.kinds.clone(),
            checks_passed: alt.checks_passed,
            saving_concrete: saving(base.concrete, r.concrete),
            saving_steel: saving(base.steel, r.steel),
            saving_cost: base_cost.zip(cost_of(r)).map(|(b, c)| saving(b, c)),
            rank: rank[index],
        })
        .collect();

    let mut findings = vec![Finding {
        ar: "مقارنة كميات فقط لا تصميم: يوفّر البديل مادةً إن اجتاز فحوص التصميم الخاصة به (sizing وrebar والانحراف والقص الثاقب). الأطوال بلا تراكب ولا خطافات ولا هدر.".into(),
        en: "A comparison of quantities, not a design: an alternative saves material only if it passes its own design checks (sizing, rebar, deflection, punching). Lengths carry no laps, hooks or waste.".into(),
    }];
    if alternatives[baseline].checks_passed == Some(false) {
        findings.push(Finding {
            ar: format!("البديل المرجعي «{}» لم يجتز فحوصه، فالتوفير المقيس عليه ليس توفيراً عن تصميم صالح.", alternatives[baseline].label),
            en: format!("The baseline '{}' failed its checks, so savings measured against it are not savings over a valid design.", alternatives[baseline].label),
        });
    }
    for alt in alternatives {
        match alt.checks_passed {
            Some(false) => findings.push(Finding {
                ar: format!("البديل «{}» لم يجتز فحوصه فلم يُرتَّب ولا يُختار.", alt.label),
                en: format!("Alternative '{}' failed its checks and is neither ranked nor chosen.", alt.label),
            }),
            None => findings.push(Finding {
                ar: format!("البديل «{}» بلا نتيجة فحوص (checks_passed غائب): رُتِّب وهو غير مُتحقَّق.", alt.label),
                en: format!("Alternative '{}' has no check result (checks_passed absent): it is ranked but unverified.", alt.label),
            }),
            Some(true) => {}
        }
    }
    let best = eligible.first().copied();
    match best {
        None => findings.push(Finding {
            ar: "لا بديل صالح للترتيب: كلها لم تجتز فحوصها.".into(),
            en: "No alternative can be ranked: every one failed its checks.".into(),
        }),
        Some(b) if criterion == Criterion::SteelKg && results[b].concrete_m3 > results[baseline].concrete_m3 => findings.push(Finding {
            ar: format!("«{}» أقل حديداً لكنها أكثر خرسانة من المرجع.", results[b].label),
            en: format!("'{}' uses less steel but more concrete than the baseline.", results[b].label),
        }),
        Some(b) if criterion == Criterion::ConcreteM3 && results[b].steel_kg > results[baseline].steel_kg => findings.push(Finding {
            ar: format!("«{}» أقل خرسانة لكنها أكثر حديداً من المرجع.", results[b].label),
            en: format!("'{}' uses less concrete but more steel than the baseline.", results[b].label),
        }),
        Some(_) => {}
    }

    Ok(SavingsReport { criterion, currency, baseline, alternatives: results, best, findings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(key: &str, n: u32, b: f64, h: f64, len: f64, bars: u32, dia: f64) -> MemberIn {
        MemberIn {
            key: key.into(),
            kind: "column".into(),
            count: Some(n),
            b_mm: Some(b),
            h_mm: Some(h),
            length_mm: Some(len),
            bars: vec![BarSet { dia_mm: Some(dia), count: Some(bars), length_mm: None }],
            links: None,
        }
    }

    fn alt(label: &str, members: Vec<MemberIn>, ok: Option<bool>) -> AlternativeIn {
        AlternativeIn { label: label.into(), members, checks_passed: ok }
    }

    fn price(v: f64) -> Option<Price> {
        Some(Price { value: Some(v), source: Some("supplier quote 12".into()), date: Some("2026-11-01".into()) })
    }

    #[test]
    fn quantities_are_counted_exactly() {
        // 4 columns 300x300x3000: 4 * 0.3 * 0.3 * 3 = 1.08 m3; 8 bars dia 16 of 3 m: 4 * 8 * pi/4 * 0.016^2 * 3 * 7850.
        let a = alt("A", vec![col("c", 4, 300.0, 300.0, 3000.0, 8, 16.0)], Some(true));
        let r = compare(&[a], Some("steel_kg"), None, None).unwrap();
        assert_eq!(r.alternatives[0].concrete_m3, 1.08);
        let raw = 4.0 * 8.0 * std::f64::consts::PI / 4.0 * 0.016f64.powi(2) * 3.0 * 7850.0;
        assert_eq!(r.alternatives[0].steel_kg, r3(raw));
        assert_eq!(r.alternatives[0].steel_kg_per_m3, Some(r3(raw / 1.08)));
    }

    #[test]
    fn links_count_is_floor_of_span_over_spacing_plus_one() {
        let mut m = col("b", 1, 200.0, 500.0, 5000.0, 2, 12.0);
        m.bars.clear();
        m.links = Some(LinkSet { dia_mm: Some(8.0), spacing_mm: Some(200.0), length_mm: Some(1400.0) });
        let r = compare(&[alt("A", vec![m], Some(true))], Some("steel_kg"), None, None).unwrap();
        // floor(5000/200)+1 = 26 links of 1.4 m, dia 8.
        assert_eq!(r.alternatives[0].steel_kg, r3(26.0 * bar_kg(8.0, 1400.0)));
    }

    #[test]
    fn ranks_by_criterion_and_reports_saving_against_baseline() {
        let base = alt("base", vec![col("c", 4, 400.0, 400.0, 3000.0, 8, 20.0)], Some(true));
        let slim = alt("slim", vec![col("c", 4, 300.0, 300.0, 3000.0, 8, 16.0)], Some(true));
        let r = compare(&[base, slim], Some("steel_kg"), None, None).unwrap();
        assert_eq!(r.best, Some(1));
        assert_eq!(r.alternatives[1].rank, Some(1));
        assert_eq!(r.alternatives[0].rank, Some(2));
        assert_eq!(r.alternatives[0].saving_steel.abs, 0.0);
        assert!(r.alternatives[1].saving_steel.abs > 0.0);
        // 0.09 vs 0.16 m2 section: 43.75 % less concrete.
        assert_eq!(r.alternatives[1].saving_concrete.pct, Some(43.75));
    }

    #[test]
    fn failed_alternative_is_never_ranked_or_chosen() {
        let base = alt("base", vec![col("c", 4, 400.0, 400.0, 3000.0, 8, 20.0)], Some(true));
        let cheat = alt("too thin", vec![col("c", 4, 150.0, 150.0, 3000.0, 4, 10.0)], Some(false));
        let r = compare(&[base, cheat], Some("concrete_m3"), None, None).unwrap();
        assert_eq!(r.best, Some(0));
        assert_eq!(r.alternatives[1].rank, None);
        assert!(r.findings.iter().any(|f| f.en.contains("failed its checks")));
    }

    #[test]
    fn all_failed_gives_no_best() {
        let a = alt("a", vec![col("c", 1, 300.0, 300.0, 3000.0, 4, 16.0)], Some(false));
        let r = compare(&[a], Some("steel_kg"), None, None).unwrap();
        assert_eq!(r.best, None);
    }

    #[test]
    fn unverified_alternative_is_ranked_but_flagged() {
        let a = alt("a", vec![col("c", 1, 300.0, 300.0, 3000.0, 4, 16.0)], None);
        let r = compare(&[a], Some("steel_kg"), None, None).unwrap();
        assert_eq!(r.best, Some(0));
        assert!(r.findings.iter().any(|f| f.en.contains("unverified")));
    }

    #[test]
    fn cost_needs_prices_with_source_and_date_and_currency() {
        let a = alt("a", vec![col("c", 1, 300.0, 300.0, 3000.0, 4, 16.0)], Some(true));
        assert!(compare(std::slice::from_ref(&a), Some("cost"), None, None).unwrap_err().contains("needs prices"));
        let no_date = Prices { concrete_per_m3: price(1.0), steel_per_kg: Some(Price { value: Some(2.0), source: Some("q".into()), date: None }), currency: Some("SAR".into()) };
        assert!(compare(std::slice::from_ref(&a), Some("cost"), None, Some(&no_date)).unwrap_err().contains("steel_per_kg.date is required"));
        let no_cur = Prices { concrete_per_m3: price(1.0), steel_per_kg: price(2.0), currency: None };
        assert!(compare(std::slice::from_ref(&a), Some("cost"), None, Some(&no_cur)).unwrap_err().contains("currency is required"));
        let ok = Prices { concrete_per_m3: price(100.0), steel_per_kg: price(10.0), currency: Some("SAR".into()) };
        let r = compare(&[a], Some("cost"), None, Some(&ok)).unwrap();
        let c = &r.alternatives[0];
        assert_eq!(c.cost, Some(r3(c.concrete_m3 * 100.0 + c.steel_kg * 10.0)));
        assert_eq!(r.currency.as_deref(), Some("SAR"));
    }

    #[test]
    fn cost_can_pick_a_different_winner_than_steel() {
        // Alternative x: less steel, much more concrete. With a dear concrete price, y wins on cost; x wins on steel.
        let x = alt("x", vec![col("c", 1, 1000.0, 1000.0, 3000.0, 4, 12.0)], Some(true));
        let y = alt("y", vec![col("c", 1, 300.0, 300.0, 3000.0, 8, 25.0)], Some(true));
        let p = Prices { concrete_per_m3: price(1000.0), steel_per_kg: price(1.0), currency: Some("SAR".into()) };
        assert_eq!(compare(&[x.clone(), y.clone()], Some("steel_kg"), None, Some(&p)).unwrap().best, Some(0));
        assert_eq!(compare(&[x, y], Some("cost"), None, Some(&p)).unwrap().best, Some(1));
    }

    #[test]
    fn missing_inputs_are_named_and_nothing_is_assumed() {
        let a = alt("a", vec![col("c", 1, 300.0, 300.0, 3000.0, 4, 16.0)], Some(true));
        assert!(compare(std::slice::from_ref(&a), None, None, None).unwrap_err().contains("criterion is required"));
        assert!(compare(std::slice::from_ref(&a), Some("weight"), None, None).unwrap_err().contains("unknown"));
        assert!(compare(&[], Some("steel_kg"), None, None).unwrap_err().contains("at least one"));
        assert!(compare(std::slice::from_ref(&a), Some("steel_kg"), Some(3), None).unwrap_err().contains("out of range"));
        let mut no_b = a.clone();
        no_b.members[0].b_mm = None;
        assert!(compare(&[no_b], Some("steel_kg"), None, None).unwrap_err().contains("member 'c': b_mm is required"));
        let mut no_dia = a.clone();
        no_dia.members[0].bars[0].dia_mm = None;
        assert!(compare(&[no_dia], Some("steel_kg"), None, None).unwrap_err().contains("bars[0].dia_mm is required"));
        let mut zero = a;
        zero.members[0].count = Some(0);
        assert!(compare(&[zero], Some("steel_kg"), None, None).unwrap_err().contains("count must be at least 1"));
    }

    #[test]
    fn tie_on_criterion_falls_back_to_other_quantity_then_order() {
        let a = alt("a", vec![col("c", 1, 300.0, 300.0, 3000.0, 4, 16.0)], Some(true));
        let b = alt("b", vec![col("c", 1, 300.0, 300.0, 3000.0, 4, 16.0)], Some(true));
        let r = compare(&[a, b], Some("steel_kg"), None, None).unwrap();
        assert_eq!(r.best, Some(0));
        assert_eq!(r.alternatives[1].rank, Some(2));
    }

    #[test]
    fn breakdown_groups_by_kind_and_baseline_can_be_chosen() {
        let beam = MemberIn { key: "b".into(), kind: "beam".into(), count: Some(2), b_mm: Some(200.0), h_mm: Some(500.0), length_mm: Some(5000.0), bars: vec![], links: None };
        let a = alt("a", vec![col("c1", 2, 300.0, 300.0, 3000.0, 4, 16.0), col("c2", 2, 300.0, 300.0, 3000.0, 4, 16.0), beam], Some(true));
        let b = alt("b", vec![col("c1", 1, 300.0, 300.0, 3000.0, 4, 16.0)], Some(true));
        let r = compare(&[a, b], Some("concrete_m3"), Some(1), None).unwrap();
        let kinds: Vec<&str> = r.alternatives[0].by_kind.iter().map(|k| k.kind.as_str()).collect();
        assert_eq!(kinds, ["column", "beam"]);
        assert_eq!(r.alternatives[0].by_kind[0].concrete_m3, 1.08);
        assert_eq!(r.baseline, 1);
        assert!(r.alternatives[0].saving_concrete.abs < 0.0, "a is bigger than the baseline, so its saving is negative");
    }
}
