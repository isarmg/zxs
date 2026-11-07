//! Per-scope custom prefixes and explicit recursive disk-name protection.
pub(crate) mod recursive;
use super::{actions, model::file_context, review};
use crate::{
    api,
    ui::{State, accept_read_response, fail},
};
use data_serialization::{Deserialize, Deserializer};
use json_codec::{Value, json};
use std::collections::BTreeMap;
use web_interface::components::{LoadingState, Select, SelectOption};
use web_interface::tasks::spawn_local;
use web_interface::{i18n::t, prelude::*};

#[derive(Clone)]
pub(crate) struct Draft {
    id: String,
    repository: String,
    path: String,
    configuration: Option<Configuration>,
    loading: bool,
}

// Only the projection used by this editor is decoded here. Unused server
// fields, including nullable policy IDs, do not affect this read/save boundary.
#[derive(Clone, Deserialize)]
struct Configuration {
    scope: Scope,
    #[serialization(with = "storage_contract::u64_string")]
    generation: u64,
    inherited: bool,
    target_copies: u32,
    backup_prefixes: BTreeMap<String, String>,
    #[serialization(deserialize_with = "read_routes")]
    backup_routes: Vec<Vec<String>>,
    max_copies: usize,
    backup_paths: BTreeMap<String, String>,
}

#[derive(Clone, Deserialize)]
struct Scope {
    id: String,
}

fn read_routes<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Vec<String>>, D::Error> {
    let routes = Vec::<Vec<String>>::deserialize(deserializer)?;
    if routes
        .iter()
        .any(|route| route.is_empty() || route.iter().any(String::is_empty))
    {
        return Err(data_serialization::de::Error::custom(
            "empty backup route or slot",
        ));
    }
    Ok(routes)
}

