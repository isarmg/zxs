//! Auxiliary history reads replace their single relation page or field fragment.
use super::{FieldFragment, FileHistory};
use crate::{files::{actions, page::Entry}, ui::State};
use std::sync::Arc;
use storage_contract::history_window::{
    HistoryCollection, HistoryField, HistoryPageCursors, HistoryResolvedContext, HistoryWindowPage,
};
use web_interface::{components::{LoadingState, Select, SelectOption}, i18n::t, prelude::*};

/// Event payloads retain one bounded selection identity, never an old history page.
#[derive(Clone, PartialEq)]
struct HistoryOwner {
    epoch: u64,
    slot: String,
    directory: String,
    entry: Entry,
    root: String,
    context: HistoryResolvedContext,
}
impl HistoryOwner {
    fn capture(state: &State) -> Option<Self> {
        if !state.session_active() {
            return None;
        }
        Some(Self {
            epoch: state.session_epoch,
            slot: state.files.slot.clone(),
            directory: state.files.directory.clone(),
            entry: state.files.selected.clone()?,
            root: state.files.file_history.root()?.to_owned(),
            context: state.files.file_history.identity()?.context.clone(),
        })
    }

    fn matches(&self, state: &State) -> bool {
        Self::capture(state).as_ref() == Some(self)
    }

    fn allows(&self, state: &State) -> bool {
        self.matches(state) && !navigation_disabled(state)
    }
}

fn navigation_disabled(state: &State) -> bool {
    !actions::history_navigation_ready(state)
        || actions::history_aux_loading(state)
        || state.files.history_editors != 0
}

fn field_name(field: HistoryField) -> String {
    match field {
        HistoryField::Message => t("提交说明", "Message"),
        HistoryField::Author => t("作者", "Author"),
        HistoryField::CreatedAt => t("记录时间", "Recorded time"),
    }
}

fn field_complete(fragment: &FieldFragment) -> bool {
    fragment.offset == 0 && fragment.end
}

fn field_range(fragment: &FieldFragment) -> String {
    // Admission proves this UTF-8 fragment ends at or before total bytes.
    let end = fragment.offset + fragment.text.len() as u64;
    format!(
        "{} [{}, {}) / {}",
        t("UTF-8 字节", "UTF-8 bytes"),
        fragment.offset,
        end,
        fragment.total,
    )
}

fn field_status(fragment: &FieldFragment) -> String {
    if field_complete(fragment) {
        t("已显示完整字段。", "The complete field is displayed.")
    } else if fragment.end {
        t(
            "这是最后一片；本面板仅保留当前片段。",
            "This is the last fragment; this panel retains only this fragment.",
        )
    } else {
        t(
            "字段尚有后续片段；翻页会替换当前片段。",
            "More fragments remain; the next fragment replaces this one.",
        )
    }
}

#[derive(Clone, PartialEq, Eq)]
struct RelationKey {
    collection: HistoryCollection,
    previous: Option<String>,
    next: Option<String>,
}
impl RelationKey {
    fn new(collection: &HistoryCollection, page: &HistoryPageCursors) -> Self {
        Self {
            collection: collection.clone(),
            previous: page.previous_cursor.as_ref().map(|cursor| cursor.as_str().to_owned()),
            next: page.next_cursor.as_ref().map(|cursor| cursor.as_str().to_owned()),
        }
    }

    fn matches(&self, history: &FileHistory) -> bool {
        history.relation().is_some_and(|page| {
            let (collection, cursors) = relation_metadata(page);
            Self::new(collection, cursors) == *self
        })
    }
}

