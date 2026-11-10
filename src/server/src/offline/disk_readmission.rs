//! Offline root coordinator for the exact retained terminal metadata lane.
//! Root plans never certify publication; native SYSTEM decisions own recovery.
use crate::offline::coordinator::{self, digest as plan_digest};
mod worker;
use crate::offline::{
    Result,
    managed_layout::{self as layout, Layout},
};
use data_serialization::{Deserialize, Serialize};
use std::path::Path;
use storage_engine::{
    OperationId,
    storage_readmission::{ReadmissionPreview, ReadmissionStatus},
};
use storage_server::auth::CredentialRevision;
const MAX_RECORD_BYTES: usize = 64 * 1024;

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
    acknowledge_original_directory_and_new_epoch: bool,
}
#[derive(Clone, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "zxs-helper-retained-terminal-readmission")]
    Readmission,
}
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Plan {
    format: Format,
    context: layout::Context,
    credential: CredentialRevision,
    readmission: ReadmissionPreview,
}
#[derive(Serialize)]
pub(crate) struct Preview {
    plan: Plan,
    request_digest: String,
    original_status: Option<ReadmissionStatus>,
    opens_original_directory_on_confirmation: bool,
    restores_first_metadata_voter: bool,
    imports_foreign_history: bool,
    supports_after_terminal_retirement: bool,
    service_started: bool,
    complete_installation_ready: bool,
}
type WorkerRequest = crate::offline::service_worker::Envelope<Action>;
#[derive(Serialize, Deserialize)]
#[serialization(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Preview {
        request: storage_engine::storage_readmission::ReadmissionRequest,
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
    Preview { plan: Plan },
    Original { status: Option<ReadmissionStatus> },
    Status { status: ReadmissionStatus },
}
impl crate::offline::service_worker::Action for Action {
    type Output = Output;
    const COMMAND: &'static str = "internal-readmission-worker";
    fn execute(request: WorkerRequest) -> Result<Output> {
        worker::execute(request)
    }
}
pub(crate) fn worker_entry() -> Result<()> {
    crate::offline::service_worker::entry::<WorkerRequest>()
}
type Root = coordinator::Root<Plan>;
impl coordinator::Plan for Plan {
    const ROOT_REQUIRED: &'static str =
        "retained readmission requires the manually run root helper";
    const CONTEXT_CHANGED: &'static str =
        "readmission host/account/layout changed; no replacement is selected";
    const PENDING: &'static str = "readmission may be pending or published; retain and query the original operation/digest before retrying, without changing the target or clearing evidence";
    fn context(&self) -> &layout::Context {
        &self.context
    }
    fn check_storage(&self, _layout: &layout::Layout) -> Result<()> {
        supported(&self.readmission)?;
        // Syntax-only: original result lookup does not probe the target.
        layout::names(std::slice::from_ref(&self.readmission.target.root_path)).map(|_| ())
    }
}
impl coordinator::RetainedPlan for Plan {
    const RECORD_PREFIX: &'static str = ".zxs-helper-terminal-readmission-";
    const DIGEST_REQUIRED: &'static str =
        "the exact lowercase original readmission digest is required";
    const MISSING: &'static str =
        "original retained readmission plan is missing; preserve native evidence";
    const DIGEST_CHANGED: &'static str = "retained readmission digest differs";
}

