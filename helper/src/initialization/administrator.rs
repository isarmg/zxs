//! First administrator in the installed empty layout. It deliberately creates
//! management-without-storage, never a ready storage pool or dummy disk.
mod target;
use crate::Result;
use data_serialization::{Deserialize, Deserializer, Serialize};
use json_codec::json;
use platform_io::secret::{SecretBytes, SecretString};
use sha2::{Digest, Sha256};
use storage_server::auth::{AuthStore, InitialManagementOutcome, PreparedInitialAdministrator};
const MAX_RECORD_BYTES: usize = 64 * 1024;
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Input {
    pub service_user: String,
    pub administrator: String,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct RecoverPreview {
    pub input: Input,
    pub request_digest: String,
}
// Secrets are only admitted from a bounded POST body, never Debug/Serialize/argv.
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Create {
    pub input: Input,
    pub request_digest: String,
    pub confirmation: String,
    pub external_writers_stopped: bool,
    #[serialization(deserialize_with = "secret")]
    pub password: SecretString,
    #[serialization(deserialize_with = "secret")]
    pub repeat_password: SecretString,
}
fn secret<'de, D: Deserializer<'de>>(de: D) -> std::result::Result<SecretString, D::Error> {
    String::deserialize(de).map(SecretString::new)
}
#[derive(Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "zxs-helper-first-management-administrator")]
    ManagementOnly,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Plan {
    format: Format,
    context: target::Context,
    administrator: String,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Record {
    plan: Plan,
    candidate: PreparedInitialAdministrator,
}
#[derive(Serialize)]
pub(crate) struct Preview {
    plan: Plan,
    pub request_digest: String,
    recovering_original_operation: bool,
    initializes_management_only: bool,
    storage_initialized: bool,
    normal_storage_start_ready: bool,
    unix_account_created: bool,
    samba_account_created: bool,
    current_daemon_exclusion: &'static str,
}
fn digest(plan: &Plan) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(json_codec::to_vec(plan)?)))
}
fn checked_digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("first administrator requires the exact lowercase preview digest".into());
    }
    Ok(())
}
fn response(plan: Plan, request_digest: String, recovering: bool) -> Preview {
    Preview {
        plan,
        request_digest,
        recovering_original_operation: recovering,
        initializes_management_only: true,
        storage_initialized: false,
        normal_storage_start_ready: false,
        unix_account_created: false,
        samba_account_created: false,
        current_daemon_exclusion: "held_during_this_operation_only",
    }
}
pub(crate) fn preview(input: Input) -> Result<Preview> {
    let administrator = authentication::normalize_administrator_username(&input.administrator)?;
    let target = target::Target::open(input.service_user)?;
    target.admit_empty()?;
    let plan = Plan {
        format: Format::ManagementOnly,
        context: target.context.clone(),
        administrator,
    };
    let digest = digest(&plan)?;
    Ok(response(plan, digest, false))
}
fn load(target: &target::Target, request_digest: &str) -> Result<Option<Record>> {
    let bytes = target.host.read_operation(
        &format!(".zxs-helper-first-admin-{request_digest}"),
        "administrator.json",
        MAX_RECORD_BYTES,
    )?;
    match bytes {
        None => Ok(None),
        Some(bytes) => {
            let bytes = SecretBytes::new(bytes);
            json_codec::from_slice(bytes.expose())
                .map(Some)
                .map_err(|_| {
                    "private first-administrator record is malformed; it was preserved".into()
                })
        }
    }
}
fn admit_record(
    target: &target::Target,
    record: &Record,
    request: &str,
    administrator: &str,
) -> Result<()> {
    if digest(&record.plan)? != request
        || record.plan.context != target.context
        || record.plan.administrator != administrator
        || record.candidate.credential_revision()?.principal() != administrator
    {
        return Err("original first-administrator request, identity or account differs; no authority was overwritten".into());
    }
    target.admit_original(&record.candidate)
}
pub(crate) fn recover_preview(input: RecoverPreview) -> Result<Preview> {
    checked_digest(&input.request_digest)?;
    let administrator =
        authentication::normalize_administrator_username(&input.input.administrator)?;
    let target = target::Target::open(input.input.service_user)?;
    let record = load(&target, &input.request_digest)?.ok_or("no complete retained initial-administrator record exists; unknown staging was not replaced")?;
    admit_record(&target, &record, &input.request_digest, &administrator)?;
    Ok(response(record.plan, input.request_digest, true))
}
pub(crate) fn create(input: Create) -> Result<json_codec::Value> {
    if input.confirmation != "initialize-first-management-administrator"
        || !input.external_writers_stopped
    {
        return Err("explicit management-only first-administrator and external-writer-stop acknowledgments are required before system access".into());
    }
    checked_digest(&input.request_digest)?;
    if input.password.expose() != input.repeat_password.expose() {
        return Err("password entries do not match; no system access was attempted".into());
    }
    authentication::validate_password(input.password.expose())?;
    let administrator =
        authentication::normalize_administrator_username(&input.input.administrator)?;
    let target = target::Target::open(input.input.service_user)?;
    let record = match load(&target, &input.request_digest)? {
        Some(record) => {
            admit_record(&target, &record, &input.request_digest, &administrator)?;
            if !record
                .candidate
                .verifies_repeated_password(input.password.expose())?
            {
                return Err("the original initialization retains a different password; re-enter the same original password instead of replacing its record".into());
            }
            record
        }
        None => {
            target.admit_empty()?;
            let plan = Plan {
                format: Format::ManagementOnly,
                context: target.context.clone(),
                administrator: administrator.clone(),
            };
            if digest(&plan)? != input.request_digest {
                return Err("initial administrator, service account or layout differs from preview; no initializer was published".into());
            }
            let candidate =
                PreparedInitialAdministrator::new(&administrator, input.password.expose())?;
            Record { plan, candidate }
        }
    };
    target.recheck()?;
    let bytes = SecretBytes::new(json_codec::to_vec(&record)?);
    if bytes.expose().len() > MAX_RECORD_BYTES {
        return Err("private administrator initialization record exceeds its budget".into());
    }
    target.host.publish(
        &format!(".zxs-helper-first-admin-{}", input.request_digest),
        &[("administrator.json", bytes.expose())],
    )?;
    finish(&target, &record, &input.request_digest).map_err(|source| {
        Box::new(Pending {
            request_digest: input.request_digest,
            source,
        }) as Box<dyn std::error::Error + Send + Sync>
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
            "first administrator {} may be partly or fully published; preserve the original private record and retry the same request/password; storage is not initialized and no rollback is claimed",
            self.request_digest
        )
    }
}
impl std::error::Error for Pending {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}
fn finish(
    target: &target::Target,
    record: &Record,
    request_digest: &str,
) -> Result<json_codec::Value> {
    target.admit_original(&record.candidate)?;
    target.publish_intent()?;
    let (auth, outcome) = AuthStore::create_initial_system_management(
        target.management_directory()?,
        &record.candidate,
    )?;
    target.recheck()?;
    target.publish_authority()?;
    target.verify_complete()?;
    if auth.credential_revision()? != record.candidate.credential_revision()? {
        return Err("administrator differs after initial publication".into());
    }
    let receipt = json!({"format":"zxs-helper-first-management-administrator-complete","request_digest":request_digest,
        "administrator":record.plan.administrator,"generation":"1","management_initialized":true,
        "storage_initialized":false,"normal_storage_start_ready":false,"unix_account_created":false,
        "samba_account_created":false,"service_started":false});
    let bytes = json_codec::to_vec(&receipt)?;
    target.host.publish(
        &format!(".zxs-helper-first-admin-complete-{request_digest}"),
        &[("receipt.json", bytes.as_slice())],
    )?;
    target.verify_complete()?;
    // auth drops here, before the caller can release setup and the host lease.
    Ok(
        json!({"state":match outcome { InitialManagementOutcome::Created => "first_management_administrator_durably_created", InitialManagementOutcome::ExistingDurablyVerified => "original_first_management_administrator_durably_resolved" },"receipt":receipt,
        "current_daemon_exclusion":"held_during_this_operation_only","all_external_writers_excluded":false}),
    )
}
#[cfg(test)]
#[path = "../../../test/helper/unit/initialization/administrator_tests.rs"]
mod tests;
