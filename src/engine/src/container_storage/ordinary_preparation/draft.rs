//! Immutable non-Version draft material inside one already admitted container.
//! Its identity becomes current only through the same SYSTEM/FILE publication.
use super::*;
use crate::file_stamp::FileStamp;
use storage_contract::file_draft::DraftPayload;
use std::io::{Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;

pub(super) struct HeldDraft {
    payload: DraftPayload,
    path: PathBuf,
    file: DiskFile,
    stamp: FileStamp,
}
impl HeldDraft {
    pub(super) fn capture(
        container: &ObservedContainerReplica, payload: DraftPayload,
    ) -> Result<Self> {
        let path = draft_path(&container.held, payload);
        let mut file = container.held.open_leased_regular(&path)?;
        let stamp = FileStamp::from_metadata(&file.metadata()?);
        file.verify_full(platform_io::payload::Expectation {
            bytes: payload.logical_size(), blake3: Some(payload.content_hash().digest()),
        })?;
        let held = Self { payload, path, file, stamp };
        held.validate(container)?;
        Ok(held)
    }

    pub(super) fn validate(&self, container: &ObservedContainerReplica) -> Result<()> {
        if container.record.working.draft().map(|(_, payload)| *payload) != Some(self.payload) {
            return Err(Failure::ContainerScope.into());
        }
        self.file.check_running()?;
        let named = container.held.open_leased_regular(&self.path)?;
        if FileStamp::from_metadata(&self.file.metadata()?) != self.stamp
            || FileStamp::from_metadata(&named.metadata()?) != self.stamp
        {
            return Err(Failure::DraftChanged.into());
        }
        Ok(())
    }

    pub(super) fn open(&self, container: &ObservedContainerReplica) -> Result<ReadFile> {
        self.validate(container)?;
        let mut file = ReadFile::from(self.file.try_clone()?);
        file.seek(SeekFrom::Start(0))?;
        Ok(file)
    }
}

fn draft_path(container: &DiskRoot, payload: DraftPayload) -> PathBuf {
    container.path().join(layout::container::DRAFTS.as_path())
        .join(layout::container::draft_payload(payload.payload_id()).as_path())
}

/// The only writer consumes the actual live plan's sealed payload input after
/// metadata and capacity preflight. No idempotency-by-existing-bytes shortcut.
pub(super) fn populate(
    held: &mut BTreeMap<(FileId, DiskId), ObservedContainerReplica>,
    input: Option<&mut crate::engine::file_draft::DraftPublicationInput>,
    maximum: u64,
) -> Result<()> {
    if let Some(input) = input {
        let file = input.file();
        let payload = input.payload();
        if payload.logical_size() > maximum {
            return Err(Failure::DraftTooLarge.into());
        }
        let mut selected = held.iter_mut().filter(|((id, _), _)| *id == file);
        let (_, target) = selected.next().ok_or(Failure::ContainerScope)?;
        if selected.next().is_some() || target.record.replicas.len() != 1
            || target.record.working.draft().map(|(_, payload)| *payload) != Some(payload)
            || target.disk.seal.disk_id.parse::<DiskId>()? != target.record.owner_disk
        {
            return Err(Failure::ContainerScope.into());
        }
        target.verify_entity()?;
        let path = draft_path(&target.held, payload);
        if target.held.optional_metadata(&path)?.is_some() {
            return Err(Failure::ContainerIdentity.into());
        }
        let directory_path = target.held.path().join(layout::container::DRAFTS.as_path());
        target.held.private_dir(&directory_path)?;
        let directory = target.held.private_child(&directory_path)?;
        input.source().seek(SeekFrom::Start(0))?;
        let copied = directory.copy_new_entry_verified_synced(
            &layout::container::draft_payload(payload.payload_id()), input.source(),
            platform_io::payload::Expectation {
                bytes: payload.logical_size(), blake3: Some(payload.content_hash().digest()),
            },
        ).map_err(|error| match error {
            NewEntryCopyError::Admission(source) | NewEntryCopyError::Published { source, .. } => {
                source
            }
            NewEntryCopyError::Payload(source) => source.into(),
        })?;
        target.held.sync_dir(target.held.path())?;
        target.verify_entity()?;
        let captured = HeldDraft::capture(target, payload)?;
        let metadata = captured.file.metadata()?;
        let identity = FileIdentity { device: metadata.dev(), inode: metadata.ino() };
        if identity != copied.identity {
            return Err(Failure::DraftChanged.into());
        }
        // Carry the observed file from this exact exclusive copy, rather than
        // discarding its receipt and accepting a later equal-byte replacement.
        target.draft = Some(captured);
    }
    // Cold and ordinary follow-up consumers read the exact dirty payload from
    // the same State record. A missing body cannot be an empty-vector success.
    for container in held.values_mut() {
        if let Some((_, payload)) = container.record.working.draft() {
            if payload.logical_size() > maximum {
                return Err(Failure::DraftTooLarge.into());
            }
            if container.draft.is_none() {
                container.draft = Some(HeldDraft::capture(container, *payload)?);
            }
        }
    }
    Ok(())
}
