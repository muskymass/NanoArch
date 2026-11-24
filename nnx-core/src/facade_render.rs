//! Photoreal-style facade rendering, fully offline: ONE self-contained HTML page with the model's solids and a physically based
//! WebGL renderer inside. No network, no library, no texture file and no photograph: every surface is a procedural shader.
//!
//! - Light: the sun comes from `analysis::sun` at the latitude, day, solar hour and plan north offset the USER enters (none is
//!   defaulted; a sun below the horizon is refused). Direct light is Cook-Torrance (GGX distribution, Smith-Schlick geometry, Schlick
//!   Fresnel) with a PCF shadow map; the sky is an analytic gradient with a sun disc that also lights the ambient and is reflected
//!   by the glass; output is ACES tone-mapped and gamma corrected.
//! - Materials: the project's chosen facade finish (`finishes`) selects the procedural facade shader (stone courses, travertine
//!   bands, profile plaster, wood planks) tinted with the catalog colour; glass, door leaves, concrete, interior plaster and paving
//!   have fixed procedural looks that are presentation only, not material properties.
//! - Geometry: the model's own boxes (`model3d`), exterior-facing kinds only. Structure, services and furniture are not included.
//!
//! This is a visual preview of the design, not a measurement: it has no bump mapping, no global illumination and no neighbours.

use crate::analysis;
use crate::finishes::Finish;
use crate::model3d::Scene;
use serde::Serialize;
use serde_json::json;

/// The kinds a facade render draws.
const KINDS: [&str; 7] = ["Wall", "Door", "Window", "Slab", "Facade", "Site", "Vertical"];

