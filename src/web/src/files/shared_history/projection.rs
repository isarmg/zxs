use super::Selection;
use std::collections::BTreeMap;
use storage_contract::shared_history::{SharedContent, SharedHistoryGraph, SharedHistoryNodeKind};
use web_interface::i18n::t;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Projection {
    pub records: Vec<(String, u64, Vec<String>)>,
    pub cards: BTreeMap<String, Card>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Card {
    pub title: String,
    pub lines: Vec<String>,
    pub shared: bool,
}
fn hub_id(content: &SharedContent) -> String {
    format!(
        "shared-content:{}:{}",
        content.content_hash, content.logical_size
    )
}
fn kind(kind: SharedHistoryNodeKind) -> String {
    match kind {
        SharedHistoryNodeKind::Content => t("正文节点", "Content event"),
        SharedHistoryNodeKind::Rename => t("路径节点", "Path event"),
        SharedHistoryNodeKind::Tags => t("标签节点", "Tag event"),
        SharedHistoryNodeKind::Merge => t("合并节点", "Merge event"),
        SharedHistoryNodeKind::Deletion => t("删除节点（无正文）", "Deletion event (no body)"),
    }
}
fn content_state(state: Option<storage_contract::ContentState>) -> String {
    match state {
        Some(storage_contract::ContentState::Available) => t(
            "记录保留正文，未检查当前健康",
            "Body recorded, current health not checked",
        ),
        Some(storage_contract::ContentState::Purged) => t(
            "正文已清除，关联元数据保留",
            "Body purged, relationship metadata retained",
        ),
        Some(storage_contract::ContentState::Migrated) => t("正文已迁出", "Body migrated"),
        Some(storage_contract::ContentState::Retired) => {
            t("节点已永久退役", "Node permanently retired")
        }
        None => t("正文状态未知", "Body state unknown"),
    }
}
pub(super) fn project(graph: &SharedHistoryGraph, selection: &Selection) -> Projection {
    let visible = |file: &storage_contract::history_window::HistoryFileId| match selection {
        Selection::All => true,
        Selection::File(selected) => selected == file,
    };
    let versions = graph
        .versions
        .iter()
        .map(|version| (version.version_id.as_str(), version))
        .collect::<BTreeMap<_, _>>();
    let mut result = Projection::default();
    let mut hubs = BTreeMap::new();
    for content in &graph.shared_contents {
        if !content
            .version_ids
            .iter()
            .any(|id| visible(&versions[id.as_str()].file_id))
        {
            continue;
        }
        let id = hub_id(content);
        let mut lines = vec![
            format!("{} · {} B", content.content_hash, content.logical_size),
            t(
                "无箭头连线表示同内容，不是父子时间关系。",
                "Lines without arrows mean equal content, not temporal ancestry.",
            ),
        ];
        for version in &content.version_ids {
            let version = versions[version.as_str()];
            lines.push(format!(
                "{} · {} · {}",
                version.creation_name, version.file_id, version.version_id
            ));
            if visible(&version.file_id) {
                hubs.insert(version.version_id.as_str(), id.clone());
            }
        }
        result.records.push((id.clone(), 0, Vec::new()));
        result.cards.insert(
            id,
            Card {
                title: t("共享内容关系", "Shared-content relationship"),
                lines,
                shared: true,
            },
        );
    }
    let mut heads = BTreeMap::<&str, Vec<String>>::new();
    for branch in &graph.branches {
        let state = match branch.state {
            storage_contract::BranchState::Active => t("活跃", "Active"),
            storage_contract::BranchState::Archived => t("已归档", "Archived"),
            storage_contract::BranchState::Migrated => t("已迁出", "Migrated"),
            storage_contract::BranchState::Merged => t("已合并", "Merged"),
        };
        let lines = heads.entry(branch.head_node_id.as_str()).or_default();
        lines.push(format!(
            "{}: {} ({}) · {}: {}",
            t("分支", "Branch"),
            branch.name,
            branch.branch_id,
            t("状态", "State"),
            state
        ));
        if let Some(target) = &branch.merged_into {
            lines.push(format!(
                "{}: {} → {}",
                t(
                    "分支合并归属（非节点祖先边）",
                    "Branch merge destination (not event ancestry)"
                ),
                branch.branch_id,
                target
            ));
        }
    }
    for node in &graph.nodes {
        if !visible(&node.file_id) {
            continue;
        }
        let id = node.node_id.as_str().to_owned();
        let mut parents = node
            .parent_node_ids
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect::<Vec<_>>();
        if let Some(hub) = hubs.get(id.as_str()) {
            parents.push(hub.clone());
        }
        let mut lines = vec![node.file_id.to_string(), id.clone()];
        let title = if let Some(version) = &node.version_id {
            let version = versions[version.as_str()];
            lines.push(format!(
                "{} · {}",
                version.version_id,
                content_state(version.content_state)
            ));
            format!("{} · {}", version.creation_name, kind(node.kind))
        } else {
            kind(node.kind)
        };
        if let Some(names) = heads.get(id.as_str()) {
            lines.extend(names.iter().cloned());
        }
        result.records.push((id.clone(), node.sequence, parents));
        result.cards.insert(
            id,
            Card {
                title,
                lines,
                shared: false,
            },
        );
    }
    result
}
