//! Automatic orbit video of the building, fully offline: the facade-render page plus a camera path and a local WebM recorder.
//!
//! - The page is `facade_render`'s single self-contained HTML file (procedural PBR shaders, sun from the inputs the user enters).
//!   Two buttons are added to it: a live preview of the orbit, and a recording that renders every frame to the page's WebGL canvas
//!   and encodes it with the browser's own `MediaRecorder` + `canvas.captureStream` into a `tour-video.webm` download. No network,
//!   no FFmpeg, no library and no upload: the video never leaves the machine.
//! - The path is one constant-speed circle around the model's centre, starting at the south face (looking north) and turning
//!   `turns` full circles over `duration_s`, anticlockwise or clockwise seen from above. Every value (duration, frame rate, turns,
//!   direction, camera pitch, distance factor, resolution, bitrate) is the user's and none has a default; a missing or impossible
//!   one is refused by name. The numbers describe presentation only; they are not engineering values.
//! - Limits stated in the result: the frame rate is the target of the recorder (the browser may drop frames on a slow GPU, the
//!   clip length is wall-clock time), and the codec must be WebM VP9/VP8 as offered by the viewer; where the browser has no
//!   `MediaRecorder` the page says so instead of failing silently.

use crate::facade_render::{self, RenderInput, RenderPage};
use crate::finishes::Finish;
use crate::model3d::Scene;
use serde::Serialize;
use serde_json::json;

/// Everything the user chooses for the video. No field has a default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TourVideoInput {
    /// Length of the clip, seconds (2..=300).
    pub duration_s: f64,
    /// Target frames per second (10..=60).
    pub fps: u32,
    /// Full circles around the building during the clip (0.25..=4).
    pub turns: f64,
    /// `true` turns clockwise seen from above, `false` anticlockwise.
    pub clockwise: bool,
    /// Camera elevation angle above the horizon, degrees (0..=80).
    pub pitch_deg: f64,
    /// Orbit radius as a multiple of the distance that frames the whole building (0.5..=4).
    pub distance_factor: f64,
    /// Output size in pixels (width 320..=3840, height 240..=2160, both even).
    pub width: u32,
    pub height: u32,
    /// Encoder bitrate, kilobits per second (500..=50 000).
    pub bitrate_kbps: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct TourVideoPage {
    #[serde(flatten)]
    pub page: RenderPage,
    /// The name the recorded file is saved under by the page.
    pub video_name: &'static str,
    pub frame_count: u32,
    pub degrees_per_second: f64,
    pub total_degrees: f64,
}

/// Where the orbit starts: the camera south of the model (plan -y) looking north.
pub const START_YAW_RAD: f64 = -std::f64::consts::FRAC_PI_2;

/// Camera azimuth (radians, the page's `st.yaw`) `t` seconds into the clip. Anticlockwise seen from above increases it.
pub fn yaw_at(input: &TourVideoInput, t: f64) -> f64 {
    let f = (t / input.duration_s).clamp(0.0, 1.0);
    let sign = if input.clockwise { -1.0 } else { 1.0 };
    START_YAW_RAD + sign * input.turns * std::f64::consts::TAU * f
}

fn finite_in(name: &str, v: f64, lo: f64, hi: f64, unit: &str) -> Result<(), String> {
    if !v.is_finite() || v < lo || v > hi {
        return Err(format!("{name} must be between {lo} and {hi} {unit} (got {v})"));
    }
    Ok(())
}

/// Refuses an impossible choice by name.
pub fn validate(input: &TourVideoInput) -> Result<(), String> {
    finite_in("duration_s", input.duration_s, 2.0, 300.0, "seconds")?;
    if !(10..=60).contains(&input.fps) {
        return Err(format!("fps must be between 10 and 60 (got {})", input.fps));
    }
    finite_in("turns", input.turns, 0.25, 4.0, "turns")?;
    finite_in("pitch_deg", input.pitch_deg, 0.0, 80.0, "degrees")?;
    finite_in("distance_factor", input.distance_factor, 0.5, 4.0, "x the framing distance")?;
    if !(320..=3840).contains(&input.width) || !(240..=2160).contains(&input.height) {
        return Err(format!("the size must be 320..3840 x 240..2160 pixels (got {} x {})", input.width, input.height));
    }
    if !input.width.is_multiple_of(2) || !input.height.is_multiple_of(2) {
        return Err(format!("width and height must be even for video encoders (got {} x {})", input.width, input.height));
    }
    if !(500..=50_000).contains(&input.bitrate_kbps) {
        return Err(format!("bitrate_kbps must be between 500 and 50000 (got {})", input.bitrate_kbps));
    }
    Ok(())
}

