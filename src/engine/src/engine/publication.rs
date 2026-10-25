//! Durable preparation, decision and publication of one frozen transaction.
use super::{Engine, Prepared};
use crate::{disk::*, journal::*, *};
use json_codec::json;
use std::collections::BTreeMap;
#[path = "verified_payloads.rs"]
mod verified_payloads;
use verified_payloads::{PayloadPreparation, VerifiedPayloadSet};

/// A publication receipt, not a hash cache. Only successful decide constructs
/// this token, and reuse requires the exact current State allocation and config.
#[derive(Clone)]
pub(super) struct PublishedStateBase {
    state: std::sync::Weak<State>,
    snapshot: String,
    sequence: u64,
    config_hash: String,
    roots: BTreeMap<String, String>,
    snapshot_basis: Option<SnapshotReuse>,
}
impl PublishedStateBase {
    fn matches(&self, engine: &Engine, config_hash: &str) -> bool {
        self.state.ptr_eq(&std::sync::Arc::downgrade(&engine.state))
            && engine.snapshot.as_deref() == Some(self.snapshot.as_str())
            && engine.state.sequence == self.sequence
            && self.config_hash == config_hash
    }
}

/// An owned preparation, never serializable or constructible by callers.
pub(crate) struct DetachedPublication {
    next: State,
    candidate: Candidate,
    result: OperationResult,
    verified: VerifiedPayloadSet,
    names: crate::container_storage::PreparedNames,
    base_root: std::path::PathBuf,
    base_snapshot: Option<String>,
    base_sequence: u64,
    base_next_slot: u64,
    config_hash: String,
    snapshot_basis: Option<SnapshotReuse>,
}

