//! A photographed or scanned hand sketch read into rooms, for the user to review.
//!
//! - **The model only finds boxes.** A vision model on this computer (see `src/ollama.rs`) returns, for each room it sees, a
//!   box in IMAGE PIXELS, a label, a function and any dimension written on the drawing. It never returns millimetres.
//! - **The scale is the user's.** Pixels become millimetres only through a reference the user gives: two points on the image
//!   and the real distance between them (`Scale::from_reference`). Without it nothing is converted; no scale is guessed from
//!   the picture, and the model's own idea of size is not used.
//! - **The origin is the user's.** The south-west corner of the buildable area, as a pixel of the image, is given too: the page
//!   frame of the engine has y up while an image has y down, so a room box is flipped about that point.
//! - **Checked.** A box outside the image, with no area, smaller than 0.5 m on a side (the smallest room the generator
//!   accepts), or with a repeated key is dropped and named. Two boxes that overlap are both kept and both flagged, never
//!   merged or trimmed. A dimension written on the drawing is compared with the scaled box: over the user's tolerance it is
//!   flagged `conflict`; with no tolerance given it is `unchecked`, and a box with no written dimension is `no_written_dimension`.
//! - **Reviewed.** Every room comes back `unconfirmed`. `accept` turns only the keys the user confirms into brief rooms and
//!   a `placement` for `generate`; a room with an issue cannot be accepted until the user corrects its box.

use crate::briefai::{DraftRoom, FUNCS};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The smallest side of a room the generator accepts (`levels::placed_building`).
pub const MIN_SIDE_MM: f64 = 500.0;

/// Pixel size of an image, read from its header: PNG, JPEG or BMP.
pub fn image_size(bytes: &[u8]) -> Result<(u32, u32), String> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        if bytes.len() < 24 || &bytes[12..16] != b"IHDR" {
            return Err("the PNG file has no header".into());
        }
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return nonzero(w, h);
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        let mut i = 2;
        while i + 9 < bytes.len() {
            if bytes[i] != 0xFF {
                return Err("the JPEG file is damaged".into());
            }
            let marker = bytes[i + 1];
            if marker == 0xFF {
                i += 1;
                continue;
            }
            if (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker) {
                let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
                return nonzero(w, h);
            }
            let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
            i += 2 + len;
        }
        return Err("the JPEG file has no size header".into());
    }
    if bytes.starts_with(b"BM") && bytes.len() >= 26 {
        let w = i32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]);
        let h = i32::from_le_bytes([bytes[22], bytes[23], bytes[24], bytes[25]]);
        return nonzero(w.unsigned_abs(), h.unsigned_abs());
    }
    if bytes.starts_with(b"%PDF") {
        return Err("a PDF cannot be sent to the model: render the page to an image (PNG or JPEG) first".into());
    }
    Err("the image type is not recognised: use PNG, JPEG or BMP".into())
}

fn nonzero(w: u32, h: u32) -> Result<(u32, u32), String> {
    if w == 0 || h == 0 { Err("the image has no size".into()) } else { Ok((w, h)) }
}

/// Millimetres per image pixel, from a distance the user knows.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Scale {
    pub mm_per_px: f64,
}

impl Scale {
    pub fn from_reference(p1: (f64, f64), p2: (f64, f64), real_mm: f64) -> Result<Scale, String> {
        if ![p1.0, p1.1, p2.0, p2.1, real_mm].iter().all(|v| v.is_finite()) {
            return Err("the scale reference has a value that is not a number".into());
        }
        if real_mm <= 0.0 {
            return Err("the real distance of the scale reference must be above zero".into());
        }
        let px = ((p1.0 - p2.0).powi(2) + (p1.1 - p2.1).powi(2)).sqrt();
        if px < 1.0 {
            return Err("the two points of the scale reference are the same point".into());
        }
        Ok(Scale { mm_per_px: real_mm / px })
    }
}

