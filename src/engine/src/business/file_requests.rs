//! Canonical metadata mutation requests, including optional namespace context.
use crate::{Engine, Error, FileMergeInput, Result};
use json_codec::{Value, json};
use storage_contract::{FileLocation, NamespaceEntryKind, OperationId};

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

pub struct FileHistoryMergeRequest<'a> {
    pub file_id: &'a str,
    pub inputs: &'a [storage_contract::FileHistoryMergeInput],
    pub chosen_node_id: &'a str,
    pub name: &'a storage_contract::BranchName,
    pub location: Option<&'a FileLocation>,
    pub principal: &'a str,
    pub operation_id: OperationId,
}

impl FileHistoryMergeRequest<'_> {
    pub(super) fn body(&self) -> Value {
        json!({"file_id":self.file_id,"inputs":self.inputs,"chosen_node_id":self.chosen_node_id,
            "name":self.name.as_str(),"location":self.location})
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
    let path = name_policy::LookupPath::from(name_policy::PortablePath::parse(&location.path)?);
    let object = match engine.state().repositories.get(&location.repository_id) {
        Some(repository) => engine.locate_object(&repository.main_branch_id, &path)?,
        None => None,
    };
    if !object.is_some_and(|object| {
        object.entry().kind == NamespaceEntryKind::File && object.entry().file_id == file
    }) {
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

pub struct FileDefaultSelection<'a> {
    pub file_id: &'a str,
    pub branch_id: &'a str,
    pub repository_id: &'a str,
    pub path: &'a name_policy::PortablePath,
    pub expected_node_id: &'a str,
    pub expected_head: &'a str,
    pub principal: &'a str,
    pub operation_id: OperationId,
}
