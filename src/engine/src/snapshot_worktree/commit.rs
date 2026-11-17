//! Explicit publication from one registered, durably sealed edit. Only an
//! independently published FILE child and directory node may finish their exact
//! ledger/SYSTEM tail on retry. Earlier uncertain preparations remain Pending.
mod recovery;
mod version;
pub(crate) use version::ReservedFileVersion;

use crate::container_storage::namespace_host::NamespaceHost;
use crate::directory_worktree::ledger::{Event, HeldWorktreeLedger, LedgerError};
use crate::directory_worktree::store::HeldSparseWorktree;
use crate::persist::snapshot_worktree::{
    CommitCompletion, CommitNode, CommitReservation, FileRequestCommitment, Record,
    CommitRouting, HistoricalCommit, HistoricalFile, HistoricalReady,
};
use crate::snapshot_worktree::registry::CommitRetry;
use crate::snapshot_worktree::{RegistrationError, Result, publication, with_worktree};
use crate::working_tree::snapshot::{
    AdmittedSnapshotManifest, HeldSnapshotNode, SnapshotFile, SnapshotFileSource,
    SnapshotStoreError,
};
use crate::{CommitRequest, Engine};
use std::{io::Read, sync::Arc};
use storage_contract::snapshot_worktree::{CommitWorktree, SnapshotHandle};
use storage_contract::working_tree::{MaterialCommitment, SnapshotBranchId, SnapshotNodeId};
use storage_contract::{OperationId, Principal, Sequence};

pub(crate) fn commit_worktree(
    engine: &mut Engine, request: &CommitWorktree, principal: &Principal,
) -> Result<SnapshotHandle> {
    commit_inner(engine, request, principal).map_err(|source| {
        if engine.state().operations.get(&request.operation_id).is_some_and(|record| {
            record.kind == crate::persist::snapshot_worktree::HISTORICAL_RESERVE
        }) && !matches!(source, RegistrationError::Pending(_) | RegistrationError::Digest(_)
            | RegistrationError::Operation(_))
        {
            // Later FILE/ready/physical failure cannot turn a durable original
            // into a fresh rejection or permission to submit another operation.
            RegistrationError::DirectoryResponse {
                operation: request.operation_id, source: Box::new(source),
            }
        } else { source }
    })
}