/// What the user fixes before the model's boxes mean anything.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    pub width_px: u32,
    pub height_px: u32,
    pub scale: Scale,
    /// The south-west corner of the buildable area, as a pixel of the image.
    pub origin_px: (f64, f64),
    /// Largest accepted difference, in percent, between a dimension written on the drawing and the scaled box. Not defaulted.
    pub tolerance_pct: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadRoom {
    pub key: String,
    pub name: Option<String>,
    pub func: String,
    /// The box in the engine frame: millimetres, origin at the south-west corner of the buildable area, y up.
    pub x_mm: i64,
    pub y_mm: i64,
    pub w_mm: i64,
    pub h_mm: i64,
    pub area_m2: f64,
    /// The box as the model returned it: `[x0, y0, x1, y1]` in image pixels, y down.
    pub bbox_px: [f64; 4],
    pub written_width_mm: Option<f64>,
    pub written_length_mm: Option<f64>,
    /// `ok`, `conflict`, `unchecked` (no tolerance given) or `no_written_dimension`.
    pub dimension_check: String,
    /// Keys of the rooms this box overlaps.
    pub overlaps: Vec<String>,
    /// Always `unconfirmed` until `accept` is called with this key.
    pub status: String,
    /// What the user must correct before this room can be accepted. Empty when it can.
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    pub rooms: Vec<ReadRoom>,
    /// What was read, dropped, replaced or not checked.
    pub notes: Vec<String>,
    pub source: String,
    pub needs_review: bool,
}

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "rooms": { "type": "array", "items": { "type": "object", "properties": {
                "label": { "type": ["string", "null"] },
                "func": { "type": ["string", "null"], "enum": [FUNCS[0], FUNCS[1], FUNCS[2], FUNCS[3], FUNCS[4], FUNCS[5], FUNCS[6], null] },
                "bbox_px": { "type": "array", "items": { "type": "number" }, "minItems": 4, "maxItems": 4 },
                "written_width_mm": { "type": ["number", "null"] },
                "written_length_mm": { "type": ["number", "null"] }
            }, "required": ["label", "func", "bbox_px"] } },
            "notes": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["rooms"]
    })
}

pub fn system_prompt(width_px: u32, height_px: u32) -> String {
    format!(
        "You read a hand-drawn floor plan (a photo or a scan, {width_px} x {height_px} pixels) and return ONLY a JSON object. \
         For each ROOM drawn as a closed rectangle, give bbox_px = [x0, y0, x1, y1] in PIXELS of this image (origin at the top-left, \
         y down, x0 < x1, y0 < y1), the label written inside it (null if none, in the language written), func in {FUNCS:?} (null if \
         you cannot tell), and written_width_mm / written_length_mm only if a dimension is written on the drawing, converted to \
         millimetres (null otherwise). Do not return the plot outline, walls, doors or furniture as rooms. Never estimate real sizes \
         from the picture. Use notes for anything unclear. Output JSON only."
    )
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64().filter(|n| n.is_finite())
}

