//! Licensed code packs (SBC 2024 and any other edition the user holds a licence for).
//!
//! NanoArch ships no text of SBC 2024. When the user holds the licensed text they (or their office) transcribe it into a rule pack
//! (`rules::PackDef` plus two header objects) and load it here. Loading is stricter than for an ordinary overlay, because the values
//! are statutory:
//! - the pack declares `code` (for example `SBC 301`), an `edition` containing `2024`, and a `license` object
//!   (`holder`, `reference`, optional `date`) saying whose licence the text was taken under;
//! - every rule has a clause, and every parameter that carries a value has a source that names a page (`p.`, `pp.`, `page` or `ص`
//!   followed by a number), so a value can be traced back to the printed text;
//! - a parameter whose value is `null` stays unset: its rule is reported as skipped, never as passed.
//!
//! `audit` is a pure function: it reads the pack and returns everything wrong with it, by name, plus what it contains. The loader in
//! the backend then stores it as an overlay (review, confirmation, undo and the `.nnx` file all follow the overlay path).

use crate::overlay::MAX_BYTES;
use crate::rules::{PackDef, Param};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Edition marker the pack must carry in its `edition`.
pub const EDITION_MARK: &str = "2024";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct License {
    /// Who holds the licence (person, office or organisation).
    pub holder: String,
    /// The licence or purchase reference, or the document the text was transcribed from.
    pub reference: String,
    #[serde(default)]
    pub date: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Audit {
    pub code: String,
    pub edition: String,
    pub license: Option<License>,
    pub rules: usize,
    /// Parameters that carry a value (each with a page-level source).
    pub params_set: usize,
    /// `rule.param` of every parameter left `null`: its rule is skipped when checking.
    pub params_unset: Vec<String>,
    /// Everything that makes the pack unacceptable. Empty when it may be loaded.
    pub errors: Vec<String>,
}

/// True when a source names a page: a page marker (not inside another word) followed, after optional space or dot, by a digit.
pub fn names_a_page(source: &str) -> bool {
    let s = source.to_lowercase();
    ["pp.", "p.", "page", "ص"].iter().any(|m| {
        s.match_indices(m).any(|(i, _)| {
            let before_ok = *m == "ص" || !s[..i].chars().next_back().is_some_and(char::is_alphanumeric);
            let next = s[i + m.len()..].trim_start_matches([' ', '.', ':', '\u{a0}']).chars().next();
            before_ok && next.is_some_and(|c| c.is_ascii_digit() || ('\u{660}'..='\u{669}').contains(&c))
        })
    })
}

fn check_param(a: &mut Audit, what: String, p: &Param) {
    match p.value {
        None => a.params_unset.push(what),
        Some(v) => {
            if !v.is_finite() {
                a.errors.push(format!("{what}: the value is not a number"));
            } else if p.source.trim().is_empty() {
                a.errors.push(format!("{what}: has a value but no source"));
            } else if !names_a_page(&p.source) {
                a.errors.push(format!("{what}: the source '{}' names no page (write p. N of the licensed text)", p.source.trim()));
            } else {
                a.params_set += 1;
            }
        }
    }
}

/// Reads a licensed pack and reports its content and every problem with it. Never loads or stores anything.
pub fn audit(pack: &str) -> Audit {
    let mut a = Audit::default();
    if pack.len() > MAX_BYTES {
        a.errors.push(format!("the pack is larger than {MAX_BYTES} bytes"));
        return a;
    }
    let raw: Value = match serde_json::from_str(pack) {
        Ok(v) => v,
        Err(e) => {
            a.errors.push(format!("json: {e}"));
            return a;
        }
    };
    a.code = raw.get("code").and_then(Value::as_str).map(str::trim).unwrap_or("").to_string();
    if a.code.is_empty() {
        a.errors.push("the pack names no `code` (for example \"SBC 301\")".into());
    }
    match raw.get("license") {
        None | Some(Value::Null) => a.errors.push("the pack has no `license` (holder and reference): a licensed pack must say whose licence its text was taken under".into()),
        Some(l) => match serde_json::from_value::<License>(l.clone()) {
            Ok(l) if l.holder.trim().is_empty() => a.errors.push("the licence names no holder".into()),
            Ok(l) if l.reference.trim().is_empty() => a.errors.push("the licence names no reference (licence number or source document)".into()),
            Ok(l) => a.license = Some(l),
            Err(e) => a.errors.push(format!("license: {e}")),
        },
    }
    let def: PackDef = match serde_json::from_value(raw) {
        Ok(d) => d,
        Err(e) => {
            a.errors.push(format!("pack: {e}"));
            return a;
        }
    };
    a.edition = def.edition.trim().to_string();
    if !a.edition.contains(EDITION_MARK) {
        a.errors.push(format!("the edition '{}' is not a {EDITION_MARK} edition", a.edition));
    }
    if def.id.trim().is_empty() || def.version.trim().is_empty() {
        a.errors.push("the pack needs an id and a version".into());
    }
    if def.rules.is_empty() && def.overrides.is_empty() {
        a.errors.push("the pack has no rules and no overrides".into());
    }
    a.rules = def.rules.len();
    for r in &def.rules {
        if r.clause.trim().is_empty() {
            a.errors.push(format!("rule '{}': no clause reference", r.id));
        }
        for (n, p) in &r.params {
            check_param(&mut a, format!("{}.{n}", r.id), p);
        }
    }
    for (rule, ps) in &def.overrides {
        for (n, p) in ps {
            check_param(&mut a, format!("{rule}.{n} (override)"), p);
        }
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{ "id": "SBC301-2024", "edition": "2024", "version": "1", "code": "SBC 301",
        "license": { "holder": "Synthetic Office", "reference": "licence SYN-1", "date": "2027-01-01" },
        "rules": [
          { "id": "r1", "clause": "5.2", "applies_to": { "kind": "Space" }, "check": "area >= param.min",
            "params": { "min": { "value": 9, "source": "SBC 301 2024, table 5.2, p. 41" } }, "severity": "error", "message": "small" },
          { "id": "r2", "clause": "5.3", "applies_to": { "kind": "Space" }, "check": "area >= param.min",
            "params": { "min": { "value": null, "source": "" } }, "severity": "error", "message": "small" } ] }"#;

    #[test]
    fn a_pack_with_licence_clauses_and_page_sources_is_accepted_and_unset_values_are_listed() {
        let a = audit(GOOD);
        assert!(a.errors.is_empty(), "{:?}", a.errors);
        assert_eq!((a.rules, a.params_set), (2, 1));
        assert_eq!(a.params_unset, vec!["r2.min".to_string()]);
        assert_eq!(a.license.unwrap().holder, "Synthetic Office");
    }

    #[test]
    fn each_missing_proof_is_refused_by_name() {
        let cases = [
            (GOOD.replace(", p. 41", ""), "names no page"),
            (GOOD.replace("\"edition\": \"2024\"", "\"edition\": \"2007\""), "not a 2024 edition"),
            (GOOD.replace("\"code\": \"SBC 301\",", ""), "no `code`"),
            (GOOD.replace("\"holder\": \"Synthetic Office\"", "\"holder\": \" \""), "no holder"),
            (GOOD.replace("\"clause\": \"5.2\"", "\"clause\": \"\""), "no clause"),
            (GOOD.replace("SBC 301 2024, table 5.2, p. 41", ""), "no source"),
        ];
        for (pack, want) in cases {
            let a = audit(&pack);
            assert!(a.errors.iter().any(|e| e.contains(want)), "{want}: {:?}", a.errors);
        }
        let no_licence = GOOD.replace(r#""license": { "holder": "Synthetic Office", "reference": "licence SYN-1", "date": "2027-01-01" },"#, "");
        assert!(audit(&no_licence).errors.iter().any(|e| e.contains("no `license`")));
        assert!(audit("{").errors[0].starts_with("json"));
    }

    #[test]
    fn page_markers_need_a_number_and_do_not_match_inside_words() {
        for ok in ["p. 41", "pp.12-13", "page 7", "ص 55", "ص٥٥", "x, P.3"] {
            assert!(names_a_page(ok), "{ok}");
        }
        for bad in ["", "table 5.2", "p.", "paper 4", "page", "map 3"] {
            assert!(!names_a_page(bad), "{bad}");
        }
    }
}
