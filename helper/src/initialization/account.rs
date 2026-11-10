use crate::{
    Result,
    account::{LocalAccount, LocalGroup, Name},
    private_io::BundleDestination,
    system_program::{Executable, Program, Snapshot},
};
use data_serialization::{Deserialize, Serialize};
use platform_io::service_lease::HOST_ROOT;
use sha2::{Digest, Sha256};
use std::path::Path;

const MAX_RECORD_BYTES: usize = 16 * 1024;
const HOME: &str = "/nonexistent";
const SHELL: &str = "/usr/sbin/nologin";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Input {
    pub service_user: Name,
    pub service_group: Name,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Prepare {
    pub input: Input,
    pub request_digest: String,
    pub confirmation: String,
    pub external_administration_stopped: bool,
}
#[derive(Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "zxs-helper-local-service-account")]
    ServiceAccount,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Binding {
    uid: u32,
    gid: u32,
}
#[derive(Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    CreateNonlogin { program: Snapshot },
    VerifyExisting { binding: Binding },
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Plan {
    format: Format,
    input: Input,
    host_device: String,
    host_inode: String,
    action: Action,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Receipt {
    request_digest: String,
    account: Binding,
}
#[derive(Serialize)]
pub(crate) struct Preview {
    plan: Plan,
    request_digest: String,
    requires_creation: bool,
    home_for_new_account: &'static str,
    shell_for_new_account: &'static str,
    creates_home_directory: bool,
    changes_existing_account: bool,
    starts_service: bool,
}
#[derive(Serialize)]
pub(crate) struct Outcome {
    state: &'static str,
    request_digest: String,
    account: Binding,
    native_command_ran: bool,
    service_started: bool,
}
struct Prepared {
    host: BundleDestination,
    plan: Plan,
    digest: String,
    intent: String,
    completion: String,
}
fn selected(input: &Input) -> Result<Option<LocalAccount>> {
    let Some(account) = LocalAccount::lookup(input.service_user.clone())? else {
        return Ok(None);
    };
    let group = LocalGroup::resolve(input.service_group.clone())?;
    if account.owner().1 != group.gid() {
        return Err(
            "selected service group is not the account's primary group; no identity was changed"
                .into(),
        );
    }
    Ok(Some(account))
}
fn binding(account: &LocalAccount) -> Binding {
    let (uid, gid) = account.owner();
    Binding { uid, gid }
}
fn instruction(input: &Input) -> String {
    format!("u {} - - {HOME} {SHELL}", input.service_user.as_str())
}
impl Prepared {
    fn open(input: Input) -> Result<Self> {
        if rustix::process::getuid().as_raw() != 0 || rustix::process::geteuid().as_raw() != 0 {
            return Err("service account preparation requires the manually run root helper".into());
        }
        let host = BundleDestination::open(Path::new(HOST_ROOT))?;
        let identity = host.identity();
        let intent = format!(
            ".zxs-helper-service-account-plan-{}",
            input.service_user.as_str()
        );
        let completion = format!(
            ".zxs-helper-service-account-complete-{}",
            input.service_user.as_str()
        );
        let retained = host.read_operation(&intent, "plan.json", MAX_RECORD_BYTES)?;
        let plan: Plan = match retained {
            Some(bytes) => json_codec::from_slice(&bytes)?,
            None => {
                let action = match selected(&input)? {
                    Some(account) => Action::VerifyExisting {
                        binding: binding(&account),
                    },
                    None => {
                        if input.service_user != input.service_group {
                            return Err(
                                "a new service account requires its own same-name primary group"
                                    .into(),
                            );
                        }
                        Action::CreateNonlogin {
                            program: Executable::open(Program::SystemdSysusers)?.snapshot(),
                        }
                    }
                };
                Plan {
                    format: Format::ServiceAccount,
                    input: input.clone(),
                    host_device: identity.device.to_string(),
                    host_inode: identity.inode.to_string(),
                    action,
                }
            }
        };
        if plan.input != input
            || plan.host_device != identity.device.to_string()
            || plan.host_inode != identity.inode.to_string()
        {
            return Err(
                "service identity input or host root differs from the retained original plan"
                    .into(),
            );
        }
        let digest = platform_io::lower_hex(&Sha256::digest(json_codec::to_vec(&plan)?));
        Ok(Self {
            host,
            plan,
            digest,
            intent,
            completion,
        })
    }
    fn observe(&self) -> Result<Option<Binding>> {
        let account = selected(&self.plan.input)?;
        let current = match (&self.plan.action, account) {
            (Action::CreateNonlogin { .. }, Some(account)) => {
                if account.home() != Path::new(HOME) || account.shell() != Path::new(SHELL) {
                    return Err("the selected name now has a different login profile; no existing account was modified".into());
                }
                Some(binding(&account))
            }
            (Action::CreateNonlogin { .. }, None) => {
                if LocalGroup::lookup(self.plan.input.service_group.clone())?.is_some() {
                    return Err("a group exists without the original service account; inspect any partial native operation, no unrelated group was adopted".into());
                }
                None
            }
            (Action::VerifyExisting { binding: expected }, Some(account))
                if binding(&account) == *expected =>
            {
                Some(*expected)
            }
            (Action::VerifyExisting { .. }, _) => {
                return Err("the original existing service UID/GID changed or disappeared; no account was recreated".into());
            }
        };
        if let Some(bytes) =
            self.host
                .read_operation(&self.completion, "receipt.json", MAX_RECORD_BYTES)?
        {
            let receipt: Receipt = json_codec::from_slice(&bytes)?;
            if receipt.request_digest != self.digest || current != Some(receipt.account) {
                return Err("the completed service identity no longer matches its original receipt; no account was recreated".into());
            }
        }
        Ok(current)
    }
}
pub(crate) fn preview(input: Input) -> Result<Preview> {
    let prepared = Prepared::open(input)?;
    let requires_creation = prepared.observe()?.is_none();
    Ok(Preview {
        plan: prepared.plan,
        request_digest: prepared.digest,
        requires_creation,
        home_for_new_account: HOME,
        shell_for_new_account: SHELL,
        creates_home_directory: false,
        changes_existing_account: false,
        starts_service: false,
    })
}
pub(crate) fn prepare(input: Prepare) -> Result<Outcome> {
    if input.confirmation != "prepare-local-service-account"
        || !input.external_administration_stopped
    {
        return Err("confirm prepare-local-service-account and stop external account administrators before changing local identities".into());
    }
    let prepared = Prepared::open(input.input)?;
    if input.request_digest != prepared.digest {
        return Err(
            "service identity plan differs from the exact preview; no account command was admitted"
                .into(),
        );
    }
    let mut current = prepared.observe()?;
    let bytes = json_codec::to_vec(&prepared.plan)?;
    prepared
        .host
        .publish(&prepared.intent, &[("plan.json", &bytes)])?;
    let native_command_ran = current.is_none();
    if native_command_ran {
        let Action::CreateNonlogin { program } = &prepared.plan.action else {
            unreachable!("invariant: missing existing identities are rejected by observe")
        };
        let executable = Executable::open(Program::SystemdSysusers)?;
        if executable.snapshot() != *program {
            return Err(
                "the fixed systemd-sysusers executable differs from the original plan".into(),
            );
        }
        if LocalGroup::lookup(prepared.plan.input.service_group.clone())?.is_some() {
            return Err("a group now exists without the original service account; inspect the possibly partial native operation, do not adopt or replace that group".into());
        }
        prepared.host.recheck()?;
        executable.capture(&["--inline", &instruction(&prepared.plan.input)])?;
        current = prepared.observe()?;
    }
    let account = current.ok_or("systemd-sysusers did not create the selected service identity; preserve the original request")?;
    let receipt = Receipt {
        request_digest: prepared.digest.clone(),
        account,
    };
    let bytes = json_codec::to_vec(&receipt)?;
    prepared
        .host
        .publish(&prepared.completion, &[("receipt.json", &bytes)])?;
    Ok(Outcome {
        state: "local_service_identity_prepared",
        request_digest: prepared.digest,
        account,
        native_command_ran,
        service_started: false,
    })
}

#[cfg(test)]
#[path = "../../../test/helper/unit/initialization/account.rs"]
mod tests;
