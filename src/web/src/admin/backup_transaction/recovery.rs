//! Same-origin/account recovery input, never authority or an automatic retry.
use super::*;
use data_serialization::{Deserialize, Serialize};
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};

const MAX_RECORD_BYTES: usize = wire::MAX_REQUEST_BYTES + 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "original-backup-transaction")]
    Original,
    #[serialization(rename = "cancelling-backup-transaction")]
    Cancelling,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Record {
    format: Format,
    pub(super) operation_id: OperationId,
    /// Needed when the first start never reached the server before reload.
    pub(super) original_start: Option<wire::StartRequest>,
}

impl Record {
    pub(super) fn cancelling(&self) -> bool {
        self.format == Format::Cancelling
    }
}

pub(super) enum Intent {
    Continue,
    Cancel,
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

pub(super) async fn remember(
    principal: &str,
    operation: OperationId,
    original_start: Option<wire::StartRequest>,
    intent: Intent,
) -> Result<(), api::Error> {
    exclusive(principal, Action::Remember { operation, original_start, intent }).await
}

pub(super) async fn clear_cancelled(
    principal: &str,
    original: wire::OriginalRequest,
) -> Result<(), api::Error> {
    exclusive(principal, Action::ClearCancelled(original)).await
}

pub(super) async fn reject_cancellation(
    principal: &str,
    original: wire::OriginalRequest,
) -> Result<(), api::Error> {
    exclusive(principal, Action::RejectCancellation(original)).await
}

enum Action {
    Remember {
        operation: OperationId,
        original_start: Option<wire::StartRequest>,
        intent: Intent,
    },
    ClearCancelled(wire::OriginalRequest),
    RejectCancellation(wire::OriginalRequest),
}

fn write(storage: &web_sys::Storage, key: &str, record: &Record) -> Result<(), api::Error> {
    let bytes = json_codec::to_string(record).expect("invariant: typed recovery input serializes");
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(failure());
    }
    storage.set_item(key, &bytes).map_err(api::js_error)?;
    if storage.get_item(key).map_err(api::js_error)?.as_deref() != Some(bytes.as_str()) {
        return Err(failure());
    }
    Ok(())
}

fn matches_original(record: &Record, original: &wire::OriginalRequest) -> bool {
    record.operation_id == original.operation_id
        && record.original_start.as_ref().is_none_or(|start| original_request(start) == *original)
}

fn apply_locked(key: &str, action: Action) -> Result<(), api::Error> {
    let storage = storage()?;
    let previous = storage.get_item(key).map_err(api::js_error)?
        .as_deref().map(decode_record).transpose()?;
    match action {
        Action::Remember { operation, original_start, intent } => {
            let mut record = retain(previous, operation, original_start.as_ref())?;
            match intent {
                Intent::Cancel => record.format = Format::Cancelling,
                Intent::Continue if record.cancelling() => return Err(failure()),
                Intent::Continue => {}
            }
            write(&storage, key, &record)
        }
        Action::ClearCancelled(original) => {
            if let Some(record) = previous {
                if !matches_original(&record, &original) {
                    return Err(failure());
                }
                storage.remove_item(key).map_err(api::js_error)?;
            }
            if storage.get_item(key).map_err(api::js_error)?.is_some() {
                return Err(failure());
            }
            Ok(())
        }
        Action::RejectCancellation(original) => {
            let Some(mut record) = previous else {
                return Err(failure());
            };
            if !matches_original(&record, &original) {
                return Err(failure());
            }
            record.format = Format::Original;
            write(&storage, key, &record)
        }
    }
}

/// The same origin/account lock covers every writer and compare-and-remove.
/// A missing lock facility is a hard admission failure, never an unlocked fallback.
async fn exclusive(principal: &str, action: Action) -> Result<(), api::Error> {
    let window = web_sys::window().ok_or_else(failure)?;
    let navigator = js_sys::Reflect::get(window.as_ref(), &JsValue::from_str("navigator"))
        .map_err(api::js_error)?;
    let locks = js_sys::Reflect::get(&navigator, &JsValue::from_str("locks"))
        .map_err(api::js_error)?;
    if locks.is_null() || locks.is_undefined() {
        return Err(api::error(storage_contract::ErrorCode::RecoveryRequired, t(
            "此浏览器必须支持 Web Locks 才能保存或清理原备份请求；未修改本机记录或提交服务请求。",
            "Web Locks are required to retain or clean up the original backup request. No local record or service mutation was changed.",
        )));
    }
    let request = js_sys::Reflect::get(&locks, &JsValue::from_str("request"))
        .map_err(api::js_error)?.dyn_into::<js_sys::Function>().map_err(api::js_error)?;
    let key = key(principal);
    let name = JsValue::from_str(&key);
    let result = Rc::new(RefCell::new(None));
    let completed = Rc::clone(&result);
    let mut action = Some(action);
    let callback = Closure::wrap(Box::new(move |_lock: JsValue| {
        let action = action.take().expect("invariant: Web Locks invokes its granted callback once");
        *completed.borrow_mut() = Some(apply_locked(&key, action));
        JsValue::UNDEFINED
    }) as Box<dyn FnMut(JsValue) -> JsValue>);
    let promise = request.call2(&locks, &name, callback.as_ref()).map_err(api::js_error)?
        .dyn_into::<js_sys::Promise>().map_err(api::js_error)?;
    let granted = js_sys::futures::JsFuture::from(promise).await;
    drop(callback);
    granted.map_err(api::js_error)?;
    result.take().expect("invariant: resolved Web Lock request completed its callback")
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/admin/backup_transaction_recovery.rs"]
mod tests;