fn commit_inner(
    engine: &mut Engine, request: &CommitWorktree, principal: &Principal,
) -> Result<SnapshotHandle> {
    let source = engine.snapshot_receipt_source()?;
    let index = source.index()?;
    if let Some(retry) = index.retry_commit(request, principal)? {
        return match retry {
            CommitRetry::Complete(handle) => {
                index.reopen_node(engine, handle)?;
                with_worktree(engine, request.target, |_| Ok(()))?;
                Ok(handle)
            }
            CommitRetry::Published(tail) => recovery::resume(engine, &index, tail, principal),
        };
    }
    if engine.admitted_current_namespaces().map_err(RegistrationError::Publication)?.is_some()
        || engine.config.generation().is_some()
    {
        return Err(RegistrationError::InitialOnly);
    }
    let registered = index.worktree(request.target)?;
    let original = index.reopen_node(engine, registered.source().handle())?;
    let session = registered.reopen(&original)?;
    let (edited, completed) = index.completed_edit(request.target)?;
    if request.expected != session.worktree().revision()
        || session.worktree().phase() != (crate::directory_worktree::lifecycle::Phase::Active {
            saved: None,
        })
    {
        return Err(RegistrationError::CommitScope);
    }
    let SnapshotFileSource::Container { branch, node } = edited.original_source else {
        return Err(RegistrationError::CommitScope);
    };
    let file_head = engine.file_branch_head(&branch.to_string())
        .map_err(RegistrationError::Publication)?;
    let creation = registered.reservation().clone();
    NamespaceHost::registered(engine, creation.host.namespace)
        .map_err(RegistrationError::NamespaceHost)?
        .require_root_mount(creation.request.parent_directory_id,
            creation.request.expected_root_generation, &creation.request.name)
        .map_err(RegistrationError::NamespaceHost)?;
    let file = engine.state().versions.get(&edited.original.version_id.to_string())
        .ok_or(RegistrationError::CommitScope)?;
    let root = engine.namespace_host_root(
        creation.host.namespace.repository_id, creation.host.namespace.branch_id,
    ).map_err(RegistrationError::Publication)?;
    let container = engine.state().file_containers.get(&edited.original.file_id)
        .ok_or(RegistrationError::CommitScope)?;
    if container.owner_disk.to_string() != root.binding().registration.disk_id
        || container.namespace.repository != creation.host.namespace.repository_id
    {
        return Err(RegistrationError::CommitScope);
    }
    let file_history = file.history_tree_id.parse().map_err(RegistrationError::Allocation)?;
    let mut file_base = engine.state().branches[&branch.to_string()].head_commit_id.parse()
        .map_err(RegistrationError::Allocation)?;
    let source_handle = registered.source().handle();
    let binding = registered.binding();
    let before = registered.checkpoint();
    let original_file = edited.original;
    let original_source = edited.original_source;
    let edit = completed.edit.clone();
    if edit.material.content_hash == original_file.content_hash
        && edit.material.size == original_file.size
    {
        return Err(RegistrationError::CommitScope);
    }
    let instance = HeldSparseWorktree::reopen(&original, session.instance())
        .map_err(RegistrationError::EditInstance)?;
    let mut body = instance.open_material(&edit.file_edit().material)
        .map_err(RegistrationError::EditInstance)?;
    let mut bytes = Vec::new();
    let limit = edit.material.size.checked_add(1).ok_or(RegistrationError::EditBody)?;
    body.by_ref().take(limit).read_to_end(&mut bytes)
        .map_err(RegistrationError::CommitRead)?;
    if bytes.len() as u64 != edit.material.size {
        return Err(RegistrationError::EditBody);
    }
    let observed_head = index.commit_head(source_handle)?;
    let mut routing = CommitRouting {
        directory: crate::directory_worktree::lifecycle::DirectoryDestination::Continue {
            branch: source_handle.branch_id, expected_head: source_handle.node_id,
        },
        file_branch: branch,
    };
    let historical = if observed_head != source_handle || file_head != node.to_string() {
        let mount_name = storage_contract::BranchName::parse(creation.request.name.as_str())
            .map_err(RegistrationError::Allocation)?;
        let destination = if observed_head == source_handle {
            source_handle.branch_id
        } else {
            SnapshotBranchId::generate().map_err(RegistrationError::Allocation)?
        };
        let branch_name = if observed_head == source_handle {
            registered.source().branch_name().clone()
        } else { mount_name.clone() };
        let file = if file_head == node.to_string() {
            HistoricalFile::Continue { branch, base: file_base }
        } else {
            if engine.state().branches.values().any(|candidate| {
                candidate.file_id.as_deref() == Some(original_file.file_id.to_string().as_str())
                    && name_policy::lookup_key(&candidate.name) == mount_name.key()
            }) {
                return Err(RegistrationError::Mount);
            }
            HistoricalFile::Fork {
                operation: OperationId::generate().map_err(RegistrationError::Allocation)?,
                name: mount_name,
            }
        };
        let intent = HistoricalCommit {
            request: request.clone(), source: source_handle, observed_head, destination,
            branch_name, binding, before, edit: edit.clone(), original: original_file,
            original_source, file_history,
            file_observed_head: file_head.parse().map_err(RegistrationError::Allocation)?, file,
            ready: OperationId::generate().map_err(RegistrationError::Allocation)?,
            file_operation: OperationId::generate().map_err(RegistrationError::Allocation)?,
            node: SnapshotNodeId::generate().map_err(RegistrationError::Allocation)?,
            node_publication: OperationId::generate().map_err(RegistrationError::Allocation)?,
            completion: OperationId::generate().map_err(RegistrationError::Allocation)?,
        };
        let record = Record::ReserveHistoricalCommit(intent.clone());
        let commitment = record.commitment().map_err(RegistrationError::Codec)?;
        publication::publish_new(engine, request.operation_id, principal, &record)?;
        routing.directory = intent.directory();
        #[cfg(feature = "fault-injection")]
        engine.failpoint("after_worktree_historical_reservation")
            .map_err(RegistrationError::Publication)?;
        if let HistoricalFile::Fork { operation, name } = &intent.file {
            let receipt = engine.create_file_branch_from_node(
                &original_file.file_id.to_string(), &original_file.version_id.to_string(),
                &node.to_string(), name, principal.as_str(), *operation,
            ).map_err(RegistrationError::Publication)?;
            let created = crate::persist::snapshot_worktree::historical::file_branch(
                &receipt, &intent, principal,
            ).map_err(RegistrationError::Codec)?.ok_or(RegistrationError::CommitScope)?;
            routing.file_branch = created.branch_id;
            file_base = created.commit_id;
        }
        Some((intent, commitment))
    } else { None };
    // This lease is only a transient adapter to the existing FILE constructor.
    // The durable draft is the independent edit receipt/ledger above, not it.
    let workspace = engine.create_workspace(&routing.file_branch.to_string(), principal.as_str())
        .map_err(RegistrationError::Publication)?;
    let reserve_source = Arc::clone(&engine.state);
    let reserve_slot = engine.next_slot;
    let reserve_snapshot = engine.snapshot.clone();
    let attempt = (|| -> Result<_> {
        engine.stage_bytes(&workspace.workspace_id, edit.path.as_str(), &bytes)
            .map_err(RegistrationError::Publication)?;
        let command = CommitRequest {
            workspace_id: workspace.workspace_id.clone(),
            operation_id: match &historical {
                Some((intent, _)) => intent.file_operation,
                None => OperationId::generate().map_err(RegistrationError::Allocation)?,
            },
            principal: principal.as_str().into(), expected_head: workspace.base_head.clone(),
            fencing_token: workspace.fencing_token, message: request.message.clone(),
            request_digest: None,
        };
        let staged = engine.workspace(&workspace.workspace_id)
            .map_err(RegistrationError::Publication)?;
        let plan = CommitReservation {
            request: request.clone(), source: source_handle, binding, before, edit,
            original: original_file, original_source, file_history, file_base,
            file_operation: command.operation_id,
            file_request: FileRequestCommitment::capture(&staged, &command)
                .map_err(RegistrationError::Codec)?,
            node: match &historical {
                Some((intent, _)) => intent.node,
                None => SnapshotNodeId::generate().map_err(RegistrationError::Allocation)?,
            },
            node_publication: match &historical {
                Some((intent, _)) => intent.node_publication,
                None => OperationId::generate().map_err(RegistrationError::Allocation)?,
            },
            completion: match &historical {
                Some((intent, _)) => intent.completion,
                None => OperationId::generate().map_err(RegistrationError::Allocation)?,
            },
        };
        let (operation, record) = match &historical {
            Some((intent, commitment)) => (intent.ready, Record::PrepareHistoricalCommit(
                HistoricalReady {
                    reserve: request.operation_id, reservation: *commitment,
                    plan: plan.clone(), file_branch: routing.file_branch,
                },
            )),
            None => (request.operation_id, Record::ReserveCommit(plan.clone())),
        };
        let commitment = record.commitment().map_err(RegistrationError::Codec)?;
        publication::publish_new(engine, operation, principal, &record)?;
        Ok((command, plan, commitment))
    })();
    let (command, plan, commitment) = match attempt {
        Ok(reserved) => reserved,
        Err(operation) => {
            // next_slot advances before the first PREPARE, even when its I/O
            // fails. Only this still-exclusive, unchanged source proves that
            // the adapter created above has no possible reserved continuation.
            // Do not discover or adopt a workspace by actor, branch or bytes.
            if engine.blocked || engine.next_slot != reserve_slot
                || !Arc::ptr_eq(&engine.state, &reserve_source)
                || engine.snapshot != reserve_snapshot
            {
                return Err(operation);
            }
            return match engine.abort_workspace(&workspace.workspace_id, &workspace.principal) {
                Ok(()) => Err(operation),
                Err(cleanup) => Err(RegistrationError::CommitWorkspaceRelease {
                    operation: Box::new(operation),
                    cleanup,
                }),
            };
        }
    };
    let mut version = ReservedFileVersion::new(
        Arc::clone(&engine.state), plan.clone(), routing.file_branch,
    );
    let mut ledger = session.into_ledger();
    ledger.append(Event::Commit {
        operation: request.operation_id, expected: request.expected,
        current_head: routing.observed_head(), fork_branch: routing.directory.branch(),
    }, |_, _| Err(LedgerError::PublicationPending)).map_err(RegistrationError::Ledger)?;
    engine.commit_workspace_reserved_file(command, &mut version)
        .map_err(RegistrationError::Publication)?;
    let upper = Sequence::new(engine.state().sequence).next()
        .map_err(|source| RegistrationError::Ledger(LedgerError::Sequence(source)))?;
    let prepared_operation = match &historical {
        Some((intent, _)) => intent.ready,
        None => request.operation_id,
    };
    let reserve_sequence = engine.operation(prepared_operation)
        .map_err(RegistrationError::Publication)?.sequence;
    crate::snapshot_worktree::registry::verify_file_publication(
        engine.state(), &plan, routing.file_branch, principal, reserve_sequence, upper.get(),
    )?;
    complete(engine, principal, plan, routing, commitment, &original, instance, &mut ledger)
}

