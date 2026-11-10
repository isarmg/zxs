use super::*;
use crate::files::node_card::{self, Action};
use std::collections::BTreeMap;
use storage_contract::history_window::HistoryEndpoint;
use web_interface::i18n::t;

mod branches;

pub(in crate::files) fn view(state: RwSignal<State>) -> impl IntoView {
    let Some(target) = state.with_untracked(Target::capture) else {
        return view! { <section class="zxs-entry-section zxs-directory-history zxs-stack"><h3 class="zxs-row">{t("目录分支树","Folder branch tree")}</h3><p class="zxs-row">{t("此目录是磁盘或备份的汇总视图，没有独立的目录历史。请打开原始仓库中的具体文件夹查看。","This is a disk or backup aggregate, with no independent folder history. Open the original folder in its repository to view its history.")}</p></section> }.into_any();
    };
    graph(state, target).into_any()
}

pub(in crate::files) fn current_directory_view(
    state: RwSignal<State>,
    target: Memo<Option<Target>>,
) -> impl IntoView {
    view! {
        <For
            each=move || target.get()
            key=|target| target.clone()
            children=move |target| graph(state, target)
        />
    }
}

fn graph(state: RwSignal<State>, target: Target) -> impl IntoView {
    let target = StoredValue::new(target);
    let model = RwSignal::new(Model::default());
    on_cleanup(move || {
        let _ = model.try_update(Model::cancel);
    });
    let listing = Memo::new(move |_| {
        state.with(|state| {
            (
                state.session_epoch,
                state.files.generation,
                state.files.loading,
            )
        })
    });
    Effect::new(move |_| {
        let (_, _, loading) = listing.get();
        if !loading {
            load(state, model, target, Read::Refresh);
        }
    });
    let disabled = Signal::derive(move || {
        model.with(|model| model.loading || model.stale || model.nodes.is_none())
            || !state.with(|state| target.with_value(|target| target.current(state)))
    });
    let nodes = Memo::new(move |_| {
        model.with(|model| {
            model
                .nodes
                .as_ref()
                .map(|page| {
                    page.nodes
                        .iter()
                        .cloned()
                        .map(|node| (node.tree_id.clone(), node))
                        .collect::<BTreeMap<_, _>>()
                })
                .unwrap_or_default()
        })
    });
    let heads = Memo::new(move |_| {
        model.with(|model| {
            let mut heads = BTreeMap::<String, Vec<String>>::new();
            if let Some(page) = &model.branches {
                for branch in &page.branches {
                    let Some(head) = branch.committed_tree_id.clone() else {
                        continue;
                    };
                    heads.entry(head).or_default().push(if branch.materialized {
                        branch.name.clone()
                    } else {
                        format!(
                            "{} · {}",
                            branch.name,
                            t("尚未写入，引用父节点", "Unwritten, references parent node")
                        )
                    });
                }
            }
            heads
        })
    });
    let layout = Memo::new(move |_| {
        nodes.with(|nodes| {
            nodes
                .values()
                .map(|node| {
                    (
                        node.tree_id.clone(),
                        node.sequence,
                        if node.parent_endpoint == Some(HistoryEndpoint::InWindow) {
                            node.parent_tree_id.iter().cloned().collect()
                        } else {
                            Vec::new()
                        },
                    )
                })
                .collect::<Vec<_>>()
        })
    });
    view! {
        <section class="zxs-entry-section zxs-directory-history zxs-stack" aria-busy=move ||model.with(|model|model.loading).to_string()>
            <div class="zxs-file-toolbar zxs-flow"><h3 class="zxs-row">{t("目录分支树","Folder branch tree")}</h3>
                <button class="zxs-button" disabled=move ||model.with(|model|model.loading) on:click=move |_|load(state,model,target,Read::Refresh)>{t("刷新目录历史","Refresh folder history")}</button>
            </div>
            <p class="zxs-row">{t("每个节点是用户明确提交的本仓库文件及已暂存子仓库版本引用。文件保存只更新持久化工作内容，需明确暂存后再提交；父级不会代子仓库提交。连线连接前一次目录提交，分页只显示已加载的关系。","Each node commits this repository’s files and explicitly staged child-repository version references. File saves only update durable working contents; stage them explicitly before committing. A parent never commits a child. Lines connect previous directory commits; a page shows only loaded relationships.")}</p>
            <Show when=move ||model.with(|model|model.stale)><p class="zxs-row" role="status">{t("目录历史已改变；保留原图供查看，请刷新后继续定位节点。","The folder history changed. The previous graph remains visible; refresh before following nodes.")}</p></Show>
            <Show when=move ||model.with(|model|model.error.is_some())><p class="zxs-row zxs-inline-error" role="status">{move ||model.with(|model|model.error.as_ref().map(ToString::to_string).unwrap_or_default())}</p></Show>
            <Show when=move ||model.with(|model|!model.loading&&model.error.is_none()&&model.nodes.as_ref().is_some_and(|page|page.nodes.is_empty()))><p class="zxs-row" data-directory-history-empty="true">{t("此文件夹尚无目录历史记录。查看此图不会创建分支或写入记录。","This folder has no recorded history yet. Viewing this graph does not create branches or write records.")}</p></Show>
            <div class="zxs-file-toolbar zxs-flow">
                <button class="zxs-button" disabled=move ||disabled.get()||model.with(|model|model.nodes.as_ref().is_none_or(|page|page.page.previous_cursor.is_none())) on:click=move |_|page(state,model,target,false,false)>{t("上一页目录历史","Previous folder history page")}</button>
                <button class="zxs-button" disabled=move ||disabled.get()||model.with(|model|model.nodes.as_ref().is_none_or(|page|page.page.next_cursor.is_none())) on:click=move |_|page(state,model,target,false,true)>{t("下一页目录历史","Next folder history page")}</button>
                <Show when=move ||model.with(|model|model.nodes.as_ref().and_then(|page|page.current.committed_tree_id.as_ref().map(|id|!page.nodes.iter().any(|node|&node.tree_id==id))).unwrap_or(false))><button class="zxs-button" disabled=move ||disabled.get() on:click=move |_|if let Some(id)=model.with_untracked(|model|model.nodes.as_ref().and_then(|page|page.current.committed_tree_id.clone())){load(state,model,target,Read::Nodes{cursor:None,anchor:Some(id)});}>{t("定位默认目录节点","Locate default folder node")}</button></Show>
            </div>
            {super::super::version_tree::ancestry_graph(layout, move |id| {
                let id=StoredValue::new(id);
                let node=Memo::new(move |_|nodes.with(|nodes|id.with_value(|id|nodes.get(id).cloned())));
                let current=Signal::derive(move ||model.with(|model|model.nodes.as_ref().and_then(|page|page.current.committed_tree_id.as_deref())==Some(id.get_value().as_str())));
                let card_title=Signal::derive(move ||node.with(|node|node.as_ref().map(|node|node_card::title(&node.message,node.sequence)).unwrap_or_default()));
                let card_actions=Signal::derive(|| &[Action::Snapshot][..]);
                view!{<article class="zxs-version-node zxs-directory-tree-node" data-version-id=id.get_value() data-directory-tree-id=id.get_value() aria-label=t("目录历史节点","Folder history node")>
                    <details class="zxs-node-disclosure">
                    {node_card::summary(card_title,card_actions,current)}
                    <div class="zxs-content-panel zxs-node-detail zxs-frame zxs-stack zxs-stack--flush">
                    <div class="zxs-version-date zxs-row zxs-row--center"><span>{move ||node.with(|node|node.as_ref().map(|node|format!("{} #{}",t("目录快照","Folder snapshot"),node.sequence)).unwrap_or_default())}</span><Show when=move ||current.get()><span class="zxs-version-current">{t("默认","Default")}</span></Show></div>
                    <p class="zxs-row">{move ||node.with(|node|node.as_ref().map(|node|node.message.clone()).unwrap_or_default())}</p>
                    <p class="zxs-row">{move ||node.with(|node|node.as_ref().map(|node|if node.path.is_empty(){t("仓库根目录","Repository root")}else{node.path.clone()}).unwrap_or_default())}</p>
                    <For each=move ||heads.with(|heads|heads.get(&id.get_value()).cloned().unwrap_or_default()) key=|name|name.clone() children=move |name|view!{<span class="zxs-row zxs-directory-head-label">{name}</span>}/>
                    <button type="button" class="zxs-button" data-directory-snapshot=id.get_value() disabled=move ||disabled.get()||state.with(|state|state.files.loading||super::super::actions::pending(state))
                        on:click=move |_|{
                            if disabled.get_untracked(){return;}
                            if let Some(node)=node.get_untracked(){
                                let path=target.with_value(|target|state.with_untracked(|state|target.current(state)).then(||target.path.clone()));
                                if let Some(path)=path{super::super::actions::preview_directory_snapshot(state,path,node.directory_branch_id,node.tree_id);}
                            }
                        }>{t("浏览此快照","Browse this snapshot")}</button>
                    <Show when=move ||node.with(|node|node.as_ref().is_some_and(|node|node.parent_endpoint==Some(HistoryEndpoint::OutsideWindow)))><button class="zxs-button" data-directory-parent="outside_window" disabled=move ||disabled.get() on:click=move |_|if let Some(parent)=node.with_untracked(|node|node.as_ref().and_then(|node|node.parent_tree_id.clone())){load(state,model,target,Read::Nodes{cursor:None,anchor:Some(parent)});}>{t("查看前一个目录节点","Show previous folder node")}</button></Show>
                    <Show when=move ||node.with(|node|node.as_ref().is_some_and(|node|node.parent_tree_id.is_none()))><span class="zxs-row">{t("无父关系","No parent relationships")}</span></Show>
                    </div>
                    </details>
                </article>}
            })}
            {branches::view(state, model, target, disabled)}
        </section>
    }.into_any()
}
fn page(
    state: RwSignal<State>,
    model: RwSignal<Model>,
    target: StoredValue<Target>,
    branches: bool,
    next: bool,
) {
    let cursor = model.with_untracked(|model| {
        if model.loading || model.stale {
            return None;
        }
        let page = if branches {
            &model.branches
        } else {
            &model.nodes
        }
        .as_ref()?;
        if next {
            page.page.next_cursor.clone()
        } else {
            page.page.previous_cursor.clone()
        }
    });
    if let Some(cursor) = cursor {
        load(
            state,
            model,
            target,
            if branches {
                Read::Branches(cursor)
            } else {
                Read::Nodes {
                    cursor: Some(cursor),
                    anchor: None,
                }
            },
        );
    }
}
