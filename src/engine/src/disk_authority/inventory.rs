//! Metadata references retained by one captured disk, independent of any
//! physically present source, downstream disk, or system-disk cache.
use super::{CopyRow, Digest, Identity, SourceTopology, Token, VerifiedDisk};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainedVersion {
    owner: Identity,
    history_id: Token,
    file_id: Token,
    commit: Digest,
    body: Digest,
}
impl RetainedVersion {
    pub(super) fn capture(row: &CopyRow) -> Self {
        Self {
            owner: row.owner.clone(),
            history_id: row.history_id.clone(),
            file_id: row.file_id.clone(),
            commit: row.commit,
            body: row.body,
        }
    }
    pub fn owner(&self) -> &Identity {
        &self.owner
    }
    pub fn history_id(&self) -> &Token {
        &self.history_id
    }
    pub fn file_id(&self) -> &Token {
        &self.file_id
    }
    pub fn commit(&self) -> Digest {
        self.commit
    }
    pub fn body(&self) -> Digest {
        self.body
    }
}

#[derive(Clone, Debug)]
pub struct SourceInventory {
    topology: SourceTopology,
    versions: Vec<RetainedVersion>,
}
impl SourceInventory {
    pub(super) fn capture(verified: &VerifiedDisk, topology: SourceTopology) -> Self {
        Self {
            topology,
            versions: verified.rows.iter().map(RetainedVersion::capture).collect(),
        }
    }
    pub fn topology(&self) -> &SourceTopology {
        &self.topology
    }
    pub fn versions(&self) -> &[RetainedVersion] {
        &self.versions
    }
}
