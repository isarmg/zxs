//! A real CurrentNamespaces consumer for the published complete terminal chain.
//! Ownership remains stopped management plus the exact original target lock.
use super::*;
use crate::engine::disk_registry::current::CurrentNamespaces;
use crate::persist::layout;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct TerminalChainPublishedCurrentReport {
    pub core_hash: String,
    pub checkpoint_hash: String,
    pub snapshot_hash: String,
    pub publication_hash: String,
    pub seal_hash: String,
    pub registry: crate::DiskRegistryCommitment,
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub topology_generation: u64,
    pub file_branch_count: usize,
    pub directory_branch_count: usize,
    #[serialization(with = "storage_contract::u64_string")]
    pub visible_file_branch_entries: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub visible_directory_branch_entries: u64,
    pub target_reverified_this_call: bool,
    pub frozen_namespace_admitted: bool,
    pub active_engine_admitted: bool,
    pub target_writer_admitted: bool,
    pub stop_consumed: bool,
}

/// Frozen metadata only, not a mutable Engine, content reader or I/O binding.
/// This non-serializable handle owns the original target's actual locks and
/// borrows the SYSTEM management owner. Dropping it releases that target hold.
pub struct TerminalChainPublishedCurrent<'a> {
    management: &'a TerminalManagement,
    selection: Selection,
    held: target::Held,
    proof: PublishedProof,
    namespaces: CurrentNamespaces,
}
impl TerminalManagement {
    /// Opens the actual already-published complete source and original target.
    /// Missing/torn evidence is rejected, never repaired or completed here.
    pub fn open_chain_published_current(
        &self,
        principal: &str,
        operation_id: OperationId,
        proposal_digest: &str,
        directory: PendingMemberDirectory,
    ) -> Result<TerminalChainPublishedCurrent<'_>> {
        Engine::require_actor(principal)?;
        let selection = selected(self)?.ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "published Current requires an existing original chain selection",
            )
        })?;
        if selection.proposal.principal() != principal
            || selection.proposal.operation() != operation_id
            || selection.proposal.hash()? != proposal_digest
            || selection.proposal.target().disk_id != directory.disk_id
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "published Current must use the original actor/operation/proposal/target",
            ));
        }
        let certificate = activation::certificate(self, &selection)?;
        let prepared = first::Prepared::new(&selection, &certificate, self.verified_authority())?;
        let held = target::Held::open(self, &selection.proposal, directory)?;
        let (actual, independent_source) = held.verify_decision(&selection)?;
        if actual.decision != certificate.decision {
            return Err(Error::new(
                ErrorCode::LogFork,
                "published Current target vote differs from SYSTEM",
            ));
        }
        let independent = first::Prepared::new(&selection, &actual, &independent_source)?;
        activation::exact(independent.manifest(), prepared.manifest())?;
        let proof = publication::read_existing(self, &selection, &prepared, &held)?;
        let checkpoint = selection
            .proposal
            .activation_source(self.verified_authority())?;
        let namespaces = selection
            .proposal
            .namespaces(&checkpoint)?
            .admit_terminal_chain(checkpoint.state(), &proof)?;
        let reader = TerminalChainPublishedCurrent {
            management: self,
            selection,
            held,
            proof,
            namespaces,
        };
        reader.recheck()?;
        Ok(reader)
    }
}
impl TerminalChainPublishedCurrent<'_> {
    pub(super) fn recheck(&self) -> Result<()> {
        self.held.verify(&self.selection.proposal)?;
        require_selected(self.management, &self.selection)
    }
    /// Current projected entries, not the raw immutable Commit's hidden rows.
    /// Exact selected head equality remains enforced by CurrentNamespaces.
    pub fn branch_entries(&self, branch_id: &str) -> Result<&[crate::Entry]> {
        self.recheck()?;
        let branch = self.proof.state().branches.get(branch_id).ok_or_else(|| {
            Error::new(
                ErrorCode::NotFound,
                "published terminal-chain file branch not found",
            )
        })?;
        self.namespaces
            .branch_entries(branch_id, &branch.head_commit_id)
    }
    /// Current projected directory entries with the complete inherited mask.
    pub fn directory_entries(&self, branch_id: &str) -> Result<&[crate::Entry]> {
        self.recheck()?;
        let branch = self
            .proof
            .state()
            .directory_branches
            .get(branch_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::NotFound,
                    "published terminal-chain directory branch not found",
                )
            })?;
        self.namespaces
            .directory_entries(branch_id, &branch.head_tree_id)
    }
    /// This observation requires a live held original target, unlike historical
    /// source-only status. It never asserts content availability or active I/O.
    pub fn report(&self) -> Result<TerminalChainPublishedCurrentReport> {
        self.namespace_report(&self.namespaces)
    }
    /// The selected-generation reader supplies the Current reconstructed from
    /// ordinary installed objects. Both paths retain the same native proof.
    pub(super) fn namespace_report(
        &self,
        namespaces: &CurrentNamespaces,
    ) -> Result<TerminalChainPublishedCurrentReport> {
        self.recheck()?;
        let state = self.proof.state();
        let mut files = 0_u64;
        for (id, branch) in &state.branches {
            files = files
                .checked_add(namespaces.branch_entries(id, &branch.head_commit_id)?.len() as u64)
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::ResourceLimit,
                        "published Current entry count exhausted",
                    )
                })?;
        }
        let mut directories = 0_u64;
        for (id, branch) in &state.directory_branches {
            directories = directories
                .checked_add(
                    namespaces
                        .directory_entries(id, &branch.head_tree_id)?
                        .len() as u64,
                )
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::ResourceLimit,
                        "published Current entry count exhausted",
                    )
                })?;
        }
        let publication = self.proof.report()?;
        Ok(TerminalChainPublishedCurrentReport {
            core_hash: self.proof.manifest().core_hash.clone(),
            checkpoint_hash: self.selection.proposal.checkpoint_digest().into(),
            snapshot_hash: self.proof.snapshot().into(),
            publication_hash: publication.publication_hash,
            seal_hash: publication.seal_hash,
            registry: namespaces.registry().clone(),
            sequence: namespaces.activation_sequence(),
            topology_generation: state.topology_generation,
            file_branch_count: state.branches.len(),
            directory_branch_count: state.directory_branches.len(),
            visible_file_branch_entries: files,
            visible_directory_branch_entries: directories,
            target_reverified_this_call: true,
            frozen_namespace_admitted: true,
            active_engine_admitted: false,
            target_writer_admitted: false,
            stop_consumed: false,
        })
    }
}

