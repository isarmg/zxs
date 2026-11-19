//! The portable ordinary-file declaration, not evidence of custody or a
//! current registration. Only the selected, confirmed SYSTEM can supply
//! the expected row. No paths are selected or changed by this module.
use crate::{OperationId, PoolId, Role, VolumeId, fs::invalid};
use std::{ffi::CStr, fs::File, io, os::unix::fs::{FileExt, MetadataExt}};

/// Exactly one reserved service name at an original registered ROOT. This
/// is not a reserved business display name and applies to no child parent.
pub(super) const ROOT_NAME: &CStr = c".zxs-new-ffffffffffffffffffffffffffffffff-0";
const DOMAIN: &[u8; 22] = b"ZXS-PORTABLE-1/VOLUME\0";
const SIZE: usize = 103;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Declaration {
    pub(super) role: Role,
    pub(super) pool: PoolId,
    pub(super) volume: VolumeId,
    pub(super) claimed_by: OperationId,
    pub(super) source_digest: [u8; 32],
}

impl Declaration {
    /// Exact bytes. Neither a known digest nor an exact clone of the bytes
    /// can grant a write, locate a pool or prove the original medium.
    pub(super) fn decode(bytes: &[u8]) -> io::Result<Self> {
        let b: &[u8; SIZE] = bytes.try_into().map_err(|_| invalid("wrong portable volume declaration length"))?;
        if !b.starts_with(DOMAIN) { return Err(invalid("unknown portable volume declaration domain")); }
        let role = match b[22] {
            0 => Role::System, 1 => Role::Recycle, 2 => Role::File,
            _ => return Err(invalid("unknown portable volume role")),
        };
        fn id(raw: &[u8]) -> io::Result<[u8; 16]> {
            let value: [u8; 16] = raw.try_into().map_err(|_| invalid("wrong portable ID size"))?;
            if value == [0; 16] { return Err(invalid("all-zero portable registration ID")); }
            Ok(value)
        }
        // All positions below are within the exact-size array above. The
        // source digest can only be compared with the original claim body,
        // not recomputed from this file or the latest REGISTER contents.
        let mut source_digest = [0; 32];
        source_digest.copy_from_slice(&b[71..103]);
        Ok(Self { role, pool: PoolId(id(&b[23..39])?), volume: VolumeId(id(&b[39..55])?),
            claimed_by: OperationId(id(&b[55..71])?), source_digest })
    }

    /// Read from a caller-held ordinary regular file without moving its seek
    /// offset. This is still only a declaration; no directory/role permission
    /// or authenticated original is returned even on complete byte equality.
    pub(super) fn read(file: &File) -> io::Result<Self> {
        let stamp = || -> io::Result<_> {
            let m = file.metadata()?;
            if !m.is_file() || m.len() != SIZE as u64 { return Err(invalid("portable marker is not an exact-size regular file")); }
            Ok((m.mtime(), m.mtime_nsec(), m.ctime(), m.ctime_nsec()))
        };
        let before = stamp()?;
        let mut raw = [0; SIZE];
        file.read_exact_at(&mut raw, 0)?;
        if before != stamp()? { return Err(invalid("portable volume declaration changed during observation")); }
        Self::decode(&raw)
    }
}

#[cfg(test)]
#[path="../../tests/unit/portable_claim.rs"]
mod tests;
