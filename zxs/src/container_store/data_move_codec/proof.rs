//! Stable metadata of copied names. No path, native query, or payload I/O.
use super::{Checked, Digest, Intent, Journal, MAX_ROWS, Scope, digest, eof, take, word};
use crate::{
    container_store::{Change, Id, Receipt, codec, invalid},
    hash::Sha256,
};
use std::io::{self, Write};

const MAGIC: &[u8; 8] = b"ZXDMP1\0\0";
const HISTORY: &[u8] = b"ZXS offline DATA move history v1\0";
pub(in crate::container_store) const HEADER_BYTES: usize = 144;
pub(in crate::container_store) const ROW_BYTES: usize = 128;
pub(in crate::container_store) const PROOF_BYTES_MAX: usize = 8_388_752;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::container_store) enum Side {
    Source = 1,
    Target = 2,
}

/// Expected only from checked phase records, never from an observed proof header.
#[derive(Debug)]
pub(in crate::container_store) struct Expect {
    header: Header,
    hash: Digest,
}

#[derive(Debug)]
pub(in crate::container_store) struct Header {
    side: Side,
    scope: Scope,
    token: Digest,
    container_inode: u64,
    count: u64,
    ledger: Digest,
}

impl Expect {
    pub(in crate::container_store) fn source(intent: &Checked<Intent>) -> Self {
        let i = intent.value();
        Self {
            header: Header {
                side: Side::Source,
                scope: i.scope,
                token: i.source,
                container_inode: i.source_inode,
                count: i.rows,
                ledger: i.ledger,
            },
            hash: i.source_proof,
        }
    }

    pub(in crate::container_store) fn target(journal: &Journal) -> io::Result<Self> {
        let i = journal.intent().value();
        let r = journal
            .ready()
            .ok_or_else(|| invalid("no checked DATA move target proof"))?
            .value();
        Ok(Self {
            header: Header {
                side: Side::Target,
                scope: i.scope,
                token: i.target,
                container_inode: r.target_inode,
                count: i.rows,
                ledger: i.ledger,
            },
            hash: r.target_proof,
        })
    }

    /// Size that Core must qualify on the held regular record before reading/allocating.
    pub(in crate::container_store) fn extent(&self) -> io::Result<usize> {
        self.header.extent()
    }

    pub(in crate::container_store) fn header(&self) -> io::Result<[u8; HEADER_BYTES]> {
        self.header.encode()
    }
}

impl Header {
    /// Construct an unselected header before the source proof and INTENT or target proof
    /// and READY are written. This does not create any checked phase evidence.
    pub(in crate::container_store) fn new(
        side: Side,
        scope: Scope,
        token: Digest,
        container_inode: u64,
        count: u64,
        ledger: Digest,
    ) -> io::Result<Self> {
        bounded_count(count)?;
        if !scope.valid() || token == [0; 32] || container_inode == 0 {
            return Err(invalid("invalid DATA move proof header"));
        }
        Ok(Self {
            side,
            scope,
            token,
            container_inode,
            count,
            ledger,
        })
    }

    fn extent(&self) -> io::Result<usize> {
        bounded_count(self.count)?
            .checked_mul(ROW_BYTES)
            .and_then(|n| n.checked_add(HEADER_BYTES))
            .filter(|n| *n <= PROOF_BYTES_MAX)
            .ok_or_else(|| invalid("DATA move proof extent overflow"))
    }

    pub(in crate::container_store) fn encode(&self) -> io::Result<[u8; HEADER_BYTES]> {
        let mut raw = [0; HEADER_BYTES];
        raw[..8].copy_from_slice(MAGIC);
        raw[8] = self.side as u8;
        let mut rest = &mut raw[16..];
        self.scope.write(&mut rest)?;
        rest.write_all(&self.token)?;
        rest.write_all(&self.container_inode.to_be_bytes())?;
        rest.write_all(&self.count.to_be_bytes())?;
        rest.write_all(&self.ledger)?;
        Ok(raw)
    }
}

