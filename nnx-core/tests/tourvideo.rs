use nnx_core::facade_render::RenderInput;
use nnx_core::finishes;
use nnx_core::project::{self, NanoArchProject};
use nnx_core::tourvideo::{self, TourVideoInput};
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

fn sun() -> RenderInput {
    RenderInput { latitude_deg: 24.7, day: 172, solar_hour: 10.0, north_offset_deg: 0.0 }
}

fn video() -> TourVideoInput {
    TourVideoInput { duration_s: 20.0, fps: 30, turns: 1.0, clockwise: false, pitch_deg: 20.0, distance_factor: 1.0, width: 1920, height: 1080, bitrate_kbps: 8000 }
}

fn stone() -> &'static finishes::Finish {
    finishes::find("facade", "riyadh_stone").unwrap()
}

#[test]
fn the_path_is_a_constant_speed_circle_that_closes_and_respects_the_direction() {
    let v = video();
    let tau = std::f64::consts::TAU;
    assert_eq!(tourvideo::yaw_at(&v, 0.0), tourvideo::START_YAW_RAD);
    assert!((tourvideo::yaw_at(&v, 10.0) - (tourvideo::START_YAW_RAD + tau / 2.0)).abs() < 1e-12);
    assert!((tourvideo::yaw_at(&v, 20.0) - (tourvideo::START_YAW_RAD + tau)).abs() < 1e-12);
    let (a, b, c) = (tourvideo::yaw_at(&v, 3.0), tourvideo::yaw_at(&v, 6.0), tourvideo::yaw_at(&v, 9.0));
    assert!(((b - a) - (c - b)).abs() < 1e-12);
    let cw = TourVideoInput { clockwise: true, ..v };
    assert!(tourvideo::yaw_at(&cw, 5.0) < tourvideo::START_YAW_RAD);
    assert!((tourvideo::yaw_at(&cw, 5.0) - (tourvideo::START_YAW_RAD - tau / 4.0)).abs() < 1e-12);
    let fast = TourVideoInput { turns: 2.0, ..v };
    assert!((tourvideo::yaw_at(&fast, 10.0) - (tourvideo::START_YAW_RAD + tau)).abs() < 1e-12);
    assert_eq!(tourvideo::yaw_at(&v, 99.0), tourvideo::yaw_at(&v, 20.0));
}

#[test]
fn impossible_choices_are_refused_by_name() {
    let v = video();
    let cases: Vec<(TourVideoInput, &str)> = vec![
        (TourVideoInput { duration_s: 1.0, ..v }, "duration_s"),
        (TourVideoInput { duration_s: f64::NAN, ..v }, "duration_s"),
        (TourVideoInput { duration_s: 301.0, ..v }, "duration_s"),
        (TourVideoInput { fps: 5, ..v }, "fps"),
        (TourVideoInput { fps: 120, ..v }, "fps"),
        (TourVideoInput { turns: 0.0, ..v }, "turns"),
        (TourVideoInput { turns: 5.0, ..v }, "turns"),
        (TourVideoInput { pitch_deg: -1.0, ..v }, "pitch_deg"),
        (TourVideoInput { pitch_deg: 85.0, ..v }, "pitch_deg"),
        (TourVideoInput { distance_factor: 0.1, ..v }, "distance_factor"),
        (TourVideoInput { width: 100, ..v }, "size"),
        (TourVideoInput { height: 4320, ..v }, "size"),
        (TourVideoInput { width: 1281, ..v }, "even"),
        (TourVideoInput { height: 721, ..v }, "even"),
        (TourVideoInput { bitrate_kbps: 100, ..v }, "bitrate_kbps"),
        (TourVideoInput { bitrate_kbps: 90_000, ..v }, "bitrate_kbps"),
    ];
    for (input, name) in cases {
        let e = tourvideo::validate(&input).unwrap_err();
        assert!(e.contains(name), "{name}: {e}");
    }
    assert!(tourvideo::validate(&v).is_ok());
}

#[test]
fn the_page_is_the_offline_render_with_a_preview_and_a_local_webm_recorder() {
    let doc = plan();
    let scene = model3d::build(&doc.model);
    let p = tourvideo::render_html(&scene, "Villa </script> tour", false, stone(), &sun(), &video()).unwrap();
    let html = &p.page.html;
    assert_eq!(p.page.name, "tour-video.html");
    assert_eq!(p.video_name, "tour-video.webm");
    assert_eq!(p.frame_count, 600);
    assert!((p.total_degrees - 360.0).abs() < 1e-9 && (p.degrees_per_second - 18.0).abs() < 1e-9);
    assert!(html.starts_with("<!doctype html>") && html.contains("<canvas id=\"gl\">") && !html.contains("@@"));
    for term in ["new MediaRecorder(cv.captureStream(C.fps)", "video/webm;codecs=vp9", "tour-video.webm", "\"fps\":30", "\"width\":1920", "\"kbps\":8000", "\"clockwise\":false", "FIX=[C.width,C.height]"] {
        assert!(html.contains(term), "missing {term}");
    }
    for banned in ["src=\"http", "href=\"http", "fetch(", "XMLHttpRequest", "import(", "WebSocket"] {
        assert!(!html.contains(banned), "found {banned}");
    }
    assert!(!html.contains("Villa </script>"));
    assert!(p.page.notes.iter().any(|n| n.contains("MediaRecorder")) && p.page.notes.iter().any(|n| n.contains("Not tried on a real GPU")));
}

#[test]
fn the_arabic_page_has_arabic_controls_and_the_plain_facade_page_has_no_recorder() {
    let scene = model3d::build(&plan().model);
    let ar = tourvideo::render_html(&scene, "x", true, stone(), &sun(), &video()).unwrap();
    assert!(ar.page.html.contains("dir=\"rtl\"") && ar.page.html.contains("تسجيل فيديو WebM"));
    let plain = nnx_core::facade_render::render_html(&scene, "x", false, stone(), &sun()).unwrap().html;
    assert!(!plain.contains("MediaRecorder") && !plain.contains("@@") && plain.contains("let FIX=null"));
}

#[test]
fn an_invalid_choice_or_a_non_facade_finish_or_an_empty_model_gives_no_page() {
    let scene = model3d::build(&plan().model);
    let bad = TourVideoInput { fps: 1, ..video() };
    assert!(tourvideo::render_html(&scene, "x", false, stone(), &sun(), &bad).unwrap_err().contains("fps"));
    let floor = finishes::find("floor", "marble").unwrap();
    assert!(tourvideo::render_html(&scene, "x", false, floor, &sun(), &video()).unwrap_err().contains("not a facade finish"));
    let night = RenderInput { solar_hour: 1.0, ..sun() };
    assert!(tourvideo::render_html(&scene, "x", false, stone(), &night, &video()).unwrap_err().contains("below the horizon"));
    let empty = model3d::build(&Document::default().model);
    assert!(tourvideo::render_html(&empty, "x", false, stone(), &sun(), &video()).is_err());
}
