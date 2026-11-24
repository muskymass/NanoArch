use nnx_core::arpreview::{self, ArInput, Camera, Mark};
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

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
    b.extend(w.to_be_bytes());
    b.extend(h.to_be_bytes());
    b.extend([8, 2, 0, 0, 0, 0, 0, 0, 0]);
    b
}

/// A JPEG skeleton: SOI, an optional EXIF block with the orientation tag, a frame header, end of image.
fn jpeg(w: u16, h: u16, orientation: Option<u16>) -> Vec<u8> {
    let mut b = vec![0xFF, 0xD8];
    if let Some(o) = orientation {
        let mut tiff = b"MM".to_vec();
        tiff.extend([0, 42, 0, 0, 0, 8, 0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1]);
        tiff.extend(o.to_be_bytes());
        tiff.extend([0, 0, 0, 0, 0, 0]);
        let mut seg = b"Exif\0\0".to_vec();
        seg.extend(tiff);
        b.extend([0xFF, 0xE1]);
        b.extend(((seg.len() + 2) as u16).to_be_bytes());
        b.extend(seg);
    }
    b.extend([0xFF, 0xC0, 0, 11, 8]);
    b.extend(h.to_be_bytes());
    b.extend(w.to_be_bytes());
    b.extend([1, 1, 0x11, 0]);
    b.extend([0xFF, 0xD9]);
    b
}

fn cam_input() -> ArInput {
    // The camera stands 20 m south of the building's south face at eye height, looking up the plan (north), level, 90 degrees wide.
    ArInput { camera_x_m: 3.0, camera_y_m: -20.0, camera_z_m: 1.5, heading_deg: 0.0, pitch_deg: 0.0, h_fov_deg: 90.0, marks: vec![] }
}

fn origin_cam() -> ArInput {
    ArInput { camera_x_m: 0.0, camera_y_m: 0.0, camera_z_m: 1.5, heading_deg: 0.0, pitch_deg: 0.0, h_fov_deg: 90.0, marks: vec![] }
}

#[test]
fn the_photo_type_and_displayed_size_come_from_its_own_bytes() {
    let p = arpreview::photo_info(&png(1000, 500)).unwrap();
    assert_eq!((p.mime, p.width, p.height, p.exif_orientation), ("image/png", 1000, 500, 1));
    let j = arpreview::photo_info(&jpeg(4000, 3000, None)).unwrap();
    assert_eq!((j.mime, j.width, j.height), ("image/jpeg", 4000, 3000));
    // EXIF orientation 6 (rotate 90) is shown by browsers as portrait: the displayed size swaps.
    let r = arpreview::photo_info(&jpeg(4000, 3000, Some(6))).unwrap();
    assert_eq!((r.width, r.height, r.exif_orientation), (3000, 4000, 6));
    let flat = arpreview::photo_info(&jpeg(4000, 3000, Some(3))).unwrap();
    assert_eq!((flat.width, flat.height), (4000, 3000));
    for (bad, msg) in [(vec![], "empty"), (b"GIF89a....................".to_vec(), "JPEG or a PNG"), (vec![0xFF, 0xD8, 0xFF, 0xD9], "no frame header"), (png(0, 10), "zero size")] {
        assert!(arpreview::photo_info(&bad).unwrap_err().contains(msg), "{msg}");
    }
    assert!(arpreview::photo_info(&vec![0xFFu8; arpreview::MAX_PHOTO_BYTES + 1]).unwrap_err().contains("limit"));
}

#[test]
fn the_pinhole_projection_matches_hand_computed_pixels() {
    // 1000 x 500 photo, 90 degrees wide: focal length 500 px.
    let c = Camera::new(&origin_cam(), 1000, 500).unwrap();
    let near = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9;
    assert!(near(c.project([0.0, 10.0, 1.5]).unwrap(), [500.0, 250.0]));
    // 2 m to the right at 10 m: 500 * 2 / 10 = 100 px right. 1 m higher: 50 px up.
    assert!(near(c.project([2.0, 10.0, 1.5]).unwrap(), [600.0, 250.0]));
    assert!(near(c.project([0.0, 10.0, 2.5]).unwrap(), [500.0, 200.0]));
    assert!(c.project([0.0, -5.0, 1.5]).is_none(), "behind the camera");
    assert!(c.project([0.0, 0.01, 1.5]).is_none(), "closer than the near plane");
    // Turning to look east (heading 90) puts a point due east on the centre; a point north of it is then to the left.
    let east = Camera::new(&ArInput { heading_deg: 90.0, ..origin_cam() }, 1000, 500).unwrap();
    assert!(near(east.project([10.0, 0.0, 1.5]).unwrap(), [500.0, 250.0]));
    assert!(near(east.project([10.0, 2.0, 1.5]).unwrap(), [400.0, 250.0]));
    // Looking 10 degrees up moves the horizon DOWN the picture by f * tan(10).
    let up = Camera::new(&ArInput { pitch_deg: 10.0, ..origin_cam() }, 1000, 500).unwrap();
    let expected = 250.0 + 500.0 * 10f64.to_radians().tan();
    assert!((up.horizon_row() - expected).abs() < 1e-9, "{} vs {expected}", up.horizon_row());
    // A very distant point at the camera's own height lands on the horizon row.
    let far = up.project([0.0, 1.0e6, 1.5]).unwrap();
    assert!((far[1] - up.horizon_row()).abs() < 0.05, "{} vs {}", far[1], up.horizon_row());
}

