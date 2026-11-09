//! First-install runtime directories only. Never clears used runtime state or
//! turns a directory receipt into storage/service readiness evidence.
mod directories;

use super::Input;
use crate::{
    Result,
    account::{LocalAccount, Name, Snapshot},
    private_io::BundleDestination,
};
use data_serialization::{Deserialize, Serialize};
use platform_io::service_lease::HOST_ROOT;
use sha2::{Digest, Sha256};
use std::path::Path;

const MAX_RECORD_BYTES: usize = 16 * 1024;

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Prepare {
    pub input: Input,
    pub request_digest: String,
    pub confirmation: String,
    pub external_administration_stopped: bool,
}

#[derive(Serialize)]
struct Plan {
    format: &'static str,
    revision: u32,
    host_device: String,
    host_inode: String,
    service_account: Snapshot,
    directories: Vec<directories::Policy>,
}

#[derive(Serialize)]
pub(crate) struct Preview {
    plan: Plan,
    pub request_digest: String,
    observed_directories: Vec<directories::Observation>,
    intent_directory: String,
    completion_directory: String,
    completion_phase: directories::CompletionPhase,
    complete_original_receipt_present: bool,
    existing_permissions_changed: bool,
    system_initialized: bool,
    service_started: bool,
    all_external_writers_excluded: bool,
}

#[derive(Serialize)]
struct Receipt {
    format: &'static str,
    revision: u32,
    request_digest: String,
    directories: Vec<directories::Observation>,
    service_started: bool,
    system_initialized: bool,
    complete_installation_ready: bool,
}

#[derive(Serialize)]
pub(crate) struct Outcome {
    state: &'static str,
    receipt: Receipt,
    created_in_this_attempt: Vec<&'static str>,
    intent_directory: String,
    completion_directory: String,
    current_daemon_exclusion: &'static str,
    all_external_writers_excluded: bool,
    existing_permissions_changed: bool,
    samba_passdb_changed: bool,
}

struct Prepared {
    host: BundleDestination,
    account: LocalAccount,
    directories: directories::Directories,
    preview: Preview,
}

impl Prepared {
    fn recheck(&self) -> Result<()> {
        self.host.recheck()?;
        self.account.recheck()?;
        self.directories.recheck()
    }

    fn receipt(&self) -> Result<Receipt> {
        Ok(Receipt {
            format: "zxs-helper-runtime-directories-prepared",
            revision: 1,
            request_digest: self.preview.request_digest.clone(),
            directories: self.directories.completed(&self.preview.request_digest)?,
            service_started: false,
            system_initialized: false,
            complete_installation_ready: false,
        })
    }
}

fn names(digest: &str) -> (String, String) {
    (
        format!(".zxs-helper-runtime-intent-{digest}"),
        format!(".zxs-helper-runtime-complete-{digest}"),
    )
}

fn build(input: Input) -> Result<Prepared> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("runtime directory preparation requires the manually run root helper".into());
    }
    let account = LocalAccount::resolve(Name::new(input.service_user)?)?;
    let host = BundleDestination::open(Path::new(HOST_ROOT))?;
    let directories = directories::Directories::open(Path::new(HOST_ROOT), &host, account.owner())?;
    let identity = host.identity();
    let plan = Plan {
        format: "zxs-helper-runtime-directory-plan",
        revision: 1,
        host_device: identity.device.to_string(),
        host_inode: identity.inode.to_string(),
        service_account: account.snapshot(),
        directories: directories.policies(),
    };
    let request_digest = platform_io::lower_hex(&Sha256::digest(json_codec::to_vec(&plan)?));
    let (intent, completion) = names(&request_digest);
    let observed_directories = directories.inspect(&request_digest)?;
    let prepared = Prepared {
        host,
        account,
        directories,
        preview: Preview {
            plan,
            request_digest,
            observed_directories,
            intent_directory: format!("{HOST_ROOT}/{intent}"),
            completion_directory: format!("{HOST_ROOT}/{completion}"),
            completion_phase: directories::CompletionPhase::Absent,
            complete_original_receipt_present: false,
            existing_permissions_changed: false,
            system_initialized: false,
            service_started: false,
            all_external_writers_excluded: false,
        },
    };
    bind_original(prepared)
}