impl TerminalChainPublishedCurrent<'_> {
    /// Used while the bootstrap store is already locked. Full source replay
    /// takes that same lock, so bracket it outside the store owner instead.
    pub(super) fn recheck_held_target(&self) -> Result<()> {
        self.management.verify_held_terminal_writer()?;
        self.held.verify(&self.selection.proposal)
    }
    pub(super) fn epoch_source(&self) -> Result<super::epoch::ChainEpochSource<'_>> {
        self.recheck()?;
        super::epoch::ChainEpochSource::new(
            &self.selection,
            self.proof.manifest(),
            &self.proof.report()?,
        )
    }
    pub(super) fn target_observation(
        &self,
    ) -> Result<crate::engine::disk_lifecycle::bootstrap::RootObservation> {
        self.recheck()?;
        let disk = self.selection.proposal.target_disk();
        crate::engine::disk_lifecycle::bootstrap::RootObservation::capture(
            self.selection.proposal.target(),
            &disk.root,
            &self.held.root.directory(&disk.root)?,
            true,
        )
        .map_err(crate::engine::disk_lifecycle::bootstrap::runtime::management_error)
    }
}

impl TerminalChainPublishedCurrent<'_> {
    pub(super) fn selection(&self) -> &Selection {
        &self.selection
    }
    pub(super) fn held_target(&self) -> &target::Held {
        &self.held
    }
    /// A decision boundary rereads the actual first-slot log and all immutable
    /// dependencies; the previously admitted in-memory Current is not a receipt.
    pub(super) fn recheck_publication(&self) -> Result<()> {
        self.recheck()?;
        let certificate = activation::certificate(self.management, &self.selection)?;
        let prepared = first::Prepared::new(
            &self.selection,
            &certificate,
            self.management.verified_authority(),
        )?;
        let actual =
            publication::read_existing(self.management, &self.selection, &prepared, &self.held)?;
        activation::exact(actual.manifest(), self.proof.manifest())?;
        let actual = actual.report()?;
        let expected = self.proof.report()?;
        if actual.publication_hash != expected.publication_hash
            || actual.seal_hash != expected.seal_hash
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "epoch decision lost the original actual Publication/seal",
            ));
        }
        self.recheck()
    }
}

impl TerminalChainPublishedCurrent<'_> {
    pub(super) fn prepared_generation_inputs(&self) -> Result<first::Prepared> {
        self.recheck_publication()?;
        let certificate = activation::certificate(self.management, &self.selection)?;
        first::Prepared::new(
            &self.selection,
            &certificate,
            self.management.verified_authority(),
        )
    }
    pub(super) fn original_log_image(&self) -> Result<Vec<u8>> {
        self.recheck_publication()?;
        let path = pending::member_directory(
            self.selection.proposal.target_disk(),
            &self.selection.hash()?,
        )
        .join(activation::TARGET_DIRECTORY)
        .join("first-publication")
        .join(layout::JOURNAL.as_path());
        let bytes = self.held.root.read_bytes(&path, 16 * 1024)?;
        self.proof.verify_log_image(&bytes)?;
        self.recheck()?;
        Ok(bytes)
    }
    pub(super) fn install_generation_publication(&self) -> Result<()> {
        self.recheck_publication()?;
        self.proof
            .install_generation_objects(&self.held.root, self.selection.proposal.target_disk())
    }
    pub(super) fn generation_current(
        &self,
        metadata: &DiskRoot,
        bytes: &[u8],
        prepared: &first::Prepared,
    ) -> Result<CurrentNamespaces> {
        self.recheck_publication()?;
        prepared
            .require_generation_layout(&self.held.root, self.selection.proposal.target_disk())?;
        self.proof
            .require_generation_layout(&self.held.root, self.selection.proposal.target_disk())?;
        let proof = self.proof.read_generation(metadata, bytes, prepared)?;
        let checkpoint = self
            .selection
            .proposal
            .activation_source(self.management.verified_authority())?;
        let namespaces = self
            .selection
            .proposal
            .namespaces(&checkpoint)?
            .admit_terminal_chain(checkpoint.state(), &proof)?;
        self.recheck()?;
        Ok(namespaces)
    }
}

impl TerminalChainPublishedCurrent<'_> {
    pub(super) fn publication_report(&self) -> Result<TerminalChainFirstPublication> {
        self.proof.report()
    }
    pub(super) fn file_branch_head(&self, branch_id: &str) -> Result<&str> {
        self.proof
            .state()
            .branches
            .get(branch_id)
            .map(|branch| branch.head_commit_id.as_str())
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::NotFound,
                    "selected generation file branch not found",
                )
            })
    }
    pub(super) fn directory_branch_head(&self, branch_id: &str) -> Result<&str> {
        self.proof
            .state()
            .directory_branches
            .get(branch_id)
            .map(|branch| branch.head_tree_id.as_str())
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::NotFound,
                    "selected generation directory branch not found",
                )
            })
    }
}
