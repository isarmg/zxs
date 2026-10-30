//! The shared HTTP/SMB verification boundary: expose the exact buffer hashed.
use std::io::{Read, Seek, SeekFrom};

pub(crate) fn validate_manifest(
    manifest: &storage_contract::ChecksumManifest,
    size: u64,
) -> Result<(), crate::ApiError> {
    if !storage_contract::SUPPORTED_CHECKSUM_CHUNK_BYTES.contains(&manifest.chunk_bytes)
        || manifest.logical_size != size
        || manifest.chunks.len() as u64 != size.div_ceil(u64::from(manifest.chunk_bytes))
    {
        return Err(crate::ApiError::invalid("invalid checksum manifest"));
    }
    Ok(())
}

pub(crate) struct VerifiedChunk {
    base: u64,
    bytes: bytes::Bytes,
}

impl VerifiedChunk {
    /// A shared slice retains the authenticated allocation, including after the
    /// next read evicts this chunk. No payload reread or unverified tail escapes.
    pub(crate) fn range(&self, offset: u64, length: usize) -> Option<bytes::Bytes> {
        let from = usize::try_from(offset.checked_sub(self.base)?).ok()?;
        if from >= self.bytes.len() {
            return None;
        }
        let count = length.min(self.bytes.len() - from);
        Some(self.bytes.slice(from..from + count))
    }
}

#[derive(Debug, error_definition::Error)]
#[error("CHECKSUM_MISMATCH: no replica has a verified payload chunk")]
pub(crate) struct VerifiedChunkUnavailable {
    #[source]
    io: Option<std::io::Error>,
}

/// The caller has admitted the manifest and an offset inside its logical extent.
pub(crate) fn read<R: Read + Seek>(
    file: &mut R,
    fallbacks: &mut [R],
    manifest: &storage_contract::ChecksumManifest,
    offset: u64,
) -> std::io::Result<VerifiedChunk> {
    let chunk_bytes = u64::from(manifest.chunk_bytes);
    let index = offset / chunk_bytes;
    let base = index * chunk_bytes;
    let count = chunk_bytes.min(manifest.logical_size - base) as usize;
    let mut buffer = vec![0; count];
    let expected = &manifest.chunks[index as usize];
    let mut verified_source = None;
    let mut first_io_error = None;
    for (source, file) in std::iter::once(&mut *file)
        .chain(fallbacks.iter_mut())
        .enumerate()
    {
        match file
            .seek(SeekFrom::Start(base))
            .and_then(|_| file.read_exact(&mut buffer))
        {
            Ok(()) => {
                if blake3::hash(&buffer).as_bytes() == expected {
                    verified_source = Some(source);
                    break;
                }
            }
            Err(error) => {
                first_io_error.get_or_insert(error);
            }
        }
    }
    let Some(source) = verified_source else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            VerifiedChunkUnavailable { io: first_io_error },
        ));
    };
    if source != 0 {
        std::mem::swap(file, &mut fallbacks[source - 1]);
    }
    Ok(VerifiedChunk {
        base,
        bytes: bytes::Bytes::from(buffer),
    })
}
