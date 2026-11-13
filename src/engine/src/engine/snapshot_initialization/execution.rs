//! Original-intent initialization, same-candidate material installation and reads.
use crate::engine::snapshot_initialization::{
    bootstrap::AdmittedFreshRoot,
    failure,
    intent::{AdmittedIntent, Source},
    material,
    registration::{DirectoryInitializationBinding, RegistrationRecord, INITIALIZE_KIND},
    store,
    SnapshotInitializationError as Failure,
};
use crate::engine::directory_snapshot_source::{
    DirectorySnapshotContent, VerifiedDirectorySnapshotSource,
};
use crate::journal::{Candidate, Finalize, Publication, PublicationSeal, Receipt};
use crate::working_tree::snapshot::{
    SnapshotEntry, SnapshotEntryContent, SnapshotFile, SnapshotFileSource, SnapshotStoreError,
};
use crate::{Engine, OperationId, OperationResult, State};
use data_serialization::{Deserialize, Serialize};
use name_policy::PortablePath;
use storage_contract::working_tree::{
    DirectoryTargetPath, DirectoryTargetRequest, MaterialCommitment, RootId, SnapshotBranchId,
    SnapshotNodeId, SnapshotRepositoryId,
};
use storage_contract::{ContentHash, FileId, FileNodeId, Principal, Sequence, VersionId};

use crate::persist::layout::snapshot_init::DECISION as PROOF;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct MaterialDecision {
    candidate: String,
    finalize: Finalize,
    binding: DirectoryInitializationBinding,
}

pub(in crate::engine) struct RegisteredSnapshot {
    candidate: Candidate,
    publication: Publication,
    record: RegistrationRecord,
}

#[derive(Clone, Debug)]
pub struct InitializedDirectoryEntry {
    pub path: PortablePath,
    pub file: Option<InitializedDirectoryFile>,
}
#[derive(Clone, Copy, Debug)]
pub struct InitializedDirectoryFile {
    pub file_id: FileId,
    pub version_id: VersionId,
    pub size: u64,
    pub content_hash: ContentHash,
}
pub struct InitializedDirectoryRead {
    pub file: crate::ReadFile,
    pub metadata: InitializedDirectoryFile,
}

fn rejected() -> crate::Error {
    failure(Failure::Publication)
}
fn record(candidate: &Candidate, next: &State) -> crate::Result<RegistrationRecord> {
    let result = next.operations.get(&candidate.operation_id).ok_or_else(rejected)?;
    RegistrationRecord::from_candidate_result(candidate, result)
        .map_err(|error| failure(Failure::Record(error)))
}
fn proof(candidate: &Candidate, finalize: &Finalize) -> crate::Result<MaterialDecision> {
    Ok(MaterialDecision {
        candidate: storage_contract::object_hash("candidate", candidate)?,
        finalize: finalize.clone(),
        binding: candidate.directory_material().ok_or_else(rejected)?.clone(),
    })
}

