//! Exact single-file movement within one ordinary primary-disk namespace.
use crate::history_window::{HistoryCommitId, HistoryFileId, HistoryRepositoryId};
use data_serialization::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serialization(transparent)]
pub struct NamespaceEntryId(String);
impl NamespaceEntryId {
    pub fn parse(value: &str) -> crate::Result<Self> {
        crate::validate_id(value, "entry")?;
        Ok(Self(value.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl<'de> Deserialize<'de> for NamespaceEntryId {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        let value = String::deserialize(decoder)?;
        Self::parse(&value).map_err(data_serialization::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct FileRelocation {
    pub repository_id: HistoryRepositoryId,
    pub expected_head: HistoryCommitId,
    pub entry_id: NamespaceEntryId,
    pub file_id: HistoryFileId,
    pub source: name_policy::PortablePath,
    pub destination: name_policy::PortablePath,
}
impl FileRelocation {
    pub fn change(&self) -> crate::NamespaceOperation {
        crate::NamespaceOperation::Rename {
            source: name_policy::LookupPath::from(&self.source),
            destination: self.destination.clone(),
        }
    }
    /// Shared by browser recovery, publication and authenticated cold replay.
    pub fn request(&self) -> json_codec::Value {
        json_codec::json!({
            "repository_id":self.repository_id,
            "change":self.change(),
            "original_file":self,
        })
    }
}
