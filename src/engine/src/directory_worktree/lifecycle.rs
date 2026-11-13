use name_policy::LookupName;
use storage_contract::working_tree::{
    DirectoryId, MaterialCommitment, SnapshotBranchId, SnapshotNodeId, SnapshotRepositoryId,
};
use storage_contract::{Generation, GenerationExhausted, Id, IdKind, OperationId};

pub(crate) enum WorktreeKind {}
impl IdKind for WorktreeKind {
    const PREFIX: &'static str = "worktree";
}
pub(crate) type WorktreeId = Id<WorktreeKind>;

/// Identifies original immutable material, never a previous worktree's contents.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SnapshotSource {
    pub(crate) repository: SnapshotRepositoryId,
    pub(crate) node: SnapshotNodeId,
    pub(crate) branch: SnapshotBranchId,
    pub(crate) manifest: MaterialCommitment,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Mount {
    pub(crate) parent: DirectoryId,
    pub(crate) name: LookupName,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Revision {
    pub(crate) generation: Generation,
    pub(crate) manifest: MaterialCommitment,
}

/// Both variants require every file, including unchanged files, to be snapshotted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PublicationMode {
    DirectoryCommit { destination: DirectoryDestination },
    FilesBeforeDelete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DirectoryDestination {
    Continue {
        branch: SnapshotBranchId,
        expected_head: SnapshotNodeId,
    },
    Fork {
        branch: SnapshotBranchId,
        expected_source_head: SnapshotNodeId,
    },
}
impl DirectoryDestination {
    pub(crate) fn branch(self) -> SnapshotBranchId {
        match self {
            Self::Continue { branch, .. } | Self::Fork { branch, .. } => branch,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PublicationIntent {
    pub(crate) worktree: WorktreeId,
    pub(crate) source: SnapshotSource,
    pub(crate) revision: Revision,
    pub(crate) operation: OperationId,
    pub(crate) mode: PublicationMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PublicationScope {
    Directory {
        branch: SnapshotBranchId,
        node: SnapshotNodeId,
    },
    FilesOnly,
}

/// This is a reference to an externally verified permanent receipt, not proof
/// that writing this local record published any file or directory nodes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Publication {
    pub(crate) intent: PublicationIntent,
    pub(crate) scope: PublicationScope,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Phase {
    Active {
        saved: Option<Publication>,
    },
    Publishing {
        intent: PublicationIntent,
        saved: Option<Publication>,
    },
    Deleted {
        publication: Publication,
        deletion: OperationId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Worktree {
    id: WorktreeId,
    source: SnapshotSource,
    mount: Mount,
    revision: Revision,
    phase: Phase,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DeletePlan {
    Publish(PublicationIntent),
    Retain,
}

#[derive(Debug, error_definition::Error)]
pub(crate) enum LifecycleError {
    #[error("worktree has a pending publication")]
    Publishing,
    #[error("worktree is retained as deleted")]
    Deleted,
    #[error("worktree is not deleted")]
    NotDeleted,
    #[error("worktree publication receipt differs from its frozen intent")]
    PublicationMismatch,
    #[error("worktree publication created the wrong kind of directory history")]
    PublicationScope,
    #[error("a historical fork must allocate a different branch identity")]
    ForkIdentity,
    #[error("worktree generation is exhausted: {0}")]
    Generation(#[source] GenerationExhausted),
    #[error("worktree restore destination is occupied")]
    NameCollision,
}

impl Worktree {
    /// The owner must allocate an unused ID and install a full independent copy
    /// from source before publishing this record. No retained instance is input.
    pub(crate) fn fresh(id: WorktreeId, source: SnapshotSource, mount: Mount) -> Self {
        Self {
            id,
            source,
            mount,
            revision: Revision {
                generation: Generation::new(0),
                manifest: source.manifest,
            },
            phase: Phase::Active { saved: None },
        }
    }

    pub(crate) fn id(&self) -> WorktreeId {
        self.id
    }

    pub(crate) fn source(&self) -> SnapshotSource {
        self.source
    }

    pub(crate) fn mount(&self) -> &Mount {
        &self.mount
    }

    pub(crate) fn revision(&self) -> Revision {
        self.revision
    }

    pub(crate) fn phase(&self) -> Phase {
        self.phase
    }

    pub(crate) fn record_edit(
        &mut self,
        manifest: MaterialCommitment,
    ) -> Result<(), LifecycleError> {
        self.active_saved()?;
        let generation = self.revision.generation.next().map_err(LifecycleError::Generation)?;
        self.revision = Revision { generation, manifest };
        Ok(())
    }

    /// Observe the source branch HEAD under the final publication lock, not
    /// when the worktree was opened. Fork IDs must be freshly reserved there.
    pub(crate) fn prepare_commit(
        &mut self,
        operation: OperationId,
        current_head: SnapshotNodeId,
        fork_branch: SnapshotBranchId,
    ) -> Result<PublicationIntent, LifecycleError> {
        let destination = if current_head == self.source.node {
            DirectoryDestination::Continue {
                branch: self.source.branch,
                expected_head: current_head,
            }
        } else {
            if fork_branch == self.source.branch {
                return Err(LifecycleError::ForkIdentity);
            }
            DirectoryDestination::Fork {
                branch: fork_branch,
                expected_source_head: current_head,
            }
        };
        self.prepare_publication(operation, PublicationMode::DirectoryCommit { destination })
    }

    pub(crate) fn prepare_delete(
        &mut self,
        operation: OperationId,
    ) -> Result<DeletePlan, LifecycleError> {
        if let Phase::Deleted { deletion, .. } = self.phase {
            return if deletion == operation {
                Ok(DeletePlan::Retain)
            } else {
                Err(LifecycleError::Deleted)
            };
        }
        if let Phase::Publishing { .. } = self.phase {
            return self.prepare_publication(operation, PublicationMode::FilesBeforeDelete)
                .map(DeletePlan::Publish);
        }
        if let Some(publication) = self.active_saved()? {
            if publication.intent.revision == self.revision {
                self.phase = Phase::Deleted { publication, deletion: operation };
                return Ok(DeletePlan::Retain);
            }
        }
        self.prepare_publication(operation, PublicationMode::FilesBeforeDelete)
            .map(DeletePlan::Publish)
    }

    fn prepare_publication(
        &mut self,
        operation: OperationId,
        mode: PublicationMode,
    ) -> Result<PublicationIntent, LifecycleError> {
        let intent = PublicationIntent {
            worktree: self.id,
            source: self.source,
            revision: self.revision,
            operation,
            mode,
        };
        if let Phase::Publishing { intent: pending, .. } = self.phase {
            return if pending == intent {
                Ok(pending)
            } else {
                Err(LifecycleError::Publishing)
            };
        }
        let saved = self.active_saved()?;
        if let Some(publication) = saved {
            if publication.intent.operation == operation {
                return if publication.intent == intent {
                    Ok(intent)
                } else {
                    Err(LifecycleError::PublicationMismatch)
                };
            }
        }
        self.phase = Phase::Publishing { intent, saved };
        Ok(intent)
    }

    /// Only call after the authoritative publisher has verified every file
    /// result and the exact receipt. Unknown outcomes remain Publishing.
    pub(crate) fn complete_publication(
        &mut self,
        publication: Publication,
    ) -> Result<(), LifecycleError> {
        let intent = match self.phase {
            Phase::Publishing { intent, .. } => intent,
            Phase::Active { saved } => return replay_publication(saved, publication),
            Phase::Deleted { publication: saved, .. } => {
                return replay_publication(Some(saved), publication);
            }
        };
        if intent != publication.intent {
            return Err(LifecycleError::PublicationMismatch);
        }
        match (intent.mode, publication.scope) {
            (
                PublicationMode::DirectoryCommit { destination },
                PublicationScope::Directory { branch, node },
            ) => {
                if branch != destination.branch() || node == self.source.node {
                    return Err(LifecycleError::PublicationScope);
                }
                self.phase = Phase::Active { saved: Some(publication) };
            }
            (PublicationMode::FilesBeforeDelete, PublicationScope::FilesOnly) => {
                self.phase = Phase::Deleted { publication, deletion: intent.operation };
            }
            (PublicationMode::DirectoryCommit { .. }, PublicationScope::FilesOnly)
            | (PublicationMode::FilesBeforeDelete, PublicationScope::Directory { .. }) => {
                return Err(LifecycleError::PublicationScope);
            }
        }
        Ok(())
    }

    /// The caller checks the real namespace under the publication fence. A
    /// conflict never changes the retained body, name, revision or receipt.
    pub(crate) fn restore(
        &mut self,
        mount: Mount,
        destination_occupied: bool,
    ) -> Result<(), LifecycleError> {
        let Phase::Deleted { publication, .. } = self.phase else {
            return Err(LifecycleError::NotDeleted);
        };
        if destination_occupied {
            return Err(LifecycleError::NameCollision);
        }
        self.mount = mount;
        self.phase = Phase::Active { saved: Some(publication) };
        Ok(())
    }

    fn active_saved(&self) -> Result<Option<Publication>, LifecycleError> {
        match self.phase {
            Phase::Active { saved } => Ok(saved),
            Phase::Publishing { .. } => Err(LifecycleError::Publishing),
            Phase::Deleted { .. } => Err(LifecycleError::Deleted),
        }
    }
}

fn replay_publication(
    saved: Option<Publication>,
    publication: Publication,
) -> Result<(), LifecycleError> {
    if saved == Some(publication) {
        Ok(())
    } else {
        Err(LifecycleError::PublicationMismatch)
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/directory_worktree/lifecycle.rs"]
mod tests;
