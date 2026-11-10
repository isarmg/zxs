//! Root-owned offline deregistration intent. Never walks a target after D3;
//! retained bytes and historical registration definitions are not deleted.
use crate::offline::coordinator::{self, digest as plan_digest};
pub(crate) mod chain_archive;
pub(crate) mod chain_readmission;
pub(crate) mod chain_target;
pub(crate) mod terminal;
pub(crate) mod terminal_action;
mod worker;
use crate::offline::{
    Result,
    managed_layout::{self as layout, Layout},
};
use data_serialization::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use storage_engine::{
    OperationId,
    storage_lifecycle::{ExitAction, ExitAuthority, ExitPreview, ExitStatus},
};
use storage_server::auth::CredentialRevision;
const MAX_RECORD_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct InventoryInput {
    service_user: String,
    external_writers_stopped: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Input {
    service_user: String,
    operation_id: OperationId,
    slot_id: String,
    expected_disk_id: String,
    external_writers_stopped: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Original {
    service_user: String,
    request_digest: String,
    external_writers_stopped: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Commit {
    input: Original,
    plan: Plan,
    confirmation: String,
    acknowledge_current_view_loss: bool,
    acknowledge_terminal_management: bool,
}
#[derive(Clone, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "zxs-helper-active-disk-deregistration")]
    Deregistration,
}
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Plan {
    format: Format,
    context: layout::Context,
    credential: CredentialRevision,
    exit: ExitPreview,
}
#[derive(Serialize)]
pub(crate) struct Preview {
    plan: Plan,
    request_digest: String,
    original_status: Option<ExitStatus>,
    deregisters_current_member: bool,
    will_enter_terminal_management: bool,
    physical_bytes_erased: bool,
    remaining_content_preservation_verified: bool,
    complete_installation_ready: bool,
    service_started: bool,
}
type WorkerRequest = crate::offline::service_worker::Envelope<Action>;
#[derive(Serialize, Deserialize)]
#[serialization(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    TerminalPreview {
        request: storage_engine::storage_lifecycle::ExitRequest,
    },
    TerminalOriginal {
        plan: terminal_action::Plan,
    },
    TerminalCommit {
        plan: terminal_action::Plan,
    },
    Inventory,
    InspectTerminal {
        slot_id: String,
        expected_disk_id: String,
    },
    Preview {
        operation_id: OperationId,
        slot_id: String,
        expected_disk_id: String,
    },
    Original {
        plan: Plan,
    },
    Commit {
        plan: Plan,
    },
}
#[derive(Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Output {
    TerminalPreview {
        plan: terminal_action::Plan,
    },
    TerminalResult {
        result: Option<storage_engine::OperationResult>,
    },
    Inventory {
        inventory: worker::Inventory,
    },
    TerminalInspection {
        inspection: terminal::Inspection,
    },
    Preview {
        plan: Plan,
    },
    Original {
        status: Option<ExitStatus>,
    },
    Status {
        status: ExitStatus,
    },
}
impl crate::offline::service_worker::Action for Action {
    type Output = Output;
    const COMMAND: &'static str = "internal-removal-worker";
    fn execute(request: WorkerRequest) -> Result<Output> {
        worker::execute(request)
    }
}
pub(crate) fn worker_entry() -> Result<()> {
    crate::offline::service_worker::entry::<WorkerRequest>()
}
type Root = coordinator::Root<Plan>;
impl coordinator::Plan for Plan {
    const ROOT_REQUIRED: &'static str = "deregistration requires the manually run root helper";
    const CONTEXT_CHANGED: &'static str =
        "original deregistration host/account/layout identity changed; no substitute was selected";
    const PENDING: &'static str = "deregistration may be partly or fully published; retain the original operation/digest and inspect or continue only it; do not unplug media on an uncertain result, clean up or start another exit";
    fn context(&self) -> &layout::Context {
        &self.context
    }
    fn check_storage(&self, _layout: &layout::Layout) -> Result<()> {
        // Status remains available after D3 without probing the old target.
        supported(&self.exit)
    }
}
impl coordinator::RetainedPlan for Plan {
    const RECORD_PREFIX: &'static str = ".zxs-helper-deregister-";
    const DIGEST_REQUIRED: &'static str =
        "the exact lowercase original deregistration digest is required";
    const MISSING: &'static str = "original deregistration plan is missing; preserve all material";
    const DIGEST_CHANGED: &'static str = "retained deregistration plan digest differs";
}

