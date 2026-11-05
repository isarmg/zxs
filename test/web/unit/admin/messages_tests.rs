use super::*;

#[test]
fn every_admin_message_has_exact_chinese_and_english_copy() {
    for (key, zh, en) in [
        (
            Message::ResponseUnconfirmed,
            "响应未能确认所请求的操作。重试前请检查原操作。",
            "The response did not confirm the requested action. Inspect the original operation before retrying.",
        ),
        (Message::Title, "日常管理", "Daily administration"),
        (
            Message::Introduction,
            "唯一账户管理全部存储。数据盘不可写时，存储操作禁用。账号设置可修改密码；文件内容请使用 SMB。",
            "The sole account manages all storage. Storage changes require writable data storage. Change the password in account settings; use SMB for file contents.",
        ),
        (
            Message::UnknownOutcome,
            "结果未确认，禁止重复修改。请查询原操作结果，不要改用新操作 ID 重发；无法查询时请在服务器本地核实。",
            "Outcome unknown; do not repeat mutations. Check the original operation instead of resending under a new ID. Verify locally on the server if its outcome cannot be queried.",
        ),
        (
            Message::CheckOriginalOperation,
            "查询原操作",
            "Check original operation",
        ),
        (
            Message::PathRuleHelp,
            "此表单仅创建或修改无稳定文件／目录身份绑定的现行路径规则，只影响后续版本，不回填历史。稳定文件／目录规则请使用对应的「备份设置」；文件保留设置请显式配置该文件。这些备份设置会为仍可用的历史正文补足副本。",
            "This form only creates or updates current path-only rules without a stable file/directory scope. Changes affect future versions only and do not backfill history. Use the file or directory's Backup settings for stable scoped rules, or the file's Backup settings for retained settings. Saving backup settings also backfills copies for available historical content.",
        ),
        (
            Message::AcknowledgePlan,
            "我已审阅此计划及永久删除等后果，确认执行所示摘要对应的计划。",
            "I reviewed this plan and its consequences, including permanent deletion where applicable, and confirm this exact digest.",
        ),
        (Message::RepositoryAction, "仓库", "Repository"),
        (Message::PolicyAction, "路径副本规则", "Path copy rule"),
        (Message::RuleAction, "主盘规则", "Placement rule"),
        (Message::BranchAction, "历史分支", "History branch"),
        (Message::LinkAction, "合并连线", "Merge link"),
        (Message::PurgeAction, "清理预览", "Purge preview"),
        (Message::SplitAction, "拆分预览", "Split preview"),
        (
            Message::SaveAction,
            "历史保存预览",
            "Historical save preview",
        ),
        (Message::InspectAction, "检查 / 查询", "Inspect / query"),
        (Message::Name, "名称", "Name"),
        (Message::PrimarySlot, "主盘槽位", "Primary slot"),
        (
            Message::Copies,
            "份数（含主盘；1 份无冗余）",
            "Copies including primary; 1 has no redundancy",
        ),
        (Message::RepositoryId, "仓库 ID", "Repository ID"),
        (Message::PathRuleId, "路径规则 ID", "Path rule ID"),
        (Message::PathPrefix, "路径前缀", "Path prefix"),
        (
            Message::Generation,
            "预期代数（新建填 0）",
            "Expected generation (0 for new)",
        ),
        (Message::RuleId, "规则 ID", "Rule ID"),
        (Message::SourceCommit, "来源提交", "Source commit"),
        (Message::HistoryId, "历史树 ID", "History ID"),
        (Message::SourceBranch, "来源分支", "Source branch"),
        (Message::TargetBranch, "目标分支", "Target branch"),
        (Message::TargetCommit, "目标提交", "Target commit"),
        (
            Message::VersionIds,
            "版本 ID（逗号分隔）",
            "Version IDs (comma separated)",
        ),
        (Message::CutCommit, "切分提交 ID", "Cut commit ID"),
        (
            Message::Workspace,
            "现有工作区 ID（可选）",
            "Existing workspace ID (optional)",
        ),
        (
            Message::OptionalSlot,
            "主盘槽位（可选）",
            "Primary slot (optional)",
        ),
        (Message::OptionalCopies, "份数（可选）", "Copies (optional)"),
        (
            Message::InspectionId,
            "操作 / 计划 / 策略 / 拓扑 ID",
            "Operation / plan / policy / topology ID",
        ),
        (
            Message::Submit,
            "保存配置 / 创建预览",
            "Save configuration / Create preview",
        ),
        (Message::PlanReview, "计划预览", "Plan review"),
        (Message::CommitPlan, "提交所示计划", "Commit reviewed plan"),
        (Message::CancelPlan, "取消计划", "Cancel plan"),
        (Message::Repositories, "仓库列表", "Repositories"),
        (Message::Disks, "磁盘", "Disks"),
        (Message::IntegrityCheck, "完整性检查", "Integrity check"),
        (Message::Repository, "仓库详情", "Repository"),
        (Message::Operation, "操作", "Operation"),
        (Message::Plan, "计划", "Plan"),
        (Message::BranchTree, "分支树", "Branch tree"),
        (Message::BranchDetails, "分支详情", "Branch details"),
        (Message::FileHistory, "文件历史", "File history"),
        (Message::HistoryGraph, "历史图", "History graph"),
        (Message::Policy, "副本规则", "Policy"),
        (Message::PlacementRule, "主盘规则", "Placement rule"),
        (Message::Topology, "拓扑", "Topology"),
        (Message::UnknownOperation, "未知操作", "unknown"),
        (
            Message::PositiveCopies,
            "份数必须为正整数",
            "copies must be a positive integer",
        ),
        (
            Message::SelectAction,
            "请先选择管理操作",
            "Select a management action first",
        ),
        (Message::LoadPlan, "请先加载计划", "Load a plan first"),
        (
            Message::ReadyPlan,
            "只有就绪计划才能提交或取消",
            "Only a ready plan may be committed or cancelled",
        ),
        (
            Message::ReviewRequired,
            "请先完整审阅计划并确认其后果",
            "Review the full plan and acknowledge its consequences first",
        ),
    ] {
        let actual = key.translations();
        assert_eq!(actual.0, zh, "{key:?}");
        assert_eq!(actual.1, en, "{key:?}");
        assert!(!zh.is_empty());
        assert!(!en.is_empty());
    }
}

