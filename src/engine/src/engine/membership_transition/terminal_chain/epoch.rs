//! Select the exact future epoch/Stop successor only after actual Publication
//! and complete Current admission. The locator itself remains terminal/stopped.
use super::*;
use crate::engine::disk_lifecycle::bootstrap::{
    runtime::open_store,
    store::{ChainRootPlanInfo, PreparedRootExtension},
};
use crate::engine::membership_transition::readmission::{ReadmissionRequest, ReadmissionRole};
const FILE: &str = "chain-activation-plan.json";
const EPOCH_KIND: &str = "terminal-chain-io-admission";

#[derive(Clone, Debug, Serialize, Deserialize)]
enum EpochFormat {
    #[serialization(rename = "terminal-chain-io-admission")]
    TerminalChain,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct EpochIntent {
    format: EpochFormat,
    selection_hash: String,
    core_hash: String,
    checkpoint_hash: String,
    candidate_hash: String,
    publication_hash: String,
    seal_hash: String,
    operation_id: OperationId,
    principal: String,
    request: ReadmissionRequest,
}
/// Source-authenticated recipe, not a live admission or a capability to consume
/// Stop. The store exposes only preparation/inspection for this typed recipe.
pub(in crate::engine) struct ChainEpochSource<'a> {
    selection: &'a Selection,
    intent: EpochIntent,
    epoch: String,
}
impl<'a> ChainEpochSource<'a> {
    pub(super) fn new(
        selection: &'a Selection,
        manifest: &first::Manifest,
        publication: &TerminalChainFirstPublication,
    ) -> Result<Self> {
        let proposal = &selection.proposal;
        let preparation = proposal.preparation()?;
        let (operation, transition, format) =
            terminal_resume::stopped_source(&selection.terminal, proposal.target())?;
        if selection.original_seal.format != format
            || manifest.selection_hash != selection.hash()?
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "chain epoch recipe changed its selected original lifetime",
            ));
        }
        let intent = EpochIntent {
            format: EpochFormat::TerminalChain,
            selection_hash: selection.hash()?,
            core_hash: manifest.core_hash.clone(),
            checkpoint_hash: proposal.checkpoint_digest().into(),
            candidate_hash: manifest.candidate_hash.clone(),
            publication_hash: publication.publication_hash.clone(),
            seal_hash: publication.seal_hash.clone(),
            operation_id: proposal.operation(),
            principal: proposal.principal().into(),
            request: ReadmissionRequest {
                target: proposal.target().clone(),
                directory: selection.directory.clone(),
                original_seal: selection.original_seal.clone(),
                predecessor_registry: preparation.source.registry,
                stopped_operation: operation,
                stopped_transition: transition,
                previous_admission: preparation.previous_admission,
                role: ReadmissionRole::Metadata,
            },
        };
        let epoch = PreparedMetadataObject::new(EPOCH_KIND, &intent, MAX_METADATA)?.hash;
        if intent.request.previous_admission.as_deref() == Some(epoch.as_str()) {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "chain readmission cannot reuse its previous I/O epoch",
            ));
        }
        Ok(Self {
            selection,
            intent,
            epoch,
        })
    }
    pub(in crate::engine) fn request(&self) -> &ReadmissionRequest {
        &self.intent.request
    }
    pub(in crate::engine) fn operation(&self) -> OperationId {
        self.intent.operation_id
    }
    pub(in crate::engine) fn epoch(&self) -> &str {
        &self.epoch
    }
    pub(in crate::engine) fn terminal(&self) -> &TerminalAuthority {
        &self.selection.terminal
    }
    pub(in crate::engine) fn disk(&self) -> &DiskConfig {
        self.selection.proposal.target_disk()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
enum PlanFormat {
    #[serialization(rename = "terminal-chain-activation-plan")]
    Selected,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct SelectedPlan {
    format: PlanFormat,
    intent: EpochIntent,
    planned_epoch: String,
    bootstrap: PreparedRootExtension,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct TerminalChainActivationPlanReport {
    pub planned_io_epoch: String,
    pub previous_io_epoch: Option<String>,
    pub stopped_operation: OperationId,
    pub stopped_transition: String,
    pub core_hash: String,
    pub publication_hash: String,
    pub seal_hash: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub previous_locator_generation: u64,
    pub previous_locator_hash: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub planned_locator_generation: u64,
    pub planned_locator_hash: String,
    pub target_reverified_this_call: bool,
    pub locator_installed: bool,
    pub stop_consumed: bool,
    pub active_writer_admitted: bool,
}
impl SelectedPlan {
    fn verify_source(&self, source: &ChainEpochSource<'_>) -> Result<()> {
        if self.planned_epoch != source.epoch
            || canonical_json(&self.intent)? != canonical_json(&source.intent)?
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "chain activation plan changed original Publication, epoch, physical root or Stop",
            ));
        }
        Ok(())
    }
    fn report(&self, info: ChainRootPlanInfo, target: bool) -> TerminalChainActivationPlanReport {
        TerminalChainActivationPlanReport {
            planned_io_epoch: self.planned_epoch.clone(),
            previous_io_epoch: self.intent.request.previous_admission.clone(),
            stopped_operation: self.intent.request.stopped_operation,
            stopped_transition: self.intent.request.stopped_transition.clone(),
            core_hash: self.intent.core_hash.clone(),
            publication_hash: self.intent.publication_hash.clone(),
            seal_hash: self.intent.seal_hash.clone(),
            previous_locator_generation: info.previous_generation,
            previous_locator_hash: info.previous_hash,
            planned_locator_generation: info.next_generation,
            planned_locator_hash: info.next_hash,
            target_reverified_this_call: target,
            locator_installed: false,
            stop_consumed: false,
            active_writer_admitted: false,
        }
    }
}
pub(super) fn owns_initial_entry(name: &str) -> bool {
    name == FILE
}
pub(super) fn started(management: &TerminalManagement) -> Result<bool> {
    Ok(management
        .system_root()
        .optional_metadata(&operation(management).join(FILE))?
        .is_some())
}
pub(super) fn require_absent(management: &TerminalManagement) -> Result<()> {
    if started(management)? {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "chain activation plan lost original source/Publication selection",
        ));
    }
    Ok(())
}
pub(super) fn observe(
    management: &TerminalManagement,
    selection: &Selection,
    prepared: &first::Prepared,
    status: &mut TerminalChainReadmissionStatus,
) -> Result<()> {
    let Some(plan): Option<SelectedPlan> =
        load(management.system_root(), &operation(management).join(FILE))?
    else {
        return Ok(());
    };
    let publication = status.publication.as_ref().ok_or_else(|| {
        Error::new(
            ErrorCode::RecoveryRequired,
            "chain activation plan requires complete original Publication evidence",
        )
    })?;
    let source = ChainEpochSource::new(selection, prepared.manifest(), publication)?;
    plan.verify_source(&source)?;
    let store = open_store(management.system_root().path())?;
    let info = store.inspect_chain_root_extension(&plan.bootstrap, &source)?;
    status.phase = TerminalChainReadmissionPhase::ActivationPlanSelected;
    status.activation_plan = Some(plan.report(info, false));
    Ok(())
}
impl TerminalManagement {
    /// Explicitly persists only the exact future epoch and locator record.
    /// The current locator, original Stop/seal and runtime I/O remain unchanged.
    pub fn prepare_chain_activation_plan(
        &mut self,
        principal: &str,
        operation_id: OperationId,
        proposal_digest: &str,
        directory: PendingMemberDirectory,
    ) -> Result<TerminalChainReadmissionStatus> {
        let reader =
            self.open_chain_published_current(principal, operation_id, proposal_digest, directory)?;
        let source = reader.epoch_source()?;
        let actual_target = reader.target_observation()?;
        let store = open_store(self.system_root().path())?;
        let path = operation(self).join(FILE);
        let prior: Option<SelectedPlan> = load(self.system_root(), &path)?;
        let is_new = prior.is_none();
        let plan: SelectedPlan = match prior {
            Some(plan) => plan,
            None => SelectedPlan {
                format: PlanFormat::Selected,
                intent: source.intent.clone(),
                planned_epoch: source.epoch.clone(),
                bootstrap: store.prepare_chain_root_extension(&source, actual_target)?,
            },
        };
        plan.verify_source(&source)?;
        let info = store.inspect_chain_root_extension(&plan.bootstrap, &source)?;
        let object =
            PreparedMetadataObject::new("terminal-chain-activation-plan", &plan, MAX_METADATA)?;
        if is_new && self.system_root().available()? < object.bytes().len() as u64 * 2 + 1024 * 1024
        {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "chain activation plan requires independent SYSTEM capacity",
            ));
        }
        reader.recheck_held_target()?;
        self.system_root().write_json(&path, &plan, false)?;
        let saved: SelectedPlan =
            decode_strict(&self.system_root().read_bytes(&path, MAX_METADATA)?)?;
        if canonical_json(&saved)? != object.bytes() {
            return Err(Error::new(
                ErrorCode::LogFork,
                "chain activation plan changed during durable readback",
            ));
        }
        saved.verify_source(&source)?;
        store.inspect_chain_root_extension(&saved.bootstrap, &source)?;
        reader.recheck_held_target()?;
        let report = saved.report(info, true);
        drop(source);
        drop(store);
        reader.recheck()?;
        drop(reader);
        let mut status = self
            .chain_readmission_status()?
            .expect("invariant: selected native chain remains owned");
        status.activation_plan = Some(report);
        Ok(status)
    }
}
