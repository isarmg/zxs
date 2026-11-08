//! Owned immutable history preparation; no publication or topology authority.
use super::*;
use crate::engine::ContentReadPlan;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) struct HistoryMetadata {
    pub(super) manifest: HistoryManifest,
    pub(super) proof: PackageProof,
    pub(super) graph_objects: Vec<AuthenticatedMetadataObject>,
}

impl Engine {
    pub(super) fn history_package_metadata(&self, history: &str) -> Result<HistoryMetadata> {
        self.repository_for_history(history)?;
        crate::node_retirement::require_portable_history(&self.state, history)?;
        let manifest = HistoryManifest::project_state(&self.state, history)?;
        let mut proof = None;
        let mut graph_objects = Vec::new();
        let crate::engine::EvidenceMembers {
            members,
            first_error,
        } = self.trusted_evidence_members();
        for disk in &members {
            for (record, _) in self
                .metadata_integrity
                .observe(scan_configured_at(
                    &self.disk_resources.get(disk)?.root,
                    disk,
                    &self.config,
                ))?
                .records
                .into_iter()
                .rev()
                .filter(|(r, _)| r.record_type == RecordType::Publication)
            {
                let candidate: Candidate =
                    crate::publication_evidence::verified_object_observed_at(
                        &self.disk_resources,
                        &members,
                        "candidate",
                        &record.candidate_hash,
                        &mut |error| self.metadata_integrity.fault(error),
                    )?;
                self.verify_candidate_membership(&candidate)?;
                if candidate.pool_id != self.config.pool_id || candidate.sequence != record.sequence
                {
                    return Err(Error::new(
                        storage_contract::ErrorCode::PublicationInvalid,
                        "candidate differs from journal or pool",
                    ));
                }
                let (value, history_root_proof) = crate::metadata_map::read_proof(
                    &candidate.history_roots,
                    history,
                    |kind, hash| {
                        crate::publication_evidence::verified_graph_object_observed_at(
                            &self.disk_resources,
                            &members,
                            kind,
                            hash,
                            &mut |error| self.metadata_integrity.fault(error),
                        )
                    },
                )?;
                let Some(value) = value else {
                    continue;
                };
                let hash = value
                    .as_str()
                    .ok_or_else(|| {
                        Error::new(
                            ErrorCode::ObjectCorrupt,
                            "history catalog digest is not text",
                        )
                    })?
                    .to_owned();
                let (bound, objects) = bound_history_graph_observed_at(
                    &self.disk_resources,
                    &members,
                    &hash,
                    &mut |error| self.metadata_integrity.fault(error),
                )?;
                if canonical_json(&bound)? != canonical_json(&manifest)? {
                    continue;
                }
                graph_objects = objects;
                let publication: Publication =
                    crate::publication_evidence::verified_object_observed_at(
                        &self.disk_resources,
                        &members,
                        "publication",
                        &record.object_hash,
                        &mut |error| self.metadata_integrity.fault(error),
                    )?;
                let publication_seal: PublicationSeal =
                    crate::publication_evidence::verified_publication_seal_observed_at(
                        &self.disk_resources,
                        &members,
                        &candidate,
                        &publication,
                        &mut |error| self.metadata_integrity.fault(error),
                    )?;
                proof = Some(PackageProof {
                    manifest_hash: hash.clone(),
                    candidate,
                    publication,
                    publication_seal,
                    history_root_proof,
                });
                break;
            }
            if proof.is_some() {
                break;
            }
        }
        let proof = proof.ok_or_else(|| match first_error {
            Some(error) => error.context(
                storage_contract::ErrorCode::RecoveryRequired,
                "no published manifest matches the current history",
            ),
            None => Error::new(
                storage_contract::ErrorCode::RecoveryRequired,
                "no published manifest matches the current history",
            ),
        })?;
        Ok(HistoryMetadata {
            manifest,
            proof,
            graph_objects,
        })
    }
}

/// A frozen complete history and leases for every retained body. This cannot
/// publish, rebind a disk or decide that a replacement has become authoritative.
pub(crate) struct PreparedHistoryPackage {
    metadata: HistoryMetadata,
    generation: Option<Vec<u8>>,
    bodies: BTreeMap<String, ContentReadPlan>,
}

