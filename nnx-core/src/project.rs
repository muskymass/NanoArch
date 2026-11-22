//! `NanoArchProject`: everything a project contains besides the drawing elements, stored as ONE element of kind
//! `Project` inside the model. Because it is an element, every change to it is a `SetProp` command on the
//! document's stack: `undo`, `redo`, `is_dirty` (a hash of the whole model) and the `.nnx` file (which stores every
//! element) cover it with no extra bookkeeping, and the 2D view, 3D view, quantities and schedule all read the same
//! value.
//!
//! The project holds the metadata (building type, style, smart technology, climate zone, plot outline), the
//! architectural brief, the structural, MEP and BOQ inputs, the room display names and the limits the municipality
//! sets (setbacks, plot coverage), each with its source. Nothing here supplies an engineering value.

use crate::boq::BoqLib;
use crate::mep::MepInputs;
use crate::model::{Discipline, Element, Id, Model, Source};
use crate::rules::Param;
use crate::typed::{Typed, element_of};
use crate::{Command, Document, Origin};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildingType {
    Villa,
    House,
    Palace,
    ApartmentBuilding,
}

impl BuildingType {
    /// `(English, Arabic)`.
    pub fn label(self) -> (&'static str, &'static str) {
        match self {
            BuildingType::Villa => ("Villa", "فيلا"),
            BuildingType::House => ("House", "منزل"),
            BuildingType::Palace => ("Palace", "قصر"),
            BuildingType::ApartmentBuilding => ("Apartment building", "عمارة سكنية"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectMeta {
    pub title: String,
    pub building_type: Option<BuildingType>,
    /// Architectural style, free text chosen by the user.
    pub style: Option<String>,
    /// Smart-technology options chosen by the user (for example `"lighting"`, `"access"`).
    pub smart_tech: Vec<String>,
    /// Climate zone as named by the code the user works with; the climate-dependent rules read it.
    pub climate_zone: Option<String>,
    /// The plot outline in metres: any simple polygon. The planner draws rectangles, so the building is the largest rectangle in
    /// the buildable polygon (see `site`); the brief's width and length are that rectangle when the outline is not a rectangle.
    pub plot_polygon_m: Option<Vec<[f64; 2]>>,
    /// What each edge of the outline is (front street, side street, neighbour, plaza), its street width and any setback you entered.
    pub plot_edges: Vec<crate::site::Edge>,
    /// The setback rule set for the edges that carry no entered setback.
    pub setback_rule: Option<crate::site::RuleSet>,
    /// Height of the roof parapet in metres, drawn on elevations and sections. Unset draws none (it is not defaulted).
    pub parapet_height_m: Option<f64>,
    /// Keys of the rooms whose place is locked: regenerating the layout keeps them where they are.
    pub locked_rooms: Vec<String>,
    /// Ids of the walls that cannot be moved or have their openings changed.
    pub locked_walls: Vec<String>,
    /// Boundary walls, gate, guard room, carport, pool and courtyard (all sizes entered by the user).
    pub site_layout: Option<crate::site_layout::SiteLayoutInput>,
    /// The walkway clearance for the furniture check.
    pub furniture: Option<crate::furniture::FurnitureInput>,
    /// Elevator shaft, basement levels with a ramp, and a roof annex.
    pub vertical: Option<crate::vertical::VerticalInput>,
    /// Inputs of the compliance pack: thermal layers, accessibility limits, ventilation, projections, the Malath checklist.
    pub compliance: crate::compliance::ComplianceInputs,
    /// Wall assemblies (named wall types with explicit layers) and which of them walls fall back to.
    pub assemblies: Vec<crate::assembly::WallAssembly>,
    pub assembly_defaults: crate::assembly::AssemblyDefaults,
    /// The change log's baseline snapshot and the closed revisions.
    pub baseline: Option<crate::compliance::Baseline>,
    pub revision_history: Vec<crate::compliance::RevisionEntry>,
    /// Inputs of the advanced structural pack: column grid, slab systems, punching, merging, tanks, rebar cutting.
    pub structural: crate::structx::StructuralInputs,
    /// Inputs of the advanced MEP pack: solar, smart home, Wi-Fi, CCTV, EV chargers, balancing, greywater, head loss.
    pub mep_ext: crate::mepx::MepExtInputs,
    /// The facade and floor finishes chosen for the 3D view, the quantities and the BOQ.
    pub finishes: crate::finishes::Finishes,
    /// Measured coordinates and dimensions compared with the design.
    pub as_built: crate::asbuilt::AsBuilt,
    /// Milestone 6 inputs: the schedule setup, cost tiers, tender pack, payment milestones, long-lead items, QA/QC records and the apartment ROI.
    pub execution: crate::execution::ExecutionInputs,
}

/// Limits set by the municipality for this plot. Each carries the source the user took it from. A limit without a
/// value is unset: the check that needs it is reported as skipped, never as passed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MunicipalLimits {
    pub min_setback_front_m: Option<Param>,
    pub min_setback_back_m: Option<Param>,
    pub min_setback_side_m: Option<Param>,
    /// Maximum built footprint divided by plot area (0..1).
    pub max_coverage_ratio: Option<Param>,
    /// When true, an edit that adds a code violation is refused. When false it is accepted and the violation reported.
    pub enforce: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Construction phase of the room (one of `qto::PHASES`); absent: none is assumed. Kept here so the phases survive a reopened project.
    #[serde(default)]
    pub phase: Option<String>,
}

/// The stair of a building with several floors. Nothing here is defaulted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StairIn {
    pub width_m: f64,
    pub length_m: f64,
    /// `west` or `east`.
    pub side: String,
}

/// The architectural brief exactly as the user entered it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Wall thickness of the drawings (mm). Both or none: with one missing the engine's default is used for it.
    #[serde(default)]
    pub wall_exterior_mm: Option<i64>,
    #[serde(default)]
    pub wall_interior_mm: Option<i64>,
    /// Wall height = storey height (mm). None keeps the engine's default (3000); it must be positive.
    #[serde(default)]
    pub wall_height_mm: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StructIn {
    pub dead_kpa: Option<f64>,
    pub live_kpa: Option<f64>,
    pub bearing_kpa: Option<f64>,
    pub floors: Option<u32>,
    pub max_span_mm: Option<i64>,
    #[serde(default)]
    pub show_structure: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NanoArchProject {
    pub meta: ProjectMeta,
    pub brief: Option<BriefIn>,
    pub structure: StructIn,
    pub mep: Option<MepInputs>,
    pub boq: Option<BoqLib>,
    /// Inputs of the structural schedules (footing thickness, tie beams, reinforcement templates).
    pub sizing: Option<crate::sizing::SizingInputs>,
    /// Room key -> display name.
    pub labels: BTreeMap<String, String>,
    pub limits: MunicipalLimits,
    /// Named versions of the project (their own top-level property, so an edit elsewhere does not copy them into the undo history).
    pub snapshots: Vec<crate::snapshot::Snapshot>,
    /// Municipal and authority rule overlays loaded by the user, in the order they apply.
    pub overlays: Vec<crate::overlay::Overlay>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl NanoArchProject {
    /// Plot area in m2: the outline's area when there is one, otherwise the entered width times length.
    pub fn plot_area_m2(&self) -> Option<f64> {
        match &self.meta.plot_polygon_m {
            Some(p) if p.len() >= 3 => Some(crate::site::signed_area(p).abs()),
            _ => self.brief.as_ref().map(|b| b.width_m * b.length_m),
        }
    }

    /// True when the plot is an outline that is not a rectangle.
    pub fn plot_is_irregular(&self) -> bool {
        self.meta.plot_polygon_m.as_ref().is_some_and(|p| p.len() >= 3 && rectangle_size(p).is_none())
    }

    /// The site description of the outline, for `site::analyse`.
    pub fn site_input(&self) -> Option<crate::site::SiteInput> {
        let polygon = self.meta.plot_polygon_m.clone()?;
        Some(crate::site::SiteInput { polygon_m: polygon, edges: self.meta.plot_edges.clone(), rule: self.meta.setback_rule.unwrap_or_default(), floors: self.structure.floors })
    }

    /// The site analysis of this project: its outline and rule, or the rectangle with the setbacks entered in the brief.
    pub fn site_result(&self) -> Option<Result<crate::site::SiteResult, Vec<String>>> {
        let input = match self.site_input() {
            Some(i) => i,
            None => {
                let b = self.brief.as_ref()?;
                let mut i = crate::site::rectangle_with_setbacks(b.width_m, b.length_m, b.setback_front_m, b.setback_back_m, b.setback_side_m);
                i.floors = self.structure.floors;
                i
            }
        };
        Some(crate::site::analyse(&input))
    }

    /// Everything wrong with the project's own data. A project with errors is never stored.
    pub fn check(&self) -> Result<(), String> {
        if let Some(s) = &self.meta.site_layout {
            s.check()?;
        }
        if let Some(v) = &self.meta.vertical {
            v.check()?;
        }
        self.meta.compliance.check()?;
        crate::assembly::check_all(&self.meta.assemblies, &self.meta.assembly_defaults)?;
        self.meta.structural.check()?;
        self.meta.mep_ext.check()?;
        self.meta.finishes.check()?;
        self.meta.as_built.check()?;
        self.meta.execution.check()?;
        crate::snapshot::check_all(&self.snapshots)?;
        crate::overlay::check_all(&self.overlays)?;
        if let Some(f) = &self.meta.furniture
            && f.min_clear_mm.is_some_and(|m| !m.is_finite() || m <= 0.0)
        {
            return Err("the minimum clear walkway must be a positive number of millimetres".into());
        }
        if let Some(b) = &self.brief {
            for (name, v) in [("plot width", b.width_m), ("plot length", b.length_m)] {
                if !v.is_finite() || v <= 0.0 {
                    return Err(format!("{name} must be a positive number of metres"));
                }
            }
            for (name, v) in [("front setback", b.setback_front_m), ("rear setback", b.setback_back_m), ("side setback", b.setback_side_m)] {
                if !v.is_finite() || v < 0.0 {
                    return Err(format!("the {name} must be zero or more"));
                }
            }
            if b.width_m - 2.0 * b.setback_side_m <= 0.0 || b.length_m - b.setback_front_m - b.setback_back_m <= 0.0 {
                return Err("the setbacks leave no buildable area".into());
            }
            if let Some(r) = b.rooms.iter().find(|r| !r.area.is_finite() || r.area <= 0.0) {
                return Err(format!("room '{}' needs a positive area", r.key));
            }
        }
        if let Some(p) = self.meta.parapet_height_m
            && (!p.is_finite() || p <= 0.0 || p > 5.0)
        {
            return Err("the parapet height must be between 0 and 5 metres".into());
        }
        if let Some(poly) = &self.meta.plot_polygon_m {
            if poly.len() < 3 || poly.iter().flatten().any(|v| !v.is_finite()) {
                return Err("the plot outline needs at least 3 vertices, each a pair of numbers".into());
            }
            if !crate::site::is_simple(poly) || crate::site::signed_area(poly).abs() < 1e-6 {
                return Err("the plot outline crosses itself or has no area".into());
            }
            if !self.meta.plot_edges.is_empty() && self.meta.plot_edges.len() != poly.len() {
                return Err(format!("the plot outline has {} edges but {} edge descriptions were given", poly.len(), self.meta.plot_edges.len()));
            }
            // A rectangle written as an outline must agree with the brief; any other shape sets the brief's building size itself.
            if let (Some((w, l)), Some(b)) = (rectangle_size(poly), &self.brief)
                && ((w - b.width_m).abs() > 0.001 || (l - b.length_m).abs() > 0.001)
            {
                return Err(format!("the plot outline is {w} x {l} m but the brief says {} x {} m", b.width_m, b.length_m));
            }
        }
        let l = &self.limits;
        for (name, p, max) in [
            ("front setback limit", &l.min_setback_front_m, f64::INFINITY),
            ("rear setback limit", &l.min_setback_back_m, f64::INFINITY),
            ("side setback limit", &l.min_setback_side_m, f64::INFINITY),
            ("coverage ratio limit", &l.max_coverage_ratio, 1.0),
        ] {
            let Some(p) = p else { continue };
            let Some(v) = p.value else { continue };
            if !v.is_finite() || v < 0.0 || v > max {
                return Err(format!("the {name} {v} is out of range"));
            }
            if p.source.trim().is_empty() {
                return Err(format!("the {name} needs its source (regulation, article, municipality)"));
            }
        }
        Ok(())
    }
}

/// `(width, length)` in metres if the points are the four corners of an axis-aligned rectangle.
fn rectangle_size(poly: &[[f64; 2]]) -> Option<(f64, f64)> {
    if poly.len() != 4 || poly.iter().flatten().any(|v| !v.is_finite()) {
        return None;
    }
    let xs = poly.iter().map(|p| p[0]);
    let ys = poly.iter().map(|p| p[1]);
    let (x0, x1) = (xs.clone().fold(f64::INFINITY, f64::min), xs.fold(f64::NEG_INFINITY, f64::max));
    let (y0, y1) = (ys.clone().fold(f64::INFINITY, f64::min), ys.fold(f64::NEG_INFINITY, f64::max));
    let corner = |x: f64, y: f64| poly.iter().any(|p| (p[0] - x).abs() < 1e-9 && (p[1] - y).abs() < 1e-9);
    (x1 > x0 && y1 > y0 && corner(x0, y0) && corner(x1, y0) && corner(x1, y1) && corner(x0, y1)).then_some((x1 - x0, y1 - y0))
}

impl Typed for NanoArchProject {
    const KIND: &'static str = "Project";
    const DISCIPLINE: Discipline = Discipline::Site;
    fn validate(&self, model: &Model, el: &Element) -> Result<(), String> {
        if model.elements.values().any(|o| o.id != el.id && o.kind == Self::KIND) {
            return Err("a model holds exactly one project element".into());
        }
        self.check()
    }
}

/// The project element of the model.
pub fn find(model: &Model) -> Option<&Element> {
    model.elements.values().find(|e| e.kind == NanoArchProject::KIND)
}

/// The project stored in the model. `Ok(None)` if the model has none (an empty model, or a file from before projects
/// were stored in the model).
pub fn get(model: &Model) -> Result<Option<NanoArchProject>, String> {
    find(model).map(|e| e.typed::<NanoArchProject>().map_err(|err| format!("project: {err}"))).transpose()
}

/// The command that adds `p` to a model as its project element.
pub fn create_command(p: &NanoArchProject) -> Result<Command, String> {
    p.check()?;
    Ok(Command::Create(element_of(p, Source::User)))
}

/// What `update` did.
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    /// False when the edit left every value as it was (nothing was committed, so nothing became dirty).
    pub changed: bool,
    /// Code violations the edit introduced (accepted because the project does not enforce the code).
    pub new_violations: Vec<crate::sbc::Violation>,
}

/// Changes the project through the command stack as ONE undoable step. The edit is applied to a copy first:
/// the project's own checks, the typed schema and (when the project enforces the code) the code checks must pass,
/// or nothing changes.
pub fn update(doc: &mut Document, label: &str, edit: impl FnOnce(&mut NanoArchProject) -> Result<(), String>) -> Result<Update, String> {
    let cmds = update_commands(&doc.model, edit)?;
    if cmds.is_empty() {
        return Ok(Update { changed: false, new_violations: Vec::new() });
    }
    let mut scratch = Document::new(doc.model.clone());
    scratch.commit(label, Origin::User, cmds.clone()).map_err(|e| e.to_string())?;
    let added = crate::sbc::introduced(&doc.model, &scratch.model);
    if let Some(why) = crate::sbc::refusal(&doc.model, &scratch.model) {
        return Err(why);
    }
    doc.commit(label, Origin::User, cmds).map_err(|e| e.to_string())?;
    Ok(Update { changed: true, new_violations: added })
}

/// The commands an edit of the project amounts to (one `SetProp` per top-level property that differs), checked but not committed, so a
/// caller can commit them together with edits of other elements as one undoable step.
pub fn update_commands(model: &Model, edit: impl FnOnce(&mut NanoArchProject) -> Result<(), String>) -> Result<Vec<Command>, String> {
    let el = find(model).ok_or("this model has no project")?;
    let before: NanoArchProject = el.typed().map_err(|e| format!("project: {e}"))?;
    let mut after = before.clone();
    edit(&mut after)?;
    after.check()?;
    Ok(changed_props(el.id, &props_of(&before), &props_of(&after)))
}

fn props_of(p: &NanoArchProject) -> Map<String, Value> {
    match serde_json::to_value(p) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

/// One `SetProp` per top-level property that differs.
fn changed_props(id: Id, old: &Map<String, Value>, new: &Map<String, Value>) -> Vec<Command> {
    let mut cmds = Vec::new();
    for (k, v) in new {
        if old.get(k) != Some(v) {
            cmds.push(Command::SetProp { id, key: k.clone(), value: Some(v.clone()) });
        }
    }
    for k in old.keys().filter(|k| !new.contains_key(*k)) {
        cmds.push(Command::SetProp { id, key: k.clone(), value: None });
    }
    cmds
}
