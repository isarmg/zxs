//! Node-scoped inherited labels. The editor borrows the chip/pressed-choice
//! interaction of the referenced tag toolbar, with ZXS geometry and publication.
//!
//! Checked nodes are recorded with exactly the data the tag request needs, so
//! paging through history never drops a selection and never retains a page.
//! Each addition is checked against the complete encoded request budget.
use super::{
    actions, merge,
    model::{Model, selected_branch},
    review,
};
use crate::{api, ui::State};
use json_codec::{Value, json};
use std::collections::BTreeSet;
use web_interface::{i18n::t, prelude::*};
mod view;
pub(crate) use view::{chips, view};

/// Rows of the selected list rendered per local page.
pub(crate) const SELECTED_PAGE: usize = 50;

/// A checked history node, captured from the resident page when checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CheckedNode {
    pub(crate) root: super::history::HistoryRoot,
    pub(crate) node_id: String,
    pub(crate) branch: String,
    pub(crate) expected: String,
    pub(crate) tags: BTreeSet<String>,
    pub(crate) label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    source: String,
    branch: String,
    expected: String,
    tags: BTreeSet<String>,
    label: String,
}
impl Target {
    fn from_checked(node: &CheckedNode) -> Self {
        Self {
            source: node.node_id.clone(),
            branch: node.branch.clone(),
            expected: node.expected.clone(),
            tags: node.tags.clone(),
            label: node.label.clone(),
        }
    }
    fn request_entry(&self) -> Value {
        json!({"source_node_id":self.source,"branch_id":self.branch,"expected_node_id":self.expected})
    }
}

/// Compact explicitly selected branch/head, never a retained candidate page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BranchChoice {
    id: String,
    name: String,
    head: String,
}

#[derive(Clone)]
pub(crate) struct Draft {
    file: String,
    repository: String,
    path: String,
    /// Root the targets' expected heads were captured under.
    root: super::history::HistoryRoot,
    branches: Vec<BranchChoice>,
    targets: Vec<Target>,
    add: BTreeSet<String>,
    remove: BTreeSet<String>,
    input: String,
    /// Local offset into `targets` for the paged selected list.
    pub(crate) offset: usize,
}

fn valid_tag(value: &str) -> bool {
    storage_contract::validate_tag(value).is_ok()
}
fn result_tags(
    tags: &BTreeSet<String>,
    add: &BTreeSet<String>,
    remove: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut result = tags.clone();
    result.retain(|name| !remove.contains(name));
    result.extend(add.iter().cloned());
    result
}

/// Fixed reserve for everything in a tag request except the targets: the
/// operation id, a repository id and path at their maximum legal lengths and
/// the largest possible add/remove sets. The per-target accounting then only
/// has to add each encoded target entry.
pub(crate) fn request_reserve() -> usize {
    let tags = (0..storage_contract::MAX_FILE_TAGS)
        .map(|_| "\\".repeat(storage_contract::MAX_TAG_BYTES))
        .collect::<Vec<_>>();
    let shell = json!({
        "targets":[],
        "add":tags,
        "remove":tags,
        "repository_id":"\0".repeat(256),
        "path":"\0".repeat(768),
        "operation_id":format!("op_{}", "0".repeat(32)),
    });
    json_codec::to_vec(&shell)
        .expect("invariant: a literal JSON shell encodes")
        .len()
}
fn operation_id_reserve() -> usize {
    json_codec::to_vec(&json!({"operation_id":format!("op_{}", "0".repeat(32))}))
        .expect("invariant: a literal JSON shell encodes")
        .len()
}
fn target_bytes(target: &Target) -> usize {
    // Each array element is followed by a separator; count one per entry.
    json_codec::to_vec(&target.request_entry())
        .expect("invariant: a literal JSON entry encodes")
        .len()
        + 1
}
const MAX_REQUEST_BYTES: usize = storage_contract::client_protocol::MAX_JSON_REQUEST_BYTES;

