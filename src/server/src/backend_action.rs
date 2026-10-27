//! Closed internal dispatch vocabulary. Payload/response typing is migrated separately.
macro_rules! actions {
    ($($variant:ident),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum BackendAction { $($variant),+ }
        #[cfg(all(test, feature = "core"))]
        impl BackendAction {
            pub(crate) const ALL: &'static [Self] = &[$(Self::$variant),+];
        }
    };
}
actions! {
    FileManagerBackupConfiguration,
    FileManagerConfigureBackups,
    FileManagerMerge,
    FileManagerNamespace,
    FileManagerTags,
    FileManagerWriteSession,
    HistoricalSavePlan,
    Plan,
    PlanCancel,
    PlanCommit,
    PurgePlan,
    RegisterDisk,
    SplitPlan,
}
