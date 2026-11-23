//! Municipality regulation table: the user's own record of what each municipality (and zone or land use inside it) allows.
//!
//! Nothing here is a built-in value. Setbacks, coverage, height, floor count and floor-area ratio are set by each municipality's
//! regulations, so every number is entered by the user and carries its own source (regulation, article, circular). A value without
//! a source is refused; a value left empty is unset and the check that needs it stays skipped, never passed.
//!
//! An entry is data only. Applying one copies the limits the project already checks (setbacks and coverage) into the project's
//! `limits` as one undoable step that shows old and new; the height, floor and FAR limits are stored and reported as not yet applied
//! (their checks belong to the setback/coverage/height check, not to this table).

use crate::project::NanoArchProject;
use crate::rules::Param;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The most entries a project keeps.
pub const MAX_ENTRIES: usize = 200;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MunicipalityEntry {
    pub id: String,
    /// The municipality (amanah, baladiyah) that issued the values.
    pub municipality: String,
    /// Zone, land use or plot class inside the municipality (for example a residential zone code). Free text from the document.
    pub zone: String,
    /// The regulation, circular or table the values come from, with its edition.
    pub document: String,
    /// Date of the document (`YYYY-MM-DD`), when known.
    pub date: Option<String>,
    pub min_setback_front_m: Option<Param>,
    pub min_setback_back_m: Option<Param>,
    pub min_setback_side_m: Option<Param>,
    /// Maximum built footprint divided by plot area (0..1).
    pub max_coverage_ratio: Option<Param>,
    /// Maximum building height in metres.
    pub max_height_m: Option<Param>,
    /// Maximum number of floors (a whole number).
    pub max_floors: Option<Param>,
    /// Maximum total floor area divided by plot area.
    pub max_far: Option<Param>,
}

impl MunicipalityEntry {
    /// `(name, value, largest allowed value, must be a whole number)` for every limit slot.
    fn slots(&self) -> [(&'static str, &Option<Param>, f64, bool); 7] {
        [
            ("front setback", &self.min_setback_front_m, f64::INFINITY, false),
            ("rear setback", &self.min_setback_back_m, f64::INFINITY, false),
            ("side setback", &self.min_setback_side_m, f64::INFINITY, false),
            ("coverage ratio", &self.max_coverage_ratio, 1.0, false),
            ("height", &self.max_height_m, f64::INFINITY, false),
            ("floors", &self.max_floors, f64::INFINITY, true),
            ("floor-area ratio", &self.max_far, f64::INFINITY, false),
        ]
    }

    /// How many limits carry a value.
    pub fn set_count(&self) -> usize {
        self.slots().iter().filter(|(_, p, _, _)| p.as_ref().is_some_and(|p| p.value.is_some())).count()
    }

    /// The names of the limits left unset: their checks stay skipped.
    pub fn unset(&self) -> Vec<&'static str> {
        self.slots().iter().filter(|(_, p, _, _)| !p.as_ref().is_some_and(|p| p.value.is_some())).map(|(n, _, _, _)| *n).collect()
    }
}

/// Everything wrong with one entry. Empty when it is acceptable.
pub fn problems(e: &MunicipalityEntry) -> Vec<String> {
    let mut v = Vec::new();
    if e.id.trim().is_empty() {
        v.push("an entry needs an id".into());
    }
    if e.municipality.trim().is_empty() {
        v.push("the entry needs its municipality".into());
    }
    if e.document.trim().is_empty() {
        v.push("the entry needs the document its values come from (regulation, circular, edition)".into());
    }
    if let Some(d) = &e.date
        && let Err(err) = crate::execution::parse_date("municipality entry date", d)
    {
        v.push(err);
    }
    for (name, p, max, whole) in e.slots() {
        let Some(p) = p else { continue };
        let Some(x) = p.value else { continue };
        if !x.is_finite() || x < 0.0 || x > max {
            v.push(format!("{name}: {x} is out of range"));
        } else if whole && x.fract() != 0.0 {
            v.push(format!("{name}: {x} must be a whole number"));
        } else if matches!(name, "height" | "floors" | "floor-area ratio") && x == 0.0 {
            v.push(format!("{name}: zero would forbid any building; leave it unset if the document gives no value"));
        }
        if p.source.trim().is_empty() {
            v.push(format!("{name}: has a value but no source (regulation, article, municipality)"));
        }
    }
    if e.set_count() == 0 {
        v.push("the entry sets no limit: enter at least one value with its source".into());
    }
    v
}