/// Capture the checked node from the resident page with the branch/head the
/// request will assert. Returns `None` when the node is not resident.
fn capture(model: &Model, id: &str) -> Option<CheckedNode> {
    let window = model.history.loaded()?;
    let (position, node) = window
        .nodes
        .iter()
        .enumerate()
        .find(|(_, node)| node.node_id == id)?;
    let selected = selected_branch(model);
    let branch = model
        .history
        .branch_page()
        .and_then(|page| page.branch(&selected))
        .filter(|branch| branch.writable());
    Some(CheckedNode {
        root: window.root().clone(),
        node_id: node.node_id.clone(),
        branch: branch
            .map(|branch| branch.id().to_owned())
            .unwrap_or_default(),
        expected: branch
            .and_then(|branch| branch.head())
            .unwrap_or_default()
            .to_owned(),
        tags: node.tags.clone(),
        label: merge::node_label(node, position),
    })
}

/// Outcome of checking a node: the budget decision is explicit, never silent.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Check {
    Changed,
    NotResident,
    BudgetExceeded,
    Ignored,
}

pub(crate) fn check(model: &mut Model, id: &str, checked: bool) -> Check {
    if model.tag_edit.is_some() {
        return Check::Ignored;
    }
    if !checked {
        if let Some(removed) = model.checked_nodes.remove(id) {
            model.checked_bytes -= target_bytes(&Target::from_checked(&removed));
            return Check::Changed;
        }
        return Check::Ignored;
    }
    if model.checked_nodes.contains_key(id) {
        return Check::Ignored;
    }
    let Some(node) = capture(model, id) else {
        return Check::NotResident;
    };
    let bytes = target_bytes(&Target::from_checked(&node));
    if request_reserve() + model.checked_bytes + bytes > MAX_REQUEST_BYTES {
        return Check::BudgetExceeded;
    }
    model.checked_bytes += bytes;
    model.checked_nodes.insert(id.to_owned(), node);
    Check::Changed
}

