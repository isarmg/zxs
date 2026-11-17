//! In-memory, explicitly kept selections from bounded ordinary listing pages.
//! The actual directory POST still checks the one original head, every original
//! location and every revision together. There is no durable second journal.
use super::{Candidate, Review, Target, can_review, wire};
use crate::{api, ui::State};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use storage_contract::snapshot_worktree::discovery::DirectorySnapshotHead;
use storage_contract::{CommitId, EntryId, Principal, RepositoryId};
use web_interface::{i18n::t, prelude::*};

#[derive(Clone)]
pub(super) struct Kept {
    principal: Principal,
    epoch: u64,
    slot: String,
    repository: RepositoryId,
    namespace_head: CommitId,
    head: DirectorySnapshotHead,
    items: Arc<Vec<Candidate>>,
}
impl Kept {
    fn new(principal: &Principal, review: &Review, items: Vec<Candidate>) -> Self {
        Self {
            principal: principal.clone(),
            epoch: review.target.epoch,
            slot: review.target.slot.clone(),
            repository: review.target.repository,
            namespace_head: review.target.namespace_head,
            head: review.head.clone(),
            items: Arc::new(items),
        }
    }
    pub(super) fn same_owner(&self, who: &Principal, target: &Target) -> bool {
        self.principal == *who
            && self.epoch == target.epoch
            && self.slot == target.slot
            && self.repository == target.repository
    }
    fn current(&self, who: &Principal, review: &Review) -> bool {
        self.same_owner(who, &review.target)
            && self.namespace_head == review.target.namespace_head
            && self.head == review.head
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Rejection {
    SourceChanged,
    ConflictingOccurrence,
    TooMany,
    InvalidPage,
}
impl Rejection {
    fn message(self) -> String {
        match self {
            Self::SourceChanged => t(
                "原目录分支或列表基准已变；请清除原选集，重新读取后再选择。",
                "The original snapshot or namespace head changed. Clear the kept set and review again.",
            ),
            Self::ConflictingOccurrence => t(
                "选集中的文件修订、路径或稳定身份发生冲突；未提交，请移除原选项并重新读取。",
                "Selected revisions, paths or stable identities conflict. Nothing was submitted. Remove the old selection and review again.",
            ),
            Self::TooMany => t(
                "所选文件超出单次提交上限，请减少选项。",
                "The explicit set exceeds the per-commit limit. Remove some selections.",
            ),
            Self::InvalidPage => t(
                "本页原选项已改变，请重新读取工作稿。",
                "This page no longer contains the reviewed selections. Refresh its drafts.",
            ),
        }
    }
    pub(super) fn error(self) -> api::Error {
        api::error(storage_contract::ErrorCode::InvalidRequest, self.message())
    }
}

/// Keep the original order within previously kept pages and the current listing.
/// One identical original from both inputs is included once; neither a changed
/// revision nor a same-spelling different FILE silently replaces the earlier one.
pub(super) fn collect<'a>(
    kept: Option<&'a Kept>,
    who: &Principal,
    review: &'a Review,
) -> Result<Vec<&'a Candidate>, Rejection> {
    if kept.is_some_and(|k| !k.current(who, review)) {
        return Err(Rejection::SourceChanged);
    }
    let mut output = Vec::new();
    let mut by_entry = BTreeMap::<EntryId, &'a Candidate>::new();
    let mut files = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let current = review
        .candidates
        .iter()
        .filter(|item| review.selected.contains(&item.selected.target.entry));
    let mut on_page = BTreeSet::new();
    for item in current.clone() {
        if !on_page.insert(item.selected.target.entry) {
            return Err(Rejection::InvalidPage);
        }
    }
    if on_page != review.selected {
        return Err(Rejection::InvalidPage);
    }
    for item in kept.into_iter().flat_map(|k| k.items.iter()).chain(current) {
        let location = &item.selected.target;
        if location.repository != review.target.repository {
            return Err(Rejection::ConflictingOccurrence);
        }
        if let Some(previous) = by_entry.get(&location.entry) {
            if **previous == *item {
                continue;
            }
            return Err(Rejection::ConflictingOccurrence);
        }
        if !files.insert(location.file) || !paths.insert(location.path.key()) {
            return Err(Rejection::ConflictingOccurrence);
        }
        if output.len() == wire::MAX_SELECTIONS {
            return Err(Rejection::TooMany);
        }
        by_entry.insert(location.entry, item);
        output.push(item);
    }
    Ok(output)
}

