//! A single delivery bundle: one ZIP with the deliverables sorted into numbered folders and an index of what is inside.
//!
//! The caller supplies the files (name, folder, bytes); this module only validates the paths, orders them, writes the ZIP
//! (`xlsx::zip_stored`, every file stored) and adds `00-index/manifest.json` listing every file with its size and CRC-32, so the
//! receiver can check the package arrived whole. Nothing is invented: no file is added except the index, and an empty bundle is
//! refused.
//!
//! Rejected by name: no files, an unknown folder, an empty file name, a name with a path part (`/`, `\`, `..`, a drive colon or
//! a control character), a name longer than 120 characters, an empty file, and the same path twice (compared without regard to
//! case, since Windows treats `A.dxf` and `a.dxf` as one file).

use crate::xlsx::{crc32, zip_stored};
use serde_json::json;
use std::collections::BTreeSet;

/// The folders of a bundle, in delivery order. The first is reserved for the index this module writes.
pub const FOLDERS: [&str; 5] = ["00-index", "01-models", "02-drawings", "03-quantities", "04-documents"];

/// The folders of the full project dossier, in delivery order. The first is reserved for the index.
pub const DOSSIER_FOLDERS: [&str; 9] = ["00_Index", "01_Survey_Geotech", "02_IFC_Drawings", "03_Calculations", "04_Tender_BOQ", "05_Permits", "06_Supervision_QA_QC", "07_Commercial_IPC", "08_CloseOut_AsBuilt"];

/// Longest file name accepted.
const MAX_NAME: usize = 120;

#[derive(Debug, Clone)]
pub struct Entry {
    /// One of `FOLDERS` except `00-index`.
    pub folder: String,
    /// A bare file name (no path).
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Bundle {
    /// The ZIP archive.
    pub zip: Vec<u8>,
    /// The archive paths in the order written (index last).
    pub paths: Vec<String>,
    /// The index file's text.
    pub manifest: String,
}

fn check_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("bundle: a file has an empty name".into());
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!("bundle: file name '{name}' is longer than {MAX_NAME} characters"));
    }
    if name == "." || name == ".." || name.contains("..") || name.chars().any(|c| matches!(c, '/' | '\\' | ':') || c.is_control()) {
        return Err(format!("bundle: file name '{name}' must be a bare name (no folder part, '..', drive colon or control character)"));
    }
    Ok(())
}

/// Build the bundle. `title` and `date` (UTC `YYYY-MM-DD`) go into the index as given.
pub fn build(title: &str, date: &str, entries: Vec<Entry>) -> Result<Bundle, String> {
    build_in(&FOLDERS, title, date, entries)
}

/// The full project dossier: the same checks and index as `build`, with the folders of `DOSSIER_FOLDERS`.
pub fn build_dossier(title: &str, date: &str, entries: Vec<Entry>) -> Result<Bundle, String> {
    build_in(&DOSSIER_FOLDERS, title, date, entries)
}

fn build_in(folders: &[&str], title: &str, date: &str, mut entries: Vec<Entry>) -> Result<Bundle, String> {
    if entries.is_empty() {
        return Err("bundle: nothing to deliver (choose at least one file)".into());
    }
    let mut seen = BTreeSet::new();
    for e in &entries {
        if !folders[1..].contains(&e.folder.as_str()) {
            return Err(format!("bundle: unknown folder '{}' (use one of {})", e.folder, folders[1..].join(", ")));
        }
        check_name(&e.name)?;
        if e.bytes.is_empty() {
            return Err(format!("bundle: '{}/{}' is empty", e.folder, e.name));
        }
        if !seen.insert(format!("{}/{}", e.folder, e.name).to_lowercase()) {
            return Err(format!("bundle: '{}/{}' appears twice", e.folder, e.name));
        }
    }
    entries.sort_by(|a, b| {
        let rank = |f: &str| folders.iter().position(|x| *x == f).unwrap_or(usize::MAX);
        (rank(&a.folder), a.name.to_lowercase()).cmp(&(rank(&b.folder), b.name.to_lowercase()))
    });
    let files: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| json!({ "path": format!("{}/{}", e.folder, e.name), "bytes": e.bytes.len(), "crc32": format!("{:08x}", crc32(&e.bytes)) }))
        .collect();
    let manifest = serde_json::to_string_pretty(&json!({ "title": title, "date": date, "folders": folders, "file_count": files.len(), "files": files }))
        .map_err(|e| format!("bundle: index could not be written: {e}"))?;
    let mut archive: Vec<(String, Vec<u8>)> = entries.into_iter().map(|e| (format!("{}/{}", e.folder, e.name), e.bytes)).collect();
    archive.insert(0, (format!("{}/manifest.json", folders[0]), manifest.clone().into_bytes()));
    let paths = archive.iter().map(|(p, _)| p.clone()).collect();
    Ok(Bundle { zip: zip_stored(&archive), paths, manifest })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(folder: &str, name: &str, data: &[u8]) -> Entry {
        Entry { folder: folder.into(), name: name.into(), bytes: data.to_vec() }
    }

    #[test]
    fn orders_folders_and_writes_a_readable_index() {
        let b = build("Villa", "2026-11-23", vec![entry("03-quantities", "boq.xlsx", b"x"), entry("01-models", "model.ifc", b"ifc!"), entry("02-drawings", "plan.dxf", b"dxf")]).unwrap();
        assert_eq!(b.paths, vec!["00-index/manifest.json", "01-models/model.ifc", "02-drawings/plan.dxf", "03-quantities/boq.xlsx"]);
        let m: serde_json::Value = serde_json::from_str(&b.manifest).unwrap();
        assert_eq!(m["file_count"], 3);
        assert_eq!(m["files"][0]["path"], "01-models/model.ifc");
        assert_eq!(m["files"][0]["bytes"], 4);
        assert_eq!(m["files"][0]["crc32"], format!("{:08x}", crc32(b"ifc!")));
        assert_eq!(&b.zip[..4], b"PK\x03\x04");
        assert!(b.zip.windows(22).any(|w| w.starts_with(b"PK\x05\x06")));
        assert!(b.zip.windows(26).any(|w| w == b"02-drawings/plan.dxf".as_slice() || w.starts_with(b"02-drawings/plan.dxf")));
    }

    #[test]
    fn refuses_bad_input_by_name() {
        let e = |r: Result<Bundle, String>| r.unwrap_err();
        assert!(e(build("t", "d", vec![])).contains("nothing to deliver"));
        assert!(e(build("t", "d", vec![entry("00-index", "a.txt", b"x")])).contains("unknown folder"));
        assert!(e(build("t", "d", vec![entry("misc", "a.txt", b"x")])).contains("unknown folder"));
        for bad in ["", "  ", "../a.txt", "a/b.txt", "a\\b.txt", "C:a.txt", "a\n.txt", ".."] {
            assert!(build("t", "d", vec![entry("01-models", bad, b"x")]).is_err(), "{bad:?}");
        }
        assert!(e(build("t", "d", vec![entry("01-models", &"n".repeat(121), b"x")])).contains("longer"));
        assert!(e(build("t", "d", vec![entry("01-models", "a.txt", b"")])).contains("is empty"));
        assert!(e(build("t", "d", vec![entry("01-models", "A.txt", b"x"), entry("01-models", "a.TXT", b"y")])).contains("twice"));
        // The same name in two folders is fine.
        assert!(build("t", "d", vec![entry("01-models", "a.txt", b"x"), entry("02-drawings", "a.txt", b"y")]).is_ok());
    }
}
