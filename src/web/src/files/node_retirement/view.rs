use super::*;

fn choices(state: &State) -> Vec<(Target, String, String, bool)> {
    let Some(target) = Target::capture(state) else {
        return Vec::new();
    };
    let current = state
        .files
        .selected
        .as_ref()
        .filter(|entry| !entry.retained_history())
        .and_then(|entry| entry.version_id());
    let mut rows = BTreeMap::new();
    for node in state.files.file_history.nodes() {
        let Some(version) = node.version_id() else {
            continue;
        };
        if matches!(
            node.content_state(),
            Some(
                storage_contract::ContentState::Migrated | storage_contract::ContentState::Retired
            )
        ) {
            continue;
        }
        let label = format!(
            "{} · {} · {}",
            node.path().unwrap_or(&target.path),
            node.note(),
            version
        );
        rows.entry(version.to_owned()).or_insert((
            target.clone(),
            version.to_owned(),
            label,
            current == Some(version),
        ));
    }
    rows.into_values().collect()
}
fn can_edit(state: &State) -> bool {
    admission_ready(state)
        && !state.files.node_retirement.loading
        && state.files.node_retirement.attempt.is_none()
        && state.files.node_retirement.recovery.ready_for_new(state)
}
fn can_submit(state: &State) -> bool {
    let model = &state.files.node_retirement;
    can_edit(state)
        && model.confirmed
        && model.preview.as_ref().is_some_and(|preview| {
            preview
                .affected_branch_heads
                .iter()
                .all(|branch| preview.archived_branch_heads.contains(branch))
                && preview.frozen_files.is_empty()
                && model
                    .selection()
                    .is_some_and(|selection| preview.validate(&selection).is_ok())
        })
}
fn summary(state: &State) -> Option<String> {
    let attempt = state.files.node_retirement.attempt.as_ref()?;
    let text = match status(state, attempt) {
        SaveStatus::Unknown if attempt.resume_only => t(
            "此恢复入口只允许继续已经发布的原操作。当前会话尚未核到准确发布收据，只可查询，不能重发原删除请求。",
            "This recovery entry can only resume an already published operation. This session has not confirmed its exact publication receipt; only receipt lookup is allowed, never resubmission of the original deletion.",
        ),
        SaveStatus::Unknown => t(
            "发布结果未确认。只可查询或重试原操作，不得换新 ID 重提。",
            "Publication is unconfirmed. Query or retry only this original operation; do not submit a new ID.",
        ),
        SaveStatus::Rejected => t(
            "本次请求被拒绝，没有确认任何退役发布。",
            "This request was rejected; no retirement publication was confirmed.",
        ),
        SaveStatus::Published | SaveStatus::Degraded => {
            if attempt
                .cleanup
                .as_ref()
                .is_some_and(Cleanup::all_required_bindings_absent)
            {
                t(
                    "节点已永久退役；本次观察确认原决定记录的副本义务及当前路由正文均缺失。不是离线旧盘、外部备份或旧认证元数据的擦除证明。",
                    "Nodes are permanently retired; this observation found no body on the original recorded bindings or current routes. It does not prove erasure of offline disks, external backups or old authentication metadata.",
                )
            } else {
                t(
                    "节点退役已经发布，不可撤销；正文清理仍待完成或核验。请使用原操作继续清理。",
                    "Retirement was published and cannot be undone. Body cleanup is still pending or unverified. Continue cleanup using the original operation.",
                )
            }
        }
    };
    Some(format!(
        "{}\n{} · {}\n{}",
        text, attempt.target.path, attempt.target.file, attempt.operation
    ))
}
fn replica_label(replica: &wire::Replica) -> String {
    let label = match &replica.status {
        wire::ReplicaState::NotRequested => t("未获清理授权", "Cleanup not requested"),
        wire::ReplicaState::MissingTombstone => t(
            "缺少认证墓碑，未确认清理",
            "Authenticated tombstone missing; cleanup unverified",
        ),
        wire::ReplicaState::PayloadPresent => t("正文仍存在", "Body is still present"),
        wire::ReplicaState::VerifiedAbsent => t("已观察到正文缺失", "Body observed absent"),
        wire::ReplicaState::Unverified { error_code } => {
            format!("{}: {}", t("未核验", "Unverified"), error_code)
        }
    };
    format!(
        "{} · {} @ {} · {} · {}",
        replica.version_id,
        replica.slot_id,
        replica.binding_generation,
        if replica.original_requirement {
            t("原决定义务", "Original obligation")
        } else {
            t("新增当前路由", "Additional current route")
        },
        label
    )
}
pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    Effect::new(move |_| {
        let target = state.with(Target::capture);
        if state.with_untracked(|s| s.files.node_retirement.target != target) {
            state.update(|s| s.files.node_retirement.retarget(target));
        }
    });
    let choices = Memo::new(move |_| state.with(choices));
    view! {
        <details class="zxs-entry-section zxs-stack" data-zxs-node-retirement="true">
            <summary class="zxs-row">{t("永久退役历史节点", "Permanently retire historical nodes")}</summary>
            <p class="zxs-row">{t("此操作不可撤销：移除所选正文及复用它的事件，并清理正文。旧节点描述和提交证明仍保留作认证。只有跨文件共享关系全部消失才拆分联合图；单文件内部不拆分。", "This cannot be undone: selected bodies and their coupled events leave the live history, and their payloads are cleaned up. Old descriptors and commit proofs remain for authentication. Files separate only after all cross-file sharing paths disappear; internal deletion never splits a file.")}</p>
            <p class="zxs-row">{t("当前 namespace、D、目录/Git、工作区或读取租约仍会阻止执行。已经脱离这些引用的文件可把完整保留分支 HEAD/合并前沿永久归档，与最后节点一起终止；原 file_id 不可复活。预览不是执行许可或擦除证明。", "Current namespace, D, directory/Git references, workspaces and read leases still block execution. A detached file can permanently archive its complete branch HEAD/merge frontier with its last nodes; its original file_id cannot revive. Preview is not execution permission or erasure proof.")}</p>
            <p class="zxs-row">{t("提交前会在本浏览器保存账号、文件名、准确版本和原操作 ID，用于刷新后的恢复。记录无法保存时不会发送永久操作；未解决的记录请在上方恢复面板处理。", "Before submitting, this browser saves the account, file label, exact versions and original operation ID for recovery after reload. No permanent request is sent if the record cannot be saved. Resolve retained records in the recovery panel above.")}</p>
            <Show when=move ||state.with(|s|s.files.node_retirement.attempt.is_none())>
                <fieldset class="zxs-stack" disabled=move ||!state.with(can_edit)>
                    <legend class="zxs-row">{t("从当前历史页显式选择正文版本", "Explicitly select body versions from this history page")}</legend>
                    <For each=move ||choices.get() key=|row|row.1.clone() children=move |row| {
                        let row=StoredValue::new(row);
                        view!{<label class="zxs-row zxs-flow">
                            <input type="checkbox" disabled=move ||row.with_value(|row|row.3)||!state.with(can_edit)
                                prop:checked=move ||row.with_value(|row|state.with(|s|s.files.node_retirement.selected.contains_key(&row.1)))
                                on:change=move |event|row.with_value(|row|checkbox(state,&row.0,&row.1,&row.2,event_target_checked(&event)))/>
                            <span>{row.with_value(|row|row.2.clone())}</span>
                            {row.with_value(|row|row.3).then(||view!{<span>{t("当前正文不可退役", "Current body cannot be retired")}</span>})}
                        </label>}
                    }/>
                </fieldset>
                <p class="zxs-row">{move ||state.with(|s|format!("{} {}",t("已选版本数：", "Selected versions:"),s.files.node_retirement.selected.len()))}</p>
                <details class="zxs-stack"><summary class="zxs-row">{t("检查所有已选版本（包括其他历史页）", "Review all selected versions, including other pages")}</summary>
                    {move ||state.with(|s|s.files.node_retirement.selected.values().cloned().collect::<Vec<_>>()).into_iter().map(|label|view!{<p class="zxs-row">{label}</p>}).collect_view()}
                </details>
                <div class="zxs-flow">
                    <button class="zxs-button" disabled=move ||!state.with(can_edit)||state.with(|s|s.files.node_retirement.selected.is_empty()) on:click=move |_|action::preview(state)>{t("预览准确选择的影响", "Preview this exact selection")}</button>
                    <button class="zxs-button" disabled=move ||!state.with(can_edit) on:click=move |_|state.update(|s|{let m=&mut s.files.node_retirement;m.selected.clear();m.preview=None;m.confirmed=false;})>{t("清空选择", "Clear selection")}</button>
                </div>
                {move ||state.with(|s|s.files.node_retirement.preview.clone()).map(|preview|view!{
                    <div class="zxs-stack" role="status">
                        <p class="zxs-row">{format!("{} {} · {} {} · {} {}",t("正文版本", "Body versions"),preview.version_ids.len(),t("耦合事件", "Coupled events"),preview.coupled_node_ids.len(),t("退役后文件分组", "File components after retirement"),preview.file_components.len())}</p>
                        <p class="zxs-row">{format!("{} {}",t("保留的原提交引用：", "Original commit references retained:"),preview.retained_commit_entries)}</p>
                        <p class="zxs-row">{format!("{} {}",t("可条件归档的完整文件分支 HEAD 数：", "Conditional complete-file branch HEAD archives:"),preview.archived_branch_heads.len())}</p>
                        <p class="zxs-row">{format!("{} {}",t("选择覆盖最后有效版本的文件（仍须脱离引用）：", "Files whose last live versions are selected (references must still be detached):"),preview.last_live_files.iter().map(ToString::to_string).collect::<Vec<_>>().join(" · "))}</p>
                        {preview.file_components.into_iter().map(|component|view!{<p class="zxs-row">{component.iter().map(ToString::to_string).collect::<Vec<_>>().join(" · ")}</p>}).collect_view()}
                        {(preview.affected_branch_heads.iter().any(|branch| !preview.archived_branch_heads.contains(branch))||!preview.frozen_files.is_empty()).then(||view!{<p class="zxs-row">{t("当前选择包含不可归档的活动 HEAD 或 D 冻结文件，不能执行。", "This selection includes a non-archivable live HEAD or D-frozen file and cannot execute.")}</p>})}
                    </div>
                })}
                <label class="zxs-row zxs-flow"><input type="checkbox" disabled=move ||!state.with(can_edit)||state.with(|s|s.files.node_retirement.preview.is_none()) prop:checked=move ||state.with(|s|s.files.node_retirement.confirmed)
                    on:change=move |event|{let checked=event_target_checked(&event);state.update(|s|{if can_edit(s)&&s.files.node_retirement.preview.is_some(){s.files.node_retirement.confirmed=checked;}});}/>
                    <span>{t("我确认永久退役上述准确版本及耦合事件，并清理正文；此操作不可撤销，旧认证证据和外部副本不在抹除承诺内。", "I confirm permanent retirement of these exact versions and coupled events, and body cleanup. This cannot be undone; old authentication evidence and external copies are not promised erased.")}</span>
                </label>
                <button class="zxs-button" data-zxs-confirm-node-retirement="true" disabled=move ||!state.with(can_submit) on:click=move |_|action::submit(state)>{t("确认永久退役并清理正文", "Confirm permanent retirement and body cleanup")}</button>
            </Show>
            <Show when=move ||state.with(|s|s.files.node_retirement.loading)><p class="zxs-row" role="status">{t("正在计算当前根的完整条件影响…", "Computing complete conditional impact at the current root…")}</p></Show>
            {move ||state.with(|s|s.files.node_retirement.error.clone()).map(|error|view!{<p class="zxs-row" role="alert">{error.to_string()}</p>})}
        </details>
    }
}