impl Engine {
    pub(crate) fn prepare_backup_history(&self, history: &str) -> Result<PreparedHistoryPackage> {
        let metadata = self.history_package_metadata(history)?;
        metadata.manifest.validate()?;
        let bodies = metadata
            .manifest
            .versions
            .keys()
            .filter(|id| metadata.manifest.content_states[*id] == ContentState::Available)
            .map(|id| Ok((id.clone(), self.prepare_content(id)?)))
            .collect::<Result<_>>()?;
        let generation = self
            .config
            .generation()
            .map(|generation| canonical_json(&generation.portable))
            .transpose()?;
        Ok(PreparedHistoryPackage {
            metadata,
            generation,
            bodies,
        })
    }
}

pub(crate) fn preparation_checkpoint(stop: &AtomicBool) -> Result<()> {
    if stop.load(Ordering::Acquire) {
        return Err(Error::new(
            ErrorCode::OperationAborted,
            "backup preparation stopped; durable evidence retained",
        ));
    }
    Ok(())
}

/// Probe from the held root one component at a time: an absent ancestor is
/// absence. Existence is not integrity admission: subsequent held reads or
/// private-directory opens reject links and other invalid object types.
pub(crate) fn preparation_path_exists(root: &DiskRoot, path: &Path) -> Result<bool> {
    let relative = path.strip_prefix(root.path()).map_err(|_| {
        Error::new(
            ErrorCode::UnsafePath,
            "prepared path is outside the held target",
        )
    })?;
    let mut current = root.path().to_path_buf();
    for component in relative.components() {
        current.push(component);
        if root.optional_metadata(&current)?.is_none() {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Idempotent immutable installation. A conflicting file is evidence, not a
/// reason to overwrite, delete, or silently restart the operation.
pub(crate) fn install_preparation_bytes(root: &DiskRoot, path: &Path, bytes: &[u8]) -> Result<()> {
    root.private_dir(
        path.parent()
            .expect("invariant: prepared object has parent"),
    )?;
    if root.optional_metadata(path)?.is_some() {
        if root.read_bytes(path, bytes.len())? != bytes {
            return Err(Error::new(
                ErrorCode::ObjectCollision,
                "prepared metadata differs from its frozen source",
            ));
        }
        root.open_regular(path)?.sync_all()?;
        root.sync_dir(path.parent().expect("invariant: metadata has parent"))
    } else {
        root.write_atomic(path, bytes, false)
    }
}

struct StoppableReader<'a> {
    source: ReadFile,
    stop: &'a AtomicBool,
}
impl std::io::Read for StoppableReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        preparation_checkpoint(self.stop).map_err(std::io::Error::other)?;
        let count = bytes.len().min(64 * 1024);
        std::io::Read::read(&mut self.source, &mut bytes[..count])
    }
}

