//! Publication-bound foreign history previews and independent offline capsules.
//! Neither admits a disk, rewrites an old seal, or authorizes an import commit.
use crate::{disk::*, journal::*, *};
use std::{collections::BTreeMap, fs::File, path::PathBuf};

mod identity_closure;
mod inspect;
pub use identity_closure::{
    ForeignIdentityBaseline, ForeignIdentityClosureReport, ForeignIdentityTarget,
    ForeignIdentityTargetRequest,
};
mod intent;
pub use intent::{
    ConfirmedForeignImportIntent, ForeignImportIntentInfo, ForeignImportIntentRequest,
    ForeignImportIntentSource, ForeignImportIntentTarget, PreparedForeignImportIntent,
};
mod mapping;
mod migration;
mod model;
pub use migration::{
    ForeignDatabaseStatus, ForeignIdentityClosureStatus, ForeignMigrationPlan,
    ForeignMigrationReceipt, ForeignMigrationStatus, ForeignMigrationTransaction,
    ForeignOriginalDatabasesReport,
};
pub use model::{
    ForeignCheckpoint, ForeignEvidenceDigest, ForeignHistory, ForeignImportLimits,
    ForeignImportMapping, ForeignImportPreview, ForeignLocalBinding, ForeignMappedHistory,
    ForeignOriginalStateReport, ForeignPrimaryOrigin, ForeignSourceIdentity,
    ForeignVersionAvailability,
};

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::HistoryPackageInvalid, message)
}

#[cfg(test)]
#[path = "../../../test/engine/unit/foreign_import/tests.rs"]
mod tests;
