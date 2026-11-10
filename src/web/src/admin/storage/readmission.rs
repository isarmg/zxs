use super::*;
use std::collections::BTreeMap;
use storage_contract::{OperationId, storage_readmission::*};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RegistrationAuthority {
    Active,
    Terminal,
    #[default]
    Unadmitted,
}

#[derive(Clone, Default)]
pub(super) struct Model {
    registrations: BTreeMap<String, RegistrationEntry>,
    authority: RegistrationAuthority,
    registrations_loading: bool,
    registrations_generation: u64,
    preview: Option<ReadmissionPreview>,
    status: Option<ReadmissionStatus>,
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
                .is_some_and(|status| status.state != ReadmissionState::Completed)
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
            if self.status.is_none() {
                self.operation = None;
            }
        }
        self.registrations_generation += 1;
        self.registrations_loading = false;
    }
}
pub(in crate::admin) fn can_preview(state: &State, slot: &str, disk: &str) -> bool {
    let storage = &state.admin_console.storage;
    let eligible = registration(state, slot, disk).is_some_and(|entry| {
        entry.mode == RegistrationMode::Disabled
            && storage.snapshot.as_ref().is_some_and(|snapshot| {
                matches!(
                    (
                        snapshot.state,
                        storage.readmission.authority,
                        entry.readmission
                    ),
                    (StorageState::Active, RegistrationAuthority::Active, Some(_))
                        | (
                            StorageState::TerminalManagement,
                            RegistrationAuthority::Terminal,
                            Some(ReadmissionRole::MetadataMember)
                        )
                )
            })
    });
    state.session_active()
        && eligible
        && !storage.busy
        && !storage.addition.blocks_edit()
        && !storage.conversion.blocks_edit()
        && !storage.lifecycle.blocks_edit()
        && !storage.readmission.blocks_edit()
}
fn registration<'a>(state: &'a State, slot: &str, disk: &str) -> Option<&'a RegistrationEntry> {
    let model = &state.admin_console.storage.readmission;
    if model.authority == RegistrationAuthority::Unadmitted {
        return None;
    }
    model
        .registrations
        .get(slot)
        .filter(|entry| entry.disk_id == disk)
}
pub(super) fn active_registration<'a>(
    state: &'a State,
    slot: &str,
    disk: &str,
) -> Option<&'a RegistrationEntry> {
    let storage = &state.admin_console.storage;
    if storage.readmission.authority != RegistrationAuthority::Active
        || storage
            .snapshot
            .as_ref()
            .is_none_or(|snapshot| snapshot.state != StorageState::Active)
    {
        return None;
    }
    registration(state, slot, disk)
}

pub(in crate::admin) fn registration_mode(
    state: &State,
    slot: &str,
    disk: &str,
) -> Option<RegistrationMode> {
    registration(state, slot, disk).map(|entry| entry.mode)
}

pub(in crate::admin) fn registration_label(state: &State, slot: &str, disk: &str) -> String {
    match registration_mode(state, slot, disk) {
        Some(RegistrationMode::Enabled)
            if state.admin_console.storage.readmission.authority
                == RegistrationAuthority::Terminal =>
        {
            t(
                "登记：已启用；当前无活跃元数据成员，管理态未开放目录 I/O。",
                "Registration: Enabled; no active metadata voters remain, so management state keeps directory I/O closed.",
            )
        }
        Some(RegistrationMode::Enabled) => t(
            "登记：已启用。设备故障离线不等于计划下线。",
            "Registration: Enabled. An unavailable device is not a planned offline registration.",
        ),
        Some(RegistrationMode::Disabled)
            if state.admin_console.storage.readmission.authority
                == RegistrationAuthority::Terminal =>
        {
            if registration(state, slot, disk)
                .is_some_and(|entry| entry.readmission == Some(ReadmissionRole::MetadataMember))
            {
                t(
                    "登记：已禁用；已认证历史元数据角色，可预览重新启用。确认前不会访问目录。",
                    "Registration: Disabled; its authenticated historical metadata role can be previewed for readmission. The directory stays unopened until confirmation.",
                )
            } else {
                t(
                    "登记：已禁用。此目录不能独立恢复元数据法定组；请先重新启用保留的元数据成员，或添加新元数据成员。",
                    "Registration: Disabled. This directory cannot independently restore a metadata quorum. First readmit a retained metadata voter or add a new metadata voter.",
                )
            }
        }
        Some(RegistrationMode::Disabled) => t(
            "登记：已禁用（计划下线），目录 I/O 关闭。",
            "Registration: Disabled (planned offline), with directory I/O closed.",
        ),
        None => t(
            "登记：未准入或当前状态未知，不能重新启用。",
            "Registration: not admitted or currently unknown; readmission is unavailable.",
        ),
    }
}

