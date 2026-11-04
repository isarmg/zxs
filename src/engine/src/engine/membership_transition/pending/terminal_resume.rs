//! Explicit terminal resumption holds SYSTEM and all selected fresh directories.
//! Every new member receives and rereads the complete old archive before voting.
use super::super::terminal_resume::{ResumeCore, ResumeProposal, validate_target};
use super::*;
use crate::engine::disk_lifecycle::terminal::TerminalManagement;
use reservation::terminal_resume::ResumeRoot;
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalMetadataResumeStatus {
    pub operation_id: OperationId,
    pub terminal_authority_hash: String,
    pub transition_hash: String,
    pub original_metadata_members: usize,
    pub original_metadata_quorum: usize,
    pub target_metadata_members: usize,
    pub target_metadata_quorum: usize,
    pub synchronized_target_votes: Vec<String>,
    pub installed_certificates: Vec<String>,
}
impl TerminalMetadataResumeStatus {
    pub fn target_quorum_accepted(&self) -> bool {
        self.synchronized_target_votes.len() >= self.target_metadata_quorum
    }
    pub fn all_control_metadata_synchronized(&self) -> bool {
        self.synchronized_target_votes.len() == self.target_metadata_members
            && self.installed_certificates.len() == self.target_metadata_members
    }
}

/// Owns the original management lock and immutable request. Status is observed
/// preparation only; neither Q nor a certificate is an ordinary operation result.
pub struct TerminalMetadataResume {
    management: TerminalManagement,
    source: PendingControlArchive,
    proposal: ResumeProposal,
    members: BTreeMap<String, PendingDisk>,
    votes: BTreeMap<String, super::super::decision::DecisionReceipt>,
    installed: BTreeSet<String>,
    core: Option<ResumeCore>,
}

impl TerminalManagement {
    /// Select fresh metadata directories for this exact completed terminal
    /// authority. All old registrations remain unchanged and no old root opens.
    /// On failure, reopen the same management request, never initialize Genesis.
    pub fn prepare_metadata_resume(
        self,
        target: Config,
        principal: &str,
        operation_id: OperationId,
        directories: Vec<PendingMemberDirectory>,
    ) -> Result<TerminalMetadataResume> {
        Engine::require_actor(principal)?;
        let target = ValidatedConfig::try_from(target)?;
        let verified = self.verified_authority();
        validate_target(verified.authority.original(), &target)?;
        if self.operation(operation_id).is_some() {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "resume operation already belongs to permanent history",
            ));
        }
        if target.disks.iter().any(|disk| {
            disk.root.starts_with(self.system_root().path())
                || self.system_root().path().starts_with(&disk.root)
        }) {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "resume storage overlaps protected system state",
            ));
        }
        let archive = self.control_archive();
        let source = PendingControlArchive::from_installed(
            self.system_root().private_child(&archive.archive_path)?,
            &archive.root_hash,
            &archive.transition_hash,
        )?;
        let roots = hold_roots(&self, &target, directories, &source)?;
        let authority = &verified.authority;
        let intent = TransitionIntent {
            operation_id,
            principal: principal.to_owned(),
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
            added_roots: roots
                .iter()
                .map(|root| (root.disk.disk_id.clone(), root.identity.clone()))
                .collect(),
            effect: None,
            fence_scope: None,
            boundary: authority.intent().boundary.clone(),
            fence_sequence: self.management_state().sequence,
        };
        let proposal = ResumeProposal::prepare(verified, &source, intent)?;
        for root in &roots {
            root.inspect(&proposal.intent, &archive.root_hash)?;
        }
        let path = resume_path(&self);
        if let Some(file) = self.system_root().open_optional_read_regular(&path)? {
            let saved: ResumeProposal = decode_strict(&read_file_bytes(file, MAX_METADATA)?)?;
            if saved.hash()? != proposal.hash()? {
                return Err(Error::new(
                    ErrorCode::IdempotencyKeyReused,
                    "terminal authority already reserves another exact resume request",
                ));
            }
        }
        self.system_root().write_json(&path, &proposal, false)?;
        let mut members = BTreeMap::new();
        for root in roots {
            let disk_id = root.disk.disk_id.clone();
            let reserved = root.reserve(&proposal.intent, &archive.root_hash)?;
            let seal = PendingDiskSeal {
                format: PendingFormat::MembershipPending,
                identity: reserved.identity,
                transition_hash: object_hash(INTENT_KIND, &proposal.intent)?,
                target_config_hash: object_hash("membership-target", &proposal.intent.target)?,
                control_archive_root: archive.root_hash.clone(),
            };
            members.insert(
                disk_id,
                PendingDisk {
                    root: reserved.root,
                    seal,
                    reservation_name: reserved.name,
                    _lock: reserved.writer,
                },
            );
        }
        Ok(TerminalMetadataResume {
            management: self,
            source,
            proposal,
            members,
            votes: BTreeMap::new(),
            installed: BTreeSet::new(),
            core: None,
        })
    }

    /// Reenter only the protected immutable request. Paths recorded in Config
    /// are never enough: callers resupply the exact selected directory handles.
    pub fn reopen_metadata_resume(
        root: &Path,
        directories: Vec<PendingMemberDirectory>,
    ) -> Result<TerminalMetadataResume> {
        let management = Self::open_for_resume(root)?;
        let proposal: ResumeProposal = decode_strict(
            &management
                .system_root()
                .read_bytes(&resume_path(&management), MAX_METADATA)?,
        )?;
        if proposal.terminal.hash()? != management.verified_authority().authority.hash()? {
            return Err(Error::new(
                ErrorCode::LogFork,
                "resume request selects another terminal authority",
            ));
        }
        proposal.verify()?;
        proposal.verify_source(&management.verified_authority().state)?;
        management.prepare_metadata_resume(
            (*proposal.intent.target).clone(),
            &proposal.intent.principal,
            proposal.intent.operation_id,
            directories,
        )
    }
}

