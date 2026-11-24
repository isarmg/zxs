//! Canonical recycle selector and matching R ready record. On-disk identity is not a live FD stamp.
use super::{Id, Mutation, invalid};
use crate::fs::Entity;
use std::io;

pub(super) const LEGACY_BYTES: usize = 108;
pub(super) const ENROLLED_BYTES: usize = 172;
pub(super) type DiskToken = [u8; 32];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VolumeIdentity {
    Legacy { source: u64, recycle: u64 },
    Enrolled { pool: Id, data: DiskToken, recycle: DiskToken },
}

#[derive(Clone, Copy)]
pub(super) struct RecycleRole {
    pub(super) pool: Id,
    pub(super) token: DiskToken,
}

impl RecycleRole {
    pub(super) fn require(self) -> io::Result<()> {
        if self.pool == [0; 16] || self.token == [0; 32] {
            return Err(invalid("invalid enrolled recycle role"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RecycleReceipt {
    pub(crate) mutation: Mutation,
    pub(crate) original_sequence: u64,
    identity: VolumeIdentity,
    source_inode: u64,
    recycle_inode: u64,
    pub(super) source_proof: [u8; 32],
}

impl RecycleReceipt {
    pub(super) fn enrolled(
        mutation: Mutation,
        original_sequence: u64,
        role: RecycleRole,
        data: DiskToken,
        inodes: (u64, u64),
        source_proof: [u8; 32],
    ) -> io::Result<Self> {
        let record = Self {
            mutation,
            original_sequence,
            identity: VolumeIdentity::Enrolled { pool: role.pool, data, recycle: role.token },
            source_inode: inodes.0,
            recycle_inode: inodes.1,
            source_proof,
        };
        record.validate()?;
        Ok(record)
    }

    pub(super) fn is_enrolled(self) -> bool {
        matches!(self.identity, VolumeIdentity::Enrolled { .. })
    }

    pub(super) fn require_data(self, token: DiskToken) -> io::Result<()> {
        if let VolumeIdentity::Enrolled { data, .. } = self.identity
            && data != token
        {
            return Err(invalid("recycle receipt belongs to a different routed DATA token"));
        }
        Ok(())
    }

    // The caller independently holds the enrolled root, child and actual mount.
    // RC1 deliberately does not accept any drift in its old device coordinate.
    pub(super) fn require_arrival(self, role: RecycleRole, observed: Entity) -> io::Result<()> {
        let exact = match self.identity {
            VolumeIdentity::Legacy { recycle, .. } => recycle == observed.device,
            VolumeIdentity::Enrolled { pool, recycle, .. } => (pool, recycle) == (role.pool, role.token),
        };
        if !exact || observed.inode != self.recycle_inode {
            return Err(invalid("recycle location differs from its original ready receipt"));
        }
        Ok(())
    }

    pub(super) fn require_source(self, data: DiskToken, observed: Entity) -> io::Result<()> {
        self.require_data(data)?;
        if observed.inode != self.source_inode
            || matches!(self.identity, VolumeIdentity::Legacy { source, .. } if source != observed.device)
        {
            return Err(invalid("source retirement no longer identifies the exact moved container"));
        }
        Ok(())
    }

    pub(super) fn encode(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(if self.is_enrolled() { ENROLLED_BYTES } else { LEGACY_BYTES });
        out.extend_from_slice(if self.is_enrolled() { b"RC2\0" } else { b"RC1\0" });
        out.extend_from_slice(&self.mutation.container);
        out.extend_from_slice(&self.mutation.request);
        out.extend_from_slice(&self.original_sequence.to_be_bytes());
        match self.identity {
            VolumeIdentity::Legacy { source, recycle } => {
                for number in [source, self.source_inode, recycle, self.recycle_inode] {
                    out.extend_from_slice(&number.to_be_bytes());
                }
            }
            VolumeIdentity::Enrolled { pool, data, recycle } => {
                out.extend_from_slice(&pool);
                out.extend_from_slice(&data);
                out.extend_from_slice(&recycle);
                out.extend_from_slice(&self.source_inode.to_be_bytes());
                out.extend_from_slice(&self.recycle_inode.to_be_bytes());
            }
        }
        out.extend_from_slice(&self.source_proof);
        out
    }

    pub(super) fn decode(raw: &[u8]) -> io::Result<Self> {
        let (identity, source_at, recycle_at, proof_at) = match (raw.len(), raw.get(..4)) {
            (LEGACY_BYTES, Some(b"RC1\0")) => (
                VolumeIdentity::Legacy { source: word(raw, 44)?, recycle: word(raw, 60)? }, 52, 68, 76,
            ),
            (ENROLLED_BYTES, Some(b"RC2\0")) => (
                VolumeIdentity::Enrolled { pool: field(raw, 44)?, data: field(raw, 60)?, recycle: field(raw, 92)? },
                124, 132, 140,
            ),
            _ => return Err(invalid("unknown recycle receipt format or extent")),
        };
        let record = Self {
            mutation: Mutation { container: field(raw, 4)?, request: field(raw, 20)? },
            original_sequence: word(raw, 36)?,
            identity,
            source_inode: word(raw, source_at)?,
            recycle_inode: word(raw, recycle_at)?,
            source_proof: field(raw, proof_at)?,
        };
        record.validate()?;
        Ok(record)
    }

    fn validate(self) -> io::Result<()> {
        let volume = match self.identity {
            VolumeIdentity::Legacy { source, recycle } => source != recycle,
            VolumeIdentity::Enrolled { pool, data, recycle } => {
                pool != [0; 16] && data != [0; 32] && recycle != [0; 32] && data != recycle
                    && self.source_inode != 0 && self.recycle_inode != 0
            }
        };
        if !volume || self.mutation.container == [0; 16] || self.mutation.request == [0; 16]
            || self.original_sequence == 0
        {
            return Err(invalid("invalid recycle transaction identity"));
        }
        Ok(())
    }
}

fn field<const N: usize>(raw: &[u8], at: usize) -> io::Result<[u8; N]> {
    raw.get(at..at + N)
        .and_then(|value| value.try_into().ok())
        .ok_or_else(|| invalid("invalid recycle receipt field"))
}

fn word(raw: &[u8], at: usize) -> io::Result<u64> {
    field(raw, at).map(u64::from_be_bytes)
}

#[cfg(test)]
#[path = "../../tests/unit/recycle_receipt.rs"]
mod tests;