fn bind_original(mut prepared: Prepared) -> Result<Prepared> {
    let (intent, completion) = names(&prepared.preview.request_digest);
    let original = prepared
        .host
        .read_operation(&intent, "plan.json", MAX_RECORD_BYTES)?;
    if let Some(bytes) = &original
        && *bytes != json_codec::to_vec_pretty(&prepared.preview.plan)?
    {
        return Err(
            "runtime directory intent differs from the exact plan; all material was preserved"
                .into(),
        );
    }
    if prepared
        .preview
        .observed_directories
        .iter()
        .any(|entry| matches!(entry, directories::Observation::OriginalPending { .. }))
        && original.is_none()
    {
        return Err(
            "runtime pending directories have no complete original intent; nothing was repaired"
                .into(),
        );
    }
    let receipt = prepared
        .host
        .read_operation(&completion, "receipt.json", MAX_RECORD_BYTES)?;
    let phase = prepared.directories.completion_phase(&completion)?;
    match receipt {
        Some(bytes) => {
            if original.is_none() || bytes != json_codec::to_vec_pretty(&prepared.receipt()?)? {
                return Err("completed runtime preparation no longer matches its exact original directories; missing or changed directories are not recreated".into());
            }
            prepared.preview.complete_original_receipt_present = true;
        }
        None => match phase {
            directories::CompletionPhase::Absent => {}
            directories::CompletionPhase::PendingBundle => {
                if original.is_none() {
                    return Err(
                        "runtime completion started without its complete original intent".into(),
                    );
                }
                // Completion staging is created only after every path exists.
                // Its absence of a receipt is not authority to recreate paths.
                prepared
                    .directories
                    .completed(&prepared.preview.request_digest)?;
            }
            directories::CompletionPhase::FinalBundle => {
                return Err("published runtime completion is missing its receipt; preserve the damaged record and all directories".into());
            }
        },
    }
    prepared.preview.completion_phase = phase;
    prepared.recheck()?;
    Ok(prepared)
}

pub(crate) fn preview(input: Input) -> Result<Preview> {
    Ok(build(input)?.preview)
}

pub(crate) fn prepare(input: Prepare) -> Result<Outcome> {
    if input.confirmation != "prepare-empty-runtime-only" || !input.external_administration_stopped
    {
        return Err("explicit prepare-empty-runtime-only confirmation and acknowledgment that external service/host administration is stopped are required before inspecting or writing paths; that acknowledgment is not an exclusion proof".into());
    }
    let prepared = build(input.input)?;
    if input.request_digest != prepared.preview.request_digest {
        return Err("service identity, identity source or host root differs from the exact runtime preview; no directory write was attempted".into());
    }
    publish(&prepared).map_err(|source| {
        Box::new(Pending {
            digest: prepared.preview.request_digest.clone(),
            intent: prepared.preview.intent_directory.clone(),
            source,
        }) as Box<dyn std::error::Error + Send + Sync>
    })
}

fn publish(prepared: &Prepared) -> Result<Outcome> {
    prepared.recheck()?;
    let digest = &prepared.preview.request_digest;
    let (intent, completion) = names(digest);
    let bytes = json_codec::to_vec_pretty(&prepared.preview.plan)?;
    prepared.host.publish(&intent, &[("plan.json", &bytes)])?;
    prepared.recheck()?;
    let created = if prepared.preview.completion_phase != directories::CompletionPhase::Absent {
        prepared.directories.sync_completed(digest)?;
        Vec::new()
    } else {
        prepared
            .directories
            .publish(digest, || prepared.recheck())?
    };
    prepared.recheck()?;
    let receipt = prepared.receipt()?;
    let bytes = json_codec::to_vec_pretty(&receipt)?;
    prepared
        .host
        .publish(&completion, &[("receipt.json", &bytes)])?;
    prepared.recheck()?;
    if json_codec::to_vec_pretty(&prepared.receipt()?)? != bytes {
        return Err("runtime directories changed after the completion receipt; it is not current readiness evidence".into());
    }
    Ok(Outcome {
        state: "empty_runtime_directories_durably_prepared_not_started",
        receipt,
        created_in_this_attempt: created,
        intent_directory: prepared.preview.intent_directory.clone(),
        completion_directory: prepared.preview.completion_directory.clone(),
        current_daemon_exclusion: "held_during_this_operation_only",
        all_external_writers_excluded: false,
        existing_permissions_changed: false,
        samba_passdb_changed: false,
    })
}

#[derive(Debug)]
struct Pending {
    digest: String,
    intent: String,
    source: Box<dyn std::error::Error + Send + Sync>,
}
impl std::fmt::Display for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "runtime preparation {} is incomplete or publication is uncertain; preserve {} and all pending/final directories, retry only the original request; no rollback or service start is claimed",
            self.digest, self.intent
        )
    }
}
impl std::error::Error for Pending {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

#[cfg(test)]
#[path = "../../../test/helper/unit/initialization/runtime_tests.rs"]
mod tests;
