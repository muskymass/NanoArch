//! Element model: the single source of truth for every project element.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use uuid::Uuid;

pub type Id = Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Discipline {
    Arc,
    Str,
    Ele,
    Ict,
    Plb,
    Drn,
    Hvac,
    Site,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    User,
    Generated,
    Computed,
    Imported,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub source: Source,
    /// A locked element is protected from generators; only the user may change it.
    pub locked: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Element {
    pub id: Id,
    /// Element type, e.g. "Wall", "Door", "Column".
    pub kind: String,
    pub discipline: Discipline,
    pub level_id: Option<Id>,
    /// Host element (a door's wall). Enforced: the host must exist.
    pub host_id: Option<Id>,
    /// Type-specific properties. Lengths in integer millimetres.
    pub props: Map<String, Value>,
    pub provenance: Provenance,
    /// Incremented on every change; used for cache invalidation.
    pub rev: u64,
}

impl Element {
    pub fn new(kind: &str, discipline: Discipline, source: Source) -> Self {
        Self {
            id: Uuid::now_v7(),
            kind: kind.to_string(),
            discipline,
            level_id: None,
            host_id: None,
            props: Map::new(),
            provenance: Provenance { source, locked: false },
            rev: 0,
        }
    }

    pub fn with_prop(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.props.insert(key.to_string(), value.into());
        self
    }

    pub fn hosted_by(mut self, host: Id) -> Self {
        self.host_id = Some(host);
        self
    }
}

/// The unified project model. A `BTreeMap` keeps iteration (and therefore
/// serialization and hashing) deterministic.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub elements: BTreeMap<Id, Element>,
}

impl Model {
    pub fn get(&self, id: &Id) -> Option<&Element> {
        self.elements.get(id)
    }

    pub fn len(&self) -> usize {
        self.elements.len()
    }

    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    /// Elements hosted by `id` (doors and windows in a wall).
    pub fn dependents(&self, id: &Id) -> Vec<Id> {
        self.elements
            .values()
            .filter(|e| e.host_id == Some(*id))
            .map(|e| e.id)
            .collect()
    }

    /// Stable 64-bit FNV-1a hash of the full model state, for equality checks
    /// across undo/redo and save/load.
    pub fn state_hash(&self) -> u64 {
        let bytes = serde_json::to_vec(&self.elements).expect("model serializes");
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }
}
