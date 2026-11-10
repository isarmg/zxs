//! Confirmed password reset for an existing local Samba account.
//! Unknown external-command outcomes retain their original request for inspection.
pub(crate) mod enrollment;
mod operation;
mod state;
use super::Input as DeploymentInput;
use crate::{Result, private_io::BundleDestination, system_program::Executable};
use data_serialization::{Deserialize, Deserializer, Serialize};
use operation::{Context, Pending, digest};
use platform_io::secret::SecretString;
use std::path::Path;

const MAX_RECORD_BYTES: usize = 64 * 1024;
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Input {
    pub deployment: DeploymentInput,
    pub operation_id: String,
    pub external_writers_stopped: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Reset {
    pub input: Input,
    pub request_digest: String,
    pub confirmation: String,
    #[serialization(deserialize_with = "secret")]
    pub new_password: SecretString,
    #[serialization(deserialize_with = "secret")]
    pub repeat_password: SecretString,
}
fn secret<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<SecretString, D::Error> {
    String::deserialize(deserializer).map(SecretString::new)
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Original {
    request_digest: String,
}
#[derive(Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "zxs-helper-existing-local-samba-password-reset")]
    LocalSambaPassword,
}
type Plan = operation::Plan<Format, state::Revision>;
#[derive(Serialize)]
pub(crate) struct Preview {
    pub request_digest: String,
    operation_id: String,
    account: String,
    database: &'static str,
    exact_installed_configuration: bool,
    local_backend_parameters_checked: bool,
    account_existence_check_pending: bool,
    all_external_writers_excluded: bool,
    active_smb_sessions_revoked: bool,
    changes_unix_or_zxs_password: bool,
    complete_installation_ready: bool,
    automatic_replay_available: bool,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Receipt {
    request_digest: String,
    account: String,
    after: state::Revision,
    smbpasswd_reported_success: bool,
    existing_account_read_back: bool,
    database_synchronized: bool,
    password_authentication_verified: bool,
    active_smb_sessions_revoked: bool,
    zxs_administrator_changed: bool,
    service_started: bool,
    complete_installation_ready: bool,
}
pub(crate) type Status = operation::Status<Receipt>;
struct Prepared {
    context: Context,
    database: state::Database,
    plan: Plan,
    digest: String,
}
fn hex(value: &str, length: usize) -> Result<()> {
    if value.len() != length
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(
            "operation identity/digest must be the complete lowercase hexadecimal value".into(),
        );
    }
    Ok(())
}
fn prepare(input: Input) -> Result<Prepared> {
    if rustix::process::getuid().as_raw() != 0 || rustix::process::geteuid().as_raw() != 0 {
        return Err(
            "Samba password maintenance requires the manually run real/effective root helper"
                .into(),
        );
    }
    if !input.external_writers_stopped {
        return Err("stop Samba and external passdb/configuration administrators before preparing this reset; this acknowledgment is not an OS lock for those writers".into());
    }
    hex(&input.operation_id, 32)?;
    let (context, account) = Context::open(input.deployment)?;
    let database = state::Database::open()?;
    let plan = Plan {
        format: Format::LocalSambaPassword,
        operation_id: input.operation_id,
        installed_configuration_digest: context.configuration.context_digest().into(),
        account,
        database: database.revision()?,
        programs: context.programs(),
    };
    let digest = digest(&plan)?;
    Ok(Prepared {
        context,
        database,
        plan,
        digest,
    })
}
impl Prepared {
    fn unchanged(&self) -> Result<()> {
        self.context.recheck()?;
        self.database.require_original()
    }
    fn name(&self) -> String {
        format!(".zxs-helper-samba-reset-{}", self.digest)
    }
}
pub(super) fn require_activation(
    testparm: &Executable,
    pdbedit: &Executable,
    account: &crate::account::Snapshot,
) -> Result<()> {
    // pdbedit may initialize a missing passdb, so hold the existing file and
    // its exact revision before querying it.
    let database = state::Database::open()?;
    state::require_local_configuration(testparm)?;
    state::require_existing_account(pdbedit, account)?;
    database.require_original()
}

pub(crate) fn preview(input: Input) -> Result<Preview> {
    let prepared = prepare(input)?;
    prepared.unchanged()?;
    Ok(Preview {
        request_digest: prepared.digest,
        operation_id: prepared.plan.operation_id,
        account: prepared.plan.account.name.as_str().into(),
        database: state::DATABASE,
        exact_installed_configuration: true,
        local_backend_parameters_checked: true,
        account_existence_check_pending: true,
        all_external_writers_excluded: false,
        active_smb_sessions_revoked: false,
        changes_unix_or_zxs_password: false,
        complete_installation_ready: false,
        automatic_replay_available: false,
    })
}
pub(crate) fn reset(input: Reset) -> Result<Status> {
    if input.confirmation != "reset-existing-local-samba-password"
        || !input.input.external_writers_stopped
    {
        return Err("explicit existing local Samba password reset and stopped external-writer acknowledgments are required".into());
    }
    hex(&input.request_digest, 64)?;
    authentication::validate_password(input.new_password.expose())?;
    if input.new_password.expose() != input.repeat_password.expose() {
        return Err("new password entries do not match".into());
    }
    let prepared = prepare(input.input)?;
    if input.request_digest != prepared.digest {
        return Err("Samba configuration, account, database metadata or fixed programs differ from the exact preview; no password command was admitted".into());
    }
    prepared.unchanged()?;
    operation::admit(
        prepared.context.configuration.host(),
        &prepared.name(),
        &prepared.plan,
    )?;
    execute(&prepared, &input.new_password).map_err(|source| {
        Box::new(Pending {
            operation: "reset",
            request_digest: prepared.digest.clone(),
            source,
        }) as Box<dyn std::error::Error + Send + Sync>
    })
}
fn execute(prepared: &Prepared, password: &SecretString) -> Result<Status> {
    prepared.unchanged()?;
    state::require_existing_account(&prepared.context.pdbedit, &prepared.plan.account)?;
    // pdbedit is not called with export/hash/modification flags. Unexpected
    // metadata changes, including its own maintenance, prevent password write.
    prepared.unchanged()?;
    prepared.context.smbpasswd.password(
        &[
            "-L",
            "-s",
            "-D",
            "0",
            "-c",
            crate::private_io::SAMBA_PATH,
            prepared.plan.account.name.as_str(),
        ],
        password,
    )?;
    prepared.context.configuration.require_existing()?;
    prepared.database.revision()?;
    state::require_existing_account(&prepared.context.pdbedit, &prepared.plan.account)?;
    let after = prepared.database.synchronize()?;
    let receipt = Receipt {
        request_digest: prepared.digest.clone(),
        account: prepared.plan.account.name.as_str().into(),
        after,
        smbpasswd_reported_success: true,
        existing_account_read_back: true,
        database_synchronized: true,
        password_authentication_verified: false,
        active_smb_sessions_revoked: false,
        zxs_administrator_changed: false,
        service_started: false,
        complete_installation_ready: false,
    };
    let bytes = json_codec::to_vec_pretty(&receipt)?;
    prepared.context.configuration.host().publish(
        &format!("{}-complete", prepared.name()),
        &[("receipt.json", bytes.as_slice())],
    )?;
    prepared.context.configuration.require_existing()?;
    if prepared.database.revision()? != receipt.after {
        return Err(
            "Samba database changed after reset receipt; receipt is historical only".into(),
        );
    }
    Ok(Status::CommandCompletedHistoricalReceipt {
        receipt: Box::new(receipt),
        current_password_or_service_state_verified: false,
    })
}
pub(crate) fn status(input: Original) -> Result<Status> {
    hex(&input.request_digest, 64)?;
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("Samba reset records require root".into());
    }
    let host = BundleDestination::open(Path::new("/zxs"))?;
    let name = format!(".zxs-helper-samba-reset-{}", input.request_digest);
    let Some(bytes) = host.read_operation(&name, "plan.json", MAX_RECORD_BYTES)? else {
        return Ok(Status::NoCompletePlanReadable {
            request_digest: input.request_digest,
        });
    };
    let plan: Plan = json_codec::from_slice(&bytes)
        .map_err(|_| "retained Samba reset plan is malformed; preserve it")?;
    if digest(&plan)? != input.request_digest {
        return Err("retained Samba reset plan does not match its request digest".into());
    }
    let Some(bytes) = host.read_operation(
        &format!("{name}-complete"),
        "receipt.json",
        MAX_RECORD_BYTES,
    )?
    else {
        return Ok(Status::OutcomeUnknownDoNotReplay {
            request_digest: input.request_digest,
            account: plan.account.name.as_str().into(),
        });
    };
    let receipt: Receipt = json_codec::from_slice(&bytes)
        .map_err(|_| "retained Samba reset receipt is malformed; preserve it")?;
    if receipt.request_digest != input.request_digest
        || receipt.account != plan.account.name.as_str()
        || !receipt.smbpasswd_reported_success
        || !receipt.existing_account_read_back
        || !receipt.database_synchronized
        || receipt.password_authentication_verified
        || receipt.active_smb_sessions_revoked
        || receipt.zxs_administrator_changed
        || receipt.service_started
        || receipt.complete_installation_ready
    {
        return Err(
            "retained Samba receipt differs from this operation's exact completion contract".into(),
        );
    }
    host.recheck()?;
    Ok(Status::CommandCompletedHistoricalReceipt {
        receipt: Box::new(receipt),
        current_password_or_service_state_verified: false,
    })
}
#[cfg(test)]
#[path = "../../../test/helper/unit/samba_password.rs"]
mod tests;