/// Reads the model's JSON, room by room. Invalid rooms are dropped and named in the notes.
pub fn from_model_json(v: &Value, model: &str, frame: &Frame) -> Result<Reading, String> {
    let o = v.as_object().ok_or("the model's answer is not a JSON object")?;
    let items = o.get("rooms").and_then(Value::as_array).ok_or("the model's answer has no 'rooms' list")?;
    if !(frame.scale.mm_per_px.is_finite() && frame.scale.mm_per_px > 0.0) {
        return Err("the scale is not valid".into());
    }
    if let Some(t) = frame.tolerance_pct
        && !(t.is_finite() && t >= 0.0)
    {
        return Err("the tolerance must be zero or more".into());
    }
    if !(frame.origin_px.0.is_finite() && frame.origin_px.1.is_finite()) {
        return Err("the origin has a value that is not a number".into());
    }
    let (w_img, h_img) = (frame.width_px as f64, frame.height_px as f64);
    let mut notes = Vec::new();
    let mut rooms: Vec<ReadRoom> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        let n = i + 1;
        let Some(b) = it.get("bbox_px").and_then(Value::as_array).filter(|b| b.len() == 4) else {
            notes.push(format!("room {n}: dropped, its box is not four numbers"));
            continue;
        };
        let Some(bx) = b.iter().map(num).collect::<Option<Vec<f64>>>() else {
            notes.push(format!("room {n}: dropped, its box has a value that is not a number"));
            continue;
        };
        let (x0, y0, x1, y1) = (bx[0], bx[1], bx[2], bx[3]);
        if !(x0 < x1 && y0 < y1) {
            notes.push(format!("room {n}: dropped, its box has no area (x0 < x1 and y0 < y1 are required)"));
            continue;
        }
        if x0 < 0.0 || y0 < 0.0 || x1 > w_img || y1 > h_img {
            notes.push(format!("room {n}: dropped, its box lies outside the {} x {} px image", frame.width_px, frame.height_px));
            continue;
        }
        let s = frame.scale.mm_per_px;
        let (w, h) = ((x1 - x0) * s, (y1 - y0) * s);
        if w < MIN_SIDE_MM || h < MIN_SIDE_MM {
            notes.push(format!("room {n}: dropped, it measures {:.2} x {:.2} m and a room side is at least 0.5 m", w / 1000.0, h / 1000.0));
            continue;
        }
        let round10 = |v: f64| ((v / 10.0).round() as i64) * 10;
        let x_mm = round10((x0 - frame.origin_px.0) * s);
        let y_mm = round10((frame.origin_px.1 - y1) * s);
        let (w_mm, h_mm) = (round10(w), round10(h));
        let label = it.get("label").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
        let func = match it.get("func").and_then(Value::as_str) {
            Some(f) if FUNCS.contains(&f) => f.to_string(),
            Some(f) => {
                notes.push(format!("room {n}: the function '{f}' is not one the planner knows, set to 'other'"));
                "other".to_string()
            }
            None => {
                notes.push(format!("room {n}: no function read, set to 'other'"));
                "other".to_string()
            }
        };
        let written = |k: &str| it.get(k).and_then(num).filter(|v| *v > 0.0);
        let (ww, wl) = (written("written_width_mm"), written("written_length_mm"));
        let dimension_check = match (ww, wl, frame.tolerance_pct) {
            (None, None, _) => "no_written_dimension",
            (_, _, None) => "unchecked",
            (a, b, Some(t)) => {
                // The written pair may be given in either order: compare both orders and keep the closer.
                let off = |x: f64, y: f64| ((x - w_mm as f64).abs() / x).max((y - h_mm as f64).abs() / y) * 100.0;
                let single = |x: f64| (((x - w_mm as f64).abs() / x).min((x - h_mm as f64).abs() / x)) * 100.0;
                let worst = match (a, b) {
                    (Some(a), Some(b)) => off(a, b).min(off(b, a)),
                    (Some(x), None) | (None, Some(x)) => single(x),
                    (None, None) => 0.0,
                };
                if worst <= t { "ok" } else { "conflict" }
            }
        };
        let mut issues = Vec::new();
        if dimension_check == "conflict" {
            issues.push(format!("the dimension written on the drawing differs from the scaled box by more than {}%: check the scale or the box", frame.tolerance_pct.unwrap_or(0.0)));
        }
        rooms.push(ReadRoom {
            key: format!("s{n}"),
            name: label,
            func,
            x_mm,
            y_mm,
            w_mm,
            h_mm,
            area_m2: (w_mm as f64 * h_mm as f64) / 1.0e6,
            bbox_px: [x0, y0, x1, y1],
            written_width_mm: ww,
            written_length_mm: wl,
            dimension_check: dimension_check.to_string(),
            overlaps: Vec::new(),
            status: "unconfirmed".to_string(),
            issues,
        });
    }
    if rooms.is_empty() {
        return Err(format!("no room could be read from the sketch{}", if notes.is_empty() { String::new() } else { format!(": {}", notes.join("; ")) }));
    }
    flag_overlaps(&mut rooms);
    if let Some(a) = o.get("notes").and_then(Value::as_array) {
        for t in a.iter().filter_map(Value::as_str).map(str::trim).filter(|t| !t.is_empty()) {
            notes.push(format!("model: {t}"));
        }
    }
    if frame.tolerance_pct.is_none() && rooms.iter().any(|r| r.dimension_check == "unchecked") {
        notes.push("no tolerance was given, so the dimensions written on the drawing were not compared with the scaled boxes".to_string());
    }
    notes.push("these rooms were found by a model and are unconfirmed: review every box, then accept the ones that are right".to_string());
    Ok(Reading { rooms, notes, source: format!("ollama:{model}"), needs_review: true })
}

fn flag_overlaps(rooms: &mut [ReadRoom]) {
    let n = rooms.len();
    for i in 0..n {
        for j in 0..n {
            if i == j {
                continue;
            }
            let (a, b) = (&rooms[i], &rooms[j]);
            let hit = a.x_mm < b.x_mm + b.w_mm && b.x_mm < a.x_mm + a.w_mm && a.y_mm < b.y_mm + b.h_mm && b.y_mm < a.y_mm + a.h_mm;
            if hit {
                let other = b.key.clone();
                rooms[i].issues.push(format!("overlaps {other}"));
                rooms[i].overlaps.push(other);
            }
        }
    }
}

/// What the user confirmed: brief rooms and a `placement` for `generate`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Accepted {
    pub rooms: Vec<DraftRoom>,
    /// `{ key: { x, y, w, h } }` in millimetres; the keys are the reading's, matching `rooms` by position.
    pub placement: Value,
    pub keys: Vec<String>,
    pub notes: Vec<String>,
}

