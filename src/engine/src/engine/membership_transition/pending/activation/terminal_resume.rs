//! Terminal activation reuses ordinary generation promotion, full publication
//! recovery and namespace admission. New-Q preparation never unblocks writes.
use super::*;
use crate::engine::disk_lifecycle::{
    bootstrap::{
        RootObservation,
        runtime::{management_error, open_store},
        store::PreparedRootExtension,
    },
    terminal::TerminalManagement,
};
use crate::engine::membership_transition::{
    terminal::VerifiedTerminalAuthority,
    terminal_resume::{ResumeCore, ResumeProposal},
};

const PREPARED_FILE: &str = "resume-generation.json";
const COMPLETE_FILE: &str = "resume-complete.json";
const BOOTSTRAP_FILE: &str = "resume-bootstrap.json";

#[derive(Serialize, Deserialize)]
enum BootstrapFormat {
    #[serialization(rename = "terminal-resume-bootstrap-extension")]
    Extension,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct ResumeBootstrap {
    format: BootstrapFormat,
    generation_hash: String,
    extension: PreparedRootExtension,
}

#[derive(Serialize, Deserialize)]
enum CompletionFormat {
    #[serialization(rename = "published-terminal-resumption")]
    Published,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Completion {
    format: CompletionFormat,
    terminal_authority_hash: String,
    generation_hash: String,
    operation: OperationResult,
}

fn operation_directory(management: &TerminalManagement) -> PathBuf {
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
}

pub(in crate::engine::membership_transition::pending) fn activate(
    management: TerminalManagement,
    source: PendingControlArchive,
    core: ResumeCore,
    members: BTreeMap<String, PendingDisk>,
) -> Result<Engine> {
    let portable = PortableAuthority::new_terminal_resume(core.clone())?;
    let intent = portable.intent()?;
    let verified = management.verified_authority();
    if core.terminal().hash()? != verified.authority.hash()? {
        return Err(Error::new(
            ErrorCode::LogFork,
            "resume activation selects another completed terminal authority",
        ));
    }
    let (base, history) = source.generation_base(core.archive_intent())?;
    core.verify_source(&base)?;
    let generation = Arc::new(Generation {
        hash: portable.hash()?,
        workspace_roots: portable.workspace_root_aliases()?,
        original: portable.genesis_config()?,
        epoch: portable.epoch()?,
        intent: intent.clone(),
        portable,
        history,
        base: Arc::new(base),
    });
    generation.next_state()?;
    PreparedMetadataObject::new(
        "membership-generation-bundle",
        &generation.portable,
        MAX_METADATA,
    )?;
    let config = generation.portable.restrict_io(
        intent
            .target
            .clone()
            .with_generation(Arc::clone(&generation))?,
    )?;
    // New roots were explicitly selected. Existing addresses may only come from
    // the protected ledger and are further reduced by authenticated stop history.
    let mut bootstrap = open_store(management.system_root().path())?;
    let locator = bootstrap
        .reload(&config.pool_id)
        .map_err(management_error)?;
    let restricted = locator.restrict_config(config.clone(), &config)?;
    let allowed = restricted
        .io_disks()
        .map(|disk| disk.disk_id.clone())
        .chain(config.metadata_members().map(|disk| disk.disk_id.clone()))
        .collect();
    let config = config
        .with_disk_roots(
            restricted
                .disks
                .iter()
                .map(|disk| disk.root.clone())
                .collect(),
        )?
        .restrict_io_disks(allowed)?;
    let (handles, locks) = hold_resources(&management, &config, &core, &members)?;
    let observations = handles
        .iter()
        .map(|handle| {
            let disk = config
                .disks
                .iter()
                .find(|disk| disk.disk_id == handle.seal.disk_id)
                .expect("invariant: resume handle is configured");
            RootObservation::capture(
                &crate::DiskRegistrationIdentity {
                    pool_id: config.pool_id.clone(),
                    disk_id: disk.disk_id.clone(),
                    slot_id: disk.slot_id.clone(),
                    binding_generation: disk.binding_generation,
                },
                &disk.root,
                &handle.root.directory(&disk.root)?,
                disk.metadata_member,
            )
            .map_err(management_error)
        })
        .collect::<Result<Vec<_>>>()?;
    // The selected core is durable before any active marker or locator expands.
    // This is the exact cold-resume selector, never an authority hash alone.
    management.system_root().write_json(
        &operation_directory(&management).join(PREPARED_FILE),
        &generation.portable,
        false,
    )?;
    let plan_path = operation_directory(&management).join(BOOTSTRAP_FILE);
    let extension = match management
        .system_root()
        .open_optional_read_regular(&plan_path)?
    {
        Some(file) => {
            let saved: ResumeBootstrap = decode_strict(&read_file_bytes(file, MAX_METADATA)?)?;
            if saved.generation_hash != generation.hash {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "bootstrap plan selects another terminal resumption",
                ));
            }
            saved
        }
        None => ResumeBootstrap {
            format: BootstrapFormat::Extension,
            generation_hash: generation.hash.clone(),
            extension: bootstrap.prepare_terminal_root_extension(
                &locator,
                observations.clone(),
                verified,
                &core,
            )?,
        },
    };
    management
        .system_root()
        .write_json(&plan_path, &extension, false)?;
    bootstrap.resume_terminal_root_extension(
        &extension.extension,
        observations,
        verified,
        &core,
    )?;
    for disk in config.io_disks() {
        let root = &handles
            .iter()
            .find(|handle| handle.seal.disk_id == disk.disk_id)
            .expect("invariant: held exact resume resources")
            .root;
        if disk.metadata_member {
            core.verify_member(root, disk)?;
        }
        publication::install_marker(root, disk, &generation)?;
    }
    let resources = Arc::new(DiskResources::generation_handles(&config, handles)?);
    let mut engine = Engine::recovery_resources_with_locks(
        management.system_root().path().to_owned(),
        config,
        resources,
        locks,
    )?;
    drop(bootstrap);
    engine.blocked = true;
    engine.state = Arc::clone(&generation.base);
    engine.snapshot = generation.snapshot();
    engine.next_slot = generation.first_sequence()?;
    engine.admit_disk_exit_io_exclusion()?;
    engine.promote_generation_resources()?;
    let warnings = engine.prepare_generation_resources()?;
    let mut prepared = engine.prepare_recovery_with_warnings(warnings)?;
    engine.preflight_workspace_receipts(&mut prepared)?;
    engine.recover_prepared(crate::engine::RecoveryMode::CompletePending, prepared)?;
    let operation = engine.complete_generation_activation()?;
    engine.restore_current_disk_exit()?;
    engine.load_workspaces()?;
    engine.rebuild_index()?;
    if engine.operation_result_for_terminal_resume(&generation)? != operation {
        return Err(Error::new(
            ErrorCode::PublicationInvalid,
            "resume activation did not publish its exact permanent result",
        ));
    }
    management.system_root().write_json(
        &operation_directory(&management).join(COMPLETE_FILE),
        &Completion {
            format: CompletionFormat::Published,
            terminal_authority_hash: core.terminal().hash()?,
            generation_hash: generation.hash.clone(),
            operation,
        },
        false,
    )?;
    Ok(engine)
}

