//! Owned backup-policy read projection, independent of its HTTP encoding.
use crate::NamespaceEntryKind;
use std::collections::{BTreeMap, BTreeSet};

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
