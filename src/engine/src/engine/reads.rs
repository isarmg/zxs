//! Verified replicas, read leases, offline export and disk observations.
pub(crate) mod exposed;
use super::provenance::MetadataReadContext;
use super::{Engine, OpenedContent, ReadLease, ReadSource};
use crate::error::BackupReadError;
use crate::{disk::*, *};
use std::path::Path;
use std::{io::Read, path::PathBuf, sync::Arc};

/// A resolved replica path and immutable version, created only by the complete
/// existing read-proof path below. Callers may retain it but cannot substitute
/// another file path or manufacture this capability from a disk seal.
pub(crate) struct VerifiedReplicaSource {
    scope: Arc<crate::container_storage::ObservedContainerReplica>,
    path: PathBuf,
    version: FileVersion,
}
impl VerifiedReplicaSource {
    /// Re-protection may reuse only a fully read original managed version from
    /// its actual prior route and this planning occurrence's same held disk.
    pub(crate) fn require_backup_source(
        &self,
        context: &super::PlanningContext<'_>,
        version: &FileVersion,
    ) -> Result<()> {
        let invalid = crate::protection::invalid_automatic_backup;
        let placement = context.state.placements.get(&version.version_id)
            .ok_or_else(invalid)?;
        let disk = context.disk(&self.scope.disk().seal.slot_id)?;
        if &self.version != version
            || context.state.versions.get(&version.version_id) != Some(version)
            || !placement.route_slot_ids.contains(&disk.slot_id)
            || placement.binding_generations.get(&disk.slot_id) != Some(&disk.binding_generation)
            || !Arc::ptr_eq(context.disk_resources.get(disk)?, self.scope.disk())
        {
            return Err(invalid());
        }
        self.scope.verify()?;
        self.scope.container()?.admit_io()?.check_running()
    }

    pub(super) fn owner(&self) -> &Arc<DiskHandle> {
        self.scope.disk()
    }
    pub(super) fn open(&self) -> Result<ReadFile> {
        let container = self.scope.container()?;
        let expected = self.scope.body_path(&self.version)?;
        if self.path != expected {
            return Err(super::fresh_layout::FreshLayoutError::ContainerScope.into());
        }
        let file = container.open_leased_regular(&self.path)?;
        self.scope.verify()?;
        Ok(ReadFile::in_container(file, Arc::clone(&self.scope)))
    }

    pub(super) fn with_path(&self, path: PathBuf) -> Self {
        Self { scope: Arc::clone(&self.scope), path, version: self.version.clone() }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    pub(super) fn version(&self) -> &FileVersion {
        &self.version
    }
}

impl Engine {
    pub(crate) fn used_bytes(&self, slot: &str) -> u64 {
        self.planning_context().used_bytes(slot)
    }
    pub(crate) fn valid_replica(&self, version_id: &str) -> Result<(ReadSource, FileVersion)> {
        let source = self.validated_replica_source(version_id)?;
        let version = source.version.clone();
        Ok((ReadSource::VerifiedReplica(Arc::new(source)), version))
    }

    pub(crate) fn valid_backup_replica(&self, version_id: &str) -> Result<ReadSource> {
        self.validated_replica_source(version_id)
            .map(|source| ReadSource::VerifiedReplica(Arc::new(source)))
    }

    fn validated_replica_source(&self, version_id: &str) -> Result<VerifiedReplicaSource> {
        self.with_replica(version_id, |scope, material, version, lease| {
            verify_read_payload(material, version, lease)?;
            let content = scope.body_path(version)?;
            Ok(VerifiedReplicaSource {
                scope: Arc::clone(scope), path: content, version: version.clone(),
            })
        })
    }
    fn verified_replica(&self, version_id: &str) -> Result<(ReadFile, FileVersion, PathBuf)> {
        self.with_replica(version_id, |scope, material, version, lease| {
            let mut file = verify_read_payload(material, version, lease)?;
            use std::io::{Seek, SeekFrom};
            file.seek(SeekFrom::Start(0))?;
            Ok((
                ReadFile::in_container(file, Arc::clone(scope)),
                version.clone(),
                material.directory()?,
            ))
        })
    }