fn hold_resources(
    management: &TerminalManagement,
    config: &ValidatedConfig,
    core: &ResumeCore,
    members: &BTreeMap<String, PendingDisk>,
) -> Result<(Vec<Arc<DiskHandle>>, crate::disk::WriterLocks)> {
    let mut locks = management.writer_locks()?;
    let mut handles = Vec::new();
    for disk in config.io_disks() {
        let (root, writer) = if disk.metadata_member {
            let member = members.get(&disk.disk_id).ok_or_else(|| {
                Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "resume lost an explicitly held new member",
                )
            })?;
            reservation::verify_writer(&member.root, &member._lock)?;
            core.verify_member(&member.root, disk)?;
            (
                member.root.retained(disk.root.clone())?,
                Arc::clone(&member._lock),
            )
        } else {
            let root = DiskRoot::open(&disk.root)?;
            admission::verify_named_root(&root)?;
            let writer = root.open_regular(&root.path().join(".writer.lock"))?;
            platform_io::fs_safety::linux::SingleLinkRequirement::check(&writer)
                .map_err(fs_error)?;
            reservation::lock(&writer)?;
            (root, crate::disk::FenceOwner::new(writer, None))
        };
        let seal = crate::disk::identity_at(config, disk, &root.directory(root.path())?)?;
        let bytes = root.read_bytes(&disk.root.join("disk.json"), MAX_METADATA)?;
        publication::predecessor_seal_matches(config, disk, &root, &bytes, &seal)?;
        let handle = Arc::new(DiskHandle::new(root, seal, config)?);
        locks.insert(
            disk.disk_id.clone(),
            crate::disk::DiskWriter::adopt(&handle.root, writer)?,
        );
        handles.push(handle);
    }
    Ok((handles, locks))
}

