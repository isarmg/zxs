//! Verified replicas, read leases, offline export and disk observations.
use super::provenance::MetadataReadContext;
use super::{Engine, OpenedContent, ReadLease, ReadSource};
use crate::{disk::*, *};
use std::path::Path;
use std::{io::Read, path::PathBuf, sync::Arc};

/// A resolved replica path and immutable version, created only by the complete
/// existing read-proof path below. Callers may retain it but cannot substitute
/// another file path or manufacture this capability from a disk seal.
pub(crate) struct VerifiedReplicaSource {
    owner: Arc<DiskHandle>,
    path: PathBuf,
    version: FileVersion,
}
impl VerifiedReplicaSource {
    pub(super) fn owner(&self) -> &Arc<DiskHandle> {
        &self.owner
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
        Ok((
            ReadSource::on_disk(&source.owner, source.path),
            source.version,
        ))
    }

    pub(crate) fn valid_backup_replica(&self, version_id: &str) -> Result<ReadSource> {
        self.validated_replica_source(version_id)
            .map(|source| ReadSource::VerifiedReplica(Arc::new(source)))
    }

    fn validated_replica_source(&self, version_id: &str) -> Result<VerifiedReplicaSource> {
        self.with_replica(version_id, |disk, directory, version, lease| {
            verify_read_payload(disk, directory, version, lease)?;
            let content = crate::container_storage::content_path_at(
                &disk.root,
                directory,
                version,
                &mut |_| lease.check_running(),
            )?;
            Ok(VerifiedReplicaSource {
                owner: Arc::clone(disk),
                path: content,
                version: version.clone(),
            })
        })
    }
    fn verified_replica(&self, version_id: &str) -> Result<(DiskFile, FileVersion, PathBuf)> {
        self.with_replica(version_id, |disk, directory, version, lease| {
            let mut file = verify_read_payload(disk, directory, version, lease)?;
            use std::io::{Seek, SeekFrom};
            file.seek(SeekFrom::Start(0))?;
            Ok((file, version.clone(), directory.to_path_buf()))
        })
    }

    fn with_replica<T>(
        &self,
        version_id: &str,
        mut read: impl FnMut(&Arc<DiskHandle>, &Path, &FileVersion, &DiskIoLease) -> Result<T>,
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
                let dir = version_dir(d, &version.history_tree_id, version_id);
                let metadata = MetadataReadContext::for_disk(
                    &self.config,
                    &disk.seal,
                    d.metadata_member
                        .then_some(self.metadata_integrity.as_ref()),
                );
                crate::journal::verify_local_publication_at(
                    &disk.root,
                    &dir,
                    version,
                    &mut |_| lease.check_running(),
                    Some(&metadata),
                )?;
                read(disk, &dir, version, &lease)
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
        self.with_replica(version, |disk, directory, version, lease| {
            let (file, checks, manifest) = open_read_payload(disk, directory, version, lease)?;
            verify_read_file(file, manifest, version)?;
            Ok(checks)
        })
    }
    pub fn export_version(&self, version_id: &str, destination: &Path) -> Result<()> {
        self.with_replica(version_id, |disk, directory, version, lease| {
            let (file, _, _) = open_read_payload(disk, directory, version, lease)?;
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
fn read_checksums_leased(
    disk: &DiskHandle,
    directory: &Path,
    version: &FileVersion,
) -> Result<Vec<u8>> {
    let length = checksum_manifest_length(version)?;
    let mut file = disk
        .root
        .open_leased_regular(&directory.join("checksums.bin"))?;
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
    disk: &DiskHandle,
    directory: &Path,
    version: &FileVersion,
    lease: &DiskIoLease,
) -> Result<DiskFile> {
    let path =
        crate::container_storage::content_path_at(&disk.root, directory, version, &mut |_| {
            lease.check_running()
        })?;
    let file = disk.root.open_leased_regular(&path)?;
    if file.metadata()?.len() != version.logical_size {
        return Err(Error::new(
            ErrorCode::ChecksumMismatch,
            "payload length differs",
        ));
    }
    Ok(file)
}

fn open_read_payload(
    disk: &DiskHandle,
    directory: &Path,
    version: &FileVersion,
    lease: &DiskIoLease,
) -> Result<(DiskFile, Vec<u8>, ChecksumManifest)> {
    let checks = read_checksums_leased(disk, directory, version)?;
    let manifest = ChecksumManifest::decode(&checks)?;
    if manifest.logical_size != version.logical_size
        || manifest.chunk_bytes != version.checksum_chunk_bytes
    {
        return Err(Error::new(
            ErrorCode::ChecksumMismatch,
            "checksum header differs from version",
        ));
    }
    let file = open_read_payload_file(disk, directory, version, lease)?;
    Ok((file, checks, manifest))
}

fn verify_read_payload(
    disk: &DiskHandle,
    directory: &Path,
    version: &FileVersion,
    lease: &DiskIoLease,
) -> Result<DiskFile> {
    let (file, _, manifest) = open_read_payload(disk, directory, version, lease)?;
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
    config: ValidatedConfig,
    disk_resources: Arc<DiskResources>,
    disks: Vec<DiskConfig>,
    version: FileVersion,
    lease: ReadLease,
    metadata_integrity: Arc<super::provenance::MetadataIntegrity>,
}

pub struct RangeContent {
    pub fallback_files: Vec<DiskFile>,
    pub file: DiskFile,
    pub size: u64,
    pub content_hash: crate::ContentHash,
    pub manifest: ChecksumManifest,
    pub lease: ReadLease,
}

impl Engine {
    /// Does no filesystem access and reads no payload bytes.
    pub fn prepare_content(&self, version_id: &str) -> Result<ContentReadPlan> {
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
        let disks = placement
            .route_slot_ids
            .iter()
            .map(|slot| self.disk(slot).cloned())
            .collect::<Result<Vec<_>>>()?;
        self.reads
            .lock()
            .expect("invariant: read lease lock poisoned")
            .acquire(version_id.into())?;
        Ok(ContentReadPlan {
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
        let (handle, directory, proof_lease) = self.authenticate_replica(disk)?;
        drop(handle.verify_payload(&directory, &self.version)?);
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
    ) -> Result<(&DiskHandle, PathBuf, DiskIoLease)> {
        let handle = self.disk_resources.get(disk)?;
        let lease = handle.root.admit_io()?;
        let dir = version_dir(
            disk,
            &self.version.history_tree_id,
            &self.version.version_id,
        );
        let metadata = MetadataReadContext::for_disk(
            &self.config,
            &handle.seal,
            disk.metadata_member
                .then_some(self.metadata_integrity.as_ref()),
        );
        crate::journal::verify_local_publication_at(
            &handle.root,
            &dir,
            &self.version,
            &mut |_| lease.check_running(),
            Some(&metadata),
        )?;
        Ok((handle, dir, lease))
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
                let (handle, dir, lease) = self.authenticate_replica(disk)?;
                if selected.is_some() {
                    // Digest equality binds these bytes to the same descriptor;
                    // decoding the identical hash array again adds no proof.
                    drop(read_checksums_leased(handle, &dir, &self.version)?);
                    let file = open_read_payload_file(handle, &dir, &self.version, &lease)?;
                    fallback_files.push(file);
                } else {
                    let (file, checks, manifest) =
                        open_read_payload(handle, &dir, &self.version, &lease)?;
                    drop(checks);
                    selected = Some((file, manifest));
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
                .and_then(|(handle, dir, lease)| {
                    open_read_payload(handle, &dir, &self.version, &lease)
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
