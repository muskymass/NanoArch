//! A land title deed (صك) or a survey sketch (كروكي), scanned or photographed, read into plot facts for the user to review.
//!
//! - **The model only transcribes.** A vision model on this computer (see `src/ollama.rs`) returns what is WRITTEN on the page:
//!   the deed number, its date as written, the plot and plan numbers, the district and city, the stated area and, for each of the
//!   four directions, the stated length and the neighbour. The owner's name and identity number are not asked for and not kept.
//! - **PDF without an external renderer.** A scanned deed is a PDF whose pages are embedded JPEG images; `images_from_pdf` pulls
//!   those streams out byte for byte (no decoding), largest first. A PDF whose pages are text or compressed some other way holds
//!   nothing this can pull out, and is refused with the reason: no OCR and no guess.
//! - **Checked, never completed.** A value that is not a positive number, a direction that is not one of the four, or a repeated
//!   direction is dropped or flagged and named. The sides are compared with the stated area only under a tolerance the user
//!   enters (no default): `ok`, `conflict`, `unchecked` (no tolerance), `not_rectangular` (opposite sides differ by more than the
//!   tolerance, so four lengths cannot give an area), `incomplete` (a direction is missing or flagged) or `no_stated_area`.
//! - **Reviewed.** Every reading comes back `unconfirmed`. `accept` turns only the facts the user names into plot values for the
//!   brief; a value with a conflict or an issue is refused until the user types the correct one.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// How many page images are sent to the model for one deed.
pub const MAX_PDF_IMAGES: usize = 4;
const DIRECTIONS: [&str; 4] = ["north", "south", "east", "west"];
const MAX_TEXT_CHARS: usize = 200;

