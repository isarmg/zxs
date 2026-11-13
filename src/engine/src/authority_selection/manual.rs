//! Typed failures at the container-edit storage boundary.
use crate::ErrorCode;

#[derive(Debug, error_definition::Error)]
pub enum ManualFormatError {
    #[error("manual format filesystem admission failed")]
    Filesystem(#[source] platform_io::fs_safety::Error),
    #[error("manual format metadata decoding failed")]
    Codec(#[source] storage_contract::Error),
    #[error("manual format requires independent primary level-one roots")]
    Topology,
    #[error("manual format requires empty roots; existing material was preserved")]
    Occupied,
    #[error("manual format original configuration, operation or root identity changed")]
    Changed,
    #[error("manual format metadata is unsupported, noncanonical or exceeds its budget")]
    Format,
    #[error("manual format initialization is incomplete")]
    Incomplete,
    #[error("this reader cannot admit manual container storage")]
    UnsupportedReader,
}
impl ManualFormatError {
    pub(crate) fn code(&self) -> ErrorCode {
        match self {
            Self::Filesystem(_) | Self::Occupied | Self::Incomplete => ErrorCode::RecoveryRequired,
            Self::Changed => ErrorCode::PublicationInvalid,
            Self::Topology => ErrorCode::UnsupportedOperation,
            Self::Codec(_) | Self::Format | Self::UnsupportedReader => ErrorCode::FormatUnsupported,
        }
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/manual_format_tests.rs"]
mod tests;
