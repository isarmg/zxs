//! Historical save preparation input; dynamic scope and placement checks remain in the engine.
use data_serialization::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoricalSaveOptions {
    pub source_commit_id: String,
    pub name: String,
    pub workspace_id: Option<String>,
    pub primary_slot_id: Option<String>,
    pub target_copies: Option<u32>,
}
