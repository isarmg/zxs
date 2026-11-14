//! Physical edit leaves preserve their wire spelling without charging the prefix
//! to the user-name limit. A parsed prefix never identifies an authoritative file.
use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};
use name_policy::LookupName;
use platform_io::fs_safety::EntryName;
use storage_contract::VersionStorageId;

#[derive(Debug, error_definition::Error)]
pub(crate) enum EditLeafError {
    #[error("worktree edit leaf lacks its fixed storage prefix")]
    Shape,
    #[error("worktree edit leaf has an invalid storage prefix")]
    Prefix(#[source] storage_contract::InvalidVersionStorageId),
    #[error("worktree edit leaf has an invalid user name")]
    UserName(#[source] name_policy::Error),
    #[error("worktree edit leaf is not one physical directory entry")]
    Entry(#[source] platform_io::fs_safety::Error),
}

/// A physical component, not a user name or an identity authority. The private
/// entry is constructed from UTF-8 and retains its exact wire spelling.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EditLeaf(EntryName);
impl EditLeaf {
    pub(crate) fn parse(spelling: &str) -> Result<Self, EditLeafError> {
        let (prefix, name) = spelling.split_once('-').ok_or(EditLeafError::Shape)?;
        prefix.parse::<VersionStorageId>().map_err(EditLeafError::Prefix)?;
        LookupName::parse(name).map_err(EditLeafError::UserName)?;
        EntryName::new(spelling).map(Self).map_err(EditLeafError::Entry)
    }
    pub(crate) fn as_str(&self) -> &str {
        self.0.as_os_str().to_str()
            .expect("invariant: edit leaves are constructed from UTF-8 wire strings")
    }
    pub(crate) fn key(&self) -> String {
        name_policy::lookup_key(self.as_str())
    }
    pub(crate) fn entry(&self) -> &EntryName {
        &self.0
    }
}
impl Serialize for EditLeaf {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
impl<'de> Deserialize<'de> for EditLeaf {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let spelling = String::deserialize(deserializer)?;
        Self::parse(&spelling).map_err(data_serialization::de::Error::custom)
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/directory_worktree/edit_leaf.rs"]
mod tests;
