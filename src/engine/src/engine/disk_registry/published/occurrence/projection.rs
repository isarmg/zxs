//! A single immutable projected baseline for read lookup and future writers.
//! Entries copied out of it are drafts, not authenticated history or authority.
use super::{
    DiskRegistryCommitment, Engine, Entry, FrozenDetachSource, NamespaceScope, ProjectionError,
    ProjectionResult, ScopeDisposition, ScopeMask,
};
use crate::{BranchState, Error, NamespaceEntryKind, Workspace};
use name_policy::PortablePath;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProjectionBinding {
    registry: DiskRegistryCommitment,
    mask_hash: String,
    scope: NamespaceScope,
    frozen_head_id: String,
}

/// Constructible only from a verified mask and its exact proposed successor.
/// This proves namespace projection, never active registry publication, ACLs,
/// write routing, placement policy, workspace admission or I/O quiescence.
pub(in crate::engine) struct WriteProjection<'s, 'e> {
    source: &'s FrozenDetachSource<'e>,
    binding: ProjectionBinding,
    disposition: ScopeDisposition,
    entries: Vec<Entry>,
    by_key: BTreeMap<String, usize>,
    by_identity: BTreeMap<String, usize>,
    hidden_paths: BTreeSet<String>,
}
impl<'s, 'e> WriteProjection<'s, 'e> {
    pub(super) fn from_verified(
        source: &'s FrozenDetachSource<'e>,
        registry: DiskRegistryCommitment,
        mask_hash: &str,
        mask: &ScopeMask,
        original_entries: &[Entry],
    ) -> Self {
        let entries: Vec<_> = original_entries
            .iter()
            .filter(|entry| !mask.hidden.contains_key(entry.lookup_key()))
            .cloned()
            .collect();
        let by_key = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.lookup_key().to_owned(), index))
            .collect();
        let by_identity = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.entry_id.clone(), index))
            .collect();
        Self {
            source,
            binding: ProjectionBinding {
                registry,
                mask_hash: mask_hash.into(),
                scope: mask.binding.scope.clone(),
                frozen_head_id: mask.binding.frozen_head_id.clone(),
            },
            disposition: mask.binding.disposition,
            entries,
            by_key,
            by_identity,
            hidden_paths: mask.hidden.keys().cloned().collect(),
        }
    }
    pub(in crate::engine) fn entries(&self) -> &[Entry] {
        &self.entries
    }
    pub(in crate::engine) fn lookup(&self, path: &PortablePath) -> Option<&Entry> {
        self.by_key
            .get(path.key())
            .map(|&index| &self.entries[index])
    }
    fn require_writable(&self) -> ProjectionResult<()> {
        if self.disposition != ScopeDisposition::RegisteredNamespace {
            return Err(ProjectionError::NotWritable);
        }
        match &self.binding.scope {
            NamespaceScope::BranchHead(id) => {
                let branch = self
                    .source
                    .state
                    .branches
                    .get(id)
                    .expect("invariant: mask scope belongs to the frozen source");
                if branch.state != BranchState::Active {
                    return Err(ProjectionError::NotWritable);
                }
            }
            NamespaceScope::DirectoryHead(_) => {}
            NamespaceScope::Commit(_) | NamespaceScope::DirectoryTree(_) => {
                return Err(ProjectionError::NotWritable);
            }
        }
        Ok(())
    }
    /// No fallback to HEAD or pending_file_versions is permitted. Hidden paths
    /// are absent, so recreation allocates both identities instead of reviving
    /// the detached file's identity or version context.
    pub(in crate::engine) fn file_identity(
        &self,
        path: &PortablePath,
    ) -> ProjectionResult<ProjectedFileIdentity> {
        self.require_writable()?;
        if let Some(entry) = self.lookup(path) {
            if entry.kind != NamespaceEntryKind::File {
                return Err(ProjectionError::IdentityReuse(entry.entry_id.clone()));
            }
            return Ok(ProjectedFileIdentity {
                entry_id: entry.entry_id.clone(),
                file_id: entry.file_id.clone(),
                base_version_id: Some(entry.version_id.clone()),
            });
        }
        Ok(ProjectedFileIdentity {
            entry_id: fresh_id("entry")?,
            file_id: fresh_id("file")?,
            base_version_id: None,
        })
    }
    /// Final projection-specific fence for a draft derived from these entries.
    /// Ordinary namespace, version publication, quota and authorization checks
    /// still apply. Imports preserving identity from another scope need that
    /// scope's projected baseline; raw historical entries are not accepted here.
    pub(in crate::engine) fn validate_draft(&self, entries: &[Entry]) -> ProjectionResult<()> {
        self.require_writable()?;
        for entry in entries {
            // An identity surviving at another alias does not reopen this removed
            // occurrence. Occupying its old path is a fresh creation in this
            // draft contract; typed relocation/import admission remains separate.
            if self.hidden_paths.contains(entry.lookup_key())
                && (self
                    .source
                    .known_entry_ids
                    .contains(entry.entry_id.as_str())
                    || (entry.kind == NamespaceEntryKind::File
                        && self.source.known_file_ids.contains(entry.file_id.as_str())))
            {
                return Err(ProjectionError::IdentityReuse(entry.entry_id.clone()));
            }
            if let Some(&index) = self.by_identity.get(&entry.entry_id) {
                let base = &self.entries[index];
                if entry.kind != base.kind || entry.file_id != base.file_id {
                    return Err(ProjectionError::IdentityReuse(entry.entry_id.clone()));
                }
            } else if self
                .source
                .known_entry_ids
                .contains(entry.entry_id.as_str())
                || (entry.kind == NamespaceEntryKind::File
                    && self.source.known_file_ids.contains(entry.file_id.as_str()))
            {
                return Err(ProjectionError::IdentityReuse(entry.entry_id.clone()));
            }
            if entry.kind == NamespaceEntryKind::File {
                if let Some(version) = self.source.state.versions.get(&entry.version_id) {
                    let impact = self.source.preview.version(&entry.version_id)?;
                    if version.file_id != entry.file_id
                        || impact.remaining_holders.iter().next().is_none()
                    {
                        return Err(ProjectionError::IdentityReuse(entry.version_id.clone()));
                    }
                } else if self
                    .source
                    .pending_version_ids
                    .contains(entry.version_id.as_str())
                {
                    return Err(ProjectionError::IdentityReuse(entry.version_id.clone()));
                }
            }
        }
        Ok(())
    }
    /// The proof supplies a fresh workspace ID; an existing or recovered
    /// workspace cannot attach itself to a new view by minting another stamp.
    /// No Workspace is installed and no admission/fencing token is allocated.
    pub(in crate::engine) fn new_workspace(
        &self,
        principal: &str,
        fencing_token: u64,
    ) -> ProjectionResult<WorkspaceProjection> {
        self.require_writable()?;
        if !matches!(self.binding.scope, NamespaceScope::BranchHead(_)) {
            return Err(ProjectionError::NotWritable);
        }
        Engine::require_actor(principal)?;
        Ok(WorkspaceProjection {
            binding: self.binding.clone(),
            workspace_id: fresh_id("ws")?,
            principal: principal.into(),
            fencing_token,
        })
    }
    /// Required again at the consumer's commit linearization point using the
    /// current projection. Never accept a serialized or caller-built stamp.
    pub(in crate::engine) fn validate_workspace(
        &self,
        proof: &WorkspaceProjection,
        workspace: &Workspace,
    ) -> ProjectionResult<()> {
        self.require_writable()?;
        let NamespaceScope::BranchHead(branch) = &self.binding.scope else {
            return Err(ProjectionError::NotWritable);
        };
        if proof.binding != self.binding
            || proof.workspace_id != workspace.workspace_id
            || proof.principal != workspace.principal
            || proof.fencing_token != workspace.fencing_token
            || branch != &workspace.branch_id
            || self.binding.frozen_head_id != workspace.base_head
        {
            return Err(ProjectionError::BaselineMismatch);
        }
        Ok(())
    }
}
fn fresh_id(prefix: &str) -> ProjectionResult<String> {
    Ok(crate::new_id(prefix).map_err(Error::from)?)
}

pub(in crate::engine) struct ProjectedFileIdentity {
    entry_id: String,
    file_id: String,
    base_version_id: Option<String>,
}
impl ProjectedFileIdentity {
    pub(in crate::engine) fn entry_id(&self) -> &str {
        &self.entry_id
    }
    pub(in crate::engine) fn file_id(&self) -> &str {
        &self.file_id
    }
    pub(in crate::engine) fn base_version_id(&self) -> Option<&str> {
        self.base_version_id.as_deref()
    }
}

/// In-memory identity binding, deliberately neither Serialize nor Deserialize.
/// Persistence/admission of post-detach workspaces is a separate integration step.
pub(in crate::engine) struct WorkspaceProjection {
    binding: ProjectionBinding,
    workspace_id: String,
    principal: String,
    fencing_token: u64,
}
impl WorkspaceProjection {
    pub(in crate::engine) fn workspace_id(&self) -> &str {
        &self.workspace_id
    }
}
