//! Common native choice for a complete registry/placement projection. ALL old
//! members use their existing Abort/Joint slot. A certificate is not yet an
//! active Config, serving root, new voter or permission to erase the source.
use super::*;
use crate::engine::backup_preparation::{
    BackupRegistryProjection, MAX_DECISION_BYTES, PreparedBackupRegistry,
};
use crate::engine::membership_transition::decision::DecisionKind;
use crate::history_package::{
    install_preparation_bytes, preparation_checkpoint, preparation_path_exists,
};
use decision::{DecisionReceipt, DecisionStore};
use pending::{MemberIdentity, admission};
use std::{io::Read, sync::atomic::AtomicBool};

mod first_finalize;
pub use first_finalize::BackupFirstFinalizeReport;
mod first_publication;
pub(in crate::engine) use first_publication::PreparedBackupFirstPublication;
pub use first_publication::{BackupFirstPublicationHash, BackupFirstPublicationReport};
mod payloads;
use payloads::PayloadSource;
mod control;
pub(in crate::engine) use control::BackupFrozenControl;
mod recovery;
pub(in crate::engine) use recovery::AdmittedBackupDecision;
pub(in crate::engine::membership_transition) use recovery::BackupDecisionCertificate;
pub use recovery::BackupRegistryDecisionHash;

const CORE_KIND: &str = "backup-registry-placement-core";
const ACCEPTANCE_KIND: &str = "backup-registry-placement-acceptance";
const CERTIFICATE_KIND: &str = "backup-registry-placement-certificate";
const RETAINED: &str = "metadata/backup-registry-decisions";
const RESERVE: u64 = 16 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct OriginalMember {
    identity: MemberIdentity,
    seal_hash: String,
    fence: Receipt,
    journal: MembershipJournalInspection,
}
struct Source {
    root: DiskRoot,
    disk: DiskConfig,
    seal: Vec<u8>,
    evidence: OriginalMember,
    lease: DiskIoLease,
}
/// Only live Engine admission constructs this context. Recovery reconstructs it
/// from the admitted original fence, never from the target certificate itself.
pub(in crate::engine) struct BackupChoiceContext {
    intent: TransitionIntent,
    transition_hash: String,
    sources: BTreeMap<String, Source>,
    payload_sources: BTreeMap<String, PayloadSource>,
}

impl Engine {
    pub(in crate::engine) fn capture_backup_choice_context(
        &self,
        operation: OperationId,
        principal: &str,
    ) -> Result<BackupChoiceContext> {
        let (transition_hash, _) = self.fenced_backup_inputs(operation, principal)?;
        // Existing full publication-prefix verifier, including actual native
        // fence tails. A receipt copied into SYSTEM cannot substitute for it.
        let inspected = self.inspect_membership_metadata_archive()?;
        let fence = self
            .membership_fence
            .as_ref()
            .expect("invariant: backup fence admitted");
        if inspected.transition_hash != transition_hash {
            return Err(Error::new(
                ErrorCode::LogFork,
                "backup choice source archive differs",
            ));
        }
        let mut sources = BTreeMap::new();
        for journal in inspected.journals {
            let disk = self
                .config
                .metadata_members()
                .find(|disk| disk.disk_id == journal.disk_id)
                .expect("invariant: inspected original member")
                .clone();
            let handle = self.disk_resources.get(&disk)?;
            let admitted = &handle.root;
            let lease = admitted.io_binding()?.admit()?;
            self._locks.verify_disk(&disk, admitted)?;
            let identity = admission::member_identity(
                &self.config,
                &disk,
                admission::verify_named_root(admitted)?,
            );
            let seal = admitted.read_bytes(&admitted.path().join("disk.json"), MAX_METADATA)?;
            validate_stored_seal(&decode_seal(&seal)?, &handle.seal)?;
            let evidence = OriginalMember {
                identity,
                seal_hash: blake3::hash(&seal).to_hex().to_string(),
                fence: fence.receipts[&disk.disk_id].clone(),
                journal,
            };
            sources.insert(
                disk.disk_id.clone(),
                Source {
                    root: admitted.private_child(admitted.path())?,
                    disk,
                    seal,
                    evidence,
                    lease,
                },
            );
        }
        if sources.len() != self.config.metadata_members().len() {
            return Err(Error::new(
                ErrorCode::MetadataQuorumUnavailable,
                "registry decision requires every original member, never a reduced online quorum",
            ));
        }
        let Origin::BackupCutover { evidence, .. } = &fence.intent.origin else {
            unreachable!("invariant: fenced Backup capability checked the origin");
        };
        let mut payload_sources = BTreeMap::new();
        for slot in evidence.retained_payload_slots() {
            let disk = self
                .config
                .disks
                .iter()
                .find(|disk| disk.slot_id == slot)
                .expect("invariant: frozen original source slot exists");
            payload_sources.insert(slot, PayloadSource::capture(self, disk)?);
        }
        Ok(BackupChoiceContext {
            intent: fence.intent.clone(),
            transition_hash,
            sources,
            payload_sources,
        })
    }
}

