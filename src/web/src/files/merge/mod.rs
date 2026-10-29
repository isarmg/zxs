//! Explicit multi-branch merge review. Selected heads are frozen when checked;
//! candidate pages are replaceable. The chosen result may be any file node, reached
//! by paging the resident window. The draft keeps only identities and one
//! compact summary of the chosen node, never copies of nodes or branches.
use super::{
    actions,
    history::{HistoryNodeSummary, HistoryRoot},
    model::{Model, selected_branch},
    review,
};
use crate::{api, format, ui::State};
use json_codec::{Value, json};
use std::collections::BTreeMap;
use web_interface::{i18n::t, prelude::*};
mod view;
pub(crate) use view::{history, view};

/// Display summary of the chosen result node; it stays visible after the
/// page that contained the node has been replaced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NodeLabel {
    pub(crate) node_id: String,
    pub(crate) version_id: String,
    pub(crate) path: String,
    pub(crate) created_at: String,
}
impl NodeLabel {
    pub(crate) fn capture(node: &HistoryNodeSummary) -> Self {
        Self {
            node_id: node.node_id.clone(),
            version_id: node.version_id.clone(),
            path: node.path.clone(),
            created_at: node.created_at.clone(),
        }
    }
}

/// An explicitly selected input: identity, display name and asserted head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InputBranch {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) head: Option<String>,
}

#[derive(Clone)]
pub(crate) struct Draft {
    file: String,
    repository: String,
    path: String,
    /// Root the branch heads were captured under.
    root: HistoryRoot,
    /// Explicit selections only. Unselected candidates belong to the current
    /// branch page, which is discarded when paging.
    branch_heads: BTreeMap<String, InputBranch>,
    input_bytes: usize,
    /// Local display page of selected inputs (independent of candidate pages).
    offset: usize,
    chosen: String,
    chosen_summary: Option<NodeLabel>,
    survivor: String,
}
impl Draft {
    pub(crate) fn chosen_summary(&self) -> Option<&NodeLabel> {
        self.chosen_summary.as_ref()
    }
    pub(crate) fn branches(&self) -> impl Iterator<Item = &InputBranch> {
        self.branch_heads.values()
    }
}

impl InputBranch {
    fn capture(branch: &super::history::FileBranch) -> Self {
        Self {
            id: branch.id().to_owned(),
            name: branch.name().to_owned(),
            head: branch.head().map(str::to_owned),
        }
    }
    fn request_entry(&self) -> Value {
        json!({"branch_id":self.id,"expected_node_id":self.head})
    }
    fn encoded_bytes(&self) -> usize {
        json_codec::to_vec(&self.request_entry())
            .expect("invariant: a JSON input encodes")
            .len()
            + 1
    }
}

pub(super) const SELECTED_PAGE: usize = 50;
const MAX_REQUEST_BYTES: usize = storage_contract::client_protocol::MAX_JSON_REQUEST_BYTES;

fn input_reserve(draft: &Draft) -> usize {
    // The two choices may still change. Reserve their maximum escaped size;
    // repository/path are already fixed for this draft, and the operation id
    // is added only at publication. Nothing is truncated to fit the budget.
    let maximum_id = "\0".repeat(256);
    json_codec::to_vec(&json!({
        "inputs":[], "chosen_node_id":maximum_id, "surviving_branch_id":maximum_id,
        "repository_id":draft.repository, "path":draft.path, "confirmed":true,
        "operation_id":format!("op_{}", "0".repeat(32)),
    }))
    .expect("invariant: a JSON request shell encodes")
    .len()
}

pub(super) fn select_input(model: &mut Model, id: &str, checked: bool) -> Result<(), String> {
    let Some(draft) = model.merge.as_ref() else {
        return Ok(());
    };
    if !checked {
        let draft = model
            .merge
            .as_mut()
            .expect("invariant: draft was borrowed above");
        if let Some(branch) = draft.branch_heads.remove(id) {
            draft.input_bytes -= branch.encoded_bytes();
        }
        if draft.survivor == id {
            draft.survivor.clear();
        }
        draft.offset = draft
            .offset
            .min(draft.branch_heads.len().saturating_sub(1) / SELECTED_PAGE * SELECTED_PAGE);
        return Ok(());
    }
    if model.history.root() != Some(&draft.root) {
        return Err(t(
            "历史已变化，请取消并重审草稿。",
            "The history changed. Cancel and review the draft again.",
        ));
    }
    if draft.branch_heads.contains_key(id) {
        return Ok(());
    }
    let branch = model
        .history
        .branch_page()
        .and_then(|page| page.branch(id))
        .filter(|branch| branch.writable() && branch.head().is_some())
        .ok_or_else(|| {
            t(
                "请选择本页中有依据节点的可写分支。",
                "Choose a writable branch with a base node from this page.",
            )
        })?;
    let branch = InputBranch::capture(branch);
    let bytes = branch.encoded_bytes();
    if input_reserve(draft) + draft.input_bytes + bytes > MAX_REQUEST_BYTES {
        return Err(t(
            "所选分支已达到单次请求 1 MiB 上限，无法再添加。",
            "The selected branches reached the 1 MiB request limit; this addition was refused.",
        ));
    }
    let draft = model
        .merge
        .as_mut()
        .expect("invariant: draft was borrowed above");
    draft.input_bytes += bytes;
    draft.branch_heads.insert(id.to_owned(), branch);
    Ok(())
}

pub(super) fn toggle_input(state: RwSignal<State>, id: &str, checked: bool) {
    if let Some(Err(error)) = state.try_update(|s| select_input(&mut s.files, id, checked)) {
        review::error(state, review::Kind::Merge, error);
    }
}

