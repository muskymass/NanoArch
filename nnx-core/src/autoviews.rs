//! Automatic elevations and sections for the whole building, every level in each of them.
//!
//! One call draws the four elevations, the sections A-A and B-B through the middle of the building, and one extra section
//! through every stair shaft (the shaft is the same on every level, so its cut shows the stair opening on all of them). The
//! drawings come from `elevation`, which draws from the 3D scene, so each shows every level of the model: the level marks, the
//! storey heights and the slabs. Nothing is defaulted: the parapet height is the caller's input (`ViewOpts::parapet_m`).
//!
//! Each view reports the levels it shows next to the levels the model has; a model level missing from a view is reported by
//! name and `complete` is false, never passed silently. Only what the model stores is drawn: no stair flight geometry (the
//! cut passes through the shaft room), no site datum, no printed scale.

use crate::building;
use crate::elevation::{self, Cut, Side, ViewOpts, ViewShapes, ViewSpec};
use crate::model::Model;
use crate::typed::{Level, Space, Typed};
use serde::Serialize;

/// Cuts closer than this to an existing cut on the same axis are the same section and are not drawn twice.
const SAME_CUT_M: f64 = 0.3;

#[derive(Debug, Clone, Serialize)]
pub struct AutoView {
    /// File name stem, e.g. `elevation-north`, `section-A-A`, `section-S1`.
    pub name: String,
    /// `elevation` or `section`.
    pub kind: &'static str,
    /// Why this view exists, in Arabic and English.
    pub reason_en: String,
    pub reason_ar: String,
    pub svg: String,
    /// Levels the drawing shows, bottom to top, with the elevation of each above the ground floor top (metres).
    pub levels_shown: Vec<(String, f64)>,
    /// Model levels that the drawing does not show (empty when the view is complete).
    pub levels_missing: Vec<String>,
    /// Where a section is cut: the axis (`y` or `x`) and the position in metres.
    pub cut: Option<(String, f64)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ViewSet {
    pub views: Vec<AutoView>,
    /// The model's level names, bottom to top.
    pub model_levels: Vec<String>,
    /// True when every view shows every level of the model.
    pub complete: bool,
    pub notes: Vec<String>,
}

fn model_level_names(model: &Model) -> Vec<String> {
    let mut v: Vec<(i64, String)> = model.elements.values().filter(|e| e.kind == Level::KIND).filter_map(|e| e.typed::<Level>().ok()).map(|l| (l.index, l.name)).collect();
    v.sort();
    v.into_iter().map(|(_, n)| n).collect()
}

/// Section planes through the stair shafts: `(axis, position in metres)`. The plane runs along the shaft's longer side, so a
/// long shaft is cut lengthwise; one position per shaft however many levels repeat it.
fn shaft_cuts(model: &Model) -> Vec<(bool, f64)> {
    // `true` = AtY (plane y = at), `false` = AtX.
    let mut out: Vec<(bool, f64, i64, i64)> = Vec::new();
    for e in model.elements.values().filter(|e| e.kind == Space::KIND) {
        let Ok(s) = e.typed::<Space>() else { continue };
        if !building::is_shaft(&s.requirement_key) {
            continue;
        }
        let (cx, cy) = (s.x + s.w / 2, s.y + s.h / 2);
        if out.iter().any(|(_, _, x, y)| *x == cx && *y == cy) {
            continue;
        }
        let at_y = s.w >= s.h;
        let at = if at_y { cy as f64 / 1000.0 } else { cx as f64 / 1000.0 };
        out.push((at_y, at, cx, cy));
    }
    out.sort_by_key(|a| (a.2, a.3));
    out.into_iter().map(|(y, at, _, _)| (y, at)).collect()
}

fn shown(shapes: &ViewShapes, model_levels: &[String]) -> (Vec<(String, f64)>, Vec<String>) {
    let missing = model_levels.iter().filter(|n| !shapes.levels.iter().any(|(m, _)| m == *n)).cloned().collect();
    (shapes.levels.clone(), missing)
}

/// Every elevation and section the building needs, drawn in one pass. Fails with the drawing error of the first view that
/// cannot be drawn (an empty model, a bad parapet height).
pub fn build(model: &Model, opts: &ViewOpts) -> Result<ViewSet, String> {
    let model_levels = model_level_names(model);
    let mut specs: Vec<(String, String, String, ViewSpec)> = Vec::new();
    for (side, key, en, ar) in [
        (Side::North, "north", "North elevation", "الواجهة الشمالية"),
        (Side::South, "south", "South elevation", "الواجهة الجنوبية"),
        (Side::East, "east", "East elevation", "الواجهة الشرقية"),
        (Side::West, "west", "West elevation", "الواجهة الغربية"),
    ] {
        specs.push((format!("elevation-{key}"), en.into(), ar.into(), ViewSpec::Elevation(side)));
    }
    let mut notes = Vec::new();
    let mut cuts: Vec<(bool, f64)> = Vec::new();
    for name in ["A-A", "B-B"] {
        let Some(cut) = elevation::default_cut(model, name) else { return Err(elevation::ViewError::Empty.to_string()) };
        let (at_y, at) = match &cut {
            Cut::AtY { at, .. } => (true, *at),
            Cut::AtX { at, .. } => (false, *at),
        };
        cuts.push((at_y, at));
        let (en, ar) = if at_y { ("Section A-A through the middle of the building depth", "المقطع A-A عبر منتصف عمق المبنى") } else { ("Section B-B through the middle of the building width", "المقطع B-B عبر منتصف عرض المبنى") };
        specs.push((format!("section-{name}"), en.into(), ar.into(), ViewSpec::Section(cut)));
    }
    let mut n = 0;
    for (at_y, at) in shaft_cuts(model) {
        if cuts.iter().any(|(y, a)| *y == at_y && (a - at).abs() < SAME_CUT_M) {
            notes.push(format!("a stair shaft at {} = {at:.2} m lies on an existing section and gets no extra one", if at_y { "y" } else { "x" }));
            continue;
        }
        cuts.push((at_y, at));
        n += 1;
        let name = format!("S{n}");
        let cut = if at_y { Cut::AtY { name: name.clone(), at } } else { Cut::AtX { name: name.clone(), at } };
        specs.push((
            format!("section-{name}"),
            format!("Section {name} through a stair shaft ({} = {at:.2} m)", if at_y { "y" } else { "x" }),
            format!("المقطع {name} عبر بئر الدرج ({} = {at:.2} م)", if at_y { "y" } else { "x" }),
            ViewSpec::Section(cut),
        ));
    }
    if n == 0 {
        notes.push("no stair shaft in the model: only the two mid-building sections are drawn".to_string());
    }
    notes.push("every view shows all levels of the model; stair flight geometry is not drawn (the cut passes through the shaft room) and no scale is printed".to_string());

    let mut views = Vec::new();
    for (name, en, ar, spec) in specs {
        let (svg, shapes, kind, cut) = match &spec {
            ViewSpec::Elevation(side) => {
                let (svg, shapes) = (elevation::elevation_svg(model, *side, opts).map_err(|e| e.to_string())?, elevation::elevation_shapes(model, *side, opts).map_err(|e| e.to_string())?);
                (svg, shapes, "elevation", None)
            }
            ViewSpec::Section(cut) => {
                let (svg, shapes) = (elevation::section_svg(model, cut, opts).map_err(|e| e.to_string())?, elevation::section_shapes(model, cut, opts).map_err(|e| e.to_string())?);
                let c = match cut {
                    Cut::AtY { at, .. } => ("y".to_string(), *at),
                    Cut::AtX { at, .. } => ("x".to_string(), *at),
                };
                (svg, shapes, "section", Some(c))
            }
        };
        let (levels_shown, levels_missing) = shown(&shapes, &model_levels);
        views.push(AutoView { name, kind, reason_en: en, reason_ar: ar, svg, levels_shown, levels_missing, cut });
    }
    let complete = views.iter().all(|v| v.levels_missing.is_empty());
    if !complete {
        for v in views.iter().filter(|v| !v.levels_missing.is_empty()) {
            notes.push(format!("{} does not show the level(s): {}", v.name, v.levels_missing.join(", ")));
        }
    }
    Ok(ViewSet { views, model_levels, complete, notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Command, Origin};
    use crate::model::Source;
    use crate::typed::{Slab, Wall, element_of};
    use crate::Document;

    fn space(key: &str, func: &str, x: i64, y: i64, w: i64, h: i64) -> Space {
        Space { func: func.into(), requirement_key: key.into(), area: (w * h) as f64 / 1e6, target_area: 0.0, x, y, w, h, boundary: vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]], net_area: None, glazing_ratio: None, extra: Default::default() }
    }

