pub(crate) mod enrollment;
mod state;
use super::{Input as DeploymentInput, install};
use crate::{
    Result,
    account::Snapshot as AccountSnapshot,
    private_io::{BundleDestination, BundlePublication},
    system_program::{Executable, Program, Snapshot as ProgramSnapshot},
};
use data_serialization::{Deserialize, Deserializer, Serialize};
use platform_io::secret::SecretString;
use sha2::{Digest, Sha256};
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
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Plan {
    format: Format,
    operation_id: String,
    installed_configuration_digest: String,
    account: AccountSnapshot,
    database: state::Revision,
    programs: Vec<ProgramSnapshot>,
}
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
#[derive(Serialize)]
#[serialization(tag = "state", rename_all = "snake_case")]
pub(crate) enum Status {
    NoCompletePlanReadable {
        request_digest: String,
    },
    OutcomeUnknownDoNotReplay {
        request_digest: String,
        account: String,
    },
    CommandCompletedHistoricalReceipt {
        receipt: Box<Receipt>,
        current_password_or_service_state_verified: bool,
    },
}
struct Prepared {
    configuration: install::Prepared,
    testparm: Executable,
    pdbedit: Executable,
    smbpasswd: Executable,
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
fn digest(plan: &Plan) -> Result<String> {
    Ok(platform_io::lower_hex(&Sha256::digest(json_codec::to_vec(
        plan,
    )?)))
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
    let configuration = install::existing(input.deployment)?;
    let account = configuration.samba_account()?.snapshot();
    let testparm = Executable::open(Program::Testparm)?;
    let pdbedit = Executable::open(Program::Pdbedit)?;
    let smbpasswd = Executable::open(Program::Smbpasswd)?;
    state::require_local_configuration(&testparm)?;
    configuration.require_existing()?;
    let database = state::Database::open()?;
    let plan = Plan {
        format: Format::LocalSambaPassword,
        operation_id: input.operation_id,
        installed_configuration_digest: configuration.context_digest().into(),
        account,
        database: database.revision()?,
        programs: vec![
            testparm.snapshot(),
            pdbedit.snapshot(),
            smbpasswd.snapshot(),
        ],
    };
    let digest = digest(&plan)?;
    Ok(Prepared {
        configuration,
        testparm,
        pdbedit,
        smbpasswd,
        database,
        plan,
        digest,
    })
}
impl Prepared {
    fn unchanged(&self) -> Result<()> {
        self.configuration.require_existing()?;
        self.testparm.recheck()?;
        self.pdbedit.recheck()?;
        self.smbpasswd.recheck()?;
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
#[derive(Debug)]
struct Pending {
    request_digest: String,
    source: Box<dyn std::error::Error + Send + Sync>,
}
impl std::fmt::Display for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Samba reset {} has a retained intent and may have changed the password; query this exact request, preserve evidence and do not automatically replay or restore an old database",
            self.request_digest
        )
    }
}
impl std::error::Error for Pending {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
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
    let recorded = prepared.configuration.host().read_operation(
        &prepared.name(),
        "plan.json",
        MAX_RECORD_BYTES,
    )?;
    if recorded.is_some() {
        return Err("this original Samba reset was already admitted; query its retained outcome, never automatically repeat it".into());
    }
    let bytes = json_codec::to_vec_pretty(&prepared.plan)?;
    if !matches!(
        prepared
            .configuration
            .host()
            .publish(&prepared.name(), &[("plan.json", bytes.as_slice())])?,
        BundlePublication::Created
    ) {
        return Err(
            "original Samba reset staging already existed; command was not repeated".into(),
        );
    }
    execute(&prepared, &input.new_password).map_err(|source| {
        Box::new(Pending {
            request_digest: prepared.digest.clone(),
            source,
        }) as Box<dyn std::error::Error + Send + Sync>
    })
}
fn execute(prepared: &Prepared, password: &SecretString) -> Result<Status> {
    prepared.unchanged()?;
    state::require_existing_account(&prepared.pdbedit, &prepared.plan.account)?;
    prepared.unchanged()?;
    prepared.smbpasswd.password(
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
    prepared.configuration.require_existing()?;
    prepared.database.revision()?;
    state::require_existing_account(&prepared.pdbedit, &prepared.plan.account)?;
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
    prepared.configuration.host().publish(
        &format!("{}-complete", prepared.name()),
        &[("receipt.json", bytes.as_slice())],
    )?;
    prepared.configuration.require_existing()?;
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
