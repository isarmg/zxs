//! Replay the verified publication prefix and reconcile pending decisions.
use super::Engine;
use crate::journal::RecordType;
use crate::publication_evidence::{
    repair_object_copies_at, verified_object_at, verified_publication_seal_at,
    verified_recovery_snapshot_at,
};
use crate::{disk::*, journal::*, *};
use std::collections::{BTreeMap, BTreeSet};

mod payload_admission;
mod workspace_receipts;
pub(crate) use workspace_receipts::{
    InstalledOriginalReceipts, OriginalReceiptCursor, OriginalReceiptPrefix,
    ReadOnlyPublishedCandidate, DirectoryReplayPrefix, ContentReplayCut,
};
pub(in crate::engine) mod container_locations;

/// Read-only journal admission, retained with its held descriptors until apply.
pub(super) struct PreparedRecovery {
    members: Vec<DiskConfig>,
    online: Vec<DiskConfig>,
    prefixes: BTreeMap<String, BTreeSet<String>>,
    journals: BTreeMap<String, OpenJournal>,
    records: super::recovery_checkpoint::Records,
    torn_tail: bool,
    candidates: BTreeMap<u64, workspace_receipts::PreparedCandidate>,
    container_locations: container_locations::ReplayContainerLocations,
    original_receipts: workspace_receipts::OriginalReceiptCursor,
    directory_checkpoints: BTreeMap<u64, workspace_receipts::CheckpointDirectoryObservation>,
    original_content_prepare: Option<(u64, String)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RecoveryMode {
    CompletePending,
    PreservePending,
}

impl Engine {
    /// A surviving full publication barrier can disprove an empty or truncated
    /// journal, but cannot authorize inventing the missing decision history.
    /// Check before any recovery repair or disposable-index rebuild can write.
    fn reject_unjournaled_publications(
        &self,
        online: &[DiskConfig],
        journaled: &BTreeSet<&str>,
    ) -> Result<()> {
        for disk in online {
            let root = &self.disk_resources.get(disk)?.root;
            let directory = disk.root.join("metadata/publication-seals");
            match root.optional_metadata(&directory)? {
                None => continue,
                Some(_) => {
                    root.directory(&directory)?;
                }
            }
            for entry in root.entries(&directory)? {
                let entry = entry?;
                let name = entry.as_os_str();
                let Some(hash) = name.to_str().and_then(|name| name.strip_suffix(".json")) else {
                    // An unfinished atomic-write temporary is not a seal.
                    continue;
                };
                if journaled.contains(hash) {
                    continue;
                }
                let seal: PublicationSeal =
                    root.read_json_observed(&directory.join(entry.as_path()), &mut |_| Ok(()))?;
                let candidate: Candidate =
                    verified_object_at(&self.disk_resources, online, "candidate", hash)?;
                self.verify_candidate_membership(&candidate)?;
                // Ancestor publications belong to their exact archived journal,
                // never to this generation's active namespace.
                if self.config.generation().is_some_and(|generation| {
                    generation.historical_candidate(candidate.sequence) == Some(hash)
                }) {
                    continue;
                }
                let publication: Publication = verified_object_at(
                    &self.disk_resources,
                    online,
                    "publication",
                    &seal.publication_hash,
                )?;
                verify_publication_seal(&candidate, &publication, &seal)?;
                if candidate.pool_id != self.config.pool_id {
                    return Err(Error::new(
                        storage_contract::ErrorCode::PublicationInvalid,
                        "publication seal belongs to another pool",
                    ));
                }
                return Err(Error::new(
                    storage_contract::ErrorCode::RecoveryRequired,
                    "journal history omits a sealed publication; restore the missing journal evidence before recovery",
                ));
            }
        }
        Ok(())
    }

    /// Read the complete exact candidate-bound object closure across replicas.
    /// A damaged preferred member must not hide healthy metadata dependencies.
    pub(crate) fn load_bound_snapshot(&self, hash: &str) -> Result<State> {
        Ok(crate::publication_evidence::verified_snapshot_observed_at(
            &self.disk_resources,
            &self.members(),
            hash,
            &mut |error| self.metadata_integrity.fault(error),
        )?
        .into_state())
    }

    /// Preserve the candidate's original snapshot layout when completing a
    /// persisted decision, including decisions made by an older page writer.
    pub(crate) fn repair_bound_snapshot(&self, hash: &str) -> Result<()> {
        let members = self.members();
        crate::publication_evidence::verified_snapshot_observed_at(
            &self.disk_resources,
            &members,
            hash,
            &mut |error| self.metadata_integrity.fault(error),
        )?;
        repair_snapshot_copies(
            &self.disk_resources,
            &members,
            &members,
            hash,
            &mut BTreeSet::new(),
        )
    }

