//! Only successful ordinary publication creates an in-process continuation.
//! A decoded reserve, even with identical bytes, cannot reconstruct this token.
use crate::container_storage::namespace_host::{
    NamespacePreparationObservation, PreparedNamespaceHost, PreparedNamespaceNode,
};
use crate::engine::MutationPlan;
use crate::persist::snapshot_worktree::{
    HostEvidence, NodeAllocation, NodeCompletion, NodeReservation, PublicationEncoding, Record,
};
use crate::snapshot_worktree::{RegistrationError, Result, source};
use crate::working_tree::snapshot::{
    AdmittedSnapshotManifest, SnapshotEntryContent, SnapshotFile, SnapshotStoreError,
};
use crate::{Engine, NamespaceSnapshot, OpenedContent, State};
use std::{io::Read, sync::Arc};
use storage_contract::snapshot_worktree::{InitializeSnapshot, SnapshotHandle};
use storage_contract::working_tree::MaterialCommitment;
use storage_contract::{OperationId, Principal};

pub(crate) struct PublishedNodeReservation {
    before: Arc<State>,
    after: Arc<State>,
    plan: NodeReservation,
    principal: Principal,
    commitment: MaterialCommitment,
    observation: NamespacePreparationObservation,
    after_namespace: NamespaceSnapshot,
}

impl PublishedNodeReservation {
    pub(crate) fn require_host(
        &self,
        engine: &Engine,
        before: &Arc<State>,
        operation: OperationId,
        host: &HostEvidence,
        observation: &NamespacePreparationObservation,
    ) -> Result<()> {
        if !Arc::ptr_eq(before, &self.before) || !Arc::ptr_eq(&engine.state, &self.after)
            || operation != self.plan.request.operation_id || host != &self.plan.host
            || !self.observation.same(observation)
        {
            return Err(RegistrationError::PublicationChanged);
        }
        Ok(())
    }

    pub(crate) fn after(&self) -> &Arc<State> {
        &self.after
    }

    pub(crate) fn material(&self) -> crate::working_tree::snapshot::SnapshotReference {
        self.plan.material
    }

    pub(crate) fn after_namespace(&self) -> &NamespaceSnapshot {
        &self.after_namespace
    }

    pub(crate) fn open_payload(
        &self,
        engine: &Engine,
        file: &SnapshotFile,
    ) -> std::result::Result<OriginalPayload, SnapshotStoreError> {
        if !Arc::ptr_eq(&engine.state, &self.after)
            || !self.plan.entries.iter().any(|entry| matches!(entry.content,
                SnapshotEntryContent::File { material, .. } if material == *file))
        {
            return Err(SnapshotStoreError::Entries);
        }
        let opened = engine.open_content(&file.version_id.to_string())
            .map_err(SnapshotStoreError::Disk)?;
        if opened.size != file.size || opened.content_hash != file.content_hash {
            return Err(SnapshotStoreError::Entries);
        }
        Ok(OriginalPayload { opened })
    }
}

/// The actual BODY reader's read lease survives for the complete verified copy.
pub(crate) struct OriginalPayload {
    opened: OpenedContent,
}
impl Read for OriginalPayload {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.opened.file.read(buffer)
    }
}

