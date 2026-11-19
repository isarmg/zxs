//! The original portable GENESIS, not enrollment or a FILE write authority.
//! Only a complete, selected SYSTEM with its real REG lock can turn this
//! declaration into an observation. Never try a legacy wire interpretation.
use super::portable_claim::Declaration;
use crate::{OperationId, PoolId, Role, VolumeId, fs::{self, ComponentLimit, invalid}, hash};
use std::{collections::BTreeSet, ffi::OsStr, io, os::unix::ffi::OsStrExt, path::{Path,PathBuf}};

const B: &[u8; 23] = b"ZXS-PORTABLE-1/GENESIS\0";
const FLOOR: &[u8; 25] = b"ZXS-PORTABLE-1/CONFIRMED\0";
const MAX_ROWS: usize = 64;
const MAX_PATH: usize = 4096;
const MAX_GENESIS: usize = 77 + MAX_ROWS * (20 + MAX_PATH) + 32;
const FLOOR_SIZE: usize = 105;
const MARK_SIZE: usize = 103;

struct Row {
    role: Role,
    volume: VolumeId,
    limit: ComponentLimit,
    path: PathBuf,
}
struct Genesis {
    pool: PoolId,
    op: OperationId,
    approval_ref: [u8;16],
    uid: u32,
    rows: Vec<Row>,
    digest: [u8;32],
}
fn take<'a>(b: &mut &'a [u8], n: usize) -> io::Result<&'a [u8]> {
    let (x, rest) = b.split_at_checked(n).ok_or_else(|| invalid("short portable GENESIS"))?;
    *b = rest; Ok(x)
}
fn fixed<const N: usize>(b: &mut &[u8]) -> io::Result<[u8;N]> {
    Ok(take(b, N)?.try_into().expect("exact-width field"))
}
fn id(b: &mut &[u8]) -> io::Result<[u8;16]> {
    let value = fixed(b)?;
    if value == [0;16] { return Err(invalid("zero portable genesis identity")); }
    Ok(value)
}
impl Genesis {
    /// This reader understands exactly an empty register, including EOF.
    /// No speculative/unknown suffix can masquerade as the confirmed cut.
    #[cfg(test)]
    fn only(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > MAX_GENESIS { return Err(invalid("portable GENESIS exceeds its format limit")); }
        let (genesis,tail)=Self::prefix(bytes)?;
        if !tail.is_empty(){return Err(invalid("not exactly the empty portable registration"));}
        Ok(genesis)
    }

    /// Row and path counts bound the genesis itself before any decision is
    /// considered. Hash exactly the bytes decoded here, not a new read from
    /// a pathname, the first assumed length, or the last 32 bytes of a journal.
    fn prefix(bytes:&[u8])->io::Result<(Self,&[u8])>{
        let mut b = bytes;
        if take(&mut b, B.len())? != B { return Err(invalid("unknown portable genesis family")); }
        let pool = PoolId(id(&mut b)?);
        let op = OperationId(id(&mut b)?);
        let approval_ref = id(&mut b)?; // correlation only, never authorization
        let uid = u32::from_be_bytes(fixed(&mut b)?);
        let count = usize::from(u16::from_be_bytes(fixed(&mut b)?));
        if uid == 0 || uid == u32::MAX || !(3..=MAX_ROWS).contains(&count) {
            return Err(invalid("invalid original storage uid or root count"));
        }
        let mut rows: Vec<Row> = Vec::with_capacity(count);
        let mut ids = BTreeSet::new();
        for i in 0..count {
            let tag = fixed::<1>(&mut b)?[0];
            let (wanted, role) = match i { 0 => (0, Role::System), 1 => (1, Role::Recycle), _ => (2, Role::File) };
            let volume = VolumeId(id(&mut b)?);
            let limit = ComponentLimit::from_original(fixed::<1>(&mut b)?[0])?;
            let n = usize::from(u16::from_be_bytes(fixed(&mut b)?));
            if tag != wanted || !ids.insert(volume) || n == 0 || n > MAX_PATH {
                return Err(invalid("wrong portable role, duplicate volume or invalid root size"));
            }
            let raw = take(&mut b, n)?;
            let path = Path::new(OsStr::from_bytes(raw));
            fs::check_path(path)?;
            if i > 2 && rows[i - 1].path.as_os_str().as_bytes() >= raw {
                return Err(invalid("original FILE roots not strictly byte-sorted"));
            }
            if rows.iter().any(|r| r.path.starts_with(path) || path.starts_with(&r.path)) {
                return Err(invalid("duplicate or nested portable roots"));
            }
            rows.push(Row { role, volume, limit, path: path.to_owned() });
        }
        let decoded=bytes.len()-b.len();
        if decoded>MAX_GENESIS-32{return Err(invalid("portable GENESIS exceeds its format limit"));}
        let digest=hash::digest(&bytes[..decoded])?;
        if fixed::<32>(&mut b)?!=digest{return Err(invalid("portable genesis body digest mismatch"));}
        Ok((Self {pool,op,approval_ref,uid,rows,digest},b))
    }
    fn mark(&self, row: &Row) -> Declaration {
        Declaration { role: row.role, pool: self.pool, volume: row.volume, claimed_by: self.op, source_digest: self.digest }
    }
    fn require_zero(&self, floor: &[u8]) -> io::Result<()> {
        if floor.len() != FLOOR_SIZE || &floor[..25] != FLOOR || floor[25..57] != self.digest ||
            floor[57..73] != [0;16] || floor[73..] != self.digest {
            return Err(invalid("portable floor does not confirm this exact empty original"));
        }
        Ok(())
    }
}

mod decision;
pub(in crate::pool) mod opened;
#[cfg(test)]
#[path="../../tests/unit/portable_genesis.rs"]
mod tests;

#[cfg(test)]
#[path="../../tests/unit/portable_decisions.rs"]
mod decision_tests;
