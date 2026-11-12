//! Existing directory context for branch review, navigation and physical layout repair.
mod layout;
use super::actions;
use crate::{
    api,
    ui::{State, accept_read_response},
};
use std::collections::BTreeSet;
use storage_contract::directory_git::{DirectoryGitChildStatus, DirectoryGitStatus};
use web_interface::{components::ErrorState, i18n::t, prelude::*, tasks::spawn_local};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    epoch: u64,
    generation: u64,
    repository: String,
    path: String,
    namespace_head: String,
}
impl Target {
    fn capture(state: &State) -> Option<Self> {
        if !state.session_active()
            || !state.files.loaded
            || state.files.loading
            || state.files.repository_id.is_empty()
            || state.files.directory_preview.is_some()
            || state.files.file_preview.is_some()
        {
            return None;
        }
        Some(Self {
            epoch: state.session_epoch,
            generation: state.files.generation,
            repository: state.files.repository_id.clone(),
            path: state.files.repository_path.clone(),
            namespace_head: state.files.namespace_head.clone(),
        })
    }
    fn endpoint(&self) -> String {
        format!(
            "/api/repositories/{}/directory-git?path={}",
            api::segment(&self.repository),
            api::segment(&self.path)
        )
    }
}
#[derive(Clone, Default)]
struct Model {
    target: Option<Target>,
    review_target: Option<Target>,
    read_scope: Option<(u64, String, String)>,
    status: Option<DirectoryGitStatus>,
    error: Option<api::Error>,
    cancellation: Option<api::ReadCancellation>,
    reading: bool,
    request_generation: u64,
}
impl Model {
    fn cancel(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.abort();
        }
    }
    fn begin(&mut self, target: Option<Target>) -> u64 {
        self.cancel();
        let scope = target
            .as_ref()
            .map(|target| (target.epoch, target.repository.clone(), target.path.clone()));
        // A listing refresh temporarily has no target. Retain the old context
        // for display, but reviewed() denies authority until its read completes.
        if scope.is_some() && self.read_scope != scope {
            self.status = None;
            self.review_target = None;
            self.read_scope = scope;
        }
        self.target = target;
        self.error = None;
        self.reading = self.target.is_some();
        self.request_generation += 1;
        self.request_generation
    }
    fn current(&self, target: &Target, generation: u64) -> bool {
        self.target.as_ref() == Some(target) && self.request_generation == generation
    }
    fn reviewed<'a>(&'a self, state: &State) -> Option<(&'a Target, &'a DirectoryGitStatus)> {
        if self.reading || self.error.is_some() {
            return None;
        }
        let target = self.target.as_ref()?;
        if self.review_target.as_ref() != Some(target)
            || Target::capture(state).as_ref() != Some(target)
        {
            return None;
        }
        Some((target, self.status.as_ref()?))
    }
    fn ready<'a>(&'a self, state: &State) -> Option<(&'a Target, &'a DirectoryGitStatus)> {
        if !actions::write_allowed(state) {
            return None;
        }
        self.reviewed(state)
    }
}
fn invalid() -> api::Error {
    api::error(
        storage_contract::ErrorCode::InvalidResponse,
        t(
            "目录仓库响应未通过核对，请刷新。",
            "The directory repository response could not be verified. Refresh its status.",
        ),
    )
}
fn valid_path(path: &str) -> bool {
    path.is_empty() || name_policy::LookupPath::parse(path).is_ok()
}
fn descendant(path: &str, parent: &str) -> bool {
    !path.is_empty()
        && (parent.is_empty()
            || path
                .strip_prefix(parent)
                .is_some_and(|suffix| suffix.starts_with('/')))
}
fn directory_id(id: &str, path: &str, repository: &str) -> bool {
    if path.is_empty() {
        id == repository
    } else {
        storage_contract::validate_id(id, "entry").is_ok()
    }
}
fn decode(value: json_codec::Value, target: &Target) -> Result<DirectoryGitStatus, api::Error> {
    let status: DirectoryGitStatus = json_codec::from_value(value).map_err(|source| {
        api::Error::invalid_response(
            t(
                "目录仓库响应格式无效。",
                "The directory repository response did not match its contract.",
            ),
            api::ErrorSource::DirectoryGit(source),
        )
    })?;
    if status.repository_id != target.repository || status.path != target.path {
        return Err(api::error(
            storage_contract::ErrorCode::HistoryChanged,
            t(
                "目录位置已变化，请刷新后再操作。",
                "The folder location changed. Refresh before making changes.",
            ),
        ));
    }
    if storage_contract::validate_id(&status.repository_id, "repo").is_err()
        || !valid_path(&status.path)
        || !directory_id(&status.directory_id, &status.path, &status.repository_id)
        || status.owner.as_ref().is_some_and(|owner| {
            !valid_path(&owner.path)
                || !directory_id(&owner.directory_id, &owner.path, &status.repository_id)
                || (owner.path != status.path && !descendant(&status.path, &owner.path))
        })
        || status
            .directory_branch_id
            .as_ref()
            .is_some_and(|id| storage_contract::validate_id(id, "dbr").is_err())
        || [&status.working_tree_id, &status.head_tree_id]
            .into_iter()
            .flatten()
            .any(|id| storage_contract::validate_id(id, "dtree").is_err())
    {
        return Err(invalid());
    }
    if status.initialized {
        if status.directory_branch_id.is_none()
            || status.working_tree_id.is_none()
            || status.index_generation == 0
            || (status.head_tree_id.is_none() && !status.staged)
            || !status.owner.as_ref().is_some_and(|owner| {
                owner.directory_id == status.directory_id && owner.path == status.path
            })
        {
            return Err(invalid());
        }
    } else if status.directory_branch_id.is_some()
        || status.working_tree_id.is_some()
        || status.head_tree_id.is_some()
        || status.index_generation != 0
        || status.staged
        || status.unstaged
        || status.needs_restage
        || status.added != 0
        || status.modified != 0
        || status.deleted != 0
        || !status.children.is_empty()
        || status
            .owner
            .as_ref()
            .is_some_and(|owner| owner.directory_id == status.directory_id)
    {
        return Err(invalid());
    }
    let mut children = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for child in &status.children {
        if !valid_path(&child.path)
            || !descendant(&child.path, &status.path)
            || !directory_id(&child.directory_id, &child.path, &status.repository_id)
            || !children.insert(&child.directory_id)
            || !paths.insert(&child.path)
            || storage_contract::validate_id(&child.directory_branch_id, "dbr").is_err()
            || [
                &child.head_tree_id,
                &child.staged_tree_id,
                &child.committed_tree_id,
            ]
            .into_iter()
            .flatten()
            .any(|id| storage_contract::validate_id(id, "dtree").is_err())
            || child.reference_changed != (child.head_tree_id != child.staged_tree_id)
        {
            return Err(invalid());
        }
    }
    Ok(status)
}
fn load(state: RwSignal<State>, model: RwSignal<Model>, target: Option<Target>) {
    let generation = model.try_update(|model| model.begin(target.clone()));
    let (Some(target), Some(generation)) = (target, generation) else {
        return;
    };
    let cancellation = match api::ReadCancellation::new() {
        Ok(cancellation) => cancellation,
        Err(error) => {
            model.update(|model| {
                model.reading = false;
                model.error = Some(error);
            });
            return;
        }
    };
    model.update(|model| model.cancellation = Some(cancellation.clone()));
    spawn_local(async move {
        let result = api::cancellable_get(&target.endpoint(), cancellation)
            .await
            .and_then(|value| decode(value, &target));
        if !accept_read_response(state, target.epoch, &result)
            || !state
                .try_with_untracked(|state| Target::capture(state).as_ref() == Some(&target))
                .unwrap_or(false)
            || !model
                .try_with_untracked(|model| model.current(&target, generation))
                .unwrap_or(false)
        {
            return;
        }
        model.update(|model| {
            model.cancellation = None;
            model.reading = false;
            match result {
                Ok(status) => {
                    model.status = Some(status);
                    model.review_target = Some(target);
                }
                Err(error) => model.error = Some(error),
            }
        });
    });
}
fn path_label(path: &str) -> String {
    if path.is_empty() {
        t("根目录", "Root directory")
    } else {
        path.into()
    }
}

