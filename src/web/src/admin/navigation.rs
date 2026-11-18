//! Administration is split by responsibility, not by backend operation names.
use super::{messages::Message, request::Kind};
use web_interface::i18n::t;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Section {
    Rules,
    Diagnostics,
}
impl Section {
    pub(super) fn title(self) -> String {
        match self {
            Self::Rules => t("备份与放置", "Backup and placement"),
            Self::Diagnostics => t("恢复与检查", "Recovery and diagnostics"),
        }
    }
    pub(super) fn actions(self) -> &'static [(Kind, Message<'static>)] {
        match self {
            Self::Rules => &[
                (Kind::Policy, Message::PolicyAction),
                (Kind::Rule, Message::RuleAction),
            ],
            Self::Diagnostics => &[(Kind::Inspect, Message::InspectAction)],
        }
    }
    pub(super) fn contains(self, kind: Kind) -> bool {
        self.actions().iter().any(|(action, _)| *action == kind)
    }
    pub(super) fn help(self) -> String {
        match self {
            Self::Rules => t(
                "日常备份请使用文件或目录详情中的「逐级备份」与「自定义备份前缀」。以下是按路径匹配的高级规则，不会搬移旧版本；路径副本规则不回填历史。更换某盘的下一级备份盘是另一项操作，必须迁移所有数据；此表单不能替代迁移。",
                "For everyday backups, use Recursive backups and Custom backup prefixes in file or folder Details. These advanced path rules do not move existing versions; path copy rules do not backfill history. Replacing a disk’s next backup disk is a separate operation that must migrate all data; this form does not perform that migration.",
            ),
            Self::Diagnostics => t(
                "按 ID 查询原操作或已存在的文件版本清理计划。执行前，请核对列出的版本 ID；这些版本的正文将被永久删除。此页不会创建清理计划。",
                "Look up an original operation or an existing file-version purge plan by ID. Before committing, check the listed version IDs: their content will be permanently deleted. This page does not create a purge plan.",
            ),
        }
    }
}
