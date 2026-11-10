use crate::{
    files::{actions, detail, model::directory_branch_name},
    ui::State,
};
use web_interface::{i18n::t, prelude::*};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Kind {
    Working(String),
    Committed,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Preview {
    epoch: u64,
    slot: String,
    directory: String,
    root: String,
    branch: String,
    tree: String,
    kind: Kind,
}
impl Preview {
    fn capture(state: &State) -> Option<Self> {
        if !state.session_active() {
            return None;
        }
        let preview = state.files.directory_preview.as_ref()?;
        Some(Self {
            epoch: state.session_epoch,
            slot: state.files.slot.clone(),
            directory: state.files.directory.clone(),
            root: preview.path.clone(),
            branch: preview.branch_id().to_owned(),
            tree: preview.tree_id().to_owned(),
            kind: preview.branch().map_or(Kind::Committed, |branch| {
                Kind::Working(directory_branch_name(branch))
            }),
        })
    }

    fn current(&self, state: &State) -> bool {
        Self::capture(state).as_ref() == Some(self) && !actions::pending(state)
    }
}

pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    let preview = Memo::new(move |_| state.with(Preview::capture));
    view! {
        <For each=move ||preview.get() key=|preview|preview.clone()
            children=move |preview|controls(state,preview)/>
    }
}

fn controls(state: RwSignal<State>, preview: Preview) -> impl IntoView {
    let working = matches!(&preview.kind, Kind::Working(_));
    let message = match &preview.kind {
        Kind::Working(name) => format!(
            "{}: {name} · {}",
            t("只读审阅分支工作内容", "Read-only branch work preview"),
            t(
                "显示选取时已保存的工作树，包括未提交更改，不是实时视图。尚未修改默认读取；请在该分支目录详情中明确设为默认。",
                "This is the saved working tree selected for review, including uncommitted changes, not a live view. Default reads are unchanged; set the branch as default explicitly in its folder Details.",
            ),
        ),
        Kind::Committed => t(
            "正在只读浏览明确选定的历史快照，不是当前工作内容。此快照不能直接设为默认；返回默认视图后继续当前工作。",
            "You are viewing an explicitly selected immutable snapshot, not current working content. This snapshot cannot be set as default directly; return to the default view to continue current work.",
        ),
    };
    let preview = StoredValue::new(preview);
    let disabled = Signal::derive(move || {
        state.with(|state| !preview.with_value(|preview| preview.current(state)))
    });
    view! {
        <section class="zxs-stack" data-directory-preview=if working{"working"}else{"committed"}>
            <p class="zxs-row" role="status" data-directory-working-preview=working.then_some("")
                data-directory-snapshot-readonly=(!working).then_some("")>{message}</p>
            <div class="zxs-file-toolbar zxs-flow">
                <button type="button" class="zxs-button" data-return-directory-default="" disabled=move ||disabled.get()
                    on:click=move |_|{
                        if state.with_untracked(|state|preview.with_value(|preview|preview.current(state))) {
                            actions::return_to_default(state);
                        }
                    }>{t("返回当前默认工作视图", "Return to current default work")}</button>
                <button type="button" class="zxs-button" data-open-preview-origin="" disabled=move ||disabled.get()
                    on:click=move |_|{
                        if state.with_untracked(|state|preview.with_value(|preview|preview.current(state))) {
                            actions::open_preview_origin(state);
                        }
                    }>{t("返回原目录详情", "Return to source folder Details")}</button>
                {working.then(move ||view!{
                    <button type="button" class="zxs-button" data-review-directory-default="" disabled=move ||disabled.get()
                        on:click=move |_|{
                            let root=state.with_untracked(|state|preview.with_value(|preview|preview.current(state).then(||preview.root.clone())));
                            if let Some(root)=root {
                                if state.with_untracked(|state|state.files.directory==root) {
                                    detail::open_current_directory(state);
                                } else {
                                    actions::go_directory_details(state,root);
                                }
                            }
                        }>{t("打开分支目录详情", "Open branch folder Details")}</button>
                })}
            </div>
        </section>
    }
}
