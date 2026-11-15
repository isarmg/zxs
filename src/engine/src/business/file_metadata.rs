//! One metadata-only construction for live requests and authenticated replay.
use crate::engine::CurrentNamespaces;
use crate::{BranchState, Engine, Error, FileMetadataError, OperationId, OperationResult, Result};
use crate::{Branch, Repository, State};
use storage_contract::{BranchId, BranchName, FileId, NamespaceEntryKind};
use storage_contract::{Note, VersionId};

#[derive(Clone, Copy)]
pub(crate) enum Request<'a> {
    Note { file: FileId, version: Option<VersionId>, note: &'a Note },
    Rename { file: FileId, branch: BranchId, name: &'a BranchName },
}

impl Request<'_> {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Note { version: None, .. } => "file.metadata",
            Self::Note { version: Some(_), .. } => "version.metadata",
            Self::Rename { .. } => "file.branch.rename",
        }
    }
}

pub(crate) enum Plan<'a> {
    Note { file: FileId, version: Option<VersionId>, note: &'a Note },
    Rename {
        file: FileId,
        branch: BranchId,
        source: &'a Branch,
        repository: &'a Repository,
        name: &'a BranchName,
    },
}

impl Plan<'_> {
    pub(crate) fn request(&self) -> Request<'_> {
        match self {
            Self::Note { file, version, note } => Request::Note {
                file: *file, version: *version, note,
            },
            Self::Rename { file, branch, name, .. } => Request::Rename {
                file: *file, branch: *branch, name,
            },
        }
    }

    pub(crate) fn apply(&self, state: &mut State) {
        match self {
            Self::Note { file, version, note } => {
                let (notes, key) = match version {
                    Some(version) => (&mut state.version_notes, version.to_string()),
                    None => (&mut state.file_notes, file.to_string()),
                };
                if note.as_str().is_empty() {
                    notes.remove(&key);
                } else {
                    notes.insert(key, note.as_str().into());
                }
            }
            Self::Rename { branch, source, name, .. } => {
                if source.name != name.as_str() {
                    state.branches.get_mut(&branch.to_string())
                        .expect("invariant: planning selected this existing branch")
                        .name = name.as_str().into();
                }
            }
        }
    }
}

pub(crate) fn invalid() -> Error {
    Error::FileMetadata(FileMetadataError::Publication)
}

fn repository<'a>(state: &'a State, history: &str) -> Result<&'a Repository> {
    let history = state.histories.get(history)
        .ok_or(Error::FileMetadata(FileMetadataError::HistoryMissing))?;
    state.repositories.get(&history.repository_id)
        .ok_or(Error::FileMetadata(FileMetadataError::RepositoryMissing))
}

fn current_file(state: &State, current: Option<&CurrentNamespaces>, file: FileId) -> Result<()> {
    let Some(view) = current else { return Ok(()); };
    let file = file.to_string();
    for branch in state.branches.values() {
        if branch.state != BranchState::Migrated
            && view.branch_entries(&branch.branch_id, &branch.head_commit_id)?.iter()
                .any(|entry| entry.kind == NamespaceEntryKind::File && entry.file_id == file)
        {
            return Ok(());
        }
    }
    for branch in state.directory_branches.values() {
        if view.directory_entries(&branch.directory_branch_id, &branch.head_tree_id)?.iter()
            .any(|entry| entry.kind == NamespaceEntryKind::File && entry.file_id == file)
        {
            return Ok(());
        }
    }
    Err(Error::FileMetadata(FileMetadataError::CurrentFileMissing))
}

pub(crate) fn plan<'a>(
    state: &'a State,
    current: Option<&CurrentNamespaces>,
    request: Request<'a>,
) -> Result<Plan<'a>> {
    match request {
        Request::Note { file, version, note } => {
            let file_text = file.to_string();
            let source = match version {
                Some(version) => state.versions.get(&version.to_string())
                    .filter(|version| version.file_id == file_text),
                None => {
                    current_file(state, current, file)?;
                    state.versions.values().find(|version| version.file_id == file_text)
                }
            }.ok_or(Error::FileMetadata(FileMetadataError::FileMissing))?;
            repository(state, &source.history_tree_id)?;
            Ok(Plan::Note { file, version, note })
        }
        Request::Rename { file, branch, name } => {
            let file_text = file.to_string();
            let branch_text = branch.to_string();
            let source = state.branches.get(&branch_text).filter(|branch| {
                branch.file_id.as_deref() == Some(file_text.as_str())
                    && branch.state != BranchState::Migrated
            }).ok_or(Error::FileMetadata(FileMetadataError::BranchMissing))?;
            let repository = repository(state, &source.history_tree_id)?;
            if source.state != BranchState::Active
                || state.file_branch_aliases.contains_key(&branch_text)
            {
                return Err(Error::FileMetadata(FileMetadataError::InactiveBranch));
            }
            if let Some(view) = current
                && !view.branch_entries(&branch_text, &source.head_commit_id)?.iter()
                    .any(|entry| {
                        entry.kind == NamespaceEntryKind::File && entry.file_id == file_text
                    })
            {
                return Err(Error::FileMetadata(FileMetadataError::CurrentBranchMissing));
            }
            if state.branches.values().any(|other| {
                other.file_id.as_deref() == Some(file_text.as_str())
                    && other.branch_id != branch_text
                    && name_policy::lookup_key(&other.name) == name.key()
            }) {
                return Err(Error::FileMetadata(FileMetadataError::NameCollision));
            }
            Ok(Plan::Rename {
                file,
                branch,
                source,
                repository,
                name,
            })
        }
    }
}

impl Engine {
    pub fn set_file_note(
        &mut self,
        file_id: &str,
        version_id: Option<&str>,
        note: &Note,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        let kind = if version_id.is_some() { "version.metadata" } else { "file.metadata" };
        self.mutate(
            kind, principal, operation,
            crate::persist::file_metadata::note_input(file_id, version_id, note),
            |engine, next, _| {
                let parse = |source| Error::FileMetadata(FileMetadataError::FileIdentifier(source));
                let request = Request::Note {
                    file: file_id.parse().map_err(parse)?,
                    version: version_id.map(str::parse).transpose().map_err(parse)?,
                    note,
                };
                let replacement = plan(
                    engine.state(), engine.current_namespaces.as_deref(), request,
                )?;
                replacement.apply(next);
                Ok(crate::persist::file_metadata::outcome(&replacement))
            },
        )
    }

    pub fn rename_file_branch(
        &mut self,
        file_id: &str,
        branch_id: &str,
        name: &BranchName,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.mutate(
            "file.branch.rename", principal, operation,
            crate::persist::file_metadata::rename_input(file_id, branch_id, name),
            |engine, next, _| {
                let parse = |source| {
                    Error::FileMetadata(FileMetadataError::BranchIdentifier(source))
                };
                let request = Request::Rename {
                    file: file_id.parse().map_err(parse)?,
                    branch: branch_id.parse().map_err(parse)?,
                    name,
                };
                let replacement = plan(
                    engine.state(), engine.current_namespaces.as_deref(), request,
                )?;
                replacement.apply(next);
                Ok(crate::persist::file_metadata::outcome(&replacement))
            },
        )
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/retained_metadata_tests.rs"]
mod retained_metadata_tests;
