//! Explicit multi-branch merge review. Inputs and their heads are frozen when
//! opening the panel; the chosen result may be any node of this file.
use super::{
    actions,
    history::{FileNode, NodeKind, reads::ReviewOwner},
    model::Model,
    review,
};
use crate::{api, ui::State};
use json_codec::{Value, json};
use std::collections::BTreeMap;
use web_interface::{i18n::t, prelude::*};
pub(in crate::files) mod selection;
mod view;
use selection::{ChosenNode, ReviewInventory, SelectedBranch, check_request, check_selection};
pub(crate) use view::{history, view};

#[derive(Clone)]
struct ResolvedReview {
    root: String,
    selected: BTreeMap<String, SelectedBranch>,
    chosen: Option<ChosenNode>,
    max_entries: usize,
}

#[derive(Clone)]
pub(crate) struct Draft {
    instance: u64,
    file: String,
    repository: String,
    path: String,
    root: String,
    max_entries: usize,
    selected: BTreeMap<String, SelectedBranch>,
    chosen: Option<ChosenNode>,
    survivor: String,
    selected_page: usize,
    anchor: String,
    resolved: Option<ResolvedReview>,
}
pub(crate) fn available(model: &Model) -> bool {
    !model.history_loading && model.file_history.ready()
}
impl Draft {
    pub(crate) fn instance(&self) -> u64 {
        self.instance
    }
    pub(crate) fn root(&self) -> &str {
        &self.root
    }

    fn insert_branch(&mut self, branch: SelectedBranch) -> Result<(), String> {
        let id = branch.id.clone();
        let previous = self.selected.insert(id.clone(), branch);
        if let Err(error) = check_selection(&payload(self), self.selected.len(), self.max_entries) {
            if let Some(previous) = previous {
                self.selected.insert(id, previous);
            } else {
                self.selected.remove(&id);
            }
            return Err(error.to_string());
        }
        self.resolved = None;
        Ok(())
    }

    pub(crate) fn review_inventory(&self) -> ReviewInventory {
        ReviewInventory {
            instance: self.instance,
            node_ids: self.chosen.iter().map(|node| node.id.clone()).collect(),
            branch_ids: self.selected.keys().cloned().collect(),
        }
    }

    pub(crate) fn discard_review(&mut self) {
        self.resolved = None;
    }

    pub(in crate::files) fn resolve_review(
        &mut self,
        root: String,
        nodes: &[ChosenNode],
        branches: &[SelectedBranch],
        max_entries: usize,
    ) -> Result<(), String> {
        self.resolved = None;
        let branches = branches
            .iter()
            .map(|branch| (branch.id.as_str(), branch))
            .collect::<BTreeMap<_, _>>();
        let mut selected = BTreeMap::new();
        for id in self.selected.keys() {
            let branch = branches.get(id.as_str()).ok_or_else(unavailable_branch)?;
            selected.insert(id.clone(), (**branch).clone());
        }
        let chosen = self
            .chosen
            .as_ref()
            .map(|chosen| {
                nodes
                    .iter()
                    .find(|node| node.id == chosen.id)
                    .cloned()
                    .ok_or_else(missing_node)
            })
            .transpose()?;
        let mut reviewed = self.clone();
        reviewed.root = root.clone();
        reviewed.selected = selected.clone();
        reviewed.chosen = chosen.clone();
        reviewed.max_entries = max_entries;
        reviewed.resolved = None;
        check_request(&payload(&reviewed), selected.len(), max_entries)
            .map_err(|error| error.to_string())?;
        self.resolved = Some(ResolvedReview {
            root,
            selected,
            chosen,
            max_entries,
        });
        Ok(())
    }

    pub(crate) fn accept_review(&mut self, current_root: &str) -> Result<(), String> {
        let Some(review) = self.resolved.take() else {
            return Err(review_required());
        };
        if review.root != current_root {
            return Err(review_required());
        }
        self.root = review.root;
        self.selected = review.selected;
        self.chosen = review.chosen;
        self.max_entries = review.max_entries;
        Ok(())
    }
}

