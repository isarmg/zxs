pub(crate) mod activation;
pub(crate) mod install;
mod model;
mod render;
pub(crate) mod samba_password;

use crate::{
    Result, installation, print,
    private_io::{BundleDestination, BundlePublication},
};
use command_line::{Args, Subcommand};
use data_serialization::{Deserialize, Serialize};
use json_codec::json;
use sha2::{Digest, Sha256};
use std::{net::SocketAddr, path::Path};

const PARENT: &str = "/zxs";

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Verify installed prerequisites and preview explicit service activation.
    ActivationPreview {
        #[command(flatten)]
        options: Options,
        #[arg(long)]
        enable_on_boot: bool,
    },
    /// Start only the verified ZXS units; never initialize data or stop another service.
    Activate {
        #[command(flatten)]
        options: Options,
        #[arg(long)]
        request_digest: String,
        #[arg(long)]
        enable_on_boot: bool,
        #[arg(long)]
        confirm_activate_installed_services: bool,
        #[arg(long)]
        external_administration_stopped: bool,
    },
    /// Observe the original requested services without starting or enabling them.
    ActivationStatus {
        #[command(flatten)]
        options: Options,
        #[arg(long)]
        request_digest: String,
        #[arg(long)]
        enable_on_boot: bool,
    },
    /// Preview an inactive systemd/Samba bundle; does not inspect or stop services.
    Preview(Options),
    /// Preview fixed systemd/Samba file installation, not service activation.
    InstallPreview(Options),
    /// Recover the exact recorded configuration request without re-entering settings.
    InstallRecoverPreview {
        #[arg(long)]
        request_digest: String,
    },
    /// Continue only the original recorded configuration installation, never activate it.
    InstallRecoverFiles {
        #[arg(long)]
        request_digest: String,
        #[arg(long)]
        confirm_original_configuration_files_only: bool,
        #[arg(long)]
        external_administration_stopped: bool,
    },
    /// Install matching configuration files without replacement, reload or start.
    InstallFiles {
        #[command(flatten)]
        options: Options,
        #[arg(long)]
        request_digest: String,
        #[arg(long)]
        confirm_configuration_files_only: bool,
        /// Operator acknowledgment, not proof that external writers hold a lock.
        #[arg(long)]
        external_administration_stopped: bool,
    },
    /// Durably prepare exactly the previewed bundle; never activate configuration.
    Prepare {
        #[command(flatten)]
        options: Options,
        #[arg(long)]
        request_digest: String,
        #[arg(long)]
        confirm_prepared_only: bool,
    },
}
#[derive(Args)]
pub(crate) struct Options {
    #[arg(long)]
    service_user: String,
    #[arg(long)]
    service_group: String,
    #[arg(long, default_value = "0.0.0.0:7400")]
    listen: SocketAddr,
    #[arg(long, requires = "smb_peer_uid")]
    smb_account: Option<String>,
    #[arg(long, requires = "smb_account")]
    smb_peer_uid: Option<u32>,
}
impl Options {
    fn input(self) -> Result<Input> {
        let smb = match (self.smb_account, self.smb_peer_uid) {
            (None, None) => None,
            (Some(account), Some(peer_uid)) => Some(SmbInput { account, peer_uid }),
            _ => return Err("SMB account and trusted peer UID must be supplied together".into()),
        };
        Ok(Input {
            service_user: self.service_user,
            service_group: self.service_group,
            listen: self.listen,
            smb,
        })
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Input {
    pub service_user: String,
    pub service_group: String,
    pub listen: SocketAddr,
    pub smb: Option<SmbInput>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct SmbInput {
    pub account: String,
    pub peer_uid: u32,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Prepare {
    pub input: Input,
    pub request_digest: String,
    pub confirmation: String,
}
#[derive(Serialize)]
struct Plan {
    format: &'static str,
    revision: u32,
    state: &'static str,
    parent: &'static str,
    parent_device: String,
    parent_inode: String,
    payload: installation::PayloadMetadata,
    service: model::Settings,
    unit: render::ConfigurationFile,
    samba: Option<render::ConfigurationFile>,
}
#[derive(Serialize)]
pub(crate) struct Preview {
    plan: Plan,
    pub request_digest: String,
    destination: String,
    live_configuration_modified: bool,
    service_started_or_enabled: bool,
    service_exclusion_established: bool,
    participating_daemon_exclusion: &'static str,
    required_before_activation: &'static [&'static str],
}
const REQUIREMENTS: &[&str] = &[
    "Verified non-root Unix service account/group and independent Samba account/passdb",
    "Matching ZXS executable, OS dependencies and exact Samba VFS ABI installed",
    "Owned private data/runtime paths and root-owned empty SMB export prepared",
    "Complete first-run system/disk initialization and durable handoff verified",
    "Lifetime exclusion of ZXS/Samba and any other writers before changing active configuration",
    "systemd and testparm validation of exact target environment, then explicit activation approval",
];
fn build(input: Input) -> Result<(BundleDestination, Preview)> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("deployment preparation requires the manually run root helper".into());
    }
    let settings = model::Settings::try_from(input)?;
    let payload = installation::verified_payload_metadata()?;
    let parent = BundleDestination::open(Path::new(PARENT))?;
    let identity = parent.identity();
    let (unit, samba) = render::files(&settings);
    let plan = Plan {
        format: "zxs-helper-deployment",
        revision: 1,
        state: "prepared_not_active",
        parent: PARENT,
        parent_device: identity.device.to_string(),
        parent_inode: identity.inode.to_string(),
        payload,
        service: settings,
        unit,
        samba,
    };
    let request_digest = platform_io::lower_hex(&Sha256::digest(json_codec::to_vec(&plan)?));
    let destination = format!("{PARENT}/.zxs-helper-deployment-{request_digest}");
    Ok((
        parent,
        Preview {
            plan,
            request_digest,
            destination,
            live_configuration_modified: false,
            service_started_or_enabled: false,
            service_exclusion_established: false,
            participating_daemon_exclusion: "held_during_this_operation_only",
            required_before_activation: REQUIREMENTS,
        },
    ))
}
pub(crate) fn preview(input: Input) -> Result<Preview> {
    Ok(build(input)?.1)
}
pub(crate) fn prepare(input: Prepare) -> Result<json_codec::Value> {
    if input.confirmation != "prepare-inactive-configuration" {
        return Err("explicit prepare-inactive-configuration confirmation is required; no configuration write was attempted".into());
    }
    let (parent, preview) = build(input.input)?;
    if input.request_digest != preview.request_digest {
        return Err("deployment inputs, payload or parent identity differ from the exact preview; no configuration write was attempted".into());
    }
    let name = format!(".zxs-helper-deployment-{}", preview.request_digest);
    let manifest = json_codec::to_vec_pretty(&preview)?;
    let mut files = vec![
        ("manifest.json", manifest.as_slice()),
        ("zxs.service", preview.plan.unit.content.as_bytes()),
    ];
    if let Some(samba) = &preview.plan.samba {
        files.push(("smb.conf", samba.content.as_bytes()));
    }
    let publication = parent.publish(&name, &files)?;
    let state = match publication {
        BundlePublication::Created => "new_inactive_bundle_prepared",
        BundlePublication::Resumed => "interrupted_inactive_bundle_completed",
        BundlePublication::Existing => "matching_inactive_bundle_durably_verified",
    };
    Ok(
        json!({"state":state,"destination":preview.destination,"request_digest":preview.request_digest,
        "live_configuration_modified":false,"service_started_or_enabled":false,
        "service_exclusion_established":false,"participating_daemon_exclusion":"held_during_this_operation_only","system_initialized":false,"samba_credentials_changed":false,
        "required_before_activation":REQUIREMENTS}),
    )
}
pub(crate) fn run(command: Command) -> Result<()> {
    match command {
        Command::ActivationPreview {
            options,
            enable_on_boot,
        } => print(activation::preview(activation::Input {
            deployment: options.input()?,
            enable_on_boot,
        })?),
        Command::ActivationStatus {
            options,
            request_digest,
            enable_on_boot,
        } => print(activation::status(activation::Observation {
            input: activation::Input {
                deployment: options.input()?,
                enable_on_boot,
            },
            request_digest,
        })?),
        Command::Activate {
            options,
            request_digest,
            enable_on_boot,
            confirm_activate_installed_services,
            external_administration_stopped,
        } => {
            if !confirm_activate_installed_services {
                return Err("review exact activation and pass --confirm-activate-installed-services; no service action was attempted".into());
            }
            print(activation::activate(activation::Commit {
                input: activation::Input {
                    deployment: options.input()?,
                    enable_on_boot,
                },
                request_digest,
                confirmation: "activate-installed-zxs-services".into(),
                external_administration_stopped,
            })?)
        }
        Command::Preview(options) => print(preview(options.input()?)?),
        Command::InstallPreview(options) => print(install::preview(options.input()?)?),
        Command::InstallRecoverPreview { request_digest } => {
            print(install::recovery::preview(install::recovery::Original {
                request_digest,
            })?)
        }
        Command::InstallRecoverFiles {
            request_digest,
            confirm_original_configuration_files_only,
            external_administration_stopped,
        } => {
            if !confirm_original_configuration_files_only {
                return Err("review the recorded configuration request and pass --confirm-original-configuration-files-only; no host path was inspected".into());
            }
            print(install::recovery::commit(install::recovery::Commit {
                request_digest,
                confirmation: "resume-original-configuration-files-only".into(),
                external_administration_stopped,
            })?)
        }
        Command::InstallFiles {
            options,
            request_digest,
            confirm_configuration_files_only,
            external_administration_stopped,
        } => {
            if !confirm_configuration_files_only {
                return Err("review the exact file installation and pass --confirm-configuration-files-only; no configuration was inspected or modified".into());
            }
            print(install::commit(install::Commit {
                input: options.input()?,
                request_digest,
                confirmation: "install-configuration-files-only".into(),
                external_administration_stopped,
            })?)
        }
        Command::Prepare {
            options,
            request_digest,
            confirm_prepared_only,
        } => {
            if !confirm_prepared_only {
                return Err(
                    "review the exact inactive configuration and pass --confirm-prepared-only"
                        .into(),
                );
            }
            print(prepare(Prepare {
                input: options.input()?,
                request_digest,
                confirmation: "prepare-inactive-configuration".into(),
            })?)
        }
    }
}
#[cfg(test)]
#[path = "../../test/helper/unit/deployment/tests.rs"]
mod tests;