    fn with_replica<T>(
        &self,
        version_id: &str,
        mut read: impl FnMut(
            &Arc<crate::container_storage::ObservedContainerReplica>,
            &VersionMaterial<'_>, &FileVersion, &DiskIoLease,
        ) -> Result<T>,
    ) -> Result<T> {
        if self
            .state
            .content_states
            .get(version_id)
            .is_some_and(|s| *s != ContentState::Available)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::ContentPurged,
                "version content is no longer available in this history",
            ));
        }
        let version = self.state.versions.get(version_id).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::VersionNotFound,
                "version does not exist",
            )
        })?;
        let placement = self.state.placements.get(version_id).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::PlacementNotFound,
                "version placement missing",
            )
        })?;
        let mut first_error: Option<Error> = None;
        for slot in &placement.route_slot_ids {
            let d = self.disk(slot)?;
            let candidate = (|| {
                let disk = self.disk_resources.get(d)?;
                let lease = disk.root.admit_io()?;
                let scope = Arc::new(super::fresh_layout::containers::observe_replica(
                    &self.config, &self.disk_resources, &self.state,
                    version.file_id.parse()?, d,
                )?);
                let material = VersionMaterial::current(&scope, version)?;
                let metadata = MetadataReadContext::for_disk(
                    &self.config,
                    &disk.seal,
                    d.metadata_member.then_some(self.metadata_integrity.as_ref()),
                );
                crate::journal::verify_source_publication(
                    &material, &mut |_| lease.check_running(), Some(&metadata),
                )?;
                let result = read(&scope, &material, version, &lease)?;
                scope.verify()?;
                Ok(result)
            })();
            match candidate {
                Ok(replica) => return Ok(replica),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        Err(match first_error {
            Some(error) => error.context(
                storage_contract::ErrorCode::ContentUnavailable,
                "no verified complete replica is available",
            ),
            None => Error::new(
                storage_contract::ErrorCode::ContentUnavailable,
                "version placement has no replica routes",
            ),
        })
    }

    pub fn open_content(&self, version_id: &str) -> Result<OpenedContent> {
        let (file, version, _) = self.verified_replica(version_id)?;
        let mut reads = self
            .reads
            .lock()
            .expect("invariant: read lease lock poisoned");
        reads.acquire(version_id.into())?;
        drop(reads);
        Ok(OpenedContent {
            file: file.into(),
            size: version.logical_size,
            content_hash: version.content_hash,
            lease: ReadLease {
                version: version_id.into(),
                reads: Arc::clone(&self.reads),
            },
        })
    }
    pub fn prepare_workspace_content(
        &self,
        workspace: &str,
        item: &str,
        principal: &str,
    ) -> Result<WorkspaceReadPlan> {
        let w = self.workspace(workspace)?;
        if w.principal != principal {
            return Err(Error::new(
                storage_contract::ErrorCode::PermissionDenied,
                "workspace belongs to another principal",
            ));
        }
        self.repository_for_branch(&w.branch_id)?;
        let staged = w.files.get(item).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::ItemNotFound,
                "workspace item not found",
            )
        })?;
        let file = self
            .planning_context()
            .staged_source(&w, staged)?
            .open_leased()?;
        let key = staged
            .base_version_id
            .clone()
            .unwrap_or_else(|| format!("workspace:{workspace}:{item}"));
        self.reads
            .lock()
            .expect("invariant: read lease lock poisoned")
            .acquire(key.clone())?;
        Ok(WorkspaceReadPlan(OpenedContent {
            file,
            size: staged.size,
            content_hash: staged.content_hash,
            lease: ReadLease {
                version: key,
                reads: Arc::clone(&self.reads),
            },
        }))
    }
    pub fn open_workspace_content(
        &self,
        workspace: &str,
        item: &str,
        principal: &str,
    ) -> Result<OpenedContent> {
        self.prepare_workspace_content(workspace, item, principal)?
            .open()
    }
    pub fn read_checksums(&self, version: &str) -> Result<Vec<u8>> {
        self.with_replica(version, |_scope, material, version, lease| {
            let (file, checks, manifest) = open_read_payload(material, version, lease)?;
            verify_read_file(file, manifest, version)?;
            Ok(checks)
        })
    }
    pub fn export_version(&self, version_id: &str, destination: &Path) -> Result<()> {
        self.with_replica(version_id, |_scope, material, version, lease| {
            let (file, _, _) = open_read_payload(material, version, lease)?;
            match verified_full_copy(file, destination, version, version.logical_size) {
                Ok(_) => Ok(Ok(())),
                Err(CopyFailure::Source(error)) => Err(error),
                Err(CopyFailure::Destination(error)) => Ok(Err(error)),
            }
        })?
    }
    pub fn check(&self) -> Result<CheckReport> {
        let mut result = CheckReport {
            checked_versions: 0,
            healthy_replicas: 0,
            tombstoned_versions: 0,
            issues: Vec::new(),
        };
        for d in &self.config.disks {
            let checked = if d.metadata_member {
                self.metadata_integrity
                    .observe(identity(&self.config, d))
                    .and_then(|identity| {
                        let metadata = self
                            .metadata_integrity
                            .read_context(&self.config, &identity);
                        offline_check_in_context(&d.root, Some(&metadata))
                    })
            } else {
                offline_check(&d.root)
            };
            match checked {
                Ok(r) => {
                    result.checked_versions += r.checked_versions;
                    result.healthy_replicas += r.healthy_replicas;
                    result.tombstoned_versions += r.tombstoned_versions;
                    result
                        .issues
                        .extend(r.issues.into_iter().map(|s| format!("{}: {s}", d.slot_id)));
                }
                Err(e) => result.issues.push(format!("{}: {e}", d.slot_id)),
            }
        }
        Ok(result)
    }
}

