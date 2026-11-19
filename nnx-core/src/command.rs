//! Atomic commands. Every change to the model goes through a command, and
//! every applied command yields its exact inverse.

use crate::model::{Element, Id, Model};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Origin {
    User,
    Generator,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Command {
    Create(Element),
    Delete(Id),
    /// `None` removes the property.
    SetProp { id: Id, key: String, value: Option<Value> },
    SetLocked { id: Id, locked: bool },
    /// Internal: restores a full element snapshot (used by inverses).
    Restore(Element),
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CommandError {
    #[error("element {0} already exists")]
    AlreadyExists(Id),
    #[error("element {0} not found")]
    NotFound(Id),
    #[error("host {0} not found")]
    MissingHost(Id),
    #[error("element {0} hosts {1} element(s); delete them first")]
    HasDependents(Id, usize),
    #[error("element {0} is locked by the user; generators may not change it")]
    Locked(Id),
    #[error("element {0} would be invalid: {1}")]
    Invalid(Id, String),
}

impl Command {
    /// Applies the command and returns its inverse. On error the model is unchanged.
    pub fn apply(&self, model: &mut Model, origin: Origin) -> Result<Command, CommandError> {
        match self {
            Command::Create(el) => {
                if model.elements.contains_key(&el.id) {
                    return Err(CommandError::AlreadyExists(el.id));
                }
                if let Some(h) = el.host_id
                    && !model.elements.contains_key(&h) {
                        return Err(CommandError::MissingHost(h));
                    }
                model.elements.insert(el.id, el.clone());
                Ok(Command::Delete(el.id))
            }
            Command::Delete(id) => {
                let el = model.get(id).ok_or(CommandError::NotFound(*id))?;
                guard_lock(el, origin)?;
                let deps = model.dependents(id).len();
                if deps > 0 {
                    return Err(CommandError::HasDependents(*id, deps));
                }
                let el = model.elements.remove(id).expect("checked");
                Ok(Command::Create(el))
            }
            Command::SetProp { id, key, value } => {
                let el = model.elements.get_mut(id).ok_or(CommandError::NotFound(*id))?;
                guard_lock(el, origin)?;
                let before = el.clone();
                match value {
                    Some(v) => {
                        el.props.insert(key.clone(), v.clone());
                    }
                    None => {
                        el.props.remove(key);
                    }
                }
                el.rev += 1;
                Ok(Command::Restore(before))
            }
            Command::SetLocked { id, locked } => {
                if origin != Origin::User {
                    return Err(CommandError::Locked(*id));
                }
                let el = model.elements.get_mut(id).ok_or(CommandError::NotFound(*id))?;
                let before = el.clone();
                el.provenance.locked = *locked;
                el.rev += 1;
                Ok(Command::Restore(before))
            }
            Command::Restore(snapshot) => {
                let el = model
                    .elements
                    .get_mut(&snapshot.id)
                    .ok_or(CommandError::NotFound(snapshot.id))?;
                let before = std::mem::replace(el, snapshot.clone());
                Ok(Command::Restore(before))
            }
        }
    }
}

fn guard_lock(el: &Element, origin: Origin) -> Result<(), CommandError> {
    if el.provenance.locked && origin == Origin::Generator {
        Err(CommandError::Locked(el.id))
    } else {
        Ok(())
    }
}
