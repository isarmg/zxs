//! Replay the verified publication prefix and reconcile pending decisions.
use super::Engine;
use crate::journal::RecordType;
use crate::publication_evidence::{
    repair_object_copies_at, verified_object_at, verified_publication_seal_at, verified_snapshot_at,
};
use crate::{disk::*, journal::*, *};
use std::collections::{BTreeMap, BTreeSet};

mod workspace_receipts;

/// Read-only journal admission, retained with its held descriptors until apply.
pub(super) struct PreparedRecovery {
    members: Vec<DiskConfig>,
    online: Vec<DiskConfig>,
    prefixes: BTreeMap<String, BTreeSet<String>>,
    journals: BTreeMap<String, OpenJournal>,
    records: super::recovery_checkpoint::Records,
    torn_tail: bool,
    candidates: BTreeMap<u64, workspace_receipts::PreparedCandidate>,
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
        self.metadata_integrity.invalidate();
        self.published_state_base = None;
        self.recovery_warnings = super::RecoveryWarnings::default();
        let members = self.members();
        let mut online = Vec::new();
        let mut prefixes = BTreeMap::<String, BTreeSet<String>>::new();
        let mut opened_journals = BTreeMap::new();
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
            let opened = OpenJournal::scan_at(&handle.root, d)?;
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
            return Err(Error::new(
                storage_contract::ErrorCode::MetadataQuorumUnavailable,
                "online service requires a majority of the configured metadata members; use offline recovery",
            ));
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
        Ok(PreparedRecovery {
            members,
            online,
            prefixes,
            journals: opened_journals,
            records,
            torn_tail,
            candidates: BTreeMap::new(),
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
        let PreparedRecovery {
            members,
            online,
            prefixes,
            journals: mut opened_journals,
            records,
            torn_tail,
            mut candidates,
        } = prepared;
        let mut scanned_journals = BTreeMap::new();
        for disk in &online {
            let opened = opened_journals
                .remove(&disk.disk_id)
                .expect("scanned online journal");
            scanned_journals.insert(disk.disk_id.clone(), opened.scan.records.clone());
            self.journals
                .insert(disk.disk_id.clone(), opened.into_writer()?);
        }
        let checkpoint = if online.len() == members.len() && !torn_tail {
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
                .filter(|(_, rs)| {
                    rs.iter().any(|receipt| {
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
            let prepared = match candidates.remove(&sequence) {
                Some(prepared) => prepared,
                None => self.read_workspace_receipt_candidate(
                    &online,
                    sequence,
                    &hash,
                    &rs,
                    &self.snapshot,
                )?,
            };
            let candidate = prepared.candidate;
            let publication = prepared.publication;
            let mut barrier_failure = prepared.barrier_failure;
            let admitted = prepared.snapshot.map(|snapshot| snapshot.into_admitted());
            if online.len() == members.len()
                && let Some(source) = barrier_failure.take()
            {
                self.recovery_warnings
                    .seals
                    .push(super::RecoverySealFailure {
                        context: super::RecoverySealContext::Publication {
                            candidate_hash: hash.clone(),
                        },
                        source,
                    });
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
                        None => verified_snapshot_at(
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
                crate::history_package::bound_history_objects_at(
                    &self.disk_resources,
                    &online,
                    &candidate,
                    admitted.as_ref(),
                )?;
                if let Some(admitted) = admitted {
                    let mut next = admitted.into_state();
                    if publication.recovery.is_some() {
                        next.operations
                            .insert(candidate.operation_id, publication.result.clone());
                    }
                    self.state = std::sync::Arc::new(next);
                }
                self.snapshot = Some(candidate.snapshot_hash);
                continue;
            }
            let admitted = match admitted {
                Some(admitted) => admitted,
                None => {
                    verified_snapshot_at(&self.disk_resources, &online, &candidate.snapshot_hash)?
                }
            };
            let history_objects = crate::history_package::bound_history_objects_at(
                &self.disk_resources,
                &online,
                &candidate,
                Some(&admitted),
            )?;
            let mut next = admitted.into_state();
            if publication.is_none() && mode == RecoveryMode::PreservePending {
                self.blocked = true;
                break;
            }
            if publication.is_none()
                && mode == RecoveryMode::CompletePending
                && online.len() == members.len()
            {
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
                    &next,
                    None,
                    MAX_METADATA,
                )?;
                self.repair_pending_payloads(&candidate)?;
            }
            if online.len() == members.len() {
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
                if let Some(publication) = publication {
                    self.complete_published_candidate(
                        &candidate,
                        &hash,
                        &next,
                        publication,
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
                    )?;
                } else {
                    self.complete_candidate(&candidate, &hash, &next, None)?;
                }
                if let Some(result) = recovered_result {
                    next.operations.insert(candidate.operation_id, result);
                }
                self.state = std::sync::Arc::new(next);
                self.snapshot = Some(candidate.snapshot_hash);
            } else if let Some(p) = publication {
                if p.candidate_hash != hash || p.result.operation_id != candidate.operation_id {
                    return Err(Error::new(
                        storage_contract::ErrorCode::PublicationInvalid,
                        "publication differs from candidate",
                    ));
                }
                // A PUBLICATION object or a single frame is not the complete
                // barrier. Preserve the old visible state until all configured
                // synchronized receipts can be checked after restart.
                let barrier = match barrier_failure {
                    Some(source) => Err(source),
                    None => {
                        verified_publication_seal_at(&self.disk_resources, &online, &candidate, &p)
                    }
                };
                if let Err(source) = barrier {
                    self.recovery_warnings
                        .seals
                        .push(super::RecoverySealFailure {
                            context: super::RecoverySealContext::Publication {
                                candidate_hash: hash,
                            },
                            source,
                        });
                    self.blocked = true;
                    break;
                }
                if p.recovery.is_some() {
                    next.operations
                        .insert(candidate.operation_id, p.result.clone());
                }
                self.state = std::sync::Arc::new(next);
                self.snapshot = Some(candidate.snapshot_hash);
                self.blocked = true;
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
        if online.len() != members.len() {
            self.blocked = true;
        }
        if mode == RecoveryMode::CompletePending && !self.blocked {
            self.repair_journal_tails()?;
        } else if self.journals.values().any(JournalWriter::needs_recovery) {
            self.blocked = true;
        }
        self.cleanup_purged()?;
        // Optional acceleration only. Failure to create a checkpoint must never
        // fail or roll back an otherwise verified recovery. Existing checkpoints
        // become ineligible whenever their journal/coverage hashes differ.
        if checkpoint
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
    let root = crate::publication_evidence::verified_graph_object_at(
        resources, sources, "snapshot", hash,
    )?;
    let snapshot: Snapshot = json_codec::from_value(root.value().clone())?;
    validate_snapshot_schema(&snapshot)?;
    for reference in snapshot
        .maps
        .values()
        .chain(std::iter::once(&snapshot.events))
    {
        crate::metadata_map::visit_objects(
            reference,
            |kind, hash| {
                crate::publication_evidence::verified_graph_object_at(
                    resources, sources, kind, hash,
                )
            },
            |object| {
                // Domain participates in the key: a digest is not an object kind.
                let address = format!("{}/{}", object.kind(), object.hash());
                if !repaired_pages.contains(&address) {
                    crate::publication_evidence::repair_authenticated_object_at(
                        resources, members, &object,
                    )?;
                    remember_repaired_page(repaired_pages, &address);
                    #[cfg(test)]
                    PAGE_REPAIR_COUNT.with(|count| count.set(count.get() + 1));
                }
                Ok(())
            },
        )?;
    }
    let auth = crate::publication_evidence::verified_graph_object_at(
        resources,
        sources,
        "state-control-auth",
        snapshot.control_auth.hash(),
    )?;
    crate::publication_evidence::repair_authenticated_object_at(resources, members, &auth)?;
    crate::publication_evidence::repair_authenticated_object_at(resources, members, &root)
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