impl PreparedHistoryPackage {
    pub(crate) fn history_id(&self) -> &str {
        &self.metadata.manifest.history.history_tree_id
    }
    pub(crate) fn manifest_hash(&self) -> &str {
        &self.metadata.proof.manifest_hash
    }
    pub(crate) fn body_count(&self) -> u64 {
        self.bodies.len() as u64
    }
    pub(crate) fn body_bytes(&self) -> Result<u64> {
        self.bodies.keys().try_fold(0u64, |total, id| {
            total
                .checked_add(self.metadata.manifest.versions[id].logical_size)
                .ok_or_else(|| {
                    Error::new(ErrorCode::ResourceLimit, "backup preparation size overflow")
                })
        })
    }
    pub(crate) fn remaining_bytes(&self, target: &DiskRoot) -> Result<u64> {
        let package = target.path().join("histories").join(self.history_id());
        let mut total = 0u64;
        let mut account = |path: PathBuf, bytes: u64| -> Result<()> {
            if !preparation_path_exists(target, &path)? {
                total = total
                    .checked_add(bytes)
                    .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "backup space overflow"))?;
            }
            Ok(())
        };
        for object in &self.metadata.graph_objects {
            let path = if object.kind() == "history-manifest" {
                package.join("manifest.json")
            } else {
                package_object(&package, object.kind(), object.hash())?
            };
            account(path, object.bytes().len() as u64)?;
        }
        account(
            package.join("publication.json"),
            canonical_json(&self.metadata.proof)?.len() as u64,
        )?;
        if let Some(bytes) = &self.generation {
            account(
                package.join("membership-generation.json"),
                bytes.len() as u64,
            )?;
        }
        for id in self.bodies.keys() {
            let version = &self.metadata.manifest.versions[id];
            let directory = package_version(&package, id);
            account(
                directory.join("content").join(&version.creation_name),
                version.logical_size,
            )?;
            account(
                directory.join("version.json"),
                canonical_json(version)?.len() as u64,
            )?;
            account(
                directory.join("checksums.bin"),
                checksum_manifest_length(version)? as u64,
            )?;
        }
        Ok(total)
    }
    pub(crate) fn copy(
        self,
        root: &DiskRoot,
        stop: &AtomicBool,
        mut progress: impl FnMut(u64) -> Result<()>,
        mut verifying: impl FnMut() -> Result<()>,
    ) -> Result<HistoryPackageReport> {
        preparation_checkpoint(stop)?;
        for object in &self.metadata.graph_objects {
            preparation_checkpoint(stop)?;
            let path = if object.kind() == "history-manifest" {
                root.path().join("manifest.json")
            } else {
                package_object(root.path(), object.kind(), object.hash())?
            };
            install_preparation_bytes(root, &path, object.bytes())?;
        }
        if let Some(generation) = &self.generation {
            install_preparation_bytes(
                root,
                &root.path().join("membership-generation.json"),
                generation,
            )?;
        }
        install_preparation_bytes(
            root,
            &root.path().join("publication.json"),
            &canonical_json(&self.metadata.proof)?,
        )?;
        for (id, plan) in self.bodies {
            preparation_checkpoint(stop)?;
            let version = &self.metadata.manifest.versions[&id];
            let directory = package_version(root.path(), &id);
            let path = directory.join("content").join(&version.creation_name);
            root.private_dir(&directory.join("content"))?;
            if root.optional_metadata(&path)?.is_none() {
                let source = plan.open()?;
                // Metadata is durable before the body can be installed, so a
                // resumed complete body never needs the source to invent proof.
                install_preparation_bytes(
                    root,
                    &directory.join("version.json"),
                    &canonical_json(version)?,
                )?;
                install_preparation_bytes(
                    root,
                    &directory.join("checksums.bin"),
                    &source.manifest.encode()?,
                )?;
                let mut input = StoppableReader {
                    source: source.file.into(),
                    stop,
                };
                root.copy_verified_bytes_synced(
                    &path,
                    &mut input,
                    platform_io::payload::Expectation {
                        bytes: version.logical_size,
                        blake3: Some(version.content_hash.digest()),
                    },
                )?;
            }
            // Never trust the persisted progress or a previously completed copy.
            verify_package_payload_at(root, &directory, version, &mut |_| {
                preparation_checkpoint(stop)
            })?;
            root.sync_dir(&directory)?;
            progress(version.logical_size)?;
        }
        preparation_checkpoint(stop)?;
        verifying()?;
        let (_, _, report) = load_package_at(root, &mut |_| preparation_checkpoint(stop))?;
        root.sync_dir(root.path())?;
        Ok(report)
    }
}

/// Cross-check a target-local history against the complete admitted snapshot.
/// A valid but older/partial history package cannot stand in for this task's unit.
pub(crate) fn verify_prepared_history_state(
    root: &DiskRoot,
    state: &AdmittedState,
    history: &str,
    pool: &str,
    stop: &AtomicBool,
) -> Result<HistoryPackageReport> {
    let (manifest, proof, report) = load_package_at(root, &mut |_| preparation_checkpoint(stop))?;
    let expected = HistoryManifest::project_state(state.state(), history)?;
    if proof.candidate.pool_id != pool
        || proof.candidate.sequence > state.state().sequence
        || canonical_json(&manifest)? != canonical_json(&expected)?
    {
        return Err(Error::new(
            ErrorCode::HistoryPackageInvalid,
            "prepared history does not match the frozen complete snapshot",
        ));
    }
    Ok(report)
}
