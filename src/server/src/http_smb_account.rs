//! Existing Samba account self-service, without passdb or Unix account authority.
use crate::{ApiError, AppState, auth::Identity, http_auth, lock};
use data_serialization::Deserialize;
use http_routing::{
    Extension, Json,
    extract::{Request, State},
    http::StatusCode,
};
use json_codec::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use storage_contract::smb_account::{PasswordChangeAvailability, SmbAccountStatus};

const CLIENT: &str = "/usr/bin/smbpasswd";
const CLIENT_TIMEOUT: Duration = Duration::from_secs(15);
// Independent of the host's smb.conf, which may contain credential helpers or
// weaker client policy. No share, passdb, domain mapping or secrets are written.
const CLIENT_CONFIG: &str = "[global]\nclient min protocol = SMB3_00\nclient max protocol = SMB3\nclient ipc min protocol = SMB3_00\nclient ipc max protocol = SMB3\nclient signing = mandatory\nclient ipc signing = mandatory\nclient smb encrypt = required\nclient lanman auth = no\nname resolve order = host\n";

struct LocalAccount(String);
impl LocalAccount {
    fn parse(username: &str) -> Option<Self> {
        (!username.is_empty()
            && username.len() <= 255
            && username
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)))
        .then(|| Self(username.to_owned()))
    }
}

fn client_mode_supported(mode: u32) -> bool {
    // Do not turn a non-root Web request into a setuid/setgid credential mutation.
    mode & 0o111 != 0 && mode & 0o6000 == 0
}

fn password_client(username: &str) -> Result<LocalAccount, PasswordChangeAvailability> {
    use PasswordChangeAvailability::*;
    let account = LocalAccount::parse(username).ok_or(UnsupportedAccountName)?;
    // smbpasswd's root path opens passdb even with -r. Never take that path.
    if rustix::process::getuid().is_root() || rustix::process::geteuid().is_root() {
        return Err(RequiresUnprivilegedService);
    }
    let metadata = std::fs::metadata(CLIENT).map_err(|_| ClientUnavailable)?;
    use std::os::unix::fs::PermissionsExt;
    if !metadata.is_file() || !client_mode_supported(metadata.permissions().mode()) {
        return Err(ClientUnavailable);
    }
    Ok(account)
}

pub(crate) async fn account(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> Result<Json<SmbAccountStatus>, ApiError> {
    state.auth_reader.revalidate_identity(&identity)?;
    if state.config.smb.is_some() && state.smb_account.is_none() {
        return Err(unavailable(
            "SMB account binding requires recovery on the server",
        ));
    }
    Ok(Json(status(state.smb_account.as_deref())?))
}

fn status(
    authority: Option<&crate::smb_account_binding::Authority>,
) -> Result<SmbAccountStatus, ApiError> {
    let Some(authority) = authority else {
        return Ok(SmbAccountStatus::Disabled {});
    };
    let selection = authority.committed_selection()?;
    Ok(SmbAccountStatus::Configured {
        username: selection.username.clone(),
        generation: selection.generation,
        candidates: authority.candidates().to_vec(),
        password_change: match password_client(&selection.username) {
            Ok(_) => PasswordChangeAvailability::Available,
            Err(reason) => reason,
        },
    })
}

// Deliberately no Debug or Serialize: never include secrets in diagnostics.
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct PasswordInput {
    #[serialization(with = "storage_contract::u64_string")]
    expected_generation: u64,
    current_password: String,
    new_password: String,
}

struct PasswordChange {
    expected_generation: u64,
    current: String,
    new: String,
}
impl TryFrom<PasswordInput> for PasswordChange {
    type Error = ApiError;
    fn try_from(input: PasswordInput) -> Result<Self, ApiError> {
        for password in [&input.current_password, &input.new_password] {
            // Samba 4.22 smbpasswd uses a 256-byte fstring for confirmation.
            if password.is_empty() || password.len() > 255 || password.chars().any(char::is_control)
            {
                return Err(ApiError::invalid(
                    "SMB passwords require 1–255 UTF-8 bytes without control characters",
                ));
            }
        }
        Ok(Self {
            expected_generation: input.expected_generation,
            current: input.current_password,
            new: input.new_password,
        })
    }
}

fn unavailable(message: &'static str) -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        storage_contract::ErrorCode::SmbUnavailable,
        message,
    )
}
fn unknown(message: impl Into<String>) -> ApiError {
    let mut error = ApiError::new(
        StatusCode::BAD_GATEWAY,
        storage_contract::ErrorCode::SmbUnavailable,
        message,
    );
    // Samba changes are not operation-idempotent. Verify the effective password
    // before submitting again, even when the transport failure is transient.
    error.retryable = false;
    error
}

