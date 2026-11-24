// End-to-end check of the Deliverables tab against the REAL NanoArch window, driven with real mouse clicks over the WebView2 debugging port.
//
//   cargo build
//   node scripts/e2e-deliverables.js <output folder for screenshots and exports>
//
// Generates a plan, opens the tab, and exercises: the overview, a work inspection request that is REFUSED with an unsigned hold point and
// accepted once signed, the soil report, a payment certificate on the project's own BOQ, the draft contract, and the full dossier ZIP written
// to disk. Needs Node 22 or later (built-in WebSocket). Exits 1 if any check fails.
const { spawn } = require("child_process");
const fs = require("fs");
const http = require("http");
const D = process.argv[2];
if (!D) { console.error("usage: node scripts/e2e-deliverables.js <output folder>"); process.exit(2); }
fs.mkdirSync(D, { recursive: true });
const PORT = 9334;
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
  const mouse = (type, x, y) => send("Input.dispatchMouseEvent", { type, x, y, button: "left", buttons: type === "mouseReleased" ? 0 : 1, clickCount: 1 });
  const click = async sel => {
    const c = await ev(`(()=>{const e=document.querySelector(${JSON.stringify(sel)}); if(!e) return null; e.scrollIntoView({block:"center"}); const b=e.getBoundingClientRect(); return [b.x+b.width/2, b.y+b.height/2];})()`);
    if (!c) throw new Error("no element " + sel);
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: c[0], y: c[1], button: "none", buttons: 0 }); await sleep(80);
    await mouse("mousePressed", c[0], c[1]); await mouse("mouseReleased", c[0], c[1]); await sleep(900);
  };
  const setVal = (sel, v) => ev(`(()=>{const e=document.querySelector(${JSON.stringify(sel)}); e.value=${JSON.stringify(v)}; return true;})()`);
  const text = sel => ev(`(document.querySelector(${JSON.stringify(sel)})||{}).innerText||""`);

  try {
    await sleep(2500);
    await ev(`(async()=>{
      const r = (key, name, func, area, zone, light) => ({ key, name, func, area, zone, light });
      const brief = { title: "e2e", width_m: 18, length_m: 22, setback_front_m: 2, setback_back_m: 2, setback_side_m: 2, entry: "south", rooms: [
        r("majlis","المجلس","majlis",36,"public",true), r("living","الصالة","living",30,"public",true), r("dining","الطعام","dining",24,"public",true), r("kitchen","المطبخ","kitchen",20,"service",true),
        r("master","الماستر","bedroom",28,"private",true), r("bed1","غرفة 1","bedroom",20,"private",true), r("bed2","غرفة 2","bedroom",20,"private",true), r("bed3","غرفة 3","bedroom",20,"private",true),
        r("bath1","حمام 1","bathroom",6,"service",false), r("bath2","حمام 2","bathroom",6,"service",false), r("bath3","حمام 3","bathroom",6,"service",false)] };
      const st = { dead_kpa: 5, live_kpa: 2, bearing_kpa: 150, max_span_mm: 5000 };
      await call("generate", { brief, struct: st }); const s = await call("select", { index: 0, struct: st });
      showPlan(s.svg); setLevels(s.levels); setPlanButtons(true); lang = "en"; return true; })()`);
    await sleep(800);

    // ---- open the tab with a real click
    await click("#tab-dv");
    check("the Deliverables tab opens and shows the ten indicators", (await ev(`document.querySelectorAll("#p-dv .hint + b").length`)) === 10, await text("#p-dv"));
    check("the close-out is not ready on an empty project", (await text("#dv-body")).toLowerCase().includes("not ready"));
    await shot("dv_1_overview");

    // ---- WIR: a request started from the stage's hold points
    await click('[data-dvsec="wir"]');
    await click("#dv-tpl");
    check("the template adds a request with the stage's hold points", (await ev(`document.querySelector('#dv-rows textarea[data-f="holdPoints"]').value.split("\\n").length`)) >= 2);
    await setVal('#dv-rows [data-f="element"]', "FND-1");
    await setVal('#dv-rows [data-f="status"]', "approved");
    await setVal('#dv-rows [data-f="inspector"]', "Eng. A");
    await setVal('#dv-rows [data-f="decidedDate"]', "2026-11-02");
    await click("#dv-save");
    check("an approval with unsigned hold points is refused and nothing is stored", (await ev(`DV.d.inputs.wir.length`)) === 0);
    await shot("dv_2_wir_refused");
    const signed = await ev(`document.querySelector('#dv-rows textarea[data-f="holdPoints"]').value.split("\\n").map(l => l.split(";")[0].trim() + " ; Eng. A ; 2026-11-02").join("\\n")`);
    await setVal('#dv-rows textarea[data-f="holdPoints"]', signed);
    await click("#dv-save");
    check("once every hold point is signed the approval is stored", (await ev(`DV.d.inputs.wir.length`)) === 1 && (await ev(`DV.d.inputs.wir[0].status`)) === "approved");
    check("the computed table shows the hold points signed", (await text("#dv-body")).includes("/"));
    await shot("dv_3_wir_saved");

    // ---- soil report
    await click('[data-dvsec="geo"]');
    await setVal("#dv-g-ref", "GR-1"); await setVal("#dv-g-date", "2026-05-01"); await setVal("#dv-g-q", "120"); await setVal("#dv-g-gw", "3.5"); await setVal("#dv-g-rec", "isolated");
    await setVal("#dv-g-layers", "0 ; 3 ; sand ; loose\n3 ; 8 ; clay ;");
    await click("#dv-g-save");
    const geo = await text("#dv-body");
    check("the soil report is stored and a bearing conflict with the structural input is reported", (await ev(`DV.d.inputs.geotech.bearingKpa`)) === 120 && /conflict/i.test(geo), geo.slice(0, 160));
    await shot("dv_4_geotech");

    // ---- payment certificate on the project's own BOQ
    await click('[data-dvsec="ipc"]');
    const code = await ev(`DV.ipc.boq_items[0].code`);
    await setVal("#dv-i-cur", "SAR"); await setVal("#dv-i-ret", "10"); await setVal("#dv-i-vat", "15");
    await click("#dv-ipc-add");
    await setVal('#dv-ipc [data-f="date"]', "2026-12-01"); await setVal('#dv-ipc [data-f="lines"]', code + " ; 1");
    await click("#dv-i-save");
    check("a certificate is stored and valued from the BOQ", (await ev(`DV.d.inputs.ipc.certificates.length`)) === 1 && (await ev(`DV.ipc.report.certificates[0].lines.length`)) === 1, code);
    await shot("dv_5_ipc");
    await click("#dv-ipc-add"); await setVal('#dv-ipc tr[data-r="1"] [data-f="date"]', "2026-12-15"); await setVal('#dv-ipc tr[data-r="1"] [data-f="lines"]', code + " ; 0.5");
    await click("#dv-i-save");
    check("a quantity that goes down between certificates is refused", (await ev(`DV.d.inputs.ipc.certificates.length`)) === 1);

    // ---- contract draft
    await click('[data-dvsec="contract"]');
    await setVal("#dv-c-val", "1000000"); await setVal("#dv-c-cur", "SAR"); await setVal("#dv-c-ret", "10");
    await setVal("#dv-c-parties", "Owner ; A ;;\nContractor ; B ;;");
    await click("#dv-c-save");
    const draft = await ev(`call("contract_draft", { lang: "en" }).then(r => ({ draft: r.html.includes("DRAFT"), missing: r.missing }))`);
    check("the draft contract is marked a draft and names what is missing", draft.draft && draft.missing.includes("Warranty") && !draft.missing.includes("Contract value"), draft.missing.join(","));

    // ---- the AS-BUILT set carries the room plan with the open snags and RFIs
    const ab = await ev(`call("asbuilt_sheets", { lang: "en" }).then(r => ({ plan: r.html.includes("Open snags and RFIs on the plan"), rooms: (r.html.match(/<rect x=/g) || []).length, tail: r.html.slice(-300) }))`);
    check("the AS-BUILT set holds the snag / RFI plan with its rooms", ab.plan && ab.rooms > 0, JSON.stringify(ab));

    // ---- full dossier, written to disk by the real export
    await click('[data-dvsec="pre"]');
    await setVal("#dv-pe", "Eng. A"); await setVal("#dv-pl", "L-1"); await setVal("#dv-pb", "Council"); await setVal("#dv-pn", "P-1"); await setVal("#dv-pr", "Rev A");
    await click("#dv-dossier");
    await sleep(2500);
    const out = await text("#dv-dossier-out");
    const zips = fs.readdirSync(D).filter(f => f.endsWith(".zip"));
    check("the dossier ZIP was written", zips.length === 1, zips.join(","));
    if (zips.length) {
      const bytes = fs.readFileSync(`${D}\\${zips[0]}`);
      const names = bytes.toString("latin1");
      for (const f of ["00_Index/manifest.json", "01_Survey_Geotech/", "02_IFC_Drawings/", "04_Tender_BOQ/", "05_Permits/", "06_Supervision_QA_QC/", "07_Commercial_IPC/", "08_CloseOut_AsBuilt/"]) check("dossier contains " + f, names.includes(f));
    }
    check("the page lists the dossier's file count", /files|ملف/.test(out), out.slice(0, 120));
    await shot("dv_6_dossier");
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
