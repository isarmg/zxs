//! Storage administration for the sole authenticated account.
mod request;
mod response;
use crate::{
    api,
    ui::{State, current_session, fail},
};
use json_codec::Value;
use web_interface::i18n::t;
use web_interface::{prelude::*, tasks::spawn_local};

use request::Kind;

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
    fn label(&self) -> &str {
        match self {
            Self::MissingId => "unknown",
            Self::Known(id) => id,
        }
    }
}

#[derive(Clone)]
pub(crate) struct Model {
    fields: request::Fields,
    kind: Kind,
    busy: bool,
    result: String,
    error: String,
    plan: Value,
    acknowledged: bool,
    uncertain_operation: Option<UncertainOperation>,
}
impl Default for Model {
    fn default() -> Self {
        Self {
            fields: request::Fields::new(),
            kind: Kind::Inspect,
            busy: false,
            result: String::new(),
            error: String::new(),
            plan: Value::Null,
            acknowledged: false,
            uncertain_operation: None,
        }
    }
}
impl Model {
    pub(crate) fn reload_blocked(&self) -> bool {
        self.busy || self.uncertain_operation.is_some()
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
        self.plan = Value::Null;
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
fn execute(
    state: RwSignal<State>,
    method: &'static str,
    path: String,
    body: Option<Value>,
    plan: bool,
    operation: String,
) {
    if !allowed(state, method != "GET") {
        return;
    }
    let epoch = state.with_untracked(|s| s.session_epoch);
    state.update(|s| {
        s.admin_console.busy = true;
        s.admin_console.error.clear();
    });
    spawn_local(async move {
        let contract = state.with_untracked(|s| {
            response::Contract::new(method, &path, body.as_ref(), &operation, &s.principal)
        });
        let result = api::request(method, &path, body)
            .await
            .and_then(|value| contract.validate(value));
        if !current_session(state, epoch) {
            return;
        }
        state.update(|s| s.admin_console.busy = false);
        match result {
            Ok(value) => {
                state.update(|s| accept(&mut s.admin_console, method, &path, plan, value));
            }
            Err(error) => {
                state.update(|s| reject(&mut s.admin_console, method, plan, &operation, &error));
                if error.status == 401 {
                    fail(state, error)
                }
            }
        }
    });
}
fn reject(m: &mut Model, method: &str, plan: bool, operation: &str, error: &api::Error) {
    m.error = error.to_string();
    if method != "GET" && !plan && error.mutation_outcome_unknown() {
        m.uncertain_operation = Some(if operation.is_empty() {
            UncertainOperation::MissingId
        } else {
            UncertainOperation::Known(operation.into())
        });
    }
}

/// Call only after validating the endpoint-specific contract.
fn accept(m: &mut Model, method: &str, path: &str, plan: bool, value: Value) {
    if plan {
        m.plan = value.clone();
        m.acknowledged = false;
    }
    if path.ends_with("/commit") || path.ends_with("/cancel") {
        m.plan = Value::Null;
        m.acknowledged = false;
    }
    m.result = json_codec::to_string_pretty(&value).expect("invariant: JSON values serialize");
    // An inspection cannot prove completion of an earlier write. Only its
    // exact terminal operation receipt can release the uncertainty fence.
    if method != "GET"
        || m.uncertain_operation
            .as_ref()
            .and_then(UncertainOperation::id)
            .is_some_and(|id| {
                path == format!("/api/operations/{}", api::segment(id))
                    && response::terminal(&value, id)
            })
    {
        m.uncertain_operation = None;
        if method == "GET" {
            m.plan = Value::Null;
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
        Ok((method, path, body)) => {
            state.update(|s| {
                s.admin_console.plan = Value::Null;
                s.admin_console.acknowledged = false;
            });
            execute(
                state,
                method,
                path,
                Some(body),
                matches!(kind, Kind::Purge | Kind::Split | Kind::Save),
                operation,
            );
        }
        Err(error) => state.update(|s| s.admin_console.error = error.to_string()),
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
            &s.admin_console.plan,
            commit,
            s.admin_console.acknowledged,
            &operation,
        )
    });
    match result {
        Ok((method, path, body)) => execute(state, method, path, Some(body), false, operation),
        Err(error) => state.update(|s| s.admin_console.error = error.to_string()),
    }
}
fn select(state: RwSignal<State>, kind: Kind) {
    if !allowed(state, false)
        || state.with_untracked(|s| s.admin_console.uncertain_operation.is_some())
    {
        return;
    }
    state.update(|s| {
        s.admin_console.kind = kind;
        s.admin_console.fields = request::initial_fields(kind);
        s.admin_console.plan = Value::Null;
        s.admin_console.acknowledged = false;
        s.admin_console.error.clear();
        s.admin_console.result.clear();
    });
}

fn field(state: RwSignal<State>, key: &'static str, label: &'static str) -> impl IntoView {
    view! {<label class="zxs-flow"><span>{label}</span><input class="zxs-input" type="text" autocomplete="off" disabled=move ||state.with(|s|s.admin_console.reload_blocked()) prop:value=move ||state.with(|s|s.admin_console.fields.get(key).cloned().unwrap_or_default()) on:input=move |ev|state.update(|s|s.admin_console.edit_field(key,event_target_value(&ev)))/></label>}
}
fn toggle(state: RwSignal<State>, key: &'static str, label: &'static str) -> impl IntoView {
    view! {<label class="zxs-row"><input type="checkbox" disabled=move ||state.with(|s|s.admin_console.reload_blocked()) prop:checked=move ||state.with(|s|s.admin_console.fields.get(key).is_some_and(|v|v=="true")) on:change=move |ev|state.update(|s|s.admin_console.edit_field(key,event_target_checked(&ev).to_string()))/>{label}</label>}
}
fn query(
    state: RwSignal<State>,
    prefix: &'static str,
    key: &'static str,
    suffix: &'static str,
    plan: bool,
) {
    if !allowed(state, false) {
        return;
    }
    let value =
        state.with_untracked(|s| s.admin_console.fields.get(key).cloned().unwrap_or_default());
    if !key.is_empty() && value.trim().is_empty() {
        state.update(|s| s.admin_console.error = format!("{key} is required"));
        return;
    }
    let path = if key.is_empty() {
        prefix.into()
    } else {
        format!("{prefix}/{}{suffix}", api::segment(&value))
    };
    execute(state, "GET", path, None, plan, String::new());
}
pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {<section class="zxs-flow" aria-label="Administration">
    <h2>{t("日常管理","Daily administration")}</h2>
    <p>{t("唯一账户管理全部存储。数据盘不可写时，存储操作禁用。账号设置可修改密码；文件内容请使用 SMB。","The sole account manages all storage. Storage changes require writable data storage. Change the password in account settings; use SMB for file contents.")}</p>
    <nav class="zxs-flow">{[(Kind::Repository,"仓库 Repository"),(Kind::Smb,"SMB 绑定 Binding"),(Kind::Policy,"路径副本规则 Path copy rule"),(Kind::Rule,"主盘规则 Placement"),(Kind::Branch,"历史分支 Branch"),(Kind::Link,"合并连线 Link"),(Kind::Purge,"清理预览 Purge"),(Kind::Split,"拆分预览 Split"),(Kind::Save,"历史保存预览 Historical save"),(Kind::Inspect,"检查 / 查询 Inspect")].into_iter().map(move |(kind,label)|view!{<button class="zxs-button" disabled=move ||state.with(|s|s.admin_console.busy||s.admin_console.uncertain_operation.is_some()) on:click=move |_|select(state,kind)>{label}</button>}).collect_view()}</nav>
    <p role="alert">{move ||state.with(|s|s.admin_console.error.clone())}</p>
    <Show when=move ||state.with(|s|s.admin_console.uncertain_operation.is_some())><p role="alert">{t("结果未确认，禁止重复修改。请查询原操作结果，不要改用新操作 ID 重发；无法查询时请在服务器本地核实。","Outcome unknown; do not repeat mutations. Check the original operation instead of resending under a new ID. Verify locally on the server if its outcome cannot be queried.")}</p><p>{move ||state.with(|s|s.admin_console.uncertain_operation.as_ref().map(|operation| operation.label().to_owned()))}</p><button class="zxs-button" disabled=move ||state.with(|s|!permitted(s,false)||s.admin_console.uncertain_operation.as_ref().and_then(UncertainOperation::id).is_none()) on:click=move |_|{let Some(id)=state.with_untracked(|s|s.admin_console.uncertain_operation.as_ref().and_then(UncertainOperation::id).map(str::to_owned)) else {return};execute(state,"GET",format!("/api/operations/{}",api::segment(&id)),None,false,String::new())}>{t("查询原操作","Check original operation")}</button></Show>
    <Show when=move ||state.with(|s|s.admin_console.kind==Kind::Policy)><p>{t("此表单仅创建或修改无稳定文件／目录身份绑定的现行路径规则，只影响后续版本，不回填历史。稳定文件／目录规则请使用对应的「备份设置」；文件保留设置请显式配置该文件。这些备份设置会为仍可用的历史正文补足副本。","This form only creates or updates current path-only rules without a stable file/directory scope. Changes affect future versions only and do not backfill history. Use the file or directory's Backup settings for stable scoped rules, or the file's Backup settings for retained settings. Saving backup settings also backfills copies for available historical content.")}</p></Show>
    {move ||{let kind=state.with(|s|s.admin_console.kind); let fields:Vec<(&'static str,&'static str)>=match kind{
        Kind::Repository=>vec![("name","名称 Name"),("slot","主盘槽位 Primary slot"),("copies","份数（含主盘；1 份无冗余） Copies including primary; 1 has no redundancy")],
        Kind::Smb=>vec![("repository","仓库 ID"),("branch","分支 ID"),("generation","Expected history generation")],
        Kind::Policy=>vec![("id","路径规则 ID Path rule ID"),("repository","仓库 ID"),("prefix","路径前缀 Path prefix"),("copies","份数（含主盘；1 份无冗余） Copies including primary; 1 has no redundancy"),("generation","Expected generation (0 for new)")],
        Kind::Rule=>vec![("id","规则 ID"),("repository","仓库 ID"),("prefix","路径前缀 Path prefix"),("slot","主盘槽位 Primary slot"),("generation","Expected generation (0 for new)")],
        Kind::Branch=>vec![("repository","仓库 ID"),("commit","来源提交 Source commit"),("name","名称 Name")],
        Kind::Link=>vec![("history","历史树 History ID"),("source_branch","来源分支 Source branch"),("source_commit","来源提交 Source commit"),("target_branch","目标分支 Target branch"),("target_commit","目标提交 Target commit")],
        Kind::Purge=>vec![("history","历史树 History ID"),("versions","版本 IDs (comma separated)")],
        Kind::Split=>vec![("history","历史树 History ID"),("commit","切分提交 Cut commit ID")],
        Kind::Save=>vec![("history","历史树 History ID"),("commit","来源提交 Source commit"),("name","名称 Name"),("workspace","Existing workspace ID (optional)"),("slot","Primary slot (optional)"),("copies","Copies (optional)")],
        Kind::Inspect=>vec![("repository","仓库 ID"),("id","操作 / 计划 / 策略 / 拓扑 ID")]};fields.into_iter().map(move |(key,label)|field(state,key,label)).collect_view()}}
    <Show when=move ||state.with(|s|s.admin_console.kind==Kind::Smb)>{toggle(state,"enabled","启用 SMB Enabled")}</Show>
    <Show when=move ||state.with(|s|s.admin_console.kind!=Kind::Inspect)><button class="zxs-button" disabled=move ||state.with(|s|!permitted(s,true)||s.admin_console.uncertain_operation.is_some()) on:click=move |_|submit(state)>"保存配置 / 创建预览 Save configuration / Create preview"</button></Show>
    <Show when=move ||state.with(|s|s.admin_console.kind==Kind::Inspect)>{[("Repositories","/api/repositories","","",false),("Disks","/api/admin/disks","","",false),("Integrity check","/api/admin/check","","",false),("Repository","/api/repositories","repository","",false),("SMB binding","/api/repositories","repository","/smb-binding",false),("Operation","/api/operations","id","",false),("Plan","/api/plans","id","",true),("Branch tree","/api/branches","id","/tree",false),("Branch details","/api/branches","id","",false),("File history","/api/files","id","/history",false),("History graph","/api/history-trees","id","/graph",false),("Policy","/api/policies","id","",false),("Placement rule","/api/primary-placement-rules","id","",false),("Topology","/api/backup-topologies/default","","",false)].into_iter().map(move |(label,prefix,key,suffix,plan)|view!{<button class="zxs-button" disabled=move ||state.with(|s|s.admin_console.busy) on:click=move |_|query(state,prefix,key,suffix,plan)>{label}</button>}).collect_view()}</Show>
    <pre>{move ||state.with(|s|s.admin_console.result.clone())}</pre>
    <Show when=move ||state.with(|s|s.admin_console.plan.is_object())><h3>"计划预览 Plan review"</h3><pre>{move ||state.with(|s|json_codec::to_string_pretty(&s.admin_console.plan).expect("invariant: JSON values serialize"))}</pre><label><input type="checkbox" prop:checked=move ||state.with(|s|s.admin_console.acknowledged) disabled=move ||state.with(|s|s.admin_console.busy) on:change=move |ev|state.update(|s|s.admin_console.acknowledged=event_target_checked(&ev))/>{t("我已审阅此计划及永久删除等后果，确认执行所示摘要对应的计划。","I reviewed this plan and its consequences, including permanent deletion where applicable, and confirm this exact digest.")}</label><button class="zxs-button" disabled=move ||state.with(|s|!permitted(s,true)||!s.admin_console.acknowledged||s.admin_console.uncertain_operation.is_some()) on:click=move |_|plan_action(state,true)>"提交所示计划 Commit reviewed plan"</button><button class="zxs-button" disabled=move ||state.with(|s|!permitted(s,true)||s.admin_console.uncertain_operation.is_some()) on:click=move |_|plan_action(state,false)>"取消计划 Cancel plan"</button></Show>
    </section>}
}

#[cfg(test)]
#[path = "../../../../test/web/unit/admin/tests.rs"]
mod tests;
