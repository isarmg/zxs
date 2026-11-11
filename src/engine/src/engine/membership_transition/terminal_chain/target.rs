//! Original target ownership and real durable decision read-back. No D3 bypass,
//! fresh seal, active journal, locator expansion or root discovery is supplied.
use super::*;
use crate::engine::disk_lifecycle::bootstrap::runtime::{management_error, open_store};
use crate::engine::membership_transition::decision::DecisionKind;
use crate::persist::layout;
pub(super) struct Held {
    pub(super) root: DiskRoot,
    writer: std::sync::Arc<crate::disk::FenceOwner>,
    pub(super) identity: MemberIdentity,
    pub(super) seal: DiskSeal,
}
impl Held {
    pub(super) fn open(
        management: &TerminalManagement,
        proposal: &ChainReadmissionProposal,
        directory: PendingMemberDirectory,
    ) -> Result<Self> {
        let target = proposal.target();
        DiskIoBinding::require_closed_lifetimes(target)?;
        let disk = proposal.target_disk();
        let root = DiskRoot::from_directory(disk.root.clone(), directory.directory)?;
        let physical = admission::verify_named_root(&root)?;
        open_store(management.system_root().path())?
            .reload(&target.pool_id)
            .map_err(management_error)?
            .verify_stopped_directory(target, &disk.root, physical)
            .map_err(management_error)?;
        let directory_lock = root.directory(root.path())?;
        reservation::lock(&directory_lock)?;
        let writer = root.open_regular_entry(&layout::DISK_WRITER_LOCK)?;
        platform_io::fs_safety::linux::SingleLinkRequirement::check(&writer).map_err(fs_error)?;
        reservation::lock(&writer)?;
        let writer = crate::disk::FenceOwner::new(writer, Some(directory_lock));
        reservation::verify_writer(&root, &writer)?;
        let seal = decode_seal(&root.read_entry_bytes(&layout::DISK_IDENTITY, MAX_METADATA)?)?;
        let identity = admission::member_identity(proposal.configuration(), disk, physical);
        let held = Self {
            root,
            writer,
            identity,
            seal,
        };
        held.verify(proposal)?;
        Ok(held)
    }
    pub(super) fn verify(&self, proposal: &ChainReadmissionProposal) -> Result<()> {
        reservation::verify_writer(&self.root, &self.writer)?;
        self.identity.verify_physical_root(&self.root)?;
        let actual = decode_seal(
            &self
                .root
                .read_entry_bytes(&layout::DISK_IDENTITY, MAX_METADATA)?,
        )?;
        validate_stored_seal(&actual, &self.seal)?;
        if &actual.format != proposal.seal_format() {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "chain target does not retain its exact original seal",
            ));
        }
        Ok(())
    }
    pub(super) fn synchronize(
        &self,
        management: &TerminalManagement,
        selection: &Selection,
    ) -> Result<TerminalChainReadmissionStatus> {
        self.verify(&selection.proposal)?;
        if activation::started(management)? {
            self.verify_decision(selection)?;
            require_selected(management, selection)?;
            let mut status = management.chain_readmission_status()?.ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "selected chain request disappeared",
                )
            })?;
            status.target_reverified_this_call = true;
            return Ok(status);
        }
        let source = management.control_archive();
        let archive = PendingControlArchive::from_installed(
            management
                .system_root()
                .private_child(&source.archive_path)?,
            &source.root_hash,
            &source.transition_hash,
        )?;
        let hash = selection.hash()?;
        let directory = pending::member_directory(selection.proposal.target_disk(), &hash);
        self.root.private_dir(&directory)?;
        let parent = self.root.private_child(&directory)?;
        parent.write_json(
            &parent.path().join("chain-selection.json"),
            selection,
            false,
        )?;
        let archive_path = parent.path().join(pending::CONTROL_ARCHIVE_DIRECTORY);
        parent.private_dir(&archive_path)?;
        let installed = archive.install(parent.private_child(&archive_path)?)?;
        let installed = PendingControlArchive::from_installed(
            installed,
            &source.root_hash,
            &source.transition_hash,
        )?;
        let verified = selection
            .terminal
            .clone()
            .verify_installed_archive(&installed)?;
        selection.verify(&verified)?;
        self.verify(&selection.proposal)?;
        // DecisionStore owns the real atomic slot and fsync/readback barriers.
        // Planning a certificate above never substitutes for this local vote.
        require_selected(management, selection)?;
        let store = DecisionStore::open(&self.root, &self.identity, &hash, true)?;
        let decision = store.receipt(
            DecisionKind::TerminalChainReadmission,
            &hash,
            &selection.acceptance()?,
            true,
            #[cfg(feature = "fault-injection")]
            None,
        )?;
        let certificate = Certificate {
            selection_hash: hash.clone(),
            decision,
        };
        selection.verify_certificate(&certificate)?;
        parent.write_json(
            &parent.path().join("chain-certificate.json"),
            &certificate,
            false,
        )?;
        let saved: Selection = decode_strict(
            &parent.read_bytes(&parent.path().join("chain-selection.json"), MAX_METADATA)?,
        )?;
        if saved.hash()? != hash {
            return Err(Error::new(
                ErrorCode::LogFork,
                "installed chain target selection changed",
            ));
        }
        let saved: Certificate = decode_strict(
            &parent.read_bytes(&parent.path().join("chain-certificate.json"), MAX_METADATA)?,
        )?;
        selection.verify_certificate(&saved)?;
        store.verify_receipt(&saved.decision, true)?;
        self.verify(&selection.proposal)?;
        require_selected(management, selection)?;
        management
            .system_root()
            .write_json(&operation(management).join(RECEIPT), &saved, false)?;
        selection.status(Some(&saved), true)
    }
}

