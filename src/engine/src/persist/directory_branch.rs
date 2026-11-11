//! Canonical request and receipt encoding for historical directory registration.
use crate::business::directory_git::{HistoricalBranchCommand, HistoricalBranchRegistration};
use crate::error::HistoricalBranchError;
use crate::{OperationId, Result, State};
use data_serialization::{Deserialize, Serialize};
use name_policy::LookupPath;
use storage_contract::directory_git::HistoricalDirectoryBranchSource;
use storage_contract::{DirectoryBranchId, RepositoryId};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Input {
    repository_id: RepositoryId,
    path: String,
    name: storage_contract::BranchName,
    source: HistoricalDirectoryBranchSource,
}

impl HistoricalBranchCommand<'_> {
    pub(crate) fn digest_body(&self) -> json_codec::Value {
        json_codec::json!({
            "repository_id": self.source.current().repository_id,
            "path": self.path.map_or("", LookupPath::as_str),
            "name": self.name.as_str(),
            "source": self.source,
        })
    }
}

pub(crate) fn receipt(
    command: &HistoricalBranchCommand<'_>,
    registration: HistoricalBranchRegistration<'_>,
) -> json_codec::Value {
    let branch = registration.branch;
    let files = branch
        .entries
        .iter()
        .filter(|entry| entry.kind == storage_contract::NamespaceEntryKind::File)
        .map(|entry| {
            json_codec::json!({
                "file_id": entry.file_id,
                "branch_id": branch.file_branches[&entry.file_id],
                "path": entry.path(),
                "version_id": entry.version_id,
            })
        })
        .collect::<Vec<_>>();
    json_codec::json!({
        "input": command.digest_body(),
        "directory_branch_id": branch.directory_branch_id,
        "history_tree_id": registration.repository.history_tree_id,
        "path": command.path.map_or("", LookupPath::as_str),
        "name": command.name.as_str(),
        "head_tree_id": branch.head_tree_id,
        "count": files.len().to_string(),
        "branches": files,
        "copied_files": "0",
        "placeholder_files": "0",
        "materialized": false,
        "selected": false,
    })
}

pub(crate) fn replay(
    state: &mut State,
    view: Option<&crate::engine::CurrentNamespaces>,
    input: &json_codec::Value,
    operation: OperationId,
) -> Result<json_codec::Value> {
    let input: Input =
        json_codec::from_value(input.clone()).map_err(HistoricalBranchError::Decode)?;
    if input.repository_id.to_string() != input.source.current().repository_id {
        return Err(HistoricalBranchError::InputScope.into());
    }
    let path = if input.path.is_empty() {
        None
    } else {
        Some(LookupPath::parse(&input.path)?)
    };
    let command = HistoricalBranchCommand {
        source: &input.source,
        path: path.as_ref(),
        name: &input.name,
    };
    let registration = command.apply(state, view, operation)?;
    Ok(receipt(&command, registration))
}

/// The registration is ordinal zero in the established directory identity domain.
pub(crate) fn branch_id(operation: OperationId) -> Result<DirectoryBranchId> {
    #[derive(Serialize)]
    struct Identity {
        operation_id: OperationId,
        ordinal: &'static str,
        kind: &'static str,
    }
    let encoded = crate::canonical_json(&Identity {
        operation_id: operation,
        ordinal: "0",
        kind: "dbr",
    })?;
    let mut hash = blake3::Hasher::new();
    hash.update(b"zxs:directory-commit-id\0");
    hash.update(&encoded);
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&hash.finalize().as_bytes()[..16]);
    Ok(DirectoryBranchId::from_random(bytes))
}
