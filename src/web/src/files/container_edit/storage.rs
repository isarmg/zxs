//! Immutable browser recovery hints. No credentials or local completion claims.
use super::{CommitPreparation, CommitPreparationError, Target, validate_frozen};
use crate::{api, files::recovery_storage::Store};
use data_serialization::{Deserialize, Serialize};
use storage_contract::{EntryId, Generation, OperationId, Principal};
use storage_contract::file_container_tags::ReadRequest;

const PREFIX: &str = "zxs:container-draft-commit:";
const MAX_RECORD_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
enum Format {
    OriginalIntent,
    FrozenEnvelope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Intent {
    principal: Principal,
    request: ReadRequest,
    entry: EntryId,
    generation: Generation,
    draft: OperationId,
    operation: OperationId,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Record {
    format: Format,
    intent: Intent,
    envelope: Option<json_codec::Value>,
}
fn invalid() -> api::Error {
    CommitPreparationError::RecoveryBinding.into()
}
fn prefix(principal: &Principal) -> String {
    format!("{PREFIX}{}:", api::segment(principal.as_str()))
}
fn key(intent: &Intent, format: &Format) -> String {
    let phase = match format {
        Format::OriginalIntent => "intent",
        Format::FrozenEnvelope => "envelope",
    };
    format!("{}{}:{phase}", prefix(&intent.principal), intent.operation)
}
fn intent(value: &CommitPreparation) -> Result<Intent, api::Error> {
    Ok(Intent {
        principal: value.principal.clone(),
        request: value.target.request.clone(),
        entry: value.target.entry.parse().map_err(CommitPreparationError::Contract)?,
        generation: value.generation,
        draft: value.draft,
        operation: value.operation,
    })
}
fn encode(value: &CommitPreparation, format: Format) -> Result<(String, String), api::Error> {
    let intent = intent(value)?;
    let envelope = match format {
        Format::OriginalIntent => None,
        Format::FrozenEnvelope => Some(value.envelope.clone().ok_or_else(invalid)?),
    };
    let key = key(&intent, &format);
    let record = Record {
        format,
        intent,
        envelope,
    };
    let bytes = json_codec::to_string(&record).map_err(CommitPreparationError::Stored)?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(CommitPreparationError::RecoveryLimit.into());
    }
    Ok((key, bytes))
}
fn decode(key_value: &str, principal: &Principal, bytes: &str) -> Result<Record, api::Error> {
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(CommitPreparationError::RecoveryLimit.into());
    }
    let record: Record = json_codec::from_str(bytes).map_err(CommitPreparationError::Stored)?;
    if &record.intent.principal != principal || key(&record.intent, &record.format) != key_value {
        return Err(invalid());
    }
    match (&record.format, &record.envelope) {
        (Format::OriginalIntent, None) => {}
        (Format::FrozenEnvelope, Some(body)) => {
            validate_frozen(body, &recovered(&record.intent, 0, Some(body.clone())))?;
        }
        _ => return Err(invalid()),
    }
    Ok(record)
}
fn recovered(
    intent: &Intent,
    epoch: u64,
    envelope: Option<json_codec::Value>,
) -> CommitPreparation {
    CommitPreparation {
        principal: intent.principal.clone(),
        target: Target {
            epoch,
            navigation: 0,
            entry: intent.entry.to_string(),
            request: intent.request.clone(),
        },
        generation: intent.generation,
        draft: intent.draft,
        operation: intent.operation,
        commit_started: envelope.is_some(),
        envelope,
        in_flight: false,
        settled: None,
        error: String::new(),
    }
}
fn assemble(records: Vec<Record>, epoch: u64) -> Result<Option<CommitPreparation>, api::Error> {
    let mut original: Option<Intent> = None;
    let mut frozen = None;
    let mut seen_intent = false;
    for record in records {
        if original.as_ref().is_some_and(|intent| intent != &record.intent) {
            return Err(invalid());
        }
        original = Some(record.intent);
        match record.format {
            Format::OriginalIntent if !seen_intent => seen_intent = true,
            Format::FrozenEnvelope if frozen.is_none() => frozen = record.envelope,
            Format::OriginalIntent | Format::FrozenEnvelope => return Err(invalid()),
        }
    }
    // Cleanup removes intent first. An envelope on its own still retains every
    // original field and must only be queried/replayed, never prepared anew.
    Ok(original.map(|intent| recovered(&intent, epoch, frozen)))
}
pub(super) fn load(principal: &Principal, epoch: u64)
    -> Result<Option<CommitPreparation>, api::Error>
{
    let mut records = Vec::new();
    Store::open()?.scan(&prefix(principal), |key, bytes| {
        if records.len() >= 2 {
            return Err(CommitPreparationError::RecoveryLimit.into());
        }
        records.push(decode(key, principal, bytes)?);
        Ok(())
    })?;
    assemble(records, epoch)
}
pub(super) fn save_intent(value: &CommitPreparation) -> Result<(), api::Error> {
    if load(&value.principal, value.target.epoch)?.is_some() {
        return Err(invalid());
    }
    let (key, bytes) = encode(value, Format::OriginalIntent)?;
    Store::open()?.save(&key, &bytes, |actual| Ok(actual == bytes))
}
pub(super) fn save_envelope(value: &CommitPreparation) -> Result<(), api::Error> {
    verify_intent(value)?;
    let (key, bytes) = encode(value, Format::FrozenEnvelope)?;
    Store::open()?.save(&key, &bytes, |actual| Ok(actual == bytes))
}
fn verify_intent(value: &CommitPreparation) -> Result<(), api::Error> {
    let (key, bytes) = encode(value, Format::OriginalIntent)?;
    Store::open()?.verify(&key, |actual| Ok(actual == bytes))
}
pub(super) fn verify(value: &CommitPreparation) -> Result<(), api::Error> {
    if value.envelope.is_none() {
        return verify_intent(value);
    }
    let (key, bytes) = encode(value, Format::FrozenEnvelope)?;
    Store::open()?.verify(&key, |actual| Ok(actual == bytes))
}
pub(super) fn remove(value: &CommitPreparation) -> Result<(), api::Error> {
    let store = Store::open()?;
    let (key, bytes) = encode(value, Format::OriginalIntent)?;
    store.remove(&key, |actual| Ok(actual == bytes))?;
    if value.envelope.is_some() {
        let (key, bytes) = encode(value, Format::FrozenEnvelope)?;
        store.remove(&key, |actual| Ok(actual == bytes))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/container_edit_storage.rs"]
mod tests;