fn find_from(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= hay.len() || needle.is_empty() {
        return None;
    }
    hay[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    find_from(hay, needle, 0).is_some()
}

/// The JPEG page images embedded in a scanned PDF, largest pixel area first, at most `MAX_PDF_IMAGES`, with notes about what was
/// left out. Streams are returned unchanged; an image stored with any other filter (or with a second filter in front of the
/// JPEG one) is counted in the notes and not returned.
pub fn images_from_pdf(bytes: &[u8]) -> Result<(Vec<Vec<u8>>, Vec<String>), String> {
    if !bytes.starts_with(b"%PDF") {
        return Err("the file is not a PDF".into());
    }
    if contains(bytes, b"/Encrypt") {
        return Err("the PDF is encrypted: save an unprotected copy, or export its page as PNG or JPEG".into());
    }
    let mut found: Vec<(u64, Vec<u8>)> = Vec::new();
    let mut other_filter = 0usize;
    let mut pos = 0;
    while let Some(i) = find_from(bytes, b"stream", pos) {
        if i >= 3 && &bytes[i - 3..i] == b"end" {
            pos = i + 6;
            continue;
        }
        let mut start = i + 6;
        if bytes[start..].starts_with(b"\r\n") {
            start += 2;
        } else if bytes[start..].starts_with(b"\n") || bytes[start..].starts_with(b"\r") {
            start += 1;
        } else {
            pos = i + 6;
            continue;
        }
        let end = find_from(bytes, b"endstream", start).ok_or("the PDF is damaged: a stream has no end")?;
        pos = end + 9;
        let dict_start = bytes[..i].windows(3).rposition(|w| w == b"obj").unwrap_or(0);
        let dict = &bytes[dict_start..i];
        if !contains(dict, b"/Image") {
            continue;
        }
        if !contains(dict, b"/DCTDecode") || contains(dict, b"/FlateDecode") || contains(dict, b"/LZWDecode") || contains(dict, b"/ASCII") || contains(dict, b"/RunLengthDecode") {
            other_filter += 1;
            continue;
        }
        let mut data = &bytes[start..end];
        while let Some((last, rest)) = data.split_last() {
            if matches!(last, b'\r' | b'\n' | b' ') {
                data = rest;
            } else {
                break;
            }
        }
        if !(data.starts_with(&[0xFF, 0xD8]) && data.ends_with(&[0xFF, 0xD9])) {
            other_filter += 1;
            continue;
        }
        match crate::sketchread::image_size(data) {
            Ok((w, h)) => found.push((w as u64 * h as u64, data.to_vec())),
            Err(_) => other_filter += 1,
        }
    }
    let mut notes = Vec::new();
    if other_filter > 0 {
        notes.push(format!("{other_filter} image(s) in the PDF are not plain JPEG streams and were not read"));
    }
    if found.is_empty() {
        return Err("the PDF has no embedded JPEG page image: a scan is read, a PDF of text or of compressed images is not. Export the page as PNG or JPEG and give that".into());
    }
    found.sort_by_key(|a| std::cmp::Reverse(a.0));
    if found.len() > MAX_PDF_IMAGES {
        notes.push(format!("the PDF holds {} images: only the {MAX_PDF_IMAGES} largest were sent to the model", found.len()));
        found.truncate(MAX_PDF_IMAGES);
    }
    Ok((found.into_iter().map(|(_, d)| d).collect(), notes))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Boundary {
    /// `north`, `south`, `east` or `west`.
    pub direction: String,
    pub length_m: Option<f64>,
    /// The neighbour as written (a street, a plot number, an owner's plot), verbatim.
    pub neighbour: Option<String>,
    /// What the user must correct before this side can be used. Empty when it can.
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeedReading {
    pub deed_number: Option<String>,
    /// The date exactly as written (Hijri or Gregorian); it is not converted.
    pub deed_date_text: Option<String>,
    pub plot_number: Option<String>,
    pub plan_number: Option<String>,
    pub district: Option<String>,
    pub city: Option<String>,
    pub area_m2: Option<f64>,
    pub boundaries: Vec<Boundary>,
    /// `ok`, `conflict`, `unchecked`, `not_rectangular`, `incomplete` or `no_stated_area`.
    pub area_check: String,
    /// Width (the north/south side length, along x) and length (the east/west side length, along y) when the sides close a rectangle.
    pub rect_width_m: Option<f64>,
    pub rect_length_m: Option<f64>,
    /// The area the four sides give, when they close a rectangle.
    pub rect_area_m2: Option<f64>,
    pub notes: Vec<String>,
    pub source: String,
    /// Always `unconfirmed` until `accept` is called.
    pub status: String,
    pub needs_review: bool,
}

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "deed_number": { "type": ["string", "null"] },
            "deed_date_text": { "type": ["string", "null"] },
            "plot_number": { "type": ["string", "null"] },
            "plan_number": { "type": ["string", "null"] },
            "district": { "type": ["string", "null"] },
            "city": { "type": ["string", "null"] },
            "area_m2": { "type": ["number", "null"] },
            "boundaries": { "type": "array", "items": { "type": "object", "properties": {
                "direction": { "type": "string", "enum": DIRECTIONS },
                "length_m": { "type": ["number", "null"] },
                "neighbour": { "type": ["string", "null"] }
            }, "required": ["direction", "length_m", "neighbour"] } },
            "notes": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["boundaries"]
    })
}

pub fn system_prompt() -> String {
    "You read a Saudi land title deed or a plot survey sketch (a scan or a photo, Arabic or English) and return ONLY a JSON object \
     that transcribes what is written. Fields: deed_number, deed_date_text (exactly as written, do not convert the calendar), \
     plot_number, plan_number, district, city, area_m2 (the stated area in square metres, a number), and boundaries: one entry for each of \
     north, south, east, west that is written, with length_m (the stated length in metres, a number) and neighbour (the text written \
     for that side, verbatim). Use null for anything that is not written or not legible. Never compute, convert or estimate a value. \
     Do not return the owner's name or any identity number. Use notes for anything unclear. Output JSON only."
        .to_string()
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64().filter(|n| n.is_finite())
}

fn text(v: Option<&Value>) -> Option<String> {
    let t = v?.as_str()?.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() { None } else { Some(t.chars().take(MAX_TEXT_CHARS).collect()) }
}

