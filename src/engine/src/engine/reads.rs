//! Verified replicas, read leases, offline export and disk observations.
use super::{Engine, OpenedContent, ReadLease};
use crate::{disk::*, journal::verify_local_publication, *};
use std::path::Path;
use std::{io::Read, path::PathBuf, sync::Arc};

impl Engine {
    pub(crate) fn used_bytes(&self, slot: &str) -> u64 {
        self.planning_context().used_bytes(slot)
    }
    pub(crate) fn valid_replica(&self, version_id: &str) -> Result<(PathBuf, FileVersion)> {
        let (_, version, directory) = self.verified_replica(version_id)?;
        let content = crate::container_storage::content_path(&directory, &version)?;
        Ok((content, version))
    }
    fn verified_replica(&self, version_id: &str) -> Result<(std::fs::File, FileVersion, PathBuf)> {
        self.with_replica(version_id, |directory, version| {
            let mut file = verify_payload_held(directory, version)?;
            use std::io::{Seek, SeekFrom};
            file.seek(SeekFrom::Start(0))?;
            Ok((file, version.clone(), directory.to_path_buf()))
        })
    }

    fn with_replica<T>(
        &self,
        version_id: &str,
        mut read: impl FnMut(&Path, &FileVersion) -> Result<T>,
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
                validate_seal(&self.config, d)?;
                let dir = version_dir(d, &version.history_tree_id, version_id);
                verify_local_publication(&dir, version)?;
                read(&dir, version)
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
        *reads.entry(version_id.into()).or_default() += 1;
        drop(reads);
        Ok(OpenedContent {
            file,
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
        let file = open_regular(&staged.source_path)?;
        let key = staged
            .base_version_id
            .clone()
            .unwrap_or_else(|| format!("workspace:{workspace}:{item}"));
        *self
            .reads
            .lock()
            .expect("invariant: read lease lock poisoned")
            .entry(key.clone())
            .or_default() += 1;
        Ok(WorkspaceReadPlan(OpenedContent {
            file,
            size: staged.size,
            content_hash: staged.content_hash.clone(),
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
        let (_, version, directory) = self.verified_replica(version)?;
        let checks = read_bytes(&directory.join("checksums.bin"), MAX_CHECKSUM_MANIFEST)?;
        // Bind the returned bytes too, not an earlier read of this pathname.
        if blake3::hash(&checks).to_hex().as_str() != version.checksum_manifest_hash {
            return Err(Error::new(
                storage_contract::ErrorCode::ChecksumMismatch,
                "checksum manifest digest differs",
            ));
        }
        Ok(checks)
    }
    pub fn export_version(&self, version_id: &str, destination: &Path) -> Result<()> {
        self.with_replica(version_id, |directory, version| {
            let (file, _) = open_payload(directory, version)?;
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
            match offline_check(&d.root) {
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

/// A metadata-only snapshot acquired while the caller serializes Engine access.
/// The lease is acquired before releasing that lock, so purge cannot remove the
/// selected version while publication evidence and payload handles are opened.
pub struct ContentReadPlan {
    config: Arc<ValidatedConfig>,
    disks: Vec<DiskConfig>,
    version: FileVersion,
    lease: ReadLease,
}

pub struct RangeContent {
    pub fallback_files: Vec<std::fs::File>,
    pub file: std::fs::File,
    pub size: u64,
    pub content_hash: String,
    pub manifest: ChecksumManifest,
    pub lease: ReadLease,
}

impl Engine {
    /// Does no filesystem access and reads no payload bytes.
    pub fn prepare_content(&self, version_id: &str) -> Result<ContentReadPlan> {
        self.prepare_content_with_config(version_id, Arc::new(self.config.clone()))
    }

    pub(super) fn prepare_content_with_config(
        &self,
        version_id: &str,
        config: Arc<ValidatedConfig>,
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
        let disks = placement
            .route_slot_ids
            .iter()
            .map(|slot| self.disk(slot).cloned())
            .collect::<Result<Vec<_>>>()?;
        *self
            .reads
            .lock()
            .expect("invariant: read lease lock poisoned")
            .entry(version_id.into())
            .or_default() += 1;
        Ok(ContentReadPlan {
            config,
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
    /// A fresh, complete verification, never a cached protection assertion.
    /// Keep the plan (and therefore its read lease) alive for the entire scan.
    pub(super) fn health(&self, target_copies: u32) -> storage_contract::CurrentHealth {
        let healthy = self
            .disks
            .iter()
            .filter(|disk| {
                let directory = version_dir(
                    disk,
                    &self.version.history_tree_id,
                    &self.version.version_id,
                );
                validate_seal(&self.config, disk).is_ok()
                    && verify_local_publication(&directory, &self.version).is_ok()
                    && verify_payload(&directory, &self.version).is_ok()
            })
            .count();
        if healthy == 0 {
            storage_contract::CurrentHealth::Unavailable
        } else if healthy < target_copies as usize {
            storage_contract::CurrentHealth::Degraded
        } else {
            storage_contract::CurrentHealth::Healthy
        }
    }

    fn authenticated_replicas(&self) -> Result<Vec<(std::fs::File, Vec<u8>, ChecksumManifest)>> {
        let mut replicas = Vec::new();
        let mut first_error: Option<Error> = None;
        for disk in &self.disks {
            let candidate = (|| {
                validate_seal(&self.config, disk)?;
                let dir = version_dir(
                    disk,
                    &self.version.history_tree_id,
                    &self.version.version_id,
                );
                verify_local_publication(&dir, &self.version)?;
                let checks = read_bytes(&dir.join("checksums.bin"), MAX_CHECKSUM_MANIFEST)?;
                if blake3::hash(&checks).to_hex().as_str() != self.version.checksum_manifest_hash {
                    return Err(Error::new(
                        storage_contract::ErrorCode::ChecksumMismatch,
                        "checksum manifest digest differs",
                    ));
                }
                let manifest = ChecksumManifest::decode(&checks)?;
                if manifest.logical_size != self.version.logical_size
                    || manifest.chunk_bytes != self.version.checksum_chunk_bytes
                {
                    return Err(Error::new(
                        storage_contract::ErrorCode::ChecksumMismatch,
                        "checksum header differs from version",
                    ));
                }
                let file = open_regular(&crate::container_storage::content_path(
                    &dir,
                    &self.version,
                )?)?;
                if file.metadata()?.len() != self.version.logical_size {
                    return Err(Error::new(
                        storage_contract::ErrorCode::ChecksumMismatch,
                        "payload length differs",
                    ));
                }
                Ok((file, checks, manifest))
            })();
            match candidate {
                Ok(candidate) => replicas.push(candidate),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        if replicas.is_empty() {
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
        } else {
            Ok(replicas)
        }
    }
    /// Authenticate the checksum manifest without reading the payload. Consumers
    /// must verify each complete checksum chunk before exposing any of its bytes.
    pub fn open(self) -> Result<RangeContent> {
        let mut replicas = self.authenticated_replicas()?.into_iter();
        let (file, _, manifest) = replicas.next().expect("nonempty replicas");
        let fallback_files = replicas.map(|(file, _, _)| file).collect();
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
        self.authenticated_replicas()
            .map(|replicas| replicas.into_iter().next().expect("nonempty replicas").1)
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
            let count = self.0.file.read(&mut buffer)?;
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
        if remaining != 0 || hash.finalize().to_hex().as_str() != self.0.content_hash {
            return Err(Error::new(
                storage_contract::ErrorCode::ChecksumMismatch,
                "workspace full file was modified outside the engine",
            ));
        }
        use std::io::{Seek, SeekFrom};
        self.0.file.seek(SeekFrom::Start(0))?;
        Ok(self.0)
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/reads/tests.rs"]
mod tests;
