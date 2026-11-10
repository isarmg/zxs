//! Resume the exact recorded configuration installation.
use super::{Input, Prepared};
use crate::{
    Result,
    private_io::{BundleDestination, BundleState},
};
use data_serialization::{Deserialize, Serialize};
use std::path::Path;

const MAX_RECORD_BYTES: usize = 64 * 1024;
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Original {
    pub request_digest: String,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Commit {
    pub request_digest: String,
    pub confirmation: String,
    pub external_administration_stopped: bool,
}
#[derive(Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "zxs-helper-original-configuration-installation")]
    OriginalConfiguration,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Record {
    format: Format,
    revision: u32,
    request_digest: String,
    input: Input,
}
#[derive(Clone, Copy, Serialize)]
#[serialization(rename_all = "snake_case")]
pub(super) enum RecoveryState {
    NotRecorded,
    OriginalRequestRecorded,
    IntentPreparationPending,
    ConfigurationFilesPending,
    CompletionPublicationPending,
    CompletedConfigurationFiles,
}
#[derive(Serialize)]
pub(crate) struct Preview {
    pub request_digest: String,
    state: RecoveryState,
    original_input: Input,
    installation: super::Preview,
    pub continuation_available: bool,
    service_started_or_enabled: bool,
    complete_installation_ready: bool,
}
fn require_digest(digest: &str) -> Result<()> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("the exact 64-character lowercase original configuration digest is required before host access".into());
    }
    Ok(())
}
pub(super) fn request_name(digest: &str) -> String {
    format!(".zxs-helper-config-request-{digest}")
}
pub(super) fn record_bytes(prepared: &Prepared) -> Result<Vec<u8>> {
    Ok(json_codec::to_vec_pretty(&Record {
        format: Format::OriginalConfiguration,
        revision: 2,
        request_digest: prepared.preview.request_digest.clone(),
        input: prepared.original_input.clone(),
    })?)
}
fn read_original(host: &BundleDestination, digest: &str) -> Result<Input> {
    let bytes = host.read_operation(&request_name(digest), "request.json", MAX_RECORD_BYTES)?
        .ok_or("the complete original configuration request is unavailable; preserve any pending or older-format material, do not reconstruct it from current files")?;
    let record: Record = json_codec::from_slice(&bytes)?;
    if record.revision != 2 || record.request_digest != digest {
        return Err("unsupported or mismatched original configuration request; no compatibility conversion was attempted".into());
    }
    Ok(record.input)
}
fn recover(digest: &str) -> Result<Prepared> {
    require_digest(digest)?;
    super::require_root()?;
    // Keep the host lease while loading inputs and opening the remaining
    // resources. Reopening it inside build would contend with our own lease.
    let host = BundleDestination::open(Path::new("/zxs"))?;
    let input = read_original(&host, digest)?;
    let prepared = super::build_at(input, host)?;
    if prepared.preview.request_digest != digest {
        return Err("original configuration, payload, accounts or parent identities no longer match; no new plan was substituted".into());
    }
    Ok(prepared)
}

fn ordered_state(
    request: BundleState,
    intent: BundleState,
    completion: BundleState,
    present: &[bool],
) -> Result<RecoveryState> {
    use BundleState::{Absent, Pending, Published};
    if request != Published && (intent != Absent || completion != Absent) {
        return Err("configuration material exists without its published original request; preserve the inconsistent records".into());
    }
    if completion != Absent && intent != Published {
        return Err("configuration completion exists without its complete published intent; no material was reconstructed".into());
    }
    if completion != Absent && present.iter().any(|item| !item) {
        return Err("a configuration file is missing after completion publication began; the original installation cannot recreate it".into());
    }
    Ok(match (request, intent, completion) {
        (_, _, Published) => RecoveryState::CompletedConfigurationFiles,
        (_, _, Pending) => RecoveryState::CompletionPublicationPending,
        (_, Published, Absent) => RecoveryState::ConfigurationFilesPending,
        (_, Pending, Absent) => RecoveryState::IntentPreparationPending,
        (Published | Pending, Absent, Absent) => RecoveryState::OriginalRequestRecorded,
        (Absent, Absent, Absent) => RecoveryState::NotRecorded,
    })
}
/// Inspect all retained stages before any write, including ordinary same-plan
/// retries. Published evidence and completion-stage missing files are not healed.
pub(super) fn check_records(prepared: &Prepared) -> Result<RecoveryState> {
    prepared.recheck()?;
    let plan = &prepared.preview.plan;
    let digest = &prepared.preview.request_digest;
    let original = record_bytes(prepared)?;
    let request = prepared.host.inspect(
        &request_name(digest),
        &[("request.json", original.as_slice())],
    )?;
    let manifest = json_codec::to_vec_pretty(plan)?;
    let mut files = vec![
        ("plan.json", manifest.as_slice()),
        ("zxs.service", plan.unit.content.as_bytes()),
    ];
    if let Some(samba) = &plan.samba {
        files.push(("zxs.conf", samba.content.as_bytes()));
    }
    let intent = prepared
        .host
        .inspect(&format!(".zxs-helper-config-intent-{digest}"), &files)?;
    let receipt = json_codec::to_vec_pretty(&super::completion_receipt(prepared))?;
    let completion = prepared.host.inspect(
        &format!(".zxs-helper-config-installed-{digest}"),
        &[("receipt.json", receipt.as_slice())],
    )?;
    let present = prepared.destinations.inspect(
        plan.unit.content.as_bytes(),
        plan.samba.as_ref().map(|file| file.content.as_bytes()),
    )?;
    prepared.recheck()?;
    ordered_state(request, intent, completion, &present)
}
pub(crate) fn preview(original: Original) -> Result<Preview> {
    let prepared = recover(&original.request_digest)?;
    let state = check_records(&prepared)?;
    Ok(Preview {
        request_digest: original.request_digest,
        state,
        original_input: prepared.original_input,
        installation: prepared.preview,
        continuation_available: !matches!(state, RecoveryState::CompletedConfigurationFiles),
        service_started_or_enabled: false,
        complete_installation_ready: false,
    })
}
pub(crate) fn commit(input: Commit) -> Result<json_codec::Value> {
    if input.confirmation != "resume-original-configuration-files-only"
        || !input.external_administration_stopped
    {
        return Err("explicit original-configuration continuation and stopped external-administration acknowledgments are required before any host access".into());
    }
    let prepared = recover(&input.request_digest)?;
    super::publish_pending(&prepared)
}

#[cfg(test)]
#[path = "../../../../test/helper/unit/deployment/configuration_recovery_tests.rs"]
mod tests;
