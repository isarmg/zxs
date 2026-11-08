//! Root-owned offline deregistration intent. Never walks a target after D3;
//! retained bytes and historical registration definitions are not deleted.
mod worker;
use crate::{
    Result,
    managed_layout::{self as layout, Layout},
    private_io::BundleDestination,
};
use data_serialization::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use storage_contract::{
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
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct WorkerRequest {
    context: layout::Context,
    action: Action,
}
#[derive(Serialize, Deserialize)]
#[serialization(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Inventory,
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
    Inventory { inventory: worker::Inventory },
    Preview { plan: Plan },
    Original { status: Option<ExitStatus> },
    Status { status: ExitStatus },
}
impl crate::service_worker::Request for WorkerRequest {
    type Output = Output;
    const COMMAND: &'static str = "internal-removal-worker";
    fn account(&self) -> &crate::account::Snapshot {
        &self.context.account
    }
    fn execute(self) -> Result<Output> {
        worker::execute(self)
    }
}
pub(crate) fn worker_entry() -> Result<()> {
    crate::service_worker::entry::<WorkerRequest>()
}
struct Root {
    host: BundleDestination,
    layout: Layout,
    context: layout::Context,
}
impl Root {
    fn open(user: String) -> Result<Self> {
        if rustix::process::geteuid().as_raw() != 0 {
            return Err("deregistration requires the manually run root helper".into());
        }
        let host = BundleDestination::open(Path::new("/zxs"))?;
        let layout = Layout::open(user)?;
        let context = layout.context(host.identity());
        Ok(Self {
            host,
            layout,
            context,
        })
    }
    fn check(&self, plan: &Plan) -> Result<()> {
        self.host.recheck()?;
        self.layout.recheck()?;
        if self.context != plan.context {
            return Err("original deregistration host/account/layout identity changed; no substitute was selected".into());
        }
        supported(&plan.exit)
        // Deliberately no target exists/stat/open/inventory here. Successful
        // deregistration must remain queryable after its media is unplugged.
    }
    fn load(&self, digest: &str) -> Result<Plan> {
        request_digest(digest)?;
        let bytes = self
            .host
            .read_operation(
                &format!(".zxs-helper-deregister-{digest}"),
                "plan.json",
                MAX_RECORD_BYTES,
            )?
            .ok_or("original deregistration plan is missing; preserve all material")?;
        let plan: Plan = json_codec::from_slice(&bytes)?;
        if plan_digest(&plan)? != digest {
            return Err("retained deregistration plan digest differs".into());
        }
        self.check(&plan)?;
        Ok(plan)
    }
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
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("the exact lowercase original deregistration digest is required".into());
    }
    Ok(())
}
fn plan_digest(plan: &Plan) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(json_codec::to_vec(plan)?)))
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
    let context = root.context.clone();
    drop(root);
    let Output::Inventory { inventory } = crate::service_worker::run(WorkerRequest {
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
    let context = root.context.clone();
    drop(root);
    let Output::Preview { plan } = crate::service_worker::run(WorkerRequest {
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
    let plan = root.load(&input.request_digest)?;
    let context = root.context.clone();
    drop(root);
    let Output::Original { status } = crate::service_worker::run(WorkerRequest {
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
    finish(input, request).map_err(|source| Box::new(Pending { source }) as _)
}
fn finish(input: Commit, request: WorkerRequest) -> Result<json_codec::Value> {
    let Output::Status { status } = crate::service_worker::run(request)? else {
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
#[derive(Debug)]
struct Pending {
    source: Box<dyn std::error::Error + Send + Sync>,
}
impl std::fmt::Display for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "deregistration may be partly or fully published; retain the original operation/digest and inspect or continue only it; do not unplug media on an uncertain result, clean up or start another exit: {}",
            self.source
        )
    }
}
impl std::error::Error for Pending {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}
#[cfg(test)]
#[path = "../../test/helper/unit/disk_removal_tests.rs"]
mod tests;