impl Engine {
    /// Trusted local API: the caller must authorize this root and source branch.
    /// Only an explicit fresh storage root is admitted. This publishes a complete
    /// immutable snapshot from the explicit FILE namespace source. It does not
    /// adopt ordinary host files, create a mutable worktree or create an SMB mount.
    pub fn initialize_directory_snapshot_from_source(
        &mut self,
        root_id: RootId,
        verified: VerifiedDirectorySnapshotSource,
        principal: &str,
        operation: OperationId,
    ) -> crate::Result<OperationResult> {
        Self::require_actor(principal)?;
        self.require_write()?;
        if self.state.operations.contains_key(&operation)
            && !self.directory_snapshots.contains_key(&operation)
        {
            return Err(rejected());
        }
        let admitted = self.fresh_directory_root(root_id)?;
        let root = admitted.binding().clone();
        let frozen = self.freeze_snapshot_initialization_source(verified)
            .map_err(|error| failure(Failure::Source(error)))?;
        let source_branch = frozen.branch().to_string();
        let source_repository = frozen.repository().to_string();
        let snapshot = self.namespace_snapshot(&source_branch)?;
        let mut entries = Vec::with_capacity(frozen.entries().len());
        for entry in frozen.entries() {
            frozen.checkpoint().map_err(|error| failure(Failure::Source(error)))?;
            let content = match entry.content {
                DirectorySnapshotContent::Directory => SnapshotEntryContent::Directory {},
                DirectorySnapshotContent::File { file_id, version_id, size, content_hash } => {
                    let full = match frozen.root() {
                        Some(root) => entry.path.rebase(0, root).map_err(|_| rejected())?,
                        None => entry.path.clone(),
                    };
                    let current = snapshot.tree().get_key(full.key()).ok_or_else(rejected)?;
                    let context = self.file_context_for_current_entry(&source_repository, current)
                        .ok_or_else(rejected)?;
                    let branch = context.branch_id.parse()
                        .map_err(|error| failure(Failure::Contract(error)))?;
                    let node = match context.node_id.as_deref() {
                        Some(node) => node.parse()
                            .map_err(|error| failure(Failure::Contract(error)))?,
                        None => FileNodeId::Version(version_id),
                    };
                    SnapshotEntryContent::File {
                        material: SnapshotFile { file_id, version_id, size, content_hash },
                        source: SnapshotFileSource::Container { branch, node },
                    }
                }
            };
            entries.push(SnapshotEntry {
                path: entry.path.clone(),
                content,
            });
        }
        entries.sort_by(|left, right| left.path.key().cmp(right.path.key()));
        let branch = self.state.branches.get(&source_branch).ok_or_else(rejected)?;
        let snapshot_hash: ContentHash = self.snapshot.as_deref().ok_or_else(rejected)?
            .parse().map_err(|_| rejected())?;
        let source = Source {
            repository: frozen.repository(),
            branch: frozen.branch(),
            head: branch.head_commit_id.parse().map_err(|error| failure(Failure::Contract(error)))?,
            scope: frozen.root().cloned(),
            publication: MaterialCommitment::from_digest(snapshot_hash.digest()),
            sequence: Sequence::new(self.state.sequence),
        };
        if let Some(original) = self.directory_snapshots.get(&operation) {
            if self.state.operations.get(&operation) != Some(&original.publication.result) {
                return Err(rejected());
            }
            let retained = &original.record.intent;
            if retained.root.root_id != root_id
                || retained.principal.as_str() != principal
                || retained.source.repository != source.repository
                || retained.source.branch != source.branch
                || retained.source.head != source.head
                || retained.source.scope != source.scope
                || retained.entries != entries
            {
                return Err(rejected());
            }
            self.read_registered_snapshot(original)?;
            return Ok(original.publication.result.clone());
        }
        let mut members = self.config.disks.iter()
            .filter(|disk| disk.metadata_member)
            .map(|disk| {
                self.registered_disk(&disk.disk_id)
                    .map(|registered| registered.identity())
                    .ok_or_else(rejected)
            })
            .collect::<crate::Result<Vec<_>>>()?;
        members.sort_by(|left, right| left.disk_id.cmp(&right.disk_id));
        let intent = AdmittedIntent::prepare(
            operation,
            Principal::new(principal.into()).map_err(|_| rejected())?,
            source,
            DirectoryTargetRequest {
                root_id,
                path: DirectoryTargetPath::Root,
                expected_root_generation: root.generation,
                operation_id: operation,
            },
            root.clone(),
            root.roots.common_data.entity,
            members,
            SnapshotRepositoryId::generate().map_err(|error| failure(Failure::Contract(error)))?,
            SnapshotBranchId::generate().map_err(|error| failure(Failure::Contract(error)))?,
            SnapshotNodeId::generate().map_err(|error| failure(Failure::Contract(error)))?,
            entries,
        )
        .map_err(|error| failure(Failure::Intent(error)))?;
        drop(snapshot);
        material::require_empty_target(
            &admitted,
            self.disk_resources.get(self.disk(&root.registration.slot_id)?)?,
        )
        .map_err(|error| failure(Failure::Material(error)))?;
        self.retain_snapshot_initialization(&intent)?;
        let publish = (|| -> crate::Result<OperationResult> {
            let disk = self.disk(&root.registration.slot_id)?;
            let handle = self.disk_resources.get(disk)?;
            let prepared = material::prepare(&admitted, handle, &intent, |file| {
                frozen.checkpoint().map_err(|error| {
                    SnapshotStoreError::Disk(failure(Failure::Source(error)))
                })?;
                let opened = self
                    .open_content(&file.version_id.to_string())
                    .map_err(SnapshotStoreError::Disk)?;
                if opened.size != file.size || opened.content_hash != file.content_hash {
                    return Err(SnapshotStoreError::Version);
                }
                // The frozen source owns all version leases through decision.
                Ok(opened.file)
            })
            .map_err(|error| failure(Failure::Material(error)))?;
            let (record, binding) = RegistrationRecord::prepare(&intent, prepared.binding())
                .map_err(|error| failure(Failure::Record(error)))?;
            let mut next = (*self.state).clone();
            next.sequence = self.next_slot;
            let result = OperationResult {
                operation_id: operation,
                kind: INITIALIZE_KIND.into(),
                commit_id: None,
                history_tree_id: None,
                branch_id: None,
                commit_state: crate::CommitState::Published,
                protection_at_publish: crate::ProtectionAtPublish::NotApplicable,
                current_health: crate::CurrentHealth::Healthy,
                request_digest: binding.intent.hash().to_string(),
                sequence: next.sequence,
                result: record.result_value().map_err(|error| failure(Failure::Record(error)))?,
            };
            next.operations.insert(operation, result.clone());
            next.operation_principals
                .insert(operation, intent.record().principal.as_str().to_owned());
            let detached = self.planning_context().prepare_directory_snapshot_publication(
                next,
                result,
                binding,
                crate::disk::MAX_METADATA,
            )?;
            frozen.checkpoint().map_err(|error| failure(Failure::Source(error)))?;
            let result = self.decide_directory_snapshot_publication(detached)?;
            drop(prepared);
            drop(frozen);
            Ok(result)
        })();
        publish.map_err(|error| {
            if error.code() == crate::ErrorCode::InDoubt {
                error
            } else {
                error.context(
                    crate::ErrorCode::RecoveryRequired,
                    format!(
                        "original initialization {operation} remains retained; \
                         preserve its material and recover"
                    ),
                )
            }
        })
    }
}

