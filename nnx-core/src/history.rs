//! Transactions and undo/redo. A transaction is all-or-nothing: if any
//! command fails, the ones already applied are rolled back.

use crate::command::{Command, CommandError, Origin};
use crate::model::{Id, Model};

#[derive(Debug, Clone)]
pub struct Transaction {
    pub label: String,
    pub origin: Origin,
    /// Inverses in application order; undo runs them in reverse.
    inverses: Vec<Command>,
    forward: Vec<Command>,
}

#[derive(Debug)]
pub struct Document {
    pub model: Model,
    /// When true (the default) every commit is checked against the typed element schema
    /// (`typed::validate_element`) and rolled back if it would leave an invalid element.
    pub strict: bool,
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    /// State hash at the last save (or load); `is_dirty` compares against it.
    saved_hash: u64,
}

#[derive(Debug, thiserror::Error, PartialEq)]
#[error("transaction '{label}' failed at command {index}: {source}")]
pub struct TxError {
    pub label: String,
    pub index: usize,
    pub source: CommandError,
}

impl Default for Document {
    fn default() -> Self {
        Self { saved_hash: Model::default().state_hash(), model: Model::default(), strict: true, undo: Vec::new(), redo: Vec::new() }
    }
}

impl Document {
    pub fn new(model: Model) -> Self {
        Self { saved_hash: model.state_hash(), model, ..Default::default() }
    }

    /// Call after a successful save. Undoing back to the saved state makes the document clean again.
    pub fn mark_saved(&mut self) {
        self.saved_hash = self.model.state_hash();
    }

    pub fn is_dirty(&self) -> bool {
        self.model.state_hash() != self.saved_hash
    }

    /// A document that accepts partially specified elements (imports, scratch data).
    pub fn lenient() -> Self {
        Self { strict: false, ..Default::default() }
    }

    /// Applies `cmds` atomically as one undoable step.
    pub fn commit(&mut self, label: &str, origin: Origin, cmds: Vec<Command>) -> Result<(), TxError> {
        let inverses = run_atomic(&mut self.model, &cmds, origin).map_err(|(index, source)| TxError {
            label: label.to_string(),
            index,
            source,
        })?;
        if self.strict
            && let Some((index, id, msg)) = first_invalid(&self.model, &cmds) {
                for inv in inverses.iter().rev() {
                    inv.apply(&mut self.model, Origin::User).expect("rollback is infallible");
                }
                return Err(TxError { label: label.to_string(), index, source: CommandError::Invalid(id, msg) });
            }
        self.undo.push(Transaction { label: label.to_string(), origin, inverses, forward: cmds });
        self.redo.clear();
        Ok(())
    }

    pub fn undo(&mut self) -> bool {
        let Some(tx) = self.undo.pop() else { return false };
        let rev: Vec<Command> = tx.inverses.iter().rev().cloned().collect();
        // Inverses of applied commands cannot fail on the state they produced.
        run_atomic(&mut self.model, &rev, Origin::User).expect("undo is infallible");
        self.redo.push(tx);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(mut tx) = self.redo.pop() else { return false };
        tx.inverses = run_atomic(&mut self.model, &tx.forward, tx.origin).expect("redo is infallible");
        self.undo.push(tx);
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|t| t.label.as_str())
    }
}

fn run_atomic(model: &mut Model, cmds: &[Command], origin: Origin) -> Result<Vec<Command>, (usize, CommandError)> {
    let mut inverses = Vec::with_capacity(cmds.len());
    for (i, cmd) in cmds.iter().enumerate() {
        match cmd.apply(model, origin) {
            Ok(inv) => inverses.push(inv),
            Err(e) => {
                for inv in inverses.iter().rev() {
                    inv.apply(model, Origin::User).expect("rollback is infallible");
                }
                return Err((i, e));
            }
        }
    }
    Ok(inverses)
}

/// First element touched by `cmds` (or hosted by one) that violates the typed schema.
/// Returns the index of the command that touched it.
fn first_invalid(model: &Model, cmds: &[Command]) -> Option<(usize, Id, String)> {
    use crate::typed::{Space, Typed, validate_element};
    let mut done = std::collections::BTreeSet::new();
    for (index, cmd) in cmds.iter().enumerate() {
        let id = match cmd {
            Command::Create(el) | Command::Restore(el) => el.id,
            Command::SetProp { id, .. } => *id,
            Command::Delete(_) | Command::SetLocked { .. } => continue,
        };
        let Some(el) = model.get(&id) else { continue };
        let mut targets = vec![id];
        targets.extend(model.dependents(&id));
        for t in targets {
            if !done.insert(t) {
                continue;
            }
            let Some(e) = model.get(&t) else { continue };
            if let Err(msg) = validate_element(model, e) {
                return Some((index, t, msg));
            }
        }
        if el.kind == Space::KIND
            && let Ok(s) = el.typed::<Space>() {
                let clash = model.elements.values().any(|o| {
                    o.id != id && o.kind == Space::KIND && o.props.get("requirementKey").and_then(|v| v.as_str()) == Some(&s.requirement_key)
                });
                if clash {
                    return Some((index, id, format!("requirementKey '{}' is already used by another space", s.requirement_key)));
                }
            }
    }
    None
}
