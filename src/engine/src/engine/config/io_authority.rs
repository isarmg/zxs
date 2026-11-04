//! In-memory Genesis control-context equivalence, not an authentication proof.
//!
//! Voter membership alone cannot authorize control over all registered disks.
//! Preserve every persisted Config field except relocatable root addresses;
//! physical binding is checked independently by the held disk's I/O gate.
use crate::{Config, DiskConfig, Mode, RetainedWorkspaceLimits};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
struct RegisteredDisk {
    slot_id: String,
    level: u32,
    upstream_slot_id: Option<String>,
    metadata_member: bool,
    binding_generation: u64,
    capacity_bytes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GenesisIoAuthority {
    control: GenesisControlContext,
    retained: RetainedWorkspaceLimits,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GenesisControlContext {
    pool_id: String,
    mode: Mode,
    disks: BTreeMap<String, RegisteredDisk>,
    checksum_chunk_bytes: u32,
    max_file_bytes: u64,
    max_workspace_bytes: u64,
    max_workspaces: usize,
    max_entries: usize,
    metadata_reserve_bytes: u64,
    workspace_ttl_seconds: u64,
}

impl GenesisIoAuthority {
    pub(super) fn from_config(config: &Config) -> Self {
        // Exhaustive destructuring makes a new persisted field require an
        // explicit decision here. No serialization or persistent hash changes.
        let Config {
            pool_id,
            mode,
            disks,
            checksum_chunk_bytes,
            max_file_bytes,
            max_workspace_bytes,
            max_workspaces,
            max_retained_workspaces,
            max_retained_workspaces_per_principal,
            max_retained_workspace_metadata_bytes,
            max_retained_workspace_metadata_bytes_per_principal,
            max_entries,
            metadata_reserve_bytes,
            workspace_ttl_seconds,
        } = config;
        let disks = disks
            .iter()
            .map(|disk| {
                let DiskConfig {
                    disk_id,
                    slot_id,
                    root: _,
                    level,
                    upstream_slot_id,
                    metadata_member,
                    binding_generation,
                    capacity_bytes,
                } = disk;
                (
                    disk_id.clone(),
                    RegisteredDisk {
                        slot_id: slot_id.clone(),
                        level: *level,
                        upstream_slot_id: upstream_slot_id.clone(),
                        metadata_member: *metadata_member,
                        binding_generation: *binding_generation,
                        capacity_bytes: *capacity_bytes,
                    },
                )
            })
            .collect();
        Self {
            control: GenesisControlContext {
                pool_id: pool_id.clone(),
                mode: mode.clone(),
                disks,
                checksum_chunk_bytes: *checksum_chunk_bytes,
                max_file_bytes: *max_file_bytes,
                max_workspace_bytes: *max_workspace_bytes,
                max_workspaces: *max_workspaces,
                max_entries: *max_entries,
                metadata_reserve_bytes: *metadata_reserve_bytes,
                workspace_ttl_seconds: *workspace_ttl_seconds,
            },
            retained: RetainedWorkspaceLimits {
                max_retained_workspaces: *max_retained_workspaces,
                max_retained_workspaces_per_principal: *max_retained_workspaces_per_principal,
                max_retained_workspace_metadata_bytes: *max_retained_workspace_metadata_bytes,
                max_retained_workspace_metadata_bytes_per_principal:
                    *max_retained_workspace_metadata_bytes_per_principal,
            },
        }
    }

    /// Recovery may impose a stricter local retained-evidence budget, never
    /// change registration or use the projection to raise the saved baseline.
    /// Full I/O-context equality above still includes all four persisted limits.
    pub(super) fn permits_recovery_projection(&self, projected: &Self) -> bool {
        let saved = &self.retained;
        let local = &projected.retained;
        self.control == projected.control
            && local.max_retained_workspaces <= saved.max_retained_workspaces
            && local.max_retained_workspaces_per_principal
                <= saved.max_retained_workspaces_per_principal
            && local.max_retained_workspace_metadata_bytes
                <= saved.max_retained_workspace_metadata_bytes
            && local.max_retained_workspace_metadata_bytes_per_principal
                <= saved.max_retained_workspace_metadata_bytes_per_principal
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/config/io_authority_tests.rs"]
mod tests;
