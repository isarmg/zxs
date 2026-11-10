//! Explicit text backup of an original intent.
use super::*;

const MAX_ORIGIN_BYTES: usize = 2048;
const MAX_TRANSFER_BYTES: usize = wire::MAX_REQUEST_BYTES + 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum TransferFormat {
    #[serialization(rename = "file-node-retirement-transfer")]
    OriginalRequest,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Transfer {
    format: TransferFormat,
    origin: String,
    record: Record,
}
fn site_origin() -> Result<String, api::Error> {
    let origin = web_sys::window()
        .ok_or_else(|| failure("Browser origin is unavailable."))?
        .origin();
    if origin.len() > MAX_ORIGIN_BYTES
        || !(origin.starts_with("https://") || origin.starts_with("http://"))
    {
        return Err(failure(
            "Retirement request transfer requires a concrete HTTP(S) site origin.",
        ));
    }
    Ok(origin)
}
impl Transfer {
    fn encode(origin: String, record: Record) -> Result<String, api::Error> {
        record.encode()?;
        let transfer = Self {
            format: TransferFormat::OriginalRequest,
            origin,
            record,
        };
        let bytes = json_codec::to_string(&transfer).map_err(wire_error)?;
        if bytes.len() > MAX_TRANSFER_BYTES {
            return Err(api::error(
                ErrorCode::ResourceLimit,
                "Retirement transfer exceeds its byte budget.",
            ));
        }
        Ok(bytes)
    }
    fn decode(bytes: &str, origin: &str, principal: &str) -> Result<Self, api::Error> {
        if bytes.len() > MAX_TRANSFER_BYTES {
            return Err(api::error(
                ErrorCode::ResourceLimit,
                "Retirement transfer exceeds its byte budget; nothing was imported.",
            ));
        }
        let value = storage_contract::strict_json(bytes.as_bytes()).map_err(DecodeError::Name)?;
        if !value.is_object() || !value.get("record").is_some_and(Value::is_object) {
            return Err(failure(
                "Retirement transfer and record must be named objects.",
            ));
        }
        let transfer: Self = json_codec::from_value(value).map_err(wire_error)?;
        if transfer.origin.len() > MAX_ORIGIN_BYTES
            || transfer.origin != origin
            || transfer.record.principal != principal
        {
            return Err(failure(t(
                "恢复文本不属于当前准确站点和账号，未导入。不可修改来源或身份字段来绕过此限制。",
                "This recovery text does not belong to this exact site and account. It was not imported. Do not edit its origin or identity fields to bypass this restriction.",
            )));
        }
        transfer.record.encode()?;
        Ok(transfer)
    }
}
fn export(
    state: RwSignal<State>,
    epoch: u64,
    record: &Record,
) -> Result<Option<String>, api::Error> {
    if !state.with_untracked(|s| owner(s) == Some((epoch, record.principal.clone()))) {
        return Ok(None);
    }
    let records = load(&record.principal)?;
    if !records.iter().any(|saved| saved == record) {
        return Err(failure(
            "The saved recovery record changed or disappeared; reload it before exporting.",
        ));
    }
    Transfer::encode(site_origin()?, record.clone()).map(Some)
}
pub(super) fn export_view(state: RwSignal<State>, epoch: u64, record: Record) -> impl IntoView {
    let record = StoredValue::new(record);
    let output = RwSignal::new(None::<Result<String, api::Error>>);
    view! {
        <details class="zxs-stack" data-zxs-retirement-export="true">
            <summary class="zxs-row">{t("备份这条准确原请求", "Back up this exact original request")}</summary>
            <p class="zxs-row">{t("仅显示可手动复制的恢复文本，含站点、账号、文件标签、原操作与准确版本等元数据，不含密码、令牌、正文或清理结论。请私下保存；不自动写剪贴板、上传或发送给他人。", "Show recovery text for you to copy manually. It contains the site, account, file label, original operation and exact version metadata, not passwords, tokens, bodies or cleanup conclusions. Keep it privately. Nothing is copied to the clipboard, uploaded or sent automatically.")}</p>
            <button class="zxs-button" disabled=move ||record.with_value(|record|!state.with(|s|owner(s)==Some((epoch,record.principal.clone()))))
                on:click=move |_|record.with_value(|record|match export(state,epoch,record) {
                    Ok(Some(bytes))=>output.set(Some(Ok(bytes))), Ok(None)=>{}, Err(error)=>output.set(Some(Err(error))),
                })>{t("读取原记录并显示备份文本", "Read the original record and show backup text")}</button>
            {move ||output.get().map(|result|match result {
                Ok(bytes)=>view!{<textarea class="zxs-input" readonly rows="6" aria-label=t("准确原请求备份文本", "Exact original-request backup text") prop:value=bytes/>}.into_any(),
                Err(error)=>view!{<p class="zxs-row" role="alert">{error.to_string()}</p>}.into_any(),
            })}
        </details>
    }
}

#[derive(Clone, Default)]
struct Import {
    owner: Option<(u64, String)>,
    input: String,
    generation: u64,
    candidate: Option<Transfer>,
    error: Option<api::Error>,
    saved: Option<OperationId>,
}
impl Import {
    fn edit(&mut self, input: String) {
        self.generation += 1;
        self.input = input;
        self.candidate = None;
        self.error = None;
        self.saved = None;
    }
}
fn preview(state: RwSignal<State>, model: RwSignal<Import>) {
    let Some(current) = state.with_untracked(owner) else {
        return;
    };
    let result = site_origin().and_then(|origin| {
        model.with_untracked(|model| Transfer::decode(&model.input, &origin, &current.1))
    });
    model.update(|model| {
        model.owner = Some(current);
        model.candidate = None;
        model.saved = None;
        model.generation += 1;
        match result {
            Ok(candidate) => {
                model.candidate = Some(candidate);
                model.error = None;
            }
            Err(error) => model.error = Some(error),
        }
    });
}
fn can_import(state: &State, owner: &(u64, String), record: &Record) -> bool {
    self::owner(state).as_ref() == Some(owner) && !state.files.mutation_pending
        && !state.files.node_retirement.attempt.as_ref().is_some_and(|attempt| attempt.in_flight)
        // An ID-only cached final status cannot authenticate newly imported bytes.
        // Use the existing record or exact published-receipt lookup instead.
        && state.files.operations.iter().all(|operation| operation.id() != record.operation.to_string() || operation.status() == SaveStatus::Unknown)
}
fn accept(state: RwSignal<State>, model: RwSignal<Import>, generation: u64) {
    let captured = model.with_untracked(|model| {
        (model.generation == generation)
            .then(|| Some((model.owner.clone()?, model.candidate.clone()?)))
            .flatten()
    });
    let Some((current, candidate)) = captured else {
        return;
    };
    if !state.with_untracked(|s| can_import(s, &current, &candidate.record)) {
        return;
    }
    let result = site_origin().and_then(|origin| {
        if origin != candidate.origin { return Err(failure("Site origin changed; the original request was not imported.")); }
        let saved = retain_original(&candidate.record)?;
        if saved.format != candidate.record.format {
            return Err(failure("The existing record has another recovery mode; it was preserved. Use that original entry rather than changing its mode through import."));
        }
        Ok(saved)
    });
    let record = match result {
        Ok(record) => record,
        Err(error) => {
            model.update(|model| model.error = Some(error));
            return;
        }
    };
    restore(state); // Existing scheduling queries receipts only, never POSTs.
    if let Some(error) = state.with_untracked(|s| s.files.node_retirement.recovery.error.clone()) {
        model.update(|model| model.error = Some(error));
        return;
    }
    open(state, record.operation, current.0);
    model.update(|model| {
        model.edit(String::new());
        model.saved = Some(record.operation);
    });
}
pub(super) fn import_view(state: RwSignal<State>) -> impl IntoView {
    let model = RwSignal::new(Import::default());
    Effect::new(move |_| {
        let current = state.with(owner);
        if model.with_untracked(|model| model.owner != current) {
            model.update(|model| {
                model.edit(String::new());
                model.owner = current;
            });
        }
    });
    view! {
        <Show when=move ||state.with(State::session_active)>
            <details class="zxs-entry-section zxs-stack" data-zxs-retirement-import="true">
                <summary class="zxs-row">{t("恢复另行备份的准确原请求", "Restore a separately backed-up original request")}</summary>
                <p class="zxs-row">{t("仅粘贴此前从原站点导出的恢复文本。它不是签名收据或删除许可；须属于当前准确站点和账号，不能修改身份、版本或原操作 ID。导入只补存本机并查收据，不自动删除。发布未知的原请求仍需另行核对全部选择并明确确认，resume-only 模式不会放宽。", "Paste only recovery text exported earlier from the original site. It is not a signed receipt or deletion permission. It must belong to this exact site and account; do not edit identities, versions or the original operation ID. Import only saves a local hint and queries receipts, never automatically deletes. An unknown original request still requires a separate full-selection review and explicit confirmation; resume-only remains restricted.")}</p>
                <label class="zxs-row" for="zxs-retirement-transfer-input">{t("原恢复文本", "Original recovery text")}</label>
                <textarea class="zxs-input" id="zxs-retirement-transfer-input" rows="6" maxlength=MAX_TRANSFER_BYTES autocomplete="off" spellcheck="false"
                    prop:value=move ||model.with(|model|model.input.clone()) on:input=move |event|model.update(|model|model.edit(event_target_value(&event)))/>
                <div class="zxs-flow">
                    <button class="zxs-button" disabled=move ||model.with(|model|model.input.is_empty()) on:click=move |_|preview(state,model)>{t("解析并核对，不保存", "Parse and review without saving")}</button>
                    <button class="zxs-button" on:click=move |_|model.update(|model|model.edit(String::new()))>{t("清空粘贴文本", "Clear pasted text")}</button>
                </div>
                <For each=move ||model.with(|model|model.owner.clone().zip(model.candidate.clone()).map(|(owner,candidate)|(model.generation,owner,candidate)).into_iter().collect::<Vec<_>>()) key=|row|row.0 children=move |(generation,current,candidate)| {
                    let description = format!("{} · {}\n{} · {}\n{}",candidate.origin,candidate.record.principal,candidate.record.operation,candidate.record.file_id,candidate.record.history_root);
                    let mode = if candidate.record.format == Format::ResumeOnly {t("仅允许原操作 resume", "Original-operation resume only")}else{t("准确原请求；未知重发须重新明确确认", "Exact original request; unknown resubmission requires renewed explicit confirmation")};
                    let versions = candidate.record.version_ids.clone();
                    let captured = StoredValue::new((current,candidate.record));
                    view!{<div class="zxs-stack">
                        <p class="zxs-row">{description}</p><p class="zxs-row">{mode}</p>
                        <details class="zxs-stack"><summary class="zxs-row">{t("核对全部原版本", "Review every original version")}</summary>
                            {versions.into_iter().map(|version|view!{<p class="zxs-row">{version.to_string()}</p>}).collect_view()}
                        </details>
                        <p class="zxs-row">{t("本会话若已对同一 ID 有终态观察，请使用现有原记录或上方已发布收据入口，不能让该观察为新粘贴的内容授信。", "If this session already has a final observation for the same ID, use the existing record or published-receipt lookup above. That observation must not authenticate newly pasted content.")}</p>
                        <button class="zxs-button" disabled=move ||captured.with_value(|(current,record)|!state.with(|s|can_import(s,current,record))) on:click=move |_|accept(state,model,generation)>{t("仅补存准确原请求并打开恢复入口", "Save only this exact original request and open recovery")}</button>
                    </div>}
                }/>
                {move ||model.with(|model|model.saved).map(|operation|view!{<p class="zxs-row" role="status">{format!("{} {operation}",t("已保留原操作，未发送永久请求。", "Original operation retained; no permanent request was sent."))}</p>})}
                {move ||model.with(|model|model.error.clone()).map(|error|view!{<p class="zxs-row" role="alert">{error.to_string()}</p>})}
            </details>
        </Show>
    }
}

#[cfg(test)]
#[path = "../../../../../../test/web/unit/files/node_retirement/recovery/request_text.rs"]
mod tests;