/// The named proof is itself a managed resource, not a side read authorized by
/// the payload's lease. Cancellation is observed between 64 KiB reads even for
/// a maximum-sized manifest.
fn read_checksums_leased(material: &VersionMaterial<'_>) -> Result<Vec<u8>> {
    let version = material.version();
    let length = checksum_manifest_length(version)?;
    let mut file = material.root()?.open_leased_regular(
        &material.directory()?.join(crate::persist::layout::VERSION_CHECKSUMS.as_path()),
    )?;
    if file.metadata()?.len() != length as u64 {
        return Err(Error::new(
            ErrorCode::ChecksumMismatch,
            "checksum manifest length differs",
        ));
    }
    let mut checks = vec![0; length];
    for chunk in checks.chunks_mut(64 * 1024) {
        file.read_exact(chunk)?;
    }
    if file.read(&mut [0; 1])? != 0
        || blake3::hash(&checks).to_hex().as_str() != version.checksum_manifest_hash
    {
        return Err(Error::new(
            ErrorCode::ChecksumMismatch,
            "checksum manifest digest differs",
        ));
    }
    Ok(checks)
}

fn open_read_payload_file(
    material: &VersionMaterial<'_>,
    version: &FileVersion,
    lease: &DiskIoLease,
) -> Result<DiskFile> {
    lease.check_running()?;
    let path = material.body_path()?;
    let file = material.container()?.open_leased_regular(&path)?;
    if file.metadata()?.len() != version.logical_size {
        return Err(Error::new(
            ErrorCode::ChecksumMismatch,
            "payload length differs",
        ));
    }
    Ok(file)
}

fn open_read_payload(
    material: &VersionMaterial<'_>,
    version: &FileVersion,
    lease: &DiskIoLease,
) -> Result<(DiskFile, Vec<u8>, ChecksumManifest)> {
    let checks = read_checksums_leased(material)?;
    let manifest = ChecksumManifest::decode(&checks)?;
    if manifest.logical_size != version.logical_size
        || manifest.chunk_bytes != version.checksum_chunk_bytes
    {
        return Err(Error::new(
            ErrorCode::ChecksumMismatch,
            "checksum header differs from version",
        ));
    }
    let file = open_read_payload_file(material, version, lease)?;
    Ok((file, checks, manifest))
}

