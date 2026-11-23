//! Declared scores for layout alternatives: lighting, connectivity and privacy, each 0-100, plus an overall
//! score that ranks the alternatives.
//!
//! These are geometric comparison scores between alternatives of one brief. They are not code values and not
//! statutory checks: no number here comes from a standard, and none is invented. Every score is a plain
//! fraction of a counted set, and the set is returned with it (`counted` of `total`) so the user can see what the
//! percentage stands for. A criterion with nothing to measure (for example no room that needs light) is `None`
//! and is left out of the overall score; it is never filled with a default.
//!
//! - **Lighting**: share of the floor area of rooms that need daylight that sits in rooms touching an outside wall
//!   of the building envelope.
//! - **Connectivity**: the mean of two shares, when both can be measured. (a) Private rooms (bathrooms excluded)
//!   that share at least [`ACCESS_MIN_MM`] of wall with a circulation room. (b) The weight of satisfied neighbour
//!   rules: a wanted pair satisfied when the rooms share at least [`NEIGHBOUR_MIN_MM`] of wall, an unwanted pair
//!   satisfied when they share none.
//! - **Privacy**: the mean of two shares over private rooms (bathrooms excluded). (a) Rooms that do not sit on the
//!   street side of the envelope. (b) Rooms that share less than [`ACCESS_MIN_MM`] of wall with any public-zone room,
//!   so they do not open straight onto guest space.
//!
//! The overall score is the mean of the available criteria weighted by [`ScoreWeights`], which the user can set;
//! the weights used are returned with the result.

use crate::layout::{Adjacency, Layout, Rect, Side, Zone};
use serde::{Deserialize, Serialize};

/// A private room needs this much shared wall with a circulation room to count as having access (door plus margins).
pub const ACCESS_MIN_MM: i64 = 1200;
/// Two rooms that should be neighbours must share at least this much wall (one door width plus margins).
pub const NEIGHBOUR_MIN_MM: i64 = 1000;
const WET_FUNCS: [&str; 2] = ["bathroom", "toilet"];
/// The filler room the generator adds when the program leaves area over; it acts as circulation.
const RESERVE_FUNC: &str = "circulation_reserve";

/// Relative importance of each criterion in the overall score. Zero removes a criterion from the ranking.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScoreWeights {
    pub lighting: f64,
    pub connectivity: f64,
    pub privacy: f64,
}

impl Default for ScoreWeights {
    /// Equal weight: the user states a preference by changing it.
    fn default() -> Self {
        Self { lighting: 1.0, connectivity: 1.0, privacy: 1.0 }
    }
}

impl ScoreWeights {
    pub fn validate(&self) -> Result<(), String> {
        for (name, v) in [("lighting", self.lighting), ("connectivity", self.connectivity), ("privacy", self.privacy)] {
            if !v.is_finite() || v < 0.0 {
                return Err(format!("score weight '{name}' must be zero or more, not {v}"));
            }
        }
        if self.lighting + self.connectivity + self.privacy <= 0.0 {
            return Err("at least one score weight must be above zero".into());
        }
        Ok(())
    }
}

/// A counted fraction. `den == 0` means nothing could be measured.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Tally {
    pub num: f64,
    pub den: f64,
}

impl Tally {
    fn add(&mut self, other: Tally) {
        self.num += other.num;
        self.den += other.den;
    }
    fn percent(&self) -> Option<f64> {
        (self.den > 0.0).then(|| 100.0 * self.num / self.den)
    }
}

/// Everything counted on one or more levels. Tallies add up across levels, so a building's score is weighted by what
/// each level contains and not an average of averages.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Tallies {
    /// Floor area (m2) of rooms needing light that touch an outside wall, over all rooms needing light.
    pub light: Tally,
    /// Private rooms with access to a circulation room, over all private rooms.
    pub access: Tally,
    /// Satisfied neighbour-rule weight over total neighbour-rule weight.
    pub neighbours: Tally,
    /// Private rooms off the street side, over all private rooms.
    pub off_street: Tally,
    /// Private rooms not opening onto public rooms, over all private rooms.
    pub not_on_public: Tally,
}

