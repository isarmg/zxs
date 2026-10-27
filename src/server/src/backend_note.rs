//! The three existing mutable note targets, independent of immutable content nodes.
#[derive(Debug)]
pub enum NoteTarget {
    File(String),
    Version(String),
    Node { file: String, node: String },
}
pub struct UpdateNote {
    pub target: NoteTarget,
    pub note: storage_contract::Note,
    pub operation_id: storage_contract::OperationId,
}
