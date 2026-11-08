//! Owned backup-policy read projection, independent of its HTTP encoding.
use crate::NamespaceEntryKind;
use data_serialization::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Origin of the effective policy, not proof that an entry was captured.
/// `Unconfigured` still uses the repository's default placement/copy count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "kebab-case")]
pub enum BackupConfigurationMode {
    Unconfigured,
    Custom,
    RecursiveFiles,
    RecursiveDirectoryTree,
    Retained,
    Unverified,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupScope {
    pub id: String,
    pub kind: NamespaceEntryKind,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupConfigurationSnapshot {
    pub scope: BackupScope,
    pub policy_id: Option<String>,
    pub effective_policy_id: Option<String>,
    pub effective_mode: BackupConfigurationMode,
    pub generation: u64,
    pub effective_generation: u64,
    pub inherited: bool,
    pub target_copies: u32,
    pub backup_prefixes: BTreeMap<String, String>,
    pub route_slot_ids: Vec<String>,
    pub available_route_slot_ids: Vec<String>,
    pub backup_routes: Vec<Vec<String>>,
    pub backup_slot_ids: BTreeSet<String>,
    pub max_copies: usize,
    pub file_count: u64,
    pub backup_paths: BTreeMap<String, String>,
}