impl Engine {
    fn operation_result_for_terminal_resume(
        &self,
        generation: &Generation,
    ) -> Result<OperationResult> {
        let expected = generation.result()?;
        if self.current_namespaces.as_ref().is_none_or(|view| {
            view.registry()
                != generation
                    .portable
                    .exit_registry()
                    .expect("invariant: resume registry")
        }) || self.state.operations.get(&expected.operation_id) != Some(&expected)
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "resume requires its actual publication and admitted namespace",
            ));
        }
        Ok(expected)
    }
}

/// A prepared generation is chosen by protected SYSTEM state, then every fresh
/// member is checked against that exact core before any promotion is repeated.
pub(in crate::engine::membership_transition) fn reopen(root: &Path) -> Result<Engine> {
    let management = TerminalManagement::open_for_resume(root)?;
    let proposal: super::super::super::terminal_resume::ResumeProposal =
        decode_strict(&management.system_root().read_bytes(
            &operation_directory(&management).join("resume.json"),
            MAX_METADATA,
        )?)?;
    let portable: PortableAuthority = decode_strict(&management.system_root().read_bytes(
        &operation_directory(&management).join(PREPARED_FILE),
        MAX_METADATA,
    )?)?;
    let core = portable
        .terminal_resume_core()
        .ok_or_else(|| {
            Error::new(
                ErrorCode::PublicationInvalid,
                "protected resume selector is not TerminalResume",
            )
        })?
        .clone();
    if core.proposal.hash()? != proposal.hash()?
        || core.terminal().hash()? != management.verified_authority().authority.hash()?
    {
        return Err(Error::new(
            ErrorCode::LogFork,
            "prepared resumption differs from protected exact request",
        ));
    }
    let intent = core.intent()?;
    let archive = management.control_archive();
    let source = PendingControlArchive::from_installed(
        management
            .system_root()
            .private_child(&archive.archive_path)?,
        &archive.root_hash,
        &archive.transition_hash,
    )?;
    let mut members = BTreeMap::new();
    for disk in intent.target.metadata_members() {
        let identity = intent.added_roots[&disk.disk_id].clone();
        let root = DiskRoot::open(&disk.root)?;
        identity.verify_physical_root(&root)?;
        let directory = root.directory(root.path())?;
        reservation::lock(&directory)?;
        let writer = root.open_regular(&root.path().join(".writer.lock"))?;
        platform_io::fs_safety::linux::SingleLinkRequirement::check(&writer).map_err(fs_error)?;
        reservation::lock(&writer)?;
        let owner = crate::disk::FenceOwner::new(writer, Some(directory));
        reservation::verify_writer(&root, &owner)?;
        core.verify_member(&root, disk)?;
        members.insert(
            disk.disk_id.clone(),
            PendingDisk {
                root: Arc::new(root),
                seal: PendingDiskSeal {
                    format: PendingFormat::MembershipPending,
                    identity: identity.clone(),
                    transition_hash: object_hash(INTENT_KIND, &intent)?,
                    target_config_hash: object_hash("membership-target", &intent.target)?,
                    control_archive_root: archive.root_hash.clone(),
                },
                reservation_name: reservation::name(
                    &intent.origin,
                    &intent.target,
                    &intent.principal,
                    intent.operation_id,
                    &identity,
                )?,
                _lock: owner,
            },
        );
    }
    activate(management, source, core, members)
}

