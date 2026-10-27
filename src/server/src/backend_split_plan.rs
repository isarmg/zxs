//! Split preparation retains history admission before selection decoding errors.
pub struct PrepareSplit {
    pub history_tree_id: String,
    pub cut_commit_id: Result<String, crate::ApiError>,
}
