//! Same-origin/account recovery input, never authority or an automatic retry.
use super::*;
use data_serialization::{Deserialize, Serialize};

const MAX_RECORD_BYTES: usize = wire::MAX_REQUEST_BYTES + 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "original-backup-transaction")]
    Original,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Record {
    format: Format,
    pub(super) operation_id: OperationId,
    /// Needed when the first start never reached the server before reload.
    pub(super) original_start: Option<wire::StartRequest>,
}

fn key(principal: &str) -> String {
    format!("zxs-backup-transaction:{}", api::segment(principal))
}
fn failure() -> api::Error {
    api::error(
        storage_contract::ErrorCode::RecoveryRequired,
        t(
            "无法可靠保存或读取原备份操作及输入；尚未发送新写请求。保留原记录并检查浏览器存储。",
            "The original backup operation and input could not be reliably saved or read. No new mutation was sent. Preserve the record and check browser storage.",
        ),
    )
}
fn storage() -> Result<web_sys::Storage, api::Error> {
    web_sys::window()
        .ok_or_else(failure)?
        .local_storage()
        .map_err(api::js_error)?
        .ok_or_else(failure)
}
fn validate_record(record: &Record) -> Result<(), api::Error> {
    if let Some(original) = &record.original_start {
        if original.operation_id != record.operation_id {
            return Err(failure());
        }
        validate_request(&original_request(original)).map_err(|_| failure())?;
    }
    Ok(())
}
fn decode_record(bytes: &str) -> Result<Record, api::Error> {
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(failure());
    }
    let record = storage_contract::decode_strict::<Record>(bytes.as_bytes()).map_err(|source| {
        let mut error = failure();
        error.source = Some(std::sync::Arc::new(api::ErrorSource::BackupInput(source)));
        error
    })?;
    validate_record(&record)?;
    Ok(record)
}

fn retain(
    previous: Option<Record>,
    operation_id: OperationId,
    original_start: Option<&wire::StartRequest>,
) -> Result<Record, api::Error> {
    let mut record = match previous {
        Some(record) if record.operation_id == operation_id => record,
        Some(_) => return Err(failure()),
        None => Record {
            format: Format::Original,
            operation_id,
            original_start: None,
        },
    };
    if let Some(original) = original_start {
        match &record.original_start {
            Some(retained) if retained != original => return Err(failure()),
            Some(_) => {}
            None => record.original_start = Some(original.clone()),
        }
    }
    validate_record(&record)?;
    Ok(record)
}

pub(super) fn load(principal: &str) -> Result<Option<Record>, api::Error> {
    storage()?
        .get_item(&key(principal))
        .map_err(api::js_error)?
        .as_deref()
        .map(decode_record)
        .transpose()
}

pub(super) fn remember(
    principal: &str,
    operation: OperationId,
    original_start: Option<&wire::StartRequest>,
) -> Result<(), api::Error> {
    let storage = storage()?;
    let key = key(principal);
    let previous = storage
        .get_item(&key)
        .map_err(api::js_error)?
        .as_deref()
        .map(decode_record)
        .transpose()?;
    let record = retain(previous, operation, original_start)?;
    let bytes = json_codec::to_string(&record).expect("invariant: typed recovery input serializes");
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(failure());
    }
    storage.set_item(&key, &bytes).map_err(api::js_error)?;
    if storage.get_item(&key).map_err(api::js_error)?.as_deref() != Some(bytes.as_str()) {
        return Err(failure());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/admin/backup_transaction_recovery.rs"]
mod tests;
