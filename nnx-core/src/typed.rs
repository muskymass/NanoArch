//! Typed views of the model's elements.
//!
//! Storage stays a JSON property map (so `.nnx` files and unknown or future properties
//! survive), but known kinds have a typed struct with the same property names. A typed
//! struct can be read from any element of its kind, and rejects missing or mistyped
//! fields. Every typed struct carries an `extra` map so properties it does not know
//! are preserved on a round trip.
//!
//! Each kind also has semantic checks (a wall is axis-aligned and its length matches its
//! end points; a door fits inside its host wall; a space's area matches its rectangle).
//! `Document` runs these on every commit, so a model edited through commands cannot hold
//! an invalid wall, door, window or space.

use crate::model::{Discipline, Element, Id, Model, Source};
use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

/// Whole millimetres. Accepts `1500` and `1500.0`; rejects `1500.5`.
fn mm<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    let v = f64::deserialize(d)?;
    if v.fract() != 0.0 || !v.is_finite() {
        return Err(D::Error::custom(format!("{v} is not a whole number of millimetres")));
    }
    Ok(v as i64)
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TypedError {
    #[error("expected kind {expected}, found {found}")]
    WrongKind { expected: &'static str, found: String },
    #[error("{0}")]
    Invalid(String),
}

pub trait Typed: Serialize + DeserializeOwned + Sized {
    const KIND: &'static str;
    const DISCIPLINE: Discipline;
    /// Semantic checks beyond field types. `el` is the element holding this value.
    fn validate(&self, _model: &Model, _el: &Element) -> Result<(), String> {
        Ok(())
    }
}

impl Element {
    /// Reads this element as `T`. Fails if the kind differs or a field is missing or mistyped.
    pub fn typed<T: Typed>(&self) -> Result<T, TypedError> {
        if self.kind != T::KIND {
            return Err(TypedError::WrongKind { expected: T::KIND, found: self.kind.clone() });
        }
        serde_json::from_value(Value::Object(self.props.clone())).map_err(|e| TypedError::Invalid(e.to_string()))
    }
}

/// A new element (fresh id, `rev` 0) holding `t`. Set `level_id` / `host_id` on the result as needed.
pub fn element_of<T: Typed>(t: &T, source: Source) -> Element {
    let Value::Object(props) = serde_json::to_value(t).expect("typed element serializes") else {
        unreachable!("typed elements serialize to objects")
    };
    let mut el = Element::new(T::KIND, T::DISCIPLINE, source);
    el.props = props;
    el
}

fn opt_is_none<T>(o: &Option<T>) -> bool {
    o.is_none()
}

// ---------------------------------------------------------------- Space

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Space {
    pub func: String,
    pub requirement_key: String,
    /// Gross area, m2. Derived from the rectangle; must match it.
    pub area: f64,
    pub target_area: f64,
    #[serde(deserialize_with = "mm")]
    pub x: i64,
    #[serde(deserialize_with = "mm")]
    pub y: i64,
    #[serde(deserialize_with = "mm")]
    pub w: i64,
    #[serde(deserialize_with = "mm")]
    pub h: i64,
    pub boundary: Vec<[i64; 2]>,
    /// Net area after wall thickness, m2.
    #[serde(default, skip_serializing_if = "opt_is_none")]
    pub net_area: Option<f64>,
    #[serde(default, skip_serializing_if = "opt_is_none")]
    pub glazing_ratio: Option<f64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Typed for Space {
    const KIND: &'static str = "Space";
    const DISCIPLINE: Discipline = Discipline::Arc;
    fn validate(&self, _m: &Model, _e: &Element) -> Result<(), String> {
        if self.func.is_empty() || self.requirement_key.is_empty() {
            return Err("func and requirementKey must not be empty".into());
        }
        if self.w <= 0 || self.h <= 0 {
            return Err(format!("rectangle {}x{} mm has no area", self.w, self.h));
        }
        let expect = self.w as f64 * self.h as f64 / 1e6;
        if (self.area - expect).abs() > 0.011 {
            return Err(format!("area {} m2 does not match the {}x{} mm rectangle ({expect:.2} m2)", self.area, self.w, self.h));
        }
        if let Some(n) = self.net_area
            && (n < 0.0 || n > self.area + 0.011) {
                return Err(format!("netArea {n} must be between 0 and the gross area {}", self.area));
            }
        Ok(())
    }
}

// ---------------------------------------------------------------- Wall

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Wall {
    #[serde(deserialize_with = "mm")]
    pub x1: i64,
    #[serde(deserialize_with = "mm")]
    pub y1: i64,
    #[serde(deserialize_with = "mm")]
    pub x2: i64,
    #[serde(deserialize_with = "mm")]
    pub y2: i64,
    #[serde(deserialize_with = "mm")]
    pub thickness: i64,
    #[serde(deserialize_with = "mm")]
    pub height: i64,
    /// Derived from the end points; must match them.
    #[serde(deserialize_with = "mm")]
    pub length: i64,
    pub is_exterior: bool,
    pub room_a: String,
    #[serde(default, skip_serializing_if = "opt_is_none")]
    pub room_b: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Wall {
    pub fn horizontal(&self) -> bool {
        self.y1 == self.y2
    }
    /// Coordinate of the wall's start along its length (the smaller end).
    pub fn start(&self) -> i64 {
        if self.horizontal() { self.x1.min(self.x2) } else { self.y1.min(self.y2) }
    }
    /// Coordinate of the centre line across the wall.
    pub fn centre_line(&self) -> i64 {
        if self.horizontal() { self.y1 } else { self.x1 }
    }
}

impl Typed for Wall {
    const KIND: &'static str = "Wall";
    const DISCIPLINE: Discipline = Discipline::Arc;
    fn validate(&self, _m: &Model, _e: &Element) -> Result<(), String> {
        if (self.x1 == self.x2) == (self.y1 == self.y2) {
            return Err("a wall must be axis-aligned and not zero-length".into());
        }
        let len = (self.x2 - self.x1).abs() + (self.y2 - self.y1).abs();
        if self.length != len {
            return Err(format!("length {} does not match the end points ({len})", self.length));
        }
        if self.thickness <= 0 || self.height <= 0 {
            return Err("thickness and height must be positive".into());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- Openings

/// The wall hosting an opening, checked to exist and to be a wall.
fn host_wall(model: &Model, el: &Element) -> Result<Wall, String> {
    let id = el.host_id.ok_or("must be hosted by a wall")?;
    let host = model.get(&id).ok_or_else(|| format!("host {id} does not exist"))?;
    host.typed::<Wall>().map_err(|e| format!("host {id} is not a valid wall: {e}"))
}

fn fits(offset: i64, width: i64, wall: &Wall) -> Result<(), String> {
    if width <= 0 {
        return Err("width must be positive".into());
    }
    if offset - width / 2 < 0 || offset + width / 2 > wall.length {
        return Err(format!("opening ({width} mm at {offset} mm) does not fit the {} mm wall", wall.length));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Door {
    #[serde(deserialize_with = "mm")]
    pub width: i64,
    #[serde(deserialize_with = "mm")]
    pub height: i64,
    /// Distance from the host wall's start to the door centre.
    #[serde(deserialize_with = "mm")]
    pub offset_center: i64,
    pub is_egress: bool,
    pub from: String,
    pub to: String,
    pub opens_into: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Typed for Door {
    const KIND: &'static str = "Door";
    const DISCIPLINE: Discipline = Discipline::Arc;
    fn validate(&self, model: &Model, el: &Element) -> Result<(), String> {
        let wall = host_wall(model, el)?;
        fits(self.offset_center, self.width, &wall)?;
        if self.height <= 0 || self.height > wall.height {
            return Err(format!("height {} mm must be positive and within the wall height {}", self.height, wall.height));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    #[serde(deserialize_with = "mm")]
    pub width: i64,
    #[serde(deserialize_with = "mm")]
    pub height: i64,
    #[serde(deserialize_with = "mm")]
    pub sill_height: i64,
    #[serde(deserialize_with = "mm")]
    pub offset_center: i64,
    pub room: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Typed for Window {
    const KIND: &'static str = "Window";
    const DISCIPLINE: Discipline = Discipline::Arc;
    fn validate(&self, model: &Model, el: &Element) -> Result<(), String> {
        let wall = host_wall(model, el)?;
        fits(self.offset_center, self.width, &wall)?;
        if self.height <= 0 || self.sill_height < 0 || self.sill_height + self.height > wall.height {
            return Err(format!("sill {} + height {} must fit the wall height {}", self.sill_height, self.height, wall.height));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- Structure

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    /// Centre of the column.
    #[serde(deserialize_with = "mm")]
    pub x: i64,
    #[serde(deserialize_with = "mm")]
    pub y: i64,
    #[serde(deserialize_with = "mm")]
    pub b: i64,
    #[serde(deserialize_with = "mm")]
    pub h: i64,
    pub grid_ref: String,
    /// Service axial load from the preliminary load take-down, kN.
    #[serde(default, skip_serializing_if = "opt_is_none")]
    pub axial_service_kn: Option<f64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Typed for Column {
    const KIND: &'static str = "Column";
    const DISCIPLINE: Discipline = Discipline::Str;
    fn validate(&self, _m: &Model, _e: &Element) -> Result<(), String> {
        if self.b <= 0 || self.h <= 0 {
            return Err("column section must be positive".into());
        }
        if self.axial_service_kn.is_some_and(|p| p < 0.0) {
            return Err("axial load must not be negative".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Beam {
    #[serde(deserialize_with = "mm")]
    pub x1: i64,
    #[serde(deserialize_with = "mm")]
    pub y1: i64,
    #[serde(deserialize_with = "mm")]
    pub x2: i64,
    #[serde(deserialize_with = "mm")]
    pub y2: i64,
    /// Section width and total depth.
    #[serde(deserialize_with = "mm")]
    pub b: i64,
    #[serde(deserialize_with = "mm")]
    pub h: i64,
    /// Derived from the end points; must match them.
    #[serde(deserialize_with = "mm")]
    pub length: i64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Typed for Beam {
    const KIND: &'static str = "Beam";
    const DISCIPLINE: Discipline = Discipline::Str;
    fn validate(&self, _m: &Model, _e: &Element) -> Result<(), String> {
        if (self.x1 == self.x2) == (self.y1 == self.y2) {
            return Err("a beam must be axis-aligned and not zero-length".into());
        }
        let len = (self.x2 - self.x1).abs() + (self.y2 - self.y1).abs();
        if self.length != len {
            return Err(format!("length {} does not match the end points ({len})", self.length));
        }
        if self.b <= 0 || self.h <= 0 {
            return Err("beam section must be positive".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Slab {
    #[serde(deserialize_with = "mm")]
    pub x: i64,
    #[serde(deserialize_with = "mm")]
    pub y: i64,
    #[serde(deserialize_with = "mm")]
    pub w: i64,
    #[serde(deserialize_with = "mm")]
    pub h: i64,
    #[serde(deserialize_with = "mm")]
    pub thickness: i64,
    pub kind: String,
    /// Requirement key of the room this slab covers.
    pub room: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Typed for Slab {
    const KIND: &'static str = "Slab";
    const DISCIPLINE: Discipline = Discipline::Str;
    fn validate(&self, _m: &Model, _e: &Element) -> Result<(), String> {
        if self.w <= 0 || self.h <= 0 || self.thickness <= 0 {
            return Err("slab size and thickness must be positive".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Footing {
    /// Centre of the footing.
    #[serde(deserialize_with = "mm")]
    pub x: i64,
    #[serde(deserialize_with = "mm")]
    pub y: i64,
    #[serde(deserialize_with = "mm")]
    pub length: i64,
    #[serde(deserialize_with = "mm")]
    pub width: i64,
    pub service_load_kn: f64,
    /// Allowable bearing pressure used for sizing, kPa. Must come from the soil report.
    pub bearing_kpa: f64,
    /// Id of the column this footing carries.
    pub column: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Typed for Footing {
    const KIND: &'static str = "Footing";
    const DISCIPLINE: Discipline = Discipline::Str;
    fn validate(&self, model: &Model, _e: &Element) -> Result<(), String> {
        if self.length <= 0 || self.width <= 0 || self.service_load_kn <= 0.0 || self.bearing_kpa <= 0.0 {
            return Err("footing size, load and bearing pressure must be positive".into());
        }
        let area = self.length as f64 / 1000.0 * self.width as f64 / 1000.0;
        if area * self.bearing_kpa + 1e-6 < self.service_load_kn {
            return Err(format!(
                "footing {area:.2} m2 at {} kPa carries {:.0} kN, less than the {} kN load",
                self.bearing_kpa,
                area * self.bearing_kpa,
                self.service_load_kn
            ));
        }
        let col: Id = self.column.parse().map_err(|_| format!("column '{}' is not an element id", self.column))?;
        match model.get(&col) {
            Some(c) if c.kind == Column::KIND => Ok(()),
            _ => Err(format!("column {col} does not exist")),
        }
    }
}

// ---------------------------------------------------------------- MEP

/// A point of an MEP system placed in a room: a light, socket, fixture, panel or air-conditioning unit.
/// Its position and the loads it carries come from the MEP engines; the room is a requirement key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MepPoint {
    /// `light`, `socket`, `panel`, `fixture`, `ac_indoor`.
    pub role: String,
    #[serde(deserialize_with = "mm")]
    pub x: i64,
    #[serde(deserialize_with = "mm")]
    pub y: i64,
    pub room: String,
    #[serde(default, skip_serializing_if = "opt_is_none")]
    pub watts: Option<f64>,
    /// Circuit or system label, when the point belongs to one.
    #[serde(default, skip_serializing_if = "opt_is_none")]
    pub circuit: Option<String>,
    /// Fixture type or unit description.
    #[serde(default, skip_serializing_if = "opt_is_none")]
    pub label: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Typed for MepPoint {
    const KIND: &'static str = "MepPoint";
    const DISCIPLINE: Discipline = Discipline::Ele;
    fn validate(&self, model: &Model, _e: &Element) -> Result<(), String> {
        if self.role.is_empty() {
            return Err("role must not be empty".into());
        }
        if self.watts.is_some_and(|w| w < 0.0) {
            return Err("watts must not be negative".into());
        }
        // The point stands inside the room it names.
        let room = model.elements.values().filter(|e| e.kind == Space::KIND).filter_map(|e| e.typed::<Space>().ok()).find(|s| s.requirement_key == self.room);
        match room {
            Some(s) if self.x >= s.x && self.x <= s.x + s.w && self.y >= s.y && self.y <= s.y + s.h => Ok(()),
            Some(s) => Err(format!("point ({}, {}) is outside room '{}' ({}..{} x {}..{})", self.x, self.y, self.room, s.x, s.x + s.w, s.y, s.y + s.h)),
            None => Err(format!("room '{}' does not exist", self.room)),
        }
    }
}

/// A storey of the building. `index` 0 is the ground level. Elements belong to a level through their `level_id`;
/// the elevation of a level is derived from the heights below it (see `model3d`), never stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Level {
    pub name: String,
    pub index: i64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Typed for Level {
    const KIND: &'static str = "Level";
    const DISCIPLINE: Discipline = Discipline::Arc;
    fn validate(&self, model: &Model, e: &Element) -> Result<(), String> {
        if self.index < 0 {
            return Err("a level index must not be negative".into());
        }
        let clash = model.elements.values().any(|o| o.id != e.id && o.kind == Level::KIND && o.props.get("index").and_then(|v| v.as_i64()) == Some(self.index));
        if clash { Err(format!("another level already has index {}", self.index)) } else { Ok(()) }
    }
}

// ---------------------------------------------------------------- MEP runs

/// A straight duct, pipe or drain between two points, an axis-aligned box in 3D. Heights are millimetres above the floor
/// top of the element's level (negative: below the floor, for drains). The cross-section is `width` x `height`; a round
/// pipe uses its outer diameter for both. Runs are entered or imported: the application does not route them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MepRun {
    /// `duct`, `pipe` or `drain`.
    pub system: String,
    #[serde(deserialize_with = "mm")]
    pub x1: i64,
    #[serde(deserialize_with = "mm")]
    pub y1: i64,
    #[serde(deserialize_with = "mm")]
    pub z1: i64,
    #[serde(deserialize_with = "mm")]
    pub x2: i64,
    #[serde(deserialize_with = "mm")]
    pub y2: i64,
    #[serde(deserialize_with = "mm")]
    pub z2: i64,
    #[serde(deserialize_with = "mm")]
    pub width: i64,
    #[serde(deserialize_with = "mm")]
    pub height: i64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl MepRun {
    /// The discipline a run belongs to: ducts are HVAC, pipes plumbing, drains drainage.
    pub fn discipline_of(system: &str) -> Option<Discipline> {
        match system {
            "duct" => Some(Discipline::Hvac),
            "pipe" => Some(Discipline::Plb),
            "drain" => Some(Discipline::Drn),
            _ => None,
        }
    }
}

impl Typed for MepRun {
    const KIND: &'static str = "MepRun";
    const DISCIPLINE: Discipline = Discipline::Hvac;
    fn validate(&self, _m: &Model, el: &Element) -> Result<(), String> {
        let expected = MepRun::discipline_of(&self.system).ok_or_else(|| format!("unknown run system '{}' (duct, pipe or drain)", self.system))?;
        if el.discipline != expected {
            return Err(format!("a {} belongs to the {:?} discipline", self.system, expected));
        }
        if self.width <= 0 || self.height <= 0 {
            return Err("run cross-section must be positive".into());
        }
        let differing = [self.x1 != self.x2, self.y1 != self.y2, self.z1 != self.z2].iter().filter(|d| **d).count();
        if differing != 1 {
            return Err("a run must be straight along one axis and not zero-length".into());
        }
        Ok(())
    }
}

/// A new run element of the right discipline.
pub fn run_element(run: &MepRun, level: Option<Id>, source: Source) -> Result<Element, String> {
    let mut el = element_of(run, source);
    el.discipline = MepRun::discipline_of(&run.system).ok_or_else(|| format!("unknown run system '{}' (duct, pipe or drain)", run.system))?;
    el.level_id = level;
    Ok(el)
}

// ---------------------------------------------------------------- model-level checks

/// Validates one element if its kind is a known typed kind; unknown kinds pass.
pub fn validate_element(model: &Model, el: &Element) -> Result<(), String> {
    fn run<T: Typed>(model: &Model, el: &Element) -> Result<(), String> {
        el.typed::<T>().map_err(|e| e.to_string())?.validate(model, el)
    }
    match el.kind.as_str() {
        Space::KIND => run::<Space>(model, el),
        Wall::KIND => run::<Wall>(model, el),
        Door::KIND => run::<Door>(model, el),
        Window::KIND => run::<Window>(model, el),
        Column::KIND => run::<Column>(model, el),
        Beam::KIND => run::<Beam>(model, el),
        Slab::KIND => run::<Slab>(model, el),
        Footing::KIND => run::<Footing>(model, el),
        MepPoint::KIND => run::<MepPoint>(model, el),
        Level::KIND => run::<Level>(model, el),
        MepRun::KIND => run::<MepRun>(model, el),
        crate::project::NanoArchProject::KIND => run::<crate::project::NanoArchProject>(model, el),
        _ => Ok(()),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub id: Id,
    pub kind: String,
    pub message: String,
}

/// Validates every element, plus model-wide rules (space keys are unique).
pub fn validate_model(model: &Model) -> Vec<Issue> {
    let mut issues: Vec<Issue> = model
        .elements
        .values()
        .filter_map(|el| {
            validate_element(model, el).err().map(|message| Issue { id: el.id, kind: el.kind.clone(), message })
        })
        .collect();
    let mut seen = std::collections::BTreeMap::new();
    for el in model.elements.values().filter(|e| e.kind == Space::KIND) {
        if let Ok(s) = el.typed::<Space>()
            && let Some(first) = seen.insert(s.requirement_key.clone(), el.id) {
                issues.push(Issue {
                    id: el.id,
                    kind: el.kind.clone(),
                    message: format!("requirementKey '{}' is already used by space {first}", s.requirement_key),
                });
            }
    }
    issues
}
