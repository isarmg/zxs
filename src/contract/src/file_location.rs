use data_serialization::Serialize;

/// Current namespace context is a mutation precondition, not stored graph data.
#[derive(Debug, Clone, Serialize)]
pub struct FileLocation {
    pub repository_id: String,
    pub path: String,
}
