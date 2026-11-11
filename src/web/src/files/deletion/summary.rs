//! Large deletion records use bounded audit pages and the same exact undo command.
use crate::api;
use crate::files::deletion::{Inspection, wire};
use crate::ui::State;
use data_serialization::Deserialize;
use json_codec::Value;
use std::sync::Arc;
use storage_contract::{
    BranchId,
    file_deletion_review::{BranchPage, ParentPage, Summary},
};

mod read;
mod restore;
mod view;
pub(super) use read::load;
pub(super) use restore::Review as RestoreReview;
pub(super) use view::view;

pub(super) fn needs_pages(code: storage_contract::ErrorCode) -> bool {
    matches!(
        code,
        storage_contract::ErrorCode::ResourceLimitExceeded
            | storage_contract::ErrorCode::ResponseTooLarge
            | storage_contract::ErrorCode::ResourceLimit
    )
}

#[derive(Clone)]
pub(super) struct Model {
    summary: Arc<Summary>,
    page: AuditPage,
}
#[derive(Clone)]
enum AuditPage {
    Empty,
    Branches { start: u64, page: Arc<BranchPage> },
    Parents(Arc<ParentPage>),
}
#[derive(Clone, Copy)]
enum Read {
    Branches { after: Option<BranchId>, start: u64 },
    Parents { offset: u64 },
}
#[derive(Clone)]
struct Target {
    epoch: u64,
    generation: u64,
    summary: Arc<Summary>,
}
impl Target {
    fn matches(&self, state: &State) -> bool {
        state.session_active()
            && state.session_epoch == self.epoch
            && state.files.deletion.accepts(self.generation)
            && state
                .files
                .deletion
                .node
                .as_ref()
                .and_then(Inspection::paged)
                .is_some_and(|model| Arc::ptr_eq(&model.summary, &self.summary))
    }
    fn current(&self, state: &State) -> bool {
        self.matches(state) && !state.files.deletion.loading
    }
    fn begin(&self, state: &mut State) -> Option<Self> {
        if !self.current(state) {
            return None;
        }
        let model = &mut state.files.deletion;
        model.cancel_review();
        model.generation += 1;
        model.loading = true;
        Some(Self {
            epoch: self.epoch,
            generation: model.generation,
            summary: self.summary.clone(),
        })
    }
}

fn invalid(message: &str) -> api::Error {
    decode_error(<json_codec::Error as data_serialization::de::Error>::custom(message))
}
fn decode_error(source: json_codec::Error) -> api::Error {
    api::Error::invalid_response(
        "The deletion audit response did not match the selected file and deletion.",
        api::ErrorSource::FileDeletion(source),
    )
}
fn summary_endpoint(summary: &Summary) -> String {
    format!(
        "/api/files/{}/deletion/summary?deletion_node_id={}",
        summary.file_id, summary.node_id,
    )
}
fn decode(value: Value, expected: &wire::Summary) -> Result<Summary, api::Error> {
    let summary = Summary::deserialize(value).map_err(decode_error)?;
    if summary.file_id.to_string() != expected.file_id.as_str()
        || summary.node_id.to_string() != expected.deletion_node_id
        || summary.repository_id.to_string() != expected.repository_id.as_str()
        || summary.path != expected.path
    {
        return Err(invalid(
            "the deletion summary belongs to another selected identity",
        ));
    }
    Ok(summary)
}
fn unchanged(value: Value, expected: &Summary) -> Result<Summary, api::Error> {
    let summary = Summary::deserialize(value).map_err(decode_error)?;
    if &summary != expected {
        return Err(invalid(
            "the deletion summary changed during restoration review",
        ));
    }
    Ok(summary)
}

pub(super) fn digest(summary: &Summary, principal: &str) -> Result<String, api::Error> {
    #[derive(data_serialization::Serialize)]
    struct Input {
        file_id: storage_contract::FileId,
        deletion_node_id: storage_contract::FileDeletionId,
    }
    crate::files::deletion::request::native_digest(
        "file.delete.undo",
        principal,
        Input {
            file_id: summary.file_id,
            deletion_node_id: summary.node_id,
        },
    )
}

pub(super) fn receipt_matches(id: &str, summary: &Summary, receipt: &Value) -> bool {
    receipt["operation_id"].as_str() == Some(id)
        && receipt["operation_kind"] == "file.delete.undo"
        && storage_contract::client_protocol::operation_status(receipt)
            == Some(storage_contract::client_protocol::OperationStatus::Published)
        && wire::publication_sequence(
            receipt,
            &["restored_file_id", "removed_deletion_summary"],
        )
        .is_some_and(|sequence| sequence > summary.sequence.get())
        && receipt["result"]["restored_file_id"].as_str()
            == Some(summary.file_id.to_string().as_str())
        && matches!(
            Summary::deserialize(receipt["result"]["removed_deletion_summary"].clone()),
            Ok(removed) if &removed == summary
        )
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/deletion/summary_tests.rs"]
mod tests;
