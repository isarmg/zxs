//! Frozen complete State material, not a disk activation or restore authority.
//! Every target read is local and bound to the original published snapshot.
use super::Job;
use crate::disk::{DiskRoot, MAX_METADATA};
use crate::engine::membership_transition::pending::activation::PortableAuthority;
use crate::history_package::AdmittedState;
use crate::journal::{
    AuthenticatedMetadataObject, Candidate, Publication, PublicationSeal, RecordType, Snapshot,
    decode_proof, resolve_admitted_snapshot, verify_publication_seal,
};
use crate::publication_evidence::{
    verified_object_observed_at, verified_publication_seal_observed_at, visit_snapshot_objects_at,
};
use crate::{Config, Engine, Error, ErrorCode, Result, canonical_json, object_hash};
use data_serialization::{Deserialize, Serialize};
use std::path::Path;
use std::{collections::BTreeMap, path::PathBuf, sync::atomic::AtomicBool};

const FORMAT: &str = "zxs-backup-pool-snapshot";
const ROOT: &str = "pool-snapshot";
const MAX_OBJECTS: usize = 4096;
const MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Envelope {
    format: String,
    config: Config,
    generation: Option<PortableAuthority>,
    candidate: Candidate,
    publication: Publication,
    seal: PublicationSeal,
}

impl Envelope {
    fn verify(&self, job: &Job) -> Result<()> {
        if self.format != FORMAT
            || self.candidate.pool_id != job.pool_id
            || self.candidate.sequence != job.request.expected_sequence
            || self.candidate.snapshot_hash != job.snapshot
            || object_hash("backup-preparation-config", &self.config)? != job.configuration_hash
        {
            return Err(invalid("pool snapshot differs from the frozen task"));
        }
        let config = super::super::ValidatedConfig::try_from(self.config.clone())?;
        match (&self.generation, self.candidate.generation()) {
            (None, None) => config.verify_original_candidate_membership(&self.candidate)?,
            (Some(authority), Some(_)) => {
                let original = authority.verify_candidate(&self.candidate)?;
                if canonical_json(&original)? != canonical_json(&config)? {
                    return Err(invalid(
                        "snapshot authority and captured configuration differ",
                    ));
                }
            }
            _ => return Err(invalid("snapshot generation authority is incomplete")),
        }
        verify_publication_seal(&self.candidate, &self.publication, &self.seal)
    }
}

fn invalid(detail: &str) -> Error {
    Error::new(ErrorCode::HistoryPackageInvalid, detail)
}

#[derive(Default)]
struct Budget {
    objects: usize,
    bytes: usize,
}
impl Budget {
    fn account(&mut self, bytes: usize) -> Result<()> {
        // Charge every decode, including repeated references. Deduplicating the
        // stored bytes must not permit amplified offline allocation or work.
        self.objects += 1;
        self.bytes = self.bytes.checked_add(bytes).ok_or_else(|| {
            Error::new(
                ErrorCode::ResourceLimit,
                "snapshot preparation byte count overflow",
            )
        })?;
        if self.objects > MAX_OBJECTS || self.bytes > MAX_BYTES {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "complete snapshot preparation exceeds its object/byte budget; no subset is complete",
            ));
        }
        Ok(())
    }
}

fn object_path(root: &Path, kind: &str, hash: &str) -> Result<PathBuf> {
    if !matches!(
        kind,
        "snapshot" | "state-control-auth" | "state-page" | "metadata-index"
    ) || hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("invalid pool snapshot object address"));
    }
    Ok(root
        .join(ROOT)
        .join("objects")
        .join(kind)
        .join(format!("{hash}.json")))
}

