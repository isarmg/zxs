//! Current-only structural closure, deliberately outside historical ownership.
use crate::business::file_draft::invalid;
use crate::business::namespace::working::WorkingNamespace;
use crate::persist::file_draft::birth as codec;
use crate::{Result, State};
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::file_draft::{CommitMode, UnbornBody, UnbornWorking};
use storage_contract::file_container::FileContainerRecord;
use storage_contract::{FileId, Principal, Sequence};

/// Structural closure of Birth and Save receipts. Native pair admission still
/// reconstructs every complete original transition and the held consumer checks
/// every extent. Later mode transitions have no issuer here.
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
    match working.body() {
        UnbornBody::Empty {} if working == &UnbornWorking::birth() => Ok(()),
        UnbornBody::Empty {} => Err(invalid()),
        UnbornBody::Saved { manifest } => {
            let saved = state.operations.get(&manifest.operation()).ok_or_else(invalid)?;
            let principal = state.operation_principals.get(&manifest.operation())
                .ok_or_else(invalid)?                .parse::<Principal>()                .map_err(|_| invalid())?;
            let save = crate::persist::file_draft::unborn_save::decode(saved)?;
            if saved.sequence <= receipt.sequence || saved.sequence > state.sequence
                || save.receipt(manifest.operation(), Sequence::new(saved.sequence), &principal)?
                    != *saved
                || save.request().target != *decoded.target()
                || &save.request().manifest != manifest
                || working.mode() != CommitMode::Manual
                || save.request().expected.generation().get()
                    != manifest.generation().generation().get()
                || save.request().expected.next().map_err(|source| {
                    crate::Error::FileDraft(crate::DraftError::RevisionExhausted(source))
                })? != working.revision()
            {
                return Err(invalid());
            }
            Ok(())
        }
    }
}
