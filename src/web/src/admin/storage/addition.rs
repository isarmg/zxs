//! Existing-pool addition keeps its permanent operation across request failures.
use super::*;
use std::collections::BTreeMap;
use storage_contract::{OperationId, storage_addition::*};

#[derive(Clone)]
struct Fields {
    slot: String,
    level: String,
    upstream: String,
    metadata: bool,
}
#[derive(Clone, Default)]
pub(super) struct Model {
    fields: BTreeMap<String, Fields>,
    preview: Option<AdditionPreview>,
    status: Option<AdditionStatus>,
    operation: Option<OperationId>,
    submitted: bool,
    rejected: bool,
    sending: bool,
    reading: bool,
    generation: u64,
    acknowledged: bool,
    error: Option<api::Error>,
}
impl Model {
    pub(super) fn blocks_edit(&self) -> bool {
        self.sending
            || self.reading
            || self.submitted
            || self
                .status
                .as_ref()
                .is_some_and(|status| status.state != AdditionState::Completed)
    }
    pub(super) fn invalidate(&mut self) {
        self.preview = None;
        self.operation = None;
        self.acknowledged = false;
    }
}
fn active(model: &super::Model) -> bool {
    model
        .snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.state == StorageState::Active)
}
pub(super) fn selected(model: &mut super::Model) {
    model.addition.invalidate();
    let selections = &model.selections;
    model
        .addition
        .fields
        .retain(|id, _| selections.iter().any(|s| s.selection_id == *id));
    for (index, selection) in selections.iter().enumerate() {
        model
            .addition
            .fields
            .entry(selection.selection_id.clone())
            .or_insert_with(|| Fields {
                slot: format!("N{:02}", index + 1),
                level: "1".into(),
                upstream: String::new(),
                metadata: false,
            });
    }
}
fn input_error(message: String) -> api::Error {
    api::error(storage_contract::ErrorCode::ConfigInvalid, &message)
}
fn make_request(
    model: &super::Model,
    operation_id: OperationId,
) -> Result<AdditionRequest, api::Error> {
    let additions = model
        .selections
        .iter()
        .map(|selection| {
            let fields = model
                .addition
                .fields
                .get(&selection.selection_id)
                .expect("invariant: selected addition has fields");
            let level = fields.level.parse::<u32>().map_err(|_| {
                input_error(t("层级必须是正整数。", "Level must be a positive integer."))
            })?;
            if level == 0 {
                return Err(input_error(t(
                    "层级必须是正整数。",
                    "Level must be a positive integer.",
                )));
            }
            Ok(DirectoryAddition {
                selection: selection.clone(),
                slot_id: fields.slot.clone(),
                level,
                upstream_slot_id: (!fields.upstream.is_empty()).then(|| fields.upstream.clone()),
                metadata_member: fields.metadata,
            })
        })
        .collect::<Result<Vec<_>, api::Error>>()?;
    Ok(AdditionRequest {
        operation_id,
        additions,
    })
}
fn preview(state: RwSignal<State>) {
    let Some(id) = crate::ui::operation_id(state) else {
        return;
    };
    let operation: OperationId = id
        .parse()
        .expect("invariant: browser operation generator produces canonical IDs");
    let captured = state
        .try_update(|s| {
            if !s.session_active() {
                return None;
            }
            let model = &mut s.admin_console.storage;
            if !model.editable() || !active(model) || model.selections.is_empty() {
                return None;
            }
            let request = match make_request(model, operation) {
                Ok(request) => request,
                Err(error) => {
                    model.addition.error = Some(error);
                    return None;
                }
            };
            model.addition.sending = true;
            model.addition.error = None;
            model.addition.generation += 1;
            Some((s.session_epoch, model.addition.generation, request))
        })
        .flatten();
    let Some((epoch, generation, request)) = captured else {
        return;
    };
    spawn_local(async move {
        let expected = request.clone();
        let result = api::validated_request(
            "POST",
            "/api/admin/disks/preview",
            Some(json!(request)),
            |value| {
                let preview: AdditionPreview = decode(value)?;
                validate_preview(&preview)?;
                if preview.request != expected {
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
            if s.session_epoch != epoch || s.admin_console.storage.addition.generation != generation
            {
                return;
            }
            let model = &mut s.admin_console.storage.addition;
            model.sending = false;
            match result {
                Ok(preview) => {
                    model.operation = Some(preview.request.operation_id);
                    model.status = None;
                    model.preview = Some(preview);
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
            if !s.session_active() {
                return None;
            }
            let model = &mut s.admin_console.storage.addition;
            if model.sending {
                return None;
            }
            let operation = model.operation?;
            let expected_preview = model
                .preview
                .clone()
                .or_else(|| model.status.as_ref().map(|status| status.preview.clone()))?;
            let body = if resume
                && model
                    .status
                    .as_ref()
                    .is_some_and(|status| status.operation_id == operation)
            {
                if !model.submitted
                    && !model
                        .status
                        .as_ref()
                        .is_some_and(|status| status.state == AdditionState::RecoveryRequired)
                {
                    return None;
                }
                json!(AdditionResume {
                    operation_id: operation
                })
            } else {
                if !resume && (model.submitted || !model.acknowledged) {
                    return None;
                }
                json!(AdditionCommit {
                    preview: model.preview.clone()?
                })
            };
            let resume = resume
                && model
                    .status
                    .as_ref()
                    .is_some_and(|status| status.operation_id == operation);
            model.sending = true;
            model.submitted = true;
            model.rejected = false;
            model.error = None;
            model.generation += 1;
            Some((
                s.session_epoch,
                model.generation,
                operation,
                body,
                resume,
                expected_preview,
            ))
        })
        .flatten();
    let Some((epoch, generation, operation, body, resume, expected_preview)) = captured else {
        return;
    };
    spawn_local(async move {
        let path = if resume {
            "/api/admin/disks/addition/resume"
        } else {
            "/api/admin/disks"
        };
        let result = api::validated_request("POST", path, Some(body), |value| {
            let status: AdditionStatus = decode(value)?;
            validate_status(&status, Some(operation))?;
            if status.preview != expected_preview {
                return Err(invalid(ResponseError::Identity));
            }
            Ok(status)
        })
        .await;
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        let completed = state.try_update(|s| {
            if s.session_epoch != epoch || s.admin_console.storage.addition.generation != generation
            {
                return false;
            }
            let model = &mut s.admin_console.storage.addition;
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
            completed_addition(state);
        }
        load(state);
    });
}
fn validate_preview(preview: &AdditionPreview) -> Result<(), api::Error> {
    let digest = |value: &str| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    if preview.request.additions.is_empty()
        || preview.request.additions.len() > 32
        || preview.added_directories != preview.request.additions.len()
        || preview.original_metadata_members == 0
        || preview.original_metadata_members.checked_add(
            preview
                .request
                .additions
                .iter()
                .filter(|addition| addition.metadata_member)
                .count(),
        ) != Some(preview.target_metadata_members)
        || !digest(&preview.preview_digest)
        || !digest(&preview.configuration_digest)
        || storage_contract::validate_id(&preview.pool_id, "pool").is_err()
    {
        return Err(invalid(ResponseError::Identity));
    }
    Ok(())
}
fn validate_status(
    status: &AdditionStatus,
    expected: Option<OperationId>,
) -> Result<(), api::Error> {
    validate_preview(&status.preview)?;
    if expected.is_some_and(|id| id != status.operation_id)
        || status.preview.request.operation_id != status.operation_id
        || (status.state == AdditionState::Completed) != (status.phase == AdditionPhase::Completed)
        || (status.state == AdditionState::Completed && status.failure.is_some())
    {
        return Err(invalid(ResponseError::Identity));
    }
    Ok(())
}
fn accept(model: &mut Model, status: AdditionStatus) -> bool {
    let completed = status.state == AdditionState::Completed;
    let newly_completed = completed
        && !model.status.as_ref().is_some_and(|old| {
            old.operation_id == status.operation_id && old.state == AdditionState::Completed
        });
    model.operation = Some(status.operation_id);
    model.submitted = !completed;
    model.status = Some(status);
    newly_completed
}
fn completed_addition(state: RwSignal<State>) {
    state.update(|s| {
        s.admin_console.storage.selections.clear();
        s.admin_console.storage.inspected = None;
        s.admin_console.storage.addition.preview = None;
        s.admin_console.storage.addition.acknowledged = false;
        s.files.invalidate_disks();
    });
    super::super::disks::load(state);
    crate::management::load(state);
}
pub(super) fn load(state: RwSignal<State>) {
    let captured = state
        .try_update(|s| {
            if !s.session_active() {
                return None;
            }
            let model = &mut s.admin_console.storage.addition;
            if model.reading {
                return None;
            }
            model.reading = true;
            // Preview-only IDs do not yet have a persisted status.
            let operation = model.submitted.then_some(model.operation).flatten();
            let expected = model.submitted.then(|| model.preview.clone()).flatten();
            Some((s.session_epoch, model.generation, operation, expected))
        })
        .flatten();
    let Some((epoch, generation, operation, expected)) = captured else {
        return;
    };
    spawn_local(async move {
        let path = operation.map_or_else(
            || "/api/admin/disks/addition".into(),
            |id| format!("/api/admin/disks/addition?operation_id={id}"),
        );
        let result = api::validated_request("GET", &path, None, |value| {
            let response: AdditionStatusResponse = decode(value)?;
            if let Some(status) = &response.operation {
                validate_status(status, operation)?;
                if expected
                    .as_ref()
                    .is_some_and(|preview| preview != &status.preview)
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
            let model = &mut s.admin_console.storage.addition;
            model.reading = false;
            if model.generation != generation {
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
                        && !model.sending
                    {
                        model.submitted = false;
                        model.status = None;
                        model.invalidate();
                    } else {
                        model.error = Some(error);
                    }
                    false
                }
            }
        }) == Some(true);
        if completed {
            completed_addition(state);
        }
    });
}
pub(super) fn poll(state: RwSignal<State>) {
    if state.with_untracked(|s| {
        s.session_active()
            && s.admin_console.storage.addition.submitted
            && !s.admin_console.storage.addition.reading
            && (s.admin_console.storage.addition.sending
                || s.admin_console
                    .storage
                    .addition
                    .status
                    .as_ref()
                    .is_none_or(|status| status.state == AdditionState::Adding))
    }) {
        load(state);
    }
}
fn edit(state: RwSignal<State>, id: &str, field: &'static str, value: String) {
    state.update(|s| {
        if !s.session_active() || !s.admin_console.storage.editable() {
            return;
        }
        let model = &mut s.admin_console.storage.addition;
        let fields = model
            .fields
            .get_mut(id)
            .expect("invariant: rendered selection has fields");
        match field {
            "slot" => fields.slot = value,
            "level" => fields.level = value,
            "upstream" => fields.upstream = value,
            _ => unreachable!("invariant: addition field"),
        }
        model.invalidate();
    });
}
fn field(
    state: RwSignal<State>,
    id: StoredValue<String>,
    key: &'static str,
    label: String,
) -> impl IntoView {
    view! {<label class="zxs-form-field"><span>{label}</span><input class="zxs-input" type="text" autocomplete="off" disabled=move ||state.with(|s|!s.admin_console.storage.editable()) prop:value=move ||state.with(|s|s.admin_console.storage.addition.fields.get(&id.get_value()).map(|fields|match key{"slot"=>fields.slot.clone(),"level"=>fields.level.clone(),"upstream"=>fields.upstream.clone(),_=>unreachable!("invariant: addition field")}).unwrap_or_default()) on:input=move |event|edit(state,&id.get_value(),key,event_target_value(&event))/></label>}
}
pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {
        <Show when=move ||state.with(|s|active(&s.admin_console.storage))>
            <p class="zxs-row">{t("向当前存储池登记目录。允许同一文件系统中的不同目录，但它们不提供独立磁盘故障冗余。新增目录不会自动搬移已有文件或改变现有放置规则。", "Register directories in the current pool. Different directories on one filesystem are allowed, but do not provide independent disk-failure redundancy. Adding directories does not move existing files or change placement rules.")}</p>
            <For each=move ||state.with(|s|s.admin_console.storage.selections.clone()) key=|item|item.selection_id.clone() children=move |selection|{
                let id=store_value(selection.selection_id);view!{<section class="zxs-frame zxs-stack"><h3 class="zxs-row">{selection.root_path}</h3><div class="zxs-form-grid">
                    {field(state,id,"slot",t("新槽位（唯一）", "New slot (unique)"))}
                    {field(state,id,"level",t("层级（1 为主存储）", "Level (1 for primary storage)"))}
                    {field(state,id,"upstream",t("上游槽位（备份层必填）", "Upstream slot (required for backup tiers)"))}
                </div><label class="zxs-row"><input type="checkbox" disabled=move ||state.with(|s|!s.admin_console.storage.editable()) prop:checked=move ||state.with(|s|s.admin_console.storage.addition.fields.get(&id.get_value()).is_some_and(|fields|fields.metadata)) on:change=move |event|state.update(|s|{if s.session_active()&&s.admin_console.storage.editable(){let model=&mut s.admin_console.storage.addition;model.fields.get_mut(&id.get_value()).expect("invariant: selection fields").metadata=event_target_checked(&event);model.invalidate();}})/>{t("同时保存元数据（不勾选则仅存数据）", "Also store metadata (unchecked means data only)")}</label></section>}
            }/>
            <button type="button" class="zxs-button" disabled=move ||state.with(|s|!s.admin_console.storage.editable()||s.admin_console.storage.selections.is_empty()) on:click=move |_|preview(state)>{t("预览在线添加", "Preview online addition")}</button>
            {move ||state.with(|s|s.admin_console.storage.addition.preview.clone()).map(|preview|view!{
                <p class="zxs-row">{format!("{}{} · {}{} → {}",t("新增目录：", "New directories: "),preview.added_directories,t("元数据成员：", "Metadata members: "),preview.original_metadata_members,preview.target_metadata_members)}</p>
                <p class="zxs-row">{t("提交后将暂停本代写入，复制并验证控制元数据，再发布新成员。中断只能继续同一操作；关闭页面不取消，暂不支持移出成员。", "Submitting pauses writes for this generation, copies and verifies control metadata, then publishes the new membership. Interrupted work resumes the same operation. Closing the page does not cancel it. Member removal is not available.")}</p>
                <label class="zxs-row"><input type="checkbox" disabled=move ||state.with(|s|s.admin_console.storage.addition.blocks_edit()) prop:checked=move ||state.with(|s|s.admin_console.storage.addition.acknowledged) on:change=move |event|state.update(|s|{if s.session_active()&&!s.admin_console.storage.addition.blocks_edit(){s.admin_console.storage.addition.acknowledged=event_target_checked(&event);}})/>{t("已核对目录、槽位、层级和元数据角色，确认添加。", "I reviewed the directories, slots, levels and metadata roles and confirm this addition.")}</label>
                <button type="button" class="zxs-button" disabled=move ||state.with(|s|s.admin_console.storage.addition.blocks_edit()||!s.admin_console.storage.addition.acknowledged) on:click=move |_|submit(state,false)>{t("添加到当前存储池", "Add to current pool")}</button>
            })}
        </Show>
        <Show when=move ||state.with(|s|s.admin_console.storage.addition.submitted)><p class="zxs-row" role="status">{t("添加已提交，正在核对服务器阶段。请保留原操作，不要换用新编号重发。", "The addition was submitted. Checking its server progress. Keep the original operation; do not resubmit with a new ID.")}</p></Show>
        {move ||state.with(|s|s.admin_console.storage.addition.status.clone()).map(|status|view!{
            <p class="zxs-row" role="status">{format!("{} · {} · {}",status.operation_id,state_text(status.state),phase_text(status.phase))}</p>
            {status.failure.map(|failure|view!{<p class="zxs-row" role="alert">{format!("{}: {}",failure.code,failure.message)}</p>})}
        })}
        <Show when=move ||state.with(|s|s.admin_console.storage.addition.submitted&&!s.admin_console.storage.addition.sending)>
            <button type="button" class="zxs-button" on:click=move |_|submit(state,true)>{t("继续原添加操作", "Continue original addition")}</button>
            <button type="button" class="zxs-button" disabled=move ||state.with(|s|s.admin_console.storage.addition.reading) on:click=move |_|load(state)>{t("核对原操作", "Check original operation")}</button>
        </Show>
        {move ||state.with(|s|s.admin_console.storage.addition.error.clone()).map(|error|{let feedback=crate::ui::feedback(&error);view!{<ErrorState message=feedback.message request_id=feedback.request_id/>}})}
    }
}
fn state_text(state: AdditionState) -> String {
    match state {
        AdditionState::Adding => t("添加中", "Adding"),
        AdditionState::RecoveryRequired => t("需要恢复", "Recovery required"),
        AdditionState::Completed => t("添加完成", "Addition complete"),
    }
}
fn phase_text(phase: AdditionPhase) -> String {
    match phase {
        AdditionPhase::Prepared => t("意图已保存", "Intent saved"),
        AdditionPhase::Fencing => t("冻结原成员", "Fencing original members"),
        AdditionPhase::Archiving => t("归档控制元数据", "Archiving control metadata"),
        AdditionPhase::Enrolling => t("准备新增目录", "Preparing new directories"),
        AdditionPhase::JointDecision => t("确认联合决定", "Confirming joint decision"),
        AdditionPhase::Activating => t("发布新成员", "Publishing membership"),
        AdditionPhase::Completed => t("已发布", "Published"),
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/admin/storage_addition_tests.rs"]
mod tests;