fn complete(
    engine: &mut Engine, principal: &Principal, plan: CommitReservation,
    routing: CommitRouting, commitment: MaterialCommitment, original: &HeldSnapshotNode,
    instance: HeldSparseWorktree, ledger: &mut HeldWorktreeLedger<'_>,
) -> Result<SnapshotHandle> {
    let index = engine.snapshot_receipts()?;
    let host = index.node(plan.source)?.host().clone();
    let manifest = index.into_commit_manifest(plan.request.operation_id)?;
    let root = engine.namespace_host_root(host.namespace.repository_id, host.namespace.branch_id)
        .map_err(RegistrationError::Publication)?;
    let transfer = ReservedDirectoryTransfer {
        original, instance, plan: &plan, expected: manifest.reference(),
    };
    let node = HeldSnapshotNode::prepare_worktree_commit(
        root.common().map_err(RegistrationError::Publication)?, manifest, &transfer,
    ).map_err(RegistrationError::NodePreparation)?;
    // The actual prepare's entity is reread in full before the SYSTEM record
    // can turn it into a permanent node. A pathname alone never supplies it.
    HeldSnapshotNode::reopen_at_host(
        root.common().map_err(RegistrationError::Publication)?, node.binding(),
    ).map_err(RegistrationError::NodeReadback)?;
    publication::publish_new(engine, plan.node_publication, principal,
        &Record::PublishCommitNode(CommitNode {
            reserve: plan.request.operation_id, reservation: commitment, binding: node.binding(),
        }))?;
    #[cfg(feature = "fault-injection")]
    engine.failpoint("after_worktree_node_publication").map_err(RegistrationError::Publication)?;
    let index = engine.snapshot_receipts()?;
    let files = vec![plan.file_result_at(routing.file_branch)];
    let checkpoint = ledger.append(Event::Completed {
        publication: plan.publication_at(routing), files,
    }, |publication, files| {
        index.require_committed_publication(plan.request.operation_id, publication, files)
    }).map_err(RegistrationError::Ledger)?;
    publish_completion(engine, principal, &plan, routing, commitment, checkpoint)
}