/// Mounted outside file navigation, so a terminal/unlisted file never strands
/// its original operation. Controls carry the operation they were rendered for.
pub(super) fn attempt_view(state: RwSignal<State>) -> impl IntoView {
    view! {
        <For each=move ||state.with(|s|s.files.node_retirement.attempt.as_ref().map(|a|(a.target.epoch,a.operation.clone())).into_iter().collect::<Vec<_>>()) key=|id|id.clone() children=move |(epoch,id)| {
            let operation = StoredValue::new(id);
            view! {
                <div class="zxs-stack">
            {move ||state.with(summary).map(|message|view!{<p class="zxs-row" role="status">{message}</p>})}
            {move ||state.with(|s|s.files.node_retirement.attempt.as_ref().and_then(|a|a.separation.clone())).map(|partition|match partition {
                wire::Separation::Published { history_root, original_files, terminated_files, file_components } => view! {
                    <details class="zxs-stack"><summary class="zxs-row">{t("原退役决定的真实文件分组", "File groups published by the original retirement")}</summary>
                        <p class="zxs-row">{format!("{} {} · {} {}", t("原共享分量文件数", "Files in the original component"), original_files.len(), t("发布后分组数", "Groups at publication"), file_components.len())}</p>
                        {file_components.into_iter().map(|group|view!{<p class="zxs-row">{group.iter().map(ToString::to_string).collect::<Vec<_>>().join(" · ")}</p>}).collect_view()}
                        <p class="zxs-row">{format!("{} {}", t("已终止的原文件身份：", "Terminated original file identities:"), terminated_files.iter().map(ToString::to_string).collect::<Vec<_>>().join(" · "))}</p>
                        <p class="zxs-row">{format!("{} {}", t("原来源根：", "Original source root:"), history_root)}</p>
                        <p class="zxs-row">{t("此为原操作发布时的准确结果，不是清理证明。之后新增相同内容或继续退役可能改变当前联合图，不改写此原收据。", "This is the exact original publication result, not cleanup proof. Later matching saves or further retirements may change the live graph without rewriting this receipt.")}</p>
                    </details>
                }.into_any(),
                wire::Separation::Unverified { error_code } => view!{<p class="zxs-row">{format!("{}: {}",t("原决定分组未核验", "Original decision groups unverified"), error_code)}</p>}.into_any(),
            })}
            {move ||state.with(|s|s.files.node_retirement.attempt.as_ref().and_then(|a|a.cleanup.clone())).map(|cleanup|match cleanup {
                Cleanup::Observed{replicas,..}=>view!{<details class="zxs-stack"><summary class="zxs-row">{t("原副本义务与当前路由观察", "Original copy obligations and current routes")}</summary>{replicas.iter().map(|replica|view!{<p class="zxs-row">{replica_label(replica)}</p>}).collect_view()}</details>}.into_any(),
                Cleanup::Unverified{error_code}=>view!{<p class="zxs-row">{format!("{}: {}",t("正文清理未核验", "Body cleanup unverified"),error_code)}</p>}.into_any(),
            })}
            <Show when=move ||state.with(|s|s.files.node_retirement.attempt.is_some())>
                <p class="zxs-row">{t("原请求已保存在本浏览器。刷新或重新登录后仍只使用这个原操作；清除网站数据、换浏览器或换设备不会转移恢复记录，请另外保留原操作 ID。", "The original request is retained in this browser. Reloading or signing in again keeps the same operation. Clearing site data, changing browsers or changing devices does not transfer recovery records; keep the operation ID separately.")}</p>
                <Show when=move ||state.with(|s|s.files.node_retirement.attempt.as_ref().is_some_and(|a|a.restored && !a.resume_only && status(s,a)==SaveStatus::Unknown))>
                    <details class="zxs-stack"><summary class="zxs-row">{t("核对本机保存的准确版本", "Review the exact locally saved versions")}</summary>
                        <p class="zxs-row">{move ||state.with(|s|s.files.node_retirement.attempt.as_ref().map(|a|a.selection.history_root().to_string()).unwrap_or_default())}</p>
                        {move ||state.with(|s|s.files.node_retirement.attempt.as_ref().map(|a|a.selection.version_ids().to_vec()).unwrap_or_default()).into_iter().map(|id|view!{<p class="zxs-row">{id.to_string()}</p>}).collect_view()}
                    </details>
                    <label class="zxs-row zxs-flow"><input type="checkbox" disabled=move ||state.with(|s|s.files.node_retirement.attempt.as_ref().is_none_or(|a|a.in_flight||!a.owned_by(s)))
                        prop:checked=move ||state.with(|s|s.files.node_retirement.attempt.as_ref().is_some_and(|a|a.retry_confirmed))
                        on:change=move |event|{let checked=event_target_checked(&event);operation.with_value(|id|state.update(|s|{
                            let principal=s.principal.clone();let active=s.session_active()&&s.session_epoch==epoch;
                            if let Some(a)=&mut s.files.node_retirement.attempt
                                && active
                                && a.target.epoch == epoch
                                && Some(&a.principal) == principal.as_ref()
                                && a.operation == *id
                                && !a.in_flight
                            {
                                a.retry_confirmed = checked;
                            }
                        }));}/><span>{t("本机记录不是服务器回执。我已核对上述文件、原根及全部版本，确认只重发同一原操作的不可撤销退役请求。", "A local record is not a server receipt. I reviewed the file, original root and all versions, and confirm resubmitting only this original irreversible retirement request.")}</span>
                    </label>
                </Show>
                <div class="zxs-flow">
                <button class="zxs-button" disabled=move ||state.with(|s|s.files.node_retirement.attempt.as_ref().is_none_or(|a|!a.retry_allowed(s))) on:click=move |_|operation.with_value(|id|action::retry(state,id,epoch))>{t("重试原操作 / 继续正文清理", "Retry original operation / continue body cleanup")}</button>
                <button class="zxs-button" disabled=move ||state.with(|s|s.files.node_retirement.attempt.as_ref().is_none_or(|a|!a.dismiss_allowed(s))) on:click=move |_|operation.with_value(|id|action::dismiss(state,id,epoch))>{t("关闭已确认结果", "Close confirmed result")}</button>
            </div></Show>

                    {move ||state.with(|s|s.files.node_retirement.error.clone()).map(|error|view!{<p class="zxs-row" role="alert">{error.to_string()}</p>})}
                </div>
            }
        }/>
    }
}