pub(crate) fn start(state: RwSignal<State>) {
    let Some(mut draft) = state.with_untracked(|snapshot| {
        if !actions::write_allowed(snapshot)
            || super::model::selected_entry_read_only(&snapshot.files)
            || !available(&snapshot.files)
        {
            return None;
        }
        let entry = snapshot.files.selected.as_ref()?;
        let file = entry.file_id().to_owned();
        let (repository, path) = super::model::file_context(entry)?;
        let history = &snapshot.files.file_history;
        let root = history.root()?.to_owned();
        let max_entries = history.action_max_entries()?;
        let selected = history
            .current_branch()
            .or_else(|| {
                history
                    .selected()
                    .and_then(|id| history.branches().iter().find(|branch| branch.id() == id))
            })
            .and_then(SelectedBranch::capture)
            .into_iter()
            .map(|branch| (branch.id.clone(), branch))
            .collect();
        let draft = Draft {
            instance: snapshot.files.history_next_draft,
            file,
            repository,
            path,
            root,
            max_entries,
            selected,
            chosen: None,
            survivor: String::new(),
            selected_page: 0,
            anchor: String::new(),
            resolved: None,
        };
        Some(draft)
    }) else {
        return;
    };
    review::reset(state, review::Kind::Merge);
    state.update(|s| {
        if let Some(previous) = s.files.merge.as_ref() {
            s.files
                .history_aux
                .cancel_review(super::history::reads::ReviewOwner {
                    kind: review::Kind::Merge,
                    instance: previous.instance,
                });
        }
        s.files.history_next_draft += 1;
        draft.instance = s.files.history_next_draft;
        s.files.merge = Some(draft);
    });
}
fn unavailable_branch() -> String {
    t(
        "分支不可用或依据节点缺失，请重新审阅。",
        "A branch is unavailable or has no base node. Review the selection again.",
    )
}
fn missing_node() -> String {
    t(
        "所选历史节点不可用，请重新审阅。",
        "A selected history node is unavailable. Review the selection again.",
    )
}
fn review_required() -> String {
    t(
        "历史已改变。请读取并确认全部所选依据后再保存。",
        "The history changed. Load and confirm every selected base before saving.",
    )
}
fn payload(draft: &Draft) -> Value {
    let inputs = draft
        .selected
        .values()
        .map(|branch| json!({"branch_id":branch.id,"expected_node_id":branch.expected}))
        .collect::<Vec<_>>();
    json!({"inputs":inputs,"chosen_node_id":draft.chosen.as_ref().map(|node|node.id.as_str()).unwrap_or(""),"surviving_branch_id":draft.survivor,"repository_id":draft.repository,"path":draft.path,"confirmed":true})
}
fn request(draft: &Draft) -> Result<Value, String> {
    if draft.selected.len() < 2 {
        return Err(t(
            "至少选择两个可写文件分支。",
            "Select at least two writable file branches.",
        ));
    }
    if draft.chosen.is_none() {
        return Err(t(
            "请选择这个文件的一个历史节点作为结果。",
            "Choose a history node of this file as the result.",
        ));
    }
    if !draft.selected.contains_key(&draft.survivor) {
        return Err(t(
            "请明确选择合并后保留的分支。",
            "Explicitly choose the branch to keep after merging.",
        ));
    }
    let body = payload(draft);
    check_request(&body, draft.selected.len(), draft.max_entries)
        .map_err(|error| error.to_string())?;
    Ok(body)
}

fn select_branch(state: RwSignal<State>, instance: u64, id: String, checked: bool) {
    let result = state.try_update(|s| {
        if actions::pending(s) {
            return Ok(());
        }
        let Some(draft) = s.files.merge.as_mut() else {
            return Ok(());
        };
        if draft.instance != instance {
            return Ok(());
        }
        let owner = ReviewOwner {
            kind: review::Kind::Merge,
            instance,
        };
        if !checked {
            draft.selected.remove(&id);
            if draft.survivor == id {
                draft.survivor.clear();
            }
            draft.resolved = None;
            s.files.history_aux.cancel_review(owner);
            return Ok(());
        }
        if s.files.file_history.root() != Some(draft.root()) || !s.files.file_history.ready() {
            return Err(review_required());
        }
        let branch = s
            .files
            .file_history
            .branches()
            .iter()
            .find(|branch| branch.id() == id)
            .and_then(SelectedBranch::capture)
            .ok_or_else(unavailable_branch)?;
        draft.insert_branch(branch)?;
        s.files.history_aux.cancel_review(owner);
        Ok(())
    });
    if let Some(Err(error)) = result {
        review::error(state, review::Kind::Merge, error);
    }
}

