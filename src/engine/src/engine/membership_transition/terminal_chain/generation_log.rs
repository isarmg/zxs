//! Preserve the actual published log at its generation address, and consume
//! Current from ordinary immutable objects. No active selector or Stop changes.
use super::*;
const SELECTED: &str = "chain-generation-log-selected.json";
const COMPLETE: &str = "chain-generation-log.json";
const OWNER: &str = "chain-log-owner.json";
const INSTALLED: &str = "chain-log-complete.json";
#[derive(Clone, Debug, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "terminal-chain-generation-log")]
    Prepared,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Manifest {
    format: Format,
    pub(super) native: first::Manifest,
    pub(super) plan_hash: String,
    pub(super) io_epoch: String,
    pub(super) decision_hash: String,
    journal: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct TerminalChainGenerationLogReport {
    pub core_hash: String,
    pub io_epoch: String,
    pub decision_hash: String,
    pub selection_hash: String,
    pub journal_bytes: usize,
    pub native_objects: usize,
    #[serialization(with = "storage_contract::u64_string")]
    pub native_bytes: u64,
    pub target_reverified_this_call: bool,
    pub installed_current_reverified_this_call: bool,
    pub locator_installed: bool,
    pub stop_consumed: bool,
    pub active_writer_admitted: bool,
}
impl Manifest {
    pub(super) fn hash(&self) -> Result<String> {
        Ok(object_hash("terminal-chain-generation-log", self)?)
    }
    fn exact(&self, expected: &Self) -> Result<()> {
        if canonical_json(self)? != canonical_json(expected)? {
            return Err(Error::new(
                ErrorCode::LogFork,
                "generation log changed original native inputs, epoch decision or actual frames",
            ));
        }
        Ok(())
    }
    fn verify_source(
        &self,
        management: &TerminalManagement,
        prepared: &first::Prepared,
        decision: &TerminalChainIoDecisionReport,
    ) -> Result<()> {
        activation::exact(&self.native, prepared.manifest())?;
        if self.plan_hash != decision.plan_hash
            || self.io_epoch != decision.io_epoch
            || self.decision_hash != decision.decision_hash
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "generation log differs from the completed original I/O decision",
            ));
        }
        publication::verify_source_log_image(management, prepared, &self.journal)
    }
    fn report(&self, fresh: bool) -> TerminalChainGenerationLogReport {
        TerminalChainGenerationLogReport {
            core_hash: self.native.core_hash.clone(),
            io_epoch: self.io_epoch.clone(),
            decision_hash: self.decision_hash.clone(),
            selection_hash: self.native.selection_hash.clone(),
            journal_bytes: self.journal.len(),
            native_objects: self.native.native_objects,
            native_bytes: self.native.native_bytes,
            target_reverified_this_call: fresh,
            installed_current_reverified_this_call: fresh,
            locator_installed: false,
            stop_consumed: false,
            active_writer_admitted: false,
        }
    }
}
pub(super) fn owns_initial_entry(name: &str) -> bool {
    matches!(name, SELECTED | COMPLETE) || selector::owns_initial_entry(name)
}
pub(super) fn started(management: &TerminalManagement) -> Result<bool> {
    for name in [SELECTED, COMPLETE] {
        if management
            .system_root()
            .optional_metadata(&operation(management).join(name))?
            .is_some()
        {
            return Ok(true);
        }
    }
    selector::started(management)
}
pub(super) fn require_absent(management: &TerminalManagement) -> Result<()> {
    if started(management)? {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "generation log lost its completed original I/O decision",
        ));
    }
    Ok(())
}
pub(super) fn observe(
    management: &TerminalManagement,
    prepared: &first::Prepared,
    status: &mut TerminalChainReadmissionStatus,
) -> Result<()> {
    let root = management.system_root();
    let Some(manifest): Option<Manifest> = load(root, &operation(management).join(SELECTED))?
    else {
        require_absent(management)?;
        return Ok(());
    };
    let decision = status.io_decision.as_ref().ok_or_else(|| {
        Error::new(
            ErrorCode::RecoveryRequired,
            "generation log needs original completed I/O evidence",
        )
    })?;
    manifest.verify_source(management, prepared, decision)?;
    status.phase = TerminalChainReadmissionPhase::GenerationLogPending;
    if let Some(complete) = load::<Manifest>(root, &operation(management).join(COMPLETE))? {
        complete.exact(&manifest)?;
        status.phase = TerminalChainReadmissionPhase::GenerationLogPreserved;
        status.generation_log = Some(complete.report(false));
    }
    Ok(())
}
struct Install<'a, 'm> {
    management: &'m TerminalManagement,
    reader: &'a TerminalChainPublishedCurrent<'m>,
    prepared: first::Prepared,
    manifest: Manifest,
    generation_path: PathBuf,
}
impl<'a, 'm> Install<'a, 'm> {
    fn new(
        management: &'m TerminalManagement,
        reader: &'a TerminalChainPublishedCurrent<'m>,
    ) -> Result<Self> {
        let decision = epoch_decision::verify_existing(management, reader)?;
        let prepared = reader.prepared_generation_inputs()?;
        let manifest = Manifest {
            format: Format::Prepared,
            native: prepared.manifest().clone(),
            plan_hash: decision.plan_hash,
            io_epoch: decision.io_epoch,
            decision_hash: decision.decision_hash,
            journal: reader.original_log_image()?,
        };
        PreparedMetadataObject::new("terminal-chain-generation-log", &manifest, MAX_METADATA)?;
        let generation_path =
            pending::activation::generation_directory(prepared.disk(), &manifest.native.core_hash);
        Ok(Self {
            management,
            reader,
            prepared,
            manifest,
            generation_path,
        })
    }
    fn recheck(&self) -> Result<()> {
        let actual = epoch_decision::verify_existing(self.management, self.reader)?;
        self.manifest
            .verify_source(self.management, &self.prepared, &actual)?;
        if self.reader.original_log_image()? != self.manifest.journal {
            return Err(Error::new(
                ErrorCode::LogFork,
                "generation image no longer matches the actual original target journal",
            ));
        }
        Ok(())
    }
    fn read_marker(&self, root: &DiskRoot, path: &Path) -> Result<bool> {
        match load::<Manifest>(root, path)? {
            Some(saved) => {
                saved.exact(&self.manifest)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }
    fn preserve_marker(&self, root: &DiskRoot, path: &Path) -> Result<()> {
        root.write_json(path, &self.manifest, false)?;
        if !self.read_marker(root, path)? {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "generation log marker disappeared after write",
            ));
        }
        Ok(())
    }
    fn inventory(&self, generation: &DiskRoot) -> Result<()> {
        for (index, name) in generation.entries(&self.generation_path)?.enumerate() {
            if index == 64 {
                return Err(Error::new(
                    ErrorCode::ResourceLimit,
                    "generation log directory exceeds its entry budget",
                ));
            }
            let name = name.map_err(fs_error)?;
            if platform_io::fs_safety::AtomicFile::is_temporary_name(&name) {
                generation.read_bytes(&self.generation_path.join(name.as_path()), MAX_METADATA)?;
            } else if name.as_os_str().to_str() == Some(selector::AUTHORITY_FILE) {
                selector::verify_generation_entry(self.management, generation, &self.manifest)?;
            } else if !matches!(
                name.as_os_str().to_str(),
                Some(OWNER | INSTALLED | "journal.bin")
            ) {
                return Err(Error::new(
                    ErrorCode::UnsafePath,
                    "generation log directory contains unrelated material",
                ));
            }
        }
        Ok(())
    }
    fn verify_installed(
        &self,
        generation: &DiskRoot,
    ) -> Result<crate::engine::disk_registry::current::CurrentNamespaces> {
        self.inventory(generation)?;
        let held = self.reader.held_target();
        if held.root.directory_identity(&self.generation_path)?
            != generation.directory_identity(&self.generation_path)?
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "original generation log directory was replaced",
            ));
        }
        if !self.read_marker(generation, &self.generation_path.join(OWNER))? {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "generation log lost its original owner selection",
            ));
        }
        let bytes = generation.read_bytes(&self.generation_path.join("journal.bin"), 16 * 1024)?;
        if bytes != self.manifest.journal {
            return Err(Error::new(
                ErrorCode::LogFork,
                "installed generation log changed its original exact bytes",
            ));
        }
        let metadata = held
            .root
            .private_child(&self.prepared.disk().root.join("metadata"))?;
        let current = self
            .reader
            .generation_current(&metadata, &bytes, &self.prepared)?;
        if current.registry()
            != self
                .prepared
                .candidate()
                .registry()
                .expect("invariant: complete native chain registry")
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "installed generation Current changed the selected registry",
            ));
        }
        self.recheck()?;
        Ok(current)
    }
    fn install(&self) -> Result<TerminalChainGenerationLogReport> {
        self.recheck()?;
        let system = self.management.system_root();
        let root = &self.reader.held_target().root;
        let selected = self.read_marker(system, &operation(self.management).join(SELECTED))?;
        let complete = self.read_marker(system, &operation(self.management).join(COMPLETE))?;
        let exists = root.optional_metadata(&self.generation_path)?.is_some();
        if (!selected && (complete || exists)) || (complete && !exists) {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "generation installation lost its original source selection or completed target",
            ));
        }
        let installed = if exists {
            let generation = root.private_child(&self.generation_path)?;
            self.inventory(&generation)?;
            let installed = self.read_marker(&generation, &self.generation_path.join(INSTALLED))?;
            if installed || complete {
                self.verify_installed(&generation)?;
            }
            if complete && !installed {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "completed generation lost target completion; do not rebuild",
                ));
            }
            installed
        } else {
            false
        };
        if !installed {
            self.prepared.require_capacity(root)?;
            self.prepared
                .require_generation_layout(root, self.prepared.disk())?;
        }
        if !complete
            && system.available()? < canonical_json(&self.manifest)?.len() as u64 * 4 + 1024 * 1024
        {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "generation installation requires independent SYSTEM selector capacity",
            ));
        }
        self.preserve_marker(system, &operation(self.management).join(SELECTED))?;
        if !exists {
            root.private_dir(
                self.generation_path
                    .parent()
                    .expect("invariant: generation directory has a parent"),
            )?;
            root.create_private_directory_synced(&self.generation_path)?;
        }
        let generation = root.private_child(&self.generation_path)?;
        if !installed {
            // The original request may resume only absent immutable files. Any
            // named different bytes fail; completed material is never rebuilt.
            self.preserve_marker(&generation, &self.generation_path.join(OWNER))?;
            self.recheck()?;
            self.prepared
                .install_generation_objects(root, self.prepared.disk())?;
            self.reader.install_generation_publication()?;
            generation.write_atomic(
                &self.generation_path.join("journal.bin"),
                &self.manifest.journal,
                false,
            )?;
            self.verify_installed(&generation)?;
            self.preserve_marker(&generation, &self.generation_path.join(INSTALLED))?;
        }
        self.verify_installed(&generation)?;
        self.preserve_marker(system, &operation(self.management).join(COMPLETE))?;
        self.verify_installed(&generation)?;
        Ok(self.manifest.report(true))
    }
}
impl TerminalManagement {
    /// Prepare actual native generation-addressed log and ordinary immutable
    /// objects; admit only a frozen Current reader. No active authority changes.
    pub fn prepare_chain_generation_log(
        &mut self,
        principal: &str,
        operation_id: OperationId,
        proposal_digest: &str,
        directory: PendingMemberDirectory,
    ) -> Result<TerminalChainReadmissionStatus> {
        let reader =
            self.open_chain_published_current(principal, operation_id, proposal_digest, directory)?;
        let report = Install::new(self, &reader)?.install()?;
        drop(reader);
        let mut status = self
            .chain_readmission_status()?
            .expect("invariant: original generation selection remains owned");
        status.generation_log = Some(report);
        Ok(status)
    }
}

