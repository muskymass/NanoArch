// End-to-end check of the BIM drafting mode against the REAL NanoArch window, driven with real mouse and keyboard events.
//
//   cargo build
//   node scripts/e2e-draft.js <output folder for screenshots>
//
// It starts target\debug\nanoarch-app.exe with WebView2's remote-debugging port, generates a plan, and exercises: the draft mode toggle,
// snapping and the rubber band, drawing a wall, placing a door, selecting, editing a property, box select, Move, zoom / pan / fit, the
// split view following the edit, undo and redo; then a BLANK project drafted from nothing: walls and the room they enclose, a door, a window,
// grid lines, a permanent dimension attached to the walls, a room tag, renaming in the property panel, DXF and elevation output;
// then v2.0.0: a click on every thin element (first click, at a small zoom too), a room separator that makes two rooms, Offset, door tags, Split, Align and Trim to corner,
// a wall assembly made in the dialog and given to a wall, the real DXF export button, the toolbar on one row at 900 px, and a Blank project that inherits no sizes. Needs Node 22 or later (built-in WebSocket). Exits 1 if any check fails.
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

    // ---- P across a room of the GENERATED plan divides it, as one undoable edit, and the plan keeps its walls
    await key("Home"); await sleep(900); await key("p");
    check("P selects the room separator tool on a generated plan", (await ev(`DR.tool`)) === "separator");
    const gen = await ev(`(()=>{ let best=null; for (const r of document.querySelectorAll("#viewer rect.room")) { const b=r.getBBox(); if (!best || b.width*b.height > best.a) best={a:b.width*b.height,x:b.x,y:b.y,w:b.width,h:b.height}; } if(!best) return null; const p0=drToWorld({x:best.x,y:best.y}), p1=drToWorld({x:best.x+best.w,y:best.y+best.h}); return [Math.min(p0[0],p1[0]),Math.min(p0[1],p1[1]),Math.max(p0[0],p1[0]),Math.max(p0[1],p1[1])]; })()`);
    check("a room of the generated plan was found", Array.isArray(gen), JSON.stringify(gen));
    const gx = Math.round((gen[0] + gen[2]) / 2 / 100) * 100;
    const roomsGen0 = await count("#viewer rect.room"), wallsGen0 = await count("#viewer rect.wall"), tagsGen0 = await count("#viewer g.tag");
    const gp0 = await px(gx, gen[1] + 20), gp1 = await px(gx, gen[3] - 20);
    await click(gp0[0], gp0[1]); await click(gp1[0], gp1[1]);
    await key("Escape"); await key("Escape");
    check("P across a generated room: one separator and one more room", (await count("#viewer g.separator")) === 1 && (await count("#viewer rect.room")) === roomsGen0 + 1, `${roomsGen0} -> ${await count("#viewer rect.room")} rooms`);
    check("the generated plan keeps its walls (the separator is not a wall)", (await count("#viewer rect.wall")) === wallsGen0);
    const divided = await ev(`(async()=>{const o=await call("outline"); const keys=[]; const walk=n=>{ if(n.kind==="Space") keys.push(n.label||n.key||""); n.children.forEach(walk)}; walk(o.tree); return keys.length;})()`);
    check("the outliner lists the divided room's parts", divided === roomsGen0 + 1, `${divided} spaces`);
    await shot("e2e_4b_generated_room_divided");
    await key("z", { modifiers: 2 }); await sleep(900);
    check("one Ctrl+Z takes the division back: the room and the separator are gone", (await count("#viewer rect.room")) === roomsGen0 && (await count("#viewer g.separator")) === 0 && (await count("#viewer g.tag")) === tagsGen0, `${await count("#viewer rect.room")} rooms`);
    await key("y", { modifiers: 2 }); await sleep(900);
    check("Ctrl+Y divides it again", (await count("#viewer rect.room")) === roomsGen0 + 1 && (await count("#viewer g.separator")) === 1);
    await key("z", { modifiers: 2 }); await sleep(900); await key("v");

    // ================================================================ a BLANK project, drafted from nothing
    // The Blank project button asks before it discards unsaved work (a native dialog a script cannot answer): accept it.
    await ev(`window.confirm = () => true`);
    await ev(`(async()=>{ if (DR.on) await drToggle(false); })()`);
    const nb = await ev(`(()=>{const b=$("newblank").getBoundingClientRect(); return [b.x+b.width/2, b.y+b.height/2];})()`);
    await click(nb[0], nb[1]); await sleep(1500);
    check("the Blank project button opens an empty project in draft mode", await ev(`DR.on && !!DR.frame && document.querySelectorAll("#viewer rect.wall").length===0`));
    const lv = await ev(`(async()=>JSON.stringify((await call("draft_frame")).defaults))()`);
    check("nothing is pre-filled for a blank project (the user enters sizes)", JSON.parse(lv).wall === null, lv);
    const lvl = await ev(`(async()=>{const o=await call("outline"); return o.tree.children.filter(c=>c.kind==="Level").map(c=>c.en);})()`);
    check("the project has the level LVL-1", lvl.length === 1 && lvl[0].startsWith("LVL-1"), JSON.stringify(lvl));
    await shot("e2e_5_blank");

    // walls need a thickness and a height: with none entered the first wall is refused in words
    await key("w");
    const B = (x, y) => px(x, y);
    let c0 = await B(2000, 2000), c1 = await B(8000, 2000);
    await click(c0[0], c0[1]); await click(c1[0], c1[1]);
    check("a wall without a thickness and a height is refused", (await count("#viewer rect.wall")) === 0 && /thickness/i.test(await ev(`document.getElementById("toast").textContent`)), await ev(`document.getElementById("toast").textContent`));
    await key("Escape"); await key("Escape");
    const setOpt = (k, v) => ev(`(()=>{const i=document.querySelector('[data-dopt="${k}"]'); i.value="${v}"; i.dispatchEvent(new Event("change",{bubbles:true}));})()`);
    await key("w");
    await setOpt("thick", 200); await setOpt("height", 3000);
    const corners = [[2000, 2000], [8000, 2000], [8000, 6000], [2000, 6000], [2000, 2000]];
    for (const [x, y] of corners) { const q = await B(x, y); await click(q[0], q[1]); }
    check("four wall clicks make four walls", (await count("#viewer rect.wall")) === 4, `${await count("#viewer rect.wall")} walls`);
    const roomsNow = await ev(`(async()=>JSON.stringify((await call("outline")).tree.children[0].children.filter(g=>g.kind==="Spaces").map(g=>g.children.map(c=>c.en))))()`);
    check("the closed walls became a room, found automatically", (await count("#viewer rect.room")) === 1, roomsNow);
    await key("Escape"); await key("Escape");

    // a door and a window in the bottom wall: sizes are entered first
    await key("d"); await setOpt("dw", 900); await setOpt("dh", 2100);
    const m1 = await B(4000, 2000); await click(m1[0], m1[1]);
    check("a door is placed in a drafted wall of the blank project", (await count("#viewer g.door")) === 1);
    await key("n"); await setOpt("ww", 1200); await setOpt("wh", 1200); await setOpt("ws", 900);
    const m2 = await B(6500, 2000); await click(m2[0], m2[1]);
    check("a window is placed too", (await count("#viewer g.window")) === 1);

    // grid lines: G, two clicks each; names are automatic
    await key("g");
    let g0 = await B(1000, 1000), g1 = await B(1000, 7500);
    await click(g0[0], g0[1]); await click(g1[0], g1[1]);
    let h0 = await B(500, 4000), h1 = await B(9500, 4000);
    await click(h0[0], h0[1]); await click(h1[0], h1[1]);
    const gnames = await ev(`JSON.stringify([...document.querySelectorAll("#viewer g.gridline text")].map(t=>t.textContent))`);
    check("two grid lines drawn with the names A and 1", (await count("#viewer g.gridline")) === 2 && gnames.includes("\"A\"") && gnames.includes("\"1\""), gnames);

    // a permanent dimension between the left and right walls: three clicks, attached to them
    await key("i");
    const d0 = await B(2000, 5000), d1 = await B(8000, 5000), d2 = await B(5000, 7000);
    await click(d0[0], d0[1]); await click(d1[0], d1[1]);
    await move(d2[0], d2[1]); await sleep(400);
    check("the dimension preview is drawn before the third click", (await count("#dr-ov line")) >= 3);
    await click(d2[0], d2[1]);
    check("a permanent dimension of 6000 mm is on the plan", (await count("#viewer g.dimension")) === 1 && (await ev(`document.querySelector("#viewer g.dimension text").textContent`)) === "6000");
    await shot("e2e_6_grid_dim");

    // a room tag by clicking inside the room, then rename the room in the property panel and see the tag follow
    await key("q");
    const rc = await B(5000, 4500); await click(rc[0], rc[1]);
    check("the room tag shows the name, the area and the level elevation", (await count("#viewer g.tag")) === 1 && /24\.00 m²/.test(await ev(`document.querySelector("#viewer g.tag").textContent`)) && /±0\.00 m/.test(await ev(`document.querySelector("#viewer g.tag").textContent`)), await ev(`document.querySelector("#viewer g.tag").textContent`));
    check("a tagged room has no second label", (await count("#viewer text.name")) === 0);
    await key("v");
    const rp = await B(3000, 5200); await click(rp[0], rp[1]);
    const isSpace = await ev(`DR.props && DR.props.kind==="Space"`);
    check("clicking the room opens its properties", isSpace);
    await ev(`(()=>{const i=document.querySelector('[data-pf="name"]'); i.value="Majlis"; i.dispatchEvent(new Event("change",{bubbles:true}));})()`); await sleep(1000);
    check("renaming the room in the panel changes the tag", /Majlis/.test(await ev(`document.querySelector("#viewer g.tag").textContent`)));

    // the elevations and the DXF carry the grid lines and the dimension
    const exp = await ev(`(async()=>{ const d=await call("export_dxf",{}); const e=await call("elevation",{side:"south"}); return JSON.stringify({grid:d.counts["A-GRID"]||0, dims:d.counts["A-DIMS"]||0, eg:(e.svg.match(/class="grid"/g)||[]).length}); })()`);
    const ex = JSON.parse(exp);
    check("the DXF has A-GRID and A-DIMS entities and the elevation shows grid A", ex.grid >= 6 && ex.dims >= 6 && ex.eg === 1, exp);
    // one undo removes the last edit (the rename), another the tag
    const nTags = () => count("#viewer g.tag");
    await key("z", { modifiers: 2 }); await sleep(800); await key("z", { modifiers: 2 }); await sleep(800);
    check("undo walks back through the tag", (await nTags()) === 0);
    await key("y", { modifiers: 2 }); await sleep(600); await key("y", { modifiers: 2 }); await sleep(800);
    check("redo brings it back", (await nTags()) === 1);
    await shot("e2e_7_blank_done");


    // ================================================================ v2.0.0: first-click selection, separators, offset, assemblies, modify tools
    await key("Home"); await sleep(900); await key("v");
    const sel1 = async (label, q, expectKind) => {
      await ev(`selectIds([],{src:"draft"})`); await sleep(250);
      await click(q[0], q[1]); await sleep(300);
      const got = await ev(`DR.props ? DR.props.kind : null`);
      check(`one click selects ${label}`, got === expectKind, `got ${got}`);
    };
    const wallScreen = await B(2000, 3000);
    await sel1("a thin wall (pointer on it)", wallScreen, "Wall");
    await sel1("a thin wall (pointer 6 px beside it)", [wallScreen[0] - 6, wallScreen[1]], "Wall");
    const gridScreen = await B(1000, 3000);
    await sel1("a grid line (pointer 6 px beside it)", [gridScreen[0] + 6, gridScreen[1]], "Grid");
    const bubble = await ev(`(()=>{const c=document.querySelector("#viewer g.gridline circle.gbub"); const b=c.getBoundingClientRect(); return [b.x+b.width/2, b.y+b.height/2];})()`);
    await sel1("a grid bubble", bubble, "Grid");
    const dimScreen = await B(3000, 7000);
    await sel1("a dimension line (pointer 6 px beside it)", [dimScreen[0], dimScreen[1] - 6], "Dimension");
    // zoomed far out the walls are about one pixel wide: the target around them stays at least 8 px
    for (let i = 0; i < 3; i++) { await send("Input.dispatchMouseEvent", { type: "mouseWheel", x: 600, y: 400, deltaX: 0, deltaY: 240 }); await sleep(250); }
    const small = await B(2000, 3000);
    await sel1("a wall at a small zoom (pointer 7 px beside it)", [small[0] - 7, small[1]], "Wall");
    await key("Home"); await sleep(900);

    // ---- room separator: two rooms from one open room, no wall added
    await key("p");
    check("P selects the room separator tool", (await ev(`DR.tool`)) === "separator");
    const wallsBefore = await count("#viewer rect.wall");
    const s0 = await B(5000, 2000), s1 = await B(5000, 6000);
    await click(s0[0], s0[1]); await click(s1[0], s1[1]);
    check("a separator was drawn and the open room became two rooms", (await count("#viewer g.separator")) === 1 && (await count("#viewer rect.room")) === 2, `${await count("#viewer g.separator")} separators, ${await count("#viewer rect.room")} rooms`);
    check("the separator is not a wall", (await count("#viewer rect.wall")) === wallsBefore);
    await key("Escape");
    await ev(`document.querySelector('[data-dact="tagall"]').click()`); await sleep(1000);
    const tagTxt = await ev(`[...document.querySelectorAll("#viewer g.tag")].map(g=>g.textContent).join(" | ")`);
    check("each of the two spaces has its own area tag (12.00 m² each)", (tagTxt.match(/12\.00 m²/g) || []).length === 2, tagTxt);
    await shot("e2e_8_separator");

    // ---- offset: a parallel copy of the separator, 500 mm to the right
    await key("o");
    await setOpt("off", 500);
    const o0 = await B(5000, 3000); await click(o0[0], o0[1]);
    check("Offset: the element is taken", await ev(`!!DR.ref`));
    const o1 = await B(6500, 3000); await click(o1[0], o1[1]);
    check("Offset makes a second separator and a third room", (await count("#viewer g.separator")) === 2 && (await count("#viewer rect.room")) === 3, `${await count("#viewer g.separator")} separators, ${await count("#viewer rect.room")} rooms`);
    await key("Escape");

    // ---- tags on doors and windows
    await key("q");
    const dq = await B(4000, 2000); await click(dq[0], dq[1]);
    check("a door tag shows its kind and size", /Door/.test(await ev(`[...document.querySelectorAll("#viewer g.tag")].map(g=>g.textContent).join(" ")`)) && /900 × 2100/.test(await ev(`[...document.querySelectorAll("#viewer g.tag")].map(g=>g.textContent).join(" ")`)));
    await key("v");

    // ---- Split, Align and Trim to corner with the real mouse
    const wallsA = await count("#viewer rect.wall");
    await key("s");
    const sp = await B(8000, 3000); await click(sp[0], sp[1]);
    check("Split cuts the right wall in two", (await count("#viewer rect.wall")) === wallsA + 1, `${wallsA} -> ${await count("#viewer rect.wall")}`);
    await key("Escape");
    const drawWall = async (a, b) => { await key("w"); const q0 = await B(a[0], a[1]), q1 = await B(b[0], b[1]); await click(q0[0], q0[1]); await click(q1[0], q1[1]); await key("Escape"); await key("Escape"); };
    await drawWall([2000, 7500], [3500, 7500]);
    await drawWall([4000, 8000], [5500, 8000]);
    await key("a");
    const ra = await B(2500, 7500); await click(ra[0], ra[1]);
    const rb = await B(4500, 8000); await click(rb[0], rb[1]);
    const aligned = await ev(`(async()=>{ const o=await call("draft_box",{box:[3900,7400,5600,7600],mode:"window"}); return o.ids.length; })()`);
    check("Align brings the second wall onto the first one's line", aligned === 1, `${aligned} wall(s) now at y 7500 beyond x 3900`);
    await key("Escape");
    await drawWall([2000, 9000], [3000, 9000]);
    await drawWall([3500, 8700], [3500, 9500]);
    await key("t");
    const tr0 = await B(2300, 9000); await click(tr0[0], tr0[1]);
    const tr1 = await B(3500, 9300); await click(tr1[0], tr1[1]);
    const trimmed = await ev(`(async()=>{ const o=await call("draft_box",{box:[1900,8900,3700,9100],mode:"window"}); const out=[]; for (const id of o.ids) { const p=await call("draft_props",{id}); const l=p.fields.find(f=>f.key==="length"); if (l) out.push(l.value); } return out; })()`);
    check("Trim to corner extends the first wall to the second (length 1500)", trimmed.includes(1500), JSON.stringify(trimmed));
    await key("Escape"); await key("v");
    await shot("e2e_9_modify");

    // ---- wall assemblies: define one in the dialog, give it to the left wall
    await ev(`document.querySelector('[data-dact="asm"]').click()`); await sleep(1200);
    check("the assemblies dialog opens", (await count("#asm-box")) === 1);
    await ev(`document.getElementById("asm-add").click()`); await sleep(400);
    await ev(`(()=>{ const set=(sel,v)=>{ const i=document.querySelector(sel); i.value=v; i.dispatchEvent(new Event("change",{bubbles:true})); }; set('#asm-box input[data-f=name]', 'Block 200'); set('#asm-box input[data-f=thicknessMm]', '200'); })()`); await sleep(300);
    await ev(`document.getElementById("asm-save").click()`); await sleep(1500);
    const asm = await ev(`(async()=>{ const r=await call("assemblies_get"); return JSON.stringify(r.assemblies.map(a=>({name:a.assembly.name, t:a.thickness_mm}))); })()`);
    check("the assembly was saved with a thickness equal to the sum of its layers", asm === JSON.stringify([{ name: "Block 200", t: 200 }]), asm);
    check("the dialog closed", (await count("#asm-box")) === 0);
    await key("v");
    const lw = await B(2000, 4500); await click(lw[0], lw[1]);
    await ev(`(()=>{ const s=document.querySelector("[data-asm]"); s.value=s.options[1].value; s.dispatchEvent(new Event("change",{bubbles:true})); })()`); await sleep(1200);
    const lwId = await ev(`[...SEL.ids][0]`);
    const lwProps = await ev(`(async()=>JSON.stringify((await call("draft_props",{id:[...SEL.ids][0]})).fields.filter(f=>["thickness","assembly"].includes(f.key)).map(f=>[f.key,f.value])))()`);
    check("the wall now uses the assembly and its thickness is the layers' sum", lwProps === JSON.stringify([["thickness", 200], ["assembly", "Block 200"]]), lwProps);
    const refused = await ev(`(async()=>{ try { await call("draft_set",{id:"${lwId}",key:"thickness",value:300}); return "accepted"; } catch (e) { return e.message || String(e); } })()`);
    check("typing another thickness on that wall is refused (the assembly owns it)", /assembly/.test(refused), refused);
    const sc = await ev(`(async()=>Object.keys((await call("model3d")).assemblies||{}).length)()`);
    check("the 3D scene names the wall's assembly", sc === 1, String(sc));

    // ---- the real DXF export carries the separators
    const eb = await ev(`(()=>{const b=$("exportdxf").getBoundingClientRect(); return [b.x+b.width/2, b.y+b.height/2];})()`);
    for (const f of fs.readdirSync(D)) if (/\.dxf$/i.test(f)) fs.unlinkSync(`${D}\\${f}`);
    const before = new Set(fs.readdirSync(D));
    await click(eb[0], eb[1]); await sleep(2500);
    const newDxf = fs.readdirSync(D).filter(f => !before.has(f) && /\.dxf$/i.test(f));
    const dxfText = newDxf.length ? fs.readFileSync(`${D}\\${newDxf[0]}`, "utf8") : "";
    check("the exported DXF file has the room separators", newDxf.length === 1 && dxfText.includes("A-ROOM-SEP") && dxfText.includes("A-GRID"), newDxf.join(","));

    // ---- the toolbar stays on one row at a narrow window
    for (const w of [1100, 900]) {
      await send("Emulation.setDeviceMetricsOverride", { width: w, height: 700, deviceScaleFactor: 1, mobile: false }); await sleep(700);
      const rows = await ev(`(()=>{ const bar=$("draftbar"); const tops=new Set([...bar.querySelectorAll(".grp")].map(g=>Math.round(g.getBoundingClientRect().top))); return { tops: tops.size, h: Math.round(bar.getBoundingClientRect().height) }; })()`);
      check(`the drafting toolbar is one compact row at ${w} px`, rows.tops === 1 && rows.h >= 30 && rows.h < 60, JSON.stringify(rows));
      await shot(`e2e_10_narrow_${w}`);
    }
    await send("Emulation.clearDeviceMetricsOverride"); await sleep(500);

    // ---- a Blank project never inherits the sizes of the previous one
    await ev(`window.confirm = () => true`);
    await ev(`(async()=>{ if (DR.on) { const wb = document.querySelector('[data-dtool=wall]'); wb.click(); } })()`); await sleep(300);
    await setOpt("thick", 333); await setOpt("height", 2777);
    check("sizes are set before the reset", (await ev(`DR.opt.thick + "/" + DR.opt.height`)) === "333/2777");
    const nb2 = await ev(`(()=>{const b=$("newblank").getBoundingClientRect(); return [b.x+b.width/2, b.y+b.height/2];})()`);
    await click(nb2[0], nb2[1]); await sleep(2000);
    await key("w");
    const leak = await ev(`JSON.stringify({ opt: [DR.opt.thick, DR.opt.height, DR.opt.dw, DR.opt.ww, DR.opt.off], inputs: [...document.querySelectorAll('#draftbar [data-dopt=thick], #draftbar [data-dopt=height]')].map(i=>i.value) })`);
    check("after Blank project the thickness and height inputs are empty", leak === JSON.stringify({ opt: ["", "", "", "", ""], inputs: ["", ""] }), leak);
    check("the new blank project has no assemblies and no walls", (await ev(`(async()=>(await call("assemblies_get")).assemblies.length)()`)) === 0 && (await count("#viewer rect.wall")) === 0);

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