fn registration_projection(
    snapshot: RegistrationSnapshot,
) -> Result<(RegistrationAuthority, BTreeMap<String, RegistrationEntry>), api::Error> {
    let (authority, entries) = match snapshot {
        RegistrationSnapshot::ActiveMembership { registrations } => {
            (RegistrationAuthority::Active, registrations)
        }
        RegistrationSnapshot::TerminalManagement { registrations } => {
            (RegistrationAuthority::Terminal, registrations)
        }
        RegistrationSnapshot::Unadmitted => {
            return Ok((RegistrationAuthority::Unadmitted, BTreeMap::new()));
        }
    };
    if authority == RegistrationAuthority::Active && entries.is_empty() {
        return Err(invalid(ResponseError::Identity));
    }
    let mut registrations = BTreeMap::new();
    for entry in entries {
        if entry.slot_id.is_empty()
            || storage_contract::validate_id(&entry.disk_id, "disk").is_err()
            || entry.binding_generation == 0
            || (entry.mode == RegistrationMode::Disabled && entry.metadata_member)
            || (authority == RegistrationAuthority::Terminal && entry.metadata_member)
            || (entry.mode == RegistrationMode::Enabled && entry.readmission.is_some())
            || (authority == RegistrationAuthority::Active
                && entry.mode == RegistrationMode::Disabled
                && entry.readmission.is_none())
            || (authority == RegistrationAuthority::Terminal
                && entry.readmission == Some(ReadmissionRole::DataOnly))
            || registrations.insert(entry.slot_id.clone(), entry).is_some()
        {
            return Err(invalid(ResponseError::Identity));
        }
    }
    Ok((authority, registrations))
}

fn finish_registrations(
    model: &mut Model,
    generation: u64,
    operation_generation: u64,
    result: Result<(RegistrationAuthority, BTreeMap<String, RegistrationEntry>), api::Error>,
) -> bool {
    if !model.registrations_loading || model.registrations_generation != generation {
        return false;
    }
    model.registrations_loading = false;
    if model.generation != operation_generation {
        return true;
    }
    match result {
        Ok((authority, entries)) => {
            if model.authority != authority || model.registrations != entries {
                model.authority = authority;
                model.registrations = entries;
            }
        }
        Err(error) => {
            model.authority = RegistrationAuthority::Unadmitted;
            model.registrations.clear();
            model.error = Some(error);
        }
    }
    false
}

pub(in crate::admin) fn load_registrations(state: RwSignal<State>) {
    let captured = state
        .try_update(|s| {
            if !s.session_active() || s.admin_console.storage.readmission.registrations_loading {
                return None;
            }
            let model = &mut s.admin_console.storage.readmission;
            model.registrations_loading = true;
            model.registrations_generation += 1;
            Some((
                s.session_epoch,
                model.registrations_generation,
                model.generation,
            ))
        })
        .flatten();
    let Some((epoch, generation, operation_generation)) = captured else {
        return;
    };
    spawn_local(async move {
        let result =
            api::validated_request("GET", "/api/admin/disks/registrations", None, |value| {
                registration_projection(decode::<RegistrationSnapshot>(value)?)
            })
            .await;
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        let refresh = state.try_update(|s| {
            if s.session_epoch != epoch {
                return false;
            }
            finish_registrations(
                &mut s.admin_console.storage.readmission,
                generation,
                operation_generation,
                result,
            )
        }) == Some(true);
        if refresh {
            load_registrations(state);
        }
    });
}

