//! Root-coordinated first storage batch. Domain genesis and recovery stay in
//! SetupRecovery/Engine; all helper IPC, plans and Web orchestration live here.
use crate::offline::coordinator::{self, digest};
use crate::offline::managed_layout as layout;
pub(crate) mod directories;
mod worker;
use crate::offline::Result;
use data_serialization::{Deserialize, Serialize};
use json_codec::json;
use storage_engine::storage_directories::CandidateSelection;
use storage_server::auth::CredentialRevision;
const MAX_RECORD_BYTES: usize = 64 * 1024;
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Input {
    pub service_user: String,
    pub roots: Vec<String>,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Original {
    pub service_user: String,
    pub request_digest: String,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Commit {
    pub input: Original,
    pub confirmation: String,
    pub external_writers_stopped: bool,
    pub plan: Plan,
}
#[derive(Clone, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "zxs-helper-first-storage-batch")]
    FirstStorage,
}
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Plan {
    format: Format,
    context: layout::Context,
    credential: CredentialRevision,
    selections: Vec<CandidateSelection>,
}
impl Plan {
    fn roots(&self) -> Vec<String> {
        self.selections
            .iter()
            .map(|item| item.root_path.clone())
            .collect()
    }
    fn activation(&self) -> storage_server::setup_recovery::StorageActivationRequest {
        storage_server::setup_recovery::StorageActivationRequest {
            selections: self.selections.clone(),
            mode: storage_engine::Mode::Production,
        }
    }
}
#[derive(Serialize)]
pub(crate) struct Preview {
    plan: Plan,
    pub request_digest: String,
    recovery: bool,
    initializes_all_selected_roots_as_independent_primaries: bool,
    formats_or_mounts_devices: bool,
    starts_service: bool,
    complete_installation_ready: bool,
}
type WorkerRequest = crate::offline::service_worker::Envelope<Action>;
#[derive(Serialize, Deserialize)]
#[serialization(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Preview { roots: Vec<String> },
    InspectOriginal { plan: Plan },
    Execute { plan: Plan },
}
#[derive(Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum WorkerOutput {
    Preview { plan: Box<Plan> },
    OriginalInspected,
    Initialized { report: worker::Materialized },
}
impl crate::offline::service_worker::Action for Action {
    type Output = WorkerOutput;
    const COMMAND: &'static str = "internal-storage-worker";
    fn execute(request: WorkerRequest) -> Result<Self::Output> {
        worker::execute(request)
    }
}
pub(crate) fn worker_entry() -> Result<()> {
    crate::offline::service_worker::entry::<WorkerRequest>()
}
type Root = coordinator::Root<Plan>;
impl coordinator::Plan for Plan {
    const ROOT_REQUIRED: &'static str = "first storage requires the manually run root helper";
    const CONTEXT_CHANGED: &'static str =
        "first-storage layout/account/host no longer matches its original plan";
    const PENDING: &'static str = "first-storage batch may be partly or fully published; retain its original saved plan and request digest, retry only that operation; no disk cleanup, rollback, service start or complete installation readiness is claimed";
    fn context(&self) -> &layout::Context {
        &self.context
    }
    fn check_storage(&self, layout: &layout::Layout) -> Result<()> {
        layout.inventory(&self.roots())
    }
}

