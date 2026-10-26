//! Node-scoped inherited labels. The editor borrows the chip/pressed-choice
//! interaction of the referenced tag toolbar, with ZXS geometry and publication.
use super::{
    actions, merge,
    model::{history_node_id, history_nodes, selected_branch},
    review,
};
use crate::{
    api,
    ui::{State, text},
};
use json_codec::{Value, json};
use std::collections::BTreeSet;
use web_interface::{i18n::t, prelude::*};
mod view;
pub(crate) use view::{chips, view};

#[derive(Clone)]
struct Target {
    source: String,
    branch: String,
    expected: String,
}
#[derive(Clone)]
pub(crate) struct Draft {
    file: String,
    repository: String,
    path: String,
    nodes: Vec<Value>,
    branches: Vec<Value>,
    targets: Vec<Target>,
    add: BTreeSet<String>,
    remove: BTreeSet<String>,
    input: String,
}
fn tags(node: &Value) -> BTreeSet<String> {
    node["tags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}
fn valid_tag(value: &str) -> bool {
    storage_contract::validate_tag(value).is_ok()
}
fn result_tags(
    node: &Value,
    add: &BTreeSet<String>,
    remove: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut result = tags(node);
    result.retain(|name| !remove.contains(name));
    result.extend(add.iter().cloned());
    result
}
pub(crate) fn toggle(state: RwSignal<State>, id: String, checked: bool) {
    state.update(|s| {
        if s.files.tag_edit.is_some() {
            return;
        }
        if checked {
            s.files.checked_nodes.insert(id);
        } else {
            s.files.checked_nodes.remove(&id);
        }
    });
}
pub(crate) fn edit_node(state: RwSignal<State>, id: String) {
    state.update(|s| {
        s.files.checked_nodes = BTreeSet::from([id]);
    });
    start(state);
}
pub(crate) fn start(state: RwSignal<State>) {
    let draft = state.with_untracked(|snapshot| {
        if !actions::write_allowed(snapshot)
            || super::model::selected_entry_read_only(&snapshot.files)
            || snapshot.files.history_loading
            || snapshot.files.checked_nodes.is_empty()
        {
            return Ok(None);
        }
        let Some(entry) = snapshot.files.selected.as_ref() else {
            return Ok(None);
        };
        let Some((repository, path)) = super::model::file_context(&snapshot.files, entry) else {
            return Ok(None);
        };
        let file = text(entry, &["file_id"]);
        let nodes = history_nodes(&snapshot.files.file_history)
            .into_iter()
            .filter(|node| {
                snapshot
                    .files
                    .checked_nodes
                    .contains(&history_node_id(node))
            })
            .collect::<Vec<_>>();
        // A refresh may remove a previously checked node. Never silently turn a
        // user's batch into a partial edit.
        if nodes.len() != snapshot.files.checked_nodes.len() {
            return Err(t(
                "所选节点已变化，请重新选择后编辑标签。",
                "The selected nodes have changed. Select them again before editing tags.",
            ));
        }
        let branches = snapshot.files.file_history["branches"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|branch| merge::branch_writable(branch))
            .cloned()
            .collect::<Vec<_>>();
        let selected = selected_branch(&snapshot.files);
        let branch = branches
            .iter()
            .find(|branch| text(branch, &["branch_id"]) == selected);
        let expected = branch.map(super::merge::branch_head).unwrap_or_default();
        let selected = if branch.is_some() {
            selected
        } else {
            String::new()
        };
        let targets = nodes
            .iter()
            .map(|node| Target {
                source: history_node_id(node),
                branch: selected.clone(),
                expected: expected.clone(),
            })
            .collect();
        Ok(Some(Draft {
            file,
            repository,
            path,
            nodes,
            branches,
            targets,
            add: BTreeSet::new(),
            remove: BTreeSet::new(),
            input: String::new(),
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
    update(state, |draft| {
        draft.remove.remove(&value);
        draft.add.insert(value);
        draft.input.clear();
    });
}
fn request(draft: &Draft) -> Result<Value, String> {
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
    let mut targets = vec![];
    let mut changed = false;
    for target in &draft.targets {
        let node = draft
            .nodes
            .iter()
            .find(|node| history_node_id(node) == target.source)
            .ok_or_else(|| {
                t(
                    "节点缺失，请取消并刷新。",
                    "A node is missing. Cancel and refresh.",
                )
            })?;
        let branch = draft
            .branches
            .iter()
            .find(|branch| {
                text(branch, &["branch_id"]) == target.branch && merge::branch_writable(branch)
            })
            .ok_or_else(|| {
                t(
                    "请为每个节点选择可写的关联分支。",
                    "Choose a writable associated branch for each node.",
                )
            })?;
        if target.expected.is_empty() || target.expected != merge::branch_head(branch) {
            return Err(t(
                "分支依据节点缺失，请取消并刷新。",
                "A branch base node is missing. Cancel and refresh.",
            ));
        }
        let projected = result_tags(node, &draft.add, &draft.remove);
        if projected.len() > storage_contract::MAX_FILE_TAGS {
            return Err(t(
                "每个节点最多16个标签。",
                "Each node can have at most 16 tags.",
            ));
        }
        changed |= projected != tags(node);
        targets.push(json!({"source_node_id":target.source,"branch_id":target.branch,"expected_node_id":target.expected}));
    }
    if !changed {
        return Err(t("标签没有变化。", "The tags have not changed."));
    }
    Ok(
        json!({"targets":targets,"add":draft.add,"remove":draft.remove,"repository_id":draft.repository,"path":draft.path}),
    )
}
fn submit(state: RwSignal<State>) {
    let Some(draft) = state.with_untracked(|s| s.files.tag_edit.clone()) else {
        return;
    };
    match request(&draft) {
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
