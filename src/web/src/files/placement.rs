//! Directory and repository primary selection, separate from backup protection.
use super::{actions, model::file_context, review};
use crate::{
    api,
    ui::{State, accept_read_response, fail},
};
use data_serialization::{Deserialize, Deserializer};
use json_codec::{Value, json};
use web_interface::{
    components::{LoadingState, Select, SelectOption},
    i18n::t,
    prelude::*,
    tasks::spawn_local,
};

#[derive(Clone)]
pub(crate) struct Draft {
    id: String,
    repository: String,
    path: String,
    expected_scope: Option<String>,
    configuration: Option<Configuration>,
    selected: String,
    loading: bool,
}
#[derive(Clone, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Configuration {
    repository_id: String,
    repository_name: String,
    scope: Scope,
    #[serialization(deserialize_with = "nullable_rule")]
    rule_id: Option<String>,
    #[serialization(with = "storage_contract::u64_string")]
    generation: u64,
    primary_slot_id: String,
    inherited: bool,
    required_copies: u32,
    targets: Vec<Target>,
}
#[derive(Clone, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Scope {
    id: String,
    path: String,
}
#[derive(Clone, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Target {
    slot_id: String,
    root: String,
    #[serialization(deserialize_with = "nullable_block")]
    unavailable_reason: Option<storage_contract::PrimaryPlacementBlock>,
}
fn nullable_block<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<storage_contract::PrimaryPlacementBlock>, D::Error> {
    Option::deserialize(deserializer)
}
fn nullable_rule<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Option::deserialize(deserializer)
}
fn decode_error(source: json_codec::Error) -> api::Error {
    api::Error::invalid_response(
        t(
            "主盘规则响应无效，请重新读取。",
            "The primary-placement response is invalid. Reload it.",
        ),
        api::ErrorSource::PlacementConfiguration(source),
    )
}
fn read_configuration(value: Value, draft: &Draft) -> Result<Configuration, api::Error> {
    let configuration = Configuration::deserialize(value).map_err(decode_error)?;
    let valid_path = configuration.scope.path.is_empty()
        || name_policy::validate_path(&configuration.scope.path).is_ok();
    let unique_targets = configuration
        .targets
        .iter()
        .map(|target| &target.slot_id)
        .collect::<std::collections::BTreeSet<_>>();
    if configuration.repository_id != draft.repository
        || configuration.scope.id.is_empty()
        || draft
            .expected_scope
            .as_ref()
            .is_some_and(|scope| *scope != configuration.scope.id)
        || !valid_path
        || name_policy::path_key(&configuration.scope.path) != name_policy::path_key(&draft.path)
        || configuration.rule_id.as_ref().is_some_and(String::is_empty)
        || configuration.inherited != configuration.rule_id.is_none()
        || configuration.rule_id.is_none() != (configuration.generation == 0)
        || configuration.required_copies == 0
        || configuration.targets.is_empty()
        || unique_targets.len() != configuration.targets.len()
        || configuration
            .targets
            .iter()
            .any(|target| target.slot_id.is_empty() || target.root.is_empty())
        || name_policy::validate_slot(&configuration.primary_slot_id).is_err()
    {
        return Err(decode_error(
            <json_codec::Error as data_serialization::de::Error>::custom(
                "placement response does not match the requested directory and rule baseline",
            ),
        ));
    }
    Ok(configuration)
}

