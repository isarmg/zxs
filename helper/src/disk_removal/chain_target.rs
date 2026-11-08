//! Explicit target preparation and first-slot publication. Native SYSTEM owns the original
//! operation and all recovery evidence; no root cache can certify publication.
use super::chain_readmission::Plan;
use super::*;
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Commit {
    service_user: String,
    external_writers_stopped: bool,
    plan: Plan,
    confirmation: String,
    acknowledge_reserved_request_without_activation: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct FirstInput {
    service_user: String,
    external_writers_stopped: bool,
    plan: Plan,
    confirmation: String,
    acknowledge_unpublished_inputs: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct PublicationInput {
    service_user: String,
    external_writers_stopped: bool,
    plan: Plan,
    confirmation: String,
    acknowledge_publication_without_activation: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct ActivationPlanInput {
    service_user: String,
    external_writers_stopped: bool,
    plan: Plan,
    confirmation: String,
    acknowledge_plan_without_stop_consumption: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct IoDecisionInput {
    service_user: String,
    external_writers_stopped: bool,
    plan: Plan,
    confirmation: String,
    acknowledge_irreversible_decision_without_activation: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct GenerationLogInput {
    service_user: String,
    external_writers_stopped: bool,
    plan: Plan,
    confirmation: String,
    acknowledge_generation_log_without_active_writer: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct GenerationSelectionInput {
    service_user: String,
    external_writers_stopped: bool,
    plan: Plan,
    confirmation: String,
    acknowledge_selector_without_active_writer: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct CurrentInput {
    service_user: String,
    external_writers_stopped: bool,
    plan: Plan,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Status {
    plan: Option<Plan>,
    native: Option<storage_engine::TerminalChainReadmissionStatus>,
    service_started: bool,
    completed_readmission: bool,
    published_current: Option<storage_engine::TerminalChainPublishedCurrentReport>,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Request {
    context: layout::Context,
    action: Action,
}
#[derive(Serialize, Deserialize)]
#[serialization(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
enum Action {
    Status,
    Synchronize { plan: Plan },
    PrepareFirst { plan: Plan },
    PublishFirst { plan: Plan },
    InspectPublished { plan: Plan },
    PrepareActivation { plan: Plan },
    DecideIo { plan: Plan },
    PrepareGenerationLog { plan: Plan },
    SelectGeneration { plan: Plan },
    InspectGeneration { plan: Plan },
}
impl crate::service_worker::Request for Request {
    type Output = Status;
    const COMMAND: &'static str = "internal-chain-target-worker";
    fn account(&self) -> &crate::account::Snapshot {
        &self.context.account
    }
    fn execute(self) -> Result<Status> {
        let mut target = worker::Target::open(&self.context)?;
        target.recheck()?;
        let mut published_current = None;
        let native = match self.action {
            Action::Status => target.control.terminal_chain_readmission_status()?,
            Action::SelectGeneration { plan } => {
                if plan.context != self.context
                    || plan.credential != target.credential_revision()?
                {
                    return Err(
                        "chain generation selector host or primary credential revision changed"
                            .into(),
                    );
                }
                let preparation = plan.preparation;
                Some(target.control.select_terminal_chain_generation(
                    storage_contract::storage_readmission::ReadmissionRequest {
                        operation_id: preparation.operation_id,
                        slot_id: preparation.target.slot_id,
                        expected_disk_id: preparation.target.disk_id,
                    },
                    &preparation.proposal_digest,
                )?)
            }
            Action::InspectGeneration { plan } => {
                if plan.context != self.context
                    || plan.credential != target.credential_revision()?
                {
                    return Err(
                        "chain generation selector host or primary credential revision changed"
                            .into(),
                    );
                }
                let preparation = plan.preparation;
                Some(target.control.inspect_terminal_chain_generation(
                    storage_contract::storage_readmission::ReadmissionRequest {
                        operation_id: preparation.operation_id,
                        slot_id: preparation.target.slot_id,
                        expected_disk_id: preparation.target.disk_id,
                    },
                    &preparation.proposal_digest,
                )?)
            }
            Action::PrepareGenerationLog { plan } => {
                if plan.context != self.context
                    || plan.credential != target.credential_revision()?
                {
                    return Err(
                        "chain generation log host or primary credential revision changed".into(),
                    );
                }
                let preparation = plan.preparation;
                Some(target.control.prepare_terminal_chain_generation_log(
                    storage_contract::storage_readmission::ReadmissionRequest {
                        operation_id: preparation.operation_id,
                        slot_id: preparation.target.slot_id,
                        expected_disk_id: preparation.target.disk_id,
                    },
                    &preparation.proposal_digest,
                )?)
            }
            Action::DecideIo { plan } => {
                if plan.context != self.context
                    || plan.credential != target.credential_revision()?
                {
                    return Err(
                        "chain I/O decision host or primary credential revision changed".into(),
                    );
                }
                let preparation = plan.preparation;
                Some(target.control.decide_terminal_chain_io_admission(
                    storage_contract::storage_readmission::ReadmissionRequest {
                        operation_id: preparation.operation_id,
                        slot_id: preparation.target.slot_id,
                        expected_disk_id: preparation.target.disk_id,
                    },
                    &preparation.proposal_digest,
                )?)
            }
            Action::PrepareActivation { plan } => {
                if plan.context != self.context
                    || plan.credential != target.credential_revision()?
                {
                    return Err(
                        "chain activation plan host or primary credential revision changed".into(),
                    );
                }
                let preparation = plan.preparation;
                Some(target.control.prepare_terminal_chain_activation_plan(
                    storage_contract::storage_readmission::ReadmissionRequest {
                        operation_id: preparation.operation_id,
                        slot_id: preparation.target.slot_id,
                        expected_disk_id: preparation.target.disk_id,
                    },
                    &preparation.proposal_digest,
                )?)
            }
            Action::InspectPublished { plan } => {
                if plan.context != self.context
                    || plan.credential != target.credential_revision()?
                {
                    return Err(
                        "published Current host or primary credential revision changed".into(),
                    );
                }
                let preparation = plan.preparation;
                published_current = Some(target.control.inspect_terminal_chain_current(
                    storage_contract::storage_readmission::ReadmissionRequest {
                        operation_id: preparation.operation_id,
                        slot_id: preparation.target.slot_id,
                        expected_disk_id: preparation.target.disk_id,
                    },
                    &preparation.proposal_digest,
                )?);
                target.control.terminal_chain_readmission_status()?
            }
            Action::PublishFirst { plan } => {
                if plan.context != self.context
                    || plan.credential != target.credential_revision()?
                {
                    return Err(
                        "chain Publication host or primary credential revision changed".into(),
                    );
                }
                let preparation = plan.preparation;
                Some(target.control.publish_terminal_chain_first(
                    storage_contract::storage_readmission::ReadmissionRequest {
                        operation_id: preparation.operation_id,
                        slot_id: preparation.target.slot_id,
                        expected_disk_id: preparation.target.disk_id,
                    },
                    &preparation.proposal_digest,
                )?)
            }
            Action::PrepareFirst { plan } => {
                if plan.context != self.context
                    || plan.credential != target.credential_revision()?
                {
                    return Err(
                        "chain first-input host or primary credential revision changed".into(),
                    );
                }
                let preparation = plan.preparation;
                Some(target.control.prepare_terminal_chain_first_candidate(
                    storage_contract::storage_readmission::ReadmissionRequest {
                        operation_id: preparation.operation_id,
                        slot_id: preparation.target.slot_id,
                        expected_disk_id: preparation.target.disk_id,
                    },
                    &preparation.proposal_digest,
                )?)
            }
            Action::Synchronize { plan } => {
                if plan.context != self.context
                    || plan.credential != target.credential_revision()?
                {
                    return Err(
                        "chain target proposal host or primary credential revision changed".into(),
                    );
                }
                let preparation = plan.preparation;
                Some(target.control.synchronize_terminal_chain_readmission(
                    storage_contract::storage_readmission::ReadmissionRequest {
                        operation_id: preparation.operation_id,
                        slot_id: preparation.target.slot_id,
                        expected_disk_id: preparation.target.disk_id,
                    },
                    &preparation.proposal_digest,
                )?)
            }
        };
        target.recheck()?;
        let plan = native
            .as_ref()
            .map(|status| -> Result<Plan> {
                Ok(Plan {
                    context: self.context.clone(),
                    credential: target.credential_revision()?,
                    preparation: status.preparation.clone(),
                })
            })
            .transpose()?;
        Ok(Status {
            native,
            plan,
            service_started: false,
            completed_readmission: false,
            published_current,
        })
    }
}
pub(crate) fn worker_entry() -> Result<()> {
    crate::service_worker::entry::<Request>()
}
fn run(input: InventoryInput, action: Action) -> Result<Status> {
    stopped(input.external_writers_stopped)?;
    let root = Root::open(input.service_user.clone())?;
    let context = root.context.clone();
    if let Action::Synchronize { plan }
    | Action::PrepareFirst { plan }
    | Action::PublishFirst { plan }
    | Action::InspectPublished { plan }
    | Action::PrepareActivation { plan }
    | Action::DecideIo { plan }
    | Action::PrepareGenerationLog { plan }
    | Action::SelectGeneration { plan }
    | Action::InspectGeneration { plan } = &action
        && plan.context != context
    {
        return Err("chain target no longer matches its original host/account/layout".into());
    }
    drop(root);
    let result = crate::service_worker::run(Request {
        context: context.clone(),
        action,
    })?;
    let root = Root::open(input.service_user)?;
    if root.context != context {
        return Err("chain target may already be selected; host layout changed, preserve native evidence and query original status".into());
    }
    root.host.recheck()?;
    root.layout.recheck()?;
    Ok(result)
}
pub(crate) fn status(input: InventoryInput) -> Result<Status> {
    run(input, Action::Status)
}
pub(crate) fn synchronize(input: Commit) -> Result<Status> {
    if input.confirmation != "prepare-chain-target-without-activation"
        || !input.acknowledge_reserved_request_without_activation
    {
        return Err("explicitly acknowledge source reservation, immutable target decision, no active writer and no completed readmission".into());
    }
    run(
        InventoryInput {
            service_user: input.service_user,
            external_writers_stopped: input.external_writers_stopped,
        },
        Action::Synchronize { plan: input.plan },
    )
}

pub(crate) fn prepare_first(input: FirstInput) -> Result<Status> {
    if input.confirmation != "prepare-chain-first-inputs-without-publication"
        || !input.acknowledge_unpublished_inputs
    {
        return Err("explicitly acknowledge isolated native inputs, extra SYSTEM/target storage, no Publication or active writer, and same-request recovery only".into());
    }
    run(
        InventoryInput {
            service_user: input.service_user,
            external_writers_stopped: input.external_writers_stopped,
        },
        Action::PrepareFirst { plan: input.plan },
    )
}

pub(crate) fn publish_first(input: PublicationInput) -> Result<Status> {
    if input.confirmation != "publish-chain-first-slot-without-activation"
        || !input.acknowledge_publication_without_activation
    {
        return Err("explicitly acknowledge actual irreversible native ACCEPT/FINALIZE/Publication for the same operation; no Current/writer or service activation, no cancellation or replacement".into());
    }
    run(
        InventoryInput {
            service_user: input.service_user,
            external_writers_stopped: input.external_writers_stopped,
        },
        Action::PublishFirst { plan: input.plan },
    )
}

/// A real Current consumer, not an activation or request completion endpoint.
pub(crate) fn inspect_current(input: CurrentInput) -> Result<Status> {
    run(
        InventoryInput {
            service_user: input.service_user,
            external_writers_stopped: input.external_writers_stopped,
        },
        Action::InspectPublished { plan: input.plan },
    )
}

pub(crate) fn prepare_activation(input: ActivationPlanInput) -> Result<Status> {
    if input.confirmation != "prepare-chain-activation-plan-without-consuming-stop"
        || !input.acknowledge_plan_without_stop_consumption
    {
        return Err("explicitly acknowledge an immutable future epoch/locator plan only; original Stop and active discovery remain unchanged, no writer or completed readmission".into());
    }
    run(
        InventoryInput {
            service_user: input.service_user,
            external_writers_stopped: input.external_writers_stopped,
        },
        Action::PrepareActivation { plan: input.plan },
    )
}

/// Actual immutable native choice, still not a Stop/writer activation API.
pub(crate) fn decide_io(input: IoDecisionInput) -> Result<Status> {
    if input.confirmation != "decide-chain-io-epoch-without-activation"
        || !input.acknowledge_irreversible_decision_without_activation
    {
        return Err("explicitly acknowledge the irreversible original-target epoch decision; no cancellation, replacement, Stop consumption, active writer or completed readmission".into());
    }
    run(
        InventoryInput {
            service_user: input.service_user,
            external_writers_stopped: input.external_writers_stopped,
        },
        Action::DecideIo { plan: input.plan },
    )
}

pub(crate) fn prepare_generation_log(input: GenerationLogInput) -> Result<Status> {
    if input.confirmation != "prepare-chain-generation-log-without-active-writer"
        || !input.acknowledge_generation_log_without_active_writer
    {
        return Err("explicitly acknowledge immutable generation log and native object installation, original-request recovery only, no active selector, Stop consumption, writer or completed readmission".into());
    }
    run(
        InventoryInput {
            service_user: input.service_user,
            external_writers_stopped: input.external_writers_stopped,
        },
        Action::PrepareGenerationLog { plan: input.plan },
    )
}

pub(crate) fn select_generation(input: GenerationSelectionInput) -> Result<Status> {
    if input.confirmation != "select-chain-generation-without-active-writer"
        || !input.acknowledge_selector_without_active_writer
    {
        return Err("explicitly acknowledge original immutable cold selector, same-request recovery only, no Stop consumption, active writer or completed readmission".into());
    }
    run(
        InventoryInput {
            service_user: input.service_user,
            external_writers_stopped: input.external_writers_stopped,
        },
        Action::SelectGeneration { plan: input.plan },
    )
}
pub(crate) fn inspect_generation(input: CurrentInput) -> Result<Status> {
    run(
        InventoryInput {
            service_user: input.service_user,
            external_writers_stopped: input.external_writers_stopped,
        },
        Action::InspectGeneration { plan: input.plan },
    )
}
