//! Object-bound history selection. Source identities stay frozen across pages;
//! only the selected result is adopted into a new, explicitly named branch.
use super::*;
use std::collections::BTreeSet;

#[derive(Clone)]
pub(super) struct Source {
    branch: String,
    node: ChosenNode,
    label: String,
}

#[derive(Clone)]
pub(super) struct Selection {
    pub(super) sources: BTreeMap<String, Source>,
    name: String,
}
impl Selection {
    pub(super) fn inventory(&self, draft: &Draft) -> ReviewInventory {
        ReviewInventory {
            instance: draft.instance,
            node_ids: self
                .sources
                .keys()
                .map(String::as_str)
                .chain(draft.chosen.iter().map(|node| node.id.as_str()))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .map(str::to_owned)
                .collect(),
            branch_ids: self
                .sources
                .values()
                .map(|source| source.branch.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        }
    }

    pub(super) fn validate_nodes(&self, nodes: &[ChosenNode]) -> Result<(), String> {
        let nodes = nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect::<BTreeMap<_, _>>();
        for source in self.sources.values() {
            if nodes.get(source.node.id.as_str()).copied() != Some(&source.node) {
                return Err(missing_node());
            }
        }
        Ok(())
    }

    pub(super) fn payload(&self, draft: &Draft) -> Value {
        let inputs = self.sources.values().map(|source| json!({
            "branch_id":source.branch,"source_node_id":source.node.id,
            "expected_node_id":draft.selected.get(&source.branch).map(|branch|branch.expected.as_str())
        })).collect::<Vec<_>>();
        json!({"inputs":inputs,"chosen_node_id":draft.chosen.as_ref().map(|node|node.id.as_str()),
            "name":self.name,"repository_id":draft.repository,"path":draft.path,"confirmed":true})
    }

    pub(super) fn ready(&self, draft: &Draft) -> bool {
        self.sources.len() >= 2
            && draft.chosen.is_some()
            && self.bases_ready(draft)
            && storage_contract::validate_branch_name(&self.name).is_ok()
    }

    fn bases_ready(&self, draft: &Draft) -> bool {
        self.sources
            .values()
            .all(|source| draft.selected.contains_key(&source.branch))
    }

    pub(super) fn validate(&self, draft: &Draft) -> Result<(), String> {
        if self.sources.len() < 2 {
            return Err(t(
                "请在历史图中选择至少两个不同节点。",
                "Choose at least two different history nodes.",
            ));
        }
        if self
            .sources
            .values()
            .any(|source| !draft.selected.contains_key(&source.branch))
        {
            return Err(t(
                "正在读取所选节点的分支依据，请稍候。",
                "The selected nodes' branch bases are still loading.",
            ));
        }
        if draft.chosen.is_none() {
            return Err(t(
                "请点击要作为合并结果的节点。",
                "Choose the node to adopt as the merge result.",
            ));
        }
        storage_contract::BranchName::parse(&self.name).map_err(|source| source.to_string())?;
        Ok(())
    }
}

pub(crate) fn start_history(state: RwSignal<State>) {
    if !state.with_untracked(actions::history_write_allowed) {
        return;
    }
    super::start(state);
    state.update(|s| {
        if let Some(draft) = s.files.merge.as_mut() {
            draft.selected.clear();
            draft.historical = Some(Selection {
                sources: BTreeMap::new(),
                name: format!("{} {}", t("合并结果", "Merged history"), draft.instance),
            });
        }
    });
}

pub(crate) fn start_from_node(state: RwSignal<State>, id: String) {
    start_history(state);
    if let Some(instance) =
        state.with_untracked(|s| s.files.merge.as_ref().map(|draft| draft.instance))
    {
        select_source(state, instance, &id);
    }
}

fn change_source(state: &mut State, instance: u64, id: &str) -> Result<bool, String> {
    if !actions::history_write_allowed(state) {
        return Ok(false);
    }
    let Some(draft) = state
        .files
        .merge
        .as_ref()
        .filter(|draft| draft.instance == instance)
    else {
        return Ok(false);
    };
    if draft.historical.is_none() {
        return Ok(false);
    }
    if state.files.file_history.root() != Some(draft.root()) {
        return Err(review_required());
    }
    let mut candidate = draft.clone();
    let selection = candidate
        .historical
        .as_mut()
        .expect("invariant: graph draft has source selection");
    if selection.sources.remove(id).is_none() {
        let node = state.files.file_history.node(id).ok_or_else(missing_node)?;
        let branch = node
            .branch_id()
            .map(str::to_owned)
            .ok_or_else(unavailable_branch)?;
        let captured = ChosenNode::capture(node).ok_or_else(missing_node)?;
        if let Some(record) = state
            .files
            .file_history
            .branches()
            .iter()
            .chain(state.files.file_history.current_branch())
            .find(|record| record.id() == branch)
            .and_then(SelectedBranch::capture_reference)
        {
            candidate.selected.entry(branch.clone()).or_insert(record);
        }
        selection.sources.insert(
            id.to_owned(),
            Source {
                branch,
                node: captured,
                label: node_label(node, 0),
            },
        );
    }
    candidate.selected.retain(|branch, _| {
        selection
            .sources
            .values()
            .any(|source| &source.branch == branch)
    });
    check_selection(
        &payload(&candidate),
        candidate.input_count(),
        candidate.max_entries,
    )
    .map_err(|error| error.to_string())?;
    candidate.resolved = None;
    let needs_read = candidate
        .historical
        .as_ref()
        .expect("invariant: graph draft retains selection")
        .sources
        .values()
        .any(|source| !candidate.selected.contains_key(&source.branch));
    state.files.merge = Some(candidate);
    state.files.history_aux.cancel_review(ReviewOwner {
        kind: review::Kind::Merge,
        instance,
    });
    Ok(needs_read)
}

/// The proof was admitted against this draft's exact root. It may complete
/// after the node page changes, so retain the immutable clicked facts directly.
pub(super) fn insert_resolved_source(
    state: &mut State,
    instance: u64,
    node: ChosenNode,
    label: String,
    branch: SelectedBranch,
) -> Result<(), String> {
    let Some(draft) = state
        .files
        .merge
        .as_ref()
        .filter(|draft| draft.instance == instance && draft.historical())
    else {
        return Ok(());
    };
    let mut candidate = draft.clone();
    let selection = candidate
        .historical
        .as_mut()
        .expect("invariant: source resolution owns a history merge");
    selection.sources.insert(
        node.id.clone(),
        Source {
            branch: branch.id.clone(),
            node,
            label,
        },
    );
    if let Some(previous) = candidate.selected.get(&branch.id)
        && previous.expected != branch.expected
    {
        return Err(review_required());
    }
    candidate.selected.insert(branch.id.clone(), branch);
    check_selection(
        &payload(&candidate),
        candidate.input_count(),
        candidate.max_entries,
    )
    .map_err(|error| error.to_string())?;
    candidate.resolved = None;
    state.files.merge = Some(candidate);
    Ok(())
}

fn select_source(state: RwSignal<State>, instance: u64, id: &str) {
    if state.with_untracked(actions::history_aux_loading) {
        return;
    }
    let unresolved = state.with_untracked(|s| {
        s.files
            .merge
            .as_ref()
            .and_then(|draft| draft.historical.as_ref())
            .is_some_and(|selection| !selection.sources.contains_key(id))
            && s.files
                .file_history
                .node(id)
                .is_some_and(|node| node.branch_id().is_none())
    });
    if unresolved {
        super::source::resolve(state, instance, id);
        return;
    }
    match state.try_update(|s| change_source(s, instance, id)) {
        Some(Ok(true)) => actions::review_history_merge(state, instance),
        Some(Err(error)) => review::error(state, review::Kind::Merge, error),
        _ => {}
    }
}

pub(crate) fn node_control(state: RwSignal<State>, id: String) -> impl IntoView {
    let id = StoredValue::new(id);
    let active = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .merge
                .as_ref()
                .filter(|draft| draft.historical())
                .map(|draft| draft.instance)
        })
    });
    let disabled = Signal::derive(move || {
        state.with(|s| !actions::history_write_allowed(s) || actions::history_aux_loading(s))
    });
    let records = super::super::history_index::shared();
    let result_available = Memo::new(move |_| {
        records.with(|index| {
            index
                .node(&id.get_value())
                .is_some_and(|(_, node)| super::result_available(node))
        })
    });
    view! {
        <Show when=move ||active.get().is_none()>
            <button class="zxs-button" disabled=move ||disabled.get() on:click=move |_|start_from_node(state,id.get_value())>{t("从此节点合并","Merge from this node")}</button>
        </Show>
        <For each={move ||active.get().into_iter().collect::<Vec<_>>()} key=|instance|*instance children=move |instance|view!{
            <button class="zxs-button" data-history-merge-input=id.get_value() aria-pressed=move ||state.with(|s|s.files.merge.as_ref().and_then(|draft|draft.historical.as_ref()).is_some_and(|selection|selection.sources.contains_key(&id.get_value()))).to_string()
                disabled=move ||disabled.get() on:click=move |_|select_source(state,instance,&id.get_value())>{move ||state.with(|s|if s.files.merge.as_ref().and_then(|draft|draft.historical.as_ref()).is_some_and(|selection|selection.sources.contains_key(&id.get_value())){t("已选为合并输入","Selected merge input")}else{t("选择为合并输入","Select merge input")})}</button>
            <button class="zxs-button" data-history-merge-result=id.get_value() aria-pressed=move ||state.with(|s|s.files.merge.as_ref().and_then(|draft|draft.chosen.as_ref()).is_some_and(|node|node.id==id.get_value())).to_string()
                disabled=move ||disabled.get()||!result_available.get()||state.with(|s|s.files.merge.as_ref().is_none_or(|draft|draft.input_count()<2||draft.historical.as_ref().is_none_or(|selection|!selection.bases_ready(draft))))
                on:click=move |_|choose_node(state,instance,id.get_value())>{t("采用此节点作为结果","Use this node as result")}</button>
        }/>
    }
}

