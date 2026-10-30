//! Node-scoped tag edits retain only explicitly selected identities and labels.
use super::{
    actions,
    history::{FileNode, reads::ReviewOwner},
    merge::selection::{ReviewInventory, SelectedBranch, check_request, check_selection},
    review,
};
use crate::{api, ui::State};
use json_codec::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use web_interface::{i18n::t, prelude::*};
mod selection;
mod view;
pub(crate) use selection::{SelectedNode, Selection};
pub(crate) use view::{chips, view};

#[derive(Clone)]
struct ResolvedReview {
    root: String,
    nodes: Vec<SelectedNode>,
    branches: BTreeMap<String, SelectedBranch>,
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
    targets: BTreeMap<String, Option<String>>,
    branches: BTreeMap<String, SelectedBranch>,
    add: BTreeSet<String>,
    remove: BTreeSet<String>,
    input: String,
    target_page: usize,
    delta_page: usize,
    resolved: Option<ResolvedReview>,
}
impl Draft {
    pub(crate) fn instance(&self) -> u64 {
        self.instance
    }
    pub(crate) fn root(&self) -> &str {
        &self.root
    }

    pub(crate) fn review_inventory(&self) -> ReviewInventory {
        ReviewInventory {
            instance: self.instance,
            node_ids: self.targets.keys().cloned().collect(),
            branch_ids: self.branches.keys().cloned().collect(),
        }
    }

    pub(crate) fn discard_review(&mut self) {
        self.resolved = None;
    }

    pub(crate) fn resolve_review(
        &mut self,
        root: String,
        nodes: &[SelectedNode],
        branches: &[SelectedBranch],
        max_entries: usize,
    ) -> Result<(), String> {
        self.resolved = None;
        let nodes = nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect::<BTreeMap<_, _>>();
        let branches = branches
            .iter()
            .map(|branch| (branch.id.as_str(), branch))
            .collect::<BTreeMap<_, _>>();
        let mut resolved_nodes = Vec::with_capacity(self.targets.len());
        for id in self.targets.keys() {
            let node = nodes.get(id.as_str()).ok_or_else(missing_node)?;
            resolved_nodes.push((**node).clone());
        }
        let mut resolved_branches = BTreeMap::new();
        for id in self.branches.keys() {
            let branch = branches.get(id.as_str()).ok_or_else(missing_branch)?;
            resolved_branches.insert(id.clone(), (**branch).clone());
        }
        let mut candidate = self.clone();
        candidate.branches = resolved_branches.clone();
        candidate.max_entries = max_entries;
        check_request(&payload(&candidate), candidate.targets.len(), max_entries)
            .map_err(|error| error.to_string())?;
        self.resolved = Some(ResolvedReview {
            root,
            nodes: resolved_nodes,
            branches: resolved_branches,
            max_entries,
        });
        Ok(())
    }

    pub(crate) fn accept_review(
        &mut self,
        current_root: &str,
        selected: &mut Selection,
    ) -> Result<(), String> {
        let Some(resolved) = self.resolved.take() else {
            return Err(review_required());
        };
        if resolved.root != current_root {
            return Err(review_required());
        }
        if !self
            .targets
            .keys()
            .map(String::as_str)
            .eq(selected.values().map(|node| node.id.as_str()))
        {
            return Err(missing_node());
        }
        selected
            .accept_review(resolved.root.clone(), resolved.nodes)
            .map_err(|error| error.to_string())?;
        self.root = resolved.root;
        self.branches = resolved.branches;
        self.max_entries = resolved.max_entries;
        Ok(())
    }
}

