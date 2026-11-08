//! The prescribed first Backup candidate, not a Publication or writer lease.
//! A single recipe produces its complete native State, exact payload obligations
//! and history catalog from the original selected decision. No online subset,
//! target completion file or caller-provided State can change that recipe.
use super::*;
use crate::history_package::preparation_checkpoint;
use std::{collections::BTreeMap, path::PathBuf, sync::atomic::AtomicBool};

const MAX_BYTES: usize = 128 * 1024 * 1024;
const MAX_OBJECTS: usize = 8192;

fn invalid(detail: &str) -> Error {
    Error::new(ErrorCode::PublicationInvalid, detail)
}
fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "first Backup candidate exceeds 8192 native objects or 128 MiB complete metadata",
    )
}

/// Actual native objects for a proposed first slot. In particular, neither the
/// prospective PUBLISHED result in its snapshot nor the presence of these files
/// authorizes returning business success. Only actual publication barriers can.
pub(in crate::engine) struct PreparedBackupFirstCandidate {
    candidate: Candidate,
    object: PreparedMetadataObject,
    files: BTreeMap<PathBuf, Vec<u8>>,
    bytes: usize,
    state_hash: String,
}

impl BackupCore {
    fn first_state(
        &self,
        authority_hash: &str,
        source: &State,
        stop: &AtomicBool,
    ) -> Result<State> {
        self.verify_source(source)?;
        let decision = AdmittedBackupDecision::from_certificate(self.decision.clone())?;
        let intent = decision.transition();
        if source.operations.contains_key(&intent.operation_id)
            || source
                .operation_principals
                .contains_key(&intent.operation_id)
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "Backup first publication would replace an original permanent operation",
            ));
        }
        let Origin::BackupCutover { evidence, .. } = &intent.origin else {
            unreachable!("invariant: native Backup core retains BackupCutover");
        };
        let mut next = decision
            .projection()
            .project_unpublished_routing(evidence, source, stop)?;
        if prepare_snapshot(&next, MAX_METADATA)?.hash != self.routing_snapshot_hash {
            return Err(invalid(
                "Backup first candidate differs from the originally selected routing snapshot",
            ));
        }
        // The common routing recipe already advances topology exactly once.
        // No event, HEAD, policy, body state or old receipt is rewritten here.
        let result = OperationResult {
            operation_id: intent.operation_id,
            kind: "backup-target-replacement".into(),
            commit_id: None,
            history_tree_id: None,
            branch_id: None,
            commit_state: storage_contract::CommitState::Published,
            protection_at_publish: storage_contract::ProtectionAtPublish::TargetMet,
            current_health: storage_contract::CurrentHealth::Healthy,
            request_digest: object_hash(INTENT_KIND, intent)?,
            sequence: decision.intended_first_sequence()?,
            result: json_codec::json!({
                "generation": authority_hash,
                "decision": decision.hash().as_str(),
                "metadata_members": intent.origin.config().metadata_members().len(),
                "topology_generation": next.topology_generation.to_string(),
                "targets": decision.projection().selected_nodes(),
                "source_data": "retained",
            }),
        };
        next.sequence = result.sequence;
        next.operations.insert(intent.operation_id, result);
        next.operation_principals
            .insert(intent.operation_id, intent.principal.clone());
        Ok(next)
    }

    fn first_candidate(
        &self,
        authority_hash: &str,
        source: &State,
        stop: &AtomicBool,
    ) -> Result<PreparedBackupFirstCandidate> {
        preparation_checkpoint(stop)?;
        let intent = self.intent()?;
        let next = self.first_state(authority_hash, source, stop)?;
        let result = next
            .operations
            .get(&intent.operation_id)
            .expect("invariant: first-state recipe inserted this operation");
        let snapshot = prepare_snapshot(&next, MAX_METADATA)?;
        let (history, objects) =
            crate::history_package::prepare_history_manifests(&next, MAX_METADATA)?;
        let mut versions = Vec::new();
        // These are obligations, not claims of successful copying. Every
        // AVAILABLE retained version must be protected on its complete explicit
        // route by a future publisher. Tombstones and retirement receipts stay
        // in the full snapshot; do not fabricate a body for unavailable content.
        for (id, version) in &next.versions {
            preparation_checkpoint(stop)?;
            match next.content_states.get(id) {
                None | Some(storage_contract::ContentState::Available) => {
                    versions.push(VersionEvidence {
                        version: version.clone(),
                        placement: next
                            .placements
                            .get(id)
                            .expect("invariant: whole-unit recipe covers every placement")
                            .clone(),
                    });
                }
                Some(storage_contract::ContentState::Purged)
                | Some(storage_contract::ContentState::Migrated)
                | Some(storage_contract::ContentState::Retired) => (),
            }
        }
        let candidate = Candidate::new(
            CandidateData {
                pool_id: intent.origin.config().pool_id.clone(),
                sequence: result.sequence,
                transaction_id: format!("tx_{}", &authority_hash[..32]),
                operation_id: intent.operation_id,
                request_digest: result.request_digest.clone(),
                previous_snapshot: Some(
                    intent
                        .boundary
                        .as_ref()
                        .expect("invariant: Backup origin has a published boundary")
                        .snapshot_hash
                        .clone(),
                ),
                snapshot_hash: snapshot.hash.clone(),
                versions,
                // Metadata membership is NOT inferred from new target count.
                // The selected Backup protocol retains all original voters.
                metadata_disk_ids: intent
                    .origin
                    .config()
                    .metadata_members()
                    .map(|disk| disk.disk_id.clone())
                    .collect(),
                history_roots: history.reference().clone(),
            },
            Some(authority_hash),
        );
        let object = PreparedMetadataObject::new("candidate", &candidate, MAX_METADATA)?;
        let mut prepared = PreparedBackupFirstCandidate {
            candidate,
            object,
            files: BTreeMap::new(),
            bytes: 0,
            state_hash: object_hash("backup-first-candidate-state", &next)?,
        };
        snapshot.visit_prepared_objects(&mut |kind, hash, bytes| {
            preparation_checkpoint(stop)?;
            prepared.add(kind, hash, bytes)
        })?;
        for object in objects {
            preparation_checkpoint(stop)?;
            prepared.add(object.kind(), &object.hash, object.bytes())?;
        }
        let candidate_bytes = prepared.object.bytes().to_vec();
        let hash = prepared.object.hash.clone();
        prepared.add("candidate", &hash, &candidate_bytes)?;
        Ok(prepared)
    }
}