pub(crate) async fn change_password(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let ip = http_auth::client_ip(&request, &state.config)?;
    let input = storage_contract::decode_strict::<PasswordInput>(
        &crate::http_boundary::read_body(request, 4096).await?,
    )
    // Decoder errors can contain submitted text, including a secret.
    .map_err(|_| ApiError::invalid("invalid SMB password request"))?;
    let passwords = PasswordChange::try_from(input)?;
    let authority = state.smb_account.clone().ok_or_else(|| {
        unavailable("SMB is not configured or its account binding requires recovery")
    })?;
    http_auth::admit_attempts(
        &state.password_attempts,
        [
            format!("smb-password:ip:{ip}"),
            format!("smb-password:user:{}", identity.principal),
        ],
        "SMB password admission capacity reached",
        "too many SMB password changes; retry after 60 seconds",
    )?;
    let permit = state
        .smb_password_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| {
            ApiError::limit("an SMB password change is already running").with_source(source)
        })?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let binding = authority.read()?;
        binding.require_generation(passwords.expected_generation)?;
        let account = password_client(&binding.selection().username).map_err(|_| unavailable(
            "SMB password changes require a nonprivileged service, a local account name and /usr/bin/smbpasswd",
        ))?;
        let config = client_config()?;
        // Linearize admission and credential dispatch against ZXS logout or
        // password revocation, then release the auth lock during network I/O.
        let mut child = {
            let auth = lock(&state.auth);
            auth.revalidate_identity(&identity)?;
            start_client(Path::new(CLIENT), config.path(), &account, &passwords)?
        };
        drop(passwords);
        finish_client(&mut child, CLIENT_TIMEOUT)
    })
    .await
    .map_err(|source| {
        unknown("SMB password task did not complete; verify credentials before retrying")
            .with_source(source)
    })??;
    Ok(Json(json!({"changed":true})))
}

// The Web administrator password proves intent; this never accepts or stores a
// Samba password, arbitrary account or path. Candidate provisioning is external.
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct AccountInput {
    username: String,
    #[serialization(with = "storage_contract::u64_string")]
    expected_generation: u64,
    administrator_password: String,
}
pub(crate) async fn select_account(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    request: Request,
) -> Result<Json<SmbAccountStatus>, ApiError> {
    let ip = http_auth::client_ip(&request, &state.config)?;
    let input = storage_contract::decode_strict::<AccountInput>(
        &crate::http_boundary::read_body(request, 16384).await?,
    )
    .map_err(|_| ApiError::invalid("invalid SMB account selection request"))?;
    if input.administrator_password.is_empty() || input.administrator_password.len() > 1024 {
        return Err(ApiError::invalid(
            "current Web password must contain 1–1024 UTF-8 bytes",
        ));
    }
    let authority = state.smb_account.clone().ok_or_else(|| {
        unavailable("SMB is not configured or its account binding requires recovery")
    })?;
    if !authority.candidates().contains(&input.username) {
        return Err(ApiError::forbidden());
    }
    http_auth::admit_attempts(
        &state.password_attempts,
        [
            format!("smb-binding:ip:{ip}"),
            format!("smb-binding:user:{}", identity.principal),
        ],
        "SMB account admission capacity reached",
        "too many SMB account changes; retry after 60 seconds",
    )?;
    let mutation_permit = state
        .smb_password_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| {
            ApiError::limit("an SMB account or password change is running").with_source(source)
        })?;
    let hash_permit = state
        .password_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("password verification is busy").with_source(source))?;
    let challenge = state.auth_reader.login_challenge(&identity.principal)?;
    tokio::task::spawn_blocking(move || {
        let _mutation_permit = mutation_permit;
        let _hash_permit = hash_permit;
        let proof = challenge
            .verify(&input.administrator_password)
            .map_err(|_| {
                ApiError::new(
                    StatusCode::FORBIDDEN,
                    storage_contract::ErrorCode::CurrentPasswordInvalid,
                    "current Web password is incorrect",
                )
            })?;
        drop(input.administrator_password);
        authority.select(input.expected_generation, &input.username, || {
            let auth = lock(&state.auth);
            auth.revalidate_password_proof(&identity, &proof)?;
            Ok(auth)
        })?;
        status(Some(&authority)).map(Json)
    })
    .await
    .map_err(|source| {
        unknown("SMB account save did not complete; refresh the selected account before retrying")
            .with_source(source)
    })?
}

