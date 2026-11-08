//! Bounded bytes from the actual isolated image. No path supplied by a caller
//! is opened, and no response is released before its same-pass full digest.
use super::*;
use std::io::Read;

pub(crate) const MAX_RECOVERY_RANGE: usize = 4 * 1024 * 1024;

impl Image<'_> {
    pub(crate) fn read_version_range(
        &self,
        root: &DiskRoot,
        version_id: &str,
        offset: u64,
        length: usize,
        stop: &AtomicBool,
    ) -> Result<(FileVersion, Vec<u8>)> {
        if length > MAX_RECOVERY_RANGE {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "decided backup read exceeds 4 MiB",
            ));
        }
        let (relative, version) = self
            .files
            .iter()
            .find_map(|(path, data)| match data {
                Data::Body { version, .. } if version.version_id == version_id => {
                    Some((path, *version))
                }
                _ => None,
            })
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ContentUnavailable,
                    "version has no AVAILABLE body in this decided frozen image",
                )
            })?;
        let end = offset
            .checked_add(length as u64)
            .filter(|end| *end <= version.logical_size)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ResourceLimit,
                    "recovery range is outside the frozen version",
                )
            })?;
        let mut file = root.open_read_regular(&root.path().join(self.root_name).join(relative))?;
        if file.metadata()?.len() != version.logical_size {
            return Err(Error::new(
                ErrorCode::ChecksumMismatch,
                "decided image payload length differs",
            ));
        }
        let mut result = Vec::with_capacity(length);
        let mut buffer = [0u8; 64 * 1024];
        let mut digest = blake3::Hasher::new();
        let mut position = 0u64;
        loop {
            preparation_checkpoint(stop)?;
            file.check_running()?;
            let read = match file.read(&mut buffer) {
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                value => value?,
            };
            if read == 0 {
                break;
            }
            let next = position
                .checked_add(read as u64)
                .filter(|next| *next <= version.logical_size)
                .ok_or_else(|| {
                    Error::new(ErrorCode::ChecksumMismatch, "decided image payload grew")
                })?;
            digest.update(&buffer[..read]);
            let begin = position.max(offset);
            let finish = next.min(end);
            if begin < finish {
                // Both differences are within this fixed 64 KiB block.
                result.extend_from_slice(
                    &buffer[(begin - position) as usize..(finish - position) as usize],
                );
            }
            position = next;
        }
        if position != version.logical_size
            || file.metadata()?.len() != version.logical_size
            || digest.finalize() != version.content_hash.digest()
            || result.len() != length
        {
            return Err(Error::new(
                ErrorCode::ChecksumMismatch,
                "decided image full content changed during recovery read",
            ));
        }
        file.check_running()?;
        Ok((version.clone(), result))
    }
}
