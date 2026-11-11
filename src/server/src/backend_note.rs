//! The three existing mutable note targets, independent of immutable content nodes.
#[derive(Debug)]
pub enum NoteTarget {
    File(FileNoteTarget),
    Version(VersionNoteTarget),
    Node { file: FileNoteTarget, node: NodeNoteTarget },
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


/// Only path and metadata nodes support this endpoint's mutable notes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoteNodeId {
    Anchor(storage_contract::FileAnchorId),
    Metadata(storage_contract::FileMetadataId),
}

impl std::str::FromStr for NoteNodeId {
    type Err = storage_contract::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.split_once('_') {
            Some(("fnode", _)) => value.parse().map(Self::Anchor),
            Some(("fmeta", _)) => value.parse().map(Self::Metadata),
            _ => Err(storage_contract::Error::InvalidIdentifier {
                prefix: "note node",
                retryable: false,
            }),
        }
    }
}

impl std::fmt::Display for NoteNodeId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Anchor(id) => std::fmt::Display::fmt(id, formatter),
            Self::Metadata(id) => std::fmt::Display::fmt(id, formatter),
        }
    }
}

/// Unknown node text remains part of the request digest before lookup can fail.
#[derive(Debug)]
pub struct NodeNoteTarget(NodeNoteInput);

#[derive(Debug)]
enum NodeNoteInput {
    Canonical(NoteNodeId),
    Malformed {
        original: String,
        source: storage_contract::Error,
    },
}

impl From<NoteNodeId> for NodeNoteTarget {
    fn from(id: NoteNodeId) -> Self {
        Self(NodeNoteInput::Canonical(id))
    }
}

impl NodeNoteTarget {
    pub(crate) fn from_wire(original: String) -> Self {
        Self(match original.parse() {
            Ok(id) => NodeNoteInput::Canonical(id),
            Err(source) => NodeNoteInput::Malformed { original, source },
        })
    }

    pub fn parsed_id(&self) -> Result<NoteNodeId, &storage_contract::Error> {
        match &self.0 {
            NodeNoteInput::Canonical(id) => Ok(*id),
            NodeNoteInput::Malformed { source, .. } => Err(source),
        }
    }

    // The engine still admits receipts and write fences before resolving nodes.
    // Retain this spelling for that digest bridge, not as a trusted node identity.
    pub(crate) fn request_text(&self) -> std::borrow::Cow<'_, str> {
        match &self.0 {
            NodeNoteInput::Canonical(id) => std::borrow::Cow::Owned(id.to_string()),
            NodeNoteInput::Malformed { original, .. } => std::borrow::Cow::Borrowed(original),
        }
    }
}

pub struct UpdateNote {
    pub target: NoteTarget,
    pub note: storage_contract::Note,
    pub operation_id: storage_contract::OperationId,
}
