//! Validated public branch labels and their fixed-Unicode collision keys.
use crate::{Error, ErrorCode, Result};

#[derive(Debug, PartialEq, Eq)]
pub struct BranchName {
    display: String,
    key: String,
}
impl BranchName {
    /// Decode an external creation label without changing its NFC display spelling.
    pub fn parse(name: &str) -> Result<Self> {
        validate_branch_name(name)?;
        Ok(Self {
            display: name.into(),
            key: name_policy::lookup_key(name),
        })
    }
    pub fn as_str(&self) -> &str {
        &self.display
    }
    pub fn key(&self) -> &str {
        &self.key
    }
}
/// Validate raw persisted labels and the public entrypoints not yet carrying BranchName.
/// This shares the policy without constructing an owned label solely for validation.
pub fn validate_branch_name(name: &str) -> Result<()> {
    if name.len() > 128 {
        return Err(Error::new(
            ErrorCode::MetadataTooLong,
            "branch names must not exceed 128 UTF-8 bytes",
        ));
    }
    name_policy::validate_name(name).map_err(Into::into)
}

#[cfg(test)]
#[path = "../../../test/contract/unit/branch_name.rs"]
mod tests;
