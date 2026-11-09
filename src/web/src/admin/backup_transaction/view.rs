use super::*;
use storage_contract::storage_directories::CandidateSelection;

fn confirmation(
    state: RwSignal<State>,
    approval: Approval,
    zh: &'static str,
    en: &'static str,
) -> impl IntoView {
    view! {<label class="zxs-row"><input type="checkbox"
    disabled=move ||state.with(|s|!available(s)||!s.admin_console.backup.can_approve(approval))
    prop:checked=move ||state.with(|s|s.admin_console.backup.approval==Some(approval))
    on:change=move |event|{let checked=event_target_checked(&event);state.update(|s|{
        if available(s)&&s.admin_console.backup.can_approve(approval){s.admin_console.backup.approval=checked.then_some(approval);}
    });}/>{t(zh,en)}</label>}
}

fn candidate(state: RwSignal<State>, selection: CandidateSelection) -> impl IntoView {
    let path = selection.root_path.clone();
    let disabled_path = path.clone();
    view! {<div class="zxs-flow"><span>{path}</span>
        <button type="button" class="zxs-button"
            disabled=move ||state.with(|s|!editable(s)||s.admin_console.backup.targets.len()>=8||s.admin_console.backup.targets.iter().any(|target|target.selection.root_path==disabled_path))
            on:click=move |_|actions::add_target(state,selection.clone())>{t("选择完整副本目标","Select complete-copy target")}</button>
    </div>}
}

#[derive(Clone, Copy)]
enum Field {
    Slot,
    Upstream,
}
fn target_field(state: RwSignal<State>, id: String, field: Field) -> impl IntoView {
    let value_id = id.clone();
    view! {<label class="zxs-form-field"><span>{match field{Field::Slot=>t("新目标槽位","New target slot"),Field::Upstream=>t("该目标的直接上游槽位","This target's direct upstream slot")}}</span>
        <input type="text" class="zxs-input" autocomplete="off" disabled=move ||state.with(|s|!editable(s))
            prop:value=move ||state.with(|s|s.admin_console.backup.targets.iter().find(|target|target.disk_id==value_id).map(|target|match field{Field::Slot=>target.slot_id.clone(),Field::Upstream=>target.upstream_slot_id.clone()}).unwrap_or_default())
            on:input=move |event|{let value=event_target_value(&event);state.update(|s|{
                if !editable(s){return;}
                if let Some(target)=s.admin_console.backup.targets.iter_mut().find(|target|target.disk_id==id){match field{Field::Slot=>target.slot_id=value,Field::Upstream=>target.upstream_slot_id=value};}
                s.admin_console.backup.approval=None;
            });}/>
    </label>}
}

fn target(state: RwSignal<State>, id: String) -> impl IntoView {
    let path_id = id.clone();
    let remove_id = id.clone();
    view! {<section class="zxs-frame zxs-stack">
        <p class="zxs-row">{move ||state.with(|s|s.admin_console.backup.targets.iter().find(|target|target.disk_id==path_id).map(|target|target.selection.root_path.clone()))}</p>
        <p class="zxs-row">{id.clone()}</p>
        {target_field(state,id.clone(),Field::Slot)}
        {target_field(state,id,Field::Upstream)}
        <button type="button" class="zxs-button" disabled=move ||state.with(|s|!editable(s)) on:click=move |_|state.update(|s|{
            if editable(s){s.admin_console.backup.targets.retain(|target|target.disk_id!=remove_id);s.admin_console.backup.approval=None;}
        })>{t("移出未提交选择","Remove from unsubmitted selection")}</button>
    </section>}
}

