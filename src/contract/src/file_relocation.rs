//! Captured movement intent shared by the engine, Web and SMB adapters.
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

pub const OBJECT_MOVE_KIND: &str = "object.move";

/// The caller chooses one target disk and location; the engine determines the
/// complete file or directory-repository unit from the current saved contents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ObjectMoveRequest {
    pub source_repository_id: HistoryRepositoryId,
    pub source_entry_id: NamespaceEntryId,
    pub source_path: name_policy::LookupPath,
    pub target_slot_id: String,
    #[serialization(deserialize_with = "required_repository")]
    pub target_repository_id: Option<HistoryRepositoryId>,
    pub target_path: name_policy::PortablePath,
    pub replace: bool,
    #[serialization(deserialize_with = "required_target_entry")]
    pub target_entry_id: Option<NamespaceEntryId>,
}

fn required_repository<'de, D: Deserializer<'de>>(
    decoder: D,
) -> Result<Option<HistoryRepositoryId>, D::Error> {
    Option::<HistoryRepositoryId>::deserialize(decoder)
}

fn required_target_entry<'de, D: Deserializer<'de>>(
    decoder: D,
) -> Result<Option<NamespaceEntryId>, D::Error> {
    Option::<NamespaceEntryId>::deserialize(decoder)
}

fn required_file<'de, D: Deserializer<'de>>(
    decoder: D,
) -> Result<Option<HistoryFileId>, D::Error> {
    Option::<HistoryFileId>::deserialize(decoder)
}

impl ObjectMoveRequest {
    pub fn require_target_intent(&self) -> crate::Result<()> {
        let same_object = self.target_repository_id.as_ref() == Some(&self.source_repository_id)
            && self.target_entry_id.as_ref() == Some(&self.source_entry_id)
            && self.target_path.key() == self.source_path.key();
        if self.target_slot_id.is_empty()
            || (!self.replace && self.target_entry_id.is_some() && !same_object)
            || (self.target_repository_id.is_none() && self.target_entry_id.is_some())
        {
            return Err(crate::Error::new(
                crate::ErrorCode::InvalidRequest,
                "move requires a target slot and an explicit occupied-target intent",
            ));
        }
        Ok(())
    }

    /// Use the same domain request digest as Engine::native_request_digest.
    pub fn digest(&self, principal: &str) -> crate::Result<String> {
        crate::object_hash("request", &json_codec::json!({
            "kind": OBJECT_MOVE_KIND, "principal": principal, "body": self,
        }))
    }

    /// Match original publication facts, never today's source path or owner.
    /// The caller separately verifies operation ID and publication evidence.
    pub fn matches_receipt(&self, principal: &str, receipt: &json_codec::Value) -> bool {
        let Ok(digest) = self.digest(principal) else {
            return false;
        };
        let Ok(fact) = json_codec::from_value::<ObjectMoveFact>(receipt["result"]["movement"].clone()) else {
            return false;
        };
        receipt["operation_kind"] == OBJECT_MOVE_KIND
            && receipt["request_digest"] == digest
            && fact.request == *self
            && fact.principal == principal
            && self.target_repository_id.as_ref()
                .is_none_or(|repository| repository == &fact.target_repository_id)
    }
}

/// Stored once in the ordinary operation result. Empty-disk creation records
/// its actual root here while the request keeps its original None intention.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ObjectMoveFact {
    pub request: ObjectMoveRequest,
    pub principal: String,
    pub target_repository_id: HistoryRepositoryId,
    pub target_entry_id: NamespaceEntryId,
    #[serialization(deserialize_with = "required_file")]
    pub source_file_id: Option<HistoryFileId>,
    #[serialization(deserialize_with = "required_file")]
    pub displaced_target_file_id: Option<HistoryFileId>,
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