    fn on_level(mut e: crate::model::Element, level: crate::model::Id) -> crate::model::Element {
        e.level_id = Some(level);
        e
    }

    fn two_storey(with_stair: bool) -> Document {
        let mut doc = Document::default();
        let mut cmds = Vec::new();
        let mut lv_ids = Vec::new();
        for (i, name) in ["Ground", "First"].into_iter().enumerate() {
            let lv = Level { name: name.into(), index: i as i64, extra: Default::default() };
            let el = element_of(&lv, Source::User);
            lv_ids.push(el.id);
            cmds.push(Command::Create(el));
        }
        for (i, id) in lv_ids.iter().enumerate() {
            let mut rooms = vec![space("r1", "bedroom", 0, 0, 6000, 5000)];
            if with_stair {
                rooms.push(space("stair", "stair", 6000, 0, 2000, 5000));
            }
            for mut s in rooms {
                if i > 0 {
                    s.requirement_key = format!("{}@{i}", s.requirement_key);
                }
                cmds.push(Command::Create(on_level(element_of(&s, Source::User), *id)));
            }
            let slab = Slab { x: 0, y: 0, w: 8000, h: 5000, thickness: 150, kind: "solid".into(), room: "r1".into(), extra: Default::default() };
            cmds.push(Command::Create(on_level(element_of(&slab, Source::User), *id)));
            for (x1, y1, x2, y2) in [(0, 0, 8000, 0), (8000, 0, 8000, 5000), (8000, 5000, 0, 5000), (0, 5000, 0, 0)] {
                let w = Wall { x1, y1, x2, y2, thickness: 200, height: 3000, length: (x2 - x1).abs() + (y2 - y1).abs(), is_exterior: true, room_a: "r1".into(), room_b: None, extra: Default::default() };
                cmds.push(Command::Create(on_level(element_of(&w, Source::User), *id)));
            }
        }
        doc.commit("two storeys", Origin::User, cmds).unwrap();
        doc
    }

