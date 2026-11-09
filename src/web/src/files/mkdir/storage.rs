//! Immutable browser hints. A local record never proves server publication.
use super::*;
use data_serialization::{Deserialize, Serialize};
use storage_contract::history_window::HistoryRepositoryId;

const PREFIX: &str = "zxs:folder-create:";
pub(super) const MAX_RECORDS: usize = 32;
const MAX_RECORD_BYTES: usize = 32 * 1024;
const MAX_STORAGE_KEYS: u32 = 4096;

#[derive(Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "folder-create-original-request")]
    OriginalRequest,
}
#[derive(Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Namespace {
    UninitializedRoot,
    Repository {
        repository: HistoryRepositoryId,
        path: name_policy::PortablePath,
    },
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Stored {
    format: Format,
    principal: String,
    operation: OperationId,
    slot: String,
    path: name_policy::PortablePath,
    namespace: Namespace,
}

pub(super) struct Record {
    stored: Stored,
    pub(super) bytes: Arc<str>,
    digest: Option<String>,
}
impl Record {
    pub(super) fn operation(&self) -> OperationId {
        self.stored.operation
    }
    pub(super) fn principal(&self) -> &str {
        &self.stored.principal
    }
    pub(super) fn label(&self) -> String {
        format!(
            "{}/{} · {}",
            self.stored.slot,
            self.stored.path.as_str(),
            self.operation()
        )
    }
    pub(super) fn can_lookup(&self) -> bool {
        self.digest.is_some()
    }
    fn key(&self) -> String {
        format!("{}{}", prefix(self.principal()), self.operation())
    }
    pub(super) fn request(&self) -> (String, Value) {
        (
            format!(
                "/api/file-manager/disks/{}/namespace",
                api::segment(&self.stored.slot)
            ),
            json!({
                "kind":"mkdir", "path":self.stored.path.as_str(),
                "operation_id":self.operation(),
            }),
        )
    }
    pub(super) fn matches_receipt(&self, receipt: &Value, origin: ReceiptOrigin) -> bool {
        let path = match &self.stored.namespace {
            Namespace::UninitializedRoot => self.stored.path.as_str(),
            Namespace::Repository { path, .. } => path.as_str(),
        };
        receipt["operation_kind"] == "file.namespace"
            && receipt["result"]["action"] == "mkdir"
            && receipt["result"]["path"].as_str() == Some(path)
            && match &self.digest {
                Some(digest) => receipt["request_digest"].as_str() == Some(digest.as_str()),
                // The first empty disk has no repository ID until the server
                // creates its root. Only the exact original POST can bind its
                // receipt; a generic ID lookup cannot prove the missing input.
                None => origin == ReceiptOrigin::OriginalPost,
            }
    }
    pub(super) fn capture(
        state: &State,
        operation: OperationId,
        name: &str,
    ) -> Result<Self, api::Error> {
        let path = name_policy::PortablePath::parse(&super::super::model::join_path(
            &state.files.directory,
            name,
        ))
        .map_err(|source| decode_error(source.into()))?;
        let namespace = if state.files.repository_id.is_empty() {
            if !state.files.directory.is_empty() || !state.files.repository_path.is_empty() {
                return Err(failure(
                    "The folder listing has no admitted repository identity.",
                ));
            }
            Namespace::UninitializedRoot
        } else {
            Namespace::Repository {
                repository: HistoryRepositoryId::parse(&state.files.repository_id)
                    .map_err(decode_error)?,
                path: name_policy::PortablePath::parse(&super::super::model::join_path(
                    &state.files.repository_path,
                    name,
                ))
                .map_err(|source| decode_error(source.into()))?,
            }
        };
        let stored = Stored {
            format: Format::OriginalRequest,
            principal: state.principal.clone(),
            operation,
            slot: state.files.slot.clone(),
            path,
            namespace,
        };
        let bytes = storage_contract::canonical_json(&stored).map_err(decode_error)?;
        let bytes = String::from_utf8(bytes).expect("invariant: canonical JSON is UTF-8");
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(limit(
                "The original folder request exceeds 32 KiB; no request was sent.",
            ));
        }
        Self::bind(stored, Arc::from(bytes))
    }
    fn decode(key: &str, principal: &str, bytes: &str) -> Result<Self, api::Error> {
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(limit(
                "A folder recovery record exceeds 32 KiB; it was preserved.",
            ));
        }
        if !bytes.trim_start().starts_with('{') {
            return Err(failure("Folder recovery must be a named request object."));
        }
        let stored: Stored = storage_contract::StrictJson::parse(bytes.as_bytes())
            .and_then(|json| json.decode::<Stored>())
            .map_err(decode_error)?;
        if stored.principal != principal {
            return Err(failure("The folder recovery account does not match."));
        }
        let record = Self::bind(stored, Arc::from(bytes))?;
        if record.key() != key {
            return Err(failure(
                "The folder operation differs from its immutable storage key.",
            ));
        }
        Ok(record)
    }
    fn bind(stored: Stored, bytes: Arc<str>) -> Result<Self, api::Error> {
        if stored.principal.is_empty() || stored.principal.len() > 1024 {
            return Err(failure("The folder recovery account is invalid."));
        }
        name_policy::validate_slot(&stored.slot).map_err(|source| decode_error(source.into()))?;
        let digest = match &stored.namespace {
            Namespace::UninitializedRoot => {
                name_policy::validate_name(stored.path.as_str())
                    .map_err(|source| decode_error(source.into()))?;
                None
            }
            Namespace::Repository { repository, path } => {
                // A named repository removes exactly one displayed component;
                // a disk-root repository keeps the complete displayed path.
                if path.as_str() != stored.path.as_str()
                    && stored.path.as_str().split_once('/').map(|(_, tail)| tail)
                        != Some(path.as_str())
                {
                    return Err(failure(
                        "The original folder paths do not share one namespace.",
                    ));
                }
                let change = storage_contract::NamespaceOperation::Mkdir { path: path.clone() };
                Some(
                    storage_contract::object_hash(
                        "request",
                        &json!({
                            "kind":"file.namespace", "principal":stored.principal,
                            "body":{"repository_id":repository.as_str(),"change":change},
                        }),
                    )
                    .map_err(decode_error)?,
                )
            }
        };
        Ok(Self {
            stored,
            bytes,
            digest,
        })
    }
}
fn decode_error(source: storage_contract::Error) -> api::Error {
    let mut error = failure("The original folder request is invalid; it was preserved.");
    error.source = Some(Arc::new(api::ErrorSource::FolderCreationRecovery(source)));
    error
}
fn prefix(principal: &str) -> String {
    format!("{PREFIX}{}:", api::segment(principal))
}
fn storage() -> Result<web_sys::Storage, api::Error> {
    web_sys::window()
        .ok_or_else(|| failure("Browser storage is unavailable; no folder request was sent."))?
        .local_storage()
        .map_err(api::js_error)?
        .ok_or_else(|| failure("Browser storage is unavailable; no folder request was sent."))
}
pub(super) fn load(principal: &str) -> Result<Vec<Arc<Record>>, api::Error> {
    let storage = storage()?;
    let count = storage.length().map_err(api::js_error)?;
    if count > MAX_STORAGE_KEYS {
        return Err(limit(
            "Folder recovery exceeds the local storage scan budget.",
        ));
    }
    let prefix = prefix(principal);
    let mut records = Vec::new();
    for index in 0..count {
        let Some(key) = storage.key(index).map_err(api::js_error)? else {
            continue;
        };
        if !key.starts_with(&prefix) {
            continue;
        }
        let Some(bytes) = storage.get_item(&key).map_err(api::js_error)? else {
            continue;
        };
        if records.len() == MAX_RECORDS {
            return Err(limit(
                "Folder recovery exceeds 32 operations; nothing was removed.",
            ));
        }
        records.push(Arc::new(Record::decode(&key, principal, &bytes)?));
    }
    records.sort_by_key(|record| record.operation());
    Ok(records)
}
pub(super) fn save(record: &Record) -> Result<(), api::Error> {
    if !load(record.principal())?.is_empty() {
        return Err(failure(
            "Resolve the original folder creation before starting another.",
        ));
    }
    let storage = storage()?;
    let key = record.key();
    if storage.get_item(&key).map_err(api::js_error)?.is_some() {
        return Err(failure(
            "The folder operation key is occupied; it was not overwritten.",
        ));
    }
    storage
        .set_item(&key, &record.bytes)
        .map_err(api::js_error)?;
    verify_with(&storage, record)
}
fn verify_with(storage: &web_sys::Storage, record: &Record) -> Result<(), api::Error> {
    match storage.get_item(&record.key()).map_err(api::js_error)? {
        Some(bytes) if bytes == record.bytes.as_ref() => Ok(()),
        _ => Err(failure(
            "The original folder record is missing or changed; no request was sent.",
        )),
    }
}
pub(super) fn verify(record: &Record) -> Result<(), api::Error> {
    verify_with(&storage()?, record)
}
pub(super) fn remove(record: &Record) -> Result<(), api::Error> {
    let storage = storage()?;
    let key = record.key();
    if let Some(bytes) = storage.get_item(&key).map_err(api::js_error)? {
        if bytes != record.bytes.as_ref() {
            return Err(failure(
                "The folder recovery record changed; it was not removed.",
            ));
        }
        storage.remove_item(&key).map_err(api::js_error)?;
    }
    if storage.get_item(&key).map_err(api::js_error)?.is_some() {
        return Err(failure("Local folder recovery cleanup was not confirmed."));
    }
    Ok(())
}
