//! Root-only offline reset of an existing system administrator. Recovery and
//! input handling live here; the normal credential format/publisher stays shared.
mod target;
use crate::{Result, account::Name};
use data_serialization::{Deserialize, Deserializer, Serialize};
use json_codec::json;
use platform_io::secret::{SecretBytes, SecretString};
use sha2::{Digest, Sha256};
use storage_server::auth::{
    CredentialRevision, PasswordReplacementOutcome, PreparedPasswordReplacement,
};

const MAX_RECORD_BYTES: usize = 64 * 1024;
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Input {
    pub service_user: String,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct RecoverPreview {
    pub input: Input,
    pub request_digest: String,
}
// Deliberately no Debug, Clone or Serialize. These values must never be sent
// back to the browser, recorded in an operation receipt or put on an argv/URL.
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Reset {
    pub input: Input,
    pub request_digest: String,
    pub confirmation: String,
    pub external_writers_stopped: bool,
    #[serialization(deserialize_with = "secret_string")]
    pub new_password: SecretString,
    #[serialization(deserialize_with = "secret_string")]
    pub repeat_password: SecretString,
}
fn secret_string<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<SecretString, D::Error> {
    String::deserialize(deserializer).map(SecretString::new)
}
#[derive(Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "zxs-helper-administrator-password-reset")]
    Reset,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Plan {
    format: Format,
    context: target::Context,
    previous: CredentialRevision,
}
#[derive(Serialize)]
pub(crate) struct Preview {
    plan: Plan,
    pub request_digest: String,
    administrator: String,
    current_generation: String,
    revoke_all_zxs_sessions: bool,
    changes_samba_password: bool,
    initializes_system: bool,
    current_daemon_exclusion: &'static str,
    all_external_writers_excluded: bool,
    disaster_recovery_replica_updated: bool,
    recovering_original_operation: bool,
    password_may_already_be_changed: bool,
}
// Protected journal only. It retains a verifier, never the submitted password.
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Record {
    plan: Plan,
    change: PreparedPasswordReplacement,
}
fn digest(plan: &Plan) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(json_codec::to_vec(plan)?)))
}
fn request_digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
    {
        return Err("administrator reset requires the exact lowercase preview digest".into());
    }
    Ok(())
}
pub(crate) fn preview(input: Input) -> Result<Preview> {
    let target = target::Target::open(input.service_user)?;
    let previous = target.auth.credential_revision()?;
    let administrator = previous.principal().to_owned();
    let current_generation = previous.generation().to_string();
    let plan = Plan {
        format: Format::Reset,
        context: target.context.clone(),
        previous,
    };
    let request_digest = digest(&plan)?;
    target.recheck()?;
    Ok(Preview {
        plan,
        request_digest,
        administrator,
        current_generation,
        revoke_all_zxs_sessions: true,
        changes_samba_password: false,
        initializes_system: false,
        current_daemon_exclusion: "held_during_this_operation_only",
        all_external_writers_excluded: false,
        disaster_recovery_replica_updated: false,
        recovering_original_operation: false,
        password_may_already_be_changed: false,
    })
}
pub(crate) fn recover_preview(input: RecoverPreview) -> Result<Preview> {
    request_digest(&input.request_digest)?;
    let target = target::Target::open(input.input.service_user)?;
    let name = format!(".zxs-helper-admin-reset-{}", input.request_digest);
    let bytes = target.host.read_operation(&name, "reset.json", MAX_RECORD_BYTES)?
        .ok_or("no complete retained reset record exists for this exact request; incomplete staging was not replaced")?;
    let bytes = SecretBytes::new(bytes);
    let record: Record = json_codec::from_slice(bytes.expose())
        .map_err(|_| "protected reset record is malformed and was preserved")?;
    if digest(&record.plan)? != input.request_digest
        || record.plan.context != target.context
        || record.change.previous() != &record.plan.previous
    {
        return Err("original reset context differs; no recovery preview is admitted".into());
    }
    let current = target.auth.credential_revision()?;
    let replacement = record.change.replacement_revision()?;
    if current != record.plan.previous && current != replacement {
        return Err("credentials changed after this original request; old reset recovery cannot overwrite the later authority".into());
    }
    target.recheck()?;
    Ok(Preview {
        administrator: current.principal().to_owned(),
        current_generation: current.generation().to_string(),
        password_may_already_be_changed: current == replacement,
        plan: record.plan,
        request_digest: input.request_digest,
        revoke_all_zxs_sessions: true,
        changes_samba_password: false,
        initializes_system: false,
        current_daemon_exclusion: "held_during_this_operation_only",
        all_external_writers_excluded: false,
        disaster_recovery_replica_updated: false,
        recovering_original_operation: true,
    })
}
pub(crate) fn reset(input: Reset) -> Result<json_codec::Value> {
    if input.confirmation != "reset-existing-zxs-administrator-password"
        || !input.external_writers_stopped
    {
        return Err("explicit existing-administrator reset and external-writer-stop acknowledgments are required before credential access; an acknowledgment is not an OS lock".into());
    }
    request_digest(&input.request_digest)?;
    if input.new_password.expose() != input.repeat_password.expose() {
        return Err("new password entries do not match; no credential access was attempted".into());
    }
    authentication::validate_password(input.new_password.expose())?;
    let _ = Name::new(input.input.service_user.clone())?;
    let mut target = target::Target::open(input.input.service_user)?;
    let name = format!(".zxs-helper-admin-reset-{}", input.request_digest);
    let retained = target
        .host
        .read_operation(&name, "reset.json", MAX_RECORD_BYTES)?;
    let record = match retained {
        Some(bytes) => {
            let bytes = SecretBytes::new(bytes);
            // Do not send a private-record JSON parser diagnostic to the Web.
            let record: Record = json_codec::from_slice(bytes.expose()).map_err(|_| "protected reset record is malformed; no credentials were changed and the record was preserved")?;
            if !record
                .change
                .verifies_repeated_password(input.new_password.expose())?
            {
                return Err("this request already retains a different new password; re-enter the original password to resolve it, never replace its operation record".into());
            }
            record
        }
        None => {
            let plan = Plan {
                format: Format::Reset,
                context: target.context.clone(),
                previous: target.auth.credential_revision()?,
            };
            if digest(&plan)? != input.request_digest {
                return Err("credentials, service account or directory identity differ from the preview; no reset was prepared".into());
            }
            let change = target
                .auth
                .prepare_system_password_replacement(input.new_password.expose())?;
            Record { plan, change }
        }
    };
    if digest(&record.plan)? != input.request_digest
        || record.plan.context != target.context
        || record.change.previous() != &record.plan.previous
    {
        return Err("protected reset request differs from the exact original context; no credentials were replaced".into());
    }
    target.recheck()?;
    let bytes = SecretBytes::new(json_codec::to_vec(&record)?);
    if bytes.expose().len() > MAX_RECORD_BYTES {
        return Err("protected reset record exceeds its byte budget".into());
    }
    target
        .host
        .publish(&name, &[("reset.json", bytes.expose())])?;
    finish(&mut target, &record, &input.request_digest).map_err(|source| {
        Box::new(PendingReset {
            request_digest: input.request_digest,
            source,
        }) as Box<dyn std::error::Error + Send + Sync>
    })
}
#[derive(Debug)]
struct PendingReset {
    request_digest: String,
    source: Box<dyn std::error::Error + Send + Sync>,
}
impl std::fmt::Display for PendingReset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "administrator reset {} may already have changed the password; preserve its private operation and re-enter the same new password with the same original request to resolve it; no rollback or replica update is claimed",
            self.request_digest
        )
    }
}
impl std::error::Error for PendingReset {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}
fn finish(
    target: &mut target::Target,
    record: &Record,
    request_digest: &str,
) -> Result<json_codec::Value> {
    target.recheck()?;
    let outcome = target
        .auth
        .apply_system_password_replacement(&record.change)?;
    target.recheck()?;
    let replacement = record.change.replacement_revision()?;
    if target.auth.credential_revision()? != replacement {
        return Err("credential authority differs after reset publication".into());
    }
    let receipt = json!({"format":"zxs-helper-administrator-password-reset-complete", "request_digest":request_digest,
        "administrator":replacement.principal(),"generation":replacement.generation().to_string(),
        "all_zxs_sessions_revoked":true,"samba_password_changed":false,"disaster_recovery_replica_updated":false,
        "system_initialized":false,"service_started":false});
    let bytes = json_codec::to_vec(&receipt)?;
    target.host.publish(
        &format!(".zxs-helper-admin-reset-complete-{request_digest}"),
        &[("receipt.json", bytes.as_slice())],
    )?;
    target.recheck()?;
    Ok(
        json!({"state":match outcome { PasswordReplacementOutcome::Published => "administrator_password_reset_durably_completed", PasswordReplacementOutcome::ExistingDurablyVerified => "original_administrator_reset_durably_resolved" },
        "receipt":receipt,"current_daemon_exclusion":"held_during_this_operation_only","all_external_writers_excluded":false}),
    )
}

#[cfg(test)]
#[path = "../../test/helper/unit/credentials/tests.rs"]
mod tests;