fn supported(preview: &ReadmissionPreview) -> Result<()> {
    if !preview.authority.is_terminal() || !preview.metadata_member {
        return Err(
            "this helper lane only restores one still-registered Disabled terminal metadata member"
                .into(),
        );
    }
    Ok(())
}
fn stopped(value: bool) -> Result<()> {
    if !value {
        return Err("stop ZXS, Samba, older binaries and other writers; host exclusion does not prove external writer shutdown".into());
    }
    Ok(())
}
fn request_digest(value: &str) -> Result<()> {
    coordinator::request_digest::<Plan>(value)
}
fn record_name(digest: &str) -> String {
    format!(".zxs-helper-terminal-readmission-{digest}")
}
fn response(plan: Plan, status: Option<ReadmissionStatus>) -> Result<Preview> {
    Ok(Preview {
        request_digest: plan_digest(&plan)?,
        plan,
        original_status: status,
        opens_original_directory_on_confirmation: true,
        restores_first_metadata_voter: true,
        imports_foreign_history: false,
        supports_after_terminal_retirement: false,
        service_started: false,
        complete_installation_ready: false,
    })
}
pub(crate) fn preview(input: Input) -> Result<Preview> {
    stopped(input.external_writers_stopped)?;
    let root = Root::open(input.service_user.clone())?;
    let context = root.into_context();
    let Output::Preview { plan } = crate::offline::service_worker::run(WorkerRequest {
        context,
        action: Action::Preview {
            request: storage_engine::storage_readmission::ReadmissionRequest {
                operation_id: input.operation_id,
                slot_id: input.slot_id,
                expected_disk_id: input.expected_disk_id,
            },
        },
    })?
    else {
        return Err("unexpected readmission preview response".into());
    };
    Root::open(input.service_user)?.check(&plan)?;
    response(plan, None)
}
pub(crate) fn original(input: Original) -> Result<Preview> {
    stopped(input.external_writers_stopped)?;
    request_digest(&input.request_digest)?;
    let root = Root::open(input.service_user.clone())?;
    let plan = root.retained(&input.request_digest)?;
    let context = root.into_context();
    let Output::Original { status } = crate::offline::service_worker::run(WorkerRequest {
        context,
        action: Action::Original { plan: plan.clone() },
    })?
    else {
        return Err("unexpected readmission original response".into());
    };
    Root::open(input.service_user)?.check(&plan)?;
    response(plan, status)
}
pub(crate) fn commit(input: Commit) -> Result<json_codec::Value> {
    stopped(input.input.external_writers_stopped)?;
    if input.confirmation != "readmit-retained-terminal-metadata"
        || !input.acknowledge_original_directory_and_new_epoch
    {
        return Err("explicitly confirm reopening this exact Disabled directory and issuing its new I/O epoch".into());
    }
    supported(&input.plan.readmission)?;
    request_digest(&input.input.request_digest)?;
    if plan_digest(&input.plan)? != input.input.request_digest {
        return Err("readmission confirmation differs from its exact preview".into());
    }
    let root = Root::open(input.input.service_user.clone())?;
    root.check(&input.plan)?;
    let request = WorkerRequest {
        context: root.context.clone(),
        action: Action::Commit {
            plan: input.plan.clone(),
        },
    };
    let bytes = json_codec::to_vec(&input.plan)?;
    if bytes.len() > MAX_RECORD_BYTES || json_codec::to_vec(&request)?.len() > MAX_RECORD_BYTES {
        return Err("readmission plan exceeds its bounded envelope; no intent saved".into());
    }
    root.host.publish(
        &record_name(&input.input.request_digest),
        &[("plan.json", &bytes)],
    )?;
    drop(root);
    finish(input, request).map_err(coordinator::pending::<Plan>)
}
fn finish(input: Commit, request: WorkerRequest) -> Result<json_codec::Value> {
    let Output::Status { status } = crate::offline::service_worker::run(request)? else {
        return Err(
            "unexpected native readmission completion response; inspect original operation".into(),
        );
    };
    Root::open(input.input.service_user)?.check(&input.plan)?;
    Ok(json_codec::json!({
        "format":"zxs-helper-terminal-readmission-result", "request_digest":input.input.request_digest,
        "status":status, "service_started":false, "complete_installation_ready":false,
        "imports_foreign_history":false, "physical_bytes_erased":false,
        "supports_after_terminal_retirement":false,
    }))
}

#[cfg(test)]
#[path = "../../../../test/server/unit/offline/disk_readmission_tests.rs"]
mod tests;
