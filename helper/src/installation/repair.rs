//! Restore only the exact recorded embedded payload. This does not authorize
//! a version change, execute the program, or open runtime data/configuration.
use super::{
    baseline::{self, PayloadIdentity},
    payload,
};
use crate::{
    Result,
    private_io::{BundleDestination, ProgramDestination, ProgramSnapshot},
};
use data_serialization::{Deserialize, Serialize};
use platform_io::service_lease::HOST_ROOT;
use sha2::{Digest, Sha256};
use std::path::Path;

const MAX_RECORD: usize = 16 * 1024;

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "zxs-helper-same-payload-program-repair")]
    Current,
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Plan {
    format: Format,
    revision: u32,
    host_device: String,
    host_inode: String,
    parent_device: String,
    parent_inode: String,
    payload: PayloadIdentity,
    baseline_sha256: String,
    original: Option<ProgramSnapshot>,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Original {
    pub request_digest: String,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Commit {
    pub request_digest: String,
    pub confirmation: String,
    pub external_administration_stopped: bool,
}

#[derive(Serialize)]
pub(crate) struct Preview {
    plan: Plan,
    request_digest: String,
    original_request_recovered: bool,
    program_publication_observed: bool,
    backup_directory: String,
    original_inode_retained_at: String,
    data_or_configuration_modified: bool,
    service_started: bool,
    upgrade_or_downgrade_supported: bool,
}

#[derive(Serialize)]
struct Receipt<'a> {
    format: &'static str,
    request_digest: &'a str,
    installed: ProgramSnapshot,
    original_file_existed: bool,
    original_inode_and_independent_backup_retained: bool,
    data_or_configuration_modified: bool,
    service_started: bool,
    complete_installation_ready: bool,
}

struct Context {
    host: BundleDestination,
    program: ProgramDestination,
    payload: payload::Payload,
    baseline: Vec<u8>,
}
impl Context {
    fn open() -> Result<Self> {
        super::require_root()?;
        let payload = payload::required()?;
        let host = BundleDestination::open(Path::new(HOST_ROOT))?;
        let program = ProgramDestination::open(Path::new(super::DESTINATION))?;
        let baseline = baseline::expected(&host, &program, payload)?;
        baseline::require(&host, &baseline)?;
        Ok(Self {
            host,
            program,
            payload,
            baseline,
        })
    }
    fn recheck(&self) -> Result<()> {
        self.host.recheck()?;
        self.program.repair_recheck()?;
        baseline::require(&self.host, &self.baseline)
    }
    fn plan(&self, original: Option<ProgramSnapshot>) -> Plan {
        let host = self.host.identity();
        let parent = self.program.parent_identity();
        Plan {
            format: Format::Current,
            revision: 1,
            host_device: host.device.to_string(),
            host_inode: host.inode.to_string(),
            parent_device: parent.device.to_string(),
            parent_inode: parent.inode.to_string(),
            payload: PayloadIdentity::from_payload(self.payload),
            baseline_sha256: platform_io::lower_hex(&Sha256::digest(&self.baseline)),
            original,
        }
    }
    fn fresh(&self) -> Result<Plan> {
        Ok(self.plan(self.program.repair_snapshot(self.payload.bytes)?))
    }
    fn original(&self, digest: &str) -> Result<Option<Plan>> {
        let intent = intent_name(digest);
        let Some(bytes) = self.host.read_operation(&intent, "plan.json", MAX_RECORD)? else {
            if self.host.operation_started(&intent)? {
                return Err("repair intent is incomplete; preserve it rather than inventing an original source".into());
            }
            return Ok(None);
        };
        let plan: Plan = json_codec::from_slice(&bytes)?;
        let expected = self.plan(plan.original.clone());
        if json_codec::to_vec_pretty(&expected)? != bytes || plan_digest(&plan)? != digest {
            return Err("repair intent differs from the current exact payload, host, baseline or original digest".into());
        }
        let _ = self
            .program
            .repair_is_published(digest, &plan.original, self.payload.bytes)?;
        Ok(Some(plan))
    }
}

fn digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("repair request digest must be 64 lowercase hexadecimal characters".into());
    }
    Ok(())
}
fn plan_digest(plan: &Plan) -> Result<String> {
    Ok(platform_io::lower_hex(&Sha256::digest(json_codec::to_vec(
        plan,
    )?)))
}
fn intent_name(digest: &str) -> String {
    format!(".zxs-helper-program-repair-{digest}")
}
fn backup_name(digest: &str) -> String {
    format!(".zxs-helper-program-original-{digest}")
}
fn completion_name(digest: &str) -> String {
    format!(".zxs-helper-program-repaired-{digest}")
}

