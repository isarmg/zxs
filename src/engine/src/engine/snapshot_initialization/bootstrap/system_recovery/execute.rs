//! Explicit destination installation after immutable source admission.
use crate::authority_selection::carrier::{RecoveryRequest, RecoverySelection};
use crate::authority_selection::{FreshBootstrapError, SelectionGuard};
use crate::engine::snapshot_initialization::bootstrap::{
    MAX_BYTES, ROOTS, bootstrap_identity, decision::BootstrapDecision, failure, name,
};
use crate::engine::snapshot_initialization::bootstrap::system_recovery::{
    StorageRecoveryAdmission, decision::{RecoveryBinding, RecoveryCompletion, KIND}, refused,
};
use crate::disk::WriterLocks;
use crate::{Engine, OperationId};
use platform_io::fs_safety::{AtomicFile, PayloadOpenMode, PrivateDirectory};
use platform_io::fs_safety::linux::{MountPolicy, OpenAt2Root};
use storage_contract::{ContentHash, Principal};
use storage_contract::working_tree::{LocalEntityIdentity, MaterialCommitment};

impl Engine {
    /// Create one explicitly authorized successor SYSTEM. This first execution
    /// slice requires every original configured root online; source admission
    /// itself still uses the original metadata quorum. No quorum rule is relaxed.
    pub fn restore_storage_held(
        system: PrivateDirectory,
        operation: OperationId,
        principal: Principal,
        mut admission: StorageRecoveryAdmission,
        control_intent: MaterialCommitment,
        control: crate::StorageRecoveryControl,
        workspace_limits: Option<crate::RetainedWorkspaceLimits>,
    ) -> crate::Result<Self> {
        control.require_origin(&admission.origin)?;
        if control.commitment()? != control_intent || control.binding.operation != operation
            || control.principal != principal
        { return Err(refused()); }
        admission.require_recovery_intent_source(operation, &principal,
            control.source_evidence, control_intent)?;
        admission.config = admission.config.with_workspace_limits(workspace_limits)?;
        if admission.resources.handles().count() != admission.config.disks.len() {
            return Err(failure(FreshBootstrapError::Incomplete));
        }
        if let Some(result) = admission.state.operations.get(&operation) {
            if result.kind != KIND {
                return Err(refused());
            }
            let binding: RecoveryBinding = storage_contract::decode_strict(
                &storage_contract::canonical_json(&result.result)?,
            )?;
            if binding.request.operation != operation || binding.request.principal != principal
                || binding.request.control_intent != control_intent
                || result != &binding.result(result.sequence)?
            {
                return Err(refused());
            }
            return reopen_completed(system, admission, &binding);
        }
        let anchor = OpenAt2Root::from_private_directory(system.try_clone()?, MountPolicy::SameMount)?;
        let identity = anchor.identity();
        let request = RecoveryRequest {
            operation,
            principal,
            control_intent,
            source_evidence: admission.evidence,
            source_candidate: storage_contract::object_hash("candidate", &admission.source)?
                .parse::<ContentHash>().map_err(|_| refused())?,
            source_snapshot: admission.source.snapshot_hash.parse::<ContentHash>().map_err(|_| refused())?,
            system: LocalEntityIdentity::new(identity.device, identity.inode),
        };
        let selected = RecoverySelection::new(admission.origin.clone(), request.clone(), control);
        let selection = SelectionGuard::acquire_held(system.try_clone()?)?.select_recovery(selected)?;
        let writer = system.open_payload(&crate::persist::layout::CONTROL_WRITER_LOCK,
            PayloadOpenMode::CreateNew)?;
        crate::engine::config::acquire_writer(&writer)?;
        let writer_identity = platform_io::fs_safety::linux::FileIdentity::from_fd(&writer)?;
        let mut locks = WriterLocks::new(writer, selection);
        for (disk, writer) in admission.writers {
            locks.insert(disk, writer);
        }
        // Only the new carrier receives installation material. Original source
        // root identity records, config, selectors and seals stay untouched.
        let root = system.path().to_owned();
        crate::engine::disk_lifecycle::bootstrap::runtime::initialize_recovery_carrier(
            &system, &admission.config, &admission.resources,
        )?;
        let (bootstrap_root, bootstrap_writer) = bootstrap_identity(&system).map_err(failure)?;
        let first = admission.config.io_disks().next().ok_or_else(refused)?;
        let original_bytes = admission.resources.get(first)?.root.read_bytes(
            &first.root.join(crate::persist::layout::STORAGE_INITIALIZATION_PROOF.as_path()),
            MAX_BYTES,
        )?;
        let (origin, record) = BootstrapDecision::admit_source(&original_bytes)?;
        if record != admission.original {
            return Err(refused());
        }
        let binding = RecoveryBinding {
            request,
            system_writer: LocalEntityIdentity::new(writer_identity.device, writer_identity.inode),
            bootstrap_root,
            bootstrap_writer,
            origin,
        };
        AtomicFile::create(&system, &name(ROOTS).map_err(failure)?, &record.bytes().map_err(failure)?)?;
        AtomicFile::create(&system, &crate::persist::layout::SYSTEM_CONFIG,
            &storage_contract::canonical_json(&admission.config)?)?;
        let mut engine = Engine::recovery_resources_with_locks(
            root, admission.config, admission.resources, locks,
        )?;
        for (disk, opened) in admission.journals {
            if opened.scan.torn_tail {
                return Err(refused());
            }
            let (writer, _) = opened.into_writer()?;
            writer.verify_retained_name()?;
            engine.journals.insert(disk, writer);
        }
        engine.install_recovered_namespace_state(
            admission.state,
            admission.source.snapshot_hash.clone(),
        )?;
        engine.next_slot = engine.state.sequence.checked_add(1).ok_or_else(refused)?;
        let mut next = (*engine.state).clone();
        next.sequence = engine.next_slot;
        let result = binding.result(next.sequence)?;
        next.operations.insert(operation, result.clone());
        next.operation_principals.insert(operation, binding.request.principal.as_str().to_owned());
        let prepared = engine.prepare_publication(next, Vec::new(), result, crate::disk::MAX_METADATA)?;
        engine.decide_publication(prepared)?;
        let (proof, _) = crate::engine::snapshot_initialization::bootstrap::system_recovery::decision
            ::require_complete(&system, &engine.config, &engine.disk_resources)?;
        engine.config = engine.config.clone().with_fresh_completion(proof);
        let bootstrap = crate::engine::disk_lifecycle::bootstrap::runtime::open_store_for_config(
            &engine.root, &engine.config,
        )?;
        // Rebuild the live Current/directory/history bindings through ordinary
        // verified replay, not by treating the latest State as every runtime
        // projection. Keep the same admitted resources and writer owners while
        // resetting only this not-yet-returned Engine's in-memory replay cursor.
        engine.named_namespaces.install_state(&mut engine.state, std::sync::Arc::new(crate::State {
            topology_generation: 1,
            ..crate::State::default()
        }));
        engine.named_namespaces.install_current(&mut engine.current_namespaces, None);
        engine.snapshot = None;
        engine.next_slot = 1;
        engine.published_state_base = None;
        engine.history_window_roots = None;
        engine.directory_snapshots.clear();
        let result = Engine::finish_open(
            engine, Vec::new(), crate::engine::recovery::RecoveryMode::CompletePending,
        );
        drop(bootstrap);
        result
    }
}

