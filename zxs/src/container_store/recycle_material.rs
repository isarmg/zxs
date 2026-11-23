//! Whole-container F→R staging. No visibility switch or source deletion here.
use super::{
    Change, Error, Id, Receipt, codec, create_file, ensure_dir, entry_leaf, invalid, private_dir,
    read, sync_dir, write_file,
};
use crate::{
    fs::{HeldDir, OPAQUE_STAMP_BYTES},
    hash::Sha256,
};
use std::{
    collections::BTreeMap,
    ffi::CString,
    fs::File,
    io::{self, Read, Write},
    os::unix::fs::MetadataExt,
    path::Path,
};

pub(super) const PROOF_ROW: usize = 16 + OPAQUE_STAMP_BYTES * 2;

pub(super) fn entry(original: &HeldDir, leaf: &str) -> io::Result<HeldDir> {
    let native = CString::new(leaf).map_err(|_| invalid("invalid original receipt position"))?;
    let from = original
        .child_directory(&native)?
        .ok_or_else(|| invalid("selected original node directory is absent"))?;
    if !original.same_mount(&from)? {
        return Err(invalid("opaque node directory crosses its enrolled mount"));
    }
    Ok(from)
}

/// Caller keeps exact original container lock and both enrolled root
/// capabilities. This walks only that container's explicit receipts, not a
/// volume or arbitrary pathname. The returned destination is unpublished.
/// Each confirmed node is streamed once and checked against its receipt;
/// metadata-only operations never open a payload.
pub(super) fn copy(
    selected: &BTreeMap<Id, Receipt>,
    original: &HeldDir,
    candidate: &Path,
    proof: &mut File,
    live: impl Fn() -> io::Result<()>,
) -> Result<[u8; 32], Error> {
    original.require_named()?;
    private_dir(candidate)?;
    if original.entity.device == std::fs::metadata(candidate)?.dev() {
        return Err(invalid("recycle storage must be on a distinct filesystem").into());
    }
    live()?;
    let mut proof_hash = Sha256::new();
    for receipt in selected.values() {
        live()?;
        let leaf = entry_leaf(receipt.sequence, receipt.mutation.request);
        let from = entry(original, &leaf)?;
        let to = candidate.join(&leaf);
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
        if !ensure_dir(&to)? {
            return Err(invalid("private recycle candidate is not a fresh object").into());
        }
        if let Change::Save { body, .. } = &receipt.change {
            let pinned = from
                .opaque_pin(c"payload")?
                .ok_or_else(|| invalid("original node bytes absent"))?;
            let mut input = &pinned.file;
            if input.metadata()?.len() != body.length {
                return Err(Error::WrongLength);
            }
            row[16 + OPAQUE_STAMP_BYTES..].copy_from_slice(&pinned.stamp());
            let mut output = create_file(&to.join("payload"))?;
            let mut digest = Sha256::new();
            let mut left = body.length;
            let mut block = [0; 64 * 1024];
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
        write_file(&to.join("receipt"), &canonical)?;
        sync_dir(&to)?;
    }
    proof.sync_all()?;
    sync_dir(candidate)?;
    live()?;
    Ok(proof_hash.finish())
}
