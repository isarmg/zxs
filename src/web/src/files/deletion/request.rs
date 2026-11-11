//! One explicit file or directory request. Navigation cancels only an unsubmitted
//! review; the original in-flight/unknown attempt is owned by the session ledger.
use super::*;
use crate::files::{FileOperation, SaveStatus, model::entry_key, page::Entry};
use json_codec::{Value, json};
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};
use web_interface::i18n::t;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Target {
    pub(super) file: HistoryFileId,
    pub(super) repository: HistoryRepositoryId,
    pub(super) path: name_policy::PortablePath,
}
#[derive(Clone)]
pub(super) struct Review {
    pub(super) generation: u64,
    pub(super) display_path: String,
    pub(super) target: Target,
    pub(super) epoch: u64,
    slot: String,
    directory: String,
    listing_generation: u64,
    entry_id: String,
}
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Intent {
    Delete(Target),
    DeleteBatch(Arc<batch::Request>),
    DeleteDirectory(Arc<directory::Request>),
    Undo(Arc<storage_contract::FileDeletionNode>),
    UndoBatch(Arc<batch_restore::Request>),
    UndoDirectory(Arc<storage_contract::directory_git::DirectoryDeletionNode>),
}
#[derive(Clone)]
pub(super) struct Attempt {
    pub(super) epoch: u64,
    pub(super) principal: String,
    pub(super) operation: String,
    pub(super) intent: Intent,
    pub(super) request_digest: String,
    pub(super) in_flight: bool,
}

pub(super) fn eligible(entry: &Entry) -> bool {
    entry.kind() == storage_contract::NamespaceEntryKind::File
        && !entry.read_only()
        && !entry.preview()
        && !entry.backup()
        && !entry.retained_history()
        && entry.context().is_some()
}
fn chosen<'a>(state: &'a State, explicit: Option<&'a Entry>) -> Option<&'a Entry> {
    if !actions::write_allowed(state) || !state.files.loaded {
        return None;
    }
    let model = &state.files;
    let entry = if let Some(entry) = explicit {
        entry
    } else if model.checked_entries.is_empty() {
        model.selected.as_ref()?
    } else {
        if model.checked_entries.len() != 1 {
            return None;
        }
        model
            .entries
            .iter()
            .find(|entry| model.checked_entries.contains(&entry_key(entry)))?
    };
    // A stale row callback cannot target an entry that left the admitted page.
    if !eligible(entry)
        || !model
            .entries
            .iter()
            .chain(model.selected.iter())
            .any(|current| {
                eligible(current)
                    && current.entry_id() == entry.entry_id()
                    && current.file_id() == entry.file_id()
                    && current.path() == entry.path()
                    && current.context() == entry.context()
            })
    {
        return None;
    }
    Some(entry)
}
pub(crate) fn available(state: &State) -> bool {
    chosen(state, None).is_some() || batch::available(state)
}
fn identity_error(source: storage_contract::Error) -> api::Error {
    api::Error::invalid_response(
        "The selected file does not carry a valid deletion identity.",
        api::ErrorSource::Path(source),
    )
}
fn capture(state: &State, explicit: Option<&Entry>) -> Result<Option<Review>, api::Error> {
    let Some(entry) = chosen(state, explicit) else {
        return Ok(None);
    };
    let (repository, path) = entry
        .context()
        .expect("invariant: chosen file has a current location");
    Ok(Some(Review {
        generation: state.files.deletion.review_generation + 1,
        display_path: format!("{}/{}", state.files.slot, entry.path()),
        target: Target {
            file: HistoryFileId::parse(entry.file_id()).map_err(identity_error)?,
            repository: HistoryRepositoryId::parse(repository).map_err(identity_error)?,
            path: name_policy::PortablePath::parse(path)
                .map_err(|source| identity_error(source.into()))?,
        },
        epoch: state.session_epoch,
        slot: state.files.slot.clone(),
        directory: state.files.directory.clone(),
        listing_generation: state.files.generation,
        entry_id: entry.entry_id().into(),
    }))
}
pub(crate) fn start(state: RwSignal<State>, entry: Option<Entry>) {
    if entry.is_none() && state.with_untracked(|s| s.files.checked_entries.len() > 1) {
        batch::start(state);
        return;
    }
    match state.with_untracked(|s| capture(s, entry.as_ref())) {
        Ok(Some(review)) => state.update(|s| {
            s.files.deletion.cancel_review();
            s.files.deletion.review_generation = review.generation;
            s.files.deletion.review = Some(review);
        }),
        Ok(None) => crate::ui::set_notice(
            state,
            t(
                "请只选择一个当前可写文件。目录、多个文件和只读历史不能使用此软删除入口；不会回退为目录分支删除。",
                "Select one current writable file. Folders, multiple files and read-only history are not supported here; this never falls back to folder-branch deletion.",
            ),
        ),
        Err(error) => fail(state, error),
    }
}