impl Engine {
    fn snapshot_material_root(
        &self,
        retained: &RegistrationRecord,
    ) -> crate::Result<(AdmittedFreshRoot, std::sync::Arc<crate::disk::DiskHandle>)> {
        let root = &retained.intent.root;
        let admitted = self.fresh_directory_root(root.root_id)?;
        if admitted.binding() != root
            || retained.intent.target.path != DirectoryTargetPath::Root
            || retained.intent.host_entity != root.roots.common_data.entity
        {
            return Err(rejected());
        }
        let registered = self.registered_disk(&root.registration.disk_id).ok_or_else(rejected)?;
        if registered.identity() != root.registration
            || registered.registration_mode != crate::engine::RegistrationMode::Enabled
        {
            return Err(rejected());
        }
        let handle = std::sync::Arc::clone(
            self.disk_resources.get(self.disk(&root.registration.slot_id)?)?,
        );
        Ok((admitted, handle))
    }

    fn require_snapshot_source_inventory(
        &self,
        retained: &RegistrationRecord,
    ) -> crate::Result<()> {
        let source = &retained.intent.source;
        let snapshot = self.namespace_snapshot(&source.branch.to_string())?;
        if snapshot.entries().len()
            > crate::engine::DirectorySnapshotLimits::default().scanned_entries
        {
            return Err(rejected());
        }
        let tree = snapshot.tree();
        if let Some(root) = &source.scope {
            let current = tree.get_key(root.key()).ok_or_else(rejected)?;
            if current.kind != crate::NamespaceEntryKind::Directory || current.path != *root {
                return Err(rejected());
            }
        }
        let expected: std::collections::BTreeMap<_, _> = retained.intent.entries.iter()
            .map(|entry| (entry.path.key(), entry)).collect();
        let mut count = 0usize;
        for (index, current) in snapshot.entries().iter().enumerate() {
            if tree.visible_entry(index).is_none() {
                continue;
            }
            let path = match &source.scope {
                None => current.path.clone(),
                Some(root) => {
                    let Some(suffix) = current.lookup_key().strip_prefix(root.key()) else {
                        continue;
                    };
                    if !suffix.starts_with('/') {
                        continue;
                    }
                    current.path.strip_components(root.as_str().split('/').count())
                        .ok_or_else(rejected)?
                }
            };
            let entry = expected.get(path.key()).ok_or_else(rejected)?;
            if entry.path != path {
                return Err(rejected());
            }
            match entry.content {
                SnapshotEntryContent::Directory {} => {
                    if current.kind != crate::NamespaceEntryKind::Directory {
                        return Err(rejected());
                    }
                }
                SnapshotEntryContent::File {
                    material, source: SnapshotFileSource::Container { branch, node },
                } => {
                    if current.kind != crate::NamespaceEntryKind::File
                        || current.file_id != material.file_id.to_string()
                        || current.version_id != material.version_id.to_string()
                    {
                        return Err(rejected());
                    }
                    let (version, disposition) = self.version_metadata(material.version_id)
                        .ok_or_else(rejected)?;
                    if disposition.is_some_and(|state| state != crate::ContentState::Available)
                        || self.file_deletion(&version.file_id).is_some()
                        || version.file_id != current.file_id
                        || version.logical_size != material.size
                        || version.content_hash != material.content_hash
                    {
                        return Err(rejected());
                    }
                    self.repository_for_history(&version.history_tree_id)?;
                    let context = self.file_context_for_current_entry(
                        &source.repository.to_string(), current,
                    )
                        .ok_or_else(rejected)?;
                    let current_node = context.node_id.as_deref().unwrap_or(&current.version_id);
                    if context.branch_id != branch.to_string()
                        || current_node != node.to_string()
                    {
                        return Err(rejected());
                    }
                }
                SnapshotEntryContent::File { source: SnapshotFileSource::SnapshotOnly, .. } => {
                    return Err(rejected());
                }
            }
            count += 1;
        }
        if count != retained.intent.entries.len() {
            return Err(rejected());
        }
        Ok(())
    }