fn client_config() -> Result<tempfile::NamedTempFile, ApiError> {
    let mut config = tempfile::NamedTempFile::new()
        .map_err(|source| unavailable("cannot prepare SMB password client").with_source(source))?;
    config
        .write_all(CLIENT_CONFIG.as_bytes())
        .map_err(|source| unavailable("cannot prepare SMB password client").with_source(source))?;
    Ok(config)
}

fn command(program: &Path, config: &Path, account: &LocalAccount) -> Command {
    let mut command = Command::new(program);
    command
        .args(["-s", "-D", "0", "-r", "127.0.0.1", "-U", &account.0, "-c"])
        .arg(config)
        .env_clear()
        .env("LC_ALL", "C.UTF-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn start_client(
    program: &Path,
    config: &Path,
    account: &LocalAccount,
    passwords: &PasswordChange,
) -> Result<Child, ApiError> {
    let mut child = command(program, config, account)
        .spawn()
        .map_err(|source| unavailable("cannot start SMB password client").with_source(source))?;
    let result = {
        let mut input = child
            .stdin
            .take()
            .expect("invariant: child stdin was piped");
        // At most 768 bytes: below the minimum pipe capacity, even if the child
        // has not read yet. Closing stdin prevents further interactive prompts.
        write!(
            input,
            "{}\n{}\n{}\n",
            passwords.current, passwords.new, passwords.new
        )
    };
    if let Err(source) = result {
        stop_client(&mut child)?;
        return Err(unknown(
            "SMB password change was not confirmed; verify credentials before retrying",
        )
        .with_source(source));
    }
    Ok(child)
}

fn stop_client(child: &mut Child) -> Result<(), ApiError> {
    // A process that has already exited may race kill; wait remains mandatory.
    if let Err(source) = child.kill()
        && child
            .try_wait()
            .map_err(|e| unknown("cannot inspect SMB password client").with_source(e))?
            .is_none()
    {
        return Err(unknown("cannot stop SMB password client").with_source(source));
    }
    child
        .wait()
        .map_err(|source| unknown("cannot reap SMB password client").with_source(source))?;
    Ok(())
}

fn finish_client(child: &mut Child, timeout: Duration) -> Result<(), ApiError> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(unknown(format!(
                    "Samba password client {status}; change not confirmed. Check the current password, password policy and local Samba service before retrying"
                )));
            }
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(25))
            }
            result => {
                stop_client(child)?;
                return Err(match result {
                    Err(source) => unknown(
                        "cannot observe SMB password change; verify credentials before retrying",
                    )
                    .with_source(source),
                    _ => {
                        unknown("SMB password change timed out; verify credentials before retrying")
                    }
                });
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_smb_account.rs"]
mod tests;
