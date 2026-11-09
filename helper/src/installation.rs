//! Embedded program-only installation and separately confirmed same-payload
//! repair. Install/repair never initialize storage, configure or start services.
//! The separate maintenance transport runs only the sealed matching payload.
mod baseline;
pub(crate) mod materials;
mod payload;
pub(crate) mod repair;
pub(crate) mod worker;
pub(crate) use payload::Metadata as PayloadMetadata;
pub(crate) fn verified_payload_metadata() -> Result<PayloadMetadata> {
    Ok(payload::required()?.metadata())
}
/// Hold the normal program-publication lock while configuration is derived from
/// the exact installed payload. Configuration never repairs a different binary.
pub(crate) struct InstalledProgram {
    destination: crate::private_io::ProgramDestination,
    payload: payload::Payload,
}
impl InstalledProgram {
    pub(crate) fn open() -> Result<Self> {
        let payload = payload::required()?;
        let destination =
            crate::private_io::ProgramDestination::open(std::path::Path::new(DESTINATION))?;
        let result = Self {
            destination,
            payload,
        };
        result.recheck()?;
        Ok(result)
    }
    pub(crate) fn metadata(&self) -> PayloadMetadata {
        self.payload.metadata()
    }
    pub(crate) fn recheck(&self) -> Result<()> {
        if !self.destination.matches_or_absent(self.payload.bytes)? {
            return Err(
                "install the exact embedded ZXS program before installing configuration".into(),
            );
        }
        Ok(())
    }
}
use crate::{
    Result, print,
    private_io::{BundleDestination, ProgramDestination},
};
use command_line::Subcommand;
use data_serialization::{Deserialize, Serialize};
use json_codec::json;
use sha2::{Digest, Sha256};
use std::path::Path;

