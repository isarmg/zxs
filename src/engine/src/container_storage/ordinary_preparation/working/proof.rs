//! Exact complete raw working-pack evidence borrowed from one admitted FILE.
//! This operation is separate from serving a range; no paths or manifests
//! provided by a caller are accepted as physical authority.
use super::{HeldPack, ObservedContainerReplica, changed};
use crate::container_storage::ordinary_preparation::draft::HeldDraft;
use crate::{Error, ErrorCode, Result};
use std::{collections::BTreeMap, sync::atomic::{AtomicBool, Ordering}};
use storage_contract::file_draft::{UnbornBody, manifest::{WorkingManifest, packed::{PackId, WorkingPack}}};

#[derive(Debug, error_definition::Error)]
#[error("complete working pack observation was stopped; no partial inventory is admitted")]
struct Stopped;
fn running(stop: &AtomicBool) -> Result<()> {
    if stop.load(Ordering::Acquire) {
        Err(Error::new(ErrorCode::OperationAborted, Stopped.to_string()))
    } else { Ok(()) }
}

/// The hash covers physical data PLUS the complete original trailer in order.
/// Neither this hash nor the pack's index digest denotes a logical FULL body.
pub(crate) struct VerifiedWorkingPackFile {
    pack: WorkingPack,
    raw: blake3::Hash,
}
impl VerifiedWorkingPackFile {
    pub(crate) fn pack(&self) -> WorkingPack { self.pack }
    pub(crate) fn physical_bytes(&self) -> u64 { self.pack.physical_bytes() }
    pub(crate) fn raw_blake3(&self) -> blake3::Hash { self.raw }
}

pub(crate) struct CapturedPackedWorkingFiles<'a> {
    scope: &'a ObservedContainerReplica,
    manifest: &'a WorkingManifest,
    files: Vec<VerifiedWorkingPackFile>,
}
impl CapturedPackedWorkingFiles<'_> {
    pub(crate) fn manifest(&self) -> &WorkingManifest { self.manifest }
    pub(crate) fn files(&self) -> &[crate::container_storage::VerifiedWorkingPackFile] { &self.files }
    /// Scope verification rechecks the selected container/parent and every
    /// held/named pack stamp. Native callers also keep their own current FILE
    /// row/registration/publication check; this loan cannot grant a decision.
    pub(crate) fn require_current(&self) -> Result<()> { self.scope.verify() }
}

impl ObservedContainerReplica {
    fn packed_material(&self) -> Result<Option<(&WorkingManifest, &BTreeMap<PackId, HeldPack>)>> {
        if let Some(unborn) = self.record.working.unborn() {
            return match unborn.body() {
                UnbornBody::Empty {} => Ok(None),
                UnbornBody::Saved { manifest } => {
                    let held = self.unborn.as_ref().ok_or_else(changed)?;
                    Ok(Some((manifest, &held.packs)))
                }
            };
        }
        match self.record.working.incremental() {
            None => Ok(None),
            Some(manifest) => match &self.draft {
                Some(HeldDraft::Incremental(held)) => Ok(Some((manifest.material(), held.packs()))),
                Some(HeldDraft::Full(_)) | None => Err(changed()),
            },
        }
    }

    /// None means only that this exact recorded working state has no packed
    /// body. A saved sparse/all-zero body instead returns Some with its real
    /// manifest and potentially no files. Unknown partials are never walked.
    pub(crate) fn capture_packed_working_files(&self, stop: &AtomicBool) -> Result<Option<crate::container_storage::CapturedPackedWorkingFiles<'_>>> {
        running(stop)?;
        self.verify()?;
        let Some((manifest, held)) = self.packed_material()? else { return Ok(None); };
        if manifest.file() != self.file || manifest.packs().len() != held.len() { return Err(changed()); }
        let mut files = Vec::with_capacity(held.len());
        for &pack in manifest.packs() {
            running(stop)?;
            let handle = held.get(&pack.id()).ok_or_else(changed)?;
            if handle.evidence() != pack { return Err(changed()); }
            let raw = handle.raw_blake3(&self.held, || running(stop))?;
            files.push(VerifiedWorkingPackFile { pack, raw });
        }
        let complete = CapturedPackedWorkingFiles { scope: self, manifest, files };
        complete.require_current()?;
        running(stop)?;
        Ok(Some(complete))
    }
}
