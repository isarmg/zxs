use super::history::{FileNode, NodeKind};
use crate::icons::FileIcon;
use web_interface::{i18n::t, prelude::*};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Action {
    Content,
    Rename,
    Move,
    PathChange,
    Tags,
    Merge,
    Snapshot,
}

impl Action {
    fn name(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::Rename => "rename",
            Self::Move => "move",
            Self::PathChange => "path-change",
            Self::Tags => "tags",
            Self::Merge => "merge",
            Self::Snapshot => "snapshot",
        }
    }

    fn label(self) -> String {
        match self {
            Self::Content => t("保存正文", "Content saved"),
            Self::Rename => t("重命名", "Renamed"),
            Self::Move => t("移动", "Moved"),
            Self::PathChange => t("路径变化", "Path changed"),
            Self::Tags => t("标签更改", "Tags changed"),
            Self::Merge => t("分支合并", "Branches merged"),
            Self::Snapshot => t("目录提交", "Folder committed"),
        }
    }
}

pub(super) fn title(note: &str, sequence: u64) -> String {
    if note.is_empty() {
        format!("{} #{sequence}", t("无备注", "No note"))
    } else {
        note.to_owned()
    }
}

pub(super) fn file_title(node: &FileNode) -> String {
    let note = match node.kind() {
        NodeKind::Content => node.body_note().unwrap_or_default(),
        NodeKind::Rename | NodeKind::Tags | NodeKind::Merge => node.note(),
    };
    title(note, node.sequence())
}

pub(super) fn file_actions(node: &FileNode) -> &'static [Action] {
    match node.kind() {
        NodeKind::Content => &[Action::Content],
        NodeKind::Tags => &[Action::Tags],
        NodeKind::Merge => &[Action::Merge],
        NodeKind::Rename => path_actions(node.previous_path(), node.path()),
    }
}

fn path_actions(previous: Option<&str>, current: Option<&str>) -> &'static [Action] {
    let (Some(previous), Some(current)) = (previous, current) else {
        return &[Action::PathChange];
    };
    let (previous_parent, previous_name) = previous.rsplit_once('/').unwrap_or(("", previous));
    let (current_parent, current_name) = current.rsplit_once('/').unwrap_or(("", current));
    match (
        previous_parent != current_parent,
        previous_name != current_name,
    ) {
        (true, true) => &[Action::Move, Action::Rename],
        (true, false) => &[Action::Move],
        (false, true) => &[Action::Rename],
        (false, false) => &[Action::PathChange],
    }
}

pub(super) fn summary(
    title: Signal<String>,
    actions: Signal<&'static [Action]>,
    current: Signal<bool>,
) -> impl IntoView {
    view! {
        <summary class="zxs-node-card zxs-frame"
            aria-current=move ||current.get().to_string()
            title=move ||title.get()
            aria-label=move ||format!("{}; {}; {}",title.get(),
                actions.get().iter().map(|action|action.label()).collect::<Vec<_>>().join(", "),
                t("展开或收起节点详情","Expand or collapse node details"))>
            <span class="zxs-node-card-title">{move ||title.get()}</span>
            <span class="zxs-node-card-actions">
                <For each=move ||actions.get().iter().copied() key=|action|*action children=move |action|{
                    view!{<span class="zxs-node-action" data-history-action=action.name()
                        role="img" aria-label=action.label() title=action.label()>
                        <FileIcon name=action.name()/>
                    </span>}
                }/>
            </span>
        </summary>
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/node_card_tests.rs"]
mod tests;