/// Authenticated SYSTEM selector only. Actual target roots, fixed NEW-Q and
/// first-publication replay remain mandatory in normal restricted startup.
pub(in crate::engine) struct CompletedTerminalResume {
    generation_hash: String,
}
impl CompletedTerminalResume {
    pub(in crate::engine) fn verify_selected(&self, config: &ValidatedConfig) -> Result<()> {
        let generation = config.generation().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "completed terminal resumption cannot select Genesis",
            )
        })?;
        if !generation.portable.contains(&self.generation_hash)? {
            return Err(Error::new(
                ErrorCode::LogFork,
                "startup authority does not contain the protected terminal resumption",
            ));
        }
        Ok(())
    }
}

/// This runs before acquiring the bootstrap ledger. It touches only protected
/// SYSTEM state; absence/corruption of a completion is never Config fallback.
pub(in crate::engine) fn completed_selector(
    root: &Path,
) -> Result<Option<CompletedTerminalResume>> {
    let system = DiskRoot::open(root)?;
    let directory = system.path().join("terminal-management");
    if system.optional_metadata(&directory)?.is_none()
        || system
            .open_optional_read_regular(&directory.join("head.json"))?
            .is_none()
    {
        return Ok(None);
    }
    let (system, verified) = TerminalManagement::verified_completed_source(root)?;
    let path = system
        .path()
        .join("terminal-management")
        .join(verified.authority.operation_id().to_string());
    let bytes = system
        .read_bytes(&path.join(COMPLETE_FILE), MAX_METADATA)
        .map_err(|error| {
            error.context(
                ErrorCode::RecoveryRequired,
                "terminal resumption has no authenticated completion",
            )
        })?;
    let completed: Completion = decode_strict(&bytes).map_err(|error| {
        Error::from(error).context(
            ErrorCode::RecoveryRequired,
            "terminal resumption completion is malformed",
        )
    })?;
    let portable: PortableAuthority =
        decode_strict(&system.read_bytes(&path.join(PREPARED_FILE), MAX_METADATA)?)?;
    let proposal: super::super::super::terminal_resume::ResumeProposal =
        decode_strict(&system.read_bytes(&path.join("resume.json"), MAX_METADATA)?)?;
    let core = portable.terminal_resume_core().ok_or_else(|| {
        Error::new(
            ErrorCode::PublicationInvalid,
            "completion is not a terminal resumption",
        )
    })?;
    if completed.terminal_authority_hash != verified.authority.hash()?
        || core.terminal().hash()? != completed.terminal_authority_hash
        || core.proposal.hash()? != proposal.hash()?
        || completed.generation_hash != portable.hash()?
    {
        return Err(Error::new(
            ErrorCode::LogFork,
            "terminal completion breaks its protected exact authority and request",
        ));
    }
    core.verify_source(&verified.state)?;
    let generation = Generation {
        hash: portable.hash()?,
        workspace_roots: portable.workspace_root_aliases()?,
        original: portable.genesis_config()?,
        epoch: portable.epoch()?,
        intent: portable.intent()?,
        history: verified.history.clone(),
        base: Arc::clone(&verified.state),
        portable,
    };
    if generation.result()? != completed.operation {
        return Err(Error::new(
            ErrorCode::PublicationInvalid,
            "terminal completion does not carry its prescribed permanent result",
        ));
    }
    Ok(Some(CompletedTerminalResume {
        generation_hash: generation.hash,
    }))
}

fn selected_portable(
    system: &DiskRoot,
    verified: &VerifiedTerminalAuthority,
) -> Result<PortableAuthority> {
    let path = system
        .path()
        .join("terminal-management")
        .join(verified.authority.operation_id().to_string());
    let proposal: ResumeProposal =
        decode_strict(&system.read_bytes(&path.join("resume.json"), MAX_METADATA)?)?;
    let portable: PortableAuthority =
        decode_strict(&system.read_bytes(&path.join(PREPARED_FILE), MAX_METADATA)?)?;
    let core = portable.terminal_resume_core().ok_or_else(|| {
        Error::new(
            ErrorCode::PublicationInvalid,
            "protected resume selector is not TerminalResume",
        )
    })?;
    if core.proposal.hash()? != proposal.hash()?
        || core.terminal().hash()? != verified.authority.hash()?
    {
        return Err(Error::new(
            ErrorCode::LogFork,
            "resume selector breaks its completed terminal authority or request",
        ));
    }
    core.verify_source(&verified.state)?;
    portable.hash()?;
    Ok(portable)
}

