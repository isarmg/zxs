//! Local, read-only entry for two fixed existing configuration names. This
//! source slice has no write, service mutation, package action or Web route.
mod stopped;

use super::{Input, context::Context, render};
use crate::{
    Result, account::LocalAccount, print,
    private_io::{BundleDestination, configuration_files::replacement::{Original, Target}},
};
use data_serialization::Serialize;
use platform_io::service_lease::{HOST_ROOT, ServiceLease};
use sha2::{Digest, Sha256};
use std::path::Path;
use storage_engine::OperationId;

const ACTIVE: &str = ".zxs-helper-config-replacement-active";
const PLAN_MAXIMUM: usize = 64 * 1024;

fn require_root() -> Result<()> {
    if rustix::process::getuid().as_raw() != 0 || rustix::process::geteuid().as_raw() != 0 {
        return Err("configuration replacement preview requires local real and effective root".into());
    }
    Ok(())
}

/// This deliberately bypasses the ordinary first-install bootstrap that may
/// create /zxs. A missing original lease is not a read-only preview opportunity.
pub(crate) fn enter_existing() -> Result<ServiceLease> {
    require_root()?;
    Ok(ServiceLease::host()?.enter_host()?)
}

#[derive(Serialize)]
struct NewFile<'a> {
    destination: &'static str,
    mode: &'static str,
    bytes: u32,
    sha256: String,
    // This is generated here, never read from the old file or a submitted body.
    content: &'a str,
}
fn generated(target: Target, file: &render::ConfigurationFile) -> Result<NewFile<'_>> {
    if file.content.len() > PLAN_MAXIMUM { return Err("generated configuration exceeds file budget".into()); }
    Ok(NewFile {
        destination: target.path(), mode: "0644", bytes: u32::try_from(file.content.len())?,
        sha256: platform_io::lower_hex(&Sha256::digest(file.content.as_bytes())), content: &file.content,
    })
}
#[derive(Serialize)]
struct Pair<'a> { original: &'a Original, generated: NewFile<'a> }

pub(crate) fn preview(input: Input) -> Result<()> {
    require_root()?;
    let host = BundleDestination::open(Path::new(HOST_ROOT))?;
    if host.operation_started(ACTIVE)? {
        return Err("a configuration replacement has existing or incomplete active evidence; do not create another plan".into());
    }
    let (context, unit, samba) = Context::open(&input)?;
    let stopped = stopped::Stopped::capture(samba.is_some())?;
    let originals = context.configuration.hold_existing_for_replacement()?;
    let (old_unit, old_samba) = originals.snapshots();
    let target_unit = Pair { original: old_unit, generated: generated(Target::SystemdService, &unit)? };
    let target_samba = match (old_samba, &samba) {
        (Some(original), Some(file)) => Some(Pair {
            original, generated: generated(Target::SambaConfiguration, file)?,
        }),
        (None, None) => None,
        _ => return Err("original and generated fixed configuration targets differ".into()),
    };
    let operation = OperationId::generate()?;
    let plan = json_codec::json!({
        "format":"zxs-local-current-configuration-replacement",
        "operation":operation, "request":input,
        "host":{"device":host.identity().device.to_string(),"inode":host.identity().inode.to_string()},
        "configuration_parents":context.configuration.parents(), "payload":context.program.metadata(),
        "service_account":context.account.snapshot(), "service_group":context.group.snapshot(),
        "samba_account":context.samba_account.as_ref().map(LocalAccount::snapshot),
        "stop_observation":stopped.plan(), "systemd":target_unit, "samba":target_samba,
        "original_retention":"same inode in a root-private directory on its original filesystem; not a separate backup",
        "atomicity":"one exchange per target; a two-target operation may be partially applied",
        "external_administration":"must remain stopped; observations and advisory locks do not exclude other root writers",
    });
    let encoded = storage_engine::canonical_json_value(&plan)?;
    if encoded.len() > PLAN_MAXIMUM { return Err("replacement preview exceeds plan byte budget".into()); }
    let digest = platform_io::lower_hex(&Sha256::digest(encoded));
    // Recheck both original FDs/names, the held environment and actual stopped
    // gates as a group. Nothing survives this command as a write capability.
    host.recheck()?;
    context.recheck()?;
    stopped.recheck()?;
    originals.recheck(&context.configuration)?;
    if host.operation_started(ACTIVE)? { return Err("configuration replacement activity changed".into()); }
    print(json_codec::json!({
        "state":"read_only_preview", "operation":operation, "request_digest":digest, "plan":plan,
        "configuration_changed":false, "old_content_copied":false, "service_action_taken":false,
        "durable_decision_created":false, "apply_or_undo_available":false,
    }))
}
