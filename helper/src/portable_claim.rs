//! Exact bytes of one portable volume claim, not a registration reader or authority.
//! No paths, I/O, state transition, credentials or storage-specific inputs.
pub(crate) const ROOT_SLOT: &[u8] = b".zxs-new-ffffffffffffffffffffffffffffffff-0";
const MAGIC: &[u8; 22] = b"ZXS-PORTABLE-1/VOLUME\0";
pub(crate) const LEN: usize = 103;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum Role {
    System = 0,
    Recycle = 1,
    Data = 2,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum Invalid {
    Length,
    Domain,
    Role,
    ZeroIdentity,
    DifferentRow,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct Claim {
    role: Role,
    pool: [u8; 16],
    volume: [u8; 16],
    acquisition: [u8; 16],
    source_digest: [u8; 32],
}

impl Claim {
    /// Syntactic fields only. A caller cannot infer the authority of a SYSTEM
    /// record, consent, or a physical disk from this value or its digest.
    pub(crate) fn from_fields(
        role: Role,
        pool: [u8; 16],
        volume: [u8; 16],
        acquisition: [u8; 16],
        source_digest: [u8; 32],
    ) -> Result<Self, Invalid> {
        if [pool, volume, acquisition].contains(&[0; 16]) {
            return Err(Invalid::ZeroIdentity);
        }
        Ok(Self { role, pool, volume, acquisition, source_digest })
    }

    pub(crate) fn decode(raw: &[u8]) -> Result<Self, Invalid> {
        if raw.len() != LEN {
            return Err(Invalid::Length);
        }
        let fields = raw.strip_prefix(MAGIC.as_slice()).ok_or(Invalid::Domain)?;
        let (&tag, ids) = fields.split_first().ok_or(Invalid::Length)?;
        let role = match tag {
            0 => Role::System,
            1 => Role::Recycle,
            2 => Role::Data,
            _ => return Err(Invalid::Role),
        };
        let (pool, tail) = ids.split_first_chunk::<16>().ok_or(Invalid::Length)?;
        let (volume, tail) = tail.split_first_chunk::<16>().ok_or(Invalid::Length)?;
        let (op, digest) = tail.split_first_chunk::<16>().ok_or(Invalid::Length)?;
        Self::from_fields(role, *pool, *volume, *op, digest.try_into().map_err(|_| Invalid::Length)?)
    }

    pub(crate) fn encode(self) -> [u8; LEN] {
        let mut out = [0u8; LEN];
        out[..22].copy_from_slice(MAGIC);
        out[22] = self.role as u8;
        out[23..39].copy_from_slice(&self.pool);
        out[39..55].copy_from_slice(&self.volume);
        out[55..71].copy_from_slice(&self.acquisition);
        out[71..].copy_from_slice(&self.source_digest);
        out
    }

    /// Byte-level match with an explicitly expected row. Even a complete
    /// match cannot distinguish a byte-for-byte copy from the original volume.
    pub(crate) fn check_row(self, actual: &[u8]) -> Result<(), Invalid> {
        if Self::decode(actual)? != self {
            return Err(Invalid::DifferentRow);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/portable/volume_claim.rs"]
mod tests;