/// One correction the user made to a room before accepting it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Edit {
    pub name: Option<String>,
    pub func: Option<String>,
    pub x_mm: Option<i64>,
    pub y_mm: Option<i64>,
    pub w_mm: Option<i64>,
    pub h_mm: Option<i64>,
}

/// Turns the confirmed keys of `reading` into brief rooms on `floor`. `edits` maps a key to the user's corrections; an edited room
/// is re-checked (size, overlap with every other room of the reading, outside the plot origin). A key that is unknown, repeated or
/// has an issue is refused by name.
pub fn accept(reading: &Reading, keys: &[String], floor: u32, edits: &std::collections::BTreeMap<String, Edit>) -> Result<Accepted, String> {
    if floor == 0 {
        return Err("floor must be 1 or more (1 is the ground floor)".into());
    }
    if keys.is_empty() {
        return Err("confirm at least one room".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut chosen: Vec<ReadRoom> = Vec::new();
    for k in keys {
        if !seen.insert(k.clone()) {
            return Err(format!("room '{k}' is confirmed twice"));
        }
        let r = reading.rooms.iter().find(|r| &r.key == k).ok_or_else(|| format!("room '{k}' is not in the reading"))?;
        let mut r = r.clone();
        if let Some(e) = edits.get(k) {
            if let Some(n) = &e.name {
                r.name = Some(n.trim().to_string()).filter(|n| !n.is_empty());
            }
            if let Some(f) = &e.func {
                if !FUNCS.contains(&f.as_str()) {
                    return Err(format!("room '{k}': the function '{f}' is not one the planner knows"));
                }
                r.func = f.clone();
            }
            let edited = e.x_mm.is_some() || e.y_mm.is_some() || e.w_mm.is_some() || e.h_mm.is_some();
            r.x_mm = e.x_mm.unwrap_or(r.x_mm);
            r.y_mm = e.y_mm.unwrap_or(r.y_mm);
            r.w_mm = e.w_mm.unwrap_or(r.w_mm);
            r.h_mm = e.h_mm.unwrap_or(r.h_mm);
            if edited {
                if r.w_mm < MIN_SIDE_MM as i64 || r.h_mm < MIN_SIDE_MM as i64 {
                    return Err(format!("room '{k}': a side is under 0.5 m after the correction"));
                }
                if r.x_mm < 0 || r.y_mm < 0 {
                    return Err(format!("room '{k}': the corrected box is west or south of the buildable area's corner"));
                }
                r.area_m2 = (r.w_mm as f64 * r.h_mm as f64) / 1.0e6;
                // An edit replaces the box, so the issues about the old box no longer apply; the other rooms are rechecked below.
                r.issues.clear();
                r.overlaps.clear();
            }
        }
        chosen.push(r);
    }
    // Overlaps among the confirmed rooms are checked on the final boxes, edited or not.
    for (i, a) in chosen.iter().enumerate() {
        for b in chosen.iter().skip(i + 1) {
            if a.x_mm < b.x_mm + b.w_mm && b.x_mm < a.x_mm + a.w_mm && a.y_mm < b.y_mm + b.h_mm && b.y_mm < a.y_mm + a.h_mm {
                return Err(format!("rooms '{}' and '{}' overlap: correct one of the boxes", a.key, b.key));
            }
        }
    }
    for r in &chosen {
        let edited = edits.get(&r.key).is_some_and(|e| e.x_mm.is_some() || e.y_mm.is_some() || e.w_mm.is_some() || e.h_mm.is_some());
        let blocking: Vec<&String> = r.issues.iter().filter(|i| !i.starts_with("overlaps ")).collect();
        if !blocking.is_empty() && !edited {
            return Err(format!("room '{}' has an issue: {}. Correct its box before accepting it", r.key, blocking[0]));
        }
        // An overlap with a room that is not confirmed does not stop this one.
    }
    let mut rooms = Vec::new();
    let mut placement = serde_json::Map::new();
    for r in &chosen {
        rooms.push(DraftRoom { func: r.func.clone(), name: r.name.clone(), area_m2: Some(r.area_m2), area_suggested: false, floor, assumed: false });
        placement.insert(r.key.clone(), json!({ "x": r.x_mm, "y": r.y_mm, "w": r.w_mm, "h": r.h_mm }));
    }
    let notes = vec![format!("{} room(s) confirmed from the sketch on floor {floor}; their areas are measured from the boxes at the scale you set, so they are as accurate as that scale", chosen.len())];
    Ok(Accepted { rooms, placement: Value::Object(placement), keys: chosen.iter().map(|r| r.key.clone()).collect(), notes })
}
