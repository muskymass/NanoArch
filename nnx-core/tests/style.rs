//! Architectural styles as facades on the 3D scene, the elevations, the sections and the DXF.

use nnx_core::dxf::{self, DxfOpts};
use nnx_core::elevation::{self, Cut, Side, ViewOpts, ViewSpec};
use nnx_core::model3d;
use nnx_core::style::{self, Style};
use nnx_core::typed::{Door, Slab, Space, Wall, Window, element_of};
use nnx_core::*;
use std::collections::BTreeMap;

fn space(key: &str, x: i64) -> Space {
    Space { func: "living".into(), requirement_key: key.into(), area: 24.0, target_area: 24.0, x, y: 0, w: 6000, h: 4000, boundary: vec![[x, 0], [x + 6000, 0], [x + 6000, 4000], [x, 4000]], net_area: None, glazing_ratio: None, extra: Default::default() }
}

fn wall(x1: i64, y1: i64, x2: i64, y2: i64, a: &str) -> Wall {
    Wall { x1, y1, x2, y2, thickness: 200, height: 3000, length: (x2 - x1).abs() + (y2 - y1).abs(), is_exterior: true, room_a: a.into(), room_b: None, extra: Default::default() }
}

/// Two rooms 12 x 4 m, exterior walls 200 mm thick and 3 m high, a door (x 0.55..1.45 m) and a window (x 3.25..4.75 m) in the
/// south wall, a 150 mm slab on each room.
fn house() -> Document {
    let mut cmds = vec![Command::Create(element_of(&space("a", 0), Source::User)), Command::Create(element_of(&space("b", 6000), Source::User))];
    let south = element_of(&wall(0, 0, 12000, 0, "a"), Source::User);
    let sid = south.id;
    cmds.push(Command::Create(south));
    for w in [wall(12000, 0, 12000, 4000, "b"), wall(12000, 4000, 0, 4000, "b"), wall(0, 4000, 0, 0, "a")] {
        cmds.push(Command::Create(element_of(&w, Source::User)));
    }
    let door = Door { width: 900, height: 2100, offset_center: 1000, is_egress: true, from: "a".into(), to: "out".into(), opens_into: "a".into(), extra: Default::default() };
    let win = Window { width: 1500, height: 1200, sill_height: 900, offset_center: 4000, room: "a".into(), extra: Default::default() };
    cmds.push(Command::Create(element_of(&door, Source::User).hosted_by(sid)));
    cmds.push(Command::Create(element_of(&win, Source::User).hosted_by(sid)));
    for (key, x) in [("a", 0), ("b", 6000)] {
        cmds.push(Command::Create(element_of(&Slab { x, y: 0, w: 6000, h: 4000, thickness: 150, kind: "roof".into(), room: key.into(), extra: Default::default() }, Source::User)));
    }
    let mut doc = Document::default();
    doc.commit("house", Origin::User, cmds).unwrap();
    doc
}

fn pieces(model: &Model, s: Style) -> Vec<model3d::Box3> {
    model3d::build_styled(model, Some(s)).boxes.into_iter().filter(|b| b.kind == "Facade").collect()
}

fn of<'a>(v: &'a [model3d::Box3], motif: &str, side: &str) -> Vec<&'a model3d::Box3> {
    v.iter().filter(|b| b.class == format!("facade-{motif}:{side}")).collect()
}

#[test]
fn a_style_is_found_from_its_key_its_label_in_either_language_or_a_keyword() {
    for (text, want) in [
        ("modern_najdi", Style::ModernNajdi),
        ("Modern Najdi", Style::ModernNajdi),
        ("نجدي حديث", Style::ModernNajdi),
        ("najdi", Style::ModernNajdi),
        ("Contemporary", Style::Contemporary),
        ("معاصر", Style::Contemporary),
        ("modern", Style::Contemporary),
        ("neo_classical", Style::NeoClassical),
        ("Neo-Classical", Style::NeoClassical),
        ("كلاسيكي حديث", Style::NeoClassical),
        ("minimalist", Style::Minimalist),
        ("مينيمال", Style::Minimalist),
    ] {
        assert_eq!(style::parse(text), Some(want), "{text}");
    }
    for text in ["", "  ", "baroque", "xyz"] {
        assert_eq!(style::parse(text), None, "{text}");
    }
    for s in style::ALL {
        assert_eq!(style::parse(s.key()), Some(s));
        assert_eq!(style::parse(s.label().0), Some(s));
        assert_eq!(style::parse(s.label().1), Some(s));
    }
}