pub(super) fn review_allowed(state: &State, generation: u64) -> bool {
    let Some(review) = state.files.deletion.review.as_ref() else {
        return false;
    };
    actions::write_allowed(state)
        && state.files.loaded
        && review.generation == generation
        && review.epoch == state.session_epoch
        && review.slot == state.files.slot
        && review.directory == state.files.directory
        && review.listing_generation == state.files.generation
        && state
            .files
            .entries
            .iter()
            .chain(state.files.selected.iter())
            .any(|entry| {
                eligible(entry)
                    && entry.entry_id() == review.entry_id
                    && entry.file_id() == review.target.file.as_str()
                    && entry.context()
                        == Some((
                            review.target.repository.as_str(),
                            review.target.path.as_str(),
                        ))
            })
}
pub(super) fn submit_delete(state: RwSignal<State>, generation: u64) {
    let target = state.with_untracked(|s| {
        review_allowed(s, generation).then(|| {
            s.files
                .deletion
                .review
                .as_ref()
                .expect("invariant: admitted review exists")
                .target
                .clone()
        })
    });
    if let Some(target) = target {
        submit(state, Intent::Delete(target));
    }
}
pub(super) fn submit_undo(state: RwSignal<State>, node: Arc<storage_contract::FileDeletionNode>) {
    submit(state, Intent::Undo(node));
}
#[derive(data_serialization::Serialize)]
#[serialization(untagged)]
pub(super) enum Body {
    Json(Value),
    BatchUndo(storage_contract::file_deletion_batch::UndoFileDeletionBatchBody),
}

#[derive(data_serialization::Serialize)]
struct DigestInput<'a, T> {
    kind: &'a str,
    principal: &'a str,
    body: T,
}
pub(super) fn native_digest<T: data_serialization::Serialize>(
    kind: &str,
    principal: &str,
    body: T,
) -> Result<String, api::Error> {
    storage_contract::object_hash(
        "request",
        &DigestInput {
            kind,
            principal,
            body,
        },
    )
    .map_err(identity_error)
}