    fn repair_journal_tails(&mut self) -> Result<()> {
        for journal in self.journals.values_mut() {
            if let Err(error) = journal.repair_torn_tail() {
                self.blocked = true;
                return Err(error);
            }
        }
        Ok(())
    }

    pub(super) fn prepare_recovery(&mut self) -> Result<PreparedRecovery> {
        self.prepare_recovery_with_warnings(Vec::new())
    }

    pub(super) fn prepare_recovery_with_warnings(
        &mut self,
        admission_warnings: Vec<super::RecoverySealFailure>,
    ) -> Result<PreparedRecovery> {
        self.metadata_integrity.invalidate();
        self.published_state_base = None;
        // Only this invocation's read-only admission may seed diagnostics.
        // Excluded roots will not be visited again by the journal scan below.
        self.recovery_warnings = super::RecoveryWarnings {
            seals: admission_warnings,
            ..super::RecoveryWarnings::default()
        };
        let members = self.members();
        let mut online = Vec::new();
        let mut prefixes = BTreeMap::<String, BTreeSet<String>>::new();
        let mut opened_journals = BTreeMap::new();
        let mut rejected_anchors = BTreeSet::new();
        let mut torn_tail = false;
        let mut records = super::recovery_checkpoint::Records::new();
        for d in &members {
            let Some(handle) = self.disk_resources.available(d) else {
                continue;
            };
            if let Err(source) = super::provenance::validate_owned_seal(
                &self.disk_resources,
                d,
                &self.metadata_integrity,
            ) {
                self.recovery_warnings
                    .seals
                    .push(super::RecoverySealFailure {
                        context: super::RecoverySealContext::Member {
                            disk_id: d.disk_id.clone(),
                            slot_id: d.slot_id.clone(),
                        },
                        source,
                    });
                continue;
            }
            // Even an empty genesis journal must retain its original membership.
            // An edited system Config cannot establish a replacement voter set.
            let anchor = self.config.verify_root_membership(&handle.root, d);
            if let Err(source) = anchor {
                self.metadata_integrity.fault(&source);
                rejected_anchors.insert(d.disk_id.clone());
                self.recovery_warnings
                    .seals
                    .push(super::RecoverySealFailure {
                        context: super::RecoverySealContext::MembershipAnchor {
                            disk_id: d.disk_id.clone(),
                            slot_id: d.slot_id.clone(),
                        },
                        source,
                    });
                continue;
            }
            let opened = OpenJournal::scan_configured_at(&handle.root, d, &self.config)?;
            let scanned = &opened.scan;
            torn_tail |= scanned.torn_tail;
            prefixes.insert(
                d.disk_id.clone(),
                scanned
                    .records
                    .iter()
                    .map(|(_, hash)| hash.clone())
                    .collect(),
            );
            for (r, frame_hash) in &scanned.records {
                self.next_slot =
                    self.next_slot.max(r.sequence.checked_add(1).ok_or_else(|| {
                        Error::new(
                            storage_contract::ErrorCode::RecoveryRequired,
                            "journal decision sequence exhausted",
                        )
                    })?);
                records
                    .entry(r.sequence)
                    .or_default()
                    .entry(r.candidate_hash.clone())
                    .or_default()
                    .push(Receipt {
                        disk_id: d.disk_id.clone(),
                        slot_id: d.slot_id.clone(),
                        frame_hash: frame_hash.clone(),
                        record: r.clone(),
                    });
            }
            opened_journals.insert(d.disk_id.clone(), opened);
            online.push(d.clone());
        }
        if online.len() < self.config.metadata_quorum() {
            let message = "online service requires a majority of the configured metadata members; use offline recovery";
            if let Some(index) = self.recovery_warnings.seals.iter().position(|failure| {
                matches!(
                    failure.context,
                    super::RecoverySealContext::MembershipAnchor { .. }
                )
            }) {
                let super::RecoverySealFailure { context, source } =
                    self.recovery_warnings.seals.remove(index);
                return Err(source.context(
                    ErrorCode::MetadataQuorumUnavailable,
                    format!("{message}; rejected {context:?}"),
                ));
            }
            return Err(Error::new(ErrorCode::MetadataQuorumUnavailable, message));
        }
        if !rejected_anchors.is_empty() {
            self.disk_resources =
                std::sync::Arc::new(self.disk_resources.excluding(&rejected_anchors));
        }
        if let Some(selected) = &self.move_completion {
            // A selector may be retained before the first PREPARE. Keep an
            // in-memory cursor to its exact stored candidate, not a fake frame.
            records
                .entry(selected.sequence())
                .or_default()
                .entry(selected.candidate_hash().to_owned())
                .or_default();
            self.next_slot = self
                .next_slot
                .max(selected.sequence().checked_add(1).ok_or_else(|| {
                    Error::new(ErrorCode::ResourceLimit, "selected move sequence exhausted")
                })?);
        }
        let journaled = records
            .values()
            .flat_map(|candidates| {
                candidates
                    .iter()
                    .filter(|(_, records)| {
                        records.iter().any(|receipt| {
                            matches!(
                                receipt.record.record_type,
                                RecordType::Accept | RecordType::Publication
                            )
                        })
                    })
                    .map(|(hash, _)| hash.as_str())
            })
            .collect();
        self.reject_unjournaled_publications(&online, &journaled)?;
        self.admit_membership_fence(&online, &records)?;
        Ok(PreparedRecovery {
            members,
            online,
            prefixes,
            journals: opened_journals,
            records,
            torn_tail,
            candidates: BTreeMap::new(),
            directory_checkpoints: BTreeMap::new(),
            original_content_prepare: None,
            container_locations: container_locations::ReplayContainerLocations::default(),
            original_receipts: workspace_receipts::OriginalReceiptCursor::for_recovery(self)?,
        })
    }

