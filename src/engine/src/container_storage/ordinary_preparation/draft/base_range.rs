//! Per-request authenticated base range. A wider checksum chunk is read and
//! hashed once across adjacent working blocks, not once per 64 KiB sub-block.
//! This is not a cross-request cache or a substitute for held inode checks.
use super::incremental::changed;
use crate::Result;
use std::io;
use storage_contract::ChecksumManifest;
use storage_contract::file_draft::manifest::WORKING_BLOCK_BYTES;

pub(super) struct BaseRange<'a> {
    checksums: &'a ChecksumManifest,
    current: Option<(u64, Vec<u8>)>,
}

impl<'a> BaseRange<'a> {
    pub(super) fn new(checksums: &'a ChecksumManifest) -> Self {
        Self { checksums, current: None }
    }

    pub(super) fn block(
        &mut self,
        index: u64,
        retained: u64,
        output: &mut [u8; WORKING_BLOCK_BYTES as usize],
        mut read: impl FnMut(&mut [u8], u64) -> io::Result<()>,
    ) -> Result<()> {
        output.fill(0);
        let start = index.checked_mul(WORKING_BLOCK_BYTES).ok_or_else(changed)?;
        let end = self.checksums.logical_size.min(retained);
        let count = end.saturating_sub(start).min(WORKING_BLOCK_BYTES) as usize;
        let width = u64::from(self.checksums.chunk_bytes);
        if width == 0 {
            return Err(changed());
        }
        let mut done = 0;
        while done < count {
            let position = start + done as u64;
            let number = position / width;
            let chunk_start = number * width;
            if self.current.as_ref().is_none_or(|(loaded, _)| *loaded != number) {
                // Drop old bytes first: the scratch bound is one admitted
                // checksum chunk, even at a boundary or on an I/O error.
                self.current = None;
                let index = usize::try_from(number).map_err(|_| changed())?;
                let expected = self.checksums.chunks.get(index).ok_or_else(changed)?;
                let length = (self.checksums.logical_size - chunk_start).min(width) as usize;
                let verified = platform_io::payload::read_verified_chunk(
                    length,
                    blake3::Hash::from_bytes(*expected),
                    |bytes| read(bytes, chunk_start),
                )?;
                self.current = Some((number, verified));
            }
            let (_, verified) = self.current.as_ref().expect("invariant: verified chunk installed");
            let offset = (position - chunk_start) as usize;
            let chunk = verified.get(offset..).filter(|bytes| !bytes.is_empty())
                .ok_or_else(changed)?;
            let copy = chunk.len().min(count - done);
            output[done..done + copy].copy_from_slice(&chunk[..copy]);
            done += copy;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/container_storage/working_base_range.rs"]
mod tests;