impl PreparedBackupAuthority {
    pub(in crate::engine) fn first_candidate(
        &self,
        source: &State,
        stop: &AtomicBool,
    ) -> Result<PreparedBackupFirstCandidate> {
        self.core.first_candidate(self.hash(), source, stop)
    }
}

impl PreparedBackupFirstCandidate {
    fn add(&mut self, kind: &str, hash: &str, bytes: &[u8]) -> Result<()> {
        let path = object_path(kind, hash);
        if let Some(saved) = self.files.get(&path) {
            if saved != bytes {
                return Err(invalid("first candidate has colliding native object bytes"));
            }
            return Ok(());
        }
        self.bytes = self.bytes.checked_add(bytes.len()).ok_or_else(budget)?;
        if self.files.len() == MAX_OBJECTS || self.bytes > MAX_BYTES {
            return Err(budget());
        }
        self.files.insert(path, bytes.to_vec());
        Ok(())
    }
    pub(in crate::engine) fn hash(&self) -> &str {
        &self.object.hash
    }
    pub(in crate::engine) fn snapshot_hash(&self) -> &str {
        &self.candidate.snapshot_hash
    }
    pub(in crate::engine) fn sequence(&self) -> u64 {
        self.candidate.sequence
    }
    pub(in crate::engine) fn available_versions(&self) -> usize {
        self.candidate.versions.len()
    }
    pub(in crate::engine) fn members(&self) -> &[String] {
        &self.candidate.metadata_disk_ids
    }
    pub(in crate::engine) fn files(&self) -> impl Iterator<Item = (&PathBuf, &[u8])> {
        self.files
            .iter()
            .map(|(path, bytes)| (path, bytes.as_slice()))
    }
    pub(in crate::engine) fn bytes(&self) -> usize {
        self.bytes
    }
    pub(in crate::engine) fn objects(&self) -> usize {
        self.files.len()
    }