/// The sun the render is lit by. Every field is the user's; none has a default.
#[derive(Debug, Clone, Copy)]
pub struct RenderInput {
    pub latitude_deg: f64,
    /// Day of the year, 1..=366.
    pub day: u32,
    /// Solar hour, 12.0 is solar noon.
    pub solar_hour: f64,
    /// Compass bearing of the plan's +y axis (0 when the top of the plan is true north).
    pub north_offset_deg: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SunReport {
    pub altitude_deg: f64,
    pub azimuth_deg: f64,
    /// Unit vector towards the sun in plan axes (x east of the plan, y up the plan, z up).
    pub direction: [f64; 3],
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderPage {
    pub html: String,
    pub name: String,
    pub sun: SunReport,
    pub facade: &'static str,
    pub boxes: usize,
    pub notes: Vec<String>,
}

/// The procedural facade shader a catalog finish selects; `None` for a finish this renderer has no shader for.
fn facade_type(id: &str) -> Option<u8> {
    match id {
        "riyadh_stone" => Some(0),
        "travertine" => Some(1),
        "profile_plaster" => Some(2),
        "wood_cladding" => Some(3),
        _ => None,
    }
}

/// 0 facade, 1 interior plaster, 2 glass, 3 door leaf, 4 concrete, 5 paving.
fn material_of(kind: &str, class: &str) -> u8 {
    match kind {
        "Wall" if class == "ext" => 0,
        "Wall" => 1,
        "Facade" => 0,
        "Window" => 2,
        "Door" => 3,
        "Site" => 5,
        _ => 4,
    }
}

/// The sun for `input`, refusing values that are not a real sun.
pub fn sun_for(input: &RenderInput) -> Result<SunReport, String> {
    if !input.latitude_deg.is_finite() || !(-90.0..=90.0).contains(&input.latitude_deg) {
        return Err(format!("latitude_deg must be between -90 and 90 (got {})", input.latitude_deg));
    }
    if !(1..=366).contains(&input.day) {
        return Err(format!("day must be a day of the year, 1 to 366 (got {})", input.day));
    }
    if !input.solar_hour.is_finite() || !(0.0..24.0).contains(&input.solar_hour) {
        return Err(format!("solar_hour must be in [0, 24) (got {})", input.solar_hour));
    }
    if !input.north_offset_deg.is_finite() {
        return Err("north_offset_deg must be a number".to_string());
    }
    let s = analysis::sun(input.latitude_deg, input.day, input.solar_hour).ok_or_else(|| format!("the sun is below the horizon at latitude {} on day {} at solar hour {}: choose a daytime hour", input.latitude_deg, input.day, input.solar_hour))?;
    // Same convention as `shadows::box_shadow`: the plan bearing is the compass azimuth minus the plan's north offset.
    let bearing = (s.azimuth_deg - input.north_offset_deg).to_radians();
    let alt = s.altitude_deg.to_radians();
    let r6 = |v: f64| (v * 1e6).round() / 1e6;
    Ok(SunReport { altitude_deg: s.altitude_deg, azimuth_deg: s.azimuth_deg, direction: [r6(bearing.sin() * alt.cos()), r6(bearing.cos() * alt.cos()), r6(alt.sin())] })
}

/// The standalone render page. `facade` must be a facade finish of the catalog.
pub fn render_html(scene: &Scene, title: &str, arabic: bool, facade: &Finish, input: &RenderInput) -> Result<RenderPage, String> {
    render_html_with(scene, title, arabic, facade, input, "")
}

/// The same page with `extra` script appended inside the renderer's closure (it sees `st`, `draw`, `cv`, `ctr`, `ext`, `FIX`, `$`, `D`).
/// `extra` is written by this crate (never user text).
pub(crate) fn render_html_with(scene: &Scene, title: &str, arabic: bool, facade: &Finish, input: &RenderInput, extra: &str) -> Result<RenderPage, String> {
    if facade.kind != "facade" {
        return Err(format!("'{}' is not a facade finish", facade.id));
    }
    let ftype = facade_type(facade.id).ok_or_else(|| format!("no procedural material for the facade finish '{}'", facade.id))?;
    let sun = sun_for(input)?;
    let r3 = |v: f64| (v * 1000.0).round() / 1000.0;
    let boxes: Vec<serde_json::Value> = scene
        .boxes
        .iter()
        .filter(|b| KINDS.contains(&b.kind))
        .enumerate()
        .map(|(i, b)| {
            let seed = ((i as u64).wrapping_mul(2_654_435_761) % 1000) as f64 / 1000.0;
            json!({ "m": material_of(b.kind, &b.class), "s": seed, "mn": b.min.map(r3), "mx": b.max.map(r3), "g": b.kind == "Window" })
        })
        .collect();
    if !boxes.iter().any(|b| b["m"] != 2) {
        return Err("nothing to render: the model has no walls, slabs or other solids".into());
    }
    let n = boxes.len();
    let data = json!({
        "title": title, "ar": arabic, "min": scene.min.map(r3), "max": scene.max.map(r3), "boxes": boxes,
        "sun": { "alt": sun.altitude_deg, "az": sun.azimuth_deg, "dir": sun.direction, "lat": input.latitude_deg, "day": input.day, "hour": input.solar_hour, "north": input.north_offset_deg },
        "facade": { "id": facade.id, "type": ftype, "color": facade.color, "name": if arabic { facade.ar } else { facade.en } },
    });
    let text = data.to_string().replace("</", "<\\/");
    let html = TEMPLATE.replace("@@EXTRA@@", extra).replace("@@TITLE@@", &crate::tour::esc(title)).replace("@@DIR@@", if arabic { "rtl" } else { "ltr" }).replace("@@LANG@@", if arabic { "ar" } else { "en" }).replace("@@DATA@@", &text);
    let notes = vec![
        "Procedural preview: surfaces are generated by shaders, not photographs, and have no bump or normal mapping.".to_string(),
        "Lit by the sun you entered plus an analytic sky; no global illumination, no neighbouring buildings or trees.".to_string(),
        "Geometry is the model's own solids (walls, slabs, openings, facade elements, site, verticals); structure, services and furniture are not drawn.".to_string(),
        "A visual aid for presentation; it is not a measurement and does not replace the drawings.".to_string(),
    ];
    Ok(RenderPage { html, name: "facade-render.html".into(), sun, facade: facade.id, boxes: n, notes })
}

const TEMPLATE: &str = r##"<!doctype html>
<html lang="@@LANG@@" dir="@@DIR@@"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>@@TITLE@@</title>
<style>
html,body{margin:0;height:100%;font-family:'Segoe UI',Tahoma,Arial,sans-serif;background:#10141b;color:#1b1f27}
#bar{position:fixed;top:0;inset-inline:0;display:flex;flex-wrap:wrap;gap:8px;align-items:center;padding:6px 10px;background:#ffffffe6;border-bottom:1px solid #ccd;z-index:2;font-size:13px}
#bar b{font-size:15px}#bar button,#bar select,#bar input{font:inherit;padding:3px 8px}
canvas{position:fixed;inset:0;width:100%;height:100%;display:block;touch-action:none}
#info{position:fixed;bottom:8px;inset-inline-start:10px;background:#ffffffd9;padding:6px 10px;border-radius:6px;font-size:12px;z-index:2;max-width:70vw}
#err{position:fixed;inset:40px 10px auto 10px;background:#fff3f3;color:#8a1111;padding:8px 12px;border:1px solid #d99;border-radius:6px;z-index:3;display:none;white-space:pre-wrap;font-size:12px}
</style></head><body>
<canvas id="gl"></canvas>
<div id="bar"><b id="ttl"></b><select id="view"></select><label><span id="l-ex"></span> <input id="ex" type="range" min="0.4" max="2.5" step="0.05" value="1"></label>
<label><input type="checkbox" id="sh" checked> <span id="l-sh"></span></label><button id="png"></button></div>
<div id="info"></div><div id="err"></div>
<script>
const D=@@DATA@@;
(function(){
const T=D.ar?{png:"لقطة PNG",ex:"التعريض",sh:"الظلال",orbit:"مدار حر",face:"واجهة باتجاه",sun:"الشمس",alt:"الارتفاع",az:"السمت",lat:"خط العرض",day:"اليوم",hour:"الساعة الشمسية",fac:"الكسوة",note:"معاينة إجرائية بالشيدرات وليست صورة فوتوغرافية. اسحب للدوران، العجلة للتقريب، Shift للتحريك.",noshadow:"تعذّر إنشاء خريطة الظلال في هذا المتصفح فعُطّلت الظلال.",fail:"تعذّر تشغيل المُصيِّر: "}
:{png:"PNG snapshot",ex:"Exposure",sh:"Shadows",orbit:"Free orbit",face:"Facade facing",sun:"Sun",alt:"altitude",az:"azimuth",lat:"latitude",day:"day",hour:"solar hour",fac:"Facade",note:"Procedural shader preview, not a photograph. Drag to orbit, wheel to zoom, Shift or right button to pan.",noshadow:"A shadow map could not be created in this browser, so shadows are off.",fail:"The renderer could not start: "};
const $=id=>document.getElementById(id);
const fmt=(v,d)=>(+v).toFixed(d);
$("ttl").textContent=D.title;$("png").textContent=T.png;$("l-ex").textContent=T.ex;$("l-sh").textContent=T.sh;
$("info").textContent=T.note+"  |  "+T.fac+": "+D.facade.name+"  |  "+T.sun+": "+T.alt+" "+fmt(D.sun.alt,1)+"°, "+T.az+" "+fmt(D.sun.az,1)+"° ("+T.lat+" "+D.sun.lat+"°, "+T.day+" "+D.sun.day+", "+T.hour+" "+D.sun.hour+")";
const fail=m=>{const e=$("err");e.style.display="block";e.textContent=T.fail+m};
try{
const cv=$("gl"),gl=cv.getContext("webgl",{antialias:true,preserveDrawingBuffer:true});
if(!gl)throw new Error("WebGL is not available in this browser.");
const sh=(t,s)=>{const o=gl.createShader(t);gl.shaderSource(o,s);gl.compileShader(o);if(!gl.getShaderParameter(o,gl.COMPILE_STATUS))throw new Error(gl.getShaderInfoLog(o));return o};
const prog=(v,f)=>{const p=gl.createProgram();gl.attachShader(p,sh(gl.VERTEX_SHADER,v));gl.attachShader(p,sh(gl.FRAGMENT_SHADER,f));gl.bindAttribLocation(p,0,"p");gl.bindAttribLocation(p,1,"n");gl.bindAttribLocation(p,2,"a");gl.linkProgram(p);if(!gl.getProgramParameter(p,gl.LINK_STATUS))throw new Error(gl.getProgramInfoLog(p));return p};
const loc=(p,n)=>gl.getUniformLocation(p,n);
const SKY="uniform vec3 sunDir;uniform vec3 sunCol;uniform vec3 zen;uniform vec3 hor;"+
"vec3 sky(vec3 d){float h=clamp(d.z,0.0,1.0);vec3 c=mix(hor,zen,pow(h,0.45));if(d.z<0.0)c=mix(hor,hor*0.55,clamp(-d.z*4.0,0.0,1.0));float s=max(dot(d,sunDir),0.0);c+=sunCol*(pow(s,600.0)*30.0+pow(s,10.0)*0.12);return c;}"+
"vec3 aces(vec3 x){return clamp((x*(2.51*x+0.03))/(x*(2.43*x+0.59)+0.14),0.0,1.0);}";
const PK="vec4 pk(float d){vec4 e=fract(d*vec4(1.0,255.0,65025.0,16581375.0));e-=e.yzww*vec4(1.0/255.0,1.0/255.0,1.0/255.0,0.0);return e;}"+
"float unpk(vec4 c){return dot(c,vec4(1.0,1.0/255.0,1.0/65025.0,1.0/16581375.0));}";
const MAIN_V="attribute vec3 p;attribute vec3 n;attribute vec2 a;uniform mat4 m;uniform mat4 ls;varying vec3 wp;varying vec3 wn;varying vec2 va;varying vec4 lp;void main(){wp=p;wn=n;va=a;lp=ls*vec4(p,1.0);gl_Position=m*vec4(p,1.0);}";
const MAIN_F="precision highp float;varying vec3 wp;varying vec3 wn;varying vec2 va;varying vec4 lp;uniform vec3 eye;uniform vec3 facCol;uniform float facType;uniform float expo;uniform float shOn;uniform float texel;uniform float sunI;uniform sampler2D sm;const float PI=3.14159265;"+SKY+PK+
"float hash(vec2 p){return fract(sin(dot(p,vec2(127.1,311.7)))*43758.5453);}"+
"float noise(vec2 p){vec2 i=floor(p);vec2 f=fract(p);f=f*f*(3.0-2.0*f);return mix(mix(hash(i),hash(i+vec2(1.0,0.0)),f.x),mix(hash(i+vec2(0.0,1.0)),hash(i+vec2(1.0,1.0)),f.x),f.y);}"+
"float fbm(vec2 p){float s=0.0;float a=0.5;for(int i=0;i<4;i++){s+=a*noise(p);p*=2.03;a*=0.5;}return s;}"+
"vec2 planar(){vec3 n=abs(wn);if(n.z>=n.x&&n.z>=n.y)return wp.xy;if(n.x>n.y)return wp.yz;return wp.xz;}"+
"float shadow(float ndl){if(shOn<0.5)return 1.0;vec3 q=lp.xyz/lp.w*0.5+0.5;if(q.x<0.0||q.x>1.0||q.y<0.0||q.y>1.0||q.z>1.0)return 1.0;float bias=max(0.003*(1.0-ndl),0.001);float s=0.0;for(int i=-1;i<=1;i++){for(int j=-1;j<=1;j++){float d=unpk(texture2D(sm,q.xy+vec2(float(i),float(j))*texel));s+=(q.z-bias>d)?0.0:1.0;}}return s/9.0;}"+
"void surf(out vec3 alb,out float rough,out float met,out float ao){vec2 uv=planar();float mt=va.x;met=0.0;ao=1.0;float sd=va.y;"+
"if(mt<0.5){vec3 b=pow(facCol,vec3(2.2));"+
" if(facType<0.5){float row=floor(uv.y/0.2);vec2 cell=vec2((uv.x+mod(row,2.0)*0.2)/0.4,uv.y/0.2);vec2 id=floor(cell);vec2 f=fract(cell);float dm=min(min(f.x,1.0-f.x)*0.4,min(f.y,1.0-f.y)*0.2);float mortar=1.0-smoothstep(0.004,0.010,dm);vec3 stone=b*(0.84+0.32*hash(id+sd))*(0.88+0.24*fbm(uv*40.0));alb=mix(stone,b*0.5,mortar);ao=1.0-0.35*mortar;rough=0.85;}"+
" else if(facType<1.5){float band=noise(vec2(uv.x*0.7,uv.y*14.0))*0.18;float pit=step(0.84,noise(uv*70.0));vec2 pc=vec2(uv.x/0.6,uv.y/0.3);vec2 pf=fract(pc);float dj=min(min(pf.x,1.0-pf.x)*0.6,min(pf.y,1.0-pf.y)*0.3);float joint=1.0-smoothstep(0.002,0.006,dj);alb=b*(0.9+band)*(1.0-0.28*pit);alb=mix(alb,b*0.6,joint);ao=1.0-0.25*pit;rough=0.55;}"+
" else if(facType<2.5){float line=1.0-smoothstep(0.003,0.009,abs(fract(uv.y/0.25)-0.5)*0.25-0.1);alb=b*(0.96+0.08*fbm(uv*25.0))*(1.0-0.06*line);rough=0.9;}"+
" else{float id=floor(uv.x/0.14);float fx=fract(uv.x/0.14);float gap=1.0-smoothstep(0.02,0.06,min(fx,1.0-fx));float g=noise(vec2(uv.x*36.0,uv.y*3.0+hash(vec2(id,1.0))*50.0));alb=b*(0.72+0.42*g)*(0.88+0.24*hash(vec2(id,2.0)));alb=mix(alb,b*0.25,gap);ao=1.0-0.4*gap;rough=0.6;}}"+
"else if(mt<1.5){alb=vec3(0.78)*(0.97+0.05*noise(uv*30.0));rough=0.92;}"+
"else if(mt<3.5){vec3 w=pow(vec3(0.45,0.30,0.17),vec3(2.2));float g=noise(vec2(uv.x*6.0,uv.y*60.0));alb=w*(0.75+0.5*g);rough=0.5;}"+
"else if(mt<4.5){alb=pow(vec3(0.62,0.62,0.60),vec3(2.2))*(0.9+0.2*fbm(uv*18.0));rough=0.9;}"+
"else{float jn=1.0-smoothstep(0.01,0.03,min(abs(fract(uv.x/0.6)-0.5),abs(fract(uv.y/0.6)-0.5)));alb=pow(vec3(0.55,0.54,0.52),vec3(2.2))*(0.88+0.2*fbm(uv*12.0));alb*=1.0-0.18*(1.0-jn);rough=0.88;}}"+
"void main(){vec3 N=normalize(wn);vec3 V=normalize(eye-wp);vec3 L=sunDir;float ndl=max(dot(N,L),0.0);"+
"if(va.x>1.5&&va.x<2.5){vec3 R=reflect(-V,N);float f=0.04+0.96*pow(1.0-max(dot(N,V),0.0),5.0);vec3 c=sky(R)*f+vec3(0.02,0.04,0.05)*(1.0-f);float al=clamp(f+0.55,0.0,1.0);gl_FragColor=vec4(pow(aces(c*expo),vec3(1.0/2.2)),al);return;}"+
"vec3 alb;float rough;float met;float ao;surf(alb,rough,met,ao);"+
"float shd=ndl>0.0?shadow(ndl):0.0;"+
"vec3 H=normalize(V+L);float ndv=max(dot(N,V),0.001);float ndh=max(dot(N,H),0.0);float vdh=max(dot(V,H),0.0);"+
"float a=rough*rough;float a2=a*a;float dd=ndh*ndh*(a2-1.0)+1.0;float Dg=a2/(PI*dd*dd);"+
"float k=(rough+1.0)*(rough+1.0)/8.0;float G=(ndl/(ndl*(1.0-k)+k))*(ndv/(ndv*(1.0-k)+k));"+
"vec3 F0=mix(vec3(0.04),alb,met);vec3 F=F0+(1.0-F0)*pow(1.0-vdh,5.0);"+
"vec3 spec=Dg*G*F/(4.0*ndv*max(ndl,0.001));vec3 dif=(1.0-F)*(1.0-met)*alb/PI;"+
"vec3 direct=(dif+spec)*sunCol*sunI*ndl*shd;"+
"float low=clamp(wp.z/1.2,0.0,1.0);float aoh=(abs(N.z)<0.5)?mix(0.7,1.0,low):1.0;"+
"vec3 amb=mix(hor*0.45,zen,N.z*0.5+0.5)*alb*ao*aoh*0.9;"+
"vec3 col=direct+amb;col=pow(aces(col*expo),vec3(1.0/2.2));gl_FragColor=vec4(col,1.0);}";
const SH_V="attribute vec3 p;attribute vec3 n;attribute vec2 a;uniform mat4 ls;void main(){gl_Position=ls*vec4(p,1.0);}";
const SH_F="precision highp float;"+PK+"void main(){gl_FragColor=pk(gl_FragCoord.z);}";
const SKY_V="attribute vec2 p;varying vec2 uv;void main(){uv=p;gl_Position=vec4(p,0.9999,1.0);}";
const SKY_F="precision highp float;varying vec2 uv;uniform vec3 fwd;uniform vec3 rgt;uniform vec3 upv;uniform float tf;uniform float asp;uniform float expo;"+SKY+
"void main(){vec3 d=normalize(fwd+uv.x*tf*asp*rgt+uv.y*tf*upv);gl_FragColor=vec4(pow(aces(sky(d)*expo),vec3(1.0/2.2)),1.0);}";
const PM=prog(MAIN_V,MAIN_F),PS=prog(SH_V,SH_F),PK2=prog(SKY_V,SKY_F);
const UM={m:loc(PM,"m"),ls:loc(PM,"ls"),eye:loc(PM,"eye"),facCol:loc(PM,"facCol"),facType:loc(PM,"facType"),expo:loc(PM,"expo"),shOn:loc(PM,"shOn"),texel:loc(PM,"texel"),sunI:loc(PM,"sunI"),sm:loc(PM,"sm"),sunDir:loc(PM,"sunDir"),sunCol:loc(PM,"sunCol"),zen:loc(PM,"zen"),hor:loc(PM,"hor")};
const US={ls:loc(PS,"ls")};
const UK={fwd:loc(PK2,"fwd"),rgt:loc(PK2,"rgt"),upv:loc(PK2,"upv"),tf:loc(PK2,"tf"),asp:loc(PK2,"asp"),expo:loc(PK2,"expo"),sunDir:loc(PK2,"sunDir"),sunCol:loc(PK2,"sunCol"),zen:loc(PK2,"zen"),hor:loc(PK2,"hor")};
// Sky and sun colours follow the sun altitude the user's inputs produced.
const alt=D.sun.alt*Math.PI/180,k=Math.max(0,Math.min(1,D.sun.alt/30)),lerp=(a,b,t)=>a.map((v,i)=>v+(b[i]-v)*t);
const SUN=D.sun.dir,sunCol=lerp([1,.55,.3],[1,.96,.9],Math.pow(k,.5)),zen=lerp([.06,.1,.25],[.2,.42,.85],k),hor=lerp([.95,.6,.4],[.72,.82,.93],k),sunI=4*Math.sqrt(Math.sin(alt));
// Geometry: interleaved position(3) normal(3) material and seed(2).
const lo=D.min,hi=D.max,ctr=[(lo[0]+hi[0])/2,(lo[1]+hi[1])/2,(lo[2]+hi[2])/2],ext=Math.hypot(hi[0]-lo[0],hi[1]-lo[1],hi[2]-lo[2]);
function push(o,mn,mx,m,s){const[x0,y0,z0]=mn,[x1,y1,z1]=mx,f=[[[x0,y0,z0],[x1,y0,z0],[x1,y1,z0],[x0,y1,z0],[0,0,-1]],[[x0,y0,z1],[x1,y0,z1],[x1,y1,z1],[x0,y1,z1],[0,0,1]],[[x0,y0,z0],[x1,y0,z0],[x1,y0,z1],[x0,y0,z1],[0,-1,0]],[[x0,y1,z0],[x1,y1,z0],[x1,y1,z1],[x0,y1,z1],[0,1,0]],[[x0,y0,z0],[x0,y1,z0],[x0,y1,z1],[x0,y0,z1],[-1,0,0]],[[x1,y0,z0],[x1,y1,z0],[x1,y1,z1],[x1,y0,z1],[1,0,0]]];
 for(const[a,b,c,d,n]of f)for(const v of[a,b,c,a,c,d])o.push(v[0],v[1],v[2],n[0],n[1],n[2],m,s)}
const op=[],gl2=[],ground=[];
for(const b of D.boxes){push(b.g?gl2:op,b.mn,b.mx,b.m,b.s)}
const gh=Math.max(60,ext*6);push(ground,[ctr[0]-gh,ctr[1]-gh,-0.05],[ctr[0]+gh,ctr[1]+gh,-0.001],5,.5);
const mk=a=>{const b=gl.createBuffer();gl.bindBuffer(gl.ARRAY_BUFFER,b);gl.bufferData(gl.ARRAY_BUFFER,new Float32Array(a),gl.STATIC_DRAW);return{b,n:a.length/8}};
const B={op:mk(op),gl:mk(gl2),gr:mk(ground)};
const tri=gl.createBuffer();gl.bindBuffer(gl.ARRAY_BUFFER,tri);gl.bufferData(gl.ARRAY_BUFFER,new Float32Array([-1,-1,3,-1,-1,3]),gl.STATIC_DRAW);
const bind=b=>{gl.bindBuffer(gl.ARRAY_BUFFER,b.b);for(const i of[0,1,2])gl.enableVertexAttribArray(i);gl.vertexAttribPointer(0,3,gl.FLOAT,false,32,0);gl.vertexAttribPointer(1,3,gl.FLOAT,false,32,12);gl.vertexAttribPointer(2,2,gl.FLOAT,false,32,24)};
// Matrices (column-major).
const M={persp(fy,a,n,f){const t=1/Math.tan(fy/2);return[t/a,0,0,0,0,t,0,0,0,0,(f+n)/(n-f),-1,0,0,2*f*n/(n-f),0]},
 ortho(l,r,b,t,n,f){return[2/(r-l),0,0,0,0,2/(t-b),0,0,0,0,-2/(f-n),0,-(r+l)/(r-l),-(t+b)/(t-b),-(f+n)/(f-n),1]},
 look(e,c,u){const sub=(a,b)=>a.map((v,i)=>v-b[i]),cr=(a,b)=>[a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]],nm=a=>{const l=Math.hypot(...a);return a.map(v=>v/l)},dt=(a,b)=>a[0]*b[0]+a[1]*b[1]+a[2]*b[2];const z=nm(sub(e,c)),x=nm(cr(u,z)),y=cr(z,x);return{m:[x[0],y[0],z[0],0,x[1],y[1],z[1],0,x[2],y[2],z[2],0,-dt(x,e),-dt(y,e),-dt(z,e),1],x,y,z}},
 mul(a,b){const o=new Array(16).fill(0);for(let i=0;i<4;i++)for(let j=0;j<4;j++)for(let q=0;q<4;q++)o[j*4+i]+=a[q*4+i]*b[j*4+q];return o}};
// Shadow map: depth packed into RGBA so no extension is needed.
let FIX=null;const SZ=2048;let shadowOk=true,fbo=null,tex=null;
try{tex=gl.createTexture();gl.bindTexture(gl.TEXTURE_2D,tex);gl.texImage2D(gl.TEXTURE_2D,0,gl.RGBA,SZ,SZ,0,gl.RGBA,gl.UNSIGNED_BYTE,null);
 for(const[p,v]of[[gl.TEXTURE_MIN_FILTER,gl.NEAREST],[gl.TEXTURE_MAG_FILTER,gl.NEAREST],[gl.TEXTURE_WRAP_S,gl.CLAMP_TO_EDGE],[gl.TEXTURE_WRAP_T,gl.CLAMP_TO_EDGE]])gl.texParameteri(gl.TEXTURE_2D,p,v);
 const rb=gl.createRenderbuffer();gl.bindRenderbuffer(gl.RENDERBUFFER,rb);gl.renderbufferStorage(gl.RENDERBUFFER,gl.DEPTH_COMPONENT16,SZ,SZ);
 fbo=gl.createFramebuffer();gl.bindFramebuffer(gl.FRAMEBUFFER,fbo);gl.framebufferTexture2D(gl.FRAMEBUFFER,gl.COLOR_ATTACHMENT0,gl.TEXTURE_2D,tex,0);gl.framebufferRenderbuffer(gl.FRAMEBUFFER,gl.DEPTH_ATTACHMENT,gl.RENDERBUFFER,rb);
 shadowOk=gl.checkFramebufferStatus(gl.FRAMEBUFFER)===gl.FRAMEBUFFER_COMPLETE;gl.bindFramebuffer(gl.FRAMEBUFFER,null)}catch(e){shadowOk=false}
if(!shadowOk){$("sh").checked=false;$("sh").disabled=true;$("info").textContent+="  |  "+T.noshadow}
const R=ext/2*1.1+3,lv=M.look([ctr[0]+SUN[0]*R*2,ctr[1]+SUN[1]*R*2,ctr[2]+SUN[2]*R*2],ctr,Math.abs(SUN[2])>.99?[0,1,0]:[0,0,1]);
const LS=M.mul(M.ortho(-R,R,-R,R,.1,R*5),lv.m);
// Camera: free orbit plus the four sides of the plan.
const sides=[[0,-1],[1,0],[0,1],[-1,0]];
const comp=v=>{let b=(D.sun.north+Math.atan2(v[0],v[1])*180/Math.PI)%360;if(b<0)b+=360;return fmt(b,0)+"°"};
$("view").innerHTML='<option value="orbit">'+T.orbit+'</option>'+sides.map((v,i)=>'<option value="'+i+'">'+T.face+' '+comp(v)+'</option>').join("");
const st={yaw:-Math.PI/2-.55,pitch:.32,dist:ext*1.15+4,tg:[ctr[0],ctr[1],ctr[2]]};
function draw(){const dpr=devicePixelRatio||1,w=FIX?FIX[0]:Math.max(2,Math.round(cv.clientWidth*dpr)),h=FIX?FIX[1]:Math.max(2,Math.round(cv.clientHeight*dpr));if(cv.width!==w||cv.height!==h){cv.width=w;cv.height=h}
 const useSh=shadowOk&&$("sh").checked,expo=parseFloat($("ex").value)||1;
 gl.disable(gl.BLEND);gl.enable(gl.DEPTH_TEST);gl.depthMask(true);
 if(useSh){gl.bindFramebuffer(gl.FRAMEBUFFER,fbo);gl.viewport(0,0,SZ,SZ);gl.clearColor(1,1,1,1);gl.clear(gl.COLOR_BUFFER_BIT|gl.DEPTH_BUFFER_BIT);gl.useProgram(PS);gl.uniformMatrix4fv(US.ls,false,new Float32Array(LS));bind(B.op);gl.drawArrays(gl.TRIANGLES,0,B.op.n);gl.bindFramebuffer(gl.FRAMEBUFFER,null)}
 gl.viewport(0,0,w,h);gl.clearColor(0,0,0,1);gl.clear(gl.COLOR_BUFFER_BIT|gl.DEPTH_BUFFER_BIT);
 const cp=Math.cos(st.pitch),eye=[st.tg[0]+st.dist*cp*Math.cos(st.yaw),st.tg[1]+st.dist*cp*Math.sin(st.yaw),st.tg[2]+st.dist*Math.sin(st.pitch)],fov=.8,asp=w/h;
 const v=M.look(eye,st.tg,[0,0,1]),VP=M.mul(M.persp(fov,asp,.1,ext*40+800),v.m);
 gl.disable(gl.DEPTH_TEST);gl.useProgram(PK2);gl.bindBuffer(gl.ARRAY_BUFFER,tri);gl.enableVertexAttribArray(0);gl.vertexAttribPointer(0,2,gl.FLOAT,false,0,0);
 gl.uniform3f(UK.fwd,-v.z[0],-v.z[1],-v.z[2]);gl.uniform3f(UK.rgt,...v.x);gl.uniform3f(UK.upv,...v.y);gl.uniform1f(UK.tf,Math.tan(fov/2));gl.uniform1f(UK.asp,asp);gl.uniform1f(UK.expo,expo);
 gl.uniform3f(UK.sunDir,...SUN);gl.uniform3f(UK.sunCol,...sunCol);gl.uniform3f(UK.zen,...zen);gl.uniform3f(UK.hor,...hor);gl.drawArrays(gl.TRIANGLES,0,3);
 gl.enable(gl.DEPTH_TEST);gl.useProgram(PM);
 gl.uniformMatrix4fv(UM.m,false,new Float32Array(VP));gl.uniformMatrix4fv(UM.ls,false,new Float32Array(LS));gl.uniform3f(UM.eye,...eye);gl.uniform3f(UM.facCol,...D.facade.color);gl.uniform1f(UM.facType,D.facade.type);
 gl.uniform1f(UM.expo,expo);gl.uniform1f(UM.shOn,useSh?1:0);gl.uniform1f(UM.texel,1/SZ);gl.uniform1f(UM.sunI,sunI);
 gl.uniform3f(UM.sunDir,...SUN);gl.uniform3f(UM.sunCol,...sunCol);gl.uniform3f(UM.zen,...zen);gl.uniform3f(UM.hor,...hor);
 gl.activeTexture(gl.TEXTURE0);gl.bindTexture(gl.TEXTURE_2D,tex);gl.uniform1i(UM.sm,0);
 bind(B.gr);gl.drawArrays(gl.TRIANGLES,0,B.gr.n);bind(B.op);gl.drawArrays(gl.TRIANGLES,0,B.op.n);
 if(B.gl.n){gl.enable(gl.BLEND);gl.blendFunc(gl.SRC_ALPHA,gl.ONE_MINUS_SRC_ALPHA);gl.depthMask(false);bind(B.gl);gl.drawArrays(gl.TRIANGLES,0,B.gl.n);gl.depthMask(true);gl.disable(gl.BLEND)}}
$("view").onchange=()=>{const v=$("view").value;if(v!=="orbit"){const s=sides[+v];st.yaw=Math.atan2(s[1],s[0]);st.pitch=.18;st.tg=[ctr[0],ctr[1],ctr[2]]}draw()};
$("ex").oninput=draw;$("sh").onchange=draw;
$("png").onclick=()=>{draw();const a=document.createElement("a");a.download="facade-render.png";a.href=cv.toDataURL("image/png");a.click()};
let drag=null;cv.addEventListener("pointerdown",e=>{drag={x:e.clientX,y:e.clientY,pan:e.button===2||e.shiftKey};cv.setPointerCapture(e.pointerId)});
cv.addEventListener("pointerup",()=>drag=null);cv.addEventListener("contextmenu",e=>e.preventDefault());
cv.addEventListener("pointermove",e=>{if(!drag)return;const dx=e.clientX-drag.x,dy=e.clientY-drag.y;drag.x=e.clientX;drag.y=e.clientY;
 if(drag.pan){const kk=st.dist*.0016,c=Math.cos(st.yaw),s=Math.sin(st.yaw);st.tg[0]+=(s*dx-c*dy*Math.sin(st.pitch))*kk;st.tg[1]+=(-c*dx-s*dy*Math.sin(st.pitch))*kk}
 else{st.yaw-=dx*.007;st.pitch=Math.max(-.05,Math.min(1.5,st.pitch+dy*.007))}$("view").value="orbit";draw()});
cv.addEventListener("wheel",e=>{e.preventDefault();st.dist=Math.max(2,Math.min(ext*20+50,st.dist*Math.exp(e.deltaY*.001)));draw()},{passive:false});
addEventListener("resize",draw);
draw();
@@EXTRA@@
window.__renderReady=true;
}catch(e){fail(String(e&&e.message||e))}
})();
</script></body></html>
"##;