/// Source-only historical evidence. This never probes the original disk.
pub(super) fn completed_source(
    management: &TerminalManagement,
    prepared: &first::Prepared,
    decision: &TerminalChainIoDecisionReport,
) -> Result<Manifest> {
    let root = management.system_root();
    let selected: Manifest = load(root, &operation(management).join(SELECTED))?
        .ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "generation selector requires its original log selection",
            )
        })?;
    selected.verify_source(management, prepared, decision)?;
    let complete: Manifest = load(root, &operation(management).join(COMPLETE))?
        .ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "generation selector requires completed original log evidence",
            )
        })?;
    complete.exact(&selected)?;
    Ok(complete)
}

/// A held actual generation directory and Current decoded from its installed
/// native bytes. This value carries no writer or Stop-consumption capability.
pub(super) struct InstalledGeneration {
    pub(super) root: DiskRoot,
    pub(super) namespaces: crate::engine::disk_registry::current::CurrentNamespaces,
    pub(super) manifest: Manifest,
}
pub(super) fn open_complete(
    management: &TerminalManagement,
    reader: &TerminalChainPublishedCurrent<'_>,
) -> Result<InstalledGeneration> {
    let install = Install::new(management, reader)?;
    let system = management.system_root();
    if !install.read_marker(system, &operation(management).join(SELECTED))?
        || !install.read_marker(system, &operation(management).join(COMPLETE))?
    {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "generation reader requires both original completed SYSTEM log records",
        ));
    }
    let root = reader
        .held_target()
        .root
        .private_child(&install.generation_path)?;
    if !install.read_marker(&root, &install.generation_path.join(INSTALLED))? {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "completed generation reader lost its target log completion",
        ));
    }
    let namespaces = install.verify_installed(&root)?;
    Ok(InstalledGeneration {
        root,
        namespaces,
        manifest: install.manifest,
    })
}