/// Called only after the exact completed terminal authority and full old archive
/// have been authenticated, before ordinary strict bootstrap reload. No storage
/// root is opened: this repairs exactly one protected root-extension transaction,
/// which still cannot grant membership, publication or an I/O admission.
pub(in crate::engine) fn resume_bootstrap_extension(
    system: &DiskRoot,
    verified: &VerifiedTerminalAuthority,
) -> Result<()> {
    let path = system
        .path()
        .join("terminal-management")
        .join(verified.authority.operation_id().to_string());
    let Some(file) = system.open_optional_read_regular(&path.join(BOOTSTRAP_FILE))? else {
        return Ok(());
    };
    let saved: ResumeBootstrap = decode_strict(&read_file_bytes(file, MAX_METADATA)?)?;
    let portable = selected_portable(system, verified)?;
    if saved.generation_hash != portable.hash()? {
        return Err(Error::new(
            ErrorCode::LogFork,
            "prepared locator expansion selects another generation",
        ));
    }
    let core = portable
        .terminal_resume_core()
        .expect("invariant: selected terminal resume core");
    let observations = RootObservation::from_terminal_resume(verified, core)?;
    let mut store = open_store(system.path())?;
    store.resume_terminal_root_extension(&saved.extension, observations, verified, core)?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalMetadataResumePhase {
    ResupplyDirectories,
    CompleteGeneration,
}

#[derive(Clone, Debug)]
pub struct TerminalMetadataResumeRecovery {
    pub operation_id: OperationId,
    pub principal: String,
    pub target: Config,
    pub terminal_authority_hash: String,
    pub transition_hash: String,
    pub control_archive: crate::MembershipControlArchive,
    pub phase: TerminalMetadataResumePhase,
}

impl TerminalManagement {
    /// SYSTEM-only routing, not proof that a target is writable. Each selected
    /// route reauthenticates its actual directories and independent new quorum.
    pub fn metadata_resume_recovery(root: &Path) -> Result<Option<TerminalMetadataResumeRecovery>> {
        let management = Self::open_for_resume(root)?;
        let path = operation_directory(&management);
        let system = management.system_root();
        let selected = system
            .open_optional_read_regular(&path.join(PREPARED_FILE))?
            .is_some();
        let Some(file) = system.open_optional_read_regular(&path.join("resume.json"))? else {
            if selected
                || system
                    .open_optional_read_regular(&path.join(COMPLETE_FILE))?
                    .is_some()
            {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "terminal resume selector has no protected request",
                ));
            }
            return Ok(None);
        };
        let proposal: ResumeProposal = decode_strict(&read_file_bytes(file, MAX_METADATA)?)?;
        proposal.verify()?;
        proposal.verify_source(&management.verified_authority().state)?;
        if proposal.terminal.hash()? != management.verified_authority().authority.hash()? {
            return Err(Error::new(
                ErrorCode::LogFork,
                "resume recovery selects another terminal authority",
            ));
        }
        let phase = if selected {
            selected_portable(system, management.verified_authority())?;
            TerminalMetadataResumePhase::CompleteGeneration
        } else {
            if system
                .open_optional_read_regular(&path.join(COMPLETE_FILE))?
                .is_some()
            {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "resume completion has no selected generation",
                ));
            }
            TerminalMetadataResumePhase::ResupplyDirectories
        };
        Ok(Some(TerminalMetadataResumeRecovery {
            operation_id: proposal.intent.operation_id,
            principal: proposal.intent.principal.clone(),
            target: (*proposal.intent.target).clone(),
            terminal_authority_hash: proposal.terminal.hash()?,
            transition_hash: object_hash(INTENT_KIND, &proposal.intent)?,
            control_archive: management.control_archive(),
            phase,
        }))
    }
}
