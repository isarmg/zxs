//! Root-coordinated first storage batch. Domain genesis and recovery stay in
//! SetupRecovery/Engine; all helper IPC, plans and Web orchestration live here.
mod layout;
pub(crate) mod process;
mod worker;
use crate::{Result, private_io::BundleDestination};
use data_serialization::{Deserialize, Serialize};
use json_codec::json;
use sha2::{Digest, Sha256};
use std::path::Path;
use storage_contract::storage_directories::CandidateSelection;
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
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct WorkerRequest {
    context: layout::Context,
    action: Action,
}
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
    Preview { plan: Plan },
    OriginalInspected,
    Initialized { report: worker::Materialized },
}
#[derive(Serialize, Deserialize)]
#[serialization(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum WorkerReply {
    Success { result: WorkerOutput },
    Failed { message: String },
}
struct Root {
    layout: layout::Layout,
    context: layout::Context,
    host: BundleDestination,
}
impl Root {
    fn open(user: String) -> Result<Self> {
        if rustix::process::geteuid().as_raw() != 0 {
            return Err("first storage requires the manually run root helper".into());
        }
        let host = BundleDestination::open(Path::new("/zxs"))?;
        let layout = layout::Layout::open(user)?;
        let context = layout.context(host.identity());
        Ok(Self {
            layout,
            context,
            host,
        })
    }
    fn check(&self, plan: &Plan) -> Result<()> {
        self.host.recheck()?;
        self.layout.recheck()?;
        if self.context != plan.context {
            return Err(
                "first-storage layout/account/host no longer matches its original plan".into(),
            );
        }
        self.layout.inventory(&plan.roots())
    }
}
fn digest(plan: &Plan) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(json_codec::to_vec(plan)?)))
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
    layout::names(&input.roots)?;
    let root = Root::open(input.service_user.clone())?;
    root.layout.inventory(&input.roots)?;
    let context = root.context.clone();
    drop(root); // Worker acquires its own lease and revalidates the exact context.
    let WorkerOutput::Preview { plan } = process::run(WorkerRequest {
        context,
        action: Action::Preview { roots: input.roots },
    })?
    else {
        return Err("unexpected first-storage preview worker result".into());
    };
    let root = Root::open(input.service_user)?;
    root.check(&plan)?;
    Ok(preview_response(plan.clone(), digest(&plan)?, false))
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
    let context = root.context.clone();
    drop(root);
    if !matches!(
        process::run(WorkerRequest {
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
    if let Some(original) = load(&root, &input.input.request_digest)? {
        if digest(&original)? != input.input.request_digest {
            return Err(
                "retained first-storage operation differs; no material was replaced".into(),
            );
        }
    }
    let bytes = json_codec::to_vec(&input.plan)?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err("first-storage plan exceeds its retained-record budget".into());
    }
    root.host.publish(
        &format!(".zxs-helper-first-storage-{}", input.input.request_digest),
        &[("plan.json", bytes.as_slice())],
    )?;
    let context = root.context.clone();
    drop(root);
    finish(input, context)
        .map_err(|source| Box::new(Pending { source }) as Box<dyn std::error::Error + Send + Sync>)
}
fn finish(input: Commit, context: layout::Context) -> Result<json_codec::Value> {
    let WorkerOutput::Initialized { report } = process::run(WorkerRequest {
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
#[derive(Debug)]
struct Pending {
    source: Box<dyn std::error::Error + Send + Sync>,
}
impl std::fmt::Display for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "first-storage batch may be partly or fully published; retain its original saved plan and request digest, retry only that operation; no disk cleanup, rollback, service start or complete installation readiness is claimed: {}",
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
#[path = "../../../test/helper/unit/initialization/storage_tests.rs"]
mod tests;
