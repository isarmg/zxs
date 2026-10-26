//! Closed dispatch vocabulary. Payload/response typing is migrated separately.
use crate::ApiError;
macro_rules! actions {
    ($($variant:ident => $wire:literal),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum BackendAction { $($variant),+ }
        impl BackendAction {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $wire),+ }
            }
        }
        impl TryFrom<&str> for BackendAction {
            type Error = ApiError;
            fn try_from(value: &str) -> Result<Self, Self::Error> {
                match value { $($wire => Ok(Self::$variant),)+ _ => Err(ApiError::unsupported()) }
            }
        }
    };
}
actions! {
    Archive => "archive",
    Check => "check",
    Commit => "commit",
    Compare => "compare",
    CreateBranch => "create_branch",
    CreateRepository => "create_repository",
    DefaultBranch => "default_branch",
    DirectoryBranchScopes => "directory_branch_scopes",
    DirectoryBranchSelection => "directory_branch_selection",
    Disks => "disks",
    Events => "events",
    FileManagerBackupConfiguration => "file_manager_backup_configuration",
    FileManagerBranch => "file_manager_branch",
    FileManagerConfigureBackups => "file_manager_configure_backups",
    FileManagerDirectoryBranches => "file_manager_directory_branches",
    FileManagerDirectorySelect => "file_manager_directory_select",
    FileManagerDisks => "file_manager_disks",
    FileManagerEntries => "file_manager_entries",
    FileManagerFileHistory => "file_manager_file_history",
    FileManagerFileMetadata => "file_manager_file_metadata",
    FileManagerHistory => "file_manager_history",
    FileManagerMerge => "file_manager_merge",
    FileManagerNamespace => "file_manager_namespace",
    FileManagerNamespaceBatch => "file_manager_namespace_batch",
    FileManagerNodeNote => "file_manager_node_note",
    FileManagerRenameBranch => "file_manager_rename_branch",
    FileManagerSelect => "file_manager_select",
    FileManagerTags => "file_manager_tags",
    FileManagerVersionMetadata => "file_manager_version_metadata",
    FileManagerWriteSession => "file_manager_write_session",
    Graph => "graph",
    HistoricalSavePlan => "historical_save_plan",
    History => "history",
    InitializeDirectoryScope => "initialize_directory_scope",
    MergeLink => "merge_link",
    MonitorDisks => "monitor_disks",
    Placement => "placement",
    PlacementRule => "placement_rule",
    Plan => "plan",
    PlanCancel => "plan_cancel",
    PlanCommit => "plan_commit",
    Policy => "policy",
    PurgePlan => "purge_plan",
    RegisterDisk => "register_disk",
    RenameDirectoryBranch => "rename_directory_branch",
    Resolve => "resolve",
    SmbBinding => "smb_binding",
    SplitPlan => "split_plan",
    Topology => "topology",
    UpdateDefaultBranch => "update_default_branch",
    UpdatePlacementRule => "update_placement_rule",
    UpdatePolicy => "update_policy",
    UpdateSmbBinding => "update_smb_binding",
}

#[cfg(test)]
#[path = "../../../test/server/unit/backend_action/tests.rs"]
mod tests;