fn direction_of(s: &str) -> Option<&'static str> {
    let t = s.trim().to_lowercase();
    let t = t.trim_start_matches("ال");
    match t {
        "north" | "n" | "شمال" => Some("north"),
        "south" | "s" | "جنوب" => Some("south"),
        "east" | "e" | "شرق" => Some("east"),
        "west" | "w" | "غرب" => Some("west"),
        _ => None,
    }
}

/// Reads the model's JSON. `tolerance_pct` (no default) is the largest accepted difference, in percent, used by the area and
/// rectangle checks; without it they are `unchecked`.
pub fn from_model_json(v: &Value, model: &str, tolerance_pct: Option<f64>) -> Result<DeedReading, String> {
    let o = v.as_object().ok_or("the model's answer is not a JSON object")?;
    let items = o.get("boundaries").and_then(Value::as_array).ok_or("the model's answer has no 'boundaries' list")?;
    if let Some(t) = tolerance_pct
        && !(t.is_finite() && t >= 0.0)
    {
        return Err("the tolerance must be zero or more".into());
    }
    let mut notes: Vec<String> = o.get("notes").and_then(Value::as_array).map(|a| a.iter().filter_map(|n| n.as_str()).map(|s| s.chars().take(MAX_TEXT_CHARS).collect()).collect()).unwrap_or_default();
    let area_m2 = match o.get("area_m2") {
        None | Some(Value::Null) => None,
        Some(a) => match num(a).filter(|n| *n > 0.0) {
            Some(n) => Some(n),
            None => {
                notes.push("the stated area was dropped: it is not a number above zero".into());
                None
            }
        },
    };
    let mut boundaries: Vec<Boundary> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        let Some(dir) = it.get("direction").and_then(Value::as_str).and_then(direction_of) else {
            notes.push(format!("boundary {}: dropped, its direction is not north, south, east or west", i + 1));
            continue;
        };
        let mut issues = Vec::new();
        let length_m = match it.get("length_m") {
            None | Some(Value::Null) => {
                issues.push("no length is written for this side".to_string());
                None
            }
            Some(l) => match num(l).filter(|n| *n > 0.0) {
                Some(n) => Some(n),
                None => {
                    issues.push("the length is not a number above zero".to_string());
                    None
                }
            },
        };
        boundaries.push(Boundary { direction: dir.to_string(), length_m, neighbour: text(it.get("neighbour")), issues });
    }
    for d in DIRECTIONS {
        if boundaries.iter().filter(|b| b.direction == d).count() > 1 {
            for b in boundaries.iter_mut().filter(|b| b.direction == d) {
                b.issues.push(format!("the {d} side appears more than once: keep one"));
            }
        }
    }
    let side = |d: &str| boundaries.iter().find(|b| b.direction == d && b.issues.is_empty()).and_then(|b| b.length_m);
    let (mut rect_width_m, mut rect_length_m, mut rect_area_m2) = (None, None, None);
    let area_check = match (side("north"), side("south"), side("east"), side("west")) {
        _ if area_m2.is_none() => "no_stated_area",
        (Some(n), Some(s), Some(e), Some(w)) => match tolerance_pct {
            None => "unchecked",
            Some(t) => {
                let differ = |a: f64, b: f64| (a - b).abs() / a.max(b) * 100.0 > t;
                if differ(n, s) || differ(e, w) {
                    "not_rectangular"
                } else {
                    let (wd, ln) = ((n + s) / 2.0, (e + w) / 2.0);
                    rect_width_m = Some(wd);
                    rect_length_m = Some(ln);
                    rect_area_m2 = Some(wd * ln);
                    let stated = area_m2.unwrap_or(0.0);
                    if (wd * ln - stated).abs() / stated * 100.0 > t { "conflict" } else { "ok" }
                }
            }
        },
        _ => "incomplete",
    }
    .to_string();
    match area_check.as_str() {
        "unchecked" => notes.push("no tolerance was given: the sides were not compared with the stated area".into()),
        "not_rectangular" => notes.push("opposite sides differ by more than the tolerance: the plot is not a rectangle, so its width, length and area cannot be taken from four side lengths".into()),
        "conflict" => notes.push("the sides give a different area from the stated one: read the deed again and correct the value that is wrong".into()),
        "incomplete" => notes.push("not all four sides have a valid length: the sides were not compared with the stated area".into()),
        _ => {}
    }
    Ok(DeedReading {
        deed_number: text(o.get("deed_number")),
        deed_date_text: text(o.get("deed_date_text")),
        plot_number: text(o.get("plot_number")),
        plan_number: text(o.get("plan_number")),
        district: text(o.get("district")),
        city: text(o.get("city")),
        area_m2,
        boundaries,
        area_check,
        rect_width_m,
        rect_length_m,
        rect_area_m2,
        notes,
        source: format!("ollama:{model}"),
        status: "unconfirmed".into(),
        needs_review: true,
    })
}