fn repository_destination(state: &State, model: &Model, directory: &str) -> Option<String> {
    if actions::pending(state) {
        return None;
    }
    let (target, status) = model.reviewed(state)?;
    if directory == status.directory_id {
        return None;
    }
    let path = status
        .owner
        .as_ref()
        .filter(|owner| owner.directory_id == directory)
        .map(|owner| owner.path.as_str())
        .or_else(|| {
            status
                .children
                .iter()
                .find(|child| child.directory_id == directory)
                .map(|child| child.path.as_str())
        })?;
    // Named repositories have a displayed root prefix; disk-root repositories
    // do not. Preserve that prefix when leaving the current repository path.
    let prefix = if target.path.is_empty() {
        state.files.directory.as_str()
    } else if name_policy::path_key(&state.files.directory) == name_policy::path_key(&target.path) {
        ""
    } else {
        state.files.directory.split_once('/')?.0
    };
    Some(if path.is_empty() {
        prefix.to_owned()
    } else {
        super::model::join_path(prefix, path)
    })
}

fn open_repository(state: RwSignal<State>, model: RwSignal<Model>, directory: &str) {
    let destination = state.with_untracked(|state| {
        model.with_untracked(|model| repository_destination(state, model, directory))
    });
    if let Some(destination) = destination {
        actions::go_directory_details(state, destination);
    }
}

