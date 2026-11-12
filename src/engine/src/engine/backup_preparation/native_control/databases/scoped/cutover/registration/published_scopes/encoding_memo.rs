//! Published authority bindings for request-local pure SQLite bytes.
use data_serialization::Serialize;
use std::sync::atomic::AtomicBool;
use crate::Result;
use crate::engine::backup_preparation::native_control::databases::scoped::encoding_memo
    as common_encoding;
pub(in super::super) use common_encoding::{ScopeEncodingMemo, Session, digest};

#[derive(Serialize)]
pub(super) struct Context<'a> {
    pub(super) target: &'a crate::DiskRegistrationIdentity,
    pub(super) device: u64,
    pub(super) inode: u64,
    pub(super) operation: crate::OperationId,
    pub(super) source_fence: &'a str,
    pub(super) archive: &'a str,
    pub(super) candidate: &'a str,
    pub(super) decision: &'a str,
    pub(super) publication: &'a str,
    pub(super) authority: &'a str,
    pub(super) snapshot: &'a str,
    pub(super) first_sequence: u64,
    pub(super) registry: &'a crate::DiskRegistryCommitment,
    pub(super) configuration: &'a str,
}

impl Context<'_> {
    pub(super) fn digest(&self, stop: &AtomicBool) -> Result<Option<[u8; 32]>> {
        digest(&("zxs-backup-scoped-sqlite-encoding-v1", self), stop)
    }
}


#[cfg(test)]
#[path =
    "../../../../../../../../../../../test/engine/unit/backup_published_scope_context_tests.rs"
]
mod tests;
