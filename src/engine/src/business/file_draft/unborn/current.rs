//! Current-only structural closure, deliberately outside historical ownership.
use crate::business::file_draft::invalid;
use crate::business::namespace::working::WorkingNamespace;
use crate::persist::file_draft::birth as codec;
use crate::{Result, State};
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::file_draft::{DraftCommand, UnbornBody, UnbornDecision, UnbornWorking};
use storage_contract::file_container::FileContainerRecord;
use storage_contract::{FileId, Principal, Sequence};

/// Birth, save and the last explicit mode decision are independently related to
/// the current working row. Native admission replays each original pair; actual
/// extents remain mandatory when the held content is used.
pub(crate) fn validate(state: &State, maximum: usize) -> Result<BTreeSet<FileId>> {
    let mut namespaces = BTreeMap::new();
    let mut files = BTreeSet::new();
    for (file, record) in state.file_containers.iter() {
        if record.working.unborn().is_none() { continue; }
        if record.namespace.parent.is_worktree() {
            super::super::worktree::validate_record(state, *file, record)?;
            files.insert(*file);
            continue;
        }
        let repository = record.namespace.repository;
        if let std::collections::btree_map::Entry::Vacant(entry) = namespaces.entry(repository) {
            entry.insert(WorkingNamespace::from_main_state(state, repository, maximum)?);
        }
        let namespace = namespaces.get(&repository).ok_or_else(invalid)?;
        require_record(state, namespace, *file, record)?;
        files.insert(*file);
    }
    Ok(files)
}

pub(crate) fn validate_record(
    state: &State,
    file: FileId,
    record: &FileContainerRecord,
) -> Result<()> {
    if record.namespace.parent.is_worktree() {
        return super::super::worktree::validate_record(state, file, record);
    }
    let namespace = WorkingNamespace::from_main_state(
        state, record.namespace.repository, usize::MAX,
    )?;
    require_record(state, &namespace, file, record)
}

fn require_record(
    state: &State,
    namespace: &WorkingNamespace<'_>,
    file: FileId,
    record: &FileContainerRecord,
) -> Result<()> {
    if namespace.unborn_file(file)? != record {
        return Err(invalid());
    }
    let receipt = state.operations.get(&record.created_by).ok_or_else(invalid)?;
    let principal = state.operation_principals.get(&record.created_by)
        .ok_or_else(invalid)?        .parse::<Principal>()        .map_err(|_| invalid())?;
    let decoded = codec::decode(receipt)?;
    if receipt.sequence > state.sequence
        || decoded.receipt(record.created_by, Sequence::new(receipt.sequence), &principal)?
        != *receipt
        || decoded.owner() != record.owner_disk
        || decoded.target().file != file
        || decoded.target().entry != record.namespace.entry
        || decoded.target().repository != record.namespace.repository
    {
        return Err(invalid());
    }
    let current = namespace.get(&name_policy::LookupPath::from(&decoded.target().path))
        .ok_or_else(invalid)?;
    if current.entry_id()? != record.namespace.entry {
        return Err(invalid());
    }
    let working = record.working.unborn().ok_or_else(invalid)?;
    if let Some(controller) = working.automatic_controller() {
        controller.validate(working.revision())
            .map_err(|error| crate::Error::FileDraft(crate::DraftError::Controller(error)))?;
    }
    match working.last_decision() {
        UnbornDecision::Birth {} if working == &UnbornWorking::birth() => return Ok(()),
        UnbornDecision::Birth {} => return Err(invalid()),
        UnbornDecision::Mode { operation } => {
            let change = state.operations.get(&operation).ok_or_else(invalid)?;
            let actor = state.operation_principals.get(&operation).ok_or_else(invalid)?;
            let original = crate::persist::file_draft::decode(state, change)?;
            let outcome = original.outcome();
            if change.sequence <= receipt.sequence || change.sequence > state.sequence
                || !matches!(&outcome.request, crate::persist::file_draft::Request::Command(
                    DraftCommand::SetMode { .. } | DraftCommand::ResumeAutomatic { .. }))
                || outcome.request.target() != decoded.target() || outcome.content.is_some()
                || outcome.working != record.working
                || original.receipt(state, operation, change.sequence, actor)? != *change
            { return Err(invalid()); }
        }
        UnbornDecision::Save { operation } => {
            if !matches!(working.body(), UnbornBody::Saved { manifest } if manifest.operation() == operation) {
                return Err(invalid());
            }
        }
    }
    if let UnbornBody::Saved { manifest } = working.body() {
        let saved = state.operations.get(&manifest.operation()).ok_or_else(invalid)?;
        let principal = state.operation_principals.get(&manifest.operation())
            .ok_or_else(invalid)?.parse::<Principal>().map_err(|_| invalid())?;
        let save = crate::persist::file_draft::unborn_save::decode(saved)?;
        let after = save.request().expected.next()
            .map_err(|source| crate::Error::FileDraft(crate::DraftError::RevisionExhausted(source)))?;
        if saved.sequence <= receipt.sequence || saved.sequence > state.sequence
            || save.receipt(manifest.operation(), Sequence::new(saved.sequence), &principal)? != *saved
            || save.request().target != *decoded.target() || &save.request().manifest != manifest
            || after > working.revision()
            || (matches!(working.last_decision(), UnbornDecision::Save { .. }) &&
                (after != working.revision() || save.request().controller != record.working.save_controller()))
        { return Err(invalid()); }
    }
    Ok(())
}