struct ChildSnapshot {
    path: String,
    branch: String,
    tree: String,
}

fn child_snapshot(state: &State, model: &Model, directory: &str) -> Option<ChildSnapshot> {
    let path = repository_destination(state, model, directory)?;
    let child = model
        .status
        .as_ref()?
        .children
        .iter()
        .find(|child| child.directory_id == directory)?;
    Some(ChildSnapshot {
        path,
        branch: child.directory_branch_id.clone(),
        tree: child.head_tree_id.clone()?,
    })
}

fn preview_child(state: RwSignal<State>, model: RwSignal<Model>, directory: &str) {
    let snapshot = state.with_untracked(|state| {
        model.with_untracked(|model| child_snapshot(state, model, directory))
    });
    if let Some(snapshot) = snapshot {
        actions::preview_directory_snapshot(state, snapshot.path, snapshot.branch, snapshot.tree);
    }
}

fn child_view(
    state: RwSignal<State>,
    model: RwSignal<Model>,
    child: DirectoryGitChildStatus,
) -> impl IntoView {
    let committed = child.head_tree_id.is_some();
    let id = StoredValue::new(child.directory_id);
    view! {
        <section class="zxs-frame zxs-stack" data-directory-git-child=id.get_value()>
            <h3 class="zxs-row">{path_label(&child.path)}</h3>
            <button
                type="button"
                class="zxs-button"
                data-open-directory-child=id.get_value()
                disabled=move ||state.with(|state|model.with(|model| {
                    repository_destination(state,model,&id.get_value()).is_none()
                }))
                on:click=move |_|open_repository(state,model,&id.get_value())
            >
                {t("打开子目录详情", "Open child folder Details")}
            </button>
            <button
                type="button"
                class="zxs-button"
                data-preview-directory-child=id.get_value()
                disabled=move ||!committed || state.with(|state| {
                    actions::pending(state) || model.with(|model|model.reviewed(state).is_none())
                })
                on:click=move |_|preview_child(state,model,&id.get_value())
            >
                {t("预览已有子目录快照", "Preview existing child snapshot")}
            </button>
            <Show when=move ||committed>
                <p class="zxs-row">
                    {t(
                        "此预览只读取已有快照，不包含未提交工作，也不修改父目录引用。",
                        "Preview excludes uncommitted work and does not change parent references.",
                    )}
                </p>
            </Show>
        </section>
    }
}

pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    let model = RwSignal::new(Model::default());
    let target = Memo::new(move |_| state.with(Target::capture));
    let children = Memo::new(move |_| {
        model.with(|model| {
            model
                .status
                .as_ref()
                .map(|status| status.children.clone())
                .unwrap_or_default()
        })
    });
    Effect::new(move |_| load(state, model, target.get()));
    on_cleanup(move || {
        let _ = model.try_update(Model::cancel);
    });
    view! {
        <Show when=move ||target.get().is_some()>
            <section class="zxs-stack" aria-label=t("已有目录上下文", "Existing folder context")>
                <h3 class="zxs-row">{t("已有目录上下文", "Existing folder context")}</h3>
                <p class="zxs-row">
                    {t(
                        "Web 已移除原目录仓库的初始化、暂存、提交及分支写操作。以下只保留已有上下文和布局操作，不代表新的 .zxs 目录快照功能。",
                        concat!(
                            "Directory initialization, staging, commit and branch writes were removed ",
                            "from Web. Existing context and layout are not the new .zxs snapshots.",
                        ),
                    )}
                </p>
                <Show when=move ||model.with(|model|model.reading)>
                    <p class="zxs-row" role="status">
                        {t("正在读取目录上下文…", "Reading folder context…")}
                    </p>
                </Show>
                {move ||model.with(|model|model.error.clone()).map(|error| {
                    let feedback=crate::ui::feedback(&error);
                    view! {<ErrorState message=feedback.message request_id=feedback.request_id/>}
                })}
                {move ||model.with(|model|model.status.clone()).map(|status|view! {
                    <p
                        class="zxs-row"
                        role="status"
                        data-directory-git-initialized=status.initialized.to_string()
                        data-directory-git-staged=status.staged.to_string()
                        data-directory-git-unstaged=status.unstaged.to_string()
                        data-directory-git-needs-restage=status.needs_restage.to_string()
                    >
                        {if status.initialized {
                            t("此目录已有登记上下文。", "This folder has an existing registered context.")
                        } else {
                            t("此目录没有独立登记上下文。", "This folder has no independent registered context.")
                        }}
                    </p>
                    {status.owner.map(|owner| {
                        let current = owner.directory_id == status.directory_id;
                        let label = path_label(&owner.path);
                        let id = StoredValue::new(owner.directory_id);
                        view! {
                            <div class="zxs-flow">
                                <p class="zxs-row">
                                    {format!("{}: {}",t("当前工作归属", "Current work owner"),label)}
                                </p>
                                {(!current).then(move ||view! {
                                    <button
                                        type="button"
                                        class="zxs-button"
                                        data-open-directory-owner=id.get_value()
                                        disabled=move ||state.with(|state|model.with(|model| {
                                            repository_destination(state,model,&id.get_value()).is_none()
                                        }))
                                        on:click=move |_|open_repository(state,model,&id.get_value())
                                    >
                                        {t("打开所属目录详情", "Open owning folder Details")}
                                    </button>
                                })}
                            </div>
                        }
                    })}
                })}
                <Show when=move ||model.with(|model| {
                    model.status.as_ref().is_some_and(|status|status.initialized)
                })>
                    <For
                        each=move ||children.get()
                        key=|child|(
                            child.directory_id.clone(),
                            child.path.clone(),
                            child.directory_branch_id.clone(),
                            child.head_tree_id.clone(),
                        )
                        children=move |child|child_view(state,model,child)
                    />
                </Show>
                {layout::view(state, model)}
            </section>
        </Show>
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/staging_tests.rs"]
mod tests;
