//! Embedded program-only first installation. It never configures or starts a
//! service, initializes storage, or replaces a different existing program.
mod payload;
use crate::{Result, print, private_io::ProgramDestination};
use command_line::Subcommand;
use data_serialization::{Deserialize, Serialize};
use json_codec::json;
use sha2::{Digest, Sha256};
use std::path::Path;

pub(crate) const DESTINATION: &str = "/usr/local/bin/zxs";
#[derive(Subcommand)]
pub(crate) enum Command {
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
fn prepare(payload: payload::Payload) -> Result<(ProgramDestination, Preview)> {
    require_root()?;
    let destination = ProgramDestination::open(Path::new(DESTINATION))?;
    let identity = destination.parent_identity();
    let matches = destination.matches_or_absent(payload.bytes)?;
    let plan = Plan {
        format: "zxs-helper-program-install",
        destination: DESTINATION,
        parent_device: identity.device.to_string(),
        parent_inode: identity.inode.to_string(),
        payload: payload.metadata(),
    };
    let digest = format!("{:x}", Sha256::digest(json_codec::to_vec(&plan)?));
    Ok((
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
    Ok(prepare(payload::required()?)?.1)
}
pub(crate) fn commit(input: Commit) -> Result<json_codec::Value> {
    if input.confirmation != "install-program-only" {
        return Err(
            "explicit install-program-only confirmation is required; no installation was attempted"
                .into(),
        );
    }
    let payload = payload::required()?;
    let (destination, preview) = prepare(payload)?;
    if preview.request_digest != input.request_digest {
        return Err("installation parent or payload differs from the exact preview; no installation was attempted".into());
    }
    let created = destination.install(payload.bytes)?;
    Ok(
        json!({"state":if created {"new_program_installed"} else {"matching_program_durably_verified"}, "destination":DESTINATION, "payload":payload.metadata(), "request_digest":preview.request_digest, "system_initialized":false, "service_configured":false, "service_started":false, "replaced_different_program":false}),
    )
}
pub(crate) fn availability() -> json_codec::Value {
    json!({"embedded":payload::metadata(),"destination":DESTINATION,"program_only":true,"repair_available":false,"network_download":false})
}
pub(crate) fn run(command: Command) -> Result<()> {
    match command {
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
