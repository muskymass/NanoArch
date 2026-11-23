//! Office layer and line-type standard applied to the DXF export.
//!
//! The engine draws on its own layer names with its own colours and line types (`dxf::layer_table`). An office has its own CAD
//! standard, so the user may enter one: for each engine layer a new layer name, an AutoCAD colour index, a line type, or any of the
//! three. Nothing is built in: there is no "standard" office style, and the style must say where it comes from (`source`, the
//! office document it was copied from). A layer the style does not mention keeps the engine's own name, colour and line type.
//!
//! What is rewritten is the finished file: the LAYER table (name, colour, line type) and the layer of every entity. A construction
//! phase layer `<layer>_<PHASE>` follows its base layer: it takes the base layer's new name as its prefix and its colour and line
//! type, as the engine already does for the unstyled file.
//!
//! Rejected by name, all at once: a missing source, an empty style, a layer the engine does not draw, a layer listed twice, a rule
//! that changes nothing, a colour outside 1..=255, a line type the file does not define, a layer name R12 cannot carry, and two
//! layers that would end up with the same name. Lineweights are not set: the R12 file has no lineweight field, so they stay with
//! the plot style table of the office.

use crate::dxf::{self, DxfOut};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

/// Longest layer name R12 allows.
const MAX_NAME: usize = 31;
/// Characters a layer name must not contain.
const FORBIDDEN: &str = "<>/\\\":;?*|=`,'";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerRule {
    /// The engine layer this rule restyles (`A-WALL-EXT`, `S-COLS`, ...).
    pub layer: String,
    /// New layer name.
    #[serde(default)]
    pub name: Option<String>,
    /// AutoCAD colour index, 1..=255.
    #[serde(default)]
    pub color: Option<i32>,
    /// `CONTINUOUS`, `DASHED` or `HIDDEN`.
    #[serde(default)]
    pub linetype: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerStyle {
    /// Name of the office standard, for the record.
    pub name: String,
    /// The office document the rules were taken from.
    pub source: String,
    pub layers: Vec<LayerRule>,
}

/// What one rule did to the file.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct AppliedLayer {
    pub layer: String,
    pub name: String,
    pub color: Option<i32>,
    pub linetype: Option<String>,
    /// Whether any entity was drawn on the layer (or one of its phase layers) in this file.
    pub used: bool,
}

#[derive(Debug, Clone)]
pub struct Styled {
    pub out: DxfOut,
    pub applied: Vec<AppliedLayer>,
}

/// A rule resolved against the engine's layer table.
struct Resolved<'a> {
    rule: &'a LayerRule,
    linetype: Option<String>,
}

fn valid_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("is empty".into());
    }
    if name != name.trim() {
        return Err("has leading or trailing spaces".into());
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!("is longer than {MAX_NAME} characters"));
    }
    if let Some(c) = name.chars().find(|c| FORBIDDEN.contains(*c) || !(' '..='~').contains(c)) {
        return Err(format!("contains '{c}', which an R12 layer name cannot carry"));
    }
    Ok(())
}

/// Validates the style against the engine's layers and returns the resolved rules by layer.
fn resolve(style: &LayerStyle) -> Result<BTreeMap<String, Resolved<'_>>, String> {
    let mut errors: Vec<String> = Vec::new();
    if style.name.trim().is_empty() {
        errors.push("style name is empty".into());
    }
    if style.source.trim().is_empty() {
        errors.push("source (the office document the style comes from) is missing".into());
    }
    if style.layers.is_empty() {
        errors.push("the style has no layer rules".into());
    }
    let table = dxf::layer_table();
    let known: BTreeMap<&str, (i32, &str)> = table.iter().map(|(n, c, l)| (n.as_str(), (*c, *l))).collect();
    let linetypes = dxf::linetype_names();
    let mut out: BTreeMap<String, Resolved<'_>> = BTreeMap::new();
    for rule in &style.layers {
        if !known.contains_key(rule.layer.as_str()) {
            errors.push(format!("layer '{}' is not drawn by the engine", rule.layer));
            continue;
        }
        if out.contains_key(&rule.layer) {
            errors.push(format!("layer '{}' is listed twice", rule.layer));
            continue;
        }
        if rule.name.is_none() && rule.color.is_none() && rule.linetype.is_none() {
            errors.push(format!("layer '{}': the rule changes nothing (give name, color or linetype)", rule.layer));
            continue;
        }
        if let Some(Err(e)) = rule.name.as_deref().map(valid_name) {
            errors.push(format!("layer '{}': new name '{}' {e}", rule.layer, rule.name.as_deref().unwrap_or("")));
        }
        if let Some(c) = rule.color.filter(|c| !(1..=255).contains(c)) {
            errors.push(format!("layer '{}': colour {c} is outside 1..=255", rule.layer));
        }
        let linetype = rule.linetype.as_ref().map(|l| l.trim().to_uppercase());
        if linetype.as_deref().is_some_and(|l| !linetypes.contains(&l)) {
            errors.push(format!("layer '{}': line type '{}' is not defined in the file (use {})", rule.layer, rule.linetype.as_deref().unwrap_or(""), linetypes.join(", ")));
        }
        out.insert(rule.layer.clone(), Resolved { rule, linetype });
    }
    // Final names must be unique across every layer the engine draws, renamed or not.
    let mut finals: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for (n, _, _) in &table {
        let name = out.get(n.as_str()).and_then(|r| r.rule.name.clone()).unwrap_or_else(|| n.clone());
        finals.entry(name.to_uppercase()).or_default().push(n.as_str());
    }
    for (name, from) in finals.iter().filter(|(_, f)| f.len() > 1) {
        errors.push(format!("layers {} would all be named '{name}'", from.join(", ")));
    }
    if errors.is_empty() { Ok(out) } else { Err(format!("layer style rejected: {}", errors.join("; "))) }
}

