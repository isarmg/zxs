//! Browser records retain original recovery inputs.
//! Each operation owns an immutable key for independent tabs.
use super::*;
use crate::files::recovery_storage::Store;
use data_serialization::{Deserialize, Serialize};
use storage_contract::{ErrorCode, OperationId, Principal};
mod receipt_lookup;
mod request_text;

const PREFIX: &str = "zxs:node-retirement:";
const MAX_RECORDS: usize = 32;
const MAX_LABEL_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "file-node-retirement-recovery")]
    Request,
    #[serialization(rename = "file-node-retirement-resume-recovery")]
    ResumeOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Record {
    format: Format,
    principal: Principal,
    operation: OperationId,
    file_id: HistoryFileId,
    history_root: HistoryRoot,
    version_ids: Vec<HistoryVersionId>,
    label: String,
}

#[derive(Clone, Default)]
pub(super) struct Queue {
    owner: Option<(u64, Principal)>,
    records: Vec<Record>,
    pub(super) error: Option<api::Error>,
}
impl Queue {
    pub(super) fn ready_for_new(&self, state: &State) -> bool {
        self.owner.as_ref().is_some_and(|(epoch, principal)| {
            *epoch == state.session_epoch && Some(principal) == state.principal.as_ref()
        }) && self.error.is_none()
            && self.records.is_empty()
    }
}

fn failure(message: impl Into<String>) -> api::Error {
    api::error(ErrorCode::InvalidRequest, message)
}
fn wire_error(source: json_codec::Error) -> api::Error {
    let mut error = failure(t(
        "本机节点退役恢复记录无法解析；记录已保留，未发送永久操作。",
        "The local retirement recovery record cannot be decoded. It is preserved; no permanent request was sent.",
    ));
    error.source = Some(std::sync::Arc::new(api::ErrorSource::RetirementRecovery(
        source,
    )));
    error
}
fn prefix(principal: &Principal) -> String {
    format!("{PREFIX}{}:", api::segment(principal.as_str()))
}
impl Record {
    fn key(&self) -> String {
        format!("{}{}", prefix(&self.principal), self.operation)
    }
    fn selection(&self) -> Result<Selection, api::Error> {
        if self.principal.as_str().len() > 1024
            || self.label.len() > MAX_LABEL_BYTES
            || self.version_ids.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(failure(
                "Invalid retirement recovery identity or ordering; the original record is preserved.",
            ));
        }
        Selection::new(
            self.file_id.clone(),
            self.history_root,
            self.version_ids.clone(),
        )
        .map_err(|error| DecodeError::Name(error).into())
    }
    fn from_attempt(attempt: &Attempt) -> Self {
        Self {
            format: if attempt.resume_only {
                Format::ResumeOnly
            } else {
                Format::Request
            },
            principal: attempt.principal.clone(),
            operation: attempt
                .operation
                .parse()
                .expect("invariant: UI generated a canonical operation identity"),
            file_id: attempt.selection.file_id().clone(),
            history_root: attempt.selection.history_root(),
            version_ids: attempt.selection.version_ids().to_vec(),
            label: attempt.target.path.clone(),
        }
    }
    fn decode(key: &str, principal: &Principal, bytes: &str) -> Result<Self, api::Error> {
        if bytes.len() > wire::MAX_REQUEST_BYTES {
            return Err(api::error(
                ErrorCode::ResourceLimit,
                "Retirement recovery record exceeds its byte budget; it was not removed.",
            ));
        }
        let record: Self = json_codec::from_str(bytes).map_err(wire_error)?;
        record.selection()?;
        if &record.principal != principal || record.key() != key {
            return Err(failure(
                "Retirement recovery record does not belong to this exact account/key; it was not removed.",
            ));
        }
        Ok(record)
    }
    fn encode(&self) -> Result<String, api::Error> {
        self.selection()?;
        let bytes = json_codec::to_string(self).map_err(wire_error)?;
        if bytes.len() > wire::MAX_REQUEST_BYTES {
            return Err(api::error(
                ErrorCode::ResourceLimit,
                "Retirement recovery request exceeds its byte budget.",
            ));
        }
        Ok(bytes)
    }
    fn same_request(&self, other: &Self) -> bool {
        self.principal == other.principal
            && self.operation == other.operation
            && self.file_id == other.file_id
            && self.history_root == other.history_root
            && self.version_ids == other.version_ids
    }
    fn restored_attempt(&self, epoch: u64) -> Result<Attempt, api::Error> {
        Ok(Attempt {
            target: Target {
                epoch,
                file: self.file_id.to_string(),
                entry: String::new(),
                path: self.label.clone(),
                slot: String::new(),
                root: self.history_root.to_string(),
            },
            principal: self.principal.clone(),
            selection: self.selection()?,
            operation: self.operation.to_string(),
            in_flight: false,
            uncertain: true,
            restored: true,
            resume_only: self.format == Format::ResumeOnly,
            retry_confirmed: false,
            cleanup: None,
            separation: None,
        })
    }
}

