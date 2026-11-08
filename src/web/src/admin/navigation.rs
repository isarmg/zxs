//! Administration is split by responsibility, not by backend operation names.
use super::{messages::Message, request::Kind};
use web_interface::i18n::t;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Section {
    Repositories,
    Rules,
    Diagnostics,
}
impl Section {
    pub(super) fn title(self) -> String {
        match self {
            Self::Repositories => t("仓库", "Repositories"),
            Self::Rules => t("备份与放置", "Backup and placement"),
            Self::Diagnostics => t("恢复与检查", "Recovery and diagnostics"),
        }
    }
    pub(super) fn actions(self) -> &'static [(Kind, Message<'static>)] {
        match self {
            Self::Repositories => &[(Kind::Repository, Message::RepositoryAction)],
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
            Self::Repositories => t(
                "这里创建独立历史域及命名共享，不是初始化文件夹仓库。文件和目录的分支、节点、暂存与提交请在对应对象的「详情」管理。",
                "Create independent history domains and named shares here, not folder repositories. Manage file and folder branches, nodes, staging and commits in the object's Details.",
            ),
            Self::Rules => t(
                "日常备份请使用文件或目录详情中的「逐级备份」与「自定义备份前缀」。以下是按路径匹配的高级规则，不会搬移旧版本；路径副本规则不回填历史。更换某盘的下一级备份盘是另一项操作，必须迁移所有数据；此表单不能替代迁移。",
                "For everyday backups, use Recursive backups and Custom backup prefixes in file or folder Details. These advanced path rules do not move existing versions; path copy rules do not backfill history. Replacing a disk’s next backup disk is a separate operation that must migrate all data; this form does not perform that migration.",
            ),
            Self::Diagnostics => t(
                "检查运行状态、查询原操作及已存在的计划。旧整树分支、手工合并连线、清理、拆分和历史保存创建表单已移除；它们不能代替当前文件及目录模型。查询到计划不代表它适用于当前存储，执行仍由核心准入。",
                "Inspect service state, original operations and existing plans. Legacy whole-history branch, manual merge-link, purge, split and historical-save creation forms have been removed; they do not implement the current file and folder model. A retrieved plan is not proof of current support; the core still admits execution.",
            ),
        }
    }
}
