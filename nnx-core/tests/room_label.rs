//! The label of a room stays inside the room.
use nnx_core::drawing::{DrawOpts, plan_svg};
use nnx_core::typed::{Space, element_of};
use nnx_core::*;

fn plan(w: i64) -> String {
    let mut doc = Document::default();
    let area = w as f64 * 3.0 / 1000.0;
    let space = Space { func: "room".into(), requirement_key: "R1".into(), area, target_area: area, x: 0, y: 0, w, h: 3000, boundary: vec![[0, 0], [w, 0], [w, 3000], [0, 3000]], net_area: None, glazing_ratio: None, extra: Default::default() };
    doc.commit("room", Origin::User, vec![Command::Create(element_of(&space, Source::User))]).unwrap();
    plan_svg(&doc.model, None, &DrawOpts::default()).unwrap()
}

#[test]
fn a_wide_room_prints_one_line_and_a_narrow_one_splits_and_shrinks() {
    let wide = plan(6000);
    assert!(wide.contains("m²  ·  6.00 × 3.00</text>") && !wide.contains("style=\"font-size"));
    let narrow = plan(1500);
    assert!(narrow.contains(">1.50 × 3.00</text>") && !narrow.contains("m²  ·"), "the sides have a line of their own");
    let tiny = plan(300);
    assert!(tiny.contains("style=\"font-size:6.0px\">"), "never below the smallest readable size");
}
