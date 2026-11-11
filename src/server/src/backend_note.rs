//! The three existing mutable note targets, independent of immutable content nodes.
#[derive(Debug)]
pub enum NoteTarget {
    File(FileNoteTarget),
    Version(VersionNoteTarget),
    Node { file: String, node: String },
}

/// File lookup failures are resolved after current-credential admission.
#[derive(Debug)]
pub struct FileNoteTarget(Result<storage_contract::FileId, storage_contract::Error>);

impl From<storage_contract::FileId> for FileNoteTarget {
    fn from(id: storage_contract::FileId) -> Self {
        Self(Ok(id))
    }
}

impl FileNoteTarget {
    pub(crate) fn malformed(source: storage_contract::Error) -> Self {
        Self(Err(source))
    }

    pub fn into_id(self) -> Result<storage_contract::FileId, storage_contract::Error> {
        self.0
    }
}

/// Version lookup failures are resolved after current-credential admission.
#[derive(Debug)]
pub struct VersionNoteTarget(Result<storage_contract::VersionId, storage_contract::Error>);

impl From<storage_contract::VersionId> for VersionNoteTarget {
    fn from(id: storage_contract::VersionId) -> Self {
        Self(Ok(id))
    }
}

impl VersionNoteTarget {
    pub(crate) fn malformed(source: storage_contract::Error) -> Self {
        Self(Err(source))
    }

    pub fn into_id(self) -> Result<storage_contract::VersionId, storage_contract::Error> {
        self.0
    }
}

pub struct UpdateNote {
    pub target: NoteTarget,
    pub note: storage_contract::Note,
    pub operation_id: storage_contract::OperationId,
}