pub(super) fn has_current(kept: Option<&Kept>, who: &Principal, review: &Review) -> bool {
    kept.is_none_or(|k| k.current(who, review))
        && (!review.selected.is_empty() || kept.is_some_and(|k| !k.items.is_empty()))
}

fn keep(state: RwSignal<State>) {
    state.update(|s| {
        if !can_review(s) {
            return;
        }
        let Some(who) = s.principal.as_ref() else {
            return;
        };
        let model = &s.files.directory_snapshot;
        let Some(review) = model.reviewed(s) else {
            return;
        };
        if review.selected.is_empty() || wire::ready_source(&review.head).is_none() {
            return;
        }
        let value = collect(model.kept.as_ref(), who, review)
            .map(|items| Kept::new(who, review, items.into_iter().cloned().collect()));
        let model = &mut s.files.directory_snapshot;
        match value {
            Ok(kept) => {
                model.kept = Some(kept);
                if let Some(review) = model.review.as_mut() {
                    review.selected.clear();
                }
                model.error = None;
            }
            Err(err) => model.error = Some(err.error()),
        }
    });
}

fn remove(state: RwSignal<State>, entry: Option<EntryId>, generation: u64) {
    state.update(|s| {
        if !can_review(s) || generation != s.files.directory_snapshot.read_generation {
            return;
        }
        let Some(who) = s.principal.as_ref() else {
            return;
        };
        let model = &s.files.directory_snapshot;
        let (Some(kept), Some(review)) = (model.kept.as_ref(), model.reviewed(s)) else {
            return;
        };
        if !kept.same_owner(who, &review.target) || (entry.is_some() && !kept.current(who, review))
        {
            return;
        }
        let model = &mut s.files.directory_snapshot;
        match entry {
            Some(id) => {
                if let Some(k) = model.kept.as_mut() {
                    Arc::make_mut(&mut k.items).retain(|it| it.selected.target.entry != id);
                    if k.items.is_empty() {
                        model.kept = None;
                    }
                }
            }
            None => model.kept = None,
        }
        model.error = None;
    });
}

pub(super) fn controls(state: RwSignal<State>) -> impl IntoView {
    view! {
        <div class="zxs-stack" data-zxs-kept-drafts="">
            <button class="zxs-button" data-zxs-keep-drafts=""
                disabled=move ||!state.with(|s|can_review(s)&&s.files.directory_snapshot.reviewed(s).is_some_and(|r|!r.selected.is_empty()))
                on:click=move |_|keep(state)>{t("保留本页勾选，继续选择其他文件夹","Keep this page’s choices for other folders")}</button>
            {move || state.with(|s| {
                let model=&s.files.directory_snapshot;
                let who=s.principal.as_ref()?;
                let review=model.reviewed(s)?;
                let kept=model.kept.as_ref()?;
                if !kept.same_owner(who,&review.target){return None;}
                let current=kept.current(who,review);
                let names:Vec<_>=if current {kept.items.iter().map(|c|(c.selected.target.entry,c.selected.target.path.as_str().to_owned())).collect()}else{Vec::new()};
                Some((current,names,model.read_generation))
            }).map(|(current,names,generation)| view! {
                <div class="zxs-stack">
                    <p role="status">{if current {
                        format!("{} {}",t("本次已保留的准确工作稿：","Exact drafts kept for this commit:"),names.len())
                    }else{Rejection::SourceChanged.message()}}</p>
                    {names.into_iter().map(|(id,path)|view!{
                        <div class="zxs-flow" data-zxs-kept-draft=id.to_string()>
                            <span>{path}</span>
                            <button class="zxs-button" on:click=move |_|remove(state,Some(id),generation)>{t("从选集移除","Remove from set")}</button>
                        </div>
                    }).collect_view()}
                    <button class="zxs-button" data-zxs-clear-kept-drafts="" on:click=move |_|remove(state,None,generation)>{t("清除保留选集","Clear kept set")}</button>
                </div>
            })}
        </div>
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/directory_snapshot_picks.rs"]
mod tests;
