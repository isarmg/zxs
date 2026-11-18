//! A decided exchange of a required existing database. This path cannot
//! create an absent predecessor, repair a displaced attempt, or adopt an
//! already-visible successor from its bytes alone.
use crate::database_install::{Envelope, MAX_ENVELOPE, PreparedInstall, Scope, invalid};
use crate::database_install::InstallAdmission;
use crate::disk::{DiskRoot, FileStamp, HeldRegular};
use crate::Result;
use platform_io::fs_safety::{RelativePath, linux::FileIdentity};
use std::{path::Path, sync::Arc};

/// Prospective storage only. An attempt gets its own exclusive directory, one
/// payload and one bounded intent inode (the pending name is renamed in place).
pub(crate) struct OriginalFileStage {
    pub(crate) generation: RelativePath,
    pub(crate) payload: u64,
}
impl OriginalFileStage {
    pub(crate) const INTENT_LIMIT: u64 = MAX_ENVELOPE as u64;
}

pub(crate) struct ExistingExchange {
    install: PreparedInstall,
    original: FileStamp,
}

impl ExistingExchange {
    pub(crate) fn capture(
        retention: &DiskRoot,
        history: &Path,
        entity: &DiskRoot,
        scope: Scope,
        next: Arc<Vec<u8>>,
        maximum: usize,
        verify_original: impl Fn(&[u8]) -> Result<()>,
    ) -> Result<Self> {
        let install = PreparedInstall::capture_held(
            retention, history, entity, (scope, InstallAdmission::DecidedRecovery),
            Arc::clone(&next), maximum, |seen| {
                if seen == next.as_slice() {
                    // The successor was independently admitted by the caller. It
                    // still needs this root's own exchange and real predecessor.
                    Ok(())
                } else {
                    verify_original(seen)
                }
            },
        )?;
        let live = install.old.as_ref().ok_or_else(missing)?;
        let original = match (install.already_current, attempt(&install, retention)?) {
            (false, None) => live.stamp().clone(),
            (false, Some((stage, envelope))) => {
                if envelope.old.as_ref() != Some(live.stamp()) {
                    return Err(missing());
                }
                with_payload(&stage, &envelope.new, maximum, |_| Ok(()))?;
                live.stamp().clone()
            }
            (true, Some((stage, envelope))) => {
                if live.stamp() != &envelope.new {
                    return Err(missing());
                }
                let original = envelope.old.as_ref().ok_or_else(missing)?;
                with_payload(&stage, original, maximum, &verify_original)?;
                original.clone()
            }
            (true, None) => return Err(missing()),
        };
        live.check_running()?;
        Ok(Self { install, original })
    }

    pub(crate) fn reservation(&self) -> (u64, usize) {
        self.install.reservation()
    }

    pub(crate) fn pending_stage(&self) -> Result<Option<OriginalFileStage>> {
        if !self.install.needs_new_stage() {
            return Ok(None);
        }
        Ok(Some(OriginalFileStage {
            generation: self.install.stage_generation()?,
            payload: self.install.source.bytes,
        }))
    }

    pub(crate) fn apply(
        &mut self, retention: &DiskRoot, observe: &mut impl FnMut(&'static str) -> Result<()>,
    ) -> Result<()> {
        self.install.prepare_stage(retention, observe)?;
        let (stage, envelope) = attempt(&self.install, retention)?.ok_or_else(missing)?;
        if envelope.old.as_ref() != Some(&self.original) {
            return Err(missing());
        }
        // Reuse the production two-inode exchange including both directory
        // syncs on an already-completed attempt. Keep the attempt for readback.
        self.install.finish(&stage, envelope, observe)
    }

    /// `live` is a newly read stamp of the actual ordinary database. Accept
    /// only the exact local exchanged inode and its retained, unmodified old
    /// inode. A later replacement cannot become a fresh completion.
    pub(crate) fn require_installed(&self, retention: &DiskRoot, live: &FileStamp) -> Result<()> {
        let (stage, envelope) = attempt(&self.install, retention)?.ok_or_else(missing)?;
        if &envelope.new != live || envelope.old.as_ref() != Some(&self.original) {
            return Err(missing());
        }
        with_payload(&stage, &self.original, self.install.maximum, |_| Ok(()))
    }
}

/// The generic installer distinguishes a completed exchange from a fresh or
/// displaced repair. Only one non-displaced old/new pair is admissible here.
fn attempt<'a>(
    install: &'a PreparedInstall, retention: &DiskRoot,
) -> Result<Option<(DiskRoot, &'a Envelope)>> {
    if install.recovered_displaced {
        return Err(missing());
    }
    let (path, envelope) = match (install.resume.as_ref(), install.completed_tails.as_slice()) {
        (None, []) => return Ok(None),
        (Some((stage, envelope)), []) => (stage.path(), envelope),
        (None, [(path, _, envelope)]) => (path.as_path(), envelope),
        _ => return Err(missing()),
    };
    let identity = FileIdentity { device: envelope.stage_device, inode: envelope.stage_inode };
    let stage = retention.private_child_matching(path, identity)?;
    if stage.read_bytes(&stage.path().join("intent.json"), MAX_ENVELOPE)?
        != crate::disk::encode_metadata(envelope, MAX_ENVELOPE)?
    {
        return Err(missing());
    }
    Ok(Some((stage, envelope)))
}

fn with_payload(
    stage: &DiskRoot, expected: &FileStamp, maximum: usize,
    inspect: impl FnOnce(&[u8]) -> Result<()>,
) -> Result<()> {
    let path = stage.path().join("payload");
    let before = crate::file_stamp::FileStamp::from_metadata(
        &stage.optional_metadata(&path)?.ok_or_else(missing)?,
    );
    let payload = HeldRegular::capture(stage, "payload", maximum)?.ok_or_else(missing)?;
    if payload.stamp() != expected {
        return Err(missing());
    }
    inspect(payload.bytes())?;
    let named = stage.open_read_regular(&path)?.metadata()?;
    let identity = FileIdentity::from_metadata(&named);
    if crate::file_stamp::FileStamp::from_metadata(&named) != before
        || identity.device != expected.device || identity.inode != expected.inode
    {
        return Err(missing());
    }
    payload.check_running()
}

fn missing() -> crate::Error {
    invalid("required original database has no exact local predecessor/exchange pair")
}
