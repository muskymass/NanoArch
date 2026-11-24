//! Augmented-reality preview on the photograph of the real plot, fully offline (tracker item 82).
//!
//! The output is ONE self-contained HTML page: the user's photo (embedded as a data URI), the model's solids, and a small pinhole-camera
//! overlay that draws the building over the photo. A second button turns the same overlay into a live view over the device camera
//! (`getUserMedia` + the device orientation sensors) when the page is opened on a phone. No network, no library, no tile or map service.
//!
//! Nothing about the camera is guessed. Every value is the user's and a missing one is refused by name:
//! - where the camera stood in plan metres (`camera_x_m`, `camera_y_m`) and its height `camera_z_m` above the model's z = 0 (the finished
//!   floor level of the ground storey; the model's own datum);
//! - `heading_deg`, the compass-style bearing of the view direction measured clockwise from the plan's +y axis (0 looks up the plan);
//! - `pitch_deg`, positive looking up, and `h_fov_deg`, the horizontal field of view of the photo (from the camera or lens data);
//! - optionally `marks`: points whose plan position and pixel position in the photo the user knows (a plot corner, a pole). The report
//!   gives the reprojection error of each in pixels so the user can see whether the alignment is good before showing it to anyone.
//!
//! Limits, stated: the horizon is level (no roll); lens distortion is not modelled; the photo's field of view must be known; the overlay is
//! a visual aid for presentation and is not a survey or a measurement. The photo is a JPEG or PNG (the type is read from the file's own
//! bytes, not from a name), its EXIF rotation is honoured, and it is capped in size.

use crate::model3d::Scene;
use crate::xlsx::base64_encode;
use serde::Serialize;
use serde_json::json;

/// Largest photo embedded, in bytes.
pub const MAX_PHOTO_BYTES: usize = 25 * 1024 * 1024;
/// The camera never sees closer than this (metres): points nearer the camera plane are clipped.
pub const NEAR_M: f64 = 0.05;

/// The kinds drawn over a photo: what a visitor would see of the finished building.
const KINDS: [&str; 8] = ["Wall", "Door", "Window", "Slab", "Facade", "Site", "Vertical", "Space"];

/// A point of the photo the user can locate both in the plan and in the picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mark {
    /// Plan position in metres and height above the model datum.
    pub p: [f64; 3],
    /// Pixel position in the photo (from the top-left, in the photo as displayed).
    pub u: f64,
    pub v: f64,
}

/// Every field is the user's; none has a default.
#[derive(Debug, Clone)]
pub struct ArInput {
    pub camera_x_m: f64,
    pub camera_y_m: f64,
    pub camera_z_m: f64,
    pub heading_deg: f64,
    pub pitch_deg: f64,
    pub h_fov_deg: f64,
    pub marks: Vec<Mark>,
}

/// The photo as the page will show it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PhotoInfo {
    pub mime: &'static str,
    /// Pixels as displayed (EXIF rotation applied).
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
    /// EXIF orientation 1..=8 (1 when the file has none).
    pub exif_orientation: u8,
}