fn resume_path(management: &TerminalManagement) -> PathBuf {
    management
        .system_root()
        .path()
        .join("terminal-management")
        .join(
            management
                .verified_authority()
                .authority
                .operation_id()
                .to_string(),
        )
        .join("resume.json")
}

fn hold_roots(
    management: &TerminalManagement,
    target: &ValidatedConfig,
    directories: Vec<PendingMemberDirectory>,
    source: &PendingControlArchive,
) -> Result<Vec<ResumeRoot>> {
    let selected = target
        .metadata_members()
        .map(|disk| (disk.disk_id.as_str(), disk))
        .collect::<BTreeMap<_, _>>();
    if directories.len() != selected.len() {
        return Err(Error::new(
            ErrorCode::ConfigConflict,
            "resume must hold every fresh metadata directory exactly once",
        ));
    }
    let system = FileIdentity::from_fd(
        management
            .system_root()
            .directory(management.system_root().path())?,
    )?;
    let archive = source.identity()?;
    let (_, old_members) = source.abort_material()?;
    let (_, old_members) = old_members.verify(
        &source.generation_root(),
        &management
            .verified_authority()
            .authority
            .transition_hash()?,
    )?;
    let mut identities = BTreeSet::from([
        (system.device, system.inode),
        (archive.device, archive.inode),
    ]);
    for member in old_members.values() {
        identities.insert((member.seal.device, member.seal.inode));
    }
    let mut supplied = BTreeSet::new();
    let mut observations = Vec::new();
    let mut held = Vec::new();
    for directory in directories {
        let disk = selected.get(directory.disk_id.as_str()).ok_or_else(|| {
            Error::new(
                ErrorCode::ConfigConflict,
                "resume directory is outside its fresh metadata group",
            )
        })?;
        if !supplied.insert(directory.disk_id) {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "duplicate resume directory identity",
            ));
        }
        let physical = FileIdentity::from_fd(&directory.directory)?;
        if !identities.insert((physical.device, physical.inode)) {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "resume directory aliases a held source, old member or another target",
            ));
        }
        observations.push(
            crate::engine::disk_lifecycle::bootstrap::RootObservation::capture(
                &crate::DiskRegistrationIdentity {
                    pool_id: target.pool_id.clone(),
                    disk_id: disk.disk_id.clone(),
                    slot_id: disk.slot_id.clone(),
                    binding_generation: disk.binding_generation,
                },
                &disk.root,
                &directory.directory,
                true,
            )
            .map_err(crate::engine::disk_lifecycle::bootstrap::runtime::management_error)?,
        );
        held.push(((*disk).clone(), directory.directory));
    }
    let store = crate::engine::disk_lifecycle::bootstrap::runtime::open_store(
        management.system_root().path(),
    )?;
    let locator = store
        .reload(&target.pool_id)
        .map_err(crate::engine::disk_lifecycle::bootstrap::runtime::management_error)?;
    locator
        .validate_terminal_root_expansion(observations)
        .map_err(crate::engine::disk_lifecycle::bootstrap::runtime::management_error)?;
    // Validate the full protected historical alias set before named-root checks.
    let mut roots = held
        .into_iter()
        .map(|(disk, directory)| {
            let root = DiskRoot::from_directory(disk.root.clone(), directory)?;
            ResumeRoot::hold(disk, root, target)
        })
        .collect::<Result<Vec<_>>>()?;
    roots.sort_by(|left, right| left.disk.disk_id.cmp(&right.disk.disk_id));
    Ok(roots)
}