fn choose_node(state: RwSignal<State>, instance: u64, id: String) {
    let result = state.try_update(|s| {
        if actions::pending(s) {
            return Ok(());
        }
        let Some(draft) = s.files.merge.as_mut() else {
            return Ok(());
        };
        if draft.instance != instance {
            return Ok(());
        }
        if s.files.file_history.root() != Some(draft.root()) || !s.files.file_history.ready() {
            return Err(review_required());
        }
        let chosen = s
            .files
            .file_history
            .node(&id)
            .and_then(ChosenNode::capture)
            .or_else(|| {
                draft
                    .chosen
                    .as_ref()
                    .filter(|chosen| chosen.id == id)
                    .cloned()
            })
            .ok_or_else(missing_node)?;
        let mut candidate = draft.clone();
        candidate.chosen = Some(chosen);
        check_selection(
            &payload(&candidate),
            candidate.selected.len(),
            candidate.max_entries,
        )
        .map_err(|error| error.to_string())?;
        candidate.resolved = None;
        *draft = candidate;
        s.files.history_aux.cancel_review(ReviewOwner {
            kind: review::Kind::Merge,
            instance,
        });
        Ok(())
    });
    if let Some(Err(error)) = result {
        review::error(state, review::Kind::Merge, error);
    }
}

fn choose_survivor(state: RwSignal<State>, instance: u64, id: String) {
    let result = state.try_update(|s| {
        if actions::pending(s) {
            return Ok(());
        }
        let Some(draft) = s.files.merge.as_mut() else {
            return Ok(());
        };
        if draft.instance != instance {
            return Ok(());
        }
        if !draft.selected.contains_key(&id) {
            return Err(unavailable_branch());
        }
        let mut candidate = draft.clone();
        candidate.survivor = id;
        check_selection(
            &payload(&candidate),
            candidate.selected.len(),
            candidate.max_entries,
        )
        .map_err(|error| error.to_string())?;
        candidate.resolved = None;
        *draft = candidate;
        s.files.history_aux.cancel_review(ReviewOwner {
            kind: review::Kind::Merge,
            instance,
        });
        Ok(())
    });
    if let Some(Err(error)) = result {
        review::error(state, review::Kind::Merge, error);
    }
}

fn accept_review(state: RwSignal<State>, instance: u64) {
    let result = state.try_update(|s| {
        if actions::pending(s) {
            return Ok(());
        }
        if !s
            .files
            .merge
            .as_ref()
            .is_some_and(|draft| draft.instance == instance)
        {
            return Ok(());
        }
        let root = s
            .files
            .file_history
            .root()
            .ok_or_else(review_required)?
            .to_owned();
        if !s.files.file_history.ready() {
            return Err(review_required());
        }
        let Some(draft) = s.files.merge.as_mut() else {
            return Ok(());
        };
        draft.accept_review(&root)
    });
    if let Some(Err(error)) = result {
        review::error(state, review::Kind::Merge, error);
    }
}

fn submit(state: RwSignal<State>, instance: u64) {
    if !state.with_untracked(|s| {
        s.files
            .merge
            .as_ref()
            .is_some_and(|draft| draft.instance == instance)
    }) {
        return;
    }
    if !state.with_untracked(|s| {
        actions::history_write_allowed(s)
            && s.files
                .merge
                .as_ref()
                .is_some_and(|draft| s.files.file_history.root() == Some(draft.root()))
    }) {
        review::error(state, review::Kind::Merge, review_required());
        return;
    }
    let Some(draft) = state.with_untracked(|s| s.files.merge.clone()) else {
        return;
    };
    match request(&draft) {
        Ok(body) => review::submit(
            state,
            review::Kind::Merge,
            "POST",
            format!(
                "/api/file-manager/files/{}/merge",
                api::segment(&draft.file)
            ),
            body,
        ),
        Err(error) => review::error(state, review::Kind::Merge, error),
    }
}
pub(super) fn node_label(node: &FileNode, _index: usize) -> String {
    let note = node.note();
    let descriptor = if note.is_empty() {
        match node.kind() {
            NodeKind::Rename => t("路径变化", "Path changed"),
            NodeKind::Tags => t("标签变化", "Tags changed"),
            NodeKind::Merge => t("合并节点", "Merge node"),
            NodeKind::Content => t("文件内容", "File content"),
        }
    } else {
        note.to_owned()
    };
    format!("{} · {descriptor}", node.sequence())
}
#[cfg(test)]
#[path = "../../../../../test/web/unit/files/merge/tests.rs"]
mod tests;
