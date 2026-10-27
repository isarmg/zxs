//! Purge preparation retains history admission before selection decoding errors.
pub struct PreparePurge {
    pub history_tree_id: String,
    pub version_ids: Result<Vec<String>, crate::ApiError>,
}