    pub(in crate::engine) fn require_directory_snapshot_transition(
        &self,
        candidate: &Candidate,
        next: &State,
    ) -> crate::Result<()> {
        if candidate.directory_material().is_none() {
            return Ok(());
        }
        let retained = record(candidate, next)?;
        if !candidate.versions.is_empty()
            || self.snapshot_initialization_intent != Some(candidate.operation_id)
            || self.state.operations.contains_key(&candidate.operation_id)
            || self.state.sequence != retained.intent.source.sequence.get()
            || self.snapshot != candidate.previous_snapshot
        {
            return Err(rejected());
        }
        let mut expected = (*self.state).clone();
        expected.sequence = candidate.sequence;
        expected.operations.insert(
            candidate.operation_id,
            next.operations.get(&candidate.operation_id).ok_or_else(rejected)?.clone(),
        );
        expected.operation_principals.insert(
            candidate.operation_id,
            retained.intent.principal.as_str().to_owned(),
        );
        if &expected != next {
            return Err(rejected());
        }
        let branch = self.state.branches.get(&retained.intent.source.branch.to_string())
            .ok_or_else(rejected)?;
        let repository = self.state.repositories.get(&retained.intent.source.repository.to_string())
            .ok_or_else(rejected)?;
        if branch.head_commit_id != retained.intent.source.head.to_string()
            || repository.main_branch_id != retained.intent.source.branch.to_string()
        {
            return Err(rejected());
        }
        self.require_snapshot_source_inventory(&retained)?;
        let original = store::read(&self.snapshot_intent_system().map_err(failure)?)
            .map_err(|error| failure(Failure::Store(error)))?.ok_or_else(rejected)?;
        if original.record() != &retained.intent {
            return Err(rejected());
        }
        for member in &retained.intent.metadata_members {
            let registered = self.registered_disk(&member.disk_id).ok_or_else(rejected)?;
            if registered.identity() != *member || !registered.metadata_member
                || registered.registration_mode != crate::engine::RegistrationMode::Enabled
            {
                return Err(rejected());
            }
            let handle = self.disk_resources.get(self.disk(&member.slot_id)?)?;
            let copy = store::read(&handle.root).map_err(|error| failure(Failure::Store(error)))?
                .ok_or_else(rejected)?;
            if copy.bytes() != original.bytes() {
                return Err(rejected());
            }
        }
        let (admitted, handle) = self.snapshot_material_root(&retained)?;
        // A prior FINALIZE may already have installed exactly this entity.
        // Only true target-leaf absence selects the original stage.
        material::reopen_decided(&admitted, &handle, retained.material, candidate.operation_id)
            .map_err(|error| failure(Failure::Material(error)))?;
        Ok(())
    }

    /// Called only after all original ACCEPT and FINALIZE receipts are retained.
    pub(in crate::engine) fn complete_directory_snapshot_material(
        &self,
        candidate: &Candidate,
        next: &State,
        finalize: &Finalize,
        final_receipts: &[Receipt],
    ) -> crate::Result<()> {
        if candidate.directory_material().is_none() {
            return Ok(());
        }
        let retained = record(candidate, next)?;
        let provisional = Publication {
            candidate_hash: storage_contract::object_hash("candidate", candidate)?,
            finalize: finalize.clone(),
            finalize_receipts: final_receipts.to_vec(),
            payload_receipts: Vec::new(),
            recovery: None,
            result: next.operations.get(&candidate.operation_id).ok_or_else(rejected)?.clone(),
        };
        crate::journal::verify_finalized_publication(candidate, &provisional)?;
        let (admitted, handle) = self.snapshot_material_root(&retained)?;
        let mut held = material::reopen_decided(&admitted, &handle,
            retained.material, candidate.operation_id)
            .map_err(|error| failure(Failure::Material(error)))?;
        held.install(&admitted, &handle).map_err(|error| failure(Failure::Material(error)))?;
        let bytes = storage_contract::canonical_json(&proof(candidate, finalize)?)?;
        held.repository().write_relative_atomic(&PROOF.as_relative(), &bytes, false)?;
        let actual = held.repository().read_entry_bytes(&PROOF, bytes.len())?;
        if actual != bytes {
            return Err(rejected());
        }
        material::reopen_installed(&admitted, &handle, retained.material, candidate.operation_id)
            .map_err(|error| failure(Failure::Material(error)))?;
        Ok(())
    }

