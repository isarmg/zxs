//! Read-only candidate discovery precedes every user-directed admission. A source
//! observation is never converted into an empty-directory admission proof.
use super::*;
use std::collections::BTreeSet;
use storage_contract::storage_directories::{DiscoveredDisk, DiscoveredDiskState, DiscoveredDisks};

#[derive(Clone, Default)]
pub(super) struct Model {
    discovery: Option<DiscoveredDisks>,
    choice: Option<String>,
    generation: u64,
    reading: bool,
    changed: bool,
    error: Option<api::Error>,
}
impl Model {
    pub(super) fn busy(&self) -> bool {
        self.reading
    }
    pub(super) fn failed(&self) -> bool {
        self.error.is_some()
    }
    pub(super) fn clear_choice(&mut self) {
        self.choice = None;
    }
    pub(super) fn candidates(&self) -> Vec<DiscoveredDisk> {
        self.discovery
            .as_ref()
            .map(|discovery| discovery.candidates.clone())
            .unwrap_or_default()
    }
    pub(super) fn selected(&self, path: &str) -> bool {
        self.choice.as_deref() == Some(path)
    }
    pub(super) fn chosen(&self) -> Option<&DiscoveredDisk> {
        let path = self.choice.as_ref()?;
        self.discovery
            .as_ref()?
            .candidates
            .iter()
            .find(|candidate| candidate.path == *path)
    }
}

fn immediate_child(root: &str, path: &str, name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\0'])
        && path.strip_prefix(root).is_some_and(|suffix| {
            if root == "/" {
                suffix == name
            } else {
                suffix.strip_prefix('/') == Some(name)
            }
        })
}
fn read(value: Value) -> Result<DiscoveredDisks, api::Error> {
    let discovery: DiscoveredDisks = decode(value)?;
    let root = &discovery.discovery_root;
    if !root.starts_with('/')
        || root.contains('\0')
        || (root != "/" && root.ends_with('/'))
        || root.split('/').any(|part| matches!(part, "." | ".."))
    {
        return Err(invalid(ResponseError::Identity));
    }
    let mut paths = BTreeSet::new();
    for candidate in &discovery.candidates {
        if !immediate_child(root, &candidate.path, &candidate.name)
            || !paths.insert(&candidate.path)
        {
            return Err(invalid(ResponseError::Identity));
        }
        match &candidate.state {
            DiscoveredDiskState::Empty { selection } => {
                if selection.root_path != candidate.path
                    || selection.discovery_root != *root
                    || !selection
                        .selection_id
                        .strip_prefix("directory_")
                        .is_some_and(|digest| {
                            digest.len() == 64
                                && digest.bytes().all(|byte| {
                                    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
                                })
                        })
                {
                    return Err(invalid(ResponseError::Identity));
                }
            }
            DiscoveredDiskState::Zxs {
                disk_id,
                slot_id,
                role,
                source,
            } => {
                if disk_id.is_empty()
                    || name_policy::validate_slot(slot_id).is_err()
                    || source.path != candidate.path
                    || role.level == 0
                    || (role.level == 1) != role.upstream_slot_id.is_none()
                    || role.upstream_slot_id.as_ref().is_some_and(|upstream| {
                        name_policy::validate_slot(upstream).is_err()
                            || upstream.eq_ignore_ascii_case(slot_id)
                    })
                {
                    return Err(invalid(ResponseError::Identity));
                }
            }
            DiscoveredDiskState::Unsupported { source, .. } => {
                if source
                    .as_ref()
                    .is_some_and(|source| source.path != candidate.path)
                {
                    return Err(invalid(ResponseError::Identity));
                }
            }
            DiscoveredDiskState::Unavailable { .. } => {}
        }
    }
    Ok(discovery)
}
fn discoverable(model: &super::Model) -> bool {
    model.snapshot.as_ref().is_some_and(|snapshot| {
        !snapshot.completion_available
            && matches!(
                snapshot.state,
                StorageState::Pending | StorageState::Active | StorageState::TerminalManagement
            )
    })
}
pub(super) fn load(state: RwSignal<State>) {
    let captured = state
        .try_update(|s| {
            if !s.session_active() {
                return None;
            }
            let model = &mut s.admin_console.storage;
            if model.busy || model.reload_blocked() || model.wizard.reading || !discoverable(model)
            {
                return None;
            }
            model.confirmed = false;
            model.addition.invalidate();
            model.conversion.invalidate();
            model.foreign.invalidate();
            model.wizard.reading = true;
            model.wizard.error = None;
            model.wizard.generation += 1;
            Some((s.session_epoch, model.wizard.generation))
        })
        .flatten();
    let Some((epoch, generation)) = captured else {
        return;
    };
    spawn_local(async move {
        let result =
            api::validated_request("GET", "/api/admin/storage/discovered-disks", None, read).await;
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        state.update(|s| {
            if !s.session_active()
                || s.session_epoch != epoch
                || s.admin_console.storage.wizard.generation != generation
            {
                return;
            }
            finish(&mut s.admin_console.storage, result);
        });
    });
}
fn finish(model: &mut super::Model, result: Result<DiscoveredDisks, api::Error>) {
    model.wizard.reading = false;
    match result {
        Ok(discovery) => {
            let previous = model.selections.len();
            model.selections.retain(|selected| discovery.candidates.iter().any(|candidate| {
                matches!(&candidate.state, DiscoveredDiskState::Empty { selection } if selection == selected)
            }));
            model.wizard.changed = model.selections.len() != previous;
            if model.wizard.choice.as_ref().is_some_and(|path| {
                !discovery
                    .candidates
                    .iter()
                    .any(|candidate| candidate.path == *path)
            }) {
                model.wizard.clear_choice();
            }
            model.wizard.discovery = Some(discovery);
            model.wizard.error = None;
            addition::selected(model);
        }
        Err(error) => model.wizard.error = Some(error),
    }
}
pub(super) fn choose(model: &mut super::Model, path: &str) {
    if model.editable()
        && !model.wizard.selected(path)
        && model.wizard.discovery.as_ref().is_some_and(|discovery| {
            discovery
                .candidates
                .iter()
                .any(|candidate| candidate.path == path)
        })
    {
        model.conversion.invalidate();
        model.foreign.invalidate();
        model.wizard.choice = Some(path.into());
    }
}
fn add(model: &mut super::Model) {
    if !model.editable() || model.selections.len() >= 32 {
        return;
    }
    let Some(DiscoveredDisk {
        state: DiscoveredDiskState::Empty { selection },
        ..
    }) = model.wizard.chosen()
    else {
        return;
    };
    if !model
        .selections
        .iter()
        .any(|selected| selected.selection_id == selection.selection_id)
    {
        model.selections.push(selection.clone());
        model.confirmed = false;
        addition::selected(model);
    }
    model.wizard.clear_choice();
}
fn remove(model: &mut super::Model, id: &str) {
    if model.editable() {
        model
            .selections
            .retain(|selected| selected.selection_id != id);
        model.confirmed = false;
        addition::selected(model);
    }
}
pub(super) fn kind(state: &DiscoveredDiskState) -> String {
    match state {
        DiscoveredDiskState::Empty { .. } => t("空目录", "Empty directory"),
        DiscoveredDiskState::Zxs { .. } => t("ZXS 格式", "ZXS format"),
        DiscoveredDiskState::Unsupported { .. } => t("需要处理", "Needs processing"),
        DiscoveredDiskState::Unavailable { .. } => t("不可用", "Unavailable"),
    }
}

