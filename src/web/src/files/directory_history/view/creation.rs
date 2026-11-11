//! A historical branch combines one loaded commit with the current clean review.
use super::{Model, Scope, Target};
use crate::files::{branch, staging::BranchReview};
use crate::ui::State;
use storage_contract::{DirectoryTreeId, directory_git::DirectoryBranchCreateSource};
use web_interface::{i18n::t, prelude::*};

fn available(
    state: &State,
    target: &Target,
    model: &Model,
    source: &DirectoryBranchCreateSource,
    tree: DirectoryTreeId,
) -> bool {
    target.can_manage_branches(state)
        && !model.loading
        && !model.stale
        && model.error.is_none()
        && source.repository_id == target.repository
        && source.directory_id == target.directory
        && model.nodes.as_ref().is_some_and(|page| {
            page.nodes.iter().any(|node| node.tree_id == tree.to_string())
        })
}

pub(super) fn control(
    state: RwSignal<State>,
    model: RwSignal<Model>,
    target: StoredValue<Target>,
    source: Signal<Option<BranchReview>>,
    tree: DirectoryTreeId,
) -> impl IntoView {
    let working = target.with_value(|target| {
        matches!(&target.scope, Scope::CurrentDirectory { initialized: true, .. })
    });
    let current = Signal::derive(move || {
        let review = source.get()?;
        let allowed = state.with(|state| target.with_value(|target| {
            model.with(|model| available(state, target, model, review.source(), tree))
        }));
        allowed.then_some(review)
    });
    let disabled = Signal::derive(move || current.get().is_none());
    working.then(move || view! {
        <div class="zxs-flow" data-historical-directory-branch=tree.to_string()>
            {branch::control(
                state,
                t("从此提交建立分支", "Create branch from this commit"),
                branch::Target::HistoricalDirectory { current, tree },
                disabled,
            )}
            <span class="zxs-row">
                {t(
                    "引用此历史提交；登记后保持当前工作分支。",
                    "Reference this historical commit and keep the current working branch.",
                )}
            </span>
        </div>
    })
}

#[cfg(test)]
#[path = "../../../../../../test/web/unit/files/directory_history/creation_tests.rs"]
mod tests;