    /// Disk readback uses the native strict candidate decoder, authenticated
    /// snapshot resolver and complete State admission. Every decode is charged,
    /// including repeats, before returning a prepared-material observation.
    pub(in crate::engine) fn verify_at(&self, held: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        let mut budget = ReadBudget::default();
        let candidate = budget.read(held, "candidate", self.hash(), stop)?;
        let decoded: Candidate = crate::journal::decode_proof(candidate.bytes())?;
        if candidate.bytes() != self.object.bytes() {
            return Err(invalid(
                "saved first candidate changes generation, member set, state, history or payload obligations",
            ));
        }
        let snapshot = budget.read(held, "snapshot", &decoded.snapshot_hash, stop)?;
        let snapshot: crate::journal::Snapshot = json_codec::from_value(snapshot.into_value())?;
        let state = crate::journal::resolve_admitted_snapshot(snapshot, |kind, hash| {
            budget.read(held, kind, hash, stop)
        })?;
        if object_hash("backup-first-candidate-state", state.state())? != self.state_hash {
            return Err(invalid(
                "saved first candidate does not preserve the exact prescribed complete State",
            ));
        }
        // History graph bytes are also exact encoder outputs. Read each actual
        // native object under its hash, rather than trusting a manifest digest.
        for path in self.files.keys() {
            preparation_checkpoint(stop)?;
            let relative = held.path().join(path);
            let expected = &self.files[path];
            budget.charge(expected.len())?;
            if held.read_bytes(&relative, expected.len())? != *expected {
                return Err(invalid("first candidate native dependency differs"));
            }
        }
        Ok(())
    }
}

fn object_path(kind: &str, hash: &str) -> PathBuf {
    PathBuf::from("objects")
        .join(kind)
        .join(&hash[..2])
        .join(format!("{hash}.json"))
}
#[derive(Default)]
struct ReadBudget {
    objects: usize,
    bytes: usize,
}
impl ReadBudget {
    fn charge(&mut self, bytes: usize) -> Result<()> {
        self.objects += 1;
        self.bytes = self.bytes.checked_add(bytes).ok_or_else(budget)?;
        // Resolution plus exact dependency comparison visits each normal object
        // twice. This is a bounded read budget, not a deduplication exemption.
        if self.objects > MAX_OBJECTS * 3 || self.bytes > MAX_BYTES * 3 {
            return Err(budget());
        }
        Ok(())
    }
    fn read(
        &mut self,
        held: &DiskRoot,
        kind: &'static str,
        hash: &str,
        stop: &AtomicBool,
    ) -> Result<AuthenticatedMetadataObject> {
        preparation_checkpoint(stop)?;
        require_hash(hash)?;
        if !matches!(
            kind,
            "candidate" | "snapshot" | "state-control-auth" | "state-page" | "metadata-index"
        ) {
            return Err(invalid(
                "first candidate resolver requested an unknown native kind",
            ));
        }
        // Bound each read before decoding; account cumulative work afterwards.
        let bytes = held.read_bytes(&held.path().join(object_path(kind, hash)), MAX_METADATA)?;
        self.charge(bytes.len())?;
        AuthenticatedMetadataObject::authenticate(
            kind,
            hash,
            storage_contract::strict_json(&bytes)?,
        )
    }
}
