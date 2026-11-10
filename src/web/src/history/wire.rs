//! Current server backend_activity response, kept private to the browser boundary.
//! The server owns this DTO and is not a permitted WASM dependency.
use data_serialization::{Deserialize, Deserializer};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Response {
    pub events: Vec<Event>,
    pub limit: usize,
}

#[derive(Deserialize)]
#[expect(
    dead_code,
    reason = "Validate the complete wire record before projecting display fields"
)]
pub(super) struct Event {
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    event_id: String,
    #[serialization(deserialize_with = "nullable")]
    pub timestamp: Option<String>,
    pub slot_id: String,
    pub path: String,
    pub action: String,
    #[serialization(deserialize_with = "nullable")]
    pub principal: Option<String>,
    file_id: String,
    message: String,
    #[serialization(flatten)]
    detail: Detail,
}

// Optional values are always serialized by the server, including explicit null.
fn nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

#[derive(Deserialize)]
#[serialization(untagged, deny_unknown_fields)]
#[expect(
    dead_code,
    reason = "Activity details are validated but not rendered in the activity list"
)]
enum Detail {
    Plain {
        #[serialization(deserialize_with = "nullable")]
        version_id: Option<String>,
    },
    Content {
        #[serialization(deserialize_with = "nullable")]
        version_id: Option<String>,
        materialized: bool,
        inherited: bool,
        #[serialization(deserialize_with = "nullable")]
        own_version_id: Option<String>,
        #[serialization(deserialize_with = "nullable")]
        source_version_id: Option<String>,
        content_state: ContentState,
        visible_content_state: ContentState,
    },
    FileOperation {
        #[serialization(deserialize_with = "nullable")]
        version_id: Option<String>,
        #[serialization(deserialize_with = "nullable")]
        node_id: Option<String>,
        #[serialization(deserialize_with = "nullable")]
        branch_id: Option<String>,
    },
    DirectoryOperation {
        #[serialization(deserialize_with = "nullable")]
        version_id: Option<String>,
        directory_branch_id: String,
        #[serialization(deserialize_with = "nullable")]
        head_tree_id: Option<String>,
    },
    Backup {
        #[serialization(deserialize_with = "nullable")]
        version_id: Option<String>,
        #[serialization(deserialize_with = "nullable")]
        target_copies: Option<u64>,
    },
    Path {
        version_id: String,
        node_id: String,
        previous_path: String,
        branch_id: String,
    },
    Tree {
        #[serialization(deserialize_with = "nullable")]
        version_id: Option<String>,
        directory_branch_id: String,
        head_tree_id: String,
        #[serialization(deserialize_with = "nullable")]
        parent_tree_id: Option<String>,
    },
}

#[derive(Deserialize)]
#[serialization(rename_all = "SCREAMING_SNAKE_CASE")]
enum ContentState {
    Live,
    Purged,
    Available,
    Migrated,
    Pending,
}