fn choice_view(state: RwSignal<State>, candidate: DiscoveredDisk) -> impl IntoView {
    view! {
        <section class="zxs-frame zxs-stack" data-storage-wizard-choice=candidate.path.clone()>
            <h3 class="zxs-row">{t("2. 处理所选磁盘", "2. Process the selected disk")}</h3>
            <p class="zxs-row">{candidate.path.clone()}</p>
            {match candidate.state {
                DiscoveredDiskState::Empty { .. } => view! {
                    <p class="zxs-row">{t("已只读核验为空目录。下一步仅加入待确认列表；提交时会再次核验目录与磁盘身份，再创建 ZXS 数据。", "Read-only inspection found an empty directory. The next step only stages it for review. Submission revalidates directory and disk identity before creating ZXS data.")}</p>
                    <button type="button" class="zxs-button" data-storage-add-selection="" disabled=move ||state.with(|s|!s.admin_console.storage.editable()||s.admin_console.storage.selections.len()>=32)
                        on:click=move |_|state.update(|s|{if s.session_active(){add(&mut s.admin_console.storage);}})>{t("选择此空目录", "Select this empty directory")}</button>
                }.into_any(),
                DiscoveredDiskState::Zxs { disk_id, slot_id, role, .. } => view! {
                    <p class="zxs-row">{format!("{}: {slot_id} · {disk_id}", t("原磁盘身份", "Original disk identity"))}</p>
                    <p class="zxs-row">{format!("{}: {} · {}: {}",t("来源层级", "Source tier"),role.level,t("证据", "Evidence"),match role.evidence {
                        storage_contract::storage_directories::DiskRoleEvidence::InitialDeclaration => t("原盘声明（未证明完整历史）", "Original declaration (not complete history)"),
                        storage_contract::storage_directories::DiskRoleEvidence::GenerationEvidence => t("已核验成员代际（未证明完整历史）", "Verified membership generation (not complete history)"),
                    })}</p>
                    <p class="zxs-row">{if role.metadata_member {t("来源声明为元数据成员。", "The source declares metadata membership.")}else{t("来源声明为数据成员。", "The source declares a data-only role.")}}</p>
                    {role.upstream_slot_id.map(|upstream|view!{<p class="zxs-row">{format!("{}: {upstream}",t("来源上游槽位", "Source upstream slot"))}</p>})}
                    <p class="zxs-row" role="status">{t("已识别 ZXS 格式与来源角色，但完整历史迁入与重建尚未通过接入验证。此目录不能按空盘初始化，现有内容与历史保持不变。", "ZXS format and source role are recognized, but full-history migration and reconstruction have not passed admission verification. This directory cannot be initialized as empty; existing contents and history are unchanged.")}</p>
                }.into_any(),
                DiscoveredDiskState::Unsupported { code, message, .. } => view! {
                    <p class="zxs-row" role="alert">{format!("{code}: {message}")}</p>
                    <p class="zxs-row">{t("未将此目录当作空盘。转换必须先核对真实转换计划和目标位置，不能直接加入。", "This directory is not treated as empty. Conversion requires a verified plan and target before admission.")}</p>
                    {conversion::form(state)}
                }.into_any(),
                DiscoveredDiskState::Unavailable { problem } => view! {
                    <p class="zxs-row" role="alert">{match problem {
                        storage_contract::storage_directories::DirectoryProblem::SymbolicLink => t("符号链接不能作为磁盘候选。", "Symbolic links cannot be disk candidates."),
                        storage_contract::storage_directories::DirectoryProblem::PermissionDenied => t("无权读取此目录。请检查主机上的目录权限。", "This directory cannot be read. Check its host permissions."),
                        storage_contract::storage_directories::DirectoryProblem::Unavailable => t("此目录当前不可用，请刷新候选。", "This directory is unavailable. Refresh candidates."),
                    }}</p>
                }.into_any(),
            }}
            <button type="button" class="zxs-button" data-storage-wizard-back="" disabled=move ||state.with(|s|!s.admin_console.storage.editable())
                on:click=move |_|state.update(|s|{if s.session_active()&&s.admin_console.storage.editable(){s.admin_console.storage.conversion.invalidate();s.admin_console.storage.foreign.invalidate();s.admin_console.storage.wizard.clear_choice();}})>{t("返回候选磁盘", "Back to disk candidates")}</button>
        </section>
    }
}
fn initial_review(state: RwSignal<State>) -> impl IntoView {
    view! {
        <Show when=move ||state.with(|s|s.admin_console.storage.snapshot.as_ref().is_some_and(|snapshot|snapshot.state==StorageState::Pending)&&!s.admin_console.storage.selections.is_empty())>
            <h3 class="zxs-row">{t("3. 核对并加入", "3. Review and admit")}</h3>
            <p class="zxs-row">{t("每个目录将登记为独立的第 1 层主盘（D01、D02…），不会按选择顺序创建备份关系。全部作为元数据成员，正常写入需要所有成员在线。同一设备上的多个目录不提供磁盘故障冗余。", "Each directory becomes an independent tier-1 primary (D01, D02…). Selection order does not create backup relationships. All are metadata members and must be online for normal writes. Directories on one device do not protect against disk failure.")}</p>
            <p class="zxs-row">{move ||state.with(|s|{let count=s.admin_console.storage.selections.len();format!("{}: {count} · {}: {}",t("元数据成员", "Metadata members"),t("恢复所需一致原成员", "Consistent original members required for recovery"),count/2+1)})}</p>
            <Show when=move ||state.with(|s|s.admin_console.storage.snapshot.as_ref().is_some_and(|snapshot|snapshot.allow_development))>
                <label class="zxs-row"><input data-storage-development="" type="checkbox" prop:checked=move ||state.with(|s|s.admin_console.storage.development) disabled=move ||state.with(|s|!s.admin_console.storage.editable())
                    on:change=move |event|state.update(|s|{if s.session_active()&&s.admin_console.storage.editable(){s.admin_console.storage.development=event_target_checked(&event);s.admin_console.storage.confirmed=false;}})/>{t("开发模拟目录（非生产）", "Development simulation directories (not production)")}</label>
            </Show>
            <label class="zxs-row"><input data-storage-confirm="" type="checkbox" prop:checked=move ||state.with(|s|s.admin_console.storage.confirmed) disabled=move ||state.with(|s|!s.admin_console.storage.editable())
                on:change=move |event|state.update(|s|{if s.session_active()&&s.admin_console.storage.editable(){s.admin_console.storage.confirmed=event_target_checked(&event);}})/>{t("确认在以上空目录创建 ZXS 身份与元数据，并加入新存储池。", "Create ZXS identities and metadata in the empty directories above and admit them to the new pool.")}</label>
            <button type="button" class="zxs-button" data-storage-activate="" disabled=move ||state.with(|s|!s.admin_console.storage.can_activate()) on:click=move |_|super::activate(state,false)>{t("确认加入并激活", "Confirm admission and activate")}</button>
        </Show>
    }
}
pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {
        <Show when=move ||state.with(|s|discoverable(&s.admin_console.storage))>
            <section class="zxs-stack" id="zxs-disk-wizard" tabindex="-1" data-storage-wizard="" aria-label=t("磁盘向导", "Disk wizard")>
                <h3 class="zxs-row">{t("1. 选择自动发现的磁盘", "1. Choose an automatically discovered disk")}</h3>
                <p class="zxs-row">{t("在下方磁盘树的 -1 层选择候选，打开处理步骤。发现只读取信息，不会自动加入或修改内容。", "Choose a candidate in tier -1 of the disk tree below to open its processing steps. Discovery only reads information; it never admits or changes contents automatically.")}</p>
                <button type="button" class="zxs-button" data-storage-discover="" disabled=move ||state.with(|s|s.admin_console.storage.busy||s.admin_console.storage.reload_blocked()||s.admin_console.storage.wizard.reading) on:click=move |_|load(state)>{t("刷新候选磁盘", "Refresh disk candidates")}</button>
                {move ||state.with(|s|s.admin_console.storage.wizard.discovery.clone()).map(|discovery|view!{
                    <p class="zxs-row">{format!("{}: {}",t("发现父目录", "Discovery parent"),discovery.discovery_root)}</p>
                    <Show when=move ||discovery.candidates.is_empty()><p class="zxs-row" role="status">{t("未发现未登记候选目录。", "No unregistered candidate directories were found.")}</p></Show>
                    <Show when=move ||{discovery.truncated||discovery.skipped_non_utf8>0}><p class="zxs-row" role="status">{t("候选清单不完整，部分目录超出扫描范围或名称无法显示。请核对主机目录，勿视为全部磁盘。", "The candidate inventory is incomplete: some directories exceed its scan bound or have undisplayable names. Check the host; this is not a complete disk inventory.")}</p></Show>
                })}
                {move ||state.with(|s|s.admin_console.storage.wizard.chosen().cloned()).map(|candidate|choice_view(state,candidate))}
                {foreign::view(state)}
                <Show when=move ||state.with(|s|s.admin_console.storage.wizard.changed)><p class="zxs-row" role="alert">{t("部分候选身份或状态已变化，已移除对应待加入项。请重新核对。", "Some candidate identities or states changed. Their staged selections were removed; review them again.")}</p></Show>
                <For each=move ||state.with(|s|s.admin_console.storage.selections.clone()) key=|selection|selection.selection_id.clone() children=move |selection|{
                    let id=StoredValue::new(selection.selection_id);
                    view!{<div class="zxs-flow" data-storage-selected=selection.root_path.clone()><p class="zxs-row">{selection.root_path.clone()}</p>
                        <button type="button" class="zxs-button" disabled=move ||state.with(|s|!s.admin_console.storage.editable()) on:click=move |_|state.update(|s|{if s.session_active(){remove(&mut s.admin_console.storage,&id.get_value());}})>{t("移除待加入项", "Remove staged directory")}</button>
                    </div>}
                }/>
                {initial_review(state)}
                {move ||state.with(|s|s.admin_console.storage.wizard.error.clone()).map(|error|{let feedback=crate::ui::feedback(&error);view!{<ErrorState message=feedback.message request_id=feedback.request_id/>}})}
            </section>
        </Show>
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/admin/storage_wizard_tests.rs"]
pub(super) mod tests;
