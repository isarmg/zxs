//! Independently recoverable, publication-bound history packages.
use crate::journal::RecordType;
use crate::{disk::*, journal::*, *};
use data_serialization::{Deserialize, Serialize};
use json_codec::json;
mod changes;
mod delta;
mod graph;
pub(crate) use graph::decode_history_root;
mod manifest;
mod state_read;
pub use manifest::HistoryManifest;
pub(crate) use state_read::{AdmittedState, RawState};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoryPackageReport {
    pub history_tree_id: String,
    pub repository_id: String,
    pub manifest_hash: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub publication_sequence: u64,
    pub commits: usize,
    pub versions: usize,
    pub full_files: usize,
    pub tombstones: usize,
    #[serialization(with = "storage_contract::u64_string")]
    pub full_bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct PackageProof {
    manifest_hash: String,
    candidate: Candidate,
    publication: Publication,
    publication_seal: PublicationSeal,
}
pub(crate) fn package_proof_bytes(
    candidate: &Candidate,
    publication: &Publication,
    seal: &PublicationSeal,
    maximum: usize,
) -> Result<usize> {
    Ok(encode_metadata(
        &PackageProof {
            manifest_hash: "0".repeat(64),
            candidate: candidate.clone(),
            publication: publication.clone(),
            publication_seal: seal.clone(),
        },
        maximum,
    )?
    .len())
}
#[cfg(test)]
pub(crate) fn store_history_manifests_bounded(
    members: &[DiskConfig],
    state: &State,
    maximum: usize,
) -> Result<BTreeMap<String, String>> {
    let (hashes, objects) = prepare_history_manifests(state, maximum)?;
    for object in objects {
        object.store(members)?;
    }
    Ok(hashes)
}
pub(crate) fn prepare_history_manifests(
    state: &State,
    maximum: usize,
) -> Result<(BTreeMap<String, String>, Vec<PreparedMetadataObject>)> {
    let mut hashes = BTreeMap::new();
    let mut objects = Vec::new();
    for id in state.histories.keys() {
        let manifest = HistoryManifest::from_state(state, id)?;
        let (hash, graph) = graph::prepare(&manifest, maximum)?;
        hashes.insert(id.clone(), hash);
        objects.extend(graph);
    }
    Ok((hashes, unique_prepared_objects(objects)?))
}

/// Reuse is offered only by the engine's same-process published baseline.
/// Every reused dependency is still read, hash/graph validated, admitted and
/// installed through the original collision checks and durability barriers.
pub(crate) fn prepare_history_manifests_incremental(
    before: &State,
    after: &State,
    previous_roots: &BTreeMap<String, String>,
    members: &[DiskConfig],
    maximum: usize,
) -> Result<(BTreeMap<String, String>, Vec<PreparedMetadataObject>)> {
    if previous_roots.keys().ne(before.histories.keys()) {
        return Err(Error::new(
            storage_contract::ErrorCode::StateInvalid,
            "published history baseline coverage differs",
        ));
    }
    let projection = delta::ProjectionDelta::between(before, after);
    let changed = match &projection {
        delta::ProjectionDelta::StableBranches(edits) => edits.keys().cloned().collect(),
        delta::ProjectionDelta::Full => changes::HistoryChanges::between(before, after).histories,
    };
    let mut hashes = BTreeMap::new();
    let mut objects = Vec::new();
    for id in after.histories.keys() {
        if changed.contains(id) || !previous_roots.contains_key(id) {
            let incremental = try_prepare_branch_update(
                members,
                previous_roots.get(id),
                id,
                &projection,
                maximum,
            )?;
            let (hash, graph) = match incremental {
                Some(graph) => graph,
                None => {
                    let manifest = HistoryManifest::from_state(after, id)?;
                    graph::prepare(&manifest, maximum)?
                }
            };
            hashes.insert(id.clone(), hash);
            objects.extend(graph);
        } else {
            let hash = &previous_roots[id];
            let (manifest, graph) = match bound_history_graph(members, hash) {
                Ok(graph) => graph,
                Err(_) => {
                    // The full publisher can recreate wholly missing replicas
                    // from State. Preserve that ability without replacing the
                    // candidate-bound root or bypassing store collision checks.
                    let manifest = HistoryManifest::from_state(after, id)?;
                    let (rebuilt_hash, rebuilt) = graph::prepare(&manifest, maximum)?;
                    if &rebuilt_hash != hash {
                        return Err(Error::new(
                            storage_contract::ErrorCode::ObjectCorrupt,
                            "reconstructed history differs from published baseline",
                        ));
                    }
                    hashes.insert(id.clone(), hash.clone());
                    objects.extend(rebuilt);
                    continue;
                }
            };
            if manifest.history.history_tree_id != *id {
                return Err(Error::new(
                    storage_contract::ErrorCode::ObjectCorrupt,
                    "published history baseline identity differs",
                ));
            }
            for object in graph {
                objects.push(object.into_prepared(maximum)?);
            }
            hashes.insert(id.clone(), hash.clone());
        }
    }
    Ok((hashes, unique_prepared_objects(objects)?))
}

fn try_prepare_branch_update(
    members: &[DiskConfig],
    previous: Option<&String>,
    id: &str,
    projection: &delta::ProjectionDelta,
    maximum: usize,
) -> Result<Option<(String, Vec<PreparedMetadataObject>)>> {
    let delta::ProjectionDelta::StableBranches(edits) = projection else {
        return Ok(None);
    };
    let (Some(edits), Some(previous)) = (edits.get(id), previous) else {
        return Ok(None);
    };
    let (manifest, objects) = match read_history_graph(members, previous) {
        Ok(graph) => graph,
        // Only this read-only source graph acquisition may fall back to the
        // original complete encoder for the admitted after State. It prepares
        // a new changed root, never substitutes a decided/unchanged root.
        // Update/admission errors below and later visitor/store/decision errors
        // propagate; none can enter this source-read fallback.
        Err(_) => return Ok(None),
    };
    if manifest.history.history_tree_id != id {
        return Err(Error::new(
            storage_contract::ErrorCode::ObjectCorrupt,
            "published history baseline identity differs",
        ));
    }
    graph::prepare_branch_update(manifest, objects, edits, maximum).map(Some)
}

// First occurrence retains dependency order across the frozen history graphs.
fn unique_prepared_objects(
    objects: Vec<PreparedMetadataObject>,
) -> Result<Vec<PreparedMetadataObject>> {
    let mut seen = BTreeMap::<(_, _), usize>::new();
    let mut unique: Vec<PreparedMetadataObject> = Vec::new();
    for object in objects {
        let address = (object.kind(), object.hash.clone());
        if let Some(&index) = seen.get(&address) {
            if unique[index].bytes() != object.bytes() {
                return Err(Error::new(
                    storage_contract::ErrorCode::ObjectCollision,
                    "prepared immutable objects have different bytes at the same address",
                ));
            }
        } else {
            seen.insert(address, unique.len());
            unique.push(object);
        }
    }
    Ok(unique)
}

type HistoryGraphObjects = Vec<AuthenticatedMetadataObject>;

/// Select every dependency independently from trusted replicas, without repair.
fn bound_history_graph(
    members: &[DiskConfig],
    hash: &str,
) -> Result<(HistoryManifest, HistoryGraphObjects)> {
    let (manifest, objects) = read_history_graph(members, hash)?;
    manifest.validate()?;
    Ok((manifest, objects))
}

fn read_history_graph(
    members: &[DiskConfig],
    hash: &str,
) -> Result<(HistoryManifest, HistoryGraphObjects)> {
    let root =
        crate::publication_evidence::verified_graph_object(members, "history-manifest", hash)?;
    graph::resolve_with_objects(root, |kind, hash| {
        crate::publication_evidence::verified_graph_object(members, kind, hash)
    })
}

/// Bind the exact authenticated graph to an admitted snapshot projection, or
/// independently admit its semantics when replay has no snapshot projection.
pub(crate) fn bound_history_objects(
    members: &[DiskConfig],
    candidate: &Candidate,
    admitted: Option<&AdmittedState>,
) -> Result<HistoryGraphObjects> {
    let mut objects = Vec::new();
    let mut seen = BTreeSet::new();
    let state = admitted.map(AdmittedState::state);
    if let Some(state) = state
        && candidate
            .history_manifest_hashes
            .keys()
            .ne(state.histories.keys())
    {
        return Err(Error::new(
            storage_contract::ErrorCode::ObjectCorrupt,
            "candidate history set differs",
        ));
    }
    for (id, hash) in &candidate.history_manifest_hashes {
        let (manifest, graph) = read_history_graph(members, hash)?;
        if manifest.history.history_tree_id != *id {
            return Err(Error::new(
                storage_contract::ErrorCode::ObjectCorrupt,
                "bound history identity differs",
            ));
        }
        match state {
            Some(state) => {
                let expected = HistoryManifest::project_state(state, id)?;
                if canonical_json(&expected)? != canonical_json(&manifest)? {
                    return Err(Error::new(
                        storage_contract::ErrorCode::ObjectCorrupt,
                        "bound history projection differs",
                    ));
                }
            }
            None => manifest.validate()?,
        }
        for object in graph {
            if seen.insert((object.kind(), object.hash().to_owned())) {
                objects.push(object);
            }
        }
    }
    Ok(objects)
}

pub(crate) fn repair_history_objects(
    members: &[DiskConfig],
    objects: &HistoryGraphObjects,
    repaired: &mut BTreeSet<(String, String)>,
) -> Result<()> {
    // Resolver output is dependency-first, including child indexes before parents.
    for object in objects {
        let address = (object.kind().to_owned(), object.hash().to_owned());
        if !repaired.contains(&address) {
            crate::publication_evidence::repair_authenticated_object(members, object)?;
            if repaired.len() < 16_384 {
                repaired.insert(address);
            }
        }
    }
    Ok(())
}

fn package_version(package: &Path, version: &str) -> PathBuf {
    package.join("versions").join(version)
}
fn package_object(package: &Path, kind: &str, hash: &str) -> Result<PathBuf> {
    if !matches!(kind, "state-page" | "history-index")
        || hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::new(
            storage_contract::ErrorCode::HistoryPackageInvalid,
            "invalid graph object address",
        ));
    }
    Ok(package
        .join("objects")
        .join(kind)
        .join(&hash[..2])
        .join(format!("{hash}.json")))
}

