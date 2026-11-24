//! Commands of the Deliverables panels: supervision registers, quantity-based payment certificates, the soil report, the draft
//! contract, the AS-BUILT sheet set and the full project dossier.
//!
//! Like the rest of the backend this file holds no engineering logic: it reads the project, calls `nnx-core` and returns what the
//! page draws or saves. Nothing is defaulted; whatever an output needs and the project does not have is reported by name.

use super::{App, utc_now};
use nnx_core::bundle::Entry;
use nnx_core::building;
use nnx_core::drawing::plan_svg;
use nnx_core::schedule::Date;
use nnx_core::supervision::{self, Context, SupervisionInputs, VoStatus};
use nnx_core::xlsx::{base64_decode, base64_encode};
use serde_json::{Value, json};

/// One file of the dossier before it is checked: its folder, its name and the bytes or the reason it could not be built.
type Part = (&'static str, String, Result<Vec<u8>, String>);

fn arabic(args: &Value) -> bool {
    args.get("lang").and_then(Value::as_str) != Some("en")
}

fn today() -> Option<Date> {
    Date::parse(utc_now().get(..10)?)
}

impl App {
    fn supervision_inputs(&self) -> Result<SupervisionInputs, String> {
        Ok(self.project().ok_or("choose a layout first")?.meta.supervision)
    }

    pub(super) fn supervision_report(&self, args: &Value) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let rooms: Option<Vec<String>> = p.brief.as_ref().map(|b| b.rooms.iter().map(|r| r.key.clone()).collect());
        let ctx = Context { today: today(), room_keys: rooms.as_deref(), baseline_finish: self.stored_schedule().0.map(|s| s.finish) };
        let ar = arabic(args);
        let r = supervision::report(&p.meta.supervision, &ctx, ar);
        Ok(json!({
            "html": r.html(&self.display_title(), ar),
            "xlsx_b64": base64_encode(&r.workbook(ar).to_bytes()),
            "report": r,
            "inputs": p.meta.supervision,
            "wir_stages": supervision::WIR_STAGES.iter().map(|s| json!({ "id": s.0, "en": s.1, "ar": s.2 })).collect::<Vec<_>>(),
            "mar_categories": supervision::MAR_CATEGORIES.iter().map(|s| json!({ "id": s.0, "en": s.1, "ar": s.2 })).collect::<Vec<_>>(),
            "closeout_items": supervision::CLOSEOUT_ITEMS.iter().map(|s| json!({ "id": s.0, "en": s.1, "ar": s.2 })).collect::<Vec<_>>(),
        }))
    }

    /// `{ section, value }`: replaces ONE section of the supervision inputs (`handover`, `mar`, `shopDrawings`, `wir`, `mir`, `lab`,
    /// `ncr`, `rfi`, `mom`, `variations`, `progress`, `snags`, `closeout`, `ipc`, `contract`, `geotech`). The whole value is validated
    /// before it is stored; a refusal changes nothing. One undo step per call.
    pub(super) fn supervision_set(&mut self, args: &Value) -> Result<Value, String> {
        let section = args.get("section").and_then(Value::as_str).ok_or("supervision: `section` is required")?.to_string();
        let value = args.get("value").cloned().ok_or("supervision: `value` is required")?;
        let mut cur = serde_json::to_value(self.supervision_inputs()?).map_err(|e| e.to_string())?;
        let obj = cur.as_object_mut().ok_or("supervision: inputs are not an object")?;
        if !obj.contains_key(&section) {
            let mut known: Vec<&str> = obj.keys().map(String::as_str).collect();
            known.sort_unstable();
            return Err(format!("supervision: '{section}' is not a section ({})", known.join(", ")));
        }
        obj.insert(section.clone(), value);
        let next: SupervisionInputs = serde_json::from_value(cur).map_err(|e| format!("supervision {section}: {e}"))?;
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        let changed = nnx_core::project::update(doc, &format!("Supervision: {section}"), |p| {
            p.meta.supervision = next.clone();
            Ok(())
        })?
        .changed;
        let mut v = self.supervision_report(args)?;
        v["changed"] = json!(changed);
        Ok(v)
    }

    /// `{ stage, lang? }`: the stage's procedural hold points, to start a work inspection request from.
    pub(super) fn wir_hold_points(&self, args: &Value) -> Result<Value, String> {
        let stage = args.get("stage").and_then(Value::as_str).ok_or("wir hold points: `stage` is required")?;
        let t = supervision::hold_templates(stage);
        if t.is_empty() {
            return Err(format!("wir hold points: '{stage}' is not a stage"));
        }
        let ar = arabic(args);
        Ok(json!({ "hold_points": t.iter().map(|(en, a)| json!({ "text": if ar { a } else { en }, "signedBy": null, "signedDate": null })).collect::<Vec<_>>() }))
    }

    fn ipc_context(&self) -> Result<(Vec<nnx_core::ipc::ContractLine>, Vec<nnx_core::ipc::ApprovedVo>), String> {
        let boq = nnx_core::ipc::lines_from_boq(&self.boq_now()?);
        let vos = self.supervision_inputs()?.variations.iter().filter(|v| v.status == VoStatus::Approved).filter_map(|v| v.cost_delta.map(|c| nnx_core::ipc::ApprovedVo { id: v.id.clone(), cost: c })).collect();
        Ok((boq, vos))
    }

    pub(super) fn ipc_report(&self, args: &Value) -> Result<Value, String> {
        let (boq, vos) = self.ipc_context()?;
        let inp = self.supervision_inputs()?.ipc;
        let r = nnx_core::ipc::compute(&inp, &boq, &vos);
        let ar = arabic(args);
        Ok(json!({ "html": r.html(&self.display_title(), ar), "xlsx_b64": base64_encode(&r.workbook(ar).to_bytes()), "report": r, "boq_items": boq, "approved_variations": vos, "inputs": inp }))
    }

    pub(super) fn geotech_report(&self, args: &Value) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let ar = arabic(args);
        let r = nnx_core::geotech::report(&p.meta.supervision.geotech, p.structure.bearing_kpa, ar);
        Ok(json!({ "html": r.html(&self.display_title(), ar), "xlsx_b64": base64_encode(&r.workbook(ar).to_bytes()), "report": r, "inputs": p.meta.supervision.geotech }))
    }

    pub(super) fn contract_draft(&self, args: &Value) -> Result<Value, String> {
        let inp = self.supervision_inputs()?.contract;
        let milestones: Vec<nnx_core::contract::MilestoneLine> = self
            .payment_now()
            .map(|r| r.milestones.iter().map(|m| nnx_core::contract::MilestoneLine { name: m.name.clone(), when: m.date.clone(), percent: m.pct, amount: m.amount }).collect())
            .unwrap_or_default();
        let d = nnx_core::contract::draft(&self.display_title(), &inp, &milestones, arabic(args));
        Ok(json!({ "html": d.html, "missing": d.missing, "name": "contract_draft.pdf" }))
    }

    /// The plan set re-issued as AS-BUILT: the printable plan set with an AS-BUILT stamp on every sheet, followed by the approved
    /// variation orders it must reflect and the measured deviations stored in the project. It states only what the project holds:
    /// without approved variations or measured deviations the page says there are none, it does not claim the building matches.
    /// The field redlines: each level's plan with the measured position of every surveyed element drawn against its design position
    /// (green within the tolerance, red beyond it, grey while no tolerance is entered). `None` while no row gives an x or y.
    fn redline_plans(&self, ar: bool) -> Option<String> {
        let p = self.project()?;
        let model = &self.doc().ok()?.model;
        let rep = nnx_core::asbuilt::compare(model, &p.meta.as_built);
        if rep.points.is_empty() {
            return None;
        }
        let opts = self.draw_opts(self.show_structure);
        let levels: Vec<Option<nnx_core::Id>> = {
            let l = building::levels(model);
            if l.is_empty() { vec![None] } else { l.into_iter().map(|(_, id)| Some(id)).collect() }
        };
        let mut out = format!("<section style=\"page-break-before:always;font-family:'Segoe UI',Tahoma,sans-serif\"><h2>{}</h2>", if ar { "تعديلات الموقع: المقاس مقابل التصميم" } else { "Field redlines: measured against design" });
        let mut drawn = 0;
        for (i, lv) in levels.iter().enumerate() {
            let Ok(svg) = plan_svg(model, *lv, &opts) else { continue };
            let marked = nnx_core::asbuilt::add_markers(&svg, model, *lv, &opts, &rep);
            if marked == svg {
                continue;
            }
            drawn += 1;
            out.push_str(&format!("<h3>{} {}</h3>{marked}", if ar { "الطابق" } else { "Level" }, i + 1));
        }
        if drawn == 0 {
            return None;
        }
        out.push_str(&format!("<p>{}</p><p>{}: {} · {}: {}</p>", if ar { "أخضر: ضمن التفاوت · أحمر: خارجه · رمادي: لا تفاوت مدخل" } else { "Green: within tolerance · red: beyond it · grey: no tolerance entered" }, if ar { "قياسات خارج التفاوت" } else { "Over tolerance" }, rep.over, if ar { "أسماء بلا عنصر" } else { "Names matching no element" }, if rep.unmatched.is_empty() { "—".to_string() } else { html_esc(&rep.unmatched.join(", ")) }));
        out.push_str("</section>");
        Some(out)
    }

    /// The rooms of every level with the unverified snags and open RFIs marked on them; `None` while the model has no room.
    pub(super) fn issues_plan(&self, ar: bool) -> Option<String> {
        use nnx_core::typed::{Level, Space, Typed};
        let model = &self.doc().ok()?.model;
        // Rooms grouped by the level they sit on; a room that names no level is drawn on a plan of its own, never merged into another level's.
        let mut groups: std::collections::BTreeMap<Option<nnx_core::model::Id>, Vec<supervision::PlanRoom>> = Default::default();
        for e in model.elements.values().filter(|e| e.kind == Space::KIND) {
            if let Ok(s) = e.typed::<Space>() {
                groups.entry(e.level_id).or_default().push(supervision::PlanRoom { key: s.requirement_key, x: s.x, y: s.y, w: s.w, h: s.h });
            }
        }
        let mut levels: Vec<(i64, supervision::PlanLevel)> = groups
            .into_iter()
            .map(|(lv, rooms)| {
                let info = lv.and_then(|id| model.elements.get(&id)).filter(|e| e.kind == Level::KIND).and_then(|e| e.typed::<Level>().ok());
                match info {
                    Some(l) => (l.index, supervision::PlanLevel { name: l.name, rooms }),
                    None => (i64::MAX, supervision::PlanLevel { name: if ar { "المخطط".into() } else { "Plan".into() }, rooms }),
                }
            })
            .collect();
        levels.sort_by_key(|l| l.0);
        let levels: Vec<_> = levels.into_iter().map(|l| l.1).collect();
        supervision::issues_plan_svg(&levels, &self.project()?.meta.supervision, ar)
    }

    pub(super) fn asbuilt_sheets(&self, args: &Value) -> Result<Value, String> {
        let ar = arabic(args);
        let t = |a: &'static str, e: &'static str| if ar { a } else { e };
        let plans = self.pdf_html(ar)?;
        let mut html = plans["html"].as_str().ok_or("as-built: the plan set was empty")?.to_string();
        let stamp = format!("<style>body::before{{content:'{}';position:fixed;top:8mm;inset-inline-end:10mm;border:3px solid #b91c1c;color:#b91c1c;padding:4px 14px;font:700 20px 'Segoe UI',sans-serif;letter-spacing:2px;z-index:9;background:rgba(255,255,255,.85)}}</style>", "AS-BUILT");
        let mut tail = String::new();
        let p = self.project().ok_or("choose a layout first")?;
        let vos: Vec<_> = p.meta.supervision.variations.iter().filter(|v| v.status == VoStatus::Approved).collect();
        tail.push_str(&format!("<section style=\"page-break-before:always;font-family:'Segoe UI',Tahoma,sans-serif\"><h2>{}</h2>", t("أوامر التغيير المعتمدة التي يجب أن تعكسها المخططات", "Approved variation orders these drawings must reflect")));
        if vos.is_empty() {
            tail.push_str(&format!("<p>{}</p>", t("لا أوامر تغيير معتمدة مسجلة.", "No approved variation orders are recorded.")));
        } else {
            tail.push_str("<table border=\"1\" cellpadding=\"4\" style=\"border-collapse:collapse;width:100%\"><tr><th>#</th><th>");
            tail.push_str(t("الوصف", "Description"));
            tail.push_str("</th><th>");
            tail.push_str(t("مرجع المالك", "Owner reference"));
            tail.push_str("</th></tr>");
            for v in vos {
                tail.push_str(&format!("<tr><td>{}</td><td>{}</td><td>{}</td></tr>", html_esc(&v.id), html_esc(&v.description), html_esc(v.source.as_deref().unwrap_or(""))));
            }
            tail.push_str("</table>");
        }
        let n = p.meta.as_built.rows.len();
        tail.push_str(&format!("<p>{}: {n}</p></section>", t("عدد القياسات الميدانية المسجلة", "Field measurements recorded")));
        if let Some(sec) = self.redline_plans(ar) {
            tail.push_str(&sec);
        }
        if let Some(svg) = self.issues_plan(ar) {
            tail.push_str(&format!("<section style=\"page-break-before:always;font-family:'Segoe UI',Tahoma,sans-serif\"><h2>{}</h2>{svg}</section>", t("مخطط الملاحظات والاستفسارات المفتوحة", "Open snags and RFIs on the plan")));
        }
        if let Some(i) = html.rfind("</body>") {
            html.insert_str(i, &tail);
        } else {
            html.push_str(&tail);
        }
        if let Some(i) = html.find("</head>") {
            html.insert_str(i, &stamp);
        } else {
            html.insert_str(0, &stamp);
        }
        Ok(json!({ "html": html, "name": "as_built.pdf", "variations": p.meta.supervision.variations.iter().filter(|v| v.status == VoStatus::Approved).count(), "measurements": n }))
    }

    /// The full project dossier as one ZIP in the numbered folders of `nnx_core::bundle::DOSSIER_FOLDERS`: `{ lang?, force? }`.
    /// Every part is built from what the project holds; a part that cannot be built (a missing input) is NOT silently dropped, it is
    /// listed in `skipped` with the reason, and the index lists exactly what is inside. `permit` (the permit-pack inputs) adds the
    /// stamped permit sheets; without it that part is skipped by name. Returns `{ zip_b64, name, paths, manifest, skipped }`.
    pub(super) fn dossier_export(&self, args: &Value) -> Result<Value, String> {
        let ar = arabic(args);
        let slug: String = {
            let s: String = self.display_title().chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).collect();
            let s: String = s.trim_matches('_').chars().take(60).collect();
            if s.is_empty() { "project".to_string() } else { s }
        };
        let mut parts: Vec<Part> = Vec::new();
        let mut add = |folder: &'static str, name: String, part: Result<Vec<u8>, String>| parts.push((folder, name, part));
        let html_of = |r: Result<Value, String>| r.and_then(|v| v["html"].as_str().map(|s| s.as_bytes().to_vec()).ok_or_else(|| "no document was produced".to_string()));
        let xlsx_of = |r: Result<Value, String>| r.and_then(|v| v["xlsx_b64"].as_str().ok_or_else(|| "no workbook was produced".to_string()).and_then(base64_decode));

        // 01 Survey and geotechnical.
        let geo = self.geotech_report(args);
        add("01_Survey_Geotech", format!("{slug}-geotechnical.html"), html_of(geo.clone()));
        add("01_Survey_Geotech", format!("{slug}-geotechnical.xlsx"), xlsx_of(geo));
        // 02 Drawings.
        add("02_IFC_Drawings", format!("{slug}-plans.html"), html_of(self.pdf_html(ar)));
        add("02_IFC_Drawings", format!("{slug}.dxf"), self.export_dxf(false, false, None).and_then(|v| v["dxf"].as_str().map(|s| s.as_bytes().to_vec()).ok_or_else(|| "no DXF was produced".to_string())));
        add("02_IFC_Drawings", format!("{slug}.ifc"), self.gate(args).and_then(|_| self.export_ifc(args)).and_then(|v| v["ifc"].as_str().map(|s| s.as_bytes().to_vec()).ok_or_else(|| "no IFC was produced".to_string())));
        // 03 Calculations.
        add("03_Calculations", format!("{slug}-structural-calculation.md"), self.structural_calc_report(&json!({ "lang": if ar { "ar" } else { "en" } })).and_then(|v| v["markdown"].as_str().map(|s| s.as_bytes().to_vec()).ok_or_else(|| "the structural calculation was not produced".to_string())));
        // 04 Tender and BOQ.
        let tender = self.tender_report_in(args);
        add("04_Tender_BOQ", format!("{slug}-tender.html"), html_of(tender.clone()));
        add("04_Tender_BOQ", format!("{slug}-tender.xlsx"), xlsx_of(tender));
        add("04_Tender_BOQ", format!("{slug}-boq.xlsx"), self.boq_now().map(|r| nnx_core::xlsx::boq_workbook(&r, ar).to_bytes()));
        add("04_Tender_BOQ", format!("{slug}-contract-draft.html"), html_of(self.contract_draft(args)));
        // 05 Permits.
        match args.get("permit") {
            Some(permit) => {
                let mut a = permit.clone();
                a["lang"] = json!(if ar { "ar" } else { "en" });
                add("05_Permits", format!("{slug}-permit-pack.html"), html_of(self.permit_pack(&a)));
            }
            None => add("05_Permits", format!("{slug}-permit-pack.html"), Err("the engineer and submission inputs (`permit`) were not given".into())),
        }
        // 06 Supervision.
        let sup = self.supervision_report(args);
        add("06_Supervision_QA_QC", format!("{slug}-supervision.html"), html_of(sup.clone()));
        add("06_Supervision_QA_QC", format!("{slug}-supervision.xlsx"), xlsx_of(sup));
        let qa = self.qaqc_report_in(args);
        add("06_Supervision_QA_QC", format!("{slug}-qaqc-checklists.html"), html_of(qa.clone()));
        add("06_Supervision_QA_QC", format!("{slug}-qaqc-checklists.xlsx"), xlsx_of(qa));
        // 07 Commercial.
        let ipc = self.ipc_report(args);
        add("07_Commercial_IPC", format!("{slug}-ipc.html"), html_of(ipc.clone()));
        add("07_Commercial_IPC", format!("{slug}-ipc.xlsx"), xlsx_of(ipc));
        add("07_Commercial_IPC", format!("{slug}-payment-schedule.csv"), self.payment_now().map(|r| r.csv().into_bytes()));
        // 08 Close-out.
        add("08_CloseOut_AsBuilt", format!("{slug}-as-built-sheets.html"), html_of(self.asbuilt_sheets(args)));
        let mut entries: Vec<Entry> = Vec::new();
        let mut skipped: Vec<Value> = Vec::new();
        for (folder, name, part) in parts {
            match part {
                Ok(b) if !b.is_empty() => entries.push(Entry { folder: folder.into(), name, bytes: b }),
                Ok(_) => skipped.push(json!({ "file": format!("{folder}/{name}"), "reason": "the output was empty" })),
                Err(e) => skipped.push(json!({ "file": format!("{folder}/{name}"), "reason": e })),
            }
        }
        let date: String = utc_now().chars().take(10).collect();
        if entries.is_empty() {
            return Err(format!("dossier: nothing could be built; {}", skipped.iter().map(|s| format!("{}: {}", s["file"].as_str().unwrap_or(""), s["reason"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("; ")));
        }
        let b = nnx_core::bundle::build_dossier(&self.display_title(), &date, entries)?;
        Ok(json!({ "zip_b64": base64_encode(&b.zip), "name": format!("{slug}-dossier.zip"), "paths": b.paths, "manifest": b.manifest, "skipped": skipped }))
    }
}

fn html_esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}
