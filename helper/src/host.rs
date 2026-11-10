//! Prepare the fixed administrative host root.
use crate::{Result, print, private_io::HostRoot};
use command_line::Subcommand;
use data_serialization::{Deserialize, Serialize};
use json_codec::json;
use platform_io::service_lease::HOST_ROOT;
use sha2::{Digest, Sha256};

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Inspect only the fixed /zxs directory required for current-daemon exclusion.
    Preview,
    /// Prepare exactly that empty root if absent; no credentials/data/service changes.
    Prepare {
        #[arg(long)]
        request_digest: String,
        #[arg(long)]
        confirm_host_root_only: bool,
    },
}
#[derive(Serialize)]
struct Plan {
    format: &'static str,
    destination: &'static str,
    parent_device: String,
    parent_inode: String,
    owner_uid: u32,
    mode: &'static str,
}
#[derive(Serialize)]
pub(crate) struct Preview {
    plan: Plan,
    pub request_digest: String,
    already_present: bool,
    system_initialized: bool,
    live_configuration_modified: bool,
    service_stopped_or_started: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Prepare {
    pub request_digest: String,
    pub confirmation: String,
}
fn build() -> Result<(HostRoot, Preview)> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("host root preparation requires the manually run root helper".into());
    }
    let root = HostRoot::fixed()?;
    let identity = root.parent_identity();
    let already_present = root.present()?;
    let plan = Plan {
        format: "zxs-helper-host-root",
        destination: HOST_ROOT,
        parent_device: identity.device.to_string(),
        parent_inode: identity.inode.to_string(),
        owner_uid: 0,
        mode: "0755",
    };
    let request_digest = platform_io::lower_hex(&Sha256::digest(json_codec::to_vec(&plan)?));
    Ok((
        root,
        Preview {
            plan,
            request_digest,
            already_present,
            system_initialized: false,
            live_configuration_modified: false,
            service_stopped_or_started: false,
        },
    ))
}
pub(crate) fn preview() -> Result<Preview> {
    Ok(build()?.1)
}
pub(crate) fn prepare(input: Prepare) -> Result<json_codec::Value> {
    if input.confirmation != "prepare-host-root-only" {
        return Err(
            "explicit prepare-host-root-only confirmation is required; no host write was attempted"
                .into(),
        );
    }
    let (root, preview) = build()?;
    if input.request_digest != preview.request_digest {
        return Err("host root parent or preparation contract differs from the exact preview; no host write was attempted".into());
    }
    let (created, identity) = root.prepare()?;
    Ok(
        json!({"state":if created {"new_host_root_prepared"} else {"existing_host_root_durably_verified"},
        "path":HOST_ROOT,"device":identity.device.to_string(),"inode":identity.inode.to_string(),
        "request_digest":preview.request_digest,"system_initialized":false,"accounts_created":false,
        "disks_initialized":false,"service_stopped_or_started":false,"live_configuration_modified":false,
        "service_exclusion_held_after_response":false}),
    )
}
pub(crate) fn run(command: Command) -> Result<()> {
    match command {
        Command::Preview => print(preview()?),
        Command::Prepare {
            request_digest,
            confirm_host_root_only,
        } => {
            if !confirm_host_root_only {
                return Err("review the exact host root and pass --confirm-host-root-only".into());
            }
            print(prepare(Prepare {
                request_digest,
                confirmation: "prepare-host-root-only".into(),
            })?)
        }
    }
}
#[cfg(test)]
#[path = "../../test/helper/unit/host/tests.rs"]
mod tests;
