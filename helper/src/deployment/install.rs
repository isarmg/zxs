//! Confirmed, no-overwrite installation of fixed configuration files.
pub(crate) mod recovery;
use super::{Input, context::Context, render};
use crate::{
    Result,
    account::{GroupSnapshot, LocalAccount, Snapshot},
    installation::PayloadMetadata,
    private_io::{BundleDestination, ParentSnapshot, SAMBA_PATH, SYSTEMD_PATH},
};
use data_serialization::{Deserialize, Serialize};
use json_codec::json;
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Commit {
    pub input: Input,
    pub request_digest: String,
    pub confirmation: String,
    pub external_administration_stopped: bool,
}
#[derive(Serialize)]
struct Plan {
    format: &'static str,
    revision: u32,
    host_device: String,
    host_inode: String,
    configuration_parents: Vec<ParentSnapshot>,
    payload: PayloadMetadata,
    service_account: Snapshot,
    service_group: GroupSnapshot,
    samba_account: Option<Snapshot>,
    unit_destination: &'static str,
    samba_destination: Option<&'static str>,
    mode: &'static str,
    unit: render::ConfigurationFile,
    samba: Option<render::ConfigurationFile>,
}
#[derive(Serialize)]
pub(crate) struct Preview {
    plan: Plan,
    pub request_digest: String,
    exact_existing_files: Vec<bool>,
    intent_directory: String,
    completion_directory: String,
    activation_included: bool,
    system_initialized: bool,
    current_daemon_exclusion: &'static str,
    all_external_writers_excluded: bool,
    required_before_start: &'static [&'static str],
}
const REQUIREMENTS: &[&str] = &[
    "Complete verified first-run account/storage/database handoff; configuration files are not readiness evidence",
    "Prepared service-owned data, runtime and log paths and independently protected empty SMB export",
    "Verified installed OS/Samba dependencies, exact ZXS VFS ABI and independently initialized Samba passdb",
    "Target-system systemd and Samba syntax/configuration validation, including existing unit drop-ins and Samba includes",
    "Separate explicit activation: reload, select the standalone /etc/samba/zxs.conf and start/enable the intended services",
];
pub(super) struct Prepared {
    host: BundleDestination,
    original_input: Input,
    context: Context,
    preview: Preview,
}
impl Prepared {
    fn recheck(&self) -> Result<()> {
        self.host.recheck()?;
        self.context.recheck()
    }
    pub(super) fn require_existing(&self) -> Result<()> {
        self.recheck()?;
        self.context
            .require_existing(&self.preview.plan.unit, self.preview.plan.samba.as_ref())
    }
    pub(super) fn context_digest(&self) -> &str {
        &self.preview.request_digest
    }
    pub(super) fn host(&self) -> &BundleDestination {
        &self.host
    }
    pub(super) fn samba_account(&self) -> Result<&LocalAccount> {
        self.context
            .samba_account
            .as_ref()
            .ok_or_else(|| "an explicitly configured independent Samba account is required".into())
    }
}
pub(super) fn existing(input: Input) -> Result<Prepared> {
    let prepared = build(input)?;
    prepared.require_existing()?;
    Ok(prepared)
}

fn require_root() -> Result<()> {
    if rustix::process::getuid().as_raw() != 0 || rustix::process::geteuid().as_raw() != 0 {
        return Err(
            "configuration file installation requires the manually run real/effective root helper"
                .into(),
        );
    }
    Ok(())
}
fn build(input: Input) -> Result<Prepared> {
    require_root()?;
    let host = BundleDestination::open(Path::new("/zxs"))?;
    build_at(input, host)
}
fn build_at(input: Input, host: BundleDestination) -> Result<Prepared> {
    let (context, unit, samba) = Context::open(&input)?;
    let present = context.configuration.inspect(
        unit.content.as_bytes(),
        samba.as_ref().map(|file| file.content.as_bytes()),
    )?;
    let identity = host.identity();
    let plan = Plan {
        format: "zxs-helper-configuration-file-installation",
        revision: 2,
        host_device: identity.device.to_string(),
        host_inode: identity.inode.to_string(),
        configuration_parents: context.configuration.parents(),
        payload: context.program.metadata(),
        service_account: context.account.snapshot(),
        service_group: context.group.snapshot(),
        samba_account: context.samba_account.as_ref().map(LocalAccount::snapshot),
        unit_destination: SYSTEMD_PATH,
        samba_destination: samba.as_ref().map(|_| SAMBA_PATH),
        mode: "0644",
        unit,
        samba,
    };
    let request_digest = platform_io::lower_hex(&Sha256::digest(json_codec::to_vec(&plan)?));
    let preview = Preview {
        intent_directory: format!("/zxs/.zxs-helper-config-intent-{request_digest}"),
        completion_directory: format!("/zxs/.zxs-helper-config-installed-{request_digest}"),
        plan,
        request_digest,
        exact_existing_files: present,
        activation_included: false,
        system_initialized: false,
        current_daemon_exclusion: "held_for_helper_lifetime",
        all_external_writers_excluded: false,
        required_before_start: REQUIREMENTS,
    };
    Ok(Prepared {
        host,
        original_input: input,
        context,
        preview,
    })
}
pub(crate) fn preview(input: Input) -> Result<Preview> {
    let prepared = build(input)?;
    prepared.recheck()?;
    Ok(prepared.preview)
}

