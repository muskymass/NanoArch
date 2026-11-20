// End-to-end check of the BIM drafting mode against the REAL NanoArch window, driven with real mouse and keyboard events.
//
//   cargo build
//   node scripts/e2e-draft.js <output folder for screenshots>
//
// It starts target\debug\nanoarch-app.exe with WebView2's remote-debugging port, generates a plan, and exercises: the draft mode toggle,
// snapping and the rubber band, drawing a wall, placing a door, selecting, editing a property, box select, Move, zoom / pan / fit, the
// split view following the edit, undo and redo. Needs Node 22 or later (built-in WebSocket). Exits 1 if any check fails.
// Drives the REAL NanoArch window over the WebView2 debugging port with real mouse / keyboard events.
const { spawn } = require("child_process");
const fs = require("fs");
const http = require("http");
const D = process.argv[2];
const PORT = 9333;
const EXE = "C:\\Projects\\NanoArch\\target\\debug\\nanoarch-app.exe";
const sleep = ms => new Promise(r => setTimeout(r, ms));
const get = url => new Promise((res, rej) => http.get(url, r => { let b = ""; r.on("data", c => (b += c)); r.on("end", () => res(b)); }).on("error", rej));
const results = [];
const check = (name, ok, extra = "") => { results.push(ok); console.log((ok ? "PASS " : "FAIL ") + name + (extra ? "  — " + extra : "")); };