fn request(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("first storage requires its exact lowercase preview digest".into());
    }
    Ok(())
}
fn preview_response(plan: Plan, request_digest: String, recovery: bool) -> Preview {
    Preview {
        plan,
        request_digest,
        recovery,
        initializes_all_selected_roots_as_independent_primaries: true,
        formats_or_mounts_devices: false,
        starts_service: false,
        complete_installation_ready: false,
    }
}
pub(crate) fn preview(input: Input) -> Result<Preview> {
    let root = Root::open(input.service_user.clone())?;
    root.layout.inventory(&input.roots)?;
    let context = root.into_context(); // Worker acquires its own lease and revalidates the exact context.
    let WorkerOutput::Preview { plan } = crate::offline::service_worker::run(WorkerRequest {
        context,
        action: Action::Preview { roots: input.roots },
    })?
    else {
        return Err("unexpected first-storage preview worker result".into());
    };
    let plan = *plan;
    let root = Root::open(input.service_user)?;
    root.check(&plan)?;
    let request_digest = digest(&plan)?;
    Ok(preview_response(plan, request_digest, false))
}
fn load(root: &Root, request_digest: &str) -> Result<Option<Plan>> {
    root.host
        .read_operation(
            &format!(".zxs-helper-first-storage-{request_digest}"),
            "plan.json",
            MAX_RECORD_BYTES,
        )?
        .map(|bytes| {
            json_codec::from_slice(&bytes)
                .map_err(|_| "retained first-storage plan is malformed; it was preserved".into())
        })
        .transpose()
}
pub(crate) fn recover_preview(input: Original) -> Result<Preview> {
    request(&input.request_digest)?;
    let root = Root::open(input.service_user.clone())?;
    let plan = load(&root, &input.request_digest)?.ok_or(
        "no complete retained first-storage plan exists; unknown pending files were preserved",
    )?;
    if digest(&plan)? != input.request_digest {
        return Err("retained first-storage plan digest differs".into());
    }
    root.check(&plan)?;
    let context = root.into_context();
    if !matches!(
        crate::offline::service_worker::run(WorkerRequest {
            context,
            action: Action::InspectOriginal { plan: plan.clone() }
        })?,
        WorkerOutput::OriginalInspected
    ) {
        return Err("unexpected first-storage recovery preview result".into());
    }
    Root::open(input.service_user)?.check(&plan)?;
    Ok(preview_response(plan, input.request_digest, true))
}
pub(crate) fn commit(input: Commit) -> Result<json_codec::Value> {
    if input.confirmation != "initialize-first-storage-batch" || !input.external_writers_stopped {
        return Err("explicit first-batch initialization and external-writer-stop acknowledgments are required before system access".into());
    }
    request(&input.input.request_digest)?;
    if digest(&input.plan)? != input.input.request_digest {
        return Err("first-storage plan differs from the exact preview digest".into());
    }
    let root = Root::open(input.input.service_user.clone())?;
    root.check(&input.plan)?;
    if let Some(original) = load(&root, &input.input.request_digest)?
        && digest(&original)? != input.input.request_digest
    {
        return Err("retained first-storage operation differs; no material was replaced".into());
    }
    let bytes = json_codec::to_vec(&input.plan)?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err("first-storage plan exceeds its retained-record budget".into());
    }
    root.host.publish(
        &format!(".zxs-helper-first-storage-{}", input.input.request_digest),
        &[("plan.json", bytes.as_slice())],
    )?;
    let context = root.into_context();
    finish(input, context).map_err(coordinator::pending::<Plan>)
}
fn finish(input: Commit, context: layout::Context) -> Result<json_codec::Value> {
    let WorkerOutput::Initialized { report } =
        crate::offline::service_worker::run(WorkerRequest {
            context,
            action: Action::Execute {
                plan: input.plan.clone(),
            },
        })?
    else {
        return Err("unexpected first-storage execution reply".into());
    };
    // A daemon winning this handoff returns busy, not a false completion. The
    // durable activation itself is never erased or re-created to retry a reply.
    let root = Root::open(input.input.service_user)?;
    root.check(&input.plan)?;
    let receipt = json!({"format":"zxs-helper-first-storage-batch-complete","request_digest":input.input.request_digest,
        "materialized":report,"storage_activation_completed":true,"complete_installation_ready":false,
        "clean_start_gate_installed":false,"service_started":false,"samba_credentials_changed":false,
        "independent_disk_authority_migrated":false,"object_databases":"no_user_objects_in_first_pool"});
    let bytes = json_codec::to_vec(&receipt)?;
    root.host.publish(
        &format!(
            ".zxs-helper-first-storage-complete-{}",
            input.input.request_digest
        ),
        &[("receipt.json", bytes.as_slice())],
    )?;
    root.check(&input.plan)?;
    Ok(receipt)
}
#[cfg(test)]
#[path = "../../../../../test/server/unit/offline/initialization/storage_tests.rs"]
mod tests;