    #[test]
    fn draws_four_elevations_two_sections_and_one_per_stair_shaft_with_every_level() {
        let doc = two_storey(true);
        let set = build(&doc.model, &ViewOpts::default()).unwrap();
        let names: Vec<&str> = set.views.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["elevation-north", "elevation-south", "elevation-east", "elevation-west", "section-A-A", "section-B-B", "section-S1"]);
        assert_eq!(set.model_levels, ["Ground", "First"]);
        assert!(set.complete, "{:?}", set.notes);
        for v in &set.views {
            let n: Vec<&str> = v.levels_shown.iter().map(|(n, _)| n.as_str()).collect();
            assert_eq!(n, ["Ground", "First"], "{}", v.name);
            assert!(v.svg.starts_with("<svg"), "{}", v.name);
            assert!(v.svg.contains("First"), "{}", v.name);
        }
        let s1 = set.views.iter().find(|v| v.name == "section-S1").unwrap();
        // The shaft is 2 m wide and 5 m deep: cut lengthwise at x = its centre line.
        assert_eq!(s1.cut, Some(("x".to_string(), 7.0)));
    }

    #[test]
    fn without_a_stair_shaft_only_the_two_mid_sections_are_drawn_and_it_says_so() {
        let doc = two_storey(false);
        let set = build(&doc.model, &ViewOpts::default()).unwrap();
        assert_eq!(set.views.len(), 6);
        assert!(set.notes.iter().any(|n| n.contains("no stair shaft")));
    }

    #[test]
    fn a_shaft_on_an_existing_section_adds_no_duplicate() {
        let mut doc = Document::default();
        let lv = Level { name: "Ground".into(), index: 0, extra: Default::default() };
        let lel = element_of(&lv, Source::User);
        let id = lel.id;
        let cmds = vec![
            Command::Create(lel),
            // 4 m wide room with the shaft in its middle: the B-B plane (x = 2 m) already passes through the shaft's centre line.
            Command::Create(on_level(element_of(&space("stair", "stair", 1500, 0, 1000, 5000), Source::User), id)),
            Command::Create(on_level(element_of(&space("r1", "bedroom", 0, 0, 1500, 5000), Source::User), id)),
            Command::Create(on_level(element_of(&space("r2", "bedroom", 2500, 0, 1500, 5000), Source::User), id)),
        ];
        doc.commit("one storey", Origin::User, cmds).unwrap();
        let set = build(&doc.model, &ViewOpts::default()).unwrap();
        assert_eq!(set.views.iter().filter(|v| v.kind == "section").count(), 2);
        assert!(set.notes.iter().any(|n| n.contains("gets no extra one")));
    }

    #[test]
    fn an_empty_model_is_reported_not_drawn() {
        let doc = Document::default();
        assert!(build(&doc.model, &ViewOpts::default()).is_err());
    }

    #[test]
    fn a_bad_parapet_is_refused() {
        let doc = two_storey(true);
        let opts = ViewOpts { parapet_m: Some(-1.0), ..Default::default() };
        assert!(build(&doc.model, &opts).unwrap_err().contains("parapet"));
    }
}
