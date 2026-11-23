//! A standalone HTML tour of the 3D model: ONE file with the scene data and a small WebGL viewer inside, no network and no library, that a
//! client opens in any browser: orbit, walk through at an eye height, toggle doors, x-ray, levels, lighting presets and a PNG snapshot.
//!
//! The scene is the model's own 3D solids (`model3d`): what you see is what the model holds. The facade and floor finishes of the
//! project colour the exterior walls and the floors. Nothing is fetched at run time.

use crate::model3d::Scene;
use serde_json::json;

const KINDS: [&str; 15] = ["Wall", "Slab", "Column", "Beam", "Footing", "TieBeam", "Door", "Window", "Space", "MepRun", "MepPoint", "Facade", "Site", "Vertical", "Furniture"];

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Kinds a client presentation keeps: what a visitor sees in the finished building. Structure (columns, beams, footings, tie beams, slabs) and
/// services (MEP runs and points) are engineering content and are dropped from the data itself, not only hidden by the page.
const CLIENT_KINDS: [&str; 8] = ["Wall", "Door", "Window", "Space", "Facade", "Site", "Vertical", "Furniture"];

/// The full tour for the design team: x-ray, MEP toggle and the eye-height input are present.
pub fn tour_html(scene: &Scene, title: &str, arabic: bool, facade: Option<[f64; 3]>, floor: Option<[f64; 3]>) -> Result<String, String> {
    tour_build(scene, title, arabic, facade, floor, false)
}

/// The client presentation: the same single offline file, but the structure and services are not in it and the page has no x-ray, MEP or
/// eye-height controls. Orbit, walk-through, doors, levels, lighting presets and the PNG snapshot remain (the walk eye height stays at the
/// page's 1.70 m default). Slabs are not included: the full tour only shows them in x-ray, which the client page does not offer.
pub fn tour_client_html(scene: &Scene, title: &str, arabic: bool, facade: Option<[f64; 3]>, floor: Option<[f64; 3]>) -> Result<String, String> {
    tour_build(scene, title, arabic, facade, floor, true)
}

fn tour_build(scene: &Scene, title: &str, arabic: bool, facade: Option<[f64; 3]>, floor: Option<[f64; 3]>, client: bool) -> Result<String, String> {
    let kinds: &[&str] = if client { &CLIENT_KINDS } else { &KINDS };
    let r3 = |v: f64| (v * 1000.0).round() / 1000.0;
    let boxes: Vec<serde_json::Value> = scene
        .boxes
        .iter()
        .filter(|b| kinds.contains(&b.kind))
        .map(|b| json!({ "k": b.kind, "c": b.class, "id": b.id, "mn": b.min.map(r3), "mx": b.max.map(r3), "l": b.level }))
        .collect();
    if boxes.is_empty() {
        return Err("nothing to export: the 3D model is empty".into());
    }
    let data = json!({
        "title": title, "ar": arabic, "client": client, "boxes": boxes, "min": scene.min.map(r3), "max": scene.max.map(r3),
        "levels": scene.levels.iter().map(|l| json!({ "index": l.index, "name": l.name, "z0": r3(l.z0), "storey": r3(l.storey) })).collect::<Vec<_>>(),
        "fin": { "facade": facade, "floor": floor },
    });
    let text = data.to_string().replace("</", "<\\/");
    Ok(TEMPLATE.replace("@@TITLE@@", &esc(title)).replace("@@DIR@@", if arabic { "rtl" } else { "ltr" }).replace("@@LANG@@", if arabic { "ar" } else { "en" }).replace("@@DATA@@", &text))
}

