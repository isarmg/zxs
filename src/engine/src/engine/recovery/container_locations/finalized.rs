//! A finalized tail is recovery-only. Merely finding ACCEPT, a local rename
//! intent, or a serialized Finalize object cannot select its new physical name.
use super::*;
use crate::engine::Engine;
use crate::journal::{Finalize, Receipt, RecordType, receipt_binds};
use crate::publication_evidence::verified_object_at;
use crate::{DiskConfig, object_hash};
use std::collections::BTreeSet;

pub(super) struct PendingName {
    pub(super) before: FileContainerRecord,
    pub(super) after: ContainerNamespace,
    pub(super) sequence: u64,
    pub(super) published: bool,
}

impl ReplayContainerLocations {
    /// `receipts` are from this invocation's fully scanned, held member journals.
    /// The caller already checked the candidate's exact predecessor snapshot.
    /// Missing FINALIZE is not permission to infer a completed physical move.
    pub(in crate::engine::recovery) fn admit_finalized(
        &mut self,
        engine: &Engine,
        online: &[DiskConfig],
        receipts: &[Receipt],
        previous: &State,
        original: &AdmittedSnapshot,
        candidate: &Candidate,
        publication: Option<&Publication>,
    ) -> Result<()> {
        let next = original.for_candidate(candidate)?.state();
        if !has_name_change(previous, next) {
            return Ok(());
        }
        let transition = containers::verify_transition(&engine.config, previous, next, candidate)?;
        let changes = transition.renames();
        if changes.is_empty() {
            return Ok(());
        }
        if !self.pending.is_empty() || candidate.generation().is_some()
            || !candidate.versions.is_empty()
        {
            return Err(Failure::ContainerScope.into());
        }
        let finalized = receipts.iter().filter(|receipt| {
            receipt.record.record_type == RecordType::Finalize
        }).collect::<Vec<_>>();
        let members = candidate.metadata_disk_ids.iter().collect::<BTreeSet<_>>();
        if members.len() != candidate.metadata_disk_ids.len() || members.is_empty() {
            return Err(Failure::ContainerScope.into());
        }
        if finalized.len() < members.len() {
            return Ok(());
        }
        if finalized.len() != members.len()
            || finalized.iter().map(|receipt| &receipt.disk_id).collect::<BTreeSet<_>>() != members
        {
            return Err(Failure::ContainerScope.into());
        }
        let first = finalized.first().ok_or(Failure::ContainerScope)?;
        let hash = &first.record.object_hash;
        if finalized.iter().any(|receipt| &receipt.record.object_hash != hash) {
            return Err(Failure::ContainerScope.into());
        }
        let finalize: Finalize = verified_object_at(
            &engine.disk_resources, online, "finalize", hash,
        )?;
        let candidate_hash = object_hash("candidate", candidate)?;
        if finalize.candidate_hash != candidate_hash || finalize.recovery_id.is_some()
            || finalize.accepts.len() != members.len()
            || finalize.accepts.iter().map(|receipt| &receipt.disk_id)
                .collect::<BTreeSet<_>>() != members
        {
            return Err(Failure::ContainerScope.into());
        }
        let mut slots = BTreeSet::new();
        for final_receipt in &finalized {
            let disk = online.iter().find(|disk| disk.disk_id == final_receipt.disk_id)
                .ok_or(Failure::ContainerScope)?;
            let accept = finalize.accepts.iter()
                .find(|accept| accept.disk_id == final_receipt.disk_id)
                .ok_or(Failure::ContainerScope)?;
            if !slots.insert(&final_receipt.slot_id)
                || final_receipt.slot_id != disk.slot_id
                || accept.slot_id != disk.slot_id
                || !receipts.iter().any(|actual| actual == accept)
                || final_receipt.record.previous_frame_hash != accept.frame_hash
                || !receipt_binds(accept, RecordType::Accept, candidate.sequence,
                    &candidate_hash, &candidate_hash)?
                || !receipt_binds(final_receipt, RecordType::Finalize, candidate.sequence,
                    &candidate_hash, hash)?
            {
                return Err(Failure::ContainerScope.into());
            }
        }
        if let Some(publication) = publication {
            crate::journal::verify_finalized_publication(candidate, publication)?;
            let result = next.operations.get(&candidate.operation_id)
                .ok_or(Failure::ContainerScope)?;
            if publication.recovery.is_some() || &publication.result != result
                || publication.candidate_hash != candidate_hash
                || publication.finalize != finalize
                || publication.finalize_receipts.len() != finalized.len()
                || publication.finalize_receipts.iter()
                    .any(|receipt| !finalized.iter().any(|actual| *actual == receipt))
            {
                return Err(Failure::ContainerScope.into());
            }
        }
        for (file, change) in changes {
            let before = previous.file_containers.get(file).ok_or(Failure::ContainerScope)?;
            for disk in before.replicas.keys() {
                // A published displacement must already end at this tail's
                // original name; a missing link cannot be patched by the tail.
                if self.historical_name(previous, *file, *disk)?
                    .is_some_and(|name| name != &before.namespace.name)
                {
                    return Err(Failure::ContainerScope.into());
                }
            }
            self.pending.insert(*file, PendingName {
                before: before.clone(),
                after: change.target().clone(),
                sequence: candidate.sequence,
                published: publication.is_some(),
            });
        }
        Ok(())
    }
}
