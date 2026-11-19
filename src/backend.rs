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
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
pub struct RoomIn {
    pub key: String,
    /// Display name (for example Arabic); the key is used when absent.
    pub name: Option<String>,
    pub func: String,
    pub area: f64,
    pub zone: String,
    #[serde(default)]
    pub light: bool,
    pub min_width_mm: Option<i64>,
    /// 1 is the ground floor. Absent means the ground floor.
    pub floor: Option<u32>,
}

/// The stair of a building with several floors. Nothing here is defaulted.
#[derive(Debug, Deserialize, Clone)]
pub struct StairIn {
    pub width_m: f64,
    pub length_m: f64,
    /// `west` or `east`.
    pub side: String,
}

#[derive(Debug, Deserialize)]
pub struct BriefIn {
    pub title: Option<String>,
    pub width_m: f64,
    pub length_m: f64,
    #[serde(default)]
    pub setback_front_m: f64,
    #[serde(default)]
    pub setback_back_m: f64,
    #[serde(default)]
    pub setback_side_m: f64,
    pub entry: String,
    pub rooms: Vec<RoomIn>,
    pub seed: Option<u64>,
    pub stair: Option<StairIn>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct StructIn {
    pub dead_kpa: Option<f64>,
    pub live_kpa: Option<f64>,
    pub bearing_kpa: Option<f64>,
    pub floors: Option<u32>,
    pub max_span_mm: Option<i64>,
    #[serde(default)]
    pub show_structure: bool,
}

#[derive(Default)]
pub struct App {
    alternatives: Vec<Building>,
    plan: Option<BuildingPlan>,
    brief: Option<Brief>,
    /// Index of the level shown in the plan view.
    cur_level: usize,
    title: String,
    labels: BTreeMap<String, String>,
    doc: Option<Document>,
    sopts: Option<StructOpts>,
    show_structure: bool,
    show_mep: bool,
    /// MEP and insulation inputs exactly as entered; kept so quantities and saved projects use them.
    raw_mep: Value,
    /// BOQ inputs (specifications, rates, consumption factors, mark-ups) exactly as entered.
    raw_boq: Value,
    /// The inputs exactly as the user entered them, kept so they can be saved with the project.
    raw_brief: Value,
    raw_struct: Value,
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
        let mut title = self.title.clone();
        if let Some((_, _, name)) = self.level_list().get(self.cur_level) {
            title = format!("{title} — {name}");
        }
        DrawOpts { title, labels: self.labels.clone(), show_structure, show_mep: self.show_mep, ..DrawOpts::default() }
    }

    fn render(&self) -> Result<String, String> {
        plan_svg(&self.doc()?.model, self.current_level(), &self.draw_opts(self.show_structure)).map_err(|e| e.to_string())
    }

