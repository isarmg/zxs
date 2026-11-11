//! Actual original-member bytes at the chosen fence, never reconstructed logs.
use super::*;
use crate::persist::layout;
use std::path::{Path, PathBuf};

const MAX_MEMBER_JOURNAL: usize = 16 * 1024 * 1024;
const MAX_ORIGINAL_BYTES: usize = 128 * 1024 * 1024;

pub(in crate::engine::membership_transition) struct FrozenMember {
    pub disk: DiskConfig,
    pub config: Vec<u8>,
    pub seal: Vec<u8>,
    pub journal: Vec<u8>,
    pub inspection: MembershipJournalInspection,
    pub fence: Receipt,
}
/// Constructed only by comparing real bytes with every member of the admitted
/// original decision. This remains old evidence, not new-target voting power.
pub(in crate::engine) struct BackupFrozenControl {
    pub(in crate::engine::membership_transition) intent: TransitionIntent,
    pub(in crate::engine::membership_transition) members: Vec<FrozenMember>,
}

fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "backup fenced journals exceed 16 MiB/member or 128 MiB total",
    )
}
fn member_path(disk: &DiskConfig) -> PathBuf {
    PathBuf::from("journals")
        .join(&disk.disk_id)
        .join(&disk.slot_id)
        .join(disk.binding_generation.to_string())
}
impl BackupFrozenControl {
    fn read(
        decision: &AdmittedBackupDecision,
        stop: &AtomicBool,
        mut bytes: impl FnMut(&DiskConfig, &Path, usize) -> Result<Vec<u8>>,
    ) -> Result<Self> {
        let intent = decision.transition();
        verify_intent(intent)?;
        if intent.origin.predecessor().is_some() {
            return Err(Error::new(
                ErrorCode::UnsupportedOperation,
                "backup control handoff requires the complete fixed Genesis source; predecessor archives cannot be omitted",
            ));
        }
        let original = intent.origin.config();
        let mut members = Vec::new();
        let mut used = 0usize;
        for disk in original.metadata_members() {
            preparation_checkpoint(stop)?;
            let expected = &decision.original_members()[&disk.disk_id];
            if expected.journal.bytes > MAX_MEMBER_JOURNAL as u64 {
                return Err(budget());
            }
            let mut read = |name: &Path, maximum: usize| -> Result<Vec<u8>> {
                preparation_checkpoint(stop)?;
                let data = bytes(
                    disk,
                    name,
                    maximum.min(MAX_ORIGINAL_BYTES - used),
                )?;
                used = used.checked_add(data.len()).ok_or_else(budget)?;
                if used > MAX_ORIGINAL_BYTES {
                    return Err(budget());
                }
                Ok(data)
            };
            let config = read(Path::new("config.json"), MAX_METADATA)?;
            let saved: ValidatedConfig = crate::journal::decode_proof(&config)?;
            original.verify_saved_metadata_membership(&saved)?;
            let seal = read(layout::DISK_IDENTITY.as_path(), MAX_METADATA)?;
            let journal = read(layout::JOURNAL.as_path(), MAX_MEMBER_JOURNAL)?;
            if blake3::hash(&seal).to_hex().as_str() != expected.seal_hash
                || journal.len() as u64 != expected.journal.bytes
                || blake3::hash(&journal).to_hex().as_str() != expected.journal.content_hash
            {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "fenced control bytes differ from the exact original chosen member",
                ));
            }
            // Frame semantics, old publication proof, member seal identity and
            // the actual final MembershipFence are independently replayed by
            // the normal native control-archive reader after installation.
            members.push(FrozenMember {
                disk: disk.clone(),
                config,
                seal,
                journal,
                inspection: expected.journal.clone(),
                fence: expected.fence.clone(),
            });
        }
        Ok(Self {
            intent: intent.clone(),
            members,
        })
    }

    /// The caller supplies a held target-only archive directory. Stored paths
    /// never determine a source root and no peer can mask a missing local byte.
    pub(in crate::engine) fn read_target(
        decision: &AdmittedBackupDecision,
        archive: &DiskRoot,
        stop: &AtomicBool,
    ) -> Result<Self> {
        Self::read(decision, stop, |disk, name, maximum| {
            archive.read_bytes(&archive.path().join(member_path(disk)).join(name), maximum)
        })
    }
}
impl BackupChoiceContext {
    pub(in crate::engine) fn freeze_generation_control(
        &self,
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        stop: &AtomicBool,
    ) -> Result<BackupFrozenControl> {
        if self.observe_decision(prepared, stop)?.hash() != decision.hash() {
            return Err(Error::new(
                ErrorCode::LogFork,
                "generation control belongs to another actual decision",
            ));
        }
        let frozen = BackupFrozenControl::read(decision, stop, |disk, name, maximum| {
            let source = &self.sources[&disk.disk_id];
            source.lease.check_running()?;
            let path = match name.to_str() {
                Some("config.json") => disk.root.join("metadata/pool-membership/config.json"),
                Some(_) if name == layout::DISK_IDENTITY.as_path() => {
                    disk.root.join(layout::DISK_IDENTITY.as_path())
                }
                Some(_) if name == layout::JOURNAL.as_path() => crate::journal::log_path(disk),
                _ => unreachable!("invariant: fixed original control filenames"),
            };
            let data = source.root.read_bytes(&path, maximum)?;
            source.lease.check_running()?;
            Ok(data)
        })?;
        self.observe_decision(prepared, stop)?;
        Ok(frozen)
    }
}
