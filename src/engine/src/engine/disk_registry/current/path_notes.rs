//! Historical node notes change one mutable annotation, never a namespace.
//! Replay authenticates the same request and exact write set before advance can
//! take its unchanged-HEAD shortcut. No body or stopped-root access is needed.
use super::*;
use crate::{CommitState, OperationId, OperationResult, ProtectionAtPublish};
use json_codec::json;
use storage_contract::{MapChanges, Note};

const KIND: &str = "file.path.metadata";

fn invalid() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "file node note publication differs from its historical target or request",
    )
}

impl Engine {
    /// A note names an existing path or metadata node, including historical and
    /// consumed-branch nodes. It does not prove a current occurrence or body I/O.
    pub(crate) fn file_node_note_history<'a>(
        state: &'a State,
        file: &str,
        node: &str,
    ) -> Result<&'a str> {
        let missing = || Error::new(ErrorCode::FileNodeNotFound, "file metadata node missing");
        let (id, owner, history, version) = match (
            state.file_path_nodes.get(node),
            state.file_metadata_nodes.get(node),
        ) {
            (Some(path), None) => (
                &path.node_id,
                &path.file_id,
                &path.history_tree_id,
                &path.version_id,
            ),
            (None, Some(metadata)) => (
                &metadata.node_id,
                &metadata.file_id,
                &metadata.history_tree_id,
                &metadata.version_id,
            ),
            _ => return Err(missing()),
        };
        if id != node
            || owner != file
            || crate::node_retirement_engine::is_retired(state, version)
            || !state.versions.get(version).is_some_and(|body| {
                body.version_id == *version
                    && body.file_id == file
                    && body.history_tree_id == *history
            })
        {
            return Err(missing());
        }
        Ok(history)
    }
}

struct NoteRequest<'a> {
    file: &'a str,
    node: &'a str,
    note: Note,
}

impl<'a> NoteRequest<'a> {
    fn from_receipt(receipt: &'a OperationResult) -> Result<Self> {
        let result = &receipt.result;
        let fields = result.as_object().ok_or_else(invalid)?;
        // The three public result fields plus the ordinary planner envelope.
        if fields.len() != 6
            || !result["timestamp"]
                .as_str()
                .is_some_and(|value| value.parse::<u64>().is_ok())
            || !result["membership_epoch"]
                .as_str()
                .is_some_and(|value| value.parse::<u64>().is_ok())
            || !result["version_protection"]
                .as_array()
                .is_some_and(Vec::is_empty)
        {
            return Err(invalid());
        }
        let file = result["file_id"].as_str().ok_or_else(invalid)?;
        let node = result["node_id"].as_str().ok_or_else(invalid)?;
        let note = result["note"].as_str().ok_or_else(invalid)?;
        Ok(Self {
            file,
            node,
            note: Note::parse(note.to_owned()).map_err(|_| invalid())?,
        })
    }

    fn verify(&self, previous: &State, next: &State, receipt: &OperationResult) -> Result<()> {
        let operation = receipt.operation_id;
        let principal = next
            .operation_principals
            .get(&operation)
            .ok_or_else(invalid)?;
        Engine::require_actor(principal).map_err(|_| invalid())?;
        let history = Engine::file_node_note_history(previous, self.file, self.node)
            .map_err(|_| invalid())?;
        let history_record = previous.histories.get(history).ok_or_else(invalid)?;
        let repository = previous
            .repositories
            .get(&history_record.repository_id)
            .ok_or_else(invalid)?;
        if history_record.history_tree_id != history
            || repository.repository_id != history_record.repository_id
            || repository.history_tree_id != history
            || receipt.kind != KIND
            || receipt.sequence != next.sequence
            || previous.operations.contains_key(&operation)
            || previous.operation_principals.contains_key(&operation)
            || receipt.commit_id.is_some()
            || receipt.branch_id.is_some()
            || receipt.history_tree_id.is_some()
            || !matches!(
                receipt.commit_state,
                CommitState::Published | CommitState::PublishedDegraded
            )
            || receipt.protection_at_publish != ProtectionAtPublish::NotApplicable
            || receipt.request_digest
                != Engine::native_request_digest(
                    KIND,
                    principal,
                    &json!({"file_id":self.file,"node_id":self.node,"note":self.note.as_str()}),
                )?
        {
            return Err(invalid());
        }
        let expected = (!self.note.as_str().is_empty()).then_some(self.note.as_str());
        if next.file_path_notes.get(self.node).map(String::as_str) != expected
            || !notes_unchanged_except(previous, next, Some(self.node))
        {
            return Err(invalid());
        }
        crate::publication_shape::verify(
            previous,
            next,
            receipt,
            crate::publication_shape::ChangedTable::FilePathNotes,
            invalid,
        )
    }
}

impl CurrentNamespaces {
    pub(super) fn verify_path_notes(&self, previous: &State, next: &State) -> Result<()> {
        let mut note = None;
        let mut inspect = |id: &OperationId| -> Result<()> {
            let before = previous.operations.get(id);
            let after = next.operations.get(id);
            if before == after
                || !before
                    .into_iter()
                    .chain(after)
                    .any(|receipt| receipt.kind == KIND)
            {
                return Ok(());
            }
            let receipt = after.ok_or_else(invalid)?;
            if before.is_some() || receipt.operation_id != *id || note.replace(receipt).is_some() {
                return Err(invalid());
            }
            Ok(())
        };
        match next.operations.changes_since(&previous.operations) {
            MapChanges::Keys(keys) => {
                for id in keys.iter() {
                    inspect(id)?;
                }
            }
            MapChanges::All => {
                for id in next.operations.keys() {
                    inspect(id)?;
                }
                for id in previous
                    .operations
                    .keys()
                    .filter(|id| !next.operations.contains_key(*id))
                {
                    inspect(id)?;
                }
            }
        }
        publication_delta::events(
            previous,
            next,
            note.map(|receipt| receipt.operation_id),
            |kind| kind == KIND,
            invalid,
        )?;
        match note {
            Some(receipt) => NoteRequest::from_receipt(receipt)?.verify(previous, next, receipt),
            None if notes_unchanged_except(previous, next, None) => Ok(()),
            None => Err(invalid()),
        }
    }
}

fn notes_unchanged_except(previous: &State, next: &State, node: Option<&str>) -> bool {
    let unchanged = |id: &String| {
        node == Some(id.as_str())
            || previous.file_path_notes.get(id) == next.file_path_notes.get(id)
    };
    match next
        .file_path_notes
        .changes_since(&previous.file_path_notes)
    {
        MapChanges::Keys(keys) => keys.iter().all(unchanged),
        MapChanges::All => previous
            .file_path_notes
            .keys()
            .chain(next.file_path_notes.keys())
            .all(unchanged),
    }
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/current_path_note_tests.rs"]
mod tests;