fn missing_node() -> String {
    t(
        "每个所选节点都必须可读取，不能只提交当前页。",
        "Every selected node must be readable; the action cannot submit only the current page.",
    )
}
fn missing_branch() -> String {
    t(
        "请为每个节点选择有依据节点的可写分支。",
        "Choose a writable branch with a base node for every selected node.",
    )
}
fn review_required() -> String {
    t(
        "历史已改变。请读取并确认全部所选节点及分支依据。",
        "The history changed. Load and confirm every selected node and branch base.",
    )
}
fn tags(node: &FileNode) -> BTreeSet<String> {
    node.tags().iter().cloned().collect()
}
fn valid_tag(value: &str) -> bool {
    storage_contract::validate_tag(value).is_ok()
}
fn result_tags(
    node: &SelectedNode,
    add: &BTreeSet<String>,
    remove: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut result = node.tags.clone();
    result.retain(|name| !remove.contains(name));
    result.extend(add.iter().cloned());
    result
}
fn default_branch(state: &State) -> Option<SelectedBranch> {
    let history = &state.files.file_history;
    history
        .current_branch()
        .or_else(|| {
            history
                .selected()
                .and_then(|id| history.branches().iter().find(|branch| branch.id() == id))
        })
        .and_then(SelectedBranch::capture)
}
fn selection_request(
    selected: &Selection,
    branch: Option<&SelectedBranch>,
    repository: &str,
    path: &str,
) -> Value {
    let targets = selected
        .values()
        .map(|node| {
            json!({
                "source_node_id":node.id,
                "branch_id":branch.map(|branch|branch.id.as_str()).unwrap_or(""),
                "expected_node_id":branch.map(|branch|branch.expected.as_str()).unwrap_or("")
            })
        })
        .collect::<Vec<_>>();
    json!({"targets":targets,"add":[],"remove":[],"repository_id":repository,"path":path})
}
fn checked_selection(state: &State, id: &str, replace: bool) -> Result<Selection, String> {
    if !actions::history_write_allowed(state) {
        return Err(review_required());
    }
    let history = &state.files.file_history;
    let root = history.root().ok_or_else(review_required)?;
    let max_entries = history.action_max_entries().ok_or_else(review_required)?;
    let node = history
        .node(id)
        .and_then(SelectedNode::capture)
        .ok_or_else(missing_node)?;
    let entry = state.files.selected.as_ref().ok_or_else(missing_node)?;
    let (repository, path) = super::model::file_context(entry).ok_or_else(missing_node)?;
    let mut selected = if replace {
        Selection::default()
    } else {
        state.files.checked_nodes.clone()
    };
    selected
        .insert(root, node)
        .map_err(|error| error.to_string())?;
    check_selection(
        &selection_request(
            &selected,
            default_branch(state).as_ref(),
            &repository,
            &path,
        ),
        selected.len(),
        max_entries,
    )
    .map_err(|error| error.to_string())?;
    Ok(selected)
}
pub(crate) fn toggle(state: RwSignal<State>, id: String, checked: bool) {
    state.update(|s| {
        if actions::pending(s) || s.files.tag_edit.is_some() {
            return;
        }
        if !checked {
            s.files.checked_nodes.remove(&id);
            return;
        }
        match checked_selection(s, &id, false) {
            Ok(selected) => s.files.checked_nodes = selected,
            Err(error) => s.notice = error,
        }
    });
}
pub(crate) fn edit_node(state: RwSignal<State>, id: String) {
    let selected = state.with_untracked(|s| checked_selection(s, &id, true));
    match selected {
        Ok(selected) => {
            state.update(|s| s.files.checked_nodes = selected);
            start(state);
        }
        Err(error) => state.update(|s| s.notice = error),
    }
}
pub(crate) fn start(state: RwSignal<State>) {
    let draft = state.with_untracked(|snapshot| {
        if !actions::write_allowed(snapshot)
            || super::model::selected_entry_read_only(&snapshot.files)
            || !snapshot.files.file_history.ready()
            || snapshot.files.checked_nodes.is_empty()
        {
            return None;
        }
        let entry = snapshot.files.selected.as_ref()?;
        let (repository, path) = super::model::file_context(entry)?;
        let root = snapshot.files.checked_nodes.root()?.to_owned();
        let max_entries = snapshot.files.file_history.action_max_entries()?;
        let branch = (snapshot.files.file_history.root() == Some(root.as_str()))
            .then(|| default_branch(snapshot))
            .flatten();
        let targets = snapshot
            .files
            .checked_nodes
            .values()
            .map(|node| {
                (
                    node.id.clone(),
                    branch.as_ref().map(|branch| branch.id.clone()),
                )
            })
            .collect();
        let branches = branch
            .into_iter()
            .map(|branch| (branch.id.clone(), branch))
            .collect();
        Some(Draft {
            instance: snapshot.files.history_next_draft,
            file: entry.file_id().to_owned(),
            repository,
            path,
            root,
            max_entries,
            targets,
            branches,
            add: BTreeSet::new(),
            remove: BTreeSet::new(),
            input: String::new(),
            target_page: 0,
            delta_page: 0,
            resolved: None,
        })
    });
    if let Some(mut draft) = draft {
        review::reset(state, review::Kind::Tags);
        state.update(|s| {
            if let Some(previous) = s.files.tag_edit.as_ref() {
                s.files
                    .history_aux
                    .cancel_review(super::history::reads::ReviewOwner {
                        kind: review::Kind::Tags,
                        instance: previous.instance,
                    });
            }
            s.files.history_next_draft += 1;
            draft.instance = s.files.history_next_draft;
            s.files.tag_edit = Some(draft);
        });
    }
}
fn payload(draft: &Draft) -> Value {
    let targets=draft.targets.iter().map(|(source,branch)| {
        let branch=branch.as_ref().and_then(|id|draft.branches.get(id));
        json!({"source_node_id":source,"branch_id":branch.map(|branch|branch.id.as_str()).unwrap_or(""),"expected_node_id":branch.map(|branch|branch.expected.as_str()).unwrap_or("")})
    }).collect::<Vec<_>>();
    json!({"targets":targets,"add":draft.add,"remove":draft.remove,"repository_id":draft.repository,"path":draft.path})
}
fn update(state: RwSignal<State>, instance: u64, f: impl FnOnce(&mut Draft)) {
    let result = state.try_update(|s| {
        if actions::pending(s) {
            return Ok(());
        }
        let Some(draft) = s.files.tag_edit.as_mut() else {
            return Ok(());
        };
        if draft.instance != instance {
            return Ok(());
        }
        let mut candidate = draft.clone();
        f(&mut candidate);
        check_selection(
            &payload(&candidate),
            candidate.targets.len(),
            candidate.max_entries,
        )
        .map_err(|error| error.to_string())?;
        candidate.resolved = None;
        *draft = candidate;
        s.files.history_aux.cancel_review(ReviewOwner {
            kind: review::Kind::Tags,
            instance,
        });
        Ok(())
    });
    if let Some(Err(error)) = result {
        review::error(state, review::Kind::Tags, error);
    }
}
fn add_input(state: RwSignal<State>, instance: u64) {
    let value = state.with_untracked(|s| {
        s.files
            .tag_edit
            .as_ref()
            .filter(|draft| draft.instance == instance)
            .map(|draft| draft.input.trim().to_owned())
            .unwrap_or_default()
    });
    if value.is_empty() {
        update(state, instance, |draft| draft.input.clear());
        return;
    }
    if !valid_tag(&value) {
        review::error(
            state,
            review::Kind::Tags,
            t(
                "标签不能为空，最多128个UTF-8字节，不能包含控制字符。",
                "Tags must be nonempty, at most 128 UTF-8 bytes, without control characters.",
            ),
        );
        return;
    }
    update(state, instance, |draft| {
        draft.remove.remove(&value);
        draft.add.insert(value);
        draft.input.clear();
    });
}
fn edit_input(state: RwSignal<State>, instance: u64, value: String) {
    state.update(|s| {
        if actions::pending(s) {
            return;
        }
        let Some(draft) = s.files.tag_edit.as_mut() else {
            return;
        };
        if draft.instance != instance {
            return;
        }
        draft.input = value;
        draft.resolved = None;
        s.files.history_aux.cancel_review(ReviewOwner {
            kind: review::Kind::Tags,
            instance,
        });
    });
}
fn assign_branch(state: RwSignal<State>, instance: u64, source: String, id: String) {
    let result = state.try_update(|s| {
        if actions::pending(s) {
            return Ok(());
        }
        let Some(draft) = s.files.tag_edit.as_mut() else {
            return Ok(());
        };
        if draft.instance != instance {
            return Ok(());
        }
        if s.files.file_history.root() != Some(draft.root()) || !s.files.file_history.ready() {
            return Err(review_required());
        }
        if !draft.targets.contains_key(&source) {
            return Err(missing_node());
        }
        let branch = s
            .files
            .file_history
            .branches()
            .iter()
            .find(|branch| branch.id() == id)
            .and_then(SelectedBranch::capture)
            .or_else(|| draft.branches.get(&id).cloned())
            .ok_or_else(missing_branch)?;
        let mut candidate = draft.clone();
        candidate.targets.insert(source, Some(id.clone()));
        candidate.branches.insert(id, branch);
        let retained = candidate
            .targets
            .values()
            .flatten()
            .cloned()
            .collect::<BTreeSet<_>>();
        candidate.branches.retain(|id, _| retained.contains(id));
        check_selection(
            &payload(&candidate),
            candidate.targets.len(),
            candidate.max_entries,
        )
        .map_err(|error| error.to_string())?;
        candidate.resolved = None;
        *draft = candidate;
        s.files.history_aux.cancel_review(ReviewOwner {
            kind: review::Kind::Tags,
            instance,
        });
        Ok(())
    });
    if let Some(Err(error)) = result {
        review::error(state, review::Kind::Tags, error);
    }
}
fn accept_review(state: RwSignal<State>, instance: u64) {
    let result = state.try_update(|s| {
        if actions::pending(s) {
            return Ok(());
        }
        if !s
            .files
            .tag_edit
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
        let Some(draft) = s.files.tag_edit.as_mut() else {
            return Ok(());
        };
        draft.accept_review(&root, &mut s.files.checked_nodes)
    });
    if let Some(Err(error)) = result {
        review::error(state, review::Kind::Tags, error);
    }
}
fn request(draft: &Draft, selected: &Selection) -> Result<Value, String> {
    if !draft.input.trim().is_empty() {
        return Err(t(
            "请先点击“添加标签”加入输入框中的标签，或清空输入框后再保存。",
            "Click Add tag to include the tag in the input, or clear the input before saving.",
        ));
    }
    if draft.targets.is_empty() || draft.add.is_empty() && draft.remove.is_empty() {
        return Err(t(
            "请先选择节点并添加或移除标签。",
            "Select nodes, then add or remove tags.",
        ));
    }
    if selected.root() != Some(draft.root()) || selected.len() != draft.targets.len() {
        return Err(missing_node());
    }
    if draft
        .add
        .iter()
        .chain(&draft.remove)
        .any(|name| !valid_tag(name))
    {
        return Err(t(
            "标签无效，最多128个UTF-8字节。",
            "Invalid tag. Use at most 128 UTF-8 bytes.",
        ));
    }
    let mut changed = false;
    for (source, branch) in &draft.targets {
        let node = selected.get(source).ok_or_else(missing_node)?;
        branch
            .as_ref()
            .and_then(|id| draft.branches.get(id))
            .ok_or_else(missing_branch)?;
        let projected = result_tags(node, &draft.add, &draft.remove);
        if projected.len() > storage_contract::MAX_FILE_TAGS {
            return Err(t(
                "每个节点最多16个标签。",
                "Each node can have at most 16 tags.",
            ));
        }
        changed |= projected != node.tags;
    }
    if !changed {
        return Err(t("标签没有变化。", "The tags have not changed."));
    }
    let body = payload(draft);
    check_request(&body, draft.targets.len(), draft.max_entries)
        .map_err(|error| error.to_string())?;
    Ok(body)
}
fn submit(state: RwSignal<State>, instance: u64) {
    let result = state.with_untracked(|s| {
        let draft = s.files.tag_edit.as_ref()?;
        if draft.instance != instance {
            return None;
        }
        if !actions::history_write_allowed(s) || s.files.file_history.root() != Some(draft.root()) {
            return Some(Err(review_required()));
        }
        Some(request(draft, &s.files.checked_nodes).map(|body| (draft.file.clone(), body)))
    });
    match result {
        Some(Ok((file, body))) => review::submit(
            state,
            review::Kind::Tags,
            "POST",
            format!("/api/file-manager/files/{}/tags", api::segment(&file)),
            body,
        ),
        Some(Err(error)) => review::error(state, review::Kind::Tags, error),
        None => {}
    }
}
#[cfg(test)]
#[path = "../../../../../test/web/unit/files/tags/tests.rs"]
pub(crate) mod tests;
