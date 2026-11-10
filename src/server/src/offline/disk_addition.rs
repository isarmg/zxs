//! Offline-only root coordinator. Native membership records remain shared with
//! the daemon; helper plans, privilege separation and HTTP work stay here.
use crate::offline::coordinator::{self, digest as plan_digest};
mod worker;
use crate::offline::{Result, managed_layout as layout};
use data_serialization::{Deserialize, Serialize};
use storage_engine::{
    OperationId,
    storage_addition::{AdditionPreview, AdditionStatus},
};
use storage_server::auth::CredentialRevision;
const MAX_RECORD_BYTES: usize = 64 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct NewDirectory {
    root_path: String,
    slot_id: String,
    level: u32,
    upstream_slot_id: Option<String>,
    metadata_member: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Input {
    service_user: String,
    operation_id: OperationId,
    additions: Vec<NewDirectory>,
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
}
#[derive(Clone, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "zxs-helper-active-empty-directory-addition")]
    Addition,
}
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Plan {
    format: Format,
    context: layout::Context,
    credential: CredentialRevision,
    registered_roots: Vec<String>,
    addition: AdditionPreview,
}
impl Plan {
    fn roots(&self) -> Vec<String> {
        self.registered_roots
            .iter()
            .cloned()
            .chain(
                self.addition
                    .request
                    .additions
                    .iter()
                    .map(|item| item.selection.root_path.clone()),
            )
            .collect()
    }
}
#[derive(Serialize)]
pub(crate) struct Preview {
    plan: Plan,
    request_digest: String,
    original_status: Option<AdditionStatus>,
    changes_existing_pool_membership: bool,
    imports_foreign_history: bool,
    formats_or_mounts_devices: bool,
    starts_service: bool,
    complete_installation_ready: bool,
}
type WorkerRequest = crate::offline::service_worker::Envelope<Action>;
#[derive(Serialize, Deserialize)]
#[serialization(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Preview {
        operation_id: OperationId,
        additions: Vec<NewDirectory>,
    },
    Status {
        plan: Plan,
    },
    Commit {
        plan: Plan,
    },
}
#[derive(Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Output {
    Preview { plan: Plan },
    Status { status: AdditionStatus },
    Original { status: Option<AdditionStatus> },
}
impl crate::offline::service_worker::Action for Action {
    type Output = Output;
    const COMMAND: &'static str = "internal-addition-worker";
    fn execute(request: WorkerRequest) -> Result<Output> {
        worker::execute(request)
    }
}
pub(crate) fn worker_entry() -> Result<()> {
    crate::offline::service_worker::entry::<WorkerRequest>()
}
type Root = coordinator::Root<Plan>;
impl coordinator::Plan for Plan {
    const ROOT_REQUIRED: &'static str = "disk additions require the manually run root helper";
    const CONTEXT_CHANGED: &'static str =
        "original addition host/account/layout identity changed; no substitute was selected";
    const PENDING: &'static str = "addition may be partly or fully published; retain its original plan/digest and inspect or continue only that operation; no cleanup or rollback was performed";
    fn context(&self) -> &layout::Context {
        &self.context
    }
}
impl coordinator::RetainedPlan for Plan {
    const RECORD_PREFIX: &'static str = ".zxs-helper-addition-";
    const DIGEST_REQUIRED: &'static str =
        "the exact lowercase original addition digest is required";
    const MISSING: &'static str = "original addition plan is missing; preserve all material";
    const DIGEST_CHANGED: &'static str =
        "retained addition plan digest differs; preserved without retry";
}

