//! One full-set CAS before a directory or file consumer opens working material.
//! This is logical selection only: the Engine separately retains actual sources.
use crate::business::file_draft::{DraftError as Failure, commit, selection, unborn};
use crate::business::namespace::working::WorkingNamespace;
use crate::{Error, Result, State};
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::file_container::FileContainerRecord;
use storage_contract::file_draft::{
    WorkingContentExpectation, WorkingExpectation, WorkingSelection,
};

pub(crate) struct CheckedWorkingSelection<'a> {
    request: &'a WorkingSelection,
    record: &'a FileContainerRecord,
}

impl CheckedWorkingSelection<'_> {
    pub(crate) fn request(&self) -> &WorkingSelection { self.request }
    pub(crate) fn record(&self) -> &FileContainerRecord { self.record }
}

pub(crate) fn require_all<'a>(
    state: &'a State,
    requests: &'a [WorkingSelection],
    maximum_entries: usize,
    maximum_bytes: u64,
) -> Result<Vec<CheckedWorkingSelection<'a>>> {
    if requests.is_empty() || requests.len() > maximum_entries {
        return Err(Error::FileDraft(Failure::WorkingSelection));
    }
    let mut files = BTreeSet::new();
    let mut entries = BTreeSet::new();
    let mut namespaces = BTreeMap::new();
    let mut checked = Vec::with_capacity(requests.len());
    let mut paths: Vec<_> = requests.iter()
        .map(|request| (request.target.repository, &request.target.path)).collect();
    paths.sort_by(|left, right| {
        left.0.cmp(&right.0).then_with(|| left.1.key().cmp(right.1.key()))
    });
    if paths.windows(2).any(|pair| {
        pair[0].0 == pair[1].0 && pair[0].1.key() == pair[1].1.key()
    }) {
        return Err(Error::FileDraft(Failure::WorkingSelection));
    }
    for request in requests {
        let target = &request.target;
        if !files.insert(target.file) || !entries.insert(target.entry) {
            return Err(Error::FileDraft(Failure::WorkingSelection));
        }
        let record = state.file_containers.get(&target.file)
            .ok_or(Error::FileDraft(Failure::Selection))?;
        let revision = record.working.revision();
        if revision != request.expected.revision {
            return Err(Error::FileDraft(Failure::RevisionChanged {
                expected: request.expected.revision, actual: revision,
            }));
        }
        if WorkingExpectation::from_working(&record.working) != Some(request.expected) {
            return Err(Error::FileDraft(Failure::WorkingBodyChanged));
        }
        match request.expected.content {
            WorkingContentExpectation::Born { base, payload } => {
                let source = selection::resolve(state, target)?;
                selection::validate(state, target.file, record)?;
                let planned = commit(&record.working, revision).map_err(Error::FileDraft)?;
                if source.base != base || *planned.base() != base || *planned.payload() != payload {
                    return Err(Error::FileDraft(Failure::BaseChanged));
                }
                if payload.logical_size() > maximum_bytes {
                    return Err(Error::FileDraft(Failure::Size));
                }
            }
            WorkingContentExpectation::UnbornEmpty
            | WorkingContentExpectation::UnbornSaved { .. } => {
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    namespaces.entry(target.repository)
                {
                    entry.insert(WorkingNamespace::from_main_state(
                        state, target.repository, maximum_entries,
                    )?);
                }
                let namespace = namespaces.get(&target.repository)
                    .ok_or(Error::FileDraft(Failure::WorkingNamespace))?;
                unborn::selected(namespace, target, revision)?;
                let working = record.working.unborn()
                    .ok_or(Error::FileDraft(Failure::UnbornRequired))?;
                if working.logical_size() > maximum_bytes {
                    return Err(Error::FileDraft(Failure::Size));
                }
            }
        }
        checked.push(CheckedWorkingSelection { request, record });
    }
    Ok(checked)
}
