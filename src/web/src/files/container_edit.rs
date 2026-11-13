//! Container policy is shared mutable metadata, never an implicit history commit.
use crate::{api, ui::{State, accept_read_response}};
use super::{actions, model, review};
use storage_contract::container_edit::{CommitMode, ContainerEdit};
use storage_contract::file_container_tags::{Context, ReadRequest};
use web_interface::{i18n::t, prelude::*, tasks::spawn_local};
mod storage;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    epoch: u64,
    navigation: u64,
    entry: String,
    request: ReadRequest,
}
impl Target {
    fn capture(state: &State) -> Option<Self> {
        if !state.session_active() { return None; }
        let entry = state.files.selected.as_ref()?;
        if entry.kind() != storage_contract::NamespaceEntryKind::File { return None; }
        let (repository, path) = entry.context()?;
        Some(Self {
            epoch: state.session_epoch,
            navigation: state.files.generation,
            entry: entry.entry_id().to_owned(),
            request: ReadRequest {
                file_id: entry.file_id().parse().ok()?,
                context: Context {
                    repository_id: repository.parse().ok()?,
                    path: name_policy::PortablePath::parse(path).ok()?,
                },
            },
        })
    }
    fn route(&self) -> String {
        format!("/api/file-manager/files/{}/edit-mode", self.request.file_id)
    }
    fn read_route(&self) -> String {
        format!("{}?repository={}&path={}", self.route(),
            api::segment(&self.request.context.repository_id.to_string()),
            api::segment(self.request.context.path.as_str()))
    }
}