#[derive(Debug, Clone, Serialize)]
pub struct MarkError {
    pub index: usize,
    /// Where the camera puts the mark's plan point, in pixels; `None` when it is behind the camera.
    pub projected: Option<[f64; 2]>,
    pub error_px: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArPage {
    pub html: String,
    pub name: String,
    pub photo: PhotoInfo,
    /// Row of the horizon in the photo (pixels from the top; may lie outside the picture when the camera is pitched far).
    pub horizon_row_px: f64,
    pub boxes: usize,
    /// Boxes with at least one corner in front of the camera.
    pub boxes_in_front: usize,
    /// Boxes whose projection overlaps the picture.
    pub boxes_in_frame: usize,
    pub marks: Vec<MarkError>,
    pub notes: Vec<String>,
}

/// Reads the type and displayed size of a JPEG or PNG from its own bytes. Anything else is refused.
pub fn photo_info(bytes: &[u8]) -> Result<PhotoInfo, String> {
    if bytes.is_empty() {
        return Err("the photo is empty".into());
    }
    if bytes.len() > MAX_PHOTO_BYTES {
        return Err(format!("the photo is {} bytes; the limit is {} bytes", bytes.len(), MAX_PHOTO_BYTES));
    }
    if bytes.len() >= 24 && bytes[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] && &bytes[12..16] == b"IHDR" {
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        if w == 0 || h == 0 {
            return Err("the PNG photo has a zero size".into());
        }
        return Ok(PhotoInfo { mime: "image/png", width: w, height: h, bytes: bytes.len(), exif_orientation: 1 });
    }
    if bytes.len() >= 4 && bytes[0] == 0xFF && bytes[1] == 0xD8 {
        return jpeg_info(bytes);
    }
    Err("the photo must be a JPEG or a PNG file (WebP, HEIC and other formats are not read here: convert it first)".into())
}

fn jpeg_info(b: &[u8]) -> Result<PhotoInfo, String> {
    let mut i = 2usize;
    let mut orientation = 1u8;
    let mut size: Option<(u32, u32)> = None;
    while i + 4 <= b.len() {
        if b[i] != 0xFF {
            return Err("the JPEG photo is damaged (a segment marker is missing)".into());
        }
        let marker = b[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        if marker == 0xD9 || marker == 0xDA {
            break;
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        if len < 2 || i + 2 + len > b.len() {
            return Err("the JPEG photo is damaged (a segment runs past the end of the file)".into());
        }
        let seg = &b[i + 4..i + 2 + len];
        if marker == 0xE1 && seg.len() >= 6 && &seg[..6] == b"Exif\0\0" {
            orientation = exif_orientation(&seg[6..]).unwrap_or(1);
        }
        let is_sof = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            if seg.len() < 5 {
                return Err("the JPEG photo is damaged (its frame header is truncated)".into());
            }
            let h = u16::from_be_bytes([seg[1], seg[2]]) as u32;
            let w = u16::from_be_bytes([seg[3], seg[4]]) as u32;
            size = Some((w, h));
        }
        i += 2 + len;
    }
    let (w, h) = size.ok_or("the JPEG photo has no frame header, so its size cannot be read")?;
    if w == 0 || h == 0 {
        return Err("the JPEG photo has a zero size".into());
    }
    let (w, h) = if (5..=8).contains(&orientation) { (h, w) } else { (w, h) };
    Ok(PhotoInfo { mime: "image/jpeg", width: w, height: h, bytes: b.len(), exif_orientation: orientation })
}

/// The orientation tag (0x0112) of the first IFD of a TIFF block.
fn exif_orientation(t: &[u8]) -> Option<u8> {
    if t.len() < 8 {
        return None;
    }
    let le = match &t[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16at = |o: usize| -> Option<u16> { t.get(o..o + 2).map(|s| if le { u16::from_le_bytes([s[0], s[1]]) } else { u16::from_be_bytes([s[0], s[1]]) }) };
    let u32at = |o: usize| -> Option<u32> { t.get(o..o + 4).map(|s| if le { u32::from_le_bytes([s[0], s[1], s[2], s[3]]) } else { u32::from_be_bytes([s[0], s[1], s[2], s[3]]) }) };
    let ifd = u32at(4)? as usize;
    let n = u16at(ifd)? as usize;
    for k in 0..n {
        let e = ifd + 2 + k * 12;
        if u16at(e)? == 0x0112 {
            let v = u16at(e + 8)?;
            return u8::try_from(v).ok().filter(|v| (1..=8).contains(v));
        }
    }
    None
}

/// The pinhole camera derived from the user's input and the photo size.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pos: [f64; 3],
    right: [f64; 3],
    up: [f64; 3],
    fwd: [f64; 3],
    /// Focal length in pixels (square pixels).
    focal: f64,
    w: f64,
    h: f64,
}

impl Camera {
    pub fn new(input: &ArInput, width: u32, height: u32) -> Result<Camera, String> {
        for (name, v) in [("camera_x_m", input.camera_x_m), ("camera_y_m", input.camera_y_m), ("camera_z_m", input.camera_z_m), ("heading_deg", input.heading_deg), ("pitch_deg", input.pitch_deg), ("h_fov_deg", input.h_fov_deg)] {
            if !v.is_finite() {
                return Err(format!("{name} must be a number"));
            }
        }
        if !(-100.0..=2000.0).contains(&input.camera_z_m) {
            return Err(format!("camera_z_m must be between -100 and 2000 metres above the model datum (got {})", input.camera_z_m));
        }
        if input.camera_x_m.abs() > 100_000.0 || input.camera_y_m.abs() > 100_000.0 {
            return Err("camera_x_m and camera_y_m must be within 100 km of the model origin".into());
        }
        if !(-89.0..=89.0).contains(&input.pitch_deg) {
            return Err(format!("pitch_deg must be between -89 and 89 (got {})", input.pitch_deg));
        }
        if !(5.0..=170.0).contains(&input.h_fov_deg) {
            return Err(format!("h_fov_deg must be between 5 and 170 degrees (got {})", input.h_fov_deg));
        }
        let th = input.heading_deg.to_radians();
        let ph = input.pitch_deg.to_radians();
        let fwd = [th.sin() * ph.cos(), th.cos() * ph.cos(), ph.sin()];
        let right = [th.cos(), -th.sin(), 0.0];
        let up = [-th.sin() * ph.sin(), -th.cos() * ph.sin(), ph.cos()];
        let (w, h) = (f64::from(width), f64::from(height));
        let focal = (w / 2.0) / (input.h_fov_deg.to_radians() / 2.0).tan();
        Ok(Camera { pos: [input.camera_x_m, input.camera_y_m, input.camera_z_m], right, up, fwd, focal, w, h })
    }

    /// Camera-space coordinates (right, up, forward) of a plan point.
    fn camera_space(&self, p: [f64; 3]) -> [f64; 3] {
        let d = [p[0] - self.pos[0], p[1] - self.pos[1], p[2] - self.pos[2]];
        let dot = |a: [f64; 3]| a[0] * d[0] + a[1] * d[1] + a[2] * d[2];
        [dot(self.right), dot(self.up), dot(self.fwd)]
    }

    /// The pixel a plan point lands on, or `None` when it is at or behind the near plane.
    pub fn project(&self, p: [f64; 3]) -> Option<[f64; 2]> {
        let c = self.camera_space(p);
        if c[2] < NEAR_M {
            return None;
        }
        Some([self.w / 2.0 + self.focal * c[0] / c[2], self.h / 2.0 - self.focal * c[1] / c[2]])
    }

    /// Row of the horizon (pixels from the top).
    pub fn horizon_row(&self) -> f64 {
        // Looking up puts the horizon below the picture centre: its row is h/2 + f * tan(pitch).
        let tan_pitch = self.fwd[2] / (1.0 - self.fwd[2] * self.fwd[2]).sqrt();
        self.h / 2.0 + self.focal * tan_pitch
    }
}

fn corners(min: [f64; 3], max: [f64; 3]) -> [[f64; 3]; 8] {
    let mut c = [[0.0; 3]; 8];
    for (i, v) in c.iter_mut().enumerate() {
        *v = [if i & 1 == 0 { min[0] } else { max[0] }, if i & 2 == 0 { min[1] } else { max[1] }, if i & 4 == 0 { min[2] } else { max[2] }];
    }
    c
}

/// Checks the camera and the photo and builds the page. `scene` is the model's own 3D solids in metres.
pub fn render_html(scene: &Scene, title: &str, arabic: bool, photo: &[u8], input: &ArInput) -> Result<ArPage, String> {
    let info = photo_info(photo)?;
    let cam = Camera::new(input, info.width, info.height)?;
    let r3 = |v: f64| (v * 1000.0).round() / 1000.0;
    let sel: Vec<&crate::model3d::Box3> = scene.boxes.iter().filter(|b| KINDS.contains(&b.kind)).collect();
    if sel.is_empty() {
        return Err("nothing to overlay: the 3D model has no walls, slabs or other solids".into());
    }
    let (mut in_front, mut in_frame) = (0usize, 0usize);
    for b in &sel {
        let pts: Vec<[f64; 2]> = corners(b.min, b.max).iter().filter_map(|&c| cam.project(c)).collect();
        if pts.is_empty() {
            continue;
        }
        in_front += 1;
        let (x0, x1) = pts.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p[0]), a.1.max(p[0])));
        let (y0, y1) = pts.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p[1]), a.1.max(p[1])));
        if x1 >= 0.0 && x0 <= cam.w && y1 >= 0.0 && y0 <= cam.h {
            in_frame += 1;
        }
    }
    if in_front == 0 {
        return Err("the whole building is behind the camera: check camera_x_m, camera_y_m and heading_deg".into());
    }
    let mut marks = Vec::new();
    for (index, m) in input.marks.iter().enumerate() {
        if !(m.u.is_finite() && m.v.is_finite() && m.p.iter().all(|v| v.is_finite())) {
            return Err(format!("mark {} has a value that is not a number", index + 1));
        }
        if m.u < 0.0 || m.v < 0.0 || m.u > cam.w || m.v > cam.h {
            return Err(format!("mark {} lies outside the photo ({} x {} px)", index + 1, info.width, info.height));
        }
        let projected = cam.project(m.p);
        let error_px = projected.map(|p| r3(((p[0] - m.u).powi(2) + (p[1] - m.v).powi(2)).sqrt()));
        marks.push(MarkError { index: index + 1, projected: projected.map(|p| [r3(p[0]), r3(p[1])]), error_px });
    }
    let boxes: Vec<serde_json::Value> = sel.iter().map(|b| json!({ "k": b.kind, "c": b.class, "mn": b.min.map(r3), "mx": b.max.map(r3) })).collect();
    let data = json!({
        "title": title, "ar": arabic, "boxes": boxes,
        "cam": { "x": input.camera_x_m, "y": input.camera_y_m, "z": input.camera_z_m, "heading": input.heading_deg, "pitch": input.pitch_deg, "fov": input.h_fov_deg },
        "photo": { "w": info.width, "h": info.height },
        "marks": input.marks.iter().map(|m| json!({ "p": m.p, "u": m.u, "v": m.v })).collect::<Vec<_>>(),
    });
    let text = data.to_string().replace("</", "<\\/");
    let uri = format!("data:{};base64,{}", info.mime, base64_encode(photo));
    let html = TEMPLATE
        .replace("@@TITLE@@", &crate::tour::esc(title))
        .replace("@@DIR@@", if arabic { "rtl" } else { "ltr" })
        .replace("@@LANG@@", if arabic { "ar" } else { "en" })
        .replace("@@DATA@@", &text)
        .replace("@@PHOTO@@", &uri);
    let mut notes = vec![
        "The overlay is a pinhole model: the horizon is level (no roll) and lens distortion is not corrected, so a wide-angle or phone photo may drift toward its edges.".to_string(),
        "Alignment rests on the camera position, heading, pitch and field of view you entered; the page lets you nudge them, and the marks report how far the overlay lands from points you located in the photo.".to_string(),
        "Faces are painted back to front per solid, without shadows or occlusion by trees, neighbours or terrain in the photo.".to_string(),
        "Live camera mode needs a browser that allows camera and motion sensors for the page (usually https or localhost) and a compass; phone compasses drift, so confirm the heading against a known landmark. It was not run on a device here.".to_string(),
        "A visual aid for presentation; it is not a survey or a measurement and does not replace the drawings or the approving authority's review.".to_string(),
    ];
    if info.exif_orientation != 1 {
        notes.push(format!("The photo carries EXIF orientation {}; its displayed size ({} x {} px) is used, so pixel positions of marks are in the rotated picture.", info.exif_orientation, info.width, info.height));
    }
    if in_frame == 0 {
        notes.push("No solid falls inside the frame with these camera values: the building is in front of the camera but outside the photo.".to_string());
    }
    if input.marks.is_empty() {
        notes.push("No reference marks were given, so the alignment error is not measured.".to_string());
    }
    Ok(ArPage { html, name: "ar-preview.html".into(), horizon_row_px: (cam.horizon_row() * 1000.0).round() / 1000.0, photo: info, boxes: sel.len(), boxes_in_front: in_front, boxes_in_frame: in_frame, marks, notes })
}

