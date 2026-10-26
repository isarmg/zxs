//! Closed internal dispatch vocabulary. Payload/response typing is migrated separately.
macro_rules! actions {
    ($($variant:ident),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum BackendAction { $($variant),+ }
        #[cfg(test)]
        impl BackendAction {
            pub(crate) const ALL: &'static [Self] = &[$(Self::$variant),+];
        }
    };
}
actions! {
    Archive,
    CreateBranch,
    CreateRepository,
    DefaultBranch,
    DirectoryBranchScopes,
    DirectoryBranchSelection,
    FileManagerBackupConfiguration,
    FileManagerBranch,
    FileManagerConfigureBackups,
    FileManagerDirectoryBranches,
    FileManagerDirectorySelect,
    FileManagerEntries,
    FileManagerFileHistory,
    FileManagerFileMetadata,
    FileManagerHistory,
    FileManagerMerge,
    FileManagerNamespace,
    FileManagerNamespaceBatch,
    FileManagerNodeNote,
    FileManagerRenameBranch,
    FileManagerSelect,
    FileManagerTags,
    FileManagerVersionMetadata,
    FileManagerWriteSession,
    HistoricalSavePlan,
    InitializeDirectoryScope,
    MergeLink,
    Plan,
    PlanCancel,
    PlanCommit,
    PurgePlan,
    RegisterDisk,
    RenameDirectoryBranch,
    SplitPlan,
    UpdateDefaultBranch,
}
