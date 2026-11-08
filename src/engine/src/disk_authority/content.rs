//! Bounded reads of independently named FULL copies from one captured proof.
//! The snapshot has no writer, registration or publication capability.
use super::{
    Digest, Error, Identity, MAX_BYTES, Result, RetainedVersion, VerifiedDisk, codec,
    model::Object, storage::Disk, verify::ReadStore,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct PublishedContent {
    version: RetainedVersion,
    expected_bytes: u64,
}
impl PublishedContent {
    pub fn version(&self) -> &RetainedVersion {
        &self.version
    }
    pub fn expected_bytes(&self) -> u64 {
        self.expected_bytes
    }
}

pub struct ContentSnapshot<'a> {
    disk: &'a Disk,
    identity: Identity,
    checkpoint: Option<Digest>,
    sequence: u64,
    versions: BTreeMap<Digest, PublishedContent>,
    read_bytes: usize,
}
impl<'a> ContentSnapshot<'a> {
    pub(super) fn capture(disk: &'a Disk, verified: VerifiedDisk) -> Self {
        let read_bytes = verified.bundle.objects.values().map(Vec::len).sum();
        let versions = verified
            .rows
            .iter()
            .map(|row| {
                let raw = verified
                    .bundle
                    .objects
                    .get(&row.commit)
                    .expect("invariant: verified copy retains its immutable commit bytes");
                let Object::Commit(commit) = codec::decode(raw)
                    .expect("invariant: captured verified commit bytes remain decodable")
                else {
                    unreachable!("invariant: a verified copy refers to a commit");
                };
                (
                    row.commit,
                    PublishedContent {
                        version: RetainedVersion::capture(row),
                        expected_bytes: commit.body.bytes,
                    },
                )
            })
            .collect();
        Self {
            disk,
            identity: verified.identity,
            checkpoint: verified.checkpoint,
            sequence: verified.sequence,
            versions,
            read_bytes,
        }
    }
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
    pub fn checkpoint(&self) -> Option<Digest> {
        self.checkpoint
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn versions(&self) -> impl ExactSizeIterator<Item = &PublishedContent> {
        self.versions.values()
    }
    /// Recheck the held source and child directory identities. This does not
    /// make the captured checkpoint globally latest or grant a write lease.
    pub fn recheck(&self) -> Result<()> {
        self.disk.check_identity()
    }
    /// Read only a commit present in this captured proof, with the original
    /// FULL length and digest. Every body read, including a corrupt one,
    /// consumes the shared material budget; the first excess ends this reader.
    /// Filesystem errors retain their source; absence is ContentUnavailable.
    pub fn read_content(&mut self, commit: Digest) -> Result<Vec<u8>> {
        if self.read_bytes > MAX_BYTES {
            return Err(Error::Budget);
        }
        let expected = self.versions.get(&commit).ok_or(Error::Missing(commit))?;
        self.recheck()?;
        let bytes = self.disk.body(commit)?;
        self.recheck()?;
        let bytes = bytes.ok_or(Error::ContentUnavailable(commit))?;
        self.read_bytes = self
            .read_bytes
            .checked_add(bytes.len())
            .ok_or(Error::Budget)?;
        if self.read_bytes > MAX_BYTES {
            return Err(Error::Budget);
        }
        if bytes.len() as u64 != expected.expected_bytes
            || Digest::body(&bytes) != expected.version.body()
        {
            return Err(Error::Corrupt(commit.to_string()));
        }
        Ok(bytes)
    }
}
