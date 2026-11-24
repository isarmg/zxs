//! Keep resumable transmission tied to one frozen local original and one durable remote prefix.
use super::{
    ARCHIVED, BasicLane, Id, Intent, Lease, Lookup, Original, Resolution, Source, Workspace,
    held_lease_at, payload,
};
use crate::{
    Result, invalid,
    session::{
        ExistingBasicSession,
        upload::{Exchange, Progress, Stage, UploadId, block_length, valid_offset},
    },
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::{os::unix::fs::FileExt as _, path::Path, sync::Arc};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Checkpoint {
    upload_id: Option<UploadId>,
    next_offset: u64,
    // The greatest end we armed for a PUT, never advanced by a read-only query.
    issued_to: u64,
}

impl Checkpoint {
    pub(super) fn unstarted() -> Self {
        Self {
            upload_id: None,
            next_offset: 0,
            issued_to: 0,
        }
    }
    pub(super) fn validate(&self, length: u64, phase: &str) -> Result<()> {
        if !(matches!(phase, "attempted" | "confirmed" | "conflict")
            || (phase == "prepared" && self.upload_id.is_none() && self.next_offset == 0))
            || !valid_offset(self.next_offset, length)
            || !valid_offset(self.issued_to, length)
            || self.next_offset > self.issued_to
            || (self.upload_id.is_none() && self.issued_to != 0)
            || (self.next_offset < length
                && self.issued_to
                    > self.next_offset + block_length(self.next_offset, length)? as u64)
        {
            return Err(invalid("invalid local resumable original"));
        }
        Ok(())
    }

    fn next(&self, p: &Progress, length: u64) -> Result<Self> {
        self.validate(length, "attempted")?;
        if self.upload_id.as_ref().is_some_and(|id| id != &p.upload_id) {
            return Err(invalid("the original upload ID changed"));
        }
        if p.next_offset < self.next_offset || p.next_offset > self.issued_to {
            return Err(invalid(
                "the remote offset rolled back or skipped an unissued chunk",
            ));
        }
        Ok(Self {
            upload_id: Some(p.upload_id.clone()),
            next_offset: p.next_offset,
            issued_to: self.issued_to,
        })
    }
}

/// Opaque preview bound to the held workspace. Selecting and showing it is offline.
pub struct UploadSelection {
    owner: Arc<()>,
    original: Original,
}

impl UploadSelection {
    #[must_use]
    pub fn intent(&self) -> &Intent {
        &self.original.intent
    }
    #[must_use]
    pub fn path(&self) -> &str {
        &self.original.path
    }
    #[must_use]
    pub fn basis(&self) -> Option<&super::Basis> {
        self.original.prior.as_ref()
    }
}

/// Only Receipt(Confirmed) is publication. A durable prefix is not a receipt.
#[derive(Debug)]
pub enum UploadObservation {
    Receipt(Resolution),
    NoProgress,
    Staged { next_offset: u64, length: u64 },
}

struct Remaining {
    original: Original,
    lease: Lease,
    phase: String,
    stage: Option<Progress>,
}

impl Workspace {
    /// Select one frozen operation for a receipt or status check.
    ///
    /// # Errors
    /// Refuses an unverified workspace, missing operation, or invalid frozen record.
    pub fn choose_upload(&self, op: &Id) -> Result<UploadSelection> {
        self.validate()?;
        Ok(UploadSelection {
            owner: Arc::clone(&self.read_session_owner),
            original: self.original(op)?.0,
        })
    }

    /// Only show a send preview for a never-attempted freeze or its own resumable lane.
    ///
    /// # Errors
    /// Also rejects oversized bodies and originals attempted on a different lane.
    pub fn choose_upload_for_send(&self, op: &Id) -> Result<UploadSelection> {
        self.validate()?;
        let (original, phase, _) = self.original(op)?;
        if !valid_offset(0, original.intent.body.length)
            || !(phase == "prepared" || (phase == "attempted" && original.upload.is_some()))
        {
            return Err(invalid("this original is only eligible for receipt lookup"));
        }
        Ok(UploadSelection {
            owner: Arc::clone(&self.read_session_owner),
            original,
        })
    }

    fn upload_guard(
        &self,
        basic: &ExistingBasicSession,
        original: &Original,
        lease: &Lease,
    ) -> Result<()> {
        self.validate_basic_lane(BasicLane::Existing(basic))?;
        if self.original(&original.intent.operation_id)?.0 != *original {
            return Err(invalid("the exact frozen upload selection changed"));
        }
        held_lease_at(&self.root, self.db_id, lease)
    }

    // Changes phase and checkpoint together BEFORE authorizing any mutation.
    // Only an operation known never to have used the full-file lane can switch.
    fn pin_upload(&self, original: &mut Original, pin: Checkpoint, lease: &Lease) -> Result<()> {
        let (retained, phase, _) = self.original(&original.intent.operation_id)?;
        let eligible = phase == "prepared" || (phase == "attempted" && original.upload.is_some());
        if retained != *original || !eligible {
            return Err(invalid(
                "local original is not eligible for this upload transition",
            ));
        }
        self.require_live_write_basis(original)?;
        pin.validate(original.intent.body.length, "attempted")?;
        if phase == "attempted" && original.upload.as_ref() == Some(&pin) {
            return held_lease_at(&self.root, self.db_id, lease);
        }
        let mut next = original.clone();
        next.upload = Some(pin);
        held_lease_at(&self.root, self.db_id, lease)?;
        if self.db.execute(
            "UPDATE original SET record=?1, phase='attempted' WHERE op=?2 AND record=?3 AND phase=?4 AND receipt IS NULL",
            params![serde_json::to_string(&next)?, original.intent.operation_id.as_str(), serde_json::to_string(original)?, phase],
        )? != 1 {
            return Err(invalid("original changed before saving upload checkpoint"));
        }
        self.root.sync()?;
        held_lease_at(&self.root, self.db_id, lease)?;
        *original = next;
        Ok(())
    }

    fn save_progress(
        &self,
        original: &mut Original,
        progress: &Progress,
        lease: &Lease,
    ) -> Result<()> {
        let pin = original
            .upload
            .as_ref()
            .ok_or_else(|| invalid("server progress has no locally armed original"))?
            .next(progress, original.intent.body.length)?;
        self.pin_upload(original, pin, lease)
    }

    /// Explicit authentication may look up one original and its progress. This entry never
    /// starts, writes, retries or commits, even if both GETs prove absence.
    ///
    /// # Errors
    /// Refuses changed custody, an unavailable session, or unverified remote evidence.
    pub async fn upload_status(
        &mut self,
        basic: &ExistingBasicSession,
        choice: &UploadSelection,
    ) -> Result<UploadObservation> {
        self.run_upload(basic, choice, false, |_, _| {}).await
    }

    /// One explicit confirmation covers this one original, including its remaining chunks
    /// and commit. Network uncertainty stops the series; it never reconnects or resends.
    ///
    /// # Errors
    /// Stops on missing confirmation, changed custody, or an unverified exchange.
    /// Keep the original after failure and check its receipt before retrying.
    pub async fn send_upload(
        &mut self,
        basic: &ExistingBasicSession,
        choice: &UploadSelection,
        confirmed: bool,
        progress: impl FnMut(u64, u64),
    ) -> Result<UploadObservation> {
        if !confirmed {
            return Err(invalid(
                "confirm this exact resumable original before sending",
            ));
        }
        self.run_upload(basic, choice, true, progress).await
    }

    async fn run_upload(
        &mut self,
        basic: &ExistingBasicSession,
        choice: &UploadSelection,
        send: bool,
        report: impl FnMut(u64, u64),
    ) -> Result<UploadObservation> {
        self.validate_basic_lane(BasicLane::Existing(basic))?;
        if !Arc::ptr_eq(&self.read_session_owner, &choice.owner) {
            return Err(invalid("the upload preview belongs to another workspace"));
        }
        let lease = self.hold_container(&choice.intent().container_id)?;
        let (mut original, phase, prior) = self.original(&choice.intent().operation_id)?;
        if original != choice.original {
            return Err(invalid("upload preview changed; choose the original again"));
        }
        if let Some(sequence) = prior {
            return Ok(UploadObservation::Receipt(self.settle(
                &original,
                Lookup::Confirmed { sequence },
                &lease,
            )?));
        }

        // Login uses the explicitly selected, already verified H2 channel; Basic goes
        // only to the session bootstrap. Every subsequent request uses this Bearer.
        let guard = || self.upload_guard(basic, &original, &lease);
        let peer = Exchange::login(basic, &guard).await?;
        let found = peer.receipt(&original.intent, &guard).await?;
        if found != Lookup::Absent || matches!(phase.as_str(), "conflict" | ARCHIVED) {
            return Ok(UploadObservation::Receipt(
                self.settle(&original, found, &lease)?,
            ));
        }
        // No stage or file is even considered before the original receipt GET.
        let previous = original
            .upload
            .as_ref()
            .and_then(|pin| pin.upload_id.as_ref());
        let stage = peer.status(&original.intent, previous, &guard).await?;
        if stage.as_ref().is_some_and(|p| p.state == Stage::Committed) {
            let got = peer.receipt(&original.intent, guard).await?;
            let result = if matches!(got, Lookup::Confirmed { .. }) {
                got
            } else {
                Lookup::Unknown
            };
            return Ok(UploadObservation::Receipt(
                self.settle(&original, result, &lease)?,
            ));
        }
        if let Some(p) = &stage {
            if phase == "prepared" {
                return Err(invalid(
                    "unexpected remote stage for a never-attempted original",
                ));
            }
            self.save_progress(&mut original, p, &lease)?;
        }
        let length = original.intent.body.length;
        if !send {
            return Ok(stage.map_or(UploadObservation::NoProgress, |p| {
                UploadObservation::Staged {
                    next_offset: p.next_offset,
                    length,
                }
            }));
        }
        self.send_remaining(
            basic,
            &peer,
            Remaining {
                original,
                lease,
                phase,
                stage,
            },
            report,
        )
        .await
    }

    async fn send_remaining(
        &mut self,
        basic: &ExistingBasicSession,
        peer: &Exchange<'_>,
        transfer: Remaining,
        mut report: impl FnMut(u64, u64),
    ) -> Result<UploadObservation> {
        let Remaining {
            mut original,
            lease,
            phase,
            stage,
        } = transfer;
        let length = original.intent.body.length;
        // Prepared bytes were fully hashed and read back before the original was
        // saved. Re-prove the same private, read-only inode (including ctime and
        // length), not a newly captured working file. No O(file) rescan on every
        // login or chunk. The server independently verifies the full digest at commit.
        let source = self
            .root
            .source(Path::new(&payload(&original.intent.operation_id)))?;
        if source.before != original.sealed || source.before.length != length {
            return Err(invalid(
                "original sealed bytes unavailable; only check the original receipt",
            ));
        }
        if phase == "prepared" || original.upload.is_none() {
            self.pin_upload(&mut original, Checkpoint::unstarted(), &lease)?;
        }
        let mut p = if let Some(p) = stage {
            p
        } else {
            let p = peer
                .begin(&original.intent, || {
                    self.upload_write_guard(basic, &original, &source, &lease)
                })
                .await?;
            if p.state == Stage::Committed {
                let found = peer
                    .receipt(&original.intent, || {
                        self.upload_guard(basic, &original, &lease)
                    })
                    .await?;
                let found = if matches!(found, Lookup::Confirmed { .. }) {
                    found
                } else {
                    Lookup::Unknown
                };
                return Ok(UploadObservation::Receipt(
                    self.settle(&original, found, &lease)?,
                ));
            }
            self.save_progress(&mut original, &p, &lease)?;
            p
        };
        report(p.next_offset, length);
        while p.state == Stage::Open {
            self.upload_write_guard(basic, &original, &source, &lease)?;
            peer.live()?;
            let mut bytes = vec![0; block_length(p.next_offset, length)?];
            source.file.read_exact_at(&mut bytes, p.next_offset)?;
            source.check()?;
            self.pin_upload(
                &mut original,
                Checkpoint {
                    upload_id: Some(p.upload_id.clone()),
                    next_offset: p.next_offset,
                    issued_to: p.next_offset + bytes.len() as u64,
                },
                &lease,
            )?;
            p = peer
                .put(&original.intent, &p, bytes.into(), || {
                    self.upload_write_guard(basic, &original, &source, &lease)
                })
                .await?;
            self.save_progress(&mut original, &p, &lease)?;
            report(p.next_offset, length);
        }
        let found = peer
            .commit(&original.intent, &p.upload_id, || {
                self.upload_write_guard(basic, &original, &source, &lease)
            })
            .await?;
        Ok(UploadObservation::Receipt(
            self.settle(&original, found, &lease)?,
        ))
    }

    fn upload_write_guard(
        &self,
        basic: &ExistingBasicSession,
        original: &Original,
        source: &Source,
        lease: &Lease,
    ) -> Result<()> {
        self.validate_basic_lane(BasicLane::Existing(basic))?;
        self.check_attempt(original, source, lease)
    }
}

#[cfg(test)]
mod tests {
    use super::{Checkpoint, Progress, UploadId};
    use serde_json::json;

    #[test]
    fn status_cannot_walk_forward_past_the_only_previously_authorized_put() {
        let u: UploadId = serde_json::from_value(json!("a".repeat(64))).unwrap();
        let length = 3 * 8_388_608;
        let progress = |offset| -> Progress {
            serde_json::from_value(json!({
                "upload_id":u, "container_id":"1".repeat(32), "operation_id":"2".repeat(32),
                "node_id":"3".repeat(32), "target":{"kind":"first_main","name":"a"},
                "length":length,"sha256":"b".repeat(64),"next_offset":offset,"state":"open"
            }))
            .unwrap()
        };
        let armed = Checkpoint::unstarted();
        assert!(armed.next(&progress(8_388_608), length).is_err());
        let pinned = armed.next(&progress(0), length).unwrap();
        assert!(pinned.next(&progress(8_388_608), length).is_err());
        let sent = Checkpoint {
            issued_to: 8_388_608,
            ..pinned
        };
        let recovered = sent.next(&progress(8_388_608), length).unwrap();
        assert!(recovered.next(&progress(0), length).is_err());
        assert!(recovered.next(&progress(16_777_216), length).is_err());
        assert_eq!(
            recovered.next(&progress(8_388_608), length).unwrap(),
            recovered
        );
    }
}
