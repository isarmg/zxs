//! Management health is independent of the data backend. Unknown is fail-closed,
//! and no storage/authentication error is ever interpreted as a fresh install.
use crate::{
    api, files,
    ui::{State, feedback},
};
use json_codec::{Value, json};
use web_interface::{
    components::{ErrorState, Feedback},
    i18n::t,
    prelude::*,
    tasks::spawn_local,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Storage {
    #[default]
    Unknown,
    Ready,
    ReadOnly,
    Unavailable,
    Unconfigured,
    RecoveryRequired,
    TerminalManagement,
}
impl Storage {
    pub(crate) fn writable(self) -> bool {
        self == Self::Ready
    }
    pub(crate) fn readable(self) -> bool {
        matches!(self, Self::Ready | Self::ReadOnly)
    }
    fn parse(value: &Value) -> Self {
        match value["state"].as_str() {
            Some("ready") => Self::Ready,
            Some("read_only") => Self::ReadOnly,
            Some("unavailable") => Self::Unavailable,
            Some("unconfigured") => Self::Unconfigured,
            Some("recovery_required") => Self::RecoveryRequired,
            Some("terminal_management") => Self::TerminalManagement,
            _ => Self::Unknown,
        }
    }
    fn label(self) -> String {
        match self {
            Self::Unknown => t(
                "存储状态尚未确认；写入已禁用",
                "Storage status is unconfirmed; writes are disabled",
            ),
            Self::Ready => t("存储可读写", "Storage is ready"),
            Self::ReadOnly => t(
                "存储只读；写入已禁用",
                "Storage is read-only; writes are disabled",
            ),
            Self::Unavailable => t(
                "数据存储不可用；管理服务仍可使用",
                "Data storage is unavailable; management remains accessible",
            ),
            Self::Unconfigured => t(
                "尚未添加存储，请在管理页添加硬盘与存储目录",
                "Storage has not been added. Add disks and storage directories in Administration",
            ),
            Self::TerminalManagement => t(
                "最后一个元数据成员已退出，现为管理态；登记与历史仍保留，普通读写已关闭",
                "The final metadata member has exited. Management retains registrations and history; ordinary storage access is closed",
            ),
            Self::RecoveryRequired => t(
                "存储需要恢复；不会自动初始化或重置账户",
                "Storage recovery is required; no automatic initialization or account reset",
            ),
        }
    }
}
#[derive(Clone, Default)]
pub(crate) struct Model {
    pub storage: Storage,
    pub auth_recovery: bool,
    pub loading: bool,
    pub retrying: bool,
    pub retryable: bool,
    pub recovering: bool,
    pub failure: Option<Feedback>,
    pub retry_unknown: bool,
    generation: u64,
}
impl Model {
    /// A mutation can invalidate a health read admitted before it settled.
    /// Keep the occupied read slot until completion; its response cannot make
    /// storage writable again and will schedule a fresh observation.
    pub(crate) fn invalidate_storage(&mut self) {
        self.storage = Storage::Unknown;
        self.generation += 1;
    }

    pub(crate) fn needs_attention(&self) -> bool {
        !matches!(self.storage, Storage::Ready | Storage::Unconfigured)
            || self.auth_recovery
            || self.failure.is_some()
            || self.retrying
            || self.recovering
            || self.retry_unknown
    }

    fn retry_visible(&self) -> bool {
        (self.retryable && self.storage != Storage::Unconfigured)
            || self.retrying
            || self.recovering
            || self.retry_unknown
    }
}
fn accept(model: &mut Model, value: &Value) {
    model.storage = Storage::parse(&value["storage"]);
    model.auth_recovery = value["authentication"] == "recovery_required";
    model.retryable = value["storage"]["retryable"] == true;
    model.recovering = value["storage"]["recovering"] == true;
    model.failure = None;
}
fn invalidate_restored_disks(state: &mut State, was_readable: bool) {
    if !was_readable && state.management.storage.readable() {
        state.files.invalidate_disks();
    }
}
fn needs_disk_refresh(state: &State, was_readable: bool) -> bool {
    state.session_active()
        && state.management.storage.readable()
        // An authorized account may legitimately have no visible disks.
        // Only an unfetched listing or restored storage needs an automatic read.
        && (!was_readable || !state.files.disks_loaded)
}

fn accept_status(s: &mut State, value: &Value) -> bool {
    let was_readable = s.management.storage.readable();
    accept(&mut s.management, value);
    invalidate_restored_disks(s, was_readable);
    // A successful status check resolves the retry admission fence,
    // not any outstanding data mutation's outcome.
    s.management.retry_unknown = false;
    let load_files = needs_disk_refresh(s, was_readable);
    // Recovery remains available in Administration and above blocked pages.
    // A global status refresh never replaces the current page or its draft owners.
    load_files
}

#[derive(Clone, Copy)]
enum Refresh {
    Status,
    CurrentFolder,
}
#[derive(PartialEq, Eq)]
struct FolderRefresh {
    page: crate::ui::Page,
    generation: u64,
}
impl FolderRefresh {
    fn capture(state: &State) -> Self {
        Self {
            page: state.page,
            generation: state.files.generation,
        }
    }
}
struct StatusRead {
    epoch: u64,
    generation: u64,
    folder: Option<FolderRefresh>,
}
fn begin_load(state: &mut State, refresh: Refresh) -> Option<StatusRead> {
    // Never let a pre-retry snapshot resolve the retry outcome fence.
    // The completion path starts a fresh read after the POST settles.
    if state.management.loading
        || state.management.retrying
        || state.logout_pending()
        || state.logout_unconfirmed()
    {
        return None;
    }
    let folder = match refresh {
        Refresh::Status => None,
        Refresh::CurrentFolder => {
            if !state.session_active()
                || !matches!(
                    state.page,
                    crate::ui::Page::Files | crate::ui::Page::Details
                )
                || state.files.slot.is_empty()
                || state.files.loading
                || files::pending(state)
            {
                return None;
            }
            // A fresh health read, not the cached Ready value, precedes the
            // requested directory read. This does not settle the prior mutation.
            state.management.storage = Storage::Unknown;
            Some(FolderRefresh::capture(state))
        }
    };
    state.management.loading = true;
    state.management.generation += 1;
    Some(StatusRead {
        epoch: state.session_epoch,
        generation: state.management.generation,
        folder,
    })
}
#[derive(Debug, PartialEq, Eq)]
enum StatusCompletion {
    Obsolete,
    Superseded,
    Accepted { disks: bool, folder: bool },
}
fn complete_load(
    state: &mut State,
    read: &StatusRead,
    result: Result<Value, api::Error>,
) -> StatusCompletion {
    if state.session_epoch != read.epoch || state.logout_pending() || state.logout_unconfirmed() {
        return StatusCompletion::Obsolete;
    }
    state.management.loading = false;
    if state.management.generation != read.generation {
        return StatusCompletion::Superseded;
    }
    match result {
        Ok(value) => {
            let disks = accept_status(state, &value);
            let folder = read.folder.as_ref() == Some(&FolderRefresh::capture(state))
                && state.session_active()
                && state.management.storage.readable()
                && !state.files.loading
                && !files::pending(state);
            StatusCompletion::Accepted { disks, folder }
        }
        Err(error) => {
            state.management.storage = Storage::Unknown;
            state.management.failure = Some(feedback(&error));
            StatusCompletion::Accepted {
                disks: false,
                folder: false,
            }
        }
    }
}

pub(crate) fn load(state: RwSignal<State>) {
    load_with(state, Refresh::Status);
}

/// Explicit read-only recovery step. Never retries a mutation or opens storage.
pub(crate) fn refresh_storage_and_folder(state: RwSignal<State>) {
    load_with(state, Refresh::CurrentFolder);
}

fn load_with(state: RwSignal<State>, refresh: Refresh) {
    let Some(read) = state
        .try_update(|state| begin_load(state, refresh))
        .flatten()
    else {
        return;
    };
    spawn_local(async move {
        let result = api::request("GET", "/api/management/status", None).await;
        match state.try_update(|state| complete_load(state, &read, result)) {
            Some(StatusCompletion::Superseded) => load(state),
            Some(StatusCompletion::Accepted { disks, folder }) => {
                if disks {
                    files::load_disks(state);
                }
                if folder {
                    files::load_entries(state);
                }
            }
            Some(StatusCompletion::Obsolete) | None => {}
        }
    });
}

fn retry(state: RwSignal<State>) {
    let Some(epoch) = state
        .try_update(|s| {
            if !s.session_active()
                || s.management.retrying
                || s.management.loading
                || s.management.recovering
                || !s.management.retryable
                || s.management.retry_unknown
            {
                return None;
            }
            s.management.retrying = true;
            s.management.failure = None;
            Some(s.session_epoch)
        })
        .flatten()
    else {
        return;
    };
    spawn_local(async move {
        let result = api::request("POST", "/api/system/storage/retry", Some(json!({}))).await;
        if !crate::ui::current_session(state, epoch) {
            return;
        }
        state.update(|s| {
            s.management.retrying = false;
            match result {
                Ok(value) => {
                    let was_readable = s.management.storage.readable();
                    s.management.storage = Storage::parse(&value);
                    s.management.retryable = value["retryable"] == true;
                    s.management.recovering = value["recovering"] == true;
                    // Fast recovery may already be ready in the POST response.
                    // Keep its refresh demand for the following status read too.
                    invalidate_restored_disks(s, was_readable);
                }
                Err(error) => {
                    s.management.retry_unknown = error.mutation_outcome_unknown();
                    if error.status == 401 {
                        s.authenticated = false;
                    }
                    s.management.failure = Some(feedback(&error));
                }
            }
        });
        load(state);
    });
}
pub(crate) fn summary(state: RwSignal<State>) -> impl IntoView {
    view! {
        <section class="zxs-content-panel zxs-stack" aria-label=t("存储与认证状态","Storage and authentication status")>
            <p class="zxs-row" role="status">{move ||state.with(|s|s.management.storage.label())}</p>
            <Show when=move ||state.with(|s|s.management.auth_recovery)><p class="zxs-row">{t("认证状态需要本地恢复。不会因数据盘故障创建新管理员或清空凭据。","Authentication requires local recovery. A disk failure never creates a new administrator or clears credentials.")}</p></Show>
            <div class="zxs-flow"><button class="zxs-button" disabled=move ||state.with(|s|s.management.loading||s.management.retrying||s.logout_pending()) on:click=move |_|load(state)>{t("检查管理状态","Check management status")}</button>
            <Show when=move ||state.with(|s|s.session_active()&&s.management.retry_visible())><button class="zxs-button" disabled=move ||state.with(|s|s.management.loading||s.management.retrying||s.management.recovering||!s.management.retryable||s.management.retry_unknown) on:click=move |_|retry(state)>{t("重试打开数据存储","Retry opening data storage")}</button></Show></div>
            <Show when=move ||state.with(|s|s.management.retry_unknown)><p class="zxs-row" role="alert">{t("重试结果尚未确认。请先检查管理状态，不要重复提交。","The retry outcome is unconfirmed. Check management status before submitting again.")}</p></Show>
            {move ||state.with(|s|s.management.failure.clone()).map(|failure|view!{<ErrorState message=failure.message request_id=failure.request_id/>})}
        </section>
    }
}
pub(crate) fn recovery(state: RwSignal<State>) -> impl IntoView {
    view! {
        <section class="zxs-stack zxs-management-recovery">
            <h2 class="zxs-row">{t("全局存储与认证恢复","Storage and authentication recovery")}</h2>
            {summary(state)}
            <p class="zxs-row">{t("这里显示全局服务状态，不代表硬盘健康。存储故障不影响管理访问；恢复磁盘挂载后可重试打开，重试不会格式化磁盘。各目录所属设备的健康信息显示在下面的目录信息块中。","This is global service status, not disk health. Management remains accessible during storage faults. Restore disk mounts before retrying; retry does not format disks. Device health appears in the corresponding directory card below.")}</p>
        </section>
    }
}
#[cfg(test)]
#[path = "../../../test/web/unit/management/tests.rs"]
mod tests;