#[derive(Serialize)]
struct Core<'a> {
    format: &'static str,
    transition_hash: &'a str,
    transition: &'a TransitionIntent,
    registry_placement: &'a BackupRegistryProjection,
    original_members: BTreeMap<&'a str, &'a OriginalMember>,
}
#[derive(Serialize)]
struct Acceptance<'a> {
    core_hash: &'a str,
    original: &'a OriginalMember,
}
#[derive(Serialize)]
struct Certificate<'a> {
    format: &'static str,
    core_hash: &'a str,
    core: &'a Core<'a>,
    decisions: &'a BTreeMap<String, DecisionReceipt>,
}
impl BackupChoiceContext {
    /// Inputs are typed target-local admission, not caller-supplied JSON. The
    /// planned receipt vector below budgets bytes only; it is NEVER installed
    /// as a certificate until each actual no-replace slot is read and synced.
    pub(in crate::engine) fn decide(
        &self,
        prepared: &PreparedBackupRegistry,
        stop: &AtomicBool,
    ) -> Result<String> {
        if prepared.source_fence_hash() != self.transition_hash {
            return Err(Error::new(
                ErrorCode::LogFork,
                "registry choice has another source fence",
            ));
        }
        let core = Core {
            format: CORE_KIND,
            transition_hash: &self.transition_hash,
            transition: &self.intent,
            registry_placement: prepared.projection(),
            original_members: self
                .sources
                .iter()
                .map(|(id, source)| (id.as_str(), &source.evidence))
                .collect(),
        };
        let object = PreparedMetadataObject::new(CORE_KIND, &core, MAX_DECISION_BYTES)?;
        let mut acceptances = BTreeMap::new();
        let mut planned = BTreeMap::new();
        for (id, source) in &self.sources {
            let acceptance = PreparedMetadataObject::new(
                ACCEPTANCE_KIND,
                &Acceptance {
                    core_hash: &object.hash,
                    original: &source.evidence,
                },
                MAX_DECISION_BYTES,
            )?;
            planned.insert(
                id.clone(),
                DecisionReceipt::plan(
                    DecisionKind::BackupRegistryPlacement,
                    &self.transition_hash,
                    &source.evidence.identity,
                    &object.hash,
                    &acceptance.hash,
                )?,
            );
            acceptances.insert(id.clone(), acceptance);
        }
        let planned_certificate = certificate(&core, &object.hash, &planned)?;
        prepared.require_capacity(object.bytes(), planned_certificate.bytes())?;
        for source in self.sources.values() {
            preparation_checkpoint(stop)?;
            source.recheck(stop)?;
            source.require_capacity(&self.transition_hash, &object, &planned_certificate)?;
        }
        // Every original slot is checked before the first new choice. Opening
        // its neutral binding is not a vote. Another kind is never relabelled.
        let mut stores = BTreeMap::new();
        for (id, source) in &self.sources {
            let store = DecisionStore::open(
                &source.root,
                &source.evidence.identity,
                &self.transition_hash,
                true,
            )?;
            store.require_backup_allowed(&object.hash, &acceptances[id].hash)?;
            stores.insert(id.clone(), store);
        }
        prepared.install_inputs(object.bytes(), stop)?;
        for (id, source) in &self.sources {
            preparation_checkpoint(stop)?;
            source.recheck(stop)?;
            object.store_at_root(&source.root, &source.disk)?;
            acceptances[id].store_at_root(&source.root, &source.disk)?;
        }
        prepared.verify(stop)?;
        let mut actual = BTreeMap::new();
        for (id, source) in &self.sources {
            preparation_checkpoint(stop)?;
            prepared.recheck_names()?;
            source.recheck(stop)?;
            let receipt = stores[id].receipt(
                DecisionKind::BackupRegistryPlacement,
                &object.hash,
                &acceptances[id].hash,
                true,
                #[cfg(feature = "fault-injection")]
                None,
            )?;
            receipt.verify(
                DecisionKind::BackupRegistryPlacement,
                &self.transition_hash,
                &source.evidence.identity,
                &object.hash,
                &acceptances[id].hash,
            )?;
            if receipt != planned[id] {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "actual registry vote differs from budgeted exact decision",
                ));
            }
            actual.insert(id.clone(), receipt);
        }
        // No N/Q reduction, no cached counts, and no marker before ALL actual
        // native decisions and a final independent complete target readback.
        for (id, source) in &self.sources {
            preparation_checkpoint(stop)?;
            source.recheck(stop)?;
            stores[id].verify_receipt(&actual[id], true)?;
        }
        prepared.verify(stop)?;
        let confirmed = certificate(&core, &object.hash, &actual)?;
        if confirmed.bytes() != planned_certificate.bytes() {
            return Err(Error::new(
                ErrorCode::LogFork,
                "registry certificate differs from exact original selection",
            ));
        }
        for source in self.sources.values() {
            preparation_checkpoint(stop)?;
            source.recheck(stop)?;
            confirmed.store_at_root(&source.root, &source.disk)?;
            install_preparation_bytes(
                &source.root,
                &source.decision_path(&self.transition_hash),
                confirmed.bytes(),
            )?;
        }
        prepared.install_decision(confirmed.bytes(), stop)?;
        // The diagnostics may only report selected-after-copy when every
        // pointer still names the exact complete certificate we just admitted.
        for (id, source) in &self.sources {
            preparation_checkpoint(stop)?;
            source.recheck(stop)?;
            stores[id].verify_receipt(&actual[id], false)?;
            if source.root.read_bytes(
                &source.decision_path(&self.transition_hash),
                MAX_DECISION_BYTES,
            )? != confirmed.bytes()
            {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "original registry certificate changed",
                ));
            }
        }
        prepared.recheck_names()?;
        Ok(confirmed.hash)
    }
}
fn certificate(
    core: &Core<'_>,
    hash: &str,
    decisions: &BTreeMap<String, DecisionReceipt>,
) -> Result<PreparedMetadataObject> {
    PreparedMetadataObject::new(
        CERTIFICATE_KIND,
        &Certificate {
            format: CERTIFICATE_KIND,
            core_hash: hash,
            core,
            decisions,
        },
        MAX_DECISION_BYTES,
    )
}
impl Source {
    fn decision_path(&self, transition: &str) -> std::path::PathBuf {
        self.root
            .path()
            .join(RETAINED)
            .join(transition)
            .join("certificate.json")
    }
    fn require_capacity(
        &self,
        transition: &str,
        core: &PreparedMetadataObject,
        certificate: &PreparedMetadataObject,
    ) -> Result<()> {
        let path = self.decision_path(transition);
        if preparation_path_exists(&self.root, &path)?
            && self.root.read_bytes(&path, MAX_DECISION_BYTES)? != certificate.bytes()
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "original registry choice pointer conflicts",
            ));
        }
        // Conservative reservation also covers the small acceptance and slot.
        let required = RESERVE + core.bytes().len() as u64 + 2 * certificate.bytes().len() as u64;
        if self.root.available()? < required {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "each original member must retain the complete native registry choice",
            ));
        }
        Ok(())
    }
    fn recheck(&self, stop: &AtomicBool) -> Result<()> {
        self.lease.check_running()?;
        self.evidence.identity.verify_physical_root(&self.root)?;
        if self
            .root
            .read_bytes(&self.root.path().join("disk.json"), MAX_METADATA)?
            != self.seal
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "original source seal changed; registry choice remains stopped",
            ));
        }
        let mut file = self
            .root
            .open_read_regular(&crate::journal::log_path(&self.disk))?;
        let mut digest = blake3::Hasher::new();
        let mut count = 0u64;
        let mut bytes = [0u8; 64 * 1024];
        loop {
            preparation_checkpoint(stop)?;
            self.lease.check_running()?;
            let read = file.read(&mut bytes)?;
            if read == 0 {
                break;
            }
            count = count.checked_add(read as u64).ok_or_else(|| {
                Error::new(ErrorCode::ResourceLimit, "original journal size overflow")
            })?;
            if count > self.evidence.journal.bytes {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "original journal extends past its frozen fence",
                ));
            }
            digest.update(&bytes[..read]);
        }
        if count != self.evidence.journal.bytes
            || digest.finalize().to_hex().as_str() != self.evidence.journal.content_hash
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "original source journal differs from admitted frozen control prefix",
            ));
        }
        self.evidence.identity.verify_physical_root(&self.root)?;
        self.lease.check_running()
    }
}