pub(crate) fn start_entry(state: RwSignal<State>, entry: super::page::Entry) {
    if entry.kind() != storage_contract::NamespaceEntryKind::Directory
        || entry.read_only()
        || entry.backup()
    {
        return;
    }
    if let Some((repository, path)) = file_context(&entry) {
        start(state, repository, path, Some(entry.entry_id().into()));
    }
}
pub(crate) fn start_current(state: RwSignal<State>) {
    let (repository, path) = state.with_untracked(|s| {
        (
            s.files.repository_id.clone(),
            s.files.repository_path.clone(),
        )
    });
    if !repository.is_empty() {
        start(state, repository, path, None);
    }
}
fn start(state: RwSignal<State>, repository: String, path: String, expected_scope: Option<String>) {
    if !state.with_untracked(actions::write_allowed) {
        return;
    }
    let Some(id) = crate::ui::operation_id(state) else {
        return;
    };
    let epoch = state.with_untracked(|s| s.session_epoch);
    let draft = Draft {
        id: id.clone(),
        repository,
        path,
        expected_scope,
        configuration: None,
        selected: String::new(),
        loading: true,
    };
    review::reset(state, review::Kind::Placement);
    state.update(|s| s.files.placement = Some(draft.clone()));
    spawn_local(async move {
        let result = api::validated_request(
            "GET",
            &format!(
                "/api/repositories/{}/placement-configuration?path={}",
                api::segment(&draft.repository),
                api::segment(&draft.path)
            ),
            None,
            |value| read_configuration(value, &draft),
        )
        .await;
        complete_load(state, epoch, &id, result);
    });
}
fn complete_load(
    state: RwSignal<State>,
    epoch: u64,
    id: &str,
    result: Result<Configuration, api::Error>,
) {
    if !accept_read_response(state, epoch, &result)
        || !state.with_untracked(|s| {
            s.files
                .placement
                .as_ref()
                .is_some_and(|draft| draft.id == id)
        })
    {
        return;
    }
    match result {
        Ok(configuration) => state.update(|s| {
            let draft = s
                .files
                .placement
                .as_mut()
                .expect("invariant: matching placement draft remains current");
            draft.selected = configuration
                .targets
                .iter()
                .find(|target| target.slot_id == configuration.primary_slot_id)
                .map(|target| target.slot_id.clone())
                .unwrap_or_default();
            draft.configuration = Some(configuration);
            draft.loading = false;
        }),
        Err(error) => {
            state.update(|s| {
                s.files
                    .placement
                    .as_mut()
                    .expect("invariant: matching placement draft remains current")
                    .loading = false
            });
            review::error(state, review::Kind::Placement, error.to_string());
            fail(state, error);
        }
    }
}
fn request(draft: &Draft) -> Result<Value, String> {
    let configuration = draft
        .configuration
        .as_ref()
        .filter(|_| !draft.loading)
        .ok_or_else(|| {
            t(
                "请先读取此目录的主盘规则。",
                "Load this folder’s primary-placement rule first.",
            )
        })?;
    if !configuration
        .targets
        .iter()
        .any(|target| target.slot_id == draft.selected && target.unavailable_reason.is_none())
    {
        return Err(t(
            "请选择当前可写且能满足此范围备份份数和已设置前缀的主盘。",
            "Choose a writable primary that supports this scope’s backup counts and configured prefixes.",
        ));
    }
    Ok(
        json!({"path":configuration.scope.path,"primary_slot_id":draft.selected,
        "expected_scope_id":configuration.scope.id,"expected_rule_id":configuration.rule_id,
        "expected_generation":configuration.generation.to_string()}),
    )
}
fn submit(state: RwSignal<State>) {
    if !state.with_untracked(actions::write_allowed) {
        return;
    }
    let Some(draft) = state.with_untracked(|s| s.files.placement.clone()) else {
        return;
    };
    match request(&draft) {
        Ok(body) => review::submit(
            state,
            review::Kind::Placement,
            "PUT",
            format!(
                "/api/repositories/{}/placement-configuration",
                api::segment(&draft.repository)
            ),
            body,
        ),
        Err(error) => review::error(state, review::Kind::Placement, error),
    }
}
fn reload(state: RwSignal<State>) {
    if let Some(draft) = state.with_untracked(|s| s.files.placement.clone()) {
        start(state, draft.repository, draft.path, draft.expected_scope);
    }
}
fn options(configuration: &Configuration) -> Vec<SelectOption> {
    configuration
        .targets
        .iter()
        .map(|target| {
            let mut option = SelectOption::new(
                &target.slot_id,
                format!(
                    "{} · {}{}",
                    target.root,
                    target.slot_id,
                    match target.unavailable_reason {
                        None => String::new(),
                        Some(storage_contract::PrimaryPlacementBlock::InsufficientCopies) =>
                            format!(" · {}", t("备份链份数不足", "Insufficient backup copies")),
                        Some(storage_contract::PrimaryPlacementBlock::RouteUnavailable) => format!(
                            " · {}",
                            t("存储链已下线或移除", "Storage route is offline or removed")
                        ),
                        Some(storage_contract::PrimaryPlacementBlock::MissingBackupPrefixes) =>
                            format!(
                                " · {}",
                                t(
                                    "目标备份盘未设置前缀",
                                    "Target backup prefixes are not configured"
                                )
                            ),
                    }
                ),
            );
            option.disabled = target.unavailable_reason.is_some();
            option
        })
        .collect()
}
pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    let open = Memo::new(move |_| state.with(|s| s.files.placement.is_some()));
    let disabled = Signal::derive(move || {
        state.with(|s| {
            !actions::write_allowed(s)
                || s.files
                    .placement
                    .as_ref()
                    .is_none_or(|draft| draft.loading || draft.configuration.is_none())
        })
    });
    let choices = Signal::derive(move || {
        state.with(|s| {
            s.files
                .placement
                .as_ref()
                .and_then(|draft| draft.configuration.as_ref())
                .map(options)
                .unwrap_or_default()
        })
    });
    let value = Signal::derive(move || {
        state.with(|s| {
            s.files
                .placement
                .as_ref()
                .map(|draft| draft.selected.clone())
                .unwrap_or_default()
        })
    });
    view! {
        <Show when=move ||open.get()>
            <section class="zxs-frame zxs-stack zxs-placement-review" aria-label=t("主盘规则", "Primary placement")>
                <div class="zxs-file-toolbar zxs-flow">
                    <h3 class="zxs-row">{t("后续版本存放位置", "Store future versions on")}</h3>
                    <button class="zxs-button" disabled=move ||state.with(actions::pending) on:click=move |_|{
                        state.update(|s|s.files.placement=None);review::reset(state,review::Kind::Placement);
                    }>{t("关闭", "Close")}</button>
                </div>
                <p class="zxs-row">{move ||state.with(|s|s.files.placement.as_ref().map(|draft|{
                    let path=if draft.path.is_empty(){"/"}else{&draft.path};
                    draft.configuration.as_ref().map(|configuration|format!("{} / {}",configuration.repository_name,path)).unwrap_or_else(||path.into())
                }).unwrap_or_default())}</p>
                <p class="zxs-row">{t("只决定此目录及子目录后续新版本使用的主盘，不搬移已有版本，也不改变备份份数或备份前缀。子目录已有的独立主盘规则继续生效。", "This sets the primary for future versions in this folder and its subfolders. Existing versions stay where they are, and backup counts and prefixes stay unchanged. Independent child placement rules still apply.")}</p>
                <p class="zxs-row">{t("存储链含已下线或移除的目录，或目标备份盘缺少前缀时不能切换；此处不会自动改写备份设置。", "A route with offline or removed storage directories, or missing backup prefixes, cannot be selected. This editor does not change backup settings automatically.")}</p>
                <Show when=move ||state.with(|s|s.files.placement.as_ref().and_then(|draft|draft.configuration.as_ref()).is_some_and(|configuration|!configuration.targets.iter().any(|target|target.slot_id==configuration.primary_slot_id)))>
                    <p class="zxs-row">{t("原主盘已移除，请选择当前已登记的主盘。旧版本和原规则保留，保存后只改变后续版本。", "The previous primary was removed. Choose a registered primary. Saving changes only future placement. Existing versions stay in place.")}</p>
                </Show>
                <p class="zxs-row">{t("规则按当前目录路径生效；目录改名或移动后，请在新位置重新检查主盘规则。", "The rule applies to the current folder path. After renaming or moving the folder, review placement at its new location.")}</p>
                <Show when=move ||state.with(|s|s.files.placement.as_ref().is_some_and(|draft|draft.loading))><LoadingState/></Show>
                <Show when=move ||state.with(|s|s.files.placement.as_ref().is_some_and(|draft|draft.configuration.as_ref().is_some_and(|configuration|configuration.inherited)))>
                    <p class="zxs-row">{t("当前沿用上级或仓库主盘；保存后为此路径单独设置。", "This path currently inherits its parent or repository primary. Saving creates a rule for this path.")}</p>
                </Show>
                <Select label=t("主盘目录", "Primary directory") options=choices value=value disabled=disabled
                    on_change=Callback::new(move |value:String|state.update(|s|if let Some(draft)=s.files.placement.as_mut(){draft.selected=value;}))/>
                <p class="zxs-row">{move ||state.with(|s|s.files.placement.as_ref().and_then(|draft|draft.configuration.as_ref()).map(|configuration|
                    format!("{} {}",t("此范围的备份链至少需要的总份数：", "Minimum total copies required by this scope:"),configuration.required_copies)).unwrap_or_default())}</p>
                <div class="zxs-file-toolbar zxs-flow">
                    <button class="zxs-button" disabled=move ||disabled.get() on:click=move |_|submit(state)>{t("保存主盘规则", "Save primary placement")}</button>
                    <button class="zxs-button" disabled=move ||state.with(|s|!actions::write_allowed(s)||s.files.placement.as_ref().is_some_and(|draft|draft.loading)) on:click=move |_|reload(state)>{t("重新读取规则", "Reload current rule")}</button>
                </div>
                <p class="zxs-row zxs-inline-error" role="status">{move ||state.with(|s|review::error_text(s,review::Kind::Placement))}</p>
            </section>
        </Show>
    }
}
#[cfg(test)]
#[path = "../../../../test/web/unit/files/placement.rs"]
mod tests;