impl Engine {
    pub(crate) fn persist(
        &mut self,
        next: State,
        prepared: Vec<Prepared>,
        result: OperationResult,
    ) -> Result<()> {
        self.persist_with_metadata_limit(next, prepared, result, MAX_METADATA)
    }
    pub(crate) fn persist_with_metadata_limit(
        &mut self,
        next: State,
        prepared: Vec<Prepared>,
        result: OperationResult,
        maximum: usize,
    ) -> Result<()> {
        let detached = self.prepare_publication(next, prepared, result, maximum)?;
        self.decide_publication(detached).map(|_| ())
    }
    pub(crate) fn prepare_publication(
        &self,
        next: State,
        prepared: Vec<Prepared>,
        result: OperationResult,
        maximum: usize,
    ) -> Result<DetachedPublication> {
        self.prepare_publication_bounded(next, prepared, result, maximum, maximum)
    }
    fn prepare_publication_bounded(
        &self,
        next: State,
        prepared: Vec<Prepared>,
        result: OperationResult,
        maximum: usize,
        cache_maximum: usize,
    ) -> Result<DetachedPublication> {
        let members = self.members();
        let mut required = BTreeMap::<String, u64>::new();
        for p in &prepared {
            for slot in &p.placement.route_slot_ids {
                let disk = self.disk(slot)?;
                if verify_payload(
                    &version_dir(disk, &p.version.history_tree_id, &p.version.version_id),
                    &p.version,
                )
                .is_ok()
                {
                    continue;
                }
                *required.entry(slot.clone()).or_default() = required
                    .get(slot)
                    .copied()
                    .unwrap_or(0)
                    .checked_add(p.version.logical_size)
                    .ok_or_else(|| {
                        Error::new(
                            storage_contract::ErrorCode::ResourceLimit,
                            "admission bytes overflow",
                        )
                    })?;
            }
        }
        for (slot, bytes) in &required {
            let d = self.disk(slot)?;
            validate_seal(&self.config, d)?;
            let free = available(&d.root)?;
            let budget = d
                .capacity_bytes
                .map(|limit| limit.saturating_sub(self.used_bytes(slot)))
                .unwrap_or(u64::MAX);
            if bytes.saturating_add(self.config.metadata_reserve_bytes) > free || *bytes > budget {
                return Err(Error::new(
                    storage_contract::ErrorCode::NoSpace,
                    format!("fixed route slot {slot} cannot reserve the complete payload"),
                ));
            }
        }
        // Resolve real page hashes and enforce every metadata bound without
        // publishing files or journal records. No estimate enters the decision.
        let config_hash = object_hash("publication-config", &self.config)?;
        let baseline = self
            .published_state_base
            .as_ref()
            .filter(|base| base.matches(self, &config_hash));
        let snapshot = match baseline.and_then(|base| {
            base.snapshot_basis
                .as_ref()
                .filter(|basis| basis.matches_snapshot(&base.snapshot))
        }) {
            Some(basis) => prepare_snapshot_incremental(&self.state, &next, basis, maximum)?,
            None => prepare_snapshot(&next, maximum)?,
        };
        let snapshot_hash = snapshot.hash.clone();
        #[cfg(test)]
        assert_eq!(
            snapshot_hash,
            prepare_snapshot(&next, maximum)?.hash,
            "incremental snapshot differs from full encoding"
        );
        let (history_manifest_hashes, manifests) = match self
            .published_state_base
            .as_ref()
            .filter(|base| base.matches(self, &config_hash))
        {
            Some(base) => crate::history_package::prepare_history_manifests_incremental(
                &self.state,
                &next,
                &base.roots,
                &members,
                maximum,
            )?,
            None => crate::history_package::prepare_history_manifests(&next, maximum)?,
        };
        // The full implementation remains an independent oracle in every
        // engine publication test, including merge/split/restore workflows.
        #[cfg(test)]
        assert_eq!(
            history_manifest_hashes,
            crate::history_package::prepare_history_manifests(&next, maximum)?.0,
            "incremental history roots differ from complete projection",
        );
        let candidate = Candidate {
            pool_id: self.config.pool_id.clone(),
            sequence: next.sequence,
            transaction_id: new_id("tx"),
            operation_id: result.operation_id.clone(),
            request_digest: result.request_digest.clone(),
            previous_snapshot: self.snapshot.clone(),
            snapshot_hash,
            versions: prepared
                .iter()
                .map(|p| VersionEvidence {
                    version: p.version.clone(),
                    placement: p.placement.clone(),
                })
                .collect(),
            metadata_disk_ids: members.iter().map(|d| d.disk_id.clone()).collect(),
            history_manifest_hashes,
        };
        crate::metadata_budget::preflight_publication_prepared(
            self,
            &candidate,
            &result,
            &next,
            None,
            maximum,
            Some(&snapshot),
        )?;
        crate::container_storage::validate_new_projection_paths(&self.state, &next)?;
        let preparation = PayloadPreparation::new(&self.config, &candidate)?;
        self.prepare_payloads(&prepared)?;
        let verified = preparation.verify(&self.config, &candidate)?;
        // Payload installation establishes the physical container, but no decision
        // exists yet. Encode its complete projection before ACCEPT can be written.
        let names = crate::container_storage::prepare_names(
            &self.config,
            &self.state,
            &next,
            cache_maximum,
        )?;
        snapshot.store(&members, &next)?;
        let snapshot_basis = snapshot.reuse_basis();
        for manifest in manifests {
            manifest.store(&members)?;
        }
        Ok(DetachedPublication {
            next,
            candidate,
            result,
            verified,
            names,
            base_root: self.root.clone(),
            base_snapshot: self.snapshot.clone(),
            base_sequence: self.state.sequence,
            base_next_slot: self.next_slot,
            config_hash: object_hash("publication-config", &self.config)?,
            snapshot_basis,
        })
    }
    pub(crate) fn decide_publication(
        &mut self,
        detached: DetachedPublication,
    ) -> Result<OperationResult> {
        if self.blocked
            || self.root != detached.base_root
            || self.snapshot != detached.base_snapshot
            || self.state.sequence != detached.base_sequence
            || self.next_slot != detached.base_next_slot
            || object_hash("publication-config", &self.config)? != detached.config_hash
        {
            return Err(Error::new(
                storage_contract::ErrorCode::PreparationStale,
                "engine changed during detached publication preparation",
            )
            .retryable());
        }
        let DetachedPublication {
            next,
            candidate,
            result,
            verified,
            names,
            snapshot_basis,
            ..
        } = detached;
        verified
            .validate(&self.config, &candidate)
            .map_err(|error| {
                let message = format!("prepared payload identity changed: {error}");
                error
                    .context(ErrorCode::PreparationStale, message)
                    .retryable()
            })?;
        // Preparation consumed payload capacity, but concurrent staged uploads
        // may have consumed the metadata reserve without changing State.
        for disk in &self.config.disks {
            if (disk.metadata_member
                || candidate
                    .versions
                    .iter()
                    .any(|evidence| evidence.placement.route_slot_ids.contains(&disk.slot_id)))
                && available(&disk.root)? < self.config.metadata_reserve_bytes
            {
                return Err(Error::new(
                    storage_contract::ErrorCode::NoSpace,
                    "metadata reserve was consumed during payload preparation",
                ));
            }
        }
        let members = self.members();
        for disk in &members {
            validate_seal(&self.config, disk)?;
        }
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_payloads")?;
        let hash = object_hash("candidate", &candidate)?;
        self.next_slot = candidate.sequence.checked_add(1).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "decision sequence exhausted",
            )
        })?;
        #[cfg(feature = "fault-injection")]
        let mut prepared = 0;
        for d in &members {
            store_object(d, "candidate", &candidate)?;
            append(
                d,
                "PREPARE",
                candidate.sequence,
                &hash,
                &candidate.snapshot_hash,
            )?;
            #[cfg(feature = "fault-injection")]
            {
                prepared += 1;
                self.failpoint(&format!("after_prepare_{prepared}"))?;
            }
        }
        // Once the first ACCEPT may have reached disk, failure is indeterminate.
        self.blocked = true;
        let finish = self.complete_candidate_verified(
            &candidate,
            &hash,
            &next,
            None,
            Some(&verified),
            Some(names),
        );
        if let Err(e) = finish {
            return Err(Error::new(
                storage_contract::ErrorCode::InDoubt,
                format!("{}; query operation {} and recover", e, result.operation_id),
            )
            .retryable());
        }
        let previous = std::mem::replace(&mut self.state, std::sync::Arc::new(next));
        self.snapshot = Some(candidate.snapshot_hash.clone());
        self.update_index(&previous)?;
        // Do not install on partial publication or post-decision index failure.
        self.published_state_base = Some(std::sync::Arc::new(PublishedStateBase {
            state: std::sync::Arc::downgrade(&self.state),
            snapshot: candidate.snapshot_hash,
            sequence: self.state.sequence,
            config_hash: object_hash("publication-config", &self.config)?,
            roots: candidate.history_manifest_hashes,
            snapshot_basis,
        }));
        self.blocked = false;
        Ok(result)
    }
    fn prepare_payloads(&self, prepared: &[Prepared]) -> Result<()> {
        // Versions of the same file can share a named container. Preserve their
        // order, and each version's route chain, while independent files copy
        // with bounded parallel workers. No decision is written until all join.
        let mut by_file = BTreeMap::<&str, Vec<&Prepared>>::new();
        for payload in prepared {
            by_file
                .entry(&payload.version.file_id)
                .or_default()
                .push(payload);
        }
        let groups: Vec<_> = by_file.into_values().collect();
        run_payload_groups(&groups, |payload| self.prepare_payload(payload))
    }
    pub(crate) fn prepare_payload(&self, p: &Prepared) -> Result<()> {
        let mut source = p.source.clone();
        for slot in &p.placement.route_slot_ids {
            let disk = self.disk(slot)?;
            validate_seal(&self.config, disk)?;
            let directory = version_dir(disk, &p.version.history_tree_id, &p.version.version_id);
            if directory.join("version.json").exists() {
                let existing: FileVersion = read_json(&directory.join("version.json"))?;
                if object_hash("version", &existing)? != object_hash("version", &p.version)? {
                    return Err(Error::new(
                        storage_contract::ErrorCode::ObjectCollision,
                        "version identity already exists with different bytes",
                    ));
                }
                verify_payload(&directory, &p.version)?;
            } else {
                let staging = disk.root.join("staging").join(new_id("op"));
                private_dir(&staging.join("content"))?;
                let target = staging.join("content").join(&p.version.creation_name);
                let (size, hash, checks) = full_copy(
                    &source,
                    &target,
                    self.config.checksum_chunk_bytes,
                    self.config.max_file_bytes,
                )?;
                if size != p.version.logical_size
                    || hash != p.version.content_hash
                    || blake3::hash(&checks.encode()?).to_hex().as_str()
                        != p.version.checksum_manifest_hash
                {
                    return Err(Error::new(
                        storage_contract::ErrorCode::SourceChanged,
                        "prepared full file differs from the sealed input",
                    ));
                }
                write_json(&staging.join("version.json"), &p.version, false)?;
                write_atomic(&staging.join("checksums.bin"), &checks.encode()?, false)?;
                write_json(
                    &staging.join("payload.json"),
                    &json!({"payload_id":format!("payload_{}",&p.version.version_id[4..]),"version_id":p.version.version_id,"kind":"FULL","size":size.to_string(),"content_hash":hash}),
                    false,
                )?;
                sync_dir(&staging)?;
                private_dir(directory.parent().expect("version parent"))?;
                let staging_parent = platform_io::fs_safety::linux::OpenAt2Root::open(
                    staging.parent().expect("staging parent"),
                    platform_io::fs_safety::linux::MountPolicy::SameMount,
                )
                .map_err(|error| {
                    let message = error.to_string();
                    Error::from(error).context(storage_contract::ErrorCode::UnsafePath, message)
                })?;
                let version_parent = platform_io::fs_safety::linux::OpenAt2Root::open(
                    directory.parent().expect("version parent"),
                    platform_io::fs_safety::linux::MountPolicy::SameMount,
                )
                .map_err(|error| {
                    let message = error.to_string();
                    Error::from(error).context(storage_contract::ErrorCode::UnsafePath, message)
                })?;
                rustix::fs::renameat_with(
                    staging_parent.directory(),
                    staging.file_name().expect("staging name"),
                    version_parent.directory(),
                    directory.file_name().expect("version name"),
                    rustix::fs::RenameFlags::NOREPLACE,
                )
                .map_err(std::io::Error::from)?;
                sync_dir(directory.parent().expect("version parent"))?;
            }
            // Install a real version file in this file's named container before
            // any protection receipt is issued. The next level reads this copy.
            source = crate::container_storage::install_version_content(&directory, &p.version)?;
        }
        Ok(())
    }
    pub(crate) fn complete_candidate(
        &mut self,
        c: &Candidate,
        hash: &str,
        next: &State,
        existing: Option<Publication>,
    ) -> Result<()> {
        self.complete_candidate_verified(c, hash, next, existing, None, None)
    }
    pub(super) fn complete_published_candidate(
        &mut self,
        c: &Candidate,
        hash: &str,
        next: &State,
        publication: Publication,
        mut append_receipt: impl FnMut(&DiskConfig, &str) -> Result<Receipt>,
    ) -> Result<()> {
        self.verify_candidate_membership(c)?;
        if publication.candidate_hash != hash
            || publication.result.request_digest != c.request_digest
            || (publication.recovery.is_none()
                && publication.finalize.accepts.len() != c.metadata_disk_ids.len())
            || publication.finalize_receipts.len() != c.metadata_disk_ids.len()
        {
            return Err(Error::new(
                storage_contract::ErrorCode::PublicationInvalid,
                "stored publication does not bind this candidate",
            ));
        }
        crate::container_storage::repair_historical_names(&self.config, &self.state, next)?;
        let pub_hash = object_hash("publication", &publication)?;
        let mut publication_receipts = Vec::new();
        for d in self.members() {
            store_object(&d, "publication", &publication)?;
            publication_receipts.push(append_receipt(&d, &pub_hash)?);
            #[cfg(feature = "fault-injection")]
            if publication.recovery.is_some() {
                self.failpoint(&format!(
                    "after_recovery_publication_{}",
                    publication_receipts.len()
                ))?;
            }
        }
        let seal = PublicationSeal {
            candidate_hash: hash.into(),
            publication_hash: pub_hash,
            receipts: publication_receipts.clone(),
        };
        verify_publication_seal(c, &publication, &seal)?;
        for d in self.members() {
            write_json(&publication_seal_path(&d, hash), &seal, false)?;
        }
        // Historical success is preserved even if bytes were later purged or lost.
        // Reconstruct only its original proof, never claim a newly verified payload.
        for evidence in &c.versions {
            for slot in &evidence.placement.route_slot_ids {
                let d = self.disk(slot)?;
                if validate_seal(&self.config, d).is_err() {
                    continue;
                }
                let dir = version_dir(
                    d,
                    &evidence.version.history_tree_id,
                    &evidence.version.version_id,
                );
                if !dir.exists() {
                    continue;
                }
                let proof = CommitProof {
                    candidate_hash: hash.into(),
                    finalize: publication.finalize.clone(),
                    version_hash: object_hash("version", &evidence.version)?,
                    disk_id: d.disk_id.clone(),
                    slot_id: slot.clone(),
                    route_slot_ids: evidence.placement.route_slot_ids.clone(),
                };
                let expected = publication
                    .payload_receipts
                    .iter()
                    .find(|r| r.version_id == evidence.version.version_id && r.slot_id == *slot);
                let Some(expected) = expected else {
                    if publication.recovery.is_some() {
                        continue;
                    }
                    return Err(Error::new(
                        storage_contract::ErrorCode::PublicationInvalid,
                        "stored local receipt missing",
                    ));
                };
                if expected.proof_hash != object_hash("commit-proof", &proof)? {
                    return Err(Error::new(
                        storage_contract::ErrorCode::PublicationInvalid,
                        "stored local receipt differs",
                    ));
                }
                // The first published placement on this slot owns the root
                // proof. Derive that fact from the replayed prefix, not a
                // possibly corrupt local commit-proof candidate pointer.
                let first_on_slot = self
                    .state
                    .placements
                    .get(&evidence.version.version_id)
                    .is_none_or(|placement| !placement.route_slot_ids.contains(slot));
                let proof_dir = if first_on_slot {
                    dir
                } else {
                    local_proof_directory(&dir, hash)?
                };
                repair_local_proof(&proof_dir.join("commit-proof.json"), &proof)?;
                repair_local_proof(
                    &proof_dir.join("publication-proof.json"),
                    &LocalPublication {
                        candidate: c.clone(),
                        publication: publication.clone(),
                        commit_proof: proof,
                        publication_receipts: publication_receipts.clone(),
                    },
                )?;
            }
        }
        Ok(())
    }
    fn complete_candidate_verified(
        &mut self,
        c: &Candidate,
        hash: &str,
        next: &State,
        existing: Option<Publication>,
        verified: Option<&VerifiedPayloadSet>,
        names: Option<crate::container_storage::PreparedNames>,
    ) -> Result<()> {
        if let Some(publication) = existing {
            return self.complete_published_candidate(c, hash, next, publication, |d, pub_hash| {
                append(d, "PUBLICATION", c.sequence, hash, pub_hash)
            });
        }
        self.verify_candidate_membership(c)?;
        let names = match names {
            Some(names) => names,
            None => crate::container_storage::prepare_names(
                &self.config,
                &self.state,
                next,
                MAX_METADATA,
            )?,
        };
        let members = self.members();
        let mut accepts = Vec::new();
        for d in &members {
            validate_seal(&self.config, d)?;
            accepts.push(append(d, "ACCEPT", c.sequence, hash, hash)?);
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!("after_accept_{}", accepts.len()))?;
        }
        let finalize = Finalize {
            candidate_hash: hash.into(),
            accepts,
            recovery_id: None,
        };
        let final_hash = object_hash("finalize", &finalize)?;
        let mut final_receipts = Vec::new();
        for d in &members {
            store_object(d, "finalize", &finalize)?;
            final_receipts.push(append(d, "FINALIZE", c.sequence, hash, &final_hash)?);
        }
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_finalize")?;
        // Only a decided candidate may rename a published file container.
        // A crash leaves the operation indeterminate and recovery replays it.
        names.apply(&self.config)?;
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_container_names")?;
        let mut payload_receipts = Vec::new();
        for evidence in &c.versions {
            for slot in &evidence.placement.route_slot_ids {
                let d = self.disk(slot)?;
                validate_seal(&self.config, d)?;
                let directory = version_dir(
                    d,
                    &evidence.version.history_tree_id,
                    &evidence.version.version_id,
                );
                if let Some(verified) = verified {
                    verified.validate_payload(&self.config, &evidence.version, slot)?;
                } else {
                    // Recovery has no live held-inode proof and must re-read all bytes.
                    verify_payload(&directory, &evidence.version)?;
                }
                let proof = CommitProof {
                    candidate_hash: hash.into(),
                    finalize: finalize.clone(),
                    version_hash: object_hash("version", &evidence.version)?,
                    disk_id: d.disk_id.clone(),
                    slot_id: d.slot_id.clone(),
                    route_slot_ids: evidence.placement.route_slot_ids.clone(),
                };
                let proof_dir = local_proof_directory(&directory, hash)?;
                write_json(&proof_dir.join("commit-proof.json"), &proof, false)?;
                payload_receipts.push(PayloadReceipt {
                    version_id: evidence.version.version_id.clone(),
                    disk_id: d.disk_id.clone(),
                    slot_id: d.slot_id.clone(),
                    proof_hash: object_hash("commit-proof", &proof)?,
                });
            }
        }
        payload_receipts
            .sort_by(|a, b| (&a.version_id, &a.disk_id).cmp(&(&b.version_id, &b.disk_id)));
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_commit_proofs")?;
        let publication = Publication {
            recovery: None,
            candidate_hash: hash.into(),
            finalize,
            finalize_receipts: final_receipts,
            payload_receipts,
            result: next
                .operations
                .get(&c.operation_id)
                .cloned()
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::StateInvalid,
                        "candidate result is absent",
                    )
                })?,
        };
        let pub_hash = object_hash("publication", &publication)?;
        let mut publication_receipts = Vec::new();
        for d in &members {
            store_object(d, "publication", &publication)?;
            publication_receipts.push(append(d, "PUBLICATION", c.sequence, hash, &pub_hash)?);
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!("after_publication_{}", publication_receipts.len()))?;
        }
        let seal = PublicationSeal {
            candidate_hash: hash.into(),
            publication_hash: pub_hash,
            receipts: publication_receipts.clone(),
        };
        for d in &members {
            write_json(&publication_seal_path(d, hash), &seal, false)?;
        }
        for evidence in &c.versions {
            for slot in &evidence.placement.route_slot_ids {
                let d = self.disk(slot)?;
                let directory = version_dir(
                    d,
                    &evidence.version.history_tree_id,
                    &evidence.version.version_id,
                );
                let proof_dir = local_proof_directory(&directory, hash)?;
                let proof = read_json(&proof_dir.join("commit-proof.json"))?;
                write_json(
                    &proof_dir.join("publication-proof.json"),
                    &LocalPublication {
                        candidate: c.clone(),
                        publication: publication.clone(),
                        commit_proof: proof,
                        publication_receipts: publication_receipts.clone(),
                    },
                    false,
                )?;
            }
        }
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_local_publications")?;
        Ok(())
    }
}