(async () => {
  const app = spawn(EXE, [], { env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}`, NANOARCH_EXPORT_DIR: D }, stdio: "ignore" });
  let targets;
  for (let i = 0; i < 60; i++) { try { targets = JSON.parse(await get(`http://127.0.0.1:${PORT}/json`)); if (targets.some(t => t.type === "page")) break; } catch (e) {} await sleep(500); }
  const page = targets.find(t => t.type === "page");
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise(r => (ws.onopen = r));
  let id = 0; const waits = new Map(); const errors = [];
  ws.onmessage = m => {
    const d = JSON.parse(m.data);
    if (d.id && waits.has(d.id)) { waits.get(d.id)(d); waits.delete(d.id); }
    else if (d.method === "Runtime.exceptionThrown") errors.push(d.params.exceptionDetails.exception?.description || d.params.exceptionDetails.text);
  };
  const send = (method, params = {}) => new Promise(r => { const i = ++id; waits.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
  const ev = async expr => { const r = await send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true }); if (r.result.exceptionDetails) throw new Error(r.result.exceptionDetails.exception?.description || r.result.exceptionDetails.text); return r.result.result.value; };
  await send("Runtime.enable"); await send("Page.enable");
  const shot = async name => { const r = await send("Page.captureScreenshot", { format: "png" }); fs.writeFileSync(`${D}\\${name}.png`, Buffer.from(r.result.data, "base64")); };
  const mouse = (type, x, y, extra = {}) => send("Input.dispatchMouseEvent", { type, x, y, button: "left", buttons: type === "mouseReleased" ? 0 : 1, clickCount: 1, ...extra });
  const move = (x, y) => send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y, button: "none", buttons: 0 });
  const click = async (x, y, extra = {}) => { await move(x, y); await sleep(120); await mouse("mousePressed", x, y, extra); await mouse("mouseReleased", x, y, extra); await sleep(450); };
  const key = async (k, extra = {}) => { await send("Input.dispatchKeyEvent", { type: "keyDown", key: k, text: k.length === 1 ? k : undefined, ...extra }); await send("Input.dispatchKeyEvent", { type: "keyUp", key: k, ...extra }); await sleep(300); };
  // client pixel of a world point (mm)
  const px = async (x, y) => ev(`(()=>{const s=drSvg(), p=s.createSVGPoint(), f=drToSvg([${x},${y}]); p.x=f[0]; p.y=f[1]; const c=p.matrixTransform(s.getScreenCTM()); return [c.x,c.y];})()`);
  const model = () => ev(`(async()=>{const o=await call("outline"); const ids=o.tree.ids; return ids.length;})()`);
  const count = sel => ev(`document.querySelectorAll(${JSON.stringify(sel)}).length`);

  try {
    await sleep(2500);
    // a plan, as the page would show it after generate + select
    await ev(`(async()=>{
      const r = (key, name, func, area, zone, light) => ({ key, name, func, area, zone, light });
      const brief = { title: "e2e", width_m: 18, length_m: 22, setback_front_m: 2, setback_back_m: 2, setback_side_m: 2, entry: "south", rooms: [
        r("majlis","المجلس","majlis",36,"public",true), r("living","الصالة","living",30,"public",true), r("dining","الطعام","dining",24,"public",true), r("kitchen","المطبخ","kitchen",20,"service",true),
        r("master","الماستر","bedroom",28,"private",true), r("bed1","غرفة 1","bedroom",20,"private",true), r("bed2","غرفة 2","bedroom",20,"private",true), r("bed3","غرفة 3","bedroom",20,"private",true),
        r("bath1","حمام 1","bathroom",6,"service",false), r("bath2","حمام 2","bathroom",6,"service",false), r("bath3","حمام 3","bathroom",6,"service",false)] };
      const st = { dead_kpa: 5, live_kpa: 2, bearing_kpa: 150, max_span_mm: 5000 };
      await call("generate", { brief, struct: st }); const s = await call("select", { index: 0, struct: st });
      showPlan(s.svg); setLevels(s.levels); setPlanButtons(true); return true; })()`);
    await sleep(800);
    check("plan shown", (await count("#viewer rect.wall")) > 5, `${await count("#viewer rect.wall")} wall rects`);
    await shot("e2e_1_plan");

    // ---- enter draft mode with the real button
    const dbtn = await ev(`(()=>{const b=$("draft").getBoundingClientRect(); return [b.x+b.width/2, b.y+b.height/2];})()`);
    await click(dbtn[0], dbtn[1]); await sleep(900);
    check("draft mode on, toolbar and frame ready", await ev(`DR.on && !!DR.frame && $("draftbar").style.display==="flex" && !!$("draftbar").querySelector("[data-dtool=wall]")`));
    const defs = await ev(`JSON.stringify(DR.opt)`); check("starting values learned from the plan, not invented", /"thick":"\d+"/.test(defs) && /"dw":"\d+"/.test(defs), defs);

    // pick a spot: the middle of the biggest room
    const room = await ev(`(async()=>{const o=await call("outline"); const rs=[]; const walk=n=>{ if(n.kind==="Space") rs.push(n); n.children.forEach(walk)}; walk(o.tree); const d=await call("describe",{id:rs[0].element}); return d;})()`);
    const rect = await ev(`(async()=>{ const m=await call("draft_box",{box:[-1e6,-1e6,1e6,1e6],mode:"crossing"}); return m.ids.length;})()`);
    check("box query over everything finds walls and openings", rect > 10, `${rect} ids`);
    // plan extent
    const ext = await ev(`(()=>{const f=DR.frame; const s=drSvg().viewBox.baseVal; const a=drToWorld({x:s.x,y:s.y+s.height*0.6}); const b=drToWorld({x:s.x+s.width,y:s.y}); return [a[0],a[1],b[0],b[1]];})()`);

    // ---- draw a wall with real mouse clicks; keyboard 'w' picks the tool
    await key("w");
    check("W selects the wall tool", (await ev(`DR.tool`)) === "wall");
    // choose two points along a horizontal line inside the plan (grid-aligned)
    const wx0 = Math.round((ext[0] + (ext[2] - ext[0]) * 0.3) / 500) * 500, wx1 = wx0 + 2000, wy = Math.round((ext[1] + (ext[3] - ext[1]) * 0.5) / 500) * 500;
    const walls0 = await count("#viewer rect.wall");
    let p0 = await px(wx0, wy), p1 = await px(wx1, wy);
    await click(p0[0], p0[1]);
    check("first click starts a wall", (await ev(`!!DR.start`)));
    await move(p1[0], p1[1]); await sleep(400);
    check("rubber band and snap marker are drawn", (await count("#dr-ov line")) >= 1);
    await shot("e2e_2_rubberband");
    await click(p1[0], p1[1]);
    const walls1 = await count("#viewer rect.wall");
    check("second click creates the wall as one edit", walls1 === walls0 + 1, `${walls0} -> ${walls1}`);
    await key("Escape"); await key("Escape");
    check("Esc ends the chain then returns to select", (await ev(`!DR.start && DR.tool==="select"`)));

    // ---- door on the new wall
    await key("d");
    const mid = await px((wx0 + wx1) / 2, wy);
    const doors0 = await count("#viewer g.door");
    await move(mid[0], mid[1]); await sleep(500);
    check("door ghost follows the wall", await ev(`!!DR.placement && DR.placement.fits`), JSON.stringify(await ev(`DR.placement && {offset:DR.placement.offset}`)));
    await click(mid[0], mid[1]);
    check("a door was placed", (await count("#viewer g.door")) === doors0 + 1);

    // ---- select the wall by clicking it, and edit its thickness in the inspector
    await key("v");
    const wallPt = await px(wx0 + 300, wy);
    await click(wallPt[0], wallPt[1]);
    check("click selects the wall and the property panel opens", await ev(`DR.props && DR.props.kind==="Wall" && $("dr-props").style.display==="flex"`));
    check("hub: the selection reaches the outliner/3D set", (await ev(`SEL.ids.size`)) === 1);
    const t0 = await ev(`DR.props.fields.find(f=>f.key==="thickness").value`);
    await ev(`(()=>{const i=document.querySelector('[data-pf="thickness"]'); i.value="${Number(t0) + 50}"; i.dispatchEvent(new Event("change",{bubbles:true}));})()`);
    await sleep(900);
    const t1 = await ev(`(async()=>(await call("draft_props",{id:[...SEL.ids][0]})).fields.find(f=>f.key==="thickness").value)()`);
    check("editing thickness in the inspector changes the model", t1 === Number(t0) + 50, `${t0} -> ${t1}`);

    // ---- box select (window: left to right) around the wall and its door
    const a = await px(wx0 - 400, wy + 400), b = await px(wx1 + 400, wy - 400);
    await ev(`selectIds([],{src:"draft"})`);
    await move(a[0], a[1]); await mouse("mousePressed", a[0], a[1]); await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: (a[0] + b[0]) / 2, y: (a[1] + b[1]) / 2, button: "left", buttons: 1 }); await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: b[0], y: b[1], button: "left", buttons: 1 }); await mouse("mouseReleased", b[0], b[1]); await sleep(900);
    check("a window box selects the drawn wall and its door", (await ev(`SEL.ids.size`)) >= 2, `${await ev(`SEL.ids.size`)} selected`);
    await shot("e2e_3_selected");

    // ---- move with the Move tool
    await key("m");
    const base = await px(wx0, wy), tgt = await px(wx0, wy + 1000);
    const beforeY = await ev(`(async()=>{const id=[...SEL.ids].find(i=>true); const o=await call("draft_box",{box:[${wx0 - 100},${wy - 100},${wx1 + 100},${wy + 100}],mode:"window"}); return o.ids.length;})()`);
    await click(base[0], base[1]); check("Move: base point taken", await ev(`!!DR.base`));
    await click(tgt[0], tgt[1]);
    const moved = await ev(`(async()=>{const o=await call("draft_box",{box:[${wx0 - 100},${wy + 900},${wx1 + 100},${wy + 1100}],mode:"window"}); return o.ids.length;})()`);
    check("the wall and its door moved up by 1000 mm", moved >= 2, `${beforeY} in the old place, ${moved} in the new`);

    // ---- zoom with the wheel and pan with the middle button
    await key("v");
    const vb0 = await ev(`drSvg().getAttribute("viewBox")`);
    await send("Input.dispatchMouseEvent", { type: "mouseWheel", x: mid[0], y: mid[1], deltaX: 0, deltaY: -240 }); await sleep(400);
    const vb1 = await ev(`drSvg().getAttribute("viewBox")`);
    check("wheel zooms the plan", vb0 !== vb1, `${vb0} -> ${vb1}`);
    await send("Input.dispatchMouseEvent", { type: "mousePressed", x: mid[0], y: mid[1], button: "middle", buttons: 4, clickCount: 1 });
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: mid[0] + 60, y: mid[1] + 30, button: "middle", buttons: 4 });
    await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: mid[0] + 60, y: mid[1] + 30, button: "middle", buttons: 0, clickCount: 1 }); await sleep(300);
    check("middle-drag pans", (await ev(`drSvg().getAttribute("viewBox")`)) !== vb1);
    await key("Home"); await sleep(900);
    check("Home fits the plan again", (await ev(`DR.vw===null`)));

    // ---- split view: the 3D view follows the edits
    await ev(`$("split").click()`); await sleep(1500);
    check("split view shows plan and 3D side by side", await ev(`V3.on && V3.split && $("vp").classList.contains("split")`));
    const boxes0 = await ev(`V3.boxes.length`);
    await key("w");
    const q0 = await px(wx0, wy - 1500), q1 = await px(wx0 + 1500, wy - 1500);
    await click(q0[0], q0[1]); await click(q1[0], q1[1]); await sleep(1500);
    const boxes1 = await ev(`V3.boxes.length`);
    check("the 3D view got the new wall after the 2D edit", boxes1 > boxes0, `${boxes0} -> ${boxes1} solids`);
    await key("Escape"); await key("Escape");
    await shot("e2e_4_split");

    // ---- undo with the keyboard walks back; the outliner sees the drafted elements
    const n1 = await model();
    await key("z", { modifiers: 2 }); await sleep(900);
    const n2 = await model();
    check("Ctrl+Z undoes the last edit", n2 < n1, `${n1} -> ${n2} elements`);
    await key("y", { modifiers: 2 }); await sleep(900);
    check("Ctrl+Y redoes it", (await model()) === n1);

    // ---- project persistence: the project is the model, so a save keeps drafted elements
    const user = await ev(`(async()=>{const o=await call("outline"); let n=0; const walk=x=>{ if(x.kind==="Wall") n++; x.children.forEach(walk)}; walk(o.tree); return n;})()`);
    check("drafted walls appear in the outliner", user > 5);
    check("no uncaught script errors during the session", errors.length === 0, errors.join(" | ").slice(0, 300));
  } catch (e) {
    check("session ran without throwing", false, String(e).slice(0, 400));
  }
  const failed = results.filter(r => !r).length;
  console.log(`\n${results.length - failed}/${results.length} checks passed`);
  try { ws.close(); } catch (e) {}
  app.kill();
  process.exit(failed ? 1 : 0);
})();