    pub(in crate::engine) fn admit_directory_snapshot_publication(
        &mut self,
        candidate: &Candidate,
        next: &State,
        publication: &Publication,
    ) -> crate::Result<()> {
        if candidate.directory_material().is_none() {
            return Ok(());
        }
        let retained = record(candidate, next)?;
        let registered = RegisteredSnapshot {
            candidate: candidate.clone(),
            publication: publication.clone(),
            record: retained,
        };
        self.read_registered_snapshot(&registered)?;
        if self.directory_snapshots.values().any(|existing| {
            existing.record.intent.root.root_id == registered.record.intent.root.root_id
                && existing.candidate.operation_id != candidate.operation_id
        }) {
            return Err(rejected());
        }
        self.directory_snapshots.insert(candidate.operation_id, registered);
        if self.snapshot_initialization_intent == Some(candidate.operation_id) {
            self.snapshot_initialization_intent = None;
        }
        Ok(())
    }

    fn read_registered_snapshot(&self, registered: &RegisteredSnapshot)
        -> crate::Result<material::PreparedMaterial>
    {
        let candidate = &registered.candidate;
        let publication = &registered.publication;
        self.verify_candidate_membership(candidate)?;
        crate::journal::verify_finalized_publication(candidate, publication)?;
        if publication.recovery.is_some() {
            return Err(rejected());
        }
        let hash = storage_contract::object_hash("candidate", candidate)?;
        for member in self.members() {
            let seal: PublicationSeal = self.disk_resources.get(&member)?.root.read_json_observed(
                &crate::journal::publication_seal_path(&member, &hash), &mut |_| Ok(()),
            )?;
            crate::journal::verify_publication_seal(candidate, publication, &seal)?;
        }
        let (admitted, handle) = self.snapshot_material_root(&registered.record)?;
        let held = material::reopen_installed(&admitted, &handle,
            registered.record.material, candidate.operation_id)
            .map_err(|error| failure(Failure::Material(error)))?;
        let expected = storage_contract::canonical_json(&proof(candidate, &publication.finalize)?)?;
        if held.repository().read_entry_bytes(&PROOF, expected.len())? != expected {
            return Err(rejected());
        }
        Ok(held)
    }

    /// Reads only a registration admitted from the original published candidate.
    /// Every call revalidates current disk identity, full material and its proof.
    pub fn list_initialized_directory(&self, operation: OperationId)
        -> crate::Result<Vec<InitializedDirectoryEntry>>
    {
        let registered = self.directory_snapshots.get(&operation).ok_or_else(rejected)?;
        if self.state.operations.get(&operation) != Some(&registered.publication.result) {
            return Err(rejected());
        }
        let held = self.read_registered_snapshot(registered)?;
        Ok(held.node().manifest().entries().iter().map(|entry| InitializedDirectoryEntry {
            path: entry.path.clone(),
            file: match entry.content {
                SnapshotEntryContent::Directory {} => None,
                SnapshotEntryContent::File { material, .. } => Some(InitializedDirectoryFile {
                    file_id: material.file_id, version_id: material.version_id,
                    size: material.size, content_hash: material.content_hash,
                }),
            },
        }).collect())
    }

    pub fn open_initialized_directory_file(
        &self,
        operation: OperationId,
        path: &PortablePath,
    ) -> crate::Result<InitializedDirectoryRead> {
        let registered = self.directory_snapshots.get(&operation).ok_or_else(rejected)?;
        if self.state.operations.get(&operation) != Some(&registered.publication.result) {
            return Err(rejected());
        }
        let held = self.read_registered_snapshot(registered)?;
        let entry = held.node().manifest().entries().iter()
            .find(|entry| entry.path == *path).ok_or_else(rejected)?;
        let SnapshotEntryContent::File { material, .. } = entry.content else {
            return Err(rejected());
        };
        let file = held.node().open_payload(material.version_id)
            .map_err(|error| failure(Failure::Material(material::MaterialError::Snapshot(error))))?;
        Ok(InitializedDirectoryRead {
            file,
            metadata: InitializedDirectoryFile {
                file_id: material.file_id,
                version_id: material.version_id,
                size: material.size,
                content_hash: material.content_hash,
            },
        })
    }
}
