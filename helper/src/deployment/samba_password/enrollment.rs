//! Explicit first registration after proving the selected local account absent.
//! External writers must remain stopped; uncertain commands are never replayed.
use super::{Input, Original, hex, state};
use crate::{
    Result,
    account::Snapshot as AccountSnapshot,
    deployment::install,
    private_io::{BundleDestination, BundlePublication},
    system_program::{Executable, Program, Snapshot as ProgramSnapshot},
};
use data_serialization::{Deserialize, Serialize};
use platform_io::secret::SecretString;
use sha2::{Digest, Sha256};
use std::path::Path;

pub(crate) type Create = super::Reset;
const MAX_RECORD_BYTES: usize = 64 * 1024;

#[derive(Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "zxs-helper-missing-local-samba-account-enrollment")]
    MissingLocalAccount,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Plan {
    format: Format,
    operation_id: String,
    installed_configuration_digest: String,
    account: AccountSnapshot,
    database: state::enrollment::Baseline,
    programs: Vec<ProgramSnapshot>,
}
#[derive(Serialize)]
pub(crate) struct Preview {
    pub request_digest: String,
    operation_id: String,
    account: String,
    database: &'static str,
    account_absence_verified: bool,
    native_add_is_not_atomic_create_only: bool,
    creates_native_passdb: bool,
    other_account_listing_must_be_unchanged: bool,
    native_samba_identity_may_be_initialized: bool,
    all_external_writers_excluded: bool,
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
    created_account_read_back: bool,
    other_account_listing_unchanged: bool,
    database_synchronized: bool,
    password_authentication_verified: bool,
    active_smb_sessions_revoked: bool,
    zxs_or_unix_account_changed: bool,
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
    database: state::enrollment::Target,
    plan: Plan,
    digest: String,
}
fn digest(plan: &Plan) -> Result<String> {
    Ok(platform_io::lower_hex(&Sha256::digest(json_codec::to_vec(
        plan,
    )?)))
}
fn prepare(input: Input) -> Result<Prepared> {
    if rustix::process::getuid().as_raw() != 0 || rustix::process::geteuid().as_raw() != 0 {
        return Err(
            "Samba account enrollment requires the manually run real/effective root helper".into(),
        );
    }
    if !input.external_writers_stopped {
        return Err("stop Samba and external passdb/configuration administrators before enrollment; this acknowledgment is not an OS lock for those writers".into());
    }
    hex(&input.operation_id, 32)?;
    let configuration = install::existing(input.deployment)?;
    let account = configuration.samba_account()?.snapshot();
    let testparm = Executable::open(Program::Testparm)?;
    let pdbedit = Executable::open(Program::Pdbedit)?;
    let smbpasswd = Executable::open(Program::Smbpasswd)?;
    state::require_local_configuration(&testparm)?;
    configuration.require_existing()?;
    let database = state::enrollment::Target::open(&pdbedit, &account)?;
    let plan = Plan {
        format: Format::MissingLocalAccount,
        operation_id: input.operation_id,
        installed_configuration_digest: configuration.context_digest().into(),
        account,
        database: database.baseline(),
        programs: vec![
            testparm.snapshot(),
            pdbedit.snapshot(),
            smbpasswd.snapshot(),
        ],
    };
    let digest = digest(&plan)?;
    let result = Prepared {
        configuration,
        testparm,
        pdbedit,
        smbpasswd,
        database,
        plan,
        digest,
    };
    result.unchanged()?;
    Ok(result)
}
impl Prepared {
    fn context(&self) -> Result<()> {
        self.configuration.require_existing()?;
        self.testparm.recheck()?;
        self.pdbedit.recheck()?;
        self.smbpasswd.recheck()
    }
    fn unchanged(&self) -> Result<()> {
        self.context()?;
        self.database.require_original()
    }
    fn name(&self) -> String {
        format!(".zxs-helper-samba-enroll-{}", self.digest)
    }
}
pub(crate) fn preview(input: Input) -> Result<Preview> {
    let prepared = prepare(input)?;
    Ok(Preview {
        request_digest: prepared.digest,
        operation_id: prepared.plan.operation_id,
        account: prepared.plan.account.name.as_str().into(),
        database: state::DATABASE,
        account_absence_verified: true,
        native_add_is_not_atomic_create_only: true,
        creates_native_passdb: prepared.database.creates_database(),
        other_account_listing_must_be_unchanged: true,
        native_samba_identity_may_be_initialized: true,
        all_external_writers_excluded: false,
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
            "Samba enrollment {} has a retained intent and may have created its account; query this exact request and preserve all native files, never automatically repeat it or restore an old passdb",
            self.request_digest
        )
    }
}
impl std::error::Error for Pending {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}
pub(crate) fn create(input: Create) -> Result<Status> {
    if input.confirmation != "create-missing-local-samba-account"
        || !input.input.external_writers_stopped
    {
        return Err("explicit missing local Samba account creation and stopped external-writer acknowledgments are required".into());
    }
    hex(&input.request_digest, 64)?;
    authentication::validate_password(input.new_password.expose())?;
    if input.new_password.expose() != input.repeat_password.expose() {
        return Err("new password entries do not match".into());
    }
    let prepared = prepare(input.input)?;
    if prepared.digest != input.request_digest {
        return Err("Samba account absence, database, programs or installed configuration differ from the exact preview; no account command was admitted".into());
    }
    prepared.unchanged()?;
    let host = prepared.configuration.host();
    if host
        .read_operation(&prepared.name(), "plan.json", MAX_RECORD_BYTES)?
        .is_some()
    {
        return Err("this Samba enrollment was already admitted; query its retained outcome, never automatically repeat it".into());
    }
    let bytes = json_codec::to_vec_pretty(&prepared.plan)?;
    if !matches!(
        host.publish(&prepared.name(), &[("plan.json", bytes.as_slice())])?,
        BundlePublication::Created
    ) {
        return Err(
            "original Samba enrollment staging already existed; command was not repeated".into(),
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
    // Native -a changes the password when the name exists. The absence proof
    // therefore requires external Samba writers to remain stopped through -a.
    prepared.smbpasswd.password(
        &[
            "-L",
            "-s",
            "-a",
            "-D",
            "0",
            "-c",
            crate::private_io::SAMBA_PATH,
            prepared.plan.account.name.as_str(),
        ],
        password,
    )?;
    prepared.context()?;
    let after = prepared
        .database
        .complete(&prepared.pdbedit, &prepared.plan.account)?;
    let receipt = Receipt {
        request_digest: prepared.digest.clone(),
        account: prepared.plan.account.name.as_str().into(),
        after,
        smbpasswd_reported_success: true,
        created_account_read_back: true,
        other_account_listing_unchanged: true,
        database_synchronized: true,
        password_authentication_verified: false,
        active_smb_sessions_revoked: false,
        zxs_or_unix_account_changed: false,
        service_started: false,
        complete_installation_ready: false,
    };
    let bytes = json_codec::to_vec_pretty(&receipt)?;
    prepared.configuration.host().publish(
        &format!("{}-complete", prepared.name()),
        &[("receipt.json", bytes.as_slice())],
    )?;
    prepared.context()?;
    prepared.database.require_after(&receipt.after)?;
    Ok(Status::CommandCompletedHistoricalReceipt {
        receipt: Box::new(receipt),
        current_password_or_service_state_verified: false,
    })
}
pub(crate) fn status(input: Original) -> Result<Status> {
    hex(&input.request_digest, 64)?;
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("Samba enrollment records require root".into());
    }
    let host = BundleDestination::open(Path::new("/zxs"))?;
    let name = format!(".zxs-helper-samba-enroll-{}", input.request_digest);
    let Some(bytes) = host.read_operation(&name, "plan.json", MAX_RECORD_BYTES)? else {
        return Ok(Status::NoCompletePlanReadable {
            request_digest: input.request_digest,
        });
    };
    let plan: Plan = json_codec::from_slice(&bytes)
        .map_err(|_| "retained Samba enrollment plan is malformed; preserve it")?;
    if digest(&plan)? != input.request_digest {
        return Err("retained Samba enrollment plan differs from its request digest".into());
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
        .map_err(|_| "retained Samba enrollment receipt is malformed; preserve it")?;
    if receipt.request_digest != input.request_digest
        || receipt.account != plan.account.name.as_str()
        || !receipt.smbpasswd_reported_success
        || !receipt.created_account_read_back
        || !receipt.other_account_listing_unchanged
        || !receipt.database_synchronized
        || receipt.password_authentication_verified
        || receipt.active_smb_sessions_revoked
        || receipt.zxs_or_unix_account_changed
        || receipt.service_started
        || receipt.complete_installation_ready
    {
        return Err(
            "retained Samba enrollment receipt differs from its completion contract".into(),
        );
    }
    host.recheck()?;
    Ok(Status::CommandCompletedHistoricalReceipt {
        receipt: Box::new(receipt),
        current_password_or_service_state_verified: false,
    })
}
