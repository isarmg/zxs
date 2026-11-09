//! A known original operation may recover a missing browser hint, never a new write.
use super::*;
use std::sync::Arc;
use storage_contract::history_window::HistoryTreeId;

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct PublishedRequest {
    history_tree_id: HistoryTreeId,
    history_root: HistoryRoot,
    origin_file_id: HistoryFileId,
    version_ids: Vec<HistoryVersionId>,
}
#[derive(Clone)]
struct Candidate {
    owner: (u64, String),
    history: HistoryTreeId,
    record: Record,
    receipt: Arc<Value>,
}
impl Candidate {
    fn read(owner: (u64, String), operation: OperationId, receipt: Value) -> Result<Self, api::Error> {
        if receipt["operation_kind"] != "file.nodes.retire"
            || !published(FileOperation::from_receipt(&operation.to_string(), &receipt).status()) {
            return Err(failure(t("没有取得这个原 ID 的已发布节点退役收据。未保存记录、未猜测选择，也未发送删除。", "No published node-retirement receipt was found for this original ID. Nothing was saved, no selection was guessed, and no deletion was sent.")));
        }
        let request = &receipt["result"]["retirement"]["request"];
        if !request.is_object() {
            return Err(failure("The published retirement request is not a named object; nothing was saved."));
        }
        let request: PublishedRequest = json_codec::from_value(request.clone())
            .map_err(|source| api::Error::from(DecodeError::Wire(source)))?;
        let record = Record {
            format: Format::ResumeOnly, principal: owner.1.clone(), operation,
            label: request.origin_file_id.to_string(), file_id: request.origin_file_id,
            history_root: request.history_root, version_ids: request.version_ids,
        };
        // Bound the complete saved hint now, not after the user accepts it.
        record.encode()?;
        Ok(Self { owner, history: request.history_tree_id, record, receipt: Arc::new(receipt) })
    }
}

