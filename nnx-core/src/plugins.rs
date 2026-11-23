//! Office plugins: a named, versioned bundle of an office's own rule pack and starter templates that the user loads into a project.
//!
//! A plugin adds data only, never code, and never touches the built-in checks (`sbc`) or the built-in templates:
//! - its **rules** are an ordinary rule pack (`rules::PackDef`) validated exactly like an overlay (every value with its source, every
//!   rule with its clause). While the plugin is enabled the pack is a layer on top of the enabled overlays (`effective_overlays`);
//! - its **templates** are starting points (plot, floors, room program) written by the office. Nothing is computed for them: setbacks
//!   are not derived (no Ministry rule is assumed for an office template), so the project's own limits still govern; a room area
//!   below the SBC minimum for its type is reported as a warning, a type with no SBC value is reported as not checked;
//! - the plugin needs a publisher and the source of its content (office standard, document, edition), so a value never appears
//!   without saying whose it is.
//!
//! Plugins are stored in the project (`plugins`): undo, redo and the `.nnx` file follow. Installing is never silent: `review` says
//! what the plugin adds and which rules it would replace; replacing needs the user's confirmation.

use crate::execution::parse_date;
use crate::overlay::{self, Overlay};
use crate::rules::{LayerReport, PackDef};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The most plugins a project keeps.
pub const MAX_PLUGINS: usize = 20;
/// The most templates one plugin carries.
pub const MAX_TEMPLATES: usize = 50;
/// The most rooms one template carries.
pub const MAX_ROOMS: usize = 200;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PluginRoom {
    pub key: String,
    pub name: String,
    /// Room type (`majlis`, `bedroom`, ...): lower-case letters, digits and `_`.
    pub func: String,
    pub area_m2: f64,
    pub floor: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PluginTemplate {
    pub id: String,
    pub title_ar: String,
    pub title_en: String,
    pub building_type: String,
    pub style: String,
    pub city: String,
    pub plot_width_m: f64,
    pub plot_length_m: f64,
    pub floors: u32,
    pub street_width_m: Option<f64>,
    pub rooms: Vec<PluginRoom>,
    /// What the office states about this template (assumptions, scope).
    pub assumptions: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Plugin {
    pub id: String,
    pub name: String,
    pub version: String,
    /// The office or body that publishes the plugin.
    pub publisher: String,
    /// The standard, document or edition the plugin's content comes from.
    pub source: String,
    pub description: String,
    /// `YYYY-MM-DD`, when known.
    pub date: Option<String>,
    pub enabled: bool,
    /// The office rule pack as JSON text (`rules::PackDef`), when the plugin carries rules.
    pub rules: Option<String>,
    pub templates: Vec<PluginTemplate>,
}

fn identifier(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn positive(x: f64) -> bool {
    x.is_finite() && x > 0.0
}

impl Plugin {
    /// The plugin's rule pack as an overlay, so it is validated and layered by the overlay code.
    pub fn as_overlay(&self) -> Option<Overlay> {
        self.rules.as_ref().map(|pack| Overlay {
            id: format!("plugin:{}", self.id),
            name: self.name.clone(),
            authority: self.publisher.clone(),
            document: self.source.clone(),
            date: self.date.clone(),
            enabled: self.enabled,
            pack: pack.clone(),
        })
    }

    /// Everything wrong with the plugin on its own. Empty when it is acceptable.
    pub fn problems(&self) -> Vec<String> {
        let mut v = Vec::new();
        if !identifier(&self.id) {
            v.push("a plugin id is lower-case letters, digits and '_' (for example 'office_std')".into());
        }
        for (what, s) in [("name", &self.name), ("version", &self.version), ("publisher (the office that issued it)", &self.publisher), ("source (the standard or document its content comes from)", &self.source)] {
            if s.trim().is_empty() {
                v.push(format!("the plugin needs its {what}"));
            }
        }
        if let Some(d) = &self.date
            && let Err(e) = parse_date("plugin date", d)
        {
            v.push(e);
        }
        if self.rules.is_none() && self.templates.is_empty() {
            v.push("the plugin carries neither rules nor templates".into());
        }
        if let Some(o) = self.as_overlay() {
            v.extend(overlay::problems(&o));
        }
        if self.templates.len() > MAX_TEMPLATES {
            v.push(format!("a plugin carries at most {MAX_TEMPLATES} templates"));
        }
        let mut ids = BTreeSet::new();
        for t in &self.templates {
            if !ids.insert(t.id.as_str()) {
                v.push(format!("the template id '{}' is used twice", t.id));
            }
            v.extend(t.problems().into_iter().map(|p| format!("template '{}': {p}", t.id)));
        }
        v
    }
}

impl PluginTemplate {
    /// Everything wrong with the template on its own.
    pub fn problems(&self) -> Vec<String> {
        let mut v = Vec::new();
        if !identifier(&self.id) {
            v.push("the id is lower-case letters, digits and '_'".into());
        }
        for (what, s) in [("Arabic title", &self.title_ar), ("English title", &self.title_en), ("building type", &self.building_type)] {
            if s.trim().is_empty() {
                v.push(format!("needs its {what}"));
            }
        }
        for (what, x) in [("plot width", self.plot_width_m), ("plot length", self.plot_length_m)] {
            if !positive(x) {
                v.push(format!("the {what} must be a positive number of metres"));
            }
        }
        if let Some(w) = self.street_width_m
            && !positive(w)
        {
            v.push("the street width must be a positive number of metres".into());
        }
        if self.floors == 0 {
            v.push("needs at least one floor".into());
        }
        if self.rooms.is_empty() {
            v.push("needs a room program".into());
        }
        if self.rooms.len() > MAX_ROOMS {
            v.push(format!("at most {MAX_ROOMS} rooms"));
        }
        let mut keys = BTreeSet::new();
        for r in &self.rooms {
            if r.key.trim().is_empty() || !keys.insert(r.key.as_str()) {
                v.push(format!("the room key '{}' is empty or used twice", r.key));
            }
            if !identifier(&r.func) {
                v.push(format!("room '{}': the type '{}' is not lower-case letters, digits and '_'", r.key, r.func));
            }
            if !positive(r.area_m2) {
                v.push(format!("room '{}': the area must be a positive number of m²", r.key));
            }
            if r.floor == 0 || r.floor > self.floors {
                v.push(format!("room '{}': floor {} is outside 1..{}", r.key, r.floor, self.floors));
            }
        }
        if positive(self.plot_width_m) && positive(self.plot_length_m) && self.floors > 0 {
            let plot = self.plot_width_m * self.plot_length_m;
            for f in 1..=self.floors {
                let sum: f64 = self.rooms.iter().filter(|r| r.floor == f && positive(r.area_m2)).map(|r| r.area_m2).sum();
                if sum > plot {
                    v.push(format!("floor {f}: the rooms total {sum:.1} m², more than the whole plot ({plot:.1} m²)"));
                }
            }
        }
        v
    }

    /// Room areas below the SBC minimum for their type, and the types with no SBC value (not checked, never passed).
    pub fn notes(&self) -> Vec<String> {
        let mut v = Vec::new();
        for r in &self.rooms {
            match crate::sbc::code_value(&format!("room_min_area_m2.{}", r.func)) {
                Some(c) if r.area_m2 < c.value => {
                    v.push(format!("room '{}' ({} m²) is below the SBC minimum for '{}' ({} m², {} {}, clause {})", r.key, r.area_m2, r.func, c.value, c.code, c.edition, c.clause));
                }
                Some(_) => {}
                None => v.push(format!("room '{}': no SBC minimum area is recorded for '{}', so the area is not checked", r.key, r.func)),
            }
        }
        v
    }
}

/// What the project's list of plugins must satisfy to be stored.
pub fn check_all(list: &[Plugin]) -> Result<(), String> {
    if list.len() > MAX_PLUGINS {
        return Err(format!("at most {MAX_PLUGINS} plugins are kept in a project"));
    }
    let mut seen = BTreeSet::new();
    for p in list {
        if !seen.insert(p.id.as_str()) {
            return Err(format!("the plugin id '{}' is used twice", p.id));
        }
        if let Some(first) = p.problems().into_iter().next() {
            return Err(format!("plugin '{}': {first}", p.id));
        }
    }
    Ok(())
}

/// The overlays in force: the project's own, then the rule packs of the enabled plugins in the order listed.
pub fn effective_overlays(overlays: &[Overlay], plugins: &[Plugin]) -> Vec<Overlay> {
    let mut v = overlays.to_vec();
    v.extend(plugins.iter().filter(|p| p.enabled).filter_map(Plugin::as_overlay));
    v
}

/// A template found in an enabled plugin, addressed as `plugin_id/template_id`.
pub fn find_template<'a>(plugins: &'a [Plugin], address: &str) -> Result<&'a PluginTemplate, String> {
    let (pid, tid) = address.split_once('/').ok_or_else(|| format!("'{address}' is not a plugin template address (plugin_id/template_id)"))?;
    let p = plugins.iter().find(|p| p.id == pid).ok_or_else(|| format!("no plugin '{pid}'"))?;
    if !p.enabled {
        return Err(format!("the plugin '{pid}' is disabled"));
    }
    p.templates.iter().find(|t| t.id == tid).ok_or_else(|| format!("plugin '{pid}' has no template '{tid}'"))
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Review {
    pub label: String,
    pub errors: Vec<String>,
    pub rules_added: Vec<String>,
    /// `(rule id, layer whose rule is replaced)`.
    pub rules_replaced: Vec<(String, String)>,
    pub parameters_overridden: usize,
    pub templates: Vec<String>,
    pub notes: Vec<String>,
    /// The plugin replaces or overrides rules already in force, or replaces a plugin with the same id: the user must confirm.
    pub needs_confirm: bool,
}

/// What installing `plugin` would do on top of the project's current overlays and plugins, without storing anything.
pub fn review(plugin: &Plugin, overlays: &[Overlay], plugins: &[Plugin]) -> Review {
    let mut r = Review { label: format!("{}@{}", plugin.id, plugin.version), errors: plugin.problems(), ..Default::default() };
    if !r.errors.is_empty() {
        return r;
    }
    let others: Vec<Plugin> = plugins.iter().filter(|p| p.id != plugin.id).cloned().collect();
    if others.len() != plugins.len() {
        r.needs_confirm = true;
        r.notes.push(format!("a plugin '{}' is already installed: it will be replaced", plugin.id));
    }
    let (mut base, _) = match overlay::rule_set(None, &effective_overlays(overlays, &others)) {
        Ok(x) => x,
        Err(e) => {
            r.errors.push(e);
            return r;
        }
    };
    if let Some(o) = plugin.as_overlay() {
        match base.add_layer_reporting(&o.pack) {
            Err(e) => r.errors.push(e.to_string()),
            Ok(LayerReport { added, replaced, overridden, .. }) => {
                r.rules_added = added;
                r.parameters_overridden = overridden.len();
                if !replaced.is_empty() || !overridden.is_empty() {
                    r.needs_confirm = true;
                }
                r.rules_replaced = replaced;
                if let Ok(def) = serde_json::from_str::<PackDef>(&o.pack) {
                    let unset = def.rules.iter().flat_map(|x| x.params.iter().filter(|(_, p)| p.value.is_none()).map(|(n, _)| format!("{}.{n}", x.id))).count();
                    if unset > 0 {
                        r.notes.push(format!("{unset} parameter(s) are unset (null): their rules are skipped, never defaulted"));
                    }
                }
            }
        }
    }
    for t in &plugin.templates {
        r.templates.push(t.id.clone());
        r.notes.extend(t.notes().into_iter().map(|n| format!("template '{}': {n}", t.id)));
    }
    r
}

/// The unused id `plugin1`, `plugin2` ... for a plugin whose author gave none.
pub fn next_id(list: &[Plugin]) -> String {
    (1..).map(|n| format!("plugin{n}")).find(|id| !list.iter().any(|p| &p.id == id)).unwrap_or_default()
}
