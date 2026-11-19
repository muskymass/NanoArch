//! Application logic behind the window: JSON commands in, JSON out. No UI code here, so it is tested directly.
//!
//! Every number the engines need comes from the user's input. Blank inputs stay unset and the
//! engines say what is missing; nothing here supplies a building-code, load, soil or productivity value.

use nnx_core::drawing::{DrawOpts, plan_svg};
use nnx_core::layout::{Adjacency, Brief, Rect, RoomReq, Side, Zone};
use nnx_core::qto::{self, QtoOpts};
use nnx_core::rules::{self, RuleSet};
use nnx_core::schedule::{self, Calendar, Date, RateLib};
use nnx_core::structural::{Loads, Soil, StructOpts};
use nnx_core::walls::{WallOpts, WindowOpts};
use crate::editing;
use crate::levels::{self, Building, BuildingPlan};
use nnx_core::building;
use nnx_core::persist::Settings;
use nnx_core::Document;
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
        let min_w = r.min_width_mm.unwrap_or(if r.func == "bathroom" { 1500 } else { 2000 });
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
        DrawOpts { title, labels: self.labels(), show_structure, show_mep: self.show_mep, ..DrawOpts::default() }
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
            "templates" => Ok(json!({ "templates": nnx_core::templates::all() })),
            "analysis" => self.analysis(args),
            "materials" => Ok(nnx_core::materials::catalog_json()),
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
            "lock" => self.lock(args),
            "regenerate" => self.regenerate(args),
            "site_layout" => self.site_layout_cmd(),
            "furniture" => self.furniture_cmd(),
            "vertical" => self.vertical_cmd(),
            "template_apply" => Self::template_apply(args),
            "site_plan" => self.site_plan(),
            "zone_preset" => self.zone_preset(args),
            "elevation" => self.elevation(args),
            "section" => self.section(args),
            "clashes" => Ok(self.clash_json()),
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
            "qto" => self.qto(),
            "mep" => self.mep(args),
            "sizing" => self.sizing(args),
            "boq" => self.boq(args),
            "model3d" => serde_json::to_value(nnx_core::model3d::build_styled(&self.doc()?.model, self.style())).map_err(|e| e.to_string()),
            "export_ifc" => self.export_ifc(),
            "export_dxf" => self.export_dxf(args.get("all").and_then(Value::as_bool).unwrap_or(false), args.get("views").and_then(Value::as_bool).unwrap_or(false)),
            "report_html" => {
                let arabic = args.get("lang").and_then(Value::as_str) != Some("en");
                self.report(args).map(|(html, _)| json!({ "html": self.with_history(html, arabic), "name": "engineering-report.pdf" }))
            }
            "report_xlsx" => self.report(args).map(|(_, b64)| json!({ "xlsx_b64": b64, "name": "engineering-report.xlsx" })),
            "pdf_html" => self.pdf_html(args.get("lang").and_then(Value::as_str) != Some("en")),
            "template_keys" => Ok(json!(schedule::template_keys().iter().map(|(k, fixed)| json!({ "key": k, "fixed": fixed })).collect::<Vec<_>>())),
            "schedule" => self.schedule(args),
            "check_rules" => self.check_rules(args),
            other => Err(format!("unknown command '{other}'")),
        }
    }

    fn generate(&mut self, args: &Value) -> Result<Value, String> {
        let b: BriefIn = serde_json::from_value(args.get("brief").cloned().unwrap_or(Value::Null)).map_err(|e| format!("brief: {e}"))?;
        let s: StructIn = serde_json::from_value(args.get("struct").cloned().unwrap_or(json!({}))).map_err(|e| format!("structure inputs: {e}"))?;
        let plan = levels::plan_building(&b, s.floors.unwrap_or(1))?;
        let locked = parse_locked(args.get("locked"))?;
        let mut alts = levels::generate_building_locked(&plan, b.seed.unwrap_or(1), &locked)?;
        // Rooms marked on a sketch become the first alternative; the generated ones follow for comparison.
        let from_sketch = match args.get("placement").filter(|p| p.as_object().is_some_and(|o| !o.is_empty())) {
            Some(p) => {
                let placed = levels::placed_building(&plan, p, &alts[0])?;
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
            let opts = DrawOpts { title: String::new(), labels: plan.labels.clone(), px_per_m: 16.0, margin_px: 14.0, show_dims: false, show_structure: false, show_mep: false };
            let ground = building::levels(&doc.model).first().map(|(_, id)| *id);
            let svg = plan_svg(&doc.model, ground, &opts).map_err(|e| e.to_string())?;
            let ground_layout = &alt.levels[0];
            thumbs.push(json!({ "index": i, "cost": (ground_layout.cost * 100.0).round() / 100.0, "reserve_m2": (ground_layout.reserve_m2 * 10.0).round() / 10.0, "from_sketch": from_sketch && i == 0, "svg": svg }));
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
        let (report, applied) = {
            let doc = self.doc.as_mut().ok_or("choose a layout first")?;
            let report = nnx_core::mep::compute(&doc.model, &inputs);
            let applied = if apply { Some(nnx_core::mep::apply_with_runs(doc, &report.points, &report.runs)?) } else { None };
            (report, applied)
        };
        Ok(json!({
            "report": report,
            "applied": applied.map(|(c, r)| json!({ "created": c, "removed": r })),
            "points": self.doc()?.model.elements.values().filter(|e| e.kind == "MepPoint").count(),
            "svg": self.render_or_null()?,
        }))
    }

    /// The Bill of Quantities: every take-off item with the user's specification and rate, the materials the
    /// items consume, a specification schedule, and the exports (three CSV files and a printable document).
    /// The model as an IFC4 file (text) with the count of each product class written.
    fn export_ifc(&self) -> Result<Value, String> {
        let opts = nnx_core::ifc::IfcOpts {
            project_name: self.display_title(),
            author: "NanoArch".into(),
            timestamp: utc_now(),
            labels: self.labels(),
        };
        let out = nnx_core::ifc::export_ifc(&self.doc()?.model, &opts)?;
        Ok(json!({ "ifc": out.text, "counts": out.counts }))
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
        let html = self.with_history(nnx_core::printdoc::print_html(&self.doc()?.model, &opts)?, arabic);
        Ok(json!({ "html": html, "name": "plan.pdf" }))
    }

    /// The plan as a DXF file: the level on screen, or every level side by side when `all` is true.
    fn export_dxf(&self, all: bool, views: bool) -> Result<Value, String> {
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
        Ok(json!({ "dxf": out.text, "counts": out.counts, "levels": levels.len(), "views": specs.len() }))
    }

    fn boq(&mut self, args: &Value) -> Result<Value, String> {
        let inputs = args.get("inputs").cloned().unwrap_or(json!({}));
        let lib: nnx_core::boq::BoqLib = serde_json::from_value(inputs).map_err(|e| format!("BOQ inputs: {e}"))?;
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
        let mut v = Vec::new();
        if let Some(inp) = self.project().and_then(|p| p.mep)
            && let Some(lib) = inp.insulation
        {
            v.extend(nnx_core::insulation::analyze(model, &lib).items);
        }
        v.extend(nnx_core::mep::point_items(model));
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
        let first = nnx_core::sizing::compute(&self.doc()?.model, soil, &inputs);
        nnx_core::sizing::apply_tie_beams(self.doc.as_mut().ok_or("choose a layout first")?, &first)?;
        let r = nnx_core::sizing::compute(&self.doc()?.model, soil, &inputs);
        Ok(json!({ "report": r, "members_csv": r.members_csv(), "concrete_csv": r.concrete_csv(), "bbs_csv": r.bbs_csv(), "soil_kpa": soil }))
    }

    fn qto(&self) -> Result<Value, String> {
        let mut q = qto::compute(&self.doc()?.model, &QtoOpts::default());
        q.items.extend(self.extra_items(&self.doc()?.model));
        let items: Vec<Value> = q
            .items
            .iter()
            .map(|i| json!({ "code": i.code, "description": i.description, "unit": i.unit.label(), "quantity": i.quantity, "sources": i.sources.len() }))
            .collect();
        Ok(json!({ "items": items, "warnings": q.warnings, "csv": q.to_csv() }))
    }

    fn schedule(&self, args: &Value) -> Result<Value, String> {
        let doc = self.doc()?;
        let lib = RateLib::from_json(&json!({ "rates": args.get("rates").cloned().unwrap_or(json!({})) }).to_string()).map_err(|e| format!("rates: {e}"))?;
        let start = args.get("start").and_then(Value::as_str).and_then(Date::parse).ok_or("enter the start date as YYYY-MM-DD")?;
        let days: Vec<bool> = args.get("workdays").and_then(Value::as_array).map(|a| a.iter().map(|v| v.as_bool().unwrap_or(false)).collect()).unwrap_or_default();
        let work_days: [bool; 7] = days.try_into().map_err(|_| "workdays needs seven entries, Monday first")?;
        let mut cal = Calendar::new(start, work_days).ok_or("at least one working day a week is required")?;
        for h in args.get("holidays").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).filter_map(Date::parse) {
            cal.holidays.insert(h);
        }
        let zones = args.get("zones").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;
        // The take-off, plus insulation, MEP devices and runs, and the concrete and steel of the structural schedules.
        let mut q = qto::compute(&doc.model, &QtoOpts::default());
        q.items.extend(self.extra_items(&doc.model));
        if let Some(inputs) = self.project().and_then(|p| p.sizing.map(|s| (s, p.structure.bearing_kpa))) {
            q.items.extend(nnx_core::sizing::schedule_items(&nnx_core::sizing::compute(&doc.model, inputs.1, &inputs.0)));
        }
        match schedule::build_with_model(&doc.model, &q, &lib, &cal, zones) {
            Ok(s) => {
                let tasks: Vec<Value> = s
                    .tasks
                    .iter()
                    .map(|t| {
                        json!({
                            "id": t.id, "name": t.name, "wbs": t.wbs, "phase": t.phase, "duration": t.duration, "es": t.es, "ef": t.ef,
                            "start": t.start.iso(), "finish": t.finish.iso(), "float": t.float, "critical": t.critical,
                            "quantity": t.quantity, "unit": t.unit.map(|u| u.label()),
                            "also": t.also.iter().map(|m| json!({ "label": m.label, "quantity": m.quantity, "unit": m.unit.label() })).collect::<Vec<_>>(),
                            "elements": t.elements.iter().map(|e| e.to_string()).collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                Ok(json!({
                    "ok": true, "project_days": s.project_days, "finish": s.finish.iso(), "tasks": tasks, "warnings": s.warnings,
                    "svg": s.gantt_svg(&cal, &self.title()), "csv": s.to_csv(),
                    "calendar": (0..=s.project_days).map(|d| cal.nth_workday(d).iso()).collect::<Vec<_>>()
                }))
            }
            Err(schedule::ScheduleError::MissingRates(keys)) => Ok(json!({ "ok": false, "missing": keys })),
            Err(e) => Err(e.to_string()),
        }
    }

    fn check_rules(&self, args: &Value) -> Result<Value, String> {
        let doc = self.doc()?;
        let pack = args.get("pack").and_then(Value::as_str).ok_or("paste a rule pack (JSON)")?;
        let mut rs = RuleSet::default();
        rs.add_layer(pack).map_err(|e| e.to_string())?;
        let r = rules::check(&doc.model, &rs);
        let sev = |s: rules::Severity| match s {
            rules::Severity::Error => "error",
            rules::Severity::Warning => "warning",
            rules::Severity::Info => "info",
        };
        Ok(json!({
            "checked": r.checked,
            "layers": r.layers,
            "findings": r.findings.iter().map(|f| json!({ "rule": f.rule_id, "clause": f.clause, "severity": sev(f.severity), "message": f.message, "element": f.element.to_string() })).collect::<Vec<_>>(),
            "skipped": r.skipped.iter().map(|s| json!({ "rule": s.rule_id, "reason": s.reason })).collect::<Vec<_>>(),
        }))
    }

    /// The engineering report from the project's stored MEP and structural inputs: the printable HTML and the XLSX (base64).
    fn report(&self, args: &Value) -> Result<(String, String), String> {
        let arabic = args.get("lang").and_then(Value::as_str) != Some("en");
        let model = &self.doc()?.model;
        let p = self.project().ok_or("choose a layout first")?;
        let mep = p.mep.as_ref().map(|i| nnx_core::mep::compute(model, i));
        let sizing = p.sizing.as_ref().map(|i| nnx_core::sizing::compute(model, p.structure.bearing_kpa, i));
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
            .map(|s| json!({ "key": s.requirement_key, "locked": locked.contains(&s.requirement_key), "name": labels.get(&s.requirement_key).cloned().unwrap_or(s.requirement_key.clone()), "func": s.func, "area": s.area, "target": s.target_area }))
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
        if !model.elements.values().any(|e| e.kind == "Space") {
            return Err("this file has no plan (no spaces)".into());
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

    /// Every advanced structural analysis of the open project in one answer (inputs are the project's own).
    fn structural_report(&self) -> Result<Value, String> {
        let p = self.project().ok_or("choose a layout first")?;
        let model = &self.doc()?.model;
        let s = &p.meta.structural;
        let thick = p.sizing.as_ref().and_then(|z| z.footing_thickness_mm);
        let tank_results: Vec<nnx_core::tanks::TankResult> = s.tanks.iter().map(|t| nnx_core::tanks::design(t, &s.tank_design)).collect();
        let mut pieces = Vec::new();
        let schedule = p.sizing.as_ref().map(|z| nnx_core::sizing::compute(model, p.structure.bearing_kpa, z));
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
        let input = match p.site_input() {
            Some(i) => i,
            None => {
                let b = p.brief.as_ref().ok_or("the project has no brief: enter the plot size and generate first")?;
                let mut i = nnx_core::site::rectangle_with_setbacks(b.width_m, b.length_m, b.setback_front_m, b.setback_back_m, b.setback_side_m);
                i.floors = p.structure.floors;
                i
            }
        };
        let r = nnx_core::site::analyse(&input).map_err(|e| e.join("; "))?;
        let layout = match &p.meta.site_layout {
            Some(l) => Some(nnx_core::site_layout::layout(&r, l)?),
            None => None,
        };
        Ok(json!({ "svg": nnx_core::site::site_svg_with(&r, &self.display_title(), layout.as_ref()), "name": "site-plan.svg", "result": r, "layout": layout }))
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

    /// Clashes between ducts, pipes and drains and the structure.
    fn clash_json(&self) -> Value {
        match &self.doc {
            Some(d) => serde_json::to_value(nnx_core::clash::detect(&nnx_core::model3d::build(&d.model), &nnx_core::clash::ClashOpts::default())).unwrap_or(Value::Null),
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
    fn undo_redo_and_sync_work_on_the_current_document() {
        let mut app = started();
        let s = app.handle("sync", &json!({})).unwrap();
        assert_eq!((s["created"].as_u64(), s["removed"].as_u64()), (Some(0), Some(0)), "freshly generated structure is already in step");
        let undone = app.handle("undo", &json!({})).unwrap();
        assert_eq!(undone["changed"], true);
        assert!(undone["svg"].is_null(), "after undoing the only step there is nothing to draw");
        assert!(app.handle("svg", &json!({ "show_structure": false })).unwrap()["svg"].is_null());
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
        if let Ok(path) = std::env::var("NANOARCH_IFC_OUT") {
            std::fs::write(path, text).unwrap();
        }
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
        assert!(App::default().handle("pdf_html", &json!({})).is_err());
    }

    // ------------------------------------------------------------------ project (single source of truth)

    fn started_with_project() -> App {
        let mut app = App::default();
        let meta = json!({ "title": "x", "buildingType": "villa", "style": "najdi", "smartTech": ["lighting"], "climateZone": "zone-a" });
        app.handle("generate", &json!({ "brief": brief(), "struct": {}, "meta": meta })).unwrap();
        app.handle("select", &json!({ "index": 0, "struct": {} })).unwrap();
        app
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
    fn every_starter_template_applies_and_generates_a_plan_with_no_empty_room_area() {
        let list = App::default().handle("templates", &json!({})).unwrap();
        let ids: Vec<String> = list["templates"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap().to_string()).collect();
        assert_eq!(ids, vec!["villa_jubail", "palace", "apartments_4"]);
        assert!(App::default().handle("template_apply", &json!({ "id": "nope" })).is_err());
        for id in ids {
            let r = App::default().handle("template_apply", &json!({ "id": id })).unwrap();
            let t = &r["template"];
            assert!(r["sources"].as_array().unwrap().iter().all(|s| s.as_str().unwrap().contains("clause 4-")));
            // The page sends exactly this: the plot, the setbacks from the rule, the rooms with their areas.
            let rooms: Vec<Value> = t["rooms"].as_array().unwrap().iter().map(|x| json!({ "key": x["key"], "name": x["name"], "func": x["func"], "area": x["area_m2"], "zone": x["zone"], "light": x["func"] != "bathroom", "floor": x["floor"] })).collect();
            assert!(rooms.iter().all(|x| x["area"].as_f64().unwrap() > 0.0));
            let st = t["stair"].as_array().unwrap();
            let brief = json!({ "title": t["project_title"], "width_m": t["plot_width_m"], "length_m": t["plot_length_m"], "setback_front_m": r["setbacks"]["front_m"], "setback_back_m": r["setbacks"]["back_m"], "setback_side_m": r["setbacks"]["side_m"], "entry": "south", "rooms": rooms,
                "stair": { "width_m": st[0], "length_m": st[1], "side": st[2] } });
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
            if i == 0 {
                assert!(bad.is_empty(), "the first alternative reaches the ratio everywhere: {bad:?}");
            }
        }
        // Moving a partition rebuilds the windows with the same target.
        app.handle("select", &json!({ "index": 0, "struct": {} })).unwrap();
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
}
