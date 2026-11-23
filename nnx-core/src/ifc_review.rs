//! Review of an existing IFC file (STEP, ISO 10303-21): what it contains and what is wrong with it.
//!
//! This is a READ-ONLY review. It does not turn the file into a NanoArch model; it reports what the file says and where
//! the file is incomplete or inconsistent, so the user can decide what to do with it.
//!
//! - **Formats.** Plain STEP text (`.ifc`) is read. A Revit file (`.rvt`/`.rfa`, a proprietary binary) cannot be read
//!   without Autodesk's software: it is detected and refused with the instruction to export IFC from Revit. `.ifcZIP` and
//!   `.ifcXML` are detected and refused with the reason.
//! - **Units.** Taken from the project's `IfcUnitAssignment`. A length or area unit that is missing or ambiguous is reported
//!   and every value that needs it is left unset (`None`); no unit is assumed.
//! - **Areas.** A space's area is its `Qto` quantity (`NetFloorArea`, then `GrossFloorArea`) when the file carries one, else
//!   the area of its extruded rectangle or closed polyline profile. The source of each area is stated. No area is estimated.
//! - **Findings.** Only facts checkable from the file: dangling references, duplicate GlobalIds, missing project, units or
//!   storeys, elements outside any storey, doors and windows that fill no opening, spaces without an area. No statutory
//!   value is applied here: compliance is the rule engine's job, on a NanoArch model.

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

// ------------------------------------------------------------------ STEP values

#[derive(Debug, Clone, PartialEq)]
enum V {
    Null,
    Star,
    Num(f64),
    Str(String),
    Enum(String),
    Ref(u64),
    List(Vec<V>),
    Typed(String, Vec<V>),
}

impl V {
    fn str(&self) -> Option<&str> {
        match self {
            V::Str(s) => Some(s),
            V::Typed(_, a) if a.len() == 1 => a[0].str(),
            _ => None,
        }
    }
    fn num(&self) -> Option<f64> {
        match self {
            V::Num(n) => Some(*n),
            V::Typed(_, a) if a.len() == 1 => a[0].num(),
            _ => None,
        }
    }
    fn reference(&self) -> Option<u64> {
        if let V::Ref(r) = self { Some(*r) } else { None }
    }
    fn enum_name(&self) -> Option<&str> {
        if let V::Enum(e) = self { Some(e) } else { None }
    }
    fn refs(&self) -> Vec<u64> {
        match self {
            V::Ref(r) => vec![*r],
            V::List(l) => l.iter().flat_map(V::refs).collect(),
            _ => Vec::new(),
        }
    }
    /// Every reference anywhere inside the value (lists and typed values included).
    fn all_refs(&self, out: &mut Vec<u64>) {
        match self {
            V::Ref(r) => out.push(*r),
            V::List(l) | V::Typed(_, l) => l.iter().for_each(|v| v.all_refs(out)),
            _ => {}
        }
    }
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    fn value(&mut self) -> Result<V, String> {
        self.ws();
        let Some(&c) = self.s.get(self.i) else { return Err("the statement ends inside a value".into()) };
        match c {
            b'$' => {
                self.i += 1;
                Ok(V::Null)
            }
            b'*' => {
                self.i += 1;
                Ok(V::Star)
            }
            b'#' => {
                self.i += 1;
                let st = self.i;
                while self.s.get(self.i).is_some_and(u8::is_ascii_digit) {
                    self.i += 1;
                }
                std::str::from_utf8(&self.s[st..self.i]).ok().and_then(|t| t.parse().ok()).map(V::Ref).ok_or_else(|| "a reference without a number".to_string())
            }
            b'\'' => self.string(),
            b'.' => {
                self.i += 1;
                let st = self.i;
                while self.s.get(self.i).is_some_and(|c| *c != b'.') {
                    self.i += 1;
                }
                if self.s.get(self.i).is_none() {
                    return Err("an enumeration is not closed".into());
                }
                let name = String::from_utf8_lossy(&self.s[st..self.i]).to_ascii_uppercase();
                self.i += 1;
                Ok(V::Enum(name))
            }
            b'(' => self.list().map(V::List),
            b'0'..=b'9' | b'-' | b'+' => {
                let st = self.i;
                while self.s.get(self.i).is_some_and(|c| matches!(c, b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')) {
                    self.i += 1;
                }
                let t = std::str::from_utf8(&self.s[st..self.i]).unwrap_or("");
                t.parse::<f64>().map(V::Num).map_err(|_| format!("'{t}' is not a number"))
            }
            c if c.is_ascii_alphabetic() => {
                let st = self.i;
                while self.s.get(self.i).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') {
                    self.i += 1;
                }
                let name = String::from_utf8_lossy(&self.s[st..self.i]).to_ascii_uppercase();
                self.ws();
                if self.s.get(self.i) != Some(&b'(') {
                    return Err(format!("'{name}' is not followed by its arguments"));
                }
                Ok(V::Typed(name, self.list()?))
            }
            other => Err(format!("unexpected character '{}'", other as char)),
        }
    }

