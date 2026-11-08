//! Confirmed, no-overwrite installation of fixed configuration files. This is
//! deliberately not daemon reload/start/enable, Samba passdb, or a ready marker.
use super::{Input, model, render};
use crate::{
    Result,
    account::{GroupSnapshot, LocalAccount, LocalGroup, Snapshot},
    installation::{InstalledProgram, PayloadMetadata},
    private_io::{BundleDestination, ConfigurationFiles, ParentSnapshot, SAMBA_PATH, SYSTEMD_PATH},
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
    program: InstalledProgram,
    account: LocalAccount,
    group: LocalGroup,
    samba_account: Option<LocalAccount>,
    destinations: ConfigurationFiles,
    preview: Preview,
}
impl Prepared {
    fn recheck(&self) -> Result<()> {
        self.host.recheck()?;
        self.program.recheck()?;
        self.account.recheck()?;
        self.group.recheck()?;
        if let Some(account) = &self.samba_account {
            account.recheck()?;
        }
        self.destinations.recheck()
    }
}
impl Prepared {
    pub(super) fn require_existing(&self) -> Result<()> {
        self.recheck()?;
        let plan = &self.preview.plan;
        if self
            .destinations
            .inspect(
                plan.unit.content.as_bytes(),
                plan.samba.as_ref().map(|item| item.content.as_bytes()),
            )?
            .iter()
            .any(|present| !present)
        {
            return Err("exact installed configuration is required; missing files are not installed by this operation".into());
        }
        Ok(())
    }
    pub(super) fn context_digest(&self) -> &str {
        &self.preview.request_digest
    }
    pub(super) fn host(&self) -> &BundleDestination {
        &self.host
    }
    pub(super) fn samba_account(&self) -> Result<&LocalAccount> {
        self.samba_account
            .as_ref()
            .ok_or_else(|| "an explicitly configured independent Samba account is required".into())
    }
}
pub(super) fn existing(input: Input) -> Result<Prepared> {
    let prepared = build(input)?;
    prepared.require_existing()?;
    Ok(prepared)
}

fn build(input: Input) -> Result<Prepared> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("configuration file installation requires the manually run root helper".into());
    }
    let settings = model::Settings::try_from(input)?;
    let account = LocalAccount::resolve(settings.user.clone())?;
    let group = LocalGroup::resolve(settings.group.clone())?;
    if group.gid() != account.owner().1 {
        return Err("service group must resolve to the selected account's primary GID; supplementary or mismatched group overrides are not supported".into());
    }
    let samba_account = match &settings.smb {
        Some(smb) => {
            let resolved = LocalAccount::resolve(smb.account.clone())?;
            if resolved.owner().0 == account.owner().0 {
                return Err(
                    "Samba authentication UID must be distinct from the private ZXS service UID"
                        .into(),
                );
            }
            Some(resolved)
        }
        None => None,
    };
    // This is the same stable /zxs directory lock used for the full lifetime of
    // the current daemon. A console does not hold this lease between requests.
    let host = BundleDestination::open(Path::new("/zxs"))?;
    let program = InstalledProgram::open()?;
    let destinations = ConfigurationFiles::open(settings.smb.is_some())?;
    let (unit, samba) = render::files(&settings);
    let present = destinations.inspect(
        unit.content.as_bytes(),
        samba.as_ref().map(|file| file.content.as_bytes()),
    )?;
    let identity = host.identity();
    let plan = Plan {
        format: "zxs-helper-configuration-file-installation",
        revision: 1,
        host_device: identity.device.to_string(),
        host_inode: identity.inode.to_string(),
        configuration_parents: destinations.parents(),
        payload: program.metadata(),
        service_account: account.snapshot(),
        service_group: group.snapshot(),
        samba_account: samba_account.as_ref().map(LocalAccount::snapshot),
        unit_destination: SYSTEMD_PATH,
        samba_destination: samba.as_ref().map(|_| SAMBA_PATH),
        mode: "0644",
        unit,
        samba,
    };
    let request_digest = format!("{:x}", Sha256::digest(json_codec::to_vec(&plan)?));
    let preview = Preview {
        intent_directory: format!("/zxs/.zxs-helper-config-intent-{request_digest}"),
        completion_directory: format!("/zxs/.zxs-helper-config-installed-{request_digest}"),
        plan,
        request_digest,
        exact_existing_files: present,
        activation_included: false,
        system_initialized: false,
        current_daemon_exclusion: "held_during_this_operation_only",
        all_external_writers_excluded: false,
        required_before_start: REQUIREMENTS,
    };
    let result = Prepared {
        host,
        program,
        account,
        group,
        samba_account,
        destinations,
        preview,
    };
    result.recheck()?;
    Ok(result)
}
pub(crate) fn preview(input: Input) -> Result<Preview> {
    Ok(build(input)?.preview)
}

/// An error after the intent is durable does not claim rollback. Keep the
/// request and possible installed files, then retry only this exact operation.
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
    publish(&prepared).map_err(|source| {
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
        .destinations
        .install_unit(plan.unit.content.as_bytes())?;
    prepared.recheck()?;
    let samba_created = match &plan.samba {
        Some(file) => Some(
            prepared
                .destinations
                .install_samba(file.content.as_bytes())?,
        ),
        None => None,
    };
    prepared.recheck()?;
    let presence = prepared.destinations.inspect(
        plan.unit.content.as_bytes(),
        plan.samba.as_ref().map(|file| file.content.as_bytes()),
    )?;
    if presence.iter().any(|present| !present) {
        return Err(
            "a configuration file disappeared before the durable completion receipt".into(),
        );
    }
    let receipt = json!({"format":"zxs-helper-configuration-files-installed", "revision":1,
        "request_digest":digest,"unit_destination":SYSTEMD_PATH,"samba_destination":plan.samba_destination,
        "service_reloaded":false,"service_started":false,"autostart_enabled":false,
        "samba_default_configuration_changed":false,"samba_passdb_changed":false,
        "system_initialized":false,"activation_verified":false});
    let bytes = json_codec::to_vec_pretty(&receipt)?;
    prepared.host.publish(
        &format!(".zxs-helper-config-installed-{digest}"),
        &[("receipt.json", bytes.as_slice())],
    )?;
    prepared.recheck()?;
    let final_presence = prepared.destinations.inspect(
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
        "current_daemon_exclusion":"held_during_this_operation_only","all_external_writers_excluded":false,
        "required_before_start":REQUIREMENTS}),
    )
}
