//! Complete nonsecret SYSTEM projection. Observe rows only after their native
//! SQL check. O(one canonical row) temporary memory, not a dump of State.
use crate::Result;
use data_serialization::Serialize;
use storage_contract::{
    CanonicalArrayDigest, ContentHash, Sequence, working_tree::MaterialCommitment,
};

pub(in crate::engine::index) struct LogicalRows {
    versions: CanonicalArrayDigest,
    members: CanonicalArrayDigest,
    changes: CanonicalArrayDigest,
}
impl LogicalRows {
    pub(super) fn new() -> Result<Self> {
        Ok(Self {
            versions: CanonicalArrayDigest::new("system-history-versions")?,
            members: CanonicalArrayDigest::new("system-scope-members")?,
            changes: CanonicalArrayDigest::new("system-business-changes")?,
        })
    }
    pub(in crate::engine::index) fn version(
        &mut self,
        version: &str,
        file: &str,
        history: &str,
        sequence: u64,
    ) -> Result<()> {
        self.versions
            .push(&(version, file, history, Sequence::new(sequence)))?;
        Ok(())
    }
    pub(in crate::engine::index) fn member(
        &mut self,
        kind: usize,
        id: &str,
        position: &str,
        file: &str,
        history: &str,
    ) -> Result<()> {
        self.members
            .push(&(kind as u32, id, position, file, history))?;
        Ok(())
    }
    pub(in crate::engine::index) fn change(
        &mut self,
        kind: i64,
        id: &str,
        sequence: u64,
        canonical_payload: &[u8],
    ) -> Result<()> {
        // These are the exact typed canonical bytes read back by the native
        // verifier. Do not copy any raw State or control_auth map into a row.
        self.changes.push(&(
            kind,
            id,
            Sequence::new(sequence),
            ContentHash::from_digest(blake3::hash(canonical_payload)),
        ))?;
        Ok(())
    }
    pub(super) fn finish(self, sequence: u64) -> Result<MaterialCommitment> {
        #[derive(Serialize)]
        struct Scopes {
            sequence: Sequence,
            history_versions: MaterialCommitment,
            scope_members: MaterialCommitment,
            system_changes: MaterialCommitment,
        }
        let value = Scopes {
            sequence: Sequence::new(sequence),
            history_versions: MaterialCommitment::from_digest(self.versions.finish()),
            scope_members: MaterialCommitment::from_digest(self.members.finish()),
            system_changes: MaterialCommitment::from_digest(self.changes.finish()),
        };
        Ok(MaterialCommitment::from_digest(
            storage_contract::canonical_object_digest("system-scope-rows", &value)?,
        ))
    }
}