fn read_configuration(value: Value) -> Result<Configuration, api::Error> {
    Configuration::deserialize(value).map_err(configuration_error)
}
fn configuration_error(source: json_codec::Error) -> api::Error {
    api::Error {
        code: storage_contract::ErrorCode::InvalidResponse,
        message: t(
            "备份配置响应无效，请关闭后重新读取。",
            "The backup configuration response is invalid. Close and reload.",
        ),
        retryable: false,
        status: 200,
        request_id: None,
        source: Some(std::sync::Arc::new(api::ErrorSource::BackupConfiguration(
            source,
        ))),
    }
}
pub(crate) fn start_entry(state: RwSignal<State>, entry: super::page::Entry) {
    let Some((repository, path)) = file_context(&entry) else {
        return;
    };
    start(state, repository, path);
}
pub(crate) fn start_current(state: RwSignal<State>) {
    let (repository, path) = state.with_untracked(|s| {
        (
            s.files.repository_id.clone(),
            s.files.repository_path.clone(),
        )
    });
    if !repository.is_empty() {
        start(state, repository, path);
    }
}
fn start(state: RwSignal<State>, repository: String, path: String) {
    if state.with_untracked(|s| !s.session_active() || actions::pending(s)) {
        return;
    }
    let Some(id) = crate::ui::file_operation_id(state) else {
        return;
    };
    let epoch = state.with_untracked(|s| s.session_epoch);
    review::reset(state, review::Kind::Backup);
    state.update(|s| {
        s.files.recursive_backup = None;
        s.files.backup = Some(Draft {
            id: id.clone(),
            repository: repository.clone(),
            path: path.clone(),
            configuration: None,
            loading: true,
        })
    });
    spawn_local(async move {
        let result = api::validated_request(
            "GET",
            &format!(
                "/api/repositories/{}/backup-configuration?path={}",
                api::segment(&repository),
                api::segment(&path)
            ),
            None,
            read_configuration,
        )
        .await;
        complete_configuration_load(state, epoch, &id, result);
    });
}
fn complete_configuration_load(
    state: RwSignal<State>,
    epoch: u64,
    id: &str,
    result: Result<Configuration, api::Error>,
) {
    if !accept_read_response(state, epoch, &result)
        || !state.with_untracked(|s| s.files.backup.as_ref().is_some_and(|draft| draft.id == id))
    {
        return;
    }
    match result {
        Ok(configuration) => state.update(|s| {
            if let Some(draft) = s.files.backup.as_mut() {
                draft.configuration = Some(configuration);
                draft.loading = false;
            }
        }),
        Err(error) => {
            state.update(|s| {
                if let Some(draft) = s.files.backup.as_mut() {
                    draft.loading = false;
                }
            });
            review::error(state, review::Kind::Backup, error.to_string());
            fail(state, error);
        }
    }
}
fn max_copies(configuration: &Configuration) -> usize {
    let available = configuration
        .backup_routes
        .iter()
        .map(Vec::len)
        .min()
        .unwrap_or(0);
    configuration.max_copies.min(available)
}
fn target_slots(configuration: &Configuration) -> Vec<String> {
    configuration
        .backup_routes
        .iter()
        .flat_map(|route| {
            route
                .iter()
                .skip(1)
                .take(configuration.target_copies.saturating_sub(1) as usize)
                .cloned()
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}
fn valid_prefix(prefix: &str) -> bool {
    prefix.len() <= 768 && (prefix.is_empty() || name_policy::validate_path(prefix).is_ok())
}
fn request(draft: &Draft) -> Result<Value, String> {
    if draft.loading {
        return Err(t(
            "备份策略仍在加载，请稍后再试。",
            "The backup configuration is still loading. Try again when it is ready.",
        ));
    }
    let Some(configuration) = draft.configuration.as_ref() else {
        return Err(t(
            "保护策略依据缺失，请关闭后重新读取。",
            "The policy baseline is missing. Close and reload.",
        ));
    };
    if configuration.scope.id.is_empty() {
        return Err(t(
            "保护策略依据缺失，请关闭后重新读取。",
            "The policy baseline is missing. Close and reload.",
        ));
    }
    if configuration.target_copies == 0
        || configuration.target_copies as usize > max_copies(configuration)
    {
        return Err(t(
            "所选份数超出这条备份路径的能力。",
            "The copy count exceeds this backup route’s capacity.",
        ));
    }
    let mut prefixes = BTreeMap::new();
    for slot in target_slots(configuration) {
        let prefix = configuration
            .backup_prefixes
            .get(&slot)
            .cloned()
            .unwrap_or_default();
        if !valid_prefix(&prefix) {
            return Err(t(
                "备份前缀必须是相对路径，最多768个UTF-8字节，可留空。",
                "Backup prefixes must be relative paths of at most 768 UTF-8 bytes, or empty.",
            ));
        }
        prefixes.insert(slot, prefix);
    }
    Ok(
        json!({"path":draft.path,"target_copies":configuration.target_copies,"backup_prefixes":prefixes,"expected_generation":configuration.generation.to_string(),"expected_scope_id":configuration.scope.id}),
    )
}
fn submit(state: RwSignal<State>) {
    if state.with_untracked(|s| !actions::write_allowed(s)) {
        return;
    }
    let Some(draft) = state.with_untracked(|s| s.files.backup.clone()) else {
        return;
    };
    match request(&draft) {
        Ok(body) => review::submit(
            state,
            review::Kind::Backup,
            "PUT",
            format!(
                "/api/repositories/{}/backup-configuration",
                api::segment(&draft.repository)
            ),
            body,
        ),
        Err(error) => review::error(state, review::Kind::Backup, error),
    }
}
pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    let open = Memo::new(move |_| state.with(|s| s.files.backup.is_some()));
    let disabled = Signal::derive(move || {
        state.with(|s| {
            !actions::write_allowed(s)
                || s.files
                    .backup
                    .as_ref()
                    .is_none_or(|draft| draft.loading || draft.configuration.is_none())
        })
    });
    let options = Signal::derive(move || {
        state.with(|s| {
            s.files
                .backup
                .as_ref()
                .and_then(|draft| draft.configuration.as_ref())
                .map(|configuration| {
                    (1..=max_copies(configuration))
                        .map(|n| {
                            SelectOption::new(
                                n.to_string(),
                                format!("{n} {}", t("份（含主盘）", "copies (including primary)")),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
    });
    let target = Signal::derive(move || {
        state.with(|s| {
            s.files
                .backup
                .as_ref()
                .and_then(|draft| draft.configuration.as_ref())
                .map(|configuration| configuration.target_copies.to_string())
                .unwrap_or_default()
        })
    });
    let slots = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .backup
                .as_ref()
                .and_then(|draft| draft.configuration.as_ref())
                .map(target_slots)
                .unwrap_or_default()
        })
    });
    let paths = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .backup
                .as_ref()
                .and_then(|draft| draft.configuration.as_ref())
                .map(|configuration| {
                    configuration
                        .backup_paths
                        .clone()
                        .into_iter()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });
    let custom = view! {<Show when=move ||open.get()><section class="zxs-frame zxs-stack zxs-backup-review" aria-label=t("备份保护设置","Backup protection settings")>
        <div class="zxs-file-toolbar zxs-flow"><h3 class="zxs-row">{t("备份保护设置","Backup protection settings")}</h3><button class="zxs-button" disabled=move ||state.with(actions::pending) on:click=move |_|{state.update(|s|s.files.backup=None);review::reset(state,review::Kind::Backup);}>{t("关闭","Close")}</button></div>
        <p class="zxs-row">{move ||state.with(|s|s.files.backup.as_ref().map(|draft|if draft.path.is_empty(){"/".into()}else{draft.path.clone()}).unwrap_or_default())}</p>
        <Show when=move ||state.with(|s|s.files.backup.as_ref().is_some_and(|draft|draft.loading))><LoadingState/></Show>
        <Show when=move ||state.with(|s|s.files.backup.as_ref().is_some_and(|draft|!draft.loading&&draft.configuration.is_some()))>
            <p class="zxs-row">{t("为每个备份硬盘分别设置逻辑前缀；文件使用前缀加文件名，文件夹使用前缀加目录内相对路径。主盘路径保持原样。","Set a separate logical prefix for each backup disk. Files append their name; folders append paths relative to the selected folder. Primary paths stay unchanged.")}</p>
            <Show when=move ||state.with(|s|s.files.backup.as_ref().is_some_and(|draft|draft.configuration.as_ref().is_some_and(|configuration|configuration.inherited)))><p class="zxs-row">{t("当前继承上级规则；保存后将为此范围单独设置。","This scope currently inherits its parent’s rule. Saving creates a rule for this scope.")}</p></Show>
            <div class="zxs-file-toolbar zxs-flow"><Select label=t("目标份数","Target copy count") options=options value=target disabled=disabled on_change=Callback::new(move |value:String|state.update(|s|if let Some(configuration)=s.files.backup.as_mut().and_then(|draft|draft.configuration.as_mut()){configuration.target_copies=value.parse().unwrap_or(0);}))/></div>
            {move ||slots.get().into_iter().map(move |slot|{let read=slot.clone();let write=slot.clone();let label=format!("{slot} {}",t("备份前缀","backup prefix"));view!{<label class="zxs-form-field"><span class="zxs-row">{label.clone()}</span><input class="zxs-input zxs-input--compact" aria-label=label data-max-bytes="768" disabled=move ||disabled.get() prop:value=move ||state.with(|s|s.files.backup.as_ref().and_then(|draft|draft.configuration.as_ref()).and_then(|configuration|configuration.backup_prefixes.get(&read)).cloned().unwrap_or_default()) on:input=move |event|state.update(|s|if let Some(configuration)=s.files.backup.as_mut().and_then(|draft|draft.configuration.as_mut()){configuration.backup_prefixes.insert(write.clone(),event_target_value(&event));})/></label>}}).collect_view()}
            <div class="zxs-stack">{move ||paths.get().into_iter().map(|(slot,path)|view!{<p class="zxs-row">{format!("{slot} / {path}")}</p>}).collect_view()}</div>
            <Show when=move ||state.with(|s|!s.management.storage.writable())><p class="zxs-row">{t("此备份策略仅可查看。","This backup configuration is read only.")}</p></Show>
            <button class="zxs-button" disabled=move ||disabled.get() on:click=move |_|submit(state)>{t("保存备份设置","Save backup settings")}</button>
        </Show>
        <p class="zxs-row zxs-inline-error" role="status">{move ||state.with(|s|review::error_text(s,review::Kind::Backup))}</p>
    </section></Show>};
    (custom, recursive::view(state))
}
#[cfg(test)]
#[path = "../../../../test/web/unit/files/backup/tests.rs"]
mod tests;