fn load(principal: &Principal) -> Result<Vec<Record>, api::Error> {
    let mut records = BTreeMap::new();
    Store::open()?.scan(&prefix(principal), |key, bytes| {
        let record = Record::decode(key, principal, bytes)?;
        records.insert(record.operation, record);
        if records.len() > MAX_RECORDS {
            return Err(api::error(
                ErrorCode::ResourceLimit,
                "Retirement recovery queue exceeds 32 operations; no records were deleted.",
            ));
        }
        Ok(())
    })?;
    Ok(records.into_values().collect())
}

/// Persist and read back before the first network mutation. A failed/uncertain
/// storage write must not be retried under a newly generated operation ID.
pub(super) fn save(attempt: &Attempt) -> Result<Record, api::Error> {
    if !load(&attempt.principal)?.is_empty() {
        return Err(failure(t(
            "请先处理本机保留的节点退役操作。",
            "Resolve the retained retirement operations first.",
        )));
    }
    let record = Record::from_attempt(attempt);
    persist(&record)?;
    Ok(record)
}
fn persist(record: &Record) -> Result<(), api::Error> {
    let bytes = record.encode()?;
    let key = record.key();
    Store::open()?.save(&key, &bytes, |saved| {
        Ok(Record::decode(&key, &record.principal, saved)? == *record)
    })
}
fn retain_original(record: &Record) -> Result<Record, api::Error> {
    let records = load(&record.principal)?;
    if let Some(existing) = records
        .iter()
        .find(|saved| saved.operation == record.operation)
    {
        if !existing.same_request(record) {
            return Err(failure(
                "The retained operation has another exact request; neither record was replaced.",
            ));
        }
        // Preserve the original label and original submission/recovery mode.
        return Ok(existing.clone());
    }
    if records.len() == MAX_RECORDS {
        return Err(api::error(
            ErrorCode::ResourceLimit,
            "Retirement recovery queue already contains 32 operations; no records were removed.",
        ));
    }
    persist(record)?;
    Ok(record.clone())
}
pub(super) fn saved(model: &mut Model, record: Record) {
    model.recovery.records.push(record);
}

fn owner(state: &State) -> Option<(u64, Principal)> {
    state
        .principal
        .as_ref()
        .filter(|_| state.session_active())
        .map(|principal| (state.session_epoch, principal.clone()))
}
pub(crate) fn restore(state: RwSignal<State>) {
    let Some(owner) = state.with_untracked(owner) else {
        return;
    };
    let result = load(&owner.1);
    let mut operations = Vec::new();
    state.update(|s| {
        if self::owner(s).as_ref() != Some(&owner) {
            return;
        }
        let model = &mut s.files.node_retirement;
        if model.recovery.owner.as_ref() != Some(&owner) {
            model.attempt = None;
            model.error = None;
            model.recovery.records.clear();
        }
        model.recovery.owner = Some(owner);
        match result {
            Ok(records) => {
                operations = records
                    .iter()
                    .map(|record| record.operation.to_string())
                    .collect();
                model.recovery.records = records;
                model.recovery.error = None;
            }
            Err(error) => model.recovery.error = Some(error),
        }
    });
    for operation in operations {
        actions::record_operation(state, FileOperation::unknown(&operation));
    }
}
pub(super) fn receipt_matches(state: &State, id: &str, receipt: &Value) -> bool {
    state
        .files
        .node_retirement
        .recovery
        .records
        .iter()
        .find(|record| record.operation.to_string() == id)
        .is_none_or(|record| {
            Some(&record.principal) == state.principal.as_ref()
                && matches_request(
                    &record.file_id,
                    record.history_root,
                    &record.version_ids,
                    receipt,
                )
        })
}
fn open(state: RwSignal<State>, operation: OperationId, epoch: u64) {
    state.update(|s| {
        if !s.session_active() || s.session_epoch != epoch || s.files.mutation_pending {
            return;
        }
        let model = &mut s.files.node_retirement;
        if model
            .attempt
            .as_ref()
            .is_some_and(|attempt| attempt.in_flight)
        {
            return;
        }
        let Some(record) = model
            .recovery
            .records
            .iter()
            .find(|record| record.operation == operation)
        else {
            return;
        };
        if Some(&record.principal) != s.principal.as_ref() {
            return;
        }
        match record.restored_attempt(epoch) {
            Ok(attempt) => {
                model.attempt = Some(attempt);
                model.error = None;
            }
            Err(error) => model.recovery.error = Some(error),
        }
    });
}

