//! Single-line metadata edits share the branch editor's commit/cancel behavior.
use super::{actions, history::NodeKind, model::NoteTarget};
use crate::{icons::FileIcon, ui::State};
use wasm_bindgen::JsCast;
use web_interface::{html, i18n::t, prelude::*};

// Branch labels and notes retain their metadata budget; namespace names use name-policy.
pub(super) const MAX_TEXT_BYTES: usize = storage_contract::MAX_NOTE_BYTES;

fn needs_first_folder(model: &super::model::Model) -> bool {
    model.loaded
        && !model.loading
        && model.writable
        && !model.slot.is_empty()
        && model.directory.is_empty()
        && model.repository_id.is_empty()
        && model.query.is_empty()
        && model.total_entries == 0
}

pub(super) fn first_folder_hint(state: RwSignal<State>) -> impl IntoView {
    view! {
        <Show when=move ||state.with(|state|needs_first_folder(&state.files))>
            <p class="zxs-row" role="status" data-first-folder="">{t(
                "先用“新建文件夹”创建第一个目录。创建确认后，可在当前目录详情或新文件夹详情中手动初始化独立仓库；建目录不会自动初始化或提交。",
                "Use New folder to create the first folder. Once creation is confirmed, initialize an independent repository in the current folder's Details or the new folder's Details. Creating a folder does not initialize or commit a repository.",
            )}</p>
        </Show>
    }
}

#[derive(Clone)]
pub(super) enum Target {
    CreateDirectory,
    RenameFileBranch {
        file: String,
        branch_id: String,
        name: String,
    },
    Note(NoteTarget),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InputKind {
    NamespaceName,
    BranchName,
    Note,
}

impl Target {
    fn scope(&self) -> EditScope {
        match self {
            Self::CreateDirectory => EditScope::Location,
            Self::RenameFileBranch { .. } | Self::Note(_) => EditScope::History,
        }
    }

    fn input_kind(&self) -> InputKind {
        match self {
            Self::CreateDirectory => InputKind::NamespaceName,
            Self::RenameFileBranch { .. } => InputKind::BranchName,
            Self::Note(_) => InputKind::Note,
        }
    }

    fn edit_value(&self, value: &str, directory: &str) -> Edit {
        let edit = edit_value(value, self.input_kind());
        if matches!(self, Self::CreateDirectory)
            && let Edit::Save(name) = &edit
            && let Err(source) =
                name_policy::validate_path(&super::model::join_path(directory, name))
        {
            return Edit::NameError(super::naming::Error::new(name, source));
        }
        edit
    }

    fn initial(&self, state: &State) -> Option<String> {
        match self {
            Self::CreateDirectory => Some(String::new()),
            Self::RenameFileBranch { name, .. } => Some(name.clone()),
            Self::Note(NoteTarget::File) => state.files.file_history.file_note().map(str::to_owned),
            Self::Note(NoteTarget::Node(id)) => state
                .files
                .file_history
                .node(id)
                .map(|node| node.note().to_owned()),
            Self::Note(NoteTarget::Version(id)) => {
                state.files.file_history.body_note(id).map(str::to_owned)
            }
        }
    }

    fn display(&self, history: &super::history_index::HistoryIndex) -> String {
        let node = match self {
            Self::Note(NoteTarget::Node(id)) => history.node(id).map(|(_, node)| node),
            Self::Note(NoteTarget::Version(id)) => history.version(id),
            _ => unreachable!("invariant: version note displays only node or version metadata"),
        };
        let value = node
            .and_then(|node| match self {
                Self::Note(NoteTarget::Node(_)) => Some(node.note()),
                Self::Note(NoteTarget::Version(_)) => node.body_note(),
                _ => unreachable!("invariant: version note displays only node or version metadata"),
            })
            .unwrap_or_default();
        if !value.is_empty() {
            return value.to_owned();
        }
        if matches!(self, Self::Note(NoteTarget::Node(_)))
            && let Some(node) = node
        {
            match node.kind() {
                NodeKind::Merge => return t("合并分支", "Merged branches"),
                NodeKind::Rename => {
                    let (Some(previous), Some(path)) = (node.previous_path(), node.path()) else {
                        return t("路径变化", "Path changed");
                    };
                    let action = if previous.rsplit_once('/').map(|(parent, _)| parent)
                        == path.rsplit_once('/').map(|(parent, _)| parent)
                    {
                        t("改名", "Renamed")
                    } else {
                        t("路径变化", "Path changed")
                    };
                    return format!("{action} {previous} → {path}");
                }
                NodeKind::Content => {}
            }
        }
        t("无备注", "No note")
    }