/// Stable suffix of an original `OpaquePin`, not its live device or mount coordinate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::container_store) struct Stamp {
    pub(in crate::container_store) inode: u64,
    pub(in crate::container_store) len: u64,
    pub(in crate::container_store) mtime: (i64, i64),
    pub(in crate::container_store) ctime: (i64, i64),
}

impl Stamp {
    fn valid(self) -> bool {
        self.inode != 0
            && (0..1_000_000_000).contains(&self.mtime.1)
            && (0..1_000_000_000).contains(&self.ctime.1)
    }

    fn write(self, out: &mut impl Write) -> io::Result<()> {
        out.write_all(&self.inode.to_be_bytes())?;
        out.write_all(&self.len.to_be_bytes())?;
        for field in [self.mtime.0, self.mtime.1, self.ctime.0, self.ctime.1] {
            out.write_all(&field.to_be_bytes())?;
        }
        Ok(())
    }

    fn read(input: &mut &[u8]) -> io::Result<Self> {
        Ok(Self {
            inode: word(input)?,
            len: word(input)?,
            mtime: (
                i64::from_be_bytes(take(input)?),
                i64::from_be_bytes(take(input)?),
            ),
            ctime: (
                i64::from_be_bytes(take(input)?),
                i64::from_be_bytes(take(input)?),
            ),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::container_store) struct Row {
    pub(in crate::container_store) sequence: u64,
    pub(in crate::container_store) operation: Id,
    pub(in crate::container_store) dir_inode: u64,
    pub(in crate::container_store) receipt: Stamp,
    pub(in crate::container_store) payload: Option<Stamp>,
}

impl Row {
    fn require(self) -> io::Result<()> {
        if !(1..=MAX_ROWS).contains(&self.sequence)
            || self.operation == [0; 16]
            || self.dir_inode == 0
            || !self.receipt.valid()
            || !(1..=codec::MAX_RECEIPT as u64).contains(&self.receipt.len)
            || self.payload.is_some_and(|stamp| !stamp.valid())
            || self.sequence == 1 && self.payload.is_none()
        {
            return Err(invalid("invalid DATA move proof row"));
        }
        Ok(())
    }

    pub(in crate::container_store) fn encode(self) -> io::Result<[u8; ROW_BYTES]> {
        self.require()?;
        let mut raw = [0; ROW_BYTES];
        let mut out = &mut raw[..];
        out.write_all(&self.sequence.to_be_bytes())?;
        out.write_all(&self.operation)?;
        out.write_all(&self.dir_inode.to_be_bytes())?;
        self.receipt.write(&mut out)?;
        if let Some(stamp) = self.payload {
            stamp.write(&mut out)?;
        }
        Ok(raw)
    }

    fn read(mut raw: &[u8], index: u64) -> io::Result<Self> {
        let sequence = word(&mut raw)?;
        let operation = take(&mut raw)?;
        let dir_inode = word(&mut raw)?;
        let receipt = Stamp::read(&mut raw)?;
        let payload_bytes = take::<48>(&mut raw)?;
        let payload = if payload_bytes == [0; 48] {
            None
        } else {
            Some(Stamp::read(&mut payload_bytes.as_slice())?)
        };
        eof(raw)?;
        let row = Self {
            sequence,
            operation,
            dir_inode,
            receipt,
            payload,
        };
        row.require()?;
        if sequence != index {
            return Err(invalid("noncontiguous DATA move proof sequence"));
        }
        Ok(row)
    }
}

/// Borrow already bounded bytes. No row, even a valid prefix, escapes until the
/// full header, checksum, syntax and exact extent agree with the phase record.
pub(in crate::container_store) struct Proof<'a> {
    rows: &'a [u8],
    scope: Scope,
    count: u64,
}

impl<'a> Proof<'a> {
    pub(in crate::container_store) fn decode(raw: &'a [u8], expect: &Expect) -> io::Result<Self> {
        let header = expect.header()?;
        if raw.len() != expect.extent()? || !raw.starts_with(&header) {
            return Err(invalid("wrong DATA move proof header or extent"));
        }
        let rows = &raw[HEADER_BYTES..];
        if digest(raw)? != expect.hash {
            return Err(invalid("wrong DATA move proof checksum"));
        }
        for (n, raw) in (1..=expect.header.count).zip(rows.as_chunks::<ROW_BYTES>().0) {
            Row::read(raw, n)?;
        }
        Ok(Self {
            rows,
            scope: expect.header.scope,
            count: expect.header.count,
        })
    }

    /// Expose the phase-qualified native stamp only for the corresponding canonical ledger row.
    /// The caller still checks whole canonical history, now-held fd provenance and actual bytes.
    pub(in crate::container_store) fn for_receipt(&self, receipt: &Receipt) -> io::Result<Row> {
        if receipt.mutation.container != self.scope.container
            || !(1..=self.count).contains(&receipt.sequence)
        {
            return Err(invalid("receipt belongs to a different DATA move proof"));
        }
        let index = usize::try_from(receipt.sequence - 1)
            .map_err(|_| invalid("invalid DATA move proof index"))?;
        let raw = self
            .rows
            .as_chunks::<ROW_BYTES>()
            .0
            .get(index)
            .ok_or_else(|| invalid("missing DATA move proof row"))?;
        let row = Row::read(raw, receipt.sequence)?;
        let canonical = codec::encode(receipt)?;
        let len =
            u64::try_from(canonical.len()).map_err(|_| invalid("DATA move receipt extent"))?;
        let body = match &receipt.change {
            Change::Save { body, .. } => Some(body.length),
            _ => None,
        };
        if row.operation != receipt.mutation.request
            || row.receipt.len != len
            || row.payload.map(|s| s.len) != body
        {
            return Err(invalid(
                "DATA move native row differs from canonical ledger",
            ));
        }
        Ok(row)
    }
}

fn bounded_count(count: u64) -> io::Result<usize> {
    if !(1..=MAX_ROWS).contains(&count) {
        return Err(invalid("DATA move proof row limit"));
    }
    usize::try_from(count).map_err(|_| invalid("DATA move proof row count out of range"))
}

/// Logical receipt-only commitment, independent of DATA inode allocation/re-mount.
/// The creating adapter must derive these receipts by complete canonical State replay.
pub(in crate::container_store) struct LedgerHash {
    sha: Sha256,
    container: Id,
    count: u64,
    consumed: u64,
}

impl LedgerHash {
    pub(in crate::container_store) fn new(container: Id, count: u64) -> io::Result<Self> {
        bounded_count(count)?;
        if container == [0; 16] {
            return Err(invalid("zero DATA move ledger container"));
        }
        let mut sha = Sha256::new();
        sha.update(HISTORY)?;
        sha.update(&container)?;
        sha.update(&count.to_be_bytes())?;
        Ok(Self {
            sha,
            container,
            count,
            consumed: 0,
        })
    }

    pub(in crate::container_store) fn push(&mut self, receipt: &Receipt) -> io::Result<()> {
        if receipt.mutation.container != self.container
            || receipt.mutation.request == [0; 16]
            || self.consumed == self.count
            || receipt.sequence != self.consumed + 1
        {
            return Err(invalid("invalid DATA move ledger ordering or scope"));
        }
        let raw = codec::encode(receipt)?;
        if raw.len() > codec::MAX_RECEIPT {
            return Err(invalid("DATA move ledger receipt exceeds capacity"));
        }
        let len =
            u32::try_from(raw.len()).map_err(|_| invalid("DATA move ledger receipt extent"))?;
        self.sha.update(&receipt.sequence.to_be_bytes())?;
        self.sha.update(&receipt.mutation.request)?;
        self.sha.update(&len.to_be_bytes())?;
        self.sha.update(&raw)?;
        self.consumed += 1;
        Ok(())
    }

    pub(in crate::container_store) fn finish(self) -> io::Result<Digest> {
        if self.consumed != self.count {
            return Err(invalid("incomplete DATA move ledger fingerprint"));
        }
        Ok(self.sha.finish())
    }
}