fn result(state: RwSignal<State>) -> impl IntoView {
    view! {<Show when=move ||state.with(|s|s.admin_console.backup.snapshot.is_some())>
        <h3 class="zxs-row">{t("原任务与真实进度","Original task and observed progress")}</h3>
        <pre class="zxs-admin-result">{move ||state.with(|s|s.admin_console.backup.snapshot.as_ref().map(|snapshot|json_codec::to_string_pretty(&snapshot.request).expect("invariant: original backup request serializes")))}</pre>
        <p class="zxs-row" role="status">{move ||state.with(|s|s.admin_console.backup.snapshot.as_ref().map(|snapshot|format!("{} · {} / {} {} · {} / {} {}",snapshot.progress.stage,snapshot.progress.completed_files,snapshot.progress.total_files,t("文件","files"),snapshot.progress.completed_bytes,snapshot.progress.total_bytes,t("字节","bytes"))))}</p>
        <p class="zxs-row">{move ||state.with(|s|s.admin_console.backup.snapshot.as_ref().map(|snapshot|snapshot.progress.detail.clone()))}</p>
        <p class="zxs-row" role="status">{move ||state.with(|s|if s.admin_console.backup.cancelled(){t("核心已确认取消并归档原任务，目标已写字节全部保留。这不是备份完成或清盘。","The core confirmed cancellation and archived the original task. All target bytes are retained. This is neither backup completion nor disk cleanup.")}else if s.admin_console.backup.cancelling(){t("取消意图已保留，但尚未确认取消完成；只查询或明确重试同一取消。","Cancellation intent is retained, but cancellation is not confirmed. Only query or explicitly retry the same cancellation.")}else if s.admin_console.backup.completed(){t("原备份事务已完成活动准入，以下是同一操作的真实发布回执。","The original backup is serving. Its exact published operation receipt follows.")}else{t("尚未完成切换。复制、数据库、Publication 或后台线程完成都不代表备份成功。","Cutover is not complete. Copying, databases, Publication or worker completion alone is not backup success.")})}</p>
        <pre class="zxs-admin-result">{move ||state.with(|s|s.admin_console.backup.snapshot.as_ref().and_then(|snapshot|snapshot.completion.as_ref()).map(|receipt|json_codec::to_string_pretty(receipt).expect("invariant: receipt serializes")))}</pre>
    </Show>}
}

fn decisions(state: RwSignal<State>) -> impl IntoView {
    view! {<Show when=move ||state.with(|s|s.admin_console.backup.snapshot.is_some()&&s.admin_console.backup.can_continue())>
        <p class="zxs-row">{t("每次只推进同一任务。准备在停写、共同决定、激活边界停止，下面的按钮必须分别确认；核心会拒绝未满足的前提。","Each request advances the same task. Preparation stops at write-fencing, common-decision and activation boundaries. Confirm each separately; the core rejects unmet prerequisites.")}</p>
        <button type="button" class="zxs-button" disabled=move ||state.with(|s|!available(s)||s.admin_console.backup.unknown)
            on:click=move |_|actions::advance(state,wire::Command::ContinuePreparation {},None)>{t("继续原任务的材料准备","Continue original preparation")}</button>
        {confirmation(state,Approval::Fence,"我确认暂停原池全部写入，之后只恢复这个原任务。","I confirm stopping all original pool writes and retaining this exact task for recovery.")}
        <button type="button" class="zxs-button" disabled=move ||state.with(|s|!available(s)||s.admin_console.backup.unknown||s.admin_console.backup.approval!=Some(Approval::Fence))
            on:click=move |_|actions::advance(state,wire::Command::FenceSource{confirmation:wire::StopConfirmation::StopAllOriginalPoolWrites},Some(Approval::Fence))>{t("明确停止原池写入","Explicitly fence original pool writes")}</button>
        {confirmation(state,Approval::Decide,"我确认用已显示的原目标与拓扑作不可逆共同决定，不能改盘或另起操作绕过恢复。","I confirm the irreversible common decision for these original targets and topology, with no replacement targets or new-operation recovery bypass.")}
        <button type="button" class="zxs-button" disabled=move ||state.with(|s|!available(s)||s.admin_console.backup.unknown||s.admin_console.backup.approval!=Some(Approval::Decide))
            on:click=move |_|actions::advance(state,wire::Command::DecideRegistry{confirmation:wire::DecisionConfirmation::SelectOriginalIrreversibleRegistryDecision},Some(Approval::Decide))>{t("提交原登记与放置决定","Select original registry and placement")}</button>
        {confirmation(state,Approval::Activate,"我确认只激活这个原决定；缺少任一完整材料或活动门禁时保持拒绝。","I confirm activation of only this original decision; missing complete material or serving admission must remain an error.")}
        <button type="button" class="zxs-button" disabled=move ||state.with(|s|!available(s)||s.admin_console.backup.unknown||s.admin_console.backup.approval!=Some(Approval::Activate))
            on:click=move |_|actions::advance(state,wire::Command::Activate{confirmation:wire::ActivationConfirmation::ActivateOnlyTheOriginalPublishedBackup},Some(Approval::Activate))>{t("完成原决定的活动切换","Complete original serving activation")}</button>
    </Show>}
}

