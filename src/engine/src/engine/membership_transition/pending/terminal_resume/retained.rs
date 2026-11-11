//! Explicitly held original directories keep their old seal and payload bytes.
//! Only control objects under this operation's new namespace are prepared.
use super::*;
use crate::engine::disk_lifecycle::bootstrap::runtime::{management_error, open_store};
use crate::engine::membership_transition::readmission::{ReadmissionRequest, ReadmissionRole};
use crate::engine::membership_transition::terminal_resume::{ResumeAdmission, stopped_source};
use crate::persist::layout;

impl TerminalManagement {
    pub fn prepare_disk_readmission(
        self,
        disk_id: &str,
        principal: &str,
        operation_id: OperationId,
        directory: PendingMemberDirectory,
    ) -> Result<TerminalMetadataResume> {
        Engine::require_actor(principal)?;
        let preview = self.preview_disk_readmission(disk_id)?;
        if self.operation(operation_id).is_some() {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "terminal readmission operation is already in permanent history",
            ));
        }
        if directory.disk_id != disk_id {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "terminal readmission descriptor names another registration",
            ));
        }
        DiskIoBinding::require_closed_lifetimes(&preview.target)?;
        let root = DiskRoot::from_directory(preview.root.clone(), directory.directory)?;
        let physical = admission::verify_named_root(&root)?;
        let store = open_store(self.system_root().path())?;
        store
            .reload(&preview.target.pool_id)
            .map_err(management_error)?
            .verify_stopped_directory(&preview.target, &preview.root, physical)
            .map_err(management_error)?;
        drop(store);
        let directory_lock = root.directory(root.path())?;
        reservation::lock(&directory_lock)?;
        let writer = root.open_regular_entry(&layout::DISK_WRITER_LOCK)?;
        platform_io::fs_safety::linux::SingleLinkRequirement::check(&writer).map_err(fs_error)?;
        reservation::lock(&writer)?;
        let writer = crate::disk::FenceOwner::new(writer, Some(directory_lock));
        reservation::verify_writer(&root, &writer)?;
        let seal = decode_seal(&root.read_entry_bytes(&layout::DISK_IDENTITY, MAX_METADATA)?)?;
        let authority = &self.verified_authority().authority;
        let (stopped_operation, stopped_transition, expected_format) =
            stopped_source(authority, &preview.target)?;
        if seal.format != expected_format {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "retained directory does not carry the seal of its exact stopped lifetime",
            ));
        }
        let mut target = (**authority.original()).clone();
        for disk in &mut target.disks {
            disk.metadata_member = disk.disk_id == disk_id;
        }
        let target = ValidatedConfig::try_from(target)?;
        let disk = target
            .disks
            .iter()
            .find(|disk| disk.disk_id == disk_id)
            .expect("invariant: readmission preview retains the original definition");
        let identity = admission::member_identity(&target, disk, physical);
        let request = ReadmissionRequest {
            target: preview.target,
            directory: identity.clone(),
            original_seal: seal,
            predecessor_registry: preview.registry,
            stopped_operation,
            stopped_transition,
            previous_admission: authority.io_admission_epoch(disk_id)?,
            role: ReadmissionRole::Metadata,
        };
        let archive = self.control_archive();
        let source = PendingControlArchive::from_installed(
            self.system_root().private_child(&archive.archive_path)?,
            &archive.root_hash,
            &archive.transition_hash,
        )?;
        let intent = TransitionIntent {
            operation_id,
            principal: principal.into(),
            origin: Origin::TerminalResume {
                config: authority.original().clone(),
                predecessor_authority_hash: authority
                    .intent()
                    .origin
                    .predecessor()
                    .map(str::to_owned),
                terminal_authority_hash: authority.hash()?,
            },
            target,
            added_roots: BTreeMap::new(),
            effect: None,
            foreign: None,
            fence_scope: None,
            boundary: authority.intent().boundary.clone(),
            fence_sequence: self.management_state().sequence,
        };
        let attempts = super::super::super::terminal_resume::cancellation::attempts::load(
            self.system_root(),
            self.verified_authority(),
        )?;
        let mut proposal = ResumeProposal::prepare_admission(
            self.verified_authority(),
            &source,
            intent,
            ResumeAdmission::RetainedMetadata {
                request: Box::new(request),
            },
        )?;
        proposal.follow_cancellation(attempts.previous_hash()?);
        proposal.preflight_outcomes(&attempts.cancelled)?;
        super::super::super::terminal_resume::cancellation::attempts::synchronize_history(
            self.system_root(),
            self.verified_authority(),
        )?;
        // Durable before writing a new control object in the retained directory.
        attempts.persist_request(self.system_root(), &proposal)?;
        let reservation_name = reservation::name(
            &proposal.intent.origin,
            &proposal.intent.target,
            principal,
            operation_id,
            &identity,
        )?;
        let member = PendingDisk {
            root: Arc::new(root),
            seal: PendingDiskSeal {
                format: PendingFormat::MembershipPending,
                identity,
                transition_hash: object_hash(INTENT_KIND, &proposal.intent)?,
                target_config_hash: object_hash("membership-target", &proposal.intent.target)?,
                control_archive_root: archive.root_hash,
            },
            reservation_name,
            _lock: writer,
        };
        Ok(TerminalMetadataResume {
            management: self,
            source,
            proposal,
            members: BTreeMap::from([(disk_id.into(), member)]),
            votes: BTreeMap::new(),
            installed: BTreeSet::new(),
            ready_data_disks: BTreeSet::new(),
            core: None,
        })
    }
}

impl TerminalMetadataResume {
    pub(super) fn synchronize_admitted_seal(
        &self,
        member: &PendingDisk,
        disk: &DiskConfig,
    ) -> Result<()> {
        match self.proposal.retained_request() {
            Some(request) => {
                request.directory.verify_physical_root(&member.root)?;
                let seal = decode_seal(
                    &member
                        .root
                        .read_bytes(
                            &disk.root.join(layout::DISK_IDENTITY.as_path()),
                            MAX_METADATA,
                        )?,
                )?;
                validate_stored_seal(&seal, &request.original_seal)
            }
            None => {
                admission::inspect_new_root(&member.root, &member.seal, &member.reservation_name)?;
                member
                    .root
                    .write_json(
                        &disk.root.join(layout::DISK_IDENTITY.as_path()),
                        &member.seal,
                        false,
                    )
            }
        }
    }
}