impl Intent {
    pub(super) fn request(&self, operation_id: storage_contract::OperationId) -> (String, Body) {
        let operation = operation_id.to_string();
        let operation = operation.as_str();
        let (endpoint, body) = match self {
            Self::Delete(target) => (
                format!("/api/files/{}/deletion", api::segment(target.file.as_str())),
                json!({"repository_id":target.repository.as_str(),"path":target.path.as_str(),"operation_id":operation}),
            ),
            Self::DeleteBatch(request) => request.request(operation),
            Self::UndoBatch(request) => {
                return (
                    "/api/file-deletion-batches/undo".into(),
                    Body::BatchUndo(request.request(operation_id)),
                );
            }
            Self::DeleteDirectory(request) => request.request(operation),
            Self::Undo(node) => (
                format!("/api/files/{}/deletion/undo", api::segment(&node.file_id)),
                json!({"deletion_node_id":node.node_id,"operation_id":operation}),
            ),
            Self::UndoDirectory(node) => (
                format!(
                    "/api/repositories/{}/directory-deletions/undo",
                    api::segment(&node.repository_id)
                ),
                json!({"owner_directory_id":node.owner_directory_id,"directory_id":node.directory_id,"deletion_node_id":node.node_id,"operation_id":operation}),
            ),
        };
        (endpoint, Body::Json(body))
    }
    pub(super) fn digest(&self, principal: &str) -> Result<String, api::Error> {
        let (kind, body) = match self {
            Self::Delete(target) => (
                "file.delete",
                json!({
                    "repository_id":target.repository.as_str(), "file_id":target.file.as_str(), "path":target.path.key(),
                }),
            ),
            Self::DeleteBatch(request) => ("file.delete.batch", request.input()),
            Self::UndoBatch(request) => return request.digest(principal),
            Self::DeleteDirectory(request) => ("directory.delete", request.input()),
            Self::Undo(node) => (
                "file.delete.undo",
                json!({
                    "file_id":node.file_id, "deletion_node_id":node.node_id,
                }),
            ),
            Self::UndoDirectory(node) => (
                "directory.delete.undo",
                json!({"repository_id":node.repository_id,"owner_directory_id":node.owner_directory_id,"directory_id":node.directory_id,"deletion_node_id":node.node_id}),
            ),
        };
        native_digest(kind, principal, body)
    }
    pub(super) fn receipt_matches(&self, id: &str, principal: &str, receipt: &Value) -> bool {
        match self {
            Self::Delete(target) => {
                if receipt["operation_kind"] != "file.delete" {
                    return false;
                }
                let Some(sequence) = wire::publication_sequence(receipt, &["deletion_node"]) else {
                    return false;
                };
                let value = &receipt["result"]["deletion_node"];
                // Complete bodyless frontier validation is shared with the read
                // boundary. Neither a different file nor a generic receipt can
                // release this exact request's unknown-outcome fence.
                let Some(deletion_id) = value["node_id"].as_str() else {
                    return false;
                };
                if storage_contract::validate_id(deletion_id, "fdel").is_err() {
                    return false;
                }
                let summary = wire::Summary {
                    file_id: target.file.clone(),
                    repository_id: target.repository.clone(),
                    deletion_node_id: deletion_id.into(),
                    path: target.path.clone(),
                };
                matches!(wire::node(value.clone(), &summary), Ok(Some(node)) if node.operation_id.to_string() == id && node.author == principal && node.sequence == sequence)
            }
            Self::DeleteBatch(request) => request.receipt_matches(id, principal, receipt),
            Self::UndoBatch(request) => request.receipt_matches(receipt),
            Self::DeleteDirectory(request) => request.receipt_matches(id, principal, receipt),
            Self::Undo(node) => {
                receipt["operation_kind"] == "file.delete.undo"
                    && wire::publication_sequence(
                        receipt,
                        &["restored_file_id", "removed_deletion_node"],
                    )
                    .is_some_and(|sequence| node.sequence < sequence)
                    && receipt["result"]["restored_file_id"] == node.file_id
                    && receipt["result"]["removed_deletion_node"] == json!(node.as_ref())
            }
            Self::UndoDirectory(node) => directory::wire::undo_receipt_matches(node, receipt),
        }
    }
}
pub(super) fn submit(state: RwSignal<State>, intent: Intent) {
    // Admission is checked before generating an operation ID or touching storage.
    // A review alone does not authorize bypassing the restored-request queue.
    if !state.with_untracked(|s| s.files.deletion.recovery.ready_for_new(s)) {
        return;
    }
    let request_digest = match state.with_untracked(|s| intent.digest(&s.principal)) {
        Ok(digest) => digest,
        Err(error) => {
            fail(state, error);
            return;
        }
    };
    let Some(id) = crate::ui::file_operation_id(state) else {
        return;
    };
    let epoch = state.with_untracked(|s| s.session_epoch);
    let attempt = Attempt {
        epoch,
        principal: state.with_untracked(|s| s.principal.clone()),
        operation: id,
        intent,
        request_digest,
        in_flight: false,
    };
    // Saving and reading back the exact original is a prerequisite, not a
    // best-effort side effect after a server mutation has already started.
    if recovery::save(state, &attempt) {
        send(state, attempt, Submission::First);
    }
}
pub(super) enum Submission {
    First,
    OriginalRetry,
}
pub(super) fn send(state: RwSignal<State>, mut attempt: Attempt, submission: Submission) {
    if !state.with_untracked(|s| {
        s.session_active()
            && s.session_epoch == attempt.epoch
            && s.principal == attempt.principal
            && !s.files.mutation_pending
            && !s
                .files
                .deletion
                .attempt
                .as_ref()
                .is_some_and(|a| a.in_flight)
    }) {
        return;
    }
    let id = attempt.operation.clone();
    let epoch = attempt.epoch;
    let operation = id
        .parse()
        .expect("invariant: the retained or generated operation ID is canonical");
    let (endpoint, body) = attempt.intent.request(operation);
    attempt.in_flight = true;
    state.update(|s| {
        s.files.mutation_pending = true;
        s.files.deletion.attempt = Some(attempt);
    });
    // Record the original ID before starting network work, including when the
    // review or deleted-files panel closes while the request is outstanding.
    actions::record_operation(state, FileOperation::unknown(&id));
    spawn_local(async move {
        if !current_session(state, epoch) {
            return;
        }
        let result = api::typed_request("POST", &endpoint, body).await;
        if !current_session(state, epoch) {
            return;
        }
        state.update(|s| {
            if let Some(attempt) = s
                .files
                .deletion
                .attempt
                .as_mut()
                .filter(|a| a.operation == id)
            {
                attempt.in_flight = false;
            }
        });
        match result {
            Ok(value) => {
                actions::record_receipt(state, &id, value);
            }
            Err(error) => failed(state, epoch, &id, error, submission),
        }
        state.update(|s| s.files.mutation_pending = false);
        if state.with_untracked(|s| s.files.deletion.open) {
            load(state, false);
        }
    });
}
fn failed(state: RwSignal<State>, epoch: u64, id: &str, error: api::Error, submission: Submission) {
    match submission {
        Submission::First => actions::record_failure(state, epoch, id, error),
        Submission::OriginalRetry => {
            // A rejected retry says nothing about the original attempt.
            // Keep its immutable record, ID and unknown-outcome fence.
            actions::record_operation(state, FileOperation::unknown(id));
            crate::ui::fail_in(
                state,
                epoch,
                crate::ui::ErrorContext::Operation(id.to_owned()),
                error,
            );
        }
    }
}
pub(crate) fn in_flight(state: &State, id: &str) -> bool {
    state
        .files
        .deletion
        .attempt
        .as_ref()
        .is_some_and(|attempt| attempt.operation == id && attempt.in_flight)
}
pub(crate) fn receipt_matches(state: &State, id: &str, receipt: &Value) -> bool {
    let attempt = state
        .files
        .deletion
        .attempt
        .as_ref()
        .filter(|attempt| attempt.operation == id);
    if attempt.is_some_and(|attempt| {
        !state.session_active()
            || attempt.epoch != state.session_epoch
            || attempt.principal != state.principal
    }) {
        return false;
    }
    recovery::receipt_matches(state, id, receipt).unwrap_or_else(|| {
        attempt.is_none_or(|attempt| {
            receipt["request_digest"].as_str() == Some(attempt.request_digest.as_str())
                && attempt
                    .intent
                    .receipt_matches(id, &attempt.principal, receipt)
        })
    })
}

pub(crate) fn settle(model: &mut super::super::model::Model, id: &str, status: SaveStatus) {
    let retained = model.deletion.recovery.settle(id, status);
    let active = model
        .deletion
        .attempt
        .as_ref()
        .is_some_and(|attempt| attempt.operation == id);
    if status == SaveStatus::Unknown || !(retained || active) {
        return;
    }
    if active {
        model.deletion.attempt = None;
    }
    model.deletion.cancel_review();
    if matches!(status, SaveStatus::Published | SaveStatus::Degraded) {
        model.checked_entries.clear();
        model.deletion.directory.invalidate();
        // Cached D is not restoration authority after another lifecycle event.
        model.deletion.generation += 1;
        model.deletion.loading = false;
        model.deletion.node = None;
        model.deletion.selected = None;
        model.deletion.page = None;
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/deletion/request_tests.rs"]
mod tests;
