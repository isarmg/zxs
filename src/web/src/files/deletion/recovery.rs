//! Original soft-delete/undo requests survive refresh and credential replacement.
//! Browser state is only a hint: the server reauthorizes every read and retry.
use super::*;
use crate::files::{FileOperation, SaveStatus};
use json_codec::Value;
use storage_contract::{ErrorCode, OperationId, Principal};
mod storage;
mod view;
pub(crate) use view::view;

#[derive(Clone)]
struct Record {
    principal: Principal,
    operation: OperationId,
    intent: request::Intent,
    request_digest: String,
    bytes: Arc<str>,
}
impl Record {
    fn attempt(&self, epoch: u64) -> request::Attempt {
        request::Attempt {
            epoch,
            principal: self.principal.clone(),
            operation: self.operation.to_string(),
            intent: self.intent.clone(),
            request_digest: self.request_digest.clone(),
            in_flight: false,
        }
    }
    fn matches_receipt(&self, id: &str, receipt: &Value) -> bool {
        receipt["request_digest"].as_str() == Some(self.request_digest.as_str())
            && self.intent.receipt_matches(id, &self.principal, receipt)
    }
}
#[derive(Clone)]
struct Retained {
    record: Arc<Record>,
    // Never persisted. A local completion flag cannot stand in for a live receipt.
    settled: Option<SaveStatus>,
}
impl Retained {
    fn unknown(record: Record) -> Self {
        Self {
            record: Arc::new(record),
            settled: None,
        }
    }
}
#[derive(Clone)]
struct RetryReview {
    epoch: u64,
    generation: u64,
    record: Arc<Record>,
}
#[derive(Clone, Default)]
pub(super) struct Queue {
    pub(super) owner: Option<(u64, Principal)>,
    records: Vec<Retained>,
    error: Option<api::Error>,
    generation: u64,
    review: Option<RetryReview>,
}
impl Queue {
    fn owns(&self, state: &State) -> bool {
        state.session_active()
            && self.owner.as_ref().is_some_and(|(epoch, principal)| {
                *epoch == state.session_epoch && Some(principal) == state.principal.as_ref()
            })
    }
    pub(super) fn ready_for_new(&self, state: &State) -> bool {
        self.owns(state)
            && state.principal.as_ref().is_some_and(|principal| principal.as_str().len() <= 1024)
            && self.records.is_empty()
            && self.error.is_none()
    }
    pub(super) fn settle(&mut self, id: &str, status: SaveStatus) -> bool {
        if status == SaveStatus::Unknown {
            return false;
        }
        if let Some(retained) = self
            .records
            .iter_mut()
            .find(|r| r.record.operation.to_string() == id)
        {
            retained.settled = Some(status);
            self.cancel_review();
            true
        } else {
            false
        }
    }
    fn cancel_review(&mut self) {
        self.generation += 1;
        self.review = None;
    }
    fn merge(&mut self, records: Vec<Retained>) -> Result<(), api::Error> {
        let mut next = self.records.clone();
        for retained in records {
            if let Some(original) = next
                .iter()
                .find(|r| r.record.operation == retained.record.operation)
            {
                if original.record.bytes != retained.record.bytes {
                    return Err(failure(
                        "A retained deletion operation changed; the original in-memory request is preserved.",
                    ));
                }
            } else {
                next.push(retained);
            }
        }
        if next.len() > storage::MAX_RECORDS
            || next.iter().map(|r| r.record.bytes.len()).sum::<usize>() > storage::MAX_QUEUE_BYTES
        {
            return Err(limit(
                "Deletion recovery exceeds its queue budget; the original requests are preserved.",
            ));
        }
        next.sort_by_key(|r| r.record.operation);
        self.records = next;
        Ok(())
    }
}
fn failure(message: &str) -> api::Error {
    api::error(ErrorCode::InvalidRequest, message)
}
fn limit(message: &str) -> api::Error {
    api::error(ErrorCode::ResourceLimit, message)
}
pub(super) fn owner(state: &State) -> Option<(u64, Principal)> {
    state
        .principal
        .as_ref()
        .filter(|_| state.session_active())
        .map(|principal| (state.session_epoch, principal.clone()))
}
pub(crate) fn blocks_writes(state: &State) -> bool {
    let queue = &state.files.deletion.recovery;
    queue.owns(state) && (!queue.records.is_empty() || queue.error.is_some())
}
pub(super) fn save(state: RwSignal<State>, attempt: &request::Attempt) -> bool {
    match storage::save(attempt) {
        Ok(record) => {
            state.update(|s| {
                s.files
                    .deletion
                    .recovery
                    .records
                    .push(Retained::unknown(record))
            });
            true
        }
        Err(error) => {
            state.update(|s| {
                s.files.deletion.recovery.error = Some(error);
                s.notices.clear_operation_notice(&attempt.operation);
            });
            false
        }
    }
}
pub(crate) fn restore(state: RwSignal<State>) {
    let Some(owner) = state.with_untracked(owner) else {
        return;
    };
    if state.with_untracked(|s| {
        s.files.deletion.recovery.owner.as_ref() == Some(&owner)
            && (s.files.mutation_pending
                || s.files
                    .deletion
                    .attempt
                    .as_ref()
                    .is_some_and(|a| a.in_flight))
    }) {
        return;
    }
    let result = storage::load(&owner.1);
    state.update(|s| {
        if self::owner(s).as_ref() != Some(&owner) {
            return;
        }
        adopt_owner(s, &owner);
        let queue = &mut s.files.deletion.recovery;
        queue.cancel_review();
        match result.and_then(|records| queue.merge(records)) {
            Ok(()) => queue.error = None,
            Err(error) => queue.error = Some(error),
        }
    });
    register_pending(state);
    actions::poll(state);
}
fn register_pending(state: RwSignal<State>) {
    let operations = state
        .try_update(|s| {
            if !s.files.deletion.recovery.owns(s) {
                return Vec::new();
            }
            // Generic old ledger observations cannot authenticate restored bytes.
            // Re-query under each exact original request witness.
            let operations: Vec<_> = s
                .files
                .deletion
                .recovery
                .records
                .iter()
                .filter(|r| r.settled.is_none())
                .map(|r| r.record.operation.to_string())
                .collect();
            s.files
                .operations
                .retain(|op| !operations.iter().any(|id| id == op.id()));
            operations
        })
        .expect("invariant: mounted recovery owns its application state");
    for operation in operations {
        actions::record_operation(state, FileOperation::unknown(&operation));
    }
}
fn adopt_owner(state: &mut State, owner: &(u64, Principal)) {
    let model = &mut state.files.deletion;
    if model.recovery.owner.as_ref() == Some(owner) {
        return;
    }
    let mut previous: Vec<_> = model
        .recovery
        .records
        .iter()
        .map(|r| r.record.operation.to_string())
        .collect();
    if let Some(attempt) = model.attempt.take() {
        previous.push(attempt.operation);
        if attempt.in_flight {
            state.files.mutation_pending = false;
        }
    }
    state
        .files
        .operations
        .retain(|op| !previous.iter().any(|id| id == op.id()));
    state
        .files
        .polling_operations
        .retain(|id| !previous.contains(id));
    model.cancel_review();
    model.close();
    // Replacing credentials in the same live app must not discard its exact
    // witness if another tab removed or changed a local key. New credentials
    // still require a fresh receipt, so no old completion flag is carried over.
    let records = if model
        .recovery
        .owner
        .as_ref()
        .is_some_and(|(_, principal)| principal == &owner.1)
    {
        let mut records = std::mem::take(&mut model.recovery.records);
        for retained in &mut records {
            retained.settled = None;
        }
        records
    } else {
        Vec::new()
    };
    model.recovery = Queue {
        owner: Some(owner.clone()),
        records,
        ..Default::default()
    };
}
pub(super) fn compact_outcome(state: &State, id: &str) -> bool {
    let queue = &state.files.deletion.recovery;
    queue.owns(state)
        && queue.records.iter().any(|retained| {
            retained.record.operation.to_string() == id
                && matches!(&retained.record.intent, request::Intent::UndoSummary(_))
        })
}

