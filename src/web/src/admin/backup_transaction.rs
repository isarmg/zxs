//! One original task survives navigation; reads never advance native decisions.
mod actions;
mod recovery;
mod view;
use crate::{api, ui::State};
use storage_contract::{OperationId, backup_transaction as wire};
pub(super) use view::view;
use web_interface::{i18n::t, prelude::*};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Approval {
    Prepare,
    Resume,
    Fence,
    Decide,
    Activate,
}

#[derive(Clone, Default)]
pub(super) struct Model {
    context: Option<wire::Context>,
    source: String,
    targets: Vec<wire::Target>,
    operation_text: String,
    original_start: Option<wire::StartRequest>,
    retained_operation: Option<OperationId>,
    retained_request: Option<wire::OriginalRequest>,
    snapshot: Option<wire::Snapshot>,
    busy: bool,
    unknown: bool,
    checked_current: bool,
    approval: Option<Approval>,
    error: Option<api::Error>,
    notice: String,
}
impl Model {
    pub(super) fn reload_blocked(&self) -> bool {
        self.busy || self.unknown
    }
    fn operation(&self) -> Option<OperationId> {
        self.original_start
            .as_ref()
            .map(|request| request.operation_id)
            .or_else(|| {
                self.snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.request.operation_id)
            })
            .or(self.retained_operation)
            .or_else(|| {
                self.retained_request
                    .as_ref()
                    .map(|request| request.operation_id)
            })
    }
    fn editable(&self) -> bool {
        !self.busy && self.operation().is_none() && !self.unknown
    }
    fn completed(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.completion.is_some())
    }
    fn can_start(&self) -> bool {
        self.editable()
            && self.checked_current
            && self.context.is_some()
            && self.approval == Some(Approval::Prepare)
            && !self.source.is_empty()
            && (1..=8).contains(&self.targets.len())
            && self
                .targets
                .iter()
                .all(|target| !target.slot_id.is_empty() && !target.upstream_slot_id.is_empty())
    }
    fn accepts_request(&self, request: &wire::OriginalRequest) -> bool {
        self.operation().is_none_or(|id| request.operation_id == id)
            && self
                .original_start
                .as_ref()
                .is_none_or(|original| original_request(original) == *request)
            && self
                .retained_request
                .as_ref()
                .is_none_or(|original| *original == *request)
            && self
                .snapshot
                .as_ref()
                .is_none_or(|previous| previous.request == *request)
    }
    fn accept(&mut self, snapshot: wire::Snapshot) -> Result<(), api::Error> {
        validate(&snapshot)?;
        if !self.accepts_request(&snapshot.request) {
            return Err(invalid());
        }
        self.operation_text = snapshot.request.operation_id.to_string();
        self.unknown = false;
        self.error = None;
        if self.snapshot.as_ref() != Some(&snapshot) {
            self.snapshot = Some(snapshot);
        }
        Ok(())
    }
    /// Returns true only when the server observed no retained original input.
    fn accept_current(&mut self, current: wire::Current) -> Result<bool, api::Error> {
        match (current.request, current.transaction) {
            (Some(request), Some(snapshot)) if request == snapshot.request => {
                self.accept(snapshot)?;
                Ok(false)
            }
            (Some(request), None) => {
                validate_request(&request)?;
                if !self.accepts_request(&request) {
                    return Err(invalid());
                }
                self.operation_text = request.operation_id.to_string();
                self.retained_request = Some(request);
                self.unknown = true;
                self.approval = None;
                self.error = None;
                self.notice = t(
                    "原输入已持久保留，但没有活动进度；仅明确恢复该原任务，不创建替代任务。",
                    "The original input is retained without active progress. Explicitly resume that task; do not create a replacement.",
                );
                Ok(false)
            }
            (None, None) if self.operation().is_none() => Ok(true),
            _ => Err(invalid()),
        }
    }
}

fn original_request(request: &wire::StartRequest) -> wire::OriginalRequest {
    wire::OriginalRequest {
        operation_id: request.operation_id,
        source_slot_id: request.source_slot_id.clone(),
        expected_sequence: request.expected_sequence,
        targets: request
            .targets
            .iter()
            .map(|target| wire::OriginalTarget {
                root_path: target.selection.root_path.clone(),
                disk_id: target.disk_id.clone(),
                slot_id: target.slot_id.clone(),
                upstream_slot_id: target.upstream_slot_id.clone(),
            })
            .collect(),
    }
}

fn invalid() -> api::Error {
    let mut error = api::error(
        storage_contract::ErrorCode::InvalidAdminResponse,
        t(
            "备份响应未确认同一原任务与完成条件；请只查询原操作。",
            "The backup response did not confirm the original task and completion conditions. Query that original operation.",
        ),
    );
    error.status = 200;
    error
}

fn input_operation(model: &Model) -> Result<OperationId, api::Error> {
    if let Some(operation) = model.operation() {
        return Ok(operation);
    }
    model.operation_text.parse().map_err(|source| {
        let mut error = api::error(
            storage_contract::ErrorCode::InvalidRequest,
            t(
                "请输入准确原操作 ID。",
                "Enter the exact original operation ID.",
            ),
        );
        error.source = Some(std::sync::Arc::new(api::ErrorSource::BackupInput(source)));
        error
    })
}

fn decode<T: data_serialization::de::DeserializeOwned>(
    value: json_codec::Value,
) -> Result<T, api::Error> {
    json_codec::from_value(value).map_err(|source| {
        let mut error = invalid();
        error.source = Some(std::sync::Arc::new(api::ErrorSource::BackupTransaction(
            source,
        )));
        error
    })
}

fn validate(snapshot: &wire::Snapshot) -> Result<(), api::Error> {
    let request = &snapshot.request;
    validate_request(request)?;
    match (&snapshot.completion, snapshot.progress.observation) {
        (Some(receipt), wire::Observation::ServingActive)
            if receipt.operation_id == request.operation_id
                && receipt.kind == "backup-target-replacement"
                && matches!(
                    receipt.commit_state,
                    storage_contract::CommitState::Published | storage_contract::CommitState::Acked
                )
                && snapshot.progress.error_code.is_none() =>
        {
            Ok(())
        }
        (
            None,
            wire::Observation::Preparing
            | wire::Observation::Blocked
            | wire::Observation::SourceChanged,
        ) => Ok(()),
        _ => Err(invalid()),
    }
}

fn validate_request(request: &wire::OriginalRequest) -> Result<(), api::Error> {
    if !(1..=8).contains(&request.targets.len())
        || request.source_slot_id.is_empty()
        || request.targets.iter().any(|target| {
            target.root_path.is_empty()
                || target.disk_id.is_empty()
                || target.slot_id.is_empty()
                || target.upstream_slot_id.is_empty()
        })
    {
        return Err(invalid());
    }
    Ok(())
}

fn available(state: &State) -> bool {
    state.session_active() && !state.admin_console.backup.busy
}
fn editable(state: &State) -> bool {
    state.session_active() && state.admin_console.backup.editable()
}

pub(super) fn poll(state: RwSignal<State>) {
    if state.try_with_untracked(|s| {
        available(s)
            && s.page == crate::ui::Page::Admin
            && s.admin_console.backup.operation().is_some()
            && !s.admin_console.backup.unknown
            && !s.admin_console.backup.completed()
    }) == Some(true)
    {
        actions::read_original(state);
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/admin/backup_transaction.rs"]
mod tests;