const TEMPLATE: &str = r##"<!doctype html>
<html lang="@@LANG@@" dir="@@DIR@@"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>@@TITLE@@</title>
<style>
html,body{margin:0;height:100%;background:#14171c;color:#e8ebf0;font-family:'Segoe UI',Tahoma,Arial,sans-serif}
#bar{display:flex;flex-wrap:wrap;gap:8px 14px;align-items:center;padding:6px 10px;background:#1d222a;border-bottom:1px solid #333;font-size:13px}
#bar label{display:flex;align-items:center;gap:4px}#bar input[type=range]{width:120px}#bar b{font-size:15px}#bar button{font:inherit;padding:3px 10px}
#stage{position:relative;width:100%;height:calc(100% - 92px);display:flex;align-items:center;justify-content:center;overflow:hidden}
canvas{max-width:100%;max-height:100%;background:#000}video{display:none}
#info{padding:4px 10px;font-size:12px;color:#aab2bf;min-height:18px}
</style></head><body>
<div id="bar"><b id="ttl"></b>
<label><span id="l-h"></span><input id="s-h" type="range" min="-180" max="180" step="0.1"><output id="o-h"></output></label>
<label><span id="l-p"></span><input id="s-p" type="range" min="-60" max="60" step="0.1"><output id="o-p"></output></label>
<label><span id="l-f"></span><input id="s-f" type="range" min="20" max="140" step="0.1"><output id="o-f"></output></label>
<label><input type="checkbox" id="faces" checked><span id="l-faces"></span></label>
<label><input type="checkbox" id="edges" checked><span id="l-edges"></span></label>
<label><span id="l-a"></span><input id="s-a" type="range" min="0.1" max="1" step="0.05" value="0.55"></label>
<button id="reset"></button><button id="live"></button><button id="png"></button></div>
<div id="stage"><canvas id="cv"></canvas><video id="vid" playsinline muted></video></div>
<div id="info"></div>
<script>
const D=@@DATA@@;
const PHOTO="@@PHOTO@@";
(function(){
const T=D.ar?{h:"الاتجاه",p:"الميل",f:"زاوية الرؤية",faces:"أوجه",edges:"حواف",a:"الشفافية",reset:"إعادة القيم",live:"الكاميرا الحية",png:"لقطة PNG",stop:"إيقاف الكاميرا",
 note:"معاينة تقريبية على الصورة: النموذج المعروض مجسم هندسي لا قياس مساحي.",err:"تعذّر تشغيل الكاميرا أو المستشعرات: ",marks:"نقاط المرجع (خطأ بالبكسل): "}
:{h:"Heading",p:"Pitch",f:"Field of view",faces:"Faces",edges:"Edges",a:"Opacity",reset:"Reset",live:"Live camera",png:"PNG snapshot",stop:"Stop camera",
 note:"Approximate overlay on the photo: the model is drawn as solids, not a survey.",err:"Camera or sensors unavailable: ",marks:"Reference marks (error px): "};
const $=id=>document.getElementById(id);
$("ttl").textContent=D.title;$("l-h").textContent=T.h;$("l-p").textContent=T.p;$("l-f").textContent=T.f;$("l-faces").textContent=T.faces;$("l-edges").textContent=T.edges;$("l-a").textContent=T.a;
$("reset").textContent=T.reset;$("live").textContent=T.live;$("png").textContent=T.png;
const cv=$("cv"),g=cv.getContext("2d"),vid=$("vid");
const C0={heading:D.cam.heading,pitch:D.cam.pitch,fov:D.cam.fov};
const cam={x:D.cam.x,y:D.cam.y,z:D.cam.z,heading:C0.heading,pitch:C0.pitch,fov:C0.fov};
const img=new Image();let live=false,stream=null;
const COL={Wall:[200,196,188],Door:[120,84,52],Window:[90,150,200],Slab:[150,150,150],Facade:[196,182,150],Site:[110,150,100],Vertical:[170,170,170],Space:[220,220,120]};
function frame(){
  const W=live?(vid.videoWidth||D.photo.w):D.photo.w,H=live?(vid.videoHeight||D.photo.h):D.photo.h;
  if(cv.width!==W||cv.height!==H){cv.width=W;cv.height=H}
  g.clearRect(0,0,W,H);
  if(live&&vid.readyState>=2)g.drawImage(vid,0,0,W,H);else if(img.complete&&img.naturalWidth)g.drawImage(img,0,0,W,H);
  const th=cam.heading*Math.PI/180,ph=cam.pitch*Math.PI/180;
  const fw=[Math.sin(th)*Math.cos(ph),Math.cos(th)*Math.cos(ph),Math.sin(ph)],rt=[Math.cos(th),-Math.sin(th),0],up=[-Math.sin(th)*Math.sin(ph),-Math.cos(th)*Math.sin(ph),Math.cos(ph)];
  const foc=(W/2)/Math.tan(cam.fov*Math.PI/360),NEAR=0.05;
  const cc=p=>{const d=[p[0]-cam.x,p[1]-cam.y,p[2]-cam.z];return[rt[0]*d[0]+rt[1]*d[1]+rt[2]*d[2],up[0]*d[0]+up[1]*d[1]+up[2]*d[2],fw[0]*d[0]+fw[1]*d[1]+fw[2]*d[2]]};
  const px=c=>[W/2+foc*c[0]/c[2],H/2-foc*c[1]/c[2]];
  // Clips a camera-space segment to the near plane; returns null when it is wholly behind.
  const clip=(a,b)=>{if(a[2]<NEAR&&b[2]<NEAR)return null;if(a[2]<NEAR){const t=(NEAR-a[2])/(b[2]-a[2]);a=[a[0]+t*(b[0]-a[0]),a[1]+t*(b[1]-a[1]),NEAR]}else if(b[2]<NEAR){const t=(NEAR-b[2])/(a[2]-b[2]);b=[b[0]+t*(a[0]-b[0]),b[1]+t*(a[1]-b[1]),NEAR]}return[a,b]};
  const lw=Math.max(1,W/900),alpha=parseFloat($("s-a").value),showF=$("faces").checked,showE=$("edges").checked;
  const items=[];
  for(const b of D.boxes){const n=b.mn,x=b.mx;const ctr=[(n[0]+x[0])/2,(n[1]+x[1])/2,(n[2]+x[2])/2];const cz=cc(ctr)[2];items.push({b,d:cz})}
  items.sort((p,q)=>q.d-p.d);
  const FACES=[[0,1,3,2],[4,5,7,6],[0,1,5,4],[2,3,7,6],[0,2,6,4],[1,3,7,5]],EDGES=[[0,1],[2,3],[4,5],[6,7],[0,2],[1,3],[4,6],[5,7],[0,4],[1,5],[2,6],[3,7]];
  for(const it of items){
    const b=it.b,n=b.mn,x=b.mx,col=COL[b.k]||[180,180,180];
    const pts=[];for(let i=0;i<8;i++)pts.push([i&1?x[0]:n[0],i&2?x[1]:n[1],i&4?x[2]:n[2]]);
    const cs=pts.map(cc);
    if(showF){
      for(const f of FACES){
        if(f.some(i=>cs[i][2]<NEAR))continue;
        const a=cs[f[0]],bb=cs[f[1]],c=cs[f[2]];
        const e1=[bb[0]-a[0],bb[1]-a[1],bb[2]-a[2]],e2=[c[0]-a[0],c[1]-a[1],c[2]-a[2]];
        const nz=[e1[1]*e2[2]-e1[2]*e2[1],e1[2]*e2[0]-e1[0]*e2[2],e1[0]*e2[1]-e1[1]*e2[0]];
        // Orient the normal away from the box centre, then draw the face only when it is turned toward the eye (the camera origin).
        const mid=[(cs[f[0]][0]+cs[f[2]][0])/2,(cs[f[0]][1]+cs[f[2]][1])/2,(cs[f[0]][2]+cs[f[2]][2])/2];
        const bc=cc([(n[0]+x[0])/2,(n[1]+x[1])/2,(n[2]+x[2])/2]);
        const sgn=(nz[0]*(mid[0]-bc[0])+nz[1]*(mid[1]-bc[1])+nz[2]*(mid[2]-bc[2]))>=0?1:-1;
        const vis=sgn*(nz[0]*mid[0]+nz[1]*mid[1]+nz[2]*mid[2])<0;
        if(!vis)continue;
        const sh=0.6+0.4*Math.abs(nz[1])/Math.hypot(nz[0],nz[1],nz[2]);
        g.fillStyle="rgba("+Math.round(col[0]*sh)+","+Math.round(col[1]*sh)+","+Math.round(col[2]*sh)+","+(b.k==="Window"?alpha*0.6:alpha)+")";
        g.beginPath();f.forEach((i,k)=>{const p=px(cs[i]);k?g.lineTo(p[0],p[1]):g.moveTo(p[0],p[1])});g.closePath();g.fill();
      }
    }
    if(showE){
      g.strokeStyle="rgba(255,255,255,0.85)";g.lineWidth=lw;g.beginPath();
      for(const [i,j] of EDGES){const s=clip(cs[i],cs[j]);if(!s)continue;const a=px(s[0]),c=px(s[1]);g.moveTo(a[0],a[1]);g.lineTo(c[0],c[1])}
      g.stroke();
    }
  }
  // Reference marks: the photo position the user located (circle) and where the plan point lands (cross).
  let msg=T.note;
  if(D.marks.length&&!live){
    const errs=[];
    D.marks.forEach((m,i)=>{
      g.strokeStyle="#ffd54a";g.lineWidth=lw*1.5;g.beginPath();g.arc(m.u,m.v,lw*7,0,6.2832);g.stroke();
      const c=cc(m.p);if(c[2]<NEAR){errs.push((i+1)+": -");return}
      const p=px(c);g.strokeStyle="#ff5252";g.beginPath();g.moveTo(p[0]-lw*8,p[1]);g.lineTo(p[0]+lw*8,p[1]);g.moveTo(p[0],p[1]-lw*8);g.lineTo(p[0],p[1]+lw*8);g.stroke();
      errs.push((i+1)+": "+Math.hypot(p[0]-m.u,p[1]-m.v).toFixed(1));
    });
    msg+="  "+T.marks+errs.join("  ");
  }
  $("info").textContent=msg;
  $("o-h").textContent=cam.heading.toFixed(1)+"°";$("o-p").textContent=cam.pitch.toFixed(1)+"°";$("o-f").textContent=cam.fov.toFixed(1)+"°";
  document.body.dataset.drawn=String(items.length);
}
function sync(){$("s-h").value=cam.heading;$("s-p").value=cam.pitch;$("s-f").value=cam.fov}
function loop(){frame();if(live)requestAnimationFrame(loop)}
["s-h","s-p","s-f"].forEach(id=>$(id).addEventListener("input",()=>{cam.heading=parseFloat($("s-h").value);cam.pitch=parseFloat($("s-p").value);cam.fov=parseFloat($("s-f").value);if(!live)frame()}));
["faces","edges","s-a"].forEach(id=>$(id).addEventListener("input",()=>{if(!live)frame()}));
$("reset").onclick=()=>{cam.heading=C0.heading;cam.pitch=C0.pitch;cam.fov=C0.fov;sync();if(!live)frame()};
$("png").onclick=()=>{frame();cv.toBlob(b=>{const a=document.createElement("a");a.href=URL.createObjectURL(b);a.download="ar-preview.png";a.click();setTimeout(()=>URL.revokeObjectURL(a.href),2000)},"image/png")};
function onOrient(e){
  // iOS gives a true compass heading; elsewhere alpha is measured from an arbitrary start, so it is only a relative turn.
  let hd=(typeof e.webkitCompassHeading==="number")?e.webkitCompassHeading:(e.alpha==null?null:(360-e.alpha)%360);
  if(hd!=null)cam.heading=((hd+540)%360)-180;
  if(e.beta!=null)cam.pitch=Math.max(-60,Math.min(60,e.beta-90));
  sync();
}
function stopLive(){live=false;if(stream)stream.getTracks().forEach(t=>t.stop());stream=null;window.removeEventListener("deviceorientation",onOrient,true);$("live").textContent=T.live;frame()}
$("live").onclick=async()=>{
  if(live){stopLive();return}
  try{
    if(typeof DeviceOrientationEvent!=="undefined"&&typeof DeviceOrientationEvent.requestPermission==="function"){const r=await DeviceOrientationEvent.requestPermission();if(r!=="granted")throw new Error("orientation permission denied")}
    stream=await navigator.mediaDevices.getUserMedia({video:{facingMode:"environment"},audio:false});
    vid.srcObject=stream;await vid.play();live=true;$("live").textContent=T.stop;
    window.addEventListener("deviceorientation",onOrient,true);loop();
  }catch(e){stopLive();$("info").textContent=T.err+(e&&e.message?e.message:e)}
};
img.onload=()=>{sync();frame()};img.onerror=()=>{$("info").textContent="photo failed to load"};img.src=PHOTO;
})();
</script></body></html>
"##;