pub(super) fn view(state: RwSignal<State>, instance: u64) -> impl IntoView {
    let disabled = Signal::derive(move || {
        state.with(|s| !actions::history_write_allowed(s) || actions::history_aux_loading(s))
    });
    let rows = Memo::new(move |_| {
        state.with(|s| {
            s.files
                .merge
                .as_ref()
                .and_then(|draft| {
                    draft.historical.as_ref().map(|selection| {
                        selection
                            .sources
                            .values()
                            .skip(draft.selected_page * super::view::SELECTED_ROWS)
                            .take(super::view::SELECTED_ROWS)
                            .map(|source| {
                                (
                                    source.node.id.clone(),
                                    source.label.clone(),
                                    draft
                                        .selected
                                        .get(&source.branch)
                                        .map(|branch| branch.name.clone()),
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .unwrap_or_default()
        })
    });
    view! {
        <section class="zxs-stack" data-history-merge-steps="">
            <p class="zxs-row">{t("1. 在下方历史图中选择至少两个节点。2. 点击要采用的结果节点。3. 确认新分支名称并合并。换页保留选择。","1. Select at least two nodes in the history graph below. 2. Pick the result node. 3. Confirm the new branch name and merge. Selections survive page changes.")}</p>
            <p class="zxs-row">{t("从历史节点建立新的合并分支，原分支、较新版本和默认视图不变。结果采用所选节点的正文和标签，不自动拼接内容。","This creates a new merged branch from historical nodes. Original branches, newer versions and the default view stay unchanged. The result adopts the selected node's content and tags; contents are not combined automatically.")}</p>
            <For each=move ||rows.get() key=|(id,_,_)|id.clone() children=move |(id,label,branch)|{
                let locate=id.clone();let remove=id.clone();
                view!{<div class="zxs-file-toolbar zxs-flow"><span class="zxs-row">{format!("{} · {label}",branch.unwrap_or_else(||t("分支依据待读取","Branch base not yet read")))}</span>
                    <button class="zxs-button" disabled=move ||state.with(|s|!actions::history_navigation_ready(s)) on:click=move |_|actions::anchor_history_node(state,locate.clone())>{t("查看节点","Show node")}</button>
                    <button class="zxs-button" disabled=move ||disabled.get() on:click=move |_|select_source(state,instance,&remove)>{t("移除选择","Remove selection")}</button>
                </div>}
            }/>
            <p class="zxs-row" role="status">{move ||state.with(|s|s.files.merge.as_ref().map(|draft|format!("{} {} · {}",draft.input_count(),t("个输入节点","input nodes"),if draft.chosen.is_some(){t("结果节点已选择","Result selected")}else{t("尚未选择结果节点","Choose a result node")})).unwrap_or_default())}</p>
            <div class="zxs-file-toolbar zxs-flow">
                <button class="zxs-button" disabled=move ||state.with(|s|s.files.merge.as_ref().is_none_or(|draft|draft.selected_page==0)) on:click=move |_|super::view::move_selected_page(state,instance,false)>{t("上一页所选项","Previous selected items")}</button>
                <button class="zxs-button" disabled=move ||state.with(|s|s.files.merge.as_ref().is_none_or(|draft|(draft.selected_page+1)*super::view::SELECTED_ROWS>=draft.input_count())) on:click=move |_|super::view::move_selected_page(state,instance,true)>{t("下一页所选项","Next selected items")}</button>
                <Show when=move ||state.with(|s|s.files.merge.as_ref().and_then(|draft|draft.chosen.as_ref()).is_some())><button class="zxs-button" disabled=move ||state.with(|s|!actions::history_navigation_ready(s)) on:click=move |_|if let Some(id)=state.with_untracked(|s|s.files.merge.as_ref().and_then(|draft|draft.chosen.as_ref()).map(|node|node.id.clone())){actions::anchor_history_node(state,id);}>{t("查看所选结果节点","Show chosen result node")}</button></Show>
            </div>
            <label class="zxs-form-field"><span class="zxs-row">{t("新合并分支名称","New merged branch name")}</span><input class="zxs-input zxs-input--compact" aria-label=t("新合并分支名称","New merged branch name") maxlength=128 disabled=move ||disabled.get() prop:value=move ||state.with(|s|s.files.merge.as_ref().and_then(|draft|draft.historical.as_ref()).map(|selection|selection.name.clone()).unwrap_or_default()) on:input=move |event|state.update(|s|{if !actions::history_write_allowed(s){return;}if let Some(draft)=s.files.merge.as_mut().filter(|draft|draft.instance==instance)&&let Some(selection)=draft.historical.as_mut(){selection.name=event_target_value(&event);draft.resolved=None;}})/></label>
            <button class="zxs-button" disabled=move ||disabled.get() on:click=move |_|super::start(state)>{t("改为合并当前分支并结束其他参与分支","Instead merge current branches and close the other inputs")}</button>
        </section>
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/merge/graph_tests.rs"]
mod tests;
