//! Storage administration for the sole authenticated account.
mod backup_transaction;
mod disks;
mod messages;
mod navigation;
pub(crate) use navigation::Section;
mod request;
mod response;
mod storage;
use crate::{
    api,
    ui::{State, current_session, fail},
};
use messages::Message;
use web_interface::{prelude::*, tasks::spawn_local};

pub use disks::DecodeError as DiskDetailsError;
pub use storage::ResponseError as StorageManagementError;
pub(crate) fn poll_storage_activation(state: RwSignal<State>) {
    storage::poll(state);
    backup_transaction::poll(state);
}
pub(crate) fn can_load_disks(state: &State) -> bool {
    storage::can_load(state) && disks::can_load(state)
}
pub(crate) fn disks_refreshing(state: &State) -> bool {
    storage::refreshing(state) || disks::refreshing(state)
}
pub(crate) fn load_disks(state: RwSignal<State>) {
    storage::load(state);
}
use request::{Kind, QueryKind};
pub(crate) use response::ResponseError;
use response::{Contract, ValidatedAdminResponse, ValidatedPlan};

impl From<ResponseError> for crate::api::Error {
    fn from(source: ResponseError) -> Self {
        let mut error = crate::api::error(
            storage_contract::ErrorCode::InvalidAdminResponse,
            Message::ResponseUnconfirmed.text(),
        );
        error.status = 200;
        error.source = Some(std::sync::Arc::new(crate::api::ErrorSource::AdminResponse(
            source,
        )));
        error
    }
}

