use crate::journal::RecordType;
use crate::{
    disk::*,
    journal::*,
    *,
};
use json_codec::json;
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::fs;

impl Engine {
    pub fn create_purge_plan(
        &mut self,
        history: &str,
        versions: Vec<String>,
        principal: &str,
    ) -> Result<Plan> {
        Self::require_actor(principal)?;
        self.require_write()?;
        self.repository_for_history(history)?;
        let mut ids = versions;
        ids.sort();
        ids.dedup();
        if ids.is_empty() {
            return Err(Error::new(
                storage_contract::ErrorCode::InvalidRequest,
                "purge selection is empty",
            ));
        }
        self.check_purge(history, &ids)?;
        self.save_purge_plan(
            principal,
            json!({"history_tree_id":history,"version_ids":ids}),
        )
    }
    fn save_purge_plan(&mut self, principal: &str, input: json_codec::Value) -> Result<Plan> {
        crate::ordinary_authority::plan_input("purge", &input)?;
        let plan = Plan {
            plan_id: new_id("plan")?,
            kind: "purge".into(),
            principal: principal.into(),
            expected_sequence: self.state.business_generation,
            digest: object_hash_value("plan", &input)?,
            input,
            state: PlanState::Ready,
        };
        // Plans are durable local preparations; business effects still require publication.
        write_json(
            &self
                .root
                .join("plans")
                .join(format!("{}.json", plan.plan_id)),
            &plan,
            false,
        )?;
        Ok(plan)
    }
    pub fn cancel_plan(
        &mut self,
        id: &str,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        Self::require_actor(principal)?;
        let input = json!({"plan_id": id});
        let digest = Self::native_request_digest("plan.cancel", principal, &input)?;
        if let Some(op) = self.state.operations.get(&operation) {
            if op.kind == "plan.cancel"
                && op.operation_id == operation
                && op.request_digest == digest
                && op.result["plan_id"].as_str() == Some(id)
                && self.operation_owner(operation)?.as_deref() == Some(principal)
            {
                return Ok(self.operation_record(operation)?.into_result());
            }
            return Err(Error::new(
                storage_contract::ErrorCode::IdempotencyKeyReused,
                "operation already names a different action",
            ));
        }
        let plan = self.plan(id)?;
        if plan.principal != principal {
            return Err(Error::new(
                storage_contract::ErrorCode::PermissionDenied,
                "plan belongs to another principal",
            ));
        }
        if plan.state != PlanState::Ready {
            return Err(Error::new(
                storage_contract::ErrorCode::PlanClosed,
                "only an unexecuted plan can be cancelled",
            ));
        }
        let mut cancelled = plan;
        cancelled.state = PlanState::Cancelled;
        let cancelled_plan_hash = object_hash("plan-cancellation", &cancelled)?;
        self.mutate_digest(
            "plan.cancel",
            principal,
            operation,
            input,
            Some(digest),
            |_, state, _| {
                state.plans.insert(id.into(), cancelled);
                Ok(json!({
                    "plan_id": id,
                    "state": "cancelled",
                    "cancelled_plan_hash": cancelled_plan_hash
                }))
            },
        )
    }