    fn list(&mut self) -> Result<Vec<V>, String> {
        self.i += 1; // '('
        let mut out = Vec::new();
        loop {
            self.ws();
            match self.s.get(self.i) {
                None => return Err("a list is not closed".into()),
                Some(b')') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b',') => {
                    self.i += 1;
                }
                Some(_) => out.push(self.value()?),
            }
        }
    }

    fn string(&mut self) -> Result<V, String> {
        self.i += 1; // opening quote
        let mut raw = Vec::new();
        loop {
            match self.s.get(self.i) {
                None => return Err("a string is not closed".into()),
                Some(b'\'') if self.s.get(self.i + 1) == Some(&b'\'') => {
                    raw.push(b'\'');
                    self.i += 2;
                }
                Some(b'\'') => {
                    self.i += 1;
                    return Ok(V::Str(decode_step(&String::from_utf8_lossy(&raw))));
                }
                Some(&c) => {
                    raw.push(c);
                    self.i += 1;
                }
            }
        }
    }
}

/// STEP string escapes: `\\`, `\X2\hhhh..\X0\` (UTF-16), `\X\hh` and `\S\c` (Latin-1). Other directives are dropped.
fn decode_step(raw: &str) -> String {
    let ch: Vec<char> = raw.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let at = |i: usize, pat: &str| pat.chars().enumerate().all(|(k, p)| ch.get(i + k) == Some(&p));
    while i < ch.len() {
        if ch[i] != '\\' {
            out.push(ch[i]);
            i += 1;
        } else if at(i, "\\\\") {
            out.push('\\');
            i += 2;
        } else if at(i, "\\X2\\") {
            let st = i + 4;
            let mut j = st;
            while j < ch.len() && !at(j, "\\X0\\") {
                j += 1;
            }
            let hex: String = ch[st..j.min(ch.len())].iter().collect();
            let units: Vec<u16> = (0..hex.len() / 4).filter_map(|k| u16::from_str_radix(&hex[k * 4..k * 4 + 4], 16).ok()).collect();
            out.push_str(&String::from_utf16_lossy(&units));
            i = (j + 4).min(ch.len());
        } else if at(i, "\\X\\") && i + 5 <= ch.len() {
            let hex: String = ch[i + 3..i + 5].iter().collect();
            if let Ok(b) = u8::from_str_radix(&hex, 16) {
                out.push(b as char);
            }
            i += 5;
        } else if at(i, "\\S\\") && i + 4 <= ch.len() {
            out.push(((ch[i + 3] as u32 + 128).min(255) as u8) as char);
            i += 4;
        } else if at(i, "\\P") && ch.get(i + 3) == Some(&'\\') {
            i += 4;
        } else {
            out.push('\\');
            i += 1;
        }
    }
    out
}

