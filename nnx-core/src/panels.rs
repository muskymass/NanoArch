//! Distribution-board load schedules: phase assignment, per-phase demand, main breaker and way count.
//!
//! Every value that decides a result is the caller's: the supply (phase-to-neutral voltage, power factor, single or three
//! phase), the demand factor of each load kind, the breaker sizes on offer, the breaker margin, and each board's way count.
//! Nothing here was read from an electrical code or a manufacturer's catalogue; a missing value is refused by name. Optional
//! limits (bus rating of a board, maximum phase imbalance, minimum spare ways) are the caller's: a missing limit leaves that
//! check `unchecked`, never `ok`.
//!
//! Single-phase loads are placed on the phase with the least demand so far (loads that name a phase are placed first, then
//! the rest from the largest demand down; ties go to the lower phase). A three-phase load takes three ways and an equal third
//! of its demand on each phase. A load takes `phases` ways in its board.
//!
//! The demand of a load is `watts * demand_factor[kind]`. The current of a single-phase load is `W / (V * pf)`; a balanced
//! three-phase load draws `W / (3 * V * pf)` per line, `V` being the phase-to-neutral voltage. The main breaker of a board is
//! the smallest listed size at or above `margin * (highest phase demand current)`; none large enough is a failure, never a
//! silent choice of the largest size.
//!
//! This is a preliminary schedule. It does not size cables or check discrimination, short-circuit rating, harmonics, neutral
//! loading or earthing, and it does not replace an electrical design or the approving authority.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Board {
    pub key: String,
    /// Number of single-pole ways the board has.
    #[serde(default)]
    pub ways: Option<u32>,
    /// Rated busbar current, A. Absent: the main breaker is not compared with it (`unchecked`).
    #[serde(default)]
    pub bus_rating_a: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PanelLoad {
    pub id: String,
    /// Key of the board that feeds it.
    pub board: String,
    /// Load kind (for example `lighting`, `sockets`, `hvac`): looked up in `demand_factors`.
    pub kind: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Connected power, W.
    #[serde(default)]
    pub watts: Option<f64>,
    /// 1 or 3. A three-phase load needs a three-phase supply.
    #[serde(default)]
    pub phases: Option<u8>,
    /// Fixed phase (1-based) of a single-phase load. Absent: the schedule picks the lightest phase.
    #[serde(default)]
    pub phase: Option<u8>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PanelInput {
    /// Phase-to-neutral voltage, V.
    #[serde(default)]
    pub voltage_v: Option<f64>,
    #[serde(default)]
    pub power_factor: Option<f64>,
    #[serde(default)]
    pub three_phase: Option<bool>,
    /// Demand factor per load kind, in (0, 1].
    #[serde(default)]
    pub demand_factors: BTreeMap<String, f64>,
    /// Breaker ratings available, A.
    #[serde(default)]
    pub breaker_sizes_a: Vec<f64>,
    /// Multiplier on the design current before choosing a breaker.
    #[serde(default)]
    pub breaker_margin: Option<f64>,
    pub boards: Vec<Board>,
    #[serde(default)]
    pub loads: Vec<PanelLoad>,
    /// Largest acceptable phase imbalance of a board, `(max - min) / max` of the phase demands, in percent. Absent: `unchecked`.
    #[serde(default)]
    pub max_imbalance_pct: Option<f64>,
    /// Fewest spare ways a board must keep. Absent: `unchecked`.
    #[serde(default)]
    pub min_spare_ways: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScheduleRow {
    pub id: String,
    pub description: Option<String>,
    pub kind: String,
    pub phases: u8,
    /// Phases it sits on (1-based): one for a single-phase load, all for a three-phase load.
    pub on_phases: Vec<u8>,
    pub watts: f64,
    pub demand_factor: f64,
    pub demand_w: f64,
    pub current_a: f64,
    pub breaker_a: Option<f64>,
    pub breaker_status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PhaseRow {
    pub phase: u8,
    pub connected_w: f64,
    pub demand_w: f64,
    pub demand_current_a: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoardRow {
    pub key: String,
    pub ways: u32,
    pub ways_used: u32,
    pub spare_ways: Option<u32>,
    pub ways_status: &'static str,
    pub spare_status: &'static str,
    pub loads: Vec<ScheduleRow>,
    pub phases: Vec<PhaseRow>,
    pub connected_w: f64,
    pub demand_w: f64,
    pub demand_kva: f64,
    /// `(max - min) / max` of the phase demands, percent. `None` for a single-phase supply or a board with no demand.
    pub imbalance_pct: Option<f64>,
    pub imbalance_status: &'static str,
    pub main_current_a: f64,
    pub main_breaker_a: Option<f64>,
    pub bus_rating_a: Option<f64>,
    pub bus_status: &'static str,
    pub status: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PanelCounts {
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
pub struct PanelReport {
    pub boards: Vec<BoardRow>,
    pub connected_w: f64,
    pub demand_w: f64,
    pub demand_kva: f64,
    pub counts: PanelCounts,
    pub findings: Vec<Finding>,
}

fn positive(v: f64, field: &str) -> Result<f64, String> {
    if v.is_finite() && v > 0.0 {
        Ok(v)
    } else {
        Err(format!("{field} must be a positive number, got {v}"))
    }
}

fn required(v: Option<f64>, field: &str) -> Result<f64, String> {
    positive(v.ok_or_else(|| format!("{field} is required and has no default"))?, field)
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

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Supply values after validation.
struct Supply {
    v: f64,
    pf: f64,
    nph: usize,
    margin: f64,
    breakers: Vec<f64>,
}

/// A load with its validated numbers, before it is placed on a phase.
struct Placed {
    idx: usize,
    phases: u8,
    watts: f64,
    factor: f64,
    demand: f64,
    fixed: Option<usize>,
}

fn validate(inp: &PanelInput) -> Result<Supply, String> {
    if inp.boards.is_empty() {
        return Err("nothing to schedule: give at least one board".into());
    }
    let v = required(inp.voltage_v, "voltage_v")?;
    let pf = required(inp.power_factor, "power_factor")?;
    if pf > 1.0 {
        return Err(format!("power_factor must be above 0 and at most 1, got {pf}"));
    }
    let three = inp.three_phase.ok_or_else(|| "three_phase is required and has no default".to_string())?;
    let margin = required(inp.breaker_margin, "breaker_margin")?;
    if inp.breaker_sizes_a.is_empty() {
        return Err("breaker_sizes_a is required and has no default (list the breaker ratings available)".into());
    }
    let mut breakers = inp.breaker_sizes_a.clone();
    for b in &breakers {
        positive(*b, "breaker_sizes_a entry")?;
    }
    breakers.sort_by(f64::total_cmp);
    for (k, f) in &inp.demand_factors {
        if !(f.is_finite() && *f > 0.0 && *f <= 1.0) {
            return Err(format!("demand_factors['{k}'] must be above 0 and at most 1, got {f}"));
        }
    }
    if let Some(v) = inp.max_imbalance_pct.filter(|v| !(v.is_finite() && *v >= 0.0)) {
        return Err(format!("max_imbalance_pct must be zero or more, got {v}"));
    }
    let mut seen = BTreeSet::new();
    for b in &inp.boards {
        if b.key.trim().is_empty() {
            return Err("a board has an empty key".into());
        }
        if !seen.insert(b.key.as_str()) {
            return Err(format!("board key '{}' is used twice", b.key));
        }
        let ways = b.ways.ok_or_else(|| format!("board '{}': ways is required and has no default", b.key))?;
        if ways == 0 {
            return Err(format!("board '{}': ways must be at least 1", b.key));
        }
        if let Some(r) = b.bus_rating_a {
            positive(r, &format!("board '{}': bus_rating_a", b.key))?;
        }
    }
    let mut ids = BTreeSet::new();
    for l in &inp.loads {
        if l.id.trim().is_empty() {
            return Err("a load has an empty id".into());
        }
        if !ids.insert(l.id.as_str()) {
            return Err(format!("load id '{}' is used twice", l.id));
        }
    }
    Ok(Supply { v, pf, nph: if three { 3 } else { 1 }, margin, breakers })
}

/// The board's loads, the demand on each phase after placement, and the phase index chosen for each load (`usize::MAX` for
/// a three-phase load).
type Placement = (Vec<Placed>, Vec<f64>, Vec<usize>);

fn place_loads(inp: &PanelInput, s: &Supply, board: &Board) -> Result<Placement, String> {
    let mut items = Vec::new();
    for (idx, l) in inp.loads.iter().enumerate().filter(|(_, l)| l.board == board.key) {
        let watts = required(l.watts, &format!("load '{}': watts", l.id))?;
        let phases = l.phases.ok_or_else(|| format!("load '{}': phases is required (1 or 3) and has no default", l.id))?;
        if phases != 1 && phases != 3 {
            return Err(format!("load '{}': phases must be 1 or 3, got {phases}", l.id));
        }
        if phases == 3 && s.nph == 1 {
            return Err(format!("load '{}': a three-phase load needs three_phase = true", l.id));
        }
        let factor = *inp
            .demand_factors
            .get(&l.kind)
            .ok_or_else(|| format!("load '{}': demand_factors has no entry for kind '{}' and none is assumed", l.id, l.kind))?;
        let fixed = match l.phase {
            None => None,
            Some(p) if phases == 1 && usize::from(p) >= 1 && usize::from(p) <= s.nph => Some(usize::from(p) - 1),
            Some(p) if phases == 3 => return Err(format!("load '{}': a three-phase load cannot be fixed to phase {p}", l.id)),
            Some(p) => return Err(format!("load '{}': phase {p} does not exist on this supply (1 to {})", l.id, s.nph)),
        };
        items.push(Placed { idx, phases, watts, factor, demand: watts * factor, fixed });
    }
    // Fixed loads first, then three-phase (they do not choose), then single-phase from the largest demand down.
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| {
        let (x, y) = (&items[a], &items[b]);
        let rank = |p: &Placed| if p.fixed.is_some() { 0 } else if p.phases == 3 { 1 } else { 2 };
        rank(x).cmp(&rank(y)).then(y.demand.total_cmp(&x.demand)).then(x.idx.cmp(&y.idx))
    });
    let mut phase_demand = vec![0.0_f64; s.nph];
    let mut assigned = vec![usize::MAX; items.len()];
    for &i in &order {
        let it = &items[i];
        if it.phases == 3 {
            phase_demand.iter_mut().for_each(|d| *d += it.demand / 3.0);
        } else {
            let p = it.fixed.unwrap_or_else(|| {
                let mut best = 0;
                for (k, d) in phase_demand.iter().enumerate() {
                    if *d < phase_demand[best] - 1e-9 {
                        best = k;
                    }
                }
                best
            });
            phase_demand[p] += it.demand;
            assigned[i] = p;
        }
    }
    Ok((items, phase_demand, assigned))
}

fn board_row(inp: &PanelInput, s: &Supply, board: &Board) -> Result<BoardRow, String> {
    let (items, phase_demand, assigned) = place_loads(inp, s, board)?;
    let ways = board.ways.unwrap_or(0);
    let mut connected_by_phase = vec![0.0_f64; s.nph];
    let mut rows = Vec::with_capacity(items.len());
    let mut ways_used = 0_u32;
    let mut breaker_fail = false;
    for (i, it) in items.iter().enumerate() {
        let src = &inp.loads[it.idx];
        ways_used += u32::from(it.phases);
        let on_phases: Vec<u8> = if it.phases == 3 { (1..=3).collect() } else { vec![assigned[i] as u8 + 1] };
        if it.phases == 3 {
            connected_by_phase.iter_mut().for_each(|c| *c += it.watts / 3.0);
        } else {
            connected_by_phase[assigned[i]] += it.watts;
        }
        let current = if it.phases == 3 { it.watts / (3.0 * s.v * s.pf) } else { it.watts / (s.v * s.pf) };
        let breaker = super::mep::pick_ge(&s.breakers, current * s.margin);
        let breaker_status = if breaker.is_some() { "ok" } else { "fails" };
        breaker_fail |= breaker.is_none();
        rows.push(ScheduleRow {
            id: src.id.clone(),
            description: src.description.clone(),
            kind: src.kind.clone(),
            phases: it.phases,
            on_phases,
            watts: round2(it.watts),
            demand_factor: it.factor,
            demand_w: round2(it.demand),
            current_a: round2(current),
            breaker_a: breaker,
            breaker_status,
        });
    }
    let phases: Vec<PhaseRow> = phase_demand
        .iter()
        .enumerate()
        .map(|(k, d)| PhaseRow { phase: k as u8 + 1, connected_w: round2(connected_by_phase[k]), demand_w: round2(*d), demand_current_a: round2(d / (s.v * s.pf)) })
        .collect();
    let connected: f64 = connected_by_phase.iter().sum();
    let demand: f64 = phase_demand.iter().sum();
    let max_phase = phase_demand.iter().copied().fold(0.0_f64, f64::max);
    let min_phase = phase_demand.iter().copied().fold(f64::INFINITY, f64::min);
    let imbalance_pct = if s.nph > 1 && max_phase > 0.0 { Some((max_phase - min_phase) / max_phase * 100.0) } else { None };
    let imbalance_status = match (imbalance_pct, inp.max_imbalance_pct) {
        (None, _) if s.nph == 1 || max_phase <= 0.0 => "ok",
        (Some(v), Some(l)) if v <= l + 1e-9 => "ok",
        (Some(_), Some(_)) => "fails",
        _ => "unchecked",
    };
    let main_current = max_phase / (s.v * s.pf);
    let main_breaker = if items.is_empty() { None } else { super::mep::pick_ge(&s.breakers, main_current * s.margin) };
    let main_failed = !items.is_empty() && main_breaker.is_none();
    let bus_status = match (main_breaker, board.bus_rating_a) {
        _ if main_failed => "fails",
        (None, _) => "ok",
        (Some(b), Some(r)) if b <= r + 1e-9 => "ok",
        (Some(_), Some(_)) => "fails",
        (Some(_), None) => "unchecked",
    };
    let ways_status = if ways_used <= ways { "ok" } else { "fails" };
    let spare = ways.checked_sub(ways_used);
    let spare_status = match (spare, inp.min_spare_ways) {
        (Some(sp), Some(m)) if sp >= m => "ok",
        (_, Some(_)) => "fails",
        (_, None) => "unchecked",
    };
    let status = worst(&[ways_status, spare_status, imbalance_status, bus_status, if breaker_fail { "fails" } else { "ok" }]);
    Ok(BoardRow {
        key: board.key.clone(),
        ways,
        ways_used,
        spare_ways: spare,
        ways_status,
        spare_status,
        loads: rows,
        phases,
        connected_w: round2(connected),
        demand_w: round2(demand),
        demand_kva: round2(demand / s.pf / 1000.0),
        imbalance_pct: imbalance_pct.map(round2),
        imbalance_status,
        main_current_a: round2(main_current),
        main_breaker_a: main_breaker,
        bus_rating_a: board.bus_rating_a,
        bus_status,
        status,
    })
}

pub fn check(inp: &PanelInput) -> Result<PanelReport, String> {
    let s = validate(inp)?;
    for l in &inp.loads {
        if !inp.boards.iter().any(|b| b.key == l.board) {
            return Err(format!("load '{}': board '{}' is not in the board list", l.id, l.board));
        }
    }
    let boards = inp.boards.iter().map(|b| board_row(inp, &s, b)).collect::<Result<Vec<_>, _>>()?;
    let mut counts = PanelCounts::default();
    for b in &boards {
        match b.status {
            "ok" => counts.ok += 1,
            "fails" => counts.fails += 1,
            _ => counts.unchecked += 1,
        }
    }
    let connected: f64 = boards.iter().map(|b| b.connected_w).sum();
    let demand: f64 = boards.iter().map(|b| b.demand_w).sum();

    let mut findings = Vec::new();
    if inp.max_imbalance_pct.is_none() && s.nph > 1 {
        findings.push(Finding {
            ar: "لم يُدخل حد عدم اتزان الأطوار (max_imbalance_pct) فلم يُحكم على الاتزان.".into(),
            en: "No max_imbalance_pct given, so no pass or fail is declared for phase balance.".into(),
        });
    }
    if inp.min_spare_ways.is_none() {
        findings.push(Finding {
            ar: "لم يُدخل أقل عدد مسارات احتياطية (min_spare_ways) فلم يُحكم على الاحتياطي.".into(),
            en: "No min_spare_ways given, so no pass or fail is declared for spare ways.".into(),
        });
    }
    for b in &boards {
        if b.bus_rating_a.is_none() {
            findings.push(Finding {
                ar: format!("اللوحة «{}»: لم يُدخل تصنيف القضبان (bus_rating_a) فلم يُقارن القاطع الرئيسي به.", b.key),
                en: format!("Board '{}': no bus_rating_a given, so the main breaker is not compared with the busbar rating.", b.key),
            });
        }
        if b.ways_status == "fails" {
            findings.push(Finding {
                ar: format!("اللوحة «{}»: الأحمال تحتاج {} مساراً واللوحة فيها {}.", b.key, b.ways_used, b.ways),
                en: format!("Board '{}': the loads need {} ways and the board has {}.", b.key, b.ways_used, b.ways),
            });
        }
        if b.bus_status == "fails" && b.main_breaker_a.is_none() {
            findings.push(Finding {
                ar: format!("اللوحة «{}»: التيار الرئيسي {:.1} أ بعد المعامل يفوق أكبر قاطع في القائمة.", b.key, b.main_current_a),
                en: format!("Board '{}': the main current of {:.1} A, with the margin, is above the largest breaker listed.", b.key, b.main_current_a),
            });
        }
        for l in b.loads.iter().filter(|l| l.breaker_a.is_none()) {
            findings.push(Finding {
                ar: format!("الحمل «{}» في اللوحة «{}»: تياره {:.1} أ يفوق أكبر قاطع في القائمة.", l.id, b.key, l.current_a),
                en: format!("Load '{}' on board '{}': its current of {:.1} A is above the largest breaker listed.", l.id, b.key, l.current_a),
            });
        }
    }
    findings.push(Finding {
        ar: "جدول أولي بمدخلاتك: لا مقاسات كابلات ولا تنسيق حماية ولا قدرة قطع ولا توافقيات ولا حمل النيوترال ولا تأريض، ولا يغني عن تصميم كهربائي أو الجهة المعتمدة.".into(),
        en: "Preliminary schedule from your inputs: no cable sizing, protection discrimination, breaking capacity, harmonics, neutral loading or earthing, and it does not replace an electrical design or the approving authority.".into(),
    });
    Ok(PanelReport { boards, connected_w: round2(connected), demand_w: round2(demand), demand_kva: round2(demand / s.pf / 1000.0), counts, findings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(id: &str, board: &str, kind: &str, watts: f64, phases: u8) -> PanelLoad {
        PanelLoad { id: id.into(), board: board.into(), kind: kind.into(), description: None, watts: Some(watts), phases: Some(phases), phase: None }
    }
    fn board(key: &str, ways: u32) -> Board {
        Board { key: key.into(), ways: Some(ways), bus_rating_a: Some(100.0) }
    }
    fn input(loads: Vec<PanelLoad>) -> PanelInput {
        PanelInput {
            voltage_v: Some(230.0),
            power_factor: Some(1.0),
            three_phase: Some(true),
            demand_factors: [("lighting".to_string(), 1.0), ("hvac".to_string(), 0.5)].into_iter().collect(),
            breaker_sizes_a: vec![10.0, 16.0, 20.0, 32.0, 63.0, 100.0],
            breaker_margin: Some(1.25),
            boards: vec![board("DB1", 12)],
            loads,
            max_imbalance_pct: Some(10.0),
            min_spare_ways: Some(2),
        }
    }

    #[test]
    fn single_phase_loads_balance_across_phases() {
        // 2300 W, 2300 W, 2300 W: one on each phase; 4600 W goes first onto L1, the rest fill L2 and L3.
        let r = check(&input(vec![
            load("a", "DB1", "lighting", 2300.0, 1),
            load("b", "DB1", "lighting", 4600.0, 1),
            load("c", "DB1", "lighting", 2300.0, 1),
            load("d", "DB1", "lighting", 2300.0, 1),
        ]))
        .unwrap();
        let b = &r.boards[0];
        let by: Vec<f64> = b.phases.iter().map(|p| p.demand_w).collect();
        assert_eq!(by, vec![4600.0, 4600.0, 2300.0]);
        assert_eq!(b.loads.iter().find(|l| l.id == "b").unwrap().on_phases, vec![1]);
        assert_eq!(b.ways_used, 4);
        assert_eq!(b.spare_ways, Some(8));
        assert!((b.imbalance_pct.unwrap() - 50.0).abs() < 1e-9);
        assert_eq!(b.imbalance_status, "fails");
        assert_eq!(b.status, "fails");
    }

    #[test]
    fn three_phase_load_splits_equally_and_takes_three_ways() {
        // 6900 W * 0.5 = 3450 W demand: 1150 W per phase; line current 6900 / (3*230) = 10 A -> 12.5 A -> 16 A.
        let r = check(&input(vec![load("ac", "DB1", "hvac", 6900.0, 3)])).unwrap();
        let b = &r.boards[0];
        assert_eq!(b.ways_used, 3);
        assert!(b.phases.iter().all(|p| (p.demand_w - 1150.0).abs() < 1e-9 && (p.connected_w - 2300.0).abs() < 1e-9));
        assert_eq!(b.loads[0].on_phases, vec![1, 2, 3]);
        assert!((b.loads[0].current_a - 10.0).abs() < 1e-9);
        assert_eq!(b.loads[0].breaker_a, Some(16.0));
        assert_eq!(b.imbalance_pct, Some(0.0));
        assert_eq!(b.imbalance_status, "ok");
        assert_eq!((b.main_current_a, b.main_breaker_a), (5.0, Some(10.0)));
        assert_eq!(b.bus_status, "ok");
        assert_eq!(b.demand_kva, 3.45);
        assert_eq!(b.status, "ok");
    }

    #[test]
    fn main_breaker_follows_the_heaviest_phase() {
        // L1 carries 23 000 W of demand -> 100 A * 1.25 = 125 A: above every listed breaker, so it fails by name.
        let r = check(&input(vec![load("big", "DB1", "lighting", 23000.0, 1)])).unwrap();
        let b = &r.boards[0];
        assert_eq!(b.main_breaker_a, None);
        assert_eq!(b.bus_status, "fails");
        assert_eq!(b.loads[0].breaker_status, "fails");
        assert_eq!(b.status, "fails");
        assert!(r.findings.iter().any(|f| f.en.contains("above the largest breaker listed")));
    }

    #[test]
    fn busbar_ways_and_spare_limits() {
        let mut inp = input(vec![load("a", "DB1", "lighting", 11500.0, 1), load("b", "DB1", "lighting", 11500.0, 1), load("c", "DB1", "lighting", 11500.0, 1)]);
        // 50 A * 1.25 = 62.5 A -> 63 A main; a 40 A busbar rejects it.
        inp.boards[0].bus_rating_a = Some(40.0);
        assert_eq!(check(&inp).unwrap().boards[0].bus_status, "fails");
        inp.boards[0].bus_rating_a = Some(63.0);
        let r = check(&inp).unwrap();
        assert_eq!((r.boards[0].main_breaker_a, r.boards[0].bus_status), (Some(63.0), "ok"));
        // Too few ways.
        inp.boards[0].ways = Some(2);
        let r = check(&inp).unwrap();
        assert_eq!((r.boards[0].ways_status, r.boards[0].spare_ways), ("fails", None));
        // Spare ways limit: 12 ways, 3 used, 9 spare; a limit of 10 fails, 9 passes.
        inp.boards[0].ways = Some(12);
        inp.min_spare_ways = Some(10);
        assert_eq!(check(&inp).unwrap().boards[0].spare_status, "fails");
        inp.min_spare_ways = Some(9);
        assert_eq!(check(&inp).unwrap().boards[0].spare_status, "ok");
    }

    #[test]
    fn missing_limits_stay_unchecked_never_ok() {
        let mut inp = input(vec![load("a", "DB1", "lighting", 2300.0, 1)]);
        inp.max_imbalance_pct = None;
        inp.min_spare_ways = None;
        inp.boards[0].bus_rating_a = None;
        let r = check(&inp).unwrap();
        let b = &r.boards[0];
        assert_eq!((b.imbalance_status, b.spare_status, b.bus_status, b.status), ("unchecked", "unchecked", "unchecked", "unchecked"));
        assert_eq!(r.counts, PanelCounts { ok: 0, fails: 0, unchecked: 1 });
        assert_eq!(r.findings.iter().filter(|f| f.en.starts_with("No ") || f.en.contains("no bus_rating_a")).count(), 3);
    }

    #[test]
    fn single_phase_supply_has_one_phase_and_no_imbalance() {
        let mut inp = input(vec![load("a", "DB1", "lighting", 2300.0, 1), load("b", "DB1", "lighting", 2300.0, 1)]);
        inp.three_phase = Some(false);
        inp.max_imbalance_pct = None;
        let r = check(&inp).unwrap();
        let b = &r.boards[0];
        assert_eq!(b.phases.len(), 1);
        assert_eq!((b.imbalance_pct, b.imbalance_status), (None, "ok"));
        assert_eq!(b.main_current_a, 20.0);
        assert_eq!(b.main_breaker_a, Some(32.0)); // 25 A with the margin
        let mut three = input(vec![load("t", "DB1", "hvac", 3000.0, 3)]);
        three.three_phase = Some(false);
        assert!(check(&three).unwrap_err().contains("needs three_phase = true"));
    }

    #[test]
    fn fixed_phases_are_honoured_and_checked() {
        let mut a = load("a", "DB1", "lighting", 2300.0, 1);
        a.phase = Some(3);
        let b = load("b", "DB1", "lighting", 1000.0, 1);
        let r = check(&input(vec![b, a])).unwrap();
        let rows = &r.boards[0].loads;
        assert_eq!(rows.iter().find(|l| l.id == "a").unwrap().on_phases, vec![3]);
        assert_eq!(rows.iter().find(|l| l.id == "b").unwrap().on_phases, vec![1]);
        let mut bad = load("x", "DB1", "lighting", 100.0, 1);
        bad.phase = Some(4);
        assert!(check(&input(vec![bad])).unwrap_err().contains("phase 4 does not exist"));
        let mut tri = load("y", "DB1", "hvac", 900.0, 3);
        tri.phase = Some(1);
        assert!(check(&input(vec![tri])).unwrap_err().contains("cannot be fixed"));
    }

    #[test]
    fn multiple_boards_are_scheduled_independently() {
        let mut inp = input(vec![load("a", "DB1", "lighting", 2300.0, 1), load("b", "DB2", "hvac", 4600.0, 1)]);
        inp.boards.push(board("DB2", 6));
        let r = check(&inp).unwrap();
        assert_eq!(r.boards.len(), 2);
        assert_eq!((r.boards[0].demand_w, r.boards[1].demand_w), (2300.0, 2300.0));
        assert_eq!((r.connected_w, r.demand_w), (6900.0, 4600.0));
        assert_eq!(r.demand_kva, 4.6);
    }

    #[test]
    fn empty_board_has_no_main_breaker_and_all_ways_spare() {
        let r = check(&input(vec![])).unwrap();
        let b = &r.boards[0];
        assert_eq!((b.main_breaker_a, b.spare_ways, b.ways_used, b.bus_status), (None, Some(12), 0, "ok"));
        assert_eq!(b.status, "ok");
    }

    #[test]
    fn missing_or_bad_inputs_are_refused_by_name() {
        let base = || input(vec![load("a", "DB1", "lighting", 2300.0, 1)]);
        assert!(check(&PanelInput::default()).unwrap_err().contains("nothing to schedule"));
        let mut i = base();
        i.voltage_v = None;
        assert!(check(&i).unwrap_err().contains("voltage_v is required"));
        let mut i = base();
        i.power_factor = Some(1.2);
        assert!(check(&i).unwrap_err().contains("power_factor"));
        let mut i = base();
        i.three_phase = None;
        assert!(check(&i).unwrap_err().contains("three_phase is required"));
        let mut i = base();
        i.breaker_margin = None;
        assert!(check(&i).unwrap_err().contains("breaker_margin is required"));
        let mut i = base();
        i.breaker_sizes_a.clear();
        assert!(check(&i).unwrap_err().contains("breaker_sizes_a is required"));
        let mut i = base();
        i.boards[0].ways = None;
        assert!(check(&i).unwrap_err().contains("ways is required"));
        let mut i = base();
        i.demand_factors.clear();
        assert!(check(&i).unwrap_err().contains("no entry for kind 'lighting'"));
        let mut i = base();
        i.demand_factors.insert("lighting".into(), 1.5);
        assert!(check(&i).unwrap_err().contains("at most 1"));
        let mut i = base();
        i.loads[0].watts = None;
        assert!(check(&i).unwrap_err().contains("watts is required"));
        let mut i = base();
        i.loads[0].phases = None;
        assert!(check(&i).unwrap_err().contains("phases is required"));
        let mut i = base();
        i.loads[0].phases = Some(2);
        assert!(check(&i).unwrap_err().contains("must be 1 or 3"));
        let mut i = base();
        i.loads[0].board = "ghost".into();
        assert!(check(&i).unwrap_err().contains("not in the board list"));
        let mut i = base();
        i.loads.push(load("a", "DB1", "lighting", 1.0, 1));
        assert!(check(&i).unwrap_err().contains("used twice"));
        let mut i = base();
        i.boards.push(board("DB1", 4));
        assert!(check(&i).unwrap_err().contains("board key 'DB1' is used twice"));
    }
}
