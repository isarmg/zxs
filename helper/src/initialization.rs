//! Staged offline first installation: layout, administrator, and initial storage.
pub(crate) mod account;
mod layout;
pub(crate) mod runtime;
use crate::{
    Result,
    account::{LocalAccount, Name, Snapshot},
    print,
};
use command_line::Subcommand;
use data_serialization::{Deserialize, Serialize};
use json_codec::json;
use sha2::{Digest, Sha256};

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Preview creation of a missing nonlogin Unix service identity or verification of an existing one.
    AccountPreview {
        #[arg(long)]
        service_user: String,
        #[arg(long)]
        service_group: String,
    },
    /// Explicitly prepare only the original local Unix service account and primary group.
    AccountPrepare {
        #[arg(long)]
        service_user: String,
        #[arg(long)]
        service_group: String,
        #[arg(long)]
        request_digest: String,
        #[arg(long)]
        confirm_local_service_account: bool,
        #[arg(long)]
        external_administration_stopped: bool,
    },
    /// Preview first-install private runtime paths and a protected empty SMB export.
    RuntimePreview {
        #[arg(long)]
        service_user: String,
    },
    /// Prepare only those empty directories; never clear used runtime state or start services.
    RuntimePrepare {
        #[arg(long)]
        service_user: String,
        #[arg(long)]
        request_digest: String,
        #[arg(long)]
        confirm_empty_runtime_only: bool,
        #[arg(long)]
        external_administration_stopped: bool,
    },
    /// Preview the three empty private data directories for an existing local account.
    LayoutPreview {
        #[arg(long)]
        service_user: String,
    },
    /// Create that complete empty layout only; never chown or clear existing data.
    LayoutPrepare {
        #[arg(long)]
        service_user: String,
        #[arg(long)]
        request_digest: String,
        #[arg(long)]
        confirm_private_layout_only: bool,
    },
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Input {
    pub service_user: String,
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
    destination: &'static str,
    parent_device: String,
    parent_inode: String,
    account: Snapshot,
    child_directories: &'static [&'static str],
    mode: &'static str,
}
#[derive(Serialize)]
pub(crate) struct Preview {
    plan: Plan,
    pub request_digest: String,
    matching_empty_layout_present: bool,
    system_initialized: bool,
    existing_permissions_changed: bool,
}
fn build(input: Input) -> Result<(LocalAccount, layout::Destination, Preview)> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("private layout preparation requires the manually run root helper".into());
    }
    let account = LocalAccount::resolve(Name::new(input.service_user)?)?;
    let destination = layout::Destination::host(account.owner())?;
    let existing = destination.matching_or_absent()?;
    let identity = destination.identity();
    let plan = Plan {
        format: "zxs-helper-private-data-layout",
        destination: "/zxs/data",
        parent_device: identity.device.to_string(),
        parent_inode: identity.inode.to_string(),
        account: account.snapshot(),
        child_directories: layout::CHILDREN,
        mode: "0700",
    };
    let request_digest = platform_io::lower_hex(&Sha256::digest(json_codec::to_vec(&plan)?));
    Ok((
        account,
        destination,
        Preview {
            plan,
            request_digest,
            matching_empty_layout_present: existing,
            system_initialized: false,
            existing_permissions_changed: false,
        },
    ))
}
pub(crate) fn preview(input: Input) -> Result<Preview> {
    Ok(build(input)?.2)
}
pub(crate) fn prepare(input: Prepare) -> Result<json_codec::Value> {
    if input.confirmation != "prepare-private-layout-only" {
        return Err("explicit prepare-private-layout-only confirmation is required; no data layout write was attempted".into());
    }
    let (account, destination, preview) = build(input.input)?;
    if input.request_digest != preview.request_digest {
        return Err("service identity, passwd source or host root differs from the exact preview; no data layout write was attempted".into());
    }
    account.recheck()?;
    let created = destination.publish(&preview.request_digest, || account.recheck())?;
    account.recheck()?;
    Ok(
        json!({"state":if created {"new_empty_private_layout_prepared"} else {"matching_empty_private_layout_durably_verified"},
        "destination":"/zxs/data","service_account":account.snapshot(),"request_digest":preview.request_digest,
        "system_initialized":false,"administrator_created":false,"databases_initialized":false,"disks_initialized":false,
        "existing_permissions_changed":false,"service_started":false}),
    )
}
pub(crate) fn run(command: Command) -> Result<()> {
    match command {
        Command::AccountPreview {
            service_user,
            service_group,
        } => print(account::preview(account::Input {
            service_user: Name::new(service_user)?,
            service_group: Name::new(service_group)?,
        })?),
        Command::AccountPrepare {
            service_user,
            service_group,
            request_digest,
            confirm_local_service_account,
            external_administration_stopped,
        } => {
            if !confirm_local_service_account {
                return Err(
                    "review the exact account plan and pass --confirm-local-service-account".into(),
                );
            }
            print(account::prepare(account::Prepare {
                input: account::Input {
                    service_user: Name::new(service_user)?,
                    service_group: Name::new(service_group)?,
                },
                request_digest,
                confirmation: "prepare-local-service-account".into(),
                external_administration_stopped,
            })?)
        }
        Command::RuntimePreview { service_user } => {
            print(runtime::preview(Input { service_user })?)
        }
        Command::RuntimePrepare {
            service_user,
            request_digest,
            confirm_empty_runtime_only,
            external_administration_stopped,
        } => {
            if !confirm_empty_runtime_only {
                return Err("review the exact runtime paths and pass --confirm-empty-runtime-only; no directory was inspected or changed".into());
            }
            print(runtime::prepare(runtime::Prepare {
                input: Input { service_user },
                request_digest,
                confirmation: "prepare-empty-runtime-only".into(),
                external_administration_stopped,
            })?)
        }
        Command::LayoutPreview { service_user } => print(preview(Input { service_user })?),
        Command::LayoutPrepare {
            service_user,
            request_digest,
            confirm_private_layout_only,
        } => {
            if !confirm_private_layout_only {
                return Err(
                    "review the exact account/layout and pass --confirm-private-layout-only".into(),
                );
            }
            print(prepare(Prepare {
                input: Input { service_user },
                request_digest,
                confirmation: "prepare-private-layout-only".into(),
            })?)
        }
    }
}