fn report(context: &Context, plan: Plan, recovered: bool) -> Result<Preview> {
    let request_digest = plan_digest(&plan)?;
    Ok(Preview {
        program_publication_observed: context.program.repair_is_published(
            &request_digest,
            &plan.original,
            context.payload.bytes,
        )?,
        backup_directory: format!("{HOST_ROOT}/{}", backup_name(&request_digest)),
        original_inode_retained_at: format!(
            "/usr/local/bin/.zxs-helper-repair-{request_digest}.program"
        ),
        plan,
        request_digest,
        original_request_recovered: recovered,
        data_or_configuration_modified: false,
        service_started: false,
        upgrade_or_downgrade_supported: false,
    })
}
pub(crate) fn preview() -> Result<Preview> {
    let context = Context::open()?;
    report(&context, context.fresh()?, false)
}
pub(crate) fn recover_preview(input: Original) -> Result<Preview> {
    digest(&input.request_digest)?;
    let context = Context::open()?;
    let plan = context
        .original(&input.request_digest)?
        .ok_or("there is no original repair intent for this digest")?;
    report(&context, plan, true)
}

pub(crate) fn commit(input: Commit) -> Result<json_codec::Value> {
    if input.confirmation != "repair-same-payload-program-only"
        || !input.external_administration_stopped
    {
        return Err("explicit repair-same-payload-program-only confirmation and stopped external service/host administration are required before reading or writing program paths; the acknowledgment is not an exclusion proof".into());
    }
    digest(&input.request_digest)?;
    let context = Context::open()?;
    let plan = match context.original(&input.request_digest)? {
        Some(plan) => plan,
        None => context.fresh()?,
    };
    if plan_digest(&plan)? != input.request_digest {
        return Err("program, payload, baseline or parent differs from the exact repair preview; nothing was replaced".into());
    }
    publish(&context, &plan, &input.request_digest).map_err(|source| {
        Box::new(Pending {
            digest: input.request_digest,
            source,
        }) as Box<dyn std::error::Error + Send + Sync>
    })
}

fn receipt<'a>(context: &Context, plan: &Plan, digest: &'a str) -> Result<Receipt<'a>> {
    Ok(Receipt {
        format: "zxs-helper-same-payload-program-repaired",
        request_digest: digest,
        installed: context.program.completed_repair_snapshot(
            digest,
            &plan.original,
            context.payload.bytes,
        )?,
        original_file_existed: plan.original.is_some(),
        original_inode_and_independent_backup_retained: plan.original.is_some(),
        data_or_configuration_modified: false,
        service_started: false,
        complete_installation_ready: false,
    })
}

fn publish(context: &Context, plan: &Plan, digest: &str) -> Result<json_codec::Value> {
    context.recheck()?;
    let completion = completion_name(digest);
    if let Some(bytes) = context
        .host
        .read_operation(&completion, "receipt.json", MAX_RECORD)?
    {
        if bytes != json_codec::to_vec_pretty(&receipt(context, plan, digest)?)? {
            return Err("completed repair receipt no longer matches its exact executable; it was not recreated".into());
        }
    } else if context.host.operation_started(&completion)? {
        return Err("repair completion record is incomplete; preserve it and both program names for explicit recovery".into());
    }
    let original =
        context
            .program
            .repair_original_bytes(digest, &plan.original, context.payload.bytes)?;
    let plan_bytes = json_codec::to_vec_pretty(plan)?;
    context
        .host
        .publish(&intent_name(digest), &[("plan.json", &plan_bytes)])?;
    if let Some(bytes) = original {
        context
            .host
            .publish(&backup_name(digest), &[("original.bin", &bytes)])?;
    } else if context.host.operation_started(&backup_name(digest))? {
        return Err("missing-program repair unexpectedly has an original backup; preserve conflicting evidence".into());
    }
    context.recheck()?;
    context
        .program
        .repair_install(digest, &plan.original, context.payload.bytes, || {
            context.recheck()
        })?;
    let record = receipt(context, plan, digest)?;
    let bytes = json_codec::to_vec_pretty(&record)?;
    context
        .host
        .publish(&completion, &[("receipt.json", &bytes)])?;
    context.recheck()?;
    if json_codec::to_vec_pretty(&receipt(context, plan, digest)?)? != bytes {
        return Err(
            "program changed after the repair receipt; no current readiness is claimed".into(),
        );
    }
    Ok(
        json_codec::json!({"state":"same_payload_program_repaired_not_started",
        "receipt":record,"request_digest":digest,
        "backup_directory":format!("{HOST_ROOT}/{}",backup_name(digest)),
        "service_started":false,"data_or_configuration_modified":false,
        "all_external_writers_excluded":false,"upgrade_or_downgrade_supported":false}),
    )
}

#[derive(Debug)]
struct Pending {
    digest: String,
    source: Box<dyn std::error::Error + Send + Sync>,
}
impl std::fmt::Display for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "program repair {} is incomplete or uncertain; preserve the intent, independent backup and both executable names; resume this exact request only, no rollback or service start is claimed",
            self.digest
        )
    }
}
impl std::error::Error for Pending {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

#[cfg(test)]
#[path = "../../../test/helper/unit/installation/repair_tests.rs"]
mod tests;