impl TerminalMetadataResume {
    /// Exact old control closure selected by this owned pending operation.
    /// This accessor performs no I/O and grants no additional authority.
    pub fn control_archive(&self) -> crate::MembershipControlArchive {
        self.management.control_archive()
    }

    pub fn status(&self) -> Result<TerminalMetadataResumeStatus> {
        let original = self.proposal.terminal.original();
        Ok(TerminalMetadataResumeStatus {
            operation_id: self.proposal.intent.operation_id,
            terminal_authority_hash: self.proposal.terminal.hash()?,
            transition_hash: object_hash(INTENT_KIND, &self.proposal.intent)?,
            original_metadata_members: original.metadata_members().len(),
            original_metadata_quorum: original.metadata_quorum(),
            target_metadata_members: self.proposal.intent.target.metadata_members().len(),
            target_metadata_quorum: self.proposal.intent.target.metadata_quorum(),
            synchronized_target_votes: self.votes.keys().cloned().collect(),
            installed_certificates: self.installed.iter().cloned().collect(),
        })
    }

    /// Partial writes remain reserved for the same request. Every retry clears
    /// cached observations and repeats independent archive/read/decision barriers.
    pub fn synchronize(&mut self) -> Result<TerminalMetadataResumeStatus> {
        self.votes.clear();
        self.installed.clear();
        self.core = None;
        self.management.system_root().write_json(
            &resume_path(&self.management),
            &self.proposal,
            false,
        )?;
        let transition = object_hash(INTENT_KIND, &self.proposal.intent)?;
        let proposal_hash = self.proposal.hash()?;
        for disk in self.proposal.intent.target.metadata_members() {
            let member = &self.members[&disk.disk_id];
            reservation::verify_writer(&member.root, &member._lock)?;
            admission::inspect_new_root(&member.root, &member.seal, &member.reservation_name)?;
            member
                .root
                .write_json(&disk.root.join("disk.json"), &member.seal, false)?;
            let parent_path = member_directory(disk, &transition);
            member.root.private_dir(&parent_path)?;
            let parent = member.root.private_child(&parent_path)?;
            let archive_path = parent.path().join(CONTROL_ARCHIVE_DIRECTORY);
            parent.private_dir(&archive_path)?;
            let installed = self.source.install(parent.private_child(&archive_path)?)?;
            let archive = PendingControlArchive::from_installed(
                installed,
                self.proposal.terminal.archive_root_hash(),
                &self.proposal.terminal.transition_hash()?,
            )?;
            let (state, _) = archive.generation_base(self.proposal.terminal.intent())?;
            self.proposal.verify_source(&state)?;
            parent.write_json(
                &parent.path().join("resume-proposal.json"),
                &self.proposal,
                false,
            )?;
            let store = super::super::decision::DecisionStore::open(
                &member.root,
                &member.seal.identity,
                &transition,
                true,
            )?;
            let receipt = store.terminal_resume_receipt(
                &proposal_hash,
                &self.proposal.acceptance_hash(&disk.disk_id)?,
                true,
            )?;
            self.votes.insert(disk.disk_id.clone(), receipt);
        }
        let core = ResumeCore::new(self.proposal.clone(), self.votes.clone())?;
        for disk in self.proposal.intent.target.metadata_members() {
            let member = &self.members[&disk.disk_id];
            let parent = member
                .root
                .private_child(&member_directory(disk, &transition))?;
            parent.write_json(&parent.path().join("resume-certificate.json"), &core, false)?;
            core.verify_member(&member.root, disk)?;
            self.installed.insert(disk.disk_id.clone());
        }
        self.core = Some(core);
        self.status()
    }
}

impl TerminalMetadataResume {
    /// Completes real first-generation publication and returns an admitted
    /// Engine. Any error preserves the exact pending request for explicit retry.
    pub fn activate(mut self) -> Result<Engine> {
        self.synchronize()?;
        let core = self
            .core
            .take()
            .expect("invariant: synchronization installed every certificate");
        activation::terminal_resume::activate(self.management, self.source, core, self.members)
    }
}

impl TerminalManagement {
    /// Resume after the durable generation selector exists, including crashes
    /// between any marker, journal, seal, publication or completion barrier.
    pub fn complete_metadata_resume(root: &Path) -> Result<Engine> {
        activation::terminal_resume::reopen(root)
    }
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/terminal_resume_tests.rs"]
mod tests;
