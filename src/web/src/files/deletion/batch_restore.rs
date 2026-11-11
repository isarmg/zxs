//! Restore one complete original file-deletion batch through the existing ledger.
use crate::{
    api,
    ui::{State, accept_read_response, fail},
};
use crate::files::deletion::{request, restore, wire};
use std::{collections::BTreeSet, sync::Arc};
use web_interface::{prelude::*, tasks::spawn_local};
use data_serialization::Deserialize;
use json_codec::Value;
use storage_contract::{
    FileDeletionNode, OperationId,
    client_protocol::MAX_FILE_DELETION_FRONTIER_ENTRIES,
    file_deletion_batch::{FileDeletionBatchUndoPreview, UndoFileDeletionBatchBody, UNDO_KIND},
};
mod view;
pub(super) use view::{button, retained_view, view};

#[derive(Debug, PartialEq)]
pub(super) struct Request {
    preview: FileDeletionBatchUndoPreview,
}
fn invalid(message: &str) -> api::Error {
    api::Error::invalid_response(
        "The complete deletion batch did not match its original identities.",
        api::ErrorSource::FileDeletion(
            <json_codec::Error as data_serialization::de::Error>::custom(message),
        ),
    )
}
impl Request {
    pub(super) fn admit(
        deletion_operation_id: OperationId,
        deletion_nodes: Vec<FileDeletionNode>,
    ) -> Result<Self, api::Error> {
        if deletion_nodes.is_empty() || deletion_nodes.len() > 100 {
            return Err(invalid("the batch must contain one to one hundred deletion nodes"));
        }
        let first = &deletion_nodes[0];
        if first.sequence == 0
            || deletion_nodes.windows(2).any(|pair| pair[0].file_id >= pair[1].file_id)
        {
            return Err(invalid("the original batch requires a publication sequence and ordered unique files"));
        }
        let mut node_ids = BTreeSet::new();
        let mut branches = 0;
        let mut parents = 0;
        for node in &deletion_nodes {
            wire::validate_node(node)?;
            if node.operation_id != deletion_operation_id {
                return Err(invalid("the deletion node belongs to another operation"));
            }
            if node.repository_id != first.repository_id
                || node.history_tree_id != first.history_tree_id
                || node.sequence != first.sequence
                || node.author != first.author
                || !node_ids.insert(&node.node_id)
            {
                return Err(invalid("the original batch members must share one publication and distinct deletion identities"));
            }
            branches += node.branch_heads.len();
            parents += node.parent_node_ids.len();
            if branches > MAX_FILE_DELETION_FRONTIER_ENTRIES
                || parents > MAX_FILE_DELETION_FRONTIER_ENTRIES
            {
                return Err(invalid("the complete batch exceeds its frontier budget"));
            }
        }
        Ok(Self {
            preview: FileDeletionBatchUndoPreview {
                deletion_operation_id,
                deletion_nodes,
            },
        })
    }
    fn decode(value: Value, selected: &FileDeletionNode) -> Result<Self, api::Error> {
        if !value.is_object()
            || !value["deletion_nodes"].as_array()
                .is_some_and(|nodes| nodes.iter().all(Value::is_object))
        {
            return Err(invalid("the batch preview requires named deletion-node objects"));
        }
        let preview = FileDeletionBatchUndoPreview::deserialize(value).map_err(|source| {
            api::Error::invalid_response(
                "The complete deletion batch response is invalid.",
                api::ErrorSource::FileDeletion(source),
            )
        })?;
        let request = Self::admit(preview.deletion_operation_id, preview.deletion_nodes)?;
        if request.preview.deletion_operation_id != selected.operation_id
            || !request.preview.deletion_nodes.iter().any(|node| node == selected)
        {
            return Err(invalid("the selected complete D is not in this original batch"));
        }
        Ok(request)
    }
    pub(super) fn preview(&self) -> &FileDeletionBatchUndoPreview {
        &self.preview
    }
    pub(super) fn original(&self) -> OperationId {
        self.preview.deletion_operation_id
    }
    pub(super) fn nodes(&self) -> &[FileDeletionNode] {
        &self.preview.deletion_nodes
    }
    pub(super) fn digest(&self, principal: &str) -> Result<String, api::Error> {
        #[derive(data_serialization::Serialize)]
        struct Input {
            deletion_operation_id: OperationId,
        }
        request::native_digest(UNDO_KIND, principal, Input {
            deletion_operation_id: self.original(),
        })
    }
    pub(super) fn request(&self, operation_id: OperationId) -> UndoFileDeletionBatchBody {
        UndoFileDeletionBatchBody {
            deletion_operation_id: self.original(),
            operation_id,
        }
    }
    pub(super) fn receipt_matches(&self, receipt: &Value) -> bool {
        if receipt["operation_kind"] != UNDO_KIND
            || receipt["result"]["deletion_operation_id"].as_str()
                != Some(self.original().to_string().as_str())
            || !wire::publication_sequence(
                receipt, &["deletion_operation_id", "removed_deletion_nodes"],
            ).is_some_and(|sequence| self.nodes().iter().all(|node| sequence > node.sequence))
        {
            return false;
        }
        let removed = &receipt["result"]["removed_deletion_nodes"];
        if !removed.as_array().is_some_and(|nodes| nodes.len() == self.nodes().len()
            && nodes.iter().all(Value::is_object))
        {
            return false;
        }
        matches!(Vec::<FileDeletionNode>::deserialize(removed),
            Ok(nodes) if nodes == self.preview.deletion_nodes)
    }
}
#[derive(Clone)]
pub(super) struct Review {
    target: restore::Target,
    generation: u64,
    request: Arc<Request>,
}
fn begin(state: RwSignal<State>, target: restore::Target) {
    if !state.with_untracked(|s| target.current(s)) {
        return;
    }
    let (generation, review_generation) = state.try_update(|s| {
        let model = &mut s.files.deletion;
        model.cancel_review();
        model.generation += 1;
        model.loading = true;
        (model.generation, model.review_generation)
    }).expect("invariant: mounted batch restore owns its application state");
    spawn_local(async move {
        let endpoint = format!("/api/file-deletion-batches/{}", target.node.operation_id);
        let result = api::validated_request("GET", &endpoint, None, |value| {
            Request::decode(value, &target.node)
        }).await;
        complete(state, target, generation, review_generation, result);
    });
}
fn complete(
    state: RwSignal<State>,
    target: restore::Target,
    generation: u64,
    review_generation: u64,
    result: Result<Request, api::Error>,
) {
    if !accept_read_response(state, target.epoch, &result)
        || !state.with_untracked(|s| s.files.deletion.accepts(generation))
    {
        return;
    }
    state.update(|s| s.files.deletion.loading = false);
    if !state.with_untracked(|s| s.files.deletion.review_generation == review_generation) {
        return;
    }
    match result {
        Ok(request) => state.update(|s| {
            s.files.deletion.restore_review = Some(restore::Review::Batch(Review {
                target: restore::Target { generation, ..target },
                generation: review_generation,
                request: Arc::new(request),
            }));
        }),
        Err(error) => fail(state, error),
    }
}
fn reviewed(state: &State, epoch: u64, generation: u64) -> Option<Arc<Request>> {
    let review = state.files.deletion.restore_review.as_ref()?.batch()?;
    (review.target.epoch == epoch
        && review.generation == generation
        && state.files.deletion.review_generation == generation
        && review.target.current(state))
        .then(|| review.request.clone())
}
fn confirm(state: RwSignal<State>, epoch: u64, generation: u64) {
    let Some(request) = state.with_untracked(|s| reviewed(s, epoch, generation)) else {
        return;
    };
    state.update(|s| s.files.deletion.cancel_review());
    request::submit(state, request::Intent::UndoBatch(request));
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/deletion/batch_restore_tests.rs"]
mod tests;
