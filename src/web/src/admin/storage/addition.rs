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
pub(super) struct Form {
    fields: BTreeMap<String, Fields>,
    cancel_acknowledged: bool,
    resume_add_acknowledged: bool,
}
pub(super) type Model = registration_operation::Model<AdditionStatus>;

impl Model {
    pub(super) fn invalidate(&mut self) {
        self.preview = None;
        self.operation = None;
        self.acknowledged = false;
        self.context.cancel_acknowledged = false;
        self.context.resume_add_acknowledged = false;
    }
}
impl registration_operation::Status for AdditionStatus {
    type Preview = AdditionPreview;
    type Context = Form;
    const PATH: &'static str = "/api/admin/disks/addition";

    fn model(state: &State) -> &Model {
        &state.admin_console.storage.addition
    }
    fn model_mut(state: &mut State) -> &mut Model {
        &mut state.admin_console.storage.addition
    }
    fn operation(&self) -> OperationId {
        self.operation_id
    }
    fn preview(&self) -> &Self::Preview {
        &self.preview
    }
    fn completed(&self) -> bool {
        self.state.is_terminal()
    }
    fn running(&self) -> bool {
        matches!(
            self.state,
            AdditionState::Adding | AdditionState::Cancelling
        )
    }
    fn validate(&self, expected: Option<OperationId>) -> Result<(), api::Error> {
        validate_status(self, expected)
    }
    fn response(value: Value) -> Result<Option<Self>, api::Error> {
        Ok(decode::<AdditionStatusResponse>(value)?.operation)
    }
    fn accept(model: &mut Model, status: Self) -> bool {
        accept(model, status)
    }
    fn discard_rejected(model: &mut Model) -> bool {
        if model.sending {
            return false;
        }
        model.submitted = false;
        model.status = None;
        model.invalidate();
        true
    }
    fn refresh(state: RwSignal<State>) {
        completed_addition(state);
    }
}
fn active(model: &super::Model) -> bool {
    model.snapshot.as_ref().is_some_and(|snapshot| {
        matches!(
            snapshot.state,
            StorageState::Active | StorageState::TerminalManagement
        )
    })
}
fn terminal(model: &super::Model) -> bool {
    model
        .snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.state == StorageState::TerminalManagement)
}
fn can_cancel(model: &Model) -> bool {
    model.status.as_ref().is_some_and(|status| {
        !status.state.is_terminal()
            && (!status.preview.authority.is_terminal()
                || matches!(
                    status.phase,
                    AdditionPhase::ResumeCancellable
                        | AdditionPhase::CertifyingAbort
                        | AdditionPhase::Cancelling
                ))
    }) && !model.sending
}
pub(super) fn selected(model: &mut super::Model) {
    model.addition.invalidate();
    let metadata = terminal(model);
    let selections = &model.selections;
    model
        .addition
        .context
        .fields
        .retain(|id, _| selections.iter().any(|s| s.selection_id == *id));
    for (index, selection) in selections.iter().enumerate() {
        model
            .addition
            .context
            .fields
            .entry(selection.selection_id.clone())
            .or_insert_with(|| Fields {
                slot: format!("N{:02}", index + 1),
                level: "1".into(),
                upstream: String::new(),
                metadata,
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
                .context
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
    if terminal(model) && !additions.iter().any(|addition| addition.metadata_member) {
        return Err(input_error(t(
            "恢复活动写服务至少需要一个新的元数据成员；其余新目录可以仅存数据。",
            "Resuming active writes requires at least one new metadata voter; other new directories may be data only.",
        )));
    }
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
#[derive(Clone, Copy)]
enum Submit {
    Commit,
    Resume(AdditionContinuation),
    Cancel,
}
fn resume(state: RwSignal<State>) {
    let continuation = state.with_untracked(|s| {
        s.admin_console
            .storage
            .addition
            .status
            .as_ref()
            .map(|status| status.continuation)
    });
    match continuation {
        Some(continuation) => submit(state, Submit::Resume(continuation)),
        // A lost initial response can precede persistence. Reuse the exact
        // original commit body rather than inventing another operation ID.
        None => submit(state, Submit::Commit),
    }
}
fn submit(state: RwSignal<State>, action: Submit) {
    let captured = state
        .try_update(|s| {
            if !s.session_active() || s.admin_console.storage.conversion.blocks_edit() {
                return None;
            }
            if matches!(action, Submit::Commit)
                && !s.admin_console.storage.addition.submitted
                && !s.admin_console.storage.editable()
            {
                return None;
            }
            let model = &mut s.admin_console.storage.addition;
            if model.sending
                || model
                    .status
                    .as_ref()
                    .is_some_and(|status| status.state.is_terminal())
            {
                return None;
            }
            let operation = model.operation?;
            let expected_preview = model
                .preview
                .clone()
                .or_else(|| model.status.as_ref().map(|status| status.preview.clone()))?;
            let (path, body) = match action {
                Submit::Commit => {
                    if !model.acknowledged && !model.submitted {
                        return None;
                    }
                    (
                        "/api/admin/disks",
                        json!(AdditionCommit {
                            preview: model.preview.clone()?
                        }),
                    )
                }
                Submit::Resume(continuation) => {
                    let status = model.status.as_ref()?;
                    if status.operation_id != operation {
                        return None;
                    }
                    if continuation == AdditionContinuation::Add
                        && status.continuation == AdditionContinuation::Cancel
                        && !model.context.resume_add_acknowledged
                    {
                        return None;
                    }
                    (
                        "/api/admin/disks/addition/resume",
                        json!(AdditionResume {
                            operation_id: operation,
                            continuation,
                        }),
                    )
                }
                Submit::Cancel => {
                    if !can_cancel(model) || !model.context.cancel_acknowledged {
                        return None;
                    }
                    (
                        "/api/admin/disks/addition/cancel",
                        json!(AdditionCancel {
                            operation_id: operation,
                            preview_digest: expected_preview.preview_digest.clone(),
                        }),
                    )
                }
            };
            model.sending = true;
            model.submitted = true;
            model.rejected = false;
            model.error = None;
            model.generation += 1;
            Some(registration_operation::Submission::<AdditionStatus> {
                epoch: s.session_epoch,
                generation: model.generation,
                operation,
                path,
                body,
                preview: expected_preview,
            })
        })
        .flatten();
    if let Some(request) = captured {
        registration_operation::send(state, request);
    }
}
pub(super) fn validate_preview(preview: &AdditionPreview) -> Result<(), api::Error> {
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
        || match &preview.authority {
            AdditionAuthority::ActiveMembership => {
                preview.original_metadata_members.checked_add(
                    preview
                        .request
                        .additions
                        .iter()
                        .filter(|addition| addition.metadata_member)
                        .count(),
                ) != Some(preview.target_metadata_members)
            }
            AdditionAuthority::TerminalManagement {
                terminal_authority_hash,
                terminal_operation_id,
            } => {
                !digest(terminal_authority_hash)
                    || terminal_operation_id == &preview.request.operation_id
                    || preview.target_metadata_members == 0
                    || preview.target_metadata_members
                        != preview
                            .request
                            .additions
                            .iter()
                            .filter(|addition| addition.metadata_member)
                            .count()
            }
        }
        || !digest(&preview.preview_digest)
        || !digest(&preview.configuration_digest)
        || storage_contract::validate_id(&preview.pool_id, "pool").is_err()
    {
        return Err(invalid(ResponseError::Identity));
    }
    Ok(())
}
pub(super) fn validate_status(
    status: &AdditionStatus,
    expected: Option<OperationId>,
) -> Result<(), api::Error> {
    validate_preview(&status.preview)?;
    if expected.is_some_and(|id| id != status.operation_id)
        || status.preview.request.operation_id != status.operation_id
        || (status.preview.authority.is_terminal() && !status.phase.is_metadata_resume())
        || (!status.preview.authority.is_terminal()
            && status.phase.is_terminal_resume_observation())
        || (matches!(
            status.state,
            AdditionState::Cancelling | AdditionState::Cancelled
        ) && status.continuation != AdditionContinuation::Cancel)
        || (matches!(
            status.state,
            AdditionState::Adding | AdditionState::Completed
        ) && status.continuation != AdditionContinuation::Add)
        || (status.state == AdditionState::Completed) != (status.phase == AdditionPhase::Completed)
        || (status.state == AdditionState::Cancelled) != (status.phase == AdditionPhase::Cancelled)
        || status.state.is_terminal() != status.result.is_some()
        || (status.state.is_terminal() && status.failure.is_some())
        || status.result.as_ref().is_some_and(|result| {
            result.operation_id != status.operation_id
                || result.commit_state != storage_contract::CommitState::Published
                || result.request_digest.len() != 64
                || !result
                    .request_digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                || result.kind
                    != if status.preview.authority.is_terminal() {
                        if status.state == AdditionState::Cancelled {
                            "terminal-metadata-resume-cancelled"
                        } else {
                            "terminal-metadata-resume"
                        }
                    } else if status.state == AdditionState::Cancelled {
                        "membership-add-cancelled"
                    } else {
                        "membership-add"
                    }
                || match &status.preview.authority {
                    AdditionAuthority::ActiveMembership => false,
                    AdditionAuthority::TerminalManagement {
                        terminal_authority_hash,
                        ..
                    } => {
                        result
                            .result
                            .get("terminal_authority")
                            .and_then(Value::as_str)
                            != Some(terminal_authority_hash.as_str())
                            || result
                                .result
                                .get("metadata_members")
                                .and_then(Value::as_u64)
                                != Some(status.preview.target_metadata_members as u64)
                    }
                }
        })
    {
        return Err(invalid(ResponseError::Identity));
    }
    Ok(())
}
fn accept(model: &mut Model, status: AdditionStatus) -> bool {
    let completed = status.state.is_terminal();
    if model.status.as_ref() != Some(&status) {
        model.context.cancel_acknowledged = false;
        model.context.resume_add_acknowledged = false;
    }
    model.operation = Some(status.operation_id);
    model.submitted = !completed;
    model.status = Some(status);
    completed
}
fn completed_addition(state: RwSignal<State>) {
    state.update(|s| {
        s.admin_console.storage.selections.clear();
        s.admin_console.storage.wizard.clear_choice();
        s.admin_console.storage.addition.preview = None;
        s.admin_console.storage.addition.acknowledged = false;
        s.files.invalidate_disks();
    });
    super::super::disks::load(state);
    crate::management::load(state);
    super::load(state);
}
pub(super) fn load(state: RwSignal<State>) {
    registration_operation::load::<AdditionStatus>(state);
}
pub(super) fn poll(state: RwSignal<State>) {
    registration_operation::poll::<AdditionStatus>(state);
}
fn edit(state: RwSignal<State>, id: &str, field: &'static str, value: String) {
    state.update(|s| {
        if !s.session_active() || !s.admin_console.storage.editable() {
            return;
        }
        let model = &mut s.admin_console.storage.addition;
        let fields = model
            .context
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
    view! {<label class="zxs-form-field"><span>{label}</span><input class="zxs-input" type="text" autocomplete="off" disabled=move ||state.with(|s|!s.admin_console.storage.editable()) prop:value=move ||state.with(|s|s.admin_console.storage.addition.context.fields.get(&id.get_value()).map(|fields|match key{"slot"=>fields.slot.clone(),"level"=>fields.level.clone(),"upstream"=>fields.upstream.clone(),_=>unreachable!("invariant: addition field")}).unwrap_or_default()) on:input=move |event|edit(state,&id.get_value(),key,event_target_value(&event))/></label>}
}
pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {
        <Show when=move ||state.with(|s|active(&s.admin_console.storage)&&!s.admin_console.storage.selections.is_empty())>
            <h3 class="zxs-row">{t("3. 核对加入方式", "3. Review admission")}</h3>
            <p class="zxs-row">{move ||state.with(|s|if terminal(&s.admin_console.storage) {
                t("从已认证的完整历史恢复当前池。选择全新空目录与槽位；恢复活动写服务至少需要一个新的元数据成员，其余新目录可以仅存数据。保留原池身份、历史与现有登记，不初始化新池。同一文件系统中的不同目录允许使用，但不构成独立磁盘故障冗余。", "Restore this pool from its authenticated complete history. Select fresh empty directories and slots. Resuming active writes requires at least one new metadata voter; other new directories may be data only. The existing pool identity, history and registrations are retained. Different directories on one filesystem are allowed, but do not provide independent disk-failure redundancy.")
            } else { t("向当前存储池登记目录。允许同一文件系统中的不同目录，但它们不提供独立磁盘故障冗余。新增目录不会自动搬移已有文件或改变现有放置规则。", "Register directories in the current pool. Different directories on one filesystem are allowed, but do not provide independent disk-failure redundancy. Adding directories does not move existing files or change placement rules.") })}</p>
            <For each=move ||state.with(|s|s.admin_console.storage.selections.clone()) key=|item|item.selection_id.clone() children=move |selection|{
                let id = StoredValue::new(selection.selection_id);
                view!{<section class="zxs-frame zxs-stack"><h3 class="zxs-row">{selection.root_path}</h3><div class="zxs-form-grid">
                    {field(state,id,"slot",t("新槽位（唯一）", "New slot (unique)"))}
                    {field(state,id,"level",t("层级（1 为主存储）", "Level (1 for primary storage)"))}
                    {field(state,id,"upstream",t("上游槽位（备份层必填）", "Upstream slot (required for backup tiers)"))}
                </div><label class="zxs-row"><input type="checkbox" disabled=move ||state.with(|s|!s.admin_console.storage.editable()) prop:checked=move ||state.with(|s|s.admin_console.storage.addition.context.fields.get(&id.get_value()).is_some_and(|fields|fields.metadata)) on:change=move |event|state.update(|s|{if s.session_active()&&s.admin_console.storage.editable(){let model=&mut s.admin_console.storage.addition;model.context.fields.get_mut(&id.get_value()).expect("invariant: selection fields").metadata=event_target_checked(&event);model.invalidate();}})/>{t("同时保存元数据（不勾选则仅存数据）", "Also store metadata (unchecked means data only)")}</label></section>}
            }/>
            <button type="button" class="zxs-button" disabled=move ||state.with(|s|!s.admin_console.storage.editable()||s.admin_console.storage.selections.is_empty()) on:click=move |_|preview(state)>{move ||state.with(|s|if terminal(&s.admin_console.storage){t("预览恢复元数据组", "Preview metadata group recovery")}else{t("预览在线添加", "Preview online addition")})}</button>
            {move ||state.with(|s|s.admin_console.storage.addition.preview.clone()).map(|preview|view!{
                <p class="zxs-row">{preview_counts(&preview)}</p>
                <p class="zxs-row">{if preview.authority.is_terminal() {
                    t("提交后复制并核验完整历史，由新组真实多数决定并发布后才恢复服务。取消需要新组实际的不可逆阻断决定，已获多数决定的恢复不能撤销；关闭页面不会取消。这里的旧成员数仅是历史认证分母，不表示旧目录重新上线。", "Submitting copies and verifies complete history. Service resumes only after the new group's actual quorum decision and publication. Cancellation requires an actual irreversible blocking decision from the new group. An accepted quorum decision cannot be undone; closing this page does not cancel it. The old member count describes historical authority, not reactivated directories.")
                } else { t("提交后将暂停本代写入，复制并验证控制元数据，再发布新成员。中断后保留原操作编号。关闭页面不会取消；明确申请取消也必须由原成员固定多数的真实决定许可，已经发布的添加不会改称取消。", "Submitting pauses writes for this generation, copies and verifies control metadata, then publishes the new membership. Keep the original operation after interruption. Closing the page does not cancel it. Explicit cancellation requires the original fixed quorum's actual decision; a published addition cannot become cancelled.") }}</p>
                <label class="zxs-row"><input type="checkbox" disabled=move ||state.with(|s|s.admin_console.storage.addition.blocks_edit()) prop:checked=move ||state.with(|s|s.admin_console.storage.addition.acknowledged) on:change=move |event|state.update(|s|{if s.session_active()&&!s.admin_console.storage.addition.blocks_edit(){s.admin_console.storage.addition.acknowledged=event_target_checked(&event);}})/>{t("已核对目录、槽位、层级和元数据角色，确认添加。", "I reviewed the directories, slots, levels and metadata roles and confirm this addition.")}</label>
                <button type="button" class="zxs-button" disabled=move ||state.with(|s|s.admin_console.storage.addition.blocks_edit()||!s.admin_console.storage.addition.acknowledged) on:click=move |_|submit(state, Submit::Commit)>{move ||state.with(|s|if terminal(&s.admin_console.storage){t("恢复当前池的新元数据组", "Restore this pool's metadata group")}else{t("添加到当前存储池", "Add to current pool")})}</button>
            })}
        </Show>
        <Show when=move ||state.with(|s|s.admin_console.storage.addition.submitted)><p class="zxs-row" role="status">{t("存储操作已提交，正在核对服务器阶段。请保留原操作，不要换用新编号重发。", "The storage operation was submitted. Checking its server progress. Keep the original operation; do not resubmit with a new ID.")}</p></Show>
        {move ||state.with(|s|s.admin_console.storage.addition.status.clone()).map(|status|view!{
            <p class="zxs-row" role="status">{status_text(&status)}</p>
            <p class="zxs-row">{preview_counts(&status.preview)}</p>
            {status.failure.map(|failure|view!{<p class="zxs-row" role="alert">{format!("{}: {}",failure.code,failure.message)}</p>})}
        })}
        <Show when=move ||state.with(|s|s.admin_console.storage.addition.submitted&&!s.admin_console.storage.addition.sending)>
            <button type="button" class="zxs-button" on:click=move |_|resume(state)>{move || state.with(|s| if s.admin_console.storage.addition.status.as_ref().is_some_and(|status| status.continuation == AdditionContinuation::Cancel) { t("继续原取消操作", "Continue original cancellation") } else if s.admin_console.storage.addition.status.as_ref().is_some_and(|status| status.preview.authority.is_terminal()) { t("继续原元数据恢复", "Continue original metadata recovery") } else { t("继续原添加操作", "Continue original addition") })}</button>
            <button type="button" class="zxs-button" disabled=move ||state.with(|s|s.admin_console.storage.addition.reading.is_some()) on:click=move |_|load(state)>{t("核对原操作", "Check original operation")}</button>
        </Show>
        <Show when=move ||state.with(|s|can_cancel(&s.admin_console.storage.addition))>
            <p class="zxs-row">{move || state.with(|s| if s.admin_console.storage.addition.status.as_ref().is_some_and(|status| status.preview.authority.is_terminal()) {
                t("取消元数据组恢复需要新组持久化足以阻断其固定多数的互斥取消决定。已有多数决定不可撤销；取消不删除新目录和准备材料，也不重新打开旧目录。", "Cancelling metadata group recovery requires immutable decisions that block its fixed new quorum. An accepted quorum decision cannot be reversed. Cancellation preserves new directories and preparation data and does not reopen old directories.")
            } else {
                t("取消仅适用于尚能取得原成员固定多数取消票的添加。已形成联合多数时不可取消；取消不会删除新目录或已写入的准备材料。", "Cancellation requires the original fixed quorum's Abort votes. An existing joint quorum cannot be cancelled. Cancellation does not delete new directories or preparation data.")
            })}</p>
            <label class="zxs-row"><input type="checkbox" prop:checked=move ||state.with(|s|s.admin_console.storage.addition.context.cancel_acknowledged) on:change=move |event|state.update(|s|{if s.session_active(){s.admin_console.storage.addition.context.cancel_acknowledged=event_target_checked(&event);}})/>{t("确认申请取消此原添加操作，并保留所有目录数据。", "I confirm cancellation of this original addition, retaining all directory data.")}</label>
            <button type="button" class="zxs-button" disabled=move ||state.with(|s|!s.admin_console.storage.addition.context.cancel_acknowledged) on:click=move |_|submit(state, Submit::Cancel)>{t("申请取消原添加", "Request original addition cancellation")}</button>
            <Show when=move ||state.with(|s|s.admin_console.storage.addition.status.as_ref().is_some_and(|status|status.continuation==AdditionContinuation::Cancel&&status.state==AdditionState::RecoveryRequired))>
                <p class="zxs-row">{t("取消失败不会自动转回添加。若改为继续添加，已持久化的取消票仍可能拒绝此操作。", "A failed cancellation never automatically resumes addition. If you choose addition instead, existing durable Abort votes can still prevent it.")}</p>
                <label class="zxs-row"><input type="checkbox" prop:checked=move ||state.with(|s|s.admin_console.storage.addition.context.resume_add_acknowledged) on:change=move |event|state.update(|s|{if s.session_active(){s.admin_console.storage.addition.context.resume_add_acknowledged=event_target_checked(&event);}})/>{t("我明确改为继续原添加。", "I explicitly choose to continue the original addition instead.")}</label>
                <button type="button" class="zxs-button" disabled=move ||state.with(|s|!s.admin_console.storage.addition.context.resume_add_acknowledged) on:click=move |_|submit(state, Submit::Resume(AdditionContinuation::Add))>{t("改为继续原添加", "Continue addition instead")}</button>
            </Show>
        </Show>
        {move ||state.with(|s|s.admin_console.storage.addition.error.clone()).map(|error|{let feedback=crate::ui::feedback(&error);view!{<ErrorState message=feedback.message request_id=feedback.request_id/>}})}
    }
}
fn preview_counts(preview: &AdditionPreview) -> String {
    if preview.authority.is_terminal() {
        format!(
            "{}{} · {}{} · {}{}/{}",
            t("新目录：", "Fresh directories: "),
            preview.added_directories,
            t("原历史成员：", "Historical members: "),
            preview.original_metadata_members,
            t("新组 N/Q：", "New group N/Q: "),
            preview.target_metadata_members,
            preview.target_metadata_members / 2 + 1
        )
    } else {
        format!(
            "{}{} · {}{} → {}",
            t("新增目录：", "New directories: "),
            preview.added_directories,
            t("元数据成员：", "Metadata members: "),
            preview.original_metadata_members,
            preview.target_metadata_members
        )
    }
}
fn status_text(status: &AdditionStatus) -> String {
    let (state, phase) = if status.preview.authority.is_terminal() {
        let state = match status.state {
            AdditionState::Completed => {
                t("元数据组恢复已发布", "Metadata group recovery published")
            }
            AdditionState::RecoveryRequired => t("需要继续原恢复", "Continue original recovery"),
            AdditionState::Adding => t("元数据组恢复中", "Recovering metadata group"),
            AdditionState::Cancelling => {
                t("申请取消元数据组恢复", "Cancelling metadata group recovery")
            }
            AdditionState::Cancelled => {
                t("元数据组恢复已取消", "Metadata group recovery cancelled")
            }
        };
        let phase = if status.phase == AdditionPhase::JointDecision {
            t(
                "新组多数决定与完整历史同步",
                "New quorum decision and complete history synchronization",
            )
        } else if status.phase == AdditionPhase::CertifyingAbort {
            t(
                "认证新组不可逆取消决定",
                "Certifying the new group's irreversible cancellation",
            )
        } else if status.phase == AdditionPhase::Cancelling {
            t("持久化取消证明", "Persisting cancellation evidence")
        } else {
            phase_text(status.phase)
        };
        (state, phase)
    } else {
        (state_text(status.state), phase_text(status.phase))
    };
    format!("{} · {} · {}", status.operation_id, state, phase)
}
fn state_text(state: AdditionState) -> String {
    match state {
        AdditionState::Adding => t("添加中", "Adding"),
        AdditionState::Cancelling => t("取消中", "Cancelling"),
        AdditionState::Cancelled => t("取消已发布", "Cancellation published"),
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
        AdditionPhase::ResumeCancellable => t("已核验可取消", "Cancellation verified available"),
        AdditionPhase::ResumeQuorumAccepted => t(
            "新多数已决定，仅可继续原恢复",
            "New quorum decided; continue the original recovery",
        ),
        AdditionPhase::ResumeCancellationUnknown => t(
            "需恢复原操作以核验取消状态",
            "Recover the original operation to check cancellation",
        ),
        AdditionPhase::Activating => t("发布新成员", "Publishing membership"),
        AdditionPhase::CertifyingAbort => {
            t("认证原成员取消票", "Certifying original-member Abort votes")
        }
        AdditionPhase::AbortCertified => t(
            "取消已获认证，等待发布",
            "Abort certified, publication pending",
        ),
        AdditionPhase::Cancelling => t("发布取消代", "Publishing cancellation generation"),
        AdditionPhase::Cancelled => t(
            "原添加已永久取消",
            "Original addition permanently cancelled",
        ),
        AdditionPhase::Completed => t("已发布", "Published"),
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/admin/storage_addition_tests.rs"]
mod tests;
