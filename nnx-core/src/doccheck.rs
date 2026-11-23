//! Missing permit-documents checklist.
//!
//! Which documents a permit application needs is set by the approving authority, so no list is built in. The user enters the
//! required list (each item with its key and name) and `list_source` (the authority's document, issuer and date), then the
//! documents already prepared. The check reports every required item as `submitted` or `missing`, the submitted entries that
//! match no required item (a typo is never silently dropped), and an overall verdict. A missing input is named and nothing is
//! built. It does not judge the CONTENT of a document, only that an entry exists for it.

use crate::permitpack::valid_date;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// The most required items accepted in one list.
pub const MAX_REQUIRED: usize = 500;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Required {
    /// Unique key the submitted entries refer to.
    pub key: String,
    pub name: String,
    /// Optional grouping (for example "Architectural", "Structural").
    pub group: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Submitted {
    pub key: String,
    /// The drawing/sheet/document number or file the user points to. Required: an entry without one is not evidence.
    pub reference: String,
    /// Optional date of the document, `YYYY-MM-DD`.
    pub date: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Input {
    pub required: Vec<Required>,
    /// Where the required list comes from (document, issuer, date).
    pub list_source: String,
    pub submitted: Vec<Submitted>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Row {
    pub key: String,
    pub name: String,
    pub group: String,
    /// `submitted` or `missing`.
    pub status: &'static str,
    pub reference: Option<String>,
    pub date: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub rows: Vec<Row>,
    /// Submitted keys that match no required item.
    pub unmatched: Vec<String>,
    pub required: usize,
    pub submitted: usize,
    pub missing: usize,
    /// `complete` when nothing is missing, else `incomplete`.
    pub overall: &'static str,
    pub list_source: String,
    pub csv: String,
    pub notes: Vec<String>,
}

fn csv_field(s: &str) -> String {
    // Neutralise spreadsheet formula injection from user text, then quote.
    let guarded = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{s}") } else { s.to_string() };
    format!("\"{}\"", guarded.replace('"', "\"\""))
}

/// Builds the checklist. Refuses, naming each problem, when an input is missing or malformed.
pub fn check(input: &Input) -> Result<Report, String> {
    let mut errs: Vec<String> = Vec::new();
    if input.list_source.trim().is_empty() {
        errs.push("list_source (the authority's document, issuer and date the required list comes from)".into());
    }
    if input.required.is_empty() {
        errs.push("required (the list of required documents)".into());
    }
    if input.required.len() > MAX_REQUIRED {
        errs.push(format!("required has {} items; the limit is {MAX_REQUIRED}", input.required.len()));
    }
    let mut keys: HashSet<&str> = HashSet::new();
    for (i, r) in input.required.iter().enumerate() {
        let k = r.key.trim();
        if k.is_empty() {
            errs.push(format!("required[{i}].key"));
        } else if !keys.insert(k) {
            errs.push(format!("required[{i}].key `{k}` is duplicated"));
        }
        if r.name.trim().is_empty() {
            errs.push(format!("required[{i}].name"));
        }
    }
    let mut sub: HashMap<&str, &Submitted> = HashMap::new();
    for (i, s) in input.submitted.iter().enumerate() {
        let k = s.key.trim();
        if k.is_empty() {
            errs.push(format!("submitted[{i}].key"));
            continue;
        }
        if s.reference.trim().is_empty() {
            errs.push(format!("submitted[{i}].reference (an entry without a drawing number or file is not evidence)"));
        }
        if !s.date.trim().is_empty() && !valid_date(s.date.trim()) {
            errs.push(format!("submitted[{i}].date must be YYYY-MM-DD, got `{}`", s.date.trim()));
        }
        if sub.insert(k, s).is_some() {
            errs.push(format!("submitted[{i}].key `{k}` is duplicated"));
        }
    }
    if !errs.is_empty() {
        return Err(format!("documents checklist: {}. Nothing is defaulted and no document list is built in; enter them.", errs.join("; ")));
    }

    let rows: Vec<Row> = input
        .required
        .iter()
        .map(|r| {
            let found = sub.get(r.key.trim());
            Row {
                key: r.key.trim().to_string(),
                name: r.name.trim().to_string(),
                group: r.group.trim().to_string(),
                status: if found.is_some() { "submitted" } else { "missing" },
                reference: found.map(|s| s.reference.trim().to_string()),
                date: found.and_then(|s| (!s.date.trim().is_empty()).then(|| s.date.trim().to_string())),
            }
        })
        .collect();
    let unmatched: Vec<String> = input.submitted.iter().map(|s| s.key.trim()).filter(|k| !keys.contains(k)).map(str::to_string).collect();
    let missing = rows.iter().filter(|r| r.status == "missing").count();
    let submitted = rows.len() - missing;

    let mut csv = String::from("key,name,group,status,reference,date\n");
    for r in &rows {
        csv.push_str(&format!(
            "{},{},{},{},{},{}\n",
            csv_field(&r.key),
            csv_field(&r.name),
            csv_field(&r.group),
            r.status,
            csv_field(r.reference.as_deref().unwrap_or("")),
            csv_field(r.date.as_deref().unwrap_or(""))
        ));
    }
    let mut notes = vec![
        "The required list is the user's input with its stated source; the application does not know any authority's list. It checks that an entry exists for each item, not that the document is correct or complete / القائمة المطلوبة من إدخال المستخدم بمصدرها؛ يفحص وجود مدخل لكل بند لا صحة الوثيقة.".to_string(),
    ];
    if !unmatched.is_empty() {
        notes.push(format!("Submitted keys that match no required item (check for typos): {} / مفاتيح مقدّمة لا تطابق أي بند مطلوب.", unmatched.join(", ")));
    }
    Ok(Report { rows, unmatched, required: input.required.len(), submitted, missing, overall: if missing == 0 { "complete" } else { "incomplete" }, list_source: input.list_source.trim().to_string(), csv, notes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(k: &str, n: &str) -> Required {
        Required { key: k.into(), name: n.into(), group: String::new() }
    }
    fn sub(k: &str, r: &str, d: &str) -> Submitted {
        Submitted { key: k.into(), reference: r.into(), date: d.into() }
    }
    fn base() -> Input {
        Input { required: vec![req("deed", "Title deed"), req("arch", "Architectural drawings"), req("str", "Structural calcs")], list_source: "Municipality form 3, 2026-01-01".into(), submitted: vec![sub("arch", "A-01..A-05", "2026-02-03")] }
    }

    #[test]
    fn marks_each_required_item_and_reports_missing_and_unmatched() {
        let mut i = base();
        i.submitted.push(sub("archh", "typo.pdf", ""));
        let r = check(&i).unwrap();
        assert_eq!((r.required, r.submitted, r.missing), (3, 1, 2));
        assert_eq!(r.overall, "incomplete");
        assert_eq!(r.rows[1].status, "submitted");
        assert_eq!(r.rows[1].reference.as_deref(), Some("A-01..A-05"));
        assert_eq!(r.rows[0].status, "missing");
        assert_eq!(r.unmatched, vec!["archh".to_string()]);
        assert!(r.notes.iter().any(|n| n.contains("archh")));
    }

    #[test]
    fn complete_only_when_nothing_is_missing() {
        let mut i = base();
        i.submitted = vec![sub("deed", "D1", ""), sub("arch", "A1", ""), sub("str", "S1", "")];
        let r = check(&i).unwrap();
        assert_eq!((r.missing, r.overall), (0, "complete"));
    }

    #[test]
    fn missing_or_malformed_inputs_are_named_and_nothing_is_built_in() {
        let e = check(&Input::default()).unwrap_err();
        assert!(e.contains("list_source") && e.contains("required"), "{e}");
        let mut i = base();
        i.required.push(req("deed", " "));
        i.submitted = vec![sub("arch", " ", "03/02/2026")];
        let e = check(&i).unwrap_err();
        assert!(e.contains("duplicated") && e.contains("required[3].name") && e.contains("reference") && e.contains("YYYY-MM-DD"), "{e}");
    }

    #[test]
    fn csv_neutralises_formulas_and_quotes() {
        let mut i = base();
        i.required[0].name = "=cmd|' /C calc'!A0".into();
        i.required[1].name = "Plans, \"final\"".into();
        let r = check(&i).unwrap();
        assert!(r.csv.starts_with("key,name,group,status,reference,date\n"));
        assert!(r.csv.contains("\"'=cmd") && r.csv.contains("\"Plans, \"\"final\"\"\""), "{}", r.csv);
    }
}