pub(in crate::admin) fn preview(state: RwSignal<State>, slot_id: String, expected_disk_id: String) {
    let Some(operation) = crate::ui::operation_id(state) else {
        return;
    };
    let operation_id: OperationId = operation
        .parse()
        .expect("invariant: browser operation generator produces canonical IDs");
    let request = ReadmissionRequest {
        operation_id,
        slot_id,
        expected_disk_id,
    };
    let captured = state
        .try_update(|s| {
            if !can_preview(s, &request.slot_id, &request.expected_disk_id) {
                return None;
            }
            let expected = registration(s, &request.slot_id, &request.expected_disk_id)
                .expect("invariant: eligible readmission has an exact registration")
                .clone();
            let model = &mut s.admin_console.storage.readmission;
            model.sending = true;
            model.error = None;
            model.generation += 1;
            Some((s.session_epoch, model.generation, model.authority, expected))
        })
        .flatten();
    let Some((epoch, generation, authority, registration)) = captured else {
        return;
    };
    spawn_local(async move {
        let expected = request.clone();
        let result = api::validated_request(
            "POST",
            "/api/admin/disks/readmission/preview",
            Some(json!(request)),
            |value| {
                let preview: ReadmissionPreview = decode(value)?;
                validate_preview(&preview)?;
                if preview.request != expected
                    || !preview_matches_registration(&preview, authority, &registration)
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
                || s.admin_console.storage.readmission.generation != generation
            {
                return;
            }
            let model = &mut s.admin_console.storage.readmission;
            model.sending = false;
            match result {
                Ok(preview) => {
                    if model.authority != authority
                        || model.registrations.get(&registration.slot_id) != Some(&registration)
                    {
                        return;
                    }
                    model.operation = Some(preview.request.operation_id);
                    model.preview = Some(preview);
                    model.status = None;
                    model.acknowledged = false;
                }
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
            let model = &s.admin_console.storage.readmission;
            if model.sending
                || model
                    .status
                    .as_ref()
                    .is_some_and(|status| status.state == ReadmissionState::Completed)
            {
                return None;
            }
            let operation = model.operation?;
            let preview = model
                .preview
                .clone()
                .or_else(|| model.status.as_ref().map(|status| status.preview.clone()))?;
            if !model.submitted
                && (!can_preview(s, &preview.target.slot_id, &preview.target.disk_id)
                    || registration(s, &preview.target.slot_id, &preview.target.disk_id)
                        .is_none_or(|entry| {
                            !preview_matches_registration(&preview, model.authority, entry)
                        }))
            {
                return None;
            }
            let (path, body) = if resume && model.status.is_some() {
                (
                    "/api/admin/disks/readmission/resume",
                    json!(ReadmissionResume {
                        operation_id: operation
                    }),
                )
            } else {
                if !model.submitted && !model.acknowledged {
                    return None;
                }
                (
                    "/api/admin/disks/readmission",
                    json!(ReadmissionCommit {
                        preview: preview.clone()
                    }),
                )
            };
            let model = &mut s.admin_console.storage.readmission;
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
            let status: ReadmissionStatus = decode(value)?;
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
                || s.admin_console.storage.readmission.generation != generation
            {
                return false;
            }
            let model = &mut s.admin_console.storage.readmission;
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
            refresh_after_readmission(state);
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
fn preview_matches_registration(
    preview: &ReadmissionPreview,
    authority: RegistrationAuthority,
    registration: &RegistrationEntry,
) -> bool {
    let authority_matches = matches!(
        (&preview.authority, authority),
        (
            ReadmissionAuthority::ActiveMembership { .. },
            RegistrationAuthority::Active
        ) | (
            ReadmissionAuthority::TerminalManagement { .. },
            RegistrationAuthority::Terminal
        )
    );
    authority_matches
        && registration.mode == RegistrationMode::Disabled
        && registration.slot_id == preview.target.slot_id
        && registration.disk_id == preview.target.disk_id
        && registration.binding_generation == preview.target.binding_generation
        && registration.readmission
            == Some(if preview.metadata_member {
                ReadmissionRole::MetadataMember
            } else {
                ReadmissionRole::DataOnly
            })
}
fn validate_preview(preview: &ReadmissionPreview) -> Result<(), api::Error> {
    if preview.request.expected_disk_id != preview.target.disk_id
        || preview.request.slot_id != preview.target.slot_id
        || storage_contract::validate_id(&preview.target.pool_id, "pool").is_err()
        || storage_contract::validate_id(&preview.target.disk_id, "disk").is_err()
        || preview.target.slot_id.is_empty()
        || preview.target.root_path.is_empty()
        || preview.target.binding_generation == 0
        || preview.principal.is_empty()
        || preview.registry_generation == 0
        || !digest(&preview.configuration_digest)
        || !digest(&preview.preview_digest)
        || !digest(preview.authority.hash())
        || (preview.authority.is_terminal() && !preview.metadata_member)
        || !digest(&preview.registry_root_hash)
        || !digest(&preview.snapshot_hash)
    {
        return Err(invalid(ResponseError::Identity));
    }
    Ok(())
}
fn validate_status(
    status: &ReadmissionStatus,
    expected: Option<OperationId>,
) -> Result<(), api::Error> {
    validate_preview(&status.preview)?;
    let completed = status.state == ReadmissionState::Completed;
    if expected.is_some_and(|operation| operation != status.operation_id)
        || status.preview.request.operation_id != status.operation_id
        || completed != (status.phase == ReadmissionPhase::Completed)
        || completed != status.result.is_some()
        || (completed && status.failure.is_some())
    {
        return Err(invalid(ResponseError::Identity));
    }
    if let Some(result) = &status.result {
        let target = &status.preview.target;
        let generation = target.binding_generation.to_string();
        let outcome = |key: &str| result.result.get(key).and_then(Value::as_str);
        let authority_matches = match &status.preview.authority {
            ReadmissionAuthority::ActiveMembership { .. } => {
                result.result.get("authority_kind").is_none()
            }
            ReadmissionAuthority::TerminalManagement {
                terminal_authority_hash,
                ..
            } => {
                outcome("authority_kind") == Some("terminal-metadata-resume")
                    && outcome("terminal_authority") == Some(terminal_authority_hash.as_str())
            }
        };
        if result.operation_id != status.operation_id
            || !authority_matches
            || result.kind != "disk-readmit"
            || result.commit_state != storage_contract::CommitState::Published
            || !digest(&result.request_digest)
            || outcome("disk_id") != Some(target.disk_id.as_str())
            || outcome("slot_id") != Some(target.slot_id.as_str())
            || outcome("binding_generation") != Some(generation.as_str())
            || outcome("registration") != Some("enabled")
            || outcome("io_admission_epoch") != Some(result.request_digest.as_str())
        {
            return Err(invalid(ResponseError::Identity));
        }
    }
    Ok(())
}
fn accept(model: &mut Model, status: ReadmissionStatus) -> bool {
    let completed = status.state == ReadmissionState::Completed;
    let changed = completed
        && !model.status.as_ref().is_some_and(|old| {
            old.operation_id == status.operation_id && old.state == ReadmissionState::Completed
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
fn refresh_after_readmission(state: RwSignal<State>) {
    state.update(|s| s.files.invalidate_disks());
    super::super::disks::load(state);
    crate::management::load(state);
    super::load(state);
}
pub(super) fn load(state: RwSignal<State>) {
    load_registrations(state);
    let captured = state
        .try_update(|s| {
            if !s.session_active() {
                return None;
            }
            let model = &mut s.admin_console.storage.readmission;
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
            || "/api/admin/disks/readmission".into(),
            |operation| format!("/api/admin/disks/readmission?operation_id={operation}"),
        );
        let result = api::validated_request("GET", &path, None, |value| {
            let response: ReadmissionStatusResponse = decode(value)?;
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
            let model = &mut s.admin_console.storage.readmission;
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
            refresh_after_readmission(state);
        }
    });
}
pub(super) fn poll(state: RwSignal<State>) {
    if state.with_untracked(|s| {
        s.session_active()
            && s.admin_console.storage.readmission.submitted
            && s.admin_console.storage.readmission.reading.is_none()
            && (s.admin_console.storage.readmission.sending
                || s.admin_console
                    .storage
                    .readmission
                    .status
                    .as_ref()
                    .is_none_or(|status| status.state == ReadmissionState::Running))
    }) {
        load(state);
    }
}
fn phase_text(phase: ReadmissionPhase) -> String {
    match phase {
        ReadmissionPhase::Prepared => t("意图已保存", "Intent saved"),
        ReadmissionPhase::Fencing => t(
            "核验旧 I/O 已关闭并冻结原代",
            "Verifying old I/O closure and fencing the source",
        ),
        ReadmissionPhase::Archiving => t(
            "准备认证控制证据",
            "Preparing authenticated control evidence",
        ),
        ReadmissionPhase::Activating => t(
            "决定并发布新的 I/O 准入代际",
            "Deciding and publishing a new I/O admission epoch",
        ),
        ReadmissionPhase::Completed => t("重新启用已核验发布", "Readmission publication verified"),
    }
}
pub(in crate::admin) fn view(state: RwSignal<State>) -> impl IntoView {
    on_cleanup(move || {
        let _ = state.try_update(|s| s.admin_console.storage.readmission.leave_preview());
    });
    view! {
        {move ||state.with(|s|s.admin_console.storage.readmission.preview.clone()).map(|preview|view!{
            <section class="zxs-frame zxs-stack">
                <h3 class="zxs-row">{t("重新启用原登记目录", "Readmit the registered directory")}</h3>
                <p class="zxs-row">{format!("{} · {} · {}",preview.target.slot_id,preview.target.disk_id,preview.target.root_path)}</p>
                <p class="zxs-row">{format!("{}{} · {}{}",t("登记绑定代数：", "Binding generation: "),preview.target.binding_generation,t("当前登记视图：", "Current registry generation: "),preview.registry_generation)}</p>
                <p class="zxs-row">{if preview.metadata_member { t("恢复角色：元数据成员。", "Restored role: metadata member.") } else { t("恢复角色：仅数据目录。", "Restored role: data-only directory.") }}</p>
                <p class="zxs-row">{t("该登记当前已禁用。此预览仅查询已认证登记与历史，不打开或探测这个目录。", "This registration is Disabled. This preview reads authenticated registry and history metadata without opening or probing the directory.")}</p>
                <p class="zxs-row">{t("确认提交才会打开上列原登记目录并核验身份、完整控制证据与旧 I/O 关闭证明。成功后保留原身份、绑定代数和历史，只授予新的 I/O 准入代际；旧任务与旧句柄仍永久禁用。", "Confirming submission opens only the original directory above and verifies its identity, complete control evidence, and old I/O closure. Success retains its identity, binding generation, and history while admitting a new I/O epoch. Old tasks and handles remain permanently disabled.")}</p>
                <p class="zxs-row">{if preview.authority.is_terminal() { t("本次由已认证的管理态恢复这个保留元数据成员，并由它完整核验旧控制历史、形成非零元数据法定组。不会初始化新池，也不会重新启用其他禁用目录。", "The authenticated management authority restores this retained metadata voter, which verifies the complete old control history and establishes a nonzero metadata quorum. This does not initialize a new pool or readmit any other Disabled directory.") } else { t("这是恢复原登记，不是添加、注销后重建或初始化新池。", "This restores an existing registration, not an addition, re-registration, or new pool.") }}</p>
                <p class="zxs-row">{t("目录可以位于同一文件系统；这不证明物理副本独立。", "Directories may share a filesystem; that does not prove independent physical copies.")}</p>
                <p class="zxs-row">{t("完成发布前仍为禁用。失败、关闭页面或结果未知都不能换编号重新执行，只能核对并继续这个原操作。", "The registration remains Disabled until publication completes. After failure, closing the page, or an unknown result, check and continue this original operation instead of submitting another ID.")}</p>
                <label class="zxs-row"><input type="checkbox" disabled=move ||state.with(|s|s.admin_console.storage.readmission.blocks_edit()) prop:checked=move ||state.with(|s|s.admin_console.storage.readmission.acknowledged) on:change=move |event|state.update(|s|{if s.session_active()&&!s.admin_console.storage.readmission.blocks_edit(){s.admin_console.storage.readmission.acknowledged=event_target_checked(&event);}})/>{t("我已核对原登记，允许本次提交访问该目录并重新启用。", "I reviewed the original registration and authorize this submission to access and readmit this directory.")}</label>
                <button class="zxs-button" disabled=move ||state.with(|s|s.admin_console.storage.readmission.blocks_edit()||!s.admin_console.storage.readmission.acknowledged) on:click=move |_|submit(state,false)>{t("确认重新启用", "Confirm readmission")}</button>
                <button class="zxs-button" disabled=move ||state.with(|s|s.admin_console.storage.readmission.blocks_edit()) on:click=move |_|state.update(|s|{if s.session_active()&&!s.admin_console.storage.readmission.blocks_edit(){s.admin_console.storage.readmission.leave_preview();}})>{t("关闭未提交预览", "Dismiss unsubmitted preview")}</button>
            </section>
        })}
        {move ||state.with(|s|s.admin_console.storage.readmission.status.clone()).map(|status|{let completed=status.state==ReadmissionState::Completed; view!{
            <p class="zxs-row" role="status">{format!("{} · {}",status.operation_id,phase_text(status.phase))}</p>
            {completed.then(||view!{<p class="zxs-row">{t("这是该操作发布时的永久回执。当前目录是否启用以上方登记状态为准；后续下线不会改变这份历史结果。", "This is the operation's permanent publication receipt. The registration above shows whether the directory is currently Enabled; a later offline action does not change this historical result.")}</p>})}
            {status.failure.map(|failure|view!{<p class="zxs-row" role="alert">{format!("{}: {}",failure.code,failure.message)}</p>})}
        }})}
        <Show when=move ||state.with(|s|s.admin_console.storage.readmission.submitted&&!s.admin_console.storage.readmission.sending)>
            <p class="zxs-row">{t("保留原操作编号；结果未确认时不要换编号提交。", "Keep the original operation ID; do not submit another ID while its result is unconfirmed.")}</p>
            <button class="zxs-button" on:click=move |_|submit(state,true)>{t("继续原重新启用操作", "Continue original readmission")}</button>
            <button class="zxs-button" disabled=move ||state.with(|s|s.admin_console.storage.readmission.reading.is_some()) on:click=move |_|load(state)>{t("核对原操作", "Check original operation")}</button>
        </Show>
        {move ||state.with(|s|s.admin_console.storage.readmission.error.clone()).map(|error|{let feedback=crate::ui::feedback(&error);view!{<ErrorState message=feedback.message request_id=feedback.request_id/>}})}
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/admin/storage_readmission_tests.rs"]
mod tests;
