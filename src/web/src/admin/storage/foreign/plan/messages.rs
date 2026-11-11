use super::*;
impl DraftError {
    pub(super) fn message(&self) -> String {
        match self {
            Self::Slot(_) => t(
                "请明确填写有效的目标槽位。",
                "Enter an explicit valid target slot.",
            ),
            Self::Parent(_) => t(
                "目标必须是一级盘中的有效相对目录；留空表示盘根。",
                "Use a valid relative directory in the primary; leave it empty for the disk root.",
            ),
            Self::CompleteMapping => t(
                "每份来源历史都必须明确一个目标名称，不能漏掉或合并来源组。",
                "Give every source history its own destination name; do not omit or merge groups.",
            ),
            Self::Name(_) => t(
                "目标子目录名不符合当前命名规则。",
                "A destination child name violates the current naming rules.",
            ),
            Self::Collision => t(
                "目标名称按规范化规则发生碰撞；请分别命名，不能覆盖。",
                "Destination names collide under the naming policy. Choose distinct names; overwriting is not allowed.",
            ),
            Self::SourceTargetAlias => t(
                "可选复制目标不能与来源指向同一对象。",
                "The optional copy target must not refer to the same object as the source.",
            ),
            Self::SourceTier => t(
                "降为二级仅适用于已核验的三级来源。",
                "Secondary admission applies only to a verified tier-3 source.",
            ),
            Self::ExplicitPrimary => t(
                "三级降为二级必须显式选择完整的一级身份，不会默认指定。",
                "Converting tier 3 to tier 2 requires an explicitly selected complete primary identity; none is chosen by default.",
            ),
            Self::PrimaryChanged => t(
                "一级目标身份或绑定代数已变化，请重新核对；新槽位不能与一级相同。",
                "The primary identity or binding generation changed. Review it again; the new slot must differ from the primary.",
            ),
            Self::PreviewUnavailable => t(
                "这是本地意图草稿，当前服务端尚不能审核此接纳方式，也没有迁入执行能力。",
                "This is a local intent draft. The server cannot review this admission mode yet, and import execution is unavailable.",
            ),
        }
    }
}
impl Mode {
    pub(super) fn label(self) -> String {
        match self {
            Self::ReconstructExistingPrimary => t(
                "重建到已有一级的空目录",
                "Reconstruct in an existing primary's empty directory",
            ),
            Self::AdmitNewPrimary => t("新一级接纳草稿", "Draft admission as a new primary"),
            Self::AdmitSecondary => t("三级降为二级草稿", "Draft tier-3 to tier-2 admission"),
        }
    }
}
