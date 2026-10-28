//! Admit the file-history branch projection once; other history payloads stay unchanged.
use crate::api;
use json_codec::Value;
use std::{collections::BTreeSet, ops::Deref};

mod wire;

#[derive(Debug, error_definition::Error)]
pub enum DecodeError {
    #[error("invalid file-history branch response: {0}")]
    Wire(#[source] json_codec::Error),
    #[error("invalid file-history branch name: {0}")]
    Name(#[source] storage_contract::Error),
    #[error("inconsistent file-history branch response: {0}")]
    Relation(&'static str),
}
impl From<DecodeError> for api::Error {
    fn from(source: DecodeError) -> Self {
        Self {
            status: 200,
            code: storage_contract::ErrorCode::InvalidResponse,
            message: "The file-history branches did not match the server contract.".into(),
            retryable: false,
            request_id: None,
            source: Some(std::sync::Arc::new(api::ErrorSource::FileHistory(source))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileBranch {
    id: String,
    name: String,
    head: Option<String>,
    pending: bool,
    writable: bool,
}
impl FileBranch {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn head(&self) -> Option<&str> {
        self.head.as_deref()
    }
    pub(crate) fn pending(&self) -> bool {
        self.pending
    }
    pub(crate) fn writable(&self) -> bool {
        self.writable
    }
}

/// Default is an unloaded history, not an accepted empty wire response.
#[derive(Clone, Debug, Default)]
pub(crate) struct FileHistory {
    metadata: Value,
    branches: Vec<FileBranch>,
    selected: Option<String>,
}
impl FileHistory {
    pub(crate) fn parse(mut value: Value, file: &str) -> Result<Self, api::Error> {
        let fields = value
            .as_object_mut()
            .ok_or(DecodeError::Relation("file history must be an object"))?;
        if fields.get("file_id").and_then(Value::as_str) != Some(file) {
            return Err(
                DecodeError::Relation("file history differs from the requested file").into(),
            );
        }
        let selected = fields
            .remove("selected_branch_id")
            .ok_or(DecodeError::Relation("selected_branch_id is required"))?;
        let selected = wire::selected(selected)?;
        let Some(Value::Array(values)) = fields.remove("branches") else {
            return Err(DecodeError::Relation("branches must be an array").into());
        };
        let mut branches = Vec::with_capacity(values.len());
        let mut ids = BTreeSet::new();
        for value in values {
            let branch = wire::branch(value, file, selected.as_deref())?;
            if !ids.insert(branch.id.clone()) {
                return Err(DecodeError::Relation("branches repeat an identity").into());
            }
            branches.push(branch);
        }
        Ok(Self {
            metadata: value,
            branches,
            selected,
        })
    }
    pub(crate) fn branches(&self) -> &[FileBranch] {
        &self.branches
    }
    pub(crate) fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }
    pub(crate) fn for_entry(mut self, entry: &super::page::Entry) -> Self {
        self.metadata = super::model::history_for_entry(self.metadata, entry);
        self
    }
}
// Existing node/version consumers cannot mutate or re-interpret branch records.
impl Deref for FileHistory {
    type Target = Value;
    fn deref(&self) -> &Value {
        &self.metadata
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/history/tests.rs"]
pub(crate) mod tests;