#[test]
fn parameters_are_preserved_literally_in_both_languages() {
    let field = "repository/{0}/中文";
    let (zh, en) = Message::Required(field).translations();
    assert_eq!(zh, "必须填写 repository/{0}/中文");
    assert_eq!(en, "repository/{0}/中文 is required");
    let detail = "invalid {1}: generation=18446744073709551616";
    let (zh, en) = Message::InvalidGeneration(detail).translations();
    assert_eq!(zh, format!("代数无效：{detail}"));
    assert_eq!(en, format!("Invalid generation: {detail}"));
}

#[test]
fn localized_input_feedback_preserves_typed_errors_and_sources() {
    use super::super::request::InputError;
    use std::error::Error;
    let copies = InputError::Copies("invalid".parse::<u32>().unwrap_err());
    assert!(copies.source().is_some());
    assert_eq!(input_error(&copies), "copies must be a positive integer");
    let source = storage_contract::parse_u64_decimal("invalid").unwrap_err();
    let detail = source.to_string();
    let generation = InputError::Generation(source);
    assert_eq!(
        input_error(&generation),
        format!("Invalid generation: {detail}")
    );
    assert_eq!(generation.source().unwrap().to_string(), detail);
    assert_eq!(generation.to_string(), detail);
    for (error, expected) in [
        (InputError::Required("repository"), "repository is required"),
        (InputError::ZeroCopies, "copies must be a positive integer"),
        (InputError::Inspect, "Select a management action first"),
        (InputError::MissingPlan, "Load a plan first"),
        (
            InputError::PlanNotReady,
            "Only a ready plan may be committed or cancelled",
        ),
        (
            InputError::ReviewRequired,
            "Review the full plan and acknowledge its consequences first",
        ),
    ] {
        assert_eq!(input_error(&error), expected);
        assert_eq!(error.to_string(), expected);
    }
}

#[test]
fn actual_action_and_query_buttons_use_distinct_localized_keys() {
    use super::super::request::{Kind, QUERIES};
    assert_eq!(
        ACTIONS.map(|(kind, _)| kind),
        [
            Kind::Repository,
            Kind::Policy,
            Kind::Rule,
            Kind::Branch,
            Kind::Link,
            Kind::Purge,
            Kind::Split,
            Kind::Save,
            Kind::Inspect,
        ]
    );
    for keys in [
        ACTIONS.map(|(_, key)| key).to_vec(),
        QUERIES.map(|(id, _)| query_label(id)).to_vec(),
    ] {
        for (index, key) in keys.iter().enumerate() {
            assert!(
                !keys[..index].contains(key),
                "duplicate button key: {key:?}"
            );
            let (zh, en) = key.translations();
            assert!(!zh.is_ascii(), "missing Chinese: {key:?}");
            assert!(en.is_ascii(), "mixed English: {key:?}");
            assert_ne!(zh, en);
        }
    }
}

#[test]
fn shared_default_view_has_no_separate_web_binding_controls() {
    let sources = [
        include_str!("../../../../src/web/src/admin/mod.rs"),
        include_str!("../../../../src/web/src/admin/request.rs"),
        include_str!("../../../../src/web/src/admin/messages.rs"),
    ];
    for source in sources {
        assert!(!source.contains("smb-binding"));
        assert!(!source.contains("Kind::Smb"));
    }
    for (_, key) in ACTIONS {
        assert!(!key.translations().1.contains("SMB binding"));
    }
}
