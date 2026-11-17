//! Root-owned whole-chain identity intent. The map is independently prepared
//! by each lower-root owner, never substituted by its healthy peer or this DTO.
use super::*;
use storage_engine::ForeignIdentityClosureReport;

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serialization(rename_all = "snake_case")]
pub(super) enum Progress {
    NotStarted,
    Preparing,
    Verified,
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Observation {
    pub expected: ForeignIdentityClosureReport,
    pub statuses: Vec<Progress>,
}
impl Observation {
    pub(super) fn require_plan(
        &self,
        plan: &Plan,
        databases: &databases::Expected,
    ) -> Result<()> {
        require_expected(plan, databases, &self.expected)?;
        if self.statuses.len() != plan.binding.lower.len() {
            return Err("foreign identity observation omits an original lower target".into());
        }
        Ok(())
    }

    pub(super) fn completed(&self, plan: &Plan) -> bool {
        self.statuses.len() == plan.binding.lower.len()
            && !self.statuses.is_empty()
            && self
                .statuses
                .iter()
                .all(|status| *status == Progress::Verified)
    }
}

pub(super) fn require_expected(
    plan: &Plan,
    databases: &databases::Expected,
    report: &ForeignIdentityClosureReport,
) -> Result<()> {
    if report.baseline.request_digest != digest(plan)?
        || report.baseline.operation_id != plan.binding.operation_id
        || report.baseline.chain_digest != digest(&plan.binding)?
    {
        return Err(
            "foreign object identity intent is not the original complete-chain operation".into(),
        );
    }
    databases.require_original(plan, &report.original)
}
pub(super) fn match_expected(
    plan: &Plan,
    databases: &databases::Expected,
    observed: &Observation,
    expected: &ForeignIdentityClosureReport,
) -> Result<()> {
    observed.require_plan(plan, databases)?;
    if &observed.expected != expected {
        return Err("foreign identity preview changed; no root intent was saved".into());
    }
    Ok(())
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Prepare {
    input: Original,
    expected: ForeignIdentityClosureReport,
    mapping_digest: String,
    confirmation: String,
}
fn name(plan: &Plan, complete: bool) -> String {
    format!(
        "{}-identities{}",
        operation_name(plan.binding.operation_id),
        if complete { "-complete" } else { "" }
    )
}
fn saved(
    root: &Root,
    plan: &Plan,
    databases: &databases::Expected,
    complete: bool,
) -> Result<Option<ForeignIdentityClosureReport>> {
    let Some(bytes) =
        root.host
            .read_operation(&name(plan, complete), "plan.json", MAX_RECORD_BYTES)?
    else {
        return Ok(None);
    };
    let report: ForeignIdentityClosureReport = json_codec::from_slice(&bytes)?;
    require_expected(plan, databases, &report)?;
    if complete && saved(root, plan, databases, false)?.as_ref() != Some(&report) {
        return Err("foreign object identity completion lacks its exact immutable intent".into());
    }
    Ok(Some(report))
}
fn response(
    input: &Original,
    plan: &Plan,
    report: Observation,
    complete: bool,
) -> Result<json_codec::Value> {
    if complete && !report.completed(plan) {
        return Err(
            "completed foreign identity batch lost one independently verified target".into(),
        );
    }
    Ok(json_codec::json!({
        "operation_id":input.operation_id,"request_digest":input.request_digest,
        "mapping_digest":report.expected.mapping_digest,"expected":report.expected,"statuses":report.statuses,
        "object_identity_material_prepared":complete,
        "identity_mapping_only":true,"source_authentication_imported":false,
        "joint_state_installed":false,"target_identity_databases_installed":false,
        "foreign_admitted":false,"changes_existing_pool_membership":false,
        "source_modified":false,"starts_service":false,"complete_installation_ready":false
    }))
}
pub(crate) fn preview(input: Original) -> Result<json_codec::Value> {
    let (root, plan, databases) = databases::completed_original(&input)?;
    let previous = saved(&root, &plan, &databases, false)?;
    let completion = saved(&root, &plan, &databases, true)?;
    let envelope = WorkerRequest {
        context: root.context.clone(),
        action: Action::Objects {
            plan: Box::new(plan.clone()),
            databases: databases.clone(),
            prepare: None,
            completed: completion.is_some(),
        },
    };
    drop(root);
    let Output::Objects { report } = crate::offline::service_worker::run(envelope)? else {
        return Err("unexpected foreign object identity preview response".into());
    };
    report.require_plan(&plan, &databases)?;
    if previous
        .as_ref()
        .is_some_and(|saved| saved != &report.expected)
        || completion
            .as_ref()
            .is_some_and(|saved| saved != &report.expected || !report.completed(&plan))
    {
        return Err("original object identity material cannot be rebound to a different current State or foreign source".into());
    }
    Root::open(input.service_user.clone())?.check(&plan)?;
    response(&input, &plan, report, completion.is_some())
}
pub(crate) fn prepare(input: Prepare) -> Result<json_codec::Value> {
    if input.confirmation != "prepare-complete-chain-object-identities-without-admission" {
        return Err("explicit prepare-complete-chain-object-identities-without-admission confirmation is required before system access".into());
    }
    if input.mapping_digest != input.expected.mapping_digest
        || input.expected.baseline.request_digest != input.input.request_digest
        || input.expected.baseline.operation_id != input.input.operation_id
    {
        return Err(
            "object identity confirmation differs from the reviewed original operation".into(),
        );
    }
    let (root, plan, databases) = databases::completed_original(&input.input)?;
    require_expected(&plan, &databases, &input.expected)?;
    if saved(&root, &plan, &databases, false)?
        .as_ref()
        .is_some_and(|saved| saved != &input.expected)
    {
        return Err("original foreign identity intent cannot be replaced".into());
    }
    let completion = saved(&root, &plan, &databases, true)?;
    if completion
        .as_ref()
        .is_some_and(|saved| saved != &input.expected)
    {
        return Err("completed foreign identity batch cannot be replaced".into());
    }
    let (root, plan, databases) = if completion.is_none() {
        preflight(root, &plan, &databases, &input.expected)?;
        databases::completed_original(&input.input)?
    } else {
        // Existing exact completion already admitted this intent. Physical
        // readback still happens in the write worker, without a duplicate scan.
        (root, plan, databases)
    };
    require_expected(&plan, &databases, &input.expected)?;
    let completion = saved(&root, &plan, &databases, true)?;
    if completion.as_ref().is_some_and(|saved| saved != &input.expected) {
        return Err("completed foreign identity batch cannot be replaced".into());
    }
    let envelope = WorkerRequest {
        context: root.context.clone(),
        action: Action::Objects {
            plan: Box::new(plan.clone()),
            databases,
            prepare: Some(input.expected.clone()),
            completed: completion.is_some(),
        },
    };
    if json_codec::to_vec(&envelope)?.len() > MAX_RECORD_BYTES {
        return Err("foreign object identity IPC exceeds 64 KiB; nothing was reserved".into());
    }
    root.host.publish(
        &name(&plan, false),
        &[("plan.json", &json_codec::to_vec(&input.expected)?)],
    )?;
    drop(root);
    finish(input, plan, envelope).map_err(coordinator::pending::<super::Plan>)
}
fn preflight(
    root: Root,
    plan: &Plan,
    databases: &databases::Expected,
    expected: &ForeignIdentityClosureReport,
) -> Result<()> {
    // No immutable intent is written until the worker recomputes the complete
    // current-State and source mapping. Release root's lease before it runs.
    let envelope = WorkerRequest {
        context: root.into_context(),
        action: Action::Objects {
            plan: Box::new(plan.clone()),
            databases: databases.clone(),
            prepare: None,
            completed: false,
        },
    };
    let Output::Objects { report } = crate::offline::service_worker::run(envelope)? else {
        return Err("unexpected foreign identity preflight response".into());
    };
    match_expected(plan, databases, &report, expected)
}
fn finish(input: Prepare, plan: Plan, envelope: WorkerRequest) -> Result<json_codec::Value> {
    let Output::Objects { report } = crate::offline::service_worker::run(envelope)? else {
        return Err("unexpected foreign object identity preparation response".into());
    };
    if report.expected != input.expected || !report.completed(&plan) {
        return Err(
            "not every lower root independently completed the same original identity mapping"
                .into(),
        );
    }
    let root = Root::open(input.input.service_user.clone())?;
    root.check(&plan)?;
    root.host.publish(
        &name(&plan, true),
        &[("plan.json", &json_codec::to_vec(&input.expected)?)],
    )?;
    response(&input.input, &plan, report, true)
}
