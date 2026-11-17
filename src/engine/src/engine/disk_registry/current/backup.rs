//! Backup-specific Current read admission. The original first Candidate has no
//! lifecycle-registry field, so it cannot borrow an Exit/Add publication proof.
//! Full fixed-Genesis replay preserves recursive-policy authority and historical
//! identity fences before a read-only Current loan can be returned.
use super::*;
use crate::engine::membership_transition::{
    PreparedBackupFirstCandidate, PreparedBackupFirstPublication,
};
use crate::history_package::AdmittedState;
use crate::journal::Candidate;
mod routing;
pub(in crate::engine::disk_registry) use routing::BackupRouting;

/// A private replay accumulator, never a serving Current or writer capability.
/// The archive owner calls observe only for authenticated native publications,
/// then completes the entire archive before calling finish.
pub(in crate::engine) struct BackupNamespaceReplay {
    view: CurrentNamespaces,
    original: crate::ValidatedConfig,
    previous: State,
    snapshot: Option<String>,
}
impl BackupNamespaceReplay {
    pub(in crate::engine) fn new(first: &PreparedBackupFirstCandidate) -> Result<Self> {
        let registry =
            super::super::VerifiedDiskRegistry::backup_replay_base(first.original_config())?;
        let source_snapshot = first.candidate().previous_snapshot.clone().ok_or_else(|| {
            Error::new(
                ErrorCode::PublicationInvalid,
                "Backup namespace has no frozen source",
            )
        })?;
        Ok(Self {
            view: CurrentNamespaces {
                registry: registry.commitment().clone(),
                registry_state: Arc::new(registry),
                mask_hash: None,
                source_snapshot,
                source_sequence: 0,
                activation_sequence: 0,
                source_business_generation: 0,
                branches: Arc::new(BTreeMap::new()),
                directories: Arc::new(BTreeMap::new()),
                known_entries: Arc::new(BTreeSet::new()),
                known_files: Arc::new(BTreeSet::new()),
                unavailable_versions: Arc::new(BTreeSet::new()),
                recursive_policies: Arc::default(),
                backup_routing: None,
            },
            original: first.original_config().clone(),
            previous: State::default(),
            snapshot: None,
        })
    }

    pub(in crate::engine) fn observe(
        &mut self,
        candidate: &Candidate,
        published: &AdmittedState,
    ) -> Result<()> {
        let next = published.state();
        if candidate.generation().is_some()
            || candidate.registry().is_some()
            || self.previous.sequence.checked_add(1) != Some(candidate.sequence)
            || candidate.previous_snapshot != self.snapshot
            || next.sequence != candidate.sequence
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "Backup Current replay does not extend its exact original Genesis publication",
            ));
        }
        // The normal exact-write-set verifiers maintain deletion freezes,
        // retirement, copy identity fences and recursive-follow policy state.
        // Initial automatic publications instead use their original complete
        // business/container replay. No receipt label alone admits a delta.
        let source = ArchivedBackupPublication {
            config: &self.original,
            previous: &self.previous,
            next,
            candidate,
            snapshot: self.snapshot.as_deref(),
        };
        self.view = match crate::protection::verify_archived_automatic_backup(&source)? {
            Some(verified) => self.view.advance_archived_automatic(&verified)?,
            None => self.view.advance(&self.previous, next)?,
        };
        self.previous = next.clone();
        self.snapshot = Some(candidate.snapshot_hash.clone());
        Ok(())
    }

    pub(in crate::engine) fn finish<'a>(
        self,
        publication: &'a PreparedBackupFirstPublication,
        candidate: &PreparedBackupFirstCandidate,
        registry: &super::super::VerifiedDiskRegistry,
        published: &'a AdmittedState,
    ) -> Result<BackupNamespaceAdmission<'a>> {
        let mut admitted = self.finish_native(publication, candidate, registry, published)?;
        for scope in Arc::make_mut(&mut admitted.view.branches)
            .values_mut()
            .chain(Arc::make_mut(&mut admitted.view.directories).values_mut())
        {
            Arc::make_mut(scope).writable = false;
        }
        Ok(admitted)
    }

    pub(in crate::engine) fn finish_serving(
        self,
        publication: &PreparedBackupFirstPublication,
        candidate: &PreparedBackupFirstCandidate,
        registry: &super::super::VerifiedDiskRegistry,
        published: &AdmittedState,
        barrier: &crate::engine::backup_preparation::ServingBarrier,
    ) -> Result<CurrentNamespaces> {
        barrier.require_native(candidate, publication, registry.commitment())?;
        // Keep the writability proved by full original replay. Never turn an
        // originally frozen/retired namespace writable by flipping all flags.
        Ok(self
            .finish_native(publication, candidate, registry, published)?
            .view)
    }

    fn finish_native<'a>(
        mut self,
        publication: &'a PreparedBackupFirstPublication,
        candidate: &PreparedBackupFirstCandidate,
        registry: &super::super::VerifiedDiskRegistry,
        published: &'a AdmittedState,
    ) -> Result<BackupNamespaceAdmission<'a>> {
        publication.require_namespace_candidate(candidate)?;
        candidate.require_state(published)?;
        registry.require_backup_namespace(candidate, published)?;
        if self.snapshot != candidate.candidate().previous_snapshot
            || self.snapshot.is_none()
            || self.previous.sequence >= candidate.sequence()
            || self.previous.business_generation != published.state().business_generation
            || !self.view.matches_source(&self.previous)
            || !self.view.matches_source(published.state())
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "Backup Current does not retain its completely replayed original namespaces",
            ));
        }
        // The prescribed Backup State changes routing, not namespace heads or
        // entries. Check actual entries too, not only labels/counts, before
        // replacing the registry binding of this read-only derived view.
        for (id, branch) in &published.state().branches {
            let commit = &published.state().commits[&branch.head_commit_id];
            if self.view.branch_entries(id, &branch.head_commit_id)? != commit.entries.as_slice() {
                return Err(stale());
            }
        }
        for (id, branch) in &published.state().directory_branches {
            if self.view.directory_entries(id, &branch.head_tree_id)? != branch.entries.as_slice() {
                return Err(stale());
            }
        }
        self.view.backup_routing = Some(Arc::new(BackupRouting::capture(
            candidate,
            published.state(),
        )?));
        self.view.registry = registry.commitment().clone();
        self.view.registry_state = Arc::new(registry.clone());
        self.view.source_snapshot = self
            .snapshot
            .expect("invariant: complete replay has a snapshot");
        self.view.source_sequence = self.previous.sequence;
        self.view.source_business_generation = self.previous.business_generation;
        self.view.activation_sequence = candidate.sequence();
        Ok(BackupNamespaceAdmission {
            published,
            view: self.view,
            publication,
        })
    }
}