/// The rule that governs `layer`: its own, or that of the longest base layer it is a phase layer of (`<base>_<PHASE>`).
fn governing<'a, 'b>(layer: &'b str, rules: &'b BTreeMap<String, Resolved<'a>>) -> Option<(&'b str, &'b Resolved<'a>, &'b str)> {
    // The suffix (`_NEW`, ...) is whatever follows the base layer's name.
    let table = dxf::layer_table();
    let base = table.iter().map(|(n, _, _)| n.as_str()).filter(|n| layer == *n || layer.strip_prefix(*n).is_some_and(|r| r.starts_with('_'))).max_by_key(|n| n.len())?;
    let (key, r) = rules.get_key_value(base)?;
    Some((key.as_str(), r, &layer[base.len()..]))
}

/// Applies `style` to a finished DXF export.
pub fn apply(out: &DxfOut, style: &LayerStyle) -> Result<Styled, String> {
    let rules = resolve(style)?;
    let lines: Vec<&str> = out.text.lines().collect();
    if !lines.len().is_multiple_of(2) {
        return Err("the DXF text is not made of group code / value pairs".into());
    }
    let renamed = |layer: &str| -> String {
        match governing(layer, &rules) {
            Some((_, r, suffix)) => match &r.rule.name {
                Some(n) => format!("{n}{suffix}"),
                None => layer.to_string(),
            },
            None => layer.to_string(),
        }
    };
    let mut owned: Vec<String> = Vec::with_capacity(lines.len());
    let mut last_zero = String::new();
    let mut in_layer_table = false;
    let mut entry: Option<&Resolved<'_>> = None;
    let mut i = 0;
    while i < lines.len() {
        let code = lines[i].trim();
        let value = lines[i + 1];
        let mut new_value = value.to_string();
        match code {
            "0" => {
                if value == "ENDTAB" {
                    in_layer_table = false;
                    entry = None;
                }
                last_zero = value.to_string();
            }
            "2" if last_zero == "TABLE" && value == "LAYER" => in_layer_table = true,
            "2" if in_layer_table && last_zero == "LAYER" => {
                entry = governing(value, &rules).map(|(_, r, _)| r);
                new_value = renamed(value);
            }
            "62" if in_layer_table && last_zero == "LAYER" => {
                if let Some(c) = entry.and_then(|r| r.rule.color) {
                    new_value = c.to_string();
                }
            }
            "6" if in_layer_table && last_zero == "LAYER" => {
                if let Some(l) = entry.and_then(|r| r.linetype.clone()) {
                    new_value = l;
                }
            }
            "8" if !in_layer_table => new_value = renamed(value),
            _ => {}
        }
        owned.push(lines[i].to_string());
        owned.push(new_value);
        i += 2;
    }
    let mut text = owned.join("\n");
    text.push('\n');
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (layer, n) in &out.counts {
        *counts.entry(renamed(layer)).or_default() += n;
    }
    let used_bases: BTreeSet<&str> = out.counts.keys().filter_map(|l| governing(l, &rules).map(|(k, _, _)| k)).collect();
    let applied = rules
        .iter()
        .map(|(layer, r)| AppliedLayer { layer: layer.clone(), name: r.rule.name.clone().unwrap_or_else(|| layer.clone()), color: r.rule.color, linetype: r.linetype.clone(), used: used_bases.contains(layer.as_str()) })
        .collect();
    Ok(Styled { out: DxfOut { text, counts }, applied })
}
