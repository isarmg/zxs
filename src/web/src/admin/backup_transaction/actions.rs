use super::*;
use crate::ui::{accept_read_response, current_session, fail};
use storage_contract::storage_directories::{CandidateSelection, DiscoveredDiskState};
use web_interface::tasks::spawn_local;

pub(super) fn load(state: RwSignal<State>) {
    if !state.with_untracked(available) {
        return;
    }
    if state.with_untracked(|s| s.admin_console.backup.operation().is_none()) {
        match state.with_untracked(|s| recovery::load(&s.principal)) {
            Ok(Some(record)) => state.update(|s| {
                s.admin_console.backup.retained_operation = Some(record.operation_id);
                s.admin_console.backup.operation_text = record.operation_id.to_string();
                s.admin_console.backup.original_start = record.original_start;
                s.admin_console.backup.unknown = true;
            }),
            Ok(None) => {}
            Err(error) => {
                state.update(|s| s.admin_console.backup.error = Some(error));
                return;
            }
        }
    }
    if state.with_untracked(|s| s.admin_console.backup.operation().is_some()) {
        read_original(state);
        return;
    }
    let epoch = state.with_untracked(|s| s.session_epoch);
    state.update(|s| {
        s.admin_console.backup.busy = true;
        s.admin_console.backup.approval = None;
        s.admin_console.backup.checked_current = false;
    });
    spawn_local(async move {
        let result = api::validated_request(
            "GET",
            "/api/admin/backup-transactions",
            None,
            decode::<wire::Current>,
        )
        .await;
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        let Some(current) = state.try_update(|s| {
            result.and_then(|current| s.admin_console.backup.accept_current(current))
        }) else {
            return;
        };
        match current {
            Ok(false) => state.update(|s| s.admin_console.backup.busy = false),
            Ok(true) => {
                let context = api::validated_request(
                    "GET",
                    "/api/admin/backup-transactions/context",
                    None,
                    decode::<wire::Context>,
                )
                .await;
                if !accept_read_response(state, epoch, &context) {
                    return;
                }
                state.update(|s| {
                    let model = &mut s.admin_console.backup;
                    model.busy = false;
                    match context {
                        Ok(context) => {
                            model.context = Some(context); model.targets.clear(); model.approval = None;
                            model.checked_current = true; model.error = None;
                            model.notice = t("未观察到保留的原输入；新建仍由核心检查原输入与全部前提。", "No retained original input was observed. New creation still requires the core's original-input and full admission checks.");
                        }
                        Err(error) => model.error = Some(error),
                    }
                });
            }
            Err(error) => finish(state, epoch, false, Err(error)),
        }
    });
}

pub(super) fn read_original(state: RwSignal<State>) {
    if !state.with_untracked(available) {
        return;
    }
    let operation = match state.with_untracked(|s| input_operation(&s.admin_console.backup)) {
        Ok(operation) => operation,
        Err(error) => {
            state.update(|s| s.admin_console.backup.error = Some(error));
            return;
        }
    };
    request(state, Request::Read(operation));
}

pub(super) fn add_target(state: RwSignal<State>, selection: CandidateSelection) {
    if !state.with_untracked(editable) {
        return;
    }
    let id = match api::operation_id() {
        Ok(id) => format!(
            "disk_{}",
            id.strip_prefix("op_")
                .expect("invariant: browser operation IDs carry the fixed prefix")
        ),
        Err(error) => {
            fail(state, error);
            return;
        }
    };
    state.update(|s| {
        let model = &mut s.admin_console.backup;
        if model.targets.len() == 8
            || model
                .targets
                .iter()
                .any(|target| target.selection.root_path == selection.root_path)
        {
            return;
        }
        model.targets.push(wire::Target {
            selection,
            disk_id: id,
            slot_id: String::new(),
            upstream_slot_id: model.source.clone(),
        });
        model.approval = None;
    });
}