fn verify_read_payload(
    material: &VersionMaterial<'_>,
    version: &FileVersion,
    lease: &DiskIoLease,
) -> Result<DiskFile> {
    let (file, _, manifest) = open_read_payload(material, version, lease)?;
    verify_read_file(file, manifest, version)
}

fn verify_read_file(
    mut file: DiskFile,
    manifest: ChecksumManifest,
    version: &FileVersion,
) -> Result<DiskFile> {
    let mut full = blake3::Hasher::new();
    let mut buffer = vec![0; manifest.chunk_bytes as usize];
    let mut remaining = version.logical_size;
    for expected in manifest.chunks {
        let length = remaining.min(buffer.len() as u64) as usize;
        for block in buffer[..length].chunks_mut(64 * 1024) {
            file.read_exact(block)?;
        }
        if blake3::hash(&buffer[..length]).as_bytes() != &expected {
            return Err(Error::new(
                ErrorCode::ChecksumMismatch,
                "payload chunk differs",
            ));
        }
        full.update(&buffer[..length]);
        remaining -= length as u64;
    }
    if file.read(&mut [0; 1])? != 0 || file.metadata()?.len() != version.logical_size {
        return Err(Error::new(
            ErrorCode::ChecksumMismatch,
            "payload length changed during verification",
        ));
    }
    if ContentHash::from_digest(full.finalize()) != version.content_hash {
        return Err(Error::new(
            ErrorCode::ChecksumMismatch,
            "complete payload hash differs",
        ));
    }
    Ok(file)
}

/// A metadata-only snapshot acquired while the caller serializes Engine access.
/// The lease is acquired before releasing that lock, so purge cannot remove the
/// selected version while publication evidence and payload handles are opened.
pub struct ContentReadPlan {
    state: Arc<State>,
    config: ValidatedConfig,
    disk_resources: Arc<DiskResources>,
    disks: Vec<DiskConfig>,
    version: FileVersion,
    lease: ReadLease,
    metadata_integrity: Arc<super::provenance::MetadataIntegrity>,
}

pub struct RangeContent {
    pub fallback_files: Vec<ReadFile>,
    pub file: ReadFile,
    pub size: u64,
    pub content_hash: crate::ContentHash,
    pub manifest: ChecksumManifest,
    pub lease: ReadLease,
}

impl Engine {
    /// Does no filesystem access and reads no payload bytes.
    pub fn prepare_content(&self, version_id: &str) -> Result<ContentReadPlan> {
        self.prepare_content_route(version_id, None)
    }

    /// Read only this backup occurrence, never a different replica of the version.
    /// The normal publication and chunk-proof path still decides whether it can be read.
    pub fn prepare_backup_content(
        &self,
        version_id: storage_contract::VersionId,
        slot: &name_policy::SlotId,
    ) -> Result<ContentReadPlan> {
        self.prepare_content_route(&version_id.to_string(), Some(slot))
    }

