//! Object-scope SQLite material from one complete, already admitted State.
//! These bytes stay in a preparation domain, not an active container or an
//! independently elected authority. No database is copied from a live pathname.
use super::{directory_database, file_database};
use crate::{
    Error, ErrorCode, Result, canonical_json,
    history_package::{AdmittedState, preparation_checkpoint},
    state_projection::ProjectionTransition,
};
use data_serialization::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, sync::atomic::AtomicBool};

const MAX_DATABASES: usize = 4096;
const MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Scope {
    File {
        history: String,
        file: String,
    },
    Directory {
        history: String,
        repository: String,
        directory: String,
    },
}
impl Scope {
    fn path(&self) -> PathBuf {
        match self {
            Self::File { history, file } => PathBuf::from("histories")
                .join(history)
                .join("files")
                .join(file)
                .join(file_database::NAME),
            Self::Directory {
                history, directory, ..
            } => PathBuf::from("histories")
                .join(history)
                .join("directories")
                .join(directory)
                .join(directory_database::NAME),
        }
    }
    fn verify(&self, bytes: &[u8]) -> Result<()> {
        match self {
            Self::File { history, file } => file_database::verify_scope(bytes, file, history),
            Self::Directory {
                repository,
                directory,
                ..
            } => directory_database::verify_scope(bytes, repository, directory),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Descriptor {
    scope: Scope,
    #[serialization(with = "storage_contract::u64_string")]
    bytes: u64,
    digest: String,
}

pub(crate) struct Database {
    descriptor: Descriptor,
    bytes: Vec<u8>,
}
impl Database {
    pub(crate) fn path(&self) -> PathBuf {
        self.descriptor.scope.path()
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub(crate) fn verify(&self, actual: &[u8]) -> Result<()> {
        // Expected bytes originate from the admitted snapshot, not a stored
        // self hash. Exact comparison also binds sequence, rows and scope.
        if actual != self.bytes {
            return Err(Error::new(
                ErrorCode::ContainerLayoutInvalid,
                "prepared object database differs from the complete frozen State",
            ));
        }
        self.descriptor.scope.verify(actual)
    }
}

pub(crate) struct PreparedDatabases {
    databases: Vec<Database>,
    bytes: usize,
    files: usize,
    directories: usize,
}
impl PreparedDatabases {
    pub(crate) fn capture(admitted: &AdmittedState, stop: &AtomicBool) -> Result<Self> {
        let state = admitted.state();
        let mut files = BTreeMap::new();
        for version in state.versions.values() {
            if files
                .get(&version.file_id)
                .is_some_and(|history| *history != &version.history_tree_id)
            {
                return Err(Error::new(
                    ErrorCode::StateInvalid,
                    "one prepared file cannot belong to multiple history units",
                ));
            }
            files.insert(&version.file_id, &version.history_tree_id);
            if files.len() > MAX_DATABASES {
                return Err(budget());
            }
        }
        if files
            .len()
            .checked_add(state.directory_git_repositories.len())
            .is_none_or(|count| count > MAX_DATABASES)
        {
            return Err(budget());
        }
        let transition = ProjectionTransition::new(state, state, None)?;
        let view = transition.previous();
        let mut prepared = Self {
            databases: Vec::new(),
            bytes: 0,
            files: 0,
            directories: 0,
        };
        for (file, history) in files {
            preparation_checkpoint(stop)?;
            // The older graph-only copy never retained its imported D/undo.
            // Do not convert that absence into a complete database certificate.
            if view.owners().file_rows(file).is_some_and(|rows| {
                rows.history_copies
                    .keys()
                    .any(|id| state.operations[id].kind == crate::business::history_copy::KIND)
            }) {
                return Err(Error::new(
                    ErrorCode::UnsupportedOperation,
                    "graph-only copied history lacks its imported lifecycle closure",
                ));
            }
            let bytes = file_database::prepare(view, file, history)?.bytes;
            prepared.add(
                Scope::File {
                    history: history.clone(),
                    file: file.clone(),
                },
                bytes,
            )?;
            prepared.files += 1;
        }
        for (directory, git) in &state.directory_git_repositories {
            preparation_checkpoint(stop)?;
            let history = &state.repositories[&git.repository_id].history_tree_id;
            let bytes = directory_database::prepare(state, directory)?.bytes;
            prepared.add(
                Scope::Directory {
                    history: history.clone(),
                    repository: git.repository_id.clone(),
                    directory: directory.clone(),
                },
                bytes,
            )?;
            prepared.directories += 1;
        }
        Ok(prepared)
    }
    fn add(&mut self, scope: Scope, bytes: Vec<u8>) -> Result<()> {
        self.bytes = self.bytes.checked_add(bytes.len()).ok_or_else(budget)?;
        if self.bytes > MAX_BYTES || self.databases.len() == MAX_DATABASES {
            return Err(budget());
        }
        scope.verify(&bytes)?;
        let descriptor = Descriptor {
            scope,
            bytes: bytes.len() as u64,
            digest: blake3::hash(&bytes).to_hex().to_string(),
        };
        self.databases.push(Database { descriptor, bytes });
        Ok(())
    }
    pub(crate) fn databases(&self) -> &[Database] {
        &self.databases
    }
    pub(crate) fn file_count(&self) -> usize {
        self.files
    }
    pub(crate) fn directory_count(&self) -> usize {
        self.directories
    }
    pub(crate) fn total_bytes(&self) -> u64 {
        self.bytes as u64
    }
    pub(crate) fn manifest(&self) -> Result<Vec<u8>> {
        let bytes = canonical_json(
            &self
                .databases
                .iter()
                .map(|database| &database.descriptor)
                .collect::<Vec<_>>(),
        )?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(budget());
        }
        Ok(bytes)
    }
}
fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "all object databases must fit the 4096-database/64-MiB complete-set budget",
    )
}