fn relation_metadata(page: &HistoryWindowPage) -> (&HistoryCollection, &HistoryPageCursors) {
    match page {
        HistoryWindowPage::NodeParents { identity, page, .. }
        | HistoryWindowPage::MergeInputs { identity, page, .. }
        | HistoryWindowPage::MergeParents { identity, page, .. }
        | HistoryWindowPage::VersionSources { identity, page, .. } => (&identity.collection, page),
        HistoryWindowPage::Nodes { .. }
        | HistoryWindowPage::Versions { .. }
        | HistoryWindowPage::Branches { .. }
        | HistoryWindowPage::Merges { .. }
        | HistoryWindowPage::NodeField { .. } => {
            unreachable!("invariant: the relation owner contains only a relation page")
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct RelationRow {
    label: String,
    node: Option<String>,
    outside: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct RelationView {
    key: RelationKey,
    title: String,
    rows: Vec<RelationRow>,
}

fn relation_view(history: &FileHistory) -> Option<RelationView> {
    let page = history.relation()?;
    let (collection, cursors) = relation_metadata(page);
    let (title, rows) = match page {
        HistoryWindowPage::NodeParents { identity, parents, .. } => (
            format!("{} · {}", t("节点父关系", "Node parents"), relation_subject(&identity.collection)),
            parents.iter().map(|parent| {
                let node = parent.parent_node_id.to_string();
                relation_node(history, node.clone(), node)
            }).collect(),
        ),
        HistoryWindowPage::MergeInputs { identity, inputs, .. } => (
            format!("{} · {}", t("合并输入", "Merge inputs"), relation_subject(&identity.collection)),
            input_rows(history, inputs),
        ),
        HistoryWindowPage::MergeParents { identity, parents, .. } => (
            format!("{} · {}", t("保存的合并父关系", "Saved merge parents"), relation_subject(&identity.collection)),
            parents.iter().map(|parent| {
                let node = parent.parent_node_id.to_string();
                relation_node(history, format!("#{} · {}", parent.ordinal, node), node)
            }).collect(),
        ),
        HistoryWindowPage::VersionSources { identity, sources, .. } => (
            format!("{} · {}", t("正文来源版本", "Body source versions"), relation_subject(&identity.collection)),
            sources.iter().map(|source| RelationRow {
                label: format!("#{} · {}", source.ordinal, source.version_id),
                node: None,
                outside: false,
            }).collect(),
        ),
        HistoryWindowPage::Nodes { .. }
        | HistoryWindowPage::Versions { .. }
        | HistoryWindowPage::Branches { .. }
        | HistoryWindowPage::Merges { .. }
        | HistoryWindowPage::NodeField { .. } => {
            unreachable!("invariant: the relation owner contains only a relation page")
        }
    };
    Some(RelationView { key: RelationKey::new(collection, cursors), title, rows })
}

fn relation_node(history: &FileHistory, label: String, node: String) -> RelationRow {
    RelationRow {
        label,
        outside: !history.nodes().iter().any(|loaded| loaded.id() == node),
        node: Some(node),
    }
}

fn input_rows(
    history: &FileHistory,
    inputs: &[storage_contract::history_window::HistoryMergeInputMember],
) -> Vec<RelationRow> {
    inputs.iter().map(|input| {
        let node = input.node_id.to_string();
        relation_node(history, format!("#{} · {} → {}", input.ordinal, input.branch_id, node), node)
    }).collect()
}

fn relation_subject(collection: &HistoryCollection) -> String {
    match collection {
        HistoryCollection::NodeParents { node_id } => node_id.to_string(),
        HistoryCollection::MergeInputs { merge_id } | HistoryCollection::MergeParents { merge_id } => merge_id.to_string(),
        HistoryCollection::VersionSources { version_id } => version_id.to_string(),
        HistoryCollection::Nodes
        | HistoryCollection::Versions
        | HistoryCollection::Branches
        | HistoryCollection::Merges
        | HistoryCollection::NodeField { .. } => {
            unreachable!("invariant: a relation title belongs to a relation collection")
        }
    }
}

fn change_relation(state: RwSignal<State>, owner: &HistoryOwner, key: &RelationKey, next: bool) {
    if !state.with_untracked(|s| owner.allows(s) && key.matches(&s.files.file_history)) {
        return;
    }
    let cursor = if next { key.next.clone() } else { key.previous.clone() };
    let Some(cursor) = cursor else { return; };
    match &key.collection {
        HistoryCollection::NodeParents { node_id } => actions::open_history_parents(state, node_id.to_string(), Some(cursor)),
        HistoryCollection::MergeInputs { merge_id } => actions::open_history_merge_inputs(state, merge_id.to_string(), Some(cursor)),
        HistoryCollection::MergeParents { merge_id } => actions::open_history_merge_parents(state, merge_id.to_string(), Some(cursor)),
        HistoryCollection::VersionSources { version_id } => actions::open_history_version_sources(state, version_id.to_string(), Some(cursor)),
        HistoryCollection::Nodes
        | HistoryCollection::Versions
        | HistoryCollection::Branches
        | HistoryCollection::Merges
        | HistoryCollection::NodeField { .. } => {
            unreachable!("invariant: relation navigation dispatches a relation collection")
        }
    }
}

fn relation_panel(state: RwSignal<State>, owner: Arc<HistoryOwner>, page: RelationView) -> impl IntoView {
    let key = Arc::new(page.key);
    let previous_owner = Arc::clone(&owner);
    let previous_key = Arc::clone(&key);
    let next_owner = Arc::clone(&owner);
    let next_key = Arc::clone(&key);
    let close_owner = Arc::clone(&owner);
    let close_key = Arc::clone(&key);
    let previous = key.previous.is_some();
    let next = key.next.is_some();
    let empty = page.rows.is_empty();
    let rows = page.rows.into_iter().map(move |row| {
        let locate_owner = Arc::clone(&owner);
        let locate_key = Arc::clone(&key);
        view! {<div class="zxs-row zxs-flow">
            <span>{row.label}</span>
            {row.outside.then(||view!{<span>{t("父节点／来源节点尚未加载。","Parent or source node is not loaded.")}</span>})}
            {row.node.map(move |node|view!{<button class="zxs-button" disabled=move ||state.with(navigation_disabled)
                on:click=move |_|if state.with_untracked(|s|locate_owner.allows(s)&&locate_key.matches(&s.files.file_history)){actions::anchor_history_node(state,node.clone());}>{t("定位节点","Locate node")}</button>})}
        </div>}
    }).collect_view();
    view! {<section class="zxs-frame zxs-stack zxs-history-relations" aria-label=t("历史关系窗口","History relation window")>
        <div class="zxs-file-toolbar zxs-flow"><h3 class="zxs-row">{page.title}</h3>
            <button class="zxs-button" disabled=move ||state.with(|s|s.files.history_editors!=0)
                on:click=move |_|if state.with_untracked(|s|close_owner.matches(s)&&close_key.matches(&s.files.file_history)){actions::close_history_relation(state);}>{t("关闭关系窗口","Close relation window")}</button>
        </div>
        {rows}
        {empty.then(||view!{<p class="zxs-row">{t("此关系窗口没有记录。","This relation window has no records.")}</p>})}
        <p class="zxs-row">{t("这里只显示当前窗口；末页不表示本面板保留了之前的全部关系。","Only this window is displayed; reaching the last page does not retain earlier relations.")}</p>
        <div class="zxs-file-toolbar zxs-flow">
            <button class="zxs-button" disabled=move ||!previous||state.with(navigation_disabled) on:click=move |_|change_relation(state,&previous_owner,&previous_key,false)>{t("上一页关系","Previous relation page")}</button>
            <button class="zxs-button" disabled=move ||!next||state.with(navigation_disabled) on:click=move |_|change_relation(state,&next_owner,&next_key,true)>{t("下一页关系","Next relation page")}</button>
        </div>
    </section>}
}

fn load_selected_field(state: RwSignal<State>, owner: &HistoryOwner, selected: RwSignal<String>, field: HistoryField) {
    let selected = selected.get_untracked();
    let node = state.with_untracked(|s| {
        owner.allows(s).then(|| s.files.file_history.nodes().iter()
            .find(|node| node.id() == selected).map(|node|node.id().to_owned())).flatten()
    });
    if let Some(node) = node { actions::load_history_field(state,node,field,None); }
}

fn field_controls(state: RwSignal<State>, owner: Arc<HistoryOwner>) -> impl IntoView {
    let selected = RwSignal::new(String::new());
    let options = Signal::derive(move ||state.with(|s|s.files.file_history.nodes().iter()
        .map(|node|SelectOption::new(node.id(),format!("#{} · {}",node.sequence(),node.id()))).collect()));
    let selected_value = Signal::derive(move ||selected.get());
    let disabled = Signal::derive(move ||state.with(navigation_disabled));
    let missing = Signal::derive(move || {
        let selected = selected.get();
        state.with(|s|!s.files.file_history.nodes().iter().any(|node|node.id()==selected))
    });
    let missing_body = Signal::derive(move || {
        let selected = selected.get();
        state.with(|s|s.files.file_history.nodes().iter().find(|node|node.id()==selected)
            .is_none_or(|node|node.version_id().is_none()))
    });
    let fields = [HistoryField::Message,HistoryField::Author,HistoryField::CreatedAt].into_iter().map(|field| {
        let owner = Arc::clone(&owner);
        view!{<button class="zxs-button" disabled=move ||disabled.get()||missing.get()
            on:click=move |_|load_selected_field(state,&owner,selected,field)>{field_name(field)}</button>}
    }).collect_view();
    let source_owner = Arc::clone(&owner);
    let selected_owner = Arc::clone(&owner);
    view! {<section class="zxs-stack zxs-stack--flush" aria-label=t("历史字段与正文来源","History fields and body sources")>
        <div class="zxs-file-toolbar zxs-flow">
            <Select label=t("本页节点","Node on this page") options=options value=selected_value disabled=disabled
                placeholder=t("选择要读取的节点","Choose a node to read") on_change=Callback::new(move |id:String|{
                    if state.with_untracked(|s|selected_owner.allows(s)&&s.files.file_history.nodes().iter().any(|node|node.id()==id)){selected.set(id);}
                })/>
            {fields}
            <button class="zxs-button" disabled=move ||disabled.get()||missing_body.get() on:click=move |_|{
                let selected=selected.get_untracked();
                let version=state.with_untracked(|s|source_owner.allows(s).then(||s.files.file_history.nodes().iter()
                    .find(|node|node.id()==selected).and_then(|node|node.version_id()).map(str::to_owned)).flatten());
                if let Some(version)=version{actions::open_history_version_sources(state,version,None);}
            }>{t("读取正文来源","Load body sources")}</button>
        </div>
    </section>}
}

#[derive(Clone, PartialEq, Eq)]
struct FieldKey {
    node: String,
    field: HistoryField,
    offset: u64,
    next: Option<String>,
}
impl FieldKey {
    fn new(fragment: &FieldFragment) -> Self {
        Self { node: fragment.node.clone(), field: fragment.field,
            offset: fragment.offset, next: fragment.next.clone() }
    }

    fn matches(&self, history: &FileHistory) -> bool {
        history.fragment().is_some_and(|fragment| Self::new(fragment) == *self)
    }
}

fn field_fragment_panel(
    state: RwSignal<State>,
    owner: Arc<HistoryOwner>,
    key: FieldKey,
    history: Memo<crate::snapshot::Snapshot<FileHistory>>,
) -> impl IntoView {
    let key = Arc::new(key);
    let next_owner = Arc::clone(&owner);
    let restart_owner = Arc::clone(&owner);
    let next_key = Arc::clone(&key);
    let restart_key = Arc::clone(&key);
    view! {
        <section class="zxs-frame zxs-stack zxs-history-field" aria-label=t("历史字段片段","History field fragment")>
            <div class="zxs-file-toolbar zxs-flow"><h3 class="zxs-row">{move ||history.with(|h|h.fragment().map(|f|format!("{} · {}",field_name(f.field),f.node)).unwrap_or_default())}</h3>
                <button class="zxs-button" disabled=move ||state.with(|s|s.files.history_editors!=0) on:click=move |_|if state.with_untracked(|s|owner.matches(s)&&key.matches(&s.files.file_history)){actions::close_history_field(state);}>{t("关闭字段","Close field")}</button>
            </div>
            <p class="zxs-row">{move ||history.with(|h|h.fragment().map(field_range).unwrap_or_default())}</p>
            <p class="zxs-row" role="status" data-field-complete=move ||history.with(|h|h.fragment().is_some_and(field_complete)).to_string()>{move ||history.with(|h|h.fragment().map(field_status).unwrap_or_default())}</p>
            <pre>{move ||history.with(|h|h.fragment().map(|f|f.text.clone()).unwrap_or_default())}</pre>
            <Show when=move ||history.with(|h|h.fragment().is_some_and(|f|f.total==0))><p class="zxs-row">{t("字段为空。","The field is empty.")}</p></Show>
            <div class="zxs-file-toolbar zxs-flow">
                <button class="zxs-button" disabled=move ||state.with(navigation_disabled)||history.with(|h|h.fragment().is_none_or(|f|f.offset==0)) on:click=move |_|{
                    if state.with_untracked(|s|restart_owner.allows(s)&&restart_key.matches(&s.files.file_history)){
                        actions::load_history_field(state,restart_key.node.clone(),restart_key.field,None);
                    }
                }>{t("从字段开头读取","Read from field start")}</button>
                <button class="zxs-button" disabled=move ||state.with(navigation_disabled)||history.with(|h|h.fragment().is_none_or(|f|f.next.is_none())) on:click=move |_|{
                    if state.with_untracked(|s|next_owner.allows(s)&&next_key.matches(&s.files.file_history))
                        &&let Some(cursor)=next_key.next.clone(){
                        actions::load_history_field(state,next_key.node.clone(),next_key.field,Some(cursor));
                    }
                }>{t("下一片字段","Next field fragment")}</button>
            </div>
        </section>
    }
}

fn field_panel(state: RwSignal<State>, owner: Arc<HistoryOwner>) -> impl IntoView {
    let history = Memo::new(move |_|state.with(|s|s.files.file_history.clone()));
    let key = Memo::new(move |_|history.with(|h|h.fragment().map(FieldKey::new)));
    view! {{move ||key.get().map(|key|field_fragment_panel(state,Arc::clone(&owner),key,history))}}
}

pub(crate) fn auxiliary_view(state: RwSignal<State>) -> impl IntoView {
    let owner = Memo::new(move |_|state.with(HistoryOwner::capture));
    let history = Memo::new(move |_|state.with(|s|s.files.file_history.clone()));
    let relation = Memo::new(move |_|history.with(|h|relation_view(h)));
    view! {{move ||owner.get().map(|owner|{
        let owner=Arc::new(owner);
        let relation_owner=Arc::clone(&owner);
        view!{<div class="zxs-stack zxs-stack--flush">
            <Show when=move ||state.with(actions::history_aux_loading)><LoadingState/></Show>
            {field_controls(state,Arc::clone(&owner))}
            {move ||relation.get().map(|page|relation_panel(state,Arc::clone(&relation_owner),page))}
            {field_panel(state,owner)}
        </div>}
    })}}
}

pub(crate) fn provenance_view(state: RwSignal<State>) -> impl IntoView {
    let owner = Memo::new(move |_|state.with(HistoryOwner::capture));
    view! {{move ||owner.get().map(|owner|{
        let owner=Arc::new(owner);
        let load_owner=Arc::clone(&owner);
        let previous_owner=Arc::clone(&owner);
        let next_owner=Arc::clone(&owner);
        view!{<section class="zxs-stack zxs-merge-history" aria-label=t("合并来源记录","Merge provenance")>
            <div class="zxs-file-toolbar zxs-flow"><h3 class="zxs-row">{t("合并来源记录","Merge provenance")}</h3>
                <button class="zxs-button" disabled=move ||state.with(navigation_disabled) on:click=move |_|if state.with_untracked(|s|load_owner.allows(s)){actions::load_history_merges(state,None);}>{t("读取合并记录首页","Load first merge page")}</button>
                <button class="zxs-button" disabled=move ||state.with(navigation_disabled)||state.with(|s|s.files.file_history.merge_cursors().is_none_or(|p|p.previous_cursor.is_none())) on:click=move |_|{
                    let cursor=state.with_untracked(|s|previous_owner.allows(s).then(||s.files.file_history.merge_cursors().and_then(|p|p.previous_cursor.as_ref()).map(|c|c.as_str().to_owned())).flatten());
                    if let Some(cursor)=cursor{actions::load_history_merges(state,Some(cursor));}
                }>{t("上一页合并记录","Previous merge page")}</button>
                <button class="zxs-button" disabled=move ||state.with(navigation_disabled)||state.with(|s|s.files.file_history.merge_cursors().is_none_or(|p|p.next_cursor.is_none())) on:click=move |_|{
                    let cursor=state.with_untracked(|s|next_owner.allows(s).then(||s.files.file_history.merge_cursors().and_then(|p|p.next_cursor.as_ref()).map(|c|c.as_str().to_owned())).flatten());
                    if let Some(cursor)=cursor{actions::load_history_merges(state,Some(cursor));}
                }>{t("下一页合并记录","Next merge page")}</button>
            </div>
            <For each=move ||state.with(|s|s.files.file_history.merges().to_vec()) key=|record|record.merge_id.clone() children=move |record|{
                let merge=record.merge_id.to_string();
                let input_merge=merge.clone();let input_owner=Arc::clone(&owner);
                let parent_merge=merge.clone();let parent_owner=Arc::clone(&owner);
                let chosen_merge=merge.clone();let chosen_owner=Arc::clone(&owner);
                let result_merge=merge;let result_owner=Arc::clone(&owner);
                let chosen=record.chosen_node_id.to_string();
                let result=record.result_node_id.to_string();
                view!{<article class="zxs-frame zxs-stack">
                    <p class="zxs-row">{format!("#{} · {}",record.sequence,record.merge_id)}</p>
                    <p class="zxs-row">{format!("{}: {} → {}",t("采用节点及结果","Chosen node and result"),record.chosen_node_id,record.result_node_id)}</p>
                    <p class="zxs-row">{format!("{}: {}",t("保留分支","Surviving branch"),record.surviving_branch_id)}</p>
                    <p class="zxs-row">{if record.reused_node{t("复用已有节点，来源另行读取。","Reused an existing node; sources are read separately.")}else{t("创建了复用正文的合并节点。","Created a merge node reusing body content.")}}</p>
                    <div class="zxs-file-toolbar zxs-flow">
                        <button class="zxs-button" disabled=move ||state.with(navigation_disabled) on:click=move |_|if state.with_untracked(|s|input_owner.allows(s)&&s.files.file_history.merges().iter().any(|m|m.merge_id.as_str()==input_merge)){actions::open_history_merge_inputs(state,input_merge.clone(),None);}>{format!("{} ({})",t("读取输入","Load inputs"),record.input_count)}</button>
                        <button class="zxs-button" disabled=move ||state.with(navigation_disabled) on:click=move |_|if state.with_untracked(|s|parent_owner.allows(s)&&s.files.file_history.merges().iter().any(|m|m.merge_id.as_str()==parent_merge)){actions::open_history_merge_parents(state,parent_merge.clone(),None);}>{format!("{} ({})",t("读取保存的父关系","Read saved parents"),record.parent_count)}</button>
                        <button class="zxs-button" disabled=move ||state.with(navigation_disabled) on:click=move |_|if state.with_untracked(|s|chosen_owner.allows(s)&&s.files.file_history.merges().iter().any(|m|m.merge_id.as_str()==chosen_merge)){actions::anchor_history_node(state,chosen.clone());}>{t("定位采用节点","Locate chosen node")}</button>
                        <button class="zxs-button" disabled=move ||state.with(navigation_disabled) on:click=move |_|if state.with_untracked(|s|result_owner.allows(s)&&s.files.file_history.merges().iter().any(|m|m.merge_id.as_str()==result_merge)){actions::anchor_history_node(state,result.clone());}>{t("定位结果节点","Locate result node")}</button>
                    </div>
                </article>}
            }/>
            <Show when=move ||state.with(|s|s.files.file_history.merge_cursors().is_some()&&s.files.file_history.merges().is_empty())><p class="zxs-row">{t("此窗口没有合并记录。","This window has no merge records.")}</p></Show>
        </section>}
    })}}
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/history/view_tests.rs"]
mod tests;
