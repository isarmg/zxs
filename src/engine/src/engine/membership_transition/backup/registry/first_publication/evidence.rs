//! Original reserved Publication evidence. Parsing a digest does not establish
//! trust: detached callers retain it independently after the live barrier.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupFirstPublicationHash(String);
impl BackupFirstPublicationHash {
    /// Syntax only. Never discover this anchor from the target being admitted.
    pub fn parse(value: &str) -> Result<Self> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid(
                "Backup Publication digest must be 64 lowercase hexadecimal characters",
            ));
        }
        Ok(Self(value.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An exact observed or independently hash-authenticated native Publication.
/// This grants no active Config/registry, target lease or source fence release.
/// Target-local bytes must still be read through `verify_targets` before use.
pub(in crate::engine) struct PreparedBackupFirstPublication {
    publication: Publication,
    seal: PublicationSeal,
    object: PreparedMetadataObject,
    seal_bytes: Vec<u8>,
    report: BackupFirstPublicationReport,
}
impl PreparedBackupFirstPublication {
    pub(super) fn observed(
        prepared: &PreparedBackupRegistry,
        candidate: &PreparedBackupFirstCandidate,
        publication: Publication,
        seal: PublicationSeal,
    ) -> Result<Self> {
        verify_publication_seal(candidate.candidate(), &publication, &seal)?;
        if publication.recovery.is_some()
            || publication.finalize.recovery_id.is_some()
            || canonical_json(&publication.result)? != canonical_json(candidate.result())?
        {
            return Err(invalid(
                "Backup Publication differs from its original exact first result",
            ));
        }
        // The native seal checks the all-member frame chain. Bind its slot
        // labels to the actual archived Config as well; a target cannot rename
        // an original voter or make a later slot look like the first slot.
        for accept in &publication.finalize.accepts {
            let disk = candidate
                .original_config()
                .metadata_members()
                .find(|disk| disk.disk_id == accept.disk_id)
                .ok_or_else(|| invalid("Backup Publication names a non-original voter"))?;
            if accept.slot_id != disk.slot_id || accept.record.previous_frame_hash != "0".repeat(64)
            {
                return Err(invalid(
                    "Backup Publication changes an original voter or first-slot boundary",
                ));
            }
        }
        let retained = prepared.source_evidence().retained_payload_slots();
        let nodes = prepared.projection().selected_nodes();
        let mut bindings: BTreeMap<&str, (&str, u64)> = BTreeMap::new();
        for slot in &retained {
            let disk = candidate
                .original_config()
                .disks
                .iter()
                .find(|disk| disk.slot_id == *slot)
                .ok_or_else(|| invalid("Backup retained route is absent from original Config"))?;
            bindings.insert(
                slot.as_str(),
                (disk.disk_id.as_str(), disk.binding_generation),
            );
        }
        for node in &nodes {
            if bindings
                .insert(
                    &node.identity.slot_id,
                    (&node.identity.disk_id, node.identity.binding_generation),
                )
                .is_some()
            {
                return Err(invalid(
                    "Backup selected target overlaps its retained source route",
                ));
            }
        }
        let mut expected = Vec::new();
        for version in &candidate.candidate().versions {
            let route: std::collections::BTreeSet<_> = version
                .placement
                .route_slot_ids
                .iter()
                .map(String::as_str)
                .collect();
            if route != bindings.keys().copied().collect()
                || route.len() != version.placement.route_slot_ids.len()
                || version.placement.binding_generations.len() != bindings.len()
            {
                return Err(invalid(
                    "Backup Publication splits or reduces the original whole-unit fanout",
                ));
            }
            for (slot, (disk, generation)) in &bindings {
                if expected.len() == 65_536 {
                    return Err(Error::new(
                        ErrorCode::ResourceLimit,
                        "Backup Publication exceeds 65536 payload receipts",
                    ));
                }
                if version.placement.binding_generations.get(*slot) != Some(generation) {
                    return Err(invalid("Backup Publication substitutes a payload binding"));
                }
                let proof = CommitProof {
                    candidate_hash: candidate.hash().into(),
                    finalize: publication.finalize.clone(),
                    version_hash: object_hash("version", &version.version)?,
                    disk_id: (*disk).into(),
                    slot_id: (*slot).into(),
                    route_slot_ids: version.placement.route_slot_ids.clone(),
                };
                expected.push(PayloadReceipt {
                    version_id: version.version.version_id.clone(),
                    disk_id: (*disk).into(),
                    slot_id: (*slot).into(),
                    proof_hash: object_hash("commit-proof", &proof)?,
                });
            }
        }
        payloads::sort(&mut expected);
        if canonical_json(&expected)? != canonical_json(&publication.payload_receipts)? {
            return Err(invalid(
                "Backup Publication does not contain every exact routed payload receipt",
            ));
        }
        let object = PreparedMetadataObject::new("publication", &publication, MAX_METADATA)?;
        let seal_bytes = encode_metadata(&seal, MAX_METADATA)?;
        let report = BackupFirstPublicationReport {
            operation_id: candidate.result().operation_id,
            decision_hash: candidate.decision_hash().clone(),
            authority_hash: candidate.authority_hash().into(),
            candidate_hash: candidate.hash().into(),
            snapshot_hash: candidate.snapshot_hash().into(),
            publication_hash: object.hash.clone(),
            first_sequence: candidate.sequence(),
            original_metadata_members: candidate.members().to_vec(),
            payload_receipts: publication.payload_receipts.len(),
        };
        Ok(Self {
            publication,
            seal,
            object,
            seal_bytes,
            report,
        })
    }

    pub(in crate::engine) fn from_trusted_bytes(
        prepared: &PreparedBackupRegistry,
        candidate: &PreparedBackupFirstCandidate,
        expected: &BackupFirstPublicationHash,
        publication: &[u8],
        seal: &[u8],
    ) -> Result<Self> {
        if publication.len() > MAX_METADATA || seal.len() > MAX_METADATA {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "Backup Publication evidence exceeds its native bound",
            ));
        }
        let admitted = Self::observed(
            prepared,
            candidate,
            decode_proof(publication)?,
            decode_proof(seal)?,
        )?;
        if admitted.object.hash != expected.as_str()
            || admitted.object.bytes() != publication
            || admitted.seal_bytes != seal
        {
            return Err(invalid(
                "Backup Publication is not the independently retained exact original publication",
            ));
        }
        Ok(admitted)
    }

    /// Consume native Publication evidence, not a public diagnostic report or
    /// the prospective PUBLISHED receipt embedded in a prepared snapshot.
    pub(in crate::engine) fn require_namespace_candidate(
        &self,
        candidate: &PreparedBackupFirstCandidate,
    ) -> Result<()> {
        verify_publication_seal(candidate.candidate(), &self.publication, &self.seal)?;
        if self.report.candidate_hash != candidate.hash()
            || self.report.authority_hash != candidate.authority_hash()
            || self.report.decision_hash != *candidate.decision_hash()
            || self.report.snapshot_hash != candidate.snapshot_hash()
            || self.report.first_sequence != candidate.sequence()
            || self.publication.result != *candidate.result()
            || self.publication.recovery.is_some()
        {
            return Err(invalid(
                "Backup namespace admission does not consume its exact actual first Publication",
            ));
        }
        Ok(())
    }

    /// The journal layer receives only the already-authenticated native seal,
    /// not this engine-only publication capability or a caller-supplied digest.
    pub(in crate::engine) fn original_journal_binding(
        &self,
        root: &DiskRoot,
        disk: &DiskConfig,
        candidate: &PreparedBackupFirstCandidate,
        stop: &AtomicBool,
    ) -> Result<(platform_io::fs_safety::linux::FileIdentity, String)> {
        self.require_namespace_candidate(candidate)?;
        crate::journal::BackupFirstJournal::published_binding(
            root, disk, candidate, &self.seal, stop,
        )
    }

    pub(in crate::engine) fn report(&self) -> &BackupFirstPublicationReport {
        &self.report
    }
    pub(in crate::engine) fn publication_bytes(&self) -> &[u8] {
        self.object.bytes()
    }
    pub(in crate::engine) fn seal_bytes(&self) -> &[u8] {
        &self.seal_bytes
    }
    pub(in crate::engine) fn verify_targets(
        &self,
        prepared: &PreparedBackupRegistry,
        candidate: &PreparedBackupFirstCandidate,
        stop: &AtomicBool,
    ) -> Result<()> {
        // This intentionally never probes an original source path. It verifies
        // all selected target bytes/proofs without claiming current health of
        // the retained source copies or creating missing acknowledgements.
        if self.report.candidate_hash != candidate.hash()
            || self.report.decision_hash != *candidate.decision_hash()
        {
            return Err(invalid(
                "Backup Publication is not bound to this exact candidate",
            ));
        }
        payloads::observe_selected(prepared, candidate, &self.publication, &self.seal, stop)
    }
}

fn invalid(detail: &str) -> Error {
    Error::new(ErrorCode::PublicationInvalid, detail)
}