fn publish_completion(
    engine: &mut Engine, principal: &Principal, plan: &CommitReservation,
    routing: CommitRouting, commitment: MaterialCommitment,
    checkpoint: crate::directory_worktree::ledger::Checkpoint,
) -> Result<SnapshotHandle> {
    #[cfg(feature = "fault-injection")]
    engine.failpoint("after_worktree_ledger_completion").map_err(RegistrationError::Publication)?;
    publication::publish_new(engine, plan.completion, principal,
        &Record::CompleteCommit(CommitCompletion {
            reserve: plan.request.operation_id, reservation: commitment, checkpoint,
        }))?;
    let handle = plan.handle_at(routing.directory.branch());
    engine.snapshot_receipts()?.reopen_node(engine, handle)?;
    with_worktree(engine, plan.request.target, |_| Ok(()))?;
    Ok(handle)
}

/// No raw constructor is exposed. This borrows the actual completed FILE
/// publication's original SYSTEM intent and the same held sparse instance.
pub(crate) struct ReservedDirectoryTransfer<'a> {
    original: &'a HeldSnapshotNode,
    instance: HeldSparseWorktree,
    plan: &'a CommitReservation,
    expected: crate::working_tree::snapshot::SnapshotReference,
}

impl ReservedDirectoryTransfer<'_> {
    pub(crate) fn original(&self) -> &HeldSnapshotNode {
        self.original
    }

    pub(crate) fn require_manifest(&self, manifest: &AdmittedSnapshotManifest)
        -> std::result::Result<(), SnapshotStoreError>
    {
        if manifest.reference() != self.expected
            || self.original.manifest().reference() != self.plan.binding.source()
        {
            return Err(SnapshotStoreError::Repository);
        }
        Ok(())
    }

    pub(crate) fn transfer(
        &self, target: &crate::disk::DiskRoot,
        file: &SnapshotFile, name: &platform_io::fs_safety::EntryName,
    ) -> std::result::Result<(), SnapshotStoreError> {
        if file.version_id == self.plan.edit.material.version_id {
            if *file != self.plan.edit.material {
                return Err(SnapshotStoreError::Version);
            }
            self.instance.directory().move_new_entry_verified_synced(
                self.plan.edit.leaf.entry(), target, name, crate::disk::FrozenFileMove {
                    bytes: file.size, content_hash: file.content_hash,
                    entity: self.plan.edit.entity,
                },
            ).map_err(SnapshotStoreError::Move)?;
        } else {
            if self.original.manifest().files().get(&file.version_id) != Some(file) {
                return Err(SnapshotStoreError::Version);
            }
            target.copy_new_entry_verified_synced(name,
                &mut self.original.open_payload(file.version_id)?,
                platform_io::payload::Expectation {
                    bytes: file.size, blake3: Some(file.content_hash.digest()),
                },
            ).map_err(SnapshotStoreError::Copy)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/snapshot_worktree/historical_commit.rs"]
mod historical_tests;
