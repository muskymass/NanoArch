//! Structural-span comparison of layout alternatives: which alternative asks the structure for the fewest large spans.
//!
//! A room with no column inside it needs beams that cross its full width and depth. When either side is longer than the span
//! limit YOU enter, the room needs intermediate support lines (beams on columns or a wall) so that every bay is within the limit.
//! For each room this module counts the bays on each side (`ceil(side / limit)`), the support lines that implies
//! (`bays - 1` per side), and the part of each side that exceeds the limit. Alternatives are then ranked on counted
//! quantities only, in this order: total support lines, total excess length, longest span, then the order given.
//!
//! - No price, productivity or code value is used. The maximum span is a design input with a stated source; without it
//!   nothing is compared and the result names it as missing.
//! - "Cheaper" here means fewer large spans and fewer support lines to add: a geometric comparison between layouts. It is not a
//!   cost estimate and not a member design; the structure must still be designed for the loads and soil of the project.

use crate::layout::{Layout, Rect};
use serde::Serialize;

/// One room that is wider or deeper than the limit.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LargeSpan {
    pub room: String,
    pub width_mm: i64,
    pub depth_mm: i64,
    /// Bays needed across the width (`ceil(width / limit)`), at least 1.
    pub bays_x: i64,
    /// Bays needed across the depth.
    pub bays_y: i64,
    /// Support lines to add inside the room: `(bays_x - 1) + (bays_y - 1)`.
    pub support_lines: i64,
    /// Each side's excess over the limit, summed, mm (only sides over the limit count).
    pub excess_mm: i64,
    /// Bay size after splitting evenly, mm: `[width / bays_x, depth / bays_y]` rounded up.
    pub bay_mm: [i64; 2],
}

/// The result for one alternative.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AlternativeSpan {
    /// Position in the list given (0-based).
    pub index: usize,
    pub label: String,
    pub rooms: usize,
    pub longest_span_mm: i64,
    pub rooms_over_limit: usize,
    pub support_lines: i64,
    pub total_excess_mm: i64,
    pub large_spans: Vec<LargeSpan>,
    /// 1 = fewest large spans.
    pub rank: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub ar: String,
    pub en: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpanComparison {
    pub max_span_mm: i64,
    /// Alternatives in the order given, each with its rank.
    pub alternatives: Vec<AlternativeSpan>,
    /// Index of the alternative ranked first.
    pub best: usize,
    /// Support lines saved by the best alternative against the first one given (0 when the first is best).
    pub support_lines_saved_vs_first: i64,
    pub findings: Vec<Finding>,
}

fn bays(side_mm: i64, limit_mm: i64) -> i64 {
    ((side_mm + limit_mm - 1) / limit_mm).max(1)
}

/// Large spans of one layout against `limit_mm`.
pub fn large_spans(layout: &Layout, limit_mm: i64) -> Vec<LargeSpan> {
    layout
        .rooms
        .iter()
        .filter_map(|r| {
            let Rect { w, h, .. } = r.rect;
            if w <= limit_mm && h <= limit_mm {
                return None;
            }
            let (bx, by) = (bays(w, limit_mm), bays(h, limit_mm));
            Some(LargeSpan {
                room: r.key.clone(),
                width_mm: w,
                depth_mm: h,
                bays_x: bx,
                bays_y: by,
                support_lines: (bx - 1) + (by - 1),
                excess_mm: (w - limit_mm).max(0) + (h - limit_mm).max(0),
                bay_mm: [(w + bx - 1) / bx, (h + by - 1) / by],
            })
        })
        .collect()
}