pub(super) fn receipt_matches(state: &State, id: &str, receipt: &Value) -> Option<bool> {
    let queue = &state.files.deletion.recovery;
    queue
        .records
        .iter()
        .find(|r| r.record.operation.to_string() == id)
        .map(|r| {
            queue.owns(state)
                && Some(&r.record.principal) == state.principal.as_ref()
                && r.record.matches_receipt(id, receipt)
        })
}
/// Called only after the ledger has admitted this exact receipt, or the first
/// POST was definitively rejected. A restored flag from localStorage never gets
/// here. Failed local cleanup keeps the saved witness and the write fence.
pub(crate) fn finish(state: RwSignal<State>, id: &str) {
    let record = state.with_untracked(|s| {
        let queue = &s.files.deletion.recovery;
        if !queue.owns(s) {
            return None;
        }
        queue
            .records
            .iter()
            .find(|r| r.record.operation.to_string() == id && r.settled.is_some())
            .map(|r| r.record.clone())
    });
    let Some(record) = record else {
        return;
    };
    match storage::remove(&record) {
        Ok(()) => {
            let had_error = state.with_untracked(|s| s.files.deletion.recovery.error.is_some());
            state.update(|s| {
                s.files
                    .deletion
                    .recovery
                    .records
                    .retain(|r| r.record.operation != record.operation)
            });
            if had_error {
                // Do not erase a possibly unrelated scan failure just because
                // this one key was removed. Re-establish the bounded scan.
                restore(state);
            }
        }
        Err(error) => state.update(|s| s.files.deletion.recovery.error = Some(error)),
    }
}
fn begin_retry(state: RwSignal<State>, epoch: u64, operation: OperationId) {
    state.update(|s| {
        if !s.session_active() || s.session_epoch != epoch || s.files.mutation_pending {
            return;
        }
        let queue = &mut s.files.deletion.recovery;
        let Some(retained) = queue
            .records
            .iter()
            .find(|r| r.record.operation == operation && r.settled.is_none())
        else {
            return;
        };
        if Some(&retained.record.principal) != s.principal.as_ref() {
            return;
        }
        let record = retained.record.clone();
        queue.cancel_review();
        queue.review = Some(RetryReview {
            epoch,
            generation: queue.generation,
            record,
        });
    });
}
fn reviewed(state: &State, epoch: u64, generation: u64) -> Option<Arc<Record>> {
    let queue = &state.files.deletion.recovery;
    let review = queue.review.as_ref()?;
    let id = review.record.operation.to_string();
    (queue.owns(state)
        && state.session_epoch == epoch
        && review.epoch == epoch
        && review.generation == generation
        && queue.error.is_none()
        && state.management.storage.writable()
        && !state.files.mutation_pending
        && !state
            .files
            .deletion
            .attempt
            .as_ref()
            .is_some_and(|a| a.in_flight)
        && !state.files.polling_operations.contains(&id)
        && queue
            .records
            .iter()
            .any(|r| r.record.bytes == review.record.bytes && r.settled.is_none())
        && state
            .files
            .operations
            .iter()
            .any(|op| op.id() == id && op.status() == SaveStatus::Unknown))
    .then(|| review.record.clone())
}
fn retry(state: RwSignal<State>, epoch: u64, generation: u64) {
    let Some(record) = state.with_untracked(|s| reviewed(s, epoch, generation)) else {
        return;
    };
    if let Err(error) = storage::verify(&record) {
        state.update(|s| s.files.deletion.recovery.error = Some(error));
        return;
    }
    let attempt = record.attempt(epoch);
    state.update(|s| s.files.deletion.recovery.cancel_review());
    request::send(state, attempt, request::Submission::OriginalRetry);
}