    fn submit(self, state: RwSignal<State>, value: String) -> Option<String> {
        match self {
            Self::CreateDirectory => actions::create_directory(state, &value),
            Self::RenameFileBranch {
                file, branch_id, ..
            } => actions::rename_file_branch(state, file, branch_id, value),
            Self::Note(target) => actions::update_note(state, target, value),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Edit {
    Cancel,
    Invalid,
    NameError(super::naming::Error),
    Save(String),
}

pub(super) fn valid_note(value: &str) -> bool {
    storage_contract::validate_note(value).is_ok()
}

fn edit_value(value: &str, kind: InputKind) -> Edit {
    if kind == InputKind::Note {
        if valid_note(value) {
            Edit::Save(value.to_owned())
        } else {
            Edit::Invalid
        }
    } else {
        if value.trim().is_empty() {
            Edit::Cancel
        } else if kind == InputKind::BranchName && value.len() > MAX_TEXT_BYTES {
            Edit::Invalid
        } else {
            match super::naming::validate(value) {
                Ok(()) => Edit::Save(value.to_owned()),
                Err(error) => Edit::NameError(error),
            }
        }
    }
}

/// Shared lifetime and receipt handling for metadata and branch-name editors.
/// A request owns its draft until that exact operation is terminal. Disabling
/// the input can fire blur, which must never discard an in-flight draft.
#[derive(Clone, Copy)]
pub(super) struct Draft {
    state: RwSignal<State>,
    context: RwSignal<Option<Context>>,
    pub(super) editing: RwSignal<bool>,
    pub(super) value: RwSignal<String>,
    pub(super) error: RwSignal<bool>,
    pub(super) name_error: RwSignal<Option<std::sync::Arc<super::naming::Error>>>,
    operation: RwSignal<Option<String>>,
    history_root: RwSignal<Option<String>>,
    scope: EditScope,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum EditScope {
    Location,
    History,
}

#[derive(Clone, PartialEq)]
struct Context {
    epoch: u64,
    active: bool,
    page: crate::ui::Page,
    slot: String,
    directory: String,
    repository: String,
    directory_branch: String,
    selected: Option<SelectedContext>,
}

#[derive(Clone, PartialEq)]
struct SelectedContext {
    entry: String,
    file: String,
    branch: Option<String>,
    version: Option<String>,
    node: Option<String>,
    repository: Option<String>,
    path: Option<String>,
    backup: bool,
}
impl Context {
    fn capture(state: &State) -> Self {
        Self {
            epoch: state.session_epoch,
            active: state.session_active(),
            page: state.page,
            slot: state.files.slot.clone(),
            directory: state.files.directory.clone(),
            repository: state.files.repository_id.clone(),
            directory_branch: state
                .files
                .directory_branches
                .iter()
                .find(|branch| branch.current())
                .map(|branch| branch.id().to_owned())
                .unwrap_or_default(),
            selected: state.files.selected.as_ref().map(|entry| SelectedContext {
                entry: super::model::entry_key(entry),
                file: entry.file_id().to_owned(),
                branch: entry.selected_branch_id().map(str::to_owned),
                version: entry.version_id().map(str::to_owned),
                node: entry.node_id().map(str::to_owned),
                repository: entry.repository_id().map(str::to_owned),
                path: entry.repository_path().map(str::to_owned),
                backup: entry.backup(),
            }),
        }
    }
}
#[derive(Clone, Copy)]
struct SharedContext(Memo<Context>);

pub(super) fn provide_context(state: RwSignal<State>) {
    web_interface::prelude::provide_context(SharedContext(Memo::new(move |_| {
        state.with(Context::capture)
    })));
}

pub(super) fn history_navigation_allowed(state: RwSignal<State>) -> bool {
    if state.with_untracked(|s| s.files.history_editors == 0) {
        return true;
    }
    state.update(|s| {
        s.set_file_notice(t(
            "请先保存或取消当前编辑，再切换历史页面。",
            "Save or cancel the current edit before changing history pages.",
        ));
    });
    false
}

impl Draft {
    fn unobserved_in(state: RwSignal<State>, scope: EditScope) -> Self {
        let draft = Self {
            state,
            context: RwSignal::new(None),
            editing: RwSignal::new(false),
            value: RwSignal::new(String::new()),
            error: RwSignal::new(false),
            name_error: RwSignal::new(None),
            operation: RwSignal::new(None),
            history_root: RwSignal::new(None),
            scope,
        };
        on_cleanup(move || draft.release_registration());
        draft
    }

    pub(super) fn new(state: RwSignal<State>, scope: EditScope) -> Self {
        let draft = Self::unobserved_in(state, scope);
        let context = use_context::<SharedContext>()
            .expect("invariant: file page provides its editor context")
            .0;
        Effect::new(move |_| {
            context.with(|_| ());
            draft.cancel();
        });
        Effect::new(move |_| {
            if let Some(id) = draft.operation.get() {
                let status = state.with(|s| {
                    s.files
                        .operations
                        .iter()
                        .find(|op| op.id() == id)
                        .map(super::model::FileOperation::status)
                });
                if let Some(status) = status {
                    draft.settle(&id, status);
                }
            }
        });
        draft
    }

    fn cancel(self) {
        self.release_registration();
        self.context.set(None);
        self.editing.set(false);
        self.value.set(String::new());
        self.error.set(false);
        self.name_error.set(None);
        self.operation.set(None);
        self.history_root.set(None);
    }

    fn release_registration(self) {
        if !self.editing.get_untracked() {
            return;
        }
        let epoch = self.context.with_untracked(|context| {
            context
                .as_ref()
                .expect("invariant: an active editor captured its context")
                .epoch
        });
        // The state may already be disposed when the document owner closes.
        let _ = self.state.try_update(|state| {
            if state.session_epoch == epoch {
                state.files.history_editors -= 1;
            }
        });
    }

    pub(super) fn blur(self, event: web_sys::FocusEvent) {
        if self.name_error.with_untracked(|error| {
            error
                .as_ref()
                .is_none_or(|error| error.candidate().is_none())
        }) {
            self.cancel_idle();
            return;
        }
        let within_editor = event
            .related_target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
            .zip(
                event
                    .target()
                    .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                    .and_then(|input| {
                        input
                            .closest(".zxs-inline-edit, .zxs-inline-branch, .zxs-version-note-row")
                            .expect("invariant: editor selector is valid")
                    }),
            )
            .is_some_and(|(next, editor)| editor.contains(Some(&next)));
        if !within_editor {
            self.cancel_idle();
        }
    }

    pub(super) fn accept_name(self, expected: &std::sync::Arc<super::naming::Error>) -> bool {
        let candidate = self.name_error.with_untracked(|error| {
            let error = error.as_ref()?;
            (std::sync::Arc::ptr_eq(error, expected) && self.value.get_untracked() == error.name)
                .then(|| error.candidate().map(str::to_owned))
                .flatten()
        });
        let Some(candidate) = candidate else {
            return false;
        };
        self.value.set(candidate);
        self.name_error.set(None);
        true
    }

    pub(super) fn cancel_idle(self) {
        if self.operation.get_untracked().is_none() {
            self.cancel();
        }
    }

    pub(super) fn start(self, value: String) {
        if self.editing.get_untracked() || self.operation.get_untracked().is_some() {
            return;
        }
        let context = self.state.with_untracked(Context::capture);
        if !context.active {
            return;
        }
        if self.scope == EditScope::History
            && !self.state.with_untracked(actions::history_write_allowed)
        {
            return;
        }
        self.state.update(|state| state.files.history_editors += 1);
        self.context.set(Some(context));
        self.history_root.set(
            self.state
                .with_untracked(|state| state.files.file_history.root().map(str::to_owned)),
        );
        self.value.set(value);
        self.error.set(false);
        self.name_error.set(None);
        self.editing.set(true);
    }

    pub(super) fn pending(self) -> bool {
        self.operation.with(|id| id.is_some())
    }

    pub(super) fn submit(self, submit: impl FnOnce() -> Option<String>) {
        if self.operation.get_untracked().is_some() {
            return;
        }
        let current = self.state.with_untracked(Context::capture);
        if !current.active || self.context.get_untracked().as_ref() != Some(&current) {
            self.cancel();
            return;
        }
        if self.scope == EditScope::History
            && !self.state.with_untracked(|state| {
                actions::history_write_allowed(state)
                    && self
                        .history_root
                        .with_untracked(|root| root.as_deref() == state.files.file_history.root())
            })
        {
            self.state.update(|state| {
                state.set_file_notice(t(
                    "历史依据已改变；请取消当前编辑，刷新后重新核对。",
                    "The history basis changed. Cancel this edit, refresh, and review it again.",
                ));
            });
            return;
        }
        // Establish the blur guard before admission changes disabled state.
        self.operation.set(Some(String::new()));
        self.error.set(false);
        self.name_error.set(None);
        self.operation.set(submit());
    }

    fn settle(self, id: &str, status: super::model::SaveStatus) {
        if self.operation.get_untracked().as_deref() != Some(id) {
            return;
        }
        match status {
            super::model::SaveStatus::Published | super::model::SaveStatus::Degraded => {
                self.cancel()
            }
            super::model::SaveStatus::Rejected => self.operation.set(None),
            super::model::SaveStatus::Unknown => {}
        }
    }
}

/// Persisted metadata is only read when editing starts, never on a failed save.
#[derive(Clone, Copy)]
struct Editor {
    state: RwSignal<State>,
    draft: Draft,
    target: StoredValue<Target>,
    label: StoredValue<String>,
    disabled: Signal<bool>,
    kind: InputKind,
}

impl Editor {
    fn new(state: RwSignal<State>, label: String, target: Target, disabled: Signal<bool>) -> Self {
        Self {
            state,
            draft: Draft::new(state, target.scope()),
            kind: target.input_kind(),
            target: StoredValue::new(target),
            label: StoredValue::new(label),
            disabled,
        }
    }

    fn start(self) {
        if self.disabled.get_untracked() {
            return;
        }
        if let Some(value) = self
            .state
            .with_untracked(|s| self.target.get_value().initial(s))
        {
            self.draft.start(value);
        }
    }

    fn commit(self) {
        if self.disabled.get_untracked() || self.draft.operation.get_untracked().is_some() {
            return;
        }
        let edit = self.state.with_untracked(|state| {
            self.target
                .get_value()
                .edit_value(&self.draft.value.get_untracked(), &state.files.directory)
        });
        match edit {
            Edit::Invalid => self.draft.error.set(true),
            Edit::NameError(error) => self.draft.name_error.set(Some(std::sync::Arc::new(error))),
            Edit::Cancel => self.draft.cancel_idle(),
            Edit::Save(value) => self
                .draft
                .submit(|| self.target.get_value().submit(self.state, value)),
        }
    }
}

fn editor_input(editor: Editor) -> impl IntoView {
    let input = NodeRef::<html::Input>::new();
    input.on_load(|input| {
        if let Err(error) = input.focus() {
            web_sys::console::warn_2(&"Could not move keyboard focus".into(), &error);
        }
        input.select();
    });
    view! {
        <input class="zxs-inline-input zxs-input zxs-input--compact" node_ref=input type="text"
            aria-label=editor.label.get_value() aria-invalid=move ||(editor.draft.error.get() || editor.draft.name_error.get().is_some()).to_string()
            title=if editor.kind == InputKind::NamespaceName {
                t("名称及完整路径须符合共享命名规则；Enter 保存，Esc 或点击其他位置取消。", "Names and complete paths must meet the shared naming rules. Enter saves; Escape or clicking elsewhere cancels.")
            } else {
                t("最多 128 个 UTF-8 字节；Enter 保存，Esc 或点击其他位置取消。", "Up to 128 UTF-8 bytes. Enter saves; Escape or clicking elsewhere cancels.")
            }
            data-max-bytes=(editor.kind != InputKind::NamespaceName).then_some(MAX_TEXT_BYTES) autocomplete="off" autocapitalize="none" spellcheck="false"
            prop:value=move ||editor.draft.value.get() disabled=move ||editor.disabled.get()||editor.draft.pending()
            on:input=move |event|{editor.draft.value.set(event_target_value(&event));editor.draft.error.set(false);editor.draft.name_error.set(None);}
            on:blur=move |event|editor.draft.blur(event)
            on:keydown=move |event:web_sys::KeyboardEvent| {
                if event.is_composing(){return;}
                match event.key().as_str() {
                    "Escape"=>{event.prevent_default();event.stop_propagation();editor.draft.cancel_idle();}
                    "Enter"=>{event.prevent_default();event.stop_propagation();editor.commit();}
                    _=>{}
                }
            }/>
        <span class="zxs-inline-hint zxs-row" aria-live="polite">{move ||{
            let bytes=editor.draft.value.with(|value|value.len());
            if editor.kind == InputKind::NamespaceName {
                format!("{bytes} {}",t("字节","bytes"))
            } else {
                format!("{bytes}/{MAX_TEXT_BYTES} {}",t("字节","bytes"))
            }
        }}</span>
        {name_feedback(editor.draft, move |expected| { if !editor.disabled.get_untracked() && !editor.draft.pending() && editor.draft.accept_name(expected) { editor.commit(); } })}
        <Show when=move ||editor.draft.error.get()>
            <span class="zxs-inline-error zxs-row" role="status">{match editor.kind {
                InputKind::Note => t("备注最多 128 个 UTF-8 字节，不能包含换行或控制字符。", "Notes must be at most 128 UTF-8 bytes without line breaks or control characters."),
                InputKind::BranchName => t("分支名称无效，最多 128 个 UTF-8 字节。", "Invalid branch name. Use at most 128 UTF-8 bytes."),
                InputKind::NamespaceName => t("名称或完整路径不符合共享命名规则。", "The name or complete path does not meet the shared naming rules."),
            }}</span>
        </Show>
    }
}

pub(super) fn name_feedback(
    draft: Draft,
    confirm: impl Fn(&std::sync::Arc<super::naming::Error>) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let cancel_on_escape = move |event: web_sys::KeyboardEvent| {
        if event.key() == "Escape" {
            event.prevent_default();
            event.stop_propagation();
            draft.cancel_idle();
        }
    };
    view! {
        <span class="zxs-inline-error zxs-row" role="status">
            {move || draft.name_error.with(|error| error.as_ref().map(|error| error.message()))}
        </span>
        {move ||draft.name_error.get().filter(|error|error.candidate().is_some()).map(|expected|view! {
            <button class="zxs-button" type="button"
                on:blur=move |event|draft.blur(event) on:keydown=cancel_on_escape
                on:click=move |_|confirm(&expected)>
                {t("确认使用 NFC 名称", "Confirm NFC name")}
            </button>
            <button class="zxs-button" type="button"
                on:blur=move |event|draft.blur(event) on:keydown=cancel_on_escape
                on:click=move |_|draft.cancel_idle()>
                {t("取消", "Cancel")}
            </button>
        })}
    }
}

pub(super) fn control(
    state: RwSignal<State>,
    label: String,
    icon: Option<&'static str>,
    target: Target,
    disabled: Signal<bool>,
) -> impl IntoView {
    let editor = Editor::new(state, label, target, disabled);
    view! {
        <span class="zxs-inline-edit zxs-flow zxs-flow--inline" class:zxs-inline-note=(editor.kind == InputKind::Note)>
            {move ||if editor.draft.editing.get() {
                editor_input(editor).into_any()
            } else {
                view! {
                    <button class="zxs-button" type="button" aria-label=editor.label.get_value()
                        disabled=move ||editor.disabled.get() on:click=move |_|editor.start()>
                        {icon.map(|name|view!{<FileIcon name=name/>})}
                        {move ||if editor.kind == InputKind::Note {
                            let note=state.with(|s|editor.target.get_value().initial(s)).unwrap_or_default();
                            if note.is_empty(){editor.label.get_value()}else{note}
                        }else{editor.label.get_value()}}
                    </button>
                }.into_any()
            }}
        </span>
    }
}

/// Expanded node details keep the full note and an explicit edit action.
/// Only that note text is replaced by an underline input, not the card title.
pub(super) fn version_note(
    state: RwSignal<State>,
    target: Target,
    disabled: Signal<bool>,
) -> (impl IntoView, impl IntoView) {
    let editor = Editor::new(state, t("编辑备注", "Edit note"), target, disabled);
    let history = super::history_index::shared();
    let caption = Memo::new(move |_| {
        history.with(|history| editor.target.with_value(|target| target.display(history)))
    });
    let note = view! {
        <div class="zxs-version-note-row zxs-row" class:zxs-inline-note=move ||editor.draft.editing.get()>
            {move ||if editor.draft.editing.get() {
                editor_input(editor).into_any()
            } else {
                view!{<span class="zxs-version-note">{move ||{
                    caption.get()
                }}</span>}.into_any()
            }}
        </div>
    };
    let can_modify = Signal::derive(move || {
        state.with(|s| s.files.writable && !super::model::selected_entry_read_only(&s.files))
    });
    let action = view! {
        <Show when=move ||can_modify.get()><button class="zxs-button zxs-version-note-action" type="button"
            disabled=move ||editor.disabled.get()||editor.draft.editing.get()
            on:click=move |_|editor.start()>{t("编辑备注","Edit note")}</button></Show>
    };
    (note, action)
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/inline/tests.rs"]
mod tests;