pub(crate) fn toggle(state: RwSignal<State>, id: String, checked: bool) {
    let outcome = state
        .try_update(|s| check(&mut s.files, &id, checked))
        .unwrap_or(Check::Ignored);
    match outcome {
        Check::BudgetExceeded => review::error(
            state,
            review::Kind::Tags,
            t(
                "所选节点已达到单次请求 1 MiB 上限，无法再添加；请先保存或取消部分选择。",
                "The selected nodes reached the 1 MiB request limit. Save or unselect some nodes before adding more.",
            ),
        ),
        Check::NotResident => review::error(
            state,
            review::Kind::Tags,
            t(
                "该节点不在当前历史页，无法选择。",
                "This node is not on the current history page and cannot be selected.",
            ),
        ),
        Check::Changed | Check::Ignored => {}
    }
}
pub(crate) fn edit_node(state: RwSignal<State>, id: String) {
    state.update(|s| s.files.clear_checked_nodes());
    toggle(state, id, true);
    start(state);
}
pub(crate) fn start(state: RwSignal<State>) {
    let draft = state.with_untracked(|snapshot| {
        if !actions::write_allowed(snapshot)
            || super::model::selected_entry_read_only(&snapshot.files)
            || snapshot.files.history.loading()
            || snapshot.files.checked_nodes.is_empty()
        {
            return Ok(None);
        }
        let Some(entry) = snapshot.files.selected.as_ref() else {
            return Ok(None);
        };
        let Some((repository, path)) = super::model::file_context(entry) else {
            return Ok(None);
        };
        // Writes depend on a current root; a stale window offers no branch page.
        let Some(root) = snapshot.files.history.root() else {
            return Err(t(
                "历史已变化，请刷新后再编辑标签。",
                "The history changed. Refresh before editing tags.",
            ));
        };
        if snapshot
            .files
            .checked_nodes
            .values()
            .any(|node| &node.root != root)
        {
            return Err(t(
                "历史已变化，请取消旧选择后重新选择节点。",
                "The history changed. Clear the old selection and select the nodes again.",
            ));
        }
        let page = snapshot.files.history.branch_page();
        let branches = snapshot
            .files
            .checked_nodes
            .values()
            .filter(|node| !node.branch.is_empty() && !node.expected.is_empty())
            .map(|node| {
                (
                    node.branch.clone(),
                    BranchChoice {
                        id: node.branch.clone(),
                        name: page
                            .and_then(|page| page.branch(&node.branch))
                            .map(|branch| branch.name().to_owned())
                            .unwrap_or_else(|| node.branch.clone()),
                        head: node.expected.clone(),
                    },
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>()
            .into_values()
            .collect();
        let file = entry.file_id().to_owned();
        // Targets are the recorded checked nodes; nodes checked under this
        // root on an earlier page keep their recorded branch and head.
        let targets = snapshot
            .files
            .checked_nodes
            .values()
            .map(Target::from_checked)
            .collect::<Vec<_>>();
        Ok(Some(Draft {
            file,
            repository,
            path,
            root: root.clone(),
            branches,
            targets,
            add: BTreeSet::new(),
            remove: BTreeSet::new(),
            input: String::new(),
            offset: 0,
        }))
    });
    match draft {
        Ok(Some(draft)) => {
            review::reset(state, review::Kind::Tags);
            state.update(|s| s.files.tag_edit = Some(draft));
        }
        Ok(None) => {}
        Err(message) => state.update(|s| s.notice = message),
    }
}

fn update(state: RwSignal<State>, f: impl FnOnce(&mut Draft)) {
    state.update(|s| {
        if let Some(draft) = s.files.tag_edit.as_mut() {
            f(draft)
        }
    });
}

/// A branch selection is an explicit current-page action. It may replace a
/// target chosen on another page, but never refreshes an old root's heads.
fn choose_branch(model: &mut Model, source: &str, id: &str) -> Result<(), String> {
    let Some(draft) = model.tag_edit.as_ref() else {
        return Ok(());
    };
    if model.history.root() != Some(&draft.root) {
        return Err(t(
            "历史已变化，请取消并重审草稿。",
            "The history changed. Cancel and review the draft again.",
        ));
    }
    let candidate = model
        .history
        .branch_page()
        .and_then(|page| page.branch(id))
        .filter(|branch| branch.writable())
        .ok_or_else(|| {
            t(
                "请选择本页的可写分支。",
                "Choose a writable branch from this page.",
            )
        })?;
    let head = candidate
        .head()
        .ok_or_else(|| t("分支依据节点缺失。", "The branch base node is missing."))?;
    let choice = BranchChoice {
        id: id.to_owned(),
        name: candidate.name().to_owned(),
        head: head.to_owned(),
    };
    let Some(target) = draft.targets.iter().find(|target| target.source == source) else {
        return Ok(());
    };
    let mut changed = target.clone();
    changed.branch = choice.id.clone();
    changed.expected = choice.head.clone();
    let current_bytes: usize = draft.targets.iter().map(target_bytes).sum();
    if request_reserve() + current_bytes - target_bytes(target) + target_bytes(&changed)
        > MAX_REQUEST_BYTES
    {
        return Err(t(
            "请求已达到 1 MiB 上限，无法更改关联分支。",
            "The request reached the 1 MiB limit; the associated branch was not changed.",
        ));
    }
    let draft = model
        .tag_edit
        .as_mut()
        .expect("invariant: draft was borrowed above");
    *draft
        .targets
        .iter_mut()
        .find(|target| target.source == source)
        .expect("invariant: selected target was found above") = changed;
    if !draft.branches.iter().any(|branch| branch.id == choice.id) {
        draft.branches.push(choice);
    }
    draft.branches.retain(|branch| {
        draft
            .targets
            .iter()
            .any(|target| target.branch == branch.id)
    });
    Ok(())
}

fn select_branch(state: RwSignal<State>, source: &str, id: &str) {
    if let Some(Err(error)) = state.try_update(|s| choose_branch(&mut s.files, source, id)) {
        review::error(state, review::Kind::Tags, error);
    }
}
fn add_input(state: RwSignal<State>) {
    let value = state.with_untracked(|s| {
        s.files
            .tag_edit
            .as_ref()
            .map(|draft| draft.input.trim().to_owned())
            .unwrap_or_default()
    });
    if value.is_empty() {
        update(state, |draft| draft.input.clear());
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
    let error = state
        .try_update(|s| {
            s.files
                .tag_edit
                .as_mut()
                .and_then(|draft| add_tag(draft, value).err())
        })
        .flatten();
    if let Some(error) = error {
        review::error(state, review::Kind::Tags, error);
    }
}

fn delta_limit() -> String {
    t(
        "一次最多添加或移除 16 个不同标签；此次修改未加入。",
        "One request can add or remove at most 16 distinct tags; this change was not added.",
    )
}

fn add_tag(draft: &mut Draft, value: String) -> Result<(), String> {
    if draft.add.len() == storage_contract::MAX_FILE_TAGS && !draft.add.contains(&value) {
        return Err(delta_limit());
    }
    draft.remove.remove(&value);
    draft.add.insert(value);
    draft.input.clear();
    Ok(())
}

fn toggle_removal(draft: &mut Draft, value: &str) -> Result<(), String> {
    if draft.remove.contains(value) {
        draft.remove.remove(value);
        return Ok(());
    }
    if draft.remove.len() == storage_contract::MAX_FILE_TAGS {
        return Err(delta_limit());
    }
    draft.add.remove(value);
    draft.remove.insert(value.to_owned());
    Ok(())
}

fn remove_tag(state: RwSignal<State>, value: &str) {
    let error = state
        .try_update(|s| {
            s.files
                .tag_edit
                .as_mut()
                .and_then(|draft| toggle_removal(draft, value).err())
        })
        .flatten();
    if let Some(error) = error {
        review::error(state, review::Kind::Tags, error);
    }
}
fn request(
    draft: &Draft,
    current_root: Option<&super::history::HistoryRoot>,
) -> Result<Value, String> {
    if current_root != Some(&draft.root) {
        return Err(t(
            "历史已变化，草稿需要重审：请刷新后重新确认所选节点。",
            "The history changed. Review the draft: refresh and confirm the selected nodes again.",
        ));
    }
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
    if draft.add.len() > storage_contract::MAX_FILE_TAGS
        || draft.remove.len() > storage_contract::MAX_FILE_TAGS
    {
        return Err(delta_limit());
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
    let mut targets = Vec::with_capacity(draft.targets.len());
    let mut changed = false;
    for target in &draft.targets {
        let branch = draft
            .branches
            .iter()
            .find(|branch| branch.id == target.branch)
            .ok_or_else(|| {
                t(
                    "请为每个节点选择可写的关联分支。",
                    "Choose a writable associated branch for each node.",
                )
            })?;
        if branch.head != target.expected {
            return Err(t(
                "分支依据节点缺失，请取消并刷新。",
                "A branch base node is missing. Cancel and refresh.",
            ));
        }
        let projected = result_tags(&target.tags, &draft.add, &draft.remove);
        if projected.len() > storage_contract::MAX_FILE_TAGS {
            return Err(t(
                "每个节点最多16个标签。",
                "Each node can have at most 16 tags.",
            ));
        }
        changed |= projected != target.tags;
        targets.push(target.request_entry());
    }
    if !changed {
        return Err(t("标签没有变化。", "The tags have not changed."));
    }
    let body = json!({"targets":targets,"add":draft.add,"remove":draft.remove,"repository_id":draft.repository,"path":draft.path});
    // The per-target accounting already reserved the shell; the final check
    // measures the complete encoded request (plus the operation id added by
    // the submitter) so no path can send an oversized body or split it.
    let encoded = json_codec::to_vec(&body).expect("invariant: a literal JSON body encodes");
    if encoded.len() + operation_id_reserve() > MAX_REQUEST_BYTES {
        return Err(t(
            "请求超过 1 MiB 上限，请减少所选节点。",
            "The request exceeds the 1 MiB limit. Select fewer nodes.",
        ));
    }
    Ok(body)
}
fn submit(state: RwSignal<State>) {
    let Some(draft) = state.with_untracked(|s| s.files.tag_edit.clone()) else {
        return;
    };
    let result = state.with_untracked(|s| request(&draft, s.files.history.root()));
    match result {
        Ok(body) => review::submit(
            state,
            review::Kind::Tags,
            "POST",
            format!("/api/file-manager/files/{}/tags", api::segment(&draft.file)),
            body,
        ),
        Err(error) => review::error(state, review::Kind::Tags, error),
    }
}
#[cfg(test)]
#[path = "../../../../../test/web/unit/files/tags/tests.rs"]
mod tests;