/// Splits the text into statements at `;` outside strings and `/* */` comments.
fn statements(text: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let (mut in_str, mut i) = (false, 0);
    while i < text.len() {
        let c = text[i];
        if in_str {
            cur.push(c);
            if c == b'\'' {
                if text.get(i + 1) == Some(&b'\'') {
                    cur.push(b'\'');
                    i += 1;
                } else {
                    in_str = false;
                }
            }
        } else if c == b'/' && text.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < text.len() && !(text[i] == b'*' && text[i + 1] == b'/') {
                i += 1;
            }
            i += 1;
        } else if c == b';' {
            let s = String::from_utf8_lossy(&cur).trim().to_string();
            if !s.is_empty() {
                out.push(s);
            }
            cur.clear();
        } else {
            if c == b'\'' {
                in_str = true;
            }
            cur.push(c);
        }
        i += 1;
    }
    out
}

// ------------------------------------------------------------------ report

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub message_ar: String,
    /// How many entities are affected (1 for a file-level finding).
    pub count: usize,
    /// Up to 10 STEP ids (`#123`) of the affected entities.
    pub examples: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoreyInfo {
    pub id: String,
    pub name: Option<String>,
    /// Metres, from the file's length unit. `None` when the file gives no elevation or no usable unit.
    pub elevation_m: Option<f64>,
    /// Elements contained in or aggregated under the storey.
    pub elements: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpaceInfo {
    pub id: String,
    pub name: Option<String>,
    pub long_name: Option<String>,
    pub storey: Option<String>,
    pub area_m2: Option<f64>,
    /// `quantity NetFloorArea`, `quantity GrossFloorArea`, `rectangle profile` or `closed polyline profile`.
    pub area_source: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IfcReview {
    pub schema: Option<String>,
    pub project_name: Option<String>,
    pub entity_count: usize,
    /// Statements that could not be read (complex entities and malformed ones).
    pub unreadable_statements: usize,
    pub length_unit_m: Option<f64>,
    pub area_unit_m2: Option<f64>,
    /// Entity count by IFC class for the building classes the review cares about (upper case as in the file).
    pub counts: BTreeMap<String, usize>,
    pub storeys: Vec<StoreyInfo>,
    pub spaces: Vec<SpaceInfo>,
    /// Sum of the spaces that have an area; `spaces_without_area` are not in it.
    pub spaces_area_m2: f64,
    pub spaces_without_area: usize,
    /// Walls that carry a `Length` quantity, and the sum of those lengths in metres.
    pub walls_measured: usize,
    pub walls_length_m: f64,
    pub findings: Vec<Finding>,
}

/// Classes counted in `IfcReview::counts`.
const COUNTED: &[&str] = &[
    "IFCWALL",
    "IFCWALLSTANDARDCASE",
    "IFCSLAB",
    "IFCCOLUMN",
    "IFCBEAM",
    "IFCFOOTING",
    "IFCDOOR",
    "IFCWINDOW",
    "IFCSTAIR",
    "IFCROOF",
    "IFCSPACE",
    "IFCOPENINGELEMENT",
    "IFCBUILDINGSTOREY",
    "IFCBUILDINGELEMENTPROXY",
];

/// Classes that must sit in a storey.
const STOREY_BOUND: &[&str] = &["IFCWALL", "IFCWALLSTANDARDCASE", "IFCSLAB", "IFCCOLUMN", "IFCBEAM", "IFCDOOR", "IFCWINDOW", "IFCSPACE", "IFCSTAIR", "IFCROOF"];

type Ents = BTreeMap<u64, (String, Vec<V>)>;

fn prefix_factor(v: Option<&V>) -> Option<f64> {
    match v {
        None | Some(V::Null) => Some(1.0),
        Some(V::Enum(p)) => Some(match p.as_str() {
            "MILLI" => 1e-3,
            "CENTI" => 1e-2,
            "DECI" => 1e-1,
            "DECA" => 1e1,
            "HECTO" => 1e2,
            "KILO" => 1e3,
            "MICRO" => 1e-6,
            "MEGA" => 1e6,
            _ => return None,
        }),
        _ => None,
    }
}

/// Metres per unit (length) or square metres per unit (area) of one unit entity; `None` when the unit is not understood.
fn unit_factor(ents: &Ents, id: u64, depth: u8) -> Option<(String, f64)> {
    let (class, a) = ents.get(&id)?;
    let kind = a.get(1)?.enum_name()?.to_string();
    match class.as_str() {
        "IFCSIUNIT" => {
            let name = a.get(3)?.enum_name()?;
            match (kind.as_str(), name) {
                ("LENGTHUNIT", "METRE") => Some((kind, prefix_factor(a.get(2))?)),
                // A prefix on a square metre is ambiguous (milli-square-metre or square millimetre): refused, not guessed.
                ("AREAUNIT", "SQUARE_METRE") if matches!(a.get(2), None | Some(V::Null)) => Some((kind, 1.0)),
                _ => None,
            }
        }
        "IFCCONVERSIONBASEDUNIT" if depth < 3 => {
            let m = ents.get(&a.get(3)?.reference()?)?;
            if m.0 != "IFCMEASUREWITHUNIT" {
                return None;
            }
            let value = m.1.first()?.num()?;
            let (base_kind, base) = unit_factor(ents, m.1.get(1)?.reference()?, depth + 1)?;
            (base_kind == kind).then_some((kind, value * base))
        }
        _ => None,
    }
}

fn first_of(ents: &Ents, class: &str) -> Option<u64> {
    ents.iter().find(|(_, (c, _))| c == class).map(|(id, _)| *id)
}

/// Area (m2 as the unit-scaled number's square) of an extruded profile of a representation, with the profile kind.
fn profile_area(ents: &Ents, rep: u64, len_m: f64) -> Option<(f64, &'static str)> {
    let (c, a) = ents.get(&rep)?;
    if c != "IFCPRODUCTDEFINITIONSHAPE" {
        return None;
    }
    for r in a.get(2)?.refs() {
        let Some((rc, ra)) = ents.get(&r) else { continue };
        if rc != "IFCSHAPEREPRESENTATION" {
            continue;
        }
        for item in ra.get(3).map(V::refs).unwrap_or_default() {
            let Some((ic, ia)) = ents.get(&item) else { continue };
            if ic != "IFCEXTRUDEDAREASOLID" {
                continue;
            }
            let Some(p) = ia.first().and_then(V::reference) else { continue };
            let Some((pc, pa)) = ents.get(&p) else { continue };
            match pc.as_str() {
                "IFCRECTANGLEPROFILEDEF" => {
                    if let (Some(x), Some(y)) = (pa.get(3).and_then(V::num), pa.get(4).and_then(V::num))
                        && x > 0.0
                        && y > 0.0
                    {
                        return Some((x * y * len_m * len_m, "rectangle profile"));
                    }
                }
                "IFCARBITRARYCLOSEDPROFILEDEF" => {
                    let curve = pa.get(2).and_then(V::reference).and_then(|c| ents.get(&c));
                    if let Some((cc, ca)) = curve
                        && cc == "IFCPOLYLINE"
                    {
                        let pts: Vec<(f64, f64)> = ca
                            .first()
                            .map(V::refs)
                            .unwrap_or_default()
                            .iter()
                            .filter_map(|pid| ents.get(pid))
                            .filter(|(k, _)| k == "IFCCARTESIANPOINT")
                            .filter_map(|(_, v)| match v.first() {
                                Some(V::List(c)) if c.len() >= 2 => Some((c[0].num()?, c[1].num()?)),
                                _ => None,
                            })
                            .collect();
                        let n = pts.len();
                        if n >= 3 {
                            let mut s = 0.0;
                            for k in 0..n {
                                let (x1, y1) = pts[k];
                                let (x2, y2) = pts[(k + 1) % n];
                                s += x1 * y2 - x2 * y1;
                            }
                            if s.abs() > 0.0 {
                                return Some((s.abs() / 2.0 * len_m * len_m, "closed polyline profile"));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Reviews an IFC file given as bytes.
pub fn review_ifc(bytes: &[u8]) -> Result<IfcReview, String> {
    if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
        return Err("this is a Revit file (.rvt/.rfa), a proprietary binary that cannot be read without Autodesk Revit. In Revit use File > Export > IFC and open the .ifc file here / \
هذا ملف Revit بصيغة ثنائية مغلقة لا يُقرأ دون برنامج Revit. صدّره من Revit عبر File > Export > IFC ثم افتح ملف .ifc هنا"
            .into());
    }
    if bytes.starts_with(b"PK\x03\x04") {
        return Err("this is a zip archive (.ifcZIP): extract the .ifc file inside it and open that / هذا ملف مضغوط (ifcZIP): فُكّ ضغطه وافتح ملف .ifc بداخله".into());
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_string();
    if head.trim_start().starts_with("<?xml") || head.contains("<ifcXML") {
        return Err("this is ifcXML, which is not read; export the model as STEP (.ifc) / هذه صيغة ifcXML غير مدعومة، صدّر النموذج بصيغة STEP (.ifc)".into());
    }
    if !head.contains("ISO-10303-21") {
        return Err("this is not an IFC STEP file (the ISO-10303-21 header is missing) / ليس ملف IFC بصيغة STEP (ترويسة ISO-10303-21 غير موجودة)".into());
    }

    let mut ents: Ents = BTreeMap::new();
    let mut schema = None;
    let mut unreadable = 0usize;
    for st in statements(bytes) {
        if st.starts_with("FILE_SCHEMA") {
            if let Ok(V::Typed(_, a)) = (Parser { s: st.as_bytes(), i: 0 }).value()
                && let Some(V::List(l)) = a.first()
            {
                schema = l.first().and_then(V::str).map(str::to_string);
            }
            continue;
        }
        let Some(rest) = st.strip_prefix('#') else { continue };
        let Some((num, body)) = rest.split_once('=') else {
            unreadable += 1;
            continue;
        };
        let (Ok(id), body) = (num.trim().parse::<u64>(), body.trim()) else {
            unreadable += 1;
            continue;
        };
        if body.starts_with('(') {
            unreadable += 1; // complex (multi-class) instance
            continue;
        }
        match (Parser { s: body.as_bytes(), i: 0 }).value() {
            Ok(V::Typed(class, args)) => {
                ents.insert(id, (class, args));
            }
            _ => unreadable += 1,
        }
    }
    if ents.is_empty() {
        return Err("the file has no readable entities / لا يحتوي الملف على كيانات مقروءة".into());
    }

    let mut findings: Vec<Finding> = Vec::new();
    let mut add = |severity: Severity, code: &'static str, en: String, ar: String, ids: Vec<u64>| {
        let count = ids.len().max(1);
        findings.push(Finding { severity, code, message: en, message_ar: ar, count, examples: ids.iter().take(10).map(|i| format!("#{i}")).collect() });
    };

    if schema.is_none() {
        add(Severity::Warning, "no_schema", "The header has no FILE_SCHEMA.".into(), "ترويسة الملف بلا FILE_SCHEMA.".into(), vec![]);
    }
    if unreadable > 0 {
        add(
            Severity::Warning,
            "unreadable_statements",
            format!("{unreadable} statement(s) could not be read (complex or malformed instances); they are left out of the review."),
            format!("تعذّرت قراءة {unreadable} عبارة (كيانات مركّبة أو معطوبة) وهي خارج هذه المراجعة."),
            vec![],
        );
    }

    // Project and units.
    let project = first_of(&ents, "IFCPROJECT");
    let project_name = project.and_then(|p| ents[&p].1.get(2)).and_then(V::str).map(str::to_string);
    if project.is_none() {
        add(Severity::Error, "no_project", "The file has no IfcProject.".into(), "الملف بلا IfcProject.".into(), vec![]);
    }
    let (mut length_unit, mut area_unit) = (None, None);
    let mut area_prefixed = false;
    if let Some(p) = project
        && let Some(ua) = ents[&p].1.get(8).and_then(V::reference).and_then(|u| ents.get(&u))
        && ua.0 == "IFCUNITASSIGNMENT"
    {
        for u in ua.1.first().map(V::refs).unwrap_or_default() {
            match unit_factor(&ents, u, 0) {
                Some((k, f)) if k == "LENGTHUNIT" => length_unit = Some(f),
                Some((k, f)) if k == "AREAUNIT" => area_unit = Some(f),
                _ => {
                    if let Some((c, a)) = ents.get(&u)
                        && c == "IFCSIUNIT"
                        && a.get(1).and_then(V::enum_name) == Some("AREAUNIT")
                    {
                        area_prefixed = true;
                    }
                }
            }
        }
    }
    if length_unit.is_none() {
        add(
            Severity::Error,
            "no_length_unit",
            "The project has no readable length unit; elevations and profile areas are not computed.".into(),
            "لا وحدة طول مقروءة في المشروع، فلا تُحسب المناسيب ولا مساحات المقاطع.".into(),
            vec![],
        );
    }
    if area_unit.is_none() {
        let (en, ar) = if area_prefixed {
            ("The area unit carries an SI prefix, which is ambiguous; quantity areas are not read.", "وحدة المساحة عليها بادئة SI وهي ملتبسة، فلا تُقرأ مساحات الكميات.")
        } else {
            ("The project has no area unit; quantity areas are not read.", "لا وحدة مساحة في المشروع، فلا تُقرأ مساحات الكميات.")
        };
        add(Severity::Warning, "no_area_unit", en.into(), ar.into(), vec![]);
    }

    // Counts.
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (c, _) in ents.values() {
        if COUNTED.contains(&c.as_str()) {
            *counts.entry(c.clone()).or_default() += 1;
        }
    }

    // Dangling references and duplicate GlobalIds.
    let mut dangling: Vec<u64> = Vec::new();
    let mut seen_guid: BTreeMap<&str, u64> = BTreeMap::new();
    let mut dup: BTreeSet<u64> = BTreeSet::new();
    for (id, (_, args)) in &ents {
        let mut r = Vec::new();
        args.iter().for_each(|a| a.all_refs(&mut r));
        if r.iter().any(|x| !ents.contains_key(x)) {
            dangling.push(*id);
        }
        if let Some(g) = args.first().and_then(|a| if let V::Str(s) = a { Some(s.as_str()) } else { None }).filter(|g| g.len() == 22)
            && let Some(prev) = seen_guid.insert(g, *id)
        {
            dup.insert(prev);
            dup.insert(*id);
        }
    }
    if !dangling.is_empty() {
        add(
            Severity::Error,
            "dangling_reference",
            format!("{} entit(ies) refer to an id that is not defined in the file.", dangling.len()),
            format!("{} كيان يشير إلى معرّف غير معرَّف في الملف.", dangling.len()),
            dangling,
        );
    }
    if !dup.is_empty() {
        add(
            Severity::Error,
            "duplicate_guid",
            format!("{} entit(ies) share a GlobalId with another entity.", dup.len()),
            format!("{} كيان يتشارك GlobalId مع كيان آخر.", dup.len()),
            dup.into_iter().collect(),
        );
    }

    // Spatial structure: storey of each element.
    let storey_ids: Vec<u64> = ents.iter().filter(|(_, (c, _))| c == "IFCBUILDINGSTOREY").map(|(i, _)| *i).collect();
    let mut storey_of: BTreeMap<u64, u64> = BTreeMap::new();
    for (c, a) in ents.values() {
        let (parent, kids) = match c.as_str() {
            "IFCRELCONTAINEDINSPATIALSTRUCTURE" => (a.get(5).and_then(V::reference), a.get(4).map(V::refs)),
            "IFCRELAGGREGATES" => (a.get(4).and_then(V::reference), a.get(5).map(V::refs)),
            _ => continue,
        };
        if let (Some(p), Some(kids)) = (parent, kids)
            && storey_ids.contains(&p)
        {
            kids.into_iter().for_each(|k| {
                storey_of.insert(k, p);
            });
        }
    }
    let mut storeys: Vec<StoreyInfo> = storey_ids
        .iter()
        .map(|id| {
            let a = &ents[id].1;
            StoreyInfo {
                id: format!("#{id}"),
                name: a.get(2).and_then(V::str).map(str::to_string),
                elevation_m: a.get(9).and_then(V::num).zip(length_unit).map(|(e, f)| (e * f * 1e6).round() / 1e6),
                elements: storey_of.values().filter(|s| *s == id).count(),
            }
        })
        .collect();
    storeys.sort_by(|a, b| a.elevation_m.partial_cmp(&b.elevation_m).unwrap_or(std::cmp::Ordering::Equal));
    if storey_ids.is_empty() {
        add(Severity::Error, "no_storeys", "The file has no IfcBuildingStorey.".into(), "الملف بلا أي IfcBuildingStorey.".into(), vec![]);
    } else {
        let missing = storeys.iter().filter(|s| s.elevation_m.is_none()).count();
        if missing > 0 && length_unit.is_some() {
            add(Severity::Warning, "storey_without_elevation", format!("{missing} storey(s) have no elevation."), format!("{missing} طابق بلا منسوب."), vec![]);
        }
        let outside: Vec<u64> = ents.iter().filter(|(id, (c, _))| STOREY_BOUND.contains(&c.as_str()) && !storey_of.contains_key(id)).map(|(i, _)| *i).collect();
        // A door or window may be placed only through its host wall's opening: those are not "outside" a storey.
        if !outside.is_empty() {
            add(
                Severity::Warning,
                "element_outside_storey",
                format!("{} element(s) are not contained in or aggregated under any storey.", outside.len()),
                format!("{} عنصر غير محتوى في أي طابق.", outside.len()),
                outside,
            );
        }
    }

    // Doors and windows must fill an opening.
    let filled: BTreeSet<u64> = ents.values().filter(|(c, _)| c == "IFCRELFILLSELEMENT").filter_map(|(_, a)| a.get(5).and_then(V::reference)).collect();
    let unfilled: Vec<u64> = ents.iter().filter(|(id, (c, _))| (c == "IFCDOOR" || c == "IFCWINDOW") && !filled.contains(id)).map(|(i, _)| *i).collect();
    if !unfilled.is_empty() {
        add(
            Severity::Warning,
            "door_window_without_opening",
            format!("{} door(s) or window(s) fill no opening in a wall.", unfilled.len()),
            format!("{} باب أو نافذة لا تملأ فتحة في جدار.", unfilled.len()),
            unfilled,
        );
    }

    // Quantities per object.
    let mut qty: BTreeMap<u64, Vec<(String, String, f64)>> = BTreeMap::new();
    for (c, a) in ents.values() {
        if c != "IFCRELDEFINESBYPROPERTIES" {
            continue;
        }
        let Some(def) = a.get(5).and_then(V::reference).and_then(|d| ents.get(&d)) else { continue };
        if def.0 != "IFCELEMENTQUANTITY" {
            continue;
        }
        let list: Vec<(String, String, f64)> = def
            .1
            .get(5)
            .map(V::refs)
            .unwrap_or_default()
            .iter()
            .filter_map(|q| ents.get(q))
            .filter_map(|(qc, qa)| Some((qa.first()?.str()?.to_string(), qc.clone(), qa.get(3)?.num()?)))
            .collect();
        for target in a.get(4).map(V::refs).unwrap_or_default() {
            qty.entry(target).or_default().extend(list.iter().cloned());
        }
    }

    // Spaces.
    let mut spaces = Vec::new();
    for (id, (c, a)) in &ents {
        if c != "IFCSPACE" {
            continue;
        }
        let q = qty.get(id).map(Vec::as_slice).unwrap_or(&[]);
        let named = |n: &str| q.iter().find(|(qn, qc, _)| qn == n && qc == "IFCQUANTITYAREA").map(|(_, _, v)| *v);
        let from_qty = area_unit.and_then(|f| named("NetFloorArea").map(|v| (v * f, "quantity NetFloorArea")).or_else(|| named("GrossFloorArea").map(|v| (v * f, "quantity GrossFloorArea"))));
        let from_geom = || length_unit.and_then(|lf| a.get(6).and_then(V::reference).and_then(|r| profile_area(&ents, r, lf)));
        let area = from_qty.or_else(from_geom);
        spaces.push(SpaceInfo {
            id: format!("#{id}"),
            name: a.get(2).and_then(V::str).map(str::to_string),
            long_name: a.get(7).and_then(V::str).map(str::to_string),
            storey: storey_of.get(id).and_then(|s| ents[s].1.get(2)).and_then(V::str).map(str::to_string),
            area_m2: area.map(|(v, _)| (v * 1e4).round() / 1e4),
            area_source: area.map(|(_, s)| s.to_string()),
        });
    }
    let no_area: Vec<u64> = spaces.iter().filter(|s| s.area_m2.is_none()).filter_map(|s| s.id[1..].parse().ok()).collect();
    if !no_area.is_empty() {
        add(
            Severity::Warning,
            "space_without_area",
            format!("{} space(s) have neither an area quantity nor a readable rectangular/polyline profile.", no_area.len()),
            format!("{} فراغ بلا كمية مساحة ولا مقطع مستطيل أو متعدد الخطوط مقروء.", no_area.len()),
            no_area.clone(),
        );
    }
    if !ents.values().any(|(c, _)| c == "IFCSPACE") {
        add(Severity::Info, "no_spaces", "The file has no IfcSpace: rooms cannot be reviewed.".into(), "الملف بلا IfcSpace: لا يمكن مراجعة الغرف.".into(), vec![]);
    }

    // Walls.
    let (mut walls_measured, mut walls_length) = (0usize, 0.0f64);
    if let Some(lf) = length_unit {
        for (id, (c, _)) in &ents {
            if c != "IFCWALL" && c != "IFCWALLSTANDARDCASE" {
                continue;
            }
            if let Some((_, _, v)) = qty.get(id).and_then(|q| q.iter().find(|(n, k, _)| n == "Length" && k == "IFCQUANTITYLENGTH")) {
                walls_measured += 1;
                walls_length += v * lf;
            }
        }
    }

    let spaces_area: f64 = spaces.iter().filter_map(|s| s.area_m2).sum();
    let rank = |s: Severity| match s {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Info => 2,
    };
    findings.sort_by_key(|f| rank(f.severity));
    Ok(IfcReview {
        schema,
        project_name,
        entity_count: ents.len(),
        unreadable_statements: unreadable,
        length_unit_m: length_unit,
        area_unit_m2: area_unit,
        counts,
        storeys,
        spaces_area_m2: (spaces_area * 1e4).round() / 1e4,
        spaces_without_area: no_area.len(),
        spaces,
        walls_measured,
        walls_length_m: (walls_length * 1e4).round() / 1e4,
        findings,
    })
}
