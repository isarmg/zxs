//! Validated public branch labels and their fixed-Unicode collision keys.
use crate::{Error, ErrorCode, Result};
use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};

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

impl Serialize for BranchName {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for BranchName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(data_serialization::de::Error::custom)
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
