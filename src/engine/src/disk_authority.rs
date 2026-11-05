//! Independent, explicitly initialized per-disk authority. This module does not
//! read the managed-directory format or participate in the current pool quorum.
//! The offline rescue consumer is separate from Engine's online namespace.
mod cache;
mod codec;
mod fault;
mod journal;
mod location;
mod model;
mod projection;
mod storage;
mod verify;

#[cfg(test)]
#[path = "../../../test/engine/unit/disk_authority/authority_selection.rs"]
mod authority_selection_tests;

pub use cache::CachedCopy;
pub use journal::{
    DiskReport, DiskSelection, FileWrite, IndependentDisk, OperationStatus, ReviewedPublication,
};
use model::{
    Authority, Body, Bundle, Commit, Format, MAX_BYTES, MAX_OBJECTS, MAX_PUBLICATIONS, Manifest,
    PreparedEvidence, Publication, PublicationIntent, SourceCheckpoint,
};
pub use model::{Digest, DiskPublicationReceipt, Identity, Origin, Role, Token};
pub use projection::rebuild_projection;
use storage::{Disk, OperationEvidence, PublicationRequest};
use verify::VerifiedDisk;
pub use verify::{Availability, CopyRow};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, error_definition::Error)]
pub enum Error {
    #[error("engine filesystem boundary: {0}")]
    EngineBoundary(#[source] crate::Error),
    #[error("filesystem: {0}")]
    Io(#[from] std::io::Error),
    #[error("safe filesystem: {0}")]
    Filesystem(#[from] platform_io::fs_safety::Error),
    #[error("JSON boundary: {0}")]
    Json(#[from] json_codec::Error),
    #[error("SQLite: {0}")]
    Sqlite(#[from] sqlite_access::Error),
    #[error(
        "cache operation failed ({operation}); closing its SQLite worker also failed ({close})"
    )]
    CacheOperationAndClose {
        #[source]
        operation: Box<Error>,
        close: sqlite_access::Error,
    },
    #[error("invalid protocol: {0}")]
    Invalid(&'static str),
    #[error("disk identity or binding differs from the selected identity")]
    Identity,
    #[error("required metadata object is absent: {0}")]
    Missing(Digest),
    #[error("published content is temporarily unavailable: {0}")]
    ContentUnavailable(Digest),
    #[error("body promised by this publication is missing: {0}")]
    RequiredBodyMissing(Digest),
    #[error("immutable bytes differ at {0}")]
    Corrupt(String),
    #[error("operation identifier was already bound to different input")]
    OperationConflict,
    #[error("publication base changed; a new review is required")]
    StaleBase,
    #[error("independent disk inventory exceeded its limit")]
    Budget,
}

#[cfg(test)]
#[path = "../../../test/engine/unit/disk_authority/journal.rs"]
mod journal_tests;
#[cfg(test)]
#[path = "../../../test/engine/unit/disk_authority/protocol.rs"]
mod protocol_tests;
