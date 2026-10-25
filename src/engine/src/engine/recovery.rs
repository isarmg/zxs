//! Replay the verified publication prefix and reconcile pending decisions.
use super::Engine;
use crate::publication_evidence::{
    repair_object_copies, verified_object, verified_publication_seal, verified_snapshot,
};
use crate::{disk::*, journal::*, *};
use std::collections::{BTreeMap, BTreeSet};

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
            let directory = disk.root.join("metadata/publication-seals");
            match std::fs::symlink_metadata(&directory) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
                Ok(_) => {
                    existing_private_dir(&directory)?;
                }
            }
            for entry in std::fs::read_dir(&directory)? {
                let entry = entry?;
                let name = entry.file_name();
                let Some(hash) = name.to_str().and_then(|name| name.strip_suffix(".json")) else {
                    // An unfinished atomic-write temporary is not a seal.
                    continue;
                };
                if journaled.contains(hash) {
                    continue;
                }
                let seal: PublicationSeal = read_json(&entry.path())?;
                let candidate: Candidate = verified_object(online, "candidate", hash)?;
                self.verify_candidate_membership(&candidate)?;
                let publication: Publication =
                    verified_object(online, "publication", &seal.publication_hash)?;
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
        verified_snapshot(&self.members(), hash)
    }

    /// Preserve the candidate's original snapshot layout when completing a
    /// persisted decision, including decisions made by an older page writer.
    pub(crate) fn repair_bound_snapshot(&self, hash: &str) -> Result<()> {
        let members = self.members();
        verified_snapshot(&members, hash)?;
        repair_snapshot_copies(&members, &members, hash, &mut BTreeSet::new())
    }

    pub(super) fn recover_mode(&mut self, complete_unpublished: bool) -> Result<()> {
        let members = self.members();
        let mut online = Vec::new();
        let mut prefixes = BTreeMap::<String, BTreeSet<String>>::new();
        let mut journal_stamps = BTreeMap::new();
        let mut torn_tail = false;
        let mut records = BTreeMap::<u64, BTreeMap<String, Vec<(String, Record)>>>::new();
        for d in &members {
            if validate_seal(&self.config, d).is_err() {
                continue;
            }
            let scanned = scan(d)?;
            torn_tail |= scanned.torn_tail;
            if let Some(stamp) = scanned.stamp {
                journal_stamps.insert(d.disk_id.clone(), stamp);
            }
            prefixes.insert(
                d.disk_id.clone(),
                scanned
                    .records
                    .iter()
                    .map(|(_, hash)| hash.clone())
                    .collect(),
            );
            for (r, _) in scanned.records {
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
                    .push((d.slot_id.clone(), r));
            }
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
                        records.iter().any(|(_, record)| {
                            matches!(record.record_type.as_str(), "ACCEPT" | "PUBLICATION")
                        })
                    })
                    .map(|(hash, _)| hash.as_str())
            })
            .collect();
        self.reject_unjournaled_publications(&online, &journaled)?;
        let checkpoint = if online.len() == members.len() && !torn_tail {
            super::recovery_checkpoint::load(self, &prefixes, &records).filter(|c| {
                records
                    .get(&c.sequence)
                    .is_some_and(|candidates| candidates.contains_key(&c.candidate_hash))
            })
        } else {
            None
        };
        // Recovery holds the pool writer locks. Immutable page replicas repaired
        // in this invocation need not be decoded and compared again for each
        // historical root that references them. Never persist this evidence.
        let mut repaired_pages = BTreeSet::new();
        let mut repaired_histories = BTreeSet::new();
        for (_seq, candidates) in records {
            let active = candidates
                .into_iter()
                .filter(|(_, rs)| {
                    rs.iter()
                        .any(|(_, r)| r.record_type == "ACCEPT" || r.record_type == "PUBLICATION")
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
            let candidate: Candidate = verified_object(&online, "candidate", &hash)?;
            self.verify_candidate_membership(&candidate)?;
            if candidate.pool_id != self.config.pool_id
                || candidate.previous_snapshot != self.snapshot
            {
                return Err(Error::new(
                    storage_contract::ErrorCode::LogFork,
                    "candidate does not extend the verified publication prefix",
                ));
            }
            let publication_record = rs.iter().find(|(_, r)| r.record_type == "PUBLICATION");
            let publication = if let Some((_, r)) = publication_record {
                Some(verified_object::<Publication>(
                    &online,
                    "publication",
                    &r.object_hash,
                )?)
            } else {
                crate::degraded_recovery::discover_publication(self, &candidate, &hash)?
            };
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
                    let seal: PublicationSeal = read_json(&publication_seal_path(d, &hash))?;
                    verify_publication_seal(&candidate, publication, &seal)?;
                }
                // Checkpoint coverage never substitutes for candidate-bound graph verification.
                crate::history_package::bound_history_objects(&online, &candidate, None)?;
                if candidate.sequence == checkpoint.sequence {
                    if hash != checkpoint.candidate_hash
                        || candidate.snapshot_hash != checkpoint.snapshot_hash
                    {
                        return Err(Error::new(
                            storage_contract::ErrorCode::CheckpointInvalid,
                            "checkpoint boundary differs from journal",
                        ));
                    }
                    let mut next = verified_snapshot(&online, &candidate.snapshot_hash)?;
                    if publication.recovery.is_some() {
                        next.operations
                            .insert(candidate.operation_id.clone(), publication.result.clone());
                    }
                    self.state = std::sync::Arc::new(next);
                }
                self.snapshot = Some(candidate.snapshot_hash);
                continue;
            }
            let mut next = verified_snapshot(&online, &candidate.snapshot_hash)?;
            let history_objects =
                crate::history_package::bound_history_objects(&online, &candidate, Some(&next))?;
            if publication.is_none() && !complete_unpublished {
                self.blocked = true;
                break;
            }
            if publication.is_none() && complete_unpublished && online.len() == members.len() {
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
                    &online,
                    &members,
                    &candidate.snapshot_hash,
                    &mut repaired_pages,
                )?;
                // The original hash-bound root and all of its pages are now
                // installed on every member. Re-encoding State here duplicates
                // that work and can create unrelated roots when the writer's
                // page partitioning has changed since this snapshot was saved.
                crate::history_package::repair_history_objects(
                    &members,
                    &history_objects,
                    &mut repaired_histories,
                )?;
                repair_object_copies(&members, "candidate", &hash, &candidate)?;
                if let Some(publication) = &publication {
                    repair_object_copies(
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
                        |d, pub_hash| {
                            let scanned = journal_stamps
                                .get(&d.disk_id)
                                .zip(prefixes.get(&d.disk_id))
                                .zip(rs.iter().find(|(slot, r)| {
                                    slot == &d.slot_id && r.record_type == "PUBLICATION"
                                }))
                                .map(|((stamp, hashes), (_, record))| ScannedReceipt {
                                    stamp,
                                    hashes,
                                    record,
                                });
                            replay_publication(d, candidate.sequence, &hash, pub_hash, scanned)
                        },
                    )?;
                } else {
                    self.complete_candidate(&candidate, &hash, &next, None)?;
                }
                if let Some(result) = recovered_result {
                    next.operations
                        .insert(candidate.operation_id.clone(), result);
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
                let sealed = verified_publication_seal(&online, &candidate, &p).is_ok();
                if !sealed {
                    self.blocked = true;
                    break;
                }
                if p.recovery.is_some() {
                    next.operations
                        .insert(candidate.operation_id.clone(), p.result.clone());
                }
                self.state = std::sync::Arc::new(next);
                self.snapshot = Some(candidate.snapshot_hash);
                self.blocked = true;
            } else {
                self.blocked = true;
                break;
            }
        }
        if online.len() != members.len() {
            self.blocked = true;
        }
        self.cleanup_purged()?;
        // Optional acceleration only. Failure to create a checkpoint must never
        // fail or roll back an otherwise verified recovery. Existing checkpoints
        // become ineligible whenever their journal/coverage hashes differ.
        if checkpoint
            .as_ref()
            .is_none_or(|c| c.sequence < self.state.sequence)
        {
            let _ = super::recovery_checkpoint::save(self);
        }
        Ok(())
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
    sources: &[DiskConfig],
    members: &[DiskConfig],
    hash: &str,
    repaired_pages: &mut BTreeSet<String>,
) -> Result<()> {
    fn install<T: data_serialization::Serialize + data_serialization::de::DeserializeOwned>(
        sources: &[DiskConfig],
        members: &[DiskConfig],
        kind: &str,
        hash: &str,
    ) -> Result<()> {
        let value: T = verified_object(sources, kind, hash)?;
        repair_object_copies(members, kind, hash, &value)
    }
    let snapshot: Snapshot = verified_object(sources, "snapshot", hash)?;
    for hash in snapshot.pages.values().flatten() {
        if !repaired_pages.contains(hash) {
            install::<json_codec::Map<String, json_codec::Value>>(
                sources,
                members,
                "state-page",
                hash,
            )?;
            // Record only after every replica was successfully checked/repaired.
            remember_repaired_page(repaired_pages, hash);
            #[cfg(test)]
            PAGE_REPAIR_COUNT.with(|count| count.set(count.get() + 1));
        }
    }
    install::<Snapshot>(sources, members, "snapshot", hash)
}

#[cfg(test)]
thread_local! {
    static PAGE_REPAIR_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/recovery/tests.rs"]
mod tests;
