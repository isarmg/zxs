//! Offline-only root coordinator. Native membership records remain shared with
//! the daemon; helper plans, privilege separation and HTTP work stay here.
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
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct WorkerRequest {
    context: layout::Context,
    action: Action,
}
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
impl crate::service_worker::Request for WorkerRequest {
    type Output = Output;
    const COMMAND: &'static str = "internal-addition-worker";
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
            return Err("disk additions require the manually run root helper".into());
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
            return Err("original addition host/account/layout identity changed; no substitute was selected".into());
        }
        self.layout.inventory_all(&plan.roots())
    }
    fn load(&self, digest: &str) -> Result<Plan> {
        request_digest(digest)?;
        let bytes = self
            .host
            .read_operation(
                &format!(".zxs-helper-addition-{digest}"),
                "plan.json",
                MAX_RECORD_BYTES,
            )?
            .ok_or("original addition plan is missing; preserve all material")?;
        let plan: Plan = json_codec::from_slice(&bytes)?;
        if plan_digest(&plan)? != digest {
            return Err("retained addition plan digest differs; preserved without retry".into());
        }
        self.check(&plan)?;
        Ok(plan)
    }
}
fn stopped(value: bool) -> Result<()> {
    if !value {
        return Err("stop ZXS, Samba, older binaries and other writers before opening this pool; explicit acknowledgment is required, not proof of external exclusion".into());
    }
    Ok(())
}
fn request_digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("the exact lowercase original addition digest is required".into());
    }
    Ok(())
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
fn plan_digest(plan: &Plan) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(json_codec::to_vec(plan)?)))
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
    let context = root.context.clone();
    drop(root); // The service-UID owner takes and holds its own host lease.
    let Output::Preview { plan } = crate::service_worker::run(WorkerRequest {
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
    let plan = root.load(&input.request_digest)?;
    let context = root.context.clone();
    drop(root);
    let Output::Original { status } = crate::service_worker::run(WorkerRequest {
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
    let context = root.context.clone();
    drop(root);
    finish(input, context).map_err(|source| Box::new(Pending { source }) as _)
}
fn finish(input: Commit, context: layout::Context) -> Result<json_codec::Value> {
    let Output::Status { status } = crate::service_worker::run(WorkerRequest {
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
#[derive(Debug)]
struct Pending {
    source: Box<dyn std::error::Error + Send + Sync>,
}
impl std::fmt::Display for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "addition may be partly or fully published; retain its original plan/digest and inspect or continue only that operation; no cleanup or rollback was performed: {}",
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
#[path = "../../test/helper/unit/disk_addition_tests.rs"]
mod tests;