fn cancellation(state: RwSignal<State>) -> impl IntoView {
    view! {
        <Show when=move ||state.with(|s|!s.admin_console.backup.operation_text.is_empty()&&!s.admin_console.backup.completed()&&!s.admin_console.backup.cancelled())>
            <section class="zxs-frame zxs-stack" data-zxs-backup-cancel="">
                <h3 class="zxs-row">{t("取消未决定的原准备任务","Cancel the undecided original preparation")}</h3>
                <p class="zxs-row">{t("是否仍可取消由核心按原任务判断，不以页面上的进度文字授权。取消必须在原池停写和不可逆决定之前；请求会停止复制，保留全部目标已写字节与 SYSTEM 原任务归档，不删除目标内容。","The core decides whether this exact task can still be cancelled; displayed stage text grants no permission. Cancellation must precede source write-fencing and the irreversible decision. It stops copying while retaining all written target bytes and the SYSTEM task archive; it never deletes target contents.")}</p>
                {confirmation(state,Approval::Cancel,"我确认仅取消上方同一原操作，停止复制并保留全部目标数据。确认取消后，新任务必须重新选择空目标目录和使用新操作 ID。","I confirm cancelling only the original operation above, stopping copying and preserving all target data. After confirmed cancellation, a new task must select fresh empty directories and use a new operation ID.")}
                <button type="button" class="zxs-button"
                    disabled=move ||state.with(|s|!available(s)||s.admin_console.backup.approval!=Some(Approval::Cancel))
                    on:click=move |_|actions::cancel(state)>{move ||state.with(|s|if s.admin_console.backup.cancelling(){t("明确重试同一取消","Explicitly retry the same cancellation")}else{t("取消原准备并保留全部目标字节","Cancel original preparation and preserve all target bytes")})}</button>
            </section>
        </Show>
        <Show when=move ||state.with(|s|s.admin_console.backup.cancelled())>
            <section class="zxs-frame zxs-stack" data-zxs-backup-restart="">
                <p class="zxs-row">{t("服务已确认并归档此取消。重新开始只清理本账号的同一原操作恢复记录，清空本机目标选择和旧来源序号，再读取当前任务及新的来源上下文；不会复用目标已写目录或旧操作 ID，不会自动创建新任务。","The service confirmed and archived this cancellation. Restart clears only this account's matching original recovery record, resets local target choices and the old source sequence, then rereads the current task and fresh source context. It never reuses written target directories or the old operation ID, and never creates a new task automatically.")}</p>
                <button type="button" class="zxs-button"
                    disabled=move ||state.with(|s|!s.session_active()||!s.admin_console.backup.can_restart())
                    on:click=move |_|actions::restart(state)>{t("重新开始：清空表单并重新读取安全目标","Start over: reset the form and reread safe targets")}</button>
            </section>
        </Show>
    }
}