/// A durable intent retains the original request and published files so the
/// same installation can resume after an error.
#[derive(Debug)]
struct InstallationPending {
    request_digest: String,
    intent_directory: String,
    source: Box<dyn std::error::Error + Send + Sync>,
}
impl std::fmt::Display for InstallationPending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "configuration installation {} is incomplete or has an uncertain publication; inspect {}, preserve installed/staging files and retry the exact request only; no rollback or service activation is claimed",
            self.request_digest, self.intent_directory
        )
    }
}
impl std::error::Error for InstallationPending {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}
pub(crate) fn commit(input: Commit) -> Result<json_codec::Value> {
    if input.confirmation != "install-configuration-files-only"
        || !input.external_administration_stopped
    {
        return Err("explicit install-configuration-files-only confirmation and acknowledgment that external service/configuration administration is stopped are required before inspecting or writing any configuration; the acknowledgment is not an OS exclusion proof".into());
    }
    let prepared = build(input.input)?;
    if input.request_digest != prepared.preview.request_digest {
        return Err("configuration, payload, account/group source or parent identity differs from the exact preview; no installation was attempted".into());
    }
    publish_pending(&prepared)
}
fn publish_pending(prepared: &Prepared) -> Result<json_codec::Value> {
    publish(prepared).map_err(|source| {
        Box::new(InstallationPending {
            request_digest: prepared.preview.request_digest.clone(),
            intent_directory: prepared.preview.intent_directory.clone(),
            source,
        }) as Box<dyn std::error::Error + Send + Sync>
    })
}
fn publish(prepared: &Prepared) -> Result<json_codec::Value> {
    let plan = &prepared.preview.plan;
    let digest = &prepared.preview.request_digest;
    recovery::check_records(prepared)?;
    let original = recovery::record_bytes(prepared)?;
    prepared.host.publish(
        &recovery::request_name(digest),
        &[("request.json", original.as_slice())],
    )?;
    prepared.recheck()?;
    let manifest = json_codec::to_vec_pretty(plan)?;
    let mut files = vec![
        ("plan.json", manifest.as_slice()),
        ("zxs.service", plan.unit.content.as_bytes()),
    ];
    if let Some(samba) = &plan.samba {
        files.push(("zxs.conf", samba.content.as_bytes()));
    }
    prepared
        .host
        .publish(&format!(".zxs-helper-config-intent-{digest}"), &files)?;
    prepared.recheck()?;
    let unit_created = prepared
        .context
        .configuration
        .install_unit(plan.unit.content.as_bytes())?;
    prepared.recheck()?;
    let samba_created = match &plan.samba {
        Some(file) => Some(
            prepared
                .context
                .configuration
                .install_samba(file.content.as_bytes())?,
        ),
        None => None,
    };
    prepared.recheck()?;
    let presence = prepared.context.configuration.inspect(
        plan.unit.content.as_bytes(),
        plan.samba.as_ref().map(|file| file.content.as_bytes()),
    )?;
    if presence.iter().any(|present| !present) {
        return Err(
            "a configuration file disappeared before the durable completion receipt".into(),
        );
    }
    let receipt = completion_receipt(prepared);
    let bytes = json_codec::to_vec_pretty(&receipt)?;
    prepared.host.publish(
        &format!(".zxs-helper-config-installed-{digest}"),
        &[("receipt.json", bytes.as_slice())],
    )?;
    prepared.recheck()?;
    let final_presence = prepared.context.configuration.inspect(
        plan.unit.content.as_bytes(),
        plan.samba.as_ref().map(|file| file.content.as_bytes()),
    )?;
    if final_presence.iter().any(|present| !present) {
        return Err("configuration changed after the installation receipt; receipt is not current readiness evidence".into());
    }
    Ok(
        json!({"state":"configuration_files_durably_installed_not_activated", "receipt":receipt,
        "unit_created_in_this_attempt":unit_created,"samba_created_in_this_attempt":samba_created,
        "intent_directory":prepared.preview.intent_directory,"completion_directory":prepared.preview.completion_directory,
        "current_daemon_exclusion":"held_for_helper_lifetime","all_external_writers_excluded":false,
        "required_before_start":REQUIREMENTS}),
    )
}

fn completion_receipt(prepared: &Prepared) -> json_codec::Value {
    json!({"format":"zxs-helper-configuration-files-installed", "revision":2,
        "request_digest":prepared.preview.request_digest,"unit_destination":SYSTEMD_PATH,
        "samba_destination":prepared.preview.plan.samba_destination,
        "service_reloaded":false,"service_started":false,"autostart_enabled":false,
        "samba_default_configuration_changed":false,"samba_passdb_changed":false,
        "system_initialized":false,"activation_verified":false})
}