fn reopen_completed(
    system: PrivateDirectory,
    admission: StorageRecoveryAdmission,
    binding: &RecoveryBinding,
) -> crate::Result<Engine> {
    let selected = RecoverySelection::read(&system).map_err(failure)?.ok_or_else(refused)?;
    if selected.request != binding.request || selected.origin != admission.origin {
        return Err(refused());
    }
    let selection = SelectionGuard::acquire_held(system.try_clone()?)?.admit_recovery(selected)?;
    let (proof, _) = crate::engine::snapshot_initialization::bootstrap::system_recovery::decision
        ::require_complete(&system, &admission.config, &admission.resources)?;
    let complete = RecoveryCompletion::decode(&system.read_bounded(
        &name(crate::engine::snapshot_initialization::bootstrap::COMPLETE).map_err(failure)?,
        MAX_BYTES,
    )?)?;
    if storage_contract::canonical_json(&complete.binding)? != storage_contract::canonical_json(binding)? {
        return Err(refused());
    }
    let writer = system.open_payload(&crate::persist::layout::CONTROL_WRITER_LOCK,
        PayloadOpenMode::Existing)?;
    crate::engine::config::acquire_writer(&writer)?;
    let mut locks = WriterLocks::new(writer, selection);
    for (disk, writer) in admission.writers {
        locks.insert(disk, writer);
    }
    let config = admission.config.with_fresh_completion(proof);
    let bootstrap = crate::engine::disk_lifecycle::bootstrap::runtime::open_store_for_config(
        system.path(), &config,
    )?;
    let engine = Engine::recovery_resources_with_locks(
        system.path().to_owned(), config, admission.resources, locks,
    )?;
    let result = Engine::finish_open(
        engine, Vec::new(), crate::engine::recovery::RecoveryMode::CompletePending,
    );
    drop(bootstrap);
    result
}

#[cfg(test)]
#[path = "../../../../../../../test/engine/unit/system_recovery_execution_tests.rs"]
mod tests;