/// What the project's table must satisfy to be stored.
pub fn check_all(list: &[MunicipalityEntry]) -> Result<(), String> {
    if list.len() > MAX_ENTRIES {
        return Err(format!("at most {MAX_ENTRIES} municipality entries are kept in a project"));
    }
    let mut ids = BTreeSet::new();
    let mut keys = BTreeSet::new();
    for e in list {
        if !ids.insert(e.id.as_str()) {
            return Err(format!("the municipality entry id '{}' is used twice", e.id));
        }
        if let Some(first) = problems(e).into_iter().next() {
            return Err(format!("municipality entry '{}': {first}", e.id));
        }
        let key = (e.municipality.trim().to_lowercase(), e.zone.trim().to_lowercase());
        if !keys.insert(key) {
            return Err(format!("municipality entry '{}': '{}' / '{}' is already in the table; edit or remove the old entry", e.id, e.municipality.trim(), e.zone.trim()));
        }
    }
    Ok(())
}

/// The unused id `mn1`, `mn2` ...
pub fn next_id(list: &[MunicipalityEntry]) -> String {
    (1..).map(|n| format!("mn{n}")).find(|id| !list.iter().any(|e| &e.id == id)).unwrap_or_default()
}

/// One limit that applying an entry changed in the project.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LimitChange {
    pub name: &'static str,
    pub from: Option<Param>,
    pub to: Param,
}

/// What `apply` did.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Applied {
    pub changed: Vec<LimitChange>,
    /// Limits the entry carries that the project does not check yet (stored, not applied).
    pub stored_only: Vec<&'static str>,
}