fn supported(preview: &ExitPreview) -> Result<()> {
    if preview.request.action != ExitAction::Deregister {
        return Err("planned-offline is not an available helper action".into());
    }
    if (preview.authority == ExitAuthority::TerminalManagement)
        != (preview.target_metadata_members == 0)
    {
        return Err(
            "exit authority differs from its declared remaining metadata membership".into(),
        );
    }
    layout::names(std::slice::from_ref(&preview.target.root_path))?;
    Ok(())
}
fn stopped(value: bool) -> Result<()> {
    if !value {
        return Err("stop ZXS, Samba, older binaries and other writers before opening existing storage; explicit acknowledgment is required, not proof of external exclusion".into());
    }
    Ok(())
}
fn request_digest(value: &str) -> Result<()> {
    coordinator::request_digest::<Plan>(value)
}
fn preview_result(plan: Plan, original_status: Option<ExitStatus>) -> Result<Preview> {
    supported(&plan.exit)?;
    Ok(Preview {
        request_digest: plan_digest(&plan)?,
        will_enter_terminal_management: plan.exit.authority == ExitAuthority::TerminalManagement,
        plan,
        original_status,
        deregisters_current_member: true,
        physical_bytes_erased: false,
        remaining_content_preservation_verified: false,
        complete_installation_ready: false,
        service_started: false,
    })
}
pub(crate) fn inventory(input: InventoryInput) -> Result<worker::Inventory> {
    stopped(input.external_writers_stopped)?;
    let root = Root::open(input.service_user.clone())?;
    let context = root.into_context();
    let Output::Inventory { inventory } = crate::offline::service_worker::run(WorkerRequest {
        context: context.clone(),
        action: Action::Inventory,
    })?
    else {
        return Err("unexpected deregistration inventory reply".into());
    };
    let root = Root::open(input.service_user)?;
    if root.context != context {
        return Err("inventory root/account identity changed; no operation was admitted".into());
    }
    Ok(inventory)
}
pub(crate) fn preview(input: Input) -> Result<Preview> {
    stopped(input.external_writers_stopped)?;
    let root = Root::open(input.service_user.clone())?;
    let context = root.into_context();
    let Output::Preview { plan } = crate::offline::service_worker::run(WorkerRequest {
        context,
        action: Action::Preview {
            operation_id: input.operation_id,
            slot_id: input.slot_id,
            expected_disk_id: input.expected_disk_id,
        },
    })?
    else {
        return Err("unexpected deregistration preview reply".into());
    };
    Root::open(input.service_user)?.check(&plan)?;
    preview_result(plan, None)
}
pub(crate) fn recover_preview(input: Original) -> Result<Preview> {
    stopped(input.external_writers_stopped)?;
    let root = Root::open(input.service_user.clone())?;
    let plan = root.retained(&input.request_digest)?;
    let context = root.into_context();
    let Output::Original { status } = crate::offline::service_worker::run(WorkerRequest {
        context,
        action: Action::Original { plan: plan.clone() },
    })?
    else {
        return Err("unexpected original deregistration reply".into());
    };
    Root::open(input.service_user)?.check(&plan)?;
    preview_result(plan, status)
}
fn confirm(input: &Commit) -> Result<()> {
    supported(&input.plan.exit)?;
    let terminal = input.plan.exit.authority == ExitAuthority::TerminalManagement;
    let expected = if terminal {
        "deregister-final-voter-stop-storage"
    } else {
        "deregister-member-preserve-bytes"
    };
    if input.confirmation != expected
        || !input.acknowledge_current_view_loss
        || (terminal && !input.acknowledge_terminal_management)
    {
        return Err("exact deregistration confirmation, current-view loss acknowledgment and, for the final voter, separate acknowledgment of terminal management with no active storage are required before system access".into());
    }
    Ok(())
}
pub(crate) fn commit(input: Commit) -> Result<json_codec::Value> {
    confirm(&input)?;
    stopped(input.input.external_writers_stopped)?;
    supported(&input.plan.exit)?;
    request_digest(&input.input.request_digest)?;
    if plan_digest(&input.plan)? != input.input.request_digest {
        return Err("deregistration plan no longer matches its reviewed digest".into());
    }
    let root = Root::open(input.input.service_user.clone())?;
    root.check(&input.plan)?;
    let context = root.context.clone();
    let request = WorkerRequest {
        context: context.clone(),
        action: Action::Commit {
            plan: input.plan.clone(),
        },
    };
    if json_codec::to_vec(&request)?.len() > MAX_RECORD_BYTES {
        return Err(
            "deregistration envelope exceeds IPC budget; no original intent was saved".into(),
        );
    }
    let bytes = json_codec::to_vec(&input.plan)?;
    root.host.publish(
        &format!(".zxs-helper-deregister-{}", input.input.request_digest),
        &[("plan.json", &bytes)],
    )?;
    drop(root);
    finish(input, request).map_err(coordinator::pending::<Plan>)
}
fn finish(input: Commit, request: WorkerRequest) -> Result<json_codec::Value> {
    let Output::Status { status } = crate::offline::service_worker::run(request)? else {
        return Err("unexpected deregistration execution reply".into());
    };
    Root::open(input.input.service_user)?.check(&input.plan)?;
    Ok(
        json_codec::json!({"format":"zxs-helper-active-deregistration-result",
        "request_digest":input.input.request_digest,"operation":status,
        "requested_terminal_management":input.plan.exit.authority == ExitAuthority::TerminalManagement,
        "physical_bytes_erased":false,"remaining_content_preservation_verified":false,
        "service_started":false,"complete_installation_ready":false}),
    )
}
#[cfg(test)]
#[path = "../../../../test/server/unit/offline/disk_removal_tests.rs"]
mod tests;