fn stopped(value: bool) -> Result<()> {
    if !value {
        return Err("stop ZXS, Samba, older binaries and other writers before opening this pool; explicit acknowledgment is required, not proof of external exclusion".into());
    }
    Ok(())
}
fn request_digest(value: &str) -> Result<()> {
    coordinator::request_digest::<Plan>(value)
}
fn current_roots_match(plan: &Plan, current: &[String]) -> Result<()> {
    use std::collections::BTreeSet;
    let current: BTreeSet<_> = current.iter().collect();
    let original: BTreeSet<_> = plan.registered_roots.iter().collect();
    let all_roots = plan.roots();
    let target: BTreeSet<_> = all_roots.iter().collect();
    if current != original && current != target {
        return Err("helper root inventory is not the original or exact target configuration; unknown material was not registered".into());
    }
    Ok(())
}
fn preview_result(plan: Plan, status: Option<AdditionStatus>) -> Result<Preview> {
    Ok(Preview {
        request_digest: plan_digest(&plan)?,
        plan,
        original_status: status,
        changes_existing_pool_membership: true,
        imports_foreign_history: false,
        formats_or_mounts_devices: false,
        starts_service: false,
        complete_installation_ready: false,
    })
}
pub(crate) fn preview(input: Input) -> Result<Preview> {
    stopped(input.external_writers_stopped)?;
    layout::names(
        &input
            .additions
            .iter()
            .map(|a| a.root_path.clone())
            .collect::<Vec<_>>(),
    )?;
    let root = Root::open(input.service_user.clone())?;
    let context = root.into_context(); // The service-UID owner takes and holds its own host lease.
    let Output::Preview { plan } = crate::offline::service_worker::run(WorkerRequest {
        context,
        action: Action::Preview {
            operation_id: input.operation_id,
            additions: input.additions,
        },
    })?
    else {
        return Err("unexpected addition preview worker result".into());
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
        action: Action::Status { plan: plan.clone() },
    })?
    else {
        return Err("unexpected original addition status reply".into());
    };
    Root::open(input.service_user)?.check(&plan)?;
    preview_result(plan, status)
}
pub(crate) fn commit(input: Commit) -> Result<json_codec::Value> {
    if input.confirmation != "add-and-initialize-empty-directories" {
        return Err("explicit add-and-initialize-empty-directories confirmation is required before system access".into());
    }
    stopped(input.input.external_writers_stopped)?;
    request_digest(&input.input.request_digest)?;
    if plan_digest(&input.plan)? != input.input.request_digest {
        return Err("addition plan no longer matches its reviewed digest".into());
    }
    let root = Root::open(input.input.service_user.clone())?;
    root.check(&input.plan)?;
    let bytes = json_codec::to_vec(&input.plan)?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err("addition plan exceeds its retained-record budget".into());
    }
    // The IPC envelope repeats the admission context. Reject a plan that fits
    // its local record but cannot be sent to its only execution owner.
    let envelope = WorkerRequest {
        context: root.context.clone(),
        action: Action::Commit {
            plan: input.plan.clone(),
        },
    };
    if json_codec::to_vec(&envelope)?.len() > MAX_RECORD_BYTES {
        return Err(
            "addition execution envelope exceeds its IPC budget; no original intent was created"
                .into(),
        );
    }
    root.host.publish(
        &format!(".zxs-helper-addition-{}", input.input.request_digest),
        &[("plan.json", &bytes)],
    )?;
    let context = root.into_context();
    finish(input, context).map_err(coordinator::pending::<Plan>)
}
fn finish(input: Commit, context: layout::Context) -> Result<json_codec::Value> {
    let Output::Status { status } = crate::offline::service_worker::run(WorkerRequest {
        context,
        action: Action::Commit {
            plan: input.plan.clone(),
        },
    })?
    else {
        return Err("unexpected original addition execution reply".into());
    };
    let root = Root::open(input.input.service_user)?;
    root.check(&input.plan)?;
    // This is the exact original native result, not a fabricated whole-system
    // readiness certificate or a foreign/independent-journal migration result.
    Ok(
        json_codec::json!({"format":"zxs-helper-active-addition-result",
        "request_digest":input.input.request_digest, "operation":status,
        "foreign_history_imported":false, "complete_installation_ready":false,
        "devices_formatted":false, "service_started":false}),
    )
}
#[cfg(test)]
#[path = "../../../../test/server/unit/offline/disk_addition_tests.rs"]
mod tests;