    #[cfg(test)]
    pub(super) fn recover_mode(&mut self, mode: RecoveryMode) -> Result<()> {
        let mut prepared = self.prepare_recovery()?;
        self.preflight_workspace_receipts(&mut prepared)?;
        self.recover_prepared(mode, prepared)
    }

    pub(super) fn recover_prepared(
        &mut self,
        mode: RecoveryMode,
        prepared: PreparedRecovery,
    ) -> Result<()> {
        let move_retained = self.move_is_retained();
        if let Some(selected) = &self.move_completion
            && prepared
                .records
                .keys()
                .any(|sequence| *sequence > selected.sequence())
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "journal extends beyond the retained complete move candidate",
            ));
        }
        if let Some(choice) = &self.history_move_choice {
            if prepared
                .candidates
                .keys()
                .any(|slot| *slot > choice.decision_slot())
            {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "journal extends beyond the unresolved original move decision",
                ));
            }
            self.admit_selected_move_prepares(&prepared.records, prepared.torn_tail)?;
        }
        let PreparedRecovery {
            members,
            online,
            prefixes,
            journals: mut opened_journals,
            records,
            torn_tail,
            mut candidates,
            container_locations,
            original_receipts,
            mut directory_checkpoints,
            original_content_prepare,
        } = prepared;
        let mut original_receipts = std::sync::Arc::new(original_receipts);
        // Preflight retained these originals under these exact held resources.
        // An archived seed must have supplied its actual original/outcome
        // stream. The frozen visible State alone cannot fill missing pairs.
        self.original_receipts = original_receipts.install(
            &self.config, &self.disk_resources, &self.state, &self.snapshot,
        )?;
        let fenced = self.membership_fence.is_some();
        let members_ready = online.len() == members.len() && !fenced;
        let can_repair = members_ready && !move_retained;
        let mut scanned_journals = BTreeMap::new();
        for disk in &online {
            let opened = opened_journals
                .remove(&disk.disk_id)
                .expect("scanned online journal");
            let (writer, scanned) = opened.into_writer()?;
            scanned_journals.insert(disk.disk_id.clone(), scanned);
            self.journals.insert(disk.disk_id.clone(), writer);
        }
        let exit_generation = self
            .config
            .generation()
            .is_some_and(|generation| generation.registry().is_some());
        let checkpoint = if can_repair && !torn_tail && !exit_generation {
            match super::recovery_checkpoint::load(self, &prefixes, &records) {
                Ok(checkpoint) => checkpoint.filter(|c| {
                    records
                        .get(&c.sequence)
                        .is_some_and(|candidates| candidates.contains_key(&c.candidate_hash))
                }),
                Err(error) => {
                    self.recovery_warnings.checkpoint_load = Some(error);
                    None
                }
            }
        } else {
            None
        };
        tracing::info!(
            target: "storage_engine::recovery_checkpoint",
            checkpoint_selected = checkpoint.is_some(),
            checkpoint_sequence = checkpoint.as_ref().map(|value| value.sequence),
            checkpoint_candidate_hash = checkpoint.as_ref().map(|value| value.candidate_hash.as_str()),
            checkpoint_snapshot_hash = checkpoint.as_ref().map(|value| value.snapshot_hash.as_str()),
            "recovery checkpoint selection"
        );
        // Recovery holds the pool writer locks. Immutable page replicas repaired
        // in this invocation need not be decoded and compared again for each
        // historical root that references them. Never persist this evidence.
        let mut repaired_pages = BTreeSet::new();
        let mut repaired_histories = BTreeSet::new();
        for (sequence, records) in records {
            let active = records
                .into_iter()
                .filter(|(hash, rs)| {
                    self.move_completion.as_ref().is_some_and(|selected| {
                        selected.sequence() == sequence && selected.candidate_hash() == hash
                    }) || original_content_prepare.as_ref().is_some_and(|(s,h)| *s == sequence && h == hash)
                        || rs.iter().any(|receipt| {
                        matches!(
                            receipt.record.record_type,
                            RecordType::Accept | RecordType::Publication
                        )
                    })
                })
                .collect::<Vec<_>>();
            if active.len() > 1 {
                return Err(Error::new(
                    storage_contract::ErrorCode::LogFork,
                    "different accepted candidates occupy the same slot",
                ));
            }
            let Some((hash, rs)) = active.into_iter().next() else {
                continue;
            };
            let covered_directory = checkpoint.as_ref()
                .filter(|checkpoint| sequence <= checkpoint.sequence)
                .and_then(|checkpoint| directory_checkpoints.remove(&sequence)
                    .map(|observation| (checkpoint, observation)));
            let prepared = match (candidates.remove(&sequence), covered_directory) {
                (Some(prepared), _) => prepared,
                (None, Some((checkpoint, observed))) => observed.read_covered(
                    self, &online, sequence, &hash, &rs, &self.snapshot, checkpoint,
                )?,
                (None, None) => self.read_workspace_receipt_candidate(
                    &online,
                    sequence,
                    &hash,
                    &rs,
                    workspace_receipts::ReceiptPrefix {
                        snapshot: &self.snapshot,
                        state: &self.state,
                        namespaces: self.current_namespaces.as_deref(),
                        originals: original_receipts.as_ref(),
                    },
                )?,
            };
            let candidate = prepared.candidate;
            let publication = prepared.publication;
            let mut barrier_failure = prepared.barrier_failure;
            let admitted = prepared.snapshot.map(|snapshot| snapshot.into_admitted());
            if can_repair && let Some(source) = barrier_failure.take() {
                self.recovery_warnings
                    .seals
                    .push(super::RecoverySealFailure {
                        context: super::RecoverySealContext::Publication {
                            candidate_hash: hash.clone(),
                        },
                        source,
                    });
            }
            if publication.as_ref().is_some_and(|published| {
                published.result.kind == storage_contract::file_relocation::OBJECT_MOVE_KIND
            }) && !self.move_completion.as_ref().is_some_and(|selected| {
                selected.sequence() == candidate.sequence && selected.candidate_hash() == hash
            }) {
                super::move_execution::CompletionBinding::require_completed(
                    self, &candidate, &hash,
                )?;
            }
            if let Some(checkpoint) = checkpoint
                .as_ref()
                .filter(|c| candidate.sequence <= c.sequence)
            {
                let publication = publication.as_ref().ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::CheckpointInvalid,
                        "covered publication is absent",
                    )
                })?;
                // Recheck the publication barrier independently of the checkpoint.
                // Coverage established that all historical repair artifacts remain
                // byte-for-byte present after the previous successful full replay.
                for d in &online {
                    let seal: PublicationSeal = self
                        .disk_resources
                        .get(d)?
                        .root
                        .read_json_observed(&publication_seal_path(d, &hash), &mut |_| Ok(()))?;
                    verify_publication_seal(&candidate, publication, &seal)?;
                }
                let admitted = if candidate.sequence == checkpoint.sequence {
                    if hash != checkpoint.candidate_hash
                        || candidate.snapshot_hash != checkpoint.snapshot_hash
                    {
                        return Err(Error::new(
                            storage_contract::ErrorCode::CheckpointInvalid,
                            "checkpoint boundary differs from journal",
                        ));
                    }
                    Some(match admitted {
                        Some(admitted) => admitted,
                        None => verified_recovery_snapshot_at(
                            &self.disk_resources,
                            &online,
                            &candidate.snapshot_hash,
                        )?,
                    })
                } else {
                    None
                };
                // Checkpoint coverage never substitutes for candidate-bound graph
                // verification. At the boundary, retain the original State proof
                // for history-set and projected-manifest comparisons as well.
                let original = admitted
                    .as_ref()
                    .map(|snapshot| snapshot.for_candidate(&candidate))
                    .transpose()?;
                let (_, roots) = crate::history_package::bound_history_objects_with_roots_at(
                    &self.disk_resources,
                    &online,
                    &candidate,
                    original,
                )?;
                if let Some(admitted) = admitted {
                    if original_content_prepare.as_ref().is_some_and(|(s,h)| *s == sequence && h == &hash) {
                    let observed = content.as_ref().ok_or(Error::OriginalReceipt(crate::OriginalReceiptError::Candidate))?;
                    observed.require_context(&self.planning_context(), &candidate)
                        .map_err(crate::business::file_draft::worktree::failure)?;
                    // The ordinary decision still needs each original PREPARE.
                    // Repair the same hash-bound Candidate, never a newly
                    // planned slot or partially reconstructed latest State.
                    for member in &members {
                        self.append_recovered_journal(member, RecordType::Prepare, sequence, &hash, &candidate.snapshot_hash)?;
                    }
                }
                let recovered_result = publication
                        .recovery
                        .as_ref()
                        .map(|_| publication.result.clone());
                    self.install_recovered_namespace_state(
                        admitted.into_visible_state(candidate.operation_id, recovered_result),
                        candidate.snapshot_hash.clone(),
                        &original_receipts,
                    )?;
                    self.bind_recovered_history_roots(roots);
                }
                self.snapshot = Some(candidate.snapshot_hash.clone());
                continue;
            }
            let admitted = match admitted {
                Some(admitted) => admitted,
                None => verified_recovery_snapshot_at(
                    &self.disk_resources,
                    &online,
                    &candidate.snapshot_hash,
                )?,
            };
            let original = admitted.for_candidate(&candidate)?;
            let (history_objects, roots) =
                crate::history_package::bound_history_objects_with_roots_at(
                    &self.disk_resources,
                    &online,
                    &candidate,
                    Some(original),
                )?;
            let next = original.state();
            // Prove the authenticated transition before repairing any of this
            // candidate's replicas, payloads, names or publication receipts.
            // advance is pure: install the new view only after completion below.
            // An already Published read-only prefix may outlive an upstream
            // device. Authenticate the unchanged complete barrier before using
            // its logical registration, not merely a Publication object/frame.
            // Pending and can_repair paths retain their original source gates.
            let readonly_seal = if !can_repair && let Some(p) = &publication {
                if p.candidate_hash != hash || p.result.operation_id != candidate.operation_id {
                    return Err(Error::new(
                        ErrorCode::PublicationInvalid,
                        "publication differs from candidate",
                    ));
                }
                let barrier = match barrier_failure.take() {
                    Some(source) => Err(source),
                    None => verified_publication_seal_at(
                        &self.disk_resources, &online, &candidate, p,
                    ),
                };
                match barrier {
                    Ok(seal) => Some(seal),
                    Err(source) => {
                        self.recovery_warnings.seals.push(super::RecoverySealFailure {
                            context: super::RecoverySealContext::Publication {
                                candidate_hash: hash,
                            },
                            source,
                        });
                        self.blocked = true;
                        break;
                    }
                }
            } else {
                None
            };
            let directory_kind = next.operations.get(&candidate.operation_id)
                .is_some_and(|result| {
                    result.kind == crate::persist::directory_draft_commit::native::KIND
                });
            let content = if next.operations.get(&candidate.operation_id)
                .is_some_and(|r| r.kind == crate::persist::snapshot_worktree::CONTENT_PUBLISH) {
                let cut = self.original_content_cut(&candidate, &hash, next, publication.as_ref(), &online, &rs)?;
                Some(crate::snapshot_worktree::ObservedOriginalContent::original(self, &candidate, &admitted, cut)
                    .map_err(crate::business::file_draft::worktree::failure)?)
            } else { None };
            if !directory_kind {
                // Native directory originals were fully admitted by the scanner;
                // apply re-observes their exact material below, before repairs.
                if self.current_namespaces.is_none()
                    && self.is_backup_first_candidate(&candidate)?
                {
                    // A cold selected Backup has the completed native first
                    // barrier, but Current cannot be installed before this exact
                    // first publication is replayed. Do not ask ordinary Current to
                    // authorize the operation that establishes it.
                    let generation = self.config.generation()
                        .ok_or_else(super::disk_registry::current::pending_publication)?;
                    if self.snapshot != generation.snapshot()
                        || self.state.as_ref() != generation.base.as_ref()
                    {
                        return Err(super::disk_registry::current::pending_publication());
                    }
                    generation.verify_activation_candidate(&candidate)?;
                    let published = publication.as_ref().ok_or_else(|| {
                        Error::new(ErrorCode::RecoveryRequired,
                            "selected Backup first replay lacks its verified publication")
                    })?;
                    let seal = match readonly_seal {
                        Some(seal) => seal,
                        None => verified_publication_seal_at(
                            &self.disk_resources, &online, &candidate, published,
                        )?,
                    };
                    self.preflight_backup_first_current(&candidate, original, published, &seal)?;
                } else {
                    let locations = super::fresh_layout::containers::recovery_rename_locations(
                        &self.state, next, &candidate, &container_locations,
                    );
                    self.verify_replay_namespace_at_locations(
                        &candidate, original, publication.as_ref(),
                        readonly_seal.as_ref(), &locations,
                    )?;
                }
            }
            if self.move_completion.as_ref().is_some_and(|selected| {
                selected.sequence() == candidate.sequence && selected.candidate_hash() == hash
            }) && next
                .operations
                .get(&candidate.operation_id)
                .is_some_and(|receipt| {
                    receipt.kind == storage_contract::file_relocation::OBJECT_MOVE_KIND
                })
            {
                if !members_ready || mode == RecoveryMode::PreservePending {
                    self.blocked = true;
                    break;
                }
                self.recover_selected_object_move(candidate, hash, next, publication)?;
                for disk in &online {
                    let history = scanned_journals
                        .get_mut(&disk.disk_id)
                        .expect("invariant: scanned original move member");
                    retain_replayed_records(history, self.journals[&disk.disk_id].records());
                }
                continue;
            }
            if self.move_completion.as_ref().is_some_and(|selected| {
                selected.sequence() == candidate.sequence && selected.candidate_hash() == hash
            }) && !self
                .history_move_choice
                .as_ref()
                .is_some_and(|choice| choice.decision_slot() == candidate.sequence)
            {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "retained file move lost its original business selection; preserve the candidate and both ends",
                ));
            }
            if publication.is_none() && mode == RecoveryMode::PreservePending {
                self.blocked = true;
                break;
            }
            if self
                .history_move_choice
                .as_ref()
                .is_some_and(|choice| choice.decision_slot() == candidate.sequence)
            {
                if !members_ready {
                    self.blocked = true;
                    break;
                }
                self.recover_selected_move(candidate, hash, next, publication)?;
                for disk in &online {
                    let history = scanned_journals
                        .get_mut(&disk.disk_id)
                        .expect("scanned online member");
                    retain_replayed_records(history, self.journals[&disk.disk_id].records());
                }
                continue;
            }
            // PreservePending and the independently selected move consumers
            // above do not borrow ordinary future-body/install authority.
            let mut pending_containers = None;
            let mut directory_payloads = None;
            if publication.is_some() && !directory_kind {
                crate::engine::fresh_layout::containers::verify_historical_available(
                    &self.config, &self.disk_resources, next, &container_locations,
                )?;
            }
            if publication.is_none() && mode == RecoveryMode::CompletePending && can_repair {
                let result = next
                    .operations
                    .get(&candidate.operation_id)
                    .ok_or_else(|| {
                        Error::new(
                            storage_contract::ErrorCode::StateInvalid,
                            "candidate result is absent",
                        )
                    })?;
                crate::metadata_budget::preflight_bound_publication(
                    self,
                    &candidate,
                    result,
                    next,
                    None,
                    MAX_METADATA,
                )?;
                // Metadata admission remains before BODY observation. Only this
                // exact original preparation may reach the decided installer;
                // there is no matching-byte search or unbound repair fallback.
                if !directory_kind {
                    pending_containers = Some(self.observe_ordinary_pending(
                        &candidate, &admitted, &container_locations,
                    )?);
                }
            }
            if directory_kind {
                // The same already-decided node and FILEs must be present.
                // This read is before snapshot repair, journal append, FILE
                // image update or names installation, including Published replay.
                directory_payloads = Some(self.observe_directory_pending(
                    &candidate, &admitted, MAX_METADATA,
                )?);
            }
            if can_repair {
                // Repair only replicas of the exact hash-bound snapshot. Never
                // manufacture a replacement state from an unverifiable copy.
                repair_snapshot_copies(
                    &self.disk_resources,
                    &online,
                    &members,
                    &candidate.snapshot_hash,
                    &mut repaired_pages,
                )?;
                // The original hash-bound root and all of its pages are now
                // installed on every member. Re-encoding State here duplicates
                // that work and can create unrelated roots when the writer's
                // page partitioning has changed since this snapshot was saved.
                crate::history_package::repair_history_objects_at(
                    &self.disk_resources,
                    &members,
                    &history_objects,
                    &mut repaired_histories,
                )?;
                repair_object_copies_at(
                    &self.disk_resources,
                    &members,
                    "candidate",
                    &hash,
                    &candidate,
                )?;
                if let Some(publication) = &publication {
                    repair_object_copies_at(
                        &self.disk_resources,
                        &members,
                        "publication",
                        &object_hash("publication", publication)?,
                        publication,
                    )?;
                }
                let recovered_result = publication
                    .as_ref()
                    .filter(|p| p.recovery.is_some())
                    .map(|p| p.result.clone());
                let completed = if let Some(publication) = publication {
                    self.complete_historical_candidate(
                        &candidate,
                        &hash,
                        next,
                        publication,
                        &container_locations,
                        |engine, d, pub_hash| {
                            if let Some(receipt) = rs.iter().find(|receipt| {
                                receipt.disk_id == d.disk_id
                                    && receipt.record.record_type == RecordType::Publication
                                    && receipt.record.sequence == candidate.sequence
                                    && receipt.record.object_hash == pub_hash
                            }) {
                                Ok(receipt.clone())
                            } else {
                                match mode {
                                    RecoveryMode::CompletePending => engine
                                        .append_recovered_journal(
                                            d,
                                            RecordType::Publication,
                                            candidate.sequence,
                                            &hash,
                                            pub_hash,
                                        ),
                                    RecoveryMode::PreservePending => engine.append_journal(
                                        d,
                                        RecordType::Publication,
                                        candidate.sequence,
                                        &hash,
                                        pub_hash,
                                    ),
                                }
                            }
                        },
                    )?
                } else if let Some(observed) = directory_payloads.take() {
                    self.complete_observed_directory_candidate(&candidate, &hash, next, observed)?
                } else {
                    match pending_containers {
                        Some(observed) => self.complete_observed_with_original_content(
                            &candidate, &hash, next, observed, content.as_ref(),
                        )?,
                        None => return Err(
                            super::fresh_layout::FreshLayoutError::ContainerScope.into(),
                        ),
                    }
                };
                if let Some(observed) = &directory_payloads {
                    observed.require_candidate(&candidate)?;
                }
                drop(directory_payloads);
                if let Some(held) = &content {
                    held.require_context(&self.planning_context(), &candidate)
                        .and_then(|_| held.require_installed()).map_err(crate::business::file_draft::worktree::failure)?;
                }
                // Retain the exact original already admitted before repairs.
                // The returned completion is the actual member/barrier tail,
                // not a presumed success inferred from its visible State.
                let original = admitted.into_shared();
                let mut visible = original.share_state().as_ref().clone();
                if let Some(result) = recovered_result {
                    visible.operations.insert(candidate.operation_id, result);
                }
                completed.retain_original(
                    self, &candidate, &original, &visible,
                    std::sync::Arc::make_mut(&mut original_receipts),
                )?;
                self.install_recovered_namespace_state(
                    visible, candidate.snapshot_hash.clone(), &original_receipts,
                )?;
                self.bind_recovered_history_roots(roots);
                self.snapshot = Some(candidate.snapshot_hash.clone());
            } else if let Some(p) = publication {
                if let Some(observed) = &directory_payloads {
                    observed.require_candidate(&candidate)?;
                }
                drop(directory_payloads);
                if let Some(held) = &content {
                    held.require_context(&self.planning_context(), &candidate)
                        .and_then(|_| held.require_installed()).map_err(crate::business::file_draft::worktree::failure)?;
                }
                // This is the same no-repair branch whose complete actual
                // barrier and exact Candidate were required above. A failed
                // observation breaks the replay loop before visible installation.
                let recovered_result = p.recovery.as_ref().map(|_| p.result.clone());
                self.install_recovered_namespace_state(
                    admitted.into_visible_state(candidate.operation_id, recovered_result),
                    candidate.snapshot_hash.clone(),
                    &original_receipts,
                )?;
                self.bind_recovered_history_roots(roots);
                self.snapshot = Some(candidate.snapshot_hash.clone());
                self.blocked |= !members_ready;
            } else {
                self.blocked = true;
                break;
            }
            // Preserve every receipt written during this replay before the tail advances to another slot.
            for disk in &online {
                let history = scanned_journals
                    .get_mut(&disk.disk_id)
                    .expect("scanned online member");
                retain_replayed_records(history, self.journals[&disk.disk_id].records());
            }
        }
        if !members_ready {
            self.blocked = true;
        }
        if self.history_move_choice.is_some() {
            self.restore_selected_move_slot()?;
            if torn_tail && mode == RecoveryMode::CompletePending && members_ready && !self.blocked
            {
                self.repair_selected_move_prepare_tail()?;
            } else if torn_tail {
                self.blocked = true;
            }
            // A selected PREPARE/preserved ACCEPT keeps the original old State.
            // Apart from the proven original torn tail above, no ordinary layout,
            // purge or checkpoint writes may release the selection.
            self.require_move_choice_baseline()?;
            self.restore_namespace_workspace_baselines()?;
            return Ok(());
        }
        if self.history_move_intent.is_some() {
            self.restore_original_move_intent_slot()?;
            if torn_tail {
                self.blocked = true;
            }
            // An envelope is a deny fence, not repair authority. Preserve all
            // original material until an authorized retry resumes that intent.
            self.restore_namespace_workspace_baselines()?;
            return Ok(());
        }
        if self.move_is_retained() {
            // A completion-only selection holds the same ordinary repair fence.
            self.restore_namespace_workspace_baselines()?;
            return Ok(());
        }
        if mode == RecoveryMode::CompletePending && !self.blocked {
            self.repair_journal_tails()?;
        } else if self.journals.values().any(JournalWriter::needs_recovery) {
            self.blocked = true;
        }
        // These are real disk writes, not the read-only receipt projection.
        // An admitted published prefix does not grant repair authority while
        // membership is fenced, a voter is missing, or a tail remains blocked.
        if can_repair && !self.blocked {
            crate::protection::repair_current_backup_directories(
                &self.config,
                &self.disk_resources,
                &self.state,
            )?;
        }
        self.restore_namespace_workspace_baselines()?;
        if fenced {
            return Ok(());
        }
        if mode == RecoveryMode::CompletePending && !self.blocked {
            // Names are derived from the final admitted State once. Replaying
            // intermediate publications must not undo later shared placements.
            crate::container_storage::prepare_layout_repair(
                &self.config,
                &self.disk_resources,
                &self.state,
                None,
                crate::database_install::InstallAdmission::DecidedRecovery,
            )?
            .apply()?;
        }
        self.cleanup_purged()?;
        // Optional acceleration only. Failure to create a checkpoint must never
        // fail or roll back an otherwise verified recovery. Existing checkpoints
        // become ineligible whenever their journal/coverage hashes differ.
        if !exit_generation
            && checkpoint
                .as_ref()
                .is_none_or(|c| c.sequence < self.state.sequence)
            && let Err(error) = super::recovery_checkpoint::save(self, &scanned_journals)
        {
            self.recovery_warnings.checkpoint_save = Some(error);
        }
        Ok(())
    }
}