/// What the user typed to replace a value of the reading.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Edit {
    pub area_m2: Option<f64>,
    pub width_m: Option<f64>,
    pub length_m: Option<f64>,
    pub city: Option<String>,
}

/// Plot values for the brief, from the facts the user confirmed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Accepted {
    pub plot_area_m2: Option<f64>,
    pub plot_width_m: Option<f64>,
    pub plot_length_m: Option<f64>,
    pub city: Option<String>,
    pub notes: Vec<String>,
}

fn positive(v: f64, what: &str) -> Result<f64, String> {
    if v.is_finite() && v > 0.0 { Ok(v) } else { Err(format!("{what} must be a number above zero")) }
}

/// Confirms `keys` (`area`, `dimensions`, `city`). A fact the reading flags (a conflict, a side with an issue, a plot that is not a
/// rectangle) is refused unless the user supplies the value in `edit`.
pub fn accept(r: &DeedReading, keys: &[String], edit: &Edit) -> Result<Accepted, String> {
    if keys.is_empty() {
        return Err("name the facts you confirm: area, dimensions, city".into());
    }
    let mut out = Accepted::default();
    for k in keys {
        match k.as_str() {
            "area" => {
                if let Some(a) = edit.area_m2 {
                    out.plot_area_m2 = Some(positive(a, "the area you entered")?);
                    out.notes.push("plot area typed by the user".into());
                } else {
                    let a = r.area_m2.ok_or("the deed reading has no area: type it")?;
                    if r.area_check == "conflict" {
                        return Err("the area conflicts with the sides: type the correct area".into());
                    }
                    out.plot_area_m2 = Some(a);
                    out.notes.push(format!("plot area {a} m² confirmed from the deed (check: {})", r.area_check));
                }
            }
            "dimensions" => {
                match (edit.width_m, edit.length_m) {
                    (Some(w), Some(l)) => {
                        out.plot_width_m = Some(positive(w, "the width you entered")?);
                        out.plot_length_m = Some(positive(l, "the length you entered")?);
                        out.notes.push("plot width and length typed by the user".into());
                    }
                    (None, None) => {
                        let (Some(w), Some(l)) = (r.rect_width_m, r.rect_length_m) else {
                            return Err(format!("the sides do not give a width and a length (check: {}): type both", r.area_check));
                        };
                        if r.area_check != "ok" {
                            return Err(format!("the sides are not confirmed by the area (check: {}): type the width and the length", r.area_check));
                        }
                        out.plot_width_m = Some(w);
                        out.plot_length_m = Some(l);
                        out.notes.push(format!("plot width {w} m (north/south sides) and length {l} m (east/west sides) from the deed, area check ok"));
                    }
                    _ => return Err("type both the width and the length, or neither".into()),
                };
            }
            "city" => {
                let c = match &edit.city {
                    Some(c) => c.trim().to_string(),
                    None => r.city.clone().ok_or("the deed reading has no city: type it")?,
                };
                if c.is_empty() {
                    return Err("the city is empty".into());
                }
                out.city = Some(c);
            }
            other => return Err(format!("'{other}' is not a fact of the reading: use area, dimensions or city")),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_jpeg(w: u16, h: u16) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08];
        v.extend(h.to_be_bytes());
        v.extend(w.to_be_bytes());
        v.extend([0x01, 0x01, 0x11, 0x00, 0x00, 0x00, 0xFF, 0xD9]);
        v
    }

    fn pdf_with(objs: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut p = b"%PDF-1.4\n".to_vec();
        for (i, (dict, data)) in objs.iter().enumerate() {
            p.extend(format!("{} 0 obj\n<< /Type /XObject /Subtype /Image {dict} /Length {} >>\nstream\n", i + 1, data.len()).bytes());
            p.extend(data);
            p.extend(b"\nendstream\nendobj\n");
        }
        p.extend(b"%%EOF");
        p
    }

    #[test]
    fn jpeg_pages_are_pulled_out_of_a_pdf_largest_first() {
        let pdf = pdf_with(&[("/Filter /DCTDecode", fake_jpeg(100, 100)), ("/Filter /DCTDecode", fake_jpeg(2000, 3000)), ("/Filter /FlateDecode", vec![1, 2, 3])]);
        let (imgs, notes) = images_from_pdf(&pdf).unwrap();
        assert_eq!(imgs.len(), 2);
        assert_eq!(crate::sketchread::image_size(&imgs[0]).unwrap(), (2000, 3000));
        assert_eq!(imgs[0], fake_jpeg(2000, 3000));
        assert!(notes.iter().any(|n| n.contains("1 image(s)")));
    }

    #[test]
    fn a_pdf_with_no_jpeg_or_an_encrypted_one_is_refused_with_the_reason() {
        let e = images_from_pdf(&pdf_with(&[("/Filter /FlateDecode", vec![1, 2, 3])])).unwrap_err();
        assert!(e.contains("no embedded JPEG"));
        let mut enc = pdf_with(&[("/Filter /DCTDecode", fake_jpeg(10, 10))]);
        enc.extend(b"/Encrypt 5 0 R");
        assert!(images_from_pdf(&enc).unwrap_err().contains("encrypted"));
        assert!(images_from_pdf(b"hello").unwrap_err().contains("not a PDF"));
    }

    #[test]
    fn at_most_four_images_are_kept() {
        let objs: Vec<(&str, Vec<u8>)> = (1..=6).map(|i| ("/Filter /DCTDecode", fake_jpeg(100 * i, 100))).collect();
        let (imgs, notes) = images_from_pdf(&pdf_with(&objs)).unwrap();
        assert_eq!(imgs.len(), MAX_PDF_IMAGES);
        assert_eq!(crate::sketchread::image_size(&imgs[0]).unwrap().0, 600);
        assert!(notes.iter().any(|n| n.contains("6 images")));
    }

    fn deed_json() -> Value {
        json!({
            "deed_number": " 310123456789 ", "deed_date_text": "1445/03/12 هـ", "plot_number": "512", "plan_number": "2/1500",
            "district": "الفيحاء", "city": "الجبيل", "area_m2": 600,
            "boundaries": [
                { "direction": "north", "length_m": 20, "neighbour": "شارع عرض 20م" },
                { "direction": "south", "length_m": 20.2, "neighbour": "قطعة 513" },
                { "direction": "east", "length_m": 30, "neighbour": "قطعة 511" },
                { "direction": "west", "length_m": 30, "neighbour": "قطعة 514" }
            ]
        })
    }

    #[test]
    fn a_deed_is_read_unconfirmed_and_the_sides_are_checked_against_the_area_under_the_users_tolerance() {
        let r = from_model_json(&deed_json(), "m", Some(2.0)).unwrap();
        assert_eq!(r.deed_number.as_deref(), Some("310123456789"));
        assert_eq!((r.status.as_str(), r.needs_review, r.area_check.as_str()), ("unconfirmed", true, "ok"));
        assert_eq!(r.rect_width_m, Some(20.1));
        assert_eq!(r.rect_length_m, Some(30.0));
        let none = from_model_json(&deed_json(), "m", None).unwrap();
        assert_eq!((none.area_check.as_str(), none.rect_width_m), ("unchecked", None));
        let mut v = deed_json();
        v["area_m2"] = json!(700);
        assert_eq!(from_model_json(&v, "m", Some(2.0)).unwrap().area_check, "conflict");
        v["boundaries"][1]["length_m"] = json!(25);
        assert_eq!(from_model_json(&v, "m", Some(2.0)).unwrap().area_check, "not_rectangular");
    }

    #[test]
    fn bad_values_are_dropped_or_flagged_never_filled() {
        let mut v = deed_json();
        v["area_m2"] = json!(-5);
        v["boundaries"][0]["length_m"] = json!(0);
        v["boundaries"][1]["direction"] = json!("up");
        v["boundaries"].as_array_mut().unwrap().push(json!({ "direction": "الشرق", "length_m": 30, "neighbour": null }));
        let r = from_model_json(&v, "m", Some(2.0)).unwrap();
        assert_eq!(r.area_m2, None);
        assert_eq!(r.area_check, "no_stated_area");
        assert!(r.notes.iter().any(|n| n.contains("stated area was dropped")));
        assert!(r.notes.iter().any(|n| n.contains("boundary 2: dropped")));
        assert_eq!(r.boundaries.len(), 4);
        assert!(r.boundaries[0].issues.iter().any(|i| i.contains("above zero")));
        assert!(r.boundaries.iter().filter(|b| b.direction == "east").all(|b| b.issues.iter().any(|i| i.contains("more than once"))));
        let mut w = deed_json();
        w["boundaries"].as_array_mut().unwrap().pop();
        assert_eq!(from_model_json(&w, "m", Some(2.0)).unwrap().area_check, "incomplete");
        assert!(from_model_json(&json!({}), "m", None).unwrap_err().contains("no 'boundaries'"));
        assert!(from_model_json(&deed_json(), "m", Some(-1.0)).unwrap_err().contains("tolerance"));
    }

    #[test]
    fn accept_takes_only_what_the_user_names_and_refuses_flagged_facts() {
        let r = from_model_json(&deed_json(), "m", Some(2.0)).unwrap();
        let a = accept(&r, &["area".into(), "dimensions".into(), "city".into()], &Edit::default()).unwrap();
        assert_eq!((a.plot_area_m2, a.plot_width_m, a.plot_length_m, a.city.as_deref()), (Some(600.0), Some(20.1), Some(30.0), Some("الجبيل")));
        let only = accept(&r, &["city".into()], &Edit::default()).unwrap();
        assert_eq!((only.plot_area_m2, only.plot_width_m), (None, None));
        assert!(accept(&r, &[], &Edit::default()).unwrap_err().contains("name the facts"));
        assert!(accept(&r, &["owner".into()], &Edit::default()).unwrap_err().contains("not a fact"));

        let mut v = deed_json();
        v["area_m2"] = json!(700);
        let c = from_model_json(&v, "m", Some(2.0)).unwrap();
        assert!(accept(&c, &["area".into()], &Edit::default()).unwrap_err().contains("conflicts"));
        assert!(accept(&c, &["dimensions".into()], &Edit::default()).unwrap_err().contains("not confirmed by the area"));
        let typed = accept(&c, &["area".into(), "dimensions".into()], &Edit { area_m2: Some(603.0), width_m: Some(20.1), length_m: Some(30.0), city: None }).unwrap();
        assert_eq!((typed.plot_area_m2, typed.plot_width_m), (Some(603.0), Some(20.1)));
        assert!(accept(&c, &["dimensions".into()], &Edit { width_m: Some(20.0), ..Edit::default() }).unwrap_err().contains("both"));
        assert!(accept(&c, &["area".into()], &Edit { area_m2: Some(0.0), ..Edit::default() }).unwrap_err().contains("above zero"));
        let unchecked = from_model_json(&deed_json(), "m", None).unwrap();
        assert!(accept(&unchecked, &["dimensions".into()], &Edit::default()).unwrap_err().contains("type both"));
    }
}
