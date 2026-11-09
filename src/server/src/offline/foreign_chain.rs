//! One exact complete lower-chain material batch. It does not invoke Add,
//! rewrite the source, install an active Config or grant membership authority.
mod worker;
use crate::offline::{
    Result,
    managed_layout::{self as layout, Layout},
    private_io::BundleDestination,
};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::linux::FileIdentity;
use sha2::{Digest, Sha256};
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
    if !(2..=3).contains(&level) || lower.len() != (level - 1) as usize {
        return Err("a level 2 source needs its real level 1; a level 3 source needs real levels 1 and 2 in the same batch".into());
    }
    storage_engine::validate_slot(source_slot)?;
    let mut slots = BTreeSet::from([source_slot.to_ascii_lowercase()]);
    for (index, item) in lower.iter().enumerate() {
        storage_engine::validate_slot(&item.slot_id)?;
        let expected_upstream = index
            .checked_sub(1)
            .map(|previous| lower[previous].slot_id.as_str());
        if item.level != index as u32 + 1
            || item.upstream_slot_id.as_deref() != expected_upstream
            || !slots.insert(item.slot_id.to_ascii_lowercase())
        {
            return Err("list every lower level once in ascending order with its exact preceding upstream slot".into());
        }
    }
    if upstream
        != lower
            .last()
            .expect("invariant: complete lower chain is nonempty")
            .slot_id
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
}
#[derive(Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Output {
    Preview { plan: Plan },
    Report { report: Report },
}
impl crate::offline::service_worker::Request for WorkerRequest {
    type Output = Output;
    const COMMAND: &'static str = "internal-foreign-chain-worker";
    fn account(&self) -> &crate::offline::account::Snapshot {
        &self.context.account
    }
    fn execute(self) -> Result<Output> {
        worker::execute(self)
    }
}
pub(crate) fn worker_entry() -> Result<()> {
    crate::offline::service_worker::entry::<WorkerRequest>()
}

struct Root {
    host: BundleDestination,
    layout: Layout,
    context: layout::Context,
}
impl Root {
    fn open(user: String) -> Result<Self> {
        if rustix::process::geteuid().as_raw() != 0 {
            return Err(
                "foreign-chain material preparation requires the manual root helper".into(),
            );
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
        if self.context != plan.binding.context {
            return Err(
                "original chain host/account/layout changed; no substitute was opened".into(),
            );
        }
        Ok(())
    }
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
fn digest(value: &impl Serialize) -> Result<String> {
    Ok(platform_io::lower_hex(&Sha256::digest(json_codec::to_vec(
        value,
    )?)))
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
    let context = root.context.clone();
    drop(root);
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
    preview_result(plan, None, false)
}
pub(crate) fn original(input: Original) -> Result<json_codec::Value> {
    stopped(input.external_writers_stopped)?;
    let root = Root::open(input.service_user.clone())?;
    let plan = root.load(&input)?;
    let completion = root.completion(&plan)?;
    let context = root.context.clone();
    drop(root);
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
    finish(input, envelope, completion).map_err(|source| Box::new(Pending { source }) as _)
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
#[derive(Debug)]
struct Pending {
    source: Box<dyn std::error::Error + Send + Sync>,
}
impl std::fmt::Display for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "complete-chain material may be partly or fully prepared; retain its original operation/digest and all roots, never clear or replace them; foreign membership was not admitted: {}",
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
#[path = "../../../../test/server/unit/offline/foreign_chain_tests.rs"]
mod tests;