fn retain_replayed_records(history: &mut Vec<(Record, String)>, tail: &[(Record, String)]) {
    let Some((first, _)) = tail.first() else {
        return;
    };
    let start = history.partition_point(|(record, _)| record.sequence < first.sequence);
    for (record, hash) in tail {
        if !history[start..].iter().any(|(_, old)| old == hash) {
            history.push((record.clone(), hash.clone()));
        }
    }
}

// Optimization only: bound hash storage independently of retained history.
// Once full, uncached pages still receive the complete verification/repair.
const MAX_REPAIRED_PAGES: usize = 16_384;
fn remember_repaired_page(repaired: &mut BTreeSet<String>, hash: &str) {
    if repaired.len() < MAX_REPAIRED_PAGES {
        repaired.insert(hash.to_owned());
    }
}

fn repair_snapshot_copies(
    resources: &DiskResources,
    sources: &[DiskConfig],
    members: &[DiskConfig],
    hash: &str,
    repaired_pages: &mut BTreeSet<String>,
) -> Result<()> {
    crate::publication_evidence::visit_snapshot_objects_at(resources, sources, hash, |object| {
        // Domain participates in the key: a digest is not an object kind.
        let address = format!("{}/{}", object.kind(), object.hash());
        let cacheable = matches!(object.kind(), "state-page" | "metadata-index");
        if !cacheable || !repaired_pages.contains(&address) {
            crate::publication_evidence::repair_authenticated_object_at(
                resources, members, &object,
            )?;
            if cacheable {
                remember_repaired_page(repaired_pages, &address);
                #[cfg(test)]
                PAGE_REPAIR_COUNT.with(|count| count.set(count.get() + 1));
            }
        }
        Ok(())
    })
}

#[cfg(test)]
thread_local! {
    static PAGE_REPAIR_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/recovery/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/recovery/diagnostics_tests.rs"]
mod diagnostics_tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/recovery/snapshot_proof_tests.rs"]
mod snapshot_proof_tests;
