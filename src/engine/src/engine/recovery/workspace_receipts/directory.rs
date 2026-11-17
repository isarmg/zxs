//! Actual startup prefix loan for the native directory decision. Its source
//! is the same held replay cursor; there is no State-only or live-context ctor.
use super::*;
use crate::snapshot_worktree::ReceiptIndex;
use crate::snapshot_worktree::drafts::DirectoryPublicationError;

pub(crate) struct DirectoryReplayPrefix<'a> {
    config: &'a ValidatedConfig,
    resources: &'a Arc<DiskResources>,
    previous: &'a State,
    snapshot: &'a Option<String>,
    index: ReceiptIndex<'a>,
}

impl<'a> DirectoryReplayPrefix<'a> {
    pub(super) fn capture(engine: &'a Engine, prefix: &ReceiptPrefix<'a>) -> Result<Self> {
        crate::engine::planning::require_manual_directory_pool(
            &engine.config, &engine.disk_resources, prefix.namespaces,
        )?;
        prefix.originals.environment.as_ref()
            .ok_or(Error::OriginalReceipt(crate::OriginalReceiptError::Prefix))?
            .require(&engine.config, &engine.disk_resources)?;
        let index = ReceiptIndex::capture_with_originals(prefix.state, prefix.originals)
            .map_err(|source| {
                DirectoryPublicationError::ReceiptIndex(source.into()).into_engine()
            })?;
        Ok(Self {
            config: &engine.config, resources: &engine.disk_resources,
            previous: prefix.state, snapshot: prefix.snapshot, index,
        })
    }

    pub(crate) fn config(&self) -> &'a ValidatedConfig { self.config }
    pub(crate) fn resources(&self) -> &'a Arc<DiskResources> { self.resources }
    pub(crate) fn previous(&self) -> &'a State { self.previous }
    pub(crate) fn snapshot(&self) -> &'a Option<String> { self.snapshot }
    pub(crate) fn index(&self) -> &ReceiptIndex<'a> { &self.index }
}

impl Engine {
    pub(super) fn read_directory_receipt_candidate(
        &self, candidate: Candidate,
        original: crate::publication_evidence::AdmittedSnapshot,
        journal_publication: Option<Publication>, hash: &str,
        prefix: ReceiptPrefix<'_>,
    ) -> Result<PreparedCandidate> {
        let source = DirectoryReplayPrefix::capture(self, &prefix)?;
        let directory = crate::snapshot_worktree::drafts::observe_replay_original(
            &source, &candidate, &original, MAX_METADATA,
        ).map_err(DirectoryPublicationError::into_engine)?;
        // A native directory decision never goes through the ordinary unknown
        // kind fallback. FULL FILE and directory material are observed before
        // local evidence, recovery repair or namespace installation can run.
        let containers = Arc::new(
            crate::container_storage::PreparedContainerEntities::
                observe_directory_candidate_from_prefix(
                    &source, &candidate, &original, directory.proof(),
                )?,
        );
        let publication = self.verify_directory_replay_payloads(
            &candidate, &original, journal_publication, hash, &containers,
        )?;
        directory.require_material().map_err(DirectoryPublicationError::into_engine)?;
        let directory_admission = publication.as_ref().map(|publication| {
            DirectoryCandidateAdmission::capture(self, &candidate, publication)
        }).transpose()?;
        drop(containers);
        drop(directory);
        drop(source);
        Ok(PreparedCandidate {
            candidate, publication, barrier_failure: None,
            snapshot: Some(original.into_shared()), next_namespaces: None,
            directory_admission,
        })
    }
}

/// Issued only above, after this native Candidate's complete original transition
/// and actual directory/FILE material passed. No State, body or writer is kept.
pub(super) struct DirectoryCandidateAdmission {
    environment: super::installed::ReceiptEnvironment,
    sequence: u64,
    candidate_hash: String,
    previous_snapshot: Option<String>,
    publication_hash: String,
}

impl DirectoryCandidateAdmission {
    fn capture(engine: &Engine, candidate: &Candidate, publication: &Publication) -> Result<Self> {
        Ok(Self {
            environment: super::installed::ReceiptEnvironment::capture(
                &engine.config, &engine.disk_resources,
            ),
            sequence: candidate.sequence,
            candidate_hash: object_hash("candidate", candidate)?,
            previous_snapshot: candidate.previous_snapshot.clone(),
            publication_hash: object_hash("publication", publication)?,
        })
    }

    pub(super) fn into_checkpoint(
        self, engine: &Engine, candidate: &Candidate,
        publication: &Publication, seal: &PublicationSeal,
    ) -> Result<CheckpointDirectoryObservation> {
        self.environment.require(&engine.config, &engine.disk_resources)?;
        if object_hash("candidate", candidate)? != self.candidate_hash
            || object_hash("publication", publication)? != self.publication_hash
        {
            return Err(Error::OriginalReceipt(crate::OriginalReceiptError::Candidate));
        }
        verify_publication_seal(candidate, publication, seal)?;
        Ok(CheckpointDirectoryObservation {
            admitted: self, seal_hash: object_hash("publication-seal", seal)?,
        })
    }
}

/// One bounded observation per native receipt, not an all-history State cache.
/// It can only reborrow the existing checkpoint-covered read path. Neither a
/// sequence alone nor a serialized checkpoint can construct this product.
pub(in crate::engine::recovery) struct CheckpointDirectoryObservation {
    admitted: DirectoryCandidateAdmission,
    seal_hash: String,
}

impl CheckpointDirectoryObservation {
    pub(in crate::engine::recovery) fn read_covered(
        self, engine: &Engine, online: &[DiskConfig], sequence: u64,
        hash: &str, receipts: &[Receipt], snapshot: &Option<String>,
        checkpoint: &crate::engine::recovery_checkpoint::Checkpoint,
    ) -> Result<PreparedCandidate> {
        let admitted = self.admitted;
        admitted.environment.require(&engine.config, &engine.disk_resources)?;
        if sequence != admitted.sequence || sequence > checkpoint.sequence
            || hash != admitted.candidate_hash.as_str() || *snapshot != admitted.previous_snapshot
        {
            return Err(Error::OriginalReceipt(crate::OriginalReceiptError::Prefix));
        }
        // Re-read the exact objects and current original barrier. Full-State
        // reconstruction already used the authentic preceding State in this
        // invocation's read-only preflight, before any recovery writer opened.
        let candidate: Candidate = verified_object_at(
            &engine.disk_resources, online, "candidate", hash,
        )?;
        engine.verify_candidate_membership(&candidate)?;
        if candidate.sequence != sequence || candidate.previous_snapshot != *snapshot {
            return Err(Error::OriginalReceipt(crate::OriginalReceiptError::Candidate));
        }
        let publication: Publication = verified_object_at(
            &engine.disk_resources, online, "publication", &admitted.publication_hash,
        )?;
        let seal = published_barrier(engine, online, receipts, &candidate, &publication)?;
        if object_hash("publication-seal", &seal)? != self.seal_hash {
            return Err(Error::OriginalReceipt(crate::OriginalReceiptError::Visible));
        }
        Ok(PreparedCandidate {
            candidate, publication: Some(publication), barrier_failure: None,
            snapshot: None, next_namespaces: None, directory_admission: None,
        })
    }
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/recovery/directory_checkpoint.rs"]
mod tests;
