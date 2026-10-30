//! Canonical metadata mutation requests, including optional namespace context.
use crate::{Engine, Error, FileMergeInput, FileTagTarget, Result};
use json_codec::{Value, json};
use storage_contract::{FileLocation, OperationId};

pub struct FileNodeTagsUpdateRequest<'a> {
    pub file_id: &'a str,
    pub targets: &'a [FileTagTarget],
    pub add: &'a [String],
    pub remove: &'a [String],
    pub location: Option<&'a FileLocation>,
    pub principal: &'a str,
    pub operation_id: OperationId,
}

impl FileNodeTagsUpdateRequest<'_> {
    pub(super) fn body(&self) -> Value {
        // Explicit null binds the absence of context too. A receipt produced
        // before location was bound must never match by dropping its location.
        json!({"file_id":self.file_id,"targets":self.targets,"add":self.add,
            "remove":self.remove,"location":self.location})
    }
}

pub struct FileBranchMergeRequest<'a> {
    pub file_id: &'a str,
    pub inputs: &'a [FileMergeInput],
    pub chosen_node_id: &'a str,
    pub surviving_branch_id: &'a str,
    pub location: Option<&'a FileLocation>,
    pub principal: &'a str,
    pub operation_id: OperationId,
}

impl FileBranchMergeRequest<'_> {
    pub(super) fn body(&self) -> Value {
        json!({"file_id":self.file_id,"inputs":self.inputs,"chosen_node_id":self.chosen_node_id,
            "surviving_branch_id":self.surviving_branch_id,"location":self.location})
    }
}

/// Only new mutations check the current path. Exact replay is resolved first
/// by mutate using the complete request body and immutable operation actor.
pub(super) fn validate_location(
    engine: &Engine,
    file: &str,
    location: Option<&FileLocation>,
) -> Result<()> {
    let Some(location) = location else {
        return Ok(());
    };
    name_policy::validate_path(&location.path)?;
    let version = engine
        .state()
        .versions
        .values()
        .find(|version| version.file_id == file)
        .ok_or_else(|| Error::new(storage_contract::ErrorCode::FileNotFound, "file not found"))?;
    let repo = engine.repository_for_history(&version.history_tree_id)?;
    let key = name_policy::path_key(&location.path);
    if location.repository_id != repo.repository_id
        || !engine
            .namespace_tree(&repo.main_branch_id)?
            .contains_file_key(&key, file)?
    {
        return Err(Error::new(
            storage_contract::ErrorCode::FileNotFound,
            "file is not present at this logical path",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/file_requests/tests.rs"]
mod tests;
