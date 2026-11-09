//! Exact cold generation selection after the real epoch decision and complete
//! installed journal. Selection does not consume Stop or authorize a writer.
use super::*;
use generation_log::InstalledGeneration;

const REQUEST: &str = "chain-generation-selector-request.json";
const COMPLETE: &str = "chain-generation-selector.json";
pub(super) const AUTHORITY_FILE: &str = "chain-authority.json";
const KIND: &str = "terminal-chain-generation-selector";

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "terminal-chain-selected-generation")]
    Selected,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Descriptor {
    format: Format,
    native: first::Manifest,
    generation_log_hash: String,
    decision_hash: String,
    plan: epoch::SelectedPlan,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct TerminalChainGenerationSelectionReport {
    pub selector_hash: String,
    pub core_hash: String,
    pub generation_log_hash: String,
    pub decision_hash: String,
    pub io_epoch: String,
    pub publication_hash: String,
    pub seal_hash: String,
    pub installed_current: Option<TerminalChainPublishedCurrentReport>,
    pub target_reverified_this_call: bool,
    pub installed_current_reverified_this_call: bool,
    pub locator_installed: bool,
    pub stop_consumed: bool,
    pub active_writer_admitted: bool,
}
fn missing(message: &str) -> Error {
    Error::new(ErrorCode::RecoveryRequired, message)
}
impl Descriptor {
    fn expected(
        management: &TerminalManagement,
        selection: &Selection,
        prepared: &first::Prepared,
        publication: &TerminalChainFirstPublication,
        decision: &TerminalChainIoDecisionReport,
    ) -> Result<Self> {
        let source = epoch::ChainEpochSource::new(selection, prepared.manifest(), publication)?;
        let plan = epoch::selected_plan(management, &source)?;
        let log = generation_log::completed_source(management, prepared, decision)?;
        if log.plan_hash != plan.object()?.hash || log.io_epoch != plan.epoch() {
            return Err(Error::new(
                ErrorCode::LogFork,
                "generation selector has another completed epoch plan",
            ));
        }
        let descriptor = Self {
            format: Format::Selected,
            generation_log_hash: log.hash()?,
            native: log.native,
            decision_hash: log.decision_hash,
            plan,
        };
        descriptor.object()?;
        Ok(descriptor)
    }
    fn for_reader(
        management: &TerminalManagement,
        reader: &TerminalChainPublishedCurrent<'_>,
    ) -> Result<Self> {
        let decision = epoch_decision::verify_existing(management, reader)?;
        let prepared = reader.prepared_generation_inputs()?;
        Self::expected(
            management,
            reader.selection(),
            &prepared,
            &reader.publication_report()?,
            &decision,
        )
    }
    fn object(&self) -> Result<PreparedMetadataObject> {
        PreparedMetadataObject::new(KIND, self, MAX_METADATA)
    }
    fn exact(&self, expected: &Self) -> Result<()> {
        if self.object()?.bytes() != expected.object()?.bytes() {
            return Err(Error::new(
                ErrorCode::LogFork,
                "selected generation changed the original native core, epoch, Publication or installed log",
            ));
        }
        Ok(())
    }
    fn read_at(&self, root: &DiskRoot, path: &Path) -> Result<bool> {
        match load::<Self>(root, path)? {
            Some(saved) => {
                saved.exact(self)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }
    fn preserve(&self, root: &DiskRoot, path: &Path) -> Result<()> {
        root.write_json(path, self, false)?;
        if !self.read_at(root, path)? {
            return Err(missing(
                "generation selector disappeared after immutable write",
            ));
        }
        Ok(())
    }
    fn verify_install(&self, installed: &InstalledGeneration) -> Result<()> {
        self.verify_manifest(&installed.manifest)
    }
    fn verify_manifest(&self, manifest: &generation_log::Manifest) -> Result<()> {
        activation::exact(&self.native, &manifest.native)?;
        if manifest.hash()? != self.generation_log_hash
            || manifest.io_epoch != self.plan.epoch()
            || manifest.decision_hash != self.decision_hash
            || manifest.plan_hash != self.plan.object()?.hash
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "selected generation differs from actual installed journal/epoch evidence",
            ));
        }
        Ok(())
    }
    fn historical_report(&self) -> Result<TerminalChainGenerationSelectionReport> {
        Ok(TerminalChainGenerationSelectionReport {
            selector_hash: self.object()?.hash,
            core_hash: self.native.core_hash.clone(),
            generation_log_hash: self.generation_log_hash.clone(),
            decision_hash: self.decision_hash.clone(),
            io_epoch: self.plan.epoch().into(),
            publication_hash: self.plan.publication_hash().into(),
            seal_hash: self.plan.seal_hash().into(),
            installed_current: None,
            target_reverified_this_call: false,
            installed_current_reverified_this_call: false,
            locator_installed: false,
            stop_consumed: false,
            active_writer_admitted: false,
        })
    }
}
pub(super) fn owns_initial_entry(name: &str) -> bool {
    matches!(name, REQUEST | COMPLETE)
}
/// The earlier log installer may encounter the later selector only while its
/// exact SYSTEM request and native manifest still own that target filename.
pub(super) fn verify_generation_entry(
    management: &TerminalManagement,
    generation: &DiskRoot,
    manifest: &generation_log::Manifest,
) -> Result<()> {
    let request: Descriptor = load(
        management.system_root(),
        &operation(management).join(REQUEST),
    )?
    .ok_or_else(|| missing("generation descriptor has no original SYSTEM selector request"))?;
    request.verify_manifest(manifest)?;
    if !request.read_at(generation, &generation.path().join(AUTHORITY_FILE))? {
        return Err(missing("generation selector disappeared during inventory"));
    }
    Ok(())
}
pub(super) fn started(management: &TerminalManagement) -> Result<bool> {
    for name in [REQUEST, COMPLETE] {
        if management
            .system_root()
            .optional_metadata(&operation(management).join(name))?
            .is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}
pub(super) fn observe(
    management: &TerminalManagement,
    selection: &Selection,
    prepared: &first::Prepared,
    status: &mut TerminalChainReadmissionStatus,
) -> Result<()> {
    let root = management.system_root();
    let Some(request): Option<Descriptor> = load(root, &operation(management).join(REQUEST))?
    else {
        if started(management)? {
            return Err(missing("generation selector lost its original request"));
        }
        return Ok(());
    };
    let publication = status
        .publication
        .as_ref()
        .ok_or_else(|| missing("generation selector requires completed source Publication"))?;
    let decision = status
        .io_decision
        .as_ref()
        .ok_or_else(|| missing("generation selector requires completed source epoch decision"))?;
    let expected = Descriptor::expected(management, selection, prepared, publication, decision)?;
    request.exact(&expected)?;
    status.phase = TerminalChainReadmissionPhase::GenerationSelectionPending;
    if request.read_at(root, &operation(management).join(COMPLETE))? {
        status.phase = TerminalChainReadmissionPhase::GenerationSelected;
        status.generation_selection = Some(request.historical_report()?);
    }
    Ok(())
}

/// This owns the actual selected generation directory and the original target
/// locks through its published reader. It cannot become an Engine/I/O binding.
pub struct TerminalChainSelectedGeneration<'a> {
    management: &'a TerminalManagement,
    reader: TerminalChainPublishedCurrent<'a>,
    installed: InstalledGeneration,
    descriptor: Descriptor,
}
impl TerminalChainSelectedGeneration<'_> {
    fn verify_generation(&self) -> Result<()> {
        let expected = Descriptor::for_reader(self.management, &self.reader)?;
        self.descriptor.exact(&expected)?;
        let actual = generation_log::open_complete(self.management, &self.reader)?;
        self.descriptor.verify_install(&actual)?;
        if actual.root.directory_identity(actual.root.path())?
            != self
                .installed
                .root
                .directory_identity(self.installed.root.path())?
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "selected generation directory was replaced",
            ));
        }
        Ok(())
    }
    fn verify_target(&self) -> Result<()> {
        self.verify_generation()?;
        if !self.descriptor.read_at(
            &self.installed.root,
            &self.installed.root.path().join(AUTHORITY_FILE),
        )? {
            return Err(missing(
                "selected generation lost its actual authority descriptor",
            ));
        }
        Ok(())
    }
    fn recheck(&self) -> Result<()> {
        self.verify_target()?;
        for name in [REQUEST, COMPLETE] {
            if !self.descriptor.read_at(
                self.management.system_root(),
                &operation(self.management).join(name),
            )? {
                return Err(missing(
                    "selected generation reader lost its completed SYSTEM selector",
                ));
            }
        }
        Ok(())
    }
    /// Revalidates actual selected log, immutable objects and native epoch choice.
    pub fn report(&self) -> Result<TerminalChainGenerationSelectionReport> {
        self.recheck()?;
        let mut report = self.descriptor.historical_report()?;
        report.installed_current = Some(self.reader.namespace_report(&self.installed.namespaces)?);
        report.target_reverified_this_call = true;
        report.installed_current_reverified_this_call = true;
        Ok(report)
    }
    /// Reads the projection decoded from installed generation bytes, not a DTO.
    pub fn branch_entries(&self, branch_id: &str) -> Result<&[crate::Entry]> {
        self.recheck()?;
        let head = self.reader.file_branch_head(branch_id)?;
        self.installed.namespaces.branch_entries(branch_id, head)
    }
    pub fn directory_entries(&self, branch_id: &str) -> Result<&[crate::Entry]> {
        self.recheck()?;
        let head = self.reader.directory_branch_head(branch_id)?;
        self.installed.namespaces.directory_entries(branch_id, head)
    }
}
impl TerminalManagement {
    /// Preserve one exact cold selector only after all original native evidence
    /// is complete. Never overwrite a selector or install an active pool marker.
    pub fn select_chain_generation(
        &mut self,
        principal: &str,
        operation_id: OperationId,
        proposal_digest: &str,
        directory: PendingMemberDirectory,
    ) -> Result<TerminalChainReadmissionStatus> {
        let reader =
            self.open_chain_published_current(principal, operation_id, proposal_digest, directory)?;
        let descriptor = Descriptor::for_reader(self, &reader)?;
        let installed = generation_log::open_complete(self, &reader)?;
        descriptor.verify_install(&installed)?;
        let system = self.system_root();
        let request = operation(self).join(REQUEST);
        let complete = operation(self).join(COMPLETE);
        let authority = installed.root.path().join(AUTHORITY_FILE);
        let selected = descriptor.read_at(system, &request)?;
        let completed = descriptor.read_at(system, &complete)?;
        let target = descriptor.read_at(&installed.root, &authority)?;
        if (!selected && (completed || target)) || (completed && !target) {
            return Err(missing(
                "selector lost original request or completed target; do not reconstruct",
            ));
        }
        let bytes = descriptor.object()?.bytes().len() as u64;
        if (!completed && system.available()? < bytes * 4 + 1024 * 1024)
            || (!target && installed.root.available()? < bytes * 2 + 1024 * 1024)
        {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "generation selector requires independent original SYSTEM/target capacity",
            ));
        }
        descriptor.preserve(system, &request)?;
        // The actual generation identity is already held from open_complete.
        // Recheck installed evidence after the durable SYSTEM selection.
        let selected = TerminalChainSelectedGeneration {
            management: self,
            reader,
            installed,
            descriptor,
        };
        selected.verify_generation()?;
        selected
            .descriptor
            .preserve(&selected.installed.root, &authority)?;
        selected.verify_target()?;
        selected.descriptor.preserve(system, &complete)?;
        let report = selected.report()?;
        drop(selected);
        let mut status = self
            .chain_readmission_status()?
            .expect("invariant: selected generation retains original operation");
        status.generation_selection = Some(report);
        Ok(status)
    }

    /// Cold read only: requires both original SYSTEM records and the actual
    /// target authority. Missing completed evidence is never installed here.
    pub fn open_chain_selected_generation(
        &self,
        principal: &str,
        operation_id: OperationId,
        proposal_digest: &str,
        directory: PendingMemberDirectory,
    ) -> Result<TerminalChainSelectedGeneration<'_>> {
        let reader =
            self.open_chain_published_current(principal, operation_id, proposal_digest, directory)?;
        let descriptor = Descriptor::for_reader(self, &reader)?;
        let installed = generation_log::open_complete(self, &reader)?;
        let selected = TerminalChainSelectedGeneration {
            management: self,
            reader,
            installed,
            descriptor,
        };
        selected.recheck()?;
        Ok(selected)
    }
}