    fn prepare_content_route(
        &self,
        version_id: &str,
        backup_slot: Option<&name_policy::SlotId>,
    ) -> Result<ContentReadPlan> {
        if self
            .state
            .content_states
            .get(version_id)
            .is_some_and(|s| *s != ContentState::Available)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::ContentPurged,
                "version content is no longer available in this history",
            ));
        }
        let version = self
            .state
            .versions
            .get(version_id)
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::VersionNotFound,
                    "version does not exist",
                )
            })?
            .clone();
        let placement = self.state.placements.get(version_id).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::PlacementNotFound,
                "version placement missing",
            )
        })?;
        let disks = if let Some(slot) = backup_slot {
            let disk = self.disk(slot.as_str())?;
            if disk.level == 1 {
                return Err(BackupReadError::Primary.into());
            }
            let generation = placement.binding_generations.get(&disk.slot_id);
            if !placement.route_slot_ids.contains(&disk.slot_id)
                || generation != Some(&disk.binding_generation)
            {
                return Err(BackupReadError::MissingReplica.into());
            }
            vec![disk.clone()]
        } else {
            placement
                .route_slot_ids
                .iter()
                .map(|slot| self.disk(slot).cloned())
                .collect::<Result<Vec<_>>>()?
        };
        self.reads
            .lock()
            .expect("invariant: read lease lock poisoned")
            .acquire(version_id.into())?;
        Ok(ContentReadPlan {
            state: Arc::clone(&self.state),
            metadata_integrity: Arc::clone(&self.metadata_integrity),
            config: self.config.clone(),
            disk_resources: Arc::clone(&self.disk_resources),
            disks,
            version,
            lease: ReadLease {
                version: version_id.into(),
                reads: Arc::clone(&self.reads),
            },
        })
    }
}

impl ContentReadPlan {
    /// An explicit observation of current named members, not a held-resource
    /// content read. Keep the read lease alive throughout the complete scan.
    pub(crate) fn health(&self, target_copies: u32) -> storage_contract::CurrentHealth {
        let healthy = self
            .disks
            .iter()
            .filter(|disk| self.observe_replica_health(disk).is_ok())
            .count();
        if healthy == 0 {
            storage_contract::CurrentHealth::Unavailable
        } else if healthy < target_copies as usize {
            storage_contract::CurrentHealth::Degraded
        } else {
            storage_contract::CurrentHealth::Healthy
        }
    }

    /// Health is a named-directory observation, but it is still an operation on
    /// an admitted binding. A detached pre-exit plan cannot reopen its old path.
    fn observe_replica_health(&self, disk: &DiskConfig) -> Result<()> {
        let handle = self.disk_resources.get(disk)?;
        let lease = handle.root.admit_io()?;
        lease.check_running()?;
        let identity = validate_seal_observed(&self.config, disk, &mut |_| lease.check_running())
            .and_then(|identity| validate_stored_seal(&handle.seal, &identity));
        if disk.metadata_member {
            self.metadata_integrity.observe(identity)?;
        } else {
            identity?;
        }
        lease.check_running()?;
        // Named identity observation must not turn later proof/body reads into
        // raw pathname opens. Both verifiers retain the same managed binding.
        let (scope, proof_lease) = self.authenticate_replica(disk)?;
        let material = VersionMaterial::current(&scope, &self.version)?;
        drop(verify_read_payload(
            &material, &self.version, &proof_lease,
        )?);
        scope.verify()?;
        proof_lease.check_running()?;
        lease.check_running()
    }

    /// Encoded proof size for admission before allocating or reading disk bytes.
    /// Rejects sizes beyond the current checksum format's resource ceiling.
    pub fn checksum_manifest_bytes(&self) -> Result<usize> {
        checksum_manifest_length(&self.version)
    }

    fn authenticate_replica(
        &self,
        disk: &DiskConfig,
    ) -> Result<(Arc<crate::container_storage::ObservedContainerReplica>, DiskIoLease)> {
        let handle = self.disk_resources.get(disk)?;
        let lease = handle.root.admit_io()?;
        let scope = Arc::new(super::fresh_layout::containers::observe_replica(
            &self.config, &self.disk_resources, &self.state, self.version.file_id.parse()?, disk,
        )?);
        let material = VersionMaterial::current(&scope, &self.version)?;
        let metadata = MetadataReadContext::for_disk(
            &self.config, &handle.seal,
            disk.metadata_member.then_some(self.metadata_integrity.as_ref()),
        );
        crate::journal::verify_source_publication(
            &material, &mut |_| lease.check_running(), Some(&metadata),
        )?;
        Ok((scope, lease))
    }

