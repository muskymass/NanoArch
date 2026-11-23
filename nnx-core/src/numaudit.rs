//! Number audit file: every number the project holds, with the source and date it was entered with.
//!
//! The audit reads the serialised project and invents nothing. For each numeric value it looks for a text field that says where
//! the value came from, in this order: a sibling `<key>_source`, then a `source` in the same object, then a `source` in the
//! nearest enclosing object. The date is found the same way (`<key>_date`, `date`, `source_date`). A value with no non-blank
//! source is listed as `unsourced`; it is never given a source on the user's behalf. A number with a source but no date is
//! `sourced` with an empty date, because only the user can say when a source was opened.
//!
//! Skipped as not being inputs: undo snapshots, the baseline and revision history (copies of earlier states), and the free-form
//! `extra` bag of the file. Booleans and numbers inside source text are not numbers of the project.
//!
//! The file is a record of provenance, not a validation of the values. It does not replace a licensed engineer.

use crate::project::NanoArchProject;
use serde::Serialize;
use serde_json::Value;

/// Top-level and nested keys that hold copies or history rather than inputs.
const SKIPPED_KEYS: [&str; 4] = ["snapshots", "baseline", "revisionHistory", "revision_history"];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuditEntry {
    /// Dotted path from the project root (`limits.minSetbackFrontM.value`, `municipalities[0].front.value`).
    pub path: String,
    pub value: f64,
    /// The source text found for the value; `None` when there is none or it is blank.
    pub source: Option<String>,
    /// The date found with the source; `None` when none was entered.
    pub date: Option<String>,
    /// `sourced` or `unsourced`.
    pub status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuditFile {
    pub entries: Vec<AuditEntry>,
    pub total: usize,
    pub sourced: usize,
    pub unsourced: usize,
    /// Sourced entries that carry no date.
    pub undated: usize,
    /// Paths of the unsourced numbers, in file order.
    pub unsourced_paths: Vec<String>,
    pub notes_en: Vec<String>,
    pub notes_ar: Vec<String>,
}

/// Build the audit file of a project.
pub fn audit(project: &NanoArchProject) -> Result<AuditFile, String> {
    let v = serde_json::to_value(project).map_err(|e| format!("number audit: the project could not be read: {e}"))?;
    let mut entries = Vec::new();
    let mut ctx: Vec<(Option<String>, Option<String>)> = Vec::new();
    walk(&v, "", "", &mut ctx, &mut entries);
    let sourced = entries.iter().filter(|e| e.status == "sourced").count();
    let unsourced = entries.len() - sourced;
    let undated = entries.iter().filter(|e| e.status == "sourced" && e.date.is_none()).count();
    let unsourced_paths = entries.iter().filter(|e| e.status == "unsourced").map(|e| e.path.clone()).collect();
    Ok(AuditFile {
        total: entries.len(),
        sourced,
        unsourced,
        undated,
        unsourced_paths,
        entries,
        notes_en: vec![
            "A source is taken only from text the user entered next to the number; none is inferred or filled in.".into(),
            "Numbers without a source are unverified inputs, not wrong values. Snapshots, the baseline and revision history are not audited.".into(),
        ],
        notes_ar: vec![
            "المصدر يؤخذ فقط من نص أدخله المستخدم بجانب الرقم، ولا يُستنتج ولا يُملأ.".into(),
            "الرقم بلا مصدر مدخل غير موثّق لا قيمة خاطئة. لقطات التراجع والخط الأساسي وسجل المراجعات لا تُدقَّق.".into(),
        ],
    })
}

fn text_of(o: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    o.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

fn first_text(o: &serde_json::Map<String, Value>, keys: &[String]) -> Option<String> {
    keys.iter().find_map(|k| text_of(o, k))
}

/// `ctx` holds the (source, date) declared by each enclosing object, outermost first.
fn walk(v: &Value, path: &str, key: &str, ctx: &mut Vec<(Option<String>, Option<String>)>, out: &mut Vec<AuditEntry>) {
    match v {
        Value::Object(o) => {
            let own = (first_text(o, &["source".to_string()]), first_text(o, &["date".to_string(), "source_date".to_string(), "sourceDate".to_string()]));
            ctx.push(own);
            for (k, child) in o {
                if SKIPPED_KEYS.contains(&k.as_str()) || k.starts_with("_") {
                    continue;
                }
                let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                match child {
                    Value::Number(n) => {
                        if let Some(x) = n.as_f64().filter(|x| x.is_finite()) {
                            out.push(entry(&p, x, k, o, ctx));
                        }
                    }
                    _ => walk(child, &p, k, ctx, out),
                }
            }
            ctx.pop();
        }
        Value::Array(a) => {
            for (i, child) in a.iter().enumerate() {
                let p = format!("{path}[{i}]");
                match child {
                    Value::Number(n) => {
                        if let Some(x) = n.as_f64().filter(|x| x.is_finite()) {
                            // A bare number in an array takes the declaration of the enclosing object.
                            out.push(entry_from_ctx(&p, x, key, ctx));
                        }
                    }
                    _ => walk(child, &p, key, ctx, out),
                }
            }
        }
        _ => {}
    }
}

fn entry(path: &str, value: f64, key: &str, o: &serde_json::Map<String, Value>, ctx: &[(Option<String>, Option<String>)]) -> AuditEntry {
    let src_keys = [format!("{key}_source"), format!("{key}Source")];
    let date_keys = [format!("{key}_date"), format!("{key}Date")];
    let source = first_text(o, &src_keys).or_else(|| nearest(ctx, |c| c.0.clone()));
    let date = first_text(o, &date_keys).or_else(|| nearest(ctx, |c| c.1.clone()));
    finish(path, value, source, date)
}

fn entry_from_ctx(path: &str, value: f64, _key: &str, ctx: &[(Option<String>, Option<String>)]) -> AuditEntry {
    finish(path, value, nearest(ctx, |c| c.0.clone()), nearest(ctx, |c| c.1.clone()))
}

fn nearest(ctx: &[(Option<String>, Option<String>)], pick: impl Fn(&(Option<String>, Option<String>)) -> Option<String>) -> Option<String> {
    ctx.iter().rev().find_map(pick)
}

fn finish(path: &str, value: f64, source: Option<String>, date: Option<String>) -> AuditEntry {
    // A date means nothing without the source it dates.
    let date = if source.is_some() { date } else { None };
    let status = if source.is_some() { "sourced" } else { "unsourced" };
    AuditEntry { path: path.to_string(), value, source, date, status }
}

fn csv_field(s: &str) -> String {
    // Neutralise spreadsheet formula injection from user text, then quote.
    let guarded = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{s}") } else { s.to_string() };
    format!("\"{}\"", guarded.replace('"', "\"\""))
}

/// The audit as CSV with a header row: `path,value,status,source,date`.
pub fn to_csv(file: &AuditFile) -> String {
    let mut s = String::from("path,value,status,source,date\n");
    for e in &file.entries {
        s.push_str(&format!(
            "{},{},{},{},{}\n",
            csv_field(&e.path),
            e.value,
            e.status,
            csv_field(e.source.as_deref().unwrap_or("")),
            csv_field(e.date.as_deref().unwrap_or(""))
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Param;

    fn base() -> NanoArchProject {
        let mut p = NanoArchProject::default();
        p.limits.min_setback_front_m = Some(Param { value: Some(3.0), source: "Municipal circular 12".into() });
        p.limits.min_setback_back_m = Some(Param { value: Some(2.0), source: "  ".into() });
        p.structure.bearing_kpa = Some(150.0);
        p
    }

    #[test]
    fn sourced_and_unsourced_numbers_are_told_apart() {
        let f = audit(&base()).unwrap();
        let front = f.entries.iter().find(|e| e.path == "limits.minSetbackFrontM.value").unwrap();
        assert_eq!((front.status, front.source.as_deref(), front.value), ("sourced", Some("Municipal circular 12"), 3.0));
        let back = f.entries.iter().find(|e| e.path == "limits.minSetbackBackM.value").unwrap();
        assert_eq!((back.status, back.source.clone()), ("unsourced", None), "a blank source is no source");
        let bearing = f.entries.iter().find(|e| e.path == "structure.bearing_kpa").unwrap();
        assert_eq!(bearing.status, "unsourced");
        assert_eq!(f.total, f.sourced + f.unsourced);
        assert!(f.unsourced_paths.contains(&"structure.bearing_kpa".to_string()));
    }

    #[test]
    fn sibling_source_and_date_win_and_a_date_needs_a_source() {
        let mut v = serde_json::json!({ "a": { "x": 1.5, "x_source": "SBC 201 table 4", "x_date": "2026-01-02", "y": 2.0, "date": "2026-03-04" } });
        let mut out = Vec::new();
        walk(&v, "", "", &mut Vec::new(), &mut out);
        let x = out.iter().find(|e| e.path == "a.x").unwrap();
        assert_eq!((x.source.as_deref(), x.date.as_deref()), (Some("SBC 201 table 4"), Some("2026-01-02")));
        let y = out.iter().find(|e| e.path == "a.y").unwrap();
        assert_eq!((y.status, y.date.clone()), ("unsourced", None), "a date alone does not source a number");
        v["a"]["source"] = serde_json::json!("Memo 9");
        let mut out2 = Vec::new();
        walk(&v, "", "", &mut Vec::new(), &mut out2);
        let y2 = out2.iter().find(|e| e.path == "a.y").unwrap();
        assert_eq!((y2.source.as_deref(), y2.date.as_deref()), (Some("Memo 9"), Some("2026-03-04")));
    }

    #[test]
    fn arrays_inherit_and_history_is_skipped() {
        let v = serde_json::json!({ "plot": { "source": "Deed 5", "pts": [[0.0, 1.0], [2.0, 3.0]] }, "snapshots": [{ "n": 9 }], "baseline": { "n": 8 } });
        let mut out = Vec::new();
        walk(&v, "", "", &mut Vec::new(), &mut out);
        assert_eq!(out.len(), 4);
        assert!(out.iter().all(|e| e.status == "sourced" && e.source.as_deref() == Some("Deed 5")));
        assert_eq!(out[0].path, "plot.pts[0][0]");
    }

    #[test]
    fn csv_quotes_and_neutralises_formulas() {
        let mut f = audit(&base()).unwrap();
        f.entries.clear();
        f.entries.push(AuditEntry { path: "p".into(), value: 1.0, source: Some("=cmd|' /c calc'!A1, \"x\"".into()), date: None, status: "sourced" });
        let c = to_csv(&f);
        assert!(c.starts_with("path,value,status,source,date\n"));
        assert!(c.contains("\"'=cmd|' /c calc'!A1, \"\"x\"\"\""), "{c}");
    }
}