pub(super) fn reserve(
    engine: &mut Engine,
    host: &PreparedNamespaceHost,
    request: &InitializeSnapshot,
    allocated: NodeAllocation,
    principal: &Principal,
) -> Result<(Arc<PublishedNodeReservation>, AdmittedSnapshotManifest)> {
    host.require_root_source(engine).map_err(RegistrationError::NamespaceHost)?;
    host.require_request(request).map_err(RegistrationError::NamespaceHost)?;
    let (address, evidence) = host.reservation_evidence(engine)
        .map_err(RegistrationError::NamespaceHost)?;
    if address != request.host {
        return Err(RegistrationError::Source);
    }
    let before = Arc::clone(&engine.state);
    let before_namespace = host.namespace_snapshot(engine)
        .map_err(RegistrationError::NamespaceHost)?.clone();
    let observation = host.observation();
    let input = PublicationEncoding::node_request(request).map_err(RegistrationError::Codec)?;
    let mut constructed = None;
    let planned = engine.plan_mutation_digest(
        crate::persist::snapshot_worktree::NODE_RESERVE, principal.as_str(),
        request.operation_id, input, None,
        |engine, next, _| {
            let manifest = source::construct(
                engine, next, &request.source, allocated.repository,
                allocated.node, request.operation_id,
            )?;
            let plan = NodeReservation {
                request: request.clone(), host: evidence, allocated,
                branch_name: storage_contract::BranchName::parse("main")?,
                entries: manifest.entries().to_vec(), material: manifest.reference(),
            };
            let result = Record::ReserveNode(plan.clone()).result().map_err(source::encoding)?;
            constructed = Some((plan, manifest));
            Ok(result)
        },
    ).map_err(RegistrationError::Publication)?;
    let (plan, manifest) = constructed.ok_or(RegistrationError::Operation(request.operation_id))?;
    let commitment = Record::ReserveNode(plan.clone()).commitment()
        .map_err(RegistrationError::Codec)?;
    publish_planned(
        engine, &before, request.operation_id, principal,
        &Record::ReserveNode(plan.clone()), planned,
    )?;
    let after_namespace = engine.namespace_snapshot(&request.source.branch_id.to_string())
        .map_err(RegistrationError::Publication)?;
    if before_namespace.entries() != after_namespace.entries() {
        return Err(RegistrationError::PublicationChanged);
    }
    Ok((Arc::new(PublishedNodeReservation {
        before, after: Arc::clone(&engine.state), plan,
        principal: principal.clone(), commitment, observation, after_namespace,
    }), manifest))
}

pub(super) fn complete(
    engine: &mut Engine,
    node: &PreparedNamespaceNode,
) -> Result<SnapshotHandle> {
    node.require_engine(engine)?;
    let reserved = node.reservation();
    let plan = &reserved.plan;
    let record = Record::CompleteNode(NodeCompletion {
        reserve: plan.request.operation_id,
        reservation: reserved.commitment,
        binding: node.binding(),
    });
    publish_new(engine, plan.allocated.completion, &reserved.principal, &record)?;
    let handle = SnapshotHandle {
        publication: plan.allocated.completion,
        repository_id: plan.allocated.repository,
        branch_id: plan.allocated.branch,
        node_id: plan.allocated.node,
    };
    // Reopen through the permanent index and actual current root, not the
    // still-held writer's success value. Failure never undoes the publication.
    engine.snapshot_receipts()?.reopen_node(engine, handle)?;
    Ok(handle)
}

pub(super) fn publish_new(
    engine: &mut Engine,
    operation: OperationId,
    principal: &Principal,
    record: &Record,
) -> Result<()> {
    let encoded = PublicationEncoding::new(record).map_err(RegistrationError::Codec)?;
    let before = Arc::clone(&engine.state);
    let planned = engine.plan_mutation_digest(
        record.kind(), principal.as_str(), operation, encoded.request(), None,
        |_, _, _| Ok(encoded.result()),
    ).map_err(RegistrationError::Publication)?;
    publish_planned(engine, &before, operation, principal, record, planned)
}

fn publish_planned(
    engine: &mut Engine,
    before: &Arc<State>,
    operation: OperationId,
    principal: &Principal,
    record: &Record,
    planned: MutationPlan,
) -> Result<()> {
    let MutationPlan::Prepared { next, prepared, result } = planned else {
        return Err(RegistrationError::Operation(operation));
    };
    if !prepared.is_empty() {
        return Err(RegistrationError::PublicationChanged);
    }
    // This closes the original typed reserve/completion relationship before a
    // decision is written, including all fresh allocations and pending hosts.
    {
        let index = engine.snapshot_receipts()?;
        index.require_planned(&next, &result, principal, record)?;
        index.require_new_mount_namespace(&next, engine.config.max_entries, record)?;
    }
    let expected = next.as_ref().clone();
    engine.persist(*next, prepared, result).map_err(RegistrationError::Publication)?;
    if !crate::publication_shape::same_transition(before, &expected, engine.state()) {
        return Err(RegistrationError::PublicationChanged);
    }
    Ok(())
}
