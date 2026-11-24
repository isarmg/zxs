//! Whole-container F→R staging. No visibility switch or source deletion here.
mod tree;

use super::{Change, Error, FlatStage, Id, Receipt, codec, entry_leaf, held_entry, invalid};
use crate::{
    fs::{Entity, HeldDir, OPAQUE_STAMP_BYTES, OpaquePin},
    hash::Sha256,
};
use std::{
    collections::BTreeMap,
    ffi::CString,
    fs::File,
    io::{self, Read, Write},
};
pub(super) use tree::CopyTree;

const LEGACY_ROW: usize = 16 + OPAQUE_STAMP_BYTES * 2;
const STABLE_STAMP: usize = OPAQUE_STAMP_BYTES - 8;
const STABLE_ROW: usize = 8 + STABLE_STAMP * 2;

// Row version derives only from the exact selector and ready record.
#[derive(Clone, Copy)]
pub(super) enum ProofFormat {
    Legacy,
    Enrolled,
}
impl ProofFormat {
    fn width(self) -> usize {
        match self {
            Self::Legacy => LEGACY_ROW,
            Self::Enrolled => STABLE_ROW,
        }
    }
    fn directory_bytes(self) -> usize {
        match self {
            Self::Legacy => 16,
            Self::Enrolled => 8,
        }
    }
    fn stamp_bytes(self) -> usize {
        match self {
            Self::Legacy => OPAQUE_STAMP_BYTES,
            Self::Enrolled => STABLE_STAMP,
        }
    }
    fn stamp(self, full: &[u8; OPAQUE_STAMP_BYTES]) -> &[u8] {
        &full[OPAQUE_STAMP_BYTES - self.stamp_bytes()..]
    }
}

pub(super) struct SourceRow {
    raw: [u8; LEGACY_ROW],
    format: ProofFormat,
}
impl SourceRow {
    pub(super) fn read(
        input: &mut impl Read,
        format: ProofFormat,
        payload: bool,
    ) -> io::Result<Self> {
        let mut row = Self {
            raw: [0; LEGACY_ROW],
            format,
        };
        input.read_exact(&mut row.raw[..format.width()])?;
        if matches!(format, ProofFormat::Enrolled) {
            let (receipt, body) = row.members();
            if row.raw[..8] == [0; 8]
                || receipt[..8] == [0; 8]
                || (body[..8] != [0; 8]) != payload
                || !payload && body.iter().any(|b| *b != 0)
            {
                return Err(invalid("noncanonical enrolled recycle source proof row"));
            }
        }
        Ok(row)
    }

    fn enrolled(directory: Entity, receipt: &[u8; OPAQUE_STAMP_BYTES]) -> Self {
        let mut row = Self {
            raw: [0; LEGACY_ROW],
            format: ProofFormat::Enrolled,
        };
        row.raw[..8].copy_from_slice(&directory.inode.to_be_bytes());
        row.raw[8..8 + STABLE_STAMP].copy_from_slice(row.format.stamp(receipt));
        row
    }

    fn bind_body(&mut self, stamp: &[u8; OPAQUE_STAMP_BYTES]) {
        let format = self.format;
        let start = format.directory_bytes() + format.stamp_bytes();
        self.raw[start..format.width()].copy_from_slice(format.stamp(stamp));
    }

    pub(super) fn bytes(&self) -> &[u8] {
        &self.raw[..self.format.width()]
    }

    fn members(&self) -> (&[u8], &[u8]) {
        self.bytes()[self.format.directory_bytes()..].split_at(self.format.stamp_bytes())
    }

    pub(super) fn require_directory(&self, observed: Entity, same_mount: bool) -> io::Result<()> {
        let start = self.format.directory_bytes() - 8;
        if !same_mount
            || self.raw[start..start + 8] != observed.inode.to_be_bytes()
            || matches!(self.format, ProofFormat::Legacy)
                && self.raw[..8] != observed.device.to_be_bytes()
        {
            return Err(invalid(
                "source member is not the same copied original directory",
            ));
        }
        Ok(())
    }

    pub(super) fn require_members(
        &self,
        receipt: Option<&OpaquePin>,
        body: Option<&OpaquePin>,
    ) -> io::Result<()> {
        let (expected_receipt, expected_body) = self.members();
        if receipt.is_some_and(|pin| self.format.stamp(&pin.stamp()) != expected_receipt)
            || body.is_some_and(|pin| self.format.stamp(&pin.stamp()) != expected_body)
            || receipt.is_none() && body.is_some()
        {
            return Err(invalid(
                "old source names no longer identify equal copied originals",
            ));
        }
        Ok(())
    }
}

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
        let mut row = SourceRow::enrolled(from.entity, &pinned_receipt.stamp());
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
            row.bind_body(&pinned.stamp());
            let output = to.create(c"payload")?;
            body.copy_exact(&mut input, output, &live)?;
            pinned.require(&from, c"payload")?;
            output.sync_all()?;
        }
        pinned_receipt.require(&from, c"receipt")?;
        proof.write_all(row.bytes())?;
        proof_hash.update(row.bytes())?;
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

/// Recovering a durable R copy without the original copying process must
/// re-read every exact archived payload before it can select or retire DATA.
/// Normal receipt lookup does not require a full payload scan.
pub(super) fn verify_recovered(
    selected: &BTreeMap<Id, Receipt>,
    archive: &HeldDir,
    live: impl Fn() -> io::Result<()>,
) -> io::Result<()> {
    live()?;
    archive.require_named()?;
    for receipt in selected.values() {
        let Change::Save { body, .. } = &receipt.change else {
            continue;
        };
        live()?;
        let entry = held_entry(
            archive,
            &entry_leaf(receipt.sequence, receipt.mutation.request),
        )?;
        let original = entry
            .opaque_pin(c"payload")?
            .ok_or_else(|| invalid("prepared archive is missing a selected payload"))?;
        if original.file.metadata()?.len() != body.length {
            return Err(invalid("prepared archived payload differs in length"));
        }
        body.copy_exact(&mut &original.file, &mut io::sink(), &live)
            .map_err(|error| match error {
                Error::Io(error) => error,
                _ => invalid("prepared archived payload failed complete verification"),
            })?;
        original.require(&entry, c"payload")?;
    }
    archive.require_named()?;
    live()
}

#[cfg(test)]
#[path = "../../tests/unit/recycle_source_proof.rs"]
mod tests;