    pub fn plan(&self, id: &str) -> Result<Plan> {
        validate_id(id, "plan")?;
        let plan = if let Some(plan) = self.state.plans.get(id) {
            plan.clone()
        } else {
            use platform_io::fs_safety::{
                RelativePath,
                linux::{MountPolicy, OpenAt2Root},
            };
            // Only a missing descendant of a verified system root is absence.
            // A fresh pool need not have created its optional plans directory.
            let private = existing_private_dir(&self.root)?;
            let root =
                OpenAt2Root::open(private.path(), MountPolicy::SameMount).map_err(fs_error)?;
            let relative = RelativePath::new(format!("plans/{id}.json")).map_err(fs_error)?;
            let file = match root.open_file(&relative, false) {
                Ok(file) => file,
                Err(platform_io::fs_safety::Error::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound =>
                {
                    return Err(Error::new(
                        storage_contract::ErrorCode::PlanNotFound,
                        "plan not found",
                    ));
                }
                Err(error) => return Err(fs_error(error)),
            };
            decode_strict::<Plan>(&read_file_bytes(file, MAX_METADATA)?)?
        };
        crate::ordinary_authority::plan_input(&plan.kind, &plan.input)?;
        // Every admitted plan must execute exactly the input that was previewed.
        // Checking only the stored digest field would accept a stale digest
        // paired with modified local purge input after restart.
        if plan.plan_id != id || object_hash_value("plan", &plan.input)? != plan.digest {
            return Err(Error::new(
                storage_contract::ErrorCode::PlanInvalid,
                "plan identity or input digest differs",
            ));
        }
        Ok(plan)
    }
    pub fn commit_plan(
        &mut self,
        id: &str,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        if let Some(op) = self.state.operations.get(&operation) {
            if op.kind == "content.purge" && op.result["plan_id"].as_str() == Some(id)
                && self.operation_owner(operation)?.as_deref() == Some(principal)
            {
                // The receipt proves publication, not completion of physical
                // cleanup. Resume that idempotent phase after a failed attempt.
                self.cleanup_purged()?;
                return Ok(self.operation_record(operation)?.into_result());
            }
            return Err(Error::new(
                storage_contract::ErrorCode::IdempotencyKeyReused,
                "operation already used",
            ));
        }
        let plan = self.plan(id)?;
        if plan.principal != principal {
            return Err(Error::new(
                storage_contract::ErrorCode::PermissionDenied,
                "plan belongs to another principal",
            ));
        }
        if plan.state != PlanState::Ready {
            return Err(Error::new(
                storage_contract::ErrorCode::PlanClosed,
                "plan is no longer ready",
            ));
        }
        if plan.expected_sequence != self.state.business_generation {
            return Err(Error::new(
                storage_contract::ErrorCode::PlanStale,
                "published state changed; preview a new plan",
            ));
        }
        let history = plan.input["history_tree_id"]
            .as_str()
            .ok_or_else(|| Error::new(storage_contract::ErrorCode::PlanInvalid, "history missing"))?
            .to_string();
        self.repository_for_history(&history)?;
        let ids = plan.input["version_ids"]
            .as_array()
            .ok_or_else(|| {
                Error::new(storage_contract::ErrorCode::PlanInvalid, "versions missing")
            })?
            .iter()
            .map(|v| {
                v.as_str().map(str::to_string).ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::PlanInvalid,
                        "version id is not a string",
                    )
                })
            })
            .collect::<Result<Vec<_>>>()?;
        self.check_purge(&history, &ids)?;
        let result = self.mutate(
            "content.purge",
            principal,
            operation,
            json!({"plan_id":id,"digest":plan.digest}),
            |_, state, _| {
                for version in &ids {
                    state
                        .content_states
                        .insert(version.clone(), ContentState::Purged);
                }
                let mut p = plan.clone();
                p.state = PlanState::Committed;
                state.plans.insert(id.into(), p);
                Ok(json!({"plan_id":id,"history_tree_id":history,"purged_versions":ids}))
            },
        )?;
        self.cleanup_purged()?;
        Ok(result)
    }
    pub(crate) fn check_purge(&self, history: &str, versions: &[String]) -> Result<()> {
        self.check_purge_with_archived(history, versions, &BTreeSet::new())
    }
    /// Only the exact branch set frozen by a complete terminal decision may be
    /// read as immutable evidence instead of a live HEAD. Every other purge
    /// admission, including leases, directories, Git and workspaces, is unchanged.
    pub(crate) fn check_retirement_purge(
        &self,
        history: &str,
        versions: &[String],
        decision: &crate::node_retirement_engine::Decision,
    ) -> Result<()> {
        self.check_purge_with_archived(history, versions, &decision.archived_branch_ids())
    }
    fn check_purge_with_archived(
        &self,
        history: &str,
        versions: &[String],
        archived: &BTreeSet<String>,
    ) -> Result<()> {
        let selected = versions.iter().collect::<BTreeSet<_>>();
        let reads = self
            .reads
            .lock()
            .expect("invariant: read lease lock poisoned");
        let counts = reads.counts()?;
        // Scan each HEAD collection at most once, retaining only selected IDs.
        // Laziness preserves cheap missing/history/lease rejection and the
        // existing branch-before-directory error precedence.
        let branch_heads = std::cell::LazyCell::new(|| {
            self.state
                .branches
                .values()
                .filter(|branch| {
                    branch.state == BranchState::Active && !archived.contains(&branch.branch_id)
                })
                .filter_map(|branch| self.state.commits.get(&branch.head_commit_id))
                .flat_map(|commit| &commit.entries)
                .filter(|entry| selected.contains(&entry.version_id))
                .map(|entry| entry.version_id.as_str())
                .collect::<BTreeSet<_>>()
        });
        let directory_heads = std::cell::LazyCell::new(|| {
            self.state
                .directory_branches
                .values()
                .flat_map(|branch| &branch.entries)
                .filter(|entry| {
                    entry.kind == NamespaceEntryKind::File && selected.contains(&entry.version_id)
                })
                .map(|entry| entry.version_id.as_str())
                .collect::<BTreeSet<_>>()
        });
        let git_versions = std::cell::LazyCell::new(|| {
            crate::business::directory_git::protected_git_versions(&self.state, &selected)
        });
        for id in versions {
            let v = self.state.versions.get(id).ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::VersionNotFound,
                    "selected version missing",
                )
            })?;
            if v.history_tree_id != history {
                return Err(Error::new(
                    storage_contract::ErrorCode::HistoryMismatch,
                    "purge version belongs to another history",
                ));
            }
            if counts.get(id).copied().unwrap_or(0) > 0 {
                return Err(Error::new(
                    storage_contract::ErrorCode::ContentPinned,
                    "version has an active read lease",
                ));
            }
            if branch_heads.contains(id.as_str()) {
                return Err(Error::new(
                    storage_contract::ErrorCode::ContentPinned,
                    "version is used by an active branch HEAD",
                ));
            }
            if directory_heads.contains(id.as_str()) {
                return Err(Error::new(
                    storage_contract::ErrorCode::ContentPinned,
                    "version is referenced by a retained directory branch HEAD",
                ));
            }
        }
        for w in self
            .workspaces
            .values()
            .filter(|w| matches!(w.state, WorkspaceState::Open | WorkspaceState::Sealed))
        {
            if self
                .state
                .commits
                .get(&w.base_head)
                .is_some_and(|c| c.entries.iter().any(|e| selected.contains(&e.version_id)))
            {
                return Err(Error::new(
                    storage_contract::ErrorCode::ContentPinned,
                    "an open workspace needs a selected version",
                ));
            }
        }
        // Preserve every existing rejection before adding the Git snapshot
        // roots. The lazy projection scans those roots once for the whole batch.
        for id in versions {
            if git_versions.contains(id.as_str()) {
                return Err(Error::new(
                    storage_contract::ErrorCode::ContentPinned,
                    "version is referenced by a directory Git HEAD or index",
                ));
            }
        }
        Ok(())
    }
    pub(crate) fn cleanup_purged(&mut self) -> Result<()> {
        self.require_unfenced_membership()?;
        self.require_no_commit_preparation()?;
        if !self.state.content_states.values().any(|s| {
            matches!(
                s,
                ContentState::Purged | ContentState::Migrated | ContentState::Retired
            )
        }) {
            return Ok(());
        }
        // Embed the hashed state page and its publication closure in each local
        // tombstone, so an offline disk can distinguish intentional removal
        // from lost bytes without consulting another disk or SQLite.
        let mut published = None;
        let crate::engine::EvidenceMembers {
            members,
            first_error,
        } = self.trusted_evidence_members();
        for member in &members {
            let scanned = self.metadata_integrity.observe(scan_configured_at(
                &self.disk_resources.get(member)?.root,
                member,
                &self.config,
            ))?;
            if let Some((record, _)) = scanned.records.iter().rev().find(|(r, _)| {
                r.sequence == self.state.sequence && r.record_type == RecordType::Publication
            }) {
                let candidate: Candidate =
                    crate::publication_evidence::verified_object_observed_at(
                        &self.disk_resources,
                        &members,
                        "candidate",
                        &record.candidate_hash,
                        &mut |error| self.metadata_integrity.fault(error),
                    )?;
                self.verify_candidate_membership(&candidate)?;
                let publication: Publication =
                    crate::publication_evidence::verified_object_observed_at(
                        &self.disk_resources,
                        &members,
                        "publication",
                        &record.object_hash,
                        &mut |error| self.metadata_integrity.fault(error),
                    )?;
                let seal = crate::publication_evidence::verified_publication_seal_observed_at(
                    &self.disk_resources,
                    &members,
                    &candidate,
                    &publication,
                    &mut |error| self.metadata_integrity.fault(error),
                )?;
                if candidate.sequence != self.state.sequence
                    || self.snapshot.as_deref() != Some(candidate.snapshot_hash.as_str())
                {
                    return Err(Error::new(
                        storage_contract::ErrorCode::TombstoneInvalid,
                        "tombstone publication differs from recovered state",
                    ));
                }
                let snapshot: Snapshot = crate::publication_evidence::verified_object_observed_at(
                    &self.disk_resources,
                    &members,
                    "snapshot",
                    &candidate.snapshot_hash,
                    &mut |error| self.metadata_integrity.fault(error),
                )?;
                published = Some((candidate, publication, seal, snapshot));
                break;
            }
        }
        let (candidate, publication, seal, snapshot) =
            published.ok_or_else(|| match first_error {
                Some(error) => error.context(
                    storage_contract::ErrorCode::RecoveryRequired,
                    "published tombstone evidence is unavailable",
                ),
                None => Error::new(
                    storage_contract::ErrorCode::RecoveryRequired,
                    "published tombstone evidence is unavailable",
                ),
            })?;
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_purge_evidence")?;
        // Reading the publication closure does not authorize destructive I/O
        // indefinitely. Re-admit each actual cleanup target at first use, not
        // for every version, retaining the existing unavailable-target skip.
        let mut cleanup_slots = BTreeMap::new();
        let original_bindings =
            crate::node_retirement_engine::original_cleanup_bindings(&self.state)?;
        for (id, state) in &self.state.content_states {
            if *state == ContentState::Available {
                continue;
            }
            let v = self
                .state
                .versions
                .get(id)
                .expect("invariant: every content state references an existing version");
            let p = self
                .state
                .placements
                .get(id)
                .expect("invariant: every version has a placement");
            validate_snapshot_schema(&snapshot)?;
            let (value, content_state_proof) = crate::metadata_map::read_proof(
                &snapshot.maps["content_states"],
                id,
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
            if value.as_ref().and_then(json_codec::Value::as_str) != Some(state.as_str()) {
                return Err(Error::new(
                    ErrorCode::TombstoneInvalid,
                    "published state proof does not contain the tombstone",
                ));
            }
            let tombstone = ContentTombstone {
                version_id: id.clone(),
                state: *state,
                candidate: candidate.clone(),
                publication: publication.clone(),
                publication_seal: seal.clone(),
                snapshot: snapshot.clone(),
                content_state_proof,
            };
            // Keep the admitted route order: a failure on an earlier replica
            // must not let later replicas clean up before that failure is seen.
            let mut targets = p
                .route_slot_ids
                .iter()
                .map(|slot| (slot.clone(), p.binding_generations[slot]))
                .collect::<Vec<_>>();
            if *state == ContentState::Retired {
                let bindings = original_bindings.get(id).ok_or_else(|| {
                    Error::new(
                        ErrorCode::PublicationInvalid,
                        "retired body has no permanent original cleanup bindings",
                    )
                })?;
                let mut included = targets.iter().cloned().collect::<BTreeSet<_>>();
                targets.extend(
                    bindings
                        .iter()
                        .map(|binding| (binding.slot_id.clone(), binding.binding_generation))
                        .filter(|binding| included.insert(binding.clone())),
                );
            }
            for (slot, generation) in targets {
                // Never reopen a withdrawn root or substitute today's disk for
                // the original binding. The receipt keeps this obligation pending.
                let Some(d) = self
                    .config
                    .disks
                    .iter()
                    .find(|disk| disk.slot_id == slot && disk.binding_generation == generation)
                else {
                    continue;
                };
                let admitted = cleanup_slots.entry((slot, generation)).or_insert_with(|| {
                    crate::engine::provenance::validate_owned_seal(
                        &self.disk_resources,
                        d,
                        &self.metadata_integrity,
                    )
                    .is_ok()
                });
                if !*admitted {
                    continue;
                }
                let handle = self
                    .disk_resources
                    .available(d)
                    .expect("invariant: an admitted cleanup slot retains its disk handle");
                let scope = crate::engine::fresh_layout::containers::observe_replica(
                    &self.config, &self.disk_resources, &self.state, v.file_id.parse()?, d,
                )?;
                let material = VersionMaterial::current(&scope, v)?;
                let root = material.root()?;
                let dir = material.directory()?;
                match root.directory_identity(&dir) {
                    Ok(_) => {}
                    Err(Error::Filesystem(platform_io::fs_safety::Error::Io(error)))
                    | Err(Error::Contract(storage_contract::Error::Io { source: error, .. }))
                        if error.kind() == std::io::ErrorKind::NotFound =>
                    {
                        continue;
                    }
                    Err(error) => return Err(error),
                }
                if *state == ContentState::Retired {
                    let stored: FileVersion =
                        root.read_json_observed(&dir.join("version.json"), &mut |_| Ok(()))?;
                    if stored != *v {
                        return Err(Error::new(
                            ErrorCode::ObjectCorrupt,
                            "retirement cleanup descriptor differs from its retained identity",
                        ));
                    }
                    let metadata = crate::engine::provenance::MetadataReadContext::for_disk(
                        &self.config,
                        &handle.seal,
                        d.metadata_member
                            .then_some(self.metadata_integrity.as_ref()),
                    );
                    verify_source_version_publication(&material, &mut |_| Ok(()), Some(&metadata))?;
                }
                root.write_json(&dir.join("purged.json"), &tombstone, true)?;
                // Version availability is independent of whole-container
                // lifecycle. Keep both removals under their already admitted
                // Current source domains; neither path can select an owner.
                let body = scope.body_path(v)?;
                let container = scope.container()?;
                container.directory_identity(body.parent().ok_or(
                    crate::engine::FreshLayoutError::ContainerScope,
                )?)?;
                container.open_optional_read_regular(&body)?;
                let prepared = dir.join(crate::persist::layout::PREPARED_CONTENT.as_path())
                    .join(&v.creation_name);
                let prepared_present = root.open_optional_read_regular(&prepared)?.is_some();
                container.remove_file_if_present_synced(&body)?;
                if prepared_present {
                    root.remove_file_if_present_synced(&prepared)?;
                }
                material.verify()?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../test/engine/unit/maintenance_corruption_tests.rs"]
mod corruption_tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/maintenance/cleanup_tests.rs"]
mod cleanup_tests;
