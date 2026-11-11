//! Listing controls retain the page they displayed across delayed DOM events.
use crate::api;
use crate::files::deletion::{load, select, wire};
use crate::ui::State;
use std::sync::Arc;
use web_interface::{i18n::t, prelude::*};

#[derive(Clone)]
pub(super) struct Target {
    epoch: u64,
    page: Arc<wire::Page>,
}
impl Target {
    fn current(&self, state: &State) -> bool {
        let model = &state.files.deletion;
        state.session_active()
            && state.session_epoch == self.epoch
            && model.open
            && !model.loading
            && model
                .page
                .as_ref()
                .is_some_and(|page| Arc::ptr_eq(page, &self.page))
    }
}

pub(super) enum Read {
    First,
    Next(Target),
}
impl Read {
    pub(super) fn endpoint(&self, state: &State) -> Option<String> {
        match self {
            Self::First => (state.session_active() && !state.files.deletion.loading)
                .then(|| "/api/file-deletions".into()),
            Self::Next(target) => {
                if !target.current(state) {
                    return None;
                }
                let after = target.page.next_after.as_ref()?;
                Some(format!(
                    "/api/file-deletions?after={}&snapshot={}",
                    api::segment(after.as_str()),
                    api::segment(&target.page.snapshot)
                ))
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct Selection {
    target: Target,
    index: usize,
}
impl Selection {
    pub(super) fn begin(&self, state: &mut State) -> Option<(u64, u64, wire::Summary)> {
        if !self.target.current(state) {
            return None;
        }
        let summary = self.target.page.entries[self.index].clone();
        let model = &mut state.files.deletion;
        model.selected = Some(summary.clone());
        Some((self.target.epoch, model.begin(), summary))
    }
}

pub(super) fn view(state: RwSignal<State>, page: Arc<wire::Page>) -> impl IntoView {
    let target = StoredValue::new(Target {
        epoch: state.with_untracked(|s| s.session_epoch),
        page: page.clone(),
    });
    let empty = page.entries.is_empty();
    let next = page.next_after.is_some();
    let entries = page
        .entries
        .iter()
        .enumerate()
        .map(|(index, summary)| {
            let selection = StoredValue::new(Selection {
                target: target.get_value(),
                index,
            });
            let identity = format!("{} · {}", summary.repository_id, summary.file_id);
            view! {
                <li>
                    <button
                        class="zxs-button"
                        data-deleted-file=summary.file_id.to_string()
                        disabled=move ||!state.with(|s|target.with_value(|target|target.current(s)))
                        on:click=move |_|select(state,selection.get_value())
                    >
                        {summary.path.as_str().to_owned()}
                    </button>
                    <code>{identity}</code>
                </li>
            }
        })
        .collect_view();
    view! {
        <Show when=move ||empty>
            <p>{t("这一页没有已删除文件。", "No deleted files on this page.")}</p>
        </Show>
        <ul>{entries}</ul>
        <Show when=move ||next>
            <button
                class="zxs-button"
                disabled=move ||!state.with(|s|target.with_value(|target|target.current(s)))
                on:click=move |_|load(state,Read::Next(target.get_value()))
            >
                {t("下一页", "Next page")}
            </button>
        </Show>
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/deletion/listing_tests.rs"]
mod tests;
