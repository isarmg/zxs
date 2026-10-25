//! Explicit loss acknowledgement for an already decided, unpublished candidate.
//! This path never authorizes a new business write with fewer required copies.
use crate::journal::RecordType;
use crate::{disk::*, journal::*, *};
use data_serialization::{Deserialize, Serialize};
use json_codec::{Value, json};
use std::{collections::BTreeSet, fs};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct RecoveryPlan {
    pub recovery_id: String,
    pub digest: String,
    pub operation_id: String,
    pub candidate_hash: String,
    pub principal: String,
    pub reason: String,
    /// A unique fence persisted by every current member while all original OS locks are held.
    pub isolation_generation: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    pub version_protection: Vec<Value>,
    pub loss_details: Vec<Value>,
    members: Vec<MemberWitness>,
    original_accepts: Vec<Receipt>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct MemberWitness {
    disk_id: String,
    slot_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    binding_generation: u64,
    #[serialization(with = "storage_contract::u64_string")]
    log_bytes: u64,
    last_frame_hash: String,
    records: Vec<Receipt>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct RecoveryProof {
    pub plan: RecoveryPlan,
    pub reprotect: Vec<Receipt>,
}
fn required(message: impl Into<String>) -> Error {
    Error::new(storage_contract::ErrorCode::RecoveryRequired, message)
}
fn plan_digest(plan: &RecoveryPlan) -> Result<String> {
    let mut plan = plan.clone();
    plan.digest.clear();
    object_hash("recovery-plan-digest", &plan).map_err(Into::into)
}
fn receipts_ids(receipts: &[Receipt]) -> BTreeSet<String> {
    receipts.iter().map(|r| r.disk_id.clone()).collect()
}
fn plan_path(root: &std::path::Path, id: &str) -> std::path::PathBuf {
    root.join("recovery-plans").join(format!("{id}.json"))
}
fn member_plan_path(disk: &DiskConfig, hash: &str) -> std::path::PathBuf {
    disk.root
        .join("metadata/recovery-plans")
        .join(format!("{hash}.json"))
}
fn observe(engine: &Engine, candidate: &Candidate) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for evidence in &candidate.versions {
        let mut slots = Vec::new();
        for slot in &evidence.placement.route_slot_ids {
            let disk = engine.disk(slot)?;
            validate_seal(&engine.config, disk)?;
            if evidence.placement.binding_generations.get(slot) != Some(&disk.binding_generation) {
                return Err(required("candidate payload binding generation differs"));
            }
            let dir = version_dir(
                disk,
                &evidence.version.history_tree_id,
                &evidence.version.version_id,
            );
            let state = match verify_payload(&dir, &evidence.version) {
                Ok(()) => "COMPLETE_VERIFIED",
                Err(error) => {
                    // An inaccessible device or an arbitrary I/O failure is not permanent loss.
                    let content = crate::container_storage::content_path(&dir, &evidence.version)?;
                    match fs::symlink_metadata(&content) {
                        Err(io) if io.kind() == std::io::ErrorKind::NotFound => "MISSING",
                        Err(source) => {
                            return Err(Error::from(source).context(
                                ErrorCode::RecoveryRequired,
                                format!("cannot classify payload slot {slot}: {error}"),
                            ));
                        }
                        Ok(meta) if meta.is_file() => "CORRUPT",
                        Ok(_) => {
                            return Err(required(
                                "untrusted payload path requires manual quarantine",
                            ));
                        }
                    }
                }
            };
            slots.push(json!({"slot_id":slot,"copy_level":disk.level,"disk_id":disk.disk_id,"binding_generation":disk.binding_generation.to_string(),"state":state}));
        }
        out.push(json!({"version_id":evidence.version.version_id,"primary_slot_id":evidence.placement.primary_slot_id,"target_copies":evidence.placement.target_copies,"required_slots":evidence.placement.route_slot_ids,"topology_generation":evidence.placement.topology_generation.to_string(),"slots":slots}));
    }
    Ok(out)
}
fn freeze_observation(mut observed: Vec<Value>) -> (Vec<Value>, Vec<Value>) {
    let mut details = Vec::new();
    for version in &mut observed {
        let id = version["version_id"].clone();
        for slot in version["slots"].as_array_mut().expect("observed slots") {
            if slot["state"] != "COMPLETE_VERIFIED" {
                details
                    .push(json!({"version_id":id,"slot_id":slot["slot_id"],"cause":slot["state"]}));
                slot["state"] = json!("UNAVAILABLE");
            }
        }
    }
    (observed, details)
}
pub(crate) fn discover_publication(
    engine: &Engine,
    candidate: &Candidate,
    hash: &str,
) -> Result<Option<Publication>> {
    let mut found: Option<Publication> = None;
    for disk in engine.members() {
        for name in list_object_hashes(&disk, "publication")? {
            let publication: Publication =
                load_object(&disk, "publication", &name).map_err(|source| {
                    source.context(
                        ErrorCode::RecoveryRequired,
                        "an unreadable publication object prevents proving absence",
                    )
                })?;
            if publication.candidate_hash == hash {
                if found.as_ref().is_some_and(|old| {
                    object_hash("publication", old).ok()
                        != object_hash("publication", &publication).ok()
                }) {
                    return Err(required("conflicting publication results exist"));
                }
                found = Some(publication);
            }
        }
    }
    for evidence in &candidate.versions {
        for slot in &evidence.placement.route_slot_ids {
            let directory = version_dir(
                engine.disk(slot)?,
                &evidence.version.history_tree_id,
                &evidence.version.version_id,
            );
            let path = local_proof_directory(&directory, hash)?.join("publication-proof.json");
            if path.exists() {
                let local: LocalPublication = read_json(&path).map_err(|source| {
                    source.context(
                        ErrorCode::RecoveryRequired,
                        "unreadable local publication evidence",
                    )
                })?;
                if object_hash("candidate", &local.candidate)? != hash {
                    return Err(required("conflicting local publication evidence"));
                }
                if found.as_ref().is_some_and(|old| {
                    object_hash("publication", old).ok()
                        != object_hash("publication", &local.publication).ok()
                }) {
                    return Err(required("local and metadata publication results conflict"));
                }
                found = Some(local.publication);
            }
        }
    }
    Ok(found)
}
fn exclude_publication(engine: &Engine, candidate: &Candidate, hash: &str) -> Result<()> {
    if discover_publication(engine, candidate, hash)?.is_some() {
        return Err(required(
            "original publication evidence exists; replay that result",
        ));
    }
    Ok(())
}
fn require_permanent_loss(engine: &Engine, candidate: &Candidate, health: &[Value]) -> Result<()> {
    let mut unavailable = false;
    for version in &candidate.versions {
        let state = health
            .iter()
            .find(|v| v["version_id"] == version.version.version_id)
            .ok_or_else(|| required("health coverage missing"))?;
        let slots = state["slots"]
            .as_array()
            .ok_or_else(|| required("health slots missing"))?;
        if slots.iter().all(|s| s["state"] == "COMPLETE_VERIFIED") {
            continue;
        }
        // Use exactly the same source search as normal repair, including old
        // workspace generations and registered repair quarantine directories.
        if engine
            .pending_payload_source(&version.version, &version.placement)?
            .is_some()
        {
            return Err(required(
                "verified full bytes survive; repair the original protection instead of acknowledging permanent loss",
            ));
        }
        unavailable = true;
    }
    if !unavailable {
        return Err(required("no permanent content loss was established"));
    }
    Ok(())
}
impl RecoveryPlan {
    pub(crate) fn accepts(&self) -> &[Receipt] {
        &self.original_accepts
    }
}
impl RecoveryProof {
    pub(crate) fn verify(&self, c: &Candidate, p: &Publication) -> Result<()> {
        let bad = || {
            Error::new(
                storage_contract::ErrorCode::PublicationInvalid,
                "degraded publication recovery evidence is invalid",
            )
        };
        let plan = &self.plan;
        let candidate = object_hash("candidate", c)?;
        let ids = c.metadata_disk_ids.iter().cloned().collect::<BTreeSet<_>>();
        if plan.digest != plan_digest(plan)?
            || plan.candidate_hash != candidate
            || plan.sequence != c.sequence
            || plan.operation_id != c.operation_id
            || plan.reason.trim().is_empty()
            || plan.principal.is_empty()
            || ids.is_empty()
            || ids.len() != c.metadata_disk_ids.len()
            || plan.members.len() != ids.len()
            || plan.original_accepts.len() < ids.len() / 2 + 1
            || plan.original_accepts.len() > ids.len()
            || receipts_ids(&plan.original_accepts).len() != plan.original_accepts.len()
            || !receipts_ids(&plan.original_accepts).is_subset(&ids)
            || self.reprotect.len() != ids.len()
            || receipts_ids(&self.reprotect) != ids
            || p.finalize.recovery_id.as_deref() != Some(plan.recovery_id.as_str())
            || canonical_json(&p.finalize.accepts)? != canonical_json(&plan.original_accepts)?
            || p.result.commit_state != storage_contract::CommitState::PublishedDegraded
            || p.result.protection_at_publish != storage_contract::ProtectionAtPublish::BelowTarget
            || p.result.result["recovery_id"].as_str() != Some(plan.recovery_id.as_str())
            || p.result.result["recovery_plan_digest"].as_str() != Some(plan.digest.as_str())
            || canonical_json(&p.result.result["version_protection"])?
                != canonical_json(&plan.version_protection)?
        {
            return Err(bad());
        }
        validate_id(&plan.recovery_id, "recovery")?;
        validate_id(&plan.isolation_generation, "fence")?;
        let plan_hash = object_hash("recovery-plan", plan)?;
        let mut members = BTreeSet::new();
        for member in &plan.members {
            if !members.insert(member.disk_id.clone()) || member.records.is_empty() {
                return Err(bad());
            }
            let mut previous = member.records[0].record.previous_frame_hash.clone();
            for record in &member.records {
                if record.disk_id != member.disk_id
                    || record.slot_id != member.slot_id
                    || record.record.record_type == RecordType::Publication
                    || record.record.record_type == RecordType::Reprotect
                    || record.record.previous_frame_hash != previous
                    || !receipt_binds(
                        record,
                        record.record.record_type,
                        c.sequence,
                        &candidate,
                        &record.record.object_hash,
                    )?
                {
                    return Err(bad());
                }
                previous = record.frame_hash.clone();
            }
            if previous != member.last_frame_hash {
                return Err(bad());
            }
            let fence = self
                .reprotect
                .iter()
                .find(|r| r.disk_id == member.disk_id)
                .ok_or_else(bad)?;
            if fence.slot_id != member.slot_id
                || fence.record.previous_frame_hash != member.last_frame_hash
                || !receipt_binds(
                    fence,
                    RecordType::Reprotect,
                    c.sequence,
                    &candidate,
                    &plan_hash,
                )?
            {
                return Err(bad());
            }
        }
        if members != ids {
            return Err(bad());
        }
        for accept in &plan.original_accepts {
            if !receipt_binds(
                accept,
                RecordType::Accept,
                c.sequence,
                &candidate,
                &candidate,
            )? || !plan
                .members
                .iter()
                .any(|m| m.records.iter().any(|r| r.frame_hash == accept.frame_hash))
            {
                return Err(bad());
            }
        }
        if plan.version_protection.len() != c.versions.len() {
            return Err(bad());
        }
        let mut expected = BTreeSet::new();
        let mut below = false;
        for version in &c.versions {
            let observation = plan
                .version_protection
                .iter()
                .find(|v| v["version_id"].as_str() == Some(version.version.version_id.as_str()))
                .ok_or_else(bad)?;
            let slots = observation["slots"].as_array().ok_or_else(bad)?;
            if slots.len() != version.placement.route_slot_ids.len()
                || observation["required_slots"] != json!(version.placement.route_slot_ids)
                || observation["target_copies"] != json!(version.placement.target_copies)
                || observation["primary_slot_id"] != json!(version.placement.primary_slot_id)
            {
                return Err(bad());
            }
            let mut slot_set = BTreeSet::new();
            for slot in slots {
                let id = slot["slot_id"].as_str().ok_or_else(bad)?;
                if !slot_set.insert(id) || !version.placement.route_slot_ids.iter().any(|s| s == id)
                {
                    return Err(bad());
                }
                match slot["state"].as_str() {
                    Some("COMPLETE_VERIFIED") => {
                        expected.insert((version.version.version_id.clone(), id.to_string()));
                    }
                    Some("UNAVAILABLE") => below = true,
                    _ => return Err(bad()),
                }
            }
        }
        let actual = p
            .payload_receipts
            .iter()
            .map(|r| (r.version_id.clone(), r.slot_id.clone()))
            .collect::<BTreeSet<_>>();
        if !below || actual != expected || actual.len() != p.payload_receipts.len() {
            return Err(bad());
        }
        Ok(())
    }
}
impl Engine {
    fn pending_recovery_candidate(
        &self,
        operation: &str,
        resume: Option<&RecoveryPlan>,
    ) -> Result<(Candidate, State, Vec<MemberWitness>, Vec<Receipt>)> {
        self.require_recovery_locks()?;
        let mut candidate = None;
        let mut witnesses = Vec::new();
        let mut accepts = Vec::new();
        for disk in self.members() {
            let scanned = scan(&disk)?;
            if scanned.torn_tail {
                return Err(required("a torn log cannot exclude an earlier publication"));
            }
            let mut found = None;
            for (record, _) in &scanned.records {
                let c: Candidate = load_object(&disk, "candidate", &record.candidate_hash)?;
                if c.operation_id == operation {
                    found = Some(c);
                }
            }
            let c = found.ok_or_else(|| {
                required("candidate must be present in all configured original member logs")
            })?;
            self.verify_candidate_membership(&c)?;
            let hash = object_hash("candidate", &c)?;
            if candidate.as_ref().is_some_and(|existing: &Candidate| {
                object_hash("candidate", existing).ok().as_deref() != Some(hash.as_str())
            }) {
                return Err(required("original members disagree about the candidate"));
            }
            if c.previous_snapshot != self.snapshot || c.pool_id != self.config.pool_id {
                return Err(required(
                    "candidate does not extend the applied publication prefix",
                ));
            }
            let tail = scanned
                .records
                .last()
                .ok_or_else(|| required("member journal is empty"))?;
            if tail.0.sequence != c.sequence {
                return Err(required("recovery candidate is not the last journal slot"));
            }
            let mut records = scanned
                .records
                .iter()
                .filter(|(r, _)| r.sequence == c.sequence)
                .map(|(record, frame_hash)| Receipt {
                    disk_id: disk.disk_id.clone(),
                    slot_id: disk.slot_id.clone(),
                    frame_hash: frame_hash.clone(),
                    record: record.clone(),
                })
                .collect::<Vec<_>>();
            if let Some(plan) = resume {
                let expected = plan
                    .members
                    .iter()
                    .find(|m| m.disk_id == disk.disk_id)
                    .ok_or_else(|| required("recovery member missing"))?;
                if records.len() < expected.records.len()
                    || canonical_json(&records[..expected.records.len()])?
                        != canonical_json(&expected.records)?
                {
                    return Err(required("original recovery log prefix differs"));
                }
                let plan_hash = object_hash("recovery-plan", plan)?;
                for record in &records[expected.records.len()..] {
                    let valid = match record.record.record_type {
                        RecordType::Reprotect => record.record.object_hash == plan_hash,
                        RecordType::Finalize => {
                            load_object::<Finalize>(&disk, "finalize", &record.record.object_hash)?
                                .recovery_id
                                .as_deref()
                                == Some(plan.recovery_id.as_str())
                        }
                        RecordType::Prepare | RecordType::Accept | RecordType::Publication => false,
                    };
                    if !valid || record.record.candidate_hash != hash {
                        return Err(required(
                            "unexpected evidence appeared after recovery preview",
                        ));
                    }
                }
                records = expected.records.clone();
            }
            if records.iter().any(|r| {
                r.record.candidate_hash != hash
                    || r.record.record_type == RecordType::Publication
                    || r.record.record_type == RecordType::Reprotect
            }) {
                return Err(required(
                    "publication or another recovery must be reconciled first",
                ));
            }
            for receipt in records
                .iter()
                .filter(|r| r.record.record_type == RecordType::Accept)
            {
                if !receipt_binds(receipt, RecordType::Accept, c.sequence, &hash, &hash)? {
                    return Err(required(
                        "original ACCEPT does not bind the fixed candidate",
                    ));
                }
                accepts.push(receipt.clone());
            }
            if let Some(plan) = resume {
                witnesses.push(
                    plan.members
                        .iter()
                        .find(|m| m.disk_id == disk.disk_id)
                        .expect("member checked")
                        .clone(),
                );
            } else {
                witnesses.push(MemberWitness {
                    disk_id: disk.disk_id.clone(),
                    slot_id: disk.slot_id,
                    binding_generation: disk.binding_generation,
                    log_bytes: scanned.valid_bytes,
                    last_frame_hash: tail.1.clone(),
                    records,
                });
            }
            candidate = Some(c);
        }
        if accepts.len() < self.config.metadata_quorum()
            || receipts_ids(&accepts).len() != accepts.len()
        {
            return Err(required(
                "a configured majority of original durable ACCEPT receipts is required",
            ));
        }
        let candidate = candidate.ok_or_else(|| required("candidate missing"))?;
        let hash = object_hash("candidate", &candidate)?;
        exclude_publication(self, &candidate, &hash)?;
        let state = self.load_bound_snapshot(&candidate.snapshot_hash)?;
        crate::history_package::bound_history_objects(&self.members(), &candidate, Some(&state))?;
        Ok((candidate, state, witnesses, accepts))
    }
    pub fn prepare_degraded_recovery(
        &mut self,
        operation: &str,
        principal: &str,
        reason: &str,
    ) -> Result<RecoveryPlan> {
        if principal.is_empty() || reason.trim().is_empty() || reason.len() > 4096 {
            return Err(Error::new(
                storage_contract::ErrorCode::InvalidRequest,
                "recovery requires a principal and a bounded explicit permanent-loss reason",
            ));
        }
        let (candidate, _, members, original_accepts) =
            self.pending_recovery_candidate(operation, None)?;
        let hash = object_hash("candidate", &candidate)?;
        let old = member_plan_path(&self.members()[0], &hash);
        if old.exists() {
            let plan: RecoveryPlan = read_json(&old)?;
            if plan.principal != principal || plan.digest != plan_digest(&plan)? {
                return Err(required(
                    "a unique recovery plan already exists for this candidate",
                ));
            }
            for disk in self.members() {
                write_json(&member_plan_path(&disk, &hash), &plan, false)?;
                store_object(&disk, "recovery-plan", &plan)?;
            }
            write_json(&plan_path(&self.root, &plan.recovery_id), &plan, false)?;
            return Ok(plan);
        }
        let observed = observe(self, &candidate)?;
        require_permanent_loss(self, &candidate, &observed)?;
        let (version_protection, loss_details) = freeze_observation(observed);
        if !version_protection.iter().any(|v| {
            v["slots"]
                .as_array()
                .is_some_and(|s| s.iter().any(|s| s["state"] != "COMPLETE_VERIFIED"))
        }) {
            return Err(required(
                "all original payloads remain healthy; finish the original normal publication",
            ));
        }
        let mut plan = RecoveryPlan {
            recovery_id: new_id("recovery"),
            digest: String::new(),
            operation_id: operation.into(),
            candidate_hash: hash.clone(),
            principal: principal.into(),
            reason: reason.into(),
            isolation_generation: new_id("fence"),
            sequence: candidate.sequence,
            version_protection,
            loss_details,
            members,
            original_accepts,
        };
        plan.digest = plan_digest(&plan)?;
        for disk in self.members() {
            write_json(&member_plan_path(&disk, &hash), &plan, false)?;
            store_object(&disk, "recovery-plan", &plan)?;
        }
        write_json(&plan_path(&self.root, &plan.recovery_id), &plan, false)?;
        Ok(plan)
    }
    pub fn recovery_plan(&self, id: &str) -> Result<RecoveryPlan> {
        validate_id(id, "recovery")?;
        let local = plan_path(&self.root, id);
        let plan: RecoveryPlan = if local.exists() {
            read_json(&local)?
        } else {
            let mut found: Option<RecoveryPlan> = None;
            for disk in self.members() {
                let directory = disk.root.join("metadata/recovery-plans");
                if !directory.exists() {
                    continue;
                }
                for entry in fs::read_dir(directory)? {
                    let candidate: RecoveryPlan = read_json(&entry?.path())?;
                    if candidate.recovery_id == id {
                        if found
                            .as_ref()
                            .is_some_and(|old| old.digest != candidate.digest)
                        {
                            return Err(required("member recovery plans conflict"));
                        }
                        found = Some(candidate);
                    }
                }
            }
            let recovered = found.ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::RecoveryPlanNotFound,
                    "no member holds this recovery plan",
                )
            })?;
            if recovered.digest != plan_digest(&recovered)? {
                return Err(required("member recovery plan digest differs"));
            }
            write_json(&local, &recovered, false)?;
            recovered
        };
        if plan.recovery_id != id || plan.digest != plan_digest(&plan)? {
            return Err(required("recovery plan digest differs"));
        }
        Ok(plan)
    }
    pub fn commit_degraded_recovery(
        &mut self,
        id: &str,
        digest: &str,
        principal: &str,
        accept_permanent_loss: bool,
    ) -> Result<OperationResult> {
        self.require_no_commit_preparation()?;
        let plan = self.recovery_plan(id)?;
        if plan.digest != digest || plan.principal != principal {
            return Err(Error::new(
                storage_contract::ErrorCode::PermissionDenied,
                "recovery plan binding differs",
            ));
        }
        if !accept_permanent_loss {
            return Err(Error::new(
                storage_contract::ErrorCode::PermanentLossNotAcknowledged,
                "explicit permanent-loss acknowledgement is required",
            ));
        }
        if let Some(result) = self.state.operations.get(&plan.operation_id) {
            if result.result["recovery_id"].as_str() == Some(id) {
                return self.operation(&plan.operation_id);
            }
            return Err(required(
                "the original operation already has a different published result",
            ));
        }
        let (candidate, mut next, members, accepts) =
            self.pending_recovery_candidate(&plan.operation_id, Some(&plan))?;
        let (observed, loss_details) = freeze_observation(observe(self, &candidate)?);
        if canonical_json(&members)? != canonical_json(&plan.members)?
            || canonical_json(&accepts)? != canonical_json(&plan.original_accepts)?
            || canonical_json(&observed)? != canonical_json(&plan.version_protection)?
            || canonical_json(&loss_details)? != canonical_json(&plan.loss_details)?
        {
            return Err(Error::new(
                storage_contract::ErrorCode::PlanStale,
                "recovery evidence changed after preview",
            ));
        }
        require_permanent_loss(self, &candidate, &plan.version_protection)?;
        let member_disks = self.members();
        let history_objects =
            crate::history_package::bound_history_objects(&member_disks, &candidate, Some(&next))?;
        let mut result = next
            .operations
            .get(&candidate.operation_id)
            .cloned()
            .ok_or_else(|| required("candidate result missing"))?;
        result.commit_state = storage_contract::CommitState::PublishedDegraded;
        result.protection_at_publish = storage_contract::ProtectionAtPublish::BelowTarget;
        result.current_health = if plan.version_protection.iter().any(|v| {
            !v["slots"]
                .as_array()
                .expect("slots")
                .iter()
                .any(|s| s["state"] == "COMPLETE_VERIFIED")
        }) {
            "UNAVAILABLE"
        } else {
            "DEGRADED"
        }
        .into();
        result.result["recovery_id"] = json!(id);
        result.result["recovery_plan_digest"] = json!(plan.digest);
        result.result["isolation_generation"] = json!(plan.isolation_generation);
        result.result["outcome"] = json!("COMMITTED_DEGRADED");
        result.result["version_protection"] = json!(plan.version_protection);
        crate::metadata_budget::preflight_bound_publication(
            self,
            &candidate,
            &result,
            &next,
            Some(RecoveryProof {
                plan: plan.clone(),
                reprotect: Vec::new(),
            }),
            MAX_METADATA,
        )?;
        // All graph and combined proof checks precede any recovery writes.
        self.repair_bound_snapshot(&candidate.snapshot_hash)?;
        crate::history_package::repair_history_objects(
            &member_disks,
            &history_objects,
            &mut BTreeSet::new(),
        )?;
        let mut reprotect = Vec::new();
        let plan_hash = object_hash("recovery-plan", &plan)?;
        for disk in &member_disks {
            store_object(disk, "candidate", &candidate)?;
            store_object(disk, "recovery-plan", &plan)?;
            reprotect.push(append(
                disk,
                RecordType::Reprotect,
                candidate.sequence,
                &plan.candidate_hash,
                &plan_hash,
            )?);
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!("after_recovery_reprotect_{}", reprotect.len()))?;
        }
        let finalize = Finalize {
            candidate_hash: plan.candidate_hash.clone(),
            accepts: plan.original_accepts.clone(),
            recovery_id: Some(id.into()),
        };
        let final_hash = object_hash("finalize", &finalize)?;
        let mut finalize_receipts = Vec::new();
        for disk in &member_disks {
            store_object(disk, "finalize", &finalize)?;
            finalize_receipts.push(append(
                disk,
                RecordType::Finalize,
                candidate.sequence,
                &plan.candidate_hash,
                &final_hash,
            )?);
        }
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_recovery_finalize")?;
        let mut payload_receipts = Vec::new();
        for evidence in &candidate.versions {
            let health = plan
                .version_protection
                .iter()
                .find(|v| v["version_id"] == evidence.version.version_id)
                .ok_or_else(|| required("version health missing"))?;
            for slot in &evidence.placement.route_slot_ids {
                if !health["slots"]
                    .as_array()
                    .expect("frozen slots")
                    .iter()
                    .any(|s| s["slot_id"] == *slot && s["state"] == "COMPLETE_VERIFIED")
                {
                    continue;
                }
                let disk = self.disk(slot)?;
                let dir = version_dir(
                    disk,
                    &evidence.version.history_tree_id,
                    &evidence.version.version_id,
                );
                verify_payload(&dir, &evidence.version)?;
                let proof = CommitProof {
                    candidate_hash: plan.candidate_hash.clone(),
                    finalize: finalize.clone(),
                    version_hash: object_hash("version", &evidence.version)?,
                    disk_id: disk.disk_id.clone(),
                    slot_id: slot.clone(),
                    route_slot_ids: evidence.placement.route_slot_ids.clone(),
                };
                let proof_dir = local_proof_directory(&dir, &plan.candidate_hash)?;
                write_json(&proof_dir.join("commit-proof.json"), &proof, true)?;
                payload_receipts.push(PayloadReceipt {
                    version_id: evidence.version.version_id.clone(),
                    disk_id: disk.disk_id.clone(),
                    slot_id: slot.clone(),
                    proof_hash: object_hash("commit-proof", &proof)?,
                });
            }
        }
        let publication = Publication {
            candidate_hash: plan.candidate_hash.clone(),
            finalize,
            finalize_receipts,
            payload_receipts,
            result: result.clone(),
            recovery: Some(RecoveryProof {
                plan: plan.clone(),
                reprotect,
            }),
        };
        publication
            .recovery
            .as_ref()
            .expect("recovery")
            .verify(&candidate, &publication)?;
        self.complete_candidate(&candidate, &plan.candidate_hash, &next, Some(publication))?;
        next.operations
            .insert(candidate.operation_id.clone(), result.clone());
        self.state = std::sync::Arc::new(next);
        self.snapshot = Some(candidate.snapshot_hash);
        self.next_slot = self.next_slot.max(
            candidate
                .sequence
                .checked_add(1)
                .ok_or_else(|| required("decision sequence exhausted"))?,
        );
        // Publication is durable, but a failed local reconciliation must retain
        // the write fence, just like ordinary publication's index update. A
        // restart can rebuild the disposable index and reconcile workspaces.
        self.rebuild_index()?;
        self.load_workspaces()?;
        self.blocked = false;
        self.operation(&result.operation_id)
    }
}

#[cfg(test)]
#[path = "../../../test/engine/unit/degraded_recovery/tests.rs"]
mod tests;
