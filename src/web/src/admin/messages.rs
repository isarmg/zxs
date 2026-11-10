//! Stable, finite message keys for the administration page only.
//! Protocol identifiers and remote diagnostics are not translated.
use super::request::QueryId;
use std::borrow::Cow;
use web_interface::i18n::t;

pub(super) fn query_label(id: QueryId) -> Message<'static> {
    match id {
        QueryId::Repositories => Message::Repositories,
        QueryId::Disks => Message::Disks,
        QueryId::IntegrityCheck => Message::IntegrityCheck,
        QueryId::Repository => Message::Repository,
        QueryId::Operation => Message::Operation,
        QueryId::Plan => Message::Plan,
        QueryId::BranchTree => Message::BranchTree,
        QueryId::BranchDetails => Message::BranchDetails,
        QueryId::FileHistory => Message::FileHistory,
        QueryId::HistoryGraph => Message::HistoryGraph,
        QueryId::Policy => Message::Policy,
        QueryId::PlacementRule => Message::PlacementRule,
        QueryId::Topology => Message::Topology,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Message<'a> {
    UnknownOutcome,
    ResponseUnconfirmed,
    CheckOriginalOperation,
    PathRuleHelp,
    AcknowledgePlan,
    RepositoryAction,
    PolicyAction,
    RuleAction,
    InspectAction,
    Name,
    PrimarySlot,
    Copies,
    RepositoryId,
    PathRuleId,
    PathPrefix,
    Generation,
    RuleId,
    InspectionId,
    Submit,
    PlanReview,
    CommitPlan,
    CancelPlan,
    Repositories,
    Disks,
    IntegrityCheck,
    Repository,
    Operation,
    Plan,
    BranchTree,
    BranchDetails,
    FileHistory,
    HistoryGraph,
    Policy,
    PlacementRule,
    Topology,
    UnknownOperation,
    PositiveCopies,
    SelectAction,
    LoadPlan,
    ReadyPlan,
    ReviewRequired,
    Required(&'a str),
    InvalidGeneration(&'a str),
}
impl Message<'_> {
    fn translations(self) -> (Cow<'static, str>, Cow<'static, str>) {
        match self {
            Self::ResponseUnconfirmed => ("响应未能确认所请求的操作。重试前请检查原操作。".into(), "The response did not confirm the requested action. Inspect the original operation before retrying.".into()),
            Self::UnknownOutcome => ("结果未确认，禁止重复修改。请查询原操作结果，不要改用新操作 ID 重发；无法查询时请在服务器本地核实。".into(), "Outcome unknown; do not repeat mutations. Check the original operation instead of resending under a new ID. Verify locally on the server if its outcome cannot be queried.".into()),
            Self::CheckOriginalOperation => ("查询原操作".into(), "Check original operation".into()),
            Self::PathRuleHelp => ("此表单仅创建或修改无稳定文件／目录身份绑定的现行路径规则，只影响后续版本，不回填历史。稳定文件／目录规则请使用对应的「备份设置」；文件保留设置请显式配置该文件。这些备份设置会为仍可用的历史正文补足副本。".into(), "This form only creates or updates current path-only rules without a stable file/directory scope. Changes affect future versions only and do not backfill history. Use the file or directory's Backup settings for stable scoped rules, or the file's Backup settings for retained settings. Saving backup settings also backfills copies for available historical content.".into()),
            Self::AcknowledgePlan => ("我已审阅此计划及永久删除等后果，确认执行所示摘要对应的计划。".into(), "I reviewed this plan and its consequences, including permanent deletion where applicable, and confirm this exact digest.".into()),
            Self::RepositoryAction => ("仓库".into(), "Repository".into()),
            Self::PolicyAction => ("路径副本规则".into(), "Path copy rule".into()),
            Self::RuleAction => ("主盘规则".into(), "Placement rule".into()),
            Self::InspectAction => ("检查 / 查询".into(), "Inspect / query".into()),
            Self::Name => ("名称".into(), "Name".into()),
            Self::PrimarySlot => ("主盘槽位".into(), "Primary slot".into()),
            Self::Copies => ("份数（含主盘；1 份无冗余）".into(), "Copies including primary; 1 has no redundancy".into()),
            Self::RepositoryId => ("仓库 ID".into(), "Repository ID".into()),
            Self::PathRuleId => ("路径规则 ID".into(), "Path rule ID".into()),
            Self::PathPrefix => ("路径前缀".into(), "Path prefix".into()),
            Self::Generation => ("预期代数（新建填 0）".into(), "Expected generation (0 for new)".into()),
            Self::RuleId => ("规则 ID".into(), "Rule ID".into()),
            Self::InspectionId => ("操作 / 计划 / 策略 / 拓扑 ID".into(), "Operation / plan / policy / topology ID".into()),
            Self::Submit => ("保存配置".into(), "Save configuration".into()),
            Self::PlanReview => ("计划预览".into(), "Plan review".into()),
            Self::CommitPlan => ("提交所示计划".into(), "Commit reviewed plan".into()),
            Self::CancelPlan => ("取消计划".into(), "Cancel plan".into()),
            Self::Repositories => ("仓库列表".into(), "Repositories".into()),
            Self::Disks => ("磁盘".into(), "Disks".into()),
            Self::IntegrityCheck => ("完整性检查".into(), "Integrity check".into()),
            Self::Repository => ("仓库详情".into(), "Repository".into()),
            Self::Operation => ("操作".into(), "Operation".into()),
            Self::Plan => ("计划".into(), "Plan".into()),
            Self::BranchTree => ("分支树".into(), "Branch tree".into()),
            Self::BranchDetails => ("分支详情".into(), "Branch details".into()),
            Self::FileHistory => ("文件历史".into(), "File history".into()),
            Self::HistoryGraph => ("历史图".into(), "History graph".into()),
            Self::Policy => ("副本规则".into(), "Policy".into()),
            Self::PlacementRule => ("主盘规则".into(), "Placement rule".into()),
            Self::Topology => ("拓扑".into(), "Topology".into()),
            Self::UnknownOperation => ("未知操作".into(), "unknown".into()),
            Self::PositiveCopies => ("份数必须为正整数".into(), "copies must be a positive integer".into()),
            Self::SelectAction => ("请先选择管理操作".into(), "Select a management action first".into()),
            Self::LoadPlan => ("请先加载计划".into(), "Load a plan first".into()),
            Self::ReadyPlan => ("只有就绪计划才能提交或取消".into(), "Only a ready plan may be committed or cancelled".into()),
            Self::ReviewRequired => ("请先完整审阅计划并确认其后果".into(), "Review the full plan and acknowledge its consequences first".into()),
            Self::Required(field) => (format!("必须填写 {field}").into(), format!("{field} is required").into()),
            Self::InvalidGeneration(detail) => (format!("代数无效：{detail}").into(), format!("Invalid generation: {detail}").into()),
        }
    }
    pub(super) fn text(self) -> String {
        let (zh, en) = self.translations();
        t(&zh, &en)
    }
}

pub(super) fn input_error(error: &super::request::InputError) -> String {
    use super::request::InputError;
    match error {
        InputError::Required(field) => Message::Required(field).text(),
        InputError::Copies(_) | InputError::ZeroCopies => Message::PositiveCopies.text(),
        InputError::Generation(source) => Message::InvalidGeneration(&source.to_string()).text(),
        InputError::Inspect => Message::SelectAction.text(),
        InputError::MissingPlan => Message::LoadPlan.text(),
        InputError::PlanNotReady => Message::ReadyPlan.text(),
        InputError::ReviewRequired => Message::ReviewRequired.text(),
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/admin/messages_tests.rs"]
mod tests;
