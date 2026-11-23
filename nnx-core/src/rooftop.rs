//! Usable roof: a rooftop room, canopies (pergolas, shades) and water tanks placed on the roof of the top floor.
//!
//! The roof is the envelope (the lower-left corner is the origin) and every item is a rectangle in millimetres in that
//! frame. Nothing is assumed:
//! - an item outside the roof, overlapping another item, with a non-positive size or height, a duplicate key, or a tank
//!   without a water depth (or a water depth on a non-tank) is refused with the reason;
//! - the minimum edge set-back, the minimum gap between items, the minimum parapet height, the minimum clear height under a
//!   room or canopy and the largest share of the roof a rooftop room may take are user inputs taken from the code or the
//!   approving authority; without one the matching check is `unchecked`, never passed;
//! - the parapet height is a user input too: when it is missing the parapet check is `unchecked`.
//!
//! The report gives each item's area, its nearest distance to a roof edge and to another item, a tank's volume and the mass
//! of the water it holds (1 m³ of water is 1000 kg), the totals by kind and the free roof area left.

use crate::courtyard::Finding;
use crate::layout::Rect;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoofKind {
    /// An enclosed room on the roof (stair head house, service room, penthouse).
    Room,
    /// A roofed but open structure: pergola, shade, canopy.
    Canopy,
    /// A water tank standing on the roof.
    Tank,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RoofItem {
    pub key: String,
    pub kind: RoofKind,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    /// Room: clear height inside. Canopy: clear height under it. Tank: overall height of the tank.
    pub height_mm: i64,
    /// Tank only: depth of the water at full level.
    #[serde(default)]
    pub water_depth_mm: Option<i64>,
}

/// The items and the optional limits, all lengths in millimetres.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RooftopSpec {
    pub items: Vec<RoofItem>,
    /// Height of the roof parapet; `None` leaves the parapet check `unchecked`.
    #[serde(default)]
    pub parapet_mm: Option<i64>,
    #[serde(default)]
    pub min_parapet_mm: Option<i64>,
    /// Smallest distance from any item to a roof edge.
    #[serde(default)]
    pub min_edge_setback_mm: Option<i64>,
    /// Smallest gap between two items.
    #[serde(default)]
    pub min_gap_mm: Option<i64>,
    /// Smallest clear height inside a room or under a canopy.
    #[serde(default)]
    pub min_clear_mm: Option<i64>,
    /// Largest share (0 to 1) of the roof that rooftop rooms may cover.
    #[serde(default)]
    pub max_room_share: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    Pass,
    Fail,
    /// No limit (or no measured value to compare) was entered: nothing was compared.
    Unchecked,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ItemReport {
    pub key: String,
    pub kind: RoofKind,
    pub area_m2: f64,
    /// Nearest distance from the item to a roof edge.
    pub edge_distance_mm: i64,
    /// Nearest gap to another item; `None` when it is the only item.
    pub nearest_gap_mm: Option<i64>,
    /// Tank only: volume of the water at full level.
    pub water_m3: Option<f64>,
    /// Tank only: mass of that water.
    pub water_kg: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RooftopReport {
    pub roof_m2: f64,
    pub items: Vec<ItemReport>,
    pub room_m2: f64,
    pub canopy_m2: f64,
    pub tank_m2: f64,
    /// Roof not taken by any item.
    pub free_m2: f64,
    pub room_share: f64,
    pub tank_water_kg: f64,
    pub edge_check: Check,
    pub gap_check: Check,
    pub parapet_check: Check,
    pub clear_check: Check,
    pub share_check: Check,
    pub findings: Vec<Finding>,
}

fn compare_min(value: Option<i64>, min: Option<i64>) -> Check {
    match (value, min) {
        (Some(v), Some(m)) if v >= m => Check::Pass,
        (Some(_), Some(_)) => Check::Fail,
        _ => Check::Unchecked,
    }
}

/// Smallest distance between two rectangles, rounded up to a whole millimetre; zero when they touch.
fn rect_gap(a: &Rect, b: &Rect) -> i64 {
    let dx = (b.x - (a.x + a.w)).max(a.x - (b.x + b.w)).max(0);
    let dy = (b.y - (a.y + a.h)).max(a.y - (b.y + b.h)).max(0);
    if dx == 0 || dy == 0 {
        dx.max(dy)
    } else {
        ((dx as f64).hypot(dy as f64)).ceil() as i64
    }
}

fn validate(roof: &Rect, spec: &RooftopSpec) -> Result<(), String> {
    if roof.w <= 0 || roof.h <= 0 {
        return Err(format!("rooftop: the roof must have a positive size (got {} x {} mm)", roof.w, roof.h));
    }
    if spec.items.is_empty() {
        return Err("rooftop: no items were given; list at least one room, canopy or tank".to_string());
    }
    for (name, v) in [
        ("parapet_mm", spec.parapet_mm),
        ("min_parapet_mm", spec.min_parapet_mm),
        ("min_edge_setback_mm", spec.min_edge_setback_mm),
        ("min_gap_mm", spec.min_gap_mm),
        ("min_clear_mm", spec.min_clear_mm),
    ] {
        if let Some(m) = v.filter(|m| *m <= 0) {
            return Err(format!("rooftop: {name} must be positive, not {m} mm"));
        }
    }
    if let Some(s) = spec.max_room_share.filter(|s| !s.is_finite() || *s <= 0.0 || *s > 1.0) {
        return Err(format!("rooftop: max_room_share must be in (0, 1], not {s}"));
    }
    let mut keys = BTreeSet::new();
    for it in &spec.items {
        if it.key.trim().is_empty() {
            return Err("rooftop: every item needs a key".to_string());
        }
        if !keys.insert(it.key.as_str()) {
            return Err(format!("rooftop: the key '{}' is used by more than one item", it.key));
        }
        if it.w <= 0 || it.h <= 0 {
            return Err(format!("rooftop: item '{}' must have a positive width and depth (got {} x {} mm)", it.key, it.w, it.h));
        }
        if it.height_mm <= 0 {
            return Err(format!("rooftop: item '{}' must have a positive height_mm, not {} mm", it.key, it.height_mm));
        }
        if it.x < 0 || it.y < 0 || it.x + it.w > roof.w || it.y + it.h > roof.h {
            return Err(format!("rooftop: item '{}' lies outside the roof ({} x {} mm)", it.key, roof.w, roof.h));
        }
        match (it.kind, it.water_depth_mm) {
            (RoofKind::Tank, None) => return Err(format!("rooftop: tank '{}' needs water_depth_mm", it.key)),
            (RoofKind::Tank, Some(d)) if d <= 0 || d > it.height_mm => {
                return Err(format!("rooftop: tank '{}' water_depth_mm ({d} mm) must be positive and not above its height ({} mm)", it.key, it.height_mm));
            }
            (RoofKind::Room | RoofKind::Canopy, Some(_)) => {
                return Err(format!("rooftop: only a tank holds water; remove water_depth_mm from '{}'", it.key));
            }
            _ => {}
        }
    }
    for (i, a) in spec.items.iter().enumerate() {
        for b in &spec.items[i + 1..] {
            if a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h {
                return Err(format!("rooftop: items '{}' and '{}' overlap", a.key, b.key));
            }
        }
    }
    Ok(())
}

fn rect_of(it: &RoofItem) -> Rect {
    Rect { x: it.x, y: it.y, w: it.w, h: it.h }
}

/// Checks the items placed on a roof of the given size against the limits the user entered.
pub fn check_rooftop(roof: &Rect, spec: &RooftopSpec) -> Result<RooftopReport, String> {
    validate(roof, spec)?;
    let roof_mm2 = roof.w as i128 * roof.h as i128;
    let roof_m2 = roof_mm2 as f64 / 1e6;

    let mut items = Vec::with_capacity(spec.items.len());
    let (mut room_mm2, mut canopy_mm2, mut tank_mm2) = (0i128, 0i128, 0i128);
    let mut tank_water_kg = 0.0;
    for it in &spec.items {
        let r = rect_of(it);
        let area = r.w as i128 * r.h as i128;
        match it.kind {
            RoofKind::Room => room_mm2 += area,
            RoofKind::Canopy => canopy_mm2 += area,
            RoofKind::Tank => tank_mm2 += area,
        }
        let edge = it.x.min(it.y).min(roof.w - (it.x + it.w)).min(roof.h - (it.y + it.h));
        let nearest = spec.items.iter().filter(|o| o.key != it.key).map(|o| rect_gap(&r, &rect_of(o))).min();
        let (water_m3, water_kg) = match it.water_depth_mm {
            Some(d) => {
                let m3 = it.w as f64 * it.h as f64 * d as f64 / 1e9;
                tank_water_kg += m3 * 1000.0;
                (Some(m3), Some(m3 * 1000.0))
            }
            None => (None, None),
        };
        items.push(ItemReport { key: it.key.clone(), kind: it.kind, area_m2: area as f64 / 1e6, edge_distance_mm: edge, nearest_gap_mm: nearest, water_m3, water_kg });
    }
    let free_mm2 = roof_mm2 - room_mm2 - canopy_mm2 - tank_mm2;
    let room_share = room_mm2 as f64 / roof_mm2 as f64;

    let min_edge = items.iter().map(|i| i.edge_distance_mm).min();
    let min_gap = items.iter().filter_map(|i| i.nearest_gap_mm).min();
    let min_clear = spec.items.iter().filter(|i| i.kind != RoofKind::Tank).map(|i| i.height_mm).min();
    let edge_check = compare_min(min_edge, spec.min_edge_setback_mm);
    let gap_check = compare_min(min_gap, spec.min_gap_mm);
    let parapet_check = compare_min(spec.parapet_mm, spec.min_parapet_mm);
    let clear_check = compare_min(min_clear, spec.min_clear_mm);
    let share_check = match spec.max_room_share {
        None => Check::Unchecked,
        Some(s) if room_share <= s => Check::Pass,
        Some(_) => Check::Fail,
    };

    let mut findings = Vec::new();
    if let (Check::Fail, Some(m), Some(v)) = (edge_check, spec.min_edge_setback_mm, min_edge) {
        let k = items.iter().find(|i| i.edge_distance_mm == v).map_or("", |i| i.key.as_str());
        findings.push(Finding {
            ar: format!("«{k}» يبعد {v} مم عن حافة السطح وهو أقل من الحد الأدنى {m} مم."),
            en: format!("'{k}' is {v} mm from the roof edge, below the {m} mm minimum."),
        });
    }
    if let (Check::Fail, Some(m), Some(v)) = (gap_check, spec.min_gap_mm, min_gap) {
        findings.push(Finding {
            ar: format!("أضيق فراغ بين عنصرين على السطح {v} مم وهو أقل من الحد الأدنى {m} مم."),
            en: format!("The narrowest gap between two roof items is {v} mm, below the {m} mm minimum."),
        });
    }
    if let (Check::Fail, Some(m), Some(v)) = (parapet_check, spec.min_parapet_mm, spec.parapet_mm) {
        findings.push(Finding {
            ar: format!("ارتفاع الحاجز الجانبي {v} مم أقل من الحد الأدنى {m} مم."),
            en: format!("The parapet is {v} mm high, below the {m} mm minimum."),
        });
    }
    if let (Check::Fail, Some(m), Some(v)) = (clear_check, spec.min_clear_mm, min_clear) {
        findings.push(Finding {
            ar: format!("أقل ارتفاع صافٍ داخل غرفة أو تحت مظلة {v} مم وهو أقل من الحد الأدنى {m} مم."),
            en: format!("The lowest clear height in a room or under a canopy is {v} mm, below the {m} mm minimum."),
        });
    }
    if let (Check::Fail, Some(s)) = (share_check, spec.max_room_share) {
        findings.push(Finding {
            ar: format!("غرف السطح تغطي {:.1}% من مساحته وهي فوق الحد الأقصى {:.1}%.", room_share * 100.0, s * 100.0),
            en: format!("Rooftop rooms cover {:.1}% of the roof, above the {:.1}% maximum.", room_share * 100.0, s * 100.0),
        });
    }
    for (check, ar, en) in [
        (edge_check, "البعد عن حافة السطح", "the set-back from the roof edge"),
        (gap_check, "الفراغ بين العناصر", "the gap between items"),
        (parapet_check, "ارتفاع الحاجز الجانبي (يلزم إدخال الارتفاع والحد معاً)", "the parapet height (both the height and the minimum are needed)"),
        (clear_check, "الارتفاع الصافي للغرف والمظلات", "the clear height of rooms and canopies"),
        (share_check, "نسبة غرف السطح", "the share of the roof taken by rooms"),
    ] {
        if check == Check::Unchecked {
            findings.push(Finding { ar: format!("لم يُدخَل ما يلزم لفحص {ar}: لم يُفحص."), en: format!("Nothing was entered to check {en}: not checked.") });
        }
    }
    Ok(RooftopReport {
        roof_m2,
        items,
        room_m2: room_mm2 as f64 / 1e6,
        canopy_m2: canopy_mm2 as f64 / 1e6,
        tank_m2: tank_mm2 as f64 / 1e6,
        free_m2: free_mm2 as f64 / 1e6,
        room_share,
        tank_water_kg,
        edge_check,
        gap_check,
        parapet_check,
        clear_check,
        share_check,
        findings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOF: Rect = Rect { x: 0, y: 0, w: 10000, h: 10000 };

    fn item(key: &str, kind: RoofKind, x: i64, y: i64, w: i64, h: i64, height: i64) -> RoofItem {
        RoofItem { key: key.into(), kind, x, y, w, h, height_mm: height, water_depth_mm: None }
    }
    fn tank(key: &str, x: i64, y: i64, w: i64, h: i64, height: i64, depth: i64) -> RoofItem {
        RoofItem { water_depth_mm: Some(depth), ..item(key, RoofKind::Tank, x, y, w, h, height) }
    }
    fn spec(items: Vec<RoofItem>) -> RooftopSpec {
        RooftopSpec { items, parapet_mm: None, min_parapet_mm: None, min_edge_setback_mm: None, min_gap_mm: None, min_clear_mm: None, max_room_share: None }
    }

    #[test]
    fn areas_free_roof_and_water_mass_are_exact() {
        let s = spec(vec![
            item("stair", RoofKind::Room, 1000, 1000, 3000, 4000, 2600),
            item("shade", RoofKind::Canopy, 6000, 1000, 3000, 3000, 2400),
            tank("t1", 1000, 7000, 2000, 2000, 1500, 1200),
        ]);
        let r = check_rooftop(&ROOF, &s).unwrap();
        assert!((r.roof_m2 - 100.0).abs() < 1e-9);
        assert!((r.room_m2 - 12.0).abs() < 1e-9);
        assert!((r.canopy_m2 - 9.0).abs() < 1e-9);
        assert!((r.tank_m2 - 4.0).abs() < 1e-9);
        assert!((r.free_m2 - 75.0).abs() < 1e-9);
        assert!((r.room_share - 0.12).abs() < 1e-12);
        let t = r.items.iter().find(|i| i.key == "t1").unwrap();
        assert!((t.water_m3.unwrap() - 4.8).abs() < 1e-9);
        assert!((r.tank_water_kg - 4800.0).abs() < 1e-6);
    }

    #[test]
    fn edge_distance_and_nearest_gap_are_measured_including_diagonals() {
        let s = spec(vec![item("a", RoofKind::Room, 500, 2000, 2000, 2000, 2500), item("b", RoofKind::Room, 5500, 6000, 1000, 1000, 2500)]);
        let r = check_rooftop(&ROOF, &s).unwrap();
        let a = &r.items[0];
        assert_eq!(a.edge_distance_mm, 500);
        // dx = 5500 - 2500 = 3000, dy = 6000 - 4000 = 2000: diagonal gap = sqrt(13) * 1000 rounded up.
        assert_eq!(a.nearest_gap_mm, Some(3606));
        let touching = spec(vec![item("a", RoofKind::Room, 0, 0, 2000, 2000, 2500), item("b", RoofKind::Room, 2000, 500, 1000, 1000, 2500)]);
        assert_eq!(check_rooftop(&ROOF, &touching).unwrap().items[0].nearest_gap_mm, Some(0));
    }

    #[test]
    fn limits_not_met_fail_and_limits_not_entered_are_unchecked() {
        let items = vec![item("a", RoofKind::Room, 500, 2000, 4000, 4000, 2200), item("b", RoofKind::Canopy, 5000, 2000, 2000, 2000, 2400)];
        let none = check_rooftop(&ROOF, &spec(items.clone())).unwrap();
        assert_eq!((none.edge_check, none.gap_check, none.parapet_check, none.clear_check, none.share_check), (Check::Unchecked, Check::Unchecked, Check::Unchecked, Check::Unchecked, Check::Unchecked));
        assert_eq!(none.findings.iter().filter(|f| f.en.contains("not checked")).count(), 5);
        // A parapet limit with no parapet height (or the reverse) cannot be compared.
        let half = RooftopSpec { min_parapet_mm: Some(1000), ..spec(items.clone()) };
        assert_eq!(check_rooftop(&ROOF, &half).unwrap().parapet_check, Check::Unchecked);
        let tight = RooftopSpec { parapet_mm: Some(900), min_parapet_mm: Some(1000), min_edge_setback_mm: Some(1500), min_gap_mm: Some(1500), min_clear_mm: Some(2300), max_room_share: Some(0.1), ..spec(items) };
        let r = check_rooftop(&ROOF, &tight).unwrap();
        assert_eq!((r.edge_check, r.gap_check, r.parapet_check, r.clear_check, r.share_check), (Check::Fail, Check::Fail, Check::Fail, Check::Fail, Check::Fail));
        assert!(r.findings.iter().any(|f| f.en.contains("'a' is 500 mm from the roof edge, below the 1500 mm")));
        assert!(r.findings.iter().any(|f| f.en.contains("gap between two roof items is 500 mm")));
        assert!(r.findings.iter().any(|f| f.en.contains("The parapet is 900 mm high")));
        assert!(r.findings.iter().any(|f| f.en.contains("lowest clear height") && f.en.contains("2200 mm")));
        assert!(r.findings.iter().any(|f| f.en.contains("16.0% of the roof, above the 10.0% maximum")));
        let ok = RooftopSpec { parapet_mm: Some(1200), min_parapet_mm: Some(1000), min_edge_setback_mm: Some(500), min_gap_mm: Some(500), min_clear_mm: Some(2200), max_room_share: Some(0.2), ..spec(tight.items.clone()) };
        let r = check_rooftop(&ROOF, &ok).unwrap();
        assert_eq!((r.edge_check, r.gap_check, r.parapet_check, r.clear_check, r.share_check), (Check::Pass, Check::Pass, Check::Pass, Check::Pass, Check::Pass));
    }

    #[test]
    fn a_tank_is_excluded_from_the_clear_height_check() {
        let s = RooftopSpec { min_clear_mm: Some(2300), ..spec(vec![tank("t", 0, 0, 1000, 1000, 1500, 1000), item("r", RoofKind::Room, 3000, 3000, 2000, 2000, 2400)]) };
        assert_eq!(check_rooftop(&ROOF, &s).unwrap().clear_check, Check::Pass);
        let only_tank = RooftopSpec { min_clear_mm: Some(2300), ..spec(vec![tank("t", 0, 0, 1000, 1000, 1500, 1000)]) };
        assert_eq!(check_rooftop(&ROOF, &only_tank).unwrap().clear_check, Check::Unchecked);
    }

    #[test]
    fn invalid_input_is_refused_with_the_reason() {
        let room = |k: &str, x, y, w, h, ht| item(k, RoofKind::Room, x, y, w, h, ht);
        let e = |s: RooftopSpec| check_rooftop(&ROOF, &s).unwrap_err();
        assert!(e(spec(vec![])).contains("no items"));
        assert!(e(spec(vec![room("a", 0, 0, 0, 1000, 2500)])).contains("positive width and depth"));
        assert!(e(spec(vec![room("a", 0, 0, 1000, 1000, 0)])).contains("positive height_mm"));
        assert!(e(spec(vec![room("a", 9500, 0, 1000, 1000, 2500)])).contains("outside the roof"));
        assert!(e(spec(vec![room("a", 0, 0, 2000, 2000, 2500), room("a", 5000, 5000, 1000, 1000, 2500)])).contains("more than one item"));
        assert!(e(spec(vec![room("a", 0, 0, 2000, 2000, 2500), room("b", 1000, 1000, 2000, 2000, 2500)])).contains("'a' and 'b' overlap"));
        assert!(e(spec(vec![item("t", RoofKind::Tank, 0, 0, 1000, 1000, 1500)])).contains("needs water_depth_mm"));
        assert!(e(spec(vec![tank("t", 0, 0, 1000, 1000, 1500, 1600)])).contains("not above its height"));
        assert!(e(spec(vec![RoofItem { water_depth_mm: Some(500), ..room("a", 0, 0, 1000, 1000, 2500) }])).contains("only a tank holds water"));
        assert!(e(RooftopSpec { min_gap_mm: Some(0), ..spec(vec![room("a", 0, 0, 1000, 1000, 2500)]) }).contains("min_gap_mm must be positive"));
        assert!(e(RooftopSpec { max_room_share: Some(f64::NAN), ..spec(vec![room("a", 0, 0, 1000, 1000, 2500)]) }).contains("max_room_share"));
    }
}