#[test]
fn a_style_adds_visual_solids_to_the_scene_and_leaves_the_model_and_the_plain_scene_alone() {
    let doc = house();
    let hash = doc.model.state_hash();
    let plain = model3d::build(&doc.model);
    assert_eq!(model3d::build_styled(&doc.model, None).boxes.len(), plain.boxes.len());
    for s in style::ALL {
        let styled = model3d::build_styled(&doc.model, Some(s));
        assert!(styled.boxes.len() > plain.boxes.len(), "{s:?} adds solids");
        assert_eq!(&styled.boxes[..plain.boxes.len()].iter().map(|b| &b.id).collect::<Vec<_>>(), &plain.boxes.iter().map(|b| &b.id).collect::<Vec<_>>(), "the building's own solids come first, unchanged");
        let extra = &styled.boxes[plain.boxes.len()..];
        assert!(extra.iter().all(|b| b.kind == "Facade" && b.discipline == "architectural" && b.visual && b.id.starts_with("facade:")));
        let mut ids: Vec<&String> = extra.iter().map(|b| &b.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), extra.len(), "unique ids");
        // Every solid is on one side and within a metre of the building's outline (plus the roof motifs above it).
        for b in extra {
            assert!(style::split_class(&b.class).is_some_and(|(_, side)| ["south", "north", "west", "east"].contains(&side)), "{}", b.class);
            assert!(b.min[0] >= -1.0 && b.max[0] <= 13.0 && b.min[1] >= -1.0 && b.max[1] <= 5.0, "{b:?}");
            assert!(b.min[2] >= -0.01 && b.max[2] <= 4.2, "{b:?}");
        }
        assert!(styled.max[2] >= plain.max[2] && styled.min[0] <= plain.min[0]);
        // Quantities and the structure do not see the facade: the model is untouched.
        assert_eq!(doc.model.state_hash(), hash);
    }
}

#[test]
fn modern_najdi_has_stepped_merlons_on_a_cap_and_slits_that_avoid_the_openings() {
    let doc = house();
    let p = pieces(&doc.model, Style::ModernNajdi);
    for side in ["south", "north", "west", "east"] {
        assert_eq!(of(&p, "cap", side).len(), 1, "{side}");
    }
    // South facade 12.2 m: 13 merlons of 3 steps each, steps 0.2 m higher and narrower.
    let m = of(&p, "merlon", "south");
    assert_eq!(m.len(), 13 * 3);
    let base = m.iter().map(|b| b.min[2]).fold(f64::MAX, f64::min);
    near(base, 3.15 + 0.35, 1e-9);
    let mut widths: Vec<f64> = m.iter().map(|b| ((b.max[0] - b.min[0]) * 100.0).round() / 100.0).collect();
    widths.sort_by(f64::total_cmp);
    widths.dedup();
    assert_eq!(widths, vec![0.18, 0.34, 0.5], "a stepped triangle");
    let mut heights: Vec<f64> = m.iter().map(|b| ((b.max[2] - b.min[2]) * 100.0).round() / 100.0).collect();
    heights.dedup();
    assert_eq!(heights, vec![0.2]);
    // Slits: 12.2 / 1.8 -> 6 of them; those against the door (x 0.55..1.45) or the window (x 3.25..4.75) are left out.
    let slits = of(&p, "slit", "south");
    assert!(!slits.is_empty() && slits.len() < 6, "{} slits", slits.len());
    for s in &slits {
        let (a0, a1) = (s.min[0], s.max[0]);
        assert!(!(a1 > 0.55 - 0.3 && a0 < 1.45 + 0.3) && !(a1 > 3.25 - 0.3 && a0 < 4.75 + 0.3), "a slit at x {a0}..{a1} is beside an opening");
        near(a1 - a0, 0.12, 1e-9);
        assert!(s.min[2] >= 0.9 - 1e-9 && s.max[2] <= 2.4 + 1e-9, "{s:?}");
    }
    // The north wall has no openings, so every one of its slits is drawn: 12.2 / 1.8 = 6.
    assert_eq!(of(&p, "slit", "north").len(), 6);
}

