//! Application logic behind the window: JSON commands in, JSON out. No UI code here, so it is tested directly.
//!
//! Every number the engines need comes from the user's input. Blank inputs stay unset and the
//! engines say what is missing; nothing here supplies a building-code, load, soil or productivity value.

use nnx_core::drawing::{DrawOpts, plan_svg};
use nnx_core::layout::{Adjacency, Brief, Rect, RoomReq, Side, Zone};
use nnx_core::qto::{self, QtoOpts};
use nnx_core::rules;
use nnx_core::execution::{Activity, BuildError, ExecutionInputs, ScheduleSetup};
use nnx_core::schedule::{self, Date};
use nnx_core::structural::{Loads, Soil, StructOpts};
use nnx_core::walls::{WallOpts, WindowOpts};
use crate::editing;
use crate::levels::{self, Building, BuildingPlan};
use nnx_core::building;
use nnx_core::persist::Settings;
use nnx_core::{Command, Document, Id, Origin};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub use nnx_core::project::{BriefIn, NanoArchProject, RoomIn, StructIn};

/// The application state. The project itself (metadata, brief, structure, MEP and BOQ inputs, room names, municipal
/// limits) lives only inside the document's model as the `Project` element, so undo, redo, saving and every view agree.
/// What is kept here is either input that exists before a document does, or view state that is not part of the project.
#[derive(Default)]
pub struct App {
    alternatives: Vec<Building>,
    plan: Option<BuildingPlan>,
    /// The inputs entered before a layout is chosen. Once a document exists the project element is the only copy.
    draft: Option<NanoArchProject>,
    /// Index of the level shown in the plan view.
    cur_level: usize,
    doc: Option<Document>,
    show_structure: bool,
    show_mep: bool,
    /// Draw revision clouds around what changed since the baseline.
    show_clouds: bool,
}

/// The buildable rectangle (plot minus setbacks) and the entrance side.
pub(crate) fn envelope_and_entry(b: &BriefIn) -> Result<(Rect, Side), String> {
    let width = (b.width_m - 2.0 * b.setback_side_m) * 1000.0;
    let length = (b.length_m - b.setback_front_m - b.setback_back_m) * 1000.0;
    if width <= 0.0 || length <= 0.0 {
        return Err("the setbacks leave no buildable area".into());
    }
    let round10 = |v: f64| ((v / 10.0).round() as i64) * 10;
    let entry = match b.entry.as_str() {
        "south" => Side::South,
        "north" => Side::North,
        "east" => Side::East,
        "west" => Side::West,
        other => return Err(format!("unknown entry side '{other}'")),
    };
    Ok((Rect { x: 0, y: 0, w: round10(width), h: round10(length) }, entry))
}

/// Room requirements and their display names.
pub(crate) fn room_reqs(rooms: &[&RoomIn]) -> Result<(Vec<RoomReq>, BTreeMap<String, String>), String> {
    let mut out = Vec::new();
    let mut labels = BTreeMap::new();
    for r in rooms {
        let zone = match r.zone.as_str() {
            "public" => Zone::Public,
            "private" => Zone::Private,
            "service" => Zone::Service,
            other => return Err(format!("room '{}': unknown zone '{other}'", r.key)),
        };
        // Display minimum widths are design defaults the user can change; they are not code values.
        let min_w = r.min_width_mm.unwrap_or(match r.func.as_str() {
            "bathroom" => 1500,
            // A shaft is sized by its area; this is only the narrowest the generator may make it.
            "lift" => 1000,
            _ => 2000,
        });
        out.push(RoomReq { key: r.key.clone(), func: r.func.clone(), target_area_m2: r.area, min_width_mm: min_w, zone, needs_light: r.light });
        labels.insert(r.key.clone(), r.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| r.key.clone()));
    }
    Ok((out, labels))
}

/// Kitchen next to dining, when both are on the level.
pub(crate) fn adjacency_of(rooms: &[RoomReq]) -> Vec<Adjacency> {
    let first = |f: &str| rooms.iter().find(|r| r.func == f).map(|r| r.key.clone());
    match (first("kitchen"), first("dining")) {
        (Some(k), Some(d)) => vec![Adjacency { a: k, b: d, weight: 2 }],
        _ => Vec::new(),
    }
}

/// `{room key: {x, y, w, h}}` in millimetres.
/// Declared scores of one alternative: every level is counted against the whole building envelope and the counts are added,
/// so a level with more rooms weighs more.
fn score_building(alt: &Building, plan: &levels::BuildingPlan, w: &nnx_core::scoring::ScoreWeights) -> Result<nnx_core::scoring::Scores, String> {
    let hubs = nnx_core::layout::Options::default().hub_funcs;
    let mut total = nnx_core::scoring::Tallies::default();
    for (layout, lp) in alt.levels.iter().zip(&plan.levels) {
        total.add(&nnx_core::scoring::tally_level(layout, &plan.merged.envelope, plan.merged.entry, &lp.brief.adjacency, &hubs));
    }
    nnx_core::scoring::scores_from(&total, w)
}

fn parse_locked(v: Option<&Value>) -> Result<std::collections::BTreeMap<String, nnx_core::layout::Rect>, String> {
    let mut out = std::collections::BTreeMap::new();
    for (k, r) in v.and_then(Value::as_object).into_iter().flatten() {
        let f = |n: &str| r.get(n).and_then(Value::as_i64).ok_or_else(|| format!("locked room '{k}': {n} must be a whole number of millimetres"));
        out.insert(k.clone(), nnx_core::layout::Rect { x: f("x")?, y: f("y")?, w: f("w")?, h: f("h")? });
    }
    Ok(out)
}

fn id_arg(args: &Value, key: &str) -> Result<nnx_core::Id, String> {
    args.get(key).and_then(Value::as_str).and_then(|s| s.parse().ok()).ok_or_else(|| format!("'{key}' must be an element id"))
}

fn struct_opts(s: &StructIn) -> StructOpts {
    StructOpts {
        floors: s.floors.unwrap_or(1).max(1),
        max_span_mm: s.max_span_mm.filter(|&m| m > 0),
        loads: match (s.dead_kpa, s.live_kpa) {
            (Some(d), Some(l)) => Some(Loads { dead_kpa: d, live_kpa: l, source: "entered by the user".into() }),
            _ => None,
        },
        soil: s.bearing_kpa.filter(|&b| b > 0.0).map(|b| Soil { bearing_kpa: b, source: "entered by the user".into() }),
        ..StructOpts::default()
    }
}

impl App {
    fn doc(&self) -> Result<&Document, String> {
        self.doc.as_ref().ok_or_else(|| "choose a layout first".to_string())
    }

    /// The project stored in the open model, if there is one.
    fn project(&self) -> Option<NanoArchProject> {
        self.doc.as_ref().and_then(|d| nnx_core::project::get(&d.model).ok().flatten())
    }

    fn title(&self) -> String {
        self.project().map(|p| p.meta.title).unwrap_or_default()
    }

    fn display_title(&self) -> String {
        let t = self.title();
        if t.trim().is_empty() { "NanoArch project".into() } else { t }
    }

    fn labels(&self) -> BTreeMap<String, String> {
        self.project().map(|p| p.labels).unwrap_or_default()
    }

    /// The structure options, from the structural inputs stored in the project.
    fn sopts(&self) -> Result<StructOpts, String> {
        self.project().map(|p| struct_opts(&p.structure)).ok_or_else(|| "choose a layout first".to_string())
    }

    /// The layout brief of the open project (plot, entrance, rooms of every level), rebuilt from its stored inputs.
    fn layout_brief(&self) -> Result<Brief, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let b = p.brief.ok_or("this project has no saved brief: the entrance side is unknown, so partitions cannot be moved")?;
        let n_levels = building::levels(&self.doc()?.model).len().max(1) as u32;
        Ok(levels::plan_building(&b, n_levels)?.merged)
    }

    /// Code check of the open model, as JSON for the page.
    fn sbc_json(&self) -> Value {
        match &self.doc {
            Some(d) => serde_json::to_value(nnx_core::sbc::check(&d.model)).unwrap_or(Value::Null),
            None => Value::Null,
        }
    }

    /// Levels of the open model as `(index, id, name)`, lowest first. Empty for a single-storey model.
    fn level_list(&self) -> Vec<(i64, nnx_core::Id, String)> {
        let Some(d) = &self.doc else { return Vec::new() };
        building::levels(&d.model).into_iter().map(|(i, id)| (i, id, d.model.get(&id).and_then(|e| e.typed::<nnx_core::typed::Level>().ok()).map(|l| l.name).unwrap_or_default())).collect()
    }

    fn levels_json(&self) -> Value {
        json!({ "list": self.level_list().iter().map(|(i, _, n)| json!({ "index": i, "name": n })).collect::<Vec<_>>(), "current": self.cur_level })
    }

    /// The level shown, or `None` for a single-storey model.
    fn current_level(&self) -> Option<nnx_core::Id> {
        let l = self.level_list();
        l.get(self.cur_level.min(l.len().saturating_sub(1))).map(|(_, id, _)| *id)
    }

    fn draw_opts(&self, show_structure: bool) -> DrawOpts {
        let mut title = self.title();
        if let Some((_, _, name)) = self.level_list().get(self.cur_level) {
            title = format!("{title} — {name}");
        }
        DrawOpts { title, labels: self.labels(), show_structure, show_mep: self.show_mep, blank_canvas: true, ..DrawOpts::default() }
    }

    fn render(&self) -> Result<String, String> {
        plan_svg(&self.doc()?.model, self.current_level(), &self.draw_opts(self.show_structure)).map(|s| self.with_clouds(s)).map_err(|e| e.to_string())
    }

    /// The plan with revision clouds around the elements changed since the baseline (when switched on and a baseline exists).
    fn with_clouds(&self, svg: String) -> String {
        if !self.show_clouds {
            return svg;
        }
        let (Some(doc), Some(base)) = (self.doc.as_ref(), self.project().and_then(|p| p.meta.baseline)) else { return svg };
        let ch = nnx_core::revisions::changes(&doc.model, &base);
        nnx_core::revisions::add_clouds(&svg, &doc.model, self.current_level(), &self.draw_opts(self.show_structure), &ch)
    }

    /// The revision-history table appended to an exported document.
    /// The code-check statement printed with the plan set: which rule layers ran, how many checks, every finding, and every rule that
    /// did NOT run with the parameter it lacks, so a sheet never reads as "complies" for what was not checked.
    fn with_compliance(&self, html: String, arabic: bool) -> String {
        let Ok(r) = self.check_rules(&json!({ "sourced": true })) else { return html };
        let t = |ar: &'static str, en: &'static str| if arabic { ar } else { en };
        let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let findings = r["findings"].as_array().cloned().unwrap_or_default();
        let skipped = r["skipped"].as_array().cloned().unwrap_or_default();
        let layers = r["layers"].as_array().map(|l| l.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; ")).unwrap_or_default();
        let frows: String = findings.iter().map(|f| format!("<tr><td>{}</td><td>{}</td><td>{}</td></tr>", esc(f["rule"].as_str().unwrap_or("")), esc(f["clause"].as_str().unwrap_or("")), esc(f["message"].as_str().unwrap_or("")))).collect();
        let srows: String = skipped.iter().map(|k| format!("<tr><td>{}</td><td>{}</td><td>{}</td></tr>", esc(k["rule"].as_str().unwrap_or("")), esc(k["param"].as_str().unwrap_or("")), k["elements"].as_array().map_or(0, Vec::len))).collect();
        let frag = format!(
            "<section class=\"sheet\" style=\"padding:8mm\"><h1>{}</h1><p>{}</p><p>{}: <bdi dir=\"ltr\">{}</bdi></p><p>{}: {} · {}: {} · {}: {}</p>             <h2>{}</h2><table><tr><th>{}</th><th>{}</th><th>{}</th></tr>{}</table><h2>{}</h2><table><tr><th>{}</th><th>{}</th><th>{}</th></tr>{}</table></section>",
            t("بيان فحص الكود", "Code-check statement"),
            t("فحص أولي لا يغني عن مهندس مرخّص ولا عن الجهة المعتمدة. «لم تُفحص» تعني أن القاعدة لم تعمل لنقص قيمة، ولا تعني الامتثال.", "A preliminary check; it does not replace a licensed engineer or the approving authority. \"Not run\" means the rule did not execute for a missing value; it does not mean compliance."),
            t("الحزم المستعملة", "Rule layers used"),
            esc(&layers),
            t("عدد الفحوص", "Checks run"),
            r["checked"],
            t("المخالفات", "Findings"),
            findings.len(),
            t("قواعد لم تُفحص", "Rules not run"),
            skipped.len(),
            t("المخالفات", "Findings"),
            t("القاعدة", "Rule"),
            t("البند", "Clause"),
            t("الرسالة", "Message"),
            if frows.is_empty() { format!("<tr><td colspan=\"3\">{}</td></tr>", t("لا شيء", "None")) } else { frows },
            t("قواعد لم تُفحص", "Rules not run"),
            t("القاعدة", "Rule"),
            t("القيمة الناقصة", "Missing value"),
            t("الغرف غير المفحوصة", "Rooms unchecked"),
            if srows.is_empty() { format!("<tr><td colspan=\"3\">{}</td></tr>", t("لا شيء", "None")) } else { srows },
        );
        match html.rfind("</body>") {
            Some(i) => format!("{}{}{}", &html[..i], frag, &html[i..]),
            None => format!("{html}{frag}"),
        }
    }

    fn with_history(&self, html: String, arabic: bool) -> String {
        let (Some(doc), Some(p)) = (self.doc.as_ref(), self.project()) else { return html };
        let ch = p.meta.baseline.as_ref().map(|b| nnx_core::revisions::changes(&doc.model, b)).unwrap_or_default();
        let frag = nnx_core::revisions::history_html(&p, &ch, arabic);
        match html.rfind("</body>") {
            Some(i) => format!("{}{}{}", &html[..i], frag, &html[i..]),
            None => format!("{html}{frag}"),
        }
    }

    /// The plan as JSON `svg`, or null when the model is empty (after undoing the whole generation).
    fn render_or_null(&self) -> Result<Value, String> {
        Ok(match plan_svg(&self.doc()?.model, self.current_level(), &self.draw_opts(self.show_structure)) {
            Ok(svg) => Value::String(self.with_clouds(svg)),
            Err(nnx_core::drawing::DrawError::NoSpaces) => Value::Null,
        })
    }

    pub fn handle(&mut self, cmd: &str, args: &Value) -> Result<Value, String> {
        match cmd {
            "generate" => self.generate(args),
            "select" => self.select(args),
            "svg" => {
                self.show_structure = args.get("show_structure").and_then(Value::as_bool).unwrap_or(false);
                Ok(json!({ "svg": self.render_or_null()?, "levels": self.levels_json() }))
            }
            "level" => {
                let i = args.get("index").and_then(Value::as_u64).ok_or("index is required")? as usize;
                if i >= self.level_list().len().max(1) {
                    return Err("no such level".into());
                }
                self.cur_level = i;
                Ok(json!({ "svg": self.render_or_null()?, "levels": self.levels_json(), "rooms": self.rooms_json() }))
            }
            "undo" | "redo" => {
                let doc = self.doc.as_mut().ok_or("nothing to undo")?;
                let changed = if cmd == "undo" { doc.undo() } else { doc.redo() };
                self.cur_level = self.cur_level.min(self.level_list().len().saturating_sub(1));
                Ok(json!({ "changed": changed, "svg": self.render_or_null()?, "levels": self.levels_json(), "rooms": self.rooms_json(), "settings": self.form_state(), "sbc": self.sbc_json() }))
            }
            "sync" => {
                let so = self.sopts()?;
                let doc = self.doc.as_mut().ok_or("choose a layout first")?;
                let r = levels::sync_all(doc, &so)?;
                Ok(json!({ "created": r.created, "removed": r.removed, "unchanged": r.unchanged, "warnings": r.warnings, "svg": self.render()? }))
            }
            "describe" => {
                let id = id_arg(args, "id")?;
                let mut d = editing::describe(self.doc()?, id)?;
                if d["kind"] == "Wall" {
                    d["locked"] = json!(self.project().is_some_and(|p| p.meta.locked_walls.contains(&id.to_string())));
                }
                Ok(d)
            }
            "move_partition" => {
                let (id, delta) = (id_arg(args, "wall")?, args.get("delta_mm").and_then(Value::as_i64).ok_or("delta_mm is required")?);
                self.guard_lock(&[id])?;
                let brief = self.layout_brief()?;
                let so = self.sopts()?;
                let doc = self.doc.as_mut().ok_or("choose a layout first")?;
                let points_before = doc.model.elements.values().filter(|e| e.kind == "MepPoint").count();
                let moved = editing::move_partition(doc, &brief, &so, id, delta)?;
                let points_after = doc.model.elements.values().filter(|e| e.kind == "MepPoint").count();
                Ok(json!({ "moved": moved, "mep_cleared": points_before - points_after, "svg": self.render_or_null()?, "rooms": self.rooms_json(), "sbc": self.sbc_json() }))
            }
            "resize_room" | "swap_rooms" | "delete_room" => {
                let ids: Vec<nnx_core::Id> = ["room", "a", "b", "into"].iter().filter_map(|k| id_arg(args, k).ok()).collect();
                self.guard_lock(&ids)?;
                let brief = self.layout_brief()?;
                let so = self.sopts()?;
                let doc = self.doc.as_mut().ok_or("choose a layout first")?;
                let mep_before = doc.model.elements.values().filter(|e| e.kind == "MepPoint").count();
                let mut out = json!({});
                match cmd {
                    "resize_room" => {
                        let edge = editing::Edge::parse(args.get("edge").and_then(Value::as_str).ok_or("edge is required")?)?;
                        let grow = args.get("grow_mm").and_then(Value::as_i64).ok_or("grow_mm is required")?;
                        out["moved"] = json!(editing::resize_room(doc, &brief, &so, id_arg(args, "room")?, edge, grow)?);
                    }
                    "swap_rooms" => editing::swap_rooms(doc, &brief, &so, id_arg(args, "a")?, id_arg(args, "b")?)?,
                    _ => editing::delete_room(doc, &brief, &so, id_arg(args, "room")?, id_arg(args, "into")?)?,
                }
                let mep_after = doc.model.elements.values().filter(|e| e.kind == "MepPoint").count();
                out["mep_cleared"] = json!(mep_before.saturating_sub(mep_after));
                out["svg"] = self.render_or_null()?;
                out["rooms"] = json!(self.rooms_json());
                out["sbc"] = self.sbc_json();
                Ok(out)
            }
            "set_opening" => {
                let id = id_arg(args, "id")?;
                self.guard_lock(&[id])?;
                let (width, offset) = (args.get("width").and_then(Value::as_i64), args.get("offset").and_then(Value::as_i64));
                let so = self.sopts()?;
                editing::set_opening(self.doc.as_mut().ok_or("choose a layout first")?, &so, id, width, offset)?;
                Ok(json!({ "svg": self.render_or_null()?, "sbc": self.sbc_json() }))
            }
            "delete_opening" => {
                let id = id_arg(args, "id")?;
                self.guard_lock(&[id])?;
                let so = self.sopts()?;
                editing::delete_opening(self.doc.as_mut().ok_or("choose a layout first")?, &so, id)?;
                Ok(json!({ "svg": self.render_or_null()?, "sbc": self.sbc_json() }))
            }
            "presets" => Ok(nnx_core::presets::presets_json()),
            "site_analyze" => Self::site_analyze(args),
            "plot_import_dxf" => Self::plot_import_dxf(args),
            "ifc_review" => Self::ifc_review(args),
            "entrances_check" => Self::entrances_check(args),
            "privacy_check" => Self::privacy_check(args),
            "sun_check" => Self::sun_check(args),
            "span_compare" => Self::span_compare(args),
            "wall_load" => Self::wall_load(args),
            "drift_check" => Self::drift_check(args),
            "deep_foundation_check" => Self::deep_foundation_check(args),
            "spalling_check" => Self::spalling_check(args),
            "lighting_plan" => Self::lighting_plan(args),
            "panel_schedule" => Self::panel_schedule(args),
            "cable_check" => Self::cable_check(args),
            "water_storage_check" => Self::water_storage_check(args),
            "insulation_options" => self.insulation_options(args),
            "electricity_bill" => Self::electricity_bill(args),
            "energy_certificate" => Self::energy_certificate(args),
            "fire_protection_check" => Self::fire_protection_check(args),
            "egress_check" => Self::egress_check(args),
            "access_check" => Self::access_check(args),
            "shear_core_check" => Self::shear_core_check(args),
            "savings_compare" => Self::savings_compare(args),
            "furniture_fit" => Self::furniture_fit(args),
            "wind_check" => Self::wind_check(args),
            "courtyard_check" => Self::courtyard_check(args),
            "room_shape_check" => Self::room_shape_check(args),
            "mezzanine_check" => Self::mezzanine_check(args),
            "rooftop_check" => Self::rooftop_check(args),
            "basement_check" => Self::basement_check(args),
            "parking_plan" => Self::parking_plan(args),
            "terrain_check" => Self::terrain_check(args),
            "plot_import_survey" => Self::plot_import_survey(args),
            "brief_clarify" => Self::brief_clarify(args),
            "areas_import" => Self::areas_import(args),
            "prefs_learn" => Self::prefs_learn(args),
            "templates" => Ok(json!({ "templates": nnx_core::templates::all() })),
            "analysis" => self.analysis(args),
            "materials" => Ok(nnx_core::materials::catalog_json()),
            "mepx" => self.mepx_report(),
            "mepx_set" => self.mepx_set(args),
            "smart_suggest" => Ok(json!({ "devices": nnx_core::smarthome::suggest(&self.doc()?.model) })),
            "mepx_svg" => self.mepx_svg(args),
            "structural" => self.structural_report(),
            "structural_set" => self.structural_set(args),
            "grid_apply" => self.grid_apply(),
            "merge_apply" => self.merge_apply(),
            "export_analytical" => self.export_analytical(args),
            "compliance" => self.compliance_report(),
            "compliance_set" => self.compliance_set(args),
            "malath_set" => self.malath_set(args),
            "baseline_set" => self.baseline_set(args),
            "set_clouds" => {
                self.show_clouds = args.get("show").and_then(Value::as_bool).unwrap_or(false);
                Ok(json!({ "svg": self.render_or_null()? }))
            }
            "certificate_html" => self.certificate(args),
            "permit_pack" => self.permit_pack(args),
            "signoff_form" => self.signoff_form(args),
            "permit_documents_check" => self.permit_documents_check(args),
            "room_finishes_schedule" => self.room_finishes_schedule(args),
            "wet_room_detail" => self.wet_room_detail(args),
            "ceiling_plan" => self.ceiling_plan(args),
            "axes_plan" => self.axes_plan(args),
            "stair_detail" => self.stair_detail(args),
            "site_fence_plan" => self.site_fence_plan(args),
            "number_audit" => self.number_audit(),
            "lock" => self.lock(args),
            "regenerate" => self.regenerate(args),
            "site_layout" => self.site_layout_cmd(),
            "furniture" => self.furniture_cmd(),
            "vertical" => self.vertical_cmd(),
            "expansion_check" => self.expansion_check(),
            "template_apply" => Self::template_apply(args),
            "site_plan" => self.site_plan(),
            "zone_preset" => self.zone_preset(args),
            "elevation" => self.elevation(args),
            "section" => self.section(args),
            "auto_views" => self.auto_views(),
            "clashes" => Ok(self.clash_json()),
            "clash_classes" => Ok(self.clash_classes(args)),
            "runs" => Ok(json!(self.runs_json())),
            "add_run" => self.add_run(args),
            "delete_run" => {
                let id = id_arg(args, "id")?;
                let doc = self.doc.as_mut().ok_or("choose a layout first")?;
                if doc.model.get(&id).is_none_or(|e| e.kind != "MepRun") {
                    return Err("that element is not a duct, pipe or drain".into());
                }
                doc.commit("delete run", nnx_core::Origin::User, vec![nnx_core::Command::Delete(id)]).map_err(|e| e.to_string())?;
                Ok(json!({ "runs": self.runs_json(), "clashes": self.clash_json() }))
            }
            "project" => Ok(json!({ "settings": self.form_state(), "sbc": self.sbc_json(), "plan_outdated": self.plan_outdated() })),
            "project_set" => self.project_set(args),
            "sbc_check" => Ok(self.sbc_json()),
            "status" => Ok(json!({ "has_doc": self.doc.is_some(), "dirty": self.is_dirty() })),
            "assumptions" => Ok(assumptions()),
            "qto" => self.qto(args),
            "qto_elements" => self.qto_elements(args),
            "bid_compare" => self.bid_compare(args),
            "material_requests" => self.material_requests(args),
            "set_phase" => self.set_phase(args),
            "add_feature" => self.add_feature(args),
            "features" => self.features_json(args),
            "mep" => self.mep(args),
            "sizing" => self.sizing(args),
            "structural_calc_report" => self.structural_calc_report(args),
            "rebar_drawings" => self.rebar_drawings(),
            "boq" => self.boq(args),
            "model3d" => self.model3d(),
            "sun_shadows" => self.sun_shadows(args),
            "finishes" => self.finishes_report(),
            "finishes_set" => self.finishes_set(args),
            "asbuilt" => self.asbuilt_report(),
            "asbuilt_set" => self.asbuilt_set(args),
            "asbuilt_svg" => self.asbuilt_svg(),
            "tour_export" => self.tour_export(args),
            "client_view_export" => self.client_view_export(args),
            "stl_export" => self.stl_export(args),
            "exec" => self.exec_state(),
            "cost" => self.cost_report(),
            "cost_set" => self.cost_set(args),
            "tender" => self.tender_report(args),
            "tender_set" => self.tender_set(args),
            "payment" => self.payment_report(),
            "payment_set" => self.payment_set(args),
            "change_log" => self.change_log(args),
            "site_photos" => self.site_photos_report(),
            "site_photos_set" => self.site_photos_set(args),
            "progress_payments" => self.progress_report(),
            "progress_payments_set" => self.progress_set(args),
            "procure" => self.procure_report(),
            "procure_set" => self.procure_set(args),
            "qaqc" => self.qaqc_report(args),
            "qaqc_set" => self.qaqc_set(args),
            "roi" => self.roi_report(),
            "roi_set" => self.roi_set(args),
            "outline" => self.outline_report(),
            "snapshots" => self.snapshots_report(),
            "snapshot_save" => self.snapshot_save(args),
            "snapshot_delete" => self.snapshot_delete(args),
            "snapshot_diff" => self.snapshot_diff(args),
            "snapshot_report" => self.snapshot_report(args),
            "snapshot_svg" => self.snapshot_svg(args),
            "overlays" => self.overlays_report(),
            "plugins" => self.plugins_report(),
            "plugin_review" => self.plugin_review(args),
            "plugin_add" => self.plugin_add(args),
            "plugin_toggle" => self.plugin_toggle(args),
            "plugin_remove" => self.plugin_remove(args),
            "plugin_template" => self.plugin_template(args),
            "overlay_review" => self.overlay_review(args),
            "overlay_add" => self.overlay_add(args),
            "overlay_toggle" => self.overlay_toggle(args),
            "overlay_remove" => self.overlay_remove(args),
            "overlay_apply_limits" => self.overlay_apply_limits(args),
            "municipalities" => self.municipalities_report(),
            "municipality_add" => self.municipality_add(args),
            "municipality_remove" => self.municipality_remove(args),
            "municipality_apply" => self.municipality_apply(args),
            "zoning_check" => self.zoning_check(args),
            "draft_frame" => self.draft_frame(),
            "draft_snap" => self.draft_snap(args),
            "draft_wall" => self.draft_wall(args),
            "draft_place" => self.draft_place(args),
            "draft_opening" => self.draft_opening(args),
            "draft_box" => self.draft_box(args),
            "draft_xform" => self.draft_xform(args),
            "draft_align" => self.draft_align(args),
            "draft_split" => self.draft_split(args),
            "draft_trim" => self.draft_trim(args),
            "draft_delete" => self.draft_delete(args),
            "draft_props" => self.draft_props(args),
            "draft_set" => self.draft_set(args),
            "project_blank" => self.project_blank(args),
            "draft_grid" => self.draft_grid(args),
            "draft_dimension" => self.draft_dimension(args),
            "draft_tag" => self.draft_tag(args),
            "draft_rooms" => self.draft_rooms(),
            "draft_separator" => self.draft_separator(args),
            "draft_offset" => self.draft_offset(args),
            "assemblies_get" => self.assemblies_get(),
            "assemblies_set" => self.assemblies_set(args),
            "assembly_assign" => self.assembly_assign(args),
            "precheck" => Ok(json!(nnx_core::precheck::run(&self.doc()?.model))),
            "export_ifc" => self.gate(args).and_then(|_| self.export_ifc(args)),
            "export_bundle" => self.gate(args).and_then(|_| self.export_bundle(args)),
            "export_dxf" => self.gate(args).and_then(|_| self.export_dxf(args.get("all").and_then(Value::as_bool).unwrap_or(false), args.get("views").and_then(Value::as_bool).unwrap_or(false), args.get("layer_style"))),
            "report_html" => {
                let arabic = args.get("lang").and_then(Value::as_str) != Some("en");
                self.report(args).map(|(html, _)| json!({ "html": self.with_history(html, arabic), "name": "engineering-report.pdf" }))
            }
            "report_xlsx" => self.report(args).map(|(_, b64)| json!({ "xlsx_b64": b64, "name": "engineering-report.xlsx" })),
            "pdf_html" => self.gate(args).and_then(|_| self.pdf_html(args.get("lang").and_then(Value::as_str) != Some("en"))),
            "template_keys" => Ok(json!(schedule::template_keys().iter().map(|(k, fixed)| json!({ "key": k, "fixed": fixed })).collect::<Vec<_>>())),
            "schedule" => self.schedule(args),
            "check_rules" => self.check_rules(args),
            other => Err(format!("unknown command '{other}'")),
        }
    }

    fn generate(&mut self, args: &Value) -> Result<Value, String> {
        let b: BriefIn = serde_json::from_value(args.get("brief").cloned().unwrap_or(Value::Null)).map_err(|e| format!("brief: {e}"))?;
        let s: StructIn = serde_json::from_value(args.get("struct").cloned().unwrap_or(json!({}))).map_err(|e| format!("structure inputs: {e}"))?;
        let mut plan = levels::plan_building(&b, s.floors.unwrap_or(1))?;
        let locked = parse_locked(args.get("locked"))?;
        let weights: nnx_core::scoring::ScoreWeights = match args.get("score_weights").filter(|v| !v.is_null()) {
            Some(v) => serde_json::from_value(v.clone()).map_err(|e| format!("score_weights: {e}"))?,
            None => Default::default(),
        };
        weights.validate()?;
        let mut alts = levels::generate_building_locked(&plan, b.seed.unwrap_or(1), &locked)?;
        // Ranked by the declared overall score (lighting, connectivity, privacy), best first; the generator cost breaks ties.
        let mut scored = alts.drain(..).map(|a| score_building(&a, &plan, &weights).map(|s| (a, s))).collect::<Result<Vec<_>, _>>()?;
        scored.sort_by(|x, y| nnx_core::scoring::rank_order((&x.1, x.0.levels[0].cost), (&y.1, y.0.levels[0].cost)));
        let (mut alts, mut all_scores): (Vec<Building>, Vec<nnx_core::scoring::Scores>) = scored.into_iter().unzip();
        // Rooms marked on a sketch become the first alternative; the generated ones follow for comparison.
        let from_sketch = match args.get("placement").filter(|p| p.as_object().is_some_and(|o| !o.is_empty())) {
            Some(p) => {
                // The front door marked on the sketch (`_entry`, envelope frame in mm) goes into the brief, so every rebuild of the walls keeps it.
                if let Some(e) = p.get("_entry") {
                    let f = |k: &str| e.get(k).and_then(Value::as_f64).ok_or_else(|| format!("the marked entry point has no '{k}'"));
                    plan.merged.entry_at = Some((f("x")?.round() as i64, f("y")?.round() as i64));
                }
                let placed = levels::placed_building(&plan, p, &alts[0])?;
                all_scores.insert(0, score_building(&placed, &plan, &weights)?);
                alts.insert(0, placed);
                true
            }
            None => false,
        };
        let so = struct_opts(&s);
        let mut draft = NanoArchProject {
            meta: serde_json::from_value(args.get("meta").cloned().unwrap_or(json!({}))).map_err(|e| format!("project details: {e}"))?,
            limits: serde_json::from_value(args.get("limits").cloned().unwrap_or(json!({}))).map_err(|e| format!("municipal limits: {e}"))?,
            labels: plan.labels.clone(),
            structure: s.clone(),
            brief: Some(b.clone()),
            ..NanoArchProject::default()
        };
        draft.meta.title = b.title.clone().unwrap_or_default();
        draft.check()?;
        let mut thumbs = Vec::new();
        for (i, alt) in alts.iter().enumerate() {
            let (doc, _) = levels::assemble(alt, &plan, &so, None)?;
            let opts = DrawOpts { title: String::new(), labels: plan.labels.clone(), px_per_m: 16.0, margin_px: 14.0, show_dims: false, show_structure: false, show_mep: false, blank_canvas: false };
            let ground = building::levels(&doc.model).first().map(|(_, id)| *id);
            let svg = plan_svg(&doc.model, ground, &opts).map_err(|e| e.to_string())?;
            let ground_layout = &alt.levels[0];
            thumbs.push(json!({ "index": i, "cost": (ground_layout.cost * 100.0).round() / 100.0, "reserve_m2": (ground_layout.reserve_m2 * 10.0).round() / 10.0, "from_sketch": from_sketch && i == 0, "scores": all_scores[i], "svg": svg }));
        }
        self.draft = Some(draft);
        self.alternatives = alts;
        self.plan = Some(plan);
        self.doc = None;
        self.cur_level = 0;
        Ok(json!({ "alternatives": thumbs }))
    }

    fn select(&mut self, args: &Value) -> Result<Value, String> {
        let i = args.get("index").and_then(Value::as_u64).ok_or("index is required")? as usize;
        let s: StructIn = serde_json::from_value(args.get("struct").cloned().unwrap_or(json!({}))).map_err(|e| e.to_string())?;
        let alt = self.alternatives.get(i).ok_or("no such alternative")?.clone();
        let plan = self.plan.as_ref().ok_or("generate first")?;
        let so = struct_opts(&s);
        // The project is created inside the generation step: undoing the whole generation removes it with the plan.
        let mut project = self.draft.clone().ok_or("generate first")?;
        project.structure = s.clone();
        let (doc, mut warnings) = levels::assemble(&alt, plan, &so, Some(&project))?;
        warnings.splice(0..0, plan.notes.iter().cloned());
        let n = alt.levels.len();
        if n > 1 && s.floors.unwrap_or(1) as usize != n {
            warnings.push(format!("the floors input ({}) differs from the {n} levels drawn: generate again", s.floors.unwrap_or(1)));
        }
        self.show_structure = s.show_structure;
        self.doc = Some(doc);
        self.cur_level = 0;
        Ok(json!({ "svg": self.render()?, "warnings": warnings, "rooms": self.rooms_json(), "levels": self.levels_json(), "sbc": self.sbc_json() }))
    }

    /// Computes MEP and insulation from the user's inputs. With `apply`, the generated points become model
    /// elements in one undoable step; `show` switches the overlay. Missing inputs are reported by name.
    fn mep(&mut self, args: &Value) -> Result<Value, String> {
        let inputs_json = args.get("inputs").cloned().unwrap_or(json!({}));
        let inputs: nnx_core::mep::MepInputs = serde_json::from_value(inputs_json).map_err(|e| format!("MEP inputs: {e}"))?;
        // The inputs are part of the project: stored through the command stack (a no-op when unchanged).
        nnx_core::project::update(self.doc.as_mut().ok_or("choose a layout first")?, "MEP inputs", |p| {
            p.mep = Some(inputs.clone());
            Ok(())
        })?;
        if let Some(show) = args.get("show").and_then(Value::as_bool) {
            self.show_mep = show;
        }
        let apply = args.get("apply").and_then(Value::as_bool).unwrap_or(false);
        let ext = self.project().map(|p| p.meta.mep_ext).unwrap_or_default();
        let (report, applied) = {
            let doc = self.doc.as_mut().ok_or("choose a layout first")?;
            let report = nnx_core::mep::compute(&doc.model, &inputs);
            // The smart-home bus and the greywater drains of the advanced pack are generated runs too: they are applied with the rest.
            let applied = if apply {
                let mut runs = report.runs.clone();
                runs.extend(nnx_core::smarthome::topology(&doc.model, &ext.smart).runs);
                runs.extend(nnx_core::greywater::design(&doc.model, &inputs, &report, &ext.grey).runs);
                Some(nnx_core::mep::apply_with_runs(doc, &report.points, &runs)?)
            } else {
                None
            };
            (report, applied)
        };
        Ok(json!({
            "report": report,
            "applied": applied.map(|(c, r)| json!({ "created": c, "removed": r })),
            "points": self.doc()?.model.elements.values().filter(|e| e.kind == "MepPoint").count(),
            // Every duct, pipe, drain and cable in the model against the structure, checked on each call: an applied route that hits a beam, column or footing is named here without a separate request.
            "structure_clashes": self.clash_json(),
            "svg": self.render_or_null()?,
        }))
    }

    /// The Bill of Quantities: every take-off item with the user's specification and rate, the materials the
    /// items consume, a specification schedule, and the exports (three CSV files and a printable document).
    /// The model as an IFC4 file (text) with the count of each product class written.
    /// Stops an export while the model contradicts itself (rooms on top of each other, openings on the same wall, a door that
    /// names a room that is not there). `force: true` exports anyway; the warnings never stop it.
    fn gate(&self, args: &Value) -> Result<(), String> {
        let Some(doc) = self.doc.as_ref() else { return Ok(()) };
        if args.get("force").and_then(Value::as_bool).unwrap_or(false) {
            return Ok(());
        }
        let report = nnx_core::precheck::run(&doc.model);
        if report.has_errors() {
            return Err(format!("export stopped, the model has errors (run the precheck, fix them, or export with force): {}", report.error_text()));
        }
        Ok(())
    }

    /// `{ profile? }`: `"standard"` (default) or `"revit"` (type objects, Pset/Qto sets; see `nnx_core::ifc::export_ifc_revit`).
    fn export_ifc(&self, args: &Value) -> Result<Value, String> {
        let revit = match args.get("profile").and_then(Value::as_str).map(str::trim) {
            None | Some("") | Some("standard") => false,
            Some("revit") => true,
            Some(other) => return Err(format!("export ifc: unknown profile '{other}' (standard or revit)")),
        };
        let opts = nnx_core::ifc::IfcOpts {
            project_name: self.display_title(),
            author: "NanoArch".into(),
            timestamp: utc_now(),
            labels: self.labels(),
        };
        let model = &self.doc()?.model;
        let out = if revit { nnx_core::ifc::export_ifc_revit(model, &opts)? } else { nnx_core::ifc::export_ifc(model, &opts)? };
        Ok(json!({ "ifc": out.text, "counts": out.counts, "profile": if revit { "revit" } else { "standard" } }))
    }

    /// One delivery ZIP with numbered folders and an index: `{ include: ["ifc", "dxf", "plan_html", "boq"], lang?, profile?, all?, views? }`.
    /// Nothing is chosen for the user (`include` is required). `boq` needs the BOQ inputs saved in the project, since an unpriced
    /// quantity workbook would be passed off as a priced one. Returns `{ zip_b64, name, paths, manifest }`.
    fn export_bundle(&self, args: &Value) -> Result<Value, String> {
        use nnx_core::bundle::Entry;
        let include: Vec<&str> = args
            .get("include")
            .and_then(Value::as_array)
            .ok_or("export bundle: `include` is required (a list of ifc, dxf, plan_html, boq)")?
            .iter()
            .map(|v| v.as_str().ok_or("export bundle: `include` must hold only names"))
            .collect::<Result<_, _>>()?;
        if include.is_empty() {
            return Err("export bundle: `include` is empty (choose at least one of ifc, dxf, plan_html, boq)".into());
        }
        let arabic = args.get("lang").and_then(Value::as_str) != Some("en");
        let text = |v: &Value, key: &str| v[key].as_str().map(|s| s.as_bytes().to_vec()).ok_or_else(|| format!("export bundle: the {key} output was empty"));
        let slug: String = {
            let s: String = self.display_title().chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).collect();
            s.trim_matches('_').chars().take(60).collect()
        };
        let slug = if slug.is_empty() { "project".to_string() } else { slug };
        let mut entries = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for name in include {
            if !seen.insert(name) {
                return Err(format!("export bundle: '{name}' is listed twice"));
            }
            match name {
                "ifc" => {
                    let r = self.export_ifc(args)?;
                    entries.push(Entry { folder: "01-models".into(), name: format!("{slug}.ifc"), bytes: text(&r, "ifc")? });
                }
                "dxf" => {
                    let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
                    let views = args.get("views").and_then(Value::as_bool).unwrap_or(false);
                    let r = self.export_dxf(all, views, args.get("layer_style"))?;
                    entries.push(Entry { folder: "02-drawings".into(), name: format!("{slug}.dxf"), bytes: text(&r, "dxf")? });
                }
                "plan_html" => {
                    let r = self.pdf_html(arabic)?;
                    entries.push(Entry { folder: "02-drawings".into(), name: format!("{slug}-plans.html"), bytes: text(&r, "html")? });
                }
                "boq" => {
                    if self.project().and_then(|p| p.boq).is_none() {
                        return Err("export bundle: `boq` needs the BOQ inputs (prices and rates) saved in the project; enter them first".into());
                    }
                    let r = self.boq_now()?;
                    entries.push(Entry { folder: "03-quantities".into(), name: format!("{slug}-boq.xlsx"), bytes: nnx_core::xlsx::boq_workbook(&r, arabic).to_bytes() });
                }
                other => return Err(format!("export bundle: unknown item '{other}' (ifc, dxf, plan_html, boq)")),
            }
        }
        let date: String = utc_now().chars().take(10).collect();
        let b = nnx_core::bundle::build(&self.display_title(), &date, entries)?;
        Ok(json!({ "zip_b64": nnx_core::xlsx::base64_encode(&b.zip), "name": format!("{slug}-delivery.zip"), "paths": b.paths, "manifest": b.manifest }))
    }

    /// The printable plan set (one A3 sheet per level) as HTML; the shell prints it to PDF.
    fn pdf_html(&self, arabic: bool) -> Result<Value, String> {
        let opts = nnx_core::printdoc::PrintOpts {
            title: self.display_title(),
            labels: self.labels(),
            arabic,
            show_structure: self.show_structure,
            show_mep: self.show_mep,
            date: utc_now().chars().take(10).collect(),
        };
        let html = self.with_history(self.with_compliance(nnx_core::printdoc::print_html(&self.doc()?.model, &opts)?, arabic), arabic);
        Ok(json!({ "html": html, "name": "plan.pdf" }))
    }

    /// The plan as a DXF file: the level on screen, or every level side by side when `all` is true. An optional `layer_style`
    /// (the office's layer names, colours and line types, with the document they come from) restyles the finished file.
    fn export_dxf(&self, all: bool, views: bool, layer_style: Option<&Value>) -> Result<Value, String> {
        let model = &self.doc()?.model;
        let levels: Vec<Option<nnx_core::Id>> = if all {
            let l = building::levels(model);
            if l.is_empty() { vec![None] } else { l.into_iter().map(|(_, id)| Some(id)).collect() }
        } else {
            vec![self.current_level()]
        };
        let opts = nnx_core::dxf::DxfOpts { title: self.display_title(), labels: self.labels(), mep: true };
        // With `views`, the four elevations and the sections A-A and B-B (through the middle of the building) follow the plans.
        let specs: Vec<nnx_core::elevation::ViewSpec> = if views {
            use nnx_core::elevation::{Side, ViewSpec, default_cut};
            let mut v: Vec<ViewSpec> = [Side::North, Side::South, Side::East, Side::West].into_iter().map(ViewSpec::Elevation).collect();
            v.extend(["A-A", "B-B"].into_iter().filter_map(|n| default_cut(model, n)).map(ViewSpec::Section));
            v
        } else {
            Vec::new()
        };
        let out = nnx_core::dxf::export_dxf_views(model, &levels, &specs, &self.view_opts(), &opts)?;
        if let Some(raw) = layer_style.filter(|v| !v.is_null()) {
            let style: nnx_core::layerstyle::LayerStyle = serde_json::from_value(raw.clone()).map_err(|e| format!("layer_style: {e}"))?;
            let styled = nnx_core::layerstyle::apply(&out, &style)?;
            return Ok(json!({ "dxf": styled.out.text, "counts": styled.out.counts, "levels": levels.len(), "views": specs.len(), "layer_style": { "name": style.name, "source": style.source, "applied": styled.applied } }));
        }
        Ok(json!({ "dxf": out.text, "counts": out.counts, "levels": levels.len(), "views": specs.len() }))
    }

    fn boq(&mut self, args: &Value) -> Result<Value, String> {
        let inputs = args.get("inputs").cloned().unwrap_or(json!({}));
        let lib: nnx_core::boq::BoqLib = serde_json::from_value(inputs).map_err(|e| format!("BOQ inputs: {e}"))?;
        nnx_core::boq::validate(&lib)?;
        nnx_core::project::update(self.doc.as_mut().ok_or("choose a layout first")?, "BOQ inputs", |p| {
            p.boq = Some(lib.clone());
            Ok(())
        })?;
        let arabic = args.get("lang").and_then(Value::as_str) != Some("en");
        let model = &self.doc()?.model;
        let mut items = qto::compute(model, &QtoOpts::default()).items;
        items.extend(self.extra_items(model));
        let r = nnx_core::boq::compute(&items, &lib);
        Ok(json!({ "report": r, "boq_csv": r.boq_csv(), "materials_csv": r.materials_csv(), "specs_csv": r.specs_csv(), "html": r.html(arabic), "xlsx_b64": nnx_core::xlsx::base64_encode(&nnx_core::xlsx::boq_workbook(&r, arabic).to_bytes()) }))
    }

    /// Insulation and MEP-point items for the quantity table, from the saved inputs and the model's points.
    fn extra_items(&self, model: &nnx_core::Model) -> Vec<nnx_core::qto::QtoItem> {
        self.extra_items_for_phase(model, None)
    }

    fn extra_items_for_phase(&self, model: &nnx_core::Model, phase: Option<&str>) -> Vec<nnx_core::qto::QtoItem> {
        let mut v = Vec::new();
        if let Some(inp) = self.project().and_then(|p| p.mep)
            && let Some(lib) = inp.insulation
        {
            v.extend(nnx_core::insulation::analyze_for_phase(model, &lib, phase).items);
        }
        v.extend(nnx_core::mep::point_items_for_phase(model, phase));
        if let Some(p) = self.project() {
            v.extend(nnx_core::finishes::items_for_phase(model, &p.meta.finishes, phase));
        }
        v
    }

    /// The structural schedules: column loads by tributary area, footings against the soil value, tie beams, concrete and
    /// the bar bending schedule. `inputs` are stored in the project (one undoable step, a no-op when unchanged); the soil
    /// bearing value is the one of the structural inputs.
    fn sizing(&mut self, args: &Value) -> Result<Value, String> {
        let inputs: nnx_core::sizing::SizingInputs = serde_json::from_value(args.get("inputs").cloned().unwrap_or(json!({}))).map_err(|e| format!("sizing inputs: {e}"))?;
        let stored = inputs.clone();
        nnx_core::project::update(self.doc.as_mut().ok_or("choose a layout first")?, "structural sizing inputs", |p| {
            p.sizing = Some(stored.clone());
            Ok(())
        })?;
        let soil = self.project().and_then(|p| p.structure.bearing_kpa);
        // The tie beams become model elements (one undoable step), so the 3D view, quantities and schedule can cite them.
        let loaded = inputs.with_floor_load(&self.project().map(|p| p.structure).unwrap_or_default());
        let first = nnx_core::sizing::compute(&self.doc()?.model, soil, &loaded);
        nnx_core::sizing::apply_tie_beams(self.doc.as_mut().ok_or("choose a layout first")?, &first)?;
        let r = nnx_core::sizing::compute(&self.doc()?.model, soil, &loaded);
        // The bar bending schedule with shapes, bends, type marks and the per-diameter summary. When it cannot be built the reason
        // comes back by name next to the report; the sizing itself is not hidden by it.
        let (bbs, bbs_error) = match nnx_core::bbs::build(&r, &loaded) {
            Ok(b) => (Some(b), None),
            Err(e) => (None, Some(e)),
        };
        Ok(json!({
            "report": r, "members_csv": r.members_csv(), "concrete_csv": r.concrete_csv(), "bbs_csv": r.bbs_csv(), "soil_kpa": soil,
            "bbs": bbs, "bbs_lines_csv": bbs.as_ref().map(|b| b.lines_csv()), "bbs_summary_csv": bbs.as_ref().map(|b| b.summary_csv()), "bbs_error": bbs_error,
        }))
    }

    /// Reviewable structural calculation report of the sizing inputs stored in the project: assumptions, then results, then what was
    /// not computed. `{ lang?: "ar"|"en" }` (default `en`). Read-only; run `sizing` first.
    fn structural_calc_report(&self, args: &Value) -> Result<Value, String> {
        let lang = args.get("lang").and_then(Value::as_str).unwrap_or("en");
        let p = self.project().ok_or("choose a layout first")?;
        let inputs = p.sizing.clone().ok_or("no structural sizing inputs yet: run `sizing` first")?;
        let loaded = inputs.with_floor_load(&p.structure);
        let rep = nnx_core::sizing::compute(&self.doc()?.model, p.structure.bearing_kpa, &loaded);
        let report = nnx_core::calcreport::build(&p.meta.title, &p.structure, &inputs, &rep);
        let markdown = report.markdown(lang)?;
        Ok(json!({ "calc_report": report, "markdown": markdown }))
    }

    /// Reinforcement detail drawings (DXF) of the sizing inputs stored in the project: sections and elevations of the columns,
    /// beams, tie beams and footings, with the bars of your templates or of the design. Read-only: it changes nothing. Members that
    /// cannot be drawn come back in `skipped` with the reason.
    fn rebar_drawings(&self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let inputs = p.sizing.clone().ok_or("no structural sizing inputs yet: run `sizing` first")?;
        let loaded = inputs.with_floor_load(&p.structure);
        let rep = nnx_core::sizing::compute(&self.doc()?.model, p.structure.bearing_kpa, &loaded);
        let d = nnx_core::rebardraw::build(&rep, &p.meta.title)?;
        Ok(json!({ "dxf": d.dxf, "sheets": d.sheets, "skipped": d.skipped, "notes": d.notes, "counts": d.counts }))
    }

    /// The construction phases an element can carry. Nothing is assumed: an element with no phase stays unset.
    const PHASES: [&'static str; 6] = nnx_core::qto::PHASES;

    /// Gives rooms a construction phase (`phases`: room key -> phase). Walls take it from the rooms on their sides (a wall between an
    /// existing and a new room is new; two different phases that are not that pair make it `UNVERIFIED`), doors and windows from their
    /// wall. One undoable step. Rooms not named keep whatever they had.
    fn set_phase(&mut self, args: &Value) -> Result<Value, String> {
        let map = args.get("phases").and_then(Value::as_object).ok_or("phases (room key -> phase) is required")?;
        let mut by_room: BTreeMap<String, String> = BTreeMap::new();
        for (k, v) in map {
            let p = v.as_str().ok_or_else(|| format!("the phase of '{k}' must be text"))?;
            if !Self::PHASES.contains(&p) {
                return Err(format!("'{p}' is not a phase: use one of {}", Self::PHASES.join(", ")));
            }
            by_room.insert(k.clone(), p.to_string());
        }
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        let phase_of = |key: &str| by_room.get(key).or_else(|| by_room.get(key.split('@').next().unwrap_or(""))).cloned();
        let mut cmds: Vec<Command> = Vec::new();
        let mut wall_phase: BTreeMap<Id, String> = BTreeMap::new();
        // (level, rectangle, phase) of every room that has a phase: structural members take theirs from the rooms they stand in or touch.
        let mut space_phase: Vec<(Option<Id>, [i64; 4], String)> = Vec::new();
        let set = |id: Id, p: &str| Command::SetProp { id, key: "phase".into(), value: Some(json!(p)) };
        for e in doc.model.elements.values() {
            match e.kind.as_str() {
                "Space" => {
                    if let Ok(s) = e.typed::<nnx_core::typed::Space>()
                        && let Some(p) = phase_of(&s.requirement_key)
                    {
                        cmds.push(set(e.id, &p));
                        space_phase.push((e.level_id, [s.x, s.y, s.x + s.w, s.y + s.h], p));
                    }
                }
                "Wall" => {
                    let Ok(w) = e.typed::<nnx_core::typed::Wall>() else { continue };
                    let (a, b) = (phase_of(&w.room_a), w.room_b.as_deref().and_then(phase_of));
                    let p = match (a, b, w.room_b.is_some()) {
                        (Some(a), Some(b), _) if a == b => Some(a),
                        (Some(a), Some(b), _) if (a == "NEW" && b == "EXISTING") || (a == "EXISTING" && b == "NEW") => Some("NEW".to_string()),
                        (Some(_), Some(_), _) => Some("UNVERIFIED".to_string()),
                        (Some(a), None, false) => Some(a),
                        (None, Some(b), _) => Some(b),
                        (Some(a), None, true) => Some(a),
                        (None, None, _) => None,
                    };
                    if let Some(p) = p {
                        wall_phase.insert(e.id, p.clone());
                        cmds.push(set(e.id, &p));
                    }
                }
                _ => {}
            }
        }
        for e in doc.model.elements.values().filter(|e| e.kind == "Door" || e.kind == "Window") {
            if let Some(p) = e.host_id.and_then(|h| wall_phase.get(&h)) {
                cmds.push(set(e.id, p));
            }
        }
        // Columns, footings, beams, tie beams and slabs. A point takes the phase of the rooms of its level that contain it or lie within
        // 300 mm of it (the width of a wall): one phase gives that phase; new and existing together give NEW (as for walls); anything else
        // UNVERIFIED; no room near it leaves the member without a phase. A slab takes the phase of its own room.
        const NEAR_MM: i64 = 300;
        let at = |level: Option<Id>, x: i64, y: i64| -> Option<String> {
            let mut found: Vec<&str> = Vec::new();
            for (l, r, p) in &space_phase {
                if *l == level && x >= r[0] - NEAR_MM && x <= r[2] + NEAR_MM && y >= r[1] - NEAR_MM && y <= r[3] + NEAR_MM && !found.contains(&p.as_str()) {
                    found.push(p);
                }
            }
            match found.as_slice() {
                [] => None,
                [one] => Some((*one).to_string()),
                two if two.len() == 2 && two.contains(&"NEW") && two.contains(&"EXISTING") => Some("NEW".to_string()),
                _ => Some("UNVERIFIED".to_string()),
            }
        };
        for e in doc.model.elements.values() {
            let p = match e.kind.as_str() {
                "Column" => e.typed::<nnx_core::typed::Column>().ok().and_then(|c| at(e.level_id, c.x, c.y)),
                "Footing" => e.typed::<nnx_core::typed::Footing>().ok().and_then(|f| at(e.level_id, f.x, f.y)),
                "Beam" => e.typed::<nnx_core::typed::Beam>().ok().and_then(|b| at(e.level_id, (b.x1 + b.x2) / 2, (b.y1 + b.y2) / 2)),
                "TieBeam" => e.typed::<nnx_core::typed::TieBeam>().ok().and_then(|b| at(e.level_id, (b.x1 + b.x2) / 2, (b.y1 + b.y2) / 2)),
                "Slab" => e.typed::<nnx_core::typed::Slab>().ok().and_then(|s| phase_of(&s.room)),
                // A MEP point or run takes the phase of the room it names, else of the rooms around its position (a run: its midpoint;
                // between new and existing rooms it is NEW, as for walls).
                "MepPoint" => e.typed::<nnx_core::typed::MepPoint>().ok().and_then(|m| phase_of(&m.room).or_else(|| at(e.level_id, m.x, m.y))),
                "MepRun" => e.typed::<nnx_core::typed::MepRun>().ok().and_then(|r| r.room.as_deref().and_then(phase_of).or_else(|| at(e.level_id, (r.x1 + r.x2) / 2, (r.y1 + r.y2) / 2))),
                _ => None,
            };
            if let Some(p) = p {
                cmds.push(set(e.id, &p));
            }
        }
        if cmds.is_empty() {
            return Err("no room in the plan has one of these keys".into());
        }
        let n = cmds.len();
        doc.commit("Set construction phases", Origin::User, cmds).map_err(|e| e.to_string())?;
        Ok(json!({ "elements": n, "svg": self.render_or_null()? }))
    }

    /// Adds a named feature of the design (`role`: expansion joint, transfer beam, light well, shaft, lift, entrance landing or path ...)
    /// at the geometry the drawing gives, on one level (`level`, 0 = ground). One undoable step. Only what is stated is stored.
    fn add_feature(&mut self, args: &Value) -> Result<Value, String> {
        let num = |k: &str| args.get(k).and_then(Value::as_f64).map(|v| v.round() as i64);
        let text = |k: &str| args.get(k).and_then(Value::as_str).map(String::from);
        let role = text("role").ok_or("role is required")?;
        let shape = text("shape").ok_or("shape (line, rect or point) is required")?;
        let f = nnx_core::typed::Feature {
            role,
            shape,
            x1: num("x1").ok_or("x1 is required")?,
            y1: num("y1").ok_or("y1 is required")?,
            x2: num("x2").unwrap_or(0),
            y2: num("y2").unwrap_or(0),
            width: num("width").unwrap_or(0),
            name: text("name"),
            note: text("note"),
            room: text("room").filter(|r| !r.is_empty()),
            risers: num("risers").map(|v| v as u32),
            riser_mm: num("riserMm"),
            extra: Default::default(),
        };
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        let level = args.get("level").and_then(Value::as_u64).unwrap_or(0) as usize;
        let lid = building::levels(&doc.model).get(level).map(|l| l.1).ok_or("there is no such level")?;
        let mut el = nnx_core::typed::element_of(&f, nnx_core::Source::User);
        el.level_id = Some(lid);
        nnx_core::typed::Typed::validate(&f, &doc.model, &el)?;
        if let Some(p) = text("phase") {
            if !Self::PHASES.contains(&p.as_str()) {
                return Err(format!("'{p}' is not a phase"));
            }
            el.props.insert("phase".into(), json!(p));
        }
        let id = el.id;
        doc.commit("Add feature", Origin::User, vec![Command::Create(el)]).map_err(|e| e.to_string())?;
        Ok(json!({ "id": id.to_string(), "svg": self.render_or_null()? }))
    }

    fn features_json(&self, args: &Value) -> Result<Value, String> {
        let doc = self.doc()?;
        let lv = building::levels(&doc.model);
        let only = args.get("level").and_then(Value::as_u64).map(|l| l as usize);
        let list: Vec<Value> = doc
            .model
            .elements
            .values()
            .filter(|e| e.kind == "Feature")
            .filter_map(|e| {
                let level = lv.iter().position(|(_, id)| Some(*id) == e.level_id)?;
                if only.is_some_and(|o| o != level) {
                    return None;
                }
                let f = e.typed::<nnx_core::typed::Feature>().ok()?;
                Some(json!({ "id": e.id.to_string(), "level": level, "role": f.role, "shape": f.shape, "name": f.name, "x1": f.x1, "y1": f.y1, "x2": f.x2, "y2": f.y2, "width": f.width, "room": f.room, "risers": f.risers, "riserMm": f.riser_mm, "phase": e.props.get("phase") }))
            })
            .collect();
        Ok(json!({ "features": list }))
    }

    fn qto(&self, args: &Value) -> Result<Value, String> {
        let phase = args.get("phase").and_then(Value::as_str).filter(|p| !p.is_empty()).map(String::from);
        if let Some(p) = phase.as_ref().filter(|p| *p != "UNSET" && !Self::PHASES.contains(&p.as_str())) {
            return Err(format!("'{p}' is not a phase"));
        }
        let mut q = qto::compute(&self.doc()?.model, &QtoOpts { phase: phase.clone(), ..QtoOpts::default() });
        q.items.extend(self.extra_items_for_phase(&self.doc()?.model, phase.as_deref()));
        let items: Vec<Value> = q
            .items
            .iter()
            .map(|i| json!({ "code": i.code, "description": i.description, "unit": i.unit.label(), "quantity": i.quantity, "sources": i.sources.len() }))
            .collect();
        Ok(json!({ "items": items, "warnings": q.warnings, "csv": q.to_csv() }))
    }

    /// The quantities of the model grouped by the element they were measured on: `{ phase?, element? }`. Read-only.
    /// Contractor bids compared on the project's own BOQ (read-only): `{ bids: [{ contractor, source, date, currency, rates: { CODE: rate }, markups? }] }`.
    /// Rates, mark-ups, dates and sources are the bidders' own as entered; nothing is stored and nothing is filled in.
    fn bid_compare(&self, args: &Value) -> Result<Value, String> {
        let bids: Vec<nnx_core::bids::Bid> = serde_json::from_value(args.get("bids").cloned().ok_or("bids is required")?).map_err(|e| format!("bids: {e}"))?;
        let boq = self.boq_now()?;
        let cmp = nnx_core::bids::compare(&boq, &bids)?;
        Ok(json!({ "comparison": cmp, "csv": cmp.csv(), "name": "bid-comparison" }))
    }

    /// Material requests by construction stage (read-only): `{ inputs?: { as_of?, buffer_days?, lead: { material: { days, source } } } }`.
    /// The materials are the BOQ's own (the project's stored consumption factors) split by the stages of the stored schedule; lead
    /// times are the user's, each with its source, and nothing is stored.
    fn material_requests(&self, args: &Value) -> Result<Value, String> {
        let inputs: nnx_core::matreq::MatReqInputs = match args.get("inputs").filter(|v| !v.is_null()) {
            Some(v) => serde_json::from_value(v.clone()).map_err(|e| format!("inputs: {e}"))?,
            None => Default::default(),
        };
        let (sched, why) = self.stored_schedule();
        let sched = sched.ok_or_else(|| why.unwrap_or_else(|| "the schedule is not available".into()))?;
        let model = &self.doc()?.model;
        let mut items = qto::compute(model, &QtoOpts::default()).items;
        items.extend(self.extra_items(model));
        let lib = self.project().and_then(|p| p.boq).unwrap_or_default();
        let today = Date::parse(&utc_now().chars().take(10).collect::<String>());
        let r = nnx_core::matreq::build(&items, &lib, &sched, &inputs, today)?;
        Ok(json!({ "report": r, "csv": r.csv(), "name": "material-requests" }))
    }

    /// Each row is one element's share of an item; the shares of one code add up to that item (checked in the tests).
    /// Items other modules add (MEP, insulation, finishes) have no per-element breakdown and are named in `not_linked`.
    fn qto_elements(&self, args: &Value) -> Result<Value, String> {
        let phase = args.get("phase").and_then(Value::as_str).filter(|p| !p.is_empty()).map(String::from);
        if let Some(p) = phase.as_ref().filter(|p| *p != "UNSET" && !Self::PHASES.contains(&p.as_str())) {
            return Err(format!("'{p}' is not a phase"));
        }
        let only = match args.get("element").filter(|v| !v.is_null()) {
            None => None,
            Some(v) => Some(v.as_str().and_then(|s| s.parse::<nnx_core::Id>().ok()).ok_or("element must be an element id")?),
        };
        let doc = self.doc()?;
        if let Some(id) = only
            && doc.model.get(&id).is_none()
        {
            return Err(format!("element {id} is not in the model"));
        }
        let q = qto::compute(&doc.model, &QtoOpts { phase: phase.clone(), ..QtoOpts::default() });
        let extra = self.extra_items_for_phase(&doc.model, phase.as_deref());
        let mut by_element: std::collections::BTreeMap<nnx_core::Id, Vec<&nnx_core::qto::ElementQty>> = std::collections::BTreeMap::new();
        for r in &q.element_rows {
            if only.is_none_or(|id| id == r.element) {
                by_element.entry(r.element).or_default().push(r);
            }
        }
        let elements: Vec<Value> = by_element
            .iter()
            .filter_map(|(id, rows)| {
                let e = doc.model.get(id)?;
                let quantities: Vec<Value> = rows.iter().map(|r| json!({ "code": r.code, "unit": r.unit.label(), "quantity": r.quantity })).collect();
                Some(json!({ "id": id.to_string(), "kind": e.kind, "level_id": e.level_id.map(|l| l.to_string()), "phase": e.props.get("phase"), "quantities": quantities }))
            })
            .collect();
        let mut not_linked: Vec<String> = q.unlinked_codes().into_iter().map(String::from).collect();
        not_linked.extend(extra.iter().map(|i| i.code.clone()));
        Ok(json!({ "elements": elements, "not_linked": not_linked, "warnings": q.warnings }))
    }

    /// The take-off the schedule is built from: the model's quantities, insulation, MEP devices and runs, and the concrete and steel of
    /// the structural schedules. A finish has no productivity input, so it is a quantity and a BOQ row, not a scheduled activity.
    fn schedule_qto(&self) -> Result<qto::Qto, String> {
        self.schedule_qto_for(None)
    }

    /// The same take-off restricted to one construction phase. The concrete and steel of the structural schedules are not split by phase
    /// (the sizing schedules compute from the whole model and are not split by phase), so they are left out of a phase take-off and the schedule says so.
    fn schedule_qto_for(&self, phase: Option<&str>) -> Result<qto::Qto, String> {
        let doc = self.doc()?;
        let mut q = qto::compute(&doc.model, &QtoOpts { phase: phase.map(String::from), ..QtoOpts::default() });
        q.items.extend(self.extra_items_for_phase(&doc.model, phase));
        q.items.retain(|i| !i.code.starts_with("FIN."));
        if phase.is_some() {
            return Ok(q);
        }
        if let Some(inputs) = self.project().and_then(|p| p.sizing.as_ref().map(|s| (s.with_floor_load(&p.structure), p.structure.bearing_kpa))) {
            q.items.extend(nnx_core::sizing::schedule_items(&nnx_core::sizing::compute(&doc.model, inputs.1, &inputs.0)));
        }
        Ok(q)
    }

    /// Computes the 4D schedule from the rates, start date and calendar sent, and keeps them in the project (one undoable step, a no-op
    /// when unchanged) so the payment, procurement and QA/QC features read the very same schedule.
    fn schedule(&mut self, args: &Value) -> Result<Value, String> {
        let setup: ScheduleSetup = serde_json::from_value(args.clone()).map_err(|e| format!("schedule inputs: {e}"))?;
        if setup.start.trim().is_empty() {
            return Err("enter the start date as YYYY-MM-DD".into());
        }
        // `phase`: a schedule of the work of one construction phase only. It is a view: the stored setup is not touched.
        let phase = args.get("phase").and_then(Value::as_str).filter(|p| !p.is_empty()).map(String::from);
        if let Some(p) = phase.as_ref().filter(|p| *p != "UNSET" && !Self::PHASES.contains(&p.as_str())) {
            return Err(format!("'{p}' is not a phase"));
        }
        let q = self.schedule_qto_for(phase.as_deref())?;
        let built = setup.build(&self.doc()?.model, &q);
        match built {
            Ok((s, cal)) => {
                let tasks: Vec<Value> = s
                    .tasks
                    .iter()
                    .map(|t| {
                        json!({
                            "id": t.id, "key": t.key, "name": t.name, "wbs": t.wbs, "phase": t.phase, "duration": t.duration, "es": t.es, "ef": t.ef,
                            "start": t.start.iso(), "finish": t.finish.iso(), "float": t.float, "critical": t.critical,
                            "quantity": t.quantity, "unit": t.unit.map(|u| u.label()),
                            "also": t.also.iter().map(|m| json!({ "label": m.label, "quantity": m.quantity, "unit": m.unit.label() })).collect::<Vec<_>>(),
                            "elements": t.elements.iter().map(|e| e.to_string()).collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                let mut warnings = s.warnings.clone();
                if let Some(p) = &phase {
                    warnings.push(format!("schedule of the {p} work only: the concrete and steel of the structural schedules are not split by phase (they are computed from the whole model), so they are not in it"));
                }
                let out = json!({
                    "ok": true, "phase": phase, "project_days": s.project_days, "finish": s.finish.iso(), "tasks": tasks, "warnings": warnings,
                    "svg": s.gantt_svg(&cal, &self.title()), "csv": s.to_csv(),
                    "calendar": (0..=s.project_days).map(|d| cal.nth_workday(d).iso()).collect::<Vec<_>>()
                });
                if self.project().is_some() && phase.is_none() {
                    self.exec_update("Schedule setup", |e| {
                        e.schedule = Some(setup.clone());
                        Ok(())
                    })?;
                }
                Ok(out)
            }
            Err(BuildError::MissingRates(keys)) => Ok(json!({ "ok": false, "missing": keys })),
            Err(BuildError::Invalid(why)) => Err(why),
        }
    }

    /// The execution inputs of the project, changed as ONE undoable step (nothing happens when the edit changes nothing).
    fn exec_update(&mut self, label: &str, edit: impl FnOnce(&mut ExecutionInputs) -> Result<(), String>) -> Result<bool, String> {
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        Ok(nnx_core::project::update(doc, label, |p| edit(&mut p.meta.execution))?.changed)
    }

    fn exec_inputs(&self) -> Result<ExecutionInputs, String> {
        Ok(self.project().ok_or("choose a layout first")?.meta.execution)
    }

    /// The activities of the schedule stored in the project, or why there are none yet.
    fn stored_activities(&self) -> (Option<Vec<Activity>>, Option<String>) {
        let (sched, why) = self.stored_schedule();
        (sched.map(|s| nnx_core::execution::activities(&s)), why)
    }

    /// The BOQ report of the project's stored library (every unpriced item stays listed).
    fn boq_now(&self) -> Result<nnx_core::boq::BoqReport, String> {
        let model = &self.doc()?.model;
        let mut items = qto::compute(model, &QtoOpts::default()).items;
        items.extend(self.extra_items(model));
        let lib = self.project().and_then(|p| p.boq).unwrap_or_default();
        Ok(nnx_core::boq::compute(&items, &lib))
    }

    fn cost_now(&self) -> Result<nnx_core::costtier::CostReport, String> {
        let model = &self.doc()?.model;
        let mut items = qto::compute(model, &QtoOpts::default()).items;
        items.extend(self.extra_items(model));
        Ok(nnx_core::costtier::compute(&items, &self.exec_inputs()?.cost))
    }

    /// Everything Milestone 6 stores, for the page to fill its forms from.
    fn exec_state(&self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        Ok(json!({
            "inputs": p.meta.execution,
            "tiers": nnx_core::costtier::Tier::ALL.iter().map(|t| json!({ "id": t.id(), "en": t.label().0, "ar": t.label().1, "phases": t.expected_phases().iter().map(|&i| nnx_core::schedule::PHASES[i]).collect::<Vec<_>>() })).collect::<Vec<_>>(),
            "phases": nnx_core::schedule::PHASES,
            "tasks": schedule::template_names().iter().map(|(k, n, p)| json!({ "key": k, "name": n, "phase": p })).collect::<Vec<_>>(),
            "stages": nnx_core::qaqc::STAGES.iter().map(|s| json!({ "id": s.id, "en": s.en, "ar": s.ar })).collect::<Vec<_>>(),
            "categories": nnx_core::procure::CATEGORIES,
            "building_type": p.meta.building_type,
        }))
    }

    fn cost_report(&self) -> Result<Value, String> {
        Ok(json!({ "report": self.cost_now()? }))
    }

    /// Stores the cost lines, the selected tier and the currency: ONE undoable edit.
    fn cost_set(&mut self, args: &Value) -> Result<Value, String> {
        let c: nnx_core::costtier::CostInputs = serde_json::from_value(args.get("cost").cloned().ok_or("cost is required")?).map_err(|e| format!("cost: {e}"))?;
        let changed = self.exec_update("Cost tiers", |e| {
            e.cost = c.clone();
            Ok(())
        })?;
        let mut v = self.cost_report()?;
        v["changed"] = json!(changed);
        Ok(v)
    }

    fn tender_pack(&self, arabic: bool) -> Result<(nnx_core::tender::TenderPack, bool), String> {
        let p = self.project().ok_or("choose a layout first")?;
        let model = &self.doc()?.model;
        let finishes = nnx_core::finishes::report(model, &p.meta.finishes, p.boq.as_ref());
        let payment = self.payment_now().ok();
        Ok((nnx_core::tender::pack(nnx_core::tender::TenderParts { project: &p, boq: self.boq_now()?, cost: self.cost_now()?, finishes, payment }), arabic))
    }

    /// The tender pack: the data, the printable document and the workbook. `lang` is `en` or `ar` (Arabic when absent).
    fn tender_report_in(&self, args: &Value) -> Result<Value, String> {
        let (pack, arabic) = self.tender_pack(args.get("lang").and_then(Value::as_str) != Some("en"))?;
        Ok(json!({ "pack": pack, "html": pack.html(arabic), "xlsx_b64": nnx_core::xlsx::base64_encode(&pack.workbook(arabic).to_bytes()), "name": "tender-pack" }))
    }

    fn tender_report(&self, args: &Value) -> Result<Value, String> {
        self.tender_report_in(args)
    }

    fn tender_set(&mut self, args: &Value) -> Result<Value, String> {
        let t: nnx_core::tender::TenderInputs = serde_json::from_value(args.get("tender").cloned().ok_or("tender is required")?).map_err(|e| format!("tender: {e}"))?;
        let changed = self.exec_update("Tender notes", |e| {
            e.tender = t.clone();
            Ok(())
        })?;
        let mut v = self.tender_report_in(args)?;
        v["changed"] = json!(changed);
        Ok(v)
    }

    fn payment_now(&self) -> Result<nnx_core::payment::PaymentReport, String> {
        let inputs = self.exec_inputs()?.payment;
        let (acts, _) = self.stored_activities();
        let boq = self.boq_now()?;
        let boq_total = boq.complete.then_some(boq.total);
        Ok(nnx_core::payment::compute(&inputs, acts.as_deref(), boq_total))
    }

    fn payment_report(&self) -> Result<Value, String> {
        let (acts, why) = self.stored_activities();
        let r = self.payment_now()?;
        Ok(json!({ "csv": r.csv(), "report": r, "schedule_ready": acts.is_some(), "schedule_note": why }))
    }

    fn payment_set(&mut self, args: &Value) -> Result<Value, String> {
        let pay: nnx_core::payment::PaymentInputs = serde_json::from_value(args.get("payment").cloned().ok_or("payment is required")?).map_err(|e| format!("payment: {e}"))?;
        let changed = self.exec_update("Payment schedule", |e| {
            e.payment = pay.clone();
            Ok(())
        })?;
        let mut v = self.payment_report()?;
        v["changed"] = json!(changed);
        Ok(v)
    }

    /// The log of committed steps (who, which elements, BOQ cost impact at the stored rates). Read-only: the document is not changed.
    /// `limit` (optional, at least 1) keeps the newest steps.
    fn change_log(&self, args: &Value) -> Result<Value, String> {
        let doc = self.doc()?;
        let limit = match args.get("limit") {
            None | Some(Value::Null) => None,
            Some(v) => Some(v.as_u64().filter(|&n| n >= 1).ok_or("limit must be a whole number of at least 1")? as usize),
        };
        let lib = self.project().and_then(|p| p.boq).unwrap_or_default();
        let log = nnx_core::changelog::build(doc, limit, |m| {
            let mut items = qto::compute(m, &QtoOpts::default()).items;
            items.extend(self.extra_items(m));
            Ok(nnx_core::boq::compute(&items, &lib))
        });
        Ok(json!({ "log": log, "csv": log.csv(), "html": log.html() }))
    }

    fn site_photos_report(&self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let rep = nnx_core::sitephotos::report(&self.doc()?.model, &p.meta.site_photos);
        Ok(json!({ "inputs": p.meta.site_photos, "csv": rep.csv(), "html": rep.html(), "report": rep }))
    }

    /// Replaces the stored photo references (`photos`): ONE undoable edit. Every photo needs a file, a real date and at least one
    /// element that exists in the model; nothing is stored when any photo is refused.
    fn site_photos_set(&mut self, args: &Value) -> Result<Value, String> {
        let sp: nnx_core::sitephotos::SitePhotos = serde_json::from_value(json!({ "photos": args.get("photos").cloned().ok_or("photos is required")? })).map_err(|e| format!("photos: {e}"))?;
        sp.check()?;
        nnx_core::sitephotos::validate_links(&self.doc()?.model, &sp)?;
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        let out = nnx_core::project::update(doc, "Site photos", |p| {
            p.meta.site_photos = sp.clone();
            Ok(())
        })?;
        let mut v = self.site_photos_report()?;
        v["changed"] = json!(out.changed);
        Ok(v)
    }

    fn progress_report(&self) -> Result<Value, String> {
        let inputs = self.exec_inputs()?.payment;
        let pay = self.payment_now()?;
        let r = nnx_core::progress::compute(&inputs.progress, &pay);
        Ok(json!({ "csv": r.csv(), "report": r }))
    }

    /// Replaces only the certificates and the retention; the milestones and the contract value are left as they are.
    fn progress_set(&mut self, args: &Value) -> Result<Value, String> {
        let prog: nnx_core::progress::ProgressInputs = serde_json::from_value(args.get("progress").cloned().ok_or("progress is required")?).map_err(|e| format!("progress: {e}"))?;
        let changed = self.exec_update("Progress payment certificates", |e| {
            e.payment.progress = prog.clone();
            Ok(())
        })?;
        let mut v = self.progress_report()?;
        v["changed"] = json!(changed);
        Ok(v)
    }

    fn procure_now(&self) -> Result<nnx_core::procure::ProcureReport, String> {
        let inputs = self.exec_inputs()?.procurement;
        let (acts, _) = self.stored_activities();
        let today = Date::parse(&utc_now().chars().take(10).collect::<String>());
        let has_elevator = self.project().is_some_and(|p| p.meta.vertical.is_some_and(|v| v.elevator.is_some()));
        Ok(nnx_core::procure::compute(&inputs, acts.as_deref(), today, has_elevator))
    }

    fn procure_report(&self) -> Result<Value, String> {
        let (acts, why) = self.stored_activities();
        let r = self.procure_now()?;
        Ok(json!({ "csv": r.csv(), "report": r, "schedule_ready": acts.is_some(), "schedule_note": why }))
    }

    fn procure_set(&mut self, args: &Value) -> Result<Value, String> {
        let pr: nnx_core::procure::ProcureInputs = serde_json::from_value(args.get("procurement").cloned().ok_or("procurement is required")?).map_err(|e| format!("procurement: {e}"))?;
        let changed = self.exec_update("Long-lead procurement", |e| {
            e.procurement = pr.clone();
            Ok(())
        })?;
        let mut v = self.procure_report()?;
        v["changed"] = json!(changed);
        Ok(v)
    }

    fn qaqc_report_in(&self, args: &Value) -> Result<Value, String> {
        let inputs = self.exec_inputs()?.qaqc;
        let (acts, why) = self.stored_activities();
        let r = nnx_core::qaqc::report(&inputs, acts.as_deref());
        let arabic = args.get("lang").and_then(Value::as_str) != Some("en");
        Ok(json!({ "html": r.html(&self.display_title(), arabic), "xlsx_b64": nnx_core::xlsx::base64_encode(&r.workbook(arabic).to_bytes()), "report": r, "schedule_ready": acts.is_some(), "schedule_note": why }))
    }

    fn qaqc_report(&self, args: &Value) -> Result<Value, String> {
        self.qaqc_report_in(args)
    }

    fn qaqc_set(&mut self, args: &Value) -> Result<Value, String> {
        let qa: nnx_core::qaqc::QaInputs = serde_json::from_value(args.get("qaqc").cloned().ok_or("qaqc is required")?).map_err(|e| format!("qaqc: {e}"))?;
        let changed = self.exec_update("QA/QC records", |e| {
            e.qaqc = qa.clone();
            Ok(())
        })?;
        let mut v = self.qaqc_report_in(args)?;
        v["changed"] = json!(changed);
        Ok(v)
    }

    fn roi_now(&self) -> Result<nnx_core::roi::RoiReport, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let cost = self.cost_now()?;
        let boq = self.boq_now()?;
        let ctx = nnx_core::roi::RoiContext {
            is_apartment_building: p.meta.building_type == Some(nnx_core::project::BuildingType::ApartmentBuilding),
            tier_total: cost.selected_result().map(|t| (t.total, t.complete)),
            boq_total: (boq.priced > 0).then_some((boq.total, boq.complete)),
            floors: p.structure.floors,
        };
        Ok(nnx_core::roi::compute(&p.meta.execution.roi, &ctx))
    }

    fn roi_report(&self) -> Result<Value, String> {
        Ok(json!({ "report": self.roi_now()? }))
    }

    fn roi_set(&mut self, args: &Value) -> Result<Value, String> {
        let r: nnx_core::roi::RoiInputs = serde_json::from_value(args.get("roi").cloned().ok_or("roi is required")?).map_err(|e| format!("roi: {e}"))?;
        let changed = self.exec_update("Apartment ROI", |e| {
            e.roi = r.clone();
            Ok(())
        })?;
        let mut v = self.roi_report()?;
        v["changed"] = json!(changed);
        Ok(v)
    }

    /// Runs the rules: the pack pasted in the Rules tab (if any), then the enabled municipal overlays in order. Every finding names the
    /// layer its rule belongs to, and `overlays` says what each layer added, replaced and overrode.
    fn check_rules(&self, args: &Value) -> Result<Value, String> {
        let doc = self.doc()?;
        let base = args.get("pack").and_then(Value::as_str);
        let overlays = self.project().map(|p| nnx_core::plugins::effective_overlays(&p.overlays, &p.plugins)).unwrap_or_default();
        let sourced = args.get("sourced").and_then(Value::as_bool).unwrap_or(false);
        if !sourced && base.is_none_or(|b| b.trim().is_empty()) && !overlays.iter().any(|o| o.enabled) {
            return Err("paste a rule pack (JSON), use the sourced pack, or load and enable a municipal overlay".into());
        }
        // The sourced pack is the bottom layer; a pasted pack and the enabled overlays go on top of it and may replace its rules.
        let (mut rs, mut reports) = if sourced {
            let (mut rs, mut reports) = nnx_core::overlay::rule_set(Some(&nnx_core::sbc::sourced_pack_json()), &[])?;
            if let Some(b) = base.filter(|b| !b.trim().is_empty()) {
                reports.push(rs.add_layer_reporting(b).map_err(|e| format!("rule pack: {e}"))?);
            }
            for o in overlays.iter().filter(|o| o.enabled) {
                reports.push(rs.add_layer_reporting(&o.pack).map_err(|e| format!("overlay '{}': {e}", o.name))?);
            }
            (rs, reports)
        } else {
            nnx_core::overlay::rule_set(base, &overlays)?
        };
        // Limits the user entered for rules that were not run: each needs a value and its source; they go on top as one more layer.
        let limits = args.get("limits").and_then(Value::as_array).cloned().unwrap_or_default();
        if !limits.is_empty() {
            let mut overrides = serde_json::Map::new();
            for l in &limits {
                let rule = l.get("rule").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or("a limit names no rule")?;
                let param = l.get("param").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| format!("the limit for '{rule}' names no parameter"))?;
                let value = l.get("value").and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| format!("the limit for '{rule}' has no numeric value"))?;
                let source = l.get("source").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| format!("the limit for '{rule}' has no source: state where the value comes from"))?;
                overrides.insert(rule.to_string(), json!({ param: { "value": value, "source": source } }));
            }
            let pack = json!({ "id": "USER-LIMITS", "edition": "entered in the application", "version": "1", "overrides": overrides });
            reports.push(rs.add_layer_reporting(&pack.to_string()).map_err(|e| format!("limits: {e}"))?);
        }
        let r = rules::check(&doc.model, &rs);
        let sev = |s: rules::Severity| match s {
            rules::Severity::Error => "error",
            rules::Severity::Warning => "warning",
            rules::Severity::Info => "info",
        };
        Ok(json!({
            "checked": r.checked,
            "layers": r.layers,
            "overlays": reports,
            "findings": r.findings.iter().map(|f| json!({ "rule": f.rule_id, "clause": f.clause, "layer": f.layer, "severity": sev(f.severity), "message": f.message, "element": f.element.to_string() })).collect::<Vec<_>>(),
            "skipped": r.skipped.iter().map(|s| json!({ "rule": s.rule_id, "param": s.param, "reason": s.reason, "elements": self.skipped_rooms(&s.elements) })).collect::<Vec<_>>(),
        }))
    }

    /// The rooms a skipped rule left unchecked: name, area and measured glazing ratio (null when the model has none).
    fn skipped_rooms(&self, ids: &[nnx_core::Id]) -> Vec<Value> {
        let Ok(doc) = self.doc() else { return Vec::new() };
        let labels = self.labels();
        ids.iter()
            .filter_map(|id| doc.model.get(id))
            .filter_map(|e| e.typed::<nnx_core::typed::Space>().ok().map(|s| (e, s)))
            .map(|(e, s)| {
                let name = nnx_core::annot::room_name(&s, &labels).unwrap_or(s.requirement_key.clone());
                json!({ "id": e.id.to_string(), "name": name, "area": s.area, "glazing_ratio": s.glazing_ratio })
            })
            .collect()
    }

    // ------------------------------------------------------------------ Milestone 7: outliner, snapshots, overlays

    /// The schedule built from the setup stored in the project, or why there is none yet.
    fn stored_schedule(&self) -> (Option<schedule::Schedule>, Option<String>) {
        let Some(setup) = self.project().and_then(|p| p.meta.execution.schedule) else {
            return (None, Some("compute the schedule first (Schedule tab)".into()));
        };
        let (Ok(doc), Ok(q)) = (self.doc(), self.schedule_qto()) else { return (None, Some("choose a layout first".into())) };
        match setup.build(&doc.model, &q) {
            Ok((s, _)) => (Some(s), None),
            Err(BuildError::MissingRates(k)) => (None, Some(format!("the schedule is missing {} rate(s): {}", k.len(), k.join(", ")))),
            Err(BuildError::Invalid(why)) => (None, Some(why)),
        }
    }

    /// Element id -> its name in the schedules (`COL-1`, `FND-1`, `BM-1`, `SL-1`).
    fn element_labels(&self) -> BTreeMap<String, String> {
        let thickness = self.project().and_then(|p| p.sizing).and_then(|s| s.footing_thickness_mm);
        self.doc().map(|d| nnx_core::structx::schedule_names(&d.model, thickness)).unwrap_or_default()
    }

    /// The project hierarchy: Site -> Levels -> Spaces, Envelope, Structural, MEP -> Schedule and Execution.
    fn outline_report(&self) -> Result<Value, String> {
        let model = &self.doc()?.model;
        let p = self.project();
        let (sched, _) = self.stored_schedule();
        let tasks: Option<Vec<nnx_core::outline::OutlineTask>> = sched.map(|s| {
            s.tasks
                .iter()
                .map(|t| nnx_core::outline::OutlineTask { key: t.key.to_string(), name: t.name.clone(), phase: t.phase.to_string(), start: t.start.iso(), finish: t.finish.iso(), elements: t.elements.iter().map(|e| e.to_string()).collect() })
                .collect()
        });
        Ok(json!({ "tree": nnx_core::outline::build(model, p.as_ref(), &self.element_labels(), tasks.as_deref()) }))
    }

    /// A snapshot of the model as it is now, with its quantities and cost figures.
    fn capture_now(&self, id: &str, name: &str, note: Option<&str>) -> Result<nnx_core::snapshot::Snapshot, String> {
        let model = &self.doc()?.model;
        let mut items = qto::compute(model, &QtoOpts::default()).items;
        items.extend(self.extra_items(model));
        let lib = self.project().and_then(|p| p.boq).unwrap_or_default();
        let boq = nnx_core::boq::compute(&items, &lib);
        let cost = nnx_core::costtier::compute(&items, &self.exec_inputs()?.cost);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let mut labels = self.element_labels();
        if let Some(p) = self.project() {
            labels.extend(nnx_core::snapshot::room_labels(model, &p.labels));
        }
        Ok(nnx_core::snapshot::capture(model, id, name, note, now, &labels, &items, &boq, &cost))
    }

    fn snapshots_report(&self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let list: Vec<Value> = p
            .snapshots
            .iter()
            .map(|s| json!({ "id": s.id, "name": s.name, "note": s.note, "date": nnx_core::revisions::date(s.created_unix), "elements": s.elements.len(), "boq_total": s.costs.boq_total, "boq_complete": s.costs.boq_complete }))
            .collect();
        Ok(json!({ "snapshots": list, "max": nnx_core::snapshot::MAX_SNAPSHOTS }))
    }

    /// Saves a named snapshot of the model, its quantities and its cost figures in the project: ONE undoable step.
    fn snapshot_save(&mut self, args: &Value) -> Result<Value, String> {
        let name = args.get("name").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).ok_or("give the snapshot a name")?.to_string();
        let note = args.get("note").and_then(Value::as_str).map(str::to_string);
        let existing = self.project().ok_or("choose a layout first")?.snapshots;
        let snap = self.capture_now(&nnx_core::snapshot::next_id(&existing), &name, note.as_deref())?;
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        nnx_core::project::update(doc, "Snapshot", |p| {
            p.snapshots.push(snap.clone());
            Ok(())
        })?;
        self.snapshots_report()
    }

    fn snapshot_delete(&mut self, args: &Value) -> Result<Value, String> {
        let id = args.get("id").and_then(Value::as_str).ok_or("id is required")?.to_string();
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        nnx_core::project::update(doc, "Delete snapshot", |p| {
            let before = p.snapshots.len();
            p.snapshots.retain(|s| s.id != id);
            if p.snapshots.len() == before {
                return Err(format!("no snapshot '{id}'"));
            }
            Ok(())
        })?;
        self.snapshots_report()
    }

    /// The snapshot `id`, or the model as it is now for `current`.
    fn snapshot_or_current(&self, id: &str) -> Result<nnx_core::snapshot::Snapshot, String> {
        if id == "current" {
            return self.capture_now("current", "Current model", None);
        }
        self.project().ok_or("choose a layout first")?.snapshots.into_iter().find(|s| s.id == id).ok_or_else(|| format!("no snapshot '{id}'"))
    }

    /// What changed from snapshot `a` to snapshot `b` (or the current model). `statuses` names the changed elements that still exist,
    /// for colouring them in the 3D view.
    fn snapshot_diff(&self, args: &Value) -> Result<Value, String> {
        let a = self.snapshot_or_current(args.get("a").and_then(Value::as_str).ok_or("choose the first snapshot")?)?;
        let b = self.snapshot_or_current(args.get("b").and_then(Value::as_str).unwrap_or("current"))?;
        let d = nnx_core::snapshot::compare(&a, &b);
        let statuses: BTreeMap<&str, &str> = d.elements.iter().filter(|e| e.status != "removed").map(|e| (e.id.as_str(), e.status)).collect();
        Ok(json!({ "diff": d, "statuses": statuses, "on_plan": args.get("b").and_then(Value::as_str).is_none_or(|b| b == "current") }))
    }

    /// The printable change report (`html`, bilingual) and its CSV between snapshot `a` and `b` (default: the current model).
    /// Read-only. `lang` is `en` for English, anything else Arabic.
    fn snapshot_report(&self, args: &Value) -> Result<Value, String> {
        let a = self.snapshot_or_current(args.get("a").and_then(Value::as_str).ok_or("choose the first snapshot")?)?;
        let b = self.snapshot_or_current(args.get("b").and_then(Value::as_str).unwrap_or("current"))?;
        let d = nnx_core::snapshot::compare(&a, &b);
        let date: String = utc_now().chars().take(10).collect();
        let arabic = args.get("lang").and_then(Value::as_str) != Some("en");
        Ok(json!({
            "html": nnx_core::changereport::html(&d, &self.display_title(), &date, arabic),
            "csv": nnx_core::changereport::csv(&d),
            "name": "change_report.pdf",
            "csv_name": "change_report.csv",
            "has_changes": nnx_core::changereport::has_changes(&d),
            "added": d.added, "removed": d.removed, "modified": d.modified,
        }))
    }

    /// The plan of the level on screen with revision clouds around what changed from snapshot `a` to the current model.
    fn snapshot_svg(&self, args: &Value) -> Result<Value, String> {
        let a = self.snapshot_or_current(args.get("a").and_then(Value::as_str).ok_or("choose a snapshot")?)?;
        let b = self.capture_now("current", "Current model", None)?;
        let d = nnx_core::snapshot::compare(&a, &b);
        let changes: Vec<nnx_core::revisions::Change> = d.elements.iter().filter(|e| e.status != "removed").map(|e| nnx_core::revisions::Change { id: e.id.clone(), kind: e.kind.clone(), status: e.status }).collect();
        let model = &self.doc()?.model;
        let opts = self.draw_opts(self.show_structure);
        let svg = plan_svg(model, self.current_level(), &opts).map_err(|e| e.to_string())?;
        Ok(json!({ "svg": nnx_core::revisions::add_clouds(&svg, model, self.current_level(), &opts, &changes), "clouds": changes.len() }))
    }

    fn overlays_report(&self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let list: Vec<Value> = p
            .overlays
            .iter()
            .map(|o| {
                let label = serde_json::from_str::<nnx_core::rules::PackDef>(&o.pack).map(|d| format!("{}@{}/{}", d.id, d.edition, d.version)).unwrap_or_default();
                json!({ "id": o.id, "name": o.name, "authority": o.authority, "document": o.document, "date": o.date, "enabled": o.enabled, "label": label, "has_limits": nnx_core::overlay::limits_of(&o.pack).ok().flatten().is_some() })
            })
            .collect();
        Ok(json!({ "overlays": list, "limits": p.limits }))
    }

    fn overlay_from_args(&self, args: &Value) -> Result<nnx_core::overlay::Overlay, String> {
        let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).unwrap_or("").to_string();
        let list = self.project().ok_or("choose a layout first")?.overlays;
        Ok(nnx_core::overlay::Overlay {
            id: nnx_core::overlay::next_id(&list),
            name: s("name"),
            authority: s("authority"),
            document: s("document"),
            date: Some(s("date")).filter(|d| !d.is_empty()),
            enabled: args.get("enabled").and_then(Value::as_bool).unwrap_or(true),
            pack: args.get("pack").and_then(Value::as_str).ok_or("paste or open the overlay (JSON)")?.to_string(),
        })
    }

    /// The rule set in force before a new overlay: the pasted base pack (if any), then the enabled overlays already loaded.
    fn overlay_base(&self, args: &Value) -> Result<nnx_core::rules::RuleSet, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let list = nnx_core::plugins::effective_overlays(&p.overlays, &p.plugins);
        nnx_core::overlay::rule_set(args.get("base").and_then(Value::as_str), &list).map(|r| r.0)
    }

    /// What loading this overlay would do, without storing anything.
    fn overlay_review(&self, args: &Value) -> Result<Value, String> {
        let o = self.overlay_from_args(args)?;
        let p = self.project().ok_or("choose a layout first")?;
        let base = self.overlay_base(args)?;
        Ok(json!({ "review": nnx_core::overlay::review(&base, &o, &p.limits) }))
    }

    /// Stores the overlay once it is valid and, when it replaces or overrides anything, once the user confirmed that.
    fn overlay_add(&mut self, args: &Value) -> Result<Value, String> {
        let o = self.overlay_from_args(args)?;
        let p = self.project().ok_or("choose a layout first")?;
        let review = nnx_core::overlay::review(&self.overlay_base(args)?, &o, &p.limits);
        if !review.errors.is_empty() {
            return Err(format!("overlay refused: {}", review.errors.join("; ")));
        }
        if review.needs_confirm && args.get("confirm").and_then(Value::as_bool) != Some(true) {
            return Ok(json!({ "stored": false, "review": review }));
        }
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        nnx_core::project::update(doc, "Load overlay", |p| {
            p.overlays.push(o.clone());
            Ok(())
        })?;
        let mut v = self.overlays_report()?;
        v["stored"] = json!(true);
        v["review"] = json!(review);
        Ok(v)
    }

    fn overlay_toggle(&mut self, args: &Value) -> Result<Value, String> {
        let id = args.get("id").and_then(Value::as_str).ok_or("id is required")?.to_string();
        let on = args.get("enabled").and_then(Value::as_bool).ok_or("enabled is required")?;
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        nnx_core::project::update(doc, if on { "Enable overlay" } else { "Disable overlay" }, |p| {
            p.overlays.iter_mut().find(|o| o.id == id).ok_or_else(|| format!("no overlay '{id}'"))?.enabled = on;
            Ok(())
        })?;
        self.overlays_report()
    }

    fn overlay_remove(&mut self, args: &Value) -> Result<Value, String> {
        let id = args.get("id").and_then(Value::as_str).ok_or("id is required")?.to_string();
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        nnx_core::project::update(doc, "Remove overlay", |p| {
            let before = p.overlays.len();
            p.overlays.retain(|o| o.id != id);
            if p.overlays.len() == before {
                return Err(format!("no overlay '{id}'"));
            }
            Ok(())
        })?;
        self.overlays_report()
    }

    /// Writes the limits of an overlay into the project's municipal limits (old and new are returned): ONE undoable step.
    fn overlay_apply_limits(&mut self, args: &Value) -> Result<Value, String> {
        let id = args.get("id").and_then(Value::as_str).ok_or("id is required")?.to_string();
        let mut changed = Vec::new();
        let mut violations = Vec::new();
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        let out = nnx_core::project::update(doc, "Apply overlay limits", |p| {
            let o = p.overlays.iter().find(|o| o.id == id).cloned().ok_or_else(|| format!("no overlay '{id}'"))?;
            changed = nnx_core::overlay::apply_limits(p, &o)?;
            Ok(())
        })?;
        violations.extend(out.new_violations.iter().map(|v| v.en.clone()));
        let mut v = self.overlays_report()?;
        v["changed"] = json!(changed);
        v["new_violations"] = json!(violations);
        Ok(v)
    }

    // ------------------------------------------------------------------ Office plugins

    fn plugins_report(&self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let list: Vec<Value> = p
            .plugins
            .iter()
            .map(|x| {
                let templates: Vec<Value> = x.templates.iter().map(|t| json!({ "address": format!("{}/{}", x.id, t.id), "title_ar": t.title_ar, "title_en": t.title_en, "rooms": t.rooms.len(), "floors": t.floors })).collect();
                json!({ "id": x.id, "name": x.name, "version": x.version, "publisher": x.publisher, "source": x.source, "description": x.description, "date": x.date, "enabled": x.enabled, "has_rules": x.rules.is_some(), "templates": templates })
            })
            .collect();
        Ok(json!({ "plugins": list }))
    }

    /// The plugin typed or loaded by the user (`plugin`: an object or its JSON text). It is installed disabled-or-enabled as `enabled` says.
    fn plugin_from_args(&self, args: &Value) -> Result<nnx_core::plugins::Plugin, String> {
        let raw = args.get("plugin").ok_or("plugin is required (the plugin definition)")?;
        let mut v = match raw {
            Value::String(text) => serde_json::from_str::<Value>(text).map_err(|e| format!("plugin json: {e}"))?,
            other => other.clone(),
        };
        if v.get("rules").is_some_and(Value::is_object) {
            let text = v["rules"].to_string();
            v["rules"] = Value::String(text);
        }
        let mut pl: nnx_core::plugins::Plugin = serde_json::from_value(v).map_err(|e| format!("plugin: {e}"))?;
        if pl.id.trim().is_empty() {
            pl.id = nnx_core::plugins::next_id(&self.project().ok_or("choose a layout first")?.plugins);
        }
        pl.enabled = args.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        Ok(pl)
    }

    /// What installing this plugin would do, without storing anything.
    fn plugin_review(&self, args: &Value) -> Result<Value, String> {
        let pl = self.plugin_from_args(args)?;
        let p = self.project().ok_or("choose a layout first")?;
        Ok(json!({ "review": nnx_core::plugins::review(&pl, &p.overlays, &p.plugins) }))
    }

    /// Stores the plugin once it is valid and, when it replaces anything, once the user confirmed that.
    fn plugin_add(&mut self, args: &Value) -> Result<Value, String> {
        let pl = self.plugin_from_args(args)?;
        let p = self.project().ok_or("choose a layout first")?;
        let review = nnx_core::plugins::review(&pl, &p.overlays, &p.plugins);
        if !review.errors.is_empty() {
            return Err(format!("plugin refused: {}", review.errors.join("; ")));
        }
        if review.needs_confirm && args.get("confirm").and_then(Value::as_bool) != Some(true) {
            return Ok(json!({ "stored": false, "review": review }));
        }
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        nnx_core::project::update(doc, "Install plugin", |p| {
            p.plugins.retain(|x| x.id != pl.id);
            p.plugins.push(pl.clone());
            Ok(())
        })?;
        let mut v = self.plugins_report()?;
        v["stored"] = json!(true);
        v["review"] = json!(review);
        Ok(v)
    }

    fn plugin_toggle(&mut self, args: &Value) -> Result<Value, String> {
        let id = args.get("id").and_then(Value::as_str).ok_or("id is required")?.to_string();
        let on = args.get("enabled").and_then(Value::as_bool).ok_or("enabled is required")?;
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        nnx_core::project::update(doc, if on { "Enable plugin" } else { "Disable plugin" }, |p| {
            p.plugins.iter_mut().find(|x| x.id == id).ok_or_else(|| format!("no plugin '{id}'"))?.enabled = on;
            Ok(())
        })?;
        self.plugins_report()
    }

    fn plugin_remove(&mut self, args: &Value) -> Result<Value, String> {
        let id = args.get("id").and_then(Value::as_str).ok_or("id is required")?.to_string();
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        nnx_core::project::update(doc, "Remove plugin", |p| {
            let before = p.plugins.len();
            p.plugins.retain(|x| x.id != id);
            if p.plugins.len() == before {
                return Err(format!("no plugin '{id}'"));
            }
            Ok(())
        })?;
        self.plugins_report()
    }

    /// A template of an enabled plugin (`address`: `plugin_id/template_id`) with the notes about its room areas. Read-only. No
    /// setbacks are derived: an office template states none, so the project's own limits govern.
    fn plugin_template(&self, args: &Value) -> Result<Value, String> {
        let address = args.get("address").and_then(Value::as_str).ok_or("address is required (plugin_id/template_id)")?;
        let p = self.project().ok_or("choose a layout first")?;
        let t = nnx_core::plugins::find_template(&p.plugins, address)?;
        Ok(json!({ "template": t, "notes": t.notes(), "setbacks": Value::Null, "setbacks_note": "an office template carries no setbacks: enter them from the municipality's regulation" }))
    }

    // ------------------------------------------------------------------ Municipality regulation table

    fn municipalities_report(&self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let list: Vec<Value> = p.municipalities.iter().map(|e| json!({ "entry": e, "set": e.set_count(), "unset": e.unset() })).collect();
        Ok(json!({ "municipalities": list, "limits": p.limits }))
    }

    /// Stores one entry typed by the user. Every value needs its source; nothing is filled in.
    fn municipality_add(&mut self, args: &Value) -> Result<Value, String> {
        let list = self.project().ok_or("choose a layout first")?.municipalities;
        let mut e: nnx_core::municipality::MunicipalityEntry =
            serde_json::from_value(args.get("entry").cloned().ok_or("entry is required")?).map_err(|e| format!("municipality entry: {e}"))?;
        e.id = nnx_core::municipality::next_id(&list);
        e.municipality = e.municipality.trim().to_string();
        e.zone = e.zone.trim().to_string();
        e.document = e.document.trim().to_string();
        if let Some(first) = nnx_core::municipality::problems(&e).into_iter().next() {
            return Err(format!("municipality entry refused: {first}"));
        }
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        nnx_core::project::update(doc, "Add municipality entry", |p| {
            p.municipalities.push(e.clone());
            Ok(())
        })?;
        let mut v = self.municipalities_report()?;
        v["added"] = json!(e.id);
        Ok(v)
    }

    fn municipality_remove(&mut self, args: &Value) -> Result<Value, String> {
        let id = args.get("id").and_then(Value::as_str).ok_or("id is required")?.to_string();
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        nnx_core::project::update(doc, "Remove municipality entry", |p| {
            let before = p.municipalities.len();
            p.municipalities.retain(|e| e.id != id);
            if p.municipalities.len() == before {
                return Err(format!("no municipality entry '{id}'"));
            }
            Ok(())
        })?;
        self.municipalities_report()
    }

    /// Writes the setback and coverage limits of an entry into the project's limits (old and new are returned): ONE undoable step.
    fn municipality_apply(&mut self, args: &Value) -> Result<Value, String> {
        let id = args.get("id").and_then(Value::as_str).ok_or("id is required")?.to_string();
        let mut applied = nnx_core::municipality::Applied::default();
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        let out = nnx_core::project::update(doc, "Apply municipality limits", |p| {
            applied = nnx_core::municipality::apply(p, &id)?;
            Ok(())
        })?;
        let mut v = self.municipalities_report()?;
        v["changed"] = json!(applied.changed);
        v["stored_only"] = json!(applied.stored_only);
        v["new_violations"] = json!(out.new_violations.iter().map(|x| x.en.clone()).collect::<Vec<_>>());
        Ok(v)
    }

    /// Read-only: the model against the limits of one municipality entry (`id`), without applying them to the project.
    fn zoning_check(&self, args: &Value) -> Result<Value, String> {
        let id = args.get("id").and_then(Value::as_str).ok_or("id is required")?;
        let d = self.doc.as_ref().ok_or("choose a layout first")?;
        serde_json::to_value(nnx_core::zoning::run(&d.model, id)?).map_err(|e| e.to_string())
    }

    // ------------------------------------------------------------------ Milestone 8: BIM drafting

    fn ids_arg(args: &Value, key: &str) -> Result<Vec<nnx_core::Id>, String> {
        let list = args.get(key).and_then(Value::as_array).ok_or_else(|| format!("{key} is required"))?;
        list.iter().map(|v| v.as_str().ok_or("an id is not text".to_string())?.parse::<nnx_core::Id>().map_err(|_| "an id is not valid".to_string())).collect()
    }

    fn point_arg(v: Option<&Value>, what: &str) -> Result<[f64; 2], String> {
        let a = v.and_then(Value::as_array).filter(|a| a.len() == 2).ok_or_else(|| format!("{what} is a pair of numbers"))?;
        Ok([a[0].as_f64().ok_or_else(|| format!("{what}: x is not a number"))?, a[1].as_f64().ok_or_else(|| format!("{what}: y is not a number"))?])
    }

    fn int_point(v: Option<&Value>, what: &str) -> Result<[i64; 2], String> {
        let p = Self::point_arg(v, what)?;
        Ok([p[0].round() as i64, p[1].round() as i64])
    }

    /// The plan's frame (to turn pixels into millimetres) and the starting values the drawing tools offer.
    fn draft_frame(&self) -> Result<Value, String> {
        let model = &self.doc()?.model;
        let opts = self.draw_opts(self.show_structure);
        let frame = nnx_core::drawing::frame(model, self.current_level(), &opts).map(|(minx, maxy, s, m)| json!({ "minx": minx, "maxy": maxy, "s": s, "m": m }));
        Ok(json!({ "frame": frame, "defaults": nnx_core::draft::defaults(model, self.current_level()) }))
    }

    /// Commits a draft edit (and the structure sync that follows from it) as ONE undoable step and answers like the other plan edits.
    /// A wall edit also brings the level's computed rooms in line with the regions its walls enclose, in the same step.
    fn draft_commit(&mut self, label: &str, edit: nnx_core::draft::Edit, level: Option<nnx_core::Id>) -> Result<Value, String> {
        self.draft_commit_with(label, edit, level, true)
    }

    fn draft_commit_with(&mut self, label: &str, edit: nnx_core::draft::Edit, level: Option<nnx_core::Id>, find_rooms: bool) -> Result<Value, String> {
        if edit.cmds.is_empty() {
            return Err("nothing to change".into());
        }
        let so = self.sopts()?;
        let mut cmds = edit.cmds;
        let mut sync = nnx_core::annot::RoomSync::default();
        if find_rooms {
            let mut scratch = Document::new(self.doc()?.model.clone());
            scratch.commit(label, nnx_core::Origin::User, cmds.clone()).map_err(|e| e.to_string())?;
            sync = nnx_core::annot::rooms_sync(&scratch.model, level);
            cmds.extend(sync.cmds.clone());
            if !sync.cmds.is_empty() {
                scratch.commit(label, nnx_core::Origin::User, sync.cmds.clone()).map_err(|e| e.to_string())?;
            }
            cmds.extend(nnx_core::draft::refresh_walls(&scratch.model, level));
        }
        editing::commit_with_sync(self.doc.as_mut().ok_or("choose a layout first")?, &so, label, cmds, level)?;
        let mut notes = edit.notes;
        if sync.not_rect > 0 {
            notes.push(format!("{} enclosed region(s) are not rectangles and got no room: split them with a wall", sync.not_rect));
        }
        if !sync.overlapping.is_empty() {
            notes.push(format!("a new enclosed region overlaps the room(s) {} and was not made a room", sync.overlapping.join(", ")));
        }
        notes.extend(sync.notes.iter().cloned());
        notes.extend(sync.blocked.iter().cloned());
        let mut v = self.draft_frame()?;
        v["created"] = json!(edit.created.iter().map(|i| i.to_string()).collect::<Vec<_>>());
        v["notes"] = json!(notes);
        v["rooms_added"] = json!(sync.added);
        v["rooms_removed"] = json!(sync.removed);
        v["svg"] = self.render_or_null()?;
        v["rooms"] = json!(self.rooms_json());
        v["sbc"] = self.sbc_json();
        Ok(v)
    }

    /// A new empty project: its project element and the level LVL-1. Drafting (walls, doors, windows, grid lines, dimensions, tags) then
    /// works on it directly; rooms are found from the walls that enclose them.
    fn project_blank(&mut self, args: &Value) -> Result<Value, String> {
        let title = args.get("title").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let mut doc = Document::new(nnx_core::Model::default());
        doc.commit("new project", nnx_core::Origin::User, nnx_core::annot::blank_project_commands(&title)?).map_err(|e| e.to_string())?;
        doc.mark_saved();
        self.alternatives.clear();
        self.plan = None;
        self.draft = None;
        self.cur_level = 0;
        self.show_structure = false;
        self.doc = Some(doc);
        let mut v = self.draft_frame()?;
        v["svg"] = self.render_or_null()?;
        v["rooms"] = json!(self.rooms_json());
        v["levels"] = self.levels_json();
        v["settings"] = self.form_state();
        v["sbc"] = self.sbc_json();
        Ok(v)
    }

    fn draft_grid(&mut self, args: &Value) -> Result<Value, String> {
        let spec = nnx_core::annot::GridSpec { a: Self::int_point(args.get("a"), "a")?, b: Self::int_point(args.get("b"), "b")?, name: args.get("name").and_then(Value::as_str).map(str::to_string) };
        let (cmd, id) = nnx_core::annot::grid_command(&self.doc()?.model, &spec)?;
        let level = self.current_level();
        let mut v = self.draft_commit_with("Draw grid line", nnx_core::draft::Edit { cmds: vec![cmd], created: vec![id], notes: vec![] }, level, false)?;
        v["grid"] = json!(id.to_string());
        Ok(v)
    }

    fn draft_dimension(&mut self, args: &Value) -> Result<Value, String> {
        let opt_id = |k: &str| -> Result<Option<nnx_core::Id>, String> { args.get(k).and_then(Value::as_str).map(|t| t.parse().map_err(|_| format!("{k} is not an element id"))).transpose() };
        let spec = nnx_core::annot::DimSpec { a: Self::int_point(args.get("a"), "a")?, b: Self::int_point(args.get("b"), "b")?, at: Self::int_point(args.get("at"), "at")?, ref_a: opt_id("refA")?, ref_b: opt_id("refB")? };
        let level = self.current_level();
        let (cmd, id, notes) = nnx_core::annot::dimension_command(&self.doc()?.model, level, &spec)?;
        let mut v = self.draft_commit_with("Add dimension", nnx_core::draft::Edit { cmds: vec![cmd], created: vec![id], notes }, level, false)?;
        v["dimension"] = json!(id.to_string());
        Ok(v)
    }

    /// Tags the given elements (rooms, doors, windows, columns or footings: `hosts`, or `rooms` for the old name). With no list, every room of
    /// the level that has none.
    fn draft_tag(&mut self, args: &Value) -> Result<Value, String> {
        let level = self.current_level();
        let model = &self.doc()?.model;
        let key = if args.get("hosts").is_some() { "hosts" } else { "rooms" };
        let rooms = if args.get(key).is_some() { Self::ids_arg(args, key)? } else { nnx_core::annot::untagged_rooms(model, level) };
        let (cmds, ids) = nnx_core::annot::tag_commands(model, &rooms)?;
        if cmds.is_empty() {
            return Err("every selected element already has a tag (or there is nothing to tag)".into());
        }
        self.draft_commit_with("Tag rooms", nnx_core::draft::Edit { cmds, created: ids, notes: vec![] }, level, false)
    }

    /// Draws a room separator and finds the rooms it creates, as one undoable step.
    fn draft_separator(&mut self, args: &Value) -> Result<Value, String> {
        let (a, b) = (Self::int_point(args.get("a"), "a")?, Self::int_point(args.get("b"), "b")?);
        let level = self.current_level();
        let (cmd, id) = nnx_core::annot::separator_command(&self.doc()?.model, level, (a[0], a[1]), (b[0], b[1]))?;
        let mut v = self.draft_commit("Draw room separator", nnx_core::draft::Edit { cmds: vec![cmd], created: vec![id], notes: vec![] }, level)?;
        v["separator"] = json!(id.to_string());
        Ok(v)
    }

    /// A parallel copy of a wall, room separator or grid line at a typed distance, on the side of the pointer.
    fn draft_offset(&mut self, args: &Value) -> Result<Value, String> {
        let id = id_arg(args, "id")?;
        self.guard_lock(&[id])?;
        let distance = args.get("distance").and_then(Value::as_i64).ok_or("enter the offset distance (mm)")?;
        let toward = Self::point_arg(args.get("toward"), "toward")?;
        let level = self.doc()?.model.get(&id).and_then(|e| e.level_id);
        let edit = nnx_core::draft::offset(&self.doc()?.model, id, distance, toward)?;
        self.draft_commit("Offset", edit, level)
    }

    /// The wall assemblies with their thickness and U-value, the defaults and the walls that disagree with their assembly.
    fn assemblies_get(&self) -> Result<Value, String> {
        let mut v = nnx_core::assembly::report(&self.doc()?.model);
        v["catalog"] = nnx_core::materials::catalog_json();
        Ok(v)
    }

    /// Replaces the project's wall assemblies and defaults. The thickness of every wall that names an assembly follows its layers, in the
    /// same undo step.
    fn assemblies_set(&mut self, args: &Value) -> Result<Value, String> {
        let list: Vec<nnx_core::assembly::WallAssembly> = serde_json::from_value(args.get("assemblies").cloned().ok_or("assemblies is required")?).map_err(|e| format!("assemblies: {e}"))?;
        let defaults: nnx_core::assembly::AssemblyDefaults = match args.get("defaults") {
            Some(d) if !d.is_null() => serde_json::from_value(d.clone()).map_err(|e| format!("defaults: {e}"))?,
            _ => Default::default(),
        };
        let model = &self.doc()?.model;
        let mut cmds = nnx_core::project::update_commands(model, |p| {
            p.meta.assemblies = list.clone();
            p.meta.assembly_defaults = defaults.clone();
            Ok(())
        })?;
        cmds.extend(nnx_core::assembly::retarget_commands(model, &list)?);
        let level = self.current_level();
        let mut v = if cmds.is_empty() { self.draft_frame()? } else { self.draft_commit_with("Wall assemblies", nnx_core::draft::Edit { cmds, ..Default::default() }, level, false)? };
        v["assemblies"] = self.assemblies_get()?;
        Ok(v)
    }

    /// Gives walls an assembly (or clears it with `null`); each wall's thickness becomes the sum of the assembly's layers.
    fn assembly_assign(&mut self, args: &Value) -> Result<Value, String> {
        let walls = Self::ids_arg(args, "walls")?;
        self.guard_lock(&walls)?;
        let asm = args.get("assembly").and_then(Value::as_str);
        let level = walls.first().and_then(|i| self.doc().ok()?.model.get(i)?.level_id);
        let cmds = nnx_core::assembly::assign_commands(&self.doc()?.model, &walls, asm)?;
        let mut v = self.draft_commit_with(if asm.is_some() { "Assign wall assembly" } else { "Clear wall assembly" }, nnx_core::draft::Edit { cmds, ..Default::default() }, level, false)?;
        v["assemblies"] = self.assemblies_get()?;
        Ok(v)
    }

    /// Finds the rooms the walls enclose, as one undoable step (the same thing every wall edit does on its own).
    fn draft_rooms(&mut self) -> Result<Value, String> {
        let level = self.current_level();
        let sync = nnx_core::annot::rooms_sync(&self.doc()?.model, level);
        if sync.cmds.is_empty() {
            let mut why = vec!["no new room: every enclosed rectangle already has one".to_string()];
            if sync.not_rect > 0 {
                why.push(format!("{} enclosed region(s) are not rectangles", sync.not_rect));
            }
            return Err(why.join("; "));
        }
        let mut v = self.draft_commit_with("Find rooms", nnx_core::draft::Edit { cmds: sync.cmds.clone(), created: vec![], notes: sync.notes.iter().chain(sync.blocked.iter()).cloned().collect() }, level, false)?;
        v["rooms_added"] = json!(sync.added);
        v["rooms_removed"] = json!(sync.removed);
        Ok(v)
    }

    fn draft_snap(&self, args: &Value) -> Result<Value, String> {
        let world = Self::point_arg(args.get("p"), "p")?;
        let ctx = nnx_core::draft::SnapCtx {
            scale: args.get("scale").and_then(Value::as_f64).filter(|s| *s > 0.0).ok_or("scale is required")?,
            from: args.get("from").filter(|f| !f.is_null()).map(|f| Self::point_arg(Some(f), "from")).transpose()?,
            ortho: args.get("ortho").and_then(Value::as_bool).unwrap_or(false),
            exclude: Self::ids_arg(args, "exclude").unwrap_or_default(),
        };
        Ok(json!(nnx_core::draft::snap(&self.doc()?.model, self.current_level(), world, &ctx)))
    }

    fn draft_wall(&mut self, args: &Value) -> Result<Value, String> {
        let spec = nnx_core::draft::WallSpec { a: Self::int_point(args.get("a"), "a")?, b: Self::int_point(args.get("b"), "b")?, thickness: args.get("thickness").and_then(Value::as_i64).ok_or("enter the wall's thickness (mm)")?, height: args.get("height").and_then(Value::as_i64).ok_or("enter the wall's height (mm)")? };
        let level = self.current_level();
        let (cmd, id) = nnx_core::draft::wall_command(&self.doc()?.model, level, &spec)?;
        let mut v = self.draft_commit("Draw wall", nnx_core::draft::Edit { cmds: vec![cmd], created: vec![id], notes: vec![] }, level)?;
        v["wall"] = json!(id.to_string());
        Ok(v)
    }

    fn draft_place(&self, args: &Value) -> Result<Value, String> {
        let p = Self::point_arg(args.get("p"), "p")?;
        let width = args.get("width").and_then(Value::as_i64).ok_or("enter the width (mm)")?;
        let scale = args.get("scale").and_then(Value::as_f64).filter(|s| *s > 0.0).ok_or("scale is required")?;
        Ok(json!({ "placement": nnx_core::draft::placement(&self.doc()?.model, self.current_level(), p, width, scale) }))
    }

    fn draft_opening(&mut self, args: &Value) -> Result<Value, String> {
        let spec: nnx_core::draft::OpeningSpec = serde_json::from_value(args.clone()).map_err(|e| format!("opening: {e}"))?;
        self.guard_lock(&[spec.wall])?;
        let level = self.doc()?.model.get(&spec.wall).and_then(|e| e.level_id);
        let (cmds, id) = nnx_core::draft::opening_command(&self.doc()?.model, &spec)?;
        let label = if spec.kind == nnx_core::draft::OpeningKind::Door { "Place door" } else { "Place window" };
        let mut v = self.draft_commit_with(label, nnx_core::draft::Edit { cmds, created: vec![id], notes: vec![] }, level, false)?;
        v["opening"] = json!(id.to_string());
        Ok(v)
    }

    fn draft_box(&self, args: &Value) -> Result<Value, String> {
        let b = args.get("box").and_then(Value::as_array).filter(|a| a.len() == 4).ok_or("box is four numbers")?;
        let n = |i: usize| b[i].as_f64().ok_or("box: a number is expected");
        let mode: nnx_core::draft::BoxMode = serde_json::from_value(args.get("mode").cloned().unwrap_or(json!("window"))).map_err(|e| format!("mode: {e}"))?;
        let ids = nnx_core::draft::pick_box(&self.doc()?.model, self.current_level(), [n(0)?, n(1)?, n(2)?, n(3)?], mode);
        Ok(json!({ "ids": ids.iter().map(|i| i.to_string()).collect::<Vec<_>>() }))
    }

    fn draft_xform(&mut self, args: &Value) -> Result<Value, String> {
        let ids = Self::ids_arg(args, "ids")?;
        self.guard_lock(&ids)?;
        let xf: nnx_core::draft::Xf = serde_json::from_value(args.get("xf").cloned().ok_or("xf is required")?).map_err(|e| format!("xf: {e}"))?;
        let copy = args.get("copy").and_then(Value::as_bool).unwrap_or(false);
        let level = ids.first().and_then(|i| self.doc().ok()?.model.get(i)?.level_id);
        let edit = nnx_core::draft::xform(&self.doc()?.model, &ids, xf, copy)?;
        let label = match (xf, copy) {
            (nnx_core::draft::Xf::Move { .. }, false) => "Move",
            (nnx_core::draft::Xf::Move { .. }, true) => "Copy",
            (nnx_core::draft::Xf::Rotate { .. }, _) => "Rotate",
            _ => "Mirror",
        };
        self.draft_commit(label, edit, level)
    }

    fn draft_align(&mut self, args: &Value) -> Result<Value, String> {
        let (r, t) = (id_arg(args, "reference")?, id_arg(args, "target")?);
        self.guard_lock(&[t])?;
        let level = self.doc()?.model.get(&t).and_then(|e| e.level_id);
        let edit = nnx_core::draft::align(&self.doc()?.model, r, t)?;
        self.draft_commit("Align wall", edit, level)
    }

    fn draft_split(&mut self, args: &Value) -> Result<Value, String> {
        let w = id_arg(args, "wall")?;
        self.guard_lock(&[w])?;
        let level = self.doc()?.model.get(&w).and_then(|e| e.level_id);
        let edit = nnx_core::draft::split(&self.doc()?.model, w, args.get("at").and_then(Value::as_i64).ok_or("at is required")?)?;
        self.draft_commit("Split wall", edit, level)
    }

    fn draft_trim(&mut self, args: &Value) -> Result<Value, String> {
        let side = |k: &str| -> Result<(nnx_core::Id, [f64; 2]), String> {
            let v = args.get(k).ok_or_else(|| format!("{k} is required"))?;
            Ok((id_arg(v, "id")?, [v.get("x").and_then(Value::as_f64).ok_or("x is required")?, v.get("y").and_then(Value::as_f64).ok_or("y is required")?]))
        };
        let (a, b) = (side("a")?, side("b")?);
        self.guard_lock(&[a.0, b.0])?;
        let level = self.doc()?.model.get(&a.0).and_then(|e| e.level_id);
        let edit = nnx_core::draft::trim_corner(&self.doc()?.model, a, b)?;
        self.draft_commit("Trim to corner", edit, level)
    }

    fn draft_delete(&mut self, args: &Value) -> Result<Value, String> {
        let ids = Self::ids_arg(args, "ids")?;
        self.guard_lock(&ids)?;
        let level = ids.first().and_then(|i| self.doc().ok()?.model.get(i)?.level_id);
        let edit = nnx_core::draft::delete(&self.doc()?.model, &ids)?;
        self.draft_commit("Delete", edit, level)
    }

    fn draft_props(&self, args: &Value) -> Result<Value, String> {
        let id = id_arg(args, "id")?;
        let mut v = nnx_core::draft::properties(&self.doc()?.model, id)?;
        if v["kind"] == "Wall" {
            v["assemblyId"] = json!(self.doc()?.model.get(&id).and_then(|e| e.props.get("assembly").cloned()));
            v["wallLocked"] = json!(self.project().is_some_and(|p| p.meta.locked_walls.contains(&id.to_string())));
        }
        Ok(v)
    }

    fn draft_set(&mut self, args: &Value) -> Result<Value, String> {
        let id = id_arg(args, "id")?;
        self.guard_lock(&[id])?;
        let key = args.get("key").and_then(Value::as_str).ok_or("key is required")?.to_string();
        let level = self.doc()?.model.get(&id).and_then(|e| e.level_id);
        let edit = nnx_core::draft::set_property(&self.doc()?.model, id, &key, args.get("value").ok_or("value is required")?)?;
        let mut v = self.draft_commit("Edit properties", edit, level)?;
        v["properties"] = self.draft_props(&json!({ "id": id.to_string() }))?;
        Ok(v)
    }

    /// The engineering report from the project's stored MEP and structural inputs: the printable HTML and the XLSX (base64).
    fn report(&self, args: &Value) -> Result<(String, String), String> {
        let arabic = args.get("lang").and_then(Value::as_str) != Some("en");
        let model = &self.doc()?.model;
        let p = self.project().ok_or("choose a layout first")?;
        let mep = p.mep.as_ref().map(|i| nnx_core::mep::compute(model, i));
        let sizing = p.sizing.as_ref().map(|i| nnx_core::sizing::compute(model, p.structure.bearing_kpa, &i.with_floor_load(&p.structure)));
        if mep.is_none() && sizing.is_none() {
            return Err("enter the MEP inputs (MEP tab) or the structural schedule inputs (Structural tab) and compute them first".into());
        }
        let data = nnx_core::report::ReportData { title: self.display_title(), date: utc_now().chars().take(10).collect(), mep: mep.as_ref(), sizing: sizing.as_ref() };
        let html = nnx_core::report::engineering_html(&data, arabic);
        let b64 = nnx_core::xlsx::base64_encode(&nnx_core::report::engineering_workbook(&data, arabic).to_bytes());
        Ok((html, b64))
    }

    /// The current model, for saving.
    pub fn model(&self) -> Result<&nnx_core::Model, String> {
        Ok(&self.doc()?.model)
    }

    /// The room table (name, target and actual area) read from the model's spaces.
    fn rooms_json(&self) -> Vec<Value> {
        let level = self.current_level();
        let labels = self.labels();
        let locked = self.project().map(|p| p.meta.locked_rooms).unwrap_or_default();
        self.doc
            .iter()
            .flat_map(|d| d.model.elements.values())
            .filter(|e| level.is_none() || e.level_id == level)
            .filter_map(|e| e.typed::<nnx_core::typed::Space>().ok())
            .map(|s| json!({ "key": s.requirement_key, "locked": locked.contains(&s.requirement_key), "name": nnx_core::annot::room_name(&s, &labels).unwrap_or(s.requirement_key.clone()), "func": s.func, "area": s.area, "target": s.target_area }))
            .collect()
    }

    /// What a saved file keeps besides the model. The project is a model element, so only view state is left here.
    pub fn settings(&self) -> Settings {
        let mut s = Settings::new();
        s.insert("show_structure".into(), json!(self.show_structure));
        s.insert("show_mep".into(), json!(self.show_mep));
        s
    }

    /// Opens a saved project: the model becomes the document (clean, nothing unsaved). A file written before the project
    /// was stored in the model carries its inputs as settings; they are moved into a project element, and anything that
    /// cannot be read is reported. Generated alternatives are not saved, so they are cleared.
    pub fn open(&mut self, mut model: nnx_core::Model, settings: &Settings) -> Result<Value, String> {
        if !model.elements.values().any(|e| matches!(e.kind.as_str(), "Space" | "Wall" | "Level" | "Project")) {
            return Err("this file has no plan (no rooms, walls or project)".into());
        }
        let mut warnings: Vec<String> = Vec::new();
        if nnx_core::project::find(&model).is_none() {
            let (project, notes) = legacy_project(settings);
            warnings.extend(notes);
            let cmd = nnx_core::project::create_command(&project).map_err(|e| format!("the saved inputs are not valid: {e}"))?;
            let mut empty = Document::lenient();
            empty.model = model;
            empty.commit("project", nnx_core::Origin::User, vec![cmd]).map_err(|e| e.to_string())?;
            model = empty.model;
        }
        let doc = Document::new(model);
        self.show_structure = settings.get("show_structure").and_then(Value::as_bool).unwrap_or(false);
        self.show_mep = settings.get("show_mep").and_then(Value::as_bool).unwrap_or(false);
        let so = struct_opts(&nnx_core::project::get(&doc.model)?.map(|p| p.structure).unwrap_or_default());
        // Say plainly what is wrong with the file instead of hiding it.
        warnings.extend(nnx_core::typed::validate_model(&doc.model).iter().take(5).map(|i| format!("{} {}: {}", i.kind, i.id, i.message)));
        {
            let mut probe = Document::new(doc.model.clone());
            match levels::settle(&mut probe, &so, None) {
                Ok((cmds, _)) if !cmds.is_empty() => warnings.push("the structure does not match the walls: press Sync".into()),
                Ok(_) => {}
                Err(e) => warnings.push(format!("the structure cannot be checked: {e}")),
            }
        }
        self.alternatives.clear();
        self.plan = None;
        self.draft = None;
        self.cur_level = 0;
        self.doc = Some(doc);
        Ok(json!({ "svg": self.render_or_null()?, "warnings": warnings, "rooms": self.rooms_json(), "levels": self.levels_json(), "settings": self.form_state(), "sbc": self.sbc_json() }))
    }

    /// Sets the project's climate zone (a number or a city) and puts that zone's wall and roof U limits into the insulation inputs,
    /// as one undoable step. The values are the guide's reproduction of SBC 602 Table 5.2 (see `presets`).
    fn zone_preset(&mut self, args: &Value) -> Result<Value, String> {
        let text = args.get("zone").map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string)).ok_or("zone is required (1, 2, 3 or a city)")?;
        let z = nnx_core::presets::zone_of(&text).and_then(nnx_core::presets::zone).ok_or_else(|| format!("'{text}' is not a zone (1, 2, 3) or a city of the table"))?;
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        nnx_core::project::update(doc, "climate zone", |p| {
            p.meta.climate_zone = Some(text.trim().to_string());
            let lib = p.mep.get_or_insert_with(Default::default).insulation.get_or_insert_with(Default::default);
            lib.u_limits.insert("wall".into(), z.wall_u_max);
            lib.u_limits.insert("roof".into(), z.roof_u_max);
            Ok(())
        })?;
        Ok(json!({ "zone": z.zone, "preset": nnx_core::presets::presets_json()["zones"][(z.zone - 1) as usize].clone(), "settings": self.form_state(), "sbc": self.sbc_json() }))
    }

    /// Analyses a plot outline before there is a project: `{ polygon_m, edges, rule, floors }`. Returns the setbacks, the buildable
    /// polygon, the building rectangle and the site plan drawing. Every problem is listed.
    fn site_analyze(args: &Value) -> Result<Value, String> {
        let input: nnx_core::site::SiteInput = serde_json::from_value(args.clone()).map_err(|e| format!("plot: {e}"))?;
        let r = nnx_core::site::analyse(&input).map_err(|e| e.join("; "))?;
        let svg = nnx_core::site::site_svg(&r, "Site plan");
        Ok(json!({ "result": r, "svg": svg, "rule_source": nnx_core::site::RULE_SOURCE }))
    }

    /// Reviews an existing IFC file without importing it: `{ path }`. A Revit file, an archive or ifcXML is refused with the
    /// reason; the reply lists units, counts, storeys, spaces with area and source, and findings.
    fn ifc_review(args: &Value) -> Result<Value, String> {
        let path = args.get("path").and_then(Value::as_str).filter(|p| !p.trim().is_empty()).ok_or("ifc review: the path of the IFC file is missing")?;
        let bytes = std::fs::read(path).map_err(|e| format!("ifc review: cannot read {path}: {e}"))?;
        let r = nnx_core::ifc_review::review_ifc(&bytes).map_err(|e| format!("ifc review: {e}"))?;
        Ok(json!({ "review": r }))
    }

    /// Checks that the guest door and the family door can be kept apart on the street side of a level:
    /// `{ rooms: [{ key, func, zone, x, y, w, h }], envelope: { w, h }, entry, guest_funcs?, min_separation_mm? }` (mm).
    /// `guest_funcs` defaults to `["majlis"]`. Without `min_separation_mm` the distance is reported and the check is skipped.
    fn entrances_check(args: &Value) -> Result<Value, String> {
        let (layout, envelope) = Self::layout_from_args(args, "entrances")?;
        let entry = Self::side_from_str(args.get("entry").and_then(Value::as_str), "entrances: entry")?;
        let guest_funcs: Vec<String> = match args.get("guest_funcs").filter(|v| !v.is_null()) {
            Some(v) => serde_json::from_value(v.clone()).map_err(|e| format!("entrances: guest_funcs must be a list of room functions: {e}"))?,
            None => vec!["majlis".to_string()],
        };
        let min = match args.get("min_separation_mm").filter(|v| !v.is_null()) {
            Some(v) => Some(v.as_i64().ok_or("entrances: min_separation_mm must be a whole number of millimetres")?),
            None => None,
        };
        let report = nnx_core::entrances::check_entrances(&layout, &envelope, entry, &guest_funcs, min)?;
        Ok(json!({ "entrances": report }))
    }

    /// Reads `{ rooms: [{ key, func, zone, x, y, w, h }], envelope: { w, h } }` (mm) into a layout and its envelope.
    /// `tag` prefixes every error so the user knows which command refused the input.
    fn layout_from_args(args: &Value, tag: &str) -> Result<(nnx_core::layout::Layout, Rect), String> {
        use nnx_core::layout::{Breakdown, Layout, PlacedRoom};
        let num = |v: &Value, k: &str, what: &str| v.get(k).and_then(Value::as_i64).ok_or_else(|| format!("{tag}: {what} '{k}' must be a whole number of millimetres"));
        let rooms_in = args.get("rooms").and_then(Value::as_array).filter(|r| !r.is_empty()).ok_or_else(|| format!("{tag}: rooms are required"))?;
        let mut rooms = Vec::new();
        for r in rooms_in {
            let key = r.get("key").and_then(Value::as_str).ok_or_else(|| format!("{tag}: every room needs a key"))?;
            let zone = match r.get("zone").and_then(Value::as_str) {
                Some("public") => Zone::Public,
                Some("private") => Zone::Private,
                Some("service") => Zone::Service,
                other => return Err(format!("{tag}: room '{key}' has an unknown zone {other:?}")),
            };
            let rect = Rect { x: num(r, "x", key)?, y: num(r, "y", key)?, w: num(r, "w", key)?, h: num(r, "h", key)? };
            rooms.push(PlacedRoom {
                key: key.to_string(),
                func: r.get("func").and_then(Value::as_str).ok_or_else(|| format!("{tag}: room '{key}' has no func"))?.to_string(),
                zone,
                needs_light: false,
                rect,
                area_m2: rect.area_m2(),
                target_m2: rect.area_m2(),
                deviation_pct: 0.0,
            });
        }
        let env = args.get("envelope").ok_or_else(|| format!("{tag}: envelope is required"))?;
        let envelope = Rect { x: 0, y: 0, w: num(env, "w", "envelope")?, h: num(env, "h", "envelope")? };
        Ok((Layout { rooms, cost: 0.0, breakdown: Breakdown::default(), reserve_m2: 0.0 }, envelope))
    }

    fn side_from_str(v: Option<&str>, what: &str) -> Result<Side, String> {
        match v {
            Some("south") => Ok(Side::South),
            Some("north") => Ok(Side::North),
            Some("east") => Ok(Side::East),
            Some("west") => Ok(Side::West),
            other => Err(format!("{what} must be south, north, east or west, not {other:?}")),
        }
    }

    /// Checks the privacy of private rooms against neighbouring plots:
    /// `{ rooms, envelope, neighbours: [{ side, distance_mm }], private_funcs, min_distance_mm? }` (mm). `neighbours` lists
    /// only the sides that face a neighbour with the distance to that boundary; `private_funcs` has no default.
    /// Without `min_distance_mm` the distances are reported and the check is skipped.
    fn privacy_check(args: &Value) -> Result<Value, String> {
        use nnx_core::privacy::NeighbourSide;
        let (layout, envelope) = Self::layout_from_args(args, "privacy")?;
        let list = args.get("neighbours").and_then(Value::as_array).filter(|n| !n.is_empty()).ok_or("privacy: neighbours are required (each side that faces a neighbour with its distance_mm)")?;
        let mut neighbours = Vec::new();
        for n in list {
            let side = Self::side_from_str(n.get("side").and_then(Value::as_str), "privacy: neighbour side")?;
            let distance_mm = n.get("distance_mm").and_then(Value::as_i64).ok_or_else(|| format!("privacy: the {side:?} neighbour needs distance_mm as a whole number of millimetres"))?;
            neighbours.push(NeighbourSide { side, distance_mm });
        }
        let private_funcs: Vec<String> = serde_json::from_value(args.get("private_funcs").cloned().unwrap_or(Value::Null)).map_err(|e| format!("privacy: private_funcs must be a list of room functions: {e}"))?;
        let min = match args.get("min_distance_mm").filter(|v| !v.is_null()) {
            Some(v) => Some(v.as_i64().ok_or("privacy: min_distance_mm must be a whole number of millimetres")?),
            None => None,
        };
        let report = nnx_core::privacy::check_privacy(&layout, &envelope, &neighbours, &private_funcs, min)?;
        Ok(json!({ "privacy": report }))
    }

    /// Reports the sleeping rooms that take the afternoon sun on a west outside wall: `{ rooms, envelope, sleeping_funcs }`
    /// (mm). `sleeping_funcs` has no default.
    fn sun_check(args: &Value) -> Result<Value, String> {
        let (layout, envelope) = Self::layout_from_args(args, "sun")?;
        let sleeping_funcs: Vec<String> = serde_json::from_value(args.get("sleeping_funcs").cloned().unwrap_or(Value::Null)).map_err(|e| format!("sun: sleeping_funcs must be a list of room functions: {e}"))?;
        let report = nnx_core::sun::check_sun(&layout, &envelope, &sleeping_funcs)?;
        Ok(json!({ "sun": report }))
    }

    /// Compares layout alternatives by how many large structural spans each asks for: `{ envelope: { w, h }, max_span_mm,
    /// alternatives: [{ label?, rooms: [{ key, func, zone, x, y, w, h }] }] }` (mm). `max_span_mm` is a design input with no
    /// default; without it nothing is compared. Ranks on counted support lines, not on prices.
    fn span_compare(args: &Value) -> Result<Value, String> {
        let alts_in = args.get("alternatives").and_then(Value::as_array).filter(|a| !a.is_empty()).ok_or("span: alternatives are required")?;
        let limit = match args.get("max_span_mm").filter(|v| !v.is_null()) {
            Some(v) => Some(v.as_i64().ok_or("span: max_span_mm must be a whole number of millimetres")?),
            None => None,
        };
        let mut layouts = Vec::new();
        for (i, a) in alts_in.iter().enumerate() {
            let label = a.get("label").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("#{}", i + 1));
            let mut one = json!({ "rooms": a.get("rooms").cloned().unwrap_or(Value::Null) });
            one["envelope"] = args.get("envelope").cloned().unwrap_or(Value::Null);
            let (layout, _) = Self::layout_from_args(&one, "span")?;
            layouts.push((label, layout));
        }
        let refs: Vec<(String, &nnx_core::layout::Layout)> = layouts.iter().map(|(l, lay)| (l.clone(), lay)).collect();
        let report = nnx_core::spanalt::compare(&refs, limit)?;
        Ok(json!({ "span": report }))
    }

    /// Takes wall weights down onto the beams under them: `{ walls: [{ key, a: [x, y], b: [x, y], thickness_mm, height_mm,
    /// unit_weight_kn_m3, finish_kn_m2_per_face?, finished_faces?, openings?: [{ offset_mm, width_mm }] }], beams: [{ key, a, b }] }`
    /// (mm). Thickness, height and unit weight have no defaults. Simply supported beams; a take-down, not a design.
    fn wall_load(args: &Value) -> Result<Value, String> {
        let walls: Vec<nnx_core::wallload::WallIn> = serde_json::from_value(args.get("walls").cloned().unwrap_or(Value::Null))
            .map_err(|e| format!("wall load: walls need key, a, b ([x, y] whole mm), thickness_mm, height_mm and unit_weight_kn_m3: {e}"))?;
        let beams: Vec<nnx_core::wallload::BeamIn> = serde_json::from_value(args.get("beams").cloned().unwrap_or(Value::Null))
            .map_err(|e| format!("wall load: beams need key, a and b ([x, y] whole mm): {e}"))?;
        let report = nnx_core::wallload::take_down(&walls, &beams)?;
        Ok(json!({ "wall_load": report }))
    }

    /// Serviceability screen: `{ e_mpa?, beams?: [{ key, span_mm, b_mm, h_mm, support: simple|fixed|cantilever, w_kn_m,
    /// cracked_factor?, long_term_factor?, limit_span_ratio? }], storeys?: [{ key, height_mm, shear_kn, columns: [{ count,
    /// width_mm, depth_mm }], stiffness_factor? }], max_drift_ratio?, footings?: [{ key, load_kn, width_mm, length_mm, es_mpa, nu,
    /// influence }], pairs?: [{ a, b, distance_mm }], max_settlement_mm?, max_angular_distortion? }`. The concrete modulus is
    /// required for beams and storeys; a missing limit leaves its check `unchecked`. Preliminary, not a design.
    fn drift_check(args: &Value) -> Result<Value, String> {
        let input: nnx_core::drift::DriftInput = serde_json::from_value(args.clone())
            .map_err(|e| format!("drift check: inputs are malformed (see the field list for beams, storeys, footings and pairs): {e}"))?;
        let report = nnx_core::drift::check(&input)?;
        Ok(json!({ "drift": report }))
    }

    /// Mat and pile-group screen: `{ screen?: { isolated_area_m2, building_area_m2, max_coverage_ratio? }, mat?: { x_mm, y_mm,
    /// w_mm, l_mm, thickness_mm?, unit_weight_kn_m3?, allowable_kpa? }, columns?: [{ key, x_mm, y_mm, load_kn, mx_knm?, my_knm? }],
    /// piles?: [{ key, load_kn, mx_knm?, my_knm?, capacity_kn, uplift_capacity_kn?, spacing_mm?, positions?: [[x, y]],
    /// min_spacing_mm? }] }`. Every capacity and limit is the caller's input; a missing limit leaves its check `unchecked`.
    /// Preliminary, not a design.
    fn deep_foundation_check(args: &Value) -> Result<Value, String> {
        let input: nnx_core::deepfound::DeepInput = serde_json::from_value(args.clone())
            .map_err(|e| format!("deep foundation check: inputs are malformed (see the field list for screen, mat, columns and piles): {e}"))?;
        let report = nnx_core::deepfound::check(&input)?;
        Ok(json!({ "deep_foundation": report }))
    }

    /// Explosive thermal spalling screen: `{ elements: [{ key, kind: column|beam|slab|wall|footing, b_mm, h_mm, cover_mm, fc_mpa,
    /// exposed_sides, moisture_pct?, pp_fibre_kg_m3? }], max_moisture_pct?, max_fc_mpa?, max_cover_mm?, min_pp_fibre_kg_m3? }`.
    /// Every threshold is the caller's input with its own source; a missing one leaves its check `unchecked`. Preliminary.
    fn spalling_check(args: &Value) -> Result<Value, String> {
        let input: nnx_core::spalling::SpallInput = serde_json::from_value(args.clone())
            .map_err(|e| format!("spalling check: inputs are malformed (see the field list for elements and limits): {e}"))?;
        let report = nnx_core::spalling::check(&input)?;
        Ok(json!({ "spalling": report }))
    }

    /// Luminaire count and grid layout for target illuminance (lumen method): `{ rooms: [{ key, x_mm, y_mm, w_mm, h_mm, target_lux,
    /// target_source, luminaire, mounting_height_mm? }], luminaires: [{ key, lumens, utilisation, maintenance, watts? }],
    /// max_spacing_to_height?, max_w_per_m2? }`. Targets (with source) and photometric factors are the caller's; none is assumed.
    /// Preliminary average-illuminance screen, not a lighting design.
    fn lighting_plan(args: &Value) -> Result<Value, String> {
        let input: nnx_core::lighting::LightInput = serde_json::from_value(args.clone())
            .map_err(|e| format!("lighting plan: inputs are malformed (see the field list for rooms and luminaires): {e}"))?;
        let report = nnx_core::lighting::check(&input)?;
        Ok(json!({ "lighting": report }))
    }

    /// Distribution-board load schedules: `{ voltage_v, power_factor, three_phase, demand_factors: { kind: factor }, breaker_sizes_a: [],
    /// breaker_margin, boards: [{ key, ways, bus_rating_a? }], loads: [{ id, board, kind, watts, phases: 1|3, phase?, description? }],
    /// max_imbalance_pct?, min_spare_ways? }`. Every value is the caller's; none is assumed. Preliminary schedule, not an electrical design.
    fn panel_schedule(args: &Value) -> Result<Value, String> {
        let input: nnx_core::panels::PanelInput = serde_json::from_value(args.clone())
            .map_err(|e| format!("panel schedule: inputs are malformed (see the field list for boards and loads): {e}"))?;
        let report = nnx_core::panels::check(&input)?;
        Ok(json!({ "panels": report }))
    }

    /// Cable routes and voltage drop: `{ voltage_v, power_factor, three_phase, derating_factor, allowance_mm, catalogue: [{ key,
    /// area_mm2, r_ohm_km, x_ohm_km, ampacity_a, source }], obstacles?: [{ key, x_mm, y_mm, w_mm, h_mm }], clearance_mm?,
    /// circuits: [{ id, from: {x_mm,y_mm}, to, via?, rise_mm?, watts | current_a, phases: 1|3, cable? }], max_drop_pct? }`. Every
    /// value is the caller's; none is assumed. Preliminary check, not an electrical design.
    fn cable_check(args: &Value) -> Result<Value, String> {
        let input: nnx_core::cables::CableInput = serde_json::from_value(args.clone())
            .map_err(|e| format!("cable check: inputs are malformed (see the field list for catalogue and circuits): {e}"))?;
        let report = nnx_core::cables::check(&input)?;
        Ok(json!({ "cables": report }))
    }

    /// Insulation options for the wall or roof compared by cost and yearly cooling saving: `{ scope, degree_hours,
    /// degree_hours_source, cop, tariff_per_kwh, study_years, baseline_insulation_cost_per_m2?, options: [{ name, lambda,
    /// thicknesses_mm, price_per_m3, install_per_m2, source }] }`. The baseline is the project's stored `wall` or `roof` assembly
    /// and its U limit; the area comes from the model. Every figure is the caller's; a missing one is refused by name.
    fn insulation_options(&self, args: &Value) -> Result<Value, String> {
        let input: nnx_core::insulopt::Input = serde_json::from_value(args.clone())
            .map_err(|e| format!("insulation options: inputs are malformed (see the field list for the scope, energy figures and options): {e}"))?;
        let lib = self
            .project()
            .and_then(|p| p.mep)
            .and_then(|m| m.insulation)
            .ok_or("insulation options: enter the insulation assemblies in the MEP inputs first (the base assembly is the baseline)")?;
        let report = nnx_core::insulopt::compare(self.model()?, &lib, &input)?;
        Ok(json!({ "insulation_options": report }))
    }

    /// Yearly electricity bill from stated loads and a tariff: `{ loads: [{ name, kw, quantity, demand_factor, hours_per_day,
    /// active_months, source }], tariff: { blocks: [{ up_to_kwh?, rate_per_kwh }], fixed_per_month, vat_pct, source, source_date },
    /// generation_kwh_per_month?: [12], generation_source? }`. The tariff and every load figure are the caller's; none is assumed.
    /// Preliminary estimate, not a utility bill.
    fn electricity_bill(args: &Value) -> Result<Value, String> {
        let input: nnx_core::elecbill::Input = serde_json::from_value(args.clone())
            .map_err(|e| format!("electricity bill: inputs are malformed (see the field list for loads and tariff): {e}"))?;
        let report = nnx_core::elecbill::estimate(&input)?;
        Ok(json!({ "electricity_bill": report }))
    }

    /// Preliminary energy rating: `{ end_uses: [{ name, kwh_per_year, source }], conditioned_area_m2, area_source,
    /// bands: [{ label, up_to_kwh_m2? }] (best first, last has no bound), scheme_source, scheme_date,
    /// generation_kwh_per_year?, generation_source?, emission_factor_kg_per_kwh?, emission_source? }`. The rating scale is the
    /// caller's; none is built in. Not an official energy certificate.
    fn energy_certificate(args: &Value) -> Result<Value, String> {
        let input: nnx_core::energycert::Input = serde_json::from_value(args.clone())
            .map_err(|e| format!("energy certificate: inputs are malformed (see the field list for end uses and bands): {e}"))?;
        let report = nnx_core::energycert::estimate(&input)?;
        Ok(json!({ "energy_certificate": report }))
    }

    /// Ground and roof water tanks plus the transfer pump: `{ persons, litres_per_person_day, demand_source, ground_storage_days,
    /// roof_storage_days, usable_fraction, ground_footprint_m2?, roof_footprint_m2?, max_water_depth_m?, transfer: { fill_hours,
    /// ground_low_level_m, roof_high_level_m, main: { length_m, diameter_mm, hazen_c, minor_k }, pump_efficiency, motor_efficiency,
    /// motor_margin, motor_sizes_kw?, max_velocity_mps? }, gravity?: { roof_low_level_m, highest_fixture_level_m, residual_head_m,
    /// peak_flow_lps, main } }`. Every value is the caller's; a missing limit leaves its check `unchecked`. Preliminary, not a
    /// plumbing design.
    fn water_storage_check(args: &Value) -> Result<Value, String> {
        let input: nnx_core::pumps::WaterInput = serde_json::from_value(args.clone())
            .map_err(|e| format!("water storage check: inputs are malformed (see the field list for tanks, transfer and gravity): {e}"))?;
        let report = nnx_core::pumps::check(&input)?;
        Ok(json!({ "water": report }))
    }

    /// Fire detection, sprinkler and call-point layout with the sprinkler water demand: `{ rooms: [{ key, x_mm, y_mm, w_mm, h_mm,
    /// detector?, sprinkler?, zone?, ceiling_height_mm? }], detectors?: [{ key, coverage_m2, max_spacing_mm, source,
    /// max_wall_distance_mm?, max_ceiling_height_mm? }], sprinklers?: [same], exits?: [{ key, x_mm, y_mm }],
    /// max_call_point_distance_mm?, max_detectors_per_zone?, max_zone_area_m2?, water?: { design_density_mm_min, design_area_m2,
    /// duration_min, source, hose_allowance_lpm?, available_storage_m3? } }`. Every figure is the caller's; a missing limit leaves
    /// its check `unchecked`. Preliminary, not a fire-protection design.
    fn fire_protection_check(args: &Value) -> Result<Value, String> {
        let input: nnx_core::firesys::FireInput = serde_json::from_value(args.clone())
            .map_err(|e| format!("fire protection check: inputs are malformed (see the field list for rooms, devices, exits and water): {e}"))?;
        let report = nnx_core::firesys::check(&input)?;
        Ok(json!({ "fire": report }))
    }

    /// Fire-safety egress: `{ rooms: [{ key, x_mm, y_mm, w_mm, h_mm, exits?: [key] }], exits?: [{ key, x_mm, y_mm, clear_width_mm? }],
    /// corridors?: [{ key, x_mm, y_mm, w_mm, h_mm, min_width_mm? }], max_travel_distance_mm?, min_corridor_width_mm?,
    /// min_exit_width_mm?, min_exits_per_room?, source }`. Every limit is the caller's and `source` is required with any limit; a
    /// missing limit leaves its check `unchecked`. Travel distance is a straight-line lower bound. Preliminary, not a fire design.
    fn egress_check(args: &Value) -> Result<Value, String> {
        let input: nnx_core::egress::EgressInput = serde_json::from_value(args.clone())
            .map_err(|e| format!("egress check: inputs are malformed (see the field list for rooms, exits, corridors and limits): {e}"))?;
        let report = nnx_core::egress::check(&input)?;
        Ok(json!({ "egress": report }))
    }

    /// Accessibility for persons with disabilities: `{ doors?: [{ key, clear_width_mm?, threshold_height_mm? }],
    /// corridors?: [{ key, w_mm, h_mm, min_width_mm? }], rooms?: [{ key, w_mm, h_mm, accessible? }],
    /// ramps?: [{ key, rise_mm, run_mm, width_mm? }], min_door_clear_width_mm?, max_threshold_height_mm?, min_corridor_width_mm?,
    /// min_turning_diameter_mm?, max_ramp_slope?, min_ramp_width_mm?, source }`. Every limit is the caller's and `source` is
    /// required with any limit; a missing limit leaves its check `unchecked`. Preliminary, not an accessibility audit.
    fn access_check(args: &Value) -> Result<Value, String> {
        let input: nnx_core::access::AccessInput = serde_json::from_value(args.clone())
            .map_err(|e| format!("accessibility check: inputs are malformed (see the field list for doors, corridors, rooms, ramps and limits): {e}"))?;
        let report = nnx_core::access::check(&input)?;
        Ok(json!({ "access": report }))
    }

    /// Shear-wall core around lifts and stairs:`{ walls: [{ key, x_mm, y_mm, w_mm, h_mm }], shafts?: [{ key, kind: lift|stair,
    /// x_mm, y_mm, w_mm, h_mm, open_sides?, min_clear_x_mm?, min_clear_y_mm? }], e_mpa?, stiffness_factor?, levels?: [{ height_mm,
    /// fx_kn?, fy_kn? }], axial_kn?, min_wall_thickness_mm?, max_drift_ratio?, max_top_ratio?, max_shear_mpa?,
    /// max_compression_mpa?, max_tension_mpa? }`. `e_mpa` is required when `levels` are given; a missing limit leaves its check
    /// `unchecked`. Preliminary, not a design.
    fn shear_core_check(args: &Value) -> Result<Value, String> {
        let input: nnx_core::shearcore::CoreInput = serde_json::from_value(args.clone())
            .map_err(|e| format!("shear core check: inputs are malformed (see the field list for walls, shafts and levels): {e}"))?;
        let report = nnx_core::shearcore::check(&input)?;
        Ok(json!({ "shear_core": report }))
    }

    /// Compares structural alternatives on concrete and steel: `{ criterion: steel_kg|concrete_m3|cost, baseline?, prices?: {
    /// concrete_per_m3, steel_per_kg: { value, source, date }, currency }, alternatives: [{ label, checks_passed?, members: [{ key,
    /// kind, count?, b_mm, h_mm, length_mm, bars?: [{ dia_mm, count, length_mm? }], links?: { dia_mm, spacing_mm, length_mm } }] }] }`.
    /// The criterion has no default and a price needs its source and date. Quantities only: not a design.
    fn savings_compare(args: &Value) -> Result<Value, String> {
        let alts: Vec<nnx_core::savings::AlternativeIn> = serde_json::from_value(args.get("alternatives").cloned().unwrap_or(Value::Null))
            .map_err(|e| format!("savings: alternatives need label and members (key, kind, b_mm, h_mm, length_mm, bars, links): {e}"))?;
        let prices: Option<nnx_core::savings::Prices> = match args.get("prices") {
            None | Some(Value::Null) => None,
            Some(v) => Some(serde_json::from_value(v.clone()).map_err(|e| format!("savings: prices need concrete_per_m3 and steel_per_kg ({{ value, source, date }}) and currency: {e}"))?),
        };
        let baseline = match args.get("baseline") {
            None | Some(Value::Null) => None,
            Some(v) => Some(v.as_u64().ok_or("savings: baseline must be the index of an alternative (0, 1, ...)")? as usize),
        };
        let report = nnx_core::savings::compare(&alts, args.get("criterion").and_then(Value::as_str), baseline, prices.as_ref())?;
        Ok(json!({ "savings": report }))
    }

    /// Arranges furniture of real sizes in one room and reports whether it fits: `{ room: { w, h }, min_walkway_mm, pieces: [{ key,
    /// name?, w_mm, d_mm, count?, against_wall?, rotatable? }], doors?: [{ wall: south|north|west|east, offset_mm, width_mm }],
    /// door_clear_mm?, max_occupancy_share? }` (mm). The walkway has no default; doors need `door_clear_mm`. Access from the doors is
    /// `unchecked` without doors and the occupancy check is `unchecked` without a limit.
    fn furniture_fit(args: &Value) -> Result<Value, String> {
        let spec: nnx_core::furnfit::FitSpec = serde_json::from_value(args.clone())
            .map_err(|e| format!("furniture fit: room needs w and h, pieces need key, w_mm and d_mm (count, against_wall and rotatable are optional), doors need wall, offset_mm and width_mm, and sizes are whole millimetres: {e}"))?;
        let report = nnx_core::furnfit::arrange(&spec)?;
        Ok(json!({ "furniture_fit": report }))
    }

    /// Classes habitable rooms by cross ventilation against the prevailing wind:
    /// `{ rooms, envelope, wind_from, wind_source, habitable_funcs }` (mm). The wind side, its source and the functions
    /// have no default.
    fn wind_check(args: &Value) -> Result<Value, String> {
        let (layout, envelope) = Self::layout_from_args(args, "wind")?;
        let wind_from = match args.get("wind_from").filter(|v| !v.is_null()) {
            Some(v) => Some(Self::side_from_str(v.as_str(), "wind: wind_from")?),
            None => None,
        };
        let source = args.get("wind_source").and_then(Value::as_str).unwrap_or("");
        let funcs: Vec<String> = serde_json::from_value(args.get("habitable_funcs").cloned().unwrap_or(Value::Null)).map_err(|e| format!("wind: habitable_funcs must be a list of room functions: {e}"))?;
        let report = nnx_core::wind::check_wind(&layout, &envelope, wind_from, source, &funcs)?;
        Ok(json!({ "wind": report }))
    }

    /// Checks a non-rectangular room (L, T or any horizontal-and-vertical outline):
    /// `{ rooms, envelope, key, outline: [{ x, y }], min_wall_mm? }` (mm). Reports area, perimeter, shape, an exact rectangle
    /// decomposition, the notch left in the bounding box and one wall segment per edge with what lies outside it. Without
    /// `min_wall_mm` the segment-length check is reported as `unchecked`.
    fn room_shape_check(args: &Value) -> Result<Value, String> {
        let (layout, envelope) = Self::layout_from_args(args, "room shape")?;
        let key = args.get("key").and_then(Value::as_str).ok_or("room shape: 'key' (the room being reshaped) is required")?;
        let outline: Vec<nnx_core::courtyard::Pt> = serde_json::from_value(args.get("outline").cloned().unwrap_or(Value::Null)).map_err(|e| format!("room shape: outline must be a list of {{ x, y }} vertices in whole millimetres: {e}"))?;
        let min_wall = match args.get("min_wall_mm") {
            None | Some(Value::Null) => None,
            Some(v) => Some(v.as_i64().ok_or("room shape: 'min_wall_mm' must be a whole number of millimetres")?),
        };
        let report = nnx_core::roomshape::check_room_shape(&layout, &envelope, key, &outline, min_wall)?;
        Ok(json!({ "room_shape": report }))
    }

    /// Checks a mezzanine or partial floor: `{ rooms, envelope, outline: [{ x, y }], storey_height_mm, level_mm, slab_mm,
    /// min_clear_below_mm?, min_clear_above_mm?, max_area_share? }` (mm). The vertical set-out is required; each limit left out
    /// is reported as `unchecked`. Reports area, share of the lower floor, clear heights, covered rooms and open edges.
    fn mezzanine_check(args: &Value) -> Result<Value, String> {
        let (layout, envelope) = Self::layout_from_args(args, "mezzanine")?;
        let outline: Vec<nnx_core::courtyard::Pt> = serde_json::from_value(args.get("outline").cloned().unwrap_or(Value::Null)).map_err(|e| format!("mezzanine: outline must be a list of {{ x, y }} vertices in whole millimetres: {e}"))?;
        for name in ["storey_height_mm", "level_mm", "slab_mm"] {
            if args.get(name).and_then(Value::as_i64).is_none() {
                return Err(format!("mezzanine: '{name}' is required (whole millimetres)"));
            }
        }
        let spec: nnx_core::mezzanine::MezzanineSpec = serde_json::from_value(args.clone()).map_err(|e| format!("mezzanine: limits must be numbers (min_clear_*_mm in whole millimetres, max_area_share between 0 and 1): {e}"))?;
        let report = nnx_core::mezzanine::check_mezzanine(&layout, &envelope, &outline, &spec)?;
        Ok(json!({ "mezzanine": report }))
    }

    /// Checks the usable roof: `{ envelope: { w, h }, items: [{ key, kind: room|canopy|tank, x, y, w, h, height_mm,
    /// water_depth_mm? }], parapet_mm?, min_parapet_mm?, min_edge_setback_mm?, min_gap_mm?, min_clear_mm?, max_room_share? }`
    /// (mm). Each limit left out is reported as `unchecked`; tanks need `water_depth_mm`.
    fn rooftop_check(args: &Value) -> Result<Value, String> {
        let env = args.get("envelope").ok_or("rooftop: envelope is required")?;
        let side = |k: &str| env.get(k).and_then(Value::as_i64).ok_or_else(|| format!("rooftop: envelope '{k}' must be a whole number of millimetres"));
        let roof = Rect { x: 0, y: 0, w: side("w")?, h: side("h")? };
        let spec: nnx_core::rooftop::RooftopSpec = serde_json::from_value(args.clone())
            .map_err(|e| format!("rooftop: items need key, kind (room, canopy or tank), x, y, w, h and height_mm in whole millimetres, and limits must be numbers (max_room_share between 0 and 1): {e}"))?;
        let report = nnx_core::rooftop::check_rooftop(&roof, &spec)?;
        Ok(json!({ "rooftop": report }))
    }

    /// Checks a basement level: `{ envelope: { w, h }, outline: { x, y, w, h }, storey_mm, slab_mm, depth_mm, spaces: [{ key,
    /// kind: parking|storage|service|ramp|core, x, y, w, h, stalls?, run_along?: x|y }], water_table_depth_mm?, min_clear_mm?,
    /// max_ramp_slope_pct?, min_ramp_width_mm?, min_area_per_stall_m2?, min_exits?, max_travel_mm? }` (mm). A parking space needs
    /// `stalls`, a ramp needs `run_along`; each limit left out is reported as `unchecked`.
    fn basement_check(args: &Value) -> Result<Value, String> {
        let env = args.get("envelope").ok_or("basement: envelope is required")?;
        let side = |k: &str| env.get(k).and_then(Value::as_i64).ok_or_else(|| format!("basement: envelope '{k}' must be a whole number of millimetres"));
        let bounds = Rect { x: 0, y: 0, w: side("w")?, h: side("h")? };
        let spec: nnx_core::basement::BasementSpec = serde_json::from_value(args.clone())
            .map_err(|e| format!("basement: give outline (x, y, w, h), storey_mm, slab_mm and depth_mm in whole millimetres; spaces need key, kind (parking, storage, service, ramp or core), x, y, w, h (a ramp also run_along x or y, parking also stalls); limits must be numbers: {e}"))?;
        let report = nnx_core::basement::check_basement(&bounds, &spec)?;
        Ok(json!({ "basement": report }))
    }

    /// Packs 90-degree parking stalls into an area: `{ envelope: { w, h }, area: { x, y, w, h }, stall_width_mm, stall_length_mm,
    /// aisle_width_mm, min_stall_width_mm?, min_stall_length_mm?, min_aisle_width_mm?, required_stalls?, turn?: { vehicle_width_mm,
    /// wheelbase_mm, front_overhang_mm, centre_radius_mm, lane_width_mm? } }` (mm). Sizes have no defaults; each limit left out
    /// is reported as `unchecked`.
    fn parking_plan(args: &Value) -> Result<Value, String> {
        let env = args.get("envelope").ok_or("parking: envelope is required")?;
        let side = |k: &str| env.get(k).and_then(Value::as_i64).ok_or_else(|| format!("parking: envelope '{k}' must be a whole number of millimetres"));
        let bounds = Rect { x: 0, y: 0, w: side("w")?, h: side("h")? };
        let spec: nnx_core::parking::ParkingSpec = serde_json::from_value(args.clone())
            .map_err(|e| format!("parking: give area (x, y, w, h), stall_width_mm, stall_length_mm and aisle_width_mm in whole millimetres; turn needs vehicle_width_mm, wheelbase_mm, front_overhang_mm and centre_radius_mm; limits must be numbers: {e}"))?;
        let report = nnx_core::parking::plan_parking(&bounds, &spec)?;
        Ok(json!({ "parking": report }))
    }

    /// Fits sloping ground to surveyed spot levels and checks platforms, steps and ramps: `{ envelope: { w, h }, spots: [{ x, y, z_mm }],
    /// platforms?: [{ key, x, y, w, h, level_mm }], ramps?: [{ key, from, to, run_mm, width_mm }], max_site_slope_pct?,
    /// max_plane_residual_mm?, max_platform_ground_range_mm?, max_cut_mm?, max_fill_mm?, max_step_mm?, max_ramp_slope_pct?,
    /// min_ramp_width_mm? }` (mm). At least three spot levels off one line are required; each limit left out is `unchecked`.
    fn terrain_check(args: &Value) -> Result<Value, String> {
        let env = args.get("envelope").ok_or("terrain: envelope is required")?;
        let side = |k: &str| env.get(k).and_then(Value::as_i64).ok_or_else(|| format!("terrain: envelope '{k}' must be a whole number of millimetres"));
        let bounds = Rect { x: 0, y: 0, w: side("w")?, h: side("h")? };
        let spec: nnx_core::terrain::TerrainSpec = serde_json::from_value(args.clone())
            .map_err(|e| format!("terrain: give spots (x, y, z_mm), platforms (key, x, y, w, h, level_mm) and ramps (key, from, to, run_mm, width_mm) in whole millimetres; limits must be numbers: {e}"))?;
        let report = nnx_core::terrain::check_terrain(&bounds, &spec)?;
        Ok(json!({ "terrain": report }))
    }

    /// Checks a courtyard of any simple outline: `{ rooms, envelope, outline: [{ x, y }] }` (mm). Reports area, perimeter, a
    /// rectangle decomposition (rectilinear outlines only) and the contact with each neighbouring room.
    fn courtyard_check(args: &Value) -> Result<Value, String> {
        let (layout, envelope) = Self::layout_from_args(args, "courtyard")?;
        let outline: Vec<nnx_core::courtyard::Pt> = serde_json::from_value(args.get("outline").cloned().unwrap_or(Value::Null)).map_err(|e| format!("courtyard: outline must be a list of {{ x, y }} vertices in whole millimetres: {e}"))?;
        let report = nnx_core::courtyard::check_courtyard(&layout, &envelope, &outline)?;
        Ok(json!({ "courtyard": report }))
    }

    /// Reads plot boundary candidates from a DXF drawing: `{ path, units?, index? }`. A drawing without a unit needs `units`;
    /// several boundaries need `index`. A DWG is refused with the reason.
    fn plot_import_dxf(args: &Value) -> Result<Value, String> {
        let path = args.get("path").and_then(Value::as_str).filter(|p| !p.trim().is_empty()).ok_or("plot import: the path of the DXF file is missing")?;
        let bytes = std::fs::read(path).map_err(|e| format!("plot import: cannot read {path}: {e}"))?;
        let units = args.get("units").and_then(Value::as_str);
        let index = match args.get("index") {
            None | Some(Value::Null) => None,
            Some(v) => Some(v.as_u64().ok_or("plot import: index must be a whole number")? as usize),
        };
        let r = nnx_core::plot_import::import_dxf(&bytes, units, index).map_err(|e| format!("plot import: {e}"))?;
        Ok(json!({ "result": r }))
    }

    /// Reads the plot boundary from surveyed corner coordinates: `{ text | path, order?, crs?, units? }`. `text` (pasted rows) or
    /// `path` (a CSV or text file) gives the rows; without a header `order` (`en` or `ne`) is required; a system other than UTM
    /// needs `units`.
    /// Learns the user's usual room sizes from earlier projects: `{ paths: [".nnx file", ...] }`. A file that cannot be read or
    /// has no project is listed in `skipped` with the reason; it does not stop the others. Pass the returned `preferences` to
    /// `brief_clarify` as `prefs` to start suggested room areas from them.
    fn prefs_learn(args: &Value) -> Result<Value, String> {
        let paths = args.get("paths").and_then(Value::as_array).filter(|a| !a.is_empty()).ok_or("preferences: give the paths of the earlier projects")?;
        let mut projects = Vec::new();
        let mut skipped = Vec::new();
        for v in paths {
            let Some(path) = v.as_str() else {
                skipped.push(json!({ "path": v, "reason": "the path is not text" }));
                continue;
            };
            match nnx_core::persist::load(std::path::Path::new(path)).map_err(|e| e.to_string()).and_then(|m| nnx_core::project::get(&m)) {
                Ok(Some(p)) => projects.push(p),
                Ok(None) => skipped.push(json!({ "path": path, "reason": "the file has no NanoArch project" })),
                Err(e) => skipped.push(json!({ "path": path, "reason": e })),
            }
        }
        Ok(json!({ "preferences": nnx_core::prefs::learn(&projects), "skipped": skipped }))
    }

    /// The clarifying conversation: `{ draft, field?, answer? }`. With a field and an answer the answer is applied first (an
    /// unreadable one is an error and the draft is returned unchanged by the page). The reply carries the draft, the next
    /// question (`null` when the brief is complete) and every question still open.
    fn brief_clarify(args: &Value) -> Result<Value, String> {
        use nnx_core::{briefai::BriefDraft, clarify};
        let mut draft: BriefDraft = serde_json::from_value(args.get("draft").cloned().ok_or("draft is required")?).map_err(|e| format!("the draft is not valid: {e}"))?;
        if let Some(field) = args.get("field").and_then(Value::as_str) {
            let answer = args.get("answer").and_then(Value::as_str).ok_or("answer is required with field")?;
            clarify::apply_answer(&mut draft, field, answer)?;
            if field == "rooms"
                && let Some(p) = args.get("prefs").filter(|p| !p.is_null())
            {
                let prefs: nnx_core::prefs::Preferences = serde_json::from_value(p.clone()).map_err(|e| format!("the preferences are not valid: {e}"))?;
                nnx_core::prefs::apply(&mut draft, &prefs);
            }
        }
        Ok(json!({ "draft": draft, "next": clarify::next_question(&draft), "missing": clarify::missing(&draft), "complete": clarify::is_complete(&draft) }))
    }

    /// Reads a room-area schedule from an Excel workbook (`path`, or `xlsx_b64` from the page). With a `draft`, the rooms replace
    /// the draft's and the clarification state comes back with it.
    fn areas_import(args: &Value) -> Result<Value, String> {
        use nnx_core::clarify;
        let bytes = match (args.get("xlsx_b64").and_then(Value::as_str).filter(|t| !t.trim().is_empty()), args.get("path").and_then(Value::as_str).filter(|p| !p.trim().is_empty())) {
            (Some(b), _) => nnx_core::xlsx::base64_decode(b).map_err(|e| format!("areas import: {e}"))?,
            (None, Some(p)) => std::fs::read(p).map_err(|e| format!("areas import: cannot read {p}: {e}"))?,
            (None, None) => return Err("areas import: give the workbook's path, or its content as xlsx_b64".into()),
        };
        let table = nnx_core::areatable::import(&bytes, args.get("sheet").and_then(Value::as_str)).map_err(|e| format!("areas import: {e}"))?;
        let Some(d) = args.get("draft").filter(|d| !d.is_null()) else {
            return Ok(json!({ "table": table }));
        };
        let mut draft: nnx_core::briefai::BriefDraft = serde_json::from_value(d.clone()).map_err(|e| format!("areas import: the draft is not valid: {e}"))?;
        draft.rooms = table.rooms.clone();
        draft.notes.push(format!("rooms read from the sheet '{}' of the workbook: {} room(s), {:.1} m² with an area", table.sheet, table.rooms.len(), table.total_area_m2));
        Ok(json!({ "table": table, "draft": draft, "next": clarify::next_question(&draft), "missing": clarify::missing(&draft), "complete": clarify::is_complete(&draft) }))
    }

    fn plot_import_survey(args: &Value) -> Result<Value, String> {
        let text_arg = args.get("text").and_then(Value::as_str).filter(|t| !t.trim().is_empty());
        let path_arg = args.get("path").and_then(Value::as_str).filter(|p| !p.trim().is_empty());
        let text = match (text_arg, path_arg) {
            (Some(t), _) => t.to_string(),
            (None, Some(p)) => {
                let bytes = std::fs::read(p).map_err(|e| format!("survey import: cannot read {p}: {e}"))?;
                String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}').to_string()
            }
            (None, None) => return Err("survey import: give the corner rows as text, or the path of the file".into()),
        };
        let field = |k: &str| args.get(k).and_then(Value::as_str);
        let r = nnx_core::plot_import::import_survey(&text, field("order"), field("crs"), field("units")).map_err(|e| format!("survey import: {e}"))?;
        Ok(json!({ "result": r }))
    }

    /// Every advanced structural analysis of the open project in one answer (inputs are the project's own).
    fn structural_report(&self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let model = &self.doc()?.model;
        let s = &p.meta.structural;
        let thick = p.sizing.as_ref().and_then(|z| z.footing_thickness_mm);
        let tank_results: Vec<nnx_core::tanks::TankResult> = s.tanks.iter().map(|t| nnx_core::tanks::design(t, &s.tank_design)).collect();
        let mut pieces = Vec::new();
        let schedule = p.sizing.as_ref().map(|z| nnx_core::sizing::compute(model, p.structure.bearing_kpa, &z.with_floor_load(&p.structure)));
        if let Some(r) = &schedule {
            pieces.extend(nnx_core::cutting::pieces_from_schedule(&r.bars));
        }
        for t in &tank_results {
            pieces.extend(nnx_core::cutting::pieces_from_tank(&t.name, &t.bars));
        }
        Ok(json!({
            "inputs": s,
            "grid": nnx_core::gridopt::analyze(model, &s.grid),
            "slabs": nnx_core::slabadv::advise(model, &s.slabs),
            "punching": nnx_core::punching::check(model, s, thick, p.structure.dead_kpa, p.structure.live_kpa),
            "merge": nnx_core::merger::plan(model, &s.merge, thick),
            "tanks": tank_results,
            "cutting": nnx_core::cutting::optimize(&pieces, s.cutting.stock_mm, s.cutting.lap_mm),
            "bars_in_schedule": schedule.map_or(0, |r| r.bars.len()),
        }))
    }

    /// Replaces the structural inputs (one undoable project edit).
    fn structural_set(&mut self, args: &Value) -> Result<Value, String> {
        let s: nnx_core::structx::StructuralInputs = serde_json::from_value(args.get("structural").cloned().ok_or("structural is required")?).map_err(|e| format!("structural inputs: {e}"))?;
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        let out = nnx_core::project::update(doc, "structural inputs", |p| {
            p.meta.structural = s.clone();
            Ok(())
        })?;
        let mut v = self.structural_report()?;
        v["changed"] = json!(out.changed);
        Ok(v)
    }

    /// Moves the columns the grid optimizer proposes (with their footings and beam ends) as one undoable step.
    fn grid_apply(&mut self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let inp = p.meta.structural.grid.clone();
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        let rep = nnx_core::gridopt::analyze(&doc.model, &inp);
        let cmds = nnx_core::gridopt::apply(&doc.model, &rep, inp.snap_mm.unwrap_or(0.0).round() as i64);
        let moved = rep.proposals.iter().filter(|x| x.status == "move").count();
        if cmds.is_empty() {
            return Err("no column to move: see the grid report for why".into());
        }
        doc.commit("optimise the column grid", nnx_core::Origin::User, cmds).map_err(|e| e.to_string())?;
        let mut v = self.structural_report()?;
        v["moved"] = json!(moved);
        v["svg"] = self.render_or_null()?;
        Ok(v)
    }

    /// Merges the footings into combined footings or a raft as one undoable step.
    fn merge_apply(&mut self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let thick = p.sizing.as_ref().and_then(|z| z.footing_thickness_mm);
        let inp = p.meta.structural.merge.clone();
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        let plan = nnx_core::merger::plan(&doc.model, &inp, thick);
        if plan.groups.is_empty() {
            return Err(if plan.missing.is_empty() { plan.notes.first().cloned().unwrap_or_else(|| "nothing to merge".into()) } else { format!("enter: {}", plan.missing.join("; ")) });
        }
        let cmds = nnx_core::merger::apply(&doc.model, &plan);
        doc.commit("merge footings", nnx_core::Origin::User, cmds).map_err(|e| e.to_string())?;
        let mut v = self.structural_report()?;
        v["merged"] = json!(plan.groups.iter().map(|g| g.label.clone()).collect::<Vec<_>>());
        v["svg"] = self.render_or_null()?;
        Ok(v)
    }

    /// The analytical model as a DXF drawing or an IFC4 structural analysis view.
    fn export_analytical(&self, args: &Value) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let a = nnx_core::analytical::build(&self.doc()?.model, p.meta.structural.load_factor)?;
        let title = self.display_title();
        match args.get("kind").and_then(Value::as_str).unwrap_or("dxf") {
            "dxf" => Ok(json!({ "data": nnx_core::analytical::dxf(&a, &title), "name": "analytical-model.dxf", "nodes": a.nodes.len(), "members": a.members.len(), "surfaces": a.surfaces.len(), "loads": a.loads.len(), "notes": a.notes })),
            "ifc" => Ok(json!({ "data": nnx_core::analytical::ifc(&a, &title, &utc_now()), "name": "analytical-model.ifc", "nodes": a.nodes.len(), "members": a.members.len(), "surfaces": a.surfaces.len(), "loads": a.loads.len(), "notes": a.notes })),
            other => Err(format!("unknown export kind '{other}' (dxf or ifc)")),
        }
    }

    /// Early-design analyses (`kind`: solar, screen, compare, stair, ramp). Every limit comes from the caller; a missing one is named.
    fn analysis(&self, args: &Value) -> Result<Value, String> {
        use nnx_core::analysis as an;
        let n = |k: &str| args.get(k).and_then(Value::as_f64);
        let need = |k: &str| n(k).ok_or_else(|| format!("{k} is missing"));
        let kind = args.get("kind").and_then(Value::as_str).ok_or("kind is missing")?;
        let ser = |v: Result<serde_json::Value, serde_json::Error>| v.map_err(|e| e.to_string());
        match kind {
            "solar" => ser(serde_json::to_value(an::solar(need("latitude_deg")?, n("window_height_m"))?)),
            "screen" => ser(serde_json::to_value(an::screen_height(need("eye_m")?, need("viewer_to_screen_m")?, need("screen_to_target_m")?, need("target_m")?)?)),
            "stair" => ser(serde_json::to_value(an::stair(need("floor_height_mm")?, need("risers")? as u32, need("tread_mm")?, need("target_mm")?, need("tolerance_mm")?)?)),
            "ramp" => ser(serde_json::to_value(an::ramp(need("rise_mm")?, need("run_mm")?, n("max_slope_pct"))?)),
            "compare" => {
                let alts: Vec<(f64, Vec<(String, f64)>)> = self.alternatives.iter().map(|b| (b.levels[0].cost, b.levels.iter().flat_map(|l| l.rooms.iter().map(|r| (r.func.clone(), r.area_m2))).collect())).collect();
                ser(serde_json::to_value(an::compare(&alts, need("max_circulation_pct")?)?))
            }
            other => Err(format!("unknown analysis '{other}'")),
        }
    }

    /// Refuses an edit of a locked room or wall (or of an opening on a locked wall).
    fn guard_lock(&self, ids: &[nnx_core::Id]) -> Result<(), String> {
        let (Some(p), Some(doc)) = (self.project(), self.doc.as_ref()) else { return Ok(()) };
        for id in ids {
            let Some(e) = doc.model.get(id) else { continue };
            let wall_locked = |wid: &nnx_core::Id| p.meta.locked_walls.contains(&wid.to_string());
            match e.kind.as_str() {
                "Wall" if wall_locked(&e.id) => return Err("that wall is locked: unlock it first".into()),
                "Space" => {
                    if let Ok(s) = e.typed::<nnx_core::typed::Space>()
                        && p.meta.locked_rooms.contains(&s.requirement_key)
                    {
                        return Err(format!("room '{}' is locked: unlock it first", s.requirement_key));
                    }
                }
                "Door" | "Window" if e.host_id.is_some_and(|h| wall_locked(&h)) => return Err("the wall that carries this opening is locked: unlock it first".into()),
                _ => {}
            }
        }
        Ok(())
    }

    /// Locks or unlocks a room (`key`) or a wall (`id`) as one undoable project edit.
    fn lock(&mut self, args: &Value) -> Result<Value, String> {
        let kind = args.get("kind").and_then(Value::as_str).ok_or("kind (room or wall) is required")?;
        let on = args.get("locked").and_then(Value::as_bool).ok_or("locked (true or false) is required")?;
        let name = match kind {
            "room" => {
                let doc = self.doc.as_ref().ok_or("choose a layout first")?;
                let key = args.get("key").and_then(Value::as_str).ok_or("key is required")?.to_string();
                let known = doc.model.elements.values().filter(|e| e.kind == "Space").filter_map(|e| e.typed::<nnx_core::typed::Space>().ok()).any(|s| s.requirement_key == key);
                if !known {
                    return Err(format!("there is no room '{key}' in the plan"));
                }
                let base = key.split('@').next().unwrap_or("");
                let in_brief = self.project().and_then(|p| p.brief).is_some_and(|b| b.rooms.iter().any(|q| q.key == base));
                if !in_brief {
                    return Err(format!("'{key}' is circulation or stair space generated to fill the plan, not a room of the brief: it cannot be locked"));
                }
                key
            }
            "wall" => {
                let doc = self.doc.as_ref().ok_or("choose a layout first")?;
                let id = id_arg(args, "id")?;
                if doc.model.get(&id).is_none_or(|e| e.kind != "Wall") {
                    return Err("that element is not a wall".into());
                }
                id.to_string()
            }
            other => return Err(format!("unknown lock kind '{other}'")),
        };
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        let out = nnx_core::project::update(doc, if on { "lock" } else { "unlock" }, |p| {
            let list = if kind == "room" { &mut p.meta.locked_rooms } else { &mut p.meta.locked_walls };
            list.retain(|k| *k != name);
            if on {
                list.push(name.clone());
            }
            list.sort();
            Ok(())
        })?;
        Ok(json!({ "changed": out.changed, "locked_rooms": self.project().map(|p| p.meta.locked_rooms).unwrap_or_default(), "locked_walls": self.project().map(|p| p.meta.locked_walls).unwrap_or_default() }))
    }

    /// Generates again from the stored brief with the locked rooms kept at their exact rectangles.
    fn regenerate(&mut self, args: &Value) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let mut brief = p.brief.clone().ok_or("this project has no brief")?;
        let doc = self.doc.as_ref().ok_or("choose a layout first")?;
        let mut locked = serde_json::Map::new();
        for s in doc.model.elements.values().filter(|e| e.kind == "Space").filter_map(|e| e.typed::<nnx_core::typed::Space>().ok()) {
            if p.meta.locked_rooms.contains(&s.requirement_key) {
                locked.insert(s.requirement_key.clone(), json!({ "x": s.x, "y": s.y, "w": s.w, "h": s.h }));
            }
        }
        if let Some(missing) = p.meta.locked_rooms.iter().find(|k| !locked.contains_key(*k)) {
            return Err(format!("the locked room '{missing}' is not in the plan"));
        }
        brief.seed = Some(args.get("seed").and_then(Value::as_u64).unwrap_or(brief.seed.unwrap_or(1).wrapping_add(1)));
        let args2 = json!({ "brief": brief, "struct": p.structure, "meta": p.meta, "limits": p.limits, "locked": locked });
        let mut out = self.generate(&args2)?;
        out["locked_rooms"] = json!(p.meta.locked_rooms);
        Ok(out)
    }

    /// Landscape layout of the project's site (inputs in `meta.siteLayout`).
    fn site_layout_cmd(&self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let input = p.meta.site_layout.clone().ok_or("no site layout inputs: enter the wall, carport, guard room, pool or courtyard sizes")?;
        let r = p.site_result().ok_or("the project has no brief")?.map_err(|e| e.join("; "))?;
        let l = nnx_core::site_layout::layout(&r, &input)?;
        Ok(json!({ "svg": nnx_core::site::site_svg_with(&r, &self.display_title(), Some(&l)), "layout": l }))
    }

    fn furniture_cmd(&self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let f = p.meta.furniture.clone().ok_or("no furniture inputs: enter the minimum clear walkway")?;
        let scene = nnx_core::model3d::build(&self.doc()?.model);
        let rep = nnx_core::furniture::plan(&scene, &f)?;
        let level = self.level_list().get(self.cur_level).map(|_| self.cur_level as i64);
        let svg = nnx_core::furniture::svg(&rep, if scene.levels.is_empty() { None } else { level });
        Ok(json!({ "svg": svg, "report": rep, "params": nnx_core::furniture::PARAMS }))
    }

    /// Reports the expansion saved in the project (`meta.expansion`, set through `project_set`) against the built model.
    fn expansion_check(&self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let x = p.meta.expansion.clone().ok_or("no expansion is saved: enter a vertical or a horizontal expansion")?;
        let scene = nnx_core::model3d::build(&self.doc()?.model);
        let report = nnx_core::expansion::report(&scene, &x, p.plot_area_m2())?;
        Ok(json!({ "expansion": report }))
    }

    fn vertical_cmd(&self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let v = p.meta.vertical.clone().ok_or("no vertical inputs: enter the elevator, basement or roof annex")?;
        let scene = nnx_core::model3d::build(&self.doc()?.model);
        serde_json::to_value(nnx_core::vertical::report(&scene, &v)?).map_err(|e| e.to_string())
    }

    /// Every compliance audit of the open project in one answer.
    fn compliance_report(&self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let model = &self.doc()?.model;
        let changes = p.meta.baseline.as_ref().map(|b| nnx_core::revisions::changes(model, b)).unwrap_or_default();
        let (a, m, r) = nnx_core::revisions::counts(&changes);
        Ok(json!({
            "inputs": p.meta.compliance,
            "thermal": nnx_core::thermal::report(model, &p),
            "access": nnx_core::audits::accessibility(model, &p),
            "ventilation": nnx_core::audits::ventilation(model, &p),
            "cantilever": nnx_core::audits::cantilever(model, &p),
            "malath": nnx_core::malath::report(&p.meta.compliance),
            "baseline": p.meta.baseline.as_ref().map(|b| json!({ "name": b.name, "date": nnx_core::revisions::date(b.created_unix), "elements": b.elements.len() })),
            "history": p.meta.revision_history,
            "changes": changes,
            "change_counts": { "added": a, "modified": m, "removed": r },
            "rooms": nnx_core::audits::room_keys(model),
        }))
    }

    /// Replaces the compliance inputs (one undoable project edit).
    fn compliance_set(&mut self, args: &Value) -> Result<Value, String> {
        let c: nnx_core::compliance::ComplianceInputs = serde_json::from_value(args.get("compliance").cloned().ok_or("compliance is required")?).map_err(|e| format!("compliance inputs: {e}"))?;
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        let out = nnx_core::project::update(doc, "compliance inputs", |p| {
            p.meta.compliance = c.clone();
            Ok(())
        })?;
        let mut v = self.compliance_report()?;
        v["changed"] = json!(out.changed);
        Ok(v)
    }

    /// Sets the status of one checklist item.
    fn malath_set(&mut self, args: &Value) -> Result<Value, String> {
        let id = args.get("id").and_then(Value::as_str).ok_or("id is required")?.to_string();
        let status = args.get("status").and_then(Value::as_str).ok_or("status is required")?.to_string();
        let note = args.get("note").and_then(Value::as_str).unwrap_or("").to_string();
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        nnx_core::project::update(doc, "checklist", |p| nnx_core::malath::set_status(&mut p.meta.compliance, &id, &status, &note))?;
        self.compliance_report()
    }

    /// Takes a baseline snapshot of the model; the previous baseline is closed into the revision history.
    fn baseline_set(&mut self, args: &Value) -> Result<Value, String> {
        let name = args.get("name").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).ok_or("give the baseline a name")?.to_string();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        let model = doc.model.clone();
        nnx_core::project::update(doc, "baseline", |p| {
            nnx_core::revisions::set_baseline(&model, p, &name, now);
            Ok(())
        })?;
        let mut v = self.compliance_report()?;
        v["svg"] = self.render_or_null()?;
        Ok(v)
    }

    /// The preliminary compliance report as a printable document.
    fn certificate(&self, args: &Value) -> Result<Value, String> {
        let arabic = args.get("lang").and_then(Value::as_str) != Some("en");
        let p = self.project().ok_or("choose a layout first")?;
        let model = &self.doc()?.model;
        let changes = p.meta.baseline.as_ref().map(|b| nnx_core::revisions::changes(model, b)).unwrap_or_default();
        let d = nnx_core::certificate::CertData { model, project: &p, changes: &changes, date: utc_now().chars().take(10).collect() };
        Ok(json!({ "html": nnx_core::certificate::certificate_html(&d, arabic), "name": "balady_certificate.pdf" }))
    }

    /// The permit package as numbered A3 sheets with a title-block header and the engineer's stamp box: `{ engineer: { name,
    /// licence_no, licensing_body, discipline? }, project_no, sheet_prefix, revision, issued_for?, lang? }`. Nothing is defaulted:
    /// a missing input is named. The stamp has an empty seal and signature area; the package is never marked approved.
    /// `lang: "both"` returns the Arabic and English packages as a matched pair (`ar`, `en`, shared `sheets`).
    fn permit_pack(&self, args: &Value) -> Result<Value, String> {
        let input: nnx_core::permitpack::Input = serde_json::from_value(args.clone())
            .map_err(|e| format!("permit pack: inputs are malformed (engineer {{ name, licence_no, licensing_body, discipline? }}, project_no, sheet_prefix, revision, issued_for?): {e}"))?;
        let lang = args.get("lang").and_then(Value::as_str);
        let arabic = lang != Some("en");
        let date: String = utc_now().chars().take(10).collect();
        let title = self.display_title();
        let ctx = nnx_core::permitpack::Context { model: &self.doc()?.model, title: &title, labels: self.labels(), arabic, show_structure: self.show_structure, show_mep: self.show_mep, date: &date };
        if lang == Some("both") {
            let b = nnx_core::permitpack::build_bilingual(&ctx, &input)?;
            let part = |p: &nnx_core::permitpack::Pack, name: &str| json!({ "html": p.html, "name": name, "sheets": p.sheets, "notes": p.notes });
            return Ok(json!({ "bilingual": true, "ar": part(&b.ar, "permit_pack_ar.pdf"), "en": part(&b.en, "permit_pack_en.pdf"), "sheets": b.sheets, "notes": b.notes }));
        }
        let pack = nnx_core::permitpack::build(&ctx, &input)?;
        Ok(json!({ "html": pack.html, "name": "permit_pack.pdf", "sheets": pack.sheets, "notes": pack.notes }))
    }

    /// The engineer's sign-off form with a statement of responsibility (one A4 page): `{ engineer: { name, licence_no,
    /// licensing_body, discipline? }, project_no, revision, scope: [..], statement, statement_source, lang? }`. The statement
    /// wording is the user's input and is never written here; signature, seal and date boxes stay empty.
    fn signoff_form(&self, args: &Value) -> Result<Value, String> {
        let input: nnx_core::signoff::Input = serde_json::from_value(args.clone())
            .map_err(|e| format!("sign-off form: inputs are malformed (engineer {{ name, licence_no, licensing_body, discipline? }}, project_no, revision, scope [text], statement, statement_source): {e}"))?;
        let arabic = args.get("lang").and_then(Value::as_str) != Some("en");
        let date: String = utc_now().chars().take(10).collect();
        let title = self.display_title();
        self.doc()?;
        let form = nnx_core::signoff::build(&title, &date, arabic, &input)?;
        Ok(json!({ "html": form.html, "name": "engineer_signoff.pdf", "notes": form.notes }))
    }

    /// The missing permit-documents checklist (read-only): `{ required: [{ key, name, group? }], list_source, submitted: [{ key,
    /// reference, date? }] }`. The required list is the user's, with its source; none is built in. Returns the rows, the
    /// unmatched submitted keys, the counts, the verdict and a CSV (`name`: `permit_documents.csv`).
    fn permit_documents_check(&self, args: &Value) -> Result<Value, String> {
        let input: nnx_core::doccheck::Input = serde_json::from_value(args.clone())
            .map_err(|e| format!("documents checklist: inputs are malformed (required [{{ key, name, group? }}], list_source, submitted [{{ key, reference, date? }}]): {e}"))?;
        self.doc()?;
        let report = nnx_core::doccheck::check(&input)?;
        let mut v = serde_json::to_value(&report).map_err(|e| e.to_string())?;
        v["name"] = json!("permit_documents.csv");
        Ok(v)
    }

    /// The room finishes schedule (read-only): `{ finishes: [{ code, name, surface: floor|wall|ceiling, source }], assignments:
    /// [{ key | func, floor?, wall?, ceiling? }], wall_height_mm? (required with a wall finish), level? }`. The catalogue is the
    /// user's; areas are measured from the model's rooms. Returns the room rows, the totals per finish, the unassigned surfaces
    /// and a CSV (`name`: `room_finishes.csv`).
    fn room_finishes_schedule(&self, args: &Value) -> Result<Value, String> {
        let input: nnx_core::roomfinish::Input = serde_json::from_value(args.clone())
            .map_err(|e| format!("room finishes schedule: inputs are malformed (finishes [{{ code, name, surface, source }}], assignments [{{ key|func, floor?, wall?, ceiling? }}], wall_height_mm?, level?): {e}"))?;
        let schedule = nnx_core::roomfinish::build(&self.doc()?.model, &input)?;
        let mut v = serde_json::to_value(&schedule).map_err(|e| e.to_string())?;
        v["name"] = json!("room_finishes.csv");
        Ok(v)
    }

    /// The bathroom / kitchen detail (read-only): `{ room (requirement key), level?, title?, fixtures: [{ key, kind?, x_mm, y_mm,
    /// w_mm, d_mm, group?, source }], doors?: [{ wall, offset_mm, width_mm, swing_mm? }], min_clearance_mm?, clearance_source? }`.
    /// Fixtures are the user's entries; returns the dimensioned SVG, the per-fixture wall gaps and the fit / overlap / door swing /
    /// clearance checks (`unchecked` when their inputs are absent).
    fn wet_room_detail(&self, args: &Value) -> Result<Value, String> {
        let input: nnx_core::wetroom::Input = serde_json::from_value(args.clone())
            .map_err(|e| format!("wet room detail: inputs are malformed (room, fixtures [{{ key, x_mm, y_mm, w_mm, d_mm, source }}], doors? [{{ wall, offset_mm, width_mm, swing_mm? }}], min_clearance_mm?, clearance_source?): {e}"))?;
        let detail = nnx_core::wetroom::build(&self.doc()?.model, &input)?;
        serde_json::to_value(&detail).map_err(|e| e.to_string())
    }

    /// The suspended ceiling plan (read-only): `{ room (requirement key), level?, title?, ceiling_height_mm, soffit_height_mm?,
    /// tile_w_mm?, tile_d_mm?, devices: [{ key, kind?, x_mm, y_mm (centre), w_mm, d_mm, recess_mm?, source }], min_spacing_mm?,
    /// spacing_source?, min_ceiling_height_mm?, headroom_source? }`. Heights, devices and limits are the user's entries; returns the
    /// reflected ceiling plan SVG, the per-device wall gaps, device counts, the tile summary and the fit / overlap / spacing /
    /// plenum / headroom checks (`unchecked` when their inputs are absent).
    fn ceiling_plan(&self, args: &Value) -> Result<Value, String> {
        let input: nnx_core::ceiling::Input = serde_json::from_value(args.clone())
            .map_err(|e| format!("ceiling plan: inputs are malformed (room, ceiling_height_mm, soffit_height_mm?, tile_w_mm?, tile_d_mm?, devices [{{ key, x_mm, y_mm, w_mm, d_mm, recess_mm?, source }}], min_spacing_mm?, spacing_source?, min_ceiling_height_mm?, headroom_source?): {e}"))?;
        let plan = nnx_core::ceiling::build(&self.doc()?.model, &input)?;
        serde_json::to_value(&plan).map_err(|e| e.to_string())
    }

    /// The axes and detailed dimensions plan (read-only): `{ level?, title?, axes_x: [{ label, pos_mm }], axes_y: [...] }` or
    /// `{ from_columns: true, cluster_mm }`, with `tolerance_mm?` and `tolerance_source?`. Returns the axes with bay spacings and
    /// overall dimensions, each column's offset from its nearest axes, each orthogonal wall's offset from the nearest parallel
    /// axis, the columns-on-axes check (`unchecked` without a tolerance) and the SVG.
    fn axes_plan(&self, args: &Value) -> Result<Value, String> {
        let input: nnx_core::axes::Input = serde_json::from_value(args.clone())
            .map_err(|e| format!("axes plan: inputs are malformed (level?, axes_x [{{ label, pos_mm }}], axes_y, from_columns?, cluster_mm?, tolerance_mm?, tolerance_source?): {e}"))?;
        let plan = nnx_core::axes::build(&self.doc()?.model, &input)?;
        serde_json::to_value(&plan).map_err(|e| e.to_string())
    }

    /// The stair detail (read-only): `{ rise_mm, riser_count, going_mm, width_mm, nosing_mm?, landing_after_riser?, landing_length_mm?,
    /// room?, level?, run_axis?, limits?: { min_riser_mm?, max_riser_mm?, ..., source }, title? }`. Every figure is the user's;
    /// returns the numbered step table, the flights, the section + plan SVG and the riser / going / width / landing / 2R+G / pitch /
    /// fit checks (`unchecked` when their limit is absent).
    fn stair_detail(&self, args: &Value) -> Result<Value, String> {
        let input: nnx_core::stairdetail::Input = serde_json::from_value(args.clone())
            .map_err(|e| format!("stair detail: inputs are malformed (rise_mm, riser_count, going_mm, width_mm, nosing_mm?, landing_after_riser?, landing_length_mm?, room?, run_axis?, limits?): {e}"))?;
        let detail = nnx_core::stairdetail::build(&self.doc()?.model, &input)?;
        serde_json::to_value(&detail).map_err(|e| e.to_string())
    }

    /// The number audit file of the open project: every number with the source and date entered beside it, and the list of
    /// numbers that have none. Read-only; returns the entries, the counts and a CSV text (`name`: `number_audit.csv`).
    fn number_audit(&self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let file = nnx_core::numaudit::audit(&p)?;
        let csv = nnx_core::numaudit::to_csv(&file);
        let mut v = serde_json::to_value(&file).map_err(|e| e.to_string())?;
        v["csv"] = json!(csv);
        v["name"] = json!("number_audit.csv");
        Ok(v)
    }

    /// Every advanced MEP analysis (solar, smart home, Wi-Fi, CCTV, EV, balancing, greywater, head loss) of the open project.
    fn mepx_report(&self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let model = &self.doc()?.model;
        let ext = &p.meta.mep_ext;
        let mi = p.mep.clone().unwrap_or_default();
        let rep = nnx_core::mep::compute(model, &mi);
        Ok(json!({
            "inputs": ext,
            "solar": nnx_core::solarpv::size(model, &p),
            "smart": nnx_core::smarthome::topology(model, &ext.smart),
            "wifi": nnx_core::wireless::wifi(model, &ext.wifi),
            "cctv": nnx_core::wireless::cctv(model, &ext.cctv),
            "ev": nnx_core::evload::ev(model, &mi, &rep, &ext.ev),
            "balance": nnx_core::evload::balance(&mi, &rep, &ext.ev, &ext.balance),
            "grey": nnx_core::greywater::design(model, &mi, &rep, &ext.grey),
            "hydra": nnx_core::hydraulics::size(model, &mi, &rep, &ext.hydra),
            "rooms": nnx_core::audits::room_keys(model),
        }))
    }

    /// Replaces the advanced MEP inputs (one undoable project edit).
    fn mepx_set(&mut self, args: &Value) -> Result<Value, String> {
        let e: nnx_core::mepx::MepExtInputs = serde_json::from_value(args.get("mepExt").cloned().ok_or("mepExt is required")?).map_err(|e| format!("MEP pack inputs: {e}"))?;
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        let out = nnx_core::project::update(doc, "MEP pack inputs", |p| {
            p.meta.mep_ext = e.clone();
            Ok(())
        })?;
        let mut v = self.mepx_report()?;
        v["changed"] = json!(out.changed);
        Ok(v)
    }

    /// The network plan of one floor: Wi-Fi signal, access points, CCTV coverage, smart devices and bus.
    fn mepx_svg(&self, args: &Value) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let model = &self.doc()?.model;
        let ext = &p.meta.mep_ext;
        let floor = args.get("floor").and_then(Value::as_u64).unwrap_or(1) as u32;
        let mut w = ext.wifi.clone();
        w.floor = Some(floor);
        let wifi = nnx_core::wireless::wifi(model, &w);
        let cctv = nnx_core::wireless::cctv(model, &ext.cctv);
        let smart = nnx_core::smarthome::topology(model, &ext.smart);
        let svg = nnx_core::wireless::network_svg(model, floor, (!wifi.cells.is_empty()).then_some(&wifi), (!ext.cctv.cameras.is_empty()).then_some((&ext.cctv, &cctv)), (smart.panel.is_some()).then_some((&ext.smart, &smart)), ext.wifi.threshold_dbm)?;
        Ok(json!({ "svg": svg, "name": "network-plan.svg" }))
    }

    /// The 3D scene, with the finish colours the project chose and the as-built deviations by element.
    fn model3d(&self) -> Result<Value, String> {
        let model = &self.doc()?.model;
        let mut v = serde_json::to_value(nnx_core::model3d::build_styled(model, self.style())).map_err(|e| e.to_string())?;
        if let Some(p) = self.project() {
            let col = |kind: &str, id: &Option<String>| id.as_deref().and_then(|i| nnx_core::finishes::find(kind, i)).map(|f| f.color);
            v["finishes"] = json!({ "facade": col("facade", &p.meta.finishes.facade), "floor": col("floor", &p.meta.finishes.floor) });
            if !p.meta.as_built.rows.is_empty() {
                v["deviations"] = json!(nnx_core::asbuilt::compare(model, &p.meta.as_built).by_element);
            }
        }
        Ok(v)
    }

    /// Ground shadows through the year: `{ latitude_deg, hours: [..], days: [..] | monthly: true, north_offset_deg, probes?: [[x, y]] }`.
    /// Latitude, hours, the days (or the explicit `monthly`: the 21st of every month) and the north offset are the user's; none is defaulted.
    fn sun_shadows(&self, args: &Value) -> Result<Value, String> {
        let scene = nnx_core::model3d::build_styled(&self.doc()?.model, self.style());
        let latitude_deg = args.get("latitude_deg").and_then(Value::as_f64).ok_or("sun_shadows: latitude_deg is missing")?;
        let north_offset_deg = args.get("north_offset_deg").and_then(Value::as_f64).ok_or("sun_shadows: north_offset_deg is missing (0 when the plan's up is true north)")?;
        let hours: Vec<f64> = serde_json::from_value(args.get("hours").cloned().unwrap_or(Value::Null)).map_err(|e| format!("sun_shadows: hours must be a list of solar hours: {e}"))?;
        let monthly = args.get("monthly").and_then(Value::as_bool).unwrap_or(false);
        let days: Vec<u32> = match (monthly, args.get("days")) {
            (true, Some(d)) if !d.is_null() => return Err("sun_shadows: give either days or monthly, not both".to_string()),
            (true, _) => nnx_core::shadows::MONTH_21ST.to_vec(),
            (false, d) => serde_json::from_value(d.cloned().unwrap_or(Value::Null)).map_err(|e| format!("sun_shadows: days must be a list of days of the year (or monthly: true): {e}"))?,
        };
        let probes: Vec<[f64; 2]> = match args.get("probes") {
            Some(p) if !p.is_null() => serde_json::from_value(p.clone()).map_err(|e| format!("sun_shadows: probes must be a list of [x, y] metres: {e}"))?,
            _ => Vec::new(),
        };
        let input = nnx_core::shadows::ShadowInput { latitude_deg, days, hours, north_offset_deg, probes };
        Ok(json!({ "shadows": nnx_core::shadows::study(&scene, &input)? }))
    }

    /// The finish catalog, the project's choice and the quantities (with prices only where the user entered a rate).
    fn finishes_report(&self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let rep = nnx_core::finishes::report(&self.doc()?.model, &p.meta.finishes, p.boq.as_ref());
        Ok(json!({ "catalog": nnx_core::finishes::CATALOG, "chosen": p.meta.finishes, "report": rep }))
    }

    /// Chooses the facade and floor finishes (and optionally the rate of a finish code in the BOQ library): ONE undoable edit.
    fn finishes_set(&mut self, args: &Value) -> Result<Value, String> {
        let f: nnx_core::finishes::Finishes = serde_json::from_value(args.get("finishes").cloned().ok_or("finishes is required")?).map_err(|e| format!("finishes: {e}"))?;
        let rates: BTreeMap<String, f64> = match args.get("rates") {
            Some(r) if !r.is_null() => serde_json::from_value(r.clone()).map_err(|e| format!("rates: {e}"))?,
            _ => BTreeMap::new(),
        };
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        let out = nnx_core::project::update(doc, "Finishes", |p| {
            p.meta.finishes = f.clone();
            for (code, rate) in &rates {
                nnx_core::finishes::set_rate(&mut p.boq, code, *rate)?;
            }
            Ok(())
        })?;
        let mut v = self.finishes_report()?;
        v["changed"] = json!(out.changed);
        Ok(v)
    }

    /// The as-built inputs and their deviations from the design, in mm.
    fn asbuilt_report(&self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let rep = nnx_core::asbuilt::compare(&self.doc()?.model, &p.meta.as_built);
        Ok(json!({ "inputs": p.meta.as_built, "report": rep }))
    }

    /// Stores measured rows (CSV text in `csv`, or `rows`), the tolerance and the source: ONE undoable edit. A null tolerance clears it.
    fn asbuilt_set(&mut self, args: &Value) -> Result<Value, String> {
        let mut ab = self.project().ok_or("choose a layout first")?.meta.as_built;
        if let Some(text) = args.get("csv").and_then(Value::as_str) {
            ab.rows = nnx_core::asbuilt::parse_csv(text)?;
        } else if let Some(rows) = args.get("rows") {
            ab.rows = serde_json::from_value(rows.clone()).map_err(|e| format!("as-built rows: {e}"))?;
        }
        if let Some(t) = args.get("toleranceMm") {
            ab.tolerance_mm = t.as_f64();
        }
        if let Some(s) = args.get("source") {
            ab.source = s.as_str().filter(|s| !s.trim().is_empty()).map(str::to_string);
        }
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        let out = nnx_core::project::update(doc, "As-built measurements", |p| {
            p.meta.as_built = ab.clone();
            Ok(())
        })?;
        let mut v = self.asbuilt_report()?;
        v["changed"] = json!(out.changed);
        Ok(v)
    }

    /// The plan of the level on screen with the as-built point deviations drawn over it.
    fn asbuilt_svg(&self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let model = &self.doc()?.model;
        let rep = nnx_core::asbuilt::compare(model, &p.meta.as_built);
        let opts = self.draw_opts(self.show_structure);
        let svg = plan_svg(model, self.current_level(), &opts).map_err(|e| e.to_string())?;
        Ok(json!({ "svg": nnx_core::asbuilt::add_markers(&svg, model, self.current_level(), &opts, &rep), "report": rep }))
    }

    /// The standalone HTML tour: one file with the 3D scene and a viewer inside.
    fn tour_export(&self, args: &Value) -> Result<Value, String> {
        self.tour_file(args, false)
    }

    /// The client presentation page: the same single offline file without the structure, the services or any engineering control.
    fn client_view_export(&self, args: &Value) -> Result<Value, String> {
        self.tour_file(args, true)
    }

    fn tour_file(&self, args: &Value, client: bool) -> Result<Value, String> {
        let arabic = args.get("lang").and_then(Value::as_str) != Some("en");
        let scene = nnx_core::model3d::build_styled(&self.doc()?.model, self.style());
        let (facade, floor) = match self.project() {
            Some(p) => (p.meta.finishes.facade.as_deref().and_then(|i| nnx_core::finishes::find("facade", i)).map(|f| f.color), p.meta.finishes.floor.as_deref().and_then(|i| nnx_core::finishes::find("floor", i)).map(|f| f.color)),
            None => (None, None),
        };
        let (html, name) = if client {
            (nnx_core::tour::tour_client_html(&scene, &self.display_title(), arabic, facade, floor)?, "client-view.html")
        } else {
            (nnx_core::tour::tour_html(&scene, &self.display_title(), arabic, facade, floor)?, "tour.html")
        };
        Ok(json!({ "html": html, "name": name }))
    }

    /// A binary STL of the solids at the scale entered (`scale: 100` is 1:100), optionally one level.
    fn stl_export(&self, args: &Value) -> Result<Value, String> {
        let opts = nnx_core::stl::StlOpts { scale: args.get("scale").and_then(Value::as_f64), level: args.get("level").and_then(Value::as_i64), min_feature_mm: args.get("minFeatureMm").and_then(Value::as_f64) };
        let scene = nnx_core::model3d::build_styled(&self.doc()?.model, self.style());
        let (tris, rep) = nnx_core::stl::triangles(&scene, &opts)?;
        let bytes = nnx_core::stl::to_binary(&tris, &format!("NanoArch 1:{}", rep.scale));
        let level = opts.level.map(|l| format!("-L{}", l + 1)).unwrap_or_default();
        Ok(json!({ "stl_b64": nnx_core::xlsx::base64_encode(&bytes), "name": format!("model-1to{}{level}.stl", rep.scale), "report": rep, "closed": nnx_core::stl::each_solid_closed(&tris) }))
    }
    /// A starter template with the Ministry-rule setbacks of its plot (each carries its clause).
    fn template_apply(args: &Value) -> Result<Value, String> {
        let id = args.get("id").and_then(Value::as_str).ok_or("template id missing")?;
        let t = nnx_core::templates::find(id).ok_or_else(|| format!("unknown template '{id}'"))?;
        let (sb, sources) = nnx_core::templates::setbacks(&t).map_err(|e| e.join("; "))?;
        Ok(json!({ "template": t, "setbacks": { "front_m": sb[0], "side_m": sb[1], "back_m": sb[2] }, "sources": sources }))
    }

    /// The site plan of the open project: its plot outline, edges and rule, or for a rectangular plot the brief's plot size and
    /// the setbacks entered for it.
    fn site_plan(&self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let r = self.site_result()?;
        let layout = match &p.meta.site_layout {
            Some(l) => Some(nnx_core::site_layout::layout(&r, l)?),
            None => None,
        };
        Ok(json!({ "svg": nnx_core::site::site_svg_with(&r, &self.display_title(), layout.as_ref()), "name": "site-plan.svg", "result": r, "layout": layout }))
    }

    /// The site analysis of the open project (plot outline, edges, setbacks, building).
    fn site_result(&self) -> Result<nnx_core::site::SiteResult, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let input = match p.site_input() {
            Some(i) => i,
            None => {
                let b = p.brief.as_ref().ok_or("the project has no brief: enter the plot size and generate first")?;
                let mut i = nnx_core::site::rectangle_with_setbacks(b.width_m, b.length_m, b.setback_front_m, b.setback_back_m, b.setback_side_m);
                i.floors = p.structure.floors;
                i
            }
        };
        nnx_core::site::analyse(&input).map_err(|e| e.join("; "))
    }

    /// The site plan with the user's fences and gates (read-only): `{ title?, fences: [{ edge, kind?, height_m, thickness_m,
    /// source }], gates?: [{ key, edge, kind (vehicle | pedestrian), offset_m, width_m, operation? (swing_in | swing_out |
    /// sliding), source }] }`. Edge numbers follow `site_plan`'s `result.edges`. Returns the drawing, the fence and gate
    /// schedule and the fenced lengths.
    fn site_fence_plan(&self, args: &Value) -> Result<Value, String> {
        let input: nnx_core::siteplan::Input = serde_json::from_value(args.clone())
            .map_err(|e| format!("site fence plan: inputs are malformed (fences [{{ edge, height_m, thickness_m, source }}], gates? [{{ key, edge, kind, offset_m, width_m, source }}]): {e}"))?;
        let plan = nnx_core::siteplan::build(&self.site_result()?, &input)?;
        serde_json::to_value(&plan).map_err(|e| e.to_string())
    }

    /// The architectural style the project names (`meta.style`), if it names one of the four.
    fn style(&self) -> Option<nnx_core::style::Style> {
        self.project().and_then(|p| p.meta.style).and_then(|s| nnx_core::style::parse(&s))
    }

    fn view_opts(&self) -> nnx_core::elevation::ViewOpts {
        nnx_core::elevation::ViewOpts { title: self.title(), parapet_m: self.project().and_then(|p| p.meta.parapet_height_m), style: self.style(), ..Default::default() }
    }

    /// An elevation of one side: `{ "side": "north" | "south" | "east" | "west" }`.
    fn elevation(&self, args: &Value) -> Result<Value, String> {
        let name = args.get("side").and_then(Value::as_str).ok_or("side is required")?;
        let side = nnx_core::elevation::Side::parse(name).ok_or_else(|| format!("unknown side '{name}' (north, south, east or west)"))?;
        let svg = nnx_core::elevation::elevation_svg(&self.doc()?.model, side, &self.view_opts()).map_err(|e| e.to_string())?;
        Ok(json!({ "svg": svg, "name": format!("elevation-{name}.svg") }))
    }

    /// A section: `{ "name": "A-A" | "B-B", "at": metres }`. Without `at` the plane is cut through the middle of the building.
    fn section(&self, args: &Value) -> Result<Value, String> {
        let name = args.get("name").and_then(Value::as_str).ok_or("name is required (A-A or B-B)")?;
        let model = &self.doc()?.model;
        let mut cut = nnx_core::elevation::default_cut(model, name).ok_or_else(|| format!("unknown section '{name}' (A-A or B-B)"))?;
        if let Some(at) = args.get("at").and_then(Value::as_f64) {
            match &mut cut {
                nnx_core::elevation::Cut::AtY { at: a, .. } | nnx_core::elevation::Cut::AtX { at: a, .. } => *a = at,
            }
        }
        let svg = nnx_core::elevation::section_svg(model, &cut, &self.view_opts()).map_err(|e| e.to_string())?;
        Ok(json!({ "svg": svg, "name": format!("section-{name}.svg"), "cut": format!("{cut:?}") }))
    }

    /// Every elevation and section of the building in one call (four elevations, A-A, B-B and one section per stair shaft), each
    /// showing all levels. Read-only; the per-view `levels_missing` and the overall `complete` say whether a level was left out.
    fn auto_views(&self) -> Result<Value, String> {
        let set = nnx_core::autoviews::build(&self.doc()?.model, &self.view_opts())?;
        serde_json::to_value(set).map_err(|e| e.to_string())
    }

    /// Clashes between ducts, pipes and drains and the structure.
    fn clash_json(&self) -> Value {
        match &self.doc {
            Some(d) => serde_json::to_value(nnx_core::clash::detect(&nnx_core::model3d::build(&d.model), &nnx_core::clash::ClashOpts::default())).unwrap_or(Value::Null),
            None => Value::Null,
        }
    }

    /// Every conflict class (hard, clearance, routing, penetration) with what was not assessed. `clearance_mm` is the gap the user requires around structure;
    /// without it clearance is reported as not assessed.
    fn clash_classes(&self, args: &Value) -> Value {
        let gap_m = args.get("clearance_mm").and_then(Value::as_f64).map(|mm| mm / 1000.0);
        match &self.doc {
            Some(d) => serde_json::to_value(nnx_core::clash::classify(&nnx_core::model3d::build(&d.model), &nnx_core::clash::ClashOpts::default(), gap_m)).unwrap_or(Value::Null),
            None => Value::Null,
        }
    }

    fn runs_json(&self) -> Vec<Value> {
        self.doc
            .iter()
            .flat_map(|d| d.model.elements.values())
            .filter_map(|e| Some((e, e.typed::<nnx_core::typed::MepRun>().ok()?)))
            .map(|(e, r)| json!({ "id": e.id.to_string(), "system": r.system, "x1": r.x1, "y1": r.y1, "z1": r.z1, "x2": r.x2, "y2": r.y2, "z2": r.z2, "width": r.width, "height": r.height, "level": e.level_id.and_then(|l| building::levels(&self.doc.as_ref()?.model).iter().find(|(_, id)| *id == l).map(|(i, _)| *i)) }))
            .collect()
    }

    /// Adds a duct, pipe or drain the user entered (millimetres; heights above the level's floor top) as one undoable step.
    fn add_run(&mut self, args: &Value) -> Result<Value, String> {
        let mm = |k: &str| args.get(k).and_then(Value::as_i64).ok_or_else(|| format!("{k} is required (whole millimetres)"));
        let run = nnx_core::typed::MepRun {
            system: args.get("system").and_then(Value::as_str).ok_or("system is required (duct, pipe or drain)")?.to_string(),
            x1: mm("x1")?,
            y1: mm("y1")?,
            z1: mm("z1")?,
            x2: mm("x2")?,
            y2: mm("y2")?,
            z2: mm("z2")?,
            width: mm("width")?,
            height: mm("height")?,
            room: args.get("room").and_then(Value::as_str).map(str::to_string),
            role: None,
            label: args.get("label").and_then(Value::as_str).map(str::to_string),
            extra: Default::default(),
        };
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        let levels = building::levels(&doc.model);
        let level = match args.get("level").and_then(Value::as_i64) {
            Some(i) => Some(levels.iter().find(|(n, _)| *n == i).map(|(_, id)| *id).ok_or_else(|| format!("no level {i}"))?),
            None if levels.is_empty() => None,
            None => return Err("this building has several levels: say which level the run is on".into()),
        };
        let el = nnx_core::typed::run_element(&run, level, nnx_core::Source::User)?;
        let id = el.id;
        doc.commit("add run", nnx_core::Origin::User, vec![nnx_core::Command::Create(el)]).map_err(|e| e.to_string())?;
        let clashes = self.clash_json();
        let mine: Vec<Value> = clashes["clashes"].as_array().map(|a| a.iter().filter(|c| c["run_id"] == id.to_string()).cloned().collect()).unwrap_or_default();
        Ok(json!({ "id": id.to_string(), "runs": self.runs_json(), "clashes": clashes, "new_clashes": mine }))
    }

    /// True when the plot minus the setbacks no longer matches the building outline drawn on the ground level: the
    /// brief was changed after the plan was generated, so the plan must be generated again to follow it.
    fn plan_outdated(&self) -> bool {
        let (Some(d), Some(b)) = (&self.doc, self.project().and_then(|p| p.brief)) else { return false };
        let Ok((env, _)) = envelope_and_entry(&b) else { return true };
        let ground = building::levels(&d.model).first().map(|(_, id)| *id);
        let (mut x1, mut y1) = (0i64, 0i64);
        for s in d.model.elements.values().filter(|e| e.kind == "Space" && (ground.is_none() || e.level_id == ground)).filter_map(|e| e.typed::<nnx_core::typed::Space>().ok()) {
            x1 = x1.max(s.x + s.w);
            y1 = y1.max(s.y + s.h);
        }
        x1 != env.w || y1 != env.h
    }

    /// Changes project details, municipal limits or setbacks as ONE undoable step. Setbacks only change the stored
    /// brief: the drawn plan follows after it is generated again, and `plan_outdated` says so.
    fn project_set(&mut self, args: &Value) -> Result<Value, String> {
        let doc = self.doc.as_mut().ok_or("choose a layout first")?;
        let outcome = nnx_core::project::update(doc, "edit project", |p| {
            if let Some(Value::Object(patch)) = args.get("meta") {
                // Only the given keys change (the plot outline, for one, is not on the page's form).
                let Ok(Value::Object(mut merged)) = serde_json::to_value(&p.meta) else { return Err("project details cannot be read".into()) };
                merged.extend(patch.clone());
                p.meta = serde_json::from_value(Value::Object(merged)).map_err(|e| format!("project details: {e}"))?;
            }
            if let Some(l) = args.get("limits") {
                p.limits = serde_json::from_value(l.clone()).map_err(|e| format!("municipal limits: {e}"))?;
            }
            if let Some(s) = args.get("setbacks") {
                let b = p.brief.as_mut().ok_or("this project has no brief")?;
                for (key, slot) in [("front_m", &mut b.setback_front_m), ("back_m", &mut b.setback_back_m), ("side_m", &mut b.setback_side_m)] {
                    if let Some(v) = s.get(key) {
                        *slot = v.as_f64().ok_or_else(|| format!("setbacks.{key} must be a number of metres"))?;
                    }
                }
            }
            Ok(())
        })?;
        Ok(json!({ "changed": outcome.changed, "new_violations": outcome.new_violations, "settings": self.form_state(), "sbc": self.sbc_json(), "plan_outdated": self.plan_outdated() }))
    }

    /// The inputs the page shows in its forms, read from the project (and the view state). Computed on demand; the
    /// project element stays the only stored copy.
    fn form_state(&self) -> Value {
        let p = self.project().unwrap_or_default();
        let mut v = json!({
            "title": p.meta.title,
            "labels": p.labels,
            "structure": p.structure,
            "show_structure": self.show_structure,
            "show_mep": self.show_mep,
            "meta": p.meta,
            "limits": p.limits,
        });
        v["brief"] = serde_json::to_value(&p.brief).unwrap_or(Value::Null);
        // A project saved before the brief kept room phases has them only on the rooms of the model: hand them back to the room table
        // (a room the brief already gives a phase keeps it; none is invented for a room whose elements have none).
        if let (Some(doc), Some(rooms)) = (self.doc.as_ref(), v["brief"]["rooms"].as_array_mut()) {
            for r in rooms.iter_mut().filter(|r| r["phase"].is_null()) {
                let key = r["key"].as_str().unwrap_or_default().to_string();
                let found = doc.model.elements.values().filter(|e| e.kind == "Space").find_map(|e| {
                    let s = e.typed::<nnx_core::typed::Space>().ok()?;
                    (s.requirement_key.split('@').next() == Some(key.as_str())).then(|| e.props.get("phase").and_then(Value::as_str).map(String::from)).flatten()
                });
                if let Some(p) = found {
                    r["phase"] = json!(p);
                }
            }
        }
        v["mep"] = serde_json::to_value(&p.mep).unwrap_or(Value::Null);
        v["boq"] = serde_json::to_value(&p.boq).unwrap_or(Value::Null);
        v["sizing"] = serde_json::to_value(&p.sizing).unwrap_or(Value::Null);
        v
    }
    /// True when the open plan has changes that were never saved.
    pub fn is_dirty(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.is_dirty())
    }

    pub fn mark_saved(&mut self) {
        if let Some(d) = self.doc.as_mut() {
            d.mark_saved();
        }
    }
}

/// The project of a file saved before projects were stored in the model: rebuilt from its settings, with a note for
/// every part that could not be read (nothing is dropped silently).
fn legacy_project(settings: &Settings) -> (NanoArchProject, Vec<String>) {
    let mut notes = Vec::new();
    let mut p = NanoArchProject::default();
    p.meta.title = settings.get("title").and_then(Value::as_str).unwrap_or("").to_string();
    p.labels = settings.get("labels").and_then(Value::as_object).map(|o| o.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()).unwrap_or_default();
    match settings.get("brief").filter(|v| !v.is_null()) {
        Some(v) => match serde_json::from_value::<BriefIn>(v.clone()) {
            Ok(b) => p.brief = Some(b),
            Err(e) => notes.push(format!("the saved brief could not be read ({e}): partitions cannot be moved until you generate again")),
        },
        None => notes.push("this file has no saved brief: partitions cannot be moved".into()),
    }
    if let Some(v) = settings.get("structure").filter(|v| !v.is_null()) {
        match serde_json::from_value::<StructIn>(v.clone()) {
            Ok(s) => p.structure = s,
            Err(e) => notes.push(format!("the saved structural inputs could not be read ({e})")),
        }
    }
    if let Some(v) = settings.get("mep").filter(|v| !v.is_null()) {
        match serde_json::from_value(v.clone()) {
            Ok(m) => p.mep = Some(m),
            Err(e) => notes.push(format!("the saved MEP inputs could not be read ({e})")),
        }
    }
    if let Some(v) = settings.get("boq").filter(|v| !v.is_null()) {
        match serde_json::from_value(v.clone()) {
            Ok(b) => p.boq = Some(b),
            Err(e) => notes.push(format!("the saved BOQ inputs could not be read ({e})")),
        }
    }
    (p, notes)
}
/// The current UTC time as `YYYY-MM-DDTHH:MM:SS` (for file headers).
fn utc_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil date from days since 1970-01-01 (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// The design defaults the engines use when the user has not given a value. They are placeholders to be
/// replaced by project or SBC values; the interface lists them so none is applied silently.
fn assumptions() -> Value {
    let (w, win, s) = (WallOpts::default(), WindowOpts::default(), nnx_core::structural::StructOpts::default());
    let rows: Vec<(&str, &str, f64, &str)> = vec![
        ("Exterior wall thickness", "سماكة الجدار الخارجي", w.exterior_mm as f64, "mm"),
        ("Interior wall thickness", "سماكة الجدار الداخلي", w.interior_mm as f64, "mm"),
        ("Wall height (storey height)", "ارتفاع الجدار (ارتفاع الدور)", w.height_mm as f64, "mm"),
        ("Entry door width", "عرض باب المدخل", w.entry_door_mm as f64, "mm"),
        ("Interior door width", "عرض الباب الداخلي", w.interior_door_mm as f64, "mm"),
        ("Bathroom door width", "عرض باب الحمام", w.wet_door_mm as f64, "mm"),
        ("Door height", "ارتفاع الباب", w.door_height_mm as f64, "mm"),
        ("Solid wall beside a door", "جدار صلب بجانب الباب", w.door_margin_mm as f64, "mm"),
        ("Window width", "عرض النافذة", win.width_mm as f64, "mm"),
        ("Bathroom window width", "عرض نافذة الحمام", win.wet_width_mm as f64, "mm"),
        ("Window height", "ارتفاع النافذة", win.height_mm as f64, "mm"),
        ("Window sill height", "ارتفاع جلسة النافذة", win.sill_mm as f64, "mm"),
        ("Column section (square)", "مقطع العمود (مربع)", s.column_mm as f64, "mm"),
        ("Beam span / depth ratio", "نسبة بحر الكمرة إلى عمقها", s.beam_span_depth, ""),
        ("Slab span / thickness ratio", "نسبة بحر البلاطة إلى سماكتها", s.slab_span_depth, ""),
        ("Minimum room width (rooms)", "أقل عرض للغرفة", 2000.0, "mm"),
        ("Minimum room width (bathrooms)", "أقل عرض للحمام", 1500.0, "mm"),
    ];
    json!(rows.iter().map(|(en, ar, v, unit)| json!({ "en": en, "ar": ar, "value": v, "unit": unit })).collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic program, not a design requirement.
    fn brief() -> Value {
        let r = |key: &str, name: &str, func: &str, area: f64, zone: &str, light: bool| json!({ "key": key, "name": name, "func": func, "area": area, "zone": zone, "light": light });
        json!({
            "title": "فيلا تجريبية", "width_m": 18.0, "length_m": 22.0,
            "setback_front_m": 2.0, "setback_back_m": 2.0, "setback_side_m": 2.0, "entry": "south",
            "rooms": [
                r("majlis", "المجلس", "majlis", 36.0, "public", true), r("living", "الصالة", "living", 30.0, "public", true),
                r("dining", "الطعام", "dining", 24.0, "public", true), r("kitchen", "المطبخ", "kitchen", 20.0, "service", true),
                r("master", "الماستر", "bedroom", 28.0, "private", true), r("bed1", "غرفة 1", "bedroom", 20.0, "private", true),
                r("bed2", "غرفة 2", "bedroom", 20.0, "private", true), r("bed3", "غرفة 3", "bedroom", 20.0, "private", true),
                r("bath1", "حمام 1", "bathroom", 6.0, "service", false), r("bath2", "حمام 2", "bathroom", 6.0, "service", false),
                r("bath3", "حمام 3", "bathroom", 6.0, "service", false)
            ]
        })
    }

    fn started() -> App {
        let mut app = App::default();
        let g = app.handle("generate", &json!({ "brief": brief(), "struct": {} })).unwrap();
        assert!(g["alternatives"].as_array().unwrap().len() >= 2);
        app.handle("select", &json!({ "index": 0, "struct": { "show_structure": true } })).unwrap();
        app
    }

    #[test]
    fn generate_returns_distinct_alternatives_with_arabic_labels() {
        let mut app = App::default();
        let g = app.handle("generate", &json!({ "brief": brief() })).unwrap();
        let alts = g["alternatives"].as_array().unwrap();
        assert!(alts.len() >= 2);
        let svg = alts[0]["svg"].as_str().unwrap();
        assert!(svg.starts_with("<svg") && svg.contains("المجلس"));
    }

    #[test]
    fn alternatives_carry_declared_scores_and_are_ranked_best_first() {
        let mut app = App::default();
        let g = app.handle("generate", &json!({ "brief": brief() })).unwrap();
        let alts = g["alternatives"].as_array().unwrap();
        let overall: Vec<f64> = alts.iter().map(|a| a["scores"]["overall"].as_f64().expect("every alternative of this brief is scored")).collect();
        assert!(overall.windows(2).all(|w| w[0] >= w[1]), "not ranked best first: {overall:?}");
        for a in alts {
            for k in ["lighting", "connectivity", "privacy"] {
                let v = a["scores"][k].as_f64().unwrap_or_else(|| panic!("{k} missing"));
                assert!((0.0..=100.0).contains(&v), "{k} = {v}");
            }
            assert_eq!(a["scores"]["weights"]["lighting"], 1.0);
        }
        // Weighting only lighting ranks by lighting alone.
        let g = app.handle("generate", &json!({ "brief": brief(), "score_weights": { "lighting": 1.0, "connectivity": 0.0, "privacy": 0.0 } })).unwrap();
        let light: Vec<f64> = g["alternatives"].as_array().unwrap().iter().map(|a| a["scores"]["lighting"].as_f64().unwrap()).collect();
        assert!(light.windows(2).all(|w| w[0] >= w[1]), "not ranked by lighting: {light:?}");
        let err = app.handle("generate", &json!({ "brief": brief(), "score_weights": { "lighting": -1.0, "connectivity": 1.0, "privacy": 1.0 } })).unwrap_err();
        assert!(err.contains("lighting"), "{err}");
    }

    /// Two rooms on a 14 x 18 m buildable area (plot 18 x 22 m, 2 m setbacks). Synthetic.
    fn two_room_brief() -> Value {
        json!({ "title": "t", "width_m": 18.0, "length_m": 22.0, "setback_front_m": 2.0, "setback_back_m": 2.0, "setback_side_m": 2.0, "entry": "south",
                "rooms": [{ "key": "a", "name": "A", "func": "living", "area": 20.0, "zone": "public", "light": true },
                          { "key": "b", "name": "B", "func": "bedroom", "area": 16.0, "zone": "private", "light": true }] })
    }

    #[test]
    fn rooms_marked_on_a_sketch_are_built_exactly_as_marked() {
        let mut app = App::default();
        let place = json!({ "a": { "x": 0, "y": 0, "w": 5000, "h": 4000 }, "b": { "x": 5000, "y": 0, "w": 4000, "h": 4000 } });
        let g = app.handle("generate", &json!({ "brief": two_room_brief(), "placement": place })).unwrap();
        let alts = g["alternatives"].as_array().unwrap();
        assert!(alts.len() >= 2 && alts[0]["from_sketch"] == true && alts[1]["from_sketch"] == false, "the sketch comes first, generated ones follow");
        let s = app.handle("select", &json!({ "index": 0, "struct": {} })).unwrap();
        let rooms = s["rooms"].as_array().unwrap();
        assert_eq!(rooms.len(), 2, "nothing is added to fill the gaps");
        let area = |n: &str| rooms.iter().find(|r| r["name"] == n).unwrap()["area"].as_f64().unwrap();
        assert_eq!((area("A"), area("B")), (20.0, 16.0), "5 x 4 m and 4 x 4 m");
        // without a placement nothing is flagged
        let g2 = app.handle("generate", &json!({ "brief": two_room_brief() })).unwrap();
        assert!(g2["alternatives"].as_array().unwrap().iter().all(|a| a["from_sketch"] == false));
    }

    #[test]
    fn a_bad_sketch_placement_is_refused_with_the_reason() {
        let err = |place: Value| App::default().handle("generate", &json!({ "brief": two_room_brief(), "placement": place })).unwrap_err();
        let ok = |k: &str, x: i64, y: i64, w: i64, h: i64| (k.to_string(), json!({ "x": x, "y": y, "w": w, "h": h }));
        let obj = |v: Vec<(String, Value)>| Value::Object(v.into_iter().collect());
        assert!(err(obj(vec![ok("a", 0, 0, 5000, 4000), ok("b", 13000, 0, 4000, 4000)])).contains("outside"), "past the 14 m width");
        assert!(err(obj(vec![ok("a", 0, 0, 5000, 4000), ok("b", 4000, 1000, 4000, 4000)])).contains("overlap"));
        assert!(err(obj(vec![ok("a", 0, 0, 5000, 4000)])).contains("no level has all its rooms"), "b is not marked");
        assert!(err(obj(vec![ok("a", 0, 0, 5000, 300), ok("b", 5000, 0, 4000, 4000)])).contains("smaller than 0.5 m"));
    }

    #[test]
    fn select_draws_structure_on_request_and_warns_about_missing_inputs() {
        let mut app = App::default();
        app.handle("generate", &json!({ "brief": brief() })).unwrap();
        let plain = app.handle("select", &json!({ "index": 0, "struct": {} })).unwrap();
        assert!(!plain["svg"].as_str().unwrap().contains(r#"class="column""#));
        let warnings = plain["warnings"].to_string();
        assert!(warnings.contains("floor loads not set"), "{warnings}");
        let shown = app.handle("svg", &json!({ "show_structure": true })).unwrap();
        assert!(shown["svg"].as_str().unwrap().contains(r#"class="column""#));
    }

    #[test]
    fn user_inputs_flow_to_the_footings() {
        let mut app = App::default();
        app.handle("generate", &json!({ "brief": brief() })).unwrap();
        let s = json!({ "dead_kpa": 5.0, "live_kpa": 2.0, "bearing_kpa": 150.0, "floors": 2 });
        let r = app.handle("select", &json!({ "index": 0, "struct": s })).unwrap();
        assert!(!r["warnings"].to_string().contains("floor loads not set"));
        let q = app.handle("qto", &json!({})).unwrap();
        assert!(q["items"].as_array().unwrap().iter().any(|i| i["code"] == "FOOTING.COUNT"));
        assert!(q["csv"].as_str().unwrap().contains("preliminary"));
    }

    #[test]
    fn quantities_by_element_add_up_to_the_items_and_name_what_has_no_breakdown() {
        let mut app = App::default();
        app.handle("generate", &json!({ "brief": brief() })).unwrap();
        app.handle("select", &json!({ "index": 0 })).unwrap();
        let q = app.handle("qto", &json!({})).unwrap();
        let r = app.handle("qto_elements", &json!({})).unwrap();
        let elements = r["elements"].as_array().unwrap();
        assert!(!elements.is_empty());
        for item in q["items"].as_array().unwrap() {
            let code = item["code"].as_str().unwrap();
            if r["not_linked"].as_array().unwrap().iter().any(|c| c == code) {
                continue;
            }
            let sum: f64 = elements.iter().flat_map(|e| e["quantities"].as_array().unwrap()).filter(|x| x["code"] == code).map(|x| x["quantity"].as_f64().unwrap()).sum();
            assert!((sum - item["quantity"].as_f64().unwrap()).abs() < 0.01, "{code}: {sum} vs {}", item["quantity"]);
        }
        // One element on its own returns only its rows.
        let id = elements[0]["id"].as_str().unwrap();
        let one = app.handle("qto_elements", &json!({ "element": id })).unwrap();
        assert_eq!(one["elements"].as_array().unwrap().len(), 1);
        assert_eq!(one["elements"][0]["quantities"], elements[0]["quantities"]);
        // Bad input is refused by name.
        assert!(app.handle("qto_elements", &json!({ "element": "nonsense" })).unwrap_err().contains("element id"));
        assert!(app.handle("qto_elements", &json!({ "element": "00000000-0000-0000-0000-000000000000" })).unwrap_err().contains("not in the model"));
        assert!(app.handle("qto_elements", &json!({ "phase": "BOGUS" })).is_err());
        assert!(App::default().handle("qto_elements", &json!({})).is_err());
    }

    #[test]
    fn schedule_refuses_without_rates_and_runs_with_them() {
        let mut app = started();
        let none = app.handle("schedule", &json!({ "rates": {}, "start": "2026-11-15", "workdays": [true, true, true, true, false, false, true], "zones": 2 })).unwrap();
        assert_eq!(none["ok"], false);
        let keys = app.handle("template_keys", &json!({})).unwrap();
        let mut rates = serde_json::Map::new();
        for k in keys.as_array().unwrap() {
            let r = if k["fixed"] == true { json!({ "fixed_days": 3, "source": "synthetic" }) } else { json!({ "per_crew_day": 40, "source": "synthetic" }) };
            rates.insert(k["key"].as_str().unwrap().to_string(), r);
        }
        let ok = app.handle("schedule", &json!({ "rates": rates, "start": "2026-11-15", "workdays": [true, true, true, true, false, false, true], "zones": 2 })).unwrap();
        assert_eq!(ok["ok"], true);
        assert!(ok["svg"].as_str().unwrap().starts_with("<svg"));
        assert!(ok["project_days"].as_i64().unwrap() > 0);
        assert!(app.handle("schedule", &json!({ "rates": rates, "start": "nope", "workdays": [true, true, true, true, true, true, true], "zones": 1 })).is_err());
    }

    #[test]
    fn rules_run_on_the_current_model() {
        let mut app = started();
        let pack = r#"{"id":"T","edition":"synthetic","version":"1","rules":[{"id":"d","clause":"S","applies_to":{"kind":"Door","where":"isEgress == true"},"check":"width >= param.min","params":{"min":{"value":1200,"source":"synthetic"}},"severity":"error","message":"narrow"}]}"#;
        let r = app.handle("check_rules", &json!({ "pack": pack })).unwrap();
        assert_eq!(r["checked"], 1);
        assert_eq!(r["findings"].as_array().unwrap().len(), 1, "the entry door is narrower than the synthetic 1200 mm");
        assert!(app.handle("check_rules", &json!({ "pack": "{" })).is_err());
    }

    #[test]
    fn the_sourced_pack_runs_without_a_pasted_pack_and_every_rule_names_its_source() {
        let mut app = started();
        assert!(app.handle("check_rules", &json!({})).is_err(), "nothing to run without a pack, the sourced pack or an overlay");
        let r = app.handle("check_rules", &json!({ "sourced": true })).unwrap();
        assert!(r["checked"].as_u64().unwrap() > 0, "the sourced rules ran on the model's rooms");
        assert_eq!(r["layers"][0].as_str().unwrap().split('@').next(), Some("SBC-SOURCED"));
        let skipped: Vec<String> = r["skipped"].as_array().unwrap().iter().map(|s| s["rule"].as_str().unwrap().to_string()).collect();
        assert_eq!(skipped, ["sbc.daylight.other", "sbc.daylight.stair"], "only the functions without a read ratio are reported as not run");
        assert!(!r["findings"].to_string().contains("could not be evaluated"), "the generated windows give every room a glazing ratio: {}", r["findings"]);
        let pack: Value = serde_json::from_str(&nnx_core::sbc::sourced_pack_json()).unwrap();
        for rule in pack["rules"].as_array().unwrap().iter().filter(|r| !r["params"]["min"]["value"].is_null()) {
            let src = rule["params"]["min"]["source"].as_str().unwrap();
            assert!(src.contains("clause") && src.contains("p."), "{} has no clause/page source: {src}", rule["id"]);
        }
        // A pasted pack goes on top and may replace a sourced rule.
        let over = r#"{"id":"O","edition":"e","version":"1","rules":[{"id":"sbc.room_area.bedroom","clause":"X","applies_to":{"kind":"Space"},"check":"area >= param.min","params":{"min":{"value":1,"source":"test"}},"severity":"error","message":"m"}]}"#;
        let r2 = app.handle("check_rules", &json!({ "sourced": true, "pack": over })).unwrap();
        assert_eq!(r2["overlays"][1]["replaced"][0][0], "sbc.room_area.bedroom");
    }

    #[test]
    fn a_limit_entered_with_its_source_makes_a_not_run_rule_run_and_one_without_a_source_is_refused() {
        let mut app = started();
        let base = app.handle("check_rules", &json!({ "sourced": true })).unwrap();
        let other = base["skipped"].as_array().unwrap().iter().find(|s| s["rule"] == "sbc.daylight.other").expect("the rule is not run without a limit");
        assert_eq!(other["param"], "min", "the response names the parameter to enter");
        let unchecked = other["elements"].as_array().unwrap().len();
        let lim = |source: &str| json!({ "sourced": true, "limits": [{ "rule": "sbc.daylight.other", "param": "min", "value": 0.0, "source": source }] });
        let r = app.handle("check_rules", &lim("test limit, entered by the user")).unwrap();
        assert!(!r["skipped"].as_array().unwrap().iter().any(|s| s["rule"] == "sbc.daylight.other"), "the rule runs once its limit is entered");
        assert!(r["layers"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().starts_with("USER-LIMITS")), "the entered limits are a recorded layer");
        assert!(r["checked"].as_u64().unwrap() >= base["checked"].as_u64().unwrap() + unchecked as u64, "the spaces the rule left unchecked are now checked");
        for bad in ["", "   "] {
            assert!(app.handle("check_rules", &lim(bad)).unwrap_err().contains("no source"), "a limit with no source is refused");
        }
        let no_value = json!({ "sourced": true, "limits": [{ "rule": "sbc.daylight.other", "param": "min", "source": "x" }] });
        assert!(app.handle("check_rules", &no_value).unwrap_err().contains("no numeric value"));
        let unknown = json!({ "sourced": true, "limits": [{ "rule": "no.such.rule", "param": "min", "value": 1.0, "source": "x" }] });
        assert!(app.handle("check_rules", &unknown).is_err(), "a limit for a rule that does not exist is refused");
    }

    #[test]
    fn undo_redo_and_sync_work_on_the_current_document() {
        let mut app = started();
        let s = app.handle("sync", &json!({})).unwrap();
        assert_eq!((s["created"].as_u64(), s["removed"].as_u64()), (Some(0), Some(0)), "freshly generated structure is already in step");
        let undone = app.handle("undo", &json!({})).unwrap();
        assert_eq!(undone["changed"], true);
        let blank = undone["svg"].as_str().expect("an empty model draws the blank drawing area");
        assert!(!blank.contains("class=\"wall"), "after undoing the only step nothing is drawn but the empty area");
        assert!(app.handle("svg", &json!({ "show_structure": false })).unwrap()["svg"].is_string());
        let redone = app.handle("redo", &json!({})).unwrap();
        assert_eq!(redone["changed"], true);
        assert!(redone["svg"].as_str().unwrap().starts_with("<svg"));
        assert_eq!(app.handle("redo", &json!({})).unwrap()["changed"], false, "nothing left to redo");
    }

    #[test]
    fn saved_state_follows_the_model_not_a_counter() {
        let mut app = App::default();
        assert_eq!(app.handle("status", &json!({})).unwrap()["dirty"], false, "nothing open, nothing to save");
        app.handle("generate", &json!({ "brief": brief() })).unwrap();
        assert!(!app.is_dirty(), "generated alternatives are not a document yet");
        app.handle("select", &json!({ "index": 0, "struct": {} })).unwrap();
        assert!(app.is_dirty(), "a freshly generated plan is unsaved");
        app.mark_saved();
        assert_eq!(app.handle("status", &json!({})).unwrap()["dirty"], false);
        app.handle("undo", &json!({})).unwrap();
        assert!(app.is_dirty(), "undoing past the saved state is a change");
        app.handle("redo", &json!({})).unwrap();
        assert!(!app.is_dirty(), "back at the saved state it is clean again");
    }

    #[test]
    fn default_assumptions_are_listed_with_their_real_values() {
        let mut app = App::default();
        let rows = app.handle("assumptions", &json!({})).unwrap();
        let rows = rows.as_array().unwrap();
        assert!(rows.len() >= 15);
        let wall = rows.iter().find(|r| r["en"] == "Exterior wall thickness").unwrap();
        assert_eq!(wall["value"].as_f64(), Some(WallOpts::default().exterior_mm as f64), "the list reads the engine's own default");
        assert!(rows.iter().all(|r| r["ar"].as_str().is_some_and(|s| !s.is_empty())));
    }

    #[test]
    fn a_saved_project_reopens_exactly_with_its_names_inputs_and_structure() {
        let s = json!({ "dead_kpa": 5.0, "live_kpa": 2.0, "bearing_kpa": 150.0, "floors": 1, "max_span_mm": 5000, "show_structure": true });
        let mut first = App::default();
        first.handle("generate", &json!({ "brief": brief(), "struct": s })).unwrap();
        let shown = first.handle("select", &json!({ "index": 0, "struct": s })).unwrap();
        let (model, settings) = (first.model().unwrap().clone(), first.settings());

        // Through a real file, as the Save and Open buttons do.
        let dir = std::env::temp_dir().join(format!("nanoarch-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("p.nnx");
        nnx_core::persist::save_with_settings(&model, &settings, &file).unwrap();
        let (loaded, back) = nnx_core::persist::load_with_settings(&file).unwrap();
        std::fs::remove_dir_all(&dir).ok();

        let mut second = App::default();
        let opened = second.open(loaded, &back).unwrap();
        assert_eq!(opened["svg"], shown["svg"], "the reopened plan draws identically, Arabic names included");
        assert_eq!(opened["settings"]["brief"]["title"], "فيلا تجريبية");
        assert_eq!(opened["settings"]["structure"]["bearing_kpa"], 150.0);
        assert!(!second.is_dirty(), "a just-opened project has nothing unsaved");
        assert!(opened["warnings"].as_array().unwrap().is_empty(), "the structure is in step: {}", opened["warnings"]);
        assert_eq!(second.handle("sync", &json!({})).unwrap()["created"], 0, "and sync finds nothing to change");
        assert_eq!(second.handle("qto", &json!({})).unwrap()["csv"], first.handle("qto", &json!({})).unwrap()["csv"]);
        // The settings survive a second round trip unchanged.
        assert_eq!(second.settings(), settings);
    }

    #[test]
    fn opening_a_file_without_settings_or_without_a_plan_is_handled() {
        let mut app = App::default();
        assert!(app.open(nnx_core::Model::default(), &Settings::new()).unwrap_err().contains("no plan"));

        // A plan saved by something that did not store settings still opens; the structure is checked.
        let mut first = App::default();
        first.handle("generate", &json!({ "brief": brief() })).unwrap();
        first.handle("select", &json!({ "index": 0, "struct": {} })).unwrap();
        let mut plain = App::default();
        let opened = plain.open(first.model().unwrap().clone(), &Settings::new()).unwrap();
        assert!(opened["svg"].as_str().unwrap().starts_with("<svg"));
        assert!(plain.handle("generate", &json!({})).is_err(), "regenerating still needs a brief");
    }

    fn walls_of(app: &App, exterior: bool) -> Vec<String> {
        app.doc.as_ref().unwrap().model.elements.values().filter(|e| e.kind == "Wall" && e.props["isExterior"] == json!(exterior)).map(|e| e.id.to_string()).collect()
    }

    fn hash(app: &App) -> u64 {
        app.doc.as_ref().unwrap().model.state_hash()
    }

    /// Hash of the drawing alone: every element except the project element.
    fn drawing_hash(app: &App) -> u64 {
        let mut m = app.doc.as_ref().unwrap().model.clone();
        m.elements.retain(|_, e| e.kind != "Project");
        m.state_hash()
    }

    /// The plan still tiles the buildable rectangle (14 m x 18 m here) with no gaps or overlaps.
    fn assert_tiles(app: &App) {
        let rects: Vec<(i64, i64, i64, i64)> = app
            .doc
            .as_ref()
            .unwrap()
            .model
            .elements
            .values()
            .filter_map(|e| e.typed::<nnx_core::typed::Space>().ok())
            .map(|s| (s.x, s.y, s.w, s.h))
            .collect();
        assert_eq!(rects.iter().map(|r| r.2 * r.3).sum::<i64>(), 14_000 * 18_000, "total room area equals the buildable area");
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                let (w, h) = (((a.0 + a.2).min(b.0 + b.2) - a.0.max(b.0)).max(0), ((a.1 + a.3).min(b.1 + b.3) - a.1.max(b.1)).max(0));
                assert_eq!(w * h, 0, "two rooms overlap");
            }
        }
    }

    #[test]
    fn moving_a_partition_keeps_everything_valid_and_undoes_in_one_step() {
        let mut app = App::default();
        app.handle("generate", &json!({ "brief": brief(), "struct": { "dead_kpa": 5.0, "live_kpa": 2.0, "bearing_kpa": 150.0 } })).unwrap();
        app.handle("select", &json!({ "index": 0, "struct": { "dead_kpa": 5.0, "live_kpa": 2.0, "bearing_kpa": 150.0 } })).unwrap();
        let before = hash(&app);
        let before_areas = app.rooms_json();

        let mut done = None;
        'search: for wall in walls_of(&app, false) {
            for d in [300, -300, 600, -600] {
                if let Ok(r) = app.handle("move_partition", &json!({ "wall": wall, "delta_mm": d })) {
                    done = Some(r);
                    break 'search;
                }
                assert_eq!(hash(&app), before, "a refused move changes nothing");
            }
        }
        let r = done.expect("at least one partition of the example can move");
        assert!(r["moved"].as_u64().unwrap() >= 2, "a partition resizes the rooms on both sides");
        assert_ne!(r["rooms"], json!(before_areas), "the room table follows the edit");

        assert_tiles(&app);
        assert!(nnx_core::typed::validate_model(&app.doc.as_ref().unwrap().model).is_empty(), "every element is still valid");
        let s = app.handle("sync", &json!({})).unwrap();
        assert_eq!((s["created"].as_u64(), s["removed"].as_u64()), (Some(0), Some(0)), "the structure was synced inside the same edit");
        assert!(app.is_dirty());

        assert_eq!(app.handle("undo", &json!({})).unwrap()["changed"], true);
        assert_eq!(hash(&app), before, "one undo restores the plan exactly, walls and structure included");
    }

    #[test]
    fn edits_that_would_break_the_plan_are_refused_and_change_nothing() {
        let mut app = started();
        let before = hash(&app);
        let outer = walls_of(&app, true)[0].clone();
        let e = app.handle("move_partition", &json!({ "wall": outer, "delta_mm": 300 })).unwrap_err();
        assert!(e.contains("exterior"), "{e}");
        let inner = walls_of(&app, false)[0].clone();
        assert!(app.handle("move_partition", &json!({ "wall": inner, "delta_mm": 100_000 })).is_err(), "a room would vanish");
        assert!(app.handle("move_partition", &json!({ "wall": inner, "delta_mm": 5 })).is_err(), "too small to matter");
        assert!(app.handle("move_partition", &json!({ "wall": "not-an-id", "delta_mm": 300 })).is_err());
        assert_eq!(hash(&app), before);
    }

    #[test]
    fn doors_and_windows_can_be_resized_moved_and_deleted() {
        let mut app = started();
        let find = |app: &App, kind: &str| app.doc.as_ref().unwrap().model.elements.values().find(|e| e.kind == kind && e.props.get("isEgress") != Some(&json!(true))).or_else(|| app.doc.as_ref().unwrap().model.elements.values().find(|e| e.kind == kind)).unwrap().id.to_string();
        let count = |app: &App, kind: &str| app.doc.as_ref().unwrap().model.elements.values().filter(|e| e.kind == kind).count();
        let before = hash(&app);

        let window = find(&app, "Window");
        let info = app.handle("describe", &json!({ "id": window })).unwrap();
        assert_eq!(info["kind"], "Window");
        let wall_len = info["wall_length"].as_i64().unwrap();
        assert!(app.handle("set_opening", &json!({ "id": window, "width": wall_len + 500 })).is_err(), "wider than its wall");
        assert_eq!(hash(&app), before, "the refused edit changed nothing");
        app.handle("set_opening", &json!({ "id": window, "width": 700 })).unwrap();
        assert_eq!(app.handle("describe", &json!({ "id": window })).unwrap()["width"], 700);
        assert!(nnx_core::typed::validate_model(&app.doc.as_ref().unwrap().model).is_empty());

        let door = find(&app, "Door");
        let doors = count(&app, "Door");
        app.handle("delete_opening", &json!({ "id": door })).unwrap();
        assert_eq!(count(&app, "Door"), doors - 1);
        assert!(app.handle("delete_opening", &json!({ "id": walls_of(&app, true)[0] })).is_err(), "walls are not deleted this way");
        // Both edits undo, one at a time.
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(count(&app, "Door"), doors);
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(hash(&app), before);
    }

    /// SYNTHETIC values for the example plan; not code values.
    fn mep_inputs() -> Value {
        json!({
            "insulation": { "assemblies": {
                "wall": { "rsi": 0.13, "rse": 0.04, "layers": [{ "material": "block", "thickness_mm": 200, "lambda": 1.0 }, { "material": "eps", "thickness_mm": 50, "lambda": 0.04, "kind": "insulation" }] },
                "roof": { "rsi": 0.1, "rse": 0.04, "layers": [{ "material": "concrete", "thickness_mm": 150, "lambda": 1.5 }] } }, "u_limits": { "wall": 0.5 } },
            "electrical": { "voltage_v": 230, "power_factor": 0.95, "three_phase": false, "lights_per_m2": 0.5, "light_w": 10, "sockets_per_m": 0.4, "socket_w": 200,
                "demand_factor": 0.6, "max_light_points": 12, "max_socket_points": 8, "breaker_margin": 1.25, "breaker_sizes_a": [6, 10, 16, 20, 32, 40, 63],
                "cables": [{ "amps": 10, "mm2": 1.5 }, { "amps": 16, "mm2": 2.5 }, { "amps": 25, "mm2": 4 }, { "amps": 32, "mm2": 6 }, { "amps": 50, "mm2": 10 }],
                "resistivity": 0.0175, "max_vdrop_pct": 3, "panel_room": "living", "route_factor": 1.3 },
            "plumbing": { "catalog": { "wc": { "flow_lps": 0.1, "dfu": 4 }, "basin": { "flow_lps": 0.1, "dfu": 1 }, "sink": { "flow_lps": 0.15, "dfu": 2 } },
                "fixtures_by_func": { "bathroom": { "wc": 1, "basin": 1 }, "kitchen": { "sink": 1 } }, "simultaneity": 0.4, "max_velocity_mps": 2,
                "pipes": [{ "name": "DN15", "inner_mm": 13 }, { "name": "DN20", "inner_mm": 18 }, { "name": "DN25", "inner_mm": 23 }, { "name": "DN32", "inner_mm": 30 }],
                "drain_table": [{ "max_dfu": 6, "dia_mm": 50 }, { "max_dfu": 20, "dia_mm": 75 }, { "max_dfu": 160, "dia_mm": 100 }] },
            "hvac": { "indoor_c": 24, "outdoor_c": 45, "window_u": 3.0, "window_shgc": 0.5, "solar_w_m2": 400, "person_w": 100, "area_per_person_m2": 10, "lights_w_m2": 10,
                "equipment_w_m2": 5, "vent_l_s_per_m2": 1, "safety_factor": 1.1, "unit_sizes_w": [2500, 3500, 5000, 7000, 10500], "roof_exposed": true }
        })
    }

    fn mep_points(app: &App) -> usize {
        app.doc.as_ref().unwrap().model.elements.values().filter(|e| e.kind == "MepPoint").count()
    }

    #[test]
    fn mep_computes_applies_draws_counts_saves_and_undoes() {
        let mut app = started();
        let before = drawing_hash(&app);
        // Compute only: no drawing element is added. The inputs themselves are part of the project, so they are stored
        // (one undoable step), and entering the same inputs again changes nothing.
        let r = app.handle("mep", &json!({ "inputs": mep_inputs(), "apply": false, "show": true })).unwrap();
        assert!(r["report"]["electrical"].is_object() && r["report"]["plumbing"].is_object() && r["report"]["hvac"].is_object());
        assert_eq!(r["report"]["missing"], json!({}));
        assert_eq!(mep_points(&app), 0);
        assert_eq!(drawing_hash(&app), before);
        assert!(app.project().unwrap().mep.is_some(), "the inputs are stored in the project");
        let stored = hash(&app);
        app.handle("mep", &json!({ "inputs": mep_inputs(), "apply": false, "show": true })).unwrap();
        assert_eq!(hash(&app), stored, "the same inputs again change nothing");
        // Apply: points enter the model, the overlay draws them, quantities count them and the insulation.
        let r = app.handle("mep", &json!({ "inputs": mep_inputs(), "apply": true, "show": true })).unwrap();
        let n = mep_points(&app);
        assert!(n > 20 && r["applied"]["created"] == json!(n));
        assert!(r["svg"].as_str().unwrap().contains("mep-light"));
        let q = app.handle("qto", &json!({})).unwrap();
        let codes: Vec<&str> = q["items"].as_array().unwrap().iter().map(|i| i["code"].as_str().unwrap()).collect();
        for c in ["MEP.light", "MEP.socket", "MEP.panel", "MEP.ac_indoor", "INS.wall.eps"] {
            assert!(codes.contains(&c), "{c} in {codes:?}");
        }
        assert!(q["csv"].as_str().unwrap().contains("MEP.socket"));
        // Applying the same thing again changes nothing.
        let again = app.handle("mep", &json!({ "inputs": mep_inputs(), "apply": true, "show": true })).unwrap();
        assert_eq!(again["applied"], json!({ "created": 0, "removed": 0 }));
        // The inputs and the overlay switch are saved and come back with the file.
        let (model, settings) = (app.model().unwrap().clone(), app.settings());
        let mut second = App::default();
        let opened = second.open(model, &settings).unwrap();
        assert_eq!(opened["settings"]["mep"]["electrical"]["voltage_v"].as_f64(), Some(230.0));
        assert_eq!(mep_points(&second), n);
        assert_eq!(second.handle("qto", &json!({})).unwrap()["csv"], q["csv"]);
        // One undo removes every point.
        assert_eq!(app.handle("undo", &json!({})).unwrap()["changed"], true);
        assert_eq!(hash(&app), stored, "the points are gone and the stored inputs remain");
        assert_eq!(drawing_hash(&app), before);
        // A second undo takes the inputs back out of the project.
        assert_eq!(app.handle("undo", &json!({})).unwrap()["changed"], true);
        assert!(app.project().unwrap().mep.is_none());
    }

    #[test]
    fn incomplete_mep_inputs_are_reported_by_name_and_nothing_is_added() {
        let mut app = started();
        let mut i = mep_inputs();
        i["electrical"]["voltage_v"] = Value::Null;
        let r = app.handle("mep", &json!({ "inputs": i, "apply": true })).unwrap();
        assert!(r["report"]["electrical"].is_null());
        assert!(r["report"]["missing"]["electrical"].to_string().contains("electrical.voltage_v"));
        assert!(r["report"]["plumbing"].is_object(), "the other disciplines still compute");
        assert!(app.handle("mep", &json!({ "inputs": "nonsense" })).is_err());
    }

    #[test]
    fn plan_edits_after_mep_are_either_applied_whole_or_refused_with_a_reason() {
        let mut app = started();
        app.handle("mep", &json!({ "inputs": mep_inputs(), "apply": true })).unwrap();
        let before = hash(&app);
        let mut moved = false;
        for wall in walls_of(&app, false) {
            for d in [300, -300] {
                match app.handle("move_partition", &json!({ "wall": wall, "delta_mm": d })) {
                    Ok(r) => {
                        moved = true;
                        assert!(nnx_core::typed::validate_model(&app.doc.as_ref().unwrap().model).is_empty(), "every element is still valid");
                        assert!(r["mep_cleared"].as_u64().unwrap() > 0, "generated points are cleared with the edit");
                        assert_eq!(mep_points(&app), 0);
                        // One undo restores the plan and the points together, exactly.
                        app.handle("undo", &json!({})).unwrap();
                        assert_eq!(hash(&app), before);
                        assert!(mep_points(&app) > 20);
                    }
                    Err(e) => {
                        assert!(!e.is_empty());
                        assert_eq!(hash(&app), before, "a refused edit changes nothing");
                    }
                }
            }
        }
        assert!(moved, "at least one partition can move");
    }

    #[test]
    fn bid_comparison_is_read_only_and_ranks_only_complete_bids() {
        let mut app = started();
        let inputs = json!({ "project": { "currency": "XXX" }, "items": { "SLAB.VOL": { "rate": 100, "rate_source": "test" } } });
        let r = app.handle("boq", &json!({ "inputs": inputs })).unwrap();
        let codes: Vec<String> = r["report"]["rows"].as_array().unwrap().iter().filter(|x| x["reference_only"] == false).map(|x| x["code"].as_str().unwrap().to_string()).collect();
        assert!(codes.len() >= 2);
        let rates = |k: f64| codes.iter().map(|c| (c.clone(), json!(k))).collect::<serde_json::Map<_, _>>();
        let bid = |n: &str, k: f64| json!({ "contractor": n, "source": "synthetic", "date": "2026-11-01", "currency": "XXX", "rates": rates(k) });
        let before = hash(&app);
        let r = app.handle("bid_compare", &json!({ "bids": [bid("A", 2.0), bid("B", 1.0)] })).unwrap();
        assert_eq!(hash(&app), before, "the comparison changes nothing");
        assert_eq!(r["comparison"]["lowest_complete"], "B");
        assert!(r["csv"].as_str().unwrap().contains("contractor,source,date"));
        let mut partial = bid("C", 0.5);
        partial["rates"] = json!({ codes[0].clone(): 0.5 });
        let r = app.handle("bid_compare", &json!({ "bids": [bid("A", 2.0), partial] })).unwrap();
        assert_eq!(r["comparison"]["totals"][1]["rank"], Value::Null);
        assert!(app.handle("bid_compare", &json!({})).unwrap_err().contains("bids is required"));
        assert!(app.handle("bid_compare", &json!({ "bids": [bid("A", 2.0)] })).unwrap_err().contains("at least two"));
        assert!(app.handle("bid_compare", &json!({ "bids": [bid("A", 2.0), { "contractor": "Z", "extra": 1 }] })).is_err());
        assert!(App::default().handle("bid_compare", &json!({ "bids": [] })).is_err());
    }

    #[test]
    fn boq_prices_what_the_user_priced_lists_materials_exports_and_saves() {
        let mut app = started();
        app.handle("mep", &json!({ "inputs": mep_inputs(), "apply": true })).unwrap();
        // SYNTHETIC values; not real rates or specifications.
        let inputs = json!({
            "project": { "name": "مشروع تجريبي", "currency": "XXX" },
            "items": { "SLAB.VOL": { "spec": "synthetic spec", "spec_source": "test", "rate": 100, "rate_source": "test" }, "COLUMN.VOL": { "rate": 100, "rate_source": "test" }, "MEP.light": { "rate": 5, "rate_source": "test" } },
            "materials": { "SLAB.*": [{ "material": "cement", "unit": "bag", "per_unit": 7, "source": "test" }] },
            "markups": [{ "name": "VAT", "pct": 15, "source": "test" }]
        });
        let r = app.handle("boq", &json!({ "inputs": inputs, "lang": "ar" })).unwrap();
        let rows = r["report"]["rows"].as_array().unwrap();
        let find = |c: &str| rows.iter().find(|x| x["code"] == c).unwrap_or_else(|| panic!("{c}")).clone();
        let slab = find("SLAB.VOL");
        let q = slab["quantity"].as_f64().unwrap();
        assert!((slab["amount"].as_f64().unwrap() - (q * 100.0 * 100.0).round() / 100.0).abs() < 0.01);
        assert!(find("MEP.light")["amount"].as_f64().unwrap() > 0.0, "MEP points are BOQ items too");
        assert!(rows.iter().any(|x| x["code"] == "INS.wall.eps" || x["code"] == "MEP.socket"), "every take-off item appears");
        assert_eq!(r["report"]["complete"], false, "unpriced items make the total partial");
        let cement = r["report"]["materials"].as_array().unwrap().iter().find(|m| m["material"] == "cement").unwrap().clone();
        assert!(cement["quantity"].as_f64().unwrap() > 7.0 * q);
        assert!(r["html"].as_str().unwrap().contains("مشروع تجريبي") && r["boq_csv"].as_str().unwrap().contains("PARTIAL"));
        let zip = nnx_core::xlsx::base64_decode(r["xlsx_b64"].as_str().unwrap()).unwrap();
        assert_eq!(&zip[..2], b"PK", "the workbook is a zip package");
        assert!(app.handle("boq", &json!({ "inputs": "nonsense" })).is_err());
        // The inputs come back with the saved project.
        let (model, settings) = (app.model().unwrap().clone(), app.settings());
        let mut second = App::default();
        let opened = second.open(model, &settings).unwrap();
        assert_eq!(opened["settings"]["boq"]["markups"][0]["pct"].as_f64(), Some(15.0));
    }

    #[test]
    fn the_3d_scene_follows_the_model_including_edits_and_points() {
        let mut app = started();
        let scene = app.handle("model3d", &json!({})).unwrap();
        let boxes = scene["boxes"].as_array().unwrap();
        let count = |k: &str| boxes.iter().filter(|b| b["kind"] == k).count();
        assert!(count("Wall") > 20 && count("Slab") >= 1 && count("Column") >= 4 && count("Space") == 12 && count("MepPoint") == 0);
        assert!(boxes.iter().all(|b| app.doc.as_ref().unwrap().model.elements.keys().any(|k| k.to_string() == b["id"].as_str().unwrap())));
        app.handle("mep", &json!({ "inputs": mep_inputs(), "apply": true })).unwrap();
        let with_points = app.handle("model3d", &json!({})).unwrap();
        assert_eq!(with_points["boxes"].as_array().unwrap().iter().filter(|b| b["kind"] == "MepPoint").count(), mep_points(&app));
        assert!(App::default().handle("model3d", &json!({})).is_err(), "no plan, no model");
    }

    #[test]
    fn sun_shadows_run_over_the_year_and_name_every_missing_input() {
        let mut app = started();
        let base = json!({ "latitude_deg": 24.0, "north_offset_deg": 0.0, "hours": [9.0, 12.0, 15.0], "monthly": true });
        let r = app.handle("sun_shadows", &base).unwrap();
        let days = r["shadows"]["days"].as_array().unwrap();
        assert_eq!(days.len(), 12, "the 21st of every month");
        let longest = |d: usize| days[d]["hours"][1]["longest_shadow_m"].as_f64().unwrap();
        assert!(longest(11) > longest(5), "the December noon shadow is longer than the June one");
        assert!(days[5]["hours"][1]["polygons"].as_array().unwrap().len() > 10, "one polygon per wall, slab, column or beam");
        for key in ["latitude_deg", "north_offset_deg", "hours"] {
            let mut b = base.clone();
            b.as_object_mut().unwrap().remove(key);
            assert!(app.handle("sun_shadows", &b).unwrap_err().contains(key), "{key}");
        }
        let mut no_days = base.clone();
        no_days.as_object_mut().unwrap().remove("monthly");
        assert!(app.handle("sun_shadows", &no_days).unwrap_err().contains("days"));
        let mut both = base.clone();
        both["days"] = json!([80]);
        assert!(app.handle("sun_shadows", &both).unwrap_err().contains("not both"));
        assert!(App::default().handle("sun_shadows", &base).is_err(), "no plan, no model");
    }

    // ------------------------------------------------------------------ several floors

    /// Ground: public rooms and a kitchen. First floor: bedrooms. SYNTHETIC sizes.
    fn brief2() -> Value {
        let r = |key: &str, name: &str, func: &str, area: f64, zone: &str, light: bool, floor: u32| json!({ "key": key, "name": name, "func": func, "area": area, "zone": zone, "light": light, "floor": floor });
        json!({
            "title": "فيلا بدورين", "width_m": 18.0, "length_m": 22.0,
            "setback_front_m": 2.0, "setback_back_m": 2.0, "setback_side_m": 2.0, "entry": "south",
            "stair": { "width_m": 2.5, "length_m": 5.0, "side": "west" },
            "rooms": [
                r("majlis", "المجلس", "majlis", 36.0, "public", true, 1), r("living", "الصالة", "living", 30.0, "public", true, 1),
                r("dining", "الطعام", "dining", 24.0, "public", true, 1), r("kitchen", "المطبخ", "kitchen", 20.0, "service", true, 1),
                r("bath1", "حمام 1", "bathroom", 6.0, "service", false, 1),
                r("master", "الماستر", "bedroom", 28.0, "private", true, 2), r("bed1", "غرفة 1", "bedroom", 20.0, "private", true, 2),
                r("bed2", "غرفة 2", "bedroom", 20.0, "private", true, 2), r("bath2", "حمام 2", "bathroom", 6.0, "service", false, 2),
                r("bath3", "حمام 3", "bathroom", 6.0, "service", false, 2)
            ]
        })
    }

    fn loads2() -> Value {
        json!({ "dead_kpa": 5.0, "live_kpa": 2.0, "bearing_kpa": 150.0, "floors": 2, "max_span_mm": 5000 })
    }

    fn started2() -> App {
        let mut app = App::default();
        app.handle("generate", &json!({ "brief": brief2(), "struct": loads2() })).unwrap();
        app.handle("select", &json!({ "index": 0, "struct": loads2() })).unwrap();
        app
    }

    fn of_level<'a>(app: &'a App, level: usize, kind: &'a str) -> Vec<&'a nnx_core::Element> {
        let lid = building::levels(&app.doc.as_ref().unwrap().model)[level].1;
        app.doc.as_ref().unwrap().model.elements.values().filter(|e| e.kind == kind && e.level_id == Some(lid)).collect()
    }

    #[test]
    fn a_lift_on_each_floor_has_one_landing_door_no_window_and_is_counted_apart_from_the_doors() {
        let mut brief = brief2();
        let r = |key: &str, floor: u32| json!({ "key": key, "name": "مصعد", "func": "lift", "area": 4.0, "zone": "service", "light": false, "floor": floor });
        brief["rooms"].as_array_mut().unwrap().extend([r("lift1", 1), r("lift2", 2)]);
        let mut app = App::default();
        app.handle("generate", &json!({ "brief": brief, "struct": loads2() })).unwrap();
        let sel = app.handle("select", &json!({ "index": 0, "struct": loads2() })).unwrap();
        let model = &app.doc.as_ref().unwrap().model;
        for key in ["lift1", "lift2"] {
            let doors = model.elements.values().filter(|e| e.kind == "Door" && (e.props["from"] == key || e.props["to"] == key)).count();
            assert_eq!(doors, 1, "{key} has exactly one door");
            let sp = model.elements.values().find(|e| e.kind == "Space" && e.props["requirementKey"] == key).unwrap();
            assert_eq!(sp.props["glazingRatio"].as_f64(), Some(0.0), "{key} has no window");
        }
        let notes = sel["warnings"].to_string();
        assert!(!notes.contains("'lift1' needs daylight") && !notes.contains("'lift2' needs daylight"), "a shaft needs no daylight: {notes}");
        let q = app.handle("qto", &json!({})).unwrap();
        let model = &app.doc.as_ref().unwrap().model;
        let item = |code: &str| q["items"].as_array().unwrap().iter().find(|i| i["code"] == code).map(|i| i["quantity"].as_f64().unwrap());
        assert_eq!(item("LIFT.SHAFT"), Some(2.0), "one shaft per level served");
        assert_eq!(item("LIFT.LANDING"), Some(2.0), "the landing openings are counted apart");
        let door_total: f64 = q["items"].as_array().unwrap().iter().filter(|i| i["code"].as_str().unwrap().starts_with("DOOR.")).map(|i| i["quantity"].as_f64().unwrap()).sum();
        let all_doors = model.elements.values().filter(|e| e.kind == "Door").count() as f64;
        assert_eq!(door_total, all_doors - 2.0, "no lift landing is priced as an interior door");
    }

    #[test]
    fn a_neighbour_rule_is_validated_and_a_wanted_pair_shares_a_wall_and_a_door() {
        let mut brief = brief2();
        brief["adjacency"] = json!([{ "a": "majlis", "b": "bath1", "weight": 0 }]);
        let mut app = App::default();
        assert!(app.handle("generate", &json!({ "brief": brief.clone(), "struct": loads2() })).unwrap_err().contains("not 0"));
        brief["adjacency"] = json!([{ "a": "majlis", "b": "nobody", "weight": 2 }]);
        assert!(app.handle("generate", &json!({ "brief": brief.clone(), "struct": loads2() })).unwrap_err().contains("not in the brief"));
        brief["adjacency"] = json!([{ "a": "living", "b": "majlis", "weight": 3 }, { "a": "master", "b": "bed1", "weight": -3 }]);
        app.handle("generate", &json!({ "brief": brief, "struct": loads2() })).unwrap();
        app.handle("select", &json!({ "index": 0, "struct": loads2() })).unwrap();
        let model = &app.doc.as_ref().unwrap().model;
        let door_between = |a: &str, b: &str| model.elements.values().any(|e| e.kind == "Door" && ((e.props["from"] == a && e.props["to"] == b) || (e.props["from"] == b && e.props["to"] == a)));
        assert!(door_between("living", "majlis"), "a wanted pair that touches gets a door");
        assert!(!door_between("master", "bed1"), "a forbidden pair never shares a door");
    }

    #[test]
    fn the_precheck_passes_a_generated_model_and_stops_exports_when_the_model_contradicts_itself() {
        let mut app = started2();
        let r = app.handle("precheck", &json!({})).unwrap();
        let errors: Vec<&Value> = r["issues"].as_array().unwrap().iter().filter(|i| i["level"] == "error").collect();
        assert!(errors.is_empty(), "a generated model has no errors: {errors:?}");
        assert!(app.handle("export_ifc", &json!({})).is_ok());
        // Put two rooms of the ground level on top of each other: the exports refuse, naming the code.
        let lid0 = building::levels(&app.doc.as_ref().unwrap().model)[0].1;
        let mut found = app.doc.as_ref().unwrap().model.elements.values().filter(|e| e.kind == "Space" && e.level_id == Some(lid0)).map(|e| e.id).collect::<Vec<_>>();
        found.sort();
        let (a, b) = (found[0], found[1]);
        let pos: Vec<Value> = ["x", "y", "w", "h"].iter().map(|k| app.doc.as_ref().unwrap().model.elements[&a].props[*k].clone()).collect();
        let el = app.doc.as_mut().unwrap().model.elements.get_mut(&b).unwrap();
        for (k, v) in ["x", "y", "w", "h"].iter().zip(pos) {
            el.props.insert((*k).into(), v);
        }
        let r = app.handle("precheck", &json!({})).unwrap();
        assert!(r["issues"].as_array().unwrap().iter().any(|i| i["code"] == "ROOM.OVERLAP" && i["level"] == "error"));
        let e = app.handle("export_ifc", &json!({})).unwrap_err();
        assert!(e.contains("ROOM.OVERLAP"), "{e}");
        assert!(app.handle("export_dxf", &json!({})).is_err() && app.handle("pdf_html", &json!({})).is_err());
        assert!(app.handle("export_ifc", &json!({ "force": true })).is_ok(), "force exports anyway");
    }

    #[test]
    fn a_courtyard_has_no_walls_slab_ceiling_or_door_and_its_neighbours_get_windows_onto_it() {
        let mut brief = brief2();
        brief["rooms"].as_array_mut().unwrap().push(json!({ "key": "court", "name": "فناء", "func": "courtyard", "area": 16.0, "zone": "service", "light": false, "floor": 1 }));
        let mut app = App::default();
        app.handle("generate", &json!({ "brief": brief, "struct": loads2() })).unwrap();
        let sel = app.handle("select", &json!({ "index": 0, "struct": loads2() })).unwrap();
        let model = &app.doc.as_ref().unwrap().model;
        assert!(model.elements.values().filter(|e| e.kind == "Wall").all(|e| e.props["roomA"] != "court" && e.props.get("roomB").is_none_or(|v| v != "court")), "no wall belongs to the courtyard");
        assert!(model.elements.values().filter(|e| e.kind == "Door").all(|e| e.props["from"] != "court" && e.props["to"] != "court"), "nobody enters the courtyard");
        assert!(model.elements.values().filter(|e| e.kind == "Slab").all(|e| e.props["room"] != "court"), "no slab over a courtyard");
        let notes = sel["warnings"].to_string();
        assert!(!notes.contains("'court'"), "no note about the courtyard: {notes}");
        let q = app.handle("qto", &json!({})).unwrap();
        let item = |code: &str| q["items"].as_array().unwrap().iter().find(|i| i["code"] == code).map(|i| i["quantity"].as_f64().unwrap());
        assert!(item("COURT.AREA").is_some_and(|a| a > 10.0), "the courtyard area is counted apart");
        assert!(item("CEILING.AREA.courtyard").is_none() && item("SKIRTING.courtyard").is_none());
    }

    #[test]
    fn the_reference_two_floor_project_has_no_integrity_errors_and_generates_the_same_model_twice() {
        let run = || {
            let mut app = App::default();
            app.handle("generate", &json!({ "brief": brief2(), "struct": loads2() })).unwrap();
            app.handle("select", &json!({ "index": 0, "struct": loads2() })).unwrap();
            let p = app.handle("precheck", &json!({})).unwrap();
            let errors: Vec<String> = p["issues"].as_array().unwrap().iter().filter(|i| i["level"] == "error").map(|i| i["code"].to_string()).collect();
            let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
            for e in app.doc.as_ref().unwrap().model.elements.values() {
                *kinds.entry(e.kind.clone()).or_default() += 1;
            }
            let q = app.handle("qto", &json!({})).unwrap();
            let gross = q["items"].as_array().unwrap().iter().find(|i| i["code"] == "SPACE.GROSS.AREA").map(|i| i["quantity"].as_f64().unwrap());
            (errors, kinds, gross)
        };
        let (errors, kinds, gross) = run();
        assert!(errors.is_empty(), "a freshly generated reference project has no integrity error: {errors:?}");
        for k in ["Space", "Wall", "Door", "Window", "Slab"] {
            assert!(kinds.get(k).copied().unwrap_or(0) > 0, "the reference project has {k} elements: {kinds:?}");
        }
        assert!(gross.is_some_and(|g| g > 0.0), "the gross floor area is counted");
        assert_eq!(run(), (errors, kinds, gross), "the same brief gives the same model");
    }

    #[test]
    fn a_lift_shaft_is_open_in_the_slab_only_where_an_identical_shaft_stands_above() {
        let mut brief = brief2();
        let r = |key: &str, floor: u32| json!({ "key": key, "name": "مصعد", "func": "lift", "area": 4.0, "zone": "service", "light": false, "floor": floor });
        brief["rooms"].as_array_mut().unwrap().extend([r("lift1", 1), r("lift2", 2)]);
        let mut app = App::default();
        app.handle("generate", &json!({ "brief": brief, "struct": loads2() })).unwrap();
        app.handle("select", &json!({ "index": 0, "struct": loads2() })).unwrap();
        let lid0 = building::levels(&app.doc.as_ref().unwrap().model)[0].1;
        let open = |app: &App| levels::level_opts(&app.doc.as_ref().unwrap().model, &StructOpts::default(), Some(lid0)).open_rooms;
        assert!(!open(&app).contains(&"lift1".to_string()), "the generator does not stack the shafts, so the slab stays closed");
        let rect: Vec<Value> = {
            let sp = app.doc.as_ref().unwrap().model.elements.values().find(|e| e.kind == "Space" && e.props["requirementKey"] == "lift1").unwrap();
            ["x", "y", "w", "h"].iter().map(|k| sp.props[*k].clone()).collect()
        };
        let up = app.doc.as_mut().unwrap().model.elements.values_mut().find(|e| e.kind == "Space" && e.props["requirementKey"] == "lift2").unwrap();
        for (k, v) in ["x", "y", "w", "h"].iter().zip(rect) {
            up.props.insert((*k).into(), v);
        }
        let o = open(&app);
        assert!(o.contains(&"lift1".to_string()) && o.contains(&"stair".to_string()), "a stacked shaft opens its slab beside the stair: {o:?}");
    }

    #[test]
    fn two_floors_make_two_levels_with_the_stair_stacked_one_entrance_and_footings_only_below() {
        let mut app = App::default();
        let g = app.handle("generate", &json!({ "brief": brief2(), "struct": loads2() })).unwrap();
        assert!(g["alternatives"].as_array().unwrap().len() >= 2);
        let sel = app.handle("select", &json!({ "index": 0, "struct": loads2() })).unwrap();
        assert_eq!(sel["levels"]["list"].as_array().unwrap().len(), 2);
        assert_eq!(app.doc.as_ref().unwrap().model.elements.values().filter(|e| e.kind == "Level").count(), 2);
        // The stair block is the same rectangle on both levels.
        let rect = |level: usize, key: &str| -> [i64; 4] {
            let e = of_level(&app, level, "Space").into_iter().find(|e| e.props["requirementKey"] == key).unwrap_or_else(|| panic!("{key} on level {level}"));
            [e.props["x"].as_i64().unwrap(), e.props["y"].as_i64().unwrap(), e.props["w"].as_i64().unwrap(), e.props["h"].as_i64().unwrap()]
        };
        assert_eq!(rect(0, "stair"), rect(1, "stair@1"));
        assert_eq!(rect(0, "stairhall"), rect(1, "stairhall@1"));
        assert_eq!(rect(0, "stair")[2], 2500, "the entered stair width");
        assert_eq!(rect(0, "stair")[3], 5000, "the entered stair length");
        // Rooms are where the user put them.
        for k in ["majlis", "living", "dining", "kitchen", "bath1"] {
            of_level(&app, 0, "Space").iter().find(|e| e.props["requirementKey"] == k).unwrap_or_else(|| panic!("{k} on the ground floor"));
        }
        for k in ["master", "bed1", "bed2", "bath2", "bath3"] {
            of_level(&app, 1, "Space").iter().find(|e| e.props["requirementKey"] == k).unwrap_or_else(|| panic!("{k} on the first floor"));
        }
        // One entrance, on the ground floor.
        assert!(of_level(&app, 0, "Door").iter().any(|d| d.props["isEgress"] == json!(true)));
        assert!(of_level(&app, 1, "Door").iter().all(|d| d.props["isEgress"] != json!(true)), "no door to the street upstairs");
        // Footings are under the ground level only; the shaft is open in the ground slab and closed by the roof.
        assert!(of_level(&app, 1, "Footing").is_empty() && !of_level(&app, 0, "Footing").is_empty());
        let slab_rooms = |l: usize| -> Vec<String> { of_level(&app, l, "Slab").iter().map(|e| e.props["room"].as_str().unwrap().to_string()).collect() };
        assert!(!slab_rooms(0).contains(&"stair".to_string()) && slab_rooms(1).contains(&"stair@1".to_string()));
        // Upper columns carry load; the ground ones carry more (their own slab plus what stands on them).
        let max_load = |l: usize| of_level(&app, l, "Column").iter().filter_map(|c| c.props["axialServiceKn"].as_f64()).fold(0.0, f64::max);
        assert!(max_load(1) > 0.0 && max_load(0) > max_load(1));
        // Every upper column stands on a column below it, and the notes say what that costs.
        assert!(building::unsupported_columns(&app.doc.as_ref().unwrap().model).is_empty(), "no upper column is left without support");
        let notes = sel["warnings"].to_string();
        assert!(!notes.contains("no column below") && !notes.contains("not carried"), "{notes}");
        assert!(notes.contains("continue down from the level above"), "{notes}");
        // Every element is valid and the structure is in step.
        assert!(nnx_core::typed::validate_model(&app.doc.as_ref().unwrap().model).is_empty());
        let s = app.handle("sync", &json!({})).unwrap();
        assert_eq!((s["created"].as_u64(), s["removed"].as_u64()), (Some(0), Some(0)));
    }

    /// Both floors of `brief2` marked on a sketch in a 14 x 18 m envelope, with the stair in the middle of the plan and the front
    /// door marked at `entry`. SYNTHETIC sizes.
    fn sketch2(entry: Value) -> Value {
        let r = |x: i64, y: i64, w: i64, h: i64| json!({ "x": x, "y": y, "w": w, "h": h });
        json!({
            "majlis": r(0, 0, 6000, 6000), "living": r(6000, 0, 5000, 6000), "dining": r(0, 6000, 6000, 5000),
            "kitchen": r(8500, 6000, 5500, 5000), "bath1": r(0, 11000, 3000, 2500),
            "master": r(0, 0, 6000, 6000), "bed1": r(6000, 0, 5000, 5000), "bed2": r(0, 6000, 6000, 5000),
            "bath2": r(8500, 6000, 3000, 3000), "bath3": r(0, 11000, 3000, 2500),
            "_stair": r(6000, 6000, 2500, 5000), "_entry": entry
        })
    }

    impl App {
        fn handle_ro_qto(&self, phase: &str) -> Value {
            self.qto(&json!({ "phase": phase })).unwrap()
        }
    }

    fn generated_from(sketch: Value) -> App {
        let mut app = App::default();
        let g = app.handle("generate", &json!({ "brief": brief2(), "struct": loads2(), "placement": sketch })).unwrap();
        assert_eq!(g["alternatives"][0]["from_sketch"], json!(true));
        app.handle("select", &json!({ "index": 0, "struct": loads2() })).unwrap();
        app
    }

    #[test]
    fn a_stair_marked_on_the_sketch_stands_where_it_was_marked_on_every_level() {
        let app = generated_from(sketch2(json!({ "x": 3000, "y": 0 })));
        let rect = |level: usize, key: &str| -> Option<[i64; 4]> {
            let e = of_level(&app, level, "Space").into_iter().find(|e| e.props["requirementKey"] == key)?;
            Some([e.props["x"].as_i64()?, e.props["y"].as_i64()?, e.props["w"].as_i64()?, e.props["h"].as_i64()?])
        };
        assert_eq!(rect(0, "stair"), Some([6000, 6000, 2500, 5000]), "where it was marked, not along the west side");
        assert_eq!(rect(1, "stair@1"), rect(0, "stair"), "stacked on the next level");
        assert_eq!(rect(0, "stairhall"), None, "no strip hall is added: the hall is a room the user marks");
    }

    #[test]
    fn the_front_door_goes_on_the_outside_wall_nearest_to_the_marked_point() {
        let opens_into = |entry: Value| -> String {
            let app = generated_from(sketch2(entry));
            let d = of_level(&app, 0, "Door").into_iter().find(|d| d.props["isEgress"] == json!(true)).expect("an entry door");
            d.props["opensInto"].as_str().unwrap().to_string()
        };
        assert_eq!(opens_into(json!({ "x": 3000, "y": 0 })), "majlis", "on the south wall of the majlis");
        assert_eq!(opens_into(json!({ "x": 14000, "y": 8500 })), "kitchen", "on the east wall of the kitchen, not on the street side");
    }

    #[test]
    fn an_entry_point_far_from_any_outside_wall_is_reported_and_the_street_rule_is_used() {
        let mut app = App::default();
        app.handle("generate", &json!({ "brief": brief2(), "struct": loads2(), "placement": sketch2(json!({ "x": 7000, "y": 9000 })) })).unwrap();
        let sel = app.handle("select", &json!({ "index": 0, "struct": loads2() })).unwrap();
        assert!(sel["warnings"].to_string().contains("not within 1.5 m of an outside wall"), "{}", sel["warnings"]);
    }

    #[test]
    fn the_wall_thickness_the_user_gives_is_the_one_the_walls_are_built_with() {
        let thicknesses = |brief: Value| -> std::collections::BTreeSet<i64> {
            let mut app = App::default();
            app.handle("generate", &json!({ "brief": brief, "struct": loads2(), "placement": sketch2(json!({ "x": 3000, "y": 0 })) })).unwrap();
            app.handle("select", &json!({ "index": 0, "struct": loads2() })).unwrap();
            of_level(&app, 0, "Wall").iter().filter_map(|w| w.props["thickness"].as_i64()).collect()
        };
        let default = thicknesses(brief2());
        assert!(default.contains(&300) && default.contains(&150), "the engine's defaults without the fields: {default:?}");
        let mut b = brief2();
        b["wall_exterior_mm"] = json!(200);
        b["wall_interior_mm"] = json!(200);
        assert_eq!(thicknesses(b), std::collections::BTreeSet::from([200]), "every wall follows the given thickness");
    }

    #[test]
    fn the_phase_of_a_room_is_kept_in_the_stored_brief_and_a_bad_phase_is_refused() {
        let mut brief = brief2();
        let key = brief["rooms"][0]["key"].as_str().unwrap().to_string();
        brief["rooms"][0]["phase"] = json!("NEW");
        let mut app = App::default();
        app.handle("generate", &json!({ "brief": brief, "struct": loads2(), "placement": sketch2(json!({ "x": 3000, "y": 0 })) })).unwrap();
        app.handle("select", &json!({ "index": 0, "struct": loads2() })).unwrap();
        // The stored brief, which a reopened project loads into the room table, still has the phase; the other rooms have none.
        let settings = app.handle("project", &json!({})).unwrap();
        let rooms = settings["settings"]["brief"]["rooms"].as_array().unwrap();
        let phase_of = |k: &str| rooms.iter().find(|r| r["key"] == k).and_then(|r| r["phase"].as_str().map(String::from));
        assert_eq!(phase_of(&key).as_deref(), Some("NEW"), "{rooms:?}");
        assert!(rooms.iter().filter(|r| r["key"] != key.as_str()).all(|r| r["phase"].is_null()), "no phase is invented for the other rooms");
        // A room phased only on its model elements (a project saved before this field existed) is handed back to the room table too.
        let other = rooms.iter().find(|r| r["key"] != key.as_str()).unwrap()["key"].as_str().unwrap().to_string();
        app.handle("set_phase", &json!({ "phases": { other.clone(): "EXISTING" } })).unwrap();
        let settings = app.handle("project", &json!({})).unwrap();
        let back = settings["settings"]["brief"]["rooms"].as_array().unwrap().iter().find(|r| r["key"] == other.as_str()).unwrap()["phase"].as_str().map(String::from);
        assert_eq!(back.as_deref(), Some("EXISTING"));
        // An unknown phase is refused by name, not dropped.
        let mut bad = brief2();
        bad["rooms"][0]["phase"] = json!("BUILT");
        let err = App::default().handle("generate", &json!({ "brief": bad, "struct": loads2(), "placement": sketch2(json!({ "x": 3000, "y": 0 })) })).unwrap_err();
        assert!(err.contains("BUILT") && err.contains("use one of"), "{err}");
    }

    #[test]
    fn the_wall_height_the_user_gives_is_the_storey_height_of_every_level() {
        let built = |brief: Value| -> App {
            let mut app = App::default();
            app.handle("generate", &json!({ "brief": brief, "struct": loads2(), "placement": sketch2(json!({ "x": 3000, "y": 0 })) })).unwrap();
            app.handle("select", &json!({ "index": 0, "struct": loads2() })).unwrap();
            app
        };
        let heights = |app: &App| -> std::collections::BTreeSet<i64> { ["Wall"].iter().flat_map(|k| (0..2).flat_map(move |l| of_level(app, l, k))).filter_map(|w| w.props["height"].as_i64()).collect() };
        let default = built(brief2());
        assert_eq!(heights(&default), std::collections::BTreeSet::from([3000]), "the engine's default without the field");
        let mut b = brief2();
        b["wall_height_mm"] = json!(3600);
        let app = built(b);
        assert_eq!(heights(&app), std::collections::BTreeSet::from([3600]), "every wall on both levels takes the given height");
        // The upper level stands one storey (plus its slab) higher.
        let z1 = |a: &App| nnx_core::model3d::build(&a.doc.as_ref().unwrap().model).levels.iter().find(|l| l.index == 1).map(|l| l.z0).unwrap();
        assert!((z1(&app) - z1(&default) - 0.6).abs() < 1e-6, "{} vs {}", z1(&app), z1(&default));
        // A height that is not positive is refused, not replaced by the default.
        let mut bad = brief2();
        bad["wall_height_mm"] = json!(0);
        let err = App::default().handle("generate", &json!({ "brief": bad, "struct": loads2(), "placement": sketch2(json!({ "x": 3000, "y": 0 })) })).unwrap_err();
        assert!(err.contains("wall height") && err.contains("positive"), "{err}");
    }

    #[test]
    fn construction_phases_are_set_on_rooms_walls_and_openings_and_split_the_quantities() {
        let mut app = generated_from(sketch2(json!({ "x": 3000, "y": 0 })));
        let wall_len = |app: &App, phase: &str| -> f64 { of_level(app, 0, "Wall").iter().filter(|w| w.props.get("phase").and_then(Value::as_str) == Some(phase)).map(|w| w.props["length"].as_f64().unwrap()).sum() };
        assert_eq!(wall_len(&app, "EXISTING") + wall_len(&app, "NEW"), 0.0, "nothing is assumed before the user sets it");
        let err = app.handle("set_phase", &json!({ "phases": { "majlis": "BUILT" } })).unwrap_err();
        assert!(err.contains("not a phase"), "{err}");
        let r = app.handle("set_phase", &json!({ "phases": { "majlis": "EXISTING", "living": "NEW" } })).unwrap();
        assert!(r["svg"].as_str().unwrap().contains("ph-EXISTING") && r["svg"].as_str().unwrap().contains("ph-NEW"), "the plan colours the two phases apart");
        let space = |app: &App, key: &str| of_level(app, 0, "Space").into_iter().find(|e| e.props["requirementKey"] == key).and_then(|e| e.props.get("phase").and_then(Value::as_str).map(String::from));
        assert_eq!(space(&app, "majlis").as_deref(), Some("EXISTING"));
        assert_eq!(space(&app, "living").as_deref(), Some("NEW"));
        assert_eq!(space(&app, "dining"), None, "a room that was not named keeps no phase");
        let shared = of_level(&app, 0, "Wall").into_iter().find(|w| { let (a, b) = (w.props["roomA"].as_str().unwrap(), w.props.get("roomB").and_then(Value::as_str)); (a == "majlis" && b == Some("living")) || (a == "living" && b == Some("majlis")) }).expect("a wall between the two rooms");
        assert_eq!(shared.props.get("phase").and_then(Value::as_str), Some("NEW"), "a wall between an existing and a new room is new work");
        let total = |phase: &str| -> f64 { app.handle_ro_qto(phase)["items"].as_array().unwrap().iter().filter(|i| i["code"] == "WALL.INT.AREA" || i["code"] == "WALL.EXT.AREA").map(|i| i["quantity"].as_f64().unwrap()).sum() };
        let (e, n, u, all) = (total("EXISTING"), total("NEW"), total("UNSET"), total(""));
        assert!(e > 0.0 && n > 0.0 && u > 0.0, "{e} {n} {u}");
        assert!(((e + n + u) - all).abs() < 0.01, "the phases add up to the whole: {e} + {n} + {u} vs {all}");
        app.doc.as_mut().unwrap().undo();
        assert_eq!(space(&app, "majlis"), None, "one undo takes the phases back");
    }

    #[test]
    fn structural_members_take_the_phase_of_the_rooms_they_stand_in() {
        let mut app = generated_from(sketch2(json!({ "x": 3000, "y": 0 })));
        app.handle("set_phase", &json!({ "phases": { "majlis": "EXISTING", "living": "NEW" } })).unwrap();
        let model = &app.doc.as_ref().unwrap().model;
        let phase = |e: &nnx_core::model::Element| e.props.get("phase").and_then(Value::as_str).map(String::from);
        let kinds = ["Column", "Footing", "Slab"];
        let members: Vec<_> = model.elements.values().filter(|e| kinds.contains(&e.kind.as_str())).collect();
        assert!(!members.is_empty());
        let phased = members.iter().filter(|e| phase(e).is_some()).count();
        assert!(phased > 0, "no column, footing or slab got a phase");
        assert!(members.iter().filter_map(|e| phase(e)).all(|p| App::PHASES.contains(&p.as_str())));
        // A slab carries the phase of its own room.
        for e in model.elements.values().filter(|e| e.kind == "Slab") {
            let s = e.typed::<nnx_core::typed::Slab>().unwrap();
            let expect = match s.room.as_str() { "majlis" => Some("EXISTING"), "living" => Some("NEW"), _ => None };
            assert_eq!(phase(e).as_deref(), expect, "slab of room {}", s.room);
        }
        // The columns are counted in some phase table, not only in UNSET.
        let in_phase: f64 = ["EXISTING", "NEW", "UNVERIFIED"].iter().map(|p| app.handle_ro_qto(p)["items"].as_array().unwrap().iter().filter(|i| i["code"] == "COLUMN.COUNT").map(|i| i["quantity"].as_f64().unwrap()).sum::<f64>()).sum();
        assert!(in_phase > 0.0, "every column is still UNSET");
    }

    #[test]
    fn mep_points_and_runs_take_the_phase_of_their_room_or_position_and_are_counted_by_phase() {
        use nnx_core::typed::{MepPoint, MepRun, element_of};
        let mut app = generated_from(sketch2(json!({ "x": 3000, "y": 0 })));
        let (centre, lid) = {
            let m = &app.doc.as_ref().unwrap().model;
            let sp = m.elements.values().filter(|e| e.kind == "Space").filter_map(|e| e.typed::<nnx_core::typed::Space>().ok().map(|s| (s, e.level_id))).collect::<Vec<_>>();
            let centre = |k: &str| sp.iter().find(|(s, _)| s.requirement_key == k).map(|(s, _)| (s.x + s.w / 2, s.y + s.h / 2)).unwrap();
            ((centre("majlis"), centre("living"), centre("dining")), sp[0].1)
        };
        let ((mx, my), (lx, _ly), (dx, dy)) = centre;
        let point = |room: &str, x: i64, y: i64| MepPoint { role: "light".into(), x, y, room: room.into(), watts: Some(10.0), circuit: None, label: None, extra: Default::default() };
        let run = |room: Option<&str>, x: i64, y: i64| MepRun { system: "cable".into(), x1: x, y1: y, z1: 2500, x2: x + 1000, y2: y, z2: 2500, width: 50, height: 50, room: room.map(String::from), role: None, label: None, extra: Default::default() };
        let mut cmds = Vec::new();
        // One light in the existing room, one in the new room, one in a room that was not given a phase.
        for p in [point("majlis", mx, my), point("living", lx, _ly), point("dining", dx, dy)] {
            let mut el = element_of(&p, nnx_core::Source::User);
            el.level_id = lid;
            cmds.push(Command::Create(el));
        }
        // A run that names a room, and a run with no room whose midpoint lies in the existing room.
        for r in [run(Some("living"), mx, my), run(None, mx - 500, my)] {
            cmds.push(Command::Create(nnx_core::typed::run_element(&r, lid, nnx_core::Source::User).unwrap()));
        }
        app.doc.as_mut().unwrap().commit("mep", nnx_core::Origin::User, cmds).unwrap();
        app.handle("set_phase", &json!({ "phases": { "majlis": "EXISTING", "living": "NEW" } })).unwrap();
        let phase = |e: &nnx_core::Element| e.props.get("phase").and_then(Value::as_str).map(String::from);
        let model = &app.doc.as_ref().unwrap().model;
        let by_room = |room: &str| model.elements.values().filter(|e| e.kind == "MepPoint").find(|e| e.typed::<MepPoint>().unwrap().room == room).and_then(phase);
        assert_eq!(by_room("majlis").as_deref(), Some("EXISTING"));
        assert_eq!(by_room("living").as_deref(), Some("NEW"));
        assert_eq!(by_room("dining"), None, "a room with no phase gives its point no phase: none is invented");
        let mut runs: Vec<(Option<String>, Option<String>)> = model.elements.values().filter(|e| e.kind == "MepRun").map(|e| (e.typed::<MepRun>().unwrap().room, phase(e))).collect();
        runs.sort();
        assert_eq!(runs, vec![(None, Some("EXISTING".to_string())), (Some("living".to_string()), Some("NEW".to_string()))], "by the named room, else by position");
        // The counts are split by phase and add up to the whole.
        let count = |p: &str| -> f64 { app.handle_ro_qto(p)["items"].as_array().unwrap().iter().filter(|i| i["code"] == "MEP.light").map(|i| i["quantity"].as_f64().unwrap()).sum() };
        assert_eq!((count("EXISTING"), count("NEW"), count("UNSET"), count("")), (1.0, 1.0, 1.0, 3.0));
    }

    #[test]
    fn phase_quantity_exports_reconcile_with_unfiltered_items() {
        let mut app = generated_from(sketch2(json!({ "x": 3000, "y": 0 })));
        app.handle("set_phase", &json!({ "phases": { "majlis": "EXISTING", "living": "NEW" } })).unwrap();
        let full = app.handle_ro_qto("");
        let mut per_phase: BTreeMap<String, f64> = BTreeMap::new();
        for phase in ["EXISTING", "NEW", "UNSET", "TO_BE_DEMOLISHED", "TO_BE_MODIFIED", "TEMPORARY", "UNVERIFIED"] {
            let q = app.handle_ro_qto(phase);
            for item in q["items"].as_array().unwrap() {
                *per_phase.entry(item["code"].as_str().unwrap().to_string()).or_default() += item["quantity"].as_f64().unwrap();
            }
        }
        for item in full["items"].as_array().unwrap() {
            let code = item["code"].as_str().unwrap();
            let total = item["quantity"].as_f64().unwrap();
            let allocated = per_phase.get(code).copied().unwrap_or_default();
            assert!((total - allocated).abs() < 0.015, "{code}: whole={total}, phases={allocated}");
        }
    }

    #[test]
    fn named_features_are_stored_drawn_quantified_and_undone_as_stated() {
        let mut app = generated_from(sketch2(json!({ "x": 3000, "y": 0 })));
        let svg0 = app.render().unwrap();
        assert!(!svg0.contains("class=\"feat feat-"), "no features before any is added");
        let err = app.handle("add_feature", &json!({ "role": "MOAT", "shape": "line", "x1": 0, "y1": 0, "x2": 0, "y2": 5000 })).unwrap_err();
        assert!(err.contains("not a feature role"), "{err}");
        let err = app.handle("add_feature", &json!({ "role": "EXPANSION_JOINT", "shape": "line", "x1": 100, "y1": 100, "x2": 100, "y2": 100, "width": 50 })).unwrap_err();
        assert!(err.contains("two different end points"), "{err}");
        let r = app.handle("add_feature", &json!({ "role": "EXPANSION_JOINT", "shape": "line", "x1": 6000, "y1": 0, "x2": 6000, "y2": 8000, "width": 50, "name": "EJ", "phase": "NEW" })).unwrap();
        assert!(r["svg"].as_str().unwrap().contains("class=\"feat feat-EXPANSION_JOINT"), "drawn on the plan");
        app.handle("add_feature", &json!({ "role": "SHAFT", "shape": "rect", "x1": 1000, "y1": 1000, "x2": 1552, "y2": 1552, "name": "SH1" })).unwrap();
        app.handle("add_feature", &json!({ "role": "SHAFT", "shape": "rect", "x1": 3000, "y1": 1000, "x2": 3552, "y2": 1552, "name": "SH2" })).unwrap();
        let q = app.handle("qto", &json!({})).unwrap();
        let get = |code: &str| q["items"].as_array().unwrap().iter().find(|i| i["code"] == code).map(|i| i["quantity"].as_f64().unwrap());
        assert_eq!(get("FEATURE.EXPANSION_JOINT.COUNT"), Some(1.0));
        assert_eq!(get("FEATURE.EXPANSION_JOINT.LEN"), Some(8.0), "metres, from the stated end points");
        assert_eq!(get("FEATURE.SHAFT.COUNT"), Some(2.0));
        assert!((get("FEATURE.SHAFT.AREA").unwrap() - 2.0 * 0.552 * 0.552).abs() < 1e-3);
        let only_new = app.handle("qto", &json!({ "phase": "NEW" })).unwrap();
        assert!(only_new["items"].as_array().unwrap().iter().any(|i| i["code"] == "FEATURE.EXPANSION_JOINT.COUNT") && !only_new["items"].as_array().unwrap().iter().any(|i| i["code"] == "FEATURE.SHAFT.COUNT"), "the phase filter applies to features");
        let list = app.handle("features", &json!({ "level": 0 })).unwrap();
        assert_eq!(list["features"].as_array().unwrap().len(), 3);
        assert_eq!(app.handle("features", &json!({ "level": 1 })).unwrap()["features"].as_array().unwrap().len(), 0, "a feature belongs to its level");
        app.doc.as_mut().unwrap().undo();
        assert_eq!(app.handle("features", &json!({})).unwrap()["features"].as_array().unwrap().len(), 2, "one undo removes the last feature");
    }

    #[test]
    fn a_stair_on_the_sketch_needs_every_level_marked() {
        let mut sketch = sketch2(json!({ "x": 3000, "y": 0 }));
        sketch.as_object_mut().unwrap().remove("bath3");
        let err = App::default().handle("generate", &json!({ "brief": brief2(), "struct": loads2(), "placement": sketch })).unwrap_err();
        assert!(err.contains("every room of every level must be marked"), "{err}");
    }

    #[test]
    fn every_level_can_be_shown_with_its_own_plan_and_rooms() {
        let mut app = started2();
        let ground = app.render().unwrap();
        assert!(ground.contains("Ground floor") && ground.contains("المجلس") && !ground.contains("الماستر"));
        let up = app.handle("level", &json!({ "index": 1 })).unwrap();
        let svg = up["svg"].as_str().unwrap();
        assert!(svg.contains("Floor 1") && svg.contains("الماستر") && !svg.contains("المجلس"));
        assert_eq!(up["levels"]["current"], 1);
        let rooms: Vec<String> = up["rooms"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap().to_string()).collect();
        assert!(rooms.contains(&"الماستر".to_string()) && !rooms.contains(&"المجلس".to_string()));
        assert!(app.handle("level", &json!({ "index": 5 })).is_err());
    }

    #[test]
    fn bad_multi_floor_inputs_are_refused_by_name() {
        let mut app = App::default();
        let mut no_stair = brief2();
        no_stair.as_object_mut().unwrap().remove("stair");
        assert!(app.handle("generate", &json!({ "brief": no_stair, "struct": loads2() })).unwrap_err().contains("stair"));
        let mut too_high = brief2();
        too_high["rooms"][0]["floor"] = json!(3);
        assert!(app.handle("generate", &json!({ "brief": too_high, "struct": loads2() })).unwrap_err().contains("floor 3"));
        let mut dup = brief2();
        dup["rooms"][5]["key"] = json!("majlis");
        assert!(app.handle("generate", &json!({ "brief": dup, "struct": loads2() })).unwrap_err().contains("unique"));
        let mut long = brief2();
        long["stair"]["length_m"] = json!(17.5);
        assert!(app.handle("generate", &json!({ "brief": long, "struct": loads2() })).unwrap_err().contains("hall"));
        let mut side = brief2();
        side["stair"]["side"] = json!("up");
        assert!(app.handle("generate", &json!({ "brief": side, "struct": loads2() })).unwrap_err().contains("stair side"));
        // three floors with rooms on two: the third repeats the second and says so
        let three = brief2();
        let s3 = json!({ "dead_kpa": 5.0, "live_kpa": 2.0, "bearing_kpa": 150.0, "floors": 3, "max_span_mm": 5000 });
        app.handle("generate", &json!({ "brief": three.clone(), "struct": s3 })).unwrap();
        let sel = app.handle("select", &json!({ "index": 0, "struct": s3 })).unwrap();
        assert!(sel["warnings"].to_string().contains("floor 3 has no rooms of its own"));
        assert_eq!(sel["levels"]["list"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn an_edit_on_the_upper_floor_leaves_the_ground_floor_alone_and_one_undo_restores_everything() {
        let mut app = started2();
        let before = hash(&app);
        let ground_before: Vec<String> = of_level(&app, 0, "Wall").iter().map(|e| format!("{}{:?}", e.id, e.props)).collect();
        app.handle("level", &json!({ "index": 1 })).unwrap();
        let lid = building::levels(&app.doc.as_ref().unwrap().model)[1].1;
        let walls: Vec<String> = app.doc.as_ref().unwrap().model.elements.values().filter(|e| e.kind == "Wall" && e.level_id == Some(lid) && e.props["isExterior"] == json!(false)).map(|e| e.id.to_string()).collect();
        let mut moved = false;
        for w in &walls {
            for d in [300, -300] {
                match app.handle("move_partition", &json!({ "wall": w, "delta_mm": d })) {
                    Ok(_) => {
                        moved = true;
                        break;
                    }
                    Err(_) => assert!(hash(&app) == before || moved, "a refused edit changes nothing"),
                }
            }
            if moved {
                break;
            }
        }
        assert!(moved, "some upper partition can move");
        let ground_after: Vec<String> = of_level(&app, 0, "Wall").iter().map(|e| format!("{}{:?}", e.id, e.props)).collect();
        assert_eq!(ground_before, ground_after, "the ground floor walls are untouched");
        assert!(nnx_core::typed::validate_model(&app.doc.as_ref().unwrap().model).is_empty());
        let s = app.handle("sync", &json!({})).unwrap();
        assert_eq!((s["created"].as_u64(), s["removed"].as_u64()), (Some(0), Some(0)), "structure of every level was synced inside the edit");
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(hash(&app), before, "one undo restores both levels");
    }

    #[test]
    fn the_stair_block_cannot_be_moved() {
        let mut app = started2();
        let doc = app.doc.as_ref().unwrap();
        let wall = doc.model.elements.values().find(|e| e.kind == "Wall" && e.props["isExterior"] == json!(false) && (e.props["roomA"].as_str().is_some_and(|k| k.starts_with("stair")) || e.props["roomB"].as_str().is_some_and(|k| k.starts_with("stair")))).map(|e| e.id.to_string()).expect("a wall of the stair block");
        let before = hash(&app);
        let e = app.handle("move_partition", &json!({ "wall": wall, "delta_mm": 300 })).unwrap_err();
        assert!(e.contains("stair block"), "{e}");
        assert_eq!(hash(&app), before);
    }

    #[test]
    fn two_floors_save_reopen_and_measure_both_levels() {
        let mut app = started2();
        let q = app.handle("qto", &json!({})).unwrap();
        let single = {
            let mut one = App::default();
            one.handle("generate", &json!({ "brief": brief(), "struct": { "dead_kpa": 5.0, "live_kpa": 2.0, "bearing_kpa": 150.0, "max_span_mm": 5000 } })).unwrap();
            one.handle("select", &json!({ "index": 0, "struct": { "dead_kpa": 5.0, "live_kpa": 2.0, "bearing_kpa": 150.0, "max_span_mm": 5000 } })).unwrap();
            one.handle("qto", &json!({})).unwrap()
        };
        let area = |q: &Value, code: &str| q["items"].as_array().unwrap().iter().find(|i| i["code"] == code).map(|i| i["quantity"].as_f64().unwrap()).unwrap_or(0.0);
        // two floors of the same plot: about twice the gross area of one floor of it (the shaft is open in the ground slab)
        assert!(area(&q, "SPACE.GROSS.AREA") > 1.8 * area(&single, "SPACE.GROSS.AREA"));
        let (model, settings) = (app.model().unwrap().clone(), app.settings());
        let mut second = App::default();
        let opened = second.open(model, &settings).unwrap();
        assert_eq!(opened["levels"]["list"].as_array().unwrap().len(), 2);
        assert!(opened["warnings"].as_array().unwrap().is_empty(), "{}", opened["warnings"]);
        assert_eq!(second.project().unwrap().brief.unwrap().stair.unwrap().side, "west");
        assert_eq!(second.handle("sync", &json!({})).unwrap()["created"], 0);
        assert_eq!(second.handle("qto", &json!({})).unwrap()["csv"], q["csv"]);
        assert!(second.layout_brief().is_ok(), "edits still work after reopening");
    }

    #[test]
    fn the_3d_model_stacks_the_levels_and_the_roof_insulation_counts_only_the_top_slabs() {
        let mut app = started2();
        let scene = app.handle("model3d", &json!({})).unwrap();
        let lv = scene["levels"].as_array().unwrap();
        assert_eq!(lv.len(), 2);
        let (z0, z1, st0) = (lv[0]["z0"].as_f64().unwrap(), lv[1]["z0"].as_f64().unwrap(), lv[0]["storey"].as_f64().unwrap());
        assert_eq!(z0, 0.0);
        assert!(z1 > st0 && z1 < st0 + 0.5, "the upper floor sits on the ground slab: {z1} vs storey {st0}");
        let top = scene["boxes"].as_array().unwrap().iter().map(|b| b["max"][2].as_f64().unwrap()).fold(0.0, f64::max);
        assert!(top > z1 + st0, "the second storey stands on the first: {top}");
        // roof insulation: only the top level's slabs
        let inputs = json!({ "insulation": { "assemblies": { "roof": { "rsi": 0.1, "rse": 0.04, "layers": [{ "material": "concrete", "thickness_mm": 150, "lambda": 1.5 }] } } } });
        let r = app.handle("mep", &json!({ "inputs": inputs, "apply": false })).unwrap();
        let roof = r["report"]["insulation"].as_array().unwrap().iter().find(|a| a["scope"] == "roof").unwrap().clone();
        let top_area: f64 = of_level(&app, 1, "Slab").iter().map(|s| s.props["w"].as_f64().unwrap() * s.props["h"].as_f64().unwrap() / 1e6).sum();
        assert!((roof["area_m2"].as_f64().unwrap() - top_area).abs() < 0.01, "{} vs {top_area}", roof["area_m2"]);
    }

    #[test]
    fn mep_points_belong_to_their_level_and_move_with_nothing_else() {
        let mut app = started2();
        let mut i = mep_inputs();
        i["electrical"]["panel_room"] = json!("living");
        i["hvac"]["roof_exposed"] = json!(true);
        let r = app.handle("mep", &json!({ "inputs": i, "apply": true })).unwrap();
        assert!(r["report"]["warnings"].to_string().contains("risers"), "{}", r["report"]["warnings"]);
        let on = |l: usize| of_level(&app, l, "MepPoint").len();
        assert!(on(0) > 0 && on(1) > 0 && on(0) + on(1) == mep_points(&app));
        // a point sits on the level of the room that holds it
        for l in 0..2 {
            let keys: Vec<String> = of_level(&app, l, "Space").iter().map(|e| e.props["requirementKey"].as_str().unwrap().to_string()).collect();
            assert!(of_level(&app, l, "MepPoint").iter().all(|p| keys.contains(&p.props["room"].as_str().unwrap().to_string())));
        }
        // air-conditioning is computed for the rooms of both levels
        let hv = r["report"]["hvac"]["rooms"].as_array().unwrap();
        for k in ["living", "master"] {
            assert!(hv.iter().any(|x| x["room"] == k), "{k} has a cooling load");
        }
        // applying the same inputs again changes nothing
        let again = app.handle("mep", &json!({ "inputs": i, "apply": true })).unwrap();
        assert_eq!(again["applied"], json!({ "created": 0, "removed": 0 }));
    }

    #[test]
    fn no_opening_stands_in_a_column_after_generation_edits_and_reopening() {
        use nnx_core::typed::{Column, Typed, Wall};
        let mut app = started2();
        let model_levels = |app: &App| -> Vec<Option<nnx_core::Id>> { building::levels(&app.doc.as_ref().unwrap().model).iter().map(|(_, id)| Some(*id)).collect() };
        let (fix, unresolved) = building::clear_openings(&app.doc.as_ref().unwrap().model, &model_levels(&app));
        assert!(fix.is_empty(), "every movable opening was already moved: {} left", fix.len());
        let sel_notes = app.handle("select", &json!({ "index": 0, "struct": loads2() })).unwrap()["warnings"].to_string();
        assert!(!sel_notes.contains("stand in a door or window opening") || !unresolved.is_empty(), "{sel_notes}");
        // The structure is in step, so a sync changes nothing, openings included.
        let before = hash(&app);
        let s = app.handle("sync", &json!({})).unwrap();
        assert_eq!((s["created"].as_u64(), s["removed"].as_u64()), (Some(0), Some(0)));
        assert_eq!(hash(&app), before, "settled: nothing moves again");

        // A position typed by the user is refused when it would put an opening in a column, and nothing changes.
        let model = &app.doc.as_ref().unwrap().model;
        let mut target = None;
        'find: for o in model.elements.values().filter(|e| e.kind == "Door" || e.kind == "Window") {
            let Some(w) = o.host_id.and_then(|h| model.get(&h)).and_then(|h| h.typed::<Wall>().ok()) else { continue };
            for c in model.elements.values().filter(|e| e.kind == Column::KIND && e.level_id == o.level_id).filter_map(|e| e.typed::<Column>().ok()) {
                let (along, across) = if w.horizontal() { (c.x, c.y) } else { (c.y, c.x) };
                if (across - w.centre_line()).abs() <= w.thickness / 2 && along > w.start() + 600 && along < w.start() + w.length - 600 {
                    target = Some((o.id.to_string(), along - w.start()));
                    break 'find;
                }
            }
        }
        let (id, offset) = target.expect("some wall has a column along it");
        let e = app.handle("set_opening", &json!({ "id": id, "offset": offset })).unwrap_err();
        assert!(e.contains("column"), "{e}");
        assert_eq!(hash(&app), before);

        // After saving and reopening, nothing needs to move either.
        let (model, settings) = (app.model().unwrap().clone(), app.settings());
        let mut second = App::default();
        let opened = second.open(model, &settings).unwrap();
        assert!(opened["warnings"].as_array().unwrap().is_empty(), "{}", opened["warnings"]);
    }

    #[test]
    fn ifc_export_has_every_element_of_both_levels_and_keeps_the_arabic_names() {
        let mut app = started2();
        app.handle("mep", &json!({ "inputs": mep_inputs(), "apply": true })).unwrap();
        let r = app.handle("export_ifc", &json!({})).unwrap();
        let text = r["ifc"].as_str().unwrap();
        let model = &app.doc.as_ref().unwrap().model;
        let n = |kind: &str| model.elements.values().filter(|e| e.kind == kind).count() as u64;
        let c = |class: &str| r["counts"][class].as_u64().unwrap_or(0);
        assert_eq!((c("IfcWall"), c("IfcDoor"), c("IfcWindow"), c("IfcSpace")), (n("Wall"), n("Door"), n("Window"), n("Space")));
        assert_eq!((c("IfcSlab"), c("IfcColumn"), c("IfcBeam"), c("IfcFooting")), (n("Slab"), n("Column"), n("Beam"), n("Footing")));
        assert_eq!(c("IfcLightFixture") + c("IfcOutlet") + c("IfcElectricDistributionBoard") + c("IfcSanitaryTerminal") + c("IfcUnitaryEquipment"), n("MepPoint"));
        assert_eq!(text.matches("IFCBUILDINGSTOREY(").count(), 2);
        assert_eq!(text.matches("IFCOPENINGELEMENT(").count() as u64, n("Door") + n("Window"));
        assert!(text.is_ascii() && text.contains("\\X2\\"), "Arabic room names are escaped");
        assert!(text.contains("FILE_NAME('model.ifc','20"), "a UTC timestamp is written");
        assert!(App::default().handle("export_ifc", &json!({})).is_err());
        assert_eq!(r["profile"], "standard");
        assert!(!text.contains("IFCWALLTYPE("), "the standard profile writes no type objects");
        let rv = app.handle("export_ifc", &json!({ "profile": "revit" })).unwrap();
        let rt = rv["ifc"].as_str().unwrap();
        assert_eq!(rv["profile"], "revit");
        assert!(rt.contains("IFCWALLTYPE(") && rt.contains("IFCRELDEFINESBYTYPE(") && rt.contains("'Pset_DoorCommon'"));
        assert_eq!(rv["counts"]["IfcWall"], r["counts"]["IfcWall"], "the same products in both profiles");
        assert!(app.handle("export_ifc", &json!({ "profile": "archicad" })).unwrap_err().contains("unknown profile"));
        if let Ok(path) = std::env::var("NANOARCH_IFC_OUT") {
            std::fs::write(path, text).unwrap();
        }
    }

    #[test]
    fn export_bundle_packs_chosen_files_into_numbered_folders_and_refuses_bad_requests() {
        let mut app = started2();
        app.handle("mep", &json!({ "inputs": mep_inputs(), "apply": true })).unwrap();
        let r = app.handle("export_bundle", &json!({ "include": ["dxf", "ifc", "plan_html"], "lang": "en" })).unwrap();
        let paths: Vec<String> = r["paths"].as_array().unwrap().iter().map(|p| p.as_str().unwrap().to_string()).collect();
        assert_eq!(paths.len(), 4);
        assert_eq!(paths[0], "00-index/manifest.json");
        assert!(paths[1].starts_with("01-models/") && paths[1].ends_with(".ifc"), "{paths:?}");
        assert!(paths[2].starts_with("02-drawings/") && paths[3].starts_with("02-drawings/"), "{paths:?}");
        let zip = nnx_core::xlsx::base64_decode(r["zip_b64"].as_str().unwrap()).unwrap();
        assert_eq!(&zip[..4], b"PK");
        assert!(r["name"].as_str().unwrap().ends_with("-delivery.zip"));
        let m: Value = serde_json::from_str(r["manifest"].as_str().unwrap()).unwrap();
        assert_eq!(m["file_count"], 3);
        // nothing is chosen for the user
        assert!(app.handle("export_bundle", &json!({})).unwrap_err().contains("include"));
        assert!(app.handle("export_bundle", &json!({ "include": [] })).unwrap_err().contains("empty"));
        assert!(app.handle("export_bundle", &json!({ "include": ["dwg"] })).unwrap_err().contains("unknown item"));
        assert!(app.handle("export_bundle", &json!({ "include": ["ifc", "ifc"] })).unwrap_err().contains("twice"));
        // no saved BOQ inputs: the priced workbook is refused, not faked
        assert!(app.handle("export_bundle", &json!({ "include": ["boq"] })).unwrap_err().contains("BOQ inputs"));
        assert!(App::default().handle("export_bundle", &json!({ "include": ["ifc"] })).is_err());
    }

    #[test]
    fn dxf_export_applies_an_office_layer_style_and_refuses_a_bad_one() {
        let mut app = started2();
        app.handle("mep", &json!({ "inputs": mep_inputs(), "apply": true })).unwrap();
        let plain = app.handle("export_dxf", &json!({})).unwrap();
        let style = json!({ "name": "Office", "source": "Office CAD manual, section 4", "layers": [{ "layer": "S-COLS", "name": "STR-COLUMN", "color": 11, "linetype": "HIDDEN" }] });
        let styled = app.handle("export_dxf", &json!({ "layer_style": style })).unwrap();
        assert_eq!(styled["counts"]["STR-COLUMN"], plain["counts"]["S-COLS"]);
        assert!(styled["counts"].get("S-COLS").is_none());
        assert!(styled["dxf"].as_str().unwrap().contains("STR-COLUMN") && !styled["dxf"].as_str().unwrap().contains("S-COLS"));
        assert_eq!(styled["layer_style"]["applied"][0]["used"], true);
        assert_eq!(styled["layer_style"]["source"], "Office CAD manual, section 4");
        // no style: the engine's own layers
        assert!(plain.get("layer_style").is_none());
        let no_source = json!({ "name": "Office", "source": "", "layers": [{ "layer": "S-COLS", "color": 11 }] });
        assert!(app.handle("export_dxf", &json!({ "layer_style": no_source })).unwrap_err().contains("source"));
        assert!(app.handle("export_dxf", &json!({ "layer_style": { "name": "x" } })).unwrap_err().contains("layer_style"));
    }

    #[test]
    fn dxf_export_draws_the_shown_level_or_all_of_them_with_every_wall_cut_and_named_rooms() {
        let mut app = started2();
        app.handle("mep", &json!({ "inputs": mep_inputs(), "apply": true })).unwrap();
        let ground = app.handle("export_dxf", &json!({})).unwrap();
        let layer = |r: &Value, l: &str| r["counts"][l].as_u64().unwrap_or(0);
        let model = app.doc.as_ref().unwrap().model.clone();
        let level_ids: Vec<nnx_core::Id> = building::levels(&model).iter().map(|(_, id)| *id).collect();
        let on_level = |kind: &str, lvl: nnx_core::Id| model.elements.values().filter(|e| e.kind == kind && e.level_id == Some(lvl)).count() as u64;
        assert_eq!(ground["levels"], 1);
        assert_eq!(layer(&ground, "A-ROOM"), on_level("Space", level_ids[0]));
        assert_eq!(layer(&ground, "S-COLS"), 2 * on_level("Column", level_ids[0]), "fill and outline per column");
        assert_eq!(layer(&ground, "S-BEAM"), on_level("Beam", level_ids[0]));
        assert!(layer(&ground, "A-WALL-EXT") + layer(&ground, "A-WALL-INT") >= on_level("Wall", level_ids[0]), "a wall gives at least one piece");
        assert_eq!(layer(&ground, "A-GLAZ"), 2 * on_level("Window", level_ids[0]));
        assert_eq!(layer(&ground, "A-DOOR"), 4 * on_level("Door", level_ids[0]), "two jambs, a leaf and an arc per door");
        assert!(layer(&ground, "E-LITE") > 0 && layer(&ground, "A-STAIR") == 2);
        let dxf = ground["dxf"].as_str().unwrap();
        assert!(dxf.is_ascii() && dxf.contains("\\U+") && dxf.contains("Ground floor"), "Arabic names are escaped, the level is titled");
        // the upper level only
        app.handle("level", &json!({ "index": 1 })).unwrap();
        let up = app.handle("export_dxf", &json!({})).unwrap();
        assert_eq!(layer(&up, "A-ROOM"), on_level("Space", level_ids[1]));
        assert_eq!(layer(&up, "S-FTNG"), 0, "no footings on the upper level");
        // all levels side by side
        let all = app.handle("export_dxf", &json!({ "all": true })).unwrap();
        assert_eq!(all["levels"], 2);
        assert_eq!(layer(&all, "A-ROOM"), layer(&ground, "A-ROOM") + layer(&up, "A-ROOM"));
        assert!(App::default().handle("export_dxf", &json!({})).is_err());
        if let Ok(path) = std::env::var("NANOARCH_DXF_OUT") {
            std::fs::write(path, all["dxf"].as_str().unwrap()).unwrap();
        }
    }

    #[test]
    fn the_pdf_document_has_a_sheet_per_level_with_the_current_drawing_options() {
        let mut app = started2();
        let ar = app.handle("pdf_html", &json!({})).unwrap();
        let html = ar["html"].as_str().unwrap();
        assert_eq!(ar["name"], "plan.pdf");
        assert_eq!(html.matches("<section class=\"sheet\">").count(), 2);
        assert!(html.contains("dir=\"rtl\"") && html.contains("المجلس") && html.contains("الماستر"));
        assert!(html.contains("20") && html.contains("ورقة <bdi dir=\"ltr\">2/2</bdi>"));
        assert!(!html.contains("class=\"column\""), "structure is drawn only when it is switched on");
        app.handle("svg", &json!({ "show_structure": true })).unwrap();
        let on = app.handle("pdf_html", &json!({ "lang": "en" })).unwrap();
        let on = on["html"].as_str().unwrap();
        assert!(on.contains("class=\"column\"") && on.contains("dir=\"ltr\"") && on.contains("Sheet <bdi dir=\"ltr\">1/2</bdi>"));
        assert!(on.contains("Code-check statement") && on.contains("Rules not run"), "the sheets end with the code-check statement");
        assert!(on.contains("Door and window schedule (mm)") && on.contains("<td>D1</td>") && on.contains("<td>W1</td>"), "each sheet carries its door and window schedule");
        assert!(App::default().handle("pdf_html", &json!({})).is_err());
    }

    // ------------------------------------------------------------------ project (single source of truth)

    fn started_with_project() -> App {
        let mut app = App::default();
        let meta = json!({ "title": "x", "buildingType": "villa", "style": "najdi", "smartTech": ["lighting"], "climateZone": "zone-a" });
        let g = app.handle("generate", &json!({ "brief": brief(), "struct": {}, "meta": meta })).unwrap();
        app.handle("select", &json!({ "index": generator_best(&g), "struct": {} })).unwrap();
        app
    }

    /// Alternatives are ranked by the declared scores; some tests assert things about the generator's own best layout
    /// (lowest annealing cost), which is not always the top-scored one.
    fn generator_best(g: &Value) -> usize {
        let alts = g["alternatives"].as_array().unwrap();
        (0..alts.len()).min_by(|&a, &b| alts[a]["cost"].as_f64().unwrap().total_cmp(&alts[b]["cost"].as_f64().unwrap())).unwrap()
    }

    fn setbacks(front: f64, back: f64, side: f64) -> Value {
        json!({ "setbacks": { "front_m": front, "back_m": back, "side_m": side } })
    }

    #[test]
    fn the_project_lives_in_the_model_and_the_page_state_is_derived_from_it() {
        let app = started_with_project();
        let p = app.project().expect("generation stores the project in the model");
        assert_eq!(p.meta.building_type, Some(nnx_core::project::BuildingType::Villa));
        assert_eq!(p.meta.climate_zone.as_deref(), Some("zone-a"));
        assert_eq!(p.meta.smart_tech, vec!["lighting".to_string()]);
        assert_eq!(p.brief.as_ref().unwrap().setback_front_m, 2.0);
        assert_eq!(p.labels.get("majlis").map(String::as_str), Some("المجلس"));
        // Only view state is left outside the model.
        let keys: Vec<String> = app.settings().keys().cloned().collect();
        assert_eq!(keys, vec!["show_mep".to_string(), "show_structure".to_string()]);
        assert_eq!(app.form_state()["brief"]["title"], "فيلا تجريبية");
    }

    #[test]
    fn a_setback_change_is_one_undoable_step_and_marks_the_plan_outdated() {
        let mut app = started_with_project();
        let (before, clean) = (hash(&app), app.is_dirty());
        assert!(app.is_dirty(), "a generated plan is unsaved");
        app.mark_saved();
        assert!(!app.is_dirty());
        assert_eq!(app.handle("project", &json!({})).unwrap()["plan_outdated"], false);

        let r = app.handle("project_set", &setbacks(3.0, 2.0, 2.0)).unwrap();
        assert_eq!(r["changed"], true);
        assert!(app.is_dirty(), "a project change makes the file dirty");
        assert_eq!(r["plan_outdated"], true, "the drawn plan no longer matches plot minus setbacks");
        assert_eq!(app.project().unwrap().brief.unwrap().setback_front_m, 3.0);

        // The same value again is not a change.
        let again = app.handle("project_set", &setbacks(3.0, 2.0, 2.0)).unwrap();
        assert_eq!(again["changed"], false);

        let u = app.handle("undo", &json!({})).unwrap();
        assert_eq!(u["changed"], true);
        assert_eq!(app.project().unwrap().brief.unwrap().setback_front_m, 2.0);
        assert!(!app.is_dirty(), "undoing back to the saved state is clean again");
        assert_eq!(app.handle("project", &json!({})).unwrap()["plan_outdated"], false);
        app.handle("redo", &json!({})).unwrap();
        assert_eq!(app.project().unwrap().brief.unwrap().setback_front_m, 3.0);
        app.handle("undo", &json!({})).unwrap();
        let _ = (before, clean);
        assert!(!app.is_dirty());
    }

    #[test]
    fn impossible_or_unsourced_project_inputs_are_refused_and_change_nothing() {
        let mut app = started_with_project();
        let before = hash(&app);
        let e = app.handle("project_set", &setbacks(2.0, 2.0, 12.0)).unwrap_err();
        assert!(e.contains("no buildable area"), "{e}");
        let e = app.handle("project_set", &setbacks(-1.0, 2.0, 2.0)).unwrap_err();
        assert!(e.contains("zero or more"), "{e}");
        let e = app.handle("project_set", &json!({ "limits": { "minSetbackFrontM": { "value": 3.0, "source": "" } } })).unwrap_err();
        assert!(e.contains("source"), "{e}");
        let e = app.handle("project_set", &json!({ "limits": { "maxCoverageRatio": { "value": 1.5, "source": "x" } } })).unwrap_err();
        assert!(e.contains("out of range"), "{e}");
        // An outline that crosses itself is refused; an L-shaped one is a valid plot.
        let bowtie = json!({ "meta": { "plotPolygonM": [[0, 0], [18, 22], [18, 0], [0, 22]] } });
        assert!(app.handle("project_set", &bowtie).unwrap_err().contains("crosses itself"));
        let two_vertices = json!({ "meta": { "plotPolygonM": [[0, 0], [18, 22]] } });
        assert!(app.handle("project_set", &two_vertices).unwrap_err().contains("at least 3 vertices"));
        let bad_edges = json!({ "meta": { "plotPolygonM": [[0, 0], [18, 0], [18, 10], [8, 10], [8, 22], [0, 22]], "plotEdges": [{ "kind": "front_street" }] } });
        assert!(app.handle("project_set", &bad_edges).unwrap_err().contains("edge descriptions"));
        let wrong = json!({ "meta": { "plotPolygonM": [[0, 0], [10, 0], [10, 10], [0, 10]] } });
        assert!(app.handle("project_set", &wrong).unwrap_err().contains("brief says"));
        assert_eq!(hash(&app), before);
        assert!(app.handle("project_set", &json!({ "meta": { "plotPolygonM": [[0, 0], [18, 0], [18, 22], [0, 22]] } })).is_ok());
        // An L-shaped outline is accepted, the area of the plot is the outline's, and the front/rear/side limits no longer apply.
        let l_shape = json!({ "meta": { "plotPolygonM": [[0, 0], [18, 0], [18, 10], [8, 10], [8, 22], [0, 22]] } });
        app.handle("project_set", &l_shape).unwrap();
        assert!((app.project().unwrap().plot_area_m2().unwrap() - (18.0 * 10.0 + 8.0 * 12.0)).abs() < 1e-9);
        let skipped = app.handle("sbc_check", &json!({})).unwrap()["skipped"].to_string();
        assert!(skipped.contains("not a rectangle"), "{skipped}");
    }

    #[test]
    fn a_dxf_plot_boundary_is_imported_through_the_backend() {
        let mut app = App::default();
        let dir = std::env::temp_dir().join(format!("nnx_plot_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("plot.dxf");
        std::fs::write(&f, "0
SECTION
2
HEADER
9
$INSUNITS
70
6
0
ENDSEC
0
SECTION
2
ENTITIES
0
LWPOLYLINE
8
P
90
4
70
1
10
0
20
0
10
18
20
0
10
18
20
22
10
0
20
22
0
ENDSEC
0
EOF
").unwrap();
        let r = app.handle("plot_import_dxf", &json!({ "path": f.to_str().unwrap() })).unwrap();
        assert_eq!(r["result"]["selected"], 0);
        assert!((r["result"]["candidates"][0]["area_m2"].as_f64().unwrap() - 396.0).abs() < 1e-9);
        assert!(app.handle("plot_import_dxf", &json!({})).unwrap_err().contains("path"));
        assert!(app.handle("plot_import_dxf", &json!({ "path": dir.join("none.dxf").to_str().unwrap() })).unwrap_err().contains("cannot read"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn privacy_check_flags_close_neighbour_walls_and_names_bad_input() {
        let mut app = App::default();
        let rooms = json!([
            { "key": "bed1", "func": "bedroom", "zone": "private", "x": 0, "y": 0, "w": 6000, "h": 10000 },
            { "key": "hall", "func": "hall", "zone": "public", "x": 6000, "y": 0, "w": 6000, "h": 10000 }
        ]);
        let base = json!({ "rooms": rooms, "envelope": { "w": 12000, "h": 10000 }, "neighbours": [{ "side": "west", "distance_mm": 1500 }], "private_funcs": ["bedroom"], "min_distance_mm": 3000 });
        assert_eq!(app.handle("privacy_check", &base).unwrap()["privacy"]["status"], "exposed");
        let mut skipped = base.clone();
        skipped.as_object_mut().unwrap().remove("min_distance_mm");
        assert_eq!(app.handle("privacy_check", &skipped).unwrap()["privacy"]["status"], "unchecked");
        let mut no_funcs = base.clone();
        no_funcs.as_object_mut().unwrap().remove("private_funcs");
        assert!(app.handle("privacy_check", &no_funcs).unwrap_err().contains("private_funcs"));
        let mut bad_side = base.clone();
        bad_side["neighbours"] = json!([{ "side": "up", "distance_mm": 1 }]);
        assert!(app.handle("privacy_check", &bad_side).unwrap_err().contains("neighbour side"));
        assert!(app.handle("privacy_check", &json!({ "rooms": rooms, "envelope": { "w": 1, "h": 1 } })).unwrap_err().contains("neighbours are required"));
    }

    #[test]
    fn span_compare_ranks_alternatives_and_names_bad_input() {
        let mut app = App::default();
        let wide = json!([{ "key": "hall", "func": "hall", "zone": "public", "x": 0, "y": 0, "w": 13000, "h": 5000 }]);
        let split = json!([
            { "key": "a", "func": "hall", "zone": "public", "x": 0, "y": 0, "w": 6000, "h": 5000 },
            { "key": "b", "func": "hall", "zone": "public", "x": 6000, "y": 0, "w": 6000, "h": 5000 }
        ]);
        let base = json!({ "envelope": { "w": 13000, "h": 5000 }, "max_span_mm": 6000, "alternatives": [{ "label": "A", "rooms": wide }, { "label": "B", "rooms": split }] });
        let r = app.handle("span_compare", &base).unwrap();
        assert_eq!(r["span"]["best"], 1);
        assert_eq!(r["span"]["support_lines_saved_vs_first"], 2);
        let mut no_limit = base.clone();
        no_limit.as_object_mut().unwrap().remove("max_span_mm");
        assert!(app.handle("span_compare", &no_limit).unwrap_err().contains("max_span_mm is required"));
        assert!(app.handle("span_compare", &json!({ "envelope": { "w": 1, "h": 1 }, "max_span_mm": 6000 })).unwrap_err().contains("alternatives are required"));
    }

    #[test]
    fn drift_check_screens_beams_storeys_and_footings_and_names_missing_input() {
        let mut app = App::default();
        let args = json!({
            "e_mpa": 25000.0,
            "beams": [{ "key": "b1", "span_mm": 5000, "b_mm": 300, "h_mm": 500, "support": "simple", "w_kn_m": 10.0, "limit_span_ratio": 360 }],
            "footings": [{ "key": "F1", "load_kn": 400, "width_mm": 2000, "length_mm": 2000, "es_mpa": 10, "nu": 0.3, "influence": 0.82 }]
        });
        let r = app.handle("drift_check", &args).unwrap();
        assert_eq!(r["drift"]["beams"][0]["immediate_mm"], 1.042);
        assert_eq!(r["drift"]["beams"][0]["status"], "ok");
        assert_eq!(r["drift"]["settlements"][0]["settlement_mm"], 14.924);
        assert_eq!(r["drift"]["settlements"][0]["status"], "unchecked");
        let mut no_e = args.clone();
        no_e.as_object_mut().unwrap().remove("e_mpa");
        assert!(app.handle("drift_check", &no_e).unwrap_err().contains("e_mpa is required"));
        assert!(app.handle("drift_check", &json!({})).unwrap_err().contains("nothing to check"));
        assert!(app.handle("drift_check", &json!({ "beams": 3 })).unwrap_err().contains("malformed"));
    }

    #[test]
    fn panel_schedule_balances_phases_and_refuses_missing_inputs() {
        let mut app = App::default();
        let args = json!({
            "voltage_v": 230, "power_factor": 1, "three_phase": true, "breaker_margin": 1.25,
            "breaker_sizes_a": [16, 32, 63], "demand_factors": { "lighting": 1.0 },
            "boards": [{ "key": "DB1", "ways": 12 }],
            "loads": [
                { "id": "L1", "board": "DB1", "kind": "lighting", "watts": 2300, "phases": 1 },
                { "id": "L2", "board": "DB1", "kind": "lighting", "watts": 2300, "phases": 1 },
                { "id": "L3", "board": "DB1", "kind": "lighting", "watts": 2300, "phases": 1 }
            ]
        });
        let r = app.handle("panel_schedule", &args).unwrap();
        assert_eq!(r["panels"]["boards"][0]["imbalance_pct"], 0.0);
        assert_eq!(r["panels"]["boards"][0]["main_breaker_a"], 16.0);
        assert_eq!(r["panels"]["boards"][0]["status"], "unchecked");
        let mut no_v = args.clone();
        no_v.as_object_mut().unwrap().remove("voltage_v");
        assert!(app.handle("panel_schedule", &no_v).unwrap_err().contains("voltage_v is required"));
        assert!(app.handle("panel_schedule", &json!({ "boards": 3 })).unwrap_err().contains("malformed"));
    }

    #[test]
    fn water_storage_check_sizes_tanks_and_refuses_missing_inputs() {
        let mut app = App::default();
        let args = json!({
            "persons": 100, "litres_per_person_day": 200, "demand_source": "test data",
            "ground_storage_days": 1, "roof_storage_days": 0.5, "usable_fraction": 0.8,
            "transfer": {
                "fill_hours": 2, "ground_low_level_m": 0, "roof_high_level_m": 20,
                "main": { "length_m": 30, "diameter_mm": 50, "hazen_c": 130, "minor_k": 0 },
                "pump_efficiency": 0.5, "motor_efficiency": 0.8, "motor_margin": 1.25, "motor_sizes_kw": [0.37, 1.5]
            }
        });
        let r = app.handle("water_storage_check", &args).unwrap();
        assert_eq!(r["water"]["ground"]["gross_m3"], 25.0);
        assert_eq!(r["water"]["transfer"]["motor_kw"], 1.5);
        assert_eq!(r["water"]["gravity_status"], "unchecked");
        let mut no_p = args.clone();
        no_p.as_object_mut().unwrap().remove("persons");
        assert!(app.handle("water_storage_check", &no_p).unwrap_err().contains("persons is required"));
        assert!(app.handle("water_storage_check", &json!({ "persons": "x" })).unwrap_err().contains("malformed"));
    }

    #[test]
    fn insulation_options_compare_against_the_stored_base_assembly() {
        let mut app = started();
        let args = json!({
            "scope": "wall", "degree_hours": 50000, "degree_hours_source": "test data", "cop": 3, "tariff_per_kwh": 0.2, "study_years": 10,
            "options": [{ "name": "pir", "lambda": 0.025, "thicknesses_mm": [30, 60], "price_per_m3": 400, "install_per_m2": 8, "source": "test data" }]
        });
        assert!(app.handle("insulation_options", &args).unwrap_err().contains("enter the insulation assemblies"));
        app.handle("mep", &json!({ "inputs": mep_inputs(), "apply": false, "show": false })).unwrap();
        let r = app.handle("insulation_options", &args).unwrap();
        let rep = &r["insulation_options"];
        assert_eq!(rep["scope"], "wall");
        assert_eq!(rep["rows"].as_array().unwrap().len(), 2);
        assert!(rep["rows"][1]["u_value"].as_f64().unwrap() < rep["rows"][0]["u_value"].as_f64().unwrap());
        assert!(rep["area_m2"].as_f64().unwrap() > 0.0);
        let mut no_cop = args.clone();
        no_cop.as_object_mut().unwrap().remove("cop");
        assert!(app.handle("insulation_options", &no_cop).unwrap_err().contains("cop is required"));
        assert!(app.handle("insulation_options", &json!({ "cop": "x" })).unwrap_err().contains("malformed"));
    }

    #[test]
    fn electricity_bill_estimates_from_stated_loads_and_tariff() {
        let mut app = App::default();
        let args = json!({
            "loads": [{ "name": "ac", "kw": 2, "quantity": 2, "demand_factor": 0.5, "hours_per_day": 10, "active_months": [6, 7], "source": "test data" }],
            "tariff": { "blocks": [{ "up_to_kwh": 300, "rate_per_kwh": 0.1 }, { "rate_per_kwh": 0.2 }],
                        "fixed_per_month": 5, "vat_pct": 0, "source": "test tariff", "source_date": "2026-01-01" }
        });
        let r = app.handle("electricity_bill", &args).unwrap();
        let rep = &r["electricity_bill"];
        // June: 2 kW x 2 x 0.5 x 10 h x 30 d = 600 kWh -> 30 + 60 = 90 + 5.
        assert_eq!(rep["months"][5]["total"].as_f64().unwrap(), 95.0);
        let mut no_src = args.clone();
        no_src["tariff"].as_object_mut().unwrap().remove("source");
        assert!(app.handle("electricity_bill", &no_src).unwrap_err().contains("tariff.source is required"));
        assert!(app.handle("electricity_bill", &json!({ "loads": "x" })).unwrap_err().contains("malformed"));
    }

    #[test]
    fn energy_certificate_rates_from_entered_scale_and_refuses_missing_inputs() {
        let mut app = App::default();
        let args = json!({
            "end_uses": [{ "name": "cooling", "kwh_per_year": 6000, "source": "test data" }, { "name": "lighting", "kwh_per_year": 2000, "source": "test data" }],
            "conditioned_area_m2": 100, "area_source": "model",
            "bands": [{ "label": "A", "up_to_kwh_m2": 50 }, { "label": "B", "up_to_kwh_m2": 100 }, { "label": "C" }],
            "scheme_source": "test scheme", "scheme_date": "2026-01-01"
        });
        let r = app.handle("energy_certificate", &args).unwrap();
        let rep = &r["energy_certificate"];
        assert_eq!(rep["band"], "B");
        assert_eq!(rep["next_better"]["reduction_kwh_per_year"].as_f64().unwrap(), 3000.0);
        let mut no_src = args.clone();
        no_src.as_object_mut().unwrap().remove("scheme_source");
        assert!(app.handle("energy_certificate", &no_src).unwrap_err().contains("scheme_source is required"));
        assert!(app.handle("energy_certificate", &json!({ "end_uses": "x" })).unwrap_err().contains("malformed"));
    }

    #[test]
    fn fire_protection_check_lays_out_devices_and_refuses_missing_inputs() {
        let mut app = App::default();
        let args = json!({
            "rooms": [{ "key": "hall", "x_mm": 0, "y_mm": 0, "w_mm": 6000, "h_mm": 5000, "detector": "D1", "sprinkler": "S1", "zone": "Z1" }],
            "detectors": [{ "key": "D1", "coverage_m2": 20, "max_spacing_mm": 5000, "source": "test datasheet" }],
            "sprinklers": [{ "key": "S1", "coverage_m2": 12, "max_spacing_mm": 4000, "source": "test datasheet" }],
            "exits": [{ "key": "E1", "x_mm": 0, "y_mm": 0 }],
            "water": { "design_density_mm_min": 5, "design_area_m2": 30, "duration_min": 60, "source": "test table" }
        });
        let r = app.handle("fire_protection_check", &args).unwrap();
        assert_eq!(r["fire"]["total_detectors"], 2);
        assert_eq!(r["fire"]["water"]["required_storage_m3"], 9.0);
        assert_eq!(r["fire"]["water"]["status"], "unchecked");
        assert_eq!(r["fire"]["call_points"]["status"], "unchecked");
        let mut no_cov = args.clone();
        no_cov["detectors"][0].as_object_mut().unwrap().remove("coverage_m2");
        assert!(app.handle("fire_protection_check", &no_cov).unwrap_err().contains("coverage_m2 is required"));
        assert!(app.handle("fire_protection_check", &json!({ "rooms": "x" })).unwrap_err().contains("malformed"));
    }

    #[test]
    fn egress_check_measures_travel_and_widths_and_refuses_missing_source() {
        let mut app = App::default();
        let args = json!({
            "rooms": [{ "key": "hall", "x_mm": 0, "y_mm": 0, "w_mm": 6000, "h_mm": 8000 }],
            "exits": [{ "key": "E1", "x_mm": 0, "y_mm": 0, "clear_width_mm": 800 }],
            "corridors": [{ "key": "C1", "x_mm": 0, "y_mm": 0, "w_mm": 20000, "h_mm": 1200 }],
            "max_travel_distance_mm": 9000,
            "min_corridor_width_mm": 1100,
            "min_exit_width_mm": 900,
            "source": "test clause"
        });
        let r = app.handle("egress_check", &args).unwrap();
        assert_eq!(r["egress"]["overall"], "fails");
        let checks = r["egress"]["checks"].as_array().unwrap();
        let by = |rule: &str| checks.iter().find(|c| c["rule"] == rule).unwrap().clone();
        assert_eq!(by("travel_distance")["status"], "fails");
        assert_eq!(by("travel_distance")["value"].as_f64().unwrap().round(), 10000.0);
        assert_eq!(by("corridor_width")["status"], "passes");
        assert_eq!(by("exit_width")["status"], "fails");
        let mut no_src = args.clone();
        no_src.as_object_mut().unwrap().remove("source");
        assert!(app.handle("egress_check", &no_src).unwrap_err().contains("source is required"));
        assert!(app.handle("egress_check", &json!({ "rooms": "x" })).unwrap_err().contains("malformed"));
    }

    #[test]
    fn access_check_measures_doors_and_ramps_and_refuses_missing_source() {
        let mut app = App::default();
        let args = json!({
            "doors": [{ "key": "D1", "clear_width_mm": 750, "threshold_height_mm": 0 }],
            "rooms": [{ "key": "WC", "w_mm": 1600, "h_mm": 2000 }],
            "ramps": [{ "key": "R1", "rise_mm": 150, "run_mm": 1800 }],
            "min_door_clear_width_mm": 800,
            "min_turning_diameter_mm": 1500,
            "max_ramp_slope": 0.0834,
            "min_ramp_width_mm": 1000,
            "source": "test clause"
        });
        let r = app.handle("access_check", &args).unwrap();
        assert_eq!(r["access"]["overall"], "fails");
        let checks = r["access"]["checks"].as_array().unwrap();
        let by = |rule: &str| checks.iter().find(|c| c["rule"] == rule).unwrap().clone();
        assert_eq!(by("door_width")["status"], "fails");
        assert_eq!(by("threshold")["status"], "unchecked");
        assert_eq!(by("turning_space")["status"], "passes");
        assert_eq!(by("ramp_slope")["status"], "passes");
        assert_eq!(by("ramp_width")["status"], "unchecked");
        let mut no_src = args.clone();
        no_src.as_object_mut().unwrap().remove("source");
        assert!(app.handle("access_check", &no_src).unwrap_err().contains("source is required"));
        assert!(app.handle("access_check", &json!({ "doors": "x" })).unwrap_err().contains("malformed"));
    }

    #[test]
    fn cable_check_sizes_by_drop_and_refuses_missing_inputs() {
        let mut app = App::default();
        let args = json!({
            "voltage_v": 230, "power_factor": 1, "three_phase": false, "derating_factor": 1, "allowance_mm": 0,
            "catalogue": [
                { "key": "c4", "area_mm2": 4, "r_ohm_km": 5, "x_ohm_km": 0, "ampacity_a": 30, "source": "test data" },
                { "key": "c10", "area_mm2": 10, "r_ohm_km": 2, "x_ohm_km": 0, "ampacity_a": 60, "source": "test data" }
            ],
            "circuits": [{ "id": "C1", "from": { "x_mm": 0, "y_mm": 0 }, "to": { "x_mm": 50000, "y_mm": 0 }, "current_a": 25, "phases": 1 }],
            "max_drop_pct": 3
        });
        let r = app.handle("cable_check", &args).unwrap();
        assert_eq!(r["cables"]["circuits"][0]["cable"], "c10");
        assert_eq!(r["cables"]["circuits"][0]["status"], "ok");
        let mut no_v = args.clone();
        no_v.as_object_mut().unwrap().remove("voltage_v");
        assert!(app.handle("cable_check", &no_v).unwrap_err().contains("voltage_v is required"));
        assert!(app.handle("cable_check", &json!({ "circuits": 3 })).unwrap_err().contains("malformed"));
    }

    #[test]
    fn lighting_plan_counts_luminaires_and_refuses_missing_targets() {
        let mut app = App::default();
        let args = json!({
            "rooms": [{ "key": "r1", "x_mm": 0, "y_mm": 0, "w_mm": 5000, "h_mm": 4000, "target_lux": 300, "target_source": "client brief", "luminaire": "L1" }],
            "luminaires": [{ "key": "L1", "lumens": 4000, "utilisation": 0.5, "maintenance": 0.8 }]
        });
        let r = app.handle("lighting_plan", &args).unwrap();
        assert_eq!(r["lighting"]["rooms"][0]["count"], 4);
        assert_eq!(r["lighting"]["rooms"][0]["status"], "unchecked");
        let mut no_target = args.clone();
        no_target["rooms"][0].as_object_mut().unwrap().remove("target_lux");
        assert!(app.handle("lighting_plan", &no_target).unwrap_err().contains("target_lux is required"));
        assert!(app.handle("lighting_plan", &json!({ "rooms": 3 })).unwrap_err().contains("malformed"));
    }

    #[test]
    fn spalling_check_screens_elements_and_never_passes_without_limits() {
        let mut app = App::default();
        let args = json!({
            "elements": [
                { "key": "c1", "kind": "column", "b_mm": 400, "h_mm": 400, "cover_mm": 40, "fc_mpa": 90, "exposed_sides": 4, "moisture_pct": 2.0, "pp_fibre_kg_m3": 2.0 },
                { "key": "c2", "kind": "column", "b_mm": 400, "h_mm": 400, "cover_mm": 40, "fc_mpa": 90, "exposed_sides": 4, "moisture_pct": 2.0 }
            ],
            "max_moisture_pct": 3.0, "max_fc_mpa": 60.0, "max_cover_mm": 70.0, "min_pp_fibre_kg_m3": 2.0
        });
        let r = app.handle("spalling_check", &args).unwrap();
        assert_eq!(r["spalling"]["elements"][0]["status"], "mitigated");
        assert_eq!(r["spalling"]["elements"][1]["status"], "fails");
        let no_limits = json!({ "elements": [args["elements"][0].clone()] });
        let r = app.handle("spalling_check", &no_limits).unwrap();
        assert_eq!(r["spalling"]["elements"][0]["status"], "unchecked");
        assert!(app.handle("spalling_check", &json!({ "elements": [] })).unwrap_err().contains("nothing to check"));
        assert!(app.handle("spalling_check", &json!({ "elements": 3 })).unwrap_err().contains("malformed"));
    }

    #[test]
    fn deep_foundation_check_screens_mat_and_piles_and_names_missing_input() {
        let mut app = App::default();
        let args = json!({
            "mat": { "x_mm": 0, "y_mm": 0, "w_mm": 10000, "l_mm": 10000, "allowable_kpa": 150.0 },
            "columns": [
                { "key": "a", "x_mm": 2000, "y_mm": 2000, "load_kn": 1000 },
                { "key": "b", "x_mm": 8000, "y_mm": 2000, "load_kn": 1000 },
                { "key": "c", "x_mm": 2000, "y_mm": 8000, "load_kn": 1000 },
                { "key": "d", "x_mm": 8000, "y_mm": 8000, "load_kn": 1000 }
            ],
            "piles": [{ "key": "C1", "load_kn": 1600, "capacity_kn": 500, "positions": [[-750, -750], [750, -750], [-750, 750], [750, 750]] }]
        });
        let r = app.handle("deep_foundation_check", &args).unwrap();
        assert_eq!(r["deep_foundation"]["mat"]["max_kpa"], 40.0);
        assert_eq!(r["deep_foundation"]["mat"]["status"], "ok");
        assert_eq!(r["deep_foundation"]["piles"][0]["max_kn"], 400.0);
        assert_eq!(r["deep_foundation"]["piles"][0]["spacing_status"], "unchecked");
        let mut no_spacing = args.clone();
        no_spacing["piles"][0].as_object_mut().unwrap().remove("positions");
        assert!(app.handle("deep_foundation_check", &no_spacing).unwrap_err().contains("spacing_mm is required"));
        assert!(app.handle("deep_foundation_check", &json!({})).unwrap_err().contains("nothing to check"));
        assert!(app.handle("deep_foundation_check", &json!({ "piles": 3 })).unwrap_err().contains("malformed"));
    }

    #[test]
    fn shear_core_check_reports_enclosure_section_and_lateral_and_names_missing_input() {
        let mut app = App::default();
        let args = json!({
            "walls": [
                { "key": "S", "x_mm": 0, "y_mm": 0, "w_mm": 3000, "h_mm": 200 },
                { "key": "N", "x_mm": 0, "y_mm": 2800, "w_mm": 3000, "h_mm": 200 },
                { "key": "W", "x_mm": 0, "y_mm": 200, "w_mm": 200, "h_mm": 2600 },
                { "key": "E", "x_mm": 2800, "y_mm": 200, "w_mm": 200, "h_mm": 2600 }
            ],
            "shafts": [{ "key": "L1", "kind": "lift", "x_mm": 200, "y_mm": 200, "w_mm": 2600, "h_mm": 2600 }],
            "e_mpa": 25000.0,
            "levels": [{ "height_mm": 12000, "fx_kn": 100.0 }],
            "max_shear_mpa": 1.0
        });
        let r = app.handle("shear_core_check", &args).unwrap();
        assert_eq!(r["shear_core"]["shafts"][0]["enclosure"], "enclosed");
        assert_eq!(r["shear_core"]["section"]["area_mm2"], 2_240_000.0);
        assert_eq!(r["shear_core"]["lateral"]["base_moment_about_y_knm"], 1200.0);
        assert_eq!(r["shear_core"]["lateral"]["shear_status"], "ok");
        assert_eq!(r["shear_core"]["lateral"]["top_status"], "unchecked");
        let mut no_e = args.clone();
        no_e.as_object_mut().unwrap().remove("e_mpa");
        assert!(app.handle("shear_core_check", &no_e).unwrap_err().contains("e_mpa is required"));
        assert!(app.handle("shear_core_check", &json!({ "walls": [] })).unwrap_err().contains("no walls"));
        assert!(app.handle("shear_core_check", &json!({ "walls": 3 })).unwrap_err().contains("malformed"));
    }

    #[test]
    fn snapshot_report_builds_html_and_csv_between_two_versions() {
        let mut app = started_with_project();
        app.handle("snapshot_save", &json!({ "name": "Rev A" })).unwrap();
        let same = app.handle("snapshot_report", &json!({ "a": "S1", "lang": "en" })).unwrap();
        assert_eq!(same["has_changes"], false);
        assert!(same["html"].as_str().unwrap().contains("No difference between the two versions"));
        let ar = app.handle("snapshot_report", &json!({ "a": "S1" })).unwrap();
        assert!(ar["html"].as_str().unwrap().contains("dir=\"rtl\""));
        assert!(ar["csv"].as_str().unwrap().starts_with("section,id,"));
        assert_eq!(ar["name"], "change_report.pdf");
        assert!(app.handle("snapshot_report", &json!({})).is_err());
        assert!(app.handle("snapshot_report", &json!({ "a": "S9" })).is_err());
        assert!(App::default().handle("snapshot_report", &json!({ "a": "S1" })).is_err());
    }

    #[test]
    fn number_audit_lists_every_number_with_its_source_or_flags_it() {
        let mut app = started_with_project();
        let r = app.handle("number_audit", &json!({})).unwrap();
        assert_eq!(r["name"], "number_audit.csv");
        assert_eq!(r["total"].as_u64().unwrap(), r["sourced"].as_u64().unwrap() + r["unsourced"].as_u64().unwrap());
        assert_eq!(r["entries"].as_array().unwrap().len() as u64, r["total"].as_u64().unwrap());
        assert_eq!(r["unsourced_paths"].as_array().unwrap().len() as u64, r["unsourced"].as_u64().unwrap());
        assert!(r["csv"].as_str().unwrap().starts_with("path,value,status,source,date
"));
        assert!(App::default().handle("number_audit", &json!({})).is_err());
    }

    #[test]
    fn permit_pack_numbers_sheets_stamps_the_engineer_and_names_missing_inputs() {
        let mut app = started_with_project();
        let args = json!({
            "engineer": { "name": "Eng <A>", "licence_no": "L-123", "licensing_body": "Test council", "discipline": "Architectural" },
            "project_no": "P-7", "sheet_prefix": "A", "revision": "Rev B", "issued_for": "Review", "lang": "en"
        });
        let r = app.handle("permit_pack", &args).unwrap();
        let h = r["html"].as_str().unwrap();
        assert_eq!(r["name"], "permit_pack.pdf");
        let sheets = r["sheets"].as_array().unwrap();
        assert_eq!(sheets[0]["number"], "A-01");
        assert_eq!(sheets[0]["kind"], "index");
        assert!(sheets.len() >= 2 && sheets[1]["number"] == "A-02" && sheets[1]["kind"] == "plan");
        assert_eq!(h.matches("<section class=\"sheet\">").count(), sheets.len());
        assert!(h.contains("Eng &lt;A&gt;") && !h.contains("Eng <A>") && h.contains("L-123") && h.contains("Test council") && h.contains("Rev B") && h.contains("P-7"));
        assert!(h.contains("Not valid until stamped and signed") && h.contains("class=\"seal\"") && !h.to_lowercase().contains("approved by"));
        let n = sheets.len();
        assert!(h.contains(&format!("{} / {n}", n)), "page x / total");
        // Nothing is defaulted: every missing input is named at once.
        let mut bad = args.clone();
        bad.as_object_mut().unwrap().remove("project_no");
        bad["engineer"]["licence_no"] = json!("  ");
        let e = app.handle("permit_pack", &bad).unwrap_err();
        assert!(e.contains("engineer.licence_no") && e.contains("project_no") && !e.contains("revision"), "{e}");
        assert!(app.handle("permit_pack", &json!({ "engineer": "x" })).unwrap_err().contains("malformed"));
        // Office letterhead: optional, printed on every sheet, escaped; a non-embedded logo is refused by name.
        let mut lh = args.clone();
        lh["letterhead"] = json!({ "office_name": "Studio <Z>", "footer": "CR 9" });
        let h2 = app.handle("permit_pack", &lh).unwrap()["html"].as_str().unwrap().to_string();
        assert!(h2.contains("Studio &lt;Z&gt;") && h2.contains("class=\"lf\"") && !h2.contains("Studio <Z>"));
        assert!(!h.contains("class=\"lh\""));
        lh["letterhead"]["logo_data_uri"] = json!("https://example.test/x.png");
        assert!(app.handle("permit_pack", &lh).unwrap_err().contains("letterhead.logo_data_uri"));
        assert!(App::default().handle("permit_pack", &args).is_err());
        // lang "both": a matched Arabic/English pair with the same sheet numbers.
        let mut both = args.clone();
        both["lang"] = json!("both");
        let r = app.handle("permit_pack", &both).unwrap();
        assert_eq!(r["bilingual"], true);
        let (ar, en) = (r["ar"]["html"].as_str().unwrap(), r["en"]["html"].as_str().unwrap());
        assert!(ar.contains("dir=\"rtl\"") && ar.contains("غير نافذة حتى يختمها") && en.contains("dir=\"ltr\"") && en.contains("Not valid until stamped"));
        let nk = |v: &Value| v.as_array().unwrap().iter().map(|s| (s["number"].clone(), s["kind"].clone())).collect::<Vec<_>>();
        assert_eq!(nk(&r["ar"]["sheets"]), nk(&r["en"]["sheets"]), "same numbers and kinds; titles are translated");
        assert_eq!(nk(&r["sheets"]), nk(&r["en"]["sheets"]));
        assert_ne!(r["ar"]["sheets"][0]["title"], r["en"]["sheets"][0]["title"]);
        assert_eq!(r["ar"]["name"], "permit_pack_ar.pdf");
        assert_eq!(r["en"]["name"], "permit_pack_en.pdf");
        both["project_no"] = json!("");
        assert!(app.handle("permit_pack", &both).unwrap_err().contains("project_no"));
    }

    #[test]
    fn signoff_form_prints_user_statement_leaves_boxes_empty_and_names_missing_inputs() {
        let mut app = started_with_project();
        let args = json!({
            "engineer": { "name": "Eng <A>", "licence_no": "L-123", "licensing_body": "Test council" },
            "project_no": "P-7", "revision": "Rev B", "scope": ["Architectural drawings A-01..A-03", " "],
            "statement": "I take responsibility <as worded>.", "statement_source": "Council form 9, 2026-01-01", "lang": "en"
        });
        let r = app.handle("signoff_form", &args).unwrap();
        let h = r["html"].as_str().unwrap();
        assert_eq!(r["name"], "engineer_signoff.pdf");
        assert!(h.contains("Eng &lt;A&gt;") && h.contains("L-123") && h.contains("Test council") && h.contains("I take responsibility &lt;as worded&gt;.") && h.contains("Council form 9"));
        assert_eq!(h.matches("<li>").count(), 1, "blank scope entries are dropped");
        assert!(h.contains("void until the licensed engineer signs") && !h.to_lowercase().contains("approved by"));
        let mut bad = args.clone();
        bad.as_object_mut().unwrap().remove("statement");
        bad["scope"] = json!([" "]);
        let e = app.handle("signoff_form", &bad).unwrap_err();
        assert!(e.contains("statement") && e.contains("scope") && !e.contains("revision"), "{e}");
        assert!(app.handle("signoff_form", &json!({ "engineer": "x" })).unwrap_err().contains("malformed"));
        assert!(App::default().handle("signoff_form", &args).is_err());
    }

    #[test]
    fn permit_documents_check_lists_missing_items_and_names_bad_input() {
        let mut app = started_with_project();
        let args = json!({
            "required": [{ "key": "deed", "name": "Title deed" }, { "key": "arch", "name": "Architectural drawings" }],
            "list_source": "Municipality form 3, 2026-01-01",
            "submitted": [{ "key": "arch", "reference": "A-01", "date": "2026-02-03" }]
        });
        let r = app.handle("permit_documents_check", &args).unwrap();
        assert_eq!(r["name"], "permit_documents.csv");
        assert_eq!((r["missing"].as_u64(), r["overall"].as_str()), (Some(1), Some("incomplete")));
        assert_eq!(r["rows"][0]["status"], "missing");
        assert_eq!(r["rows"][1]["status"], "submitted");
        let e = app.handle("permit_documents_check", &json!({ "required": [] })).unwrap_err();
        assert!(e.contains("list_source") && e.contains("required"), "{e}");
        assert!(app.handle("permit_documents_check", &json!({ "required": "x" })).unwrap_err().contains("malformed"));
        assert!(App::default().handle("permit_documents_check", &args).is_err());
    }

    #[test]
    fn room_finishes_schedule_lists_the_model_rooms_and_names_bad_input() {
        let mut app = started_with_project();
        let args = json!({
            "finishes": [{ "code": "F1", "name": "Porcelain", "surface": "floor", "source": "Spec 09, 2026-01-01" }],
            "assignments": [{ "func": "no-such-function", "floor": "F1" }]
        });
        let r = app.handle("room_finishes_schedule", &args).unwrap();
        assert_eq!(r["name"], "room_finishes.csv");
        let rows = r["rows"].as_array().unwrap();
        assert!(!rows.is_empty());
        assert_eq!(r["complete"], false);
        assert_eq!(r["unassigned"].as_array().unwrap().len(), rows.len() * 3);
        let e = app.handle("room_finishes_schedule", &json!({ "finishes": [] })).unwrap_err();
        assert!(e.contains("finishes") && e.contains("assignments"), "{e}");
        assert!(app.handle("room_finishes_schedule", &json!({ "finishes": "x" })).unwrap_err().contains("malformed"));
        assert!(App::default().handle("room_finishes_schedule", &args).is_err());
    }

    #[test]
    fn stair_detail_needs_the_users_dimensions_and_reports_unentered_limits_as_unchecked() {
        let mut app = started_with_project();
        let e = app.handle("stair_detail", &json!({})).unwrap_err();
        assert!(e.contains("rise_mm") && e.contains("riser_count"), "{e}");
        assert!(app.handle("stair_detail", &json!({ "rise_mm": "x" })).unwrap_err().contains("malformed"));
        let v = app.handle("stair_detail", &json!({ "rise_mm": 3000, "riser_count": 15, "going_mm": 250, "width_mm": 1200 })).unwrap();
        assert_eq!((v["riser_height_mm"].as_f64(), v["tread_count"].as_u64(), v["overall"].as_str()), (Some(200.0), Some(14), Some("incomplete")));
        assert!(v["svg"].as_str().unwrap().starts_with("<svg"));
        assert!(App::default().handle("stair_detail", &json!({})).is_err());
    }

    #[test]
    fn wet_room_detail_needs_the_users_fixtures_and_a_model_room() {
        let mut app = started_with_project();
        let e = app.handle("wet_room_detail", &json!({ "room": "no-such-room", "fixtures": [{ "key": "wc", "x_mm": 0, "y_mm": 0, "w_mm": 400, "d_mm": 700, "source": "Data sheet" }] })).unwrap_err();
        assert!(e.contains("no room with key"), "{e}");
        let e = app.handle("wet_room_detail", &json!({ "room": "x" })).unwrap_err();
        assert!(e.contains("fixtures"), "{e}");
        assert!(app.handle("wet_room_detail", &json!({ "fixtures": "x" })).unwrap_err().contains("malformed"));
        assert!(App::default().handle("wet_room_detail", &json!({})).is_err());
    }

    #[test]
    fn ceiling_plan_needs_the_users_heights_and_devices_and_a_model_room() {
        let mut app = started_with_project();
        let dev = json!({ "key": "L1", "x_mm": 500, "y_mm": 500, "w_mm": 300, "d_mm": 300, "source": "Data sheet" });
        let e = app.handle("ceiling_plan", &json!({ "room": "no-such-room", "ceiling_height_mm": 2700, "devices": [dev] })).unwrap_err();
        assert!(e.contains("no room with key"), "{e}");
        let e = app.handle("ceiling_plan", &json!({ "room": "x" })).unwrap_err();
        assert!(e.contains("ceiling_height_mm") && e.contains("devices"), "{e}");
        assert!(app.handle("ceiling_plan", &json!({ "devices": "x" })).unwrap_err().contains("malformed"));
        assert!(App::default().handle("ceiling_plan", &json!({})).is_err());
    }

    #[test]
    fn axes_plan_needs_the_users_axes_or_the_columns_to_read_them_from() {
        let mut app = started_with_project();
        let e = app.handle("axes_plan", &json!({})).unwrap_err();
        assert!(e.contains("axes_x"), "{e}");
        let e = app.handle("axes_plan", &json!({ "from_columns": true })).unwrap_err();
        assert!(e.contains("cluster_mm"), "{e}");
        assert!(app.handle("axes_plan", &json!({ "axes_x": "x" })).unwrap_err().contains("malformed"));
        assert!(App::default().handle("axes_plan", &json!({})).is_err());
    }

    #[test]
    fn site_fence_plan_cuts_the_users_gates_into_the_users_fences() {
        let mut app = started_with_project();
        let e = app.handle("site_fence_plan", &json!({})).unwrap_err();
        assert!(e.contains("fences"), "{e}");
        assert!(app.handle("site_fence_plan", &json!({ "fences": "x" })).unwrap_err().contains("malformed"));
        let n = app.handle("site_plan", &json!({})).unwrap()["result"]["edges"].as_array().unwrap().len();
        let fences: Vec<Value> = (0..n).map(|e| json!({ "edge": e, "kind": "solid wall", "height_m": 2.4, "thickness_m": 0.2, "source": "Brief" })).collect();
        let out = app.handle("site_fence_plan", &json!({ "fences": fences, "gates": [{ "key": "G1", "edge": 0, "kind": "vehicle", "offset_m": 1.0, "width_m": 3.0, "source": "Brief" }] })).unwrap();
        assert_eq!((out["vehicle_gates"].as_u64(), out["fences"][0]["gates"].as_u64()), (Some(1), Some(1)));
        assert!(out["svg"].as_str().unwrap().starts_with("<svg"));
        let e = app.handle("site_fence_plan", &json!({ "fences": [{ "edge": 0, "height_m": 2.4, "thickness_m": 0.2 }] })).unwrap_err();
        assert!(e.contains("source"), "{e}");
        assert!(App::default().handle("site_fence_plan", &json!({})).is_err());
    }

    #[test]
    fn wall_load_takes_walls_down_to_beams_and_names_bad_input() {
        let mut app = App::default();
        let args = json!({
            "walls": [{ "key": "w1", "a": [0, 0], "b": [5000, 0], "thickness_mm": 200, "height_mm": 3000, "unit_weight_kn_m3": 14.0 }],
            "beams": [{ "key": "b1", "a": [0, 0], "b": [5000, 0] }]
        });
        let r = app.handle("wall_load", &args).unwrap();
        assert_eq!(r["wall_load"]["beams"][0]["total_kn"], 42.0);
        assert_eq!(r["wall_load"]["beams"][0]["max_moment_knm"], 26.25);
        let mut no_g = args.clone();
        no_g["walls"][0].as_object_mut().unwrap().remove("unit_weight_kn_m3");
        assert!(app.handle("wall_load", &no_g).unwrap_err().contains("unit_weight_kn_m3 is required"));
        assert!(app.handle("wall_load", &json!({ "beams": args["beams"] })).unwrap_err().contains("walls need"));
    }

    #[test]
    fn savings_compare_ranks_alternatives_and_names_missing_input() {
        let mut app = App::default();
        let m = |b: f64| json!({ "key": "c", "kind": "column", "count": 4, "b_mm": b, "h_mm": b, "length_mm": 3000, "bars": [{ "dia_mm": 16, "count": 8 }] });
        let args = json!({ "criterion": "concrete_m3", "alternatives": [
            { "label": "A", "checks_passed": true, "members": [m(400.0)] },
            { "label": "B", "checks_passed": true, "members": [m(300.0)] } ] });
        let r = app.handle("savings_compare", &args).unwrap();
        assert_eq!(r["savings"]["best"], 1);
        assert_eq!(r["savings"]["alternatives"][1]["saving_concrete"]["pct"], 43.75);
        let mut no_c = args.clone();
        no_c.as_object_mut().unwrap().remove("criterion");
        assert!(app.handle("savings_compare", &no_c).unwrap_err().contains("criterion is required"));
        let mut cost = args.clone();
        cost["criterion"] = json!("cost");
        assert!(app.handle("savings_compare", &cost).unwrap_err().contains("needs prices"));
        assert!(app.handle("savings_compare", &json!({ "criterion": "steel_kg" })).unwrap_err().contains("alternatives need"));
    }

    #[test]
    fn sun_check_flags_west_bedrooms_and_names_bad_input() {
        let mut app = App::default();
        let rooms = json!([
            { "key": "bed1", "func": "bedroom", "zone": "private", "x": 0, "y": 0, "w": 6000, "h": 10000 },
            { "key": "hall", "func": "hall", "zone": "public", "x": 6000, "y": 0, "w": 6000, "h": 10000 }
        ]);
        let base = json!({ "rooms": rooms, "envelope": { "w": 12000, "h": 10000 }, "sleeping_funcs": ["bedroom"] });
        let r = app.handle("sun_check", &base).unwrap();
        assert_eq!(r["sun"]["status"], "west_exposed");
        assert_eq!(r["sun"]["west_rooms"][0]["room"], "bed1");
        let mut no_funcs = base.clone();
        no_funcs.as_object_mut().unwrap().remove("sleeping_funcs");
        assert!(app.handle("sun_check", &no_funcs).unwrap_err().contains("sleeping_funcs"));
    }

    #[test]
    fn courtyard_check_measures_an_l_shaped_courtyard_and_names_bad_input() {
        let mut app = App::default();
        let rooms = json!([{ "key": "a", "func": "bedroom", "zone": "private", "x": 0, "y": 0, "w": 3000, "h": 12000 }]);
        let base = json!({ "rooms": rooms, "envelope": { "w": 12000, "h": 12000 }, "outline": [
            { "x": 3000, "y": 3000 }, { "x": 9000, "y": 3000 }, { "x": 9000, "y": 6000 }, { "x": 6000, "y": 6000 }, { "x": 6000, "y": 9000 }, { "x": 3000, "y": 9000 }] });
        let r = app.handle("courtyard_check", &base).unwrap();
        assert_eq!(r["courtyard"]["shape"], "l_shape");
        assert!((r["courtyard"]["area_m2"].as_f64().unwrap() - 27.0).abs() < 1e-9);
        assert_eq!(r["courtyard"]["contacts"][0]["room"], "a");
        assert_eq!(r["courtyard"]["contacts"][0]["length_mm"], 6000);
        let mut over = base.clone();
        over["outline"] = json!([{ "x": 1000, "y": 1000 }, { "x": 5000, "y": 1000 }, { "x": 5000, "y": 5000 }]);
        assert!(app.handle("courtyard_check", &over).unwrap_err().contains("covers part of the room"));
        let mut none = base.clone();
        none.as_object_mut().unwrap().remove("outline");
        assert!(app.handle("courtyard_check", &none).unwrap_err().contains("outline"));
    }

    #[test]
    fn room_shape_check_measures_an_l_room_and_names_bad_input() {
        let mut app = App::default();
        let rooms = json!([{ "key": "living", "func": "living", "zone": "public", "x": 0, "y": 0, "w": 8000, "h": 8000 }]);
        let base = json!({ "rooms": rooms, "envelope": { "w": 12000, "h": 12000 }, "key": "living", "min_wall_mm": 3000, "outline": [
            { "x": 0, "y": 0 }, { "x": 8000, "y": 0 }, { "x": 8000, "y": 4000 }, { "x": 4000, "y": 4000 }, { "x": 4000, "y": 8000 }, { "x": 0, "y": 8000 }] });
        let r = app.handle("room_shape_check", &base).unwrap();
        assert_eq!(r["room_shape"]["shape"], "l_shape");
        assert!((r["room_shape"]["area_m2"].as_f64().unwrap() - 48.0).abs() < 1e-9);
        assert_eq!(r["room_shape"]["length_check"], "pass");
        assert_eq!(r["room_shape"]["walls"][0]["faces"], "south");
        let mut none = base.clone();
        none.as_object_mut().unwrap().remove("min_wall_mm");
        assert_eq!(app.handle("room_shape_check", &none).unwrap()["room_shape"]["length_check"], "unchecked");
        let mut bad = base.clone();
        bad["min_wall_mm"] = json!("x");
        assert!(app.handle("room_shape_check", &bad).unwrap_err().contains("min_wall_mm"));
        let mut nokey = base.clone();
        nokey.as_object_mut().unwrap().remove("key");
        assert!(app.handle("room_shape_check", &nokey).unwrap_err().contains("key"));
        let mut slanted = base.clone();
        slanted["outline"] = json!([{ "x": 0, "y": 0 }, { "x": 4000, "y": 0 }, { "x": 2000, "y": 3000 }]);
        assert!(app.handle("room_shape_check", &slanted).unwrap_err().contains("slanted"));
    }

    #[test]
    fn mezzanine_check_measures_a_partial_floor_and_names_bad_input() {
        let mut app = App::default();
        let rooms = json!([{ "key": "shop", "func": "living", "zone": "public", "x": 0, "y": 0, "w": 10000, "h": 10000 }]);
        let base = json!({ "rooms": rooms, "envelope": { "w": 10000, "h": 10000 }, "storey_height_mm": 5400, "level_mm": 2800, "slab_mm": 300,
            "min_clear_below_mm": 2300, "outline": [{ "x": 0, "y": 5000 }, { "x": 10000, "y": 5000 }, { "x": 10000, "y": 10000 }, { "x": 0, "y": 10000 }] });
        let r = app.handle("mezzanine_check", &base).unwrap();
        assert!((r["mezzanine"]["area_m2"].as_f64().unwrap() - 50.0).abs() < 1e-9);
        assert_eq!(r["mezzanine"]["clear_below_mm"], 2500);
        assert_eq!(r["mezzanine"]["clear_below_check"], "pass");
        assert_eq!(r["mezzanine"]["clear_above_check"], "unchecked");
        let mut missing = base.clone();
        missing.as_object_mut().unwrap().remove("level_mm");
        assert!(app.handle("mezzanine_check", &missing).unwrap_err().contains("level_mm"));
        let mut bad = base.clone();
        bad["max_area_share"] = json!("x");
        assert!(app.handle("mezzanine_check", &bad).unwrap_err().contains("limits must be numbers"));
        let mut deep = base.clone();
        deep["slab_mm"] = json!(3000);
        assert!(app.handle("mezzanine_check", &deep).unwrap_err().contains("thinner"));
    }

    #[test]
    fn furniture_fit_arranges_pieces_and_names_bad_input() {
        let mut app = App::default();
        let base = json!({ "room": { "w": 4000, "h": 3500 }, "min_walkway_mm": 800,
            "pieces": [{ "key": "bed", "w_mm": 1600, "d_mm": 2000 }, { "key": "chair", "w_mm": 500, "d_mm": 500, "count": 2 }],
            "doors": [{ "wall": "south", "offset_mm": 500, "width_mm": 900 }], "door_clear_mm": 900 });
        let r = app.handle("furniture_fit", &base).unwrap();
        assert_eq!(r["furniture_fit"]["status"], "fits");
        assert_eq!(r["furniture_fit"]["placements"].as_array().unwrap().len(), 3);
        assert_eq!(r["furniture_fit"]["access_check"], "pass");
        assert_eq!(r["furniture_fit"]["occupancy_check"], "unchecked");
        let mut no_walk = base.clone();
        no_walk.as_object_mut().unwrap().remove("min_walkway_mm");
        assert!(app.handle("furniture_fit", &no_walk).unwrap_err().contains("min_walkway_mm"));
        let mut bad = base.clone();
        bad["pieces"][0]["w_mm"] = json!("wide");
        assert!(app.handle("furniture_fit", &bad).unwrap_err().contains("whole millimetres"));
        let mut small = base.clone();
        small["room"] = json!({ "w": 1500, "h": 1500 });
        small["doors"] = json!([]);
        let r = app.handle("furniture_fit", &small).unwrap();
        assert_eq!(r["furniture_fit"]["status"], "no_arrangement_found");
    }

    #[test]
    fn expansion_is_saved_in_the_project_reported_and_undone() {
        let mut app = started_with_project();
        assert!(app.handle("expansion_check", &json!({})).unwrap_err().contains("no expansion is saved"));
        let bad = json!({ "meta": { "expansion": { "vertical": { "extraStoreys": 11, "storeyHeightMm": 3200.0 } } } });
        assert!(app.handle("project_set", &bad).unwrap_err().contains("more than 10"));
        let ok = json!({ "meta": { "expansion": { "vertical": { "extraStoreys": 1, "storeyHeightMm": 3200.0, "maxStoreys": 4, "limitsSource": "municipality letter" }, "note": "phase 2" } } });
        assert_eq!(app.handle("project_set", &ok).unwrap()["changed"], json!(true));
        let r = app.handle("expansion_check", &json!({})).unwrap();
        let v = &r["expansion"]["vertical"];
        assert_eq!(v["extra_storeys"], json!(1));
        assert_eq!(v["storeys_check"], json!("pass"));
        assert_eq!(v["height_check"], json!("unchecked"));
        assert_eq!(v["limits_source"], json!("municipality letter"));
        app.handle("undo", &json!({})).unwrap();
        assert!(app.handle("expansion_check", &json!({})).unwrap_err().contains("no expansion is saved"));
    }

    #[test]
    fn rooftop_check_measures_the_roof_and_names_bad_input() {
        let mut app = App::default();
        let base = json!({ "envelope": { "w": 10000, "h": 10000 }, "min_edge_setback_mm": 500,
            "items": [{ "key": "stair", "kind": "room", "x": 1000, "y": 1000, "w": 3000, "h": 4000, "height_mm": 2600 },
                      { "key": "t1", "kind": "tank", "x": 6000, "y": 6000, "w": 2000, "h": 2000, "height_mm": 1500, "water_depth_mm": 1200 }] });
        let r = app.handle("rooftop_check", &base).unwrap();
        assert!((r["rooftop"]["free_m2"].as_f64().unwrap() - 84.0).abs() < 1e-9);
        assert!((r["rooftop"]["tank_water_kg"].as_f64().unwrap() - 4800.0).abs() < 1e-6);
        assert_eq!(r["rooftop"]["edge_check"], "pass");
        assert_eq!(r["rooftop"]["gap_check"], "unchecked");
        let mut no_env = base.clone();
        no_env.as_object_mut().unwrap().remove("envelope");
        assert!(app.handle("rooftop_check", &no_env).unwrap_err().contains("envelope is required"));
        let mut bad = base.clone();
        bad["items"][0]["kind"] = json!("garden");
        assert!(app.handle("rooftop_check", &bad).unwrap_err().contains("kind (room, canopy or tank)"));
        let mut dry = base.clone();
        dry["items"][1].as_object_mut().unwrap().remove("water_depth_mm");
        assert!(app.handle("rooftop_check", &dry).unwrap_err().contains("needs water_depth_mm"));
    }

    #[test]
    fn basement_check_measures_the_level_and_names_bad_input() {
        let mut app = App::default();
        let base = json!({ "envelope": { "w": 20000, "h": 30000 }, "outline": { "x": 2000, "y": 2000, "w": 16000, "h": 26000 },
            "storey_mm": 3500, "slab_mm": 300, "depth_mm": 3000, "max_ramp_slope_pct": 20.0,
            "spaces": [{ "key": "bays", "kind": "parking", "x": 2000, "y": 2000, "w": 16000, "h": 10000, "stalls": 12 },
                       { "key": "ramp", "kind": "ramp", "x": 2000, "y": 13000, "w": 4000, "h": 15000, "run_along": "y" }] });
        let r = app.handle("basement_check", &base).unwrap();
        assert!((r["basement"]["excavation_m3"].as_f64().unwrap() - 1248.0).abs() < 1e-9);
        assert_eq!(r["basement"]["parking_stalls"], 12);
        assert_eq!(r["basement"]["ramp_slope_check"], "pass");
        assert_eq!(r["basement"]["groundwater_check"], "unchecked");
        let mut no_env = base.clone();
        no_env.as_object_mut().unwrap().remove("envelope");
        assert!(app.handle("basement_check", &no_env).unwrap_err().contains("envelope is required"));
        let mut bad = base.clone();
        bad["spaces"][0]["kind"] = json!("garage");
        assert!(app.handle("basement_check", &bad).unwrap_err().contains("kind (parking, storage, service, ramp or core)"));
        let mut dry = base.clone();
        dry["spaces"][1].as_object_mut().unwrap().remove("run_along");
        assert!(app.handle("basement_check", &dry).unwrap_err().contains("needs run_along"));
        let mut no_depth = base.clone();
        no_depth.as_object_mut().unwrap().remove("depth_mm");
        assert!(app.handle("basement_check", &no_depth).unwrap_err().contains("depth_mm"));
    }

    #[test]
    fn parking_plan_packs_stalls_and_names_bad_input() {
        let mut app = App::default();
        let base = json!({ "envelope": { "w": 40000, "h": 40000 }, "area": { "x": 1000, "y": 2000, "w": 15000, "h": 16000 },
            "stall_width_mm": 2500, "stall_length_mm": 5000, "aisle_width_mm": 6000, "required_stalls": 12,
            "turn": { "vehicle_width_mm": 1800, "wheelbase_mm": 2700, "front_overhang_mm": 900, "centre_radius_mm": 5000, "lane_width_mm": 4000 } });
        let r = app.handle("parking_plan", &base).unwrap();
        assert_eq!(r["parking"]["stall_count"], 12);
        assert_eq!(r["parking"]["stalls"].as_array().unwrap().len(), 12);
        assert_eq!(r["parking"]["count_check"], "pass");
        assert_eq!(r["parking"]["turn_check"], "pass");
        assert_eq!(r["parking"]["aisle_check"], "unchecked");
        let mut no_env = base.clone();
        no_env.as_object_mut().unwrap().remove("envelope");
        assert!(app.handle("parking_plan", &no_env).unwrap_err().contains("envelope is required"));
        let mut no_stall = base.clone();
        no_stall.as_object_mut().unwrap().remove("stall_width_mm");
        assert!(app.handle("parking_plan", &no_stall).unwrap_err().contains("stall_width_mm"));
        let mut zero = base.clone();
        zero["aisle_width_mm"] = json!(0);
        assert!(app.handle("parking_plan", &zero).unwrap_err().contains("aisle_width_mm is required"));
    }

    #[test]
    fn terrain_check_fits_ground_and_names_bad_input() {
        let mut app = App::default();
        let base = json!({ "envelope": { "w": 20000, "h": 20000 },
            "spots": [{ "x": 0, "y": 0, "z_mm": 100000 }, { "x": 20000, "y": 0, "z_mm": 101000 }, { "x": 0, "y": 20000, "z_mm": 100000 }],
            "platforms": [{ "key": "A", "x": 0, "y": 0, "w": 10000, "h": 10000, "level_mm": 100250 }],
            "max_cut_mm": 300 });
        let r = app.handle("terrain_check", &base).unwrap();
        assert_eq!(r["terrain"]["ground"]["fall_side"], "x_min");
        assert_eq!(r["terrain"]["cut_check"], "pass");
        assert_eq!(r["terrain"]["fill_check"], "unchecked");
        let mut no_env = base.clone();
        no_env.as_object_mut().unwrap().remove("envelope");
        assert!(app.handle("terrain_check", &no_env).unwrap_err().contains("envelope is required"));
        let mut few = base.clone();
        few["spots"] = json!([{ "x": 0, "y": 0, "z_mm": 1 }]);
        assert!(app.handle("terrain_check", &few).unwrap_err().contains("at least 3 spot levels"));
        let mut bad = base.clone();
        bad["spots"][0].as_object_mut().unwrap().remove("z_mm");
        assert!(app.handle("terrain_check", &bad).unwrap_err().contains("z_mm"));
    }

    #[test]
    fn wind_check_classes_rooms_and_names_bad_input() {
        let mut app = App::default();
        let rooms = json!([
            { "key": "a", "func": "bedroom", "zone": "private", "x": 0, "y": 0, "w": 6000, "h": 10000 },
            { "key": "b", "func": "hall", "zone": "public", "x": 6000, "y": 0, "w": 6000, "h": 10000 }
        ]);
        let base = json!({ "rooms": rooms, "envelope": { "w": 12000, "h": 10000 }, "wind_from": "north", "wind_source": "met station 2020", "habitable_funcs": ["bedroom"] });
        let r = app.handle("wind_check", &base).unwrap();
        assert_eq!(r["wind"]["rooms"][0]["flow"], "cross");
        let mut no_dir = base.clone();
        no_dir.as_object_mut().unwrap().remove("wind_from");
        assert!(app.handle("wind_check", &no_dir).unwrap_err().contains("prevailing wind"));
        let mut no_src = base.clone();
        no_src.as_object_mut().unwrap().remove("wind_source");
        assert!(app.handle("wind_check", &no_src).unwrap_err().contains("source"));
        let mut bad = base.clone();
        bad["wind_from"] = json!("up");
        assert!(app.handle("wind_check", &bad).unwrap_err().contains("wind_from"));
        let mut no_funcs = base.clone();
        no_funcs.as_object_mut().unwrap().remove("habitable_funcs");
        assert!(app.handle("wind_check", &no_funcs).unwrap_err().contains("habitable_funcs"));
    }

    #[test]
    fn entrances_check_separates_guest_and_family_doors_and_names_bad_input() {
        let mut app = App::default();
        let rooms = json!([
            { "key": "majlis", "func": "majlis", "zone": "public", "x": 0, "y": 0, "w": 5000, "h": 4000 },
            { "key": "hall", "func": "hall", "zone": "public", "x": 5000, "y": 0, "w": 7000, "h": 4000 }
        ]);
        let base = json!({ "rooms": rooms, "envelope": { "w": 12000, "h": 10000 }, "entry": "south", "min_separation_mm": 6000 });
        let r = app.handle("entrances_check", &base).unwrap();
        assert_eq!(r["entrances"]["status"], "separated");
        assert_eq!(r["entrances"]["separation_mm"], 10800);
        let skipped = app.handle("entrances_check", &json!({ "rooms": rooms, "envelope": { "w": 12000, "h": 10000 }, "entry": "south" })).unwrap();
        assert_eq!(skipped["entrances"]["status"], "unchecked");
        assert!(app.handle("entrances_check", &json!({ "rooms": rooms, "envelope": { "w": 12000, "h": 10000 }, "entry": "up" })).unwrap_err().contains("entry"));
        assert!(app.handle("entrances_check", &json!({ "envelope": { "w": 1, "h": 1 }, "entry": "south" })).unwrap_err().contains("rooms are required"));
    }

    #[test]
    fn ifc_review_reads_a_file_and_refuses_a_revit_file() {
        let mut app = App::default();
        let dir = std::env::temp_dir().join(format!("nanoarch_ifcrev_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ifc = dir.join("a.ifc");
        std::fs::write(&ifc, "ISO-10303-21;
HEADER;
FILE_SCHEMA(('IFC4'));
ENDSEC;
DATA;
#1=IFCPROJECT('7777777777777777777777',$,'P',$,$,$,$,$,$);
ENDSEC;
END-ISO-10303-21;
").unwrap();
        let r = app.handle("ifc_review", &json!({ "path": ifc.to_str().unwrap() })).unwrap();
        assert_eq!(r["review"]["schema"], "IFC4");
        assert_eq!(r["review"]["project_name"], "P");
        assert!(r["review"]["findings"].as_array().unwrap().iter().any(|f| f["code"] == "no_storeys"));
        let rvt = dir.join("a.rvt");
        std::fs::write(&rvt, [0xD0u8, 0xCF, 0x11, 0xE0, 0, 0]).unwrap();
        assert!(app.handle("ifc_review", &json!({ "path": rvt.to_str().unwrap() })).unwrap_err().contains("Revit"));
        assert!(app.handle("ifc_review", &json!({})).unwrap_err().contains("path"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn learned_preferences_reach_the_suggested_room_areas() {
        let mut app = App::default();
        let err = app.handle("prefs_learn", &json!({ "paths": [] })).unwrap_err();
        assert!(err.contains("paths"));
        let r = app.handle("prefs_learn", &json!({ "paths": ["C:/nonexistent/none.nnx"] })).unwrap();
        assert_eq!(r["skipped"].as_array().unwrap().len(), 1);
        assert_eq!(r["preferences"]["projects_used"], 0);
        let prefs = json!({ "rooms": [{ "func": "bedroom", "median_m2": 21.5, "min_m2": 18.0, "max_m2": 25.0, "samples": 5 }], "projects_read": 2, "projects_used": 2, "too_few": [] });
        let draft = json!({ "building_type": "villa", "floors": 1, "city": "Riyadh", "climate_zone": 1, "style": "contemporary" });
        let r = app.handle("brief_clarify", &json!({ "draft": draft, "field": "rooms", "answer": "2", "prefs": prefs })).unwrap();
        assert_eq!(r["draft"]["rooms"][0]["area_m2"], 21.5);
    }

    #[test]
    fn the_clarifying_conversation_asks_only_what_is_missing_and_ends_when_complete() {
        let mut app = App::default();
        let r = app.handle("brief_clarify", &json!({ "draft": { "building_type": "villa", "floors": 1, "city": "Riyadh", "climate_zone": 1, "style": "contemporary" } })).unwrap();
        assert_eq!(r["next"]["field"], "plot_size");
        assert_eq!(r["missing"].as_array().unwrap().len(), 2);
        let r = app.handle("brief_clarify", &json!({ "draft": r["draft"], "field": "plot_size", "answer": "20x30" })).unwrap();
        assert_eq!(r["next"]["field"], "rooms");
        let r = app.handle("brief_clarify", &json!({ "draft": r["draft"], "field": "rooms", "answer": "3" })).unwrap();
        assert_eq!((r["complete"].as_bool(), r["next"].is_null()), (Some(true), true));
        assert!(app.handle("brief_clarify", &json!({ "draft": r["draft"], "field": "floors", "answer": "many" })).unwrap_err().contains(" | "));
        assert!(app.handle("brief_clarify", &json!({})).unwrap_err().contains("draft is required"));
    }

    #[test]
    fn an_excel_area_schedule_fills_the_draft_rooms_through_the_backend() {
        let mut app = App::default();
        let b64 = nnx_core::xlsx::base64_encode(include_bytes!("../nnx-core/tests/data/areas_deflated.xlsx"));
        let r = app.handle("areas_import", &json!({ "xlsx_b64": b64 })).unwrap();
        assert_eq!(r["table"]["rooms"][0]["func"], "kitchen");
        assert!(r.get("draft").is_none());
        let draft = json!({ "building_type": "villa", "floors": 1, "city": "Riyadh", "climate_zone": 1, "style": "contemporary", "rooms": [{ "func": "bedroom", "name": "old", "area_m2": 9.0, "floor": 1, "assumed": false }] });
        let r = app.handle("areas_import", &json!({ "xlsx_b64": b64, "draft": draft })).unwrap();
        assert_eq!(r["draft"]["rooms"].as_array().unwrap().len(), 1);
        assert_eq!(r["draft"]["rooms"][0]["area_m2"], 14.5);
        assert_eq!(r["next"]["field"], "plot_size");
        assert!(app.handle("areas_import", &json!({})).unwrap_err().contains("workbook's path"));
        assert!(app.handle("areas_import", &json!({ "path": "C:/nope/none.xlsx" })).unwrap_err().contains("cannot read"));
    }

    #[test]
    fn surveyed_utm_corners_are_imported_through_the_backend() {
        let mut app = App::default();
        let t = "E,N\n500000,2700000\n500018,2700000\n500018,2700022\n500000,2700022\n";
        let r = app.handle("plot_import_survey", &json!({ "text": t, "crs": "UTM 38N" })).unwrap();
        assert!((r["result"]["candidates"][0]["area_m2"].as_f64().unwrap() - 396.0).abs() < 1e-6);
        assert!(app.handle("plot_import_survey", &json!({})).unwrap_err().contains("corner rows"));
        assert!(app.handle("plot_import_survey", &json!({ "text": t })).unwrap_err().contains("unit"));
        assert!(app.handle("plot_import_survey", &json!({ "path": "C:/nope/none.csv" })).unwrap_err().contains("cannot read"));
    }

    #[test]
    fn a_non_rectangular_plot_is_analysed_drawn_and_kept_in_the_project() {
        // The page sends the outline; the engine returns every setback and the building rectangle (a chamfered corner here).
        let input = json!({ "polygonM": [[0, 0], [18, 0], [18, 18], [14, 22], [0, 22]],
            "edges": [{ "kind": "front_street", "streetWidthM": 15.0 }, { "kind": "neighbour" }, { "kind": "neighbour" }, { "kind": "side_street", "streetWidthM": 10.0 }, { "kind": "neighbour" }],
            "rule": "ministry_villa", "floors": 2 });
        let mut app = App::default();
        let r = app.handle("site_analyze", &input).unwrap();
        assert!(r["svg"].as_str().unwrap().starts_with("<svg") && r["rule_source"].as_str().unwrap().contains("4-1"));
        let res = &r["result"];
        assert!((res["area_m2"].as_f64().unwrap() - (18.0 * 22.0 - 8.0)).abs() < 1e-6, "{res}");
        assert_eq!(res["setbacks_m"].as_array().unwrap().len(), 5);
        assert!(res["building"]["width_m"].as_f64().unwrap() > 5.0 && res["building"]["entry"] == "south");
        // Bad shapes and missing rules come back as messages, with nothing changed.
        assert!(app.handle("site_analyze", &json!({ "polygonM": [[0, 0], [10, 0]] })).is_err());
        assert!(app.handle("site_analyze", &json!({ "polygonM": "x" })).unwrap_err().starts_with("plot:"));
        // No project yet: nothing to draw. A rectangular plot draws from the brief and its setbacks (18 x 22 m, 2 m each side).
        assert!(app.handle("site_plan", &json!({})).is_err());
        let mut app = started_with_project();
        let rect = app.handle("site_plan", &json!({})).unwrap();
        assert!(rect["svg"].as_str().unwrap().contains(">N</text>") && rect["svg"].as_str().unwrap().contains("class=\"building\""));
        assert_eq!(rect["result"]["setbacks_m"], json!([2.0, 2.0, 2.0, 2.0]));
        assert_eq!((rect["result"]["building"]["width_m"].as_f64(), rect["result"]["building"]["length_m"].as_f64()), (Some(14.0), Some(18.0)));
        let edges: Vec<Value> = ["front_street", "neighbour", "neighbour", "neighbour", "neighbour", "neighbour"].iter().map(|k| json!({ "kind": k, "streetWidthM": if *k == "front_street" { json!(15.0) } else { Value::Null } })).collect();
        let l_shape = json!({ "meta": { "plotPolygonM": [[0, 0], [18, 0], [18, 10], [8, 10], [8, 22], [0, 22]], "plotEdges": edges } });
        app.handle("project_set", &l_shape).unwrap();
        let plan = app.handle("site_plan", &json!({})).unwrap();
        assert_eq!(plan["name"], "site-plan.svg");
        assert!(plan["svg"].as_str().unwrap().contains("<polygon"), "the outline is drawn");
        assert!(plan["result"]["buildable_area_m2"].as_f64().unwrap() > 100.0, "{}", plan["result"]["buildable_area_m2"]);
        // Switching back to a rectangle removes the outline, and the site plan is the rectangle's again.
        app.handle("project_set", &json!({ "meta": { "plotPolygonM": null, "plotEdges": [], "setbackRule": null } })).unwrap();
        assert_eq!(app.handle("site_plan", &json!({})).unwrap()["result"]["polygon_m"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn the_analysis_commands_report_missing_inputs_by_name_and_compare_generated_alternatives() {
        let mut app = App::default();
        let s = app.handle("analysis", &json!({ "kind": "solar", "latitude_deg": 27.0, "window_height_m": 1.5 })).unwrap();
        assert_eq!(s["table"].as_array().unwrap().len(), 4);
        assert!(app.handle("analysis", &json!({ "kind": "solar" })).unwrap_err().contains("latitude_deg is missing"));
        assert!(app.handle("analysis", &json!({ "kind": "ramp", "rise_mm": 1500, "run_mm": 10000 })).unwrap_err().contains("maximum ramp slope"));
        assert_eq!(app.handle("analysis", &json!({ "kind": "ramp", "rise_mm": 1500, "run_mm": 10000, "max_slope_pct": 15 })).unwrap()["ok"], true);
        assert!(app.handle("analysis", &json!({ "kind": "stair", "floor_height_mm": 3000, "risers": 17, "tread_mm": 270, "target_mm": 620, "tolerance_mm": 20 })).unwrap()["comfortable"].as_bool().unwrap());
        assert!(app.handle("analysis", &json!({ "kind": "screen", "eye_m": 1.7, "viewer_to_screen_m": 3, "screen_to_target_m": 6, "target_m": 1.2 })).unwrap()["required_height_m"].as_f64().unwrap() > 1.0);
        assert!(app.handle("analysis", &json!({ "kind": "nope" })).is_err());
        // No alternatives yet; then the generated ones are compared (at most three).
        assert!(app.handle("analysis", &json!({ "kind": "compare", "max_circulation_pct": 15 })).is_err());
        app.handle("generate", &json!({ "brief": brief(), "struct": {} })).unwrap();
        assert!(app.handle("analysis", &json!({ "kind": "compare" })).unwrap_err().contains("max_circulation_pct is missing"));
        let c = app.handle("analysis", &json!({ "kind": "compare", "max_circulation_pct": 15 })).unwrap();
        let rows = c["rows"].as_array().unwrap();
        assert!(!rows.is_empty() && rows.len() <= 3 && rows.iter().all(|r| r["efficiency"]["gross_m2"].as_f64().unwrap() > 0.0));
    }

    fn spaces(app: &App) -> std::collections::BTreeMap<String, (i64, i64, i64, i64)> {
        app.doc().unwrap().model.elements.values().filter(|e| e.kind == "Space").filter_map(|e| e.typed::<nnx_core::typed::Space>().ok()).map(|s| (s.requirement_key.clone(), (s.x, s.y, s.w, s.h))).collect()
    }

    #[test]
    fn a_locked_room_keeps_its_exact_rectangle_when_the_layout_is_generated_again_and_locks_undo() {
        let mut app = started_with_project();
        let before = spaces(&app);
        // Lock a room that the generator placed (not the stair or the hall).
        let key = before.keys().find(|k| *k == "majlis" || *k == "master").cloned().unwrap_or_else(|| before.keys().next().unwrap().clone());
        assert!(app.handle("lock", &json!({ "kind": "room", "key": "nope", "locked": true })).is_err());
        let r = app.handle("lock", &json!({ "kind": "room", "key": key, "locked": true })).unwrap();
        assert_eq!(r["changed"], true);
        assert_eq!(r["locked_rooms"], json!([key]));
        assert!(app.is_dirty());
        // Editing a locked room is refused, with the reason.
        let id = app.doc().unwrap().model.elements.values().find(|e| e.kind == "Space" && e.typed::<nnx_core::typed::Space>().is_ok_and(|s| s.requirement_key == key)).unwrap().id;
        assert!(app.handle("resize_room", &json!({ "room": id.to_string(), "edge": "east", "grow_mm": 100 })).unwrap_err().contains("is locked"));
        // Generate again with another seed: the locked room is where it was, to the millimetre.
        let g = app.handle("regenerate", &json!({ "seed": 7 })).unwrap();
        assert!(!g["alternatives"].as_array().unwrap().is_empty());
        for i in 0..g["alternatives"].as_array().unwrap().len() {
            app.handle("select", &json!({ "index": i, "struct": {} })).unwrap();
            assert_eq!(spaces(&app).get(&key), before.get(&key), "alternative {i}");
            assert_eq!(app.project().unwrap().meta.locked_rooms, vec![key.clone()], "the lock is part of the project");
        }
        // Undo removes the lock (one undoable step) and redo brings it back.
        app.handle("select", &json!({ "index": 0, "struct": {} })).unwrap();
        app.handle("lock", &json!({ "kind": "room", "key": key, "locked": false })).unwrap();
        assert!(app.project().unwrap().meta.locked_rooms.is_empty());
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(app.project().unwrap().meta.locked_rooms, vec![key.clone()]);
        app.handle("redo", &json!({})).unwrap();
        assert!(app.project().unwrap().meta.locked_rooms.is_empty());
    }

    #[test]
    fn a_locked_wall_refuses_changes_to_itself_and_its_openings_until_unlocked() {
        let mut app = started_with_project();
        let (door, wall) = {
            let m = &app.doc().unwrap().model;
            let d = m.elements.values().find(|e| e.kind == "Door").unwrap();
            (d.id, d.host_id.unwrap())
        };
        assert!(app.handle("lock", &json!({ "kind": "wall", "id": door.to_string(), "locked": true })).is_err(), "a door is not a wall");
        app.handle("lock", &json!({ "kind": "wall", "id": wall.to_string(), "locked": true })).unwrap();
        let e = app.handle("set_opening", &json!({ "id": door.to_string(), "width": 1000 })).unwrap_err();
        assert!(e.contains("locked"), "{e}");
        assert!(app.handle("delete_opening", &json!({ "id": door.to_string() })).unwrap_err().contains("locked"));
        assert!(app.handle("move_partition", &json!({ "wall": wall.to_string(), "delta_mm": 100 })).unwrap_err().contains("locked"));
        app.handle("lock", &json!({ "kind": "wall", "id": wall.to_string(), "locked": false })).unwrap();
        assert!(app.handle("set_opening", &json!({ "id": door.to_string(), "width": 1000 })).is_ok());
    }

    #[test]
    fn locking_rooms_that_leave_no_space_for_the_rest_says_so() {
        let mut app = started_with_project();
        let before = spaces(&app);
        let mut locked = Vec::new();
        for k in before.keys() {
            match app.handle("lock", &json!({ "kind": "room", "key": k, "locked": true })) {
                Ok(_) => locked.push(k.clone()),
                Err(e) => assert!(e.contains("cannot be locked") && k.starts_with("reserve"), "{k}: {e}"),
            }
        }
        assert!(locked.len() >= 8, "{locked:?}");
        // Every room of the brief locked: they stay exactly where they are (the filler hall is not a room of the brief).
        let g = app.handle("regenerate", &json!({})).unwrap();
        assert_eq!(g["alternatives"].as_array().unwrap().len(), 1);
        app.handle("select", &json!({ "index": 0, "struct": {} })).unwrap();
        let after = spaces(&app);
        for k in &locked {
            assert_eq!(after.get(k), before.get(k), "{k}");
        }
    }

    #[test]
    fn site_layout_vertical_and_furniture_inputs_live_in_the_project_and_show_in_3d_and_the_site_plan() {
        let mut app = started_with_project();
        let kinds = |app: &mut App, kind: &str| app.handle("model3d", &json!({})).unwrap()["boxes"].as_array().unwrap().iter().filter(|b| b["kind"] == kind).count();
        assert!(app.handle("site_layout", &json!({})).unwrap_err().contains("no site layout inputs"));
        assert!(app.handle("furniture", &json!({})).unwrap_err().contains("minimum clear walkway"));
        assert!(app.handle("vertical", &json!({})).unwrap_err().contains("no vertical inputs"));
        assert_eq!((kinds(&mut app, "Site"), kinds(&mut app, "Furniture"), kinds(&mut app, "Vertical")), (0, 0, 0));
        // Impossible numbers are refused and change nothing.
        let hash = app.doc().unwrap().model.state_hash();
        assert!(app.handle("project_set", &json!({ "meta": { "siteLayout": { "wallHeightM": -2.0 } } })).is_err());
        assert!(app.handle("project_set", &json!({ "meta": { "vertical": { "basement": { "levels": 9, "heightMm": 3000.0 } } } })).is_err());
        assert_eq!(app.doc().unwrap().model.state_hash(), hash);
        let meta = json!({ "meta": {
            "siteLayout": { "wallHeightM": 2.5, "wallThicknessM": 0.2, "gateWidthM": 3.0, "carport": { "widthM": 2.5, "lengthM": 4.0, "heightM": 2.5 } },
            "furniture": { "minClearMm": 900.0 },
            "vertical": { "elevator": { "carWidthMm": 1000.0, "carDepthMm": 1300.0, "clearSideMm": 100.0, "clearBackMm": 150.0, "clearFrontMm": 100.0, "wallMm": 200.0, "pitMm": 1200.0, "overheadMm": 3800.0, "xMm": 300.0, "yMm": 300.0 },
                          "roofAnnex": { "xMm": 500.0, "yMm": 500.0, "widthMm": 4000.0, "lengthMm": 3000.0, "heightMm": 2800.0 } } } });
        assert_eq!(app.handle("project_set", &meta).unwrap()["changed"], true);
        assert!(kinds(&mut app, "Site") >= 5 && kinds(&mut app, "Furniture") > 0 && kinds(&mut app, "Vertical") >= 2);
        let site = app.handle("site_layout", &json!({})).unwrap();
        assert!(site["svg"].as_str().unwrap().contains("gate"));
        // The plain site plan shows the layout too.
        let plan = app.handle("site_plan", &json!({})).unwrap();
        assert!(plan["svg"].as_str().unwrap().contains("gate") && plan["layout"]["walls"].as_array().unwrap().len() >= 4);
        let f = app.handle("furniture", &json!({})).unwrap();
        assert!(f["svg"].as_str().unwrap().starts_with("<svg") && !f["report"]["pieces"].as_array().unwrap().is_empty());
        assert!(app.handle("vertical", &json!({})).unwrap()["shaft"]["outer_mm"] == json!([1600.0 + 0.0, 1850.0 + 0.0]) || app.handle("vertical", &json!({})).unwrap()["shaft"].is_object());
        // One undo reverts all of it (it was one project edit), and the inputs survive a save and reopen.
        let dir = std::env::temp_dir().join(format!("nanoarch-m1-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m1.nnx");
        app.handle("save_to", &json!({ "path": path.to_string_lossy() })).ok();
        app.handle("undo", &json!({})).unwrap();
        assert_eq!((kinds(&mut app, "Site"), kinds(&mut app, "Furniture"), kinds(&mut app, "Vertical")), (0, 0, 0));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn compliance_inputs_checklist_and_baseline_live_in_the_project_with_undo() {
        let mut app = started_with_project();
        assert_eq!(app.handle("materials", &json!({})).unwrap()["materials"].as_array().unwrap().len(), 22);
        let r = app.handle("compliance", &json!({})).unwrap();
        assert_eq!(r["malath"]["items"].as_array().unwrap().len(), 11);
        assert!(r["baseline"].is_null() && r["changes"].as_array().unwrap().is_empty());
        assert!(!r["rooms"].as_array().unwrap().is_empty());
        // Inputs: stored as one undoable project edit; impossible numbers are refused and change nothing.
        let hash = app.doc().unwrap().model.state_hash();
        assert!(app.handle("compliance_set", &json!({ "compliance": { "streetWidthM": -3.0 } })).is_err());
        assert!(app.handle("compliance_set", &json!({ "compliance": { "thermal": { "bandRatios": [1, 2] } } })).is_err());
        assert_eq!(app.doc().unwrap().model.state_hash(), hash);
        let inputs = json!({ "streetWidthM": 15.0, "access": { "doorMinClearMm": 900.0, "source": "test circular" },
            "thermal": { "rsi": 0.12, "rse": 0.03, "windowU": 2.0, "wallLayers": [{ "material": "rockwool_150", "thicknessMm": 100.0 }, { "material": "block_hollow_1216", "thicknessMm": 200.0 }], "roofLayers": [{ "material": "xps_35", "thicknessMm": 150.0 }] } });
        let r = app.handle("compliance_set", &json!({ "compliance": inputs })).unwrap();
        assert_eq!(r["changed"], true);
        assert!(r["thermal"]["rows"][0]["u"].as_f64().unwrap() > 0.0 && r["access"]["rows"].as_array().unwrap().iter().any(|x| x["rule"] == "access.door_width"));
        assert_eq!(app.project().unwrap().meta.compliance.street_width_m, Some(15.0));
        // The checklist.
        let r = app.handle("malath_set", &json!({ "id": "soil.level", "status": "accepted", "note": "checked on site" })).unwrap();
        assert_eq!(r["malath"]["items"][1]["status"], "accepted");
        assert!(app.handle("malath_set", &json!({ "id": "nope", "status": "accepted" })).is_err());
        assert!(app.handle("malath_set", &json!({ "id": "soil.level", "status": "done" })).is_err());
        app.handle("undo", &json!({})).unwrap();
        assert!(app.project().unwrap().meta.compliance.malath.is_empty(), "the tick was one undo step");
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(app.project().unwrap().meta.compliance, Default::default(), "and the inputs another");
        app.handle("redo", &json!({})).unwrap();
        assert_eq!(app.project().unwrap().meta.compliance.street_width_m, Some(15.0));
    }

    #[test]
    fn a_baseline_tracks_changes_draws_revision_clouds_and_the_exports_carry_the_history() {
        let mut app = started_with_project();
        assert!(app.handle("baseline_set", &json!({ "name": "  " })).is_err());
        let r = app.handle("baseline_set", &json!({ "name": "Rev A" })).unwrap();
        assert_eq!(r["baseline"]["name"], "Rev A");
        assert!(r["changes"].as_array().unwrap().is_empty());
        let door = app.doc().unwrap().model.elements.values().find(|e| e.kind == "Door").unwrap().id;
        app.handle("set_opening", &json!({ "id": door.to_string(), "width": 1000 })).unwrap();
        let r = app.handle("compliance", &json!({})).unwrap();
        assert!(r["change_counts"]["modified"].as_u64().unwrap() >= 1, "{}", r["change_counts"]);
        assert!(r["changes"].as_array().unwrap().iter().any(|c| c["id"] == door.to_string() && c["status"] == "modified"));
        // Clouds only when switched on.
        let plain = app.handle("svg", &json!({})).unwrap()["svg"].as_str().unwrap().to_string();
        assert!(!plain.contains("rev-cloud"));
        let cl = app.handle("set_clouds", &json!({ "show": true })).unwrap()["svg"].as_str().unwrap().to_string();
        assert!(cl.contains("class=\"rev-cloud\"") && cl.contains(&door.to_string()));
        assert!(!app.handle("set_clouds", &json!({ "show": false })).unwrap()["svg"].as_str().unwrap().contains("rev-cloud"));
        // The plan print and the certificate carry the revision-history table.
        let pdf = app.handle("pdf_html", &json!({ "lang": "en" })).unwrap();
        assert!(pdf["html"].as_str().unwrap().contains("Revision history") && pdf["html"].as_str().unwrap().contains("Rev A"));
        let cert = app.handle("certificate_html", &json!({ "lang": "en" })).unwrap();
        assert_eq!(cert["name"], "balady_certificate.pdf");
        let h = cert["html"].as_str().unwrap();
        assert!(h.contains("NOT an official certificate of Balady") && h.contains("5-1-4") && h.contains("Revision history") && h.contains("Rev A"));
        assert!(app.handle("certificate_html", &json!({})).unwrap()["html"].as_str().unwrap().contains("dir=\"rtl\""));
        // A second baseline closes the first into the history; one undo takes it back.
        let r = app.handle("baseline_set", &json!({ "name": "Rev B" })).unwrap();
        assert_eq!(r["history"][0]["name"], "Rev A");
        assert!(r["history"][0]["modified"].as_u64().unwrap() >= 1);
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(app.project().unwrap().meta.baseline.unwrap().name, "Rev A");
        // Without a document nothing is invented.
        assert!(App::default().handle("certificate_html", &json!({})).is_err() && App::default().handle("compliance", &json!({})).is_err());
    }
    /// A project with columns, footings (loads and soil given) and slabs, so the structural pack has something to work on.
    fn started_with_structure() -> App {
        let mut app = App::default();
        let s = json!({ "dead_kpa": 5.0, "live_kpa": 2.0, "bearing_kpa": 150.0, "floors": 1 });
        app.handle("generate", &json!({ "brief": brief(), "struct": s, "meta": { "title": "x" } })).unwrap();
        app.handle("select", &json!({ "index": 0, "struct": s })).unwrap();
        app
    }

    fn count(app: &App, kind: &str) -> usize {
        app.doc().unwrap().model.elements.values().filter(|e| e.kind == kind).count()
    }

    #[test]
    fn structural_inputs_live_in_the_project_and_every_analysis_names_what_it_misses() {
        let mut app = started_with_structure();
        assert!(count(&app, "Column") >= 4 && count(&app, "Footing") >= 4);
        let r = app.handle("structural", &json!({})).unwrap();
        assert!(r["grid"]["missing"].as_array().unwrap().len() == 2 && r["merge"]["missing"].as_array().unwrap().len() == 3);
        assert!(r["punching"]["missing"].as_array().unwrap().iter().any(|m| m.as_str().unwrap().contains("f'c")));
        assert!(!r["slabs"]["levels"][0]["systems"][0]["missing"].as_array().unwrap().is_empty() && r["tanks"].as_array().unwrap().is_empty());
        // Impossible numbers are refused and change nothing.
        let hash = app.doc().unwrap().model.state_hash();
        assert!(app.handle("structural_set", &json!({ "structural": { "fcMpa": -5.0 } })).is_err());
        assert!(app.handle("structural_set", &json!({ "structural": { "merge": { "raftThresholdPct": 500.0 } } })).is_err());
        assert_eq!(app.doc().unwrap().model.state_hash(), hash);
        let inputs = json!({ "fcMpa": 25.0, "loadFactor": 1.4, "phiShear": 0.75, "punchingFactor": 0.33, "coverMm": 50.0, "barDiaMm": 16.0, "beamSpanDepthMax": 16.0, "slabSpanDepthMax": 30.0,
            "grid": { "maxSpanMm": 7000.0, "snapMm": 300.0 }, "cutting": { "stockMm": 12000.0, "lapMm": 600.0 },
            "tankDesign": { "fcMpa": 25.0, "fyMpa": 420.0, "phiFlexure": 0.9, "gammaSoil": 1.6, "gammaWater": 1.4, "gammaLiquid": 1.4, "gammaDead": 1.2, "concreteUnitWeightKnM3": 25.0, "rhoMin": 0.0018, "rhoMax": 0.02, "coverMm": 50.0, "barDiasMm": [12.0, 16.0], "minSpacingMm": 100.0, "maxSpacingMm": 250.0, "upliftFsMin": 1.1, "minThicknessMm": 200.0, "hookMm": 150.0 },
            "tanks": [{ "name": "T1", "kind": "water", "innerLengthMm": 4000.0, "innerWidthMm": 3000.0, "liquidDepthMm": 2000.0, "freeboardMm": 300.0, "soilCoverMm": 500.0, "groundwaterDepthMm": 1500.0, "soilUnitWeightKnM3": 18.0, "k0": 0.5, "surchargeKpa": 10.0, "liquidUnitWeightKnM3": 9.81, "compartments": 1 }] });
        let r = app.handle("structural_set", &json!({ "structural": inputs })).unwrap();
        assert_eq!(r["changed"], true);
        assert!(r["punching"]["missing"].as_array().unwrap().iter().all(|m| !m.as_str().unwrap().contains("f'c")));
        assert!(!r["punching"]["deflection"].as_array().unwrap().is_empty());
        assert_eq!(r["tanks"][0]["status"], "ok");
        assert!(r["tanks"][0]["bars"].as_array().unwrap().len() >= 6);
        assert!(r["cutting"]["stock_bars"].as_u64().unwrap() > 0, "the tank bars are nested");
        assert!(!r["grid"]["proposals"].as_array().unwrap().is_empty());
        assert_eq!(app.project().unwrap().meta.structural.fc_mpa, Some(25.0));
        // One undo reverts the inputs.
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(app.project().unwrap().meta.structural, Default::default());
        app.handle("redo", &json!({})).unwrap();
        assert_eq!(app.project().unwrap().meta.structural.cutting.stock_mm, Some(12000.0));
    }

    #[test]
    fn merging_footings_into_a_raft_is_undoable_and_survives_a_structure_sync() {
        let mut app = started_with_structure();
        let before = count(&app, "Footing");
        assert!(app.handle("merge_apply", &json!({})).unwrap_err().contains("enter:"));
        app.handle("structural_set", &json!({ "structural": { "merge": { "minGapMm": 10.0, "raftThresholdPct": 1.0, "raftEdgeMm": 300.0 } } })).unwrap();
        let plan = app.handle("structural", &json!({})).unwrap();
        assert_eq!(plan["merge"]["raft"], true);
        let r = app.handle("merge_apply", &json!({})).unwrap();
        assert_eq!(r["merged"], json!(["RAFT-1"]));
        assert_eq!(count(&app, "Footing"), 1);
        // The structure sync does not make the single footings again.
        let s = app.handle("sync", &json!({})).unwrap();
        assert_eq!(count(&app, "Footing"), 1, "{s}");
        // The raft carries every column and shows in the schedule by its label.
        let raft = app.doc().unwrap().model.elements.values().find(|e| e.kind == "Footing").unwrap();
        assert_eq!(raft.props["columns"].as_array().unwrap().len(), before);
        app.handle("undo", &json!({})).unwrap();
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(count(&app, "Footing"), before);
    }

    #[test]
    fn the_analytical_model_exports_as_dxf_and_ifc() {
        let mut app = started_with_structure();
        let d = app.handle("export_analytical", &json!({ "kind": "dxf" })).unwrap();
        assert!(d["data"].as_str().unwrap().contains("S-GRID") && d["data"].as_str().unwrap().ends_with("EOF\n") && d["nodes"].as_u64().unwrap() >= 8);
        let i = app.handle("export_analytical", &json!({ "kind": "ifc" })).unwrap();
        let text = i["data"].as_str().unwrap();
        assert!(text.starts_with("ISO-10303-21;") && text.contains("IFCSTRUCTURALPOINTCONNECTION") && text.contains("IFCSTRUCTURALCURVEMEMBER") && text.contains("IFCSTRUCTURALSURFACEMEMBER"));
        assert!(i["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("no load factor")));
        assert!(app.handle("export_analytical", &json!({ "kind": "stl" })).is_err());
        app.handle("structural_set", &json!({ "structural": { "loadFactor": 1.4 } })).unwrap();
        assert!(app.handle("export_analytical", &json!({})).unwrap()["data"].as_str().unwrap().contains(" Pu="));
        assert!(App::default().handle("export_analytical", &json!({})).is_err() && App::default().handle("structural", &json!({})).is_err());
    }
    #[test]
    fn the_advanced_mep_inputs_live_in_the_project_and_every_analysis_names_what_it_misses() {
        let mut app = started();
        let r = app.handle("mepx", &json!({})).unwrap();
        for k in ["solar", "smart", "wifi", "cctv", "ev", "balance", "grey", "hydra"] {
            assert!(r.get(k).is_some(), "{k}");
        }
        assert!(r["solar"]["missing"].as_array().unwrap().iter().any(|m| m.as_str().unwrap().contains("edge clearance")));
        assert!(r["wifi"]["missing"].as_array().unwrap().len() == 8 && !r["smart"]["missing"].as_array().unwrap().is_empty());
        // Impossible numbers are refused and change nothing.
        let hash = app.doc().unwrap().model.state_hash();
        assert!(app.handle("mepx_set", &json!({ "mepExt": { "solar": { "coverageRatio": 2.0 } } })).is_err());
        assert!(app.handle("mepx_set", &json!({ "mepExt": { "smart": { "protocol": "x10" } } })).is_err());
        assert_eq!(app.doc().unwrap().model.state_hash(), hash);
        let ext = json!({
            "solar": { "panelW": 550.0, "panelAreaM2": 2.6, "edgeClearanceMm": 500.0, "coverageRatio": 0.6, "specificYieldKwhKw": 1700.0, "tariffPerKwh": 0.18, "selfConsumption": 0.7, "dcAcRatio": 1.2 },
            "wifi": { "bandGhz": 5.0, "txPowerDbm": 20.0, "exponent": 3.0, "interiorWallDb": 5.0, "exteriorWallDb": 10.0, "thresholdDbm": -65.0, "targetCoveragePct": 90.0, "maxAps": 4.0 },
            "cctv": { "cameras": [{ "name": "C1", "xMm": 9000.0, "yMm": 0.0, "mountHeightMm": 3000.0, "azimuthDeg": 180.0, "fovDeg": 90.0, "rangeM": 8.0 }], "perimeterOffsetMm": 1000.0 },
            "smart": { "protocol": "knx", "panelRoom": "living", "maxDevicesPerLine": 8.0, "maxLineM": 150.0, "routeFactor": 1.3, "busZMm": 2800.0, "busMm": 20.0 },
            "grey": { "fixtures": ["basin"], "fixtureLDay": { "basin": 60.0 }, "unitXMm": -2000.0, "unitYMm": -2000.0, "reuseDemandLDay": 100.0, "storageDays": 2.0 }
        });
        let r = app.handle("mepx_set", &json!({ "mepExt": ext })).unwrap();
        assert_eq!(r["changed"], true);
        assert!(r["solar"]["panels"].as_u64().unwrap() > 0 && r["solar"]["annual_saving"].as_f64().unwrap() > 0.0);
        assert!(r["wifi"]["coverage_pct"].as_f64().unwrap() > 0.0 && r["wifi"]["aps"].as_array().unwrap().iter().all(|a| a["suggested"] == true));
        assert!(r["cctv"]["covered_m2"].as_f64().unwrap() > 0.0);
        // The suggested smart devices become the project's devices (one undoable edit), and the bus is routed.
        let devices = app.handle("smart_suggest", &json!({})).unwrap()["devices"].clone();
        assert!(devices.as_array().unwrap().len() > 20);
        let mut with = ext.clone();
        with["smart"]["devices"] = devices;
        let r = app.handle("mepx_set", &json!({ "mepExt": with })).unwrap();
        assert!(!r["smart"]["lines"].as_array().unwrap().is_empty() && r["smart"]["total_bus_m"].as_f64().unwrap() > 0.0);
        // The network drawing and the 3D scene carry the coverage.
        let svg = app.handle("mepx_svg", &json!({ "floor": 1 })).unwrap();
        assert!(svg["svg"].as_str().unwrap().contains("cctv-cone") && svg["svg"].as_str().unwrap().contains("class=\"ap\"") || svg["svg"].as_str().unwrap().contains("class=\"wifi\""));
        let scene = app.handle("model3d", &json!({})).unwrap();
        assert!(scene["boxes"].as_array().unwrap().iter().any(|b| b["kind"] == "Coverage" && b["class"] == "camera"));
        // The bus and the greywater drains are generated with the MEP points when the MEP inputs are applied.
        let mut mi = mep_inputs();
        mi["routing"] = json!({ "drain_slope_pct": 1.5, "drain_invert_mm": 400, "stack_offset_mm": 1500 });
        let m = app.handle("mep", &json!({ "inputs": mi, "apply": true })).unwrap();
        assert!(m["applied"]["created"].as_u64().unwrap() > 0);
        let roles: std::collections::BTreeSet<String> = app.doc().unwrap().model.elements.values().filter(|e| e.kind == "MepRun").filter_map(|e| e.props.get("role").and_then(|v| v.as_str()).map(str::to_string)).collect();
        assert!(roles.contains("smart-bus") && roles.contains("grey"), "{roles:?}");
        // The inputs are one undoable project edit.
        let before = app.project().unwrap().meta.mep_ext.clone();
        app.handle("mepx_set", &json!({ "mepExt": { "solar": { "panelW": 400.0 } } })).unwrap();
        assert_ne!(app.project().unwrap().meta.mep_ext, before);
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(app.project().unwrap().meta.mep_ext, before);
        assert!(App::default().handle("mepx", &json!({})).is_err());
    }

    #[test]
    fn the_hvac_router_can_be_told_to_avoid_the_structure_through_the_mep_inputs() {
        let mut app = started_with_structure();
        let mut mi = mep_inputs();
        mi["routing"] = json!({ "ahu_room": "living", "supply_delta_t_c": 10, "duct_velocity_mps": 5, "duct_height_mm": 250, "ceiling_height_mm": 2300, "return_offset_mm": 400, "avoid_structure": true, "structure_clearance_mm": 50 });
        let r = app.handle("mep", &json!({ "inputs": mi, "apply": true })).unwrap();
        assert!(r["report"]["routes"].as_array().unwrap().iter().any(|x| x["system"] == "duct"), "{}", r["report"]["missing"]);
        let clashes = app.handle("clashes", &json!({})).unwrap();
        // Whatever remains is reported by the clash check, never hidden.
        assert!(clashes["runs"].as_u64().unwrap() > 0);
        // The same report comes back with the apply itself, so no separate request is needed to see what hits the structure.
        assert_eq!(r["structure_clashes"], clashes);
        let mut bad = mep_inputs();
        bad["routing"] = json!({ "ahu_room": "living", "supply_delta_t_c": 10, "duct_velocity_mps": 5, "duct_height_mm": 250, "ceiling_height_mm": 2300, "return_offset_mm": 400, "avoid_structure": true });
        let m = app.handle("mep", &json!({ "inputs": bad })).unwrap();
        assert!(m["report"]["missing"]["routing.structure"][0].as_str().unwrap().contains("structure_clearance_mm"));
    }
    #[test]
    fn every_starter_template_applies_and_generates_a_plan_with_no_empty_room_area() {
        let list = App::default().handle("templates", &json!({})).unwrap();
        let ids: Vec<String> = list["templates"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap().to_string()).collect();
        assert_eq!(ids, vec!["villa_jubail", "palace", "apartments_4", "duplex", "annex"]);
        assert!(App::default().handle("template_apply", &json!({ "id": "nope" })).is_err());
        for id in ids {
            let r = App::default().handle("template_apply", &json!({ "id": id })).unwrap();
            let t = &r["template"];
            assert!(r["sources"].as_array().unwrap().iter().all(|s| s.as_str().unwrap().contains("clause 4-")));
            // The page sends exactly this: the plot, the setbacks from the rule, the rooms with their areas.
            let rooms: Vec<Value> = t["rooms"].as_array().unwrap().iter().map(|x| json!({ "key": x["key"], "name": x["name"], "func": x["func"], "area": x["area_m2"], "zone": x["zone"], "light": x["func"] != "bathroom", "floor": x["floor"] })).collect();
            assert!(rooms.iter().all(|x| x["area"].as_f64().unwrap() > 0.0));
            // A single-storey template (the annex) has no stair, as the page sends it.
            let mut brief = json!({ "title": t["project_title"], "width_m": t["plot_width_m"], "length_m": t["plot_length_m"], "setback_front_m": r["setbacks"]["front_m"], "setback_back_m": r["setbacks"]["back_m"], "setback_side_m": r["setbacks"]["side_m"], "entry": "south", "rooms": rooms });
            if let Some(st) = t["stair"].as_array() {
                brief["stair"] = json!({ "width_m": st[0], "length_m": st[1], "side": st[2] });
            }
            let meta = json!({ "buildingType": t["building_type"], "style": t["style"], "climateZone": t["city"] });
            let mut app = App::default();
            let g = app.handle("generate", &json!({ "brief": brief, "struct": { "floors": t["floors"] }, "meta": meta })).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(!g["alternatives"].as_array().unwrap().is_empty(), "{id}");
            let s = app.handle("select", &json!({ "index": 0, "struct": { "floors": t["floors"] } })).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(s["svg"].as_str().unwrap().starts_with("<svg"), "{id}");
            // The site plan of the template's rectangle shows the Ministry setbacks.
            let site = app.handle("site_plan", &json!({})).unwrap();
            assert_eq!(site["result"]["setbacks_m"][0], r["setbacks"]["front_m"], "{id}");
        }
    }

    #[test]
    fn auto_views_draws_every_elevation_and_section_with_all_levels_and_needs_a_project() {
        let mut app = started_with_project();
        let r = app.handle("auto_views", &json!({})).unwrap();
        let views = r["views"].as_array().unwrap();
        assert!(views.len() >= 6, "four elevations and A-A, B-B at least");
        assert_eq!(views.iter().filter(|v| v["kind"] == "elevation").count(), 4);
        assert!(views.iter().all(|v| v["svg"].as_str().unwrap().starts_with("<svg")));
        assert_eq!(r["complete"], true, "{}", r["notes"]);
        let levels = r["model_levels"].as_array().unwrap().len();
        assert!(views.iter().all(|v| v["levels_shown"].as_array().unwrap().len() >= levels.max(1)));
        let mut empty = App::default();
        assert!(empty.handle("auto_views", &json!({})).is_err());
    }

    #[test]
    fn the_projects_style_adds_facade_solids_to_the_3d_scene_and_the_elevations() {
        let mut app = started_with_project();
        let count = |app: &mut App, kind: &str| app.handle("model3d", &json!({})).unwrap()["boxes"].as_array().unwrap().iter().filter(|b| b["kind"] == kind).count();
        // The helper project names the style "najdi", so its scene already carries the Najdi facade.
        assert!(count(&mut app, "Facade") > 20);
        app.handle("project_set", &json!({ "meta": { "style": null } })).unwrap();
        assert_eq!(count(&mut app, "Facade"), 0, "no style, no facade");
        let plain = app.handle("elevation", &json!({ "side": "south" })).unwrap()["svg"].as_str().unwrap().to_string();
        assert!(!plain.contains("class=\"facade-"));
        app.handle("project_set", &json!({ "meta": { "style": "نجدي حديث" } })).unwrap();
        assert!(count(&mut app, "Facade") > 20);
        let najdi = app.handle("elevation", &json!({ "side": "south" })).unwrap()["svg"].as_str().unwrap().to_string();
        assert!(najdi.contains("class=\"facade-merlon\"") && najdi.contains("class=\"facade-slit\""));
        // A style the application does not know draws nothing and is not an error.
        app.handle("project_set", &json!({ "meta": { "style": "baroque" } })).unwrap();
        assert_eq!(count(&mut app, "Facade"), 0);
        app.handle("project_set", &json!({ "meta": { "style": "contemporary" } })).unwrap();
        assert!(app.handle("elevation", &json!({ "side": "south" })).unwrap()["svg"].as_str().unwrap().contains("facade-cantilever"));
    }

    #[test]
    fn every_project_input_survives_save_and_reopen_and_stays_in_step_with_undo() {
        let mut app = started_with_project();
        app.handle("project_set", &json!({ "meta": { "title": "x", "buildingType": "apartment_building", "climateZone": "zone-b", "smartTech": [] },
            "limits": { "minSetbackFrontM": { "value": 2.0, "source": "test municipal circular" }, "enforce": false } })).unwrap();
        app.handle("mep", &json!({ "inputs": mep_inputs(), "apply": false })).unwrap();
        let project = app.project().unwrap();

        let dir = std::env::temp_dir().join(format!("nanoarch-ssot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("p.nnx");
        nnx_core::persist::save_with_settings(app.model().unwrap(), &app.settings(), &file).unwrap();
        let (loaded, back) = nnx_core::persist::load_with_settings(&file).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        let mut second = App::default();
        let opened = second.open(loaded, &back).unwrap();
        assert!(opened["warnings"].as_array().unwrap().is_empty(), "{}", opened["warnings"]);
        let reopened = second.project().unwrap();
        assert_eq!(serde_json::to_value(&reopened).unwrap(), serde_json::to_value(&project).unwrap(), "the project is identical after a round trip");
        assert!(!second.is_dirty());
        assert_eq!(second.handle("project_set", &setbacks(2.0, 2.0, 2.0)).unwrap()["changed"], false);
        second.handle("project_set", &setbacks(2.5, 2.0, 2.0)).unwrap();
        assert!(second.is_dirty());
        second.handle("undo", &json!({})).unwrap();
        assert!(!second.is_dirty(), "undo after reopening returns to the file's state");
    }

    #[test]
    fn a_file_saved_before_projects_were_in_the_model_is_migrated_and_nothing_is_dropped_silently() {
        let mut app = started_with_project();
        let mut model = app.model().unwrap().clone();
        model.elements.retain(|_, e| e.kind != "Project");
        let mut legacy = Settings::new();
        legacy.insert("title".into(), json!("قديم"));
        legacy.insert("labels".into(), json!({ "majlis": "المجلس" }));
        legacy.insert("brief".into(), brief());
        legacy.insert("structure".into(), json!({ "bearing_kpa": 150.0 }));
        legacy.insert("mep".into(), json!({ "electrical": "not an object" }));
        let mut old = App::default();
        let opened = old.open(model, &legacy).unwrap();
        let p = old.project().expect("the inputs were moved into a project element");
        assert_eq!(p.meta.title, "قديم");
        assert_eq!(p.structure.bearing_kpa, Some(150.0));
        assert!(p.brief.is_some() && p.mep.is_none());
        let warnings = opened["warnings"].to_string();
        assert!(warnings.contains("MEP inputs could not be read"), "the unreadable part is reported: {warnings}");
        assert!(!old.is_dirty(), "the migrated file opens clean");
        let _ = app.handle("level", &json!({ "index": 0 }));
    }

    #[test]
    fn municipal_limits_are_checked_with_their_source_and_a_missing_limit_is_skipped_not_passed() {
        let mut app = started_with_project();
        let r = app.handle("sbc_check", &json!({})).unwrap();
        assert!(r["violations"].as_array().unwrap().iter().all(|v| !v["rule"].as_str().unwrap().starts_with("setback") && v["rule"] != "coverage"), "no municipal limit is entered yet, so none can be broken");
        let skipped: Vec<String> = r["skipped"].as_array().unwrap().iter().map(|s| s["rule"].as_str().unwrap().to_string()).collect();
        for rule in ["setback.front", "setback.back", "setback.side", "coverage"] {
            assert!(skipped.contains(&rule.to_string()), "{rule} is reported as not checked: {skipped:?}");
        }
        let limits = json!({ "limits": {
            "minSetbackFrontM": { "value": 3.0, "source": "municipal circular (test data)" },
            "minSetbackBackM": { "value": 2.0, "source": "municipal circular (test data)" },
            "maxCoverageRatio": { "value": 0.5, "source": "municipal circular (test data)" } } });
        let r = app.handle("project_set", &limits).unwrap();
        let rules: Vec<String> = r["sbc"]["violations"].as_array().unwrap().iter().map(|v| v["rule"].as_str().unwrap().to_string()).collect();
        assert!(rules.contains(&"setback.front".to_string()), "2 m given, 3 m required: {rules:?}");
        assert!(!rules.contains(&"setback.back".to_string()), "2 m given, 2 m required passes");
        assert!(rules.contains(&"coverage".to_string()), "252 m2 on a 396 m2 plot is 63.6%, above 50%: {rules:?}");
        let v = r["sbc"]["violations"].as_array().unwrap().iter().find(|v| v["rule"] == "setback.front").unwrap().clone();
        assert!(v["en"].as_str().unwrap().contains("front setback") && v["ar"].as_str().unwrap().contains("الارتداد الأمامي"));
        assert_eq!(v["source"], "municipal circular (test data)");
        assert_eq!(r["new_violations"].as_array().unwrap().len(), 2, "the edit itself introduced both");
    }

    #[test]
    fn a_project_that_enforces_the_limits_refuses_an_edit_that_breaks_them() {
        let mut app = started_with_project();
        app.handle("project_set", &json!({ "limits": { "minSetbackFrontM": { "value": 2.0, "source": "municipal circular (test data)" }, "enforce": true } })).unwrap();
        let before = hash(&app);
        let e = app.handle("project_set", &setbacks(1.5, 2.0, 2.0)).unwrap_err();
        assert!(e.contains("front setback") && e.contains("الارتداد الأمامي"), "{e}");
        assert_eq!(hash(&app), before, "a refused edit changes nothing");
        assert!(app.handle("project_set", &setbacks(2.5, 2.0, 2.0)).is_ok());
    }

    // ------------------------------------------------------------------ room tools

    /// Ids of two rooms that share a full wall of the same length, found the way the editor decides.
    fn adjacent_pair(app: &App) -> (nnx_core::Id, nnx_core::Id) {
        let rooms: Vec<(nnx_core::Id, nnx_core::layout::Rect, String)> = app
            .doc
            .as_ref()
            .unwrap()
            .model
            .elements
            .values()
            .filter_map(|e| e.typed::<nnx_core::typed::Space>().ok().map(|s| (e.id, nnx_core::layout::Rect { x: s.x, y: s.y, w: s.w, h: s.h }, s.func)))
            .collect();
        for (i, a) in rooms.iter().enumerate() {
            for b in &rooms[i + 1..] {
                let (ra, rb) = (a.1, b.1);
                let side = ra.y == rb.y && ra.h == rb.h && (ra.x + ra.w == rb.x || rb.x + rb.w == ra.x);
                let stack = ra.x == rb.x && ra.w == rb.w && (ra.y + ra.h == rb.y || rb.y + rb.h == ra.y);
                if side || stack {
                    return (a.0, b.0);
                }
            }
        }
        panic!("the fixture plan has no two rooms sharing a full wall");
    }

    fn room_rects(app: &App) -> std::collections::BTreeMap<nnx_core::Id, (i64, i64, i64, i64)> {
        app.doc.as_ref().unwrap().model.elements.values().filter_map(|e| Some((e.id, e.typed::<nnx_core::typed::Space>().ok()?))).map(|(id, s)| (id, (s.x, s.y, s.w, s.h))).collect()
    }

    fn assert_valid(app: &App) {
        let issues = nnx_core::typed::validate_model(&app.doc.as_ref().unwrap().model);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn two_rooms_can_exchange_places_in_one_undoable_step_and_the_plan_still_tiles() {
        let mut app = started_with_project();
        let (a, b) = adjacent_pair(&app);
        let (before, rects) = (hash(&app), room_rects(&app));
        app.handle("swap_rooms", &json!({ "a": a.to_string(), "b": b.to_string() })).unwrap();
        let now = room_rects(&app);
        assert_ne!(now[&a], rects[&a], "the first room moved");
        assert_eq!((now[&a].2, now[&a].3), (rects[&a].2, rects[&a].3), "and kept its size");
        assert_eq!((now[&b].2, now[&b].3), (rects[&b].2, rects[&b].3));
        assert_tiles(&app);
        assert_valid(&app);
        assert!(app.is_dirty());
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(hash(&app), before, "one undo restores the whole plan");
        app.handle("redo", &json!({})).unwrap();
        assert_eq!(room_rects(&app), now);
    }

    #[test]
    fn a_room_is_deleted_by_giving_its_space_to_a_neighbour_and_one_undo_brings_it_back() {
        let mut app = started_with_project();
        let (a, b) = adjacent_pair(&app);
        let (before, rects) = (hash(&app), room_rects(&app));
        app.handle("delete_room", &json!({ "room": a.to_string(), "into": b.to_string() })).unwrap();
        let now = room_rects(&app);
        assert!(!now.contains_key(&a), "the room is gone");
        assert_eq!(now.len(), rects.len() - 1);
        assert_eq!(now[&b].2 * now[&b].3, rects[&a].2 * rects[&a].3 + rects[&b].2 * rects[&b].3, "the neighbour took the space");
        assert_tiles(&app);
        assert_valid(&app);
        assert_eq!(app.rooms_json().len(), rects.len() - 1);
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(hash(&app), before);
        assert_eq!(room_rects(&app), rects);
        // A room that does not share a full wall cannot absorb it.
        let far = rects.keys().copied().find(|id| *id != a && *id != b && !{
            let (ra, rf) = (rects[&a], rects[id]);
            (ra.1 == rf.1 && ra.3 == rf.3 && (ra.0 + ra.2 == rf.0 || rf.0 + rf.2 == ra.0)) || (ra.0 == rf.0 && ra.2 == rf.2 && (ra.1 + ra.3 == rf.1 || rf.1 + rf.3 == ra.1))
        });
        if let Some(far) = far {
            let e = app.handle("delete_room", &json!({ "room": a.to_string(), "into": far.to_string() })).unwrap_err();
            assert!(e.contains("full wall"), "{e}");
            assert_eq!(hash(&app), before);
        }
        assert!(app.handle("delete_room", &json!({ "room": a.to_string(), "into": a.to_string() })).is_err());
    }

    #[test]
    fn a_room_is_resized_through_its_interior_wall_and_the_outline_stays_fixed() {
        let mut app = started_with_project();
        let rects = room_rects(&app);
        let before = hash(&app);
        let mut done = None;
        'search: for (&id, r) in &rects {
            for edge in ["x0", "x1", "y0", "y1"] {
                if let Ok(out) = app.handle("resize_room", &json!({ "room": id.to_string(), "edge": edge, "grow_mm": 300 })) {
                    done = Some((id, edge, *r, out));
                    break 'search;
                }
            }
        }
        let (id, edge, old, out) = done.expect("at least one room has an interior side that can move");
        assert!(out["moved"].as_u64().unwrap() >= 2, "the room and its neighbour both changed");
        let now = room_rects(&app)[&id];
        let grown = (now.2 * now.3) - (old.2 * old.3);
        assert_eq!(grown, 300 * if edge.starts_with('x') { old.3 } else { old.2 }, "the room grew by exactly the strip moved");
        assert_tiles(&app);
        assert_valid(&app);
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(hash(&app), before);
        // A side on the building outline cannot move, and a refused edit leaves everything as it was.
        let outer = rects.iter().find(|(_, r)| r.0 == 0).map(|(id, _)| *id).unwrap();
        let e = app.handle("resize_room", &json!({ "room": outer.to_string(), "edge": "x0", "grow_mm": 300 })).unwrap_err();
        assert!(e.contains("outline"), "{e}");
        assert!(app.handle("resize_room", &json!({ "room": outer.to_string(), "edge": "north", "grow_mm": 300 })).is_err());
        assert_eq!(hash(&app), before);
    }

    /// Shrinks a bathroom until it is 1.3 m wide (the minimum in the code files is 1.4 m); returns the answer of the
    /// first edit that the geometry allows, with the hash before it.
    fn squeeze_a_bathroom(app: &mut App) -> (u64, Result<Value, String>) {
        let rooms: Vec<(nnx_core::Id, (i64, i64, i64, i64))> = app
            .doc
            .as_ref()
            .unwrap()
            .model
            .elements
            .values()
            .filter_map(|e| Some((e.id, e.typed::<nnx_core::typed::Space>().ok()?)))
            .filter(|(_, s)| s.func == "bathroom")
            .map(|(id, s)| (id, (s.x, s.y, s.w, s.h)))
            .collect();
        for (id, (_, _, w, h)) in rooms {
            for (edge, size) in [("x0", w), ("x1", w), ("y0", h), ("y1", h)] {
                let before = hash(app);
                let r = app.handle("resize_room", &json!({ "room": id.to_string(), "edge": edge, "grow_mm": 1300 - size }));
                match &r {
                    Err(e) if e.contains("outline") || e.contains("no partition") || e.contains("ends against") || e.contains("minimum for an edit") => continue,
                    _ => return (before, r),
                }
            }
        }
        panic!("no bathroom side could be moved");
    }

    #[test]
    fn code_minimums_are_reported_with_their_page_and_refuse_the_edit_only_when_the_project_enforces_them() {
        // Not enforced: the edit goes through and the violation is shown with its citation.
        let mut app = started_with_project();
        let first = app.handle("sbc_check", &json!({})).unwrap();
        let rule_of = |r: &Value, prefix: &str| r["violations"].as_array().unwrap().iter().any(|v| v["rule"].as_str().unwrap().starts_with(prefix));
        assert!(!rule_of(&first, "room_min"), "the generated plan meets the area and width minimums: {}", first["violations"]);
        // The generator sizes windows to the code's 8%, so the plan as generated has no daylight shortfall either.
        assert!(first["violations"].as_array().unwrap().is_empty(), "{}", first["violations"]);
        let (_, r) = squeeze_a_bathroom(&mut app);
        let out = r.expect("accepted while the code is not enforced");
        let v = out["sbc"]["violations"].as_array().unwrap().iter().find(|v| v["rule"] == "room_min_width_m.bathroom").expect("the narrow bathroom is reported").clone();
        assert_eq!(v["limit"], 1.4);
        assert!(v["source"].as_str().unwrap().contains("5-1-4") && v["source"].as_str().unwrap().contains("p.26"), "{}", v["source"]);
        assert!(v["ar"].as_str().unwrap().contains("العرض") && v["en"].as_str().unwrap().contains("below the minimum"));

        // Enforced: the same edit is refused and nothing changes.
        let mut strict = started_with_project();
        strict.handle("project_set", &json!({ "limits": { "enforce": true } })).unwrap();
        let (before, r) = squeeze_a_bathroom(&mut strict);
        let e = r.unwrap_err();
        // The refusal names the first new violation, with the clause it comes from (the rebuilt windows can add a
        // daylight shortfall next to the width one).
        assert!(e.contains(" / ") && e.contains("Ministry residential guidelines 5-1-"), "{e}");
        assert_eq!(hash(&strict), before);
        assert!(!rule_of(&strict.handle("sbc_check", &json!({})).unwrap(), "room_min"), "the refused edit left the plan as it was");
    }

    #[test]
    fn generated_windows_reach_the_codes_glazing_ratio_in_every_alternative_and_after_edits() {
        let mut app = App::default();
        let g = app.handle("generate", &json!({ "brief": brief(), "struct": {} })).unwrap();
        let n = g["alternatives"].as_array().unwrap().len();
        for i in 0..n {
            let sel = app.handle("select", &json!({ "index": i, "struct": {} })).unwrap();
            let bad: Vec<&Value> = sel["sbc"]["violations"].as_array().unwrap().iter().filter(|v| v["rule"].as_str().unwrap().starts_with("daylight")).collect();
            // A room that stays under the ratio is one whose exterior walls are too short, and the plan says so by name.
            for v in &bad {
                let key = v["en"].as_str().unwrap().split(' ').next().unwrap();
                assert!(sel["warnings"].to_string().contains(&format!("room '{key}' cannot reach")), "alternative {i}: {v} is not explained by {}", sel["warnings"]);
            }
            if i == generator_best(&g) {
                assert!(bad.is_empty(), "the generator's best alternative reaches the ratio everywhere: {bad:?}");
            }
        }
        // Moving a partition rebuilds the windows with the same target.
        app.handle("select", &json!({ "index": generator_best(&g), "struct": {} })).unwrap();
        let wall = app.doc.as_ref().unwrap().model.elements.values().find(|e| e.kind == "Wall" && e.props["isExterior"] == json!(false)).map(|e| e.id.to_string()).unwrap();
        for delta in [300, -300] {
            if let Ok(r) = app.handle("move_partition", &json!({ "wall": wall, "delta_mm": delta })) {
                let any = r["sbc"]["violations"].as_array().unwrap().iter().any(|v| v["rule"].as_str().unwrap().starts_with("daylight"));
                assert!(!any, "{}", r["sbc"]["violations"]);
            }
        }
        // A two-storey building as well.
        let two = started2();
        assert!(!two.sbc_json()["violations"].to_string().contains("daylight"), "{}", two.sbc_json()["violations"]);
    }

    // ------------------------------------------------------------------ elevations, sections, layers, clashes

    #[test]
    fn elevations_and_sections_follow_the_model_and_the_parapet_comes_from_the_project() {
        let mut app = started_with_project();
        for side in ["north", "south", "east", "west"] {
            let r = app.handle("elevation", &json!({ "side": side })).unwrap();
            let svg = r["svg"].as_str().unwrap();
            assert!(svg.starts_with("<svg") && svg.contains("class=\"wall\"") && svg.contains("±0.00"), "{side}");
            assert!(!svg.contains("class=\"parapet\""), "no parapet until the project has a height");
        }
        // The entrance is on the south: the south elevation has the door leaf, the north one does not.
        assert!(app.handle("elevation", &json!({ "side": "south" })).unwrap()["svg"].as_str().unwrap().contains("class=\"leaf\""));
        assert!(!app.handle("elevation", &json!({ "side": "north" })).unwrap()["svg"].as_str().unwrap().contains("class=\"leaf\""));
        // Every window pane is a window of the model, by id.
        let south = app.handle("elevation", &json!({ "side": "south" })).unwrap()["svg"].as_str().unwrap().to_string();
        let windows = app.doc.as_ref().unwrap().model.elements.values().filter(|e| e.kind == "Window").count();
        assert!(windows > 0 && south.matches("class=\"glass\"").count() > 0);

        app.handle("project_set", &json!({ "meta": { "parapetHeightM": 1.0 } })).unwrap();
        assert!(app.handle("elevation", &json!({ "side": "east" })).unwrap()["svg"].as_str().unwrap().contains("class=\"parapet\""));
        let aa = app.handle("section", &json!({ "name": "A-A" })).unwrap();
        assert!(aa["svg"].as_str().unwrap().contains("A-A") && aa["svg"].as_str().unwrap().contains("class=\"wall cut\"") && aa["svg"].as_str().unwrap().contains("class=\"parapet\""));
        let bb = app.handle("section", &json!({ "name": "B-B", "at": 5.0 })).unwrap();
        assert!(bb["svg"].as_str().unwrap().contains("x = 5.00"));
        // Errors name the problem.
        assert!(app.handle("section", &json!({ "name": "A-A", "at": 99.0 })).unwrap_err().contains("outside the building"));
        assert!(app.handle("section", &json!({ "name": "Z-Z" })).unwrap_err().contains("A-A or B-B"));
        assert!(app.handle("elevation", &json!({ "side": "up" })).unwrap_err().contains("unknown side"));
        assert!(app.handle("project_set", &json!({ "meta": { "parapetHeightM": -1.0 } })).unwrap_err().contains("parapet"));
        assert!(App::default().handle("elevation", &json!({ "side": "south" })).is_err());
        // Two storeys: the level marks of the upper floor appear.
        let two = started2();
        let mut two = two;
        let svg = two.handle("elevation", &json!({ "side": "south" })).unwrap()["svg"].as_str().unwrap().to_string();
        assert!(svg.contains("Floor 1 +") && svg.contains("Ground floor ±0.00"), "{}", &svg[..svg.len().min(200)]);
    }

    #[test]
    fn clash_classes_separate_hard_routing_and_clearance_and_state_what_is_not_assessed() {
        let mut app = started_with_project();
        let model = &app.doc.as_ref().unwrap().model;
        let col = model.elements.values().find(|e| e.kind == "Column").expect("the generated plan has columns");
        let c = col.typed::<nnx_core::typed::Column>().unwrap();
        let (cx, cy) = (c.x, c.y);
        let none = app.handle("clash_classes", &json!({})).unwrap();
        assert_eq!((none["hard"].as_array().unwrap().len(), none["routing"].as_array().unwrap().len()), (0, 0), "no runs, no conflicts");
        // A pipe through the column (hard) and a duct across that pipe (routing).
        app.handle("add_run", &json!({ "system": "pipe", "x1": cx - 600, "y1": cy, "z1": 1000, "x2": cx + 600, "y2": cy, "z2": 1000, "width": 100, "height": 100 })).unwrap();
        app.handle("add_run", &json!({ "system": "duct", "x1": cx - 300, "y1": cy - 600, "z1": 1000, "x2": cx - 300, "y2": cy + 600, "z2": 1000, "width": 100, "height": 100 })).unwrap();
        let r = app.handle("clash_classes", &json!({})).unwrap();
        assert!(r["hard"].as_array().unwrap().iter().any(|k| k["struct_kind"] == "Column"));
        assert_eq!(r["routing"].as_array().unwrap().len(), 1, "{r}");
        assert!(r["not_assessed"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().starts_with("clearance")), "no gap entered: clearance is not assessed");
        assert!(r["not_assessed"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().starts_with("maintenance access")));
        let with_gap = app.handle("clash_classes", &json!({ "clearance_mm": 100 })).unwrap();
        assert!(!with_gap["not_assessed"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().starts_with("clearance")));
    }

    #[test]
    fn runs_are_undoable_model_elements_and_clashes_name_the_run_and_the_structure() {
        let mut app = started_with_project();
        let model = &app.doc.as_ref().unwrap().model;
        let col = model.elements.values().find(|e| e.kind == "Column").expect("the generated plan has columns");
        let c = col.typed::<nnx_core::typed::Column>().unwrap();
        let (cid, cx, cy) = (col.id.to_string(), c.x, c.y);
        assert!(app.handle("clashes", &json!({})).unwrap()["clashes"].as_array().unwrap().is_empty(), "no runs, no clashes");

        let before = hash(&app);
        let r = app.handle("add_run", &json!({ "system": "pipe", "x1": cx - 600, "y1": cy, "z1": 1000, "x2": cx + 600, "y2": cy, "z2": 1000, "width": 100, "height": 100 })).unwrap();
        let run_id = r["id"].as_str().unwrap().to_string();
        let mine = r["new_clashes"].as_array().unwrap();
        assert!(mine.iter().any(|k| k["struct_id"] == cid.as_str() && k["run_id"] == run_id.as_str() && k["struct_kind"] == "Column"), "{mine:?}");
        assert!(mine.iter().all(|k| k["run_discipline"] == "plumbing" && k["struct_discipline"] == "structural"));
        assert_eq!(app.handle("runs", &json!({})).unwrap().as_array().unwrap().len(), 1);
        // The run is in the 3D scene with its discipline, and filtering by discipline works on the same data.
        let scene = app.handle("model3d", &json!({})).unwrap();
        assert!(scene["boxes"].as_array().unwrap().iter().any(|b| b["id"] == run_id.as_str() && b["discipline"] == "plumbing" && b["kind"] == "MepRun"));
        assert!(scene["boxes"].as_array().unwrap().iter().all(|b| ["architectural", "structural", "electrical", "plumbing", "hvac"].contains(&b["discipline"].as_str().unwrap())));
        // One undo removes the run and every clash it made.
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(hash(&app), before);
        assert!(app.handle("clashes", &json!({})).unwrap()["clashes"].as_array().unwrap().is_empty());
        app.handle("redo", &json!({})).unwrap();
        let del = app.handle("delete_run", &json!({ "id": run_id })).unwrap();
        assert!(del["runs"].as_array().unwrap().is_empty() && del["clashes"]["clashes"].as_array().unwrap().is_empty());
        // What is not a run, and a bad run, are refused by name and change nothing.
        let wall = app.doc.as_ref().unwrap().model.elements.values().find(|e| e.kind == "Wall").map(|e| e.id.to_string()).unwrap();
        let h = hash(&app);
        assert!(app.handle("delete_run", &json!({ "id": wall })).unwrap_err().contains("not a duct"));
        assert!(app.handle("add_run", &json!({ "system": "steam", "x1": 0, "y1": 0, "z1": 0, "x2": 100, "y2": 0, "z2": 0, "width": 10, "height": 10 })).unwrap_err().contains("unknown run system"));
        assert!(app.handle("add_run", &json!({ "system": "duct", "x1": 0, "y1": 0, "z1": 0, "x2": 100, "y2": 100, "z2": 0, "width": 10, "height": 10 })).unwrap_err().contains("straight"));
        assert!(app.handle("add_run", &json!({ "system": "duct" })).unwrap_err().contains("required"));
        assert_eq!(hash(&app), h);
        // A building with several levels needs the level of the run.
        let mut two = started2();
        let e = two.handle("add_run", &json!({ "system": "duct", "x1": 0, "y1": 0, "z1": 2500, "x2": 500, "y2": 0, "z2": 2500, "width": 100, "height": 100 })).unwrap_err();
        assert!(e.contains("which level"), "{e}");
        assert!(two.handle("add_run", &json!({ "system": "duct", "level": 1, "x1": 0, "y1": 0, "z1": 2500, "x2": 500, "y2": 0, "z2": 2500, "width": 100, "height": 100 })).is_ok());
    }

    // ------------------------------------------------------------------ routing and structural schedules

    fn routing_inputs() -> Value {
        let mut i = mep_inputs();
        i["hvac"]["cop"] = json!(3.0);
        i["routing"] = json!({
            "ahu_room": "living", "supply_delta_t_c": 10, "duct_velocity_mps": 5, "duct_height_mm": 300, "ceiling_height_mm": 2600, "return_offset_mm": 400,
            "drain_slope_pct": 1.5, "drain_invert_mm": 400, "stack_offset_mm": 1500,
            "tank": { "kind": "ground", "x_mm": -2000, "y_mm": 1000, "z_mm": 0 }, "supply_z_mm": 600,
            "tray_z_mm": 2800, "tray_width_mm": 300, "tray_height_mm": 100 });
        i
    }

    #[test]
    fn routing_through_the_mep_command_creates_runs_and_diffusers_found_by_the_clash_check_and_one_undo_removes_them() {
        let mut app = started_with_project();
        let runs_in_model = |a: &App| a.doc.as_ref().unwrap().model.elements.values().filter(|e| e.kind == "MepRun").count();
        assert_eq!(runs_in_model(&app), 0);
        let r = app.handle("mep", &json!({ "inputs": routing_inputs(), "apply": true, "show": true })).unwrap();
        assert_eq!(r["report"]["missing"], json!({}), "{}", r["report"]["missing"]);
        let routes = r["report"]["routes"].as_array().unwrap();
        for system in ["duct", "drain", "pipe", "cable"] {
            assert!(routes.iter().any(|x| x["system"] == system), "no {system} run in {}", r["report"]["routes"]);
        }
        let drain = routes.iter().find(|x| x["system"] == "drain").unwrap();
        assert!((drain["slope_pct"].as_f64().unwrap() - 1.5).abs() < 0.1, "{drain}");
        let n = runs_in_model(&app);
        assert_eq!(n, routes.len(), "every route is a model element");
        assert_eq!(app.handle("runs", &json!({})).unwrap().as_array().unwrap().len(), n);
        // Cooling load and electrical totals in the units asked for.
        let h = &r["report"]["hvac"];
        assert!(h["total_btu_hr"].as_f64().unwrap() > 0.0 && h["total_tons"].as_f64().unwrap() > 0.0 && h["rooms"][0]["airflow_lps"].as_f64().unwrap() > 0.0);
        assert!(r["report"]["electrical"]["demand_kva"].as_f64().unwrap() > 0.0);
        assert!(r["report"]["electrical"]["circuits"].as_array().unwrap().iter().any(|c| c["kind"] == "hvac"));
        // Ducts hang in the beams' zone, so the clash check reports them with exact ids.
        let c = app.handle("clashes", &json!({})).unwrap();
        let list = c["clashes"].as_array().unwrap();
        assert!(!list.is_empty(), "a duct crossing the room lines meets the beams: {c}");
        let model = &app.doc.as_ref().unwrap().model;
        for k in list {
            let (run, st) = (k["run_id"].as_str().unwrap().parse().unwrap(), k["struct_id"].as_str().unwrap().parse().unwrap());
            assert_eq!(model.get(&run).unwrap().kind, "MepRun");
            assert!(matches!(model.get(&st).unwrap().kind.as_str(), "Beam" | "Column" | "Footing"));
        }
        // The 3D scene carries every discipline now; the quantities count the runs in metres.
        let scene = app.handle("model3d", &json!({})).unwrap();
        let disc: std::collections::BTreeSet<&str> = scene["boxes"].as_array().unwrap().iter().map(|b| b["discipline"].as_str().unwrap()).collect();
        assert_eq!(disc.len(), 5, "{disc:?}");
        let q = app.handle("qto", &json!({})).unwrap();
        assert!(q["items"].as_array().unwrap().iter().any(|i| i["code"].as_str().unwrap().starts_with("MEP.run.duct") && i["unit"] == "m"));
        // One undo takes the generated points and runs away; the inputs stay.
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(runs_in_model(&app), 0);
        assert!(app.project().unwrap().mep.unwrap().routing.is_some());
        assert!(app.handle("clashes", &json!({})).unwrap()["clashes"].as_array().unwrap().is_empty());
    }

    fn sized_app() -> App {
        let mut app = App::default();
        let s = json!({ "dead_kpa": 5.0, "live_kpa": 2.0, "bearing_kpa": 150.0, "max_span_mm": 5000 });
        app.handle("generate", &json!({ "brief": brief(), "struct": s })).unwrap();
        app.handle("select", &json!({ "index": 0, "struct": s })).unwrap();
        app
    }

    fn sizing_inputs() -> Value {
        json!({
            "footing_thickness_mm": 500, "tie_beam": { "width_mm": 250, "span_depth_ratio": 12 },
            "rebar": { "cover_mm": 40, "anchorage_mm": 300, "hook_mm": 100,
                "column": { "bar_dia_mm": 16, "count": 8, "tie_dia_mm": 8, "tie_spacing_mm": 200 },
                "footing": { "bar_dia_mm": 12, "spacing_mm": 150 },
                "beam": { "top_dia_mm": 16, "top_count": 2, "bottom_dia_mm": 16, "bottom_count": 3, "stirrup_dia_mm": 8, "stirrup_spacing_mm": 150 } } })
    }

    #[test]
    fn the_structural_calc_report_lists_assumptions_then_results_and_changes_nothing() {
        let mut app = sized_app();
        assert!(app.handle("structural_calc_report", &json!({})).unwrap_err().contains("run `sizing` first"));
        app.handle("sizing", &json!({ "inputs": sizing_inputs() })).unwrap();
        let h = hash(&app);
        let r = app.handle("structural_calc_report", &json!({ "lang": "en" })).unwrap();
        assert_eq!(hash(&app), h, "the report must not edit the project");
        let rep = &r["calc_report"];
        let get = |k: &str| rep["assumptions"].as_array().unwrap().iter().find(|a| a["key"] == k).unwrap()["value"].clone();
        assert_eq!(get("sizing.footing_thickness_mm"), "500");
        assert_eq!(get("sizing.rebar.cover_mm"), "40");
        // Inputs the test never entered stay unset and are named; the report is not complete.
        assert_eq!(get("design.fy_mpa"), Value::Null);
        assert_eq!(rep["status"], "incomplete");
        assert!(rep["missing_assumptions"].as_array().unwrap().iter().any(|m| m == "design.fy_mpa"));
        let md = r["markdown"].as_str().unwrap();
        assert!(md.find("## 1.").unwrap() < md.find("## 2.").unwrap());
        let ar = app.handle("structural_calc_report", &json!({ "lang": "ar" })).unwrap();
        assert!(ar["markdown"].as_str().unwrap().contains("الفرضيات"));
        assert!(app.handle("structural_calc_report", &json!({ "lang": "fr" })).unwrap_err().contains("lang"));
    }

    #[test]
    fn the_rebar_drawings_command_draws_the_stored_sizing_inputs_and_changes_nothing() {
        let mut app = sized_app();
        assert!(app.handle("rebar_drawings", &json!({})).unwrap_err().contains("run `sizing` first"));
        app.handle("sizing", &json!({ "inputs": sizing_inputs() })).unwrap();
        let h = hash(&app);
        let r = app.handle("rebar_drawings", &json!({})).unwrap();
        assert_eq!(hash(&app), h, "drawing must not edit the project");
        // A beam the template cannot fit (a 100 mm deep member with 16 mm bars, 8 mm stirrups and 40 mm cover) is listed with its
        // reason and not drawn; nothing else is skipped.
        for s in r["skipped"].as_array().unwrap() {
            assert!(s["reason"].as_str().unwrap().contains("overlap"), "{s}");
            assert!(s["members"].as_array().unwrap().iter().all(|m| m.as_str().unwrap().starts_with("BM-") || m.as_str().unwrap().starts_with("TB-")), "{s}");
        }
        let kinds: Vec<&str> = r["sheets"].as_array().unwrap().iter().map(|s| s["kind"].as_str().unwrap()).collect();
        assert!(kinds.contains(&"column") && kinds.contains(&"footing") && kinds.contains(&"beam"), "{kinds:?}");
        let dxf = r["dxf"].as_str().unwrap();
        assert!(dxf.starts_with("0\nSECTION") && dxf.ends_with("0\nEOF\n") && dxf.contains("S-RBAR-LINK"));
        // Every column of the report is on a sheet.
        let rep = app.handle("sizing", &json!({ "inputs": sizing_inputs() })).unwrap();
        let drawn: Vec<&str> = r["sheets"].as_array().unwrap().iter().flat_map(|s| s["members"].as_array().unwrap()).map(|m| m.as_str().unwrap()).collect();
        for c in rep["report"]["columns"].as_array().unwrap() {
            assert!(drawn.contains(&c["name"].as_str().unwrap()), "{} is not drawn", c["name"]);
        }
    }

    #[test]
    fn the_sizing_command_schedules_columns_footings_tie_beams_concrete_and_bars_by_element_id() {
        let mut app = sized_app();
        let r = app.handle("sizing", &json!({ "inputs": sizing_inputs() })).unwrap();
        let rep = &r["report"];
        assert_eq!(rep["missing"], json!({}), "{}", rep["missing"]);
        assert_eq!(r["soil_kpa"], 150.0);
        let cols = rep["columns"].as_array().unwrap();
        assert!(cols.len() >= 4 && cols.iter().all(|c| c["name"].as_str().unwrap().starts_with("COL-") && c["axial_kn"].as_f64().unwrap() > 0.0));
        // The loads carried by all columns equal the loads of all slabs: 7 kPa over the plan.
        let carried: f64 = cols.iter().map(|c| c["axial_kn"].as_f64().unwrap()).sum();
        let plan_m2: f64 = app.rooms_json().iter().map(|r| r["area"].as_f64().unwrap()).sum();
        assert!((carried - plan_m2 * 7.0).abs() < 1.0 + plan_m2 * 7.0 * 1e-3, "{carried} vs {}", plan_m2 * 7.0);
        let trib: f64 = cols.iter().map(|c| c["tributary_m2"].as_f64().unwrap()).sum();
        assert!((trib - plan_m2).abs() < 0.05, "{trib} vs {plan_m2}");
        assert!(rep["footings"].as_array().unwrap().iter().all(|f| f["name"].as_str().unwrap().starts_with("FND-") && f["status"] == "ok"));
        assert!(!rep["tie_beams"].as_array().unwrap().is_empty() && rep["concrete_total_m3"].as_f64().unwrap() > 0.0 && rep["steel_total_kg"].as_f64().unwrap() > 0.0);
        assert!(rep["bars"].as_array().unwrap().iter().filter(|b| !b["element"].as_str().unwrap().starts_with("TB-")).all(|b| b["id"].is_string()));
        assert!(r["bbs_csv"].as_str().unwrap().contains("COL-1/V") && r["concrete_csv"].as_str().unwrap().contains("TOTAL") && r["members_csv"].as_str().unwrap().contains("FND-1"));
        // The schedule with shapes and bends agrees with the report's own steel total and carries a type mark on every line.
        assert!(r["bbs_error"].is_null(), "{}", r["bbs_error"]);
        assert!((r["bbs"]["total_kg"].as_f64().unwrap() - rep["steel_total_kg"].as_f64().unwrap()).abs() < 0.01);
        assert!(r["bbs"]["lines"].as_array().unwrap().iter().all(|l| l["type_mark"].as_str().unwrap().starts_with('M')));
        assert!(r["bbs_summary_csv"].as_str().unwrap().contains("unit_kg_m") && r["bbs_lines_csv"].as_str().unwrap().contains("TOTAL"));
        // The inputs live in the project: undo takes them out, the same inputs again are not a change.
        assert!(app.project().unwrap().sizing.is_some());
        let h = hash(&app);
        app.handle("sizing", &json!({ "inputs": sizing_inputs() })).unwrap();
        assert_eq!(hash(&app), h);
        let ties = |a: &App| a.doc.as_ref().unwrap().model.elements.values().filter(|e| e.kind == "TieBeam").count();
        assert_eq!(ties(&app), rep["tie_beams"].as_array().unwrap().len(), "the tie beams are model elements now");
        assert!(rep["tie_beams"].as_array().unwrap().iter().all(|t| t["id"].is_string()));
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(ties(&app), 0, "the first undo takes the tie beams out");
        assert!(app.project().unwrap().sizing.is_some());
        app.handle("undo", &json!({})).unwrap();
        assert!(app.project().unwrap().sizing.is_none(), "the second takes the inputs out");
        // Incomplete inputs are named, nothing is computed from nothing.
        let bare = app.handle("sizing", &json!({ "inputs": {} })).unwrap();
        assert!(bare["report"]["missing"]["tie beams"].is_array() && bare["report"]["missing"]["reinforcement"].is_array());
        assert!(app.handle("sizing", &json!({ "inputs": { "tie_beam": "x" } })).unwrap_err().contains("sizing inputs"));
    }

    // ------------------------------------------------------------------ 4D schedule, DXF and the engineering report

    /// A sized building with its tie beams, its routed MEP runs and the stored inputs, ready to schedule and export.
    fn complete_app() -> App {
        let mut app = sized_app();
        app.handle("sizing", &json!({ "inputs": sizing_inputs() })).unwrap();
        let mut i = routing_inputs();
        i["routing"]["tank"]["y_mm"] = json!(3000);
        let r = app.handle("mep", &json!({ "inputs": i, "apply": true, "show": true })).unwrap();
        assert!(!r["report"]["routes"].as_array().unwrap().is_empty());
        app
    }

    fn all_rates(app: &mut App) -> Value {
        let keys = app.handle("template_keys", &json!({})).unwrap();
        let mut rates = serde_json::Map::new();
        for k in keys.as_array().unwrap() {
            let r = if k["fixed"] == true { json!({ "fixed_days": 2, "crews": 1, "source": "synthetic" }) } else { json!({ "per_crew_day": 5.0, "crews": 1, "source": "synthetic" }) };
            rates.insert(k["key"].as_str().unwrap().to_string(), r);
        }
        Value::Object(rates)
    }

    fn schedule_args(rates: Value) -> Value {
        json!({ "rates": rates, "start": "2026-11-15", "workdays": [true, true, true, true, false, false, true], "zones": 1 })
    }

    #[test]
    fn a_phase_schedule_is_a_view_that_leaves_the_stored_setup_alone() {
        let mut app = complete_app();
        let rates = all_rates(&mut app);
        let full = app.handle("schedule", &schedule_args(rates.clone())).unwrap();
        assert_eq!(full["phase"], Value::Null);
        let stored = hash(&app);
        let mut args = schedule_args(rates.clone());
        args["phase"] = json!("UNSET");
        let view = app.handle("schedule", &args).unwrap();
        assert_eq!(view["ok"], true, "{view}");
        assert_eq!(view["phase"], "UNSET");
        assert_eq!(hash(&app), stored, "a phase schedule stores nothing");
        assert!(view["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("not split by phase")));
        // Nothing in the model has a phase, so the whole of the work is UNSET: the block-work quantity is the same as in the full schedule.
        let qty = |r: &Value| r["tasks"].as_array().unwrap().iter().find(|t| t["name"] == "Blockwork").map(|t| t["quantity"].as_f64().unwrap());
        assert_eq!(qty(&view), qty(&full));
        args["phase"] = json!("NEW");
        let none_new = app.handle("schedule", &args).unwrap();
        assert!(none_new["ok"] == true || none_new["missing"].is_array());
        args["phase"] = json!("DEMOLISHED");
        assert!(app.handle("schedule", &args).unwrap_err().contains("not a phase"));
    }

    #[test]
    fn the_4d_schedule_has_structural_and_mep_tasks_with_quantities_phases_and_element_ids() {
        let mut app = complete_app();
        let rates = all_rates(&mut app);
        let r = app.handle("schedule", &schedule_args(rates)).unwrap();
        assert_eq!(r["ok"], true, "{r}");
        let tasks = r["tasks"].as_array().unwrap();
        let by = |n: &str| tasks.iter().find(|t| t["name"] == n).unwrap_or_else(|| panic!("no task {n}"));
        let model = &app.doc.as_ref().unwrap().model;
        for (name, kind, unit) in [("Tie beams", "TieBeam", "m3"), ("HVAC ductwork", "MepRun", "m"), ("Below-slab drainage", "MepRun", "m"), ("Water supply pipework", "MepRun", "m"), ("Cable trays", "MepRun", "m"), ("Footing concrete pour", "Footing", "m2")] {
            let t = by(name);
            assert_eq!(t["unit"], unit, "{name}");
            let ids = t["elements"].as_array().unwrap();
            assert!(!ids.is_empty(), "{name} cites elements");
            assert!(ids.iter().all(|id| model.get(&id.as_str().unwrap().parse().unwrap()).is_some_and(|e| e.kind == kind)), "{name}: {ids:?}");
        }
        // Concrete and steel of the same work.
        let pour = by("Footing concrete pour");
        assert!(pour["also"].as_array().unwrap().iter().any(|m| m["label"] == "concrete" && m["unit"] == "m3" && m["quantity"].as_f64().unwrap() > 0.0));
        assert!(by("Footing reinforcement")["also"].as_array().unwrap().iter().any(|m| m["label"] == "steel" && m["unit"] == "kg"));
        // Phases in order and working-day bounds for the 4D slider.
        let order = ["Substructure", "Superstructure", "Insulation", "MEP rough-in", "Finishing", "Commissioning"];
        for p in order {
            assert!(tasks.iter().any(|t| t["phase"] == p), "{p}");
        }
        assert!(tasks.iter().all(|t| t["es"].as_i64().unwrap() <= t["ef"].as_i64().unwrap() && t["wbs"].as_str().unwrap().contains('.')));
        assert!(r["project_days"].as_i64().unwrap() >= tasks.iter().map(|t| t["ef"].as_i64().unwrap()).max().unwrap());
    }

    #[test]
    fn the_dxf_export_can_include_runs_tie_beams_elevations_and_sections() {
        let mut app = complete_app();
        let plain = app.handle("export_dxf", &json!({})).unwrap();
        assert_eq!(plain["views"], 0);
        assert!(plain["counts"]["M-DUCT"].as_u64().unwrap() > 0 && plain["counts"]["P-DRAN"].as_u64().unwrap() > 0 && plain["counts"]["E-TRAY"].as_u64().unwrap() > 0 && plain["counts"]["S-TIEB"].as_u64().unwrap() > 0);
        assert!(plain["counts"].as_object().unwrap().keys().all(|k| !k.starts_with("V-")));
        let with = app.handle("export_dxf", &json!({ "views": true })).unwrap();
        assert_eq!(with["views"], 6, "four elevations and two sections");
        for layer in ["V-WALL", "V-CUT", "V-FAR", "V-SLAB", "V-LEVEL", "V-TEXT"] {
            assert!(with["counts"][layer].as_u64().unwrap() > 0, "{layer}");
        }
        assert!(with["dxf"].as_str().unwrap().len() > plain["dxf"].as_str().unwrap().len());
        // Runs are left out when the MEP overlay is off in the plan? They follow the model: always exported.
    }

    #[test]
    fn the_engineering_report_comes_as_html_and_xlsx_from_the_stored_inputs() {
        let mut app = complete_app();
        let h = app.handle("report_html", &json!({ "lang": "en" })).unwrap();
        let html = h["html"].as_str().unwrap();
        for needle in ["HVAC load schedule", "Btu/hr", "kVA", "Plumbing and drainage", "COL-1", "FND-1", "TB-1", "Bar bending schedule"] {
            assert!(html.contains(needle), "{needle}");
        }
        assert_eq!(h["name"], "engineering-report.pdf");
        let ar = app.handle("report_html", &json!({})).unwrap();
        assert!(ar["html"].as_str().unwrap().contains("dir=\"rtl\""));
        let x = app.handle("report_xlsx", &json!({ "lang": "en" })).unwrap();
        let bytes = nnx_core::xlsx::base64_decode(x["xlsx_b64"].as_str().unwrap()).unwrap();
        assert_eq!(&bytes[..2], b"PK");
        assert!(bytes.len() > 3000);
        // With nothing entered there is nothing to report, and it says what to enter.
        let mut bare = started_with_project();
        assert!(bare.handle("report_html", &json!({})).unwrap_err().contains("enter the MEP inputs"));
    }

    // ------------------------------------------------------------------ presets and reinforcement design

    #[test]
    fn a_climate_zone_preset_fills_the_insulation_limits_in_one_undoable_step_and_the_thermal_check_cites_the_page() {
        let mut app = started_with_project();
        let p = app.handle("presets", &json!({})).unwrap();
        assert_eq!(p["zones"].as_array().unwrap().len(), 3);
        assert!(p["cities"].as_array().unwrap().iter().any(|c| c["en"] == "Jubail" && c["zone"] == 1));
        assert_eq!(p["concrete"]["fc_min_mpa"], 20.0);
        // The project's climate zone is free text ("zone-a" here), so the thermal check has nothing to compare against.
        let before = app.handle("sbc_check", &json!({})).unwrap();
        assert!(before["skipped"].as_array().unwrap().iter().any(|s| s["rule"] == "thermal" && s["en"].as_str().unwrap().contains("Set the climate zone")));
        app.handle("mep", &json!({ "inputs": mep_inputs(), "apply": false })).unwrap();
        let h = hash(&app);
        let r = app.handle("zone_preset", &json!({ "zone": "Jubail" })).unwrap();
        assert_eq!(r["zone"], 1);
        let proj = app.project().unwrap();
        assert_eq!(proj.meta.climate_zone.as_deref(), Some("Jubail"));
        let limits = &proj.mep.as_ref().unwrap().insulation.as_ref().unwrap().u_limits;
        assert_eq!((limits["wall"], limits["roof"]), (0.403, 0.272));
        // The assemblies of the fixture (wall U 0.607, roof U 0.575) and its glazing U 3.0 all exceed zone 1: warnings with the page.
        let rules: Vec<(String, String, String)> = r["sbc"]["violations"].as_array().unwrap().iter().filter(|v| v["rule"].as_str().unwrap().starts_with("thermal")).map(|v| (v["rule"].as_str().unwrap().into(), v["severity"].as_str().unwrap().into(), v["source"].as_str().unwrap().into())).collect();
        for rule in ["thermal.wall.u", "thermal.roof.u", "thermal.window.u"] {
            let found = rules.iter().find(|x| x.0 == rule).unwrap_or_else(|| panic!("{rule} in {rules:?}"));
            assert_eq!(found.1, "warning", "secondary values never refuse an edit");
            assert!(found.2.contains("p.172") && found.2.contains("secondary"), "{}", found.2);
        }
        assert!(!r["sbc"]["skipped"].as_array().unwrap().iter().any(|s| s["rule"] == "thermal"));
        // One undo takes the zone and the limits back out.
        app.handle("undo", &json!({})).unwrap();
        assert_ne!(app.project().unwrap().meta.climate_zone.as_deref(), Some("Jubail"));
        assert!(app.project().unwrap().mep.unwrap().insulation.unwrap().u_limits.get("wall") != Some(&0.403));
        assert_eq!(hash(&app), h, "the undo returned the model to exactly what it was");
        // A zone that does not exist is refused by name and changes nothing.
        let h2 = hash(&app);
        assert!(app.handle("zone_preset", &json!({ "zone": "Atlantis" })).unwrap_err().contains("not a zone"));
        assert!(app.handle("zone_preset", &json!({})).unwrap_err().contains("zone is required"));
        assert_eq!(hash(&app), h2);
        // Zone 3 has looser limits: the same assemblies are checked against 0.511 and 0.34.
        let r3 = app.handle("zone_preset", &json!({ "zone": 3 })).unwrap();
        let wall = r3["sbc"]["violations"].as_array().unwrap().iter().find(|v| v["rule"] == "thermal.wall.u").unwrap().clone();
        assert_eq!(wall["limit"], 0.511);
    }

    #[test]
    fn the_sizing_command_designs_the_bars_from_your_parameters_where_no_template_is_given() {
        let mut app = sized_app();
        let mut inp = sizing_inputs();
        inp["rebar"].as_object_mut().unwrap().remove("column");
        inp["rebar"].as_object_mut().unwrap().remove("footing");
        inp["rebar"].as_object_mut().unwrap().remove("beam");
        inp["rebar"]["design"] = json!({
            "fc_mpa": 25, "fy_mpa": 420, "bar_dias_mm": [12, 16, 20], "min_clear_spacing_mm": 25, "load_factor": 1.4,
            "exposure": { "column": "protected", "footing": "ground", "beam": "protected", "tie_beam": "ground" },
            "column": { "ratio_min": 0.01, "ratio_max": 0.04, "min_bars": 4, "phi": 0.65, "alpha": 0.8, "tie_dia_mm": 8, "tie_spacing_mm": 200 },
            "footing": { "ratio_min": 0.0018, "phi_flexure": 0.9, "max_spacing_mm": 300 },
            "beam": { "ratio_min": 0.004, "min_top_bars": 2, "stirrup_dia_mm": 8, "stirrup_spacing_mm": 150 } });
        let r = app.handle("sizing", &json!({ "inputs": inp })).unwrap();
        let rep = &r["report"];
        assert_eq!(rep["missing"], json!({}), "{}", rep["missing"]);
        let designs = rep["designs"].as_array().unwrap();
        assert!(designs.iter().any(|d| d["kind"] == "Column") && designs.iter().any(|d| d["kind"] == "Footing") && designs.iter().any(|d| d["kind"] == "TieBeam"));
        assert!(designs.iter().all(|d| d["bars"].as_str().unwrap().contains("Dia") && d["provided_mm2"].as_f64().unwrap() >= d["required_mm2"].as_f64().unwrap() - 1e-6 || d["status"] != "ok"));
        assert!(rep["bars"].as_array().unwrap().iter().all(|b| b["source"] == "design"));
        assert!(rep["steel_total_kg"].as_f64().unwrap() > 0.0);
        assert!(r["bbs_csv"].as_str().unwrap().contains(",design"));
        // Leave out the strength of the bars: it is named, and the design is skipped, not guessed.
        inp["rebar"]["design"].as_object_mut().unwrap().remove("fy_mpa");
        let r2 = app.handle("sizing", &json!({ "inputs": inp })).unwrap();
        assert!(r2["report"]["missing"]["design"].as_array().unwrap().iter().any(|m| m == "design.fy_mpa"));
        assert!(r2["report"]["designs"].as_array().unwrap().is_empty());
    }

    #[test]
    fn thermal_and_unread_limits_are_listed_as_not_checked() {
        let mut app = started_with_project();
        let r = app.handle("sbc_check", &json!({})).unwrap();
        let skipped: Vec<&str> = r["skipped"].as_array().unwrap().iter().map(|s| s["rule"].as_str().unwrap()).collect();
        assert!(skipped.contains(&"thermal"), "{skipped:?}");
        assert!(!skipped.contains(&"room_min_area_m2.majlis"), "a value that was read is checked, not skipped");
        assert!(r["checked"].as_u64().unwrap() > 10, "the generated rooms were compared with the minimums");
    }

    #[test]
    fn bad_input_is_an_error_message_not_a_crash() {
        let mut app = App::default();
        assert!(app.handle("generate", &json!({})).is_err());
        let mut b = brief();
        b["setback_side_m"] = json!(20.0);
        assert!(app.handle("generate", &json!({ "brief": b })).unwrap_err().contains("no buildable area"));
        let mut b = brief();
        b["entry"] = json!("up");
        assert!(app.handle("generate", &json!({ "brief": b })).unwrap_err().contains("entry"));
        assert!(app.handle("qto", &json!({})).is_err());
        assert!(app.handle("nope", &json!({})).is_err());
    }
    #[test]
    fn the_finish_choice_is_one_undoable_edit_that_reaches_the_3d_scene_and_the_quantities() {
        let mut app = started();
        let r = app.handle("finishes", &json!({})).unwrap();
        assert_eq!(r["catalog"].as_array().unwrap().len(), 8);
        assert!(r["report"]["lines"].as_array().unwrap().is_empty());
        assert!(app.handle("finishes_set", &json!({ "finishes": { "facade": "gold" } })).is_err());
        let set = json!({ "finishes": { "facade": "travertine", "floor": "marble" } });
        let r = app.handle("finishes_set", &set).unwrap();
        assert_eq!(r["changed"], true);
        let lines = r["report"]["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l["rate"].is_null()), "no price without the user's rate");
        assert!(app.handle("finishes_set", &set).unwrap()["changed"] == false);
        // The 3D scene carries the colours, and the quantity table the two finish rows.
        let scene = app.handle("model3d", &json!({})).unwrap();
        assert!(scene["finishes"]["facade"].is_array() && scene["finishes"]["floor"].is_array());
        let q = app.handle("qto", &json!({})).unwrap();
        let codes: Vec<&str> = q["items"].as_array().unwrap().iter().filter_map(|i| i["code"].as_str()).collect();
        assert!(codes.contains(&"FIN.facade.travertine") && codes.contains(&"FIN.floor.marble"), "{codes:?}");
        // A rate makes the row priced; swapping the finish moves to the other code and the old rate no longer applies.
        let rate = json!({ "finishes": { "facade": "travertine", "floor": "marble" }, "rates": { "FIN.floor.marble": 10.0, "FIN.facade.travertine": 2.0 } });
        let r = app.handle("finishes_set", &rate).unwrap();
        assert!(r["report"]["total"].as_f64().unwrap() > 0.0);
        let r = app.handle("finishes_set", &json!({ "finishes": { "facade": "riyadh_stone", "floor": "marble" } })).unwrap();
        assert!(r["report"]["total"].is_null());
        assert!(app.handle("finishes_set", &json!({ "finishes": {}, "rates": { "FIN.x.y": 1.0 } })).is_err());
        // Undo goes back one step at a time.
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(app.handle("finishes", &json!({})).unwrap()["chosen"]["facade"], "travertine");
        // A finish never becomes a scheduled activity.
        if let Ok(s) = app.handle("schedule", &json!({})) {
            assert!(!s.to_string().contains("FIN."));
        }
    }

    #[test]
    fn change_log_lists_committed_steps_read_only_and_refuses_a_bad_limit() {
        assert!(App::default().handle("change_log", &json!({})).is_err());
        let mut app = started();
        let before = app.handle("change_log", &json!({})).unwrap();
        let n = before["log"]["entries"].as_array().unwrap().len();
        assert!(app.handle("change_log", &json!({ "limit": 0 })).unwrap_err().contains("at least 1"));
        assert!(app.handle("change_log", &json!({ "limit": "x" })).is_err());
        // Reading the log changes nothing: it is not itself a step.
        assert_eq!(app.handle("change_log", &json!({})).unwrap()["log"]["entries"].as_array().unwrap().len(), n);
        let ph = json!({ "photos": [{ "id": "p1", "file": "p1.jpg", "taken": "2026-03-01", "elements": ["COL-1"] }] });
        app.handle("site_photos_set", &ph).unwrap();
        let after = app.handle("change_log", &json!({})).unwrap();
        let e = after["log"]["entries"].as_array().unwrap();
        assert_eq!(e.len(), n + 1);
        assert_eq!(e.last().unwrap()["label"], "Site photos");
        assert_eq!(e.last().unwrap()["origin"], "user");
        assert_eq!(app.handle("change_log", &json!({ "limit": 1 })).unwrap()["log"]["entries"].as_array().unwrap().len(), 1);
        assert!(after["csv"].as_str().unwrap().contains("Site photos"));
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(app.handle("change_log", &json!({})).unwrap()["log"]["entries"].as_array().unwrap().len(), n);
    }

    #[test]
    fn site_photos_are_linked_to_existing_elements_stored_as_one_undoable_edit_and_reported_per_element() {
        let mut app = started();
        assert!(App::default().handle("site_photos", &json!({})).is_err());
        assert!(app.handle("site_photos_set", &json!({})).is_err());
        let ph = |id: &str, date: &str, el: &str, pct: f64| json!({ "id": id, "file": format!("{id}.jpg"), "taken": date, "elements": [el], "observedPct": pct });
        // A reference that matches no element is refused by name and nothing is stored.
        let bad = json!({ "photos": [ph("p1", "2026-03-01", "NOPE-9", 10.0)] });
        assert!(app.handle("site_photos_set", &bad).unwrap_err().contains("NOPE-9"));
        assert!(app.handle("site_photos", &json!({})).unwrap()["inputs"]["photos"].as_array().unwrap().is_empty());
        let bad_date = json!({ "photos": [ph("p1", "2026-02-30", "COL-1", 10.0)] });
        assert!(app.handle("site_photos_set", &bad_date).unwrap_err().contains("not a date"));
        let ok = json!({ "photos": [ph("p2", "2026-03-10", "COL-1", 60.0), ph("p1", "2026-03-01", "COL-1", 30.0)] });
        let r = app.handle("site_photos_set", &ok).unwrap();
        assert_eq!(r["changed"], true);
        let el = &r["report"]["elements"][0];
        assert_eq!(el["name"], "COL-1");
        assert_eq!(el["latest_photo"], "p2");
        assert_eq!(el["latest_observed_pct"], 60.0);
        assert_eq!(r["report"]["first_taken"], "2026-03-01");
        assert!(r["csv"].as_str().unwrap().lines().count() == 3);
        assert_eq!(app.handle("site_photos_set", &ok).unwrap()["changed"], false);
        app.handle("undo", &json!({})).unwrap();
        assert!(app.handle("site_photos", &json!({})).unwrap()["inputs"]["photos"].as_array().unwrap().is_empty());
    }

    #[test]
    fn as_built_rows_are_stored_in_the_project_and_deviations_are_judged_by_the_users_tolerance() {
        let mut app = started();
        let r = app.handle("asbuilt", &json!({})).unwrap();
        assert!(r["report"]["deviations"].as_array().unwrap().is_empty());
        assert!(app.handle("asbuilt_set", &json!({ "csv": "name,x_mm\nCOL-1,abc\n" })).unwrap_err().contains("line 2"));
        assert!(app.handle("asbuilt_set", &json!({ "toleranceMm": -1.0 })).is_err());
        let r = app.handle("asbuilt_set", &json!({ "csv": "name,x_mm,y_mm\nCOL-1,10,10\nNOPE-1,1,1\n", "source": "total station" })).unwrap();
        assert_eq!(r["changed"], true);
        assert_eq!(r["report"]["unmatched"], json!(["NOPE-1"]));
        assert!(r["report"]["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("no tolerance")));
        // The column's design position is far from (10, 10) mm, so the deviation is large and, with a tolerance, over it.
        let r = app.handle("asbuilt_set", &json!({ "toleranceMm": 5.0 })).unwrap();
        assert!(r["report"]["over"].as_u64().unwrap() >= 1 && r["inputs"]["rows"].as_array().unwrap().len() == 2);
        // The 3D scene colours the elements by deviation, and the plan carries the markers.
        let scene = app.handle("model3d", &json!({})).unwrap();
        assert!(!scene["deviations"].as_object().unwrap().is_empty());
        let svg = app.handle("asbuilt_svg", &json!({})).unwrap();
        assert!(svg["svg"].as_str().unwrap().contains("class=\"asbuilt\""));
        app.handle("undo", &json!({})).unwrap();
        assert!(app.handle("asbuilt", &json!({})).unwrap()["inputs"]["toleranceMm"].is_null());
        assert!(App::default().handle("asbuilt", &json!({})).is_err());
    }

    #[test]
    fn the_stl_and_the_standalone_tour_come_from_the_open_model() {
        let mut app = started();
        assert!(app.handle("stl_export", &json!({})).unwrap_err().contains("scale"));
        assert!(app.handle("stl_export", &json!({ "scale": 100.0, "level": 99 })).is_err());
        let r = app.handle("stl_export", &json!({ "scale": 100.0, "minFeatureMm": 1.0 })).unwrap();
        assert_eq!(r["closed"], true);
        assert!(r["name"].as_str().unwrap().ends_with("1to100.stl"));
        let n = r["report"]["triangles"].as_u64().unwrap();
        assert!(n > 0 && n.is_multiple_of(12));
        assert!(!r["stl_b64"].as_str().unwrap().is_empty());
        let t = app.handle("tour_export", &json!({ "lang": "en" })).unwrap();
        let html = t["html"].as_str().unwrap();
        assert!(html.contains("<canvas id=\"gl\">") && html.contains("\"k\":\"Wall\"") && !html.contains("src=\"http"));
        assert_eq!(t["name"], "tour.html");
        let c = app.handle("client_view_export", &json!({ "lang": "en" })).unwrap();
        assert_eq!(c["name"], "client-view.html");
        let ch = c["html"].as_str().unwrap();
        assert!(ch.contains("\"client\":true") && !ch.contains("\"k\":\"Column\"") && ch.contains("\"k\":\"Wall\""));
        assert!(App::default().handle("client_view_export", &json!({})).is_err());
        assert!(App::default().handle("tour_export", &json!({})).is_err() && App::default().handle("stl_export", &json!({ "scale": 100.0 })).is_err());
    }

    // ------------------------------------------------------------------ Milestone 6: execution commands

    #[test]
    fn progress_certificates_are_stored_apart_from_the_milestones_and_report_the_payable_amount() {
        let mut app = complete_app();
        let pay = json!({ "payment": { "contractValue": 1000.0, "contractSource": "synthetic contract", "milestones": [{ "id": "m1", "name": "Footings", "link": { "kind": "task", "key": "footing_pour" }, "trigger": "finish", "pct": 100.0 }] } });
        app.handle("payment_set", &pay).unwrap();
        let prog = json!({ "progress": { "retentionPct": 5.0, "retentionSource": "synthetic clause", "certificates": [{ "id": "c1", "date": "2026-02-01", "reference": "report 1", "lines": [{ "milestone": "m1", "completionPct": 40.0 }] }] } });
        let r = app.handle("progress_payments_set", &prog).unwrap();
        assert_eq!(r["changed"], true);
        // 40% of 1000 = 400, retention 5% = 20, payable 380.
        assert_eq!(r["report"]["certificates"][0]["payable_now"], 380.0);
        assert!(r["csv"].as_str().unwrap().contains("TOTAL"));
        // The milestones survive, and the stored certificates are read back.
        assert_eq!(app.handle("payment", &json!({})).unwrap()["report"]["milestones"][0]["id"], "m1");
        assert_eq!(app.handle("progress_payments", &json!({})).unwrap()["report"]["total_paid_net"], 380.0);
        // An unknown milestone is refused by name and nothing is stored.
        let bad = json!({ "progress": { "certificates": [{ "id": "c1", "date": "2026-02-01", "reference": "r", "lines": [{ "milestone": "zz", "completionPct": 1.0 }] }] } });
        assert!(app.handle("progress_payments_set", &bad).unwrap_err().contains("not a payment milestone"));
        assert_eq!(app.handle("progress_payments", &json!({})).unwrap()["report"]["total_paid_net"], 380.0);
        assert!(app.handle("progress_payments_set", &json!({})).is_err());
    }

    #[test]
    fn the_schedule_setup_is_kept_in_the_project_and_the_m6_features_read_it() {
        let mut app = complete_app();
        // Before any schedule: nothing has dates, and the reason says so.
        let pay = json!({ "payment": { "contractValue": 1000.0, "contractSource": "synthetic contract", "milestones": [{ "id": "m1", "name": "Footings", "link": { "kind": "task", "key": "footing_pour" }, "trigger": "finish", "pct": 100.0 }] } });
        let r = app.handle("payment_set", &pay).unwrap();
        assert_eq!(r["schedule_ready"], false);
        assert!(r["schedule_note"].as_str().unwrap().contains("compute the schedule"));
        assert!(r["report"]["milestones"][0]["date"].is_null());

        // Computing the schedule stores its setup (one undoable step, a no-op the second time).
        let rates = all_rates(&mut app);
        let args = schedule_args(rates);
        let s = app.handle("schedule", &args).unwrap();
        assert_eq!(s["ok"], true);
        let pour = s["tasks"].as_array().unwrap().iter().find(|t| t["key"] == "footing_pour").unwrap();
        let stored = app.handle("exec", &json!({})).unwrap();
        assert_eq!(stored["inputs"]["schedule"]["start"], "2026-11-15");
        let steps = app.doc.as_ref().unwrap().undo_label().map(str::to_string);
        app.handle("schedule", &args).unwrap();
        assert_eq!(app.doc.as_ref().unwrap().undo_label().map(str::to_string), steps, "an unchanged setup commits nothing");

        // The payment milestone now carries the schedule's own date.
        let r = app.handle("payment", &json!({})).unwrap();
        assert_eq!(r["schedule_ready"], true);
        assert_eq!(r["report"]["milestones"][0]["date"], pour["finish"]);
        assert_eq!(r["report"]["status"], "balanced");
        // A refused schedule input is not stored.
        assert!(app.handle("schedule", &json!({ "rates": {}, "start": "nope", "workdays": [true, true, true, true, true, true, true] })).is_err());
        assert_eq!(app.handle("exec", &json!({})).unwrap()["inputs"]["schedule"]["start"], "2026-11-15");
    }

    #[test]
    fn material_requests_split_the_boq_materials_by_schedule_stage_and_change_nothing() {
        let mut app = complete_app();
        // Without a schedule there are no stages to order by: refused with the reason, nothing invented.
        assert!(app.handle("material_requests", &json!({})).is_err());
        let rates = all_rates(&mut app);
        app.handle("schedule", &schedule_args(rates)).unwrap();
        // SYNTHETIC consumption factors; not real material data.
        let mats: serde_json::Map<String, Value> = ["SLAB.*", "COLUMN.*", "FOOTING.*", "BEAM.*", "WALL.*", "PLASTER.*", "FLOOR.*"]
            .iter()
            .map(|k| (k.to_string(), json!([{ "material": "cement", "unit": "bag", "per_unit": 7, "source": "test" }])))
            .collect();
        let boq_args = json!({ "inputs": { "project": { "name": "t", "currency": "XXX" }, "materials": mats }, "lang": "en" });
        app.handle("boq", &boq_args).unwrap();
        let hash = |a: &App| format!("{:?}", a.doc.as_ref().unwrap().undo_label());
        let before = hash(&app);
        let r = app.handle("material_requests", &json!({})).unwrap();
        assert_eq!(hash(&app), before, "the request list changes nothing");
        let stages = r["report"]["stages"].as_array().unwrap();
        assert!(!stages.is_empty(), "{r}");
        let total = |v: &Value| v.as_array().unwrap().iter().filter(|m| m["material"] == "cement").map(|m| m["quantity"].as_f64().unwrap()).sum::<f64>();
        let staged: f64 = stages.iter().map(|s| total(&s["requests"])).sum();
        let whole = total(&app.handle("boq", &boq_args).unwrap()["report"]["materials"]);
        assert!(whole > 0.0, "the BOQ has cement");
        assert!((staged + total(&r["report"]["unassigned"]) - whole).abs() < 0.01, "stages + unassigned = the BOQ material list");
        assert!(r["csv"].as_str().unwrap().contains("stage,stage_start,material"));
        // A lead time for a material nothing needs, or without its source, is refused by name.
        let bad = app.handle("material_requests", &json!({ "inputs": { "lead": { "nonesuch": { "days": 5, "source": "s" } } } })).unwrap_err();
        assert!(bad.contains("nonesuch"));
        assert!(app.handle("material_requests", &json!({ "inputs": { "lead": { "cement": { "days": 5, "source": "" } } } })).is_err());
        assert!(app.handle("material_requests", &json!({ "inputs": { "nope": 1 } })).is_err());
        let ok = app.handle("material_requests", &json!({ "inputs": { "as_of": "2026-11-20", "lead": { "cement": { "days": 10, "source": "synthetic quotation" } } } }));
        assert!(ok.is_ok(), "{ok:?}");
        assert!(App::default().handle("material_requests", &json!({})).is_err());
    }

    #[test]
    fn cost_tiers_procurement_qaqc_roi_and_the_tender_pack_are_stored_undoable_and_exported() {
        let mut app = complete_app();
        let rates = all_rates(&mut app);
        app.handle("schedule", &schedule_args(rates)).unwrap();

        // Cost tiers: a rate needs its source; the tier total follows the model's areas.
        let bad = json!({ "cost": { "selected": "shell_with_materials", "lines": [{ "tier": "shell_with_materials", "phase": "Substructure", "basis": { "kind": "gross_area" }, "rate": 100.0, "source": "" }] } });
        assert!(app.handle("cost_set", &bad).is_err());
        let cost = json!({ "cost": { "selected": "shell_with_materials", "currency": "SAR", "lines": [
            { "tier": "shell_with_materials", "phase": "Substructure", "basis": { "kind": "gross_area" }, "rate": 100.0, "source": "synthetic" },
            { "tier": "shell_with_materials", "phase": "Superstructure", "basis": { "kind": "lump" }, "rate": 5000.0, "source": "synthetic" } ] } });
        let r = app.handle("cost_set", &cost).unwrap();
        assert_eq!(r["changed"], true);
        let tier = &r["report"]["tiers"][0];
        assert_eq!(tier["id"], "shell_with_materials");
        assert_eq!(tier["complete"], true);
        let gross = r["report"]["gross_area_m2"].as_f64().unwrap();
        assert!((tier["total"].as_f64().unwrap() - (gross * 100.0 + 5000.0)).abs() < 0.01);
        assert_eq!(app.handle("cost_set", &cost).unwrap()["changed"], false);

        // Procurement: order date from the stored schedule; a missing lead time stays undated.
        let proc = json!({ "procurement": { "asOf": "2026-11-20", "items": [
            { "id": "win", "name": "Windows", "category": "windows", "neededFor": { "kind": "task", "key": "doors_windows" }, "neededAt": "start", "leadDays": 60, "bufferDays": 7, "source": "synthetic quotation" },
            { "id": "hv", "name": "HVAC units", "category": "hvac", "neededFor": { "kind": "task", "key": "duct_runs" }, "neededAt": "start" } ] } });
        let r = app.handle("procure_set", &proc).unwrap();
        assert_eq!(r["schedule_ready"], true);
        let items = r["report"]["items"].as_array().unwrap();
        let win = items.iter().find(|i| i["id"] == "win").unwrap();
        assert!(win["order_by"].is_string() && win["needed_on"].is_string());
        assert_eq!(items.iter().find(|i| i["id"] == "hv").unwrap()["status"], "unknown");
        assert!(r["csv"].as_str().unwrap().contains("Windows"));

        // QA/QC: stored per check, exported as a printable report and a workbook.
        let qa = json!({ "qaqc": { "records": [{ "item": "excavation.setout", "status": "pass", "by": "Eng. Test", "date": "2026-11-20" }], "customItems": [] }, "lang": "en" });
        let r = app.handle("qaqc_set", &qa).unwrap();
        let ex = r["report"]["stages"].as_array().unwrap().iter().find(|s| s["id"] == "excavation").unwrap();
        assert_eq!(ex["state"], "in_progress");
        assert!(ex["due"].is_string(), "the excavation stage is due before the footing formwork");
        assert!(r["html"].as_str().unwrap().contains("Eng. Test") && !r["xlsx_b64"].as_str().unwrap().is_empty());
        assert!(app.handle("qaqc_set", &json!({ "qaqc": { "records": [{ "item": "excavation.setout", "status": "pass" }] } })).is_err());

        // Apartment ROI: off for a villa unless enabled; with entered figures the yield follows.
        let off = app.handle("roi", &json!({})).unwrap();
        assert_eq!(off["report"]["active"], false);
        let roi = json!({ "roi": { "enabled": true, "unitTypes": [{ "name": "2BR", "count": 4, "annualRent": 40000.0, "rentSource": "synthetic" }], "capexBasis": "manual", "capexManual": 1600000.0, "capexManualSource": "synthetic" } });
        let r = app.handle("roi_set", &roi).unwrap();
        assert_eq!(r["report"]["total_units"], 4);
        assert!((r["report"]["gross_yield_pct"].as_f64().unwrap() - 10.0).abs() < 1e-9);
        assert!((r["report"]["payback_years"].as_f64().unwrap() - 10.0).abs() < 1e-9);

        // Tender pack: PDF html and XLSX, with the tier, the gaps and no invented price.
        let t = json!({ "tender": { "scope": "Synthetic scope", "notes": [{ "title": "General", "text": "Synthetic condition" }], "bidDeadline": "2026-12-31" }, "lang": "en" });
        let r = app.handle("tender_set", &t).unwrap();
        let html = r["html"].as_str().unwrap();
        assert!(html.contains("Synthetic scope") && html.contains("Synthetic condition") && html.contains("PARTIAL") && html.contains("Shell with materials"));
        assert!(r["pack"]["missing"].as_array().unwrap().iter().any(|m| m.as_str().unwrap().starts_with("rate of ")));
        assert!(!r["xlsx_b64"].as_str().unwrap().is_empty());
        assert!(app.handle("tender_set", &json!({ "tender": { "bidDeadline": "soon" } })).is_err());

        // Everything is one undo step per edit, in reverse order, and the file model carries it.
        assert_eq!(app.handle("exec", &json!({})).unwrap()["inputs"]["tender"]["scope"], "Synthetic scope");
        app.handle("undo", &json!({})).unwrap();
        assert!(app.handle("exec", &json!({})).unwrap()["inputs"]["tender"]["scope"].is_null());
        assert_eq!(app.handle("exec", &json!({})).unwrap()["inputs"]["roi"]["unitTypes"].as_array().unwrap().len(), 1);
        assert!(App::default().handle("exec", &json!({})).is_err() && App::default().handle("cost", &json!({})).is_err() && App::default().handle("tender", &json!({})).is_err());
    }

    // ------------------------------------------------------------------ Milestone 7: outliner, snapshots, overlays

    #[test]
    fn the_outline_lists_levels_spaces_structural_mep_and_the_schedule_with_the_elements_each_names() {
        let mut app = complete_app();
        let r = app.handle("outline", &json!({})).unwrap();
        let tree = &r["tree"];
        assert_eq!(tree["kind"], "Site");
        let level = &tree["children"][0];
        assert_eq!(level["kind"], "Level");
        let kinds: Vec<&str> = level["children"].as_array().unwrap().iter().map(|c| c["kind"].as_str().unwrap()).collect();
        for k in ["Spaces", "Envelope", "Structural", "MEP"] {
            assert!(kinds.contains(&k), "{kinds:?}");
        }
        // Structural leaves carry their schedule names (COL-1, FND-1 ...).
        let st = level["children"].as_array().unwrap().iter().find(|c| c["kind"] == "Structural").unwrap();
        assert!(st["children"].as_array().unwrap().iter().any(|c| c["en"].as_str().unwrap().starts_with("COL-")), "{st}");
        // Every id in the tree is an element of the open model.
        let model = &app.doc.as_ref().unwrap().model;
        assert!(tree["ids"].as_array().unwrap().iter().all(|id| model.get(&id.as_str().unwrap().parse().unwrap()).is_some()));
        // Before a schedule exists the node says so; after, phases and tasks appear.
        let exe = |t: &Value| t["children"].as_array().unwrap().iter().find(|c| c["kind"] == "Execution").unwrap().clone();
        assert!(exe(tree)["children"][0]["en"].as_str().unwrap().contains("not computed"));
        let rates = all_rates(&mut app);
        app.handle("schedule", &schedule_args(rates)).unwrap();
        let r = app.handle("outline", &json!({})).unwrap();
        let sched = exe(&r["tree"])["children"][0].clone();
        assert_eq!(sched["goto"]["tab"], "sched");
        assert_eq!(sched["children"][0]["en"], "Substructure");
        assert!(sched["children"][0]["children"].as_array().unwrap().iter().any(|t| !t["ids"].as_array().unwrap().is_empty()), "a task names the elements it builds");
        assert!(App::default().handle("outline", &json!({})).is_err());
    }

    #[test]
    fn a_snapshot_is_saved_in_the_project_compared_with_the_current_model_drawn_and_undone() {
        let mut app = sized_app();
        assert!(app.handle("snapshot_save", &json!({ "name": "  " })).is_err());
        let r = app.handle("snapshot_save", &json!({ "name": "Rev A", "note": "before sizing" })).unwrap();
        assert_eq!(r["snapshots"][0]["id"], "S1");
        assert!(r["snapshots"][0]["elements"].as_u64().unwrap() > 0);
        assert!(app.handle("snapshot_save", &json!({ "name": "rev a" })).unwrap_err().contains("already used"));
        // Nothing changed yet: an empty difference.
        let d = app.handle("snapshot_diff", &json!({ "a": "S1" })).unwrap();
        assert_eq!(d["diff"]["added"], 0);
        assert_eq!(d["diff"]["modified"], 0);
        // The structural schedules add tie beams: they appear as added.
        app.handle("sizing", &json!({ "inputs": sizing_inputs() })).unwrap();
        let d = app.handle("snapshot_diff", &json!({ "a": "S1", "b": "current" })).unwrap();
        assert!(d["diff"]["added"].as_u64().unwrap() > 0, "{d}");
        assert_eq!(d["on_plan"], true);
        assert!(!d["statuses"].as_object().unwrap().is_empty());
        assert!(d["diff"]["elements"].as_array().unwrap().iter().any(|e| e["kind"] == "TieBeam" && e["status"] == "added"));
        let svg = app.handle("snapshot_svg", &json!({ "a": "S1" })).unwrap();
        assert!(svg["clouds"].as_u64().unwrap() > 0 && svg["svg"].as_str().unwrap().contains("rev-clouds"));
        // Two stored snapshots compare without the current model, and then cannot be drawn on the plan.
        app.handle("snapshot_save", &json!({ "name": "Rev B" })).unwrap();
        let d = app.handle("snapshot_diff", &json!({ "a": "S1", "b": "S2" })).unwrap();
        assert_eq!(d["on_plan"], false);
        assert_eq!(d["diff"]["a"]["name"], "Rev A");
        assert!(d["diff"]["added"].as_u64().unwrap() > 0);
        assert!(app.handle("snapshot_diff", &json!({ "a": "S9" })).is_err());
        // Deleting and undoing are single steps; a snapshot is part of the project.
        app.handle("snapshot_delete", &json!({ "id": "S1" })).unwrap();
        assert_eq!(app.handle("snapshots", &json!({})).unwrap()["snapshots"].as_array().unwrap().len(), 1);
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(app.handle("snapshots", &json!({})).unwrap()["snapshots"].as_array().unwrap().len(), 2);
        assert!(app.handle("snapshot_delete", &json!({ "id": "S9" })).is_err());
        assert!(App::default().handle("snapshot_save", &json!({ "name": "x" })).is_err());
    }

    #[test]
    fn a_snapshot_carries_the_cost_figures_of_its_moment() {
        let mut app = sized_app();
        let inputs = |rate: f64| json!({ "inputs": { "items": { "WALL.EXT.AREA": { "spec": "s", "spec_source": "synthetic", "rate": rate, "rate_source": "synthetic" } } } });
        app.handle("boq", &inputs(10.0)).unwrap();
        app.handle("snapshot_save", &json!({ "name": "Cheap" })).unwrap();
        app.handle("boq", &inputs(15.0)).unwrap();
        let d = app.handle("snapshot_diff", &json!({ "a": "S1" })).unwrap();
        let row = d["diff"]["costs"].as_array().unwrap().iter().find(|c| c["code"] == "WALL.EXT.AREA").expect("the wall row changed");
        let (from, delta) = (row["from"].as_f64().unwrap(), row["delta"].as_f64().unwrap());
        assert!((delta - from * 0.5).abs() < 0.01, "{from} {delta}");
        assert!(d["diff"]["elements"].as_array().unwrap().is_empty(), "only the price changed");
    }

    const OV_BASE: &str = r#"{ "id": "BASE", "edition": "synthetic", "version": "1", "rules": [ { "id": "room.area", "clause": "T-1", "applies_to": { "kind": "Space" }, "check": "area >= param.min", "params": { "min": { "value": 5, "source": "synthetic base" } }, "severity": "warning", "message": "room too small" } ] }"#;
    const OV_PACK: &str = r#"{ "id": "MUNI", "edition": "synthetic", "version": "2", "overrides": { "room.area": { "min": { "value": 30, "source": "Circular 1, art. 5" } } },
        "limits": { "minSetbackFrontM": { "value": 4, "source": "Circular 1, art. 2" } } }"#;

    fn ov_args(pack: &str) -> Value {
        json!({ "name": "Synthetic municipality", "authority": "Synthetic Municipality", "document": "Circular 1", "date": "2027-01-01", "pack": pack, "base": OV_BASE })
    }

    #[test]
    fn an_overlay_is_reviewed_confirmed_stored_layered_into_the_check_and_never_changes_limits_by_itself() {
        let mut app = started();
        // Refused without a source or an authority, by name.
        let bad = OV_PACK.replace("Circular 1, art. 5", "");
        assert!(app.handle("overlay_add", &ov_args(&bad)).unwrap_err().contains("no source"));
        let mut noauth = ov_args(OV_PACK);
        noauth["authority"] = json!("");
        assert!(app.handle("overlay_add", &noauth).unwrap_err().contains("authority"));
        // The review lists the override and the limit change; storing needs the user's confirmation.
        let rv = app.handle("overlay_review", &ov_args(OV_PACK)).unwrap();
        assert_eq!(rv["review"]["overridden"][0]["from"]["value"], 5.0);
        assert_eq!(rv["review"]["overridden"][0]["to"]["value"], 30.0);
        assert_eq!(rv["review"]["limit_changes"].as_array().unwrap().len(), 1);
        assert_eq!(rv["review"]["needs_confirm"], true);
        let r = app.handle("overlay_add", &ov_args(OV_PACK)).unwrap();
        assert_eq!(r["stored"], false);
        assert!(app.handle("overlays", &json!({})).unwrap()["overlays"].as_array().unwrap().is_empty());
        let mut yes = ov_args(OV_PACK);
        yes["confirm"] = json!(true);
        let r = app.handle("overlay_add", &yes).unwrap();
        assert_eq!(r["stored"], true);
        assert_eq!(r["overlays"][0]["label"], "MUNI@synthetic/2");
        assert_eq!(r["overlays"][0]["has_limits"], true);
        // Storing did not touch the project's limits.
        assert!(r["limits"]["minSetbackFrontM"].is_null());
        // The check applies the base pack, then the overlay: the layers are named and the overlay's override is listed.
        let layered = app.handle("check_rules", &json!({ "pack": OV_BASE })).unwrap();
        assert_eq!(layered["layers"], json!(["BASE@synthetic/1", "MUNI@synthetic/2"]));
        assert!(layered["findings"].as_array().unwrap().iter().any(|f| f["rule"] == "room.area" && f["layer"] == "BASE@synthetic/1"), "{layered}");
        assert_eq!(layered["overlays"][1]["overridden"][0]["to"]["value"], 30.0);
        // Disabled, it no longer applies; the base alone passes every room.
        app.handle("overlay_toggle", &json!({ "id": "ov1", "enabled": false })).unwrap();
        let off = app.handle("check_rules", &json!({ "pack": OV_BASE })).unwrap();
        assert_eq!(off["layers"], json!(["BASE@synthetic/1"]));
        assert!(off["findings"].as_array().unwrap().iter().all(|f| f["rule"] != "room.area"));
        // No pack and no enabled overlay is the old error.
        assert!(app.handle("check_rules", &json!({})).is_err());
        // The limits reach the project only by an explicit apply, which is one undoable step with old and new shown.
        let a = app.handle("overlay_apply_limits", &json!({ "id": "ov1" })).unwrap();
        assert_eq!(a["changed"][0]["to"]["value"], 4.0);
        assert_eq!(a["limits"]["minSetbackFrontM"]["source"], "Circular 1, art. 2");
        assert!(app.handle("overlay_apply_limits", &json!({ "id": "ov1" })).is_err());
        app.handle("undo", &json!({})).unwrap();
        assert!(app.handle("overlays", &json!({})).unwrap()["limits"]["minSetbackFrontM"].is_null());
        // Removal is undoable too.
        app.handle("overlay_remove", &json!({ "id": "ov1" })).unwrap();
        assert!(app.handle("overlays", &json!({})).unwrap()["overlays"].as_array().unwrap().is_empty());
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(app.handle("overlays", &json!({})).unwrap()["overlays"].as_array().unwrap().len(), 1);
        assert!(app.handle("overlay_remove", &json!({ "id": "zz" })).is_err());
    }

    const PL_RULES: &str = r#"{ "id": "OFFICE", "edition": "synthetic", "version": "1", "rules": [ { "id": "room.area", "clause": "OS-1", "applies_to": { "kind": "Space" }, "check": "area >= param.min", "params": { "min": { "value": 30, "source": "Synthetic office standard, OS-1" } }, "severity": "warning", "message": "room below office minimum" } ] }"#;

    fn pl_args(rules: Value) -> Value {
        json!({ "plugin": { "id": "office_std", "name": "Synthetic office", "version": "1.0", "publisher": "Synthetic Office", "source": "Synthetic office standard 2027", "rules": rules,
            "templates": [ { "id": "flat", "titleAr": "شقة", "titleEn": "Flat", "buildingType": "apartment", "plotWidthM": 12.0, "plotLengthM": 20.0, "floors": 1,
                "rooms": [ { "key": "bed", "name": "Bed", "func": "bedroom", "areaM2": 5.0, "floor": 1 }, { "key": "hall", "name": "Hall", "func": "hall", "areaM2": 12.0, "floor": 1 } ] } ] } })
    }

    #[test]
    fn an_office_plugin_is_reviewed_stored_layered_into_the_check_and_serves_its_template_without_invented_setbacks() {
        let mut app = started();
        let rules: Value = serde_json::from_str(PL_RULES).unwrap();
        // Refused by name: no publisher.
        let mut bad = pl_args(rules.clone());
        bad["plugin"]["publisher"] = json!("");
        assert!(app.handle("plugin_add", &bad).unwrap_err().contains("publisher"));
        // A rule value without a source is refused through the overlay validation.
        let nosrc: Value = serde_json::from_str(&PL_RULES.replace("Synthetic office standard, OS-1", "")).unwrap();
        assert!(app.handle("plugin_add", &pl_args(nosrc)).unwrap_err().contains("no source"));
        // A rule set that replaces nothing needs no confirmation and is stored.
        let r = app.handle("plugin_add", &pl_args(rules.clone())).unwrap();
        assert_eq!(r["stored"], true);
        assert_eq!(r["plugins"][0]["templates"][0]["address"], "office_std/flat");
        // The enabled plugin's pack is a layer of the check, named by its label.
        let c = app.handle("check_rules", &json!({})).unwrap();
        assert_eq!(c["layers"], json!(["OFFICE@synthetic/1"]));
        assert!(c["findings"].as_array().unwrap().iter().any(|f| f["rule"] == "room.area" && f["layer"] == "OFFICE@synthetic/1"), "{c}");
        // Installing the same id again replaces it, so it needs confirmation.
        let again = app.handle("plugin_add", &pl_args(rules)).unwrap();
        assert_eq!(again["stored"], false);
        assert_eq!(again["review"]["needs_confirm"], true);
        // The template comes back with the SBC notes for the 5 m2 bedroom and the hall, and no setbacks.
        let t = app.handle("plugin_template", &json!({ "address": "office_std/flat" })).unwrap();
        assert!(t["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("below the SBC minimum")), "{t}");
        assert!(t["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("not checked")), "{t}");
        assert!(t["setbacks"].is_null());
        // Disabled: no layer, no template. Removal is undoable.
        app.handle("plugin_toggle", &json!({ "id": "office_std", "enabled": false })).unwrap();
        assert!(app.handle("check_rules", &json!({})).is_err());
        assert!(app.handle("plugin_template", &json!({ "address": "office_std/flat" })).unwrap_err().contains("disabled"));
        app.handle("plugin_remove", &json!({ "id": "office_std" })).unwrap();
        assert!(app.handle("plugins", &json!({})).unwrap()["plugins"].as_array().unwrap().is_empty());
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(app.handle("plugins", &json!({})).unwrap()["plugins"].as_array().unwrap().len(), 1);
        assert!(app.handle("plugin_remove", &json!({ "id": "zz" })).is_err());
    }

    #[test]
    fn a_municipality_entry_needs_sources_is_stored_applied_in_one_undoable_step_and_removed() {
        let mut app = started();
        let good = json!({ "entry": {
            "municipality": "Synthetic Municipality", "zone": "R1", "document": "synthetic circular 7",
            "minSetbackFrontM": { "value": 4.0, "source": "synthetic circular 7, art. 2" },
            "maxCoverageRatio": { "value": 0.5, "source": "synthetic circular 7, art. 3" },
            "maxHeightM": { "value": 12.0, "source": "synthetic circular 7, art. 4" } } });
        // A value without a source is refused by name; nothing is stored.
        let mut nosrc = good.clone();
        nosrc["entry"]["maxHeightM"]["source"] = json!("");
        assert!(app.handle("municipality_add", &nosrc).unwrap_err().contains("height"));
        assert!(app.handle("municipalities", &json!({})).unwrap()["municipalities"].as_array().unwrap().is_empty());
        let r = app.handle("municipality_add", &good).unwrap();
        assert_eq!(r["added"], "mn1");
        assert_eq!(r["municipalities"][0]["set"], 3);
        assert_eq!(r["municipalities"][0]["unset"], json!(["rear setback", "side setback", "floors", "floor-area ratio"]));
        // The same municipality and zone twice is refused.
        assert!(app.handle("municipality_add", &good).is_err());
        // Storing does not touch the project's limits; applying does, once, and names the limit it only stored.
        assert!(r["limits"]["minSetbackFrontM"].is_null());
        let a = app.handle("municipality_apply", &json!({ "id": "mn1" })).unwrap();
        assert_eq!(a["changed"].as_array().unwrap().len(), 2);
        assert_eq!(a["limits"]["minSetbackFrontM"]["source"], "synthetic circular 7, art. 2");
        assert_eq!(a["stored_only"], json!(["height"]));
        assert!(app.handle("municipality_apply", &json!({ "id": "mn1" })).is_err());
        app.handle("undo", &json!({})).unwrap();
        assert!(app.handle("municipalities", &json!({})).unwrap()["limits"]["minSetbackFrontM"].is_null());
        app.handle("municipality_remove", &json!({ "id": "mn1" })).unwrap();
        assert!(app.handle("municipalities", &json!({})).unwrap()["municipalities"].as_array().unwrap().is_empty());
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(app.handle("municipalities", &json!({})).unwrap()["municipalities"].as_array().unwrap().len(), 1);
        assert!(app.handle("municipality_remove", &json!({ "id": "zz" })).is_err());
    }

    #[test]
    fn zoning_check_measures_the_model_against_an_entry_without_applying_it() {
        let mut app = started();
        let entry = |zone: &str, height: f64, floors: f64| json!({ "entry": {
            "municipality": "Synthetic Municipality", "zone": zone, "document": "synthetic circular 9",
            "maxHeightM": { "value": height, "source": "synthetic circular 9, art. 4" },
            "maxFloors": { "value": floors, "source": "synthetic circular 9, art. 5" } } });
        assert!(app.handle("zoning_check", &json!({ "id": "mn1" })).unwrap_err().contains("no municipality entry"));
        app.handle("municipality_add", &entry("R1", 100.0, 50.0)).unwrap();
        app.handle("municipality_add", &entry("R2", 0.5, 1.0)).unwrap();
        let ok = app.handle("zoning_check", &json!({ "id": "mn1" })).unwrap();
        let rows = ok["rows"].as_array().unwrap();
        let row = |rule: &str| rows.iter().find(|r| r["rule"] == rule).unwrap().clone();
        assert!(ok["measured"]["height_m"].as_f64().unwrap() > 0.5, "{ok}");
        assert!(ok["measured"]["floors"].as_u64().unwrap() >= 1);
        assert_eq!(row("height")["status"], "passes");
        assert_eq!(row("floors")["status"], "passes");
        assert_eq!(row("coverage")["status"], "unchecked", "a limit the entry leaves unset is never passed");
        assert_ne!(ok["overall"], "passes");
        let bad = app.handle("zoning_check", &json!({ "id": "mn2" })).unwrap();
        assert_eq!(bad["overall"], "fails");
        assert_eq!(bad["rows"].as_array().unwrap().iter().find(|r| r["rule"] == "height").unwrap()["status"], "fails");
        // Read-only: the project's limits were not touched.
        assert!(app.handle("municipalities", &json!({})).unwrap()["limits"]["maxCoverageRatio"].is_null());
    }

    // ------------------------------------------------------------------ Milestone 8: BIM drafting

    fn model_of(app: &App) -> &nnx_core::Model {
        &app.doc.as_ref().unwrap().model
    }

    /// The first interior wall: horizontal or vertical, shared by two rooms.
    fn partition(app: &App) -> nnx_core::typed::Wall {
        model_of(app).elements.values().filter(|e| e.kind == "Wall").filter_map(|e| e.typed::<nnx_core::typed::Wall>().ok()).find(|w| !w.is_exterior && w.room_b.is_some()).unwrap()
    }

    #[test]
    fn drafting_commands_draw_place_edit_and_delete_as_undoable_steps_that_redraw_the_plan() {
        let mut app = started();
        let f = app.handle("draft_frame", &json!({})).unwrap();
        assert!(f["frame"]["s"].as_f64().unwrap() > 0.0 && f["frame"]["m"].as_f64().unwrap() > 0.0);
        let d = &f["defaults"];
        assert!(d["wall"]["thickness"].as_i64().unwrap() > 0 && d["door"]["width"].as_i64().unwrap() > 0, "learned from the generated plan: {d}");
        // A free wall: the plan is redrawn with it; a size is required; a diagonal is refused.
        let before = model_of(&app).len();
        let hash0 = hash(&app);
        let t = d["wall"]["thickness"].as_i64().unwrap();
        let h = d["wall"]["height"].as_i64().unwrap();
        assert!(app.handle("draft_wall", &json!({ "a": [1000, 1000], "b": [2500, 1000], "height": h })).unwrap_err().contains("thickness"));
        assert!(app.handle("draft_wall", &json!({ "a": [1000, 1000], "b": [2500, 1200], "thickness": t, "height": h })).unwrap_err().contains("horizontal or vertical"));
        let r = app.handle("draft_wall", &json!({ "a": [1000, 1000], "b": [2500, 1000], "thickness": t, "height": h })).unwrap();
        let wall = r["wall"].as_str().unwrap().to_string();
        assert_eq!(model_of(&app).len(), before + 1);
        assert!(r["svg"].as_str().unwrap().starts_with("<svg") && r["frame"].is_object() && r["sbc"].is_object());
        assert_eq!(model_of(&app).get(&wall.parse().unwrap()).unwrap().provenance.source, nnx_core::Source::User);
        // A door in it, found by the placement preview; a second one on top of it is refused.
        let pl = app.handle("draft_place", &json!({ "p": [1750.0, 1000.0], "width": 800, "scale": 0.05 })).unwrap();
        assert_eq!(pl["placement"]["wall"], json!(wall), "{pl}");
        let off = pl["placement"]["offset"].as_i64().unwrap();
        let spec = json!({ "kind": "door", "wall": wall, "offset": off, "width": 800, "height": 2100, "side": 1 });
        let r = app.handle("draft_opening", &spec).unwrap();
        let door = r["opening"].as_str().unwrap().to_string();
        assert!(app.handle("draft_opening", &spec).unwrap_err().contains("overlap"));
        // Properties: read, edit with a check, read again.
        let p = app.handle("draft_props", &json!({ "id": door })).unwrap();
        assert_eq!(p["kind"], "Door");
        let r = app.handle("draft_set", &json!({ "id": door, "key": "width", "value": 900 })).unwrap();
        assert_eq!(r["properties"]["fields"].as_array().unwrap().iter().find(|f| f["key"] == "width").unwrap()["value"], 900);
        assert!(app.handle("draft_set", &json!({ "id": door, "key": "width", "value": 9000 })).unwrap_err().contains("does not fit"));
        // Move the wall with its door, copy it, then delete the copy.
        let r = app.handle("draft_xform", &json!({ "ids": [wall], "xf": { "op": "move", "dx": 0, "dy": 500 } })).unwrap();
        assert_eq!(model_of(&app).get(&wall.parse().unwrap()).unwrap().props["y1"], 1500);
        assert!(r["created"].as_array().unwrap().is_empty());
        let r = app.handle("draft_xform", &json!({ "ids": [wall], "copy": true, "xf": { "op": "move", "dx": 0, "dy": 800 } })).unwrap();
        let copy = r["created"][0].as_str().unwrap().to_string();
        assert_eq!(r["created"].as_array().unwrap().len(), 2);
        let r = app.handle("draft_delete", &json!({ "ids": [copy] })).unwrap();
        assert!(r["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("deleted with the wall")), "its door goes with it, and it says so: {}", r["notes"]);
        // Split and trim.
        let r = app.handle("draft_split", &json!({ "wall": wall, "at": 2350 })).unwrap();
        assert!(r["svg"].is_string());
        // Everything above is undoable, one step each, back to the generated plan.
        let mut steps = 0;
        while app.doc.as_mut().unwrap().undo() {
            steps += 1;
            if hash(&app) == hash0 {
                break;
            }
        }
        assert_eq!(steps, 7, "draw, place, edit, move, copy, delete and split were one step each");
        assert_eq!(hash(&app), hash0, "undo returns to the generated plan exactly");
        assert!(App::default().handle("draft_frame", &json!({})).is_err());
    }

    #[test]
    fn box_selection_snapping_and_the_lock_guard_work_through_the_commands() {
        let mut app = started();
        let p = partition(&app);
        let mid = [(p.x1 + p.x2) as f64 / 2.0, (p.y1 + p.y2) as f64 / 2.0];
        let s = app.handle("draft_snap", &json!({ "p": [mid[0] + 40.0, mid[1] - 30.0], "scale": 0.05 })).unwrap();
        assert!(["midpoint", "endpoint", "intersection"].contains(&s["kind"].as_str().unwrap()), "{s}");
        let b = app.handle("draft_box", &json!({ "box": [p.x1 as f64 - 300.0, p.y1 as f64 - 300.0, p.x2 as f64 + 300.0, p.y2 as f64 + 300.0], "mode": "window" })).unwrap();
        assert!(!b["ids"].as_array().unwrap().is_empty());
        assert!(app.handle("draft_box", &json!({ "box": [0, 0, 1], "mode": "window" })).is_err());
        // A locked wall is not edited by any draft command.
        let wid = model_of(&app).elements.values().find(|e| e.kind == "Wall" && e.props["isExterior"] == json!(false)).unwrap().id.to_string();
        app.handle("lock", &json!({ "kind": "wall", "id": wid, "locked": true })).unwrap();
        assert!(app.handle("draft_xform", &json!({ "ids": [wid], "xf": { "op": "move", "dx": 100, "dy": 0 } })).unwrap_err().contains("locked"));
        assert!(app.handle("draft_delete", &json!({ "ids": [wid] })).unwrap_err().contains("locked"));
        assert!(app.handle("draft_set", &json!({ "id": wid, "key": "thickness", "value": 250 })).unwrap_err().contains("locked"));
        assert!(app.handle("draft_split", &json!({ "wall": wid, "at": 1500 })).unwrap_err().contains("locked"));
    }

    #[test]
    fn what_the_user_drafted_survives_a_partition_move_and_an_opening_in_a_generated_wall_refuses_it() {
        let mut app = started();
        let d = app.handle("draft_frame", &json!({})).unwrap()["defaults"].clone();
        let (t, h) = (d["wall"]["thickness"].as_i64().unwrap(), d["wall"]["height"].as_i64().unwrap());
        // A free wall inside a room.
        let room = model_of(&app).elements.values().filter(|e| e.kind == "Space").filter_map(|e| e.typed::<nnx_core::typed::Space>().ok()).max_by_key(|s| s.w * s.h).unwrap();
        let (x, y) = (room.x + room.w / 2, room.y + room.h / 2);
        let r = app.handle("draft_wall", &json!({ "a": [x, y - 500], "b": [x, y + 500], "thickness": t, "height": h })).unwrap();
        let mine = r["wall"].as_str().unwrap().to_string();
        let p = partition(&app);
        let pid = model_of(&app).elements.values().find(|e| e.kind == "Wall" && e.props["x1"] == p.x1 && e.props["y1"] == p.y1 && e.props["x2"] == p.x2 && e.props["y2"] == p.y2).unwrap().id.to_string();
        let moved = app.handle("move_partition", &json!({ "wall": pid, "delta_mm": 100 }));
        // The generated walls were rebuilt; the user's wall is still there with the same id and source.
        if moved.is_ok() {
            let e = model_of(&app).get(&mine.parse().unwrap()).expect("the drafted wall survived the rebuild");
            assert_eq!(e.provenance.source, nnx_core::Source::User);
        }
        // A door the user placed in a generated wall: the rebuild would leave it without its wall, so it is refused by name.
        let gen_wall = model_of(&app).elements.values().find(|e| e.kind == "Wall" && e.provenance.source == nnx_core::Source::Generated && e.props["length"].as_i64().unwrap_or(0) >= 3000).unwrap().id.to_string();
        let off = model_of(&app).get(&gen_wall.parse().unwrap()).unwrap().props["length"].as_i64().unwrap() / 2;
        let placed = app.handle("draft_opening", &json!({ "kind": "window", "wall": gen_wall, "offset": off, "width": 600, "height": 900, "sill": 900 }));
        if placed.is_ok() {
            let p2 = partition(&app);
            let pid2 = model_of(&app).elements.values().find(|e| e.kind == "Wall" && e.props["x1"] == p2.x1 && e.props["y1"] == p2.y1 && e.props["x2"] == p2.x2 && e.props["y2"] == p2.y2).unwrap().id.to_string();
            let err = app.handle("move_partition", &json!({ "wall": pid2, "delta_mm": 100 })).unwrap_err();
            assert!(err.contains("you placed") && err.contains("delete it"), "{err}");
        }
    }

    // ------------------------------------------------------------------ Milestone 9: blank project, grid lines, dimensions, tags

    fn blank_app() -> App {
        let mut app = App::default();
        app.handle("project_blank", &json!({ "title": "Synthetic" })).unwrap();
        app
    }

    fn draw(app: &mut App, a: [i64; 2], b: [i64; 2]) -> Value {
        app.handle("draft_wall", &json!({ "a": a, "b": b, "thickness": 200, "height": 3000 })).unwrap()
    }

    #[test]
    fn a_blank_project_is_drafted_directly_and_the_fourth_wall_finds_the_room_in_the_same_step() {
        let mut app = blank_app();
        let r = app.handle("project_blank", &json!({})).unwrap();
        assert!(r["svg"].as_str().unwrap().starts_with("<svg") && r["frame"]["s"].as_f64().unwrap() > 0.0);
        assert_eq!(r["levels"]["list"][0]["name"], "LVL-1");
        assert!(r["rooms"].as_array().unwrap().is_empty());
        assert!(r["defaults"]["wall"].is_null(), "nothing to learn from yet: the user enters the sizes");
        assert!(app.handle("draft_wall", &json!({ "a": [1000, 1000], "b": [7000, 1000], "height": 3000 })).unwrap_err().contains("thickness"));
        let hash0 = hash(&app);
        assert!(draw(&mut app, [1000, 1000], [7000, 1000])["rooms_added"].as_array().unwrap().is_empty());
        draw(&mut app, [7000, 1000], [7000, 5000]);
        draw(&mut app, [1000, 5000], [7000, 5000]);
        let last = draw(&mut app, [1000, 1000], [1000, 5000]);
        assert_eq!(last["rooms_added"], json!(["R1"]));
        assert_eq!(last["rooms"][0]["key"], "R1");
        assert_eq!(last["rooms"][0]["area"], 24.0);
        // The starting values now come from the walls just drawn.
        assert_eq!(last["defaults"]["wall"]["thickness"], 200);
        // Everything is one step per edit: the last undo takes the wall AND the room back.
        app.handle("undo", &json!({})).unwrap();
        assert!(app.handle("outline", &json!({})).unwrap()["tree"]["ids"].as_array().unwrap().len() < 6);
        assert!(rooms_of(&app).is_empty());
        app.handle("redo", &json!({})).unwrap();
        assert_eq!(rooms_of(&app).len(), 1);
        // Doors, windows, the quantities, the 3D scene, the outline and the exports all work on the drafted plan.
        let wall = model_of(&app).elements.values().find(|e| e.kind == "Wall" && e.props["y1"] == 1000 && e.props["y2"] == 1000).unwrap().id.to_string();
        app.handle("draft_opening", &json!({ "kind": "door", "wall": wall, "offset": 2000, "width": 900, "height": 2100 })).unwrap();
        app.handle("draft_opening", &json!({ "kind": "window", "wall": wall, "offset": 4500, "width": 1200, "height": 1200, "sill": 900 })).unwrap();
        let q = app.handle("qto", &json!({})).unwrap();
        assert!(q["items"].as_array().unwrap().iter().any(|i| i["code"] == "SPACE.GROSS.AREA"));
        assert!(!app.handle("model3d", &json!({})).unwrap()["boxes"].as_array().unwrap().is_empty());
        assert!(app.handle("export_dxf", &json!({})).unwrap()["dxf"].as_str().unwrap().contains("A-ROOM"));
        assert_ne!(hash(&app), hash0);
        // The file keeps all of it: a save and a reopen give the same model.
        let (model, settings) = (app.model().unwrap().clone(), app.settings());
        let mut again = App::default();
        let opened = again.open(model, &settings).unwrap();
        assert!(opened["svg"].as_str().unwrap().starts_with("<svg"));
        assert_eq!(hash(&again), hash(&app));
    }

    fn rooms_of(app: &App) -> Vec<String> {
        model_of(app).elements.values().filter(|e| e.kind == "Space").map(|e| e.props["requirementKey"].as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn grid_lines_dimensions_and_tags_are_drafted_edited_and_drawn_through_the_commands() {
        let mut app = blank_app();
        for (a, b) in [([1000, 1000], [7000, 1000]), ([7000, 1000], [7000, 5000]), ([1000, 5000], [7000, 5000]), ([1000, 1000], [1000, 5000])] {
            draw(&mut app, a, b);
        }
        // Grid lines: automatic names, a refusal for a clash, and they show on the plan, an elevation and the DXF.
        let g1 = app.handle("draft_grid", &json!({ "a": [1000, 0], "b": [1000, 6000] })).unwrap();
        let g2 = app.handle("draft_grid", &json!({ "a": [7000, 0], "b": [7000, 6000] })).unwrap();
        let g3 = app.handle("draft_grid", &json!({ "a": [0, 1000], "b": [8000, 1000] })).unwrap();
        let name = |r: &Value| model_of(&app).get(&r["grid"].as_str().unwrap().parse().unwrap()).unwrap().props["name"].as_str().unwrap().to_string();
        assert_eq!((name(&g1), name(&g2), name(&g3)), ("A".into(), "B".into(), "1".into()));
        assert!(g1["svg"].as_str().unwrap().contains("class=\"gridline\""));
        assert!(app.handle("draft_grid", &json!({ "a": [3000, 0], "b": [3000, 6000], "name": "a" })).unwrap_err().contains("already named"));
        let el = app.handle("elevation", &json!({ "side": "south" })).unwrap();
        assert_eq!(el["svg"].as_str().unwrap().matches("class=\"grid\"").count(), 2);
        assert!(app.handle("export_dxf", &json!({})).unwrap()["counts"]["A-GRID"].as_u64().unwrap() >= 3);
        // A dimension between the box's left wall and a free wall beyond it, attached to both; it follows when that wall moves.
        let free = draw(&mut app, [9000, 1000], [9000, 5000])["wall"].as_str().unwrap().to_string();
        let left = model_of(&app).elements.values().find(|e| e.kind == "Wall" && e.props["x1"] == 1000 && e.props["x2"] == 1000).unwrap().id.to_string();
        let r = app.handle("draft_dimension", &json!({ "a": [1020, 3000], "b": [8990, 3000], "at": [0, 6000], "refA": left, "refB": free })).unwrap();
        let dim = r["dimension"].as_str().unwrap().to_string();
        assert!(r["svg"].as_str().unwrap().contains(">8000</text>"));
        app.handle("draft_xform", &json!({ "ids": [free], "xf": { "op": "move", "dx": 500, "dy": 0 } })).unwrap();
        let svg = app.handle("svg", &json!({})).unwrap();
        assert!(svg["svg"].as_str().unwrap().contains(">8500</text>"), "the dimension followed the wall");
        let p = app.handle("draft_props", &json!({ "id": dim })).unwrap();
        assert_eq!(p["kind"], "Dimension");
        // Tags: all rooms at once, a second request has nothing to do, the name comes from the room's properties.
        let room = model_of(&app).elements.values().find(|e| e.kind == "Space").unwrap().id.to_string();
        app.handle("draft_set", &json!({ "id": room, "key": "name", "value": "Majlis" })).unwrap();
        let t = app.handle("draft_tag", &json!({})).unwrap();
        let svg = t["svg"].as_str().unwrap();
        assert!(svg.contains("class=\"tag\"") && svg.contains(">Majlis</text>") && svg.contains("m²</text>") && svg.contains(">±0.00 m</text>"));
        assert!(app.handle("draft_tag", &json!({})).unwrap_err().contains("already"));
        assert!(app.handle("draft_rooms", &json!({})).unwrap_err().contains("no new room"));
        // Deleting a wall removes the room and its tag in one step; one undo restores them.
        let top = model_of(&app).elements.values().find(|e| e.kind == "Wall" && e.props["y1"] == 5000 && e.props["y2"] == 5000).unwrap().id.to_string();
        let r = app.handle("draft_delete", &json!({ "ids": [top] })).unwrap();
        assert_eq!(r["rooms_removed"], json!(["R1"]));
        assert!(rooms_of(&app).is_empty() && model_of(&app).elements.values().all(|e| e.kind != "Tag"));
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(rooms_of(&app).len(), 1);
        assert_eq!(model_of(&app).elements.values().filter(|e| e.kind == "Tag").count(), 1);
        // Moving a wall that a layout would have rebuilt is not involved: the partition tools still refuse a blank project politely.
        assert!(app.handle("move_partition", &json!({ "wall": top, "delta_mm": 100 })).is_err());
    }

    // ------------------------------------------------------------------ v2.0.0: separators, offset, assemblies, element tags

    /// A blank project with the 6 x 4 m box (1000..7000, 1000..5000) drawn and its room found.
    fn boxed_app() -> App {
        let mut app = blank_app();
        for (a, b) in [([1000, 1000], [7000, 1000]), ([7000, 1000], [7000, 5000]), ([1000, 5000], [7000, 5000]), ([1000, 1000], [1000, 5000])] {
            draw(&mut app, a, b);
        }
        app
    }

    fn area_of(app: &App, key: &str) -> f64 {
        model_of(app).elements.values().filter(|e| e.kind == "Space").find(|e| e.props["requirementKey"] == key).unwrap().props["area"].as_f64().unwrap()
    }

    #[test]
    fn a_room_separator_divides_an_open_space_into_two_rooms_with_their_own_areas_and_names_without_a_wall() {
        let mut app = boxed_app();
        assert_eq!(rooms_of(&app), vec!["R1"]);
        let walls_before = model_of(&app).elements.values().filter(|e| e.kind == "Wall").count();
        assert!(app.handle("draft_separator", &json!({ "a": [3000, 1000], "b": [3500, 1500] })).unwrap_err().contains("horizontal or vertical"));
        let r = app.handle("draft_separator", &json!({ "a": [3000, 1000], "b": [3000, 5000] })).unwrap();
        assert_eq!(r["rooms_removed"], json!(["R1"]));
        let added: Vec<String> = serde_json::from_value(r["rooms_added"].clone()).unwrap();
        assert_eq!(added.len(), 2);
        let mut areas: Vec<f64> = added.iter().map(|k| area_of(&app, k)).collect();
        areas.sort_by(f64::total_cmp);
        assert_eq!(areas, vec![8.0, 16.0], "2 x 4 m and 4 x 4 m");
        assert_eq!(model_of(&app).elements.values().filter(|e| e.kind == "Wall").count(), walls_before, "no wall was added");
        assert!(r["svg"].as_str().unwrap().contains("class=\"separator\""));
        // The same line twice is refused; the DXF has its own layer; tags name and measure each space.
        assert!(app.handle("draft_separator", &json!({ "a": [3000, 5000], "b": [3000, 1000] })).unwrap_err().contains("already"));
        assert!(app.handle("export_dxf", &json!({})).unwrap()["counts"]["A-ROOM-SEP"].as_u64().unwrap() >= 1);
        let small = model_of(&app).elements.values().find(|e| e.kind == "Space" && e.props["area"] == 8.0).unwrap().id.to_string();
        app.handle("draft_set", &json!({ "id": small, "key": "name", "value": "Majlis" })).unwrap();
        let t = app.handle("draft_tag", &json!({})).unwrap();
        let svg = t["svg"].as_str().unwrap();
        assert!(svg.contains(">Majlis</text>") && svg.contains(">8.00 m²</text>") && svg.contains(">16.00 m²</text>"));
        // Moving the separator re-divides the space in the same step; deleting it joins the two again.
        let sep = r["separator"].as_str().unwrap().to_string();
        let moved = app.handle("draft_xform", &json!({ "ids": [sep], "xf": { "op": "move", "dx": 1000, "dy": 0 } })).unwrap();
        assert_eq!(moved["rooms_added"].as_array().unwrap().len(), 2);
        let mut areas: Vec<f64> = rooms_of(&app).iter().map(|k| area_of(&app, k)).collect();
        areas.sort_by(f64::total_cmp);
        assert_eq!(areas, vec![12.0, 12.0]);
        let del = app.handle("draft_delete", &json!({ "ids": [sep] })).unwrap();
        assert_eq!(del["rooms_added"].as_array().unwrap().len(), 1);
        assert_eq!(rooms_of(&app).len(), 1);
        assert_eq!(area_of(&app, &rooms_of(&app)[0]), 24.0);
        // One undo per edit.
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(rooms_of(&app).len(), 2);
    }

    #[test]
    fn a_separator_across_a_room_of_a_generated_layout_divides_it_in_one_undoable_step_and_removing_it_restores_the_room() {
        let mut app = started();
        let room = model_of(&app).elements.values().filter(|e| e.kind == "Space" && !e.props["requirementKey"].as_str().unwrap().starts_with("stair") && e.props["w"].as_i64().unwrap() >= 2000).min_by_key(|e| e.props["requirementKey"].as_str().unwrap().to_string()).unwrap().clone();
        let key = room.props["requirementKey"].as_str().unwrap().to_string();
        let func = room.props["func"].as_str().unwrap().to_string();
        let (x, y, w, h) = (room.props["x"].as_i64().unwrap(), room.props["y"].as_i64().unwrap(), room.props["w"].as_i64().unwrap(), room.props["h"].as_i64().unwrap());
        let before = rooms_of(&app);
        let hash0 = hash(&app);
        let r = app.handle("draft_separator", &json!({ "a": [x + w / 3, y], "b": [x + w / 3, y + h] })).unwrap();
        let after = rooms_of(&app);
        assert_eq!(after.len(), before.len() + 1, "{}", r["notes"]);
        assert!(r["notes"].as_array().unwrap().iter().any(|s| s.as_str().unwrap().contains("was divided by a room separator")), "{}", r["notes"]);
        // The larger part keeps the key and the function; the other is a generic room named after it; no other room of the layout moved.
        let part = |app: &App, k: &str| model_of(app).elements.values().find(|e| e.kind == "Space" && e.props["requirementKey"] == k).cloned();
        let big = part(&app, &key).expect("the key stays");
        let small = part(&app, &format!("{key}-2")).expect("the other part");
        assert_eq!((big.props["func"].as_str().unwrap(), small.props["func"].as_str().unwrap()), (func.as_str(), "room"));
        assert_eq!((big.props["w"].as_i64().unwrap(), small.props["w"].as_i64().unwrap()), (w - w / 3, w / 3));
        assert_eq!(small.props["targetArea"], small.props["area"], "no target is invented");
        assert_eq!(big.props["area"].as_f64().unwrap() + small.props["area"].as_f64().unwrap(), room.props["area"].as_f64().unwrap());
        for k in &before {
            if *k != key {
                assert!(after.contains(k), "{k} is untouched");
            }
        }
        // Everything that named the room still names a room that exists, and the SBC check and the exports still run.
        let names: std::collections::BTreeSet<String> = after.iter().cloned().collect();
        for e in model_of(&app).elements.values().filter(|e| e.kind == "Window") {
            assert!(names.contains(e.props["room"].as_str().unwrap()), "a window names a room that exists");
        }
        for cmd in ["sbc_check", "qto", "model3d", "svg", "compliance"] {
            assert!(app.handle(cmd, &json!({})).is_ok(), "{cmd} runs on the divided plan");
        }
        assert!(app.handle("export_dxf", &json!({})).unwrap()["counts"]["A-ROOM-SEP"].as_u64().unwrap() >= 1);
        // One undo takes it all back; redo brings it again; deleting the separator gives the layout's room back, unchanged.
        app.handle("undo", &json!({})).unwrap();
        assert_eq!(hash(&app), hash0, "the model is as it was before the separator");
        app.handle("redo", &json!({})).unwrap();
        assert_eq!(rooms_of(&app).len(), before.len() + 1);
        let sep = r["separator"].as_str().unwrap().to_string();
        let del = app.handle("draft_delete", &json!({ "ids": [sep] })).unwrap();
        assert!(del["notes"].as_array().unwrap().iter().any(|s| s.as_str().unwrap().contains("one room again")), "{}", del["notes"]);
        let mut restored = rooms_of(&app);
        let mut original = before.clone();
        restored.sort();
        original.sort();
        assert_eq!(restored, original);
        let back = part(&app, &key).unwrap();
        assert_eq!([back.props["x"].as_i64().unwrap(), back.props["y"].as_i64().unwrap(), back.props["w"].as_i64().unwrap(), back.props["h"].as_i64().unwrap()], [x, y, w, h]);
        assert_eq!(back.props["func"], room.props["func"]);
        assert!(back.props.get("splitFrom").is_none());
    }

    #[test]
    fn a_separator_that_does_not_reach_both_sides_of_a_layout_room_divides_nothing_and_says_why() {
        let mut app = started();
        let room = model_of(&app).elements.values().find(|e| e.kind == "Space" && !e.props["requirementKey"].as_str().unwrap().starts_with("stair") && e.props["w"].as_i64().unwrap() >= 2000 && e.props["h"].as_i64().unwrap() >= 2000).unwrap().clone();
        let (x, y, w, h) = (room.props["x"].as_i64().unwrap(), room.props["y"].as_i64().unwrap(), room.props["w"].as_i64().unwrap(), room.props["h"].as_i64().unwrap());
        let n = rooms_of(&app).len();
        let r = app.handle("draft_separator", &json!({ "a": [x + w / 2, y + h / 4], "b": [x + w / 2, y + h / 2] })).unwrap();
        assert!(r["notes"].as_array().unwrap().iter().any(|s| s.as_str().unwrap().contains("does not reach both sides")), "{}", r["notes"]);
        assert_eq!(rooms_of(&app).len(), n);
    }

    /// The room fields of every window, door and MEP point of the model, by element id.
    fn room_refs(app: &App) -> std::collections::BTreeMap<String, Vec<String>> {
        let mut out = std::collections::BTreeMap::new();
        for e in model_of(app).elements.values() {
            let fields: &[&str] = match e.kind.as_str() {
                "Window" | "MepPoint" => &["room"],
                "Door" => &["from", "to", "opensInto"],
                _ => continue,
            };
            out.insert(e.id.to_string(), fields.iter().map(|f| e.props.get(*f).and_then(Value::as_str).unwrap_or("").to_string()).collect());
        }
        out
    }

    /// A different project from the single-storey one: the generated two-storey villa, divided on its UPPER level.
    #[test]
    fn a_room_of_the_upper_storey_divided_by_a_separator_is_named_tagged_saved_reopened_and_exported_by_its_rooms_name() {
        fn names(r: &Value) -> std::collections::BTreeMap<String, String> {
            r["rooms"].as_array().unwrap().iter().map(|x| (x["key"].as_str().unwrap().to_string(), x["name"].as_str().unwrap().to_string())).collect()
        }
        let mut app = started2();
        let up = app.handle("level", &json!({ "index": 1 })).unwrap();
        let lid = building::levels(model_of(&app))[1].1;
        let up_spaces = |a: &App| -> Vec<nnx_core::typed::Space> { model_of(a).elements.values().filter(|e| e.kind == "Space" && e.level_id == Some(lid)).map(|e| e.typed::<nnx_core::typed::Space>().unwrap()).collect() };
        let room = up_spaces(&app).into_iter().filter(|s| s.w >= 3000 && s.h >= 3000 && !s.requirement_key.starts_with("stair")).max_by_key(|s| s.w * s.h).expect("a room on the upper level");
        let parent_name = names(&up)[&room.requirement_key].clone();
        assert!(!parent_name.is_empty());
        let n0 = up_spaces(&app).len();
        let cut = room.x + room.w / 2;
        let r = app.handle("draft_separator", &json!({ "a": [cut, room.y], "b": [cut, room.y + room.h] })).unwrap();
        assert!(r["notes"].as_array().unwrap().iter().any(|s| s.as_str().unwrap().contains("was divided")), "{}", r["notes"]);
        let part_key = format!("{}-2", room.requirement_key);
        assert_eq!(up_spaces(&app).len(), n0 + 1, "one more room, on this level only");
        // The spaces list calls the part by its room's display name plus its number; the key stays.
        let listed = names(&r);
        assert_eq!(listed[&part_key], format!("{parent_name}-2"));
        assert_eq!(listed[&room.requirement_key], parent_name);
        // Tag every room of the level, then export both ways.
        app.handle("draft_tag", &json!({})).unwrap();
        let ifc = app.handle("export_ifc", &json!({})).unwrap();
        assert!(ifc["ifc"].as_str().unwrap().contains(&format!("'{part_key}'")), "the IFC space keeps the part's key as its Name");
        let dxf = app.handle("export_dxf", &json!({})).unwrap();
        assert!(dxf["counts"]["A-ROOM-SEP"].as_u64().unwrap() >= 1);
        // Save, reopen: the same model, clean, the same names, the same exports.
        let dir = std::env::temp_dir().join(format!("nanoarch-upper-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("upper.nnx");
        nnx_core::persist::save_with_settings(app.model().unwrap(), &app.settings(), &file).unwrap();
        let divided = hash(&app);
        let (loaded, settings) = nnx_core::persist::load_with_settings(&file).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        let mut reopened = App::default();
        let opened = reopened.open(loaded, &settings).unwrap();
        assert_eq!(hash(&reopened), divided);
        assert!(!reopened.is_dirty() && opened["warnings"].as_array().unwrap().is_empty(), "{}", opened["warnings"]);
        let again = reopened.handle("level", &json!({ "index": 1 })).unwrap();
        assert_eq!(names(&again)[&part_key], format!("{parent_name}-2"));
        // The IFC header's FILE_NAME line carries the export time (to the second): two exports a second apart differ only there.
        let without_export_time = |v: &Value| v["ifc"].as_str().unwrap().lines().filter(|l| !l.starts_with("FILE_NAME(")).collect::<Vec<_>>().join("\n");
        let reopened_ifc = reopened.handle("export_ifc", &json!({})).unwrap();
        assert!(ifc["ifc"].as_str().unwrap().contains("FILE_NAME(") && reopened_ifc["ifc"].as_str().unwrap().contains("FILE_NAME("), "the header line exists, so the comparison below drops something real");
        assert_eq!(without_export_time(&reopened_ifc), without_export_time(&ifc), "the reopened project gives the same IFC");
        let dxf2 = reopened.handle("export_dxf", &json!({})).unwrap();
        assert_eq!(dxf2["counts"], dxf["counts"]);
        assert_eq!(dxf2["dxf"], dxf["dxf"], "and the same DXF");
        // In the live project: undo steps back through the tags and the division (the part and its name go), redo brings them back.
        let mut steps = 0;
        while up_spaces(&app).len() > n0 && steps < 5 {
            app.handle("undo", &json!({})).unwrap();
            steps += 1;
        }
        assert_eq!(up_spaces(&app).len(), n0);
        assert!(!names(&app.handle("level", &json!({ "index": 1 })).unwrap()).contains_key(&part_key));
        for _ in 0..steps {
            app.handle("redo", &json!({})).unwrap();
        }
        assert_eq!(hash(&app), divided, "redo walks back to the divided, tagged project");
    }

    #[test]
    fn dividing_a_generated_room_moves_its_mep_points_and_openings_to_the_part_they_stand_in_and_the_file_keeps_it() {
        let mut app = started();
        app.handle("mep", &json!({ "inputs": mep_inputs(), "apply": true, "show": true })).unwrap();
        // The room with the most MEP points, divided down its middle.
        let spaces: Vec<nnx_core::typed::Space> = model_of(&app).elements.values().filter(|e| e.kind == "Space").map(|e| e.typed::<nnx_core::typed::Space>().unwrap()).collect();
        let points: Vec<(String, i64, i64)> = model_of(&app).elements.values().filter(|e| e.kind == "MepPoint").map(|e| (e.props["room"].as_str().unwrap().to_string(), e.props["x"].as_i64().unwrap(), e.props["y"].as_i64().unwrap())).collect();
        assert!(!points.is_empty(), "the MEP step made points");
        let room = spaces.iter().filter(|s| !s.requirement_key.starts_with("stair") && s.w >= 3000).max_by_key(|s| points.iter().filter(|p| p.0 == s.requirement_key).count()).unwrap().clone();
        let cut = room.x + room.w / 2;
        let before = room_refs(&app);
        let r = app.handle("draft_separator", &json!({ "a": [cut, room.y], "b": [cut, room.y + room.h] })).unwrap();
        assert!(r["notes"].as_array().unwrap().iter().any(|s| s.as_str().unwrap().contains("was divided")), "{}", r["notes"]);
        let after = room_refs(&app);
        let part = format!("{}-2", room.requirement_key);
        let names: std::collections::BTreeSet<String> = rooms_of(&app).into_iter().collect();
        assert!(names.contains(&part) && names.contains(&room.requirement_key), "both parts exist");
        // Every reference names a room that exists; only references to the divided room changed, and only to its new part.
        for (id, refs) in &after {
            // A door also names what is not a room (outside, a reserve): such a name is left exactly as it was.
            for (was, now) in before[id].iter().zip(refs) {
                assert!(was == now || (*was == room.requirement_key && *now == part), "{id}: '{was}' became '{now}'");
            }
        }
        // Each MEP point is in the room it names, and the points left of the cut are the new part's.
        let new_part = model_of(&app).elements.values().filter(|e| e.kind == "Space").map(|e| e.typed::<nnx_core::typed::Space>().unwrap()).find(|s| s.requirement_key == part).unwrap();
        for e in model_of(&app).elements.values().filter(|e| e.kind == "MepPoint") {
            let (x, y, key) = (e.props["x"].as_i64().unwrap(), e.props["y"].as_i64().unwrap(), e.props["room"].as_str().unwrap());
            if before[&e.id.to_string()][0] == room.requirement_key {
                let inside = x >= new_part.x && x <= new_part.x + new_part.w && y >= new_part.y && y <= new_part.y + new_part.h;
                let on_cut = x == cut;
                assert!(on_cut || (key == part) == inside, "the point at ({x}, {y}) names '{key}'; the new part is {}..{}", new_part.x, new_part.x + new_part.w);
            }
        }
        let moved = after.iter().filter(|(id, refs)| before[*id] != **refs).count();
        assert!(moved >= 1, "something that named the room now names its part");
        // Saved and reopened: the same model, clean, the division intact, and removing the separator undoes to it again.
        let dir = std::env::temp_dir().join(format!("nanoarch-split-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("split.nnx");
        nnx_core::persist::save_with_settings(app.model().unwrap(), &app.settings(), &file).unwrap();
        let divided = hash(&app);
        let (loaded, settings) = nnx_core::persist::load_with_settings(&file).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        let mut reopened = App::default();
        let opened = reopened.open(loaded, &settings).unwrap();
        assert_eq!(hash(&reopened), divided, "the file holds the divided model");
        assert!(!reopened.is_dirty(), "a reopened project is clean");
        assert!(opened["warnings"].as_array().unwrap().is_empty(), "{}", opened["warnings"]);
        assert_eq!(room_refs(&reopened), after);
        assert_eq!(rooms_of(&reopened).len(), rooms_of(&app).len());
        assert!(reopened.handle("export_dxf", &json!({})).unwrap()["counts"]["A-ROOM-SEP"].as_u64().unwrap() >= 1);
        let sep = r["separator"].as_str().unwrap().to_string();
        let del = reopened.handle("draft_delete", &json!({ "ids": [sep] })).unwrap();
        assert!(del["notes"].as_array().unwrap().iter().any(|s| s.as_str().unwrap().contains("one room again")), "{}", del["notes"]);
        assert!(reopened.is_dirty());
        assert_eq!(room_refs(&reopened), before, "every reference is as the layout made it");
        reopened.handle("undo", &json!({})).unwrap();
        assert_eq!(hash(&reopened), divided);
        assert!(!reopened.is_dirty(), "back at the saved state it is clean again");
    }

    #[test]
    fn offset_copies_a_wall_a_typed_distance_to_the_pointers_side_in_one_undoable_step() {
        let mut app = boxed_app();
        let wall = model_of(&app).elements.values().find(|e| e.kind == "Wall" && e.props["y1"] == 1000 && e.props["y2"] == 1000).unwrap().id.to_string();
        assert!(app.handle("draft_offset", &json!({ "id": wall, "distance": 0, "toward": [3000, 2000] })).unwrap_err().contains("above zero"));
        let before = model_of(&app).elements.values().filter(|e| e.kind == "Wall").count();
        app.handle("draft_offset", &json!({ "id": wall, "distance": 1500, "toward": [3000, 2000] })).unwrap();
        let walls: Vec<i64> = model_of(&app).elements.values().filter(|e| e.kind == "Wall").map(|e| e.props["y1"].as_i64().unwrap()).collect();
        assert_eq!(walls.len(), before + 1);
        assert!(walls.contains(&2500), "1000 + 1500, on the side of the pointer");
        let sep = app.handle("draft_separator", &json!({ "a": [3000, 2500], "b": [3000, 5000] })).unwrap()["separator"].as_str().unwrap().to_string();
        app.handle("draft_offset", &json!({ "id": sep, "distance": 500, "toward": [0, 0] })).unwrap();
        assert_eq!(model_of(&app).elements.values().filter(|e| e.kind == "RoomSeparator" && e.props["x1"] == 2500).count(), 1);
        let grid = app.handle("draft_grid", &json!({ "a": [1000, 0], "b": [1000, 6000] })).unwrap()["grid"].as_str().unwrap().to_string();
        app.handle("draft_offset", &json!({ "id": grid, "distance": 2000, "toward": [9000, 0] })).unwrap();
        assert_eq!(model_of(&app).elements.values().filter(|e| e.kind == "Grid").count(), 2);
        let room = model_of(&app).elements.values().find(|e| e.kind == "Space").unwrap().id.to_string();
        assert!(app.handle("draft_offset", &json!({ "id": room, "distance": 100, "toward": [0, 0] })).unwrap_err().contains("only walls"));
    }

    #[test]
    fn doors_windows_columns_and_footings_take_parametric_tags_that_follow_their_size() {
        let mut app = boxed_app();
        let wall = model_of(&app).elements.values().find(|e| e.kind == "Wall" && e.props["y1"] == 1000 && e.props["y2"] == 1000).unwrap().id.to_string();
        let door = app.handle("draft_opening", &json!({ "kind": "door", "wall": wall, "offset": 2000, "width": 900, "height": 2100 })).unwrap()["opening"].as_str().unwrap().to_string();
        let win = app.handle("draft_opening", &json!({ "kind": "window", "wall": wall, "offset": 4500, "width": 1200, "height": 1200, "sill": 900 })).unwrap()["opening"].as_str().unwrap().to_string();
        let t = app.handle("draft_tag", &json!({ "hosts": [door, win] })).unwrap();
        let svg = t["svg"].as_str().unwrap();
        assert!(svg.contains(">Door</text>") && svg.contains(">900 × 2100</text>") && svg.contains(">Window</text>") && svg.contains(">1200 × 1200</text>"), "{svg}");
        assert!(app.handle("draft_tag", &json!({ "hosts": [door] })).unwrap_err().contains("already"));
        // The tag is computed from the element: resizing the door changes its text.
        app.handle("draft_set", &json!({ "id": door, "key": "width", "value": 1000 })).unwrap();
        assert!(app.handle("svg", &json!({})).unwrap()["svg"].as_str().unwrap().contains(">1000 × 2100</text>"));
        // A wall is not taggable; deleting the door takes its tag with it.
        assert!(app.handle("draft_tag", &json!({ "hosts": [wall] })).unwrap_err().contains("only rooms"));
        app.handle("draft_delete", &json!({ "ids": [door] })).unwrap();
        assert_eq!(model_of(&app).elements.values().filter(|e| e.kind == "Tag").count(), 1);
    }

    fn insulated() -> Value {
        json!({ "id": "EXT-1", "name": "Insulated block wall", "kind": "exterior_insulated", "source": "synthetic test values",
            "layers": [{ "material": "plaster_ext_1856", "thicknessMm": 30 }, { "material": "rockwool_150", "thicknessMm": 70 }, { "material": "block_hollow_1216", "thicknessMm": 200 }, { "material": "plaster_int_1100", "thicknessMm": 20 }] })
    }

    #[test]
    fn a_wall_assembly_sets_the_thickness_and_feeds_the_thermal_check_the_take_off_and_the_3d_scene_in_one_undoable_step() {
        let mut app = boxed_app();
        let ext: Vec<String> = model_of(&app).elements.values().filter(|e| e.kind == "Wall" && e.props["isExterior"] == true).map(|e| e.id.to_string()).collect();
        assert_eq!(ext.len(), 4, "the box's four walls are exterior walls");
        // Bad data is refused by name.
        let mut bad = insulated();
        bad["layers"][1]["thicknessMm"] = json!(null);
        assert!(app.handle("assemblies_set", &json!({ "assemblies": [bad] })).unwrap_err().contains("needs a thickness"));
        let mut dup = insulated();
        dup["id"] = json!("a b");
        assert!(app.handle("assemblies_set", &json!({ "assemblies": [dup] })).unwrap_err().contains("letters, digits"));
        let hash0 = hash(&app);
        let r = app.handle("assemblies_set", &json!({ "assemblies": [insulated()], "defaults": { "exterior": "EXT-1" } })).unwrap();
        assert_eq!(r["assemblies"]["assemblies"][0]["thickness_mm"], 320);
        assert!(r["assemblies"]["assemblies"][0]["u"].is_null(), "no surface resistances entered yet: the U-value names what is missing");
        assert!(r["assemblies"]["assemblies"][0]["missing"].as_array().unwrap().iter().any(|m| m.as_str().unwrap().contains("Rsi")));
        // The project default leaves the walls' own thickness alone and reports the disagreement; assigning aligns them.
        assert_eq!(r["assemblies"]["mismatches"].as_array().unwrap().len(), 4);
        let a = app.handle("assembly_assign", &json!({ "walls": ext, "assembly": "EXT-1" })).unwrap();
        assert!(a["assemblies"]["mismatches"].as_array().unwrap().is_empty());
        let w = model_of(&app).get(&ext[0].parse().unwrap()).unwrap().clone();
        assert_eq!((w.props["thickness"].as_i64(), w.props["assembly"].as_str()), (Some(320), Some("EXT-1")));
        // The thickness is no longer typed on such a wall.
        assert!(app.handle("draft_set", &json!({ "id": ext[0], "key": "thickness", "value": 250 })).unwrap_err().contains("assembly"));
        // Thermal: U = 1 / (Rsi + sum(t/lambda) + Rse), from the layers.
        app.handle("compliance_set", &json!({ "compliance": { "thermal": { "rsi": 0.12, "rse": 0.03 } } })).unwrap();
        let rep = app.handle("compliance", &json!({})).unwrap();
        let wall_row = &rep["thermal"]["rows"][0];
        let r_total: f64 = 0.12 + 0.03 + 0.030 / 0.75 + 0.070 / 0.047 + 0.200 / 1.353 + 0.020 / 0.7;
        let u = wall_row["u"].as_f64().unwrap();
        assert!((u - (1.0 / r_total * 1000.0).round() / 1000.0).abs() < 1e-9, "U {u} from the layers");
        assert!(rep["thermal"]["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("Insulated block wall")));
        // Take-off: one area item per layer, from the walls that use it.
        let q = app.handle("qto", &json!({})).unwrap();
        let layers: Vec<&Value> = q["items"].as_array().unwrap().iter().filter(|i| i["code"].as_str().unwrap().starts_with("WALL.LAYER.EXT-1.")).collect();
        assert_eq!(layers.len(), 4);
        assert_eq!(layers[0]["code"], "WALL.LAYER.EXT-1.1.plaster_ext_1856");
        // 3D: the walls that use it are named in the scene.
        let scene = app.handle("model3d", &json!({})).unwrap();
        assert_eq!(scene["assemblies"][&ext[0]][1], "Insulated block wall");
        assert_eq!(scene["assemblies"][&ext[0]][0], "exterior_insulated");
        // Editing a layer moves every wall that names the assembly, in the same step.
        let mut thicker = insulated();
        thicker["layers"][1]["thicknessMm"] = json!(100);
        app.handle("assemblies_set", &json!({ "assemblies": [thicker], "defaults": { "exterior": "EXT-1" } })).unwrap();
        assert_eq!(model_of(&app).get(&ext[1].parse().unwrap()).unwrap().props["thickness"], 350);
        // An assembly that walls name cannot be dropped, and the kind must fit the wall.
        assert!(app.handle("assemblies_set", &json!({ "assemblies": [] })).unwrap_err().contains("clear it from the wall"));
        let part = json!({ "id": "INT-1", "name": "Block partition", "kind": "interior_partition", "layers": [{ "material": "block_hollow_1216", "thicknessMm": 150 }] });
        app.handle("assemblies_set", &json!({ "assemblies": [insulated(), part], "defaults": { "exterior": "EXT-1" } })).unwrap();
        assert!(app.handle("assembly_assign", &json!({ "walls": [ext[0]], "assembly": "INT-1" })).unwrap_err().contains("interior partition"));
        // Each successful edit was one undo step (assemblies, assignment, thermal inputs, two assembly edits): five undos restore the model.
        for _ in 0..5 {
            app.handle("undo", &json!({})).unwrap();
        }
        assert_eq!(hash(&app), hash0, "the model is as it was before the first assemblies edit");
    }

    #[test]
    fn a_project_drafted_from_scratch_feeds_the_checks_schedules_and_every_export() {
        let mut app = boxed_app();
        let wall = model_of(&app).elements.values().find(|e| e.kind == "Wall" && e.props["y1"] == 1000 && e.props["y2"] == 1000).unwrap().id.to_string();
        app.handle("draft_opening", &json!({ "kind": "door", "wall": wall, "offset": 2000, "width": 900, "height": 2100 })).unwrap();
        app.handle("draft_opening", &json!({ "kind": "window", "wall": wall, "offset": 4500, "width": 1200, "height": 1200, "sill": 900 })).unwrap();
        app.handle("draft_separator", &json!({ "a": [3000, 1000], "b": [3000, 5000] })).unwrap();
        for cmd in ["sbc_check", "qto", "model3d", "outline", "compliance", "svg"] {
            assert!(app.handle(cmd, &json!({})).is_ok(), "{cmd} works on a drafted project");
        }
        let ifc = app.handle("export_ifc", &json!({})).unwrap();
        assert!(ifc["ifc"].as_str().unwrap().to_uppercase().contains("IFCWALL"), "the drafted walls are in the IFC");
        let dxf = app.handle("export_dxf", &json!({})).unwrap();
        assert!(dxf["dxf"].as_str().unwrap().contains("A-ROOM-SEP"));
    }

    #[test]
    fn an_auto_designed_plan_takes_the_assembly_default_tags_and_every_pro_tool_without_losing_its_layout() {
        let mut app = started();
        let rooms = rooms_of(&app);
        let ext_area: f64 = app.handle("qto", &json!({})).unwrap()["items"].as_array().unwrap().iter().find(|i| i["code"] == "WALL.EXT.AREA").unwrap()["quantity"].as_f64().unwrap();
        // The project default reaches every exterior wall of the generated plan: thermal notes and one take-off item per layer.
        app.handle("assemblies_set", &json!({ "assemblies": [insulated()], "defaults": { "exterior": "EXT-1" } })).unwrap();
        app.handle("compliance_set", &json!({ "compliance": { "thermal": { "rsi": 0.12, "rse": 0.03 } } })).unwrap();
        let rep = app.handle("compliance", &json!({})).unwrap();
        assert!(rep["thermal"]["rows"][0]["u"].as_f64().unwrap() > 0.0);
        let q = app.handle("qto", &json!({})).unwrap();
        let layer = q["items"].as_array().unwrap().iter().find(|i| i["code"].as_str().unwrap().starts_with("WALL.LAYER.EXT-1.2.")).unwrap();
        assert!((layer["quantity"].as_f64().unwrap() - ext_area).abs() < 1e-6, "a layer covers the exterior walls' net area");
        let r = app.handle("assemblies_get", &json!({})).unwrap();
        assert!(!r["mismatches"].as_array().unwrap().is_empty(), "generated walls keep their own thickness and are reported, not changed");
        // A door and a window of the generated plan take tags; a wall is drafted on top; a separator may not split a layout room.
        let door = model_of(&app).elements.values().find(|e| e.kind == "Door").unwrap().id.to_string();
        assert!(app.handle("draft_tag", &json!({ "hosts": [door] })).unwrap()["svg"].as_str().unwrap().contains("class=\"tag\""));
        let frame = app.handle("draft_frame", &json!({})).unwrap();
        assert!(frame["frame"].is_object());
        assert_eq!(rooms_of(&app), rooms, "the layout's rooms are untouched by all of it");
        let hash1 = hash(&app);
        app.handle("undo", &json!({})).unwrap();
        assert_ne!(hash(&app), hash1);
    }
}