impl Held {
    /// A selected first-slot preparation may only reread the original complete
    /// target decision. Missing evidence is not a reason to choose/install anew.
    pub(super) fn verify_decision(
        &self,
        selection: &Selection,
    ) -> Result<(Certificate, VerifiedTerminalAuthority)> {
        self.verify(&selection.proposal)?;
        if self.identity != selection.directory {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "terminal-chain target changed its selected physical identity",
            ));
        }
        validate_stored_seal(&self.seal, &selection.original_seal)?;
        let hash = selection.hash()?;
        let parent = self.root.private_child(&pending::member_directory(
            selection.proposal.target_disk(),
            &hash,
        ))?;
        let saved: Selection = decode_strict(
            &parent.read_bytes(&parent.path().join("chain-selection.json"), MAX_METADATA)?,
        )?;
        if saved.hash()? != hash {
            return Err(Error::new(
                ErrorCode::LogFork,
                "terminal-chain target retains another source selection",
            ));
        }
        let certificate: Certificate = decode_strict(
            &parent.read_bytes(&parent.path().join("chain-certificate.json"), MAX_METADATA)?,
        )?;
        selection.verify_certificate(&certificate)?;
        let archive = PendingControlArchive::from_installed(
            parent.private_child(&parent.path().join(pending::CONTROL_ARCHIVE_DIRECTORY))?,
            selection.terminal.archive_root_hash(),
            &selection.terminal.transition_hash()?,
        )?;
        let source = selection
            .terminal
            .clone()
            .verify_installed_archive(&archive)?;
        selection.verify(&source)?;
        let store = DecisionStore::open(&self.root, &self.identity, &hash, false)?;
        let actual = store.receipt(
            DecisionKind::TerminalChainReadmission,
            &hash,
            &selection.acceptance()?,
            false,
            #[cfg(feature = "fault-injection")]
            None,
        )?;
        if actual != certificate.decision {
            return Err(Error::new(
                ErrorCode::LogFork,
                "terminal-chain target certificate lost its actual immutable vote",
            ));
        }
        store.verify_receipt(&actual, true)?;
        self.verify(&selection.proposal)?;
        Ok((certificate, source))
    }
}
