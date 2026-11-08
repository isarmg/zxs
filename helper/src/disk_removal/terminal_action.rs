//! Root-retained exact plan for the first management-only retirement successor.
//! Native completion owns Stop/D3; helper files are never completion authority.
use super::*;
use storage_contract::{
    OperationResult,
    storage_lifecycle::{ExitRequest, TerminalRetirementPreview},
};

#[derive(Clone, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "zxs-helper-terminal-retirement")]
    Retirement,
}
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Plan {
    format: Format,
    pub(super) context: layout::Context,
    pub(super) credential: CredentialRevision,
    pub(super) retirement: TerminalRetirementPreview,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Commit {
    input: Original,
    plan: Plan,
    confirmation: String,
    acknowledge_current_view_loss: bool,
    acknowledge_no_metadata_resumption: bool,
}
#[derive(Serialize)]
pub(crate) struct Preview {
    plan: Plan,
    request_digest: String,
    original_result: Option<OperationResult>,
    physical_bytes_erased: bool,
    active_namespace_admitted: bool,
    metadata_resumption_supported: bool,
    another_terminal_retirement_supported: bool,
}
fn digest(plan: &Plan) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(json_codec::to_vec(plan)?)))
}
fn check(root: &Root, plan: &Plan) -> Result<()> {
    root.host.recheck()?;
    root.layout.recheck()?;
    if root.context != plan.context || plan.retirement.request.action != ExitAction::Deregister {
        return Err(
            "terminal retirement roots/account/action changed; no substitute is allowed".into(),
        );
    }
    layout::names(std::slice::from_ref(&plan.retirement.target.root_path))?;
    // Root paths are display-only after selection. Never stat/open an old target.
    Ok(())
}
fn load(root: &Root, expected: &str) -> Result<Plan> {
    request_digest(expected)?;
    let bytes = root
        .host
        .read_operation(
            &format!(".zxs-helper-terminal-retirement-{expected}"),
            "plan.json",
            MAX_RECORD_BYTES,
        )?
        .ok_or("original terminal retirement plan is missing; preserve SYSTEM evidence")?;
    let plan: Plan = json_codec::from_slice(&bytes)?;
    if digest(&plan)? != expected {
        return Err("saved terminal retirement plan digest differs".into());
    }
    check(root, &plan)?;
    Ok(plan)
}
fn response(plan: Plan, original_result: Option<OperationResult>) -> Result<Preview> {
    Ok(Preview {
        request_digest: digest(&plan)?,
        plan,
        original_result,
        physical_bytes_erased: false,
        active_namespace_admitted: false,
        metadata_resumption_supported: false,
        another_terminal_retirement_supported: false,
    })
}
pub(crate) fn preview(input: Input) -> Result<Preview> {
    stopped(input.external_writers_stopped)?;
    let root = Root::open(input.service_user.clone())?;
    let context = root.context.clone();
    drop(root);
    let Output::TerminalPreview { plan } = crate::service_worker::run(WorkerRequest {
        context,
        action: Action::TerminalPreview {
            request: ExitRequest {
                operation_id: input.operation_id,
                slot_id: input.slot_id,
                expected_disk_id: input.expected_disk_id,
                action: ExitAction::Deregister,
            },
        },
    })?
    else {
        return Err("unexpected terminal retirement preview reply".into());
    };
    check(&Root::open(input.service_user)?, &plan)?;
    response(plan, None)
}
pub(crate) fn original(input: Original) -> Result<Preview> {
    stopped(input.external_writers_stopped)?;
    let root = Root::open(input.service_user.clone())?;
    let plan = load(&root, &input.request_digest)?;
    let context = root.context.clone();
    drop(root);
    let Output::TerminalResult { result } = crate::service_worker::run(WorkerRequest {
        context,
        action: Action::TerminalOriginal { plan: plan.clone() },
    })?
    else {
        return Err("unexpected original terminal retirement reply".into());
    };
    check(&Root::open(input.service_user)?, &plan)?;
    response(plan, result)
}
pub(crate) fn commit(input: Commit) -> Result<json_codec::Value> {
    if input.confirmation != "retire-terminal-member-without-metadata-restart"
        || !input.acknowledge_current_view_loss
        || !input.acknowledge_no_metadata_resumption
    {
        return Err("exact terminal retirement confirmation, view-loss and no-metadata-restart acknowledgments are required before system access".into());
    }
    stopped(input.input.external_writers_stopped)?;
    request_digest(&input.input.request_digest)?;
    if digest(&input.plan)? != input.input.request_digest {
        return Err("terminal retirement plan differs from its confirmed digest".into());
    }
    let root = Root::open(input.input.service_user.clone())?;
    check(&root, &input.plan)?;
    let request = WorkerRequest {
        context: root.context.clone(),
        action: Action::TerminalCommit {
            plan: input.plan.clone(),
        },
    };
    if json_codec::to_vec(&request)?.len() > MAX_RECORD_BYTES {
        return Err(
            "terminal retirement exceeds the worker envelope bound; no plan was saved".into(),
        );
    }
    let bytes = json_codec::to_vec(&input.plan)?;
    root.host.publish(
        &format!(
            ".zxs-helper-terminal-retirement-{}",
            input.input.request_digest
        ),
        &[("plan.json", &bytes)],
    )?;
    drop(root);
    finish(input, request).map_err(|source| Box::new(Pending { source }) as _)
}
fn finish(input: Commit, request: WorkerRequest) -> Result<json_codec::Value> {
    let Output::TerminalResult {
        result: Some(result),
    } = crate::service_worker::run(request)?
    else {
        return Err("terminal retirement did not return a native permanent receipt".into());
    };
    check(&Root::open(input.input.service_user)?, &input.plan)?;
    Ok(
        json_codec::json!({"format":"zxs-helper-terminal-retirement-result",
        "request_digest":input.input.request_digest, "result":result,
        "physical_bytes_erased":false,"active_namespace_admitted":false,
        "metadata_resumption_supported":false,"another_terminal_retirement_supported":false,
        "service_started":false,"complete_installation_ready":false}),
    )
}
pub(super) fn plan(
    context: layout::Context,
    credential: CredentialRevision,
    retirement: TerminalRetirementPreview,
) -> Plan {
    Plan {
        format: Format::Retirement,
        context,
        credential,
        retirement,
    }
}