pub(super) fn candidates(model: &Model) -> Vec<CandidateSelection> {
    model
        .context
        .as_ref()
        .map(|context| {
            context
                .discovery
                .candidates
                .iter()
                .filter_map(|candidate| match &candidate.state {
                    DiscoveredDiskState::Empty { selection } => Some(selection.clone()),
                    DiscoveredDiskState::Zxs { .. }
                    | DiscoveredDiskState::Unsupported { .. }
                    | DiscoveredDiskState::Unavailable { .. } => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn start(state: RwSignal<State>) {
    if !state.with_untracked(|s| s.session_active() && s.admin_console.backup.can_start()) {
        return;
    }
    let operation = match api::operation_id() {
        Ok(id) => id
            .parse()
            .expect("invariant: the browser generated a canonical operation ID"),
        Err(error) => {
            fail(state, error);
            return;
        }
    };
    let original = state.with_untracked(|s| {
        let model = &s.admin_console.backup;
        wire::StartRequest {
            operation_id: operation,
            source_slot_id: model.source.clone(),
            expected_sequence: model
                .context
                .as_ref()
                .expect("invariant: start admitted a context")
                .sequence,
            targets: model.targets.clone(),
            confirmation: wire::PreparationConfirmation::PrepareIndependentCompleteCopies,
        }
    });
    state.update(|s| {
        s.admin_console.backup.operation_text = operation.to_string();
        s.admin_console.backup.original_start = Some(original.clone());
    });
    send_start(state, original);
}

fn send_start(state: RwSignal<State>, original: wire::StartRequest) {
    request(state, Request::Start(original));
}

pub(super) fn retry_start(state: RwSignal<State>) {
    if !state.with_untracked(|s| {
        available(s) && s.admin_console.backup.approval == Some(Approval::Prepare)
    }) {
        return;
    }
    if let Some(original) = state.with_untracked(|s| s.admin_console.backup.original_start.clone())
    {
        send_start(state, original);
    }
}

pub(super) fn advance(state: RwSignal<State>, command: wire::Command, approval: Option<Approval>) {
    if !state.with_untracked(|s| {
        available(s)
            && !s.admin_console.backup.unknown
            && s.admin_console.backup.snapshot.is_some()
            && !s.admin_console.backup.completed()
            && approval.is_none_or(|required| s.admin_console.backup.approval == Some(required))
    }) {
        return;
    }
    let operation = match state.with_untracked(|s| input_operation(&s.admin_console.backup)) {
        Ok(operation) => operation,
        Err(error) => {
            state.update(|s| s.admin_console.backup.error = Some(error));
            return;
        }
    };
    request(state, Request::Continue { operation, command });
}

pub(super) fn resume(state: RwSignal<State>) {
    if !state.with_untracked(|s| {
        available(s)
            && !s.admin_console.backup.completed()
            && s.admin_console.backup.approval == Some(Approval::Resume)
    }) {
        return;
    }
    let operation = match state.with_untracked(|s| input_operation(&s.admin_console.backup)) {
        Ok(operation) => operation,
        Err(error) => {
            state.update(|s| s.admin_console.backup.error = Some(error));
            return;
        }
    };
    let command = wire::Command::ResumeOriginalTask {
        confirmation: wire::ResumeConfirmation::RecoverOnlyTheOriginalRetainedTask,
    };
    request(state, Request::Continue { operation, command });
}

enum Request {
    Read(OperationId),
    Start(wire::StartRequest),
    Continue {
        operation: OperationId,
        command: wire::Command,
    },
}

fn request(state: RwSignal<State>, request: Request) {
    if !state.with_untracked(available) {
        return;
    }
    let mutation = !matches!(&request, Request::Read(_));
    let command_response = matches!(&request, Request::Continue { .. });
    let (operation, path, body) = match request {
        Request::Read(operation) => (
            operation,
            format!("/api/admin/backup-transactions/{operation}"),
            None,
        ),
        Request::Start(original) => (
            original.operation_id,
            "/api/admin/backup-transactions".into(),
            Some(
                json_codec::to_value(original).expect("invariant: typed backup request serializes"),
            ),
        ),
        Request::Continue { operation, command } => (
            operation,
            format!("/api/admin/backup-transactions/{operation}/continue"),
            Some(
                json_codec::to_value(command).expect("invariant: typed backup command serializes"),
            ),
        ),
    };
    let method = if mutation { "POST" } else { "GET" };
    if mutation {
        let saved = state.with_untracked(|s| {
            recovery::remember(
                &s.principal,
                operation,
                s.admin_console.backup.original_start.as_ref(),
            )
        });
        if let Err(error) = saved {
            state.update(|s| s.admin_console.backup.error = Some(error));
            return;
        }
    }
    let epoch = state.with_untracked(|s| s.session_epoch);
    state.update(|s| {
        let model = &mut s.admin_console.backup;
        model.busy = true;
        model.approval = None;
        model.error = None;
        if mutation {
            model.unknown = true;
            model.retained_operation = Some(operation);
        }
        if command_response {
            s.management.invalidate_storage();
        }
    });
    spawn_local(async move {
        let result = api::validated_request(method, &path, body, |value| {
            let response = if command_response {
                decode::<wire::CommandResponse>(value)?
            } else {
                wire::CommandResponse::Observed {
                    snapshot: decode(value)?,
                }
            };
            match &response {
                wire::CommandResponse::Observed { snapshot } => {
                    validate(snapshot)?;
                    if snapshot.request.operation_id != operation {
                        return Err(invalid());
                    }
                }
                wire::CommandResponse::RecoveryQueued { operation_id } => {
                    if *operation_id != operation {
                        return Err(invalid());
                    }
                }
            }
            Ok(response)
        })
        .await;
        match result {
            Ok(wire::CommandResponse::Observed { snapshot }) => {
                finish(state, epoch, mutation, Ok(snapshot))
            }
            Ok(wire::CommandResponse::RecoveryQueued { operation_id }) => {
                if !current_session(state, epoch) {
                    return;
                }
                state.update(|s| {
                    let model = &mut s.admin_console.backup;
                    model.busy = false; model.unknown = false; model.retained_operation = Some(operation_id);
                    model.operation_text = operation_id.to_string(); model.error = None;
                    model.notice = t("原任务恢复或切换后的服务准入已排入服务持有的恢复线程，尚未确认完成。后续仅查询，不自动重复恢复或激活。", "Original recovery or post-cutover service admission was queued in the service-owned worker, not confirmed complete. Follow-up only reads status; recovery and activation are not automatically repeated.");
                });
            }
            Err(error) => finish(state, epoch, mutation, Err(error)),
        }
    });
}

fn finish(
    state: RwSignal<State>,
    epoch: u64,
    mutation: bool,
    result: Result<wire::Snapshot, api::Error>,
) {
    if !current_session(state, epoch) {
        return;
    }
    let unauthorized = result
        .as_ref()
        .err()
        .filter(|error| error.status == 401)
        .cloned();
    state.update(|s| {
        let model = &mut s.admin_console.backup;
        model.busy = false;
        let accepted = result.and_then(|snapshot| model.accept(snapshot));
        if let Err(error) = accepted {
            if mutation {
                model.unknown = true;
            }
            model.error = Some(error);
        }
    });
    if let Some(error) = unauthorized {
        fail(state, error);
    }
}
