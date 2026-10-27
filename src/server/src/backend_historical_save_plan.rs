//! Parsed preparation input with field errors delayed until history admission.
pub struct PrepareHistoricalSave {
    pub history_tree_id: String,
    // A missing history must retain precedence over malformed save options.
    pub options: Result<storage_contract::HistoricalSaveOptions, crate::ApiError>,
}
