//! Explicit independent directory repositories and reviewed child references.
mod layout;
use super::{actions, branch};
use crate::{
    api,
    ui::{State, accept_read_response},
};
use std::{collections::BTreeSet, sync::Arc};
use storage_contract::directory_git::{
    DirectoryBranchCreateSource, DirectoryGitChildStatus, DirectoryGitStatus,
};
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
    draft_scope: Option<(u64, String, String)>,
    status: Option<DirectoryGitStatus>,
    message: String,
    submitted_message: Option<(String, String)>,
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
        // A listing refresh temporarily has no target. Keep this folder's
        // commit text while its namespace and staging state are read again.
        if scope.is_some() && self.draft_scope != scope {
            self.message.clear();
            self.submitted_message = None;
            self.status = None;
            self.review_target = None;
            self.draft_scope = scope;
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
    let status: DirectoryGitStatus =
        json_codec::from_value(value).map_err(|source| api::Error {
            status: 200,
            code: storage_contract::ErrorCode::InvalidResponse,
            message: t(
                "目录仓库响应格式无效。",
                "The directory repository response did not match its contract.",
            ),
            retryable: false,
            request_id: None,
            source: Some(Arc::new(api::ErrorSource::DirectoryGit(source))),
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
/// A nonempty commit note admitted at the text-input boundary.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CommitMessage(String);

impl CommitMessage {
    fn validate(value: &str) -> storage_contract::Result<()> {
        storage_contract::validate_note(value)?;
        if value.trim().is_empty() {
            return Err(storage_contract::Error::new(
                storage_contract::ErrorCode::InvalidRequest,
                "directory commit message must be nonempty",
            ));
        }
        Ok(())
    }

    fn parse(value: &str) -> storage_contract::Result<Self> {
        Self::validate(value)?;
        Ok(Self(value.to_owned()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Review {
    Initialize {
        repository: String,
        path: String,
        namespace_head: String,
    },
    Commit {
        repository: String,
        branch: String,
        tree: String,
        index_generation: u64,
        message: CommitMessage,
    },
    StageChild {
        repository: String,
        directory: String,
        branch: String,
        index_generation: u64,
        child: String,
        head: String,
    },
    StageOwned {
        repository: String,
        directory: String,
        branch: String,
        tree: String,
        index_generation: u64,
    },
}
fn initialize_review(state: &State, model: &Model) -> Option<Review> {
    let (target, status) = model.ready(state)?;
    (!status.initialized).then(|| Review::Initialize {
        repository: target.repository.clone(),
        path: target.path.clone(),
        namespace_head: target.namespace_head.clone(),
    })
}
fn commit_review(state: &State, model: &Model) -> Option<Review> {
    let (target, status) = model.ready(state)?;
    if !status.initialized || status.needs_restage || !status.staged {
        return None;
    }
    let message = match CommitMessage::parse(&model.message) {
        Ok(message) => message,
        Err(_) => return None,
    };
    Some(Review::Commit {
        repository: target.repository.clone(),
        branch: status
            .directory_branch_id
            .clone()
            .expect("invariant: initialized repository has a branch"),
        tree: status
            .working_tree_id
            .clone()
            .expect("invariant: initialized repository has a working tree"),
        index_generation: status.index_generation,
        message,
    })
}
fn owned_review(state: &State, model: &Model) -> Option<Review> {
    let (target, status) = model.ready(state)?;
    if !status.initialized || (!status.unstaged && !status.needs_restage) {
        return None;
    }
    Some(Review::StageOwned {
        repository: target.repository.clone(),
        directory: status.directory_id.clone(),
        branch: status
            .directory_branch_id
            .clone()
            .expect("invariant: initialized repository has a branch"),
        tree: status
            .working_tree_id
            .clone()
            .expect("invariant: initialized repository has a working tree"),
        index_generation: status.index_generation,
    })
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BranchReview {
    target: Target,
    slot: String,
    display_path: String,
    source: DirectoryBranchCreateSource,
}
impl BranchReview {
    pub(super) fn submit(self, state: RwSignal<State>, name: String) -> Option<String> {
        if !state.with_untracked(|state| {
            Target::capture(state).as_ref() == Some(&self.target)
                && state.files.slot == self.slot
                && state.files.directory == self.display_path
        }) {
            return None;
        }
        actions::directory_branch(state, self.source, name)
    }
}

fn branch_review(state: &State, model: &Model) -> Option<BranchReview> {
    if !state.files.directory_branches_supported {
        return None;
    }
    let (target, status) = model.ready(state)?;
    if !status.initialized || status.staged || status.unstaged || status.needs_restage {
        return None;
    }
    Some(BranchReview {
        target: target.clone(),
        slot: state.files.slot.clone(),
        display_path: state.files.directory.clone(),
        source: DirectoryBranchCreateSource {
            repository_id: status.repository_id.clone(),
            directory_id: status.directory_id.clone(),
            directory_branch_id: status.directory_branch_id.clone()?,
            working_tree_id: status.working_tree_id.clone()?,
            head_tree_id: status.head_tree_id.clone()?,
            index_generation: status.index_generation,
            namespace_head: target.namespace_head.clone(),
        },
    })
}

fn child_review(state: &State, model: &Model, id: &str) -> Option<Review> {
    let (target, status) = model.ready(state)?;
    if !status.initialized {
        return None;
    }
    let child = status
        .children
        .iter()
        .find(|child| child.directory_id == id)?;
    if !child.reference_changed {
        return None;
    }
    Some(Review::StageChild {
        repository: target.repository.clone(),
        directory: status.directory_id.clone(),
        branch: status
            .directory_branch_id
            .clone()
            .expect("invariant: initialized repository has a branch"),
        index_generation: status.index_generation,
        child: child.directory_id.clone(),
        head: child.head_tree_id.clone()?,
    })
}
fn submit(state: RwSignal<State>, model: RwSignal<Model>, review: Option<Review>) {
    match review {
        Some(Review::Initialize {
            repository,
            path,
            namespace_head,
        }) => {
            actions::initialize_directory_git(state, repository, path, namespace_head);
        }
        Some(Review::Commit {
            repository,
            branch,
            tree,
            index_generation,
            message,
        }) => {
            let submitted = message.as_str().to_owned();
            if let Some(operation) = actions::commit_directory(
                state,
                repository,
                branch,
                tree,
                index_generation,
                message,
            ) {
                model.update(|model| model.submitted_message = Some((operation, submitted)));
            }
        }
        Some(Review::StageChild {
            repository,
            directory,
            branch,
            index_generation,
            child,
            head,
        }) => {
            actions::stage_directory_child(
                state,
                repository,
                directory,
                branch,
                index_generation,
                child,
                head,
            );
        }
        Some(Review::StageOwned {
            repository,
            directory,
            branch,
            tree,
            index_generation,
        }) => {
            actions::stage_directory_owned(
                state,
                repository,
                directory,
                branch,
                tree,
                index_generation,
            );
        }
        None => {}
    }
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
    view! {<section class="zxs-frame zxs-stack" data-directory-git-child=id.get_value()>
        <h3 class="zxs-row">{path_label(&child.path)}</h3>
        <button type="button" class="zxs-button" data-open-directory-child=id.get_value()
            disabled=move ||state.with(|state|model.with(|model|repository_destination(state,model,&id.get_value()).is_none()))
            on:click=move |_|open_repository(state,model,&id.get_value())>{t("进入子仓库暂存／提交","Open child repository to stage / commit")}</button>
        <button type="button" class="zxs-button" data-preview-directory-child=id.get_value()
            disabled=move ||!committed || state.with(|state| {
                actions::pending(state) || model.with(|model|model.reviewed(state).is_none())
            })
            on:click=move |_|preview_child(state,model,&id.get_value())>
            {t("预览子仓库当前提交", "Preview the child's current commit")}
        </button>
        <Show when=move ||committed>
            <p class="zxs-row">{t("预览只读取子仓库已提交的内容，不包含未提交工作，也不改变父级暂存引用。", "Preview reads the child's committed content, excluding uncommitted work, and does not change the parent's staged reference.")}</p>
        </Show>
        <p class="zxs-row">{if child.dirty{t("子仓库有未提交的工作更改；父级不会代为提交。","The child repository has uncommitted work. A parent commit does not commit it.")}else{t("子仓库工作内容已提交。","The child repository has no uncommitted work.")}}</p>
        <p class="zxs-row">{if child.head_tree_id.is_none(){t("子仓库尚无提交，不能暂存引用。","The child repository has no commit to stage.")}else if child.reference_changed{t("子仓库当前提交与父级暂存引用不同。","The child's current commit differs from the parent's staged reference.")}else{t("父级暂存引用已指向子仓库当前提交。","The parent's staged reference points to the child's current commit.")}}</p>
        <p class="zxs-row">{if child.staged{t("该子仓库引用已暂存，等待父级提交。","This child reference is staged for the next parent commit.")}else{t("该子仓库引用没有已暂存更改。","This child reference has no staged change.")}}</p>
        <Show when=move ||child.needs_restage><p class="zxs-row">{t("子仓库自身的边界有变化，需进入该子仓库重新暂存。父级仍只引用其已提交内容。","The child's repository boundaries changed. Restage inside that child; the parent still references only its committed content.")}</p></Show>
        <button type="button" class="zxs-button" data-stage-directory-child=id.get_value()
            disabled=move ||state.with(|state|model.with(|model|child_review(state,model,&id.get_value()).is_none()))
            on:click=move |_|{let review=state.with_untracked(|state|model.with_untracked(|model|child_review(state,model,&id.get_value())));submit(state,model,review);}>{t("暂存此子仓库的当前提交引用","Stage this child's current commit reference")}</button>
    </section>}
}
pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    let model = RwSignal::new(Model::default());
    let message_error = Memo::new(move |_| {
        model.with(|model| {
            if model.message.is_empty() {
                return None;
            }
            match CommitMessage::validate(&model.message) {
                Ok(()) => None,
                Err(source) => Some(source.to_string()),
            }
        })
    });
    let target = Memo::new(move |_| state.with(Target::capture));
    let branch_source =
        Signal::derive(move || state.with(|state| model.with(|model| branch_review(state, model))));
    let branch_disabled = Signal::derive(move || branch_source.get().is_none());
    let history_target = Memo::new(move |_| {
        state.with(|state| {
            if state.files.selected.is_some() {
                return None;
            }
            model.with(|model| {
                let (_, status) = model.reviewed(state)?;
                Some(super::directory_history::Target::current_directory(
                    state, status,
                ))
            })
        })
    });
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
    Effect::new(move |_| {
        let operation = model.with(|model| {
            model
                .submitted_message
                .as_ref()
                .map(|(operation, _)| operation.clone())
        });
        let Some(operation) = operation else {
            return;
        };
        let status = state.with(|state| {
            state
                .files
                .operations
                .iter()
                .find(|item| item.id() == operation)
                .map(|item| item.status())
        });
        if let Some(status) = status.filter(|status| *status != super::SaveStatus::Unknown) {
            model.update(|model| {
                if let Some((_, submitted)) = model.submitted_message.take()
                    && matches!(
                        status,
                        super::SaveStatus::Published | super::SaveStatus::Degraded
                    )
                    && model.message == submitted
                {
                    model.message.clear();
                }
            });
        }
    });
    on_cleanup(move || {
        let _ = model.try_update(Model::cancel);
    });
    view! {<Show when=move ||target.get().is_some()>
        <section class="zxs-stack zxs-directory-staging" aria-label=t("目录仓库","Directory repository")>
            <h3 class="zxs-row">{t("类 Git 目录仓库","Git-like directory repository")}</h3>
            <p class="zxs-row">{t("文件保存立即持久化，不自动生成目录提交。未单独初始化的子目录属于最近的已初始化祖先；独立子仓库通过明确暂存的提交引用加入父级，父级绝不代替子仓库提交。普通浏览始终读取工作内容，历史需明确选择。","File saves are durable without creating directory commits. Uninitialized folders belong to their nearest initialized ancestor. Independent child repositories enter the parent through explicitly staged commit references; a parent never commits a child. Normal browsing reads working content; history must be selected explicitly.")}</p>
            <Show when=move ||model.with(|model|model.reading)><p class="zxs-row" role="status">{t("正在读取仓库状态…","Reading repository status…")}</p></Show>
            {move ||model.with(|model|model.error.clone()).map(|error|{let feedback=crate::ui::feedback(&error);view!{<ErrorState message=feedback.message request_id=feedback.request_id/>}})}
            {move ||model.with(|model|model.status.clone()).map(|status|view!{
                <p class="zxs-row" role="status" data-directory-staging="true" data-directory-git-initialized=status.initialized.to_string() data-directory-git-staged=status.staged.to_string() data-directory-git-unstaged=status.unstaged.to_string() data-directory-git-needs-restage=status.needs_restage.to_string()>{if !status.initialized{t("当前目录尚未初始化独立仓库。","This directory is not an independent repository yet.")}else if status.head_tree_id.is_none(){t("独立仓库已初始化，尚无提交。","The independent repository is initialized and has no commit yet.")}else if status.staged{format!("{} · +{} / ~{} / -{}",t("当前仓库有已暂存更改","This repository has staged changes to commit"),status.added,status.modified,status.deleted)}else if status.unstaged{t("当前仓库有尚未暂存的工作更改。","This repository has working changes that are not staged.")}else{t("当前仓库没有待提交更改。","This repository has no changes to commit.")}}</p>
                {status.owner.map(|owner| {
                    let current = owner.directory_id == status.directory_id;
                    let label = path_label(&owner.path);
                    let id = StoredValue::new(owner.directory_id);
                    view! {
                        <div class="zxs-flow">
                            <p class="zxs-row">{format!("{}: {}",t("工作内容所属仓库","Repository owning this work"),label)}</p>
                            {(!current).then(move ||view! {
                                <button type="button" class="zxs-button" data-open-directory-owner=id.get_value()
                                    disabled=move ||state.with(|state|model.with(|model|repository_destination(state,model,&id.get_value()).is_none()))
                                    on:click=move |_|open_repository(state,model,&id.get_value())>{t("进入所属仓库暂存／提交","Open owning repository to stage / commit")}</button>
                            })}
                        </div>
                    }
                })}
                <Show when=move ||status.legacy_history><p class="zxs-row">{t("已有目录历史保留；初始化不会删除或重置已有数据。","Existing directory history is retained. Initialization never deletes or resets existing data.")}</p></Show>
            })}
            <Show when=move ||model.with(|model|model.status.as_ref().is_some_and(|status|!status.initialized))>
                <p class="zxs-row">{t("手动初始化后，当前目录成为独立仓库；已有工作内容不会自动提交。","Manual initialization makes this directory independent. Existing working content is not committed automatically.")}</p>
                <button type="button" class="zxs-button" data-initialize-directory-git="" disabled=move ||state.with(|state|model.with(|model|initialize_review(state,model).is_none()))
                    on:click=move |_|{let review=state.with_untracked(|state|model.with_untracked(|model|initialize_review(state,model)));submit(state,model,review);}>{t("初始化此目录仓库","Initialize this directory repository")}</button>
            </Show>
            <Show when=move ||model.with(|model|model.status.as_ref().is_some_and(|status|status.initialized))>
                <div class="zxs-file-toolbar zxs-flow">
                    {branch::control(
                        state,
                        t("创建目录分支", "Create folder branch"),
                        branch::Target::CurrentDirectory(branch_source),
                        branch_disabled,
                    )}
                </div>
                <Show when=move ||model.with(|model|model.status.as_ref().is_some_and(|status|status.staged||status.unstaged||status.needs_restage))>
                    <p class="zxs-row">{t("先暂存并提交本仓库的更改，再从当前提交创建分支。子仓库的未提交工作仍由各自处理。", "Stage and commit this repository's changes before creating a branch from its current commit. Uncommitted child work remains independent.")}</p>
                </Show>
                <Show when=move ||model.with(|model|model.status.as_ref().is_some_and(|status|status.unstaged||status.needs_restage))>
                    <p class="zxs-row" role="status">{t("工作内容尚未暂存，或独立子仓库边界有变化。暂存将固定本仓库当前工作内容，不会自动跟随子仓库的新提交；之后保存的更改保留在工作区。","Working contents are unstaged, or independent child boundaries changed. Staging freezes this repository’s current work without following child commits. Later saves remain in the working tree.")}</p>
                    <button type="button" class="zxs-button" data-stage-directory-owned="" disabled=move ||state.with(|state|model.with(|model|owned_review(state,model).is_none()))
                        on:click=move |_|{let review=state.with_untracked(|state|model.with_untracked(|model|owned_review(state,model)));submit(state,model,review);}>{t("暂存本仓库工作内容","Stage this repository’s work")}</button>
                </Show>
                <For each=move ||children.get() key=|child|(child.directory_id.clone(),child.path.clone(),child.directory_branch_id.clone(),child.head_tree_id.clone(),child.staged_tree_id.clone(),child.committed_tree_id.clone(),child.dirty,child.reference_changed,child.staged,child.needs_restage)
                    children=move |child|child_view(state,model,child)/>
                <p class="zxs-row">{t("本次只提交已暂存内容和子仓库引用；未暂存的工作更改保持可见，不会加入本次提交。","This commits only staged contents and child references. Unstaged working changes stay visible and are not included.")}</p>
                <div class="zxs-file-toolbar zxs-flow">
                    <input class="zxs-input zxs-input--compact" aria-label=t("目录提交说明","Folder commit message") placeholder=t("填写本次提交说明","Describe this commit") aria-invalid=move ||message_error.get().is_some().to_string()
                        prop:value=move ||model.with(|model|model.message.clone()) disabled=move ||!state.with(actions::write_allowed)||model.with(|model|model.reading||model.error.is_some())
                        on:input=move |event|{if state.with_untracked(|state|model.with_untracked(|model|model.ready(state).is_some())){model.update(|model|model.message=event_target_value(&event));}}
                        on:keydown=move |event:web_sys::KeyboardEvent|{if event.key()=="Enter"&&!event.is_composing()&&!event.repeat(){event.prevent_default();let review=state.with_untracked(|state|model.with_untracked(|model|commit_review(state,model)));submit(state,model,review);}}/>
                    <button type="button" class="zxs-button" data-commit-directory="true" disabled=move ||state.with(|state|model.with(|model|commit_review(state,model).is_none()))
                        on:click=move |_|{let review=state.with_untracked(|state|model.with_untracked(|model|commit_review(state,model)));submit(state,model,review);}>{t("提交当前仓库更改","Commit this repository's changes")}</button>
                </div>
                <p class="zxs-row" role="status">{move ||model.with(|model|format!("{} / {} UTF-8 bytes",model.message.len(),storage_contract::MAX_NOTE_BYTES))}</p>
                {move ||message_error.get().map(|reason|view!{<p class="zxs-row zxs-inline-error" role="status" data-directory-commit-message-error="" title=reason>{t("提交说明须包含非空白内容，最多 128 UTF-8 字节，且不能包含换行或控制字符。", "Enter a nonblank commit message of at most 128 UTF-8 bytes, without line breaks or control characters.")}</p>})}
            </Show>
            {super::directory_history::current_directory_view(state, history_target)}
            {layout::view(state, model)}
        </section>
    </Show>}
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/staging_tests.rs"]
mod tests;
