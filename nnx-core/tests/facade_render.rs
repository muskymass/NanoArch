use nnx_core::facade_render::{self, RenderInput};
use nnx_core::finishes;
use nnx_core::project::{self, NanoArchProject};
use nnx_core::typed::{Slab, Space, Wall, element_of};
use nnx_core::*;

fn plan() -> Document {
    let space = Space { func: "living".into(), requirement_key: "a".into(), area: 24.0, target_area: 24.0, x: 0, y: 0, w: 6000, h: 4000, boundary: vec![[0, 0], [6000, 0], [6000, 4000], [0, 4000]], net_area: None, glazing_ratio: None, extra: Default::default() };
    let mut cmds = vec![Command::Create(element_of(&space, Source::User))];
    for (x1, y1, x2, y2) in [(0, 0, 6000, 0), (6000, 0, 6000, 4000), (6000, 4000, 0, 4000), (0, 4000, 0, 0)] {
        let w = Wall { x1, y1, x2, y2, thickness: 200, height: 3000, length: (x2 - x1).abs() + (y2 - y1).abs(), is_exterior: true, room_a: "a".into(), room_b: None, extra: Default::default() };
        cmds.push(Command::Create(element_of(&w, Source::User)));
    }
    cmds.push(Command::Create(element_of(&Slab { x: 0, y: 0, w: 6000, h: 4000, thickness: 150, kind: "solid".into(), room: "a".into(), extra: Default::default() }, Source::User)));
    cmds.push(project::create_command(&NanoArchProject::default()).unwrap());
    let mut doc = Document::default();
    doc.commit("plan", Origin::User, cmds).unwrap_or_else(|e| panic!("{e}"));
    doc
}

fn input() -> RenderInput {
    // Riyadh-like latitude, an early-summer day, mid-morning, plan up = true north.
    RenderInput { latitude_deg: 24.7, day: 172, solar_hour: 10.0, north_offset_deg: 0.0 }
}

fn stone() -> &'static finishes::Finish {
    finishes::find("facade", "riyadh_stone").unwrap()
}

#[test]
fn the_sun_matches_the_analysis_module_and_points_where_the_user_turned_north() {
    let s = facade_render::sun_for(&input()).unwrap();
    let a = analysis::sun(24.7, 172, 10.0).unwrap();
    assert_eq!(s.altitude_deg, a.altitude_deg);
    // Morning sun is in the east half: positive x in the plan when north is up.
    assert!(s.direction[0] > 0.0 && s.direction[2] > 0.0);
    let len = s.direction.iter().map(|v| v * v).sum::<f64>().sqrt();
    assert!((len - 1.0).abs() < 1e-4, "unit vector, got {len}");
    // Turning the plan's +y to compass 90 (east) puts the morning sun along +y of the plan instead.
    let turned = facade_render::sun_for(&RenderInput { north_offset_deg: 90.0, ..input() }).unwrap();
    assert!(turned.direction[1].abs() > turned.direction[0].abs() * 0.5 && turned.direction[0] < s.direction[0]);
}

#[test]
fn a_sun_below_the_horizon_and_impossible_inputs_are_refused_by_name() {
    let night = RenderInput { solar_hour: 1.0, ..input() };
    assert!(facade_render::sun_for(&night).unwrap_err().contains("below the horizon"));
    assert!(facade_render::sun_for(&RenderInput { latitude_deg: 95.0, ..input() }).unwrap_err().contains("latitude_deg"));
    assert!(facade_render::sun_for(&RenderInput { day: 0, ..input() }).unwrap_err().contains("day"));
    assert!(facade_render::sun_for(&RenderInput { day: 367, ..input() }).unwrap_err().contains("day"));
    assert!(facade_render::sun_for(&RenderInput { solar_hour: 24.0, ..input() }).unwrap_err().contains("solar_hour"));
    assert!(facade_render::sun_for(&RenderInput { north_offset_deg: f64::NAN, ..input() }).unwrap_err().contains("north_offset_deg"));
}

#[test]
fn the_page_is_one_offline_file_with_the_model_the_finish_and_the_sun_inside() {
    let doc = plan();
    let scene = model3d::build(&doc.model);
    let page = facade_render::render_html(&scene, "Villa </script> test", false, stone(), &input()).unwrap();
    let html = &page.html;
    assert!(html.starts_with("<!doctype html>") && html.contains("<canvas id=\"gl\">") && page.name == "facade-render.html");
    assert!(html.contains("\"id\":\"riyadh_stone\"") && html.contains("\"type\":0"));
    assert!(!html.contains("\"k\""), "only render fields are embedded");
    for banned in ["src=\"http", "href=\"http", "fetch(", "XMLHttpRequest", "import("] {
        assert!(!html.contains(banned), "found {banned}");
    }
    assert!(!html.contains("Villa </script>"));
    let data = &html[html.find("const D=").unwrap()..html.find("(function(){").unwrap()];
    assert!(!data.contains("</"), "the data block must not close the script");
    // Physically based terms are really in the shader.
    for term in ["Dg=a2/(PI*dd*dd)", "pow(1.0-vdh,5.0)", "aces(", "pk(gl_FragCoord.z)"] {
        assert!(html.contains(term), "missing {term}");
    }
    assert!(page.boxes > 0 && page.facade == "riyadh_stone" && page.notes.len() >= 3);
    // Structure and services are not drawn.
    assert!(!data.contains("Column") && !data.contains("MepRun"));
}

#[test]
fn each_catalog_facade_finish_selects_its_own_shader_and_floors_are_refused() {
    let doc = plan();
    let scene = model3d::build(&doc.model);
    for (id, ty) in [("riyadh_stone", 0), ("travertine", 1), ("profile_plaster", 2), ("wood_cladding", 3)] {
        let f = finishes::find("facade", id).unwrap();
        let page = facade_render::render_html(&scene, "x", true, f, &input()).unwrap();
        assert!(page.html.contains(&format!("\"type\":{ty}")), "{id}");
        assert!(page.html.contains("dir=\"rtl\""));
    }
    let floor = finishes::find("floor", "marble").unwrap();
    assert!(facade_render::render_html(&scene, "x", false, floor, &input()).unwrap_err().contains("not a facade finish"));
}

#[test]
fn exterior_walls_take_the_facade_material_and_glass_is_kept_apart() {
    let doc = plan();
    let scene = model3d::build(&doc.model);
    let page = facade_render::render_html(&scene, "x", false, stone(), &input()).unwrap();
    let data = &page.html[page.html.find("const D=").unwrap() + 8..page.html.find(";\n(function(){").unwrap()];
    let v: serde_json::Value = serde_json::from_str(data).unwrap();
    let boxes = v["boxes"].as_array().unwrap();
    assert!(boxes.iter().any(|b| b["m"] == 0), "exterior walls use the facade material");
    assert!(boxes.iter().any(|b| b["m"] == 4), "the slab is concrete");
    assert!(boxes.iter().all(|b| (b["g"] == true) == (b["m"] == 2)), "glass flag matches the glass material");
    assert_eq!(v["sun"]["lat"], 24.7);
}

#[test]
fn an_empty_model_has_no_render() {
    let scene = model3d::build(&Document::default().model);
    assert!(facade_render::render_html(&scene, "x", false, stone(), &input()).is_err());
}
