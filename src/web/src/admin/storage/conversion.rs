//! An ordinary directory is copied into a separately reviewed new primary.
//! Registration is not import completion; uncertainty retains the exact review.
use super::*;
use storage_contract::{
    CommitState, OperationId,
    storage_addition::{AdditionAuthority, AdditionState},
    storage_conversion::*,
    storage_directories::{DirectorySource, DiscoveredDiskState},
};

mod view;
pub(super) use view::{form, view};

#[derive(Clone)]
pub(super) struct Model {
    target: Option<String>,
    slot: String,
    metadata: bool,
    preview: Option<OrdinaryDiskConversionPreview>,
    status: Option<OrdinaryDiskConversionStatus>,
    submitted: bool,
    sending: bool,
    reading: bool,
    rejected: bool,
    checked: bool,
    generation: u64,
    acknowledged: bool,
    stop_acknowledged: bool,
    stop_requested: bool,
    error: Option<api::Error>,
}
impl Default for Model {
    fn default() -> Self {
        Self {
            target: None,
            slot: "N01".into(),
            metadata: false,
            preview: None,
            status: None,
            submitted: false,
            sending: false,
            reading: false,
            rejected: false,
            checked: false,
            generation: 0,
            acknowledged: false,
            stop_acknowledged: false,
            stop_requested: false,
            error: None,
        }
    }
}
impl Model {
    pub(super) fn blocks_edit(&self) -> bool {
        self.sending || self.reading || self.submitted
    }
    pub(super) fn invalidate(&mut self) {
        if !self.submitted {
            self.preview = None;
            self.acknowledged = false;
            self.stop_acknowledged = false;
            self.stop_requested = false;
        }
    }
}
fn active(model: &super::Model) -> bool {
    model
        .snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.state == StorageState::Active)
}
fn source(model: &super::Model) -> Option<DirectorySource> {
    match &model.wizard.chosen()?.state {
        DiscoveredDiskState::Unsupported { source, .. } => source.clone(),
        _ => None,
    }
}
fn targets(model: &super::Model) -> Vec<CandidateSelection> {
    model
        .wizard
        .candidates()
        .into_iter()
        .filter_map(|candidate| match candidate.state {
            DiscoveredDiskState::Empty { selection } => Some(selection),
            _ => None,
        })
        .collect()
}
fn make_request(
    model: &super::Model,
    operation_id: OperationId,
) -> Result<OrdinaryDiskConversionRequest, api::Error> {
    let invalid_input = || {
        api::error(
            storage_contract::ErrorCode::ConfigInvalid,
            t(
                "请选择已观察的来源和独立的空目标，并填写有效的新槽位。",
                "Choose an observed source, a separate empty target and a valid new slot.",
            ),
        )
    };
    if !active(model) {
        return Err(invalid_input());
    }
    let source = source(model).ok_or_else(invalid_input)?;
    let target = targets(model)
        .into_iter()
        .find(|selection| Some(&selection.root_path) == model.conversion.target.as_ref())
        .ok_or_else(invalid_input)?;
    if source.path == target.root_path
        || name_policy::validate_slot(&model.conversion.slot).is_err()
    {
        return Err(invalid_input());
    }
    Ok(OrdinaryDiskConversionRequest {
        operation_id,
        source,
        target,
        slot_id: model.conversion.slot.clone(),
        metadata_member: model.conversion.metadata,
    })
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn validate_preview(preview: &OrdinaryDiskConversionPreview) -> Result<(), api::Error> {
    addition::validate_preview(&preview.target)?;
    let [target] = preview.target.request.additions.as_slice() else {
        return Err(invalid(ResponseError::Identity));
    };
    if target.level != 1
        || target.upstream_slot_id.is_some()
        || preview.target.authority != AdditionAuthority::ActiveMembership
        || name_policy::validate_slot(&target.slot_id).is_err()
        || preview.source.path == target.selection.root_path
        || preview.operation_id == preview.import_operation_id
        || preview.operation_id == preview.target.request.operation_id
        || preview.import_operation_id == preview.target.request.operation_id
        || !digest(&preview.preview_digest)
        || !digest(&preview.source_preview.digest)
    {
        return Err(invalid(ResponseError::Identity));
    }
    Ok(())
}
fn read_preview(
    value: Value,
    expected: &OrdinaryDiskConversionRequest,
) -> Result<OrdinaryDiskConversionPreview, api::Error> {
    let preview: OrdinaryDiskConversionPreview = decode(value)?;
    validate_preview(&preview)?;
    let target = &preview.target.request.additions[0];
    if preview.operation_id != expected.operation_id
        || preview.source != expected.source
        || target.selection != expected.target
        || target.slot_id != expected.slot_id
        || target.metadata_member != expected.metadata_member
    {
        return Err(invalid(ResponseError::Identity));
    }
    Ok(preview)
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct StopEvidence {
    #[serialization(rename = "timestamp", with = "storage_contract::u64_string")]
    _timestamp: u64,
    #[serialization(rename = "membership_epoch", with = "storage_contract::u64_string")]
    _membership_epoch: u64,
    // A stop publishes no file versions. Keep the native envelope strict.
    #[serialization(rename = "version_protection")]
    _version_protection: [(); 0],
    import_operation_id: OperationId,
    target: StopTarget,
    #[serialization(deserialize_with = "required_nullable")]
    root_operation_id: Option<OperationId>,
    #[serialization(deserialize_with = "required_nullable")]
    workspace_id: Option<String>,
    workspace_retention: WorkspaceRetention,
    source_preserved: bool,
    target_preserved: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct StopTarget {
    pool_id: String,
    disk_id: String,
    slot_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    binding_generation: u64,
}
#[derive(Deserialize, PartialEq, Eq)]
#[serialization(rename_all = "snake_case")]
enum WorkspaceRetention {
    Aborted,
    NotCreated,
}
fn required_nullable<'de, T: Deserialize<'de>, D: data_serialization::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}
fn validate_stop_evidence(status: &OrdinaryDiskConversionStatus) -> Result<(), api::Error> {
    if let Some(result) = &status.stop_result {
        let evidence: StopEvidence = decode(result.result.clone())?;
        if evidence.import_operation_id != status.preview.import_operation_id
            || evidence.root_operation_id != status.root_operation_id
            || evidence.workspace_id != status.workspace_id
            || evidence.target.pool_id != status.preview.target.pool_id
            || evidence.target.slot_id != status.preview.target.request.additions[0].slot_id
            || evidence.target.binding_generation != 1
            || storage_contract::validate_id(&evidence.target.disk_id, "disk").is_err()
            || !evidence.source_preserved
            || !evidence.target_preserved
            || (evidence.workspace_retention == WorkspaceRetention::Aborted
                && evidence.workspace_id.is_none())
        {
            return Err(invalid(ResponseError::Identity));
        }
    }
    Ok(())
}
fn validate_status(
    status: &OrdinaryDiskConversionStatus,
    expected: Option<&OrdinaryDiskConversionPreview>,
    expected_stop: Option<OperationId>,
) -> Result<(), api::Error> {
    validate_preview(&status.preview)?;
    if expected.is_some_and(|preview| preview != &status.preview) {
        return Err(invalid(ResponseError::Identity));
    }
    if let Some(registration) = &status.registration {
        addition::validate_status(
            registration,
            Some(status.preview.target.request.operation_id),
        )?;
        if registration.preview != status.preview.target {
            return Err(invalid(ResponseError::Identity));
        }
    }
    validate_stop_evidence(status)?;
    let completed = status.phase == OrdinaryDiskConversionPhase::Completed;
    let stopped = status.phase == OrdinaryDiskConversionPhase::Stopped;
    if expected_stop.is_some_and(|id| status.stop_operation_id != Some(id))
        || status.stop_operation_id.is_some_and(|id| {
            id == status.preview.operation_id
                || id == status.preview.import_operation_id
                || id == status.preview.target.request.operation_id
                || Some(id) == status.root_operation_id
        })
        || (matches!(
            status.phase,
            OrdinaryDiskConversionPhase::Stopping | OrdinaryDiskConversionPhase::Stopped
        ) && status.stop_operation_id.is_none())
        || (matches!(
            status.phase,
            OrdinaryDiskConversionPhase::Registering | OrdinaryDiskConversionPhase::Importing
        ) && status.stop_operation_id.is_some())
        || stopped != status.stop_result.is_some()
        || (stopped && status.failure.is_some())
        || status.stop_result.as_ref().is_some_and(|result| {
            Some(result.operation_id) != status.stop_operation_id
                || result.kind != "directory.import.stop"
                || !matches!(
                    result.commit_state,
                    CommitState::Published | CommitState::PublishedDegraded | CommitState::Acked
                )
                || !digest(&result.request_digest)
        })
    {
        return Err(invalid(ResponseError::Identity));
    }
    if completed != status.result.is_some()
        || (completed && (status.failure.is_some() || status.workspace_id.is_none()))
        || (matches!(
            status.phase,
            OrdinaryDiskConversionPhase::Importing
                | OrdinaryDiskConversionPhase::Completed
                | OrdinaryDiskConversionPhase::Stopping
                | OrdinaryDiskConversionPhase::Stopped
        ) && !status
            .registration
            .as_ref()
            .is_some_and(|record| record.state == AdditionState::Completed))
        || status
            .workspace_id
            .as_ref()
            .is_some_and(|id| storage_contract::validate_id(id, "ws").is_err())
        || status.result.as_ref().is_some_and(|result| {
            result.operation_id != status.preview.import_operation_id
                || result.kind != "workspace.commit"
                || !digest(&result.request_digest)
                || !matches!(
                    result.commit_state,
                    CommitState::Published | CommitState::Acked | CommitState::PublishedDegraded
                )
        })
    {
        return Err(invalid(ResponseError::Identity));
    }
    Ok(())
}
fn preview(state: RwSignal<State>) {
    let Some(id) = crate::ui::operation_id(state) else {
        return;
    };
    let operation = id
        .parse()
        .expect("invariant: browser operation generator produces canonical IDs");
    let captured = state
        .try_update(|s| {
            if !s.session_active() || !s.admin_console.storage.editable() {
                return None;
            }
            let storage = &mut s.admin_console.storage;
            let request = match make_request(storage, operation) {
                Ok(request) => request,
                Err(error) => {
                    storage.conversion.error = Some(error);
                    return None;
                }
            };
            storage.foreign.invalidate();
            let model = &mut storage.conversion;
            model.sending = true;
            model.error = None;
            model.invalidate();
            model.generation += 1;
            Some((s.session_epoch, model.generation, request))
        })
        .flatten();
    let Some((epoch, generation, request)) = captured else {
        return;
    };
    spawn_local(async move {
        let result = api::validated_request(
            "POST",
            "/api/admin/storage/conversion/preview",
            Some(json!(request)),
            |value| read_preview(value, &request),
        )
        .await;
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        state.update(|s| {
            if s.session_epoch != epoch
                || s.admin_console.storage.conversion.generation != generation
            {
                return;
            }
            let model = &mut s.admin_console.storage.conversion;
            model.sending = false;
            match result {
                Ok(preview) => {
                    model.preview = Some(preview);
                    model.status = None;
                    model.checked = false;
                    model.rejected = false;
                    model.stop_requested = false;
                    model.stop_acknowledged = false;
                }
                Err(error) => model.error = Some(error),
            }
        });
    });
}
fn terminal(phase: OrdinaryDiskConversionPhase) -> bool {
    matches!(
        phase,
        OrdinaryDiskConversionPhase::Completed | OrdinaryDiskConversionPhase::Stopped
    )
}
fn resumable(model: &Model) -> bool {
    !model.sending
        && !model.reading
        && model.submitted
        && model.checked
        && !model.stop_requested
        && model.status.as_ref().is_none_or(|status| {
            status.phase == OrdinaryDiskConversionPhase::RecoveryRequired
                && status.workspace_id.is_none()
                && status.root_operation_id.is_none()
        })
}
fn stoppable(model: &Model) -> bool {
    model.submitted
        && model.checked
        && !model.sending
        && !model.reading
        && model.status.as_ref().is_some_and(|status| {
            matches!(
                status.phase,
                OrdinaryDiskConversionPhase::RecoveryRequired
                    | OrdinaryDiskConversionPhase::Stopping
            ) && status
                .registration
                .as_ref()
                .is_some_and(|registration| registration.state == AdditionState::Completed)
        })
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Submit {
    Commit,
    Resume,
    Stop,
}
fn prepare_submit(
    storage: &mut super::Model,
    action: Submit,
) -> Option<(OrdinaryDiskConversionPreview, &'static str, Value)> {
    match action {
        Submit::Commit if !storage.editable() || !storage.conversion.acknowledged => return None,
        Submit::Resume if !resumable(&storage.conversion) => return None,
        Submit::Stop
            if !stoppable(&storage.conversion) || !storage.conversion.stop_acknowledged =>
        {
            return None;
        }
        _ => {}
    }
    let model = &mut storage.conversion;
    let preview = model.preview.clone()?;
    let (path, body) = match action {
        Submit::Stop => (
            "/api/admin/storage/conversion/stop",
            json!(OrdinaryDiskConversionStop {
                operation_id: preview.operation_id,
                acknowledge_retained_state: true,
            }),
        ),
        Submit::Resume if model.status.is_some() => (
            "/api/admin/storage/conversion/resume",
            json!(OrdinaryDiskConversionResume {
                operation_id: preview.operation_id,
            }),
        ),
        Submit::Commit | Submit::Resume => (
            "/api/admin/storage/conversion/commit",
            json!(OrdinaryDiskConversionCommit {
                preview: preview.clone(),
                acknowledge_contents_only: true,
            }),
        ),
    };
    model.sending = true;
    model.submitted = true;
    model.checked = false;
    model.rejected = false;
    model.error = None;
    if action == Submit::Stop {
        model.stop_requested = true;
        model.stop_acknowledged = false;
    }
    model.generation += 1;
    Some((preview, path, body))
}
fn submit(state: RwSignal<State>, action: Submit) {
    let captured = state
        .try_update(|s| {
            if !s.session_active() {
                return None;
            }
            let storage = &mut s.admin_console.storage;
            let (preview, path, body) = prepare_submit(storage, action)?;
            storage.foreign.invalidate();
            Some((
                s.session_epoch,
                storage.conversion.generation,
                preview,
                path,
                body,
                storage
                    .conversion
                    .status
                    .as_ref()
                    .and_then(|status| status.stop_operation_id),
            ))
        })
        .flatten();
    let Some((epoch, generation, preview, path, body, expected_stop)) = captured else {
        return;
    };
    spawn_local(async move {
        let result = api::validated_request("POST", path, Some(body), |value| {
            let status = decode(value)?;
            validate_status(&status, Some(&preview), expected_stop)?;
            Ok(status)
        })
        .await;
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        let settled = state.try_update(|s| {
            if s.session_epoch != epoch
                || s.admin_console.storage.conversion.generation != generation
            {
                return false;
            }
            let model = &mut s.admin_console.storage.conversion;
            model.sending = false;
            match result {
                Ok(status) => accept(model, status),
                Err(error) => {
                    model.rejected = action == Submit::Commit && !error.mutation_outcome_unknown();
                    model.error = Some(error);
                    false
                }
            }
        }) == Some(true);
        if settled {
            refresh_terminal(state);
        } else {
            load(state);
        }
    });
}
fn accept(model: &mut Model, status: OrdinaryDiskConversionStatus) -> bool {
    // A concurrent read cannot regress a proven terminal receipt.
    if model.status.as_ref().is_some_and(|old| {
        old.preview.operation_id == status.preview.operation_id && terminal(old.phase)
    }) {
        return false;
    }
    let settled = terminal(status.phase);
    if model.status.as_ref() != Some(&status) {
        model.stop_acknowledged = false;
    }
    model.stop_requested |= status.stop_operation_id.is_some();
    model.preview = Some(status.preview.clone());
    model.status = Some(status);
    model.submitted = !settled;
    model.error = None;
    settled
}
fn finish_read(
    model: &mut Model,
    response: Result<OrdinaryDiskConversionStatusResponse, api::Error>,
) -> bool {
    model.reading = false;
    if model.preview.is_some() && !model.submitted && model.status.is_none() {
        return false;
    }
    match response {
        Ok(response) => {
            model.checked = true;
            if let Some(status) = response.operation {
                return accept(model, status);
            }
            if model.rejected && !model.sending && !model.stop_requested {
                model.submitted = false;
                model.status = None;
                model.invalidate();
            }
        }
        Err(error) => {
            if error.code == storage_contract::ErrorCode::OperationNotFound {
                model.checked = true;
                if model.rejected && !model.sending && !model.stop_requested {
                    model.submitted = false;
                    model.status = None;
                    model.invalidate();
                }
            } else {
                model.error = Some(error);
            }
        }
    }
    false
}
pub(super) fn load(state: RwSignal<State>) {
    let captured = state
        .try_update(|s| {
            if !s.session_active() {
                return None;
            }
            let model = &mut s.admin_console.storage.conversion;
            if model.reading {
                return None;
            }
            model.reading = true;
            Some((
                s.session_epoch,
                model.generation,
                (model.submitted || model.status.is_some())
                    .then(|| model.preview.clone())
                    .flatten(),
                model
                    .status
                    .as_ref()
                    .and_then(|status| status.stop_operation_id),
            ))
        })
        .flatten();
    let Some((epoch, generation, expected, expected_stop)) = captured else {
        return;
    };
    spawn_local(async move {
        let path = expected.as_ref().map_or_else(
            || "/api/admin/storage/conversion/status".into(),
            |preview| {
                format!(
                    "/api/admin/storage/conversion/status?operation_id={}",
                    preview.operation_id
                )
            },
        );
        let result = api::validated_request("GET", &path, None, |value| {
            let response: OrdinaryDiskConversionStatusResponse = decode(value)?;
            if let Some(status) = &response.operation {
                validate_status(status, expected.as_ref(), expected_stop)?;
            }
            Ok(response)
        })
        .await;
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        let settled = state.try_update(|s| {
            if s.session_epoch != epoch {
                return false;
            }
            let model = &mut s.admin_console.storage.conversion;
            model.reading = false;
            if model.generation != generation {
                return false;
            }
            finish_read(model, result)
        }) == Some(true);
        if settled {
            refresh_terminal(state);
        }
    });
}
fn refresh_terminal(state: RwSignal<State>) {
    state.update(|s| {
        s.admin_console.storage.conversion.acknowledged = false;
        s.admin_console.storage.conversion.stop_acknowledged = false;
        s.admin_console.storage.wizard.clear_choice();
        s.files.invalidate_disks();
    });
    super::super::disks::load(state);
    super::load(state);
}
pub(super) fn poll(state: RwSignal<State>) {
    if state.with_untracked(|s| {
        let model = &s.admin_console.storage.conversion;
        s.session_active()
            && model.submitted
            && !model.reading
            && (model.sending
                || model.stop_requested
                || model.status.as_ref().is_none_or(|status| {
                    matches!(
                        status.phase,
                        OrdinaryDiskConversionPhase::Registering
                            | OrdinaryDiskConversionPhase::Importing
                    )
                }))
    }) {
        load(state);
    }
}
fn edit(state: RwSignal<State>, update: impl FnOnce(&mut Model)) {
    state.update(|s| {
        if s.session_active() && s.admin_console.storage.editable() {
            let model = &mut s.admin_console.storage.conversion;
            update(model);
            model.invalidate();
        }
    });
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/admin/storage_conversion_tests.rs"]
mod tests;