/// Remove only this exact saved hint after the live UI has a definitive result.
/// Cleanup observations are never written to or restored from browser storage.
pub(super) fn remove(attempt: &Attempt) -> Result<(), api::Error> {
    let expected = Record::from_attempt(attempt);
    let key = expected.key();
    Store::open()?.remove(&key, |bytes| {
        Ok(Record::decode(&key, &attempt.principal, bytes)? == expected)
    })
}
pub(super) fn removed(model: &mut Model, operation: &str) {
    model
        .recovery
        .records
        .retain(|record| record.operation.to_string() != operation);
}

pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    Effect::new(move |_| {
        let current = state.with(owner);
        if current.is_some()
            && state.with_untracked(|s| {
                s.files.node_retirement.recovery.owner.as_ref() != current.as_ref()
            })
        {
            restore(state);
        }
    });
    view! {
        {receipt_lookup::view(state)}
        {request_text::import_view(state)}
        <Show when=move ||state.with(|s|s.session_active() && (s.files.node_retirement.attempt.is_some() || !s.files.node_retirement.recovery.records.is_empty() || s.files.node_retirement.recovery.error.is_some()))>
            <aside class="zxs-entry-section zxs-stack" data-zxs-node-retirement-recovery="true">
                <h2 class="zxs-row">{t("节点永久退役：原操作恢复", "Permanent node retirement: original-operation recovery")}</h2>
                <p class="zxs-row">{t("本浏览器按站点和账号保存原请求，不保存密码、登录令牌或清理结论。重新登录后只查询原收据，不会自动重发永久删除；不需要先找到已删除的文件。", "This browser retains original requests per site and account, not passwords, login tokens or cleanup conclusions. Signing in only queries receipts; it never automatically resubmits permanent deletion. The file does not need to remain in the listing.")}</p>
                <button class="zxs-button" disabled=move ||state.with(|s|s.files.mutation_pending) on:click=move |_|restore(state)>{t("重新读取本机恢复记录", "Reload local recovery records")}</button>
                <For each=move ||state.with(|s|s.files.node_retirement.recovery.records.iter().cloned().map(|record|(s.session_epoch,record)).collect::<Vec<_>>()) key=|row|(row.0,row.1.operation) children=move |(epoch,record)| {
                    let operation = record.operation;
                    let label = format!("{} · {} · {}", record.label, record.file_id, operation);
                    view! { <div class="zxs-stack">
                        <button class="zxs-button" disabled=move ||state.with(|s|s.files.mutation_pending||s.files.node_retirement.attempt.as_ref().is_some_and(|attempt|attempt.operation==operation.to_string())) on:click=move |_|open(state,operation,epoch)>{label}</button>
                        {request_text::export_view(state, epoch, record)}
                    </div> }
                }/>
                {super::view::attempt_view(state)}
                {move ||state.with(|s|s.files.node_retirement.recovery.error.clone()).map(|error|view!{<p class="zxs-row" role="alert">{error.to_string()}</p>})}
            </aside>
        </Show>
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/node_retirement/recovery.rs"]
mod tests;