pub(in crate::admin) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {<section class="zxs-frame zxs-stack" data-zxs-backup-transaction="">
        <h2 class="zxs-row">{t("完整备份事务","Complete backup transaction")}</h2>
        <p class="zxs-row">{t("独立的管理员操作，不是逐级备份规则或 backup.configure。当前核心仅支持固定 genesis、单一级来源的全池完整副本和 1–8 个新空目标；不等于任意现有备份拓扑替换或外来盘接纳。准备期间原发布改变会拒绝，最终切换需要明确全池停写。","This explicit administrator transaction is separate from recursive backup rules and backup.configure. The core currently supports a fixed-genesis single-primary whole-pool copy to 1–8 fresh empty targets, not arbitrary existing-topology replacement or foreign admission. Source changes invalidate preparation; cutover requires explicit whole-pool write fencing.")}</p>
        <p class="zxs-row" role="alert">{move ||state.with(|s|s.admin_console.backup.error.as_ref().map(|error|crate::ui::feedback(error).display()))}</p>
        <Show when=move ||state.with(|s|s.admin_console.backup.unknown)><p class="zxs-row" role="alert">{move ||state.with(|s|if s.admin_console.backup.cancelling(){t("取消结果未知。保留原操作和输入；只查询或明确重试同一取消，不恢复准备，不创建新任务。","The cancellation outcome is unknown. Keep the original operation and input. Only query or explicitly retry that cancellation; do not resume preparation or create a new task.")}else{t("刚才请求的结果未知。保留原操作 ID 和原输入；先查询或明确恢复同一任务，不创建新任务。","The last request outcome is unknown. Keep the original operation and inputs. Query or explicitly resume that task, not a new one.")})}</p></Show>
        <label class="zxs-form-field"><span>{t("原操作 ID（跨刷新或重启恢复）","Original operation ID (reload/restart recovery)")}</span>
            <input type="text" class="zxs-input" autocomplete="off" prop:value=move ||state.with(|s|s.admin_console.backup.operation_text.clone())
                disabled=move ||state.with(|s|!available(s)||s.admin_console.backup.operation().is_some())
                on:input=move |event|{let value=event_target_value(&event);state.update(|s|{if available(s)&&s.admin_console.backup.operation().is_none(){s.admin_console.backup.operation_text=value;s.admin_console.backup.approval=None;}});}/></label>
        <div class="zxs-flow">
            <button type="button" class="zxs-button" disabled=move ||state.with(|s|!available(s)) on:click=move |_|actions::load(state)>{t("读取保留任务／刷新安全目标","Read retained task / refresh safe targets")}</button>
            <button type="button" class="zxs-button" disabled=move ||state.with(|s|!available(s)||s.admin_console.backup.operation_text.is_empty()) on:click=move |_|actions::read_original(state)>{t("只查询原操作","Query original operation only")}</button>
        </div>
        <p class="zxs-row">{move ||state.with(|s|s.admin_console.backup.notice.clone())}</p>
        <Show when=move ||state.with(|s|s.admin_console.backup.retained_request.is_some()&&s.admin_console.backup.snapshot.is_none())>
            <pre class="zxs-admin-result">{move ||state.with(|s|s.admin_console.backup.retained_request.as_ref().map(|request|json_codec::to_string_pretty(request).expect("invariant: retained original input serializes")))}</pre>
        </Show>
        <Show when=move ||state.with(|s|s.admin_console.backup.context.is_some()&&s.admin_console.backup.operation().is_none())>
            <p class="zxs-row">{move ||state.with(|s|s.admin_console.backup.context.as_ref().map(|context|format!("{}: {} · {}: {}",t("已读序号","Observed sequence"),context.sequence,t("当前登记槽位","Current slots"),context.source_slots.join(", "))))}</p>
            <label class="zxs-form-field"><span>{t("原链来源槽位（旧终端的上游）","Source slot in the original chain (upstream of the old terminal)")}</span><input type="text" class="zxs-input" autocomplete="off"
                disabled=move ||state.with(|s|!editable(s)) prop:value=move ||state.with(|s|s.admin_console.backup.source.clone())
                on:input=move |event|{let value=event_target_value(&event);state.update(|s|{if editable(s){s.admin_console.backup.source=value;s.admin_console.backup.approval=None;}});}/></label>
            <p class="zxs-row">{t("只显示服务端受限发现根下已检查为空的目录。每个所选目标保存完整单位；上游填来源槽位或这批其它目标槽位。","Only inspected empty directories under the service's restricted discovery root are listed. Every selected target retains complete units. Its upstream must be the source or another selected target slot.")}</p>
            <For each=move ||state.with(|s|actions::candidates(&s.admin_console.backup)) key=|selection|selection.selection_id.clone() children=move |selection|candidate(state,selection)/>
            <For each=move ||state.with(|s|s.admin_console.backup.targets.iter().map(|target|target.disk_id.clone()).collect::<Vec<_>>()) key=|id|id.clone() children=move |id|target(state,id)/>
            {confirmation(state,Approval::Prepare,"我已核对全部目标及上游，确认创建独立完整副本材料；这不表示切换成功。","I reviewed every target and upstream and confirm preparing independent complete copies; this does not mean cutover succeeded.")}
            <button type="button" class="zxs-button" disabled=move ||state.with(|s|!s.session_active()||!s.admin_console.backup.can_start()) on:click=move |_|actions::start(state)>{t("创建并开始原备份任务","Create and start original backup task")}</button>
        </Show>
        <Show when=move ||state.with(|s|s.admin_console.backup.original_start.is_some()&&s.admin_console.backup.snapshot.is_none()&&s.admin_console.backup.can_continue())>
            <pre class="zxs-admin-result">{move ||state.with(|s|s.admin_console.backup.original_start.as_ref().map(|request|json_codec::to_string_pretty(request).expect("invariant: original start serializes")))}</pre>
            {confirmation(state,Approval::Prepare,"我已重新核对上方保留的原创建请求，确认只重试相同操作和输入，不创建新操作。","I reviewed the retained original creation request above and confirm retrying only that same operation and input, not a new operation.")}
            <button type="button" class="zxs-button" disabled=move ||state.with(|s|!available(s)||s.admin_console.backup.approval!=Some(Approval::Prepare)) on:click=move |_|actions::retry_start(state)>{t("仅重试完全相同的原创建请求","Retry only the identical original creation request")}</button>
        </Show>
        {result(state)}
        <Show when=move ||state.with(|s|!s.admin_console.backup.operation_text.is_empty()&&s.admin_console.backup.can_continue())>
            {confirmation(state,Approval::Resume,"我确认仅恢复这个原操作；若此前已明确激活并保存切换计划，允许完成这个原切换。不换目标、不重采样来源，也不把未知结果当取消。","I confirm recovery of only this original operation, including completing its previously explicitly activated and durably retained cutover. No replacement targets, source resampling or treating an unknown result as cancellation.")}
            <button type="button" class="zxs-button" disabled=move ||state.with(|s|!available(s)||s.admin_console.backup.approval!=Some(Approval::Resume)) on:click=move |_|actions::resume(state)>{t("明确恢复原任务","Explicitly resume original task")}</button>
        </Show>
        {decisions(state)}
        {cancellation(state)}
    </section>}
}
