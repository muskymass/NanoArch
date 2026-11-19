//! `.nnx` project file: a SQLite database. Saves write to a temporary file
//! and rename it over the target, so a crash never leaves a half-written file.
//!
//! Besides the model's elements the file can carry project settings (the user's brief, room
//! display names, structural inputs...) so a project can be closed and reopened as it was.
//! Settings live in the `meta` table under `setting:<name>` keys, so files written without
//! settings, and by older versions, still load (with empty settings).

use crate::model::{Element, Model};
use rusqlite::{Connection, params};
use std::collections::BTreeMap;
use std::path::Path;

pub const SCHEMA_VERSION: &str = "1.0.0";
const SETTING_PREFIX: &str = "setting:";

/// Project settings: name -> JSON value, stored beside the model.
pub type Settings = BTreeMap<String, serde_json::Value>;

#[derive(Debug, thiserror::Error)]
pub enum PersistError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported schema version {0}")]
    Version(String),
}

pub fn save(model: &Model, path: &Path) -> Result<(), PersistError> {
    save_with_settings(model, &Settings::new(), path)
}

pub fn save_with_settings(model: &Model, settings: &Settings, path: &Path) -> Result<(), PersistError> {
    let tmp = path.with_extension("nnx.tmp");
    if tmp.exists() {
        std::fs::remove_file(&tmp)?;
    }
    {
        let mut conn = Connection::open(&tmp)?;
        conn.execute_batch(
            "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE elements(id TEXT PRIMARY KEY, kind TEXT NOT NULL, rev INTEGER NOT NULL, json TEXT NOT NULL);
             CREATE INDEX elements_kind ON elements(kind);",
        )?;
        let tx = conn.transaction()?;
        tx.execute("INSERT INTO meta VALUES('schema_version', ?1)", params![SCHEMA_VERSION])?;
        for (name, value) in settings {
            tx.execute("INSERT INTO meta VALUES(?1, ?2)", params![format!("{SETTING_PREFIX}{name}"), serde_json::to_string(value)?])?;
        }
        {
            let mut stmt = tx.prepare("INSERT INTO elements VALUES(?1, ?2, ?3, ?4)")?;
            for el in model.elements.values() {
                stmt.execute(params![el.id.to_string(), el.kind, el.rev as i64, serde_json::to_string(el)?])?;
            }
        }
        tx.commit()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn load(path: &Path) -> Result<Model, PersistError> {
    load_with_settings(path).map(|(model, _)| model)
}

pub fn load_with_settings(path: &Path) -> Result<(Model, Settings), PersistError> {
    let conn = Connection::open(path)?;
    let version: String =
        conn.query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| r.get(0))?;
    if version.split('.').next() != SCHEMA_VERSION.split('.').next() {
        return Err(PersistError::Version(version));
    }
    let mut model = Model::default();
    {
        let mut stmt = conn.prepare("SELECT json FROM elements")?;
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
            let el: Element = serde_json::from_str(&row?)?;
            model.elements.insert(el.id, el);
        }
    }
    let mut settings = Settings::new();
    let mut stmt = conn.prepare("SELECT key, value FROM meta WHERE key LIKE 'setting:%'")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (key, value) = row?;
        settings.insert(key[SETTING_PREFIX.len()..].to_string(), serde_json::from_str(&value)?);
    }
    Ok((model, settings))
}
