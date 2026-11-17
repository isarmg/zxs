//! Read-only recognition first; only confirmed private unpublished HTTP
//! staging is eligible for removal. Published ZXS files are never opened.
use super::{
    ApiError, EntryName, Owner, PersistedUploadRecord, PrivateDirectory, metadata_and_hash_at,
    metadata_at,
};
use platform_io::fs_safety::PayloadOpenMode;
use std::{collections::BTreeMap, fs, io::Read, os::unix::fs::MetadataExt};

// Limit extra cold I/O. Larger/unproved material stays charged for original retry.
const RESTART_PREFIX_READ_BUDGET: u64 = 64 * 1024 * 1024;

/// This checkpoint is a candidate, not evidence that the actual file is
/// still short. Never discard a full-length or unknown generation here.
fn incomplete_before_publication(name: &str, record: &PersistedUploadRecord) -> bool {
    record.state == crate::uploads::UploadState::Incomplete
        && record.persisted_bytes < record.logical_size
        && record.generation != 0
        && upload_data_name(&record.data_file, "upload-")
        && storage_contract::validate_id(&record.workspace_id, "ws").is_ok()
        && storage_contract::validate_id(&record.item_id, "entry").is_ok()
        && name_policy::PortablePath::parse(&record.path).is_ok()
        && record.persisted_hash.len() == 64
        && record
            .persisted_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && name
            == format!(
                "{}.json",
                crate::uploads::key(&record.workspace_id, &record.item_id)
            )
}

fn upload_data_name(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|name| name.strip_suffix(".data"))
        .is_some_and(|token| {
            token.len() == 43
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
}

// Only a matching actual short prefix, not the progress record on its own,
// proves the ordinary spool file is the interrupted material. The final bytes
// may have reached the file before the producer checkpointed their new length.
pub(super) struct InterruptedPrefix {
    name: String,
    observed_record: blake3::Hash,
}
impl InterruptedPrefix {
    pub(super) fn observe(
        name: &str,
        record: &PersistedUploadRecord,
        observed_record: blake3::Hash,
    ) -> Option<Self> {
        incomplete_before_publication(name, record).then(|| Self {
            name: name.to_owned(),
            observed_record,
        })
    }

    fn verify(
        &self,
        directory: &PrivateDirectory,
        data: &EntryName,
        inventoried: Option<u64>,
        remaining: &mut u64,
    ) -> Result<Option<fs::File>, ApiError> {
        let current = EntryName::new(&self.name).map_err(crate::upload_fs_error)?;
        let (actual, fingerprint) =
            metadata_and_hash_at::<PersistedUploadRecord>(directory, &current)?;
        let length = actual.persisted_bytes;
        if fingerprint != self.observed_record || inventoried != Some(length) || length > *remaining
        {
            return Ok(None);
        }
        let mut held = directory
            .open_payload(data, PayloadOpenMode::Existing)
            .map_err(crate::upload_fs_error)?;
        let before = held.metadata().map_err(ApiError::io)?;
        if before.len() != length {
            return Ok(None);
        }
        *remaining -= length;
        let mut digest = blake3::Hasher::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut pending = length;
        while pending != 0 {
            let size = pending.min(buffer.len() as u64) as usize;
            let n = held.read(&mut buffer[..size]).map_err(ApiError::io)?;
            if n == 0 {
                return Ok(None);
            }
            digest.update(&buffer[..n]);
            pending -= n as u64;
        }
        let after = held.metadata().map_err(ApiError::io)?;
        if after.len() != length
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
            || before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec()
            || digest.finalize().to_hex().as_str() != actual.persisted_hash
        {
            return Ok(None);
        }
        directory
            .verify_payload(data, &held)
            .map_err(crate::upload_fs_error)?;
        Ok(Some(held))
    }
}

pub(super) fn discard_restart_uploads(
    directory: &PrivateDirectory,
    files: &mut BTreeMap<String, u64>,
    owners: &mut BTreeMap<String, String>,
    covered: &mut BTreeMap<String, String>,
    mut interrupted: Vec<(Option<InterruptedPrefix>, String)>,
) -> Result<(), ApiError> {
    let mut references = BTreeMap::<String, usize>::new();
    for data in covered.values() {
        *references.entry(data.clone()).or_default() += 1;
    }
    // One-shot draft/worktree HTTP bodies are only descriptor inputs. The
    // engine never retains a spool name; no resumable generation may reference it.
    interrupted.extend(covered.iter().filter_map(|(metadata, data)| {
        (upload_data_name(data, "complete-upload-") && metadata == &format!("{data}.owner.json"))
            .then(|| (None, data.clone()))
    }));
    let mut remaining = RESTART_PREFIX_READ_BUDGET;
    for (prefix, data) in interrupted {
        let owner = format!("{data}.owner.json");
        let owner_confirmed = covered.get(&owner) == Some(&data);
        let expected = usize::from(prefix.is_some()) + usize::from(owner_confirmed);
        if references.get(&data).copied() != Some(expected) || !owner_confirmed {
            continue;
        }
        directory
            .verify_identity()
            .map_err(crate::upload_fs_error)?;
        let owner_entry = EntryName::new(&owner).map_err(crate::upload_fs_error)?;
        let actual_owner: Owner = metadata_at(directory, &owner_entry)?;
        if actual_owner.data_file != data || owners.get(&data) != Some(&actual_owner.principal) {
            continue;
        }
        let data_entry = EntryName::new(&data).map_err(crate::upload_fs_error)?;
        let held = match &prefix {
            Some(prefix) => match prefix.verify(
                directory,
                &data_entry,
                files.get(&data).copied(),
                &mut remaining,
            )? {
                Some(held) => Some(held),
                // Includes missing/truncated, a full body behind a short
                // checkpoint, different bytes, or an exhausted verification budget.
                None => continue,
            },
            None if files.contains_key(&data) => Some(
                directory
                    .open_payload(&data_entry, PayloadOpenMode::Existing)
                    .map_err(crate::upload_fs_error)?,
            ),
            None => None,
        };
        if let Some(held) = held.as_ref() {
            directory
                .verify_payload(&data_entry, held)
                .map_err(crate::upload_fs_error)?;
            directory
                .remove_file(&data_entry)
                .map_err(crate::upload_fs_error)?;
            files.remove(&data);
        }
        // Never interpret an already missing resumable body as evidence that
        // an earlier cleanup deleted it. Uncertain remaining metadata is charged.
        for name in std::iter::once(&owner).chain(prefix.as_ref().map(|prefix| &prefix.name)) {
            let entry = EntryName::new(name).map_err(crate::upload_fs_error)?;
            directory
                .remove_file(&entry)
                .map_err(crate::upload_fs_error)?;
            files.remove(name);
            covered.remove(name);
        }
        owners.remove(&data);
    }
    Ok(())
}
