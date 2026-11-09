//! Root-owned original-chain database intent, not a membership receipt.
use super::*;
use storage_engine::{ForeignDatabaseStatus, ForeignOriginalDatabasesReport};

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
pub(super) struct Expected {
    request_digest: String,
    targets: Vec<ForeignOriginalDatabasesReport>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Observation {
    pub targets: Vec<ForeignOriginalDatabasesReport>,
    pub statuses: Vec<ForeignDatabaseStatus>,
}
impl Observation {
    pub fn completed(&self) -> bool {
        !self.targets.is_empty() && self.targets.len() == self.statuses.len()
            && self.targets.iter().zip(&self.statuses).all(|(expected, status)| {
                matches!(status, ForeignDatabaseStatus::Verified { report } if report == expected)
            })
    }
    pub(super) fn expected(&self, plan: &Plan) -> Result<Expected> {
        Ok(Expected {
            request_digest: digest(plan)?,
            targets: self.targets.clone(),
        })
    }
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Prepare {
    input: Original,
    expected: Expected,
    database_digest: String,
    confirmation: String,
}
fn name(plan: &Plan, complete: bool) -> String {
    format!(
        "{}-databases{}",
        operation_name(plan.binding.operation_id),
        if complete { "-complete" } else { "" }
    )
}
fn saved(root: &Root, plan: &Plan, complete: bool) -> Result<Option<Expected>> {
    let Some(bytes) =
        root.host
            .read_operation(&name(plan, complete), "plan.json", MAX_RECORD_BYTES)?
    else {
        return Ok(None);
    };
    let record: Expected = json_codec::from_slice(&bytes)?;
    if record.request_digest != digest(plan)? || record.targets.len() != plan.binding.lower.len() {
        return Err("foreign database record differs from the original entire lower chain".into());
    }
    if complete && saved(root, plan, false)?.as_ref() != Some(&record) {
        return Err("original database completion lacks its matching immutable intent".into());
    }
    Ok(Some(record))
}
fn root(input: &Original) -> Result<(Root, Plan)> {
    stopped(input.external_writers_stopped)?;
    let root = Root::open(input.service_user.clone())?;
    let plan = root.load(input)?;
    if root.completion(&plan)?.is_none() {
        return Err("every original lower capsule must complete before original database scopes can be prepared".into());
    }
    Ok((root, plan))
}
fn response(
    input: &Original,
    expected: Expected,
    report: Observation,
    complete: bool,
) -> Result<json_codec::Value> {
    let database_digest = digest(&expected)?;
    Ok(
        json_codec::json!({"operation_id":input.operation_id,"request_digest":input.request_digest,
        "database_digest":database_digest,"expected":expected,"observation":report,
        "original_database_scopes_prepared":complete,"target_identity_databases_installed":false,
        "foreign_admitted":false,"changes_existing_pool_membership":false,
        "source_modified":false,"starts_service":false,"complete_installation_ready":false}),
    )
}
pub(super) fn completed_original(input: &Original) -> Result<(Root, Plan, Expected)> {
    let (root, plan) = root(input)?;
    let expected = saved(&root, &plan, true)?
        .ok_or("the entire original database batch must complete before target identity mapping")?;
    Ok((root, plan, expected))
}
pub(crate) fn preview(input: Original) -> Result<json_codec::Value> {
    let (root, plan) = root(&input)?;
    let previous = saved(&root, &plan, false)?;
    let completion = saved(&root, &plan, true)?;
    let envelope = WorkerRequest {
        context: root.context.clone(),
        action: Action::Databases {
            plan: plan.clone(),
            prepare: None,
            completed: completion.is_some(),
        },
    };
    drop(root);
    let Output::Databases { report } = crate::offline::service_worker::run(envelope)? else {
        return Err("unexpected foreign database preview response".into());
    };
    let expected = report.expected(&plan)?;
    if previous.as_ref().is_some_and(|old| old != &expected)
        || completion
            .as_ref()
            .is_some_and(|old| old != &expected || !report.completed())
    {
        return Err(
            "original foreign database intent/completion differs; no material was repaired".into(),
        );
    }
    Root::open(input.service_user.clone())?.check(&plan)?;
    response(&input, expected, report, completion.is_some())
}
pub(crate) fn prepare(input: Prepare) -> Result<json_codec::Value> {
    if input.confirmation != "prepare-original-three-scope-databases-for-entire-chain" {
        return Err("explicit prepare-original-three-scope-databases-for-entire-chain confirmation is required before system access".into());
    }
    if digest(&input.expected)? != input.database_digest
        || input.expected.request_digest != input.input.request_digest
    {
        return Err(
            "foreign database confirmation differs from the reviewed whole-chain digest".into(),
        );
    }
    let (root, plan) = root(&input.input)?;
    let completion = saved(&root, &plan, true)?;
    if completion
        .as_ref()
        .is_some_and(|old| old != &input.expected)
    {
        return Err("completed original database batch cannot be replaced".into());
    }
    let envelope = WorkerRequest {
        context: root.context.clone(),
        action: Action::Databases {
            plan: plan.clone(),
            prepare: Some(input.expected.clone()),
            completed: completion.is_some(),
        },
    };
    if json_codec::to_vec(&envelope)?.len() > MAX_RECORD_BYTES {
        return Err("foreign database IPC envelope exceeds 64 KiB; nothing was reserved".into());
    }
    root.host.publish(
        &name(&plan, false),
        &[("plan.json", &json_codec::to_vec(&input.expected)?)],
    )?;
    drop(root);
    finish(input, plan, envelope).map_err(|source| Box::new(Pending { source }) as _)
}
fn finish(input: Prepare, plan: Plan, envelope: WorkerRequest) -> Result<json_codec::Value> {
    let Output::Databases { report } = crate::offline::service_worker::run(envelope)? else {
        return Err("unexpected foreign database preparation response".into());
    };
    if !report.completed() || report.expected(&plan)? != input.expected {
        return Err(
            "not every original lower target has independently verified original database scopes"
                .into(),
        );
    }
    let root = Root::open(input.input.service_user.clone())?;
    root.check(&plan)?;
    root.host.publish(
        &name(&plan, true),
        &[("plan.json", &json_codec::to_vec(&input.expected)?)],
    )?;
    response(&input.input, input.expected, report, true)
}
pub(super) fn match_expected(
    plan: &Plan,
    observed: &Observation,
    expected: &Expected,
) -> Result<()> {
    if observed.expected(plan)? != *expected {
        return Err("foreign original databases changed after their whole-chain review".into());
    }
    Ok(())
}
#[cfg(test)]
#[path = "../../../../../test/server/unit/offline/foreign_chain_databases_tests.rs"]
mod tests;