/// The standalone page that previews and records the orbit. `facade` must be a facade finish of the catalog; the sun is the user's.
pub fn render_html(scene: &Scene, title: &str, arabic: bool, facade: &Finish, sun: &RenderInput, input: &TourVideoInput) -> Result<TourVideoPage, String> {
    validate(input)?;
    let cfg = json!({
        "duration": input.duration_s, "fps": input.fps, "turns": input.turns, "clockwise": input.clockwise, "pitch": input.pitch_deg,
        "dist_factor": input.distance_factor, "width": input.width, "height": input.height, "kbps": input.bitrate_kbps, "yaw0": START_YAW_RAD,
    });
    let extra = TOUR_JS.replace("@@TOUR@@", &cfg.to_string());
    let mut page = facade_render::render_html_with(scene, title, arabic, facade, sun, &extra)?;
    page.name = "tour-video.html".into();
    page.notes.push("Video: recorded in the viewer with the browser's own MediaRecorder as WebM (VP9 or VP8); nothing is uploaded and no external encoder is used.".into());
    page.notes.push("The frame rate is the recorder's target: a slow GPU may drop frames, and the clip length is wall-clock time. The orbit is one constant-speed circle round the model's centre.".into());
    page.notes.push("Not tried on a real GPU or in WebView2 here: the page was only checked as text and in a headless software renderer.".into());
    let frame_count = (input.duration_s * f64::from(input.fps)).round() as u32;
    let total = input.turns * 360.0;
    Ok(TourVideoPage { page, video_name: "tour-video.webm", frame_count, degrees_per_second: total / input.duration_s, total_degrees: total })
}

const TOUR_JS: &str = r##"{
const A=D.ar,S=A?{prev:"معاينة الجولة",rec:"تسجيل فيديو WebM",stop:"إيقاف",busy:"جارٍ التسجيل: ",prevbusy:"معاينة: ",s:" ث",done:"اكتمل التسجيل وحُفظ الملف tour-video.webm",nomr:"هذا المتصفح لا يدعم تسجيل الفيديو (MediaRecorder مع captureStream بترميز WebM)، فاستعمل المعاينة أو متصفحاً آخر.",cut:"أُوقف التسجيل قبل اكتماله فلم يُحفظ ملف."}:{prev:"Preview tour",rec:"Record WebM video",stop:"Stop",busy:"Recording: ",prevbusy:"Previewing: ",s:" s",done:"Recording finished and saved as tour-video.webm",nomr:"This browser cannot record video (MediaRecorder with captureStream and WebM), so use the preview or another browser.",cut:"Recording was stopped before it finished, so no file was saved."};
const C=@@TOUR@@,bar=$("bar"),bp=document.createElement("button"),br=document.createElement("button"),bs=document.createElement("span");
bp.textContent=S.prev;br.textContent=S.rec;bar.appendChild(bp);bar.appendChild(br);bar.appendChild(bs);
const base={yaw:st.yaw,pitch:st.pitch,dist:st.dist,tg:st.tg.slice()},sgn=C.clockwise?-1:1;
let run=null;
function pose(t){const f=Math.max(0,Math.min(1,t/C.duration));st.yaw=C.yaw0+sgn*C.turns*2*Math.PI*f;st.pitch=C.pitch*Math.PI/180;st.dist=(ext*1.15+4)*C.dist_factor;st.tg=[ctr[0],ctr[1],ctr[2]];$("view").value="orbit"}
function mime(){if(!window.MediaRecorder||!cv.captureStream)return null;return["video/webm;codecs=vp9","video/webm;codecs=vp8","video/webm"].find(m=>MediaRecorder.isTypeSupported(m))||null}
function stop(ok){if(!run)return;const r=run;run=null;FIX=null;Object.assign(st,{yaw:base.yaw,pitch:base.pitch,dist:base.dist,tg:base.tg.slice()});bp.textContent=S.prev;br.textContent=S.rec;bp.disabled=br.disabled=false;
 if(r.mr){r.mr.__ok=ok;if(r.mr.state!=="inactive")r.mr.stop()}else bs.textContent="";draw()}
function go(rec){if(run)return;let mr=null,chunks=[];
 if(rec){const mt=mime();if(!mt){bs.textContent=S.nomr;return}FIX=[C.width,C.height];pose(0);draw();
  mr=new MediaRecorder(cv.captureStream(C.fps),{mimeType:mt,videoBitsPerSecond:C.kbps*1000});
  mr.ondataavailable=e=>{if(e.data&&e.data.size)chunks.push(e.data)};
  mr.onstop=()=>{if(mr.__ok){const a=document.createElement("a");a.download="tour-video.webm";a.href=URL.createObjectURL(new Blob(chunks,{type:"video/webm"}));a.click();setTimeout(()=>URL.revokeObjectURL(a.href),60000);bs.textContent=S.done}else bs.textContent=S.cut};
  mr.start()}
 run={mr,rec};const t0=performance.now();
 (rec?br:bp).textContent=S.stop;(rec?bp:br).disabled=true;
 const tick=now=>{if(!run||run.mr!==mr)return;const t=(now-t0)/1000;bs.textContent=(rec?S.busy:S.prevbusy)+Math.min(C.duration,t).toFixed(1)+" / "+C.duration+S.s;
  pose(Math.min(t,C.duration));draw();
  if(t>=C.duration)stop(true);else requestAnimationFrame(tick)};
 requestAnimationFrame(tick)}
bp.onclick=()=>run?stop(false):go(false);br.onclick=()=>run?stop(false):go(true);
}"##;
