//! A single prescribed first slot under an explicit terminal target owner.
//! It cannot expose JournalWriter, append a later slot or obtain active I/O.
use super::*;
use crate::engine::membership_transition::FirstSlotOwner;
const MAX_SLOT_BYTES: usize = 16 * 1024;
enum State {
    Scanned(OpenJournal),
    Written(JournalWriter),
}
pub(crate) struct TerminalChainFirstJournal<'a, 'b> {
    owner: &'a FirstSlotOwner<'b>,
    state: Option<State>,
}
impl<'a, 'b> TerminalChainFirstJournal<'a, 'b> {
    pub(crate) fn inspect(owner: &'a FirstSlotOwner<'b>) -> Result<Option<Self>> {
        let Some(io) = owner.journal_io(false)? else {
            return Ok(None);
        };
        Self::open(owner, io).map(Some)
    }
    pub(crate) fn prepare(owner: &'a FirstSlotOwner<'b>) -> Result<Self> {
        let io = owner
            .journal_io(true)?
            .expect("invariant: explicit first-slot directory created");
        Self::open(owner, io)
    }
    fn open(owner: &'a FirstSlotOwner<'b>, mut io: JournalIo) -> Result<Self> {
        owner.recheck()?;
        let scan = io.scan_bounded(JournalNamespace::Active, MAX_SLOT_BYTES)?;
        validate_prefix(&scan, owner)?;
        Ok(Self {
            owner,
            state: Some(State::Scanned(OpenJournal {
                namespace: JournalNamespace::Active,
                io,
                scan,
                #[cfg(feature = "fault-injection")]
                opening_fault: None,
            })),
        })
    }
    fn records(&self) -> Result<&[(Record, String)]> {
        match &self.state {
            Some(State::Scanned(opened)) => Ok(&opened.scan.records),
            Some(State::Written(writer)) => Ok(writer.records()),
            None => Err(Error::new(
                ErrorCode::RecoveryRequired,
                "terminal-chain first-slot I/O failed; reopen original operation",
            )),
        }
    }
    fn receipt(&self, index: usize) -> Result<Option<Receipt>> {
        Ok(self
            .records()?
            .get(index)
            .map(|(record, frame_hash)| Receipt {
                disk_id: self.owner.disk().disk_id.clone(),
                slot_id: self.owner.disk().slot_id.clone(),
                record: record.clone(),
                frame_hash: frame_hash.clone(),
            }))
    }
    pub(crate) fn complete(&self, publication: &str) -> Result<Receipt> {
        let receipt = self.receipt(2)?.ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "terminal-chain target has no complete original Publication frame",
            )
        })?;
        if receipt.record.object_hash != publication
            || self.records()?.len() != 3
            || matches!(&self.state, Some(State::Scanned(opened)) if opened.scan.torn_tail)
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "terminal-chain first-slot Publication differs or has an extra tail",
            ));
        }
        Ok(receipt)
    }
    pub(crate) fn stage_started(&self, index: usize) -> Result<bool> {
        let length = self.records()?.len();
        let torn = match &self.state {
            Some(State::Scanned(opened)) => opened.scan.torn_tail,
            Some(State::Written(writer)) => writer.needs_recovery(),
            None => true,
        };
        Ok(length > index || (length == index && torn))
    }
    pub(crate) fn accept(&mut self) -> Result<Receipt> {
        let hash = self.owner.candidate_hash().to_owned();
        self.append(0, RecordType::Accept, &hash)
    }
    pub(crate) fn finalize(&mut self, finalize: &Finalize) -> Result<Receipt> {
        let own = self.receipt(0)?.ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "terminal-chain FINALIZE requires its actual target ACCEPT",
            )
        })?;
        if finalize.candidate_hash != self.owner.candidate_hash()
            || finalize.recovery_id.is_some()
            || finalize.accepts.as_slice() != std::slice::from_ref(&own)
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "terminal-chain FINALIZE changes actual singleton ACCEPT",
            ));
        }
        self.append(1, RecordType::Finalize, &object_hash("finalize", finalize)?)
    }
    pub(crate) fn publish(&mut self, publication: &Publication) -> Result<Receipt> {
        let own = self.receipt(1)?.ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "terminal-chain Publication requires its actual FINALIZE",
            )
        })?;
        if publication.candidate_hash != self.owner.candidate_hash()
            || publication.finalize_receipts.as_slice() != std::slice::from_ref(&own)
            || publication.recovery.is_some()
            || !publication.payload_receipts.is_empty()
            || canonical_json(&publication.result)? != canonical_json(self.owner.result())?
            || object_hash("finalize", &publication.finalize)? != self.records()?[1].0.object_hash
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "terminal-chain Publication changes exact result or actual FINALIZE",
            ));
        }
        self.append(
            2,
            RecordType::Publication,
            &object_hash("publication", publication)?,
        )
    }
    fn append(&mut self, index: usize, kind: RecordType, hash: &str) -> Result<Receipt> {
        self.owner.recheck()?;
        let saved = self.receipt(index)?;
        if saved.as_ref().is_some_and(|r| r.record.object_hash != hash) {
            return Err(Error::new(
                ErrorCode::LogFork,
                "terminal-chain first slot already selected another object",
            ));
        }
        if saved.is_none() && self.records()?.len() != index {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "terminal-chain first slot cannot skip a frame",
            ));
        }
        let old = self.state.take().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "terminal-chain journal must be reopened after uncertain I/O",
            )
        })?;
        let mut writer = match old {
            State::Scanned(opened) => opened.into_writer()?.0,
            State::Written(writer) => writer,
        };
        writer.io.verify_singleton()?;
        let receipt = match saved {
            Some(receipt) => receipt,
            None => {
                writer.repair_torn_tail()?;
                writer.append(
                    self.owner.disk(),
                    kind,
                    self.owner.sequence(),
                    self.owner.candidate_hash(),
                    hash,
                )?
            }
        };
        writer.io.synchronize(
            #[cfg(feature = "fault-injection")]
            None,
        )?;
        writer.io.verify_singleton()?;
        self.owner.recheck()?;
        self.state = Some(State::Written(writer));
        Ok(receipt)
    }
}
fn validate_prefix(scan: &Scan, owner: &FirstSlotOwner<'_>) -> Result<()> {
    if scan.records.len() > 3 || (scan.records.len() == 3 && scan.torn_tail) {
        return Err(Error::new(
            ErrorCode::LogCorrupt,
            "terminal-chain first slot has unsupported later records/tail",
        ));
    }
    for (index, (record, _)) in scan.records.iter().enumerate() {
        let kind = [
            RecordType::Accept,
            RecordType::Finalize,
            RecordType::Publication,
        ][index];
        if record.record_type != kind
            || record.sequence != owner.sequence()
            || record.candidate_hash != owner.candidate_hash()
            || (index == 0 && record.object_hash != owner.candidate_hash())
            || record.object_hash.len() != 64
            || !record
                .object_hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "terminal-chain log is not the prescribed native first-slot prefix",
            ));
        }
    }
    Ok(())
}

/// Read-only exact native image verification for a generation-directory copy.
/// It exposes no JournalWriter and accepts neither an extra slot nor torn tail.
pub(crate) fn verify_terminal_chain_log_bytes(bytes: &[u8], expected: [&Receipt; 3]) -> Result<()> {
    if bytes.len() > MAX_SLOT_BYTES {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "terminal-chain first log exceeds its bound",
        ));
    }
    let scan = scan_frames(bytes)?;
    if scan.torn_tail || scan.records.len() != expected.len() {
        return Err(Error::new(
            ErrorCode::LogCorrupt,
            "generation log is not the complete original three-frame image",
        ));
    }
    for ((record, hash), receipt) in scan.records.iter().zip(expected) {
        if record != &receipt.record || hash != &receipt.frame_hash {
            return Err(Error::new(
                ErrorCode::LogFork,
                "generation log differs from the original actual Publication receipts",
            ));
        }
    }
    Ok(())
}
