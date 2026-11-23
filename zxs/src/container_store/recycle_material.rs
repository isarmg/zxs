//! Whole-container F→R staging. No visibility switch or source deletion here.
mod tree;

use super::{Change, Error, FlatStage, Id, Receipt, codec, entry_leaf, held_entry, invalid, read};
use crate::{
    fs::{HeldDir, OPAQUE_STAMP_BYTES},
    hash::Sha256,
};
use std::{
    collections::BTreeMap,
    ffi::CString,
    fs::File,
    io::{self, Read, Write},
};
pub(super) use tree::CopyTree;

pub(super) const PROOF_ROW: usize = 16 + OPAQUE_STAMP_BYTES * 2;

/// Caller keeps exact original container lock and both enrolled root
/// capabilities. This walks only that container's explicit receipts, not a
/// volume or arbitrary pathname. The returned destination is unpublished.
/// Each confirmed node is streamed once and checked against its receipt;
/// metadata-only operations never open a payload.
pub(super) fn copy(
    selected: &BTreeMap<Id, Receipt>,
    original: &HeldDir,
    candidate: &mut CopyTree,
    proof: &mut File,
    live: impl Fn() -> io::Result<()>,
) -> Result<[u8; 32], Error> {
    original.require_named()?;
    candidate.require_named()?;
    if original.entity.device == candidate.directory().entity.device {
        return Err(invalid("recycle storage must be on a distinct filesystem").into());
    }
    live()?;
    let mut proof_hash = Sha256::new();
    for receipt in selected.values() {
        live()?;
        let leaf = entry_leaf(receipt.sequence, receipt.mutation.request);
        let from = held_entry(original, &leaf)?;
        let name = CString::new(leaf).map_err(|_| invalid("invalid recycle entry name"))?;
        let pinned_receipt = from
            .opaque_pin(c"receipt")?
            .ok_or_else(|| invalid("stored source receipt absent"))?;
        let canonical = codec::encode(receipt)?;
        let mut actual = Vec::new();
        (&pinned_receipt.file)
            .take((codec::MAX_RECEIPT + 1) as u64)
            .read_to_end(&mut actual)?;
        if canonical != actual {
            return Err(invalid("held source receipt differs from its selection").into());
        }
        let mut row = [0; PROOF_ROW];
        row[..8].copy_from_slice(&from.entity.device.to_be_bytes());
        row[8..16].copy_from_slice(&from.entity.inode.to_be_bytes());
        row[16..16 + OPAQUE_STAMP_BYTES].copy_from_slice(&pinned_receipt.stamp());
        candidate.reserve_entry()?;
        let mut to = FlatStage::new(candidate.directory(), name)?;
        if let Change::Save { body, .. } = &receipt.change {
            let pinned = from
                .opaque_pin(c"payload")?
                .ok_or_else(|| invalid("original node bytes absent"))?;
            let mut input = &pinned.file;
            if input.metadata()?.len() != body.length {
                return Err(Error::WrongLength);
            }
            row[16 + OPAQUE_STAMP_BYTES..].copy_from_slice(&pinned.stamp());
            let output = to.create(c"payload")?;
            let mut digest = Sha256::new();
            let mut left = body.length;
            let mut block = vec![0; 64 * 1024];
            while left != 0 {
                live()?;
                let take = usize::try_from(left.min(block.len() as u64))
                    .map_err(|_| invalid("invalid recycle node window"))?;
                let n = read(&mut input, &mut block[..take])?;
                if n == 0 {
                    return Err(Error::WrongLength);
                }
                digest.update(&block[..n])?;
                output.write_all(&block[..n])?;
                left -= n as u64;
            }
            if read(&mut input, &mut block[..1])? != 0 {
                return Err(Error::WrongLength);
            }
            if digest.finish() != body.sha256 {
                return Err(Error::WrongDigest);
            }
            pinned.require(&from, c"payload")?;
            output.sync_all()?;
        }
        pinned_receipt.require(&from, c"receipt")?;
        proof.write_all(&row)?;
        proof_hash.update(&row)?;
        to.write(c"receipt", &canonical)?;
        let sealed = to.seal()?;
        candidate.record_entry(sealed);
    }
    proof.sync_all()?;
    candidate.directory().file.sync_all()?;
    candidate.require_named()?;
    live()?;
    Ok(proof_hash.finish())
}