#[derive(Clone, Default)]
struct Panel {
    target: Option<Target>,
    generation: u64,
    snapshot: Option<ContainerEdit>,
    loading: bool,
    error: String,
    cancellation: Option<api::ReadCancellation>,
}
impl Panel {
    fn cancel(&mut self) {
        if let Some(cancel) = self.cancellation.take() { cancel.abort(); }
        self.generation += 1;
    }
    fn accepts(&self, target: &Target, generation: u64) -> bool {
        self.target.as_ref() == Some(target) && self.generation == generation
    }
}
fn decode(value: json_codec::Value) -> Result<ContainerEdit, api::Error> {
    json_codec::from_value(value).map_err(|source| api::Error::invalid_response(
        "Invalid container edit state.", api::ErrorSource::ContainerEdit(source)))
}
fn can_switch(edit: &ContainerEdit) -> bool { edit.draft().is_none() }
fn status(edit: &ContainerEdit) -> String {
    match edit {
        ContainerEdit::Automatic { .. } => t(
            "自动：文件更改后生成版本（默认）。", "Automatic: file changes create versions (default)."),
        ContainerEdit::ManualClean { .. } => t(
            "手动：文件更改保留为共享稿，仅显式提交生成版本。",
            "Manual: file changes stay in a shared draft; only explicit commit creates a version."),
        ContainerEdit::ManualDirty { .. } => t(
            "手动：有未提交的共享稿。请先显式提交，再切回自动；更改此设置不会提交文件。",
            "Manual: a shared draft is uncommitted. Explicitly commit it before switching to automatic; changing this setting does not commit the file."),
    }
}
fn reload(state: RwSignal<State>, panel: RwSignal<Panel>, target: Option<Target>) {
    let cancellation = target.as_ref().map(|_| api::ReadCancellation::new()).transpose();
    let generation = panel.try_update(|p| {
        p.cancel();
        p.target = target.clone();
        p.snapshot = None;
        p.error.clear();
        p.loading = target.is_some();
        p.generation
    });
    let (Some(target), Some(generation)) = (target, generation) else { return; };
    let cancellation = match cancellation {
        Ok(Some(value)) => value,
        Ok(None) => return,
        Err(error) => {
            let _ = panel.try_update(|p| { p.loading = false; p.error = error.message; });
            return;
        }
    };
    let _ = panel.try_update(|p| p.cancellation = Some(cancellation.clone()));
    spawn_local(async move {
        let result = api::cancellable_get(&target.read_route(), cancellation).await.and_then(decode);
        if !accept_read_response(state, target.epoch, &result)
            || state.try_with_untracked(Target::capture).flatten().as_ref() != Some(&target)
        { return; }
        let _ = panel.try_update(|p| {
            if !p.accepts(&target, generation) { return; }
            p.loading = false;
            p.cancellation = None;
            match result {
                Ok(edit) => p.snapshot = Some(edit),
                Err(error) => p.error = error.message,
            }
        });
    });
}
fn submit(state: RwSignal<State>, panel: RwSignal<Panel>) {
    let command = state.with_untracked(|s| {
        if !actions::write_allowed(s) || model::selected_entry_read_only(&s.files)
            || s.files.container_draft_commit.is_some()
            || s.files.container_draft_recovery_error.is_some()
        {
            return None;
        }
        let target = Target::capture(s)?;
        panel.with_untracked(|p| {
            if p.loading || p.target.as_ref() != Some(&target) { return None; }
            let edit = p.snapshot.as_ref()?;
            if !can_switch(edit) { return None; }
            let mode = match edit.mode() {
                CommitMode::Automatic => CommitMode::Manual,
                CommitMode::Manual => CommitMode::Automatic,
            };
            Some((target, edit.generation(), mode))
        })
    });
    if let Some((target, generation, mode)) = command {
        review::submit(state, review::Kind::ContainerMode, "POST", target.route(),
            json_codec::json!({"repository":target.request.context.repository_id,
                "path":target.request.context.path,"expected_generation":generation,"mode":mode}));
    }
}
pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    let panel = RwSignal::new(Panel::default());
    let target = Memo::new(move |_| state.with(Target::capture));
    let pending = Memo::new(move |_| state.with(|s| s.files.publications
        .get(&review::Kind::ContainerMode).is_some_and(|p| p.operation_id.is_some())
            || s.files.container_draft_commit.is_some()
            || s.files.container_draft_recovery_error.is_some()));
    Effect::new(move |_| {
        let target = target.get();
        if !pending.get() { reload(state, panel, target); }
    });
    on_cleanup(move || { let _ = panel.try_update(Panel::cancel); });
    view! {
        <section class="zxs-entry-section zxs-stack" aria-label=t("版本提交模式", "Version commit mode")>
            <h3>{t("版本提交模式", "Version commit mode")}</h3>
            <p role="status">{move || panel.with(|p| {
                if p.loading { t("正在读取…", "Loading…") }
                else if !p.error.is_empty() { p.error.clone() }
                else { p.snapshot.as_ref().map(status).unwrap_or_default() }
            })}</p>
            <p role="status">{move || state.with(|s| review::error_text(s, review::Kind::ContainerMode))}</p>
            <div class="zxs-file-toolbar zxs-flow">
                <button class="zxs-button" disabled=move || pending.get()
                    || state.with(|s| !actions::write_allowed(s) || model::selected_entry_read_only(&s.files))
                    || panel.with(|p| p.loading || p.snapshot.as_ref().and_then(|e| e.draft()).is_none())
                    on:click=move |_|commit_current(state, panel)>
                    {t("显式提交共享稿为新版本", "Explicitly commit shared draft as a new version")}
                </button>
                <button class="zxs-button" disabled=move || pending.get()
                    on:click=move |_| reload(state, panel, state.with_untracked(Target::capture))>
                    {t("重新读取设置", "Reload setting")}
                </button>
                <button class="zxs-button" disabled=move || pending.get()
                    || state.with(|s| !actions::write_allowed(s) || model::selected_entry_read_only(&s.files))
                    || panel.with(|p| p.loading || p.snapshot.as_ref().is_none_or(|e| !can_switch(e)))
                    on:click=move |_| submit(state, panel)>
                    {move || panel.with(|p| if p.snapshot.as_ref().is_some_and(|e| e.mode() == CommitMode::Manual) {
                        t("启用自动版本提交", "Enable automatic version commits")
                    } else { t("改为仅手动提交版本", "Use manual version commits only") })}
                </button>
            </div>
        </section>
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/container_edit.rs"]
mod tests;

/// Root-owned intent survives navigation. Neither a new selection nor a retry
/// may substitute a different draft, operation or prepared envelope.
#[derive(Clone)]
pub(crate) struct CommitPreparation {
    principal: storage_contract::Principal,
    target: Target,
    generation: storage_contract::Generation,
    draft: storage_contract::OperationId,
    operation: storage_contract::OperationId,
    envelope: Option<json_codec::Value>,
    in_flight: bool,
    commit_started: bool,
    settled: Option<model::SaveStatus>,
    error: String,
}

#[derive(Debug, error_definition::Error)]
pub enum CommitPreparationError {
    #[error("the retained shared-draft request changed or belongs to another identity")]
    RecoveryBinding,
    #[error("shared-draft browser recovery exceeds its bounded record budget")]
    RecoveryLimit,
    #[error("invalid shared-draft recovery bytes: {0}")]
    Stored(#[source] json_codec::Error),
    #[error("prepared commit does not bind the explicitly selected shared draft")]
    Binding,
    #[error("invalid prepared commit: {0}")]
    Wire(#[source] storage_contract::PublicCommitParseError),
    #[error("invalid prepared commit request: {0}")]
    Contract(#[source] storage_contract::Error),
}
impl From<CommitPreparationError> for api::Error {
    fn from(source: CommitPreparationError) -> Self {
        Self::invalid_response("Invalid shared-draft commit preparation.",
            api::ErrorSource::ContainerDraftPreparation(source))
    }
}

fn frozen_envelope(value: json_codec::Value, intent: &CommitPreparation)
    -> Result<json_codec::Value, api::Error>
{
    if value["operation_id"].as_str() != Some(intent.operation.to_string().as_str()) {
        return Err(CommitPreparationError::Binding.into());
    }
    let workspace = value["workspace_id"].as_str()
        .ok_or(CommitPreparationError::Binding)?.to_owned();
    let body = storage_contract::client_protocol::prepare_commit(value, &workspace,
        &intent.operation.to_string(), "Explicit shared draft commit", None,
    ).map_err(CommitPreparationError::Contract)?;
    validate_frozen(&body, intent)?;
    Ok(body)
}
fn validate_frozen(body: &json_codec::Value, intent: &CommitPreparation) -> Result<(), api::Error> {
    let workspace = body["workspace_id"].as_str().ok_or(CommitPreparationError::Binding)?;
    let envelope = storage_contract::PublicCommitEnvelope::parse(workspace, body.clone())
        .map_err(CommitPreparationError::Wire)?;
    if envelope.operation_id() != intent.operation
        || envelope.claimed_digest() != envelope.digest()
        || envelope.container_draft_commit().is_none_or(|binding| {
            binding.file != intent.target.request.file_id || binding.generation != intent.generation
                || binding.revision.operation != intent.draft || binding.operation != intent.operation
        })
    { return Err(CommitPreparationError::Binding.into()); }
    Ok(())
}

fn commit_current(state: RwSignal<State>, panel: RwSignal<Panel>) {
    let intent = state.with_untracked(|s| {
        if !actions::write_allowed(s) || model::selected_entry_read_only(&s.files)
            || s.files.container_draft_commit.is_some()
            || s.files.container_draft_recovery_error.is_some()
        { return None; }
        let target = Target::capture(s)?;
        panel.with_untracked(|p| {
            if p.loading || p.target.as_ref() != Some(&target) { return None; }
            let edit = p.snapshot.as_ref()?;
            Some((target, edit.generation(), edit.draft()?.operation))
        })
    });
    let Some((target, generation, draft)) = intent else { return; };
    let Some(operation) = crate::ui::file_operation_id(state) else { return; };
    let operation = match operation.parse() {
        Ok(operation) => operation,
        Err(source) => {
            crate::ui::fail(state, CommitPreparationError::Contract(source).into());
            return;
        }
    };
    let Some(principal) = state.with_untracked(|s| s.principal.clone()) else { return; };
    let intent = CommitPreparation {
        principal, target, generation, draft, operation, envelope: None, in_flight: false,
        commit_started: false, settled: None, error: String::new(),
    };
    if let Err(error) = storage::save_intent(&intent) {
        state.update(|s| s.files.container_draft_recovery_error = Some(error));
        return;
    }
    state.update(|s| s.files.container_draft_commit = Some(intent));
    retry_commit(state);
}

fn can_retry_commit(state: &State) -> bool {
    let Some(intent) = &state.files.container_draft_commit else { return false; };
    state.session_active() && state.session_epoch == intent.target.epoch
        && state.principal.as_ref() == Some(&intent.principal)
        && state.files.container_draft_recovery_error.is_none()
        && state.management.storage.writable() && !state.files.mutation_pending
        && !intent.in_flight
        && !super::deletion::blocks_writes(state) && !super::mkdir::blocks_writes(state)
        && state.files.operations.iter().all(|operation| {
            operation.status() != model::SaveStatus::Unknown
                || operation.id() == intent.operation.to_string()
        })
}
fn owns_intent(state: &State, intent: &CommitPreparation) -> bool {
    state.session_active() && state.session_epoch == intent.target.epoch
        && state.principal.as_ref() == Some(&intent.principal)
        && state.files.container_draft_commit.as_ref()
            .is_some_and(|current| current.operation == intent.operation)
}
fn owns_active_intent(state: &State, intent: &CommitPreparation) -> bool {
    owns_intent(state, intent)
        && state.files.container_draft_commit.as_ref()
            .is_some_and(|current| current.settled.is_none())
}
fn retry_commit(state: RwSignal<State>) {
    let settled = state.with_untracked(|s| s.files.container_draft_commit.as_ref()
        .filter(|intent| intent.settled.is_some()).map(|intent| intent.operation.to_string()));
    if let Some(operation) = settled {
        finish(state, &operation);
        return;
    }
    let intent = state.try_update(|s| {
        if !can_retry_commit(s) { return None; }
        let intent = s.files.container_draft_commit.as_mut().unwrap();
        intent.in_flight = true;
        intent.error.clear();
        s.files.mutation_pending = true;
        Some(intent.clone())
    }).flatten();
    let Some(mut intent) = intent else { return; };
    if let Err(error) = storage::verify(&intent) {
        finish_commit_error(state, &intent, error.clone(), false);
        state.update(|s| s.files.container_draft_recovery_error = Some(error));
        return;
    }
    spawn_local(async move {
        if intent.envelope.is_none() {
            let endpoint = format!("/api/file-manager/files/{}/prepare-draft-commit",
                intent.target.request.file_id);
            let body = json_codec::json!({
                "repository":intent.target.request.context.repository_id,
                "path":intent.target.request.context.path,
                "expected_generation":intent.generation,"draft_operation":intent.draft,
                "operation_id":intent.operation,
            });
            let result = api::validated_request("POST", &endpoint, Some(body),
                |value| frozen_envelope(value, &intent)).await;
            if !state.try_with_untracked(|s| owns_active_intent(s, &intent)).unwrap_or(false) {
                return;
            }
            match result {
                Ok(body) => {
                    intent.envelope = Some(body.clone());
                    if let Err(error) = storage::save_envelope(&intent) {
                        finish_commit_error(state, &intent, error, false);
                        restore(state);
                        return;
                    }
                    let _ = state.try_update(|s| s.files.container_draft_commit.as_mut()
                        .unwrap().envelope = Some(body));
                }
                Err(error) => {
                    finish_commit_error(state, &intent, error, false);
                    return;
                }
            }
        }
        let body = intent.envelope.as_ref().unwrap().clone();
        let workspace = body["workspace_id"].as_str()
            .expect("invariant: a validated frozen envelope has a workspace");
        let endpoint = format!("/api/workspaces/{}/commit", api::segment(workspace));
        let Some(true) = state.try_update(|s| {
            if !owns_active_intent(s, &intent) { return false; }
            s.files.container_draft_commit.as_mut().unwrap().commit_started = true;
            true
        }) else { return; };
        let result = api::request("POST", &endpoint, Some(body)).await;
        if !state.try_with_untracked(|s| owns_active_intent(s, &intent)).unwrap_or(false) { return; }
        match result {
            Ok(value) => {
                let _ = state.try_update(|s| {
                    s.files.mutation_pending = false;
                    s.files.container_draft_commit.as_mut().unwrap().in_flight = false;
                });
                actions::record_receipt(state, &intent.operation.to_string(), value);
            }
            Err(error) => finish_commit_error(state, &intent, error, true),
        }
    });
}
fn finish_commit_error(
    state: RwSignal<State>, intent: &CommitPreparation, error: api::Error, sent_commit: bool,
) {
    if !state.try_with_untracked(|s| owns_active_intent(s, intent)).unwrap_or(false) {
        return;
    }
    let unknown = sent_commit && (intent.commit_started || error.mutation_outcome_unknown());
    let _ = state.try_update(|s| {
        if !owns_active_intent(s, intent) { return; }
        s.files.mutation_pending = false;
        let current = s.files.container_draft_commit.as_mut().unwrap();
        current.in_flight = false;
        current.error = error.to_string();
        // Only a definite first-commit rejection allows cancellation. Once a
        // commit outcome was uncertain, a later 4xx cannot prove nonpublication.
        current.commit_started = unknown;
    });
    if unknown {
        actions::record_operation(state, model::FileOperation::unknown(&intent.operation.to_string()));
    }
    crate::ui::fail(state, error);
}

pub(crate) fn receipt_matches(state: &State, operation: &str, value: &json_codec::Value) -> bool {
    let Some(intent) = state.files.container_draft_commit.as_ref()
        .filter(|intent| intent.operation.to_string() == operation) else { return true; };
    let Some(envelope) = &intent.envelope else { return false; };
    value["operation_kind"].as_str() == Some("content_save")
        && value["request_digest"].as_str().is_some()
        && value["request_digest"] == envelope["request_digest"]
}

pub(crate) fn settle(model: &mut model::Model, operation: &str, status: model::SaveStatus) {
    if status != model::SaveStatus::Unknown && model.container_draft_commit.as_ref()
        .is_some_and(|intent| intent.operation.to_string() == operation)
    {
        let intent = model.container_draft_commit.as_mut().unwrap();
        intent.settled = Some(status);
        intent.in_flight = false;
        // Cleanup follows a live terminal receipt, never a persisted claim.
        model.mutation_pending = false;
    }
}
pub(crate) fn finish(state: RwSignal<State>, operation: &str) {
    let intent = state.with_untracked(|s| s.files.container_draft_commit.as_ref()
        .filter(|intent| intent.operation.to_string() == operation && intent.settled.is_some()
            && owns_intent(s, intent)).cloned());
    let Some(intent) = intent else { return; };
    match storage::remove(&intent) {
        Ok(()) => state.update(|s| s.files.container_draft_commit = None),
        Err(error) => state.update(|s| {
            s.files.container_draft_commit.as_mut().unwrap().error = error.to_string();
        }),
    }
}
fn cancel_preparation(state: RwSignal<State>) {
    let intent = state.with_untracked(|s| s.files.container_draft_commit.as_ref()
        .filter(|intent| owns_intent(s, intent) && !intent.in_flight && !intent.commit_started
            && intent.envelope.is_none()).cloned());
    let Some(intent) = intent else { return; };
    match storage::remove(&intent) {
        Ok(()) => state.update(|s| s.files.container_draft_commit = None),
        Err(error) => state.update(|s| s.files.container_draft_recovery_error = Some(error)),
    }
}
pub(crate) fn restore(state: RwSignal<State>) {
    let owner = state.with_untracked(|s| {
        if !s.session_active() { return None; }
        Some((s.session_epoch, s.principal.clone()?))
    });
    let Some((epoch, principal)) = owner else { return; };
    if state.with_untracked(|s| s.files.container_draft_commit.as_ref()
        .is_some_and(|intent| owns_intent(s, intent) && intent.in_flight)) { return; }
    let retained = state.with_untracked(|s| s.files.container_draft_commit.as_ref()
        .filter(|intent| owns_intent(s, intent)).cloned());
    let result = retained.as_ref().map(storage::verify).transpose()
        .and_then(|_| storage::load(&principal, epoch));
    let operation = state.try_update(|s| {
        if !s.session_active() || s.session_epoch != epoch
            || s.principal.as_ref() != Some(&principal) { return None; }
        s.files.container_draft_recovery_error = None;
        match result {
            Ok(intent) => {
                let operation = intent.as_ref().filter(|intent| intent.envelope.is_some())
                    .map(|intent| intent.operation.to_string());
                s.files.container_draft_commit = intent;
                if let Some(operation) = &operation {
                    s.files.operations.retain(|known| known.id() != operation);
                }
                operation
            }
            Err(error) => {
                s.files.container_draft_recovery_error = Some(error);
                None
            }
        }
    }).flatten();
    if let Some(operation) = operation {
        actions::record_operation(state, model::FileOperation::unknown(&operation));
        actions::poll(state);
    }
}
pub(crate) fn recovery_view(state: RwSignal<State>) -> impl IntoView {
    view! {
        <Show when=move ||state.with(|s| s.files.container_draft_commit.is_some()
            || s.files.container_draft_recovery_error.is_some())>
            <section class="zxs-frame zxs-stack" aria-label=t("手动稿提交", "Shared draft commit")>
                <p role="status">{t(
                    "这是显式文件提交。重试保留原文件、稿代数和请求；提交结果不明时不会另建提交。",
                    "This is an explicit file commit. Retry keeps the original file, draft generation and request; an uncertain result never creates another commit.",
                )}</p>
                <p role="status">{move || state.with(|s| s.files.container_draft_recovery_error
                    .as_ref().map(ToString::to_string).unwrap_or_default())}</p>
                <button class="zxs-button" on:click=move |_|restore(state)>
                    {t("重新读取本机恢复记录", "Reload local recovery records")}
                </button>
                <p role="status">{move || state.with(|s| s.files.container_draft_commit.as_ref()
                    .map(|intent| intent.error.clone()).unwrap_or_default())}</p>
                <button class="zxs-button" disabled=move ||!state.with(can_retry_commit)
                    on:click=move |_|retry_commit(state)>{t("重试原提交", "Retry original commit")}</button>
                <button class="zxs-button" disabled=move ||state.with(|s|
                    s.files.container_draft_commit.as_ref()
                        .is_none_or(|intent| intent.in_flight || intent.commit_started || intent.envelope.is_some()))
                    on:click=move |_|cancel_preparation(state)>
                    {t("取消本次提交准备（保留共享稿）", "Cancel preparation (keep shared draft)")}
                </button>
            </section>
        </Show>
    }
}