#[derive(Clone, Debug)]
enum Failure {
    Remote(api::Error),
    Input(std::sync::Arc<request::InputError>),
}
impl Failure {
    fn display(&self) -> String {
        match self {
            Self::Remote(error) => crate::ui::feedback(error).display(),
            Self::Input(error) => messages::input_error(error),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum UncertainOperation {
    MissingId,
    Known(String),
}
impl UncertainOperation {
    fn id(&self) -> Option<&str> {
        match self {
            Self::MissingId => None,
            Self::Known(id) => Some(id),
        }
    }
    fn label(&self) -> String {
        match self {
            Self::MissingId => Message::UnknownOperation.text(),
            Self::Known(id) => id.clone(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct Model {
    backup: backup_transaction::Model,
    disks: disks::Model,
    storage: storage::Model,
    fields: request::Fields,
    kind: Kind,
    busy: bool,
    result: String,
    error: Option<Failure>,
    plan: Option<ValidatedPlan>,
    acknowledged: bool,
    uncertain_operation: Option<UncertainOperation>,
}
impl Default for Model {
    fn default() -> Self {
        Self {
            backup: backup_transaction::Model::default(),
            disks: disks::Model::default(),
            storage: storage::Model::default(),
            fields: request::Fields::new(),
            kind: Kind::Inspect,
            busy: false,
            result: String::new(),
            error: None,
            plan: None,
            acknowledged: false,
            uncertain_operation: None,
        }
    }
}
impl Model {
    pub(crate) fn reload_blocked(&self) -> bool {
        self.backup.reload_blocked()
            || self.busy
            || self.uncertain_operation.is_some()
            || self.disks.saving()
            || self.storage.reload_blocked()
    }

    /// A reviewed plan belongs to the inputs that produced or selected it.
    /// Editing those inputs requires a new preview, not just a new checkmark.
    fn edit_field(&mut self, key: &str, value: String) {
        if self.reload_blocked()
            || self.fields.get(key).map(String::as_str).unwrap_or_default() == value
        {
            return;
        }
        self.fields.insert(key.into(), value);
        self.plan = None;
        self.acknowledged = false;
        self.result.clear();
    }
}
fn permitted(state: &State, storage: bool) -> bool {
    state.session_active()
        && !state.admin_console.busy
        && (!storage || state.management.storage.writable())
}
fn allowed(state: RwSignal<State>, storage: bool) -> bool {
    state.with_untracked(|s| permitted(s, storage))
}
fn execute(state: RwSignal<State>, request: request::Request) {
    if !allowed(state, request.contract.is_write()) {
        return;
    }
    let epoch = state.with_untracked(|s| s.session_epoch);
    state.update(|s| {
        s.admin_console.busy = true;
        s.admin_console.error = None;
    });
    spawn_local(async move {
        let request::Request {
            method,
            path,
            body,
            contract,
        } = request;
        let result = api::validated_request(method, &path, body, |value| {
            contract.validate(value).map_err(Into::into)
        })
        .await;
        complete(state, epoch, &contract, result);
    });
}

fn complete(
    state: RwSignal<State>,
    epoch: u64,
    contract: &Contract,
    result: Result<ValidatedAdminResponse, api::Error>,
) {
    if !current_session(state, epoch) {
        return;
    }
    state.update(|s| s.admin_console.busy = false);
    match result {
        Ok(value) => state.update(|s| accept(&mut s.admin_console, value)),
        Err(error) => {
            state.update(|s| reject(&mut s.admin_console, contract, &error));
            if error.status == 401 {
                fail(state, error)
            }
        }
    }
}

fn reject(m: &mut Model, contract: &Contract, error: &api::Error) {
    m.error = Some(Failure::Remote(error.clone()));
    if let Some(operation) = contract.mutation_operation()
        && error.mutation_outcome_unknown()
    {
        m.uncertain_operation = Some(if operation.is_empty() {
            UncertainOperation::MissingId
        } else {
            UncertainOperation::Known(operation.into())
        });
    }
}

/// The reducer consumes the boundary proof, never reinterprets display JSON.
fn accept(m: &mut Model, value: ValidatedAdminResponse) {
    use response::Action;
    let matches_write = |operation: &str| match &m.uncertain_operation {
        None => true,
        Some(UncertainOperation::Known(original)) => original == operation,
        Some(UncertainOperation::MissingId) => false,
    };
    let completed = match value.action {
        Action::Inspect => None,
        Action::Plan(plan) => {
            m.plan = Some(plan);
            m.acknowledged = false;
            None
        }
        Action::Outcome { operation, status } => {
            // A diagnostic lookup with no pending write must not discard a review.
            (m.uncertain_operation
                .as_ref()
                .and_then(UncertainOperation::id)
                == Some(operation.as_str())
                && !matches!(
                    status,
                    storage_contract::client_protocol::OperationStatus::Unknown
                ))
            .then_some(true)
        }
        Action::Mutation { operation } => matches_write(&operation).then_some(false),
        Action::PlanReceipt(receipt) => {
            matches_write(&receipt.operation_id.to_string()).then_some(true)
        }
    };
    m.result = value.display;
    if let Some(clear_plan) = completed {
        m.uncertain_operation = None;
        if clear_plan {
            m.plan = None;
            m.acknowledged = false;
        }
    }
}

fn submit(state: RwSignal<State>) {
    let (kind, uncertain) = state.with_untracked(|s| {
        (
            s.admin_console.kind,
            s.admin_console.uncertain_operation.is_some(),
        )
    });
    if !allowed(state, true) || uncertain {
        return;
    }
    let Some(operation) = crate::ui::operation_id(state) else {
        return;
    };
    let request =
        state.with_untracked(|s| request::build(kind, &s.admin_console.fields, &operation));
    match request {
        Ok(request) => {
            state.update(|s| {
                s.admin_console.plan = None;
                s.admin_console.acknowledged = false;
            });
            execute(state, request);
        }
        Err(error) => state
            .update(|s| s.admin_console.error = Some(Failure::Input(std::sync::Arc::new(error)))),
    }
}
fn plan_action(state: RwSignal<State>, commit: bool) {
    if !allowed(state, true)
        || state.with_untracked(|s| s.admin_console.uncertain_operation.is_some())
    {
        return;
    }
    let Some(operation) = crate::ui::operation_id(state) else {
        return;
    };
    let result = state.with_untracked(|s| {
        request::plan_action(
            s.admin_console.plan.as_ref(),
            commit,
            s.admin_console.acknowledged,
            &operation,
        )
    });
    match result {
        Ok(request) => execute(state, request),
        Err(error) => state
            .update(|s| s.admin_console.error = Some(Failure::Input(std::sync::Arc::new(error)))),
    }
}
fn select(state: RwSignal<State>, kind: Kind) {
    if !allowed(state, false)
        || state.with_untracked(|s| {
            s.admin_console.uncertain_operation.is_some() || s.admin_console.kind == kind
        })
    {
        return;
    }
    state.update(|s| {
        s.admin_console.kind = kind;
        s.admin_console.fields = request::initial_fields(kind);
        s.admin_console.plan = None;
        s.admin_console.acknowledged = false;
        s.admin_console.error = None;
        s.admin_console.result.clear();
    });
}

fn field(state: RwSignal<State>, key: &'static str, label: Message<'static>) -> impl IntoView {
    view! {<label class="zxs-form-field"><span>{label.text()}</span><input class="zxs-input" type="text" autocomplete="off" disabled=move ||state.with(|s|s.admin_console.reload_blocked()) prop:value=move ||state.with(|s|s.admin_console.fields.get(key).cloned().unwrap_or_default()) on:input=move |ev|state.update(|s|s.admin_console.edit_field(key,event_target_value(&ev)))/></label>}
}
fn query(state: RwSignal<State>, kind: QueryKind) {
    if !allowed(state, false) {
        return;
    }
    match state.with_untracked(|s| request::query(kind, &s.admin_console.fields)) {
        Ok(request) => execute(state, request),
        Err(error) => state
            .update(|s| s.admin_console.error = Some(Failure::Input(std::sync::Arc::new(error)))),
    }
}
pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {<section class="zxs-content-stack zxs-stack" data-zxs-admin-section="storage">
        <h1 class="zxs-row">{web_interface::i18n::t("磁盘与接入", "Disks and admission")}</h1>
        <p class="zxs-row">{web_interface::i18n::t("更换下一级备份盘必须迁移所有数据，并按真实迁移进度和完成回执确认。下方完整备份事务只覆盖其明确列出的受限初始场景；添加磁盘或保存路径规则不代表替换已完成。", "Replacing the next backup disk must migrate all data and be confirmed by real migration progress and a completion receipt. The complete-backup transaction below covers only its explicitly listed initial scope; adding a disk or saving a path rule does not complete replacement.")}</p>
        {operation_status(state)}
        {backup_transaction::view(state)}
        {storage::view(state)}
        {disks::view(state)}
    </section>}
}

fn operation_status(state: RwSignal<State>) -> impl IntoView {
    view! {
        <p class="zxs-row" role="alert">{move ||state.with(|s|s.admin_console.error.as_ref().map(Failure::display))}</p>
        <Show when=move ||state.with(|s|s.admin_console.uncertain_operation.is_some())>
            <p class="zxs-row" role="alert">{Message::UnknownOutcome.text()}</p>
            <p class="zxs-row">{move ||state.with(|s|s.admin_console.uncertain_operation.as_ref().map(UncertainOperation::label))}</p>
            <button class="zxs-button" disabled=move ||state.with(|s|!permitted(s,false)||s.admin_console.uncertain_operation.as_ref().and_then(UncertainOperation::id).is_none()) on:click=move |_|{
                let Some(id)=state.with_untracked(|s|s.admin_console.uncertain_operation.as_ref().and_then(UncertainOperation::id).map(str::to_owned)) else {return};
                execute(state,request::operation_outcome(&id));
            }>{Message::CheckOriginalOperation.text()}</button>
        </Show>
    }
}

pub(crate) fn console(state: RwSignal<State>, section: Section) -> impl IntoView {
    view! {<section class="zxs-content-stack zxs-stack" data-zxs-admin-section=match section {Section::Repositories=>"repositories",Section::Rules=>"rules",Section::Diagnostics=>"diagnostics"}>
        <h1 class="zxs-row">{section.title()}</h1>
        <p class="zxs-row">{section.help()}</p>
        {(section==Section::Diagnostics).then(move ||crate::management::recovery(state))}
        <nav class="zxs-flow" aria-label=section.title()>{section.actions().iter().copied().map(move |(kind,label)|view!{
            <button class="zxs-button" aria-pressed=move ||state.with(|s|s.admin_console.kind==kind).to_string() disabled=move ||state.with(|s|s.admin_console.busy||s.admin_console.uncertain_operation.is_some()) on:click=move |_|select(state,kind)>{label.text()}</button>
        }).collect_view()}</nav>
        {operation_status(state)}
        <Show when=move ||state.with(|s|section.contains(s.admin_console.kind))>
            <Show when=move ||state.with(|s|s.admin_console.kind==Kind::Policy)><p class="zxs-row">{Message::PathRuleHelp.text()}</p></Show>
            <div class="zxs-form-grid">{move ||{
                let fields=match state.with(|s|s.admin_console.kind) {
                    Kind::Repository=>vec![("name",Message::Name),("slot",Message::PrimarySlot),("copies",Message::Copies)],
                    Kind::Policy=>vec![("id",Message::PathRuleId),("repository",Message::RepositoryId),("prefix",Message::PathPrefix),("copies",Message::Copies),("generation",Message::Generation)],
                    Kind::Rule=>vec![("id",Message::RuleId),("repository",Message::RepositoryId),("prefix",Message::PathPrefix),("slot",Message::PrimarySlot),("generation",Message::Generation)],
                    Kind::Inspect=>vec![("repository",Message::RepositoryId),("id",Message::InspectionId)],
                };
                fields.into_iter().map(move |(key,label)|field(state,key,label)).collect_view()
            }}</div>
            <Show when=move ||state.with(|s|s.admin_console.kind!=Kind::Inspect)><button class="zxs-button" disabled=move ||state.with(|s|!permitted(s,true)||s.admin_console.uncertain_operation.is_some()) on:click=move |_|submit(state)>{Message::Submit.text()}</button></Show>
            <Show when=move ||state.with(|s|s.admin_console.kind==Kind::Inspect)><div class="zxs-flow">{request::QUERIES.into_iter().map(move |(id,kind)|view!{
                <button class="zxs-button" disabled=move ||state.with(|s|!permitted(s,false)) on:click=move |_|query(state,kind)>{messages::query_label(id).text()}</button>
            }).collect_view()}</div></Show>
        </Show>
        <pre class="zxs-admin-result">{move ||state.with(|s|s.admin_console.result.clone())}</pre>
        {(section==Section::Diagnostics).then(move ||view!{
            <Show when=move ||state.with(|s|s.admin_console.plan.is_some())>
                <h2 class="zxs-row">{Message::PlanReview.text()}</h2>
                <pre class="zxs-admin-result">{move ||state.with(|s|s.admin_console.plan.as_ref().map(|plan|json_codec::to_string_pretty(plan.plan()).expect("invariant: plans serialize")))}</pre>
                <label class="zxs-row"><input type="checkbox" prop:checked=move ||state.with(|s|s.admin_console.acknowledged) disabled=move ||state.with(|s|s.admin_console.busy) on:change=move |ev|state.update(|s|s.admin_console.acknowledged=event_target_checked(&ev))/>{Message::AcknowledgePlan.text()}</label>
                <div class="zxs-flow">
                    <button class="zxs-button" disabled=move ||state.with(|s|!permitted(s,true)||!s.admin_console.acknowledged||s.admin_console.uncertain_operation.is_some()) on:click=move |_|plan_action(state,true)>{Message::CommitPlan.text()}</button>
                    <button class="zxs-button" disabled=move ||state.with(|s|!permitted(s,true)||s.admin_console.uncertain_operation.is_some()) on:click=move |_|plan_action(state,false)>{Message::CancelPlan.text()}</button>
                </div>
            </Show>
        })}
    </section>}
}

#[cfg(test)]
#[path = "../../../../test/web/unit/admin/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../test/web/unit/admin/navigation_tests.rs"]
mod navigation_tests;
