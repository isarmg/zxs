//! Bounded canonical ledger and held native leaf evidence. Only the caller may read raw bodies.
use super::{
    Digest, ReadAccess, Scope, absent, digest, invalid, native_name,
    proof::{HEADER_BYTES, Header, LedgerHash, PROOF_BYTES_MAX, Row, Side, Stamp},
    read_raw,
};
use crate::{
    container_store::{Change, Receipt, codec, entry_leaf, held_entry, ledger_limit, replay_with},
    fs::{HeldDir, OpaquePin},
};
use std::{io, os::unix::fs::MetadataExt};

const ACCESS: ReadAccess = ReadAccess::NoAtime;
const MAX_LEDGER_BYTES: usize = 32 << 20;
const MAX_TAGS: usize = 1 << 18;
type DirectoryStamp = (u64, i64, i64, i64, i64);

pub(super) struct Source {
    pub(super) original: HeldDir,
    pub(super) ledger: Digest,
    pub(super) count: u64,
    pub(super) bytes: Vec<u8>,
    pub(super) hash: Digest,
}

impl Source {
    pub(super) fn capture(
        data: &HeldDir,
        scope: Scope,
        token: Digest,
        live: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<Self> {
        live()?;
        let name = native_name(&crate::container_store::hex(scope.container))?;
        let original = data
            .child_directory(&name)?
            .ok_or_else(|| invalid("the exact DATA move source is absent"))?;
        let mut evidence = walk(&original, scope, live, |receipt| {
            Ok(Member::open(&original, receipt)?.row)
        })?;
        evidence.bind(Side::Source, scope, token, original.entity.inode)?;
        data.require_named()?;
        original.require_named()?;
        Ok(Self {
            original,
            ledger: evidence.ledger,
            count: evidence.count,
            hash: digest(&evidence.bytes)?,
            bytes: evidence.bytes,
        })
    }

    pub(super) fn inode(&self) -> u64 {
        self.original.entity.inode
    }

    pub(super) fn require_named(&self) -> io::Result<()> {
        self.original.require_named()
    }

    pub(super) fn require_same(&self, other: &Self) -> io::Result<()> {
        if self.original.entity != other.original.entity
            || !self.original.same_mount(&other.original)?
            || self.bytes != other.bytes
        {
            return Err(invalid(
                "the native source no longer matches its exact pre-READY proof",
            ));
        }
        self.require_named()?;
        other.require_named()
    }
}

fn top_stamp(at: &HeldDir) -> io::Result<DirectoryStamp> {
    let m = at.file.metadata()?;
    Ok((
        m.nlink(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    ))
}

pub(super) struct Evidence {
    pub(super) ledger: Digest,
    pub(super) count: u64,
    pub(super) bytes: Vec<u8>,
}

impl Evidence {
    pub(super) fn bind(
        &mut self,
        side: Side,
        scope: Scope,
        token: Digest,
        inode: u64,
    ) -> io::Result<()> {
        self.bytes[..HEADER_BYTES].copy_from_slice(
            &Header::new(side, scope, token, inode, self.count, self.ledger)?.encode()?,
        );
        Ok(())
    }
}

/// Read only the bounded canonical history here. A caller may inspect one
/// typed entry and return a witnessed row for that entry or its exact copy.
pub(super) fn walk(
    at: &HeldDir,
    scope: Scope,
    live: &dyn Fn() -> io::Result<()>,
    mut visit: impl FnMut(&Receipt) -> io::Result<Row>,
) -> io::Result<Evidence> {
    live()?;
    at.require_named()?;
    let before = top_stamp(at)?;
    let maximum = usize::try_from(super::super::MAX_ROWS).map_err(|_| ledger_limit())?;
    let mut tags = MAX_TAGS;
    let state = replay_with(
        at,
        scope.container,
        ACCESS,
        (maximum, MAX_LEDGER_BYTES),
        |_, bytes| {
            live()?;
            codec::decode_charging(bytes, || {
                tags = tags.checked_sub(1).ok_or_else(ledger_limit)?;
                Ok(())
            })
        },
    )?;
    let count = state.sequence;
    let mut digest = LedgerHash::new(scope.container, count)?;
    let mut sequence: Vec<&Receipt> = state.receipts.values().collect();
    sequence.sort_unstable_by_key(|row| row.sequence);
    let mut bytes = vec![0; HEADER_BYTES];
    for receipt in sequence {
        live()?;
        digest.push(receipt)?;
        bytes.extend_from_slice(&visit(receipt)?.encode()?);
    }
    if bytes.len() > PROOF_BYTES_MAX {
        return Err(ledger_limit());
    }
    live()?;
    if before != top_stamp(at)? {
        return Err(invalid(
            "the held DATA move tree changed during observation",
        ));
    }
    at.require_named()?;
    Ok(Evidence {
        ledger: digest.finish()?,
        count,
        bytes,
    })
}

pub(super) struct Member {
    pub(super) directory: HeldDir,
    receipt: OpaquePin,
    pub(super) payload: Option<OpaquePin>,
    pub(super) row: Row,
}

impl Member {
    pub(super) fn open(at: &HeldDir, selected: &Receipt) -> io::Result<Self> {
        let name = entry_leaf(selected.sequence, selected.mutation.request);
        let directory = held_entry(at, &name)?;
        let has_body = matches!(selected.change, Change::Save { .. });
        directory.each_name(ACCESS, |leaf| match leaf {
            b"receipt" => Ok(()),
            b"payload" if has_body => Ok(()),
            _ => Err(invalid("DATA move member has an unexpected descendant")),
        })?;
        let receipt = directory
            .opaque_pin_with(c"receipt", ACCESS)?
            .ok_or_else(|| invalid("the exact move receipt is absent"))?;
        let canonical = codec::encode(selected)?;
        if read_raw(&receipt, canonical.len())? != canonical {
            return Err(invalid(
                "the named DATA move receipt differs from canonical replay",
            ));
        }
        let payload = if has_body {
            directory.opaque_pin_with(c"payload", ACCESS)?
        } else {
            None
        };
        let row = qualified_row(&directory, selected, &receipt, payload.as_ref())?;
        Ok(Self {
            directory,
            receipt,
            payload,
            row,
        })
    }

    pub(super) fn require(&self) -> io::Result<()> {
        require_members(&self.directory, &self.receipt, self.payload.as_ref())
    }
}

pub(super) fn qualified_row(
    directory: &HeldDir,
    selected: &Receipt,
    receipt: &OpaquePin,
    payload: Option<&OpaquePin>,
) -> io::Result<Row> {
    let expected = match &selected.change {
        Change::Save { body, .. } => Some(body.length),
        _ => None,
    };
    let payload_stamp = payload.map(stamp).transpose()?;
    if payload_stamp.map(|p| p.len) != expected {
        return Err(invalid("DATA move payload shape differs from its receipt"));
    }
    let row = Row {
        sequence: selected.sequence,
        operation: selected.mutation.request,
        dir_inode: directory.entity.inode,
        receipt: stamp(receipt)?,
        payload: payload_stamp,
    };
    require_members(directory, receipt, payload)?;
    Ok(row)
}

fn require_members(
    directory: &HeldDir,
    receipt: &OpaquePin,
    payload: Option<&OpaquePin>,
) -> io::Result<()> {
    if let Some(pinned) = payload {
        pinned.require(directory, c"payload")?;
    } else {
        absent(directory, c"payload")?;
    }
    receipt.require(directory, c"receipt")?;
    directory.require_named()
}

fn stamp(pin: &OpaquePin) -> io::Result<Stamp> {
    let m = pin.file.metadata()?;
    Ok(Stamp {
        inode: m.ino(),
        len: m.len(),
        mtime: (m.mtime(), m.mtime_nsec()),
        ctime: (m.ctime(), m.ctime_nsec()),
    })
}