fn load_package(package: &Path) -> Result<(HistoryManifest, PackageProof, HistoryPackageReport)> {
    let proof: PackageProof = read_json(&package.join("publication.json"))?;
    let root = AuthenticatedMetadataObject::authenticate(
        "history-manifest",
        &proof.manifest_hash,
        read_json(&package.join("manifest.json"))?,
    )
    .map_err(|error| {
        error.context(
            storage_contract::ErrorCode::HistoryPackageInvalid,
            "manifest is not bound by the published candidate",
        )
    })?;
    let hash = root.hash().to_owned();
    let (manifest, _) = graph::resolve_with_objects(root, |kind, hash| {
        AuthenticatedMetadataObject::authenticate(
            kind,
            hash,
            read_json(&package_object(package, kind, hash)?)?,
        )
    })?;
    manifest.validate()?;
    verify_publication_seal(
        &proof.candidate,
        &proof.publication,
        &proof.publication_seal,
    )?;
    if proof
        .candidate
        .history_manifest_hashes
        .get(&manifest.history.history_tree_id)
        != Some(&hash)
    {
        return Err(Error::new(
            storage_contract::ErrorCode::HistoryPackageInvalid,
            "manifest is not bound by the published candidate",
        ));
    }
    let mut full_files = 0;
    let mut tombstones = 0;
    let mut full_bytes = 0u64;
    for (id, version) in &manifest.versions {
        if manifest.content_states[id] == ContentState::Available {
            let directory = package_version(package, id);
            let metadata: FileVersion = read_json(&directory.join("version.json"))?;
            if object_hash("version", &metadata)? != object_hash("version", version)? {
                return Err(Error::new(
                    storage_contract::ErrorCode::HistoryPackageInvalid,
                    "packaged version metadata differs",
                ));
            }
            verify_payload(&directory, version)?;
            full_files += 1;
            full_bytes = full_bytes
                .checked_add(version.logical_size)
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::ResourceLimit,
                        "package byte count overflow",
                    )
                })?;
        } else {
            tombstones += 1;
        }
    }
    let report = HistoryPackageReport {
        history_tree_id: manifest.history.history_tree_id.clone(),
        repository_id: manifest.repository.repository_id.clone(),
        manifest_hash: hash,
        publication_sequence: proof.candidate.sequence,
        commits: manifest.commits.len(),
        versions: manifest.versions.len(),
        full_files,
        tombstones,
        full_bytes,
    };
    Ok((manifest, proof, report))
}
/// Validate one history without a system index, source pool, network or account store.
pub fn offline_check_history_package(package: &Path) -> Result<HistoryPackageReport> {
    load_package(package).map(|(_, _, report)| report)
}
impl Engine {
    pub fn export_history_package(
        &self,
        history: &str,
        destination: &Path,
    ) -> Result<HistoryPackageReport> {
        self.repository_for_history(history)?;
        let manifest = HistoryManifest::project_state(&self.state, history)?;
        let mut proof = None;
        let mut graph_objects = Vec::new();
        let members = self.trusted_evidence_members();
        for disk in &members {
            for (record, _) in scan(disk)?
                .records
                .into_iter()
                .rev()
                .filter(|(r, _)| r.record_type == RecordType::Publication)
            {
                let candidate: Candidate = crate::publication_evidence::verified_object(
                    &members,
                    "candidate",
                    &record.candidate_hash,
                )?;
                self.verify_candidate_membership(&candidate)?;
                if candidate.pool_id != self.config.pool_id || candidate.sequence != record.sequence
                {
                    return Err(Error::new(
                        storage_contract::ErrorCode::PublicationInvalid,
                        "candidate differs from journal or pool",
                    ));
                }
                let Some(hash) = candidate.history_manifest_hashes.get(history).cloned() else {
                    continue;
                };
                let (bound, objects) = bound_history_graph(&members, &hash)?;
                if canonical_json(&bound)? != canonical_json(&manifest)? {
                    continue;
                }
                graph_objects = objects;
                let publication: Publication = crate::publication_evidence::verified_object(
                    &members,
                    "publication",
                    &record.object_hash,
                )?;
                let publication_seal: PublicationSeal =
                    crate::publication_evidence::verified_publication_seal(
                        &members,
                        &candidate,
                        &publication,
                    )?;
                proof = Some(PackageProof {
                    manifest_hash: hash.clone(),
                    candidate,
                    publication,
                    publication_seal,
                });
                break;
            }
            if proof.is_some() {
                break;
            }
        }
        let proof = proof.ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::RecoveryRequired,
                "no published manifest matches the current history",
            )
        })?;
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        private_dir(parent)?;
        let staging = parent.join(format!(".zxs-history-{}", new_id("req")));
        private_dir(&staging)?;
        let result = (|| {
            for (id, version) in &manifest.versions {
                if manifest.content_states[id] != ContentState::Available {
                    continue;
                }
                let (source, _) = self.valid_replica(id)?;
                let dir = package_version(&staging, id);
                private_dir(&dir.join("content"))?;
                let (size, hash, checksums) = full_copy(
                    &source,
                    &dir.join("content").join(&version.creation_name),
                    version.checksum_chunk_bytes,
                    self.config.max_file_bytes,
                )?;
                if size != version.logical_size
                    || hash != version.content_hash
                    || blake3::hash(&checksums.encode()?).to_hex().as_str()
                        != version.checksum_manifest_hash
                {
                    return Err(Error::new(
                        storage_contract::ErrorCode::SourceChanged,
                        "exported content differs from the published manifest",
                    ));
                }
                write_json(&dir.join("version.json"), version, false)?;
                write_atomic(&dir.join("checksums.bin"), &checksums.encode()?, false)?;
                sync_dir(&dir)?;
            }
            for object in &graph_objects {
                let path = if object.kind() == "history-manifest" {
                    staging.join("manifest.json")
                } else {
                    package_object(&staging, object.kind(), object.hash())?
                };
                write_atomic(&path, object.bytes(), false)?;
            }
            write_json(&staging.join("publication.json"), &proof, false)?;
            sync_dir(&staging)?;
            let report = offline_check_history_package(&staging)?;
            rustix::fs::renameat_with(
                rustix::fs::CWD,
                &staging,
                rustix::fs::CWD,
                destination,
                rustix::fs::RenameFlags::NOREPLACE,
            )
            .map_err(std::io::Error::from)?;
            sync_dir(parent)?;
            Ok(report)
        })();
        match result {
            Ok(report) => Ok(report),
            Err(operation) => match fs::remove_dir_all(&staging) {
                Ok(()) => Err(operation),
                Err(cleanup) if cleanup.kind() == std::io::ErrorKind::NotFound => Err(operation),
                Err(cleanup) => Err(Error::Cleanup {
                    operation: Box::new(operation),
                    cleanup,
                }),
            },
        }
    }
    /// Restore immutable logical identities into a new pool. Routes are explicitly
    /// rebound, every available file is fully copied, and the result is republished.
    pub fn restore_history_package(
        &mut self,
        package: &Path,
        primary: &str,
        copies: u32,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.require_write()?;
        let (manifest, proof, report) = load_package(package)?;
        let route = self.route(primary, copies)?;
        for version in manifest.versions.values() {
            if version.checksum_chunk_bytes != self.config.checksum_chunk_bytes
                || version.logical_size > self.config.max_file_bytes
            {
                return Err(Error::new(
                    storage_contract::ErrorCode::FormatUnsupported,
                    "restore requires compatible checksum chunks and file size limits",
                ));
            }
        }
        // Recovery explicitly rebinds routes. Keep the user's logical prefixes
        // by route position, never derive them from a primary physical path.
        let mut backup_slots = BTreeMap::<String, String>::new();
        let source_backup_slots = manifest
            .placements
            .values()
            .flat_map(|placement| placement.route_slot_ids.iter().skip(1))
            .collect::<BTreeSet<_>>();
        for placement in manifest.placements.values() {
            for (old, new) in placement
                .route_slot_ids
                .iter()
                .skip(1)
                .zip(route.iter().skip(1))
            {
                if backup_slots
                    .insert(old.clone(), new.clone())
                    .is_some_and(|prior| prior != *new)
                {
                    return Err(Error::new(
                        storage_contract::ErrorCode::RestoreBackupConflict,
                        "source backup slots cannot be rebound unambiguously",
                    ));
                }
            }
        }
        let remap_paths = |paths: &BTreeMap<String, String>| -> Result<BTreeMap<String, String>> {
            let mut rebound = BTreeMap::new();
            for (slot, path) in paths {
                // Empty configured scopes have no version placement to map.
                // Their explicit destination slot identity still carries the
                // chosen prefix. Never replace a known positional mapping or
                // revive a source tier excluded by the requested copy count.
                let target = backup_slots.get(slot).or_else(|| {
                    (!source_backup_slots.contains(slot))
                        .then(|| route.iter().skip(1).find(|target| *target == slot))
                        .flatten()
                });
                if let Some(target) = target
                    && rebound
                        .insert(target.clone(), path.clone())
                        .is_some_and(|prior| prior != *path)
                {
                    return Err(Error::new(
                        storage_contract::ErrorCode::RestoreBackupConflict,
                        "backup prefixes collide after route rebinding",
                    ));
                }
            }
            if !paths.is_empty() && route.iter().skip(1).any(|slot| !rebound.contains_key(slot)) {
                return Err(Error::new(
                    storage_contract::ErrorCode::BackupPrefixRequired,
                    "the restored route has a backup slot without a user prefix",
                ));
            }
            Ok(rebound)
        };
        let backup_paths = manifest
            .file_backup_paths
            .iter()
            .map(|(file, paths)| Ok((file.clone(), remap_paths(paths)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let backup_prefixes = manifest
            .protection_policies
            .iter()
            .map(|(id, policy)| Ok((id.clone(), remap_paths(&policy.backup_prefixes)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let after_source = proof.candidate.sequence.checked_add(1).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "source publication sequence exhausted",
            )
        })?;
        self.mutate("history.restore",principal,operation,json!({"manifest_hash":report.manifest_hash,"primary_slot_id":primary,"target_copies":copies}),|engine,state,pending| {
            if state.histories.contains_key(&manifest.history.history_tree_id) || state.repositories.contains_key(&manifest.repository.repository_id)
                || manifest.commits.keys().any(|id| state.commits.contains_key(id))
                || manifest.branches.keys().any(|id| state.branches.contains_key(id))
                || manifest.directory_branches.keys().any(|id| state.directory_branches.contains_key(id))
                || manifest.directory_trees.keys().any(|id| state.directory_trees.contains_key(id))
                || manifest.directory_selections.keys().any(|id| state.directory_selections.contains_key(id))
                || manifest.directory_selection_generations.keys().any(|id| state.directory_selection_generations.contains_key(id))
                || manifest.file_write_origins.keys().any(|id| state.file_write_origins.contains_key(id))
                || manifest.file_contexts.keys().any(|id| state.file_contexts.contains_key(id))
                || manifest.file_path_nodes.keys().any(|id| state.file_path_nodes.contains_key(id))
                || manifest.file_metadata_nodes.keys().any(|id| state.file_metadata_nodes.contains_key(id))
                || manifest.file_branch_merges.keys().any(|id| state.file_branch_merges.contains_key(id))
                || manifest.file_branch_aliases.keys().any(|id| state.file_branch_aliases.contains_key(id))
                || manifest.file_backup_paths.keys().any(|id| state.file_backup_paths.contains_key(id))
                || manifest.file_backup_policies.keys().any(|id| state.file_backup_policies.contains_key(id))
                || manifest.file_node_parents.keys().any(|id| state.file_node_parents.contains_key(id))
                || manifest.file_path_notes.keys().any(|id| state.file_path_notes.contains_key(id))
                || manifest.directory_tree_selections.keys().any(|id| state.directory_tree_selections.contains_key(id))
                || manifest.file_locations.keys().any(|id| state.file_locations.contains_key(id))
                || manifest.file_selection_generations.keys().any(|id| state.file_selection_generations.contains_key(id))
                || manifest.versions.keys().any(|id| state.versions.contains_key(id))
                || manifest.versions.values().any(|incoming|state.versions.values().any(|existing|existing.file_id==incoming.file_id&&existing.history_tree_id!=incoming.history_tree_id))
                || manifest.merge_links.keys().any(|id| state.merge_links.contains_key(id))
                || manifest.boundary_marks.keys().any(|id| state.history_boundaries.contains_key(id))
                || manifest.placement_rules.keys().any(|id| state.placement_rules.contains_key(id))
                || manifest.protection_policies.keys().any(|id| state.protection_policies.contains_key(id)) {
                return Err(Error::new(storage_contract::ErrorCode::ObjectCollision,"package identities already exist in the destination pool"));
            }
            if state.repositories.values().any(|r| name_policy::lookup_key(&r.name) == name_policy::lookup_key(&manifest.repository.name)) {
                return Err(Error::new(storage_contract::ErrorCode::NameCollision,"restored repository name already exists"));
            }
            state.sequence = state.sequence.max(after_source);
            let mut repository = manifest.repository.clone();
            repository.primary_slot_id = primary.into(); repository.target_copies = copies;
            repository.smb_enabled = false;
            // This newly admitted repository has no existing destination
            // binding; imported source revisions do not authorize SMB sessions.
            repository.smb_binding_generation = 1;
            repository.file_manager_root=false;
            state.repositories.insert(repository.repository_id.clone(),repository.clone());
            state.histories.insert(manifest.history.history_tree_id.clone(),manifest.history.clone());
            state.commits.extend_shared(&manifest.commits); state.branches.extend(manifest.branches.clone());
            state.file_selections.extend(manifest.file_selections.clone());
            state.directory_branches.extend(manifest.directory_branches.clone());
            state.directory_trees.extend_shared(&manifest.directory_trees);
            state.directory_selections.extend(manifest.directory_selections.clone());
            state.directory_selection_generations.extend(manifest.directory_selection_generations.clone());
            state.file_write_origins.extend(manifest.file_write_origins.clone());
            state.file_contexts.extend(manifest.file_contexts.clone());
            state.file_path_nodes.extend(manifest.file_path_nodes.clone());
            state.file_metadata_nodes.extend(manifest.file_metadata_nodes.clone());
            state.file_branch_merges.extend(manifest.file_branch_merges.clone());
            state.file_branch_aliases.extend(manifest.file_branch_aliases.clone());
            state.file_node_parents.extend(manifest.file_node_parents.clone());
            state.file_path_notes.extend(manifest.file_path_notes.clone());
            state.directory_tree_selections.extend(manifest.directory_tree_selections.clone());
            state.file_locations.extend(manifest.file_locations.clone());
            state.file_backup_paths.extend(backup_paths.clone());
            state.file_backup_policies.extend(manifest.file_backup_policies.clone());
            state.file_selection_generations.extend(manifest.file_selection_generations.clone());
            state.file_notes.extend(manifest.file_notes.clone());state.version_notes.extend(manifest.version_notes.clone());
            state.file_names.extend(manifest.file_names.clone());
            state.merge_links.extend(manifest.merge_links.clone()); state.history_boundaries.extend(manifest.boundary_marks.clone());
            for rule in manifest.placement_rules.values() {
                let mut rule = rule.clone(); rule.primary_slot_id = primary.into(); rule.generation = 1;
                state.placement_rules.insert(rule.rule_id.clone(),rule);
            }
            for policy in manifest.protection_policies.values() {
                let mut policy = policy.clone(); policy.target_copies = copies; policy.generation = 1; policy.backup_prefixes=backup_prefixes[&policy.policy_id].clone();
                state.protection_policies.insert(policy.policy_id.clone(),policy);
            }
            for (id,version) in &manifest.versions {
                let placement = Placement {version_id:id.clone(),primary_slot_id:primary.into(),target_copies:copies,
                    route_slot_ids:route.clone(),topology_generation:state.topology_generation,
                    binding_generations:route.iter().map(|slot| Ok((slot.clone(),engine.disk(slot)?.binding_generation))).collect::<Result<_>>()?};
                state.versions.insert(id.clone(),version.clone()); state.placements.insert(id.clone(),placement.clone());
                state.content_states.insert(id.clone(),manifest.content_states[id]);
                if manifest.content_states[id] == ContentState::Available {
                    pending.push(crate::engine::Prepared {version:version.clone(),placement,
                        source:package_version(package,id).join("content").join(&version.creation_name)});
                }
            }
            Ok(json!({"history_tree_id":manifest.history.history_tree_id,"repository_id":repository.repository_id,
                "branch_id":manifest.history.main_branch_id,"manifest_hash":report.manifest_hash,"full_files":report.full_files,
                "tombstones":report.tombstones,"restoration_manager":principal,"smb_rebind_required":true}))
        })
    }
}

#[cfg(test)]
#[path = "../../../test/engine/unit/history_package/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/history_package/incremental_tests.rs"]
mod incremental_tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/history_package/history_changes_oracle_tests.rs"]
mod history_changes_oracle_tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/history_package/prepared_dedup_tests.rs"]
mod prepared_dedup_tests;
