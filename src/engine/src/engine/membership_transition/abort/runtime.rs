//! The online chooser only consumes held original roots and a complete F.
use super::*;
use archive::persistent::PendingControlArchive;
use pending::activation::PortableAuthority;
use std::path::Path;

/// Abort authority is durable, but ordinary writes still require the prescribed
/// cancellation publication and every effective original resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MembershipAbortStatus {
    pub operation_id: OperationId,
    pub transition_hash: String,
    pub generation_hash: String,
    pub confirmed_members: usize,
    pub original_members: usize,
    pub original_quorum: usize,
}

impl Engine {
    /// Complete only the already admitted original barrier. No reservation,
    /// target root, or replacement intent is inspected or created here.
    pub fn resume_membership_fence(&mut self) -> Result<MembershipFenceStatus> {
        self.require_no_commit_preparation()?;
        let fence = self.membership_fence.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "no admitted membership fence to resume",
            )
        })?;
        if fence.intent.effect.is_some() {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "Abort fence resume cannot change disk registration or exit effects",
            ));
        }
        self.persist_membership_fence()
    }

    pub fn certify_membership_abort(
        &mut self,
        archive_path: &Path,
        expected_archive_root: &str,
    ) -> Result<MembershipAbortStatus> {
        self.require_no_commit_preparation()?;
        if self.membership_fence.is_none() {
            let generation = self.config.generation().ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "Abort requires an admitted original fence",
                )
            })?;
            if !generation.portable.is_abort()
                || generation.portable.archive_hash()? != expected_archive_root
            {
                return Err(Error::new(
                    ErrorCode::ConfigConflict,
                    "Abort request differs from current authority",
                ));
            }
            return generation.portable.abort_status();
        }
        let source = PendingControlArchive::open_abort(self, archive_path, expected_archive_root)?;
        let (verified, projection) = source.abort_material()?;
        let context = AbortContext::from_archive(verified)?;
        if self.config.generation_hash() != context.intent.origin.predecessor()
            || !pending::activation::authority_config_matches(
                &self.config,
                context.effective_config(),
            )
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "Abort origin differs from the admitted original resource projection",
            ));
        }
        let fence = self
            .membership_fence
            .as_ref()
            .expect("invariant: Abort fence admitted");
        if context.core.transition_hash != fence.hash {
            return Err(Error::new(
                ErrorCode::LogFork,
                "Abort archive differs from the admitted intent",
            ));
        }
        self.blocked = true;
        self.metadata_integrity.invalidate();
        self.published_state_base = None;
        let originals = self.config.metadata_members().cloned().collect::<Vec<_>>();
        let mut leases = Vec::new();
        let mut available = Vec::new();
        let mut closed = BTreeSet::new();
        // Acquire all participating original capabilities before any choice.
        // A closed admission gate is not a bad/missing decision. Exclude that
        // handle only, never its configured identity or vote from fixed N/Q.
        for disk in &self.config.disks {
            let Some(handle) = self.disk_resources.available(disk) else {
                continue;
            };
            let lease = match handle.root.io_binding()?.admit() {
                Ok(lease) => lease,
                Err(error) if error.code() == ErrorCode::DiskUnavailable => {
                    closed.insert(disk.disk_id.clone());
                    continue;
                }
                Err(error) => return Err(error),
            };
            if disk.metadata_member {
                self._locks.verify_disk(disk, &handle.root)?;
                projection.verify_live_member(
                    &context.core.archive,
                    &context.core.transition_hash,
                    &handle.root,
                    disk,
                    &context.generation.hash,
                )?;
                available.push(disk.clone());
            }
            leases.push(lease);
        }
        if available.len() < context.quorum() {
            return Err(Error::new(
                ErrorCode::MetadataQuorumUnavailable,
                format!(
                    "Abort requires fixed old Q={} of N={}; available={}",
                    context.quorum(),
                    originals.len(),
                    available.len(),
                ),
            ));
        }
        let mut eligible = 0;
        for disk in &available {
            let acceptance = context.acceptance(&disk.disk_id)?;
            let root = &self.disk_resources.get(disk)?.root;
            let store = DecisionStore::open(
                root,
                &acceptance.identity,
                &context.core.transition_hash,
                true,
            )?;
            if store.can_choose_abort(
                &context.generation.hash,
                &object_hash(ACCEPTANCE_KIND, acceptance)?,
            )? {
                eligible += 1;
            }
        }
        if eligible < context.quorum() {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                format!(
                    "Abort cannot reach fixed old Q={} of N={}; eligible={}, immutable non-Abort choices={}",
                    context.quorum(),
                    originals.len(),
                    eligible,
                    available.len() - eligible,
                ),
            ));
        }
        let mut votes = Vec::new();
        for disk in &available {
            let root = &self.disk_resources.get(disk)?.root;
            let path = pending::member_directory(disk, &context.core.transition_hash)
                .join(pending::CONTROL_ARCHIVE_DIRECTORY);
            root.private_dir(&path)?;
            source.install(root.private_child(&path)?)?;
            context
                .generation
                .store_at(&self.disk_resources, std::slice::from_ref(disk))?;
            let acceptance = context.acceptance(&disk.disk_id)?;
            let object = PreparedMetadataObject::new(ACCEPTANCE_KIND, acceptance, MAX_METADATA)?;
            object.store_at(&self.disk_resources, std::slice::from_ref(disk))?;
            self._locks.verify_disk(disk, root)?;
            let store = DecisionStore::open(
                root,
                &acceptance.identity,
                &context.core.transition_hash,
                true,
            )?;
            if let Some(receipt) = store.abort_receipt(&context.generation.hash, &object.hash)? {
                votes.push(context.observe_vote(&disk.disk_id, root, receipt)?);
            }
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!("after_membership_abort_choice_{}", disk.disk_id))?;
        }
        if votes.len() < context.quorum() {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                format!(
                    "Abort lacks fixed old Q={} of N={}; Abort votes={}, immutable non-Abort choices={}",
                    context.quorum(),
                    originals.len(),
                    votes.len(),
                    available.len() - votes.len(),
                ),
            ));
        }
        for disk in &available {
            let root = &self.disk_resources.get(disk)?.root;
            self._locks.verify_disk(disk, root)?;
            projection.verify_live_member(
                &context.core.archive,
                &context.core.transition_hash,
                root,
                disk,
                &context.generation.hash,
            )?;
        }
        let witness = context.certify(&votes)?;
        let portable = PortableAuthority::new_abort(
            context.core,
            AbortAuthorityEvidence {
                archive: projection,
                witness,
            },
            self.config
                .generation()
                .map(|generation| &generation.portable),
        )?;
        let status = portable.abort_status()?;
        if !closed.is_empty() {
            self.disk_resources = std::sync::Arc::new(self.disk_resources.excluding(&closed));
        }
        // The evidence is durable on original roots before a marker can select G.
        self.install_certified_abort(portable, &source)?;
        drop(leases);
        Ok(status)
    }

    /// Cancel this admitted pure addition or source-only backup fence, retaining
    /// the original actor/operation and a distinct permanent result. Failure
    /// after certification leaves the exact generation recoverable/fenced.
    pub fn cancel_membership_handoff(
        &mut self,
        archive_path: &Path,
        expected_archive_root: &str,
    ) -> Result<OperationResult> {
        self.certify_membership_abort(archive_path, expected_archive_root)?;
        self.activate_pending_membership()
    }
}