/// Other writable candidates may be on later pages; opening the panel must
/// not infer the total number of writable branches from one candidate page.
pub(crate) fn available(model: &Model) -> bool {
    !model.history.loading()
        && model.history.root().is_some()
        && model.history.branch_page().is_some_and(|page| {
            page.writable().count() >= 2
                || page.previous_cursor.is_some()
                || page.next_cursor.is_some()
        })
}
pub(crate) fn start(state: RwSignal<State>) {
    let Some(draft) = state.with_untracked(|snapshot| {
        if !actions::write_allowed(snapshot)
            || super::model::selected_entry_read_only(&snapshot.files)
            || !available(&snapshot.files)
        {
            return None;
        }
        let entry = snapshot.files.selected.as_ref()?;
        let file = entry.file_id().to_owned();
        let (repository, path) = super::model::file_context(entry)?;
        let root = snapshot.files.history.root()?.clone();
        let page = snapshot.files.history.branch_page()?;
        let current = selected_branch(&snapshot.files);
        let branch_heads = page
            .branch(&current)
            .filter(|branch| branch.writable() && branch.head().is_some())
            .map(|branch| (current, InputBranch::capture(branch)))
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let input_bytes = branch_heads.values().map(InputBranch::encoded_bytes).sum();
        Some(Draft {
            file,
            repository,
            path,
            root,
            branch_heads,
            input_bytes,
            offset: 0,
            chosen: String::new(),
            chosen_summary: None,
            survivor: String::new(),
        })
    }) else {
        return;
    };
    review::reset(state, review::Kind::Merge);
    state.update(|s| s.files.merge = Some(draft));
}

/// Choose the result node from the resident page. A node that is not
/// resident cannot be chosen; the user pages or anchors to it first.
pub(crate) fn choose(model: &mut Model, node_id: &str) -> bool {
    let Some(summary) = model
        .history
        .loaded()
        .and_then(|window| window.node(node_id))
        .map(NodeLabel::capture)
    else {
        return false;
    };
    if let Some(draft) = model.merge.as_mut() {
        draft.chosen = summary.node_id.clone();
        draft.chosen_summary = Some(summary);
        return true;
    }
    false
}

fn request(draft: &Draft, current_root: Option<&HistoryRoot>) -> Result<Value, String> {
    if current_root != Some(&draft.root) {
        return Err(t(
            "历史已变化，草稿需要重审：请刷新后重新确认参与分支和结果节点。",
            "The history changed. Review the draft: refresh and confirm the input branches and result node again.",
        ));
    }
    if draft.branch_heads.len() < 2 {
        return Err(t(
            "至少选择两个可写文件分支。",
            "Select at least two writable file branches.",
        ));
    }
    let mut inputs = vec![];
    for (id, branch) in &draft.branch_heads {
        let head = branch.head.as_deref().ok_or_else(|| {
            t(
                "分支依据节点缺失，请取消后刷新。",
                "A branch base node is missing. Cancel and refresh.",
            )
        })?;
        inputs.push(json!({"branch_id":id,"expected_node_id":head}));
    }
    if draft
        .chosen_summary
        .as_ref()
        .is_none_or(|summary| summary.node_id != draft.chosen || summary.version_id.is_empty())
    {
        return Err(t(
            "请选择这个文件的一个历史节点作为结果。",
            "Choose a history node of this file as the result.",
        ));
    }
    if !draft.branch_heads.contains_key(&draft.survivor) {
        return Err(t(
            "请明确选择合并后保留的分支。",
            "Explicitly choose the branch to keep after merging.",
        ));
    }
    let body = json!({"inputs":inputs,"chosen_node_id":draft.chosen,"surviving_branch_id":draft.survivor,"repository_id":draft.repository,"path":draft.path,"confirmed":true});
    let bytes = json_codec::to_vec(&body)
        .expect("invariant: a JSON request encodes")
        .len();
    let operation = json_codec::to_vec(&json!({"operation_id":format!("op_{}", "0".repeat(32))}))
        .expect("invariant: a JSON operation id encodes")
        .len();
    if bytes + operation > MAX_REQUEST_BYTES {
        return Err(t(
            "请求超过 1 MiB 上限，请减少所选分支。",
            "The request exceeds the 1 MiB limit. Select fewer branches.",
        ));
    }
    Ok(body)
}
fn submit(state: RwSignal<State>) {
    let Some(draft) = state.with_untracked(|s| s.files.merge.clone()) else {
        return;
    };
    let result = state.with_untracked(|s| request(&draft, s.files.history.root()));
    match result {
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
fn update(state: RwSignal<State>, f: impl FnOnce(&mut Draft)) {
    state.update(|s| {
        if let Some(draft) = s.files.merge.as_mut() {
            f(draft)
        }
    });
}
pub(super) fn node_label(node: &HistoryNodeSummary, index: usize) -> String {
    let date = format::timestamp(&node.created_at);
    let descriptor = if node.note.is_empty() {
        match node.kind {
            super::history::NodeKind::Rename => t("路径变化", "Path changed"),
            super::history::NodeKind::Tags => t("标签变化", "Tags changed"),
            super::history::NodeKind::Merge => t("合并节点", "Merge node"),
            super::history::NodeKind::Content => t("文件内容", "File content"),
        }
    } else {
        node.note.clone()
    };
    format!("{} · {date} · {descriptor}", index + 1)
}
pub(super) fn summary_label(summary: &NodeLabel) -> String {
    format!(
        "{} · {}",
        format::timestamp(&summary.created_at),
        if summary.path.is_empty() {
            summary.node_id.clone()
        } else {
            summary.path.clone()
        }
    )
}
#[cfg(test)]
#[path = "../../../../../test/web/unit/files/merge/tests.rs"]
mod tests;