const TEMPLATE: &str = r##"<!doctype html>
<html lang="@@LANG@@" dir="@@DIR@@"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>@@TITLE@@</title>
<style>
html,body{margin:0;height:100%;font-family:'Segoe UI',Tahoma,Arial,sans-serif;background:#eef0f4;color:#1b1f27}
#bar{position:fixed;top:0;inset-inline:0;display:flex;flex-wrap:wrap;gap:8px;align-items:center;padding:6px 10px;background:#ffffffe6;border-bottom:1px solid #ccd;z-index:2;font-size:13px}
#bar b{font-size:15px}#bar button,#bar select,#bar input{font:inherit;padding:3px 8px}
canvas{position:fixed;inset:0;width:100%;height:100%;display:block;touch-action:none}
#help{position:fixed;bottom:8px;inset-inline-start:10px;background:#ffffffd9;padding:6px 10px;border-radius:6px;font-size:12px;z-index:2;max-width:70vw}
</style></head><body>
<canvas id="gl"></canvas>
<div id="bar"><b id="ttl"></b><button id="walk"></button><label id="w-eye"><span id="l-eye"></span> <input id="eye" type="number" step="0.05" value="1.70" style="width:64px"></label>
<label id="w-xray"><input type="checkbox" id="xray"> <span id="l-xray"></span></label><select id="light"><option value="day"></option><option value="golden"></option><option value="night"></option></select>
<select id="lvl"></select><label id="w-mep"><input type="checkbox" id="mep" checked> MEP</label><button id="png"></button></div>
<div id="help"></div>
<script>
const D=@@DATA@@;
(function(){
const T=D.ar?{walk:"وضع التجوّل",orbit:"وضع المدار",eye:"ارتفاع العين (م)",xray:"أشعة سينية",day:"نهار",golden:"الساعة الذهبية",night:"ليل",all:"كل الأدوار",png:"لقطة PNG",level:"الدور",
help:"المدار: اسحب للدوران، Shift أو الزر الأيمن للتحريك، العجلة للتقريب. التجوّل: WASD أو الأسهم للحركة، اسحب للنظر، E لفتح/إغلاق الباب الأقرب."}
:{walk:"Walk",orbit:"Orbit",eye:"Eye height (m)",xray:"X-ray",day:"Day",golden:"Golden hour",night:"Night",all:"All levels",png:"PNG snapshot",level:"Level",
help:"Orbit: drag to rotate, Shift or right button to pan, wheel to zoom. Walk: WASD or arrows to move, drag to look, E opens or closes the nearest door."};
const $=id=>document.getElementById(id);
$("ttl").textContent=D.title;$("walk").textContent=T.walk;$("l-eye").textContent=T.eye;$("l-xray").textContent=T.xray;$("png").textContent=T.png;$("help").textContent=T.help;
[["day",T.day],["golden",T.golden],["night",T.night]].forEach(([v,n],i)=>$("light").options[i].textContent=n);
$("lvl").innerHTML='<option value="all">'+T.all+'</option>'+D.levels.map(l=>'<option value="'+l.index+'">'+T.level+' '+(l.index+1)+'</option>').join("");
if(D.levels.length<2)$("lvl").style.display="none";
if(D.client)["w-xray","w-eye","w-mep"].forEach(i=>$(i).style.display="none");
const cv=$("gl"),gl=cv.getContext("webgl",{antialias:true,preserveDrawingBuffer:true});
if(!gl){document.body.innerHTML="<p style='padding:2em'>WebGL is not available in this browser.</p>";return}
const sh=(t,s)=>{const o=gl.createShader(t);gl.shaderSource(o,s);gl.compileShader(o);return o};
const pr=gl.createProgram();
gl.attachShader(pr,sh(gl.VERTEX_SHADER,"attribute vec3 p;attribute vec3 n;attribute vec4 c;uniform mat4 m;varying vec4 vc;varying vec3 vn;void main(){gl_Position=m*vec4(p,1.0);vn=n;vc=c;}"));
gl.attachShader(pr,sh(gl.FRAGMENT_SHADER,"precision mediump float;varying vec4 vc;varying vec3 vn;uniform vec3 l;uniform float amb;uniform vec3 tint;void main(){float d=max(dot(normalize(vn),normalize(l)),0.0);float k=amb+(1.0-amb)*d;gl_FragColor=vec4(vc.rgb*k*tint,vc.a);}"));
gl.linkProgram(pr);gl.useProgram(pr);
const A={p:gl.getAttribLocation(pr,"p"),n:gl.getAttribLocation(pr,"n"),c:gl.getAttribLocation(pr,"c")};Object.values(A).forEach(a=>gl.enableVertexAttribArray(a));
const U={m:gl.getUniformLocation(pr,"m"),l:gl.getUniformLocation(pr,"l"),amb:gl.getUniformLocation(pr,"amb"),tint:gl.getUniformLocation(pr,"tint")};
const buf={op:gl.createBuffer(),tr:gl.createBuffer(),nOp:0,nTr:0};
const PRE={day:{sky:[.95,.96,.98],l:[-.4,-.55,.75],amb:.5,tint:[1,1,1]},golden:{sky:[.99,.83,.62],l:[-.9,-.3,.25],amb:.38,tint:[1,.85,.7]},night:{sky:[.04,.06,.13],l:[.3,.3,.5],amb:.22,tint:[.5,.55,.85]}};
const ARCH=["Wall","Door","Window","Space","Facade","Site","Furniture","Vertical"];
const COL={Wall:[.93,.91,.87],Slab:[.6,.63,.65],Column:[.71,.27,.1],Beam:[.79,.45,.24],Footing:[.48,.42,.36],TieBeam:[.55,.38,.25],Door:[.54,.35,.17],Furniture:[.62,.45,.34],Vertical:[.7,.7,.72],Site:[.7,.7,.68],Facade:[.8,.76,.68]};
const RUN={duct:[.5,.77,.83],pipe:[.56,.82,.62],drain:[.73,.63,.49],cable:[.6,.4,.7]};
let st={walk:false,xray:false,light:"day",lvl:null,mep:true,yaw:-.8,pitch:.55,dist:30,tg:[0,0,1.5],pos:[0,0,1.7],wyaw:0,wpitch:0,open:new Set(),keys:{}};
const lo=D.min,hi=D.max;st.tg=[(lo[0]+hi[0])/2,(lo[1]+hi[1])/2,(lo[2]+hi[2])/2];st.dist=1.4*Math.hypot(hi[0]-lo[0],hi[1]-lo[1],hi[2]-lo[2])/(2*Math.tan(.4))+2;
const colour=b=>{
 const n=D.fin;let c;
 if(b.k==="Wall")c=(b.c==="ext"&&n.facade)?n.facade.concat(1):COL.Wall.concat(1);
 else if(b.k==="Space")c=(n.floor&&b.c!=="bathroom"&&b.c!=="toilet")?n.floor.concat(1):[.87,.87,.87,1];
 else if(b.k==="Window")c=[.31,.59,.78,.38];
 else if(b.k==="MepRun")c=(RUN[b.c]||[.4,.4,.4]).concat(1);
 else if(b.k==="MepPoint")c=[.95,.72,.02,1];
 else c=(COL[b.k]||[.6,.6,.6]).concat(1);
 if(st.xray){if(ARCH.includes(b.k))c=[c[0],c[1],c[2],.14];else if(b.k==="Slab")c=[c[0],c[1],c[2],.45];else if(b.k==="MepRun"||b.k==="MepPoint")c=[Math.min(1,c[0]*1.1),Math.min(1,c[1]*1.1),Math.min(1,c[2]*1.1),1]}
 if(st.light==="night"&&(b.k==="Window"||b.k==="MepPoint")){c=[1,.82,.45,.9]}
 return c};
const vis=b=>(st.lvl==null||b.l==null||b.l<=st.lvl)&&(b.k!=="Slab"||st.xray||false)&&(st.mep||(b.k!=="MepRun"&&b.k!=="MepPoint"))&&!(b.k==="Door"&&st.open.has(b.id))&&(b.k!=="Space"||!st.xray)&&!(b.k==="Slab"&&!st.xray&&false);
function push(o,b,c){const[x0,y0,z0]=b.mn,[x1,y1,z1]=b.mx,f=[[[x0,y0,z0],[x1,y0,z0],[x1,y1,z0],[x0,y1,z0],[0,0,-1]],[[x0,y0,z1],[x1,y0,z1],[x1,y1,z1],[x0,y1,z1],[0,0,1]],[[x0,y0,z0],[x1,y0,z0],[x1,y0,z1],[x0,y0,z1],[0,-1,0]],[[x0,y1,z0],[x1,y1,z0],[x1,y1,z1],[x0,y1,z1],[0,1,0]],[[x0,y0,z0],[x0,y1,z0],[x0,y1,z1],[x0,y0,z1],[-1,0,0]],[[x1,y0,z0],[x1,y1,z0],[x1,y1,z1],[x1,y0,z1],[1,0,0]]];
 for(const[a,b2,c2,d,n]of f)for(const v of[a,b2,c2,a,c2,d])o.push(v[0],v[1],v[2],n[0],n[1],n[2],c[0],c[1],c[2],c[3])}
function build(){const op=[],tr=[];for(const b of D.boxes){if(!vis(b))continue;const c=colour(b);push(c[3]<1?tr:op,b,c)}
 const up=(bf,a)=>{gl.bindBuffer(gl.ARRAY_BUFFER,bf);gl.bufferData(gl.ARRAY_BUFFER,new Float32Array(a),gl.STATIC_DRAW)};up(buf.op,op);up(buf.tr,tr);buf.nOp=op.length/10;buf.nTr=tr.length/10}
const M={persp(fy,a,n,f){const t=1/Math.tan(fy/2);return[t/a,0,0,0,0,t,0,0,0,0,(f+n)/(n-f),-1,0,0,2*f*n/(n-f),0]},
 look(e,c,u){const sub=(a,b)=>a.map((v,i)=>v-b[i]),cr=(a,b)=>[a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]],nm=a=>{const l=Math.hypot(...a);return a.map(v=>v/l)},dt=(a,b)=>a[0]*b[0]+a[1]*b[1]+a[2]*b[2];const z=nm(sub(e,c)),x=nm(cr(u,z)),y=cr(z,x);return[x[0],y[0],z[0],0,x[1],y[1],z[1],0,x[2],y[2],z[2],0,-dt(x,e),-dt(y,e),-dt(z,e),1]},
 mul(a,b){const o=new Array(16).fill(0);for(let i=0;i<4;i++)for(let j=0;j<4;j++)for(let k=0;k<4;k++)o[j*4+i]+=a[k*4+i]*b[j*4+k];return o}};
function draw(){const dpr=devicePixelRatio||1,w=Math.max(2,Math.round(cv.clientWidth*dpr)),h=Math.max(2,Math.round(cv.clientHeight*dpr));if(cv.width!==w||cv.height!==h){cv.width=w;cv.height=h}
 gl.viewport(0,0,w,h);const p=PRE[st.light];gl.clearColor(p.sky[0],p.sky[1],p.sky[2],1);gl.clear(gl.COLOR_BUFFER_BIT|gl.DEPTH_BUFFER_BIT);
 let eye,tg,fov=.8;
 if(st.walk){const cp=Math.cos(st.wpitch);eye=st.pos;tg=[eye[0]+Math.cos(st.wyaw)*cp,eye[1]+Math.sin(st.wyaw)*cp,eye[2]+Math.sin(st.wpitch)];fov=1.05}
 else{const cp=Math.cos(st.pitch);eye=[st.tg[0]+st.dist*cp*Math.cos(st.yaw),st.tg[1]+st.dist*cp*Math.sin(st.yaw),st.tg[2]+st.dist*Math.sin(st.pitch)];tg=st.tg}
 gl.uniformMatrix4fv(U.m,false,new Float32Array(M.mul(M.persp(fov,w/h,.05,800),M.look(eye,tg,[0,0,1]))));gl.uniform3f(U.l,...p.l);gl.uniform1f(U.amb,p.amb);gl.uniform3f(U.tint,...p.tint);
 const bind=b=>{gl.bindBuffer(gl.ARRAY_BUFFER,b);gl.vertexAttribPointer(A.p,3,gl.FLOAT,false,40,0);gl.vertexAttribPointer(A.n,3,gl.FLOAT,false,40,12);gl.vertexAttribPointer(A.c,4,gl.FLOAT,false,40,24)};
 gl.enable(gl.DEPTH_TEST);gl.disable(gl.BLEND);gl.depthMask(true);if(buf.nOp){bind(buf.op);gl.drawArrays(gl.TRIANGLES,0,buf.nOp)}
 if(buf.nTr){gl.enable(gl.BLEND);gl.blendFunc(gl.SRC_ALPHA,gl.ONE_MINUS_SRC_ALPHA);gl.depthMask(false);bind(buf.tr);gl.drawArrays(gl.TRIANGLES,0,buf.nTr);gl.depthMask(true)}}
const levelZ=()=>{const l=D.levels.find(x=>x.index===(st.lvl??0));return l?l.z0:0};
function startPos(){const z=levelZ(),sp=D.boxes.filter(b=>b.k==="Space"&&(D.levels.length===0||b.l===(st.lvl??0)))[0];
 st.pos=sp?[(sp.mn[0]+sp.mx[0])/2,(sp.mn[1]+sp.mx[1])/2,z+(parseFloat($("eye").value)||1.7)]:[st.tg[0],st.tg[1],z+(parseFloat($("eye").value)||1.7)]}
const R=.25;
function blocked(x,y){const z=levelZ(),eye=parseFloat($("eye").value)||1.7;
 for(const b of D.boxes){if(!(b.k==="Wall"||b.k==="Column"||(b.k==="Door"&&!st.open.has(b.id))))continue;if(b.mx[2]<z+.2||b.mn[2]>z+eye)continue;
  const cx=Math.max(b.mn[0],Math.min(x,b.mx[0])),cy=Math.max(b.mn[1],Math.min(y,b.mx[1]));if(Math.hypot(x-cx,y-cy)<R)return true}return false}
function tick(t){if(st.walk){const dt=Math.min(.05,(t-(tick.t||t))/1000);tick.t=t;const k=st.keys,sp=2.2*dt;
 let f=(k.w||k.arrowup?1:0)-(k.s||k.arrowdown?1:0),s=(k.d?1:0)-(k.a?1:0);if(k.arrowleft)st.wyaw+=1.6*dt;if(k.arrowright)st.wyaw-=1.6*dt;
 const dx=(Math.cos(st.wyaw)*f+Math.sin(st.wyaw)*s)*sp,dy=(Math.sin(st.wyaw)*f-Math.cos(st.wyaw)*s)*sp;
 if(dx||dy){if(!blocked(st.pos[0]+dx,st.pos[1]))st.pos[0]+=dx;if(!blocked(st.pos[0],st.pos[1]+dy))st.pos[1]+=dy}
 st.pos[2]=levelZ()+(parseFloat($("eye").value)||1.7);draw()}requestAnimationFrame(tick)}
function door(){let best=null,bd=2.5;for(const b of D.boxes){if(b.k!=="Door")continue;const cx=(b.mn[0]+b.mx[0])/2,cy=(b.mn[1]+b.mx[1])/2,d=Math.hypot(cx-st.pos[0],cy-st.pos[1]);if(d<bd){bd=d;best=b}}
 if(best){st.open.has(best.id)?st.open.delete(best.id):st.open.add(best.id);build();draw()}}
$("walk").onclick=()=>{st.walk=!st.walk;$("walk").textContent=st.walk?T.orbit:T.walk;if(st.walk){if(st.lvl==null&&D.levels.length)st.lvl=0;startPos()}build();draw()};
$("xray").onchange=()=>{st.xray=$("xray").checked;build();draw()};$("light").onchange=()=>{st.light=$("light").value;build();draw()};
$("mep").onchange=()=>{st.mep=$("mep").checked;build();draw()};$("lvl").onchange=()=>{st.lvl=$("lvl").value==="all"?null:Number($("lvl").value);if(st.walk)startPos();build();draw()};
$("eye").onchange=()=>draw();
$("png").onclick=()=>{draw();const a=document.createElement("a");a.download="view-"+st.light+".png";a.href=cv.toDataURL("image/png");a.click()};
let drag=null;cv.addEventListener("pointerdown",e=>{drag={x:e.clientX,y:e.clientY,pan:e.button===2||e.shiftKey};cv.setPointerCapture(e.pointerId)});
cv.addEventListener("pointerup",()=>drag=null);cv.addEventListener("contextmenu",e=>e.preventDefault());
cv.addEventListener("pointermove",e=>{if(!drag)return;const dx=e.clientX-drag.x,dy=e.clientY-drag.y;drag.x=e.clientX;drag.y=e.clientY;
 if(st.walk){st.wyaw-=dx*.005;st.wpitch=Math.max(-1.2,Math.min(1.2,st.wpitch-dy*.005))}
 else if(drag.pan){const k=st.dist*.0016,c=Math.cos(st.yaw),s=Math.sin(st.yaw);st.tg[0]+=(s*dx-c*dy*Math.sin(st.pitch))*k;st.tg[1]+=(-c*dx-s*dy*Math.sin(st.pitch))*k}
 else{st.yaw-=dx*.007;st.pitch=Math.max(-.1,Math.min(1.5,st.pitch+dy*.007))}draw()});
cv.addEventListener("wheel",e=>{e.preventDefault();if(!st.walk){st.dist=Math.max(2,Math.min(400,st.dist*Math.exp(e.deltaY*.001)));draw()}},{passive:false});
addEventListener("keydown",e=>{const k=e.key.toLowerCase();if(k==="e"&&st.walk)door();else st.keys[k]=true;if(st.walk&&k.startsWith("arrow"))e.preventDefault()});
addEventListener("keyup",e=>{st.keys[e.key.toLowerCase()]=false});addEventListener("resize",draw);
build();draw();requestAnimationFrame(tick);
})();
</script></body></html>
"##;