#[test]
fn contemporary_has_cantilevered_fascias_and_curtain_glazing_on_the_entrance_facade_only() {
    let doc = house();
    let p = pieces(&doc.model, Style::Contemporary);
    for side in ["south", "north", "west", "east"] {
        let c = of(&p, "cantilever", side);
        assert_eq!(c.len(), 1, "one floor: {side}");
        near(c[0].max[1].max(c[0].max[0]) - c[0].min[1].min(c[0].min[0]), c[0].max[1].max(c[0].max[0]) - c[0].min[1].min(c[0].min[0]), 1e-9);
    }
    // Overhang 0.6 m beyond the outer face of the south wall (y = -0.1), at the slab (z 3.0..3.15).
    let s = of(&p, "cantilever", "south")[0];
    near(s.min[1], -0.1 - 0.6, 1e-9);
    near(s.min[2], 3.0, 1e-9);
    near(s.max[2], 3.15, 1e-9);
    assert!(s.min[0] < -0.1 && s.max[0] > 12.1, "it reaches past the corners");
    // The door is on the south wall: that is the entrance facade.
    assert_eq!(of(&p, "curtain", "south").len(), 1);
    for other in ["north", "west", "east"] {
        assert!(of(&p, "curtain", other).is_empty(), "{other}");
    }
    let c = of(&p, "curtain", "south")[0];
    near((c.max[0] - c.min[0]) / 12.2, 0.7, 1e-9);
    assert_eq!(of(&p, "mullion", "south").len(), ((12.2f64 * 0.7 / 1.2).floor() as usize) + 1);
}

#[test]
fn neo_classical_has_a_plinth_pilasters_at_the_corners_and_a_stepped_cornice() {
    let doc = house();
    let p = pieces(&doc.model, Style::NeoClassical);
    let plinth = of(&p, "plinth", "south")[0];
    near(plinth.min[2], 0.0, 1e-9);
    near(plinth.max[2], 0.6, 1e-9);
    let pil = of(&p, "pilaster", "south");
    // 12.2 m / 3.6 m -> 3 bays, so 4 pilasters, the first and last 0.2 m in from the ends.
    assert_eq!(pil.len(), 4);
    near(pil[0].min[0] + 0.175, -0.1 + 0.2, 1e-9);
    near(pil[3].max[0] - 0.175, 12.1 - 0.2, 1e-9);
    assert!(pil.iter().all(|b| (b.max[1] - b.min[1] - 0.12).abs() < 1e-9 && b.min[2] >= 0.6 - 1e-9 && b.max[2] <= 3.15 - 0.36 + 1e-9));
    let mut steps = of(&p, "cornice", "south");
    steps.sort_by(|a, b| a.min[2].total_cmp(&b.min[2]));
    let depth: Vec<f64> = steps.iter().map(|b| ((b.min[1].abs() - 0.1) * 100.0).round() / 100.0).collect();
    assert_eq!(depth, vec![0.12, 0.24, 0.36], "each step projects further: a cornice");
    assert_eq!(of(&p, "cap", "south").len(), 1);
}

#[test]
fn minimalist_is_a_thin_cap_and_a_reveal_at_each_floor_line() {
    let doc = house();
    let p = pieces(&doc.model, Style::Minimalist);
    assert_eq!(p.len(), 4 * 2, "a cap and one reveal per side for one floor");
    let cap = of(&p, "cap", "east")[0];
    near(cap.max[2] - cap.min[2], 0.25, 1e-9);
    let reveal = of(&p, "reveal", "east")[0];
    near(reveal.max[2] - reveal.min[2], 0.04, 1e-9);
}