/// Restore only a proof reconstructed from a verified published closure.
/// Conflicting local material is retained under the same version directory;
/// recovery never deletes it or treats it as authority for a different result.
fn repair_local_proof<T: data_serialization::Serialize>(
    path: &std::path::Path,
    value: &T,
) -> Result<()> {
    use platform_io::fs_safety::linux::{FileIdentity, MountPolicy, OpenAt2Root};
    let expected = encode_metadata(value, MAX_METADATA)?;
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return write_atomic(path, &expected, false);
        }
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    // Fail closed on links/special files and I/O errors. An oversized proof can
    // be retained by rename without allocating or interpreting its whole body.
    let original = open_regular(path)?;
    match read_bytes(path, MAX_METADATA) {
        Ok(bytes) if bytes == expected => return Ok(()),
        Ok(_) => {}
        Err(error) if error.code() == storage_contract::ErrorCode::ResourceLimit => {}
        Err(error) => return Err(error),
    }
    let parent = path.parent().expect("proof parent");
    let quarantine = parent.join("proof-quarantine");
    private_dir(&quarantine)?;
    let source = OpenAt2Root::open(parent, MountPolicy::SameMount).map_err(|error| {
        let message = error.to_string();
        Error::from(error).context(storage_contract::ErrorCode::UnsafePath, message)
    })?;
    let destination = OpenAt2Root::open(&quarantine, MountPolicy::SameMount).map_err(|error| {
        let message = error.to_string();
        Error::from(error).context(storage_contract::ErrorCode::UnsafePath, message)
    })?;
    if FileIdentity::from_fd(&original)? != FileIdentity::from_fd(open_regular(path)?)? {
        return Err(Error::new(
            storage_contract::ErrorCode::UnsafePath,
            "local proof identity changed before repair",
        ));
    }
    let name = format!(
        "{}-{}",
        path.file_name().expect("proof name").to_string_lossy(),
        new_id("req")
    );
    rustix::fs::renameat_with(
        source.directory(),
        path.file_name().expect("proof name"),
        destination.directory(),
        name.as_str(),
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(std::io::Error::from)?;
    destination.directory().sync_all()?;
    source.directory().sync_all()?;
    if FileIdentity::from_fd(&original)?
        != FileIdentity::from_fd(open_regular(&quarantine.join(name))?)?
    {
        return Err(Error::new(
            storage_contract::ErrorCode::UnsafePath,
            "local proof identity changed during repair",
        ));
    }
    write_atomic(path, &expected, false)
}

