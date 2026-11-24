//! Opening families: named door and window types with explicit sizes, stored in the project (`meta.openingFamilies`).
//!
//! - **One source of truth.** The size of an opening that names a family lives in the family. Editing the family resizes every opening
//!   that uses it in one undo step, exactly as a Revit family type does; an opening's `family` property is the only link.
//! - **Nothing is invented.** Every size is entered by the user. A window family needs its sill height; a door family has none.
//! - **Never silent.** An opening edited by hand away from its family is reported by `mismatches`, not reset behind the user's back.
//!   A family cannot be removed or changed to a kind that its openings do not match while openings still name it.
//! - **The model stays valid.** The commands go through the document, whose semantic checks refuse a door or window that no longer
//!   fits its host wall; the refusal names the opening and nothing changes.

use crate::command::Command;
use crate::model::{Element, Id, Model};
use crate::typed::{Door, Typed, Window};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpeningKind {
    #[default]
    Door,
    Window,
}

impl OpeningKind {
    pub fn element_kind(self) -> &'static str {
        match self {
            OpeningKind::Door => Door::KIND,
            OpeningKind::Window => Window::KIND,
        }
    }
    /// `(Arabic, English)`.
    pub fn label(self) -> (&'static str, &'static str) {
        match self {
            OpeningKind::Door => ("باب", "Door"),
            OpeningKind::Window => ("نافذة", "Window"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OpeningFamily {
    pub id: String,
    pub name: String,
    pub kind: OpeningKind,
    pub width_mm: i64,
    pub height_mm: i64,
    /// Windows only: the height of the sill above the floor.
    pub sill_mm: Option<i64>,
    /// Where the sizes come from (the manufacturer's sheet, the code page...).
    pub source: Option<String>,
}

/// Everything wrong with the families themselves.
pub fn check_all(list: &[OpeningFamily]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for f in list {
        let id = f.id.trim();
        if id.is_empty() || id.chars().count() > 40 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(format!("a family id is 1 to 40 letters, digits, '-' or '_' (got '{}')", f.id));
        }
        if !seen.insert(id.to_string()) {
            return Err(format!("two opening families are named '{id}'"));
        }
        if f.name.trim().is_empty() {
            return Err(format!("opening family '{id}' needs a name"));
        }
        if !(100..=20_000).contains(&f.width_mm) || !(100..=20_000).contains(&f.height_mm) {
            return Err(format!("opening family '{id}': the width and the height are whole millimetres between 100 and 20000"));
        }
        match (f.kind, f.sill_mm) {
            (OpeningKind::Window, None) => return Err(format!("window family '{id}' needs its sill height")),
            (OpeningKind::Window, Some(s)) if !(0..=20_000).contains(&s) => return Err(format!("window family '{id}': the sill height must be between 0 and 20000 mm")),
            (OpeningKind::Door, Some(_)) => return Err(format!("door family '{id}' has a sill height: only windows have one")),
            _ => {}
        }
    }
    Ok(())
}

/// The project's families, read once from the model.
pub fn of(model: &Model) -> Vec<OpeningFamily> {
    match crate::project::get(model) {
        Ok(Some(p)) => p.meta.opening_families,
        _ => Vec::new(),
    }
}

fn short(id: Id) -> String {
    id.to_string().chars().take(8).collect()
}

fn is_opening(e: &Element) -> bool {
    e.kind == Door::KIND || e.kind == Window::KIND
}

/// The commands that give an opening the family's sizes (only the properties that differ).
fn size_commands(e: &Element, f: &OpeningFamily) -> Vec<Command> {
    let mut cmds = Vec::new();
    let mut set = |key: &str, v: i64| {
        if e.props.get(key).and_then(Value::as_i64) != Some(v) {
            cmds.push(Command::SetProp { id: e.id, key: key.into(), value: Some(json!(v)) });
        }
    };
    set("width", f.width_mm);
    set("height", f.height_mm);
    if let Some(s) = f.sill_mm {
        set("sillHeight", s);
    }
    cmds
}

/// Gives openings a family (or clears it with `None`) and sets their sizes to the family's. A door cannot take a window family.
pub fn assign_commands(model: &Model, openings: &[Id], family: Option<&str>) -> Result<Vec<Command>, String> {
    let list = of(model);
    let fam = match family {
        Some(id) => Some(list.iter().find(|f| f.id == id).ok_or_else(|| format!("there is no opening family '{id}'"))?),
        None => None,
    };
    let mut cmds = Vec::new();
    for id in openings {
        let e = model.get(id).ok_or("an opening in the selection no longer exists")?;
        if !is_opening(e) {
            return Err("families apply to doors and windows only".into());
        }
        if let Some(f) = fam {
            if e.kind != f.kind.element_kind() {
                return Err(format!("'{}' is a {} family: opening {} is a {}", f.name, f.kind.label().1.to_lowercase(), short(*id), e.kind.to_lowercase()));
            }
            cmds.extend(size_commands(e, f));
        }
        cmds.push(Command::SetProp { id: *id, key: "family".into(), value: fam.map(|f| json!(f.id)) });
    }
    Ok(cmds)
}

/// The commands that resize every opening that names a family, after the families changed. A family that is gone, or whose kind no longer
/// matches its openings, is refused by name.
pub fn retarget_commands(model: &Model, after: &[OpeningFamily]) -> Result<Vec<Command>, String> {
    let mut cmds = Vec::new();
    for e in model.elements.values().filter(|e| is_opening(e)) {
        let Some(id) = e.props.get("family").and_then(Value::as_str) else { continue };
        let Some(f) = after.iter().find(|f| f.id == id) else {
            return Err(format!("opening {} uses the family '{id}': clear it from the opening first", short(e.id)));
        };
        if e.kind != f.kind.element_kind() {
            return Err(format!("opening {} is a {} but the family '{id}' is now a {}", short(e.id), e.kind.to_lowercase(), f.kind.label().1.to_lowercase()));
        }
        cmds.extend(size_commands(e, f));
    }
    Ok(cmds)
}

/// Openings whose sizes differ from their family's: `(id, what differs)`. Reported, never changed silently.
pub fn mismatches(model: &Model) -> Vec<(Id, String, String)> {
    let list = of(model);
    let mut out = Vec::new();
    for e in model.elements.values().filter(|e| is_opening(e)) {
        let Some(fid) = e.props.get("family").and_then(Value::as_str) else { continue };
        let Some(f) = list.iter().find(|f| f.id == fid) else { continue };
        let diff: Vec<String> = size_commands(e, f)
            .iter()
            .filter_map(|c| match c {
                Command::SetProp { key, value: Some(v), .. } => Some(format!("{key} is {} and the family says {v}", e.props.get(key).map_or("unset".into(), |x| x.to_string()))),
                _ => None,
            })
            .collect();
        if !diff.is_empty() {
            out.push((e.id, f.name.clone(), diff.join("; ")));
        }
    }
    out
}

/// The panel's data: the families with the openings that use each, the openings that disagree with their family and the openings with none.
pub fn report(model: &Model) -> Value {
    let list = of(model);
    let openings: Vec<&Element> = model.elements.values().filter(|e| is_opening(e)).collect();
    let rows: Vec<Value> = list
        .iter()
        .map(|f| {
            let used: Vec<String> = openings.iter().filter(|e| e.props.get("family").and_then(Value::as_str) == Some(f.id.as_str())).map(|e| e.id.to_string()).collect();
            json!({ "family": f, "kind_label": { "ar": f.kind.label().0, "en": f.kind.label().1 }, "count": used.len(), "openings": used })
        })
        .collect();
    let none = |kind: &str| openings.iter().filter(|e| e.kind == kind && e.props.get("family").is_none()).count();
    json!({
        "families": rows,
        "without_family": { "doors": none(Door::KIND), "windows": none(Window::KIND) },
        "mismatches": mismatches(model).into_iter().map(|(id, name, what)| json!({ "opening": id.to_string(), "family": name, "differs": what })).collect::<Vec<_>>(),
        "kinds": [json!({ "id": "door", "ar": "باب", "en": "Door" }), json!({ "id": "window", "ar": "نافذة", "en": "Window" })],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn door(id: &str, w: i64, h: i64) -> OpeningFamily {
        OpeningFamily { id: id.into(), name: format!("name {id}"), kind: OpeningKind::Door, width_mm: w, height_mm: h, sill_mm: None, source: None }
    }
    fn window(id: &str, w: i64, h: i64, s: Option<i64>) -> OpeningFamily {
        OpeningFamily { id: id.into(), name: format!("name {id}"), kind: OpeningKind::Window, width_mm: w, height_mm: h, sill_mm: s, source: None }
    }

    #[test]
    fn the_checks_refuse_bad_ids_duplicates_sizes_and_sills() {
        assert!(check_all(&[door("D1", 900, 2100), window("W1", 1200, 1200, Some(900))]).is_ok());
        assert!(check_all(&[door("", 900, 2100)]).unwrap_err().contains("family id"));
        assert!(check_all(&[door("a b", 900, 2100)]).unwrap_err().contains("family id"));
        assert!(check_all(&[door("D", 900, 2100), door("D", 800, 2100)]).unwrap_err().contains("two opening families"));
        assert!(check_all(&[door("D", 50, 2100)]).unwrap_err().contains("between 100 and 20000"));
        assert!(check_all(&[window("W", 1200, 1200, None)]).unwrap_err().contains("needs its sill"));
        assert!(check_all(&[window("W", 1200, 1200, Some(-1))]).unwrap_err().contains("sill height must be"));
        let mut d = door("D", 900, 2100);
        d.sill_mm = Some(100);
        assert!(check_all(&[d]).unwrap_err().contains("only windows"));
        let mut n = door("D", 900, 2100);
        n.name = "  ".into();
        assert!(check_all(&[n]).unwrap_err().contains("needs a name"));
    }
}
