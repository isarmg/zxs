//! Read-only cost and content evidence. There is deliberately no physical
//! pathname, inode, ReadSource, install, or receipt in this product.
use super::{ReadWorkingDigest, changed};
use crate::container_storage::PreparedUnbornFull;
use crate::engine::{BudgetedDirectoryFirst, PlanningContext, file_draft::SelectedWorkingSources};
use crate::{Result, ChecksumManifest};
use std::sync::Arc;
use storage_contract::ContentHash;

pub(crate) struct MeasuredUnbornFull {
    selection: Arc<SelectedWorkingSources>,
    index: usize,
    hash: ContentHash,
    manifest_hash: blake3::Hash,
    chunk_bytes: u32,
}

impl MeasuredUnbornFull {
    pub(crate) fn capture(
        context: &PlanningContext<'_>, selection: &Arc<SelectedWorkingSources>, index: usize,
    ) -> Result<Arc<Self>> {
        selection.require_context(context)?;
        let selected = selection.sources().get(index).ok_or_else(changed)?;
        let source = selected.unborn_reader()?;
        source.require_record(selected.target().file, selected.record())?;
        let size = source.logical_size();
        let chunk_bytes = context.config.checksum_chunk_bytes;
        let mut offset = 0u64;
        let mut content = blake3::Hasher::new();
        let mut chunks = Vec::new();
        let mut buffer = vec![0u8; size.min(u64::from(chunk_bytes)) as usize];
        while offset < size {
            let width = (size - offset).min(chunk_bytes.into()) as usize;
            let read = source.read_at(&mut buffer[..width], offset)?;
            if read != width { return Err(changed()); }
            content.update(&buffer[..read]);
            chunks.push(*blake3::hash(&buffer[..read]).as_bytes());
            offset += read as u64;
        }
        let checksums = ChecksumManifest { logical_size: size, chunk_bytes, chunks };
        let manifest_hash = blake3::hash(&checksums.encode()?);
        // The per-file chunk array is not retained across the selection.
        selection.require_context(context)?;
        Ok(Arc::new(Self {
            selection: Arc::clone(selection), index,
            hash: ContentHash::from_digest(content.finalize()), manifest_hash, chunk_bytes,
        }))
    }

    pub(crate) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        self.selection.require_context(context)?;
        if self.chunk_bytes != context.config.checksum_chunk_bytes { return Err(changed()); }
        self.digest()?.require_source()
    }

    pub(crate) fn digest(&self) -> Result<ReadWorkingDigest<'_>> {
        let selected = self.selection.sources().get(self.index).ok_or_else(changed)?;
        let source = selected.unborn_reader()?;
        source.require_record(selected.target().file, selected.record())?;
        Ok(ReadWorkingDigest { source, hash: self.hash })
    }

    pub(crate) fn manifest_hash(&self) -> blake3::Hash { self.manifest_hash }

    pub(crate) fn realize(
        &self, context: &PlanningContext<'_>, permit: BudgetedDirectoryFirst,
    ) -> Result<Arc<PreparedUnbornFull>> {
        self.require_context(context)?;
        let reservation = permit.into_reservation(self)?;
        let full = PreparedUnbornFull::prepare_ordinary(
            context, Arc::clone(&self.selection), self.index, Some((self.hash, reservation)),
        )?;
        full.require_selection(&self.selection, self.index)?;
        if full.digest()?.content_hash() != self.hash { return Err(changed()); }
        Ok(full)
    }
}