/// Compares alternatives. `limit_mm` is the largest acceptable span between supports; `None` is refused by name.
/// Each alternative is `(label, layout)`.
pub fn compare(alternatives: &[(String, &Layout)], limit_mm: Option<i64>) -> Result<SpanComparison, String> {
    let limit = limit_mm.ok_or("max_span_mm is required: the largest span between supports is a design input with a stated source, and nothing is assumed")?;
    if limit <= 0 {
        return Err(format!("max_span_mm must be a positive number of millimetres, not {limit}"));
    }
    if alternatives.is_empty() {
        return Err("give at least one alternative to compare".to_string());
    }
    let mut out: Vec<AlternativeSpan> = Vec::new();
    for (index, (label, layout)) in alternatives.iter().enumerate() {
        if layout.rooms.is_empty() {
            return Err(format!("alternative '{label}' has no rooms"));
        }
        if let Some(bad) = layout.rooms.iter().find(|r| r.rect.w <= 0 || r.rect.h <= 0) {
            return Err(format!("alternative '{label}': room '{}' has a non-positive size {} x {} mm", bad.key, bad.rect.w, bad.rect.h));
        }
        let spans = large_spans(layout, limit);
        out.push(AlternativeSpan {
            index,
            label: label.clone(),
            rooms: layout.rooms.len(),
            longest_span_mm: layout.rooms.iter().map(|r| r.rect.w.max(r.rect.h)).max().unwrap_or(0),
            rooms_over_limit: spans.len(),
            support_lines: spans.iter().map(|s| s.support_lines).sum(),
            total_excess_mm: spans.iter().map(|s| s.excess_mm).sum(),
            large_spans: spans,
            rank: 0,
        });
    }
    let mut order: Vec<usize> = (0..out.len()).collect();
    order.sort_by_key(|&i| (out[i].support_lines, out[i].total_excess_mm, out[i].longest_span_mm, i));
    for (pos, &i) in order.iter().enumerate() {
        out[i].rank = pos + 1;
    }
    let best = order[0];
    let saved = out[0].support_lines - out[best].support_lines;
    let mut findings = Vec::new();
    if out[best].support_lines == 0 {
        findings.push(Finding {
            ar: format!("البديل «{}» لا يحتاج خطوط دعم إضافية: كل الغرف ضمن حد البحر {} مم.", out[best].label, limit),
            en: format!("Alternative '{}' needs no extra support lines: every room is within the {} mm span limit.", out[best].label, limit),
        });
    } else {
        findings.push(Finding {
            ar: format!("أقل البدائل بحوراً كبيرة هو «{}» بـ {} خط دعم إضافياً؛ كل غرفة تتجاوز الحد تحتاج دعماً وسيطاً.", out[best].label, out[best].support_lines),
            en: format!("The alternative with the fewest large spans is '{}' with {} extra support lines; every room over the limit needs intermediate support.", out[best].label, out[best].support_lines),
        });
    }
    if saved > 0 {
        findings.push(Finding {
            ar: format!("«{}» يوفّر {} خط دعم مقارنةً بالبديل الأول.", out[best].label, saved),
            en: format!("'{}' saves {} support lines against the first alternative.", out[best].label, saved),
        });
    }
    for a in &out {
        for s in &a.large_spans {
            findings.push(Finding {
                ar: format!("{}: الغرفة «{}» ({}×{} مم) فوق الحد {} مم وتحتاج {} خط دعم (بحور {}×{} ≈ {}×{} مم).", a.label, s.room, s.width_mm, s.depth_mm, limit, s.support_lines, s.bays_x, s.bays_y, s.bay_mm[0], s.bay_mm[1]),
                en: format!("{}: room '{}' ({}x{} mm) is over the {} mm limit and needs {} support lines ({}x{} bays of about {}x{} mm).", a.label, s.room, s.width_mm, s.depth_mm, limit, s.support_lines, s.bays_x, s.bays_y, s.bay_mm[0], s.bay_mm[1]),
            });
        }
    }
    Ok(SpanComparison { max_span_mm: limit, alternatives: out, best, support_lines_saved_vs_first: saved, findings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Breakdown, PlacedRoom, Zone};

    fn room(key: &str, w: i64, h: i64) -> PlacedRoom {
        let rect = Rect { x: 0, y: 0, w, h };
        PlacedRoom { key: key.into(), func: "x".into(), zone: Zone::Public, needs_light: false, rect, area_m2: rect.area_m2(), target_m2: rect.area_m2(), deviation_pct: 0.0 }
    }
    fn layout(rooms: Vec<PlacedRoom>) -> Layout {
        Layout { rooms, cost: 0.0, breakdown: Breakdown::default(), reserve_m2: 0.0 }
    }

    #[test]
    fn a_room_within_the_limit_has_no_large_span() {
        assert!(large_spans(&layout(vec![room("a", 5000, 6000)]), 6000).is_empty());
    }

    #[test]
    fn a_room_over_the_limit_counts_bays_support_lines_and_excess() {
        let s = large_spans(&layout(vec![room("hall", 13_000, 5000)]), 6000);
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].bays_x, s[0].bays_y, s[0].support_lines), (3, 1, 2));
        assert_eq!(s[0].excess_mm, 7000);
        assert_eq!(s[0].bay_mm, [4334, 5000]);
    }

    #[test]
    fn the_alternative_with_fewer_support_lines_ranks_first_and_savings_are_reported() {
        let wide = layout(vec![room("hall", 13_000, 5000)]);
        let split = layout(vec![room("hall", 6000, 5000), room("hall2", 6000, 5000)]);
        let c = compare(&[("A".into(), &wide), ("B".into(), &split)], Some(6000)).unwrap();
        assert_eq!(c.best, 1);
        assert_eq!(c.alternatives[1].rank, 1);
        assert_eq!(c.alternatives[0].rank, 2);
        assert_eq!(c.alternatives[1].support_lines, 0);
        assert_eq!(c.support_lines_saved_vs_first, 2);
        assert!(c.findings[0].en.contains("needs no extra support lines") && c.findings[0].ar.contains("لا يحتاج"));
    }

    #[test]
    fn ties_break_on_excess_then_longest_span_then_order() {
        let a = layout(vec![room("r", 7000, 5000)]);
        let b = layout(vec![room("r", 6500, 5000)]);
        let c = compare(&[("A".into(), &a), ("B".into(), &b)], Some(6000)).unwrap();
        assert_eq!(c.alternatives[0].support_lines, c.alternatives[1].support_lines);
        assert_eq!(c.best, 1, "the smaller excess wins the tie");
        let same = compare(&[("A".into(), &a), ("B".into(), &a)], Some(6000)).unwrap();
        assert_eq!(same.best, 0, "identical alternatives keep the order given");
    }

    #[test]
    fn a_missing_or_bad_limit_and_bad_input_are_refused_by_name() {
        let l = layout(vec![room("a", 5000, 5000)]);
        assert!(compare(&[("A".into(), &l)], None).unwrap_err().contains("max_span_mm is required"));
        assert!(compare(&[("A".into(), &l)], Some(0)).unwrap_err().contains("positive"));
        assert!(compare(&[], Some(6000)).unwrap_err().contains("at least one"));
        assert!(compare(&[("A".into(), &layout(vec![]))], Some(6000)).unwrap_err().contains("no rooms"));
        assert!(compare(&[("A".into(), &layout(vec![room("z", 0, 5000)]))], Some(6000)).unwrap_err().contains("non-positive"));
    }
}
