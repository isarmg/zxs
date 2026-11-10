use super::*;
use storage_contract::{
    OperationId,
    storage_lifecycle::*,
    storage_readmission::{RegistrationEntry, RegistrationMode},
};

#[derive(Clone, Default)]
pub(super) struct Model {
    preview: Option<ExitPreview>,
    status: Option<ExitStatus>,
    operation: Option<OperationId>,
    submitted: bool,
    rejected: bool,
    sending: bool,
    reading: Option<u64>,
    generation: u64,
    acknowledged: bool,
    error: Option<api::Error>,
}
impl Model {
    pub(super) fn blocks_edit(&self) -> bool {
        self.sending
            || self.reading.is_some()
            || self.submitted
            || self
                .status
                .as_ref()
                .is_some_and(|status| status.state != ExitState::Completed)
    }
    fn finish_read(&mut self, generation: u64) -> bool {
        if self.reading != Some(generation) {
            return false;
        }
        self.reading = None;
        self.generation == generation
    }
    fn leave_preview(&mut self) {
        if !self.submitted {
            self.generation += 1;
            self.preview = None;
            self.acknowledged = false;
            self.sending = false;
            self.reading = None;
            self.operation = self.status.as_ref().map(|status| status.operation_id);
        }
    }
}
pub(in crate::admin) fn can_preview(
    state: &State,
    slot: &str,
    disk: &str,
    action: ExitAction,
) -> bool {
    let storage = &state.admin_console.storage;
    state.session_active()
        && super::readmission::active_registration(state, slot, disk)
            .is_some_and(|entry| action_available(entry, action))
        && !storage.busy
        && !storage.readmission.blocks_edit()
        && !storage.addition.blocks_edit()
        && !storage.conversion.blocks_edit()
        && !storage.lifecycle.blocks_edit()
}
fn action_available(entry: &RegistrationEntry, action: ExitAction) -> bool {
    match action {
        ExitAction::PlannedOffline => entry.mode == RegistrationMode::Enabled,
        ExitAction::Deregister => true,
    }
}
fn preview_matches_registration(preview: &ExitPreview, entry: &RegistrationEntry) -> bool {
    action_available(entry, preview.request.action)
        && preview.target.slot_id == entry.slot_id
        && preview.target.disk_id == entry.disk_id
        && preview.target.binding_generation == entry.binding_generation
        && preview.original_metadata_members - preview.target_metadata_members
            == usize::from(entry.metadata_member)
}
fn can_submit_preview(state: &State) -> bool {
    let model = &state.admin_console.storage.lifecycle;
    model.acknowledged
        && model.preview.as_ref().is_some_and(|preview| {
            let target = &preview.target;
            can_preview(
                state,
                &target.slot_id,
                &target.disk_id,
                preview.request.action,
            ) && super::readmission::active_registration(state, &target.slot_id, &target.disk_id)
                .is_some_and(|entry| preview_matches_registration(preview, entry))
        })
}
pub(in crate::admin) fn preview(
    state: RwSignal<State>,
    slot_id: String,
    expected_disk_id: String,
    action: ExitAction,
) {
    let Some(operation) = crate::ui::operation_id(state) else {
        return;
    };
    let operation_id: OperationId = operation
        .parse()
        .expect("invariant: browser operation generator produces canonical IDs");
    let request = ExitRequest {
        operation_id,
        slot_id,
        expected_disk_id,
        action,
    };
    let captured = state
        .try_update(|s| {
            if !can_preview(
                s,
                &request.slot_id,
                &request.expected_disk_id,
                request.action,
            ) {
                return None;
            }
            let registration = super::readmission::active_registration(
                s,
                &request.slot_id,
                &request.expected_disk_id,
            )
            .expect("invariant: eligible exit has an exact active registration")
            .clone();
            let model = &mut s.admin_console.storage.lifecycle;
            model.sending = true;
            model.preview = None;
            model.acknowledged = false;
            model.error = None;
            model.generation += 1;
            Some((s.session_epoch, model.generation, registration))
        })
        .flatten();
    let Some((epoch, generation, registration)) = captured else {
        return;
    };
    spawn_local(async move {
        let expected = request.clone();
        let result = api::validated_request(
            "POST",
            "/api/admin/disks/exit/preview",
            Some(json!(request)),
            |value| {
                let preview: ExitPreview = decode(value)?;
                validate_preview(&preview)?;
                if preview.request != expected
                    || !preview_matches_registration(&preview, &registration)
                {
                    return Err(invalid(ResponseError::Identity));
                }
                Ok(preview)
            },
        )
        .await;
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        state.update(|s| {
            if s.session_epoch != epoch
                || s.admin_console.storage.lifecycle.generation != generation
            {
                return;
            }
            let same_registration = super::readmission::active_registration(
                s,
                &registration.slot_id,
                &registration.disk_id,
            ) == Some(&registration);
            let model = &mut s.admin_console.storage.lifecycle;
            model.sending = false;
            match result {
                Ok(preview) if same_registration => {
                    model.operation = Some(preview.request.operation_id);
                    model.preview = Some(preview);
                    model.status = None;
                    model.acknowledged = false;
                }
                Ok(_) => model.error = Some(invalid(ResponseError::Identity)),
                Err(error) => model.error = Some(error),
            }
        });
    });
}
fn submit(state: RwSignal<State>, resume: bool) {
    let captured = state
        .try_update(|s| {
            if !s.session_active() || s.admin_console.storage.conversion.blocks_edit() {
                return None;
            }
            let model = &s.admin_console.storage.lifecycle;
            if (!model.submitted && !can_submit_preview(s))
                || model.sending
                || model
                    .status
                    .as_ref()
                    .is_some_and(|status| status.state == ExitState::Completed)
            {
                return None;
            }
            let operation = model.operation?;
            let preview = model
                .preview
                .clone()
                .or_else(|| model.status.as_ref().map(|status| status.preview.clone()))?;
            let (path, body) = if resume && model.status.is_some() {
                (
                    "/api/admin/disks/exit/resume",
                    json!(ExitResume {
                        operation_id: operation
                    }),
                )
            } else {
                (
                    "/api/admin/disks/exit",
                    json!(ExitCommit {
                        preview: preview.clone()
                    }),
                )
            };
            let model = &mut s.admin_console.storage.lifecycle;
            model.sending = true;
            model.submitted = true;
            model.rejected = false;
            model.error = None;
            model.generation += 1;
            Some((
                s.session_epoch,
                model.generation,
                operation,
                preview,
                path,
                body,
            ))
        })
        .flatten();
    let Some((epoch, generation, operation, preview, path, body)) = captured else {
        return;
    };
    spawn_local(async move {
        let result = api::validated_request("POST", path, Some(body), |value| {
            let status: ExitStatus = decode(value)?;
            validate_status(&status, Some(operation))?;
            if status.preview != preview {
                return Err(invalid(ResponseError::Identity));
            }
            Ok(status)
        })
        .await;
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        let completed = state.try_update(|s| {
            if s.session_epoch != epoch
                || s.admin_console.storage.lifecycle.generation != generation
            {
                return false;
            }
            let model = &mut s.admin_console.storage.lifecycle;
            model.sending = false;
            match result {
                Ok(status) => accept(model, status),
                Err(error) => {
                    model.rejected = !error.mutation_outcome_unknown();
                    model.error = Some(error);
                    false
                }
            }
        }) == Some(true);
        if completed {
            refresh_after_exit(state);
        }
        load(state);
    });
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn validate_preview(preview: &ExitPreview) -> Result<(), api::Error> {
    if preview.request.expected_disk_id != preview.target.disk_id
        || preview.request.slot_id != preview.target.slot_id
        || storage_contract::validate_id(&preview.target.pool_id, "pool").is_err()
        || storage_contract::validate_id(&preview.target.disk_id, "disk").is_err()
        || preview.target.slot_id.is_empty()
        || preview.target.root_path.is_empty()
        || preview.target.binding_generation == 0
        || !digest(&preview.configuration_digest)
        || !digest(&preview.preview_digest)
        || !digest(&preview.snapshot_hash)
        || preview.original_metadata_members == 0
        || (preview.authority == ExitAuthority::TerminalManagement)
            != (preview.target_metadata_members == 0)
        || preview.target_metadata_members > preview.original_metadata_members
        || preview.original_metadata_members - preview.target_metadata_members > 1
    {
        return Err(invalid(ResponseError::Identity));
    }
    Ok(())
}
fn validate_status(status: &ExitStatus, expected: Option<OperationId>) -> Result<(), api::Error> {
    validate_preview(&status.preview)?;
    let completed = status.state == ExitState::Completed;
    if expected.is_some_and(|operation| operation != status.operation_id)
        || status.preview.request.operation_id != status.operation_id
        || completed != (status.phase == ExitPhase::Completed)
        || completed != status.result.is_some()
        || (completed && status.failure.is_some())
    {
        return Err(invalid(ResponseError::Identity));
    }
    if let Some(result) = &status.result {
        let target = &status.preview.target;
        let generation = target.binding_generation.to_string();
        let outcome = |key: &str| result.result.get(key).and_then(Value::as_str);
        let registration = match status.preview.request.action {
            ExitAction::PlannedOffline => "disabled",
            ExitAction::Deregister => "detached",
        };
        if result.operation_id != status.operation_id
            || result.kind != status.preview.request.action.result_kind()
            || result.commit_state != storage_contract::CommitState::Published
            || !digest(&result.request_digest)
            || outcome("disk_id") != Some(target.disk_id.as_str())
            || outcome("slot_id") != Some(target.slot_id.as_str())
            || outcome("binding_generation") != Some(generation.as_str())
            || outcome("registration") != Some(registration)
            || outcome("target_io") != Some("closed")
            || (status.preview.authority == ExitAuthority::TerminalManagement
                && outcome("authority_kind") != Some("terminal-management"))
        {
            return Err(invalid(ResponseError::Identity));
        }
    }
    Ok(())
}
fn accept(model: &mut Model, status: ExitStatus) -> bool {
    let completed = status.state == ExitState::Completed;
    let changed = completed
        && !model.status.as_ref().is_some_and(|old| {
            old.operation_id == status.operation_id && old.state == ExitState::Completed
        });
    model.operation = Some(status.operation_id);
    model.submitted = !completed;
    model.status = Some(status);
    if completed {
        model.preview = None;
        model.acknowledged = false;
        model.error = None;
    }
    changed
}
fn refresh_after_exit(state: RwSignal<State>) {
    state.update(|s| s.files.invalidate_disks());
    super::super::disks::load(state);
    crate::management::load(state);
    super::load(state);
}
pub(super) fn load(state: RwSignal<State>) {
    let captured = state
        .try_update(|s| {
            if !s.session_active() {
                return None;
            }
            let model = &mut s.admin_console.storage.lifecycle;
            if model.reading.is_some() {
                return None;
            }
            model.reading = Some(model.generation);
            Some((
                s.session_epoch,
                model.generation,
                model.submitted.then_some(model.operation).flatten(),
                model.submitted.then(|| model.preview.clone()).flatten(),
            ))
        })
        .flatten();
    let Some((epoch, generation, operation, expected)) = captured else {
        return;
    };
    spawn_local(async move {
        let path = operation.map_or_else(
            || "/api/admin/disks/exit".into(),
            |operation| format!("/api/admin/disks/exit?operation_id={operation}"),
        );
        let result = api::validated_request("GET", &path, None, |value| {
            let response: ExitStatusResponse = decode(value)?;
            if let Some(status) = &response.operation {
                validate_status(status, operation)?;
                if expected
                    .as_ref()
                    .is_some_and(|expected| expected != &status.preview)
                {
                    return Err(invalid(ResponseError::Identity));
                }
            }
            Ok(response)
        })
        .await;
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        let completed = state.try_update(|s| {
            if s.session_epoch != epoch {
                return false;
            }
            let model = &mut s.admin_console.storage.lifecycle;
            if !model.finish_read(generation) {
                return false;
            }
            match result {
                Ok(response) => {
                    if model.preview.is_some() && !model.submitted {
                        return false;
                    }
                    response
                        .operation
                        .is_some_and(|status| accept(model, status))
                }
                Err(error) => {
                    if error.code == storage_contract::ErrorCode::OperationNotFound
                        && model.rejected
                    {
                        model.submitted = false;
                        model.preview = None;
                        model.operation = None;
                        model.status = None;
                        model.acknowledged = false;
                    } else {
                        model.error = Some(error);
                    }
                    false
                }
            }
        }) == Some(true);
        if completed {
            refresh_after_exit(state);
        }
    });
}
pub(super) fn poll(state: RwSignal<State>) {
    if state.with_untracked(|s| {
        s.session_active()
            && s.admin_console.storage.lifecycle.submitted
            && s.admin_console.storage.lifecycle.reading.is_none()
            && (s.admin_console.storage.lifecycle.sending
                || s.admin_console
                    .storage
                    .lifecycle
                    .status
                    .as_ref()
                    .is_none_or(|status| status.state == ExitState::Running))
    }) {
        load(state);
    }
}
fn action_text(action: ExitAction) -> String {
    match action {
        ExitAction::PlannedOffline => t("计划下线", "Planned offline"),
        ExitAction::Deregister => t("注销登记", "Deregister"),
    }
}
fn phase_text(phase: ExitPhase) -> String {
    match phase {
        ExitPhase::Prepared => t("意图已保存", "Intent saved"),
        ExitPhase::Fencing => t("冻结原代", "Fencing original generation"),
        ExitPhase::Archiving => t("准备控制证据", "Preparing control evidence"),
        ExitPhase::Enrolling => t("安装独立控制副本", "Installing independent control copies"),
        ExitPhase::JointDecision => t("确认联合决定", "Confirming joint decision"),
        ExitPhase::Activating => t(
            "停止目录 I/O 并核验退出发布",
            "Closing directory I/O and verifying exit publication",
        ),
        ExitPhase::Completed => t("已核验发布", "Publication verified"),
    }
}
pub(in crate::admin) fn view(state: RwSignal<State>) -> impl IntoView {
    on_cleanup(move || {
        let _ = state.try_update(|s| s.admin_console.storage.lifecycle.leave_preview());
    });
    view! {
        {move ||state.with(|s|s.admin_console.storage.lifecycle.preview.clone()).map(|preview|view!{
            <section class="zxs-frame zxs-stack">
                <h3 class="zxs-row">{action_text(preview.request.action)}</h3>
                <p class="zxs-row">{format!("{} · {} · {}",preview.target.slot_id,preview.target.disk_id,preview.target.root_path)}</p>
                <p class="zxs-row">{match preview.request.action {
                    ExitAction::PlannedOffline => t("下线保留登记与全部数据，并停止此目录的全部 ZXS I/O。不是卸载、格式化或擦盘。", "Offline retains registration and all data, and stops all ZXS I/O to this directory. It does not unmount, format or erase a disk."),
                    ExitAction::Deregister => t("注销仅移除 ZXS 登记，并在当前目录视图隐藏只由此登记持有的文件；历史与磁盘字节保留。不会擦除，也不会向目标写备份或恢复文件。", "Deregistration removes the ZXS registration and hides current files held exclusively by it. History and disk bytes remain. It does not erase data or write backup or recovery files to the target."),
                }}</p>
                <p class="zxs-row">{format!("{}{} → {}",t("元数据成员：", "Metadata members: "),preview.original_metadata_members,preview.target_metadata_members)}</p>
                <Show when=move ||preview.authority==ExitAuthority::TerminalManagement>
                    <p class="zxs-row">{t("这是最后一个元数据成员。完成后服务进入保留登记与历史的管理态，普通读写关闭；不会初始化一个零成员池。后续新组必须通过原认证历史接续。", "This is the final metadata member. Completion enters management with retained registrations and history, and closes ordinary storage access. It does not initialize a zero-member pool. A new group must resume from the authenticated original history.")}</p>
                </Show>
                <p class="zxs-row">{t("提交会冻结原代并准备真实退出证明；持有 I/O 尚未结束时需要继续同一操作。关闭页面不取消，不能把准备成功当作下线或注销完成。", "Submission fences the original generation and prepares real exit evidence. Outstanding I/O may require resuming this same operation. Closing the page does not cancel it; preparation is not completion.")}</p>
                <label class="zxs-row"><input type="checkbox" disabled=move ||state.with(|s|s.admin_console.storage.lifecycle.blocks_edit()) prop:checked=move ||state.with(|s|s.admin_console.storage.lifecycle.acknowledged) on:change=move |event|state.update(|s|{if s.session_active()&&!s.admin_console.storage.lifecycle.blocks_edit(){s.admin_console.storage.lifecycle.acknowledged=event_target_checked(&event);}})/>{t("我已核对目标、动作与影响，确认执行此操作。", "I reviewed the target, action and impact, and confirm this operation.")}</label>
                <button class="zxs-button" disabled=move ||state.with(|s|!can_submit_preview(s)) on:click=move |_|submit(state,false)>{t("确认执行", "Confirm operation")}</button>
                <button class="zxs-button" disabled=move ||state.with(|s|s.admin_console.storage.lifecycle.blocks_edit()) on:click=move |_|state.update(|s|{if s.session_active()&&!s.admin_console.storage.lifecycle.blocks_edit(){s.admin_console.storage.lifecycle.leave_preview();}})>{t("关闭未提交预览", "Dismiss unsubmitted preview")}</button>
            </section>
        })}
        {move ||state.with(|s|s.admin_console.storage.lifecycle.status.clone()).map(|status|view!{
            <p class="zxs-row" role="status">{format!("{} · {} · {}",status.operation_id,action_text(status.preview.request.action),phase_text(status.phase))}</p>
            {status.failure.map(|failure|view!{<p class="zxs-row" role="alert">{format!("{}: {}",failure.code,failure.message)}</p>})}
        })}
        <Show when=move ||state.with(|s|s.admin_console.storage.lifecycle.submitted&&!s.admin_console.storage.lifecycle.sending)>
            <p class="zxs-row">{t("保留原操作编号；结果未确认时不要换编号提交。", "Keep the original operation ID; do not submit another ID while its result is unconfirmed.")}</p>
            <button class="zxs-button" on:click=move |_|submit(state,true)>{t("继续原退出操作", "Continue original exit")}</button>
            <button class="zxs-button" disabled=move ||state.with(|s|s.admin_console.storage.lifecycle.reading.is_some()) on:click=move |_|load(state)>{t("核对原操作", "Check original operation")}</button>
        </Show>
        {move ||state.with(|s|s.admin_console.storage.lifecycle.error.clone()).map(|error|{let feedback=crate::ui::feedback(&error);view!{<ErrorState message=feedback.message request_id=feedback.request_id/>}})}
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/admin/storage_lifecycle_tests.rs"]
mod tests;
