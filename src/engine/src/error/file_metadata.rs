use storage_contract::ErrorCode;

#[derive(Debug, error_definition::Error)]
pub enum FileMetadataError {
    #[error("file or version not found")]
    FileMissing,
    #[error("file is absent from the current namespace")]
    CurrentFileMissing,
    #[error("file branch missing")]
    BranchMissing,
    #[error("merged file aliases are read-only")]
    InactiveBranch,
    #[error("file branch is absent from the current namespace")]
    CurrentBranchMissing,
    #[error("a public branch of this file already uses that name")]
    NameCollision,
    #[error("history tree does not exist")]
    HistoryMissing,
    #[error("history has no repository")]
    RepositoryMissing,
    #[error("file or version identity is invalid: {0}")]
    FileIdentifier(#[source] storage_contract::Error),
    #[error("file branch identity is invalid: {0}")]
    BranchIdentifier(#[source] storage_contract::Error),
    #[error("file metadata publication differs from its original request")]
    Publication,
    #[error("file metadata publication cannot advance its business generation")]
    PublicationGeneration(#[source] storage_contract::GenerationExhausted),
    #[error("file metadata receipt note: {0}")]
    Note(#[source] storage_contract::Error),
    #[error("file metadata receipt encoding: {0}")]
    Encoding(#[source] json_codec::Error),
}

impl FileMetadataError {
    pub(super) fn code(&self) -> ErrorCode {
        match self {
            Self::FileMissing | Self::CurrentFileMissing | Self::FileIdentifier(_) => {
                ErrorCode::FileNotFound
            }
            Self::BranchMissing | Self::CurrentBranchMissing | Self::BranchIdentifier(_) => {
                ErrorCode::FileBranchNotFound
            }
            Self::InactiveBranch => ErrorCode::BranchNotActive,
            Self::NameCollision => ErrorCode::NameCollision,
            Self::HistoryMissing => ErrorCode::HistoryNotFound,
            Self::RepositoryMissing => ErrorCode::StateInvalid,
            Self::Publication
            | Self::PublicationGeneration(_)
            | Self::Note(_)
            | Self::Encoding(_) => {
                ErrorCode::PublicationInvalid
            }
        }
    }
}
