//! Site progress photos tied to model elements (tracker item 91).
//!
//! A photo is a REFERENCE (a file path or name, never the image bytes) with the date it was taken, who took it, a caption, and the
//! elements it shows. Everything is entered by the user and nothing is defaulted:
//! - the date is a real `YYYY-MM-DD` day; a photo with no date, no file or no linked element is refused;
//! - an element is named as in the as-built check: a schedule name (`COL-3`, `FND-1`), an element id, or `room:<requirement key>`;
//!   a name that matches no element is refused by name (see [`validate_links`]), never silently dropped;
//! - `observedPct` (0 to 100) is the user's own reading of how complete the element looks in that photo. It is not computed from the
//!   image and it does not feed the schedule or the payment certificates.
//!
//! The report lists, per element, its photos and the latest one, and names every structural element (those with a schedule name)
//! that has no photo yet. Photos are evidence of what the user saw; they are not an engineer's inspection record.

use crate::asbuilt::resolve_ref;
use crate::execution::{esc, parse_date, round};
use crate::model::Model;
use crate::structx::schedule_names;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SitePhoto {
    pub id: String,
    /// Path or file name of the image; the project keeps the reference only.
    pub file: String,
    /// `YYYY-MM-DD`.
    pub taken: String,
    pub caption: Option<String>,
    pub author: Option<String>,
    /// The elements the photo shows (schedule name, element id or `room:<key>`).
    pub elements: Vec<String>,
    /// The user's reading of the completion of the shown work, 0 to 100.
    pub observed_pct: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SitePhotos {
    pub photos: Vec<SitePhoto>,
}

impl SitePhotos {
    /// Checks what can be checked without the model.
    pub fn check(&self) -> Result<(), String> {
        let mut ids = BTreeSet::new();
        for p in &self.photos {
            if p.id.trim().is_empty() {
                return Err("a site photo needs an id".into());
            }
            if !ids.insert(p.id.as_str()) {
                return Err(format!("the site photo id '{}' is used twice", p.id));
            }
            if p.file.trim().is_empty() {
                return Err(format!("site photo '{}' needs its file (path or name)", p.id));
            }
            parse_date(&format!("site photo '{}' date", p.id), &p.taken)?;
            if p.elements.is_empty() {
                return Err(format!("site photo '{}' is not linked to any element", p.id));
            }
            let mut seen = BTreeSet::new();
            for e in &p.elements {
                if e.trim().is_empty() {
                    return Err(format!("site photo '{}' has an empty element name", p.id));
                }
                if !seen.insert(e.trim()) {
                    return Err(format!("site photo '{}' links '{e}' twice", p.id));
                }
            }
            if p.observed_pct.is_some_and(|v| !v.is_finite() || !(0.0..=100.0).contains(&v)) {
                return Err(format!("site photo '{}': the observed completion must be between 0 and 100", p.id));
            }
        }
        Ok(())
    }
}

/// Refuses, by photo and name, every element reference that matches nothing in the model.
pub fn validate_links(model: &Model, sp: &SitePhotos) -> Result<(), String> {
    let names = schedule_names(model, None);
    for p in &sp.photos {
        for e in &p.elements {
            if resolve_ref(model, &names, e.trim()).is_none() {
                return Err(format!("site photo '{}': '{e}' matches no element in the model", p.id));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct PhotoRow {
    pub id: String,
    pub file: String,
    pub taken: String,
    pub caption: Option<String>,
    pub author: Option<String>,
    pub observed_pct: Option<f64>,
    /// Element ids the photo's references resolved to.
    pub element_ids: Vec<String>,
    /// References that no longer match an element (an element deleted after the photo was linked).
    pub unresolved: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ElementPhotos {
    pub element_id: String,
    pub name: Option<String>,
    pub kind: String,
    pub photo_ids: Vec<String>,
    pub latest_photo: String,
    pub latest_taken: String,
    /// The `observedPct` of the latest photo that has one.
    pub latest_observed_pct: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UnphotographedElement {
    pub element_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SitePhotoReport {
    pub photos: Vec<PhotoRow>,
    pub elements: Vec<ElementPhotos>,
    /// Schedule-named elements (columns, footings, beams, slabs, walls) with no photo.
    pub unphotographed: Vec<UnphotographedElement>,
    pub first_taken: Option<String>,
    pub last_taken: Option<String>,
    pub notes: Vec<String>,
}

pub fn report(model: &Model, sp: &SitePhotos) -> SitePhotoReport {
    let names = schedule_names(model, None);
    let mut rep = SitePhotoReport::default();
    // (taken days, input order, photo index) sorts the photos by date, then by entry order.
    let mut order: Vec<(i64, usize)> = Vec::new();
    for (i, p) in sp.photos.iter().enumerate() {
        match parse_date("site photo date", &p.taken) {
            Ok(d) => order.push((d.to_days(), i)),
            Err(e) => rep.notes.push(format!("photo '{}': {e}", p.id)),
        }
    }
    order.sort();
    let mut per: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (_, i) in &order {
        let p = &sp.photos[*i];
        let mut row = PhotoRow { id: p.id.clone(), file: p.file.clone(), taken: p.taken.clone(), caption: p.caption.clone(), author: p.author.clone(), observed_pct: p.observed_pct.map(|v| round(v, 2)), element_ids: Vec::new(), unresolved: Vec::new() };
        for e in &p.elements {
            match resolve_ref(model, &names, e.trim()) {
                Some(id) => {
                    let id = id.to_string();
                    if !row.element_ids.contains(&id) {
                        row.element_ids.push(id.clone());
                        per.entry(id).or_default().push(*i);
                    }
                }
                None => row.unresolved.push(e.clone()),
            }
        }
        if !row.unresolved.is_empty() {
            rep.notes.push(format!("photo '{}': {} no longer match an element", p.id, row.unresolved.join(", ")));
        }
        rep.photos.push(row);
    }
    rep.first_taken = order.first().map(|(_, i)| sp.photos[*i].taken.clone());
    rep.last_taken = order.last().map(|(_, i)| sp.photos[*i].taken.clone());
    for (id, idx) in &per {
        // `idx` is already in date order, so the last entry is the latest photo.
        let Some(&last) = idx.last() else { continue };
        let latest = &sp.photos[last];
        let kind = id.parse().ok().and_then(|u| model.get(&u)).map(|e| e.kind.clone()).unwrap_or_default();
        rep.elements.push(ElementPhotos {
            element_id: id.clone(),
            name: names.get(id).cloned(),
            kind,
            photo_ids: idx.iter().map(|&i| sp.photos[i].id.clone()).collect(),
            latest_photo: latest.id.clone(),
            latest_taken: latest.taken.clone(),
            latest_observed_pct: idx.iter().rev().find_map(|&i| sp.photos[i].observed_pct).map(|v| round(v, 2)),
        });
    }
    rep.elements.sort_by(|a, b| a.name.cmp(&b.name).then(a.element_id.cmp(&b.element_id)));
    for (id, name) in &names {
        if !per.contains_key(id) {
            rep.unphotographed.push(UnphotographedElement { element_id: id.clone(), name: name.clone() });
        }
    }
    rep.unphotographed.sort_by(|a, b| a.name.cmp(&b.name));
    if sp.photos.is_empty() {
        rep.notes.push("no site photos have been entered".into());
    }
    rep
}

impl SitePhotoReport {
    /// One row per photo and element.
    pub fn csv(&self) -> String {
        let cell = |s: &str| if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
        let mut o = String::from("photo,taken,file,element_id,observed_pct,author,caption\n");
        for p in &self.photos {
            for e in &p.element_ids {
                let _ = writeln!(o, "{},{},{},{},{},{},{}", cell(&p.id), p.taken, cell(&p.file), e, p.observed_pct.map(|v| v.to_string()).unwrap_or_default(), cell(p.author.as_deref().unwrap_or("")), cell(p.caption.as_deref().unwrap_or("")));
            }
        }
        o
    }

    /// A printable page: the photos by date, with the elements each one shows, and the elements still without a photo.
    pub fn html(&self) -> String {
        let mut o = String::from("<!doctype html><meta charset=\"utf-8\"><title>Site photos</title><h1>Site photos</h1>");
        let _ = write!(o, "<p>{} photos", self.photos.len());
        if let (Some(a), Some(b)) = (&self.first_taken, &self.last_taken) {
            let _ = write!(o, ", {a} to {b}");
        }
        o.push_str("</p><table border=\"1\" cellpadding=\"4\"><tr><th>Date</th><th>File</th><th>Elements</th><th>Observed %</th><th>Caption</th></tr>");
        for p in &self.photos {
            let _ = write!(o, "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>", esc(&p.taken), esc(&p.file), p.element_ids.len(), p.observed_pct.map(|v| v.to_string()).unwrap_or_default(), esc(p.caption.as_deref().unwrap_or("")));
        }
        o.push_str("</table>");
        if !self.unphotographed.is_empty() {
            let _ = write!(o, "<h2>No photo yet ({})</h2><p>{}</p>", self.unphotographed.len(), esc(&self.unphotographed.iter().map(|u| u.name.as_str()).collect::<Vec<_>>().join(", ")));
        }
        o
    }
}
