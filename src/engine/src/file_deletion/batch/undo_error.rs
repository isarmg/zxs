//! Batch restoration errors and the authenticated actor boundary.
use crate::{ErrorCode, OperationId};

#[derive(Debug, error_definition::Error)]
pub enum BatchUndoError {
    #[error("authenticated actor is empty")]
    EmptyActor,
    #[error("original deletion operation {0} is missing")]
    OperationMissing(OperationId),
    #[error("operation {0} is not a file deletion batch")]
    NotDeletionBatch(OperationId),
    #[error("an original batch member changed or was already restored")]
    MemberChanged,
    #[error("complete batch restoration exceeds its response budget")]
    ResponseBudget,
    #[error("batch restoration receipt differs from its original deletion")]
    InvalidReceipt,
    #[error("encoding batch restoration record")]
    Encoding(#[source] json_codec::Error),
}
impl BatchUndoError {
    pub(crate) fn code(&self) -> ErrorCode {
        match self {
            Self::EmptyActor => ErrorCode::Unauthenticated,
            Self::NotDeletionBatch(_) => ErrorCode::InvalidRequest,
            Self::OperationMissing(_) => ErrorCode::OperationNotFound,
            Self::MemberChanged => ErrorCode::FileNodeNotFound,
            Self::ResponseBudget => ErrorCode::ResourceLimit,
            Self::InvalidReceipt => ErrorCode::PublicationInvalid,
            Self::Encoding(_) => ErrorCode::InvalidRequest,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct BatchUndoActor<'a>(&'a str);
impl<'a> BatchUndoActor<'a> {
    pub fn parse(value: &'a str) -> std::result::Result<Self, BatchUndoError> {
        if value.is_empty() {
            Err(BatchUndoError::EmptyActor)
        } else {
            Ok(Self(value))
        }
    }
    pub(crate) fn as_str(self) -> &'a str {
        self.0
    }
}