#[test]
fn camera_values_that_are_not_a_real_camera_are_refused_by_name() {
    let ok = cam_input();
    assert!(Camera::new(&ok, 1000, 500).is_ok());
    for (bad, name) in [
        (ArInput { camera_x_m: f64::NAN, ..ok.clone() }, "camera_x_m"),
        (ArInput { heading_deg: f64::INFINITY, ..ok.clone() }, "heading_deg"),
        (ArInput { camera_z_m: 5000.0, ..ok.clone() }, "camera_z_m"),
        (ArInput { pitch_deg: 90.0, ..ok.clone() }, "pitch_deg"),
        (ArInput { h_fov_deg: 3.0, ..ok.clone() }, "h_fov_deg"),
        (ArInput { h_fov_deg: 175.0, ..ok.clone() }, "h_fov_deg"),
        (ArInput { camera_y_m: 1.0e9, ..ok.clone() }, "camera_y_m"),
    ] {
        assert!(Camera::new(&bad, 1000, 500).unwrap_err().contains(name), "{name}");
    }
}

#[test]
fn the_page_is_one_offline_file_with_the_photo_inside() {
    let scene = model3d::build(&plan().model);
    let photo = png(1600, 900);
    let page = arpreview::render_html(&scene, "Villa </script> test", false, &photo, &cam_input()).unwrap();
    assert_eq!(page.name, "ar-preview.html");
    assert_eq!((page.photo.width, page.photo.height), (1600, 900));
    assert!(page.html.contains("data:image/png;base64,iVBORw0KGgo"));
    assert!(page.html.contains("<canvas id=\"cv\">") && page.html.contains("getUserMedia"));
    assert!(!page.html.contains("src=\"http") && !page.html.contains("href=\"http") && !page.html.contains("fetch("));
    assert!(!page.html.contains("Villa </script>") && page.html.contains("Villa &lt;/script&gt; test"));
    assert!(page.boxes > 0 && page.boxes_in_front == page.boxes && page.boxes_in_frame > 0, "{page:?}");
    assert!((page.horizon_row_px - 450.0).abs() < 1e-9, "level camera: horizon on the centre row");
    assert!(page.notes.iter().any(|n| n.contains("No reference marks")));
    let ar = arpreview::render_html(&scene, "x", true, &photo, &cam_input()).unwrap();
    assert!(ar.html.contains("dir=\"rtl\"") && ar.html.contains("الاتجاه"));
}

#[test]
fn a_building_behind_the_camera_or_outside_the_frame_is_reported_not_hidden() {
    let scene = model3d::build(&plan().model);
    let photo = png(1600, 900);
    let behind = ArInput { heading_deg: 180.0, ..cam_input() };
    assert!(arpreview::render_html(&scene, "x", false, &photo, &behind).unwrap_err().contains("behind the camera"));
    // Looking north with a narrow view while standing 400 m east of the building: in front, but out of frame.
    let aside = ArInput { camera_x_m: 400.0, h_fov_deg: 30.0, ..cam_input() };
    let page = arpreview::render_html(&scene, "x", false, &photo, &aside).unwrap();
    assert!(page.boxes_in_front > 0 && page.boxes_in_frame == 0);
    assert!(page.notes.iter().any(|n| n.contains("outside the photo")));
}

#[test]
fn marks_report_their_reprojection_error_in_pixels() {
    let scene = model3d::build(&plan().model);
    let photo = png(1000, 500);
    let cam = cam_input();
    let c = Camera::new(&cam, 1000, 500).unwrap();
    let corner = [6.0, 0.0, 0.0];
    let [pu, pv] = c.project(corner).unwrap();
    let exact = ArInput { marks: vec![Mark { p: corner, u: pu, v: pv }], ..cam.clone() };
    let page = arpreview::render_html(&scene, "x", false, &photo, &exact).unwrap();
    assert_eq!(page.marks.len(), 1);
    assert!(page.marks[0].error_px.unwrap() < 1e-3);
    // The user located it 30 px to the right and 40 px lower: a 50 px error (3-4-5).
    let off = ArInput { marks: vec![Mark { p: corner, u: pu + 30.0, v: pv + 40.0 }], ..cam.clone() };
    let page = arpreview::render_html(&scene, "x", false, &photo, &off).unwrap();
    assert!((page.marks[0].error_px.unwrap() - 50.0).abs() < 1e-2);
    assert!(!page.notes.iter().any(|n| n.contains("No reference marks")));
    // A mark outside the photo is refused by name; one behind the camera gets no invented error.
    let outside = ArInput { marks: vec![Mark { p: corner, u: 5000.0, v: 10.0 }], ..cam.clone() };
    assert!(arpreview::render_html(&scene, "x", false, &photo, &outside).unwrap_err().contains("mark 1 lies outside"));
    let behind = ArInput { marks: vec![Mark { p: [3.0, -30.0, 1.0], u: 10.0, v: 10.0 }], ..cam };
    let page = arpreview::render_html(&scene, "x", false, &photo, &behind).unwrap();
    assert!(page.marks[0].projected.is_none() && page.marks[0].error_px.is_none());
}

#[test]
fn an_empty_model_has_nothing_to_overlay() {
    let scene = model3d::build(&Document::default().model);
    assert!(arpreview::render_html(&scene, "x", false, &png(100, 100), &cam_input()).unwrap_err().contains("nothing to overlay"));
}