/// Bounded scoped workers are fully joined before returning, even on failure.
/// Detached preparation may run without the engine mutex; decision follows all joins.
fn run_payload_groups<T: Sync>(
    groups: &[Vec<&T>],
    work: impl Fn(&T) -> Result<()> + Sync,
) -> Result<()> {
    const WORKERS: usize = 4;
    struct Queue {
        next: usize,
        first_error: Option<(usize, Error)>,
    }
    let queue = std::sync::Mutex::new(Queue {
        next: 0,
        first_error: None,
    });
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..WORKERS.min(groups.len()))
            .map(|_| {
                scope.spawn(|| -> Result<()> {
                    loop {
                        let index = {
                            let mut queue = queue
                                .lock()
                                .expect("invariant: payload preparation queue poisoned");
                            if queue.first_error.is_some() || queue.next == groups.len() {
                                return Ok(());
                            }
                            let index = queue.next;
                            queue.next += 1;
                            index
                        };
                        // A claimed file keeps its version order even if another file fails.
                        // Catch its panic here, not at join, so no worker keeps claiming work
                        // after the failure is recorded. No payload I/O holds the queue lock.
                        let outcome =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                for payload in &groups[index] {
                                    work(payload)?;
                                }
                                Ok(())
                            }));
                        let (error, panic) = match outcome {
                            Ok(Ok(())) => continue,
                            Ok(Err(error)) => (error, None),
                            Err(panic) => (
                                Error::new(
                                    storage_contract::ErrorCode::InternalError,
                                    "payload preparation worker panicked",
                                ),
                                Some(panic),
                            ),
                        };
                        {
                            let mut queue = queue
                                .lock()
                                .expect("invariant: payload preparation queue poisoned");
                            // Claiming is ordered, completion is not. Earlier claimed files
                            // still finish, preserving index-priority error reporting.
                            if queue
                                .first_error
                                .as_ref()
                                .is_none_or(|(first, _)| index < *first)
                            {
                                queue.first_error = Some((index, error));
                            }
                        }
                        // A custom panic payload may run a destructor; close admission first
                        // and never drop that payload while holding the queue lock.
                        drop(panic);
                        return Ok(());
                    }
                })
            })
            .collect();
        let mut worker_error = None;
        for worker in workers {
            let result = worker.join().unwrap_or_else(|_| {
                Err(Error::new(
                    storage_contract::ErrorCode::InternalError,
                    "payload preparation scheduler panicked",
                ))
            });
            if worker_error.is_none() {
                worker_error = result.err();
            }
        }
        worker_error.map_or(Ok(()), Err)
    })?;
    queue
        .into_inner()
        .expect("invariant: payload preparation queue poisoned")
        .first_error
        .map_or(Ok(()), |(_, error)| Err(error))
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/publication/parallel_preparation_tests.rs"]
mod parallel_preparation_tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/publication/cache_budget_tests.rs"]
mod cache_budget_tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/publication/history_baseline_tests.rs"]
mod history_baseline_tests;