pub(super) struct PreparedSnapshot {
    envelope: Vec<u8>,
    objects: BTreeMap<(&'static str, String), Vec<u8>>,
}
impl PreparedSnapshot {
    pub(super) fn freeze(engine: &Engine, job: &Job) -> Result<Self> {
        let crate::engine::EvidenceMembers {
            members,
            first_error,
        } = engine.trusted_evidence_members();
        let mut envelope = None;
        for disk in &members {
            let records = engine
                .metadata_integrity
                .observe(crate::journal::scan_configured_at(
                    &engine.disk_resources.get(disk)?.root,
                    disk,
                    &engine.config,
                ))?;
            if let Some((record, _)) = records.records.into_iter().rev().find(|(record, _)| {
                record.record_type == RecordType::Publication
                    && record.sequence == job.request.expected_sequence
            }) {
                let candidate: Candidate = verified_object_observed_at(
                    &engine.disk_resources,
                    &members,
                    "candidate",
                    &record.candidate_hash,
                    &mut |error| engine.metadata_integrity.fault(error),
                )?;
                engine.verify_candidate_membership(&candidate)?;
                let publication = verified_object_observed_at(
                    &engine.disk_resources,
                    &members,
                    "publication",
                    &record.object_hash,
                    &mut |error| engine.metadata_integrity.fault(error),
                )?;
                let seal = verified_publication_seal_observed_at(
                    &engine.disk_resources,
                    &members,
                    &candidate,
                    &publication,
                    &mut |error| engine.metadata_integrity.fault(error),
                )?;
                let value = Envelope {
                    format: FORMAT.into(),
                    config: engine.config().clone(),
                    generation: engine
                        .config
                        .generation()
                        .map(|generation| generation.portable.clone()),
                    candidate,
                    publication,
                    seal,
                };
                value.verify(job)?;
                envelope = Some(canonical_json(&value)?);
            }
            if envelope.is_some() {
                break;
            }
        }
        let envelope = envelope.ok_or_else(|| {
            first_error.unwrap_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "no published full snapshot matches the frozen backup task",
                )
            })
        })?;
        if envelope.len() > MAX_METADATA {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "snapshot publication proof exceeds reader budget",
            ));
        }
        let mut budget = Budget::default();
        budget.account(envelope.len())?;
        let mut objects = BTreeMap::new();
        visit_snapshot_objects_at(&engine.disk_resources, &members, &job.snapshot, |object| {
            budget.account(object.bytes().len())?;
            objects
                .entry((object.kind(), object.hash().to_owned()))
                .or_insert_with(|| object.bytes().to_vec());
            Ok(())
        })?;
        let prepared = Self { envelope, objects };
        let admitted = prepared.admit(&job.snapshot)?;
        if canonical_json(admitted.state())? != canonical_json(engine.state())? {
            return Err(invalid(
                "published snapshot differs from the complete current State",
            ));
        }
        Ok(prepared)
    }

    fn admit(&self, hash: &str) -> Result<AdmittedState> {
        let load = |kind, hash: &str| -> Result<AuthenticatedMetadataObject> {
            let bytes = self
                .objects
                .get(&(kind, hash.into()))
                .ok_or_else(|| invalid("captured snapshot object missing"))?;
            AuthenticatedMetadataObject::authenticate(
                kind,
                hash,
                storage_contract::strict_json(bytes)?,
            )
        };
        let root = load("snapshot", hash)?;
        let snapshot: Snapshot = json_codec::from_value(root.into_value())?;
        resolve_admitted_snapshot(snapshot, load)
    }

    pub(super) fn remaining_bytes(&self, root: &DiskRoot) -> Result<u64> {
        let mut total = 0u64;
        let mut account = |path: PathBuf, length: usize| -> Result<()> {
            if !crate::history_package::preparation_path_exists(root, &path)? {
                total = total.checked_add(length as u64).ok_or_else(|| {
                    Error::new(ErrorCode::ResourceLimit, "snapshot space overflow")
                })?;
            }
            Ok(())
        };
        account(
            root.path().join(ROOT).join("publication.json"),
            self.envelope.len(),
        )?;
        for ((kind, hash), bytes) in &self.objects {
            account(object_path(root.path(), kind, hash)?, bytes.len())?;
        }
        Ok(total)
    }

    pub(super) fn copy(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        for ((kind, hash), bytes) in &self.objects {
            crate::history_package::preparation_checkpoint(stop)?;
            crate::history_package::install_preparation_bytes(
                root,
                &object_path(root.path(), kind, hash)?,
                bytes,
            )?;
        }
        crate::history_package::preparation_checkpoint(stop)?;
        crate::history_package::install_preparation_bytes(
            root,
            &root.path().join(ROOT).join("publication.json"),
            &self.envelope,
        )
    }
}

pub(super) fn read(root: &DiskRoot, job: &Job, stop: &AtomicBool) -> Result<AdmittedState> {
    let envelope_bytes = root.read_bytes(
        &root.path().join(ROOT).join("publication.json"),
        MAX_METADATA,
    )?;
    let envelope: Envelope = decode_proof(&envelope_bytes)?;
    envelope.verify(job)?;
    let mut budget = Budget::default();
    budget.account(envelope_bytes.len())?;
    let mut load = |kind: &'static str, hash: &str| -> Result<AuthenticatedMetadataObject> {
        crate::history_package::preparation_checkpoint(stop)?;
        let bytes = root.read_bytes(&object_path(root.path(), kind, hash)?, MAX_METADATA)?;
        budget.account(bytes.len())?;
        AuthenticatedMetadataObject::authenticate(
            kind,
            hash,
            storage_contract::strict_json(&bytes)?,
        )
    };
    let snapshot: Snapshot = json_codec::from_value(load("snapshot", &job.snapshot)?.into_value())?;
    if snapshot.sequence != job.request.expected_sequence {
        return Err(invalid("snapshot sequence differs from its publication"));
    }
    resolve_admitted_snapshot(snapshot, load)
}
