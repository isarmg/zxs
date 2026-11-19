//! One portable initial request and its byte-level feedback. Nothing in this
//! module opens a saved path, proves an inode or lease, or authorizes recovery.
use crate::path_rules::{Kind, MAX_PATH_BYTES};
use crate::portable_claim::{Claim, Role};
use sha2::{Digest, Sha256};

const GENESIS: &[u8; 23] = b"ZXS-PORTABLE-1/GENESIS\0";
const CONFIRMED: &[u8; 25] = b"ZXS-PORTABLE-1/CONFIRMED\0";
const FLOOR_LEN: usize = CONFIRMED.len() + 32 + 8 + 8 + 32;
const MAX_ROWS: usize = 64;
const MAX_GENESIS: usize = 77 + MAX_ROWS * (20 + MAX_PATH_BYTES) + 32;
type Id = [u8; 16];

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum Invalid {
    Format, Length, Identity, Owner, Role, Path, ComponentLimit, Digest,
    TailNotCovered, Request, System, Floor, Feedback,
}

struct Row<'a> {
    role: Role,
    id: Id,
    limit: u8,
    path: &'a [u8],
}

pub(crate) struct Initial<'a> {
    body: &'a [u8],
    digest: [u8; 32],
    pool: Id,
    op: Id,
    uid: u32,
    rows: Vec<Row<'a>>,
}

struct Fields<'a>(&'a [u8]);
impl<'a> Fields<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Invalid> {
        let (field, rest) = self.0.split_at_checked(n).ok_or(Invalid::Length)?;
        self.0 = rest;
        Ok(field)
    }

    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], Invalid> {
        self.take(N)?.try_into().map_err(|_| Invalid::Length)
    }

    fn id(&mut self) -> Result<Id, Invalid> {
        let x = self.bytes()?;
        if x == [0; 16] { Err(Invalid::Identity) } else { Ok(x) }
    }
}

// Both operands have already passed StorageRoot's exact raw path rules.
fn covers(outer: &[u8], inner: &[u8]) -> bool {
    inner.strip_prefix(outer).is_some_and(|rest| rest.is_empty() || rest.starts_with(b"/"))
}

impl<'a> Initial<'a> {
    /// Parse and hash only the bounded original. Never scan, hash or consume
    /// an unknown suffix. A valid prefix with any suffix is not an empty pool.
    pub(crate) fn decode(image: &'a [u8]) -> Result<Self, Invalid> {
        let mut input = Fields(image.strip_prefix(GENESIS.as_slice()).ok_or(Invalid::Format)?);
        let pool = input.id()?;
        let op = input.id()?;
        input.id()?; // original non-secret approval reference, kept verbatim in body
        let uid = u32::from_be_bytes(input.bytes()?);
        if uid == 0 || uid == u32::MAX { return Err(Invalid::Owner); }
        let count = usize::from(u16::from_be_bytes(input.bytes()?));
        if !(3..=MAX_ROWS).contains(&count) { return Err(Invalid::Role); }

        let mut rows: Vec<Row<'a>> = Vec::with_capacity(count);
        for i in 0..count {
            let role = match i { 0 => Role::System, 1 => Role::Recycle, _ => Role::Data };
            if input.bytes::<1>()?[0] != role as u8 { return Err(Invalid::Role); }
            let id = input.id()?;
            let limit = input.bytes::<1>()?[0];
            if limit == 0 { return Err(Invalid::ComponentLimit); }
            let n = usize::from(u16::from_be_bytes(input.bytes()?));
            if n > MAX_PATH_BYTES { return Err(Invalid::Path); }
            let path = input.take(n)?;
            Kind::StorageRoot.check_path(path).map_err(|_| Invalid::Path)?;
            if rows.iter().any(|old| old.id == id) { return Err(Invalid::Identity); }
            if rows.iter().any(|old| covers(old.path, path) || covers(path, old.path))
                || (i > 2 && rows[i - 1].path >= path) {
                return Err(Invalid::Path);
            }
            rows.push(Row { role, id, limit, path });
        }

        let end = image.len() - input.0.len();
        if end > MAX_GENESIS - 32 { return Err(Invalid::Length); }
        let body = &image[..end];
        let digest: [u8; 32] = Sha256::digest(body).into();
        if input.bytes::<32>()? != digest { return Err(Invalid::Digest); }
        if !input.0.is_empty() { return Err(Invalid::TailNotCovered); }
        Ok(Self { body, digest, pool, op, uid, rows })
    }

    /// Only equality to the independently supplied canonical approved request.
    /// Recomputing the expected request from this image would prove nothing.
    pub(crate) fn check_request(&self, expected_body: &[u8]) -> Result<(), Invalid> {
        if self.body == expected_body { Ok(()) } else { Err(Invalid::Request) }
    }

    pub(crate) fn row(&self, index: usize) -> Result<(&[u8], u8, Claim), Invalid> {
        let row = self.rows.get(index).ok_or(Invalid::Feedback)?;
        let claim = Claim::from_fields(row.role, self.pool, row.id, self.op, self.digest)
            .map_err(|_| Invalid::Identity)?;
        Ok((row.path, row.limit, claim))
    }

    fn check_zero_floor(&self, floor: &[u8]) -> Result<(), Invalid> {
        if floor.len() != FLOOR_LEN { return Err(Invalid::Floor); }
        let rest = floor.strip_prefix(CONFIRMED.as_slice()).ok_or(Invalid::Floor)?;
        if rest[..32] != self.digest || rest[32..48] != [0; 16] || rest[48..] != self.digest {
            return Err(Invalid::Floor);
        }
        Ok(())
    }

    /// Bytes supplied from a selected SYSTEM only; this does not observe a
    /// publication name, grant a lock, inspect other roots or issue a receipt.
    pub(crate) fn check_system_fields(&self, selected: &[u8], uid: u32, marker: &[u8],
                                      floor: &[u8]) -> Result<(), Invalid> {
        let (path, _, claim) = self.row(0)?;
        if self.uid != uid || path != selected { return Err(Invalid::System); }
        claim.check_row(marker).map_err(|_| Invalid::Feedback)?;
        self.check_zero_floor(floor)
    }

    /// At PENDING only: all explicitly selected roots, in original row order.
    /// Return how many consecutive markers' *bytes* match, not a resume token.
    pub(crate) fn pending_feedback(&self, floor: Option<&[u8]>, markers: &[Option<&[u8]>])
        -> Result<usize, Invalid> {
        if markers.len() != self.rows.len() { return Err(Invalid::Feedback); }
        if let Some(raw) = floor { self.check_zero_floor(raw)?; }
        let mut matched = 0;
        for (i, raw) in markers.iter().enumerate() {
            if let Some(raw) = raw {
                if floor.is_none() || matched != i { return Err(Invalid::Feedback); }
                self.row(i)?.2.check_row(raw).map_err(|_| Invalid::Feedback)?;
                matched += 1;
            }
        }
        Ok(matched)
    }
}

#[cfg(test)]
#[path = "../tests/portable/genesis.rs"]
mod tests;