impl Tallies {
    pub fn add(&mut self, o: &Tallies) {
        self.light.add(o.light);
        self.access.add(o.access);
        self.neighbours.add(o.neighbours);
        self.off_street.add(o.off_street);
        self.not_on_public.add(o.not_on_public);
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Scores {
    pub lighting: Option<f64>,
    pub connectivity: Option<f64>,
    pub privacy: Option<f64>,
    /// Weighted mean of the criteria that could be measured; `None` when none could.
    pub overall: Option<f64>,
    pub weights: ScoreWeights,
    /// The counted sets behind each percentage, for display.
    pub basis: Tallies,
    /// Criteria that could not be measured, with the reason.
    pub unmeasured: Vec<String>,
}

fn shared_len(a: &Rect, b: &Rect) -> i64 {
    let overlap = |a0: i64, a1: i64, b0: i64, b1: i64| (a1.min(b1) - a0.max(b0)).max(0);
    let mut s = 0;
    if a.x + a.w == b.x || b.x + b.w == a.x {
        s += overlap(a.y, a.y + a.h, b.y, b.y + b.h);
    }
    if a.y + a.h == b.y || b.y + b.h == a.y {
        s += overlap(a.x, a.x + a.w, b.x, b.x + b.w);
    }
    s
}

fn touches_outside(r: &Rect, e: &Rect) -> bool {
    r.x == e.x || r.y == e.y || r.x + r.w == e.x + e.w || r.y + r.h == e.y + e.h
}

pub(crate) fn on_street_side(r: &Rect, e: &Rect, side: Side) -> bool {
    match side {
        Side::South => r.y == e.y,
        Side::North => r.y + r.h == e.y + e.h,
        Side::West => r.x == e.x,
        Side::East => r.x + r.w == e.x + e.w,
    }
}

/// Counts one level. `envelope` is the whole building envelope (the outside walls), not the strip left after a stair.
pub fn tally_level(layout: &Layout, envelope: &Rect, entry: Side, adjacency: &[Adjacency], hub_funcs: &[String]) -> Tallies {
    let mut t = Tallies::default();
    let rooms = &layout.rooms;
    let is_hub = |f: &str| f == RESERVE_FUNC || hub_funcs.iter().any(|h| h == f);
    for r in rooms.iter().filter(|r| r.needs_light) {
        t.light.den += r.area_m2;
        if touches_outside(&r.rect, envelope) {
            t.light.num += r.area_m2;
        }
    }
    for (i, r) in rooms.iter().enumerate() {
        if r.zone != Zone::Private || WET_FUNCS.contains(&r.func.as_str()) {
            continue;
        }
        let others = || rooms.iter().enumerate().filter(move |(j, _)| *j != i).map(|(_, o)| o);
        t.access.den += 1.0;
        if others().any(|o| is_hub(&o.func) && shared_len(&r.rect, &o.rect) >= ACCESS_MIN_MM) {
            t.access.num += 1.0;
        }
        t.off_street.den += 1.0;
        if !on_street_side(&r.rect, envelope, entry) {
            t.off_street.num += 1.0;
        }
        t.not_on_public.den += 1.0;
        if !others().any(|o| o.zone == Zone::Public && shared_len(&r.rect, &o.rect) >= ACCESS_MIN_MM) {
            t.not_on_public.num += 1.0;
        }
    }
    for a in adjacency {
        let (Some(ra), Some(rb)) = (rooms.iter().find(|r| r.key == a.a), rooms.iter().find(|r| r.key == a.b)) else {
            continue;
        };
        let w = f64::from(a.weight.unsigned_abs());
        let shared = shared_len(&ra.rect, &rb.rect);
        let ok = if a.weight > 0 { shared >= NEIGHBOUR_MIN_MM } else { shared == 0 };
        t.neighbours.den += w;
        if ok {
            t.neighbours.num += w;
        }
    }
    t
}

/// Turns counted sets into the declared scores.
pub fn scores_from(t: &Tallies, w: &ScoreWeights) -> Result<Scores, String> {
    w.validate()?;
    let mean = |parts: &[Option<f64>]| {
        let got: Vec<f64> = parts.iter().flatten().copied().collect();
        (!got.is_empty()).then(|| got.iter().sum::<f64>() / got.len() as f64)
    };
    let lighting = t.light.percent();
    let connectivity = mean(&[t.access.percent(), t.neighbours.percent()]);
    let privacy = mean(&[t.off_street.percent(), t.not_on_public.percent()]);
    let mut unmeasured = Vec::new();
    if lighting.is_none() {
        unmeasured.push("lighting: no room needs daylight in this program".to_string());
    }
    if connectivity.is_none() {
        unmeasured.push("connectivity: no private room and no neighbour rule to measure".to_string());
    }
    if privacy.is_none() {
        unmeasured.push("privacy: no private room in this program".to_string());
    }
    let parts = [(lighting, w.lighting), (connectivity, w.connectivity), (privacy, w.privacy)];
    let weight_sum: f64 = parts.iter().filter(|(s, _)| s.is_some()).map(|(_, wt)| wt).sum();
    let overall = (weight_sum > 0.0).then(|| parts.iter().filter_map(|(s, wt)| s.map(|s| s * wt)).sum::<f64>() / weight_sum);
    Ok(Scores { lighting, connectivity, privacy, overall, weights: *w, basis: *t, unmeasured })
}

/// Order for ranking: highest overall first, an alternative that could not be scored last, ties by the generator cost
/// (lower first) so the order is stable.
pub fn rank_order(a: (&Scores, f64), b: (&Scores, f64)) -> std::cmp::Ordering {
    match (a.0.overall, b.0.overall) {
        (Some(x), Some(y)) => y.total_cmp(&x).then(a.1.total_cmp(&b.1)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.1.total_cmp(&b.1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Breakdown, PlacedRoom};

    fn room(key: &str, func: &str, zone: Zone, light: bool, at: [i64; 4]) -> PlacedRoom {
        let rect = Rect { x: at[0], y: at[1], w: at[2], h: at[3] };
        PlacedRoom { key: key.into(), func: func.into(), zone, needs_light: light, rect, area_m2: rect.area_m2(), target_m2: rect.area_m2(), deviation_pct: 0.0 }
    }
    fn layout(rooms: Vec<PlacedRoom>) -> Layout {
        Layout { rooms, cost: 0.0, breakdown: Breakdown::default(), reserve_m2: 0.0 }
    }
    const ENV: Rect = Rect { x: 0, y: 0, w: 10_000, h: 10_000 };
    fn hubs() -> Vec<String> {
        vec!["hall".into(), "living".into()]
    }

    #[test]
    fn lighting_counts_area_of_rooms_on_an_outside_wall() {
        // A 4x4 room in the middle and a 10x2 strip on the south wall.
        let l = layout(vec![
            room("inner", "bedroom", Zone::Private, true, [3000, 3000, 4000, 4000]),
            room("edge", "living", Zone::Public, true, [0, 0, 10_000, 2000]),
        ]);
        let t = tally_level(&l, &ENV, Side::South, &[], &hubs());
        let s = scores_from(&t, &ScoreWeights::default()).unwrap();
        assert!((t.light.den - 36.0).abs() < 1e-9 && (t.light.num - 20.0).abs() < 1e-9);
        assert!((s.lighting.unwrap() - 100.0 * 20.0 / 36.0).abs() < 1e-9);
    }

    #[test]
    fn privacy_penalises_street_side_and_rooms_opening_onto_guest_space() {
        let l = layout(vec![
            room("majlis", "majlis", Zone::Public, false, [0, 4000, 5000, 2000]),
            room("bed_front", "bedroom", Zone::Private, true, [5000, 0, 5000, 4000]), // street side, no public wall
            room("bed_back", "bedroom", Zone::Private, true, [0, 6000, 5000, 4000]),  // touches majlis over 5 m
        ]);
        let t = tally_level(&l, &ENV, Side::South, &[], &hubs());
        assert_eq!((t.off_street.num, t.off_street.den), (1.0, 2.0));
        assert_eq!((t.not_on_public.num, t.not_on_public.den), (1.0, 2.0));
        assert!((scores_from(&t, &ScoreWeights::default()).unwrap().privacy.unwrap() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn connectivity_counts_access_and_neighbour_rules_and_skips_bathrooms() {
        let l = layout(vec![
            room("hall", "hall", Zone::Public, false, [0, 4000, 10_000, 2000]),
            room("bed1", "bedroom", Zone::Private, true, [0, 0, 5000, 4000]),   // 5 m on the hall
            room("bed2", "bedroom", Zone::Private, true, [5000, 0, 5000, 3000]), // 3 m gap above the hall: no wall
            room("bath", "bathroom", Zone::Private, false, [5000, 3000, 1000, 1000]),
        ]);
        let adj = vec![
            Adjacency { a: "bed1".into(), b: "bed2".into(), weight: 2 },  // share 3 m: satisfied
            Adjacency { a: "bed1".into(), b: "hall".into(), weight: -1 }, // touch: violated
        ];
        let t = tally_level(&l, &ENV, Side::South, &adj, &hubs());
        assert_eq!((t.access.num, t.access.den), (1.0, 2.0), "the bathroom is not counted");
        assert_eq!((t.neighbours.num, t.neighbours.den), (2.0, 3.0));
        let s = scores_from(&t, &ScoreWeights::default()).unwrap();
        assert!((s.connectivity.unwrap() - (50.0 + 100.0 * 2.0 / 3.0) / 2.0).abs() < 1e-9);
    }

    #[test]
    fn an_unmeasurable_criterion_is_reported_and_left_out_not_defaulted() {
        let l = layout(vec![room("hall", "hall", Zone::Public, false, [0, 0, 10_000, 10_000])]);
        let t = tally_level(&l, &ENV, Side::South, &[], &hubs());
        let s = scores_from(&t, &ScoreWeights::default()).unwrap();
        assert!(s.lighting.is_none() && s.connectivity.is_none() && s.privacy.is_none() && s.overall.is_none());
        assert_eq!(s.unmeasured.len(), 3);
    }

    #[test]
    fn overall_is_the_weighted_mean_of_measured_criteria_only() {
        let t = Tallies {
            light: Tally { num: 50.0, den: 100.0 },
            access: Tally { num: 1.0, den: 1.0 },
            off_street: Tally { num: 1.0, den: 1.0 },
            not_on_public: Tally { num: 1.0, den: 1.0 },
            ..Tallies::default()
        };
        let even = scores_from(&t, &ScoreWeights::default()).unwrap();
        assert!((even.overall.unwrap() - (50.0 + 100.0 + 100.0) / 3.0).abs() < 1e-9);
        let light_heavy = scores_from(&t, &ScoreWeights { lighting: 2.0, connectivity: 1.0, privacy: 1.0 }).unwrap();
        assert!((light_heavy.overall.unwrap() - (100.0 + 100.0 + 100.0) / 4.0).abs() < 1e-9);
        let only_light = scores_from(&t, &ScoreWeights { lighting: 1.0, connectivity: 0.0, privacy: 0.0 }).unwrap();
        assert!((only_light.overall.unwrap() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn bad_weights_are_rejected_by_name() {
        let t = Tallies::default();
        assert!(scores_from(&t, &ScoreWeights { lighting: -1.0, connectivity: 1.0, privacy: 1.0 }).unwrap_err().contains("lighting"));
        assert!(scores_from(&t, &ScoreWeights { lighting: 0.0, connectivity: 0.0, privacy: 0.0 }).is_err());
        assert!(scores_from(&t, &ScoreWeights { lighting: f64::NAN, connectivity: 1.0, privacy: 1.0 }).is_err());
    }

    #[test]
    fn tallies_add_across_levels_and_ranking_puts_the_best_first() {
        let mut a = Tallies { light: Tally { num: 10.0, den: 20.0 }, ..Tallies::default() };
        a.add(&Tallies { light: Tally { num: 20.0, den: 20.0 }, ..Tallies::default() });
        assert_eq!((a.light.num, a.light.den), (30.0, 40.0));
        let w = ScoreWeights::default();
        let low = scores_from(&Tallies { light: Tally { num: 1.0, den: 4.0 }, ..Tallies::default() }, &w).unwrap();
        let high = scores_from(&Tallies { light: Tally { num: 3.0, den: 4.0 }, ..Tallies::default() }, &w).unwrap();
        let none = scores_from(&Tallies::default(), &w).unwrap();
        let mut v = [(&none, 0.1), (&low, 0.2), (&high, 0.9)];
        v.sort_by(|a, b| rank_order(*a, *b));
        assert_eq!(v.iter().map(|x| x.1).collect::<Vec<_>>(), vec![0.9, 0.2, 0.1]);
    }
}
