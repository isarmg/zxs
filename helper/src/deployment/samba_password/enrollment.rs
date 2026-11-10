//! Explicit first registration after proving the selected local account absent.
//! External writers must remain stopped; uncertain commands are never replayed.
use super::{Input, Original, hex, operation, state};
use crate::{Result, private_io::BundleDestination};
use data_serialization::{Deserialize, Serialize};
use operation::{Context, Pending, digest};
use platform_io::secret::SecretString;
use std::path::Path;

pub(crate) type Create = super::Reset;
const MAX_RECORD_BYTES: usize = 64 * 1024;

#[derive(Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "zxs-helper-missing-local-samba-account-enrollment")]
    MissingLocalAccount,
}
type Plan = operation::Plan<Format, state::enrollment::Baseline>;
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
pub(crate) type Status = operation::Status<Receipt>;
struct Prepared {
    context: Context,
    database: state::enrollment::Target,
    plan: Plan,
    digest: String,
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
    let (context, account) = Context::open(input.deployment)?;
    let database = state::enrollment::Target::open(&context.pdbedit, &account)?;
    let plan = Plan {
        format: Format::MissingLocalAccount,
        operation_id: input.operation_id,
        installed_configuration_digest: context.configuration.context_digest().into(),
        account,
        database: database.baseline(),
        programs: context.programs(),
    };
    let digest = digest(&plan)?;
    let result = Prepared {
        context,
        database,
        plan,
        digest,
    };
    result.unchanged()?;
    Ok(result)
}
impl Prepared {
    fn unchanged(&self) -> Result<()> {
        self.context.recheck()?;
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
    operation::admit(
        prepared.context.configuration.host(),
        &prepared.name(),
        &prepared.plan,
    )?;
    execute(&prepared, &input.new_password).map_err(|source| {
        Box::new(Pending {
            operation: "enrollment",
            request_digest: prepared.digest.clone(),
            source,
        }) as Box<dyn std::error::Error + Send + Sync>
    })
}
fn execute(prepared: &Prepared, password: &SecretString) -> Result<Status> {
    prepared.unchanged()?;
    // Native -a changes the password when the name exists. The absence proof
    // therefore requires external Samba writers to remain stopped through -a.
    prepared.context.smbpasswd.password(
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
    prepared.context.recheck()?;
    let after = prepared
        .database
        .complete(&prepared.context.pdbedit, &prepared.plan.account)?;
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
    prepared.context.configuration.host().publish(
        &format!("{}-complete", prepared.name()),
        &[("receipt.json", bytes.as_slice())],
    )?;
    prepared.context.recheck()?;
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

#[cfg(test)]
#[path = "../../../../test/helper/unit/samba_enrollment_tests.rs"]
mod tests;
