//! CSS grid owns node and connector tracks, including wrapped text and resizing.
use super::{Source, Topology};
use crate::{admin::storage, ui::State};
use std::sync::Arc;
use storage_contract::storage_directories::DiscoveredDisk;
use web_interface::{i18n::t, prelude::*};

fn source_label(source: &Option<Source>) -> String {
    match source {
        None => t("原始副本", "Original copy"),
        Some(Source::Registered(slot)) => format!("{}: {slot}", t("来源", "Source")),
        Some(Source::Missing(slot)) => {
            format!("{}: {slot}", t("来源未登记", "Source not registered"))
        }
    }
}

fn node(state: RwSignal<State>, topology: Memo<Arc<Topology>>, slot: String) -> impl IntoView {
    let slot = StoredValue::new(slot);
    let record =
        Memo::new(move |_| topology.with(|tree| tree.nodes.get(&slot.get_value()).cloned()));
    view! {
        <button type="button" class="zxs-frame zxs-stack zxs-disk-tree-node"
            data-slot=slot.get_value()
            data-level=move ||record.with(|node|node.as_ref().map(|node|node.level.to_string()))
            style:grid-row=move ||record.with(|node|node.as_ref().map(|node|node.row.to_string()))
            style:grid-column=move ||record.with(|node|node.as_ref().map(|node|node.column.to_string()))
            aria-controls="zxs-selected-disk-details"
            aria-pressed=move ||state.with(|s|s.admin_console.disks.selected.as_deref()==Some(slot.get_value().as_str())).to_string()
            on:click=move |_|super::super::select(state,&slot.get_value())>
            <span class="zxs-row zxs-disk-tree-identity"><strong>{slot.get_value()}</strong>
                <span class="zxs-status">{move ||record.with(|node|node.as_ref().map(|node|if node.online{t("在线","Online")}else{t("离线","Offline")}))}</span>
            </span>
            <span class="zxs-row" data-disk-tier-label="">{move ||record.with(|node|node.as_ref().map(|node|node.tier_label()))}</span>
            <span class="zxs-row zxs-status">{move ||record.with(|node|node.as_ref().map(|node|source_label(&node.source)))}</span>
            <Show when=move ||state.with(|s|s.admin_console.disks.cards.get(&slot.get_value()).is_some_and(|card|card.draft.is_some()))>
                <span class="zxs-row">{t("备注编辑中","Editing note")}</span>
            </Show>
        </button>
    }
}

fn candidate_node(
    state: RwSignal<State>,
    topology: Memo<Arc<Topology>>,
    candidates: Memo<Vec<DiscoveredDisk>>,
    path: String,
) -> impl IntoView {
    let path = StoredValue::new(path);
    let record = Memo::new(move |_| {
        candidates.with(|candidates| {
            candidates
                .iter()
                .enumerate()
                .find(|(_, candidate)| candidate.path == path.get_value())
                .map(|(index, candidate)| (index + 1, candidate.clone()))
        })
    });
    view! {
        <button type="button" class="zxs-frame zxs-stack zxs-disk-tree-node zxs-disk-tree-candidate"
            data-storage-candidate=path.get_value() data-level="-1"
            style:grid-row=move ||topology.with(|tree|tree.pending_row().to_string())
            style:grid-column=move ||record.with(|record|record.as_ref().map(|(column,_)|column.to_string()))
            disabled=move ||state.with(|s|!storage::can_choose_disk(s))
            aria-controls="zxs-disk-wizard"
            aria-pressed=move ||state.with(|s|storage::chosen_disk(s,&path.get_value())).to_string()
            on:click=move |_|storage::choose_disk(state,&path.get_value())>
            <span class="zxs-row zxs-disk-tree-identity"><strong>{move ||record.with(|record|record.as_ref().map(|(_,candidate)|candidate.name.clone()))}</strong>
                <span class="zxs-status">{t("未接纳", "Not admitted")}</span>
            </span>
            <span class="zxs-row">{t("候选磁盘 · -1 层", "Candidate disk · Tier -1")}</span>
            <span class="zxs-row zxs-status">{move ||record.with(|record|record.as_ref().map(|(_,candidate)|storage::candidate_kind(&candidate.state)))}</span>
            <span class="zxs-row">{t("打开磁盘向导", "Open disk wizard")}</span>
        </button>
    }
}

pub(in super::super) fn view(state: RwSignal<State>) -> impl IntoView {
    let topology = Memo::new(move |_| state.with(|s| Arc::clone(&s.admin_console.disks.topology)));
    let candidates = Memo::new(move |_| state.with(storage::disk_candidates));
    view! {
        <Show when=move ||topology.with(|tree|!tree.nodes.is_empty())||candidates.with(|candidates|!candidates.is_empty())>
            <p class="zxs-row">{t(
                "第 1 层主盘在下，第 2 层及更高层备份盘依次向上，层级沿用实际登记值。箭头从来源盘指向上一层备份盘；层级表示配置的复制顺序，不代表文件已有的副本数或独立物理磁盘。选择磁盘查看详情与操作。未接纳候选位于最下方 -1 层，无备份箭头，点击进入磁盘向导。",
                "Tier 1 primaries are at the bottom; tier 2 and higher backup disks are above, using their registered tier numbers. Arrows point from the source to its next-tier backup. Tiers show the configured copy order, not a file's actual copy count or independent physical disks. Select a disk for details and actions. Unadmitted candidates occupy tier -1 at the bottom, have no backup arrows, and open the disk wizard.",
            )}</p>
            <div class="zxs-disk-tree-scroll" role="region" aria-label=t("备份磁盘树","Backup disk tree") tabindex="0">
                <div class="zxs-disk-tree-canvas"
                    style=("--zxs-backup-columns",move ||topology.with(|tree|tree.columns_with_pending(candidates.with(|candidates|candidates.len())).to_string()))
                    style:grid-template-rows=move ||topology.with(|tree|tree.rows_with_pending(candidates.with(|candidates|!candidates.is_empty())))>
                    <For each=move ||topology.with(|tree|tree.edges.keys().cloned().collect::<Vec<_>>())
                        key=|edge|edge.clone() children=move |(source,target)| {
                            let key=StoredValue::new((source.clone(),target.clone()));
                            view! {
                                <svg class="zxs-disk-tree-edge" data-source-slot=source data-target-slot=target
                                    style:grid-row=move ||topology.with(|tree|key.with_value(|key|tree.edges.get(key).map(|edge|edge.row.to_string())))
                                    style:grid-column=move ||topology.with(|tree|key.with_value(|key|tree.edges.get(key).map(|edge|edge.column.to_string())))
                                    viewBox="0 0 12 36" aria-hidden="true" focusable="false">
                                    <path class="zxs-disk-tree-line" d="M6 36 V4"/>
                                    <path class="zxs-disk-tree-arrow" d="M2 10 L6 2 L10 10 Z"/>
                                </svg>
                            }
                        }/>
                    <For each=move ||topology.with(|tree|tree.ordered_slots())
                        key=|slot|slot.clone() children=move |slot|node(state,topology,slot)/>
                    <For each=move ||candidates.with(|candidates|candidates.iter().map(|candidate|candidate.path.clone()).collect::<Vec<_>>())
                        key=|path|path.clone() children=move |path|candidate_node(state,topology,candidates,path)/>
                </div>
            </div>
        </Show>
    }
}