#[test]
fn the_elevations_draw_the_facade_of_the_side_they_show_and_only_when_a_style_is_set() {
    let doc = house();
    let plain = elevation::elevation_svg(&doc.model, Side::South, &ViewOpts::default()).unwrap();
    assert!(!plain.contains("class=\"facade-"));
    let with = |s: Style, side: Side| elevation::elevation_svg(&doc.model, side, &ViewOpts { style: Some(s), ..ViewOpts::default() }).unwrap();
    let najdi = with(Style::ModernNajdi, Side::South);
    assert!(najdi.contains("class=\"facade-merlon\"") && najdi.contains("class=\"facade-slit\"") && najdi.contains("class=\"facade-cap\""));
    assert_eq!(najdi.matches("class=\"facade-merlon\"").count(), 13 * 3);
    assert!(najdi.contains("data-id=\"facade:"), "facade solids carry their own ids");
    let contemporary_south = with(Style::Contemporary, Side::South);
    assert!(contemporary_south.contains("class=\"facade-curtain\"") && contemporary_south.contains("class=\"facade-cantilever\"") && contemporary_south.contains("class=\"facade-mullion\""));
    assert!(!with(Style::Contemporary, Side::North).contains("class=\"facade-curtain\""), "no curtain glazing on the back");
    assert!(with(Style::NeoClassical, Side::East).contains("class=\"facade-pilaster\"") && with(Style::NeoClassical, Side::East).contains("class=\"facade-cornice\""));
    assert!(with(Style::Minimalist, Side::West).contains("class=\"facade-reveal\""));
    // The drawing grows to include the motifs above the roof.
    let h = |svg: &str| -> f64 { svg.split("height=\"").nth(1).unwrap().split('"').next().unwrap().parse().unwrap() };
    assert!(h(&najdi) > h(&plain), "the merlons are taller than the plain roof");
    // One face of the building is not mixed with another: the north motifs are not in the south elevation.
    let shapes = elevation::elevation_shapes(&doc.model, Side::South, &ViewOpts { style: Some(Style::ModernNajdi), ..ViewOpts::default() }).unwrap();
    assert_eq!(shapes.shapes.iter().filter(|s| s.class == "facade-merlon").count(), 13 * 3);
}

#[test]
fn sections_show_the_facade_solids_they_cut_or_see_beyond() {
    let doc = house();
    let opts = ViewOpts { style: Some(Style::Contemporary), ..ViewOpts::default() };
    let svg = elevation::section_svg(&doc.model, &Cut::AtY { name: "A-A".into(), at: 2.0 }, &opts).unwrap();
    assert!(svg.contains("class=\"facade-cantilever\""), "the west and east fascias are cut by the plane y = 2");
    let plain = elevation::section_svg(&doc.model, &Cut::AtY { name: "A-A".into(), at: 2.0 }, &ViewOpts::default()).unwrap();
    assert!(!plain.contains("class=\"facade-"));
}

#[test]
fn the_dxf_puts_the_facade_on_its_own_layer() {
    let doc = house();
    let opts = ViewOpts { style: Some(Style::NeoClassical), ..ViewOpts::default() };
    let d = DxfOpts { title: "T".into(), labels: BTreeMap::new(), mep: false };
    let out = dxf::export_dxf_views(&doc.model, &[None], &[ViewSpec::Elevation(Side::South)], &opts, &d).unwrap();
    assert!(out.counts.get("V-FACADE").copied().unwrap_or(0) >= 9, "{:?}", out.counts);
    assert!(out.text.contains("2\nV-FACADE\n70\n0\n62\n"), "the layer is defined");
    let plain = dxf::export_dxf_views(&doc.model, &[None], &[ViewSpec::Elevation(Side::South)], &ViewOpts::default(), &d).unwrap();
    assert!(!plain.counts.contains_key("V-FACADE"));
}

fn near(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol, "{a} vs {b}");
}
