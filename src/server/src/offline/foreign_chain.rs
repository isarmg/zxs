//! One exact complete lower-chain material batch. It does not invoke Add,
//! rewrite the source, install an active Config or grant membership authority.
use crate::offline::coordinator::{self, digest};
pub(crate) mod databases;
pub(crate) mod identity;
pub(crate) mod objects;
mod worker;
use crate::offline::{Result, managed_layout as layout};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::linux::FileIdentity;
use std::{collections::BTreeSet, path::Path};
use storage_engine::DiskRegistrationIdentity;
use storage_engine::{OperationId, storage_directories::CandidateSelection};
use storage_server::auth::CredentialRevision;
const MAX_RECORD_BYTES: usize = 64 * 1024;
const CAPSULE: &str = "foreign-chain-material";

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct LowerDirectory {
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
    source_root: String,
    source_slot_id: String,
    source_upstream_slot_id: String,
    source_metadata_member: bool,
    lower: Vec<LowerDirectory>,
    external_writers_stopped: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Original {
    service_user: String,
    operation_id: OperationId,
    request_digest: String,
    external_writers_stopped: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Prepare {
    input: Original,
    plan: Plan,
    confirmation: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Physical {
    device: String,
    inode: String,
}
impl From<FileIdentity> for Physical {
    fn from(value: FileIdentity) -> Self {
        Self {
            device: value.device.to_string(),
            inode: value.inode.to_string(),
        }
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Source {
    root_path: String,
    physical: Physical,
    registration: DiskRegistrationIdentity,
    level: u32,
    evidence_digest: String,
    histories: usize,
    temporarily_unavailable_versions: usize,
}
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct SelectedLower {
    directory: LowerDirectory,
    selection: CandidateSelection,
}
#[derive(Clone, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "zxs-helper-foreign-complete-chain-material")]
    ChainMaterial,
}
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Binding {
    format: Format,
    context: layout::Context,
    credential: CredentialRevision,
    configuration_digest: String,
    registered_roots: Vec<String>,
    operation_id: OperationId,
    source: Source,
    source_slot_id: String,
    source_upstream_slot_id: String,
    source_metadata_member: bool,
    lower: Vec<SelectedLower>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Plan {
    binding: Binding,
    capsule_digests: Vec<String>,
}
impl Plan {
    fn roots(&self) -> Vec<String> {
        self.binding
            .registered_roots
            .iter()
            .cloned()
            .chain(std::iter::once(self.binding.source.root_path.clone()))
            .chain(
                self.binding
                    .lower
                    .iter()
                    .map(|item| item.directory.root_path.clone()),
            )
            .collect()
    }
    fn validate(&self) -> Result<()> {
        let lower = self
            .binding
            .lower
            .iter()
            .map(|item| &item.directory)
            .collect::<Vec<_>>();
        complete_chain(
            self.binding.source.level,
            &self.binding.source_slot_id,
            &self.binding.source_upstream_slot_id,
            &lower,
        )?;
        layout::names(&self.roots())?;
        if self.capsule_digests.len() != lower.len() {
            return Err("every required lower root must have one exact native capsule plan".into());
        }
        for (item, digest) in self.binding.lower.iter().zip(&self.capsule_digests) {
            request_digest(digest)?;
            if item.directory.root_path != item.selection.root_path
                || item.selection.discovery_root != layout::DISKS
            {
                return Err("lower root selection differs from the exact complete chain".into());
            }
        }
        request_digest(&self.binding.source.evidence_digest)?;
        request_digest(&self.binding.configuration_digest)
    }
}

/// A complete chain is parsed once at the real request/persisted-plan boundary.
/// Ordered levels are explicit, unique and contiguous; no placeholder fallback.
fn complete_chain(
    level: u32,
    source_slot: &str,
    upstream: &str,
    lower: &[&LowerDirectory],
) -> Result<()> {
    if !(2..=storage_engine::ForeignMigrationPlan::MAX_CHAIN_LEVEL).contains(&level)
        || lower.len() != (level - 1) as usize
    {
        return Err("a foreign source must include every real lower level in the same bounded chain; missing levels cannot be added later".into());
    }
    let source_slot = name_policy::SlotId::parse(source_slot)?;
    let mut slots = BTreeSet::from([source_slot.as_str().to_ascii_lowercase()]);
    let mut previous: Option<name_policy::SlotId> = None;
    for (index, item) in lower.iter().enumerate() {
        let slot = name_policy::SlotId::parse(&item.slot_id)?;
        let expected_upstream = previous.as_ref().map(name_policy::SlotId::as_str);
        if item.level != index as u32 + 1
            || item.upstream_slot_id.as_deref() != expected_upstream
            || !slots.insert(slot.as_str().to_ascii_lowercase())
        {
            return Err("list every lower level once in ascending order with its exact preceding upstream slot".into());
        }
        previous = Some(slot);
    }
    if upstream
        != previous
            .as_ref()
            .expect("invariant: complete lower chain is nonempty")
            .as_str()
    {
        return Err("foreign source must explicitly target the highest selected lower slot".into());
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
enum Progress {
    Unstarted,
    Reserved,
    PublishedCapsule,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct TargetProgress {
    level: u32,
    path: String,
    capsule_digest: String,
    progress: Progress,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Report {
    targets: Vec<TargetProgress>,
}
impl Report {
    fn all_published(&self) -> bool {
        !self.targets.is_empty()
            && self
                .targets
                .iter()
                .all(|item| item.progress == Progress::PublishedCapsule)
    }
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Completion {
    request_digest: String,
    report: Report,
}
type WorkerRequest = crate::offline::service_worker::Envelope<Action>;
#[derive(Serialize, Deserialize)]
#[serialization(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Preview {
        operation_id: OperationId,
        source_root: String,
        source_slot_id: String,
        source_upstream_slot_id: String,
        source_metadata_member: bool,
        lower: Vec<LowerDirectory>,
    },
    Status {
        plan: Plan,
    },
    Prepare {
        plan: Plan,
        completed: bool,
    },
    Databases {
        plan: Plan,
        prepare: Option<databases::Expected>,
        completed: bool,
    },
    Identity {
        plan: Plan,
        expected: databases::Expected,
    },
    Objects {
        plan: Box<Plan>,
        databases: databases::Expected,
        prepare: Option<storage_engine::ForeignIdentityClosureReport>,
        completed: bool,
    },
}
#[derive(Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Output {
    Preview { plan: Box<Plan> },
    Report { report: Report },
    Databases { report: databases::Observation },
    Identity { proposal: identity::Proposal },
    Objects { report: objects::Observation },
}
impl crate::offline::service_worker::Action for Action {
    type Output = Output;
    const COMMAND: &'static str = "internal-foreign-chain-worker";
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
        "foreign-chain material preparation requires the manual root helper";
    const CONTEXT_CHANGED: &'static str =
        "original chain host/account/layout changed; no substitute was opened";
    const PENDING: &'static str = "complete-chain material may be partly or fully prepared; retain its original operation/digest and all roots, never clear or replace them; foreign membership was not admitted";
    fn context(&self) -> &layout::Context {
        &self.binding.context
    }
}
impl Root {
    fn load(&self, input: &Original) -> Result<Plan> {
        request_digest(&input.request_digest)?;
        let bytes = self
            .host
            .read_operation(
                &operation_name(input.operation_id),
                "plan.json",
                MAX_RECORD_BYTES,
            )?
            .ok_or("the original complete-chain plan is missing; preserve all material")?;
        let plan: Plan = json_codec::from_slice(&bytes)?;
        plan.validate()?;
        if plan.binding.operation_id != input.operation_id || digest(&plan)? != input.request_digest
        {
            return Err("original complete-chain operation/digest differs".into());
        }
        self.check(&plan)?;
        Ok(plan)
    }
    fn completion(&self, plan: &Plan) -> Result<Option<Completion>> {
        let Some(bytes) = self.host.read_operation(
            &format!("{}-complete", operation_name(plan.binding.operation_id)),
            "receipt.json",
            MAX_RECORD_BYTES,
        )?
        else {
            return Ok(None);
        };
        let completion: Completion = json_codec::from_slice(&bytes)?;
        if completion.request_digest != digest(plan)? || !completion.report.all_published() {
            return Err("original chain completion record differs; no repair was attempted".into());
        }
        Ok(Some(completion))
    }
}

fn operation_name(operation: OperationId) -> String {
    format!(".zxs-helper-foreign-chain-{operation}")
}
fn stopped(value: bool) -> Result<()> {
    if !value {
        return Err("stop ZXS, Samba, older binaries and all other source/target writers before access; acknowledgment is not proof of exclusion".into());
    }
    Ok(())
}
fn request_digest(value: &str) -> Result<()> {
    value.parse::<storage_engine::ForeignEvidenceDigest>()?;
    Ok(())
}
fn preview_result(
    plan: Plan,
    report: Option<Report>,
    completed: bool,
) -> Result<json_codec::Value> {
    let request_digest = digest(&plan)?;
    Ok(
        json_codec::json!({"plan":plan,"request_digest":request_digest,
        "original_status":report,"batch_material_prepared":completed,
        "changes_existing_pool_membership":false,"foreign_admitted":false,
        "source_modified":false,"formats_or_mounts_devices":false,
        "starts_service":false,"complete_installation_ready":false}),
    )
}
pub(crate) fn preview(input: Input) -> Result<json_codec::Value> {
    stopped(input.external_writers_stopped)?;
    layout::names(
        &std::iter::once(input.source_root.clone())
            .chain(input.lower.iter().map(|item| item.root_path.clone()))
            .collect::<Vec<_>>(),
    )?;
    let root = Root::open(input.service_user.clone())?;
    let context = root.into_context();
    let Output::Preview { plan } = crate::offline::service_worker::run(WorkerRequest {
        context,
        action: Action::Preview {
            operation_id: input.operation_id,
            source_root: input.source_root,
            source_slot_id: input.source_slot_id,
            source_upstream_slot_id: input.source_upstream_slot_id,
            source_metadata_member: input.source_metadata_member,
            lower: input.lower,
        },
    })?
    else {
        return Err("unexpected foreign-chain preview reply".into());
    };
    plan.validate()?;
    Root::open(input.service_user)?.check(&plan)?;
    preview_result(*plan, None, false)
}
pub(crate) fn original(input: Original) -> Result<json_codec::Value> {
    stopped(input.external_writers_stopped)?;
    let root = Root::open(input.service_user.clone())?;
    let plan = root.load(&input)?;
    let completion = root.completion(&plan)?;
    let context = root.into_context();
    let Output::Report { report } = crate::offline::service_worker::run(WorkerRequest {
        context,
        action: Action::Status { plan: plan.clone() },
    })?
    else {
        return Err("unexpected foreign-chain status reply".into());
    };
    if let Some(completion) = &completion
        && completion.report != report
    {
        return Err("completed chain no longer has all original verified capsules; no missing bytes were recreated".into());
    }
    Root::open(input.service_user)?.check(&plan)?;
    preview_result(plan, Some(report), completion.is_some())
}
pub(crate) fn prepare(input: Prepare) -> Result<json_codec::Value> {
    if input.confirmation != "prepare-entire-foreign-lower-chain-material" {
        return Err("explicit prepare-entire-foreign-lower-chain-material confirmation is required before system access".into());
    }
    stopped(input.input.external_writers_stopped)?;
    input.plan.validate()?;
    if digest(&input.plan)? != input.input.request_digest
        || input.plan.binding.operation_id != input.input.operation_id
    {
        return Err("foreign-chain preparation differs from its exact reviewed request".into());
    }
    let root = Root::open(input.input.service_user.clone())?;
    root.check(&input.plan)?;
    let completion = root.completion(&input.plan)?;
    let envelope = WorkerRequest {
        context: root.context.clone(),
        action: Action::Prepare {
            plan: input.plan.clone(),
            completed: completion.is_some(),
        },
    };
    if json_codec::to_vec(&envelope)?.len() > MAX_RECORD_BYTES {
        return Err("complete-chain execution exceeds the IPC budget; no intent was saved".into());
    }
    let bytes = json_codec::to_vec(&input.plan)?;
    root.host.publish(
        &operation_name(input.input.operation_id),
        &[("plan.json", &bytes)],
    )?;
    drop(root);
    finish(input, envelope, completion).map_err(coordinator::pending::<Plan>)
}
fn finish(
    input: Prepare,
    envelope: WorkerRequest,
    completion: Option<Completion>,
) -> Result<json_codec::Value> {
    let Output::Report { report } = crate::offline::service_worker::run(envelope)? else {
        return Err("unexpected foreign-chain prepare reply".into());
    };
    if !report.all_published() || completion.as_ref().is_some_and(|old| old.report != report) {
        return Err("complete-chain result does not contain every original capsule".into());
    }
    let root = Root::open(input.input.service_user)?;
    root.check(&input.plan)?;
    let completion = Completion {
        request_digest: input.input.request_digest,
        report: report.clone(),
    };
    root.host.publish(
        &format!("{}-complete", operation_name(input.input.operation_id)),
        &[("receipt.json", &json_codec::to_vec(&completion)?)],
    )?;
    preview_result(input.plan, Some(report), true)
}
#[cfg(test)]
#[path = "../../../../test/server/unit/offline/foreign_chain_tests.rs"]
mod tests;