#[derive(Clone, Default)]
struct Lookup {
    owner: Option<(u64, String)>,
    input: String,
    generation: u64,
    loading: bool,
    cancellation: Option<api::ReadCancellation>,
    candidate: Option<Candidate>,
    saved: Option<OperationId>,
    error: Option<api::Error>,
}
impl Lookup {
    fn cancel(&mut self) {
        if let Some(cancellation) = self.cancellation.take() { cancellation.abort(); }
        self.generation += 1;
        self.loading = false;
    }
    fn edit(&mut self, input: String) {
        self.cancel();
        self.input = input;
        self.candidate = None;
        self.saved = None;
        self.error = None;
    }
    fn current(&self, owner: &(u64, String), generation: u64) -> bool {
        self.owner.as_ref() == Some(owner) && self.generation == generation
    }
}
fn lookup(state: RwSignal<State>, model: RwSignal<Lookup>) {
    let Some(current) = state.with_untracked(owner) else { return; };
    let input = model.with_untracked(|model| (!model.loading).then(|| model.input.trim().to_owned()));
    let Some(input) = input else { return; };
    let operation: OperationId = match input.parse() {
        Ok(operation) => operation,
        Err(source) => { model.update(|model| model.error = Some(DecodeError::Name(source).into())); return; }
    };
    let cancellation = match api::ReadCancellation::new() {
        Ok(cancellation) => cancellation,
        Err(error) => { model.update(|model| model.error = Some(error)); return; }
    };
    let generation = model.try_update(|model| {
        model.cancel(); model.owner = Some(current.clone()); model.loading = true;
        model.candidate = None; model.saved = None; model.error = None;
        model.cancellation = Some(cancellation.clone()); model.generation
    }).expect("invariant: mounted receipt lookup owns its state");
    spawn_local(async move {
        // The existing outcome endpoint enforces exact current operation ownership.
        // It returns publication evidence, not a new cleanup/absence observation.
        let result = api::cancellable_get(&format!("/api/operations/{operation}/outcome"), cancellation).await
            .and_then(|receipt| Candidate::read(current.clone(), operation, receipt));
        if !crate::ui::accept_read_response(state, current.0, &result)
            || state.with_untracked(owner).as_ref() != Some(&current)
            || model.try_with_untracked(|model| model.current(&current, generation)) != Some(true) { return; }
        model.update(|model| {
            model.loading = false; model.cancellation = None;
            match result {
                Ok(candidate) => model.candidate = Some(candidate),
                Err(error) => model.error = Some(error),
            }
        });
    });
}
fn can_accept(state: &State, candidate: &Candidate) -> bool {
    owner(state).as_ref() == Some(&candidate.owner) && !state.files.mutation_pending
        && !state.files.node_retirement.attempt.as_ref().is_some_and(|attempt| attempt.in_flight)
        && state.files.operations.iter().all(|operation| {
            operation.id() != candidate.record.operation.to_string() || operation.status() != SaveStatus::Rejected
        })
}
fn accept(state: RwSignal<State>, model: RwSignal<Lookup>, generation: u64) {
    let candidate = model.with_untracked(|model| {
        (model.generation == generation && !model.loading).then(|| model.candidate.clone()).flatten()
    });
    let Some(candidate) = candidate else { return; };
    if !state.with_untracked(|s| can_accept(s, &candidate)) { return; }
    if let Err(error) = retain_original(&candidate.record) {
        model.update(|model| model.error = Some(error)); return;
    }
    // Re-admit the complete bounded queue, retaining every independent operation.
    restore(state);
    let queue_error = state.with_untracked(|s| s.files.node_retirement.recovery.error.clone());
    if let Some(error) = queue_error {
        model.update(|model| model.error = Some(error)); return;
    }
    let operation = candidate.record.operation;
    let status = actions::record_receipt(state, &operation.to_string(), (*candidate.receipt).clone());
    if !published(status) {
        model.update(|model| model.error = Some(failure("The exact receipt could not settle this recovery record; the original record is preserved.")));
        return;
    }
    open(state, operation, candidate.owner.0);
    model.update(|model| {
        model.cancel(); model.candidate = None; model.saved = Some(operation); model.error = None;
    });
}
pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    let model = RwSignal::new(Lookup::default());
    on_cleanup(move || { let _ = model.try_update(Lookup::cancel); });
    Effect::new(move |_| {
        let current = state.with(owner);
        if model.with_untracked(|model| model.owner != current) {
            model.update(|model| { model.edit(String::new()); model.owner = current; });
        }
    });
    view! {
        <Show when=move ||state.with(State::session_active)>
            <details class="zxs-entry-section zxs-stack" data-zxs-retirement-receipt-lookup="true">
                <summary class="zxs-row">{t("凭原操作 ID 找回已发布的节点退役", "Recover published node retirement by original operation ID")}</summary>
                <p class="zxs-row">{t("换浏览器或本机恢复记录丢失时，可只读查询你另行保留的原 operation_id。只接受当前账号拥有的已发布退役收据；找不到、未发布或读取失败不等于未执行，不会猜测版本或改用新 ID。此入口不会自动删除或继续清理。", "If you changed browsers or lost local recovery records, look up the original operation_id you kept separately. Only this account's published retirement receipt is accepted. Missing, unpublished or failed lookups do not prove non-execution; no versions are guessed and no new ID is used. This lookup never deletes or resumes cleanup automatically.")}</p>
                <label class="zxs-row" for="zxs-retirement-recovery-operation">{t("原节点退役操作 ID", "Original node-retirement operation ID")}</label>
                <input class="zxs-input" id="zxs-retirement-recovery-operation" maxlength="64" autocomplete="off" spellcheck="false"
                    prop:value=move ||model.with(|model|model.input.clone())
                    on:input=move |event|model.update(|model|model.edit(event_target_value(&event)))/>
                <div class="zxs-flow">
                    <button class="zxs-button" disabled=move ||model.with(|model|model.loading||model.input.trim().is_empty()) on:click=move |_|lookup(state,model)>{t("只读查询准确原收据", "Read the exact original receipt")}</button>
                    <button class="zxs-button" disabled=move ||!model.with(|model|model.loading) on:click=move |_|model.update(Lookup::cancel)>{t("取消查询", "Cancel lookup")}</button>
                </div>
                <Show when=move ||model.with(|model|model.loading)><p class="zxs-row" role="status">{t("正在只读查询原操作…", "Reading the original operation…")}</p></Show>
                <For each=move ||model.with(|model|model.candidate.clone().map(|candidate|(model.generation,candidate)).into_iter().collect::<Vec<_>>()) key=|row|row.0 children=move |(generation,candidate)| {
                    let description = format!("{} · {}\n{} · {}", candidate.record.operation, candidate.record.file_id, candidate.history, candidate.record.history_root);
                    let versions = candidate.record.version_ids.clone();
                    let candidate = StoredValue::new(candidate);
                    view! { <div class="zxs-stack">
                        <p class="zxs-row">{description}</p>
                        <details class="zxs-stack"><summary class="zxs-row">{t("核对原准确版本", "Review the original exact versions")}</summary>
                            {versions.into_iter().map(|version|view!{<p class="zxs-row">{version.to_string()}</p>}).collect_view()}
                        </details>
                        <p class="zxs-row">{t("保存后仅打开原操作恢复入口。新记录只允许 resume，重新登录仍须再查到准确发布收据；它不证明正文已清理，也不会补发原删除。", "Saving only opens the original recovery entry. A new record allows resume only, and another sign-in must confirm its exact publication receipt again. It proves no body cleanup and cannot resubmit the original deletion.")}</p>
                        <button class="zxs-button" disabled=move ||candidate.with_value(|candidate|!state.with(|s|can_accept(s,candidate))) on:click=move |_|accept(state,model,generation)>{t("保存准确原请求并打开恢复入口", "Save the exact original request and open recovery")}</button>
                    </div> }
                }/>
                {move ||model.with(|model|model.saved).map(|operation|view!{<p class="zxs-row" role="status">{format!("{} {operation}",t("已保留原操作；是否继续清理请在下方明确选择。", "Original operation retained; explicitly choose below whether to resume cleanup."))}</p>})}
                {move ||model.with(|model|model.error.clone()).map(|error|view!{<p class="zxs-row" role="alert">{error.to_string()}</p>})}
            </details>
        </Show>
    }
}

#[cfg(test)]
#[path = "../../../../../../test/web/unit/files/node_retirement/recovery/receipt_lookup.rs"]
mod tests;
