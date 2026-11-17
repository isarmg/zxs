//! One explicit edit is physically sealed before its authoritative checkpoint.
//! An interrupted reserve never adopts an existing leaf or a newer local frame.
use crate::container_storage::namespace_host::NamespaceHost;
use crate::directory_worktree::ledger::{EditLeaf, EditRecord, Event, HeldWorktreeLedger};
use crate::directory_worktree::store::HeldSparseWorktree;
use crate::persist::snapshot_worktree::{
    EditCompletion, EditReservation, Record, WorktreeReservation,
};
use crate::snapshot_worktree::{RegistrationError, Result, publication, with_worktree};
use crate::working_tree::records::local_entity;
use crate::working_tree::snapshot::{HeldSnapshotNode, SnapshotEntryContent, SnapshotFile};
use crate::{Engine, State, ValidatedConfig};
use std::{io::Cursor, sync::Arc};
use storage_contract::snapshot_worktree::{EditWorktree, WorktreeRevision};
use storage_contract::working_tree::MaterialCommitment;
use storage_contract::{Generation, OperationId, Principal, VersionId};

/// Only the same successful SYSTEM reserve can construct this writer. It owns
/// its actual environment and never reconstructs continuation from local bytes.
struct ReservedEdit {
    state: Arc<State>,
    config: ValidatedConfig,
    resources: Arc<crate::disk::DiskResources>,
    worktree: WorktreeReservation,
    plan: EditReservation,
    principal: Principal,
    commitment: MaterialCommitment,
}

pub(crate) fn edit_worktree(
    engine: &mut Engine,
    request: &EditWorktree,
    bytes: &[u8],
    principal: &Principal,
) -> Result<WorktreeRevision> {
    if request.size != bytes.len() as u64
        || request.content_hash != storage_contract::ContentHash::from_digest(blake3::hash(bytes))
        || request.size > engine.config().max_file_bytes
    {
        return Err(RegistrationError::EditBody);
    }
    let receipts = engine.snapshot_receipt_source()?;
    let index = receipts.index()?;
    if let Some(revision) = index.retry_edit(request, principal)? {
        with_worktree(engine, request.target, |_| Ok(()))?;
        return Ok(revision);
    }
    if engine.admitted_current_namespaces().map_err(RegistrationError::Publication)?.is_some()
        || engine.config.generation().is_some()
    {
        return Err(RegistrationError::InitialOnly);
    }
    let registered = index.worktree(request.target)?;
    let source = index.reopen_node(engine, registered.source().handle())?;
    let session = registered.reopen(&source)?;
    if session.worktree().revision() != request.expected
        || request.expected.generation != Generation::new(0)
        || request.path.parent().is_some()
    {
        return Err(RegistrationError::EditScope);
    }
    let entry = source.manifest().entries().iter()
        .find(|entry| entry.path == request.path).ok_or(RegistrationError::Source)?;
    let SnapshotEntryContent::File { material: original, source: original_source } = entry.content
    else {
        return Err(RegistrationError::EditScope);
    };
    let plan = EditReservation {
        request: request.clone(), binding: registered.binding(), before: registered.checkpoint(),
        original, original_source,
        version: VersionId::generate().map_err(RegistrationError::Allocation)?,
        completion: OperationId::generate().map_err(RegistrationError::Allocation)?,
    };
    let worktree = registered.reservation().clone();
    require_mount(engine, &worktree)?;
    let instance = HeldSparseWorktree::reopen(&source, session.instance())
        .map_err(RegistrationError::EditInstance)?;
    let maximum = instance.directory().component_name_maximum()
        .map_err(RegistrationError::Publication)?;
    crate::name_budget::NameBudget::from_maximum(maximum)
        .map_err(RegistrationError::EditName)?
        .component(request.path.file_name(), request.path.as_str())
        .map_err(RegistrationError::EditName)?;
    let record = Record::ReserveEdit(plan.clone());
    let commitment = record.commitment().map_err(RegistrationError::Codec)?;
    let config = engine.config.clone();
    let resources = Arc::clone(&engine.disk_resources);
    // The original held node/session do not borrow Engine. The actual reserve
    // captures all expected frame/material identities before creating a leaf.
    publication::publish_new(engine, request.operation_id, principal, &record)?;
    ReservedEdit {
        state: Arc::clone(&engine.state), config, resources, worktree, plan,
        principal: principal.clone(), commitment,
    }.complete(engine, &source, session.into_ledger(), bytes)
}

impl ReservedEdit {
    fn require_engine(&self, engine: &Engine) -> Result<()> {
        if !Arc::ptr_eq(&self.state, &engine.state)
            || !self.config.same_admission(&engine.config)
            || !Arc::ptr_eq(&self.resources, &engine.disk_resources)
            || engine.current_namespaces.is_some()
        {
            return Err(RegistrationError::PublicationChanged);
        }
        require_mount(engine, &self.worktree)
    }

    fn complete(
        self,
        engine: &mut Engine,
        source: &HeldSnapshotNode,
        mut ledger: HeldWorktreeLedger<'_>,
        bytes: &[u8],
    ) -> Result<WorktreeRevision> {
        self.require_engine(engine)?;
        if source.manifest().reference() != self.plan.binding.source()
            || ledger.binding() != self.plan.binding || ledger.checkpoint() != self.plan.before
            || ledger.worktree().revision() != self.plan.request.expected
        {
            return Err(RegistrationError::Source);
        }
        let instance = HeldSparseWorktree::reopen(source, ledger.instance())
            .map_err(RegistrationError::EditInstance)?;
        let request = &self.plan.request;
        let material = SnapshotFile {
            file_id: self.plan.original.file_id, version_id: self.plan.version,
            size: request.size, content_hash: request.content_hash,
        };
        let leaf = EditLeaf::for_version(self.plan.version, &request.path);
        let copied = instance.directory().copy_new_entry_verified_synced(
            leaf.entry(), &mut Cursor::new(bytes), platform_io::payload::Expectation {
                bytes: material.size, blake3: Some(material.content_hash.digest()),
            },
        ).map_err(RegistrationError::EditPreparation)?;
        let edit = EditRecord {
            path: request.path.clone(), material, leaf, entity: local_entity(copied.identity),
        };
        self.require_engine(engine)?;
        let checkpoint = ledger.append(Event::Edits {
            operation: request.operation_id, expected: request.expected,
            edits: vec![edit.clone()],
        }, |_, _| Err(crate::directory_worktree::ledger::LedgerError::PublicationPending))
            .map_err(RegistrationError::Ledger)?;
        ledger.admit_read_view().map_err(RegistrationError::Ledger)?;
        let revision = ledger.worktree().revision();
        self.require_engine(engine)?;
        let completion = Record::CompleteEdit(EditCompletion {
            reserve: request.operation_id, reservation: self.commitment,
            edit, checkpoint, revision,
        });
        publication::publish_new(engine, self.plan.completion, &self.principal, &completion)?;
        // Only the new independent SYSTEM checkpoint may reopen the local tail.
        with_worktree(engine, request.target, |session| {
            if session.worktree().revision() != revision {
                return Err(RegistrationError::PublicationChanged);
            }
            Ok(())
        })?;
        Ok(revision)
    }
}

fn require_mount(engine: &Engine, plan: &WorktreeReservation) -> Result<()> {
    NamespaceHost::registered(engine, plan.host.namespace)
        .map_err(RegistrationError::NamespaceHost)?
        .require_root_mount(
            plan.request.parent_directory_id, plan.request.expected_root_generation,
            &plan.request.name,
        ).map_err(RegistrationError::NamespaceHost)
}
