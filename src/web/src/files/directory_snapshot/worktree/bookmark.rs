//! A last-instance convenience reference, not a publication receipt or grant.
//! It may be replaced; every use still reads and admits the exact server status.
use crate::api;
use crate::files::directory_snapshot::wire::{self, DecodeError};
use crate::files::recovery_storage::Store;
use data_serialization::{Deserialize, Serialize};
use storage_contract::snapshot_worktree::WorktreeHandle;
use storage_contract::{Principal, RepositoryId};

const MAX_BYTES: usize = 4096;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "directory-worktree-last-instance")]
    LastInstance,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Reference {
    format: Format,
    principal: Principal,
    repository: RepositoryId,
    pub(super) target: WorktreeHandle,
}

impl Reference {
    pub(super) fn new(
        principal: Principal,
        repository: RepositoryId,
        target: WorktreeHandle,
    ) -> Self {
        Self {
            format: Format::LastInstance,
            principal,
            repository,
            target,
        }
    }

    pub(super) fn belongs(&self, principal: &Principal, repository: RepositoryId) -> bool {
        &self.principal == principal && self.repository == repository
    }

    pub(super) fn encode(&self) -> Result<String, api::Error> {
        let bytes = json_codec::to_string(self)
            .map_err(|source| wire::failure(DecodeError::Wire(source)))?;
        if bytes.len() > MAX_BYTES {
            return Err(wire::invalid(
                "the worktree reference exceeds its browser byte budget",
            ));
        }
        Ok(bytes)
    }
}

pub(super) fn key(principal: &Principal, repository: RepositoryId) -> String {
    format!(
        "zxs:worktree-last:{}:{repository}",
        api::segment(principal.as_str())
    )
}

pub(super) fn parse(
    bytes: &str,
    principal: &Principal,
    repository: RepositoryId,
) -> Result<Reference, api::Error> {
    if bytes.len() > MAX_BYTES {
        return Err(wire::invalid(
            "the saved worktree reference exceeds its byte budget",
        ));
    }
    let value = storage_contract::strict_json(bytes.as_bytes())
        .map_err(|source| wire::failure(DecodeError::Contract(source)))?;
    wire::object_field(&value, "target")?;
    let reference: Reference = wire::decode(value)?;
    if !reference.belongs(principal, repository) {
        return Err(wire::invalid(
            "the saved worktree reference belongs to another account or root",
        ));
    }
    Ok(reference)
}

fn read(
    store: &Store,
    principal: &Principal,
    repository: RepositoryId,
) -> Result<Option<Reference>, api::Error> {
    let key = key(principal, repository);
    let mut reference = None;
    store.scan(&key, |found, bytes| {
        if found == key {
            reference = Some(parse(bytes, principal, repository)?);
        }
        Ok(())
    })?;
    Ok(reference)
}

pub(super) fn load(
    principal: &Principal,
    repository: RepositoryId,
) -> Result<Option<Reference>, api::Error> {
    read(&Store::open()?, principal, repository)
}

pub(super) fn save(reference: &Reference) -> Result<(), api::Error> {
    let store = Store::open()?;
    let key = key(&reference.principal, reference.repository);
    let matches =
        |bytes: &str| Ok(parse(bytes, &reference.principal, reference.repository)? == *reference);
    if let Some(previous) = read(&store, &reference.principal, reference.repository)? {
        if previous == *reference {
            return store.verify(&key, matches);
        }
        // This is replaceable navigation state, never an original request.
        // Store readback detects observed conflicts, not a cross-tab atomic CAS.
        // Another tab may replace this convenience reference after verification.
        store.remove(&key, |bytes| {
            Ok(parse(bytes, &reference.principal, reference.repository)? == previous)
        })?;
    }
    store.save(&key, &reference.encode()?, matches)
}
