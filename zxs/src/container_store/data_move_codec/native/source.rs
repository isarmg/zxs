//! Bound metadata-only observation of the current source ledger and leaves.
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

pub(super) struct Source {
    original: HeldDir,
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
        let before = top_stamp(&original)?;
        let (ledger, count, bytes) = metadata(&original, scope, token, live)?;
        if before != top_stamp(&original)? {
            return Err(invalid(
                "the exact DATA move source changed during observation",
            ));
        }
        original.require_named()?;
        data.require_named()?;
        live()?;
        Ok(Self {
            original,
            ledger,
            count,
            hash: digest(&bytes)?,
            bytes,
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

fn top_stamp(at: &HeldDir) -> io::Result<(u64, i64, i64, i64, i64)> {
    let m = at.file.metadata()?;
    Ok((
        m.nlink(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    ))
}

fn metadata(
    at: &HeldDir,
    scope: Scope,
    token: Digest,
    live: &dyn Fn() -> io::Result<()>,
) -> io::Result<(Digest, u64, Vec<u8>)> {
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
    let mut proof = vec![0; HEADER_BYTES];
    for receipt in sequence {
        live()?;
        digest.push(receipt)?;
        proof.extend_from_slice(&member(at, receipt)?.encode()?);
    }
    let ledger = digest.finish()?;
    proof[..HEADER_BYTES].copy_from_slice(
        &Header::new(Side::Source, scope, token, at.entity.inode, count, ledger)?.encode()?,
    );
    if proof.len() > PROOF_BYTES_MAX {
        return Err(ledger_limit());
    }
    Ok((ledger, count, proof))
}

fn member(at: &HeldDir, selected: &Receipt) -> io::Result<Row> {
    let name = entry_leaf(selected.sequence, selected.mutation.request);
    let directory = held_entry(at, &name)?;
    let body = match &selected.change {
        Change::Save { body, .. } => Some(body.length),
        _ => None,
    };
    directory.each_name(ACCESS, |leaf| match leaf {
        b"receipt" => Ok(()),
        b"payload" if body.is_some() => Ok(()),
        _ => Err(invalid("DATA move member has an unexpected descendant")),
    })?;
    let receipt = directory
        .opaque_pin_with(c"receipt", ACCESS)?
        .ok_or_else(|| invalid("the original move receipt is absent"))?;
    let canonical = codec::encode(selected)?;
    if read_raw(&receipt, canonical.len())? != canonical {
        return Err(invalid(
            "the named DATA move receipt differs from canonical replay",
        ));
    }
    let payload = if let Some(length) = body {
        let pinned = directory
            .opaque_pin_with(c"payload", ACCESS)?
            .ok_or_else(|| invalid("the original move payload is absent"))?;
        let typed = stamp(&pinned)?;
        if typed.len != length {
            return Err(invalid("DATA move source length differs from its receipt"));
        }
        pinned.require(&directory, c"payload")?;
        Some(typed) // Deliberately no raw-body read here.
    } else {
        absent(&directory, c"payload")?;
        None
    };
    let qualified = Row {
        sequence: selected.sequence,
        operation: selected.mutation.request,
        dir_inode: directory.entity.inode,
        receipt: stamp(&receipt)?,
        payload,
    };
    receipt.require(&directory, c"receipt")?;
    directory.require_named()?;
    Ok(qualified)
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