/// The original fixed-Genesis publication currently being replayed for Backup.
/// Only observe constructs this loan, after its caller authenticates the native
/// journal/seal/history/payload prefix. It grants neither roots nor a writer;
/// even a proved business delta remains private until the entire archive finishes.
pub(crate) struct ArchivedBackupPublication<'a> {
    config: &'a crate::ValidatedConfig,
    previous: &'a State,
    next: &'a State,
    candidate: &'a Candidate,
    snapshot: Option<&'a str>,
}
impl ArchivedBackupPublication<'_> {
    pub(crate) fn config(&self) -> &crate::ValidatedConfig { self.config }
    pub(crate) fn previous(&self) -> &State { self.previous }
    pub(crate) fn next(&self) -> &State { self.next }
    pub(crate) fn candidate(&self) -> &Candidate { self.candidate }
    pub(crate) fn snapshot(&self) -> Option<&str> { self.snapshot }
}

impl CurrentNamespaces {
    fn advance_archived_automatic(
        &self, verified: &crate::protection::VerifiedArchivedAutomaticBackup<'_>,
    ) -> Result<Self> {
        let (previous, next) = verified.states();
        if !self.matches_source(previous) || !self.matches_source(next) {
            return Err(stale());
        }
        let mut view = self.clone();
        // A changed policy invalidates an old recursive binding, but automatic
        // placement grants no new Current recursive-path authority.
        view.recursive_policies = self.advance_recursive_index(previous, next, None, None)?;
        Ok(view)
    }
}

/// A read-only Current reconstructed from every actual source publication and
/// the selected first Backup Publication. No Serialize/Deref/into-Current API.
pub(in crate::engine) struct BackupNamespaceAdmission<'a> {
    published: &'a AdmittedState,
    view: CurrentNamespaces,
    publication: &'a PreparedBackupFirstPublication,
}
impl BackupNamespaceAdmission<'_> {
    pub(in crate::engine) fn state(&self) -> &State {
        self.published.state()
    }
    pub(in crate::engine) fn registry(&self) -> &DiskRegistryCommitment {
        self.view.registry()
    }
    pub(in crate::engine) fn publication_hash(&self) -> &str {
        &self.publication.report().publication_hash
    }
    pub(in crate::engine) fn branch(&self, id: &str) -> Result<(&str, &[Entry])> {
        let branch = self.state().branches.get(id).ok_or_else(|| {
            Error::new(
                ErrorCode::BranchNotFound,
                "published Backup branch is absent",
            )
        })?;
        if branch.state == BranchState::Migrated {
            return Err(Error::new(
                ErrorCode::TreeMoved,
                "published Backup branch was migrated",
            ));
        }
        Ok((
            &branch.head_commit_id,
            self.view.branch_entries(id, &branch.head_commit_id)?,
        ))
    }
    pub(in crate::engine) fn directory(&self, id: &str) -> Result<(&str, &[Entry])> {
        let branch = self.state().directory_branches.get(id).ok_or_else(|| {
            Error::new(
                ErrorCode::DirectoryBranchNotFound,
                "published Backup directory is absent",
            )
        })?;
        Ok((
            &branch.head_tree_id,
            self.view.directory_entries(id, &branch.head_tree_id)?,
        ))
    }
}