    /// The plan as JSON `svg`, or null when the model is empty (after undoing the whole generation).
    fn render_or_null(&self) -> Result<Value, String> {
        Ok(match plan_svg(&self.doc()?.model, self.current_level(), &self.draw_opts(self.show_structure)) {
            Ok(svg) => Value::String(svg),
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
                Ok(json!({ "changed": changed, "svg": self.render_or_null()?, "levels": self.levels_json(), "rooms": self.rooms_json() }))
            }
            "sync" => {
                let so = self.sopts.clone().ok_or("choose a layout first")?;
                let doc = self.doc.as_mut().ok_or("choose a layout first")?;
                let r = levels::sync_all(doc, &so)?;
                Ok(json!({ "created": r.created, "removed": r.removed, "unchanged": r.unchanged, "warnings": r.warnings, "svg": self.render()? }))
            }
            "describe" => editing::describe(self.doc()?, id_arg(args, "id")?),
            "move_partition" => {
                let (id, delta) = (id_arg(args, "wall")?, args.get("delta_mm").and_then(Value::as_i64).ok_or("delta_mm is required")?);
                let brief = self.brief.clone().ok_or("this project has no saved brief: the entrance side is unknown, so partitions cannot be moved")?;
                let so = self.sopts.clone().ok_or("choose a layout first")?;
                let doc = self.doc.as_mut().ok_or("choose a layout first")?;
                let points_before = doc.model.elements.values().filter(|e| e.kind == "MepPoint").count();
                let moved = editing::move_partition(doc, &brief, &so, id, delta)?;
                let points_after = doc.model.elements.values().filter(|e| e.kind == "MepPoint").count();
                Ok(json!({ "moved": moved, "mep_cleared": points_before - points_after, "svg": self.render_or_null()?, "rooms": self.rooms_json() }))
            }
            "set_opening" => {
                let id = id_arg(args, "id")?;
                let (width, offset) = (args.get("width").and_then(Value::as_i64), args.get("offset").and_then(Value::as_i64));
                let so = self.sopts.clone().ok_or("choose a layout first")?;
                editing::set_opening(self.doc.as_mut().ok_or("choose a layout first")?, &so, id, width, offset)?;
                Ok(json!({ "svg": self.render_or_null()? }))
            }
            "delete_opening" => {
                let id = id_arg(args, "id")?;
                let so = self.sopts.clone().ok_or("choose a layout first")?;
                editing::delete_opening(self.doc.as_mut().ok_or("choose a layout first")?, &so, id)?;
                Ok(json!({ "svg": self.render_or_null()? }))
            }
            "status" => Ok(json!({ "has_doc": self.doc.is_some(), "dirty": self.is_dirty() })),
            "assumptions" => Ok(assumptions()),
            "qto" => self.qto(),
            "mep" => self.mep(args),
            "boq" => self.boq(args),
            "model3d" => serde_json::to_value(nnx_core::model3d::build(&self.doc()?.model)).map_err(|e| e.to_string()),
            "export_ifc" => self.export_ifc(),
            "export_dxf" => self.export_dxf(args.get("all").and_then(Value::as_bool).unwrap_or(false)),
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
        let mut alts = levels::generate_building(&plan, b.seed.unwrap_or(1))?;
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
        let mut thumbs = Vec::new();
        for (i, alt) in alts.iter().enumerate() {
            let (doc, _) = levels::assemble(alt, &plan, &so)?;
            let opts = DrawOpts { title: String::new(), labels: plan.labels.clone(), px_per_m: 16.0, margin_px: 14.0, show_dims: false, show_structure: false, show_mep: false };
            let ground = building::levels(&doc.model).first().map(|(_, id)| *id);
            let svg = plan_svg(&doc.model, ground, &opts).map_err(|e| e.to_string())?;
            let ground_layout = &alt.levels[0];
            thumbs.push(json!({ "index": i, "cost": (ground_layout.cost * 100.0).round() / 100.0, "reserve_m2": (ground_layout.reserve_m2 * 10.0).round() / 10.0, "from_sketch": from_sketch && i == 0, "svg": svg }));
        }
        self.raw_brief = args.get("brief").cloned().unwrap_or(Value::Null);
        self.raw_struct = args.get("struct").cloned().unwrap_or(json!({}));
        self.title = b.title.unwrap_or_default();
        self.labels = plan.labels.clone();
        self.alternatives = alts;
        self.brief = Some(plan.merged.clone());
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
        let (doc, mut warnings) = levels::assemble(&alt, plan, &so)?;
        warnings.splice(0..0, plan.notes.iter().cloned());
        let n = alt.levels.len();
        if n > 1 && s.floors.unwrap_or(1) as usize != n {
            warnings.push(format!("the floors input ({}) differs from the {n} levels drawn: generate again", s.floors.unwrap_or(1)));
        }
        self.show_structure = s.show_structure;
        self.raw_struct = args.get("struct").cloned().unwrap_or(json!({}));
        self.sopts = Some(so);
        self.doc = Some(doc);
        self.cur_level = 0;
        Ok(json!({ "svg": self.render()?, "warnings": warnings, "rooms": self.rooms_json(), "levels": self.levels_json() }))
    }

    /// Computes MEP and insulation from the user's inputs. With `apply`, the generated points become model
    /// elements in one undoable step; `show` switches the overlay. Missing inputs are reported by name.
    fn mep(&mut self, args: &Value) -> Result<Value, String> {
        let inputs_json = args.get("inputs").cloned().unwrap_or(json!({}));
        let inputs: nnx_core::mep::MepInputs = serde_json::from_value(inputs_json.clone()).map_err(|e| format!("MEP inputs: {e}"))?;
        self.raw_mep = inputs_json;
        if let Some(show) = args.get("show").and_then(Value::as_bool) {
            self.show_mep = show;
        }
        let apply = args.get("apply").and_then(Value::as_bool).unwrap_or(false);
        let (report, applied) = {
            let doc = self.doc.as_mut().ok_or("choose a layout first")?;
            let report = nnx_core::mep::compute(&doc.model, &inputs);
            let applied = if apply { Some(nnx_core::mep::apply(doc, &report.points)?) } else { None };
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
            project_name: if self.title.trim().is_empty() { "NanoArch project".into() } else { self.title.clone() },
            author: "NanoArch".into(),
            timestamp: utc_now(),
            labels: self.labels.clone(),
        };
        let out = nnx_core::ifc::export_ifc(&self.doc()?.model, &opts)?;
        Ok(json!({ "ifc": out.text, "counts": out.counts }))
    }

    /// The printable plan set (one A3 sheet per level) as HTML; the shell prints it to PDF.
    fn pdf_html(&self, arabic: bool) -> Result<Value, String> {
        let opts = nnx_core::printdoc::PrintOpts {
            title: if self.title.trim().is_empty() { "NanoArch project".into() } else { self.title.clone() },
            labels: self.labels.clone(),
            arabic,
            show_structure: self.show_structure,
            show_mep: self.show_mep,
            date: utc_now().chars().take(10).collect(),
        };
        let html = nnx_core::printdoc::print_html(&self.doc()?.model, &opts)?;
        Ok(json!({ "html": html, "name": "plan.pdf" }))
    }

    /// The plan as a DXF file: the level on screen, or every level side by side when `all` is true.
    fn export_dxf(&self, all: bool) -> Result<Value, String> {
        let model = &self.doc()?.model;
        let levels: Vec<Option<nnx_core::Id>> = if all {
            let l = building::levels(model);
            if l.is_empty() { vec![None] } else { l.into_iter().map(|(_, id)| Some(id)).collect() }
        } else {
            vec![self.current_level()]
        };
        let opts = nnx_core::dxf::DxfOpts { title: if self.title.trim().is_empty() { "NanoArch project".into() } else { self.title.clone() }, labels: self.labels.clone(), mep: true };
        let out = nnx_core::dxf::export_dxf(model, &levels, &opts)?;
        Ok(json!({ "dxf": out.text, "counts": out.counts, "levels": levels.len() }))
    }

    fn boq(&mut self, args: &Value) -> Result<Value, String> {
        let inputs = args.get("inputs").cloned().unwrap_or(json!({}));
        let lib: nnx_core::boq::BoqLib = serde_json::from_value(inputs.clone()).map_err(|e| format!("BOQ inputs: {e}"))?;
        self.raw_boq = inputs;
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
        if let Ok(inp) = serde_json::from_value::<nnx_core::mep::MepInputs>(self.raw_mep.clone())
            && let Some(lib) = inp.insulation
        {
            v.extend(nnx_core::insulation::analyze(model, &lib).items);
        }
        v.extend(nnx_core::mep::point_items(model));
        v
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
        let q = qto::compute(&doc.model, &QtoOpts::default());
        match schedule::build_with_model(&doc.model, &q, &lib, &cal, zones) {
            Ok(s) => {
                let tasks: Vec<Value> = s
                    .tasks
                    .iter()
                    .map(|t| json!({ "id": t.id, "name": t.name, "duration": t.duration, "start": t.start.iso(), "finish": t.finish.iso(), "float": t.float, "critical": t.critical }))
                    .collect();
                Ok(json!({
                    "ok": true, "project_days": s.project_days, "finish": s.finish.iso(), "tasks": tasks, "warnings": s.warnings,
                    "svg": s.gantt_svg(&cal, &self.title), "csv": s.to_csv()
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

    /// The current model, for saving.
    pub fn model(&self) -> Result<&nnx_core::Model, String> {
        Ok(&self.doc()?.model)
    }

    /// The room table (name, target and actual area) read from the model's spaces.
    fn rooms_json(&self) -> Vec<Value> {
        let level = self.current_level();
        self.doc
            .iter()
            .flat_map(|d| d.model.elements.values())
            .filter(|e| level.is_none() || e.level_id == level)
            .filter_map(|e| e.typed::<nnx_core::typed::Space>().ok())
            .map(|s| json!({ "name": self.labels.get(&s.requirement_key).cloned().unwrap_or(s.requirement_key.clone()), "func": s.func, "area": s.area, "target": s.target_area }))
            .collect()
    }

    /// Everything besides the model that a project needs to be reopened as it was.
    pub fn settings(&self) -> Settings {
        let mut s = Settings::new();
        s.insert("title".into(), json!(self.title));
        s.insert("labels".into(), json!(self.labels));
        s.insert("brief".into(), self.raw_brief.clone());
        s.insert("structure".into(), self.raw_struct.clone());
        s.insert("show_structure".into(), json!(self.show_structure));
        s.insert("mep".into(), self.raw_mep.clone());
        s.insert("show_mep".into(), json!(self.show_mep));
        s.insert("boq".into(), self.raw_boq.clone());
        s
    }

    /// Opens a saved project: the model becomes the document (clean, nothing unsaved) and the user's
    /// inputs are restored. Generated alternatives are not saved, so they are cleared.
    pub fn open(&mut self, model: nnx_core::Model, settings: &Settings) -> Result<Value, String> {
        let doc = Document::new(model);
        if !doc.model.elements.values().any(|e| e.kind == "Space") {
            return Err("this file has no plan (no spaces)".into());
        }
        self.title = settings.get("title").and_then(Value::as_str).unwrap_or("").to_string();
        self.labels = settings.get("labels").and_then(Value::as_object).map(|o| o.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()).unwrap_or_default();
        self.raw_brief = settings.get("brief").cloned().unwrap_or(Value::Null);
        self.raw_struct = settings.get("structure").cloned().unwrap_or(json!({}));
        self.show_structure = settings.get("show_structure").and_then(Value::as_bool).unwrap_or(false);
        self.raw_mep = settings.get("mep").cloned().unwrap_or(Value::Null);
        self.show_mep = settings.get("show_mep").and_then(Value::as_bool).unwrap_or(false);
        self.raw_boq = settings.get("boq").cloned().unwrap_or(Value::Null);
        let s: StructIn = serde_json::from_value(self.raw_struct.clone()).unwrap_or_default();
        let so = struct_opts(&s);
        // Say plainly what is wrong with the file instead of hiding it.
        let mut warnings: Vec<String> = nnx_core::typed::validate_model(&doc.model).iter().take(5).map(|i| format!("{} {}: {}", i.kind, i.id, i.message)).collect();
        {
            let mut probe = Document::new(doc.model.clone());
            match levels::settle(&mut probe, &so, None) {
                Ok((cmds, _)) if !cmds.is_empty() => warnings.push("the structure does not match the walls: press Sync".into()),
                Ok(_) => {}
                Err(e) => warnings.push(format!("the structure cannot be checked: {e}")),
            }
        }
        // A file without a saved brief (made elsewhere) still opens; only regenerating alternatives needs it.
        let n_levels = building::levels(&doc.model).len().max(1) as u32;
        self.brief = serde_json::from_value::<BriefIn>(self.raw_brief.clone()).ok().and_then(|b| levels::plan_building(&b, n_levels).ok()).map(|p| p.merged);
        self.alternatives.clear();
        self.plan = None;
        self.cur_level = 0;
        self.sopts = Some(so);
        self.doc = Some(doc);
        Ok(json!({ "svg": self.render_or_null()?, "warnings": warnings, "rooms": self.rooms_json(), "levels": self.levels_json(), "settings": settings }))
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
        let before = hash(&app);
        // Compute only: nothing enters the model.
        let r = app.handle("mep", &json!({ "inputs": mep_inputs(), "apply": false, "show": true })).unwrap();
        assert!(r["report"]["electrical"].is_object() && r["report"]["plumbing"].is_object() && r["report"]["hvac"].is_object());
        assert_eq!(r["report"]["missing"], json!({}));
        assert_eq!(mep_points(&app), 0);
        assert_eq!(hash(&app), before);
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
        assert_eq!(opened["settings"]["mep"]["electrical"]["voltage_v"], 230);
        assert_eq!(mep_points(&second), n);
        assert_eq!(second.handle("qto", &json!({})).unwrap()["csv"], q["csv"]);
        // One undo removes every point.
        assert_eq!(app.handle("undo", &json!({})).unwrap()["changed"], true);
        assert_eq!(hash(&app), before);
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
        assert_eq!(opened["settings"]["boq"]["markups"][0]["pct"], 15);
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
        assert_eq!(opened["settings"]["brief"]["stair"]["side"], "west");
        assert_eq!(second.handle("sync", &json!({})).unwrap()["created"], 0);
        assert_eq!(second.handle("qto", &json!({})).unwrap()["csv"], q["csv"]);
        assert!(second.brief.is_some(), "edits still work after reopening");
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
