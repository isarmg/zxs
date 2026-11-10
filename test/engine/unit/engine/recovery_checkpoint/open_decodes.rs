use super::*;
use crate::{DiskHistoryMaintenanceState, DiskHistoryStatus, DiskRegistrationIdentity};

pub(super) struct OpenDecodes {
    pub(super) total: usize,
    sequence: u64,
    genesis: bool,
    blocked: bool,
    histories: Vec<DiskHistoryStatus>,
}

pub(super) fn reopen(root: &Path) -> (Engine, OpenDecodes) {
    SNAPSHOT_LOAD_COUNT.with(|count| count.set(0));
    let engine = Engine::open(root).unwrap();
    let decoded = OpenDecodes {
        total: SNAPSHOT_LOAD_COUNT.with(|count| count.get()),
        sequence: engine.state().sequence,
        genesis: engine.config.generation().is_none(),
        blocked: engine.blocked,
        histories: engine.disk_history_statuses(),
    };
    (engine, decoded)
}

impl OpenDecodes {
    fn assert_genesis_disks(&self, config: &Config) {
        assert!(self.genesis, "this decode budget requires Genesis journals");
        assert!(!config.disks.is_empty());
        assert_eq!(self.histories.len(), config.disks.len());
        for disk in &config.disks {
            let identity = DiskRegistrationIdentity {
                pool_id: config.pool_id.clone(),
                disk_id: disk.disk_id.clone(),
                slot_id: disk.slot_id.clone(),
                binding_generation: disk.binding_generation,
            };
            assert!(
                self.histories
                    .iter()
                    .any(|status| status.identity == identity),
                "normal open must report the exact configured disk: {identity:?}"
            );
        }
        for status in &self.histories {
            assert_eq!(status.observed_sequence, self.sequence);
        }
    }

    pub(super) fn recovery_after_histories(&self, config: &Config) -> usize {
        self.assert_genesis_disks(config);
        assert!(!self.blocked);
        // Genesis history capture decodes exactly one snapshot per publication
        // for each disk. A fresh open must finish every install before any of
        // that work can be removed from the authoritative recovery budget.
        let mut history = 0;
        for status in &self.histories {
            let DiskHistoryMaintenanceState::Tracked {
                persisted_through_sequence,
                observed_through_sequence,
                observed_publications,
                ..
            } = &status.state
            else {
                panic!("normal open must finish this disk's history: {status:?}");
            };
            assert_eq!(*persisted_through_sequence, self.sequence);
            assert_eq!(*observed_through_sequence, self.sequence);
            history += *observed_publications as usize;
        }
        assert!(self.total >= history);
        self.total - history
    }

    pub(super) fn blocked_recovery(&self, config: &Config) -> usize {
        self.assert_genesis_disks(config);
        assert!(self.blocked);
        // require_write rejects every rebuild before capture when recovery
        // has blocked writes. No history snapshot decode is subtracted here.
        for status in &self.histories {
            let DiskHistoryMaintenanceState::Unavailable(failure) = &status.state else {
                panic!("blocked open must report each rejected rebuild: {status:?}");
            };
            assert_eq!(failure.sequence(), self.sequence);
            assert_eq!(failure.cause().code(), ErrorCode::RecoveryRequired);
        }
        self.total
    }
}