/// Copies the setback and coverage limits of entry `id` that carry a value into the project's limits. Limits the entry leaves unset
/// do not clear the project's own. Fails when nothing would change.
pub fn apply(p: &mut NanoArchProject, id: &str) -> Result<Applied, String> {
    let e = p.municipalities.iter().find(|e| e.id == id).cloned().ok_or_else(|| format!("no municipality entry '{id}'"))?;
    let mut out = Applied::default();
    let targets: [(&'static str, &Option<Param>, &mut Option<Param>); 4] = [
        ("front setback", &e.min_setback_front_m, &mut p.limits.min_setback_front_m),
        ("rear setback", &e.min_setback_back_m, &mut p.limits.min_setback_back_m),
        ("side setback", &e.min_setback_side_m, &mut p.limits.min_setback_side_m),
        ("coverage ratio", &e.max_coverage_ratio, &mut p.limits.max_coverage_ratio),
    ];
    for (name, new, slot) in targets {
        let Some(new) = new.as_ref().filter(|n| n.value.is_some()) else { continue };
        if slot.as_ref() != Some(new) {
            out.changed.push(LimitChange { name, from: slot.clone(), to: new.clone() });
            *slot = Some(new.clone());
        }
    }
    for (name, v, _, _) in e.slots().into_iter().skip(4) {
        if v.as_ref().is_some_and(|p| p.value.is_some()) {
            out.stored_only.push(name);
        }
    }
    if out.changed.is_empty() {
        return Err("the project already carries these limits (or the entry sets no setback or coverage limit)".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn param(v: f64) -> Option<Param> {
        Some(Param { value: Some(v), source: "synthetic test regulation, art. 1".into() })
    }

    fn entry(id: &str, municipality: &str, zone: &str) -> MunicipalityEntry {
        MunicipalityEntry {
            id: id.into(),
            municipality: municipality.into(),
            zone: zone.into(),
            document: "synthetic circular 1".into(),
            min_setback_front_m: param(3.0),
            max_coverage_ratio: param(0.6),
            max_height_m: param(12.0),
            max_floors: param(3.0),
            ..Default::default()
        }
    }

    #[test]
    fn a_complete_entry_is_accepted_and_reports_what_it_leaves_unset() {
        let e = entry("mn1", "Test City", "R1");
        assert!(problems(&e).is_empty(), "{:?}", problems(&e));
        assert_eq!(e.set_count(), 4);
        assert_eq!(e.unset(), vec!["rear setback", "side setback", "floor-area ratio"]);
    }

    #[test]
    fn a_value_without_a_source_is_refused() {
        let mut e = entry("mn1", "Test City", "R1");
        e.max_height_m = Some(Param { value: Some(12.0), source: "  ".into() });
        assert!(problems(&e).iter().any(|m| m.contains("height") && m.contains("no source")));
    }

    #[test]
    fn missing_identity_and_empty_entries_are_refused() {
        let mut e = entry("mn1", "", "R1");
        e.document.clear();
        let p = problems(&e);
        assert!(p.iter().any(|m| m.contains("municipality")) && p.iter().any(|m| m.contains("document")));
        let empty = MunicipalityEntry { id: "mn2".into(), municipality: "X".into(), document: "d".into(), ..Default::default() };
        assert!(problems(&empty).iter().any(|m| m.contains("sets no limit")));
    }

    #[test]
    fn out_of_range_fractional_floors_and_zero_limits_are_refused() {
        let mut e = entry("mn1", "Test City", "R1");
        e.max_coverage_ratio = param(1.2);
        e.max_floors = param(2.5);
        e.max_height_m = param(0.0);
        e.min_setback_back_m = param(-1.0);
        let p = problems(&e);
        assert!(p.iter().any(|m| m.contains("coverage ratio")));
        assert!(p.iter().any(|m| m.contains("floors") && m.contains("whole")));
        assert!(p.iter().any(|m| m.contains("height") && m.contains("zero")));
        assert!(p.iter().any(|m| m.contains("rear setback")));
        e.date = Some("2027-13-40".into());
        assert!(problems(&e).iter().any(|m| m.to_lowercase().contains("date")));
    }

    #[test]
    fn the_table_refuses_duplicate_ids_and_duplicate_municipality_zone_pairs() {
        let a = entry("mn1", "Test City", "R1");
        assert!(check_all(&[a.clone(), entry("mn2", "Test City", "R2")]).is_ok());
        assert!(check_all(&[a.clone(), entry("mn1", "Other", "R1")]).unwrap_err().contains("used twice"));
        assert!(check_all(&[a.clone(), entry("mn2", " test city ", "r1")]).unwrap_err().contains("already in the table"));
        assert_eq!(next_id(&[a]), "mn2");
    }

    #[test]
    fn apply_copies_setbacks_and_coverage_only_and_names_what_is_stored_only() {
        let mut p = NanoArchProject::default();
        p.municipalities.push(entry("mn1", "Test City", "R1"));
        let out = apply(&mut p, "mn1").unwrap();
        assert_eq!(out.changed.len(), 2);
        assert_eq!(p.limits.min_setback_front_m, param(3.0));
        assert_eq!(p.limits.max_coverage_ratio, param(0.6));
        assert!(p.limits.min_setback_back_m.is_none(), "an unset limit in the entry is not invented in the project");
        assert_eq!(out.stored_only, vec!["height", "floors"]);
        assert!(apply(&mut p, "mn1").unwrap_err().contains("already carries"));
        assert!(apply(&mut p, "nope").unwrap_err().contains("no municipality entry"));
    }

    #[test]
    fn apply_does_not_clear_a_project_limit_the_entry_leaves_unset() {
        let mut p = NanoArchProject::default();
        p.limits.min_setback_side_m = Some(Param { value: Some(1.5), source: "own survey".into() });
        p.municipalities.push(entry("mn1", "Test City", "R1"));
        apply(&mut p, "mn1").unwrap();
        assert_eq!(p.limits.min_setback_side_m.as_ref().and_then(|x| x.value), Some(1.5));
    }
}