    /// One decoded proof for all exact-version replicas; each member's local
    /// manifest must still authenticate before its held payload is admitted.
    /// Consumers must verify each complete chunk before exposing its bytes.
    pub fn open(self) -> Result<RangeContent> {
        let mut selected = None;
        let mut fallback_files = Vec::new();
        let mut first_error: Option<Error> = None;
        for disk in &self.disks {
            let candidate = (|| {
                let (scope, lease) = self.authenticate_replica(disk)?;
                let material = VersionMaterial::current(&scope, &self.version)?;
                if selected.is_some() {
                    // Digest equality binds these bytes to the same descriptor;
                    // decoding the identical hash array again adds no proof.
                    drop(read_checksums_leased(&material)?);
                    let file = open_read_payload_file(&material, &self.version, &lease)?;
                    scope.verify()?;
                    fallback_files.push(ReadFile::in_container(file, Arc::clone(&scope)));
                } else {
                    let (file, checks, manifest) =
                        open_read_payload(&material, &self.version, &lease)?;
                    drop(checks);
                    scope.verify()?;
                    selected = Some((ReadFile::in_container(file, Arc::clone(&scope)), manifest));
                }
                Ok(())
            })();
            match candidate {
                Ok(()) => (),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        let (file, manifest) = selected.ok_or_else(|| match first_error {
            Some(error) => error.context(
                storage_contract::ErrorCode::ContentUnavailable,
                "no authenticated replica is available",
            ),
            None => Error::new(
                storage_contract::ErrorCode::ContentUnavailable,
                "version placement has no replica routes",
            ),
        })?;
        Ok(RangeContent {
            fallback_files,
            file,
            size: self.version.logical_size,
            content_hash: self.version.content_hash,
            manifest,
            lease: self.lease,
        })
    }
    pub fn checksums(self) -> Result<Vec<u8>> {
        let mut first_error = None;
        for disk in &self.disks {
            match self
                .authenticate_replica(disk)
                .and_then(|(scope, lease)| {
                    let material = VersionMaterial::current(&scope, &self.version)?;
                    let payload = open_read_payload(
                        &material, &self.version, &lease,
                    )?;
                    scope.verify()?;
                    Ok(payload)
                }) {
                Ok((_, checks, _)) => return Ok(checks),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        Err(match first_error {
            Some(error) => error.context(
                storage_contract::ErrorCode::ContentUnavailable,
                "no authenticated replica is available",
            ),
            None => Error::new(
                storage_contract::ErrorCode::ContentUnavailable,
                "version placement has no replica routes",
            ),
        })
    }
}

/// Holds the exact staged inode across workspace changes; hashing happens after
/// releasing the engine lock, and replacement cannot redirect this handle.
pub struct WorkspaceReadPlan(OpenedContent);
impl WorkspaceReadPlan {
    pub fn open(mut self) -> Result<OpenedContent> {
        let mut hash = blake3::Hasher::new();
        let mut buffer = [0; 65536];
        let mut remaining = self.0.size;
        loop {
            self.0.file.check_running().map_err(|source| {
                source.context(
                    ErrorCode::ContentUnavailable,
                    "workspace content is unavailable",
                )
            })?;
            let count = self.0.file.read(&mut buffer).map_err(|source| {
                Error::from(source).context(
                    ErrorCode::ContentUnavailable,
                    "workspace payload read failed",
                )
            })?;
            if count == 0 {
                break;
            }
            remaining = remaining.checked_sub(count as u64).ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::ChecksumMismatch,
                    "workspace full file length differs",
                )
            })?;
            hash.update(&buffer[..count]);
        }
        if remaining != 0 || ContentHash::from_digest(hash.finalize()) != self.0.content_hash {
            return Err(Error::new(
                storage_contract::ErrorCode::ChecksumMismatch,
                "workspace full file was modified outside the engine",
            ));
        }
        use std::io::{Seek, SeekFrom};
        self.0.file.seek(SeekFrom::Start(0)).map_err(|source| {
            Error::from(source).context(
                ErrorCode::ContentUnavailable,
                "workspace payload seek failed",
            )
        })?;
        Ok(self.0)
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/reads/tests.rs"]
mod tests;