pub(crate) const DESTINATION: &str = "/usr/local/bin/zxs";
#[derive(Subcommand)]
pub(crate) enum Command {
    /// Preview same-payload repair using the original installation baseline.
    RepairPreview,
    /// Read only the exact retained repair request and its current program state.
    RepairRecoverPreview {
        #[arg(long)]
        request_digest: String,
    },
    /// Retain original bytes and restore the exact recorded payload; never start it.
    RepairCommit {
        #[arg(long)]
        request_digest: String,
        #[arg(long)]
        confirm_same_payload_program_only: bool,
        #[arg(long)]
        external_administration_stopped: bool,
    },
    /// Export the exact embedded public license/notice archive without extraction or replacement.
    Materials {
        #[arg(long)]
        output: std::path::PathBuf,
    },
    /// Print the exact source and file inventory for the embedded public materials.
    MaterialsManifest,
    /// Show the exact matching embedded executable, or explicit absence in a development build.
    Payload,
    /// Read-only preview of fixed /usr/local/bin/zxs; never configures or starts a service.
    Preview,
    /// Install or durably verify only the exact embedded program; never overwrite different bytes.
    Commit {
        #[arg(long)]
        request_digest: String,
        #[arg(long)]
        confirm_program_only: bool,
    },
}
#[derive(Serialize)]
struct Plan<'a> {
    format: &'static str,
    destination: &'a str,
    parent_device: String,
    parent_inode: String,
    host_device: String,
    host_inode: String,
    payload: payload::Metadata,
}
#[derive(Serialize)]
pub(crate) struct Preview {
    plan: Plan<'static>,
    pub request_digest: String,
    existing_program_matches: bool,
    overwrite_different_program: bool,
    system_initialization_included: bool,
    systemd_or_smb_configuration_included: bool,
    service_start_included: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Commit {
    pub request_digest: String,
    pub confirmation: String,
}
fn require_root() -> Result<()> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("program installation requires the manually run root helper".into());
    }
    Ok(())
}
fn prepare(payload: payload::Payload) -> Result<(BundleDestination, ProgramDestination, Preview)> {
    require_root()?;
    let host = BundleDestination::open(Path::new(platform_io::service_lease::HOST_ROOT))?;
    let destination = ProgramDestination::open(Path::new(DESTINATION))?;
    baseline::existing(&host, &baseline::expected(&host, &destination, payload)?)?;
    let identity = destination.parent_identity();
    let matches = destination.matches_or_absent(payload.bytes)?;
    let plan = Plan {
        format: "zxs-helper-program-install",
        destination: DESTINATION,
        parent_device: identity.device.to_string(),
        parent_inode: identity.inode.to_string(),
        host_device: host.identity().device.to_string(),
        host_inode: host.identity().inode.to_string(),
        payload: payload.metadata(),
    };
    let digest = platform_io::lower_hex(&Sha256::digest(json_codec::to_vec(&plan)?));
    Ok((
        host,
        destination,
        Preview {
            plan,
            request_digest: digest,
            existing_program_matches: matches,
            overwrite_different_program: false,
            system_initialization_included: false,
            systemd_or_smb_configuration_included: false,
            service_start_included: false,
        },
    ))
}
pub(crate) fn preview() -> Result<Preview> {
    Ok(prepare(payload::required()?)?.2)
}
pub(crate) fn commit(input: Commit) -> Result<json_codec::Value> {
    if input.confirmation != "install-program-only" {
        return Err(
            "explicit install-program-only confirmation is required; no installation was attempted"
                .into(),
        );
    }
    let payload = payload::required()?;
    let (host, destination, preview) = prepare(payload)?;
    if preview.request_digest != input.request_digest {
        return Err("installation parent or payload differs from the exact preview; no installation was attempted".into());
    }
    host.recheck()?;
    let created = destination.install(payload.bytes)?;
    let baseline = baseline::expected(&host, &destination, payload)?;
    host.publish(baseline::NAME, &[("receipt.json", &baseline)])?;
    host.recheck()?;
    if !destination.matches_or_absent(payload.bytes)? {
        return Err(
            "program disappeared after baseline publication; no successful installation is claimed"
                .into(),
        );
    }
    Ok(
        json!({"state":if created {"new_program_installed"} else {"matching_program_durably_verified"}, "destination":DESTINATION, "payload":payload.metadata(), "request_digest":preview.request_digest, "same_payload_baseline_recorded":true, "system_initialized":false, "service_configured":false, "service_started":false, "replaced_different_program":false}),
    )
}
pub(crate) fn availability() -> json_codec::Value {
    json!({"embedded":payload::metadata(),"legal_materials":materials::metadata(),"destination":DESTINATION,"program_only":true,"repair_available":payload::metadata().is_some(),"repair_requires_exact_installation_baseline":true,"network_download":false})
}
pub(crate) fn run(command: Command) -> Result<()> {
    match command {
        Command::RepairPreview => print(repair::preview()?),
        Command::RepairRecoverPreview { request_digest } => {
            print(repair::recover_preview(repair::Original {
                request_digest,
            })?)
        }
        Command::RepairCommit {
            request_digest,
            confirm_same_payload_program_only,
            external_administration_stopped,
        } => {
            if !confirm_same_payload_program_only {
                return Err("review the exact repair and pass --confirm-same-payload-program-only; no program was inspected or changed".into());
            }
            print(repair::commit(repair::Commit {
                request_digest,
                confirmation: "repair-same-payload-program-only".into(),
                external_administration_stopped,
            })?)
        }
        Command::Materials { output } => print(materials::export(&output)?),
        Command::MaterialsManifest => materials::print_manifest(),
        Command::Payload => print(availability()),
        Command::Preview => print(preview()?),
        Command::Commit {
            request_digest,
            confirm_program_only,
        } => {
            if !confirm_program_only {
                return Err("review the exact plan and pass --confirm-program-only; no installation was attempted".into());
            }
            print(commit(Commit {
                request_digest,
                confirmation: "install-program-only".into(),
            })?)
        }
    }
}
